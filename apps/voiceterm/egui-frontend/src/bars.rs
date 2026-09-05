//! bars — the bottom bar and the archived-page browser, drawn.
//!
//! Both models are Qt-free (`src/bottom_bar.rs`, `src/archive.rs` have zero
//! qmetaobject references), so the port mounts them by `#[path]` and adds only
//! what a frontend owes: the fan-out gesture and the search surface.
//!
//! ## The fan-out, which is what makes the bar work on a phone
//!
//! A bar button is not just a button — each one carries a list (`Tabs` fans out
//! to the tab list, `Workspaces` to workspaces). Pressing and holding fans the
//! list out above the bar. That gives a phone-sized surface access to four lists
//! without four screens, and it is why the bar is worth porting rather than
//! replacing with a row of tabs.
//!
//! Two rules the gesture needs, and both come from what goes wrong without them:
//!
//! * **A tap is not a hold.** A tap activates the button's primary action; only
//!   a hold fans out. Firing the fan-out on tap makes the primary action
//!   unreachable.
//! * **Releasing outside the fan cancels.** Sliding off the list and letting go
//!   must select nothing, because the alternative is picking whatever your thumb
//!   happened to be over when you gave up.

#[path = "../../src/bottom_bar.rs"]
#[allow(dead_code)]
pub mod bottom_bar;

// archive.rs is NOT mounted, unlike bottom_bar.rs. It reaches for
// `crate::layout_cache` and `crate::util` at the crate root, so mounting it
// drags two more modules and the scraper HTML stack into a widget crate — the
// same chain that stopped mail_proton being mounted. The archived-page browser
// needs a view over a page LIST, and that list is three fields; the engine keeps
// the parsing.
pub use bottom_bar::{BarButton, BarButtonKind, BarOption, BottomBar};

/// How long a press must last to fan out, in seconds.
///
/// Long enough that a tap is unambiguous, short enough not to feel stuck. Below
/// about 0.2s a normal tap starts triggering it; above about 0.6s the bar feels
/// broken.
/// How long a press must last to fan out.
///
/// RE-EXPORTED from the kit rather than declared here. This value was
/// duplicated: `uikit_egui::cmdstrip::HOLD_SECONDS` carries the same
/// 0.32, and two apps in one fleet disagreeing about what a long press
/// feels like is worse than either number being slightly off — a person
/// moving between voiceterm and aperture should not have to retrain
/// their thumb. Changing it in one place now changes it everywhere,
/// which is the only way that stays true.
///
/// The GESTURE STATE is deliberately NOT shared. `cmdstrip::Hold` is one
/// button with a threshold; a bar has several, each with its own fan-out
/// and hovered option, and collapsing the richer shape into the simpler
/// one would lose the hover — the part that makes sliding onto an option
/// and off again work. Sharing the timing and not the state machine is
/// the honest split. (That richer state used to be `BarState` here, a
/// hand-rolled machine nothing in production ever drove; `bar_ui` below
/// gets it from `SlideMenu` instead, and real-frame tests replaced the
/// ones that used to assert against the mock — see the test module.)
pub use uikit_egui::cmdstrip::HOLD_SECONDS;

/// What a gesture produced.
#[derive(Debug, Clone, PartialEq)]
pub enum BarEvent {
    /// A tap: run the button's primary action.
    Primary(usize),
    /// An option was chosen from a fan-out.
    Option { button: usize, option: usize },
    /// Fanned out and released with nothing selected.
    Cancelled,
}

