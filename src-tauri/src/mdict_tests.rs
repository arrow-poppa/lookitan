// Full synthetic files exercise the binary parser, rather than mocking parsed indexes.
// Fixtures contain only test-generated words/media; no third-party dictionary content.
use super::*;
use std::io::Write;

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes { a = (a + byte as u32) % 65521; b = (b + a) % 65521; }
    (b << 16) | a
}

fn number(out: &mut Vec<u8>, value: u64, wide: bool) {
    if wide { out.extend_from_slice(&value.to_be_bytes()); }
    else { out.extend_from_slice(&u32::try_from(value).unwrap().to_be_bytes()); }
}

fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf16 => text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        Encoding::Gb18030 => encoding_rs::GB18030.encode(text).0.into_owned(),
        Encoding::Big5 => encoding_rs::BIG5.encode(text).0.into_owned(),
    }
}

fn packed(data: &[u8], kind: u32) -> Vec<u8> {
    let payload = match kind {
        0 => data.to_vec(),
        1 => lzokay::compress::compress(data).unwrap(),
        2 => {
            let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(data).unwrap();
            encoder.finish().unwrap()
        }
        _ => panic!("invalid fixture compressor"),
    };
    let mut out = kind.to_le_bytes().to_vec();
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out.extend(payload);
    out
}

struct Fixture { bytes: Vec<u8>, key_header: usize, key_blocks: usize, record_header: usize, record_index: usize }

fn fixture(wide: bool, encoding: Encoding, attrs: &str, keys: &[(&str, u64)], blocks: &[&[u8]], kind: u32) -> Fixture {
    assert!(!keys.is_empty());
    let label = match encoding { Encoding::Utf8 => "UTF-8", Encoding::Utf16 => "UTF-16", Encoding::Gb18030 => "GB18030", Encoding::Big5 => "Big5" };
    let header = format!("<Dictionary GeneratedByEngineVersion=\"{}\" Encoding=\"{label}\" Encrypted=\"No\" Title=\"Synthetic &amp; Test\" {attrs}/>\0", if wide { "2.0" } else { "1.2" });
    let header = encode(&header, Encoding::Utf16);
    let mut bytes = (header.len() as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&adler32(&header).to_le_bytes());

    let mut key_data = Vec::new();
    let unit = if encoding == Encoding::Utf16 { 2 } else { 1 };
    for &(word, offset) in keys {
        number(&mut key_data, offset, wide);
        key_data.extend(encode(word, encoding));
        key_data.extend(std::iter::repeat_n(0, unit));
    }
    let key_block = packed(&key_data, kind);
    let mut key_info = Vec::new();
    number(&mut key_info, keys.len() as u64, wide);
    for word in [keys[0].0, keys[keys.len() - 1].0] {
        let encoded = encode(word, encoding);
        let length = encoded.len() / unit;
        if wide { key_info.extend_from_slice(&(length as u16).to_be_bytes()); }
        else { key_info.push(length as u8); }
        key_info.extend(encoded);
        if wide { key_info.extend(std::iter::repeat_n(0, unit)); }
    }
    number(&mut key_info, key_block.len() as u64, wide);
    number(&mut key_info, key_data.len() as u64, wide);
    let index = if wide { packed(&key_info, 2) } else { key_info.clone() };
    let key_header = bytes.len();
    number(&mut bytes, 1, wide);
    number(&mut bytes, keys.len() as u64, wide);
    if wide { number(&mut bytes, key_info.len() as u64, true); }
    number(&mut bytes, index.len() as u64, wide);
    number(&mut bytes, key_block.len() as u64, wide);
    if wide {
        let checksum = adler32(&bytes[key_header..]);
        bytes.extend_from_slice(&checksum.to_be_bytes());
    }
    bytes.extend(index);
    let key_blocks = bytes.len();
    bytes.extend(key_block);

    let record_header = bytes.len();
    let record_blocks: Vec<_> = blocks.iter().map(|block| packed(block, kind)).collect();
    number(&mut bytes, blocks.len() as u64, wide);
    number(&mut bytes, keys.len() as u64, wide);
    number(&mut bytes, (blocks.len() * if wide { 16 } else { 8 }) as u64, wide);
    number(&mut bytes, record_blocks.iter().map(Vec::len).sum::<usize>() as u64, wide);
    let record_index = bytes.len();
    for (block, raw) in record_blocks.iter().zip(blocks) {
        number(&mut bytes, block.len() as u64, wide);
        number(&mut bytes, raw.len() as u64, wide);
    }
    for block in record_blocks { bytes.extend(block); }
    Fixture { bytes, key_header, key_blocks, record_header, record_index }
}

