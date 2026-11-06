//! The checks behind `qc.check`: an edit spec (what "done" means for this edit, as JSON) and the
//! rules that compare a measured timeline against it. Pure: the command measures the sequence
//! (cuts, captions, loudness, picture samples) into [`Measured`] and these functions judge it.
//!
//! Every spec field is optional. A missing field takes its default; `null` turns a check off.

use serde_json::{Value, json};

/// A video clip on an enabled video track, in sequence seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    pub track: String,
    pub clip: u64,
    pub name: String,
    pub start: f64,
    pub end: f64,
}

/// A caption cue, sequence seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub track: String,
    pub id: u64,
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// The level at an audio clip's edge (a hard cut there can click).
#[derive(Clone, Debug, PartialEq)]
pub struct AudioEdge {
    pub track: String,
    pub clip: u64,
    pub time: f64,
    /// Peak level within 5 ms of the edge, dBFS.
    pub peak_db: f64,
    /// Covered by a transition (crossfade or fade).
    pub faded: bool,
}

/// Picture measures at one side of a cut.
#[derive(Clone, Debug, PartialEq)]
pub struct CutLook {
    pub time: f64,
    /// Mean luma 0..1 and mean chroma (Cb, Cr around 0) just before and just after the cut.
    pub before: [f32; 3],
    pub after: [f32; 3],
}

/// Everything `qc.check` measured.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measured {
    pub fps: f64,
    pub duration: f64,
    pub shots: Vec<Shot>,
    pub cues: Vec<Cue>,
    pub integrated_lufs: Option<f64>,
    pub true_peak_db: Option<f64>,
    pub audio_edges: Vec<AudioEdge>,
    /// (sequence seconds, mean luma 0..1) of sampled program frames.
    pub luma_samples: Vec<(f64, f32)>,
    pub cut_looks: Vec<CutLook>,
}

/// One finding.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub check: &'static str,
    /// "error" (breaks the spec), "warning" (likely a problem), "info".
    pub severity: &'static str,
    pub time: Option<f64>,
    pub message: String,
    pub clip: Option<u64>,
}

impl Issue {
    fn new(check: &'static str, severity: &'static str, time: Option<f64>, message: impl Into<String>) -> Self {
        Self { check, severity, time, message: message.into(), clip: None }
    }
    fn on(mut self, clip: u64) -> Self {
        self.clip = Some(clip);
        self
    }
}

/// A spec section: absent → `Some(default)`, `null` → off, an object → that object.
fn section<'a>(spec: &'a Value, key: &str, default: &'a Value) -> Option<&'a Value> {
    match spec.get(key) {
        None => Some(default),
        Some(Value::Null) => None,
        Some(v) => Some(v),
    }
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64).filter(|x| x.is_finite())
}

fn fmt_t(t: f64) -> String {
    let t = t.max(0.0);
    format!("{}:{:05.2}", (t / 60.0).floor() as i64, t % 60.0)
}

/// Sorted cut points: every video clip edge strictly inside (0, duration), merged within half a
/// frame.
pub fn cut_points(m: &Measured) -> Vec<f64> {
    let half = 0.5 / m.fps.max(1.0);
    let mut v: Vec<f64> = m.shots.iter().flat_map(|s| [s.start, s.end]).filter(|t| t.is_finite() && *t > half && *t < m.duration - half).collect();
    v.sort_by(f64::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() < half);
    v
}

/// Uncovered stretches of [0, duration) (no enabled video clip): black in the export.
pub fn gaps(m: &Measured) -> Vec<(f64, f64)> {
    let mut iv: Vec<(f64, f64)> = m.shots.iter().map(|s| (s.start, s.end)).filter(|(a, b)| a.is_finite() && b.is_finite() && b > a).collect();
    iv.sort_by(|a, b| a.0.total_cmp(&b.0));
    let eps = 0.5 / m.fps.max(1.0);
    let mut out = Vec::new();
    let mut at = 0.0;
    for (a, b) in iv {
        if a > at + eps {
            out.push((at, a));
        }
        at = f64::max(at, b);
    }
    if m.duration > at + eps {
        out.push((at, m.duration));
    }
    out
}

struct Ctx<'a> {
    m: &'a Measured,
    spec: &'a Value,
    fps: f64,
    frame: f64,
    cuts: Vec<f64>,
    asl: f64,
}

