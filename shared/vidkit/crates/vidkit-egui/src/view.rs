//! The video surface widget.
//!
//! Chrome comes from `uikit-egui` rather than being redrawn here: the carved
//! painters for the transport bar, the palette bridge for colours, the
//! `GestureLayer` for the swipe feel. An app that already uses the kit gets a
//! video surface that looks like the rest of it, and a restyle of the kit
//! restyles this too.

use egui::{Color32, Rect, Response, Sense, Ui, Vec2};
use std::time::Duration;
use uikit_egui::skin::Skin;

use crate::fit::{fit_rect, Fit, PixelAspect};
use crate::gestures::{
    apply_rotation, apply_speed_step, apply_volume_step, apply_zoom_step, GestureRouter, Intent,
    ViewLock,
};
use crate::presenter::EguiPresenter;

/// Everything the widget needs to know about playback, and the only thing it
/// mutates. The widget owns no playback state: it reports intent and the shell
/// applies it to the player, so a seek that the player refuses does not leave
/// the UI showing a position that never happened.
#[derive(Clone, Debug)]
pub struct Transport {
    pub position: Duration,
    pub duration: Option<Duration>,
    pub playing: bool,
    /// Playback rate. 1.0 is normal; gestures move it in 5% steps.
    pub speed: f64,
    /// In-app volume 0..1. Deliberately not the system volume: a gesture on a
    /// video surface should not move what every other app on the machine hears.
    pub volume: f32,
    /// Quarter turns clockwise, 0..=3.
    pub rotation: i32,
    /// Free zoom multiplier, 1.0 = fitted.
    pub zoom: f32,
    /// One line for the corner: codec, size, decode path.
    pub status: String,
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            position: Duration::ZERO,
            duration: None,
            playing: false,
            speed: 1.0,
            volume: 1.0,
            rotation: 0,
            zoom: 1.0,
            status: String::new(),
        }
    }
}

/// What the user asked for this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewAction {
    pub seek_to: Option<Duration>,
    pub toggle_play: bool,
    /// Live scrub position while dragging, before release. The shell can show
    /// it without committing a seek per frame, which on a network stream would
    /// be one re-resolve per pixel.
    pub scrubbing_to: Option<Duration>,
}

impl ViewAction {
    pub fn is_empty(&self) -> bool {
        self == &ViewAction::default()
    }
}

/// Fraction of the duration a horizontal drag of `dx` over a surface of
/// `width` corresponds to.
///
/// Separated out because the sign and the scale are both easy to get backwards
/// and neither is visible in a screenshot: a scrub that runs the wrong way
/// still looks like a working scrub.
pub fn scrub_fraction(dx: f32, width: f32) -> f32 {
    if width <= 0.0 {
        return 0.0;
    }
    dx / width
}

/// Apply a scrub fraction to a position, clamped to the media.
pub fn scrub_target(from: Duration, duration: Option<Duration>, fraction: f32) -> Duration {
    let total = match duration {
        Some(d) if d > Duration::ZERO => d,
        // Live or unknown-duration media: a full-width drag is 60 s, so the
        // control still does something instead of silently nothing.
        _ => Duration::from_secs(60),
    };
    let delta = total.as_secs_f64() * fraction as f64;
    let target = from.as_secs_f64() + delta;
    let clamped = target.clamp(0.0, duration.map(|d| d.as_secs_f64()).unwrap_or(f64::MAX));
    Duration::from_secs_f64(clamped.max(0.0))
}

/// Progress through the media, 0..1. `None` when there is no duration to be a
/// fraction of.
pub fn progress(position: Duration, duration: Option<Duration>) -> Option<f32> {
    let d = duration?;
    if d == Duration::ZERO {
        return None;
    }
    Some((position.as_secs_f32() / d.as_secs_f32()).clamp(0.0, 1.0))
}

