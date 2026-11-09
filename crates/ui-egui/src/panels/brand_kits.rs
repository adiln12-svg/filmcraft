//! Window ▸ Brand Kits: the Client Brand Kit Builder (engine `brand_kits`).
//!
//! Clients on the left (search, create, rename, archive) with their kits; the selected kit in
//! tabs: Overview (versions, dependency check, save draft, approve, archive, pin to the current
//! project), Fonts (exact Font Library faces), Colours (exact palette versions and colour roles),
//! Typography (roles with font, weight, size, colours, alignment, placement, case, tracking…),
//! Logos (the client's assets: import, preview, remove; kit usage rules), Rules (kit rules and the
//! palettes' own rules), Preview (the kit drawn on a 9:16 or 16:9 frame) and History.
//!
//! Every change goes through `brand.*` commands, so agents use the same paths. Approved versions
//! are read only; editing one saves a new draft version. Kits managed by ARCA are read only.

use std::collections::HashMap;

use filmcraft_engine::brand_kits::{
    ALTERATIONS, ANCHORS, ColorRef, ColorRole, KitContent, KitFont, KitPalette, KitRule, LogoUse, STANDARD_TYPE_ROLES, Size, TypeRole,
};
use filmcraft_project::HexColor;
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::native_dialogs::FileDialog;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Overview,
    Fonts,
    Colors,
    Typography,
    Logos,
    Rules,
    Preview,
    History,
}

const TABS: &[(Tab, &str, &str)] = &[
    (Tab::Overview, "Overview", "overview"),
    (Tab::Fonts, "Fonts", "fonts"),
    (Tab::Colors, "Colours", "colors"),
    (Tab::Typography, "Typography", "typography"),
    (Tab::Logos, "Logos", "logos"),
    (Tab::Rules, "Rules", "rules"),
    (Tab::Preview, "Preview", "preview"),
    (Tab::History, "History", "history"),
];

/// Unsaved changes to a kit.
#[derive(Clone, Debug)]
pub struct Draft {
    pub kit: String,
    pub base: u32,
    pub content: KitContent,
    pub original: KitContent,
    pub note: String,
}

#[derive(Clone, Debug, Default)]
pub struct PendingAsset {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub variant: String,
    pub source: String,
    pub license: String,
}

/// A palette version's colours and rules, for pickers.
#[derive(Clone, Debug, Default)]
pub struct PaletteInfo {
    pub name: String,
    pub colors: Vec<(String, String, HexColor)>,
    pub rules: Vec<String>,
}

#[derive(Default)]
pub struct State {
    pub open: bool,
    pub query: String,
    pub show_archived: bool,
    pub client: Option<String>,
    pub kit: Option<String>,
    pub view_version: Option<u32>,
    pub tab: Tab,
    pub draft: Option<Draft>,
    pub new_client: Option<String>,
    pub rename_client: Option<String>,
    pub new_kit: Option<String>,
    pub rename_kit: Option<String>,
    pub approve: Option<(String, String, bool)>,
    pub pending_asset: Option<PendingAsset>,
    pub pin_authorize: bool,
    pub pin_replace_client: bool,
    pub role_sel: usize,
    pub logo_sel: usize,
    pub new_role: String,
    pub new_rule: Option<KitRule>,
    pub add_palette: Option<(String, u32)>,
    pub preview_portrait: bool,
    pub samples: HashMap<String, String>,
    pub error: String,
    pub notice: String,
    clients: Option<Value>,
    kit_json: Option<Value>,
    check: Option<Value>,
    assets: Option<Value>,
    fonts: Option<Value>,
    palettes: Option<Value>,
    pin: Option<Value>,
    palette_cache: HashMap<String, PaletteInfo>,
    textures: HashMap<String, egui::TextureHandle>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("brand_kits::State").field("open", &self.open).field("client", &self.client).field("kit", &self.kit).finish()
    }
}

impl Clone for State {
    fn clone(&self) -> Self {
        State {
            open: self.open,
            query: self.query.clone(),
            show_archived: self.show_archived,
            client: self.client.clone(),
            kit: self.kit.clone(),
            view_version: self.view_version,
            tab: self.tab,
            draft: self.draft.clone(),
            preview_portrait: self.preview_portrait,
            ..Default::default()
        }
    }
}

pub fn open(app: &mut FilmcraftApp) {
    let st = &mut app.ui.brand_kits;
    st.open = true;
    st.preview_portrait = true;
    invalidate(st);
}

fn invalidate(st: &mut State) {
    st.clients = None;
    st.kit_json = None;
    st.check = None;
    st.assets = None;
    st.fonts = None;
    st.palettes = None;
    st.pin = None;
}

pub fn route(app: &mut FilmcraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    match id {
        "window.brandKits" if params.get("close").and_then(Value::as_bool) == Some(true) => {
            app.ui.brand_kits.open = false;
            Some(Ok(json!({"open": false})))
        }
        "window.brandKits" => {
            open(app);
            if let Some(c) = params.get("client").and_then(Value::as_str) {
                app.ui.brand_kits.client = Some(c.to_string());
            }
            if let Some(k) = params.get("kit").and_then(Value::as_str) {
                select_kit(app, k.to_string());
            }
            if let Some(t) = params.get("tab").and_then(Value::as_str)
                && let Some((tab, _, _)) = TABS.iter().find(|(_, _, key)| *key == t)
            {
                app.ui.brand_kits.tab = *tab;
            }
            Some(Ok(json!({"open": true})))
        }
        "brandKits.queueAsset" => {
            let path = params.get("path").and_then(Value::as_str)?.to_string();
            open(app);
            queue_asset(app, vec![path]);
            Some(Ok(json!({"pending": true})))
        }
        _ => None,
    }
}

fn run(app: &mut FilmcraftApp, cmd: &str, p: Value) -> Option<Value> {
    match app.session.execute(cmd, p) {
        Ok(v) => {
            app.ui.brand_kits.error.clear();
            Some(v)
        }
        Err(e) => {
            app.ui.brand_kits.error = e.to_string();
            None
        }
    }
}

fn select_kit(app: &mut FilmcraftApp, id: String) {
    let st = &mut app.ui.brand_kits;
    st.kit = Some(id);
    st.view_version = None;
    st.draft = None;
    st.approve = None;
    st.rename_kit = None;
    st.kit_json = None;
    st.check = None;
    st.role_sel = 0;
    st.logo_sel = 0;
}

fn refresh(app: &mut FilmcraftApp) {
    let q = app.ui.brand_kits.query.clone();
    let archived = app.ui.brand_kits.show_archived;
    if app.ui.brand_kits.clients.is_none() {
        let v = app
            .session
            .execute("brand.clients.list", json!({"query": q, "includeArchived": archived}))
            .unwrap_or_else(|e| json!({"clients": [], "error": e.to_string()}));
        app.ui.brand_kits.clients = Some(v);
    }
    if app.ui.brand_kits.fonts.is_none() {
        app.ui.brand_kits.fonts = app.session.execute("fonts.library.list", json!({})).ok();
    }
    if app.ui.brand_kits.palettes.is_none() {
        app.ui.brand_kits.palettes = app.session.execute("palettes.list", json!({})).ok();
    }
    if app.ui.brand_kits.pin.is_none() {
        app.ui.brand_kits.pin = app.session.execute("brand.project.get", json!({})).ok();
    }
    if let Some(c) = app.ui.brand_kits.client.clone()
        && app.ui.brand_kits.assets.is_none()
    {
        app.ui.brand_kits.assets = app.session.execute("brand.assets.list", json!({"client": c})).ok();
    }
    let Some(kit) = app.ui.brand_kits.kit.clone() else { return };
    if app.ui.brand_kits.kit_json.is_none() {
        let mut p = json!({"id": kit, "history": true});
        if let Some(v) = app.ui.brand_kits.view_version {
            p["version"] = json!(v);
        }
        match app.session.execute("brand.kits.get", p) {
            Ok(v) => {
                let st = &mut app.ui.brand_kits;
                if st.view_version.is_none() && st.draft.as_ref().is_none_or(|d| d.content == d.original) {
                    let content: KitContent = serde_json::from_value(v["content"].clone()).unwrap_or_default();
                    st.draft = Some(Draft {
                        kit: kit.clone(),
                        base: v["latestVersion"].as_u64().unwrap_or(0) as u32,
                        original: content.clone(),
                        content,
                        note: String::new(),
                    });
                }
                st.kit_json = Some(v);
            }
            Err(e) => {
                let st = &mut app.ui.brand_kits;
                st.error = e.to_string();
                st.kit = None;
                st.draft = None;
            }
        }
        let mut p = json!({"id": kit});
        if let Some(v) = app.ui.brand_kits.view_version {
            p["version"] = json!(v);
        }
        app.ui.brand_kits.check = app.session.execute("brand.kits.check", p).ok();
    }
    // palette versions the draft uses
    let refs: Vec<KitPalette> = app.ui.brand_kits.draft.as_ref().map(|d| d.content.palettes.clone()).unwrap_or_default();
    let viewed: Vec<KitPalette> =
        app.ui.brand_kits.kit_json.as_ref().and_then(|k| serde_json::from_value(k["content"]["palettes"].clone()).ok()).unwrap_or_default();
    for p in refs.iter().chain(viewed.iter()) {
        let key = format!("{}@{}", p.palette, p.version);
        if app.ui.brand_kits.palette_cache.contains_key(&key) {
            continue;
        }
        let info = app
            .session
            .execute("palettes.get", json!({"id": p.palette, "version": p.version}))
            .map(|v| PaletteInfo {
                name: v["name"].as_str().unwrap_or("").to_string(),
                colors: v["version"]["colors"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|c| {
                                (
                                    c["id"].as_str().unwrap_or("").to_string(),
                                    c["name"].as_str().unwrap_or("").to_string(),
                                    c["hex"].as_str().and_then(HexColor::parse).unwrap_or(HexColor([128, 128, 128])),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                rules: v["version"]["rules"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|r| match r["kind"].as_str() {
                                Some("guideline") => format!("Guideline: {}", r["text"].as_str().unwrap_or("")),
                                Some(k) => format!(
                                    "{}: {} on {} ({}) {}",
                                    k,
                                    r["foregroundName"].as_str().unwrap_or(""),
                                    r["backgroundName"].as_str().unwrap_or(""),
                                    r["appliesTo"].as_str().unwrap_or(""),
                                    r["text"].as_str().unwrap_or("")
                                ),
                                None => String::new(),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .unwrap_or_default();
        app.ui.brand_kits.palette_cache.insert(key, info);
    }
}

/// Colour choices of a kit content: (label, reference, value).
fn color_options(st: &State, c: &KitContent) -> Vec<(String, ColorRef, HexColor)> {
    let mut out = Vec::new();
    for p in &c.palettes {
        if let Some(info) = st.palette_cache.get(&format!("{}@{}", p.palette, p.version)) {
            for (id, name, hex) in &info.colors {
                out.push((format!("{name} ({} v{})", info.name, p.version), ColorRef { palette: p.palette.clone(), color: id.clone() }, *hex));
            }
        }
    }
    out
}

fn color_label(opts: &[(String, ColorRef, HexColor)], r: &Option<ColorRef>) -> String {
    match r {
        None => tl!("None").to_string(),
        Some(r) => opts.iter().find(|(_, x, _)| x == r).map(|(l, _, h)| format!("{l} {}", h.hex())).unwrap_or_else(|| format!("{} (missing)", r.color)),
    }
}

fn color_hex(opts: &[(String, ColorRef, HexColor)], r: &Option<ColorRef>) -> Option<HexColor> {
    r.as_ref().and_then(|r| opts.iter().find(|(_, x, _)| x == r).map(|(_, _, h)| *h))
}

fn color_combo(
    ui: &mut egui::Ui,
    app_auto: &mut crate::automation::Registry,
    id: &str,
    opts: &[(String, ColorRef, HexColor)],
    value: &mut Option<ColorRef>,
    allow_none: bool,
) {
    let r = craft_ui::ComboBox::from_id_salt(id).selected_text(color_label(opts, value)).width(230.0).show_ui(ui, |ui| {
        if allow_none {
            let resp = ui.selectable_label(value.is_none(), tl!("None"));
            if resp.clicked() {
                *value = None;
            }
        }
        for (label, r, hex) in opts {
            let resp = ui.selectable_label(value.as_ref() == Some(r), format!("{label} {}", hex.hex()));
            app_auto.add_response(&format!("{id}.{label}"), &resp, label);
            if resp.clicked() {
                *value = Some(r.clone());
            }
        }
    });
    app_auto.add_response(id, &r.response, id);
}

fn swatch(ui: &mut egui::Ui, hex: Option<HexColor>, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    if let Some(h) = hex {
        ui.painter().rect_filled(rect, 2.0, egui::Color32::from_rgb(h.0[0], h.0[1], h.0[2]));
    }
    ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0, ui.visuals().weak_text_color()), egui::StrokeKind::Inside);
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.ui.brand_kits.open {
        return;
    }
    refresh(app);
    let mut open = true;
    let win =
        egui::Window::new(tl!("Brand Kits")).id(egui::Id::unique("brand-kits")).open(&mut open).default_size([1120.0, 760.0]).resizable(true).show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(250.0);
                    clients_ui(app, ui);
                });
                ui.separator();
                ui.vertical(|ui| kit_ui(app, ui));
            });
            let st = &app.ui.brand_kits;
            if !st.error.is_empty() {
                ui.colored_label(ui.visuals().error_fg_color, st.error.clone());
            }
            if !st.notice.is_empty() {
                ui.label(st.notice.clone());
            }
        });
    if let Some(w) = &win {
        app.auto.add_response("brandKits.window", &w.response, tl!("Brand Kits"));
    }
    if !open {
        app.ui.brand_kits.open = false;
    }
}

