//! Tools for agents that edit without a person watching (see `docs/autonomous-editing.md`):
//!
//! - `analyze.beats`: tempo, beats, downbeats and onsets of a clip's audio or of the sequence mix,
//!   optionally as markers, so cuts can land on the music.
//! - `analyze.takes`: sharpness, exposure, clipping and motion of sampled frames of clips or
//!   project items, with a 0..100 score per take, so the best take can be picked without looking.
//! - `script.align`: every spoken take of each script line in the project's transcripts, ranked
//!   (clean reads first; fillers, stumbles and partial reads lower).
//! - `qc.check`: the active sequence against an edit spec (duration, pacing, flash frames, gaps,
//!   black frames, loudness and true peak, clicks at hard audio cuts, caption reading speed and
//!   layout, cuts on the beat, colour jumps across cuts); `pass` is false when any error remains.
//! - `qc.filmstrip`: one PNG with frames around every cut (or given times), for visual review.
//!
//! All run synchronously and cap their work (samples, frames, words), so they return in seconds
//! on long media. They are agent commands (no menu).

pub mod align;
pub mod beats;
pub mod qc;
pub mod takes;

use filmcraft_media::FrameRequest;
use filmcraft_project::{ItemId, Label, Marker, MarkerId, MarkerKind, TrackKind};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, clips_p, f64_p, has_seq, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// Analysis rate for beat tracking (sources resample to it; the sequence mix is decimated).
const BEAT_RATE: u32 = 24_000;
/// Longest stretch `analyze.beats` reads.
const MAX_BEAT_SECONDS: f64 = 15.0 * 60.0;
/// Frames `analyze.takes` may decode in one call, over all takes.
const MAX_TAKE_SAMPLES: usize = 2_000;
/// Program frames `qc.check` may render for black-frame sampling.
const MAX_QC_SAMPLES: usize = 1_200;
/// Rows of one filmstrip.
const MAX_STRIP_ROWS: usize = 40;

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "analyze.beats",
            label: "Analyze Beats",
            menu: &[],
            shortcut: None,
            params: r#"{"item":id? (a project item; else the active sequence's mix),"start":seconds?,"end":seconds?,"minBpm":f=60,"maxBpm":f=200,"markers":bool=false (add Beat / Downbeat markers),"downbeatsOnly":bool=false}"#,
            enabled: always,
            run: analyze_beats,
            journal: true,
        },
        CommandSpec {
            id: "analyze.takes",
            label: "Analyze Takes",
            menu: &[],
            shortcut: None,
            params: r#"{"items":[id]? (project items, whole media),"clips":[id]? (timeline clips; default the selection),"every":seconds=0.5,"width":px=160,"detail":bool=false (per-sample measures)}"#,
            enabled: always,
            run: analyze_takes,
            journal: false,
        },
        CommandSpec {
            id: "script.align",
            label: "Align Script to Transcripts",
            menu: &[],
            shortcut: None,
            params: r#"{"script":str (one line per sentence or beat),"items":[id]? (default every transcribed item),"top":n=3,"minScore":0..1=0.5}"#,
            enabled: always,
            run: script_align,
            journal: false,
        },
        CommandSpec {
            id: "qc.check",
            label: "Check Sequence Against Spec",
            menu: &[],
            shortcut: None,
            params: r#"{"spec":{"duration":{"min":s,"max":s}?,"minShotFrames":n=3,"maxShotSeconds":s?,"averageShotSeconds":{"min":s,"max":s}?,"allowGaps":bool=false,"blackFrames":{"maxLuma":0.02},"loudness":{"integratedLufs":-14,"tolerance":1,"truePeakMax":-1},"audioClicks":{"thresholdDb":-30},"captions":{"maxCps":17,"minSeconds":0.8,"maxLineChars":42,"maxLines":2},"beatSync":{"beats":[s],"toleranceFrames":2,"minFraction":0.8}?,"colorContinuity":{"maxLumaDelta":0.25,"maxChromaDelta":0.06}?} (null turns a check off)?,"sampleEvery":seconds=1}"#,
            enabled: has_seq,
            run: qc_check,
            journal: false,
        },
        CommandSpec {
            id: "qc.filmstrip",
            label: "Cut Filmstrip",
            menu: &[],
            shortcut: None,
            params: r#"{"times":[seconds]? (default every cut),"offset":n=0,"limit":n=12,"tileWidth":px=240,"path":str? (write a PNG there; else returned as pngBase64)}"#,
            enabled: has_seq,
            run: qc_filmstrip,
            journal: false,
        },
    ]
}

