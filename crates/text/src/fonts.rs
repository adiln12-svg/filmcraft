//! The font database: bundled fonts (always available, also on the web), the optional
//! [`CRAFT_FONTS`] (present when built with `CRAFT_FONTS_DIR`, also on the web) plus fonts
//! discovered by [`FontSource`]s (system font folders on native platforms).
//!
//! Faces are addressed by [`FaceId`] (an index that never changes for the life of the process).
//! [`resolve`] maps a family + style name (as stored in projects) to a face and the synthetic
//! bold/italic needed when the family lacks that style. Unknown families fall back to Inter.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};

use crate::sfnt::{FaceNames, read_faces, read_faces_bytes};

/// Bundled fonts (SIL OFL 1.1; see `assets/fonts/*.attribution` and ATTRIBUTION.md).
pub static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
pub static INTER_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Inter-Medium.ttf");
pub static INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
pub static INTER_BOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-Bold.ttf");
pub static INTER_ITALIC: &[u8] = include_bytes!("../../../assets/fonts/Inter-Italic.ttf");
pub static JETBRAINS_MONO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
pub static NOTO_SERIF_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/NotoSerif-Regular.ttf");

/// A font from the optional craft-fonts build input (empty unless built with `CRAFT_FONTS_DIR`; see
/// `build.rs` and storytold/craft-fonts `docs/integration.md`).
pub struct CraftFont {
    pub family: &'static str,
    pub style: &'static str,
    /// ISO 15924 scripts the font is for, e.g. `"Jpan"`.
    pub scripts: &'static [&'static str],
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/craft_fonts.rs"));

#[cfg(test)]
mod craft_wasm_tests {
    use super::*;

    /// The web build embeds the UI font and Noto Sans Thai, so Thai names draw on a browser
    /// without a Thai font (#3374). Skipped without craft-fonts.
    #[test]
    fn web_build_embeds_the_ui_font_and_the_thai_face() {
        if CRAFT_FONTS.is_empty() {
            eprintln!("skipped: built without craft-fonts");
            return;
        }
        assert!(CRAFT_WASM_FACES.contains(&("BIZ UDPGothic", "Regular")), "{CRAFT_WASM_FACES:?}");
        assert!(CRAFT_WASM_FACES.contains(&("Noto Sans Thai", "Regular")), "{CRAFT_WASM_FACES:?}");
        // every listed face is a real craft-fonts entry, and the Thai one carries the Thai script
        for (family, style) in CRAFT_WASM_FACES {
            assert!(CRAFT_FONTS.iter().any(|f| f.family == *family && f.style == *style), "{family} {style}");
        }
        assert!(CRAFT_FONTS.iter().any(|f| f.family == "Noto Sans Thai" && f.scripts.contains(&"Thai")));
    }
}

/// The craft-fonts entries for Japanese (`Jpan`), in [`CRAFT_FONTS`] order. Empty when the app was
/// built without craft-fonts.
pub fn craft_japanese() -> impl Iterator<Item = &'static CraftFont> {
    CRAFT_FONTS.iter().filter(|f| f.scripts.contains(&"Jpan"))
}

/// Origin of faces registered from [`CRAFT_FONTS`].
pub const CRAFT_ORIGIN: &str = "craft-fonts";

/// The default family for new text.
pub const DEFAULT_FAMILY: &str = "Inter";

/// Index of a face in the database.
pub type FaceId = usize;

/// Where a face's bytes come from.
#[derive(Clone, Debug)]
pub enum FaceData {
    Static(&'static [u8]),
    Shared(Arc<Vec<u8>>),
    /// A file read on first use.
    File(PathBuf),
}

/// Description of a face offered by a [`FontSource`].
#[derive(Clone, Debug)]
pub struct FaceInfo {
    pub family: String,
    pub style: String,
    pub weight: u16,
    pub italic: bool,
    pub index: u32,
    pub data: FaceData,
    /// "bundled", "system", …
    pub origin: &'static str,
    /// For a variable font, the named instance this face is ("Black" of Archivo): its axis
    /// location is used for metrics, outlines and shaping. None for static fonts and for the
    /// default instance of a variable font that has no named instances.
    pub instance: Option<u16>,
}

/// Something that can list fonts (system folders, a web font picker, a project's font folder…).
pub trait FontSource: Send + Sync {
    fn faces(&self) -> Vec<FaceInfo>;
}

/// Scans font folders on disk (reading only name tables). Native only; on the web there are no
/// folders and the scan finds nothing.
pub struct DirectorySource {
    pub dirs: Vec<PathBuf>,
}

impl DirectorySource {
    /// The platform's standard font folders.
    pub fn system() -> Self {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        // Fonts activated through Adobe Fonts (Creative Cloud) live in a CoreSync folder that the
        // OS font manager lists but the standard folders don't hold (#1878); on Windows the files
        // there have no extension.
        if cfg!(target_os = "macos") {
            dirs.extend(["/System/Library/Fonts", "/Library/Fonts"].map(PathBuf::from));
            if let Some(h) = &home {
                dirs.push(h.join("Library/Fonts"));
                dirs.push(h.join("Library/Application Support/Adobe/CoreSync/plugins/livetype"));
            }
        } else if cfg!(windows) {
            let win = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
            dirs.push(win.join("Fonts"));
            if let Some(l) = std::env::var_os("LOCALAPPDATA") {
                dirs.push(PathBuf::from(l).join("Microsoft\\Windows\\Fonts"));
            }
            if let Some(a) = std::env::var_os("APPDATA") {
                dirs.push(PathBuf::from(a).join("Adobe\\CoreSync\\plugins\\livetype"));
            }
        } else if cfg!(unix) && !cfg!(target_arch = "wasm32") {
            dirs.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(PathBuf::from));
            if let Some(h) = &home {
                dirs.push(h.join(".local/share/fonts"));
                dirs.push(h.join(".fonts"));
            }
        }
        Self { dirs }
    }
}