// ------------------------------------------------------------------ clients column

fn clients_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let r = ui.add(egui::TextEdit::singleline(&mut app.ui.brand_kits.query).hint_text(tl!("Search clients")).desired_width(230.0));
    app.auto.add_response("brandKits.search", &r, tl!("Search clients"));
    let mut changed = r.changed();
    let r = ui.checkbox(&mut app.ui.brand_kits.show_archived, tl!("Show archived"));
    app.auto.add_response("brandKits.showArchived", &r, tl!("Show archived"));
    changed |= r.changed();
    if changed {
        app.ui.brand_kits.clients = None;
    }
    let r = ui.button(tl!("New Client…"));
    app.auto.add_response("brandKits.newClient", &r, tl!("New client"));
    if r.clicked() {
        app.ui.brand_kits.new_client = Some(String::new());
    }
    if let Some(mut name) = app.ui.brand_kits.new_client.take() {
        let mut keep = true;
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut name).hint_text(tl!("Client name")).desired_width(140.0));
            app.auto.add_response("brandKits.newClientName", &r, tl!("Client name"));
            let r = ui.add_enabled(!name.trim().is_empty(), egui::Button::new(tl!("Create")));
            app.auto.add_response("brandKits.createClient", &r, tl!("Create client"));
            if r.clicked()
                && let Some(v) = run(app, "brand.clients.create", json!({"name": name, "by": "user"}))
            {
                app.ui.brand_kits.client = v["id"].as_str().map(str::to_string);
                app.ui.brand_kits.kit = None;
                app.ui.brand_kits.draft = None;
                invalidate(&mut app.ui.brand_kits);
                keep = false;
            }
            if ui.button("✕").clicked() {
                keep = false;
            }
        });
        if keep {
            app.ui.brand_kits.new_client = Some(name);
        }
    }
    ui.separator();
    let clients = app.ui.brand_kits.clients.clone().unwrap_or(Value::Null);
    egui::ScrollArea::vertical().id_salt("brand-clients").max_height(ui.available_height() - 30.0).show(ui, |ui| {
        let list = clients["clients"].as_array().cloned().unwrap_or_default();
        if list.is_empty() {
            ui.label(tl!("No clients yet."));
        }
        for c in list.iter().take(2000) {
            let id = c["id"].as_str().unwrap_or("").to_string();
            let mut label = c["name"].as_str().unwrap_or("").to_string();
            if c["archived"].as_bool() == Some(true) {
                label.push_str(tl!(" (archived)"));
            }
            if c["source"].as_str().is_some_and(|s| s.starts_with("arca:")) {
                label.push_str(" · ARCA");
            }
            let sel = app.ui.brand_kits.client.as_deref() == Some(id.as_str());
            let r = ui.selectable_label(sel, egui::RichText::new(&label).strong());
            app.auto.add_response(&format!("brandKits.client.{id}"), &r, &label);
            if r.clicked() && !sel {
                if app.ui.brand_kits.draft.as_ref().is_some_and(|d| d.content != d.original) {
                    app.ui.brand_kits.error = tl!("Save or discard the kit's changes first.").into();
                } else {
                    let st = &mut app.ui.brand_kits;
                    st.client = Some(id.clone());
                    st.kit = None;
                    st.draft = None;
                    st.assets = None;
                    st.rename_client = None;
                }
            }
            if !sel {
                continue;
            }
            // the selected client: actions and kits
            ui.indent(format!("cl-{id}"), |ui| {
                if let Some(mut n) = app.ui.brand_kits.rename_client.take() {
                    ui.horizontal(|ui| {
                        let r = ui.add(egui::TextEdit::singleline(&mut n).desired_width(120.0));
                        app.auto.add_response("brandKits.renameClientField", &r, tl!("Client name"));
                        let ok = ui.button(tl!("Rename"));
                        app.auto.add_response("brandKits.renameClientApply", &ok, tl!("Rename"));
                        if ok.clicked() {
                            if run(app, "brand.clients.update", json!({"id": id, "name": n, "by": "user"})).is_some() {
                                app.ui.brand_kits.clients = None;
                            }
                        } else if !ui.button("✕").clicked() {
                            app.ui.brand_kits.rename_client = Some(n);
                        }
                    });
                }
                ui.horizontal(|ui| {
                    let r = ui.small_button(tl!("Rename…"));
                    app.auto.add_response("brandKits.renameClient", &r, tl!("Rename client"));
                    if r.clicked() {
                        app.ui.brand_kits.rename_client = Some(c["name"].as_str().unwrap_or("").to_string());
                    }
                    let archived = c["archived"].as_bool().unwrap_or(false);
                    let r = ui.small_button(if archived { tl!("Restore") } else { tl!("Archive") });
                    app.auto.add_response("brandKits.archiveClient", &r, tl!("Archive client"));
                    if r.clicked() && run(app, "brand.clients.update", json!({"id": id, "archived": !archived, "by": "user"})).is_some() {
                        app.ui.brand_kits.clients = None;
                    }
                });
                for k in c["kits"].as_array().cloned().unwrap_or_default() {
                    let kid = k["id"].as_str().unwrap_or("").to_string();
                    if k["archived"].as_bool() == Some(true) && !app.ui.brand_kits.show_archived {
                        continue;
                    }
                    let status = match (k["approvedVersion"].as_u64(), k["latestStatus"].as_str()) {
                        (Some(a), Some("draft")) => format!("v{a} approved · v{} draft", k["latestVersion"]),
                        (Some(a), _) => format!("v{a} approved"),
                        (None, _) => format!("v{} draft", k["latestVersion"]),
                    };
                    let label = format!(
                        "{}  ·  {status}{}",
                        k["name"].as_str().unwrap_or(""),
                        if k["archived"].as_bool() == Some(true) { tl!(" (archived)") } else { "" }
                    );
                    let sel = app.ui.brand_kits.kit.as_deref() == Some(kid.as_str());
                    let r = ui.selectable_label(sel, label.as_str());
                    app.auto.add_response(&format!("brandKits.kit.{kid}"), &r, &label);
                    if r.clicked() && !sel {
                        if app.ui.brand_kits.draft.as_ref().is_some_and(|d| d.content != d.original) {
                            app.ui.brand_kits.error = tl!("Save or discard the kit's changes first.").into();
                        } else {
                            select_kit(app, kid.clone());
                        }
                    }
                }
                let r = ui.small_button(tl!("New Brand Kit…"));
                app.auto.add_response("brandKits.newKit", &r, tl!("New brand kit"));
                if r.clicked() {
                    app.ui.brand_kits.new_kit = Some(String::new());
                }
                if let Some(mut name) = app.ui.brand_kits.new_kit.take() {
                    let mut keep = true;
                    ui.horizontal(|ui| {
                        let r = ui.add(egui::TextEdit::singleline(&mut name).hint_text(tl!("Kit name")).desired_width(120.0));
                        app.auto.add_response("brandKits.newKitName", &r, tl!("Kit name"));
                        let r = ui.add_enabled(!name.trim().is_empty(), egui::Button::new(tl!("Create")));
                        app.auto.add_response("brandKits.createKit", &r, tl!("Create brand kit"));
                        if r.clicked()
                            && let Some(v) = run(app, "brand.kits.create", json!({"client": id, "name": name, "by": "user"}))
                        {
                            app.ui.brand_kits.clients = None;
                            select_kit(app, v["id"].as_str().unwrap_or("").to_string());
                            keep = false;
                        }
                        if ui.button("✕").clicked() {
                            keep = false;
                        }
                    });
                    if keep {
                        app.ui.brand_kits.new_kit = Some(name);
                    }
                }
            });
        }
    });
}

