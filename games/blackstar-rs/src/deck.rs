//! THE DECK — one object the shell talks to.
//!
//! `menu.rs` navigates, `menufx.rs` moves, `stations.rs` knows the
//! rooms, `Shipboard` holds the state. The shell should not conduct
//! that quartet — it gets ONE thing with four verbs:
//!
//!     open(station)   walk to a console (or switch consoles)
//!     input(i)        forward what the player did
//!     tick(dt)        advance the motion; maybe get an Order back
//!     view()          everything a draw pass needs, posed
//!
//! The deck enforces the feel rules so no caller can get them wrong:
//! input lands only on a SEATED screen (a deploying rack ignores taps),
//! Back folds the console shut before anything actually closes, and
//! switching stations folds the old room out before the new one
//! cascades in — the machine finishes its motion because it is a
//! machine, and that is the whole aesthetic in one rule.
//!
//! State changes store through `Shipboard` the moment they land, and
//! the console rebuilds around the change with the cursor held on the
//! same row id where it survives — a swap should feel like the world
//! changed, not like the menu reset.

use crate::menu::{Event, Host, Input, Menu};
use crate::menufx::{Motion, Pending, RowPose, Stage};
use crate::ship::Phase;
use crate::stations::{apply, build, Order, Shipboard, Station};

pub struct Deck {
    pub station: Station,
    host: Host,
    pub motion: Motion,
    sb: Shipboard,
    week: u64,
    /// the last thing a console said back — drawn under the rows
    pub feedback: Option<String>,
    /// where to walk when the current fold-out finishes
    walking_to: Option<Station>,
}

/// Size a console's scroll window to the panel that draws it.
///
/// `Menu::new` defaults `view` to 12 and nothing reconciled that with the
/// console's own geometry, which holds twenty rows (`hud::deck_rows`). So
/// every station scrolled eight rows early, and the wardeck's twenty rows
/// scrolled when they would have fitted exactly.
///
/// It is normalised UI space, so this is a geometry question rather than a
/// screen question — the same number on every display.
pub fn fitted(mut m: crate::menu::Menu) -> crate::menu::Menu {
    m.view = crate::hud::deck_rows();
    m
}

