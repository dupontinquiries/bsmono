//! chatgesture.rs — swipes onto chat actions, and the four ways that goes wrong.
//!
//! ## Why a gesture is the right fix for a crowded bar
//!
//! Messages carry `variants: Vec<String>` — every regeneration of a turn. Switching between them
//! currently needs arrow buttons, in the row that was overlapping. A horizontal swipe is how every
//! photo gallery does this, and it REMOVES controls from the bar rather than adding to it. That is
//! why this is worth doing rather than pleasant.
//!
//! `uikit-gesture` already supplies the mechanics: `DetentFired` for a one-step nudge, `PageJump` for a
//! committed throw. What is missing is what those mean here, and the guards.
//!
//! ## The four failures
//!
//! 1. **Scroll conflict.** A chat is a vertical scroller. Without a direction lock, every scroll with a
//!    little sideways drift flickers through variants — and the user cannot tell why. The lock is
//!    decided once at gesture start and held.
//! 2. **Switching mid-stream.** `cur_mut()` returns `&mut variants[cur]`. Changing `cur` while tokens
//!    are arriving means the rest of the reply lands in a DIFFERENT variant — silently corrupting two
//!    at once.
//! 3. **Wrapping at the ends.** Wrapping from the last variant to the first looks like a new one
//!    arrived. Clamping is the honest behaviour: the swipe does nothing and the content does not move.
//! 4. **Ambiguous target.** A swipe on a message means variants; a swipe on empty space means
//!    branches. Resolving that by proximity would make the same gesture do different things depending
//!    on a pixel.

/// What a swipe was aimed at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    /// On a message: step through that turn's regenerations.
    Message(usize),
    /// On the transcript background: step through branches.
    Background,
}

/// What the chat should do.
#[derive(Clone, PartialEq, Debug)]
pub enum Action {
    ShowVariant { msg: usize, delta: i32 },
    SwitchBranch { delta: i32 },
    /// Recognised but refused, with a reason a UI can show.
    Refused(String),
    None,
}

/// Which axis a gesture claimed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis { Horizontal, Vertical, Undecided }

/// Points of travel before the axis is committed.
///
/// Below this a gesture is ambiguous and must do nothing — acting early is what makes a scroll flicker.
pub const AXIS_LOCK_PT: f32 = 12.0;

/// Decide the axis once, from the first meaningful movement.
///
/// Requires a clear majority rather than a bare comparison: a 51/49 split is a vertical scroll with
/// drift, and treating it as horizontal is exactly the flicker this prevents.
pub fn lock_axis(dx: f32, dy: f32) -> Axis {
    let (ax, ay) = (dx.abs(), dy.abs());
    if ax.max(ay) < AXIS_LOCK_PT { return Axis::Undecided; }
    if ax > ay * 1.5 { Axis::Horizontal } else if ay > ax * 1.5 { Axis::Vertical } else { Axis::Undecided }
}

/// Map a locked horizontal gesture to an action.
///
/// `streaming` refuses mid-generation switches; `n_variants` and `cur` clamp at the ends.
pub fn action(target: Target, delta: i32, axis: Axis, streaming: bool,
              n_variants: usize, cur: usize) -> Action {
    if axis != Axis::Horizontal || delta == 0 { return Action::None; }
    match target {
        Target::Message(m) => {
            if streaming {
                return Action::Refused("this reply is still being written — switching now would send \
the rest of it into a different variant".into());
            }
            if n_variants <= 1 { return Action::None; }
            let next = cur as i32 + delta;
            // Clamp, never wrap. Wrapping from the last variant to the first looks like a new one
            // arrived, and the content changing under a swipe that "did nothing" is worse than a swipe
            // that visibly does nothing.
            if next < 0 || next >= n_variants as i32 { return Action::None; }
            Action::ShowVariant { msg: m, delta }
        }
        Target::Background => Action::SwitchBranch { delta },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mostly_vertical_drag_never_switches_variants() {
        // The failure that would make this feature hated: a chat is a vertical scroller, and every
        // scroll has sideways drift. Without a lock, scrolling flickers through regenerations and the
        // user cannot tell why.
        assert_eq!(lock_axis(6.0, 40.0), Axis::Vertical);
        assert_eq!(action(Target::Message(1), 1, Axis::Vertical, false, 3, 0), Action::None);
    }

    #[test]
    fn an_ambiguous_diagonal_commits_to_NOTHING() {
        // A bare dx > dy comparison treats a 51/49 drag as horizontal. Requiring a clear majority means
        // an uncertain gesture waits rather than guessing, which is what makes the lock feel solid.
        assert_eq!(lock_axis(30.0, 28.0), Axis::Undecided);
        assert_eq!(lock_axis(4.0, 5.0), Axis::Undecided, "and below the threshold, nothing at all");
    }

    #[test]
    fn switching_variants_MID_STREAM_is_refused_with_the_reason() {
        // cur_mut() returns &mut variants[cur]. Changing cur while tokens arrive sends the rest of the
        // reply into a DIFFERENT variant — corrupting two at once, silently.
        let a = action(Target::Message(2), 1, Axis::Horizontal, true, 3, 0);
        match a {
            Action::Refused(why) => assert!(why.contains("into a different variant")),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn the_ends_CLAMP_rather_than_wrap() {
        // Wrapping from the last variant to the first looks like a new one arrived. Content changing
        // under a swipe that should have done nothing is worse than a swipe that visibly does nothing.
        assert_eq!(action(Target::Message(0), -1, Axis::Horizontal, false, 3, 0), Action::None);
        assert_eq!(action(Target::Message(0), 1, Axis::Horizontal, false, 3, 2), Action::None);
        assert!(matches!(action(Target::Message(0), 1, Axis::Horizontal, false, 3, 1),
                         Action::ShowVariant { delta: 1, .. }));
    }

    #[test]
    fn a_single_variant_message_ignores_the_swipe() {
        // Most messages have one variant. A swipe there must not fall through to the branch action, or
        // swiping any ordinary message would move the whole conversation.
        assert_eq!(action(Target::Message(0), 1, Axis::Horizontal, false, 1, 0), Action::None);
    }

    #[test]
    fn a_swipe_on_the_BACKGROUND_switches_branches_instead() {
        // The target decides the meaning, so the same gesture is unambiguous. Resolving by proximity
        // would make it do different things depending on a pixel.
        assert_eq!(action(Target::Background, 1, Axis::Horizontal, false, 0, 0),
                   Action::SwitchBranch { delta: 1 });
    }

    #[test]
    fn a_clear_horizontal_drag_locks_horizontal() {
        assert_eq!(lock_axis(40.0, 6.0), Axis::Horizontal);
    }

    #[test]
    fn a_zero_delta_does_nothing_even_when_locked() {
        // A tap that registers as a gesture with no travel must not commit an action.
        assert_eq!(action(Target::Message(0), 0, Axis::Horizontal, false, 3, 1), Action::None);
    }
}
