//! Creator Mode: the few steps a short-form video needs, as ordinary commands on the shared
//! session (same project, timeline, undo history and command interface as the professional
//! workspace, the CLI, the control channel and MCP).
//!
//! - `creator.newVideo` — a sequence at a social aspect (9:16, 16:9, 1:1, 4:5) with the given
//!   files imported and placed back to back in filename order (natural order: `clip 2` before
//!   `clip 10`), each filling the frame; audio-only files go on their own track as music.
//! - `creator.addMedia` — import and append at the end of the video, in the same order.
//! - `creator.trimToPlayhead` — trim the start or the end of the clip under the playhead to the
//!   playhead and close the gap.
//! - `creator.transitions` — a transition at every cut of the main video track.
//! - `creator.exportPresets` / `creator.export` — social export presets, a file name without
//!   any doubled extension, and the export folder.
//!
//! Each editing command is one undo step. See `docs/creator-mode.md`.

use std::cmp::Ordering;
use std::path::Path;

use filmcraft_project::{ItemId, TrackKind};
use filmcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, has_seq, str_p};
use crate::{EngineError, Result, Session};

/// The aspect presets: label, frame width, frame height, what it is for.
pub const ASPECTS: [(&str, u32, u32, &str); 4] = [
    ("9:16", 1080, 1920, "TikTok, Reels, Shorts, Stories"),
    ("16:9", 1920, 1080, "YouTube, widescreen"),
    ("1:1", 1080, 1080, "Instagram and Facebook feed"),
    ("4:5", 1080, 1350, "Instagram feed"),
];

/// The social export choices: id, label, base preset, frame size (overrides the preset's).
pub const EXPORT_PRESETS: [(&str, &str, &str, (u32, u32)); 7] = [
    ("tiktok", "TikTok (9:16)", "TikTok 1080×1920", (1080, 1920)),
    ("reels", "Instagram Reels (9:16)", "Instagram Reels 1080×1920", (1080, 1920)),
    ("shorts", "YouTube Shorts (9:16)", "YouTube Shorts 1080×1920", (1080, 1920)),
    ("youtube", "YouTube (16:9)", "YouTube 1080p Full HD", (1920, 1080)),
    ("square", "Instagram Square (1:1)", "Instagram Reels 1080×1920", (1080, 1080)),
    ("portrait", "Instagram Portrait (4:5)", "Instagram Reels 1080×1920", (1080, 1350)),
    ("original", "Same as my video", filmcraft_export::presets::DEFAULT_PRESET, (0, 0)),
];

/// The most-fitting export choice for a frame size.
pub fn default_export_preset(width: u32, height: u32) -> &'static str {
    match aspect_of(width, height) {
        Some("16:9") => "youtube",
        Some("1:1") => "square",
        Some("4:5") => "portrait",
        Some(_) => "tiktok",
        None => "original",
    }
}

/// The aspect preset label of a frame size, if it is one of [`ASPECTS`] (any resolution).
pub fn aspect_of(width: u32, height: u32) -> Option<&'static str> {
    let (w, h) = (u64::from(width), u64::from(height));
    ASPECTS.iter().find(|(_, aw, ah, _)| w.saturating_mul(u64::from(*ah)) == h.saturating_mul(u64::from(*aw))).map(|a| a.0)
}

/// `name` without a trailing media or export file extension (`clip.mp4` → `clip`), so file
/// names derived from it never get the extension twice (`clip.mp4.mp4`).
pub fn strip_media_extension(name: &str) -> &str {
    let Some((stem, ext)) = name.rsplit_once('.') else { return name };
    let ext = ext.to_ascii_lowercase();
    let known = filmcraft_media::VIDEO_EXTENSIONS
        .iter()
        .chain(filmcraft_media::AUDIO_EXTENSIONS)
        .chain(filmcraft_media::STILL_EXTENSIONS)
        .chain(&["mp4", "mov", "mxf", "webm", "mkv", "gif", "m4v", "avi", "prproj", "fcp"])
        .any(|e| *e == ext);
    if known && !stem.trim().is_empty() { stem } else { name }
}

