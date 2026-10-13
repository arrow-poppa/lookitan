// Windows-only helpers: the foreground app (for per-app profiles), suspending and resuming a
// process ("freeze process"), and a low-level mouse hook for mouse-button shortcuts.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenThread, QueryFullProcessImageNameW, ResumeThread, SuspendThread, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetMessageW, GetWindowTextW, GetWindowThreadProcessId, SetWindowsHookExW,
    MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use crate::settings::{MouseButton, MouseHotkey};

#[derive(Clone, Debug, Default)]
pub struct ForegroundApp {
    pub pid: u32,
    pub executable: String,
    pub title: String,
}

/// The process and title of the foreground window, ignoring Lookitan's own windows.
pub fn foreground_app() -> Option<ForegroundApp> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 || pid == std::process::id() {
            return None;
        }
        let mut title = [0u16; 512];
        let n = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32).max(0) as usize;
        let title = String::from_utf16_lossy(&title[..n]);
        let mut executable = String::new();
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if !process.is_null() {
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0 {
                let full = String::from_utf16_lossy(&buf[..len as usize]);
                executable = full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string();
            }
            CloseHandle(process);
        }
        Some(ForegroundApp { pid, executable, title })
    }
}

fn for_each_thread(pid: u32, mut f: impl FnMut(HANDLE)) {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return;
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut ok = Thread32First(snap, &mut entry) != 0;
        while ok {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !thread.is_null() {
                    f(thread);
                    CloseHandle(thread);
                }
            }
            ok = Thread32Next(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
}

pub fn suspend_process(pid: u32) {
    for_each_thread(pid, |t| unsafe {
        SuspendThread(t);
    });
}

pub fn resume_process(pid: u32) {
    for_each_thread(pid, |t| unsafe {
        ResumeThread(t);
    });
}

// ------------------------------------------------------------------ mouse hook

static HOOK_CONFIG: Mutex<Option<MouseHotkey>> = Mutex::new(None);
static HOOK_CALLBACK: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static HOOK_STARTED: AtomicBool = AtomicBool::new(false);
static PRESSED: AtomicBool = AtomicBool::new(false);
static SWALLOW_UP: AtomicBool = AtomicBool::new(false);
const LONG_PRESS: Duration = Duration::from_millis(450);

fn key_down(vk: u16) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

fn modifiers_match(cfg: &MouseHotkey) -> bool {
    key_down(VK_MENU) == cfg.alt && key_down(VK_CONTROL) == cfg.ctrl && key_down(VK_SHIFT) == cfg.shift
}

/// Maps a hook message to (button, is_down).
fn decode(msg: u32, data: u32) -> Option<(MouseButton, bool)> {
    let x = (data >> 16) as u16;
    match msg {
        WM_LBUTTONDOWN => Some((MouseButton::Left, true)),
        WM_LBUTTONUP => Some((MouseButton::Left, false)),
        WM_RBUTTONDOWN => Some((MouseButton::Right, true)),
        WM_RBUTTONUP => Some((MouseButton::Right, false)),
        WM_MBUTTONDOWN => Some((MouseButton::Middle, true)),
        WM_MBUTTONUP => Some((MouseButton::Middle, false)),
        WM_XBUTTONDOWN => Some((if x == 1 { MouseButton::X1 } else { MouseButton::X2 }, true)),
        WM_XBUTTONUP => Some((if x == 1 { MouseButton::X1 } else { MouseButton::X2 }, false)),
        _ => None,
    }
}

fn fire() {
    std::thread::spawn(|| {
        if let Some(cb) = HOOK_CALLBACK.get() {
            cb();
        }
    });
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = &*(lparam as *const MSLLHOOKSTRUCT);
        let cfg = HOOK_CONFIG.lock().clone();
        if let (Some(cfg), Some((button, down))) = (cfg, decode(wparam as u32, info.mouseData)) {
            if cfg.button != MouseButton::None && button == cfg.button {
                if cfg.long_press {
                    // Hold to trigger: the click itself still reaches the app.
                    if down && modifiers_match(&cfg) {
                        PRESSED.store(true, Ordering::SeqCst);
                        std::thread::spawn(|| {
                            std::thread::sleep(LONG_PRESS);
                            if PRESSED.load(Ordering::SeqCst) {
                                fire();
                            }
                        });
                    } else if !down {
                        PRESSED.store(false, Ordering::SeqCst);
                    }
                } else if down && modifiers_match(&cfg) {
                    // Click to trigger: swallow the press and its release.
                    SWALLOW_UP.store(true, Ordering::SeqCst);
                    fire();
                    return 1;
                } else if !down && SWALLOW_UP.swap(false, Ordering::SeqCst) {
                    return 1;
                }
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

/// Updates the mouse shortcut; the hook thread starts on first use.
pub fn set_mouse_hotkey(cfg: MouseHotkey, on_trigger: impl Fn() + Send + Sync + 'static) {
    *HOOK_CONFIG.lock() = Some(cfg);
    let _ = HOOK_CALLBACK.set(Box::new(on_trigger));
    if HOOK_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
        if hook.is_null() {
            log::warn!("could not install the mouse hook");
            HOOK_STARTED.store(false, Ordering::SeqCst);
            return;
        }
        let mut msg: MSG = std::mem::zeroed();
        // Low-level hooks run on this thread's message loop.
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {}
    });
}
