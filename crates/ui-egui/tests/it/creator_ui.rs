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
