//! command_bar — the palette surface, as a thing the kit owns.
//!
//! snapclone's palette drew a translucent card over the page with a 45%
//! scrim. On a dark theme that scrim is invisible — dimming near-black by
//! 45% gives you near-black — so the page behind stayed at full contrast and
//! the suggestions read as loose text floating over a settings form. The fix
//! is not more scrim. It is deciding what the surface IS.
//!
//! Two answers, both here, because they suit different moments:
//!
//! * [`Mode::Bar`] — a slim strip at the bottom edge, the best match
//!   completed INLINE as ghost text, the rest as a short list above it. It
//!   never covers the page, so the page never has to be suppressed.
//! * [`Mode::Pane`] — a full overlay with a heavy scrim. Nothing behind it
//!   competes because nothing behind it is readable.
//!
//! The kit holds the surface ([`show`]: placement, scrim, keys, focus, the
//! one-line rows) and the completion logic; the app supplies the matches and
//! decides what one does. That split is the reusable part —
//! snapclone, logscope and gitmind each have a palette and each hand-rolled
//! one.

/// How the palette presents itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Slim strip at the bottom; the page stays readable behind it.
    Bar,
    /// Full overlay; the page is deliberately suppressed.
    Pane,
}

/// Scrim opacity for [`Mode::Pane`]. High enough that text behind it falls
/// out of reading contrast on a LIGHT palette too — a scrim that only works
/// on one theme is the bug this module exists to stop repeating.
pub const SCRIM_PANE: f32 = 0.78;
/// The bar draws no scrim. Covering nothing is the point.
pub const SCRIM_BAR: f32 = 0.0;

/// Rows the surface shows. Past this a palette stops being a shortcut and
/// becomes a list you read.
pub const MAX_ROWS: usize = 6;
/// Row height, at the touch floor so a thumb can hit one.
pub const ROW_H: f32 = 34.0;
/// The input's own height.
pub const INPUT_H: f32 = 34.0;

/// Pick a mode for the space available.
///
/// A bar is only worth it while the page behind stays useful. Once the bar
/// and its rows would cover half the surface, the pane is the more honest
/// answer: it admits it has taken the screen instead of half-covering it.
pub fn mode_for(surface_h: f32, rows: usize) -> Mode {
    let needed = INPUT_H + rows.min(MAX_ROWS) as f32 * ROW_H + 24.0;
    if needed > surface_h * 0.5 { Mode::Pane } else { Mode::Bar }
}

/// Height the surface wants for `rows` matches.
pub fn wanted_height(rows: usize) -> f32 {
    INPUT_H + rows.min(MAX_ROWS) as f32 * ROW_H + 16.0
}

/// The part of `best` that would be typed next — the ghost suffix shown
/// inline after the cursor.
///
/// `None` when the input is empty (ghosting a whole suggestion into an empty
/// box makes it look pre-filled), when the match adds nothing, or when it is
/// not a prefix at all. The prefix compare ignores case; the suffix comes
/// back in the CANDIDATE's case, because that is what accepting inserts.
pub fn ghost<'a>(input: &str, best: &'a str) -> Option<&'a str> {
    let t = input.trim_start();
    if t.is_empty() || best.len() <= t.len() {
        return None;
    }
    let (head, tail) = best.split_at(t.len());
    head.eq_ignore_ascii_case(t).then_some(tail)
}

/// What the input becomes when the ghost is accepted. What the user typed is
/// preserved verbatim, including its case — rewriting "FL" to "fl" under the
/// cursor is what makes inline completion feel like it is fighting you.
pub fn accept(input: &str, best: &str) -> String {
    match ghost(input, best) {
        Some(rest) => format!("{input}{rest}"),
        None => input.to_string(),
    }
}

/// Move a selection by `delta` over `n` rows, clamped rather than wrapped.
/// The list is short and ordered by score, so wrapping from the last row
/// back to the best one reads as having lost the selection.
pub fn move_cursor(cursor: usize, n: usize, delta: i32) -> usize {
    if n == 0 { return 0; }
    (cursor as i64 + delta as i64).clamp(0, n as i64 - 1) as usize
}

