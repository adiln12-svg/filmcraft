//! The Colour Palette Library: named, reusable palettes kept outside projects, in
//! `<data dir>/Palette Library/palettes.json` (the storage of [`crate::user_library`], shared
//! with the Font Library).
//!
//! - **Stable identifiers.** A palette is `pal_…`, its colours `col_…` and its usage rules
//!   `rule_…`; they never change when a palette is renamed or edited, and survive duplication.
//! - **Versions.** Every saved change to a palette's colours or rules is a new, numbered version.
//!   Saved versions are never changed: each records who saved it, why, and a SHA-256 of its
//!   content (checked on every read), so a brand kit that references `pal_…@3` (Milestone 2C)
//!   keeps exactly what was approved however the palette changes later. An edit names the
//!   version it was based on and is refused when the palette has moved on, so nothing is
//!   overwritten unseen.
//! - **Roles, shades, rules.** A colour has a name, an exact sRGB value, an optional semantic
//!   role (primary, secondary, accent, background, text, or any other label the palette's owner
//!   uses) and may be a shade of another colour. Usage rules are only what the owner states:
//!   permitted or prohibited foreground-on-background combinations, and written guidelines.
//!   FilmCraft adds none of its own.
//! - Name, description and archived state are palette metadata (not versioned); their changes
//!   are recorded in the palette's history.
//!
//! Commands (`palettes.*`) are the only way in: the Palette Library window and agents use the
//! same ones. See `docs/palette-library.md`.

use std::path::{Path, PathBuf};

use filmcraft_project::HexColor;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, str_p, u64_p};
use crate::user_library::{now_rfc3339, read_json, sha256_hex, write_json};
use crate::{Result, Session};

/// Folder of the library inside the data directory.
pub const DIR_NAME: &str = "Palette Library";
const FILE: &str = "palettes.json";
const LIBRARY_FORMAT: u32 = 1;
pub const MAX_PALETTES: usize = 1_000;
pub const MAX_COLORS: usize = 256;
pub const MAX_RULES: usize = 512;
pub const MAX_VERSIONS: usize = 2_000;
const MAX_HISTORY: usize = 2_000;
const MAX_NAME: usize = 128;
const MAX_TEXT: usize = 2_000;

/// Roles offered by the interface; any other short label is accepted as well.
pub const STANDARD_ROLES: &[&str] = &["primary", "secondary", "accent", "background", "text", "neutral"];

/// One colour of a palette version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteColor {
    pub id: String,
    pub name: String,
    /// Exact sRGB value (`#RRGGBB`).
    pub hex: HexColor,
    /// Semantic role ("primary", "text", …); empty when none was given.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
    /// The colour this one is a shade of (a colour id of the same version).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shade_of: Option<String>,
    /// The shade's label ("700", "Deep", …).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shade: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuleKind {
    /// The foreground colour may be used on the background colour.
    Permitted,
    /// The foreground colour must not be used on the background colour.
    Prohibited,
    /// A written rule (e.g. "one yellow focal point per frame"), kept as stated.
    Guideline,
}

impl RuleKind {
    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "permitted" | "allowed" | "allow" => Some(Self::Permitted),
            "prohibited" | "forbidden" | "never" => Some(Self::Prohibited),
            "guideline" | "note" => Some(Self::Guideline),
            _ => None,
        }
    }
}

/// A usage rule the palette's owner stated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRule {
    pub id: String,
    pub kind: RuleKind,
    /// Combination rules: the colour drawn (text, a graphic) …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<String>,
    /// … on this colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// Colours a guideline is about.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<String>,
    /// What the rule applies to ("text", "graphics", "any"); "any" when not given.
    #[serde(default = "any")]
    pub applies_to: String,
    /// The rule in the owner's words (required for guidelines).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

fn any() -> String {
    "any".into()
}

/// A saved, immutable state of a palette's colours and rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteVersion {
    pub version: u32,
    pub created: String,
    /// Who saved it ("user" from the interface; agents name themselves).
    pub by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub colors: Vec<PaletteColor>,
    pub rules: Vec<UsageRule>,
    /// SHA-256 of the colours and rules, checked on every read.
    pub sha256: String,
}

impl PaletteVersion {
    pub fn content_hash(colors: &[PaletteColor], rules: &[UsageRule]) -> String {
        sha256_hex(&serde_json::to_vec(&(colors, rules)).unwrap_or_default())
    }
    /// The stored content still matches its fingerprint.
    pub fn intact(&self) -> bool {
        Self::content_hash(&self.colors, &self.rules) == self.sha256
    }
    pub fn color(&self, id_or_name: &str) -> Option<&PaletteColor> {
        self.colors.iter().find(|c| c.id == id_or_name).or_else(|| self.colors.iter().find(|c| c.name.eq_ignore_ascii_case(id_or_name)))
    }
}

/// A change of metadata or a new version, as listed in a palette's history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEvent {
    pub at: String,
    pub by: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Palette {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub archived: bool,
    pub created: String,
    pub updated: String,
    /// Oldest first; never empty.
    pub versions: Vec<PaletteVersion>,
    #[serde(default)]
    pub history: Vec<HistoryEvent>,
}

