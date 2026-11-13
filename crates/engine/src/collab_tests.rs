//! M3B co-editing: the person and Claude edit the same session; protection, conflicts,
//! transactions, versions, save/reopen and recovery. Each test runs real edits and checks the
//! project, not just command results.

use serde_json::{Value, json};

use crate::Session;
use crate::collab::Actor;
use crate::media_test_util::{make_movie, tmp_dir};

const T: i64 = 254_016_000_000;

/// A 9:16 video of three clips; the session keeps its versions in `dir`.
fn session(dir: &std::path::Path) -> Session {
    let mut s = Session { library_data_dir: Some(dir.join("data")), ..Default::default() };
    let paths: Vec<String> = ["c1.mp4", "c2.mp4", "c3.mp4"]
        .iter()
        .map(|n| {
            let p = dir.join(n);
            make_movie(&p, filmcraft_media::DemoScene::Plasma, 64, 112, 48);
            p.to_string_lossy().to_string()
        })
        .collect();
    s.execute("creator.newVideo", json!({"aspect": "9:16", "paths": paths})).unwrap();
    s
}

fn as_agent(s: &mut Session, cmd: &str, p: Value) -> crate::Result<Value> {
    s.collab.actor = Actor::Agent;
    let r = s.execute(cmd, p);
    s.collab.actor = Actor::User;
    r
}

fn v1(s: &Session) -> Vec<(i64, i64, String, u64)> {
    let q = s.active_sequence().unwrap();
    let mut v: Vec<_> = q.video_tracks[0].items.iter().map(|i| (i.start.0, i.duration.0, i.name.clone(), i.id.0)).collect();
    v.sort();
    v
}

fn clip(s: &Session, n: usize) -> u64 {
    v1(s)[n].3
}

fn rev(s: &mut Session) -> u64 {
    s.execute("agent.status", json!({})).unwrap()["projectRevision"].as_u64().unwrap()
}

// 1. manual edit → Claude edit → manual edit, on one project and one history
#[test]
fn manual_then_claude_then_manual_edits_share_one_project_and_history() {
    let dir = tmp_dir("collab1");
    let mut s = session(&dir);
    // the person splits clip 1 at 0.5 s
    s.execute("playhead.set", json!({"seconds": 0.5})).unwrap();
    s.execute("sequence.addEdit", json!({})).unwrap();
    assert_eq!(v1(&s).len(), 4);
    // Claude trims the end of the last clip by half a second (ripple)
    let last = clip(&s, 3);
    let r = as_agent(&mut s, "creator.trim", json!({"clip": last, "edge": "out", "delta": -T / 2}));
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(v1(&s)[3].1, 2 * T - T / 2);
    assert_eq!(s.history.undo.last().map(|u| u.0.as_str()), Some("Claude: Trim Clip"));
    // the person deletes the first piece; Claude's trim stays
    let first = clip(&s, 0);
    s.execute("creator.delete", json!({ "clips": [first] })).unwrap();
    let after = v1(&s);
    assert_eq!(after.len(), 3);
    assert_eq!(after[2].1, 2 * T - T / 2, "Claude's trim kept");
    // the activity log names Claude's one action
    let a = s.execute("agent.activity", json!({})).unwrap();
    let list = a["activity"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["ok"], true);
    assert_eq!(list[0]["label"], "Trim Clip");
    assert!(list[0]["affected"]["clipsChanged"].as_array().unwrap().iter().any(|c| c.as_u64() == Some(last)), "{}", list[0]);
    // the changes list shows who did what, in order
    let ch = s.execute("agent.changes", json!({})).unwrap();
    let by: Vec<&str> = ch["changes"].as_array().unwrap().iter().filter_map(|c| c["by"].as_str()).collect();
    assert_eq!(by[by.len() - 3..], ["user", "agent", "user"], "{ch}");
    let _ = std::fs::remove_dir_all(&dir);
}

