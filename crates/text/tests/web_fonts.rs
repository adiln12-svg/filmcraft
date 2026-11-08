//! WOFF / WOFF2 decoding, variable-font named instances and exact font status.
//!
//! Real web fonts are not committed (AGENTS.md §1.9). The WOFF2 and variable-font tests run on
//! the font files in `FILMCRAFT_TEST_FONTS` (a folder holding `archivo.woff2`, the Archivo
//! variable font, and optionally `inter.woff2`) and are skipped when it is unset. The WOFF 1.0
//! test wraps the bundled Inter and always runs.

use filmcraft_text::fonts::{self, FontStatus};
use filmcraft_text::{ParagraphStyle, TextStyle, layout, render};

fn test_font(name: &str) -> Option<Vec<u8>> {
    let dir = std::env::var_os("FILMCRAFT_TEST_FONTS")?;
    std::fs::read(std::path::Path::new(&dir).join(name)).ok()
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// A WOFF 1.0 file holding `sfnt` with every table stored uncompressed (valid per the spec).
fn woff1(sfnt: &[u8]) -> Vec<u8> {
    let n = u16::from_be_bytes([sfnt[4], sfnt[5]]) as usize;
    let mut tables: Vec<([u8; 4], u32, &[u8])> = (0..n)
        .map(|i| {
            let r = 12 + i * 16;
            let (off, len) = (be32(sfnt, r + 8) as usize, be32(sfnt, r + 12) as usize);
            ([sfnt[r], sfnt[r + 1], sfnt[r + 2], sfnt[r + 3]], be32(sfnt, r + 4), &sfnt[off..off + len])
        })
        .collect();
    tables.sort_by_key(|t| t.0);
    let mut data = Vec::new();
    let mut dir = Vec::new();
    let mut at = 44 + 20 * n;
    let mut sfnt_size = 12 + 16 * n;
    for (tag, sum, bytes) in &tables {
        dir.extend_from_slice(tag);
        dir.extend_from_slice(&(at as u32).to_be_bytes());
        dir.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        dir.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        dir.extend_from_slice(&sum.to_be_bytes());
        data.extend_from_slice(bytes);
        while data.len() % 4 != 0 {
            data.push(0);
        }
        at = 44 + 20 * n + data.len();
        sfnt_size += bytes.len().div_ceil(4) * 4;
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"wOFF");
    out.extend_from_slice(&sfnt[0..4]);
    out.extend_from_slice(&((44 + 20 * n + data.len()) as u32).to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&(sfnt_size as u32).to_be_bytes());
    out.extend_from_slice(&[0, 1, 0, 0]);
    out.extend_from_slice(&[0; 20]);
    out.extend(dir);
    out.extend(data);
    out
}

fn ink(family: &str, style: &str) -> f64 {
    let st = TextStyle { family: family.into(), style: style.into(), size: 96.0, ..Default::default() };
    let l = layout("HAMBURGEFONSTIV", &st, &ParagraphStyle::default());
    let (m, _, _) = render::rasterize(&l, &render::at(0.0, 100.0));
    m.a.iter().map(|&v| f64::from(v)).sum()
}

#[test]
fn woff1_decodes_to_the_same_font() {
    let w = woff1(fonts::INTER_REGULAR);
    assert_eq!(fonts::container_format(&w), Some("woff"));
    let faces = fonts::describe(&w).expect("WOFF decodes");
    assert_eq!(faces[0].family, "Inter");
    let sfnt = fonts::decode_container(w).expect("decode");
    assert_eq!(fonts::container_format(&sfnt), Some("ttf"));
}

#[test]
fn hostile_font_bytes_are_errors_not_crashes() {
    for bad in [&b""[..], b"wOF2", b"wOF2\0\0\0\0garbage", b"wOFF\0\x01\0\0\xff\xff\xff\xff", b"OTTO\0\0", b"not a font at all"] {
        assert!(fonts::describe(bad).is_err(), "{bad:?}");
        assert!(fonts::add_font_data(bad.to_vec()).is_empty());
    }
    // a truncated WOFF2 / WOFF
    let w = woff1(fonts::INTER_REGULAR);
    for cut in [10, 44, 100, w.len() / 2] {
        let _ = fonts::describe(&w[..cut]);
    }
    if let Some(a) = test_font("archivo.woff2") {
        for cut in [12, 48, 200, a.len() / 3, a.len() - 1] {
            let _ = fonts::describe(&a[..cut]);
        }
        let mut flipped = a.clone();
        for i in (60..flipped.len()).step_by(97) {
            flipped[i] ^= 0x5a;
        }
        let _ = fonts::describe(&flipped);
    }
}

#[test]
fn missing_family_and_style_are_reported_not_hidden() {
    assert_eq!(fonts::status("No Such Family 7f3a", "Regular"), FontStatus::Missing);
    assert!(fonts::status("Inter", "Regular").is_exact());
    // a style no Inter has: another Inter face stands in, and that is reported
    assert!(matches!(fonts::status("Inter", "Condensed Hairline 7f3a"), FontStatus::Substituted { .. }));
}

#[test]
fn archivo_woff2_variable_weights_render_including_900() {
    let Some(a) = test_font("archivo.woff2") else {
        eprintln!("FILMCRAFT_TEST_FONTS not set: skipping the Archivo WOFF2 test");
        return;
    };
    assert_eq!(fonts::container_format(&a), Some("woff2"));
    let faces = fonts::describe(&a).expect("Archivo WOFF2 decodes");
    let black = faces.iter().find(|f| f.style == "Black").expect("a Black instance");
    assert_eq!(black.family, "Archivo");
    assert_eq!(black.weight, 900);
    assert!(faces.iter().any(|f| f.style == "Thin" && f.weight == 100));
    assert!(faces.iter().any(|f| f.style == "Regular" && f.weight == 400));

    assert_eq!(fonts::status("Archivo", "Black"), FontStatus::Missing, "not registered yet");
    let ids = fonts::add_font_data_from(a, "test");
    assert!(ids.len() >= 9, "one face per named instance, got {}", ids.len());
    for style in ["Thin", "Regular", "Bold", "Black"] {
        assert!(fonts::status("Archivo", style).is_exact(), "Archivo {style}");
    }
    let (thin, regular, bold, heavy) = (ink("Archivo", "Thin"), ink("Archivo", "Regular"), ink("Archivo", "Bold"), ink("Archivo", "Black"));
    eprintln!("ink: Thin {thin:.0}, Regular {regular:.0}, Bold {bold:.0}, Black {heavy:.0}");
    assert!(thin < regular && regular < bold && bold < heavy, "weights must get heavier");
    assert!(heavy > regular * 1.6, "Black (900) must be drawn at weight 900, not the default instance");
    // Chromium drawing the same WOFF2 (`font-weight: 100/400/700/900`, 96 px, canvas fillText):
    // advance widths from measureText and coverage (sum of alpha) of the same string. Widths
    // show the instance's metrics (HVAR) are used, ink that its outlines (gvar) are.
    let reference = [("Thin", 974.78, 13325.6), ("Regular", 980.14, 22238.0), ("Bold", 1009.45, 32666.2), ("Black", 1099.30, 44173.8)];
    for (style, width, ink_ref) in reference {
        let st = TextStyle { family: "Archivo".into(), style: style.into(), size: 96.0, ..Default::default() };
        let w = layout("HAMBURGEFONSTIV", &st, &ParagraphStyle::default()).bounds[2];
        let got = ink("Archivo", style);
        eprintln!("{style}: width {w:.2} (Chromium {width}), ink {got:.0} (Chromium {ink_ref})");
        assert!((f64::from(w) - width).abs() / width < 0.005, "{style} width {w} vs Chromium {width}");
        assert!((got - ink_ref).abs() / ink_ref < 0.02, "{style} ink {got} vs Chromium {ink_ref}");
    }
}

#[test]
fn inter_woff2_registers() {
    let Some(i) = test_font("inter.woff2") else { return };
    let faces = fonts::describe(&i).expect("Inter WOFF2 decodes");
    assert!(faces.iter().all(|f| f.family.starts_with("Inter")));
}

/// Text laid out while its font was missing is laid out again once the font is installed (the
/// layout cache keys on the font database's generation).
#[test]
fn installing_a_font_relays_out_cached_text() {
    let mut b = fonts::INTER_REGULAR.to_vec();
    let from: Vec<u8> = "Inter".encode_utf16().flat_map(u16::to_be_bytes).collect();
    let to: Vec<u8> = "Gener".encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut i = 0;
    while i + from.len() <= b.len() {
        if b[i..i + from.len()] == from[..] {
            b[i..i + from.len()].copy_from_slice(&to);
        }
        i += 1;
    }
    let st = TextStyle { family: "Gener".into(), style: "Regular".into(), size: 40.0, ..Default::default() };
    let before = layout("cached", &st, &ParagraphStyle::default());
    assert!(before.missing_font);
    assert!(!fonts::add_font_data_from(b, "test").is_empty());
    let after = layout("cached", &st, &ParagraphStyle::default());
    assert!(!after.missing_font, "the cached layout of the missing font must not be reused");
}