/// Run every check of `spec` over `m`. Returns (issues, metrics).
pub fn check(m: &Measured, spec: &Value) -> (Vec<Issue>, Value) {
    let mut out = Vec::new();
    let fps = m.fps.max(1.0);
    let cuts = cut_points(m);
    let shots = cuts.len() + 1;
    let asl = if m.duration > 0.0 { m.duration / shots as f64 } else { 0.0 };
    let c = Ctx { m, spec, fps, frame: 1.0 / fps, cuts, asl };
    check_duration(&c, &mut out);
    check_shots(&c, &mut out);
    check_gaps(&c, &mut out);
    check_black(&c, &mut out);
    check_loudness(&c, &mut out);
    check_clicks(&c, &mut out);
    check_captions(&c, &mut out);
    let on_beat = check_beats(&c, &mut out);
    check_color(&c, &mut out);
    let cuts = &c.cuts;
    let metrics = json!({
        "durationSeconds": (m.duration * 1000.0).round() / 1000.0,
        "cuts": cuts.len(),
        "shots": shots,
        "averageShotSeconds": (asl * 1000.0).round() / 1000.0,
        "integratedLufs": m.integrated_lufs.map(|v| (v * 10.0).round() / 10.0),
        "truePeakDb": m.true_peak_db.map(|v| (v * 10.0).round() / 10.0),
        "captions": m.cues.len(),
        "cutsOnBeat": on_beat,
    });
    (out, metrics)
}

/// Total runtime against `duration`.
fn check_duration(c: &Ctx<'_>, out: &mut Vec<Issue>) {
    let (m, spec, frame) = (c.m, c.spec, c.frame);
    if let Some(d) = spec.get("duration").filter(|v| v.is_object()) {
        if let Some(min) = num(d, "min").filter(|min| m.duration < min - frame / 2.0) {
            out.push(Issue::new("duration", "error", None, format!("runs {:.2} s, shorter than the {min:.2} s minimum", m.duration)));
        }
        if let Some(max) = num(d, "max").filter(|max| m.duration > max + frame / 2.0) {
            out.push(Issue::new("duration", "error", None, format!("runs {:.2} s, longer than the {max:.2} s maximum", m.duration)));
        }
    }
}

/// Flash frames, overlong shots and average shot length.
fn check_shots(c: &Ctx<'_>, out: &mut Vec<Issue>) {
    let (m, spec, fps, frame, cuts, asl) = (c.m, c.spec, c.fps, c.frame, &c.cuts, c.asl);
    let min_frames = match spec.get("minShotFrames") {
        Some(Value::Null) => None,
        Some(v) => v.as_f64().filter(|x| x.is_finite()),
        None => Some(3.0),
    };
    if let Some(min_frames) = min_frames {
        for s in &m.shots {
            let frames = ((s.end - s.start) * fps).round();
            if frames < min_frames {
                out.push(
                    Issue::new("flashFrame", "error", Some(s.start), format!("{} on {} is {frames} frame(s) long (minimum {min_frames})", s.name, s.track))
                        .on(s.clip),
                );
            }
        }
    }
    if let Some(max) = spec.get("maxShotSeconds").and_then(Value::as_f64).filter(|x| x.is_finite() && *x > 0.0) {
        let mut bounds = vec![0.0];
        bounds.extend(cuts.iter().copied());
        bounds.push(m.duration);
        for w in bounds.windows(2) {
            if w[1] - w[0] > max + frame / 2.0 {
                out.push(Issue::new(
                    "shotLength",
                    "warning",
                    Some(w[0]),
                    format!("a {:.2} s shot from {} is longer than {max:.2} s", w[1] - w[0], fmt_t(w[0])),
                ));
            }
        }
    }
    if let Some(a) = spec.get("averageShotSeconds").filter(|v| v.is_object()) {
        if let Some(min) = num(a, "min").filter(|min| asl < *min) {
            out.push(Issue::new("pacing", "warning", None, format!("average shot {asl:.2} s is faster than the {min:.2} s minimum")));
        }
        if let Some(max) = num(a, "max").filter(|max| asl > *max) {
            out.push(Issue::new("pacing", "warning", None, format!("average shot {asl:.2} s is slower than the {max:.2} s maximum")));
        }
    }
}

