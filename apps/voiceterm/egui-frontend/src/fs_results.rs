//! fs_results — the filesystem-find results panel, ported from `fsResultsPanel`.
//!
//! The find bar has two modes. In SCROLLBACK mode it searches what the terminal
//! has already printed; in FILES mode it shells out to a filesystem search and
//! shows `path:line` with the matched text. This is the second one, which the
//! egui port did not have.
//!
//! ## The rule that matters most is a refusal
//!
//! `main.qml` v0.1.363, quoted because a port that loses this is worse than one
//! that never had it:
//!
//! > refuse fs_find when in an SSH session. The search runs on the LOCAL
//! > filesystem, which is almost never what the user wants when they're working
//! > on a remote shell.
//!
//! Silently searching the wrong machine returns plausible results from files
//! that have nothing to do with the work in front of you, and nothing about
//! the output says which host they came from. [`Search::start`] refuses, and
//! says why and what to do instead.
//!
//! ## Why the panel has a searching state at all
//!
//! The QML comment again: visible when there is at least one result *"OR is
//! actively searching (so the user gets a visible 'something is happening'
//! affordance)"*. A find that takes two seconds over a large tree and shows
//! nothing until it finishes reads as a find that did not run.

/// One hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    pub line: u32,
    /// The matched text, as the searcher returned it.
    pub matched: String,
}

/// What the find bar is searching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// What the terminal has already printed. Works over SSH, because the
    /// scrollback is the remote shell's output.
    Scrollback,
    /// The local filesystem.
    Files,
}

/// Why a search would not start.
///
/// Copy: it is fieldless and `message` takes self, so every read through a
/// borrowed State would otherwise need a clone to say why it refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Working on a remote shell; a local search would answer about the wrong
    /// machine.
    RemoteSession,
    /// Nothing to search for.
    EmptyQuery,
}

impl Refusal {
    /// The message, which must name the alternative.
    ///
    /// A refusal that only says no leaves the operator with a broken feature.
    /// Scrollback mode DOES search the remote shell's visible output, so there
    /// is a real thing to do instead and it costs one word to say it.
    pub fn message(self) -> &'static str {
        match self {
            Refusal::RemoteSession =>
                "file search runs on the LOCAL filesystem and this is an SSH session — \
switch the find bar to scrollback mode to search the remote shell's output",
            Refusal::EmptyQuery => "type something to search for",
        }
    }
}

/// The panel's state.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Not searching and nothing to show. The panel takes no height.
    Idle,
    /// Running. Shown even with no hits yet — see the module note.
    Searching { query: String },
    Done { query: String, hits: Vec<Hit>, selected: usize },
    Refused(Refusal),
}

impl Default for State {
    fn default() -> Self { State::Idle }
}

/// The find bar's file-search half.
#[derive(Debug, Clone, Default)]
pub struct Search {
    pub state: State,
}

impl Search {
    /// Begin a search, or refuse with a reason.
    ///
    /// `in_ssh` is passed in rather than detected here: whether the shell is
    /// remote is the terminal's knowledge, and a UI module that sniffs for it
    /// would be a second answer to a question the shell already answers.
    pub fn start(&mut self, query: &str, in_ssh: bool) -> Result<(), Refusal> {
        if query.trim().is_empty() {
            self.state = State::Refused(Refusal::EmptyQuery);
            return Err(Refusal::EmptyQuery);
        }
        if in_ssh {
            self.state = State::Refused(Refusal::RemoteSession);
            return Err(Refusal::RemoteSession);
        }
        self.state = State::Searching { query: query.to_string() };
        Ok(())
    }

    /// The searcher finished.
    ///
    /// Selection lands on the first hit, or on nothing when there are none —
    /// the QML sets `fsSelectedIndex` to -1 for empty, and an index into an
    /// empty list is how a next/previous key panics.
    pub fn finish(&mut self, hits: Vec<Hit>) {
        let query = match &self.state {
            State::Searching { query } => query.clone(),
            // A result arriving for a search that was cancelled is dropped
            // rather than shown: it answers a question nobody is asking any
            // more, and showing it makes Cancel look broken.
            _ => return,
        };
        self.state = State::Done { query, hits, selected: 0 };
    }

    pub fn cancel(&mut self) {
        self.state = State::Idle;
    }

    /// Hits, or an empty slice.
    pub fn hits(&self) -> &[Hit] {
        match &self.state {
            State::Done { hits, .. } => hits,
            _ => &[],
        }
    }

