//! Claude co-editing and project safety (M3B): the person and an agent edit the same session,
//! with the same commands, undo history and project.
//!
//! - **Actor.** Commands run as the person ([`Actor::User`], the default) or as an agent
//!   ([`Actor::Agent`]: the control channel's `engine.execute`, the headless MCP server, and
//!   synthetic input the control channel injects). Every editing command an agent runs is one
//!   undo step labelled `Claude: …`, is checked against the person's protected work, is recorded
//!   in the activity log, and is preceded by a version snapshot when it changes a lot.
//! - **Protection.** `Project::protection` (saved with the project) lists clips, captions and
//!   tracks the person protected from Claude. After an agent command runs, what it changed is
//!   compared with the protected elements; if it touched one, the whole command is rolled back
//!   and refused with the reason (`PROTECTED: …`). Only the person can lift a protection or grant
//!   a one-time authorization (`agent.requestAuthorization` → the person allows it in the app →
//!   the agent passes the token). The person's own edits are never restricted.
//! - **Revisions and conflicts.** `projectRevision` counts changes of the project's content (not
//!   playback, selection or render previews). An agent passes the revision it last saw
//!   (`expectRevision`); when the project has changed since, the command is refused
//!   (`CONFLICT: …`) with the changes made meanwhile, and nothing is applied.
//! - **Transactions.** `agent.transaction` runs several commands as one atomic, labelled undo
//!   step: if any fails (or touches protected work), none of them is applied.
//! - **Versions.** Snapshots of the whole project (the project file only: media stays where it
//!   is) in `<data dir>/Project Versions/<project>/`, made by hand (`versions.create`), before
//!   large agent edits and before a restore; `versions.restore` brings one back as an undoable
//!   step.
//! - **Disk conflicts.** Saving over a project file that another program (another FilmCraft, a
//!   headless agent) changed since it was opened or saved is refused unless forced.
//!
//! See `docs/co-editing.md`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use filmcraft_project::{ClipId, Project, TrackId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// Who runs a command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Actor {
    #[default]
    User,
    Agent,
}

impl Actor {
    pub fn name(self) -> &'static str {
        match self {
            Actor::User => "You",
            Actor::Agent => "Claude",
        }
    }
}

/// One change of the project's content.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub revision: u64,
    pub time: String,
    pub label: String,
    pub by: Actor,
}

/// A request from Claude to change protected work, answered by the person.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRequest {
    pub id: u64,
    pub time: String,
    pub clips: Vec<u64>,
    pub tracks: Vec<u64>,
    /// What the elements are, for the person (`clip "Interview.mov" on V1`).
    pub names: Vec<String>,
    pub reason: String,
    /// `pending`, `allowed`, `denied` or `used`.
    pub status: String,
    #[serde(skip)]
    pub token: String,
}

/// Collaboration state of a session (not saved with the project).
#[derive(Default)]
pub struct Collab {
    pub actor: Actor,
    /// Inside the agent wrapper: the command itself runs as usual.
    pub(crate) wrapping: bool,
    /// Content revision (see the module docs).
    pub revision: u64,
    seen: Option<Arc<Project>>,
    pub changes: VecDeque<Change>,
    /// Claude's actions, newest last (also appended to the project's `activity.jsonl`).
    pub activity: Vec<Value>,
    pub requests: Vec<AuthRequest>,
    /// The authorization token the current agent call carries (`authorization`).
    pub authorization: Option<String>,
    /// The revision the current agent call expects (`expectRevision`).
    pub expect: Option<u64>,
    /// Versions key of an unsaved project (moves to the file's key when it is saved).
    unsaved_key: Option<String>,
    /// SHA-256 of the project file as this session last read or wrote it.
    pub(crate) disk_sha: Option<String>,
    /// `file.save {force: true}` for this call.
    pub(crate) save_force: bool,
    /// The person's undo/redo since the agent's last command, for `agent.status`.
    next_request: u64,
}

const MAX_CHANGES: usize = 500;
const MAX_ACTIVITY: usize = 500;
const MAX_VERSIONS: usize = 100;

/// Commands only the person may run.
const USER_ONLY: [&str; 2] = ["agent.authorize", "versions.delete"];
/// Commands that replace the open project: an agent may run them only when nothing is unsaved.
const REPLACES_DOCUMENT: [&str; 6] = ["file.newProject", "file.open", "file.openDemoProject", "file.revert", "file.closeProject", "file.recover"];

fn now() -> String {
    crate::user_library::now_rfc3339()
}

// ------------------------------------------------------------------------------------------
// revisions
// ------------------------------------------------------------------------------------------

/// Bring the content revision up to date: a project that is not the one last seen is a change,
/// made by `by` (labelled `label`, else the last undo step's).
pub(crate) fn observe(s: &mut Session, by: Actor, label: Option<&str>) {
    if s.collab.seen.as_ref().is_some_and(|p| Arc::ptr_eq(p, &s.project)) {
        return;
    }
    let first = s.collab.seen.is_none();
    s.collab.seen = Some(s.project.clone());
    if first {
        return;
    }
    s.collab.revision += 1;
    let label = label.map(str::to_string).or_else(|| s.history.undo.last().map(|u| u.0.clone())).unwrap_or_else(|| "Project changed".into());
    s.collab.changes.push_back(Change { revision: s.collab.revision, time: now(), label, by });
    while s.collab.changes.len() > MAX_CHANGES {
        s.collab.changes.pop_front();
    }
}

/// The content revision now.
pub fn revision(s: &mut Session) -> u64 {
    observe(s, Actor::User, None);
    s.collab.revision
}