// ── the surface ─────────────────────────────────────────────────────────────
//
// Everything above is arithmetic. Until c19 that was all the kit owned, and
// every app drew its own palette on top of it — all of them the desktop shape:
// a card floating top-centre, input first, matches below. On a phone that puts
// the input at the far end of the screen from the thumb and from the on-screen
// keyboard, and the best match furthest of all.
//
// The canon, decided here once:
//
// * [`Placement::Bottom`] on a compact or touch surface. The card sits on the
//   bottom edge, the input is its LAST row, and the matches stack upward from it
//   with the BEST match nearest the input. It is the desktop palette turned
//   over, so that the two things a thumb needs — where you type and what you
//   most likely meant — are next to each other and next to the keyboard.
//   Arrow keys follow the picture: Up moves away from the input, toward worse
//   matches.
// * [`Placement::Top`] otherwise: the familiar card, top-centre, best match
//   first under the input.
//
// Placement is about REACH and comes from [`crate::Adaptive`]. Whether the page
// behind is dimmed is about COVERAGE and still comes from [`mode_for`] — the two
// are independent: a short phone gets a bottom card WITH a scrim.

/// Where the card sits and which way its rows run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Top-centre; input first, best match directly under it.
    Top,
    /// Bottom edge; input last, best match directly above it.
    Bottom,
}

/// Bottom for anything held in a hand.
pub fn placement_for(adaptive: &crate::Adaptive) -> Placement {
    if adaptive.is_compact() || adaptive.touch { Placement::Bottom } else { Placement::Top }
}

/// [`mode_for`] with the row height the surface will really use. `ROW_H` is the
/// pointer height; on a touch surface rows grow to the touch floor, and a bar
/// budgeted at 34pt rows that draws 44pt ones covers more than it admitted to.
pub fn mode_for_rows(surface_h: f32, rows: usize, row_h: f32) -> Mode {
    let needed = INPUT_H + rows.min(MAX_ROWS) as f32 * row_h.max(ROW_H) + 24.0;
    if needed > surface_h * 0.5 { Mode::Pane } else { Mode::Bar }
}

/// The cursor after an arrow key, in PICTURE terms. In [`Placement::Bottom`] the
/// best match is at the bottom, so Up means a higher index.
pub fn arrow(cursor: usize, n: usize, up: bool, placement: Placement) -> usize {
    let toward_worse = match placement {
        Placement::Top => !up,
        Placement::Bottom => up,
    };
    move_cursor(cursor, n, if toward_worse { 1 } else { -1 })
}

/// One match. `detail` is whatever goes beside the label in a weaker voice — a
/// shortcut, a path — and is elided rather than wrapped: a row is one line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    pub label: String,
    pub detail: String,
}

impl Row {
    pub fn new(label: impl Into<String>, detail: impl Into<String>) -> Row {
        Row { label: label.into(), detail: detail.into() }
    }
}

/// The surface's own state. The app owns the matches and what choosing one does.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub query: String,
    /// Index into the rows passed to [`show`]; 0 is always the best match,
    /// whichever way up it is drawn.
    pub cursor: usize,
    open: bool,
    wants_focus: bool,
    just_opened: bool,
}

