//! Client brand kits: clients, assets, exact references, drafts and approval, protection of
//! referenced fonts, palettes and assets, project pinning, client separation, ARCA import.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Session;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("filmcraft-brand-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn session(data: &Path) -> Session {
    Session { library_data_dir: Some(data.to_path_buf()), ..Default::default() }
}

/// The bundled Inter renamed to `fam` (5 ASCII letters).
fn font_file(fam: &str, dir: &Path) -> PathBuf {
    let mut b = filmcraft_text::fonts::INTER_REGULAR.to_vec();
    let from: Vec<u8> = "Inter".encode_utf16().flat_map(u16::to_be_bytes).collect();
    let to: Vec<u8> = fam.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut i = 0;
    while i + from.len() <= b.len() {
        if b[i..i + from.len()] == from[..] {
            b[i..i + from.len()].copy_from_slice(&to);
        }
        i += 1;
    }
    let p = dir.join(format!("{fam}.ttf"));
    std::fs::write(&p, b).unwrap();
    p
}

fn png(dir: &Path, name: &str, rgba: [u8; 4], w: u32, h: u32) -> PathBuf {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
    let p = dir.join(name);
    img.save(&p).unwrap();
    p
}

fn ok(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, p.clone()).unwrap_or_else(|e| panic!("{cmd} {p}: {e}"))
}

fn err(s: &mut Session, cmd: &str, p: Value) -> String {
    s.execute(cmd, p.clone()).map(|v| panic!("{cmd} {p} should fail: {v}")).unwrap_err().to_string()
}

struct Fixture {
    d: PathBuf,
    s: Session,
    client: String,
    palette: String,
    logo: String,
    font: String,
}

