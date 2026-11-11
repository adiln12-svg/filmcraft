//! Creator Mode (M3A): a simple workspace for short-form video on the same project, timeline,
//! commands and undo history as the professional workspaces.
//!
//! - The **Creator** workspace: Assets (media, music, text, captions, transitions, looks) |
//!   Program monitor | Properties, and the Timeline with a toolbar of plain-language actions
//!   (Split, Trim start, Trim end, Delete, Replace, Add media, Add text, Export).
//! - The header's **Creator / Pro** switch: Pro goes back to the workspace used before.
//! - Files dropped on the Start screen start a new video; dropped in Creator Mode they are added
//!   at the end of the video in file-name order.
//! - **Export video…**: social presets, a file name without extension, a destination folder.
//!
//! Every action is an engine command (`creator.*`, `sequence.addEdit`, `clip.replaceFromBin`, …), so
//! agents and the Pro workspace see the same edits. See `docs/creator-mode.md`.

use egui::{Align2, Rect, Sense, Stroke, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::native_dialogs::FileDialog;
use crate::state::Mode;
use crate::theme::Tokens;

/// The Creator workspace's name.
pub const WORKSPACE: &str = "Creator";
/// Height of the Creator toolbar above the Timeline.
pub const TOOLBAR_H: f32 = 40.0;

/// The Assets panel's tabs: key, label.
pub const TABS: [(&str, &str); 6] =
    [("media", "Media"), ("music", "Music"), ("text", "Text"), ("captions", "Captions"), ("transitions", "Transitions"), ("looks", "Looks")];

/// Creator Mode's interface state (serde: the control channel reads and sets it).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// The aspect a drop on the Start screen uses.
    pub aspect: String,
    /// The workspace Pro returns to.
    pub pro_workspace: String,
    /// The Assets panel's tab (a key of [`TABS`]).
    pub tab: String,
    pub title_text: String,
    pub caption_text: String,
    pub export: ExportDialog,
}

impl Default for State {
    fn default() -> Self {
        Self {
            aspect: "9:16".into(),
            pro_workspace: "Editing".into(),
            tab: "media".into(),
            title_text: String::new(),
            caption_text: String::new(),
            export: ExportDialog::default(),
        }
    }
}

/// The Export video dialog.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportDialog {
    pub open: bool,
    /// A `creator.exportPresets` id.
    pub preset: String,
    /// File name without extension.
    pub name: String,
    pub folder: String,
    pub burn_captions: bool,
    /// Hardware encoding + GPU rendering.
    pub fast: bool,
    /// The running or finished export job.
    pub job: Option<u64>,
    pub path: String,
    pub error: String,
}

/// Whether the Creator workspace is in use.
pub fn is_on(app: &FilmcraftApp) -> bool {
    app.ui.workspace == WORKSPACE
}

/// Switch to the Creator workspace (remembering the Pro one) and to editing.
pub fn enter(app: &mut FilmcraftApp) {
    if !is_on(app) {
        app.ui.creator.pro_workspace = app.ui.workspace.clone();
        app.set_workspace(WORKSPACE);
    }
    app.ui.mode = Mode::Edit;
}

/// Back to the professional workspace used before Creator Mode.
pub fn leave(app: &mut FilmcraftApp) {
    let back = Some(app.ui.creator.pro_workspace.clone()).filter(|w| !w.is_empty() && w != WORKSPACE).unwrap_or_else(|| "Editing".into());
    app.set_workspace(&back);
}

fn media_exts() -> Vec<&'static str> {
    filmcraft_media::VIDEO_EXTENSIONS.iter().chain(filmcraft_media::AUDIO_EXTENSIONS).chain(filmcraft_media::STILL_EXTENSIONS).copied().collect()
}