fn read(bytes: &[u8], is_mdd: bool) -> Result<Mdict> {
    let mut map = memmap2::MmapMut::map_anon(bytes.len())?;
    map.copy_from_slice(bytes);
    Mdict::parse(PathBuf::from(if is_mdd { "fixture.mdd" } else { "fixture.mdx" }), map.make_read_only()?, is_mdd)
}

#[test]
fn reads_cross_block_records_and_non_adjacent_aliases_in_both_wire_versions() {
    for wide in [false, true] {
        for kind in [0, 1, 2] {
            let data = fixture(wide, Encoding::Utf8, "", &[("alpha", 0), ("alias", 0), ("beta", 7), ("late-alias", 0), ("tail", 12)],
                &[b"abc", b"defgHI", b"JKLmnop"], kind);
            let dict = read(&data.bytes, false).unwrap();
            assert_eq!(dict.len(), 5);
            assert_eq!(dict.title, "Synthetic & Test");
            assert_eq!(dict.headword(3), Some("late-alias"));
            for ordinal in [0, 1, 3] { assert_eq!(dict.record_bytes(ordinal).unwrap(), b"abcdefg"); }
            assert_eq!(dict.record_bytes(2).unwrap(), b"HIJKL");
            assert_eq!(dict.record_bytes(4).unwrap(), b"mnop");
            assert!(dict.record_bytes(9).is_err());
        }
    }
}

#[test]
fn follows_case_and_strip_flags_without_collapsing_mdd_paths() {
    let keys = [("a-b", 0), ("A B", 1), ("ab", 2), ("AB", 3)];
    let data = fixture(true, Encoding::Utf8, "KeyCaseSensitive=\"Yes\" StripKey=\"No\"", &keys, &[b"1234"], 2);
    let dict = read(&data.bytes, false).unwrap();
    assert_eq!(dict.find("AB"), vec![3]);
    assert!(dict.find("Ab").is_empty());
    let data = fixture(true, Encoding::Utf8, "KeyCaseSensitive=\"No\" StripKey=\"Yes\"", &keys, &[b"1234"], 2);
    assert_eq!(read(&data.bytes, false).unwrap().find(" A_b "), vec![0, 1, 2, 3]);
    let data = fixture(true, Encoding::Utf8, "", &keys, &[b"1234"], 0);
    let dict = read(&data.bytes, false).unwrap();
    assert_eq!(dict.find("AB"), vec![2, 3]);
    assert_eq!(dict.find("a-b"), vec![0]);
    let data = fixture(true, Encoding::Utf16, "StripKey=\"Yes\"", &[("\\i\\a-b.png", 0), ("\\i\\ab.png", 2)], &[b"12", b"345"], 1);
    let dict = read(&data.bytes, true).unwrap();
    assert_eq!(dict.resource("i/a-b.png"), Some(b"12".to_vec()));
    assert_eq!(dict.resource("i/ab.png"), Some(b"345".to_vec()));
}

#[test]
fn reads_legacy_and_utf16_encodings_through_full_files() {
    for encoding in [Encoding::Utf8, Encoding::Utf16, Encoding::Gb18030, Encoding::Big5] {
        for wide in [false, true] {
            let text = encode("中文 definição\0", encoding);
            let first = text.len().min(5); // May deliberately divide a multi-byte code unit.
            let data = fixture(wide, encoding, "", &[("中文", 0)], &[&text[..first], &text[first..]], 2);
            let dict = read(&data.bytes, false).unwrap();
            assert_eq!(dict.find("中文"), vec![0]);
            assert_eq!(dict.record_text(0).unwrap(), encoding.decode(&text).trim_end_matches('\0'));
        }
    }
}

