//! Colour Palette Library: stable ids, versions that never change, roles, shades, usage
//! rules, optimistic concurrency, persistence on disk, and the agent commands.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Session;
use crate::palette_library::{self, DIR_NAME};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("filmcraft-palettes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn session(data: &Path) -> Session {
    Session { library_data_dir: Some(data.to_path_buf()), ..Default::default() }
}

fn brand() -> Value {
    json!({
        "name": "Back Alive", "description": "Client palette", "by": "test", "note": "from the design system",
        "colors": [
            {"name": "Blue", "hex": "#0248AD", "role": "primary"},
            {"name": "Blue Deep", "hex": "#022a66", "role": "primary", "shadeOf": "Blue", "shade": "Deep"},
            {"name": "Yellow", "rgb": [255, 244, 85], "role": "accent"},
            {"name": "White", "hex": "FFFFFF", "role": "text"},
            {"name": "Black", "hex": "#07080C", "role": "background"}
        ],
        "rules": [
            {"kind": "prohibited", "foreground": "Yellow", "background": "White", "appliesTo": "text", "text": "Never yellow text on white"},
            {"kind": "permitted", "foreground": "White", "background": "Blue", "appliesTo": "text"},
            {"kind": "guideline", "colors": ["Yellow"], "text": "One yellow focal point"}
        ]
    })
}

