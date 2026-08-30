//! scroll_edge — what a panel's scroll area did, for over-scroll nav.
//!
//! The shell owns Pages-mode navigation (`studio_egui::touch::PanelNav`),
//! but only each panel knows its own scrollbars — so each panel's `show`
//! returns a [`ScrollReport`] and the shell gates the gesture on it. The
//! division of labour is the one the survey in C12_TODO §14g concluded
//! was required: no panel leaves an axis free, so "the inner view is at
//! its limit" must come from the ScrollArea itself, not from guessing at
//! unconsumed input.
//!
//! The gating logic is a pure function ([`classify`]) because it is the
//! part that decides whether normal scrolling can ever masquerade as an
//! over-scroll — the failure mode wiring was twice deferred over. The
//! rule that structurally excludes it: any frame the offset MOVED is
//! normal scrolling, full stop, whatever the deltas say. Only a pinned
//! offset at a limit, with the wheel still pulling past it, counts.
//!
//! Honestly unverified (no display here): whether egui 0.34 leaves
//! `smooth_scroll_delta` readable alongside a ScrollArea consuming it.
//! If it does not, the failure is the SAFE direction — over-scroll never
//! fires and the compact picker remains the route between panels — never
//! the unsafe one, because `moved` wins regardless.

/// One frame of a panel's main scroll area, from the panel that owns it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScrollReport {
    /// The offset changed this frame — normal scrolling. The nav streak
    /// resets so the next edge-pull starts clean.
    pub moved: bool,
    /// Signed pull past a vertical limit, normalized for the nav's
    /// convention: positive = past the BOTTOM (toward the next panel),
    /// negative = past the top. `0.0` when this frame was not an
    /// edge-pull.
    pub overscroll: f32,
}

/// The decision, separated from egui so it can be pinned by test.
/// `raw_y` is egui's wheel convention: negative = scrolling down.
pub fn classify(raw_y: f32, moved: bool, hovered: bool,
                at_top: bool, at_bottom: bool) -> ScrollReport {
    let overscroll = if moved || !hovered || raw_y == 0.0 {
        0.0
    } else if raw_y < 0.0 && at_bottom {
        -raw_y // pulling down past the bottom: toward the next panel
    } else if raw_y > 0.0 && at_top {
        -raw_y // pulling up past the top: toward the previous
    } else {
        0.0
    };
    ScrollReport { moved, overscroll }
}

/// Wrap a `ScrollAreaOutput` into a report. Frame-to-frame offset memory
/// rides egui's temp data under the area's own id, so two areas on screen
/// never share a previous-offset.
pub fn report(ui: &egui::Ui, out: &egui::scroll_area::ScrollAreaOutput<()>) -> ScrollReport {
    let raw_y = ui.input(|i| i.smooth_scroll_delta.y);
    let offset = out.state.offset.y;
    let max = (out.content_size.y - out.inner_rect.height()).max(0.0);
    let id = out.id.with("scroll-edge-prev");
    let prev: f32 = ui.ctx().data_mut(|d| *d.get_temp_mut_or(id, offset));
    ui.ctx().data_mut(|d| d.insert_temp(id, offset));
    let hovered = ui.rect_contains_pointer(out.inner_rect);
    classify(raw_y, (offset - prev).abs() > 0.1, hovered,
             offset <= 0.5, offset >= max - 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The structural guarantee: a frame where the area moved is normal
    /// scrolling no matter what else is true — this is the rule that
    /// makes blind-wiring's failure mode (panels jumping mid-scroll)
    /// impossible rather than merely unlikely.
    #[test]
    fn movement_always_wins() {
        let r = classify(-30.0, true, true, false, true);
        assert!(r.moved);
        assert_eq!(r.overscroll, 0.0, "moved: never an over-scroll, even at a limit");
    }

    /// The four gate conditions, each individually necessary.
    #[test]
    fn overscroll_needs_pin_hover_delta_and_the_matching_edge() {
        // All conditions met, pulling down at the bottom: positive (next).
        assert_eq!(classify(-30.0, false, true, false, true).overscroll, 30.0);
        // Pulling up at the top: negative (previous).
        assert_eq!(classify(20.0, false, true, true, false).overscroll, -20.0);
        // Wrong edge for the direction: nothing (down-pull at the top is
        // just the start of a scroll the area will consume next frame).
        assert_eq!(classify(-30.0, false, true, true, false).overscroll, 0.0);
        assert_eq!(classify(20.0, false, true, false, true).overscroll, 0.0);
        // Not hovered: someone else's wheel.
        assert_eq!(classify(-30.0, false, false, false, true).overscroll, 0.0);
        // No delta: nothing to classify.
        assert_eq!(classify(0.0, false, true, false, true).overscroll, 0.0);
    }

    /// A short content pane is at BOTH limits at once — over-scroll must
    /// still work there in both directions, because a phone-height mixer
    /// with three faders is exactly the pane you page away from.
    #[test]
    fn a_pane_with_no_scroll_room_pages_both_ways() {
        assert_eq!(classify(-15.0, false, true, true, true).overscroll, 15.0);
        assert_eq!(classify(15.0, false, true, true, true).overscroll, -15.0);
    }
}