impl Deck {
    /// Stand at a console. Refuses like the ship does if the phase says
    /// the walk is impossible.
    pub fn open(station: Station, phase: Phase, week: u64) -> Result<Deck, &'static str> {
        station.available(phase)?;
        let sb = Shipboard::load();
        let menu = fitted(build(station, &sb, week));
        let rows = menu.rows.len();
        let sel = menu.sel();
        let mut host = Host::new();
        host.open(menu);
        let mut motion = Motion::shut();
        motion.open(station.style(), rows, sel);
        Ok(Deck { station, host, motion, sb, week, feedback: None, walking_to: None })
    }

    pub fn is_shut(&self) -> bool {
        self.motion.stage == Stage::Shut && self.walking_to.is_none()
    }

    /// Forward input. A screen mid-motion swallows everything except
    /// Back, which is allowed to interrupt an opening — walking away
    /// from a deploying console is a thing a person can do.
    pub fn input(&mut self, i: Input) -> Option<Order> {
        if i == Input::Back {
            if self.motion.stage != Stage::Shut && self.motion.stage != Stage::Closing {
                self.motion.close_then(Pending::Dismiss);
            }
            return None;
        }
        if !self.motion.listening() {
            return None;
        }
        let ev = self.host.input(i)?;
        self.on_event(ev)
    }

    fn on_event(&mut self, ev: Event) -> Option<Order> {
        let order = apply(self.station, &ev, &mut self.sb, self.week);
        match &order {
            Some(Order::Did(msg)) => {
                self.feedback = Some(msg.clone());
                self.rebuild_in_place();
                None
            }
            Some(Order::Refused(msg)) => {
                self.feedback = Some(format!("REFUSED: {msg}"));
                None
            }
            Some(Order::Goto(st)) => {
                self.walk_to(*st);
                None
            }
            Some(Order::Sortie(_)) | Some(Order::Nightfall(_)) => order,
            None => None,
        }
    }

    /// Fold this console out; when the fold lands, the next one
    /// cascades in. The player watches the room change hands.
    pub fn walk_to(&mut self, st: Station) {
        if st == self.station {
            return;
        }
        self.walking_to = Some(st);
        self.motion.close_then(Pending::Swap(st.style()));
    }

    /// Rebuild after a state change, holding the cursor on the same row
    /// id when it still exists. No motion: the world changed, the
    /// furniture did not.
    fn rebuild_in_place(&mut self) {
        let held = self.host.top().and_then(|m| m.selected()).map(|r| r.id);
        let menu = fitted(build(self.station, &self.sb, self.week));
        let rows = menu.rows.len();
        let sel = held
            .and_then(|id| menu.rows.iter().position(|r| r.id == id && !id.is_empty()))
            .unwrap_or_else(|| menu.sel());
        let mut host = Host::new();
        host.open(menu);
        // steer to the held row through the host so its own rules apply
        host.input(Input::Point(sel));
        self.host = host;
        self.motion.cursor.aim(sel);
        let keep = self.motion.stage;
        self.motion.rows_changed(rows);
        debug_assert_eq!(self.motion.stage, keep);
    }

    /// Advance the clocks. Orders can surface here too: a fold that was
    /// hiding a station switch finishes into the new console.
    pub fn tick(&mut self, dt: f32) {
        self.motion.cursor.aim(self.host.top().map(|m| m.sel()).unwrap_or(0) as usize);
        if let Some(p) = self.motion.tick(dt) {
            match p {
                Pending::Swap(style) => {
                    if let Some(st) = self.walking_to.take() {
                        self.station = st;
                        self.feedback = None;
                        let menu = fitted(build(st, &self.sb, self.week));
                        let rows = menu.rows.len();
                        let sel = menu.sel();
                        let mut host = Host::new();
                        host.open(menu);
                        self.host = host;
                        self.motion.open(style, rows, sel);
                    }
                }
                Pending::Dismiss | Pending::Pop => {
                    // shut; `is_shut` tells the shell to drop the deck
                }
            }
        }
    }

    /// Everything a draw pass needs, posed. The deck does the sampling
    /// so `hud.rs` stays a printer.
    pub fn view(&self) -> DeckView<'_> {
        DeckView { deck: self }
    }
    pub fn menu(&self) -> Option<&Menu> {
        self.host.top()
    }
}

/// A borrowed, posed view of the deck for one frame.
pub struct DeckView<'a> {
    deck: &'a Deck,
}

