//! `text --grep`: the passages of a page's text that mention something.
//!
//! An agent that has to report one fact from a long page (a default value in
//! a reference page, a fee in a policy) otherwise reads the whole text or
//! writes `eval` after `eval` around `innerText.indexOf`. Here the search
//! runs once over the page's text as it reads, and the reply is the matching
//! lines with their neighbours: a few hundred bytes instead of the page.
//!
//! The text is lowercased once into one buffer and searched whole (memchr's
//! vectorized substring search), not line by line, and every size is
//! bounded: the passages shown, the part of a long line around its match,
//! and the reply as a whole.

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
    let mut out = String::with_capacity(s.len());
    lower_into(&mut out, s);
    out
}

/// [`lower`], appended to `out`. Each run of ASCII, which is most of most
/// pages, is copied and lowered in place, bytes at a time; only the
/// characters between the runs are looked up one by one.
fn lower_into(out: &mut String, s: &str) {
    let mut rest = s;
    loop {
        let run = rest.bytes().position(|b| !b.is_ascii()).unwrap_or(rest.len());
        let from = out.len();
        out.push_str(&rest[..run]);
        out[from..].make_ascii_lowercase();
        rest = &rest[run..];
        let Some(c) = rest.chars().next() else { return };
        out.push(c.to_lowercase().next().unwrap_or(c));
        rest = &rest[c.len_utf8()..];
    }
}

/// The lines to search, lowercased into one buffer with a line feed after
/// each: a search runs once over all of it and no line has a copy of its
/// own (a 3.7 MB page of 20,000 lines was 20,000 allocations and as many
/// searches set up).
struct Lowered {
    text: String,
    /// Where each line starts in `text`, and where the last one ends.
    starts: Vec<usize>,
}

impl Lowered {
    fn new<'a>(lines: impl Iterator<Item = &'a str> + Clone) -> Self {
        let bytes: usize = lines.clone().map(|l| l.len() + 1).sum();
        let mut text = String::with_capacity(bytes);
        let mut starts = Vec::with_capacity(lines.clone().count() + 1);
        for line in lines {
            starts.push(text.len());
            lower_into(&mut text, line);
            text.push('\n');
        }
        starts.push(text.len());
        Lowered { text, starts }
    }

    fn lines(&self) -> usize {
        self.starts.len() - 1
    }

    /// Line `i`, lowercased.
    fn line(&self, i: usize) -> &str {
        &self.text[self.starts[i]..self.starts[i + 1] - 1]
    }

    /// Calls `found(line, byte in the line)` for the first place of `needle`
    /// in each line that has it, in line order. A needle with a line feed
    /// is in no line.
    fn firsts(&self, needle: &str, mut found: impl FnMut(usize, usize)) {
        if needle.is_empty() || needle.contains('\n') {
            return;
        }
        let (mut line, mut last) = (0, usize::MAX);
        for at in memchr::memmem::find_iter(self.text.as_bytes(), needle.as_bytes()) {
            while self.starts[line + 1] <= at {
                line += 1;
            }
            if line != last {
                last = line;
                found(line, at - self.starts[line]);
            }
        }
    }

    /// The lines that have every one of `needles` (`all`) or any of them,
    /// each with the character at which the earliest of them starts.
    fn having<S: AsRef<str>>(&self, needles: &[S], all: bool) -> Vec<(usize, usize)> {
        let mut first = vec![usize::MAX; self.lines()];
        let mut have = vec![0usize; self.lines()];
        for needle in needles {
            self.firsts(needle.as_ref(), |i, at| {
                have[i] += 1;
                first[i] = first[i].min(at);
            });
        }
        let need = if all { needles.len().max(1) } else { 1 };
        (0..self.lines()).filter(|&i| have[i] >= need).map(|i| (i, self.line(i)[..first[i]].chars().count())).collect()
    }
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

