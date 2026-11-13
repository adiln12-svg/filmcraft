//! Creator Mode sound tools (M3D): plain-language wrappers over the Pro audio commands, so a
//! beginner (or Claude) gets fades, music lowered under speech, beat markers and cleaner voice in
//! one undoable step each. Nothing here processes audio itself: fades are audio transitions,
//! ducking is Essential Sound's ducking keyframes, beats are `analyze.beats`, voice clean-up is
//! Essential Sound's noise reduction.

use serde_json::{Value, json};

use filmcraft_project::{ClipId, TrackKind};

use crate::commands::{CommandSpec, bad, f64_p, has_seq, str_p};
use crate::{EngineError, Result, Session};

/// `clips` from the params, else the selection, else (with `fallback`) every clip it picks.
fn clips_p(s: &Session, p: &Value, fallback: impl Fn(&Session) -> Vec<ClipId>) -> Vec<ClipId> {
    if let Some(a) = p.get("clips").and_then(Value::as_array) {
        return a.iter().filter_map(Value::as_u64).map(ClipId).collect();
    }
    if !s.state.selection.is_empty() {
        return s.state.selection.clone();
    }
    fallback(s)
}

fn audio_only(s: &Session, clips: &[ClipId]) -> Vec<ClipId> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    clips.iter().copied().filter(|c| q.find_item(*c).is_some_and(|(t, _)| q.track(t).is_some_and(|tr| tr.kind == TrackKind::Audio))).collect()
}

/// Music: audio clips without a picture (not linked to a video clip).
pub(crate) fn music_clips(s: &Session) -> Vec<ClipId> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let linked_video: Vec<u64> = q.video_tracks.iter().flat_map(|t| t.items.iter().filter_map(|i| i.link)).collect();
    q.audio_tracks.iter().flat_map(|t| t.items.iter()).filter(|i| i.link.is_none_or(|l| !linked_video.contains(&l))).map(|i| i.id).collect()
}

/// Voice: the sound of the video clips.
fn voice_clips(s: &Session) -> Vec<ClipId> {
    let music = music_clips(s);
    let Some(q) = s.active_sequence() else { return Vec::new() };
    q.audio_tracks.iter().flat_map(|t| t.items.iter()).map(|i| i.id).filter(|c| !music.contains(c)).collect()
}

fn ids(c: &[ClipId]) -> Value {
    json!(c.iter().map(|c| c.0).collect::<Vec<_>>())
}

/// `creator.fade {clips?, edge: "in|out|both", seconds}`: the sound fades in from / out to silence.
fn fade(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "creator.fade";
    let clips = audio_only(s, &clips_p(s, p, music_clips));
    if clips.is_empty() {
        return Err(bad(cmd, "select the music or sound to fade"));
    }
    let edges: &[&str] = match str_p(p, "edge").unwrap_or("both") {
        "in" => &["in"],
        "out" => &["out"],
        "both" => &["in", "out"],
        o => return Err(bad(cmd, format!("edge `{o}`: in, out or both"))),
    };
    let secs = f64_p(p, "seconds").unwrap_or(1.0);
    if !secs.is_finite() || !(0.05..=10.0).contains(&secs) {
        return Err(bad(cmd, "seconds: 0.05 to 10"));
    }
    let frames = s.sequence_rate().frame_at(filmcraft_time::Tick::from_seconds_f64(secs)).max(1);
    s.edit_group("Fade Sound", |s| {
        let mut n = 0;
        for c in &clips {
            for e in edges {
                s.execute(
                    "sequence.applyAudioTransition",
                    json!({"clip": c.0, "edge": e, "frames": frames, "align": if *e == "in" { "start" } else { "end" }}),
                )?;
                n += 1;
            }
        }
        Ok(json!({"fades": n, "clips": ids(&clips), "frames": frames}))
    })
}

/// `creator.duckMusic {clips?, reduceDb}`: the music gets quieter wherever someone speaks.
fn duck_music(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "creator.duckMusic";
    let music = audio_only(s, &clips_p(s, p, music_clips));
    let music: Vec<ClipId> = {
        let m = music_clips(s);
        music.into_iter().filter(|c| m.contains(c)).collect()
    };
    if music.is_empty() {
        return Err(bad(cmd, "add music first (Assets ▸ Music)"));
    }
    let db = f64_p(p, "reduceDb").unwrap_or(-15.0);
    if !db.is_finite() || !(-40.0..=-1.0).contains(&db) {
        return Err(bad(cmd, "reduceDb: -40 to -1"));
    }
    s.edit_group("Lower Music Under Voice", |s| {
        s.execute("essentialSound.setType", json!({"clips": ids(&music), "type": "music"}))?;
        s.execute(
            "essentialSound.set",
            json!({"clips": ids(&music), "values": {"ducking.enabled": true, "ducking.against_untyped": true, "ducking.reduce_db": db}}),
        )?;
        let r = s.execute("essentialSound.generateDucking", json!({"clips": ids(&music)}))?;
        Ok(json!({"clips": ids(&music), "reduceDb": db, "ducking": r["clips"]}))
    })
}