/// Stretches with no enabled video clip (black in the export).
fn check_gaps(c: &Ctx<'_>, out: &mut Vec<Issue>) {
    let (m, spec) = (c.m, c.spec);
    if !spec.get("allowGaps").and_then(Value::as_bool).unwrap_or(false) {
        for (a, b) in gaps(m) {
            out.push(Issue::new("gap", "error", Some(a), format!("no video from {} to {} ({:.2} s of black)", fmt_t(a), fmt_t(b), b - a)));
        }
    }
}

/// Black program frames inside clips (fades left too long, black media).
fn check_black(c: &Ctx<'_>, out: &mut Vec<Issue>) {
    let (m, spec) = (c.m, c.spec);
    let black_default = json!({"maxLuma": 0.02});
    if let Some(b) = section(spec, "blackFrames", &black_default) {
        let max = num(b, "maxLuma").unwrap_or(0.02) as f32;
        let gap_list = gaps(m);
        let in_gap = |t: f64| gap_list.iter().any(|(a, b)| t >= *a && t < *b);
        let mut run: Option<(f64, f64)> = None;
        let mut runs = Vec::new();
        for (t, l) in &m.luma_samples {
            if *l <= max && !in_gap(*t) {
                run = Some(run.map_or((*t, *t), |(a, _)| (a, *t)));
            } else if let Some(r) = run.take() {
                runs.push(r);
            }
        }
        runs.extend(run);
        for (a, b) in runs {
            out.push(Issue::new("blackFrames", "warning", Some(a), format!("picture is black from {} to {}", fmt_t(a), fmt_t(b))));
        }
    }
}

/// Integrated loudness and true peak of the mix.
fn check_loudness(c: &Ctx<'_>, out: &mut Vec<Issue>) {
    let (m, spec) = (c.m, c.spec);
    let loud_default = json!({"integratedLufs": -14.0, "tolerance": 1.0, "truePeakMax": -1.0});
    if let Some(l) = section(spec, "loudness", &loud_default) {
        let target = num(l, "integratedLufs");
        let tol = num(l, "tolerance").unwrap_or(1.0).abs();
        match (m.integrated_lufs, target) {
            (Some(v), Some(t)) if (v - t).abs() > tol => {
                out.push(Issue::new("loudness", "error", None, format!("integrated loudness {v:.1} LUFS is off the {t:.1} ±{tol:.1} LUFS target")));
            }
            (None, Some(_)) => out.push(Issue::new("loudness", "warning", None, "the mix is silent")),
            _ => {}
        }
        if let (Some(tp), Some(max)) = (m.true_peak_db, num(l, "truePeakMax"))
            && tp > max
        {
            out.push(Issue::new("truePeak", "error", None, format!("true peak {tp:.1} dBTP is over the {max:.1} dBTP ceiling")));
        }
    }
}

/// Clicks at hard audio cuts.
fn check_clicks(c: &Ctx<'_>, out: &mut Vec<Issue>) {
    let (m, spec, frame) = (c.m, c.spec, c.frame);
    let click_default = json!({"thresholdDb": -30.0});
    if let Some(c) = section(spec, "audioClicks", &click_default) {
        let th = num(c, "thresholdDb").unwrap_or(-30.0);
        for e in m.audio_edges.iter().filter(|e| !e.faded && e.peak_db > th && e.time > frame / 2.0 && e.time < m.duration - frame / 2.0) {
            out.push(
                Issue::new(
                    "audioClick",
                    "warning",
                    Some(e.time),
                    format!("hard audio cut on {} at {} ({:.0} dBFS): add a short crossfade", e.track, fmt_t(e.time), e.peak_db),
                )
                .on(e.clip),
            );
        }
    }
}

