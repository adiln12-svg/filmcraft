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
}

/// Normalise a word: lower case, letters/digits/inner apostrophes only.
pub fn norm(w: &str) -> String {
    let s: String = w.chars().filter(|c| c.is_alphanumeric() || *c == '\'' || *c == '’').map(|c| if c == '’' { '\'' } else { c }).collect();
    s.trim_matches('\'').to_lowercase()
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
            let s: Vec<String> = line.split_whitespace().map(norm).filter(|w| !w.is_empty() && !is_filler(w)).take(MAX_LINE_WORDS).collect();
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
    let toks = words.iter().enumerate().map(|(i, w)| (norm(&w.text), i)).filter(|(w, _)| !w.is_empty() && !is_filler(w)).collect();
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
        cur[0] = Cell { score: GAP * (i + 1) as f32, start: 0, exact: 0 };
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
        let _ = i;
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
