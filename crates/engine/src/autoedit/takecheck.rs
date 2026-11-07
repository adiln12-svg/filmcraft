//! What a take's audio says about it, independently of what the recogniser wrote down.
//!
//! Whisper tends to tidy speech: a stumble ("I also— I also offered"), a false start or a filler
//! can vanish from the transcript, and the word around it simply gets longer. These checks read
//! the take's own audio (mono, 16 kHz) on a 10 ms level envelope:
//!
//! - **hidden speech**: a transcribed word holding far more voiced time than the word needs;
//! - **repeated audio**: the same sound pattern spoken twice in a row (band-energy self-similarity);
//! - **pauses**: silences inside the line longer than the allowed pause;
//! - **noise** (speech-to-floor level) and **distortion** (clipped samples);
//! - **cut points**: the in and out points moved into silence, flagged when speech would be cut.
//!
//! Hard flags make a take unusable; soft flags only lower its rank.

use filmcraft_audio_dsp::fft::Fft;

use super::align::TWord;

/// Voiced time beyond a word's expected length (seconds) that counts as hidden speech.
pub const HIDDEN_EXCESS: f64 = 0.35;

/// Envelope hop (seconds).
pub const HOP: f64 = 0.01;

/// Problem severity: hard = the take must not be used; soft = usable, ranked lower.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Hard,
    Soft,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Hard => "hard",
            Severity::Soft => "soft",
        }
    }
}

/// One finding about a take.
#[derive(Clone, Debug, PartialEq)]
pub struct Flag {
    pub kind: &'static str,
    pub severity: Severity,
    /// Media seconds the finding is about.
    pub at: Option<f64>,
    pub detail: String,
}

/// Limits for the checks.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Longest silence allowed inside a line (seconds).
    pub max_pause: f64,
    /// Below this speech-to-floor level (dB) the take cannot be verified (hard).
    pub min_snr_db: f32,
    /// Below this, the take is noisy (soft).
    pub noisy_snr_db: f32,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_pause: 0.6, min_snr_db: 6.0, noisy_snr_db: 15.0 }
    }
}

/// The level envelope of a whole item: dBFS per 10 ms frame, plus the samples for band analysis.
pub struct Envelope<'a> {
    pub samples: &'a [f32],
    pub sr: u32,
    pub db: Vec<f32>,
}

impl<'a> Envelope<'a> {
    pub fn new(samples: &'a [f32], sr: u32) -> Self {
        let hop = ((sr as f64 * HOP) as usize).max(1);
        let db = samples
            .chunks(hop)
            .map(|c| {
                let e = c.iter().filter(|x| x.is_finite()).map(|x| x * x).sum::<f32>() / c.len().max(1) as f32;
                10.0 * e.max(1e-12).log10()
            })
            .collect();
        Self { samples, sr, db }
    }

    fn frame(&self, t: f64) -> usize {
        ((t.max(0.0) / HOP) as usize).min(self.db.len())
    }

    fn time(&self, f: usize) -> f64 {
        f as f64 * HOP
    }
}

/// Levels of a take: noise floor, speech level, the active/silent threshold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    pub floor_db: f32,
    pub speech_db: f32,
    pub threshold_db: f32,
    pub snr_db: f32,
}

fn percentile(v: &mut [f32], p: f32) -> f32 {
    if v.is_empty() {
        return -120.0;
    }
    v.sort_by(f32::total_cmp);
    let i = ((v.len() - 1) as f32 * p.clamp(0.0, 1.0)).round() as usize;
    v.get(i).copied().unwrap_or(-120.0)
}

/// Levels of the take `[a, b)` seconds: the floor from its quietest tenth (the gaps between
/// words carry the room noise), the speech level from its loud frames. Takes too short to have
/// gaps borrow a second of context each side for the floor. Digital silence (below −90 dBFS, e.g.
/// padding between files) is ignored so it cannot pass for a clean floor.
pub fn levels(env: &Envelope<'_>, a: f64, b: f64) -> Levels {
    let real = |v: &[f32]| v.iter().copied().filter(|d| *d > -90.0).collect::<Vec<f32>>();
    let mut body = real(env.db.get(env.frame(a)..env.frame(b)).unwrap_or(&[]));
    let mut floor_src = if body.len() >= 80 { body.clone() } else { real(env.db.get(env.frame(a - 1.0)..env.frame(b + 1.0)).unwrap_or(&[])) };
    let floor = percentile(&mut floor_src, 0.1).max(-90.0);
    let speech = percentile(&mut body, 0.9).max(floor);
    let snr = speech - floor;
    let threshold = (floor + (0.35 * snr).max(6.0)).min(speech - 6.0).max(floor + 3.0);
    Levels { floor_db: floor, speech_db: speech, threshold_db: threshold, snr_db: snr }
}

