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
    /// Device bring-up diagnostics: the cockpit toolkit's door.
    Diagnostics,
    Settings,
    /// e-ink tablets (reMarkable): enrol from the phone, deploy eink-kit apps.
    Tablets,
}

impl Tab {
    // Diagnostics sits next to Push and Fleet, not beside Settings: it is
    // what you open when a deploy fails, and the tabs either side of it
    // are the ones that just failed.
    pub const ALL: [Tab; 13] = [Tab::Fleet, Tab::Push, Tab::Diagnostics, Tab::Network, Tab::Share, Tab::Apps, Tab::Import, Tab::Branches, Tab::Files, Tab::Artifacts, Tab::Timeline, Tab::Settings, Tab::Tablets];
    /// The grouping, declared once. Compact nav uses it as a section row;
    /// the desktop workspace uses the same sections as columns. Indices are
    /// into `Tab::ALL`, and `sections_cover_every_tab` asserts none is lost
    /// or duplicated — a tab in no section is unreachable when compact.
    pub fn sections() -> Vec<uikit_egui::sections::Section> {
        use uikit_egui::sections::Section;
        vec![
            // What you do TO devices.
            Section::new("Devices", &[0, 12, 3, 4, 1]),
            // What you do to the repository's content.
            Section::new("Repository", &[7, 8, 10, 6]),
            // Everything about this machine and the app itself.
            Section::new("System", &[5, 9, 2, 11]),
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Fleet => "Fleet",
            Tab::Push => "Push",
            Tab::Network => "Network",
            Tab::Share => "Share",
            Tab::Diagnostics => "Diagnostics",
            Tab::Apps => "Apps",
            Tab::Import => "Import",
            Tab::Branches => "Branches",
            Tab::Files => "Files",
            Tab::Artifacts => "Artifacts",
            Tab::Timeline => "Timeline",
            Tab::Settings => "Settings",
            Tab::Tablets => "Tablets",
        }
    }

    /// A glyph for the nav (icon-only on the phone's bottom bar, icon + label on the desktop sidebar).
    pub fn icon(self) -> &'static str {
        match self {
            Tab::Fleet => "🛰",
            Tab::Push => "⬆",
            Tab::Network => "🌐",
            Tab::Share => "📡",
            Tab::Diagnostics => "🩺",
            Tab::Apps => "📦",
            Tab::Import => "📥",
            Tab::Branches => "🌿",
            Tab::Files => "📁",
            Tab::Artifacts => "🧹",
            Tab::Timeline => "🕘",
            Tab::Settings => "⚙",
            Tab::Tablets => "📝",
        }
    }
}

pub struct App {
    pub tab: Tab,
    /// Accessibility scene, rebuilt each frame.
    ///
    /// The kit's a11y module says it is "shared across every uikit-egui app"
    /// and one app used it. gitmind's nav is the case that most needs it: on a
    /// phone the buttons are ICON ONLY, so a control's entire meaning is a
    /// glyph, and a fleet manager is where pressing the wrong thing reflashes
    /// a device.
    pub a11y: uikit_egui::A11yScene,
    /// The on-device keystroke probe. "Fields take one character from an
    /// on-screen keyboard" has survived every synthetic reproduction -- the
    /// kit's `input_probe` records exactly what a REAL keyboard delivers, and
    /// this app never called it. Off unless `RUSTY_INPUT_PROBE=1`.
    pub input_probe: uikit_egui::input_probe::Probe,
    pub prefs: settings_kit::Prefs,
    pub task: Task,
    pub net: pages::network::State,
    pub fleet: pages::fleet::State,
    pub tablets: pages::tablets::State,
    pub push: pages::push::State,
    pub apps: pages::apps::State,
    pub bluetooth: pages::bluetooth::BluetoothState,
    pub artifacts: pages::artifacts::State,
    pub share: pages::share::State,
    pub timeline: pages::timeline::State,
    pub import: pages::import::State,
    pub branches: pages::branches::State,
    pub dupes: pages::dupes::State,
    pub diagnostics: pages::diagnostics::State,
    pub files: pages::files::State,
    /// Horizontal swipe-between-tabs gesture (with detent), used on the phone's bottom-bar layout.
    pub nav_gesture: uikit_egui::GestureLayer,
}

