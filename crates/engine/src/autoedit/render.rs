//! `qc.render`: quality control of the exported file itself, not the timeline.
//!
//! The file is opened and decoded with FilmCraft's own decoders, then checked:
//!
//! - **delivery**: it opens, has picture and sound, and matches the frame size, rate and duration;
//! - **picture**: sampled frames match the edit as FilmCraft renders it (PSNR), so the checks
//!   below, which locate overlays by comparing against renders without them, apply to the file;
//! - **overlays**: every caption and on-screen text, found in the decoded frame as the pixels that
//!   differ from a render without it: inside the frame (not cut off), inside the platform safe
//!   area, captions in at most the allowed number of lines (measured in pixels and by the
//!   renderer's layout);
//! - **readability**: caption reading speed, time on screen, overlaps; caption text equals the
//!   approved script;
//! - **sound**: integrated loudness and true peak of the decoded audio (BS.1770);
//! - **script fidelity**: the file's audio is transcribed and every script line must be spoken,
//!   once, in order, without stumbles or unscripted speech.
//!
//! Hard failures make the state `FAILED`; only a file that passes every hard check is `COMPLETE`.

use filmcraft_media::FrameRequest;
use filmcraft_project::{ItemKind, TrackKind};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use super::align::{self, TWord};
use super::assemble::{self, Source};
use crate::commands::{bad, bool_p, f64_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// One check result.
#[derive(Clone, Debug)]
pub struct Check {
    pub check: &'static str,
    pub pass: bool,
    pub hard: bool,
    pub detail: String,
    pub evidence: Value,
}

impl Check {
    fn new(check: &'static str, pass: bool, hard: bool, detail: impl Into<String>, evidence: Value) -> Self {
        Self { check, pass, hard, detail: detail.into(), evidence }
    }
    pub fn json(&self) -> Value {
        let status = if self.pass {
            "pass"
        } else if self.hard {
            "fail"
        } else {
            "warn"
        };
        json!({"check": self.check, "status": status, "severity": if self.hard { "hard" } else { "soft" }, "detail": self.detail, "evidence": self.evidence})
    }
}

/// Safe area as fractions of the frame kept clear at each edge: (top, bottom, left, right).
pub fn safe_area(v: Option<&Value>) -> (f64, f64, f64, f64) {
    match v {
        Some(Value::String(s)) if s == "none" => (0.0, 0.0, 0.0, 0.0),
        // Instagram Reels / TikTok: the top bar and the caption, buttons and profile row at the bottom
        Some(Value::String(s)) if s == "tiktok" => (0.10, 0.20, 0.06, 0.12),
        Some(Value::Object(o)) => {
            let g = |k: &str| o.get(k).and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(0.0).clamp(0.0, 0.45);
            (g("top"), g("bottom"), g("left"), g("right"))
        }
        _ => (0.14, 0.20, 0.06, 0.06),
    }
}

/// Peak signal-to-noise ratio (dB) of two RGBA8 images, sampling `b` onto `a`'s grid.
pub fn psnr(a: &[u8], aw: usize, ah: usize, b: &[u8], bw: usize, bh: usize) -> f64 {
    if aw == 0 || ah == 0 || bw == 0 || bh == 0 || a.len() < aw * ah * 4 || b.len() < bw * bh * 4 {
        return 0.0;
    }
    let mut se = 0f64;
    let mut n = 0f64;
    for y in 0..ah {
        let by = (y * bh / ah).min(bh - 1);
        for x in 0..aw {
            let bx = (x * bw / aw).min(bw - 1);
            for c in 0..3 {
                let (p, q) = (a.get((y * aw + x) * 4 + c).copied().unwrap_or(0), b.get((by * bw + bx) * 4 + c).copied().unwrap_or(0));
                let d = p as f64 - q as f64;
                se += d * d;
                n += 1.0;
            }
        }
    }
    let mse = se / n.max(1.0);
    if mse <= 1e-9 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() }
}