/// Voiced seconds in `[a, b)`.
fn active_seconds(env: &Envelope<'_>, lv: &Levels, a: f64, b: f64) -> f64 {
    env.db.get(env.frame(a)..env.frame(b)).unwrap_or(&[]).iter().filter(|d| **d > lv.threshold_db).count() as f64 * HOP
}

/// What the audio of one take shows.
#[derive(Clone, Debug, PartialEq)]
pub struct TakeAudio {
    pub levels: Levels,
    /// Voiced seconds between the take's first and last word.
    pub voiced: f64,
    pub flags: Vec<Flag>,
}

/// Check the take whose transcript words are `words` (media seconds).
pub fn check(env: &Envelope<'_>, words: &[TWord], limits: &Limits) -> TakeAudio {
    let (Some(first), Some(last)) = (words.first(), words.last()) else {
        let lv = Levels { floor_db: -120.0, speech_db: -120.0, threshold_db: -120.0, snr_db: 0.0 };
        return TakeAudio { levels: lv, voiced: 0.0, flags: Vec::new() };
    };
    let (a, b) = (first.start.min(last.end), first.start.max(last.end));
    let lv = levels(env, a, b);
    let mut flags = Vec::new();
    let reliable = lv.snr_db >= limits.min_snr_db;
    if !reliable {
        flags.push(Flag {
            kind: "too_noisy",
            severity: Severity::Hard,
            at: Some(a),
            detail: format!("speech only {:.0} dB above the noise floor: too noisy to verify the read", lv.snr_db),
        });
    } else if lv.snr_db < limits.noisy_snr_db {
        flags.push(Flag { kind: "noisy", severity: Severity::Soft, at: Some(a), detail: format!("speech {:.0} dB above the noise floor", lv.snr_db) });
    }
    if reliable {
        // hidden speech: a word carrying much more voiced time than it can account for
        for w in words {
            // the spoken form: "11" is said as "eleven"
            let chars = super::align::tokens(&w.text).join(" ").chars().count() as f64;
            let expected = 0.30 + 0.085 * chars;
            let voiced = active_seconds(env, &lv, w.start, w.end);
            if voiced > 1.5 * expected && voiced - expected > HIDDEN_EXCESS {
                flags.push(Flag {
                    kind: "hidden_speech",
                    severity: Severity::Hard,
                    at: Some(w.start),
                    detail: format!(
                        "{voiced:.2} s of speech inside \"{}\" (expected ≤ {expected:.2} s): words the transcript left out, such as a stumble or restart",
                        w.text
                    ),
                });
            }
        }
        // pauses inside the line
        let (fa, fb) = (env.frame(a), env.frame(b));
        let body = env.db.get(fa..fb).unwrap_or(&[]);
        let first_on = body.iter().position(|d| *d > lv.threshold_db);
        let last_on = body.iter().rposition(|d| *d > lv.threshold_db);
        if let (Some(s0), Some(s1)) = (first_on, last_on) {
            let mut run = 0usize;
            for (k, d) in body.iter().enumerate().take(s1 + 1).skip(s0) {
                if *d <= lv.threshold_db {
                    run += 1;
                } else {
                    if run as f64 * HOP > limits.max_pause {
                        let t = env.time(fa + k) - run as f64 * HOP;
                        flags.push(Flag {
                            kind: "pause",
                            severity: Severity::Soft,
                            at: Some(t),
                            detail: format!("{:.2} s pause inside the line (allowed {:.2} s)", run as f64 * HOP, limits.max_pause),
                        });
                    }
                    run = 0;
                }
            }
        }
        if let Some((t, sim)) = repetition(env, &lv, a, b) {
            flags.push(Flag {
                kind: "repeated_audio",
                severity: Severity::Hard,
                at: Some(t),
                detail: format!("the same sounds are spoken twice in a row near {t:.2} s (similarity {sim:.2}): a stumble or restart"),
            });
        }
    }
    // distortion
    let (sa, sb) = (((a * env.sr as f64) as usize).min(env.samples.len()), ((b * env.sr as f64) as usize).min(env.samples.len()));
    let span = env.samples.get(sa..sb).unwrap_or(&[]);
    let clipped = span.iter().filter(|x| x.abs() >= 0.995).count();
    if !span.is_empty() && clipped * 2000 > span.len() {
        flags.push(Flag { kind: "distorted", severity: Severity::Soft, at: Some(a), detail: format!("{clipped} clipped samples") });
    }
    TakeAudio { levels: lv, voiced: active_seconds(env, &lv, a, b), flags }
}