// ------------------------------------------------------------------ kit editor

fn kit_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let Some(kit) = app.ui.brand_kits.kit_json.clone() else {
        ui.label(if app.ui.brand_kits.client.is_some() { tl!("Select or create a brand kit for this client.") } else { tl!("Select or create a client.") });
        return;
    };
    let latest = kit["latestVersion"].as_u64().unwrap_or(0) as u32;
    let managed = kit["managedBy"].as_str().map(str::to_string);
    let viewing = app.ui.brand_kits.view_version.filter(|v| *v != latest);
    let archived = kit["archived"].as_bool().unwrap_or(false);
    let editable = viewing.is_none() && managed.is_none() && !archived;
    ui.horizontal(|ui| {
        ui.heading(kit["name"].as_str().unwrap_or(""));
        ui.label(format!("· {}", kit["clientName"].as_str().unwrap_or("")));
        let r = ui.label(egui::RichText::new(kit["id"].as_str().unwrap_or("")).monospace().small());
        app.auto.add_response("brandKits.kitId", &r, kit["id"].as_str().unwrap_or(""));
        if let Some(m) = &managed {
            ui.colored_label(ui.visuals().warn_fg_color, tlf!("Managed by {m}: read only", m = m));
        }
    });
    ui.horizontal(|ui| {
        for (tab, label, key) in TABS {
            let r = ui.selectable_label(app.ui.brand_kits.tab == *tab, *label);
            app.auto.add_response(&format!("brandKits.tab.{key}"), &r, label);
            if r.clicked() {
                app.ui.brand_kits.tab = *tab;
            }
        }
    });
    ui.separator();
    // what is shown: the draft (editable) or the viewed version
    let mut content: KitContent = if editable {
        app.ui.brand_kits.draft.as_ref().map(|d| d.content.clone()).unwrap_or_default()
    } else {
        serde_json::from_value(kit["content"].clone()).unwrap_or_default()
    };
    let before = content.clone();
    let h = ui.available_height() - 40.0;
    egui::ScrollArea::vertical().id_salt("brand-kit-tab").max_height(h.max(100.0)).show(ui, |ui| match app.ui.brand_kits.tab {
        Tab::Overview => overview_ui(app, ui, &kit, editable),
        Tab::Fonts => fonts_ui(app, ui, &mut content, editable),
        Tab::Colors => colors_ui(app, ui, &mut content, editable),
        Tab::Typography => typography_ui(app, ui, &mut content, editable),
        Tab::Logos => logos_ui(app, ui, &mut content, editable),
        Tab::Rules => rules_ui(app, ui, &mut content, editable),
        Tab::Preview => preview_ui(app, ui, &content),
        Tab::History => history_ui(app, ui, &kit),
    });
    if editable
        && content != before
        && let Some(d) = app.ui.brand_kits.draft.as_mut()
    {
        d.content = content;
    }
    if editable {
        save_bar(app, ui, &kit);
    }
}

fn save_bar(app: &mut FilmcraftApp, ui: &mut egui::Ui, kit: &Value) {
    let Some(mut d) = app.ui.brand_kits.draft.take() else { return };
    let dirty = d.content != d.original;
    let latest_status = kit["latestStatus"].as_str().unwrap_or("").to_string();
    ui.separator();
    ui.horizontal(|ui| {
        if dirty {
            let msg = if latest_status == "approved" {
                tlf!("Unsaved changes (saving starts draft version {v}; approved version {a} stays as it is)", v = d.base + 1, a = d.base)
            } else {
                tl!("Unsaved changes").to_string()
            };
            ui.colored_label(ui.visuals().warn_fg_color, msg);
        } else {
            ui.label(tlf!("Version {v} · {s}", v = d.base, s = latest_status));
        }
        let r = ui.add_enabled(dirty, egui::TextEdit::singleline(&mut d.note).hint_text(tl!("What changed (optional)")).desired_width(200.0));
        app.auto.add_response("brandKits.note", &r, tl!("Draft note"));
        let r = ui.add_enabled(dirty, egui::Button::new(tl!("Save Draft")));
        app.auto.add_response("brandKits.saveDraft", &r, tl!("Save draft"));
        if r.clicked() {
            let p = json!({"id": d.kit, "baseVersion": d.base, "content": d.content, "by": "user", "note": d.note});
            if let Some(v) = run(app, "brand.kits.saveDraft", p) {
                app.ui.brand_kits.notice = tlf!("Saved draft version {v}", v = v["savedVersion"]);
                d.original = d.content.clone();
                d.base = 0;
                d.note.clear();
                app.ui.brand_kits.kit_json = None;
                app.ui.brand_kits.clients = None;
            }
        }
        let r = ui.add_enabled(dirty, egui::Button::new(tl!("Discard")));
        app.auto.add_response("brandKits.discard", &r, tl!("Discard changes"));
        if r.clicked() {
            d.content = d.original.clone();
        }
    });
    // keep the draft unless the save reloads it
    if d.base != 0 || app.ui.brand_kits.kit_json.is_some() {
        app.ui.brand_kits.draft = Some(d);
    } else {
        app.ui.brand_kits.draft = None;
    }
}

