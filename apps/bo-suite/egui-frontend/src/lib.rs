//! bo-suite-egui — the B&O soundscape controller, ported to egui.
//!
//! The Qt/QML control panel becomes an egui panel: per-device volume + playback, and scene
//! presets, **swipe-paged** (Devices / Scenes / Library) via the shared gesture layer, laid
//! out by the adaptive layer, a11y-navigable, and tracking live cross-app settings. bo-suite's
//! mixer/routing engine is already Qt-free, so on-device this drives it directly; here it
//! carries a small view model so the crate builds and the logic is unit-tested.

pub mod transport;
pub mod spaces;
pub mod library;
pub mod catalog_source;
pub mod device;
pub mod settings_page;
pub mod chrome;
pub mod dialogs;
pub mod soundscape;
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

// Six views, not three. The port had eight ported modules and three views, two
// of which were a heading and a sentence — "Browse ambient stems and playlists
// (on-device: bo-mux catalog)" is a promise, not a library.
/// Where the room is stored. Constants so the save and the restore cannot
/// point at different files.
pub const ROUTING_APP: &str = "bo-suite";
pub const ROUTING_FILE: &str = "routing.conf";

pub const VIEWS: [&str; 6] = ["Devices", "Spaces", "Library", "Mixer", "Soundscape", "Settings"];

/// Move the view index by a swipe sign, clamped to the ends.
pub fn page(idx: usize, sign: i32, n: usize) -> usize {
    (idx as i32 + sign).clamp(0, n as i32 - 1) as usize
}