/// Bands for the repetition check.
const BANDS: usize = 16;
/// Window compared (frames of 10 ms) and the lags searched.
/// Calibrated on genuine speech (DisfluencySpeech restarts against LibriSpeech, CREMA-D and
/// DisfluencySpeech clean reads): 0.2 s windows, 0.15 s apart or more, similarity ≥ 0.95 caught
/// 74% of restarts with about 1% false alarms on clean reads; shorter windows or a lower similarity
/// caught more restarts but flagged 15–50% of clean reads.
const WIN: usize = 20;
const LAG_MIN: usize = 15;
const LAG_MAX: usize = 150;
/// Correlation above which two windows count as the same sounds.
pub const REPEAT_SIMILARITY: f32 = 0.95;

/// Log band energies per 10 ms frame over `[a, b)`.
fn bands(env: &Envelope<'_>, a: f64, b: f64) -> Vec<[f32; BANDS]> {
    let n = 512usize;
    let fft = Fft::new(n);
    let hop = ((env.sr as f64 * HOP) as usize).max(1);
    let (s0, s1) = (((a * env.sr as f64) as usize).min(env.samples.len()), ((b * env.sr as f64) as usize).min(env.samples.len()));
    let mut out = Vec::new();
    let (mut re, mut im) = (vec![0f32; n], vec![0f32; n]);
    let mut at = s0;
    while at + n <= s1.max(s0 + n).min(env.samples.len()) && at < s1 && out.len() < 6000 {
        let Some(chunk) = env.samples.get(at..at + n) else { break };
        for (k, (r, x)) in re.iter_mut().zip(chunk).enumerate() {
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / n as f32).cos();
            *r = if x.is_finite() { x * w } else { 0.0 };
        }
        im.iter_mut().for_each(|v| *v = 0.0);
        fft.forward(&mut re, &mut im);
        let mut e = [0f32; BANDS];
        // bands spaced evenly on a log axis from 100 Hz to 7 kHz
        for (bi, slot) in e.iter_mut().enumerate() {
            let lo = 100.0 * (70.0f32).powf(bi as f32 / BANDS as f32);
            let hi = 100.0 * (70.0f32).powf((bi + 1) as f32 / BANDS as f32);
            let (k0, k1) = ((lo * n as f32 / env.sr as f32) as usize, ((hi * n as f32 / env.sr as f32) as usize).max(1));
            let mut acc = 0f32;
            for k in k0..=k1.min(n / 2) {
                let (x, y) = (re.get(k).copied().unwrap_or(0.0), im.get(k).copied().unwrap_or(0.0));
                acc += x * x + y * y;
            }
            *slot = (acc + 1e-9).ln();
        }
        out.push(e);
        at += hop;
    }
    out
}

