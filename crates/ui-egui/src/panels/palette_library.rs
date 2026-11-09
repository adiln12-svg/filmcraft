//! Window ▸ Palette Library: the persistent Colour Palette Library (engine `palette_library`).
//!
//! Browse and search palettes; create, rename, duplicate, archive and delete them; edit colours
//! (visual picker, hex and RGB entry, name, role, shade) and usage rules in a draft that is
//! saved as a new version (earlier versions never change and can be viewed); preview every
//! colour and every stated combination; add a palette's colours to the project's swatches.
//!
//! Every action is a `palettes.*` engine command, so agents use the same paths; every control
//! has an automation id under `paletteLibrary.*`.

use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::widgets::color_picker::{self, ColorDraft};
use filmcraft_engine::palette_library::{STANDARD_ROLES, fresh_color_id, fresh_rule_id};
use filmcraft_project::HexColor;

/// A colour being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorRow {
    pub id: String,
    pub name: String,
    pub hex: HexColor,
    pub role: String,
    pub shade_of: String,
    pub shade: String,
}

/// A usage rule being edited (colours by id).
#[derive(Clone, Debug, PartialEq)]
pub struct RuleRow {
    pub id: String,
    pub kind: String,
    pub foreground: String,
    pub background: String,
    pub colors: Vec<String>,
    pub applies_to: String,
    pub text: String,
}

/// Unsaved changes to the latest version of a palette.
#[derive(Clone, Debug)]
pub struct Draft {
    pub palette: String,
    pub base_version: u64,
    pub colors: Vec<ColorRow>,
    pub rules: Vec<RuleRow>,
    pub original: (Vec<ColorRow>, Vec<RuleRow>),
    pub note: String,
    /// Index of the colour open in the editor.
    pub editing: Option<usize>,
    pub picker: ColorDraft,
    pub new_rule: RuleRow,
}

impl Draft {
    pub fn dirty(&self) -> bool {
        (&self.colors, &self.rules) != (&self.original.0, &self.original.1)
    }
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub open: bool,
    pub query: String,
    pub show_archived: bool,
    pub selected: Option<String>,
    /// A saved version being viewed (None: the latest, editable).
    pub view_version: Option<u64>,
    pub draft: Option<Draft>,
    pub new_name: Option<(String, String)>,
    pub rename: Option<String>,
    pub confirm_delete: bool,
    pub error: String,
    pub notice: String,
    list: Option<Value>,
    current: Option<Value>,
}

pub fn open(app: &mut FilmcraftApp) {
    let st = &mut app.ui.palette_library;
    st.open = true;
    st.list = None;
    st.current = None;
}

/// Menu entry points.
pub fn route(app: &mut FilmcraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    match id {
        "window.paletteLibrary" => {
            open(app);
            if let Some(p) = params.get("select").and_then(Value::as_str) {
                select(app, p.to_string());
            }
            Some(Ok(json!({"open": true})))
        }
        _ => None,
    }
}

fn hexs(v: &Value) -> HexColor {
    v.as_str().and_then(HexColor::parse).unwrap_or(HexColor([128, 128, 128]))
}

fn rows(version: &Value) -> (Vec<ColorRow>, Vec<RuleRow>) {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let colors = version["colors"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| ColorRow {
                    id: s(&c["id"]),
                    name: s(&c["name"]),
                    hex: hexs(&c["hex"]),
                    role: s(&c["role"]),
                    shade_of: s(&c["shadeOf"]),
                    shade: s(&c["shade"]),
                })
                .collect()
        })
        .unwrap_or_default();
    let rules = version["rules"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| RuleRow {
                    id: s(&r["id"]),
                    kind: s(&r["kind"]),
                    foreground: s(&r["foreground"]),
                    background: s(&r["background"]),
                    colors: r["colors"].as_array().map(|x| x.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
                    applies_to: s(&r["appliesTo"]),
                    text: s(&r["text"]),
                })
                .collect()
        })
        .unwrap_or_default();
    (colors, rules)
}

fn empty_rule() -> RuleRow {
    RuleRow {
        id: String::new(),
        kind: "prohibited".into(),
        foreground: String::new(),
        background: String::new(),
        colors: Vec::new(),
        applies_to: "text".into(),
        text: String::new(),
    }
}

fn select(app: &mut FilmcraftApp, id: String) {
    let st = &mut app.ui.palette_library;
    st.selected = Some(id);
    st.view_version = None;
    st.draft = None;
    st.rename = None;
    st.confirm_delete = false;
    st.current = None;
    st.error.clear();
}

