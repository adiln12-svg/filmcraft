//! Beat tracking for `analyze.beats`: onset strength (half-wave rectified spectral flux of a
//! log-magnitude STFT), a global tempo from the autocorrelation of that envelope, the beat phase
//! that best fits the envelope, and each grid beat snapped to the nearest strong onset.
//!
//! Pure functions on mono samples, so they are tested on synthetic click tracks.

use filmcraft_audio_dsp::fft::Fft;

/// STFT size and hop (at any sample rate; resolution is plenty for cut placement).
const WIN: usize = 1024;
const HOP: usize = 512;
/// Below this grid confidence there is no steady beat to follow (speech, ambience, free time):
/// no tempo or beats are reported, only onsets. Steady click tracks and grooves measure about 1;
/// noise, speech-like bursts and irregular hits stay under 0.15.
pub const MIN_CONFIDENCE: f32 = 0.3;
/// Longest input analysed (samples are capped by the caller too): 20 minutes at 48 kHz.
pub const MAX_SAMPLES: usize = 48_000 * 60 * 20;

/// Result of [`analyze`].
#[derive(Clone, Debug, PartialEq)]
pub struct Beats {
    /// Tempo in beats per minute (0 when no periodicity was found).
    pub bpm: f64,
    /// Beat times, seconds from the start of the input.
    pub beats: Vec<f64>,
    /// Downbeats (every fourth beat, starting from the strongest phase), seconds.
    pub downbeats: Vec<f64>,
    /// Strong onsets (transients: hits, plucks, syllables), seconds, with strength 0..1.
    pub onsets: Vec<(f64, f32)>,
    /// Share of onset strength on the beat grid (and its half beats) above chance, 0..1.
    pub confidence: f32,
}

/// Options for [`analyze`].
#[derive(Clone, Copy, Debug)]
pub struct BeatOptions {
    pub min_bpm: f64,
    pub max_bpm: f64,
}

impl Default for BeatOptions {
    fn default() -> Self {
        Self { min_bpm: 60.0, max_bpm: 200.0 }
    }
}

/// Onset-strength envelope: one value per hop (`HOP` samples), normalised to 0..1.
pub fn onset_envelope(samples: &[f32]) -> Vec<f32> {
    envelopes(samples, 0).0
}

/// Fewer onsets than this cannot establish a tempo.
const MIN_ONSETS: usize = 8;

/// Below this frequency, onsets are kicks and bass notes: where listeners put the beat.
const LOW_HZ: f64 = 200.0;

/// The full-band onset envelope and the one of the bins below `LOW_HZ` (`sample_rate` 0: no low
/// band), each normalised to 0..1.
fn envelopes(samples: &[f32], sample_rate: u32) -> (Vec<f32>, Vec<f32>) {
    let samples = samples.get(..samples.len().min(MAX_SAMPLES)).unwrap_or(&[]);
    if samples.len() < WIN {
        return (Vec::new(), Vec::new());
    }
    let low_bins = if sample_rate == 0 { 0 } else { ((LOW_HZ * WIN as f64 / sample_rate as f64).ceil() as usize).clamp(1, WIN / 2) };
    let fft = Fft::new(WIN);
    let window: Vec<f32> = (0..WIN).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / WIN as f32).cos()).collect();
    let frames = (samples.len() - WIN) / HOP + 1;
    let bins = WIN / 2;
    let mut prev = vec![0f32; bins];
    let mut env = Vec::with_capacity(frames);
    let mut low = Vec::with_capacity(frames);
    let (mut re, mut im) = (vec![0f32; WIN], vec![0f32; WIN]);
    for f in 0..frames {
        let at = f * HOP;
        let Some(chunk) = samples.get(at..at + WIN) else { break };
        for ((r, x), w) in re.iter_mut().zip(chunk).zip(&window) {
            *r = if x.is_finite() { x * w } else { 0.0 };
        }
        im.iter_mut().for_each(|v| *v = 0.0);
        fft.forward(&mut re, &mut im);
        let (mut flux, mut low_flux) = (0f32, 0f32);
        for (k, p) in prev.iter_mut().enumerate() {
            let (a, b) = (re.get(k).copied().unwrap_or(0.0), im.get(k).copied().unwrap_or(0.0));
            let mag = (1.0 + 1000.0 * (a * a + b * b).sqrt()).ln();
            let d = mag - *p;
            if d > 0.0 {
                flux += d;
                if k < low_bins {
                    low_flux += d;
                }
            }
            *p = mag;
        }
        env.push(if f == 0 { 0.0 } else { flux });
        low.push(if f == 0 { 0.0 } else { low_flux });
    }
    (detrend(&env), detrend(&low))
}

