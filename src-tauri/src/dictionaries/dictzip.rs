//! Random access to a dictionary file that may be compressed with dictzip (`.dz`), so an entry is
//! read without unpacking the whole file. A dictzip file is a gzip file whose header carries a
//! table of chunks (the `RA` extra field), each compressed on its own. A plain gzip file without
//! that table is read from the start, and an uncompressed file is read directly.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, ensure, Context, Result};
use flate2::read::MultiGzDecoder;
use flate2::{Decompress, FlushDecompress};
use parking_lot::Mutex;

const MAX_READ: usize = 64 * 1024 * 1024;

enum Kind {
    Plain,
    /// Chunk table: each chunk's offset in the file and compressed length; `chunk` bytes each
    /// once decompressed (the last one may be shorter).
    DictZip { chunk: usize, chunks: Vec<(u64, usize)> },
    Gzip,
}

pub struct Source {
    path: PathBuf,
    kind: Kind,
    /// The last chunk read, as consecutive reads usually fall in the same one.
    cached: Mutex<Option<(usize, Vec<u8>)>>,
}

fn is_gzip(path: &Path) -> Result<bool> {
    let mut magic = [0u8; 2];
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    Ok(file.read(&mut magic)? == 2 && magic == [0x1f, 0x8b])
}

/// Parses the gzip header and returns the chunk table when it is a dictzip file.
fn chunk_table(path: &Path) -> Result<Option<(usize, Vec<(u64, usize)>)>> {
    let mut file = File::open(path)?;
    let mut head = [0u8; 10];
    file.read_exact(&mut head)?;
    ensure!(head[0] == 0x1f && head[1] == 0x8b && head[2] == 8, "not a gzip file");
    let flags = head[3];
    let mut table = None;
    let mut at = 10u64;
    if flags & 4 != 0 {
        let mut len = [0u8; 2];
        file.read_exact(&mut len)?;
        let xlen = u16::from_le_bytes(len) as usize;
        let mut extra = vec![0u8; xlen];
        file.read_exact(&mut extra)?;
        at += 2 + xlen as u64;
        let mut p = 0;
        while p + 4 <= extra.len() {
            let (id, len) = (&extra[p..p + 2], u16::from_le_bytes([extra[p + 2], extra[p + 3]]) as usize);
            let data = extra.get(p + 4..p + 4 + len).ok_or_else(|| anyhow!("truncated gzip extra field"))?;
            if id == b"RA" && data.len() >= 6 {
                let chunk = u16::from_le_bytes([data[2], data[3]]) as usize;
                let count = u16::from_le_bytes([data[4], data[5]]) as usize;
                ensure!(chunk > 0 && data.len() >= 6 + count * 2, "damaged dictzip chunk table");
                let sizes: Vec<usize> = (0..count).map(|i| u16::from_le_bytes([data[6 + i * 2], data[7 + i * 2]]) as usize).collect();
                table = Some((chunk, sizes));
            }
            p += 4 + len;
        }
    }
    let Some((chunk, sizes)) = table else { return Ok(None) };
    // Skip the file name, comment and header CRC to reach the first chunk.
    let mut byte = [0u8; 1];
    for flag in [8u8, 16] {
        if flags & flag != 0 {
            loop {
                file.read_exact(&mut byte)?;
                at += 1;
                if byte[0] == 0 { break; }
            }
        }
    }
    if flags & 2 != 0 {
        at += 2;
    }
    let mut chunks = Vec::with_capacity(sizes.len());
    for size in sizes {
        chunks.push((at, size));
        at += size as u64;
    }
    Ok(Some((chunk, chunks)))
}

impl Source {
    pub fn open(path: &Path) -> Result<Self> {
        let kind = if !is_gzip(path)? {
            Kind::Plain
        } else if let Some((chunk, chunks)) = chunk_table(path)? {
            Kind::DictZip { chunk, chunks }
        } else {
            Kind::Gzip
        };
        Ok(Self { path: path.to_path_buf(), kind, cached: Mutex::new(None) })
    }