fn refresh(app: &mut FilmcraftApp) {
    let (q, archived) = (app.ui.palette_library.query.clone(), app.ui.palette_library.show_archived);
    let list = app.session.execute("palettes.list", json!({"query": q, "includeArchived": archived})).map_err(|e| e.to_string());
    let st = &mut app.ui.palette_library;
    match list {
        Ok(v) => st.list = Some(v),
        Err(e) => {
            st.error = e;
            st.list = Some(json!({"palettes": []}));
        }
    }
    let Some(id) = st.selected.clone() else { return };
    let mut p = json!({"id": id});
    if let Some(v) = st.view_version {
        p["version"] = json!(v);
    }
    match app.session.execute("palettes.get", p) {
        Ok(v) => {
            let st = &mut app.ui.palette_library;
            // the draft follows the latest version when there are no unsaved changes
            if st.view_version.is_none() && st.draft.as_ref().is_none_or(|d| !d.dirty() && d.base_version != v["latestVersion"].as_u64().unwrap_or(0)) {
                let (colors, rules) = rows(&v["version"]);
                st.draft = Some(Draft {
                    palette: id,
                    base_version: v["latestVersion"].as_u64().unwrap_or(0),
                    original: (colors.clone(), rules.clone()),
                    colors,
                    rules,
                    note: String::new(),
                    editing: None,
                    picker: ColorDraft::new([128, 128, 128]),
                    new_rule: empty_rule(),
                });
            }
            st.current = Some(v);
        }
        Err(e) => {
            let st = &mut app.ui.palette_library;
            st.error = e.to_string();
            st.selected = None;
            st.current = None;
            st.draft = None;
        }
    }
}

fn run(app: &mut FilmcraftApp, cmd: &str, p: Value) -> Option<Value> {
    match app.session.execute(cmd, p) {
        Ok(v) => {
            app.ui.palette_library.error.clear();
            Some(v)
        }
        Err(e) => {
            app.ui.palette_library.error = e.to_string();
            None
        }
    }
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.ui.palette_library.open {
        return;
    }
    if app.ui.palette_library.list.is_none() || (app.ui.palette_library.selected.is_some() && app.ui.palette_library.current.is_none()) {
        refresh(app);
    }
    let mut open = true;
    let mut changed = false;
    let win =
        egui::Window::new(tl!("Palette Library")).id(egui::Id::unique("palette-library")).open(&mut open).default_size([980.0, 680.0]).resizable(true).show(
            ctx,
            |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(250.0);
                        changed |= list_ui(app, ui);
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        changed |= detail_ui(app, ui);
                    });
                });
                let st = &app.ui.palette_library;
                if !st.error.is_empty() {
                    ui.colored_label(ui.visuals().error_fg_color, st.error.clone());
                }
                if !st.notice.is_empty() {
                    ui.label(st.notice.clone());
                }
            },
        );
    if let Some(w) = &win {
        app.auto.add_response("paletteLibrary.window", &w.response, tl!("Palette Library"));
    }
    if changed {
        app.ui.palette_library.list = None;
        app.ui.palette_library.current = None;
    }
    if !open {
        app.ui.palette_library.open = false;
    }
}

fn swatch(ui: &mut egui::Ui, hex: HexColor, size: egui::Vec2) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click());
    let [cr, cg, cb] = hex.0;
    ui.painter().rect_filled(rect, 3.0, egui::Color32::from_rgb(cr, cg, cb));
    ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(1.0, ui.visuals().weak_text_color()), egui::StrokeKind::Inside);
    r
}