fn overview_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, kit: &Value, editable: bool) {
    let id = kit["id"].as_str().unwrap_or("").to_string();
    let latest = kit["latestVersion"].as_u64().unwrap_or(0) as u32;
    let shown = app.ui.brand_kits.view_version.unwrap_or(latest);
    // versions
    ui.horizontal(|ui| {
        ui.label(tl!("Version"));
        let versions = kit["versions"].as_array().cloned().unwrap_or_default();
        let label = |v: &Value| {
            let appr = v["approval"]["by"].as_str().map(|b| format!(" · approved by {b}")).unwrap_or_default();
            format!("v{} · {} · {}{appr}", v["version"], v["status"].as_str().unwrap_or(""), v["updated"].as_str().unwrap_or(""))
        };
        let text = versions.iter().find(|v| v["version"].as_u64() == Some(u64::from(shown))).map(label).unwrap_or_default();
        let dirty = app.ui.brand_kits.draft.as_ref().is_some_and(|d| d.content != d.original);
        let r = ui.add_enabled_ui(!dirty, |ui| {
            craft_ui::ComboBox::from_id_salt("brand-version").selected_text(text).width(480.0).show_ui(ui, |ui| {
                for v in versions.iter().rev() {
                    let n = v["version"].as_u64().unwrap_or(0) as u32;
                    let r = ui.selectable_label(n == shown, label(v));
                    app.auto.add_response(&format!("brandKits.version.{n}"), &r, &label(v));
                    if r.clicked() {
                        app.ui.brand_kits.view_version = if n == latest { None } else { Some(n) };
                        app.ui.brand_kits.kit_json = None;
                    }
                }
            })
        });
        app.auto.add_response("brandKits.versionPicker", &r.response, tl!("Version"));
    });
    if app.ui.brand_kits.view_version.is_some_and(|v| v != latest) {
        ui.colored_label(ui.visuals().warn_fg_color, tl!("An earlier version: read only."));
        let r = ui.button(tl!("Back to Latest"));
        app.auto.add_response("brandKits.latest", &r, tl!("Back to latest"));
        if r.clicked() {
            app.ui.brand_kits.view_version = None;
            app.ui.brand_kits.kit_json = None;
        }
    }
    let v = kit["versions"].as_array().and_then(|a| a.iter().find(|x| x["version"].as_u64() == Some(u64::from(shown)))).cloned().unwrap_or(Value::Null);
    egui::Grid::new("brand-overview").num_columns(2).show(ui, |ui| {
        ui.label(tl!("Status"));
        let approved = v["status"] == json!("approved");
        let r = ui.colored_label(
            if approved { ui.visuals().text_color() } else { ui.visuals().warn_fg_color },
            if approved { tl!("Approved (frozen)") } else { tl!("Draft") },
        );
        app.auto.add_response("brandKits.status", &r, if approved { "approved" } else { "draft" });
        ui.end_row();
        if approved {
            ui.label(tl!("Approval"));
            ui.label(format!(
                "{} · {} · {} ({})",
                v["approval"]["by"].as_str().unwrap_or(""),
                v["approval"]["reference"].as_str().unwrap_or(""),
                v["approval"]["at"].as_str().unwrap_or(""),
                v["approval"]["authority"].as_str().unwrap_or("")
            ));
            ui.end_row();
        }
        ui.label(tl!("Fingerprint"));
        ui.label(egui::RichText::new(v["sha256"].as_str().unwrap_or("")).monospace().small());
        ui.end_row();
        ui.label(tl!("Reference"));
        let r = ui.label(egui::RichText::new(format!("{id}@{shown}")).monospace());
        app.auto.add_response("brandKits.ref", &r, &format!("{id}@{shown}"));
        ui.end_row();
    });
    // dependency check
    ui.separator();
    let check = app.ui.brand_kits.check.clone().unwrap_or(Value::Null);
    let issues = check["issues"].as_array().cloned().unwrap_or_default();
    let ready = check["ready"].as_bool().unwrap_or(false);
    let dirty = app.ui.brand_kits.draft.as_ref().is_some_and(|d| d.content != d.original);
    let head = if ready { tl!("Dependencies: all fonts, palette versions and logos present and exact") } else { tl!("Dependencies: problems found") };
    let r = ui.colored_label(if ready { ui.visuals().text_color() } else { ui.visuals().error_fg_color }, head);
    app.auto.add_response("brandKits.dependencies", &r, head);
    if dirty {
        ui.small(tl!("(checked for the saved version; save the draft to check your changes)"));
    }
    for i in issues.iter().take(50) {
        let col = if i["severity"] == json!("error") { ui.visuals().error_fg_color } else { ui.visuals().warn_fg_color };
        ui.colored_label(
            col,
            format!("{} · {}: {}", i["severity"].as_str().unwrap_or(""), i["area"].as_str().unwrap_or(""), i["detail"].as_str().unwrap_or("")),
        );
    }
    // approve
    ui.separator();
    let is_draft = v["status"] == json!("draft") && shown == latest;
    if is_draft && editable {
        if app.ui.brand_kits.approve.is_none() {
            let r = ui.add_enabled(ready && !dirty, egui::Button::new(tl!("Approve Version…")));
            app.auto.add_response("brandKits.approve", &r, tl!("Approve this version"));
            if r.clicked() {
                app.ui.brand_kits.approve = Some((String::new(), String::new(), false));
            }
            if !ready || dirty {
                ui.small(tl!("Approval needs a saved draft without dependency errors."));
            }
        }
        if let Some((mut who, mut reference, mut confirm)) = app.ui.brand_kits.approve.take() {
            let mut keep = true;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(tlf!("Approve version {v}: it is frozen, and projects can pin it.", v = shown));
                egui::Grid::new("brand-approve").num_columns(2).show(ui, |ui| {
                    ui.label(tl!("Approved by"));
                    let r = ui.text_edit_singleline(&mut who);
                    app.auto.add_response("brandKits.approveBy", &r, tl!("Approved by"));
                    ui.end_row();
                    ui.label(tl!("Reference"));
                    let r = ui.add(egui::TextEdit::singleline(&mut reference).hint_text(tl!("The decision: meeting, email, ticket…")));
                    app.auto.add_response("brandKits.approveReference", &r, tl!("Approval reference"));
                    ui.end_row();
                });
                let r = ui.checkbox(&mut confirm, tl!("I have the authority to approve this client's branding"));
                app.auto.add_response("brandKits.approveConfirm", &r, tl!("Confirm approval"));
                ui.horizontal(|ui| {
                    let r = ui.add_enabled(confirm && !who.trim().is_empty() && !reference.trim().is_empty(), egui::Button::new(tl!("Approve")));
                    app.auto.add_response("brandKits.approveApply", &r, tl!("Approve"));
                    if r.clicked()
                        && run(app, "brand.kits.approve", json!({"id": id, "version": shown, "confirm": true, "approval": {"by": who, "reference": reference}}))
                            .is_some()
                    {
                        app.ui.brand_kits.notice = tlf!("Approved version {v}", v = shown);
                        app.ui.brand_kits.kit_json = None;
                        app.ui.brand_kits.clients = None;
                        keep = false;
                    }
                    let r = ui.button(tl!("Cancel"));
                    app.auto.add_response("brandKits.approveCancel", &r, tl!("Cancel"));
                    keep &= !r.clicked();
                });
            });
            if keep {
                app.ui.brand_kits.approve = Some((who, reference, confirm));
            }
        }
    }
    // rename / archive
    ui.horizontal(|ui| {
        if let Some(mut n) = app.ui.brand_kits.rename_kit.take() {
            let r = ui.text_edit_singleline(&mut n);
            app.auto.add_response("brandKits.renameKitField", &r, tl!("Kit name"));
            let ok = ui.button(tl!("Rename"));
            app.auto.add_response("brandKits.renameKitApply", &ok, tl!("Rename"));
            if ok.clicked() {
                if run(app, "brand.kits.update", json!({"id": id, "name": n, "by": "user"})).is_some() {
                    app.ui.brand_kits.kit_json = None;
                    app.ui.brand_kits.clients = None;
                }
            } else if !ui.button("✕").clicked() {
                app.ui.brand_kits.rename_kit = Some(n);
            }
        } else {
            let r = ui.button(tl!("Rename Kit…"));
            app.auto.add_response("brandKits.renameKit", &r, tl!("Rename kit"));
            if r.clicked() {
                app.ui.brand_kits.rename_kit = Some(kit["name"].as_str().unwrap_or("").to_string());
            }
        }
        let archived = kit["archived"].as_bool().unwrap_or(false);
        let r = ui.button(if archived { tl!("Restore Kit") } else { tl!("Archive Kit") });
        app.auto.add_response("brandKits.archiveKit", &r, tl!("Archive kit"));
        if r.clicked() && run(app, "brand.kits.update", json!({"id": id, "archived": !archived, "by": "user"})).is_some() {
            app.ui.brand_kits.kit_json = None;
            app.ui.brand_kits.clients = None;
        }
    });
    // project pin
    ui.separator();
    ui.strong(tl!("Current project"));
    let pin = app.ui.brand_kits.pin.clone().unwrap_or(Value::Null);
    if pin["pinned"] == json!(true) {
        let r = ui.label(tlf!(
            "Pinned to {kit} v{v} ({client}) · {state}",
            kit = pin["pin"]["kitName"].as_str().unwrap_or(""),
            v = pin["pin"]["version"],
            client = pin["pin"]["clientName"].as_str().unwrap_or(""),
            state = pin["state"].as_str().unwrap_or("")
        ));
        app.auto.add_response("brandKits.pinState", &r, "pinned");
        if let Some(n) = pin["newerApprovedVersion"].as_u64() {
            ui.small(tlf!("Approved version {n} is newer; the project keeps its pinned version until you move it.", n = n));
        }
    } else {
        let r = ui.label(tl!("The project has no brand kit."));
        app.auto.add_response("brandKits.pinState", &r, "none");
    }
    let approved = kit["approvedVersion"].as_u64();
    let target = if v["status"] == json!("approved") { Some(u64::from(shown)) } else { approved };
    if let Some(t) = target {
        let pinned_elsewhere = pin["pinned"] == json!(true) && (pin["pin"]["kit"] != kit["id"] || pin["pin"]["version"].as_u64() != Some(t));
        let other_client = pin["pinned"] == json!(true) && pin["pin"]["client"] != kit["client"];
        if pinned_elsewhere {
            let r = ui.checkbox(&mut app.ui.brand_kits.pin_authorize, tl!("I authorise changing this project's branding"));
            app.auto.add_response("brandKits.pinAuthorize", &r, tl!("Authorise branding change"));
        }
        if other_client {
            let r = ui.checkbox(&mut app.ui.brand_kits.pin_replace_client, tl!("This project changes client"));
            app.auto.add_response("brandKits.pinReplaceClient", &r, tl!("Project changes client"));
        }
        let r = ui.button(tlf!("Pin v{v} to the Current Project", v = t));
        app.auto.add_response("brandKits.pin", &r, tl!("Pin to project"));
        if r.clicked() {
            let p = json!({"kit": format!("{id}@{t}"), "by": "user", "authorizeUpdate": app.ui.brand_kits.pin_authorize, "replaceClient": app.ui.brand_kits.pin_replace_client});
            if run(app, "brand.project.pin", p).is_some() {
                app.ui.brand_kits.notice = tlf!("The project uses {id}@{v}", id = id, v = t);
                app.ui.brand_kits.pin = None;
                app.ui.brand_kits.pin_authorize = false;
                app.ui.brand_kits.pin_replace_client = false;
            }
        }
    } else {
        ui.small(tl!("Approve a version to pin it to a project."));
    }
}