/// Natural ("human") order of file names: case-insensitive, digit runs compared as numbers, so
/// `Clip 2` sorts before `Clip 10`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let n = digits(&mut x);
                let m = digits(&mut y);
                let o = n
                    .trim_start_matches('0')
                    .len()
                    .cmp(&m.trim_start_matches('0').len())
                    .then_with(|| n.trim_start_matches('0').cmp(m.trim_start_matches('0')));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

fn digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut s = String::new();
    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
        s.push(c);
        it.next();
    }
    s
}

fn file_name_of(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

/// `paths` sorted in natural file-name order (unless `keep` asks for the given order).
pub fn ordered_paths(paths: &[String], keep: bool) -> Vec<String> {
    let mut v = paths.to_vec();
    if !keep {
        v.sort_by(|a, b| natural_cmp(file_name_of(a), file_name_of(b)).then_with(|| natural_cmp(a, b)));
    }
    v
}

fn paths_p(p: &Value) -> Vec<String> {
    p.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn items_p(p: &Value) -> Vec<ItemId> {
    p.get("items").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_u64().map(ItemId)).collect()).unwrap_or_default()
}

/// Import `paths` in order; the imported item ids in that order, and the errors.
fn import_in_order(s: &mut Session, paths: &[String], cmd: &str) -> Result<(Vec<ItemId>, Vec<Value>)> {
    if paths.len() > 1_000 {
        return Err(bad(cmd, "at most 1000 files at a time"));
    }
    let mut ids = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        match s.execute("file.import", json!({ "paths": [path] })) {
            Ok(r) => {
                ids.extend(r["items"].as_array().into_iter().flatten().filter_map(Value::as_u64).map(ItemId));
                errors.extend(r["errors"].as_array().cloned().unwrap_or_default());
            }
            Err(e) => errors.push(json!(e.to_string())),
        }
    }
    Ok((ids, errors))
}

/// Visual items (video, stills, graphics) and audio-only items, each in order.
fn split_visual(s: &Session, items: &[ItemId]) -> (Vec<ItemId>, Vec<ItemId>) {
    items.iter().filter(|i| s.project.item(**i).is_some()).partition(|i| s.project.item(**i).is_some_and(|it| it.has_video()))
}

fn aspect_p(p: &Value, cmd: &str) -> Result<(&'static str, u32, u32)> {
    let want = str_p(p, "aspect").unwrap_or("9:16");
    ASPECTS
        .iter()
        .find(|a| a.0 == want || a.0.replace(':', "x") == want)
        .map(|a| (a.0, a.1, a.2))
        .ok_or_else(|| bad(cmd, format!("unknown aspect `{want}` (9:16, 16:9, 1:1, 4:5)")))
}

fn rate_for(s: &Session, p: &Value, visual: &[ItemId]) -> f64 {
    if let Some(f) = crate::commands::f64_p(p, "fps").filter(|f| f.is_finite() && *f >= 1.0 && *f <= 240.0) {
        return f;
    }
    let r = visual
        .iter()
        .find_map(|i| {
            s.project.item(*i).and_then(|it| it.as_media()).filter(|m| m.info.video.is_some()).map(|m| crate::commands::default_seq_settings_for(m).frame_rate)
        })
        .unwrap_or(FrameRate::new(30, 1));
    let f = r.as_f64();
    if f.is_finite() && f >= 1.0 { f } else { 30.0 }
}

fn unique_sequence_name(s: &Session, base: &str) -> String {
    let base = strip_media_extension(base.trim());
    let base = if base.is_empty() { "My Video" } else { base };
    let taken: Vec<String> = s.project.sequences().map(|i| i.name.clone()).collect();
    if !taken.iter().any(|n| n == base) {
        return base.to_string();
    }
    (2..10_000).map(|n| format!("{base} {n}")).find(|n| !taken.contains(n)).unwrap_or_else(|| base.to_string())
}

