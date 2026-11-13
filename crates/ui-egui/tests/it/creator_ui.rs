//! Creator Mode (M3A), headless: the Start screen's New Video in a shape → the file chooser →
//! the Creator workspace with the clips in file-name order; files dropped while editing are
//! added at the end; the toolbar's Split and Delete; the Export video dialog's file name; Pro and
//! back on the same timeline. Each step goes through the interface (clicks on automation ids,
//! the control channel's stand-ins for the file chooser and a file manager drop).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_engine::project::{ItemKind, Label, MediaClip, MediaRef, Project, SequenceSettings, TrackKind};
use filmcraft_media::generators::GeneratorSource;
use filmcraft_media::{DemoScene, Generator, MediaSource};
use filmcraft_time::{FrameRate, Tick, TimeRange};
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::{ControlRequest, Outcome};
use serde_json::{Value, json};

fn tmp_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let d = std::env::temp_dir().join(format!("filmcraft-ui-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A short 24 fps ProRes movie (our own encoder).
fn make_movie(path: &Path, frames: i64) {
    let (w, h, rate) = (64, 112, FrameRate::FPS_24);
    let dur = rate.tick_of(frames);
    let src = GeneratorSource::new(Generator::Demo(DemoScene::Plasma), w, h, rate, dur);
    let clip = MediaClip {
        media: MediaRef::Generator(Generator::Demo(DemoScene::Plasma)),
        info: src.info().clone(),
        interpret: Default::default(),
        mark_in: None,
        mark_out: None,
        markers: vec![],
        offline: false,
        proxy: None,
        identity: None,
    };
    let mut p = Project::new("fixture");
    let item = p.add_item("scene", Label::Iris, ItemKind::Media(Box::new(clip)), None);
    let seq = p.new_sequence("s", SequenceSettings { width: w, height: h, frame_rate: rate, ..Default::default() }, 1, 0, None);
    let v = p.make_track_item(item, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, dur), rate).unwrap();
    p.sequence_mut(seq).unwrap().video_tracks[0].items.push(v);
    let settings = filmcraft_engine::export::ExportSettings {
        format: filmcraft_engine::export::Format::ProRes,
        path: path.to_string_lossy().into_owned(),
        include_audio: false,
        ..Default::default()
    };
    let shared: filmcraft_media::SharedSource = Arc::new(src);
    let provider = move |id| (id == item).then(|| shared.clone());
    filmcraft_engine::export::export(&Arc::new(p), seq, &settings, &provider, &Default::default()).unwrap();
}

fn step(h: &mut Harness<'static, FilmcraftApp>) {
    let ctx = h.ctx.clone();
    let mut raw = std::mem::take(h.input_mut());
    eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut raw);
    *h.input_mut() = raw;
    ctx.request_repaint();
    h.step();
}

fn steps(h: &mut Harness<'static, FilmcraftApp>, n: usize) {
    for _ in 0..n {
        step(h);
    }
}

fn control(h: &mut Harness<'static, FilmcraftApp>, method: &str, params: Value) -> Value {
    let (request, _) = ControlRequest::new(method, params);
    let ctx = h.ctx.clone();
    match filmcraft_ui_egui::control::handle(h.state_mut(), &ctx, &request) {
        Outcome::Done(v) => v,
        _ => Value::Null,
    }
}

fn click(h: &mut Harness<'static, FilmcraftApp>, id: &str) {
    steps(h, 2);
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no element {id}")).rect;
    let p = egui::pos2(e[0] + e[2] / 2.0, e[1] + e[3] / 2.0);
    h.input_mut().events.push(egui::Event::PointerMoved(p));
    h.input_mut().events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    step(h);
    h.input_mut().events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    steps(h, 3);
}

fn v1(h: &Harness<'static, FilmcraftApp>) -> Vec<(Tick, Tick, String)> {
    let q = h.state().session.active_sequence().unwrap();
    let mut v: Vec<_> = q.video_tracks[0].items.iter().map(|i| (i.start, i.duration, i.name.clone())).collect();
    v.sort_by_key(|x| x.0);
    v
}

