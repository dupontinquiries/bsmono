//! wizard — the onboarding flow's pure phase machine, in the same shape as
//! `uikit-egui::setup_wizard` (a pure `next_phase(current, event)` transition function, unit-tested
//! on its own, with the egui rendering as a separate concern in `ui.rs`): **Welcome** → **Name**
//! (pick/confirm a display name) → **ShareId** (your id + QR, for a friend to add you) →
//! **AddFriend** (paste a friend's id) → **Done** (quick-match hooks into each game). Opens straight
//! to `Done` on every run after the first (see `identity::LocalIdentity::is_first_run`); the
//! "Add Friend" step is also reachable directly from the main friends panel without the rest of the
//! wizard, driven by the same transition function.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WizardPhase {
    Welcome,
    Name,
    ShareId,
    AddFriend,
    Done,
}

/// Where the wizard opens: straight to `Done` for a returning player, else `Welcome`. Pure.
pub fn initial_phase(is_first_run: bool) -> WizardPhase {
    if is_first_run {
        WizardPhase::Welcome
    } else {
        WizardPhase::Done
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WizardEvent {
    Next,
    Back,
    /// A friend id was successfully added (validated + appended to `me.friends`).
    FriendAdded,
    /// Skip straight to Done from anywhere (e.g. "I'll add friends later").
    Skip,
}

/// The pure phase transition. Returns the next phase for `event` from `current`. Unknown/no-op
/// combinations keep the current phase (mirrors `uikit-egui::setup_wizard::next_phase`).
pub fn next_phase(current: WizardPhase, event: &WizardEvent) -> WizardPhase {
    use WizardEvent::*;
    use WizardPhase::*;
    match (current, event) {
        (_, Skip) => Done,
        (Welcome, Next) => Name,
        (Name, Next) => ShareId,
        (Name, Back) => Welcome,
        (ShareId, Next) => AddFriend,
        (ShareId, Back) => Name,
        (AddFriend, Next) => Done,
        (AddFriend, FriendAdded) => Done,
        (AddFriend, Back) => ShareId,
        (Done, Back) => AddFriend,
        _ => current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use WizardEvent::*;
    use WizardPhase::*;

    #[test]
    fn first_run_opens_at_welcome_returning_player_opens_at_done() {
        assert_eq!(initial_phase(true), Welcome);
        assert_eq!(initial_phase(false), Done);
    }

    #[test]
    fn happy_path_walks_every_phase_in_order() {
        let mut p = Welcome;
        for (event, want) in [(Next, Name), (Next, ShareId), (Next, AddFriend), (Next, Done)] {
            p = next_phase(p, &event);
            assert_eq!(p, want);
        }
    }

    #[test]
    fn back_reverses_each_forward_step() {
        let mut p = AddFriend;
        p = next_phase(p, &Back);
        assert_eq!(p, ShareId);
        p = next_phase(p, &Back);
        assert_eq!(p, Name);
        p = next_phase(p, &Back);
        assert_eq!(p, Welcome);
    }

    #[test]
    fn skip_jumps_straight_to_done_from_any_phase() {
        for p in [Welcome, Name, ShareId, AddFriend, Done] {
            assert_eq!(next_phase(p, &Skip), Done);
        }
    }

    #[test]
    fn friend_added_also_advances_to_done() {
        assert_eq!(next_phase(AddFriend, &FriendAdded), Done);
    }

    #[test]
    fn unknown_combinations_are_a_no_op() {
        assert_eq!(next_phase(Welcome, &Back), Welcome);
        assert_eq!(next_phase(Done, &Next), Done);
        assert_eq!(next_phase(Welcome, &FriendAdded), Welcome);
    }
}