    /// The whole decompressed stream, for indexing it once.
    pub fn stream(path: &Path) -> Result<Box<dyn Read>> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        Ok(if is_gzip(path)? { Box::new(MultiGzDecoder::new(file)) } else { Box::new(file) })
    }

    fn chunk(&self, index: usize, chunk: usize, chunks: &[(u64, usize)]) -> Result<Vec<u8>> {
        if let Some((i, data)) = self.cached.lock().as_ref() {
            if *i == index { return Ok(data.clone()); }
        }
        let (offset, size) = *chunks.get(index).ok_or_else(|| anyhow!("read past the end of {}", self.path.display()))?;
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut packed = vec![0u8; size];
        file.read_exact(&mut packed)?;
        let mut out = Vec::with_capacity(chunk);
        // Each chunk ends with a full flush, so it inflates on its own as raw deflate data.
        let mut inflate = Decompress::new(false);
        inflate.decompress_vec(&packed, &mut out, FlushDecompress::Sync).context("damaged dictzip chunk")?;
        *self.cached.lock() = Some((index, out.clone()));
        Ok(out)
    }

    /// `len` bytes at `offset` of the decompressed file.
    pub fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        ensure!(len <= MAX_READ, "dictionary entry is too large");
        match &self.kind {
            Kind::Plain => {
                let mut file = File::open(&self.path)?;
                file.seek(SeekFrom::Start(offset))?;
                let mut out = vec![0u8; len];
                file.read_exact(&mut out).context("dictionary entry lies past the end of its file")?;
                Ok(out)
            }
            Kind::DictZip { chunk, chunks } => {
                let mut out = Vec::with_capacity(len);
                let mut at = offset;
                while out.len() < len {
                    let index = (at / *chunk as u64) as usize;
                    let data = self.chunk(index, *chunk, chunks)?;
                    let start = (at % *chunk as u64) as usize;
                    if start >= data.len() { bail!("dictionary entry lies past the end of its file"); }
                    let take = (len - out.len()).min(data.len() - start);
                    out.extend_from_slice(&data[start..start + take]);
                    at += take as u64;
                }
                Ok(out)
            }
            Kind::Gzip => {
                let mut stream = Self::stream(&self.path)?;
                std::io::copy(&mut (&mut stream).take(offset), &mut std::io::sink())?;
                let mut out = vec![0u8; len];
                stream.read_exact(&mut out).context("dictionary entry lies past the end of its file")?;
                Ok(out)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};
    use std::io::Write;

    /// Writes `data` as a dictzip file with `chunk`-byte chunks, as dictzip does.
    fn dictzip(path: &Path, data: &[u8], chunk: usize) {
        let mut packed: Vec<Vec<u8>> = Vec::new();
        let mut deflate = Compress::new(Compression::default(), false);
        for piece in data.chunks(chunk) {
            let mut out = Vec::with_capacity(piece.len() + 64);
            deflate.compress_vec(piece, &mut out, FlushCompress::Full).unwrap();
            packed.push(out);
        }
        let mut tail = Vec::with_capacity(16);
        deflate.compress_vec(&[], &mut tail, FlushCompress::Finish).unwrap();
        let mut ra = vec![b'R', b'A'];
        let body_len = 6 + packed.len() * 2;
        ra.extend_from_slice(&(body_len as u16).to_le_bytes());
        ra.extend_from_slice(&1u16.to_le_bytes());
        ra.extend_from_slice(&(chunk as u16).to_le_bytes());
        ra.extend_from_slice(&(packed.len() as u16).to_le_bytes());
        for p in &packed { ra.extend_from_slice(&(p.len() as u16).to_le_bytes()); }
        let mut f = File::create(path).unwrap();
        f.write_all(&[0x1f, 0x8b, 8, 4 | 8, 0, 0, 0, 0, 2, 3]).unwrap();
        f.write_all(&(ra.len() as u16).to_le_bytes()).unwrap();
        f.write_all(&ra).unwrap();
        f.write_all(b"name.dict\0").unwrap();
        for p in &packed { f.write_all(p).unwrap(); }
        f.write_all(&tail).unwrap();
        let crc = { let mut c = flate2::Crc::new(); c.update(data); c.sum() };
        f.write_all(&crc.to_le_bytes()).unwrap();
        f.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
    }

    #[test]
    fn dictzip_plain_and_gzip_files_read_the_same_bytes() {
        let dir = std::env::temp_dir().join(format!("lkt-dictzip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data: Vec<u8> = (0..20_000u32).flat_map(|i| format!("entry {i};").into_bytes()).collect();
        let (dz, plain, gz) = (dir.join("a.dict.dz"), dir.join("a.dict"), dir.join("b.dict.gz"));
        dictzip(&dz, &data, 4096);
        std::fs::write(&plain, &data).unwrap();
        let mut e = flate2::write::GzEncoder::new(File::create(&gz).unwrap(), Compression::default());
        e.write_all(&data).unwrap();
        e.finish().unwrap();
        for path in [&dz, &plain, &gz] {
            let source = Source::open(path).unwrap();
            assert_eq!(source.read_at(0, 10).unwrap(), &data[..10]);
            // Across a chunk boundary, and the last bytes.
            assert_eq!(source.read_at(4090, 20).unwrap(), &data[4090..4110], "{}", path.display());
            let end = data.len() as u64;
            assert_eq!(source.read_at(end - 7, 7).unwrap(), &data[data.len() - 7..]);
            assert!(source.read_at(end - 2, 10).is_err());
            let mut all = Vec::new();
            Source::stream(path).unwrap().read_to_end(&mut all).unwrap();
            assert_eq!(all, data);
        }
        assert!(matches!(Source::open(&dz).unwrap().kind, Kind::DictZip { .. }));
        assert!(matches!(Source::open(&gz).unwrap().kind, Kind::Gzip));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
