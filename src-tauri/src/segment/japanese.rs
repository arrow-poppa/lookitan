//! Japanese words from SudachiDict morphemes. Like Lookupper, a word is a content morpheme with
//! what conjugates or attaches after it: a verb or adjective with its auxiliaries, て/で plus the
//! verbs that follow it (いる, しまう, くれる…), a suffix such as さん or たち, a する verb with its
//! noun; a prefix joins the morpheme after it, a number its counter. Particles and auxiliaries
//! standing alone are words of their own, and punctuation is none.
//!
//! Morpheme details (mod.rs, from Sudachi): surface, part of speech (4 levels), conjugation type,
//! conjugation form, reading, normalized form, dictionary form.

use super::{Kind, Morpheme, Word};

const POS: usize = 1;
const CONJUGATION_FORM: usize = 6;
const NORMALIZED: usize = 8;
const DICTIONARY: usize = 9;

/// Conjunctive particles after which a verb carries on the same word (食べて + いる).
const CONTINUING: [&str; 5] = ["て", "で", "ば", "たり", "だり"];

fn pos(m: &Morpheme, level: usize) -> &str {
    m.detail(POS + level)
}

fn is(m: &Morpheme, major: &str) -> bool {
    pos(m, 0) == major
}

fn is_symbol(m: &Morpheme) -> bool {
    matches!(pos(m, 0), "補助記号" | "空白" | "記号")
}

/// A morpheme that makes what it is in a predicate, so auxiliaries may follow.
fn is_predicate(m: &Morpheme) -> bool {
    matches!(pos(m, 0), "動詞" | "形容詞" | "形状詞" | "助動詞")
        || (is(m, "接尾辞") && matches!(pos(m, 1), "動詞的" | "形容詞的"))
}

/// A noun that takes する (勉強, 散歩).
fn takes_suru(m: &Morpheme) -> bool {
    is(m, "名詞") && matches!(pos(m, 2), "サ変可能" | "サ変形状詞可能")
}

fn is_suru(m: &Morpheme) -> bool {
    matches!(m.detail(NORMALIZED), "する" | "為る")
}

fn conjugates(m: &Morpheme) -> bool {
    m.detail(CONJUGATION_FORM) != "*"
}

fn is_counter(m: &Morpheme) -> bool {
    is(m, "接尾辞") && pos(m, 1) == "名詞的" && pos(m, 2) == "助数詞"
}

/// Whether morpheme `j` belongs to the word that starts at `start`.
fn attaches(ms: &[Morpheme], start: usize, j: usize) -> bool {
    let (cur, prev) = (&ms[j], &ms[j - 1]);
    let predicate = ms[start..j].iter().any(is_predicate);
    match (pos(cur, 0), pos(cur, 1)) {
        ("助動詞", _) => predicate || is(prev, "形状詞"),
        ("形容詞", "非自立可能") => predicate && matches!(pos(prev, 0), "助動詞" | "形容詞" | "動詞"),
        ("助詞", "接続助詞") => predicate && CONTINUING.contains(&cur.surface.as_str()),
        ("動詞", "非自立可能") => {
            (is(prev, "助詞") && pos(prev, 1) == "接続助詞" && CONTINUING.contains(&prev.surface.as_str()))
                || (is(prev, "動詞") && prev.detail(CONJUGATION_FORM).contains("連用形"))
                || (takes_suru(prev) && is_suru(cur))
        }
        ("接尾辞", "名詞的") => {
            matches!(pos(prev, 0), "名詞" | "代名詞") || (is(prev, "接尾辞") && pos(prev, 1) == "名詞的")
        }
        ("接尾辞", _) => true,
        ("形状詞", "助動詞語幹") => predicate,
        _ => false,
    }
}

pub fn words(text: &[char], ms: &[Morpheme]) -> Vec<Word> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < ms.len() {
        let m = &ms[i];
        let mut j = i + 1;
        let kind = if is_symbol(m) {
            Kind::Punct
        } else if is(m, "助詞") {
            Kind::Function
        } else if is(m, "助動詞") {
            while j < ms.len() && is(&ms[j], "助動詞") {
                j += 1;
            }
            Kind::Function
        } else if is(m, "名詞") && pos(m, 1) == "数詞" {
            // Sudachi's JoinNumeric joins digits already; this joins any it leaves apart.
            while j < ms.len() && is(&ms[j], "名詞") && pos(&ms[j], 1) == "数詞" && ms[j].start == ms[j - 1].end {
                j += 1;
            }
            if j < ms.len() && is_counter(&ms[j]) {
                j += 1;
            }
            Kind::Number
        } else {
            if is(m, "接頭辞") && j < ms.len() && !is_symbol(&ms[j]) && !is(&ms[j], "助詞") {
                j += 1;
            }
            while j < ms.len() && attaches(ms, i, j) {
                j += 1;
            }
            Kind::Word
        };
        out.push(Word { start: m.start, end: ms[j - 1].end, kind, lemmas: lemmas(text, &ms[i..j], kind) });
        i = j;
    }
    out
}

