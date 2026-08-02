//! The egui gitmind shell: uikit-egui theme + adaptive nav (sidebar on desktop, bottom bar on the
//! phone) around per-tab pages that call `cockpit` directly. This is the main GUI; the Qt/QML
//! frontend remains as a fallback.
use crate::pages;
use crate::task::Task;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Fleet,
    Push,
    Network,
    Apps,
    Import,
    Branches,
    Files,
    Timeline,
    Artifacts,
    Share,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 11] = [Tab::Fleet, Tab::Push, Tab::Network, Tab::Share, Tab::Apps, Tab::Import, Tab::Branches, Tab::Files, Tab::Artifacts, Tab::Timeline, Tab::Settings];
    pub fn label(self) -> &'static str {
        match self {
            Tab::Fleet => "Fleet",
            Tab::Push => "Push",
            Tab::Network => "Network",
            Tab::Share => "Share",
            Tab::Apps => "Apps",
            Tab::Import => "Import",
            Tab::Branches => "Branches",
            Tab::Files => "Files",
            Tab::Artifacts => "Artifacts",
            Tab::Timeline => "Timeline",
            Tab::Settings => "Settings",
        }
    }

    /// A glyph for the nav (icon-only on the phone's bottom bar, icon + label on the desktop sidebar).
    pub fn icon(self) -> &'static str {
        match self {
            Tab::Fleet => "🛰",
            Tab::Push => "⬆",
            Tab::Network => "🌐",
            Tab::Share => "📡",
            Tab::Apps => "📦",
            Tab::Import => "📥",
            Tab::Branches => "🌿",
            Tab::Files => "📁",
            Tab::Artifacts => "🧹",
            Tab::Timeline => "🕘",
            Tab::Settings => "⚙",
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
    pub artifacts: pages::artifacts::State,
    pub share: pages::share::State,
    pub timeline: pages::timeline::State,
    pub import: pages::import::State,
    pub branches: pages::branches::State,
    pub dupes: pages::dupes::State,
    pub files: pages::files::State,
    /// Horizontal swipe-between-tabs gesture (with detent), used on the phone's bottom-bar layout.
    pub nav_gesture: uikit_egui::GestureLayer,
}

impl Default for App {
    fn default() -> Self {
        Self {
            artifacts: Default::default(),
            share: Default::default(),
            tab: Tab::Network,
            prefs: {
                // System palette: the minimal neutral-dark "system utilities" theme, shared with
                // rusty-board. (Not the blue Night — that matched the Qt shell but read too strong.)
                let mut p = settings_kit::Prefs::default();
                p.palette = "system".to_string();
                p
            },
            task: Task::default(),
            net: pages::network::State::default(),
            fleet: pages::fleet::State::default(),
            push: pages::push::State::default(),
            apps: pages::apps::State::default(),
            timeline: pages::timeline::State::default(),
            import: pages::import::State::default(),
            branches: pages::branches::State::default(),
            dupes: pages::dupes::State::default(),
            files: pages::files::State::default(),
            nav_gesture: uikit_egui::GestureLayer::horizontal_page(320.0),
        }
    }
}

