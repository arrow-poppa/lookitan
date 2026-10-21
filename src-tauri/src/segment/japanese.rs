//! Japanese words from SudachiDict morphemes. Like Lookupper, a word is a content morpheme with
//! what conjugates or attaches after it: a verb or adjective with its auxiliaries, て/で plus the
//! verbs that follow it (いる, しまう, くれる…), a suffix such as さん or たち, a する verb with its
//! noun; a prefix joins the morpheme after it, a number its counter. Particles and auxiliaries
//! standing alone are words of their own, and punctuation is none.
//!
//! lindera's SudachiDict details: display form, part of speech (4 levels), conjugation type,
//! conjugation form, reading, normalized form, then split data.

use super::{Kind, Morpheme, Word};

const POS: usize = 1;
const CONJUGATION_FORM: usize = 6;
const NORMALIZED: usize = 8;

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
            // lindera leaves each digit a morpheme of its own; Sudachi joins them.
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

/// The dictionary forms of a word: a する verb (勉強する), a prefixed word (お茶) and its stem,
/// and the head's dictionary form if it conjugates (読ん → 読む). A word that does not conjugate
/// is looked up as written: SudachiDict's normalized form spells some of them in rare kanji
/// (この → 此の).
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
            add(if conjugates(h) { h.detail(NORMALIZED).to_string() } else { h.surface.clone() });
        }
        Kind::Function if conjugates(&ms[0]) => add(ms[0].detail(NORMALIZED).to_string()),
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

    // Morphemes as lindera's SudachiDict gives them (lindera 6.2.0, checked with its Python build).

    #[test]
    fn a_verb_keeps_its_auxiliaries_and_te_iru_and_is_looked_up_by_its_dictionary_form() {
        let (text, ms) = sentence(&[
            ("食べ", "食べ,動詞,一般,*,*,下一段-バ行,連用形-一般,タベ,食べる,750790,A,*,*,*,*"),
            ("させ", "させ,助動詞,*,*,*,下一段-サ行,未然形-一般,サセ,させる,65526,A,*,*,*,*"),
            ("られ", "られ,助動詞,*,*,*,助動詞-レル,未然形-一般,ラレ,られる,168098,A,*,*,*,*"),
            ("なかっ", "なかっ,助動詞,*,*,*,助動詞-ナイ,連用形-促音便,ナカッ,ない,109401,A,*,*,*,*"),
            ("た", "た,助動詞,*,*,*,助動詞-タ,連体形-一般,タ,た,83558,A,*,*,*,*"),
            ("本", "本,名詞,普通名詞,一般,*,*,*,ホン,本,*,A,*,*,*,012274"),
            ("を", "を,助詞,格助詞,*,*,*,*,ヲ,を,*,A,*,*,*,*"),
            ("読ん", "読ん,動詞,一般,*,*,五段-マ行,連用形-撥音便,ヨン,読む,684363,A,*,*,*,*"),
            ("で", "で,助詞,接続助詞,*,*,*,*,デ,で,*,A,*,*,*,*"),
            ("いる", "いる,動詞,非自立可能,*,*,上一段-ア行,連体形-一般,イル,居る,18836,A,*,*,*,*"),
            ("田中さん", "田中さん,名詞,固有名詞,一般,*,*,*,タナカサン,田中さん,*,C,576189/66971,576189/66971,576189/66971,*"),
            ("たち", "たち,接尾辞,名詞的,一般,*,*,*,タチ,達,*,A,*,*,*,*"),
        ]);
        assert_eq!(show(&text, &words(&text, &ms)),
            ["食べさせられなかった→食べる", "本", "を·f", "読んでいる→読む", "田中さんたち→田中さん"]);
    }

    #[test]
    fn a_suru_noun_a_number_and_a_prefix_are_one_word_each() {
        let (text, ms) = sentence(&[
            ("国際連合", "国際連合,名詞,固有名詞,一般,*,*,*,コクサイレンゴウ,国際連合,*,C,364545/717318,364545/717318,364545/717318,001130"),
            ("で", "で,助詞,格助詞,*,*,*,*,デ,で,*,A,*,*,*,*"),
            ("1", "1,名詞,数詞,*,*,*,*,イチ,1,*,A,*,*,*,017619"),
            ("2", "2,名詞,数詞,*,*,*,*,ニ,2,*,A,*,*,*,*"),
            // A counter joins the number only as 助数詞; SudachiDict gives 人 here as 一般.
            ("人", "人,接尾辞,名詞的,一般,*,*,*,ニン,人,*,A,*,*,*,014991"),
            ("が", "が,助詞,格助詞,*,*,*,*,ガ,が,*,A,*,*,*,*"),
            ("勉強", "勉強,名詞,普通名詞,サ変可能,*,*,*,ベンキョウ,勉強,*,A,*,*,*,008178"),
            ("し", "し,動詞,非自立可能,*,*,サ行変格,連用形-一般,シ,為る,78505,A,*,*,*,*"),
            ("て", "て,助詞,接続助詞,*,*,*,*,テ,て,*,A,*,*,*,*"),
            ("い", "い,動詞,非自立可能,*,*,上一段-ア行,連用形-一般,イ,居る,18836,A,*,*,*,*"),
            ("ます", "ます,助動詞,*,*,*,助動詞-マス,終止形-一般,マス,ます,148492,A,*,*,*,*"),
            ("。", "。,補助記号,句点,*,*,*,*,。,。,*,A,*,*,*,*"),
            ("お", "お,接頭辞,*,*,*,*,*,オ,御,*,A,*,*,*,*"),
            ("茶", "茶,名詞,普通名詞,一般,*,*,*,チャ,茶,*,A,*,*,*,017974"),
        ]);
        assert_eq!(show(&text, &words(&text, &ms)),
            ["国際連合", "で·f", "12·n", "人", "が·f", "勉強しています→勉強する/勉強", "。·p", "お茶→茶"]);
    }

    #[test]
    fn a_number_takes_a_counter_suffix() {
        // 冊 tagged as a counter (助数詞), as some SudachiDict entries are.
        let (text, ms) = sentence(&[
            ("3", "3,名詞,数詞,*,*,*,*,サン,3,*,A,*,*,*,016438"),
            ("冊", "冊,接尾辞,名詞的,助数詞,*,*,*,サツ,冊,*,A,*,*,*,*"),
        ]);
        assert_eq!(show(&text, &words(&text, &ms)), ["3冊·n"]);
    }

    #[test]
    fn adjectives_na_words_and_kana_words_keep_their_written_form() {
        let (text, ms) = sentence(&[
            ("この", "この,連体詞,*,*,*,*,*,コノ,此の,*,A,*,*,*,*"),
            ("本", "本,名詞,普通名詞,一般,*,*,*,ホン,本,*,A,*,*,*,012274"),
            ("は", "は,助詞,係助詞,*,*,*,*,ハ,は,*,A,*,*,*,*"),
            ("面白", "面白,形容詞,一般,*,*,形容詞,語幹-一般,オモシロ,面白い,743287,A,*,*,*,*"),
            ("そう", "そう,形状詞,助動詞語幹,*,*,*,*,ソウ,そう,*,A,*,*,*,*"),
            ("だ", "だ,助動詞,*,*,*,助動詞-ダ,終止形-一般,ダ,だ,89557,A,*,*,*,*"),
            ("静か", "静か,形状詞,一般,*,*,*,*,シズカ,静か,*,A,*,*,*,*"),
            ("な", "な,助動詞,*,*,*,助動詞-ダ,連体形-一般,ナ,だ,89557,A,*,*,*,*"),
            ("飲み", "飲み,動詞,一般,*,*,五段-マ行,連用形-一般,ノミ,飲む,752534,A,*,*,*,*"),
            ("たく", "たく,助動詞,*,*,*,助動詞-タイ,連用形-一般,タク,たい,83573,A,*,*,*,*"),
            ("ない", "ない,形容詞,非自立可能,*,*,形容詞,終止形-一般,ナイ,無い,109410,A,*,*,*,*"),
        ]);
        assert_eq!(show(&text, &words(&text, &ms)),
            ["この", "本", "は·f", "面白そうだ→面白い", "静かな→静か", "飲みたくない→飲む"]);
    }

    #[test]
    fn a_verb_after_a_plain_particle_or_noun_starts_a_new_word() {
        let (text, ms) = sentence(&[
            ("家", "家,名詞,普通名詞,一般,*,*,*,イエ,家,*,A,*,*,*,013447/014167"),
            ("に", "に,助詞,格助詞,*,*,*,*,ニ,に,*,A,*,*,*,*"),
            ("い", "い,動詞,非自立可能,*,*,上一段-ア行,連用形-一般,イ,居る,18836,A,*,*,*,*"),
            ("まし", "まし,助動詞,*,*,*,助動詞-マス,連用形-一般,マシ,ます,148492,A,*,*,*,*"),
            ("た", "た,助動詞,*,*,*,助動詞-タ,終止形-一般,タ,た,83558,A,*,*,*,*"),
            ("と", "と,助詞,格助詞,*,*,*,*,ト,と,*,A,*,*,*,*"),
            ("思っ", "思っ,動詞,一般,*,*,五段-ワア行,連用形-促音便,オモッ,思う,432053,A,*,*,*,*"),
            ("た", "た,助動詞,*,*,*,助動詞-タ,終止形-一般,タ,た,83558,A,*,*,*,*"),
        ]);
        assert_eq!(show(&text, &words(&text, &ms)), ["家", "に·f", "いました→居る", "と·f", "思った→思う"]);
    }
}