/// A query written as grep alternation (`a\\|b`, or `a|b`): its parts, when
/// it has two or more. Read only when the query as written matches nothing.
fn alternatives(phrase: &str) -> Vec<String> {
    let parts: Vec<String> =
        phrase.replace("\\|", "|").split('|').map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
    if parts.len() > 1 {
        parts
    } else {
        Vec::new()
    }
}

/// A query written with a regular expression's escapes (`nodejs\\.org`, as
/// for grep): the same without them, when it has any. `\\|` stays: it is
/// alternation, read by [`alternatives`].
fn unescaped(query: &str) -> Option<String> {
    let mut out = String::with_capacity(query.len());
    let mut chars = query.chars().peekable();
    while let Some(c) = chars.next() {
        match chars.peek() {
            Some(&next) if c == '\\' && next.is_ascii_punctuation() && next != '|' => {}
            _ => out.push(c),
        }
    }
    (out.len() < query.len()).then_some(out)
}

/// What a query found in the lines, and how it was read.
struct Found {
    /// Each matching line, with the character its match starts at.
    hits: Vec<(usize, usize)>,
    by_words: bool,
    alts: Vec<String>,
    /// The query as it matched: as written, or without its escapes.
    query: String,
}

impl Found {
    /// A line matches when it has the query as written, letter case aside.
    /// When no line does and the query is several words, a line matches
    /// when it has all of them in any order; when none does either and the
    /// query is grep alternation (`a\\|b`), when it has any of its parts.
    fn read(lowered: &Lowered, query: &str) -> Found {
        let phrase = lower(query);
        let words: Vec<&str> = phrase.split_whitespace().collect();
        let mut by_words = false;
        let mut hits = lowered.having(&[phrase.as_str()], true);
        if hits.is_empty() && words.len() > 1 {
            by_words = true;
            hits = lowered.having(&words, true);
        }
        let mut alts = Vec::new();
        if hits.is_empty() {
            alts = alternatives(&phrase);
            hits = lowered.having(&alts, false);
        }
        Found { hits, by_words, alts, query: query.to_string() }
    }

    /// [`Found::read`] of the query, and when that finds nothing, of the
    /// query without its regular-expression escapes.
    fn of(lowered: &Lowered, query: &str) -> Found {
        let found = Found::read(lowered, query);
        if found.hits.is_empty() {
            if let Some(plain) = unescaped(query) {
                let again = Found::read(lowered, &plain);
                if !again.hits.is_empty() {
                    return again;
                }
            }
        }
        found
    }

    /// How the lines were found, as the reply says it.
    fn what(&self) -> String {
        if !self.alts.is_empty() {
            format!("any of {}", self.alts.iter().map(|a| format!("{a:?}")).collect::<Vec<_>>().join(", "))
        } else if self.by_words {
            format!("all of {:?}, in any order", self.query)
        } else {
            format!("{:?}", self.query)
        }
    }
}

