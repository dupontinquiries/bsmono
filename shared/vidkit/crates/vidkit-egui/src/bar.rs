//! The bottom bar.
//!
//! Every option here is also reachable by gesture, and that is the point: a
//! gesture is faster once you know it and undiscoverable until you do, so the
//! bar is where you find out the controls exist. It also gives a mouse a way in
//! — a right-half vertical drag is a fine phone gesture and a poor desktop one.
//!
//! Values are shown, not just controlled: the speed and volume readouts are the
//! same numbers the gesture HUD flashes, so the two cannot disagree about the
//! state.

use egui::{Color32, Ui};
use uikit_egui::skin::Skin;
use uikit_egui::PaletteColors;

use uikit_egui::adaptive::{Adaptive, DeviceClass};

use crate::fit::Fit;
use crate::gestures::{apply_speed_step, apply_volume_step};
use crate::view::{format_time, progress, Transport};

/// What the bar was asked to do. The caller applies it, same as `ViewAction` —
/// the bar never mutates playback itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarAction {
    pub toggle_play: bool,
    pub seek_to: Option<std::time::Duration>,
    pub rotate: bool,
    pub cycle_fit: bool,
    pub reset_view: bool,
}

impl BarAction {
    pub fn is_empty(&self) -> bool {
        self == &BarAction::default()
    }
}

/// Which elements the bar shows at a given device class.
///
/// The DECISION is separated from the drawing so it can be tested: at 420
/// points -- the Fairphone target -- the full bar wraps to three rows and eats
/// a fifth of the screen, and a screenshot is a slow way to find that out
/// again after every edit.
#[derive(Clone, PartialEq, Debug)]
pub struct BarLayout {
    /// Word labels before the +/- controls ("speed", "vol").
    pub word_labels: bool,
    /// Desired progress bar width in points.
    pub progress_width: f32,
    /// The codec/decode-path status line.
    pub status: bool,
    /// Total duration alongside the position.
    pub duration: bool,
}

impl BarLayout {
    pub fn for_class(class: DeviceClass) -> Self {
        match class {
            DeviceClass::Phone => Self {
                // Dropping two words is worth a whole row here.
                word_labels: false,
                progress_width: 90.0,
                status: false,
                duration: true,
            },
            DeviceClass::Tablet => Self {
                word_labels: true,
                progress_width: 150.0,
                status: false,
                duration: true,
            },
            _ => Self {
                word_labels: true,
                progress_width: 200.0,
                status: true,
                duration: true,
            },
        }
    }
}

/// Fit modes in the order the bar cycles them.
pub fn next_fit(current: Fit) -> Fit {
    match current {
        Fit::Contain => Fit::Cover,
        Fit::Cover => Fit::Stretch,
        Fit::Stretch => Fit::Contain,
    }
}

pub fn fit_label(fit: Fit) -> &'static str {
    match fit {
        Fit::Contain => "fit",
        Fit::Cover => "fill",
        Fit::Stretch => "stretch",
    }
}

/// A compact percentage, so 1.05 reads as "105%" and never "104%".
pub fn percent(v: f64) -> String {
    format!("{}%", (v * 100.0).round() as i64)
}

