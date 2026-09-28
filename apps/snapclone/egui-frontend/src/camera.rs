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
//! * **Digital zoom** — not in the Qt widget; added here. `zoom` is a
//!   window into the frame (`uikit_egui::zoom::Zoom`, pinched about the
//!   point between the fingers); the preview draws the window and the
//!   shutter crops the frame to it, so the photo is what the viewfinder
//!   showed at the sensor's native pixels. It composes with the mirror and
//!   the quarter turns in the one order that is right: window first, then
//!   mirror, then turn — a mirrored preview zoomed on its left shows the
//!   frame's right, which is what a mirror does.
//!
//! The geometry is pure and tested; the drawing is a thin pass over it.

use egui::{Color32, Pos2, Rect, Sense, Ui, Vec2};
use uikit_egui::zoom::Zoom;

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
    /// Quarter turns clockwise applied to the preview, 0..=3.
    ///
    /// A ROTATION, not a mirror. The chrome button used to call `flip()`,
    /// which swaps the front and back CAMERA — a different action entirely,
    /// already reachable from the device picker beside it, and not what a
    /// circular-arrow icon means anywhere else. A phone whose sensor is
    /// mounted sideways (which is most of them) needs a way to say so, and
    /// that is this.
    pub quarter_turns: u8,
    /// Focus point in NORMALISED sensor coordinates, so it survives a resize.
    pub focus: Option<[f32; 2]>,
    /// The reticle proper: three states and a linger, from `FocusReticle`.
    ///
    /// The fields above fade on a fixed timer, which is the SHAPE of a reticle
    /// and not its job — a reticle that disappears on schedule regardless of
    /// outcome never tells the user whether the camera focused. Kept alongside
    /// during the changeover so existing callers still compile; new drawing
    /// should use this.
    pub reticle: crate::reticle::Reticle,
    /// Seconds since the tap, for the reticle fade.
    pub focus_age: f32,
    pub recipient: Option<String>,
    /// Digital zoom: the window of the frame the preview shows and the
    /// shutter saves. 1× is the whole frame.
    pub zoom: Zoom,
}