fn secs(t: Tick) -> f64 {
    (t.seconds() * 1000.0).round() / 1000.0
}

fn ids_p(p: &Value, key: &str) -> Option<Vec<u64>> {
    p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect())
}

fn finite_p(p: &Value, key: &str) -> Option<f64> {
    f64_p(p, key).filter(|v| v.is_finite())
}

// ---------------------------------------------------------------------------------------- beats

fn mono(buf: &filmcraft_frame::AudioBuffer) -> Vec<f32> {
    let n = buf.frames();
    let ch = buf.channels.len().max(1) as f32;
    (0..n).map(|i| buf.channels.iter().filter_map(|c| c.get(i)).sum::<f32>() / ch).collect()
}

fn analyze_beats(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "analyze.beats";
    let opts = beats::BeatOptions { min_bpm: finite_p(p, "minBpm").unwrap_or(60.0), max_bpm: finite_p(p, "maxBpm").unwrap_or(200.0) };
    let start = finite_p(p, "start").unwrap_or(0.0).max(0.0);
    let item = crate::commands::item_p(p, "item");
    // read the audio: a project item's media, or the active sequence's mix
    let (samples, rate, length) = if let Some(item) = item {
        let src = s.source(item).ok_or_else(|| bad(cmd, format!("item {} has no media", item.0)))?;
        if !src.info().has_audio() {
            return Err(bad(cmd, format!("item {} has no audio", item.0)));
        }
        let length = src.info().duration.seconds();
        let end = finite_p(p, "end").unwrap_or(length).min(length).min(start + MAX_BEAT_SECONDS);
        if end <= start {
            return Err(bad(cmd, "`end` must be after `start`"));
        }
        let frames = ((end - start) * BEAT_RATE as f64) as usize;
        let mut out = Vec::with_capacity(frames);
        let mut at = (start * BEAT_RATE as f64) as i64;
        let stop = at + frames as i64;
        while at < stop {
            let n = (stop - at).min(BEAT_RATE as i64 * 10) as usize;
            let buf = src.audio(at, n, BEAT_RATE).map_err(|e| EngineError::Other(format!("{cmd}: {e}")))?;
            out.extend(mono(&buf));
            at += n as i64;
        }
        (out, BEAT_RATE, length)
    } else {
        let q = s.active_sequence().ok_or_else(|| bad(cmd, "give `item`, or open a sequence to analyse its mix"))?;
        let sr = q.settings.sample_rate.max(1);
        let length = q.duration().seconds();
        let end = finite_p(p, "end").unwrap_or(length).min(length).min(start + MAX_BEAT_SECONDS);
        if end <= start {
            return Err(bad(cmd, "the sequence has no audio in that range"));
        }
        let provider = s.media.provider(s.project.clone(), s.services.clone());
        let factor = (sr / BEAT_RATE).max(1) as usize;
        let (mut at, stop) = ((start * sr as f64) as i64, (end * sr as f64) as i64);
        let mut out = Vec::with_capacity(((stop - at).max(0) as usize) / factor);
        while at < stop {
            let n = (stop - at).min(sr as i64 * 10) as usize;
            let m = mono(&filmcraft_render::audio::mix_sequence(&s.project, q, at, n, &provider));
            out.extend(m.chunks(factor).map(|c| c.iter().sum::<f32>() / c.len().max(1) as f32));
            at += n as i64;
        }
        (out, sr / factor as u32, length)
    };
    let b = beats::analyze(&samples, rate, opts);
    let shift = |v: f64| ((v + start) * 1000.0).round() / 1000.0;
    let beat_times: Vec<f64> = b.beats.iter().map(|t| shift(*t)).filter(|t| *t <= length).collect();
    let down_times: Vec<f64> = b.downbeats.iter().map(|t| shift(*t)).filter(|t| *t <= length).collect();
    let mut added = 0usize;
    if bool_p(p, "markers").unwrap_or(false) && !beat_times.is_empty() {
        let only_down = bool_p(p, "downbeatsOnly").unwrap_or(false);
        let marks: Vec<(Tick, bool)> = beat_times
            .iter()
            .map(|t| (Tick::from_seconds_f64(*t), down_times.iter().any(|d| (d - t).abs() < 1e-6)))
            .filter(|(_, down)| !only_down || *down)
            .collect();
        added = add_markers(s, item, &marks)?;
    }
    Ok(json!({
        "bpm": b.bpm,
        "confidence": (b.confidence as f64 * 1000.0).round() / 1000.0,
        "beats": beat_times,
        "downbeats": down_times,
        "onsets": b.onsets.iter().map(|(t, v)| json!({"seconds": shift(*t), "strength": (*v as f64 * 100.0).round() / 100.0})).collect::<Vec<_>>(),
        "markersAdded": added,
        "source": item.map_or_else(|| json!("sequence"), |i| json!(i.0)),
    }))
}