fn walk(dir: &std::path::Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(file_type) = e.file_type() else { continue };
        // Font discovery is bounded to the configured folders: never follow a symlinked folder
        // into a loop or an unrelated tree. Symlinked font files (NixOS, home-manager, Debian
        // alternatives) still count.
        let is_file = if file_type.is_symlink() { std::fs::metadata(&p).is_ok_and(|m| m.is_file()) } else { file_type.is_file() };
        if file_type.is_dir() {
            if depth < 4 {
                walk(&p, depth + 1, out);
            }
        } else if is_file && is_font_file(&p) {
            out.push(p);
        }
    }
}

/// A font file by extension, or an extensionless file that starts with an OpenType / TrueType /
/// collection tag (Adobe Fonts stores its files without an extension on Windows).
fn is_font_file(p: &std::path::Path) -> bool {
    match p.extension().and_then(OsStr::to_str) {
        Some(x) => matches!(x.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc" | "otc"),
        None => {
            let mut tag = [0u8; 4];
            std::fs::File::open(p).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut tag)).is_ok()
                && matches!(&tag, [0, 1, 0, 0] | b"OTTO" | b"ttcf" | b"true")
        }
    }
}

#[cfg(all(test, unix))]
mod directory_tests {
    use super::walk;

    #[test]
    fn directory_source_does_not_follow_symlinked_font_trees() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let configured = root.path().join("configured");
        let foreign = root.path().join("foreign");
        std::fs::create_dir_all(&configured).unwrap();
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(configured.join("inside.ttf"), b"").unwrap();
        std::fs::write(foreign.join("outside.ttf"), b"").unwrap();
        symlink(&foreign, configured.join("loop")).unwrap();
        symlink(foreign.join("outside.ttf"), configured.join("linked.ttf")).unwrap();

        let mut found = Vec::new();
        walk(&configured, 0, &mut found);
        found.sort();
        assert_eq!(found, vec![configured.join("inside.ttf"), configured.join("linked.ttf")]);
    }
}

impl FontSource for DirectorySource {
    fn faces(&self) -> Vec<FaceInfo> {
        let mut files = Vec::new();
        for d in &self.dirs {
            walk(d, 0, &mut files);
        }
        files.sort();
        let mut out = Vec::new();
        for f in files {
            let Ok(mut file) = std::fs::File::open(&f) else { continue };
            for n in read_faces(&mut file) {
                out.push(info_from(n, FaceData::File(f.clone()), "system"));
            }
        }
        out
    }
}

fn info_from(n: FaceNames, data: FaceData, origin: &'static str) -> FaceInfo {
    FaceInfo { family: n.family, style: n.style, weight: n.weight, italic: n.italic, index: n.index, data, origin, instance: None }
}

/// A loaded (or loadable) face.
pub struct Face {
    pub id: FaceId,
    pub info: FaceInfo,
    bytes: OnceLock<Option<FaceBytes>>,
    shaper: OnceLock<harfrust::ShaperData>,
    location: OnceLock<skrifa::instance::Location>,
    shaper_instance: OnceLock<Option<harfrust::ShaperInstance>>,
    /// Retired (its font was removed from the library): no longer resolved or listed.
    retired: std::sync::atomic::AtomicBool,
}

#[derive(Clone)]
enum FaceBytes {
    Static(&'static [u8]),
    Shared(Arc<Vec<u8>>),
}

impl FaceBytes {
    fn get(&self) -> &[u8] {
        match self {
            FaceBytes::Static(b) => b,
            FaceBytes::Shared(b) => b,
        }
    }
}

/// Vertical metrics at a pixel size (all positive distances, y down from the baseline for the
/// underline and strikethrough positions, which are the top edges of their strokes).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub cap_height: f32,
    pub x_height: f32,
    pub underline_pos: f32,
    pub underline_thickness: f32,
    pub strikethrough_pos: f32,
    pub strikethrough_thickness: f32,
}

