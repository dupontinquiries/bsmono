//! Tell an e-ink runtime which part of the screen is the moving picture.
//!
//! eink-kit (shared/eink-kit) classifies continuously-changing regions as
//! motion and refreshes them mono, dithered and rate-capped, returning to
//! colour when they stop. It detects that on its own; declaring the picture
//! skips the detection warm-up so the first frames are already handled as
//! video, and keeps the letterbox bars and controls out of it.
//!
//! The protocol is plain egui memory so this crate does not depend on
//! eink-kit: on a desktop nothing reads the key and this is a no-op. The key
//! and value type must match `eink_kit::motion::{HINTS_KEY, BUDGET_KEY}`;
//! eink-kit's `hints_round_trip_through_plain_egui_memory` test pins them.

use egui::{Context, Id, Rect};
use std::time::Duration;

const HINTS_KEY: &str = "eink.hints";
const BUDGET_KEY: &str = "eink.motion_interval";
const HINT_VIDEO: u8 = 1;

/// Declare `rect` (egui points) as the video picture for this frame.
pub fn hint_video(ctx: &Context, rect: Rect) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<(Rect, u8)>>(Id::new(HINTS_KEY)).push((rect, HINT_VIDEO)));
}

/// How often an e-ink panel will actually show a new frame of a moving
/// picture, or `None` on an ordinary display. A decoder that honours this
/// can skip decoding frames nobody will see (eink-kit already paces UI
/// frames, and so conversion/upload, to this interval).
pub fn panel_frame_interval(ctx: &Context) -> Option<Duration> {
    ctx.data(|d| d.get_temp::<f32>(Id::new(BUDGET_KEY))).map(Duration::from_secs_f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_has_no_panel_interval_and_hints_are_harmless() {
        let ctx = Context::default();
        assert_eq!(panel_frame_interval(&ctx), None);
        hint_video(&ctx, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(10.0, 10.0)));
        let stored = ctx.data(|d| d.get_temp::<Vec<(Rect, u8)>>(Id::new(HINTS_KEY))).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].1, HINT_VIDEO);
    }
}