impl State {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open empty. A no-op when already open, so two paths that both saw the
    /// same tap open it once.
    pub fn open(&mut self) {
        if !self.open {
            self.open = true;
            self.query.clear();
            self.cursor = 0;
            self.wants_focus = true;
            self.just_opened = true;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Replace the query from outside (a recognised utterance, a recalled
    /// command) and put the caret back in the box.
    pub fn set_query(&mut self, q: impl Into<String>) {
        self.query = q.into();
        self.cursor = 0;
        self.wants_focus = true;
    }
}

/// What happened this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    /// A row was chosen: Enter on the cursor, or a tap. The surface does NOT
    /// close itself — some choices refine the query instead of ending it.
    pub chosen: Option<usize>,
    pub query_changed: bool,
    /// An arrow key moved the cursor.
    pub navigated: bool,
    /// Esc or a click outside; the surface has already closed.
    pub dismissed: bool,
    pub placement: Placement,
    pub mode: Mode,
    pub rect: egui::Rect,
}

/// Draw the palette and handle its keys. Call every frame; it does nothing while
/// closed. `status` draws between the input and the matches, in both placements
/// — it is for the line that belongs WITH the input (a mic state, "heard: …").
pub fn show(
    ctx: &egui::Context,
    id_salt: &str,
    state: &mut State,
    rows: &[Row],
    hint: &str,
    colors: &crate::PaletteColors,
    adaptive: &crate::Adaptive,
    status: impl FnOnce(&mut egui::Ui),
) -> Outcome {
    use egui::Key;
    let screen = ctx.content_rect();
    let placement = placement_for(adaptive);
    let row_h = ROW_H.max(adaptive.min_touch_target);
    let shown = rows.len().min(MAX_ROWS);
    let mode = mode_for_rows(screen.height(), shown, row_h);
    let mut out = Outcome { chosen: None, query_changed: false, navigated: false, dismissed: false, placement, mode, rect: egui::Rect::NOTHING };
    if !state.open {
        return out;
    }
    state.cursor = state.cursor.min(shown.saturating_sub(1));

    // ---- keys. Consumed, so the page underneath does not also act on them. ----
    let (esc, up, down, tab, enter) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, Key::Escape),
            i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, Key::Tab),
            i.key_pressed(Key::Enter),
        )
    });
    if esc {
        state.close();
        out.dismissed = true;
        return out;
    }
    if up || down {
        state.cursor = arrow(state.cursor, shown, up, placement);
        out.navigated = true;
    }
    if tab {
        if let Some(best) = rows.first() {
            let accepted = accept(&state.query, &best.label);
            if accepted != state.query {
                state.query = accepted;
                out.query_changed = true;
            }
        }
    }
    if enter && shown > 0 {
        out.chosen = Some(state.cursor);
    }

    // ---- surface ----
    let width = (screen.width() - 24.0).min(520.0).max(120.0);
    if mode == Mode::Pane {
        let scrim = egui::Color32::from_black_alpha((SCRIM_PANE * 255.0) as u8);
        ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new((id_salt, "scrim")))).rect_filled(screen, 0.0, scrim);
    }
    let (anchor, offset) = match placement {
        Placement::Top => (egui::Align2::CENTER_TOP, egui::vec2(0.0, 48.0 + adaptive.safe_area.top)),
        Placement::Bottom => (egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -10.0 - adaptive.safe_area.bottom)),
    };
    let cursor = state.cursor;
    let no_match = shown == 0 && !state.query.trim().is_empty();
    let mut clicked = None;
    let area = egui::Area::new(egui::Id::new((id_salt, "surface"))).order(egui::Order::Foreground).anchor(anchor, offset).show(ctx, |ui| {
        egui::Frame::new().fill(colors.bg_raised).stroke(egui::Stroke::new(1.0_f32, colors.border)).corner_radius(10.0).inner_margin(10.0).show(ui, |ui| {
            ui.set_width(width);
            let draw_rows = |ui: &mut egui::Ui, clicked: &mut Option<usize>| {
                let order: Vec<usize> = match placement {
                    Placement::Top => (0..shown).collect(),
                    Placement::Bottom => (0..shown).rev().collect(),
                };
                for i in order {
                    let r = &rows[i];
                    let mut job = egui::text::LayoutJob::default();
                    job.append(&r.label, 0.0, egui::TextFormat { font_id: egui::FontId::proportional(14.0), color: colors.text, ..Default::default() });
                    if !r.detail.is_empty() {
                        job.append(&r.detail, 10.0, egui::TextFormat { font_id: egui::FontId::proportional(11.0), color: colors.text_weak, ..Default::default() });
                    }
                    // `.truncate()`, not the job's own wrap settings: the Button
                    // overrides those with its wrap mode. Without it a long detail
                    // wrapped each row to three lines and the "bar" covered a phone.
                    // `clicked_by`, not `clicked`: egui also reports a click for a
                    // FOCUSED widget when Enter is pressed, and its arrow-key focus
                    // navigation runs before we can consume the arrows. Up-arrow
                    // therefore moved focus onto the row above the input, and Enter
                    // then "clicked" that row instead of the one under the cursor.
                    if ui.add_sized([width, row_h], egui::Button::selectable(i == cursor, job).truncate()).clicked_by(egui::PointerButton::Primary) {
                        *clicked = Some(i);
                    }
                }
                if no_match {
                    ui.label(egui::RichText::new("no match").small().color(colors.text_weak));
                }
            };
            let draw_input = |ui: &mut egui::Ui, state: &mut State, out: &mut Outcome| {
                let r = ui.add(crate::text_field::edit((id_salt, "query"), &mut state.query).hint_text(hint).desired_width(width));
                // The surface is modal and has its own arrow navigation, so the
                // box keeps the keyboard for as long as it is open: asked for on
                // open, taken back if egui's focus navigation moved it, and locked
                // against the arrows and Tab that would move it again.
                if std::mem::take(&mut state.wants_focus) || !r.has_focus() {
                    r.request_focus();
                }
                ui.memory_mut(|m| m.set_focus_lock_filter(r.id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false }));
                if r.changed() {
                    state.cursor = 0;
                    out.query_changed = true;
                }
            };
            let ghost_line = |ui: &mut egui::Ui, state: &State| {
                if let Some(rest) = rows.first().and_then(|b| ghost(&state.query, &b.label)) {
                    ui.label(egui::RichText::new(format!("tab → {}{rest}", state.query)).small().color(colors.text_weak));
                }
            };
            match placement {
                Placement::Top => {
                    draw_input(ui, state, &mut out);
                    ghost_line(ui, state);
                    status(ui);
                    draw_rows(ui, &mut clicked);
                }
                Placement::Bottom => {
                    draw_rows(ui, &mut clicked);
                    status(ui);
                    ghost_line(ui, state);
                    draw_input(ui, state, &mut out);
                }
            }
        });
    });
    out.rect = area.response.rect;
    if clicked.is_some() {
        out.chosen = clicked;
    }

    // The click that OPENED the palette is, on that same frame, a click outside
    // it. Without the guard a tap on an app's palette button opened and closed
    // it in one frame, while a long press (no click on the opening frame) worked.
    let fresh = std::mem::take(&mut state.just_opened);
    if out.chosen.is_none() && !fresh && ctx.input(|i| i.pointer.any_click()) && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| !out.rect.contains(p)) {
        state.close();
        out.dismissed = true;
    }
    out
}

