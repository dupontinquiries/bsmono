//! preview — a viewfinder for snapshot-cam.
//!
//! ## What this is modelled on
//!
//! snapclone's `camera::Viewfinder`, which already solved the two things
//! that matter: aspect handling that does not distort the frame, and a
//! front camera that is MIRRORED so the preview matches the mirror the
//! subject is used to.
//!
//! Reimplemented rather than imported, and the reason is the dependency
//! graph rather than taste: snapshot-cam does not depend on
//! snapclone-egui, and adding an app-to-app dependency for two geometry
//! functions would tie a small self-contained tool to a large one. What
//! is shared is the RULE, and the rule is short enough to state in a
//! test.
//!
//! ## Why the preview shows a placeholder
//!
//! `main.rs` says it plainly: "the capture itself is a stub: wiring a
//! real frame means…". There is no frame source in this app yet. A
//! preview that drew nothing would be indistinguishable from a preview
//! that is broken, so this draws the FRAME the picture will occupy, at
//! the right aspect, with the state written on it.
//!
//! That is the honest version of a stub: it shows what the geometry will
//! be, and says why there is no picture in it.

use egui::{Color32, Pos2, Rect, Vec2};

/// Which way the camera points. Only the mirror depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Facing {
    #[default]
    Front,
    Rear,
}

impl Facing {
    /// A front camera is mirrored, because the subject is looking at
    /// themselves. A rear camera is not, because they are looking at the
    /// world and a mirrored world is wrong.
    pub fn mirrored(self) -> bool {
        self == Facing::Front
    }
}

/// How the frame is placed in the space available.
/// Default is 16:9 rather than derived, because a derived `Default`
/// gives `sensor_aspect: 0.0` — which `image_rect` clamps to 0.01 and
/// draws as a sliver. A viewfinder that defaults to a broken shape is
/// worse than one that defaults to a common one.
#[derive(Debug, Clone, Copy)]
pub struct Preview {
    pub facing: Facing,
    /// Crop to fill (true) or letterbox to preserve the whole frame.
    pub full_bleed: bool,
    /// Sensor aspect, width / height.
    pub sensor_aspect: f32,
}

impl Default for Preview {
    fn default() -> Self {
        Preview { facing: Facing::Front, full_bleed: false, sensor_aspect: 16.0 / 9.0 }
    }
}

impl Preview {
    pub fn new(sensor_aspect: f32) -> Preview {
        Preview { facing: Facing::Front, full_bleed: false, sensor_aspect }
    }

    /// The rect the image occupies inside `into`.
    ///
    /// Never distorts: the frame is either letterboxed (whole frame
    /// visible, bars at the sides) or cropped to fill (no bars, edges
    /// lost). Stretching to fit is the third option and is not offered,
    /// because a preview that lies about proportions makes framing a
    /// photo impossible — which is the one job this window has.
    pub fn image_rect(&self, into: Rect) -> Rect {
        // Clamped away from zero: a zero-height panel during a layout
        // pass would divide by zero and produce a NaN rect, which egui
        // paints as nothing at all — an invisible failure in the widget
        // whose entire purpose is being visible.
        let want = self.sensor_aspect.max(0.01);
        let have = (into.width() / into.height().max(1.0)).max(0.01);
        let fill_wider = have > want;
        if self.full_bleed == fill_wider {
            // Either (fill and the box is wider) or (letterbox and the
            // box is taller): height is the constrained dimension.
            let h = into.width() / want;
            Rect::from_center_size(into.center(), Vec2::new(into.width(), h))
        } else {
            let w = into.height() * want;
            Rect::from_center_size(into.center(), Vec2::new(w, into.height()))
        }
    }

    /// Map a screen point to normalised sensor coordinates (0..1).
    ///
    /// Normalised so a tap survives a resize or a rotation, and mirrored
    /// for the front camera — otherwise tapping the left of the screen
    /// would focus the right of the sensor.
    ///
    /// `None` outside the image, which is a real case when letterboxed:
    /// a tap on the bar beside the picture is not a tap on the picture.
    pub fn to_sensor(&self, p: Pos2, into: Rect) -> Option<[f32; 2]> {
        let r = self.image_rect(into);
        if !r.contains(p) {
            return None;
        }
        let mut x = (p.x - r.left()) / r.width().max(0.01);
        let y = (p.y - r.top()) / r.height().max(0.01);
        if self.facing.mirrored() {
            x = 1.0 - x;
        }
        Some([x, y])
    }
}

