//! Client brand kits: clients, their logos and brand graphics, and versioned brand kits that tie
//! together exact Font Library faces, exact Colour Palette Library versions, typography roles,
//! colour roles, logo usage and visual rules. Kept in `<data dir>/Brand Kits/` (the storage of
//! [`crate::user_library`], with the Font and Palette Libraries).
//!
//! - **Clients** (`cli_…`) own their assets and kits; a kit can only use its own client's assets,
//!   and a project pinned to one client's kit is not moved to another client's kit by accident.
//! - **Brand assets** (`ast_…`): PNG, JPEG or WebP files, decoded by FilmCraft's still decoder to
//!   validate them, stored by content hash with their size and fingerprint.
//! - **Kit versions**: the latest version may be a *draft* (editable; every save is logged); an
//!   *approved* version is frozen with its fingerprint and the approval record (who, and the
//!   reference of their decision). Editing an approved kit starts a new draft version. Approval
//!   is refused while the kit has errors (a missing font or weight, a palette version or asset
//!   that is gone or altered, a rule contradicting its palette) — see [`check`].
//! - **References are exact**: fonts by Font Library id + file SHA-256 + face, palettes by
//!   `pal_…@N` + fingerprint, assets by id + SHA-256. A changed source is reported, never
//!   followed silently. Fonts, palettes and assets used by an approved kit cannot be removed.
//! - **Projects** pin one approved version (`Project::brand`); moving the pin to another version
//!   or kit needs an explicit authorisation, and to another client's kit a second one.
//! - **ARCA** (`arca.brand/0.1`, see [`arca`]): ARCA stays the authority. Kits imported from ARCA
//!   are read-only here; they change by importing a new version from ARCA, and they are approved
//!   only when the ARCA document carries ARCA's approval record.
//!
//! Commands: `brand.clients.*`, `brand.assets.*`, `brand.kits.*`, `brand.project.*`,
//! `brand.resolve`, `brand.arca.*`; the Brand Kits window drives the same functions.

pub mod arca;
mod commands;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::palette_library::HistoryEvent;
use crate::user_library::{new_id, now_rfc3339, read_json, sha256_hex, write_atomic, write_json};
use crate::{font_library, palette_library};

pub(crate) use commands::commands;

pub const DIR_NAME: &str = "Brand Kits";
const FILE: &str = "brand.json";
const ASSET_DIR: &str = "assets";
const STORE_FORMAT: u32 = 1;
pub const MAX_CLIENTS: usize = 2_000;
pub const MAX_ASSETS: usize = 10_000;
pub const MAX_KITS: usize = 5_000;
pub const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_ASSET_SIDE: u32 = 16_384;
const MAX_VERSIONS: usize = 1_000;
const MAX_HISTORY: usize = 2_000;
const MAX_ITEMS: usize = 256;
const MAX_NAME: usize = 128;
const MAX_TEXT: usize = 4_000;

/// Typography roles offered by the interface; any other named role is accepted.
pub const STANDARD_TYPE_ROLES: &[&str] = &["headline", "subtitle", "caption", "lowerThird", "cta", "wordmark", "kicker", "body"];
/// Alterations a logo rule can forbid.
pub const ALTERATIONS: &[&str] = &["recolor", "stretch", "rotate", "crop", "effects", "outline", "shadow", "rearrange"];
/// Anchors for placement rules.
pub const ANCHORS: &[&str] =
    &["top-left", "top-center", "top-right", "center-left", "center", "center-right", "bottom-left", "bottom-center", "bottom-right", "lower-third"];

// ------------------------------------------------------------------ model

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Client {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default)]
    pub archived: bool,
    /// Where the client record comes from ("filmcraft", or "arca:<client id>").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    pub created: String,
    pub updated: String,
    #[serde(default)]
    pub history: Vec<HistoryEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrandAsset {
    pub id: String,
    pub client: String,
    pub name: String,
    /// "logo" or "graphic".
    pub kind: String,
    /// The variant a logo is ("primary", "white", "icon", …).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub variant: String,
    pub file: String,
    /// "png", "jpeg" or "webp".
    pub format: String,
    pub original_name: String,
    pub sha256: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub has_alpha: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub license: String,
    pub added: String,
    pub by: String,
}

/// A colour of a palette version the kit references.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ColorRef {
    pub palette: String,
    pub color: String,
}

/// An exact face of the Font Library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KitFont {
    pub family: String,
    pub style: String,
    pub weight: u16,
    /// Font Library id of the file.
    pub library_font: String,
    /// SHA-256 of that file.
    pub sha256: String,
}

/// An exact version of a Palette Library palette.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KitPalette {
    pub palette: String,
    pub version: u32,
    pub sha256: String,
    #[serde(default)]
    pub name: String,
}

/// A named colour role of the brand ("background", "focal", "cta background", …).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorRole {
    pub role: String,
    pub color: ColorRef,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

/// A size: `value` in `unit` ("px" at the reference frame, "%w" or "%h" of the frame), clamped
/// to `min_px` / `max_px` when given (CSS `clamp()`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Size {
    pub value: f64,
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_px: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_px: Option<f64>,
}

impl Size {
    /// Pixels in a `w`×`h` frame.
    pub fn px(&self, w: f64, h: f64) -> f64 {
        let v = match self.unit.as_str() {
            "%w" => self.value * w / 100.0,
            "%h" => self.value * h / 100.0,
            _ => self.value,
        };
        let v = self.min_px.map_or(v, |m| v.max(m));
        self.max_px.map_or(v, |m| v.min(m))
    }
}

/// How one kind of text is set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeRole {
    /// "headline", "subtitle", "caption", "lowerThird", "cta" or any other name.
    pub role: String,
    pub family: String,
    pub style: String,
    pub weight: u16,
    pub size: Size,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorRef>,
    /// A box or pill behind the text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<ColorRef>,
    /// The emphasis colour (one focal word, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<ColorRef>,
    /// "left", "center" or "right".
    #[serde(default = "left")]
    pub align: String,
    /// An anchor (see [`ANCHORS`]) or the client's own description.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub placement: String,
    /// "none", "upper", "lower" or "title".
    #[serde(default = "none")]
    pub case: String,
    /// Letter spacing in em (CSS `letter-spacing`).
    #[serde(default)]
    pub tracking: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lines: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// Anything else the brand states (corner radius, shadow, …), as stated.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub styling: String,
}

