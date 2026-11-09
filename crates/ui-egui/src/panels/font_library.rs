//! Window ▸ Font Library: the persistent font library (engine `font_library`). Add font files
//! (TrueType, OpenType, collections, WOFF, WOFF2) with their licence, see every weight drawn by
//! the text engine that renders titles and captions, edit licences, remove fonts, and see which
//! fonts of the active sequence are missing or substituted (exports refuse those).
//!
//! Every control has an automation id under `fontLibrary.*`, and every action is an engine
//! command (`fonts.library.*`, `fonts.missing`), so agents drive the same paths.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::native_dialogs::FileDialog;

/// Font file extensions offered by Add Fonts….
pub const FONT_EXTS: &[&str] = &["ttf", "otf", "ttc", "otc", "woff", "woff2"];
const PREVIEW_DEFAULT: &str = "The quick brown fox jumps over the lazy dog 0123456789";
const MAX_TEXTURES: usize = 400;

/// A font file chosen with Add Fonts…, waiting for its licence.
#[derive(Clone, Debug)]
pub struct PendingImport {
    pub path: String,
    /// `fonts.library.inspect` reply (or the error).
    pub inspection: Result<Value, String>,
    pub spdx: String,
    pub license_text: String,
    pub source: String,
    pub replace: bool,
}

/// The licence editor of one library font.
#[derive(Clone, Debug, Default)]
pub struct LicenseEdit {
    pub id: String,
    pub spdx: String,
    pub text: String,
    pub source: String,
}

#[derive(Default)]
pub struct State {
    pub open: bool,
    pub preview_text: String,
    pub preview_size: f32,
    pub pending: Vec<PendingImport>,
    pub license_edit: Option<LicenseEdit>,
    /// Font id awaiting Remove confirmation, with the error of a refused first attempt.
    pub confirm_remove: Option<(String, String)>,
    pub error: String,
    pub notice: String,
    /// Cached `fonts.library.list` and `fonts.missing` replies (refreshed after every change and
    /// when the project changes).
    list: Option<Value>,
    missing: Option<Value>,
    seen_revision: Option<u64>,
    textures: HashMap<String, egui::TextureHandle>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("font_library::State").field("open", &self.open).field("pending", &self.pending.len()).finish()
    }
}

impl Clone for State {
    fn clone(&self) -> Self {
        State {
            open: self.open,
            preview_text: self.preview_text.clone(),
            preview_size: self.preview_size,
            pending: self.pending.clone(),
            license_edit: self.license_edit.clone(),
            confirm_remove: self.confirm_remove.clone(),
            error: self.error.clone(),
            notice: self.notice.clone(),
            list: None,
            missing: None,
            seen_revision: None,
            textures: HashMap::new(),
        }
    }
}

pub fn open(app: &mut FilmcraftApp) {
    let st = &mut app.ui.font_library;
    st.open = true;
    if st.preview_text.is_empty() {
        st.preview_text = PREVIEW_DEFAULT.into();
    }
    if st.preview_size <= 0.0 {
        st.preview_size = 36.0;
    }
    st.list = None;
    st.missing = None;
}

fn refresh(app: &mut FilmcraftApp) {
    let list = app.session.execute("fonts.library.list", json!({}));
    let missing = if app.session.state.active_sequence.is_some() { app.session.execute("fonts.missing", json!({})).ok() } else { None };
    let st = &mut app.ui.font_library;
    match list {
        Ok(v) => st.list = Some(v),
        Err(e) => {
            st.list = Some(json!({"fonts": [], "problems": [e.to_string()]}));
        }
    }
    st.missing = missing;
    st.seen_revision = Some(app.session.revision);
}