/// Draw the preview and return the rect the image occupies.
///
/// `status` is written into the frame rather than beside it: with no
/// camera attached the frame IS the message, and a caption under an
/// empty box reads as a caption under a broken box.
pub fn show(ui: &mut egui::Ui, p: &Preview, status: &str) -> Rect {
    let avail = ui.available_rect_before_wrap();
    // A square-ish budget rather than all remaining height: this sits
    // above the roll and the controls, and taking everything would push
    // the shutter off a phone screen.
    let budget = Rect::from_min_size(
        avail.min,
        Vec2::new(avail.width(), (avail.width() / p.sensor_aspect.max(0.01)).min(avail.height() * 0.6)),
    );
    let r = p.image_rect(budget);
    let painter = ui.painter();
    painter.rect_filled(r, 6.0, Color32::from_gray(18));
    painter.rect_stroke(r, 6.0, egui::Stroke::new(1.0, Color32::from_gray(60)),
                        egui::StrokeKind::Inside);
    painter.text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        status,
        egui::FontId::proportional(13.0),
        Color32::from_gray(140),
    );
    ui.advance_cursor_after_rect(budget);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, Rect, Vec2};

    fn box_of(w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(w, h))
    }

    #[test]
    fn the_default_aspect_is_usable_not_zero() {
        // A derived Default gives 0.0, which `image_rect` clamps to 0.01
        // and draws as a sliver — a viewfinder that defaults to a broken
        // shape. The app relies on this default.
        let p = Preview::default();
        assert!(p.sensor_aspect > 1.0, "{}", p.sensor_aspect);
        let r = p.image_rect(box_of(400.0, 400.0));
        assert!(r.width() > 100.0 && r.height() > 50.0, "{r:?}");
    }

    #[test]
    fn letterboxing_never_distorts_the_frame() {
        // The one job this window has is showing what the photo will
        // look like. A stretched preview makes framing impossible.
        let p = Preview::new(4.0 / 3.0);
        for (w, h) in [(400.0, 400.0), (800.0, 200.0), (200.0, 800.0)] {
            let r = p.image_rect(box_of(w, h));
            let got = r.width() / r.height();
            assert!((got - 4.0 / 3.0).abs() < 0.01, "{w}x{h} gave aspect {got}");
        }
    }

    #[test]
    fn letterbox_fits_inside_and_full_bleed_covers() {
        let into = box_of(400.0, 400.0);
        let letter = Preview { full_bleed: false, ..Preview::new(16.0 / 9.0) };
        let r = letter.image_rect(into);
        assert!(r.width() <= into.width() + 0.1 && r.height() <= into.height() + 0.1,
                "letterbox escaped its box: {r:?}");

        let fill = Preview { full_bleed: true, ..Preview::new(16.0 / 9.0) };
        let r = fill.image_rect(into);
        assert!(r.width() >= into.width() - 0.1 && r.height() >= into.height() - 0.1,
                "full bleed left a gap: {r:?}");
    }

    #[test]
    fn a_zero_sized_box_does_not_produce_a_nan_rect() {
        // A layout pass can hand a widget a zero-height rect. NaN there
        // paints as nothing, which is an invisible failure in the one
        // widget whose purpose is being visible.
        let p = Preview::new(4.0 / 3.0);
        let r = p.image_rect(box_of(0.0, 0.0));
        assert!(r.width().is_finite() && r.height().is_finite(), "{r:?}");
    }

    #[test]
    fn the_front_camera_is_mirrored_and_the_rear_is_not() {
        // Tapping the left of a mirrored preview must focus the right of
        // the sensor, or focus lands on the wrong side of the frame.
        let into = box_of(400.0, 300.0);
        let front = Preview { facing: Facing::Front, ..Preview::new(4.0 / 3.0) };
        let rear = Preview { facing: Facing::Rear, ..Preview::new(4.0 / 3.0) };
        let left_edge = pos2(into.left() + 1.0, into.center().y);

        let f = front.to_sensor(left_edge, into).expect("inside the image");
        let r = rear.to_sensor(left_edge, into).expect("inside the image");
        assert!(f[0] > 0.9, "front: left of screen should be right of sensor, got {}", f[0]);
        assert!(r[0] < 0.1, "rear: left of screen is left of sensor, got {}", r[0]);
    }

    #[test]
    fn a_tap_on_the_letterbox_bar_is_not_a_tap_on_the_picture() {
        // Real case: a 16:9 sensor in a square box leaves bars top and
        // bottom, and a tap there would otherwise focus a point that is
        // not in frame.
        let into = box_of(400.0, 400.0);
        let p = Preview::new(16.0 / 9.0);
        assert!(p.to_sensor(pos2(200.0, 2.0), into).is_none(), "top bar counted as image");
        assert!(p.to_sensor(into.center(), into).is_some(), "the centre is the image");
    }

    #[test]
    fn sensor_coordinates_stay_normalised_across_a_resize() {
        // The point of normalising: a focus point set on a phone screen
        // means the same thing after a rotation.
        let p = Preview::new(4.0 / 3.0);
        let small = box_of(200.0, 150.0);
        let large = box_of(800.0, 600.0);
        let a = p.to_sensor(small.center(), small).unwrap();
        let b = p.to_sensor(large.center(), large).unwrap();
        assert!((a[0] - b[0]).abs() < 0.001 && (a[1] - b[1]).abs() < 0.001, "{a:?} vs {b:?}");
    }
}
