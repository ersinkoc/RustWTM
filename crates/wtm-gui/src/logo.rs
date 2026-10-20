//! The RustWTM logo, drawn in code so every size is crisp: a gauge whose arc
//! runs from green to orange and a needle held in check, on a dark tile.
//!
//! Used for the window and tray icons; the tests also write the committed
//! `assets/rustwtm.ico` (embedded in the executables and the installer) and
//! `assets/logo.svg`.

use std::f32::consts::PI;

const BG_TOP: [f32; 3] = [0.17, 0.18, 0.28];
const BG_BOTTOM: [f32; 3] = [0.09, 0.10, 0.17];
const TRACK: [f32; 3] = [0.30, 0.32, 0.42];
const GREEN: [f32; 3] = [0.31, 0.78, 0.47];
const ORANGE: [f32; 3] = [0.87, 0.47, 0.24];
const WHITE: [f32; 3] = [0.97, 0.97, 0.98];

/// Gauge geometry in unit coordinates (0..1, y down).
const CENTER: (f32, f32) = (0.5, 0.58);
const RADIUS: f32 = 0.30;
const RING: f32 = 0.085;
/// The arc spans 240 degrees, from lower left to lower right.
const START: f32 = 210.0;
const SPAN: f32 = 240.0;
/// How far the needle and the coloured part of the arc reach.
const LEVEL: f32 = 0.68;

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// Coverage of a shape with signed distance `d` (negative inside), with
/// one-pixel anti-aliasing.
fn cover(d: f32, px: f32) -> f32 {
    (0.5 - d / px).clamp(0.0, 1.0)
}

