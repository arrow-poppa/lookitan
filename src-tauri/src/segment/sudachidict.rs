//! Japanese words as Lookupper finds them (REA on 2026-10-21): Sudachi in split mode C, with
//! SudachiDict core and Sudachi's default plugins: DefaultInputText (rewrite.def),
//! ProlongedSoundMark, MeCab-style OOV, SimpleOov, JoinNumeric and JoinKatakanaOov (min length 3).
//! Its exe has no IgnoreYomigana plugin, so neither has the config here. Sudachi is sudachi.rs
//! (Apache-2.0), which carries those plugins and their files; `char.def` must also be on disk.
//!
//! The dictionary is SudachiDict core 20260723 (Apache-2.0), the same `system.dic` Lookupper
//! downloads (217,466,039 bytes). It comes from its PyPI-style wheel, a zip, on SudachiDict's
//! GitHub release, pinned by SHA-256. sudachi.rs 0.7 reads only the newer dictionary format, so
//! Cargo.toml pins 0.6.11.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use sudachi::analysis::stateless_tokenizer::StatelessTokenizer;
use sudachi::config::Config;
use sudachi::dic::dictionary::JapaneseDictionary;

/// The dictionary's folder; bump it with the dictionary.
pub(super) const DIR: &str = "sudachidict-core-20260723";
/// lindera's SudachiDict, which this replaces.
const OLD_DIR: &str = "lindera-sudachidict-6.2.0";

const WHEEL_URL: &str = "https://github.com/WorksApplications/SudachiDict/releases/download/v20260723/sudachidict_core-20260723-py3-none-any.whl";
const WHEEL_SHA256: &str = "b3869ce6b12b4bfa09575dc19030703bb669ab41bac12a74cafcbb28c6be2498";
pub const WHEEL_BYTES: u64 = 72_275_897;
const WHEEL_DIC: &str = "sudachidict_core/resources/system.dic";
const WHEEL_LICENSE: &str = "sudachidict_core-20260723.dist-info/licenses/LICENSE-2.0.txt";

const CHAR_DEF_URL: &str = "https://raw.githubusercontent.com/WorksApplications/sudachi.rs/v0.6.11/resources/char.def";
const CHAR_DEF_SHA256: &str = "b549ec56ad67359f535c80b7efa150538af2a78b7609d0d6bae796dd89f4f29d";
const CHAR_DEF_BYTES: u64 = 5_475;

/// Sudachi's default `sudachi.json` without IgnoreYomigana.
const CONFIG: &str = r#"{
    "systemDict" : "system.dic",
    "characterDefinitionFile" : "char.def",
    "inputTextPlugin" : [
        { "class" : "com.worksap.nlp.sudachi.DefaultInputTextPlugin" },
        { "class" : "com.worksap.nlp.sudachi.ProlongedSoundMarkPlugin",
          "prolongedSoundMarks": ["ー", "-", "⁓", "〜", "〰"],
          "replacementSymbol": "ー" }
    ],
    "oovProviderPlugin" : [
        { "class" : "com.worksap.nlp.sudachi.MeCabOovPlugin" },
        { "class" : "com.worksap.nlp.sudachi.SimpleOovPlugin",
          "oovPOS" : [ "補助記号", "一般", "*", "*", "*", "*" ],
          "leftId" : 5968,
          "rightId" : 5968,
          "cost" : 3857 }
    ],
    "pathRewritePlugin" : [
        { "class" : "com.worksap.nlp.sudachi.JoinNumericPlugin",
          "enableNormalize" : true },
        { "class" : "com.worksap.nlp.sudachi.JoinKatakanaOovPlugin",
          "oovPOS" : [ "名詞", "普通名詞", "一般", "*", "*", "*" ],
          "minLength" : 3 }
    ]
}
"#;

pub type Tokenizer = StatelessTokenizer<Arc<JapaneseDictionary>>;

/// The dictionary's folder, downloaded and unpacked first if it is not there.
pub fn ensure(root: &Path, progress: &dyn Fn(u64)) -> Result<PathBuf> {
    let dir = root.join(DIR);
    if dir.join("system.dic").is_file() {
        return Ok(dir);
    }
    std::fs::create_dir_all(root)?;
    let _ = std::fs::remove_dir_all(root.join(OLD_DIR));
    for entry in std::fs::read_dir(root)?.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&format!(".building-{DIR}-")) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    let building = root.join(format!(".building-{DIR}-{}", std::process::id()));
    std::fs::create_dir_all(&building)?;
    if let Err(e) = build(&building, progress) {
        let _ = std::fs::remove_dir_all(&building);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&building, &dir).with_context(|| format!("move {DIR} into place"))?;
    Ok(dir)
}

fn build(dir: &Path, progress: &dyn Fn(u64)) -> Result<()> {
    let wheel = dir.join("sudachidict.zip");
    super::download(WHEEL_URL, &wheel, WHEEL_BYTES, Some(WHEEL_SHA256), progress)?;
    let unpacked = dir.join("wheel");
    crate::dictionaries::archive::unpack(&wheel, crate::dictionaries::archive::Kind::Zip, &unpacked, &|_| {})?;
    let _ = std::fs::remove_file(&wheel);
    if !unpacked.join(WHEEL_DIC).is_file() {
        bail!("SudachiDict has no {WHEEL_DIC}");
    }
    std::fs::rename(unpacked.join(WHEEL_DIC), dir.join("system.dic"))?;
    let _ = std::fs::rename(unpacked.join(WHEEL_LICENSE), dir.join("LICENSE-2.0.txt"));
    let _ = std::fs::remove_dir_all(&unpacked);
    super::download(CHAR_DEF_URL, &dir.join("char.def"), CHAR_DEF_BYTES, Some(CHAR_DEF_SHA256), &|_| {})?;
    std::fs::write(dir.join("sudachi.json"), CONFIG)?;
    std::fs::write(
        dir.join("NOTICE.txt"),
        "SudachiDict core 20260723 (https://github.com/WorksApplications/SudachiDict) and Sudachi's char.def\n(https://github.com/WorksApplications/sudachi.rs), Copyright (c) Works Applications Co., Ltd.,\nunder the Apache License 2.0 (LICENSE-2.0.txt).\n",
    )?;
    Ok(())
}

/// Sudachi with the dictionary in `dir`.
pub fn open(dir: &Path) -> Result<Tokenizer> {
    let config = Config::new(Some(dir.join("sudachi.json")), Some(dir.to_path_buf()), Some(dir.join("system.dic")))
        .map_err(|e| anyhow!("Sudachi config: {e}"))?;
    let dictionary = JapaneseDictionary::from_cfg(&config).map_err(|e| anyhow!("SudachiDict: {e}"))?;
    Ok(StatelessTokenizer::new(Arc::new(dictionary)))
}