fn list_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let st = &mut app.ui.palette_library;
        let r = ui.add(egui::TextEdit::singleline(&mut st.query).hint_text(tl!("Search name, colour, #hex, role")).desired_width(200.0));
        app.auto.add_response("paletteLibrary.search", &r, tl!("Search palettes"));
        changed |= r.changed();
    });
    let r = ui.checkbox(&mut app.ui.palette_library.show_archived, tl!("Show archived"));
    app.auto.add_response("paletteLibrary.showArchived", &r, tl!("Show archived"));
    changed |= r.changed();
    let r = ui.button(tl!("New Palette…"));
    app.auto.add_response("paletteLibrary.new", &r, tl!("New palette"));
    if r.clicked() {
        app.ui.palette_library.new_name = Some((String::new(), String::new()));
    }
    if let Some((mut name, mut desc)) = app.ui.palette_library.new_name.take() {
        let mut keep = true;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut name).hint_text(tl!("Palette name")));
            app.auto.add_response("paletteLibrary.newName", &r, tl!("Palette name"));
            let r = ui.add(egui::TextEdit::singleline(&mut desc).hint_text(tl!("Description (optional)")));
            app.auto.add_response("paletteLibrary.newDescription", &r, tl!("Description"));
            ui.horizontal(|ui| {
                let r = ui.add_enabled(!name.trim().is_empty(), egui::Button::new(tl!("Create")));
                app.auto.add_response("paletteLibrary.create", &r, tl!("Create palette"));
                if r.clicked()
                    && let Some(v) = run(app, "palettes.create", json!({"name": name, "description": desc, "by": "user"}))
                {
                    let id = v["id"].as_str().unwrap_or("").to_string();
                    select(app, id);
                    keep = false;
                    changed = true;
                }
                let r = ui.button(tl!("Cancel"));
                app.auto.add_response("paletteLibrary.cancelNew", &r, tl!("Cancel"));
                keep &= !r.clicked();
            });
        });
        if keep {
            app.ui.palette_library.new_name = Some((name, desc));
        }
    }
    ui.separator();
    let list = app.ui.palette_library.list.clone().unwrap_or(Value::Null);
    let pals = list["palettes"].as_array().cloned().unwrap_or_default();
    if pals.is_empty() {
        ui.label(tl!("No palettes yet."));
    }
    egui::ScrollArea::vertical().id_salt("palette-list").max_height(ui.available_height() - 40.0).show(ui, |ui| {
        for p in pals.iter().take(1000) {
            let id = p["id"].as_str().unwrap_or("").to_string();
            let selected = app.ui.palette_library.selected.as_deref() == Some(id.as_str());
            let mut label = p["name"].as_str().unwrap_or("").to_string();
            if p["archived"].as_bool() == Some(true) {
                label.push_str(tl!(" (archived)"));
            }
            let r = ui.selectable_label(selected, format!("{label}  ·  v{}", p["latestVersion"]));
            app.auto.add_response(&format!("paletteLibrary.palette.{id}"), &r, &label);
            if r.clicked() && !selected {
                if app.ui.palette_library.draft.as_ref().is_some_and(Draft::dirty) {
                    app.ui.palette_library.error = tl!("Save or discard the changes to this palette first.").into();
                } else {
                    select(app, id.clone());
                    changed = true;
                }
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for h in p["swatches"].as_array().cloned().unwrap_or_default().iter().take(24) {
                    swatch(ui, hexs(h), egui::vec2(12.0, 12.0));
                }
            });
            ui.add_space(4.0);
        }
    });
    changed
}

