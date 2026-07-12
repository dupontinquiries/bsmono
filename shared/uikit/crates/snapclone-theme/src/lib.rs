//! snapclone-theme — color palettes and design tokens.
//!
//! Every UI backend (Qt, egui, Slint, web) reads from this crate so the
//! visual design is defined exactly once. Palettes are `const` structs —
//! zero runtime cost, fully usable in const contexts.
//!
//! # Available palettes
//!
//! | Name        | Character                                         |
//! |-------------|---------------------------------------------------|
//! | `dark`      | Warm sepia-dark (the original SnapClone look)     |
//! | `pitch_black` | OLED-pure black, maximum contrast               |
//! | `paper`     | Off-white notebook, ink typography                |
//! | `neon`      | Cyberpunk: deep navy + electric cyan/magenta      |
//! | `minimal`   | Near-white with slate accents, clean & corporate  |
//! | `modern`    | Slate-dark with vivid blue accent (Material-ish)  |
//!
//! # Usage
//!
//! ```rust
//! use snapclone_theme::{Palette, DARK, PAPER};
//!
//! fn apply(palette: &Palette) {
//!     println!("Background: {}", palette.bg_deep);
//!     println!("Accent: {}", palette.accent);
//! }
//! apply(&DARK);
//! apply(&PAPER);
//! ```

pub mod palette;
pub mod tokens;

pub use palette::{Palette, PaletteId};
pub use tokens::Spacing;

pub use palette::{DARK, PITCH_BLACK, PAPER, NEON, MINIMAL, MODERN, EARTH, PASTELS, METALLIC, NIGHT, SYSTEM, VOICE};

/// Parse one of this crate's `#RRGGBB` (or `#RRGGBBAA`) color strings to raw bytes.
///
/// Palettes store colors as CSS-style hex so every backend — egui, Qt, Slint, web, pygame — reads a
/// single source of truth; this is the shared parser so none of them reimplement it and drift. Each
/// backend maps the returned bytes into its own color type (`Color32`, `QColor`, an RGB tuple, …).
/// Malformed input yields magenta, the classic "unset color" marker, rather than panicking.
pub fn rgb(hex: &str) -> [u8; 3] {
    let h = hex.trim_start_matches('#');
    if h.len() < 6 {
        return [0xFF, 0x00, 0xFF];
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0xFF);
    [b(0), b(2), b(4)]
}

/// Like [`rgb`] but with alpha: `#RRGGBBAA` → `[r,g,b,a]` (opaque `#RRGGBB` → alpha 255).
pub fn rgba(hex: &str) -> [u8; 4] {
    let [r, g, b] = rgb(hex);
    let h = hex.trim_start_matches('#');
    let a = if h.len() >= 8 {
        u8::from_str_radix(&h[6..8], 16).unwrap_or(0xFF)
    } else {
        0xFF
    };
    [r, g, b, a]
}

/// Look up a palette by its id string (matches the settings page selector).
///
/// V32.34+ FIX: this match was missing `earth`, `pastels`, and `metallic`
/// — three palettes that exist as constants, are exported, and are offered
/// by the settings selector (PaletteId::all() lists all 9, from_str maps all
/// 9), but fell through to the `_ => DARK` default here. Selecting any of the
/// three silently applied Dark instead of the chosen theme. To stop this
/// class of drift recurring, the lookup now goes through the single source of
/// truth: parse the string to a PaletteId (one canonical mapping) then resolve
/// that id to its constant. Adding a new palette in one place (the enum +
/// from_str + the id→const arm) now can't leave this function stale.
pub fn palette_by_id(id: &str) -> &'static Palette {
    palette_for(PaletteId::from_str(id))
}

/// Resolve a PaletteId to its `&'static Palette` constant. Exhaustive over
/// the enum, so the compiler forces a new arm whenever a variant is added —
/// this is the guard that keeps palette_by_id complete.
pub fn palette_for(id: PaletteId) -> &'static Palette {
    match id {
        PaletteId::Dark       => &DARK,
        PaletteId::PitchBlack => &PITCH_BLACK,
        PaletteId::Paper      => &PAPER,
        PaletteId::Neon       => &NEON,
        PaletteId::Minimal    => &MINIMAL,
        PaletteId::Modern     => &MODERN,
        PaletteId::Earth      => &EARTH,
        PaletteId::Pastels    => &PASTELS,
        PaletteId::Metallic   => &METALLIC,
        PaletteId::Night      => &NIGHT,
        PaletteId::System     => &SYSTEM,
        PaletteId::Voice      => &VOICE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_parses_palette_hex() {
        assert_eq!(rgb("#1A1917"), [0x1A, 0x19, 0x17]);
        assert_eq!(rgb(DARK.accent), [0xC9, 0xB9, 0x9A]);
        assert_eq!(rgba("#11111088"), [0x11, 0x11, 0x10, 0x88]);
        assert_eq!(rgba(DARK.bg_base), [0x1A, 0x19, 0x17, 0xFF]);
        assert_eq!(rgb("bad"), [0xFF, 0x00, 0xFF]);
    }

    // V32.34+: these guard the palette-lookup bug where palette_by_id was
    // missing earth/pastels/metallic and silently returned Dark for them.

    #[test]
    fn every_palette_id_resolves_to_its_own_constant() {
        // palette_for(id).id MUST equal id for all variants. A missing or
        // mis-wired arm (the original bug) would return a palette whose id
        // doesn't match the one asked for.
        for &id in PaletteId::all() {
            assert_eq!(palette_for(id).id, id,
                       "palette_for({id:?}) returned a palette with a different id");
        }
    }

    #[test]
    fn string_lookup_matches_for_all_palettes() {
        // palette_by_id(id.as_str()) must resolve to the SAME palette as
        // palette_for(id). This is the path the settings selector takes.
        for &id in PaletteId::all() {
            let by_str = palette_by_id(id.as_str());
            assert_eq!(by_str.id, id,
                       "palette_by_id({:?}) fell through to {:?} — string arm missing",
                       id.as_str(), by_str.id);
        }
    }

    #[test]
    fn earth_pastels_metallic_do_not_fall_back_to_dark() {
        // The exact regression: these three were absent from palette_by_id's
        // match and silently became Dark.
        assert_eq!(palette_by_id("earth").id,    PaletteId::Earth);
        assert_eq!(palette_by_id("pastels").id,  PaletteId::Pastels);
        assert_eq!(palette_by_id("metallic").id, PaletteId::Metallic);
    }

    #[test]
    fn id_string_round_trips() {
        // from_str(as_str(id)) == id for every variant — the canonical
        // mapping must be a true round-trip.
        for &id in PaletteId::all() {
            assert_eq!(PaletteId::from_str(id.as_str()), id,
                       "{id:?} did not round-trip through as_str/from_str");
        }
    }

    #[test]
    fn unknown_id_defaults_to_dark() {
        // Intentional fallback for unknown/corrupt settings values.
        assert_eq!(palette_by_id("does_not_exist").id, PaletteId::Dark);
        assert_eq!(palette_by_id("").id, PaletteId::Dark);
    }

    #[test]
    fn all_lists_every_variant_once() {
        // all() must contain each variant exactly once. If a new palette is
        // added to the enum but not to all(), the selector won't offer it.
        let ids = PaletteId::all();
        assert_eq!(ids.len(), 12, "expected 12 palettes in all()");
        for &id in ids {
            let count = ids.iter().filter(|&&x| x == id).count();
            assert_eq!(count, 1, "{id:?} appears {count} times in all()");
        }
    }
}