fn left() -> String {
    "left".into()
}
fn none() -> String {
    "none".into()
}

/// How a logo asset may be used in this kit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogoUse {
    pub asset: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub variant: String,
    /// An approved variant of the logo for this kit.
    #[serde(default = "yes")]
    pub approved: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub placement: String,
    /// Clear space from the frame edge, % of the frame width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width: Option<Size>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<Size>,
    /// Backgrounds the logo may sit on (empty: not restricted by the kit).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backgrounds: Vec<ColorRef>,
    /// Alterations that are not allowed (see [`ALTERATIONS`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbidden: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

fn yes() -> bool {
    true
}

/// A rule the brand states, beyond its palettes' own rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KitRule {
    pub id: String,
    /// "permitted", "prohibited" or "guideline".
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<ColorRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<ColorRef>,
    #[serde(default = "any")]
    pub applies_to: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

fn any() -> String {
    "any".into()
}

/// Margins nothing important may cross, as fractions of the frame (CSS `padding: 9cqw 6.5cqw …`
/// of a 9:16 canvas is top 0.09 × width …; stored here as % of the frame width).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SafeArea {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
    /// "%w" (of the frame width, CSS cqw) or "%h".
    #[serde(default = "pct_w")]
    pub unit: String,
}

fn pct_w() -> String {
    "%w".into()
}