fn detail_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui) -> bool {
    let Some(cur) = app.ui.palette_library.current.clone() else {
        ui.label(tl!("Select a palette, or create one."));
        return false;
    };
    let mut changed = false;
    let id = cur["id"].as_str().unwrap_or("").to_string();
    let latest = cur["latestVersion"].as_u64().unwrap_or(0);
    let archived = cur["archived"].as_bool().unwrap_or(false);
    // ---- name, id and palette actions
    ui.horizontal(|ui| {
        if let Some(mut name) = app.ui.palette_library.rename.take() {
            let r = ui.text_edit_singleline(&mut name);
            app.auto.add_response("paletteLibrary.renameField", &r, tl!("Palette name"));
            let ok = ui.button(tl!("Rename"));
            app.auto.add_response("paletteLibrary.renameApply", &ok, tl!("Rename"));
            let cancel = ui.button(tl!("Cancel"));
            if ok.clicked() {
                changed |= run(app, "palettes.setInfo", json!({"id": id, "name": name, "by": "user"})).is_some();
            } else if !cancel.clicked() {
                app.ui.palette_library.rename = Some(name);
            }
        } else {
            ui.heading(cur["name"].as_str().unwrap_or(""));
            let r = ui.small_button(tl!("Rename…"));
            app.auto.add_response("paletteLibrary.rename", &r, tl!("Rename palette"));
            if r.clicked() {
                app.ui.palette_library.rename = Some(cur["name"].as_str().unwrap_or("").to_string());
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        let r = ui.label(egui::RichText::new(&id).monospace().small());
        app.auto.add_response("paletteLibrary.id", &r, &id);
        if let Some(d) = cur["description"].as_str().filter(|d| !d.is_empty()) {
            ui.small(d);
        }
        if archived {
            ui.colored_label(ui.visuals().warn_fg_color, tl!("Archived"));
        }
    });
    ui.horizontal(|ui| {
        let r = ui.button(tl!("Duplicate"));
        app.auto.add_response("paletteLibrary.duplicate", &r, tl!("Duplicate palette"));
        if r.clicked() {
            let mut p = json!({"id": id, "by": "user"});
            if let Some(v) = app.ui.palette_library.view_version {
                p["version"] = json!(v);
            }
            if let Some(v) = run(app, "palettes.duplicate", p) {
                select(app, v["id"].as_str().unwrap_or("").to_string());
                changed = true;
            }
        }
        let r = ui.button(if archived { tl!("Restore") } else { tl!("Archive") });
        app.auto.add_response("paletteLibrary.archive", &r, tl!("Archive or restore palette"));
        if r.clicked() {
            changed |= run(app, "palettes.archive", json!({"id": id, "archived": !archived, "by": "user"})).is_some();
        }
        let r = ui.button(tl!("Add to Project Colors"));
        app.auto.add_response("paletteLibrary.toProject", &r, tl!("Add colours to the project's swatches"));
        if r.clicked() {
            let mut p = json!({"id": id});
            if let Some(v) = app.ui.palette_library.view_version {
                p["version"] = json!(v);
            }
            if let Some(v) = run(app, "palettes.addToProjectColors", p) {
                app.ui.palette_library.notice = tlf!("Added {n} colour(s) to the project", n = v["added"].as_array().map_or(0, Vec::len));
            }
        }
        let r = ui.button(tl!("Delete…"));
        app.auto.add_response("paletteLibrary.delete", &r, tl!("Delete palette"));
        if r.clicked() {
            app.ui.palette_library.confirm_delete = true;
        }
    });
    if app.ui.palette_library.confirm_delete {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(tlf!("Delete this palette and all {n} version(s)? Archive keeps it out of the list instead.", n = latest));
            ui.horizontal(|ui| {
                let r = ui.button(tl!("Delete"));
                app.auto.add_response("paletteLibrary.confirmDelete", &r, tl!("Delete"));
                if r.clicked() && run(app, "palettes.delete", json!({"id": id, "confirm": true})).is_some() {
                    let st = &mut app.ui.palette_library;
                    st.selected = None;
                    st.draft = None;
                    st.confirm_delete = false;
                    changed = true;
                }
                let r = ui.button(tl!("Cancel"));
                app.auto.add_response("paletteLibrary.cancelDelete", &r, tl!("Cancel"));
                if r.clicked() {
                    app.ui.palette_library.confirm_delete = false;
                }
            });
        });
    }
    // ---- versions
    ui.horizontal(|ui| {
        ui.label(tl!("Version"));
        let shown = app.ui.palette_library.view_version.unwrap_or(latest);
        let versions = cur["versions"].as_array().cloned().unwrap_or_default();
        let label = |v: &Value| {
            let note = v["note"].as_str().filter(|n| !n.is_empty()).map(|n| format!(" — {n}")).unwrap_or_default();
            format!("v{} · {} · {}{note}", v["version"], v["created"].as_str().unwrap_or(""), v["by"].as_str().unwrap_or(""))
        };
        let text = versions.iter().find(|v| v["version"].as_u64() == Some(shown)).map(label).unwrap_or_default();
        let dirty = app.ui.palette_library.draft.as_ref().is_some_and(Draft::dirty);
        let r = ui.add_enabled_ui(!dirty, |ui| {
            craft_ui::ComboBox::from_id_salt("palette-version").selected_text(text).width(420.0).show_ui(ui, |ui| {
                for v in versions.iter().rev() {
                    let n = v["version"].as_u64().unwrap_or(0);
                    let r = ui.selectable_label(shown == n, label(v));
                    app.auto.add_response(&format!("paletteLibrary.version.{n}"), &r, &label(v));
                    if r.clicked() {
                        app.ui.palette_library.view_version = if n == latest { None } else { Some(n) };
                        app.ui.palette_library.current = None;
                    }
                }
            })
        });
        app.auto.add_response("paletteLibrary.versionPicker", &r.response, tl!("Version"));
    });
    let viewing_old = app.ui.palette_library.view_version.is_some();
    if viewing_old {
        ui.colored_label(ui.visuals().warn_fg_color, tl!("A saved version: read only. It never changes; edits are saved as a new version of the latest."));
        let r = ui.button(tl!("Back to Latest"));
        app.auto.add_response("paletteLibrary.latest", &r, tl!("Back to latest version"));
        if r.clicked() {
            app.ui.palette_library.view_version = None;
            app.ui.palette_library.current = None;
        }
    }
    ui.separator();
    let editable = !viewing_old && !archived;
    let (colors, rules) =
        if editable { app.ui.palette_library.draft.as_ref().map(|d| (d.colors.clone(), d.rules.clone())).unwrap_or_default() } else { rows(&cur["version"]) };
    egui::ScrollArea::vertical().id_salt("palette-detail").max_height(ui.available_height() - 70.0).show(ui, |ui| {
        colors_ui(app, ui, &colors, editable);
        ui.separator();
        rules_ui(app, ui, &colors, &rules, editable);
    });
    if editable {
        changed |= save_bar(app, ui);
    }
    changed
}

