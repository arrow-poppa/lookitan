// Helper processes for OCR libraries that are better kept out of Lookitan's own process: Chrome
// ScreenAI (one init per process, no shutdown) and OneOCR (its own onnxruntime.dll, which cannot
// share a process with Lookitan's). The helper is this executable started as
// `--ocr-helper <engine> <folder>` (`main`, checked first thing in `run()`, before Tauri), so a crash
// or hang in the library cannot take the app down, and ending the process gives its memory back.
// It is ended after a request over `TIMEOUT` (Chrome's 10 s) or `IDLE` without one, and started
// again by the next lookup.
//
// Protocol on the helper's stdin/stdout, little-endian. Once loaded it writes the library's major
// and minor version and its largest image side (3 × u32). Each request is width, height (u32) and
// the RGBA pixels; each answer a status (0: an answer follows, 1: the library read nothing) and a
// length (u32), then that many bytes, in the engine's own format.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use image::imageops::FilterType;
use image::RgbaImage;
use parking_lot::Mutex;

/// The argument that starts this executable as a helper.
pub const ARG: &str = "--ocr-helper";
const TIMEOUT: Duration = Duration::from_secs(10);
/// Loading a library and its models takes about a second.
const START_TIMEOUT: Duration = Duration::from_secs(30);
pub const IDLE: Duration = Duration::from_secs(5 * 60);

/// The helper process's whole life: `args` are what follows `ARG` (the engine, its folder).
pub fn main(args: &[OsString]) -> i32 {
    let engine = args.first().and_then(|a| a.to_str()).unwrap_or_default();
    let dir = Path::new(args.get(1).map(OsString::as_os_str).unwrap_or_default());
    let served = match engine {
        "screenai" => super::screenai::serve(dir),
        "oneocr" => super::oneocr::serve(dir),
        other => Err(anyhow::anyhow!("unknown OCR helper {other:?}")),
    };
    match served {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("OCR helper {engine}: {e:#}");
            1
        }
    }
}

/// The helper's side: says hello, then answers each request with `read` until stdin closes.
pub fn serve(version: (u32, u32), side: u32, mut read: impl FnMut(u32, u32, &[u8]) -> Option<Vec<u8>>) -> Result<()> {
    let (mut input, mut output) = (std::io::stdin().lock(), std::io::stdout().lock());
    for v in [version.0, version.1, side] {
        output.write_all(&v.to_le_bytes())?;
    }
    output.flush()?;
    loop {
        let mut head = [0u8; 8];
        match input.read_exact(&mut head) {
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            other => other?,
        }
        let w = u32::from_le_bytes(head[0..4].try_into().unwrap());
        let h = u32::from_le_bytes(head[4..8].try_into().unwrap());
        if w == 0 || h == 0 || w > 16_384 || h > 16_384 {
            bail!("bad image size {w}x{h}");
        }
        let mut pixels = vec![0u8; w as usize * h as usize * 4];
        input.read_exact(&mut pixels)?;
        match read(w, h, &pixels) {
            None => {
                output.write_all(&1u32.to_le_bytes())?;
                output.write_all(&0u32.to_le_bytes())?;
            }
            Some(bytes) => {
                output.write_all(&0u32.to_le_bytes())?;
                output.write_all(&(bytes.len() as u32).to_le_bytes())?;
                output.write_all(&bytes)?;
            }
        }
        output.flush()?;
    }
}

struct Running {
    child: Arc<Mutex<Child>>,
    stdin: ChildStdin,
    stdout: ChildStdout,
    side: u32,
    used: Instant,
}

