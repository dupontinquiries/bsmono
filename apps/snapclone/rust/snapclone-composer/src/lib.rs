//! snapclone-composer — the group live-composer decision model.
//!
//! The group chat surface shows a "live composer": the most recent media as a
//! small stack of tiles, and you scrub up/down through the last 24h to slot
//! tiles in and out of a 3-up window (prev / active / next). One tile is the
//! ACTIVE one (centered, full playback). When the active tile has identified
//! voice, the other two must NOT play audio and freeze on a still — "one at a
//! time." Tapping any visible sub-tile makes it active (replay).
//!
//! That's a lot of interacting rules, all of it pure state math over
//! (tiles, scrub position, now, voice flag). This crate is exactly that math —
//! no Qt, no media decoding, no gestures. The app feeds it the tile list +
//! current scrub offset (driven by snapclone-gesture) + whether the active
//! tile's audio track has voice, and gets back a [`ComposerView`]: which tile
//! indices occupy prev/active/next, and each visible tile's [`PlaybackState`].
//! The Qt layer renders that and wires QMediaPlayer to it.
//!
//! Design choices (override by adjusting the model + its tests):
//!   * **3-up window = active ± 1 in time order** (prev = older, next = newer).
//!   * **Active = the tile the scrub position currently rests on.** Scrub is in
//!     "tile units" (the gesture layer converts detents → integer steps); a
//!     fractional offset means mid-drag and is reported so the UI can ease.
//!   * **24h window**: tiles older than `now - 24h` are excluded entirely.
//!   * **Voice gating**: when the active tile `has_voice`, prev/next are
//!     `Dimmed { rate_pct }` (kept playing but de-emphasized + time-warped +
//!     muted); the active tile is `PlayingWithAudio`. With no voice, prev/next
//!     are `PlayingMuted` (loop silently), active is `PlayingWithAudio`.
//!   * **Tap-to-activate**: [`activate_index`] recomputes the view as if scrub
//!     landed on that tile (replay semantics: active always (re)starts).

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod ffi;

/// Milliseconds in a 24-hour window.
pub const WINDOW_24H_MS: i64 = 24 * 60 * 60 * 1000;

/// A media item in the feed. `id` is opaque to this crate (the app maps it back
/// to a real media handle). `ts_ms` is the capture/receive time (unix millis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile {
    pub id: u64,
    pub ts_ms: i64,
    /// True if this tile is a video/audio item that *could* carry voice. Stills
    /// never play audio regardless of the voice flag.
    pub has_audio_track: bool,
}

/// Per-tile playback instruction the UI should apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    /// Centered/active tile: play with audio from the start.
    PlayingWithAudio,
    /// Adjacent tile, no voice in the active tile: loop silently.
    PlayingMuted,
    /// Adjacent tile, voice playing in active tile: keep playing but DIMMED and
    /// time-warped (slowed) and muted — de-emphasized so the active voice tile
    /// reads as the focus, without the others going dead-still. `rate_pct` is
    /// the playback rate as a percent of normal (e.g. 35 = play at 0.35x).
    Dimmed { rate_pct: u16 },
    /// A still image (no audio track): just display it.
    Still,
}

/// Time-warp rate applied to neighbors while the active tile plays voice,
/// as a percent of normal speed. Tunable; the UI may ease toward it.
pub const VOICE_DIM_RATE_PCT: u16 = 35;

/// One slot in the visible 3-up window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleTile {
    pub id: u64,
    pub playback: PlaybackState,
    /// Position in the window: -1 = prev (older), 0 = active, +1 = next (newer).
    pub slot: i8,
}

/// The computed view: up to three visible tiles + metadata for easing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComposerView {
    /// 0..=3 visible tiles, ordered prev, active, next (only those that exist).
    pub tiles: Vec<VisibleTile>,
    /// Index (into the *filtered* tile list) of the active tile, or None if the
    /// feed is empty.
    pub active_index: Option<usize>,
    /// Total tiles in the 24h window (for scrollbar/affordance sizing).
    pub window_len: usize,
    /// True if the active tile is currently playing voice (others frozen).
    pub voice_gating: bool,
}

/// Inputs that don't change per-frame; the tile list + clock.
pub struct Composer {
    /// Tiles sorted ascending by ts_ms (oldest first). Filtered to 24h on build.
    tiles: Vec<Tile>,
    now_ms: i64,
}

