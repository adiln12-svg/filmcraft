//! The persistent font library: fonts a person adds (TrueType, OpenType, collections, WOFF,
//! WOFF2), kept in `<data dir>/Font Library/` with a manifest, registered with the text engine
//! at startup so titles, captions and exports render with them after a restart.
//!
//! Every font carries the licence it was added under (an SPDX identifier, plus the licence text
//! or URL when known); FilmCraft suggests the licence the font file states but never invents
//! one. Variable fonts register one face per named instance (Archivo Thin … Black).
//!
//! The library never substitutes: [`sequence_fonts`] and [`font_report`] say, for every family
//! and style a sequence uses, whether it renders exactly, with another style of the family
//! (substituted) or in another typeface (missing). Export and `qc.render` refuse substituted and
//! missing fonts unless the caller explicitly allows it (see `docs/font-library.md`).
//!
//! Commands: `fonts.library.list`, `fonts.library.inspect`, `fonts.library.import`,
//! `fonts.library.remove`, `fonts.library.setLicense`, `fonts.library.preview`,
//! `fonts.missing`. The Font Library window (Window ▸ Font Library) drives the same functions.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use filmcraft_project::{ItemId, ItemKind, ParamValue, Project, graphic};
use filmcraft_text::fonts::{self, FaceId, FontStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, str_p, u64_p};
use crate::user_library::{now_rfc3339, read_json, sha256_hex, write_atomic, write_json};
use crate::{Result, Session};

/// Folder of the library inside the data directory.
pub const DIR_NAME: &str = "Font Library";
const MANIFEST: &str = "library.json";
const MANIFEST_VERSION: u32 = 1;
/// Most fonts a library holds.
pub const MAX_FONTS: usize = 2_000;
/// Deepest nesting followed when collecting a sequence's fonts.
const MAX_NEST: usize = 16;

/// The licence a library font was added under.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct License {
    /// SPDX identifier or expression ("OFL-1.1", "Apache-2.0", "LicenseRef-Commercial-Acme").
    pub spdx: String,
    /// Licence text or URL (from the font file or the person adding it).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Where the font came from (foundry, URL, client folder).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

/// One face a library font provides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LibraryFace {
    pub family: String,
    pub style: String,
    pub weight: u16,
    pub italic: bool,
}

/// A font file in the library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LibraryFont {
    /// First 16 hex digits of the file's SHA-256.
    pub id: String,
    /// File name inside the library folder.
    pub file: String,
    /// "ttf", "otf", "ttc", "woff", "woff2".
    pub format: String,
    /// The file name it was added from.
    pub original_name: String,
    pub faces: Vec<LibraryFace>,
    /// Variation axes ("wght", "wdth", …); empty for static fonts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub axes: Vec<String>,
    pub license: License,
    /// When it was added (RFC 3339, UTC).
    pub added: String,
    pub bytes: u64,
    pub sha256: String,
}

impl LibraryFont {
    /// Families of the font, in face order, without repeats.
    pub fn families(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for f in &self.faces {
            if !v.iter().any(|x| x.eq_ignore_ascii_case(&f.family)) {
                v.push(f.family.clone());
            }
        }
        v
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub fonts: Vec<LibraryFont>,
}

/// Faces registered per library file (by SHA-256), and load problems, for this process (the
/// text engine's font database is process-wide).
#[derive(Default)]
struct Registry {
    faces: HashMap<String, Vec<FaceId>>,
    loaded_dirs: HashSet<PathBuf>,
    problems: Vec<String>,
}

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(Default::default).lock().unwrap_or_else(PoisonError::into_inner)
}

/// The library folder: the session's override, else `<data dir>/Font Library` (the data
/// directory of the preferences file, or the default one).
pub fn library_dir(s: &Session) -> Option<PathBuf> {
    crate::user_library::data_dir(s).map(|d| d.join(DIR_NAME))
}

/// Read the manifest (an absent folder or manifest is an empty library; an unreadable one is an
/// error, so a damaged library is never silently treated as empty when writing).
pub fn read_manifest(dir: &Path) -> std::result::Result<Manifest, String> {
    match read_json::<Manifest>(&dir.join(MANIFEST), "font library manifest")? {
        Some(mut m) => {
            m.fonts.truncate(MAX_FONTS);
            Ok(m)
        }
        None => Ok(Manifest { version: MANIFEST_VERSION, fonts: Vec::new() }),
    }
}

fn write_manifest(dir: &Path, m: &Manifest) -> std::result::Result<(), String> {
    write_json(&dir.join(MANIFEST), m)
}