/// `H:MM:SS` or `M:SS`, matching how the rest of the tree writes durations.
pub fn format_time(d: Duration) -> String {
    let total = d.as_secs();
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The video surface.
pub struct VideoView {
    pub fit: Fit,
    pub pixel_aspect: PixelAspect,
    pub skin: Skin,
    /// Letterbox bar colour. Black is right for a real player; a demo whose
    /// test pattern ends in a black bar wants something else.
    pub bar: Color32,
    /// Feedback overlay text and the frame it should fade at.
    hud: Option<(String, f64)>,
    router: GestureRouter,
    /// Live scrub offset in seconds, before commit.
    scrub_seconds: f64,
}

impl VideoView {
    pub fn new() -> Self {
        Self {
            fit: Fit::Contain,
            pixel_aspect: PixelAspect::SQUARE,
            // The kit's carved look, not egui's defaults -- the whole point of
            // building on uikit-egui rather than beside it.
            skin: Skin::carved(),
            bar: Color32::BLACK,
            hud: None,
            router: GestureRouter::default(),
            scrub_seconds: 0.0,
        }
    }

    pub fn with_fit(mut self, fit: Fit) -> Self {
        self.fit = fit;
        self
    }

    /// The gesture configuration, for an app that wants to retune the zones.
    pub fn gestures(&mut self) -> &mut crate::gestures::GestureConfig {
        &mut self.router.cfg
    }

    fn flash(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        self.hud = Some((text.into(), ctx.input(|i| i.time) + 0.9));
        // The HUD has to disappear on its own, which needs a wake-up: without
        // this it lingers until some other input happens to repaint.
        ctx.request_repaint_after(std::time::Duration::from_millis(950));
    }

    /// Apply one intent to the transport and report what to show.
    ///
    /// Separated from input handling so the mapping from intent to state is
    /// testable without a pointer.
    pub fn apply(&mut self, intent: Intent, t: &mut Transport, action: &mut ViewAction) -> Option<String> {
        match intent {
            Intent::ScrubBy(s) => {
                self.scrub_seconds += s;
                let target = offset_position(t.position, t.duration, self.scrub_seconds);
                action.scrubbing_to = Some(target);
                Some(format!(
                    "{}  ({}{})",
                    format_time(target),
                    if self.scrub_seconds >= 0.0 { "+" } else { "-" },
                    format_time(Duration::from_secs_f64(self.scrub_seconds.abs()))
                ))
            }
            Intent::ScrubCommit(s) => {
                let target = offset_position(t.position, t.duration, s);
                self.scrub_seconds = 0.0;
                action.seek_to = Some(target);
                action.scrubbing_to = None;
                Some(format!("seek {}", format_time(target)))
            }
            Intent::SpeedStep(n) => {
                t.speed = apply_speed_step(t.speed, n);
                Some(format!("{:.2}x", t.speed))
            }
            Intent::VolumeStep(n) => {
                t.volume = apply_volume_step(t.volume, n);
                Some(format!("volume {}%", (t.volume * 100.0).round() as i32))
            }
            Intent::Rotate(q) => {
                t.rotation = apply_rotation(t.rotation, q);
                Some(format!("{}deg", t.rotation * 90))
            }
            Intent::ZoomStep(n) => {
                t.zoom = apply_zoom_step(t.zoom, n);
                Some(format!("zoom {}%", (t.zoom * 100.0).round() as i32))
            }
            Intent::LockView(lock) => {
                t.zoom = 1.0;
                self.fit = match lock {
                    ViewLock::Fit => Fit::Contain,
                    ViewLock::Fill => Fit::Cover,
                    ViewLock::Actual => Fit::Contain,
                };
                Some(match lock {
                    ViewLock::Fit => "fit".into(),
                    ViewLock::Fill => "fill".into(),
                    ViewLock::Actual => "1:1".to_string(),
                })
            }
            Intent::TogglePlay => {
                action.toggle_play = true;
                Some(if t.playing { "pause" } else { "play" }.into())
            }
        }
    }

    /// Paint the current frame and read input. Returns what the user asked
    /// for; the caller applies it to the player.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        presenter: &EguiPresenter,
        transport: &mut Transport,
    ) -> (Response, ViewAction) {
        let surface = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(surface, Sense::click_and_drag());
        let painter = ui.painter_at(surface);
        let mut action = ViewAction::default();

        painter.rect_filled(surface, 0.0, self.bar);

        match presenter.texture() {
            Some(tex) => {
                // Rotation swaps the frame's effective dimensions, so the fit
                // is computed on the ROTATED size -- otherwise a portrait
                // rotation is letterboxed as if it were still landscape.
                let raw = presenter.frame_size();
                let quarter = transport.rotation.rem_euclid(4);
                let effective = rotated_size(raw, quarter);
                let mut r = fit_rect(surface, effective, self.pixel_aspect, self.fit);
                if transport.zoom != 1.0 {
                    r = Rect::from_center_size(r.center(), r.size() * transport.zoom);
                }
                if r.width() > 0.0 && r.height() > 0.0 {
                    paint_rotated(&painter, tex.id(), r, quarter);
                    // Only the visible picture, not the letterbox bars.
                    crate::eink_hint::hint_video(ui.ctx(), r.intersect(surface));
                }
            }
            None => {
                let msg = if transport.status.is_empty() {
                    "no video"
                } else {
                    transport.status.as_str()
                };
                painter.text(
                    surface.center(),
                    egui::Align2::CENTER_CENTER,
                    msg,
                    egui::FontId::proportional(14.0),
                    ui.visuals().weak_text_color(),
                );
            }
        }

        self.input(ui, &response, surface, transport, &mut action);
        self.paint_hud(ui, surface);

        (response, action)
    }

    /// Gestures on `surface`: drag, tap and pinch become [`ViewAction`]s. `show` calls this after
    /// painting; a caller that presents the picture itself (the GPU path, which paints through a
    /// callback and never reaches `show`) calls it directly, or no finger ever scrubs.
    pub fn handle_gestures(&mut self, ui: &mut Ui, surface: Rect, transport: &mut Transport) -> ViewAction {
        let response = ui.allocate_rect(surface, Sense::click_and_drag());
        let mut action = ViewAction::default();
        self.input(ui, &response, surface, transport, &mut action);
        self.paint_hud(ui, surface);
        action
    }

    fn input(&mut self, ui: &mut Ui, response: &Response, surface: Rect, transport: &mut Transport, action: &mut ViewAction) {
        // --- input ------------------------------------------------------
        let mut flash: Option<String> = None;

        if response.drag_started() {
            if let Some(p) = response.interact_pointer_pos() {
                self.router.press(p, surface);
            }
        }
        if response.dragged() {
            if let Some(p) = response.interact_pointer_pos() {
                for intent in self.router.drag_to(p, surface) {
                    if let Some(f) = self.apply(intent, transport, action) {
                        flash = Some(f);
                    }
                }
            }
        }
        if response.drag_stopped() {
            for intent in self.router.release() {
                if let Some(f) = self.apply(intent, transport, action) {
                    flash = Some(f);
                }
            }
        }
        if response.clicked() {
            // A press with no committed axis is a tap. Routing it through the
            // router rather than trusting `clicked()` is what stops a finished
            // scrub from also toggling playback.
            self.router.press(
                response.interact_pointer_pos().unwrap_or(surface.center()),
                surface,
            );
            for intent in self.router.tap() {
                if let Some(f) = self.apply(intent, transport, action) {
                    flash = Some(f);
                }
            }
        }

        // Pinch. egui reports a multiplicative zoom factor for the frame.
        let zoom_delta = ui.ctx().input(|i| i.zoom_delta());
        if (zoom_delta - 1.0).abs() > f32::EPSILON && response.hovered() {
            for intent in self.router.pinch(zoom_delta) {
                if let Some(f) = self.apply(intent, transport, action) {
                    flash = Some(f);
                }
            }
        }

        if let Some(f) = flash {
            self.flash(ui.ctx(), f);
        }
    }

    fn paint_hud(&mut self, ui: &Ui, surface: Rect) {
        let now = ui.ctx().input(|i| i.time);
        let Some((text, until)) = self.hud.clone() else {
            return;
        };
        if now > until {
            self.hud = None;
            return;
        }
        let p = ui.painter_at(surface);
        let font = egui::FontId::proportional(22.0);
        let galley = p.layout_no_wrap(text, font.clone(), Color32::WHITE);
        let pad = Vec2::new(16.0, 10.0);
        let box_rect = Rect::from_center_size(surface.center(), galley.size() + pad * 2.0);
        p.rect_filled(
            box_rect,
            self.skin.radius as f32,
            Color32::from_black_alpha(170),
        );
        p.galley(box_rect.min + pad, galley, Color32::WHITE);
    }
}