impl Composer {
    /// Build from an unsorted tile slice; filters to the trailing 24h window and
    /// sorts oldest→newest. Ties on ts broken by id for determinism.
    pub fn new(tiles: &[Tile], now_ms: i64) -> Self {
        let cutoff = now_ms - WINDOW_24H_MS;
        let mut v: Vec<Tile> = tiles.iter().copied().filter(|t| t.ts_ms >= cutoff).collect();
        v.sort_by(|a, b| a.ts_ms.cmp(&b.ts_ms).then(a.id.cmp(&b.id)));
        Composer { tiles: v, now_ms }
    }

    /// Number of tiles in the 24h window.
    pub fn len(&self) -> usize { self.tiles.len() }
    pub fn is_empty(&self) -> bool { self.tiles.is_empty() }
    pub fn now_ms(&self) -> i64 { self.now_ms }

    /// The default active index: the NEWEST tile (top of the live composer).
    pub fn newest_index(&self) -> Option<usize> {
        if self.tiles.is_empty() { None } else { Some(self.tiles.len() - 1) }
    }

    fn playback_for(&self, tile: &Tile, is_active: bool, voice: bool) -> PlaybackState {
        if !tile.has_audio_track {
            return PlaybackState::Still;
        }
        if is_active {
            PlaybackState::PlayingWithAudio
        } else if voice {
            PlaybackState::Dimmed { rate_pct: VOICE_DIM_RATE_PCT }
        } else {
            PlaybackState::PlayingMuted
        }
    }

    /// Compute the view for a given active index and voice state.
    /// `active_index` is clamped into range; out-of-range (e.g. from momentum)
    /// is pinned to the nearest valid tile rather than panicking.
    pub fn view_at(&self, active_index: usize, active_has_voice: bool) -> ComposerView {
        if self.tiles.is_empty() {
            return ComposerView::default();
        }
        let last = self.tiles.len() - 1;
        let active = active_index.min(last);

        // Voice only gates if the active tile actually has an audio track.
        let voice = active_has_voice && self.tiles[active].has_audio_track;

        let mut out = Vec::with_capacity(3);
        // prev (older): active-1
        if active > 0 {
            let t = &self.tiles[active - 1];
            out.push(VisibleTile { id: t.id, slot: -1,
                playback: self.playback_for(t, false, voice) });
        }
        // active
        {
            let t = &self.tiles[active];
            out.push(VisibleTile { id: t.id, slot: 0,
                playback: self.playback_for(t, true, voice) });
        }
        // next (newer): active+1
        if active < last {
            let t = &self.tiles[active + 1];
            out.push(VisibleTile { id: t.id, slot: 1,
                playback: self.playback_for(t, false, voice) });
        }

        ComposerView {
            tiles: out,
            active_index: Some(active),
            window_len: self.tiles.len(),
            voice_gating: voice,
        }
    }

    /// View at the live position (newest tile active).
    pub fn live_view(&self, active_has_voice: bool) -> ComposerView {
        match self.newest_index() {
            Some(i) => self.view_at(i, active_has_voice),
            None => ComposerView::default(),
        }
    }

    /// Convert an integer scrub offset (in tile-steps, signed; the gesture layer
    /// produces this from detents) relative to a base index into a concrete
    /// active index, clamped. Positive = toward newer, negative = toward older.
    pub fn scrub(&self, base_index: usize, steps: i64, active_has_voice: bool) -> ComposerView {
        if self.tiles.is_empty() {
            return ComposerView::default();
        }
        let last = self.tiles.len() as i64 - 1;
        let target = (base_index as i64 + steps).clamp(0, last) as usize;
        self.view_at(target, active_has_voice)
    }

    /// Tap-to-activate by the tile's id (replay). Returns the view with that
    /// tile active, or the unchanged-by-default live view if the id is gone
    /// (e.g. it aged out of the 24h window between render and tap).
    pub fn activate_id(&self, id: u64, active_has_voice: bool) -> ComposerView {
        match self.tiles.iter().position(|t| t.id == id) {
            Some(i) => self.view_at(i, active_has_voice),
            None => self.live_view(active_has_voice),
        }
    }
}