/// `creator.newVideo` for `paths` at `aspect`, then into the Creator workspace's editing.
pub fn new_video(app: &mut FilmcraftApp, aspect: &str, paths: Vec<String>) -> Result<Value, String> {
    let r = app.session.execute("creator.newVideo", json!({"aspect": aspect, "paths": paths})).map_err(|e| e.to_string())?;
    enter(app);
    app.ui.timeline.fit_pending = true;
    let n = r["clips"].as_array().map_or(0, Vec::len);
    app.ui.status = if n > 0 {
        tlf!("New {aspect} video with {n} clips in file-name order", aspect = aspect, n = r["order"].as_array().map_or(0, Vec::len))
    } else {
        tlf!("New {aspect} video: add clips with Add media or drop files on the window", aspect = aspect)
    };
    Ok(r)
}

/// Add `paths` at the end of the video (in file-name order).
pub fn add_paths(app: &mut FilmcraftApp, paths: Vec<String>) -> Result<Value, String> {
    if paths.is_empty() {
        return Ok(Value::Null);
    }
    if app.session.active_sequence().is_none() {
        let aspect = app.ui.creator.aspect.clone();
        return new_video(app, &aspect, paths);
    }
    let r = app.session.execute("creator.addMedia", json!({ "paths": paths })).map_err(|e| e.to_string())?;
    // the whole video stays in view
    app.ui.timeline.fit_pending = true;
    let clips = r["order"].as_array().map_or(0, Vec::len);
    let music = r["music"].as_array().map_or(0, Vec::len);
    app.ui.status = match (clips, music) {
        (0, _) => tl!("Added the music from the start of the video").into(),
        (n, 0) => tlf!("Added {n} clips at the end of the video", n = n),
        (n, m) => tlf!("Added {n} clips at the end of the video and {m} music files", n = n, m = m),
    };
    Ok(r)
}

/// A file drop (or `ui.dropFiles`) the Start screen or Creator Mode takes: true when handled.
pub fn take_drop(app: &mut FilmcraftApp, paths: &[String], pointer: Option<egui::Pos2>) -> bool {
    let r = if app.ui.mode == Mode::Import {
        let aspect = app.ui.creator.aspect.clone();
        new_video(app, &aspect, paths.to_vec())
    } else if app.ui.mode == Mode::Edit && is_on(app) && !over_timeline(app, pointer) {
        add_paths(app, paths.to_vec())
    } else {
        return false;
    };
    if let Err(e) = r {
        app.ui.status = e;
    }
    true
}

fn over_timeline(app: &FilmcraftApp, pointer: Option<egui::Pos2>) -> bool {
    let Some(p) = pointer else { return false };
    app.session.active_sequence().is_some()
        && app.auto.find(&format!("panel.{}", crate::dock::PanelKind::Timeline.id())).is_some_and(|e| {
            let r = Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]));
            r.contains(p) && p.y > r.min.y + TOOLBAR_H
        })
}

/// UI commands of Creator Mode (menus, shortcuts, control channel).
pub fn route(app: &mut FilmcraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    let s = |k: &str| params.get(k).and_then(Value::as_str).map(str::to_string);
    Some(match id {
        "view.creatorMode" => {
            let on = params.get("on").and_then(Value::as_bool).unwrap_or(!is_on(app));
            if on {
                enter(app)
            } else {
                leave(app)
            }
            Ok(json!({"creator": is_on(app), "workspace": app.ui.workspace}))
        }
        "creator.start" => {
            app.ui.mode = Mode::Import;
            Ok(Value::Null)
        }
        "creator.chooseClips" => {
            let aspect = s("aspect").unwrap_or_else(|| app.ui.creator.aspect.clone());
            app.ui.creator.aspect = aspect.clone();
            let paths: Option<Vec<String>> =
                params.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect());
            match paths {
                Some(p) => new_video(app, &aspect, p),
                None => app.pick(FileDialog::open_files(&media_exts()), move |app, paths| new_video(app, &aspect, paths)),
            }
        }
        "creator.addMediaDialog" => {
            let exts: Vec<&str> =
                if params.get("music").and_then(Value::as_bool) == Some(true) { filmcraft_media::AUDIO_EXTENSIONS.to_vec() } else { media_exts() };
            app.pick(FileDialog::open_files(&exts), add_paths)
        }
        "creator.replaceDialog" => {
            if app.session.state.selection.is_empty() {
                return Some(Err(tl!("Select the clip to replace first").into()));
            }
            app.pick(FileDialog::open_files(&media_exts()), replace_with)
        }
        "creator.exportDialog" if params.get("close").and_then(Value::as_bool) == Some(true) => {
            app.ui.creator.export.open = false;
            Ok(json!({"open": false}))
        }
        "creator.exportDialog" => {
            open_export(app);
            Ok(serde_json::to_value(&app.ui.creator.export).unwrap_or_default())
        }
        "creator.chooseExportFolder" => {
            let at = Some(app.ui.creator.export.folder.clone()).filter(|f| !f.is_empty());
            app.pick(FileDialog::Folder { at }, |app, paths| {
                if let Some(f) = paths.into_iter().next() {
                    app.ui.creator.export.folder = f;
                }
                Ok(json!({"folder": app.ui.creator.export.folder}))
            })
        }
        "creator.exportNow" => export_now(app),
        _ => return None,
    })
}

