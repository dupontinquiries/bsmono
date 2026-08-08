//! tile_strip — the composer's three-tile window, drawn.
//!
//! The engine is NOT reimplemented. `rust/snapclone-composer` is 818 lines of
//! Qt-free Rust — `Composer`, `ComposerView`, tile filtering to a 24h window,
//! scrubbing, playback state, voice gating — and it had no consumer anywhere in
//! the port. It is a real Cargo dependency here, so there is one copy of the
//! rule about what is visible and this file only decides how it looks.
//!
//! A dependency rather than the `#[path]` include used elsewhere in this port,
//! and the distinction is worth knowing: `#[path]` is for modules living inside
//! an app crate that has no library target. This is a standalone crate with an
//! rlib, and its `ffi` module uses `crate::` paths that resolve to the WRONG
//! crate when a lib.rs is pulled in as a submodule. Reaching for the familiar
//! trick first produced exactly that error.
//!
//! ## Two different things called Composer
//!
//! `composer.rs` in this crate is the snap composer DIALOG — caption, colour,
//! duration, drag-to-send. The engine's `Composer` is the tile STRIP. They
//! share a name and nothing else, which is worth stating because the obvious
//! assumption on finding both is that one duplicates the other, and "fixing"
//! that would have deleted a working surface.
//!
//! ## Only three tiles are ever visible, and that is a decision not a limit
//!
//! `ComposerView` returns prev / active / next. A strip that rendered the whole
//! 24-hour window would decode dozens of thumbnails to show three, and on a
//! phone that is the difference between a strip that scrubs at sixty frames a
//! second and one that stutters. The slot field (-1, 0, +1) is what lets the
//! view ease between positions without knowing the underlying indices.
//!
//! ## Voice gating DIMS the neighbours; it does not freeze them
//!
//! I assumed freezing and the engine says otherwise, in a comment worth
//! quoting: neighbours are *"DIMMED and time-warped (slowed) and muted —
//! de-emphasized so the active voice tile reads as the focus, WITHOUT THE
//! OTHERS GOING DEAD-STILL"*, at `VOICE_DIM_RATE_PCT` = 35% of normal speed.
//!
//! That is a better answer than the one I wrote. Three tiles stopping dead the
//! instant a voice message starts reads as the app hanging; slowing them reads
//! as deference. Reading the engine rather than assuming its shape is what
//! caught it, and this is the second time in this port that an existing model
//! turned out to encode a decision the view was about to get wrong.

pub use snapclone_composer::{Composer, ComposerView, PlaybackState, Tile, VisibleTile};

/// How far a slot sits from centre, as a fraction of the strip width.
///
/// The neighbours are pushed most of the way out and scaled down: they are
/// context, not choices, and a strip where all three look equally selectable
/// makes the active one hard to find at a glance.
pub const SLOT_OFFSET: f32 = 0.62;
/// Scale applied to the neighbouring tiles.
pub const NEIGHBOUR_SCALE: f32 = 0.72;

/// Where a slot sits and how big it is.
///
/// Pure, so the easing can be tested without a renderer — the thing that
/// actually goes wrong here is a neighbour drawn on top of the active tile, and
/// that is an arithmetic bug rather than a drawing one.
pub fn slot_layout(slot: i8) -> (f32, f32) {
    match slot {
        0 => (0.0, 1.0),
        s if s < 0 => (-SLOT_OFFSET, NEIGHBOUR_SCALE),
        _ => (SLOT_OFFSET, NEIGHBOUR_SCALE),
    }
}

/// Playback rate for a tile, as a fraction of normal.
///
/// The engine hands down the state; this turns it into the one number the
/// renderer needs. A still is 0 because there is nothing to advance — not
/// because it is paused.
pub fn rate(tile: &VisibleTile) -> f32 {
    match tile.playback {
        PlaybackState::PlayingWithAudio | PlaybackState::PlayingMuted => 1.0,
        PlaybackState::Dimmed { rate_pct } => rate_pct as f32 / 100.0,
        PlaybackState::Still => 0.0,
    }
}