/// Where an imported kit comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KitSource {
    /// "arca.brand/0.1".
    pub format: String,
    /// The authority for approved decisions ("ARCA").
    pub authority: String,
    pub reference: String,
    pub imported: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KitContent {
    #[serde(default)]
    pub fonts: Vec<KitFont>,
    #[serde(default)]
    pub palettes: Vec<KitPalette>,
    #[serde(default)]
    pub colors: Vec<ColorRole>,
    #[serde(default)]
    pub typography: Vec<TypeRole>,
    #[serde(default)]
    pub logos: Vec<LogoUse>,
    #[serde(default)]
    pub rules: Vec<KitRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_area: Option<SafeArea>,
    /// The frame sizes the "px" values refer to ([1080, 1920] …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_frame: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

impl KitContent {
    pub fn fingerprint(&self) -> String {
        sha256_hex(&serde_json::to_vec(self).unwrap_or_default())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub by: String,
    pub reference: String,
    pub at: String,
    /// "FilmCraft" for an approval given here, "ARCA" for one carried from an ARCA document.
    pub authority: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KitVersion {
    pub version: u32,
    /// "draft" or "approved".
    pub status: String,
    pub created: String,
    pub updated: String,
    pub by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The approved version this draft started from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on: Option<u32>,
    pub content: KitContent,
    /// Fingerprint of `content` (fixed once approved).
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Approval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<KitSource>,
}

impl KitVersion {
    pub fn approved(&self) -> bool {
        self.status == "approved"
    }
    pub fn intact(&self) -> bool {
        self.content.fingerprint() == self.sha256
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrandKit {
    pub id: String,
    pub client: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub archived: bool,
    /// Set for kits ARCA manages (read-only here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<KitSource>,
    pub created: String,
    pub updated: String,
    pub versions: Vec<KitVersion>,
    #[serde(default)]
    pub history: Vec<HistoryEvent>,
}

impl BrandKit {
    pub fn latest(&self) -> Option<&KitVersion> {
        self.versions.last()
    }
    pub fn version(&self, n: u32) -> Option<&KitVersion> {
        self.versions.iter().find(|v| v.version == n)
    }
    pub fn latest_approved(&self) -> Option<&KitVersion> {
        self.versions.iter().rev().find(|v| v.approved())
    }
}

fn log(history: &mut Vec<HistoryEvent>, updated: &mut String, by: &str, action: &str, detail: String) {
    let at = now_rfc3339();
    updated.clone_from(&at);
    history.push(HistoryEvent { at, by: by.to_string(), action: action.to_string(), detail });
    if history.len() > MAX_HISTORY {
        let cut = history.len() - MAX_HISTORY;
        history.drain(..cut);
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub format: u32,
    pub clients: Vec<Client>,
    pub assets: Vec<BrandAsset>,
    pub kits: Vec<BrandKit>,
}

impl Store {
    pub fn client(&self, id: &str) -> Option<&Client> {
        self.clients.iter().find(|c| c.id == id)
    }
    pub fn asset(&self, id: &str) -> Option<&BrandAsset> {
        self.assets.iter().find(|a| a.id == id)
    }
    pub fn kit(&self, id: &str) -> Option<&BrandKit> {
        self.kits.iter().find(|k| k.id == id)
    }
}

// ------------------------------------------------------------------ storage

/// Folders of the libraries a kit draws on, all under one data directory.
#[derive(Clone, Debug)]
pub struct Dirs {
    pub brand: PathBuf,
    pub fonts: PathBuf,
    pub palettes: PathBuf,
}

impl Dirs {
    pub fn under(data: &Path) -> Self {
        Dirs { brand: data.join(DIR_NAME), fonts: data.join(font_library::DIR_NAME), palettes: data.join(palette_library::DIR_NAME) }
    }
    pub fn of(s: &crate::Session) -> Option<Self> {
        crate::user_library::data_dir(s).map(|d| Self::under(&d))
    }
    pub fn asset_path(&self, a: &BrandAsset) -> PathBuf {
        self.brand.join(ASSET_DIR).join(&a.file)
    }
}

pub fn load(dirs: &Dirs) -> Result<Store, String> {
    Ok(read_json::<Store>(&dirs.brand.join(FILE), "brand kit store")?.unwrap_or(Store { format: STORE_FORMAT, ..Default::default() }))
}

fn modify<T>(dirs: &Dirs, f: impl FnOnce(&mut Store) -> Result<T, String>) -> Result<T, String> {
    let mut st = load(dirs)?;
    let out = f(&mut st)?;
    st.format = STORE_FORMAT;
    write_json(&dirs.brand.join(FILE), &st)?;
    Ok(out)
}

fn clean(s: &str, what: &str, max: usize, required: bool) -> Result<String, String> {
    let t = s.trim();
    if required && t.is_empty() {
        return Err(format!("{what} cannot be empty"));
    }
    if t.len() > max || t.chars().any(|c| c.is_control() && c != '\n') {
        return Err(format!("{what} must be at most {max} bytes without control characters"));
    }
    Ok(t.to_string())
}

fn who(by: &str) -> Result<String, String> {
    let b = clean(by, "`by`", MAX_NAME, false)?;
    Ok(if b.is_empty() { "unspecified".into() } else { b })
}

// ------------------------------------------------------------------ clients

pub fn create_client(dirs: &Dirs, name: &str, notes: &str, by: &str, source: &str) -> Result<Client, String> {
    let name = clean(name, "a client name", MAX_NAME, true)?;
    let notes = clean(notes, "notes", MAX_TEXT, false)?;
    let by = who(by)?;
    modify(dirs, |st| {
        if st.clients.len() >= MAX_CLIENTS {
            return Err(format!("at most {MAX_CLIENTS} clients"));
        }
        if st.clients.iter().any(|c| !c.archived && c.name.eq_ignore_ascii_case(&name)) {
            return Err(format!("a client named \"{name}\" already exists"));
        }
        let now = now_rfc3339();
        let mut c = Client {
            id: new_id("cli", &name),
            name,
            notes,
            archived: false,
            source: source.to_string(),
            created: now.clone(),
            updated: now,
            history: Vec::new(),
        };
        let c2 = &mut c;
        log(&mut c2.history, &mut c2.updated, &by, "created", String::new());
        st.clients.push(c.clone());
        Ok(c)
    })
}

pub fn update_client(dirs: &Dirs, id: &str, name: Option<&str>, notes: Option<&str>, archived: Option<bool>, by: &str) -> Result<Client, String> {
    let by = who(by)?;
    let name = name.map(|n| clean(n, "a client name", MAX_NAME, true)).transpose()?;
    let notes = notes.map(|n| clean(n, "notes", MAX_TEXT, false)).transpose()?;
    modify(dirs, |st| {
        if let Some(n) = &name
            && st.clients.iter().any(|c| c.id != id && !c.archived && c.name.eq_ignore_ascii_case(n))
        {
            return Err(format!("a client named \"{n}\" already exists"));
        }
        let c = st.clients.iter_mut().find(|c| c.id == id).ok_or_else(|| format!("no client `{id}`"))?;
        if let Some(n) = name.filter(|n| *n != c.name) {
            let old = std::mem::replace(&mut c.name, n.clone());
            log(&mut c.history, &mut c.updated, &by, "renamed", format!("\"{old}\" → \"{n}\""));
        }
        if let Some(n) = notes.filter(|n| *n != c.notes) {
            c.notes = n;
            log(&mut c.history, &mut c.updated, &by, "notes", String::new());
        }
        if let Some(a) = archived.filter(|a| *a != c.archived) {
            c.archived = a;
            log(&mut c.history, &mut c.updated, &by, if a { "archived" } else { "restored" }, String::new());
        }
        Ok(c.clone())
    })
}

// ------------------------------------------------------------------ assets

/// What [`import_asset`] reads from a file.
pub struct AssetSpec<'a> {
    pub client: &'a str,
    pub path: &'a Path,
    pub name: &'a str,
    pub kind: &'a str,
    pub variant: &'a str,
    pub source: &'a str,
    pub license: &'a str,
    pub by: &'a str,
}

/// The format of an image file from its first bytes (the formats brand assets accept).
pub fn image_format(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpeg")
    } else if b.len() > 12 && b.get(0..4) == Some(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        Some("webp")
    } else {
        None
    }
}

/// Decode an image with FilmCraft's still decoder (under `catch_unwind`): width, height, alpha,
/// RGBA.
pub fn decode_image(name: &str, bytes: &[u8]) -> Result<filmcraft_media::still::Raster, String> {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| filmcraft_media::still::decode_raster(name, bytes)))
        .map_err(|_| format!("{name}: the image could not be decoded"))?
        .map_err(|e| format!("{name}: the image could not be decoded: {e}"))?;
    if r.width == 0 || r.height == 0 || r.width > MAX_ASSET_SIDE || r.height > MAX_ASSET_SIDE {
        return Err(format!("{name}: {}×{} is not a usable size (1 to {MAX_ASSET_SIDE} pixels a side)", r.width, r.height));
    }
    Ok(r)
}

/// Add a logo or brand graphic to a client: validated (format, decodes, size), copied by content
/// hash, recorded with its metadata.
pub fn import_asset(dirs: &Dirs, spec: &AssetSpec) -> Result<BrandAsset, String> {
    let kind = match spec.kind.trim() {
        "" | "logo" => "logo",
        "graphic" => "graphic",
        k => return Err(format!("asset kind `{k}` is not logo or graphic")),
    };
    let meta = std::fs::metadata(spec.path).map_err(|e| format!("cannot read {}: {e}", spec.path.display()))?;
    if meta.len() > MAX_ASSET_BYTES {
        return Err(format!("{} is too large ({} bytes, max {MAX_ASSET_BYTES})", spec.path.display(), meta.len()));
    }
    let bytes = std::fs::read(spec.path).map_err(|e| format!("cannot read {}: {e}", spec.path.display()))?;
    let original_name = spec.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let format = image_format(&bytes).ok_or_else(|| {
        let ext = spec.path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if ext == "svg" {
            "SVG logos are not supported yet: export the logo as PNG (with transparency) or WebP".to_string()
        } else {
            format!("{original_name} is not a PNG, JPEG or WebP image")
        }
    })?;
    let r = decode_image(&original_name, &bytes)?;
    let name = clean(if spec.name.trim().is_empty() { &original_name } else { spec.name }, "an asset name", MAX_NAME, true)?;
    let variant = clean(spec.variant, "a variant", MAX_NAME, false)?;
    let source = clean(spec.source, "a source", MAX_TEXT, false)?;
    let license = clean(spec.license, "a licence", MAX_TEXT, false)?;
    let by = who(spec.by)?;
    let sha = sha256_hex(&bytes);
    modify(dirs, |st| {
        let client = st.client(spec.client).ok_or_else(|| format!("no client `{}`", spec.client))?;
        if client.archived {
            return Err(format!("client \"{}\" is archived", client.name));
        }
        if let Some(a) = st.assets.iter().find(|a| a.client == spec.client && a.sha256 == sha) {
            return Err(format!("this file is already asset \"{}\" ({}) of the client", a.name, a.id));
        }
        if st.assets.len() >= MAX_ASSETS {
            return Err(format!("at most {MAX_ASSETS} brand assets"));
        }
        let file = format!("{}.{}", sha.get(..16).unwrap_or(&sha), if format == "jpeg" { "jpg" } else { format });
        let dir = dirs.brand.join(ASSET_DIR);
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let path = dir.join(&file);
        if !path.exists() {
            write_atomic(&path, &bytes)?;
        }
        let a = BrandAsset {
            id: new_id("ast", &name),
            client: spec.client.to_string(),
            name,
            kind: kind.into(),
            variant,
            file,
            format: format.into(),
            original_name,
            sha256: sha,
            bytes: bytes.len() as u64,
            width: r.width,
            height: r.height,
            has_alpha: r.has_alpha,
            source,
            license,
            added: now_rfc3339(),
            by,
        };
        st.assets.push(a.clone());
        Ok(a)
    })
}

/// Kit versions that use an asset: (kit id, kit name, version, approved).
pub fn asset_users(st: &Store, id: &str) -> Vec<(String, String, u32, bool)> {
    let mut out = Vec::new();
    for k in &st.kits {
        for v in &k.versions {
            if v.content.logos.iter().any(|l| l.asset == id) {
                out.push((k.id.clone(), k.name.clone(), v.version, v.approved()));
            }
        }
    }
    out
}

/// Remove an asset. Refused while any kit version uses it: an approved one always (it must keep
/// working), a draft until the logo is taken out of the draft.
pub fn remove_asset(dirs: &Dirs, id: &str) -> Result<BrandAsset, String> {
    modify(dirs, |st| {
        let users = asset_users(st, id);
        if !users.is_empty() {
            let list: Vec<String> = users.iter().map(|(_, n, v, a)| format!("{n} v{v} ({})", if *a { "approved" } else { "draft" })).collect();
            return Err(format!("the asset is used by {}; approved kit versions keep their assets, and drafts must drop it first", list.join(", ")));
        }
        let i = st.assets.iter().position(|a| a.id == id).ok_or_else(|| format!("no asset `{id}`"))?;
        let a = st.assets.remove(i);
        if !st.assets.iter().any(|b| b.file == a.file) {
            let _ = std::fs::remove_file(dirs.brand.join(ASSET_DIR).join(&a.file));
        }
        Ok(a)
    })
}

/// Downscaled RGBA of an asset for previews (longest side at most `max_side`).
pub fn asset_rgba(dirs: &Dirs, a: &BrandAsset, max_side: u32) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = std::fs::read(dirs.asset_path(a)).ok()?;
    let r = decode_image(&a.original_name, &bytes).ok()?;
    let scale = (max_side.max(1) as f32 / r.width.max(r.height) as f32).min(1.0);
    let (w, h) = (((r.width as f32 * scale).round() as u32).max(1), ((r.height as f32 * scale).round() as u32).max(1));
    let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
    for y in 0..h {
        for x in 0..w {
            let sx = ((x as f32 + 0.5) / scale) as u32;
            let sy = ((y as f32 + 0.5) / scale) as u32;
            let (sx, sy) = (sx.min(r.width - 1), sy.min(r.height - 1));
            let si = ((sy * r.width + sx) * 4) as usize;
            let di = ((y * w + x) * 4) as usize;
            if let (Some(d), Some(s)) = (out.get_mut(di..di + 4), r.rgba.get(si..si + 4)) {
                d.copy_from_slice(s);
            }
        }
    }
    Some((w, h, out))
}

// ------------------------------------------------------------------ dependency check

/// One problem of a kit version. Errors block approval and pinning.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Issue {
    /// "error" or "warning".
    pub severity: &'static str,
    /// "font", "palette", "color", "typography", "logo", "rule", "kit".
    pub area: &'static str,
    pub detail: String,
}