fn color_name(colors: &[ColorRow], id: &str) -> String {
    colors.iter().find(|c| c.id == id).map(|c| c.name.clone()).unwrap_or_default()
}

fn colors_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, colors: &[ColorRow], editable: bool) {
    ui.strong(tlf!("Colours ({n})", n = colors.len()));
    // preview grid: every colour with its name, value and role
    ui.horizontal_wrapped(|ui| {
        for (i, c) in colors.iter().enumerate() {
            ui.vertical(|ui| {
                ui.set_width(118.0);
                let r = swatch(ui, c.hex, egui::vec2(110.0, 56.0));
                app.auto.add_response(&format!("paletteLibrary.color.{i}"), &r, &c.name);
                if r.clicked() && editable {
                    start_edit(app, i);
                }
                ui.label(egui::RichText::new(&c.name).strong());
                let [r_, g, b] = c.hex.0;
                ui.small(format!("{}  {r_},{g},{b}", c.hex.hex()));
                let mut tags = c.role.clone();
                if !c.shade_of.is_empty() {
                    let base = color_name(colors, &c.shade_of);
                    tags = format!("{tags}{}{}", if tags.is_empty() { "" } else { " · " }, tlf!("shade {s} of {base}", s = c.shade, base = base));
                }
                if !tags.is_empty() {
                    ui.small(tags);
                }
            });
        }
    });
    if !editable {
        return;
    }
    ui.horizontal(|ui| {
        let r = ui.button(tl!("Add Colour"));
        app.auto.add_response("paletteLibrary.addColor", &r, tl!("Add colour"));
        if r.clicked()
            && let Some(d) = app.ui.palette_library.draft.as_mut()
        {
            let mut n = d.colors.len() + 1;
            let mut name = tlf!("Colour {n}", n = n);
            while d.colors.iter().any(|c| c.name.eq_ignore_ascii_case(&name)) {
                n += 1;
                name = tlf!("Colour {n}", n = n);
            }
            d.colors.push(ColorRow {
                id: fresh_color_id(),
                name,
                hex: HexColor([128, 128, 128]),
                role: String::new(),
                shade_of: String::new(),
                shade: String::new(),
            });
            let i = d.colors.len() - 1;
            start_edit(app, i);
        }
        ui.small(tl!("Click a swatch to edit it."));
    });
    editor_ui(app, ui);
}

fn start_edit(app: &mut FilmcraftApp, i: usize) {
    if let Some(d) = app.ui.palette_library.draft.as_mut() {
        if let Some(c) = d.colors.get(i) {
            d.picker.close_native();
            d.picker = ColorDraft::new(c.hex.0);
        }
        d.editing = Some(i);
    }
}