fn conflict(s: &Session, expected: u64) -> EngineError {
    let since: Vec<String> =
        s.collab.changes.iter().filter(|c| c.revision > expected).map(|c| format!("{} by {} (revision {})", c.label, c.by.name(), c.revision)).collect();
    EngineError::Other(format!(
        "CONFLICT: the project changed since revision {expected} (now {}): {}. Nothing was applied; inspect the project again (agent.status) and retry with the new revision.",
        s.collab.revision,
        if since.is_empty() { "the earlier changes are no longer listed".to_string() } else { since.join("; ") }
    ))
}

fn check_expected(s: &mut Session, expected: Option<u64>) -> Result<()> {
    let now = revision(s);
    match expected {
        Some(e) if e != now => Err(conflict(s, e)),
        _ => Ok(()),
    }
}

// ------------------------------------------------------------------------------------------
// what changed
// ------------------------------------------------------------------------------------------

/// What an edit changed, by id.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    pub clips_added: Vec<u64>,
    pub clips_removed: Vec<u64>,
    pub clips_changed: Vec<u64>,
    pub tracks_changed: Vec<u64>,
    pub tracks_added: Vec<u64>,
    pub tracks_removed: Vec<u64>,
    pub sequences_changed: Vec<u64>,
    pub items_added: Vec<u64>,
    pub items_removed: Vec<u64>,
    pub other: bool,
    /// What is protected from Claude changed.
    pub protection: bool,
    /// Clip and track names for the person, by id.
    #[serde(skip)]
    pub names: BTreeMap<u64, String>,
    /// The track each clip was on before and is on after.
    #[serde(skip)]
    pub clip_tracks: BTreeMap<u64, BTreeSet<u64>>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.clips_added.is_empty()
            && self.clips_removed.is_empty()
            && self.clips_changed.is_empty()
            && self.tracks_changed.is_empty()
            && self.tracks_added.is_empty()
            && self.tracks_removed.is_empty()
            && self.items_added.is_empty()
            && self.items_removed.is_empty()
            && !self.other
            && !self.protection
    }

    /// One plain sentence: "3 clips changed, 1 removed".
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        let n = |v: &Vec<u64>, one: &str, many: &str| match v.len() {
            0 => None,
            1 => Some(format!("1 {one}")),
            k => Some(format!("{k} {many}")),
        };
        parts.extend(n(&self.clips_changed, "clip changed", "clips changed"));
        parts.extend(n(&self.clips_added, "clip added", "clips added"));
        parts.extend(n(&self.clips_removed, "clip removed", "clips removed"));
        parts.extend(n(&self.tracks_added, "track added", "tracks added"));
        parts.extend(n(&self.tracks_removed, "track removed", "tracks removed"));
        parts.extend(n(&self.items_added, "file imported", "files imported"));
        parts.extend(n(&self.items_removed, "project item removed", "project items removed"));
        if self.protection {
            parts.push("protection changed".into());
        }
        if parts.is_empty() {
            if self.tracks_changed.is_empty() && !self.other { "nothing changed".into() } else { "settings changed".into() }
        } else {
            parts.join(", ")
        }
    }
}

/// Per sequence: clip id → (track, json), track id → (json without the clips, clip ids).
struct SeqIndex {
    clips: BTreeMap<u64, (u64, String)>,
    tracks: BTreeMap<u64, (String, Vec<u64>)>,
}

fn track_label(kind: &str, i: usize, name: &str) -> String {
    if name.is_empty() { format!("{kind}{}", i + 1) } else { format!("{kind}{} \"{name}\"", i + 1) }
}

fn index_sequence(q: &filmcraft_project::Sequence, names: &mut BTreeMap<u64, String>) -> SeqIndex {
    let mut clips = BTreeMap::new();
    let mut tracks = BTreeMap::new();
    let groups = [("V", &q.video_tracks), ("A", &q.audio_tracks)];
    for (kind, list) in groups {
        for (i, t) in list.iter().enumerate() {
            let label = track_label(kind, i, &t.name);
            names.insert(t.id.0, format!("track {label}"));
            let mut shell = t.clone();
            shell.items.clear();
            let ids: Vec<u64> = t.items.iter().map(|c| c.id.0).collect();
            // a track's transitions are part of its content
            tracks.insert(t.id.0, (serde_json::to_string(&shell).unwrap_or_default(), ids));
            for c in &t.items {
                names.insert(c.id.0, format!("clip \"{}\" on {kind}{}", c.name, i + 1));
                clips.insert(c.id.0, (t.id.0, serde_json::to_string(c).unwrap_or_default()));
            }
        }
    }
    for (i, t) in q.caption_tracks.iter().enumerate() {
        let label = track_label("C", i, &t.name);
        names.insert(t.id.0, format!("caption track {label}"));
        let mut shell = t.clone();
        shell.captions.clear();
        let ids: Vec<u64> = t.captions.iter().map(|c| c.id.0).collect();
        tracks.insert(t.id.0, (serde_json::to_string(&shell).unwrap_or_default(), ids));
        for c in &t.captions {
            let text: String = c.text.chars().take(30).collect();
            names.insert(c.id.0, format!("caption \"{text}\" on C{}", i + 1));
            clips.insert(c.id.0, (t.id.0, serde_json::to_string(c).unwrap_or_default()));
        }
    }
    SeqIndex { clips, tracks }
}

fn sequences(p: &Project) -> BTreeMap<u64, &Arc<filmcraft_project::Sequence>> {
    p.items
        .iter()
        .filter_map(|(id, it)| match &it.kind {
            filmcraft_project::ItemKind::Sequence(q) => Some((id.0, q)),
            _ => None,
        })
        .collect()
}