fn fonts_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, c: &mut KitContent, editable: bool) {
    ui.strong(tl!("Fonts (exact Font Library faces)"));
    let check = app.ui.brand_kits.check.clone().unwrap_or(Value::Null);
    let mut remove = None;
    for (i, f) in c.fonts.iter().enumerate() {
        ui.horizontal(|ui| {
            let r = ui.label(format!("{} {} · weight {}", f.family, f.style, f.weight));
            app.auto.add_response(&format!("brandKits.font.{i}"), &r, &format!("{} {}", f.family, f.style));
            ui.small(egui::RichText::new(format!("{} · {}…", f.library_font, f.sha256.get(..12).unwrap_or(""))).monospace());
            let problem = check["issues"]
                .as_array()
                .and_then(|a| {
                    a.iter().find(|x| x["area"] == json!("font") && x["detail"].as_str().is_some_and(|d| d.starts_with(&format!("{} {}", f.family, f.style))))
                })
                .cloned();
            match problem {
                Some(p) => {
                    ui.colored_label(ui.visuals().error_fg_color, p["detail"].as_str().unwrap_or(""));
                }
                None => match filmcraft_text::fonts::status(&f.family, &f.style) {
                    filmcraft_text::fonts::FontStatus::Exact { .. } => {
                        ui.small(tl!("installed, exact"));
                    }
                    _ => {
                        ui.colored_label(ui.visuals().error_fg_color, tl!("missing"));
                    }
                },
            }
            if editable && ui.small_button(tl!("Remove")).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        c.fonts.remove(i);
    }
    if !editable {
        return;
    }
    // every face of the Font Library, ticked when the kit uses it (a list, not a dropdown: a
    // brand usually needs several exact weights of a family)
    let fonts = app.ui.brand_kits.fonts.clone().unwrap_or(Value::Null);
    ui.separator();
    ui.label(tl!("Font Library faces (tick the ones this kit uses)"));
    for lf in fonts["fonts"].as_array().cloned().unwrap_or_default() {
        ui.horizontal_wrapped(|ui| {
            ui.strong(lf["families"].as_array().and_then(|a| a.first()).and_then(Value::as_str).unwrap_or(""));
            for face in lf["faces"].as_array().cloned().unwrap_or_default() {
                let (fam, sty) = (face["family"].as_str().unwrap_or(""), face["style"].as_str().unwrap_or(""));
                let mut on = c.fonts.iter().any(|f| f.family == fam && f.style == sty);
                let r = ui.checkbox(&mut on, format!("{sty} {}", face["weight"]));
                app.auto.add_response(&format!("brandKits.addFont.{fam} {sty}"), &r, &format!("{fam} {sty}"));
                if r.changed() {
                    if on {
                        c.fonts.push(KitFont {
                            family: fam.into(),
                            style: sty.into(),
                            weight: face["weight"].as_u64().unwrap_or(400) as u16,
                            library_font: lf["id"].as_str().unwrap_or("").into(),
                            sha256: lf["sha256"].as_str().unwrap_or("").into(),
                        });
                    } else {
                        c.fonts.retain(|f| !(f.family == fam && f.style == sty));
                    }
                }
            }
        });
    }
    ui.small(tl!("Fonts are added in Window ▸ Font Library; a kit references the exact file and weight."));
}

fn colors_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, c: &mut KitContent, editable: bool) {
    ui.strong(tl!("Palettes (exact versions)"));
    let mut remove = None;
    for (i, p) in c.palettes.iter().enumerate() {
        let info = app.ui.brand_kits.palette_cache.get(&format!("{}@{}", p.palette, p.version)).cloned().unwrap_or_default();
        ui.horizontal(|ui| {
            let r = ui.label(format!("{} · version {}", if info.name.is_empty() { &p.name } else { &info.name }, p.version));
            app.auto.add_response(&format!("brandKits.palette.{i}"), &r, &p.palette);
            ui.small(egui::RichText::new(format!("{}@{}", p.palette, p.version)).monospace());
            for (_, _, h) in info.colors.iter().take(24) {
                swatch(ui, Some(*h), egui::vec2(14.0, 14.0));
            }
            if editable && ui.small_button(tl!("Remove")).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        c.palettes.remove(i);
    }
    if editable {
        let pals = app.ui.brand_kits.palettes.clone().unwrap_or(Value::Null);
        let (mut pid, mut ver) = app.ui.brand_kits.add_palette.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            let name = pals["palettes"]
                .as_array()
                .and_then(|a| a.iter().find(|p| p["id"].as_str() == Some(pid.as_str())))
                .and_then(|p| p["name"].as_str())
                .unwrap_or(tl!("Choose a palette…"))
                .to_string();
            let r = craft_ui::ComboBox::from_id_salt("brand-add-palette").selected_text(name).width(220.0).show_ui(ui, |ui| {
                for p in pals["palettes"].as_array().cloned().unwrap_or_default() {
                    let n = p["name"].as_str().unwrap_or("");
                    let resp = ui.selectable_label(p["id"].as_str() == Some(pid.as_str()), n);
                    app.auto.add_response(&format!("brandKits.addPalette.{n}"), &resp, n);
                    if resp.clicked() {
                        pid = p["id"].as_str().unwrap_or("").to_string();
                        ver = p["latestVersion"].as_u64().unwrap_or(1) as u32;
                    }
                }
            });
            app.auto.add_response("brandKits.addPalette", &r.response, tl!("Palette"));
            let max = pals["palettes"]
                .as_array()
                .and_then(|a| a.iter().find(|p| p["id"].as_str() == Some(pid.as_str())))
                .and_then(|p| p["latestVersion"].as_u64())
                .unwrap_or(0) as u32;
            let r = craft_ui::ComboBox::from_id_salt("brand-add-palette-version").selected_text(tlf!("version {v}", v = ver)).show_ui(ui, |ui| {
                for v in (1..=max).rev() {
                    let resp = ui.selectable_label(v == ver, tlf!("version {v}", v = v));
                    app.auto.add_response(&format!("brandKits.addPaletteVersion.{v}"), &resp, &v.to_string());
                    if resp.clicked() {
                        ver = v;
                    }
                }
            });
            app.auto.add_response("brandKits.addPaletteVersion", &r.response, tl!("Palette version"));
            let r = ui.add_enabled(!pid.is_empty() && ver > 0, egui::Button::new(tl!("Add Palette Version")));
            app.auto.add_response("brandKits.addPaletteApply", &r, tl!("Add palette version"));
            if r.clicked() {
                if let Ok(v) = app.session.execute("palettes.get", json!({"id": pid, "version": ver})) {
                    c.palettes.retain(|p| p.palette != pid);
                    c.palettes.push(KitPalette {
                        palette: pid.clone(),
                        version: ver,
                        sha256: v["version"]["sha256"].as_str().unwrap_or("").into(),
                        name: v["name"].as_str().unwrap_or("").into(),
                    });
                }
                app.ui.brand_kits.add_palette = None;
            } else {
                app.ui.brand_kits.add_palette = Some((pid.clone(), ver));
            }
        });
        ui.small(tl!("A kit keeps the version you choose: later edits to the palette never change it."));
    }
    ui.separator();
    ui.strong(tl!("Colour roles"));
    let opts = color_options(&app.ui.brand_kits, c);
    let mut remove = None;
    for (i, r) in c.colors.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            swatch(ui, color_hex(&opts, &Some(r.color.clone())), egui::vec2(18.0, 18.0));
            if editable {
                let resp = ui.add(egui::TextEdit::singleline(&mut r.role).desired_width(140.0));
                app.auto.add_response(&format!("brandKits.colorRole.{i}.name"), &resp, tl!("Role"));
                let mut v = Some(r.color.clone());
                color_combo(ui, &mut app.auto, &format!("brandKits.colorRole.{i}.color"), &opts, &mut v, false);
                if let Some(v) = v {
                    r.color = v;
                }
                let resp = ui.add(egui::TextEdit::singleline(&mut r.notes).hint_text(tl!("notes")).desired_width(200.0));
                app.auto.add_response(&format!("brandKits.colorRole.{i}.notes"), &resp, tl!("Notes"));
                if ui.small_button(tl!("Remove")).clicked() {
                    remove = Some(i);
                }
            } else {
                ui.label(format!("{} · {}", r.role, color_label(&opts, &Some(r.color.clone()))));
                if !r.notes.is_empty() {
                    ui.small(&r.notes);
                }
            }
        });
    }
    if let Some(i) = remove {
        c.colors.remove(i);
    }
    if editable && !opts.is_empty() {
        let r = ui.button(tl!("Add Colour Role"));
        app.auto.add_response("brandKits.addColorRole", &r, tl!("Add colour role"));
        if r.clicked()
            && let Some((_, first, _)) = opts.first()
        {
            c.colors.push(ColorRole { role: format!("role {}", c.colors.len() + 1), color: first.clone(), notes: String::new() });
        }
    }
}

fn frame_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, c: &mut KitContent, editable: bool) {
    ui.add_enabled_ui(editable, |ui| {
        ui.horizontal(|ui| {
            ui.strong(tl!("Reference frame"));
            let [mut w, mut h] = c.reference_frame.unwrap_or([1080, 1920]);
            let mut on = c.reference_frame.is_some();
            let r = ui.checkbox(&mut on, "");
            app.auto.add_response("brandKits.frame.on", &r, tl!("Reference frame"));
            if on {
                let r = ui.add(egui::DragValue::new(&mut w).range(16..=8192));
                app.auto.add_response("brandKits.frame.w", &r, tl!("Width"));
                ui.label("×");
                let r = ui.add(egui::DragValue::new(&mut h).range(16..=8192));
                app.auto.add_response("brandKits.frame.h", &r, tl!("Height"));
                for (label, key, v) in [("9:16 1080×1920", "portrait", [1080, 1920]), ("16:9 1920×1080", "landscape", [1920, 1080])] {
                    let r = ui.small_button(label);
                    app.auto.add_response(&format!("brandKits.frame.{key}"), &r, label);
                    if r.clicked() {
                        [w, h] = v;
                    }
                }
                c.reference_frame = Some([w, h]);
            } else {
                c.reference_frame = None;
            }
            ui.small(tl!("(the frame \"px\" sizes refer to)"));
        });
        ui.horizontal(|ui| {
            ui.strong(tl!("Safe area"));
            let mut on = c.safe_area.is_some();
            let r = ui.checkbox(&mut on, "");
            app.auto.add_response("brandKits.safe.on", &r, tl!("Safe area"));
            if !on {
                c.safe_area = None;
                return;
            }
            let sa =
                c.safe_area.get_or_insert_with(|| filmcraft_engine::brand_kits::SafeArea { top: 5.0, right: 5.0, bottom: 5.0, left: 5.0, unit: "%w".into() });
            for (label, key, v) in [
                (tl!("top"), "top", &mut sa.top),
                (tl!("right"), "right", &mut sa.right),
                (tl!("bottom"), "bottom", &mut sa.bottom),
                (tl!("left"), "left", &mut sa.left),
            ] {
                ui.label(label);
                let r = ui.add(egui::DragValue::new(v).range(0.0..=50.0).speed(0.1));
                app.auto.add_response(&format!("brandKits.safe.{key}"), &r, label);
            }
            for u in ["%w", "%h"] {
                let r = ui.selectable_label(sa.unit == u, u);
                app.auto.add_response(&format!("brandKits.safe.unit.{u}"), &r, u);
                if r.clicked() {
                    sa.unit = u.into();
                }
            }
        });
    });
    ui.separator();
}