impl Palette {
    pub fn latest(&self) -> Option<&PaletteVersion> {
        self.versions.last()
    }
    pub fn version(&self, v: u32) -> Option<&PaletteVersion> {
        self.versions.iter().find(|x| x.version == v)
    }
    fn log(&mut self, by: &str, action: &str, detail: String) {
        let at = now_rfc3339();
        self.updated.clone_from(&at);
        self.history.push(HistoryEvent { at, by: by.to_string(), action: action.to_string(), detail });
        if self.history.len() > MAX_HISTORY {
            let cut = self.history.len() - MAX_HISTORY;
            self.history.drain(..cut);
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Library {
    pub format: u32,
    pub palettes: Vec<Palette>,
}

impl Library {
    pub fn get(&self, id: &str) -> Option<&Palette> {
        self.palettes.iter().find(|p| p.id == id)
    }
    fn get_mut(&mut self, id: &str) -> std::result::Result<&mut Palette, String> {
        self.palettes.iter_mut().find(|p| p.id == id).ok_or_else(|| format!("no palette `{id}`"))
    }
}

// ------------------------------------------------------------------ storage

/// The library folder of the session (see [`crate::user_library::data_dir`]).
pub fn library_dir(s: &Session) -> Option<PathBuf> {
    crate::user_library::data_dir(s).map(|d| d.join(DIR_NAME))
}

/// Read the library (none yet is an empty library; a damaged file is an error).
pub fn load(dir: &Path) -> std::result::Result<Library, String> {
    Ok(read_json::<Library>(&dir.join(FILE), "palette library")?.unwrap_or(Library { format: LIBRARY_FORMAT, palettes: Vec::new() }))
}

fn save(dir: &Path, lib: &mut Library) -> std::result::Result<(), String> {
    lib.format = LIBRARY_FORMAT;
    write_json(&dir.join(FILE), lib)
}

/// Read, change and write the library in one step.
fn modify<T>(dir: &Path, f: impl FnOnce(&mut Library) -> std::result::Result<T, String>) -> std::result::Result<T, String> {
    let mut lib = load(dir)?;
    let out = f(&mut lib)?;
    save(dir, &mut lib)?;
    Ok(out)
}

fn new_id(prefix: &str, seed: &str) -> String {
    crate::user_library::new_id(prefix, seed)
}

/// A new colour id (`col_…`), for editors that build a version before saving it.
pub fn fresh_color_id() -> String {
    new_id("col", "editor")
}

/// A new rule id (`rule_…`).
pub fn fresh_rule_id() -> String {
    new_id("rule", "editor")
}

// ------------------------------------------------------------------ validation

fn clean_text(s: &str, what: &str, max: usize, required: bool) -> std::result::Result<String, String> {
    let t = s.trim();
    if required && t.is_empty() {
        return Err(format!("{what} cannot be empty"));
    }
    if t.len() > max || t.chars().any(|c| c.is_control() && c != '\n') {
        return Err(format!("{what} must be at most {max} bytes without control characters"));
    }
    Ok(t.to_string())
}

fn clean_role(s: &str) -> std::result::Result<String, String> {
    let t = s.trim().to_ascii_lowercase();
    if t.len() > 40 || !t.chars().all(|c| c.is_ascii_alphanumeric() || " -_".contains(c)) {
        return Err(format!("role `{s}` must be a short label (letters, digits, spaces, - and _)"));
    }
    Ok(t)
}

/// Check a version's colours and rules: unique ids and names, shades of existing base colours,
/// rules about existing colours, no rule contradicting another.
pub fn validate(colors: &[PaletteColor], rules: &[UsageRule]) -> std::result::Result<(), String> {
    if colors.len() > MAX_COLORS {
        return Err(format!("a palette holds at most {MAX_COLORS} colours"));
    }
    if rules.len() > MAX_RULES {
        return Err(format!("a palette holds at most {MAX_RULES} rules"));
    }
    for (i, c) in colors.iter().enumerate() {
        if colors[..i].iter().any(|o| o.id == c.id) {
            return Err(format!("colour id `{}` is used twice", c.id));
        }
        if colors[..i].iter().any(|o| o.name.eq_ignore_ascii_case(&c.name)) {
            return Err(format!("two colours are named \"{}\"", c.name));
        }
        if let Some(base) = &c.shade_of {
            let b = colors.iter().find(|o| &o.id == base).ok_or_else(|| format!("\"{}\" is a shade of a colour that is not in the palette", c.name))?;
            if b.id == c.id || b.shade_of.is_some() {
                return Err(format!("\"{}\" must be a shade of a base colour (not of itself or of another shade)", c.name));
            }
        }
    }
    let exists = |id: &Option<String>| id.as_ref().is_some_and(|id| colors.iter().any(|c| &c.id == id));
    for (i, r) in rules.iter().enumerate() {
        if rules[..i].iter().any(|o| o.id == r.id) {
            return Err(format!("rule id `{}` is used twice", r.id));
        }
        match r.kind {
            RuleKind::Permitted | RuleKind::Prohibited => {
                if !exists(&r.foreground) || !exists(&r.background) {
                    return Err("a combination rule needs a foreground and a background colour of the palette".into());
                }
                if r.foreground == r.background {
                    return Err("a combination rule needs two different colours".into());
                }
                let same = |o: &UsageRule| o.foreground == r.foreground && o.background == r.background && o.applies_to.eq_ignore_ascii_case(&r.applies_to);
                if let Some(o) = rules[..i].iter().find(|o| o.kind != RuleKind::Guideline && same(o)) {
                    let name = |id: &Option<String>| id.as_ref().and_then(|id| colors.iter().find(|c| &c.id == id)).map(|c| c.name.clone()).unwrap_or_default();
                    let what = if o.kind == r.kind { "is stated twice" } else { "is both permitted and prohibited" };
                    return Err(format!("{} on {} ({}) {what}", name(&r.foreground), name(&r.background), r.applies_to));
                }
            }
            RuleKind::Guideline => {
                if r.text.trim().is_empty() {
                    return Err("a guideline needs its text".into());
                }
                if let Some(c) = r.colors.iter().find(|c| !colors.iter().any(|o| &o.id == *c)) {
                    return Err(format!("guideline refers to colour `{c}` that is not in the palette"));
                }
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ parsing colours and rules

/// `#RRGGBB` / `RRGGBB` from `hex`, or `rgb: [r, g, b]` (0–255 each).
fn color_value(v: &Value) -> std::result::Result<Option<HexColor>, String> {
    if let Some(h) = v.get("hex").and_then(Value::as_str) {
        return HexColor::parse(h.trim()).map(Some).ok_or_else(|| format!("`{h}` is not a six-digit hex colour (#RRGGBB)"));
    }
    if let Some(a) = v.get("rgb").and_then(Value::as_array) {
        let c: Vec<u8> = a.iter().filter_map(|x| x.as_u64().and_then(|x| u8::try_from(x).ok())).collect();
        if a.len() != 3 || c.len() != 3 {
            return Err("rgb must be three whole numbers from 0 to 255".into());
        }
        return Ok(Some(HexColor([c[0], c[1], c[2]])));
    }
    Ok(None)
}

/// A colour reference in a rule or shade: a colour id, or the exact name of a colour.
fn color_ref(colors: &[PaletteColor], r: &str) -> std::result::Result<String, String> {
    colors
        .iter()
        .find(|c| c.id == r)
        .or_else(|| colors.iter().find(|c| c.name.eq_ignore_ascii_case(r.trim())))
        .map(|c| c.id.clone())
        .ok_or_else(|| format!("no colour `{r}` in the palette"))
}

fn apply_color_fields(c: &mut PaletteColor, v: &Value, colors: &[PaletteColor]) -> std::result::Result<(), String> {
    if let Some(n) = v.get("name").and_then(Value::as_str) {
        c.name = clean_text(n, "a colour name", MAX_NAME, true)?;
    }
    if let Some(h) = color_value(v)? {
        c.hex = h;
    }
    if let Some(r) = v.get("role") {
        c.role = match r.as_str() {
            Some(s) => clean_role(s)?,
            None => String::new(),
        };
    }
    if let Some(sh) = v.get("shadeOf") {
        c.shade_of = match sh.as_str().filter(|s| !s.trim().is_empty()) {
            Some(s) => Some(color_ref(colors, s)?),
            None => None,
        };
    }
    if let Some(s) = v.get("shade") {
        c.shade = clean_text(s.as_str().unwrap_or(""), "a shade label", 40, false)?;
    }
    Ok(())
}

fn new_color(v: &Value, colors: &[PaletteColor], seed: &str) -> std::result::Result<PaletteColor, String> {
    let hex = color_value(v)?.ok_or("each colour needs `hex` (#RRGGBB) or `rgb` ([r, g, b])")?;
    let id = match v.get("id").and_then(Value::as_str) {
        Some(id) if id.starts_with("col_") && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => id.to_string(),
        Some(id) => return Err(format!("`{id}` is not a colour id (col_…)")),
        None => new_id("col", seed),
    };
    let mut c = PaletteColor { id, name: String::new(), hex, role: String::new(), shade_of: None, shade: String::new() };
    apply_color_fields(&mut c, v, colors)?;
    if c.name.is_empty() {
        c.name = hex.hex();
    }
    Ok(c)
}

fn apply_rule_fields(r: &mut UsageRule, v: &Value, colors: &[PaletteColor]) -> std::result::Result<(), String> {
    if let Some(k) = v.get("kind").and_then(Value::as_str) {
        r.kind = RuleKind::parse(k).ok_or_else(|| format!("rule kind `{k}` is not permitted, prohibited or guideline"))?;
    }
    for (key, slot) in [("foreground", &mut r.foreground), ("background", &mut r.background)] {
        if let Some(x) = v.get(key) {
            *slot = match x.as_str().filter(|s| !s.trim().is_empty()) {
                Some(s) => Some(color_ref(colors, s)?),
                None => None,
            };
        }
    }
    if let Some(a) = v.get("colors").and_then(Value::as_array) {
        r.colors = a.iter().filter_map(Value::as_str).map(|s| color_ref(colors, s)).collect::<std::result::Result<_, _>>()?;
    }
    if let Some(a) = v.get("appliesTo").and_then(Value::as_str) {
        let a = clean_role(a)?;
        r.applies_to = if a.is_empty() { any() } else { a };
    }
    if let Some(t) = v.get("text").and_then(Value::as_str) {
        r.text = clean_text(t, "a rule's text", MAX_TEXT, false)?;
    }
    Ok(())
}

fn new_rule(v: &Value, colors: &[PaletteColor], seed: &str) -> std::result::Result<UsageRule, String> {
    let kind = v.get("kind").and_then(Value::as_str).ok_or("each rule needs `kind`: permitted, prohibited or guideline")?;
    let kind = RuleKind::parse(kind).ok_or_else(|| format!("rule kind `{kind}` is not permitted, prohibited or guideline"))?;
    let id = match v.get("id").and_then(Value::as_str) {
        Some(id) if id.starts_with("rule_") && id.len() <= 40 => id.to_string(),
        Some(id) => return Err(format!("`{id}` is not a rule id (rule_…)")),
        None => new_id("rule", seed),
    };
    let mut r = UsageRule { id, kind, foreground: None, background: None, colors: Vec::new(), applies_to: any(), text: String::new() };
    apply_rule_fields(&mut r, v, colors)?;
    Ok(r)
}

/// Colours from a full list (ids kept when given; shades may refer to colours later in the list).
fn colors_from(list: &[Value], seed: &str) -> std::result::Result<Vec<PaletteColor>, String> {
    if list.len() > MAX_COLORS {
        return Err(format!("a palette holds at most {MAX_COLORS} colours"));
    }
    // first pass without shades, so a shade can name a base listed after it
    let mut out = Vec::new();
    for v in list {
        let mut plain = v.clone();
        if let Some(o) = plain.as_object_mut() {
            o.remove("shadeOf");
        }
        let c = new_color(&plain, &out, seed)?;
        out.push(c);
    }
    for (i, v) in list.iter().enumerate() {
        if let Some(sh) = v.get("shadeOf").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
            let base = color_ref(&out, sh)?;
            if let Some(c) = out.get_mut(i) {
                c.shade_of = Some(base);
            }
        }
    }
    Ok(out)
}

fn rules_from(list: &[Value], colors: &[PaletteColor], seed: &str) -> std::result::Result<Vec<UsageRule>, String> {
    if list.len() > MAX_RULES {
        return Err(format!("a palette holds at most {MAX_RULES} rules"));
    }
    list.iter().map(|v| new_rule(v, colors, seed)).collect()
}

/// Apply one edit operation to a working copy of a version's colours and rules.
fn apply_change(colors: &mut Vec<PaletteColor>, rules: &mut Vec<UsageRule>, ch: &Value, seed: &str) -> std::result::Result<(), String> {
    let op = ch.get("op").and_then(Value::as_str).ok_or("each change needs `op`")?;
    let id = || ch.get("id").and_then(Value::as_str).ok_or_else(|| format!("`{op}` needs `id`"));
    match op {
        "addColor" => {
            let c = new_color(ch.get("color").unwrap_or(ch), colors, seed)?;
            if colors.iter().any(|o| o.id == c.id) {
                return Err(format!("colour id `{}` is already in the palette", c.id));
            }
            colors.push(c);
        }
        "updateColor" => {
            let cid = color_ref(colors, id()?)?;
            let snapshot = colors.clone();
            let c = colors.iter_mut().find(|c| c.id == cid).ok_or("colour vanished")?;
            apply_color_fields(c, ch.get("color").unwrap_or(ch), &snapshot)?;
        }
        "duplicateColor" => {
            let cid = color_ref(colors, id()?)?;
            let src = colors.iter().find(|c| c.id == cid).cloned().ok_or("colour vanished")?;
            let mut name = ch.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("{} copy", src.name));
            let mut n = 2;
            while colors.iter().any(|c| c.name.eq_ignore_ascii_case(&name)) && n < 1000 {
                name = format!("{} copy {n}", src.name);
                n += 1;
            }
            let at = colors.iter().position(|c| c.id == cid).map_or(colors.len(), |i| i + 1);
            colors.insert(at, PaletteColor { id: new_id("col", seed), name, ..src });
        }
        "removeColor" => {
            let cid = color_ref(colors, id()?)?;
            let name = colors.iter().find(|c| c.id == cid).map(|c| c.name.clone()).unwrap_or_default();
            if colors.iter().any(|c| c.shade_of.as_deref() == Some(cid.as_str())) {
                return Err(format!("\"{name}\" has shades; remove them or make them base colours first"));
            }
            let used =
                |r: &&UsageRule| r.foreground.as_deref() == Some(cid.as_str()) || r.background.as_deref() == Some(cid.as_str()) || r.colors.contains(&cid);
            if let Some(r) = rules.iter().find(used) {
                return Err(format!("\"{name}\" is used by a usage rule ({}); remove or change the rule first", r.id));
            }
            colors.retain(|c| c.id != cid);
        }
        "moveColor" => {
            let cid = color_ref(colors, id()?)?;
            let from = colors.iter().position(|c| c.id == cid).ok_or("colour vanished")?;
            let to = ch.get("to").and_then(Value::as_u64).map_or(colors.len().saturating_sub(1), |t| (t as usize).min(colors.len().saturating_sub(1)));
            let c = colors.remove(from);
            colors.insert(to, c);
        }
        "addRule" => {
            let r = new_rule(ch.get("rule").unwrap_or(ch), colors, seed)?;
            rules.push(r);
        }
        "updateRule" => {
            let rid = id()?.to_string();
            let r = rules.iter_mut().find(|r| r.id == rid).ok_or_else(|| format!("no rule `{rid}` in the palette"))?;
            apply_rule_fields(r, ch.get("rule").unwrap_or(ch), colors)?;
        }
        "removeRule" => {
            let rid = id()?.to_string();
            let before = rules.len();
            rules.retain(|r| r.id != rid);
            if rules.len() == before {
                return Err(format!("no rule `{rid}` in the palette"));
            }
        }
        other => {
            return Err(format!("unknown change `{other}` (addColor, updateColor, duplicateColor, removeColor, moveColor, addRule, updateRule, removeRule)"));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ operations

fn who(p: &Value) -> std::result::Result<String, String> {
    let by = clean_text(str_p(p, "by").unwrap_or(""), "`by`", MAX_NAME, false)?;
    Ok(if by.is_empty() { "unspecified".into() } else { by })
}

/// Create a palette (version 1).
pub fn create(dir: &Path, p: &Value) -> std::result::Result<Palette, String> {
    let name = clean_text(str_p(p, "name").unwrap_or(""), "a palette name", MAX_NAME, true)?;
    let description = clean_text(str_p(p, "description").unwrap_or(""), "a description", MAX_TEXT, false)?;
    let by = who(p)?;
    let note = clean_text(str_p(p, "note").unwrap_or(""), "a note", MAX_TEXT, false)?;
    let colors = colors_from(p.get("colors").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default(), &name)?;
    let rules = rules_from(p.get("rules").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default(), &colors, &name)?;
    validate(&colors, &rules)?;
    modify(dir, |lib| {
        if lib.palettes.len() >= MAX_PALETTES {
            return Err(format!("the palette library is full ({MAX_PALETTES} palettes)"));
        }
        let now = now_rfc3339();
        let sha256 = PaletteVersion::content_hash(&colors, &rules);
        let v = PaletteVersion { version: 1, created: now.clone(), by: by.clone(), note, colors, rules, sha256 };
        let mut pal = Palette {
            id: new_id("pal", &name),
            name,
            description,
            archived: false,
            created: now.clone(),
            updated: now,
            versions: vec![v],
            history: Vec::new(),
        };
        pal.log(&by, "created", "version 1".into());
        lib.palettes.push(pal.clone());
        Ok(pal)
    })
}

/// What [`update`] did.
#[derive(Clone, Debug)]
pub struct Updated {
    pub palette: Palette,
    /// A new version was saved (false when the colours and rules did not change).
    pub new_version: bool,
}

/// Save changes to a palette's colours and rules as a new version. `baseVersion` must be the
/// palette's latest version: a palette that moved on since is refused, never overwritten.
pub fn update(dir: &Path, p: &Value) -> std::result::Result<Updated, String> {
    let id = str_p(p, "id").ok_or("need `id` (a palette id)")?.to_string();
    let base = p.get("baseVersion").and_then(Value::as_u64).ok_or("need `baseVersion`: the version the changes were made to (palettes.get)")?;
    let by = who(p)?;
    let note = clean_text(str_p(p, "note").unwrap_or(""), "a note", MAX_TEXT, false)?;
    modify(dir, |lib| {
        let pal = lib.get_mut(&id)?;
        let latest = pal.latest().cloned().ok_or("the palette has no versions")?;
        if u64::from(latest.version) != base {
            return Err(format!("the palette changed since version {base} (latest is {}); get it again and reapply the changes", latest.version));
        }
        if !latest.intact() {
            return Err(format!("version {} of the palette does not match its fingerprint (the library file was edited outside FilmCraft)", latest.version));
        }
        if pal.archived {
            return Err("the palette is archived; restore it first".into());
        }
        let mut colors = latest.colors.clone();
        let mut rules = latest.rules.clone();
        if let Some(list) = p.get("colors").and_then(Value::as_array) {
            // a full list; rules about colours no longer in it are refused by `validate`
            colors = colors_from(list, &id)?;
        }
        if let Some(list) = p.get("rules").and_then(Value::as_array) {
            rules = rules_from(list, &colors, &id)?;
        }
        for ch in p.get("changes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default().iter().take(1_000) {
            apply_change(&mut colors, &mut rules, ch, &id)?;
        }
        validate(&colors, &rules)?;
        let mut new_version = false;
        if colors != latest.colors || rules != latest.rules {
            if pal.versions.len() >= MAX_VERSIONS {
                return Err(format!("the palette has {MAX_VERSIONS} versions; duplicate it to keep editing"));
            }
            let version = latest.version.saturating_add(1);
            let sha256 = PaletteVersion::content_hash(&colors, &rules);
            pal.versions.push(PaletteVersion { version, created: now_rfc3339(), by: by.clone(), note: note.clone(), colors, rules, sha256 });
            pal.log(&by, "saved", format!("version {version}{}", if note.is_empty() { String::new() } else { format!(": {note}") }));
            new_version = true;
        }
        Ok(Updated { palette: pal.clone(), new_version })
    })
}

/// Rename a palette or change its description (metadata, not versioned; recorded in history).
pub fn set_info(dir: &Path, p: &Value) -> std::result::Result<Palette, String> {
    let id = str_p(p, "id").ok_or("need `id`")?.to_string();
    let by = who(p)?;
    let name = str_p(p, "name").map(|n| clean_text(n, "a palette name", MAX_NAME, true)).transpose()?;
    let description = str_p(p, "description").map(|d| clean_text(d, "a description", MAX_TEXT, false)).transpose()?;
    modify(dir, |lib| {
        let pal = lib.get_mut(&id)?;
        if let Some(n) = name.filter(|n| *n != pal.name) {
            let old = std::mem::replace(&mut pal.name, n.clone());
            pal.log(&by, "renamed", format!("\"{old}\" → \"{n}\""));
        }
        if let Some(d) = description.filter(|d| *d != pal.description) {
            pal.description = d;
            pal.log(&by, "description", String::new());
        }
        Ok(pal.clone())
    })
}

/// Archive (hide from the default list) or restore a palette. Its versions stay usable.
pub fn set_archived(dir: &Path, id: &str, archived: bool, by: &str) -> std::result::Result<Palette, String> {
    modify(dir, |lib| {
        let pal = lib.get_mut(id)?;
        if pal.archived != archived {
            pal.archived = archived;
            pal.log(by, if archived { "archived" } else { "restored" }, String::new());
        }
        Ok(pal.clone())
    })
}

/// A new palette (new id, version 1) holding the colours and rules of `version` (default latest).
pub fn duplicate(dir: &Path, id: &str, version: Option<u32>, name: Option<&str>, by: &str) -> std::result::Result<Palette, String> {
    modify(dir, |lib| {
        if lib.palettes.len() >= MAX_PALETTES {
            return Err(format!("the palette library is full ({MAX_PALETTES} palettes)"));
        }
        let src = lib.get(id).ok_or_else(|| format!("no palette `{id}`"))?;
        let v = match version {
            Some(n) => src.version(n).ok_or_else(|| format!("palette `{id}` has no version {n}"))?,
            None => src.latest().ok_or("the palette has no versions")?,
        };
        let name = match name {
            Some(n) => clean_text(n, "a palette name", MAX_NAME, true)?,
            None => format!("{} copy", src.name),
        };
        let now = now_rfc3339();
        let mut nv = v.clone();
        nv.version = 1;
        nv.created.clone_from(&now);
        nv.by = by.to_string();
        nv.note = format!("duplicated from {} version {}", src.id, v.version);
        let mut pal = Palette {
            id: new_id("pal", &name),
            name,
            description: src.description.clone(),
            archived: false,
            created: now.clone(),
            updated: now,
            versions: vec![nv],
            history: Vec::new(),
        };
        pal.log(by, "created", format!("duplicate of {} \"{}\" version {}", src.id, src.name, v.version));
        lib.palettes.push(pal.clone());
        Ok(pal)
    })
}

/// Delete a palette and all its versions.
pub fn delete(dir: &Path, id: &str) -> std::result::Result<Palette, String> {
    modify(dir, |lib| {
        let i = lib.palettes.iter().position(|p| p.id == id).ok_or_else(|| format!("no palette `{id}`"))?;
        Ok(lib.palettes.remove(i))
    })
}

// ------------------------------------------------------------------ JSON for commands

pub fn color_json(c: &PaletteColor) -> Value {
    json!({"id": c.id, "name": c.name, "hex": c.hex.hex(), "rgb": c.hex.0, "role": c.role, "shadeOf": c.shade_of, "shade": c.shade})
}

pub fn rule_json(r: &UsageRule, v: &PaletteVersion) -> Value {
    let name = |id: &Option<String>| id.as_ref().and_then(|id| v.color(id)).map(|c| c.name.clone());
    json!({"id": r.id, "kind": r.kind, "foreground": r.foreground, "foregroundName": name(&r.foreground), "background": r.background,
           "backgroundName": name(&r.background), "colors": r.colors, "appliesTo": r.applies_to, "text": r.text})
}

pub fn version_json(v: &PaletteVersion) -> Value {
    json!({"version": v.version, "created": v.created, "by": v.by, "note": v.note, "sha256": v.sha256, "intact": v.intact(),
           "colors": v.colors.iter().map(color_json).collect::<Vec<_>>(), "rules": v.rules.iter().map(|r| rule_json(r, v)).collect::<Vec<_>>()})
}

pub fn summary_json(p: &Palette) -> Value {
    let latest = p.latest();
    json!({"id": p.id, "name": p.name, "description": p.description, "archived": p.archived, "created": p.created, "updated": p.updated,
           "latestVersion": latest.map(|v| v.version), "colors": latest.map_or(0, |v| v.colors.len()), "rules": latest.map_or(0, |v| v.rules.len()),
           "swatches": latest.map(|v| v.colors.iter().take(24).map(|c| c.hex.hex()).collect::<Vec<_>>()).unwrap_or_default()})
}

pub fn palette_json(p: &Palette, version: Option<&PaletteVersion>) -> Value {
    let mut out = summary_json(p);
    out["versions"] = json!(
        p.versions.iter().map(|v| json!({"version": v.version, "created": v.created, "by": v.by, "note": v.note, "colors": v.colors.len(), "rules": v.rules.len(), "sha256": v.sha256})).collect::<Vec<_>>()
    );
    if let Some(v) = version.or(p.latest()) {
        out["version"] = version_json(v);
    }
    out
}

// ------------------------------------------------------------------ commands

fn dir_of(s: &Session, cmd: &str) -> Result<PathBuf> {
    library_dir(s).ok_or_else(|| bad(cmd, "no data directory for the palette library (set FILMCRAFT_DATA_DIR)"))
}

fn version_p(p: &Value) -> Option<u32> {
    u64_p(p, "version").and_then(|v| u32::try_from(v).ok())
}

fn list_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "palettes.list";
    let lib = load(&dir_of(s, cmd)?).map_err(|e| bad(cmd, e))?;
    let q = str_p(p, "query").unwrap_or("").trim().to_lowercase();
    let archived = bool_p(p, "includeArchived").unwrap_or(false);
    let hit = |pal: &Palette| {
        q.is_empty()
            || pal.name.to_lowercase().contains(&q)
            || pal.description.to_lowercase().contains(&q)
            || pal.id == q
            || pal.latest().is_some_and(|v| v.colors.iter().any(|c| c.name.to_lowercase().contains(&q) || c.hex.hex().eq_ignore_ascii_case(&q) || c.role == q))
    };
    let list: Vec<Value> = lib.palettes.iter().filter(|x| (archived || !x.archived) && hit(x)).map(summary_json).collect();
    Ok(json!({"dir": dir_of(s, cmd)?.display().to_string(), "palettes": list}))
}

fn get_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "palettes.get";
    let id = str_p(p, "id").ok_or_else(|| bad(cmd, "need `id`"))?;
    let lib = load(&dir_of(s, cmd)?).map_err(|e| bad(cmd, e))?;
    let pal = lib.get(id).ok_or_else(|| bad(cmd, format!("no palette `{id}`")))?;
    let v = match version_p(p) {
        Some(n) => Some(pal.version(n).ok_or_else(|| bad(cmd, format!("palette `{id}` has no version {n}")))?),
        None => None,
    };
    let mut out = palette_json(pal, v);
    if bool_p(p, "history").unwrap_or(false) {
        out["history"] = json!(pal.history);
    }
    Ok(out)
}

fn create_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "palettes.create";
    let pal = create(&dir_of(s, cmd)?, p).map_err(|e| bad(cmd, e))?;
    Ok(palette_json(&pal, None))
}

fn update_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "palettes.update";
    let u = update(&dir_of(s, cmd)?, p).map_err(|e| bad(cmd, e))?;
    let mut out = palette_json(&u.palette, None);
    out["newVersion"] = json!(u.new_version);
    Ok(out)
}

fn info_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "palettes.setInfo";
    let pal = set_info(&dir_of(s, cmd)?, p).map_err(|e| bad(cmd, e))?;
    Ok(summary_json(&pal))
}

fn archive_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "palettes.archive";
    let id = str_p(p, "id").ok_or_else(|| bad(cmd, "need `id`"))?;
    let by = who(p).map_err(|e| bad(cmd, e))?;
    let pal = set_archived(&dir_of(s, cmd)?, id, bool_p(p, "archived").unwrap_or(true), &by).map_err(|e| bad(cmd, e))?;
    Ok(summary_json(&pal))
}

fn duplicate_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "palettes.duplicate";
    let id = str_p(p, "id").ok_or_else(|| bad(cmd, "need `id`"))?;
    let by = who(p).map_err(|e| bad(cmd, e))?;
    let pal = duplicate(&dir_of(s, cmd)?, id, version_p(p), str_p(p, "name"), &by).map_err(|e| bad(cmd, e))?;
    Ok(palette_json(&pal, None))
}

fn delete_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "palettes.delete";
    let id = str_p(p, "id").ok_or_else(|| bad(cmd, "need `id`"))?;
    if !bool_p(p, "confirm").unwrap_or(false) {
        return Err(bad(cmd, "deleting a palette removes every version of it; pass confirm: true (archive keeps it)"));
    }
    if let Some(bd) = crate::brand_kits::Dirs::of(s) {
        let kits = crate::brand_kits::palette_users(&bd, id);
        if !kits.is_empty() {
            return Err(bad(cmd, format!("approved brand kits use this palette ({}); archive it instead", kits.join(", "))));
        }
    }
    let pal = delete(&dir_of(s, cmd)?, id).map_err(|e| bad(cmd, e))?;
    Ok(json!({"deleted": pal.id, "name": pal.name, "versions": pal.versions.len()}))
}

