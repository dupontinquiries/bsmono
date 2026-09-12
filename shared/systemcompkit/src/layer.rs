//! layer — the pure half of `wlr-layer-shell-unstable-v1`: what to ask for
//! (`LayerSurfaceIntent`), and tracking the one protocol subtlety that's
//! easy to get quietly wrong (`LayerSurfaceState`).
//!
//! ## Why `apply` returns the serial rather than acking anything itself
//!
//! The protocol's own contract: a `configure` event must be answered with
//! `ack_configure(serial)` using THAT event's serial, and a compositor may
//! withhold committing new surface content until it sees the ack. Getting
//! this wrong (acking the wrong serial, or forgetting entirely) doesn't
//! error — it just means the surface never quite appears, or appears at
//! the wrong size, which is a miserable thing to debug against a real
//! compositor with no signal pointing at the cause. `apply` hands the
//! serial back explicitly so a caller cannot silently drop it — there is no
//! path through this API that receives a `Configure` and has nowhere to put
//! the serial it owes back to the compositor.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnchorEdges {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

impl AnchorEdges {
    /// Anchored to all four edges, the common shape for a fullscreen-ish
    /// overlay (a lock screen, an app-switcher) that should track output
    /// resize rather than requesting a fixed size.
    pub const FILL: AnchorEdges = AnchorEdges { top: true, bottom: true, left: true, right: true };
    /// A bottom panel/dock: anchored to the bottom edge and both sides so
    /// its width tracks the output, height is the caller's own concern via
    /// `LayerSurfaceIntent::size`.
    pub const BOTTOM_BAR: AnchorEdges = AnchorEdges { top: false, bottom: true, left: true, right: true };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Background,
    Bottom,
    Top,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardInteractivity {
    None,
    Exclusive,
    OnDemand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerSurfaceIntent {
    pub layer: Layer,
    pub anchor: AnchorEdges,
    /// Reserves this many logical pixels along the anchored edge so other
    /// windows don't overlap it — the mechanism a panel uses to claim
    /// screen space permanently. `0` (or `-1` for "don't reserve/track
    /// automatically" per the protocol) for a surface that should overlay
    /// rather than displace, like a lock screen or an app-switcher overlay.
    pub exclusive_zone: i32,
    /// `0` on either axis means "let the compositor decide", which is what
    /// `AnchorEdges::FILL` normally wants — asking for a fixed size while
    /// anchored to all four edges is a contradiction the compositor has to
    /// resolve somehow, usually not the way the caller meant.
    pub size: (u32, u32),
    pub keyboard_interactivity: KeyboardInteractivity,
}

impl LayerSurfaceIntent {
    /// A lock screen or app-switcher overlay: overlay layer (above
    /// everything, including other layer-shell surfaces), fills the
    /// output, takes no permanent space (exclusive_zone 0), and wants
    /// keyboard focus on demand rather than grabbing it unconditionally
    /// the moment it appears.
    pub fn fullscreen_overlay() -> LayerSurfaceIntent {
        LayerSurfaceIntent {
            layer: Layer::Overlay,
            anchor: AnchorEdges::FILL,
            exclusive_zone: 0,
            size: (0, 0),
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
        }
    }

    /// A bottom dock/panel of a given height in logical pixels, reserving
    /// exactly that much space so other windows never sit under it.
    pub fn bottom_panel(height: u32) -> LayerSurfaceIntent {
        LayerSurfaceIntent {
            layer: Layer::Top,
            anchor: AnchorEdges::BOTTOM_BAR,
            exclusive_zone: height as i32,
            size: (0, height),
            keyboard_interactivity: KeyboardInteractivity::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerEvent {
    Configure { serial: u32, width: u32, height: u32 },
    Closed,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LayerSurfaceState {
    pub configured_size: Option<(u32, u32)>,
    pub closed: bool,
}

impl LayerSurfaceState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one event. Returns `Some(serial)` when the caller now owes
    /// the compositor an `ack_configure(serial)` — every `Configure`, and
    /// only a `Configure`; a `Closed` surface has nothing left to ack.
    pub fn apply(&mut self, event: LayerEvent) -> Option<u32> {
        match event {
            LayerEvent::Configure { serial, width, height } => {
                self.configured_size = Some((width, height));
                Some(serial)
            }
            LayerEvent::Closed => {
                self.closed = true;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configure_event_always_hands_back_its_own_serial_to_ack() {
        let mut s = LayerSurfaceState::new();
        assert_eq!(s.apply(LayerEvent::Configure { serial: 42, width: 800, height: 480 }), Some(42));
        assert_eq!(s.configured_size, Some((800, 480)));
    }

    #[test]
    fn a_later_configure_updates_size_and_still_returns_its_own_serial() {
        // A real compositor reconfigures on rotation/output change — the
        // caller must ack EVERY one it receives, not just the first.
        let mut s = LayerSurfaceState::new();
        s.apply(LayerEvent::Configure { serial: 1, width: 800, height: 480 });
        assert_eq!(s.apply(LayerEvent::Configure { serial: 2, width: 480, height: 800 }), Some(2));
        assert_eq!(s.configured_size, Some((480, 800)), "the LATEST configure wins, not the first");
    }

    #[test]
    fn closed_sets_the_flag_and_has_no_serial_to_ack() {
        let mut s = LayerSurfaceState::new();
        assert_eq!(s.apply(LayerEvent::Closed), None);
        assert!(s.closed);
    }

    #[test]
    fn fullscreen_overlay_fills_all_edges_and_reserves_no_space() {
        let intent = LayerSurfaceIntent::fullscreen_overlay();
        assert_eq!(intent.anchor, AnchorEdges::FILL);
        assert_eq!(intent.exclusive_zone, 0, "an overlay must not permanently displace other windows");
        assert_eq!(intent.layer, Layer::Overlay);
    }

    #[test]
    fn bottom_panel_reserves_exactly_its_own_height() {
        let intent = LayerSurfaceIntent::bottom_panel(48);
        assert_eq!(intent.exclusive_zone, 48, "the reserved space must match the panel's own requested height");
        assert_eq!(intent.size, (0, 48), "width 0 tracks the output; height is fixed");
        assert!(intent.anchor.bottom && intent.anchor.left && intent.anchor.right && !intent.anchor.top);
    }
}
