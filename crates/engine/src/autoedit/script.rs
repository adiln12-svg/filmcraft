//! Reading approved scripts as ARCA's Hook Generator writes them, without reformatting:
//!
//! ```text
//! Full Narration (V1 Haircut):
//! - [0–3s Hook] "most guys have a haircut that's quietly working against them."
//! - [8–15s Expansion] "so they look fine, never sharp." (re-hook · RP-013 information withholding)
//! ```
//!
//! Each dialogue line keeps its beat label and target timing as metadata; only the quoted words
//! are spoken text. Plain lines (no quotes, no labels) are spoken text as written. Headers and
//! `Label: value` metadata lines are skipped and reported, never guessed into dialogue.
//! The spoken text is kept verbatim: FilmCraft never rewrites an approved script.

/// One spoken line of an approved script.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptLine {
    /// 1-based line number, in script order.
    pub n: usize,
    /// Beat label (`Hook`, `Context`, …) when the script has one.
    pub beat: Option<String>,
    /// Target timing in seconds from the beat label (`0–3s` → (0, 3)).
    pub target: Option<(f64, f64)>,
    /// The words to be spoken, verbatim.
    pub text: String,
    /// The line as written in the script.
    pub raw: String,
}

/// A line of the input that is not dialogue.
#[derive(Clone, Debug, PartialEq)]
pub struct Skipped {
    pub raw: String,
    pub reason: &'static str,
}

/// Longest script read (characters) and most lines kept.
pub const MAX_SCRIPT_CHARS: usize = 200_000;
pub const MAX_LINES: usize = 2_000;

const QUOTES: &[(char, char)] = &[('"', '"'), ('“', '”'), ('«', '»')];

/// Parse a script. Returns the spoken lines and the lines skipped with the reason.
pub fn parse(script: &str) -> (Vec<ScriptLine>, Vec<Skipped>) {
    let mut lines = Vec::new();
    let mut skipped = Vec::new();
    let mut end = script.len().min(MAX_SCRIPT_CHARS);
    while !script.is_char_boundary(end) {
        end -= 1;
    }
    for raw in script.get(..end).unwrap_or("").lines() {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        if lines.len() >= MAX_LINES {
            skipped.push(Skipped { raw: t.into(), reason: "too many lines" });
            continue;
        }
        match parse_line(t) {
            Ok((beat, target, text)) => lines.push(ScriptLine { n: lines.len() + 1, beat, target, text, raw: t.into() }),
            Err(reason) => skipped.push(Skipped { raw: t.into(), reason }),
        }
    }
    (lines, skipped)
}

type Parsed = (Option<String>, Option<(f64, f64)>, String);

fn parse_line(t: &str) -> std::result::Result<Parsed, &'static str> {
    if t.starts_with('#') || t.starts_with("---") || t.starts_with('>') || t.starts_with('|') {
        return Err("heading or formatting");
    }
    let mut s = strip_bullet(t);
    // a `Line 3:` / `L3:` prefix numbers dialogue
    if let Some(rest) = strip_line_number(s) {
        s = rest;
    }
    // a leading [beat] label
    let (mut beat, mut target) = (None, None);
    if let Some(rest) = s.strip_prefix('[')
        && let Some(close) = rest.find(']')
    {
        let label = rest.get(..close).unwrap_or("").trim();
        (beat, target) = beat_label(label);
        s = rest.get(close + 1..).unwrap_or("").trim_start();
        s = s.trim_start_matches([':', '-', '–', '—']).trim_start();
    }
    let quoted = quoted_text(s);
    let has_label = beat.is_some() || target.is_some();
    if let Some(q) = quoted {
        // `Key: "value"` lines are metadata (hook variants, on-screen text) unless they carry a
        // beat label: dialogue lines are bulleted beats or bare quotes
        if !has_label && is_key_value(s) {
            return Err("label: value metadata (not a dialogue beat)");
        }
        let q = q.trim().to_string();
        return if q.is_empty() { Err("empty quote") } else { Ok((beat, target, q)) };
    }
    if s.ends_with(':') {
        return Err("header");
    }
    if !has_label && is_key_value(s) {
        return Err("label: value metadata (not a dialogue beat)");
    }
    let text = strip_trailing_parentheses(s).trim().to_string();
    if text.is_empty() || !text.chars().any(char::is_alphanumeric) {
        return Err("no words");
    }
    Ok((beat, target, text))
}