impl Default for Viewfinder {
    fn default() -> Self {
        Viewfinder {
            facing: Facing::Back, full_bleed: true, scan_mode: false,
            sensor_aspect: 4.0 / 3.0, quarter_turns: 0, focus: None, focus_age: 0.0,
            reticle: Default::default(), recipient: None, zoom: Zoom::default(),
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
        let want = self.drawn_aspect();
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
    /// Texture coordinates for the four corners of the destination rect, in
    /// the order top-left, top-right, bottom-right, bottom-left.
    ///
    /// A quarter turn cannot be expressed as a UV `Rect` — that only flips —
    /// so the preview is drawn as a mesh and this is the whole of the rotation.
    /// Pure, because getting it wrong produces a picture that is merely
    /// mirrored or upside down rather than obviously broken, and by eye those
    /// are hard to tell from a sensor that really is mounted that way.
    pub fn corner_uvs(&self) -> [Pos2; 4] {
        // The zoom window's corners, not the texture's: at 1× they are the
        // same four points.
        let w = self.zoom.window();
        let mut c = [
            Pos2::new(w[0], w[1]),
            Pos2::new(w[2], w[1]),
            Pos2::new(w[2], w[3]),
            Pos2::new(w[0], w[3]),
        ];
        // Mirror FIRST, then rotate. The other order gives the mirror image of
        // the rotation, which for odd turns is not the same picture.
        if self.facing.mirrored() {
            c.swap(0, 1);
            c.swap(2, 3);
        }
        c.rotate_right((self.quarter_turns % 4) as usize);
        c
    }

    /// A screen point as a fraction of the PICTURE drawn in `into` — the
    /// turn undone and the mirror applied, unbounded (a point off the
    /// picture maps outside 0..1). The zoom window maps this on to the
    /// frame; it is the mapping `Zoom::apply` asks for.
    pub fn picture_frac(&self, p: Pos2, into: Rect) -> [f32; 2] {
        let r = self.image_rect(into);
        let mut x = (p.x - r.min.x) / r.width().max(0.01);
        let mut y = (p.y - r.min.y) / r.height().max(0.01);
        // UNDO the rotation, so a tap lands on the part of the SENSOR the user
        // pointed at. A focus point stored in screen terms would move the
        // moment the preview was turned.
        for _ in 0..(self.quarter_turns % 4) {
            let (nx, ny) = (y, 1.0 - x);
            x = nx;
            y = ny;
        }
        if self.facing.mirrored() { x = 1.0 - x; }
        [x, y]
    }

    pub fn to_sensor(&self, p: Pos2, into: Rect) -> Option<[f32; 2]> {
        let r = self.image_rect(into);
        if r.width() <= 0.0 || r.height() <= 0.0 { return None; }
        let x = (p.x - r.min.x) / r.width();
        let y = (p.y - r.min.y) / r.height();
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) { return None; }
        // Through the zoom window last: the picture is the window, and a
        // tap on the picture's centre is the centre of the WINDOW, not of
        // the sensor.
        Some(self.zoom.frac_to_frame(self.picture_frac(p, into)))
    }

    /// The inverse, for drawing the reticle where the user tapped: the
    /// zoom window, then the mirror, then the turn — `picture_frac` run
    /// backwards, so the two round-trip.
    pub fn to_screen(&self, s: [f32; 2], into: Rect) -> Pos2 {
        let r = self.image_rect(into);
        let f = self.zoom.frame_to_frac(s);
        let (mut x, mut y) = (if self.facing.mirrored() { 1.0 - f[0] } else { f[0] }, f[1]);
        for _ in 0..(self.quarter_turns % 4) {
            let (nx, ny) = (1.0 - y, x);
            x = nx;
            y = ny;
        }
        Pos2::new(r.min.x + x * r.width(), r.min.y + y * r.height())
    }

    pub fn tap(&mut self, p: Pos2, into: Rect) -> bool {
        match self.to_sensor(p, into) {
            Some(s) => {
                self.focus = Some(s);
                self.focus_age = 0.0;
                self.reticle.trigger_at(s[0], s[1]);
                true
            }
            // A tap outside the image must NOT move focus — on a letterboxed
            // preview the bars are not part of the picture.
            None => false,
        }
    }

    pub fn tick(&mut self, dt: f32) {
        if self.focus.is_some() {
            self.reticle.tick(dt * 1000.0);
            self.focus_age += dt;
            if self.focus_age > RETICLE_SECONDS { self.focus = None; }
        }
    }

    /// Reticle opacity 0..1, or 0 when there is nothing to show.
    ///
    /// SUPERSEDED and no longer drawn — `reticle::Reticle` is what the
    /// viewfinder paints. Kept because `focus`/`focus_age` still gate the
    /// tap logic and its tests, and because this timer is what decides when
    /// `focus` clears. Removing it means deciding what clears the focus point
    /// instead, which is a behaviour change and not a tidy-up; it should
    /// happen when the reticle also owns that lifetime.
    ///
    /// Nothing new should call this. The reason is in the draw site: a fade on
    /// a fixed schedule cannot say whether the camera focused.
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

    /// Advance the preview one quarter turn clockwise, wrapping at four.
    ///
    /// The focus point is kept: unlike a camera swap, a rotation is the same
    /// sensor looking at the same scene, and the tap the user made is still
    /// about the same part of it. `to_sensor` already stores focus in
    /// normalised SENSOR coordinates for exactly this reason.
    pub fn rotate_cw(&mut self) {
        self.quarter_turns = (self.quarter_turns + 1) % 4;
    }

    /// The aspect as the preview is DRAWN — the sensor's, inverted on an odd
    /// quarter turn. Without this a rotated 4:3 frame is letterboxed as though
    /// it were still landscape and the image is squashed into the wrong box,
    /// which looks like a broken decoder rather than a rotation.
    pub fn drawn_aspect(&self) -> f32 {
        let a = self.sensor_aspect.max(0.01);
        if self.quarter_turns % 2 == 1 { 1.0 / a } else { a }
    }

    pub fn flip(&mut self) {
        self.facing = self.facing.flipped();
        // A focus point belongs to the sensor that set it; keeping it across a
        // flip focuses somewhere the user never tapped. So does a zoom window.
        self.focus = None;
        self.zoom.reset();
    }

    /// Fold a frame's gestures into the zoom: a pinch over the picture
    /// zooms about the fingers, a one-finger drag pans while zoomed.
    /// `into` is the rect the viewfinder was drawn in. Returns whether the
    /// window changed.
    pub fn zoom_gestures(&mut self, gestures: &[uikit_egui::touch::Gesture], into: Rect) -> bool {
        let picture = self.image_rect(into).intersect(into);
        let me = self.clone_geometry();
        self.zoom.apply(gestures, picture, |p| me.picture_frac(p, into))
    }

    /// The geometry alone, for a mapping closure that must not borrow
    /// `self` while the zoom is mutated.
    fn clone_geometry(&self) -> Viewfinder {
        Viewfinder { facing: self.facing, full_bleed: self.full_bleed, sensor_aspect: self.sensor_aspect,
                     quarter_turns: self.quarter_turns, zoom: self.zoom, ..Default::default() }
    }
}

/// Draw the viewfinder. `frame` is the latest camera image, if any.
/// How long the capture flash lasts, seconds.
pub const FLASH_SECS: f32 = 0.22;

/// Opacity of the capture flash `t` seconds after the shutter fired.
///
/// The shutter WAS wired — `shutter_pressed` becomes `HostCommand::CaptureSnap`,
/// the host writes the file, and `HostEvent::SnapSaved` sets a note. What it
/// had no way to say was "that happened": the only feedback was a line of text
/// under a fullscreen preview, which on a phone is easy to miss entirely. The
/// report "the photo button isn't wired yet" is what a working capture with no
/// visible confirmation looks like from the outside.
///
/// So: the flash every camera has. Bright immediately and gone quickly —
/// linear rather than eased, because a flash that lingers reads as a rendering
/// fault over a moving preview rather than as a shutter.
pub fn flash_alpha(t: f32) -> f32 {
    if t < 0.0 || t >= FLASH_SECS {
        return 0.0;
    }
    1.0 - t / FLASH_SECS
}

/// The colour the capture flash paints, at age `t`.
///
/// **Dark by default.** A white flash is what a phone screen does because a
/// phone screen is also the fill light; a camera's own shutter does the
/// opposite — the frame goes BLACK for the moment the mirror is up, and that
/// is the confirmation people who have used a camera actually recognise. It is
/// also the kinder one: this app is pointed at faces in dim rooms, and a
/// full-brightness white rectangle at arm's length in the dark is unpleasant
/// in a way the alternative is not.
///
/// `bright` is the opt-in, for anyone who wants the phone behaviour or is
/// shooting somewhere a dark flash is invisible against a dark scene.
///
/// Alpha peaks lower than opaque in both directions: a flash that fully
/// replaces the frame loses the picture the user just took, and seeing the
/// shot dim rather than vanish is what makes it read as a shutter rather than
/// as a dropped frame.
pub fn flash_color(t: f32, bright: bool) -> Color32 {
    let a = (flash_alpha(t) * 220.0) as u8;
    if bright {
        Color32::from_white_alpha(a)
    } else {
        Color32::from_black_alpha(a)
    }
}

/// The band at the foot of the viewfinder that the controls are drawn OVER.
///
/// The camera page used to RESERVE space for its controls, which itself
/// replaced a hard-coded 220px preview. Both were the wrong shape: a viewfinder
/// that stops short of the edge reads as a preview of a camera rather than as
/// the camera, and the part it gives up is the bottom — which on a phone held
/// at arm's length is where the subject is.
///
/// So the preview is the whole page and this is where the chrome floats. The
/// clamp is the load-bearing part: on a short window the controls' natural
/// height can exceed the frame, and a band taller than what it overlays would
/// cover the entire preview with a scrim to show a shutter button sitting on
/// nothing.
pub fn controls_band(vf: Rect, natural_h: f32) -> Rect {
    let h = natural_h.max(0.0).min(vf.height() * 0.5);
    Rect::from_min_max(Pos2::new(vf.left(), vf.bottom() - h), vf.max)
}

pub fn viewfinder(ui: &mut Ui, vf: &mut Viewfinder, frame: Option<&egui::TextureHandle>,
                  colors: &uikit_egui::PaletteColors) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, Color32::from_rgb(8, 9, 12));
    let img = vf.image_rect(rect);
    match frame {
        Some(t) => {
            // A MESH, not `Painter::image`: that takes a UV rect, which can
            // mirror but cannot turn a picture ninety degrees. Four vertices
            // with permuted UVs can, and it is the same one draw call.
            let uvs = vf.corner_uvs();
            let corners = [
                img.left_top(), img.right_top(), img.right_bottom(), img.left_bottom(),
            ];
            let mut mesh = egui::Mesh::with_texture(t.id());
            // Vertices pushed whole. `colored_vertex` is the white-pixel helper
            // and asserts the mesh has NO texture — a debug_assert, so release
            // builds on the device drew every frame fine while the first
            // frame of a debug build panicked "Mesh has an assigned texture".
            // Found running the app under Xvfb with the test pattern.
            for (i, pos) in corners.iter().enumerate() {
                mesh.vertices.push(egui::epaint::Vertex { pos: *pos, uv: uvs[i], color: Color32::WHITE });
            }
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            p.add(egui::Shape::mesh(mesh));
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
    // THE REAL RETICLE. `Viewfinder` has held a `reticle::Reticle` since the
    // changeover started, `tap()` already calls `trigger_at`, and the comment
    // on that field says new drawing should use it — then the drawing here
    // stayed on the old fixed-timer fade, so nothing ever rendered the module.
    //
    // The difference is not cosmetic. The timer version fades on a schedule
    // whatever the camera did, so it cannot tell "focused" from "gave up";
    // reticle::Reticle carries Searching / Locked / Failed and lingers on a
    // RESULT, which is the whole reason it was written.
    //
    // Drawn where the tap WAS: the reticle stores the sensor point, and
    // the sensor point is on screen wherever the mirror, the turn and the
    // zoom window put it. Mapping it as a fraction of the view (what
    // `reticle_ui` does on its own) was right only for an unmirrored,
    // unturned, unzoomed full-bleed preview.
    if let Some((nx, ny)) = vf.reticle.at() {
        crate::reticle::reticle_ui_at(ui, &vf.reticle, vf.to_screen([nx, ny], rect), colors);
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

    #[test]
    fn the_rotation_is_a_quarter_turn_and_wraps() {
        let mut vf = Viewfinder::default();
        assert_eq!(vf.quarter_turns, 0);
        for want in [1, 2, 3, 0] {
            vf.rotate_cw();
            assert_eq!(vf.quarter_turns, want);
        }
        // And it does NOT change the camera — the bug being fixed is that this
        // control swapped front/back instead.
        let facing = vf.facing;
        vf.rotate_cw();
        assert_eq!(vf.facing, facing, "rotating swapped the camera");
    }

    #[test]
    fn an_odd_quarter_turn_inverts_the_drawn_aspect() {
        // A rotated 4:3 frame letterboxed as though still landscape is
        // squashed into the wrong box, which reads as a broken decoder rather
        // than a rotation.
        let mut vf = Viewfinder { sensor_aspect: 4.0 / 3.0, ..Default::default() };
        assert!((vf.drawn_aspect() - 4.0 / 3.0).abs() < 1e-6);
        vf.rotate_cw();
        assert!((vf.drawn_aspect() - 3.0 / 4.0).abs() < 1e-6, "{}", vf.drawn_aspect());
        vf.rotate_cw();
        assert!((vf.drawn_aspect() - 4.0 / 3.0).abs() < 1e-6, "half a turn is the same shape");
    }

    #[test]
    fn each_quarter_turn_gives_a_different_corner_mapping() {
        // Getting this wrong produces a picture that is merely mirrored or
        // upside down rather than obviously broken, and by eye that is hard to
        // tell from a sensor genuinely mounted that way. So: all four
        // distinct, and four turns returns to the start.
        let mut seen = Vec::new();
        for q in 0..4u8 {
            let vf = Viewfinder { quarter_turns: q, ..Default::default() };
            let uvs = vf.corner_uvs();
            assert!(!seen.contains(&uvs), "turn {q} repeats an earlier mapping");
            seen.push(uvs);
            // Every corner of the texture is used exactly once, whatever the
            // turn — a permutation, never a fold.
            let mut sorted = uvs;
            sorted.sort_by(|a, b| (a.x, a.y).partial_cmp(&(b.x, b.y)).unwrap());
            assert_eq!(sorted, [Pos2::new(0.0, 0.0), Pos2::new(0.0, 1.0),
                                Pos2::new(1.0, 0.0), Pos2::new(1.0, 1.0)]);
        }
        let full = Viewfinder { quarter_turns: 4, ..Default::default() };
        assert_eq!(full.corner_uvs(), Viewfinder::default().corner_uvs());
    }

    #[test]
    fn a_tap_maps_back_through_the_rotation() {
        // The focus point is stored in SENSOR coordinates so it survives a
        // resize; it has to survive a rotation for the same reason. A tap on
        // the top-left of a preview turned once clockwise is the BOTTOM-left
        // of the sensor.
        let into = Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 300.0));
        let vf = Viewfinder { full_bleed: false, sensor_aspect: 1.0,
                              quarter_turns: 1, ..Default::default() };
        let s = vf.to_sensor(Pos2::new(10.0, 10.0), into).expect("inside the frame");
        assert!(s[0] < 0.2 && s[1] > 0.8, "got {s:?}");
        // Unrotated, the same tap is the top-left of the sensor — so the test
        // is measuring the rotation and not the mapping in general.
        let flat = Viewfinder { full_bleed: false, sensor_aspect: 1.0, ..Default::default() };
        let s0 = flat.to_sensor(Pos2::new(10.0, 10.0), into).unwrap();
        assert!(s0[0] < 0.2 && s0[1] < 0.2, "got {s0:?}");
    }

    #[test]
    fn the_zoom_window_is_what_the_mesh_draws_mirror_and_turn_included() {
        // 2× on the frame's right half. Unmirrored, unturned: the mesh's
        // top-left corner reads texture (0.5, 0.25).
        let mut vf = Viewfinder { facing: Facing::Back, ..Default::default() };
        vf.zoom.pinch(2.0, [1.0, 0.5]);
        let uvs = vf.corner_uvs();
        assert_eq!(uvs[0], Pos2::new(0.5, 0.25));
        assert_eq!(uvs[2], Pos2::new(1.0, 0.75));
        // Mirrored: the same window, drawn right to left.
        let front = Viewfinder { facing: Facing::Front, ..vf.clone() };
        let uvs = front.corner_uvs();
        assert_eq!(uvs[0], Pos2::new(1.0, 0.25));
        assert_eq!(uvs[1], Pos2::new(0.5, 0.25));
        // Turned once: still the window's four corners, permuted.
        let turned = Viewfinder { quarter_turns: 1, ..vf.clone() };
        let mut got = turned.corner_uvs();
        got.sort_by(|a, b| (a.x, a.y).partial_cmp(&(b.x, b.y)).unwrap());
        assert_eq!(got, [Pos2::new(0.5, 0.25), Pos2::new(0.5, 0.75), Pos2::new(1.0, 0.25), Pos2::new(1.0, 0.75)]);
        // At 1× the corners are the texture's, as before zoom existed.
        assert_eq!(Viewfinder::default().corner_uvs(), [Pos2::new(0.0, 0.0), Pos2::new(1.0, 0.0), Pos2::new(1.0, 1.0), Pos2::new(0.0, 1.0)]);
    }

    #[test]
    fn a_tap_while_zoomed_focuses_on_the_part_of_the_sensor_shown() {
        // The picture is the window: its centre is the WINDOW's centre.
        let mut vf = Viewfinder { full_bleed: false, sensor_aspect: 1.0, ..Default::default() };
        let r = wide();
        vf.zoom.pinch(2.0, [1.0, 0.0]);   // the frame's top-right quarter
        let s = vf.to_sensor(vf.image_rect(r).center(), r).unwrap();
        assert!((s[0] - 0.75).abs() < 0.01 && (s[1] - 0.25).abs() < 0.01, "{s:?}");
        // And the reticle is drawn back where the tap was.
        let back = vf.to_screen(s, r);
        assert!((back.x - vf.image_rect(r).center().x).abs() < 1.0 && (back.y - vf.image_rect(r).center().y).abs() < 1.0);
        // Mirrored: the same tap is the window's centre still, and the
        // window itself is where the mirror put it.
        let mut front = Viewfinder { facing: Facing::Front, full_bleed: false, sensor_aspect: 1.0, ..Default::default() };
        let img = front.image_rect(r);
        // Pinch with the fingers on the screen's LEFT edge: the frame's right.
        let g = [uikit_egui::touch::Gesture::Pinch { scale: 2.0, rel: 2.0,
            center: touchkit::Vec2::new(img.left() + 1.0, img.center().y) }];
        assert!(front.zoom_gestures(&g, r));
        assert!(front.zoom.window()[2] > 0.99, "{:?}", front.zoom.window());
    }

    #[test]
    fn a_tap_and_its_reticle_round_trip_through_mirror_turn_and_zoom() {
        // The reticle is drawn from the stored SENSOR point; it must land
        // back under the finger whatever the preview is doing.
        let into = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(360.0, 640.0));
        for facing in [Facing::Back, Facing::Front] {
            for turns in 0..4u8 {
                for zoom in [1.0, 2.3] {
                    let mut vf = Viewfinder { facing, quarter_turns: turns, sensor_aspect: 4.0 / 3.0, ..Default::default() };
                    vf.zoom.pinch(zoom, [0.7, 0.3]);
                    let img = vf.image_rect(into);
                    let p = Pos2::new(img.min.x + img.width() * 0.3, img.min.y + img.height() * 0.8);
                    let s = vf.to_sensor(p, into).expect("on the picture");
                    let back = vf.to_screen(s, into);
                    assert!((back.x - p.x).abs() < 0.5 && (back.y - p.y).abs() < 0.5,
                            "{facing:?} turns={turns} zoom={zoom}: {p:?} -> {s:?} -> {back:?}");
                }
            }
        }
    }

    #[test]
    fn flipping_resets_the_zoom_and_turning_keeps_it() {
        let mut vf = Viewfinder::default();
        vf.zoom.set_factor(2.0);
        vf.rotate_cw();
        assert!(vf.zoom.is_zoomed(), "the same sensor, the same window");
        vf.flip();
        assert!(!vf.zoom.is_zoomed(), "a window into one sensor means nothing on the other");
    }

    #[test]
    fn the_flash_is_dark_unless_asked_otherwise() {
        // The default is the whole point of this function, so the default is
        // what the test names. A check that only compared the two modes would
        // pass with them swapped.
        let dark = flash_color(0.0, false);
        assert_eq!(dark.to_srgba_unmultiplied()[..3], [0, 0, 0], "the default flash is not dark");
        let bright = flash_color(0.0, true);
        assert_eq!(bright.to_srgba_unmultiplied()[..3], [255, 255, 255]);
        // Both fade on the same clock, and neither is ever fully opaque — a
        // flash that replaces the frame loses the shot the user just took.
        for bright in [false, true] {
            assert!(flash_color(0.0, bright).a() < 255, "the flash hid the photo");
            assert_eq!(flash_color(FLASH_SECS, bright).a(), 0, "the flash lingered");
            assert!(flash_color(FLASH_SECS * 0.5, bright).a() < flash_color(0.0, bright).a());
        }
    }

    #[test]
    fn the_capture_flash_is_bright_then_gone() {
        assert!((flash_alpha(0.0) - 1.0).abs() < 1e-6);
        assert!(flash_alpha(FLASH_SECS * 0.5) < 0.6);
        assert_eq!(flash_alpha(FLASH_SECS), 0.0, "the flash must end, not linger");
        assert_eq!(flash_alpha(FLASH_SECS * 10.0), 0.0);
        assert_eq!(flash_alpha(-1.0), 0.0, "a negative age must not paint");
        // Monotone: a flash that brightened again would read as a fault.
        let mut last = flash_alpha(0.0);
        for i in 1..=20 {
            let a = flash_alpha(FLASH_SECS * i as f32 / 20.0);
            assert!(a <= last + 1e-6, "the flash brightened at step {i}");
            last = a;
        }
    }

    #[test]
    fn the_controls_band_sits_at_the_foot_of_the_frame() {
        let vf = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(400.0, 800.0));
        let b = controls_band(vf, 180.0);
        assert_eq!(b.bottom(), vf.bottom(), "the band floated off the bottom edge");
        assert_eq!(b.height(), 180.0);
        assert_eq!((b.left(), b.right()), (vf.left(), vf.right()));
        assert!(vf.contains_rect(b), "the band escaped the viewfinder");
    }

    #[test]
    fn the_band_never_takes_more_than_half_the_frame() {
        // On a short window the controls' natural height can exceed the frame.
        // Without the clamp the scrim covers the whole preview to show a
        // shutter sitting on nothing — worse than cramped chrome.
        let vf = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0));
        let b = controls_band(vf, 900.0);
        assert_eq!(b.height(), 100.0);
        assert!(vf.contains_rect(b));
        // And it does not clamp a frame with room, which a floor-only test
        // would not distinguish.
        let tall = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 900.0));
        assert_eq!(controls_band(tall, 180.0).height(), 180.0);
    }

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