/// What changed from `a` to `b`.
pub fn diff(a: &Project, b: &Project) -> Diff {
    let mut d = Diff::default();
    let (sa, sb) = (sequences(a), sequences(b));
    let ids: BTreeSet<u64> = sa.keys().chain(sb.keys()).copied().collect();
    for id in ids {
        let (qa, qb) = (sa.get(&id), sb.get(&id));
        if let (Some(x), Some(y)) = (qa, qb)
            && Arc::ptr_eq(x, y)
        {
            continue;
        }
        let empty = || SeqIndex { clips: BTreeMap::new(), tracks: BTreeMap::new() };
        let ia = qa.map(|q| index_sequence(q, &mut d.names)).unwrap_or_else(empty);
        let ib = qb.map(|q| index_sequence(q, &mut d.names)).unwrap_or_else(empty);
        let mut changed = false;
        for (c, (t, _)) in ia.clips.iter().chain(ib.clips.iter()) {
            d.clip_tracks.entry(*c).or_default().insert(*t);
        }
        for (c, (ta, ja)) in &ia.clips {
            match ib.clips.get(c) {
                None => {
                    d.clips_removed.push(*c);
                    changed = true;
                }
                Some((tb, jb)) if tb != ta || jb != ja => {
                    d.clips_changed.push(*c);
                    changed = true;
                }
                _ => {}
            }
        }
        for c in ib.clips.keys().filter(|c| !ia.clips.contains_key(c)) {
            d.clips_added.push(*c);
            changed = true;
        }
        for (t, va) in &ia.tracks {
            match ib.tracks.get(t) {
                None => {
                    d.tracks_removed.push(*t);
                    changed = true;
                }
                Some(vb) if vb != va => {
                    d.tracks_changed.push(*t);
                    changed = true;
                }
                _ => {}
            }
        }
        for t in ib.tracks.keys().filter(|t| !ia.tracks.contains_key(t)) {
            d.tracks_added.push(*t);
            changed = true;
        }
        // settings, markers, in/out: not a clip or a track
        if !changed {
            d.other = true;
        }
        d.sequences_changed.push(id);
    }
    for id in b.items.keys().filter(|i| !a.items.contains_key(i)) {
        if !sb.contains_key(&id.0) {
            d.items_added.push(id.0);
        }
    }
    for id in a.items.keys().filter(|i| !b.items.contains_key(i)) {
        if !sa.contains_key(&id.0) {
            d.items_removed.push(id.0);
        }
    }
    d.protection = a.protection != b.protection;
    if d.is_empty() && !d.protection && a != b {
        d.other = true;
    }
    d
}

// ------------------------------------------------------------------------------------------
// protection
// ------------------------------------------------------------------------------------------

/// The protected elements `d` touched (as `before` protects them), less what the current
/// authorization allows: (ids, names).
fn violations(s: &Session, before: &Project, d: &Diff) -> Vec<(u64, String)> {
    let prot = &before.protection;
    if prot.is_empty() {
        return Vec::new();
    }
    let allowed: BTreeSet<u64> = match &s.collab.authorization {
        Some(tok) => {
            s.collab.requests.iter().filter(|r| r.status == "allowed" && &r.token == tok).flat_map(|r| r.clips.iter().chain(r.tracks.iter()).copied()).collect()
        }
        None => BTreeSet::new(),
    };
    let mut out = Vec::new();
    let touched_clips = d.clips_changed.iter().chain(d.clips_removed.iter()).chain(d.clips_added.iter());
    for c in touched_clips {
        if prot.clips.contains(&ClipId(*c)) && !allowed.contains(c) {
            out.push((*c, d.names.get(c).cloned().unwrap_or_else(|| format!("clip {c}"))));
        }
        // a clip on (or moved onto, or off) a protected track is part of that track
        for t in d.clip_tracks.get(c).into_iter().flatten() {
            if prot.tracks.contains(&TrackId(*t)) && !allowed.contains(t) && !allowed.contains(c) {
                out.push((*t, d.names.get(t).cloned().unwrap_or_else(|| format!("track {t}"))));
            }
        }
    }
    let touched_tracks = d.tracks_changed.iter().chain(d.tracks_removed.iter());
    for t in touched_tracks {
        if prot.tracks.contains(&TrackId(*t)) && !allowed.contains(t) {
            out.push((*t, d.names.get(t).cloned().unwrap_or_else(|| format!("track {t}"))));
        }
    }
    // a clip added to (or moved onto) a protected track changes that track
    out.sort();
    out.dedup();
    out
}

fn protected_error(v: &[(u64, String)]) -> EngineError {
    let names: Vec<&str> = v.iter().map(|x| x.1.as_str()).collect();
    let ids: Vec<u64> = v.iter().map(|x| x.0).collect();
    EngineError::Other(format!(
        "PROTECTED: this would change work protected from Claude ({}). Nothing was changed. Ask the person to allow it: agent.requestAuthorization {{\"elements\": {ids:?}, \"reason\": \"…\"}}, or to remove the protection.",
        names.join(", ")
    ))
}

/// Use up the authorization of the current call once it has been used for a successful edit.
fn consume_authorization(s: &mut Session) {
    if let Some(tok) = s.collab.authorization.clone() {
        for r in s.collab.requests.iter_mut().filter(|r| r.status == "allowed" && r.token == tok) {
            r.status = "used".into();
        }
    }
}

// ------------------------------------------------------------------------------------------
// the agent wrapper
// ------------------------------------------------------------------------------------------

/// Whether an agent's command is wrapped (an editing command, at the top level, not already
/// inside the wrapper).
pub(crate) fn wraps(s: &Session, journal: bool) -> bool {
    s.collab.actor == Actor::Agent && !s.collab.wrapping && journal && s.exec_depth == 0
}

fn agent_label(spec: &CommandSpec, id: &str, p: &Value) -> String {
    let what = if id == "agent.transaction" { str_p(p, "label").unwrap_or("Changes").to_string() } else { spec.label.trim_end_matches('…').to_string() };
    format!("Claude: {what}")
}

