//! `brand.*` commands: the agent interface to clients, brand assets, brand kits and project
//! pinning. The Brand Kits window uses the same commands.

use std::path::Path;

use filmcraft_project::BrandPin;
use serde_json::{Value, json};

use super::{AssetSpec, Dirs, KitContent, Store, arca};
use crate::commands::{CommandSpec, always, bad, bool_p, str_p, u64_p};
use crate::user_library::now_rfc3339;
use crate::{Result, Session, font_library, palette_library};

fn dirs(s: &Session, cmd: &str) -> Result<Dirs> {
    Dirs::of(s).ok_or_else(|| bad(cmd, "no data directory for brand kits (set FILMCRAFT_DATA_DIR)"))
}

fn store(s: &Session, cmd: &str) -> Result<(Dirs, Store)> {
    let d = dirs(s, cmd)?;
    let st = super::load(&d).map_err(|e| bad(cmd, e))?;
    Ok((d, st))
}

fn need<'a>(p: &'a Value, k: &str, cmd: &str) -> Result<&'a str> {
    str_p(p, k).ok_or_else(|| bad(cmd, format!("need `{k}`")))
}

fn by(p: &Value) -> &str {
    str_p(p, "by").unwrap_or("")
}

fn version_p(p: &Value) -> Option<u32> {
    u64_p(p, "version").and_then(|v| u32::try_from(v).ok())
}

/// `kit_…@N` or `kit_…` (+ `version`).
fn kit_ref(p: &Value, key: &str) -> Option<(String, Option<u32>)> {
    let r = str_p(p, key)?;
    Some(match r.split_once('@') {
        Some((k, v)) => (k.to_string(), v.parse().ok()),
        None => (r.to_string(), version_p(p)),
    })
}

// ------------------------------------------------------------------ content normalisation

/// Fill in exact references from short ones, so agents and the interface can name fonts by
/// family and style, palettes by id and version, logos by asset id: fonts get their Font Library
/// id, file SHA-256 and weight; palettes their fingerprint and name; logos their SHA-256. A name
/// that does not match exactly is an error, never a guess.
pub fn normalize(dirs: &Dirs, st: &Store, content: &mut Value) -> std::result::Result<(), String> {
    if let Some(fonts) = content.get_mut("fonts").and_then(Value::as_array_mut) {
        let manifest = font_library::read_manifest(&dirs.fonts)?;
        for f in fonts.iter_mut() {
            if f.get("libraryFont").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) {
                continue;
            }
            let fam = f.get("family").and_then(Value::as_str).unwrap_or("").to_string();
            let sty = f.get("style").and_then(Value::as_str).unwrap_or("Regular").to_string();
            let hit = manifest
                .fonts
                .iter()
                .find_map(|lf| lf.faces.iter().find(|x| x.family.eq_ignore_ascii_case(&fam) && x.style.eq_ignore_ascii_case(&sty)).map(|x| (lf, x)));
            let Some((lf, face)) = hit else {
                let styles: Vec<String> =
                    manifest.fonts.iter().flat_map(|lf| lf.faces.iter().filter(|x| x.family.eq_ignore_ascii_case(&fam)).map(|x| x.style.clone())).collect();
                return Err(if styles.is_empty() {
                    format!("font {fam} is not in the Font Library (add it there first)")
                } else {
                    format!("{fam} has no style \"{sty}\" in the Font Library (it has {})", styles.join(", "))
                });
            };
            if let Some(w) = f.get("weight").and_then(Value::as_u64)
                && w != u64::from(face.weight)
            {
                return Err(format!("{fam} {sty} is weight {}, not {w}", face.weight));
            }
            *f = json!({"family": face.family, "style": face.style, "weight": face.weight, "libraryFont": lf.id, "sha256": lf.sha256});
        }
    }
    if let Some(pals) = content.get_mut("palettes").and_then(Value::as_array_mut) {
        let lib = palette_library::load(&dirs.palettes)?;
        for p in pals.iter_mut() {
            let id = p.get("palette").and_then(Value::as_str).unwrap_or("").to_string();
            let pal = lib.get(&id).ok_or_else(|| format!("no palette `{id}` in the Palette Library"))?;
            let v = match p.get("version").and_then(Value::as_u64) {
                Some(n) => pal.version(u32::try_from(n).unwrap_or(u32::MAX)).ok_or_else(|| format!("palette \"{}\" has no version {n}", pal.name))?,
                None => return Err(format!("palette \"{}\": name the exact version (`version`)", pal.name)),
            };
            if let Some(sha) = p.get("sha256").and_then(Value::as_str).filter(|s| !s.is_empty())
                && sha != v.sha256
            {
                return Err(format!("palette \"{}\" version {} does not match the fingerprint given", pal.name, v.version));
            }
            *p = json!({"palette": pal.id, "version": v.version, "sha256": v.sha256, "name": pal.name});
        }
    }
    if let Some(logos) = content.get_mut("logos").and_then(Value::as_array_mut) {
        for l in logos.iter_mut() {
            let id = l.get("asset").and_then(Value::as_str).unwrap_or("").to_string();
            let a = st.asset(&id).ok_or_else(|| format!("no brand asset `{id}`"))?;
            if l.get("sha256").and_then(Value::as_str).is_none_or(str::is_empty) {
                l["sha256"] = json!(a.sha256);
            }
            if l.get("variant").and_then(Value::as_str).is_none_or(str::is_empty) && !a.variant.is_empty() {
                l["variant"] = json!(a.variant);
            }
        }
    }
    if let Some(rules) = content.get_mut("rules").and_then(Value::as_array_mut) {
        for r in rules.iter_mut() {
            if r.get("id").and_then(Value::as_str).is_none_or(str::is_empty) {
                r["id"] = json!(crate::user_library::new_id("krule", "kit"));
            }
        }
    }
    Ok(())
}

