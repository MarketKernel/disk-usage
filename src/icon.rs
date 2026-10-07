//! The app icon, drawn procedurally: a small sunburst on a dark rounded square.
//!
//! The window icon is rendered at startup; `assets/icon.png`, `assets/icon-256.png` and
//! `assets/icon.ico` (used by the macOS bundle, the Debian package and the Windows
//! executable) are produced from the same code by `cargo test write_icon_assets -- --ignored`.

use std::f32::consts::TAU;

use crate::colors;

/// Sector as (start, end) fractions of a turn.
type Arc = (f32, f32);

/// (inner radius, outer radius, sectors) per ring.
const RINGS: [(f32, f32, &[Arc]); 3] = [
    (0.17, 0.38, &[(0.0, 0.46), (0.46, 0.74), (0.74, 0.9), (0.9, 1.0)]),
    (0.38, 0.58, &[(0.0, 0.3), (0.3, 0.44), (0.46, 0.62), (0.62, 0.74), (0.74, 0.86), (0.9, 0.97)]),
    (0.58, 0.78, &[(0.02, 0.22), (0.3, 0.4), (0.46, 0.56), (0.76, 0.84)]),
];

/// RGBA pixels of a `size`×`size` icon.
pub fn rgba(size: u32) -> Vec<u8> {
    const SS: u32 = 4; // supersampling per axis
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut acc = [0.0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    // Normalized coordinates in [-1, 1].
                    let u = ((x * SS + sx) as f32 + 0.5) / (size * SS) as f32 * 2.0 - 1.0;
                    let v = ((y * SS + sy) as f32 + 0.5) / (size * SS) as f32 * 2.0 - 1.0;
                    if let Some([r, g, b]) = sample(u, v) {
                        acc[0] += r;
                        acc[1] += g;
                        acc[2] += b;
                        acc[3] += 1.0;
                    }
                }
            }
            let n = (SS * SS) as f32;
            let a = acc[3];
            if a == 0.0 {
                out.extend([0, 0, 0, 0]);
            } else {
                // Straight (unpremultiplied) alpha.
                out.extend([acc[0] / a, acc[1] / a, acc[2] / a, a / n].map(|c| (c * 255.0).round() as u8));
            }
        }
    }
    out
}

/// Color at a point of the icon, `None` outside the rounded square.
fn sample(u: f32, v: f32) -> Option<[f32; 3]> {
    // Rounded square background (macOS-style inset).
    let (half, radius) = (0.84, 0.36);
    let dx = (u.abs() - (half - radius)).max(0.0);
    let dy = (v.abs() - (half - radius)).max(0.0);
    if u.abs() > half || v.abs() > half || dx * dx + dy * dy > radius * radius {
        return None;
    }
    let bg = colors::BG.to_array();
    let mut color = [bg[0], bg[1], bg[2]].map(|c| c as f32 / 255.0);

    let r = (u * u + v * v).sqrt();
    let turn = u.atan2(-v).rem_euclid(TAU) / TAU;
    for (ring, &(r0, r1, sectors)) in RINGS.iter().enumerate() {
        // A thin gap between rings and sectors, like on the chart.
        let gap = 0.012;
        if r < r0 + gap || r > r1 - gap {
            continue;
        }
        let angular_gap = gap / r / TAU;
        if let Some(&(a0, a1)) =
            sectors.iter().find(|&&(a0, a1)| turn >= a0 + angular_gap && turn <= a1 - angular_gap)
        {
            // Like the chart, bigger arcs are redder (log scale); stretch the icon's few
            // arcs (spanning 0.07..0.46 of a turn) over the whole rainbow.
            let t = ((0.46 / (a1 - a0)).ln() / (0.46f32 / 0.07).ln()).clamp(0.0, 1.0);
            let share = colors::SHARE_RED * (colors::SHARE_VIOLET / colors::SHARE_RED).powf(t);
            let c = colors::sector(share, ring as f32, true).to_array();
            color = [c[0], c[1], c[2]].map(|c| c as f32 / 255.0);
        }
    }
    Some(color)
}

pub fn window_icon() -> eframe::egui::IconData {
    const SIZE: u32 = 128;
    eframe::egui::IconData { rgba: rgba(SIZE), width: SIZE, height: SIZE }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(size: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::write_buffer_with_format(
            &mut std::io::Cursor::new(&mut bytes),
            &rgba(size),
            size,
            size,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .unwrap();
        bytes
    }

    #[test]
    fn renders_transparent_corners_and_opaque_center() {
        let px = rgba(32);
        assert_eq!(px.len(), 32 * 32 * 4);
        assert_eq!(px[3], 0); // top-left corner
        let center = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(px[center + 3], 255);
    }

    /// Regenerates the icon files checked into `assets/`.
    #[test]
    #[ignore]
    fn write_icon_assets() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("icon.png"), png(1024)).unwrap();
        std::fs::write(dir.join("icon-256.png"), png(256)).unwrap();

        // ICO with embedded PNG images (supported since Windows Vista).
        let sizes = [16u32, 24, 32, 48, 64, 128, 256];
        let images: Vec<Vec<u8>> = sizes.iter().map(|&s| png(s)).collect();
        let mut ico = Vec::new();
        ico.extend([0, 0, 1, 0]);
        ico.extend((sizes.len() as u16).to_le_bytes());
        let mut offset = 6 + 16 * sizes.len() as u32;
        for (&size, data) in sizes.iter().zip(&images) {
            let dim = if size >= 256 { 0 } else { size as u8 };
            ico.extend([dim, dim, 0, 0]);
            ico.extend(1u16.to_le_bytes()); // color planes
            ico.extend(32u16.to_le_bytes()); // bits per pixel
            ico.extend((data.len() as u32).to_le_bytes());
            ico.extend(offset.to_le_bytes());
            offset += data.len() as u32;
        }
        for data in &images {
            ico.extend(data);
        }
        std::fs::write(dir.join("icon.ico"), ico).unwrap();
    }
}