/// Register one library font's file with the text engine (once per process).
fn register(dir: &Path, f: &LibraryFont) -> std::result::Result<usize, String> {
    if let Some(ids) = registry().faces.get(&f.sha256) {
        return Ok(ids.len());
    }
    let path = dir.join(&f.file);
    let bytes = std::fs::read(&path).map_err(|e| format!("{} ({}): the font file is missing or unreadable: {e}", f.families().join(", "), path.display()))?;
    if sha256_hex(&bytes) != f.sha256 {
        return Err(format!("{} ({}): the font file changed on disk since it was added; re-import it", f.families().join(", "), path.display()));
    }
    let ids = fonts::add_font_data_from(bytes, fonts::LIBRARY_ORIGIN);
    if ids.is_empty() {
        return Err(format!("{} ({}): the font file could not be read", f.families().join(", "), path.display()));
    }
    let n = ids.len();
    registry().faces.insert(f.sha256.clone(), ids);
    Ok(n)
}

/// Register every font of the library folder (once per folder and process). Returns the number
/// of faces registered by this call; problems (missing or changed files) are kept for
/// [`problems`] and listed by `fonts.library.list`.
pub fn load_dir(dir: &Path) -> usize {
    if !registry().loaded_dirs.insert(dir.to_path_buf()) {
        return 0;
    }
    let m = match read_manifest(dir) {
        Ok(m) => m,
        Err(e) => {
            registry().problems.push(e);
            return 0;
        }
    };
    let mut n = 0;
    for f in &m.fonts {
        match register(dir, f) {
            Ok(k) => n += k,
            Err(e) => registry().problems.push(e),
        }
    }
    if n > 0 {
        log::info!("font library: {n} face(s) from {}", dir.display());
    }
    n
}

/// Load the session's library (see [`library_dir`]). Called at startup by the app and the CLI,
/// and by every font-library command.
pub fn ensure_loaded(s: &Session) -> usize {
    library_dir(s).map(|d| load_dir(&d)).unwrap_or(0)
}

/// Problems met loading library fonts in this process.
pub fn problems() -> Vec<String> {
    registry().problems.clone()
}

/// What a font file holds, before adding it: faces, format, axes and the licence it states.
#[derive(Clone, Debug, Serialize)]
pub struct Inspection {
    pub format: String,
    pub faces: Vec<LibraryFace>,
    pub axes: Vec<String>,
    /// The licence description from the font's name table (ID 13), if any.
    pub license_description: Option<String>,
    /// The licence URL from the font's name table (ID 14), if any.
    pub license_url: Option<String>,
    /// An SPDX identifier recognised from that statement (never guessed beyond it).
    pub suggested_spdx: Option<String>,
    pub bytes: u64,
    pub sha256: String,
}

/// An SPDX identifier when a font's own licence statement names a well-known licence.
pub fn spdx_from_statement(desc: Option<&str>, url: Option<&str>) -> Option<String> {
    let t = format!("{} {}", desc.unwrap_or(""), url.unwrap_or("")).to_ascii_lowercase();
    if t.contains("open font license") || t.contains("openfontlicense") || t.contains("scripts.sil.org/ofl") {
        Some("OFL-1.1".into())
    } else if t.contains("apache license") || t.contains("apache.org/licenses/license-2.0") {
        Some("Apache-2.0".into())
    } else if t.contains("ubuntu font licence") || t.contains("ubuntu font license") {
        Some("Ubuntu-font-1.0".into())
    } else if t.contains("creativecommons.org/publicdomain/zero") {
        Some("CC0-1.0".into())
    } else {
        None
    }
}

fn read_font_file(path: &Path) -> std::result::Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if meta.len() > fonts::MAX_FONT_BYTES as u64 {
        return Err(format!("{} is too large for a font ({} bytes, max {})", path.display(), meta.len(), fonts::MAX_FONT_BYTES));
    }
    std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Inspect font bytes.
pub fn inspect_bytes(bytes: &[u8]) -> std::result::Result<Inspection, String> {
    let format = fonts::container_format(bytes).ok_or("not a font file (TrueType, OpenType, WOFF or WOFF2)")?;
    let faces = fonts::describe(bytes)?;
    let (desc, url) = fonts::license_info(bytes);
    let suggested_spdx = spdx_from_statement(desc.as_deref(), url.as_deref());
    Ok(Inspection {
        format: format.to_string(),
        faces: faces.into_iter().map(|f| LibraryFace { family: f.family, style: f.style, weight: f.weight, italic: f.italic }).collect(),
        axes: fonts::variable_axes(bytes),
        license_description: desc,
        license_url: url,
        suggested_spdx,
        bytes: bytes.len() as u64,
        sha256: sha256_hex(bytes),
    })
}

