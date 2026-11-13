//! Creator Mode commands: aspect presets, file-name order, appending, trimming, transitions and
//! export naming.

use serde_json::json;

use crate::Session;
use crate::creator::{aspect_of, export_path, natural_cmp, ordered_paths, strip_media_extension};
use crate::media_test_util::{make_movie, tmp_dir};

#[test]
fn natural_order_puts_clip_2_before_clip_10() {
    let paths: Vec<String> = ["/a/clip 10.mp4", "/a/Clip 2.mp4", "/b/clip 1.mp4", "/a/clip 03.mp4"].iter().map(|s| s.to_string()).collect();
    let v = ordered_paths(&paths, false);
    assert_eq!(v, vec!["/b/clip 1.mp4", "/a/Clip 2.mp4", "/a/clip 03.mp4", "/a/clip 10.mp4"]);
    assert_eq!(ordered_paths(&paths, true), paths);
    assert_eq!(natural_cmp("phone_9", "phone_10"), std::cmp::Ordering::Less);
}

#[test]
fn media_extensions_are_stripped_once() {
    assert_eq!(strip_media_extension("phone_1.mp4"), "phone_1");
    assert_eq!(strip_media_extension("My Video.MOV"), "My Video");
    assert_eq!(strip_media_extension("v1.2 final"), "v1.2 final");
    assert_eq!(strip_media_extension(".mp4"), ".mp4");
    assert_eq!(aspect_of(1080, 1920), Some("9:16"));
    assert_eq!(aspect_of(2160, 3840), Some("9:16"));
    assert_eq!(aspect_of(1080, 1350), Some("4:5"));
    assert_eq!(aspect_of(1000, 1001), None);
}

#[test]
fn export_path_has_one_extension_and_never_overwrites() {
    let dir = tmp_dir("fc_creator_export");
    let p = export_path(&dir, "Launch.mp4", "mp4", false);
    assert!(p.ends_with("Launch.mp4") && !p.ends_with(".mp4.mp4"), "{p}");
    std::fs::write(&p, b"x").unwrap();
    let q = export_path(&dir, "Launch", "mp4", false);
    assert!(q.ends_with("Launch 2.mp4"), "{q}");
    assert!(export_path(&dir, "Launch", "mp4", true).ends_with("Launch.mp4"));
    assert!(export_path(&dir, "a/b:c", "mp4", false).ends_with("a_b_c.mp4"));
    let _ = std::fs::remove_dir_all(&dir);
}

fn clips(dir: &std::path::Path, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let p = dir.join(n);
            make_movie(&p, filmcraft_media::DemoScene::Plasma, 64, 112, 24 + 12 * (i as i64 % 2));
            p.to_string_lossy().to_string()
        })
        .collect()
}

fn v1_names(s: &Session) -> Vec<String> {
    let q = s.active_sequence().unwrap();
    let mut v: Vec<_> = q.video_tracks[0].items.iter().map(|i| (i.start, i.name.clone())).collect();
    v.sort_by_key(|x| x.0);
    v.into_iter().map(|x| x.1).collect()
}

