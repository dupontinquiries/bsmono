//! panel_rows — what the physical UI panel actually shows.
//!
//! `scene3d::physical_ui_panel` draws a scene-styled board with content
//! cards on it, and its own comment admits the gap: *"wiring live node text
//! onto the surface is future work"*. Until now the cards were four
//! rectangles at fixed widths (0.74, 0.65, 0.56, 0.47 of the board),
//! identical in every world and unrelated to anything the app holds. A
//! board showing four invented stripes is set dressing; the reference
//! renders show a CONTEXT panel listing real files with real sizes.
//!
//! This is the model between the two. It cannot draw glyphs — the scene is
//! meshes, and text on a mesh needs an atlas this renderer does not have —
//! but a row's WIDTH, its emphasis and the COUNT of them can all come from
//! the real list. That is the difference between a panel that looks like it
//! is showing your context and one that is.
//!
//! Width tracks label length because that is what a reader's eye uses at
//! this distance: five rows of near-equal length read as a directory
//! listing, one long row over three short ones reads as a title over
//! entries. Getting that shape right is most of what the panel communicates
//! before anyone walks close enough to read it.

/// One card on the board.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    /// Fraction of the board's usable width, 0..1.
    pub width01: f32,
    /// Drawn brighter: the selected entry, or the one being read.
    pub accent: bool,
}

/// Cards that fit before the board is a wall of stripes. Four was the old
/// hardcoded count and it is still the right number — past this the rows
/// are thinner than the frame's bevel and stop reading as separate cards.
pub const MAX_ROWS: usize = 5;

/// Narrowest a card may be drawn. Below this it reads as dirt on the board
/// rather than an entry, so a one-character label still gets a card.
pub const MIN_WIDTH01: f32 = 0.22;
/// Widest, leaving the frame's margin visible.
pub const MAX_WIDTH01: f32 = 0.82;

/// Rows for a real list: `(label, accent)`.
///
/// Widths are scaled against the LONGEST label present rather than an
/// absolute character count, so a panel of short names still fills the
/// board and a panel of long ones does not saturate — the shape stays
/// informative at any naming convention.
pub fn from_labels(items: &[(String, bool)]) -> Vec<Row> {
    let taken: Vec<&(String, bool)> = items.iter().take(MAX_ROWS).collect();
    let longest = taken
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0)
        .max(1) as f32;
    taken
        .iter()
        .map(|(label, accent)| {
            let frac = label.chars().count() as f32 / longest;
            Row {
                width01: (MIN_WIDTH01 + frac * (MAX_WIDTH01 - MIN_WIDTH01))
                    .clamp(MIN_WIDTH01, MAX_WIDTH01),
                accent: *accent,
            }
        })
        .collect()
}

/// The board with nothing on it.
///
/// ONE dim narrow card, not four. An empty context showing four full-width
/// stripes is the board lying about having content — the same fault as
/// snapclone's camera page claiming a service it did not have. One short
/// card reads as "a board, empty", which is true and is a real state.
pub fn empty() -> Vec<Row> {
    vec![Row { width01: MIN_WIDTH01, accent: false }]
}

/// What `build` passes while the live list is not threaded through it.
///
/// Deliberately NOT the old descending-width decoration: this is the shape
/// of a plausible context listing (a title, then entries), so the seam is
/// obvious and a reviewer can see at a glance whether a scene is showing
/// real data or this.
pub fn placeholder() -> Vec<Row> {
    vec![
        Row { width01: MAX_WIDTH01, accent: true },
        Row { width01: 0.58, accent: false },
        Row { width01: 0.63, accent: false },
        Row { width01: 0.49, accent: false },
    ]
}


