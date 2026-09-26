//! Settings.
//!
//! One struct holding what a viewer can change, and the code to push it into
//! the pieces that actually read it — `GestureConfig`, `ChromePolicy`, the fit
//! mode. Kept separate from those so that a settings panel does not become the
//! place gesture rules or fade timings are defined, and so the defaults are in
//! one place rather than scattered across four `Default` impls.
//!
//! # Every value here is clamped on the way in
//!
//! A settings panel is the one part of an app that hands a user a number and
//! lets them type anything. A hide delay of zero makes the UI unusable — it
//! vanishes before it can be clicked — and a gesture detent of zero makes every
//! pixel of movement a volume change. Neither is a state the app can be
//! recovered from without editing a config file, so the clamps are here rather
//! than in the widget: a slider that only offers sane values is not a
//! guarantee, since the same struct is loaded from disk.

use std::time::Duration;

use crate::chrome::ChromePolicy;
use crate::fit::Fit;
use crate::gestures::GestureConfig;

/// How the picture fills its surface.
///
/// "Bleed" is the plain-language name for `Fit::Cover`: the picture overflows
/// the letterbox bars rather than sitting inside them, cropping the long axis.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FillMode {
    /// Whole frame visible, bars on the short axis.
    Fit,
    /// Fill the surface, cropping. The picture bleeds past the letterbox.
    Bleed,
    /// Ignore the source aspect entirely.
    Stretch,
}

impl FillMode {
    pub fn to_fit(self) -> Fit {
        match self {
            FillMode::Fit => Fit::Contain,
            FillMode::Bleed => Fit::Cover,
            FillMode::Stretch => Fit::Stretch,
        }
    }

    pub fn from_fit(f: Fit) -> Self {
        match f {
            Fit::Contain => FillMode::Fit,
            Fit::Cover => FillMode::Bleed,
            Fit::Stretch => FillMode::Stretch,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FillMode::Fit => "fit — whole frame, bars on the short side",
            FillMode::Bleed => "bleed — fill the screen, crop the edges",
            FillMode::Stretch => "stretch — ignore the source shape",
        }
    }

    pub const ALL: [FillMode; 3] = [FillMode::Fit, FillMode::Bleed, FillMode::Stretch];
}

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    /// Quality as an intent. See `vidkit_core::quality` — this moves the
    /// selector's ceiling rather than naming a stream.
    pub quality: vidkit_core::Quality,
    pub fps_cap: vidkit_core::FpsCap,
    pub fill: FillMode,
    /// In-app rotation in quarter turns, 0..=3. Separate from the device's own
    /// orientation: a phone lying flat has no useful orientation to follow, and
    /// a video shot sideways needs correcting regardless of how the phone is
    /// held.
    pub rotation: i32,
    /// Seconds of playback before the controls fade.
    pub hide_after_secs: f32,
    /// Multiplier on gesture travel. Above 1 is less sensitive.
    pub gesture_sensitivity: f32,
    /// Whether a vertical drag on the right edge rotates. Off gives the whole
    /// right half to speed, which is the other reading of the original request.
    pub edge_rotate: bool,
    /// Keep the screen awake while watching.
    pub keep_screen_on: bool,
    /// Prefer a smaller stream on battery.
    pub save_data_on_battery: bool,
    /// Upload planes and convert on the GPU rather than converting on the CPU.
    pub gpu_convert: bool,
    /// Pointer travel (points, within 0.3 s) before motion brings the
    /// controls back: ignores a bumped couch mouse or a brushed trackpad.
    /// 0 = any motion.
    pub pointer_deadzone: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            quality: Default::default(),
            fps_cap: Default::default(),
            fill: FillMode::Fit,
            rotation: 0,
            hide_after_secs: 3.0,
            gesture_sensitivity: 1.0,
            edge_rotate: true,
            keep_screen_on: true,
            save_data_on_battery: true,
            // Off until it has run on real hardware; llvmpipe is not turnip.
            gpu_convert: false,
            pointer_deadzone: 6.0,
        }
    }
}

/// Bounds, in one place so the widget and a loaded file agree.
pub mod limits {
    pub const HIDE_AFTER: (f32, f32) = (1.0, 30.0);
    pub const SENSITIVITY: (f32, f32) = (0.5, 3.0);
    pub const DEADZONE: (f32, f32) = (0.0, 40.0);
}