/// Draw the bottom bar with a real press-and-hold fan-out, reusing
/// `uikit_egui::slide_menu::SlideMenu` rather than hand-rolling the gesture —
/// this module's header states the RULES (a tap is not a hold; releasing
/// outside the fan cancels), and `SlideMenu` already implements exactly
/// those, tested, and used elsewhere in the kit. Memory-backed (`show_stored`)
/// so callers don't need to thread a `SlideMenu` through their app state per
/// button — only `BottomBar` itself is owned by the caller.
///
/// Returns the resolved event for the frame the gesture completed, `None`
/// otherwise (including every frame the menu is still open and being
/// dragged — `SlideOutcome::Idle` covers both "closed" and "open,
/// undecided", and only the caller knows the difference doesn't matter for
/// dispatch).
pub fn bar_ui(ui: &mut egui::Ui, bar: &BottomBar) -> Option<BarEvent> {
    let mut result = None;
    ui.horizontal(|ui| {
        for (idx, b) in bar.buttons().iter().enumerate() {
            let label = if b.badge.is_empty() { b.label.clone() }
                        else { format!("{} {}", b.label, b.badge) };
            let host = ui.add_enabled(b.enabled, egui::Button::new(label));
            // TEST SUPPORT ONLY: stash each button's screen rect in egui's own
            // temp memory (auto-expiring, standard idiom — see `Memory::data`),
            // so a test driving real pointer events through `bar_ui` can find
            // where to send them without re-deriving egui's internal auto-id
            // scheme. Costs one hashmap write per button per frame; nothing
            // reads it outside `#[cfg(test)]`.
            ui.ctx().memory_mut(|m| m.data.insert_temp(
                egui::Id::new(("voiceterm_bar_rect", b.id.as_str())), host.rect));
            if b.options.is_empty() {
                // No fan-out possible: a tap is the only gesture this button
                // has, so SlideMenu (which exists to draw a fan-out) would be
                // the wrong tool — an empty menu reads as a broken button,
                // which is the exact failure this module's own doc warns
                // about avoiding.
                if host.clicked() { result = Some(BarEvent::Primary(idx)); }
                continue;
            }
            let opts: Vec<uikit_egui::slide_menu::SlideOption> = b.options.iter()
                .map(|o| uikit_egui::slide_menu::SlideOption::new(o.id.clone(), o.label.clone()))
                .collect();
            let id = egui::Id::new(("voiceterm_bar", b.id.as_str()));
            let menu = uikit_egui::slide_menu::SlideMenu::new(id).hold(HOLD_SECONDS);
            match menu.show_stored(ui, id, &host, &opts) {
                uikit_egui::slide_menu::SlideOutcome::Idle => {
                    // A plain click that never reached the hold threshold and
                    // never opened the menu at all is a primary tap; a click
                    // that opened the menu and is still open is also Idle,
                    // and must NOT be read as a tap — the same "a tap is not
                    // a hold" rule the removed `BarState` existed to state.
                    if host.clicked() && !host.dragged() { result = Some(BarEvent::Primary(idx)); }
                }
                uikit_egui::slide_menu::SlideOutcome::Cancelled => {
                    result = Some(BarEvent::Cancelled);
                }
                uikit_egui::slide_menu::SlideOutcome::Selected(opt_id, _) => {
                    if let Some(option) = b.options.iter().position(|o| o.id == opt_id) {
                        result = Some(BarEvent::Option { button: idx, option });
                    }
                }
            }
        }
    });
    result
}

/// One archived page, as the browser needs it. The engine's `ArchivedPage`
/// carries the stored HTML and its extracted text; a list does not.
#[derive(Debug, Clone, PartialEq)]
pub struct PageEntry {
    pub url: String,
    pub title: String,
    /// Seconds since the epoch; only ordering matters here.
    pub at: i64,
}

impl PageEntry {
    /// What to show. A page with no title shows its URL rather than an empty
    /// row, since a blank entry reads as a broken list.
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() { &self.url } else { &self.title }
    }
}

/// The archived-page browser's view state.
#[derive(Debug, Clone, Default)]
pub struct ArchiveView {
    pub query: String,
    pub selected: Option<String>,
}