impl Face {
    fn new(id: FaceId, info: FaceInfo) -> Self {
        Face {
            id,
            info,
            bytes: OnceLock::new(),
            shaper: OnceLock::new(),
            location: OnceLock::new(),
            shaper_instance: OnceLock::new(),
            retired: Default::default(),
        }
    }
    /// Whether the face was retired with [`retire_faces`].
    pub fn is_retired(&self) -> bool {
        self.retired.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Where in the font's design space this face sits: the named instance's axis location for
    /// a variable-font instance, the default location otherwise.
    pub fn location(&self) -> LocationRef<'_> {
        let loc = self.location.get_or_init(|| match (self.info.instance, self.font()) {
            (Some(i), Some(f)) => f.named_instances().get(i as usize).map(|n| n.location()).unwrap_or_else(|| skrifa::instance::Location::new(0)),
            _ => skrifa::instance::Location::new(0),
        });
        LocationRef::from(loc)
    }
    /// The shaping instance of a variable-font instance (None: the default instance).
    pub(crate) fn shaper_instance(&self) -> Option<&harfrust::ShaperInstance> {
        self.shaper_instance
            .get_or_init(|| match (self.info.instance, self.font()) {
                (Some(i), Some(f)) => Some(harfrust::ShaperInstance::from_named_instance(&f, i as usize)),
                _ => None,
            })
            .as_ref()
    }
    fn bytes(&self) -> Option<&[u8]> {
        self.bytes
            .get_or_init(|| match &self.info.data {
                FaceData::Static(b) => Some(FaceBytes::Static(b)),
                FaceData::Shared(b) => Some(FaceBytes::Shared(b.clone())),
                FaceData::File(p) => std::fs::read(p).ok().map(|v| FaceBytes::Shared(Arc::new(v))),
            })
            .as_ref()
            .map(FaceBytes::get)
    }
    /// A copy of the font file's bytes (None if it is missing or unreadable).
    pub fn data(&self) -> Option<Vec<u8>> {
        self.bytes().map(<[u8]>::to_vec)
    }
    /// The parsed font (None if the file is missing or unreadable).
    pub fn font(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(self.bytes()?, self.info.index).ok()
    }
    pub(crate) fn shaper_data(&self) -> Option<&harfrust::ShaperData> {
        let f = self.font()?;
        Some(self.shaper.get_or_init(|| harfrust::ShaperData::new(&f)))
    }
    pub fn units_per_em(&self) -> f32 {
        self.font().map(|f| f.metrics(Size::unscaled(), self.location()).units_per_em as f32).filter(|u| *u > 0.0).unwrap_or(1000.0)
    }
    /// Glyph for a character (None when the face does not cover it).
    pub fn glyph(&self, c: char) -> Option<u32> {
        self.font()?.charmap().map(c).map(|g| g.to_u32()).filter(|g| *g != 0)
    }
    pub fn has_char(&self, c: char) -> bool {
        self.glyph(c).is_some()
    }
    /// Whether the face covers every character of `text`. Unlike [`Face::has_char`], a face whose
    /// file is not loaded yet is read into a temporary buffer that is dropped afterwards, so
    /// probing many system faces does not keep every font file in memory.
    pub fn covers_text(&self, text: &str) -> bool {
        if let (FaceData::File(p), None) = (&self.info.data, self.bytes.get()) {
            let Ok(data) = std::fs::read(p) else { return false };
            let Ok(font) = FontRef::from_index(&data, self.info.index) else { return false };
            let charmap = font.charmap();
            return text.chars().all(|c| charmap.map(c).is_some_and(|g| g.to_u32() != 0));
        }
        text.chars().all(|c| self.has_char(c))
    }
    /// Height of the glyph's vertical origin above its baseline, in pixels.
    /// OpenType VORG takes precedence; TrueType uses the ink top plus vmtx's top bearing.
    pub(crate) fn vertical_origin(&self, gid: u32, px: f32) -> Option<f32> {
        use skrifa::raw::TableProvider;
        let font = self.font()?;
        let glyph = skrifa::GlyphId::new(gid);
        let units = match font.vorg() {
            Ok(table) => f32::from(table.vertical_origin_y(glyph)),
            Err(_) => {
                let bounds = font.glyph_metrics(Size::unscaled(), self.location()).bounds(glyph)?;
                bounds.y_max + f32::from(font.vmtx().ok()?.side_bearing(glyph)?)
            }
        };
        let origin = units * px / self.units_per_em();
        (origin.is_finite() && origin.abs() < px * 4.0).then_some(origin)
    }

    pub fn metrics(&self, px: f32) -> VMetrics {
        let Some(f) = self.font() else {
            return VMetrics {
                ascent: px * 0.8,
                descent: px * 0.2,
                cap_height: px * 0.7,
                x_height: px * 0.5,
                underline_pos: px * 0.1,
                underline_thickness: px * 0.06,
                strikethrough_pos: -px * 0.28,
                strikethrough_thickness: px * 0.06,
                ..Default::default()
            };
        };
        let m = f.metrics(Size::new(px), self.location());
        let x_height = m.x_height.unwrap_or(m.ascent * 0.5);
        // OS/2 strikeout: offset of the stroke's top above the baseline. Fonts that leave it zero
        // (or put it below the baseline) get a stroke centred on half the x-height.
        let strike_t = m.strikeout.map_or(px * 0.06, |d| d.thickness).max(px * 0.03);
        let strike_top = m.strikeout.map(|d| d.offset).filter(|o| *o > 0.0 && o.is_finite()).unwrap_or(x_height * 0.5 + strike_t * 0.5);
        VMetrics {
            ascent: m.ascent,
            descent: -m.descent,
            line_gap: m.leading,
            cap_height: m.cap_height.unwrap_or(m.ascent * 0.7),
            x_height,
            underline_pos: m.underline.map_or(px * 0.1, |u| -u.offset),
            underline_thickness: m.underline.map_or(px * 0.06, |u| u.thickness.max(px * 0.03)),
            strikethrough_pos: -strike_top,
            strikethrough_thickness: strike_t,
        }
    }
    /// Whether the face's GSUB offers an OpenType feature (e.g. `smcp`).
    pub fn has_feature(&self, tag: &[u8; 4]) -> bool {
        use skrifa::raw::TableProvider;
        let Some(f) = self.font() else { return false };
        let Ok(gsub) = f.gsub() else { return false };
        let Ok(list) = gsub.feature_list() else { return false };
        list.feature_records().iter().any(|r| r.feature_tag().to_be_bytes() == *tag)
    }
}

