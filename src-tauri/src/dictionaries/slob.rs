//! Independent Slob reader based on the published format table:
//! https://github.com/itkach/slob#slob-file-format
//! Uses seeks into the source and one decompressed bin, not a dataset-sized map.

use super::{escape_html, ImportInfo, ImportSink};
use anyhow::{bail, ensure, Context, Result};
use encoding_rs::Encoding;
use std::{
    collections::HashMap,
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
};

const MAGIC: &[u8; 8] = b"!-1SLOB\x1f";
const MAX_BIN: usize = 64 * 1024 * 1024;

pub fn supports(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("slob"))
}

pub fn import(path: &Path, sink: &mut dyn ImportSink) -> Result<ImportInfo> {
    let mut source = Slob::open(path)?;
    let mut cached: Option<(u32, Bin)> = None;
    let mut count = 0;
    for index in 0..source.refs.count {
        let reference = source.reference(index)?;
        if reference.key.trim().is_empty() { continue; }
        let mime = source.mime(&reference)?.to_owned();
        let basic = mime.split(';').next().unwrap_or("").trim();
        if !matches!(basic, "text/html" | "application/xhtml+xml" | "text/plain") {
            sink.add_resource(&reference.key, index)?;
            continue;
        }
        if cached.as_ref().map(|(index, _)| *index) != Some(reference.bin) {
            // Drop before allocating the next bin to keep the memory bound stable.
            drop(cached.take());
            cached = Some((reference.bin, source.bin(reference.bin)?));
        }
        let bin = &cached.as_ref().context("Cache Slob vazio")?.1;
        let html = article_html(&reference, &mime, bin)?;
        // Distinct references may intentionally select fragments of the same page.
        // Keep the complete HTML rather than trimming a potentially invalid subtree.
        sink.span(0, u64::from(index), 0);
        sink.add(&reference.key, &html, &[])?;
        count += 1;
    }
    ensure!(count > 0, "Slob não contém artigos text/html ou text/plain");
    Ok(ImportInfo {
        name: source.tags.get("label").cloned().filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().into_owned()),
        format: "slob",
        description: source.tags.get("description").cloned().unwrap_or_else(|| "Aard2/Slob: artigos HTML e recursos locais".into()),
    })
}

fn article_html(reference: &Reference, mime: &str, bin: &Bin) -> Result<String> {
    let basic = mime.split(';').next().unwrap_or("").trim();
    let content = bin.item(reference.item)?;
    let text = decode(content, content_encoding(mime)?)?;
    Ok(if basic == "text/plain" {
        format!("<h2>{}</h2><pre>{}</pre>", escape_html(&reference.key), escape_html(&text))
    } else { text })
}

/// The article of reference `index`, read from its bin.
pub fn article(path: &Path, index: u32) -> Result<String> {
    let mut source = Slob::open(path)?;
    let reference = source.reference(index)?;
    let mime = source.mime(&reference)?.to_owned();
    let bin = source.bin(reference.bin)?;
    article_html(&reference, &mime, &bin)
}

/// Resolve a resource previously indexed by `ImportSink::add_resource`, without
/// walking the Unicode-sorted key directory again on each image/CSS request.
pub fn resource_at(path: &Path, reference: u32) -> Option<Vec<u8>> {
    let mut source = Slob::open(path).ok()?;
    let reference = source.reference(reference).ok()?;
    let bin = source.bin(reference.bin).ok()?;
    Some(bin.item(reference.item).ok()?.to_vec())
}

#[derive(Clone, Copy)]
struct Positions { count: u32, table: u64, data: u64, end: u64 }

struct Reference { key: String, bin: u32, item: u16, _fragment: String }

struct Slob {
    file: File,
    size: u64,
    encoding: &'static Encoding,
    compression: String,
    tags: HashMap<String, String>,
    types: Vec<String>,
    refs: Positions,
    store: Positions,
}

