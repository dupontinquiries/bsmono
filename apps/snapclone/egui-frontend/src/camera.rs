//! camera — the viewfinder, ported from `CameraWidget`.
//!
//! The port had a nav bar and nothing behind it: no preview, no message list,
//! no composer. This is the first of the two surfaces that make the app read as
//! snapclone rather than as a shell.
//!
//! What the Qt widget actually contracts (include/ui/CameraWidget.hpp), and what
//! is mirrored here:
//!
//! * **Tap-to-focus with a reticle** — `handleViewfinderTap` / `FocusReticle`.
//!   A tap sets a focus point, shows a reticle that fades, and the fade is what
//!   tells the user the tap registered at all.
//! * **Full-bleed vs framed** — `setFullBleed`. Full-bleed crops to fill the
//!   viewport; framed letterboxes to preserve the sensor's aspect. Getting this
//!   wrong silently crops people out of their own photos.
//! * **Scan mode** — `setScanMode` throttles frame emission for the QR scanner,
//!   because a scanner does not need 30fps and a phone's battery says so.
//! * **Facing flip** — `flipCameraFacing`, which must also MIRROR the preview,
//!   since a front camera that is not mirrored looks wrong to everyone who has
//!   ever used a mirror.
//!
//! The geometry is pure and tested; the drawing is a thin pass over it.

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2};

/// Which way the camera points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Facing {
    #[default]
    Back,
    Front,
}

impl Facing {
    /// A front camera preview must be mirrored. Everyone has used a mirror;
    /// nobody has used an un-mirrored front camera and thought it looked right.
    pub fn mirrored(self) -> bool { matches!(self, Facing::Front) }
    pub fn flipped(self) -> Facing {
        match self { Facing::Back => Facing::Front, Facing::Front => Facing::Back }
    }
}

/// Viewfinder state.
#[derive(Debug, Clone)]
pub struct Viewfinder {
    pub facing: Facing,
    /// Crop to fill (true) or letterbox to preserve aspect (false).
    pub full_bleed: bool,
    /// Throttled frames for the QR scanner.
    pub scan_mode: bool,
    /// Sensor aspect (width / height).
    pub sensor_aspect: f32,
    /// Focus point in NORMALISED sensor coordinates, so it survives a resize.
    pub focus: Option<[f32; 2]>,
    /// Seconds since the tap, for the reticle fade.
    pub focus_age: f32,
    pub recipient: Option<String>,
}

impl Default for Viewfinder {
    fn default() -> Self {
        Viewfinder {
            facing: Facing::Back, full_bleed: true, scan_mode: false,
            sensor_aspect: 4.0 / 3.0, focus: None, focus_age: 0.0, recipient: None,
        }
    }
}

/// How long the focus reticle stays visible. Long enough to see, short enough
/// not to sit over the shot.
pub const RETICLE_SECONDS: f32 = 1.2;

/// Frames per second in scan mode. A QR scanner does not need thirty, and a
/// phone battery has opinions about it.
pub const SCAN_FPS: f32 = 8.0;

impl Viewfinder {
    /// Where the sensor image lands inside `into`.
    ///
    /// Full-bleed crops to fill; framed letterboxes. Silently choosing the wrong
    /// one crops people out of their own photos, which is why it is tested.
    pub fn image_rect(&self, into: Rect) -> Rect {
        let want = self.sensor_aspect.max(0.01);
        let have = (into.width() / into.height().max(1.0)).max(0.01);
        if self.full_bleed {
            // fill: the smaller dimension is covered, the larger overflows
            if have > want {
                let h = into.width() / want;
                Rect::from_center_size(into.center(), Vec2::new(into.width(), h))
            } else {
                let w = into.height() * want;
                Rect::from_center_size(into.center(), Vec2::new(w, into.height()))
            }
        } else if have > want {
            let w = into.height() * want;
            Rect::from_center_size(into.center(), Vec2::new(w, into.height()))
        } else {
            let h = into.width() / want;
            Rect::from_center_size(into.center(), Vec2::new(into.width(), h))
        }
    }