impl Settings {
    /// Force every field into range.
    ///
    /// Called after loading and after any edit, because the widget is not the
    /// only way values arrive.
    pub fn sanitize(&mut self) {
        self.hide_after_secs = self
            .hide_after_secs
            .clamp(limits::HIDE_AFTER.0, limits::HIDE_AFTER.1);
        self.gesture_sensitivity = self
            .gesture_sensitivity
            .clamp(limits::SENSITIVITY.0, limits::SENSITIVITY.1);
        self.rotation = self.rotation.rem_euclid(4);
        self.pointer_deadzone = if self.pointer_deadzone.is_finite() { self.pointer_deadzone.clamp(limits::DEADZONE.0, limits::DEADZONE.1) } else { 6.0 };
        if !self.hide_after_secs.is_finite() {
            self.hide_after_secs = Self::default().hide_after_secs;
        }
        if !self.gesture_sensitivity.is_finite() {
            self.gesture_sensitivity = 1.0;
        }
    }

    pub fn chrome_policy(&self) -> ChromePolicy {
        let d = ChromePolicy::default();
        ChromePolicy {
            hide_after: Duration::from_secs_f32(
                self.hide_after_secs
                    .clamp(limits::HIDE_AFTER.0, limits::HIDE_AFTER.1),
            ),
            ..d
        }
    }

    /// Scale the gesture thresholds without redefining them here — the base
    /// values stay the property of `GestureConfig`.
    pub fn gesture_config(&self) -> GestureConfig {
        let base = GestureConfig::default();
        let s = self
            .gesture_sensitivity
            .clamp(limits::SENSITIVITY.0, limits::SENSITIVITY.1);
        GestureConfig {
            detent_px: base.detent_px * s,
            axis_lock_px: base.axis_lock_px * s,
            rotate_px: base.rotate_px * s,
            rotate_edge_fraction: if self.edge_rotate {
                base.rotate_edge_fraction
            } else {
                0.0
            },
            ..base
        }
    }

    /// The budget to select against, with the viewer's intent applied.
    pub fn budget(&self, device: &vidkit_core::caps::PlaybackBudget) -> vidkit_core::caps::PlaybackBudget {
        vidkit_core::quality::apply(device, self.quality, self.fps_cap)
    }

