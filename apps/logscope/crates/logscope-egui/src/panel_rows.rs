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

/// Rows from the conversation's own segments — `(start, end)` ranges, as
/// `convograph::segments()` returns them.
///
/// Width tracks a segment's LENGTH against the longest, which is the same
/// relative rule `from_labels` uses and is real data rather than a stand-in:
/// a long stretch of talk gets a wide card. `active` marks the segment the
/// view is in.
///
/// This exists because the scene builder has spans in scope and never had
/// labels — the panel does not need text to be honest about shape.
pub fn from_spans(spans: &[(usize, usize)], active: Option<usize>) -> Vec<Row> {
    // THE LAST FIVE, not the first. `segments()` returns topic runs in order and the final
    // one always ends at the newest node, so taking from the front meant that on any
    // conversation longer than five topics the board showed the OPENING of the chat and
    // never the part being talked about — a museum of the beginning, bolted to the wall.
    //
    // `active` indexes the FULL span list, and is translated below, so the caller says
    // "segment k" without knowing which window survived.
    let skipped = spans.len().saturating_sub(MAX_ROWS);
    let taken: Vec<&(usize, usize)> = spans.iter().skip(skipped).collect();
    let longest = taken
        .iter()
        .map(|(a, b)| b.saturating_sub(*a).max(1))
        .max()
        .unwrap_or(1) as f32;
    taken
        .iter()
        .enumerate()
        .map(|(i, (a, b))| {
            let frac = b.saturating_sub(*a).max(1) as f32 / longest;
            Row {
                width01: (MIN_WIDTH01 + frac * (MAX_WIDTH01 - MIN_WIDTH01))
                    .clamp(MIN_WIDTH01, MAX_WIDTH01),
                accent: active == Some(i + skipped),
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


/// The live rows, in a form the scene cache can key on.
///
/// A fixed-size `Copy` struct rather than a `Vec` because it follows
/// `EnvParams` exactly: set every frame into a thread-local `Cell`, read by
/// `scene3d::build`, and bucketed by [`PanelContext::quantized`] for the
/// cache key. The baked scene is expensive to rebuild and the context list
/// churns — an exact-equality key on raw widths would rebuild the whole
/// world every time a filename changed by a character.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelContext {
    rows: [Row; MAX_ROWS],
    len: u8,
    /// Which tab plate along the panel's top edge is lit, 0-based.
    ///
    /// The renderer hardcoded `active = i == 0`, so the in-scene board always showed the
    /// FIRST tab lit however far the app had moved — a physical panel that is meant to BE
    /// the app's context panel as hardware, telling you it is on Files while you are on
    /// Prompting. A board that reports the wrong state is worse than one with no tabs: the
    /// tabs are the part that says the board is live rather than scenery.
    active_tab: u8,
}

impl Default for PanelContext {
    /// The stand-in, so a scene built before anyone sets a context still
    /// has a plausible board rather than an empty one.
    fn default() -> Self {
        PanelContext::from_rows(&placeholder())
    }
}

impl PanelContext {
    pub fn from_rows(rows: &[Row]) -> PanelContext {
        let mut out = [Row { width01: 0.0, accent: false }; MAX_ROWS];
        let n = rows.len().min(MAX_ROWS);
        out[..n].copy_from_slice(&rows[..n]);
        PanelContext { rows: out, len: n as u8, active_tab: 0 }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows[..self.len as usize]
    }

    /// Light the tab at `i`. Clamped by the caller's own tab count at read time, so a
    /// context set by a page this build does not have cannot light a plate that is not
    /// drawn.
    pub fn with_active_tab(mut self, i: usize) -> PanelContext {
        self.active_tab = i.min(u8::MAX as usize) as u8;
        self
    }

    /// Which plate is lit, given how many the skin draws. Out of range lights the FIRST,
    /// which is the old behaviour and the safe one: a panel with no plate lit reads as
    /// switched off.
    pub fn active_tab(&self, tabs: usize) -> usize {
        let t = self.active_tab as usize;
        if tabs == 0 || t >= tabs { 0 } else { t }
    }

    /// Bucketed for the cache key: widths to eighths, which is finer than
    /// the eye reads a card at this distance and coarse enough that renaming
    /// one entry does not rebuild the scene.
    pub fn quantized(self) -> PanelContext {
        let mut out = self;
        for r in &mut out.rows {
            r.width01 = (r.width01 * 8.0).round() / 8.0;
        }
        out
    }
}

#[cfg(test)]
mod tests {

    /// On a conversation longer than the board is tall, the panel showed the OPENING of the
    /// chat forever — a museum of the beginning, bolted to the wall. The rows a board in the
    /// room is worth having are the recent ones.
    #[test]
    fn a_long_conversation_shows_its_latest_topics_not_its_first() {
        // Eight topic runs, each a different length so the rows are distinguishable.
        let spans: Vec<(usize, usize)> = (0..8).map(|i| (i * 10, i * 10 + i + 1)).collect();
        let rows = from_spans(&spans, None);
        assert_eq!(rows.len(), MAX_ROWS);

        // The widths come from the LAST five spans (lengths 4..8), so the narrowest row here
        // is wider in proportion than the first span's would have been.
        let tail = from_spans(&spans[3..], None);
        assert_eq!(rows.len(), tail.len());
        for (a, b) in rows.iter().zip(tail.iter()) {
            assert!((a.width01 - b.width01).abs() < 1e-6, "the window is not the tail: {rows:?}");
        }

        // A conversation that fits is unchanged.
        let short = vec![(0, 3), (3, 9)];
        assert_eq!(from_spans(&short, None).len(), 2);
    }

    /// `active` indexes the FULL span list, so the caller says "segment k" without knowing
    /// which window survived — and the live segment stays accented as the chat grows past
    /// the board's height.
    #[test]
    fn the_accent_follows_the_live_segment_through_the_window() {
        let spans: Vec<(usize, usize)> = (0..8).map(|i| (i * 10, i * 10 + 4)).collect();
        let rows = from_spans(&spans, Some(7));
        assert!(rows.last().unwrap().accent, "the live segment lost its accent: {rows:?}");
        assert_eq!(rows.iter().filter(|r| r.accent).count(), 1);

        // A segment that scrolled off accents nothing — better than accenting the wrong row.
        let rows = from_spans(&spans, Some(0));
        assert!(rows.iter().all(|r| !r.accent), "an off-screen segment accented a visible row");

        // And in a short conversation the index is used directly.
        let rows = from_spans(&[(0, 3), (3, 9), (9, 12)], Some(1));
        assert!(rows[1].accent && !rows[0].accent && !rows[2].accent);
    }

    /// The renderer lit `i == 0` unconditionally, so the in-scene board reported Files
    /// however far the app had moved. The tabs are what make the board read as live rather
    /// than as scenery, so a wrong one is worse than none.
    #[test]
    fn the_lit_tab_follows_the_page_and_never_points_at_a_plate_that_is_not_drawn() {
        let base = PanelContext::from_rows(&placeholder());
        assert_eq!(base.active_tab(4), 0, "the default is the first plate");

        for i in 0..4 {
            assert_eq!(base.with_active_tab(i).active_tab(4), i);
        }

        // A page beyond the plates the skin draws lights the FIRST rather than an index
        // that is not there: a panel with no plate lit reads as switched off.
        assert_eq!(base.with_active_tab(9).active_tab(4), 0);
        assert_eq!(base.with_active_tab(4).active_tab(4), 0, "0-based: 4 is past four plates");
        // A skin with no tabs at all cannot light one.
        assert_eq!(base.with_active_tab(2).active_tab(0), 0);
        // A skin with fewer plates clamps rather than overflowing.
        assert_eq!(base.with_active_tab(3).active_tab(2), 0);
        assert_eq!(base.with_active_tab(1).active_tab(2), 1);
    }

    /// The scene cache keys on a bucketed copy of this, and the lit tab must survive it —
    /// otherwise switching pages would light the plate for one frame and lose it on the
    /// next rebuild.
    #[test]
    fn quantizing_keeps_the_lit_tab() {
        let c = PanelContext::from_rows(&placeholder()).with_active_tab(2);
        assert_eq!(c.quantized().active_tab(4), 2);
    }
    use super::*;

    fn items(v: &[(&str, bool)]) -> Vec<(String, bool)> {
        v.iter().map(|(s, a)| ((*s).to_string(), *a)).collect()
    }

    #[test]
    fn a_longer_segment_gets_a_wider_card_and_the_active_one_is_marked() {
        // Real data: spans as convograph reports them.
        let r = from_spans(&[(0, 3), (3, 20), (20, 25)], Some(1));
        assert!(r[1].width01 > r[0].width01 && r[1].width01 > r[2].width01);
        assert!((r[1].width01 - MAX_WIDTH01).abs() < 1e-6);
        assert!(r[1].accent && !r[0].accent);
        // Degenerate spans do not vanish or divide by zero.
        let z = from_spans(&[(5, 5), (5, 5)], None);
        assert_eq!(z.len(), 2);
        assert!(z.iter().all(|x| x.width01 >= MIN_WIDTH01));
        assert!(from_spans(&[], None).is_empty());
        // Reversed/rubbish spans clamp rather than panic.
        assert!(from_spans(&[(9, 2)], None)[0].width01 >= MIN_WIDTH01);
    }

    #[test]
    fn the_cache_key_survives_a_rename_but_notices_a_real_change() {
        // The property the scene cache depends on: churn in the context must
        // not rebuild a 7000-line world, but a genuinely different board must.
        let a = PanelContext::from_rows(&from_labels(&items(&[
            ("soil_analysis.pdf", false), ("field_notes.mdx", false)])));
        let b = PanelContext::from_rows(&from_labels(&items(&[
            ("soil_analysis.pdf", false), ("field_notes.mdy", false)])));
        assert_eq!(a.quantized(), b.quantized(), "a one-character rename is not a rebuild");
        let c = PanelContext::from_rows(&from_labels(&items(&[
            ("soil_analysis.pdf", false), ("field_notes.mdx", false), ("third.csv", false)])));
        assert_ne!(a.quantized(), c.quantized(), "a new entry is");
        let d = PanelContext::from_rows(&from_labels(&items(&[
            ("soil_analysis.pdf", true), ("field_notes.mdx", false)])));
        assert_ne!(a.quantized(), d.quantized(), "so is moving the selection");
    }

    #[test]
    fn a_context_round_trips_and_caps_without_panicking() {
        let many: Vec<(String, bool)> = (0..40).map(|i| (format!("e{i}"), false)).collect();
        let ctx = PanelContext::from_rows(&from_labels(&many));
        assert_eq!(ctx.rows().len(), MAX_ROWS);
        assert!(PanelContext::from_rows(&[]).rows().is_empty());
        // Default is the stand-in, not an empty board: a scene built before
        // any context arrives should still look like a board.
        assert_eq!(PanelContext::default().rows().len(), placeholder().len());
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

/// The lit tab for the current scene context, given how many plates the skin draws.
///
/// A free function because `scene3d` builds the panel deep inside its mesh walk and reaches
/// for the thread-local context there rather than threading it through eight arguments —
/// the same way it already reads `panel_context()` for the rows.
pub fn panel_context_active_tab(tabs: usize) -> usize {
    crate::scene3d::panel_context().active_tab(tabs)
}
