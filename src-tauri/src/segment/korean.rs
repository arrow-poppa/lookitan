//! Korean words from mecab-ko-dic morphemes. Korean puts spaces between words, but a word carries
//! its particles and endings (친구들과, 했습니다), which dictionaries do not list. Like Lookupper
//! with Kiwi, a word is the run of morphemes in one spaced chunk that attach to each other, looked
//! up by its stem: a verb or adjective as stem + 다 (했습니다 → 하다), a noun without its particles
//! (친구들과 → 친구들, 친구), a noun before 이다 (학생이었다 → 학생).
//!
//! lindera's ko-dic details: tag (several joined by + for an inflected form), meaning class, final
//! consonant, reading, type, first tag, last tag, and the expression an inflected form unfolds to
//! (하/VV/*+았/EP/*).

use super::{Kind, Morpheme, Word};

const TAG: usize = 0;
const EXPRESSION: usize = 7;

fn first_tag(m: &Morpheme) -> &str {
    m.detail(TAG).split('+').next().unwrap_or("")
}

fn last_tag(m: &Morpheme) -> &str {
    m.detail(TAG).rsplit('+').next().unwrap_or("")
}

/// Symbols other than foreign words (SL), hanja (SH) and numbers (SN).
fn is_symbol(tag: &str) -> bool {
    tag.starts_with('S') && !matches!(tag, "SL" | "SH" | "SN")
}

fn is_nominal(tag: &str) -> bool {
    tag.starts_with('N') || matches!(tag, "XSN" | "ETN" | "SL" | "SN" | "SH" | "XR")
}

fn is_verbal(tag: &str) -> bool {
    tag.starts_with('V') || tag.starts_with('E') || matches!(tag, "XSV" | "XSA" | "XSM")
}

/// Whether `next` continues the word whose last morpheme so far is `cur`.
fn attaches(next: &Morpheme, cur: &Morpheme) -> bool {
    let (tag, before) = (first_tag(next), last_tag(cur));
    if tag.starts_with('J') || tag == "XSN" {
        is_nominal(before)
    } else if tag.starts_with('E') {
        is_verbal(before)
    } else if matches!(tag, "XSV" | "XSA" | "XSM" | "VCP" | "VCN") {
        is_nominal(before) || before == "MAG"
    } else if tag.starts_with("NNB") {
        matches!(before, "SN" | "NR")
    } else if matches!(tag, "VV" | "VA") {
        is_nominal(before) || before == "MAG"
    } else if tag == "VX" {
        before == "EC"
    } else {
        false
    }
}

pub fn words(text: &[char], ms: &[Morpheme]) -> Vec<Word> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < ms.len() {
        let tag = first_tag(&ms[i]);
        let mut j = i + 1;
        let kind = if is_symbol(tag) {
            Kind::Punct
        } else {
            // Only morphemes written together: a space always ends the word.
            while j < ms.len() && ms[j].start == ms[j - 1].end && attaches(&ms[j], &ms[j - 1]) {
                j += 1;
            }
            if tag.starts_with('J') || tag.starts_with('E') || tag == "XSN" {
                Kind::Function
            } else if matches!(tag, "SN" | "NR") {
                Kind::Number
            } else {
                Kind::Word
            }
        };
        out.push(Word { start: ms[i].start, end: ms[j - 1].end, kind, lemmas: lemmas(text, &ms[i..j], kind) });
        i = j;
    }
    out
}

/// The parts of a morpheme: an inflected form unfolds into its expression's (stem, tag) pairs.
fn parts(m: &Morpheme) -> Vec<(String, String)> {
    let expression = m.detail(EXPRESSION);
    if expression.contains('+') {
        let parts: Vec<(String, String)> = expression
            .split('+')
            .filter_map(|p| {
                let mut bits = p.split('/');
                Some((bits.next()?.to_string(), bits.next()?.to_string()))
            })
            .collect();
        if !parts.is_empty() {
            return parts;
        }
    }
    vec![(m.surface.clone(), m.detail(TAG).to_string())]
}