/// Draw a texture rotated by `quarter` turns clockwise.
///
/// `Painter::image` cannot rotate, so the rotated cases go through a mesh with
/// the UVs permuted. Rotating the RECT instead would letterbox correctly and
/// still draw the picture the wrong way up, which looks like a decode bug.
fn paint_rotated(painter: &egui::Painter, tex: egui::TextureId, r: Rect, quarter: i32) {
    if quarter.rem_euclid(4) == 0 {
        painter.image(
            tex,
            r,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        return;
    }
    // Corner UVs in draw order: top-left, top-right, bottom-right, bottom-left.
    let base = [
        egui::pos2(0.0, 0.0),
        egui::pos2(1.0, 0.0),
        egui::pos2(1.0, 1.0),
        egui::pos2(0.0, 1.0),
    ];
    let k = quarter.rem_euclid(4) as usize;
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];

    let mut mesh = egui::Mesh::with_texture(tex);
    for i in 0..4 {
        // Vertices are pushed directly: `Mesh::colored_vertex` asserts the mesh
        // has NO texture and panics otherwise, which crashed the app on the
        // first rotation. Nothing headless caught it -- rotation needs a
        // painter -- so `rotated_painting_does_not_panic` below exists to.
        mesh.vertices.push(egui::epaint::Vertex {
            pos: corners[i],
            // Rotating the UV assignment by k turns the picture, not the quad.
            uv: base[(i + 4 - k) % 4],
            color: Color32::WHITE,
        });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// The frame's dimensions after `quarter` turns.
///
/// Pulled out of `show` so it can be tested: it lives on the painting path,
/// which needs a live context, and a mutation sweep found that dropping the
/// swap entirely killed no test. Getting it wrong letterboxes a rotated
/// portrait picture as though it were still landscape -- the picture is
/// correct and the bars are wrong, which reads as a fit bug rather than a
/// rotation one.
pub fn rotated_size(raw: Vec2, quarter: i32) -> Vec2 {
    if quarter.rem_euclid(4) % 2 == 1 {
        Vec2::new(raw.y, raw.x)
    } else {
        raw
    }
}

/// Offset a position by seconds, clamped to the media.
pub fn offset_position(from: Duration, duration: Option<Duration>, seconds: f64) -> Duration {
    let target = from.as_secs_f64() + seconds;
    let hi = duration.map(|d| d.as_secs_f64()).unwrap_or(f64::MAX);
    Duration::from_secs_f64(target.clamp(0.0, hi.max(0.0)))
}

impl Default for VideoView {
    fn default() -> Self {
        Self::new()
    }
}

/// Paint a progress bar with the kit's carved look.
pub fn progress_bar(ui: &mut Ui, at: Rect, fraction: f32, base: Color32, accent: Color32, skin: &Skin) {
    let p = ui.painter_at(at);
    let radius = skin.radius as f32;
    p.rect_filled(at, radius, base);
    let w = at.width() * fraction.clamp(0.0, 1.0);
    if w > 0.0 {
        p.rect_filled(
            Rect::from_min_size(at.min, Vec2::new(w, at.height())),
            radius,
            accent,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: f64) -> Duration {
        Duration::from_secs_f64(n)
    }

    #[test]
    fn a_rightward_drag_scrubs_forward() {
        assert!(scrub_fraction(40.0, 400.0) > 0.0);
        let t = scrub_target(secs(10.0), Some(secs(100.0)), scrub_fraction(40.0, 400.0));
        assert!(t > secs(10.0), "rightward drag went backwards: {t:?}");
    }

    #[test]
    fn a_leftward_drag_scrubs_backward() {
        let t = scrub_target(secs(50.0), Some(secs(100.0)), scrub_fraction(-40.0, 400.0));
        assert!(t < secs(50.0), "leftward drag went forwards: {t:?}");
    }

    #[test]
    fn a_full_width_drag_covers_the_whole_duration() {
        let t = scrub_target(secs(0.0), Some(secs(100.0)), scrub_fraction(400.0, 400.0));
        assert_eq!(t, secs(100.0));
    }

    #[test]
    fn scrubbing_clamps_to_the_media() {
        assert_eq!(scrub_target(secs(5.0), Some(secs(100.0)), -10.0), secs(0.0));
        assert_eq!(scrub_target(secs(95.0), Some(secs(100.0)), 10.0), secs(100.0));
    }

    /// Live media has no duration. A control that silently does nothing reads
    /// as broken, so a full-width drag is worth a fixed 60 seconds.
    #[test]
    fn unknown_duration_still_scrubs_by_a_fixed_window() {
        let t = scrub_target(secs(100.0), None, 1.0);
        assert_eq!(t, secs(160.0));
        let back = scrub_target(secs(100.0), None, -1.0);
        assert_eq!(back, secs(40.0));
    }

    #[test]
    fn a_zero_width_surface_does_not_divide_by_zero() {
        assert_eq!(scrub_fraction(40.0, 0.0), 0.0);
    }

    #[test]
    fn progress_is_none_without_a_duration() {
        assert!(progress(secs(10.0), None).is_none());
        assert!(progress(secs(10.0), Some(Duration::ZERO)).is_none());
    }

    #[test]
    fn progress_clamps_past_the_end() {
        assert_eq!(progress(secs(50.0), Some(secs(100.0))), Some(0.5));
        assert_eq!(progress(secs(150.0), Some(secs(100.0))), Some(1.0));
    }

    #[test]
    fn times_format_with_hours_only_when_there_are_hours() {
        assert_eq!(format_time(secs(0.0)), "0:00");
        assert_eq!(format_time(secs(65.0)), "1:05");
        assert_eq!(format_time(secs(3599.0)), "59:59");
        assert_eq!(format_time(secs(3600.0)), "1:00:00");
        assert_eq!(format_time(secs(3725.0)), "1:02:05");
    }

    #[test]
    fn an_empty_action_is_recognisable() {
        assert!(ViewAction::default().is_empty());
        assert!(!ViewAction {
            toggle_play: true,
            ..Default::default()
        }
        .is_empty());
    }
}

#[cfg(test)]
mod apply_tests {
    use super::*;
    use crate::gestures::{Intent, ViewLock};

    fn t() -> Transport {
        Transport {
            position: Duration::from_secs(50),
            duration: Some(Duration::from_secs(200)),
            playing: true,
            ..Default::default()
        }
    }

    fn run(intents: &[Intent]) -> (Transport, ViewAction, VideoView, Vec<String>) {
        let mut v = VideoView::new();
        let mut tr = t();
        let mut a = ViewAction::default();
        let mut hud = Vec::new();
        for i in intents {
            if let Some(s) = v.apply(*i, &mut tr, &mut a) {
                hud.push(s);
            }
        }
        (tr, a, v, hud)
    }

    #[test]
    fn a_scrub_reports_a_live_target_without_seeking() {
        let (_, a, _, hud) = run(&[Intent::ScrubBy(10.0)]);
        assert_eq!(a.scrubbing_to, Some(Duration::from_secs(60)));
        assert!(a.seek_to.is_none(), "scrub must not seek until commit");
        assert!(hud[0].contains("1:00"), "hud: {hud:?}");
    }

    #[test]
    fn a_commit_seeks_and_clears_the_live_target() {
        let (_, a, v, _) = run(&[Intent::ScrubBy(10.0), Intent::ScrubCommit(10.0)]);
        assert_eq!(a.seek_to, Some(Duration::from_secs(60)));
        assert!(a.scrubbing_to.is_none());
        // The accumulator must reset, or the next scrub starts pre-loaded.
        assert_eq!(v.scrub_seconds, 0.0);
    }

    #[test]
    fn scrubbing_past_the_end_clamps() {
        let (_, a, _, _) = run(&[Intent::ScrubCommit(9999.0)]);
        assert_eq!(a.seek_to, Some(Duration::from_secs(200)));
        let (_, a, _, _) = run(&[Intent::ScrubCommit(-9999.0)]);
        assert_eq!(a.seek_to, Some(Duration::ZERO));
    }

    #[test]
    fn speed_and_volume_steps_land_on_the_transport() {
        let (tr, _, _, hud) = run(&[Intent::SpeedStep(1), Intent::VolumeStep(-1)]);
        assert_eq!(tr.speed, 1.05);
        assert_eq!(tr.volume, 0.95);
        assert!(hud.iter().any(|h| h.contains("1.05x")), "{hud:?}");
        assert!(hud.iter().any(|h| h.contains("95%")), "{hud:?}");
    }

    #[test]
    fn rotation_accumulates_and_wraps() {
        let (tr, _, _, _) = run(&[Intent::Rotate(1), Intent::Rotate(1)]);
        assert_eq!(tr.rotation, 2);
        let (tr, _, _, hud) = run(&[Intent::Rotate(-1)]);
        assert_eq!(tr.rotation, 3);
        assert!(hud[0].contains("270"), "{hud:?}");
    }

    /// A view lock is a mode, not a magnification: it must clear the free zoom
    /// as well as setting the fit, or the picture snaps to fill AND stays 2x.
    #[test]
    fn a_view_lock_sets_the_fit_and_clears_the_zoom() {
        let mut v = VideoView::new();
        let mut tr = t();
        let mut a = ViewAction::default();
        v.apply(Intent::ZoomStep(1), &mut tr, &mut a);
        assert_ne!(tr.zoom, 1.0);

        v.apply(Intent::LockView(ViewLock::Fill), &mut tr, &mut a);
        assert_eq!(tr.zoom, 1.0, "lock left a stale zoom");
        assert_eq!(v.fit, Fit::Cover);

        v.apply(Intent::LockView(ViewLock::Fit), &mut tr, &mut a);
        assert_eq!(v.fit, Fit::Contain);
    }

    #[test]
    fn toggle_play_is_reported_not_applied() {
        let (tr, a, _, _) = run(&[Intent::TogglePlay]);
        assert!(a.toggle_play);
        // The widget must not decide playback state itself; the player may
        // refuse, and a UI that flipped anyway would show a lie.
        assert!(tr.playing, "widget changed playback state directly");
    }

    #[test]
    fn every_intent_produces_hud_feedback() {
        let all = [
            Intent::ScrubBy(1.0),
            Intent::ScrubCommit(1.0),
            Intent::SpeedStep(1),
            Intent::VolumeStep(1),
            Intent::Rotate(1),
            Intent::ZoomStep(1),
            Intent::LockView(ViewLock::Fit),
            Intent::TogglePlay,
        ];
        for i in all {
            let (_, _, _, hud) = run(&[i]);
            assert!(!hud.is_empty(), "{i:?} produced no feedback");
        }
    }
}

#[cfg(test)]
mod rotate_tests {
    use super::*;

    #[test]
    fn quarter_turns_swap_the_dimensions_and_half_turns_do_not() {
        let raw = Vec2::new(1920.0, 1080.0);
        assert_eq!(rotated_size(raw, 0), raw);
        assert_eq!(rotated_size(raw, 1), Vec2::new(1080.0, 1920.0));
        assert_eq!(rotated_size(raw, 2), raw);
        assert_eq!(rotated_size(raw, 3), Vec2::new(1080.0, 1920.0));
    }

    /// Rotation state is user-driven and can be negative or out of range.
    #[test]
    fn out_of_range_turns_normalise() {
        let raw = Vec2::new(1920.0, 1080.0);
        assert_eq!(rotated_size(raw, -1), Vec2::new(1080.0, 1920.0));
        assert_eq!(rotated_size(raw, 4), raw);
        assert_eq!(rotated_size(raw, 7), Vec2::new(1080.0, 1920.0));
    }

    /// The consequence the swap exists for: a rotated landscape frame must be
    /// PILLARboxed in a wide surface, not letterboxed as if it were still
    /// landscape.
    #[test]
    fn a_rotated_frame_is_pillarboxed_not_letterboxed() {
        let surface = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(800.0, 600.0));
        let raw = Vec2::new(1920.0, 1080.0);

        let flat = fit_rect(surface, rotated_size(raw, 0), PixelAspect::SQUARE, Fit::Contain);
        assert!(flat.width() > flat.height(), "unrotated should be wide");

        let turned = fit_rect(surface, rotated_size(raw, 1), PixelAspect::SQUARE, Fit::Contain);
        assert!(
            turned.height() > turned.width(),
            "rotated frame was fitted as landscape: {}x{}",
            turned.width(),
            turned.height()
        );
        assert!(turned.height() <= surface.height() + 0.01);
    }
}

#[cfg(test)]
mod paint_tests {
    use super::*;

    /// Every rotation must actually paint.
    ///
    /// This exists because `Mesh::colored_vertex` asserts the mesh has no
    /// texture, so the first version of `paint_rotated` panicked the moment
    /// anyone rotated -- and no unit test could see it, because painting needs
    /// a live context. Found by driving the gesture with xdotool and finding a
    /// black screen. Costs a few milliseconds; would have saved a crash.
    #[test]
    fn rotated_painting_does_not_panic() {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        let img = egui::ColorImage::from_rgba_unmultiplied([2, 2], &[255u8; 16]);
        let tex = ctx.load_texture("t", img, egui::TextureOptions::LINEAR);
        let painter = ctx.layer_painter(egui::LayerId::background());
        let r = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(100.0, 60.0));

        for quarter in 0..4 {
            paint_rotated(&painter, tex.id(), r, quarter);
        }
        // Negative and out-of-range turns normalise rather than index out of
        // bounds -- rotation state is user-driven and can be either.
        for quarter in [-1, -5, 7] {
            paint_rotated(&painter, tex.id(), r, quarter);
        }
        let _ = ctx.end_pass();
    }
}