/// Check an SPDX identifier or expression: non-empty, at most 200 characters, only the
/// characters SPDX expressions use. "NOASSERTION" and "NONE" are refused: a font is added under a
/// licence someone has stated.
pub fn validate_spdx(spdx: &str) -> std::result::Result<String, String> {
    let t = spdx.trim();
    if t.is_empty() {
        return Err("a licence is required: give its SPDX identifier (e.g. OFL-1.1, Apache-2.0, or LicenseRef-<name> for a commercial licence)".into());
    }
    if t.len() > 200 || !t.chars().all(|c| c.is_ascii_alphanumeric() || " .-+():".contains(c)) {
        return Err(format!("`{t}` is not an SPDX licence identifier or expression"));
    }
    if t.eq_ignore_ascii_case("NOASSERTION") || t.eq_ignore_ascii_case("NONE") {
        return Err("a font needs a stated licence (NOASSERTION / NONE are not accepted)".into());
    }
    Ok(t.to_string())
}

/// What [`import`] did.
#[derive(Clone, Debug, Serialize)]
pub struct Imported {
    pub font: LibraryFont,
    /// False when the same file was already in the library (only its licence may have changed).
    pub added: bool,
    /// Library fonts replaced because they provided the same family and style.
    pub replaced: Vec<String>,
    pub registered_faces: usize,
}

/// Add a font file to the library folder `dir`: validates it, copies it, records its licence and
/// registers its faces. A font that provides a family + style already in the library is refused
/// unless `replace` (then the older file is removed), so a name never means two fonts.
pub fn import(dir: &Path, path: &Path, license: License, replace: bool) -> std::result::Result<Imported, String> {
    let spdx = validate_spdx(&license.spdx)?;
    let license = License { spdx, text: license.text.trim().to_string(), source: license.source.trim().to_string() };
    let bytes = read_font_file(path)?;
    let ins = inspect_bytes(&bytes)?;
    let mut m = read_manifest(dir)?;
    if let Some(i) = m.fonts.iter().position(|f| f.sha256 == ins.sha256) {
        let mut font = m.fonts.get(i).cloned().ok_or("library changed while importing")?;
        if font.license != license {
            font.license = license;
            if let Some(slot) = m.fonts.get_mut(i) {
                *slot = font.clone();
            }
            write_manifest(dir, &m)?;
        }
        let registered_faces = register(dir, &font)?;
        return Ok(Imported { font, added: false, replaced: Vec::new(), registered_faces });
    }
    let clashes: Vec<LibraryFont> = m
        .fonts
        .iter()
        .filter(|f| f.faces.iter().any(|a| ins.faces.iter().any(|b| a.family.eq_ignore_ascii_case(&b.family) && a.style.eq_ignore_ascii_case(&b.style))))
        .cloned()
        .collect();
    if !clashes.is_empty() && !replace {
        let names: Vec<String> = clashes.iter().map(|f| format!("{} ({})", f.families().join(", "), f.original_name)).collect();
        return Err(format!("the library already has {}; pass replace to replace it", names.join(", ")));
    }
    if m.fonts.len() - clashes.len() >= MAX_FONTS {
        return Err(format!("the font library is full ({MAX_FONTS} fonts)"));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let id = ins.sha256.chars().take(16).collect::<String>();
    let file = format!("{id}.{}", ins.format);
    write_atomic(&dir.join(&file), &bytes)?;
    let original_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let font = LibraryFont {
        id,
        file,
        format: ins.format.clone(),
        original_name,
        faces: ins.faces.clone(),
        axes: ins.axes.clone(),
        license,
        added: now_rfc3339(),
        bytes: ins.bytes,
        sha256: ins.sha256.clone(),
    };
    let mut replaced = Vec::new();
    for c in &clashes {
        m.fonts.retain(|f| f.id != c.id);
        retire(c);
        let _ = std::fs::remove_file(dir.join(&c.file));
        replaced.push(c.id.clone());
    }
    m.version = MANIFEST_VERSION;
    m.fonts.push(font.clone());
    write_manifest(dir, &m)?;
    let registered_faces = register(dir, &font)?;
    Ok(Imported { font, added: true, replaced, registered_faces })
}

fn retire(f: &LibraryFont) {
    if let Some(ids) = registry().faces.remove(&f.sha256) {
        fonts::retire_faces(&ids);
    }
}

/// Remove a font from the library folder: its faces stop resolving at once and its file is
/// deleted.
pub fn remove(dir: &Path, id: &str) -> std::result::Result<LibraryFont, String> {
    let mut m = read_manifest(dir)?;
    let i = m.fonts.iter().position(|f| f.id == id).ok_or_else(|| format!("no font `{id}` in the library"))?;
    let f = m.fonts.remove(i);
    write_manifest(dir, &m)?;
    retire(&f);
    let _ = std::fs::remove_file(dir.join(&f.file));
    Ok(f)
}

/// Change the licence recorded for a library font.
pub fn set_license(dir: &Path, id: &str, license: License) -> std::result::Result<LibraryFont, String> {
    let spdx = validate_spdx(&license.spdx)?;
    let mut m = read_manifest(dir)?;
    let f = m.fonts.iter_mut().find(|f| f.id == id).ok_or_else(|| format!("no font `{id}` in the library"))?;
    f.license = License { spdx, text: license.text.trim().to_string(), source: license.source.trim().to_string() };
    let out = f.clone();
    write_manifest(dir, &m)?;
    Ok(out)
}

// ------------------------------------------------------------------ fonts a sequence uses

/// One use of a font in a sequence.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FontUse {
    pub family: String,
    pub style: String,
    /// Where: "caption track English", "Title on V2 at 00:00:03:00", "source graphic Lower Third".
    pub place: String,
}

