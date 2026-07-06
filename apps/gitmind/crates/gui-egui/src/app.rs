//! The egui gitmind shell: uikit-egui theme + adaptive nav (sidebar on desktop, bottom bar on the
//! phone) around per-tab pages that call `cockpit` directly. This is the main GUI; the Qt/QML
//! frontend remains as a fallback.
use crate::pages;
use crate::task::Task;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Fleet,
    Push,
    Network,
    Apps,
    Import,
    Branches,
    Files,
    Timeline,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 9] = [Tab::Fleet, Tab::Push, Tab::Network, Tab::Apps, Tab::Import, Tab::Branches, Tab::Files, Tab::Timeline, Tab::Settings];
    pub fn label(self) -> &'static str {
        match self {
            Tab::Fleet => "Fleet",
            Tab::Push => "Push",
            Tab::Network => "Network",
            Tab::Apps => "Apps",
            Tab::Import => "Import",
            Tab::Branches => "Branches",
            Tab::Files => "Files",
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
    pub fleet: pages::fleet::State,
    pub push: pages::push::State,
    pub apps: pages::apps::State,
    pub timeline: pages::timeline::State,
    pub import: pages::import::State,
    pub branches: pages::branches::State,
    pub files: pages::files::State,
}

impl Default for App {
    fn default() -> Self {
        Self {
            tab: Tab::Network,
            prefs: settings_kit::Prefs::default(),
            task: Task::default(),
            net: pages::network::State::default(),
            fleet: pages::fleet::State::default(),
            push: pages::push::State::default(),
            apps: pages::apps::State::default(),
            timeline: pages::timeline::State::default(),
            import: pages::import::State::default(),
            branches: pages::branches::State::default(),
            files: pages::files::State::default(),
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
            Tab::Apps => pages::apps::ui(self, ui),
            Tab::Import => pages::import::ui(self, ui),
            Tab::Branches => pages::branches::ui(self, ui),
            Tab::Files => pages::files::ui(self, ui),
            Tab::Timeline => pages::timeline::ui(self, ui),
            Tab::Fleet => pages::fleet::ui(self, ui),
            Tab::Push => pages::push::ui(self, ui),
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
