//! Picture-quality measures for `analyze.takes`: sharpness (variance of the Laplacian of luma),
//! exposure (mean luma, clipped highlights, crushed shadows) and motion (mean absolute luma
//! difference from the previous sample). Pure functions on RGBA8 frames.

/// Measures of one sampled frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    /// Variance of the 4-neighbour Laplacian of luma (0..255 scale). Higher = more detail in focus.
    pub sharpness: f32,
    /// Mean luma, 0..1.
    pub luma: f32,
    /// Fraction of pixels at or above 250/255 luma.
    pub clipped: f32,
    /// Fraction of pixels at or below 5/255 luma.
    pub crushed: f32,
    /// Mean absolute luma change from the previous sample, 0..1 (0 for the first).
    pub motion: f32,
}

/// Rec. 709 luma of 8-bit R'G'B' (code values).
fn luma8(px: &[u8]) -> f32 {
    match px {
        [r, g, b, ..] => 0.2126 * *r as f32 + 0.7152 * *g as f32 + 0.0722 * *b as f32,
        _ => 0.0,
    }
}

/// Luma plane (0..255) of an RGBA8 frame; empty if the buffer is too short for `w`×`h`.
pub fn luma_plane(rgba: &[u8], w: usize, h: usize) -> Vec<f32> {
    let Some(n) = w.checked_mul(h) else { return Vec::new() };
    if n == 0 || rgba.len() < n.saturating_mul(4) {
        return Vec::new();
    }
    rgba.chunks_exact(4).take(n).map(luma8).collect()
}

/// Measure a luma plane (from [`luma_plane`]) against the previous sample's plane.
pub fn measure(luma: &[f32], w: usize, h: usize, prev: Option<&[f32]>) -> FrameStats {
    let n = luma.len();
    if n == 0 || w.saturating_mul(h) != n {
        return FrameStats::default();
    }
    let mean = luma.iter().sum::<f32>() / n as f32;
    let clipped = luma.iter().filter(|v| **v >= 250.0).count() as f32 / n as f32;
    let crushed = luma.iter().filter(|v| **v <= 5.0).count() as f32 / n as f32;
    // Laplacian over the interior
    let (mut sum, mut sum2, mut count) = (0f64, 0f64, 0usize);
    for y in 1..h.saturating_sub(1) {
        let row = |yy: usize| luma.get(yy * w..yy * w + w).unwrap_or(&[]);
        let (up, mid, down) = (row(y - 1), row(y), row(y + 1));
        for x in 1..w.saturating_sub(1) {
            let (Some(c), Some(l), Some(r), Some(u), Some(d)) = (mid.get(x), mid.get(x - 1), mid.get(x + 1), up.get(x), down.get(x)) else { continue };
            let lap = (l + r + u + d - 4.0 * c) as f64;
            sum += lap;
            sum2 += lap * lap;
            count += 1;
        }
    }
    let sharpness = if count > 0 {
        let m = sum / count as f64;
        (sum2 / count as f64 - m * m).max(0.0) as f32
    } else {
        0.0
    };
    let motion = match prev {
        Some(p) if p.len() == n => luma.iter().zip(p).map(|(a, b)| (a - b).abs()).sum::<f32>() / n as f32 / 255.0,
        _ => 0.0,
    };
    FrameStats { sharpness, luma: mean / 255.0, clipped, crushed, motion }
}

/// One 0..100 quality score for a sample: rewards focus, penalises bad exposure and shake.
/// `sharp_ref` is the sharpest sample of the batch (so scores compare takes of the same scene).
pub fn score(f: &FrameStats, sharp_ref: f32) -> f32 {
    let focus = if sharp_ref > 0.0 { (f.sharpness / sharp_ref).clamp(0.0, 1.0) } else { 0.0 };
    // exposure: best near mid-grey-ish average, penalise clipping and crushing
    let exposure = (1.0 - ((f.luma - 0.45).abs() / 0.45)).clamp(0.0, 1.0) * (1.0 - (f.clipped * 4.0).min(1.0)) * (1.0 - (f.crushed * 3.0).min(1.0));
    // motion: some is fine (camera moves, action); a lot reads as shake or a whip
    let steady = 1.0 - ((f.motion - 0.04).max(0.0) * 8.0).min(1.0);
    (100.0 * (0.5 * focus + 0.3 * exposure + 0.2 * steady)).clamp(0.0, 100.0)
}

/// Problems worth naming in a take's report.
pub fn flags(f: &FrameStats, sharp_ref: f32) -> Vec<&'static str> {
    let mut v = Vec::new();
    if sharp_ref > 0.0 && f.sharpness < 0.35 * sharp_ref {
        v.push("soft");
    }
    if f.clipped > 0.05 {
        v.push("clipped highlights");
    }
    if f.crushed > 0.15 {
        v.push("crushed shadows");
    }
    if f.luma < 0.12 {
        v.push("underexposed");
    } else if f.luma > 0.85 {
        v.push("overexposed");
    }
    if f.motion > 0.12 {
        v.push("heavy motion");
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let c = f(x, y);
                v.extend_from_slice(&[c, c, c, 255]);
            }
        }
        v
    }

    #[test]
    fn sharp_beats_blurry_and_exposure_is_measured() {
        let (w, h) = (64, 48);
        let sharp = frame(w, h, |x, y| if (x / 2 + y / 2) % 2 == 0 { 60 } else { 190 });
        let flat = frame(w, h, |_, _| 125);
        let s = measure(&luma_plane(&sharp, w, h), w, h, None);
        let b = measure(&luma_plane(&flat, w, h), w, h, None);
        assert!(s.sharpness > 1000.0 && b.sharpness < 1e-3, "{s:?} {b:?}");
        assert!(score(&s, s.sharpness) > score(&b, s.sharpness));
        assert!(flags(&b, s.sharpness).contains(&"soft"));
        let white = measure(&luma_plane(&frame(w, h, |_, _| 255), w, h), w, h, None);
        assert!(white.clipped > 0.99 && flags(&white, 1.0).contains(&"overexposed"));
        let black = measure(&luma_plane(&frame(w, h, |_, _| 0), w, h), w, h, None);
        assert!(black.crushed > 0.99 && flags(&black, 1.0).contains(&"underexposed"));
    }

    #[test]
    fn motion_compares_with_the_previous_sample() {
        let (w, h) = (32, 32);
        let a = luma_plane(&frame(w, h, |x, _| (x * 8) as u8), w, h);
        let b = luma_plane(&frame(w, h, |x, _| 255 - (x * 8) as u8), w, h);
        assert_eq!(measure(&a, w, h, Some(&a)).motion, 0.0);
        let m = measure(&b, w, h, Some(&a)).motion;
        assert!(m > 0.3, "{m}");
        assert!(flags(&measure(&b, w, h, Some(&a)), 1.0).contains(&"heavy motion"));
    }

    #[test]
    fn hostile_sizes_do_not_panic() {
        assert!(luma_plane(&[1, 2, 3], 10, 10).is_empty());
        assert!(luma_plane(&[], usize::MAX, 2).is_empty());
        assert_eq!(measure(&[], 0, 0, None), FrameStats::default());
        assert_eq!(measure(&[1.0; 5], 2, 2, None), FrameStats::default());
        let one = measure(&[7.0], 1, 1, Some(&[1.0, 2.0]));
        assert_eq!(one.sharpness, 0.0);
        assert_eq!(score(&one, 0.0), score(&one, -1.0));
    }
}
