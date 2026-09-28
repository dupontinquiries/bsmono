//! shell — the thing that owns the state and calls the pages.
//!
//! Last piece of the port. The pages were all there, `chrome::Section` was all
//! there, `router` decides which one is current — and nothing held the state
//! each page needs or dispatched to it. Ten renderers, no caller.
//!
//! ## Why the state lives here and not in the pages
//!
//! Every page renderer takes its state by reference and draws it. That was the
//! right call in the port and it is why they were all individually testable —
//! but it means SOMETHING has to own the state across frames, and until now
//! nothing did. This is that owner, and it is deliberately dumb: a struct of
//! page states and a match. The interesting code is in the pages.
//!
//! ## The signatures differ, and that is the actual work
//!
//! `live_ui` wants `(&Live, &BreathingDot)`, `locations_ui` wants
//! `&mut LocationsPage`, `ambience_ui` wants `(&Ambience, &Sparkles, f32)`.
//! There is no uniform `Page` trait to hide behind, and inventing one would
//! mean rewriting ten working renderers to fit it. The match is longer and
//! nothing had to change underneath it.

use super::chrome::{self, Section};
use super::router::{self, Router};

/// From this width the Live page draws its cards beside the reading
/// rather than under it.
pub const WIDE_LIVE_PX: f32 = 700.0;

/// Everything a page can ask the host to do, in one queue.
///
/// The pages return their own action types (each page is testable on its
/// own); the shell wraps them so the host drains ONE list per frame and a
/// new page cannot add a second queue the host forgets to read — the
/// `locations_ui` return value was dropped with `let _ =` for exactly that
/// long, and the fix was making the queue the only path.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Location(super::locations::LocationAction),
    Calibrate(super::calibrate::CalibrateAction),
    History(super::history::HistoryAction),
    Sync(super::sync::SyncAction),
}

/// Everything the pages draw from.
///
/// `Default` throughout: a shell that cannot be constructed without live
/// sensor data cannot be opened before the first reading lands, and an app
/// that shows nothing until a sensor reports looks broken on launch.
pub struct Shell {
    pub router: Router,
    pub live: super::live::Live,
    pub dot: super::live::BreathingDot,
    pub history: super::history::History,
    pub locations: super::locations::LocationsPage,
    /// Screen-reader semantics, owned by the SHELL.
    ///
    /// Here rather than on a page because `A11yScene` derives only
    /// `Default` and the pages are `Debug + Clone + PartialEq`. It still
    /// outlives the frame, which is the property that matters — a scene
    /// rebuilt each frame forgets which element had focus, and that
    /// reads as the cursor jumping away mid-use.
    pub a11y: uikit_egui::a11y::A11yScene,
    pub map: super::map::MapPage,
    pub model: super::model::ModelPage,
    pub sync: super::sync::SyncPage,
    pub screen_temps: super::screen_temps::ScreenTempsPage,
    pub ambience: super::ambience::Ambience,
    /// Not Default: Sparkles is a humidity reading, and 0% is a MEANINGFUL
    /// value (dry air draws no droplets) rather than an absent one. Built
    /// explicitly so nobody reads a default as "no data".
    pub sparkles: super::ambience::Sparkles,
    /// The headline reading (eased temperature, confidence, sparkline) —
    /// the piece `Section::Live`'s dispatch was missing entirely until
    /// this field existed. See that arm's own comment for the parity
    /// gap this closes and what it deliberately does not touch yet
    /// (`main.rs` is still the one running binary; this brings `Shell`
    /// up to the SAME composition it already uses, additively).
    pub live_state: crate::LiveState,
    /// `t_ms` from the PREVIOUS frame, so this can derive its own delta —
    /// `Shell::ui` receives an absolute clock, not a delta, because
    /// `egui_lib::App` is the only current caller and has no other need
    /// for one. `None` on the very first frame, so that frame's delta is
    /// zero rather than "however long since the Unix epoch".
    last_t_ms: Option<f32>,
    /// Weather-sharing contacts. Empty rather than fabricated, matching
    /// every other page's demo-state convention here (see `main.rs`'s own
    /// comment on the same field): a demo with no snapclone connection
    /// has no contacts, which is the true state.
    pub contacts: Vec<(String, String)>,
    pub share: weather_share::WeatherShare,
    pub shared_note: Option<String>,
    /// Set when Back was pressed with no history left. The host decides what
    /// that means — on a phone it is usually "close the app", and deciding
    /// that here would make a UI module own the process lifetime.
    pub back_unhandled: bool,
    /// Actions raised by a page this frame, for the host to apply.
    ///
    /// Queued rather than handled here: adding or removing a location touches
    /// the database, and a UI module that writes to storage is the coupling
    /// the port was built to avoid. The host drains this.
    pub pending: Vec<Action>,
    /// The GTK Live page's "🎯 Calibrate" — entry, button, outcome.
    pub calibrate: super::calibrate::Calibrate,
    /// The GTK Live page's "🔮 Forecast" box.
    pub forecast: super::forecast_card::ForecastCard,
    /// Week navigation for the History page.
    pub history_nav: super::history::Nav,
    /// The GTK status lines under the reading: "Active sensors: 3 |
    /// Calibrations: 2 | Model: Linear" and "● Sensors locked". Filled by
    /// the host from the world state; empty means the host has not
    /// reported yet, and the lines are simply not drawn.
    pub status: Vec<String>,
    /// The host's own log, newest last — what it opened, polled, saved,
    /// refused. Drawn on the Debug page so the loop is visible: an app
    /// whose engine is a black box cannot be steered when a number looks
    /// wrong.
    pub host_log: Vec<String>,
}