fn byte(file: &mut File) -> Result<u8> {
    let mut bytes = [0]; file.read_exact(&mut bytes)?; Ok(bytes[0])
}
fn short(file: &mut File) -> Result<u16> {
    let mut bytes = [0; 2]; file.read_exact(&mut bytes)?; Ok(u16::from_be_bytes(bytes))
}
fn int(file: &mut File) -> Result<u32> {
    let mut bytes = [0; 4]; file.read_exact(&mut bytes)?; Ok(u32::from_be_bytes(bytes))
}
fn long(file: &mut File) -> Result<u64> {
    let mut bytes = [0; 8]; file.read_exact(&mut bytes)?; Ok(u64::from_be_bytes(bytes))
}
fn bytes(file: &mut File, length: usize) -> Result<Vec<u8>> {
    ensure!(length <= MAX_BIN, "Slob: bloco excede 64 MiB");
    let mut bytes = vec![0; length]; file.read_exact(&mut bytes)?; Ok(bytes)
}
fn tiny(file: &mut File, encoding: &'static Encoding) -> Result<String> {
    let length = usize::from(byte(file)?);
    decode(&bytes(file, length)?, encoding)
}
fn tag_value(file: &mut File, encoding: &'static Encoding) -> Result<String> {
    let length = usize::from(byte(file)?);
    let mut data = bytes(file, length)?;
    if encoding == encoding_rs::UTF_16LE || encoding == encoding_rs::UTF_16BE {
        data.truncate(data.len() & !1);
        while data.ends_with(&[0, 0]) { data.truncate(data.len() - 2); }
    } else {
        while data.last() == Some(&0) { data.pop(); }
    }
    decode(&data, encoding)
}
fn text(file: &mut File, encoding: &'static Encoding) -> Result<String> {
    let length = usize::from(short(file)?);
    decode(&bytes(file, length)?, encoding)
}
fn encoding(label: &str) -> Result<&'static Encoding> {
    let label = label.trim().replace('_', "-");
    Encoding::for_label(label.as_bytes()).with_context(|| format!("Codificação Slob não suportada: {label}"))
}
fn decode(bytes: &[u8], encoding: &'static Encoding) -> Result<String> {
    let (text, errors) = encoding.decode_without_bom_handling(bytes);
    ensure!(!errors, "Texto Slob inválido para {}", encoding.name());
    Ok(text.into_owned())
}
fn content_encoding(mime: &str) -> Result<&'static Encoding> {
    for parameter in mime.split(';').skip(1) {
        if let Some((name, value)) = parameter.split_once('=') {
            if name.trim().eq_ignore_ascii_case("charset") { return encoding(value.trim().trim_matches(['\'', '"'])); }
        }
    }
    Ok(encoding_rs::UTF_8)
}

fn positions(file: &mut File, offset: u64, end: u64) -> Result<Positions> {
    ensure!(offset.checked_add(4).is_some_and(|value| value <= end), "Slob: diretório de posições fora dos limites");
    file.seek(SeekFrom::Start(offset))?;
    let count = int(file)?;
    let table = file.stream_position()?;
    let data = table.checked_add(u64::from(count) * 8).context("Slob: overflow no índice")?;
    ensure!(data <= end, "Slob: índice truncado");
    Ok(Positions { count, table, data, end })
}

