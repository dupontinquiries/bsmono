//! replicatorlayout.rs — where replicator mode's columns go, and what Enter does when it is open.
//!
//! Pure and std-only, in the `mobilelayout` tradition: these are RULES, and a rule that can only
//! be checked by looking at a running app on a phone is a rule that gets broken quietly. The
//! drawing lives in `replicatorui`, the mode's state in `tabs::chat`.
//!
//! ## The bug these rules exist for
//!
//! The first version computed `col_w = (available / n).max(120.0)` and drew the columns in a
//! plain `horizontal_top`. Past four columns `n * 120` exceeds the width of a laptop window, and
//! egui's horizontal layout does not wrap or clip — it just keeps placing widgets, so headers,
//! assignment fields and transcripts marched off the right edge of the application. `.max()` is
//! how a fitting rule *sounds* right and overflows anyway: it protects the column and forgets the
//! container. `columns()` returns a set whose total is never wider than what it was given, and
//! says how many of the fan is showing so the caller can scroll to the rest instead of drawing
//! past the edge.
//!
//! ## Splitting rather than stacking
//!
//! The panel also used to be drawn INSIDE the composer's vertical flow, below the message log —
//! a second chat area stacked under the first, pushing the composer down as it filled. A fan of
//! workers is a parallel view of the same conversation, not a continuation of it, so it belongs
//! beside the log (`Split::Side`) where both scroll independently under one composer. On a phone
//! there is no width for two legible columns of prose, and side-by-side becomes two unreadable
//! ribbons: `Split::Stacked` says to show one at a time instead.

/// Narrowest column that still reads as prose rather than as a ribbon of two-word lines.
/// A chat bubble at 240pt fits roughly 6-8 words per line at the composer's font size.
pub const MIN_COL_PT: f32 = 240.0;
/// Widest useful column. Past this a column is just wasting the split — the fan's value is
/// several transcripts at once, and one fat column with empty neighbours is the wrong trade.
pub const MAX_COL_PT: f32 = 420.0;
/// The main log's floor. Below this the log stops being readable, and a split that makes BOTH
/// halves unusable is worse than showing one of them.
pub const MIN_LOG_PT: f32 = 320.0;
/// Space the fan's own frame costs (scrollbar + margins), taken off before columns are measured.
pub const FAN_CHROME_PT: f32 = 16.0;

/// How the fan's columns fit into the width it was given.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Columns {
    /// Width of ONE column, in points.
    pub col_w: f32,
    /// How many are drawn at once. `visible * col_w` never exceeds the available width — that
    /// invariant is the whole point of this type.
    pub visible: usize,
    /// True when some columns are off-view and the caller must scroll horizontally to reach
    /// them. False means the whole fan is on screen.
    pub scroll: bool,
}

/// Fit `n` columns into `avail_pt`.
///
/// Three regimes, in order: the fan fits with room (columns share the width, capped so they do
/// not become absurd); the fan fits exactly at the legible minimum; the fan does NOT fit, in
/// which case as many as fit are shown at the minimum width and `scroll` is set. There is no
/// fourth regime where the columns are drawn wider than the container — that was the bug.
pub fn columns(avail_pt: f32, n: usize) -> Columns {
    let n = n.max(1);
    let avail = avail_pt.max(0.0);
    // How many legible columns the width can hold at all. At least one: a viewport too narrow
    // for even one column still has to draw something, and the column's own wrapping is a
    // better answer than refusing to render.
    let capacity = (avail / MIN_COL_PT).floor().max(1.0) as usize;
    if capacity >= n {
        // everything fits — share the width, but do not let two columns become two billboards
        let share = (avail / n as f32).min(MAX_COL_PT);
        return Columns { col_w: share.max(MIN_COL_PT.min(avail.max(1.0))), visible: n, scroll: false };
    }
    // does not fit: show what fits, at a width that divides the space evenly so the last visible
    // column ends AT the edge rather than half-clipped (a half-column reads as a rendering bug,
    // and the scrollbar is what says there is more).
    let visible = capacity;
    let col_w = (avail / visible as f32).min(MAX_COL_PT);
    Columns { col_w, visible, scroll: true }
}

/// Where the fan goes relative to the message log.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Split {
    /// Side by side: the fan takes `fan_pt` on the right, the log keeps the rest.
    Side { fan_pt: f32 },
    /// Not enough width for both: the two share the area, one at a time.
    Stacked,
}