/// A colour reference: `pal_…@3/col_…` (version 3), `pal_…/col_…` (latest), or an object
/// `{palette, version?, color}`; the colour may also be given by its exact name.
pub fn parse_ref(p: &Value) -> Option<(String, Option<u32>, String)> {
    if let Some(r) = str_p(p, "ref") {
        let (pal, color) = r.split_once('/')?;
        let (pal, ver) = match pal.split_once('@') {
            Some((a, v)) => (a, Some(v.parse().ok()?)),
            None => (pal, None),
        };
        return Some((pal.to_string(), ver, color.to_string()));
    }
    Some((str_p(p, "palette")?.to_string(), version_p(p), str_p(p, "color")?.to_string()))
}

fn resolve_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "palettes.resolve";
    let (pid, ver, cref) = parse_ref(p).ok_or_else(|| bad(cmd, "need `ref` (pal_…@version/col_…) or `palette` and `color`"))?;
    let lib = load(&dir_of(s, cmd)?).map_err(|e| bad(cmd, e))?;
    let pal = lib.get(&pid).ok_or_else(|| bad(cmd, format!("no palette `{pid}`")))?;
    let v = match ver {
        Some(n) => pal.version(n).ok_or_else(|| bad(cmd, format!("palette `{pid}` has no version {n}")))?,
        None => pal.latest().ok_or_else(|| bad(cmd, "the palette has no versions"))?,
    };
    let c = v.color(&cref).ok_or_else(|| bad(cmd, format!("no colour `{cref}` in {pid} version {}", v.version)))?;
    let rules: Vec<Value> = v
        .rules
        .iter()
        .filter(|r| r.foreground.as_deref() == Some(c.id.as_str()) || r.background.as_deref() == Some(c.id.as_str()) || r.colors.contains(&c.id))
        .map(|r| rule_json(r, v))
        .collect();
    Ok(json!({"ref": format!("{}@{}/{}", pal.id, v.version, c.id), "palette": pal.id, "paletteName": pal.name, "version": v.version,
              "intact": v.intact(), "color": color_json(c), "rules": rules}))
}