/// Replace the selected clip with the first of `paths` (imported first), keeping its length.
fn replace_with(app: &mut FilmcraftApp, paths: Vec<String>) -> Result<Value, String> {
    let Some(path) = paths.into_iter().next() else { return Ok(Value::Null) };
    let r = app.session.execute("file.import", json!({ "paths": [path] })).map_err(|e| e.to_string())?;
    let item = r["items"].as_array().and_then(|a| a.first()).and_then(Value::as_u64).ok_or_else(|| tl!("The file could not be imported").to_string())?;
    let clips: Vec<u64> = app.session.state.selection.iter().map(|c| c.0).collect();
    app.session.execute("clip.replaceFromBin", json!({"clips": clips, "item": item})).map_err(|e| e.to_string())
}

fn open_export(app: &mut FilmcraftApp) {
    let presets = app.session.execute("creator.exportPresets", json!({})).unwrap_or_default();
    let ex = &mut app.ui.creator.export;
    let fresh = !ex.open;
    ex.open = true;
    if fresh || ex.preset.is_empty() {
        ex.preset = presets["suggested"].as_str().unwrap_or("tiktok").to_string();
    }
    if ex.folder.is_empty() {
        ex.folder = presets["folder"].as_str().unwrap_or_default().to_string();
    }
    if fresh {
        ex.error.clear();
        ex.job = None;
        ex.path.clear();
        ex.fast = cfg!(target_os = "macos");
        ex.burn_captions = app.session.active_sequence().is_some_and(|q| q.caption_tracks.iter().any(|t| !t.captions.is_empty()));
        let seq = app.session.state.active_sequence.and_then(|q| app.session.project.item(q)).map(|i| i.name.clone()).unwrap_or_else(|| "My Video".into());
        ex.name = filmcraft_engine::creator::strip_media_extension(&seq).to_string();
    }
}

fn export_now(app: &mut FilmcraftApp) -> Result<Value, String> {
    let ex = app.ui.creator.export.clone();
    let r = app
        .session
        .execute(
            "creator.export",
            json!({"preset": ex.preset, "name": ex.name, "folder": ex.folder, "burnCaptions": ex.burn_captions, "hardwareAcceleration": ex.fast}),
        )
        .map_err(|e| e.to_string());
    match &r {
        Ok(v) => {
            app.ui.creator.export.job = v["job"].as_u64();
            app.ui.creator.export.path = v["path"].as_str().unwrap_or_default().to_string();
            app.ui.creator.export.error.clear();
        }
        Err(e) => app.ui.creator.export.error = e.clone(),
    }
    r
}

// ------------------------------------------------------------------------------------------
// Assets panel
// ------------------------------------------------------------------------------------------