/// Place `visual` back to back from `at` on the main video track, fill the frame with each, and
/// put `audio` on a new audio track from 0 (music). The placed clip ids.
fn place_in_order(s: &mut Session, visual: &[ItemId], audio: &[ItemId], at: Tick, cmd: &str) -> Result<Vec<u64>> {
    let mut placed = Vec::new();
    let mut t = at;
    for item in visual {
        let r = s.execute("timeline.place", json!({"item": item.0, "track": "V1", "audioTrack": "A1", "time": t.0}))?;
        let clips: Vec<u64> = r["clips"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
        let end = s.active_sequence().and_then(|q| clips.iter().filter_map(|c| q.find_item(filmcraft_project::ClipId(*c)).map(|(_, i)| i.end())).max());
        t = end.ok_or_else(|| bad(cmd, "a clip could not be placed"))?;
        let video: Vec<u64> = clips
            .iter()
            .copied()
            .filter(|c| {
                s.active_sequence()
                    .and_then(|q| q.find_item(filmcraft_project::ClipId(*c)))
                    .is_some_and(|(tid, _)| s.active_sequence().and_then(|q| q.track(tid)).is_some_and(|tr| tr.kind == TrackKind::Video))
            })
            .collect();
        if !video.is_empty() {
            // a still or graphic without a source size can't be fitted: leave it as placed
            let _ = s.execute("clip.fillFrame", json!({ "clips": video }));
        }
        placed.extend(clips);
    }
    let video = video_end(s);
    for item in audio {
        // music as long as the video (a longer song is cut at its end; drag its end to change)
        let len = s.project.item(*item).map(|i| i.duration()).unwrap_or_default();
        let mut q = json!({"item": item.0, "audioTrack": "new", "time": 0, "video": false});
        if video > Tick::ZERO && len > video {
            q["duration"] = json!(video.0);
        }
        let r = s.execute("timeline.place", q)?;
        placed.extend(r["clips"].as_array().into_iter().flatten().filter_map(Value::as_u64));
    }
    Ok(placed)
}

/// The end of the last picture (clips, titles, captions) in the video.
fn picture_end(s: &Session) -> Tick {
    let Some(q) = s.active_sequence() else { return Tick::ZERO };
    let clips = q.video_tracks.iter().flat_map(|t| t.items.iter().map(|i| i.end()));
    let caps = q.caption_tracks.iter().flat_map(|t| t.captions.iter().map(|c| c.start + c.duration));
    clips.chain(caps).max().unwrap_or(Tick::ZERO)
}

/// The end of the main video track (where `creator.addMedia` appends).
fn video_end(s: &Session) -> Tick {
    s.active_sequence().and_then(|q| q.video_tracks.first()).and_then(|t| t.items.iter().map(|i| i.end()).max()).unwrap_or(Tick::ZERO)
}

fn order_json(s: &Session, items: &[ItemId]) -> Value {
    json!(items.iter().filter_map(|i| s.project.item(*i).map(|it| it.name.clone())).collect::<Vec<_>>())
}

fn new_video(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "creator.newVideo";
    let (aspect, w, h) = aspect_p(p, cmd)?;
    let keep = bool_p(p, "keepOrder").unwrap_or(false);
    let paths = ordered_paths(&paths_p(p), keep);
    let given = items_p(p);
    s.edit_group("New Video", |s| {
        let (mut items, errors) = import_in_order(s, &paths, cmd)?;
        items.extend(given.iter().copied().filter(|i| s.project.item(*i).is_some()));
        if !paths.is_empty() && items.is_empty() {
            return Err(EngineError::Other(format!("none of the files could be imported: {}", Value::Array(errors.clone()))));
        }
        let (visual, audio) = split_visual(s, &items);
        let fps = rate_for(s, p, &visual);
        let name = unique_sequence_name(s, str_p(p, "name").unwrap_or("My Video"));
        let r = s.execute("file.newSequence", json!({"name": name, "width": w, "height": h, "fps": fps, "video": 2, "audio": 2}))?;
        let seq = r["sequence"].as_u64().or_else(|| s.state.active_sequence.map(|q| q.0)).ok_or_else(|| bad(cmd, "no sequence was created"))?;
        let clips = place_in_order(s, &visual, &audio, Tick::ZERO, cmd)?;
        s.state.selection.clear();
        if let Some(id) = s.state.active_sequence {
            s.state.playheads.insert(id, Tick::ZERO);
        }
        let seconds = s.active_sequence().map(|q| q.duration().seconds()).unwrap_or(0.0);
        Ok(json!({"sequence": seq, "name": name, "aspect": aspect, "width": w, "height": h, "fps": fps, "items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "order": order_json(s, &visual), "music": order_json(s, &audio), "clips": clips, "seconds": seconds, "errors": errors}))
    })
}

fn add_media(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "creator.addMedia";
    s.active_sequence().ok_or(EngineError::NoSequence)?;
    let keep = bool_p(p, "keepOrder").unwrap_or(false);
    let paths = ordered_paths(&paths_p(p), keep);
    let given = items_p(p);
    if paths.is_empty() && given.is_empty() {
        return Err(bad(cmd, "need `paths` or `items`"));
    }
    s.edit_group("Add Media", |s| {
        let (mut items, errors) = import_in_order(s, &paths, cmd)?;
        items.extend(given.iter().copied().filter(|i| s.project.item(*i).is_some()));
        if items.is_empty() {
            return Err(EngineError::Other(format!("none of the files could be imported: {}", Value::Array(errors.clone()))));
        }
        let (visual, audio) = split_visual(s, &items);
        let at = match str_p(p, "at") {
            Some("playhead") => s.playhead(),
            _ => video_end(s),
        };
        let clips = place_in_order(s, &visual, &audio, at, cmd)?;
        s.state.selection.clear();
        let seconds = s.active_sequence().map(|q| q.duration().seconds()).unwrap_or(0.0);
        Ok(json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "order": order_json(s, &visual), "music": order_json(s, &audio), "clips": clips, "seconds": seconds, "errors": errors}))
    })
}

/// Run `f` (a ripple edit of `clips`) with only the clips' own tracks (and their linked sound or
/// picture) sync-locked, as one undo step: titles, music and captions on other tracks stay where
/// they are instead of blocking the edit (a title or song across the cut would). The tracks'
/// sync-lock settings are as before afterwards.
fn main_tracks_only<R>(s: &mut Session, clips: &[filmcraft_project::ClipId], label: &str, f: impl FnOnce(&mut Session) -> Result<R>) -> Result<R> {
    let keep: Vec<filmcraft_project::TrackId> = match s.active_sequence() {
        Some(q) => {
            let links: Vec<u64> = clips.iter().filter_map(|c| q.find_item(*c).and_then(|(_, i)| i.link)).collect();
            q.video_tracks
                .iter()
                .chain(q.audio_tracks.iter())
                .filter(|t| t.items.iter().any(|i| clips.contains(&i.id) || i.link.is_some_and(|l| links.contains(&l))))
                .map(|t| t.id)
                .collect()
        }
        None => return Err(EngineError::NoSequence),
    };
    s.edit_group(label, |s| {
        let saved = s.edit_sequence("Sync Lock", |q, _, _| {
            let mut saved = Vec::new();
            for t in q.video_tracks.iter_mut().chain(q.audio_tracks.iter_mut()) {
                if !keep.contains(&t.id) && t.sync_lock {
                    t.sync_lock = false;
                    saved.push(t.id);
                }
            }
            let caps: Vec<usize> = q.caption_tracks.iter().enumerate().filter(|(_, t)| t.sync_lock).map(|(i, _)| i).collect();
            for i in &caps {
                if let Some(t) = q.caption_tracks.get_mut(*i) {
                    t.sync_lock = false;
                }
            }
            Ok((saved, caps))
        })?;
        let r = f(s)?;
        s.edit_sequence("Sync Lock", |q, _, _| {
            for t in q.video_tracks.iter_mut().chain(q.audio_tracks.iter_mut()) {
                if saved.0.contains(&t.id) {
                    t.sync_lock = true;
                }
            }
            for i in &saved.1 {
                if let Some(t) = q.caption_tracks.get_mut(*i) {
                    t.sync_lock = true;
                }
            }
            Ok(())
        })?;
        Ok(r)
    })
}

fn trim(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "creator.trim";
    let clip = p.get("clip").and_then(Value::as_u64).map(filmcraft_project::ClipId).ok_or_else(|| bad(cmd, "need `clip`"))?;
    let edge = match str_p(p, "edge") {
        Some("in") => "in",
        Some("out") => "out",
        _ => return Err(bad(cmd, "edge: in | out")),
    };
    let delta = p.get("delta").and_then(Value::as_i64).ok_or_else(|| bad(cmd, "need `delta` (ticks)"))?;
    main_tracks_only(s, &[clip], "Trim", |s| {
        s.execute("timeline.trim", json!({"clip": clip.0, "edge": edge, "mode": "ripple", "delta": delta, "linked": true}))
    })
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let clips: Vec<filmcraft_project::ClipId> = match p.get("clips").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_u64).map(filmcraft_project::ClipId).collect(),
        None => s.state.selection.clone(),
    };
    if clips.is_empty() {
        return Err(bad("creator.delete", "select the clips to delete"));
    }
    let ids: Vec<u64> = clips.iter().map(|c| c.0).collect();
    main_tracks_only(s, &clips, "Delete", |s| s.execute("edit.rippleDelete", json!({ "clips": ids })))
}

