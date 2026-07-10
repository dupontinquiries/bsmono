//! bo-suite-egui — the B&O soundscape controller, ported to egui.
//!
//! The Qt/QML control panel becomes an egui panel: per-device volume + playback, and scene
//! presets, **swipe-paged** (Devices / Scenes / Library) via the shared gesture layer, laid
//! out by the adaptive layer, a11y-navigable, and tracking live cross-app settings. bo-suite's
//! mixer/routing engine is already Qt-free, so on-device this drives it directly; here it
//! carries a small view model so the crate builds and the logic is unit-tested.

use egui::Ui;
use uikit_egui::{a11y::semantic_toggle, A11yScene, Adaptive, GestureLayer, PaletteColors};

/// A networked speaker/device.
#[derive(Debug, Clone)]
pub struct Device {
    pub name: String,
    /// 0..=100
    pub volume: u8,
    pub playing: bool,
}

impl Device {
    pub fn set_volume(&mut self, v: i32) {
        self.volume = v.clamp(0, 100) as u8;
    }
}

/// A room/scene preset.
#[derive(Debug, Clone)]
pub struct Scene {
    pub name: String,
}

pub const VIEWS: [&str; 3] = ["Devices", "Scenes", "Library"];

/// Move the view index by a swipe sign, clamped to the ends.
pub fn page(idx: usize, sign: i32, n: usize) -> usize {
    (idx as i32 + sign).clamp(0, n as i32 - 1) as usize
}

/// Controller state + the swipe gesture + a11y scene.
pub struct Controller {
    pub view_idx: usize,
    pub devices: Vec<Device>,
    pub scenes: Vec<Scene>,
    pub active_scene: usize,
    gesture: GestureLayer,
    a11y: A11yScene,
    /// The user's selected palette id (empty ⇒ default); set from prefs in apply_settings.
    palette: String,
}

impl Controller {
    pub fn new(devices: Vec<Device>, scenes: Vec<Scene>) -> Self {
        Controller { view_idx: 0, devices, scenes, active_scene: 0, gesture: GestureLayer::horizontal_page(320.0), a11y: A11yScene::new(), palette: String::new() }
    }

    pub fn current(&self) -> &'static str {
        VIEWS[self.view_idx]
    }

    pub fn swipe(&mut self, sign: i32) {
        self.view_idx = page(self.view_idx, sign, VIEWS.len());
    }

    /// Adopt cross-app preferences (theme/text/button + swipe feel incl. detent animation).
    pub fn apply_settings(&mut self, ctx: &egui::Context, prefs: &settings_kit::Prefs) {
        uikit_egui::apply_prefs(ctx, prefs);
        *self.gesture.config_mut() = prefs.gesture_config(320.0);
        self.palette = prefs.palette.clone();
    }

    /// Render the controller.
    pub fn ui(&mut self, ui: &mut Ui, adaptive: &Adaptive) {
        let colors = uikit_egui::palette_colors(&self.palette);
        self.a11y.begin_frame();

        let surface = ui.available_rect_before_wrap();
        let frame = self.gesture.update(ui, surface, true);
        if let Some(dir) = frame.page_jump {
            self.swipe(dir.sign());
        }

        if !adaptive.is_compact() {
            ui.horizontal(|ui| {
                for (i, name) in VIEWS.iter().enumerate() {
                    if ui.selectable_label(i == self.view_idx, *name).clicked() {
                        self.view_idx = i;
                    }
                }
            });
            ui.separator();
        } else {
            ui.colored_label(colors.text_weak, format!("‹ swipe ›   {}", self.current()));
        }

        match self.view_idx {
            0 => self.devices_view(ui, &colors),
            1 => self.scenes_view(ui, &colors),
            _ => self.library_view(ui, &colors),
        }

        if let Some(edge) = frame.barrier_edge {
            uikit_egui::paint_barrier(ui, surface, frame.barrier_fraction, edge, colors.accent);
        }
    }

    fn devices_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Devices");
        // collect edits to avoid borrow issues while iterating
        let mut vol_edits: Vec<(usize, i32)> = Vec::new();
        for (i, d) in self.devices.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.colored_label(c.text, &d.name);
                let mut v = d.volume as i32;
                if ui.add(egui::Slider::new(&mut v, 0..=100).text("vol")).changed() {
                    vol_edits.push((i, v));
                }
            });
            // play toggle, tagged for a11y/voice
            semantic_toggle(&mut *ui, &mut self.a11y, 1000 + i as u64, "playing", &mut d.playing);
        }
        for (i, v) in vol_edits {
            self.devices[i].set_volume(v);
        }
    }

    fn scenes_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Scenes");
        for (i, s) in self.scenes.iter().enumerate() {
            if ui.selectable_label(i == self.active_scene, &s.name).clicked() {
                self.active_scene = i;
            }
        }
        if let Some(s) = self.scenes.get(self.active_scene) {
            ui.colored_label(c.text_weak, format!("active: {}", s.name));
        }
    }

    fn library_view(&self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Library");
        ui.colored_label(c.text_weak, "Browse ambient stems and playlists (on-device: bo-mux catalog).");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Controller {
        Controller::new(
            vec![
                Device { name: "Living Room".into(), volume: 30, playing: false },
                Device { name: "Kitchen".into(), volume: 50, playing: true },
            ],
            vec![Scene { name: "Morning".into() }, Scene { name: "Focus".into() }],
        )
    }

    #[test]
    fn paging_and_swipe_clamp() {
        let mut c = sample();
        assert_eq!(c.current(), "Devices");
        c.swipe(1);
        assert_eq!(c.current(), "Scenes");
        c.swipe(1);
        c.swipe(1); // clamp at Library
        assert_eq!(c.current(), "Library");
        c.swipe(-1);
        assert_eq!(c.current(), "Scenes");
    }

    #[test]
    fn volume_clamps() {
        let mut d = Device { name: "x".into(), volume: 10, playing: false };
        d.set_volume(150);
        assert_eq!(d.volume, 100);
        d.set_volume(-5);
        assert_eq!(d.volume, 0);
    }

    #[test]
    fn apply_settings_adopts_feel() {
        let ctx = egui::Context::default();
        let mut c = sample();
        let mut prefs = settings_kit::Prefs::default();
        prefs.gesture.animation = settings_kit::AnimationStyle::Snappy;
        c.apply_settings(&ctx, &prefs);
        // snappy detent animation → short takeover lock reached the gesture layer
        assert_eq!(c.gesture.config_mut().takeover_lock_ms, 60.0);
        assert_eq!(c.current(), "Devices");
    }
}