/// Bounding box `(x0, y0, x1, y1)` (inclusive) of where `a` and `b` differ clearly: pixels whose
/// largest channel difference exceeds `threshold`, keeping only rows and columns with at least
/// `min_run` such pixels (codec noise is scattered; text is not).
pub fn diff_box(a: &[u8], b: &[u8], w: usize, h: usize, threshold: u8, min_run: usize) -> Option<(usize, usize, usize, usize)> {
    if w == 0 || h == 0 || a.len() < w * h * 4 || b.len() < w * h * 4 {
        return None;
    }
    let mut rows = vec![0usize; h];
    let mut cols = vec![0usize; w];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let d = (0..3).map(|c| a.get(i + c).copied().unwrap_or(0).abs_diff(b.get(i + c).copied().unwrap_or(0))).max().unwrap_or(0);
            if d > threshold {
                if let Some(r) = rows.get_mut(y) {
                    *r += 1;
                }
                if let Some(c) = cols.get_mut(x) {
                    *c += 1;
                }
            }
        }
    }
    let y0 = rows.iter().position(|r| *r >= min_run)?;
    let y1 = rows.iter().rposition(|r| *r >= min_run)?;
    let x0 = cols.iter().position(|c| *c >= min_run)?;
    let x1 = cols.iter().rposition(|c| *c >= min_run)?;
    Some((x0, y0, x1, y1))
}

/// Decode the file's frame that starts at sequence frame `t` (asked for at mid-frame, so a
/// timestamp rounding in the container cannot land on the neighbouring frame) as RGBA8.
fn file_frame_at(src: &filmcraft_media::SharedSource, t: Tick, fd: Tick, scale: f32) -> Option<(usize, usize, Vec<u8>)> {
    let f = src.video_frame(FrameRequest { time: t + Tick(fd.0 / 2), scale }).ok()?;
    Some((f.width as usize, f.height as usize, f.to_rgba8()))
}

/// PSNR of `a` against `b` inside the box `(x0, y0, x1, y1)` (both images `w`×`h`).
fn psnr_in(a: &[u8], b: &[u8], w: usize, bx: (usize, usize, usize, usize)) -> f64 {
    let (x0, y0, x1, y1) = bx;
    let (mut se, mut n) = (0f64, 0f64);
    for y in y0..=y1 {
        for x in x0..=x1 {
            for c in 0..3 {
                let i = (y * w + x) * 4 + c;
                let d = a.get(i).copied().unwrap_or(0) as f64 - b.get(i).copied().unwrap_or(0) as f64;
                se += d * d;
                n += 1.0;
            }
        }
    }
    let mse = se / n.max(1.0);
    if mse <= 1e-9 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() }
}

