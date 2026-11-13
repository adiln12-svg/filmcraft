//! Creator Mode sound tools: fades, music lowered under speech, beat markers, voice clean-up.

use serde_json::json;

use crate::Session;
use filmcraft_project::ClipId;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

/// A 16-bit mono WAV of clicks at `bpm`.
fn click_track(path: &std::path::Path, bpm: f64, seconds: f64) {
    let sr = 44_100u32;
    let n = (seconds * sr as f64) as usize;
    let every = (60.0 / bpm * sr as f64) as usize;
    let mut pcm = Vec::with_capacity(n * 2);
    for i in 0..n {
        let k = i % every;
        let v = if k < 600 { ((k as f64 * 0.3).sin() * (1.0 - k as f64 / 600.0) * 28_000.0) as i16 } else { 0 };
        pcm.extend_from_slice(&v.to_le_bytes());
    }
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&sr.to_le_bytes());
    w.extend_from_slice(&(sr * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(&pcm);
    std::fs::write(path, w).unwrap();
}

#[test]
fn fades_and_ducking_are_one_undo_step_each_on_the_music() {
    let mut s = demo();
    let music = crate::creator_audio::music_clips(&s);
    assert!(!music.is_empty(), "the demo has a score");
    let undo0 = s.history.undo.len();
    let r = s.execute("creator.fade", json!({"edge": "both", "seconds": 1.0})).unwrap();
    assert_eq!(r["fades"].as_u64().unwrap() as usize, 2 * music.len(), "{r}");
    assert_eq!(s.history.undo.len(), undo0 + 1);
    let q = s.active_sequence().unwrap();
    let tid = q.find_item(music[0]).unwrap().0;
    assert!(q.audio_tracks.iter().find(|t| t.id == tid).unwrap().transitions.len() >= 2, "a fade in and a fade out");

    let r = s.execute("creator.duckMusic", json!({"reduceDb": -12})).unwrap();
    assert_eq!(s.history.undo.len(), undo0 + 2, "{r}");
    let q = s.active_sequence().unwrap();
    let it = q.find_item(music[0]).unwrap().1;
    let e = it.essential.as_ref().unwrap();
    assert!(e.ducking.enabled && e.ducking.against_untyped && (e.ducking.reduce_db + 12.0).abs() < 1e-9);
    // nothing to duck when there's no music; bad values refused without a change
    assert!(s.execute("creator.duckMusic", json!({"reduceDb": 5})).is_err());
    assert!(s.execute("creator.fade", json!({"seconds": -1})).is_err());
    assert_eq!(s.history.undo.len(), undo0 + 2);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let q = s.active_sequence().unwrap();
    assert!(q.find_item(music[0]).unwrap().1.essential.as_ref().is_none_or(|e| !e.ducking.enabled));
}

#[test]
fn clean_voice_turns_on_noise_reduction_for_the_video_sound() {
    let mut s = demo();
    let v1 = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("timeline.select", json!({"clips": [v1.0]})).unwrap();
    let r = s.execute("creator.cleanVoice", json!({"amount": 6})).unwrap();
    let clips: Vec<ClipId> = r["clips"].as_array().unwrap().iter().map(|c| ClipId(c.as_u64().unwrap())).collect();
    assert!(!clips.is_empty(), "{r}");
    let q = s.active_sequence().unwrap();
    for c in &clips {
        let e = q.find_item(*c).unwrap().1.essential.clone().unwrap();
        assert!(e.repair.noise.on && (e.repair.noise.amount - 6.0).abs() < 1e-9, "{:?}", e.repair);
    }
    assert!(!crate::creator_audio::music_clips(&s).iter().any(|m| clips.contains(m)), "not the music");
}

#[test]
fn beat_markers_land_on_the_music_beats_in_sequence_time() {
    let dir = crate::media_test_util::tmp_dir("fc_creator_beats");
    let wav = dir.join("clicks.wav");
    click_track(&wav, 120.0, 8.0);
    let mut s = demo();
    let item = s.execute("file.import", json!({"paths": [wav.to_string_lossy()]})).unwrap()["items"][0].as_u64().unwrap();
    // placed 2 s into the video: beats every 0.5 s from there
    s.execute("timeline.place", json!({"item": item, "audioTrack": "new", "time": filmcraft_time::Tick::from_seconds_f64(2.0).0, "video": false})).unwrap();
    let clip = s.active_sequence().unwrap().audio_tracks.last().unwrap().items[0].id;
    s.state.snapping = false;
    let old: Vec<_> = s.active_sequence().unwrap().markers.iter().map(|m| m.id).collect();
    let before = old.len();
    let undo0 = s.history.undo.len();
    let r = s.execute("creator.beatMarkers", json!({"clip": clip.0})).unwrap();
    assert!(s.state.snapping, "snapping on");
    assert_eq!(s.history.undo.len(), undo0 + 1);
    let bpm = r["bpm"].as_f64().unwrap();
    assert!((bpm - 120.0).abs() < 3.0, "{r}");
    let q = s.active_sequence().unwrap();
    let marks: Vec<f64> = q.markers.iter().filter(|m| !old.contains(&m.id)).map(|m| m.start.seconds()).collect();
    assert!(marks.iter().all(|m| *m >= 2.0 - 1e-6), "{marks:?}");
    assert!(q.markers.len() > before + 8, "{r}");
    for m in &marks {
        let phase = ((m - 2.0) / 0.5 - ((m - 2.0) / 0.5).round()).abs() * 0.5;
        assert!(phase < 0.05, "a marker {m:.3}s off the beat grid");
    }
    assert!(marks.iter().all(|m| *m <= 10.0 + 1e-6), "only beats the clip plays");
    let _ = std::fs::remove_dir_all(&dir);
}