/// Menu entry points.
pub fn route(app: &mut FilmcraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    match id {
        "window.fontLibrary" if params.get("close").and_then(Value::as_bool) == Some(true) => {
            app.ui.font_library.open = false;
            Some(Ok(json!({"open": false})))
        }
        "window.fontLibrary" => {
            open(app);
            Some(Ok(json!({"open": true})))
        }
        "fontLibrary.addFonts" if params.get("paths").is_none() => {
            open(app);
            Some(app.pick(FileDialog::open_files(FONT_EXTS), |app, paths| {
                queue_paths(app, paths);
                Ok(json!({"pending": app.ui.font_library.pending.len()}))
            }))
        }
        "fontLibrary.addFonts" => {
            let paths: Vec<String> = params["paths"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            open(app);
            queue_paths(app, paths);
            Some(Ok(json!({"pending": app.ui.font_library.pending.len()})))
        }
        _ => None,
    }
}

/// Inspect chosen files and list them for licence entry (nothing is added yet).
pub fn queue_paths(app: &mut FilmcraftApp, paths: Vec<String>) {
    for path in paths.into_iter().take(200) {
        if app.ui.font_library.pending.iter().any(|p| p.path == path) {
            continue;
        }
        let inspection = app.session.execute("fonts.library.inspect", json!({"path": path})).map_err(|e| e.to_string());
        let spdx = inspection.as_ref().ok().and_then(|v| v["suggested_spdx"].as_str()).unwrap_or("").to_string();
        let license_text =
            inspection.as_ref().ok().and_then(|v| v["license_url"].as_str().or_else(|| v["license_description"].as_str())).unwrap_or("").to_string();
        app.ui.font_library.pending.push(PendingImport { path, inspection, spdx, license_text, source: String::new(), replace: false });
    }
}

fn texture(ctx: &egui::Context, st: &mut State, family: &str, style: &str, text: &str, size: f32) -> Option<egui::TextureHandle> {
    let key = format!("{family}\u{1}{style}\u{1}{text}\u{1}{size}");
    if let Some(t) = st.textures.get(&key) {
        return Some(t.clone());
    }
    if st.textures.len() > MAX_TEXTURES {
        st.textures.clear();
    }
    let (w, h, rgba) = filmcraft_engine::font_library::preview_rgba(family, style, text, size)?;
    let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
    let t = ctx.load_texture(format!("font-preview-{key}"), img, egui::TextureOptions::LINEAR);
    st.textures.insert(key, t.clone());
    Some(t)
}

fn status_color(ui: &egui::Ui, status: &str) -> egui::Color32 {
    match status {
        "missing" => ui.visuals().error_fg_color,
        "substituted" => ui.visuals().warn_fg_color,
        _ => ui.visuals().weak_text_color(),
    }
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.ui.font_library.open {
        return;
    }
    if app.ui.font_library.list.is_none() || app.ui.font_library.seen_revision != Some(app.session.revision) {
        refresh(app);
    }
    let mut open = true;
    let mut changed = false;
    let win = egui::Window::new(tl!("Font Library")).id(egui::Id::unique("font-library")).open(&mut open).default_size([720.0, 640.0]).resizable(true).show(
        ctx,
        |ui| {
            changed = body(app, ui);
        },
    );
    if let Some(w) = &win {
        app.auto.add_response("fontLibrary.window", &w.response, tl!("Font Library"));
    }
    if changed {
        app.ui.font_library.textures.clear();
        refresh(app);
    }
    if !open {
        app.ui.font_library.open = false;
    }
}

/// The window's contents; true when the library changed.
fn body(app: &mut FilmcraftApp, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    let list = app.ui.font_library.list.clone().unwrap_or(Value::Null);
    ui.horizontal(|ui| {
        let r = ui.button(tl!("Add Fonts…"));
        app.auto.add_response("fontLibrary.add", &r, tl!("Add font files to the library"));
        if r.clicked() {
            app.pick_ui(FileDialog::open_files(FONT_EXTS), queue_paths);
        }
        ui.label(tl!("TrueType, OpenType, WOFF and WOFF2. Fonts stay installed after a restart."));
    });
    if let Some(dir) = list["dir"].as_str() {
        ui.small(tlf!("Stored in {dir}", dir = dir));
    }
    // ---- files waiting for a licence
    if !app.ui.font_library.pending.is_empty() {
        ui.separator();
        ui.strong(tl!("Add to library"));
        changed |= pending_ui(app, ui);
    }
    ui.separator();
    // ---- preview controls
    ui.horizontal(|ui| {
        ui.label(tl!("Preview text"));
        let st = &mut app.ui.font_library;
        let r = ui.add(egui::TextEdit::singleline(&mut st.preview_text).desired_width(320.0));
        app.auto.add_response("fontLibrary.previewText", &r, tl!("Preview text"));
        let r = ui.add(egui::Slider::new(&mut app.ui.font_library.preview_size, 12.0..=96.0).text(tl!("px")));
        app.auto.add_response("fontLibrary.previewSize", &r, tl!("Preview size"));
    });
    let available = ui.available_height();
    egui::ScrollArea::vertical().id_salt("font-library-scroll").max_height(available - 8.0).show(ui, |ui| {
        changed |= fonts_ui(app, ui, &list);
        sequence_ui(app, ui);
        let problems: Vec<String> = list["problems"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        if !problems.is_empty() {
            ui.separator();
            ui.colored_label(ui.visuals().error_fg_color, tl!("Library problems"));
            for p in problems.iter().take(50) {
                ui.colored_label(ui.visuals().error_fg_color, p);
            }
        }
    });
    let st = &app.ui.font_library;
    if !st.error.is_empty() {
        ui.colored_label(ui.visuals().error_fg_color, st.error.clone());
    }
    if !st.notice.is_empty() {
        ui.label(st.notice.clone());
    }
    changed
}

fn pending_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    let mut pending = std::mem::take(&mut app.ui.font_library.pending);
    let mut keep = Vec::new();
    for (i, p) in pending.drain(..).enumerate() {
        let mut p = p;
        let mut drop = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let name = std::path::Path::new(&p.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.path.clone());
            ui.strong(name);
            match &p.inspection {
                Err(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                    let r = ui.button(tl!("Dismiss"));
                    app.auto.add_response(&format!("fontLibrary.pending.{i}.dismiss"), &r, tl!("Dismiss"));
                    drop = r.clicked();
                    return;
                }
                Ok(ins) => {
                    let faces: Vec<String> = ins["faces"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|f| format!("{} {} ({})", f["family"].as_str().unwrap_or(""), f["style"].as_str().unwrap_or(""), f["weight"]))
                                .collect()
                        })
                        .unwrap_or_default();
                    let axes = ins["axes"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                    ui.label(tlf!("{format} · {n} style(s)", format = ins["format"].as_str().unwrap_or("").to_uppercase(), n = faces.len()));
                    if !axes.is_empty() {
                        ui.label(tlf!("Variable font, axes: {axes}", axes = axes));
                    }
                    ui.small(faces.join(" · "));
                    if let Some(d) = ins["license_description"].as_str() {
                        ui.small(tlf!("The font states: {d}", d = d.chars().take(300).collect::<String>()));
                    }
                }
            }
            egui::Grid::new(format!("font-lic-{i}")).num_columns(2).show(ui, |ui| {
                ui.label(tl!("Licence (SPDX)"));
                let r = ui.add(egui::TextEdit::singleline(&mut p.spdx).hint_text("OFL-1.1, Apache-2.0, LicenseRef-…"));
                app.auto.add_response(&format!("fontLibrary.pending.{i}.license"), &r, tl!("Licence"));
                ui.end_row();
                ui.label(tl!("Licence text or URL"));
                let r = ui.text_edit_singleline(&mut p.license_text);
                app.auto.add_response(&format!("fontLibrary.pending.{i}.licenseText"), &r, tl!("Licence text or URL"));
                ui.end_row();
                ui.label(tl!("Source"));
                let r = ui.add(egui::TextEdit::singleline(&mut p.source).hint_text(tl!("Foundry, URL or client folder")));
                app.auto.add_response(&format!("fontLibrary.pending.{i}.source"), &r, tl!("Source"));
                ui.end_row();
            });
            let r = ui.checkbox(&mut p.replace, tl!("Replace a library font with the same family and style"));
            app.auto.add_response(&format!("fontLibrary.pending.{i}.replace"), &r, tl!("Replace"));
            ui.horizontal(|ui| {
                let r = ui.add_enabled(!p.spdx.trim().is_empty(), egui::Button::new(tl!("Add to Library")));
                app.auto.add_response(&format!("fontLibrary.pending.{i}.add"), &r, tl!("Add to Library"));
                if p.spdx.trim().is_empty() {
                    ui.small(tl!("A licence is required"));
                }
                if r.clicked() {
                    let res = app.session.execute(
                        "fonts.library.import",
                        json!({"path": p.path, "license": {"spdx": p.spdx, "text": p.license_text, "source": p.source}, "replace": p.replace}),
                    );
                    match res {
                        Ok(v) => {
                            let n = v["imported"][0]["registeredFaces"].as_u64().unwrap_or(0);
                            app.ui.font_library.notice = tlf!("Added {n} style(s)", n = n);
                            app.ui.font_library.error.clear();
                            drop = true;
                            changed = true;
                        }
                        Err(e) => app.ui.font_library.error = e.to_string(),
                    }
                }
                let r = ui.button(tl!("Cancel"));
                app.auto.add_response(&format!("fontLibrary.pending.{i}.cancel"), &r, tl!("Cancel"));
                drop |= r.clicked();
            });
        });
        if !drop {
            keep.push(p);
        }
    }
    // files queued while this frame ran (a dialog answer) come after the ones shown
    keep.append(&mut app.ui.font_library.pending);
    app.ui.font_library.pending = keep;
    changed
}