fn strip_bullet(t: &str) -> &str {
    let s = t.trim_start_matches(['-', '*', '•', '·', '–', '—']).trim_start();
    // `1.` / `1)` numbering
    let digits = s.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && digits <= 3 {
        let rest = s.get(digits..).unwrap_or("");
        if let Some(r) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')')) {
            return r.trim_start();
        }
    }
    s
}

fn strip_line_number(s: &str) -> Option<&str> {
    let lower = s.to_ascii_lowercase();
    let after = if lower.starts_with("line ") {
        s.get(5..)?
    } else if lower.starts_with('l') && s.get(1..2).is_some_and(|c| c.chars().all(|c| c.is_ascii_digit())) {
        s.get(1..)?
    } else {
        return None;
    };
    let digits = after.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = after.get(digits..)?.trim_start();
    rest.strip_prefix(':').or_else(|| rest.strip_prefix('.')).or_else(|| rest.strip_prefix(')')).map(str::trim_start)
}

/// `0–3s Hook` → (Some("Hook"), Some((0, 3))); `Hook` → (Some("Hook"), None).
fn beat_label(label: &str) -> (Option<String>, Option<(f64, f64)>) {
    let mut words = label.split_whitespace();
    let first = words.next().unwrap_or("");
    let timing = parse_range(first);
    let rest: Vec<&str> = if timing.is_some() { words.collect() } else { label.split_whitespace().collect() };
    let name = rest.join(" ");
    ((!name.is_empty()).then_some(name), timing)
}