#[test]
fn a_beginner_makes_a_vertical_video_from_the_start_screen() {
    let dir = tmp_dir("creator");
    let names = ["take 10.mov", "take 2.mov", "take 1.mov"];
    let paths: Vec<String> = names
        .iter()
        .map(|n| {
            let p = dir.join(n);
            make_movie(&p, 24);
            p.to_string_lossy().to_string()
        })
        .collect();
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_eframe(|_cc| {
        let mut app = FilmcraftApp::new(Session::default());
        app.ui.mode = filmcraft_ui_egui::state::Mode::Import;
        app
    });
    steps(&mut h, 4);
    // the Start screen: four shapes
    for a in ["9x16", "16x9", "1x1", "4x5"] {
        assert!(h.state().auto.find(&format!("start.aspect.{a}")).is_some(), "start.aspect.{a}");
    }
    // New 9:16 video: the chooser answers with the files in the file manager's order
    control(&mut h, "ui.dialog.answer", json!({ "paths": paths }));
    click(&mut h, "start.aspect.9x16");
    let app = h.state();
    assert_eq!(app.ui.mode, filmcraft_ui_egui::state::Mode::Edit, "straight to editing");
    assert_eq!(app.ui.workspace, "Creator");
    let q = app.session.active_sequence().unwrap();
    assert_eq!((q.settings.width, q.settings.height), (1080, 1920));
    assert_eq!(v1(&h).iter().map(|x| x.2.as_str()).collect::<Vec<_>>(), ["take 1.mov", "take 2.mov", "take 10.mov"], "file-name order");
    steps(&mut h, 3);
    for id in
        ["creator.toolbar.split", "creator.toolbar.trimStart", "creator.toolbar.delete", "creator.toolbar.export", "creator.assets.addMedia", "header.proMode"]
    {
        assert!(h.state().auto.find(id).is_some(), "{id}");
    }
    // a file manager drop while editing: added at the end
    let more = dir.join("take 0.mov");
    make_movie(&more, 12);
    control(&mut h, "ui.dropFiles", json!({"paths": [more.to_string_lossy()]}));
    assert_eq!(v1(&h).last().map(|x| x.2.clone()).as_deref(), Some("take 0.mov"));
    // Split at the playhead, then Delete the second half: no gap
    h.state_mut().session.execute("playhead.set", json!({"seconds": 0.5})).unwrap();
    click(&mut h, "creator.toolbar.split");
    assert_eq!(v1(&h).len(), 5);
    let second =
        h.state().session.active_sequence().unwrap().video_tracks[0].items.iter().find(|i| i.start > Tick::ZERO && i.name == "take 1.mov").unwrap().id.0;
    h.state_mut().session.execute("timeline.select", json!({ "clips": [second] })).unwrap();
    click(&mut h, "creator.toolbar.delete");
    let clips = v1(&h);
    assert_eq!(clips.len(), 4);
    assert!(clips.windows(2).all(|w| w[0].0 + w[0].1 == w[1].0), "no gaps");
    // Export video: a typed `.mp4` is not doubled
    click(&mut h, "creator.toolbar.export");
    assert!(h.state().ui.creator.export.open);
    assert_eq!(h.state().ui.creator.export.preset, "tiktok");
    assert_eq!(h.state().ui.creator.export.name, "My Video");
    h.state_mut().ui.creator.export.name = "Launch.mp4".into();
    steps(&mut h, 3);
    assert_eq!(h.state().ui.creator.export.name, "Launch");
    click(&mut h, "creator.export.close");
    // Pro and back: the same timeline
    let before = v1(&h);
    click(&mut h, "header.proMode");
    assert_eq!(h.state().ui.workspace, "Editing");
    assert_eq!(v1(&h), before);
    click(&mut h, "header.creatorMode");
    assert_eq!(h.state().ui.workspace, "Creator");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A heavier clip than `make_movie`'s (2160×1200, more than the 2.5 MP "4K" threshold).
fn make_big_movie(path: &Path) {
    let (w, h, rate) = (2160, 1200, FrameRate::FPS_24);
    let dur = rate.tick_of(6);
    let src = GeneratorSource::new(Generator::Demo(DemoScene::Plasma), w, h, rate, dur);
    let clip = MediaClip {
        media: MediaRef::Generator(Generator::Demo(DemoScene::Plasma)),
        info: src.info().clone(),
        interpret: Default::default(),
        mark_in: None,
        mark_out: None,
        markers: vec![],
        offline: false,
        proxy: None,
        identity: None,
    };
    let mut p = Project::new("fixture");
    let item = p.add_item("scene", Label::Iris, ItemKind::Media(Box::new(clip)), None);
    let seq = p.new_sequence("s", SequenceSettings { width: w, height: h, frame_rate: rate, ..Default::default() }, 1, 0, None);
    let v = p.make_track_item(item, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, dur), rate).unwrap();
    p.sequence_mut(seq).unwrap().video_tracks[0].items.push(v);
    let settings = filmcraft_engine::export::ExportSettings {
        format: filmcraft_engine::export::Format::ProRes,
        path: path.to_string_lossy().into_owned(),
        include_audio: false,
        ..Default::default()
    };
    let shared: filmcraft_media::SharedSource = Arc::new(src);
    let provider = move |id| (id == item).then(|| shared.clone());
    filmcraft_engine::export::export(&Arc::new(p), seq, &settings, &provider, &Default::default()).unwrap();
}

