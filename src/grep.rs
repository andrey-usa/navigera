//! `text --grep`: the passages of a page's text that mention something.
//!
//! An agent that has to report one fact from a long page (a default value in
//! a reference page, a fee in a policy) otherwise reads the whole text or
//! writes `eval` after `eval` around `innerText.indexOf`. Here the search
//! runs once over the page's text as it reads, and the reply is the matching
//! lines with their neighbours: a few hundred bytes instead of the page.
//!
//! One pass over the text, no allocation per line beyond its lowercase copy,
//! and every size is bounded: the passages shown, the part of a long line
//! around its match, and the reply as a whole.

/// Passages shown when `--limit` is not given.
pub const DEFAULT_PASSAGES: usize = 5;
/// Lines shown before and after a matching line when `--context` is not given.
pub const DEFAULT_CONTEXT: usize = 1;
/// The most lines of context a caller may ask for on each side.
pub const MAX_CONTEXT: usize = 20;
/// Characters of a long line shown before its match, and from the match on.
const BEFORE_CHARS: usize = 160;
const AFTER_CHARS: usize = 320;
/// Characters of a neighbouring line shown.
const CONTEXT_CHARS: usize = 240;
/// The reply stops growing here, whatever the limit asked for.
const REPLY_CHARS: usize = 12_000;

/// Matching lines one passage holds before the next one starts.
const PASSAGE_HITS: usize = 6;

/// Lines `from..=to` of the text (indexes into its non-empty lines), and the
/// matching ones among them with where each matched.
struct Passage {
    from: usize,
    to: usize,
    hits: Vec<(usize, usize)>,
}

/// A lowercase copy with one character for each of the original's, so a
/// position found in it is a position in the original.
fn lower(s: &str) -> String {
    s.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()
}

/// The part of `line` from character `from` for `len` characters, with `…`
/// where it was cut.
fn window(line: &str, from: usize, len: usize) -> String {
    let total = line.chars().count();
    let from = from.min(total);
    let mut out = String::new();
    if from > 0 {
        out.push('…');
    }
    out.extend(line.chars().skip(from).take(len));
    if from + len < total {
        out.push('…');
    }
    out
}

/// Where in a line (in characters) the search matched.
fn match_at(line_lower: &str, phrase: &str, words: &[&str], by_words: bool) -> Option<usize> {
    let chars_before = |byte: usize| line_lower[..byte].chars().count();
    if !by_words {
        return line_lower.find(phrase).map(chars_before);
    }
    let mut first = usize::MAX;
    for w in words {
        first = first.min(line_lower.find(w)?);
    }
    Some(chars_before(first))
}