fn param_text(e: &filmcraft_project::EffectInstance, k: &str) -> String {
    match e.params.get(k).map(|x| &x.value) {
        Some(ParamValue::Text(t)) => t.trim().to_string(),
        _ => String::new(),
    }
}

fn layer_uses(e: &filmcraft_project::EffectInstance, place: &str, out: &mut Vec<FontUse>) {
    if e.effect != graphic::TEXT_LAYER || !e.enabled {
        return;
    }
    let fam = param_text(e, "font");
    let st = param_text(e, "font_style");
    let fam = if fam.is_empty() { fonts::DEFAULT_FAMILY.to_string() } else { fam };
    let st = if st.is_empty() { "Regular".to_string() } else { st };
    out.push(FontUse { family: fam.clone(), style: st.clone(), place: place.to_string() });
    for r in e.layer.as_ref().map(|x| x.runs.as_slice()).unwrap_or_default() {
        if r.style.font.is_some() || r.style.font_style.is_some() {
            out.push(FontUse {
                family: r.style.font.clone().filter(|f| !f.trim().is_empty()).unwrap_or_else(|| fam.clone()),
                style: r.style.font_style.clone().filter(|f| !f.trim().is_empty()).unwrap_or_else(|| st.clone()),
                place: format!("{place} (styled text)"),
            });
        }
    }
}

fn collect(p: &Project, seq: ItemId, captions: bool, depth: usize, seen: &mut HashSet<ItemId>, out: &mut Vec<FontUse>) {
    if depth > MAX_NEST || !seen.insert(seq) {
        return;
    }
    let Some(sq) = p.sequence(seq) else { return };
    let rate = sq.settings.frame_rate;
    for (ti, tr) in sq.video_tracks.iter().enumerate() {
        for t in tr.items.iter().filter(|t| t.enabled) {
            let place =
                format!("{} on V{} at {}", t.name, ti + 1, filmcraft_time::format_timecode_frames(rate.frame_at(t.start), rate, sq.settings.drop_frame));
            for e in &t.effects {
                layer_uses(e, &place, out);
            }
            if let Some(sg) = p.source_graphics.get(&t.item) {
                for e in &sg.layers {
                    layer_uses(e, &format!("{place} (source graphic)"), out);
                }
            }
            if let Some(item) = p.item(t.item)
                && matches!(item.kind, ItemKind::Sequence(_))
            {
                collect(p, t.item, captions, depth + 1, seen, out);
            }
        }
    }
    for t in sq.caption_tracks.iter().filter(|t| captions && t.enabled && !t.captions.is_empty()) {
        let st = if t.style.font_style.trim().is_empty() { "SemiBold" } else { t.style.font_style.trim() };
        let fam = if t.style.font.trim().is_empty() { fonts::DEFAULT_FAMILY } else { t.style.font.trim() };
        out.push(FontUse { family: fam.to_string(), style: st.to_string(), place: format!("caption track {}", t.name) });
    }
}

/// Every font a sequence renders (enabled clips, nested sequences, source graphics and, with
/// `captions`, enabled caption tracks).
pub fn sequence_fonts(p: &Project, seq: ItemId, captions: bool) -> Vec<FontUse> {
    let mut out = Vec::new();
    collect(p, seq, captions, 0, &mut HashSet::new(), &mut out);
    out
}

/// How one family + style a sequence uses will render.
#[derive(Clone, Debug, Serialize)]
pub struct FontCheck {
    pub family: String,
    pub style: String,
    /// "exact", "substituted" or "missing".
    pub status: &'static str,
    /// The face drawn instead ("Inter Regular" for a missing family).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_as: Option<String>,
    /// Synthetic bold / italic is drawn.
    pub synthetic: bool,
    /// From the font library, the system, bundled, …
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    pub uses: Vec<String>,
}