/// Run an agent's editing command: refuse what only the person may do, check the revision it
/// expects, run it as one undo step, roll it back when it touched protected work, snapshot the
/// project before a large change, and record it.
pub(crate) fn run_agent(s: &mut Session, spec: &CommandSpec, id: &str, params: &Value) -> Result<Value> {
    // anything that changed since the agent's last command was not the agent's doing
    observe(s, Actor::User, None);
    let label = agent_label(spec, id, params);
    let rev0 = s.collab.revision;
    let before = s.project.clone();
    let r = refuse_user_only(id, params).and_then(|()| {
        let expected = s.collab.expect.or_else(|| u64_p(params, "expectRevision"));
        check_expected(s, expected)
    });
    if REPLACES_DOCUMENT.contains(&id) {
        return r.and_then(|()| replace_document(s, id, params, &before, &label, rev0));
    }
    let r =
        r.and_then(
            |()| if matches!(id, "edit.undo" | "edit.redo") { run_undo_redo(s, id, params, &before) } else { run_grouped(s, id, params, &before, &label) },
        );
    let d = if Arc::ptr_eq(&before, &s.project) { Diff::default() } else { diff(&before, &s.project) };
    let mut snapshot = None;
    if r.is_ok() && !d.is_empty() {
        consume_authorization(s);
        let big = id == "agent.transaction" && bool_p(params, "snapshot") != Some(false)
            || bool_p(params, "snapshot") == Some(true)
            || !d.clips_removed.is_empty()
            || !d.tracks_removed.is_empty()
            || d.clips_changed.len() + d.clips_added.len() >= 5;
        if big {
            snapshot = write_version(s, &before, &format!("Before {label}"), Actor::Agent, true, rev0).ok();
        }
    }
    observe(s, Actor::Agent, Some(&label));
    // what changed, or what was tried and refused; reads and no-ops are not listed
    if !d.is_empty() || r.is_err() {
        record(s, id, &label, params, (&r, &d, snapshot.as_deref()), rev0);
    }
    r
}

/// Claude opens, reverts or replaces the project: a different document, so it is not compared
/// with the person's protections. Unsaved work in the project it closes is kept first, as a
/// version of the project that is open afterwards (Changes ▸ Versions ▸ Restore brings it back).
fn replace_document(s: &mut Session, id: &str, params: &Value, before: &Arc<Project>, label: &str, rev0: u64) -> Result<Value> {
    let unsaved = s.is_dirty();
    let r = inner(s, id, params);
    let mut snapshot = None;
    if r.is_ok() && unsaved && !Arc::ptr_eq(before, &s.project) {
        let what = format!("Unsaved \"{}\" before {label}", before.name);
        snapshot = write_version(s, before, &what, Actor::Agent, true, rev0).ok();
        if snapshot.is_some() {
            s.toast(format!("{label}: your unsaved project was kept as a version (Changes ▸ Versions)"));
        }
    }
    observe(s, Actor::Agent, Some(label));
    if unsaved || r.is_err() {
        record(s, id, label, params, (&r, &Diff::default(), snapshot.as_deref()), rev0);
    }
    r
}

fn refuse_user_only(id: &str, params: &Value) -> Result<()> {
    if USER_ONLY.contains(&id) {
        return Err(EngineError::Other(format!("NOT_ALLOWED: only the person using FilmCraft can run `{id}`.")));
    }
    if id == "protect.set" && bool_p(params, "on") == Some(false) {
        return Err(EngineError::Other(
            "NOT_ALLOWED: only the person can remove a protection. Ask them, or request a one-time authorization (agent.requestAuthorization).".into(),
        ));
    }
    if id == "agent.transaction" {
        for c in params.get("commands").and_then(Value::as_array).into_iter().flatten() {
            let sub = str_p(c, "command").unwrap_or_default();
            if sub == "agent.transaction" || USER_ONLY.contains(&sub) || REPLACES_DOCUMENT.contains(&sub) || matches!(sub, "edit.undo" | "edit.redo") {
                return Err(EngineError::Other(format!("NOT_ALLOWED: `{sub}` cannot be part of a transaction.")));
            }
            refuse_user_only(sub, c.get("params").unwrap_or(&Value::Null))?;
        }
    }
    Ok(())
}

fn inner(s: &mut Session, id: &str, params: &Value) -> Result<Value> {
    s.collab.wrapping = true;
    let r = s.execute(id, params.clone());
    s.collab.wrapping = false;
    r
}

fn run_grouped(s: &mut Session, id: &str, params: &Value, before: &Arc<Project>, label: &str) -> Result<Value> {
    s.edit_group(label, |s| {
        let v = inner(s, id, params)?;
        if !Arc::ptr_eq(before, &s.project) {
            let d = diff(before, &s.project);
            let bad = violations(s, before, &d);
            if !bad.is_empty() {
                return Err(protected_error(&bad));
            }
        }
        Ok(v)
    })
}

/// Undo and redo move between existing states: when the step would change protected work, it is
/// stepped back again and refused.
fn run_undo_redo(s: &mut Session, id: &str, params: &Value, before: &Arc<Project>) -> Result<Value> {
    let v = inner(s, id, params)?;
    if Arc::ptr_eq(before, &s.project) {
        return Ok(v);
    }
    let d = diff(before, &s.project);
    let bad = violations(s, before, &d);
    if bad.is_empty() {
        return Ok(v);
    }
    if id == "edit.undo" {
        s.redo();
    } else {
        s.undo();
    }
    Err(protected_error(&bad))
}