/// Decide the split for a window of `window_pt` showing `n` columns.
///
/// The fan asks for what its columns actually need (bounded by MAX_COL_PT each) rather than a
/// fixed fraction — two workers on a wide monitor should not eat 60% of the window — and the log
/// keeps MIN_LOG_PT no matter what. If honouring both would leave the fan below one legible
/// column, there is no honest side-by-side and the answer is Stacked.
pub fn split(window_pt: f32, n: usize) -> Split {
    let want = (n.max(1) as f32 * MAX_COL_PT).min(window_pt * 0.6) + FAN_CHROME_PT;
    let room = window_pt - MIN_LOG_PT;
    if room < MIN_COL_PT + FAN_CHROME_PT { return Split::Stacked; }
    Split::Side { fan_pt: want.min(room).max(MIN_COL_PT + FAN_CHROME_PT) }
}

// ── transcript lines ─────────────────────────────────────────────────────────────────────────

/// One line of a worker's transcript, with WHO SAID IT recorded rather than inferred.
///
/// The lane used to be a bare `Vec<String>` in strict prompt/reply alternation, so the renderer
/// could have derived the role from the index — until a worker errors out or is still streaming
/// and the alternation breaks, at which point every following bubble is attributed to the wrong
/// speaker. That matters beyond cosmetics: partners quote each other by this text
/// (`peer_context`'s "[w2 last said]"), so a mis-attributed line is a lie that enters another
/// worker's prompt.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LaneLine {
    /// True for the operator's message to this worker, false for the worker's own reply.
    pub from_operator: bool,
    pub text: String,
}

impl LaneLine {
    pub fn operator(text: impl Into<String>) -> Self { Self { from_operator: true, text: text.into() } }
    pub fn worker(text: impl Into<String>) -> Self { Self { from_operator: false, text: text.into() } }
}

// ── Enter routing ────────────────────────────────────────────────────────────────────────────
/// Which text field has the keyboard, as far as the Enter rule is concerned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Composer,
    /// The assignment field of column `id` — "what this one works on".
    Assignment(usize),
    Elsewhere,
}

/// The modifier state at the moment Enter was pressed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mods { pub shift: bool, pub ctrl: bool }

/// What pressing Enter should do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EnterAction {
    /// Insert a newline (the composer is multiline; this is the universal Shift+Enter).
    Newline,
    /// Send to whatever the composer is currently aimed at — one column, or all of them.
    SendAimed,
    /// Send to every column regardless of the current aim.
    SendBroadcast,
    /// Commit this column's assignment, aim the composer at it, and move the keyboard there —
    /// so "name a worker, describe its task, send" is one uninterrupted flow.
    AimAndCompose(usize),
    Ignore,
}

/// The Enter rule.
///
/// Deliberate orderings:
/// * The assignment field is checked FIRST and ignores Shift, because it is single-line — there
///   is no newline to insert, so Shift+Enter there would otherwise do nothing at all.
/// * Shift+Enter is a newline everywhere else, always. Overloading the one keystroke every text
///   box on the planet uses for "new line" is not cleverness.
/// * Ctrl+Enter broadcasts only when a fan is open and the composer is AIMED. With no fan, or
///   already broadcasting, it is the same send as Enter rather than a second, subtly different
///   one the operator has to keep straight.
pub fn enter_action(fan_open: bool, aim: Option<usize>, focus: Focus, m: Mods) -> EnterAction {
    match focus {
        Focus::Assignment(id) if fan_open => EnterAction::AimAndCompose(id),
        Focus::Assignment(_) => EnterAction::Ignore,
        Focus::Elsewhere => EnterAction::Ignore,
        Focus::Composer => {
            if m.shift { return EnterAction::Newline; }
            if m.ctrl && fan_open && aim.is_some() { return EnterAction::SendBroadcast; }
            EnterAction::SendAimed
        }
    }
}