/// The dictionary forms of a word, in Lookupper's order: a する verb (勉強する), a prefixed word
/// (お茶) and its stem, and the head's dictionary form (読ん → 読む, い → いる). The dictionary
/// form keeps the word's spelling; the normalized form would spell some in rare kanji (この →
/// 此の, いる → 居る).
fn lemmas(text: &[char], ms: &[Morpheme], kind: Kind) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: String| {
        if !s.is_empty() && s != "*" && !out.contains(&s) {
            out.push(s);
        }
    };
    let span = |a: &Morpheme, b: &Morpheme| -> String { text[a.start..b.end].iter().collect() };
    match kind {
        Kind::Word => {
            let head = usize::from(is(&ms[0], "接頭辞") && ms.len() > 1);
            let h = &ms[head];
            if let Some(next) = ms.get(head + 1) {
                if is(next, "動詞") && pos(next, 1) == "非自立可能" && takes_suru(h) && is_suru(next) {
                    add(format!("{}する", span(&ms[0], h)));
                }
            }
            if head > 0 {
                add(span(&ms[0], h));
            }
            add(h.detail(DICTIONARY).to_string());
        }
        Kind::Function if conjugates(&ms[0]) => add(ms[0].detail(DICTIONARY).to_string()),
        _ => {}
    }
    let surface = span(&ms[0], &ms[ms.len() - 1]);
    out.retain(|s| *s != surface);
    out
}

#[cfg(test)]
mod tests {
    use super::super::tests::{sentence, show};
    use super::words;

    // Morphemes as Sudachi gives them (split mode C, SudachiDict core 20260723, checked with
    // SudachiPy 0.6.11), unless a test says otherwise.

    #[test]
    fn a_verb_keeps_its_auxiliaries_and_te_iru_and_is_looked_up_by_its_dictionary_form() {
        let (text, ms) = sentence(&[
            ("食べ", "食べ,動詞,一般,*,*,下一段-バ行,連用形-一般,タベ,食べる,食べる"),
            ("させ", "させ,助動詞,*,*,*,下一段-サ行,未然形-一般,サセ,させる,させる"),
            ("られ", "られ,助動詞,*,*,*,助動詞-レル,未然形-一般,ラレ,られる,られる"),
            ("なかっ", "なかっ,助動詞,*,*,*,助動詞-ナイ,連用形-促音便,ナカッ,ない,ない"),
            ("た", "た,助動詞,*,*,*,助動詞-タ,連体形-一般,タ,た,た"),
            ("本", "本,名詞,普通名詞,一般,*,*,*,ホン,本,本"),
            ("を", "を,助詞,格助詞,*,*,*,*,ヲ,を,を"),
            ("読ん", "読ん,動詞,一般,*,*,五段-マ行,連用形-撥音便,ヨン,読む,読む"),
            ("で", "で,助詞,接続助詞,*,*,*,*,デ,で,で"),
            ("いる", "いる,動詞,非自立可能,*,*,上一段-ア行,連体形-一般,イル,居る,いる"),
            ("田中", "田中,名詞,固有名詞,人名,姓,*,*,タナカ,田中,田中"),
            ("さん", "さん,接尾辞,名詞的,一般,*,*,*,サン,さん,さん"),
            ("たち", "たち,接尾辞,名詞的,一般,*,*,*,タチ,達,たち"),
        ]);
        assert_eq!(show(&text, &words(&text, &ms)),
            ["食べさせられなかった→食べる", "本", "を·f", "読んでいる→読む", "田中さんたち→田中"]);
    }