fn record(s: &mut Session, id: &str, label: &str, params: &Value, outcome: (&Result<Value>, &Diff, Option<&str>), rev0: u64) {
    let (r, d, snapshot) = outcome;
    let commands: Vec<Value> = if id == "agent.transaction" {
        params.get("commands").and_then(Value::as_array).map(|a| a.iter().map(|c| c["command"].clone()).collect()).unwrap_or_default()
    } else {
        vec![json!(id)]
    };
    // a failed command that changed nothing and was never an edit attempt is still logged: the
    // person sees what Claude tried
    let mut names: Vec<String> =
        d.clips_changed.iter().chain(&d.clips_added).chain(&d.clips_removed).chain(&d.tracks_changed).filter_map(|i| d.names.get(i).cloned()).collect();
    names.truncate(12);
    let entry = json!({
        "id": s.collab.activity.len() as u64 + 1,
        "time": now(),
        "by": "Claude",
        "label": label.trim_start_matches("Claude: "),
        "commands": commands,
        "ok": r.is_ok(),
        "error": r.as_ref().err().map(|e| e.to_string()),
        "blocked": r.as_ref().err().is_some_and(|e| e.to_string().starts_with("PROTECTED")),
        "conflict": r.as_ref().err().is_some_and(|e| e.to_string().starts_with("CONFLICT")),
        "summary": if r.is_ok() { d.summary() } else { "nothing changed".to_string() },
        "affected": {"clipsChanged": d.clips_changed, "clipsAdded": d.clips_added, "clipsRemoved": d.clips_removed, "tracksChanged": d.tracks_changed, "tracksAdded": d.tracks_added, "tracksRemoved": d.tracks_removed, "names": names},
        "revisionBefore": rev0,
        "revisionAfter": s.collab.revision,
        "snapshot": snapshot,
        "project": s.path,
    });
    if let Some(dir) = versions_dir(s) {
        let _ = std::fs::create_dir_all(&dir);
        let line = format!("{entry}\n");
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("activity.jsonl")) {
            let _ = f.write_all(line.as_bytes());
        }
    }
    s.collab.activity.push(entry);
    if s.collab.activity.len() > MAX_ACTIVITY {
        s.collab.activity.remove(0);
    }
}

// ------------------------------------------------------------------------------------------
// versions
// ------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub id: String,
    pub time: String,
    pub label: String,
    pub by: Actor,
    /// Made automatically (before a large agent edit, before a restore).
    pub auto: bool,
    pub revision: u64,
    pub file: String,
    pub clips: usize,
    pub bytes: usize,
}

fn key_of(s: &mut Session) -> String {
    match &s.path {
        Some(p) => {
            let h = crate::user_library::sha256_hex(p.as_bytes());
            let stem = std::path::Path::new(p).file_stem().map(|x| crate::export_tools::file_safe(&x.to_string_lossy())).unwrap_or_default();
            format!("{stem}-{}", h.get(..12).unwrap_or(&h))
        }
        None => s.collab.unsaved_key.get_or_insert_with(|| crate::user_library::new_id("unsaved", &now())).clone(),
    }
}

/// The folder of this project's versions and activity log.
pub fn versions_dir(s: &mut Session) -> Option<PathBuf> {
    let root = crate::user_library::data_dir(s)?;
    Some(root.join("Project Versions").join(key_of(s)))
}

fn read_index(dir: &std::path::Path) -> Vec<Version> {
    crate::user_library::read_json::<Vec<Version>>(&dir.join("versions.json"), "version list").ok().flatten().unwrap_or_default()
}

fn count_clips(p: &Project) -> usize {
    p.sequences().filter_map(|i| i.as_sequence()).map(|q| q.video_tracks.iter().chain(q.audio_tracks.iter()).map(|t| t.items.len()).sum::<usize>()).sum()
}

/// Write `project` as a version; its id.
fn write_version(s: &mut Session, project: &Project, label: &str, by: Actor, auto: bool, revision: u64) -> std::result::Result<String, String> {
    let dir = versions_dir(s).ok_or("no data directory for project versions")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let mut list = read_index(&dir);
    let n = list.iter().filter_map(|v| v.id.strip_prefix('v').and_then(|x| x.parse::<u64>().ok())).max().unwrap_or(0) + 1;
    let id = format!("v{n}");
    let file = format!("{id}.fcproj");
    let bytes = filmcraft_format::encode(project, false);
    crate::user_library::write_atomic(&dir.join(&file), &bytes)?;
    list.push(Version { id: id.clone(), time: now(), label: label.to_string(), by, auto, revision, file, clips: count_clips(project), bytes: bytes.len() });
    // keep the newest; automatic ones go first
    while list.len() > MAX_VERSIONS {
        let i = list.iter().position(|v| v.auto).unwrap_or(0);
        let old = list.remove(i);
        let _ = std::fs::remove_file(dir.join(&old.file));
    }
    crate::user_library::write_json(&dir.join("versions.json"), &list)?;
    Ok(id)
}

/// After Save / Save As: an unsaved project's versions and activity follow it to its file.
pub(crate) fn on_saved(s: &mut Session, old_key: Option<String>) {
    let Some(root) = crate::user_library::data_dir(s) else { return };
    let new_key = key_of(s);
    if let Some(old) = old_key.filter(|k| *k != new_key) {
        let (a, b) = (root.join("Project Versions").join(&old), root.join("Project Versions").join(&new_key));
        if a.is_dir() && !b.exists() {
            let _ = std::fs::rename(&a, &b);
        }
    }
    s.collab.unsaved_key = None;
}

/// The versions key now (for [`on_saved`]).
pub(crate) fn current_key(s: &mut Session) -> Option<String> {
    crate::user_library::data_dir(s)?;
    Some(key_of(s))
}