fn fonts_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, list: &Value) -> bool {
    let mut changed = false;
    let fonts = list["fonts"].as_array().cloned().unwrap_or_default();
    if fonts.is_empty() {
        ui.label(tl!("The library is empty. Add fonts to use them in titles and captions."));
    }
    let text = app.ui.font_library.preview_text.clone();
    let size = app.ui.font_library.preview_size;
    let ink = ui.visuals().text_color();
    for f in fonts.iter().take(500) {
        let id = f["id"].as_str().unwrap_or("").to_string();
        let fams = f["families"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        let header = tlf!("{fams} · {format}", fams = fams, format = f["format"].as_str().unwrap_or("").to_uppercase());
        let resp = egui::CollapsingHeader::new(header).id_salt(format!("font-{id}")).default_open(true).show(ui, |ui| {
            let lic = &f["license"];
            ui.horizontal_wrapped(|ui| {
                ui.label(tlf!("Licence: {spdx}", spdx = lic["spdx"].as_str().unwrap_or("")));
                if let Some(t) = lic["text"].as_str() {
                    ui.small(t);
                }
                if let Some(src) = lic["source"].as_str() {
                    ui.small(tlf!("Source: {src}", src = src));
                }
                ui.small(tlf!("Added {when}", when = f["added"].as_str().unwrap_or("")));
                if f["variable"].as_bool() == Some(true) {
                    let axes = f["axes"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                    ui.small(tlf!("Variable ({axes})", axes = axes));
                }
            });
            ui.horizontal(|ui| {
                let r = ui.button(tl!("Edit Licence…"));
                app.auto.add_response(&format!("fontLibrary.font.{id}.editLicense"), &r, tl!("Edit licence"));
                if r.clicked() {
                    app.ui.font_library.license_edit = Some(LicenseEdit {
                        id: id.clone(),
                        spdx: lic["spdx"].as_str().unwrap_or("").into(),
                        text: lic["text"].as_str().unwrap_or("").into(),
                        source: lic["source"].as_str().unwrap_or("").into(),
                    });
                }
                let r = ui.button(tl!("Remove…"));
                app.auto.add_response(&format!("fontLibrary.font.{id}.remove"), &r, tl!("Remove from library"));
                if r.clicked() {
                    app.ui.font_library.confirm_remove = Some((id.clone(), String::new()));
                }
            });
            // the licence editor and the remove confirmation sit above the styles, in view
            changed |= license_edit_ui(app, ui, &id);
            changed |= confirm_remove_ui(app, ui, &id, &fams);
            for face in f["faces"].as_array().cloned().unwrap_or_default().iter().take(64) {
                let (fam, style) = (face["family"].as_str().unwrap_or(""), face["style"].as_str().unwrap_or(""));
                ui.horizontal(|ui| {
                    let state = face["state"].as_str().unwrap_or("");
                    let label = format!("{style} · {}", face["weight"]);
                    let r = ui.add_sized([150.0, 18.0], egui::Label::new(label).truncate());
                    app.auto.add_response(&format!("fontLibrary.font.{id}.style.{style}"), &r, &format!("{fam} {style}"));
                    if state != "active" {
                        ui.colored_label(ui.visuals().warn_fg_color, state);
                    }
                    let st = &mut app.ui.font_library;
                    if let Some(t) = texture(ui.ctx(), st, fam, style, &text, size) {
                        // scaled down to the room left in the row, never stretching the window
                        let sz = t.size_vec2();
                        let room = ui.available_width().max(40.0);
                        let sz = if sz.x > room { sz * (room / sz.x) } else { sz };
                        let r = ui.add(egui::Image::new((t.id(), sz)).tint(ink));
                        app.auto.add_response(&format!("fontLibrary.font.{id}.preview.{style}"), &r, &format!("{fam} {style} preview"));
                    }
                });
            }
        });
        app.auto.add_response(&format!("fontLibrary.font.{id}"), &resp.header_response, &fams);
    }
    changed
}

fn license_edit_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, id: &str) -> bool {
    let Some(mut ed) = app.ui.font_library.license_edit.take() else { return false };
    if ed.id != id {
        app.ui.font_library.license_edit = Some(ed);
        return false;
    }
    let mut keep = true;
    let mut changed = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::Grid::new(format!("lic-edit-{id}")).num_columns(2).show(ui, |ui| {
            ui.label(tl!("Licence (SPDX)"));
            let r = ui.text_edit_singleline(&mut ed.spdx);
            app.auto.add_response("fontLibrary.licenseEdit.spdx", &r, tl!("Licence"));
            ui.end_row();
            ui.label(tl!("Licence text or URL"));
            let r = ui.text_edit_singleline(&mut ed.text);
            app.auto.add_response("fontLibrary.licenseEdit.text", &r, tl!("Licence text or URL"));
            ui.end_row();
            ui.label(tl!("Source"));
            let r = ui.text_edit_singleline(&mut ed.source);
            app.auto.add_response("fontLibrary.licenseEdit.source", &r, tl!("Source"));
            ui.end_row();
        });
        ui.horizontal(|ui| {
            let r = ui.button(tl!("Save"));
            app.auto.add_response("fontLibrary.licenseEdit.save", &r, tl!("Save licence"));
            if r.clicked() {
                match app.session.execute("fonts.library.setLicense", json!({"id": ed.id, "license": {"spdx": ed.spdx, "text": ed.text, "source": ed.source}}))
                {
                    Ok(_) => {
                        keep = false;
                        changed = true;
                        app.ui.font_library.error.clear();
                    }
                    Err(e) => app.ui.font_library.error = e.to_string(),
                }
            }
            let r = ui.button(tl!("Cancel"));
            app.auto.add_response("fontLibrary.licenseEdit.cancel", &r, tl!("Cancel"));
            keep &= !r.clicked();
        });
    });
    if keep {
        app.ui.font_library.license_edit = Some(ed);
    }
    changed
}