/// The passages of `text` that mention `query`, as the reply to print.
///
/// A line matches when it has the query as written, letter case aside. When
/// no line does and the query is several words, a line matches when it has
/// all of them in any order; when none does either and the query is grep
/// alternation (`a\\|b`), when it has any of its parts. Matching lines
/// closer than the context are one passage.
pub fn grep(text: &str, query: &str, context: usize, limit: usize) -> String {
    let query = query.trim();
    if query.is_empty() {
        return "text --grep needs something to look for".to_string();
    }
    let context = context.min(MAX_CONTEXT);
    let limit = limit.max(1);
    // Lines that read as something, with their number in the text.
    let lines: Vec<(usize, &str)> =
        text.lines().enumerate().map(|(i, l)| (i + 1, l.trim())).filter(|(_, l)| !l.is_empty()).collect();
    let found = Found::of(&Lowered::new(lines.iter().map(|(_, l)| *l)), query);
    let hits = &found.hits;
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
    for &(i, at) in hits {
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
    let what = found.what();
    let head = if shown < total {
        format!("{shown} of {total} passages with {what} (--limit {total} for all; N: is the line of the text):")
    } else if total == 1 {
        format!("1 passage with {what}:")
    } else {
        format!("{total} passages with {what}:")
    };
    format!("{head}\n{}", body.trim_end())
}

/// Matching lines `ax --grep` shows when `--limit` is not given.
pub const DEFAULT_TREE_LINES: usize = 30;

/// The most lines of a table row or list item that `ax --grep` prints whole
/// around a match inside it.
const RECORD_LINES: usize = 12;

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Whether a tree line is a table row or a list item: what a match inside
/// is one field of (a search result's snippet without its title, a cell
/// without the rest of its row, says little).
fn is_record(line: &str) -> bool {
    let line = line.trim_start();
    ["- row", "- listitem"]
        .iter()
        .any(|role| line.strip_prefix(role).is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', ':'])))
}

/// A matching tree line as shown: whole when it is short; a long one from
/// its start when the match is near it, and otherwise its start, then the
/// part around the match. Its ref is kept when the cut would lose it.
fn tree_line(line: &str, at: usize) -> String {
    /// Characters of the line's start (its role and the name's first
    /// words) kept before a match further in.
    const START_CHARS: usize = 40;
    let depth = indent(line);
    let room = BEFORE_CHARS + AFTER_CHARS + depth;
    if line.len() <= room {
        return line.to_string();
    }
    let mut out = if at <= depth + START_CHARS + BEFORE_CHARS {
        window(line, 0, room)
    } else {
        let mut out: String = line.chars().take(depth + START_CHARS).collect();
        out.push_str(&window(line, at - BEFORE_CHARS, room - depth - START_CHARS));
        out
    };
    if let Some(from) = line.rfind(" [ref=") {
        let end = line[from..].find(']').map_or(line.len(), |e| from + e + 1);
        if !out.contains(&line[from..end]) {
            out.push_str(&line[from..end]);
        }
    }
    out
}

/// The lines of an `ax` text tree that mention `query`, each under the
/// containers it sits in, so a ref comes with what it belongs to: the tree,
/// pruned to what was asked for. A match inside a table row or a list item
/// of up to [`RECORD_LINES`] lines comes with all of it. Matching is as for
/// [`grep`]. One pass; a container is printed once, before the first match
/// inside it.
pub fn grep_tree(tree: &str, query: &str, limit: usize) -> String {
    let query = query.trim();
    if query.is_empty() {
        return "ax --grep needs something to look for".to_string();
    }
    let limit = limit.max(1);
    let mut all = tree.lines();
    let head = all.next().unwrap_or("");
    let lines: Vec<&str> = all.filter(|l| l.trim_start().starts_with("- ")).collect();
    let found = Found::of(&Lowered::new(lines.iter().copied()), query);
    let hits = &found.hits;
    if hits.is_empty() {
        return format!(
            "{head}\nno line of the tree has {query:?} ({} lines searched). What a page has not opened yet (closed \
             sections, menus, pages of a list) is not in it; try one distinctive word, or `text --grep` for prose",
            lines.len()
        );
    }
    // The open containers above the current line, and whether each is printed.
    let mut open: Vec<(usize, usize, bool)> = Vec::new();
    let mut out = String::new();
    // Matching lines printed, and the lines before `through` that a row or
    // list item printed whole already covers.
    let (mut shown, mut through) = (0, 0);
    let mut full = false;
    for (i, line) in lines.iter().enumerate() {
        let depth = indent(line);
        while open.last().is_some_and(|(d, _, _)| *d >= depth) {
            open.pop();
        }
        if i < through {
            open.push((depth, i, true));
            continue;
        }
        if shown >= hits.len() || shown >= limit {
            break;
        }
        if hits[shown].0 != i {
            open.push((depth, i, false));
            continue;
        }
        if shown > 0 && out.len() > REPLY_CHARS {
            full = true;
            break;
        }
        // The row or list item the match is in (or is), when it is small:
        // its lines are `from..to`. Otherwise the matching line alone.
        let record =
            if is_record(line) { Some(i) } else { open.iter().rev().map(|o| o.1).find(|&o| is_record(lines[o])) };
        let (from, to) = record
            .and_then(|r| {
                let end = lines[r + 1..].iter().take(RECORD_LINES).position(|l| indent(l) <= indent(lines[r]));
                let end = end.or((lines.len() - r - 1 < RECORD_LINES).then_some(lines.len() - r - 1))?;
                Some((r, r + 1 + end))
            })
            .unwrap_or((i, i + 1));
        for (_, at, printed) in open.iter_mut().filter(|(_, at, printed)| !*printed && *at < from) {
            out.push_str(&window(lines[*at], 0, CONTEXT_CHARS + indent(lines[*at])));
            out.push('\n');
            *printed = true;
        }
        for (j, line) in lines.iter().enumerate().take(to).skip(from) {
            if hits.get(shown).is_some_and(|h| h.0 == j) {
                out.push_str(&tree_line(line, hits[shown].1));
                shown += 1;
            } else {
                out.push_str(&window(line, 0, CONTEXT_CHARS + indent(line)));
            }
            out.push('\n');
        }
        // The match's containers from the record on are printed now.
        for o in open.iter_mut().filter(|o| o.1 >= from) {
            o.2 = true;
        }
        open.push((depth, i, true));
        through = to;
    }
    let what = found.what();
    let count = if full {
        format!(
            "{shown} of {} lines with {what} (the reply is full: add a word, or scope it with a target)",
            hits.len()
        )
    } else if shown < hits.len() {
        format!("{shown} of {} lines with {what} (--limit {} for all)", hits.len(), hits.len())
    } else if hits.len() == 1 {
        format!("1 line with {what}")
    } else {
        format!("{} lines with {what}", hits.len())
    };
    format!("{head}\n{count}, under their containers:\n{}", out.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The search as it was: each line lowered into a copy of its own and
    /// searched by itself. The one-buffer search must find the same lines
    /// at the same characters.
    fn line_by_line(lines: &[&str], needles: &[&str], all: bool) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let low: String = line.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect();
            let found: Vec<usize> = needles.iter().filter_map(|n| low.find(n)).collect();
            let enough = if all { !needles.is_empty() && found.len() == needles.len() } else { !found.is_empty() };
            if enough {
                out.push((i, low[..*found.iter().min().unwrap()].chars().count()));
            }
        }
        out
    }

    #[test]
    fn one_search_over_all_lines_finds_what_a_search_of_each_line_found() {
        // Lines of ASCII and not, letters whose lowercase is shorter in
        // bytes (the Kelvin sign, a dotted capital I, a capital sharp s), a
        // match at a line's first and last character, a line that repeats
        // its match, an empty line, and needles that end where a line ends.
        let fixed = [
            "Late payment fee: 12.99 percent",
            "the FEE for late Payment",
            "",
            "payment",
            "fee fee fee",
            "\u{212a}elvin \u{130}stanbul STRA\u{1e9e}E caf\u{c9} FEE",
            "\u{130}\u{130}\u{130} late",
            "x late payment 12.99",
            "late",
            "a|b and a\\|b",
        ];
        let mut lines: Vec<String> = fixed.iter().map(|l| l.to_string()).collect();
        // A deterministic spread of longer lines around the vector width.
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for n in 0..400 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            let pad = (seed >> 33) as usize % 70;
            let word = ["fee", "Late", "PAYMENT", "caf\u{c9}", "\u{212a}", "zz", "12.99"][(seed >> 20) as usize % 7];
            lines.push(format!("{}{word}{} {n}", "x".repeat(pad), "y".repeat((seed >> 12) as usize % 5)));
        }
        let lines: Vec<&str> = lines.iter().map(|l| l.as_str()).collect();
        let lowered = Lowered::new(lines.iter().copied());
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(lowered.line(i), lower(line));
        }
        let needles: &[&[&str]] = &[
            &["late payment"],
            &["fee"],
            &["late"],
            &["payment"],
            &["caf\u{e9}"],
            &["k"],
            &["i"],
            &["stra\u{df}e"],
            &["12.99"],
            &["zz"],
            &["nowhere"],
            &["fee", "late"],
            &["payment", "late", "fee"],
            &["fee", "fee"],
            &["zz", "nowhere"],
            &["a", "b"],
            &["e\nl"],
            &[],
        ];
        for set in needles {
            for all in [true, false] {
                assert_eq!(lowered.having(set, all), line_by_line(&lines, set, all), "{set:?}, all: {all}");
            }
        }
    }

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
    fn grep_alternation_finds_any_of_its_parts_when_the_whole_matches_nothing() {
        // `a\\|b` as grep writes it, and `a|b` as grep -E does.
        for query in ["closedby\\|usage notes", "closedby|Usage notes"] {
            let out = grep(PAGE, query, 0, 5);
            assert!(out.starts_with("2 passages with any of \"closedby\", \"usage notes\":\n"), "{out}");
            assert!(out.contains("7: closedby") && out.contains("14: Usage notes"), "{out}");
        }
        let tree = "page \"Shop\" http://x.test/\n- banner:\n  - link \"Account\" [ref=3]\n  - link \"Sign in\" [ref=4]\n- main:\n  - heading \"Shop\"";
        let out = grep_tree(tree, "account\\|sign", 30);
        assert!(out.contains("2 lines with any of \"account\", \"sign\", under their containers:"), "{out}");
        assert!(out.contains("[ref=3]") && out.contains("[ref=4]") && !out.contains("heading"), "{out}");
        // A literal `|` that is in the text is found as written.
        assert!(grep("a | b\nc", "a | b", 0, 5).starts_with("1 passage with \"a | b\":"));
        // One part alone is no alternation.
        assert!(grep(PAGE, "nothing here|", 0, 5).starts_with("no line of the text has"));
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
    fn a_tree_is_pruned_to_the_matching_lines_and_their_containers() {
        let tree = "page \"Elements\" http://x/el\n- banner:\n  - link \"Home\" [ref=1] url=/\n- main:\n  - region \"Interactive\":\n    - table:\n      - row:\n        - cell: link \"<dialog>\" [ref=7] url=/el/dialog\n        - cell \"A dialog box or subwindow.\"\n      - row:\n        - cell: link \"<details>\" [ref=9] url=/el/details\n- navigation \"Related\":\n  - link \"<dialog>\" [ref=20] url=/el/dialog";
        let out = grep_tree(tree, "DIALOG", 30);
        assert_eq!(
            out,
            "page \"Elements\" http://x/el\n3 lines with \"DIALOG\", under their containers:\n- main:\n  - region \"Interactive\":\n    - table:\n      - row:\n        - cell: link \"<dialog>\" [ref=7] url=/el/dialog\n        - cell \"A dialog box or subwindow.\"\n- navigation \"Related\":\n  - link \"<dialog>\" [ref=20] url=/el/dialog"
        );
        // A match in one cell comes with its row.
        let out = grep_tree(tree, "details", 30);
        assert!(out.ends_with("      - row:\n        - cell: link \"<details>\" [ref=9] url=/el/details"), "{out}");
        // The limit counts matching lines (a row begun is finished); what
        // was left out is said.
        let out = grep_tree(tree, "dialog", 1);
        assert!(out.contains("2 of 3 lines with \"dialog\" (--limit 3 for all)"), "{out}");
        assert!(out.ends_with("[ref=7] url=/el/dialog\n        - cell \"A dialog box or subwindow.\""), "{out}");
        // Words in any order, and nothing found.
        assert!(grep_tree(tree, "url=/el/details link", 30).contains("1 line with all of"), "{out}");
        assert!(grep_tree(tree, "popover", 30).contains("no line of the tree has \"popover\" (12 lines searched)"));
    }

    #[test]
    fn a_match_comes_with_its_row_or_list_item_when_that_is_small() {
        // A search result: the snippet matches, the title is its sibling.
        let tree = "page \"Results\" http://x/s\n- main:\n  - list:\n    - listitem:\n      - link \"Arm Holdings\" [ref=3] url=/wiki/Arm\n      - text: a reported approach in the weeks before the offering\n    - listitem:\n      - link \"Altera\" [ref=5] url=/wiki/Altera\n      - text: one of the largest listings\n  - table:\n    - row:\n      - cell \"Enel\"\n      - cell \"1999\"\n      - cell \"16.59\"";
        let out = grep_tree(tree, "reported approach", 30);
        assert_eq!(
            out,
            "page \"Results\" http://x/s\n1 line with \"reported approach\", under their containers:\n- main:\n  - list:\n    - listitem:\n      - link \"Arm Holdings\" [ref=3] url=/wiki/Arm\n      - text: a reported approach in the weeks before the offering"
        );
        let out = grep_tree(tree, "16.59", 30);
        assert!(
            out.ends_with(
                "  - table:\n    - row:\n      - cell \"Enel\"\n      - cell \"1999\"\n      - cell \"16.59\""
            ),
            "{out}"
        );
        // Two matches in one list item: it is printed once, both counted.
        let out = grep_tree(tree, "arm|offering", 30);
        assert!(out.contains("2 lines with any of \"arm\", \"offering\""), "{out}");
        assert_eq!(out.matches("Arm Holdings").count(), 1, "{out}");
        assert!(!out.contains("Altera"), "{out}");
        // A list item of many lines is not a record to print: the match alone.
        let items: String = (0..40).map(|i| format!("\n    - link \"entry {i}\" [ref={i}] url=/e/{i}")).collect();
        let big = format!("page \"Menu\" http://x/m\n- list:\n  - listitem:{items}");
        let out = grep_tree(&big, "entry 17", 30);
        assert!(out.ends_with("- list:\n  - listitem:\n    - link \"entry 17\" [ref=17] url=/e/17"), "{out}");
    }

    #[test]
    fn a_query_with_regex_escapes_matches_without_them_when_it_matches_nothing_as_written() {
        let tree = "page \"S\" http://x/\n- link \"nodejs.org\" [ref=1] url=https://nodejs.org/en/download\n- text: a\\.b as written";
        assert!(grep_tree(tree, r"nodejs\.org", 30).contains("1 line with \"nodejs.org\""));
        assert!(grep_tree(tree, r"a\.b", 30).contains("1 line with \"a\\\\.b\""));
        assert!(grep_tree(tree, r"nodejs\.org\|absent", 30).contains("1 line with any of \"nodejs.org\", \"absent\""));
        assert!(grep("see nodejs.org (LTS)", r"nodejs\.org \(LTS\)", 0, 5)
            .starts_with("1 passage with \"nodejs.org (LTS)\""));
    }

    #[test]
    fn a_long_tree_line_is_shown_around_its_match_with_its_ref_and_the_reply_is_bounded() {
        let name = format!("{} needle {}", "a".repeat(2000), "z".repeat(2000));
        let tree = format!("page \"L\" http://x/\n- main:\n  - link \"{name}\" [ref=42] url=/far");
        let out = grep_tree(&tree, "needle", 30);
        let line = out.lines().last().unwrap();
        assert!(line.starts_with("  - link \"aaaa") && line.contains("…aaa") && line.contains(" needle zzz"), "{line}");
        assert!(line.ends_with("… [ref=42]") && line.chars().count() < 520, "{line}");
        let many: String = (0..5000).map(|i| format!("\n- text: needle {i} {}", "x".repeat(300))).collect();
        let out = grep_tree(&format!("page \"M\" http://x/{many}"), "needle", 5000);
        assert!(out.len() < REPLY_CHARS + 1000, "{}", out.len());
        assert!(out.contains(" of 5000 lines with \"needle\" (the reply is full"), "{}", &out[..120]);
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