fn err(area: &'static str, detail: String) -> Issue {
    Issue { severity: "error", area, detail }
}
fn warn(area: &'static str, detail: String) -> Issue {
    Issue { severity: "warning", area, detail }
}

/// The palette versions a kit references, loaded.
fn kit_palettes(dirs: &Dirs, c: &KitContent) -> (Vec<(KitPalette, Option<palette_library::PaletteVersion>)>, Option<String>) {
    let lib = palette_library::load(&dirs.palettes);
    let lib_err = lib.as_ref().err().cloned();
    let out = c
        .palettes
        .iter()
        .map(|p| {
            let v = lib.as_ref().ok().and_then(|l| l.get(&p.palette)).and_then(|pal| pal.version(p.version)).cloned();
            (p.clone(), v)
        })
        .collect();
    (out, lib_err)
}

/// The colour a reference points at, in the kit's palette versions.
pub fn resolve_color(pals: &[(KitPalette, Option<palette_library::PaletteVersion>)], r: &ColorRef) -> Option<palette_library::PaletteColor> {
    pals.iter().find(|(p, _)| p.palette == r.palette).and_then(|(_, v)| v.as_ref()).and_then(|v| v.color(&r.color)).cloned()
}

/// Everything that keeps a kit version from being used as specified: fonts (in the Font Library,
/// registered, exact face and weight), palette versions (present, unaltered), colour references,
/// typography, logos (present, unaltered, this client's), rules (no contradiction of the palette
/// rules), and whether the version itself is intact.
pub fn check(dirs: &Dirs, st: &Store, kit: &BrandKit, v: &KitVersion) -> Vec<Issue> {
    let c = &v.content;
    let mut out = Vec::new();
    if v.approved() && !v.intact() {
        out.push(err("kit", format!("approved version {} does not match its fingerprint (the store was edited outside FilmCraft)", v.version)));
    }
    check_fonts(dirs, c, &mut out);
    // palettes
    let (pals, lib_err) = kit_palettes(dirs, c);
    if let Some(e) = lib_err {
        out.push(err("palette", e));
    }
    for (p, pv) in &pals {
        match pv {
            None => out.push(err("palette", format!("palette `{}` version {} is not in the Palette Library", p.palette, p.version))),
            Some(pv) if pv.sha256 != p.sha256 || !pv.intact() => {
                out.push(err("palette", format!("palette `{}` version {} does not match the fingerprint the kit recorded", p.palette, p.version)))
            }
            Some(_) => {}
        }
    }
    for r in &c.colors {
        color_ok(&pals, &r.color, &format!("colour role \"{}\"", r.role), &mut out);
    }
    check_typography(c, &pals, &mut out);
    check_logos(dirs, st, kit, c, &pals, &mut out);
    check_rules(c, &pals, &mut out);
    if c.typography.is_empty() {
        out.push(warn("typography", "no typography roles".into()));
    }
    if c.palettes.is_empty() {
        out.push(warn("palette", "no palette".into()));
    }
    out
}