/// Beat markers on a project item's media (media time) or on the active sequence.
fn add_markers(s: &mut Session, item: Option<ItemId>, marks: &[(Tick, bool)]) -> Result<usize> {
    let seq = s.state.active_sequence;
    let marks = marks.to_vec();
    s.edit("Add Beat Markers", move |pr, _| {
        let ids: Vec<u64> = marks.iter().map(|_| pr.alloc_id()).collect();
        let list = match item {
            Some(i) => pr.item_mut(i).and_then(|x| x.as_media_mut()).map(|m| &mut m.markers),
            None => seq.and_then(|q| pr.sequence_mut(q)).map(|q| &mut q.markers),
        };
        let Some(list) = list else { return Err(EngineError::Other("analyze.beats: nowhere to put markers".into())) };
        let mut n = 0;
        for ((t, down), id) in marks.iter().zip(ids) {
            if list.iter().any(|m| m.start == *t) {
                continue;
            }
            list.push(Marker {
                url: String::new(),
                author: String::new(),
                custom_color: None,
                id: MarkerId(id),
                start: *t,
                duration: Tick::ZERO,
                name: if *down { "Downbeat".into() } else { "Beat".into() },
                comment: String::new(),
                kind: MarkerKind::Comment,
                color: if *down { Label::Rose } else { Label::Yellow },
            });
            n += 1;
        }
        list.sort_by_key(|m| m.start);
        Ok(n)
    })
}

// ---------------------------------------------------------------------------------------- takes

struct TakeWork {
    name: String,
    item: ItemId,
    clip: Option<u64>,
    src: filmcraft_media::SharedSource,
    /// (media time of the sample, sequence time if on the timeline)
    times: Vec<(Tick, Option<Tick>)>,
}

