//! touch_nav — drive logscope's orbit camera from touchkit gestures.
//!
//! logscope's 3D conversation view is mouse-first (drag to orbit, scroll to zoom). This adopts
//! the shared multitouch recognizer so the same view responds to **pinch-zoom** and
//! **drag-orbit** on the Fairphone/pmOS fleet — the same `touchkit` path every egui app now uses
//! via `uikit_egui::touch`. Pure mapping (no egui): the frontend polls a `TouchSource` and feeds
//! the gestures here. gui-gated because it drives `spatial::OrbitCam`.

use crate::spatial::OrbitCam;
use touchkit::Gesture;

/// Touch feel: radians of orbit per pixel of drag.
const ORBIT_PER_PX: f32 = 0.006;

/// Apply one recognised gesture to the camera. Pinch zooms (spread apart → move closer), a
/// two-plus-finger / one-finger drag orbits yaw+pitch. Swipes/taps are left to the caller (node
/// focus), so this stays a pure camera mapping.
pub fn apply_gesture(cam: &mut OrbitCam, g: &Gesture) {
    match g {
        Gesture::Pinch { rel, .. } => cam.zoom(1.0 / rel.max(0.01)),
        Gesture::Pan { delta, .. } => cam.orbit(delta.x * ORBIT_PER_PX, -delta.y * ORBIT_PER_PX),
        _ => {}
    }
}

/// Apply a batch of gestures (one frame's worth from `TouchSource::poll`).
pub fn apply_all(cam: &mut OrbitCam, gestures: &[Gesture]) {
    for g in gestures {
        apply_gesture(cam, g);
    }
}