fn typography_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, c: &mut KitContent, editable: bool) {
    frame_ui(app, ui, c, editable);
    let opts = color_options(&app.ui.brand_kits, c);
    ui.horizontal_wrapped(|ui| {
        for (i, t) in c.typography.iter().enumerate() {
            let r = ui.selectable_label(app.ui.brand_kits.role_sel == i, t.role.as_str());
            app.auto.add_response(&format!("brandKits.role.{}", t.role), &r, &t.role);
            if r.clicked() {
                app.ui.brand_kits.role_sel = i;
            }
        }
    });
    if editable {
        ui.horizontal(|ui| {
            let r = craft_ui::ComboBox::from_id_salt("brand-new-role")
                .selected_text(if app.ui.brand_kits.new_role.is_empty() { tl!("Role…") } else { app.ui.brand_kits.new_role.as_str() })
                .show_ui(ui, |ui| {
                    for r in STANDARD_TYPE_ROLES {
                        let resp = ui.selectable_label(false, *r);
                        app.auto.add_response(&format!("brandKits.newRole.{r}"), &resp, r);
                        if resp.clicked() {
                            app.ui.brand_kits.new_role = (*r).to_string();
                        }
                    }
                });
            app.auto.add_response("brandKits.newRole", &r.response, tl!("Role"));
            let r = ui.add(egui::TextEdit::singleline(&mut app.ui.brand_kits.new_role).hint_text(tl!("or a name of your own")).desired_width(150.0));
            app.auto.add_response("brandKits.newRoleName", &r, tl!("Role name"));
            let name = app.ui.brand_kits.new_role.trim().to_string();
            let ok = !name.is_empty() && !c.typography.iter().any(|t| t.role.eq_ignore_ascii_case(&name)) && !c.fonts.is_empty();
            let r = ui.add_enabled(ok, egui::Button::new(tl!("Add Role")));
            app.auto.add_response("brandKits.addRole", &r, tl!("Add typography role"));
            if r.clicked()
                && let Some(f) = c.fonts.first()
            {
                c.typography.push(TypeRole {
                    role: name,
                    family: f.family.clone(),
                    style: f.style.clone(),
                    weight: f.weight,
                    size: Size { value: 48.0, unit: "px".into(), min_px: None, max_px: None },
                    color: None,
                    background: None,
                    accent: None,
                    align: "left".into(),
                    placement: String::new(),
                    case: "none".into(),
                    tracking: 0.0,
                    line_height: None,
                    max_lines: None,
                    opacity: None,
                    styling: String::new(),
                });
                app.ui.brand_kits.role_sel = c.typography.len() - 1;
                app.ui.brand_kits.new_role.clear();
            }
            if c.fonts.is_empty() {
                ui.small(tl!("Add the kit's fonts first."));
            }
        });
    }
    ui.separator();
    let sel = app.ui.brand_kits.role_sel;
    let fonts = c.fonts.clone();
    let mut remove = false;
    let Some(t) = c.typography.get_mut(sel) else {
        ui.label(tl!("No typography roles yet."));
        return;
    };
    ui.add_enabled_ui(editable, |ui| {
        egui::Grid::new("brand-type").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label(tl!("Role"));
            let r = ui.text_edit_singleline(&mut t.role);
            app.auto.add_response("brandKits.type.role", &r, tl!("Role"));
            ui.end_row();
            ui.label(tl!("Font"));
            let r = craft_ui::ComboBox::from_id_salt("brand-type-font").selected_text(format!("{} {} ({})", t.family, t.style, t.weight)).width(260.0).show_ui(
                ui,
                |ui| {
                    for f in &fonts {
                        let label = format!("{} {} ({})", f.family, f.style, f.weight);
                        let resp = ui.selectable_label(t.family == f.family && t.style == f.style, &label);
                        app.auto.add_response(&format!("brandKits.type.font.{} {}", f.family, f.style), &resp, &label);
                        if resp.clicked() {
                            t.family.clone_from(&f.family);
                            t.style.clone_from(&f.style);
                            t.weight = f.weight;
                        }
                    }
                },
            );
            app.auto.add_response("brandKits.type.font", &r.response, tl!("Font"));
            ui.end_row();
            ui.label(tl!("Size"));
            ui.horizontal(|ui| {
                let r = ui.add(egui::DragValue::new(&mut t.size.value).range(0.1..=2000.0).speed(0.1));
                app.auto.add_response("brandKits.type.size", &r, tl!("Size"));
                let r = craft_ui::ComboBox::from_id_salt("brand-type-unit").selected_text(t.size.unit.as_str()).show_ui(ui, |ui| {
                    for u in ["px", "%w", "%h"] {
                        let resp = ui.selectable_label(t.size.unit == u, u);
                        app.auto.add_response(&format!("brandKits.type.unit.{u}"), &resp, u);
                        if resp.clicked() {
                            t.size.unit = u.into();
                        }
                    }
                });
                app.auto.add_response("brandKits.type.unit", &r.response, tl!("Unit"));
                for (label, key, slot) in [(tl!("min px"), "min", &mut t.size.min_px), (tl!("max px"), "max", &mut t.size.max_px)] {
                    let mut on = slot.is_some();
                    let r = ui.checkbox(&mut on, label);
                    app.auto.add_response(&format!("brandKits.type.{key}On"), &r, label);
                    if on {
                        let v = slot.get_or_insert(24.0);
                        let r = ui.add(egui::DragValue::new(v).range(1.0..=2000.0));
                        app.auto.add_response(&format!("brandKits.type.{key}"), &r, label);
                    } else {
                        *slot = None;
                    }
                }
            });
            ui.end_row();
            for (label, key, slot, none) in [
                (tl!("Colour"), "color", &mut t.color, true),
                (tl!("Background"), "background", &mut t.background, true),
                (tl!("Accent"), "accent", &mut t.accent, true),
            ] {
                ui.label(label);
                ui.horizontal(|ui| {
                    swatch(ui, color_hex(&opts, slot), egui::vec2(18.0, 18.0));
                    color_combo(ui, &mut app.auto, &format!("brandKits.type.{key}"), &opts, slot, none);
                });
                ui.end_row();
            }
            ui.label(tl!("Alignment"));
            ui.horizontal(|ui| {
                for a in ["left", "center", "right"] {
                    let r = ui.selectable_label(t.align == a, a);
                    app.auto.add_response(&format!("brandKits.type.align.{a}"), &r, a);
                    if r.clicked() {
                        t.align = a.into();
                    }
                }
            });
            ui.end_row();
            ui.label(tl!("Placement"));
            ui.horizontal(|ui| {
                let r = craft_ui::ComboBox::from_id_salt("brand-type-place")
                    .selected_text(if t.placement.is_empty() { tl!("not set") } else { t.placement.as_str() })
                    .show_ui(ui, |ui| {
                        for a in ANCHORS {
                            let resp = ui.selectable_label(t.placement == *a, *a);
                            app.auto.add_response(&format!("brandKits.type.place.{a}"), &resp, a);
                            if resp.clicked() {
                                t.placement = (*a).into();
                            }
                        }
                    });
                app.auto.add_response("brandKits.type.place", &r.response, tl!("Placement"));
            });
            ui.end_row();
            ui.label(tl!("Case"));
            ui.horizontal(|ui| {
                for k in ["none", "upper", "lower", "title"] {
                    let r = ui.selectable_label(t.case == k, k);
                    app.auto.add_response(&format!("brandKits.type.case.{k}"), &r, k);
                    if r.clicked() {
                        t.case = k.into();
                    }
                }
            });
            ui.end_row();
            ui.label(tl!("Tracking (em)"));
            let r = ui.add(egui::DragValue::new(&mut t.tracking).range(-0.5..=2.0).speed(0.005).fixed_decimals(3));
            app.auto.add_response("brandKits.type.tracking", &r, tl!("Tracking"));
            ui.end_row();
            ui.label(tl!("Line height"));
            ui.horizontal(|ui| {
                let mut on = t.line_height.is_some();
                let r = ui.checkbox(&mut on, "");
                app.auto.add_response("brandKits.type.lineHeightOn", &r, tl!("Line height"));
                if on {
                    let v = t.line_height.get_or_insert(1.2);
                    let r = ui.add(egui::DragValue::new(v).range(0.5..=4.0).speed(0.01));
                    app.auto.add_response("brandKits.type.lineHeight", &r, tl!("Line height"));
                } else {
                    t.line_height = None;
                }
                ui.label(tl!("Max lines"));
                let mut on = t.max_lines.is_some();
                let r = ui.checkbox(&mut on, "");
                app.auto.add_response("brandKits.type.maxLinesOn", &r, tl!("Max lines"));
                if on {
                    let v = t.max_lines.get_or_insert(2);
                    ui.add(egui::DragValue::new(v).range(1..=20));
                } else {
                    t.max_lines = None;
                }
                ui.label(tl!("Opacity"));
                let mut on = t.opacity.is_some();
                ui.checkbox(&mut on, "");
                if on {
                    let v = t.opacity.get_or_insert(1.0);
                    ui.add(egui::DragValue::new(v).range(0.0..=1.0).speed(0.01));
                } else {
                    t.opacity = None;
                }
            });
            ui.end_row();
            ui.label(tl!("Styling"));
            let r = ui.add(egui::TextEdit::singleline(&mut t.styling).hint_text(tl!("e.g. pill background, radius 999px")).desired_width(360.0));
            app.auto.add_response("brandKits.type.styling", &r, tl!("Styling"));
            ui.end_row();
        });
        if editable {
            let r = ui.button(tl!("Remove Role"));
            app.auto.add_response("brandKits.type.remove", &r, tl!("Remove role"));
            remove = r.clicked();
        }
    });
    if remove {
        c.typography.remove(sel);
        app.ui.brand_kits.role_sel = 0;
    }
}

fn texture(app: &mut FilmcraftApp, ctx: &egui::Context, key: String, make: impl FnOnce() -> Option<(u32, u32, Vec<u8>)>) -> Option<egui::TextureHandle> {
    if let Some(t) = app.ui.brand_kits.textures.get(&key) {
        return Some(t.clone());
    }
    if app.ui.brand_kits.textures.len() > 300 {
        app.ui.brand_kits.textures.clear();
    }
    let (w, h, rgba) = make()?;
    let t = ctx.load_texture(format!("brand-{key}"), egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba), egui::TextureOptions::LINEAR);
    app.ui.brand_kits.textures.insert(key, t.clone());
    Some(t)
}

fn asset_texture(app: &mut FilmcraftApp, ctx: &egui::Context, a: &Value) -> Option<egui::TextureHandle> {
    let id = a["id"].as_str()?.to_string();
    let parsed: filmcraft_engine::brand_kits::BrandAsset = serde_json::from_value(a.clone()).ok()?;
    let dirs = filmcraft_engine::brand_kits::Dirs::of(&app.session)?;
    texture(app, ctx, format!("asset-{id}"), || filmcraft_engine::brand_kits::asset_rgba(&dirs, &parsed, 512))
}