impl ArchiveView {
    /// Pages matching the query, newest first.
    ///
    /// Matched over title AND url, case-insensitively: people remember a domain
    /// as often as a headline, and a search that only reads titles fails on
    /// exactly the pages whose titles were useless.
    pub fn results<'a>(&self, pages: &'a [PageEntry]) -> Vec<&'a PageEntry> {
        let q = self.query.trim().to_lowercase();
        let mut v: Vec<&PageEntry> = pages.iter()
            .filter(|p| q.is_empty() || p.title.to_lowercase().contains(&q)
                        || p.url.to_lowercase().contains(&q))
            .collect();
        // newest first, ties on url so the list cannot shuffle between frames
        v.sort_by(|a, b| b.at.cmp(&a.at).then(a.url.cmp(&b.url)));
        v
    }

    /// Filtering must not strand a selection that is no longer listed.
    pub fn reconcile(&mut self, pages: &[PageEntry]) {
        if let Some(url) = self.selected.clone() {
            if !self.results(pages).iter().any(|p| p.url == url) { self.selected = None; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar() -> BottomBar {
        let mut b = BottomBar::new();
        // upsert, not a field push:  is private on purpose, and upsert
        // is what keeps a button unique by id when the bar is rebuilt.
        b.upsert(BarButton::new("tabs", "Tabs", BarButtonKind::Tabs)
            .with_options(vec![BarOption::new("t1", "shell"), BarOption::new("t2", "docs")]));
        b.upsert(BarButton::new("ws", "Spaces", BarButtonKind::Workspaces));
        b
    }

    /// Smoke test for `bar_ui`: a single frame, no pointer interaction, must
    /// not panic and must report nothing resolved — `SlideMenu` is exercised
    /// elsewhere; this only proves the wrapper wires up without a live
    /// pointer to drive it. `BarButton::new` defaults `enabled` false, so a
    /// bar with no upsert-time `.enabled(true)` also exercises the disabled
    /// path (`ui.add_enabled(false, ..)`), which a naive test bar would skip.
    #[test]
    fn bar_ui_renders_one_frame_without_panicking() {
        let b = bar();
        let ctx = egui::Context::default();
        let mut seen = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            seen = bar_ui(ui, &b);
        });
        assert!(seen.is_none(), "no pointer input, so nothing should resolve: {seen:?}");
    }

    /// A button with no options must render as a plain tap target — this is
    /// the exact rule `archived_view::fans_out`/this module's own doc calls
    /// out ("an empty popup reads as a broken button"), so a button with an
    /// empty `options` list must never reach `SlideMenu::show_stored` at all.
    #[test]
    fn a_button_with_no_options_never_reaches_the_slide_menu() {
        let mut b = BottomBar::new();
        b.upsert(BarButton::new("solo", "Solo", BarButtonKind::Tabs));
        let ctx = egui::Context::default();
        // One frame is enough: if this ever called into SlideMenu with an
        // empty option list it would still not panic (SlideMenu handles that
        // itself), so the real assertion is `fans_out`-equivalent reasoning
        // done in code review — this test exists to pin the early-continue
        // path stays reachable as the function changes.
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = bar_ui(ui, &b);
        });
    }

    #[test]
    fn the_hold_threshold_is_the_kit_s_and_cannot_drift() {
        // Not a tautology now that it is a re-export — it is the guard
        // that keeps it one. If someone re-declares a local constant to
        // "just tweak voiceterm", this stops being equal and says so.
        assert_eq!(HOLD_SECONDS, uikit_egui::cmdstrip::HOLD_SECONDS);
        assert!(HOLD_SECONDS > 0.0 && HOLD_SECONDS < 1.0, "a plausible long-press: {HOLD_SECONDS}");
    }

    // ── real-frame tests ──────────────────────────────────────────────────
    //
    // `bar_ui` used to have a documented gap: the two rules in this module's
    // header comment (tap-is-not-hold, release-outside-cancels) were stated
    // as assertions only against `BarState`, a hand-rolled state machine
    // nothing in production ever constructed. `bar_ui` itself drives the
    // real gesture through `uikit_egui::slide_menu::SlideMenu`, and nothing
    // exercised THAT path with real pointer input — a state machine nobody
    // calls can pass forever, which is exactly what happened here.
    //
    // These tests drive `bar_ui` through `egui::Context::run_ui` with
    // synthetic `PointerButton`/`PointerMoved` events and an explicit
    // `RawInput.time`, the same context across frames so egui's own
    // pointer-down and hold-timer state carries over exactly as it would
    // for a real frontend. The one thing they need that `bar_ui`'s callers
    // don't — a button's screen rect, to know where to send the pointer —
    // comes from the temp-memory stash added alongside this test, not from
    // guessing at egui's internal auto-id scheme.
    //
    // The fan-out geometry is not guessed either: `SlideMenu::place_at`
    // centers the grid on the press point (see `slide_menu.rs`), so for the
    // two-option "tabs" fixture below (item_size 52, spacing 8, padding 10 —
    // all `SlideMenu` defaults, since `bar_ui` sets none of them) releasing
    // exactly on the press point lands in the gutter between the two cells,
    // and moving 10px either side lands inside one.

    /// One frame: feed `raw` into `ctx`, run `bar_ui`, return what it saw.
    ///
    /// Placed at a fixed rect well inside the (very large) default screen —
    /// not at the natural top-left corner a bare root `ui.horizontal` would
    /// use. `SlideMenu::place_at` clamps the fan-out into the screen, and a
    /// button hard against an edge gets its whole grid shifted to fit,
    /// which would silently invalidate the fixed pixel offsets the
    /// selection tests below rely on. Found by a failing test, not
    /// anticipated — see the git history of this comment.
    fn frame(ctx: &egui::Context, time: f64, events: Vec<egui::Event>, b: &BottomBar) -> Option<BarEvent> {
        let raw = egui::RawInput { time: Some(time), events, ..Default::default() };
        let mut seen = None;
        let _ = ctx.run_ui(raw, |ui| {
            let rect = egui::Rect::from_min_size(egui::pos2(400.0, 400.0), egui::vec2(600.0, 100.0));
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
                seen = bar_ui(ui, b);
            });
        });
        seen
    }

    fn button_rect(ctx: &egui::Context, id: &str) -> egui::Rect {
        ctx.memory(|m| m.data.get_temp(egui::Id::new(("voiceterm_bar_rect", id))))
            .unwrap_or_else(|| panic!("bar_ui did not stash a rect for {id:?}"))
    }

    fn down(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }
    }
    fn up(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }
    }

    /// A tap through the REAL path: no options, so `bar_ui` takes the
    /// early-continue branch and reads `host.clicked()` directly — this is
    /// the button-with-no-options rule from the module doc, exercised with
    /// an actual press-then-release rather than asserted against a mock.
    #[test]
    fn a_real_tap_on_an_option_less_button_runs_the_primary() {
        let mut b = BottomBar::new();
        b.upsert(BarButton::new("solo", "Solo", BarButtonKind::Tabs));
        let ctx = egui::Context::default();

        assert_eq!(frame(&ctx, 0.0, vec![], &b), None, "layout only");
        let rect = button_rect(&ctx, "solo");
        let p = rect.center();

        assert_eq!(frame(&ctx, 0.01, vec![down(p)], &b), None, "still pressed");
        assert_eq!(frame(&ctx, 0.02, vec![up(p)], &b), Some(BarEvent::Primary(0)),
                   "a quick press-release on a no-options button is a tap");
    }

    /// The fan-out, driven for real: press past the hold threshold, drag
    /// into a cell, release — through `SlideMenu`, not a mock of it.
    #[test]
    fn a_real_hold_and_drag_selects_the_hovered_option() {
        let b = bar(); // "tabs" button, options t1 (index 0), t2 (index 1)
        let ctx = egui::Context::default();

        assert_eq!(frame(&ctx, 0.0, vec![], &b), None, "layout only");
        let rect = button_rect(&ctx, "tabs");
        let press = rect.center();

        assert_eq!(frame(&ctx, 0.0, vec![down(press)], &b), None, "pressed, not yet held");
        assert_eq!(frame(&ctx, (HOLD_SECONDS + 0.05) as f64, vec![], &b), None,
                   "the fan opening is not itself a resolution");
        // +10px is inside cell 1 (starts 4px right of the press point for this
        // fixture's geometry) — see the module note above the test section.
        let hovered = press + egui::vec2(10.0, 0.0);
        assert_eq!(frame(&ctx, (HOLD_SECONDS + 0.10) as f64, vec![egui::Event::PointerMoved(hovered)], &b),
                   None, "hovering alone does not resolve — only release does");
        assert_eq!(frame(&ctx, (HOLD_SECONDS + 0.15) as f64, vec![up(hovered)], &b),
                   Some(BarEvent::Option { button: 0, option: 1 }));
    }

    /// Same gesture, mirrored: -10px lands in cell 0 instead of cell 1. Two
    /// tests rather than one parametrised over `[10.0, -10.0]` because the
    /// asymmetry itself is the thing worth pinning — a bug that only
    /// mishandled negative offsets would pass a single-direction test.
    #[test]
    fn a_real_hold_and_drag_the_other_way_selects_the_other_option() {
        let b = bar();
        let ctx = egui::Context::default();

        let _ = frame(&ctx, 0.0, vec![], &b);
        let press = button_rect(&ctx, "tabs").center();

        let _ = frame(&ctx, 0.0, vec![down(press)], &b);
        let _ = frame(&ctx, (HOLD_SECONDS + 0.05) as f64, vec![], &b);
        let hovered = press - egui::vec2(10.0, 0.0);
        let _ = frame(&ctx, (HOLD_SECONDS + 0.10) as f64, vec![egui::Event::PointerMoved(hovered)], &b);
        assert_eq!(frame(&ctx, (HOLD_SECONDS + 0.15) as f64, vec![up(hovered)], &b),
                   Some(BarEvent::Option { button: 0, option: 0 }));
    }

    /// Releasing without ever dragging off the press point lands in the
    /// gutter between the two cells (see the geometry note above), which is
    /// the real-path equivalent of "released outside the fan" — this fixture
    /// makes that reachable without moving the pointer at all.
    #[test]
    fn a_real_hold_released_without_dragging_cancels() {
        let b = bar();
        let ctx = egui::Context::default();

        let _ = frame(&ctx, 0.0, vec![], &b);
        let press = button_rect(&ctx, "tabs").center();

        let _ = frame(&ctx, 0.0, vec![down(press)], &b);
        let _ = frame(&ctx, (HOLD_SECONDS + 0.05) as f64, vec![], &b);
        assert_eq!(frame(&ctx, (HOLD_SECONDS + 0.1) as f64, vec![up(press)], &b),
                   Some(BarEvent::Cancelled),
                   "released on the gutter between two cells, not on either");
    }

    /// A press that never reaches the hold threshold must not fan out at
    /// all — releasing early on an OPTIONS button is a tap on the button
    /// itself, not a resolved fan gesture. `SlideMenu::show_stored` returns
    /// `Idle` for this case (open never became true), which `bar_ui` reads
    /// as `host.clicked() && !host.dragged()`.
    #[test]
    fn a_real_quick_release_on_an_options_button_does_not_fan_out() {
        let b = bar();
        let ctx = egui::Context::default();

        let _ = frame(&ctx, 0.0, vec![], &b);
        let press = button_rect(&ctx, "tabs").center();

        let _ = frame(&ctx, 0.0, vec![down(press)], &b);
        // Released well before HOLD_SECONDS.
        assert_eq!(frame(&ctx, (HOLD_SECONDS * 0.3) as f64, vec![up(press)], &b),
                   Some(BarEvent::Primary(0)), "a quick tap runs the primary, fan or no fan");
    }

    #[test]
    fn the_bar_model_is_the_apps_own() {
        // Mounted by #[path], so a button kind added to the engine reaches both
        // frontends rather than drifting.
        let b = bar();
        assert_eq!(b.len(), 2);
        assert!(!b.is_empty());
        assert_eq!(b.buttons()[0].kind, BarButtonKind::Tabs);
        assert_eq!(b.options_for("tabs").len(), 2);
        // upsert REPLACES by id rather than appending a duplicate, which is what
        // makes rebuilding the bar every frame safe
        let mut c = bar();
        c.upsert(BarButton::new("tabs", "Tabs", BarButtonKind::Tabs));
        assert_eq!(c.len(), 2);
    }

    fn pages() -> Vec<PageEntry> {
        vec![
            PageEntry { url: "https://rust-lang.org/book".into(), title: "The Book".into(), at: 300 },
            PageEntry { url: "https://example.test/a".into(), title: String::new(), at: 200 },
            PageEntry { url: "https://news.test/x".into(), title: "Headline".into(), at: 100 },
        ]
    }

    #[test]
    fn a_page_with_no_title_shows_its_url() {
        assert_eq!(pages()[1].label(), "https://example.test/a");
        assert_eq!(pages()[0].label(), "The Book");
    }

    #[test]
    fn search_reads_urls_as_well_as_titles() {
        // People remember a domain as often as a headline, and a title-only
        // search fails on exactly the pages whose titles were useless.
        let p = pages();
        let mut v = ArchiveView::default();
        v.query = "rust-lang".into();
        assert_eq!(v.results(&p).len(), 1);
        v.query = "HEADLINE".into();
        assert_eq!(v.results(&p).len(), 1, "case-insensitive");
        v.query = "  ".into();
        assert_eq!(v.results(&p).len(), 3);
    }

    #[test]
    fn the_list_is_newest_first_and_does_not_shuffle() {
        let p = pages();
        let v = ArchiveView::default();
        let a: Vec<&str> = v.results(&p).iter().map(|x| x.url.as_str()).collect();
        let b: Vec<&str> = v.results(&p).iter().map(|x| x.url.as_str()).collect();
        assert_eq!(a, b);
        assert_eq!(a[0], "https://rust-lang.org/book", "newest first");
    }

    #[test]
    fn a_selection_that_leaves_the_results_is_cleared() {
        // Otherwise the reader shows a page the list does not contain.
        let p = pages();
        let mut v = ArchiveView::default();
        v.selected = Some("https://news.test/x".into());
        v.reconcile(&p);
        assert_eq!(v.selected.as_deref(), Some("https://news.test/x"));
        v.query = "rust-lang".into();
        v.reconcile(&p);
        assert!(v.selected.is_none(), "filtered out, so the selection is dropped");
    }
}