fn analyze_takes(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "analyze.takes";
    let every = finite_p(p, "every").unwrap_or(0.5).clamp(1.0 / 60.0, 3600.0);
    let width = u64_p(p, "width").unwrap_or(160).clamp(16, 1920) as f64;
    let detail = bool_p(p, "detail").unwrap_or(false);
    let step = Tick::from_seconds_f64(every).max(Tick(1));
    let mut work: Vec<TakeWork> = Vec::new();
    if let Some(items) = ids_p(p, "items") {
        for id in items.into_iter().map(ItemId) {
            let Some(src) = s.source(id) else { return Err(bad(cmd, format!("item {} has no media", id.0))) };
            if !src.info().has_video() {
                return Err(bad(cmd, format!("item {} has no picture", id.0)));
            }
            let dur = src.info().duration;
            let n = (dur.0 / step.0).max(1);
            let name = s.project.item(id).map(|i| i.name.clone()).unwrap_or_default();
            work.push(TakeWork { name, item: id, clip: None, src, times: (0..n).map(|k| (Tick(step.0 * k), None)).collect() });
        }
    } else {
        let q = s.active_sequence().ok_or_else(|| bad(cmd, "give `items`, or `clips` in an open sequence"))?;
        let clips = clips_p(s, p);
        if clips.is_empty() {
            return Err(bad(cmd, "give `items` or `clips`, or select clips in the timeline"));
        }
        for c in clips {
            let Some((tid, it)) = q.find_item(c) else { return Err(bad(cmd, format!("no clip {} in the sequence", c.0))) };
            if q.track(tid).is_none_or(|t| t.kind != TrackKind::Video) {
                continue;
            }
            let Some(src) = s.source(it.item) else { continue };
            if !src.info().has_video() {
                continue;
            }
            let n = (it.duration.0 / step.0).max(1);
            let times = (0..n).map(|k| it.start + Tick(step.0 * k)).map(|t| (it.source_time_at(t), Some(t))).collect();
            work.push(TakeWork { name: it.name.clone(), item: it.item, clip: Some(c.0), src, times });
        }
    }
    if work.is_empty() {
        return Err(bad(cmd, "none of those has a picture to analyse"));
    }
    // spread the frame budget over the takes
    let total: usize = work.iter().map(|w| w.times.len()).sum();
    if total > MAX_TAKE_SAMPLES {
        for w in &mut work {
            let keep = ((w.times.len() * MAX_TAKE_SAMPLES) / total).max(2);
            let stride = w.times.len().div_ceil(keep).max(1);
            w.times = w.times.iter().copied().step_by(stride).collect();
        }
    }
    // measure
    let mut measured: Vec<Vec<(Tick, Option<Tick>, takes::FrameStats)>> = Vec::new();
    for w in &work {
        let full_w = w.src.info().video.as_ref().map(|v| v.width).unwrap_or(1).max(1) as f64;
        let scale = (width / full_w).min(1.0) as f32;
        let mut prev: Option<Vec<f32>> = None;
        let mut rows = Vec::new();
        for (mt, st) in &w.times {
            let f = w.src.video_frame(FrameRequest { time: *mt, scale }).map_err(|e| EngineError::Other(format!("{cmd}: {}: {e}", w.name)))?;
            let (fw, fh) = (f.width as usize, f.height as usize);
            let luma = takes::luma_plane(&f.to_rgba8(), fw, fh);
            let stats = takes::measure(&luma, fw, fh, prev.as_deref());
            prev = Some(luma);
            rows.push((*mt, *st, stats));
        }
        measured.push(rows);
    }
    let sharp_ref = measured.iter().flatten().map(|r| r.2.sharpness).fold(0f32, f32::max);
    let mut out: Vec<Value> = work
        .iter()
        .zip(&measured)
        .map(|(w, rows)| {
            let scores: Vec<f32> = rows.iter().map(|r| takes::score(&r.2, sharp_ref)).collect();
            let mean = scores.iter().sum::<f32>() / scores.len().max(1) as f32;
            let worst = scores.iter().copied().fold(f32::INFINITY, f32::min);
            // flags present in at least a quarter of the samples
            let mut counts: Vec<(&str, usize)> = Vec::new();
            for r in rows {
                for f in takes::flags(&r.2, sharp_ref) {
                    match counts.iter_mut().find(|c| c.0 == f) {
                        Some(c) => c.1 += 1,
                        None => counts.push((f, 1)),
                    }
                }
            }
            let flags: Vec<&str> = counts.iter().filter(|c| c.1 * 4 >= rows.len()).map(|c| c.0).collect();
            let avg = |f: fn(&takes::FrameStats) -> f32| rows.iter().map(|r| f(&r.2)).sum::<f32>() / rows.len().max(1) as f32;
            let r2 = |v: f32| (v as f64 * 1000.0).round() / 1000.0;
            let mut o = json!({
                "item": w.item.0,
                "clip": w.clip,
                "name": w.name,
                "score": (mean as f64 * 10.0).round() / 10.0,
                "worstScore": if worst.is_finite() { (worst as f64 * 10.0).round() / 10.0 } else { 0.0 },
                "flags": flags,
                "samples": rows.len(),
                "luma": r2(avg(|f| f.luma)),
                "clipped": r2(avg(|f| f.clipped)),
                "crushed": r2(avg(|f| f.crushed)),
                "motion": r2(avg(|f| f.motion)),
                "sharpness": r2(avg(|f| f.sharpness) / sharp_ref.max(1e-6)),
            });
            if detail {
                o["detail"] = json!(
                    rows.iter()
                        .zip(&scores)
                        .map(|((mt, st, f), sc)| json!({
                            "mediaSeconds": secs(*mt),
                            "sequenceSeconds": st.map(secs),
                            "score": (*sc as f64 * 10.0).round() / 10.0,
                            "sharpness": r2(f.sharpness / sharp_ref.max(1e-6)),
                            "luma": r2(f.luma), "clipped": r2(f.clipped), "crushed": r2(f.crushed), "motion": r2(f.motion),
                            "flags": takes::flags(f, sharp_ref),
                        }))
                        .collect::<Vec<_>>()
                );
            }
            o
        })
        .collect();
    out.sort_by(|a, b| b["score"].as_f64().unwrap_or(0.0).total_cmp(&a["score"].as_f64().unwrap_or(0.0)));
    Ok(json!({"takes": out, "sampledEverySeconds": every}))
}

