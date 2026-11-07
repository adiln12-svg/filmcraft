//! Script ↔ transcript alignment for `script.align`: find every spoken take of each script line in
//! the project's transcripts and rank them.
//!
//! Each line is aligned *globally* (every script word is accounted for) against a *local* stretch
//! of a transcript (the take can start and end anywhere), with free filler words ("um", "uh"),
//! so retakes, false starts and stumbles show up as separate, lower-scoring candidates.

/// Script lines longer than this (in words) are cut to it: alignment cost is lines × words.
pub const MAX_LINE_WORDS: usize = 120;
/// Transcript words considered per item.
pub const MAX_WORDS: usize = 200_000;

const FILLERS: &[&str] = &["um", "uh", "uhm", "umm", "erm", "er", "ah", "eh", "hmm", "mm", "mhm"];

/// One transcribed word, already in seconds of media time.
#[derive(Clone, Debug, PartialEq)]
pub struct TWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub confidence: f32,
}

/// One candidate take of a line.
#[derive(Clone, Debug, PartialEq)]
pub struct Take {
    /// Index into the transcripts passed to [`align`].
    pub source: usize,
    /// Media seconds of the first and just after the last word of the take.
    pub start: f64,
    pub end: f64,
    /// Alignment quality 0..1 (1 = every script word spoken, in order, nothing extra).
    pub score: f32,
    /// Fraction of the line's words spoken exactly.
    pub coverage: f32,
    /// Filler words ("um", "uh") inside the take.
    pub fillers: usize,
    /// Immediate word repetitions inside the take ("the the"): stumbles and restarts.
    pub repeats: usize,
    /// Mean recogniser confidence of the take's words.
    pub confidence: f32,
    /// Spoken words per second.
    pub pace: f32,
    /// The transcript text of the take.
    pub text: String,
    /// Index of the take's first and last word in the source transcript.
    pub word_first: usize,
    pub word_last: usize,
}

/// How one script word lines up with what was transcribed (see [`word_ops`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Script word `.0` was heard as transcript word `.1` (exactly, or one edit away).
    Match(usize, usize),
    /// Script word `.0` sits where transcript word `.1` heard something else.
    Sub(usize, usize),
    /// Script word `.0` has no transcript word.
    Del(usize),
    /// Transcript word `.0` is not in the script.
    Ins(usize),
}

/// The normalised, filler-free words of a script line, each with the index of the display word
/// (`text.split_whitespace()`) it came from.
pub fn script_words(text: &str) -> Vec<(usize, String)> {
    text.split_whitespace()
        .enumerate()
        .flat_map(|(i, w)| tokens(w).into_iter().map(move |t| (i, t)))
        .filter(|(_, w)| !w.is_empty() && !is_filler(w))
        .take(MAX_LINE_WORDS)
        .collect()
}