/// The clip to act on: the selected clip on a video track, else the main-track clip under the playhead.
fn target_clip(s: &Session) -> Option<(filmcraft_project::ClipId, Tick, Tick)> {
    let q = s.active_sequence()?;
    let t = s.playhead();
    let under = |i: &&filmcraft_project::TrackItem| i.start < t && t < i.end();
    let selected = s.state.selection.iter().find_map(|c| {
        q.find_item(*c).filter(|(tid, i)| q.track(*tid).is_some_and(|tr| tr.kind == TrackKind::Video) && under(i)).map(|(_, i)| (i.id, i.start, i.end()))
    });
    selected.or_else(|| q.video_tracks.iter().flat_map(|tr| tr.items.iter()).find(under).map(|i| (i.id, i.start, i.end())))
}

fn trim_to_playhead(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "creator.trimToPlayhead";
    let side = str_p(p, "side").unwrap_or("start");
    let (clip, start, end) = target_clip(s).ok_or_else(|| bad(cmd, "move the playhead inside the clip you want to trim"))?;
    let t = s.playhead();
    if t <= start || t >= end {
        return Err(bad(cmd, "move the playhead inside the clip first"));
    }
    let (edge, delta) = match side {
        "start" => ("in", t.0.saturating_sub(start.0)),
        "end" => ("out", t.0.saturating_sub(end.0)),
        _ => return Err(bad(cmd, "side: start | end")),
    };
    let label = if side == "start" { "Trim Start to Playhead" } else { "Trim End to Playhead" };
    main_tracks_only(s, &[clip], label, |s| {
        s.execute("timeline.trim", json!({"clip": clip.0, "edge": edge, "mode": "ripple", "delta": delta, "linked": true}))
    })?;
    if side == "start" {
        let id = s.state.active_sequence;
        if let Some(id) = id {
            s.state.playheads.insert(id, start);
        }
    }
    Ok(json!({"clip": clip.0, "side": side}))
}