fn editor_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let Some(mut d) = app.ui.palette_library.draft.take() else { return };
    if let Some(i) = d.editing.filter(|i| *i < d.colors.len()) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let others: Vec<ColorRow> = d.colors.iter().enumerate().filter(|(j, c)| *j != i && c.shade_of.is_empty()).map(|(_, c)| c.clone()).collect();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    color_picker::show(ui, &mut app.auto, &mut app.hooks, "paletteLibrary.picker", &mut d.picker);
                    color_picker::rgb_fields(ui, &mut app.auto, "paletteLibrary.picker", &mut d.picker);
                });
                ui.vertical(|ui| {
                    let Some(c) = d.colors.get_mut(i) else { return };
                    if let Some(v) = d.picker.value() {
                        c.hex = v;
                    }
                    egui::Grid::new("palette-color-fields").num_columns(2).show(ui, |ui| {
                        ui.label(tl!("Name"));
                        let r = ui.text_edit_singleline(&mut c.name);
                        app.auto.add_response("paletteLibrary.colorName", &r, tl!("Colour name"));
                        ui.end_row();
                        ui.label(tl!("Role"));
                        ui.horizontal(|ui| {
                            let r = craft_ui::ComboBox::from_id_salt("palette-role")
                                .selected_text(if c.role.is_empty() { tl!("None") } else { c.role.as_str() })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut c.role, String::new(), tl!("None"));
                                    for r in STANDARD_ROLES {
                                        let resp = ui.selectable_value(&mut c.role, (*r).to_string(), *r);
                                        app.auto.add_response(&format!("paletteLibrary.role.{r}"), &resp, r);
                                    }
                                });
                            app.auto.add_response("paletteLibrary.colorRole", &r.response, tl!("Role"));
                            let r = ui.add(egui::TextEdit::singleline(&mut c.role).hint_text(tl!("or your own")).desired_width(110.0));
                            app.auto.add_response("paletteLibrary.colorRoleText", &r, tl!("Custom role"));
                        });
                        ui.end_row();
                        ui.label(tl!("Shade of"));
                        let base =
                            others.iter().find(|o| o.id == c.shade_of).map(|o| o.name.clone()).unwrap_or_else(|| tl!("None (a base colour)").to_string());
                        let r = craft_ui::ComboBox::from_id_salt("palette-shade-of").selected_text(base).show_ui(ui, |ui| {
                            ui.selectable_value(&mut c.shade_of, String::new(), tl!("None (a base colour)"));
                            for o in &others {
                                let resp = ui.selectable_value(&mut c.shade_of, o.id.clone(), o.name.as_str());
                                app.auto.add_response(&format!("paletteLibrary.shadeOf.{}", o.name), &resp, &o.name);
                            }
                        });
                        app.auto.add_response("paletteLibrary.colorShadeOf", &r.response, tl!("Shade of"));
                        ui.end_row();
                        ui.label(tl!("Shade label"));
                        let r = ui.add_enabled(!c.shade_of.is_empty(), egui::TextEdit::singleline(&mut c.shade).hint_text("700, Deep, Light…"));
                        app.auto.add_response("paletteLibrary.colorShade", &r, tl!("Shade label"));
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        let r = ui.button(tl!("Done"));
                        app.auto.add_response("paletteLibrary.colorDone", &r, tl!("Done"));
                        if r.clicked() {
                            d.picker.close_native();
                            d.editing = None;
                        }
                        let r = ui.button(tl!("Duplicate"));
                        app.auto.add_response("paletteLibrary.colorDuplicate", &r, tl!("Duplicate colour"));
                        if r.clicked()
                            && let Some(src) = d.colors.get(i).cloned()
                        {
                            let mut name = format!("{} copy", src.name);
                            let mut n = 2;
                            while d.colors.iter().any(|c| c.name.eq_ignore_ascii_case(&name)) {
                                name = format!("{} copy {n}", src.name);
                                n += 1;
                            }
                            d.colors.insert(i + 1, ColorRow { id: fresh_color_id(), name, ..src });
                            d.editing = Some(i + 1);
                        }
                        for (label, key, delta) in [("◀", "colorLeft", -1i64), ("▶", "colorRight", 1)] {
                            let r = ui.button(label);
                            app.auto.add_response(&format!("paletteLibrary.{key}"), &r, tl!("Move colour"));
                            let j = i as i64 + delta;
                            if r.clicked() && j >= 0 && (j as usize) < d.colors.len() {
                                d.colors.swap(i, j as usize);
                                d.editing = Some(j as usize);
                            }
                        }
                        let r = ui.button(tl!("Remove"));
                        app.auto.add_response("paletteLibrary.colorRemove", &r, tl!("Remove colour"));
                        if r.clicked() {
                            let cid = d.colors.get(i).map(|c| c.id.clone()).unwrap_or_default();
                            let used = d.rules.iter().any(|r| r.foreground == cid || r.background == cid || r.colors.contains(&cid));
                            let shaded = d.colors.iter().any(|c| c.shade_of == cid);
                            if used || shaded {
                                app.ui.palette_library.error = tl!("This colour is used by a rule or has shades; change those first.").into();
                            } else {
                                d.colors.remove(i);
                                d.editing = None;
                            }
                        }
                    });
                });
            });
        });
    }
    app.ui.palette_library.draft = Some(d);
}