/// Caption reading speed, time on screen, layout and overlaps.
fn check_captions(c: &Ctx<'_>, out: &mut Vec<Issue>) {
    let (m, spec) = (c.m, c.spec);
    let cap_default = json!({"maxCps": 17.0, "minSeconds": 0.8, "maxLineChars": 42, "maxLines": 2});
    if let Some(c) = section(spec, "captions", &cap_default) {
        let max_cps = num(c, "maxCps");
        let min_s = num(c, "minSeconds");
        let max_chars = num(c, "maxLineChars");
        let max_lines = num(c, "maxLines");
        let mut by_track: Vec<&Cue> = m.cues.iter().collect();
        by_track.sort_by(|a, b| a.track.cmp(&b.track).then(a.start.total_cmp(&b.start)));
        for (k, q) in by_track.iter().enumerate() {
            let dur = (q.end - q.start).max(0.0);
            let chars = q.text.chars().filter(|c| *c != '\n').count() as f64;
            let issue = |sev, msg: String| Issue::new("captions", sev, Some(q.start), msg).on(q.id);
            if let Some(cps) = max_cps.filter(|_| dur > 0.0).filter(|m| chars / dur > *m) {
                out.push(issue("error", format!("caption at {} reads at {:.1} characters/s (max {cps})", fmt_t(q.start), chars / dur.max(1e-6))));
            }
            if let Some(ms) = min_s.filter(|ms| dur < *ms) {
                out.push(issue("warning", format!("caption at {} is on screen {dur:.2} s (min {ms})", fmt_t(q.start))));
            }
            let lines: Vec<&str> = q.text.lines().collect();
            if let Some(ml) = max_lines.filter(|ml| lines.len() as f64 > *ml) {
                out.push(issue("error", format!("caption at {} has {} lines (max {ml})", fmt_t(q.start), lines.len())));
            }
            if let Some(mc) = max_chars
                && let Some(long) = lines.iter().map(|l| l.chars().count()).max().filter(|n| *n as f64 > mc)
            {
                out.push(issue("warning", format!("caption at {} has a {long}-character line (max {mc})", fmt_t(q.start))));
            }
            if let Some(next) = by_track.get(k + 1).filter(|n| n.track == q.track && n.start < q.end - 1e-6) {
                out.push(issue("error", format!("caption at {} overlaps the next one at {}", fmt_t(q.start), fmt_t(next.start))));
            }
        }
    }
}

/// Cuts on the beat (opt-in: needs the beat times, e.g. from `analyze.beats`). Returns the share of cuts on a beat.
fn check_beats(c: &Ctx<'_>, out: &mut Vec<Issue>) -> Option<f64> {
    let (spec, fps, frame, cuts) = (c.spec, c.fps, c.frame, &c.cuts);
    let mut on_beat = None;
    if let Some(b) = spec.get("beatSync").filter(|v| v.is_object()) {
        let beats: Vec<f64> =
            b.get("beats").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).filter(|x| x.is_finite()).collect()).unwrap_or_default();
        let tol = num(b, "toleranceFrames").unwrap_or(2.0).abs() * frame;
        let min_frac = num(b, "minFraction").unwrap_or(0.8);
        if !beats.is_empty() && !cuts.is_empty() {
            let mut hits = 0;
            for c in cuts {
                let d = beats.iter().map(|b| (b - c).abs()).fold(f64::INFINITY, f64::min);
                if d <= tol {
                    hits += 1;
                } else {
                    out.push(Issue::new(
                        "beatSync",
                        "info",
                        Some(*c),
                        format!("cut at {} is {:.0} frame(s) off the nearest beat", fmt_t(*c), (d * fps).round()),
                    ));
                }
            }
            let frac = hits as f64 / cuts.len() as f64;
            on_beat = Some(frac);
            if frac < min_frac {
                out.push(Issue::new("beatSync", "warning", None, format!("{:.0}% of cuts land on a beat (want {:.0}%)", frac * 100.0, min_frac * 100.0)));
            }
        }
    }
    on_beat
}

/// Colour jumps across cuts (opt-in: scene changes are often meant to jump).
fn check_color(c: &Ctx<'_>, out: &mut Vec<Issue>) {
    let (m, spec) = (c.m, c.spec);
    if let Some(c) = spec.get("colorContinuity").filter(|v| v.is_object()) {
        let max_l = num(c, "maxLumaDelta").unwrap_or(0.25) as f32;
        let max_c = num(c, "maxChromaDelta").unwrap_or(0.06) as f32;
        for k in &m.cut_looks {
            let dl = (k.before[0] - k.after[0]).abs();
            let dc = ((k.before[1] - k.after[1]).powi(2) + (k.before[2] - k.after[2]).powi(2)).sqrt();
            if dl > max_l || dc > max_c {
                out.push(Issue::new(
                    "colorContinuity",
                    "warning",
                    Some(k.time),
                    format!("cut at {} jumps in brightness by {:.0}% and in colour by {:.3}", fmt_t(k.time), dl * 100.0, dc),
                ));
            }
        }
    }
}

