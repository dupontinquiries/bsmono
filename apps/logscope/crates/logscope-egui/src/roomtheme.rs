//! roomtheme — the chrome comes from the room.
//!
//! ## The incoherence this removes
//!
//! There were two independent colour systems. `theme::Palette` drives every pixel of egui chrome —
//! panels, ink, accents — and a handful of palettes have a matching bespoke 3D scene. Separately,
//! `scene3d::ROOMS` holds sixteen procedural rooms, each with its own floor, perimeter, roof and
//! accent colours, reachable only through `LOGSCOPE_ROOM=<n>` and bound to no palette at all.
//!
//! So standing in the Observatory — a blue-stone room lit `#88c6ff` — left the interface in
//! whatever unrelated palette happened to be active, commonly Folding@home's neutral greys. The
//! reference images have one selector labelled with the space's name and everything downstream of
//! it: the Atlantis frames are brass on teal, the Observatory frames are warm grey on parchment,
//! and neither could be produced by a chrome palette that does not know where it is.
//!
//! That is the whole of "cohesive" here. Not more ornament — one source of truth, so the panel
//! edges belong to the same room as the floor behind them.
//!
//! ## Derivation, not authorship
//!
//! Sixteen hand-written palettes would drift from the rooms the first time a room's floor changed.
//! These are COMPUTED from the room spec: the background comes from the floor, the panels from the
//! perimeter, the accent from the room's own accent, and ink is placed for contrast against the
//! result. A room recoloured tomorrow re-themes its interface with it.
//!
//! ## Legibility is audited, not eyeballed
//!
//! A derivation can produce an unreadable palette — dark ink on a dark floor — and nobody notices
//! until they are looking at it. So there is a scalar: WCAG relative-luminance contrast ratio
//! between ink and the surface it sits on, computed for every room, with a floor that the test
//! enforces and a report that names the worst offenders with their numbers. That is the same shape
//! as a value-separation audit over a table of map styles: make the judged property a number,
//! apply it to the whole population, and let the threshold name what fails.

use crate::theme::{Material, Palette};
use egui::Color32;