    /// Move the selection, wrapping. Returns the newly selected hit.
    pub fn step(&mut self, delta: i32) -> Option<&Hit> {
        let State::Done { hits, selected, .. } = &mut self.state else { return None };
        if hits.is_empty() { return None; }
        let n = hits.len() as i32;
        // rem_euclid, so a backward step from zero wraps to the end rather
        // than going negative and panicking on the cast.
        *selected = ((*selected as i32 + delta).rem_euclid(n)) as usize;
        hits.get(*selected)
    }

    /// The counter the find bar shows, e.g. "1 of 12".
    pub fn counter(&self) -> String {
        match &self.state {
            State::Done { hits, selected, .. } if !hits.is_empty() =>
                format!("{} of {}", selected + 1, hits.len()),
            State::Done { .. } => "no matches".into(),
            State::Searching { .. } => "searching…".into(),
            State::Refused(r) => r.message().into(),
            State::Idle => String::new(),
        }
    }

    /// Does the panel occupy any height?
    ///
    /// The QML computes this as `visible ? min(220, contentHeight + 8) : 0`.
    /// The rule, not the arithmetic: shown while searching or when there is
    /// something to show, and taking NO space otherwise — a panel that keeps
    /// its height when empty steals rows from the terminal permanently.
    pub fn occupies_space(&self) -> bool {
        !matches!(self.state, State::Idle)
    }
}

/// Draw the panel. Returns a hit if one was activated.
pub fn fs_results_ui(
    ui: &mut egui::Ui,
    s: &Search,
    c: &uikit_egui::PaletteColors,
    max_height: f32,
) -> Option<Hit> {
    if !s.occupies_space() { return None; }
    let mut opened = None;

    match &s.state {
        State::Searching { query } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new(format!("searching for {query:?}")).small().weak());
            });
        }
        State::Refused(r) => {
            ui.label(egui::RichText::new(r.message()).small().color(c.warning));
        }
        State::Done { hits, selected, .. } if hits.is_empty() => {
            ui.label(egui::RichText::new("no matches").small().weak());
            let _ = selected;
        }
        State::Done { hits, selected, .. } => {
            // Capped, as in the QML. A results panel that grows to fill the
            // window leaves no terminal to look at while you read them.
            egui::ScrollArea::vertical().max_height(max_height.min(220.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (i, h) in hits.iter().enumerate() {
                        let sel = i == *selected;
                        let row = ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!("{}:{}", h.path, h.line))
                                .monospace().small()
                                .color(if sel { c.accent } else { c.text_weak }));
                            ui.label(egui::RichText::new(h.matched.trim()).small());
                        });
                        if uikit_egui::dock::row_click(
                            ui, row.response.rect,
                            (h.path.as_str(), h.line)).clicked() {
                            opened = Some(h.clone());
                        }
                    }
                });
        }
        State::Idle => {}
    }
    opened
}

/// Driven through real frames — same unclickable-row bug as the rest.
#[cfg(test)]
mod frame_tests {
    use super::*;
    use crate::frame_probe::Probe;

    fn done() -> Search {
        Search {
            state: State::Done {
                query: "needle".into(),
                hits: vec![
                    Hit { path: "src/lib.rs".into(), line: 12,
                          matched: "let needle = 1;".into() },
                    Hit { path: "src/main.rs".into(), line: 90,
                          matched: "// needle again".into() },
                ],
                selected: 0,
            },
        }
    }

    #[test]
    fn clicking_a_hit_opens_that_file_and_line() {
        // path:line is the whole product of a file search; a row that
        // cannot be clicked makes the panel a read-only list of places you
        // then have to type out by hand.
        let p = Probe::default();
        let c = p.colors();
        let hit = p.click_text("src/main.rs", |ui| fs_results_ui(ui, &done(), &c, 400.0));
        let hit = hit.expect("clicking a hit opened nothing");
        assert_eq!(hit.path, "src/main.rs");
        assert_eq!(hit.line, 90);
    }

    #[test]
    fn an_idle_search_takes_no_space_at_all() {
        // `occupies_space` exists so the panel costs no terminal rows when
        // nothing is being searched for. Asserted on what was DRAWN,
        // because that is the claim.
        let p = Probe::default();
        let c = p.colors();
        let (d, out) = p.frame(|ui| fs_results_ui(ui, &Search { state: State::Idle }, &c, 400.0));
        assert!(out.is_none());
        assert!(d.texts.is_empty(), "an idle search drew {:?}", d.texts);
    }