impl Slob {
    fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path).with_context(|| format!("Não foi possível abrir {}", path.display()))?;
        let size = file.metadata()?.len();
        let mut magic = [0; 8]; file.read_exact(&mut magic)?;
        ensure!(&magic == MAGIC, "Assinatura Slob inválida");
        file.seek(SeekFrom::Current(16))?; // UUID, not used as a trust boundary.
        let encoding_name = tiny(&mut file, encoding_rs::UTF_8)?;
        let encoding = encoding(&encoding_name)?;
        let compression = tiny(&mut file, encoding)?;
        ensure!(["", "zlib", "bz2", "lzma2"].contains(&compression.as_str()), "Compressão Slob não suportada: {compression}");
        let mut tags = HashMap::new();
        for _ in 0..byte(&mut file)? {
            let key = tiny(&mut file, encoding)?;
            let value = tag_value(&mut file, encoding)?;
            tags.insert(key, value);
        }
        let mut types = Vec::new();
        for _ in 0..byte(&mut file)? { types.push(text(&mut file, encoding)?); }
        ensure!(!types.is_empty(), "Slob sem tipos de conteúdo");
        let _blob_count = int(&mut file)?;
        let store_offset = long(&mut file)?;
        let declared_size = long(&mut file)?;
        ensure!(declared_size == size, "Tamanho Slob divergente: esperado {declared_size}, encontrado {size}. Arquivos divididos devem ser reunidos antes da importação");
        let refs_offset = file.stream_position()?;
        ensure!(refs_offset < store_offset && store_offset < size, "Slob: seções fora dos limites");
        let refs = positions(&mut file, refs_offset, store_offset)?;
        let store = positions(&mut file, store_offset, size)?;
        Ok(Self { file, size, encoding, compression, tags, types, refs, store })
    }

    fn seek_item(&mut self, positions: Positions, index: u32) -> Result<u64> {
        ensure!(index < positions.count, "Slob: item fora do índice");
        self.file.seek(SeekFrom::Start(positions.table + u64::from(index) * 8))?;
        let offset = long(&mut self.file)?;
        let offset = positions.data.checked_add(offset).context("Slob: overflow na posição")?;
        ensure!(offset < positions.end, "Slob: posição fora da seção");
        self.file.seek(SeekFrom::Start(offset))?;
        Ok(offset)
    }

    fn reference(&mut self, index: u32) -> Result<Reference> {
        self.seek_item(self.refs, index)?;
        let key = text(&mut self.file, self.encoding)?;
        let bin = int(&mut self.file)?;
        let item = short(&mut self.file)?;
        let fragment = tiny(&mut self.file, self.encoding)?;
        ensure!(self.file.stream_position()? <= self.refs.end, "Slob: referência fora da seção");
        ensure!(bin < self.store.count, "Slob: referência a bin inexistente");
        Ok(Reference { key, bin, item, _fragment: fragment })
    }

    fn mime(&mut self, reference: &Reference) -> Result<&str> {
        self.seek_item(self.store, reference.bin)?;
        let count = int(&mut self.file)?;
        ensure!(count <= 65536 && u32::from(reference.item) < count, "Slob: item fora do bin");
        self.file.seek(SeekFrom::Current(i64::from(reference.item)))?;
        let type_id = usize::from(byte(&mut self.file)?);
        self.types.get(type_id).map(String::as_str).context("Slob: tipo de conteúdo inválido")
    }

    fn bin(&mut self, index: u32) -> Result<Bin> {
        self.seek_item(self.store, index)?;
        let count = int(&mut self.file)?;
        ensure!(count <= 65536, "Slob: bin com mais de 65536 itens");
        let types = bytes(&mut self.file, count as usize)?;
        ensure!(types.iter().all(|id| usize::from(*id) < self.types.len()), "Slob: tipo de conteúdo inválido");
        let length = int(&mut self.file)? as usize;
        ensure!(length <= MAX_BIN, "Slob: bin comprimido excede 64 MiB");
        ensure!(self.file.stream_position()?.checked_add(length as u64).is_some_and(|end| end <= self.size), "Slob: bin truncado");
        let compressed = bytes(&mut self.file, length)?;
        let data = decompress(&self.compression, &compressed)?;
        ensure!(count as usize * 4 <= data.len(), "Slob: diretório do bin truncado");
        Ok(Bin { count, data })
    }
}

struct Bin { count: u32, data: Vec<u8> }
impl Bin {
    fn item(&self, index: u16) -> Result<&[u8]> {
        ensure!(u32::from(index) < self.count, "Slob: item fora do bin");
        let table = usize::from(index) * 4;
        let offset = self.data.get(table..table + 4).context("Slob: diretório do bin truncado")?;
        let offset = u32::from_be_bytes(offset.try_into()?) as usize;
        let start = (self.count as usize * 4).checked_add(offset).context("Slob: overflow no bin")?;
        let length = self.data.get(start..start.saturating_add(4)).context("Slob: item truncado")?;
        let length = u32::from_be_bytes(length.try_into()?) as usize;
        let start = start.checked_add(4).context("Slob: overflow no item")?;
        let end = start.checked_add(length).context("Slob: overflow no conteúdo")?;
        self.data.get(start..end).context("Slob: conteúdo truncado")
    }
}

