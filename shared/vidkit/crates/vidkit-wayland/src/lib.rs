//! How frames actually reach the screen, and hints to the compositor about
//! what they are.
//!
//! Two layers:
//! - [`cadence`]: pure. Presentation outcomes in, a model of the display out
//!   (refresh, variable or fixed, how many refreshes each frame was held,
//!   lateness against the time it was meant for, discards). No Wayland.
//! - [`surface`]: the Wayland side. Binds `wp_presentation`,
//!   `wp_content_type_v1` and `wp_commit_timing_v1` for a `wl_surface`
//!   owned by someone else (winit, via eframe's raw handles), on its own
//!   event queue, and turns feedback events into [`cadence::Outcome`]s.
//!
//! Why: the player could only see when it *asked* for a frame, not when the
//! compositor showed it. Refresh matching (and on a VRR output, presenting
//! at the frame's own time instead of the next fixed vblank) needs the
//! second. The feedback is also what says whether a video's 24 fps is being
//! shown as a steady 3:2 on 60 Hz or with the odd 4 -- visible judder.

pub mod cadence;
pub mod surface;
