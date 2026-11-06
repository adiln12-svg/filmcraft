//! Captions whose text is the approved script, timed to the dialogue as it was actually spoken.
//!
//! The recogniser only supplies *timing*: each script word takes the time of the transcript word it
//! was aligned with, and words the recogniser missed or misheard take times interpolated between
//! their aligned neighbours. The caption *text* is always the script's, verbatim, so a mishearing
//! can never reach the screen. Cues are cut so each renders in at most the allowed number of lines
//! (checked with the renderer's own layout through `fit`), preferably at punctuation, and are held a
//! little into following silence when that makes them easier to read.

/// One script line to caption.
#[derive(Clone, Debug, PartialEq)]
pub struct LineWords {
    pub line: usize,
    /// The line's display words (`text.split_whitespace()`).
    pub words: Vec<String>,
    /// Sequence seconds of each display word, where the alignment gave one.
    pub times: Vec<Option<(f64, f64)>>,
    /// Sequence seconds the line's clip occupies (bounds for interpolation and holds).
    pub window: (f64, f64),
}

/// One caption cue.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub line: usize,
    pub start: f64,
    pub end: f64,
    /// Text with the line breaks it will be drawn with.
    pub text: String,
    /// How many of its words had interpolated (not recognised) timing.
    pub interpolated: usize,
}

impl Cue {
    pub fn chars(&self) -> usize {
        self.text.chars().filter(|c| *c != '\n').count()
    }
    pub fn cps(&self) -> f64 {
        self.chars() as f64 / (self.end - self.start).max(1e-3)
    }
}

/// Timing rules.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub min_seconds: f64,
    pub max_cps: f64,
    /// Longest hold into the following silence.
    pub max_hold: f64,
    /// Gap kept before the next cue.
    pub gap: f64,
}

impl Default for Timing {
    fn default() -> Self {
        Self { min_seconds: 0.8, max_cps: 20.0, max_hold: 0.7, gap: 0.04 }
    }
}

/// Fill missing word times: interpolate between aligned neighbours in proportion to word length;
/// with no aligned word at all, spread the words over the window.
pub fn fill_times(l: &LineWords) -> (Vec<(f64, f64)>, Vec<bool>) {
    let n = l.words.len();
    let (w0, w1) = (l.window.0.min(l.window.1), l.window.0.max(l.window.1));
    let weight = |i: usize| l.words.get(i).map(|w| w.chars().count().max(1) as f64).unwrap_or(1.0);
    let known: Vec<Option<(f64, f64)>> = (0..n).map(|i| l.times.get(i).copied().flatten().filter(|(a, b)| a.is_finite() && b.is_finite() && b >= a)).collect();
    let mut out = vec![(w0, w0); n];
    let mut guessed = vec![false; n];
    let mut i = 0;
    while i < n {
        if let Some(t) = known.get(i).copied().flatten() {
            if let Some(o) = out.get_mut(i) {
                *o = t;
            }
            i += 1;
            continue;
        }
        // a run of unknown words [i, j)
        let j = (i..n).find(|&k| known.get(k).copied().flatten().is_some()).unwrap_or(n);
        let from = if i == 0 { w0 } else { out.get(i - 1).map(|x| x.1).unwrap_or(w0) };
        let to = known.get(j).copied().flatten().map(|x| x.0).unwrap_or(w1).max(from);
        let total: f64 = (i..j).map(weight).sum();
        let mut t = from;
        for k in i..j {
            let d = (to - from) * weight(k) / total.max(1.0);
            if let Some(o) = out.get_mut(k) {
                *o = (t, t + d);
            }
            if let Some(g) = guessed.get_mut(k) {
                *g = true;
            }
            t += d;
        }
        i = j;
    }
    (out, guessed)
}