#[cfg(test)]
mod surface_tests {
    use super::*;

    const ROWS: [&str; 3] = ["step over", "continue", "step into"];

    struct Rig {
        ctx: egui::Context,
        colors: crate::PaletteColors,
        screen: egui::Rect,
        t: f64,
    }

    impl Rig {
        fn new(w: f32, h: f32) -> Rig {
            let ctx = egui::Context::default();
            let colors = crate::apply_palette(&ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0]));
            Rig { ctx, colors, screen: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h)), t: 0.0 }
        }

        /// One frame: a page with a button that opens the palette, then the palette.
        fn frame(&mut self, state: &mut State, rows: &[Row], events: Vec<egui::Event>) -> (Outcome, Vec<crate::layout_probe::Text>) {
            self.t += 0.05;
            let input = egui::RawInput { screen_rect: Some(self.screen), time: Some(self.t), events, ..Default::default() };
            let mut outcome = None;
            let colors = self.colors.clone();
            #[allow(deprecated)]
            let full = self.ctx.run(input, |ctx| {
                #[allow(deprecated)]
                egui::CentralPanel::default().show(ctx, |ui| {
                    if ui.button("open-palette").clicked() {
                        state.open();
                    }
                });
                let adaptive = crate::Adaptive::detect(ctx);
                outcome = Some(show(ctx, "test", state, rows, "type…", &colors, &adaptive, |_| {}));
            });
            (outcome.unwrap(), crate::layout_probe::texts(&full.shapes))
        }
    }

    fn rows() -> Vec<Row> {
        ROWS.iter().map(|l| Row::new(*l, "")).collect()
    }

    fn y_of(texts: &[crate::layout_probe::Text], needle: &str) -> f32 {
        texts.iter().find(|t| t.text == needle).unwrap_or_else(|| panic!("{needle:?} not drawn: {:?}", texts.iter().map(|t| &t.text).collect::<Vec<_>>())).rect.center().y
    }

    /// The canon itself: turned over on a phone, the usual way up on a desk.
    #[test]
    fn a_phone_gets_the_input_on_the_bottom_edge_with_the_best_match_next_to_it() {
        let mut rig = Rig::new(360.0, 760.0);
        let mut st = State::default();
        st.open();
        let mut got = None;
        for _ in 0..3 {
            got = Some(rig.frame(&mut st, &rows(), vec![]));
        }
        let (out, texts) = got.unwrap();
        assert_eq!(out.placement, Placement::Bottom);
        let (input, best, second, third) = (y_of(&texts, "type…"), y_of(&texts, ROWS[0]), y_of(&texts, ROWS[1]), y_of(&texts, ROWS[2]));
        assert!(input > best && best > second && second > third, "input {input} under best {best} under {second} under {third}");
        assert!(input > 760.0 * 0.85, "on the bottom edge, where the thumb and the keyboard are: {input}");
        assert!(out.rect.bottom() <= 760.0 && out.rect.left() >= 0.0 && out.rect.right() <= 360.0, "inside the screen: {:?}", out.rect);

        let mut rig = Rig::new(1300.0, 800.0);
        let mut st = State::default();
        st.open();
        let mut got = None;
        for _ in 0..3 {
            got = Some(rig.frame(&mut st, &rows(), vec![]));
        }
        let (out, texts) = got.unwrap();
        assert_eq!(out.placement, Placement::Top);
        let (input, best, second) = (y_of(&texts, "type…"), y_of(&texts, ROWS[0]), y_of(&texts, ROWS[1]));
        assert!(input < best && best < second, "desk: input {input} over best {best} over {second}");
        assert!(input < 400.0, "and in the top half: {input}");
    }

    #[test]
    fn arrows_follow_the_picture_and_enter_returns_the_row_under_the_cursor() {
        assert_eq!(arrow(0, 3, true, Placement::Bottom), 1, "bottom: up is away from the input, toward worse matches");
        assert_eq!(arrow(1, 3, false, Placement::Bottom), 0);
        assert_eq!(arrow(0, 3, false, Placement::Bottom), 0, "clamped at the best match");
        assert_eq!(arrow(0, 3, false, Placement::Top), 1);
        assert_eq!(arrow(0, 3, true, Placement::Top), 0);

        let key = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        let mut rig = Rig::new(360.0, 760.0);
        let mut st = State::default();
        st.open();
        rig.frame(&mut st, &rows(), vec![]);
        let (out, _) = rig.frame(&mut st, &rows(), vec![key(egui::Key::ArrowUp)]);
        assert!(out.navigated && st.cursor == 1);
        let (out, _) = rig.frame(&mut st, &rows(), vec![key(egui::Key::Enter)]);
        assert_eq!(out.chosen, Some(1), "Enter runs the row under the CURSOR, not whichever row egui's own arrow-key focus navigation landed on");
        assert!(st.is_open(), "choosing does not close it; the app decides");
        let (out, _) = rig.frame(&mut st, &rows(), vec![key(egui::Key::Escape)]);
        assert!(out.dismissed && !st.is_open());
    }

    /// A tap delivered inside one frame (a slow frame does this to every tap)
    /// opens the palette, and that same click must not count as a click outside.
    #[test]
    fn the_click_that_opens_it_does_not_also_close_it_and_a_later_one_outside_does() {
        let mut rig = Rig::new(360.0, 760.0);
        let mut st = State::default();
        let (_, texts) = { rig.frame(&mut st, &rows(), vec![]); rig.frame(&mut st, &rows(), vec![]) };
        let pos = texts.iter().find(|t| t.text == "open-palette").unwrap().rect.center();
        let click = |pos| vec![
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ];
        rig.frame(&mut st, &rows(), vec![egui::Event::PointerMoved(pos)]);
        rig.frame(&mut st, &rows(), click(pos));
        assert!(st.is_open(), "the tap opened it");
        rig.frame(&mut st, &rows(), vec![]);
        assert!(st.is_open(), "and it stayed open");

        let away = egui::pos2(180.0, 200.0);
        rig.frame(&mut st, &rows(), vec![egui::Event::PointerMoved(away)]);
        let (out, _) = rig.frame(&mut st, &rows(), click(away));
        assert!(out.dismissed && !st.is_open(), "click-outside still dismisses");
    }

    #[test]
    fn a_row_is_one_line_however_long_its_detail_and_a_short_screen_gets_the_scrim() {
        let long = Row::new("#0 std::panicking::rust_panic", "/rustc/ac68faa20c58cbccd01ee7208bf3b6e93a7d7f96/library/std/src/panicking.rs:886 and then some more of it");
        let many: Vec<Row> = (0..6).map(|_| long.clone()).collect();
        let mut rig = Rig::new(360.0, 760.0);
        let mut st = State::default();
        st.open();
        let mut out = None;
        for _ in 0..3 {
            out = Some(rig.frame(&mut st, &many, vec![]).0);
        }
        let out = out.unwrap();
        let budget = wanted_height(6) - INPUT_H - 16.0 + 6.0 * (44.0 - ROW_H) + 110.0;
        assert!(out.rect.height() <= budget, "six one-line rows, not six wrapped ones: {} > {budget}", out.rect.height());
        assert!(out.rect.width() <= 360.0);

        // Coverage, not placement, decides the scrim.
        assert_eq!(mode_for_rows(760.0, 6, 44.0), Mode::Bar);
        assert_eq!(mode_for_rows(420.0, 6, 44.0), Mode::Pane, "half of a phone with its keyboard up");
        assert_eq!(mode_for_rows(760.0, 6, 34.0), mode_for(760.0, 6), "at pointer height it is the old rule");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ghost_completes_only_what_the_input_actually_prefixes() {
        assert_eq!(ghost("fl", "flip camera"), Some("ip camera"));
        assert_eq!(ghost("FL", "flip camera"), Some("ip camera"), "prefix ignores case");
        assert_eq!(ghost("", "flip camera"), None, "an empty box must not look pre-filled");
        assert_eq!(ghost("  ", "flip camera"), None);
        assert_eq!(ghost("xyz", "flip camera"), None, "not a prefix");
        assert_eq!(ghost("flip camera", "flip camera"), None, "nothing left to add");
        assert_eq!(ghost("flip camera and more", "flip camera"), None, "input is longer");
    }

    #[test]
    fn accepting_keeps_what_the_user_typed_verbatim() {
        assert_eq!(accept("FL", "flip camera"), "FLip camera");
        assert_eq!(accept("fl", "flip camera"), "flip camera");
        assert_eq!(accept("xyz", "flip camera"), "xyz", "no match, no change");
        assert_eq!(accept("", "flip camera"), "");
    }

    #[test]
    fn a_short_surface_gets_the_pane_and_a_tall_one_gets_the_bar() {
        assert_eq!(mode_for(320.0, 5), Mode::Pane);
        assert_eq!(mode_for(900.0, 5), Mode::Bar);
        assert_eq!(mode_for(320.0, 0), Mode::Bar, "nothing to show keeps the page");
        assert_eq!(mode_for(400.0, 6), Mode::Pane, "the rows decide, not height alone");
    }

    #[test]
    fn the_surface_never_asks_for_more_rows_than_it_shows() {
        let capped = wanted_height(MAX_ROWS);
        assert_eq!(wanted_height(MAX_ROWS + 40), capped, "extra matches do not grow it");
        assert!(wanted_height(0) >= INPUT_H, "the input still needs room");
        assert!(wanted_height(3) < capped);
    }

    #[test]
    fn the_cursor_clamps_rather_than_wrapping_past_the_best_match() {
        assert_eq!(move_cursor(0, 5, -1), 0);
        assert_eq!(move_cursor(4, 5, 1), 4);
        assert_eq!(move_cursor(2, 5, -1), 1);
        assert_eq!(move_cursor(2, 5, 3), 4);
        assert_eq!(move_cursor(3, 0, 1), 0, "no rows is not a panic");
    }

    #[test]
    fn the_pane_scrim_is_strong_enough_to_matter_on_a_dark_theme() {
        // The bug this replaces: 0.45 over a near-black page dims near-black
        // to near-black, and the page behind stays fully legible.
        assert!(SCRIM_PANE >= 0.7, "a weak scrim is the fault being fixed");
        assert_eq!(SCRIM_BAR, 0.0, "the bar covers nothing on purpose");
    }
}
