//! mono_tile — deterministic app tiles, the "identicon" style ported from gitmind's Qt `MonoTile.qml`:
//! 1–2 bold initials from a name over a hue derived from the name's hash. The same name always yields
//! the same tile, so an app is recognizable at a glance across the fleet without any per-app art.
//!
//! The seed/hue/initials are pure and unit-tested; only the paint needs egui.

use egui::{Align2, Color32, CornerRadius, FontId, Response, Sense, Ui, Vec2};

/// FNV-1a 32-bit hash of a name — the `_seed` from the QML.
pub fn seed(name: &str) -> u32 {
    let mut s: u32 = 2166136261;
    for b in name.bytes() {
        s ^= b as u32;
        s = s.wrapping_mul(16777619);
    }
    s
}

/// The tile hue in `[0,360)` for a name.
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

/// HSL → `Color32` (`h` in `[0,360)`, `s`/`l` in `[0,1]`), matching the QML `_rgb`.
pub fn hsl(h: f32, s: f32, l: f32) -> Color32 {
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
    Color32::from_rgb(to(r), to(g), to(b))
}

/// Paint a deterministic tile for `name` at `size` px (square, clickable). Returns its `Response`.
pub fn mono_tile(ui: &mut Ui, name: &str, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let h = hue(name);
    let painter = ui.painter();
    let radius = CornerRadius::same((size * 0.28).clamp(2.0, 24.0) as u8);
    // hue-tinted background, slightly brighter when hovered (the shared motion-less quick feedback)
    let lift = if resp.hovered() { 0.06 } else { 0.0 };
    painter.rect_filled(rect, radius, hsl(h, 0.62, 0.40 + lift));
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        initials(name),
        FontId::proportional(size * 0.46),
        hsl(h, 0.66, 0.86),
    );
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
        assert_eq!(initials("gitmind"), "G");
        assert_eq!(initials("   "), "?");
    }

    #[test]
    fn hsl_hits_expected_primaries() {
        assert_eq!(hsl(0.0, 1.0, 0.5), Color32::from_rgb(255, 0, 0)); // red
        assert_eq!(hsl(120.0, 1.0, 0.5), Color32::from_rgb(0, 255, 0)); // green
        assert_eq!(hsl(240.0, 1.0, 0.5), Color32::from_rgb(0, 0, 255)); // blue
    }
}