/// Status of every family + style a sequence uses, missing and substituted first.
pub fn font_report(p: &Project, seq: ItemId, captions: bool) -> Vec<FontCheck> {
    let mut by: BTreeMap<(String, String), (String, String, Vec<String>)> = BTreeMap::new();
    for u in sequence_fonts(p, seq, captions) {
        let e = by.entry((u.family.to_ascii_lowercase(), u.style.to_ascii_lowercase())).or_insert_with(|| (u.family.clone(), u.style.clone(), Vec::new()));
        if e.2.len() < 50 {
            e.2.push(u.place);
        }
    }
    let mut v: Vec<FontCheck> = by
        .into_values()
        .map(|(family, style, uses)| {
            let (status, rendered_as, synthetic, origin) = match fonts::status(&family, &style) {
                FontStatus::Exact { face } => ("exact", None, false, Some(fonts::face(face).info.origin.to_string())),
                FontStatus::Substituted { face, used_style, synthetic } => {
                    let f = fonts::face(face);
                    ("substituted", Some(format!("{} {used_style}", f.info.family)), synthetic, Some(f.info.origin.to_string()))
                }
                FontStatus::Missing => {
                    let r = fonts::resolve(fonts::DEFAULT_FAMILY, &style);
                    let f = fonts::face(r.face);
                    ("missing", Some(format!("{} {}", f.info.family, f.info.style)), r.synth_bold || r.synth_italic, None)
                }
            };
            FontCheck { family, style, status, rendered_as, synthetic, origin, uses }
        })
        .collect();
    v.sort_by_key(|c| match c.status {
        "missing" => 0,
        "substituted" => 1,
        _ => 2,
    });
    v
}

/// The fonts of `seq` that would not render as specified, as one message per font (empty when
/// every font is exact).
pub fn font_problems(p: &Project, seq: ItemId, captions: bool) -> Vec<String> {
    font_report(p, seq, captions)
        .into_iter()
        .filter(|c| c.status != "exact")
        .map(|c| {
            let instead = c.rendered_as.clone().unwrap_or_default();
            let where_ = c.uses.first().cloned().unwrap_or_default();
            let more = if c.uses.len() > 1 { format!(" and {} more", c.uses.len() - 1) } else { String::new() };
            format!("{} {} is {} (would render as {instead}); used by {where_}{more}", c.family, c.style, c.status)
        })
        .collect()
}

/// The export guard: an export whose video uses a missing or substituted font is refused, naming
/// the fonts, unless `allow` (an explicit `allowFontSubstitution`), which is logged in the
/// decision log and returned as warnings. Audio-only exports and captions that are not burned in
/// are not checked.
pub fn check_export(
    s: &mut Session,
    project: &Project,
    seq: ItemId,
    settings: &filmcraft_export::ExportSettings,
    allow: bool,
    cmd: &str,
) -> Result<Vec<String>> {
    if !settings.has_video() {
        return Ok(Vec::new());
    }
    ensure_loaded(s);
    let problems = font_problems(project, seq, settings.burn_captions);
    if problems.is_empty() {
        return Ok(Vec::new());
    }
    if !allow {
        return Err(bad(
            cmd,
            format!(
                "fonts would not render as specified: {}. Add the fonts to the Font Library (Window ▸ Font Library) or change the text, or pass allowFontSubstitution: true to export anyway",
                problems.join("; ")
            ),
        ));
    }
    let entry = json!({"kind": "fontSubstitutionAllowed", "command": cmd, "sequence": seq.0, "fonts": problems});
    s.autoedit_log.push(entry);
    Ok(problems.into_iter().map(|p| format!("font substitution allowed: {p}")).collect())
}

// ------------------------------------------------------------------ preview

/// A preview of `text` in a family + style as straight RGBA (dark text on transparent), drawn by
/// the text engine that renders titles and captions. None when nothing is drawn.
pub fn preview_rgba(family: &str, style: &str, text: &str, size: f32) -> Option<(u32, u32, Vec<u8>)> {
    preview_rgba_tracked(family, style, text, size, 0.0)
}

/// [`preview_rgba`] with tracking in em (`-0.03` = the CSS `letter-spacing: -0.03em`).
pub fn preview_rgba_tracked(family: &str, style: &str, text: &str, size: f32, tracking_em: f32) -> Option<(u32, u32, Vec<u8>)> {
    preview_rgba_box(family, style, text, size, tracking_em, None, None)
}