fn parse_range(s: &str) -> Option<(f64, f64)> {
    let s = s.trim_end_matches(['s', 'S']);
    let (a, b) = s.split_once(['–', '-', '—'])?;
    let (a, b) = (a.trim().trim_end_matches(['s', 'S']).parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
    (a.is_finite() && b.is_finite() && b >= a && a >= 0.0).then_some((a, b))
}

/// The quoted text of a line (several quotes are joined with a space), if it has any.
fn quoted_text(s: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = s;
    while let Some((i, open, close)) = QUOTES.iter().filter_map(|(o, c)| rest.find(*o).map(|i| (i, *o, *c))).min_by_key(|x| x.0) {
        let after = rest.get(i + open.len_utf8()..).unwrap_or("");
        let Some(j) = after.find(close) else { break };
        out.push(after.get(..j).unwrap_or("").to_string());
        rest = after.get(j + close.len_utf8()..).unwrap_or("");
    }
    (!out.is_empty()).then(|| out.join(" "))
}

/// Field names ARCA uses for metadata lines around scripts.
const METADATA_KEYS: &[&str] = &[
    "objective",
    "on-screen",
    "on screen",
    "ost",
    "prompt",
    "re-hook",
    "rehook",
    "cta",
    "caption",
    "hook type",
    "format",
    "script tier",
    "note",
    "notes",
    "inputs",
    "source map",
    "performance intent",
    "engagement",
];

/// `Engagement objective: Save`, `V1 Haircut: "…"`, `On-screen: "…"`: a short label before a
/// colon that is a known ARCA field, a variation label (`V1 …`), or introduces a quoted value.
/// A dialogue line with a colon in it ("here's the thing: …") is not metadata.
fn is_key_value(s: &str) -> bool {
    let Some(i) = s.find(':') else { return false };
    let key = s.get(..i).unwrap_or("").trim();
    let value = s.get(i + 1..).unwrap_or("").trim_start();
    if key.is_empty() || key.chars().count() > 40 || key.split_whitespace().count() > 5 || QUOTES.iter().any(|(o, _)| key.contains(*o)) {
        return false;
    }
    let lower = key.to_lowercase();
    let variation = key.starts_with('V') && key.chars().nth(1).is_some_and(|c| c.is_ascii_digit());
    let quoted_value = QUOTES.iter().any(|(o, _)| value.starts_with(*o));
    variation || quoted_value || METADATA_KEYS.iter().any(|k| lower.contains(k))
}

fn strip_trailing_parentheses(s: &str) -> &str {
    let mut s = s.trim_end();
    while s.ends_with(')') {
        let Some(open) = s.rfind('(') else { break };
        s = s.get(..open).unwrap_or("").trim_end();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARCA: &str = r#"Full Narration (V1 Haircut):
- [0–3s Hook] "most guys have a haircut that's quietly working against them."
- [3–8s Context] "not a bad cut. just one that doesn't fit their face, their hairline, their head shape."
- [8–15s Expansion] "so they look fine, never sharp." (re-hook · RP-013 information withholding)
- [15–22s Payoff] "the second it's actually designed for you, people can't tell what changed — they just know something did."
- [22–25s CTA] "comment if your cut's been carrying you or holding you back."
Engagement objective: Comment · Re-hooks: 1 (RP-013)"#;

    #[test]
    fn reads_arca_narration_scripts_verbatim() {
        let (lines, skipped) = parse(ARCA);
        assert_eq!(lines.len(), 5, "{lines:#?}");
        assert_eq!(lines[0].beat.as_deref(), Some("Hook"));
        assert_eq!(lines[0].target, Some((0.0, 3.0)));
        assert_eq!(lines[0].text, "most guys have a haircut that's quietly working against them.");
        assert_eq!(lines[2].text, "so they look fine, never sharp.", "re-hook note is not spoken");
        assert_eq!(lines[4].target, Some((22.0, 25.0)));
        assert_eq!(lines.iter().map(|l| l.n).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
        assert_eq!(skipped.len(), 2, "header and metadata: {skipped:#?}");
    }

    #[test]
    fn plain_numbered_and_curly_quoted_scripts() {
        let (l, _) = parse("the king has fled\n\n  i also offered to help (aside)\n");
        assert_eq!(l.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), vec!["the king has fled", "i also offered to help"]);
        let (l, _) = parse("1. “first line”\n2) [Hook] second line\nLine 3: third line\nL4: fourth");
        assert_eq!(l.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), vec!["first line", "second line", "third line", "fourth"]);
        assert_eq!(l[1].beat.as_deref(), Some("Hook"));
        let (l, s) = parse("V1 Haircut: \"a hook variant\"\nOn-screen: \"washing ≠ fixing\"\n# Heading\n| table |");
        assert!(l.is_empty(), "{l:#?}");
        assert_eq!(s.len(), 4);
        // a colon inside dialogue does not make it metadata
        let (l, _) = parse("Here's the thing: nobody tells you this\n- [Hook] \"note: still dialogue\"");
        assert_eq!(l.len(), 2, "{l:#?}");
        assert_eq!(l[0].text, "Here's the thing: nobody tells you this");
    }

    #[test]
    fn hostile_scripts() {
        assert!(parse("").0.is_empty());
        assert!(parse("[\n]\n\"\n\"\"\n[0–3s] \"\"\n(((\n").0.is_empty());
        let (l, _) = parse("[9–3s Hook] \"backwards timing\"\n[abc–s Hook] \"bad timing\"");
        assert_eq!(l[0].target, None);
        assert_eq!(l[1].beat.as_deref(), Some("abc–s Hook"));
        let long = "é".repeat(MAX_SCRIPT_CHARS);
        let _ = parse(&long);
        let many = "word\n".repeat(MAX_LINES + 10);
        assert_eq!(parse(&many).0.len(), MAX_LINES);
    }
}