fn rule_preview(ui: &mut egui::Ui, fg: Option<HexColor>, bg: Option<HexColor>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(54.0, 26.0), egui::Sense::hover());
    let c = |h: Option<HexColor>| h.map(|h| egui::Color32::from_rgb(h.0[0], h.0[1], h.0[2]));
    if let Some(b) = c(bg) {
        ui.painter().rect_filled(rect, 3.0, b);
    }
    ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(1.0, ui.visuals().weak_text_color()), egui::StrokeKind::Inside);
    if let Some(f) = c(fg) {
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Aa", egui::FontId::proportional(16.0), f);
    }
}

fn rules_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, colors: &[ColorRow], rules: &[RuleRow], editable: bool) {
    ui.strong(tlf!("Usage rules ({n})", n = rules.len()));
    if rules.is_empty() {
        ui.small(tl!("No rules stated. FilmCraft adds none of its own."));
    }
    let hex = |id: &str| colors.iter().find(|c| c.id == id).map(|c| c.hex);
    let mut remove = None;
    for (i, r) in rules.iter().enumerate() {
        ui.horizontal(|ui| {
            let (mark, color) = match r.kind.as_str() {
                "permitted" => (tl!("Permitted"), ui.visuals().text_color()),
                "prohibited" => (tl!("Prohibited"), ui.visuals().error_fg_color),
                _ => (tl!("Guideline"), ui.visuals().weak_text_color()),
            };
            let resp = ui.colored_label(color, mark);
            app.auto.add_response(&format!("paletteLibrary.rule.{i}"), &resp, mark);
            if r.kind == "guideline" {
                let names: Vec<String> = r.colors.iter().map(|c| color_name(colors, c)).collect();
                ui.label(&r.text);
                if !names.is_empty() {
                    ui.small(format!("({})", names.join(", ")));
                }
            } else {
                rule_preview(ui, hex(&r.foreground), hex(&r.background));
                ui.label(tlf!("{fg} on {bg}", fg = color_name(colors, &r.foreground), bg = color_name(colors, &r.background)));
                ui.small(tlf!("for {what}", what = r.applies_to.as_str()));
                if !r.text.is_empty() {
                    ui.small(format!("— {}", r.text));
                }
            }
            if editable {
                let b = ui.small_button(tl!("Remove"));
                app.auto.add_response(&format!("paletteLibrary.rule.{i}.remove"), &b, tl!("Remove rule"));
                if b.clicked() {
                    remove = Some(i);
                }
            }
        });
    }
    if !editable {
        return;
    }
    let Some(mut d) = app.ui.palette_library.draft.take() else { return };
    if let Some(i) = remove.filter(|i| *i < d.rules.len()) {
        d.rules.remove(i);
    }
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.small(tl!("Add a rule"));
        let nr = &mut d.new_rule;
        ui.horizontal_wrapped(|ui| {
            let r = craft_ui::ComboBox::from_id_salt("palette-rule-kind").selected_text(nr.kind.as_str()).show_ui(ui, |ui| {
                for k in ["prohibited", "permitted", "guideline"] {
                    let resp = ui.selectable_value(&mut nr.kind, k.to_string(), k);
                    app.auto.add_response(&format!("paletteLibrary.newRule.kind.{k}"), &resp, k);
                }
            });
            app.auto.add_response("paletteLibrary.newRule.kind", &r.response, tl!("Rule kind"));
            if nr.kind == "guideline" {
                for c in colors {
                    let mut on = nr.colors.contains(&c.id);
                    let r = ui.checkbox(&mut on, c.name.as_str());
                    app.auto.add_response(&format!("paletteLibrary.newRule.color.{}", c.name), &r, &c.name);
                    if r.changed() {
                        if on {
                            nr.colors.push(c.id.clone());
                        } else {
                            nr.colors.retain(|x| x != &c.id);
                        }
                    }
                }
            } else {
                for (key, slot, label) in [("fg", &mut nr.foreground, tl!("Foreground")), ("bg", &mut nr.background, tl!("Background"))] {
                    let shown = colors.iter().find(|c| &c.id == slot).map(|c| c.name.clone()).unwrap_or_else(|| label.to_string());
                    let r = craft_ui::ComboBox::from_id_salt(format!("palette-rule-{key}")).selected_text(shown).show_ui(ui, |ui| {
                        for c in colors {
                            let resp = ui.selectable_value(slot, c.id.clone(), c.name.as_str());
                            app.auto.add_response(&format!("paletteLibrary.newRule.{key}.{}", c.name), &resp, &c.name);
                        }
                    });
                    app.auto.add_response(&format!("paletteLibrary.newRule.{key}"), &r.response, label);
                }
                let r = craft_ui::ComboBox::from_id_salt("palette-rule-applies").selected_text(nr.applies_to.as_str()).show_ui(ui, |ui| {
                    for a in ["text", "graphics", "any"] {
                        ui.selectable_value(&mut nr.applies_to, a.to_string(), a);
                    }
                });
                app.auto.add_response("paletteLibrary.newRule.appliesTo", &r.response, tl!("Applies to"));
            }
            let r = ui.add(egui::TextEdit::singleline(&mut nr.text).hint_text(tl!("The rule in your words")).desired_width(220.0));
            app.auto.add_response("paletteLibrary.newRule.text", &r, tl!("Rule text"));
        });
        let ready = if nr.kind == "guideline" {
            !nr.text.trim().is_empty()
        } else {
            !nr.foreground.is_empty() && !nr.background.is_empty() && nr.foreground != nr.background
        };
        let r = ui.add_enabled(ready, egui::Button::new(tl!("Add Rule")));
        app.auto.add_response("paletteLibrary.addRule", &r, tl!("Add rule"));
        if r.clicked() {
            let mut rule = std::mem::replace(&mut d.new_rule, empty_rule());
            rule.id = fresh_rule_id();
            if rule.kind == "guideline" {
                // a written guideline has no foreground/background scope
                rule.foreground.clear();
                rule.background.clear();
                rule.applies_to = "any".into();
            } else {
                rule.colors.clear();
            }
            d.rules.push(rule);
        }
    });
    app.ui.palette_library.draft = Some(d);
}

