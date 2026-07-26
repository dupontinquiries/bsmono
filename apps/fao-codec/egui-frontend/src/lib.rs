//! fao-codec-egui — the egui-primary port of fao_app (GTK4/libadwaita), begun per
//! UI_STRATEGY.md: uikit-egui shell + pages calling the codec crates directly; the GTK
//! app stays the parity reference until every surface lands. First slice: the shell
//! (adaptive nav, system palette) and the **Encode Lab** — the page that exercises the
//! REAL encoder (`fao_codec_lib::encode_iframe_recon`) on a generated test frame at the
//! user's quality / dead-zone / chroma settings and reports compressed size, bits per
//! pixel, and PSNR. Numbers over pictures for v1: the codec math is the thing being
//! ported around, and a headless-testable report keeps this crate green in CI the way
//! the other -egui ports are. Remaining fao_app surfaces (sequence encode, block-match
//! viz, v1/v2 comparison) are the follow-up pages — the pattern is set here.

pub mod lab;
pub mod library;
pub mod player;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Lab,
    Library,
    Player,
    Settings,
}

pub struct App {
    pub tab: Tab,
    pub prefs: settings_kit::Prefs,
    pub lab: lab::State,
    pub library: library::State,
    pub player: player::State,
}

impl Default for App {
    fn default() -> Self {
        let mut prefs = settings_kit::Prefs::default();
        prefs.palette = "system".to_string();
        App { tab: Tab::Lab, prefs, lab: lab::State::default(), library: library::State::default(), player: player::State::default() }
    }
}

impl App {
    #[allow(deprecated)] // Panel::show(ctx): same eframe root-Ui limitation as gitmind-egui
    pub fn ui(&mut self, ctx: &egui::Context) {
        uikit_egui::apply_prefs(ctx, &self.prefs);
        let adaptive = uikit_egui::adaptive::Adaptive::detect(ctx);
        let compact = adaptive.is_compact();
        if compact {
            egui::Panel::bottom("nav").show(ctx, |ui| {
                ui.horizontal(|ui| self.nav(ui));
            });
        } else {
            egui::Panel::left("nav").resizable(false).show(ctx, |ui| {
                ui.add_space(6.0);
                ui.heading("fao codec");
                ui.add_space(6.0);
                self.nav(ui);
            });
        }
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Lab => lab::ui(self, ui),
            Tab::Library => library::ui(self, ui),
            Tab::Player => player::ui(self, ui),
            Tab::Settings => {
                ui.heading("Settings");
                ui.label(egui::RichText::new("palette / text follow the shared prefs").weak().small());
            }
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        for (t, label) in [(Tab::Lab, "🎞 Encode Lab"), (Tab::Library, "🔗 Library"),
                           (Tab::Player, "▶ Player"), (Tab::Settings, "⚙ Settings")] {
            if ui.selectable_label(self.tab == t, label).clicked() {
                self.tab = t;
            }
        }
    }
}