// 2. Claude edit → undo → redo
#[test]
fn claude_edit_undoes_and_redoes_as_one_step() {
    let dir = tmp_dir("collab2");
    let mut s = session(&dir);
    let before = v1(&s);
    let ids: Vec<u64> = before.iter().map(|c| c.3).collect();
    let r = as_agent(
        &mut s,
        "agent.transaction",
        json!({"label": "Tighten the edit", "commands": [
            {"command": "creator.trim", "params": {"clip": ids[0], "edge": "out", "delta": -T / 4}},
            {"command": "creator.trim", "params": {"clip": ids[1], "edge": "out", "delta": -T / 4}},
            {"command": "creator.transitions", "params": {}}
        ]}),
    )
    .unwrap();
    assert_eq!(r["results"].as_array().unwrap().len(), 3);
    let edited = v1(&s);
    assert_ne!(edited, before);
    assert_eq!(s.history.undo.last().map(|u| u.0.as_str()), Some("Claude: Tighten the edit"));
    // one undo takes all three back
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v1(&s), before);
    assert!(s.active_sequence().unwrap().video_tracks[0].transitions.is_empty());
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(v1(&s), edited);
    assert!(!s.active_sequence().unwrap().video_tracks[0].transitions.is_empty());
    // Claude can undo its own step too
    as_agent(&mut s, "edit.undo", json!({})).unwrap();
    assert_eq!(v1(&s), before);
    let _ = std::fs::remove_dir_all(&dir);
}