fn color_ok(pals: &[(KitPalette, Option<palette_library::PaletteVersion>)], r: &ColorRef, what: &str, out: &mut Vec<Issue>) {
    if resolve_color(pals, r).is_none() {
        out.push(err("color", format!("{what}: colour `{}` of palette `{}` is not in the kit's palette versions", r.color, r.palette)));
    }
}

fn check_fonts(dirs: &Dirs, c: &KitContent, out: &mut Vec<Issue>) {
    // fonts
    let manifest = font_library::read_manifest(&dirs.fonts);
    if let Err(e) = &manifest {
        out.push(err("font", e.clone()));
    }
    for f in &c.fonts {
        let lib = manifest.as_ref().ok().and_then(|m| m.fonts.iter().find(|x| x.id == f.library_font).cloned());
        match lib {
            None => out.push(err("font", format!("{} {} is missing: Font Library font `{}` is not installed", f.family, f.style, f.library_font))),
            Some(lf) if lf.sha256 != f.sha256 => out
                .push(err("font", format!("{} {}: the Font Library file changed (expected SHA-256 {}…)", f.family, f.style, f.sha256.get(..12).unwrap_or("")))),
            Some(lf) => match lf.faces.iter().find(|x| x.family.eq_ignore_ascii_case(&f.family) && x.style.eq_ignore_ascii_case(&f.style)) {
                None => out.push(err("font", format!("{} {} is not a style of {} ({})", f.family, f.style, lf.families().join(", "), lf.original_name))),
                Some(face) if face.weight != f.weight => {
                    out.push(err("font", format!("{} {}: weight {} is not available (the face is weight {})", f.family, f.style, f.weight, face.weight)))
                }
                Some(_) => match filmcraft_text::fonts::status(&f.family, &f.style) {
                    filmcraft_text::fonts::FontStatus::Exact { .. } => {}
                    other => {
                        out.push(err("font", format!("{} {} does not render exactly ({other:?}); restart FilmCraft or re-add the font", f.family, f.style)))
                    }
                },
            },
        }
    }
}

fn check_typography(c: &KitContent, pals: &[(KitPalette, Option<palette_library::PaletteVersion>)], out: &mut Vec<Issue>) {
    // typography
    for t in &c.typography {
        let what = format!("typography \"{}\"", t.role);
        match c.fonts.iter().find(|f| f.family.eq_ignore_ascii_case(&t.family) && f.style.eq_ignore_ascii_case(&t.style)) {
            None => out.push(err("typography", format!("{what}: {} {} is not one of the kit's fonts", t.family, t.style))),
            Some(f) if f.weight != t.weight => {
                out.push(err("typography", format!("{what}: weight {} is not available in {} {} (weight {})", t.weight, t.family, t.style, f.weight)))
            }
            Some(_) => {}
        }
        for (k, r) in [("colour", &t.color), ("background", &t.background), ("accent", &t.accent)] {
            if let Some(r) = r {
                color_ok(pals, r, &format!("{what} {k}"), out);
            }
        }
        if !(t.size.value.is_finite() && t.size.value > 0.0 && t.size.value <= 2000.0) || !["px", "%w", "%h"].contains(&t.size.unit.as_str()) {
            out.push(err("typography", format!("{what}: size must be a positive number in px, %w or %h")));
        }
        if !["left", "center", "right"].contains(&t.align.as_str()) || !["none", "upper", "lower", "title"].contains(&t.case.as_str()) {
            out.push(err("typography", format!("{what}: align must be left/center/right and case none/upper/lower/title")));
        }
        if t.opacity.is_some_and(|o| !(0.0..=1.0).contains(&o)) {
            out.push(err("typography", format!("{what}: opacity must be from 0 to 1")));
        }
    }
    let mut roles: Vec<&str> = c.typography.iter().map(|t| t.role.as_str()).collect();
    roles.sort_unstable();
    if roles.windows(2).any(|w| w[0].eq_ignore_ascii_case(w[1])) {
        out.push(err("typography", "two typography roles have the same name".into()));
    }
}

fn check_logos(dirs: &Dirs, st: &Store, kit: &BrandKit, c: &KitContent, pals: &[(KitPalette, Option<palette_library::PaletteVersion>)], out: &mut Vec<Issue>) {
    // logos
    for l in &c.logos {
        match st.asset(&l.asset) {
            None => out.push(err("logo", format!("logo asset `{}` is missing", l.asset))),
            Some(a) if a.client != kit.client => out.push(err("logo", format!("logo \"{}\" belongs to another client", a.name))),
            Some(a) if a.sha256 != l.sha256 => out.push(err("logo", format!("logo \"{}\" changed since it was added to the kit", a.name))),
            Some(a) => match std::fs::read(dirs.asset_path(a)) {
                Ok(b) if sha256_hex(&b) == a.sha256 => {}
                _ => out.push(err("logo", format!("the file of logo \"{}\" is missing or altered", a.name))),
            },
        }
        for r in &l.backgrounds {
            color_ok(pals, r, "logo background", out);
        }
        if let Some(f) = l.forbidden.iter().find(|f| !ALTERATIONS.contains(&f.as_str())) {
            out.push(err("logo", format!("unknown alteration `{f}` (one of {})", ALTERATIONS.join(", "))));
        }
        if let (Some(a), Some(b)) = (&l.min_width, &l.max_width)
            && a.unit == b.unit
            && a.value > b.value
        {
            out.push(err("logo", "the logo's minimum width is larger than its maximum".into()));
        }
    }
}