fn lemmas(text: &[char], ms: &[Morpheme], kind: Kind) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if matches!(kind, Kind::Word | Kind::Number) {
        // The word without its particles and endings.
        let content: Vec<(String, String)> = ms
            .iter()
            .flat_map(parts)
            .filter(|(_, tag)| !tag.starts_with('J') && !tag.starts_with('E') && !is_symbol(tag))
            .collect();
        let join = |n: usize| content[..n].iter().map(|(s, _)| s.as_str()).collect::<String>();
        let verb = content.iter().position(|(_, tag)| tag.starts_with('V') || matches!(tag.as_str(), "XSV" | "XSA"));
        match verb {
            // A noun before the copula 이다 / 아니다.
            Some(v) if v > 0 && matches!(content[v].1.as_str(), "VCP" | "VCN") => out.push(join(v)),
            Some(v) => {
                out.push(format!("{}다", join(v + 1)));
                if v > 0 {
                    out.push(join(v));
                }
            }
            None if !content.is_empty() => {
                out.push(join(content.len()));
                if content.len() > 1 && content[content.len() - 1].1 == "XSN" {
                    out.push(join(content.len() - 1));
                }
            }
            None => {}
        }
    }
    let surface: String = text[ms[0].start..ms[ms.len() - 1].end].iter().collect();
    out.retain(|s| !s.is_empty() && *s != surface);
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::super::tests::{sentence, show};
    use super::words;

    // Morphemes as lindera's ko-dic gives them (lindera 6.2.0, checked with its Python build).

    #[test]
    fn particles_and_endings_stay_with_their_word_which_is_looked_up_by_its_stem() {
        let (text, ms) = sentence(&[
            ("오늘", "NNG,*,T,오늘,*,*,*,*"),
            ("은", "JX,*,T,은,*,*,*,*"),
            (" 날씨", "NNG,*,F,날씨,*,*,*,*"),
            ("가", "JKS,*,F,가,*,*,*,*"),
            (" 좋", "VA,*,T,좋,*,*,*,*"),
            ("아서", "EC,*,F,아서,*,*,*,*"),
            (" 친구", "NNG,*,F,친구,*,*,*,*"),
            ("들", "XSN,*,T,들,*,*,*,*"),
            ("과", "JKB,*,F,과,*,*,*,*"),
            (" 함께", "MAG,성분부사|양태부사,F,함께,*,*,*,*"),
            (" 산책", "NNG,행위,T,산책,*,*,*,*"),
            ("을", "JKO,*,T,을,*,*,*,*"),
            (" 했", "VV+EP,*,T,했,Inflect,VV,EP,하/VV/*+았/EP/*"),
            ("습니다", "EF,*,F,습니다,*,*,*,*"),
            (".", "SF,*,*,*,*,*,*,*"),
        ]);
        assert_eq!(show(&text, &words(&text, &ms)),
            ["오늘은→오늘", "날씨가→날씨", "좋아서→좋다", "친구들과→친구들/친구", "함께", "산책을→산책", "했습니다→하다", ".·p"]);
    }

    #[test]
    fn a_noun_with_hada_or_the_copula_a_negation_and_a_counted_number() {
        let (text, ms) = sentence(&[
            ("공부", "NNG,행위,F,공부,*,*,*,*"),
            ("하", "XSV,*,F,하,*,*,*,*"),
            ("고", "EC,*,F,고,*,*,*,*"),
            (" 있", "VV,*,T,있,*,*,*,*"),
            ("다", "EC,*,F,다,*,*,*,*"),
            (" 학생", "NNG,*,T,학생,*,*,*,*"),
            ("이", "VCP,*,F,이,*,*,*,*"),
            ("었", "EP,*,T,었,*,*,*,*"),
            ("다", "EC,*,F,다,*,*,*,*"),
            (" 먹", "VV,*,T,먹,*,*,*,*"),
            ("지", "EC,*,F,지,*,*,*,*"),
            (" 않", "VX,*,T,않,*,*,*,*"),
            ("았", "EP,*,T,았,*,*,*,*"),
            ("어요", "EF,*,F,어요,*,*,*,*"),
            (" 2", "SN,*,*,*,*,*,*,*"),
            ("권", "NNBC,*,T,권,*,*,*,*"),
            ("을", "JKO,*,T,을,*,*,*,*"),
        ]);
        assert_eq!(show(&text, &words(&text, &ms)),
            ["공부하고→공부하다/공부", "있다", "학생이었다→학생", "먹지→먹다", "않았어요→않다", "2권을·n→2권"]);
    }
}