/// Move a clip to another place in its track's order, closing the gap it leaves (a
/// rearrange): dropped at `time`, it goes to the nearest cut between the other clips.
fn move_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "creator.moveClip";
    let clip = p.get("clip").and_then(Value::as_u64).map(filmcraft_project::ClipId).ok_or_else(|| bad(cmd, "need `clip`"))?;
    let want = Tick(p.get("time").and_then(Value::as_i64).ok_or_else(|| bad(cmd, "need `time` (ticks)"))?.max(0));
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (tid, it) = q.find_item(clip).ok_or_else(|| bad(cmd, "no such clip"))?;
    let (start, dur) = (it.start, it.duration);
    let tr = q.track(tid).ok_or_else(|| bad(cmd, "no such track"))?;
    // the track's clips in order without this one, then where it goes among them: before the
    // first whose middle is after the drop point (in the closed-up timeline)
    let mut order: Vec<(filmcraft_project::ClipId, Tick, Tick)> = tr.items.iter().map(|i| (i.id, i.start, i.duration)).collect();
    order.sort_by_key(|x| x.1);
    let origin = order.first().map_or(Tick::ZERO, |x| x.1);
    let rest: Vec<(filmcraft_project::ClipId, Tick)> = order.iter().filter(|x| x.0 != clip).map(|x| (x.0, x.2)).collect();
    let w = if want > start { Tick(want.0.saturating_sub(dur.0)) } else { want };
    let mut t = origin;
    let mut index = rest.len();
    for (i, (_, d)) in rest.iter().enumerate() {
        if w.0 < t.0.saturating_add(d.0 / 2) {
            index = i;
            break;
        }
        t = Tick(t.0.saturating_add(d.0));
    }
    let mut new_order = rest;
    new_order.insert(index.min(new_order.len()), (clip, dur));
    let same = new_order.iter().map(|x| x.0).eq(order.iter().map(|x| x.0));
    if same {
        return Ok(json!({"clip": clip.0, "index": index, "moved": false}));
    }
    // every clip of the track to its place back to back (their sound follows; nothing else moves)
    let mut moves = Vec::new();
    let mut at = origin;
    for (c, d) in &new_order {
        moves.push(json!({"clip": c.0, "track": tid.0, "time": at.0}));
        at = Tick(at.0.saturating_add(d.0));
    }
    s.edit_group("Move Clip", |s| {
        s.execute("timeline.move", json!({ "moves": moves, "insert": false }))?;
        Ok(json!({"clip": clip.0, "index": index, "moved": true}))
    })
}