fn to_project_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "palettes.addToProjectColors";
    let id = str_p(p, "id").ok_or_else(|| bad(cmd, "need `id`"))?;
    let lib = load(&dir_of(s, cmd)?).map_err(|e| bad(cmd, e))?;
    let pal = lib.get(id).ok_or_else(|| bad(cmd, format!("no palette `{id}`")))?;
    let v = match version_p(p) {
        Some(n) => pal.version(n).ok_or_else(|| bad(cmd, format!("palette `{id}` has no version {n}")))?,
        None => pal.latest().ok_or_else(|| bad(cmd, "the palette has no versions"))?,
    };
    let mut added = Vec::new();
    for c in &v.colors {
        // the project's own swatches (Project Colors): the same command the swatch manager uses
        let name: String = format!("{} — {}", pal.name, c.name).chars().take(128).collect();
        crate::palette::add(s, &json!({"color": c.hex.hex(), "name": name}))?;
        added.push(c.hex.hex());
    }
    Ok(json!({"added": added, "palette": pal.id, "version": v.version}))
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    let q = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    vec![
        q("palettes.list", "List Palettes", r#"{"query":string? (name, description, colour name, #hex or role),"includeArchived":bool=false}"#, list_cmd),
        q("palettes.get", "Get Palette", r#"{"id":pal_…,"version":n? (default latest),"history":bool=false}"#, get_cmd),
        q(
            "palettes.create",
            "Create Palette",
            r##"{"name":string,"description":string?,"colors":[{"name","hex":"#RRGGBB" | "rgb":[r,g,b],"role":"primary|secondary|accent|background|text|…"?,"shadeOf":colour id or name?,"shade":label?}]?,"rules":[{"kind":"permitted|prohibited|guideline","foreground":colour?,"background":colour?,"colors":[colour]?,"appliesTo":"text|graphics|any"?,"text":string?}]?,"by":who?,"note":string?}"##,
            create_cmd,
        ),
        q(
            "palettes.update",
            "Save Palette Version",
            r#"{"id":pal_…,"baseVersion":n (the version edited; refused when the palette moved on),"changes":[{"op":"addColor|updateColor|duplicateColor|removeColor|moveColor|addRule|updateRule|removeRule","id":?,"color":{…}?,"rule":{…}?,"to":index?}]?,"colors":[…]? (full list),"rules":[…]? (full list),"by":who?,"note":string?} — saves a new version; earlier versions never change"#,
            update_cmd,
        ),
        q("palettes.setInfo", "Rename Palette", r#"{"id":pal_…,"name":string?,"description":string?,"by":who?}"#, info_cmd),
        q("palettes.archive", "Archive Palette", r#"{"id":pal_…,"archived":bool=true (false restores),"by":who?}"#, archive_cmd),
        q("palettes.duplicate", "Duplicate Palette", r#"{"id":pal_…,"version":n?,"name":string?,"by":who?}"#, duplicate_cmd),
        q("palettes.delete", "Delete Palette", r#"{"id":pal_…,"confirm":true}"#, delete_cmd),
        q("palettes.resolve", "Resolve Palette Colour", r#"{"ref":"pal_…@version/col_…" | "palette":pal_…,"version":n?,"color":col_… or name}"#, resolve_cmd),
        q("palettes.addToProjectColors", "Add Palette to Project Colors", r#"{"id":pal_…,"version":n?}"#, to_project_cmd),
    ]
}
