//! Font library: import, licence, persistence on disk, removal, the export guard and `qc`-side
//! reports. Fonts are made by renaming the bundled Inter (same-length UTF-16 name strings), so
//! the tests need no font files; each test uses its own family name (the font database is
//! process-wide).

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::Session;
use crate::font_library::{self, License};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("filmcraft-fontlib-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The bundled Inter with its family renamed to `fam` (5 ASCII letters).
fn renamed_font(fam: &str, dir: &Path) -> PathBuf {
    assert_eq!(fam.len(), 5);
    let mut b = filmcraft_text::fonts::INTER_REGULAR.to_vec();
    let from: Vec<u8> = "Inter".encode_utf16().flat_map(u16::to_be_bytes).collect();
    let to: Vec<u8> = fam.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut i = 0;
    while i + from.len() <= b.len() {
        if b[i..i + from.len()] == from[..] {
            b[i..i + from.len()].copy_from_slice(&to);
            i += from.len();
        } else {
            i += 1;
        }
    }
    let p = dir.join(format!("{fam}-Regular.ttf"));
    std::fs::write(&p, b).unwrap();
    p
}

fn ofl() -> License {
    License { spdx: "OFL-1.1".into(), text: "https://openfontlicense.org".into(), source: "test".into() }
}

fn session_with(lib: &Path) -> Session {
    Session { font_library_dir: Some(lib.to_path_buf()), ..Default::default() }
}

#[test]
fn import_needs_a_licence_and_registers_the_font() {
    let d = tmp("import");
    let lib = d.join("lib");
    let f = renamed_font("Fltaa", &d);
    assert_eq!(filmcraft_text::fonts::status("Fltaa", "Regular"), filmcraft_text::fonts::FontStatus::Missing);
    for spdx in ["", "  ", "NOASSERTION", "none", "OFL<1.1>"] {
        let e = font_library::import(&lib, &f, License { spdx: spdx.into(), ..Default::default() }, false).unwrap_err();
        assert!(e.contains("licen") || e.contains("SPDX"), "{spdx:?}: {e}");
    }
    assert!(!lib.join("library.json").exists(), "a refused import writes nothing");
    let r = font_library::import(&lib, &f, ofl(), false).unwrap();
    assert!(r.added);
    assert_eq!(r.font.faces[0].family, "Fltaa");
    assert_eq!(r.font.license.spdx, "OFL-1.1");
    assert!(lib.join(&r.font.file).is_file());
    let st = filmcraft_text::fonts::status("Fltaa", "Regular");
    let filmcraft_text::fonts::FontStatus::Exact { face } = st else { panic!("{st:?}") };
    assert_eq!(filmcraft_text::fonts::face(face).info.origin, filmcraft_text::fonts::LIBRARY_ORIGIN);
    // the same file again: no duplicate, licence update recorded
    let again = font_library::import(&lib, &f, License { spdx: "Apache-2.0".into(), ..Default::default() }, false).unwrap();
    assert!(!again.added);
    let m = font_library::read_manifest(&lib).unwrap();
    assert_eq!(m.fonts.len(), 1);
    assert_eq!(m.fonts[0].license.spdx, "Apache-2.0");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_second_font_with_the_same_family_and_style_needs_replace() {
    let d = tmp("clash");
    let lib = d.join("lib");
    let a = renamed_font("Fltbb", &d);
    font_library::import(&lib, &a, ofl(), false).unwrap();
    // same names, different bytes
    let mut b = std::fs::read(&a).unwrap();
    b.extend_from_slice(&[0, 0, 0, 0]);
    let bp = d.join("other.ttf");
    std::fs::write(&bp, b).unwrap();
    let e = font_library::import(&lib, &bp, ofl(), false).unwrap_err();
    assert!(e.contains("already has") && e.contains("Fltbb"), "{e}");
    let r = font_library::import(&lib, &bp, ofl(), true).unwrap();
    assert_eq!(r.replaced.len(), 1);
    let m = font_library::read_manifest(&lib).unwrap();
    assert_eq!(m.fonts.len(), 1);
    assert_eq!(m.fonts[0].original_name, "other.ttf");
    assert!(filmcraft_text::fonts::status("Fltbb", "Regular").is_exact());
    let files = std::fs::read_dir(&lib).unwrap().count();
    assert_eq!(files, 2, "manifest + the one font file");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn removing_a_font_makes_it_missing_and_deletes_the_file() {
    let d = tmp("remove");
    let lib = d.join("lib");
    let f = renamed_font("Fltcc", &d);
    let r = font_library::import(&lib, &f, ofl(), false).unwrap();
    let mut s = session_with(&lib);
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("graphics.newText", json!({"text": "Hello", "font": "Fltcc", "fontStyle": "Regular"})).unwrap();
    let e = s.execute("fonts.library.remove", json!({"id": r.font.id})).unwrap_err().to_string();
    assert!(e.contains("uses Fltcc Regular"), "{e}");
    let v = s.execute("fonts.library.remove", json!({"id": r.font.id, "force": true})).unwrap();
    assert_eq!(v["removed"], json!(r.font.id));
    assert_eq!(filmcraft_text::fonts::status("Fltcc", "Regular"), filmcraft_text::fonts::FontStatus::Missing);
    assert!(!lib.join(&r.font.file).exists());
    assert!(font_library::read_manifest(&lib).unwrap().fonts.is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_damaged_or_changed_library_is_reported_not_trusted() {
    let d = tmp("damaged");
    let lib = d.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("library.json"), b"{ not json").unwrap();
    let f = renamed_font("Fltdd", &d);
    let e = font_library::import(&lib, &f, ofl(), false).unwrap_err();
    assert!(e.contains("damaged"), "{e}");
    assert_eq!(std::fs::read(lib.join("library.json")).unwrap(), b"{ not json", "a damaged manifest is never overwritten");
    // a font file edited on disk after it was added is not registered
    let lib2 = d.join("lib2");
    let r = font_library::import(&lib2, &f, ofl(), false).unwrap();
    let copy = d.join("lib3");
    std::fs::create_dir_all(&copy).unwrap();
    let mut m = font_library::read_manifest(&lib2).unwrap();
    m.fonts[0].sha256 = "0".repeat(64);
    std::fs::write(copy.join("library.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    std::fs::copy(lib2.join(&r.font.file), copy.join(&r.font.file)).unwrap();
    assert_eq!(font_library::load_dir(&copy), 0);
    assert!(font_library::problems().iter().any(|p| p.contains("changed on disk")), "{:?}", font_library::problems());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn hostile_files_are_refused() {
    let d = tmp("hostile");
    let lib = d.join("lib");
    for (name, bytes) in [("empty.ttf", Vec::new()), ("text.otf", b"hello".to_vec()), ("trunc.woff2", b"wOF2\0\x01\0\0".to_vec())] {
        let p = d.join(name);
        std::fs::write(&p, bytes).unwrap();
        assert!(font_library::import(&lib, &p, ofl(), false).is_err(), "{name}");
    }
    assert!(font_library::import(&lib, &d.join("does-not-exist.ttf"), ofl(), false).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn export_refuses_missing_and_substituted_fonts_unless_allowed() {
    let d = tmp("export");
    let lib = d.join("lib");
    let mut s = session_with(&lib);
    s.execute("file.newSequence", json!({"width": 64, "height": 36})).unwrap();
    s.execute("graphics.newText", json!({"text": "Brand", "font": "Fltee", "fontStyle": "Regular", "size": 20, "seconds": 0.2})).unwrap();
    let report = s.execute("fonts.missing", json!({})).unwrap();
    assert_eq!(report["ok"], json!(false));
    let f0 = &report["sequences"][0]["fonts"][0];
    assert_eq!(f0["family"], json!("Fltee"));
    assert_eq!(f0["status"], json!("missing"));
    assert_eq!(f0["rendered_as"], json!("Inter Regular"));
    let out = d.join("frame.png");
    let p = json!({"path": out.to_string_lossy(), "format": "png", "range": "custom", "startSeconds": 0, "endSeconds": 0.04, "wait": true});
    let e = s.execute("file.exportMedia", p.clone()).unwrap_err().to_string();
    assert!(e.contains("Fltee Regular is missing") && e.contains("Inter Regular"), "{e}");
    let e = s.execute("export.queue.add", p.clone()).unwrap_err().to_string();
    assert!(e.contains("Fltee"), "{e}");
    // explicit override: exported, logged, warned
    let mut allowed = p.clone();
    allowed["allowFontSubstitution"] = json!(true);
    let v = s.execute("file.exportMedia", allowed).unwrap();
    assert!(v["warnings"].to_string().contains("font substitution allowed"), "{v}");
    assert!(s.autoedit_log.iter().any(|e| e["kind"] == json!("fontSubstitutionAllowed")));
    // a family that exists, a style it lacks: substituted, also refused
    s.execute("graphics.newText", json!({"text": "X", "font": "Inter", "fontStyle": "Condensed Hairline", "size": 20, "seconds": 0.2})).unwrap();
    let r = s.execute("fonts.missing", json!({})).unwrap();
    assert!(r.to_string().contains("\"substituted\""), "{r}");
    // add the font to the library: the export goes through
    let f = renamed_font("Fltee", &d);
    s.execute("fonts.library.import", json!({"path": f.to_string_lossy(), "license": "OFL-1.1"})).unwrap();
    let e = s.execute("file.exportMedia", p.clone()).unwrap_err().to_string();
    assert!(!e.contains("Fltee") && e.contains("Inter Condensed Hairline is substituted"), "{e}");
    // audio-only exports never check fonts
    let wav = json!({"path": d.join("a.wav").to_string_lossy(), "format": "wav", "wait": true, "range": "custom", "startSeconds": 0, "endSeconds": 0.04});
    s.execute("file.exportMedia", wav).unwrap();
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn library_commands_list_inspect_preview() {
    let d = tmp("commands");
    let lib = d.join("lib");
    let f = renamed_font("Fltff", &d);
    let mut s = session_with(&lib);
    let ins = s.execute("fonts.library.inspect", json!({"path": f.to_string_lossy()})).unwrap();
    assert_eq!(ins["format"], json!("ttf"));
    assert_eq!(ins["faces"][0]["family"], json!("Fltff"));
    assert_eq!(ins["suggested_spdx"], json!("OFL-1.1"), "Inter states the OFL in its name table: {ins}");
    let e = s.execute("fonts.library.import", json!({"path": f.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("licence is required"), "{e}");
    s.execute("fonts.library.import", json!({"path": f.to_string_lossy(), "license": {"spdx": "OFL-1.1", "source": "Unit test"}})).unwrap();
    let l = s.execute("fonts.library.list", json!({})).unwrap();
    let font = &l["fonts"][0];
    assert_eq!(font["families"], json!(["Fltff"]));
    assert_eq!(font["faces"][0]["state"], json!("active"));
    assert_eq!(font["license"]["source"], json!("Unit test"));
    let id = font["id"].as_str().unwrap().to_string();
    s.execute("fonts.library.setLicense", json!({"id": id, "license": "Apache-2.0"})).unwrap();
    assert_eq!(font_library::read_manifest(&lib).unwrap().fonts[0].license.spdx, "Apache-2.0");
    let png = d.join("preview.png");
    let pv = s.execute("fonts.library.preview", json!({"family": "Fltff", "path": png.to_string_lossy(), "size": 24})).unwrap();
    assert_eq!(pv["rows"][0]["exact"], json!(true));
    assert!(std::fs::metadata(&png).unwrap().len() > 100);
    let _ = std::fs::remove_dir_all(&d);
}

/// A title exported while its font was missing, then again after the font is added, must
/// differ: no cache (layouts, layer rasters) may keep drawing the stand-in font. The title is
/// not touched between the exports.
#[test]
fn installing_a_font_changes_the_rendered_title() {
    let d = tmp("rerender");
    let lib = d.join("lib");
    let mut s = session_with(&lib);
    s.execute("file.newSequence", json!({"width": 160, "height": 90})).unwrap();
    // Noto Serif renamed (same length), so its outlines differ from the Inter stand-in
    let fam = "Fltgg Serf";
    s.execute("graphics.newText", json!({"text": "Brand", "font": fam, "fontStyle": "Regular", "size": 40, "position": [10, 60], "seconds": 0.2, "time": 0}))
        .unwrap();
    let export = |s: &mut Session, name: &str, allow: bool| -> Vec<u8> {
        let out = d.join(name);
        let p = json!({"path": out.to_string_lossy(), "format": "png", "range": "custom", "startSeconds": 0, "endSeconds": 0.04, "wait": true, "allowFontSubstitution": allow});
        let v = s.execute("file.exportMedia", p).unwrap();
        std::fs::read(v["lastPath"].as_str().or(v["path"].as_str()).unwrap()).unwrap()
    };
    let before = export(&mut s, "before.png", true);
    assert_eq!(before, export(&mut s, "again.png", true), "the stand-in drawn twice is the same picture");
    let mut b = filmcraft_text::fonts::NOTO_SERIF_REGULAR.to_vec();
    let f: Vec<u8> = "Noto Serif".encode_utf16().flat_map(u16::to_be_bytes).collect();
    let t: Vec<u8> = fam.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut i = 0;
    while i + f.len() <= b.len() {
        if b[i..i + f.len()] == f[..] {
            b[i..i + f.len()].copy_from_slice(&t);
        }
        i += 1;
    }
    let path = d.join("serif.ttf");
    std::fs::write(&path, b).unwrap();
    s.execute("fonts.library.import", json!({"path": path.to_string_lossy(), "license": "OFL-1.1"})).unwrap();
    let after = export(&mut s, "after.png", false);
    assert_ne!(before, after, "the title must be drawn in the installed font");
    let _ = std::fs::remove_dir_all(&d);
}