#[cfg(test)]
mod reticle_wiring_tests {
    use super::*;
    use crate::reticle::FocusState;

    #[test]
    fn a_viewfinder_tap_starts_a_real_sweep_not_just_a_timer() {
        // The old path set a point and counted down. A reticle that fades on
        // schedule regardless of outcome never says whether focus succeeded.
        let mut v = Viewfinder::default();
        let view = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        v.tap(egui::pos2(200.0, 150.0), view);
        assert_eq!(v.reticle.state(), FocusState::Searching);
        assert!(v.reticle.visible());
    }

    #[test]
    fn the_reticle_waits_for_the_camera_rather_than_the_clock() {
        // Ten seconds of ticking with no result: still searching, because the
        // camera decides when a sweep ends.
        let mut v = Viewfinder::default();
        let view = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        v.tap(egui::pos2(200.0, 150.0), view);
        for _ in 0..100 { v.tick(0.1); }
        assert_eq!(v.reticle.state(), FocusState::Searching);
        assert!(v.reticle.visible());
    }

    #[test]
    fn a_focus_failure_is_shown_before_it_fades() {
        let mut v = Viewfinder::default();
        let view = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        v.tap(egui::pos2(200.0, 150.0), view);
        assert!(v.reticle.set_state(FocusState::Failed));
        v.tick(0.5);
        assert_eq!(v.reticle.state(), FocusState::Failed);
        assert_eq!(v.reticle.alpha(), 1.0, "lingering, not fading yet");
    }
}