#[test]
fn create_get_and_resolve_exact_values() {
    let d = tmp("create");
    let mut s = session(&d);
    let p = s.execute("palettes.create", brand()).unwrap();
    let id = p["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("pal_"));
    assert_eq!(p["latestVersion"], json!(1));
    let v = &p["version"];
    assert_eq!(v["colors"][0]["hex"], json!("#0248AD"));
    assert_eq!(v["colors"][2]["hex"], json!("#FFF455"), "rgb input stored exactly");
    assert_eq!(v["colors"][2]["rgb"], json!([255, 244, 85]));
    assert_eq!(v["colors"][1]["shadeOf"], v["colors"][0]["id"]);
    assert_eq!(v["rules"][0]["kind"], json!("prohibited"));
    assert_eq!(v["rules"][0]["foregroundName"], json!("Yellow"));
    assert_eq!(v["intact"], json!(true));
    assert!(d.join(DIR_NAME).join("palettes.json").is_file());
    // agents reference colours by id
    let yellow = v["colors"][2]["id"].as_str().unwrap();
    let r = s.execute("palettes.resolve", json!({"ref": format!("{id}@1/{yellow}")})).unwrap();
    assert_eq!(r["color"]["hex"], json!("#FFF455"));
    assert_eq!(r["rules"].as_array().unwrap().len(), 2, "the prohibition and the guideline about yellow");
    let by_name = s.execute("palettes.resolve", json!({"palette": id, "color": "blue deep"})).unwrap();
    assert_eq!(by_name["color"]["hex"], json!("#022A66"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn edits_make_new_versions_and_saved_versions_never_change() {
    let d = tmp("versions");
    let mut s = session(&d);
    let p = s.execute("palettes.create", brand()).unwrap();
    let id = p["id"].as_str().unwrap().to_string();
    let v1 = s.execute("palettes.get", json!({"id": id, "version": 1})).unwrap()["version"].clone();
    let blue = v1["colors"][0]["id"].as_str().unwrap().to_string();
    let u = s
        .execute(
            "palettes.update",
            json!({"id": id, "baseVersion": 1, "by": "agent", "note": "brighter blue", "changes": [
                {"op": "updateColor", "id": blue, "hex": "#0A5CD0"},
                {"op": "addColor", "color": {"name": "Skin", "hex": "#E7C6A6"}}
            ]}),
        )
        .unwrap();
    assert_eq!(u["newVersion"], json!(true));
    assert_eq!(u["latestVersion"], json!(2));
    // version 1 is exactly as saved; ids are stable across versions
    let again = s.execute("palettes.get", json!({"id": id, "version": 1})).unwrap()["version"].clone();
    assert_eq!(again, v1);
    let v2 = s.execute("palettes.get", json!({"id": id})).unwrap()["version"].clone();
    assert_eq!(v2["colors"][0]["id"], json!(blue));
    assert_eq!(v2["colors"][0]["hex"], json!("#0A5CD0"));
    assert_eq!(v2["by"], json!("agent"));
    assert_ne!(v2["sha256"], v1["sha256"]);
    // an edit based on an old version is refused, not merged over the newer one
    let e = s
        .execute("palettes.update", json!({"id": id, "baseVersion": 1, "changes": [{"op": "removeRule", "id": v1["rules"][2]["id"]}]}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("changed since version 1"), "{e}");
    // no change, no version
    let same = s.execute("palettes.update", json!({"id": id, "baseVersion": 2, "changes": []})).unwrap();
    assert_eq!((same["newVersion"].clone(), same["latestVersion"].clone()), (json!(false), json!(2)));
    // renaming is metadata: no new version, recorded in history
    s.execute("palettes.setInfo", json!({"id": id, "name": "Back Alive 2026", "by": "user"})).unwrap();
    let g = s.execute("palettes.get", json!({"id": id, "history": true})).unwrap();
    assert_eq!(g["latestVersion"], json!(2));
    assert!(g["history"].to_string().contains("renamed"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rules_and_shades_are_checked_never_invented() {
    let d = tmp("rules");
    let mut s = session(&d);
    let mut b = brand();
    b["rules"].as_array_mut().unwrap().push(json!({"kind": "permitted", "foreground": "Yellow", "background": "White", "appliesTo": "text"}));
    let e = s.execute("palettes.create", b).unwrap_err().to_string();
    assert!(e.contains("both permitted and prohibited"), "{e}");
    let e = s.execute("palettes.create", json!({"name": "x", "colors": [{"name": "A", "hex": "#000000", "shadeOf": "Nope"}]})).unwrap_err().to_string();
    assert!(e.contains("no colour `Nope`"), "{e}");
    let e = s.execute("palettes.create", json!({"name": "x", "colors": [{"name": "A", "hex": "#00000"}]})).unwrap_err().to_string();
    assert!(e.contains("six-digit"), "{e}");
    let e = s.execute("palettes.create", json!({"name": "x", "colors": [{"name": "A", "rgb": [0, 256, 0]}]})).unwrap_err().to_string();
    assert!(e.contains("0 to 255"), "{e}");
    let e = s
        .execute("palettes.create", json!({"name": "x", "colors": [{"name": "A", "hex": "#000000"}, {"name": "a", "hex": "#111111"}]}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("two colours are named"), "{e}");
    // a palette without rules has none: nothing is added
    let p = s.execute("palettes.create", json!({"name": "Plain", "colors": [{"name": "A", "hex": "#123456"}]})).unwrap();
    assert_eq!(p["version"]["rules"], json!([]));
    assert_eq!(p["version"]["colors"][0]["role"], json!(""));
    // a colour used by a rule, or with shades, cannot be removed out from under them
    let p = s.execute("palettes.create", brand()).unwrap();
    let id = p["id"].as_str().unwrap();
    let e = s.execute("palettes.update", json!({"id": id, "baseVersion": 1, "changes": [{"op": "removeColor", "id": "Yellow"}]})).unwrap_err().to_string();
    assert!(e.contains("used by a usage rule"), "{e}");
    let e = s.execute("palettes.update", json!({"id": id, "baseVersion": 1, "changes": [{"op": "removeColor", "id": "Blue"}]})).unwrap_err().to_string();
    assert!(e.contains("has shades"), "{e}");
    let ok = s
        .execute(
            "palettes.update",
            json!({"id": id, "baseVersion": 1, "changes": [{"op": "duplicateColor", "id": "Black"}, {"op": "removeColor", "id": "Black copy"}]}),
        )
        .unwrap();
    assert_eq!(ok["newVersion"], json!(false), "duplicate then remove is no change");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn list_search_duplicate_archive_delete() {
    let d = tmp("manage");
    let mut s = session(&d);
    let a = s.execute("palettes.create", brand()).unwrap()["id"].as_str().unwrap().to_string();
    s.execute("palettes.create", json!({"name": "Autumn", "colors": [{"name": "Rust", "hex": "#B7410E", "role": "accent"}]})).unwrap();
    let names = |v: &Value| v["palettes"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(names(&s.execute("palettes.list", json!({})).unwrap()), ["Back Alive", "Autumn"]);
    assert_eq!(names(&s.execute("palettes.list", json!({"query": "rust"})).unwrap()), ["Autumn"]);
    assert_eq!(names(&s.execute("palettes.list", json!({"query": "#fff455"})).unwrap()), ["Back Alive"]);
    let dup = s.execute("palettes.duplicate", json!({"id": a, "version": 1})).unwrap();
    assert_ne!(dup["id"], json!(a));
    assert_eq!(dup["name"], json!("Back Alive copy"));
    assert_eq!(dup["version"]["colors"], s.execute("palettes.get", json!({"id": a})).unwrap()["version"]["colors"]);
    s.execute("palettes.archive", json!({"id": a})).unwrap();
    assert_eq!(names(&s.execute("palettes.list", json!({})).unwrap()), ["Autumn", "Back Alive copy"]);
    assert_eq!(s.execute("palettes.list", json!({"includeArchived": true})).unwrap()["palettes"].as_array().unwrap().len(), 3);
    let e =
        s.execute("palettes.update", json!({"id": a, "baseVersion": 1, "changes": [{"op": "addColor", "color": {"hex": "#010101"}}]})).unwrap_err().to_string();
    assert!(e.contains("archived"), "{e}");
    assert!(s.execute("palettes.get", json!({"id": a, "version": 1})).is_ok(), "archived versions stay readable");
    let e = s.execute("palettes.delete", json!({"id": a})).unwrap_err().to_string();
    assert!(e.contains("confirm"), "{e}");
    s.execute("palettes.delete", json!({"id": a, "confirm": true})).unwrap();
    assert!(s.execute("palettes.get", json!({"id": a})).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn persists_on_disk_and_detects_tampering() {
    let d = tmp("persist");
    let id = {
        let mut s = session(&d);
        s.execute("palettes.create", brand()).unwrap()["id"].as_str().unwrap().to_string()
    };
    // a new session (as after a restart) reads the same palette
    let mut s2 = session(&d);
    let g = s2.execute("palettes.get", json!({"id": id})).unwrap();
    assert_eq!(g["version"]["colors"][0]["hex"], json!("#0248AD"));
    // an edit made outside FilmCraft breaks the fingerprint: reported, and no edit builds on it
    let path = d.join(DIR_NAME).join("palettes.json");
    let text = std::fs::read_to_string(&path).unwrap().replace("#0248AD", "#0248AE");
    std::fs::write(&path, text).unwrap();
    let g = s2.execute("palettes.get", json!({"id": id})).unwrap();
    assert_eq!(g["version"]["intact"], json!(false));
    let e = s2
        .execute("palettes.update", json!({"id": id, "baseVersion": 1, "changes": [{"op": "addColor", "color": {"hex": "#010101"}}]}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("fingerprint"), "{e}");
    // a damaged file is an error and is never overwritten
    std::fs::write(&path, "{ not json").unwrap();
    assert!(s2.execute("palettes.list", json!({})).unwrap_err().to_string().contains("damaged"));
    assert!(s2.execute("palettes.create", json!({"name": "x"})).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn palette_colours_reach_the_project_swatches() {
    let d = tmp("project");
    let mut s = session(&d);
    s.execute("file.newSequence", json!({})).unwrap();
    let id = s.execute("palettes.create", brand()).unwrap()["id"].as_str().unwrap().to_string();
    let r = s.execute("palettes.addToProjectColors", json!({"id": id})).unwrap();
    assert_eq!(r["added"].as_array().unwrap().len(), 5);
    assert!(s.project.custom_colors.iter().any(|c| c.hex() == "#FFF455"));
    assert!(s.project.custom_color_names.values().any(|n| n == "Back Alive — Yellow"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn ids_are_unique_and_stable() {
    let d = tmp("ids");
    let lib = d.join(DIR_NAME);
    let a = palette_library::create(&lib, &json!({"name": "Same", "colors": [{"name": "A", "hex": "#000000"}]})).unwrap();
    let b = palette_library::create(&lib, &json!({"name": "Same", "colors": [{"name": "A", "hex": "#000000"}]})).unwrap();
    assert_ne!(a.id, b.id);
    assert_ne!(a.versions[0].colors[0].id, b.versions[0].colors[0].id);
    let l = palette_library::load(&lib).unwrap();
    assert_eq!(l.get(&a.id).unwrap().versions[0].colors[0].id, a.versions[0].colors[0].id);
    let _ = std::fs::remove_dir_all(&d);
}
