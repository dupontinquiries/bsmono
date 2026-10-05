//! The ONLINE screen rebuilt on `menu::Menu` — the first live consumer of the
//! shared menu machinery (which was fully built + tested but had no callers).
//! Per `docs/MENU_CLEANUP_VERDICT.md`: six generic rows, no bespoke art, the
//! exact shape `menu.rs` was made for. The old `online_key`'s hand-rolled
//! wrap-nav, per-row text entry, and "row ≥ 2 activates" special case all
//! collapse into the Menu's own ops; `main` only builds the screen and
//! applies the events.
//!
//! The builder is PURE (state in → Menu out) so it's unit-tested without a
//! window; ids are stable so inserting a row can never silently rebind the
//! one below (menu.rs's own contract).

use crate::menu::{Menu, Row};

/// Stable row ids — `main` matches on these.
pub const ID_NAME: &str = "online.name";
pub const ID_FRIEND: &str = "online.friend";
pub const ID_HOST: &str = "online.host";
pub const ID_JOIN: &str = "online.join";
pub const ID_LEAVE: &str = "online.leave";
pub const ID_BACK: &str = "online.back";

/// Max chars for the name field (the old screens' cap).
pub const FIELD_MAX: usize = 16;
/// FRIEND also takes a host's `ip:port` from its HOST line; `255.255.255.255:65535` is 21.
pub const FRIEND_MAX: usize = 24;

/// Build the Online screen from live state. `online` enables LEAVE (leaving
/// while offline was always a no-op toast; now the row is simply disabled,
/// which the shared draw dims for free).
pub fn build(name: &str, friend: &str, online: bool) -> Menu {
    let mut leave = Row::action(ID_LEAVE, "LEAVE").hint("Disconnect and go offline");
    leave.enabled = online;
    Menu::new(
        "ONLINE",
        vec![
            Row::field(ID_NAME, "NAME", name, FIELD_MAX).hint("Type to edit — your callsign"),
            Row::field(ID_FRIEND, "FRIEND", friend, FRIEND_MAX).hint("Type to edit — a name on this machine's store, or a host's ip:port"),
            Row::action(ID_HOST, "HOST").hint("Host a session under your name; the line shows the address a LAN friend types"),
            Row::action(ID_JOIN, "JOIN").hint("Join the friend above (name or ip:port)"),
            leave,
            Row::action(ID_BACK, "BACK").hint("Save and return"),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{Event, Item, Value};

    #[test]
    fn rows_have_stable_ids_and_leave_tracks_online() {
        let m = build("ACE", "ZZ12", false);
        let ids: Vec<&str> = m.rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, [ID_NAME, ID_FRIEND, ID_HOST, ID_JOIN, ID_LEAVE, ID_BACK]);
        assert!(!m.rows[4].enabled, "LEAVE disabled while offline");
        assert!(build("A", "B", true).rows[4].enabled);
        // Fields carry their live values.
        assert!(matches!(&m.rows[0].item, Item::Field { text, .. } if text == "ACE"));
    }

    #[test]
    fn typing_edits_the_selected_field_and_reports_the_change() {
        let mut m = build("", "", false);
        // sel starts on the first selectable row (NAME).
        let ev = m.input_char('A');
        assert!(matches!(ev, Some(Event::Changed(ID_NAME, Value::Text(ref t))) if t == "A"));
        let _ = m.input_char('C');
        assert!(matches!(&m.rows[0].item, Item::Field { text, .. } if text == "AC"));
        let ev = m.backspace();
        assert!(matches!(ev, Some(Event::Changed(ID_NAME, Value::Text(ref t))) if t == "A"));
        // Field length is capped.
        for _ in 0..40 {
            let _ = m.input_char('x');
        }
        if let Item::Field { text, .. } = &m.rows[0].item {
            assert_eq!(text.len(), FIELD_MAX);
        } else {
            panic!("row 0 must be a field");
        }
        // Enter on a field is a no-op (typing happens in place).
        assert!(m.activate().is_none());
    }
}