    #[test]
    fn a_suru_noun_a_number_and_a_prefix_are_one_word_each() {
        let (text, ms) = sentence(&[
            ("国際連合", "国際連合,名詞,固有名詞,一般,*,*,*,コクサイレンゴウ,国際連合,国際連合"),
            ("で", "で,助詞,格助詞,*,*,*,*,デ,で,で"),
            ("12", "12,名詞,数詞,*,*,*,*,イチニ,12,12"),
            // A counter joins the number only as 助数詞; SudachiDict gives 人 here as 一般.
            ("人", "人,接尾辞,名詞的,一般,*,*,*,ニン,人,人"),
            ("が", "が,助詞,格助詞,*,*,*,*,ガ,が,が"),
            ("勉強", "勉強,名詞,普通名詞,サ変可能,*,*,*,ベンキョウ,勉強,勉強"),
            ("し", "し,動詞,非自立可能,*,*,サ行変格,連用形-一般,シ,為る,する"),
            ("て", "て,助詞,接続助詞,*,*,*,*,テ,て,て"),
            ("い", "い,動詞,非自立可能,*,*,上一段-ア行,連用形-一般,イ,居る,いる"),
            ("ます", "ます,助動詞,*,*,*,助動詞-マス,終止形-一般,マス,ます,ます"),
            ("。", "。,補助記号,句点,*,*,*,*,。,。,。"),
            ("お", "お,接頭辞,*,*,*,*,*,オ,御,お"),
            ("茶", "茶,名詞,普通名詞,一般,*,*,*,チャ,茶,茶"),
        ]);
        assert_eq!(show(&text, &words(&text, &ms)),
            ["国際連合", "で·f", "12·n", "人", "が·f", "勉強しています→勉強する/勉強", "。·p", "お茶→茶"]);
    }

    #[test]
    fn a_number_takes_a_counter_suffix() {
        // Made up: 冊 tagged as a counter (助数詞), as some SudachiDict entries are; digits apart.
        let (text, ms) = sentence(&[
            ("1", "1,名詞,数詞,*,*,*,*,イチ,1,1"),
            ("3", "3,名詞,数詞,*,*,*,*,サン,3,3"),
            ("冊", "冊,接尾辞,名詞的,助数詞,*,*,*,サツ,冊,冊"),
        ]);
        assert_eq!(show(&text, &words(&text, &ms)), ["13冊·n"]);
    }

    #[test]
    fn adjectives_na_words_and_kana_words_keep_their_written_form() {
        let (text, ms) = sentence(&[
            ("この", "この,連体詞,*,*,*,*,*,コノ,此の,この"),
            ("本", "本,名詞,普通名詞,一般,*,*,*,ホン,本,本"),
            ("は", "は,助詞,係助詞,*,*,*,*,ハ,は,は"),
            ("面白", "面白,形容詞,一般,*,*,形容詞,語幹-一般,オモシロ,面白い,面白い"),
            ("そう", "そう,形状詞,助動詞語幹,*,*,*,*,ソウ,そう,そう"),
            ("だ", "だ,助動詞,*,*,*,助動詞-ダ,終止形-一般,ダ,だ,だ"),
            ("静か", "静か,形状詞,一般,*,*,*,*,シズカ,静か,静か"),
            ("な", "な,助動詞,*,*,*,助動詞-ダ,連体形-一般,ナ,だ,だ"),
            ("飲み", "飲み,動詞,一般,*,*,五段-マ行,連用形-一般,ノミ,飲む,飲む"),
            ("たく", "たく,助動詞,*,*,*,助動詞-タイ,連用形-一般,タク,たい,たい"),
            ("ない", "ない,形容詞,非自立可能,*,*,形容詞,終止形-一般,ナイ,無い,ない"),
        ]);
        assert_eq!(show(&text, &words(&text, &ms)),
            ["この", "本", "は·f", "面白そうだ→面白い", "静かな→静か", "飲みたくない→飲む"]);
    }

    #[test]
    fn a_verb_after_a_plain_particle_or_noun_starts_a_new_word() {
        let (text, ms) = sentence(&[
            ("家", "家,名詞,普通名詞,一般,*,*,*,イエ,家,家"),
            ("に", "に,助詞,格助詞,*,*,*,*,ニ,に,に"),
            ("い", "い,動詞,非自立可能,*,*,上一段-ア行,連用形-一般,イ,居る,いる"),
            ("まし", "まし,助動詞,*,*,*,助動詞-マス,連用形-一般,マシ,ます,ます"),
            ("た", "た,助動詞,*,*,*,助動詞-タ,終止形-一般,タ,た,た"),
            ("と", "と,助詞,格助詞,*,*,*,*,ト,と,と"),
            ("思っ", "思っ,動詞,一般,*,*,五段-ワア行,連用形-促音便,オモッ,思う,思う"),
            ("た", "た,助動詞,*,*,*,助動詞-タ,終止形-一般,タ,た,た"),
        ]);
        assert_eq!(show(&text, &words(&text, &ms)), ["家", "に·f", "いました→いる", "と·f", "思った→思う"]);
    }
}