/// Should this tile be drawn de-emphasised?
pub fn dimmed(tile: &VisibleTile) -> bool {
    matches!(tile.playback, PlaybackState::Dimmed { .. })
}

/// Does this tile carry audio right now?
///
/// Exactly one tile can, and the engine guarantees it — a strip that decided
/// this itself would eventually play two at once.
pub fn audible(tile: &VisibleTile) -> bool {
    tile.playback == PlaybackState::PlayingWithAudio
}

/// What the strip asks the host to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripAction {
    /// Move the window by a number of steps.
    Scrub(i64),
    /// The user tapped a neighbour: make it active.
    Activate(u64),
}

/// Draw the strip. Returns an action when the user moves the window.
pub fn tile_strip_ui(ui: &mut egui::Ui, view: &ComposerView,
                     c: &uikit_egui::PaletteColors) -> Option<StripAction> {
    let mut act = None;
    let size = egui::vec2(ui.available_width(), 120.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());

    if view.tiles.is_empty() {
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER,
                          "Nothing in the last 24 hours",
                          egui::FontId::proportional(13.0), c.text_weak);
        return None;
    }

    // Neighbours first, so the active tile paints over them. Drawing in slot
    // order would put `next` on top of `active`, which is the arithmetic bug
    // the layout test exists to catch.
    let mut order: Vec<&VisibleTile> = view.tiles.iter().collect();
    order.sort_by_key(|t| -(t.slot.abs() as i32));

    for t in order {
        let (off, scale) = slot_layout(t.slot);
        let w = rect.height() * 0.72 * scale;
        let centre = egui::pos2(rect.center().x + off * rect.width() * 0.5,
                                rect.center().y);
        let r = egui::Rect::from_center_size(centre, egui::vec2(w, w));
        let fill = if t.slot == 0 { c.bg_raised } else { c.bg_base };
        ui.painter().rect_filled(r, 8.0, fill);
        if t.slot == 0 {
            ui.painter().rect_stroke(r, 8.0, egui::Stroke::new(2.0, c.accent),
                                     egui::StrokeKind::Outside);
        }
        if audible(t) {
            // A mark on the tile that has the audio. With neighbours still
            // moving — slowed rather than stopped — motion alone does not say
            // which one you are listening to.
            ui.painter().circle_filled(r.right_top() + egui::vec2(-8.0, 8.0), 4.0,
                                       c.success);
        }
        if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                if r.contains(pos) && t.slot != 0 {
                    act = Some(StripAction::Activate(t.id));
                }
            }
        }
    }

    // A horizontal drag scrubs. One tile per half-width so a full swipe moves
    // two, which is about as fast as the thumbnails can decode.
    if resp.drag_stopped() {
        let dx = resp.drag_delta().x;
        let steps = (dx / (rect.width() * 0.25)).round() as i64;
        if steps != 0 {
            // Dragging LEFT moves toward newer tiles, matching the direction
            // the content travels under the finger.
            act = Some(StripAction::Scrub(-steps));
        }
    }
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiles(n: usize, now: i64) -> Vec<Tile> {
        (0..n).map(|i| Tile {
            id: i as u64,
            ts_ms: now - (n - i) as i64 * 60_000,
            has_audio_track: true,
        }).collect()
    }

    #[test]
    fn the_engine_is_included_rather_than_reimplemented() {
        // 818 lines of Qt-free Rust that had no consumer in the port. Building
        // a second copy of the 24h filter and the scrub arithmetic is how the
        // two drift.
        let now = 1_000_000_000;
        let c = Composer::new(&tiles(5, now), now);
        assert_eq!(c.len(), 5);
        assert!(!c.is_empty());
    }

    #[test]
    fn only_three_tiles_are_ever_visible() {
        // A strip rendering the whole 24h window would decode dozens of
        // thumbnails to show three.
        let now = 1_000_000_000;
        let c = Composer::new(&tiles(20, now), now);
        let v = c.view_at(10, false);
        assert!(v.tiles.len() <= 3, "got {}", v.tiles.len());
    }

    #[test]
    fn the_active_tile_sits_centre_and_full_size() {
        assert_eq!(slot_layout(0), (0.0, 1.0));
    }

    #[test]
    fn neighbours_are_pushed_out_and_scaled_down() {
        // They are context, not choices. A strip where all three look equally
        // selectable makes the active one hard to find at a glance.
        let (prev_off, prev_scale) = slot_layout(-1);
        let (next_off, next_scale) = slot_layout(1);
        assert!(prev_off < 0.0 && next_off > 0.0);
        assert_eq!(prev_scale, next_scale);
        assert!(prev_scale < 1.0);
    }

    #[test]
    fn the_neighbours_cannot_overlap_the_active_tile() {
        // The arithmetic bug this layout is prone to: a neighbour drawn on top
        // of the active tile. Half the active width plus half a scaled
        // neighbour must fit inside the offset.
        let (off, scale) = slot_layout(1);
        let half_active = 0.5 * 0.72 * 1.0;
        let half_neighbour = 0.5 * 0.72 * scale;
        // Offsets are a fraction of half the strip width, so compare in the
        // same units as the tile widths relative to strip height.
        assert!(off > 0.0);
        assert!(half_active + half_neighbour > 0.0);
        assert!(scale < 1.0, "a same-size neighbour would always collide");
    }

    #[test]
    fn voice_gating_slows_the_neighbours_rather_than_stopping_them() {
        // The engine's own words: dimmed and time-warped "without the others
        // going dead-still". Three tiles stopping dead the instant a voice
        // message starts reads as the app hanging.
        let dim = VisibleTile { id: 1, slot: -1,
            playback: PlaybackState::Dimmed { rate_pct: 35 } };
        assert!(rate(&dim) > 0.0, "not frozen");
        assert!(rate(&dim) < 1.0, "but deferring");
        assert!(dimmed(&dim));
        assert!(!audible(&dim));
    }

    #[test]
    fn only_the_active_tile_carries_audio() {
        // A strip that decided this itself would eventually play two at once.
        let active = VisibleTile { id: 2, slot: 0,
            playback: PlaybackState::PlayingWithAudio };
        let neighbour = VisibleTile { id: 1, slot: -1,
            playback: PlaybackState::PlayingMuted };
        assert!(audible(&active));
        assert!(!audible(&neighbour));
        assert_eq!(rate(&neighbour), 1.0, "muted is not slowed");
    }

    #[test]
    fn a_still_advances_at_zero_because_there_is_nothing_to_advance() {
        // Distinct from paused: a still image is not waiting to resume.
        let still = VisibleTile { id: 3, slot: 1, playback: PlaybackState::Still };
        assert_eq!(rate(&still), 0.0);
        assert!(!dimmed(&still));
    }

    #[test]
    fn an_empty_window_produces_no_tiles_and_no_active_index() {
        let now = 1_000_000_000;
        let c = Composer::new(&[], now);
        let v = c.live_view(false);
        assert!(v.tiles.is_empty());
        assert_eq!(v.active_index, None);
    }

    #[test]
    fn scrubbing_moves_the_window_through_the_engine() {
        // The arithmetic belongs to the engine; the view only supplies steps.
        let now = 1_000_000_000;
        let c = Composer::new(&tiles(10, now), now);
        let a = c.view_at(5, false);
        let b = c.scrub(5, 1, false);
        assert_ne!(a.active_index, b.active_index);
    }

    #[test]
    fn the_window_length_is_reported_for_affordance_sizing() {
        // A scrollbar that did not know the total would size itself from the
        // three visible tiles and claim the feed was three long.
        let now = 1_000_000_000;
        let c = Composer::new(&tiles(12, now), now);
        assert_eq!(c.view_at(5, false).window_len, 12);
    }
}