/// Content for a save: `content` merged over `base` by top-level key, normalised and parsed.
fn content_from(dirs: &Dirs, st: &Store, base: &KitContent, given: Option<&Value>) -> std::result::Result<KitContent, String> {
    let mut v = serde_json::to_value(base).map_err(|e| e.to_string())?;
    if let Some(Value::Object(o)) = given {
        for (k, x) in o {
            v[k] = x.clone();
        }
    } else if given.is_some() {
        return Err("`content` must be an object".into());
    }
    normalize(dirs, st, &mut v)?;
    serde_json::from_value(v).map_err(|e| format!("invalid kit content: {e}"))
}

// ------------------------------------------------------------------ clients and assets

fn client_json(st: &Store, c: &super::Client) -> Value {
    json!({"id": c.id, "name": c.name, "notes": c.notes, "archived": c.archived, "source": c.source, "created": c.created, "updated": c.updated,
           "kits": st.kits.iter().filter(|k| k.client == c.id).map(|k| super::kit_summary(st, k)).collect::<Vec<_>>(),
           "assets": st.assets.iter().filter(|a| a.client == c.id).count()})
}

fn clients_list(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.clients.list";
    let (d, st) = store(s, cmd)?;
    let q = str_p(p, "query").unwrap_or("").trim().to_lowercase();
    let archived = bool_p(p, "includeArchived").unwrap_or(false);
    let list: Vec<Value> = st
        .clients
        .iter()
        .filter(|c| (archived || !c.archived) && (q.is_empty() || c.name.to_lowercase().contains(&q) || c.id == q))
        .map(|c| client_json(&st, c))
        .collect();
    Ok(json!({"dir": d.brand.display().to_string(), "clients": list}))
}

fn clients_get(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.clients.get";
    let (_, st) = store(s, cmd)?;
    let id = need(p, "id", cmd)?;
    let c = st.client(id).ok_or_else(|| bad(cmd, format!("no client `{id}`")))?;
    let mut out = client_json(&st, c);
    out["assets"] = json!(st.assets.iter().filter(|a| a.client == c.id).collect::<Vec<_>>());
    out["history"] = json!(c.history);
    Ok(out)
}

