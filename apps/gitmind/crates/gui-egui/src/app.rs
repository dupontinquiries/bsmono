//! The egui gitmind shell: uikit-egui theme + adaptive nav (sidebar on desktop, bottom bar on the
//! phone) around per-tab pages that call `cockpit` directly. This is the main GUI; the Qt/QML
//! frontend remains as a fallback.
use crate::pages;
use crate::task::Task;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Fleet,
    Network,
    Timeline,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Fleet, Tab::Network, Tab::Timeline, Tab::Settings];
    pub fn label(self) -> &'static str {
        match self {
            Tab::Fleet => "Fleet",
            Tab::Network => "Network",
            Tab::Timeline => "Timeline",
            Tab::Settings => "Settings",
        }
    }
}

pub struct App {
    pub tab: Tab,
    pub prefs: settings_kit::Prefs,
    pub task: Task,
    pub net: pages::network::State,
}

impl Default for App {
    fn default() -> Self {
        Self {
            tab: Tab::Network,
            prefs: settings_kit::Prefs::default(),
            task: Task::default(),
            net: pages::network::State::default(),
        }
    }
}

impl App {
    /// One frame. Called from the eframe `update`.
    pub fn ui(&mut self, ctx: &egui::Context) {
        uikit_egui::apply_prefs(ctx, &self.prefs);
        let adaptive = uikit_egui::adaptive::Adaptive::detect(ctx);

        if adaptive.is_compact() {
            egui::TopBottomPanel::bottom("nav").show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| self.nav_buttons(ui));
            });
        } else {
            egui::SidePanel::left("nav").resizable(false).show(ctx, |ui| {
                ui.add_space(6.0);
                ui.heading("gitmind");
                ui.add_space(6.0);
                ui.vertical(|ui| self.nav_buttons(ui));
            });
        }

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Network => pages::network::ui(self, ui),
            Tab::Fleet => pages::fleet::ui(self, ui),
            Tab::Settings => pages::settings::ui(self, ui),
            other => {
                ui.heading(other.label());
                ui.label("port in progress");
            }
        });
    }

    fn nav_buttons(&mut self, ui: &mut egui::Ui) {
        for t in Tab::ALL {
            if ui.selectable_label(self.tab == t, t.label()).clicked() {
                self.tab = t;
            }
        }
    }
}