struct Db {
    faces: Vec<Arc<Face>>,
    /// Served by [`face`] if no face is registered (the bundled faces always are).
    fallback: Arc<Face>,
    scanned: bool,
}

fn db() -> &'static RwLock<Db> {
    static DB: OnceLock<RwLock<Db>> = OnceLock::new();
    DB.get_or_init(|| {
        let info = FaceInfo {
            family: "Inter".into(),
            style: "Regular".into(),
            weight: 400,
            italic: false,
            index: 0,
            data: FaceData::Static(INTER_REGULAR),
            origin: "bundled",
            instance: None,
        };
        let fallback = Arc::new(Face::new(0, info));
        let mut db = Db { faces: Vec::new(), fallback, scanned: false };
        for b in [INTER_REGULAR, INTER_MEDIUM, INTER_SEMIBOLD, INTER_BOLD, INTER_ITALIC, JETBRAINS_MONO_REGULAR, NOTO_SERIF_REGULAR] {
            for n in read_faces_bytes(b) {
                push(&mut db, info_from(n, FaceData::Static(b), "bundled"));
            }
        }
        // after the bundled faces, before anything a FontSource finds (a system copy of the same
        // family + style is then skipped as a duplicate)
        for f in CRAFT_FONTS {
            for n in read_faces_bytes(f.bytes) {
                for i in expand_instances(info_from(n, FaceData::Static(f.bytes), CRAFT_ORIGIN), f.bytes) {
                    push(&mut db, i);
                }
            }
        }
        RwLock::new(db)
    })
}

fn push(db: &mut Db, info: FaceInfo) -> FaceId {
    let id = db.faces.len();
    db.faces.push(Arc::new(Face::new(id, info)));
    bump_generation();
    id
}

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn bump_generation() {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Changes whenever faces are added or retired: caches of resolved fonts (layouts) key on it, so
/// text laid out while a font was missing is laid out again once it is installed.
pub fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// Add faces from a source (skipping family+style pairs already present). Returns how many were
/// added.
pub fn add_source(src: &dyn FontSource) -> usize {
    let faces = src.faces();
    let mut d = db().write().unwrap_or_else(PoisonError::into_inner);
    let mut n = 0;
    for f in faces {
        let dup = d.faces.iter().any(|g| g.info.family.eq_ignore_ascii_case(&f.family) && g.info.style.eq_ignore_ascii_case(&f.style));
        if !dup {
            push(&mut d, f);
            n += 1;
        }
    }
    n
}

/// Register a font from memory (e.g. a font file the user picked on the web). Returns its faces.
/// WOFF and WOFF2 files are decoded first; a variable font registers one face per named
/// instance.
pub fn add_font_data(data: Vec<u8>) -> Vec<FaceId> {
    add_font_data_from(data, "user")
}

/// [`add_font_data`] with the origin the faces report ("user", "library", …).
pub fn add_font_data_from(data: Vec<u8>, origin: &'static str) -> Vec<FaceId> {
    let Ok(data) = decode_container(data) else { return Vec::new() };
    let data = Arc::new(data);
    let infos: Vec<FaceInfo> =
        read_faces_bytes(&data).into_iter().flat_map(|n| expand_instances(info_from(n, FaceData::Shared(data.clone()), origin), &data)).collect();
    let mut d = db().write().unwrap_or_else(PoisonError::into_inner);
    infos.into_iter().map(|i| push(&mut d, i)).collect()
}

/// The faces a font file would register, without registering them (for previews and imports).
pub fn describe(data: &[u8]) -> Result<Vec<FaceInfo>, String> {
    let sfnt = decode_container(data.to_vec())?;
    let shared = Arc::new(sfnt);
    let infos: Vec<FaceInfo> =
        read_faces_bytes(&shared).into_iter().flat_map(|n| expand_instances(info_from(n, FaceData::Shared(shared.clone()), "preview"), &shared)).collect();
    if infos.is_empty() { Err("no usable font faces in the file (it needs outlines and a name table)".into()) } else { Ok(infos) }
}

/// The origin of faces registered from the persistent font library.
pub const LIBRARY_ORIGIN: &str = "library";

/// Largest font file accepted (decoded size included).
pub const MAX_FONT_BYTES: usize = 64 * 1024 * 1024;

/// The format of a font file from its first bytes: "woff2", "woff", "ttc", "otf", "ttf".
pub fn container_format(data: &[u8]) -> Option<&'static str> {
    match data.get(0..4)? {
        b"wOF2" => Some("woff2"),
        b"wOFF" => Some("woff"),
        b"ttcf" => Some("ttc"),
        b"OTTO" => Some("otf"),
        [0, 1, 0, 0] | b"true" => Some("ttf"),
        _ => None,
    }
}

