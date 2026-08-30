//! relay_preset — the Network tab's relay dropdown, and the two rules that
//! turn it into the one string that gets persisted.
//!
//! Ported from `SettingsPage.cpp`: the `kRelays` table in `buildStunSection`,
//! the load-time matching in `loadSettings`, and the effective-URL computation
//! in `saveSettings`. One persisted key, `network/relayUrl`, holds the RESULT —
//! the dropdown index is never stored, it is re-derived from the URL every
//! load. That is the right call (a stored index rots the moment the table
//! changes) and it is also why the derivation needs to be a tested function
//! rather than inline UI code: it is the only thing standing between a saved
//! relay and a dropdown that shows the wrong entry.
//!
//! ## Four load branches, one of which surprises people
//!
//! 1. the saved URL matches a preset  → that preset
//! 2. nothing saved                   → **Magic Wormhole**, not None
//! 3. a saved URL matching no preset  → Custom, with the URL in the text field
//! 4. (2) followed by a Save          → the wormhole URL is now persisted
//!
//! Branch 2 is Qt's behaviour and this port keeps it, but it deserves saying
//! out loud: an empty config selects a relay the user never chose, and the
//! next Save writes it to disk. The empty string and "None (direct / UDP
//! only)" are the same stored value, so a user who deliberately picks None and
//! saves is indistinguishable on the next load from a user who has never
//! opened the page — and both get Wormhole. Deviating here would be a
//! behaviour change rather than a port, so it is recorded instead of fixed;
//! `nothing_saved_selects_wormhole_not_none` is where to start if that call
//! gets revisited.

/// The sentinel Qt stores as a preset's item data to mean "show the text
/// field". It is never written to `network/relayUrl`.
pub const CUSTOM: &str = "__custom__";

/// One dropdown entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Relay {
    pub label: &'static str,
    /// The URL this preset stands for, [`CUSTOM`] for the custom entry, or the
    /// empty string for None.
    pub url: &'static str,
    pub note: &'static str,
}

pub const ALL: [Relay; 3] = [
    Relay { label: "None (direct / UDP only)", url: "", note: "" },
    Relay { label: "Magic Wormhole transit  ·  tcp://transit.magic-wormhole.io:4001",
            url: "tcp://transit.magic-wormhole.io:4001",
            note: "FOSS, run by the wormhole.io project. Raw TCP relay, no WebSocket needed." },
    Relay { label: "Custom WebSocket relay (ws:// or wss://)", url: CUSTOM,
            note: "Run relay-server/relay.js on any VPS." },
];

/// Which entry is the custom one. Derived rather than hardcoded to 2, because
/// the index is used by the UI and a table edit should not need two changes.
pub fn custom_index() -> usize {
    ALL.iter().position(|r| r.url == CUSTOM).unwrap_or(ALL.len() - 1)
}

fn wormhole_index() -> Option<usize> {
    ALL.iter().position(|r| r.url.contains("transit.magic-wormhole.io"))
}

/// What the dropdown and the custom field should show for a saved URL.
///
/// Returns `(index, custom_text)`. `custom_text` is empty unless branch 3
/// applies, so a caller can assign it unconditionally without wiping a field
/// the user is mid-edit on only if it also checks the index — which is why the
/// two travel together rather than being two calls.
pub fn resolve(saved: &str) -> (usize, String) {
    // Branch 1: an exact preset match. The empty-url None entry is skipped
    // deliberately — an empty `saved` must reach branch 2, and without this
    // guard it would match None here and never get there.
    if !saved.is_empty() {
        if let Some(i) = ALL.iter().position(|r| r.url != CUSTOM && !r.url.is_empty() && r.url == saved) {
            return (i, String::new());
        }
        // Branch 3.
        return (custom_index(), saved.to_string());
    }
    // Branch 2.
    (wormhole_index().unwrap_or(0), String::new())
}

/// The single string to persist, given what the dropdown and the text field
/// currently hold.
///
/// The custom text is trimmed, and a whitespace-only custom entry becomes the
/// empty string rather than a URL made of spaces — which would be stored,
/// reloaded, fail to match any preset, and come back as a custom entry
/// containing nothing, i.e. a relay setting that looks configured and is not.
pub fn effective_url(index: usize, custom_text: &str) -> String {
    match ALL.get(index) {
        Some(r) if r.url == CUSTOM => custom_text.trim().to_string(),
        Some(r) => r.url.to_string(),
        // An index from a config written by a build with more presets. None is
        // the safe reading: it disables relaying rather than routing traffic
        // through whatever happens to sit at that index here.
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_preset_url_comes_back_as_that_preset() {
        let (i, custom) = resolve("tcp://transit.magic-wormhole.io:4001");
        assert_eq!(ALL[i].url, "tcp://transit.magic-wormhole.io:4001");
        assert!(custom.is_empty(), "the custom field is not populated for a preset");
    }

    #[test]
    fn nothing_saved_selects_wormhole_not_none() {
        // Qt's branch 2, ported deliberately. If this ever changes, it is a
        // behaviour decision and this test is the place to argue it: an empty
        // config currently selects a relay the user never chose, and the next
        // Save writes it to disk.
        let (i, _) = resolve("");
        assert!(ALL[i].url.contains("magic-wormhole"), "empty config picked {:?}", ALL[i].label);
        assert_ne!(i, 0, "specifically NOT None, however reasonable that would be");
    }

    #[test]
    fn an_unrecognised_url_becomes_a_custom_entry_carrying_it() {
        let (i, custom) = resolve("wss://relay.example.org:8443");
        assert_eq!(i, custom_index());
        assert_eq!(custom, "wss://relay.example.org:8443",
                   "the URL must reach the text field or the user's relay silently vanishes");
    }

    #[test]
    fn the_custom_sentinel_is_never_what_gets_stored() {
        // Storing "__custom__" as the relay URL would have the app try to dial
        // it. The failure would be at connect time, far from this page.
        let stored = effective_url(custom_index(), "wss://r.example.org");
        assert_eq!(stored, "wss://r.example.org");
        assert_ne!(stored, CUSTOM);
        for i in 0..ALL.len() {
            assert_ne!(effective_url(i, ""), CUSTOM);
        }
    }

    #[test]
    fn a_custom_entry_that_is_only_whitespace_stores_as_empty() {
        // Otherwise it round-trips into a custom entry containing nothing: a
        // relay setting that looks configured and is not.
        assert_eq!(effective_url(custom_index(), "   "), "");
        assert_eq!(effective_url(custom_index(), "  wss://x  "), "wss://x");
    }

    #[test]
    fn a_preset_round_trips_through_store_and_load() {
        // The property that matters end to end: what Save writes, Load must
        // put the dropdown back on.
        for i in 0..ALL.len() {
            if ALL[i].url == CUSTOM { continue; }
            let stored = effective_url(i, "");
            let (back, _) = resolve(&stored);
            if ALL[i].url.is_empty() {
                // None stores as "", which loads as wormhole — the documented
                // branch-2 collision. Asserting the collision rather than
                // pretending it round-trips, so the day it is fixed this test
                // fails and says why.
                assert!(ALL[back].url.contains("magic-wormhole"),
                        "None still collides with never-configured");
            } else {
                assert_eq!(back, i, "{} did not round-trip", ALL[i].label);
            }
        }
    }

    #[test]
    fn an_index_from_a_newer_build_disables_relaying_rather_than_guessing() {
        assert_eq!(effective_url(99, "wss://x"), "");
    }
}