/// A project was opened or replaced: its activity log comes back, nothing is pending.
pub(crate) fn on_installed(s: &mut Session) {
    s.collab.unsaved_key = None;
    s.collab.disk_sha = None;
    s.collab.requests.clear();
    s.collab.activity.clear();
    s.collab.seen = None;
    if let Some(dir) = versions_dir(s)
        && let Ok(text) = std::fs::read_to_string(dir.join("activity.jsonl"))
    {
        let lines: Vec<&str> = text.lines().collect();
        let from = lines.len().saturating_sub(MAX_ACTIVITY);
        s.collab.activity = lines.get(from..).unwrap_or_default().iter().filter_map(|l| serde_json::from_str(l).ok()).collect();
    }
    observe(s, Actor::User, None);
}

/// The disk fingerprint of the project file, after it was read or written.
pub(crate) fn note_disk(s: &mut Session, bytes: &[u8]) {
    s.collab.disk_sha = Some(crate::user_library::sha256_hex(bytes));
}

/// Saving over `path`: refused when another program changed the file since this session read or
/// wrote it (unless forced).
pub(crate) fn check_disk(s: &mut Session, path: &str) -> Result<()> {
    let force = std::mem::take(&mut s.collab.save_force);
    if force || s.path.as_deref() != Some(path) {
        return Ok(());
    }
    let Some(known) = s.collab.disk_sha.clone() else { return Ok(()) };
    match s.services.read_file(path) {
        Ok(b) if crate::user_library::sha256_hex(&b) != known => Err(EngineError::Other(format!(
            "CONFLICT: {path} was changed on disk by another program since it was opened or saved here. Nothing was saved. Use Save As to keep both, File ▸ Revert to load the other version, or save with force to overwrite it."
        ))),
        _ => Ok(()),
    }
}

// ------------------------------------------------------------------------------------------
// commands
// ------------------------------------------------------------------------------------------

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    let rev = revision(s);
    let seqs: Vec<Value> = s
        .project
        .sequences()
        .filter_map(|i| {
            i.as_sequence().map(|q| {
                json!({"id": i.id.0, "name": i.name, "width": q.settings.width, "height": q.settings.height,
                "clips": q.video_tracks.iter().chain(q.audio_tracks.iter()).map(|t| t.items.len()).sum::<usize>(),
                "captions": q.caption_tracks.iter().map(|t| t.captions.len()).sum::<usize>(), "seconds": q.duration().seconds()})
            })
        })
        .collect();
    let dir = versions_dir(s);
    let versions = dir.as_deref().map(read_index).unwrap_or_default();
    Ok(json!({
        "projectRevision": rev,
        "project": s.project.name,
        "path": s.path,
        "unsavedChanges": s.is_dirty(),
        "activeSequence": s.state.active_sequence.map(|q| q.0),
        "sequences": seqs,
        "protected": {"clips": s.project.protection.clips.iter().map(|c| c.0).collect::<Vec<_>>(), "tracks": s.project.protection.tracks.iter().map(|t| t.0).collect::<Vec<_>>()},
        "pendingAuthorizations": s.collab.requests.iter().filter(|r| r.status == "pending").count(),
        "lastActivity": s.collab.activity.last(),
        "latestVersion": versions.last(),
        "undo": s.history.undo.last().map(|u| u.0.clone()),
        "redo": s.history.redo.last().map(|u| u.0.clone()),
        "recentChanges": s.collab.changes.iter().rev().take(10).collect::<Vec<_>>(),
    }))
}

fn changes(s: &mut Session, p: &Value) -> Result<Value> {
    let rev = revision(s);
    let since = u64_p(p, "since").unwrap_or(0);
    let list: Vec<&Change> = s.collab.changes.iter().filter(|c| c.revision > since).collect();
    Ok(json!({"projectRevision": rev, "changes": list}))
}

fn activity(s: &mut Session, p: &Value) -> Result<Value> {
    let limit = u64_p(p, "limit").unwrap_or(50).min(MAX_ACTIVITY as u64) as usize;
    let from = s.collab.activity.len().saturating_sub(limit);
    Ok(json!({"activity": s.collab.activity.get(from..).unwrap_or_default()}))
}

fn transaction(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "agent.transaction";
    let list = p.get("commands").and_then(Value::as_array).ok_or_else(|| bad(cmd, "need `commands`: [{command, params}]"))?.clone();
    if list.is_empty() || list.len() > 500 {
        return Err(bad(cmd, "1 to 500 commands"));
    }
    // checked here too when the person (or a script) runs it
    if s.collab.actor == Actor::User {
        check_expected(s, u64_p(p, "expectRevision"))?;
    }
    let label = str_p(p, "label").unwrap_or("Changes").to_string();
    s.edit_group(&label, |s| {
        let mut results = Vec::new();
        for (i, c) in list.iter().enumerate() {
            let id = str_p(c, "command").ok_or_else(|| bad(cmd, format!("step {}: need `command`", i + 1)))?;
            if id == cmd {
                return Err(bad(cmd, "transactions cannot be nested"));
            }
            let params = c.get("params").cloned().unwrap_or_else(|| json!({}));
            let v = s
                .execute(id, params)
                .map_err(|e| EngineError::Other(format!("step {} (`{id}`) failed: {e}. Nothing in this transaction was applied.", i + 1)))?;
            results.push(v);
        }
        Ok(json!({ "results": results }))
    })
    .map(|mut v| {
        // inside Claude's wrapper the revision is settled (and reported) after the protection
        // check, which may still roll the transaction back
        if !s.collab.wrapping {
            v["projectRevision"] = json!(revision(s));
        }
        v
    })
}

fn element_ids(p: &Value, key: &str) -> Vec<u64> {
    p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default()
}