fn confirm_remove_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, id: &str, fams: &str) -> bool {
    let Some((cid, err)) = app.ui.font_library.confirm_remove.clone() else { return false };
    if cid != id {
        return false;
    }
    let mut changed = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(tlf!("Remove {fams} from the library? Text set in it will be reported missing and exports will stop until it is replaced.", fams = fams));
        if !err.is_empty() {
            ui.colored_label(ui.visuals().warn_fg_color, &err);
        }
        ui.horizontal(|ui| {
            let force = !err.is_empty();
            let r = ui.button(if force { tl!("Remove Anyway") } else { tl!("Remove") });
            app.auto.add_response("fontLibrary.confirmRemove", &r, tl!("Remove"));
            if r.clicked() {
                match app.session.execute("fonts.library.remove", json!({"id": id, "force": force})) {
                    Ok(_) => {
                        app.ui.font_library.confirm_remove = None;
                        app.ui.font_library.notice = tlf!("Removed {fams}", fams = fams);
                        changed = true;
                    }
                    Err(e) => app.ui.font_library.confirm_remove = Some((id.to_string(), e.to_string())),
                }
            }
            let r = ui.button(tl!("Cancel"));
            app.auto.add_response("fontLibrary.cancelRemove", &r, tl!("Cancel"));
            if r.clicked() {
                app.ui.font_library.confirm_remove = None;
            }
        });
    });
    changed
}