/// Remove the slowly moving local mean, keep the positive part, normalise to 0..1.
fn detrend(env: &[f32]) -> Vec<f32> {
    let w = 8usize;
    let mut out = vec![0f32; env.len()];
    for (i, o) in out.iter_mut().enumerate() {
        let (a, b) = (i.saturating_sub(w), (i + w + 1).min(env.len()));
        let slice = env.get(a..b).unwrap_or(&[]);
        let mean = slice.iter().sum::<f32>() / slice.len().max(1) as f32;
        *o = (env.get(i).copied().unwrap_or(0.0) - mean).max(0.0);
    }
    let max = out.iter().copied().fold(0f32, f32::max);
    if max > 0.0 {
        out.iter_mut().for_each(|v| *v /= max);
    }
    out
}

/// Track tempo and beats in `samples` (mono, `sample_rate` Hz).
pub fn analyze(samples: &[f32], sample_rate: u32, opts: BeatOptions) -> Beats {
    let empty = Beats { bpm: 0.0, beats: Vec::new(), downbeats: Vec::new(), onsets: Vec::new(), confidence: 0.0 };
    if sample_rate == 0 {
        return empty;
    }
    let (env, low) = envelopes(samples, sample_rate);
    let raw = env.clone();
    if env.len() < 16 {
        return empty;
    }
    let hop_s = HOP as f64 / sample_rate as f64;
    let onsets = peaks(&env, hop_s);
    if onsets.len() < MIN_ONSETS {
        return Beats { onsets, ..empty };
    }
    let (min_bpm, max_bpm) = {
        let (a, b) = (opts.min_bpm.clamp(20.0, 400.0), opts.max_bpm.clamp(20.0, 400.0));
        if a <= b { (a, b) } else { (b, a) }
    };
    // lag range in hops
    let lag_lo = ((60.0 / max_bpm) / hop_s).ceil().max(1.0) as usize;
    let lag_hi = (((60.0 / min_bpm) / hop_s).ceil() as usize).min(env.len() / 2).max(lag_lo);
    // tempo and phase work on a smoothed envelope: onsets are a hop or two wide, so a period that
    // is not a whole number of hops would otherwise drift off them and favour a multiple
    let env = smooth(&env);
    let energy: f32 = env.iter().map(|v| v * v).sum::<f32>() / env.len().max(1) as f32;
    if energy <= 0.0 {
        return Beats { onsets, ..empty };
    }
    // unbiased: mean product over the overlap, so long lags are not penalised
    let ac = |lag: usize| -> f32 {
        let n = env.len().saturating_sub(lag);
        if n == 0 {
            return 0.0;
        }
        env.iter().zip(env.iter().skip(lag)).map(|(a, b)| a * b).sum::<f32>() / n as f32
    };
    // score each lag by its autocorrelation plus its double (helps pick the beat, not the
    // half-beat), weighted gently toward 120 BPM, the usual perceptual centre
    let mut best = (0usize, f32::MIN);
    for lag in lag_lo..=lag_hi {
        let bpm = 60.0 / (lag as f64 * hop_s);
        let prior = (-0.5 * ((bpm / 120.0).log2() / 1.0).powi(2)).exp() as f32;
        let score = (ac(lag) + 0.5 * ac(lag * 2)) * (0.5 + 0.5 * prior);
        if score > best.1 {
            best = (lag, score);
        }
    }
    let lag = best.0.max(1);
    // refine the period to a fraction of a hop with a parabola through the neighbours
    let (l0, l1, l2) = (ac(lag.saturating_sub(1).max(1)), ac(lag), ac(lag + 1));
    let denom = l0 - 2.0 * l1 + l2;
    let frac = if denom.abs() > 1e-9 { (0.5 * (l0 - l2) / denom).clamp(-0.5, 0.5) } else { 0.0 };
    let period = (lag as f64 + frac as f64).max(1.0);
    // phase: the offset whose grid collects the most onset strength
    let steps = (period.ceil() as usize).max(1);
    let mut phase = (0f64, f32::MIN);
    for k in 0..steps {
        let mut sum = 0f32;
        let mut t = k as f64;
        while (t as usize) < env.len() {
            sum += env.get(t.round() as usize).copied().unwrap_or(0.0);
            t += period;
        }
        if sum > phase.1 {
            phase = (k as f64, sum);
        }
    }
    // confidence: the share of all onset strength (unsmoothed) within a few hops of the beat grid
    // or its half-beat subdivisions, above the share onsets at random times would give
    let confidence = grid_confidence(&raw, phase.0, period);
    if confidence < MIN_CONFIDENCE {
        return Beats { onsets, confidence, ..empty };
    }
    // a grid half a period away is as periodic: prefer the one the bass and kick fall on
    let low = smooth(&low);
    let grid_sum = |from: f64, e: &[f32]| {
        let (mut sum, mut t) = (0f32, from);
        while (t.round() as usize) < e.len() {
            sum += e.get(t.round() as usize).copied().unwrap_or(0.0);
            t += period;
        }
        sum
    };
    let half = (phase.0 + period / 2.0) % period;
    if grid_sum(half, &low) > 1.25 * grid_sum(phase.0, &low) {
        phase.0 = half;
    }
    // grid beats, each snapped to the strongest envelope value within ±12% of a period
    let reach = ((period * 0.12).round() as usize).max(1);
    let mut beats = Vec::new();
    let mut t = phase.0;
    while (t.round() as usize) < env.len() {
        let c = t.round() as usize;
        let (a, b) = (c.saturating_sub(reach), (c + reach + 1).min(env.len()));
        let mut at = c;
        let mut v = env.get(c).copied().unwrap_or(0.0);
        for i in a..b {
            let x = env.get(i).copied().unwrap_or(0.0);
            if x > v * 1.5 {
                (at, v) = (i, x);
            }
        }
        beats.push(at as f64 * hop_s + latency_s(sample_rate));
        t += period;
    }
    // downbeats: the phase (0..4) of every-fourth-beat with the most strength
    let strength = |s: f64| env.get(((s - latency_s(sample_rate)) / hop_s).round().max(0.0) as usize).copied().unwrap_or(0.0);
    let mut db = (0usize, f32::MIN);
    for k in 0..4 {
        let sum: f32 = beats.iter().skip(k).step_by(4).map(|b| strength(*b)).sum();
        if sum > db.1 {
            db = (k, sum);
        }
    }
    let downbeats = beats.iter().skip(db.0).step_by(4).copied().collect();
    let bpm = 60.0 / (period * hop_s);
    Beats { bpm: (bpm * 100.0).round() / 100.0, beats, downbeats, onsets, confidence }
}