pub fn issue_json(i: &Issue) -> Value {
    json!({"check": i.check, "severity": i.severity, "seconds": i.time.map(|t| (t * 1000.0).round() / 1000.0), "message": i.message, "clip": i.clip})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(clip: u64, a: f64, b: f64) -> Shot {
        Shot { track: "V1".into(), clip, name: format!("c{clip}"), start: a, end: b }
    }

    fn base() -> Measured {
        Measured {
            fps: 24.0,
            duration: 10.0,
            shots: vec![shot(1, 0.0, 4.0), shot(2, 4.0, 4.0 + 2.0 / 24.0), shot(3, 5.0, 10.0)],
            cues: vec![
                Cue { track: "C1".into(), id: 9, start: 1.0, end: 1.5, text: "This caption is far too long to read in half a second".into() },
                Cue { track: "C1".into(), id: 10, start: 1.4, end: 3.0, text: "ok".into() },
            ],
            integrated_lufs: Some(-20.0),
            true_peak_db: Some(0.3),
            audio_edges: vec![
                AudioEdge { track: "A1".into(), clip: 5, time: 4.0, peak_db: -6.0, faded: false },
                AudioEdge { track: "A1".into(), clip: 6, time: 5.0, peak_db: -6.0, faded: true },
            ],
            luma_samples: vec![(0.5, 0.4), (6.0, 0.01), (7.0, 0.01), (8.0, 0.5)],
            cut_looks: vec![CutLook { time: 5.0, before: [0.8, 0.0, 0.0], after: [0.2, 0.1, 0.0] }],
        }
    }

    fn checks(issues: &[Issue]) -> Vec<&str> {
        issues.iter().map(|i| i.check).collect()
    }

    #[test]
    fn default_spec_catches_the_usual_mistakes() {
        let (issues, metrics) = check(&base(), &json!({}));
        let c = checks(&issues);
        for want in ["flashFrame", "gap", "blackFrames", "loudness", "truePeak", "audioClick", "captions"] {
            assert!(c.contains(&want), "missing {want}: {issues:#?}");
        }
        // the faded edge is fine; colour and beat checks are opt-in
        assert_eq!(issues.iter().filter(|i| i.check == "audioClick").count(), 1);
        assert!(!c.contains(&"colorContinuity") && !c.contains(&"beatSync"));
        assert_eq!(metrics["cuts"], 3);
        assert!(issues.iter().any(|i| i.check == "captions" && i.message.contains("overlaps")));
    }

    #[test]
    fn null_turns_checks_off_and_opt_ins_work() {
        let spec = json!({
            "minShotFrames": null, "allowGaps": true, "blackFrames": null, "loudness": null, "audioClicks": null, "captions": null,
            "duration": {"min": 12, "max": 30}, "maxShotSeconds": 3, "averageShotSeconds": {"max": 2},
            "beatSync": {"beats": [4.0, 5.02], "toleranceFrames": 1, "minFraction": 1.0},
            "colorContinuity": {"maxLumaDelta": 0.2}
        });
        let (issues, metrics) = check(&base(), &spec);
        let c = checks(&issues);
        for want in ["duration", "shotLength", "pacing", "beatSync", "colorContinuity"] {
            assert!(c.contains(&want), "missing {want}: {issues:#?}");
        }
        for off in ["flashFrame", "gap", "blackFrames", "loudness", "truePeak", "audioClick", "captions"] {
            assert!(!c.contains(&off), "{off} should be off: {issues:#?}");
        }
        assert!(metrics["cutsOnBeat"].as_f64().unwrap() < 1.0);
    }

    #[test]
    fn a_clean_edit_passes() {
        let m = Measured {
            fps: 25.0,
            duration: 6.0,
            shots: vec![shot(1, 0.0, 3.0), shot(2, 3.0, 6.0)],
            integrated_lufs: Some(-14.2),
            true_peak_db: Some(-1.5),
            luma_samples: vec![(1.0, 0.4), (4.0, 0.5)],
            ..Default::default()
        };
        let (issues, _) = check(&m, &json!({"duration": {"min": 5, "max": 7}}));
        assert!(issues.is_empty(), "{issues:#?}");
    }

    #[test]
    fn hostile_measures_and_specs() {
        let m = Measured { fps: 0.0, duration: f64::NAN, shots: vec![shot(1, f64::NAN, f64::INFINITY)], ..Default::default() };
        let _ = check(&m, &json!({"duration": "x", "maxShotSeconds": -1, "beatSync": {"beats": ["a", null]}, "captions": 3}));
        let _ = check(&Measured::default(), &json!(null));
        assert!(gaps(&Measured { fps: 24.0, duration: 1.0, ..Default::default() }).len() == 1);
    }
}
