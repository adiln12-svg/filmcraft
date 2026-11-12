//! M3B co-editing through the interface and the control channel: Claude's edits show at once and
//! undo from the Changes panel, the toolbar's Protect, refusals, stale commands, a request the
//! person allows in the app, and injected clicks counted as Claude's.

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
use filmcraft_ui_egui::dock::PanelKind;
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

fn v1(h: &Harness<'static, FilmcraftApp>) -> Vec<(Tick, Tick, String, u64)> {
    let q = h.state().session.active_sequence().unwrap();
    let mut v: Vec<_> = q.video_tracks[0].items.iter().map(|i| (i.start, i.duration, i.name.clone(), i.id.0)).collect();
    v.sort_by_key(|x| x.0);
    v
}

fn ctl(h: &mut Harness<'static, FilmcraftApp>, method: &str, params: Value) -> Value {
    let v = control(h, method, params);
    steps(h, 2);
    v
}

fn exec(h: &mut Harness<'static, FilmcraftApp>, command: &str, params: Value) -> Value {
    ctl(h, "engine.execute", json!({"command": command, "params": params}))
}

fn harness(dir: &Path) -> Harness<'static, FilmcraftApp> {
    let paths: Vec<String> = ["a.mov", "b.mov", "c.mov"]
        .iter()
        .map(|n| {
            let p = dir.join(n);
            make_movie(&p, 48);
            p.to_string_lossy().to_string()
        })
        .collect();
    let data = dir.join("data");
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_eframe(move |_cc| {
        let mut s = Session::default();
        s.library_data_dir = Some(data.clone());
        s.execute("creator.newVideo", json!({"aspect": "9:16", "paths": paths})).unwrap();
        let mut app = FilmcraftApp::new(s);
        app.set_workspace("Creator");
        app
    });
    steps(&mut h, 4);
    h
}

const T: i64 = 254_016_000_000;

#[test]
fn the_person_and_claude_edit_one_project_through_the_interface_and_the_control_channel() {
    let dir = tmp_dir("coedit");
    let mut h = harness(&dir);
    let ids: Vec<u64> = v1(&h).iter().map(|c| c.3).collect();
    // Claude edits through the control channel: the person sees it at once, labelled
    let r = exec(&mut h, "creator.trim", json!({"clip": ids[2], "edge": "out", "delta": -T / 2}));
    assert_eq!(r["ok"], true, "{r}");
    let rev = r["projectRevision"].as_u64().unwrap();
    assert_eq!(v1(&h)[2].1, Tick(2 * T - T / 2));
    assert!(h.state().ui.status.starts_with("Claude: Trim Clip"), "{}", h.state().ui.status);
    assert_eq!(h.state().session.history.undo.last().map(|u| u.0.clone()).as_deref(), Some("Claude: Trim Clip"));
    // it is listed in the Changes panel with an Undo button that undoes it
    h.state_mut().show_panel(PanelKind::Changes);
    steps(&mut h, 3);
    assert!(h.state().auto.find("changes.claude.0").is_some());
    click(&mut h, "changes.claude.undo");
    assert_eq!(v1(&h)[2].1, Tick(2 * T));
    click(&mut h, "creator.toolbar.redo");
    assert_eq!(v1(&h)[2].1, Tick(2 * T - T / 2));
    // the person protects clip 2 with the toolbar
    h.state_mut().session.execute("timeline.select", json!({ "clips": [ids[1]] })).unwrap();
    click(&mut h, "creator.toolbar.protect");
    assert!(h.state().session.project.protection.clips.contains(&filmcraft_engine::project::ClipId(ids[1])));
    // Claude's edit of it is refused with the reason; nothing changes
    let before = h.state().session.project.clone();
    let r = exec(&mut h, "lumetri.applyPreset", json!({"name": "Golden Dusk", "clips": [ids[1]]}));
    assert_eq!(r["ok"], false);
    assert!(r["error"].as_str().unwrap().starts_with("PROTECTED"), "{r}");
    assert_eq!(*h.state().session.project, *before);
    // a stale command is refused
    let now = r["projectRevision"].as_u64().unwrap();
    assert!(now > rev);
    let r =
        ctl(&mut h, "engine.execute", json!({"command": "creator.trim", "params": {"clip": ids[0], "edge": "out", "delta": -T / 4}, "expectRevision": rev}));
    assert!(r["error"].as_str().unwrap().starts_with("CONFLICT"), "{r}");
    // Claude asks; the person allows it in the window; Claude's next try goes through
    let req =
        exec(&mut h, "agent.requestAuthorization", json!({"elements": [ids[1]], "reason": "warm up the second shot"}))["result"]["request"].as_u64().unwrap();
    steps(&mut h, 3);
    assert!(h.state().auto.find("claudeRequest.window").is_some(), "the request is shown");
    // Claude cannot answer it itself
    assert_eq!(exec(&mut h, "agent.authorize", json!({"request": req, "allow": true}))["ok"], false);
    click(&mut h, "claudeRequest.allow");
    let st = exec(&mut h, "agent.authorizationStatus", json!({ "request": req }));
    let token = st["result"]["authorization"].as_str().unwrap().to_string();
    let r =
        ctl(&mut h, "engine.execute", json!({"command": "lumetri.applyPreset", "params": {"name": "Golden Dusk", "clips": [ids[1]]}, "authorization": token}));
    assert_eq!(r["ok"], true, "{r}");
    // clicks injected by the control channel are Claude's in the app (--control): they cannot
    // delete the protected clip either
    h.state_mut().synthetic_input_is_agent = true;
    h.state_mut().session.execute("timeline.select", json!({ "clips": [ids[1]] })).unwrap();
    let before = h.state().session.project.clone();
    ctl(&mut h, "ui.click", json!({"id": "creator.toolbar.delete"}));
    steps(&mut h, 4);
    assert_eq!(*h.state().session.project, *before, "an injected click cannot remove protected work");
    // the person's own click does
    h.state_mut().synthetic_input_is_agent = false;
    ctl(&mut h, "ui.click", json!({"id": "creator.toolbar.delete"}));
    steps(&mut h, 4);
    assert_eq!(v1(&h).len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