/// How much more of `env`'s strength lies near the grid (`phase` + k·`period`/2, in hops) than
/// chance: 0 = no more than random onsets, 1 = all of it.
fn grid_confidence(env: &[f32], phase: f64, period: f64) -> f32 {
    let step = period / 2.0;
    if step <= 0.0 {
        return 0.0;
    }
    let reach = (period * 0.06).max(2.0);
    let total: f32 = env.iter().sum();
    let near: f32 = env
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            let d = (*i as f64 - phase).rem_euclid(step);
            d.min(step - d) <= reach
        })
        .map(|(_, v)| *v)
        .sum();
    let chance = ((2.0 * reach + 1.0) / step).min(1.0) as f32;
    if total <= 0.0 || chance >= 1.0 {
        return 0.0;
    }
    (((near / total) - chance) / (1.0 - chance)).clamp(0.0, 1.0)
}

/// Binomial blur (radius 3 hops).
fn smooth(env: &[f32]) -> Vec<f32> {
    const K: [f32; 7] = [1.0, 6.0, 15.0, 20.0, 15.0, 6.0, 1.0];
    (0..env.len())
        .map(|i| {
            let (mut sum, mut wsum) = (0f32, 0f32);
            for (k, w) in K.iter().enumerate() {
                if let Some(v) = (i + k).checked_sub(3).and_then(|j| env.get(j)) {
                    sum += v * w;
                    wsum += w;
                }
            }
            if wsum > 0.0 { sum / wsum } else { 0.0 }
        })
        .collect()
}

/// The flux of a hop measures the window that starts there; an onset sits about half a window
/// later.
fn latency_s(sample_rate: u32) -> f64 {
    (WIN as f64 / 2.0) / sample_rate.max(1) as f64
}

