//! `arca.brand/0.1`: the proposed contract for importing a client's approved brand from ARCA
//! (see `docs/arca-brand-0.1.md`).
//!
//! ARCA is the authority for approved brand decisions; FilmCraft stores the assets, validates
//! them and renders with them. So an import:
//!
//! - puts fonts into the Font Library, colours into a Palette Library palette (a new version
//!   when they changed) and logos into the client's brand assets — the same libraries the
//!   interface uses, never a second copy;
//! - creates or reuses the client (`source: arca:<id>`) and kit (managed by ARCA: read-only in
//!   FilmCraft) and adds a version with the document's content;
//! - approves that version only when the document carries ARCA's approval (`approval.status:
//!   "approved"` with `by` and `reference`), recorded with authority "ARCA"; FilmCraft never
//!   approves ARCA branding on its own, and an import never changes an existing version.
//!
//! [`validate`] checks a document without changing anything.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{AssetSpec, ColorRef, Dirs, KitContent, KitSource, KitVersion, LogoUse, SafeArea, Size, TypeRole};
use crate::user_library::{now_rfc3339, sha256_hex};
use crate::{font_library, palette_library};

pub const FORMAT: &str = "arca.brand/0.1";

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}

/// A file named by the document, relative to the document's folder; it must stay inside it.
fn doc_file(base: Option<&Path>, rel: &str) -> Result<PathBuf, String> {
    let base = base.ok_or("the document names files; import it from a path (`path`) so they can be found")?;
    let p = Path::new(rel);
    if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(format!("file `{rel}` must be relative to the document and stay inside its folder"));
    }
    Ok(base.join(p))
}

fn check_file(base: Option<&Path>, rel: &str, sha: &str, what: &str, errors: &mut Vec<String>) -> Option<PathBuf> {
    match doc_file(base, rel) {
        Err(e) => errors.push(format!("{what}: {e}")),
        Ok(p) => match std::fs::read(&p) {
            Err(e) => errors.push(format!("{what}: cannot read {}: {e}", p.display())),
            Ok(b) if !sha.is_empty() && sha256_hex(&b) != sha.to_ascii_lowercase() => {
                errors.push(format!("{what}: {} does not match the SHA-256 the document gives", p.display()))
            }
            Ok(_) => return Some(p),
        },
    }
    None
}

