//! The video surface widget.
//!
//! Chrome comes from `uikit-egui` rather than being redrawn here: the carved
//! painters for the transport bar, the palette bridge for colours, the
//! `GestureLayer` for the swipe feel. An app that already uses the kit gets a
//! video surface that looks like the rest of it, and a restyle of the kit
//! restyles this too.

use egui::{Color32, Rect, Response, Sense, Ui, Vec2};
use std::time::Duration;
use uikit_egui::gesture_layer::{Axis, GestureLayer};
use uikit_egui::skin::Skin;
use uikit_gesture::GestureConfig;

use crate::fit::{fit_rect, Fit, PixelAspect};
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
    /// One line for the corner: codec, size, decode path.
    pub status: String,
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            position: Duration::ZERO,
            duration: None,
            playing: false,
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
    /// Letterbox bar colour. Black by default; a palette-tinted bar reads as
    /// part of the app rather than as a hole in it.
    pub bar: Color32,
    gesture: GestureLayer,
    /// Accumulated drag since the press started, in surface fractions.
    scrub: Option<f32>,
}

impl VideoView {
    pub fn new() -> Self {
        Self {
            fit: Fit::Contain,
            pixel_aspect: PixelAspect::SQUARE,
            skin: Skin::default(),
            bar: Color32::BLACK,
            gesture: GestureLayer::new(Axis::Horizontal, GestureConfig::default()),
            scrub: None,
        }
    }

    pub fn with_fit(mut self, fit: Fit) -> Self {
        self.fit = fit;
        self
    }

    /// Paint the current frame and read input. Returns what the user asked for;
    /// the caller applies it.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        presenter: &EguiPresenter,
        transport: &Transport,
    ) -> (Response, ViewAction) {
        let surface = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(surface, Sense::click_and_drag());
        let painter = ui.painter_at(surface);
        let mut action = ViewAction::default();

        // Bars first, so the picture sits on them rather than on whatever the
        // panel behind happens to be.
        painter.rect_filled(surface, 0.0, self.bar);

        match presenter.texture() {
            Some(tex) => {
                let r = fit_rect(surface, presenter.frame_size(), self.pixel_aspect, self.fit);
                if r.width() > 0.0 && r.height() > 0.0 {
                    painter.image(
                        tex.id(),
                        r,
                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }
            None => {
                // Say why there is no picture. A black rectangle is the same
                // pixels as a decode failure, and the difference is the whole
                // question when something is wrong.
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

        // A tap that is not a drag toggles playback.
        if response.clicked() {
            action.toggle_play = true;
        }

        // Horizontal drag scrubs. The gesture layer is fed so its takeover and
        // widget-suppression rules apply here too -- without it, a drag that
        // began on a control underneath would also scrub.
        let _g = self.gesture.update(ui, surface, false);
        if response.dragged() {
            let dx = response.drag_delta().x;
            let acc = self.scrub.unwrap_or(0.0) + scrub_fraction(dx, surface.width());
            self.scrub = Some(acc);
            action.scrubbing_to = Some(scrub_target(transport.position, transport.duration, acc));
        } else if response.drag_stopped() {
            if let Some(acc) = self.scrub.take() {
                action.seek_to = Some(scrub_target(transport.position, transport.duration, acc));
                // A drag that ends is a seek, not also a tap.
                action.toggle_play = false;
            }
        }

        (response, action)
    }
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