/// Cuts on the main video track: clips that end exactly where the next one starts.
fn cuts(s: &Session) -> Vec<u64> {
    let Some(tr) = s.active_sequence().and_then(|q| q.video_tracks.first()) else { return Vec::new() };
    let mut v: Vec<_> = tr.items.iter().map(|i| (i.start, i.end(), i.id.0)).collect();
    v.sort_by_key(|x| x.0);
    v.windows(2).filter(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.1 == b.0)).filter_map(|w| w.first().map(|a| a.2)).collect()
}

fn transitions(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "creator.transitions";
    let at = cuts(s);
    if at.is_empty() {
        return Err(bad(cmd, "the video has no cuts between clips yet"));
    }
    let effect = str_p(p, "effect").unwrap_or("cross_dissolve").to_string();
    let frames = p.get("frames").and_then(Value::as_i64).unwrap_or(10).clamp(1, 600);
    s.edit_group("Add Transitions", |s| {
        let mut ids = Vec::new();
        for c in &at {
            if let Ok(r) = s.execute("sequence.applyVideoTransition", json!({"clip": c, "edge": "out", "effect": effect, "frames": frames, "align": "center"}))
            {
                ids.push(r["transition"].clone());
            }
        }
        if ids.is_empty() {
            return Err(bad(cmd, "no transition could be added (the clips need spare frames at their ends)"));
        }
        Ok(json!({"transitions": ids, "cuts": at.len()}))
    })
}

