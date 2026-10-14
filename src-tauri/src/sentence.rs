// Sentence boundaries in OCR text, following the rules Lookupper applies to the context of a word:
// - a sentence ends at . ! ? … ‼ ⁇ ⁈ ⁉ or the CJK 。！？．｡; a run of them ("?!", "...") counts once;
// - the run ends a sentence when it holds ! ? (or their doubled forms), a CJK mark, or is a single
//   "."; an ellipsis alone ("...", "…") does not;
// - closing quotes and brackets right after the run stay in the sentence;
// - a CJK mark always ends the sentence; otherwise the next character must be whitespace and the
//   next word must not start in lowercase ("3.14", "U.S.A", "the end. and" do not split);
// - a single "." after one letter ("J.", "e.g.") or after Mr, Mrs, Ms, Dr, Prof, St, Jr, Sr, Mt,
//   vs, Fig or Vol is an abbreviation, not the end of a sentence.

use std::ops::Range;

const ABBREVIATIONS: [&str; 12] = ["mr", "mrs", "ms", "dr", "prof", "st", "jr", "sr", "mt", "vs", "fig", "vol"];

fn is_terminator(c: char) -> bool {
    is_strong(c) || is_cjk_terminator(c) || c == '.' || c == '…'
}

/// Marks that end a sentence even when repeated or combined.
fn is_strong(c: char) -> bool {
    matches!(c, '!' | '?' | '‼' | '⁇' | '⁈' | '⁉')
}

/// Full-width marks of unspaced scripts: they end a sentence without a following space.
fn is_cjk_terminator(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '．' | '｡')
}

/// Closing quotes and brackets that belong to the sentence they follow.
fn is_closer(c: char) -> bool {
    matches!(c, '"' | '\'' | ')' | ']' | '»' | '’' | '”' | '›' | '〉' | '》' | '」' | '』' | '】' | '〕' | '）')
}

/// Whether the single "." at `dot` closes an abbreviation: one letter, or a known title or label.
fn is_abbreviation(text: &[char], dot: usize) -> bool {
    let mut start = dot;
    while start > 0 && text[start - 1].is_alphabetic() {
        start -= 1;
    }
    match dot - start {
        0 => false,
        1 => true,
        _ => {
            let word: String = text[start..dot].iter().flat_map(|c| c.to_lowercase()).collect();
            ABBREVIATIONS.contains(&word.as_str())
        }
    }
}

/// Splits `text` into consecutive sentence ranges (char indices) that cover it entirely; a
/// sentence keeps its trailing spaces.
pub fn split(text: &[char]) -> Vec<Range<usize>> {
    let n = text.len();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < n {
        if !is_terminator(text[i]) {
            i += 1;
            continue;
        }
        let run = i;
        let (mut strong, mut cjk) = (false, false);
        while i < n && is_terminator(text[i]) {
            strong |= is_strong(text[i]);
            cjk |= is_cjk_terminator(text[i]);
            i += 1;
        }
        let single_dot = i - run == 1 && text[run] == '.';
        while i < n && is_closer(text[i]) {
            i += 1;
        }
        if !(strong || cjk || single_dot) || i == n {
            continue;
        }
        if !cjk {
            if !text[i].is_whitespace() {
                continue;
            }
            let next = (i..n).find(|&j| !text[j].is_whitespace());
            match next {
                None => continue,
                Some(j) if text[j].is_lowercase() => continue,
                Some(_) => {}
            }
            if !strong && is_abbreviation(text, run) {
                continue;
            }
        }
        out.push(start..i);
        start = i;
    }
    if start < n {
        out.push(start..n);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::split;

    fn sentences(s: &str) -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        split(&chars).into_iter().map(|r| chars[r].iter().collect::<String>().trim().to_string()).collect()
    }

    #[test]
    fn splits_at_terminal_punctuation() {
        assert_eq!(sentences("Hello world. How are you? Fine!"), ["Hello world.", "How are you?", "Fine!"]);
    }

    #[test]
    fn keeps_abbreviations_initials_and_numbers_together() {
        assert_eq!(sentences("Mr. Smith met Dr. Who. They left."), ["Mr. Smith met Dr. Who.", "They left."]);
        assert_eq!(sentences("J. R. R. Tolkien wrote it. Yes."), ["J. R. R. Tolkien wrote it.", "Yes."]);
        assert_eq!(sentences("Use e.g. a pen. Then sign."), ["Use e.g. a pen.", "Then sign."]);
        assert_eq!(sentences("It costs 3.14 now. Ok."), ["It costs 3.14 now.", "Ok."]);
    }

    #[test]
    fn ellipsis_and_lowercase_continue_the_sentence() {
        assert_eq!(sentences("Wait... what? Yes."), ["Wait... what?", "Yes."]);
        assert_eq!(sentences("So… we go. Now."), ["So… we go.", "Now."]);
        assert_eq!(sentences("the end. and more"), ["the end. and more"]);
    }

    #[test]
    fn closing_quotes_stay_with_their_sentence() {
        assert_eq!(sentences("He said \"stop.\" Then he left."), ["He said \"stop.\"", "Then he left."]);
        assert_eq!(sentences("Really?! (Yes.) Good."), ["Really?!", "(Yes.)", "Good."]);
    }

    #[test]
    fn cjk_marks_split_without_spaces() {
        assert_eq!(sentences("今日は晴れ。明日は「雨」？そう！"), ["今日は晴れ。", "明日は「雨」？", "そう！"]);
        assert_eq!(sentences("「行こう。」と言った。"), ["「行こう。」", "と言った。"]);
    }
}