fn clients_create(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.clients.create";
    let d = dirs(s, cmd)?;
    let c = super::create_client(&d, need(p, "name", cmd)?, str_p(p, "notes").unwrap_or(""), by(p), "filmcraft").map_err(|e| bad(cmd, e))?;
    Ok(json!(c))
}

fn clients_update(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.clients.update";
    let d = dirs(s, cmd)?;
    let c = super::update_client(&d, need(p, "id", cmd)?, str_p(p, "name"), str_p(p, "notes"), bool_p(p, "archived"), by(p)).map_err(|e| bad(cmd, e))?;
    Ok(json!(c))
}

fn assets_import(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.assets.import";
    let d = dirs(s, cmd)?;
    let spec = AssetSpec {
        client: need(p, "client", cmd)?,
        path: Path::new(need(p, "path", cmd)?),
        name: str_p(p, "name").unwrap_or(""),
        kind: str_p(p, "kind").unwrap_or("logo"),
        variant: str_p(p, "variant").unwrap_or(""),
        source: str_p(p, "source").unwrap_or(""),
        license: str_p(p, "license").unwrap_or(""),
        by: by(p),
    };
    let a = super::import_asset(&d, &spec).map_err(|e| bad(cmd, e))?;
    Ok(json!(a))
}

fn assets_list(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.assets.list";
    let (d, st) = store(s, cmd)?;
    let client = need(p, "client", cmd)?;
    let list: Vec<Value> = st
        .assets
        .iter()
        .filter(|a| a.client == client)
        .map(|a| {
            let mut j = json!(a);
            j["path"] = json!(d.asset_path(a).display().to_string());
            j["usedBy"] = json!(
                super::asset_users(&st, &a.id).into_iter().map(|(k, n, v, ap)| json!({"kit": k, "name": n, "version": v, "approved": ap})).collect::<Vec<_>>()
            );
            j
        })
        .collect();
    Ok(json!({"assets": list}))
}

fn assets_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.assets.remove";
    let d = dirs(s, cmd)?;
    let a = super::remove_asset(&d, need(p, "id", cmd)?).map_err(|e| bad(cmd, e))?;
    Ok(json!({"removed": a.id, "name": a.name}))
}

// ------------------------------------------------------------------ kits

fn kits_list(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.kits.list";
    let (_, st) = store(s, cmd)?;
    let client = str_p(p, "client");
    let q = str_p(p, "query").unwrap_or("").trim().to_lowercase();
    let archived = bool_p(p, "includeArchived").unwrap_or(false);
    let list: Vec<Value> = st
        .kits
        .iter()
        .filter(|k| client.is_none_or(|c| k.client == c) && (archived || !k.archived) && (q.is_empty() || k.name.to_lowercase().contains(&q)))
        .map(|k| super::kit_summary(&st, k))
        .collect();
    Ok(json!({"kits": list}))
}

fn pick_version<'a>(k: &'a super::BrandKit, v: Option<u32>, approved: bool, cmd: &str) -> Result<&'a super::KitVersion> {
    match (v, approved) {
        (Some(n), _) => k.version(n).ok_or_else(|| bad(cmd, format!("kit \"{}\" has no version {n}", k.name))),
        (None, true) => k.latest_approved().ok_or_else(|| bad(cmd, format!("kit \"{}\" has no approved version", k.name))),
        (None, false) => k.latest().ok_or_else(|| bad(cmd, "the kit has no versions")),
    }
}

fn kits_get(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.kits.get";
    font_library::ensure_loaded(s);
    let (d, st) = store(s, cmd)?;
    let (id, v) = kit_ref(p, "id").ok_or_else(|| bad(cmd, "need `id` (kit_… or kit_…@version)"))?;
    let k = st.kit(&id).ok_or_else(|| bad(cmd, format!("no brand kit `{id}`")))?;
    let ver = pick_version(k, v, bool_p(p, "approved").unwrap_or(false), cmd)?;
    let mut out = super::kit_summary(&st, k);
    out["resolved"] = super::resolved_json(&d, &st, k, ver);
    out["content"] = json!(ver.content);
    if bool_p(p, "history").unwrap_or(false) {
        out["history"] = json!(k.history);
    }
    Ok(out)
}

