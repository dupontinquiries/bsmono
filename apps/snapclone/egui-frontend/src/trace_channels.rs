//! trace_channels — `TraceChannels.cpp` and `UiDebugFlags.cpp`, ported together.
//!
//! Diagnostic channels a user can switch on, and the legacy flag that drives
//! one of them.
//!
//! ## Grouped by what someone is debugging, not by directory
//!
//! The C++ says it: *"Category prefixes are grouped by WHAT SOMEONE IS
//! DEBUGGING, not by source directory: a person chasing a swipe bug wants
//! gesture + swipenav + confirm together and nothing else."*
//!
//! That is the difference between a switch somebody uses and one they scroll
//! past. Grouping by module would make chasing a gesture bug require knowing
//! which files it touches — which is the thing you do not know yet when you are
//! chasing it.
//!
//! ## Why `UiDebugFlags` stopped owning its own rules
//!
//! Its comment records two failures, both worth keeping out of a
//! reimplementation:
//!
//! 1. **`setFilterRules` REPLACES the entire rule set**, so a partial string
//!    silently reset every category it did not know about back to defaults —
//!    *"including any channel enabled from the CLI a moment earlier."* Turning
//!    on one switch turned off another with no indication.
//! 2. **Filter rules still pay to FORMAT each dropped message.** Deciding not
//!    to log after building the string is most of the cost of logging.
//!
//! So the flag maps onto the Layout channel rather than managing categories,
//! and existing settings keep working because Layout owns exactly the four it
//! used to list.
//!
//! ## Disjointness is enforced, not assumed
//!
//! The header notes it is *"enforced by a unit test so a new category can't
//! silently land in two channels with contradictory rules"*. That test ports
//! too — it is the invariant that keeps the grouping meaningful.

/// A diagnostic channel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    Input, Layout, Camera, Media, Net, Dht, Storage, Voice, Ui, Power,
}

impl Channel {
    pub const ALL: [Channel; 10] = [
        Channel::Input, Channel::Layout, Channel::Camera, Channel::Media, Channel::Net,
        Channel::Dht, Channel::Storage, Channel::Voice, Channel::Ui, Channel::Power,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Channel::Input => "input", Channel::Layout => "layout",
            Channel::Camera => "camera", Channel::Media => "media",
            Channel::Net => "net", Channel::Dht => "dht",
            Channel::Storage => "storage", Channel::Voice => "voice",
            Channel::Ui => "ui", Channel::Power => "power",
        }
    }

    /// What a person switching this on is trying to see.
    pub fn description(self) -> &'static str {
        match self {
            Channel::Input => "swipe/wheel/gesture classification and hand-off",
            Channel::Layout => "virtual keyboard, modal stack, dialog geometry",
            Channel::Camera => "capture pipeline, focus/VCM, camera registry",
            Channel::Media => "encode/record/transcode, viewer, thumbnails, export",
            Channel::Net => "sockets, NAT/UPnP, relays, gossip, routing",
            Channel::Dht => "DHT + identity/accounts resolution",
            Channel::Storage => "on-disk stores, inbox/outbox, databus, session",
            Channel::Voice => "voice capture/playback and group-voice coordination",
            Channel::Ui => "ambient visuals, conversation views, icons, wayfinder",
            Channel::Power => "power/sleep-state policy",
        }
    }

    /// Category prefixes this channel owns. Each covers itself and its
    /// `.`-separated children.
    pub fn prefixes(self) -> &'static [&'static str] {
        match self {
            Channel::Input => &["snapclone.ui.swipenav", "snapclone.ui.gesture", "snapclone.ux"],
            Channel::Layout => &["snapclone.ui.vkeyboard", "snapclone.ui.modal",
                                 "snapclone.ui.modaltest", "snapclone.dialog"],
            Channel::Camera => &["snapclone.camera", "snapclone.ui.camera"],
            Channel::Media => &["snapclone.video", "snapclone.viewer",
                                "snapclone.ui.thumbs", "snapclone.export"],
            Channel::Net => &["snapclone.network", "snapclone.nat", "snapclone.upnp",
                              "snapclone.udpchan", "snapclone.websocket", "snapclone.gossip",
                              "snapclone.pollinator", "snapclone.direct", "snapclone.mwrelay",
                              "snapclone.netstats", "snapclone.tiered", "snapclone.relay",
                              "snapclone.router", "snapclone.firewall"],
            Channel::Dht => &["snapclone.dht", "snapclone.identity", "snapclone.accounts"],
            Channel::Storage => &["snapclone.store", "snapclone.inbox", "snapclone.outbox",
                                  "snapclone.databus", "snapclone.session"],
            Channel::Voice => &["snapclone.voice", "snapclone.ui.voice"],
            Channel::Ui => &["snapclone.ui.ambient", "snapclone.ui.conversation",
                             "snapclone.ui.icon", "snapclone.ui.wayfinder"],
            Channel::Power => &["snapclone.power"],
        }
    }

    pub fn from_key(k: &str) -> Option<Channel> {
        Channel::ALL.iter().copied().find(|c| c.key() == k)
    }

    /// Does this channel own `category`?
    ///
    /// A prefix covers itself and its dot-children, so `snapclone.ui.modal`
    /// matches `snapclone.ui.modal.stack` but NOT `snapclone.ui.modality` — a
    /// bare `starts_with` would capture the second and hand one channel a
    /// category another was meant to own.
    pub fn owns(self, category: &str) -> bool {
        self.prefixes().iter().any(|p| {
            category == *p
                || (category.len() > p.len()
                    && category.starts_with(p)
                    && category.as_bytes()[p.len()] == b'.')
        })
    }
}