/// Word-by-word alignment of a line's script words with a stretch of transcript (`hyp`), with the
/// edit operations in order. Fillers in `hyp` come out as insertions.
pub fn word_ops(script: &[String], hyp: &[TWord]) -> Vec<Op> {
    let hyp = hyp.get(..hyp.len().min(MAX_LINE_WORDS * 4)).unwrap_or(&[]);
    // spoken tokens, each with the transcript word it came from ("21" is two tokens of one word)
    let ht: Vec<(String, usize)> = hyp
        .iter()
        .enumerate()
        .flat_map(|(j, w)| {
            let t = tokens(&w.text);
            if t.is_empty() { vec![(String::new(), j)] } else { t.into_iter().map(|x| (x, j)).collect() }
        })
        .collect();
    let word_of = |k: usize| ht.get(k).map(|x| x.1).unwrap_or(0);
    let h: Vec<String> = ht.iter().map(|x| x.0.clone()).collect();
    let (r, n) = (script.len(), h.len());
    // cost: match 0, one-edit match 0.4, substitution 1, gap 1
    let mut d = vec![vec![0f32; n + 1]; r + 1];
    for (i, row) in d.iter_mut().enumerate() {
        if let Some(c) = row.get_mut(0) {
            *c = i as f32;
        }
    }
    for j in 0..=n {
        if let Some(c) = d.get_mut(0).and_then(|row| row.get_mut(j)) {
            *c = j as f32;
        }
    }
    let cost = |i: usize, j: usize| -> f32 {
        let (a, b) = (script.get(i).map(String::as_str).unwrap_or(""), h.get(j).map(String::as_str).unwrap_or(""));
        if is_filler(b) {
            return 2.0; // never pair a filler with a script word
        }
        match sim(a, b) {
            x if x >= 1.0 => 0.0,
            x if x > 0.0 => 0.4,
            _ => 1.0,
        }
    };
    let at = |d: &Vec<Vec<f32>>, i: usize, j: usize| d.get(i).and_then(|row| row.get(j)).copied().unwrap_or(f32::INFINITY);
    for i in 1..=r {
        for j in 1..=n {
            let v = (at(&d, i - 1, j - 1) + cost(i - 1, j - 1)).min(at(&d, i - 1, j) + 1.0).min(at(&d, i, j - 1) + 1.0);
            if let Some(c) = d.get_mut(i).and_then(|row| row.get_mut(j)) {
                *c = v;
            }
        }
    }
    let (mut i, mut j) = (r, n);
    let mut ops = Vec::new();
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && (at(&d, i, j) - (at(&d, i - 1, j - 1) + cost(i - 1, j - 1))).abs() < 1e-4 {
            let c = cost(i - 1, j - 1);
            ops.push(if c < 0.5 { Op::Match(i - 1, j - 1) } else { Op::Sub(i - 1, j - 1) });
            i -= 1;
            j -= 1;
        } else if i > 0 && (at(&d, i, j) - (at(&d, i - 1, j) + 1.0)).abs() < 1e-4 {
            ops.push(Op::Del(i - 1));
            i -= 1;
        } else if j > 0 {
            ops.push(Op::Ins(j - 1));
            j -= 1;
        } else {
            ops.push(Op::Del(i - 1));
            i -= 1;
        }
    }
    ops.reverse();
    // token positions → transcript word positions; one insertion per extra word
    let mut out: Vec<Op> = Vec::with_capacity(ops.len());
    for op in ops {
        let o = match op {
            Op::Match(i, k) => Op::Match(i, word_of(k)),
            Op::Sub(i, k) => Op::Sub(i, word_of(k)),
            Op::Ins(k) => Op::Ins(word_of(k)),
            d => d,
        };
        if matches!(o, Op::Ins(_)) && out.last() == Some(&o) {
            continue;
        }
        out.push(o);
    }
    out
}

/// Normalise a word: lower case, letters/digits/inner apostrophes only.
pub fn norm(w: &str) -> String {
    let s: String = w.chars().filter(|c| c.is_alphanumeric() || *c == '\'' || *c == '’').map(|c| if c == '’' { '\'' } else { c }).collect();
    s.trim_matches('\'').to_lowercase()
}

/// The words a written word stands for when spoken: normalised, with numbers written out
/// ("11" → "eleven", "2026" → "two thousand twenty six"), so a script and a transcript that write
/// numbers differently still line up. Empty for punctuation.
pub fn tokens(w: &str) -> Vec<String> {
    let n = norm(w);
    if !n.is_empty()
        && n.len() <= 6
        && n.chars().all(|c| c.is_ascii_digit())
        && let Ok(v) = n.parse::<u64>()
    {
        return number_words(v).split(' ').map(String::from).collect();
    }
    if n.is_empty() { Vec::new() } else { vec![n] }
}

/// 0..999_999 in words, without "and": `number_words(21)` = "twenty one".
pub fn number_words(v: u64) -> String {
    const ONES: [&str; 20] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
    ];
    const TENS: [&str; 10] = ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];
    fn below_thousand(v: u64) -> Vec<&'static str> {
        let mut out = Vec::new();
        if v >= 100 {
            out.push(ONES.get((v / 100) as usize).copied().unwrap_or(""));
            out.push("hundred");
        }
        let r = v % 100;
        if r >= 20 {
            out.push(TENS.get((r / 10) as usize).copied().unwrap_or(""));
            if !r.is_multiple_of(10) {
                out.push(ONES.get((r % 10) as usize).copied().unwrap_or(""));
            }
        } else if r > 0 || v == 0 {
            out.push(ONES.get(r as usize).copied().unwrap_or(""));
        }
        out
    }
    let v = v.min(999_999);
    let mut words = Vec::new();
    if v >= 1000 {
        words.extend(below_thousand(v / 1000));
        words.push("thousand");
        if !v.is_multiple_of(1000) {
            words.extend(below_thousand(v % 1000));
        }
    } else {
        words.extend(below_thousand(v));
    }
    words.retain(|w| !w.is_empty());
    words.join(" ")
}