impl Shell {
    /// How long since the last frame, given the current absolute clock.
    /// `Shell::ui` receives `t_ms` rather than a delta (its only current
    /// caller, `egui_lib::App`, tracks one clock for everything), so this
    /// derives one and remembers where it left off. Zero on the very
    /// first call — there is no "last frame" to measure against yet, and
    /// treating a startup gap as elapsed time would jump every easing
    /// on the first frame rather than starting at rest.
    fn advance_clock(&mut self, t_ms: f32) -> f32 {
        let dt = (t_ms - self.last_t_ms.unwrap_or(t_ms)).max(0.0);
        self.last_t_ms = Some(t_ms);
        dt
    }

    /// Draw the whole frame: chrome, tabs, and the current page.
    ///
    /// `t_ms` is the animation clock, passed in rather than read from the
    /// context here so the caller can freeze it — a test that cannot stop time
    /// cannot assert on an animated surface.
    pub fn ui(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors, t_ms: f32) {
        let adaptive = uikit_egui::adaptive::Adaptive::detect(ui.ctx());
        // Icons only when narrow. The labels are what wrap, and a tab bar two
        // rows deep costs more vertical space than a phone has to spare.
        let glyphs_only = adaptive.class.is_compact();

        chrome::section_header(ui, self.router.current(), colors.accent);
        // `tab_bar_ui`'s own comment used to accept "wraps to two rows on a
        // phone" as the tradeoff for showing all seven sections at once —
        // exactly the cost `uikit_egui::responsive_nav` exists to avoid.
        // Glyph-only labels are narrower than full-text ones, so the fit
        // check uses a smaller minimum width in that mode; either way this
        // is a real fit computation against `Section::ALL`'s actual count,
        // not another device-class guess layered on top of the one that
        // already picks `glyphs_only`.
        let min_tab_width = if glyphs_only { 40.0 } else { 96.0 };
        let layout = uikit_egui::responsive_nav::choose(
            ui.available_width(), chrome::Section::ALL.len(), min_tab_width);
        match layout {
            uikit_egui::responsive_nav::NavLayout::Tabs => {
                if let Some(s) = router::tab_bar_ui(ui, self.router.current(), glyphs_only) {
                    self.router.go(s);
                }
            }
            uikit_egui::responsive_nav::NavLayout::Pages => {
                // One section at a time via a picker rather than a second
                // row of icons — the vertical space the wrapped bar used
                // to spend on row two goes back to the page underneath it.
                let current = self.router.current();
                egui::ComboBox::from_id_salt("section_picker")
                    .selected_text(current.tab_label(false))
                    .show_ui(ui, |ui| {
                        for s in chrome::Section::ALL {
                            if ui.selectable_label(s == current, s.tab_label(false)).clicked() {
                                self.router.go(s);
                            }
                        }
                    });
            }
        }
        ui.separator();

        // Keyboard shortcuts, from the table Section already owns — but
        // not while a text field has the keyboard. Typing "21.5" into the
        // calibrate entry used to land on "2" as a page shortcut and switch
        // to the Model page mid-word; found by the first screenshot that
        // typed into the field.
        if !ui.ctx().egui_wants_keyboard_input() {
            let typed: Vec<char> = ui.ctx().input(|i| i.events.iter().filter_map(|e| match e {
                egui::Event::Text(t) => t.chars().next(),
                _ => None,
            }).collect());
            for c in typed { self.router.shortcut(c); }
        }

        // BACK. Written and tested two commits ago and called from nowhere —
        // found by the new tested-but-uncalled check, which is a fair first
        // catch since it is my own dead code.
        //
        // Escape and the browser/phone Back button both land here. `back`
        // returns false when there is nowhere to go, and that return is passed
        // on rather than swallowed: on a handset, swallowing Back is how an
        // app stops being closeable.
        let back_pressed = ui.ctx().input(|i| i.key_pressed(egui::Key::Escape));
        if back_pressed && !self.router.back() {
            // Nowhere to go. Left for the host to handle as a close request.
            self.back_unhandled = true;
        }

        // Every page scrolls. The GTK pages sit in a ScrolledWindow for the
        // same reason: on a 720px-tall phone the Live page's headline,
        // status, sensors, forecast and calibrate card do not fit, and a
        // page that clips its own calibrate button is a page without one.
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            self.page_ui(ui, colors, t_ms);
        });
    }

    /// The Live page's left half: headline reading, status lines, the
    /// sensor panel.
    fn live_reading_ui(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors, dt_ms: f32) {
        self.live_state.tick(dt_ms / 1000.0);
        self.live_state.ui(ui);
        for line in &self.status {
            ui.colored_label(colors.text_weak, egui::RichText::new(line).small());
        }
        self.dot.live = self.live.readings().iter().any(|(_, _, r)| r.available());
        self.dot.tick(dt_ms);
        if self.dot.live { ui.ctx().request_repaint(); }
        ui.add_space(6.0);
        super::live::live_ui(ui, &self.live, &self.dot, colors);
    }

    /// The Live page's right half: the forecast box, the calibrate card,
    /// the share row.
    fn live_cards_ui(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
        super::forecast_card::forecast_ui(ui, &self.forecast, colors);
        ui.add_space(10.0);
        if let Some(a) = super::calibrate::calibrate_ui(ui, &mut self.calibrate, colors) {
            self.pending.push(Action::Calibrate(a));
        }
        ui.add_space(12.0);
        ui.separator();
        super::share_row_ui(ui, &self.live_state.snapshot, &self.contacts,
                             &mut self.share, &mut self.shared_note);
    }

    /// The current page, inside the scroll area.
    fn page_ui(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors, t_ms: f32) {
        match self.router.current() {
            Section::Live => {
                // The GTK Live page, top to bottom: the big reading with its
                // confidence, the status lines ("Active sensors … | Model …",
                // "● Sensors locked"), the forecast box, the calibrate
                // button. The sensor panel and the share row are this
                // port's own additions and keep their places after the
                // headline and at the end.
                //
                // One column on a phone; two side by side where there is
                // room, because a 960px-wide single column puts the sensor
                // values a screen's width from their labels.
                let dt_ms = self.advance_clock(t_ms);
                if ui.available_width() >= WIDE_LIVE_PX {
                    ui.columns(2, |cols| {
                        self.live_reading_ui(&mut cols[0], colors, dt_ms);
                        self.live_cards_ui(&mut cols[1], colors);
                    });
                } else {
                    self.live_reading_ui(ui, colors, dt_ms);
                    ui.add_space(10.0);
                    self.live_cards_ui(ui, colors);
                }
            }
            Section::History => {
                if let Some(a) = super::history::history_page_ui(
                    ui, &self.history, &self.history_nav, colors) {
                    self.pending.push(Action::History(a));
                }
            }
            Section::Places => {
                // locations_ui RETURNS AN ACTION and the other pages do not.
                // Dropping it with `let _ =` would compile and would make
                // every button on the page dead — the same class of bug this
                // whole file exists to fix, one level down.
                if let Some(a) = super::locations::locations_ui(
                    ui, &mut self.locations, colors, &mut self.a11y) {
                    self.pending.push(Action::Location(a));
                }
            }
            Section::Map => super::map::map_ui(ui, &mut self.map, colors),
            Section::Model => super::model::model_ui(ui, &self.model, colors),
            Section::Sync => {
                if let Some(a) = super::sync::sync_ui(ui, &mut self.sync, colors) {
                    self.pending.push(Action::Sync(a));
                }
            }
            // DEBUG IS THE DEVELOPER SURFACES, not a placeholder. The ambience
            // background and the screen-temperature frames are both ported and
            // neither is a top-level page in the GTK build either — they live
            // behind the wrench there too.
            Section::Debug => {
                // Ambience takes an explicit RECT because it parallaxes the
                // panel beyond its bounds, so it cannot infer one from the Ui
                // without exposing an edge on the shift.
                let rect = ui.available_rect_before_wrap();
                super::ambience::ambience_ui(
                    ui, &self.ambience, &self.sparkles, t_ms, rect, colors);
                ui.separator();
                // Unix nanoseconds, because that is the clock the publisher
                // stamps frames with (`timestamp_unix_nanos`). The app's
                // own animation clock was passed here before, and every
                // frame showed an age of minus fifty-six years.
                let now_ns = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64).unwrap_or(0);
                super::screen_temps::screen_temps_ui(
                    ui, &self.screen_temps, now_ns, colors);
                ui.separator();
                chrome::card_heading(ui, "Host log");
                if self.host_log.is_empty() {
                    ui.colored_label(colors.text_weak, "nothing yet");
                }
                for line in self.host_log.iter().rev().take(40) {
                    ui.colored_label(colors.text_weak, egui::RichText::new(line).small().monospace());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_dispatches_to_a_page() {
        // The bug this file closes: ten renderers and no caller. A source
        // check because calling them needs an egui context, and the property
        // that matters is that the match has no arm which draws nothing —
        // a Section with an empty arm is a tab that opens a blank screen.
        let src = include_str!("shell.rs");
        let body = &src[src.find("match self.router.current()").expect("the dispatch moved")..];
        for s in ["Section::Live", "Section::History", "Section::Places", "Section::Map",
                  "Section::Model", "Section::Sync", "Section::Debug"] {
            let at = body.find(s).unwrap_or_else(|| panic!("{s} has no arm — its tab opens nothing"));
            // The arm must actually call a renderer, not just exist.
            // To the next arm rather than a fixed window: a comment inside an
            // arm pushed the call past 220 chars and failed a correct arm,
            // which is a test measuring the wrong thing.
            let rest = &body[at + s.len()..];
            let end = rest.find("Section::").unwrap_or(rest.len());
            let arm = &rest[..end];
            assert!(arm.contains("_ui("), "{s}'s arm draws nothing:\n{arm}");
        }
    }

    /// The parity gap this session's own PORT_STATUS.md entry named:
    /// `Section::Live` used to call only the sensor panel, missing the
    /// headline reading and the weather-share row `main.rs`'s own
    /// (separately maintained) Live page already has. A source check for
    /// the same reason the test above is one — the property that matters
    /// is that the SPECIFIC arm calls both, not merely that some `_ui(`
    /// call exists somewhere in it.
    #[test]
    fn the_live_arm_draws_the_headline_and_the_share_row_too() {
        let src = include_str!("shell.rs");
        // The composition lives in two helpers the arm calls for both the
        // one- and two-column layouts; scan from the first to the end of
        // the second.
        let start = src.find("fn live_reading_ui(").expect("the reading helper moved");
        let end = src.find("fn page_ui(").expect("page_ui moved");
        let arm = &src[start..end];
        assert!(arm.contains("live_state.ui("), "the headline reading is missing:\n{arm}");
        assert!(arm.contains("share_row_ui("), "weather-sharing is missing:\n{arm}");
        // The GTK page's two boxes under the reading.
        assert!(arm.contains("forecast_ui("), "the forecast box is missing:\n{arm}");
        assert!(arm.contains("calibrate_ui("), "the calibrate card is missing:\n{arm}");
        // and the arm itself calls both helpers in both layouts
        let body = &src[src.find("match self.router.current()").unwrap()..];
        let live = &body[body.find("Section::Live").unwrap()..];
        let live = &live[..live.find("Section::History").unwrap()];
        assert_eq!(live.matches("live_reading_ui(").count(), 2, "both layouts draw the reading");
        assert_eq!(live.matches("live_cards_ui(").count(), 2, "both layouts draw the cards");
    }

    #[test]
    fn every_page_action_lands_in_the_one_queue() {
        // Four pages return actions. Each must push into `pending`; a page
        // whose return value is dropped has dead buttons, which is the bug
        // the queue exists to make impossible to write quietly.
        let src = include_str!("shell.rs");
        let body = &src[src.find("fn live_cards_ui(").expect("the cards helper moved")..];
        for call in ["calibrate_ui(", "history_page_ui(", "locations_ui(", "sync_ui("] {
            let at = body.find(call).unwrap_or_else(|| panic!("{call} is not called"));
            let after = &body[at..at + 400];
            assert!(after.contains("self.pending.push("), "{call}'s action is dropped:\n{after}");
        }
    }

    #[test]
    fn advance_clock_measures_zero_on_the_first_frame_then_real_deltas() {
        // No "last frame" exists yet on the very first call — treating a
        // cold-start gap as elapsed time would jump every easing rather
        // than starting at rest.
        let mut s = Shell::default();
        assert_eq!(s.advance_clock(5_000.0), 0.0, "nothing to measure against yet");
        assert_eq!(s.advance_clock(5_016.0), 16.0, "a real 16ms frame");
        assert_eq!(s.advance_clock(5_016.0), 0.0, "two frames with the same clock: no time passed");
    }

    #[test]
    fn advance_clock_never_goes_negative_on_a_clock_that_moves_backward() {
        // A resumed suspend or a clock adjustment can hand back an earlier
        // timestamp than the last one seen; a negative delta would run
        // every easing backward.
        let mut s = Shell::default();
        s.advance_clock(5_000.0);
        assert_eq!(s.advance_clock(4_000.0), 0.0, "must clamp, not go negative");
    }

    #[test]
    fn the_shell_is_constructible_before_any_sensor_reports() {
        // An app that cannot be built until the first reading lands shows
        // nothing on launch, which reads as broken rather than as waiting.
        let s = Shell::default();
        assert_eq!(s.router.current(), Section::Live);
    }

    #[test]
    fn the_shortcut_path_goes_through_the_router_not_a_second_table() {
        // The router owns the mapping; the shell must delegate to the router.
        // Three copies would be two too many.
        let src = include_str!("shell.rs");
        // A SOURCE SCAN CANNOT EXPRESS THIS ONE. Any literal naming the thing
        // it forbids appears in the file it scans, so the assertion trips on
        // itself — twice now: first on the prose, then on the literal. That is
        // the check telling me it is the wrong shape.
        //
        // What actually has to hold is that the mapping exists ONCE, and the
        // right place to prove that is where it lives: router.rs already walks
        // every Section and asserts its declared shortcut opens it. If the
        // shell grew a second table those two would disagree, and the router's
        // test is what would fail.
        //
        // So this one checks only the delegation it CAN see, and points at the
        // test that covers the rest.
        assert!(src.contains("self.router.shortcut(c)"),
                "the shell no longer routes typed characters through the router");
    }

    #[test]
    fn a_page_that_mutates_gets_a_mutable_borrow_and_the_rest_do_not() {
        // locations and map take &mut because they hold selection state; the
        // others take &. Passing &mut everywhere would compile and would hide
        // which pages actually own interaction.
        let src = include_str!("shell.rs");
        assert!(src.contains("&mut self.locations"));
        assert!(src.contains("&mut self.map"));
        assert!(src.contains("&self.history"), "history should not need a mutable borrow");
    }
}


/// The whole shell through egui's headless context, every section at a
/// phone width and a desktop width, with every page holding data.
#[cfg(test)]
mod layout_tests {
    use super::*;

    fn filled() -> Shell {
        let mut s = Shell::default();
        s.status = vec!["Active sensors: 3 | Calibrations: 4 | Model: Linear".into(), "● Sensors locked".into()];
        s.live = super::super::live::Live {
            temp_c: super::super::live::Reading::new(Some(21.4), 1),
            humidity_pct: super::super::live::Reading::new(Some(48.0), 0),
            pressure_hpa: super::super::live::Reading::new(Some(1008.0), 0),
            light: super::super::live::Reading::new(Some(120.0), 0),
            confidence: Some(0.65),
            expected: Some(4),
        };
        s.forecast = super::super::forecast_card::ForecastCard {
            one_hour: Some(super::super::forecast_card::Band { temp: 21.3, low: 20.8, high: 21.9 }),
            one_day: Some(super::super::forecast_card::Band { temp: 20.1, low: 18.0, high: 22.4 }),
            observations: 30,
        };
        s.calibrate.status = Some("calibrated to 21.5°C (4 total)".into());
        s.calibrate.session = vec![(21.5, 0)];
        s.history = super::super::history::History::new((0..7).map(|i| super::super::history::Point {
            at: 1_700_000_000 + i * 86_400, temp_c: 19.0 + i as f32 * 0.4 }).collect());
        s.history_nav.range = "Sep 21 – Sep 27".into();
        s.history_nav.note = Some("exported weather_data/exports/ambient-history-20260927_040903.csv".into());
        s.locations.set(vec![
            super::super::locations::Location { id: 0, name: "Unknown".into(), coords: None, current: false },
            super::super::locations::Location { id: 1, name: "Home".into(), coords: None, current: true },
        ]);
        s.model.stats = super::super::model::ModelStats { model_type: "Linear".into(), linear: 4, neural: 4, confidence: 0.65 };
        s.model.set_log(vec![super::super::model::CalEntry {
            source: super::super::model::Source::Egui, target_c: 21.5, at: 1_700_000_000 }]);
        s.sync.db = super::super::sync::DbSummary { calibrations: 3, linear: 4, neural: 4, model_type: "Linear".into() };
        s.sync.backend = Some(super::super::sync::TransferBackend { can_export: true, can_import: true, exports_locations: false });
        s.sync.export_dir = "weather_data/exports".into();
        s.sync.note = Some("exported weather_data/exports/sensors_export_20260927_041126.db".into());
        s.host_log = (0..12).map(|i| format!("log line {i}: something the host did, at some length")).collect();
        s
    }

    fn run(size: egui::Vec2, section: Section) -> (uikit_egui::layout_probe::Report, Vec<uikit_egui::layout_probe::Text>) {
        let mut shell = filled();
        shell.router.go(section);
        let colors = uikit_egui::palette_colors("cloudscape");
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(screen);
        let mut last = Vec::new();
        for i in 0..4 {
            #[allow(deprecated)]
            let out = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    shell.ui(ui, &colors, i as f32 * 16.0);
                });
            });
            last = out.shapes;
        }
        let compact = size.x < 600.0;
        (uikit_egui::layout_probe::analyse(screen, &last, compact), uikit_egui::layout_probe::texts(&last))
    }

    #[test]
    fn every_section_lays_out_without_collisions_at_phone_and_desktop_widths() {
        for section in Section::ALL {
            for size in [egui::vec2(360.0, 640.0), egui::vec2(1000.0, 700.0)] {
                let (r, _) = run(size, section);
                assert!(r.overlaps.is_empty(), "{section:?} at {size:?}: {}", r.describe());
            }
        }
    }

    #[test]
    fn the_live_page_shows_the_gtk_pages_parts_on_a_phone() {
        let (r, texts) = run(egui::vec2(360.0, 640.0), Section::Live);
        let has = |t: &str| texts.iter().any(|g| g.text.contains(t));
        assert!(has("Active sensors: 3"), "status line: {}", r.describe());
        assert!(has("Forecast"), "forecast box");
        assert!(has("Calibrate"), "calibrate card");
        assert!(has("1 hour"), "forecast lines");
        // The reading is the focal point: the largest text on the page.
        let big = texts.iter().max_by(|a, b| a.rect.height().partial_cmp(&b.rect.height()).unwrap()).unwrap();
        assert!(big.text.contains('°'), "the biggest text is not the reading: {:?}", big.text);
    }

    #[test]
    fn on_a_desktop_the_cards_sit_beside_the_reading() {
        let (r, texts) = run(egui::vec2(1000.0, 700.0), Section::Live);
        assert!(r.overlaps.is_empty(), "{}", r.describe());
        let reading = texts.iter().find(|g| g.text.contains("Active sensors")).expect("status line");
        let forecast = texts.iter().find(|g| g.text == "1 hour").expect("forecast line");
        assert!(forecast.rect.left() > 450.0, "the forecast is not in the right column: {:?}", forecast.rect);
        assert!(forecast.rect.top() < reading.rect.top(),
                "the right column starts at the top, not under the left one");
    }

    #[test]
    fn the_calibrate_card_is_reachable_by_scrolling_on_a_short_phone() {
        // On a 640px-tall phone the card sits below the fold; the page
        // scrolls, so the card exists in the frame (clipped), rather than
        // being cut off by a fixed layout that never draws it.
        let (_, texts) = run(egui::vec2(360.0, 640.0), Section::Live);
        assert!(texts.iter().any(|g| g.text.contains("Calibrate")), "the calibrate card was never drawn");
    }
}


impl Default for Shell {
    fn default() -> Self {
        Shell {
            a11y: Default::default(),
            router: Router::default(),
            live: Default::default(),
            dot: Default::default(),
            history: Default::default(),
            locations: Default::default(),
            map: Default::default(),
            model: Default::default(),
            sync: Default::default(),
            screen_temps: Default::default(),
            ambience: Default::default(),
            // 0% is dry, which is a reading. See the field's note.
            sparkles: super::ambience::Sparkles { humidity_pct: 0.0 },
            // A plausible reading, matching `LiveState::demo`'s own reasoning:
            // a shell that cannot be constructed before the first sensor
            // reading looks broken on launch, not waiting.
            live_state: crate::LiveState::demo(),
            last_t_ms: None,
            contacts: Vec::new(),
            share: weather_share::WeatherShare::new("ambient-learner-demo"),
            shared_note: None,
            pending: Vec::new(),
            back_unhandled: false,
            calibrate: Default::default(),
            forecast: Default::default(),
            history_nav: Default::default(),
            status: Vec::new(),
            host_log: Vec::new(),
        }
    }
}