    pub fn fit(&self) -> Fit {
        self.fill.to_fit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The setting must actually reach the selector, and only ever tighten it.
    #[test]
    fn the_quality_setting_lowers_the_budget_it_is_given() {
        let device = vidkit_core::caps::desktop_full_hw().budget;
        let auto = Settings::default();
        assert_eq!(auto.budget(&device), device, "Automatic changed the budget");

        let saver = Settings {
            quality: vidkit_core::Quality::DataSaver,
            ..Default::default()
        };
        let b = saver.budget(&device);
        assert!(b.display_width.max(b.display_height) < device.display_width.max(device.display_height));
    }

    #[test]
    fn an_fps_cap_reaches_the_budget() {
        let device = vidkit_core::caps::desktop_full_hw().budget;
        let s = Settings {
            fps_cap: vidkit_core::FpsCap::Cap30,
            ..Default::default()
        };
        assert_eq!(s.budget(&device).display_max_hz, 30);
    }

    #[test]
    fn fill_modes_round_trip_through_fit() {
        for m in FillMode::ALL {
            assert_eq!(FillMode::from_fit(m.to_fit()), m);
            assert!(!m.label().is_empty());
        }
    }

    /// "Bleed" is the plain-language name for cover; a settings panel saying
    /// "Cover" explains nothing to the person reading it.
    #[test]
    fn bleed_is_cover() {
        assert_eq!(FillMode::Bleed.to_fit(), Fit::Cover);
        assert!(FillMode::Bleed.label().contains("crop"));
    }

    /// A hide delay of zero makes the UI vanish before it can be clicked, and
    /// there is no way back from inside the app.
    #[test]
    fn an_unusable_hide_delay_is_clamped() {
        let mut s = Settings {
            hide_after_secs: 0.0,
            ..Default::default()
        };
        s.sanitize();
        assert!(s.hide_after_secs >= limits::HIDE_AFTER.0);
        assert!(s.chrome_policy().hide_after >= Duration::from_secs(1));
    }

    /// A detent of zero makes every pixel of movement a volume change.
    #[test]
    fn zero_sensitivity_is_clamped() {
        let mut s = Settings {
            gesture_sensitivity: 0.0,
            ..Default::default()
        };
        s.sanitize();
        assert!(s.gesture_sensitivity >= limits::SENSITIVITY.0);
        assert!(s.gesture_config().detent_px > 1.0);
    }

    /// The clamp has to be in `chrome_policy` and `gesture_config` too, not
    /// only in `sanitize`: a struct loaded from disk reaches them without ever
    /// passing through the widget.
    #[test]
    fn the_derived_config_is_clamped_even_without_sanitize() {
        let wild = Settings {
            hide_after_secs: -5.0,
            gesture_sensitivity: 0.0,
            ..Default::default()
        };
        assert!(wild.chrome_policy().hide_after >= Duration::from_secs(1));
        assert!(wild.gesture_config().detent_px > 1.0);
    }

    #[test]
    fn nonsense_floats_fall_back_to_the_defaults() {
        let mut s = Settings {
            hide_after_secs: f32::NAN,
            gesture_sensitivity: f32::INFINITY,
            ..Default::default()
        };
        s.sanitize();
        assert!(s.hide_after_secs.is_finite());
        assert!(s.gesture_sensitivity.is_finite());
    }

    #[test]
    fn rotation_wraps_rather_than_growing() {
        let mut s = Settings {
            rotation: 7,
            ..Default::default()
        };
        s.sanitize();
        assert_eq!(s.rotation, 3);
        let mut s = Settings {
            rotation: -1,
            ..Default::default()
        };
        s.sanitize();
        assert_eq!(s.rotation, 3);
    }

    /// Higher sensitivity value means LESS sensitive: the finger travels
    /// further per step. Getting the direction backwards makes the slider do
    /// the opposite of its label.
    #[test]
    fn a_higher_sensitivity_value_means_more_travel_per_step() {
        let low = Settings { gesture_sensitivity: 0.5, ..Default::default() };
        let high = Settings { gesture_sensitivity: 2.0, ..Default::default() };
        assert!(
            high.gesture_config().detent_px > low.gesture_config().detent_px,
            "the sensitivity slider runs backwards"
        );
    }

    /// Turning edge-rotate off hands the whole right half to speed, which is
    /// the other reading of the original gesture request.
    #[test]
    fn disabling_edge_rotate_removes_the_strip() {
        let on = Settings::default();
        assert!(on.gesture_config().rotate_edge_fraction > 0.0);
        let off = Settings { edge_rotate: false, ..Default::default() };
        assert_eq!(off.gesture_config().rotate_edge_fraction, 0.0);
    }

    /// Scaling must not redefine the base values -- those belong to
    /// GestureConfig, and a settings panel that restates them drifts from it.
    #[test]
    fn a_sensitivity_of_one_is_the_untouched_default() {
        let s = Settings::default();
        let base = GestureConfig::default();
        let derived = s.gesture_config();
        assert!((derived.detent_px - base.detent_px).abs() < 0.001);
        assert!((derived.axis_lock_px - base.axis_lock_px).abs() < 0.001);
    }

    #[test]
    fn the_gpu_path_is_off_until_it_has_run_on_hardware() {
        assert!(!Settings::default().gpu_convert);
    }
}

// ---------------------------------------------------------------------------
// the popup
// ---------------------------------------------------------------------------

/// The controls themselves, separated from the panel chrome.
fn panel_body(ui: &mut egui::Ui, s: &mut Settings, glass: &crate::glass::GlassStyle) {
    // Hints use the glass hint colour, not the palette's: a dim grey that
    // reads on an opaque surface drops under 3:1 on a translucent one.
    let hint = |ui: &mut egui::Ui, t: &str| {
        ui.colored_label(glass.hint_text, egui::RichText::new(t).small());
    };
    ui.label("quality");
    egui::ComboBox::from_label("stream")
        .selected_text(s.quality.label())
        .show_ui(ui, |ui| {
            for q in vidkit_core::Quality::ALL {
                ui.selectable_value(&mut s.quality, q, q.label())
                    .on_hover_text(q.hint());
            }
        });
    hint(ui, s.quality.hint());
    egui::ComboBox::from_label("frame rate")
        .selected_text(s.fps_cap.label())
        .show_ui(ui, |ui| {
            for f in vidkit_core::FpsCap::ALL {
                ui.selectable_value(&mut s.fps_cap, f, f.label());
            }
        });
    hint(ui, "a lower frame rate costs less than a lower resolution, and text stays sharp");

    ui.separator();
    ui.label("picture");
    for m in FillMode::ALL {
        ui.radio_value(&mut s.fill, m, m.label());
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("rotate in app");
        if ui.small_button("↺").clicked() {
            s.rotation = (s.rotation - 1).rem_euclid(4);
        }
        ui.label(format!("{}°", s.rotation * 90));
        if ui.small_button("↻").clicked() {
            s.rotation = (s.rotation + 1).rem_euclid(4);
        }
    });