/// The strongest repetition of a voiced 0.2 s stretch 0.15–1.5 s later, if it is above
/// [`REPEAT_SIMILARITY`]: (media seconds of the first occurrence, similarity).
fn repetition(env: &Envelope<'_>, lv: &Levels, a: f64, b: f64) -> Option<(f64, f32)> {
    let feats = bands(env, a, b);
    let f0 = env.frame(a);
    let voiced = |k: usize| env.db.get(f0 + k).is_some_and(|d| *d > lv.threshold_db);
    let n = feats.len();
    if n < WIN + LAG_MIN {
        return None;
    }
    let window = |k: usize| -> Option<Vec<f32>> {
        let w = feats.get(k..k + WIN)?;
        if w.len() < WIN || (k..k + WIN).filter(|&i| voiced(i)).count() * 10 < WIN * 7 {
            return None;
        }
        let flat: Vec<f32> = w.iter().flat_map(|f| f.iter().copied()).collect();
        let mean = flat.iter().sum::<f32>() / flat.len() as f32;
        let var = flat.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / flat.len() as f32;
        (var > 1e-6).then(|| flat.iter().map(|x| (x - mean) / var.sqrt()).collect())
    };
    let mut best: Option<(usize, f32)> = None;
    let mut k = 0;
    while k + WIN + LAG_MIN <= n {
        if let Some(x) = window(k) {
            for lag in LAG_MIN..=LAG_MAX {
                let Some(y) = window(k + lag) else { continue };
                let r = x.iter().zip(&y).map(|(p, q)| p * q).sum::<f32>() / x.len() as f32;
                if best.is_none_or(|bst| r > bst.1) {
                    best = Some((k, r));
                }
            }
        }
        k += 5;
    }
    best.filter(|b| b.1 >= REPEAT_SIMILARITY).map(|(k, r)| (a + k as f64 * HOP, r))
}

/// Cut points for a take, found in the audio rather than taken from the word times (the
/// recogniser stretches a word's end over the silence after it). The voiced run around the first
/// word is followed back to its onset and the run around the last word forward to its end; the
/// cuts go `pad_in` / `pad_out` into the silence beyond, but never into the neighbouring speech.
/// `n` holds the neighbouring transcript words (media seconds): the previous word's start and end
/// and the next word's start and end. Voiced runs are followed no further than the neighbours'
/// far edges; the search for syllables the recogniser missed stays inside their near edges.
/// A hard `clipped_*` flag means there is less than 30 ms of silence between this take and the
/// speech next to it.
pub fn cut_points(env: &Envelope<'_>, lv: &Levels, words: &[TWord], pad_in: f64, pad_out: f64, n: Neighbours) -> (f64, f64, Vec<Flag>) {
    let (not_before, not_after) = (n.prev_start, n.next_end);
    let mut flags = Vec::new();
    let (Some(first), Some(last)) = (words.first(), words.last()) else { return (0.0, 0.0, flags) };
    let on = |f: usize| env.db.get(f).is_some_and(|d| *d > lv.threshold_db);
    let lo = env.frame(not_before.max(0.0));
    let hi = env.frame(not_after).max(env.frame(last.end)).min(env.db.len());
    // onset: back from the first word's start while voiced, or forward to its first voiced frame
    let f0 = env.frame(first.start).max(lo);
    let mut onset = if on(f0) {
        let mut f = f0;
        while f > lo && on(f - 1) && f0 - f < 300 {
            f -= 1;
        }
        f
    } else {
        (f0..env.frame(first.end).max(f0 + 1)).find(|&f| on(f)).unwrap_or(f0)
    };
    // also catch syllables just before the word that the recogniser placed late
    let inner_lo = env.frame(n.prev_end.max(0.0)).max(lo);
    let mut probe = onset;
    while probe > inner_lo && onset - probe < 20 {
        probe -= 1;
        if on(probe) {
            onset = probe;
        }
    }
    // offset: forward from the last word's end while voiced, or back to its last voiced frame
    let f1 = env.frame(last.end).min(hi.saturating_sub(1));
    let mut offset = if on(f1) {
        let mut f = f1;
        while f + 1 < hi && on(f + 1) && f - f1 < 300 {
            f += 1;
        }
        f + 1
    } else {
        (env.frame(last.start)..=f1).rev().find(|&f| on(f)).map(|f| f + 1).unwrap_or(f1)
    };
    let inner_hi = env.frame(n.next_start).min(hi);
    let mut probe = offset;
    while probe < inner_hi && probe - offset < 20 {
        if on(probe) {
            offset = probe + 1;
        }
        probe += 1;
    }
    // hysteresis: speech edges fade below the detection threshold (a trailing "s", a drawn-out
    // vowel); follow them outward while the level stays clearly above the noise floor
    let low_db = lv.floor_db + (0.15 * lv.snr_db).clamp(6.0, (lv.threshold_db - lv.floor_db).max(6.0));
    let audible = |f: usize| env.db.get(f).is_some_and(|d| *d > low_db);
    let mut steps = 0;
    while onset > inner_lo && audible(onset - 1) && steps < 50 {
        onset -= 1;
        steps += 1;
    }
    let mut steps = 0;
    while offset < inner_hi && audible(offset) && steps < 50 {
        offset += 1;
        steps += 1;
    }
    // room before the onset and after the offset with no speech in it (judged at the detection
    // threshold: a faint tail or room tone next to a cut does not clip a word)
    let quiet_before = (lo..onset).rev().take_while(|&f| !on(f)).count();
    let quiet_after = (offset..hi).take_while(|&f| !on(f)).count();
    let pad_in_f = ((pad_in / HOP).round() as usize).min(quiet_before.saturating_sub(1));
    let pad_out_f = ((pad_out / HOP).round() as usize).min(quiet_after.saturating_sub(1));
    let cut_in = env.time(onset.saturating_sub(pad_in_f));
    let cut_out = env.time(offset + pad_out_f).max(cut_in);
    if quiet_before < 3 && onset > 0 {
        flags.push(Flag {
            kind: "clipped_in",
            severity: Severity::Hard,
            at: Some(cut_in),
            detail: format!("no silence before the line at {cut_in:.2} s: the cut would clip speech"),
        });
    }
    if quiet_after < 3 && offset < env.db.len() {
        flags.push(Flag {
            kind: "clipped_out",
            severity: Severity::Hard,
            at: Some(cut_out),
            detail: format!("no silence after the line at {cut_out:.2} s: the cut would clip speech"),
        });
    }
    (cut_in, cut_out, flags)
}