// ─── layout: the 2D retiling window manager ("two small, one big") ───────────
//
// The 3-up window is a 2D TILING layout, not a 3D solid. One tile is the BIG
// (focused) pane; the other two are SMALL peripheral panes — "two small, one
// big". The "rotate" gesture re-tiles: it promotes a different tile into the
// big slot and demotes the current one, reflowing the rects. Purpose (per the
// designer): keep context open while optimizing the viewport, and "buffer the
// eyes" when three messages arrive at once — one leads, two wait in the
// periphery instead of three equal things competing.
//
// This module is pure rect math over (viewport, which slot is big, an
// optional small-pane edge). It is INDEPENDENT of the feed/voice logic above:
// ComposerView decides which tiles + playback; TileLayout decides where each
// sits on screen. The Qt layer animates between successive layouts.

/// A rectangle in viewport pixels (origin top-left, x→right, y→down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn area(&self) -> f32 { self.w * self.h }
}

/// Which edge the two small panes stack along, relative to the big pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmallEdge {
    /// Small panes run down the right side (big pane on the left). Good for
    /// landscape / wide viewports.
    Right,
    /// Small panes run along the bottom (big pane on top). Good for portrait
    /// (phone) viewports.
    Bottom,
}

/// The computed placement of one slot's tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TilePlacement {
    /// Window slot (-1 prev, 0 active/big-by-default, +1 next).
    pub slot: i8,
    pub rect: Rect,
    /// True for the single big/focused pane.
    pub is_big: bool,
}

/// The 2D tiling layout engine.
#[derive(Debug, Clone, Copy)]
pub struct TileLayout {
    pub viewport: Rect,
    pub edge: SmallEdge,
    /// Fraction of the long axis the big pane takes (0..1). e.g. 0.68 = big
    /// pane is 68% of width (Right) or height (Bottom); smalls share the rest.
    pub big_fraction: f32,
    /// Gap between panes, in pixels.
    pub gap: f32,
}

/// Default big-pane fraction — big enough to dominate, small enough to keep the
/// two peripherals legible.
pub const DEFAULT_BIG_FRACTION: f32 = 0.68;

impl TileLayout {
    pub fn new(viewport: Rect, edge: SmallEdge) -> Self {
        TileLayout { viewport, edge, big_fraction: DEFAULT_BIG_FRACTION, gap: 8.0 }
    }

    /// Pick a sensible edge from the viewport aspect: wide → Right, tall →
    /// Bottom.
    pub fn auto_edge(viewport: Rect) -> SmallEdge {
        if viewport.w >= viewport.h { SmallEdge::Right } else { SmallEdge::Bottom }
    }

    /// Lay out the present slots, with `big_slot` getting the big pane. The
    /// other present slots become the small panes (in slot order). Returns a
    /// placement per present slot. `slots` are the window slots that exist
    /// (1..=3), as produced by ComposerView.
    pub fn layout(&self, slots: &[i8], big_slot: i8) -> Vec<TilePlacement> {
        if slots.is_empty() {
            return Vec::new();
        }
        // If the requested big slot isn't present, fall back to the first slot.
        let big = if slots.contains(&big_slot) { big_slot } else { slots[0] };

        // Single tile: it fills the viewport.
        if slots.len() == 1 {
            return vec![TilePlacement { slot: slots[0], rect: self.viewport, is_big: true }];
        }

        let smalls: Vec<i8> = slots.iter().copied().filter(|&s| s != big).collect();
        let n_small = smalls.len() as f32;
        let mut out = Vec::with_capacity(slots.len());
        let g = self.gap;

        match self.edge {
            SmallEdge::Right => {
                let big_w = (self.viewport.w - g) * self.big_fraction;
                let small_w = self.viewport.w - g - big_w;
                out.push(TilePlacement {
                    slot: big, is_big: true,
                    rect: Rect { x: self.viewport.x, y: self.viewport.y,
                                 w: big_w, h: self.viewport.h },
                });
                // smalls stacked vertically in the right column
                let small_x = self.viewport.x + big_w + g;
                let col_h = self.viewport.h - g * (n_small - 1.0).max(0.0);
                let each_h = col_h / n_small;
                for (i, &s) in smalls.iter().enumerate() {
                    let y = self.viewport.y + i as f32 * (each_h + g);
                    out.push(TilePlacement {
                        slot: s, is_big: false,
                        rect: Rect { x: small_x, y, w: small_w, h: each_h },
                    });
                }
            }
            SmallEdge::Bottom => {
                let big_h = (self.viewport.h - g) * self.big_fraction;
                let small_h = self.viewport.h - g - big_h;
                out.push(TilePlacement {
                    slot: big, is_big: true,
                    rect: Rect { x: self.viewport.x, y: self.viewport.y,
                                 w: self.viewport.w, h: big_h },
                });
                // smalls stacked horizontally along the bottom row
                let small_y = self.viewport.y + big_h + g;
                let row_w = self.viewport.w - g * (n_small - 1.0).max(0.0);
                let each_w = row_w / n_small;
                for (i, &s) in smalls.iter().enumerate() {
                    let x = self.viewport.x + i as f32 * (each_w + g);
                    out.push(TilePlacement {
                        slot: s, is_big: false,
                        rect: Rect { x, y: small_y, w: each_w, h: small_h },
                    });
                }
            }
        }
        out
    }