/// The passages of `text` that mention `query`, as the reply to print.
///
/// A line matches when it has the query as written, letter case aside. When
/// no line does and the query is several words, a line matches when it has
/// all of them in any order. Matching lines closer than the context are one
/// passage.
pub fn grep(text: &str, query: &str, context: usize, limit: usize) -> String {
    let phrase = lower(query.trim());
    if phrase.is_empty() {
        return "text --grep needs something to look for".to_string();
    }
    let context = context.min(MAX_CONTEXT);
    let limit = limit.max(1);
    // Lines that read as something, with their number in the text.
    let lines: Vec<(usize, &str)> =
        text.lines().enumerate().map(|(i, l)| (i + 1, l.trim())).filter(|(_, l)| !l.is_empty()).collect();
    let lowered: Vec<String> = lines.iter().map(|(_, l)| lower(l)).collect();
    let words: Vec<&str> = phrase.split_whitespace().collect();
    let mut by_words = false;
    let mut hits: Vec<(usize, usize)> =
        lowered.iter().enumerate().filter_map(|(i, l)| match_at(l, &phrase, &words, false).map(|at| (i, at))).collect();
    if hits.is_empty() && words.len() > 1 {
        by_words = true;
        hits = lowered
            .iter()
            .enumerate()
            .filter_map(|(i, l)| match_at(l, &phrase, &words, true).map(|at| (i, at)))
            .collect();
    }
    if hits.is_empty() {
        return format!(
            "no line of the text has {query:?} ({} lines searched). Text a page has not opened yet (closed \
             sections, other tabs of a tab list, pages of a list) is not in it; try one distinctive word",
            lines.len()
        );
    }
    // Passages: hits whose context overlaps or touches, a few to a passage,
    // so a page that repeats the word on every line is still many short
    // passages and not one long one.
    let mut passages: Vec<Passage> = Vec::new();
    for &(i, at) in &hits {
        let (from, to) = (i.saturating_sub(context), (i + context).min(lines.len() - 1));
        match passages.last_mut() {
            Some(p) if context > 0 && from <= p.to + 1 && p.hits.len() < PASSAGE_HITS => {
                p.to = to;
                p.hits.push((i, at));
            }
            Some(p) => {
                // The next passage begins no later than its own match.
                p.to = p.to.min(i - 1);
                let from = from.max(p.to + 1);
                passages.push(Passage { from, to, hits: vec![(i, at)] });
            }
            None => passages.push(Passage { from, to, hits: vec![(i, at)] }),
        }
    }
    let total = passages.len();
    let mut body = String::new();
    let mut shown = 0;
    for p in passages.iter().take(limit) {
        let mut part = String::new();
        for (i, (number, line)) in lines.iter().enumerate().take(p.to + 1).skip(p.from) {
            let shown_line = match p.hits.iter().find(|(m, _)| *m == i) {
                Some((_, at)) => {
                    let start = at.saturating_sub(BEFORE_CHARS);
                    window(line, start, at - start + AFTER_CHARS)
                }
                None => window(line, 0, CONTEXT_CHARS),
            };
            part.push_str(&format!("{number}: {shown_line}\n"));
        }
        if shown > 0 && body.len() + part.len() > REPLY_CHARS {
            break;
        }
        if shown > 0 {
            body.push_str("--\n");
        }
        body.push_str(&part);
        shown += 1;
    }
    let what = if by_words { format!("all of {query:?}, in any order") } else { format!("{query:?}") };
    let head = if shown < total {
        format!("{shown} of {total} passages with {what} (--limit {total} for all; N: is the line of the text):")
    } else if total == 1 {
        format!("1 passage with {what}:")
    } else {
        format!("{total} passages with {what}:")
    };
    format!("{head}\n{}", body.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str =
        "dialog: The Dialog element\n\nAttributes\nopen\nIndicates that the dialog box is active. It is \
                        recommended to use the .show() or .showModal() method to render dialogs.\n\nclosedby\n\n\
                        Examples\nThe button opens the dialog via showModal().\nA second line.\nA third line.\n\
                        A fourth line.\nUsage notes\nThe SHOWMODAL name is case-sensitive in script.";

    #[test]
    fn a_match_comes_with_its_neighbours_and_its_line_number() {
        let out = grep(PAGE, "showModal", 1, 5);
        assert!(out.starts_with("2 passages with \"showModal\":\n"), "{out}");
        // The first passage: the line before, the match, the line after.
        assert!(out.contains("4: open\n5: Indicates that the dialog box is active."), "{out}");
        // Two matches whose neighbours touch read as one passage.
        assert!(out.contains("\n7: closedby\n9: Examples\n10: The button"), "{out}");
        assert!(out.contains("\n11: A second line.\n--\n14: Usage notes\n"), "{out}");
        // Letter case does not matter, and the original's case is printed.
        assert!(out.contains("15: The SHOWMODAL name"), "{out}");
    }

    #[test]
    fn close_matches_are_one_passage_and_the_limit_is_said() {
        let text = (1..=40).map(|i| if i % 10 == 0 { "needle".to_string() } else { format!("row {i}") });
        let text = text.collect::<Vec<_>>().join("\n");
        let out = grep(&text, "needle", 0, 2);
        assert!(out.starts_with("2 of 4 passages with \"needle\" (--limit 4 for all"), "{out}");
        assert_eq!(out.matches("needle\n").count() + usize::from(out.ends_with("needle")), 2, "{out}");
        // With a context that reaches the next match, the two are one passage.
        let near = "a\nneedle one\nb\nneedle two\nc";
        let out = grep(near, "needle", 1, 5);
        assert!(out.starts_with("1 passage with"), "{out}");
        assert_eq!(out.matches("--").count(), 0, "{out}");
        // A word on every line: short passages, no line shown twice.
        let every = (0..40).map(|i| format!("needle {i}")).collect::<Vec<_>>().join("\n");
        let out = grep(&every, "needle", 1, 3);
        assert!(out.starts_with("3 of 7 passages"), "{out}");
        assert_eq!(out.lines().filter(|l| l.contains("needle")).count(), 18 + 1, "{out}");
    }

    #[test]
    fn several_words_match_in_any_order_when_the_phrase_is_nowhere() {
        let out = grep("Fees\nThe restocking fee is 8% after 30 days.\nShipping", "days fee", 0, 5);
        assert!(out.starts_with("1 passage with all of \"days fee\", in any order:\n2: The restocking fee"), "{out}");
        // The phrase as written wins when it is there.
        let out = grep("fee days\nThe fee after 30 days", "fee days", 0, 5);
        assert!(out.starts_with("1 passage with \"fee days\":\n1: fee days"), "{out}");
    }

    #[test]
    fn a_long_line_is_shown_around_its_match_and_nothing_found_says_so() {
        let line = format!("{}needle{}", "a".repeat(1000), "b".repeat(1000));
        let out = grep(&line, "needle", 1, 5);
        let shown = out.lines().nth(1).unwrap();
        assert!(shown.starts_with("1: …a") && shown.ends_with("b…"), "{shown}");
        assert!(shown.chars().count() < BEFORE_CHARS + AFTER_CHARS + 10, "{}", shown.chars().count());
        assert!(shown.contains("needle"));
        let none = grep(PAGE, "no such thing", 1, 5);
        assert!(none.starts_with("no line of the text has \"no such thing\" (12 lines searched)"), "{none}");
        assert!(grep(PAGE, "  ", 1, 5).contains("needs something"));
        // A character that lowercases to several keeps positions aligned.
        let out = grep("İstanbul is the needle here", "needle", 0, 5);
        assert!(out.ends_with("1: İstanbul is the needle here"), "{out}");
    }

    #[test]
    fn the_reply_is_bounded_whatever_the_limit() {
        let text =
            (0..5000).map(|i| format!("needle {i} {}", "x".repeat(300))).collect::<Vec<_>>().join("\n\nfiller\n");
        let out = grep(&text, "needle", 0, 5000);
        assert!(out.len() < REPLY_CHARS + 1000, "{}", out.len());
        assert!(out.contains(" of 5000 passages"), "{}", &out[..80]);
    }
}