/// Every clip, caption and track id in the project, by kind.
fn all_ids(p: &Project) -> (BTreeSet<u64>, BTreeSet<u64>) {
    let (mut clips, mut tracks) = (BTreeSet::new(), BTreeSet::new());
    for q in p.sequences().filter_map(|i| i.as_sequence()) {
        for t in q.video_tracks.iter().chain(q.audio_tracks.iter()) {
            tracks.insert(t.id.0);
            clips.extend(t.items.iter().map(|c| c.id.0));
        }
        for t in &q.caption_tracks {
            tracks.insert(t.id.0);
            clips.extend(t.captions.iter().map(|c| c.id.0));
        }
    }
    (clips, tracks)
}

fn protect_set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "protect.set";
    let on = bool_p(p, "on").unwrap_or(true);
    if !on && s.collab.actor == Actor::Agent {
        return Err(EngineError::Other("NOT_ALLOWED: only the person can remove a protection.".into()));
    }
    let mut clips = element_ids(p, "clips");
    let tracks = element_ids(p, "tracks");
    if clips.is_empty() && tracks.is_empty() {
        clips = s.state.selection.iter().map(|c| c.0).chain(s.state.caption_selection.iter().map(|c| c.0)).collect();
    }
    if clips.is_empty() && tracks.is_empty() {
        return Err(bad(cmd, "select the clips to protect (or give `clips` / `tracks`)"));
    }
    let (known_clips, known_tracks) = all_ids(&s.project);
    if let Some(x) = clips.iter().find(|c| !known_clips.contains(c)) {
        return Err(bad(cmd, format!("no clip or caption {x}")));
    }
    if let Some(x) = tracks.iter().find(|t| !known_tracks.contains(t)) {
        return Err(bad(cmd, format!("no track {x}")));
    }
    let label = if on { "Protect from Claude" } else { "Remove Protection" };
    s.edit(label, |pr, _| {
        for c in &clips {
            if on {
                pr.protection.clips.insert(ClipId(*c))
            } else {
                pr.protection.clips.remove(&ClipId(*c))
            };
        }
        for t in &tracks {
            if on {
                pr.protection.tracks.insert(TrackId(*t))
            } else {
                pr.protection.tracks.remove(&TrackId(*t))
            };
        }
        Ok(())
    })?;
    Ok(json!({"on": on, "clips": clips, "tracks": tracks}))
}

/// Protect the selected clips from Claude, or (when they all are) remove their protection.
fn protect_toggle(s: &mut Session, _: &Value) -> Result<Value> {
    let sel: Vec<u64> = s.state.selection.iter().map(|c| c.0).collect();
    if sel.is_empty() {
        return Err(bad("protect.toggle", "select the clips to protect"));
    }
    let all = sel.iter().all(|c| s.project.protection.clips.contains(&ClipId(*c)));
    protect_set(s, &json!({"clips": sel, "on": !all}))
}

fn has_selection(s: &Session) -> std::result::Result<(), String> {
    if s.state.selection.is_empty() { Err("select clips in the Timeline".into()) } else { Ok(()) }
}

/// Whether a clip is protected, by itself or by its track.
pub fn is_protected(s: &Session, clip: ClipId) -> bool {
    let p = &s.project.protection;
    if p.clips.contains(&clip) {
        return true;
    }
    s.active_sequence().and_then(|q| q.find_item(clip)).is_some_and(|(t, _)| p.tracks.contains(&t))
}

fn protect_list(s: &mut Session, _: &Value) -> Result<Value> {
    let d = diff(&Project::new(""), &s.project);
    let name = |i: &u64| d.names.get(i).cloned().unwrap_or_else(|| format!("{i} (not in any sequence)"));
    let clips: Vec<Value> = s.project.protection.clips.iter().map(|c| json!({"id": c.0, "name": name(&c.0)})).collect();
    let tracks: Vec<Value> = s.project.protection.tracks.iter().map(|t| json!({"id": t.0, "name": name(&t.0)})).collect();
    Ok(json!({"clips": clips, "tracks": tracks}))
}

fn request_authorization(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "agent.requestAuthorization";
    let reason = str_p(p, "reason").filter(|r| !r.trim().is_empty()).ok_or_else(|| bad(cmd, "need `reason`: what you want to change and why"))?.to_string();
    let mut ids = element_ids(p, "elements");
    ids.extend(element_ids(p, "clips"));
    ids.extend(element_ids(p, "tracks"));
    let (known_clips, known_tracks) = all_ids(&s.project);
    let clips: Vec<u64> = ids.iter().copied().filter(|i| known_clips.contains(i)).collect();
    let tracks: Vec<u64> = ids.iter().copied().filter(|i| known_tracks.contains(i)).collect();
    if clips.is_empty() && tracks.is_empty() {
        return Err(bad(cmd, "need `elements`: the protected clip and track ids"));
    }
    let d = diff(&Project::new(""), &s.project);
    let names = clips.iter().chain(tracks.iter()).map(|i| d.names.get(i).cloned().unwrap_or_else(|| i.to_string())).collect();
    s.collab.next_request += 1;
    let id = s.collab.next_request;
    let token = crate::user_library::sha256_hex(format!("{id}-{}-{:?}", now(), std::time::SystemTime::now()).as_bytes());
    s.collab.requests.push(AuthRequest { id, time: now(), clips, tracks, names, reason, status: "pending".into(), token });
    s.toast("Claude asks to change protected work: see the Changes panel");
    Ok(json!({"request": id, "status": "pending", "next": "the person answers in FilmCraft; poll agent.authorizationStatus {request}"}))
}

fn authorization_status(s: &mut Session, p: &Value) -> Result<Value> {
    let id = u64_p(p, "request").ok_or_else(|| bad("agent.authorizationStatus", "need `request`"))?;
    let r = s.collab.requests.iter().find(|r| r.id == id).ok_or_else(|| bad("agent.authorizationStatus", "no such request"))?;
    let mut v = json!({"request": id, "status": r.status});
    if r.status == "allowed" {
        v["authorization"] = json!(r.token);
        v["use"] = json!(
            "pass it as `authorization` with the command (control channel), or in agent.transaction; it is good for one successful edit of these elements"
        );
    }
    Ok(v)
}