fn rounded_square(x: f32, y: f32, r: f32) -> f32 {
    let qx = (x - 0.5).abs() - (0.5 - r);
    let qy = (y - 0.5).abs() - (0.5 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn segment(x: f32, y: f32, a: (f32, f32), b: (f32, f32), w: f32) -> f32 {
    let (px, py) = (x - a.0, y - a.1);
    let (bx, by) = (b.0 - a.0, b.1 - a.1);
    let h = ((px * bx + py * by) / (bx * bx + by * by)).clamp(0.0, 1.0);
    ((px - bx * h).powi(2) + (py - by * h).powi(2)).sqrt() - w / 2.0
}

/// Angle of the gauge at `t` (0 = start, 1 = end), in radians, maths
/// orientation (counter-clockwise, y up).
fn gauge_angle(t: f32) -> f32 {
    (START - SPAN * t) * PI / 180.0
}

/// Position along the arc (0..1) of the direction (dx, dy), if on the arc.
fn arc_position(dx: f32, dy: f32) -> Option<f32> {
    let deg = (-dy).atan2(dx).to_degrees(); // y up
    // Measured clockwise from START.
    let along = (START - deg).rem_euclid(360.0);
    (along <= SPAN).then_some(along / SPAN)
}

/// Premultiplied-free RGBA of the logo at `size` x `size` pixels.
pub fn rgba(size: u32) -> Vec<u8> {
    let px = 1.0 / size as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    let tip_angle = gauge_angle(LEVEL);
    let needle_len = RADIUS - RING * 0.2;
    let tip = (
        CENTER.0 + tip_angle.cos() * needle_len,
        CENTER.1 - tip_angle.sin() * needle_len,
    );
    for row in 0..size {
        for col in 0..size {
            let x = (col as f32 + 0.5) * px;
            let y = (row as f32 + 0.5) * px;

            // Tile.
            let tile = cover(rounded_square(x, y, 0.22), px);
            let mut color = mix(BG_TOP, BG_BOTTOM, y);

            // Arc: a ring clipped to the gauge span, with round caps.
            let (dx, dy) = (x - CENTER.0, y - CENTER.1);
            let dist = (dx * dx + dy * dy).sqrt();
            let ring = (dist - RADIUS).abs() - RING / 2.0;
            let pos = arc_position(dx, dy);
            let cap = |t: f32| {
                let a = gauge_angle(t);
                let c = (CENTER.0 + a.cos() * RADIUS, CENTER.1 - a.sin() * RADIUS);
                ((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt() - RING / 2.0
            };
            let arc = match pos {
                Some(_) => ring,
                None => cap(0.0).min(cap(1.0)),
            };
            let arc_cover = cover(arc, px);
            if arc_cover > 0.0 {
                let t = pos.unwrap_or(if cap(0.0) < cap(1.0) { 0.0 } else { 1.0 });
                let lit = if t <= LEVEL {
                    mix(GREEN, ORANGE, t / LEVEL)
                } else {
                    TRACK
                };
                color = mix(color, lit, arc_cover);
            }

            // Needle and hub.
            let needle =
                segment(x, y, CENTER, tip, 0.055).min(((dx * dx + dy * dy).sqrt()) - 0.075);
            color = mix(color, WHITE, cover(needle, px));
            // A small dark dot in the hub.
            color = mix(color, BG_BOTTOM, cover(dist - 0.028, px));

            out.extend(color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8));
            out.push((tile * 255.0).round() as u8);
        }
    }
    out
}

/// A Windows `.ico` file holding the logo at the usual sizes (32-bit BMP).
/// Only the asset tests use it; the build embeds the committed file.
#[cfg(test)]
pub fn ico() -> Vec<u8> {
    const SIZES: [u32; 9] = [16, 20, 24, 32, 48, 64, 96, 128, 256];
    let images: Vec<Vec<u8>> = SIZES.iter().map(|&s| bmp(s)).collect();
    let mut out = Vec::new();
    out.extend(0u16.to_le_bytes()); // reserved
    out.extend(1u16.to_le_bytes()); // type: icon
    out.extend((SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * SIZES.len() as u32;
    for (s, img) in SIZES.iter().zip(&images) {
        let dim = if *s >= 256 { 0 } else { *s as u8 };
        out.extend([dim, dim, 0, 0]);
        out.extend(1u16.to_le_bytes()); // planes
        out.extend(32u16.to_le_bytes()); // bits per pixel
        out.extend((img.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend(img);
    }
    out
}

/// One icon image: BITMAPINFOHEADER, bottom-up BGRA pixels, AND mask.
#[cfg(test)]
fn bmp(size: u32) -> Vec<u8> {
    let rgba = rgba(size);
    let mask_row = size.div_ceil(32) * 4;
    let mut out = Vec::new();
    out.extend(40u32.to_le_bytes());
    out.extend((size as i32).to_le_bytes());
    out.extend((size as i32 * 2).to_le_bytes()); // colour + mask
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend([0u8; 24]); // no compression, sizes and palette unused
    for row in (0..size).rev() {
        for col in 0..size {
            let i = ((row * size + col) * 4) as usize;
            out.extend([rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    // Alpha carries the shape; the mask is all zeros.
    out.extend(std::iter::repeat_n(0u8, (mask_row * size) as usize));
    out
}

/// The logo as an SVG document (README, website).
#[cfg(test)]
pub fn svg() -> String {
    let point = |t: f32, r: f32| {
        let a = gauge_angle(t);
        (
            (CENTER.0 + a.cos() * r) * 256.0,
            (CENTER.1 - a.sin() * r) * 256.0,
        )
    };
    let r = RADIUS * 256.0;
    let (sx, sy) = point(0.0, RADIUS);
    let (lx, ly) = point(LEVEL, RADIUS);
    let (ex, ey) = point(1.0, RADIUS);
    let (tx, ty) = point(LEVEL, RADIUS - RING * 0.2);
    let (cx, cy) = (CENTER.0 * 256.0, CENTER.1 * 256.0);
    let hex = |c: [f32; 3]| {
        format!(
            "#{:02x}{:02x}{:02x}",
            (c[0] * 255.0).round() as u8,
            (c[1] * 255.0).round() as u8,
            (c[2] * 255.0).round() as u8
        )
    };
    let w = RING * 256.0;
    // Both parts are under 180 degrees and run clockwise on screen.
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{bg_top}"/>
      <stop offset="1" stop-color="{bg_bottom}"/>
    </linearGradient>
    <linearGradient id="lit" x1="0" y1="1" x2="1" y2="0">
      <stop offset="0" stop-color="{green}"/>
      <stop offset="1" stop-color="{orange}"/>
    </linearGradient>
  </defs>
  <rect width="256" height="256" rx="56" fill="url(#bg)"/>
  <path d="M {lx:.1} {ly:.1} A {r:.1} {r:.1} 0 0 1 {ex:.1} {ey:.1}" fill="none" stroke="{track}" stroke-width="{w:.1}" stroke-linecap="round"/>
  <path d="M {sx:.1} {sy:.1} A {r:.1} {r:.1} 0 0 1 {lx:.1} {ly:.1}" fill="none" stroke="url(#lit)" stroke-width="{w:.1}" stroke-linecap="round"/>
  <line x1="{cx:.1}" y1="{cy:.1}" x2="{tx:.1}" y2="{ty:.1}" stroke="{white}" stroke-width="14" stroke-linecap="round"/>
  <circle cx="{cx:.1}" cy="{cy:.1}" r="19" fill="{white}"/>
  <circle cx="{cx:.1}" cy="{cy:.1}" r="7" fill="{bg_bottom}"/>
</svg>
"##,
        bg_top = hex(BG_TOP),
        bg_bottom = hex(BG_BOTTOM),
        green = hex(GREEN),
        orange = hex(ORANGE),
        track = hex(TRACK),
        white = hex(WHITE),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(img: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    #[test]
    fn draws_a_tile_with_a_gauge_and_needle() {
        let s = 64;
        let img = rgba(s);
        assert_eq!(img.len(), (s * s * 4) as usize);
        // Corners are transparent, the centre is opaque.
        assert_eq!(pixel(&img, s, 0, 0)[3], 0);
        assert_eq!(pixel(&img, s, 32, 32)[3], 255);
        // The hub is white around a dark dot.
        let hub = pixel(&img, s, 32, (s as f32 * CENTER.1 + 3.0) as u32);
        assert!(hub[0] > 230 && hub[1] > 230, "{hub:?}");
        // The start of the arc (lower left) is green-ish.
        let a = gauge_angle(0.02);
        let (x, y) = (
            (CENTER.0 + a.cos() * RADIUS) * s as f32,
            (CENTER.1 - a.sin() * RADIUS) * s as f32,
        );
        let start = pixel(&img, s, x as u32, y as u32);
        assert!(start[1] > start[0] && start[1] > start[2], "{start:?}");
        assert_eq!(
            arc_position(0.0, 1.0),
            None,
            "straight down is outside the arc"
        );
    }

    #[test]
    fn ico_has_every_size() {
        let ico = ico();
        assert_eq!(&ico[0..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]), 9);
        // The 256 px entry is written as size 0.
        assert_eq!(ico[6 + 16 * 8], 0);
        let last_size =
            u32::from_le_bytes(ico[6 + 16 * 8 + 8..6 + 16 * 8 + 12].try_into().unwrap());
        assert_eq!(last_size, 40 + 256 * 256 * 4 + 32 * 256);
    }

    #[test]
    fn executable_embeds_the_product_icon() {
        let path = std::env::current_exe().unwrap();
        let icon = wtm_core::small_icon(&path.to_string_lossy()).expect("executable icon resource");
        assert!(
            icon.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 128
                && p[0] > 180
                && p[1] > 80
                && p[1] < 180
                && p[2] < 130),
            "product orange missing"
        );
        assert!(
            icon.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 128 && p[1] > 140 && p[0] < 180),
            "gauge green missing"
        );
    }

    /// The committed assets must match the code; regenerate them with
    /// `cargo test -p wtm-gui write_assets -- --ignored`.
    #[test]
    fn committed_assets_are_up_to_date() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
        assert!(
            std::fs::read(format!("{root}/rustwtm.ico")).unwrap() == ico(),
            "Regenerate the multi-size icon with the write_assets test"
        );
        assert_eq!(
            std::fs::read_to_string(format!("{root}/logo.svg")).unwrap(),
            svg()
        );
    }

    #[test]
    #[ignore = "regenerates the committed assets"]
    fn write_assets() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(format!("{root}/rustwtm.ico"), ico()).unwrap();
        std::fs::write(format!("{root}/logo.svg"), svg()).unwrap();
    }
}