impl Drop for Running {
    fn drop(&mut self) {
        let mut child = self.child.lock();
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Runs `f`, ending the helper if it takes longer than `limit` (its pipes then close, so `f` fails).
fn watched<T>(name: &'static str, child: &Arc<Mutex<Child>>, limit: Duration, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let done = Arc::new(AtomicBool::new(false));
    let (watch, finished) = (child.clone(), done.clone());
    std::thread::spawn(move || {
        std::thread::sleep(limit);
        if !finished.load(Ordering::SeqCst) {
            log::warn!("{name} took longer than {} s; ending its helper", limit.as_secs());
            let _ = watch.lock().kill();
        }
    });
    let result = f();
    done.store(true, Ordering::SeqCst);
    result
}

fn read_u32(r: &mut impl Read) -> Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

/// The app's side of one engine's helper, started with the first request.
pub struct Helper {
    /// The engine's name in the logs, and in `ARG`'s arguments.
    name: &'static str,
    engine: &'static str,
    running: Arc<Mutex<Option<Running>>>,
}

impl Helper {
    pub fn new(name: &'static str, engine: &'static str) -> Self {
        Self { name, engine, running: Arc::new(Mutex::new(None)) }
    }

    fn start(&self, dir: &Path) -> Result<Running> {
        let exe = std::env::current_exe().context("find Lookitan's executable")?;
        let mut command = Command::new(exe);
        command
            .arg(ARG)
            .arg(self.engine)
            .arg(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The libraries log every model they load.
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().with_context(|| format!("start the {} helper", self.name))?;
        let (stdin, mut stdout) = (child.stdin.take().context("helper stdin")?, child.stdout.take().context("helper stdout")?);
        let child = Arc::new(Mutex::new(child));
        let hello = watched(self.name, &child, START_TIMEOUT, || Ok([read_u32(&mut stdout)?, read_u32(&mut stdout)?, read_u32(&mut stdout)?]));
        let [major, minor, side] = match hello {
            Ok(hello) => hello,
            Err(e) => {
                let _ = child.lock().kill();
                return Err(e.context(format!("the {} helper did not start", self.name)));
            }
        };
        log::info!("{} {major}.{minor} ready in its helper, images up to {side} px", self.name);
        Ok(Running { child, stdin, stdout, side: side.max(256), used: Instant::now() })
    }

    /// The library's answer for `img` (starting the helper with its files in `dir` if needed), and
    /// the capture pixels to one of the image it read: larger images are shrunk to the library's
    /// largest side, and its boxes come back in the shrunk image's pixels. `None` when it read
    /// nothing.
    pub fn ask(&self, dir: &Path, img: &RgbaImage) -> Result<(Option<Vec<u8>>, f32)> {
        let mut slot = self.running.lock();
        if slot.is_none() {
            *slot = Some(self.start(dir)?);
            self.end_when_idle();
        }
        let running = slot.as_mut().unwrap();
        let (w, h) = img.dimensions();
        let scale = (running.side as f32 / w.max(h) as f32).min(1.0);
        let shrunk;
        let input = if scale < 1.0 {
            shrunk = image::imageops::resize(img, ((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1), FilterType::Triangle);
            &shrunk
        } else {
            img
        };
        let started = Instant::now();
        let (stdin, stdout) = (&mut running.stdin, &mut running.stdout);
        let answer = watched(self.name, &running.child, TIMEOUT, || {
            stdin.write_all(&input.width().to_le_bytes())?;
            stdin.write_all(&input.height().to_le_bytes())?;
            stdin.write_all(input.as_raw())?;
            stdin.flush()?;
            let status = read_u32(stdout)?;
            let len = read_u32(stdout)? as usize;
            let mut bytes = vec![0u8; len];
            stdout.read_exact(&mut bytes)?;
            Ok((status == 0).then_some(bytes))
        });
        match answer {
            Ok(answer) => {
                running.used = Instant::now();
                log::info!("{} answered in {} ms", self.name, started.elapsed().as_millis());
                Ok((answer, 1.0 / scale))
            }
            Err(e) => {
                // A helper that crashed, hung or broke its pipe starts again with the next request.
                *slot = None;
                Err(e)
            }
        }
    }

    /// Ends the helper (and frees its memory) once it has been idle for `IDLE`.
    fn end_when_idle(&self) {
        let (running, name) = (Arc::downgrade(&self.running), self.name);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(30));
            let Some(running) = running.upgrade() else { return };
            let mut slot = running.lock();
            match slot.as_ref() {
                None => return,
                Some(r) if r.used.elapsed() >= IDLE => {
                    log::info!("{name} idle; ending its helper");
                    *slot = None;
                    return;
                }
                Some(_) => {}
            }
        });
    }

    /// Ends the helper now (before its files are deleted).
    pub fn stop(&self) {
        *self.running.lock() = None;
    }
}