// ---------------------------------------------------------------------------------------- script

fn script_align(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "script.align";
    let script = str_p(p, "script").ok_or_else(|| bad(cmd, "need `script` (the lines to find)"))?;
    let lines: Vec<String> = script.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).take(2_000).collect();
    if lines.is_empty() {
        return Err(bad(cmd, "`script` has no lines"));
    }
    let wanted = ids_p(p, "items");
    let sources: Vec<(ItemId, String, Vec<align::TWord>)> = s
        .project
        .transcripts
        .iter()
        .filter(|(id, _)| wanted.as_ref().is_none_or(|w| w.contains(&id.0)))
        .map(|(id, t)| {
            let name = s.project.item(*id).map(|i| i.name.clone()).unwrap_or_default();
            let words = t
                .words
                .iter()
                .map(|w| align::TWord { text: w.text.clone(), start: w.start.seconds(), end: w.end.seconds(), confidence: w.confidence })
                .collect();
            (*id, name, words)
        })
        .collect();
    if sources.is_empty() {
        return Err(bad(cmd, "no transcripts to search: run transcript.generate (or transcript.set) on the footage first"));
    }
    let top = u64_p(p, "top").unwrap_or(3).clamp(1, 50) as usize;
    let min = finite_p(p, "minScore").unwrap_or(0.5).clamp(0.0, 1.0) as f32;
    let words: Vec<Vec<align::TWord>> = sources.iter().map(|x| x.2.clone()).collect();
    let found = align::align(&lines, &words, top, min);
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    let mut missing = Vec::new();
    let out: Vec<Value> = lines
        .iter()
        .zip(&found)
        .enumerate()
        .map(|(k, (line, takes))| {
            if takes.is_empty() {
                missing.push(k);
            }
            json!({
                "line": k,
                "text": line,
                "takes": takes.iter().map(|t| {
                    let (id, name) = sources.get(t.source).map(|x| (x.0 .0, x.1.clone())).unwrap_or_default();
                    json!({
                        "item": id, "name": name,
                        "start": r3(t.start), "end": r3(t.end),
                        "startTicks": Tick::from_seconds_f64(t.start).0, "endTicks": Tick::from_seconds_f64(t.end).0,
                        "score": r3(t.score as f64), "coverage": r3(t.coverage as f64),
                        "fillers": t.fillers, "repeats": t.repeats,
                        "confidence": r3(t.confidence as f64), "wordsPerSecond": r3(t.pace as f64),
                        "text": t.text,
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({"lines": out, "missing": missing, "transcripts": sources.len()}))
}

// ---------------------------------------------------------------------------------------- QC

fn track_name(kind: TrackKind, k: usize) -> String {
    format!("{}{}", if kind == TrackKind::Video { "V" } else { "A" }, k + 1)
}

/// Mean Y, Cb, Cr (0..1 and around 0) of an RGBA8 frame.
fn ycc(rgba: &[u8]) -> [f32; 3] {
    let (mut y, mut cb, mut cr, mut n) = (0f64, 0f64, 0f64, 0f64);
    for px in rgba.chunks_exact(4) {
        let (r, g, b) = (px[0] as f64 / 255.0, px[1] as f64 / 255.0, px[2] as f64 / 255.0);
        let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        y += l;
        cb += (b - l) / 1.8556;
        cr += (r - l) / 1.5748;
        n += 1.0;
    }
    if n == 0.0 {
        return [0.0; 3];
    }
    [(y / n) as f32, (cb / n) as f32, (cr / n) as f32]
}

fn program_rgba(s: &Session, seq: ItemId, t: Tick, scale: f32) -> (usize, usize, Vec<u8>) {
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let opts = filmcraft_render::RenderOptions { scale, captions: true, ..Default::default() };
    let img = filmcraft_render::render_sequence(&s.project, seq, t, opts, &provider);
    (img.w, img.h, img.over_black_rgba8())
}

fn measure_sequence(s: &Session, spec: &Value, every: f64) -> Result<qc::Measured> {
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.frame_rate;
    let fd = rate.frame_duration();
    let duration = q.duration();
    let mut m = qc::Measured { fps: rate.as_f64(), duration: duration.seconds(), ..Default::default() };
    for (k, t) in q.video_tracks.iter().enumerate().filter(|(_, t)| t.enabled) {
        for it in t.items.iter().filter(|i| i.enabled) {
            m.shots.push(qc::Shot {
                track: track_name(TrackKind::Video, k),
                clip: it.id.0,
                name: it.name.clone(),
                start: it.start.seconds(),
                end: it.end().seconds(),
            });
        }
    }
    for t in q.caption_tracks.iter().filter(|t| t.enabled) {
        for c in &t.captions {
            m.cues.push(qc::Cue { track: t.name.clone(), id: c.id.0, start: c.start.seconds(), end: (c.start + c.duration).seconds(), text: c.text.clone() });
        }
    }
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let sr = q.settings.sample_rate.max(1);
    // loudness and true peak of the mix
    if spec.get("loudness") != Some(&Value::Null) && duration > Tick::ZERO {
        let mut meter = filmcraft_audio_dsp::LoudnessMeter::new(sr as f64, 2);
        let (mut at, stop) = (0i64, duration.to_units_floor(sr as i64));
        while at < stop {
            let n = (stop - at).min(sr as i64 * 5) as usize;
            let b = filmcraft_render::audio::mix_sequence(&s.project, q, at, n, &provider);
            if let (Some(l), Some(r)) = (b.channels.first(), b.channels.get(1)) {
                meter.process(&[l, r]);
            }
            at += n as i64;
        }
        let i = meter.integrated();
        m.integrated_lufs = i.is_finite().then_some(i);
        let tp = meter.true_peak_dbtp();
        m.true_peak_db = tp.is_finite().then_some(tp);
    }
    // the level at every audio clip edge
    if spec.get("audioClicks") != Some(&Value::Null) {
        let win = ((sr as f64 * 0.005) as usize).max(1);
        for (k, t) in q.audio_tracks.iter().enumerate().filter(|(_, t)| t.enabled && !t.muted) {
            for it in &t.items {
                for (edge, at) in [(it.start, it.start.to_units_floor(sr as i64)), (it.end(), it.end().to_units_floor(sr as i64) - win as i64)] {
                    let Some([l, r]) = filmcraft_render::audio::clip_signal(it, at.max(0), win, sr, &provider) else { continue };
                    let peak = l.iter().chain(&r).map(|v| v.abs()).fold(0f32, f32::max);
                    if peak <= 0.0 {
                        continue;
                    }
                    let faded = t.transitions.iter().any(|tr| edge >= tr.start && edge <= tr.end());
                    m.audio_edges.push(qc::AudioEdge {
                        track: track_name(TrackKind::Audio, k),
                        clip: it.id.0,
                        time: edge.seconds(),
                        peak_db: 20.0 * (peak as f64).log10(),
                        faded,
                    });
                }
            }
        }
    }
    // program picture samples (black frames)
    if spec.get("blackFrames") != Some(&Value::Null) && duration > Tick::ZERO {
        let step = Tick::from_seconds_f64(every).max(fd);
        let n = ((duration.0 / step.0.max(1)) as usize + 1).min(MAX_QC_SAMPLES);
        let step = Tick((duration.0 / n.max(1) as i64).max(fd.0));
        for k in 0..n {
            let t = rate.snap(Tick(step.0 * k as i64 + step.0 / 2)).min(duration - fd);
            let (_, _, px) = program_rgba(s, seq_id, t, 1.0 / 16.0);
            m.luma_samples.push((t.seconds(), ycc(&px)[0]));
        }
    }
    // the picture each side of every cut
    if spec.get("colorContinuity").is_some_and(Value::is_object) {
        for c in qc::cut_points(&m).into_iter().take(500) {
            let t = rate.snap_nearest(Tick::from_seconds_f64(c));
            let (_, _, a) = program_rgba(s, seq_id, t - fd, 1.0 / 8.0);
            let (_, _, b) = program_rgba(s, seq_id, t, 1.0 / 8.0);
            m.cut_looks.push(qc::CutLook { time: c, before: ycc(&a), after: ycc(&b) });
        }
    }
    Ok(m)
}

fn qc_check(s: &mut Session, p: &Value) -> Result<Value> {
    let spec = match p.get("spec") {
        None | Some(Value::Null) => json!({}),
        Some(v) if v.is_object() => v.clone(),
        Some(_) => return Err(bad("qc.check", "`spec` must be an object")),
    };
    let every = finite_p(p, "sampleEvery").unwrap_or(1.0).clamp(0.04, 600.0);
    let m = measure_sequence(s, &spec, every)?;
    let (issues, metrics) = qc::check(&m, &spec);
    let errors = issues.iter().filter(|i| i.severity == "error").count();
    let warnings = issues.iter().filter(|i| i.severity == "warning").count();
    Ok(json!({
        "pass": errors == 0,
        "errors": errors,
        "warnings": warnings,
        "issues": issues.iter().map(qc::issue_json).collect::<Vec<_>>(),
        "metrics": metrics,
    }))
}

fn qc_filmstrip(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "qc.filmstrip";
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.frame_rate;
    let fd = rate.frame_duration();
    let duration = q.duration();
    if duration <= fd {
        return Err(bad(cmd, "the sequence is empty"));
    }
    let (sw, sh) = (q.settings.width.max(1) as f64, q.settings.height.max(1) as f64);
    let times: Vec<f64> = match p.get("times").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_f64).filter(|t| t.is_finite()).collect(),
        None => {
            let mut m = qc::Measured { fps: rate.as_f64(), duration: duration.seconds(), ..Default::default() };
            for t in q.video_tracks.iter().filter(|t| t.enabled) {
                for it in t.items.iter().filter(|i| i.enabled) {
                    m.shots.push(qc::Shot { track: String::new(), clip: it.id.0, name: String::new(), start: it.start.seconds(), end: it.end().seconds() });
                }
            }
            qc::cut_points(&m)
        }
    };
    let offset = u64_p(p, "offset").unwrap_or(0) as usize;
    let limit = u64_p(p, "limit").unwrap_or(12).clamp(1, MAX_STRIP_ROWS as u64) as usize;
    let rows: Vec<f64> = times.iter().copied().skip(offset).take(limit).collect();
    if rows.is_empty() {
        return Err(bad(cmd, "no cuts (or times) to show"));
    }
    let tile_w = u64_p(p, "tileWidth").unwrap_or(240).clamp(32, 960) as usize;
    let tile_h = ((tile_w as f64 * sh / sw).round() as usize).clamp(8, 2000);
    let scale = (tile_w as f64 / sw).min(1.0) as f32;
    const COLS: usize = 4;
    const PAD: usize = 4;
    let (w, h) = (COLS * tile_w + (COLS + 1) * PAD, rows.len() * tile_h + (rows.len() + 1) * PAD);
    let mut canvas = vec![0u8; w * h * 4];
    for px in canvas.chunks_exact_mut(4) {
        px.copy_from_slice(&[24, 24, 28, 255]);
    }
    let half = Tick::from_seconds_f64(0.5);
    let last = duration - fd;
    let mut row_json = Vec::new();
    for (r, c) in rows.iter().enumerate() {
        let cut = rate.snap_nearest(Tick::from_seconds_f64(c.max(0.0))).min(last);
        let cols = [(cut - half).max(Tick::ZERO), (cut - fd).max(Tick::ZERO), cut, (cut + half).min(last)];
        for (k, t) in cols.iter().enumerate() {
            let (iw, ih, px) = program_rgba(s, seq_id, *t, scale);
            let (x0, y0) = (PAD + k * (tile_w + PAD), PAD + r * (tile_h + PAD));
            for y in 0..tile_h {
                let sy = if ih == 0 { continue } else { (y * ih / tile_h).min(ih - 1) };
                for x in 0..tile_w {
                    let sx = if iw == 0 { continue } else { (x * iw / tile_w).min(iw - 1) };
                    let (Some(src), Some(dst)) =
                        (px.get((sy * iw + sx) * 4..(sy * iw + sx) * 4 + 4), canvas.get_mut(((y0 + y) * w + x0 + x) * 4..((y0 + y) * w + x0 + x) * 4 + 4))
                    else {
                        continue;
                    };
                    dst.copy_from_slice(src);
                }
            }
        }
        // the cut sits between columns 2 and 3: mark it
        let x = PAD + 2 * (tile_w + PAD) - PAD;
        for y in PAD + r * (tile_h + PAD)..PAD + r * (tile_h + PAD) + tile_h {
            for dx in 0..PAD {
                if let Some(d) = canvas.get_mut(((y * w) + x + dx) * 4..((y * w) + x + dx) * 4 + 4) {
                    d.copy_from_slice(&[230, 70, 60, 255]);
                }
            }
        }
        row_json.push(json!({"row": r, "cutSeconds": secs(cut), "columns": cols.iter().map(|t| secs(*t)).collect::<Vec<_>>()}));
    }
    let png = filmcraft_export::encode_png(canvas, w as u32, h as u32).map_err(|e| EngineError::Other(format!("{cmd}: {e}")))?;
    let mut out = json!({"width": w, "height": h, "rows": row_json, "totalCuts": times.len(), "layout": "each row: cut-0.5s | last frame before the cut | first frame after | cut+0.5s"});
    if let Some(path) = str_p(p, "path") {
        s.services.write_file(path, &png).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        out["path"] = json!(path);
    } else {
        out["pngBase64"] = json!(filmcraft_project::gtemplate::base64_encode(&png));
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