struct LimitedOutput(Vec<u8>);
impl Write for LimitedOutput {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if data.len() > MAX_BIN.saturating_sub(self.0.len()) { return Err(std::io::Error::other("Bin Slob descomprimido excede 64 MiB")); }
        self.0.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn decompress(compression: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    ensure!(bytes.len() <= MAX_BIN, "Slob: bin comprimido excede 64 MiB");
    let mut output = LimitedOutput(Vec::new());
    match compression {
        "" => output.write_all(bytes)?,
        "zlib" => { std::io::copy(&mut flate2::read::ZlibDecoder::new(bytes), &mut output)?; }
        "bz2" => { std::io::copy(&mut bzip2::read::BzDecoder::new(bytes), &mut output)?; }
        "lzma2" => {
            validate_lzma2(bytes)?;
            lzma_rs::lzma2_decompress(&mut Cursor::new(bytes), &mut output).context("Bin Slob LZMA2 inválido")?;
        }
        _ => bail!("Compressão Slob não suportada: {compression}"),
    }
    Ok(output.0)
}

/// lzma-rs buffers LZMA2's dictionary internally. Checking every chunk's declared
/// output size before decoding prevents a compressed bomb from growing that buffer.
fn validate_lzma2(input: &[u8]) -> Result<()> {
    let mut at = 0usize;
    let mut output = 0usize;
    loop {
        let control = *input.get(at).context("Slob: stream LZMA2 sem terminador")?;
        at += 1;
        if control == 0 { ensure!(at == input.len(), "Slob: dados após o terminador LZMA2"); break; }
        let read_short = |at: usize| -> Result<usize> {
            let bytes = input.get(at..at.saturating_add(2)).context("Slob: cabeçalho LZMA2 truncado")?;
            Ok(usize::from(u16::from_be_bytes(bytes.try_into()?)))
        };
        let (unpacked, packed, header) = if control == 1 || control == 2 {
            let length = read_short(at)? + 1;
            (length, length, 2)
        } else {
            ensure!(control >= 0x80, "Slob: controle LZMA2 inválido");
            let unpacked = (usize::from(control & 0x1f) << 16) + read_short(at)? + 1;
            let packed = read_short(at + 2)? + 1;
            (unpacked, packed, if control >= 0xc0 { 5 } else { 4 })
        };
        output = output.checked_add(unpacked).context("Slob: overflow LZMA2")?;
        ensure!(output <= MAX_BIN, "Bin Slob LZMA2 descomprimido excede 64 MiB");
        at = at.checked_add(header).and_then(|value| value.checked_add(packed)).context("Slob: overflow LZMA2")?;
        ensure!(at <= input.len(), "Slob: chunk LZMA2 truncado");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Memory(Vec<(String, String)>);
    impl ImportSink for Memory {
        fn add(&mut self, word: &str, html: &str, _aliases: &[String]) -> Result<()> {
            self.0.push((word.into(), html.into())); Ok(())
        }
    }

    fn tiny(output: &mut Vec<u8>, text: &str) { output.push(text.len() as u8); output.extend_from_slice(text.as_bytes()); }
    fn text(output: &mut Vec<u8>, text: &str) { output.extend_from_slice(&(text.len() as u16).to_be_bytes()); output.extend_from_slice(text.as_bytes()); }

    fn fixture(compression: &str) -> Vec<u8> {
        let mut file = MAGIC.to_vec();
        file.extend_from_slice(&[0; 16]);
        tiny(&mut file, "utf-8"); tiny(&mut file, compression);
        file.push(1); tiny(&mut file, "label");
        file.push(255); file.extend_from_slice(b"Synthetic Slob"); file.extend_from_slice(&[0; 241]);
        file.push(2); text(&mut file, "text/html; charset=utf-8"); text(&mut file, "text/css");
        file.extend_from_slice(&2u32.to_be_bytes());
        let store_at = file.len(); file.extend_from_slice(&0u64.to_be_bytes());
        let size_at = file.len(); file.extend_from_slice(&0u64.to_be_bytes());
        let mut references = Vec::new();
        for (key, item) in [("cat", 0u16), ("cats", 0), ("style.css", 1)] {
            let mut record = Vec::new(); text(&mut record, key);
            record.extend_from_slice(&0u32.to_be_bytes()); record.extend_from_slice(&item.to_be_bytes()); tiny(&mut record, "");
            references.push(record);
        }
        file.extend_from_slice(&3u32.to_be_bytes());
        let mut position = 0u64;
        for reference in &references { file.extend_from_slice(&position.to_be_bytes()); position += reference.len() as u64; }
        for reference in references { file.extend_from_slice(&reference); }
        let store_offset = file.len() as u64;
        file[store_at..store_at + 8].copy_from_slice(&store_offset.to_be_bytes());
        file.extend_from_slice(&1u32.to_be_bytes()); file.extend_from_slice(&0u64.to_be_bytes());
        let article = b"<h2>cat</h2><p>A small feline.</p>";
        let style = b"p{color:red}";
        let mut bin = Vec::new();
        bin.extend_from_slice(&0u32.to_be_bytes()); bin.extend_from_slice(&(article.len() as u32 + 4).to_be_bytes());
        bin.extend_from_slice(&(article.len() as u32).to_be_bytes()); bin.extend_from_slice(article);
        bin.extend_from_slice(&(style.len() as u32).to_be_bytes()); bin.extend_from_slice(style);
        let mut compressed = Vec::new();
        match compression {
            "" => compressed = bin,
            "zlib" => { let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default()); encoder.write_all(&bin).unwrap(); compressed = encoder.finish().unwrap(); }
            "bz2" => { let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default()); encoder.write_all(&bin).unwrap(); compressed = encoder.finish().unwrap(); }
            "lzma2" => lzma_rs::lzma2_compress(&mut Cursor::new(bin), &mut compressed).unwrap(),
            _ => unreachable!(),
        }
        file.extend_from_slice(&2u32.to_be_bytes()); file.extend_from_slice(&[0, 1]);
        file.extend_from_slice(&(compressed.len() as u32).to_be_bytes()); file.extend_from_slice(&compressed);
        let size = file.len() as u64; file[size_at..size_at + 8].copy_from_slice(&size.to_be_bytes());
        file
    }