/// [`preview_rgba_tracked`] wrapped to `width` pixels with `line_height` (a multiple of the size,
/// CSS `line-height`), for previews of text set in a box.
pub fn preview_rgba_box(
    family: &str,
    style: &str,
    text: &str,
    size: f32,
    tracking_em: f32,
    width: Option<f32>,
    line_height: Option<f32>,
) -> Option<(u32, u32, Vec<u8>)> {
    let size = if size.is_finite() { size.clamp(4.0, 400.0) } else { 48.0 };
    let tracking = if tracking_em.is_finite() { (tracking_em * 1000.0).clamp(-500.0, 2000.0) } else { 0.0 };
    let text: String = text.chars().take(200).collect();
    let st = filmcraft_text::TextStyle { family: family.into(), style: style.into(), size, tracking, ..Default::default() };
    let mut ps = filmcraft_text::ParagraphStyle::default();
    if let Some(w) = width.filter(|w| w.is_finite() && *w > 1.0) {
        ps.width = Some(w.min(8000.0));
    }
    // CSS line-height as the line pitch: leading is added on top of the font's own line height
    if let Some(lh) = line_height.filter(|v| v.is_finite() && *v > 0.0) {
        let natural = filmcraft_text::fonts::face(filmcraft_text::resolve(family, style).face).metrics(size);
        let natural = natural.ascent + natural.descent + natural.line_gap;
        ps.leading = (lh.clamp(0.3, 5.0) * size - natural).clamp(-size, 4.0 * size);
    }
    let l = filmcraft_text::layout(&text, &st, &ps);
    let (m, _, _) = filmcraft_text::render::rasterize(&l, &filmcraft_text::render::at(0.0, 0.0));
    if m.w == 0 || m.h == 0 || m.w > 8192 || m.h > 4096 {
        return None;
    }
    let mut rgba = Vec::with_capacity(m.w.saturating_mul(m.h).saturating_mul(4));
    for a in &m.a {
        let v = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
        rgba.extend_from_slice(&[255, 255, 255, v]);
    }
    Some((u32::try_from(m.w).ok()?, u32::try_from(m.h).ok()?, rgba))
}

// ------------------------------------------------------------------ commands

fn need_dir(s: &Session, cmd: &str) -> Result<PathBuf> {
    library_dir(s).ok_or_else(|| bad(cmd, "no data directory for the font library (set FILMCRAFT_DATA_DIR)"))
}

fn license_p(p: &Value) -> License {
    match p.get("license") {
        Some(Value::String(s)) => {
            License { spdx: s.clone(), text: str_p(p, "licenseText").unwrap_or("").to_string(), source: str_p(p, "source").unwrap_or("").to_string() }
        }
        Some(Value::Object(o)) => License {
            spdx: o.get("spdx").and_then(Value::as_str).unwrap_or("").to_string(),
            text: o.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
            source: o.get("source").and_then(Value::as_str).unwrap_or("").to_string(),
        },
        _ => License::default(),
    }
}

fn font_json(f: &LibraryFont) -> Value {
    let faces: Vec<Value> = f
        .faces
        .iter()
        .map(|a| {
            let st = match fonts::status(&a.family, &a.style) {
                FontStatus::Exact { face } if fonts::face(face).info.origin == fonts::LIBRARY_ORIGIN => "active",
                FontStatus::Exact { .. } => "shadowed",
                _ => "not loaded",
            };
            json!({"family": a.family, "style": a.style, "weight": a.weight, "italic": a.italic, "state": st})
        })
        .collect();
    json!({"id": f.id, "families": f.families(), "file": f.file, "originalName": f.original_name, "format": f.format,
           "variable": !f.axes.is_empty(), "axes": f.axes, "license": f.license, "added": f.added, "bytes": f.bytes,
           "sha256": f.sha256, "faces": faces})
}

fn list(s: &mut Session, _p: &Value) -> Result<Value> {
    ensure_loaded(s);
    let dir = need_dir(s, "fonts.library.list")?;
    let m = read_manifest(&dir).map_err(|e| bad("fonts.library.list", e))?;
    Ok(json!({"dir": dir.display().to_string(), "fonts": m.fonts.iter().map(font_json).collect::<Vec<_>>(), "problems": problems()}))
}

fn inspect(_s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("fonts.library.inspect", "need `path` (a font file)"))?;
    let bytes = read_font_file(Path::new(path)).map_err(|e| bad("fonts.library.inspect", e))?;
    let ins = inspect_bytes(&bytes).map_err(|e| bad("fonts.library.inspect", e))?;
    serde_json::to_value(ins).map_err(|e| bad("fonts.library.inspect", e.to_string()))
}