fn kits_create(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.kits.create";
    let (d, st) = store(s, cmd)?;
    let content = content_from(&d, &st, &KitContent::default(), p.get("content")).map_err(|e| bad(cmd, e))?;
    let k = super::create_kit(&d, need(p, "client", cmd)?, need(p, "name", cmd)?, str_p(p, "description").unwrap_or(""), by(p), content, None)
        .map_err(|e| bad(cmd, e))?;
    Ok(super::kit_summary(&super::load(&d).map_err(|e| bad(cmd, e))?, &k))
}

fn kits_save(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.kits.saveDraft";
    font_library::ensure_loaded(s);
    let (d, st) = store(s, cmd)?;
    let id = need(p, "id", cmd)?;
    let base =
        u64_p(p, "baseVersion").and_then(|v| u32::try_from(v).ok()).ok_or_else(|| bad(cmd, "need `baseVersion`: the version the changes were made to"))?;
    let k = st.kit(id).ok_or_else(|| bad(cmd, format!("no brand kit `{id}`")))?;
    let cur = k.version(base).ok_or_else(|| bad(cmd, format!("the kit has no version {base}")))?;
    let content = content_from(&d, &st, &cur.content, p.get("content")).map_err(|e| bad(cmd, e))?;
    let saved = super::save_draft(&d, id, base, content, by(p), str_p(p, "note").unwrap_or("")).map_err(|e| bad(cmd, e))?;
    let st = super::load(&d).map_err(|e| bad(cmd, e))?;
    let mut out = super::kit_summary(&st, &saved.kit);
    out["savedVersion"] = json!(saved.version);
    out["newVersion"] = json!(saved.new_version);
    if let Some(v) = saved.kit.version(saved.version) {
        out["issues"] = json!(super::check(&d, &st, &saved.kit, v));
    }
    Ok(out)
}

fn kits_approve(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.kits.approve";
    font_library::ensure_loaded(s);
    let d = dirs(s, cmd)?;
    let (id, v) = kit_ref(p, "id").ok_or_else(|| bad(cmd, "need `id`"))?;
    let v = v.ok_or_else(|| bad(cmd, "name the exact version to approve (`version`)"))?;
    if !bool_p(p, "confirm").unwrap_or(false) {
        return Err(bad(cmd, "approval freezes this version for every project that uses it; pass confirm: true with the approval record"));
    }
    let a = p.get("approval").ok_or_else(|| bad(cmd, "need `approval`: {\"by\": who approved it, \"reference\": the decision that approves it}"))?;
    let k =
        super::approve(&d, &id, v, a.get("by").and_then(Value::as_str).unwrap_or(""), a.get("reference").and_then(Value::as_str).unwrap_or(""), "FilmCraft")
            .map_err(|e| bad(cmd, e))?;
    let st = super::load(&d).map_err(|e| bad(cmd, e))?;
    Ok(super::kit_summary(&st, &k))
}

fn kits_update(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.kits.update";
    let d = dirs(s, cmd)?;
    let k =
        super::update_kit_info(&d, need(p, "id", cmd)?, str_p(p, "name"), str_p(p, "description"), bool_p(p, "archived"), by(p)).map_err(|e| bad(cmd, e))?;
    let st = super::load(&d).map_err(|e| bad(cmd, e))?;
    Ok(super::kit_summary(&st, &k))
}

fn kits_check(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.kits.check";
    font_library::ensure_loaded(s);
    let (d, st) = store(s, cmd)?;
    let (id, v) = kit_ref(p, "id").ok_or_else(|| bad(cmd, "need `id`"))?;
    let k = st.kit(&id).ok_or_else(|| bad(cmd, format!("no brand kit `{id}`")))?;
    let ver = pick_version(k, v, false, cmd)?;
    let issues = super::check(&d, &st, k, ver);
    Ok(json!({"kit": k.id, "version": ver.version, "status": ver.status, "ready": !super::has_errors(&issues), "issues": issues}))
}