/// Local maxima of the envelope above an adaptive threshold, at least 50 ms apart.
fn peaks(env: &[f32], hop_s: f64) -> Vec<(f64, f32)> {
    let min_gap = ((0.05 / hop_s).ceil() as usize).max(1);
    let mut out: Vec<(usize, f32)> = Vec::new();
    for (j, w) in env.windows(3).enumerate() {
        let (i, p, v, n) = (j + 1, w[0], w[1], w[2]);
        if v < 0.15 || v < p || v < n {
            continue;
        }
        match out.last_mut() {
            Some(last) if i - last.0 < min_gap => {
                if v > last.1 {
                    *last = (i, v);
                }
            }
            _ => out.push((i, v)),
        }
    }
    let lat = (WIN as f64 / 2.0) * hop_s / HOP as f64;
    out.into_iter().map(|(i, v)| (i as f64 * hop_s + lat, v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click track: short decaying noise bursts every `60/bpm` s from `offset` s.
    fn clicks(bpm: f64, secs: f64, offset: f64, sr: u32) -> Vec<f32> {
        let n = (secs * sr as f64) as usize;
        let mut v = vec![0f32; n];
        let mut seed = 1u32;
        let period = 60.0 / bpm;
        let mut t = offset;
        while t < secs {
            let at = (t * sr as f64) as usize;
            for k in 0..(sr as usize / 50) {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 * 2.0 - 1.0;
                if let Some(x) = v.get_mut(at + k) {
                    *x += noise * (-(k as f32) / (sr as f32 / 400.0)).exp();
                }
            }
            t += period;
        }
        v
    }

    #[test]
    fn finds_tempo_and_beat_times_of_a_click_track() {
        let sr = 48_000;
        for bpm in [90.0, 120.0, 128.0, 150.0] {
            let b = analyze(&clicks(bpm, 20.0, 0.25, sr), sr, BeatOptions::default());
            assert!((b.bpm - bpm).abs() < 1.5, "expected {bpm}, got {}", b.bpm);
            // every detected beat lies within 30 ms of a true click
            let period = 60.0 / bpm;
            for t in &b.beats {
                let k = ((t - 0.25) / period).round();
                let err = (t - (0.25 + k * period)).abs();
                assert!(err < 0.03, "{bpm} BPM: beat at {t} is {err} s off the grid");
            }
            assert!(b.beats.len() as f64 > 20.0 * bpm / 60.0 * 0.8, "too few beats: {}", b.beats.len());
            assert!(!b.onsets.is_empty());
            assert!(b.downbeats.len() * 4 >= b.beats.len().saturating_sub(4));
        }
    }

    #[test]
    fn beats_fall_on_the_kick_not_the_louder_offbeat_hats() {
        let sr = 24_000;
        let (bpm, secs) = (120.0, 16.0);
        let period = 60.0 / bpm;
        // hats: loud noise bursts on the off-beats
        let mut v = clicks(bpm, secs, period / 2.0, sr);
        // kicks: a decaying 55 Hz thump on every beat, quieter than the hats
        for (i, x) in v.iter_mut().enumerate() {
            let t = i as f64 / sr as f64;
            let since = t % period;
            *x += (0.6 * (2.0 * std::f64::consts::PI * 55.0 * since).sin() * (-12.0 * since).exp()) as f32;
        }
        let b = analyze(&v, sr, BeatOptions::default());
        assert!((b.bpm - bpm).abs() < 1.5, "{}", b.bpm);
        for t in &b.beats {
            let off = (t / period).fract().min(1.0 - (t / period).fract()) * period;
            assert!(off < 0.03, "beat at {t} is {off} s from a kick (beats: {:?})", &b.beats[..4.min(b.beats.len())]);
        }
    }

    #[test]
    fn confidence_separates_steady_music_from_speech_and_noise() {
        let sr = 24_000;
        let steady = |name: &str, v: &[f32]| {
            let b = analyze(v, sr, BeatOptions::default());
            assert!(b.confidence > 0.8 && !b.beats.is_empty(), "{name}: confidence {}", b.confidence);
        };
        let none = |name: &str, v: &[f32]| {
            let b = analyze(v, sr, BeatOptions::default());
            assert!(b.confidence < 0.2 && b.beats.is_empty() && b.bpm == 0.0, "{name}: {} BPM, confidence {}", b.bpm, b.confidence);
        };
        for bpm in [90.0, 120.0, 150.0] {
            steady("click track", &clicks(bpm, 16.0, 0.2, sr));
        }
        let mut v = vec![0f32; sr as usize * 12];
        for t in [0.3, 1.45, 2.05, 3.9, 4.25, 5.95, 7.6, 8.15, 9.75, 10.45, 11.6] {
            let c = clicks(60.0, 0.05, 0.0, sr);
            for (k, x) in c.iter().enumerate() {
                if let Some(y) = v.get_mut((t * sr as f64) as usize + k) {
                    *y += x;
                }
            }
        }
        none("irregular hits", &v);
        // eighth-note hats with a kick on every beat
        let mut m = clicks(240.0, 16.0, 0.0, sr);
        m.iter_mut().for_each(|x| *x *= 0.4);
        for (i, x) in m.iter_mut().enumerate() {
            let t = i as f64 / sr as f64;
            let since = t % 0.5;
            *x += (0.6 * (2.0 * std::f64::consts::PI * 55.0 * since).sin() * (-12.0 * since).exp()) as f32;
        }
        steady("groove", &m);
        let mut seed = 7u32;
        let noise: Vec<f32> = (0..sr as usize * 10)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.3
            })
            .collect();
        none("noise", &noise);
        // speech-like: noise bursts with random lengths and gaps
        let mut sp = vec![0f32; sr as usize * 12];
        let mut t = 0.1f64;
        let mut s2 = 3u32;
        while t < 11.5 {
            s2 = s2.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let len = 0.08 + (s2 >> 24) as f64 / 255.0 * 0.3;
            s2 = s2.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let gap = 0.05 + (s2 >> 24) as f64 / 255.0 * 0.4;
            for i in (t * sr as f64) as usize..((t + len) * sr as f64) as usize {
                s2 = s2.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                if let Some(y) = sp.get_mut(i) {
                    *y = ((s2 >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.5;
                }
            }
            t += len + gap;
        }
        none("speech-like bursts", &sp);
    }

    #[test]
    fn irregular_hits_and_steady_tones_have_no_beat() {
        let sr = 24_000;
        let mut v = vec![0f32; sr as usize * 12];
        // hits at irregular times
        for t in [0.3, 1.45, 2.05, 3.9, 4.25, 5.95, 7.6, 8.15, 9.75, 10.45, 11.6] {
            let c = clicks(60.0, 0.05, 0.0, sr);
            for (k, x) in c.iter().enumerate() {
                if let Some(y) = v.get_mut((t * sr as f64) as usize + k) {
                    *y += x;
                }
            }
        }
        let b = analyze(&v, sr, BeatOptions::default());
        assert!(b.beats.is_empty() && b.bpm == 0.0, "no steady beat: {} BPM, confidence {}", b.bpm, b.confidence);
        assert!(b.onsets.len() >= 8, "the hits are still onsets: {}", b.onsets.len());
        let one = clicks(10.0, 6.0, 1.0, sr);
        assert!(analyze(&one[..sr as usize * 5], sr, BeatOptions::default()).beats.is_empty(), "one hit is not a beat");
        let quiet: Vec<f32> = clicks(120.0, 12.0, 0.1, sr).iter().map(|x| x * 0.01).collect();
        assert!((analyze(&quiet, sr, BeatOptions::default()).bpm - 120.0).abs() < 1.5, "quiet music (-40 dB) still has a beat");
        let tone: Vec<f32> = (0..sr as usize * 6).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / sr as f32).sin() * 0.3).collect();
        assert!(analyze(&tone, sr, BeatOptions::default()).beats.is_empty());
    }

    #[test]
    fn silence_short_and_hostile_input_give_no_beats() {
        assert_eq!(analyze(&[], 48_000, BeatOptions::default()).beats.len(), 0);
        assert_eq!(analyze(&vec![0.0; 48_000 * 3], 48_000, BeatOptions::default()).beats.len(), 0);
        assert_eq!(analyze(&[1.0; 100], 48_000, BeatOptions::default()).bpm, 0.0);
        assert_eq!(analyze(&clicks(120.0, 4.0, 0.0, 48_000), 0, BeatOptions::default()).bpm, 0.0);
        let mut nan = clicks(120.0, 4.0, 0.0, 48_000);
        nan.iter_mut().step_by(7).for_each(|x| *x = f32::NAN);
        let _ = analyze(&nan, 48_000, BeatOptions { min_bpm: 500.0, max_bpm: -3.0 });
    }
}