fn sequence_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    ui.separator();
    let Some(m) = app.ui.font_library.missing.clone() else {
        ui.label(tl!("Open a sequence to check its fonts."));
        return;
    };
    let fonts = m["sequences"][0]["fonts"].as_array().cloned().unwrap_or_default();
    let ok = m["ok"].as_bool().unwrap_or(false);
    let heading = if ok { tl!("Fonts in this sequence: all render as specified") } else { tl!("Fonts in this sequence: some will not render as specified") };
    let r = ui.colored_label(if ok { ui.visuals().text_color() } else { ui.visuals().error_fg_color }, heading);
    app.auto.add_response("fontLibrary.sequenceStatus", &r, heading);
    if !ok {
        ui.small(tl!("Exports are refused until these fonts are added to the library or the text is changed."));
    }
    egui::Grid::new("font-library-seq").num_columns(4).striped(true).show(ui, |ui| {
        for (i, f) in fonts.iter().take(200).enumerate() {
            let status = f["status"].as_str().unwrap_or("");
            ui.label(format!("{} {}", f["family"].as_str().unwrap_or(""), f["style"].as_str().unwrap_or("")));
            let r = ui.colored_label(status_color(ui, status), status);
            app.auto.add_response(&format!("fontLibrary.sequence.{i}"), &r, status);
            ui.label(f["rendered_as"].as_str().map(|r| tlf!("drawn as {r}", r = r)).unwrap_or_default());
            let uses = f["uses"].as_array().map(|a| a.iter().filter_map(Value::as_str).take(3).collect::<Vec<_>>().join("; ")).unwrap_or_default();
            ui.small(uses);
            ui.end_row();
        }
    });
}