/// Build cues for `lines` (in sequence order). `fit(words)` returns the cue text with its line
/// breaks when those words render within the allowed lines, else `None`.
///
/// Each line is split where the fastest-reading cue is slowest (when no split keeps every cue
/// under `max_cps`): a cue may stay up until the next
/// word is spoken (or, for the last cue of a line, a short hold into the silence after it), so the
/// split decides how the spoken time is shared between cues. Ties go to fewer cues and to breaks
/// after punctuation.
pub fn cues(lines: &[LineWords], timing: &Timing, fit: &dyn Fn(&[String]) -> Option<String>) -> Vec<Cue> {
    let mut out: Vec<Cue> = Vec::new();
    for l in lines {
        let (times, guessed) = fill_times(l);
        let n = l.words.len().min(times.len());
        if n == 0 {
            continue;
        }
        let window_end = l.window.0.max(l.window.1);
        // reading speed of words [i, j) as one cue
        let cps = |i: usize, j: usize| -> f64 {
            let chars: usize = l.words.get(i..j).map(|w| w.iter().map(|x| x.chars().count()).sum::<usize>() + (j - i).saturating_sub(1)).unwrap_or(0);
            let start = times.get(i).map(|t| t.0).unwrap_or(0.0);
            let until = if j < n {
                times.get(j).map(|t| t.0).unwrap_or(start)
            } else {
                (times.get(j - 1).map(|t| t.1).unwrap_or(start) + timing.max_hold).min(window_end.max(start))
            };
            chars as f64 / (until - start).max(0.05)
        };
        let punct = |j: usize| l.words.get(j - 1).is_some_and(|w| w.ends_with([',', '.', ';', ':', '?', '!', '—']));
        // best[j] = (worst cps, cues, non-punctuation breaks, previous break)
        // a one-word cue flashes by: allowed only in short lines or when nothing else fits
        let mut best: Vec<Option<(f64, usize, usize, usize)>> = vec![None; n + 1];
        for allow_single in [n < 3, true] {
            best = vec![None; n + 1];
            best[0] = Some((0.0, 0, 0, 0));
            for j in 1..=n {
                for i in (0..j).rev() {
                    let Some(prev) = best.get(i).copied().flatten() else { continue };
                    let words = l.words.get(i..j).unwrap_or(&[]);
                    if j - i > 1 && fit(words).is_none() {
                        break; // longer cues starting earlier will not fit either
                    }
                    if j - i == 1 && !allow_single {
                        continue;
                    }
                    let cand = (prev.0.max(cps(i, j)), prev.1 + 1, prev.2 + usize::from(j < n && !punct(j)), i);
                    let better = match best.get(j).copied().flatten() {
                        None => true,
                        Some(b) => {
                            // any split within the limit reads fine: then punctuation and fewer cues decide
                            let key = |v: f64| (v.max(timing.max_cps) * 10.0).round();
                            let (wc, wb) = (key(cand.0), key(b.0));
                            wc < wb || (wc == wb && (cand.1, cand.2) < (b.1, b.2))
                        }
                    };
                    if better && let Some(slot) = best.get_mut(j) {
                        *slot = Some(cand);
                    }
                }
            }
            if best.get(n).copied().flatten().is_some() {
                break;
            }
        }
        let mut cuts = Vec::new();
        let mut j = n;
        while j > 0 {
            let i = best.get(j).copied().flatten().map(|b| b.3).unwrap_or(0);
            cuts.push((i, j));
            j = i;
        }
        cuts.reverse();
        for (i, j) in cuts {
            let ws = l.words.get(i..j).unwrap_or(&[]);
            let text = fit(ws).unwrap_or_else(|| ws.join(" "));
            let start = times.get(i).map(|t| t.0).unwrap_or(l.window.0);
            let end = times.get(j - 1).map(|t| t.1).unwrap_or(start).max(start);
            out.push(Cue { line: l.line, start, end, text, interpolated: guessed.get(i..j).map(|g| g.iter().filter(|x| **x).count()).unwrap_or(0) });
        }
    }
    // order, then let each cue stay up until the next one (readability), within the hold limit
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    for k in 0..out.len() {
        let next = out.get(k + 1).map(|c| c.start).unwrap_or(f64::INFINITY);
        let Some(c) = out.get_mut(k) else { continue };
        let need = (c.chars() as f64 / timing.max_cps.max(1.0)).max(timing.min_seconds);
        let reach = if next.is_finite() { next - timing.gap } else { c.end + timing.max_hold };
        let limit = reach.max(c.end);
        c.end = (c.start + need).max(c.end).min(limit).max(c.end.min(limit));
        if next.is_finite() && next - c.end < 0.25 {
            // close a flicker-short gap: stay up until the next cue
            c.end = (next - timing.gap).max(c.end);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, t0: f64, step: f64, missing: &[usize]) -> LineWords {
        let words: Vec<String> = text.split_whitespace().map(String::from).collect();
        let times = (0..words.len()).map(|i| (!missing.contains(&i)).then_some((t0 + i as f64 * step, t0 + i as f64 * step + step * 0.9))).collect();
        LineWords { line: 1, window: (t0, t0 + words.len() as f64 * step + 0.3), words, times }
    }

    /// At most `n` characters per line, two lines.
    fn fit(n: usize) -> impl Fn(&[String]) -> Option<String> {
        move |ws: &[String]| {
            let t = ws.join(" ");
            if t.chars().count() <= n {
                return Some(t);
            }
            (1..ws.len())
                .map(|k| (ws[..k].join(" "), ws[k..].join(" ")))
                .filter(|(a, b)| a.chars().count() <= n && b.chars().count() <= n)
                .min_by_key(|(a, b)| a.chars().count().abs_diff(b.chars().count()))
                .map(|(a, b)| format!("{a}\n{b}"))
        }
    }

    #[test]
    fn captions_carry_the_script_words_with_spoken_timing() {
        let l = line("The king has fled in disgrace, and your friends are asking for you.", 1.0, 0.3, &[2, 3, 4]);
        let cues = cues(std::slice::from_ref(&l), &Timing::default(), &fit(22));
        let text: Vec<String> = cues.iter().flat_map(|c| c.text.split_whitespace().map(String::from).collect::<Vec<_>>()).collect();
        assert_eq!(text, l.words, "every script word, verbatim, in order");
        assert!(cues.iter().all(|c| c.text.lines().count() <= 2 && c.text.lines().all(|x| x.chars().count() <= 22)));
        assert_eq!(cues[0].text.split_whitespace().last(), Some("disgrace,"), "broken at punctuation: {cues:?}");
        assert!(cues[0].interpolated == 3);
        assert!((cues[0].start - 1.0).abs() < 1e-9);
        assert!(cues.windows(2).all(|w| w[0].end <= w[1].start), "{cues:?}");
    }

    #[test]
    fn no_one_word_cues_in_longer_lines() {
        // "I" is held long before the rest is said quickly: still not a cue of its own
        let mut l = line("I also offered to help your brother to escape but he would not go.", 0.0, 0.2, &[]);
        l.times[0] = Some((0.0, 0.6));
        let c = cues(&[l], &Timing::default(), &fit(22));
        assert!(c.iter().all(|x| x.text.split_whitespace().count() > 1), "{c:?}");
    }

    #[test]
    fn interpolation_and_holds() {
        let l = LineWords { line: 2, words: vec!["a".into(), "bb".into(), "cccc".into()], times: vec![None, None, None], window: (2.0, 5.0) };
        let (t, g) = fill_times(&l);
        assert!(g.iter().all(|x| *x));
        assert!((t[0].0 - 2.0).abs() < 1e-9 && (t[2].1 - 5.0).abs() < 1e-9);
        assert!(t.windows(2).all(|w| w[0].1 <= w[1].0 + 1e-9));
        // a short cue is held to the minimum duration when silence follows
        let l = line("hi there", 0.0, 0.1, &[]);
        let c = cues(&[l], &Timing::default(), &fit(40));
        assert!((c[0].end - c[0].start - 0.8).abs() < 1e-9, "{c:?}");
        // but never into the next cue
        let mut a = line("one", 0.0, 0.2, &[]);
        let mut b = line("two", 0.3, 0.2, &[]);
        b.line = 2;
        a.window.1 = 0.3;
        let c = cues(&[a, b], &Timing::default(), &fit(40));
        assert!(c[0].end <= c[1].start - 0.04 + 1e-9, "{c:?}");
    }

    #[test]
    fn hostile_inputs() {
        assert!(cues(&[], &Timing::default(), &fit(10)).is_empty());
        let l = LineWords { line: 1, words: vec!["supercalifragilistic".into()], times: vec![Some((f64::NAN, 1.0))], window: (3.0, 1.0) };
        let c = cues(&[l], &Timing { max_cps: 0.0, ..Default::default() }, &fit(5));
        assert_eq!(c.len(), 1, "a word too long for a line still gets a cue: {c:?}");
        let l = LineWords { line: 1, words: vec!["x".into(); 3], times: vec![], window: (0.0, 0.0) };
        let _ = cues(&[l], &Timing::default(), &fit(1));
    }
}
