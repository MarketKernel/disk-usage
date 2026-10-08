//! Palette: a dark background and pastel sectors colored by size along the rainbow.

use eframe::egui::Color32;

pub const BG: Color32 = Color32::from_rgb(0x24, 0x25, 0x2a);
pub const PANEL: Color32 = Color32::from_rgb(0x2b, 0x2c, 0x32);
pub const SMALL: Color32 = Color32::from_rgb(0x55, 0x56, 0x5e);
pub const TEXT: Color32 = Color32::from_rgb(0xe8, 0xe8, 0xec);
pub const TEXT_WEAK: Color32 = Color32::from_rgb(0x9a, 0x9b, 0xa3);
pub const ACCENT: Color32 = Color32::WHITE;
pub const WARNING: Color32 = Color32::from_rgb(0xf0, 0xa0, 0x6a);
pub const DANGER: Color32 = Color32::from_rgb(0xc0, 0x3a, 0x3a);

/// Share of the current view that is pure red; anything bigger stays red.
pub const SHARE_RED: f32 = 0.3;
/// Share that is pure violet; anything smaller stays violet. The chart merges items
/// below ~0.06% of the view, so this puts the whole rainbow on what is visible.
pub const SHARE_VIOLET: f32 = 0.001;
/// Rainbow from red (0°) to violet (270°).
const HUE_VIOLET: f32 = 270.0 / 360.0;

/// Position on the rainbow for a share of the view: 0 = red (big), 1 = violet (small).
/// Logarithmic, since sizes on a disk span many orders of magnitude.
pub fn rainbow_position(share: f32) -> f32 {
    let (hi, lo) = (SHARE_RED.log10(), SHARE_VIOLET.log10());
    ((hi - share.max(1e-12).log10()) / (hi - lo)).clamp(0.0, 1.0)
}

/// Pastel color for an item taking `share` (0..=1) of the current view, on ring `ring`
/// (0 = innermost; fractional while animating). Deeper rings are paler, files more muted.
pub fn sector(share: f32, ring: f32, is_dir: bool) -> Color32 {
    let hue = rainbow_position(share) * HUE_VIOLET;
    let mut sat = (0.5 - 0.045 * ring).max(0.18);
    let mut val = 0.97;
    if !is_dir {
        sat *= 0.8;
        val = 0.9;
    }
    hsv(hue, sat, val)
}

/// Plain sRGB HSV (egui's HSV types work in linear space and wash colors out).
fn hsv(h: f32, s: f32, v: f32) -> Color32 {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h6 % 2.0 - 1.0).abs());
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let byte = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(byte(r), byte(g), byte(b))
}

pub fn hovered(c: Color32) -> Color32 {
    c.lerp_to_gamma(Color32::WHITE, 0.35)
}

/// Items queued for the Trash turn gray, a bit darker than they were, so the sectors
/// stay distinguishable.
pub fn queued(c: Color32) -> Color32 {
    let luma = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    Color32::from_gray((luma * 0.62).round() as u8)
}

pub fn dimmed(c: Color32) -> Color32 {
    c.lerp_to_gamma(BG, 0.55)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_is_red_small_is_violet() {
        assert_eq!(rainbow_position(0.9), 0.0);
        assert_eq!(rainbow_position(SHARE_RED), 0.0);
        assert_eq!(rainbow_position(SHARE_VIOLET), 1.0);
        assert_eq!(rainbow_position(0.0), 1.0);
        assert!(rainbow_position(0.1) < rainbow_position(0.01));
        // Each step of 10x is an equal step along the rainbow.
        let step = rainbow_position(0.02) - rainbow_position(0.2);
        assert!((rainbow_position(0.002) - rainbow_position(0.02) - step).abs() < 1e-5);
    }

    #[test]
    fn hsv_primaries() {
        assert_eq!(hsv(0.0, 1.0, 1.0), Color32::from_rgb(255, 0, 0));
        assert_eq!(hsv(1.0 / 3.0, 1.0, 1.0), Color32::from_rgb(0, 255, 0));
        assert_eq!(hsv(2.0 / 3.0, 1.0, 1.0), Color32::from_rgb(0, 0, 255));
        assert_eq!(hsv(0.5, 0.0, 0.5), Color32::from_gray(128));
    }
}
