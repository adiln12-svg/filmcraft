//! The Changes panel (M3B): what Claude changed, versions of the project, and Claude's requests to
//! change protected work. Beginner wording in Creator Mode; revisions and commands in Pro.
//!
//! Everything here runs engine commands (`agent.activity`, `versions.*`, `agent.authorize`,
//! `edit.undo`), so the same actions are available to scripts.

use egui::{Rect, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// "14:03" from an RFC 3339 time.
fn hhmm(t: &str) -> String {
    t.get(11..16).unwrap_or(t).to_string()
}

enum Act {
    Cmd(&'static str, Value),
}

fn run(app: &mut FilmcraftApp, acts: Vec<Act>) {
    for Act::Cmd(id, p) in acts {
        match app.session.execute(id, p) {
            Ok(v) => {
                if id == "versions.create" {
                    app.ui.status = tlf!("Saved version {v}", v = v["version"].as_str().unwrap_or_default());
                }
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    }
}

fn dim(ui: &mut egui::Ui, app: &FilmcraftApp, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).size(11.5).color(app.tokens.text_dim));
}

/// The Changes panel.
pub fn panel(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    ui.painter().rect_filled(rect, 0.0, app.tokens.panel_bg);
    let mut acts = Vec::new();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(8.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    egui::ScrollArea::vertical().id_salt("changes-panel").auto_shrink([false, false]).show(&mut child, |ui| {
        requests_section(app, ui, &mut acts);
        claude_section(app, ui, &mut acts);
        ui.add_space(10.0);
        versions_section(app, ui, &mut acts);
    });
    run(app, acts);
}

fn requests_section(app: &mut FilmcraftApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let pending: Vec<(u64, String, Vec<String>)> =
        app.session.collab.requests.iter().filter(|r| r.status == "pending").map(|r| (r.id, r.reason.clone(), r.names.clone())).collect();
    for (id, reason, names) in pending {
        egui::Frame::new().fill(app.tokens.pill_active_bg).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
            ui.label(egui::RichText::new(tl!("Claude asks to change protected work")).strong());
            ui.label(format!("“{reason}”"));
            dim(ui, app, names.join(", "));
            ui.horizontal(|ui| {
                let r = ui.button(tl!("Allow once"));
                app.auto.add_response(&format!("changes.request.{id}.allow"), &r, tl!("Allow once"));
                if r.clicked() {
                    acts.push(Act::Cmd("agent.authorize", json!({"request": id, "allow": true})));
                }
                let r = ui.button(tl!("Keep protected"));
                app.auto.add_response(&format!("changes.request.{id}.deny"), &r, tl!("Keep protected"));
                if r.clicked() {
                    acts.push(Act::Cmd("agent.authorize", json!({"request": id, "allow": false})));
                }
            });
        });
        ui.add_space(6.0);
    }
}

fn claude_section(app: &mut FilmcraftApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let pro = !crate::panels::creator::is_on(app);
    ui.label(egui::RichText::new(tl!("Claude's changes")).strong());
    let list: Vec<Value> = app.session.collab.activity.iter().rev().take(40).cloned().collect();
    if list.is_empty() {
        dim(ui, app, tl!("Nothing yet. When Claude edits this project, each change is listed here and can be undone."));
        return;
    }
    // the newest undo step, when it is Claude's: Undo applies to it
    let top = app.session.history.undo.last().map(|u| u.0.clone());
    for (i, e) in list.iter().enumerate() {
        let label = e["label"].as_str().unwrap_or_default().to_string();
        let ok = e["ok"].as_bool().unwrap_or(false);
        let mark = if ok {
            "✓"
        } else if e["blocked"] == true {
            "🔒"
        } else {
            "✕"
        };
        let r = ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(hhmm(e["time"].as_str().unwrap_or_default())).color(app.tokens.text_dim));
            ui.label(format!("{mark} {label}"));
        });
        app.auto.add_response(&format!("changes.claude.{i}"), &r.response, &label);
        let detail = if ok {
            e["summary"].as_str().unwrap_or_default().to_string()
        } else if e["blocked"] == true {
            tl!("Blocked: it would have changed protected work").to_string()
        } else if e["conflict"] == true {
            tl!("Not applied: the project had changed meanwhile").to_string()
        } else {
            tlf!("Failed: {error}", error = e["error"].as_str().unwrap_or_default().chars().take(140).collect::<String>())
        };
        dim(ui, app, detail);
        let names: Vec<&str> = e["affected"]["names"].as_array().into_iter().flatten().filter_map(Value::as_str).take(4).collect();
        if ok && !names.is_empty() {
            dim(ui, app, names.join(" · "));
        }
        if pro {
            dim(ui, app, format!("revision {} → {} · {}", e["revisionBefore"], e["revisionAfter"], e["commands"]));
        }
        ui.horizontal(|ui| {
            if ok && i == 0 && top.as_deref() == Some(&format!("Claude: {label}")) {
                let r = ui.small_button(tl!("Undo"));
                app.auto.add_response("changes.claude.undo", &r, tl!("Undo Claude's change"));
                if r.clicked() {
                    acts.push(Act::Cmd("edit.undo", json!({})));
                }
            }
            if let Some(v) = e["snapshot"].as_str() {
                let r = ui.small_button(tl!("Restore the version before")).on_hover_text(tl!("The whole project as it was before this change (undoable)"));
                app.auto.add_response(&format!("changes.claude.{i}.restore"), &r, tl!("Restore the version before"));
                if r.clicked() {
                    acts.push(Act::Cmd("versions.restore", json!({ "version": v })));
                }
            }
        });
        ui.add_space(4.0);
    }
}