#[test]
fn expands_compact_styles_with_xml_entities_and_preserves_unknown_markers() {
    let attrs = "Compact='Yes' StyleSheet='1&#10;&lt;b&gt;&#10;&lt;/b&gt;&#10;2&#10;&lt;i&gt;&#10;&lt;/i&gt;' Description = 'A &amp; B &#x4e2d;'";
    let data = fixture(true, Encoding::Utf8, attrs, &[("word", 0)], &[b"lead `1`bold\r\n`2`italic`99`unknown\0"], 2);
    let dict = read(&data.bytes, false).unwrap();
    assert_eq!(dict.record_text(0).unwrap(), "lead <b>bold</b>\r\n<i>italic</i>`99`unknown");
    assert_eq!(dict.metadata.get("Description").map(String::as_str), Some("A & B 中"));
    assert_eq!(unescape_xml("&amp;lt; &#65; &#x42; &unknown;"), "&lt; A B &unknown;");
    assert_eq!(expand_styles("`x`中文 `1`x", &dict.styles).unwrap(), "`x`中文 <b>x</b>");
}

#[test]
fn repeated_requests_share_the_cached_block_and_obey_the_byte_budget() {
    let data = fixture(true, Encoding::Utf8, "", &[("word", 0)], &[b"article"], 2);
    let dict = Arc::new(read(&data.bytes, false).unwrap());
    let first = dict.block_data(0).unwrap();
    let workers: Vec<_> = (0..8).map(|_| {
        let dict = dict.clone();
        std::thread::spawn(move || dict.block_data(0).unwrap())
    }).collect();
    for worker in workers { assert!(Arc::ptr_eq(&first, &worker.join().unwrap())); }
    assert_eq!(dict.cache.lock().entries.len(), 1);
    let mut cache = BlockCache::default();
    cache.insert(0, Arc::new(vec![1; 4]), 8);
    cache.insert(1, Arc::new(vec![2; 4]), 8);
    assert!(cache.get(0).is_some());
    cache.insert(2, Arc::new(vec![3; 4]), 8);
    assert!(cache.get(1).is_none(), "evict least recently used bytes");
    cache.insert(0, Arc::new(vec![4; 3]), 8);
    assert_eq!(cache.bytes, 7);
    assert_eq!(cache.entries.iter().filter(|(index, _)| *index == 0).count(), 1);
    cache.insert(3, Arc::new(vec![5; 9]), 8);
    assert_eq!(cache.bytes, 7, "oversized blocks are served without retention");
}

#[test]
fn rejects_truncations_ranges_and_hostile_declarations_without_panicking() {
    let data = fixture(true, Encoding::Utf8, "", &[("word", 0)], &[b"article"], 2);
    for end in 1..data.bytes.len() { assert!(read(&data.bytes[..end], false).is_err(), "accepted truncation at {end}"); }
    let mut corrupt = data.bytes.clone();
    corrupt[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(read(&corrupt, false).is_err());
    let mut corrupt = data.bytes.clone();
    corrupt[data.key_header..data.key_header + 8].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(read(&corrupt, false).is_err());
    let mut corrupt = data.bytes.clone();
    corrupt[data.record_index + 8..data.record_index + 16].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(read(&corrupt, false).is_err());
    let mut corrupt = data.bytes.clone();
    corrupt[data.record_header + 16..data.record_header + 24].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(read(&corrupt, false).is_err());
    let bad_offset = fixture(true, Encoding::Utf8, "", &[("word", 999)], &[b"article"], 2);
    assert!(read(&bad_offset.bytes, false).is_err());
    let mut cursor = Cursor { data: b"", pos: usize::MAX };
    assert!(cursor.take(2).is_err());
}

#[test]
fn validates_inflated_sizes_and_key_terminators() {
    for kind in [0, 1, 2] {
        let block = packed(b"some text", kind);
        assert!(decompress_block(&block, 4).is_err());
        assert!(decompress_block(&block, 20).is_err());
        assert!(decompress_block(&block, MAX_BLOCK_BYTES + 1).is_err());
    }
    let mut keys = Vec::new();
    let mut budget = 0;
    let policy = KeyPolicy { case_sensitive: false, strip: false };
    assert!(split_key_block(&[0; 7], true, Encoding::Utf8, policy, &mut keys, &mut budget).is_err());
    let mut unterminated = 0u64.to_be_bytes().to_vec();
    unterminated.extend_from_slice(b"word");
    assert!(split_key_block(&unterminated, true, Encoding::Utf8, policy, &mut keys, &mut budget).is_err());
    let mut full = MAX_INDEX_BYTES;
    assert!(charge(&mut full, 1).is_err());
}