/// An SFNT (TrueType / OpenType / collection) font from a font file, decoding WOFF and WOFF2.
/// Decoding runs under `catch_unwind`: a hostile file is an error, never a crash.
pub fn decode_container(data: Vec<u8>) -> Result<Vec<u8>, String> {
    if data.len() > MAX_FONT_BYTES {
        return Err(format!("font file too large ({} bytes, max {MAX_FONT_BYTES})", data.len()));
    }
    let fmt = container_format(&data).ok_or("not a font file (TrueType, OpenType, WOFF or WOFF2)")?;
    let decode = |f: fn(&[u8]) -> Result<Vec<u8>, wuff::WuffErr>, what: &str| -> Result<Vec<u8>, String> {
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&data))).map_err(|_| format!("the {what} file could not be decoded"))?;
        let out = out.map_err(|e| format!("the {what} file could not be decoded: {e:?}"))?;
        if out.len() > MAX_FONT_BYTES || container_format(&out).is_none_or(|f| f.starts_with("woff")) {
            return Err(format!("the {what} file decodes to an unusable font"));
        }
        Ok(out)
    };
    match fmt {
        "woff2" => decode(wuff::decompress_woff2, "WOFF2"),
        "woff" => decode(wuff::decompress_woff1, "WOFF"),
        _ => Ok(data),
    }
}

/// The licence description and licence URL a font file states in its name table (IDs 13, 14).
pub fn license_info(data: &[u8]) -> (Option<String>, Option<String>) {
    let Ok(sfnt) = decode_container(data.to_vec()) else { return (None, None) };
    let Ok(font) = FontRef::from_index(&sfnt, 0) else { return (None, None) };
    let get = |id| font.localized_strings(id).english_or_first().map(|s| s.to_string()).filter(|s| !s.trim().is_empty());
    (get(skrifa::string::StringId::LICENSE_DESCRIPTION), get(skrifa::string::StringId::LICENSE_URL))
}

/// Whether a font file is variable (has an `fvar` table with axes) and its axis tags.
pub fn variable_axes(data: &[u8]) -> Vec<String> {
    let Ok(sfnt) = decode_container(data.to_vec()) else { return Vec::new() };
    let Ok(font) = FontRef::from_index(&sfnt, 0) else { return Vec::new() };
    font.axes().iter().map(|a| String::from_utf8_lossy(&a.tag().to_be_bytes()).into_owned()).collect()
}

/// A variable font's named instances as faces (Thin … Black), each with its own style name and
/// weight; a static font (or a variable one without named instances) as itself.
fn expand_instances(base: FaceInfo, bytes: &[u8]) -> Vec<FaceInfo> {
    let Ok(font) = FontRef::from_index(bytes, base.index) else { return vec![base] };
    let instances = font.named_instances();
    if instances.is_empty() {
        return vec![base];
    }
    let axes = font.axes();
    let axis = |tag: &[u8; 4]| axes.iter().position(|a| a.tag().to_be_bytes() == *tag);
    let (wght, ital, slnt) = (axis(b"wght"), axis(b"ital"), axis(b"slnt"));
    // Variable builds often name the family after the default instance ("Archivo SemiBold"
    // with the wght default at 600): its instances belong to the family without that suffix.
    let mut base = base;
    let defaults: Vec<f32> = axes.iter().map(|a| a.default_value()).collect();
    let default_name = instances
        .iter()
        .find(|i| i.user_coords().zip(&defaults).all(|(c, d)| (c - d).abs() < 0.01))
        .and_then(|i| font.localized_strings(i.subfamily_name_id()).english_or_first().map(|s| s.to_string()));
    if let Some(d) = default_name.filter(|d| !d.eq_ignore_ascii_case("Regular"))
        && let Some(stem) = base.family.strip_suffix(d.as_str()).and_then(|s| s.strip_suffix(' '))
        && !stem.trim().is_empty()
    {
        base.family = stem.trim().to_string();
    }
    let mut out: Vec<FaceInfo> = Vec::new();
    for (k, inst) in instances.iter().enumerate().take(256) {
        let Some(name) = font.localized_strings(inst.subfamily_name_id()).english_or_first().map(|s| s.to_string()) else { continue };
        let coords: Vec<f32> = inst.user_coords().collect();
        let weight = wght.and_then(|i| coords.get(i)).map(|w| w.round().clamp(1.0, 1000.0) as u16).unwrap_or(base.weight);
        let italic = ital.and_then(|i| coords.get(i)).is_some_and(|v| *v >= 0.5)
            || slnt.and_then(|i| coords.get(i)).is_some_and(|v| *v != 0.0)
            || name.to_ascii_lowercase().contains("italic")
            || (base.italic && ital.is_none() && slnt.is_none());
        if out.iter().any(|f| f.style.eq_ignore_ascii_case(&name)) {
            continue;
        }
        out.push(FaceInfo { style: name, weight, italic, instance: u16::try_from(k).ok(), ..base.clone() });
    }
    if out.is_empty() { vec![base] } else { out }
}