    #[test]
    fn a_running_search_says_so_rather_than_looking_empty() {
        // Shown with no hits yet, deliberately: a blank panel during a
        // search is indistinguishable from a search that found nothing.
        let p = Probe::default();
        let c = p.colors();
        let (d, _) = p.frame(|ui| fs_results_ui(
            ui, &Search { state: State::Searching { query: "needle".into() } }, &c, 400.0));
        assert!(d.has("searching"), "{:?}", d.texts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(p: &str, l: u32) -> Hit {
        Hit { path: p.into(), line: l, matched: format!("line {l}") }
    }

    #[test]
    fn a_file_search_refuses_inside_an_ssh_session() {
        // v0.1.363 in the QML, and the rule most likely to be lost in a port:
        // the search runs on the LOCAL filesystem, so over SSH it answers
        // about the wrong machine — with plausible results, from real files,
        // and nothing in the output saying which host they came from.
        let mut s = Search::default();
        assert_eq!(s.start("needle", true), Err(Refusal::RemoteSession));
        assert!(s.hits().is_empty());
        assert!(s.start("needle", false).is_ok());
    }

    #[test]
    fn the_refusal_names_what_to_do_instead() {
        // A refusal that only says no leaves the operator with a broken
        // feature. Scrollback mode does search the remote shell's output.
        let m = Refusal::RemoteSession.message();
        assert!(m.contains("scrollback"), "{m}");
        assert!(m.contains("LOCAL"), "the reason has to be in it too: {m}");
    }

    #[test]
    fn the_panel_takes_no_height_when_idle() {
        // The QML gives it height only when visible. A panel that keeps its
        // rows when empty steals them from the terminal permanently.
        let mut s = Search::default();
        assert!(!s.occupies_space());
        s.start("x", false).unwrap();
        assert!(s.occupies_space(), "a running search must be visible");
        s.cancel();
        assert!(!s.occupies_space());
    }

    #[test]
    fn a_running_search_is_visible_before_it_has_any_hits() {
        // The QML's stated reason: "so the user gets a visible 'something is
        // happening' affordance". A find over a large tree that shows nothing
        // until it finishes reads as a find that did not run.
        let mut s = Search::default();
        s.start("needle", false).unwrap();
        assert!(s.occupies_space());
        assert_eq!(s.counter(), "searching…");
    }

    #[test]
    fn results_for_a_cancelled_search_are_dropped() {
        // They answer a question nobody is asking any more, and showing them
        // makes Cancel look broken.
        let mut s = Search::default();
        s.start("needle", false).unwrap();
        s.cancel();
        s.finish(vec![hit("a.rs", 1)]);
        assert!(s.hits().is_empty(), "a cancelled search produced results");
        assert!(!s.occupies_space());
    }

    #[test]
    fn stepping_wraps_in_both_directions() {
        // A backward step from zero must wrap rather than go negative — the
        // cast back to usize is where that panics.
        let mut s = Search::default();
        s.start("n", false).unwrap();
        s.finish(vec![hit("a", 1), hit("b", 2), hit("c", 3)]);
        assert_eq!(s.counter(), "1 of 3");
        assert_eq!(s.step(-1).map(|h| h.path.clone()), Some("c".to_string()));
        assert_eq!(s.counter(), "3 of 3");
        assert_eq!(s.step(1).map(|h| h.path.clone()), Some("a".to_string()));
    }

    #[test]
    fn stepping_an_empty_result_set_is_not_an_index() {
        // The QML sets fsSelectedIndex to -1 when empty. An index into an
        // empty list is how a next/previous key panics.
        let mut s = Search::default();
        s.start("n", false).unwrap();
        s.finish(vec![]);
        assert!(s.step(1).is_none());
        assert_eq!(s.counter(), "no matches");
    }

    #[test]
    fn an_empty_query_is_refused_before_a_process_is_spawned() {
        // Cheap, and it is the difference between a no-op and shelling out a
        // whole-tree walk for the empty string.
        let mut s = Search::default();
        assert_eq!(s.start("   ", false), Err(Refusal::EmptyQuery));
    }

    #[test]
    fn ssh_state_is_passed_in_rather_than_detected_here() {
        // Whether the shell is remote is the terminal's knowledge. A UI module
        // sniffing for it would be a second answer to a question the shell
        // already answers, and the two would disagree the moment one changed.
        // The needles are ASSEMBLED, not written as literals — a source scan
        // whose needle appears in the file it scans trips on itself. Third
        // time this session: ambient-learner's shell (twice) and the peer
        // panel. Writing it down as a rule rather than rediscovering it again.
        let src = include_str!("fs_results.rs");
        for probe in [format!("SSH_{}", "CLIENT"), format!("SSH_{}", "TTY"),
                      format!("env::{}", "var")] {
            assert!(!src.contains(probe.as_str()),
                    "this module detects SSH itself via {probe}");
        }
    }
}
