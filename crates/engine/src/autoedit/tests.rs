//! The autonomous-editing commands on the demo project.

use filmcraft_time::Tick;
use serde_json::json;

use crate::Session;
use crate::commands::secs;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn first_media_item(s: &Session) -> filmcraft_project::ItemId {
    s.active_sequence().unwrap().video_tracks[0].items[0].item
}

#[test]
fn commands_are_registered_for_agents_only() {
    for id in ["analyze.beats", "analyze.takes", "script.align", "qc.check", "qc.filmstrip"] {
        let c = crate::commands::find(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(c.menu.is_empty(), "{id} is an agent command");
    }
}

#[test]
fn qc_check_reports_metrics_and_catches_a_flash_frame() {
    let mut s = demo();
    let r = s.execute("qc.check", json!({"spec": {"loudness": null}, "sampleEvery": 2.0})).unwrap();
    assert!(r["metrics"]["cuts"].as_u64().unwrap() >= 4, "{r}");
    assert!(r["metrics"]["durationSeconds"].as_f64().unwrap() > 20.0);
    let flash_before = r["issues"].as_array().unwrap().iter().filter(|i| i["check"] == "flashFrame").count();
    // razor two frames apart: a 2-frame shot appears
    let rate = s.sequence_rate();
    let t = rate.tick_of(5 * 24);
    s.execute("sequence.addEdit", json!({"time": t.0})).unwrap();
    s.execute("sequence.addEdit", json!({"time": (t + rate.tick_of(2)).0})).unwrap();
    let r = s.execute("qc.check", json!({"spec": {"loudness": null, "blackFrames": null}})).unwrap();
    let flashes: Vec<_> = r["issues"].as_array().unwrap().iter().filter(|i| i["check"] == "flashFrame").collect();
    assert!(flashes.len() > flash_before, "{r}");
    assert_eq!(r["pass"], false);
    // a spec that is not an object is a parameter error
    assert!(s.execute("qc.check", json!({"spec": 3})).is_err());
}

#[test]
fn qc_check_measures_loudness_and_duration_rules() {
    let mut s = demo();
    let r = s.execute("qc.check", json!({"spec": {"duration": {"max": 1}, "blackFrames": null, "audioClicks": null, "captions": null}})).unwrap();
    assert!(r["metrics"]["integratedLufs"].is_number(), "the demo has a score: {r}");
    let checks: Vec<&str> = r["issues"].as_array().unwrap().iter().filter_map(|i| i["check"].as_str()).collect();
    assert!(checks.contains(&"duration"), "{r}");
}

#[test]
fn filmstrip_renders_rows_around_cuts() {
    let mut s = demo();
    let r = s.execute("qc.filmstrip", json!({"limit": 2, "tileWidth": 64})).unwrap();
    assert_eq!(r["rows"].as_array().unwrap().len(), 2, "{r}");
    let png = filmcraft_project::gtemplate::base64_decode(r["pngBase64"].as_str().unwrap()).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    let r = s.execute("qc.filmstrip", json!({"times": [1.0, -5.0, 1e12], "tileWidth": 32})).unwrap();
    assert_eq!(r["rows"].as_array().unwrap().len(), 3, "out-of-range times are clamped");
    assert!(s.execute("qc.filmstrip", json!({"times": []})).is_err());
    let dir = crate::media_test_util::tmp_dir("strip");
    let path = dir.join("cuts.png").to_string_lossy().into_owned();
    s.execute("qc.filmstrip", json!({"limit": 1, "tileWidth": 48, "path": path})).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 100);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn takes_are_scored_and_ranked() {
    let mut s = demo();
    let clips: Vec<u64> = s.active_sequence().unwrap().video_tracks[0].items.iter().map(|i| i.id.0).take(3).collect();
    let r = s.execute("analyze.takes", json!({"clips": clips, "every": 1.0, "width": 64, "detail": true})).unwrap();
    let takes = r["takes"].as_array().unwrap();
    assert_eq!(takes.len(), 3, "{r}");
    let scores: Vec<f64> = takes.iter().map(|t| t["score"].as_f64().unwrap()).collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "ranked best first: {scores:?}");
    assert!(!takes[0]["detail"].as_array().unwrap().is_empty());
    let item = first_media_item(&s);
    let r = s.execute("analyze.takes", json!({"items": [item.0], "every": 2.0, "width": 32})).unwrap();
    assert_eq!(r["takes"][0]["item"], item.0);
    assert!(s.execute("analyze.takes", json!({"items": [987_654]})).is_err());
    assert!(s.execute("analyze.takes", json!({"clips": [987_654]})).is_err());
    s.execute("timeline.select", json!({"clips": []})).ok();
    s.state.selection.clear();
    assert!(s.execute("analyze.takes", json!({})).is_err());
}