/// `brand.resolve`: one reference of a kit version, exactly.
fn resolve(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.resolve";
    font_library::ensure_loaded(s);
    let (d, st) = store(s, cmd)?;
    let (id, v) = kit_ref(p, "kit").ok_or_else(|| bad(cmd, "need `kit` (kit_…@version)"))?;
    let k = st.kit(&id).ok_or_else(|| bad(cmd, format!("no brand kit `{id}`")))?;
    let ver = pick_version(k, v, true, cmd)?;
    let full = super::resolved_json(&d, &st, k, ver);
    let what = need(p, "what", cmd)?;
    let (area, name) = what
        .split_once(':')
        .ok_or_else(|| bad(cmd, "`what` is typography:<role>, color:<role>, logo:<variant or asset id>, font:<family> <style> or rules"))?;
    let name = name.trim();
    let pick =
        |key: &str, field: &str| full[key].as_array().and_then(|a| a.iter().find(|x| x[field].as_str().is_some_and(|v| v.eq_ignore_ascii_case(name))).cloned());
    let hit = match area {
        "typography" => pick("typography", "role"),
        "color" => pick("colors", "role"),
        "logo" => pick("logos", "variant").or_else(|| pick("logos", "asset")),
        "font" => full["fonts"].as_array().and_then(|a| {
            a.iter().find(|f| format!("{} {}", f["family"].as_str().unwrap_or(""), f["style"].as_str().unwrap_or("")).eq_ignore_ascii_case(name)).cloned()
        }),
        "rules" => Some(
            json!({"kit": full["rules"], "palettes": full["palettes"].as_array().map(|a| a.iter().map(|p| json!({"ref": p["ref"], "rules": p["rules"]})).collect::<Vec<_>>())}),
        ),
        _ => None,
    };
    let hit = hit.ok_or_else(|| bad(cmd, format!("kit {}@{} has no {what}", k.id, ver.version)))?;
    Ok(json!({"ref": format!("{}@{}", k.id, ver.version), "status": ver.status, "what": what, "value": hit}))
}

// ------------------------------------------------------------------ project pinning

fn pin_json(s: &Session, d: &Dirs) -> Value {
    let Some(pin) = s.project.brand.clone() else { return json!({"pinned": false}) };
    let st = super::load(d).ok();
    let kit = st.as_ref().and_then(|st| st.kit(&pin.kit));
    let v = kit.and_then(|k| k.version(pin.version));
    let state = match (kit, v) {
        (None, _) => "missing: the kit is not in this machine's brand kits",
        (Some(_), None) => "missing: the pinned version is not in the kit",
        (Some(_), Some(v)) if !v.approved() || v.sha256 != pin.sha256 || !v.intact() => {
            "changed: the pinned version does not match the fingerprint recorded in the project"
        }
        _ => "ok",
    };
    let newer = kit.and_then(|k| k.latest_approved()).filter(|l| l.version > pin.version).map(|l| l.version);
    let issues = match (st.as_ref(), kit, v) {
        (Some(st), Some(k), Some(v)) => json!(super::check(d, st, k, v)),
        _ => json!([]),
    };
    json!({"pinned": true, "pin": pin, "state": state, "newerApprovedVersion": newer, "issues": issues})
}

fn project_get(s: &mut Session, _p: &Value) -> Result<Value> {
    let cmd = "brand.project.get";
    font_library::ensure_loaded(s);
    let d = dirs(s, cmd)?;
    Ok(pin_json(s, &d))
}