    struct Temporary(std::path::PathBuf);
    impl Temporary {
        fn new(data: &[u8], label: &str) -> Self {
            let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("lookitan-{label}-{}-{nonce}.slob", std::process::id()));
            std::fs::write(&path, data).unwrap(); Self(path)
        }
    }
    impl Drop for Temporary { fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); } }

    #[test]
    fn imports_all_supported_compressions_and_serves_binary_keys() {
        for compression in ["", "zlib", "bz2", "lzma2"] {
            let file = Temporary::new(&fixture(compression), "slob");
            let mut sink = Memory::default();
            let info = import(&file.0, &mut sink).unwrap();
            assert_eq!(info.name, "Synthetic Slob");
            assert_eq!(sink.0.len(), 2);
            assert_eq!(sink.0[0].0, "cat"); assert_eq!(sink.0[1].0, "cats");
            assert!(sink.0[0].1.contains("small feline"));
            assert_eq!(resource_at(&file.0, 2), Some(b"p{color:red}".to_vec()));
            assert_eq!(resource_at(&file.0, 3), None);
        }
    }

    #[test]
    fn reads_fixture_from_the_public_python_writer() {
        use base64::Engine as _;
        // Produced by itkach/slob's public Writer, source SHA-256
        // af90fa4de366196f7b26dc6aa3a6860814f668b4fa9bcda02ca5d6d3caa85f25.
        // ASCII test ordering substituted for ICU only; header, positioned
        // directories, packed store and RAW LZMA2 are the original writer's bytes.
        // Content was authored for this test; no dictionary assets are embedded.
        let data = base64::engine::general_purpose::STANDARD.decode("IS0xU0xPQh/NhWLYgC5E2YDSCptUCFSXBXV0Zi04BWx6bWEyAQVsYWJlbP9QdWJsaWMgd3JpdGVyIGZpeHR1cmUAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACABh0ZXh0L2h0bWw7IGNoYXJzZXQ9dXRmLTgACHRleHQvY3NzAAAAAgAAAAAAAAGrAAAAAAAAAfoAAAADAAAAAAAAAAAAAAAAAAAADAAAAAAAAAAZAANjYXQAAAAAAAAAAARjYXRzAAAAAAAAAAAJc3R5bGUuY3NzAAAAAAABAAAAAAEAAAAAAAAAAAAAAAIAAQAAADngADMAMV0AAGoFeO+wuSDlrhUqxZUspud0mnM9nSJxTf++Hp2ouN/0Zw12AINkiGwusUL1H+mLAAA=").unwrap();
        let file = Temporary::new(&data, "slob-public-writer");
        let mut sink = Memory::default();
        let info = import(&file.0, &mut sink).unwrap();
        assert_eq!(info.name, "Public writer fixture");
        assert_eq!(sink.0, [("cat".to_string(), "<p>Reference feline.</p>".to_string()), ("cats".to_string(), "<p>Reference feline.</p>".to_string())]);
        assert_eq!(resource_at(&file.0, 2).unwrap(), b"p{color:red}");
    }

    #[test]
    fn import_emits_direct_resource_references() {
        #[derive(Default)]
        struct Indexed { articles: usize, resources: Vec<(String, u32)> }
        impl ImportSink for Indexed {
            fn add(&mut self, _word: &str, _html: &str, _aliases: &[String]) -> Result<()> {
                self.articles += 1; Ok(())
            }
            fn add_resource(&mut self, key: &str, reference: u32) -> Result<()> {
                self.resources.push((key.into(), reference)); Ok(())
            }
        }
        let file = Temporary::new(&fixture(""), "slob-indexed-resource");
        let mut sink = Indexed::default();
        import(&file.0, &mut sink).unwrap();
        assert_eq!(sink.articles, 2);
        assert_eq!(sink.resources, [("style.css".to_string(), 2)]);
        assert_eq!(resource_at(&file.0, sink.resources[0].1).unwrap(), b"p{color:red}");
    }

    #[test]
    fn corrupted_offsets_and_sizes_return_errors() {
        let mut data = fixture(""); data.pop();
        let file = Temporary::new(&data, "slob-truncated");
        assert!(import(&file.0, &mut Memory::default()).is_err());
        let mut data = fixture(""); data[0] = 0;
        let file = Temporary::new(&data, "slob-magic");
        assert!(import(&file.0, &mut Memory::default()).is_err());
        let malformed = Bin { count: 1, data: vec![255; 4] };
        assert!(malformed.item(0).is_err());
    }

    #[test]
    fn lzma2_checks_chunk_framing_before_allocating() {
        assert!(validate_lzma2(&[1, 0, 1, b'a', b'b', 0]).is_ok());
        assert!(validate_lzma2(&[1, 0, 1, b'a', 0]).is_err());
        assert!(validate_lzma2(&[3, 0]).is_err());
        assert!(validate_lzma2(&[0, 1]).is_err());
        // A raw, genuinely compressed LZMA2 chunk produced with the public codec,
        // rather than lzma-rs's uncompressed-chunk test writer.
        let packed = [224, 0, 99, 0, 6, 93, 0, 48, 238, 158, 0, 0, 0, 0];
        assert_eq!(decompress("lzma2", &packed).unwrap(), vec![b'a'; 100]);
        let mut bomb = Vec::new();
        for _ in 0..33 { bomb.extend_from_slice(&[0xbf, 0xff, 0xff, 0, 0, 0]); }
        bomb.push(0);
        assert!(validate_lzma2(&bomb).is_err());
        assert!(content_encoding("text/plain; charset=unknown-lookitan").is_err());
    }
}