// 3. a protected clip is not changed by Claude
#[test]
fn claude_cannot_change_a_protected_clip_until_the_person_allows_it() {
    let dir = tmp_dir("collab3");
    let mut s = session(&dir);
    let c2 = clip(&s, 1);
    s.execute("protect.set", json!({"clips": [c2]})).unwrap();
    let before = v1(&s);
    let undo_n = s.history.undo.len();
    // trimming it directly
    let e = as_agent(&mut s, "creator.trim", json!({"clip": c2, "edge": "out", "delta": -T / 4})).unwrap_err().to_string();
    assert!(e.starts_with("PROTECTED") && e.contains("c2.mp4"), "{e}");
    assert_eq!(v1(&s), before, "nothing changed");
    assert_eq!(s.history.undo.len(), undo_n, "no undo step left behind");
    // a ripple of an earlier clip would move it: refused too
    let c1 = clip(&s, 0);
    let e = as_agent(&mut s, "creator.trim", json!({"clip": c1, "edge": "out", "delta": -T / 4})).unwrap_err().to_string();
    assert!(e.starts_with("PROTECTED"), "{e}");
    assert_eq!(v1(&s), before);
    // deleting it: refused
    assert!(as_agent(&mut s, "creator.delete", json!({ "clips": [c2] })).is_err());
    // Claude cannot lift the protection or answer its own request
    assert!(as_agent(&mut s, "protect.set", json!({"clips": [c2], "on": false})).unwrap_err().to_string().starts_with("NOT_ALLOWED"));
    let req =
        as_agent(&mut s, "agent.requestAuthorization", json!({"elements": [c2], "reason": "shorten the second shot"})).unwrap()["request"].as_u64().unwrap();
    assert!(as_agent(&mut s, "agent.authorize", json!({"request": req, "allow": true})).is_err());
    assert_eq!(s.execute("agent.authorizationStatus", json!({"request": req})).unwrap()["status"], "pending");
    // the blocked attempts are in the activity log
    let a = s.execute("agent.activity", json!({})).unwrap();
    assert!(a["activity"].as_array().unwrap().iter().filter(|x| x["blocked"] == true).count() >= 2);
    // the person allows it once
    s.execute("agent.authorize", json!({"request": req, "allow": true})).unwrap();
    let tok = s.execute("agent.authorizationStatus", json!({"request": req})).unwrap()["authorization"].as_str().unwrap().to_string();
    s.collab.authorization = Some(tok.clone());
    as_agent(&mut s, "creator.trim", json!({"clip": c2, "edge": "out", "delta": -T / 4})).unwrap();
    assert_eq!(v1(&s)[1].1, 2 * T - T / 4);
    // used up: the same token does not allow a second change
    let e = as_agent(&mut s, "creator.trim", json!({"clip": c2, "edge": "out", "delta": -T / 4})).unwrap_err().to_string();
    assert!(e.starts_with("PROTECTED"), "{e}");
    s.collab.authorization = None;
    // the person edits protected work freely
    s.execute("creator.trim", json!({"clip": c2, "edge": "out", "delta": -T / 4})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

// 4. a protected track is not changed by Claude
#[test]
fn claude_cannot_change_a_protected_track() {
    let dir = tmp_dir("collab4");
    let mut s = session(&dir);
    let a1 = s.active_sequence().unwrap().audio_tracks[0].id.0;
    s.execute("protect.set", json!({"tracks": [a1]})).unwrap();
    let before = s.project.clone();
    // changing a clip's gain on the protected audio track
    let aclip = s.active_sequence().unwrap().audio_tracks[0].items[0].id.0;
    let e = as_agent(&mut s, "clip.audioGain", json!({"clips": [aclip], "mode": "set", "db": -6})).unwrap_err().to_string();
    assert!(e.starts_with("PROTECTED") && e.contains("track A1"), "{e}");
    assert_eq!(*s.project, *before);
    // a video-only edit that leaves A1 alone goes through
    let c3 = clip(&s, 2);
    let v2 = s.active_sequence().unwrap().video_tracks[1].id.0;
    let r = as_agent(&mut s, "timeline.move", json!({"moves": [{"clip": c3, "track": v2, "time": 0}], "linked": false}));
    assert!(r.is_ok(), "{r:?}");
    // a transaction with one protected step changes nothing at all, not even the revision
    let before = s.project.clone();
    let r0 = rev(&mut s);
    let changes0 = s.collab.changes.len();
    let c1 = clip(&s, 0);
    let e = as_agent(
        &mut s,
        "agent.transaction",
        json!({"label": "Mix", "commands": [
            {"command": "creator.trim", "params": {"clip": c1, "edge": "out", "delta": -T / 4}},
            {"command": "clip.audioGain", "params": {"clips": [aclip], "mode": "set", "db": -6}}
        ]}),
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("PROTECTED"), "{e}");
    assert_eq!(*s.project, *before, "all or nothing");
    assert_eq!(rev(&mut s), r0, "a refused transaction is not a change");
    assert_eq!(s.collab.changes.len(), changes0, "and leaves no phantom entries");
    let _ = std::fs::remove_dir_all(&dir);
}

// 5. a command built on an old revision is refused
#[test]
fn a_stale_command_is_refused_and_lists_what_changed() {
    let dir = tmp_dir("collab5");
    let mut s = session(&dir);
    let seen = rev(&mut s);
    // the person edits meanwhile
    s.execute("playhead.set", json!({"seconds": 0.5})).unwrap();
    s.execute("sequence.addEdit", json!({})).unwrap();
    let before = s.project.clone();
    let c1 = clip(&s, 0);
    let e = as_agent(
        &mut s,
        "agent.transaction",
        json!({"label": "Trim", "expectRevision": seen, "commands": [{"command": "creator.trim", "params": {"clip": c1, "edge": "out", "delta": -T / 8}}]}),
    )
    .unwrap_err()
    .to_string();
    assert!(e.starts_with("CONFLICT") && e.contains("Add Edit by You"), "{e}");
    assert_eq!(*s.project, *before);
    // also for a single command (the control channel's expectRevision)
    s.collab.expect = Some(seen);
    let e = as_agent(&mut s, "creator.trim", json!({"clip": c1, "edge": "out", "delta": -T / 8})).unwrap_err().to_string();
    s.collab.expect = None;
    assert!(e.starts_with("CONFLICT"), "{e}");
    // with the current revision it applies
    let now = rev(&mut s);
    as_agent(
        &mut s,
        "agent.transaction",
        json!({"label": "Trim", "expectRevision": now, "commands": [{"command": "creator.trim", "params": {"clip": c1, "edge": "out", "delta": -T / 8}}]}),
    )
    .unwrap();
    // playback and selection are not changes
    let r2 = rev(&mut s);
    s.execute("playhead.set", json!({"seconds": 1.0})).unwrap();
    s.execute("timeline.select", json!({ "clips": [c1] })).unwrap();
    assert_eq!(rev(&mut s), r2);
    let _ = std::fs::remove_dir_all(&dir);
}

// 6. edits arriving from both sides at once: the one built on stale state loses, nothing mixes
#[test]
fn simultaneous_conflicting_edits_keep_the_persons_edit() {
    let dir = tmp_dir("collab6");
    let mut s = session(&dir);
    let c2 = clip(&s, 1);
    // both read the same revision
    let r0 = rev(&mut s);
    // the person deletes clip 2 first
    s.execute("creator.delete", json!({ "clips": [c2] })).unwrap();
    let after_person = s.project.clone();
    // Claude's edit of the same clip, built on r0, arrives next
    let e = as_agent(&mut s, "agent.transaction", json!({"label": "Grade clip 2", "expectRevision": r0, "commands": [{"command": "lumetri.applyPreset", "params": {"name": "Golden Dusk", "clips": [c2]}}]}))
        .unwrap_err()
        .to_string();
    assert!(e.starts_with("CONFLICT"), "{e}");
    assert_eq!(*s.project, *after_person, "the person's delete stands, nothing half-applied");
    // without expectRevision the command fails cleanly on the missing clip
    let e = as_agent(&mut s, "lumetri.applyPreset", json!({"name": "Golden Dusk", "clips": [c2]}));
    assert!(e.is_err());
    assert_eq!(*s.project, *after_person);
    let _ = std::fs::remove_dir_all(&dir);
}

// 7. snapshots: before large agent edits, by hand, and restored
#[test]
fn versions_are_made_before_big_agent_edits_and_restore_exactly() {
    let dir = tmp_dir("collab7");
    let mut s = session(&dir);
    let original = s.project.clone();
    let mine = s.execute("versions.create", json!({"label": "My cut"})).unwrap()["version"].as_str().unwrap().to_string();
    // a big agent change (deletes two clips) snapshots first
    let ids: Vec<u64> = v1(&s).iter().map(|c| c.3).collect();
    as_agent(&mut s, "creator.delete", json!({"clips": [ids[0], ids[1]]})).unwrap();
    let act = s.execute("agent.activity", json!({})).unwrap();
    let snap = act["activity"][0]["snapshot"].as_str().unwrap().to_string();
    let list = s.execute("versions.list", json!({})).unwrap();
    let versions = list["versions"].as_array().unwrap();
    assert!(versions.iter().any(|v| v["id"] == snap && v["auto"] == true && v["label"].as_str().unwrap().starts_with("Before Claude")), "{list}");
    assert_eq!(v1(&s).len(), 1);
    // restoring the snapshot gives back exactly the project before Claude's edit
    s.execute("versions.restore", json!({ "version": snap })).unwrap();
    assert_eq!(s.project.items, original.items);
    assert_eq!(v1(&s).len(), 3);
    // the restore is one undo step, and the state it replaced was kept as a version
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v1(&s).len(), 1);
    s.execute("versions.restore", json!({ "version": mine })).unwrap();
    assert_eq!(s.project.items, original.items);
    let n = s.execute("versions.list", json!({})).unwrap()["versions"].as_array().unwrap().len();
    assert!(n >= 4, "my version, Claude's snapshot, two safety copies: {n}");
    let _ = std::fs::remove_dir_all(&dir);
}

// 8. save and reopen keep everything, protection and Claude's activity included
#[test]
fn save_and_reopen_keep_edits_protection_and_activity() {
    let dir = tmp_dir("collab8");
    let mut s = session(&dir);
    let c1 = clip(&s, 0);
    as_agent(&mut s, "creator.trim", json!({"clip": c1, "edge": "out", "delta": -T / 4})).unwrap();
    s.execute("protect.set", json!({"clips": [c1]})).unwrap();
    s.execute("captions.add", json!({"text": "Hello", "seconds": 0.2})).unwrap();
    let path = dir.join("p.fcproj").to_string_lossy().to_string();
    s.execute("file.save", json!({ "path": path })).unwrap();
    let saved = s.project.clone();
    let mut t = Session { library_data_dir: Some(dir.join("data")), ..Default::default() };
    t.execute("file.open", json!({ "path": path })).unwrap();
    assert_eq!(t.project.items, saved.items);
    assert!(t.project.protection.clips.contains(&filmcraft_project::ClipId(c1)));
    let a = t.execute("agent.activity", json!({})).unwrap();
    assert_eq!(a["activity"].as_array().unwrap().len(), 1, "Claude's activity follows the project");
    // the reopened project still protects its clip from Claude
    assert!(as_agent(&mut t, "creator.delete", json!({ "clips": [c1] })).unwrap_err().to_string().starts_with("PROTECTED"));
    // another program changed the file: saving over it is refused, forcing it works
    s.execute("playhead.set", json!({"seconds": 0.1})).unwrap();
    s.execute("sequence.addEdit", json!({})).unwrap();
    s.execute("file.save", json!({})).unwrap();
    let e = t.execute("file.save", json!({})).unwrap_err().to_string();
    assert!(e.starts_with("CONFLICT"), "{e}");
    t.execute("file.save", json!({"force": true})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

// 9. an interrupted or failing edit leaves the project as it was; unsaved work is recoverable
#[test]
fn a_failed_transaction_changes_nothing_and_unsaved_work_survives_a_crash() {
    let dir = tmp_dir("collab9");
    let mut s = session(&dir);
    let before = s.project.clone();
    let undo_n = s.history.undo.len();
    let c1 = clip(&s, 0);
    let e = as_agent(
        &mut s,
        "agent.transaction",
        json!({"label": "Half done", "commands": [
            {"command": "creator.trim", "params": {"clip": c1, "edge": "out", "delta": -T / 4}},
            {"command": "creator.trim", "params": {"clip": 999_999, "edge": "out", "delta": -T / 4}}
        ]}),
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("step 2"), "{e}");
    assert_eq!(*s.project, *before);
    assert_eq!(s.history.undo.len(), undo_n);
    // an agent closing a project with unsaved work: the work is kept as a version first
    let mut other = Session { library_data_dir: Some(dir.join("data")), ..Default::default() };
    other.execute("creator.newVideo", json!({"aspect": "1:1"})).unwrap();
    let unsaved = other.project.clone();
    as_agent(&mut other, "file.newProject", json!({"name": "x"})).unwrap();
    let v = other.execute("versions.list", json!({})).unwrap();
    let kept = v["versions"].as_array().unwrap().iter().find(|x| x["label"].as_str().unwrap_or_default().starts_with("Unsaved")).cloned().unwrap();
    other.execute("versions.restore", json!({"version": kept["id"]})).unwrap();
    assert_eq!(other.project.items, unsaved.items, "the unsaved project comes back");
    // crash: the recovery journal holds Claude's unsaved edit, and the next launch recovers it
    let data = dir.join("recovery");
    let start = |s: &mut Session| {
        let mut cfg = crate::autosave::AutosaveConfig::new(&data);
        cfg.journal_debounce = std::time::Duration::from_millis(20);
        s.start_autosave(cfg).unwrap();
    };
    start(&mut s);
    as_agent(&mut s, "creator.trim", json!({"clip": c1, "edge": "out", "delta": -T / 4})).unwrap();
    let edited = (*s.project).clone();
    let rv = s.revision;
    let t0 = std::time::Instant::now();
    loop {
        s.poll_persistence();
        if s.persistence.as_ref().unwrap().status.journaled_revision == rv {
            break;
        }
        assert!(t0.elapsed() < std::time::Duration::from_secs(10), "journal");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    s.persistence.take().unwrap().simulate_crash();
    let mut next = Session { library_data_dir: Some(dir.join("data")), ..Default::default() };
    start(&mut next);
    assert_eq!(next.recovery_candidates().len(), 1, "a recovery candidate after the crash");
    next.execute("file.recover", json!({})).unwrap();
    assert_eq!(*next.project, edited, "Claude's edit is recovered");
    next.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

// 10. a new connection continues from the latest state
#[test]
fn an_agent_reconnecting_sees_the_state_and_continues() {
    let dir = tmp_dir("collab10");
    let mut s = session(&dir);
    let c1 = clip(&s, 0);
    as_agent(&mut s, "creator.trim", json!({"clip": c1, "edge": "out", "delta": -T / 4})).unwrap();
    // the connection drops; a new one starts by asking where things are
    let st = s.execute("agent.status", json!({})).unwrap();
    let r = st["projectRevision"].as_u64().unwrap();
    assert_eq!(st["lastActivity"]["label"], "Trim Clip");
    assert_eq!(st["undo"], "Claude: Trim Clip");
    assert_eq!(st["unsavedChanges"], true);
    // and continues with that revision
    let c2 = clip(&s, 1);
    as_agent(
        &mut s,
        "agent.transaction",
        json!({"label": "Next", "expectRevision": r, "commands": [{"command": "creator.trim", "params": {"clip": c2, "edge": "out", "delta": -T / 4}}]}),
    )
    .unwrap();
    assert_eq!(s.execute("agent.activity", json!({})).unwrap()["activity"].as_array().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every way Claude's command can be refused leaves the project, its revision, the undo/redo
/// history and the change list exactly as they were.
#[test]
fn every_rejected_agent_operation_leaves_no_trace() {
    let dir = tmp_dir("collab_reject");
    let mut s = session(&dir);
    let ids: Vec<u64> = v1(&s).iter().map(|c| c.3).collect();
    // the person protects clip 2 and then edits it (so Claude's undo would change it)
    s.execute("protect.set", json!({"clips": [ids[1]]})).unwrap();
    s.execute("creator.trim", json!({"clip": ids[1], "edge": "out", "delta": -T / 4})).unwrap();
    let stale = rev(&mut s) - 1;
    type Snap = (std::sync::Arc<filmcraft_project::Project>, u64, usize, usize, usize, Vec<String>);
    let snap = |s: &mut Session| -> Snap {
        let r = rev(s);
        (s.project.clone(), r, s.history.undo.len(), s.history.redo.len(), s.collab.changes.len(), s.history.undo.iter().map(|u| u.0.clone()).collect())
    };
    let attempts: Vec<(&str, &str, Value)> = vec![
        ("PROTECTED", "creator.trim", json!({"clip": ids[1], "edge": "out", "delta": -T / 8})),
        ("PROTECTED", "creator.trim", json!({"clip": ids[0], "edge": "out", "delta": -T / 8})),
        ("PROTECTED", "creator.delete", json!({ "clips": [ids[1]] })),
        ("PROTECTED", "lumetri.applyPreset", json!({"name": "Golden Dusk", "clips": [ids[1]]})),
        (
            "PROTECTED",
            "agent.transaction",
            json!({"label": "x", "commands": [{"command": "creator.trim", "params": {"clip": ids[2], "edge": "out", "delta": -T / 8}}, {"command": "creator.delete", "params": {"clips": [ids[1]]}}]}),
        ),
        ("PROTECTED", "edit.undo", json!({})),
        (
            "CONFLICT",
            "agent.transaction",
            json!({"label": "x", "expectRevision": stale, "commands": [{"command": "creator.trim", "params": {"clip": ids[2], "edge": "out", "delta": -T / 8}}]}),
        ),
        ("NOT_ALLOWED", "protect.set", json!({"clips": [ids[1]], "on": false})),
        ("NOT_ALLOWED", "agent.transaction", json!({"label": "x", "commands": [{"command": "edit.undo", "params": {}}]})),
        (
            "step 2",
            "agent.transaction",
            json!({"label": "x", "commands": [{"command": "creator.trim", "params": {"clip": ids[2], "edge": "out", "delta": -T / 8}}, {"command": "creator.trim", "params": {"clip": 424_242, "edge": "out", "delta": -T}}]}),
        ),
    ];
    for (code, cmd, p) in attempts {
        let before = snap(&mut s);
        let e = as_agent(&mut s, cmd, p.clone()).unwrap_err().to_string();
        assert!(e.contains(code), "{cmd} {p}: {e}");
        let after = snap(&mut s);
        assert!(std::sync::Arc::ptr_eq(&before.0, &after.0) || *before.0 == *after.0, "{cmd}: project changed");
        assert_eq!(before.1, after.1, "{cmd}: revision advanced");
        assert_eq!((before.2, before.3), (after.2, after.3), "{cmd}: undo/redo history changed");
        assert_eq!(before.4, after.4, "{cmd}: phantom change entry");
        assert_eq!(before.5, after.5, "{cmd}: history labels changed");
    }
    // a redo that would change protected work is refused the same way
    s.execute("edit.undo", json!({})).unwrap();
    let before = snap(&mut s);
    let e = as_agent(&mut s, "edit.redo", json!({})).unwrap_err().to_string();
    assert!(e.starts_with("PROTECTED"), "{e}");
    let after = snap(&mut s);
    assert_eq!(*before.0, *after.0);
    assert_eq!((before.1, before.2, before.3, before.4), (after.1, after.2, after.3, after.4));
    // every refusal is in Claude's activity, and none counts as a change
    let a = s.execute("agent.activity", json!({})).unwrap();
    let list = a["activity"].as_array().unwrap();
    assert!(list.len() >= 11, "{}", list.len());
    assert!(list.iter().all(|x| x["ok"] == false && x["revisionBefore"] == x["revisionAfter"]));
    let _ = std::fs::remove_dir_all(&dir);
}

// M3D: Claude uses the short-form tools through the same commands, under the same rules
#[test]
fn claude_short_form_edits_keep_captions_in_sync_and_respect_protected_captions() {
    let dir = tmp_dir("collab-m3d");
    let mut s = session(&dir);
    // captions over the second clip, styled by Claude
    s.execute("captions.newTrack", json!({"format": "Subtitle"})).unwrap();
    s.execute("captions.add", json!({"text": "over the second clip", "seconds": 2.5, "durationSeconds": 1.0})).unwrap();
    as_agent(&mut s, "captions.applyPreset", json!({"preset": "boldPop"})).unwrap();
    assert_eq!(s.history.undo.last().map(|u| u.0.as_str()), Some("Claude: Apply Caption Preset"));
    // Claude trims the first clip: the caption over the second moves with it, in one undo step
    let c1 = clip(&s, 0);
    let undo_n = s.history.undo.len();
    as_agent(&mut s, "creator.trim", json!({"clip": c1, "edge": "out", "delta": -T / 2})).unwrap();
    assert_eq!(s.history.undo.len(), undo_n + 1);
    let start = s.active_sequence().unwrap().caption_tracks[0].captions[0].start;
    assert_eq!(start.0, 2 * T, "2.5 s → 2.0 s with its footage");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_sequence().unwrap().caption_tracks[0].captions[0].start.0, 5 * T / 2);
    // protected captions: Claude's ripple would move them, so it is refused and nothing changes
    let cap_track = s.active_sequence().unwrap().caption_tracks[0].id.0;
    s.execute("protect.set", json!({"tracks": [cap_track]})).unwrap();
    let before = v1(&s);
    let e = as_agent(&mut s, "creator.trim", json!({"clip": c1, "edge": "out", "delta": -T / 2})).unwrap_err().to_string();
    assert!(e.starts_with("PROTECTED"), "{e}");
    assert_eq!(v1(&s), before);
    assert_eq!(s.active_sequence().unwrap().caption_tracks[0].captions[0].start.0, 5 * T / 2);
    // the person's own edit is not blocked
    s.execute("creator.trim", json!({"clip": c1, "edge": "out", "delta": -T / 2})).unwrap();
    assert_eq!(s.active_sequence().unwrap().caption_tracks[0].captions[0].start.0, 2 * T);
    let _ = std::fs::remove_dir_all(&dir);
}