fn check_rules(c: &KitContent, pals: &[(KitPalette, Option<palette_library::PaletteVersion>)], out: &mut Vec<Issue>) {
    // rules: kit rules may add, never contradict, the palettes' own rules
    for (i, r) in c.rules.iter().enumerate() {
        if !["permitted", "prohibited", "guideline"].contains(&r.kind.as_str()) {
            out.push(err("rule", format!("rule kind `{}` is not permitted, prohibited or guideline", r.kind)));
            continue;
        }
        if r.kind == "guideline" {
            if r.text.trim().is_empty() {
                out.push(err("rule", "a guideline needs its text".into()));
            }
            continue;
        }
        let (Some(fg), Some(bg)) = (&r.foreground, &r.background) else {
            out.push(err("rule", "a combination rule needs a foreground and a background colour".into()));
            continue;
        };
        color_ok(pals, fg, "rule foreground", out);
        color_ok(pals, bg, "rule background", out);
        let same = |o: &KitRule| o.foreground == r.foreground && o.background == r.background && o.applies_to == r.applies_to && o.kind != "guideline";
        if c.rules[..i].iter().any(|o| same(o) && o.kind != r.kind) {
            out.push(err("rule", "a combination is both permitted and prohibited".into()));
        }
        if let (Some(f), Some(b)) = (resolve_color(pals, fg), resolve_color(pals, bg)) {
            for (p, pv) in pals {
                let Some(pv) = pv.as_ref().filter(|_| p.palette == fg.palette && p.palette == bg.palette) else { continue };
                for pr in &pv.rules {
                    let opposite = matches!(
                        (r.kind.as_str(), pr.kind),
                        ("permitted", palette_library::RuleKind::Prohibited) | ("prohibited", palette_library::RuleKind::Permitted)
                    );
                    let applies = pr.applies_to == r.applies_to || pr.applies_to == "any" || r.applies_to == "any";
                    if opposite && applies && pr.foreground.as_deref() == Some(f.id.as_str()) && pr.background.as_deref() == Some(b.id.as_str()) {
                        out.push(err(
                            "rule",
                            format!("{} on {} contradicts the palette's own rule; the palette's rules cannot be overridden by a kit", f.name, b.name),
                        ));
                    }
                }
            }
        }
    }
}

pub fn has_errors(issues: &[Issue]) -> bool {
    issues.iter().any(|i| i.severity == "error")
}

// ------------------------------------------------------------------ kits

pub fn create_kit(
    dirs: &Dirs,
    client: &str,
    name: &str,
    description: &str,
    by: &str,
    content: KitContent,
    source: Option<KitSource>,
) -> Result<BrandKit, String> {
    let name = clean(name, "a kit name", MAX_NAME, true)?;
    let description = clean(description, "a description", MAX_TEXT, false)?;
    let by = who(by)?;
    limits(&content)?;
    modify(dirs, |st| {
        let c = st.client(client).ok_or_else(|| format!("no client `{client}`"))?;
        if c.archived {
            return Err(format!("client \"{}\" is archived", c.name));
        }
        if st.kits.len() >= MAX_KITS {
            return Err(format!("at most {MAX_KITS} brand kits"));
        }
        if st.kits.iter().any(|k| k.client == client && !k.archived && k.name.eq_ignore_ascii_case(&name)) {
            return Err(format!("the client already has a kit named \"{name}\""));
        }
        let now = now_rfc3339();
        let sha256 = content.fingerprint();
        let v = KitVersion {
            version: 1,
            status: "draft".into(),
            created: now.clone(),
            updated: now.clone(),
            by: by.clone(),
            note: String::new(),
            based_on: None,
            content,
            sha256,
            approval: None,
            source: source.clone(),
        };
        let mut k = BrandKit {
            id: new_id("kit", &name),
            client: client.to_string(),
            name,
            description,
            archived: false,
            source,
            created: now.clone(),
            updated: now,
            versions: vec![v],
            history: Vec::new(),
        };
        let k2 = &mut k;
        log(&mut k2.history, &mut k2.updated, &by, "created", "draft version 1".into());
        st.kits.push(k.clone());
        Ok(k)
    })
}

fn limits(c: &KitContent) -> Result<(), String> {
    let n = [c.fonts.len(), c.palettes.len(), c.colors.len(), c.typography.len(), c.logos.len(), c.rules.len()];
    if n.iter().any(|n| *n > MAX_ITEMS) {
        return Err(format!("a kit holds at most {MAX_ITEMS} fonts, palettes, colours, roles, logos and rules each"));
    }
    if c.notes.len() > MAX_TEXT {
        return Err("kit notes are too long".into());
    }
    Ok(())
}

/// What [`save_draft`] did.
pub struct Saved {
    pub kit: BrandKit,
    pub version: u32,
    /// A new draft version was started from an approved one.
    pub new_version: bool,
}