    /// "Re-tile" step: given the current big slot and the present slots, pick
    /// the NEXT slot to promote to big (cycles through present slots). This is
    /// what the rotate gesture drives — promote a peripheral tile into focus,
    /// demoting the current big one to the periphery.
    pub fn next_big(&self, slots: &[i8], current_big: i8, forward: bool) -> i8 {
        if slots.is_empty() { return current_big; }
        // Order slots prev(-1), active(0), next(+1) for a stable cycle.
        let mut ordered: Vec<i8> = slots.to_vec();
        ordered.sort();
        let idx = ordered.iter().position(|&s| s == current_big).unwrap_or(0);
        let n = ordered.len();
        let nxt = if forward { (idx + 1) % n } else { (idx + n - 1) % n };
        ordered[nxt]
    }
}

// ─── screen adaptation: layout params from screen geometry ───────────────────
//
// The composer's feel is screen-dependent (the designer will tune this a lot).
// Rather than scatter breakpoint magic numbers through the Qt widget, derive the
// layout *parameters* from the screen here, as pure tested logic. The widget
// reads a ScreenProfile and applies it; tuning = editing these thresholds with
// the tests as a safety net.
//
// What adapts:
//   * edge (small panes right vs bottom) — by orientation.
//   * big_fraction — phones give the big pane more of the screen (small screen,
//     one thing should dominate); large/landscape screens can afford a smaller
//     big pane since the peripherals are still legible.
//   * max_visible — tiny screens may only have room for big + 1 peripheral, not
//     2; very large screens stay at 3 (the design is "two small one big").

/// A coarse screen size class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenClass {
    /// Phone (smallest edge < ~480 logical px).
    Phone,
    /// Tablet / large phone.
    Tablet,
    /// Desktop / large window.
    Desktop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
}

/// Resolved layout parameters for a given screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenProfile {
    pub class: ScreenClass,
    pub orientation: Orientation,
    pub edge: SmallEdge,
    pub big_fraction: f32,
    /// Max tiles to show (2 = big+1 small on cramped screens, 3 = the full
    /// two-small-one-big).
    pub max_visible: u8,
}

impl ScreenProfile {
    /// Derive a profile from a viewport's logical pixel size.
    pub fn from_size(w: f32, h: f32) -> Self {
        let short = w.min(h);
        let orientation = if w >= h { Orientation::Landscape } else { Orientation::Portrait };
        let class = if short < 480.0 {
            ScreenClass::Phone
        } else if short < 900.0 {
            ScreenClass::Tablet
        } else {
            ScreenClass::Desktop
        };
        // Edge: portrait stacks the peripherals along the bottom (thumb reach,
        // wide content); landscape puts them in a right column.
        let edge = match orientation {
            Orientation::Portrait => SmallEdge::Bottom,
            Orientation::Landscape => SmallEdge::Right,
        };
        // big_fraction: phones let the active tile dominate; bigger screens can
        // shrink it because the peripherals remain legible.
        let big_fraction = match class {
            ScreenClass::Phone => 0.72,
            ScreenClass::Tablet => 0.66,
            ScreenClass::Desktop => 0.60,
        };
        // max_visible: a very short portrait phone (little room for a bottom row
        // of two) drops to big + 1; everything else keeps the full 3.
        let max_visible = if class == ScreenClass::Phone
            && orientation == Orientation::Portrait
            && short < 360.0
        {
            2
        } else {
            3
        };
        ScreenProfile { class, orientation, edge, big_fraction, max_visible }
    }