/// WCAG relative luminance. The sRGB gamma expansion matters: a naive `(r+g+b)/3` rates mid-blue
/// and mid-yellow as equally bright, and the eye does not.
pub fn luminance(c: Color32) -> f32 {
    let f = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

/// WCAG contrast ratio, 1.0 (identical) to 21.0 (black on white).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

/// The floor every derived palette must clear for body text against its panel.
///
/// 4.5 is WCAG AA for normal text. This interface renders small monospace over textured, lit
/// surfaces, so it is held to AA rather than the 3.0 large-text allowance — and the audit below
/// enforces it for all sixteen rooms rather than for the ones anyone happened to open.
pub const MIN_TEXT_CONTRAST: f32 = 4.5;

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Push `c` toward white or black until it clears `target` contrast against `against`.
///
/// Iterative rather than closed-form because the gamma curve makes the closed form ugly and this
/// runs once per theme change, not per frame. Gives up at the extreme rather than looping: if white
/// on this surface still fails, no colour succeeds, and the audit should be told the real number
/// instead of being handed a lie.
pub(crate) fn force_contrast(c: Color32, against: Color32, target: f32) -> Color32 {
    if contrast(c, against) >= target { return c; }
    let toward = if luminance(against) > 0.18 { Color32::BLACK } else { Color32::WHITE };
    let mut best = c;
    for i in 1..=32 {
        let cand = mix(c, toward, i as f32 / 32.0);
        best = cand;
        if contrast(cand, against) >= target { return cand; }
    }
    best
}

/// One room's colours, as `scene3d::ROOMS` stores them.
#[derive(Clone, Copy)]
pub struct RoomColors {
    pub name: &'static str,
    pub floor: [u8; 3],
    pub perimeter: [u8; 3],
    pub accent: [u8; 3],
    /// The room's floor material, so the chrome's grain matches the ground.
    pub material: Material,
}

/// Derive a full chrome palette from the room's own colours.
///
/// The mapping is deliberately literal — background IS the floor, panels ARE the perimeter — so
/// that looking at the interface tells you where you are standing. Everything else is placed
/// relative to those two.
pub fn palette_for(room: RoomColors) -> Palette {
    let rgb = |v: [u8; 3]| Color32::from_rgb(v[0], v[1], v[2]);
    let floor = rgb(room.floor);
    let perim = rgb(room.perimeter);
    let accent = rgb(room.accent);
    let dark = luminance(floor) < 0.18;

    // Background sits slightly deeper than the floor so the floor still reads as the lit surface.
    let bg = mix(floor, if dark { Color32::BLACK } else { Color32::WHITE }, 0.25);
    // Panels are the perimeter, pulled toward the background so they belong to the room rather than
    // floating on it.
    // PANELS MUST SEPARATE FROM THE BACKGROUND BY CONSTRUCTION. In a room whose perimeter is close
    // to its floor — Cathedral is pale stone on paler stone — mixing them produces a panel the eye
    // cannot find, and the whole layout dissolves into one field. The audit caught exactly that at
    // 1.050 against a 1.06 floor; this is the derivation fixed rather than the threshold lowered,
    // which is the failure this kind of audit exists to prevent.
    //
    // Targeted at 1.10 rather than the 1.06 the test enforces, so a small future change to a room's
    // colours does not immediately land back on the line.
    let panel = force_contrast(mix(perim, bg, 0.55), bg, 1.10);
    let panel2 = mix(panel, bg, 0.35);
    let window = mix(bg, panel, 0.35);
    let line = mix(perim, accent, 0.25);

    // Ink is placed FOR CONTRAST against the panel it will sit on, not chosen. That is what keeps
    // the derivation legible across sixteen rooms without sixteen judgements.
    let ink_seed = if dark { Color32::from_rgb(0xdc, 0xe4, 0xee) } else { Color32::from_rgb(0x24, 0x22, 0x1e) };
    let ink = force_contrast(ink_seed, panel, MIN_TEXT_CONTRAST);
    // Dim and faint are the same hue stepped toward the panel, but dim still has to be readable —
    // it carries subtitles and timestamps, which are text.
    let dim = force_contrast(mix(ink, panel, 0.42), panel, 3.0);
    let faint = mix(ink, panel, 0.66);

    // The accent must be visible ON the panel; a room whose accent is close to its perimeter would
    // otherwise produce invisible highlights.
    let amber = force_contrast(accent, panel, 3.0);
    let amber_soft = mix(amber, if dark { Color32::WHITE } else { Color32::BLACK }, 0.3);
    let amber_ink = if luminance(amber) > 0.4 { Color32::from_rgb(0x1a, 0x14, 0x0a) } else { Color32::from_rgb(0xf4, 0xee, 0xe4) };

    Palette {
        name: room.name,
        dark,
        material: room.material,
        lit: true,
        canopy: false,
        light: mix(accent, Color32::WHITE, 0.45),
        bg, window, panel, panel2, line,
        ink, dim, faint,
        track_bg: mix(bg, Color32::BLACK, 0.2),
        amber, amber_soft, amber_ink,
        // Status colours keep their meaning (green = ok, red = bad) but are tinted toward the room
        // so they read as part of it, then forced legible against the panel.
        blue: force_contrast(mix(Color32::from_rgb(0x6f, 0xb0, 0xf0), accent, 0.3), panel, 3.0),
        cyan: force_contrast(mix(Color32::from_rgb(0x57, 0xd6, 0xc9), accent, 0.3), panel, 3.0),
        violet: force_contrast(mix(Color32::from_rgb(0x9a, 0x8c, 0xff), accent, 0.25), panel, 3.0),
        green: force_contrast(mix(Color32::from_rgb(0x8f, 0xd6, 0x94), accent, 0.2), panel, 3.0),
        yellow: force_contrast(mix(Color32::from_rgb(0xf5, 0xc2, 0x11), accent, 0.2), panel, 3.0),
        red: force_contrast(Color32::from_rgb(0xff, 0x63, 0x63), panel, 3.0),
        red_soft: mix(Color32::from_rgb(0xff, 0x8a, 0x8a), panel, 0.25),
        maxmark: mix(Color32::from_rgb(0xb4, 0x46, 0x46), accent, 0.2),
        whisker: if dark { Color32::WHITE } else { Color32::from_rgb(0x30, 0x2c, 0x28) },
    }
}

/// One room's legibility, as numbers. The report a threshold failure prints.
pub struct RoomAudit {
    pub name: &'static str,
    pub ink_on_panel: f32,
    pub dim_on_panel: f32,
    pub accent_on_panel: f32,
    pub panel_on_bg: f32,
}

pub fn audit(room: RoomColors) -> RoomAudit {
    let p = palette_for(room);
    RoomAudit {
        name: room.name,
        ink_on_panel: contrast(p.ink, p.panel),
        dim_on_panel: contrast(p.dim, p.panel),
        accent_on_panel: contrast(p.amber, p.panel),
        // Panels must be separable from the background or the layout dissolves into one field.
        panel_on_bg: contrast(p.panel, p.bg),
    }
}

/// Every space the picker offers: the hand-authored palettes, then the procedural rooms.
///
/// One list, because the reference interface has one selector. A room and a palette are the same
/// kind of thing from the operator's side — a place to be — and splitting them across a picker and
/// an environment variable is why the rooms went unused.
pub fn spaces() -> Vec<Palette> {
    let mut v: Vec<Palette> = crate::theme::PALETTES.to_vec();
    for i in 0..crate::scene3d::ROOM_COUNT {
        let p = palette_for(crate::scene3d::room_colors(i));
        // A room whose name collides with an authored palette would give the picker two entries
        // that look identical and behave differently. The authored one wins; the room is dropped.
        if !v.iter().any(|q| q.name == p.name) { v.push(p); }
    }
    v
}

/// Resolve a space by name, falling back to the first authored palette.
pub fn by_name(name: &str) -> Palette {
    spaces().into_iter().find(|p| p.name.eq_ignore_ascii_case(name))
        .unwrap_or(crate::theme::PALETTES[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene3d;

    fn all_rooms() -> Vec<RoomColors> {
        (0..scene3d::ROOM_COUNT).map(scene3d::room_colors).collect()
    }

    #[test]
    fn luminance_and_contrast_behave() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.01);
        // gamma-aware, not a channel average: pure green is far brighter than pure blue
        assert!(luminance(Color32::from_rgb(0, 255, 0)) > 8.0 * luminance(Color32::from_rgb(0, 0, 255)));
    }

    /// THE POPULATION AUDIT. Every room, not the ones anyone happened to open — a derivation that
    /// is legible for twelve rooms and unreadable for four is a derivation that will be found by a
    /// user rather than by a test.
    #[test]
    fn every_room_derives_a_legible_palette() {
        let mut bad = Vec::new();
        for r in all_rooms() {
            let a = audit(r);
            if a.ink_on_panel < MIN_TEXT_CONTRAST { bad.push(format!("{}: ink {:.2}", a.name, a.ink_on_panel)); }
            if a.dim_on_panel < 3.0 { bad.push(format!("{}: dim {:.2}", a.name, a.dim_on_panel)); }
            if a.accent_on_panel < 3.0 { bad.push(format!("{}: accent {:.2}", a.name, a.accent_on_panel)); }
            if a.panel_on_bg < 1.06 { bad.push(format!("{}: panel/bg {:.3}", a.name, a.panel_on_bg)); }
        }
        assert!(bad.is_empty(), "{} room(s) below the legibility floor:\n  {}", bad.len(), bad.join("\n  "));
    }

    /// The report, printed rather than asserted: what the numbers actually are, so a change that
    /// merely scrapes past the floor is visible.
    #[test]
    fn room_contrast_report() {
        let mut rows: Vec<RoomAudit> = all_rooms().into_iter().map(audit).collect();
        rows.sort_by(|a, b| a.ink_on_panel.partial_cmp(&b.ink_on_panel).unwrap());
        println!("{:<14} {:>8} {:>8} {:>8} {:>9}", "room", "ink", "dim", "accent", "panel/bg");
        for r in &rows {
            println!("{:<14} {:>8.2} {:>8.2} {:>8.2} {:>9.3}",
                     r.name, r.ink_on_panel, r.dim_on_panel, r.accent_on_panel, r.panel_on_bg);
        }
        assert!(rows.len() == scene3d::ROOM_COUNT);
    }

    /// The point of deriving rather than authoring: the chrome tracks the room. A dark room gets a
    /// dark interface and a pale one gets a pale interface, with no table to keep in sync.
    #[test]
    fn the_chrome_follows_the_room_it_came_from() {
        let dark = palette_for(RoomColors { name: "Observatory", floor: [0x1a, 0x1e, 0x2c],
            perimeter: [0x3a, 0x40, 0x52], accent: [0x88, 0xc6, 0xff], material: Material::Stone });
        let pale = palette_for(RoomColors { name: "Zen Garden", floor: [0xcf, 0xc6, 0xae],
            perimeter: [0x9a, 0x90, 0x7a], accent: [0x86, 0xb0, 0x86], material: Material::Paper });
        assert!(dark.dark && !pale.dark, "light/dark must follow the floor, not a hand-set flag");
        assert!(luminance(dark.bg) < luminance(pale.bg));
        assert!(luminance(dark.ink) > luminance(dark.bg), "dark room: light ink");
        assert!(luminance(pale.ink) < luminance(pale.bg), "pale room: dark ink");
        assert_eq!(dark.name, "Observatory", "the interface is labelled with where you are standing");
    }

    /// THE HAZARD THIS PAIRING CREATES. Putting rooms in the picker means a room name reaches the
    /// scene dispatch, and that dispatch ends in `_ => islands()`. A room that re-themed the chrome
    /// correctly and then built the generic fallback would be worse than the incoherence it
    /// replaced: the interface would confidently describe a space you are not standing in.
    #[test]
    fn every_room_in_the_picker_resolves_to_its_own_room() {
        for i in 0..scene3d::ROOM_COUNT {
            let name = scene3d::room_name(i);
            assert_eq!(scene3d::room_index(name), Some(i),
                       "'{name}' is offered in the picker but the scene dispatch cannot find it");
        }
        // and a name that is neither a palette nor a room still resolves to something
        assert_eq!(scene3d::room_index("Definitely Not A Room"), None);
    }

    /// One selector means one namespace. Two entries with the same label that behave differently is
    /// the specific way a merged list goes wrong.
    #[test]
    fn the_space_list_has_no_duplicate_names() {
        let all = spaces();
        let mut names: Vec<&str> = all.iter().map(|p| p.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate space names in the picker");
        assert!(all.len() >= crate::theme::PALETTES.len() + 10,
                "the rooms did not reach the list: {} entries", all.len());
    }

    #[test]
    fn a_space_resolves_by_name_case_insensitively() {
        assert_eq!(by_name("Observatory").name, "Observatory");
        assert_eq!(by_name("observatory").name, "Observatory");
        assert_eq!(by_name("Atlantis").name, "Atlantis");
        // an unknown name falls back rather than panicking — this path is fed by an env var
        assert_eq!(by_name("nonsense").name, crate::theme::PALETTES[0].name);
    }

    /// A room whose accent is nearly its own perimeter is the case that would silently produce
    /// invisible highlights. It has to be lifted, not passed through.
    #[test]
    fn an_accent_too_close_to_its_room_is_lifted_off_the_panel() {
        let camouflaged = RoomColors { name: "Flat", floor: [0x40, 0x40, 0x40],
            perimeter: [0x44, 0x44, 0x44], accent: [0x45, 0x45, 0x45], material: Material::Stone };
        let p = palette_for(camouflaged);
        assert!(contrast(p.amber, p.panel) >= 3.0,
                "an accent indistinguishable from the panel is an invisible highlight: {:.2}",
                contrast(p.amber, p.panel));
    }
}