fn project_pin(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.project.pin";
    let (_, st) = store(s, cmd)?;
    let (id, v) = kit_ref(p, "kit").ok_or_else(|| bad(cmd, "need `kit` (kit_…@version)"))?;
    let k = st.kit(&id).ok_or_else(|| bad(cmd, format!("no brand kit `{id}`")))?;
    let ver = pick_version(k, v, true, cmd)?;
    if !ver.approved() {
        return Err(bad(cmd, format!("version {} of \"{}\" is a draft; only approved versions can be pinned to a project", ver.version, k.name)));
    }
    if !ver.intact() {
        return Err(bad(cmd, "that version does not match its fingerprint"));
    }
    let client = st.client(&k.client).ok_or_else(|| bad(cmd, "the kit's client is missing"))?;
    let who = by(p).trim();
    let who = if who.is_empty() { "unspecified" } else { who };
    if let Some(cur) = &s.project.brand {
        if cur.kit == k.id && cur.version == ver.version {
            return Ok(json!({"pin": cur, "changed": false}));
        }
        if cur.client != k.client && !bool_p(p, "replaceClient").unwrap_or(false) {
            return Err(bad(
                cmd,
                format!(
                    "the project uses {}'s brand ({} v{}); {} belongs to {}. Pass replaceClient: true (with authorizeUpdate) only when the project really changes client",
                    cur.client_name, cur.kit_name, cur.version, k.name, client.name
                ),
            ));
        }
        if !bool_p(p, "authorizeUpdate").unwrap_or(false) {
            return Err(bad(
                cmd,
                format!(
                    "the project is pinned to {} v{}; moving it to {} v{} changes its branding: pass authorizeUpdate: true when that is authorised",
                    cur.kit_name, cur.version, k.name, ver.version
                ),
            ));
        }
    }
    let pin = BrandPin {
        client: k.client.clone(),
        client_name: client.name.clone(),
        kit: k.id.clone(),
        kit_name: k.name.clone(),
        version: ver.version,
        sha256: ver.sha256.clone(),
        pinned_at: now_rfc3339(),
        pinned_by: who.to_string(),
    };
    let previous = s.project.brand.clone();
    let p2 = pin.clone();
    s.edit("Pin Brand Kit", move |project, _| {
        project.brand = Some(p2);
        Ok(())
    })?;
    s.autoedit_log.push(json!({"kind": "brandKitPinned", "pin": pin, "previous": previous}));
    Ok(json!({"pin": pin, "changed": true, "previous": previous}))
}

fn project_unpin(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.project.unpin";
    let Some(cur) = s.project.brand.clone() else { return Ok(json!({"pinned": false})) };
    if !bool_p(p, "authorizeUpdate").unwrap_or(false) {
        return Err(bad(cmd, format!("the project uses {} v{}; pass authorizeUpdate: true to remove its brand kit", cur.kit_name, cur.version)));
    }
    s.edit("Unpin Brand Kit", |project, _| {
        project.brand = None;
        Ok(())
    })?;
    s.autoedit_log.push(json!({"kind": "brandKitUnpinned", "previous": cur, "by": by(p)}));
    Ok(json!({"pinned": false, "previous": cur}))
}

// ------------------------------------------------------------------ ARCA

fn arca_doc(p: &Value, cmd: &str) -> Result<Value> {
    if let Some(doc) = p.get("document") {
        return Ok(doc.clone());
    }
    let path = need(p, "path", cmd)?;
    let meta = std::fs::metadata(path).map_err(|e| bad(cmd, format!("cannot read {path}: {e}")))?;
    if meta.len() > 8 * 1024 * 1024 {
        return Err(bad(cmd, "the document is larger than 8 MiB"));
    }
    let b = std::fs::read(path).map_err(|e| bad(cmd, format!("cannot read {path}: {e}")))?;
    serde_json::from_slice(&b).map_err(|e| bad(cmd, format!("{path} is not JSON: {e}")))
}

fn arca_validate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.arca.validate";
    let doc = arca_doc(p, cmd)?;
    let base = str_p(p, "path").and_then(|x| Path::new(x).parent().map(Path::to_path_buf));
    let d = dirs(s, cmd)?;
    Ok(arca::validate(&d, &doc, base.as_deref()))
}