/// Check a document: structure, colour keys, files and fingerprints, fonts. Changes nothing.
pub fn validate(dirs: &Dirs, doc: &Value, base: Option<&Path>) -> Value {
    let mut errors = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    if s(doc, "format") != FORMAT {
        errors.push(format!("`format` must be \"{FORMAT}\" (got \"{}\")", s(doc, "format")));
    }
    if s(&doc["arca"], "authority") != "ARCA" {
        errors.push("`arca.authority` must be \"ARCA\"".into());
    }
    for (k, what) in [("client", "client.id"), ("document", "arca.document"), ("revision", "arca.revision")] {
        let v = if k == "client" { s(&doc["client"], "id") } else { s(&doc["arca"], k) };
        if v.trim().is_empty() {
            errors.push(format!("`{what}` is required"));
        }
    }
    if s(&doc["client"], "name").trim().is_empty() || s(&doc["kit"], "name").trim().is_empty() {
        errors.push("`client.name` and `kit.name` are required".into());
    }
    let keys: Vec<String> = arr(&doc["palette"], "colors").iter().map(|c| s(c, "key").to_string()).collect();
    if keys.is_empty() {
        errors.push("`palette.colors` is empty".into());
    }
    for (i, c) in arr(&doc["palette"], "colors").iter().enumerate() {
        if s(c, "key").is_empty() || filmcraft_project::HexColor::parse(s(c, "hex")).is_none() {
            errors.push(format!("palette colour {}: needs `key` and `hex` (#RRGGBB)", i + 1));
        }
    }
    let known = |k: &str| keys.iter().any(|x| x == k);
    let mut refs: Vec<(String, String)> = Vec::new();
    for t in arr(doc, "typography") {
        for k in ["color", "background", "accent"] {
            if let Some(c) = t.get(k).and_then(Value::as_str) {
                refs.push((format!("typography {} {k}", s(t, "role")), c.to_string()));
            }
        }
    }
    for c in arr(doc, "colors") {
        refs.push((format!("colour role {}", s(c, "role")), s(c, "color").to_string()));
    }
    for r in arr(&doc["palette"], "rules").iter().chain(arr(doc, "rules")) {
        for k in ["foreground", "background"] {
            if let Some(c) = r.get(k).and_then(Value::as_str) {
                refs.push((format!("rule {k}"), c.to_string()));
            }
        }
    }
    for l in arr(doc, "logos") {
        for c in arr(l, "backgrounds").iter().filter_map(Value::as_str) {
            refs.push(("logo background".into(), c.to_string()));
        }
    }
    for (what, k) in refs {
        if !known(&k) {
            errors.push(format!("{what}: colour key `{k}` is not in `palette.colors`"));
        }
    }
    let manifest = font_library::read_manifest(&dirs.fonts).unwrap_or_default();
    for f in arr(doc, "fonts") {
        let what = format!("font {} {}", s(f, "family"), s(f, "style"));
        let present = manifest
            .fonts
            .iter()
            .any(|lf| lf.faces.iter().any(|x| x.family.eq_ignore_ascii_case(s(f, "family")) && x.style.eq_ignore_ascii_case(s(f, "style"))));
        if !s(f, "file").is_empty() {
            check_file(base, s(f, "file"), s(f, "sha256"), &what, &mut errors);
            if s(f, "license").trim().is_empty() {
                errors.push(format!("{what}: `license` (SPDX) is required to add the font"));
            }
        } else if !present {
            errors.push(format!("{what}: not in the Font Library and the document gives no `file`"));
        }
    }
    for l in arr(doc, "logos") {
        check_file(base, s(l, "file"), s(l, "sha256"), &format!("logo {}", s(l, "name")), &mut errors);
    }
    let approval = &doc["approval"];
    let approved = s(approval, "status") == "approved";
    if approved && (s(approval, "by").trim().is_empty() || s(approval, "reference").trim().is_empty()) {
        errors.push("`approval` with status approved needs `by` and `reference`".into());
    }
    if !approved {
        warnings.push("no ARCA approval in the document: the import is a draft that only ARCA can approve".into());
    }
    json!({"valid": errors.is_empty(), "errors": errors, "warnings": warnings, "format": FORMAT,
           "client": doc["client"], "kit": s(&doc["kit"], "name"), "approved": approved,
           "counts": {"fonts": arr(doc, "fonts").len(), "colors": keys.len(), "typography": arr(doc, "typography").len(), "logos": arr(doc, "logos").len()}})
}

fn size(v: &Value) -> Option<Size> {
    Some(Size {
        value: v.get("value")?.as_f64()?,
        unit: v.get("unit").and_then(Value::as_str).unwrap_or("px").to_string(),
        min_px: v.get("minPx").and_then(Value::as_f64),
        max_px: v.get("maxPx").and_then(Value::as_f64),
    })
}

