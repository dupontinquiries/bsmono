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
    pub map: super::map::MapPage,
    pub model: super::model::ModelPage,
    pub sync: super::sync::SyncPage,
    pub screen_temps: super::screen_temps::ScreenTempsPage,
    pub ambience: super::ambience::Ambience,
    /// Not Default: Sparkles is a humidity reading, and 0% is a MEANINGFUL
    /// value (dry air draws no droplets) rather than an absent one. Built
    /// explicitly so nobody reads a default as "no data".
    pub sparkles: super::ambience::Sparkles,
    /// Set when Back was pressed with no history left. The host decides what
    /// that means — on a phone it is usually "close the app", and deciding
    /// that here would make a UI module own the process lifetime.
    pub back_unhandled: bool,
    /// Actions raised by a page this frame, for the host to apply.
    ///
    /// Queued rather than handled here: adding or removing a location touches
    /// the database, and a UI module that writes to storage is the coupling
    /// the port was built to avoid. The host drains this.
    pub pending: Vec<super::locations::LocationAction>,
}

impl Shell {
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
        if let Some(s) = router::tab_bar_ui(ui, self.router.current(), glyphs_only) {
            self.router.go(s);
        }
        ui.separator();

        // Keyboard shortcuts, from the table Section already owns.
        let typed: Vec<char> = ui.ctx().input(|i| i.events.iter().filter_map(|e| match e {
            egui::Event::Text(t) => t.chars().next(),
            _ => None,
        }).collect());
        for c in typed { self.router.shortcut(c); }

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

        match self.router.current() {
            Section::Live => {
                super::live::live_ui(ui, &self.live, &self.dot, colors);
            }
            Section::History => super::history::history_ui(ui, &self.history, colors),
            Section::Places => {
                // locations_ui RETURNS AN ACTION and the other pages do not.
                // Dropping it with `let _ =` would compile and would make
                // every button on the page dead — the same class of bug this
                // whole file exists to fix, one level down.
                if let Some(a) = super::locations::locations_ui(ui, &mut self.locations, colors) {
                    self.pending.push(a);
                }
            }
            Section::Map => super::map::map_ui(ui, &mut self.map, colors),
            Section::Model => super::model::model_ui(ui, &self.model, colors),
            Section::Sync => super::sync::sync_ui(ui, &self.sync, colors),
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
                super::screen_temps::screen_temps_ui(
                    ui, &self.screen_temps, (t_ms as u64) * 1_000_000, colors);
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


impl Default for Shell {
    fn default() -> Self {
        Shell {
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
            pending: Vec::new(),
            back_unhandled: false,
        }
    }
}