fn arca_import(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brand.arca.import";
    font_library::ensure_loaded(s);
    let doc = arca_doc(p, cmd)?;
    let base = str_p(p, "path").and_then(|x| Path::new(x).parent().map(Path::to_path_buf));
    let d = dirs(s, cmd)?;
    arca::import(&d, &doc, base.as_deref(), by(p)).map_err(|e| bad(cmd, e))
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    let q = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    vec![
        q("brand.clients.list", "List Clients", r#"{"query":string?,"includeArchived":bool=false}"#, clients_list),
        q("brand.clients.get", "Get Client", r#"{"id":cli_…}"#, clients_get),
        q("brand.clients.create", "Create Client", r#"{"name":string,"notes":string?,"by":who?}"#, clients_create),
        q("brand.clients.update", "Update Client", r#"{"id":cli_…,"name":string?,"notes":string?,"archived":bool?,"by":who?}"#, clients_update),
        q(
            "brand.assets.import",
            "Import Brand Asset",
            r#"{"client":cli_…,"path":PNG|JPEG|WebP file,"name":string?,"kind":"logo|graphic"=logo,"variant":string? (primary, white, icon…),"source":string?,"license":string?,"by":who?}"#,
            assets_import,
        ),
        q("brand.assets.list", "List Brand Assets", r#"{"client":cli_…}"#, assets_list),
        q("brand.assets.remove", "Remove Brand Asset", r#"{"id":ast_…} — refused while a kit version uses it"#, assets_remove),
        q("brand.kits.list", "List Brand Kits", r#"{"client":cli_…?,"query":string?,"includeArchived":bool=false}"#, kits_list),
        q(
            "brand.kits.get",
            "Get Brand Kit",
            r#"{"id":kit_… | kit_…@version,"version":n?,"approved":bool=false (the latest approved version),"history":bool=false}"#,
            kits_get,
        ),
        q(
            "brand.kits.create",
            "Create Brand Kit",
            r#"{"client":cli_…,"name":string,"description":string?,"content":{fonts:[{family,style}],palettes:[{palette,version}],colors:[{role,color:{palette,color}}],typography:[…],logos:[{asset,…}],rules:[…],safeArea:{…},referenceFrame:[w,h]}?,"by":who?} — draft version 1"#,
            kits_create,
        ),
        q(
            "brand.kits.saveDraft",
            "Save Brand Kit Draft",
            r#"{"id":kit_…,"baseVersion":n,"content":{…top-level keys to replace},"by":who?,"note":string?} — replaces the draft, or starts a new draft from an approved version; approved versions never change"#,
            kits_save,
        ),
        q(
            "brand.kits.approve",
            "Approve Brand Kit Version",
            r#"{"id":kit_…,"version":n,"approval":{"by":the person approving,"reference":their decision},"confirm":true} — only with explicit authorisation"#,
            kits_approve,
        ),
        q("brand.kits.update", "Rename or Archive Brand Kit", r#"{"id":kit_…,"name":string?,"description":string?,"archived":bool?,"by":who?}"#, kits_update),
        q(
            "brand.kits.check",
            "Check Brand Kit",
            r#"{"id":kit_…,"version":n?} — missing fonts and weights, palette versions, assets, rule conflicts"#,
            kits_check,
        ),
        q(
            "brand.resolve",
            "Resolve Brand Reference",
            r#"{"kit":kit_…@version (default latest approved),"what":"typography:<role>|color:<role>|logo:<variant or asset>|font:<family> <style>|rules:"}"#,
            resolve,
        ),
        q("brand.project.get", "Project Brand Kit", "{}", project_get),
        q(
            "brand.project.pin",
            "Pin Brand Kit to Project",
            r#"{"kit":kit_…@version (an approved version; default the latest approved),"by":who?,"authorizeUpdate":bool (needed to move an existing pin),"replaceClient":bool (needed to switch to another client's kit)}"#,
            project_pin,
        ),
        q("brand.project.unpin", "Unpin Brand Kit", r#"{"authorizeUpdate":true,"by":who?}"#, project_unpin),
        q("brand.arca.validate", "Validate ARCA Brand Document", r#"{"path":arca.brand/0.1 JSON | "document":{…}}"#, arca_validate),
        q("brand.arca.import", "Import ARCA Brand Document", r#"{"path":arca.brand/0.1 JSON | "document":{…},"by":who?}"#, arca_import),
    ]
}