    /// Build a TileLayout for a viewport using this profile.
    pub fn layout_for(&self, viewport: Rect) -> TileLayout {
        let mut l = TileLayout::new(viewport, self.edge);
        l.big_fraction = self.big_fraction;
        l
    }

    /// Clamp a set of visible slots to this profile's max (drops the farthest-
    /// from-active first: prefers keeping active + nearest neighbor). Slots are
    /// the window slots (-1/0/+1); returns the kept subset preserving order.
    pub fn clamp_slots(&self, slots: &[i8]) -> Vec<i8> {
        if slots.len() as u8 <= self.max_visible {
            return slots.to_vec();
        }
        // Keep active (0) always, then nearest by |slot|, up to max_visible.
        let mut kept: Vec<i8> = slots.to_vec();
        kept.sort_by_key(|&s| (s as i32).abs()); // 0 first, then ±1
        kept.truncate(self.max_visible as usize);
        // restore display order (prev, active, next)
        kept.sort();
        kept
    }
}

#[cfg(test)]
mod screen_tests {
    use super::*;

    #[test]
    fn phone_portrait_is_bottom_edge_big_dominant() {
        let p = ScreenProfile::from_size(400.0, 850.0);
        assert_eq!(p.class, ScreenClass::Phone);
        assert_eq!(p.orientation, Orientation::Portrait);
        assert_eq!(p.edge, SmallEdge::Bottom);
        assert!(p.big_fraction >= 0.70, "phone lets the active tile dominate");
        assert_eq!(p.max_visible, 3);
    }

    #[test]
    fn landscape_is_right_edge() {
        let p = ScreenProfile::from_size(1200.0, 700.0);
        assert_eq!(p.orientation, Orientation::Landscape);
        assert_eq!(p.edge, SmallEdge::Right);
    }

    #[test]
    fn bigger_screen_shrinks_big_fraction() {
        let phone = ScreenProfile::from_size(400.0, 850.0).big_fraction;
        let tablet = ScreenProfile::from_size(800.0, 1200.0).big_fraction;
        let desktop = ScreenProfile::from_size(1600.0, 1000.0).big_fraction;
        assert!(phone > tablet, "phone big-pane > tablet");
        assert!(tablet > desktop, "tablet big-pane > desktop");
    }

    #[test]
    fn class_breakpoints() {
        assert_eq!(ScreenProfile::from_size(400.0, 850.0).class, ScreenClass::Phone);
        assert_eq!(ScreenProfile::from_size(700.0, 1000.0).class, ScreenClass::Tablet);
        assert_eq!(ScreenProfile::from_size(1000.0, 1400.0).class, ScreenClass::Desktop);
    }

    #[test]
    fn tiny_portrait_phone_drops_to_two_visible() {
        let p = ScreenProfile::from_size(320.0, 600.0); // very narrow
        assert_eq!(p.max_visible, 2);
        // clamp 3 slots → keep active + nearest neighbor
        let kept = p.clamp_slots(&[-1, 0, 1]);
        assert_eq!(kept.len(), 2);
        assert!(kept.contains(&0), "active always kept");
    }

    #[test]
    fn clamp_is_noop_when_within_max() {
        let p = ScreenProfile::from_size(400.0, 850.0); // max 3
        assert_eq!(p.clamp_slots(&[-1, 0, 1]), vec![-1, 0, 1]);
        assert_eq!(p.clamp_slots(&[0, 1]), vec![0, 1]);
    }

    #[test]
    fn profile_layout_round_trips_into_tilelayout() {
        let p = ScreenProfile::from_size(400.0, 850.0);
        let l = p.layout_for(Rect { x: 0.0, y: 0.0, w: 400.0, h: 850.0 });
        assert_eq!(l.edge, SmallEdge::Bottom);
        // and it produces a valid two-small-one-big layout
        let placements = l.layout(&[-1, 0, 1], 0);
        assert_eq!(placements.iter().filter(|t| t.is_big).count(), 1);
    }