/// `creator.beatMarkers {clip?}`: sequence markers on the music's beats, with snapping on, so
/// cuts land on the beat when clips are trimmed or moved. One undo step.
fn beat_markers(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "creator.beatMarkers";
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let clips = match p.get("clip").and_then(Value::as_u64) {
        Some(c) => vec![ClipId(c)],
        None => music_clips(s),
    };
    let items: Vec<filmcraft_project::TrackItem> = clips.iter().filter_map(|c| q.find_item(*c).map(|(_, i)| i.clone())).collect();
    if items.is_empty() {
        return Err(bad(cmd, if p.get("clip").is_some() { "no such clip" } else { "add music first (Assets ▸ Music)" }));
    }
    // each song analysed once, then its beats placed under every clip that plays it
    let mut analysed: Vec<(u64, Value)> = Vec::new();
    let mut marks: Vec<(filmcraft_time::Tick, bool)> = Vec::new();
    for it in &items {
        let r = match analysed.iter().find(|a| a.0 == it.item.0) {
            Some(a) => a.1.clone(),
            None => {
                let r = s.execute("analyze.beats", json!({"item": it.item.0}))?;
                analysed.push((it.item.0, r.clone()));
                r
            }
        };
        marks.extend(clip_beats(it, &r));
    }
    if marks.is_empty() {
        return Err(bad(cmd, "no steady beat found in the music"));
    }
    marks.sort_by_key(|m| m.0);
    marks.dedup_by_key(|m| m.0);
    let n = s.edit_group("Beat Markers", |s| crate::autoedit::add_markers(s, None, &marks))?;
    s.state.snapping = true;
    Ok(json!({"bpm": analysed.first().map(|a| a.1["bpm"].clone()), "markers": n, "snapping": true}))
}

/// The beats of `analyze.beats` result `r` that clip `it` plays, in sequence time (downbeat flag).
fn clip_beats(it: &filmcraft_project::TrackItem, r: &Value) -> Vec<(filmcraft_time::Tick, bool)> {
    let speed = if it.speed.is_finite() && it.speed.abs() > 1e-6 { it.speed.abs() } else { 1.0 };
    let down: Vec<f64> = r["downbeats"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
    r["beats"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_f64)
        .filter_map(|b| {
            let off = (filmcraft_time::Tick::from_seconds_f64(b).0.saturating_sub(it.source_in.0)) as f64 / speed;
            let t = filmcraft_time::Tick(it.start.0.saturating_add(off.round() as i64));
            (off >= 0.0 && t < it.end()).then_some((t, down.iter().any(|d| (d - b).abs() < 1e-6)))
        })
        .collect()
}

/// `creator.cleanVoice {clips?, amount}`: noise reduction on the video's sound (Essential Sound ▸
/// Repair ▸ Reduce Noise).
fn clean_voice(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "creator.cleanVoice";
    let voice = voice_clips(s);
    let clips: Vec<ClipId> = audio_only(s, &clips_p(s, p, voice_clips)).into_iter().filter(|c| voice.contains(c)).collect();
    // a selected video clip means its sound
    let clips = if clips.is_empty() && p.get("clips").is_none() && !s.state.selection.is_empty() {
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        let links: Vec<u64> = s.state.selection.iter().filter_map(|c| q.find_item(*c).and_then(|(_, i)| i.link)).collect();
        voice.into_iter().filter(|c| q.find_item(*c).is_some_and(|(_, i)| i.link.is_some_and(|l| links.contains(&l)))).collect()
    } else {
        clips
    };
    if clips.is_empty() {
        return Err(bad(cmd, "no voice to clean: the video has no sound"));
    }
    let amount = f64_p(p, "amount").unwrap_or(4.0);
    if !amount.is_finite() || !(0.0..=10.0).contains(&amount) {
        return Err(bad(cmd, "amount: 0 to 10"));
    }
    s.edit_group("Clean Up Voice", |s| {
        s.execute("essentialSound.setType", json!({"clips": ids(&clips), "type": "dialogue"}))?;
        s.execute("essentialSound.set", json!({"clips": ids(&clips), "values": {"repair.noise.on": true, "repair.noise.amount": amount}}))?;
        Ok(json!({"clips": ids(&clips), "amount": amount}))
    })
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    let c = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: has_seq, run, journal: true };
    vec![
        c("creator.fade", "Fade Sound", r#"{"clips":[id]? (default: the selection, else the music),"edge":"in|out|both"="both","seconds":f64=1}"#, fade),
        c("creator.duckMusic", "Lower Music Under Voice", r#"{"clips":[id]? (default: the music),"reduceDb":f64=-15}"#, duck_music),
        c("creator.beatMarkers", "Beat Markers", r#"{"clip":id? (default: every music clip)} (sequence markers on the beats; snapping on)"#, beat_markers),
        c("creator.cleanVoice", "Clean Up Voice", r#"{"clips":[id]? (default: the selection's sound, else the video's sound),"amount":0..10=4}"#, clean_voice),
    ]
}