fn draft_params(d: &Draft) -> Value {
    let opt = |s: &str| if s.is_empty() { Value::Null } else { json!(s) };
    let colors: Vec<Value> = d
        .colors
        .iter()
        .map(|c| json!({"id": c.id, "name": c.name, "hex": c.hex.hex(), "role": c.role, "shadeOf": opt(&c.shade_of), "shade": c.shade}))
        .collect();
    let rules: Vec<Value> = d
        .rules
        .iter()
        .map(|r| json!({"id": r.id, "kind": r.kind, "foreground": opt(&r.foreground), "background": opt(&r.background), "colors": r.colors, "appliesTo": r.applies_to, "text": r.text}))
        .collect();
    json!({"id": d.palette, "baseVersion": d.base_version, "colors": colors, "rules": rules, "note": d.note, "by": "user"})
}

fn save_bar(app: &mut FilmcraftApp, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    let Some(mut d) = app.ui.palette_library.draft.take() else { return false };
    ui.separator();
    ui.horizontal(|ui| {
        let dirty = d.dirty();
        if dirty {
            ui.colored_label(ui.visuals().warn_fg_color, tl!("Unsaved changes"));
        } else {
            ui.label(tlf!("Saved as version {v}", v = d.base_version));
        }
        let r = ui.add_enabled(dirty, egui::TextEdit::singleline(&mut d.note).hint_text(tl!("What changed (optional)")).desired_width(240.0));
        app.auto.add_response("paletteLibrary.note", &r, tl!("Version note"));
        let r = ui.add_enabled(dirty, egui::Button::new(tl!("Save Version")));
        app.auto.add_response("paletteLibrary.save", &r, tl!("Save as a new version"));
        if r.clicked() {
            match app.session.execute("palettes.update", draft_params(&d)) {
                Ok(v) => {
                    app.ui.palette_library.notice = tlf!("Saved version {v} of {name}", v = v["latestVersion"], name = v["name"].as_str().unwrap_or(""));
                    app.ui.palette_library.error.clear();
                    d.picker.close_native();
                    // the next refresh loads the saved version as the new base
                    d.original = (d.colors.clone(), d.rules.clone());
                    d.base_version = 0;
                    d.editing = None;
                    changed = true;
                }
                Err(e) => app.ui.palette_library.error = e.to_string(),
            }
        }
        let r = ui.add_enabled(dirty, egui::Button::new(tl!("Discard")));
        app.auto.add_response("paletteLibrary.discard", &r, tl!("Discard changes"));
        if r.clicked() {
            d.colors = d.original.0.clone();
            d.rules = d.original.1.clone();
            d.editing = None;
            d.picker.close_native();
        }
    });
    app.ui.palette_library.draft = Some(d);
    changed
}
