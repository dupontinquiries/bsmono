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

/// What a key means in Grow mode. This is `games/blackstar/tools/spine_grower.html`'s grammar,
/// key for key — the point of porting it verbatim is that anyone who learned the shape of a form
/// in that tool already knows how to drive CADscope.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GrowAction {
    /// Steering, in the sign the grower used: h/l yaw, j/k pitch, H/L roll. These repeat while
    /// held; everything else fires once per press.
    Yaw(f32),
    Pitch(f32),
    Roll(f32),
    /// w / b.
    Thicken(f32),
    /// f / p — flare and pinch.
    Flare(f32),
    /// ] / [.
    Twist(f32),
    /// 0.
    Straighten,
    /// e.
    ToggleRidge,
    /// ` or ~.
    ToggleWobble,
    /// 1..5.
    Section(u32),
    /// space — a knuckle.
    Node,
    /// t.
    Branch,
    /// y.
    Parent,
    /// m.
    Mirror,
}

impl GrowAction {
    /// True for the steering keys, which the app should apply every frame the key is down rather
    /// than once per press.
    pub fn is_continuous(self) -> bool {
        matches!(self, GrowAction::Yaw(_) | GrowAction::Pitch(_) | GrowAction::Roll(_))
    }
}

pub fn grow_key(ch: char) -> Option<GrowAction> {
    Some(match ch {
        'h' => GrowAction::Yaw(1.0),
        'l' => GrowAction::Yaw(-1.0),
        'j' => GrowAction::Pitch(-1.0),
        'k' => GrowAction::Pitch(1.0),
        'H' => GrowAction::Roll(1.0),
        'L' => GrowAction::Roll(-1.0),
        'w' => GrowAction::Thicken(1.0),
        'b' => GrowAction::Thicken(-1.0),
        'f' => GrowAction::Flare(1.0),
        'p' => GrowAction::Flare(-1.0),
        ']' => GrowAction::Twist(1.0),
        '[' => GrowAction::Twist(-1.0),
        '0' => GrowAction::Straighten,
        'e' => GrowAction::ToggleRidge,
        '~' | '`' => GrowAction::ToggleWobble,
        ' ' => GrowAction::Node,
        't' => GrowAction::Branch,
        'y' => GrowAction::Parent,
        'm' => GrowAction::Mirror,
        '1'..='5' => GrowAction::Section(ch.to_digit(10).unwrap()),
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
    fn grow_grammar_matches_the_spine_grower() {
        // Steering signs, straight from the grower's steer(): h yaws one way, l the other, j and k
        // pitch opposite each other, H and L roll opposite each other.
        assert_eq!(grow_key('h'), Some(GrowAction::Yaw(1.0)));
        assert_eq!(grow_key('l'), Some(GrowAction::Yaw(-1.0)));
        assert_eq!(grow_key('j'), Some(GrowAction::Pitch(-1.0)));
        assert_eq!(grow_key('k'), Some(GrowAction::Pitch(1.0)));
        assert_eq!(grow_key('H'), Some(GrowAction::Roll(1.0)));
        assert_eq!(grow_key('L'), Some(GrowAction::Roll(-1.0)));
        // w thickens and b narrows; f flares and p pinches.
        assert_eq!(grow_key('w'), Some(GrowAction::Thicken(1.0)));
        assert_eq!(grow_key('b'), Some(GrowAction::Thicken(-1.0)));
        assert_eq!(grow_key('f'), Some(GrowAction::Flare(1.0)));
        assert_eq!(grow_key('p'), Some(GrowAction::Flare(-1.0)));
        // brackets twist, 0 straightens, e ridges, backtick wobbles
        assert_eq!(grow_key(']'), Some(GrowAction::Twist(1.0)));
        assert_eq!(grow_key('['), Some(GrowAction::Twist(-1.0)));
        assert_eq!(grow_key('0'), Some(GrowAction::Straighten));
        assert_eq!(grow_key('e'), Some(GrowAction::ToggleRidge));
        assert_eq!(grow_key('`'), Some(GrowAction::ToggleWobble));
        assert_eq!(grow_key('~'), Some(GrowAction::ToggleWobble));
        // structure: space knuckles, t branches, y returns, m mirrors
        assert_eq!(grow_key(' '), Some(GrowAction::Node));
        assert_eq!(grow_key('t'), Some(GrowAction::Branch));
        assert_eq!(grow_key('y'), Some(GrowAction::Parent));
        assert_eq!(grow_key('m'), Some(GrowAction::Mirror));
        for d in 1..=5u32 {
            let ch = char::from_digit(d, 10).unwrap();
            assert_eq!(grow_key(ch), Some(GrowAction::Section(d)));
        }
        assert_eq!(grow_key('q'), None);
    }

    #[test]
    fn only_steering_repeats_while_held() {
        assert!(grow_key('h').unwrap().is_continuous());
        assert!(grow_key('k').unwrap().is_continuous());
        assert!(grow_key('L').unwrap().is_continuous());
        for ch in ['w', 'b', 'f', 'p', '[', ']', '0', 'e', ' ', 't', 'y', 'm', '1'] {
            assert!(!grow_key(ch).unwrap().is_continuous(), "{ch} must not auto-repeat");
        }
    }

    #[test]
    fn escape_clears_then_exits_modes() {
        assert_eq!(escape(Mode::Normal), (Mode::Normal, Some(Action::ClearSelection)));
        assert_eq!(escape(Mode::Grow), (Mode::Normal, None));
    }
}
