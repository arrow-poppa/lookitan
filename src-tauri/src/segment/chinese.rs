//! Chinese words as Lookupper finds them (REA on 2026-10-21, `0x1401ad540`, `0x1401adfc0`): jieba's
//! algorithm, `jieba.cut` with its HMM for the runs of single characters the dictionary leaves
//! unjoined, over jieba's dict.txt.big plus every CC-CEDICT headword, traditional and simplified,
//! that it lacks, at frequency 1. Lookupper's `dict.txt` is exactly that, and its `hmm.txt` is
//! jieba's own HMM tables, which jieba-rs (MIT) carries with the algorithm.
//!
//! dict.txt.big (MIT) comes pinned from jieba's repository. CC-CEDICT (CC BY-SA 4.0) comes from
//! MDBG, which updates it daily, so it is checked by its header instead of a hash. The two are
//! merged once into `dict.txt`, with both notices in NOTICE.txt.

use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// The merged dictionary's folder; bump the number when the merge changes.
const DIR: &str = "jieba-cedict-1";
/// lindera's jieba, which this replaces.
const OLD_DIR: &str = "lindera-jieba-6.2.0";

const DICT_BIG_URL: &str = "https://raw.githubusercontent.com/fxsjy/jieba/237dc6625e5c65d7a2714ffdfa5238dba5cae7d4/extra_dict/dict.txt.big";
const DICT_BIG_SHA256: &str = "b16011275c42955ccd81fc1adecc93a59dbb7926af69d93fc95d4943d40f6aad";
const DICT_BIG_BYTES: u64 = 8_583_143;

const CEDICT_URL: &str = "https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.zip";
const CEDICT_FILE: &str = "cedict_ts.u8";
/// Far above its size (4 MB zipped in 2026).
const CEDICT_MAX_BYTES: u64 = 40_000_000;
/// About what both downloads weigh, for the progress shown in Settings.
pub const BYTES: u64 = DICT_BIG_BYTES + 4_000_000;

const JIEBA_NOTICE: &str = "jieba's dict.txt.big (https://github.com/fxsjy/jieba)\n\nThe MIT License (MIT)\n\nCopyright (c) 2013 Sun Junyi\n\nPermission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the \"Software\"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:\n\nThe above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.\n";

/// jieba's dictionary with the CC-CEDICT headwords it lacks, as `word freq [tag]` lines.
pub fn merge(big: &str, cedict: &str) -> String {
    let mut out = String::with_capacity(big.len() + cedict.len() / 4);
    let mut have = std::collections::HashSet::new();
    for line in big.lines() {
        if let Some(word) = line.split_whitespace().next() {
            have.insert(word);
            out.push_str(line);
            out.push('\n');
        }
    }
    // `traditional simplified [pinyin] /definitions/`, after `#` comments.
    for line in cedict.lines().filter(|l| !l.starts_with('#')) {
        for word in line.split(' ').take(2) {
            if !word.is_empty() && have.insert(word) {
                out.push_str(word);
                out.push_str(" 1\n");
            }
        }
    }
    out
}

/// CC-CEDICT's header: its licence and version.
fn cedict_header(cedict: &str) -> String {
    cedict.lines().take_while(|l| l.starts_with('#')).map(|l| format!("{l}\n")).collect()
}

/// The merged dictionary's folder, downloaded and merged first if it is not there.
pub fn ensure(root: &Path, progress: &dyn Fn(u64)) -> Result<PathBuf> {
    let dir = root.join(DIR);
    if dir.join("dict.txt").is_file() {
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
    let built = build(&building, progress);
    if let Err(e) = built {
        let _ = std::fs::remove_dir_all(&building);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&building, &dir).with_context(|| format!("move {DIR} into place"))?;
    Ok(dir)
}

fn build(dir: &Path, progress: &dyn Fn(u64)) -> Result<()> {
    let big_path = dir.join("dict.txt.big");
    super::download(DICT_BIG_URL, &big_path, DICT_BIG_BYTES, Some(DICT_BIG_SHA256), progress)?;
    let zip = dir.join("cedict.zip");
    super::download(CEDICT_URL, &zip, CEDICT_MAX_BYTES, None, &|done| progress(DICT_BIG_BYTES + done))?;
    let unpacked = dir.join("cedict");
    crate::dictionaries::archive::unpack(&zip, crate::dictionaries::archive::Kind::Zip, &unpacked, &|_| {})?;
    let cedict = std::fs::read_to_string(unpacked.join(CEDICT_FILE)).with_context(|| format!("CC-CEDICT has no {CEDICT_FILE}"))?;
    if !cedict.starts_with("# CC-CEDICT") {
        bail!("{CEDICT_FILE} is not CC-CEDICT");
    }
    let big = std::fs::read_to_string(&big_path)?;
    std::fs::write(dir.join("dict.txt"), merge(&big, &cedict))?;
    std::fs::write(dir.join("NOTICE.txt"), format!("{JIEBA_NOTICE}\n{}", cedict_header(&cedict)))?;
    let _ = std::fs::remove_file(&big_path);
    let _ = std::fs::remove_file(&zip);
    let _ = std::fs::remove_dir_all(&unpacked);
    Ok(())
}

/// jieba with the merged dictionary.
pub fn open(dir: &Path) -> Result<jieba_rs::Jieba> {
    let file = std::fs::File::open(dir.join("dict.txt")).with_context(|| format!("open {}", dir.join("dict.txt").display()))?;
    jieba_rs::Jieba::with_dict(&mut BufReader::new(file)).map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CEDICT: &str = "# CC-CEDICT\n#! version=1\n緬因貓 缅因猫 [Mian3 yin1 mao1] /Maine Coon/\n我們 我们 [wo3 men5] /we/\n";

    #[test]
    fn cedict_headwords_jieba_lacks_are_added_once_at_frequency_one() {
        let merged = merge("我们 100 r\n在 50 p\n", CEDICT);
        assert_eq!(merged, "我们 100 r\n在 50 p\n緬因貓 1\n缅因猫 1\n我們 1\n");
        assert_eq!(cedict_header(CEDICT), "# CC-CEDICT\n#! version=1\n");
    }

    #[test]
    fn a_word_the_dictionary_has_stays_whole_and_unknown_runs_go_through_the_hmm() {
        let dict = merge("我们 100 r\n去 80 v\n看 80 v\n吧 60 y\n緬因 2 ns\n貓吧 3 n\n", CEDICT);
        let jieba = jieba_rs::Jieba::with_dict(&mut dict.as_bytes()).unwrap();
        let words: Vec<&str> = jieba.cut("我們去看緬因貓吧", true).iter().map(|t| t.word).collect();
        assert_eq!(words, ["我們", "去", "看", "緬因貓", "吧"]);
    }
}