/// Controller state + the swipe gesture + a11y scene.
pub struct Controller {
    pub view_idx: usize,
    /// The ported surfaces, held across frames so they keep their state — a
    /// library that forgot its page, or a mixer that forgot its bands, is the
    /// same empty demo by another route.
    pub transport: crate::transport::Transport,
    pub routing: crate::spaces::Routing,
    /// Search state plus the catalogue it filters. The library module models
    /// SEARCH over a track list rather than owning a paged list, so the tracks
    /// live here and the search is the view state.
    pub search: crate::library::Search,
    /// The catalogue, fed by a source the host supplies. Previously this was a
    /// `Vec::new()` that no code ever filled — a connected placeholder that the
    /// wiring checker reported as done.
    pub catalog: crate::catalog_source::Catalog,
    pub tracks: Vec<crate::library::Track>,
    pub plays: Vec<crate::library::Play>,
    pub mixer: crate::device::Mixer,
    pub profile: crate::device::Profile,
    pub soundscape: crate::soundscape::Soundscape,
    pub settings: crate::settings_page::SettingsPage,
    pub sinks: crate::settings_page::SinkPicker,
    pub now: crate::chrome::NowPlaying,
    pub toast: crate::chrome::Toast,
    pub add_to_playlist: crate::dialogs::AddToPlaylist,
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
        Controller { view_idx: 0, devices, scenes, active_scene: 0,
            transport: Default::default(),
            routing: Default::default(),
            search: Default::default(),
            catalog: Default::default(),
            tracks: Vec::new(),
            plays: Vec::new(),
            mixer: Default::default(),
            profile: Default::default(),
            soundscape: Default::default(),
            settings: Default::default(),
            sinks: Default::default(),
            now: Default::default(),
            toast: Default::default(),
            add_to_playlist: Default::default(), gesture: GestureLayer::horizontal_page(320.0), a11y: A11yScene::new(), palette: String::new() }
    }

    pub fn current(&self) -> &'static str {
        VIEWS[self.view_idx]
    }

    /// Switch view by a swipe sign (wraps at the ends — clamped, see `page()`). Pure index arithmetic;
    /// used by tests. NOT gesture-driven in the live UI any more — compact mode uses the vertical-tabs
    /// sidebar instead of a whole-surface swipe (see `ui()`'s doc comment for why: the Devices view's
    /// volume Sliders claim horizontal drag themselves, and a whole-surface gesture reads the same raw
    /// pointer trajectory with nothing arbitrating between the two).
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

        if !adaptive.is_compact() {
            ui.horizontal(|ui| {
                for (i, name) in VIEWS.iter().enumerate() {
                    if ui.selectable_label(i == self.view_idx, *name).clicked() {
                        self.view_idx = i;
                    }
                }
            });
            ui.separator();
            match self.view_idx {
                0 => self.devices_view(ui, &colors),
                1 => self.spaces_view(ui, &colors),
                2 => self.library_view(ui, &colors),
                3 => self.mixer_view(ui, &colors),
                4 => self.soundscape_view(ui, &colors),
                _ => self.settings_view(ui, &colors),
            }
            // The transport bar is always on screen, which is the point of a
            // controller: whatever page you are on, you can pause.
            ui.separator();
            self.transport_strip(ui, &colors);
        } else {
            // Compact width: a vertical-tabs sidebar instead of a whole-surface swipe gesture. The
            // devices view has volume Sliders, which claim horizontal drag themselves — a whole-surface
            // GestureLayer reads the same raw pointer trajectory with nothing arbitrating between the
            // two (the exact rusty-board keyboard bug: see uikit_egui::vertical_tabs's doc comment), so
            // dragging a slider could also register as a page swipe. A sidebar tap has no such race.
            ui.horizontal(|ui| {
                ui.allocate_ui(egui::vec2(64.0_f32.max(adaptive.min_touch_target * 1.3), ui.available_height()), |ui| {
                    let tabs: Vec<uikit_egui::vertical_tabs::Tab> = VIEWS.iter().map(|n| uikit_egui::vertical_tabs::Tab::new(n)).collect();
                    if let Some(i) = uikit_egui::vertical_tabs::show(ui, &tabs, self.view_idx, adaptive.min_touch_target, &colors) {
                        self.view_idx = i;
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    match self.view_idx {
                        0 => self.devices_view(ui, &colors),
                        1 => self.scenes_view(ui, &colors),
                        _ => self.library_view(ui, &colors),
                    }
                });
            });
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

    fn library_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Library");
        // The real paged list: sort tabs, search, and the empty state that says
        // WHICH empty it is.
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.search.query);
            if !self.search.tag.is_empty() {
                if ui.button(format!("tag: {} ×", self.search.tag)).clicked() {
                    self.search.tag.clear();
                }
            }
        });
        // the tag cloud, most-used first
        let tags = crate::library::Search::tags(&self.tracks);
        if !tags.is_empty() && self.search.tag.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for (t, n) in tags.iter().take(8) {
                    if ui.small_button(format!("{t} ({n})")).clicked() {
                        self.search.tag = t.clone();
                    }
                }
            });
        }
        ui.separator();
        // Prefer the real catalogue. `self.tracks` remains for the in-memory
        // demo path; when a source has been supplied, the catalogue is the
        // truth and the search filters what it returned.
        if self.catalog.loaded {
            if let Some(why) = self.catalog.empty_reason() {
                ui.colored_label(c.text_weak, why);
                return;
            }
            let q = self.search.query.trim().to_lowercase();
            let hits: Vec<crate::catalog_source::CatalogTrack> = self.catalog.rows.iter()
                .filter(|t| q.is_empty() || t.label().to_lowercase().contains(&q)
                            || t.sub().to_lowercase().contains(&q))
                .cloned().collect();
            if hits.is_empty() { ui.colored_label(c.text_weak, "nothing matches"); return; }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for t in hits {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(false, t.label()).clicked() {
                            self.now.title = t.label();
                            self.now.artist = t.artists.join(", ");
                            self.transport.title = t.label();
                            self.transport.artist = self.now.artist.clone();
                            self.transport.playing = true;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.colored_label(c.text_weak, t.sub());
                        });
                    });
                }
            });
            return;
        }
        let rows: Vec<crate::library::Track> =
            self.search.results(&self.tracks).into_iter().cloned().collect();
        if rows.is_empty() {
            // Which empty it is: an unsearched library and one with no matches
            // look the same otherwise.
            ui.colored_label(c.text_weak,
                if self.tracks.is_empty() { "catalogue not opened" } else { "nothing matches" });
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for t in rows {
                ui.horizontal(|ui| {
                    if ui.selectable_label(false, t.title.as_str()).clicked() {
                        self.now.title = t.title.clone();
                        self.now.artist = t.artist.clone();
                        self.transport.title = t.title.clone();
                        self.transport.artist = t.artist.clone();
                        self.transport.duration = 210.0;
                        self.transport.playing = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.colored_label(c.text_weak, t.album.as_str());
                        if ui.small_button("+").clicked() {
                            self.add_to_playlist.open(&t.id.to_string());
                        }
                    });
                });
            }
        });
    }

    fn spaces_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Spaces");
        crate::spaces::spaces_ui(ui, &mut self.routing, c);
        ui.separator();
        ui.horizontal(|ui| {
            // Saving the room is the one persistence bo-suite cannot do
            // without: losing which speaker plays which channel means the room
            // sounds wrong on next launch and the user rebuilds it by ear.
            if ui.button("save room").clicked() {
                match config_kit::save(ROUTING_APP, ROUTING_FILE, &self.routing.render()) {
                    Ok(()) => self.toast.show("room saved"),
                    // Shown, not swallowed: a save that silently fails is the
                    // same lie as one that never wrote.
                    Err(e) => self.toast.show(&format!("could not save: {e}")),
                }
            }
            if ui.button("restore room").clicked() {
                match config_kit::load(ROUTING_APP, ROUTING_FILE) {
                    Some(text) => {
                        let n = self.routing.restore(&text);
                        // Say HOW MANY applied. A saved room restored onto a
                        // mesh missing two speakers is a partial restore, and
                        // silently succeeding would hide the gap.
                        self.toast.show(&format!("{n} of {} speakers restored",
                                                 self.routing.speakers.len()));
                    }
                    None => self.toast.show("no saved room yet"),
                }
            }
        });
    }

    fn mixer_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Mixer");
        if self.mixer.is_flat() {
            ui.colored_label(c.text_weak, "flat — no correction applied");
        }
        let ordered: Vec<(usize, crate::device::Band)> = self.mixer.ordered();
        for (i, band) in ordered {
            ui.horizontal(|ui| {
                ui.label(crate::device::hz_label(band.center_hz));
                let mut g = band.gain_db;
                if ui.add(egui::Slider::new(&mut g, -crate::device::MAX_GAIN_DB
                                            ..=crate::device::MAX_GAIN_DB)
                          .show_value(false)).changed() {
                    self.mixer.set_gain(i, g);
                }
                ui.label(crate::device::db_label(band.gain_db));
            });
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("reset").clicked() { self.mixer.reset(); }
            let peak = self.mixer.peak_boost_db();
            if peak > 6.0 {
                ui.colored_label(c.warning, format!("{} of boost — watch headroom",
                                                    crate::device::db_label(peak)));
            }
        });
    }

    fn soundscape_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Soundscape");
        if crate::soundscape::soundscape_ui(ui, &mut self.soundscape, c) {
            self.toast.show("loop search started");
        }
    }

    fn settings_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Settings");
        for d in self.settings.problems() {
            ui.colored_label(c.warning, format!("missing {} — {}", d.name, d.enables));
        }
        for d in self.settings.optional_missing() {
            ui.colored_label(c.text_weak, format!("optional: {} enables {}", d.name, d.enables));
        }
        ui.separator();
        ui.label(self.settings.scan.label());
        if let Some(f) = self.settings.scan.fraction() {
            ui.add(egui::ProgressBar::new(f));
        }
        if ui.add_enabled(self.settings.can_scan(), egui::Button::new("scan library")).clicked() {
            self.settings.scan = crate::settings_page::Scan::Running {
                seen: 0, total: None, path: String::new() };
        }
        ui.separator();
        // product capabilities gate what is shown, per hasControl
        ui.label(format!("{} {}", self.profile.emoji, self.profile.name));
        if self.profile.has_control("autoroute") { ui.label("autoroute available"); }
    }

    /// The always-on transport strip, plus the toast that warns over it.
    fn transport_strip(&mut self, ui: &mut Ui, c: &PaletteColors) {
        if let Some(msg) = self.toast.visible() {
            ui.colored_label(c.warning, msg);
        }
        if self.now.active() { ui.label(self.now.line()); }
        crate::transport::transport_ui(ui, &mut self.transport, c);
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
        // Six views now, not three: the port had eight ported modules and only
        // Devices/Scenes/Library, two of which were a heading and a sentence.
        assert_eq!(c.current(), "Devices");
        c.swipe(1);
        assert_eq!(c.current(), "Spaces");
        for _ in 0..10 { c.swipe(1); }        // clamp at the last view
        assert_eq!(c.current(), "Settings");
        c.swipe(-1);
        assert_eq!(c.current(), "Soundscape");
        for _ in 0..10 { c.swipe(-1); }        // clamp at the first
        assert_eq!(c.current(), "Devices");
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