/// Save draft content. `base` must be the kit's latest version: a draft is replaced (the save is
/// logged), an approved version is left as it is and a new draft version starts from it.
/// Approved versions are never changed; kits managed by ARCA are not edited here.
pub fn save_draft(dirs: &Dirs, id: &str, base: u32, content: KitContent, by: &str, note: &str) -> Result<Saved, String> {
    let by = who(by)?;
    let note = clean(note, "a note", MAX_TEXT, false)?;
    limits(&content)?;
    modify(dirs, |st| {
        let k = st.kits.iter_mut().find(|k| k.id == id).ok_or_else(|| format!("no brand kit `{id}`"))?;
        if let Some(src) = &k.source {
            return Err(format!("this kit is managed by {} ({}); change it there and import the new version", src.authority, src.format));
        }
        if k.archived {
            return Err("the kit is archived; restore it first".into());
        }
        let latest = k.latest().cloned().ok_or("the kit has no versions")?;
        if latest.version != base {
            return Err(format!("the kit changed since version {base} (latest is {}); reload it and reapply the changes", latest.version));
        }
        let sha256 = content.fingerprint();
        let now = now_rfc3339();
        if latest.approved() {
            if k.versions.len() >= MAX_VERSIONS {
                return Err(format!("the kit has {MAX_VERSIONS} versions"));
            }
            let version = latest.version.saturating_add(1);
            k.versions.push(KitVersion {
                version,
                status: "draft".into(),
                created: now.clone(),
                updated: now,
                by: by.clone(),
                note: note.clone(),
                based_on: Some(latest.version),
                content,
                sha256,
                approval: None,
                source: None,
            });
            log(
                &mut k.history,
                &mut k.updated,
                &by,
                "draft",
                format!("version {version} started from approved version {}{}", latest.version, note_suffix(&note)),
            );
            return Ok(Saved { kit: k.clone(), version, new_version: true });
        }
        let v = k.versions.last_mut().ok_or("the kit has no versions")?;
        if v.sha256 != sha256 || v.content != content {
            v.content = content;
            v.sha256 = sha256.clone();
            v.updated = now;
            v.by.clone_from(&by);
            if !note.is_empty() {
                v.note.clone_from(&note);
            }
            let version = v.version;
            log(&mut k.history, &mut k.updated, &by, "saved", format!("draft version {version} ({}){}", sha256.get(..12).unwrap_or(""), note_suffix(&note)));
        }
        Ok(Saved { kit: k.clone(), version: latest.version, new_version: false })
    })
}

fn note_suffix(n: &str) -> String {
    if n.is_empty() { String::new() } else { format!(": {n}") }
}

/// Approve the draft `version`: refused when it has errors, when `by` or `reference` are missing,
/// or when the version is not the kit's current draft. The version is then frozen.
pub fn approve(dirs: &Dirs, id: &str, version: u32, by: &str, reference: &str, authority: &str) -> Result<BrandKit, String> {
    let by = clean(by, "the approver (`approval.by`)", MAX_NAME, true)?;
    let reference =
        clean(reference, "the approval reference (`approval.reference`: the decision, ticket or message that approves this version)", MAX_TEXT, true)?;
    let st0 = load(dirs)?;
    let kit = st0.kit(id).ok_or_else(|| format!("no brand kit `{id}`"))?;
    let v = kit.version(version).ok_or_else(|| format!("the kit has no version {version}"))?;
    if v.approved() {
        return Err(format!("version {version} is already approved"));
    }
    if kit.latest().map(|l| l.version) != Some(version) {
        return Err(format!("only the latest draft can be approved (version {version} is not the latest)"));
    }
    if kit.source.is_some() && authority != "ARCA" {
        return Err("this kit is managed by ARCA: it is approved in ARCA and imported with that approval".into());
    }
    let issues = check(dirs, &st0, kit, v);
    if has_errors(&issues) {
        let list: Vec<String> = issues.iter().filter(|i| i.severity == "error").map(|i| i.detail.clone()).collect();
        return Err(format!("the draft cannot be approved: {}", list.join("; ")));
    }
    modify(dirs, |st| {
        let k = st.kits.iter_mut().find(|k| k.id == id).ok_or_else(|| format!("no brand kit `{id}`"))?;
        let v = k.versions.iter_mut().find(|x| x.version == version).ok_or("version vanished")?;
        if v.approved() {
            return Err(format!("version {version} is already approved"));
        }
        v.status = "approved".into();
        v.sha256 = v.content.fingerprint();
        v.approval = Some(Approval { by: by.clone(), reference: reference.clone(), at: now_rfc3339(), authority: authority.to_string() });
        log(&mut k.history, &mut k.updated, &by, "approved", format!("version {version} ({}) — {reference}", authority));
        Ok(k.clone())
    })
}

pub fn update_kit_info(dirs: &Dirs, id: &str, name: Option<&str>, description: Option<&str>, archived: Option<bool>, by: &str) -> Result<BrandKit, String> {
    let by = who(by)?;
    let name = name.map(|n| clean(n, "a kit name", MAX_NAME, true)).transpose()?;
    let description = description.map(|n| clean(n, "a description", MAX_TEXT, false)).transpose()?;
    modify(dirs, |st| {
        let clash = name.as_ref().and_then(|n| {
            let me = st.kit(id)?;
            st.kits.iter().find(|k| k.id != id && k.client == me.client && !k.archived && k.name.eq_ignore_ascii_case(n)).map(|k| k.name.clone())
        });
        if let Some(n) = clash {
            return Err(format!("the client already has a kit named \"{n}\""));
        }
        let k = st.kits.iter_mut().find(|k| k.id == id).ok_or_else(|| format!("no brand kit `{id}`"))?;
        if let Some(n) = name.filter(|n| *n != k.name) {
            let old = std::mem::replace(&mut k.name, n.clone());
            log(&mut k.history, &mut k.updated, &by, "renamed", format!("\"{old}\" → \"{n}\""));
        }
        if let Some(d) = description.filter(|d| *d != k.description) {
            k.description = d;
            log(&mut k.history, &mut k.updated, &by, "description", String::new());
        }
        if let Some(a) = archived.filter(|a| *a != k.archived) {
            k.archived = a;
            log(&mut k.history, &mut k.updated, &by, if a { "archived" } else { "restored" }, String::new());
        }
        Ok(k.clone())
    })
}

// ------------------------------------------------------------------ references held by approved kits

/// Approved kit versions that use a Font Library font: "Kit v2".
pub fn font_users(dirs: &Dirs, font_id: &str) -> Vec<String> {
    let Ok(st) = load(dirs) else { return vec!["(the brand kit store cannot be read)".into()] };
    st.kits
        .iter()
        .flat_map(|k| {
            k.versions
                .iter()
                .filter(|v| v.approved() && v.content.fonts.iter().any(|f| f.library_font == font_id))
                .map(move |v| format!("{} v{}", k.name, v.version))
        })
        .collect()
}