impl<'a> DeckView<'a> {
    pub fn title(&self) -> &'static str {
        self.deck.station.title()
    }
    pub fn room(&self) -> &'static str {
        self.deck.station.room()
    }
    pub fn accent(&self) -> [f32; 3] {
        self.deck.station.accent()
    }
    pub fn feedback(&self) -> Option<&str> {
        self.deck.feedback.as_deref()
    }
    /// cursor position in visible-row units, plus its squash
    pub fn cursor(&self) -> (f32, f32) {
        let m = self.deck.motion.cursor.y - self.deck.menu().map(|m| m.scroll()).unwrap_or(0) as f32;
        (m, self.deck.motion.cursor.squash())
    }
    /// the rows on stage: (true index, row, pose, selected)
    pub fn rows(&self) -> Vec<(usize, &crate::menu::Row, RowPose, bool)> {
        let Some(menu) = self.deck.menu() else { return Vec::new() };
        let sel = menu.sel();
        menu.visible().map(|(i, r)| (i, r, self.deck.motion.row(i), i == sel)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menufx::Stage;

    fn seated(d: &mut Deck) {
        let mut guard = 0;
        while d.motion.stage != Stage::Steady {
            d.tick(1.0 / 60.0);
            guard += 1;
            assert!(guard < 1000, "never seated");
        }
    }

    #[test]
    fn a_deploying_console_ignores_taps_and_a_seated_one_listens() {
        let mut d = Deck::open(Station::Medbay, Phase::Cruise, 3).unwrap();
        assert_eq!(d.input(Input::Move(1)), None);
        let before = d.menu().unwrap().sel();
        seated(&mut d);
        assert_eq!(d.menu().unwrap().sel(), before, "input landed mid-deploy");
        d.input(Input::Move(1));
        assert_ne!(d.menu().unwrap().sel(), before, "a seated console went deaf");
    }

    #[test]
    fn back_folds_before_anything_closes() {
        let mut d = Deck::open(Station::Medbay, Phase::Cruise, 3).unwrap();
        seated(&mut d);
        d.input(Input::Back);
        assert_eq!(d.motion.stage, Stage::Closing);
        assert!(!d.is_shut(), "the console vanished without folding");
        let mut guard = 0;
        while !d.is_shut() {
            d.tick(1.0 / 60.0);
            guard += 1;
            assert!(guard < 1000, "the fold never finished");
        }
    }

    #[test]
    fn walking_between_stations_folds_out_then_cascades_in() {
        let mut d = Deck::open(Station::Armory, Phase::Cruise, 3).unwrap();
        seated(&mut d);
        d.walk_to(Station::Hold);
        assert_eq!(d.station, Station::Armory, "the room changed before the fold");
        assert_eq!(d.motion.stage, Stage::Closing);
        let mut guard = 0;
        while d.station != Station::Hold {
            d.tick(1.0 / 60.0);
            guard += 1;
            assert!(guard < 1000, "the walk never arrived");
        }
        assert_eq!(d.motion.stage, Stage::Opening, "the hold arrived already seated");
        seated(&mut d);
        assert!(d.motion.listening());
    }

    #[test]
    fn the_hull_refuses_the_walk_when_the_phase_is_wrong() {
        assert!(Deck::open(Station::Armory, Phase::Folding, 0).is_err());
        assert!(Deck::open(Station::Bridge, Phase::Docked, 0).is_err());
        assert!(Deck::open(Station::Bridge, Phase::Plotting, 0).is_ok());
    }
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    #[test]
    fn the_scroll_window_matches_the_panel_that_draws_it() {
        // `Menu::new` defaults to 12 and the console holds twenty. Nothing
        // reconciled them, so every station scrolled eight rows early.
        let panel = crate::hud::deck_rows();
        assert!(panel > 12, "the panel holds {panel}; the old default was 12");
        let m = fitted(crate::menu::Menu::new("t", vec![]));
        assert_eq!(m.view, panel);
    }

    #[test]
    fn the_wardeck_stops_scrolling_when_it_no_longer_needs_to() {
        // twenty wardens plus a heading is the longest station that could
        // plausibly fit; the medbay is longer and still scrolls, correctly
        let sb = Shipboard::load();
        let panel = crate::hud::deck_rows();
        for st in [Station::Wardeck, Station::Medbay] {
            let m = fitted(build(st, &sb, 3));
            let needs_scroll = m.rows.len() > m.view;
            assert_eq!(
                needs_scroll,
                m.rows.len() > panel,
                "{} scrolls on the wrong side of the panel's {panel} rows ({} rows)",
                st.id(),
                m.rows.len()
            );
        }
    }

    #[test]
    fn the_geometry_is_resolution_independent() {
        // normalised UI space: the row count is a property of the layout, not
        // of the display, which is why this is answerable without one
        assert_eq!(
            crate::hud::deck_rows(),
            crate::hud::vis_rows(crate::hud::DECK_ROW_H, crate::hud::DECK_TOP, crate::hud::TW_BOTTOM)
        );
        // and the floor in vis_rows keeps a pathological layout usable
        assert!(crate::hud::vis_rows(9.0, 0.1, -0.1) >= 3);
    }
}
