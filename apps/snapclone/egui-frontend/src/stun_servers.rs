//! stun_servers — the seven servers the Network tab offers, and the bitmask
//! they persist as.
//!
//! Ported from `SettingsPage.cpp`'s `buildStunSection` plus the
//! `SettingsKey::kStunServersEnabled` / `kStunDefaultMask` contract in
//! `include/ui/SettingsPage.hpp`. The port had a single `stun_host: String`,
//! which is not the same feature: Qt tries EVERY checked server in order and
//! the first responder wins, so one host is not a smaller version of the list,
//! it is a different reachability story on a network where one server is down.
//!
//! ## The mask is the contract, not the order of this table
//!
//! Qt stores an int and assigns bit N to the Nth server by hand. That means
//! **reordering this table silently reassigns everybody's saved choices** — a
//! user who unchecked Google finds Framasoft off instead. So the bit index is
//! stored ON the entry rather than derived from its position, and a test pins
//! the seven pairs. Changing the order of `ALL` is then a cosmetic change to
//! the list's on-screen order and nothing else.
//!
//! ## Why the default excludes two of them
//!
//! Qt's default is `0b0011111` — the five FOSS servers on, Google and
//! Cloudflare off. That is a policy decision, not an oversight, and it is worth
//! keeping visible: this app talks to whatever is checked here at startup, so
//! the default decides who learns your IP address the first time you run it.
//! `is_foss` carries the reason so the UI can show it rather than making the
//! user infer it from the hostname.

/// One STUN server as the Network tab lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StunServer {
    /// The bit this server occupies in the persisted mask. NOT the index into
    /// `ALL` — see the module header for why those must be allowed to differ.
    pub bit: u8,
    pub host: &'static str,
    pub port: u16,
    /// The operator + licence line Qt shows beside the checkbox.
    pub detail: &'static str,
    pub is_foss: bool,
}