/// Approved kit versions that use a palette.
pub fn palette_users(dirs: &Dirs, palette: &str) -> Vec<String> {
    let Ok(st) = load(dirs) else { return vec!["(the brand kit store cannot be read)".into()] };
    st.kits
        .iter()
        .flat_map(|k| {
            k.versions
                .iter()
                .filter(|v| v.approved() && v.content.palettes.iter().any(|p| p.palette == palette))
                .map(move |v| format!("{} v{}", k.name, v.version))
        })
        .collect()
}

// ------------------------------------------------------------------ resolution

/// A kit version with every reference resolved to exact values: font files and faces, palette
/// colours (hex, RGB) and their rules, logo files and sizes, plus the dependency check.
pub fn resolved_json(dirs: &Dirs, st: &Store, kit: &BrandKit, v: &KitVersion) -> Value {
    let c = &v.content;
    let manifest = font_library::read_manifest(&dirs.fonts).ok();
    let (pals, _) = kit_palettes(dirs, c);
    let col = |r: &ColorRef| match resolve_color(&pals, r) {
        Some(pc) => json!({"palette": r.palette, "color": pc.id, "name": pc.name, "hex": pc.hex.hex(), "rgb": pc.hex.0, "role": pc.role}),
        None => json!({"palette": r.palette, "color": r.color, "missing": true}),
    };
    let ocol = |r: &Option<ColorRef>| r.as_ref().map_or(Value::Null, col);
    let fonts: Vec<Value> = c
        .fonts
        .iter()
        .map(|f| {
            let lf = manifest.as_ref().and_then(|m| m.fonts.iter().find(|x| x.id == f.library_font));
            json!({"family": f.family, "style": f.style, "weight": f.weight, "libraryFont": f.library_font, "sha256": f.sha256,
                   "file": lf.map(|l| dirs.fonts.join(&l.file).display().to_string()), "license": lf.map(|l| l.license.spdx.clone()),
                   "status": format!("{:?}", filmcraft_text::fonts::status(&f.family, &f.style)).split_whitespace().next().unwrap_or("").to_string()})
        })
        .collect();
    let palettes: Vec<Value> = pals
        .iter()
        .map(|(p, pv)| {
            json!({"palette": p.palette, "version": p.version, "name": p.name, "sha256": p.sha256, "ref": format!("{}@{}", p.palette, p.version),
                   "present": pv.is_some(), "colors": pv.as_ref().map(|v| v.colors.iter().map(palette_library::color_json).collect::<Vec<_>>()),
                   "rules": pv.as_ref().map(|v| v.rules.iter().map(|r| palette_library::rule_json(r, v)).collect::<Vec<_>>())})
        })
        .collect();
    let typography: Vec<Value> = c
        .typography
        .iter()
        .map(|t| {
            let mut j = serde_json::to_value(t).unwrap_or(Value::Null);
            j["color"] = ocol(&t.color);
            j["background"] = ocol(&t.background);
            j["accent"] = ocol(&t.accent);
            if let Some([w, h]) = c.reference_frame {
                j["sizePxAtReference"] = json!(t.size.px(f64::from(w), f64::from(h)));
            }
            j
        })
        .collect();
    let logos: Vec<Value> = c
        .logos
        .iter()
        .map(|l| {
            let a = st.asset(&l.asset);
            let mut j = serde_json::to_value(l).unwrap_or(Value::Null);
            j["backgrounds"] = json!(l.backgrounds.iter().map(col).collect::<Vec<_>>());
            j["file"] = json!(a.map(|a| dirs.asset_path(a).display().to_string()));
            j["name"] = json!(a.map(|a| a.name.clone()));
            j["format"] = json!(a.map(|a| a.format.clone()));
            j["pixels"] = json!(a.map(|a| [a.width, a.height]));
            j
        })
        .collect();
    let colors: Vec<Value> = c.colors.iter().map(|r| json!({"role": r.role, "color": col(&r.color), "notes": r.notes})).collect();
    let rules: Vec<Value> = c
        .rules
        .iter()
        .map(|r| json!({"id": r.id, "kind": r.kind, "foreground": ocol(&r.foreground), "background": ocol(&r.background), "appliesTo": r.applies_to, "text": r.text}))
        .collect();
    let client = st.client(&kit.client);
    let issues = check(dirs, st, kit, v);
    json!({
        "kit": kit.id, "name": kit.name, "client": kit.client, "clientName": client.map(|c| c.name.clone()),
        "ref": format!("{}@{}", kit.id, v.version), "version": v.version, "status": v.status, "sha256": v.sha256, "intact": v.intact(),
        "approval": v.approval, "source": v.source.as_ref().or(kit.source.as_ref()), "by": v.by, "note": v.note, "basedOn": v.based_on,
        "fonts": fonts, "palettes": palettes, "colors": colors, "typography": typography, "logos": logos, "rules": rules,
        "safeArea": c.safe_area, "referenceFrame": c.reference_frame, "notes": c.notes,
        "issues": issues, "ready": !has_errors(&issues),
    })
}

pub fn kit_summary(st: &Store, k: &BrandKit) -> Value {
    let latest = k.latest();
    json!({"id": k.id, "name": k.name, "client": k.client, "clientName": st.client(&k.client).map(|c| c.name.clone()), "description": k.description,
           "archived": k.archived, "managedBy": k.source.as_ref().map(|s| s.authority.clone()),
           "latestVersion": latest.map(|v| v.version), "latestStatus": latest.map(|v| v.status.clone()),
           "approvedVersion": k.latest_approved().map(|v| v.version), "updated": k.updated,
           "versions": k.versions.iter().map(|v| json!({"version": v.version, "status": v.status, "created": v.created, "updated": v.updated, "by": v.by, "note": v.note,
               "sha256": v.sha256, "approval": v.approval, "basedOn": v.based_on})).collect::<Vec<_>>()})
}
