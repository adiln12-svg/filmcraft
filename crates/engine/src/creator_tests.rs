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
