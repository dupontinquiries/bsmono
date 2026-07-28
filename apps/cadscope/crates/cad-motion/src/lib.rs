//! cad-motion — the modal input grammar, pure and testable in the `logscope-fan::vimmode` mold:
//! this crate owns the Mode enum and maps keys to ACTIONS; the egui layer only reads keys and
//! executes actions. M0 ships the Normal-mode camera/selection slice; Grow (the spine_grower.html
//! grammar: h l yaw, j k pitch, H L roll, [ ] helix twist, w b thicken/narrow, e/p/f ridge/pinch/
//! flare, 1-5 sections, space node, t branch, y parent, m mirror) and Sketch land at M2, reusing
//! this same shape. Colon-commands route through logscope-fan's cmdpalette when the liaison wires
//! in at M3 — do not grow a second command parser here.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Normal,
    /// M2: the spine grammar.
    Grow,
    /// M2: plane-locked 2D.
    Sketch,
}

/// What a keypress means. The app executes these; nothing here touches a camera or a scene.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    OrbitLeft,
    OrbitRight,
    OrbitUp,
    OrbitDown,
    DollyIn,
    DollyOut,
    /// Frame the current selection (or the whole model when nothing is selected).
    FrameSelection,
    /// Cycle selection to the next / previous part in evaluation order.
    SelectNext,
    SelectPrev,
    ClearSelection,
    EnterMode(Mode),
}

/// Normal-mode mapping. hjkl orbit (the camera IS the object you steer while inspecting), +/-
/// dolly, f frames, n/N cycles parts, Esc clears. `z` stays reserved as the camera prefix and
/// `space` as the leader, both arriving with the vimmode integration (M2) — do not reassign them.
pub fn normal_key(ch: char) -> Option<Action> {
    Some(match ch {
        'h' => Action::OrbitLeft,
        'l' => Action::OrbitRight,
        'j' => Action::OrbitDown,
        'k' => Action::OrbitUp,
        '+' | '=' => Action::DollyIn,
        '-' => Action::DollyOut,
        'f' => Action::FrameSelection,
        'n' => Action::SelectNext,
        'N' => Action::SelectPrev,
        'g' => Action::EnterMode(Mode::Grow),
        _ => return None,
    })
}

/// Escape is modal glue, not a printable char — handled once here so every mode agrees.
pub fn escape(mode: Mode) -> (Mode, Option<Action>) {
    match mode {
        Mode::Normal => (Mode::Normal, Some(Action::ClearSelection)),
        _ => (Mode::Normal, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hjkl_orbits() {
        assert_eq!(normal_key('h'), Some(Action::OrbitLeft));
        assert_eq!(normal_key('l'), Some(Action::OrbitRight));
        assert_eq!(normal_key('j'), Some(Action::OrbitDown));
        assert_eq!(normal_key('k'), Some(Action::OrbitUp));
    }

    #[test]
    fn reserved_prefixes_stay_unmapped() {
        assert_eq!(normal_key('z'), None, "z is the camera prefix (vimmode, M2)");
        assert_eq!(normal_key(' '), None, "space is the leader (vimmode, M2)");
    }

    #[test]
    fn escape_clears_then_exits_modes() {
        assert_eq!(escape(Mode::Normal), (Mode::Normal, Some(Action::ClearSelection)));
        assert_eq!(escape(Mode::Grow), (Mode::Normal, None));
    }
}