/// Where a graphic clip's layers are laid out at `t`, in sequence pixels, including any part that
/// falls outside the frame (drawn at quarter scale onto a canvas three frames wide and high).
fn graphic_layout_box(it: &filmcraft_project::TrackItem, t: Tick, w: usize, h: usize) -> Option<(f64, f64, f64, f64)> {
    let k = 0.25;
    let (cw, ch) = (((w as f64) * k).ceil() as usize, ((h as f64) * k).ceil() as usize);
    let mut img = filmcraft_render::Image::new(cw * 3, ch * 3);
    let m = filmcraft_geom::Affine::translate(cw as f64, ch as f64).then_apply(&filmcraft_geom::Affine::scale(k, k));
    filmcraft_render::graphic_clip::render_graphic(it, it.source_time_at(t), (w as u32, h as u32), &m, &mut img);
    let (iw, ih) = (img.w, img.h);
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..ih {
        for x in 0..iw {
            if img.px.get((y * iw + x) * 4 + 3).is_some_and(|a| *a > 0.05) {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    (x0 <= x1 && y0 <= y1)
        .then(|| ((x0 as f64 - cw as f64) / k, (y0 as f64 - ch as f64) / k, (x1 as f64 + 1.0 - cw as f64) / k, (y1 as f64 + 1.0 - ch as f64) / k))
}

/// Render the active sequence at `t`, optionally without captions and without some clips.
fn render(s: &Session, t: Tick, captions: bool, hide: &[filmcraft_project::ClipId], scale: f32) -> Option<(usize, usize, Vec<u8>)> {
    let seq = s.state.active_sequence?;
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let project = if hide.is_empty() {
        s.project.clone()
    } else {
        let mut p = (*s.project).clone();
        if let Some(q) = p.sequence_mut(seq) {
            for tr in q.video_tracks.iter_mut() {
                for it in tr.items.iter_mut().filter(|i| hide.contains(&i.id)) {
                    it.enabled = false;
                }
            }
        }
        std::sync::Arc::new(p)
    };
    let opts = filmcraft_render::RenderOptions { scale, captions, ..Default::default() };
    let img = filmcraft_render::render_sequence(&project, seq, t, opts, &provider);
    Some((img.w, img.h, img.over_black_rgba8()))
}

/// Nearest-neighbour resize of RGBA8.
fn resize(px: &[u8], w: usize, h: usize, nw: usize, nh: usize) -> Vec<u8> {
    if (w, h) == (nw, nh) {
        return px.to_vec();
    }
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        let sy = (y * h / nh.max(1)).min(h.saturating_sub(1));
        for x in 0..nw {
            let sx = (x * w / nw.max(1)).min(w.saturating_sub(1));
            let (i, j) = ((y * nw + x) * 4, (sy * w + sx) * 4);
            if let (Some(d), Some(sp)) = (out.get_mut(i..i + 4), px.get(j..j + 4)) {
                d.copy_from_slice(sp);
            }
        }
    }
    out
}

/// One overlay to locate: a caption cue or a graphic clip.
struct Overlay {
    kind: &'static str,
    id: u64,
    label: String,
    t: Tick,
    /// For captions: lines by the renderer's layout, and the line height in pixels.
    layout_lines: Option<usize>,
    line_height: f32,
    hide: Vec<filmcraft_project::ClipId>,
}

const MAX_OVERLAYS: usize = 80;

/// `qc.render`.
pub fn render_qc(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "qc.render";
    let path = str_p(p, "path").ok_or_else(|| bad(cmd, "need `path`: the exported file to check"))?.to_string();
    let spec = p.get("spec").cloned().unwrap_or(json!({}));
    // reading speed: preferred target (soft) and hard limit; a looser limit needs an approval
    // record. Checked before anything else, so a refused relaxation is never mistaken for a result
    let policy = super::readability::policy(spec.get("captions")).map_err(|e| bad(cmd, e))?;
    let mut checks: Vec<Check> = Vec::new();
    let q = s.active_sequence().ok_or_else(|| bad(cmd, "open the sequence that was exported"))?.clone();
    let rate = q.settings.frame_rate;
    let fd = rate.frame_duration();
    let src = match s.media.open_file(&path, &*s.services) {
        Ok(x) => x,
        Err(e) => {
            checks.push(Check::new("playable", false, true, format!("the file does not open: {e}"), json!({"path": path})));
            return finish(s, &path, checks, json!({}));
        }
    };
    let info = src.info().clone();
    let mut measures = json!({"path": path, "container": info.container, "durationSeconds": (info.duration.seconds() * 1000.0).round() / 1000.0});
    // ---- delivery
    let Some(v) = info.video.clone() else {
        checks.push(Check::new("playable", false, true, "the file has no picture", Value::Null));
        return finish(s, &path, checks, measures);
    };
    measures["width"] = json!(v.width);
    measures["height"] = json!(v.height);
    measures["fps"] = json!((v.frame_rate.as_f64() * 1000.0).round() / 1000.0);
    let last = info.duration - v.frame_rate.frame_duration();
    let decodes = [Tick::ZERO, Tick(info.duration.0 / 2), last.max(Tick::ZERO)].iter().all(|t| src.video_frame(FrameRequest { time: *t, scale: 0.25 }).is_ok());
    checks.push(Check::new("playable", decodes, true, if decodes { "first, middle and last frames decode" } else { "frames fail to decode" }, Value::Null));
    if let Some(w) = spec.get("width").and_then(Value::as_u64) {
        checks.push(Check::new("frameSize", v.width as u64 == w, true, format!("{}×{} (spec width {w})", v.width, v.height), Value::Null));
    }
    if let Some(h) = spec.get("height").and_then(Value::as_u64) {
        checks.push(Check::new("frameSize", v.height as u64 == h, true, format!("{}×{} (spec height {h})", v.width, v.height), Value::Null));
    }
    if let Some(f) = spec.get("fps").and_then(Value::as_f64) {
        let got = v.frame_rate.as_f64();
        checks.push(Check::new("frameRate", (got - f).abs() < 0.01, true, format!("{got:.3} fps (spec {f})"), Value::Null));
    }
    let dur = info.duration.seconds();
    if let Some(d) = spec.get("duration") {
        let (mn, mx) = (d.get("min").and_then(Value::as_f64), d.get("max").and_then(Value::as_f64));
        let ok = mn.is_none_or(|m| dur >= m - 0.02) && mx.is_none_or(|m| dur <= m + 0.02);
        checks.push(Check::new("duration", ok, true, format!("{dur:.2} s (spec {:?}–{:?} s)", mn, mx), Value::Null));
    }
    let edit_len = q.duration().seconds();
    checks.push(Check::new(
        "duration",
        (dur - edit_len).abs() <= 2.0 * fd.seconds() + 0.05,
        true,
        format!("file {dur:.3} s, edit {edit_len:.3} s"),
        Value::Null,
    ));
    let has_audio = info.has_audio();
    checks.push(Check::new("audioPresent", has_audio, true, if has_audio { "the file has sound" } else { "the file has no sound" }, Value::Null));

    // ---- the file matches the edit
    let n = u64_p(p, "samples").unwrap_or(6).clamp(2, 40) as i64;
    let mut worst = f64::INFINITY;
    let mut per_sample = Vec::new();
    for k in 0..n {
        let t = rate.snap(Tick((last.0.max(0) as i128 * (2 * k + 1) as i128 / (2 * n) as i128) as i64));
        // full resolution: fine detail downscaled two different ways would not compare
        let (Some((fw, fh, fpx)), Some((rw, rh, rpx))) = (file_frame_at(&src, t, fd, 1.0), render(s, t, true, &[], 1.0)) else {
            continue;
        };
        let v = psnr(&rpx, rw, rh, &fpx, fw, fh);
        per_sample.push(json!({"seconds": (t.seconds() * 1000.0).round() / 1000.0, "psnrDb": (v * 10.0).round() / 10.0}));
        worst = worst.min(v);
    }
    measures["psnrSamples"] = json!(per_sample);
    let min_psnr = f64_p(p, "minPsnr").unwrap_or(30.0);
    measures["minPsnrDb"] = json!((worst * 10.0).round() / 10.0);
    checks.push(Check::new(
        "matchesEdit",
        worst.is_finite() && worst >= min_psnr,
        true,
        format!("sampled frames match the edit at {worst:.1} dB PSNR or better (need {min_psnr} dB)"),
        Value::Null,
    ));

    // ---- overlays: captions and on-screen text
    let (sw, sh) = (q.settings.width as usize, q.settings.height as usize);
    let (top, bottom, left, right) = safe_area(spec.get("safeArea"));
    let caps = spec.get("captions").cloned().unwrap_or(json!({}));
    let max_lines = caps.get("maxLines").and_then(Value::as_u64).unwrap_or(2) as usize;
    let max_cps = policy.max_cps;
    measures["readability"] = policy.json();
    let min_secs = caps.get("minSeconds").and_then(Value::as_f64).unwrap_or(0.8);
    let (mut overlays, mut readable) = gather_overlays(s, &q, sw, sh, (policy.target_cps, max_cps), min_secs);
    checks.append(&mut readable);
    overlays.truncate(MAX_OVERLAYS);
    let mut located = Vec::new();
    for o in &overlays {
        // the overlay's exact pixels: the edit rendered with it and without it (half size)
        let with = render(s, o.t, true, &[], 0.5);
        let without = if o.kind == "caption" { render(s, o.t, false, &[], 0.5) } else { render(s, o.t, true, &o.hide, 0.5) };
        let (Some((rw, rh, wpx)), Some((_, _, npx))) = (with, without) else { continue };
        let Some((x0, y0, x1, y1)) = diff_box(&wpx, &npx, rw, rh, 24, 2) else {
            checks.push(Check::new("overlayVisible", false, true, format!("{} \"{}\" draws nothing at {:.2} s", o.kind, o.label, o.t.seconds()), Value::Null));
            continue;
        };
        // …and it is in the file: the file matches the render with it better than without it
        let present = file_frame_at(&src, o.t, fd, 0.5).map(|(fw, fh, fpx)| {
            let fpx = resize(&fpx, fw, fh, rw, rh);
            (psnr_in(&fpx, &wpx, rw, (x0, y0, x1, y1)), psnr_in(&fpx, &npx, rw, (x0, y0, x1, y1)))
        });
        let in_file = present.is_some_and(|(a, b)| a >= b + 3.0);
        checks.push(Check::new(
            "overlayInFile",
            in_file,
            true,
            format!("{} \"{}\" {} in the exported file", o.kind, o.label, if in_file { "is" } else { "is not" }),
            json!({"psnrWithDb": present.map(|p| (p.0 * 10.0).round() / 10.0), "psnrWithoutDb": present.map(|p| (p.1 * 10.0).round() / 10.0)}),
        ));
        // geometry in sequence pixels: drawn pixels for captions; the full laid-out text (also the
        // part outside the frame, which draws nothing) for graphics
        let sx = sw as f64 / rw.max(1) as f64;
        let sy = sh as f64 / rh.max(1) as f64;
        let drawn = (x0 as f64 * sx, y0 as f64 * sy, (x1 + 1) as f64 * sx, (y1 + 1) as f64 * sy);
        let laid_out = match o.kind {
            "graphic" => q
                .video_tracks
                .iter()
                .flat_map(|t| t.items.iter())
                .find(|i| i.id.0 == o.id)
                .and_then(|it| graphic_layout_box(it, o.t, sw, sh))
                .map(|b| (b.0.min(drawn.0), b.1.min(drawn.1), b.2.max(drawn.2), b.3.max(drawn.3)))
                .unwrap_or(drawn),
            _ => drawn,
        };
        let (bx0, by0, bx1, by1) = laid_out;
        let (fwf, fhf) = (sw as f64, sh as f64);
        let bbox = json!({"x0": bx0.round(), "y0": by0.round(), "x1": bx1.round(), "y1": by1.round(), "frame": [sw, sh]});
        let edge = bx0 < 0.5 || by0 < 0.5 || bx1 > fwf - 0.5 || by1 > fhf - 0.5;
        checks.push(Check::new(
            "insideFrame",
            !edge,
            true,
            if edge { format!("{} \"{}\" runs off the frame edge", o.kind, o.label) } else { format!("{} \"{}\" is inside the frame", o.kind, o.label) },
            bbox.clone(),
        ));
        let safe = bx0 >= left * fwf && bx1 <= fwf * (1.0 - right) && by0 >= top * fhf && by1 <= fhf * (1.0 - bottom);
        checks.push(Check::new(
            "safeArea",
            safe,
            true,
            if safe {
                format!("{} \"{}\" is inside the safe area", o.kind, o.label)
            } else {
                format!("{} \"{}\" extends outside the safe area", o.kind, o.label)
            },
            json!({"box": bbox, "safeArea": {"top": top, "bottom": bottom, "left": left, "right": right}}),
        ));
        if let Some(layout) = o.layout_lines {
            let lh = o.line_height as f64;
            let measured = if lh > 0.0 { ((by1 - by0) / lh).floor().max(1.0) as usize } else { 0 };
            let lines = layout.max(measured);
            checks.push(Check::new(
                "captionLines",
                lines <= max_lines,
                true,
                format!("caption \"{}\" is drawn in {lines} line(s) (max {max_lines}; layout {layout}, measured {measured})", o.label),
                bbox.clone(),
            ));
        }
        located.push(json!({"kind": o.kind, "id": o.id, "label": o.label, "seconds": o.t.seconds(), "box": bbox}));
    }
    measures["overlays"] = json!(located);

    // ---- caption text is the script
    let script = str_p(p, "script").map(str::to_string);
    let lines = script.as_deref().map(|x| super::script::parse(x).0).unwrap_or_default();
    if script.is_some() {
        checks.extend(caption_text_check(&q, &lines));
    }

    // ---- sound
    if has_audio {
        let (mut c, i, tp) = sound_checks(&src, &spec);
        checks.append(&mut c);
        measures["integratedLufs"] = json!((i * 10.0).round() / 10.0);
        measures["truePeakDb"] = json!((tp * 10.0).round() / 10.0);
    }

    // ---- script fidelity: transcribe the file
    if !lines.is_empty() && bool_p(p, "transcribe").unwrap_or(true) && has_audio {
        match fidelity(s, p, &src, &lines) {
            Ok((mut c, m)) => {
                checks.append(&mut c);
                measures["fidelity"] = m;
            }
            Err(e) => checks.push(Check::new("scriptFidelity", false, true, format!("could not verify the spoken script: {e}"), Value::Null)),
        }
    }
    finish(s, &path, checks, measures)
}

/// The captions and graphic clips of the sequence to locate in the file, and the caption
/// readability checks (reading speed, time on screen, overlaps).
fn gather_overlays(s: &Session, q: &filmcraft_project::Sequence, sw: usize, sh: usize, cps: (f64, f64), min_secs: f64) -> (Vec<Overlay>, Vec<Check>) {
    let (target_cps, max_cps) = cps;
    let rate = q.settings.frame_rate;
    let mut checks = Vec::new();
    let mut overlays: Vec<Overlay> = Vec::new();
    for tr in q.caption_tracks.iter().filter(|t| t.enabled) {
        for c in &tr.captions {
            let layout = filmcraft_captions::burn::layout_lines(c, &tr.style, sw, sh).len();
            overlays.push(Overlay {
                kind: "caption",
                id: c.id.0,
                label: c.text.replace('\n', " / "),
                t: rate.snap(c.start + Tick(c.duration.0 / 2)),
                layout_lines: Some(layout),
                line_height: filmcraft_captions::burn::line_height(&tr.style, sh),
                hide: Vec::new(),
            });
            // readability
            let chars = c.text.chars().filter(|ch| *ch != '\n').count() as f64;
            let secs = c.duration.seconds().max(1e-3);
            if chars / secs > max_cps {
                checks.push(Check::new(
                    "captionReadingSpeed",
                    false,
                    true,
                    format!("\"{}\" reads at {:.1} characters/s (hard limit {max_cps})", c.text.replace('\n', " "), chars / secs),
                    json!({"caption": c.id.0, "cps": (chars / secs * 10.0).round() / 10.0}),
                ));
            } else if chars / secs > target_cps {
                checks.push(Check::new(
                    "captionReadingTarget",
                    false,
                    false,
                    format!("\"{}\" reads at {:.1} characters/s (preferred {target_cps}, within the {max_cps} limit)", c.text.replace('\n', " "), chars / secs),
                    json!({"caption": c.id.0, "cps": (chars / secs * 10.0).round() / 10.0}),
                ));
            }
            if secs < min_secs {
                checks.push(Check::new(
                    "captionDuration",
                    false,
                    false,
                    format!("\"{}\" is on screen {secs:.2} s (min {min_secs})", c.text.replace('\n', " ")),
                    json!({"caption": c.id.0}),
                ));
            }
        }
        let mut v: Vec<&filmcraft_project::Caption> = tr.captions.iter().collect();
        v.sort_by_key(|c| c.start);
        for w in v.windows(2) {
            if w[1].start < w[0].start + w[0].duration {
                checks.push(Check::new("captionOverlap", false, true, format!("captions overlap at {:.2} s", w[1].start.seconds()), Value::Null));
            }
        }
    }
    for tr in q.video_tracks.iter().filter(|t| t.enabled && t.kind == TrackKind::Video) {
        for it in tr.items.iter().filter(|i| i.enabled) {
            if matches!(s.project.item(it.item).map(|x| &x.kind), Some(ItemKind::Graphic { .. })) {
                overlays.push(Overlay {
                    kind: "graphic",
                    id: it.id.0,
                    label: it.name.clone(),
                    t: rate.snap(it.start + Tick(it.duration.0 / 2)),
                    layout_lines: None,
                    line_height: 0.0,
                    hide: vec![it.id],
                });
            }
        }
    }
    (overlays, checks)
}

/// The burned-in caption text, word for word, against the approved script.
fn caption_text_check(q: &filmcraft_project::Sequence, lines: &[super::script::ScriptLine]) -> Option<Check> {
    let cap_words: Vec<String> = q
        .caption_tracks
        .iter()
        .filter(|t| t.enabled)
        .flat_map(|t| {
            let mut v: Vec<&filmcraft_project::Caption> = t.captions.iter().collect();
            v.sort_by_key(|c| c.start);
            v.into_iter().flat_map(|c| c.text.split_whitespace().map(align::norm).collect::<Vec<_>>())
        })
        .filter(|w| !w.is_empty())
        .collect();
    let script_words: Vec<String> = lines.iter().flat_map(|l| l.text.split_whitespace().map(align::norm)).filter(|w| !w.is_empty()).collect();
    if cap_words.is_empty() {
        return None;
    }
    let same = cap_words == script_words;
    let first_diff = cap_words.iter().zip(&script_words).position(|(a, b)| a != b);
    Some(Check::new(
        "captionText",
        same,
        true,
        if same {
            "caption text is the approved script, word for word".to_string()
        } else {
            format!(
                "caption text differs from the script (first difference at word {}: \"{}\" vs \"{}\")",
                first_diff.unwrap_or(cap_words.len().min(script_words.len())),
                first_diff.and_then(|i| cap_words.get(i)).map(String::as_str).unwrap_or("—"),
                first_diff.and_then(|i| script_words.get(i)).map(String::as_str).unwrap_or("—")
            )
        },
        Value::Null,
    ))
}

/// Loudness and true peak of the decoded sound against the spec: (checks, LUFS, dBTP).
fn sound_checks(src: &filmcraft_media::SharedSource, spec: &Value) -> (Vec<Check>, f64, f64) {
    let mut checks = Vec::new();
    let sr = 48_000u32;
    let mut meter = filmcraft_audio_dsp::LoudnessMeter::new(sr as f64, 2);
    let total = src.info().duration.to_units_floor(sr as i64).max(0);
    let mut at = 0i64;
    let mut ok = true;
    while at < total {
        let n = (total - at).min(sr as i64 * 5) as usize;
        match src.audio(at, n, sr) {
            Ok(b) => {
                let l = b.channels.first().cloned().unwrap_or_default();
                let r = b.channels.get(1).cloned().unwrap_or_else(|| l.clone());
                meter.process(&[&l, &r]);
            }
            Err(_) => {
                ok = false;
                break;
            }
        }
        at += n as i64;
    }
    let (i, tp) = (meter.integrated(), meter.true_peak_dbtp());
    checks.push(Check::new("audioDecodes", ok, true, if ok { "the sound decodes" } else { "the sound fails to decode" }, Value::Null));
    let loud = spec.get("loudness").cloned().unwrap_or(json!({"integratedLufs": -14.0, "tolerance": 1.0, "truePeakMax": -1.0}));
    if let Some(target) = loud.get("integratedLufs").and_then(Value::as_f64) {
        let tol = loud.get("tolerance").and_then(Value::as_f64).unwrap_or(1.0);
        checks.push(Check::new("loudness", i.is_finite() && (i - target).abs() <= tol, true, format!("{i:.1} LUFS (target {target} ±{tol})"), Value::Null));
    }
    if let Some(mx) = loud.get("truePeakMax").and_then(Value::as_f64) {
        checks.push(Check::new("truePeak", tp <= mx, true, format!("{tp:.1} dBTP (max {mx})"), Value::Null));
    }
    (checks, i, tp)
}

/// Transcribe the file and check every script line is spoken once, in order, cleanly.
fn fidelity(s: &Session, p: &Value, src: &filmcraft_media::SharedSource, lines: &[super::script::ScriptLine]) -> Result<(Vec<Check>, Value)> {
    let t = crate::transcript::transcriber(s, p)?;
    let sr = filmcraft_speech::SAMPLE_RATE;
    let len = src.info().duration.to_units_floor(sr as i64).max(0) as usize;
    let buf = src.audio(0, len, sr).map_err(|e| EngineError::Other(e.to_string()))?;
    let audio = filmcraft_speech::downmix(&buf.channels);
    let opts = filmcraft_speech::Options { language: Some(str_p(p, "language").unwrap_or("en").to_string()), diarize: false, ..Default::default() };
    let tr = t.transcribe(&audio, &opts, &mut |_, _| true).map_err(|e| EngineError::Other(e.to_string()))?;
    let words: Vec<TWord> =
        tr.words.iter().map(|w| TWord { text: w.text.clone(), start: w.start.seconds(), end: w.end.seconds(), confidence: w.confidence }).collect();
    let source =
        Source { item: filmcraft_project::ItemId(0), name: "export".into(), words: words.clone(), audio: Some(audio), duration: src.info().duration.seconds() };
    let mut o = assemble::opts_from(p);
    o.min_score = f64_p(p, "minLineScore").unwrap_or(0.6).clamp(0.0, 1.0) as f32;
    let judged = assemble::analyze(lines, std::slice::from_ref(&source), &o);
    let mut checks = Vec::new();
    let mut prev_end = -1.0f64;
    let mut covered: Vec<(usize, usize)> = Vec::new();
    let mut per_line = Vec::new();
    for (l, takes) in lines.iter().zip(&judged) {
        // the spoken instance of the line in the file: the best take after the previous line
        let best = takes.iter().filter(|t| t.take.start >= prev_end - 0.05).max_by(|a, b| a.take.score.total_cmp(&b.take.score));
        match best {
            None => checks.push(Check::new(
                "scriptFidelity",
                false,
                true,
                format!("line {} is not spoken in the file (or not after line {}): \"{}\"", l.n, l.n.saturating_sub(1), l.text),
                Value::Null,
            )),
            Some(b) => {
                // content problems only: in a finished edit lines sit back to back over music, so
                // the raw-take cut checks (silence around the line) do not apply
                const CONTENT: &[&str] = &["hidden_speech", "repeated_audio", "repeated_words", "extra_words", "missing_words", "incomplete", "unresolved"];
                let hard: Vec<Value> = b.hard().filter(|f| CONTENT.contains(&f.kind)).map(assemble::flag_json).collect();
                checks.push(Check::new(
                    "scriptFidelity",
                    hard.is_empty(),
                    true,
                    if hard.is_empty() {
                        format!("line {} is spoken cleanly at {:.2} s", l.n, b.take.start)
                    } else {
                        format!("line {} at {:.2} s has problems", l.n, b.take.start)
                    },
                    json!({"line": l.n, "heard": b.take.text, "score": b.take.score, "problems": hard}),
                ));
                if takes.iter().filter(|t| t.take.score >= 0.8).count() > 1 {
                    checks.push(Check::new("scriptFidelity", false, true, format!("line {} is spoken more than once", l.n), Value::Null));
                }
                prev_end = b.take.end;
                covered.push((b.take.word_first, b.take.word_last));
                per_line.push(json!({"line": l.n, "start": b.take.start, "end": b.take.end, "heard": b.take.text}));
            }
        }
    }
    // speech that belongs to no line
    let extra: Vec<&TWord> = words
        .iter()
        .enumerate()
        .filter(|(i, w)| !covered.iter().any(|(a, b)| i >= a && i <= b) && !align::is_filler(&align::norm(&w.text)) && !align::norm(&w.text).is_empty())
        .map(|(_, w)| w)
        .collect();
    checks.push(Check::new(
        "unscriptedSpeech",
        extra.len() < 2,
        true,
        if extra.len() < 2 {
            "no speech outside the script".to_string()
        } else {
            format!("speech outside the script: \"{}\"", extra.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "))
        },
        Value::Null,
    ));
    Ok((checks, json!({"transcript": words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "), "lines": per_line, "model": t.id()})))
}

fn finish(s: &mut Session, path: &str, checks: Vec<Check>, measures: Value) -> Result<Value> {
    let failed: Vec<&Check> = checks.iter().filter(|c| !c.pass && c.hard).collect();
    let warnings = checks.iter().filter(|c| !c.pass && !c.hard).count();
    let state = if failed.is_empty() { "COMPLETE" } else { "FAILED" };
    let out = json!({
        "state": state,
        "path": path,
        "hardFailures": failed.len(),
        "warnings": warnings,
        "failures": failed.iter().map(|c| c.json()).collect::<Vec<_>>(),
        "checks": checks.iter().map(Check::json).collect::<Vec<_>>(),
        "measures": measures,
    });
    assemble::log(
        s,
        "render-qc",
        state,
        json!({"path": path, "hardFailures": failed.len(), "warnings": warnings,
        "failures": failed.iter().map(|c| c.json()).collect::<Vec<_>>(), "measures": out["measures"], "checks": checks.len()}),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let c = f(x, y);
                v.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        v
    }

    #[test]
    fn psnr_and_diff_boxes() {
        let a = img(40, 30, |x, y| [(x * 6) as u8, (y * 8) as u8, 90]);
        assert_eq!(psnr(&a, 40, 30, &a, 40, 30), 99.0);
        let noisy = img(40, 30, |x, y| [(x * 6) as u8 + ((x + y) % 3) as u8, (y * 8) as u8, 90]);
        assert!(psnr(&a, 40, 30, &noisy, 40, 30) > 40.0);
        // a "caption" block at x 10..20, y 20..25 plus scattered noise
        let b = img(40, 30, |x, y| {
            if (10..20).contains(&x) && (20..25).contains(&y) {
                [255, 255, 255]
            } else if (x * 7 + y) % 97 == 0 {
                [255, 0, 0]
            } else {
                [(x * 6) as u8, (y * 8) as u8, 90]
            }
        });
        assert_eq!(diff_box(&b, &a, 40, 30, 48, 3), Some((10, 20, 19, 24)));
        assert_eq!(diff_box(&a, &a, 40, 30, 48, 3), None);
        assert_eq!(diff_box(&a, &[], 40, 30, 48, 3), None);
        assert_eq!(psnr(&a, 0, 0, &a, 40, 30), 0.0);
        let r = resize(&a, 40, 30, 20, 15);
        assert_eq!(r.len(), 20 * 15 * 4);
    }

    #[test]
    fn safe_area_presets() {
        assert_eq!(safe_area(None), (0.14, 0.20, 0.06, 0.06));
        assert_eq!(safe_area(Some(&json!("none"))), (0.0, 0.0, 0.0, 0.0));
        assert_eq!(safe_area(Some(&json!({"top": 0.1, "bottom": 9}))), (0.1, 0.45, 0.0, 0.0));
    }
}