    #[test]
    fn square_is_landscape_by_convention() {
        // w == h → treat as landscape (>=), a deterministic tie-break.
        let p = ScreenProfile::from_size(600.0, 600.0);
        assert_eq!(p.orientation, Orientation::Landscape);
    }
}


#[cfg(test)]
mod layout_tests {
    use super::*;

    fn vp(w: f32, h: f32) -> Rect { Rect { x: 0.0, y: 0.0, w, h } }

    #[test]
    fn auto_edge_by_aspect() {
        assert_eq!(TileLayout::auto_edge(vp(1000.0, 600.0)), SmallEdge::Right);
        assert_eq!(TileLayout::auto_edge(vp(400.0, 800.0)), SmallEdge::Bottom);
    }

    #[test]
    fn three_tiles_two_small_one_big() {
        let l = TileLayout::new(vp(400.0, 800.0), SmallEdge::Bottom);
        let p = l.layout(&[-1, 0, 1], 0); // active is big
        assert_eq!(p.len(), 3);
        let big: Vec<_> = p.iter().filter(|t| t.is_big).collect();
        assert_eq!(big.len(), 1, "exactly one big pane");
        assert_eq!(big[0].slot, 0);
        // the two smalls are smaller than the big
        let big_area = big[0].rect.area();
        for t in p.iter().filter(|t| !t.is_big) {
            assert!(t.rect.area() < big_area, "small < big");
        }
    }

    #[test]
    fn panes_do_not_overlap_bottom() {
        let l = TileLayout::new(vp(400.0, 800.0), SmallEdge::Bottom);
        let p = l.layout(&[-1, 0, 1], 0);
        // big occupies the top band; smalls the bottom band, side by side.
        let big = p.iter().find(|t| t.is_big).unwrap();
        for s in p.iter().filter(|t| !t.is_big) {
            // small starts below the big pane (+ gap)
            assert!(s.rect.y >= big.rect.y + big.rect.h, "small below big");
        }
        // the two smalls don't overlap horizontally
        let smalls: Vec<_> = p.iter().filter(|t| !t.is_big).collect();
        let (a, b) = (smalls[0].rect, smalls[1].rect);
        assert!(a.x + a.w <= b.x + 0.01 || b.x + b.w <= a.x + 0.01, "smalls side by side");
    }

    #[test]
    fn panes_do_not_overlap_right() {
        let l = TileLayout::new(vp(1200.0, 700.0), SmallEdge::Right);
        let p = l.layout(&[-1, 0, 1], 0);
        let big = p.iter().find(|t| t.is_big).unwrap();
        for s in p.iter().filter(|t| !t.is_big) {
            assert!(s.rect.x >= big.rect.x + big.rect.w, "small right of big");
        }
    }

    #[test]
    fn big_fraction_respected() {
        let mut l = TileLayout::new(vp(1000.0, 600.0), SmallEdge::Right);
        l.big_fraction = 0.7;
        l.gap = 0.0;
        let p = l.layout(&[-1, 0, 1], 0);
        let big = p.iter().find(|t| t.is_big).unwrap();
        assert!((big.rect.w - 700.0).abs() < 0.5, "big pane ~70% width");
    }

    #[test]
    fn retile_promotes_a_different_tile() {
        let l = TileLayout::new(vp(400.0, 800.0), SmallEdge::Bottom);
        // promote whatever's big to the next slot; layout follows.
        let next = l.next_big(&[-1, 0, 1], 0, true);
        assert_ne!(next, 0, "a different slot becomes big");
        let p = l.layout(&[-1, 0, 1], next);
        assert!(p.iter().find(|t| t.slot == next).unwrap().is_big);
    }

    #[test]
    fn retile_cycles_through_all_and_wraps() {
        let l = TileLayout::new(vp(400.0, 800.0), SmallEdge::Bottom);
        let slots = [-1i8, 0, 1];
        // forward from -1: -1→0→1→-1
        assert_eq!(l.next_big(&slots, -1, true), 0);
        assert_eq!(l.next_big(&slots, 0, true), 1);
        assert_eq!(l.next_big(&slots, 1, true), -1);
        // backward
        assert_eq!(l.next_big(&slots, -1, false), 1);
    }