    /// Screen point -> normalised sensor coordinates, honouring the mirror.
    ///
    /// Normalised, so a focus point survives a rotation or a resize; mirrored
    /// for the front camera, or tapping the left of the screen focuses the right
    /// of the sensor.
    pub fn to_sensor(&self, p: Pos2, into: Rect) -> Option<[f32; 2]> {
        let r = self.image_rect(into);
        if r.width() <= 0.0 || r.height() <= 0.0 { return None; }
        let mut x = (p.x - r.min.x) / r.width();
        let y = (p.y - r.min.y) / r.height();
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) { return None; }
        if self.facing.mirrored() { x = 1.0 - x; }
        Some([x, y])
    }

    /// The inverse, for drawing the reticle where the user tapped.
    pub fn to_screen(&self, s: [f32; 2], into: Rect) -> Pos2 {
        let r = self.image_rect(into);
        let x = if self.facing.mirrored() { 1.0 - s[0] } else { s[0] };
        Pos2::new(r.min.x + x * r.width(), r.min.y + s[1] * r.height())
    }

    pub fn tap(&mut self, p: Pos2, into: Rect) -> bool {
        match self.to_sensor(p, into) {
            Some(s) => { self.focus = Some(s); self.focus_age = 0.0; true }
            // A tap outside the image must NOT move focus — on a letterboxed
            // preview the bars are not part of the picture.
            None => false,
        }
    }

    pub fn tick(&mut self, dt: f32) {
        if self.focus.is_some() {
            self.focus_age += dt;
            if self.focus_age > RETICLE_SECONDS { self.focus = None; }
        }
    }

    /// Reticle opacity 0..1, or 0 when there is nothing to show.
    pub fn reticle_alpha(&self) -> f32 {
        match self.focus {
            None => 0.0,
            Some(_) => (1.0 - self.focus_age / RETICLE_SECONDS).clamp(0.0, 1.0),
        }
    }

    /// Seconds between emitted frames.
    pub fn frame_interval(&self) -> f32 {
        if self.scan_mode { 1.0 / SCAN_FPS } else { 1.0 / 30.0 }
    }

    pub fn flip(&mut self) {
        self.facing = self.facing.flipped();
        // A focus point belongs to the sensor that set it; keeping it across a
        // flip focuses somewhere the user never tapped.
        self.focus = None;
    }
}