/// Which channels are live.
#[derive(Clone, Debug, Default)]
pub struct TraceControl {
    enabled: Vec<Channel>,
}

impl TraceControl {
    pub fn is_enabled(&self, c: Channel) -> bool { self.enabled.contains(&c) }

    /// Turn one channel on or off, leaving the others alone.
    ///
    /// The whole point of the rewrite the C++ describes: `setFilterRules`
    /// replaced the entire rule set, so enabling one channel silently disabled
    /// every other — including one enabled from the CLI a moment earlier.
    pub fn set_channel(&mut self, c: Channel, on: bool) {
        self.enabled.retain(|x| *x != c);
        if on { self.enabled.push(c); }
    }

    /// Should this category be logged?
    pub fn should_log(&self, category: &str) -> bool {
        self.enabled.iter().any(|c| c.owns(category))
    }

    pub fn enabled(&self) -> &[Channel] { &self.enabled }

    /// The legacy "mobile display debug" switch.
    ///
    /// Maps onto Layout, which owns exactly the four categories the flag used
    /// to list — so a settings file written by the Qt build keeps working.
    pub fn set_mobile_display_debug(&mut self, on: bool) {
        self.set_channel(Channel::Layout, on);
    }

    pub fn mobile_display_debug(&self) -> bool { self.is_enabled(Channel::Layout) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_category_belongs_to_two_channels() {
        // The invariant the C++ header says is "enforced by a unit test so a
        // new category can't silently land in two channels with contradictory
        // rules". It is what keeps the grouping meaningful.
        for (i, a) in Channel::ALL.iter().enumerate() {
            for b in Channel::ALL.iter().skip(i + 1) {
                for pa in a.prefixes() {
                    assert!(!b.owns(pa),
                            "{} claims {}'s prefix {pa}", b.key(), a.key());
                    for pb in b.prefixes() {
                        assert_ne!(pa, pb, "{} and {} share {pa}", a.key(), b.key());
                    }
                }
            }
        }
    }

    #[test]
    fn a_prefix_covers_its_dot_children_but_not_a_longer_word() {
        // A bare starts_with would capture `snapclone.ui.modality` and hand
        // Layout a category another channel was meant to own.
        assert!(Channel::Layout.owns("snapclone.ui.modal"));
        assert!(Channel::Layout.owns("snapclone.ui.modal.stack"));
        assert!(!Channel::Layout.owns("snapclone.ui.modality"));
        assert!(!Channel::Layout.owns("snapclone.ui"));
    }

    #[test]
    fn enabling_one_channel_does_not_disable_another() {
        // The first failure the C++ records: setFilterRules REPLACED the whole
        // rule set, so this silently reset every category it did not know
        // about — "including any channel enabled from the CLI a moment
        // earlier".
        let mut t = TraceControl::default();
        t.set_channel(Channel::Net, true);
        t.set_channel(Channel::Camera, true);
        assert!(t.is_enabled(Channel::Net), "still on");
        assert!(t.is_enabled(Channel::Camera));
        t.set_channel(Channel::Camera, false);
        assert!(t.is_enabled(Channel::Net), "and still on after a disable");
    }

    #[test]
    fn enabling_a_channel_twice_does_not_duplicate_it() {
        // A settings restore plus a CLI flag can both enable one channel.
        let mut t = TraceControl::default();
        t.set_channel(Channel::Net, true);
        t.set_channel(Channel::Net, true);
        assert_eq!(t.enabled().len(), 1);
    }

    #[test]
    fn a_category_logs_only_when_its_own_channel_is_on() {
        let mut t = TraceControl::default();
        assert!(!t.should_log("snapclone.network"));
        t.set_channel(Channel::Net, true);
        assert!(t.should_log("snapclone.network"));
        assert!(t.should_log("snapclone.nat.upnp"), "a child of a prefix");
        assert!(!t.should_log("snapclone.camera"), "a different channel");
    }

    #[test]
    fn the_legacy_flag_maps_onto_layout_so_old_settings_keep_working() {
        // Layout owns exactly the four categories UiDebugFlags used to list:
        // vkeyboard, modal, modaltest, dialog.
        let mut t = TraceControl::default();
        t.set_mobile_display_debug(true);
        assert!(t.mobile_display_debug());
        for c in ["snapclone.ui.vkeyboard", "snapclone.ui.modal",
                  "snapclone.ui.modaltest", "snapclone.dialog"] {
            assert!(t.should_log(c), "{c} was in the legacy list");
        }
    }

    #[test]
    fn the_legacy_flag_does_not_enable_anything_beyond_layout() {
        // It was a "mobile display" switch, not a firehose.
        let mut t = TraceControl::default();
        t.set_mobile_display_debug(true);
        assert!(!t.should_log("snapclone.network"));
        assert!(!t.should_log("snapclone.camera"));
    }

    #[test]
    fn every_channel_has_a_key_a_description_and_at_least_one_prefix() {
        // A channel with no prefixes is a switch that does nothing, which is
        // worse than a missing switch.
        for c in Channel::ALL {
            assert!(!c.key().is_empty());
            assert!(!c.description().is_empty(), "{}", c.key());
            assert!(!c.prefixes().is_empty(), "{} owns nothing", c.key());
            assert_eq!(Channel::from_key(c.key()), Some(c));
        }
    }

    #[test]
    fn keys_are_unique_so_a_cli_flag_is_unambiguous() {
        let mut keys: Vec<&str> = Channel::ALL.iter().map(|c| c.key()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before);
    }

    #[test]
    fn an_unknown_key_is_none_rather_than_a_default_channel() {
        // A mistyped --trace flag must not silently enable something else.
        assert_eq!(Channel::from_key("netowrk"), None);
        assert_eq!(Channel::from_key(""), None);
    }

    #[test]
    fn nothing_logs_by_default() {
        // Diagnostics off unless asked for: the second failure the C++ records
        // is that filter rules still pay to FORMAT each dropped message.
        let t = TraceControl::default();
        for c in Channel::ALL {
            for p in c.prefixes() { assert!(!t.should_log(p)); }
        }
    }
}