fn import_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    ensure_loaded(s);
    let dir = need_dir(s, "fonts.library.import")?;
    let paths: Vec<String> = match (p.get("paths").and_then(Value::as_array), str_p(p, "path")) {
        (Some(a), _) => a.iter().filter_map(Value::as_str).map(str::to_string).take(200).collect(),
        (None, Some(x)) => vec![x.to_string()],
        _ => return Err(bad("fonts.library.import", "need `path` or `paths` (font files)")),
    };
    if paths.is_empty() {
        return Err(bad("fonts.library.import", "need `path` or `paths` (font files)"));
    }
    let license = license_p(p);
    let replace = bool_p(p, "replace").unwrap_or(false);
    let mut out = Vec::new();
    for path in &paths {
        // replacing removes the older file: refused when an approved brand kit uses it
        if replace
            && let (Some(bd), Ok(bytes)) = (crate::brand_kits::Dirs::of(s), read_font_file(Path::new(path)))
            && let Ok(ins) = inspect_bytes(&bytes)
        {
            let m = read_manifest(&dir).map_err(|e| bad("fonts.library.import", e))?;
            for f in m.fonts.iter().filter(|f| {
                f.faces.iter().any(|a| ins.faces.iter().any(|b| a.family.eq_ignore_ascii_case(&b.family) && a.style.eq_ignore_ascii_case(&b.style)))
            }) {
                let kits = crate::brand_kits::font_users(&bd, &f.id);
                if !kits.is_empty() && f.sha256 != ins.sha256 {
                    return Err(bad(
                        "fonts.library.import",
                        format!("{path}: would replace {} which approved brand kits use ({})", f.families().join(", "), kits.join(", ")),
                    ));
                }
            }
        }
        let r = import(&dir, Path::new(path), license.clone(), replace).map_err(|e| bad("fonts.library.import", format!("{path}: {e}")))?;
        out.push(json!({"font": font_json(&r.font), "added": r.added, "replaced": r.replaced, "registeredFaces": r.registered_faces}));
    }
    Ok(json!({"imported": out}))
}

fn remove_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    ensure_loaded(s);
    let dir = need_dir(s, "fonts.library.remove")?;
    let id = str_p(p, "id").ok_or_else(|| bad("fonts.library.remove", "need `id`"))?;
    let m = read_manifest(&dir).map_err(|e| bad("fonts.library.remove", e))?;
    let f = m.fonts.iter().find(|f| f.id == id).ok_or_else(|| bad("fonts.library.remove", format!("no font `{id}` in the library")))?;
    let used: Vec<String> = crate::graphic_templates::fonts_used(s)
        .into_iter()
        .filter(|(fam, st, _)| f.faces.iter().any(|a| a.family.eq_ignore_ascii_case(fam) && a.style.eq_ignore_ascii_case(st)))
        .map(|(fam, st, _)| format!("{fam} {st}"))
        .collect();
    if !used.is_empty() && !bool_p(p, "force").unwrap_or(false) {
        return Err(bad(
            "fonts.library.remove",
            format!("the open project uses {}; pass force to remove it anyway (its text will be reported missing)", used.join(", ")),
        ));
    }
    // approved brand kits keep their exact fonts: never removed, `force` or not
    if let Some(bd) = crate::brand_kits::Dirs::of(s) {
        let kits = crate::brand_kits::font_users(&bd, id);
        if !kits.is_empty() {
            return Err(bad("fonts.library.remove", format!("approved brand kits use this font ({}); it cannot be removed", kits.join(", "))));
        }
    }
    let f = remove(&dir, id).map_err(|e| bad("fonts.library.remove", e))?;
    Ok(json!({"removed": f.id, "families": f.families(), "wasUsed": used}))
}

fn set_license_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let dir = need_dir(s, "fonts.library.setLicense")?;
    let id = str_p(p, "id").ok_or_else(|| bad("fonts.library.setLicense", "need `id`"))?;
    let f = set_license(&dir, id, license_p(p)).map_err(|e| bad("fonts.library.setLicense", e))?;
    Ok(font_json(&f))
}