/// Draw the bar. Mutates `transport` for the controls it owns outright (speed,
/// volume) and reports the rest.
pub fn bottom_bar(
    ui: &mut Ui,
    transport: &mut Transport,
    fit: Fit,
    colors: &PaletteColors,
    skin: &Skin,
    adaptive: &Adaptive,
) -> BarAction {
    let layout = BarLayout::for_class(adaptive.class);
    let mut act = BarAction::default();

    // Wrapped, not a single row. A phone surface is about 400 points wide and
    // this bar has eleven controls on it; with a plain `horizontal` the tail
    // end draws on top of the middle instead of moving to a second line. Seen
    // at 900 points, so the Fairphone target would have been far worse.
    ui.horizontal_wrapped(|ui| {
        // Play/pause first and widest: it is the only control anyone uses
        // without looking.
        let label = if transport.playing { "❚❚" } else { "▶" };
        if uikit_egui::carved::button_accent(ui, label, colors.bg_raised, colors.accent, colors.text, skin)
            .clicked()
        {
            act.toggle_play = true;
        }

        ui.label(format_time(transport.position));
        if layout.duration {
            if let Some(d) = transport.duration {
                ui.label(format!("/ {}", format_time(d)));
            }
        }

        if let Some(p) = progress(transport.position, transport.duration) {
            let bar = ui.add(
                egui::ProgressBar::new(p)
                    .desired_width(layout.progress_width)
                    .desired_height(10.0),
            );
            // Click-to-seek: the bar is the one place a precise absolute jump
            // is easy, which the gesture deliberately is not.
            if bar.clicked() {
                if let (Some(pos), Some(d)) = (bar.interact_pointer_pos(), transport.duration) {
                    let f = ((pos.x - bar.rect.min.x) / bar.rect.width().max(1.0)).clamp(0.0, 1.0);
                    act.seek_to = Some(std::time::Duration::from_secs_f64(
                        d.as_secs_f64() * f as f64,
                    ));
                }
            }
        }

        ui.separator();

        // Speed. The same 5% steps the right-half gesture uses, so the two
        // controls cannot land on different values.
        if layout.word_labels {
            ui.label("speed");
        }
        if ui.small_button("−").clicked() {
            transport.speed = apply_speed_step(transport.speed, -1);
        }
        ui.label(percent(transport.speed));
        if ui.small_button("+").clicked() {
            transport.speed = apply_speed_step(transport.speed, 1);
        }

        ui.separator();

        // The speaker glyph carries the meaning when the word is dropped;
        // uikit-egui's fonts::install is what makes it render as a glyph
        // rather than a box.
        ui.label(if layout.word_labels { "vol" } else { "🔊" });
        if ui.small_button("−").clicked() {
            transport.volume = apply_volume_step(transport.volume, -1);
        }
        ui.label(percent(transport.volume as f64));
        if ui.small_button("+").clicked() {
            transport.volume = apply_volume_step(transport.volume, 1);
        }

        ui.separator();

        if ui.small_button(format!("↻ {}°", transport.rotation * 90)).clicked() {
            act.rotate = true;
        }
        if ui.small_button(fit_label(fit)).clicked() {
            act.cycle_fit = true;
        }
        if transport.zoom != 1.0 || transport.rotation != 0 {
            // Only offered when there is something to reset, so the bar does
            // not carry a permanently dead control.
            if ui.small_button(format!("reset {}", percent(transport.zoom as f64))).clicked() {
                act.reset_view = true;
            }
        }

        // Plain flow, not a right-to-left sub-layout: that one claims the
        // remaining width and paints over whatever was already there when the
        // row is full, which is exactly what it did.
        if layout.status && !transport.status.is_empty() {
            ui.colored_label(Color32::from_gray(150), &transport.status);
        }
    });

    act
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_cycles_through_every_mode_and_returns() {
        let mut f = Fit::Contain;
        let mut seen = vec![f];
        for _ in 0..3 {
            f = next_fit(f);
            seen.push(f);
        }
        assert_eq!(
            seen,
            vec![Fit::Contain, Fit::Cover, Fit::Stretch, Fit::Contain],
            "cycle must be complete and closed"
        );
    }

    #[test]
    fn every_fit_mode_has_a_label() {
        for f in [Fit::Contain, Fit::Cover, Fit::Stretch] {
            assert!(!fit_label(f).is_empty());
        }
    }

    /// The readout must not show 104% for a value the gesture set to 1.05.
    #[test]
    fn percentages_round_rather_than_truncate() {
        assert_eq!(percent(1.05), "105%");
        assert_eq!(percent(0.95), "95%");
        assert_eq!(percent(1.0), "100%");
        assert_eq!(percent(0.0), "0%");
    }

    /// The bar's buttons and the gestures must move the same values by the
    /// same amounts, or the two disagree about state.
    #[test]
    fn bar_steps_match_gesture_steps() {
        assert_eq!(apply_speed_step(1.0, 1), 1.05);
        assert_eq!(apply_volume_step(0.5, -1), 0.45);
    }

    /// The compact layout must actually be smaller, in every dimension that
    /// costs a row. Asserting only "phone != desktop" would pass on a layout
    /// that changed something irrelevant.
    #[test]
    fn the_phone_layout_is_strictly_lighter_than_the_desktop_one() {
        let phone = BarLayout::for_class(DeviceClass::Phone);
        let desktop = BarLayout::for_class(DeviceClass::Desktop);
        assert!(!phone.word_labels && desktop.word_labels);
        assert!(!phone.status && desktop.status);
        assert!(phone.progress_width < desktop.progress_width);
    }

    #[test]
    fn the_tablet_layout_sits_between_the_two() {
        let p = BarLayout::for_class(DeviceClass::Phone);
        let t = BarLayout::for_class(DeviceClass::Tablet);
        let d = BarLayout::for_class(DeviceClass::Desktop);
        assert!(p.progress_width < t.progress_width && t.progress_width < d.progress_width);
    }

    /// Position is the one readout that survives every breakpoint: a player
    /// that cannot tell you where you are is not a player.
    #[test]
    fn the_duration_readout_survives_every_class() {
        for c in [DeviceClass::Phone, DeviceClass::Tablet, DeviceClass::Desktop] {
            assert!(BarLayout::for_class(c).duration, "{c:?} dropped the duration");
        }
    }

    #[test]
    fn an_empty_action_is_recognisable() {
        assert!(BarAction::default().is_empty());
        assert!(!BarAction {
            rotate: true,
            ..Default::default()
        }
        .is_empty());
    }
}