impl App {
    /// One frame. Called from the eframe `update`.
    ///
    /// `#[allow(deprecated)]`: egui 0.34 deprecates the top-level `Panel::show(ctx)` in favour of
    /// `show_inside(ui)`, but eframe hands app code only a `Context` (no root `Ui`), so there is no
    /// non-deprecated way to render top-level panels yet. Using `Panel::left/bottom` (not the
    /// deprecated `SidePanel`/`TopBottomPanel` aliases) is as clean as 0.34 allows here; revisit
    /// when eframe exposes a root Ui.
    #[allow(deprecated)]
    pub fn ui(&mut self, ctx: &egui::Context) {
        uikit_egui::apply_prefs(ctx, &self.prefs);
        // Adopt the user's gesture feel (incl. the detent animation) for swipe-between-tabs nav.
        *self.nav_gesture.config_mut() = self.prefs.gesture_config(320.0);
        let adaptive = uikit_egui::adaptive::Adaptive::detect(ctx);
        let compact = adaptive.is_compact();

        if compact {
            egui::Panel::bottom("nav").show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| self.nav_buttons(ui, true));
            });
        } else {
            egui::Panel::left("nav").resizable(false).show(ctx, |ui| {
                ui.add_space(6.0);
                ui.heading("gitmind");
                ui.add_space(6.0);
                ui.vertical(|ui| self.nav_buttons(ui, false));
            });
        }

        // ── Global task strip ──────────────────────────────────────────────────────────────
        // A build/deploy started on Fleet keeps running when the user wanders to Files — before
        // this, nothing anywhere said so, and the finished result could silently land on a page
        // they'd left. One slim strip above the nav (bottom of the window on desktop, above the
        // bottom bar on the phone) shows the spinner + the newest log line on EVERY tab while a
        // task runs. It also owns the request_repaint for polling. NOTE: added before the
        // CentralPanel — egui panels claim space in declaration order and the central panel must
        // come last, or the strip would overlap the page content instead of shrinking it.
        #[allow(deprecated)]
        if self.task.is_running() {
            egui::Panel::bottom("task_strip").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    let tail = self.task.log();   // Vec<String>, one entry per log line
                    let last = tail.last().map(String::as_str).unwrap_or("working…");
                    ui.label(egui::RichText::new(last).monospace().small().weak());
                });
            });
            ctx.request_repaint();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let surface = ui.max_rect();
            match self.tab {
                Tab::Network => pages::network::ui(self, ui),
            Tab::Share => pages::share::ui(self, ui),
                Tab::Apps => pages::apps::ui(self, ui),
                Tab::Import => pages::import::ui(self, ui),
                Tab::Branches => pages::branches::ui(self, ui),
                Tab::Files => pages::files::ui(self, ui),
            Tab::Artifacts => pages::artifacts::ui(self, ui),
                Tab::Timeline => pages::timeline::ui(self, ui),
                Tab::Fleet => pages::fleet::ui(self, ui),
                Tab::Push => pages::push::ui(self, ui),
                Tab::Settings => pages::settings::ui(self, ui),
            }
            // Swipe left/right between tabs on the phone, with the shared detent feel. Enabled only in
            // the compact (bottom-bar) layout; on desktop the sidebar is the nav.
            let frame = self.nav_gesture.update(ui, surface, compact);
            if let Some(dir) = frame.page_jump {
                self.switch_tab(dir.sign());
            }
        });

        // ── Desktop keyboard nav ───────────────────────────────────────────────────────────
        // Ctrl+1…9 jumps straight to a tab (the sidebar order); PgUp/PgDn cycle like the phone
        // swipe. Checked AFTER page ui so a focused TextEdit's own key handling wins first.
        ctx.input(|i| {
            if i.modifiers.ctrl {
                for (n, t) in Tab::ALL.iter().enumerate() {
                    let key = [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5,
                               egui::Key::Num6, egui::Key::Num7, egui::Key::Num8, egui::Key::Num9][n];
                    if i.key_pressed(key) {
                        self.tab = *t;
                    }
                }
            }
            if i.key_pressed(egui::Key::PageDown) { self.switch_tab(1); }
            if i.key_pressed(egui::Key::PageUp) { self.switch_tab(-1); }
        });
    }

    /// Move to the previous/next tab (wrapping), used by the swipe gesture.
    fn switch_tab(&mut self, sign: i32) {
        let n = Tab::ALL.len() as i32;
        let cur = Tab::ALL.iter().position(|&t| t == self.tab).unwrap_or(0) as i32;
        self.tab = Tab::ALL[(((cur + sign) % n + n) % n) as usize];
    }

    fn nav_buttons(&mut self, ui: &mut egui::Ui, compact: bool) {
        for t in Tab::ALL {
            // Icon-only on the phone's bottom bar; icon + label on the desktop sidebar.
            let text = if compact {
                t.icon().to_string()
            } else {
                format!("{}  {}", t.icon(), t.label())
            };
            if ui.selectable_label(self.tab == t, text).clicked() {
                self.tab = t;
            }
        }
    }
}


/// A glyph for a BigSilicon app, by name — used in the Fleet/Apps store lists so each app is
/// recognizable at a glance (the QML store rows carried per-app affordances the first port dropped).
pub fn app_icon(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.contains("voiceterm") || n.contains("voiced") || n.contains("voice") {
        "🎤"
    } else if n.contains("rusty-board") || n.contains("keyboard") || n.contains("board") {
        "⌨"
    } else if n.contains("gitmind") {
        "🧠"
    } else if n.contains("av1") || n.contains("tuner") {
        "🎬"
    } else if n.contains("snapclone") || n.contains("camera") {
        "📸"
    } else if n.contains("logscope") {
        "🔭"
    } else if n.contains("beatspace") || n.contains("beat") {
        "🎵"
    } else if n.contains("bo-suite") || n.contains("bo-") {
        "🎛"
    } else if n.contains("clawbound") || n.contains("soldier") || n.contains("paperbound") || n.contains("unendlich") {
        "🎮"
    } else if n.contains("settings") {
        "⚙"
    } else if n.contains("ambient") {
        "🌿"
    } else {
        "📦"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tab_has_an_icon_and_label() {
        for t in Tab::ALL {
            assert!(!t.icon().trim().is_empty(), "{:?} needs an icon", t);
            assert!(!t.label().is_empty());
        }
    }

    #[test]
    fn swipe_cycles_tabs_with_wraparound() {
        let mut app = App::default();
        let start = app.tab;
        app.switch_tab(1);
        assert_ne!(app.tab, start, "a swipe advances to a different tab");
        // a full forward cycle returns to the start
        app.tab = start;
        for _ in 0..Tab::ALL.len() {
            app.switch_tab(1);
        }
        assert_eq!(app.tab, start, "a full cycle wraps back");
        // backward from the first tab wraps to the last
        app.tab = Tab::ALL[0];
        app.switch_tab(-1);
        assert_eq!(app.tab, Tab::ALL[Tab::ALL.len() - 1]);
    }
}