fn preview_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    ensure_loaded(s);
    let family = str_p(p, "family").ok_or_else(|| bad("fonts.library.preview", "need `family`"))?;
    let styles: Vec<String> = match str_p(p, "style") {
        Some(st) => vec![st.to_string()],
        None => filmcraft_text::families().into_iter().find(|(f, _)| f.eq_ignore_ascii_case(family)).map(|(_, st)| st).unwrap_or_default(),
    };
    if styles.is_empty() {
        return Err(bad("fonts.library.preview", format!("{family} is not installed")));
    }
    let text = str_p(p, "text").unwrap_or("The quick brown fox jumps over the lazy dog 0123456789");
    let size = f64_p(p, "size").unwrap_or(48.0) as f32;
    let pad = 8u32;
    let mut rows = Vec::new();
    for st in styles.iter().take(u64_p(p, "limit").unwrap_or(24).clamp(1, 64) as usize) {
        let label = format!("{st} — {text}");
        if let Some(r) = preview_rgba(family, st, &label, size) {
            rows.push((st.clone(), fonts::status(family, st), r));
        }
    }
    let w = rows.iter().map(|r| r.2.0).max().unwrap_or(1).saturating_add(2 * pad).min(8192);
    let h = rows.iter().map(|r| r.2.1.saturating_add(pad)).sum::<u32>().saturating_add(pad).min(16384);
    let mut img = vec![0u8; (w as usize).saturating_mul(h as usize).saturating_mul(4)];
    for px in img.chunks_exact_mut(4) {
        px.copy_from_slice(&[255, 255, 255, 255]);
    }
    let mut y0 = pad;
    let mut out_rows = Vec::new();
    for (st, status, (rw, rh, rgba)) in &rows {
        for y in 0..*rh {
            for x in 0..*rw {
                let (dx, dy) = (x + pad, y + y0);
                if dx >= w || dy >= h {
                    continue;
                }
                let a = rgba.get(((y * rw + x) * 4 + 3) as usize).copied().unwrap_or(0);
                if let Some(px) = img.get_mut(((dy * w + dx) * 4) as usize..((dy * w + dx) * 4 + 3) as usize) {
                    let v = 255u8.saturating_sub(a);
                    px.copy_from_slice(&[v, v, v]);
                }
            }
        }
        out_rows.push(json!({"style": st, "exact": status.is_exact(), "y": y0, "height": rh}));
        y0 = y0.saturating_add(rh.saturating_add(pad));
    }
    let mut reply = json!({"family": family, "width": w, "height": h, "rows": out_rows});
    if let Some(path) = str_p(p, "path") {
        let png = filmcraft_export::encode_png(img, w, h).map_err(|e| bad("fonts.library.preview", e.to_string()))?;
        std::fs::write(path, png).map_err(|e| bad("fonts.library.preview", format!("cannot write {path}: {e}")))?;
        reply["path"] = json!(path);
    }
    Ok(reply)
}

fn missing_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    ensure_loaded(s);
    let seqs: Vec<ItemId> = if bool_p(p, "all").unwrap_or(false) {
        s.project.sequences().map(|q| q.id).collect()
    } else if let Some(id) = u64_p(p, "sequence") {
        vec![ItemId(id)]
    } else {
        s.state.active_sequence.into_iter().collect()
    };
    if seqs.is_empty() {
        return Err(bad("fonts.missing", "no sequence (open one, or pass `sequence` or `all`)"));
    }
    let mut out = Vec::new();
    let mut ok = true;
    for id in seqs {
        let r = font_report(&s.project, id, !bool_p(p, "ignoreCaptions").unwrap_or(false));
        ok &= r.iter().all(|c| c.status == "exact");
        let name = s.project.item(id).map(|i| i.name.clone()).unwrap_or_default();
        out.push(json!({"sequence": id.0, "name": name, "fonts": r}));
    }
    Ok(json!({"ok": ok, "sequences": out}))
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    let q = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    vec![
        q("fonts.library.list", "List Font Library", "{}", list),
        q("fonts.library.inspect", "Inspect Font File", r#"{"path":font file (ttf, otf, ttc, woff, woff2)}"#, inspect),
        q(
            "fonts.library.import",
            "Add Fonts to Library",
            r#"{"path":font file | "paths":[...],"license":SPDX id (required, e.g. "OFL-1.1") | {"spdx","text","source"},"licenseText":string?,"source":string?,"replace":bool=false (replace a library font with the same family and style)}"#,
            import_cmd,
        ),
        q(
            "fonts.library.remove",
            "Remove Font from Library",
            r#"{"id":library font id,"force":bool=false (remove even if the open project uses it)}"#,
            remove_cmd,
        ),
        q("fonts.library.setLicense", "Set Font Licence", r#"{"id":library font id,"license":SPDX id | {"spdx","text","source"}}"#, set_license_cmd),
        q(
            "fonts.library.preview",
            "Preview Font",
            r#"{"family":string,"style":string? (default: every style),"text":string?,"size":px=48,"limit":n=24,"path":png path?}"#,
            preview_cmd,
        ),
        q("fonts.missing", "Check Fonts", r#"{"sequence":id? (default: active),"all":bool=false,"ignoreCaptions":bool=false}"#, missing_cmd),
    ]
}