/// Import a document (see the module docs). Returns what was created or reused.
pub fn import(dirs: &Dirs, doc: &Value, base: Option<&Path>, by: &str) -> Result<Value, String> {
    let report = validate(dirs, doc, base);
    if report["valid"] != json!(true) {
        return Err(format!("the document is not valid: {}", report["errors"]));
    }
    let by = if by.trim().is_empty() { "ARCA import".to_string() } else { by.trim().to_string() };
    let arca_client = s(&doc["client"], "id").to_string();
    let reference = format!("{} @ {}", s(&doc["arca"], "document"), s(&doc["arca"], "revision"));
    let source = KitSource { format: FORMAT.into(), authority: "ARCA".into(), reference: reference.clone(), imported: now_rfc3339() };
    // client
    let st = super::load(dirs)?;
    let client = match st.clients.iter().find(|c| c.source == format!("arca:{arca_client}")) {
        Some(c) => c.clone(),
        None => super::create_client(dirs, s(&doc["client"], "name"), "", &by, &format!("arca:{arca_client}"))?,
    };
    // fonts → Font Library
    let mut fonts = Vec::new();
    let mut added_fonts = Vec::new();
    for f in arr(doc, "fonts") {
        let find = || {
            let m = font_library::read_manifest(&dirs.fonts).ok()?;
            m.fonts.iter().find_map(|lf| {
                lf.faces
                    .iter()
                    .find(|x| x.family.eq_ignore_ascii_case(s(f, "family")) && x.style.eq_ignore_ascii_case(s(f, "style")))
                    .map(|x| (lf.clone(), x.clone()))
            })
        };
        let hit = match find() {
            Some(h) if s(f, "sha256").is_empty() || h.0.sha256 == s(f, "sha256").to_ascii_lowercase() => Some(h),
            _ => None,
        };
        let (lf, face) = match hit {
            Some(h) => h,
            None => {
                let path = doc_file(base, s(f, "file"))?;
                let lic = font_library::License { spdx: s(f, "license").into(), text: s(f, "licenseText").into(), source: format!("ARCA: {}", s(f, "source")) };
                let r = font_library::import(&dirs.fonts, &path, lic, false)?;
                added_fonts.push(r.font.id.clone());
                find().ok_or_else(|| format!("{} {} is not in {}", s(f, "family"), s(f, "style"), path.display()))?
            }
        };
        if let Some(w) = f.get("weight").and_then(Value::as_u64)
            && w != u64::from(face.weight)
        {
            return Err(format!("{} {} is weight {}, the document says {w}", face.family, face.style, face.weight));
        }
        if !fonts.iter().any(|x: &super::KitFont| x.library_font == lf.id && x.style == face.style) {
            fonts.push(super::KitFont { family: face.family, style: face.style, weight: face.weight, library_font: lf.id.clone(), sha256: lf.sha256.clone() });
        }
    }
    // palette → Palette Library (reuse the latest version when it already holds these colours)
    let pal_name = if s(&doc["palette"], "name").is_empty() { format!("{} (ARCA)", s(&doc["kit"], "name")) } else { s(&doc["palette"], "name").to_string() };
    let colors: Vec<Value> = arr(&doc["palette"], "colors")
        .iter()
        .map(|c| json!({"name": if s(c, "name").is_empty() { s(c, "key") } else { s(c, "name") }, "hex": s(c, "hex"), "role": s(c, "role"), "shadeOf": c.get("shadeOf").and_then(Value::as_str).map(|k| key_name(doc, k)), "shade": s(c, "shade")}))
        .collect();
    let rules: Vec<Value> = arr(&doc["palette"], "rules")
        .iter()
        .map(|r| {
            let mut j = json!({"kind": s(r, "kind"), "appliesTo": if s(r, "appliesTo").is_empty() { "any" } else { s(r, "appliesTo") }, "text": s(r, "text")});
            for k in ["foreground", "background"] {
                if let Some(c) = r.get(k).and_then(Value::as_str) {
                    j[k] = json!(key_name(doc, c));
                }
            }
            if let Some(a) = r.get("colors").and_then(Value::as_array) {
                j["colors"] = json!(a.iter().filter_map(Value::as_str).map(|k| key_name(doc, k)).collect::<Vec<_>>());
            }
            j
        })
        .collect();
    let lib = palette_library::load(&dirs.palettes)?;
    let existing = lib.palettes.iter().find(|p| p.name == pal_name && !p.archived).cloned();
    let pal = match existing {
        None => palette_library::create(
            &dirs.palettes,
            &json!({"name": pal_name, "description": format!("Imported from ARCA: {reference}"), "colors": colors, "rules": rules, "by": by, "note": reference}),
        )?,
        Some(p) => {
            // keep the ids of colours and rules that are still there, so an unchanged document
            // makes no new palette version
            let latest = p.latest().cloned();
            let base_v = latest.as_ref().map_or(0, |v| v.version);
            let mut colors = colors;
            let mut rules = rules;
            if let Some(v) = &latest {
                for c in &mut colors {
                    if let Some(old) = v.colors.iter().find(|o| Some(o.name.as_str()) == c["name"].as_str()) {
                        c["id"] = json!(old.id);
                    }
                }
                let name_of = |id: &Option<String>| id.as_ref().and_then(|i| v.colors.iter().find(|c| &c.id == i)).map(|c| c.name.clone());
                for r in &mut rules {
                    let same = v.rules.iter().find(|o| {
                        serde_json::to_value(o.kind).ok().as_ref().and_then(Value::as_str) == r["kind"].as_str()
                            && Some(o.text.as_str()) == r["text"].as_str()
                            && name_of(&o.foreground).as_deref() == r["foreground"].as_str()
                            && name_of(&o.background).as_deref() == r["background"].as_str()
                    });
                    if let Some(o) = same {
                        r["id"] = json!(o.id);
                    }
                }
            }
            palette_library::update(&dirs.palettes, &json!({"id": p.id, "baseVersion": base_v, "colors": colors, "rules": rules, "by": by, "note": reference}))?
                .palette
        }
    };
    let pv = pal.latest().cloned().ok_or("the palette has no version")?;
    let cref = |key: &str| -> Result<ColorRef, String> {
        let name = key_name(doc, key);
        let c = pv.color(&name).ok_or_else(|| format!("colour `{key}` missing from the imported palette"))?;
        Ok(ColorRef { palette: pal.id.clone(), color: c.id.clone() })
    };
    let ocref = |v: &Value, k: &str| -> Result<Option<ColorRef>, String> { v.get(k).and_then(Value::as_str).map(cref).transpose() };
    // logos → client assets
    let mut logos = Vec::new();
    for l in arr(doc, "logos") {
        let path = doc_file(base, s(l, "file"))?;
        let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let sha = sha256_hex(&bytes);
        let st = super::load(dirs)?;
        let asset = match st.assets.iter().find(|a| a.client == client.id && a.sha256 == sha) {
            Some(a) => a.clone(),
            None => super::import_asset(
                dirs,
                &AssetSpec {
                    client: &client.id,
                    path: &path,
                    name: s(l, "name"),
                    kind: "logo",
                    variant: s(l, "variant"),
                    source: &format!("ARCA: {reference}"),
                    license: s(l, "license"),
                    by: &by,
                },
            )?,
        };
        logos.push(LogoUse {
            asset: asset.id.clone(),
            sha256: asset.sha256.clone(),
            variant: s(l, "variant").to_string(),
            approved: l.get("approved").and_then(Value::as_bool).unwrap_or(true),
            placement: s(l, "placement").to_string(),
            margin_pct: l.get("marginPct").and_then(Value::as_f64),
            min_width: l.get("minWidth").and_then(size),
            max_width: l.get("maxWidth").and_then(size),
            backgrounds: arr(l, "backgrounds").iter().filter_map(Value::as_str).map(cref).collect::<Result<_, _>>()?,
            forbidden: arr(l, "forbidden").iter().filter_map(Value::as_str).map(str::to_string).collect(),
            notes: s(l, "notes").to_string(),
        });
    }
    let mut typography = Vec::new();
    for t in arr(doc, "typography") {
        typography.push(TypeRole {
            role: s(t, "role").to_string(),
            family: s(t, "family").to_string(),
            style: s(t, "style").to_string(),
            weight: t.get("weight").and_then(Value::as_u64).and_then(|w| u16::try_from(w).ok()).unwrap_or(400),
            size: t.get("size").and_then(size).ok_or_else(|| format!("typography {}: `size` needs value and unit", s(t, "role")))?,
            color: ocref(t, "color")?,
            background: ocref(t, "background")?,
            accent: ocref(t, "accent")?,
            align: if s(t, "align").is_empty() { "left".into() } else { s(t, "align").into() },
            placement: s(t, "placement").to_string(),
            case: if s(t, "case").is_empty() { "none".into() } else { s(t, "case").into() },
            tracking: t.get("tracking").and_then(Value::as_f64).unwrap_or(0.0),
            line_height: t.get("lineHeight").and_then(Value::as_f64),
            max_lines: t.get("maxLines").and_then(Value::as_u64).and_then(|x| u32::try_from(x).ok()),
            opacity: t.get("opacity").and_then(Value::as_f64),
            styling: s(t, "styling").to_string(),
        });
    }
    let colors_roles = arr(doc, "colors")
        .iter()
        .map(|c| Ok(super::ColorRole { role: s(c, "role").into(), color: cref(s(c, "color"))?, notes: s(c, "notes").into() }))
        .collect::<Result<Vec<_>, String>>()?;
    let kit_rules = arr(doc, "rules")
        .iter()
        .enumerate()
        .map(|(i, r)| {
            Ok(super::KitRule {
                // stable across imports of the same document
                id: format!("krule_{}", sha256_hex(format!("{i}\u{1}{r}").as_bytes()).get(..12).unwrap_or("0")),
                kind: s(r, "kind").into(),
                foreground: ocref(r, "foreground")?,
                background: ocref(r, "background")?,
                applies_to: if s(r, "appliesTo").is_empty() { "any".into() } else { s(r, "appliesTo").into() },
                text: s(r, "text").into(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let safe_area = doc.get("safeArea").filter(|v| v.is_object()).map(|v| SafeArea {
        top: v.get("top").and_then(Value::as_f64).unwrap_or(0.0),
        right: v.get("right").and_then(Value::as_f64).unwrap_or(0.0),
        bottom: v.get("bottom").and_then(Value::as_f64).unwrap_or(0.0),
        left: v.get("left").and_then(Value::as_f64).unwrap_or(0.0),
        unit: if s(v, "unit").is_empty() { "%w".into() } else { s(v, "unit").into() },
    });
    let reference_frame = doc
        .get("referenceFrame")
        .and_then(Value::as_array)
        .and_then(|a| Some([u32::try_from(a.first()?.as_u64()?).ok()?, u32::try_from(a.get(1)?.as_u64()?).ok()?]));
    let content = KitContent {
        fonts,
        palettes: vec![super::KitPalette { palette: pal.id.clone(), version: pv.version, sha256: pv.sha256.clone(), name: pal.name.clone() }],
        colors: colors_roles,
        typography,
        logos,
        rules: kit_rules,
        safe_area,
        reference_frame,
        notes: s(&doc["kit"], "notes").to_string(),
    };
    // kit: the ARCA kit of this client with this name, or a new one
    let kit_name = s(&doc["kit"], "name");
    let st = super::load(dirs)?;
    let existing = st.kits.iter().find(|k| k.client == client.id && k.source.is_some() && k.name.eq_ignore_ascii_case(kit_name)).cloned();
    let (kit_id, version) = match existing {
        None => {
            let k = super::create_kit(dirs, &client.id, kit_name, s(&doc["kit"], "description"), &by, content, Some(source.clone()))?;
            (k.id, 1)
        }
        Some(k) => {
            let same = k.latest().is_some_and(|v| v.content == content);
            if same { (k.id.clone(), k.latest().map_or(1, |v| v.version)) } else { append_version(dirs, &k.id, content, source.clone(), &by)? }
        }
    };
    let approval = &doc["approval"];
    let mut status = "draft".to_string();
    if s(approval, "status") == "approved" {
        let st = super::load(dirs)?;
        let already = st.kit(&kit_id).and_then(|k| k.version(version)).is_some_and(KitVersion::approved);
        if !already {
            super::approve(dirs, &kit_id, version, s(approval, "by"), &format!("{} ({})", s(approval, "reference"), s(approval, "date")), "ARCA")?;
        }
        status = "approved".into();
    }
    let st = super::load(dirs)?;
    let issues = st.kit(&kit_id).and_then(|k| k.version(version).map(|v| super::check(dirs, &st, k, v))).unwrap_or_default();
    Ok(json!({"client": client.id, "clientName": client.name, "kit": kit_id, "version": version, "ref": format!("{kit_id}@{version}"), "status": status,
              "palette": format!("{}@{}", pal.id, pv.version), "fontsAdded": added_fonts, "issues": issues, "warnings": report["warnings"]}))
}

/// The palette colour name of a document colour key.
fn key_name(doc: &Value, key: &str) -> String {
    let names: BTreeMap<&str, &str> =
        arr(&doc["palette"], "colors").iter().map(|c| (s(c, "key"), if s(c, "name").is_empty() { s(c, "key") } else { s(c, "name") })).collect();
    names.get(key).map_or_else(|| key.to_string(), |n| (*n).to_string())
}

/// A new version of an ARCA-managed kit with imported content (a draft until ARCA's approval).
fn append_version(dirs: &Dirs, id: &str, content: KitContent, source: KitSource, by: &str) -> Result<(String, u32), String> {
    super::modify(dirs, |st| {
        let k = st.kits.iter_mut().find(|k| k.id == id).ok_or_else(|| format!("no brand kit `{id}`"))?;
        // an unapproved imported draft is replaced by the newer import; approved versions stay
        if k.latest().is_some_and(|v| !v.approved()) {
            k.versions.pop();
        }
        let version = k.versions.last().map_or(1, |v| v.version.saturating_add(1));
        let now = now_rfc3339();
        let based_on = k.latest_approved().map(|v| v.version);
        let sha256 = content.fingerprint();
        k.versions.push(KitVersion {
            version,
            status: "draft".into(),
            created: now.clone(),
            updated: now,
            by: by.to_string(),
            note: format!("imported from ARCA: {}", source.reference),
            based_on,
            content,
            sha256,
            approval: None,
            source: Some(source.clone()),
        });
        k.source = Some(source);
        super::log(&mut k.history, &mut k.updated, by, "imported", format!("version {version} from ARCA"));
        Ok((k.id.clone(), version))
    })
}