fn add_title(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "creator.addTitle";
    let (w, h) = s.active_sequence().map(|q| (q.settings.width, q.settings.height)).ok_or(EngineError::NoSequence)?;
    let text = str_p(p, "text").filter(|t| !t.trim().is_empty()).unwrap_or("Your title").to_string();
    let seconds = crate::commands::f64_p(p, "seconds").filter(|x| x.is_finite() && *x > 0.0).unwrap_or(3.0).min(3600.0);
    // a tenth of the frame's short side, centred a third of the way down
    let size = (f64::from(w.min(h)) * 0.1).round().max(12.0);
    let (x, y) = (f64::from(w) / 2.0, (f64::from(h) / 3.0).round());
    let at = s.playhead();
    s.edit_group("Add Title", |s| {
        let r = s.execute("graphics.newText", json!({"text": text, "seconds": seconds, "size": size, "time": at.0}))?;
        let clip = r["clip"].as_u64().ok_or_else(|| bad(cmd, "no title clip was made"))?;
        s.execute("graphics.set", json!({"clip": clip, "layer": 0, "props": {"align": "center", "position": [x, y]}}))?;
        Ok(json!({"clip": clip}))
    })
}

fn export_presets(s: &mut Session, _: &Value) -> Result<Value> {
    let suggested = s.active_sequence().map(|q| default_export_preset(q.settings.width, q.settings.height)).unwrap_or("tiktok");
    let list: Vec<Value> = EXPORT_PRESETS
        .iter()
        .map(|(id, label, base, (w, h))| json!({"id": id, "label": label, "preset": base, "width": if *w > 0 { json!(w) } else { Value::Null }, "height": if *h > 0 { json!(h) } else { Value::Null }}))
        .collect();
    Ok(json!({"presets": list, "suggested": suggested, "folder": crate::export_tools::default_export_dir(s).to_string_lossy()}))
}

/// The export file path: `folder/name.mp4`, the name without a doubled extension, made unique
/// (`name 2.mp4`) unless `overwrite`.
pub fn export_path(folder: &Path, name: &str, ext: &str, overwrite: bool) -> String {
    let stem = crate::export_tools::file_safe(strip_media_extension(name.trim()));
    let stem = strip_media_extension(&stem).to_string();
    let first = folder.join(format!("{stem}.{ext}"));
    if overwrite || !first.exists() {
        return first.to_string_lossy().to_string();
    }
    (2..10_000).map(|n| folder.join(format!("{stem} {n}.{ext}"))).find(|p| !p.exists()).unwrap_or(first).to_string_lossy().to_string()
}