/// The transcript words around a take (media seconds); use 0 / the media length at the ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Neighbours {
    pub prev_start: f64,
    pub prev_end: f64,
    pub next_start: f64,
    pub next_end: f64,
}

impl Neighbours {
    pub fn open(len: f64) -> Self {
        Self { prev_start: 0.0, prev_end: 0.0, next_start: len, next_end: len }
    }
}

/// Voiced seconds between `a` and `b` (media seconds), for callers outside this module.
pub fn voiced_between(env: &Envelope<'_>, lv: &Levels, a: f64, b: f64) -> f64 {
    active_seconds(env, lv, a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 16_000;

    /// "Speech": noise bursts shaped like syllables (`(start, len)` seconds), each with its own
    /// spectral colour from `seed`, so different syllables do not look alike.
    fn speech(total: f64, syllables: &[(f64, f64, u32)], noise: f32) -> Vec<f32> {
        let n = (total * SR as f64) as usize;
        let mut v = vec![0f32; n];
        let mut s = 99u32;
        for x in v.iter_mut() {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *x = ((s >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * noise;
        }
        for &(t0, len, seed) in syllables {
            let (f1, f2) = (200.0 + (seed % 7) as f32 * 90.0, 900.0 + (seed % 11) as f32 * 160.0);
            for i in 0..(len * SR as f64) as usize {
                let t = i as f32 / SR as f32;
                let env = (std::f32::consts::PI * t / len as f32).sin();
                let y = 0.3 * env * ((2.0 * std::f32::consts::PI * f1 * t).sin() + 0.5 * (2.0 * std::f32::consts::PI * f2 * t + seed as f32).sin());
                if let Some(x) = v.get_mut((t0 * SR as f64) as usize + i) {
                    *x += y;
                }
            }
        }
        v
    }

    fn w(text: &str, a: f64, b: f64) -> TWord {
        TWord { text: text.into(), start: a, end: b, confidence: 1.0 }
    }

    /// A five-syllable line at `t0`.
    fn line(t0: f64) -> Vec<(f64, f64, u32)> {
        (0..5).map(|k| (t0 + k as f64 * 0.3, 0.22, k as u32 * 13 + 5)).collect()
    }

    fn words_at(t0: f64) -> Vec<TWord> {
        (0..5).map(|k| w(["the", "king", "has", "fled", "away"][k], t0 + k as f64 * 0.3, t0 + k as f64 * 0.3 + 0.27)).collect()
    }

    #[test]
    fn a_clean_take_has_no_flags_and_cuts_in_silence() {
        let a = speech(4.0, &line(1.0), 0.001);
        let env = Envelope::new(&a, SR);
        let words = words_at(1.0);
        let r = check(&env, &words, &Limits::default());
        assert!(r.flags.is_empty(), "{:?}", r.flags);
        assert!(r.levels.snr_db > 30.0);
        let (i, o, f) = cut_points(&env, &r.levels, &words, 0.08, 0.12, Neighbours::open(4.0));
        assert!(f.is_empty(), "{f:?}");
        assert!((0.85..1.0).contains(&i) && (2.4..2.6).contains(&o), "{i} {o}");
    }

    #[test]
    fn a_stumble_the_transcript_hides_is_found_two_ways() {
        // "the king has fl— the king has fled away": the first four syllables are said twice, but the
        // transcript (as Whisper writes it) has only one "the", stretched over the restart
        let mut syl = vec![(1.0, 0.22, 5), (1.3, 0.22, 18), (1.6, 0.22, 31), (1.9, 0.22, 44)];
        syl.extend(line(2.4));
        let a = speech(5.0, &syl, 0.001);
        let env = Envelope::new(&a, SR);
        let mut words = words_at(2.4);
        words[0].start = 1.0;
        let r = check(&env, &words, &Limits::default());
        let kinds: Vec<&str> = r.flags.iter().map(|f| f.kind).collect();
        assert!(kinds.contains(&"hidden_speech"), "{kinds:?}");
        assert!(kinds.contains(&"repeated_audio"), "{kinds:?}");
        assert!(r.flags.iter().all(|f| f.kind != "hidden_speech" || f.severity == Severity::Hard));
    }

    #[test]
    fn pauses_noise_and_clipped_cuts() {
        // a 1 s pause between syllables 3 and 4
        let syl: Vec<(f64, f64, u32)> = line(1.0).into_iter().enumerate().map(|(k, s)| if k >= 3 { (s.0 + 1.0, s.1, s.2) } else { s }).collect();
        let a = speech(5.0, &syl, 0.001);
        let env = Envelope::new(&a, SR);
        let mut words = words_at(1.0);
        for x in words.iter_mut().skip(3) {
            x.start += 1.0;
            x.end += 1.0;
        }
        words[2].end = words[3].start; // the transcript absorbs the pause into "has"
        let r = check(&env, &words, &Limits::default());
        assert!(r.flags.iter().any(|f| f.kind == "pause" && f.severity == Severity::Soft), "{:?}", r.flags);
        assert!(!r.flags.iter().any(|f| f.kind == "hidden_speech"), "silence is not hidden speech: {:?}", r.flags);
        // a noisy take
        let n = speech(4.0, &line(1.0), 0.25);
        let r = check(&Envelope::new(&n, SR), &words_at(1.0), &Limits::default());
        assert!(r.flags.iter().any(|f| f.kind == "noisy" || f.kind == "too_noisy"), "{:?}", r.flags);
        // the next line starts right after: the out point cannot reach silence
        let mut syl = line(1.0);
        syl.push((2.45, 0.3, 77));
        let a = speech(4.0, &syl, 0.001);
        let env = Envelope::new(&a, SR);
        let words = words_at(1.0);
        let lv = check(&env, &words, &Limits::default()).levels;
        let (_, _, f) = cut_points(&env, &lv, &words, 0.08, 0.12, Neighbours { prev_start: 0.0, prev_end: 0.0, next_start: 2.45, next_end: 2.75 });
        assert!(f.iter().any(|x| x.kind == "clipped_out"), "{f:?}");
    }

    #[test]
    fn hostile_inputs() {
        let env = Envelope::new(&[], SR);
        let r = check(&env, &[], &Limits::default());
        assert!(r.flags.is_empty());
        let r = check(&env, &[w("x", f64::NAN, -3.0)], &Limits::default());
        let _ = cut_points(
            &env,
            &r.levels,
            &[w("x", f64::NAN, 1e12)],
            -1.0,
            f64::INFINITY,
            Neighbours { prev_start: 5.0, prev_end: f64::NAN, next_start: -5.0, next_end: -1.0 },
        );
        let nan = vec![f32::NAN; 3000];
        let env = Envelope::new(&nan, 0);
        let _ = check(&env, &words_at(0.0), &Limits::default());
    }
}
