//! log_gestures — default touch gesture map for the fast log viewer (the Diagnose/Peek surface).
//!
//! The viewer had unfinished touch bits (tap-to-fold, section select) that never landed a coherent
//! scheme. Now that touchkit + the workbench nav exist, this picks **opinionated defaults** and
//! resolves a `(mode, gesture, hit)` into a single [`LogAction`] the render code performs — so the
//! interaction lives in one tested place instead of scattered hit-tests. Pure + lean-buildable.
//!
//! Design constraints that shaped the choices:
//! * touchkit exposes Tap / DoubleTap / LongPress / Pan / Swipe(4) / Pinch — **no finger count**,
//!   so genuinely cross-cutting actions (jump-to-next-error) belong on a nav-rail *button*, not a
//!   phantom two-finger gesture. Gestures are reserved for the direct-manipulation behaviours.
//! * Horizontal swipe is already "switch related tab" fleet-wide (workbench nav) — we don't
//!   overload it, we delegate to it, so the muscle memory is consistent across the surgical tools.
//!
//! ## The defaults
//! | gesture | on… | action |
//! |---|---|---|
//! | Tap | fold chevron | toggle that fold |
//! | Double-tap | any line | toggle that line's section (a big target vs the tiny chevron) |
//! | Pinch in / out | anywhere | **semantic zoom**: fold-all / unfold-all |
//! | Long-press | a line | begin area selection (enter Select mode) |
//! | Long-press | a section header | select the whole section |
//! | Pan (Select mode) | over lines | extend the selection (area select) |
//! | Tap (Select mode) | a line | extend selection to there (range A→B) |
//! | Long-press (Select mode) | — | copy the selection |
//! | Vertical Pan / Swipe (Read) | — | scroll (with flick momentum) |
//! | Horizontal swipe | — | switch related tab (delegates to workbench nav) |
//!
//! Copy/paste note: a log view is read-only, so **Copy** is the selection action (also a toolbar
//! button for discoverability) and **Paste** is contextual — it targets the filter/search field,
//! not the canvas — so it isn't a canvas gesture here.

use touchkit::Gesture;

/// What sits under the touch point; the caller resolves this by hit-testing the rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The fold chevron of a foldable section starting at this row.
    Chevron(usize),
    /// A section header (a collapsed run's first line — e.g. a `shorten` `^^` group head).
    SectionHeader(usize),
    /// An ordinary log line.
    Line(usize),
    /// Gutter / empty space.
    Empty,
}

/// Viewer interaction mode. `Read` is the default; a selection puts it in `Select`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Read,
    Select,
}

/// Step direction for navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Prev,
    Next,
}

/// A resolved viewer action. `enters_select()` / `exits_select()` tell the caller how to move the
/// mode so the mapping itself stays a pure function.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogAction {
    None,
    Scroll { dy: f32 },
    /// Toggle the fold of the section at this row.
    ToggleFold(usize),
    /// Semantic zoom: collapse everything to section headers.
    FoldAll,
    /// Semantic zoom: expand everything.
    UnfoldAll,
    /// Anchor a new area selection at this row (→ Select mode).
    BeginSelection(usize),
    /// Grow the active selection to this row.
    ExtendSelection(usize),
    /// Select every line of the section at this row.
    SelectSection(usize),
    /// Copy the current selection (→ back to Read).
    Copy,
    /// Jump to the prev/next high-severity line — nav-rail button, surfaced here for one vocabulary.
    JumpSeverity(Step),
    /// Switch to the prev/next related tab (delegated to workbench nav).
    SwitchTab(Step),
}

impl LogAction {
    /// Does performing this action put the viewer into Select mode?
    pub fn enters_select(&self) -> bool {
        matches!(self, LogAction::BeginSelection(_) | LogAction::SelectSection(_))
    }
    /// Does it return to Read mode?
    pub fn exits_select(&self) -> bool {
        matches!(self, LogAction::Copy)
    }
}

