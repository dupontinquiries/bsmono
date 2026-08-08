//! requests — inbound connection requests, over the mailbox's own model.
//!
//! `snapclone-mailbox` is 1,304 lines with no consumer in the port, and
//! `requests.rs` inside it is the approval lifecycle: a stranger asks to
//! connect, and you accept, reject, or block. Nothing in the egui frontend
//! surfaced it, so a request had nowhere to arrive.
//!
//! The engine is a **dependency**, not a copy. It ships an rlib, and the
//! composer taught this port the difference: `#[path]` is for modules inside a
//! crate with no library target, and pulling a lib.rs in as a submodule breaks
//! any `crate::` path inside it.
//!
//! ## Reject and Block are not two words for the same button
//!
//! The engine is explicit: a rejected sender *"MAY send a fresh request later
//! (which re-enters as Pending)"*, while a blocked one is *"dropped before they
//! ever reach Pending"*. That difference is the entire safety story of this
//! screen, and a UI that presented them as a single "dismiss" would quietly
//! remove the only protection against someone re-asking forever.
//!
//! So they are separate actions with separate weights: reject is the easy one,
//! block is the deliberate one, and the view says what each will do rather than
//! assuming the user has read the manual.
//!
//! ## Nothing is destructive-by-default
//!
//! There is no swipe-to-block and no pre-selected action. Blocking is
//! effectively permanent from the sender's point of view — they cannot tell they
//! were blocked and cannot ask again — so it must be chosen rather than
//! arrived at.
//!
//! ## A no-op and a refusal are different, and the user hears one of them
//!
//! `ApplyOutcome` distinguishes "nothing happened" from "that is illegal for
//! this state". Accepting an already-accepted request is a double tap and
//! deserves silence; accepting a *blocked* sender is a contradiction and
//! deserves a message. Collapsing them would either nag on every double tap or
//! silently ignore a request the user genuinely meant.

pub use snapclone_mailbox::requests::{
    ApplyOutcome, InboundRequest, RequestAction, RequestMailbox, RequestState,
};

/// What the row's buttons offer.
///
/// Only pending requests are actionable. A terminal row stays visible for a
/// moment so the user sees their choice land, but offering Accept on something
/// already accepted invites a tap that does nothing.
pub fn actions_for(state: RequestState) -> &'static [RequestAction] {
    match state {
        RequestState::Pending => &[RequestAction::Accept, RequestAction::Reject,
                                   RequestAction::Block],
        _ => &[],
    }
}

/// What a button says.
pub fn action_label(a: RequestAction) -> &'static str {
    match a {
        RequestAction::Accept => "Accept",
        RequestAction::Reject => "Reject",
        RequestAction::Block => "Block",
    }
}

/// What the action will actually do, in words.
///
/// Shown because the difference between reject and block is invisible from the
/// button text and permanent in effect.
pub fn action_hint(a: RequestAction) -> &'static str {
    match a {
        RequestAction::Accept => "They become a contact",
        RequestAction::Reject => "Dismissed — they can ask again",
        RequestAction::Block => "Dismissed — they cannot ask again",
    }
}

/// Is this action irreversible from the sender's side?
///
/// Only Block. The caller confirms these and nothing else: a confirmation on
/// every action trains people to dismiss confirmations, which is how the one
/// that mattered gets dismissed too.
pub fn needs_confirmation(a: RequestAction) -> bool { a == RequestAction::Block }

/// How a request reads once decided.
pub fn state_label(s: RequestState) -> &'static str {
    match s {
        RequestState::Pending => "Wants to connect",
        RequestState::Accepted => "Accepted",
        RequestState::Rejected => "Rejected",
        RequestState::Blocked => "Blocked",
    }
}

/// What to show for a sender with no display name.
///
/// The raw id, never a blank or a friendly placeholder. A stranger's request is
/// exactly the case where inventing a name would be dangerous — "Unknown" for
/// two different senders makes them indistinguishable at the moment the user is
/// deciding whether to trust one.
pub fn sender_label(r: &InboundRequest) -> &str {
    let n = r.display_name.trim();
    if n.is_empty() { &r.sender_id } else { n }
}

/// Pending requests, newest first.
///
/// Delegates to `RequestMailbox::pending`, which already filters and sorts. I
/// wrote the filter and the sort again before checking — the sixth time this
/// port has produced a second copy of something that existed — and the engine's
/// version is also the one the network layer agrees with, so re-deriving the
/// order here would eventually disagree with the badge count beside it.
///
/// Kept as a named function anyway so the view has one place to change if the
/// ordering ever needs to differ from the engine's, and so the tests below pin
/// the behaviour the view depends on.
pub fn pending(mailbox: &RequestMailbox) -> Vec<&InboundRequest> {
    mailbox.pending()
}