/// How brightly a row's card catches the room's light.
///
/// The board's `emissive` was one constant per material, so a card under a
/// canopy shaft was lit exactly like one in the shade and the panel read as
/// a sticker on the world rather than an object in it. `scene-kit`'s
/// `modulate_dot` already answers "how bright is this spot, in this scene's
/// physical process" — it drives the graph's dots — and the board is in the
/// same room, so it should be asking the same question.
///
/// Sampled at `t` and BAKED: `physical_ui_panel` builds static meshes, so
/// this gives spatial variation across the board, not animation. Making it
/// breathe needs the renderer to re-evaluate emissive per frame, which is a
/// different change; the mechanism is per-scene either way.
///
/// The returned multiplier keeps `modulate_dot`'s legibility floor — these
/// make the panel feel lit, never unreadable.
pub fn row_glow(light: scene_kit::mesh3d::SceneLight, index: usize, of: usize, seed: u32, t: f32) -> f32 {
    // GRID CELLS, not a 0..1 fraction. The mechanisms `floor()` their
    // coordinates — `dappled_canopy` hashes `(gx * 1.3).floor()` — so
    // normalised inputs all land in cell zero and every row comes back
    // identical. My first version did exactly that and the test caught it:
    // five rows, one value, 0.827 each.
    //
    // Rows step a whole cell down the board, and stride across it too, so a
    // mechanism with horizontal structure (BrickChase, RakingLight) varies
    // between rows rather than hitting them all the same.
    let _ = of;
    let gy = index as f32;
    let gx = index as f32 * 1.7;
    scene_kit::mesh3d::modulate_dot(light, gx, gy, seed, t).0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(v: &[(&str, bool)]) -> Vec<(String, bool)> {
        v.iter().map(|(s, a)| ((*s).to_string(), *a)).collect()
    }

    #[test]
    fn a_lit_room_varies_the_cards_and_never_puts_one_out() {
        use scene_kit::mesh3d::{scene_light_for, SceneLight};
        // A mechanism with real structure lights the rows differently...
        let dappled = scene_light_for("Canopy");
        assert_ne!(dappled, SceneLight::Uniform, "Canopy has a mechanism");
        let g: Vec<f32> = (0..5).map(|i| row_glow(dappled, i, 5, 7, 0.0)).collect();
        assert!(g.windows(2).any(|w| (w[0] - w[1]).abs() > 1e-3), "{g:?}");
        // ...but never darkens one past reading, in any scene, anywhere on
        // the board. This is the property the panel depends on.
        for name in scene_kit::mesh3d::SCENE_NAMES {
            let l = scene_light_for(name);
            for i in 0..MAX_ROWS {
                let v = row_glow(l, i, MAX_ROWS, 3, 0.0);
                assert!(v > 0.0 && v.is_finite(), "{name} row {i} = {v}");
            }
        }
        // Deterministic: the same board builds the same way twice.
        assert_eq!(row_glow(dappled, 2, 5, 7, 0.0), row_glow(dappled, 2, 5, 7, 0.0));
    }

    #[test]
    fn a_longer_label_gets_a_wider_card_and_the_longest_fills_the_board() {
        let r = from_labels(&items(&[("a", false), ("much longer name", false)]));
        assert!(r[0].width01 < r[1].width01);
        assert!((r[1].width01 - MAX_WIDTH01).abs() < 1e-6, "the longest saturates");
        assert!(r[0].width01 >= MIN_WIDTH01, "a one-char label still gets a card");
    }

    #[test]
    fn the_shape_is_relative_so_short_names_still_fill_the_board() {
        // The point of scaling against the longest present rather than an
        // absolute width: two panels of different naming conventions should
        // both read as full listings, not one as a stub.
        let short = from_labels(&items(&[("a.rs", false), ("b.rs", false), ("cc.rs", false)]));
        let long = from_labels(&items(&[
            ("eastern_grove_map.jpg", false),
            ("soil_analysis.pdf", false),
            ("sensor_readings.csv", false),
        ]));
        for set in [&short, &long] {
            assert!(set.iter().any(|r| (r.width01 - MAX_WIDTH01).abs() < 1e-6));
            assert!(set.iter().all(|r| (MIN_WIDTH01..=MAX_WIDTH01).contains(&r.width01)));
        }
    }

    #[test]
    fn the_board_caps_its_rows_and_keeps_the_accent() {
        let many: Vec<(String, bool)> = (0..20)
            .map(|i| (format!("entry-{i}"), i == 2))
            .collect();
        let r = from_labels(&many);
        assert_eq!(r.len(), MAX_ROWS, "past this they stop reading as cards");
        assert!(r[2].accent, "the selected entry is still marked");
        assert_eq!(r.iter().filter(|x| x.accent).count(), 1);
        // An accent past the cap is simply not shown — no panic, no wrap.
        let late: Vec<(String, bool)> = (0..20).map(|i| (format!("e{i}"), i == 19)).collect();
        assert!(from_labels(&late).iter().all(|x| !x.accent));
    }

    #[test]
    fn an_empty_context_shows_an_empty_board_not_four_invented_stripes() {
        assert!(from_labels(&[]).is_empty());
        let e = empty();
        assert_eq!(e.len(), 1);
        assert!(!e[0].accent);
        assert_eq!(e[0].width01, MIN_WIDTH01);
        // And the placeholder is distinguishable from real data at a glance:
        // it leads with a full-width accented row, which a directory listing
        // scaled from labels only produces if its first entry is the longest.
        let p = placeholder();
        assert!(p[0].accent && (p[0].width01 - MAX_WIDTH01).abs() < 1e-6);
        assert!(p.len() <= MAX_ROWS);
    }
}