/// Resolve a gesture into an action given the current mode and what's under the touch.
pub fn map_gesture(mode: Mode, g: &Gesture, hit: Hit) -> LogAction {
    match g {
        // ── folding ────────────────────────────────────────────────────────────
        Gesture::Tap(_) => match (mode, hit) {
            // in Select mode a tap on a row extends the range (A→B); otherwise it toggles folds
            (Mode::Select, Hit::Line(i) | Hit::SectionHeader(i)) => LogAction::ExtendSelection(i),
            (_, Hit::Chevron(i) | Hit::SectionHeader(i)) => LogAction::ToggleFold(i),
            _ => LogAction::None,
        },
        Gesture::DoubleTap(_) => match hit {
            Hit::Line(i) | Hit::SectionHeader(i) | Hit::Chevron(i) => LogAction::ToggleFold(i),
            Hit::Empty => LogAction::None,
        },
        Gesture::Pinch { rel, .. } => {
            // spread (rel > 1) shows more detail → unfold; pinch (rel < 1) shows less → fold.
            // (caller applies on a threshold so a single pinch doesn't thrash.)
            if *rel < 1.0 { LogAction::FoldAll } else { LogAction::UnfoldAll }
        }

        // ── selection + copy ───────────────────────────────────────────────────
        Gesture::LongPress(_) => match (mode, hit) {
            (Mode::Select, _) => LogAction::Copy, // you've selected; long-press to copy
            (_, Hit::SectionHeader(i) | Hit::Chevron(i)) => LogAction::SelectSection(i),
            (_, Hit::Line(i)) => LogAction::BeginSelection(i),
            (_, Hit::Empty) => LogAction::None,
        },
        Gesture::Pan { delta, .. } => match mode {
            // dragging in Select mode paints the selection across rows…
            Mode::Select => match hit {
                Hit::Line(i) | Hit::SectionHeader(i) | Hit::Chevron(i) => LogAction::ExtendSelection(i),
                Hit::Empty => LogAction::None,
            },
            // …otherwise it scrolls
            Mode::Read => LogAction::Scroll { dy: delta.y },
        },

        // ── navigation ─────────────────────────────────────────────────────────
        Gesture::SwipeUp { vel } => LogAction::Scroll { dy: -vel * 0.1 },
        Gesture::SwipeDown { vel } => LogAction::Scroll { dy: vel * 0.1 },
        Gesture::SwipeLeft { .. } => LogAction::SwitchTab(Step::Next),
        Gesture::SwipeRight { .. } => LogAction::SwitchTab(Step::Prev),

        _ => LogAction::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use touchkit::Vec2;

    fn at(x: f32, y: f32) -> Vec2 { Vec2::new(x, y) }

    #[test]
    fn folding_defaults() {
        // tap the chevron toggles its fold
        assert_eq!(map_gesture(Mode::Read, &Gesture::Tap(at(0.0, 0.0)), Hit::Chevron(3)), LogAction::ToggleFold(3));
        // double-tap anywhere on a line toggles its section (big target)
        assert_eq!(map_gesture(Mode::Read, &Gesture::DoubleTap(at(0.0, 0.0)), Hit::Line(7)), LogAction::ToggleFold(7));
        // pinch = semantic zoom
        assert_eq!(map_gesture(Mode::Read, &Gesture::Pinch { rel: 0.8, scale: 0.8, center: at(0.0, 0.0) }, Hit::Empty), LogAction::FoldAll);
        assert_eq!(map_gesture(Mode::Read, &Gesture::Pinch { rel: 1.3, scale: 1.3, center: at(0.0, 0.0) }, Hit::Empty), LogAction::UnfoldAll);
    }

    #[test]
    fn selection_flow_and_mode_transitions() {
        // long-press a line begins an area selection and enters Select mode
        let begin = map_gesture(Mode::Read, &Gesture::LongPress(at(0.0, 0.0)), Hit::Line(4));
        assert_eq!(begin, LogAction::BeginSelection(4));
        assert!(begin.enters_select());
        // long-press a header selects the whole section
        assert_eq!(map_gesture(Mode::Read, &Gesture::LongPress(at(0.0, 0.0)), Hit::SectionHeader(2)), LogAction::SelectSection(2));
        // dragging in Select mode extends the selection
        assert_eq!(map_gesture(Mode::Select, &Gesture::Pan { delta: at(0.0, 30.0), pos: at(0.0, 0.0) }, Hit::Line(9)), LogAction::ExtendSelection(9));
        // long-press while selecting copies, and returns to Read
        let copy = map_gesture(Mode::Select, &Gesture::LongPress(at(0.0, 0.0)), Hit::Line(9));
        assert_eq!(copy, LogAction::Copy);
        assert!(copy.exits_select());
    }

    #[test]
    fn scroll_vs_select_pan_depends_on_mode() {
        // read-mode drag scrolls
        assert_eq!(map_gesture(Mode::Read, &Gesture::Pan { delta: at(0.0, 12.0), pos: at(0.0, 0.0) }, Hit::Line(1)), LogAction::Scroll { dy: 12.0 });
        // select-mode drag selects (does NOT scroll)
        assert_eq!(map_gesture(Mode::Select, &Gesture::Pan { delta: at(0.0, 12.0), pos: at(0.0, 0.0) }, Hit::Line(1)), LogAction::ExtendSelection(1));
    }

    #[test]
    fn horizontal_swipe_delegates_to_tab_nav() {
        assert_eq!(map_gesture(Mode::Read, &Gesture::SwipeLeft { vel: 800.0 }, Hit::Empty), LogAction::SwitchTab(Step::Next));
        assert_eq!(map_gesture(Mode::Read, &Gesture::SwipeRight { vel: 800.0 }, Hit::Empty), LogAction::SwitchTab(Step::Prev));
    }
}