/// How a family and style resolve for rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontStatus {
    /// The exact face is installed.
    Exact { face: FaceId },
    /// The family is installed but not this style: another face of it is used, possibly with
    /// synthetic bold or italic.
    Substituted { face: FaceId, used_style: String, synthetic: bool },
    /// The family is not installed: text renders in the default face instead.
    Missing,
}

impl FontStatus {
    pub fn is_exact(&self) -> bool {
        matches!(self, FontStatus::Exact { .. })
    }
}

/// Whether `family` / `style` will render as asked ([`resolve`] substitutes silently; callers
/// that must not render a different typeface check this).
pub fn status(family: &str, style: &str) -> FontStatus {
    let family = if family.trim().is_empty() { DEFAULT_FAMILY } else { family.trim() };
    let style = if style.trim().is_empty() { "Regular" } else { style.trim() };
    let r = resolve(family, style);
    if r.missing {
        return FontStatus::Missing;
    }
    let f = face(r.face);
    let exact = f.info.family.eq_ignore_ascii_case(family) && f.info.style.eq_ignore_ascii_case(style);
    if exact && !r.synth_bold && !r.synth_italic {
        FontStatus::Exact { face: r.face }
    } else {
        FontStatus::Substituted { face: r.face, used_style: f.info.style.clone(), synthetic: r.synth_bold || r.synth_italic }
    }
}

/// Scan the system font folders once (native only; a no-op on wasm). Returns the number of faces
/// added by this call.
pub fn scan_system() -> usize {
    // Held for the whole scan: a concurrent caller waits for the complete list instead of
    // returning early and resolving against a half-filled database.
    static SCAN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _scan = SCAN.lock().unwrap_or_else(PoisonError::into_inner);
    if system_scanned() {
        return 0;
    }
    let n = if cfg!(target_arch = "wasm32") { 0 } else { add_source(&DirectorySource::system()) };
    db().write().unwrap_or_else(PoisonError::into_inner).scanned = true;
    n
}

pub fn system_scanned() -> bool {
    db().read().unwrap_or_else(PoisonError::into_inner).scanned
}

/// A face by id (unknown ids give the default face).
// The bundled faces are compiled in (`include_bytes!`) and registered when the database is
// created, so `faces` is never empty; the fallback only covers a corrupt bundle.
pub fn face(id: FaceId) -> Arc<Face> {
    let d = db().read().unwrap_or_else(PoisonError::into_inner);
    d.faces.get(id).or_else(|| d.faces.first()).unwrap_or(&d.fallback).clone()
}

/// All faces (bundled first), retired ones excluded.
pub fn all_faces() -> Vec<Arc<Face>> {
    db().read().unwrap_or_else(PoisonError::into_inner).faces.iter().filter(|f| !f.is_retired()).cloned().collect()
}