pub fn queue_asset(app: &mut FilmcraftApp, paths: Vec<String>) {
    if let Some(path) = paths.into_iter().next() {
        let name = std::path::Path::new(&path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        app.ui.brand_kits.pending_asset = Some(PendingAsset { path, name, kind: "logo".into(), ..Default::default() });
        app.ui.brand_kits.tab = Tab::Logos;
    }
}

fn logos_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, c: &mut KitContent, editable: bool) {
    let Some(client) = app.ui.brand_kits.kit_json.as_ref().and_then(|k| k["client"].as_str()).map(str::to_string) else { return };
    ui.horizontal(|ui| {
        ui.strong(tl!("Client logos and brand graphics"));
        let r = ui.button(tl!("Import Logo…"));
        app.auto.add_response("brandKits.importLogo", &r, tl!("Import logo"));
        if r.clicked() {
            app.pick_ui(FileDialog::open_files(&["png", "jpg", "jpeg", "webp"]), queue_asset);
        }
    });
    if let Some(mut pa) = app.ui.brand_kits.pending_asset.take() {
        let mut keep = true;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(&pa.path);
            egui::Grid::new("brand-asset-new").num_columns(2).show(ui, |ui| {
                for (label, key, v) in [
                    (tl!("Name"), "name", &mut pa.name),
                    (tl!("Variant"), "variant", &mut pa.variant),
                    (tl!("Source"), "source", &mut pa.source),
                    (tl!("Licence / rights"), "license", &mut pa.license),
                ] {
                    ui.label(label);
                    let r = ui.text_edit_singleline(v);
                    app.auto.add_response(&format!("brandKits.asset.{key}"), &r, label);
                    ui.end_row();
                }
                ui.label(tl!("Kind"));
                ui.horizontal(|ui| {
                    for k in ["logo", "graphic"] {
                        let r = ui.selectable_label(pa.kind == k, k);
                        app.auto.add_response(&format!("brandKits.asset.kind.{k}"), &r, k);
                        if r.clicked() {
                            pa.kind = k.into();
                        }
                    }
                });
                ui.end_row();
            });
            ui.horizontal(|ui| {
                let r = ui.button(tl!("Import"));
                app.auto.add_response("brandKits.asset.import", &r, tl!("Import asset"));
                if r.clicked()
                    && let Some(v) = run(
                        app,
                        "brand.assets.import",
                        json!({"client": client, "path": pa.path, "name": pa.name, "kind": pa.kind, "variant": pa.variant, "source": pa.source, "license": pa.license, "by": "user"}),
                    )
                {
                    app.ui.brand_kits.notice = tlf!("Imported {n} ({w}×{h} {f})", n = v["name"].as_str().unwrap_or(""), w = v["width"], h = v["height"], f = v["format"].as_str().unwrap_or(""));
                    app.ui.brand_kits.assets = None;
                    keep = false;
                }
                let r = ui.button(tl!("Cancel"));
                app.auto.add_response("brandKits.asset.cancel", &r, tl!("Cancel"));
                keep &= !r.clicked();
            });
        });
        if keep {
            app.ui.brand_kits.pending_asset = Some(pa);
        }
    }
    let assets = app.ui.brand_kits.assets.clone().unwrap_or(Value::Null);
    let list = assets["assets"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        ui.small(tl!("No logos yet (PNG with transparency, JPEG or WebP)."));
    }
    ui.horizontal_wrapped(|ui| {
        for a in &list {
            let aid = a["id"].as_str().unwrap_or("").to_string();
            ui.vertical(|ui| {
                ui.set_width(170.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(160.0, 70.0), egui::Sense::hover());
                // logos are shown on the kit's first colour role (a white logo needs its background)
                let bg = color_options(&app.ui.brand_kits, c)
                    .into_iter()
                    .find(|(_, r, _)| c.colors.first().is_some_and(|cr| &cr.color == r))
                    .map(|(_, _, h)| h)
                    .unwrap_or(HexColor([60, 60, 60]));
                ui.painter().rect_filled(rect, 3.0, egui::Color32::from_rgb(bg.0[0], bg.0[1], bg.0[2]));
                if let Some(t) = asset_texture(app, ui.ctx(), a) {
                    let s = t.size_vec2();
                    let k = (150.0 / s.x).min(60.0 / s.y).min(1.0);
                    let r = egui::Rect::from_center_size(rect.center(), s * k);
                    ui.painter().image(t.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                }
                let r = ui.label(egui::RichText::new(a["name"].as_str().unwrap_or("")).strong());
                app.auto.add_response(&format!("brandKits.asset.{aid}"), &r, a["name"].as_str().unwrap_or(""));
                ui.small(format!("{} · {}×{} · {}", a["format"].as_str().unwrap_or(""), a["width"], a["height"], a["variant"].as_str().unwrap_or("")));
                let used = a["usedBy"].as_array().map(Vec::len).unwrap_or(0);
                if used > 0 {
                    ui.small(tlf!("used by {n} kit version(s)", n = used));
                }
                ui.horizontal(|ui| {
                    if editable && !c.logos.iter().any(|l| l.asset == aid) {
                        let r = ui.small_button(tl!("Use in Kit"));
                        app.auto.add_response(&format!("brandKits.asset.{aid}.use"), &r, tl!("Use in kit"));
                        if r.clicked() {
                            c.logos.push(LogoUse {
                                asset: aid.clone(),
                                sha256: a["sha256"].as_str().unwrap_or("").into(),
                                variant: a["variant"].as_str().unwrap_or("").into(),
                                approved: true,
                                placement: String::new(),
                                margin_pct: None,
                                min_width: None,
                                max_width: None,
                                backgrounds: Vec::new(),
                                forbidden: Vec::new(),
                                notes: String::new(),
                            });
                            app.ui.brand_kits.logo_sel = c.logos.len() - 1;
                        }
                    }
                    let r = ui.small_button(tl!("Delete"));
                    app.auto.add_response(&format!("brandKits.asset.{aid}.delete"), &r, tl!("Delete asset"));
                    if r.clicked() && run(app, "brand.assets.remove", json!({"id": aid})).is_some() {
                        app.ui.brand_kits.assets = None;
                    }
                });
            });
        }
    });
    ui.separator();
    ui.strong(tl!("Logo usage in this kit"));
    let opts = color_options(&app.ui.brand_kits, c);
    ui.horizontal_wrapped(|ui| {
        for (i, l) in c.logos.iter().enumerate() {
            let name = list.iter().find(|a| a["id"].as_str() == Some(l.asset.as_str())).and_then(|a| a["name"].as_str()).unwrap_or(&l.asset).to_string();
            let r = ui.selectable_label(app.ui.brand_kits.logo_sel == i, format!("{name} ({})", if l.variant.is_empty() { "-" } else { &l.variant }));
            app.auto.add_response(&format!("brandKits.logoUse.{i}"), &r, &name);
            if r.clicked() {
                app.ui.brand_kits.logo_sel = i;
            }
        }
    });
    let sel = app.ui.brand_kits.logo_sel;
    let mut remove = false;
    let Some(l) = c.logos.get_mut(sel) else { return };
    ui.add_enabled_ui(editable, |ui| {
        egui::Grid::new("brand-logo-use").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label(tl!("Variant"));
            let r = ui.text_edit_singleline(&mut l.variant);
            app.auto.add_response("brandKits.logo.variant", &r, tl!("Variant"));
            ui.end_row();
            ui.label(tl!("Approved variant"));
            let r = ui.checkbox(&mut l.approved, tl!("may be used"));
            app.auto.add_response("brandKits.logo.approved", &r, tl!("Approved variant"));
            ui.end_row();
            ui.label(tl!("Placement"));
            let r = craft_ui::ComboBox::from_id_salt("brand-logo-place")
                .selected_text(if l.placement.is_empty() { tl!("not set") } else { l.placement.as_str() })
                .show_ui(ui, |ui| {
                    for a in ANCHORS {
                        let resp = ui.selectable_label(l.placement == *a, *a);
                        app.auto.add_response(&format!("brandKits.logo.place.{a}"), &resp, a);
                        if resp.clicked() {
                            l.placement = (*a).into();
                        }
                    }
                });
            app.auto.add_response("brandKits.logo.place", &r.response, tl!("Placement"));
            ui.end_row();
            ui.label(tl!("Clear space (% of width)"));
            ui.horizontal(|ui| {
                let mut on = l.margin_pct.is_some();
                let r = ui.checkbox(&mut on, "");
                app.auto.add_response("brandKits.logo.marginOn", &r, tl!("Clear space"));
                if on {
                    let v = l.margin_pct.get_or_insert(5.0);
                    let r = ui.add(egui::DragValue::new(v).range(0.0..=50.0).speed(0.1));
                    app.auto.add_response("brandKits.logo.margin", &r, tl!("Clear space"));
                } else {
                    l.margin_pct = None;
                }
            });
            ui.end_row();
            for (label, key, slot) in [(tl!("Minimum width"), "min", &mut l.min_width), (tl!("Maximum width"), "max", &mut l.max_width)] {
                ui.label(label);
                ui.horizontal(|ui| {
                    let mut on = slot.is_some();
                    let r = ui.checkbox(&mut on, "");
                    app.auto.add_response(&format!("brandKits.logo.{key}On"), &r, label);
                    if on {
                        let s = slot.get_or_insert(Size { value: 100.0, unit: "px".into(), min_px: None, max_px: None });
                        let r = ui.add(egui::DragValue::new(&mut s.value).range(0.1..=4000.0));
                        app.auto.add_response(&format!("brandKits.logo.{key}"), &r, label);
                        for u in ["px", "%w", "%h"] {
                            if ui.selectable_label(s.unit == u, u).clicked() {
                                s.unit = u.into();
                            }
                        }
                    } else {
                        *slot = None;
                    }
                });
                ui.end_row();
            }
            ui.label(tl!("Allowed backgrounds"));
            ui.horizontal_wrapped(|ui| {
                for (label, r, hex) in &opts {
                    let mut on = l.backgrounds.contains(r);
                    swatch(ui, Some(*hex), egui::vec2(12.0, 12.0));
                    let resp = ui.checkbox(&mut on, label.split(" (").next().unwrap_or(label));
                    app.auto.add_response(&format!("brandKits.logo.bg.{}", label.split(" (").next().unwrap_or(label)), &resp, label);
                    if resp.changed() {
                        if on {
                            l.backgrounds.push(r.clone());
                        } else {
                            l.backgrounds.retain(|x| x != r);
                        }
                    }
                }
            });
            ui.end_row();
            ui.label(tl!("Not allowed"));
            ui.horizontal_wrapped(|ui| {
                for a in ALTERATIONS {
                    let mut on = l.forbidden.iter().any(|f| f == a);
                    let resp = ui.checkbox(&mut on, *a);
                    app.auto.add_response(&format!("brandKits.logo.forbid.{a}"), &resp, a);
                    if resp.changed() {
                        if on {
                            l.forbidden.push((*a).into());
                        } else {
                            l.forbidden.retain(|f| f != a);
                        }
                    }
                }
            });
            ui.end_row();
            ui.label(tl!("Notes"));
            let r = ui.add(egui::TextEdit::singleline(&mut l.notes).desired_width(360.0));
            app.auto.add_response("brandKits.logo.notes", &r, tl!("Notes"));
            ui.end_row();
        });
        if editable {
            let r = ui.button(tl!("Remove from Kit"));
            app.auto.add_response("brandKits.logo.remove", &r, tl!("Remove logo from kit"));
            remove = r.clicked();
        }
    });
    if remove {
        c.logos.remove(sel);
        app.ui.brand_kits.logo_sel = 0;
    }
}

fn rules_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, c: &mut KitContent, editable: bool) {
    let opts = color_options(&app.ui.brand_kits, c);
    ui.strong(tl!("The palettes' own rules (kept exactly; a kit cannot override them)"));
    for p in &c.palettes {
        if let Some(info) = app.ui.brand_kits.palette_cache.get(&format!("{}@{}", p.palette, p.version)) {
            for (i, r) in info.rules.iter().enumerate() {
                let resp = ui.label(format!("{} v{} · {r}", info.name, p.version));
                app.auto.add_response(&format!("brandKits.paletteRule.{i}"), &resp, r);
            }
            if info.rules.is_empty() {
                ui.small(tlf!("{n} v{v}: no rules", n = info.name, v = p.version));
            }
        }
    }
    ui.separator();
    ui.strong(tl!("Kit rules"));
    let mut remove = None;
    for (i, r) in c.rules.iter().enumerate() {
        ui.horizontal(|ui| {
            let text = if r.kind == "guideline" {
                format!("Guideline: {}", r.text)
            } else {
                format!("{}: {} on {} ({}) {}", r.kind, color_label(&opts, &r.foreground), color_label(&opts, &r.background), r.applies_to, r.text)
            };
            let resp = ui.label(text.as_str());
            app.auto.add_response(&format!("brandKits.rule.{i}"), &resp, &text);
            if editable && ui.small_button(tl!("Remove")).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        c.rules.remove(i);
    }
    if !editable {
        return;
    }
    let mut nr = app.ui.brand_kits.new_rule.take().unwrap_or(KitRule {
        id: String::new(),
        kind: "guideline".into(),
        foreground: None,
        background: None,
        applies_to: "any".into(),
        text: String::new(),
    });
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for k in ["guideline", "permitted", "prohibited"] {
                let r = ui.selectable_label(nr.kind == k, k);
                app.auto.add_response(&format!("brandKits.newRule.kind.{k}"), &r, k);
                if r.clicked() {
                    nr.kind = k.into();
                }
            }
            if nr.kind != "guideline" {
                color_combo(ui, &mut app.auto, "brandKits.newRule.fg", &opts, &mut nr.foreground, false);
                ui.label(tl!("on"));
                color_combo(ui, &mut app.auto, "brandKits.newRule.bg", &opts, &mut nr.background, false);
                for a in ["text", "graphics", "any"] {
                    if ui.selectable_label(nr.applies_to == a, a).clicked() {
                        nr.applies_to = a.into();
                    }
                }
            }
        });
        let r = ui.add(egui::TextEdit::singleline(&mut nr.text).hint_text(tl!("The rule as the brand states it")).desired_width(520.0));
        app.auto.add_response("brandKits.newRule.text", &r, tl!("Rule text"));
        let ready = if nr.kind == "guideline" {
            !nr.text.trim().is_empty()
        } else {
            nr.foreground.is_some() && nr.background.is_some() && nr.foreground != nr.background
        };
        let r = ui.add_enabled(ready, egui::Button::new(tl!("Add Rule")));
        app.auto.add_response("brandKits.addRule", &r, tl!("Add rule"));
        if r.clicked() {
            let mut rule = std::mem::replace(
                &mut nr,
                KitRule { id: String::new(), kind: "guideline".into(), foreground: None, background: None, applies_to: "any".into(), text: String::new() },
            );
            rule.id = filmcraft_engine::user_library::new_id("krule", "ui");
            if rule.kind == "guideline" {
                rule.foreground = None;
                rule.background = None;
            }
            c.rules.push(rule);
        }
    });
    app.ui.brand_kits.new_rule = Some(nr);
}