fn authorize(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "agent.authorize";
    if s.collab.actor == Actor::Agent {
        return Err(EngineError::Other("NOT_ALLOWED: only the person can answer Claude's request.".into()));
    }
    let id = u64_p(p, "request").ok_or_else(|| bad(cmd, "need `request`"))?;
    let allow = bool_p(p, "allow").ok_or_else(|| bad(cmd, "need `allow`"))?;
    let r = s.collab.requests.iter_mut().find(|r| r.id == id && r.status == "pending").ok_or_else(|| bad(cmd, "no pending request with that id"))?;
    r.status = if allow { "allowed" } else { "denied" }.into();
    Ok(json!({"request": id, "status": r.status}))
}

fn requests(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"requests": s.collab.requests}))
}

fn versions_list(s: &mut Session, _: &Value) -> Result<Value> {
    let dir = versions_dir(s);
    let list = dir.as_deref().map(read_index).unwrap_or_default();
    Ok(json!({"folder": dir, "versions": list}))
}

fn versions_create(s: &mut Session, p: &Value) -> Result<Value> {
    let label = str_p(p, "label").filter(|l| !l.trim().is_empty()).unwrap_or("Saved version").to_string();
    let rev = revision(s);
    let by = s.collab.actor;
    let project = s.project.clone();
    let id = write_version(s, &project, &label, by, false, rev).map_err(EngineError::Other)?;
    Ok(json!({"version": id, "label": label, "projectRevision": rev}))
}

fn versions_restore(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "versions.restore";
    let id = str_p(p, "version").ok_or_else(|| bad(cmd, "need `version` (see versions.list)"))?.to_string();
    let dir = versions_dir(s).ok_or_else(|| bad(cmd, "no data directory"))?;
    let v = read_index(&dir).into_iter().find(|v| v.id == id).ok_or_else(|| bad(cmd, format!("no version `{id}`")))?;
    let bytes = std::fs::read(dir.join(&v.file)).map_err(|e| EngineError::Other(format!("version {id}: {e}")))?;
    let mut restored = filmcraft_format::decode(&bytes).map_err(|e| EngineError::Other(format!("version {id} is damaged: {e}")))?.project;
    // the file the project is saved as keeps its name
    restored.name.clone_from(&s.project.name);
    restored.root.name.clone_from(&s.project.root.name);
    // the current state first, so restoring can itself be undone after the undo history is gone
    let rev = revision(s);
    let current = s.project.clone();
    let by = s.collab.actor;
    let safety = write_version(s, &current, &format!("Before restoring \"{}\"", v.label), by, true, rev).ok();
    s.edit(&format!("Restore Version \"{}\"", v.label), |pr, st| {
        *pr = restored;
        if st.active_sequence.is_none_or(|q| pr.sequence(q).is_none()) {
            st.active_sequence = pr.sequences().next().map(|i| i.id);
        }
        st.open_sequences.retain(|q| pr.sequence(*q).is_some());
        if let Some(q) = st.active_sequence
            && !st.open_sequences.contains(&q)
        {
            st.open_sequences.push(q);
        }
        st.selection.clear();
        Ok(())
    })?;
    let path = s.path.clone();
    s.media.add_stored(&s.project, path.as_deref());
    if let Some(q) = s.state.active_sequence {
        s.events.push(crate::Event::OpenSequence(q));
    }
    Ok(json!({"restored": id, "label": v.label, "safetyVersion": safety, "undo": "edit.undo brings back what was there before"}))
}

/// The collaboration commands.
pub(crate) fn commands() -> Vec<CommandSpec> {
    let q = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    let e = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: true };
    vec![
        q(
            "agent.status",
            "Project Status for Claude",
            "{} → projectRevision, unsaved changes, sequences, protection, last activity, latest version, undo/redo, recent changes",
            status,
        ),
        q("agent.changes", "Changes Since a Revision", r#"{"since":revision=0}"#, changes),
        q("agent.activity", "Claude's Activity", r#"{"limit":n=50}"#, activity),
        e(
            "agent.transaction",
            "Transaction",
            r#"{"label":str,"commands":[{"command":id,"params":{}}],"expectRevision":n?,"snapshot":bool=true} (all or nothing; one undo step)"#,
            transaction,
        ),
        e(
            "protect.set",
            "Protect from Claude",
            r#"{"clips":[id]?,"tracks":[id]?,"on":bool=true} (default: the selected clips; only the person can remove a protection)"#,
            protect_set,
        ),
        CommandSpec {
            id: "protect.toggle",
            label: "Protect from Claude",
            menu: &["Clip"],
            shortcut: None,
            params: "{} (the selected clips; when all are protected, removes their protection)",
            enabled: has_selection,
            run: protect_toggle,
            journal: true,
        },
        q("protect.list", "Protected Work", "{}", protect_list),
        q("agent.requestAuthorization", "Ask to Change Protected Work", r#"{"elements":[clip or track id],"reason":str}"#, request_authorization),
        q("agent.authorizationStatus", "Authorization Status", r#"{"request":n}"#, authorization_status),
        q("agent.authorize", "Answer Claude's Request", r#"{"request":n,"allow":bool} (the person only)"#, authorize),
        q("agent.requests", "Claude's Requests", "{}", requests),
        q("versions.list", "Project Versions", "{}", versions_list),
        q("versions.create", "Save a Version", r#"{"label":str?}"#, versions_create),
        e("versions.restore", "Restore Version", r#"{"version":"vN"} (undoable; the current state is kept as a version first)"#, versions_restore),
    ]
}