    #[test]
    fn missing_big_slot_falls_back() {
        let l = TileLayout::new(vp(400.0, 800.0), SmallEdge::Bottom);
        // ask for big_slot +1 when only [-1,0] exist (live position)
        let p = l.layout(&[-1, 0], 1);
        assert_eq!(p.len(), 2);
        assert!(p.iter().any(|t| t.is_big));
    }

    #[test]
    fn single_tile_fills_viewport() {
        let l = TileLayout::new(vp(400.0, 800.0), SmallEdge::Bottom);
        let p = l.layout(&[0], 0);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].rect, vp(400.0, 800.0));
        assert!(p[0].is_big);
    }

    #[test]
    fn empty_slots_empty_layout() {
        let l = TileLayout::new(vp(400.0, 800.0), SmallEdge::Bottom);
        assert!(l.layout(&[], 0).is_empty());
    }

    #[test]
    fn two_tiles_one_big_one_small() {
        let l = TileLayout::new(vp(400.0, 800.0), SmallEdge::Bottom);
        let p = l.layout(&[0, 1], 0); // live: active + next
        assert_eq!(p.len(), 2);
        assert_eq!(p.iter().filter(|t| t.is_big).count(), 1);
        // the one small spans the full small-row width
        let small = p.iter().find(|t| !t.is_big).unwrap();
        assert!((small.rect.w - 400.0).abs() < 0.5);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn vid(id: u64, ts: i64) -> Tile { Tile { id, ts_ms: ts, has_audio_track: true } }
    fn still(id: u64, ts: i64) -> Tile { Tile { id, ts_ms: ts, has_audio_track: false } }

    const NOW: i64 = 1_000_000_000_000;

    fn ids(v: &ComposerView) -> Vec<u64> { v.tiles.iter().map(|t| t.id).collect() }
    fn slots(v: &ComposerView) -> Vec<i8> { v.tiles.iter().map(|t| t.slot).collect() }

    #[test]
    fn empty_feed_is_empty_view() {
        let c = Composer::new(&[], NOW);
        let v = c.live_view(false);
        assert!(v.tiles.is_empty());
        assert_eq!(v.active_index, None);
        assert_eq!(v.window_len, 0);
    }

    #[test]
    fn older_than_24h_is_excluded() {
        let tiles = [
            vid(1, NOW - WINDOW_24H_MS - 1), // just outside
            vid(2, NOW - WINDOW_24H_MS + 1), // just inside
            vid(3, NOW - 1000),
        ];
        let c = Composer::new(&tiles, NOW);
        assert_eq!(c.len(), 2);
        assert_eq!(c.live_view(false).window_len, 2);
    }

    #[test]
    fn tiles_sorted_oldest_to_newest_live_is_newest() {
        // Provided out of order.
        let tiles = [vid(3, NOW - 1000), vid(1, NOW - 3000), vid(2, NOW - 2000)];
        let c = Composer::new(&tiles, NOW);
        let v = c.live_view(false);
        // live = newest active (id 3), prev = id 2; no next (newest).
        assert_eq!(v.active_index, Some(2));
        assert_eq!(ids(&v), vec![2, 3]);
        assert_eq!(slots(&v), vec![-1, 0]);
    }

    #[test]
    fn middle_active_shows_prev_active_next() {
        let tiles = [vid(1, NOW - 4000), vid(2, NOW - 3000), vid(3, NOW - 2000), vid(4, NOW - 1000)];
        let c = Composer::new(&tiles, NOW);
        let v = c.view_at(1, false); // active = id 2
        assert_eq!(ids(&v), vec![1, 2, 3]);
        assert_eq!(slots(&v), vec![-1, 0, 1]);
    }

    #[test]
    fn no_voice_adjacent_tiles_play_muted() {
        let tiles = [vid(1, NOW - 3000), vid(2, NOW - 2000), vid(3, NOW - 1000)];
        let c = Composer::new(&tiles, NOW);
        let v = c.view_at(1, false);
        let pb: Vec<_> = v.tiles.iter().map(|t| (t.slot, t.playback)).collect();
        assert_eq!(pb, vec![
            (-1, PlaybackState::PlayingMuted),
            (0,  PlaybackState::PlayingWithAudio),
            (1,  PlaybackState::PlayingMuted),
        ]);
        assert!(!v.voice_gating);
    }

    #[test]
    fn voice_dims_and_warps_the_other_two() {
        let tiles = [vid(1, NOW - 3000), vid(2, NOW - 2000), vid(3, NOW - 1000)];
        let c = Composer::new(&tiles, NOW);
        let v = c.view_at(1, true); // active id 2 has voice
        let dim = PlaybackState::Dimmed { rate_pct: VOICE_DIM_RATE_PCT };
        let pb: Vec<_> = v.tiles.iter().map(|t| (t.slot, t.playback)).collect();
        assert_eq!(pb, vec![
            (-1, dim),
            (0,  PlaybackState::PlayingWithAudio),
            (1,  dim),
        ]);
        assert!(v.voice_gating);
    }

    #[test]
    fn voice_flag_ignored_when_active_is_a_still() {
        // A still can't have voice; the others should NOT freeze.
        let tiles = [vid(1, NOW - 3000), still(2, NOW - 2000), vid(3, NOW - 1000)];
        let c = Composer::new(&tiles, NOW);
        let v = c.view_at(1, true); // active is the still id 2
        assert!(!v.voice_gating);
        // active still → Still; neighbors play muted (no voice gating).
        let pb: Vec<_> = v.tiles.iter().map(|t| (t.slot, t.playback)).collect();
        assert_eq!(pb, vec![
            (-1, PlaybackState::PlayingMuted),
            (0,  PlaybackState::Still),
            (1,  PlaybackState::PlayingMuted),
        ]);
    }

    #[test]
    fn still_neighbor_is_still_even_under_voice() {
        let tiles = [still(1, NOW - 3000), vid(2, NOW - 2000), vid(3, NOW - 1000)];
        let c = Composer::new(&tiles, NOW);
        let v = c.view_at(1, true);
        // prev is a still → Still (unaffected); next is video → dimmed+warped.
        assert_eq!(v.tiles[0].playback, PlaybackState::Still);
        assert_eq!(v.tiles[2].playback, PlaybackState::Dimmed { rate_pct: VOICE_DIM_RATE_PCT });
    }

    #[test]
    fn scrub_moves_active_and_clamps() {
        let tiles = [vid(1, NOW - 4000), vid(2, NOW - 3000), vid(3, NOW - 2000), vid(4, NOW - 1000)];
        let c = Composer::new(&tiles, NOW);
        let base = c.newest_index().unwrap(); // 3 (id 4)
        // scrub back 2 → active id 2
        assert_eq!(c.scrub(base, -2, false).active_index, Some(1));
        // scrub back 100 → clamps to 0 (id 1)
        assert_eq!(c.scrub(base, -100, false).active_index, Some(0));
        // scrub forward past end → clamps to last
        assert_eq!(c.scrub(0, 100, false).active_index, Some(3));
    }

    #[test]
    fn tap_to_activate_by_id_replays() {
        let tiles = [vid(10, NOW - 3000), vid(20, NOW - 2000), vid(30, NOW - 1000)];
        let c = Composer::new(&tiles, NOW);
        let v = c.activate_id(10, false); // tap the oldest
        assert_eq!(v.active_index, Some(0));
        assert_eq!(v.tiles[0].slot, 0); // it's the active slot now
        assert_eq!(v.tiles[0].playback, PlaybackState::PlayingWithAudio);
    }

    #[test]
    fn tap_vanished_id_falls_back_to_live() {
        let tiles = [vid(10, NOW - 2000), vid(20, NOW - 1000)];
        let c = Composer::new(&tiles, NOW);
        let v = c.activate_id(999, false); // id not present
        assert_eq!(v.active_index, c.newest_index());
    }

    #[test]
    fn single_tile_feed_has_only_active() {
        let c = Composer::new(&[vid(1, NOW - 1000)], NOW);
        let v = c.live_view(false);
        assert_eq!(ids(&v), vec![1]);
        assert_eq!(slots(&v), vec![0]);
    }

    #[test]
    fn out_of_range_active_pins_not_panics() {
        let c = Composer::new(&[vid(1, NOW - 2000), vid(2, NOW - 1000)], NOW);
        let v = c.view_at(999, false); // way past end
        assert_eq!(v.active_index, Some(1)); // pinned to last
    }
}