/// M3C: the memory plan is in force and reported; 4K clips get the editing-copies card, and its
/// button makes the copies the monitors then play.
#[test]
fn creator_offers_editing_copies_for_4k_and_reports_the_memory_plan() {
    let dir = tmp_dir("creator4k");
    let big = dir.join("big.mov");
    make_big_movie(&big);
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_eframe(|_cc| FilmcraftApp::new(Session::default()));
    steps(&mut h, 4);
    let plan = h.state().memory_plan.expect("a memory plan is applied with the settings");
    assert_eq!(filmcraft_codecs::gop::limits(), (plan.decoded_frames, plan.max_decoders, plan.hard_decoders));
    let stats = control(&mut h, "perf.stats", json!({}));
    assert!(stats["result"]["memory"]["decodedFramesMB"].as_f64().is_some_and(|m| m > 0.0), "{stats}");
    control(&mut h, "ui.dialog.answer", json!({"paths": [big.to_string_lossy()]}));
    h.state_mut().ui.mode = filmcraft_ui_egui::state::Mode::Import;
    steps(&mut h, 2);
    click(&mut h, "start.aspect.9x16");
    steps(&mut h, 3);
    assert!(h.state().auto.find("creator.assets.smoothEditing").is_some(), "the 4K card offers editing copies");
    click(&mut h, "creator.assets.smoothEditing");
    // the copy is made in the background; wait for the job
    let t0 = std::time::Instant::now();
    while h.state().session.jobs.iter().any(|j| !j.progress.finished.load(std::sync::atomic::Ordering::Relaxed)) {
        assert!(t0.elapsed() < std::time::Duration::from_secs(120), "proxy job");
        std::thread::sleep(std::time::Duration::from_millis(50));
        steps(&mut h, 1);
    }
    steps(&mut h, 4);
    let st = h.state_mut().session.execute("creator.smoothEditingStatus", json!({})).unwrap();
    assert_eq!((st["withCopies"].as_u64(), st["using"].as_bool()), (Some(1), Some(true)), "{st}");
    assert!(h.state().auto.find("creator.assets.smoothEditingOff").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A 16-bit mono WAV of clicks at 120 bpm (music for the tests).
fn click_track(path: &Path, seconds: f64) {
    let sr = 44_100u32;
    let n = (seconds * sr as f64) as usize;
    let mut pcm = Vec::with_capacity(n * 2);
    for i in 0..n {
        let k = i % (sr as usize / 2);
        let v = if k < 600 { ((k as f64 * 0.3).sin() * (1.0 - k as f64 / 600.0) * 28_000.0) as i16 } else { 0 };
        pcm.extend_from_slice(&v.to_le_bytes());
    }
    let mut w = Vec::new();
    for part in [&b"RIFF"[..], &(36 + pcm.len() as u32).to_le_bytes(), b"WAVEfmt ", &16u32.to_le_bytes(), &1u16.to_le_bytes(), &1u16.to_le_bytes()] {
        w.extend_from_slice(part);
    }
    for part in [&sr.to_le_bytes()[..], &(sr * 2).to_le_bytes(), &2u16.to_le_bytes(), &16u16.to_le_bytes(), b"data", &(pcm.len() as u32).to_le_bytes(), &pcm] {
        w.extend_from_slice(part);
    }
    std::fs::write(path, w).unwrap();
}

fn v1_ids(h: &Harness<'static, FilmcraftApp>) -> Vec<u64> {
    h.state().session.active_sequence().unwrap().video_tracks[0].items.iter().map(|i| i.id.0).collect()
}

#[test]
fn short_form_music_stays_visible_assets_drag_to_the_timeline_and_captions_get_a_look() {
    let dir = tmp_dir("creator-m3d");
    let paths: Vec<String> = ["a.mov", "b.mov", "c.mov"]
        .iter()
        .map(|n| {
            let p = dir.join(n);
            make_movie(&p, 24);
            p.to_string_lossy().to_string()
        })
        .collect();
    let song = dir.join("song.wav");
    click_track(&song, 6.0);
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_eframe(|_cc| {
        let mut app = FilmcraftApp::new(Session::default());
        app.ui.mode = filmcraft_ui_egui::state::Mode::Import;
        app
    });
    steps(&mut h, 4);
    control(&mut h, "ui.dialog.answer", json!({ "paths": paths }));
    click(&mut h, "start.aspect.9x16");
    assert!(h.state().session.state.snapping, "a magnetic timeline in Creator Mode");
    // music: Assets ▸ Music ▸ Add music…
    click(&mut h, "creator.assets.tab.music");
    control(&mut h, "ui.dialog.answer", json!({ "paths": [song.to_string_lossy()] }));
    click(&mut h, "creator.assets.addMusic");
    steps(&mut h, 3);
    let (music, track_index) = {
        let q = h.state().session.active_sequence().unwrap();
        let (i, tr) = q.audio_tracks.iter().enumerate().find(|(_, t)| t.items.iter().any(|it| it.link.is_none())).expect("music placed");
        (tr.items[0].id.0, i)
    };
    assert_eq!(track_index, 1, "music on A2, under the clips' sound, not on a new track below the empty ones");
    // the music clip is on screen, inside the timeline's tracks (M3A: it could be scrolled out of sight)
    let r = h.state().auto.find(&format!("timeline.clip.{music}")).expect("music clip drawn").rect;
    let content = h.state().tl.layout.as_ref().unwrap().content;
    assert!(r[3] >= 18.0, "a readable music row: {r:?}");
    assert!(r[1] >= content.min.y && r[1] + r[3] <= content.max.y, "music visible: {r:?} in {content:?}");
    for c in v1_ids(&h) {
        let r = h.state().auto.find(&format!("timeline.clip.{c}")).unwrap_or_else(|| panic!("V1 clip {c} drawn")).rect;
        assert!(r[3] >= 18.0 && r[1] >= content.min.y && r[1] + r[3] <= content.max.y, "V1 visible: {r:?}");
    }
    // sound tools: fade and ducking, one undo step each
    let undo0 = h.state().session.history.undo.len();
    click(&mut h, "creator.assets.fade");
    click(&mut h, "creator.assets.duck");
    assert_eq!(h.state().session.history.undo.len(), undo0 + 2, "{}", h.state().ui.status);
    // drag a clip from Assets ▸ Media to the end of the timeline
    click(&mut h, "creator.assets.tab.media");
    let before = v1(&h).len();
    // just after the last clip, on its row
    let last = h.state().session.active_sequence().unwrap().video_tracks[0].items.iter().max_by_key(|i| i.start).unwrap().id.0;
    let r = h.state().auto.find(&format!("timeline.clip.{last}")).unwrap().rect;
    let content = h.state().tl.layout.as_ref().unwrap().content;
    let to = egui::pos2((r[0] + r[2] + 30.0).min(content.max.x - 5.0), r[1] + r[3] / 2.0);
    control(&mut h, "ui.drag", json!({"from": {"id": "creator.assets.media.0.drag"}, "to": {"x": to.x, "y": to.y}, "steps": 16}));
    steps(&mut h, 40);
    assert_eq!(v1(&h).len(), before + 1, "dropped on the timeline: {}", h.state().ui.status);
    // captions: add one, then a look and a position from the Captions tab
    click(&mut h, "creator.assets.tab.captions");
    h.state_mut().session.execute("captions.add", json!({"text": "hello there", "seconds": 0.2, "durationSeconds": 1.0})).unwrap();
    click(&mut h, "creator.assets.captionPreset.karaoke");
    click(&mut h, "creator.assets.captionPosition.upper");
    let st = h.state().session.active_sequence().unwrap().caption_tracks[0].style.clone();
    assert!(st.highlight.is_some(), "karaoke look applied");
    assert_eq!(st.anchor, filmcraft_engine::project::CaptionAnchor::Top);
    assert!(h.state().auto.find("creator.assets.caption.0.text").is_some(), "the caption is listed for editing");
    assert!(h.state().auto.find("creator.assets.exportSrt").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}