pub fn is_filler(w: &str) -> bool {
    FILLERS.contains(&w)
}

/// Similarity of two normalised words: 1 exact, 0.6 for one edit on words of 4+ letters (a
/// mis-heard plural or tense), else 0.
fn sim(a: &str, b: &str) -> f32 {
    if a == b {
        return 1.0;
    }
    let (ac, bc): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if ac.len().min(bc.len()) < 4 || ac.len().abs_diff(bc.len()) > 1 {
        return 0.0;
    }
    // Levenshtein ≤ 1
    let (s, l) = if ac.len() <= bc.len() { (&ac, &bc) } else { (&bc, &ac) };
    let (mut i, mut j, mut edits) = (0, 0, 0);
    while i < s.len() && j < l.len() {
        if s.get(i) == l.get(j) {
            i += 1;
            j += 1;
            continue;
        }
        edits += 1;
        if edits > 1 {
            return 0.0;
        }
        if s.len() == l.len() {
            i += 1;
        }
        j += 1;
    }
    edits += (s.len() - i) + (l.len() - j);
    if edits <= 1 { 0.6 } else { 0.0 }
}

/// Whether two normalised words are the same or one edit apart (see `sim`).
pub fn similar(a: &str, b: &str) -> bool {
    sim(a, b) > 0.0
}

const MATCH: f32 = 2.0;
const MISMATCH: f32 = -1.5;
const GAP: f32 = -1.0;

#[derive(Clone, Copy)]
struct Cell {
    score: f32,
    /// Transcript index (into the filler-free list) where this alignment started.
    start: usize,
    exact: u16,
}

/// Align `script_lines` against `transcripts`. Returns, per line, the candidate takes sorted best
/// first (at most `top`), keeping those scoring at least `min_score`.
pub fn align(script_lines: &[String], transcripts: &[Vec<TWord>], top: usize, min_score: f32) -> Vec<Vec<Take>> {
    let prepared: Vec<Prepared> = transcripts.iter().map(|t| prepare(t)).collect();
    script_lines
        .iter()
        .map(|line| {
            let s: Vec<String> = line.split_whitespace().flat_map(tokens).filter(|w| !w.is_empty() && !is_filler(w)).take(MAX_LINE_WORDS).collect();
            if s.is_empty() {
                return Vec::new();
            }
            let mut takes: Vec<Take> = Vec::new();
            for (src, p) in prepared.iter().enumerate() {
                takes.extend(align_one(&s, p, src, min_score));
            }
            takes.sort_by(|a, b| rank(b).total_cmp(&rank(a)));
            takes.truncate(top.max(1));
            takes
        })
        .collect()
}

/// Ranking key: alignment first, then fewer stumbles, then confidence.
fn rank(t: &Take) -> f32 {
    t.score - 0.03 * t.fillers as f32 - 0.05 * t.repeats as f32 + 0.02 * t.confidence
}

struct Prepared<'a> {
    words: &'a [TWord],
    /// Filler-free normalised words and their index in `words`.
    toks: Vec<(String, usize)>,
}

fn prepare(t: &[TWord]) -> Prepared<'_> {
    let words = t.get(..t.len().min(MAX_WORDS)).unwrap_or(&[]);
    let toks =
        words.iter().enumerate().flat_map(|(i, w)| tokens(&w.text).into_iter().map(move |t| (t, i))).filter(|(w, _)| !w.is_empty() && !is_filler(w)).collect();
    Prepared { words, toks }
}