fn apply_case(s: &str, case: &str) -> String {
    match case {
        "upper" => s.to_uppercase(),
        "lower" => s.to_lowercase(),
        "title" => s
            .split(' ')
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => s.to_string(),
    }
}

fn anchor_pos(anchor: &str, frame: egui::Rect, inner: egui::Rect, size: egui::Vec2) -> egui::Pos2 {
    let x = match anchor {
        a if a.ends_with("left") || a == "lower-third" => inner.left(),
        a if a.ends_with("right") => inner.right() - size.x,
        _ => frame.center().x - size.x / 2.0,
    };
    let y = match anchor {
        a if a.starts_with("top") => inner.top(),
        a if a.starts_with("bottom") => inner.bottom() - size.y,
        "lower-third" => frame.top() + frame.height() * 0.66,
        _ => frame.center().y - size.y / 2.0,
    };
    egui::pos2(x, y)
}

fn preview_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, c: &KitContent) {
    let opts = color_options(&app.ui.brand_kits, c);
    ui.horizontal(|ui| {
        let r = ui.selectable_label(app.ui.brand_kits.preview_portrait, "9:16");
        app.auto.add_response("brandKits.preview.portrait", &r, "9:16");
        if r.clicked() {
            app.ui.brand_kits.preview_portrait = true;
        }
        let r = ui.selectable_label(!app.ui.brand_kits.preview_portrait, "16:9");
        app.auto.add_response("brandKits.preview.landscape", &r, "16:9");
        if r.clicked() {
            app.ui.brand_kits.preview_portrait = false;
        }
        ui.small(tl!("Drawn with the kit's fonts by FilmCraft's text engine; sample text is editable below."));
    });
    let (fw, fh) = if app.ui.brand_kits.preview_portrait { (1080.0, 1920.0) } else { (1920.0, 1080.0) };
    let ph = 560.0f32;
    let k = ph / fh;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(fw * k, ph), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let bg = c
        .colors
        .iter()
        .find(|r| r.role.to_lowercase().contains("background"))
        .or(c.colors.first())
        .and_then(|r| color_hex(&opts, &Some(r.color.clone())))
        .unwrap_or(HexColor([40, 40, 40]));
    p.rect_filled(rect, 0.0, egui::Color32::from_rgb(bg.0[0], bg.0[1], bg.0[2]));
    // safe area
    let inner = c.safe_area.as_ref().map_or(rect.shrink(fw * k * 0.05), |s| {
        let base = if s.unit == "%h" { fh } else { fw } * k / 100.0;
        egui::Rect::from_min_max(
            egui::pos2(rect.left() + s.left as f32 * base, rect.top() + s.top as f32 * base),
            egui::pos2(rect.right() - s.right as f32 * base, rect.bottom() - s.bottom as f32 * base),
        )
    });
    p.rect_stroke(inner, 0.0, egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 60)), egui::StrokeKind::Inside);
    // logo
    let mut logo_slot: Option<(String, f32)> = None;
    let assets = app.ui.brand_kits.assets.clone().unwrap_or(Value::Null);
    if let Some(l) = c
        .logos
        .iter()
        .find(|l| l.approved && (l.backgrounds.is_empty() || l.backgrounds.iter().any(|b| color_hex(&opts, &Some(b.clone())) == Some(bg))))
        .or(c.logos.first())
        && let Some(a) = assets["assets"].as_array().and_then(|x| x.iter().find(|a| a["id"].as_str() == Some(l.asset.as_str()))).cloned()
        && let Some(t) = asset_texture(app, ui.ctx(), &a)
    {
        let (fw6, fh6) = (f64::from(fw), f64::from(fh));
        let width_px =
            l.min_width.as_ref().map_or(fw6 * 0.28, |s| s.px(fw6, fh6)).max(l.max_width.as_ref().map_or(0.0, |s| s.px(fw6, fh6).min(fw6 * 0.28))) as f32;
        let s = t.size_vec2();
        let size = egui::vec2(width_px * k, width_px * k * s.y / s.x.max(1.0));
        let margin = l.margin_pct.map(|m| m as f32 * fw * k / 100.0);
        let area = margin.map_or(inner, |m| rect.shrink(m));
        let pos = anchor_pos(if l.placement.is_empty() { "top-left" } else { &l.placement }, rect, area, size);
        p.image(t.id(), egui::Rect::from_min_size(pos, size), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        logo_slot = Some((if l.placement.is_empty() { "top-left".to_string() } else { l.placement.clone() }, size.y + 10.0));
    }
    // text roles, stacked per anchor (after the logo's corner)
    let mut used: HashMap<String, f32> = HashMap::new();
    if let Some((anchor, h)) = logo_slot {
        used.insert(anchor, h);
    }
    for t in c.typography.iter().rev() {
        let sample = app.ui.brand_kits.samples.get(&t.role).cloned().unwrap_or_else(|| sample_for(&t.role));
        let text = apply_case(&sample, &t.case);
        let px = t.size.px(f64::from(fw), f64::from(fh)) as f32 * k;
        // text on a pill hugs its text; other text wraps to the safe area
        let wrap = if t.background.is_some() { 0.0 } else { inner.width() };
        let Some(tex) = texture(app, ui.ctx(), format!("t-{}-{}-{}-{px:.1}-{}-{wrap:.0}-{:?}", t.family, t.style, text, t.tracking, t.line_height), || {
            filmcraft_engine::font_library::preview_rgba_box(
                &t.family,
                &t.style,
                &text,
                px.max(4.0),
                t.tracking as f32,
                (wrap > 0.0).then_some(wrap),
                t.line_height.map(|v| v as f32),
            )
        }) else {
            continue;
        };
        let size = tex.size_vec2();
        let anchor = if t.placement.is_empty() { "center" } else { t.placement.as_str() };
        let offset = used.get(anchor).copied().unwrap_or(0.0);
        let mut pos = anchor_pos(anchor, rect, inner, size);
        if anchor.starts_with("bottom") || anchor == "lower-third" {
            pos.y -= offset;
        } else {
            pos.y += offset;
        }
        used.insert(anchor.to_string(), offset + size.y + 8.0 * k / 0.29);
        let r = egui::Rect::from_min_size(pos, size);
        if let Some(b) = color_hex(&opts, &t.background) {
            p.rect_filled(r.expand2(egui::vec2(px * 0.6, px * 0.35)), px, egui::Color32::from_rgb(b.0[0], b.0[1], b.0[2]));
        }
        let col = color_hex(&opts, &t.color).map_or(egui::Color32::WHITE, |h| egui::Color32::from_rgb(h.0[0], h.0[1], h.0[2]));
        let col = t.opacity.map_or(col, |o| col.gamma_multiply(o as f32));
        p.image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), col);
    }
    app.auto.add("brandKits.preview", rect, tl!("Brand preview"));
    // sample texts
    egui::Grid::new("brand-samples").num_columns(2).show(ui, |ui| {
        for t in &c.typography {
            ui.label(&t.role);
            let mut s = app.ui.brand_kits.samples.get(&t.role).cloned().unwrap_or_else(|| sample_for(&t.role));
            let r = ui.text_edit_singleline(&mut s);
            app.auto.add_response(&format!("brandKits.sample.{}", t.role), &r, &t.role);
            if r.changed() {
                app.ui.brand_kits.samples.insert(t.role.clone(), s);
            }
            ui.end_row();
        }
    });
}

fn sample_for(role: &str) -> String {
    match role {
        "headline" => "Headline sample".into(),
        "cta" => "Call to action".into(),
        "caption" | "subtitle" => "A caption line as it would read".into(),
        "lowerThird" => "Name Surname — Title".into(),
        "wordmark" => "Brand".into(),
        _ => format!("{role} sample"),
    }
}

fn history_ui(app: &mut FilmcraftApp, ui: &mut egui::Ui, kit: &Value) {
    ui.strong(tl!("Versions"));
    egui::Grid::new("brand-versions").num_columns(5).striped(true).show(ui, |ui| {
        for v in kit["versions"].as_array().cloned().unwrap_or_default().iter().rev() {
            ui.label(format!("v{}", v["version"]));
            ui.label(v["status"].as_str().unwrap_or(""));
            ui.label(v["updated"].as_str().unwrap_or(""));
            ui.label(v["by"].as_str().unwrap_or(""));
            ui.label(match v["approval"]["by"].as_str() {
                Some(b) => format!("approved by {b}: {}", v["approval"]["reference"].as_str().unwrap_or("")),
                None => v["note"].as_str().unwrap_or("").to_string(),
            });
            ui.end_row();
        }
    });
    ui.separator();
    ui.strong(tl!("Change record"));
    for (i, e) in kit["history"].as_array().cloned().unwrap_or_default().iter().rev().take(200).enumerate() {
        let r = ui.label(format!(
            "{} · {} · {} {}",
            e["at"].as_str().unwrap_or(""),
            e["by"].as_str().unwrap_or(""),
            e["action"].as_str().unwrap_or(""),
            e["detail"].as_str().unwrap_or("")
        ));
        app.auto.add_response(&format!("brandKits.history.{i}"), &r, e["action"].as_str().unwrap_or(""));
    }
}