/// Stop resolving and listing these faces (a font removed from the library). Ids stay valid:
/// [`face`] still returns them, so layouts already made keep drawing.
pub fn retire_faces(ids: &[FaceId]) {
    let d = db().read().unwrap_or_else(PoisonError::into_inner);
    for id in ids {
        if let Some(f) = d.faces.get(*id) {
            f.retired.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    bump_generation();
}

/// Families with their style names, sorted by family; styles in weight order.
pub fn families() -> Vec<(String, Vec<String>)> {
    let mut m: std::collections::BTreeMap<String, Vec<(bool, u16, String)>> = Default::default();
    for f in all_faces() {
        let v = m.entry(f.info.family.clone()).or_default();
        if !v.iter().any(|(_, _, s)| s == &f.info.style) {
            v.push((f.info.italic, f.info.weight, f.info.style.clone()));
        }
    }
    m.into_iter()
        .map(|(k, mut v)| {
            v.sort();
            (k, v.into_iter().map(|(_, _, s)| s).collect())
        })
        .collect()
}

/// `families()` for a picker: scans the system fonts first (once), so the list taken on the first
/// opening already holds the installed families.
pub fn picker_families() -> Vec<(String, Vec<String>)> {
    scan_system();
    families()
}

/// The face chosen for a family/style request, plus synthetic emboldening / slant when the family
/// lacks a real bold / italic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Resolved {
    pub face: FaceId,
    pub synth_bold: bool,
    pub synth_italic: bool,
    /// The requested family was not found (Premiere shows such fonts as missing).
    pub missing: bool,
}

fn style_wants(style: &str) -> (u16, bool) {
    let s = style.to_ascii_lowercase().replace([' ', '-'], "");
    let italic = s.contains("italic") || s.contains("oblique");
    let w = if s.contains("thin") || s.contains("hairline") {
        100
    } else if s.contains("extralight") || s.contains("ultralight") {
        200
    } else if s.contains("light") {
        300
    } else if s.contains("medium") {
        500
    } else if s.contains("semibold") || s.contains("demibold") {
        600
    } else if s.contains("extrabold") || s.contains("ultrabold") {
        800
    } else if s.contains("black") || s.contains("heavy") {
        900
    } else if s.contains("bold") {
        700
    } else {
        400
    };
    (w, italic)
}

/// Resolve a family + style name. Family matching is case-insensitive; a style that the family
/// lacks picks the nearest weight and synthesises bold (≥ 600 requested, ≤ 500 found) or italic.
pub fn resolve(family: &str, style: &str) -> Resolved {
    if let Some(r) = resolve_in(family, style) {
        return r;
    }
    if !system_scanned() && !family.is_empty() && !family.eq_ignore_ascii_case(DEFAULT_FAMILY) {
        scan_system();
        if let Some(r) = resolve_in(family, style) {
            return r;
        }
    }
    let mut r = resolve_in(DEFAULT_FAMILY, style).unwrap_or(Resolved { face: 0, synth_bold: false, synth_italic: false, missing: true });
    r.missing = !family.is_empty() && !family.eq_ignore_ascii_case(DEFAULT_FAMILY);
    r
}

fn resolve_in(family: &str, style: &str) -> Option<Resolved> {
    let faces = all_faces();
    let fam: Vec<&Arc<Face>> = faces.iter().filter(|f| f.info.family.eq_ignore_ascii_case(family)).collect();
    if fam.is_empty() {
        return None;
    }
    // a font the user put in the library wins over a system copy of the same family and style
    let exact = |f: &&&Arc<Face>| f.info.style.eq_ignore_ascii_case(style);
    if let Some(f) = fam.iter().filter(exact).find(|f| f.info.origin == LIBRARY_ORIGIN).or_else(|| fam.iter().find(exact)) {
        return Some(Resolved { face: f.id, synth_bold: false, synth_italic: false, missing: false });
    }
    let (w, it) = style_wants(style);
    let best = fam.iter().min_by_key(|f| {
        let italic_pen = if f.info.italic == it { 0 } else { 1000 };
        italic_pen + (f.info.weight as i32 - w as i32).unsigned_abs()
    })?;
    Some(Resolved { face: best.id, synth_bold: w >= 600 && best.info.weight <= 500, synth_italic: it && !best.info.italic, missing: false })
}

/// Whether a family name reads as a serif face (Noto Serif, Shippori Mincho, …), to pick a Mincho
/// fallback for serif text and a Gothic one otherwise.
fn is_serif(family: &str) -> bool {
    let f = family.to_ascii_lowercase();
    (f.contains("serif") && !f.contains("sans")) || f.contains("mincho")
}

/// A face that covers `c`, preferring `prefer`; bundled, craft-fonts and already-registered faces
/// are searched, in that order.
pub fn fallback_for(c: char, prefer: FaceId) -> FaceId {
    let p = face(prefer);
    if p.has_char(c) || c.is_control() {
        return prefer;
    }
    let faces = all_faces();
    // prefer a face of the same italic-ness that is bundled (cheap), then anything loaded already
    for f in faces.iter().filter(|f| f.info.origin == "bundled") {
        if f.has_char(c) {
            return f.id;
        }
    }
    // then the craft-fonts faces (Japanese): Mincho for serif text, Gothic otherwise, nearest weight
    let serif = is_serif(&p.info.family);
    let mut craft: Vec<&Arc<Face>> = faces.iter().filter(|f| f.info.origin == CRAFT_ORIGIN).collect();
    craft.sort_by_key(|f| (is_serif(&f.info.family) != serif, f.info.italic != p.info.italic, f.info.weight.abs_diff(p.info.weight)));
    for f in craft {
        if f.has_char(c) {
            return f.id;
        }
    }
    for f in faces.iter().filter(|f| f.info.origin != "bundled" && f.bytes.get().is_some()) {
        if f.has_char(c) {
            return f.id;
        }
    }
    prefer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_families_and_resolution() {
        let fams = families();
        let inter = fams.iter().find(|(f, _)| f == "Inter").expect("Inter");
        for s in ["Regular", "Medium", "SemiBold", "Bold", "Italic"] {
            assert!(inter.1.iter().any(|x| x == s), "{s} in {:?}", inter.1);
        }
        assert!(fams.iter().any(|(f, _)| f == "Noto Serif"));
        assert!(fams.iter().any(|(f, _)| f == "JetBrains Mono"));
        let b = resolve("inter", "bold");
        assert_eq!(face(b.face).info.style, "Bold");
        assert!(!b.synth_bold);
        // Noto Serif has only Regular: Bold Italic is synthesised
        let s = resolve("Noto Serif", "Bold Italic");
        assert_eq!(face(s.face).info.family, "Noto Serif");
        assert!(s.synth_bold && s.synth_italic);
        // Inter Bold Italic: nearest is Italic (400) → synth bold
        let bi = resolve("Inter", "Bold Italic");
        assert!(face(bi.face).info.italic && bi.synth_bold && !bi.synth_italic);
    }

    #[test]
    fn missing_family_falls_back_to_inter() {
        let r = resolve("No Such Font Family 123", "Regular");
        assert_eq!(face(r.face).info.family, "Inter");
        assert!(r.missing);
    }

    /// Native: the system scan reads name tables only and resolves families installed there.
    #[test]
    fn system_scan_finds_fonts() {
        let t = std::time::Instant::now();
        scan_system(); // (another test may have triggered the scan already)
        let n = all_faces().iter().filter(|f| f.info.origin == "system").count();
        eprintln!("system fonts: {n} faces in {:?}", t.elapsed());
        assert_eq!(scan_system(), 0, "scans once");
        if cfg!(target_os = "macos") {
            assert!(n > 20, "{n}");
            let r = resolve("Helvetica", "Bold");
            assert_eq!(face(r.face).info.family, "Helvetica");
            let l = crate::layout("Helvetica", &crate::TextStyle { family: "Helvetica".into(), size: 30.0, ..Default::default() }, &Default::default());
            assert!(!l.missing_font && l.glyphs.iter().all(|g| g.id != 0));
        }
    }

    /// craft-fonts faces are registered after the bundled ones and serve Japanese characters the
    /// requested face lacks: Gothic for sans text, Mincho for serif text.
    #[test]
    fn japanese_falls_back_to_craft_fonts() {
        let craft: Vec<_> = all_faces().into_iter().filter(|f| f.info.origin == CRAFT_ORIGIN).collect();
        if craft_japanese().next().is_none() {
            eprintln!("SKIPPED: built without craft-fonts (set CRAFT_FONTS_DIR to run)");
            assert!(craft.is_empty());
            return;
        }
        assert!(craft.len() >= craft_japanese().count(), "{} faces", craft.len());
        let bundled = all_faces().iter().take_while(|f| f.info.origin == "bundled").count();
        assert!(craft.iter().all(|f| f.id >= bundled), "craft-fonts faces come after the bundled faces");
        let sans = resolve("Inter", "Regular").face;
        let serif = resolve("Noto Serif", "Regular").face;
        for c in "日本語の文字".chars() {
            let g = face(fallback_for(c, sans));
            assert_eq!(g.info.origin, CRAFT_ORIGIN, "{c}");
            assert!(g.has_char(c) && !is_serif(&g.info.family), "{c}: {}", g.info.family);
            let m = face(fallback_for(c, serif));
            assert!(m.has_char(c) && is_serif(&m.info.family), "{c}: {}", m.info.family);
        }
        // the bold UI face for bold text
        let bold = face(fallback_for('日', resolve("Inter", "Bold").face));
        assert!(bold.info.weight >= 600, "{} {}", bold.info.family, bold.info.style);
        // and laid out with real glyphs (no tofu)
        let l = crate::layout("日本語の文字", &crate::TextStyle { size: 40.0, ..Default::default() }, &Default::default());
        assert_eq!(l.glyphs.len(), 6);
        assert!(l.glyphs.iter().all(|g| g.id != 0), "{:?}", l.glyphs);
    }

    #[test]
    fn metrics_and_coverage() {
        let f = face(resolve("Inter", "Regular").face);
        let m = f.metrics(100.0);
        assert!(m.ascent > 80.0 && m.ascent < 110.0, "{m:?}");
        assert!(m.descent > 10.0 && m.descent < 40.0);
        assert!(m.cap_height > 60.0 && m.cap_height < 80.0);
        assert!(f.has_char('A') && f.has_char('Ж'));
        assert!(!f.has_char('\u{5d0}'), "Inter has no Hebrew");
        assert!(f.has_feature(b"liga") || f.has_feature(b"calt"));
    }

    #[test]
    fn adobe_fonts_folders_are_scanned() {
        // #1878: macOS keeps activated fonts as hidden `.r/.<id>.otf`, Windows as `r/<id>` without an extension
        let root = std::env::temp_dir().join(format!("filmcraft-livetype-{}", std::process::id()));
        for d in [".r", "r"] {
            std::fs::create_dir_all(root.join(d)).expect("temp dir");
        }
        std::fs::write(root.join(".r/.12345.otf"), INTER_REGULAR).expect("write");
        std::fs::write(root.join("r/12346"), INTER_REGULAR).expect("write");
        std::fs::write(root.join("r/12347"), b"not a font").expect("write");
        let faces = DirectorySource { dirs: vec![root.clone()] }.faces();
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(faces.iter().filter(|f| f.family == "Inter").count(), 2, "{:?}", faces.iter().map(|f| &f.data).collect::<Vec<_>>());
        // and the system scan includes the whole CoreSync path
        let expected = if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Library/Application Support/Adobe/CoreSync/plugins/livetype"))
        } else if cfg!(windows) {
            std::env::var_os("APPDATA").map(|a| std::path::PathBuf::from(a).join("Adobe\\CoreSync\\plugins\\livetype"))
        } else {
            None
        };
        if let Some(expected) = expected {
            assert!(DirectorySource::system().dirs.contains(&expected), "{expected:?}");
        }
    }

    #[test]
    fn covers_text_does_not_keep_file_bytes() {
        let path = std::env::temp_dir().join(format!("filmcraft-covers-text-{}.ttf", std::process::id()));
        std::fs::write(&path, INTER_REGULAR).expect("write temp font");
        let info = FaceInfo {
            family: "Inter".into(),
            style: "Regular".into(),
            weight: 400,
            italic: false,
            index: 0,
            data: FaceData::File(path.clone()),
            origin: "system",
            instance: None,
        };
        let f = Face::new(0, info);
        assert!(f.covers_text("A\u{416}"));
        assert!(!f.covers_text("A\u{5d0}"));
        assert!(f.bytes.get().is_none(), "probing coverage must not cache the file");
        assert!(f.data().is_some());
        assert!(f.bytes.get().is_some());
        let _ = std::fs::remove_file(&path);
    }
}