fn align_one(s: &[String], p: &Prepared<'_>, src: usize, min_score: f32) -> Vec<Take> {
    let (l, n) = (s.len(), p.toks.len());
    if n == 0 {
        return Vec::new();
    }
    let max = MATCH * l as f32;
    // rows over script words; columns over transcript words; free start anywhere in the transcript
    let zero = Cell { score: 0.0, start: 0, exact: 0 };
    let mut prev: Vec<Cell> = (0..=n).map(|j| Cell { start: j, ..zero }).collect();
    let mut cur = vec![zero; n + 1];
    for (i, sw) in s.iter().enumerate() {
        if let Some(c0) = cur.get_mut(0) {
            *c0 = Cell { score: GAP * (i + 1) as f32, start: 0, exact: 0 };
        }
        for j in 1..=n {
            let tw = p.toks.get(j - 1).map(|t| t.0.as_str()).unwrap_or("");
            let m = sim(sw, tw);
            let (Some(d), Some(u), Some(lf)) = (prev.get(j - 1).copied(), prev.get(j).copied(), cur.get(j - 1).copied()) else { continue };
            let diag = Cell { score: d.score + if m > 0.0 { MATCH * m } else { MISMATCH }, start: d.start, exact: d.exact + u16::from(m >= 1.0) };
            let up = Cell { score: u.score + GAP, ..u }; // script word skipped
            let left = Cell { score: lf.score + GAP, ..lf }; // extra transcript word
            let mut best = diag;
            for c in [up, left] {
                if c.score > best.score {
                    best = c;
                }
            }
            if let Some(slot) = cur.get_mut(j) {
                *slot = best;
            }
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    // candidate ends: local maxima of the last row above the threshold; keep non-overlapping
    let mut cands: Vec<(usize, Cell)> = (1..=n).filter_map(|j| prev.get(j).map(|c| (j, *c))).filter(|(_, c)| c.score / max >= min_score).collect();
    cands.sort_by(|a, b| b.1.score.total_cmp(&a.1.score));
    let mut used: Vec<(usize, usize)> = Vec::new();
    let mut out = Vec::new();
    for (end, c) in cands {
        let (a, b) = (c.start, end); // tokens [a, b)
        if b <= a || used.iter().any(|(x, y)| a < *y && *x < b) {
            continue;
        }
        used.push((a, b));
        let (Some(first), Some(last)) = (p.toks.get(a), p.toks.get(b - 1)) else { continue };
        let span = p.words.get(first.1..=last.1).unwrap_or(&[]);
        if span.is_empty() {
            continue;
        }
        let normed: Vec<String> = span.iter().map(|w| norm(&w.text)).collect();
        let fillers = normed.iter().filter(|w| is_filler(w)).count();
        let spoken: Vec<&String> = normed.iter().filter(|w| !w.is_empty() && !is_filler(w)).collect();
        let repeats = spoken.windows(2).filter(|w| w[0] == w[1]).count();
        let (start, end_s) = (span.first().map(|w| w.start).unwrap_or(0.0), span.last().map(|w| w.end).unwrap_or(0.0));
        let dur = (end_s - start).max(1e-3);
        out.push(Take {
            source: src,
            start,
            end: end_s,
            score: (c.score / max).clamp(0.0, 1.0),
            coverage: (c.exact as f32 / l as f32).clamp(0.0, 1.0),
            fillers,
            repeats,
            confidence: span.iter().map(|w| w.confidence.clamp(0.0, 1.0)).sum::<f32>() / span.len() as f32,
            pace: (spoken.len() as f64 / dur) as f32,
            text: span.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "),
            word_first: first.1,
            word_last: last.1,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str, t0: f64) -> Vec<TWord> {
        text.split_whitespace()
            .enumerate()
            .map(|(i, w)| TWord { text: w.into(), start: t0 + i as f64 * 0.4, end: t0 + i as f64 * 0.4 + 0.35, confidence: 0.9 })
            .collect()
    }

    #[test]
    fn finds_every_take_and_ranks_the_clean_one_first() {
        let mut t = words("so the the quick brown fox um jumps over the dog", 0.0);
        t.extend(words("cut", 10.0));
        t.extend(words("the quick brown fox jumps over the lazy dog", 20.0));
        let lines = vec!["The quick brown fox jumps over the lazy dog.".to_string(), "A line nobody said at all".to_string()];
        let r = align(&lines, &[t], 3, 0.5);
        assert_eq!(r.len(), 2);
        let takes = &r[0];
        assert!(takes.len() >= 2, "{takes:?}");
        assert!((takes[0].start - 20.0).abs() < 1e-9, "the clean take wins: {takes:?}");
        assert!(takes[0].score > 0.99 && takes[0].coverage > 0.99 && takes[0].fillers == 0);
        let flubbed = takes.iter().find(|t| t.start < 10.0).unwrap();
        assert!(flubbed.fillers == 1 && flubbed.score < takes[0].score, "{flubbed:?}");
        assert!(r[1].is_empty(), "unspoken line has no take: {:?}", r[1]);
    }

    #[test]
    fn takes_from_several_sources_and_fuzzy_words() {
        let a = words("we built this tool for editors", 5.0);
        let b = words("we build this tool for editor", 1.0);
        let r = align(&["We built this tool for editors".into()], &[b, a], 5, 0.4);
        assert_eq!(r[0][0].source, 1);
        assert!(r[0].iter().any(|t| t.source == 0 && t.score > 0.5 && t.coverage < 1.0));
        assert_eq!(norm("Editors’,"), "editors");
        assert!(sim("editor", "editors") > 0.0 && sim("cat", "cats") == 0.0);
    }

    #[test]
    fn word_ops_name_heard_misheard_missing_and_extra_words() {
        let script: Vec<String> = script_words("The king has fled in disgrace, and your friends").into_iter().map(|x| x.1).collect();
        let hyp = words("the king is flooded disgrace um and friends", 0.0);
        let ops = word_ops(&script, &hyp);
        let dels: Vec<usize> = ops.iter().filter_map(|o| if let Op::Del(i) = o { Some(*i) } else { None }).collect();
        let subs = ops.iter().filter(|o| matches!(o, Op::Sub(..))).count();
        let ins: Vec<usize> = ops.iter().filter_map(|o| if let Op::Ins(j) = o { Some(*j) } else { None }).collect();
        assert!(ops.contains(&Op::Match(0, 0)) && ops.contains(&Op::Match(1, 1)));
        assert!(subs >= 1, "{ops:?}");
        assert!(dels.contains(&7), "\"your\" is missing: {ops:?}");
        assert!(ins.contains(&5), "the filler is extra: {ops:?}");
        assert!(word_ops(&[], &[]).is_empty());
        assert_eq!(word_ops(&script, &[]).len(), script.len());
    }

    #[test]
    fn numbers_written_as_digits_match_numbers_in_words() {
        assert_eq!(number_words(11), "eleven");
        assert_eq!(number_words(2026), "two thousand twenty six");
        assert_eq!(number_words(0), "zero");
        assert_eq!(number_words(u64::MAX), "nine hundred ninety nine thousand nine hundred ninety nine");
        assert_eq!(tokens("21,"), vec!["twenty", "one"]);
        assert_eq!(tokens("o'clock"), vec!["o'clock"]);
        let r = align(&["It's eleven o'clock.".into()], &[words("It's 11 o'clock.", 2.0)], 3, 0.5);
        assert!(r[0].first().is_some_and(|t| t.score > 0.99), "{r:?}");
        let script: Vec<String> = script_words("about 21 days").into_iter().map(|x| x.1).collect();
        let ops = word_ops(&script, &words("about twenty one days", 0.0));
        assert!(ops.iter().all(|o| matches!(o, Op::Match(..))), "{ops:?}");
        let ops = word_ops(&script_words("twenty one").into_iter().map(|x| x.1).collect::<Vec<_>>(), &words("21", 0.0));
        assert_eq!(ops, vec![Op::Match(0, 0), Op::Match(1, 0)]);
    }

    #[test]
    fn empty_and_hostile_inputs() {
        assert!(align(&[], &[], 3, 0.5).is_empty());
        assert!(align(&["".into(), "um uh".into()], &[words("hello", 0.0)], 3, 0.5).iter().all(Vec::is_empty));
        assert!(align(&["hello".into()], &[vec![]], 0, 0.5)[0].is_empty());
        let long = "word ".repeat(10_000);
        let r = align(&[long], &[words("word word word", 0.0)], 2, 0.0);
        assert!(r[0].len() <= 2);
        let weird = vec![TWord { text: "!!".into(), start: f64::NAN, end: -1.0, confidence: f32::NAN }];
        let _ = align(&["!!".into(), "x".into()], &[weird], 3, -5.0);
    }
}
