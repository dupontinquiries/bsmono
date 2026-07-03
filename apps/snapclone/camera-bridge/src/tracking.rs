//! Tracking and exponential smoothing of the detected face position.
//!
//! The tracker stores face position in **pixel coordinates** of the capture
//! frame and applies exponential smoothing so the virtual camera pan/zoom
//! doesn't jitter.  When no face has been detected the smoothed position
//! drifts back toward a centred full-frame view.

use log::debug;

/// How fast the smoothed position follows new detections (0 < α ≤ 1).
/// Higher → more responsive; lower → smoother.
const ALPHA: f32 = 0.2;

/// Per-frame interpolation weight toward the full-frame fallback.
const ZOOM_OUT_SPEED: f32 = 0.05;

/// Face position stored in **pixel** coordinates of the capture frame.
/// This is intentionally not normalised — normalisation only makes sense
/// once the frame dimensions are known (in `smoothed_crop`).
#[derive(Debug, Clone, Copy)]
struct PixelBbox {
    /// Horizontal centre of the face, in pixels.
    cx: f32,
    /// Vertical centre of the face, in pixels.
    cy: f32,
    /// Width of the face bounding box, in pixels.
    w: f32,
    /// Height of the face bounding box, in pixels.
    h: f32,
}

impl PixelBbox {
    /// Build a centred, full-frame fallback for a given resolution.
    fn center_full(frame_width: f32, frame_height: f32) -> Self {
        Self {
            cx: frame_width / 2.0,
            cy: frame_height / 2.0,
            w: frame_width,
            h: frame_height,
        }
    }
}

/// Axis-aligned crop rectangle in pixel coordinates.
#[derive(Debug, Clone, Copy)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

pub struct Tracker {
    /// Current smoothed bbox in pixel coords.  `None` until the first
    /// call to `smoothed_crop` (when we finally know the frame size).
    smoothed: Option<PixelBbox>,
    /// `true` while we have a live face detection to track.
    has_face: bool,
}

impl Tracker {
    pub fn new() -> Self {
        Self {
            smoothed: None,
            has_face: false,
        }
    }

    /// Feed the latest detection into the tracker.
    ///
    /// `bbox` — `Some((x_min, y_min, x_max, y_max))` in capture-frame pixel
    /// coordinates, or `None` when inference found no face.
    pub fn update(&mut self, bbox: Option<(f32, f32, f32, f32)>) {
        match bbox {
            Some((x_min, y_min, x_max, y_max)) => {
                let target_cx = (x_min + x_max) / 2.0;
                let target_cy = (y_min + y_max) / 2.0;
                let target_w = (x_max - x_min).abs();
                let target_h = (y_max - y_min).abs();

                if !self.has_face || self.smoothed.is_none() {
                    // No previous tracking state: snap immediately to the detection.
                    self.smoothed = Some(PixelBbox {
                        cx: target_cx,
                        cy: target_cy,
                        w: target_w,
                        h: target_h,
                    });
                } else if let Some(ref mut s) = self.smoothed {
                    // Smooth toward the new detection.
                    s.cx = s.cx * (1.0 - ALPHA) + target_cx * ALPHA;
                    s.cy = s.cy * (1.0 - ALPHA) + target_cy * ALPHA;
                    s.w  = s.w  * (1.0 - ALPHA) + target_w  * ALPHA;
                    s.h  = s.h  * (1.0 - ALPHA) + target_h  * ALPHA;
                }
                self.has_face = true;

                if let Some(s) = &self.smoothed {
                    debug!(
                        "Tracker updated: cx={:.1} cy={:.1} w={:.1} h={:.1}",
                        s.cx, s.cy, s.w, s.h
                    );
                }
            }
            None => {
                self.has_face = false;
                // Keep the last smoothed position; it will drift back to
                // full-frame in `smoothed_crop`.
            }
        }
    }

    /// Compute the crop rectangle to apply to this frame.
    ///
    /// * When a face is tracked the crop is centred on it and padded by
    ///   `zoom_factor` (e.g. 1.5 gives 50% extra context around the face).
    /// * When no face is visible the crop drifts back to the full frame.
    pub fn smoothed_crop(
        &mut self,
        frame_width: u32,
        frame_height: u32,
        zoom_factor: f32,
    ) -> CropRect {
        let fw = frame_width as f32;
        let fh = frame_height as f32;

        // Lazily initialise to the full-frame centre on the first call.
        if self.smoothed.is_none() {
            self.smoothed = Some(PixelBbox::center_full(fw, fh));
        }

        let s = self.smoothed.as_mut().unwrap();

        let (cx, cy, crop_w, crop_h) = if self.has_face {
            // Pad the detected face bbox by zoom_factor.
            (s.cx, s.cy, s.w * zoom_factor, s.h * zoom_factor)
        } else {
            // Drift toward the centred full-frame fallback.
            s.cx = s.cx * (1.0 - ZOOM_OUT_SPEED) + (fw / 2.0) * ZOOM_OUT_SPEED;
            s.cy = s.cy * (1.0 - ZOOM_OUT_SPEED) + (fh / 2.0) * ZOOM_OUT_SPEED;
            s.w = (s.w * (1.0 - ZOOM_OUT_SPEED) + fw * ZOOM_OUT_SPEED).min(fw);
            s.h = (s.h * (1.0 - ZOOM_OUT_SPEED) + fh * ZOOM_OUT_SPEED).min(fh);
            (s.cx, s.cy, s.w, s.h)
        };

        // Clamp to frame bounds.
        let cw = crop_w.min(fw).max(1.0) as u32;
        let ch = crop_h.min(fh).max(1.0) as u32;

        // Compute top-left, keeping the crop fully within the frame.
        let x = ((cx - crop_w / 2.0).max(0.0) as u32)
            .min(frame_width.saturating_sub(cw));
        let y = ((cy - crop_h / 2.0).max(0.0) as u32)
            .min(frame_height.saturating_sub(ch));

        CropRect { x, y, width: cw, height: ch }
    }
}