    ui.separator();
    ui.label("controls");
    ui.add(
        egui::Slider::new(&mut s.hide_after_secs, limits::HIDE_AFTER.0..=limits::HIDE_AFTER.1)
            .text("hide after (s)"),
    );
    ui.add(
        egui::Slider::new(
            &mut s.gesture_sensitivity,
            limits::SENSITIVITY.0..=limits::SENSITIVITY.1,
        )
        // Named for what moving it does, not for the field: "less
        // sensitive" reads the same direction as the number.
        .text("gesture travel (higher = less sensitive)"),
    );
    ui.checkbox(&mut s.edge_rotate, "right edge rotates")
        .on_hover_text("off gives the whole right half to speed");
    ui.add(
        egui::Slider::new(&mut s.pointer_deadzone, limits::DEADZONE.0..=limits::DEADZONE.1)
            .step_by(1.0)
            .text("mouse jitter guard (px)"),
    )
    .on_hover_text("how far the pointer must move, quickly, before the controls come back -- for a mouse on the couch or a bumped trackpad; 0 = any movement");

    ui.separator();
    ui.label("power");
    ui.checkbox(&mut s.keep_screen_on, "keep the screen on while watching");
    ui.checkbox(&mut s.save_data_on_battery, "smaller stream on battery");
    ui.checkbox(&mut s.gpu_convert, "convert on the GPU")
        .on_hover_text("uploads 1.5 bytes a pixel instead of 4; unproven on this device");

    ui.add_space(8.0);
    if ui.button("reset to defaults").clicked() {
        *s = Settings::default();
    }
    if ui.button("reset to defaults").clicked() {
        *s = Settings::default();
    }
}

/// Draw the settings popup. Returns true while it is open.
///
/// A modal window, like the palette: it is a mode, and changes take effect
/// immediately rather than behind an Apply button — every control here is
/// reversible and cheap, so a preview you can see while dragging beats a
/// confirmation step.
pub fn show(ctx: &egui::Context, open: &mut bool, s: &mut Settings) -> bool {
    show_over(ctx, open, s, None)
}

/// As [`show`], drawing a blurred copy of `backdrop` behind the panel.
///
/// The texture is the video frame; without one the tint alone is drawn, which
/// is right rather than a fallback -- there is nothing to blur.
pub fn show_over(
    ctx: &egui::Context,
    open: &mut bool,
    s: &mut Settings,
    backdrop: Option<(egui::TextureId, egui::Rect)>,
) -> bool {
    if !*open {
        return false;
    }
    let glass = crate::glass::GlassStyle::default();
    let mut still_open = true;

    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        still_open = false;
    }

    // ONE LAYER, which is the whole reason this is an Area and not a Window.
    // A Window paints its own frame over whatever is behind it, so a
    // translucent one lets the raw video through and drowns the blur -- the
    // panel came out tinted with sharp bands rather than frosted. Here the
    // blurred backdrop and the tint are composited into the same rect and the
    // content sits on top of them with no frame of its own.
    let screen = ctx.screen_rect();
    let size = egui::vec2(
        420.0_f32.min(screen.width() - 32.0),
        (screen.height() - 96.0).clamp(220.0, 640.0),
    );
    let panel = egui::Rect::from_center_size(screen.center(), size);

    egui::Area::new(egui::Id::new("settings_panel"))
        .order(egui::Order::Foreground)
        .fixed_pos(panel.min)
        .show(ctx, |ui| {
            crate::glass::draw(ui, panel, &glass, backdrop);
            // Claim the rect, or clicks fall through to the video: a modal that
            // does not eat its own input scrubs the film while you adjust it.
            ui.allocate_rect(panel, egui::Sense::click_and_drag());

            let inner = panel.shrink(14.0);
            let mut content = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            let ui = &mut content;

            ui.horizontal(|ui| {
                ui.heading("settings");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("✕").clicked() {
                        still_open = false;
                    }
                });
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(inner.height() - 52.0)
                .show(ui, |ui| panel_body(ui, s, &glass));
        });

    // Anything typed or dragged can still be out of range: the slider is not
    // the only way values arrive, and a loaded file is not validated by egui.
    s.sanitize();
    *open = still_open;
    still_open
}