#[test]
fn new_video_places_files_in_name_order_at_the_aspect_and_is_one_undo_step() {
    let dir = tmp_dir("fc_creator_new");
    // given out of order, as a file manager drop can be
    let paths = clips(&dir, &["shot 10.mp4", "shot 2.mp4", "shot 1.mp4"]);
    let mut s = Session::default();
    let undo0 = s.history.undo.len();
    let r = s.execute("creator.newVideo", json!({"aspect": "9:16", "paths": paths})).unwrap();
    assert_eq!(r["width"], 1080);
    assert_eq!(r["height"], 1920);
    assert_eq!(r["name"], "My Video");
    let q = s.active_sequence().unwrap();
    assert_eq!((q.settings.width, q.settings.height), (1080, 1920));
    assert_eq!(v1_names(&s), vec!["shot 1.mp4", "shot 2.mp4", "shot 10.mp4"]);
    // back to back, no gaps
    let q = s.active_sequence().unwrap();
    let mut items: Vec<_> = q.video_tracks[0].items.iter().map(|i| (i.start, i.end())).collect();
    items.sort();
    assert_eq!(items[0].0, filmcraft_time::Tick::ZERO);
    assert!(items.windows(2).all(|w| w[0].1 == w[1].0));
    assert_eq!(s.history.undo.len(), undo0 + 1, "one undo step");
    assert_eq!(s.history.undo.last().map(|e| e.0.as_str()), Some("New Video"));
    // a second video gets a unique name
    let r2 = s.execute("creator.newVideo", json!({"aspect": "1:1"})).unwrap();
    assert_eq!(r2["name"], "My Video 2");
    assert_eq!(r2["width"], 1080);
    assert_eq!(r2["height"], 1080);
    assert!(s.execute("creator.newVideo", json!({"aspect": "3:7"})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn add_media_appends_at_the_end_in_order_and_trim_split_transitions_work() {
    let dir = tmp_dir("fc_creator_add");
    let first = clips(&dir, &["a1.mp4", "a2.mp4"]);
    let more = clips(&dir, &["b10.mp4", "b9.mp4"]);
    let mut s = Session::default();
    s.execute("creator.newVideo", json!({"aspect": "16:9", "paths": first})).unwrap();
    // playhead at 0 must not matter: media is appended at the end
    s.execute("creator.addMedia", json!({"paths": more})).unwrap();
    assert_eq!(v1_names(&s), vec!["a1.mp4", "a2.mp4", "b9.mp4", "b10.mp4"]);
    let before = s.active_sequence().unwrap().duration();
    // trim the start of the first clip to 0.5 s: the gap closes
    s.execute("playhead.set", json!({"seconds": 0.5})).unwrap();
    s.execute("creator.trimToPlayhead", json!({"side": "start"})).unwrap();
    let q = s.active_sequence().unwrap();
    let mut starts: Vec<_> = q.video_tracks[0].items.iter().map(|i| i.start).collect();
    starts.sort();
    assert_eq!(starts[0], filmcraft_time::Tick::ZERO);
    assert!(q.duration() < before);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_sequence().unwrap().duration(), before);
    let t = s.execute("creator.transitions", json!({})).unwrap();
    assert!(t["transitions"].as_array().is_some_and(|a| !a.is_empty()), "{t}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn creator_export_names_the_file_once_with_the_social_frame_size() {
    let dir = tmp_dir("fc_creator_exp");
    let paths = clips(&dir, &["c.mp4"]);
    let mut s = Session::default();
    s.execute("creator.newVideo", json!({"aspect": "4:5", "paths": paths, "name": "c.mp4"})).unwrap();
    let presets = s.execute("creator.exportPresets", json!({})).unwrap();
    assert_eq!(presets["suggested"], "portrait");
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let r = s.execute("creator.export", json!({"folder": out.to_string_lossy(), "wait": true})).unwrap();
    let path = r["path"].as_str().unwrap().to_string();
    assert!(path.ends_with("/c.mp4") || path.ends_with("\\c.mp4"), "{path}");
    assert!(std::path::Path::new(&path).is_file());
    // the file really is 1080×1350: import it back and read its stream
    let id = s.execute("file.import", json!({"paths": [path]})).unwrap()["items"][0].as_u64().unwrap();
    let v = s.project.item(filmcraft_project::ItemId(id)).and_then(|i| i.as_media()).and_then(|m| m.info.video.clone()).unwrap();
    assert_eq!((v.width, v.height), (1080, 1350), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

fn a1_names(s: &Session) -> Vec<String> {
    let q = s.active_sequence().unwrap();
    let mut v: Vec<_> = q.audio_tracks[0].items.iter().map(|i| (i.start, i.name.clone())).collect();
    v.sort_by_key(|x| x.0);
    v.into_iter().map(|x| x.1).collect()
}

#[test]
fn move_clip_rearranges_without_gaps_and_keeps_sound_and_music_in_place() {
    let dir = tmp_dir("fc_creator_move");
    let mut paths = clips(&dir, &["m1.mp4", "m2.mp4", "m3.mp4", "m4.mp4"]);
    // music: a longer movie's sound only would need an audio file; use a clip and drop its video
    let music = dir.join("song.mp4");
    make_movie(&music, filmcraft_media::DemoScene::Plasma, 64, 112, 200);
    let mut s = Session::default();
    s.execute("creator.newVideo", json!({"aspect": "9:16", "paths": paths.clone()})).unwrap();
    let r = s.execute("file.import", json!({"paths": [music.to_string_lossy()]})).unwrap();
    let song = r["items"][0].as_u64().unwrap();
    s.execute("timeline.place", json!({"item": song, "audioTrack": "new", "time": 0, "video": false})).unwrap();
    let music_before: Vec<_> = s.active_sequence().unwrap().audio_tracks.last().unwrap().items.iter().map(|i| (i.start, i.duration)).collect();
    let before = s.active_sequence().unwrap().duration();
    let id_of = |s: &Session, name: &str| s.active_sequence().unwrap().video_tracks[0].items.iter().find(|i| i.name == name).unwrap().id.0;
    let end_of = |s: &Session, name: &str| s.active_sequence().unwrap().video_tracks[0].items.iter().find(|i| i.name == name).map(|i| i.end()).unwrap();
    // m1 dropped just after m3 → m2 m3 m1 m4
    let t = end_of(&s, "m3.mp4").0 + 1000;
    let undo0 = s.history.undo.len();
    s.execute("creator.moveClip", json!({"clip": id_of(&s, "m1.mp4"), "time": t})).unwrap();
    assert_eq!(v1_names(&s), vec!["m2.mp4", "m3.mp4", "m1.mp4", "m4.mp4"]);
    assert_eq!(a1_names(&s), v1_names(&s), "sound follows its picture");
    assert_eq!(s.history.undo.len(), undo0 + 1, "one undo step");
    // m4 dropped at the very start → m4 m2 m3 m1
    s.execute("creator.moveClip", json!({"clip": id_of(&s, "m4.mp4"), "time": 0})).unwrap();
    assert_eq!(v1_names(&s), vec!["m4.mp4", "m2.mp4", "m3.mp4", "m1.mp4"]);
    assert_eq!(a1_names(&s), v1_names(&s));
    let q = s.active_sequence().unwrap();
    let mut items: Vec<_> = q.video_tracks[0].items.iter().map(|i| (i.start, i.end())).collect();
    items.sort();
    assert_eq!(items[0].0, filmcraft_time::Tick::ZERO);
    assert!(items.windows(2).all(|w| w[0].1 == w[1].0), "no gaps: {items:?}");
    assert_eq!(q.duration(), before);
    let music_after: Vec<_> = q.audio_tracks.last().unwrap().items.iter().map(|i| (i.start, i.duration)).collect();
    assert_eq!(music_after, music_before, "the music is not cut or moved");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v1_names(&s), vec!["m2.mp4", "m3.mp4", "m1.mp4", "m4.mp4"]);
    paths.clear();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn trim_and_delete_close_the_gap_with_music_under_the_video() {
    let dir = tmp_dir("fc_creator_music");
    let paths = clips(&dir, &["k1.mp4", "k2.mp4", "k3.mp4"]);
    let music = dir.join("song.mp4");
    make_movie(&music, filmcraft_media::DemoScene::Plasma, 64, 112, 200);
    let mut s = Session::default();
    s.execute("creator.newVideo", json!({"aspect": "9:16", "paths": paths})).unwrap();
    let song = s.execute("file.import", json!({"paths": [music.to_string_lossy()]})).unwrap()["items"][0].as_u64().unwrap();
    s.execute("timeline.place", json!({"item": song, "audioTrack": "new", "time": 0, "video": false})).unwrap();
    let music_of = |s: &Session| s.active_sequence().unwrap().audio_tracks.last().unwrap().items.iter().map(|i| (i.start, i.duration)).collect::<Vec<_>>();
    let m0 = music_of(&s);
    let before = s.active_sequence().unwrap().duration();
    s.execute("playhead.set", json!({"seconds": 0.5})).unwrap();
    s.execute("creator.trimToPlayhead", json!({"side": "end"})).unwrap();
    s.execute("playhead.set", json!({"seconds": 0.75})).unwrap();
    s.execute("creator.trimToPlayhead", json!({"side": "start"})).unwrap();
    let q = s.active_sequence().unwrap();
    let mut items: Vec<_> = q.video_tracks[0].items.iter().map(|i| (i.start, i.end())).collect();
    items.sort();
    assert!(items.windows(2).all(|w| w[0].1 == w[1].0), "no gaps: {items:?}");
    let k2 = q.video_tracks[0].items.iter().find(|i| i.name == "k2.mp4").unwrap().id.0;
    s.execute("timeline.select", json!({"clips": [k2]})).unwrap();
    let undo0 = s.history.undo.len();
    s.execute("creator.delete", json!({})).unwrap();
    assert_eq!(s.history.undo.len(), undo0 + 1, "one undo step");
    assert!(s.active_sequence().unwrap().audio_tracks.last().unwrap().sync_lock, "sync lock restored");
    assert_eq!(v1_names(&s), vec!["k1.mp4", "k3.mp4"]);
    assert_eq!(a1_names(&s), v1_names(&s));
    assert!(s.active_sequence().unwrap().duration() <= before);
    assert_eq!(music_of(&s), m0, "music untouched");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn music_is_cut_to_the_video_and_export_stops_at_the_last_picture() {
    let dir = tmp_dir("fc_creator_len");
    let paths = clips(&dir, &["v1.mp4", "v2.mp4"]);
    let song = dir.join("song.wav");
    // 10 s of tone with our own WAV writer
    let n = 48_000 * 10;
    let mut wav = Vec::new();
    let data_len = (n * 4) as u32;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&48_000u32.to_le_bytes());
    wav.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..n {
        let v = ((i as f64 * 440.0 * std::f64::consts::TAU / 48_000.0).sin() * 8000.0) as i16;
        wav.extend_from_slice(&v.to_le_bytes());
        wav.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(&song, wav).unwrap();
    let mut s = Session::default();
    s.execute("creator.newVideo", json!({"aspect": "9:16", "paths": paths})).unwrap();
    let video = s.active_sequence().unwrap().duration();
    let r = s.execute("creator.addMedia", json!({"paths": [song.to_string_lossy()]})).unwrap();
    assert_eq!(r["music"].as_array().map(Vec::len), Some(1), "{r}");
    let q = s.active_sequence().unwrap();
    assert_eq!(q.duration(), video, "the song is cut at the video's end");
    // a title, then the song made longer by hand: the export still ends with the picture
    s.execute("playhead.set", json!({"seconds": 0})).unwrap();
    let t = s.execute("creator.addTitle", json!({"text": "Hello", "seconds": 1})).unwrap();
    assert!(t["clip"].as_u64().is_some());
    let music = s.active_sequence().unwrap().audio_tracks.last().unwrap().items[0].id.0;
    s.execute("timeline.trim", json!({"clip": music, "edge": "out", "mode": "regular", "delta": 254016000000i64 * 3})).unwrap();
    assert!(s.active_sequence().unwrap().duration() > video);
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let r = s.execute("creator.export", json!({"folder": out.to_string_lossy(), "name": "len", "wait": true})).unwrap();
    let frames = r["result"]["frames"].as_u64().unwrap();
    let want = s.active_sequence().unwrap().settings.frame_rate.frame_at(video) as u64;
    assert_eq!(frames, want, "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn smooth_editing_makes_editing_copies_of_heavy_clips_only() {
    let dir = tmp_dir("fc_creator_smooth");
    // one heavy clip (2160×1200 = 2.6 MP) and one light one
    let heavy = dir.join("big.mov");
    make_movie(&heavy, filmcraft_media::DemoScene::Plasma, 2160, 1200, 6);
    let light = clips(&dir, &["small.mp4"]);
    let mut s = Session::default();
    s.execute("creator.newVideo", json!({"aspect": "16:9", "paths": [heavy.to_string_lossy(), light[0]]})).unwrap();
    let st = s.execute("creator.smoothEditingStatus", json!({})).unwrap();
    assert_eq!(st["heavyClips"], 1, "{st}");
    assert_eq!(st["suggest"], true);
    let r = s.execute("creator.smoothEditing", json!({"wait": true})).unwrap();
    assert_eq!(r["copiesStarted"], 1, "{r}");
    let st = s.execute("creator.smoothEditingStatus", json!({})).unwrap();
    assert_eq!((st["withCopies"].as_u64(), st["using"].as_bool(), st["suggest"].as_bool()), (Some(1), Some(true), Some(false)), "{st}");
    assert!(s.media.use_proxies());
    // the light clip got none
    let small = s.project.items.values().find(|i| i.name == "small.mp4").unwrap();
    assert!(small.as_media().unwrap().proxy.is_none());
    // and the originals again
    s.execute("creator.smoothEditing", json!({"on": false})).unwrap();
    assert!(!s.media.use_proxies());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Captions over each clip: text, start and end (seconds), sorted by start.
fn captions_of(s: &Session) -> Vec<(String, f64, f64)> {
    let q = s.active_sequence().unwrap();
    let mut v: Vec<_> = q.caption_tracks[0].captions.iter().map(|c| (c.text.clone(), c.start.seconds(), (c.start + c.duration).seconds())).collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1));
    v
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.03
}

#[test]
fn captions_follow_the_footage_through_trim_delete_and_move() {
    let dir = tmp_dir("fc_creator_caps");
    // c1 [0, 1) c2 [1, 2.5) c3 [2.5, 3.5) at 24 fps
    let paths = clips(&dir, &["c1.mp4", "c2.mp4", "c3.mp4"]);
    let mut s = Session::default();
    s.execute("creator.newVideo", json!({"aspect": "9:16", "paths": paths})).unwrap();
    s.execute("captions.newTrack", json!({"format": "Subtitle"})).unwrap();
    for (text, at, len) in [("one", 0.25, 0.5), ("two", 1.25, 0.5), ("three", 2.0, 0.5), ("four", 2.75, 0.5)] {
        s.execute("captions.add", json!({"text": text, "seconds": at, "durationSeconds": len})).unwrap();
    }
    // word timings on "three" (two words), to see that they move with it
    let three = s.active_sequence().unwrap().caption_tracks[0].captions.iter().find(|c| c.text == "three").unwrap().id;
    s.edit_sequence("words", |q, _, _| {
        let w = |text: &str, a: f64, b: f64| filmcraft_project::CaptionWord {
            text: text.into(),
            start: filmcraft_time::Tick::from_seconds_f64(a),
            duration: filmcraft_time::Tick::from_seconds_f64(b),
        };
        q.caption_tracks[0].words.insert(three, vec![w("thr", 0.0, 0.25), w("ee", 0.25, 0.25)]);
        Ok(())
    })
    .unwrap();
    let id_of = |s: &Session, name: &str| s.active_sequence().unwrap().video_tracks[0].items.iter().find(|i| i.name == name).unwrap().id.0;

    // trim the first half second off c2: its captions move 0.5 s earlier with the picture
    s.execute("playhead.set", json!({"seconds": 1.5})).unwrap();
    s.execute("timeline.select", json!({"clips": [id_of(&s, "c2.mp4")]})).unwrap();
    let undo0 = s.history.undo.len();
    s.execute("creator.trimToPlayhead", json!({"side": "start"})).unwrap();
    assert_eq!(s.history.undo.len(), undo0 + 1, "one undo step, captions included");
    let c = captions_of(&s);
    assert_eq!(c.len(), 4, "{c:?}");
    assert!(near(c[0].1, 0.25) && near(c[0].2, 0.75), "before the edit: unchanged {c:?}");
    // "two" was [1.25, 1.75): its first quarter second was cut, so it starts at the cut (1.0)
    assert!(near(c[1].1, 1.0) && near(c[1].2, 1.25), "{c:?}");
    assert!(near(c[2].1, 1.5) && near(c[2].2, 2.0), "{c:?}");
    assert!(near(c[3].1, 2.25) && near(c[3].2, 2.75), "{c:?}");
    let words = s.active_sequence().unwrap().caption_tracks[0].words.get(&three).cloned().unwrap();
    assert_eq!(words.len(), 2);
    assert!(near(words[1].start.seconds(), 0.25), "words stay relative to their caption");

    // delete c1: "one" (over c1 only) goes, everything else moves 1 s earlier
    s.execute("timeline.select", json!({"clips": [id_of(&s, "c1.mp4")]})).unwrap();
    s.execute("creator.delete", json!({})).unwrap();
    let c = captions_of(&s);
    assert_eq!(c.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), vec!["two", "three", "four"], "{c:?}");
    assert!(near(c[0].1, 0.0) && near(c[1].1, 0.5) && near(c[2].1, 1.25), "{c:?}");
    assert!(!s.active_sequence().unwrap().caption_tracks[0].captions.iter().any(|x| x.text == "one"));

    // move c3 (now [1, 2)) to the front: "four" goes with it
    s.execute("creator.moveClip", json!({"clip": id_of(&s, "c3.mp4"), "time": 0})).unwrap();
    assert_eq!(v1_names(&s), vec!["c3.mp4", "c2.mp4"]);
    let c = captions_of(&s);
    assert_eq!(c[0].0, "four", "{c:?}");
    assert!(near(c[0].1, 0.25) && near(c[0].2, 0.75), "{c:?}");
    assert!(near(c[1].1, 1.0) && near(c[2].1, 1.5), "{c:?}");
    s.active_sequence().unwrap().check().unwrap();

    // undo puts the captions back with the clips, all the way
    for _ in 0..3 {
        s.execute("edit.undo", json!({})).unwrap();
    }
    let c = captions_of(&s);
    assert_eq!(c.len(), 4);
    assert!(near(c[1].1, 1.25) && near(c[3].1, 2.75), "{c:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn caption_presets_style_the_track_in_one_undo_step() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("captions.newTrack", json!({"format": "Subtitle"})).unwrap();
    let list = s.execute("captions.presets", json!({})).unwrap();
    assert_eq!(list["presets"].as_array().unwrap().len(), 5);
    let undo0 = s.history.undo.len();
    s.execute("captions.applyPreset", json!({"preset": "boldPop", "position": "middle"})).unwrap();
    assert_eq!(s.history.undo.len(), undo0 + 1);
    let st = s.active_sequence().unwrap().caption_tracks[0].style.clone();
    assert!(st.uppercase && st.highlight.is_some() && st.anchor == filmcraft_project::CaptionAnchor::Middle);
    s.execute("captions.applyPreset", json!({"preset": "Word by Word"})).unwrap();
    let st = s.active_sequence().unwrap().caption_tracks[0].style.clone();
    assert_eq!(st.animation, filmcraft_project::CaptionAnimation::WordByWord);
    assert!(st.highlight.is_none(), "a preset replaces the whole look");
    assert!(st.margin > 0.2, "lower: above the platform's buttons and description");
    assert!(s.execute("captions.applyPreset", json!({"preset": "nope"})).is_err());
    assert!(s.execute("captions.applyPreset", json!({"preset": "clean", "position": "left"})).is_err());
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_sequence().unwrap().caption_tracks[0].style.uppercase);
}

#[test]
fn auto_captions_transcribe_caption_with_word_timings_and_style_in_one_step() {
    use filmcraft_project::{Transcript, Word};
    use filmcraft_speech::FixedTranscriber;
    use filmcraft_time::Tick;
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    // no speech model: a clear error and nothing changed
    let undo0 = s.history.undo.len();
    if s.transcriber.is_none() && !filmcraft_speech::available() {
        let e = s.execute("creator.autoCaptions", json!({})).unwrap_err().to_string();
        assert!(e.to_lowercase().contains("speech"), "{e}");
        assert_eq!(s.history.undo.len(), undo0);
    }
    let mut t = Transcript { language: "en".into(), ..Default::default() };
    for (i, w) in ["Make", "every", "second", "count", "today"].iter().enumerate() {
        let a = 0.2 + i as f64 * 0.4;
        t.words.push(Word::new(*w, Tick::from_seconds_f64(a), Tick::from_seconds_f64(a + 0.35)));
    }
    s.transcriber = Some(std::sync::Arc::new(FixedTranscriber { transcript: t, id: "fixed".into() }));
    let r = s.execute("creator.autoCaptions", json!({"preset": "karaoke"})).unwrap();
    assert_eq!(r["transcribed"], true, "{r}");
    assert_eq!(s.history.undo.len(), undo0 + 1, "one undo step");
    let q = s.active_sequence().unwrap();
    let tr = q.caption_tracks.last().unwrap();
    assert!(!tr.captions.is_empty());
    assert!(tr.style.highlight.is_some(), "karaoke: the spoken word is coloured");
    // every caption has its words, each inside the caption
    for c in &tr.captions {
        let words = tr.words.get(&c.id).unwrap();
        assert_eq!(words.len(), c.text.split_whitespace().count(), "{:?} {words:?}", c.text);
        assert!(words.iter().all(|w| w.start + w.duration <= c.duration + Tick(1)), "{words:?}");
        assert!(c.text.len() <= 24 + 8);
    }
    // saved and reopened: the words and the look are kept
    let dir = tmp_dir("fc_creator_autocap");
    let path = dir.join("cap.filmcraft").to_string_lossy().to_string();
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    let (words, style) = {
        let tr = s.active_sequence().unwrap().caption_tracks.last().unwrap();
        (tr.words.clone(), tr.style.clone())
    };
    let mut t2 = Session::default();
    t2.execute("file.open", json!({"path": path})).unwrap();
    let tr = t2.active_sequence().unwrap().caption_tracks.last().unwrap();
    assert_eq!(tr.words, words);
    assert_eq!(tr.style, style);
    let _ = std::fs::remove_dir_all(&dir);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_sequence().unwrap().caption_tracks.iter().all(|t| t.name != "Captions"));
}

#[test]
fn speed_ripples_the_main_track_and_captions_follow() {
    let dir = tmp_dir("fc_creator_speed");
    // s1 [0, 1) s2 [1, 2.5)
    let paths = clips(&dir, &["s1.mp4", "s2.mp4"]);
    let mut s = Session::default();
    s.execute("creator.newVideo", json!({"aspect": "9:16", "paths": paths})).unwrap();
    s.execute("captions.newTrack", json!({"format": "Subtitle"})).unwrap();
    s.execute("captions.add", json!({"text": "early", "seconds": 0.5, "durationSeconds": 0.25})).unwrap();
    s.execute("captions.add", json!({"text": "late", "seconds": 1.5, "durationSeconds": 0.5})).unwrap();
    let s1 = s.active_sequence().unwrap().video_tracks[0].items.iter().find(|i| i.name == "s1.mp4").unwrap().id.0;
    let undo0 = s.history.undo.len();
    s.execute("creator.speed", json!({"clips": [s1], "speed": 200})).unwrap();
    assert_eq!(s.history.undo.len(), undo0 + 1);
    let mut items: Vec<_> = s.active_sequence().unwrap().video_tracks[0].items.iter().map(|i| (i.start.seconds(), i.end().seconds())).collect();
    items.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert!(near(items[0].1, 0.5) && near(items[1].0, 0.5), "s1 is half as long and s2 follows: {items:?}");
    let c = captions_of(&s);
    // "early" was 0.5 s into s1's footage → 0.25 s at double speed, 0.125 s long
    assert!(near(c[0].1, 0.25) && near(c[0].2, 0.375), "{c:?}");
    assert!(near(c[1].1, 1.0) && near(c[1].2, 1.5), "{c:?}");
    assert!(s.execute("creator.speed", json!({"clips": [s1], "speed": 5})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn split_cuts_the_main_clip_and_its_sound_but_not_the_music() {
    let dir = tmp_dir("fc_creator_split");
    let paths = clips(&dir, &["p1.mp4", "p2.mp4"]);
    let music = dir.join("song.mp4");
    make_movie(&music, filmcraft_media::DemoScene::Plasma, 64, 112, 200);
    let mut s = Session::default();
    s.execute("creator.newVideo", json!({"aspect": "9:16", "paths": paths})).unwrap();
    let song = s.execute("file.import", json!({"paths": [music.to_string_lossy()]})).unwrap()["items"][0].as_u64().unwrap();
    s.execute("timeline.place", json!({"item": song, "audioTrack": "A2", "time": 0, "video": false})).unwrap();
    s.execute("playhead.set", json!({"seconds": 0.5})).unwrap();
    s.state.selection.clear();
    let undo0 = s.history.undo.len();
    s.execute("creator.split", json!({})).unwrap();
    assert_eq!(s.history.undo.len(), undo0 + 1);
    let q = s.active_sequence().unwrap();
    assert_eq!(q.video_tracks[0].items.len(), 3, "the clip under the playhead is cut");
    assert_eq!(q.audio_tracks[0].items.len(), 3, "and its sound");
    assert_eq!(q.audio_tracks[1].items.len(), 1, "the music is not cut");
    let _ = std::fs::remove_dir_all(&dir);
}