fn has_captions(s: &Session) -> bool {
    s.active_sequence().is_some_and(|q| q.caption_tracks.iter().any(|t| !t.captions.is_empty()))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "creator.export";
    let (w0, h0) = s.active_sequence().map(|q| (q.settings.width, q.settings.height)).ok_or(EngineError::NoSequence)?;
    let id = str_p(p, "preset").unwrap_or_else(|| default_export_preset(w0, h0));
    let (_, label, base, (w, h)) = EXPORT_PRESETS
        .iter()
        .find(|x| x.0 == id || x.1 == id)
        .copied()
        .ok_or_else(|| bad(cmd, format!("unknown preset `{id}` (see creator.exportPresets)")))?;
    let name = match str_p(p, "name").filter(|n| !n.trim().is_empty()) {
        Some(n) => n.to_string(),
        None => s.state.active_sequence.and_then(|q| s.project.item(q)).map(|i| i.name.clone()).unwrap_or_else(|| "My Video".into()),
    };
    let folder = match str_p(p, "folder").filter(|f| !f.trim().is_empty()) {
        Some(f) => std::path::PathBuf::from(crate::export_tools::expand_home(f)),
        None => crate::export_tools::default_export_dir(s),
    };
    if !folder.is_dir() {
        return Err(bad(cmd, format!("the folder {} does not exist", folder.display())));
    }
    let path = export_path(&folder, &name, "mp4", bool_p(p, "overwrite").unwrap_or(false));
    let mac = cfg!(target_os = "macos");
    let fast = bool_p(p, "hardwareAcceleration").unwrap_or(mac);
    let mut q = json!({
        "preset": base,
        "path": path,
        "burnCaptions": bool_p(p, "burnCaptions").unwrap_or_else(|| has_captions(s)),
        "hardwareEncoding": fast,
        "gpuRendering": fast,
        "wait": bool_p(p, "wait").unwrap_or(false),
        "allowFontSubstitution": bool_p(p, "allowFontSubstitution").unwrap_or(false),
    });
    if w > 0 && h > 0 {
        q["width"] = json!(w);
        q["height"] = json!(h);
    }
    // the video is as long as its picture: music or sound past the last clip is not exported as black
    let end = picture_end(s);
    if end > Tick::ZERO && s.active_sequence().is_some_and(|q| q.duration() > end) {
        q["range"] = json!("custom");
        q["startTime"] = json!(0);
        q["endTime"] = json!(end.0);
    }
    let mut out = s.execute("file.exportMedia", q)?;
    out["creatorPreset"] = json!(label);
    out["hardwareAcceleration"] = json!(fast);
    Ok(out)
}

/// The Creator Mode commands.
pub(crate) fn commands() -> Vec<CommandSpec> {
    let c = |id, label, params, enabled, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled, run, journal: true };
    vec![
        c(
            "creator.newVideo",
            "New Video",
            r#"{"aspect":"9:16|16:9|1:1|4:5"="9:16","name":str?,"paths":[str]? (imported and placed in file-name order),"items":[id]?,"keepOrder":bool=false,"fps":f64? (default: the first clip's)}"#,
            always,
            new_video,
        ),
        c(
            "creator.addMedia",
            "Add to Video",
            r#"{"paths":[str]?,"items":[id]?,"at":"end|playhead"="end","keepOrder":bool=false} (audio-only files become music on a new track from the start)"#,
            has_seq,
            add_media,
        ),
        c(
            "creator.trimToPlayhead",
            "Trim to Playhead",
            r#"{"side":"start|end"} (the selected clip, or the one under the playhead; the gap closes)"#,
            has_seq,
            trim_to_playhead,
        ),
        c(
            "creator.trim",
            "Creator Trim",
            r#"{"clip":id,"edge":"in|out","delta":ticks} (ripple: later clips follow; titles, music and captions stay)"#,
            has_seq,
            trim,
        ),
        c("creator.delete", "Creator Delete", r#"{"clips":[id]?} (default: the selection; titles, music and captions on other tracks stay)"#, has_seq, delete),
        c("creator.moveClip", "Move Clip", r#"{"clip":id,"time":ticks} (to the cut nearest `time`; the gap it leaves closes)"#, has_seq, move_clip),
        c("creator.addTitle", "Add Title", r#"{"text":str="Your title","seconds":f64=3} (at the playhead, centred, sized to the frame)"#, has_seq, add_title),
        c("creator.transitions", "Transitions at Every Cut", r#"{"effect":str="cross_dissolve","frames":i64=10}"#, has_seq, transitions),
        CommandSpec { journal: false, ..c("creator.exportPresets", "Social Export Presets", "{}", always, export_presets) },
        c(
            "creator.export",
            "Export Video",
            r#"{"preset":"tiktok|reels|shorts|youtube|square|portrait|original"? (default: by the video's shape),"name":str? (no extension needed),"folder":str?,"overwrite":bool=false,"burnCaptions":bool? (default: when the video has captions),"hardwareAcceleration":bool? (default: on for macOS),"wait":bool=false}"#,
            has_seq,
            export,
        ),
    ]
}