/// Draw the list. Returns the action the user chose.
pub fn requests_ui(ui: &mut egui::Ui, mailbox: &RequestMailbox,
                   c: &uikit_egui::PaletteColors)
    -> Option<(String, RequestAction)> {
    let mut chosen = None;
    let rows = pending(mailbox);
    if rows.is_empty() {
        ui.colored_label(c.text_weak, "No connection requests.");
        return None;
    }
    for r in rows {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(sender_label(r)).strong());
            ui.colored_label(c.text_weak,
                egui::RichText::new(state_label(r.state)).small());
            ui.horizontal(|ui| {
                for a in actions_for(r.state) {
                    // Block is not styled as the primary action. The safe
                    // choice is the easy one to hit.
                    let btn = egui::Button::new(action_label(*a));
                    if ui.add(btn).on_hover_text(action_hint(*a)).clicked() {
                        chosen = Some((r.sender_id.clone(), *a));
                    }
                }
            });
        });
        ui.separator();
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mailbox_with(ids: &[(&str, i64)]) -> RequestMailbox {
        let mut m = RequestMailbox::new();
        for (id, t) in ids { m.receive(id, "", *t); }
        m
    }

    #[test]
    fn the_engine_is_a_dependency_rather_than_a_reimplementation() {
        // 1,304 lines with no consumer. A second copy of the approval
        // lifecycle would drift from the one the network layer actually
        // enforces.
        let m = mailbox_with(&[("sc-a", 100)]);
        assert_eq!(pending(&m).len(), 1);
    }

    #[test]
    fn only_pending_requests_offer_buttons() {
        // Offering Accept on something already accepted invites a tap that
        // does nothing.
        assert_eq!(actions_for(RequestState::Pending).len(), 3);
        assert!(actions_for(RequestState::Accepted).is_empty());
        assert!(actions_for(RequestState::Rejected).is_empty());
        assert!(actions_for(RequestState::Blocked).is_empty());
    }

    #[test]
    fn reject_and_block_say_what_they_actually_do() {
        // The difference is invisible from the button text and permanent in
        // effect: a rejected sender may ask again, a blocked one cannot.
        assert!(action_hint(RequestAction::Reject).contains("can ask again"));
        assert!(action_hint(RequestAction::Block).contains("cannot ask again"));
        assert_ne!(action_hint(RequestAction::Reject), action_hint(RequestAction::Block));
    }

    #[test]
    fn only_blocking_is_confirmed() {
        // A confirmation on every action trains people to dismiss
        // confirmations, which is how the one that mattered gets dismissed too.
        assert!(needs_confirmation(RequestAction::Block));
        assert!(!needs_confirmation(RequestAction::Accept));
        assert!(!needs_confirmation(RequestAction::Reject));
    }

    #[test]
    fn a_rejected_sender_can_come_back_and_a_blocked_one_cannot() {
        // The engine's rule, pinned from the view's side because it is the
        // entire safety story of this screen.
        let mut m = mailbox_with(&[("sc-a", 100), ("sc-b", 200)]);
        m.apply("sc-a", RequestAction::Reject);
        m.apply("sc-b", RequestAction::Block);

        m.receive("sc-a", "", 300);
        m.receive("sc-b", "", 300);

        let ids: Vec<&str> = pending(&m).iter().map(|r| r.sender_id.as_str()).collect();
        assert!(ids.contains(&"sc-a"), "a rejection is not permanent");
        assert!(!ids.contains(&"sc-b"), "a block is");
    }

    #[test]
    fn a_stranger_shows_their_id_rather_than_an_invented_name() {
        // "Unknown" for two different senders makes them indistinguishable at
        // exactly the moment the user is deciding whether to trust one.
        let r = InboundRequest { sender_id: "sc-c1dd601b".into(),
            display_name: "  ".into(), received_ms: 0, state: RequestState::Pending };
        assert_eq!(sender_label(&r), "sc-c1dd601b");
    }

    #[test]
    fn a_named_sender_shows_their_name() {
        let r = InboundRequest { sender_id: "sc-x".into(), display_name: "Ada".into(),
            received_ms: 0, state: RequestState::Pending };
        assert_eq!(sender_label(&r), "Ada");
    }

    #[test]
    fn pending_requests_are_newest_first() {
        // A request ignored for a week is not the one the user opened the
        // screen for.
        let m = mailbox_with(&[("sc-old", 100), ("sc-new", 900), ("sc-mid", 500)]);
        let ids: Vec<&str> = pending(&m).iter().map(|r| r.sender_id.as_str()).collect();
        assert_eq!(ids, vec!["sc-new", "sc-mid", "sc-old"]);
    }

    #[test]
    fn simultaneous_arrivals_order_deterministically() {
        // Two requests in the same millisecond happen on a reconnect, and a
        // list that reshuffles between frames is one you mis-tap.
        let m = mailbox_with(&[("sc-b", 100), ("sc-a", 100)]);
        let first: Vec<&str> = pending(&m).iter().map(|r| r.sender_id.as_str()).collect();
        assert_eq!(first[0], "sc-a");
        assert_eq!(pending(&m)[0].sender_id, "sc-a");
    }

    #[test]
    fn a_decided_request_leaves_the_pending_list() {
        let mut m = mailbox_with(&[("sc-a", 100)]);
        m.apply("sc-a", RequestAction::Accept);
        assert!(pending(&m).is_empty());
    }

    #[test]
    fn every_state_reads_as_something() {
        // A blank row is indistinguishable from a rendering fault on a screen
        // about trust.
        for s in [RequestState::Pending, RequestState::Accepted,
                  RequestState::Rejected, RequestState::Blocked] {
            assert!(!state_label(s).is_empty());
        }
    }

    #[test]
    fn an_empty_mailbox_says_so() {
        let m = RequestMailbox::new();
        assert!(pending(&m).is_empty());
    }
}