enum Act {
    Cmd(&'static str, Value),
    Ui(&'static str, Value),
}

/// The Assets panel: tabs of what a short video is made from.
pub fn assets_panel(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    ui.painter().rect_filled(rect, 0.0, t.panel_bg);
    let mut acts = Vec::new();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(8.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut child;
    ui.horizontal_wrapped(|ui| {
        for (key, label) in TABS {
            let on = app.ui.creator.tab == key;
            let r = ui.selectable_label(on, crate::i18n::t(label));
            app.auto.add_response(&format!("creator.assets.tab.{key}"), &r, label);
            if r.clicked() {
                app.ui.creator.tab = key.to_string();
            }
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().id_salt("creator-assets").auto_shrink([false, false]).show(ui, |ui| match app.ui.creator.tab.as_str() {
        "music" => media_tab(app, ui, true, &mut acts),
        "text" => text_tab(app, ui, &mut acts),
        "captions" => captions_tab(app, ui, &mut acts),
        "transitions" => transitions_tab(app, ui, &mut acts),
        "looks" => looks_tab(app, ui, &mut acts),
        _ => media_tab(app, ui, false, &mut acts),
    });
    run(app, ui.ctx(), acts);
}

fn run(app: &mut FilmcraftApp, ctx: &egui::Context, acts: Vec<Act>) {
    for a in acts {
        let r = match a {
            Act::Cmd(id, p) => {
                let fit = id == "creator.addMedia";
                let r = app.session.execute(id, p).map_err(|e| e.to_string());
                app.ui.timeline.fit_pending |= fit && r.is_ok();
                r
            }
            Act::Ui(id, p) => crate::menus::invoke(app, ctx, id, p),
        };
        if let Err(e) = r {
            app.ui.status = e;
        }
    }
}

fn hint(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.label(egui::RichText::new(text).size(11.5).color(t.text_dim));
}

fn big_button(app: &mut FilmcraftApp, ui: &mut egui::Ui, id: &str, label: &str, enabled: bool) -> bool {
    let t = app.tokens;
    let b = egui::Button::new(egui::RichText::new(label).size(13.0).color(egui::Color32::WHITE))
        .fill(t.accent)
        .min_size(vec2(ui.available_width().min(260.0), 30.0));
    let r = ui.add_enabled(enabled, b);
    app.auto.add_response(id, &r, label);
    r.clicked()
}

fn small_button(app: &mut FilmcraftApp, ui: &mut egui::Ui, id: &str, label: &str, enabled: bool) -> bool {
    let r = ui.add_enabled(enabled, egui::Button::new(label));
    app.auto.add_response(id, &r, label);
    r.clicked()
}

fn media_tab(app: &mut FilmcraftApp, ui: &mut egui::Ui, music: bool, acts: &mut Vec<Act>) {
    let t = app.tokens;
    let has_seq = app.session.active_sequence().is_some();
    let (id, label) = if music { ("creator.assets.addMusic", tl!("Add music…")) } else { ("creator.assets.addMedia", tl!("Add media…")) };
    if big_button(app, ui, id, label, true) {
        acts.push(Act::Ui("creator.addMediaDialog", json!({ "music": music })));
    }
    hint(
        ui,
        &t,
        if music {
            tl!("Music starts at the beginning of the video on its own track. Select it to change its volume.")
        } else {
            tl!("Clips are added at the end of the video in file-name order. You can also drop files on the window.")
        },
    );
    ui.add_space(6.0);
    let mut ids = Vec::new();
    app.session.project.root.all_items(&mut ids);
    let items: Vec<(u64, String, bool, f64)> = ids
        .into_iter()
        .filter_map(|id| app.session.project.item(id).map(|it| (id.0, it.name.clone(), it.has_video(), it.duration().seconds())))
        .filter(|(id, _, video, _)| {
            let media = app.session.project.item(filmcraft_engine::project::ItemId(*id)).is_some_and(|it| it.as_media().is_some());
            media && (*video != music)
        })
        .collect();
    if items.is_empty() {
        hint(ui, &t, if music { tl!("No music in this project yet.") } else { tl!("No clips in this project yet.") });
    }
    let selected = !app.session.state.selection.is_empty();
    for (i, (id, name, _, secs)) in items.iter().enumerate() {
        ui.horizontal(|ui| {
            let icon = if music { crate::icons::Icon::Speaker } else { crate::icons::Icon::Film };
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            crate::icons::paint(ui.painter(), r, icon, t.text_dim);
            ui.add(egui::Label::new(format!("{name}  ·  {secs:.1}s")).truncate()).on_hover_text(name.as_str());
            let key = if music { "music" } else { "media" };
            if small_button(app, ui, &format!("creator.assets.{key}.{i}.add"), tl!("Add"), has_seq) {
                acts.push(Act::Cmd("creator.addMedia", json!({ "items": [id] })));
            }
            if !music && small_button(app, ui, &format!("creator.assets.{key}.{i}.replace"), tl!("Replace selected"), selected) {
                let clips: Vec<u64> = app.session.state.selection.iter().map(|c| c.0).collect();
                acts.push(Act::Cmd("clip.replaceFromBin", json!({"clips": clips, "item": id})));
            }
        });
    }
}

fn text_tab(app: &mut FilmcraftApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let t = app.tokens;
    hint(ui, &t, tl!("A title appears at the playhead for 3 seconds. Select it to change its font, size and colour."));
    let mut text = app.ui.creator.title_text.clone();
    let r = ui.add(egui::TextEdit::singleline(&mut text).hint_text(tl!("Your title")).desired_width(ui.available_width().min(260.0)));
    app.auto.add_response("creator.assets.titleText", &r, tl!("Title text"));
    app.ui.creator.title_text = text.clone();
    let has_seq = app.session.active_sequence().is_some();
    if big_button(app, ui, "creator.assets.addTitle", tl!("Add title"), has_seq) {
        let text = if text.trim().is_empty() { tl!("Your title").to_string() } else { text };
        acts.push(Act::Cmd("creator.addTitle", json!({"text": text, "seconds": 3})));
    }
}

fn captions_tab(app: &mut FilmcraftApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let t = app.tokens;
    hint(ui, &t, tl!("A caption appears at the playhead for 3 seconds. Captions are burned into the exported video."));
    let mut text = app.ui.creator.caption_text.clone();
    let r = ui.add(egui::TextEdit::multiline(&mut text).hint_text(tl!("What is said")).desired_rows(2).desired_width(ui.available_width().min(260.0)));
    app.auto.add_response("creator.assets.captionText", &r, tl!("Caption text"));
    app.ui.creator.caption_text = text.clone();
    let has_seq = app.session.active_sequence().is_some();
    if big_button(app, ui, "creator.assets.addCaption", tl!("Add caption"), has_seq && !text.trim().is_empty()) {
        acts.push(Act::Cmd("captions.add", json!({"text": text.trim(), "durationSeconds": 3})));
        app.ui.creator.caption_text.clear();
    }
    ui.add_space(8.0);
    if small_button(app, ui, "creator.assets.autoCaptions", tl!("Captions from speech"), has_seq) {
        acts.push(Act::Cmd("transcript.createCaptions", json!({})));
    }
    hint(ui, &t, tl!("Needs a speech-to-text model (Settings ▸ Media Analysis & Transcription)."));
}

fn transitions_tab(app: &mut FilmcraftApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let t = app.tokens;
    hint(ui, &t, tl!("Adds a short transition at every cut between clips. Undo removes them all."));
    let has_seq = app.session.active_sequence().is_some();
    for (key, label) in [("cross_dissolve", tl!("Cross dissolve at every cut")), ("dip_to_black", tl!("Dip to black at every cut"))] {
        if big_button(app, ui, &format!("creator.assets.transition.{key}"), label, has_seq) {
            acts.push(Act::Cmd("creator.transitions", json!({ "effect": key })));
        }
    }
}

fn looks_tab(app: &mut FilmcraftApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let t = app.tokens;
    hint(ui, &t, tl!("A look changes the colour of the selected clips, or of every clip when none is selected."));
    let has_seq = app.session.active_sequence().is_some();
    let targets: Vec<u64> = if app.session.state.selection.is_empty() {
        app.session.active_sequence().map(|q| q.video_tracks.iter().flat_map(|tr| tr.items.iter().map(|i| i.id.0)).collect()).unwrap_or_default()
    } else {
        app.session.state.selection.iter().map(|c| c.0).collect()
    };
    let mut folder = "";
    for p in filmcraft_render::lumetri_presets::presets() {
        if p.folder != folder {
            folder = p.folder;
            ui.label(egui::RichText::new(crate::i18n::t(folder)).strong());
        }
        let r = ui.add_enabled(has_seq && !targets.is_empty(), egui::Button::new(crate::i18n::t(p.name)).min_size(vec2(ui.available_width().min(260.0), 24.0)));
        let r = r.on_hover_text(crate::i18n::t(p.description));
        app.auto.add_response(&format!("creator.assets.look.{}", p.name), &r, p.name);
        if r.clicked() {
            acts.push(Act::Cmd("lumetri.applyPreset", json!({"name": p.name, "clips": targets})));
        }
    }
}

// ------------------------------------------------------------------------------------------
// Timeline toolbar
// ------------------------------------------------------------------------------------------

/// The Creator toolbar at the top of the Timeline panel: `rect` is the toolbar's strip.
pub fn toolbar(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    ui.painter().rect_filled(rect, 0.0, t.tl_header_bg);
    ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.separator));
    let has_seq = app.session.active_sequence().is_some();
    let sel = !app.session.state.selection.is_empty();
    let can_undo = !app.session.history.undo.is_empty();
    let can_redo = !app.session.history.redo.is_empty();
    // id, label, tooltip, command, params, enabled, ui command
    let buttons: [(&str, &str, &str, &'static str, Value, bool, bool); 10] = [
        ("undo", tl!("Undo"), tl!("Undo the last change"), "edit.undo", json!({}), can_undo, false),
        ("redo", tl!("Redo"), tl!("Redo"), "edit.redo", json!({}), can_redo, false),
        ("split", tl!("Split"), tl!("Cut the clip in two at the playhead"), "sequence.addEdit", json!({}), has_seq, false),
        (
            "trimStart",
            tl!("Trim start"),
            tl!("Remove the part of the clip before the playhead"),
            "creator.trimToPlayhead",
            json!({"side": "start"}),
            has_seq,
            false,
        ),
        ("trimEnd", tl!("Trim end"), tl!("Remove the part of the clip after the playhead"), "creator.trimToPlayhead", json!({"side": "end"}), has_seq, false),
        ("delete", tl!("Delete"), tl!("Delete the selected clips and close the gap"), "creator.delete", json!({}), sel, false),
        ("replace", tl!("Replace…"), tl!("Replace the selected clip with another file"), "creator.replaceDialog", json!({}), sel, true),
        ("addMedia", tl!("+ Media"), tl!("Add clips at the end of the video"), "creator.addMediaDialog", json!({}), true, true),
        ("addText", tl!("+ Text"), tl!("Add a title at the playhead"), "creator.addTitle", json!({"text": tl!("Your title"), "seconds": 3}), has_seq, false),
        ("pro", tl!("Pro"), tl!("Switch to the professional workspace (same project and timeline)"), "view.creatorMode", json!({"on": false}), true, true),
    ];
    let mut x = rect.min.x + 8.0;
    let mut fire: Option<(&'static str, Value, bool)> = None;
    for (id, label, tip, cmd, params, enabled, is_ui) in buttons {
        if id == "addMedia" || id == "pro" {
            x += 10.0;
        }
        let g = ui.painter().layout_no_wrap(label.to_string(), Tokens::ui(13.0), t.text);
        let r = Rect::from_min_size(pos2(x, rect.min.y + 6.0), vec2(g.size().x + 20.0, rect.height() - 12.0));
        x = r.max.x + 6.0;
        let resp = ui.interact(r, egui::Id::unique(("creator-tb", id)), if enabled { Sense::click() } else { Sense::hover() }).on_hover_text(tip);
        app.auto.add(&format!("creator.toolbar.{id}"), r, label);
        let bg = if !enabled {
            t.panel_bg
        } else if resp.hovered() {
            t.hover
        } else {
            t.control_surface
        };
        ui.painter().rect_filled(r, 5.0, bg);
        ui.painter().galley(pos2(r.min.x + 10.0, r.center().y - g.size().y / 2.0), g, if enabled { t.text } else { t.text_faint });
        if enabled && resp.clicked() {
            fire = Some((cmd, params, is_ui));
        }
    }
    // Export: the primary action, on the right
    let label = tl!("Export video");
    let g = ui.painter().layout_no_wrap(label.to_string(), Tokens::semibold(13.0), egui::Color32::WHITE);
    let r = Rect::from_min_size(pos2(rect.max.x - g.size().x - 36.0, rect.min.y + 5.0), vec2(g.size().x + 28.0, rect.height() - 10.0));
    let resp = ui.interact(r, egui::Id::unique("creator-tb-export"), if has_seq { Sense::click() } else { Sense::hover() });
    app.auto.add("creator.toolbar.export", r, label);
    ui.painter().rect_filled(r, 6.0, if resp.hovered() && has_seq { t.accent_hover } else { t.accent });
    ui.painter().galley(pos2(r.min.x + 14.0, r.center().y - g.size().y / 2.0), g, egui::Color32::WHITE);
    if has_seq && resp.clicked() {
        fire = Some(("creator.exportDialog", json!({}), true));
    }
    // what can be done with the mouse, where there is room
    if x + 40.0 < r.min.x {
        let p = ui.painter().with_clip_rect(Rect::from_min_max(pos2(x + 8.0, rect.min.y), pos2(r.min.x - 8.0, rect.max.y)));
        p.text(
            pos2(x + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            tl!("Drag clips to move them · drag a clip's edge to trim · drop files to add"),
            Tokens::ui(11.5),
            t.text_dim,
        );
    }
    if let Some((cmd, params, is_ui)) = fire {
        let ctx = ui.ctx().clone();
        let r = if is_ui { crate::menus::invoke(app, &ctx, cmd, params) } else { app.session.execute(cmd, params).map_err(|e| e.to_string()) };
        if let Err(e) = r {
            app.ui.status = e;
        }
    }
}

// ------------------------------------------------------------------------------------------
// Export video dialog
// ------------------------------------------------------------------------------------------

/// The job's state: (fraction done, finished, error, written path).
fn job_state(app: &FilmcraftApp, id: u64) -> Option<(f32, bool, Option<String>)> {
    let j = app.session.jobs.iter().find(|j| j.id == id)?;
    let v = j.to_json();
    let done = j.progress.finished.load(std::sync::atomic::Ordering::Relaxed);
    Some((j.progress.fraction().clamp(0.0, 1.0), done, v["result"]["error"].as_str().map(str::to_string)))
}

/// The Export video window (drawn every frame; shown while `open`).
pub fn export_window(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.ui.creator.export.open {
        return;
    }
    let mut open = true;
    let mut act: Option<&'static str> = None;
    let w = egui::Window::new(tl!("Export video"))
        .id(egui::Id::unique("creator-export"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .default_width(440.0)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| export_body(app, ui, &mut act));
    if let Some(w) = w {
        app.auto.add_response("creator.export.window", &w.response, tl!("Export video"));
    }
    if !open {
        app.ui.creator.export.open = false;
    }
    if let Some(id) = act {
        let r = crate::menus::invoke(app, ctx, id, json!({}));
        if let Err(e) = r {
            app.ui.creator.export.error = e;
        }
    }
}

fn export_body(app: &mut FilmcraftApp, ui: &mut egui::Ui, act: &mut Option<&'static str>) {
    let t = app.tokens;
    ui.label(egui::RichText::new(tl!("Where will you post it?")).strong());
    let current = app.ui.creator.export.preset.clone();
    for (id, label, _, (w, h)) in filmcraft_engine::creator::EXPORT_PRESETS {
        let size = if w > 0 { format!("{w}×{h}") } else { tl!("same size as the video").to_string() };
        let r = ui.radio(current == id, format!("{}  —  {size}", crate::i18n::t(label)));
        app.auto.add_response(&format!("creator.export.preset.{id}"), &r, label);
        if r.clicked() {
            app.ui.creator.export.preset = id.to_string();
        }
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(tl!("File name"));
        let mut name = app.ui.creator.export.name.clone();
        let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(240.0));
        app.auto.add_response("creator.export.name", &r, tl!("File name"));
        ui.label(".mp4");
        // a typed extension is not doubled: it goes once the field is left
        if !r.has_focus() {
            name = filmcraft_engine::creator::strip_media_extension(name.trim()).to_string();
        }
        app.ui.creator.export.name = name;
    });
    ui.horizontal(|ui| {
        ui.label(tl!("Save to"));
        let folder = app.ui.creator.export.folder.clone();
        ui.add(egui::Label::new(egui::RichText::new(&folder).color(t.text_dim)).truncate()).on_hover_text(folder.as_str());
        let r = ui.button(tl!("Change…"));
        app.auto.add_response("creator.export.changeFolder", &r, tl!("Change folder"));
        if r.clicked() {
            *act = Some("creator.chooseExportFolder");
        }
    });
    let ex = &mut app.ui.creator.export;
    let mut burn = ex.burn_captions;
    let mut fast = ex.fast;
    let r1 = ui.checkbox(&mut burn, tl!("Show captions in the video"));
    let r2 = ui
        .checkbox(&mut fast, tl!("Fast export (hardware acceleration)"))
        .on_hover_text(tl!("Uses the Mac's video encoder and GPU when available; otherwise exports normally"));
    app.auto.add_response("creator.export.burnCaptions", &r1, tl!("Show captions in the video"));
    app.auto.add_response("creator.export.fast", &r2, tl!("Fast export"));
    app.ui.creator.export.burn_captions = burn;
    app.ui.creator.export.fast = fast;
    let path_preview = filmcraft_engine::creator::export_path(std::path::Path::new(&app.ui.creator.export.folder), &app.ui.creator.export.name, "mp4", false);
    ui.label(egui::RichText::new(tlf!("Saves as {path}", path = path_preview)).size(11.5).color(t.text_dim));
    ui.add_space(6.0);
    export_status(app, ui);
    ui.add_space(6.0);
    let running = app.ui.creator.export.job.and_then(|j| job_state(app, j)).is_some_and(|s| !s.1);
    ui.horizontal(|ui| {
        let b = egui::Button::new(egui::RichText::new(tl!("Export")).color(egui::Color32::WHITE).strong()).fill(t.accent).min_size(vec2(110.0, 30.0));
        let r = ui.add_enabled(!running && !app.ui.creator.export.name.trim().is_empty(), b);
        app.auto.add_response("creator.export.export", &r, tl!("Export"));
        if r.clicked() {
            *act = Some("creator.exportNow");
        }
        let r = ui.button(tl!("More settings…")).on_hover_text(tl!("The full Export page: formats, codecs, bitrates, ranges"));
        app.auto.add_response("creator.export.more", &r, tl!("More settings"));
        if r.clicked() {
            app.ui.creator.export.open = false;
            app.ui.mode = Mode::Export;
        }
        let r = ui.button(tl!("Close"));
        app.auto.add_response("creator.export.close", &r, tl!("Close"));
        if r.clicked() {
            app.ui.creator.export.open = false;
        }
    });
}

fn export_status(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let t = app.tokens;
    if !app.ui.creator.export.error.is_empty() {
        ui.colored_label(egui::Color32::from_rgb(230, 90, 80), app.ui.creator.export.error.clone());
    }
    let Some(job) = app.ui.creator.export.job else { return };
    let Some((f, done, error)) = job_state(app, job) else { return };
    match (done, error) {
        (false, _) => {
            ui.add(egui::ProgressBar::new(f).show_percentage().desired_width(400.0));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
        (true, Some(e)) => {
            ui.colored_label(egui::Color32::from_rgb(230, 90, 80), tlf!("Export failed: {error}", error = e));
        }
        (true, None) => {
            let path = app.ui.creator.export.path.clone();
            let r = ui.label(egui::RichText::new(tlf!("Done: {path}", path = path)).color(t.text));
            app.auto.add_response("creator.export.done", &r, tl!("Export finished"));
        }
    }
}