/// Draw the viewfinder. `frame` is the latest camera image, if any.
pub fn viewfinder(ui: &mut Ui, vf: &mut Viewfinder, frame: Option<&egui::TextureHandle>,
                  colors: &uikit_egui::PaletteColors) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, Color32::from_rgb(8, 9, 12));
    let img = vf.image_rect(rect);
    match frame {
        Some(t) => {
            let uv = if vf.facing.mirrored() {
                Rect::from_min_max(Pos2::new(1.0, 0.0), Pos2::new(0.0, 1.0))
            } else {
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
            };
            p.image(t.id(), img, uv, Color32::WHITE);
        }
        None => {
            // No camera is a STATE, not an error: say so rather than showing a
            // black rectangle the user cannot distinguish from a crash.
            p.rect_filled(img, 4.0, colors.bg_card);
            p.text(img.center(), egui::Align2::CENTER_CENTER,
                   "no camera", egui::FontId::proportional(14.0), colors.text_weak);
        }
    }
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.clicked() { vf.tap(pos, rect); }
    }
    let a = vf.reticle_alpha();
    if a > 0.0 {
        if let Some(s) = vf.focus {
            let c = vf.to_screen(s, rect);
            let col = colors.accent.gamma_multiply(a);
            let r = 26.0 - 6.0 * (1.0 - a);       // settles inward as it fades
            p.circle_stroke(c, r, Stroke::new(2.0, col));
            for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
                p.line_segment([Pos2::new(c.x + dx * r, c.y + dy * r),
                                Pos2::new(c.x + dx * (r + 6.0), c.y + dy * (r + 6.0))],
                               Stroke::new(2.0, col));
            }
        }
    }
    if let Some(name) = &vf.recipient {
        p.text(Pos2::new(rect.min.x + 10.0, rect.min.y + 8.0), egui::Align2::LEFT_TOP,
               format!("to {name}"), egui::FontId::proportional(13.0), colors.text);
    }
    if vf.scan_mode {
        p.text(Pos2::new(rect.max.x - 10.0, rect.min.y + 8.0), egui::Align2::RIGHT_TOP,
               "scanning", egui::FontId::proportional(12.0), colors.warning);
    }
    rect
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide() -> Rect { Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 400.0)) }
    fn tall() -> Rect { Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 800.0)) }

    #[test]
    fn full_bleed_fills_and_framed_letterboxes() {
        // Choosing the wrong one silently crops people out of their own photos.
        let mut vf = Viewfinder { sensor_aspect: 4.0/3.0, ..Default::default() };
        let r = vf.image_rect(tall());
        assert!(r.width() >= tall().width() - 0.01, "full bleed covers the width");
        assert!(r.height() > tall().height() * 0.4, "and overflows vertically or fills");

        vf.full_bleed = false;
        let r = vf.image_rect(tall());
        assert!(r.width() <= tall().width() + 0.01 && r.height() <= tall().height() + 0.01,
                "framed must fit inside: {r:?}");
        let a = r.width() / r.height();
        assert!((a - 4.0/3.0).abs() < 0.01, "and preserve the sensor aspect, got {a}");
    }

    #[test]
    fn a_tap_maps_to_normalised_sensor_coordinates() {
        // Normalised so a focus point survives a resize or rotation.
        let vf = Viewfinder { full_bleed: false, sensor_aspect: 1.0, ..Default::default() };
        let r = wide();
        let img = vf.image_rect(r);
        let s = vf.to_sensor(img.center(), r).expect("centre is on the image");
        assert!((s[0] - 0.5).abs() < 0.01 && (s[1] - 0.5).abs() < 0.01, "centre is 0.5,0.5: {s:?}");
        // and it round-trips
        let back = vf.to_screen(s, r);
        assert!((back.x - img.center().x).abs() < 1.0);
    }

    #[test]
    fn a_tap_on_the_letterbox_bars_does_not_move_focus() {
        // The bars are not part of the picture, and focusing on them is
        // meaningless — a tap there must be ignored, not clamped.
        let mut vf = Viewfinder { full_bleed: false, sensor_aspect: 1.0, ..Default::default() };
        let r = wide();
        assert!(!vf.tap(Pos2::new(5.0, 200.0), r), "far left is a bar on a wide viewport");
        assert!(vf.focus.is_none());
        assert!(vf.tap(r.center(), r), "the centre is the image");
        assert!(vf.focus.is_some());
    }

    #[test]
    fn the_front_camera_preview_is_mirrored() {
        // Everyone has used a mirror; nobody has used an un-mirrored front
        // camera and thought it looked right.
        let back = Viewfinder { full_bleed: false, sensor_aspect: 1.0, ..Default::default() };
        let front = Viewfinder { facing: Facing::Front, ..back.clone() };
        let r = wide();
        let img = back.image_rect(r);
        let left = Pos2::new(img.min.x + img.width() * 0.2, img.center().y);
        let b = back.to_sensor(left, r).unwrap();
        let f = front.to_sensor(left, r).unwrap();
        assert!(b[0] < 0.5, "back: left of screen is left of sensor");
        assert!(f[0] > 0.5, "front: left of screen is RIGHT of sensor");
        assert!((b[0] + f[0] - 1.0).abs() < 0.01, "and they mirror exactly");
    }

    #[test]
    fn flipping_the_camera_clears_the_focus_point() {
        // A focus point belongs to the sensor that set it; carrying it across a
        // flip focuses somewhere the user never tapped.
        let mut vf = Viewfinder::default();
        vf.tap(wide().center(), wide());
        assert!(vf.focus.is_some());
        vf.flip();
        assert_eq!(vf.facing, Facing::Front);
        assert!(vf.focus.is_none());
    }

    #[test]
    fn the_reticle_fades_and_then_stops_being_drawn() {
        // The fade is what tells the user the tap registered at all.
        let mut vf = Viewfinder::default();
        assert_eq!(vf.reticle_alpha(), 0.0, "nothing to show before a tap");
        vf.tap(wide().center(), wide());
        assert!((vf.reticle_alpha() - 1.0).abs() < 1e-6, "full at the moment of the tap");
        vf.tick(RETICLE_SECONDS * 0.5);
        let mid = vf.reticle_alpha();
        assert!(mid > 0.3 && mid < 0.7, "halfway: {mid}");
        vf.tick(RETICLE_SECONDS);
        assert_eq!(vf.reticle_alpha(), 0.0);
        assert!(vf.focus.is_none(), "and it is cleared, not just invisible");
    }

    #[test]
    fn scan_mode_throttles_the_frame_rate() {
        // A QR scanner does not need thirty frames a second and a phone battery
        // has opinions about it.
        let mut vf = Viewfinder::default();
        let normal = vf.frame_interval();
        vf.scan_mode = true;
        assert!(vf.frame_interval() > normal * 2.0,
                "scanning must be slower: {} vs {}", vf.frame_interval(), normal);
        assert!((vf.frame_interval() - 1.0 / SCAN_FPS).abs() < 1e-6);
    }
}
