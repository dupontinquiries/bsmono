//! mono_tile — deterministic app tiles, ported faithfully from gitmind's Qt `MonoTile.qml`.
//!
//! Not a flat letter-on-a-swatch: the QML draws the initials as a seed-driven **layered echo** — two
//! hue-shifted "ghost" copies behind the bright top copy, each offset and rotated by a deterministic
//! PRNG (mulberry32) over a dark ground. That's the "dithered double-fade angular" look: same name →
//! same tile, but each has its own little angular drift.
//!
//! seed/hue/initials/hsl and the PRNG are pure and unit-tested; only the paint needs egui.

use egui::epaint::TextShape;
use egui::{Color32, CornerRadius, FontId, Pos2, Response, Sense, Ui, Vec2};

/// FNV-1a 32-bit hash of a name — the `_seed` from the QML.
pub fn seed(name: &str) -> u32 {
    let mut s: u32 = 2166136261;
    for b in name.bytes() {
        s ^= b as u32;
        s = s.wrapping_mul(16777619);
    }
    s
}

/// The base tile hue in `[0,360)` for a name.
pub fn hue(name: &str) -> f32 {
    (seed(name) % 360) as f32
}

/// 1–2 uppercase initials for a crate/app name (splits on `-`/`_`), matching the QML `_mono`.
pub fn initials(name: &str) -> String {
    let parts: Vec<&str> = name.split(['-', '_']).filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        let a = parts[0].chars().next().unwrap_or('?');
        let b = parts[1].chars().next().unwrap_or('?');
        format!("{a}{b}").to_uppercase()
    } else if let Some(c) = name.chars().find(|c| c.is_alphanumeric()) {
        c.to_uppercase().to_string()
    } else {
        "?".to_string()
    }
}

/// HSL → `Color32` (`h` in `[0,360)`, `s`/`l` in `[0,1]`), matching the QML `_rgb`. `alpha` in `[0,1]`.
pub fn hsla(h: f32, s: f32, l: f32, alpha: f32) -> Color32 {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to = |v: f32| (((v + m) * 255.0).round().clamp(0.0, 255.0)) as u8;
    Color32::from_rgba_unmultiplied(to(r), to(g), to(b), (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

/// mulberry32 — the QML `_rng`. Deterministic per seed; each call returns a float in `[0,1)`.
#[derive(Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng(seed)
    }
    pub fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x6D2B_79F5);
        let a = self.0;
        let mut t = (a ^ (a >> 15)).wrapping_mul(1 | a);
        t = (t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t))) ^ t;
        ((t ^ (t >> 14)) as f32) / 4_294_967_296.0
    }
    /// Centered jitter in `[-0.5, 0.5)`.
    fn signed(&mut self) -> f32 {
        self.next_f32() - 0.5
    }
}

/// Paint a deterministic tile for `name` at `size` px (square, clickable). Returns its `Response`.
pub fn mono_tile(ui: &mut Ui, name: &str, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let h = hue(name);
    let m = initials(name);
    let one = m.chars().count() == 1;
    let fs = if one { size * 0.56 } else { size * 0.40 };
    let center = rect.center();
    let mut rng = Rng::new(seed(name));

    let painter = ui.painter().clone();
    // dark ground (a touch of hover lift), rounded for the fleet list
    let ground = if resp.hovered() {
        Color32::from_rgb(0x14, 0x18, 0x24)
    } else {
        Color32::from_rgb(0x0c, 0x0f, 0x18)
    };
    painter.rect_filled(rect, CornerRadius::same((size * 0.24).clamp(2.0, 20.0) as u8), ground);

    // draw one rotated, centered copy of the initials
    let mut layer = |center: Pos2, angle: f32, color: Color32| {
        let galley = painter.layout_no_wrap(m.clone(), FontId::proportional(fs), color);
        let half = galley.size() * 0.5;
        // position so that rotating the galley about its top-left keeps the glyph centred on `center`
        let (sin, cos) = angle.sin_cos();
        let rot_half = Vec2::new(half.x * cos - half.y * sin, half.x * sin + half.y * cos);
        let pos = center - rot_half;
        let mut shape = TextShape::new(pos, galley, color);
        shape.angle = angle;
        painter.add(shape);
    };

    // two hue-shifted ghost layers (l = 2 then 1), offset + rotated, at ~0.32 alpha — the double fade
    for l in (1..=2).rev() {
        let lf = l as f32;
        let jx = rng.signed() * size * 0.087 * lf;
        let jy = rng.signed() * size * 0.087 * lf;
        let angle = rng.signed() * 0.2; // the angular drift
        let color = hsla(h + lf * 44.0, 0.62, 0.36 + lf * 0.08, 0.32);
        layer(center + Vec2::new(jx, jy), angle, color);
    }
    // bright top layer, barely rotated
    let top_angle = rng.signed() * 0.05;
    layer(center, top_angle, hsla(h, 0.66, 0.70, 1.0));

    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_and_hue_are_deterministic() {
        assert_eq!(seed("voiceterm"), seed("voiceterm"));
        assert_ne!(seed("voiceterm"), seed("rusty-board"));
        assert!(hue("gitmind") < 360.0);
    }

    #[test]
    fn initials_split_on_dash_underscore_and_fall_back() {
        assert_eq!(initials("rusty-board"), "RB");
        assert_eq!(initials("av1_tuner"), "AT");
        assert_eq!(initials("voiceterm"), "V");
        assert_eq!(initials("   "), "?");
    }

    #[test]
    fn hsl_hits_expected_primaries_opaque() {
        assert_eq!(hsla(0.0, 1.0, 0.5, 1.0), Color32::from_rgb(255, 0, 0));
        assert_eq!(hsla(120.0, 1.0, 0.5, 1.0), Color32::from_rgb(0, 255, 0));
        assert_eq!(hsla(240.0, 1.0, 0.5, 1.0), Color32::from_rgb(0, 0, 255));
    }

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = Rng::new(seed("gitmind"));
        let mut b = Rng::new(seed("gitmind"));
        for _ in 0..8 {
            let v = a.next_f32();
            assert_eq!(v, b.next_f32()); // same seed → same stream
            assert!((0.0..1.0).contains(&v));
        }
        // different seeds diverge
        let mut c = Rng::new(seed("voiceterm"));
        assert_ne!(Rng::new(seed("gitmind")).next_f32(), c.next_f32());
    }
}