fn versions_section(app: &mut FilmcraftApp, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    ui.label(egui::RichText::new(tl!("Versions")).strong());
    let r = ui.add(egui::Button::new(tl!("Save a version now")).min_size(vec2(160.0, 24.0)));
    app.auto.add_response("changes.versions.create", &r, tl!("Save a version now"));
    if r.clicked() {
        acts.push(Act::Cmd("versions.create", json!({"label": "Saved by you"})));
    }
    // the list on disk, read again when the project changes or every few seconds (not every frame)
    let key = egui::Id::unique("changes-versions");
    let stamp = (app.session.collab.revision, app.session.collab.activity.len(), app.session.path.clone());
    let now = ui.input(|i| i.time);
    let cached: Option<(u64, usize, Option<String>, f64, Vec<Value>)> = ui.ctx().data(|d| d.get_temp(key));
    let list = match cached {
        Some((r, n, p, t, l)) if r == stamp.0 && n == stamp.1 && p == stamp.2 && now - t < 3.0 => l,
        _ => {
            let l = app.session.execute("versions.list", json!({})).map(|v| v["versions"].as_array().cloned().unwrap_or_default()).unwrap_or_default();
            ui.ctx().data_mut(|d| d.insert_temp(key, (stamp.0, stamp.1, stamp.2.clone(), now, l.clone())));
            l
        }
    };
    if list.is_empty() {
        dim(ui, app, tl!("FilmCraft also keeps a version before each large change Claude makes."));
    }
    for v in list.iter().rev().take(30) {
        let id = v["id"].as_str().unwrap_or_default().to_string();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} {}",
                    v["time"].as_str().unwrap_or_default().get(..10).unwrap_or_default(),
                    hhmm(v["time"].as_str().unwrap_or_default())
                ))
                .color(app.tokens.text_dim),
            );
            ui.label(v["label"].as_str().unwrap_or_default());
            let r = ui.small_button(tl!("Restore"));
            app.auto.add_response(&format!("changes.versions.{id}.restore"), &r, tl!("Restore"));
            if r.clicked() {
                acts.push(Act::Cmd("versions.restore", json!({ "version": id })));
            }
        });
    }
}

/// Claude's pending request as a window, so the person sees it wherever they are.
pub fn request_window(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(req) = app.session.collab.requests.iter().find(|r| r.status == "pending").cloned() else { return };
    let mut acts = Vec::new();
    let w = egui::Window::new(tl!("Claude asks to change protected work"))
        .id(egui::Id::unique("claude-request"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(format!("“{}”", req.reason));
            ui.add_space(4.0);
            ui.label(tl!("It would change:"));
            for n in &req.names {
                ui.label(format!("🔒 {n}"));
            }
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(tl!("Allowing lets Claude make this one change. Everything stays protected otherwise, and Undo still works."))
                    .size(11.5)
                    .color(app.tokens.text_dim),
            );
            ui.horizontal(|ui| {
                let r = ui.button(tl!("Allow once"));
                app.auto.add_response("claudeRequest.allow", &r, tl!("Allow once"));
                if r.clicked() {
                    acts.push(Act::Cmd("agent.authorize", json!({"request": req.id, "allow": true})));
                }
                let r = ui.button(tl!("Keep protected"));
                app.auto.add_response("claudeRequest.deny", &r, tl!("Keep protected"));
                if r.clicked() {
                    acts.push(Act::Cmd("agent.authorize", json!({"request": req.id, "allow": false})));
                }
            });
        });
    if let Some(w) = w {
        app.auto.add_response("claudeRequest.window", &w.response, tl!("Claude asks to change protected work"));
    }
    run(app, acts);
}