/// The label for the send affordance, so the button and the key agree about what will happen.
/// A fan whose composer is aimed at one column while the button says "Send" is how an operator
/// broadcasts a message they meant for one worker.
pub fn enter_hint(fan_open: bool, aim: Option<usize>, n: usize) -> String {
    if !fan_open { return "Send ▶".into(); }
    match aim {
        Some(id) => format!("▶ w{id}  ·  ⌃⏎ all"),
        None => format!("▶ all {n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariant the original code broke: whatever is drawn fits in what was given.
    #[test]
    fn drawn_width_never_exceeds_the_container() {
        for avail in [120.0f32, 300.0, 480.0, 700.0, 1024.0, 1600.0, 2560.0] {
            for n in 1..=12usize {
                let c = columns(avail, n);
                let total = c.col_w * c.visible as f32;
                assert!(total <= avail.max(MIN_COL_PT) + 0.001,
                        "avail {avail} n {n}: drew {total} ({} × {})", c.visible, c.col_w);
                assert!(c.visible >= 1 && c.visible <= n);
            }
        }
    }

    #[test]
    fn the_old_rule_would_have_failed_that() {
        // (avail / n).max(120.0) at eight columns in a 900pt window: 8 × 120 = 960 > 900. The
        // regression this test guards is not hypothetical — it is what shipped.
        let avail = 900.0f32;
        let n = 8usize;
        let old = (avail / n as f32).max(120.0) * n as f32;
        assert!(old > avail, "the old rule overflowed by {}pt", old - avail);
        let c = columns(avail, n);
        assert!(c.col_w * c.visible as f32 <= avail);
        assert!(c.scroll, "and it says the rest are reachable by scrolling");
    }

    #[test]
    fn a_fan_that_fits_does_not_ask_for_a_scrollbar() {
        let c = columns(1200.0, 3);
        assert_eq!(c.visible, 3);
        assert!(!c.scroll);
        assert!(c.col_w >= MIN_COL_PT && c.col_w <= MAX_COL_PT);
    }

    #[test]
    fn two_columns_on_a_wide_monitor_do_not_become_billboards() {
        let c = columns(2560.0, 2);
        assert!(c.col_w <= MAX_COL_PT, "capped at {MAX_COL_PT}, got {}", c.col_w);
    }

    #[test]
    fn a_phone_width_still_renders_one_column() {
        let c = columns(360.0, 4);
        assert_eq!(c.visible, 1, "one legible column beats four ribbons");
        assert!(c.scroll);
    }

    #[test]
    fn the_log_keeps_its_floor_or_the_split_is_refused() {
        assert_eq!(split(380.0, 3), Split::Stacked, "a phone has no room for both");
        match split(1400.0, 3) {
            Split::Side { fan_pt } => {
                assert!(1400.0 - fan_pt >= MIN_LOG_PT, "log kept {}pt", 1400.0 - fan_pt);
                assert!(fan_pt >= MIN_COL_PT);
            }
            Split::Stacked => panic!("1400pt has room for a side split"),
        }
    }

    #[test]
    fn the_fan_asks_for_what_it_needs_not_a_fixed_fraction() {
        // one worker on a wide monitor must not claim 60% of it
        let Split::Side { fan_pt } = split(2560.0, 1) else { panic!("wide window splits") };
        assert!(fan_pt <= MAX_COL_PT + FAN_CHROME_PT, "one column asked for {fan_pt}pt");
    }

    #[test]
    fn shift_enter_is_a_newline_and_stays_one() {
        for fan in [false, true] {
            for aim in [None, Some(2)] {
                assert_eq!(enter_action(fan, aim, Focus::Composer, Mods { shift: true, ctrl: false }),
                           EnterAction::Newline);
                assert_eq!(enter_action(fan, aim, Focus::Composer, Mods { shift: true, ctrl: true }),
                           EnterAction::Newline, "shift wins over ctrl — the newline is sacred");
            }
        }
    }

    #[test]
    fn ctrl_enter_breaks_out_of_an_aim_and_is_otherwise_just_send() {
        assert_eq!(enter_action(true, Some(1), Focus::Composer, Mods { ctrl: true, shift: false }),
                   EnterAction::SendBroadcast);
        // already broadcasting, or no fan at all: one send, not two spellings of it
        assert_eq!(enter_action(true, None, Focus::Composer, Mods { ctrl: true, shift: false }),
                   EnterAction::SendAimed);
        assert_eq!(enter_action(false, None, Focus::Composer, Mods { ctrl: true, shift: false }),
                   EnterAction::SendAimed);
    }

    #[test]
    fn enter_in_an_assignment_field_aims_and_hands_over_the_keyboard() {
        assert_eq!(enter_action(true, None, Focus::Assignment(3), Mods::default()),
                   EnterAction::AimAndCompose(3));
        // single-line field: Shift must not swallow the gesture into a newline that cannot exist
        assert_eq!(enter_action(true, Some(0), Focus::Assignment(3), Mods { shift: true, ctrl: false }),
                   EnterAction::AimAndCompose(3));
    }

    #[test]
    fn enter_elsewhere_does_nothing() {
        assert_eq!(enter_action(true, None, Focus::Elsewhere, Mods::default()), EnterAction::Ignore);
    }

    #[test]
    fn the_hint_says_where_the_message_is_going() {
        assert_eq!(enter_hint(false, None, 4), "Send ▶");
        assert!(enter_hint(true, None, 4).contains('4'), "broadcast names the count");
        let aimed = enter_hint(true, Some(2), 4);
        assert!(aimed.contains("w2"), "an aimed composer names the column: {aimed}");
    }
}
