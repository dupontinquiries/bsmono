//! ════════════════════════════════════════════════════════════════════════════
//!  ILLUSTRATIVE SKETCH — NOT COMPILED here (needs egui 0.34 / uikit-egui).
//!  Every decision it wires is compiled + tested: ink-kit `air` (AirPointer,
//!  Dwell), touchkit `LongPress`, uikit-egui `menu` (HoldTracker, ContextMenu
//!  — both pure, tests in-crate).
//! ════════════════════════════════════════════════════════════════════════════
//!
//! Universal context-menu gestures: ONE menu invocation, five ways in.
//!
//! ```text
//!   mouse right-click ─────────────┐          (uikit_egui::menu::invoked)
//!   touch / pen long-press ────────┤
//!   hand pinch-hold ───────────────┤──► MenuIntent(pos) ──► ContextMenu::open
//!   gaze dwell (stage 2) ──────────┤          + menu_ui each frame
//!   glove: latch + "menu" voice ───┘
//! ```
//!
//! menu.rs's law — the row asks *"was I invoked?"* — extends unchanged: the
//! new modalities become additional producers of the same `Option<(f32,f32)>`
//! that `invoked()` already returns. No surface grows a second menu.
//!
//! Per-modality mapping and why each is what it is:
//! * **Mouse / touch / stylus** — `menu::invoked()` as-is (right-click OR
//!   HoldTracker hold with movement-cancels rule). Nothing new.
//! * **Hand (air)** — a pinch that *rests* is a hold. The pinch synthesizes the
//!   contact (ink-kit `AirPointer`), so we feed the same `HoldTracker`:
//!   press on engage, moved on motion, tick each frame. The slop check reuses
//!   `HOLD_SLOP_PX` semantics but the threshold should be wider (~3×): a
//!   tracked fingertip at arm's length jitters more px than a resting thumb.
//!   A pinch-hold that fires the menu must ALSO cancel the in-flight ink
//!   stroke (send the router a frame without the contact, or eat the stroke on
//!   `InkEvent::End` if shorter than slop) — otherwise every menu leaves a dot.
//! * **Gaze** — cannot press; `Dwell` stage 2 (`DwellEvent::Menu`) is the
//!   long-press twin. Stage 1 (`Click`) maps to primary activate. The
//!   `Dwell::progress()` fuse is painted as a shrinking ring at the gaze
//!   point — dwell without visible progress feels like the UI acting on its
//!   own.
//! * **Glove** — glove.rs already snaps its ratchet ball to a target and acts
//!   via voice `Vocab`; "menu" joins the vocabulary and opens at the *latched
//!   target's* rect center (not the ball position — same reason glove snaps).
//!
//! Menu NAVIGATION once open is already universal: ContextMenu's cursor moves
//! by Up/Down/Enter/Escape, and every modality can produce those four —
//! keyboard directly, voice via Vocab, gaze by dwell-clicking entries, hand by
//! pointing. Entries are large-hit-area rows, which air pointers need anyway.

use ink_kit::air::{AirPointer, Dwell, DwellEvent};
use ink_kit::Vec2f;
use uikit_egui::menu::{self, ContextMenu, HoldTracker, MenuItem};

/// The single funnel. One per surface that owns a context menu.
pub struct UniversalMenu {
    pub menu: ContextMenu,
    /// touch/pen/mouse hold (menu.rs path)
    hold: HoldTracker,
    /// hand pinch-hold — a second tracker with air-scale slop
    air_hold: HoldTracker,
    /// gaze two-stage dwell (ink-kit)
    dwell: Dwell,
}

pub enum MenuIntent {
    Open(Vec2f),
    /// gaze stage-1 / hand tap → forward as a primary activation
    Primary(Vec2f),
}

impl UniversalMenu {
    /// Call once per frame per surface. `resp` is the surface's egui Response;
    /// `hand`/`gaze` are this frame's air states (None if modality absent).
    pub fn update(
        &mut self,
        resp: &egui::Response,
        hand: Option<&AirPointer>,   // + engaged flag from pinch, see below
        hand_engaged: bool,
        gaze: Option<Vec2f>,          // gaze hover point in screen px
        dt_ms: f32,
        t_ms: f64,
    ) -> Option<MenuIntent> {
        // 1) classic path: right-click or contact long-press
        if let Some((x, y)) = menu::invoked(resp, &mut self.hold, dt_ms) {
            return Some(MenuIntent::Open(Vec2f::new(x, y)));
        }

        // 2) hand pinch-hold: engage=press, disengage=release, motion=moved
        if let Some(ap) = hand {
            match (hand_engaged, ap.hover()) {
                (true, Some(p)) => {
                    if !self.air_hold.is_down() {
                        self.air_hold.press(p.x, p.y);
                    } else {
                        self.air_hold.moved(p.x, p.y);
                    }
                    if self.air_hold.tick(dt_ms) {
                        return Some(MenuIntent::Open(p));
                        // caller must also cancel the live ink stroke here
                    }
                }
                _ => self.air_hold.release_discard(), // = release(), ignore tap
            }
        }

        // 3) gaze dwell
        if let Some(ev) = self.dwell.update(gaze, t_ms) {
            return Some(match ev {
                DwellEvent::Menu(p) => MenuIntent::Open(p),
                DwellEvent::Click(p) => MenuIntent::Primary(p),
            });
        }
        None
    }

    /// The per-frame draw: open menu if an intent arrived, then render.
    pub fn frame(
        &mut self,
        ui: &mut egui::Ui,
        intent: Option<MenuIntent>,
        items: impl FnOnce() -> Vec<MenuItem>,
        colors: &uikit_egui::PaletteColors,
    ) -> Option<String> {
        if let Some(MenuIntent::Open(p)) = intent {
            self.menu.open(items(), (p.x, p.y));
        }
        menu::menu_ui(ui, &mut self.menu, colors)
    }
}

// Ink-canvas item set, shared by voiceterm panel and (later) CADscope — the
// menu *contents* being one function is the other half of "universal":
//   erase stroke · change tool · recognize → niftyplot · save to note · clear
//
// NOTE small API gap found while sketching: HoldTracker has release() -> bool
// (was-it-a-tap) and cancel(); the hand path above wants release-and-discard,
// written as release_discard() — in real wiring just call release() and drop
// the bool, no menu.rs change needed.