impl StunServer {
    /// `host:port`, the form a STUN client actually wants.
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// The seven, in the order Qt lists them.
///
/// jami.net also answers on port 80, which Qt's detail line mentions; the
/// port here is the one a client dials first, and the 80 fallback belongs to
/// whatever does the dialling rather than to a settings row.
pub const ALL: [StunServer; 7] = [
    StunServer { bit: 0, host: "stun.jami.net", port: 3478,
                 detail: "Savoir-faire Linux · FOSS · port 3478 + 80", is_foss: true },
    StunServer { bit: 1, host: "stun.nextcloud.com", port: 3478,
                 detail: "Nextcloud · FOSS · port 3478", is_foss: true },
    StunServer { bit: 2, host: "stun.linphone.org", port: 3478,
                 detail: "Belledonne Communications · FOSS VoIP · port 3478", is_foss: true },
    StunServer { bit: 3, host: "stun.ekiga.net", port: 3478,
                 detail: "GNOME Project · FOSS · port 3478", is_foss: true },
    StunServer { bit: 4, host: "stun.framasoft.org", port: 3478,
                 detail: "Framasoft · French FOSS · port 3478", is_foss: true },
    StunServer { bit: 5, host: "stun.l.google.com", port: 19302,
                 detail: "Google · proprietary · port 19302", is_foss: false },
    StunServer { bit: 6, host: "stun.cloudflare.com", port: 3478,
                 detail: "Cloudflare · proprietary · port 3478", is_foss: false },
];

/// Qt's `kStunDefaultMask`: the five FOSS servers on, the two proprietary ones
/// off. Written as the literal rather than computed from `is_foss` so that a
/// later edit to a server's `is_foss` flag cannot silently change what a fresh
/// install talks to — the test below asserts the two agree today, which is the
/// place to notice if they ever stop.
pub const DEFAULT_MASK: u32 = 0b001_1111;

/// Every bit the table actually uses. A saved mask with bits outside this is
/// from a build that knew more servers than this one; those bits are kept on
/// round-trip rather than dropped, so downgrading and upgrading again does not
/// silently uncheck a server the older build could not draw.
pub const KNOWN_BITS: u32 = 0b111_1111;

/// Is this server enabled in `mask`?
pub fn enabled(mask: u32, s: &StunServer) -> bool {
    mask & (1 << s.bit) != 0
}

/// Set or clear one server's bit, leaving every other bit — including unknown
/// ones from a newer build — exactly as it was.
pub fn set(mask: u32, s: &StunServer, on: bool) -> u32 {
    if on { mask | (1 << s.bit) } else { mask & !(1 << s.bit) }
}

/// The addresses to try, in table order. Qt's rule: all checked servers are
/// tried in order and the first responder wins.
pub fn addresses(mask: u32) -> Vec<String> {
    ALL.iter().filter(|s| enabled(mask, s)).map(|s| s.address()).collect()
}

/// Is anything checked at all?
///
/// Its own function because the UI has to say so: with no server checked there
/// is no public-address discovery and therefore no cross-network connection,
/// and Qt warns about exactly this. A silently empty list looks like the app
/// deciding not to work.
pub fn none_enabled(mask: u32) -> bool {
    mask & KNOWN_BITS == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bit_assignment_matches_qt_server_for_server() {
        // The whole point of storing `bit` on the entry. If this list is ever
        // reordered for presentation, THIS is what must not change — a shifted
        // bit reassigns every existing user's saved choices to the wrong
        // server, and nothing else in the app would notice.
        let want = [
            (0u8, "stun.jami.net"),
            (1, "stun.nextcloud.com"),
            (2, "stun.linphone.org"),
            (3, "stun.ekiga.net"),
            (4, "stun.framasoft.org"),
            (5, "stun.l.google.com"),
            (6, "stun.cloudflare.com"),
        ];
        assert_eq!(ALL.len(), want.len());
        for (bit, host) in want {
            let s = ALL.iter().find(|s| s.host == host)
                .unwrap_or_else(|| panic!("{host} is gone from the table"));
            assert_eq!(s.bit, bit, "{host} moved bits");
        }
    }

    #[test]
    fn the_default_is_every_foss_server_and_neither_proprietary_one() {
        // DEFAULT_MASK is a literal ported from Qt and `is_foss` is a separate
        // hand-written flag; this is the one place that notices if they drift,
        // which matters because the default decides who learns your IP the
        // first time the app runs.
        for s in &ALL {
            assert_eq!(enabled(DEFAULT_MASK, s), s.is_foss,
                       "{} is {} by default but is_foss={}",
                       s.host, if enabled(DEFAULT_MASK, s) { "on" } else { "off" }, s.is_foss);
        }
        assert_eq!(DEFAULT_MASK, 0b001_1111, "the value Qt persists");
        assert_eq!(addresses(DEFAULT_MASK).len(), 5);
    }

    #[test]
    fn setting_one_server_leaves_the_others_alone() {
        let m = set(DEFAULT_MASK, &ALL[5], true); // Google on
        assert!(enabled(m, &ALL[5]));
        for s in ALL.iter().filter(|s| s.bit != 5) {
            assert_eq!(enabled(m, s), enabled(DEFAULT_MASK, s), "{} changed", s.host);
        }
        let back = set(m, &ALL[5], false);
        assert_eq!(back, DEFAULT_MASK, "clearing the bit returns the original mask");
    }

    #[test]
    fn a_bit_this_build_does_not_know_survives_being_edited_around() {
        // A newer build adds an eighth server; this one loads the mask, the
        // user unchecks Ekiga, and saves. Dropping the unknown bit would
        // silently turn off a server the user chose in the newer build.
        let from_newer = DEFAULT_MASK | (1 << 9);
        let edited = set(from_newer, &ALL[3], false);
        assert!(edited & (1 << 9) != 0, "the unknown bit was dropped");
        assert!(!enabled(edited, &ALL[3]));
    }

    #[test]
    fn no_servers_checked_is_reported_rather_than_looking_like_a_working_empty_list() {
        assert!(none_enabled(0));
        assert!(addresses(0).is_empty());
        assert!(!none_enabled(DEFAULT_MASK));
        // An unknown bit alone is not a server THIS build can dial, so it does
        // not count as "something is checked" — the warning is about what will
        // actually be tried.
        assert!(none_enabled(1 << 9));
    }

    #[test]
    fn an_address_is_host_colon_port_including_googles_odd_one() {
        // Google is the only one not on 3478, and a hardcoded :3478 in a
        // formatter would be invisible until a cross-network call failed.
        assert_eq!(ALL[5].address(), "stun.l.google.com:19302");
        assert_eq!(ALL[0].address(), "stun.jami.net:3478");
    }

    #[test]
    fn the_try_order_is_the_table_order() {
        // Qt: "all checked servers are tried in order — first responder wins."
        let m = set(DEFAULT_MASK, &ALL[6], true);
        let a = addresses(m);
        assert_eq!(a.first().map(String::as_str), Some("stun.jami.net:3478"));
        assert_eq!(a.last().map(String::as_str), Some("stun.cloudflare.com:3478"));
    }
}