fn fixture(name: &str, fam: &str) -> Fixture {
    let d = tmp(name);
    let mut s = session(&d.join("data"));
    let f = font_file(fam, &d);
    let font = ok(&mut s, "fonts.library.import", json!({"path": f.to_string_lossy(), "license": "OFL-1.1"}))["imported"][0]["font"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let palette = ok(
        &mut s,
        "palettes.create",
        json!({"name": format!("{name} colours"), "colors": [
            {"name": "Blue", "hex": "#0248AD", "role": "primary"}, {"name": "Yellow", "hex": "#FFF455", "role": "accent"},
            {"name": "White", "hex": "#FFFFFF", "role": "text"}, {"name": "Black", "hex": "#07080C", "role": "background"}],
            "rules": [{"kind": "prohibited", "foreground": "Yellow", "background": "White", "appliesTo": "text", "text": "Never yellow text on white"}]}),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let client = ok(&mut s, "brand.clients.create", json!({"name": format!("{name} client"), "by": "test"}))["id"].as_str().unwrap().to_string();
    let logo_file = png(&d, "logo-white.png", [255, 255, 255, 255], 400, 80);
    let logo = ok(&mut s, "brand.assets.import", json!({"client": client, "path": logo_file.to_string_lossy(), "name": "Wordmark white", "variant": "white"}))
        ["id"]
        .as_str()
        .unwrap()
        .to_string();
    Fixture { d, s, client, palette, logo, font }
}

fn content(fam: &str, palette: &str, logo: &str) -> Value {
    let c = |n: &str| json!({"palette": palette, "color": n});
    json!({
        "fonts": [{"family": fam, "style": "Regular"}],
        "palettes": [{"palette": palette, "version": 1}],
        "colors": [{"role": "background", "color": c("Blue")}, {"role": "focal", "color": c("Yellow")}],
        "typography": [
            {"role": "headline", "family": fam, "style": "Regular", "weight": 400, "size": {"value": 14.5, "unit": "%w", "minPx": 34, "maxPx": 150}, "color": c("White"), "accent": c("Yellow"), "case": "lower", "tracking": -0.03, "lineHeight": 0.92, "placement": "bottom-left"},
            {"role": "cta", "family": fam, "style": "Regular", "weight": 400, "size": {"value": 20, "unit": "px"}, "color": c("Black"), "background": c("Yellow"), "styling": "pill, radius 999px"}
        ],
        "logos": [{"asset": logo, "placement": "top-left", "marginPct": 6.5, "minWidth": {"value": 120, "unit": "px"}, "backgrounds": [c("Blue")], "forbidden": ["recolor", "stretch"]}],
        "rules": [{"kind": "guideline", "text": "Yellow is the one focal point: headlines and CTA only"}],
        "safeArea": {"top": 9, "right": 6.5, "bottom": 15, "left": 6.5, "unit": "%w"},
        "referenceFrame": [1080, 1920]
    })
}

#[test]
fn draft_approve_freeze_and_new_draft() {
    let mut f = fixture("flow", "Bkaaa");
    let s = &mut f.s;
    let k = ok(s, "brand.kits.create", json!({"client": f.client, "name": "Core", "by": "test", "content": content("Bkaaa", &f.palette, &f.logo)}));
    let kit = k["id"].as_str().unwrap().to_string();
    assert_eq!(k["latestStatus"], json!("draft"));
    // exact references were filled in from the short ones
    let g = ok(s, "brand.kits.get", json!({"id": kit}));
    assert_eq!(g["content"]["fonts"][0]["libraryFont"], json!(f.font));
    assert_eq!(g["content"]["fonts"][0]["weight"], json!(400));
    assert_eq!(g["content"]["palettes"][0]["version"], json!(1));
    assert_eq!(g["resolved"]["typography"][0]["color"]["hex"], json!("#FFFFFF"));
    assert_eq!(g["resolved"]["typography"][0]["sizePxAtReference"], json!(150.0), "14.5% of 1080 = 156.6, clamped to 150");
    assert_eq!(g["resolved"]["ready"], json!(true), "{}", g["resolved"]["issues"]);
    // approval needs explicit confirmation and a record
    assert!(err(s, "brand.kits.approve", json!({"id": kit, "version": 1, "approval": {"by": "A", "reference": "R"}})).contains("confirm"));
    assert!(err(s, "brand.kits.approve", json!({"id": kit, "version": 1, "confirm": true, "approval": {"by": "", "reference": "R"}})).contains("approver"));
    assert!(err(s, "brand.kits.approve", json!({"id": kit, "version": 1, "confirm": true, "approval": {"by": "A", "reference": ""}})).contains("reference"));
    let a = ok(s, "brand.kits.approve", json!({"id": kit, "version": 1, "confirm": true, "approval": {"by": "Brand owner", "reference": "email 2026-11-09"}}));
    assert_eq!(a["approvedVersion"], json!(1));
    let v1 = ok(s, "brand.kits.get", json!({"id": format!("{kit}@1")}));
    // editing an approved kit starts draft 2; version 1 never changes
    let mut c2 = v1["content"].clone();
    c2["typography"][0]["size"]["maxPx"] = json!(140);
    let saved = ok(
        s,
        "brand.kits.saveDraft",
        json!({"id": kit, "baseVersion": 1, "content": {"typography": c2["typography"]}, "by": "claude", "note": "smaller headline"}),
    );
    assert_eq!((saved["savedVersion"].clone(), saved["newVersion"].clone()), (json!(2), json!(true)));
    assert_eq!(ok(s, "brand.kits.get", json!({"id": format!("{kit}@1")}))["content"], v1["content"]);
    // a stale base is refused
    assert!(err(s, "brand.kits.saveDraft", json!({"id": kit, "baseVersion": 1, "content": {}})).contains("changed since version 1"));
    // draft saves are logged and do not create versions
    ok(s, "brand.kits.saveDraft", json!({"id": kit, "baseVersion": 2, "content": {"notes": "x"}, "by": "user"}));
    let h = ok(s, "brand.kits.get", json!({"id": kit, "history": true}));
    assert_eq!(h["latestVersion"], json!(2));
    assert!(h["history"].as_array().unwrap().iter().filter(|e| e["action"] == json!("saved")).count() >= 1);
    // the latest approved is what agents get by default
    assert_eq!(ok(s, "brand.resolve", json!({"kit": kit, "what": "typography:headline"}))["ref"], json!(format!("{kit}@1")));
    let _ = std::fs::remove_dir_all(&f.d);
}

#[test]
fn palette_changes_never_reach_an_approved_kit() {
    let mut f = fixture("palette", "Bkbbb");
    let s = &mut f.s;
    let kit = ok(s, "brand.kits.create", json!({"client": f.client, "name": "Core", "content": content("Bkbbb", &f.palette, &f.logo)}))["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(s, "brand.kits.approve", json!({"id": kit, "version": 1, "confirm": true, "approval": {"by": "o", "reference": "r"}}));
    let before = ok(s, "brand.kits.get", json!({"id": format!("{kit}@1")}))["resolved"].clone();
    let blue = before["palettes"][0]["colors"][0]["id"].as_str().unwrap().to_string();
    ok(s, "palettes.update", json!({"id": f.palette, "baseVersion": 1, "changes": [{"op": "updateColor", "id": blue, "hex": "#FF0000"}]}));
    let after = ok(s, "brand.kits.get", json!({"id": format!("{kit}@1")}))["resolved"].clone();
    assert_eq!(after["colors"][0]["color"]["hex"], json!("#0248AD"), "the kit still uses palette version 1");
    assert_eq!(after["palettes"], before["palettes"]);
    assert_eq!(after["ready"], json!(true));
    let _ = std::fs::remove_dir_all(&f.d);
}

#[test]
fn referenced_fonts_palettes_and_assets_are_protected() {
    let mut f = fixture("protect", "Bkccc");
    let s = &mut f.s;
    let kit = ok(s, "brand.kits.create", json!({"client": f.client, "name": "Core", "content": content("Bkccc", &f.palette, &f.logo)}))["id"]
        .as_str()
        .unwrap()
        .to_string();
    // a draft that uses the logo: removal refused until the draft drops it
    assert!(err(s, "brand.assets.remove", json!({"id": f.logo})).contains("draft"));
    ok(s, "brand.kits.approve", json!({"id": kit, "version": 1, "confirm": true, "approval": {"by": "o", "reference": "r"}}));
    assert!(err(s, "brand.assets.remove", json!({"id": f.logo})).contains("approved"));
    assert!(err(s, "fonts.library.remove", json!({"id": f.font, "force": true})).contains("approved brand kits use this font"));
    assert!(err(s, "palettes.delete", json!({"id": f.palette, "confirm": true})).contains("approved brand kits use this palette"));
    // everything still in place
    assert_eq!(ok(s, "brand.kits.check", json!({"id": kit}))["ready"], json!(true));
    // an unused asset can go
    let other = png(&f.d, "icon.png", [2, 72, 173, 255], 64, 64);
    let id = ok(s, "brand.assets.import", json!({"client": f.client, "path": other.to_string_lossy(), "variant": "icon"}))["id"].as_str().unwrap().to_string();
    ok(s, "brand.assets.remove", json!({"id": id}));
    let _ = std::fs::remove_dir_all(&f.d);
}

#[test]
fn missing_fonts_and_weights_are_reported_and_block_approval() {
    let mut f = fixture("missing", "Bkddd");
    let s = &mut f.s;
    // short references that do not match are refused, never guessed
    let mut c = content("Bkddd", &f.palette, &f.logo);
    c["fonts"] = json!([{"family": "Bkddd", "style": "Black"}]);
    assert!(err(s, "brand.kits.create", json!({"client": f.client, "name": "X", "content": c})).contains("no style \"Black\""));
    c["fonts"] = json!([{"family": "Bkddd", "style": "Regular", "weight": 900}]);
    assert!(err(s, "brand.kits.create", json!({"client": f.client, "name": "X", "content": c})).contains("weight 400, not 900"));
    c["fonts"] = json!([{"family": "Nofont", "style": "Regular"}]);
    assert!(err(s, "brand.kits.create", json!({"client": f.client, "name": "X", "content": c})).contains("not in the Font Library"));
    // a typography weight the face does not have
    let mut c = content("Bkddd", &f.palette, &f.logo);
    c["typography"][0]["weight"] = json!(900);
    let kit = ok(s, "brand.kits.create", json!({"client": f.client, "name": "Y", "content": c}))["id"].as_str().unwrap().to_string();
    let chk = ok(s, "brand.kits.check", json!({"id": kit}));
    assert_eq!(chk["ready"], json!(false));
    assert!(chk["issues"].to_string().contains("weight 900 is not available"), "{chk}");
    assert!(
        err(s, "brand.kits.approve", json!({"id": kit, "version": 1, "confirm": true, "approval": {"by": "o", "reference": "r"}}))
            .contains("cannot be approved")
    );
    // a font removed from the library after the draft referenced it
    let fine = ok(s, "brand.kits.create", json!({"client": f.client, "name": "Z", "content": content("Bkddd", &f.palette, &f.logo)}))["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(s, "fonts.library.remove", json!({"id": f.font, "force": true}));
    let chk = ok(s, "brand.kits.check", json!({"id": fine}));
    assert!(chk["issues"].to_string().contains("is missing: Font Library font"), "{chk}");
    let _ = std::fs::remove_dir_all(&f.d);
}

#[test]
fn assets_are_validated() {
    let mut f = fixture("assets", "Bkeee");
    let s = &mut f.s;
    let svg = f.d.join("logo.svg");
    std::fs::write(&svg, "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
    assert!(err(s, "brand.assets.import", json!({"client": f.client, "path": svg.to_string_lossy()})).contains("SVG logos are not supported"));
    let junk = f.d.join("junk.png");
    std::fs::write(&junk, b"\x89PNG\r\n\x1a\nnot really").unwrap();
    assert!(err(s, "brand.assets.import", json!({"client": f.client, "path": junk.to_string_lossy()})).contains("could not be decoded"));
    let again = f.d.join("logo-white.png");
    assert!(err(s, "brand.assets.import", json!({"client": f.client, "path": again.to_string_lossy()})).contains("already asset"));
    let a = ok(s, "brand.assets.list", json!({"client": f.client}));
    assert_eq!(a["assets"][0]["width"], json!(400));
    assert_eq!(a["assets"][0]["format"], json!("png"));
    let _ = std::fs::remove_dir_all(&f.d);
}

#[test]
fn project_pin_is_exact_and_guarded() {
    let mut f = fixture("pin", "Bkfff");
    let s = &mut f.s;
    s.execute("file.newSequence", json!({})).unwrap();
    let kit = ok(s, "brand.kits.create", json!({"client": f.client, "name": "Core", "content": content("Bkfff", &f.palette, &f.logo)}))["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(err(s, "brand.project.pin", json!({"kit": format!("{kit}@1")})).contains("draft"));
    ok(s, "brand.kits.approve", json!({"id": kit, "version": 1, "confirm": true, "approval": {"by": "o", "reference": "r"}}));
    let pin = ok(s, "brand.project.pin", json!({"kit": format!("{kit}@1"), "by": "user"}));
    assert_eq!(pin["pin"]["version"], json!(1));
    // version 2 approved later: the project stays on 1 until someone moves it
    ok(s, "brand.kits.saveDraft", json!({"id": kit, "baseVersion": 1, "content": {"notes": "v2"}}));
    ok(s, "brand.kits.approve", json!({"id": kit, "version": 2, "confirm": true, "approval": {"by": "o", "reference": "r2"}}));
    let g = ok(s, "brand.project.get", json!({}));
    assert_eq!((g["pin"]["version"].clone(), g["state"].clone(), g["newerApprovedVersion"].clone()), (json!(1), json!("ok"), json!(2)));
    assert!(err(s, "brand.project.pin", json!({"kit": format!("{kit}@2")})).contains("authorizeUpdate"));
    ok(s, "brand.project.pin", json!({"kit": format!("{kit}@2"), "authorizeUpdate": true}));
    // another client's kit cannot replace this client's branding by accident
    let other = ok(s, "brand.clients.create", json!({"name": "Other client"}))["id"].as_str().unwrap().to_string();
    let c = json!({"fonts": [{"family": "Bkfff", "style": "Regular"}], "palettes": [{"palette": f.palette, "version": 1}]});
    let ok_kit = ok(s, "brand.kits.create", json!({"client": other, "name": "Other", "content": c}))["id"].as_str().unwrap().to_string();
    ok(s, "brand.kits.approve", json!({"id": ok_kit, "version": 1, "confirm": true, "approval": {"by": "o", "reference": "r"}}));
    let e = err(s, "brand.project.pin", json!({"kit": format!("{ok_kit}@1"), "authorizeUpdate": true}));
    assert!(e.contains("replaceClient"), "{e}");
    assert_eq!(s.project.brand.as_ref().unwrap().client, f.client);
    // the pin is saved with the project
    let path = f.d.join("p.fcproj");
    ok(s, "file.saveAs", json!({"path": path.to_string_lossy()}));
    let mut s2 = session(&f.d.join("data"));
    ok(&mut s2, "file.open", json!({"path": path.to_string_lossy()}));
    assert_eq!(s2.project.brand.as_ref().unwrap().version, 2);
    assert_eq!(ok(&mut s2, "brand.project.get", json!({}))["state"], json!("ok"));
    let _ = std::fs::remove_dir_all(&f.d);
}

#[test]
fn a_kit_cannot_use_another_clients_logo() {
    let mut f = fixture("separate", "Bkggg");
    let s = &mut f.s;
    let other = ok(s, "brand.clients.create", json!({"name": "Someone else"}))["id"].as_str().unwrap().to_string();
    let kit = ok(s, "brand.kits.create", json!({"client": other, "name": "Theirs", "content": content("Bkggg", &f.palette, &f.logo)}))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let chk = ok(s, "brand.kits.check", json!({"id": kit}));
    assert!(chk["issues"].to_string().contains("belongs to another client"), "{chk}");
    let list = ok(s, "brand.assets.list", json!({"client": other}));
    assert_eq!(list["assets"], json!([]));
    let _ = std::fs::remove_dir_all(&f.d);
}

#[test]
fn arca_import_keeps_arca_the_authority() {
    let d = tmp("arca");
    let mut s = session(&d.join("data"));
    let docdir = d.join("arca-export");
    std::fs::create_dir_all(docdir.join("fonts")).unwrap();
    std::fs::create_dir_all(docdir.join("logos")).unwrap();
    let ff = font_file("Bkhhh", &docdir.join("fonts"));
    let lf = png(&docdir.join("logos"), "wordmark.png", [255, 255, 255, 255], 300, 60);
    let sha = |p: &Path| crate::user_library::sha256_hex(&std::fs::read(p).unwrap());
    let mut doc = json!({
        "format": "arca.brand/0.1",
        "arca": {"authority": "ARCA", "document": "Clients/Test/brand", "revision": "r1"},
        "client": {"id": "test-client", "name": "Test Client"},
        "kit": {"name": "Test Core"},
        "fonts": [{"family": "Bkhhh", "style": "Regular", "weight": 400, "file": "fonts/Bkhhh.ttf", "sha256": sha(&ff), "license": "OFL-1.1"}],
        "palette": {"colors": [{"key": "blue", "name": "Blue", "hex": "#0248AD", "role": "primary"}, {"key": "white", "name": "White", "hex": "#FFFFFF", "role": "text"}]},
        "colors": [{"role": "background", "color": "blue"}],
        "typography": [{"role": "headline", "family": "Bkhhh", "style": "Regular", "weight": 400, "size": {"value": 64, "unit": "px"}, "color": "white"}],
        "logos": [{"file": "logos/wordmark.png", "sha256": sha(&lf), "name": "Wordmark", "variant": "white", "backgrounds": ["blue"]}],
        "approval": {"status": "approved", "by": "ARCA brand owner", "reference": "ARCA decision 12", "date": "2026-11-01"}
    });
    let path = docdir.join("brand.json");
    std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let v = ok(&mut s, "brand.arca.validate", json!({"path": path.to_string_lossy()}));
    assert_eq!(v["valid"], json!(true), "{v}");
    let r = ok(&mut s, "brand.arca.import", json!({"path": path.to_string_lossy()}));
    assert_eq!(r["status"], json!("approved"));
    let kit = r["kit"].as_str().unwrap().to_string();
    let g = ok(&mut s, "brand.kits.get", json!({"id": kit, "approved": true}));
    assert_eq!(g["resolved"]["approval"]["authority"], json!("ARCA"));
    assert_eq!(g["resolved"]["ready"], json!(true), "{}", g["resolved"]["issues"]);
    // read-only here; FilmCraft cannot approve ARCA branding either
    assert!(err(&mut s, "brand.kits.saveDraft", json!({"id": kit, "baseVersion": 1, "content": {}})).contains("managed by ARCA"));
    // the same document again: nothing new
    assert_eq!(ok(&mut s, "brand.arca.import", json!({"path": path.to_string_lossy()}))["version"], json!(1));
    // a new ARCA revision without approval: a draft version 2, version 1 untouched
    doc["arca"]["revision"] = json!("r2");
    doc["typography"][0]["size"]["value"] = json!(72);
    doc.as_object_mut().unwrap().remove("approval");
    std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let r2 = ok(&mut s, "brand.arca.import", json!({"path": path.to_string_lossy()}));
    assert_eq!((r2["version"].clone(), r2["status"].clone()), (json!(2), json!("draft")));
    assert!(
        err(&mut s, "brand.kits.approve", json!({"id": kit, "version": 2, "confirm": true, "approval": {"by": "me", "reference": "x"}}))
            .contains("managed by ARCA")
    );
    assert_eq!(ok(&mut s, "brand.kits.get", json!({"id": format!("{kit}@1")}))["content"]["typography"][0]["size"]["value"], json!(64.0));
    // invalid documents change nothing
    let bad = json!({"format": "arca.brand/0.2"});
    assert_eq!(ok(&mut s, "brand.arca.validate", json!({"document": bad}))["valid"], json!(false));
    let _ = std::fs::remove_dir_all(&d);
}