#[test]
fn beats_of_the_demo_score_and_markers_undo() {
    let mut s = demo();
    let r = s.execute("analyze.beats", json!({"end": 12.0})).unwrap();
    assert!(r["bpm"].is_number() && r["beats"].is_array(), "{r}");
    let n = r["beats"].as_array().unwrap().len();
    let before = s.active_sequence().unwrap().markers.len();
    let r = s.execute("analyze.beats", json!({"end": 12.0, "markers": true})).unwrap();
    let added = r["markersAdded"].as_u64().unwrap() as usize;
    assert!(added <= n);
    assert_eq!(s.active_sequence().unwrap().markers.len(), before + added);
    if added > 0 {
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.active_sequence().unwrap().markers.len(), before);
    }
    assert!(s.execute("analyze.beats", json!({"start": 50.0, "end": 10.0})).is_err());
    assert!(s.execute("analyze.beats", json!({"item": 987_654})).is_err());
    let _ = s.execute("analyze.beats", json!({"minBpm": f64::MAX, "maxBpm": -1, "start": -4}));
}

#[test]
fn script_lines_find_their_takes() {
    let mut s = demo();
    let item = first_media_item(&s);
    assert!(s.execute("script.align", json!({"script": "hello"})).is_err(), "no transcripts yet");
    let w = |text: &str, a: f64| json!({"text": text, "start": secs(a).0, "end": secs(a + 0.3).0});
    let words =
        vec![w("Welcome", 0.0), w("to", 0.4), w("the", 0.8), w("um", 1.2), w("show", 1.6), w("Welcome", 4.0), w("to", 4.4), w("the", 4.8), w("show", 5.2)];
    s.execute("transcript.set", json!({"item": item.0, "transcript": {"language": "en", "words": words}})).unwrap();
    let r = s.execute("script.align", json!({"script": "Welcome to the show.\n\nNot in the footage at all", "top": 2})).unwrap();
    let lines = r["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    let best = &lines[0]["takes"][0];
    assert_eq!(best["item"], item.0);
    assert!((best["start"].as_f64().unwrap() - 4.0).abs() < 1e-3, "clean take first: {r}");
    assert_eq!(best["fillers"], 0);
    assert_eq!(lines[0]["takes"].as_array().unwrap().len(), 2);
    assert_eq!(r["missing"], json!([1]));
    assert_eq!(Tick(best["startTicks"].as_i64().unwrap()), secs(4.0));
    assert!(s.execute("script.align", json!({"script": "  \n "})).is_err());
    assert!(s.execute("script.align", json!({})).is_err());
}

// ---------------------------------------------------------------------- Milestone 1 commands

const ARCA: &str =
    "Full Narration (V1):\n- [0–3s Hook] \"welcome to the show.\"\n- [3–8s CTA] \"see you next time.\" (re-hook · RP-002)\nEngagement objective: Save";

#[test]
fn script_parse_reads_arca_scripts() {
    let mut s = demo();
    let r = s.execute("script.parse", json!({"script": ARCA})).unwrap();
    assert_eq!(r["lines"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(r["lines"][0]["beat"], "Hook");
    assert_eq!(r["lines"][1]["text"], "see you next time.");
    assert_eq!(r["skipped"].as_array().unwrap().len(), 2);
    assert!(s.execute("script.parse", json!({})).is_err());
}

#[test]
fn assemble_blocks_and_never_touches_the_timeline() {
    let mut s = demo();
    let before = s.project.clone();
    // no transcripts at all
    let r = s.execute("script.assemble", json!({"script": ARCA})).unwrap();
    assert_eq!(r["state"], "BLOCKED", "{r}");
    // a script with no dialogue
    let r = s.execute("script.assemble", json!({"script": "# heading\nNotes: none"})).unwrap();
    assert_eq!(r["state"], "BLOCKED");
    // one transcribed item that says only the first line: line 2 blocks, nothing is placed
    let item = first_media_item(&s);
    let w = |text: &str, a: f64| json!({"text": text, "start": secs(a).0, "end": secs(a + 0.3).0});
    let words = vec![w("welcome", 1.0), w("to", 1.4), w("the", 1.8), w("show.", 2.2)];
    s.execute("transcript.set", json!({"item": item.0, "transcript": {"language": "en", "words": words}})).unwrap();
    let after_set = s.project.clone();
    let r = s.execute("script.assemble", json!({"script": ARCA, "sequence": {"name": "x", "width": 1080, "height": 1920, "fps": 30}})).unwrap();
    assert_eq!(r["state"], "BLOCKED", "{r}");
    let blocked: Vec<u64> = r["blocked"].as_array().unwrap().iter().filter_map(|b| b["line"].as_u64()).collect();
    assert!(blocked.contains(&2), "{r}");
    assert_eq!(s.project.sequences().count(), after_set.sequences().count(), "no sequence was created");
    assert_ne!(*before, *after_set);
    let rep = s.execute("autoedit.report", json!({})).unwrap();
    assert_eq!(rep["state"], "BLOCKED");
    assert!(!rep["blocked"].as_array().unwrap().is_empty());
    // hostile parameters
    assert!(s.execute("script.assemble", json!({})).is_err());
    let r = s.execute("script.assemble", json!({"script": ARCA, "items": ["x", -1, 1e300], "minScore": f64::NAN, "padIn": -5, "maxPause": 1e9})).unwrap();
    assert!(r["state"] == "BLOCKED" || r["state"] == "ASSEMBLED");
}

#[test]
fn report_needs_a_checked_export_and_notes_are_validated() {
    let mut s = demo();
    let r = s.execute("autoedit.report", json!({})).unwrap();
    assert_eq!(r["state"], "FAILED", "nothing verified is not complete: {r}");
    s.execute("autoedit.note", json!({"kind": "correction", "detail": {"what": "test"}})).unwrap();
    assert!(s.execute("autoedit.note", json!({"kind": "rewrite the script"})).is_err());
    let r = s.execute("autoedit.report", json!({})).unwrap();
    assert_eq!(r["corrections"].as_array().unwrap().len(), 1);
}

#[test]
fn qc_render_checks_the_exported_file() {
    let mut s = demo();
    // a file that is not there
    let r = s.execute("qc.render", json!({"path": "/nonexistent/x.mp4"})).unwrap();
    assert_eq!(r["state"], "FAILED", "{r}");
    assert!(r["failures"].as_array().unwrap().iter().any(|f| f["check"] == "playable"));
    // a short real export of the demo
    let dir = crate::media_test_util::tmp_dir("qc-render");
    let path = dir.join("demo.mov").to_string_lossy().into_owned();
    s.execute("sequence.setInPoint", json!({"seconds": 0.0})).ok();
    s.execute(
        "file.exportMedia",
        json!({"path": path, "format": "prores", "range": "custom", "startSeconds": 0, "endSeconds": 1, "wait": true, "scale": 0.25}),
    )
    .unwrap();
    let r = s
        .execute("qc.render", json!({"path": path, "transcribe": false, "samples": 2, "spec": {"loudness": null, "duration": {"min": 0.5, "max": 2}}}))
        .unwrap();
    let checks = r["checks"].as_array().unwrap();
    assert!(checks.iter().any(|c| c["check"] == "playable" && c["status"] == "pass"), "{r}");
    // the export is a 1 s range of a 26 s edit: the length check catches it
    assert!(checks.iter().any(|c| c["check"] == "duration" && c["status"] == "fail"), "{r}");
    assert_eq!(r["state"], "FAILED");
    let rep = s.execute("autoedit.report", json!({})).unwrap();
    assert_eq!(rep["state"], "FAILED");
    assert!(s.execute("qc.render", json!({})).is_err());
    let _ = s.execute(
        "qc.render",
        json!({"path": path, "samples": -3, "minPsnr": f64::INFINITY, "spec": {"safeArea": {"top": 9}, "captions": "x"}, "transcribe": false}),
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_looser_reading_speed_limit_needs_an_approval_record() {
    let mut s = demo();
    let e = s.execute("script.assemble", json!({"script": ARCA, "maxCps": 20})).unwrap_err().to_string();
    assert!(e.contains("maxCpsApproval"), "{e}");
    let e = s.execute("qc.render", json!({"path": "/nonexistent.mp4", "spec": {"captions": {"maxCps": 25}}})).unwrap_err().to_string();
    assert!(e.contains("maxCpsApproval"), "{e}");
    let e = s.execute("qc.check", json!({"spec": {"captions": {"maxCps": 25}}})).unwrap_err().to_string();
    assert!(e.contains("maxCpsApproval"), "{e}");
    // stricter is always allowed; looser with a record is accepted and logged
    s.execute("script.assemble", json!({"script": ARCA, "maxCps": 12})).unwrap();
    let r = s
        .execute("script.assemble", json!({"script": ARCA, "readability": {"maxCps": 20, "maxCpsApproval": {"by": "test", "reference": "unit test"}}}))
        .unwrap();
    assert_eq!(r["state"], "BLOCKED", "no transcripts: {r}");
    assert!(s.autoedit_log.iter().any(|e| e["event"] == "policy" && e["detail"]["approval"]["by"] == "test"));
}