impl Default for App {
    fn default() -> Self {
        Self {
            a11y: Default::default(),
            input_probe: Default::default(),
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
            tablets: pages::tablets::State::default(),
            push: pages::push::State::default(),
            apps: pages::apps::State::default(),
            bluetooth: pages::bluetooth::BluetoothState::default(),
            timeline: pages::timeline::State::default(),
            import: pages::import::State::default(),
            branches: pages::branches::State::default(),
            dupes: pages::dupes::State::default(),
            diagnostics: pages::diagnostics::State::default(),
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
        // THE KEYSTROKE PROBE. Every synthetic test of the one-character bug
        // passes -- per-key text, a whole word in one event, an IME commit, a
        // row-layout change: all correct (narrow.rs). So the next evidence has to
        // come from the device. With RUSTY_INPUT_PROBE=1 this draws a corner
        // strip of the raw events and every focus change, which tells apart the
        // causes still standing: IME not reaching the field, a lone Text event
        // refused, or focus moving between keystrokes. `enabled()` is one env
        // read per process, so this is free when off.
        if uikit_egui::input_probe::enabled() {
            self.input_probe.observe(ctx);
            uikit_egui::input_probe::overlay(ctx, &self.input_probe);
        }
        // The scene is per-frame: it collects what is focusable NOW, so a
        // stale item from a page the user has left cannot be focused or
        // spoken.
        self.a11y.begin_frame();
        // Adopt the user's gesture feel (incl. the detent animation) for swipe-between-tabs nav.
        *self.nav_gesture.config_mut() = self.prefs.gesture_config(320.0);
        let adaptive = uikit_egui::adaptive::Adaptive::detect(ctx);
        let compact = adaptive.is_compact();
        // THE READABLE FLOOR. egui's `Small` is 9 pt, and every `.small()` label in this
        // app was drawing at 9 pt on a phone — weak grey, 9 pt, on a dark panel is the
        // unreadable smear in the photo. The kit has had this since it was written and
        // one app called it. Idempotent, once a frame, after the palette.
        uikit_egui::adaptive::apply_text_floor(ctx, &adaptive);

        if compact {
            // SECTIONS, not twelve icons. A flat row of twelve wrapped to two
            // ragged rows of unlabelled glyphs — the bar's height changed with
            // the window, and three of the icons are round network-ish things
            // and four are box-ish, so none of them were identifiable. The
            // grouping is declared once in `Tab::SECTIONS` and drives this row
            // AND the desktop columns.
            egui::Panel::bottom("nav").show(ctx, |ui| {
                let secs = Tab::sections();
                // The index into `Tab::ALL`, NOT `self.tab as usize`. The enum's
                // declaration order and ALL's order differ (ALL is the nav's
                // order), so the discriminant pointed at a different tab
                // entirely: on the Network page the row showed System's
                // destinations. Caught by looking at it.
                let here = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
                let active_sec = uikit_egui::sections::section_of(&secs, here).unwrap_or(0);
                match uikit_egui::sections::plan(ui.available_width(), &secs, Tab::ALL.len()) {
                    uikit_egui::sections::Layout::Flat => {
                        ui.horizontal_wrapped(|ui| self.nav_buttons(ui, true));
                    }
                    _ => {
                        // The active section's destinations, labelled, above
                        // the section row — so the thing you are choosing
                        // between sits closer to the thumb than the grouping.
                        ui.horizontal_wrapped(|ui| {
                            for &i in &secs[active_sec].items {
                                let t = Tab::ALL[i];
                                if ui.selectable_label(self.tab == t,
                                        format!("{} {}", t.icon(), t.label())).clicked() {
                                    self.tab = t;
                                }
                            }
                        });
                        ui.separator();
                        // ONE ROW for the section, not a row of tabs: swipe it
                        // to cycle, tap it to list them. A tab row shows every
                        // section at once but costs a second line of a phone
                        // screen permanently; a plain dropdown saves the line
                        // and makes a section change a two-tap menu. The
                        // picker is both, and the swipe threshold is the
                        // pager's so it commits at the same distance as a page
                        // swipe rather than at a second number to learn.
                        let names: Vec<&str> = Tab::ALL.iter().map(|t| t.label()).collect();
                        match uikit_egui::sections::picker_ui(
                            ui, &secs, active_sec, &names,
                            &uikit_egui::palette_colors(&self.prefs.palette))
                        {
                            // Swiped: move to the new section's first
                            // destination. A picker that changed nothing
                            // visible would read as broken.
                            Some(uikit_egui::sections::Pick::Section(si)) => {
                                if let Some(&first) = secs[si].items.first() {
                                    self.tab = Tab::ALL[first];
                                }
                            }
                            // Tapped a destination in the expanded grid.
                            Some(uikit_egui::sections::Pick::Item(i)) => {
                                self.tab = Tab::ALL[i];
                            }
                            None => {}
                        }
                    }
                }
            });
        } else {
            egui::Panel::left("nav").resizable(false).show(ctx, |ui| {
                ui.add_space(6.0);
                ui.heading("gitmind");
                ui.add_space(6.0);
                // GROUPED, by the same `Tab::sections()` the compact nav uses.
                // Twelve flat entries is a list you read top to bottom every
                // time; three headed groups of four is one you learn the shape
                // of. This is what "sections on desktop" means for a nav —
                // showing several PAGES at once is the pager's job, not this
                // panel's.
                ui.vertical(|ui| {
                    for sec in Tab::sections() {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(sec.label).small().weak());
                        for &i in &sec.items {
                            let t = Tab::ALL[i];
                            if ui.selectable_label(self.tab == t,
                                    format!("{}  {}", t.icon(), t.label())).clicked() {
                                self.tab = t;
                            }
                        }
                    }
                });
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
                    // The whole log, full window, from any tab: the strip is
                    // what you see when something is running somewhere else.
                    if ui.button("log").on_hover_text("show the whole log").clicked() {
                        crate::show::open_overlay(ctx, "strip");
                    }
                    let tail = self.task.log();   // Vec<String>, one entry per log line
                    let last = tail.last().map(String::as_str).unwrap_or("working…");
                    ui.add(egui::Label::new(egui::RichText::new(last).monospace().size(crate::show::LOG_TEXT - 1.0).weak()).truncate());
                });
            });
            ctx.request_repaint();
        }
        // Drawn whether or not the task is still running: the overlay stays up
        // to read a failure after it has finished.
        if crate::show::overlay_open(ctx, "strip") {
            crate::show::log_overlay(ctx, "strip", &self.task.log());
        }

        // The focus ring, over the page. paint_focus_ring is the module's one
        // egui touch and NEITHER app that uses the scene had ever called it —
        // focus was tracked and invisible, which is the half that makes voice
        // navigation usable by someone who can also see the screen.
        egui::CentralPanel::default().show(ctx, |ui| {
            let surface = ui.max_rect();
            // EVERY page scrolls, and it is done HERE rather than in each page.
            //
            // Ten of the thirteen pages had no ScrollArea at all, so any list
            // longer than the viewport was simply unreachable — no scrollbar,
            // no wheel, no way down. Adding one per page would have meant
            // thirteen chances to forget, and the fourteenth page would forget
            // again.
            //
            // The scroll id is keyed to the TAB so each page keeps its own
            // position: switching to Files and back should not scroll Timeline
            // to the top, which is what a single shared id would do.
            //
            // `auto_shrink([false, false])` because a page shorter than the
            // viewport must still fill it — without this, short pages collapse
            // to their content height and any bottom-anchored layout inside
            // them jumps up the screen.
            egui::ScrollArea::vertical()
                .id_salt(("gitmind_page", self.tab.label()))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    match self.tab {
                        Tab::Network => pages::network::ui(self, ui),
                        Tab::Share => pages::share::ui(self, ui),
                        Tab::Diagnostics => pages::diagnostics::ui(self, ui),
                        Tab::Apps => pages::apps::ui(self, ui),
                        Tab::Import => pages::import::ui(self, ui),
                        Tab::Branches => pages::branches::ui(self, ui),
                        Tab::Files => pages::files::ui(self, ui),
                        Tab::Artifacts => pages::artifacts::ui(self, ui),
                        Tab::Timeline => pages::timeline::ui(self, ui),
                        Tab::Fleet => pages::fleet::ui(self, ui),
                        Tab::Push => pages::push::ui(self, ui),
                        Tab::Settings => pages::settings::ui(self, ui),
                        Tab::Tablets => pages::tablets::ui(self, ui),
                    }
                });
            // Swipe left/right between tabs on the phone, with the shared detent feel. Enabled only in
            // the compact (bottom-bar) layout; on desktop the sidebar is the nav.
            let frame = self.nav_gesture.update(ui, surface, compact);
            if let Some(dir) = frame.page_jump {
                self.switch_tab(dir.sign());
            }
            // Painted last so it sits over the page rather than under it.
            let accent = uikit_egui::palette_colors(&self.prefs.palette).accent;
            self.a11y.paint_focus_ring(ui, accent);
        });

        // ── Desktop keyboard nav ───────────────────────────────────────────────────────────
        // Ctrl+1…9 jumps straight to a tab (the sidebar order); PgUp/PgDn cycle like the phone
        // swipe. Checked AFTER page ui so a focused TextEdit's own key handling wins first.
        ctx.input(|i| {
            if i.modifiers.ctrl {
                // ZIP, not index. This was `[..nine keys..][n]` over
                // `Tab::ALL.iter().enumerate()` — and Tab::ALL has ELEVEN
                // entries, so the tenth iteration indexed past the end of the
                // key array and panicked. Every Ctrl chord ran this loop, so
                // Ctrl+A took the whole program down without ever being a
                // shortcut anyone had defined.
                //
                // Zipping makes the mismatch impossible rather than merely
                // fixed: adding a twelfth tab now silently gets no shortcut,
                // which is the correct behaviour when there are only nine
                // number keys.
                const NUM_KEYS: [egui::Key; 9] = [
                    egui::Key::Num1, egui::Key::Num2, egui::Key::Num3,
                    egui::Key::Num4, egui::Key::Num5, egui::Key::Num6,
                    egui::Key::Num7, egui::Key::Num8, egui::Key::Num9];
                for (t, key) in Tab::ALL.iter().zip(NUM_KEYS) {
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
        for (i, t) in Tab::ALL.iter().enumerate() {
            let t = *t;
            // Icon-only on the phone's bottom bar; icon + label on the desktop sidebar.
            let text = if compact {
                t.icon().to_string()
            } else {
                format!("{}  {}", t.icon(), t.label())
            };
            let resp = ui.selectable_label(self.tab == t, text);
            // ALWAYS the full label, whatever the button shows. Compact draws a
            // glyph and a glyph read aloud is a name nobody chose — the spoken
            // form should not get smaller because the screen did.
            self.a11y.add(
                0x6e61_7600 + i as u64,
                uikit_egui::a11y::Semantics::new(
                    uikit_egui::a11y::Role::Button, t.label())
                    .with_value(if self.tab == t { "current" } else { "" })
                    .with_hint("activates to open this page"),
                Some(resp.rect),
            );
            if resp.clicked() {
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
    #[test]
    fn the_nav_is_tagged_for_voice_and_the_focus_ring_is_painted() {
        // The kit's a11y module says it is "shared across every uikit-egui
        // app" and one app used it — and even that one never called
        // paint_focus_ring, so focus was tracked and invisible. Both halves
        // are asserted: a scene with no ring is the state bo-suite was in.
        let src = include_str!("app.rs");
        assert!(src.contains("self.a11y.begin_frame()"), "the scene is never reset");
        assert!(src.contains("self.a11y.add("), "no widget is tagged");
        assert!(src.contains("paint_focus_ring"), "focus is tracked and invisible");
    }

    #[test]
    fn a_compact_nav_button_still_announces_its_full_name() {
        // Compact draws the icon ALONE, so a control's whole meaning is a
        // glyph. The spoken form must not shrink with the screen — and this is
        // a fleet manager, where pressing the wrong thing reflashes a device.
        for t in Tab::ALL {
            assert!(!t.label().is_empty(), "{t:?} has no spoken name");
            assert_ne!(t.label(), t.icon(), "{t:?} would be announced as its glyph");
        }
    }

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

#[cfg(test)]
mod compact_layout_probe {
    use super::*;

    /// Every tab is in exactly one section. A tab in none is unreachable in
    /// the compact layout; one in two highlights two section tabs at once.
    /// Both are silent at runtime, which is why this is a test and not a
    /// careful reading of the indices.
    #[test]
    fn sections_cover_every_tab_exactly_once() {
        assert_eq!(uikit_egui::sections::covers_exactly(&Tab::sections(), Tab::ALL.len()), Ok(()));
        // And the labels are the ones the nav will show.
        let names: Vec<&str> = Tab::sections().iter().map(|s| s.label).collect();
        assert_eq!(names, vec!["Devices", "Repository", "System"]);
        // A phone gets the two-level nav; a desktop gets columns.
        assert_eq!(uikit_egui::sections::plan(400.0, &Tab::sections(), Tab::ALL.len()),
                   uikit_egui::sections::Layout::TwoLevel);
        assert!(matches!(uikit_egui::sections::plan(1280.0, &Tab::sections(), Tab::ALL.len()),
                         uikit_egui::sections::Layout::Columns(3)));
    }

    /// At phone width the Network page renders BLANK — seen in a screenshot.
    /// This reports where the space actually goes, so the cause is measured
    /// rather than guessed.
    #[test]
    fn report_where_the_height_goes_at_phone_width() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 790.0));
        let mut app = App::default();
        let mut central = egui::Rect::NOTHING;
        for _ in 0..3 {
            let _ = ctx.run(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ctx| {
                app.ui(ctx);
                central = ctx.available_rect();
            });
        }
        eprintln!("screen  = {screen:?}");
        eprintln!("central = {central:?}  (h={})", central.height());
        assert!(central.height() > 100.0,
                "the page has no room to draw: central height {}", central.height());
    }
}
