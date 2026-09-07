//! advert — what an accessory says while you are not talking to it.
//!
//! ## The gap this fills
//!
//! switcheroo reads a plug's state by connecting: pair-verify, then a characteristic read.
//! That is the only way it learns anything, so a plug switched at the wall, or by the Home
//! app, or by a schedule, shows the old state until someone presses ↻. `classify` looks at
//! service UUIDs and nothing looks at the rest of the advertisement.
//!
//! But a HAP accessory broadcasts a **Global State Number** in every advert, and increments
//! it whenever a broadcast-enabled characteristic changes. That is the accessory telling
//! everyone in radio range "something about me is different now" — for free, passively, with
//! no pairing, no connection and no key.
//!
//! It does not say WHAT changed. It does not have to: knowing that a plug changed is what
//! turns a poll into a read. A row can go stale the moment the GSN moves and be refreshed
//! once, instead of every row being re-read on a timer or never.
//!
//! ## Layout (spec §7.4.2.2)
//!
//! Inside the manufacturer-specific AD structure:
//!
//! ```text
//!   0..2   company identifier, 0x004C little-endian (Apple)
//!   2      type, 0x06 = HomeKit
//!   3      subtype (high 3 bits) and length (low 5 bits); 0x31 = subtype 1, length 17
//!   4      status flags; bit 0 SET means NOT paired
//!   5..11  device id, 6 bytes, big-endian as printed (AA:BB:CC:DD:EE:FF)
//!   11..13 accessory category, little-endian
//!   13..15 global state number, little-endian
//!   15     configuration number
//!   16     compatible version
//! ```
//!
//! The device id here is the accessory's HAP identifier, which is **not** necessarily its
//! BLE MAC — a fact worth keeping, because matching an advert to a stored pairing by MAC
//! fails on any accessory that randomises its address.
//!
//! ## What this does not do
//!
//! Type `0x11` is the **encrypted notification** advert: the accessory puts the changed
//! characteristic's value directly into the broadcast, encrypted with a Broadcast
//! Encryption Key derived during pairing. Reading it needs that key and its refresh
//! protocol. [`parse`] recognises the form and says so rather than skipping it, because
//! "there is an event here I cannot read" is different information from "no event", and a
//! caller that conflated them would quietly stop noticing changes.

/// Apple's Bluetooth company identifier.
pub const APPLE_COMPANY_ID: u16 = 0x004C;
/// Manufacturer-data type for a HAP regulatory advertisement.
pub const TYPE_HAP: u8 = 0x06;
/// Manufacturer-data type for an encrypted HAP notification.
pub const TYPE_HAP_ENCRYPTED_NOTIFICATION: u8 = 0x11;

/// A HAP advertisement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advert {
    /// `AA:BB:CC:DD:EE:FF`, the accessory's HAP identifier — not necessarily its BLE MAC.
    pub device_id: String,
    /// 7 = outlet, 8 = switch, 5 = lightbulb… (spec table 12-3).
    pub category: u16,
    /// Bumped by the accessory whenever a broadcast-enabled characteristic changes.
    pub gsn: u16,
    /// Bumped when the accessory's attribute database changes — a firmware update, a
    /// factory reset. A controller must re-discover when this moves.
    pub config_number: u8,
    pub compatible_version: u8,
    /// The accessory has no pairing and is ready for pair-setup.
    pub unpaired: bool,
}

/// What was found in a manufacturer-data blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// A regulatory advert, fully readable.
    Regulatory(Advert),
    /// An encrypted notification: an event is being broadcast and reading it needs the
    /// Broadcast Encryption Key. Reported rather than skipped — see the module docs.
    EncryptedNotification { device_id: String },
    /// Not HAP. Someone else's manufacturer data, or Apple's for something else.
    NotHap,
}

fn device_id(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(":")
}

/// Parse manufacturer-specific data as it arrives from a BLE scan.
///
/// `data` is the AD structure's payload INCLUDING the two company-id bytes, which is what
/// btleplug's `manufacturer_data` map is missing (it keys BY company id) — [`parse_keyed`]
/// is the shape for that.
pub fn parse(data: &[u8]) -> Parsed {
    if data.len() < 3 {
        return Parsed::NotHap;
    }
    let company = u16::from_le_bytes([data[0], data[1]]);
    if company != APPLE_COMPANY_ID {
        return Parsed::NotHap;
    }
    parse_keyed(company, &data[2..])
}

/// Parse when the company id has already been split out, as btleplug hands it over.
pub fn parse_keyed(company: u16, body: &[u8]) -> Parsed {
    if company != APPLE_COMPANY_ID || body.is_empty() {
        return Parsed::NotHap;
    }
    match body[0] {
        TYPE_HAP => {
            // body: [type][stl][flags][id×6][cat×2][gsn×2][cfg][ver]  = 15 bytes
            if body.len() < 15 {
                return Parsed::NotHap;
            }
            // The low 5 bits of the subtype/length byte are the payload length. It is not
            // trusted for slicing — the slice above is what bounds this — but a value that
            // disagrees with reality means the advert is not what it claims, and treating
            // it as HAP anyway is how a malformed packet becomes a wrong reading.
            let declared = (body[1] & 0x1F) as usize;
            if declared != 17 {
                return Parsed::NotHap;
            }
            Parsed::Regulatory(Advert {
                unpaired: body[2] & 0x01 != 0,
                device_id: device_id(&body[3..9]),
                category: u16::from_le_bytes([body[9], body[10]]),
                gsn: u16::from_le_bytes([body[11], body[12]]),
                config_number: body[13],
                compatible_version: body[14],
            })
        }
        TYPE_HAP_ENCRYPTED_NOTIFICATION => {
            if body.len() < 9 {
                return Parsed::NotHap;
            }
            Parsed::EncryptedNotification { device_id: device_id(&body[3..9]) }
        }
        _ => Parsed::NotHap,
    }
}

/// What changed since the last advert from this accessory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// First time seen. Not a change — a controller that treats it as one refreshes every
    /// accessory on every scan.
    First,
    /// Nothing moved.
    Same,
    /// A broadcast-enabled characteristic changed. Re-read this accessory.
    StateChanged,
    /// The attribute database changed — firmware update, factory reset. Re-discover, and
    /// note that a factory reset means the pairing is gone.
    Reconfigured,
}

/// Tracks GSN per accessory across scans.
///
/// Keyed by the advert's HAP device id rather than the BLE address, because an accessory
/// that randomises its address is still the same accessory and would otherwise look like a
/// new one on every rotation — which reads as `First` forever and never as a change.
#[derive(Debug, Default)]
pub struct GsnTracker {
    seen: std::collections::HashMap<String, (u16, u8)>,
}

impl GsnTracker {
    pub fn new() -> GsnTracker {
        GsnTracker::default()
    }

    /// Feed an advert; get what it means.
    pub fn observe(&mut self, a: &Advert) -> Change {
        let key = a.device_id.clone();
        match self.seen.insert(key, (a.gsn, a.config_number)) {
            None => Change::First,
            Some((gsn, cfg)) => {
                if cfg != a.config_number {
                    Change::Reconfigured
                } else if gsn == a.gsn {
                    Change::Same
                } else {
                    Change::StateChanged
                }
            }
        }
    }

    /// Forget an accessory — after unpairing, or when it leaves the roster.
    pub fn forget(&mut self, device_id: &str) {
        self.seen.remove(device_id);
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// The accessory categories switcheroo cares to name (spec table 12-3).
pub fn category_name(c: u16) -> &'static str {
    match c {
        1 => "other",
        2 => "bridge",
        5 => "lightbulb",
        7 => "outlet",
        8 => "switch",
        9 => "thermostat",
        10 => "sensor",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regulatory(gsn: u16, cfg: u8, unpaired: bool) -> Vec<u8> {
        let mut v = vec![];
        v.extend_from_slice(&APPLE_COMPANY_ID.to_le_bytes());
        v.push(TYPE_HAP);
        v.push(0x31); // subtype 1, length 17
        v.push(if unpaired { 0x01 } else { 0x00 });
        v.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        v.extend_from_slice(&7u16.to_le_bytes()); // outlet
        v.extend_from_slice(&gsn.to_le_bytes());
        v.push(cfg);
        v.push(2); // compatible version
        v
    }

    #[test]
    fn a_regulatory_advert_reads_out_whole() {
        let Parsed::Regulatory(a) = parse(&regulatory(42, 1, false)) else {
            panic!("not parsed as regulatory");
        };
        assert_eq!(a.device_id, "AA:BB:CC:DD:EE:FF");
        assert_eq!(a.category, 7);
        assert_eq!(category_name(a.category), "outlet");
        assert_eq!(a.gsn, 42);
        assert_eq!(a.config_number, 1);
        assert_eq!(a.compatible_version, 2);
        assert!(!a.unpaired, "the paired bit is SET when NOT paired");

        let Parsed::Regulatory(fresh) = parse(&regulatory(1, 1, true)) else { panic!() };
        assert!(fresh.unpaired, "an accessory ready for pair-setup");
    }

    /// The whole point: a plug switched at the wall bumps its GSN, and a controller sees it
    /// without connecting, pairing, or holding a key.
    #[test]
    fn a_state_change_is_visible_without_connecting() {
        let mut t = GsnTracker::new();
        let a = |gsn| match parse(&regulatory(gsn, 1, false)) {
            Parsed::Regulatory(a) => a,
            _ => unreachable!(),
        };
        // First sighting is NOT a change — treating it as one refreshes every accessory on
        // every scan, which is the polling this exists to avoid.
        assert_eq!(t.observe(&a(10)), Change::First);
        assert_eq!(t.observe(&a(10)), Change::Same);
        assert_eq!(t.observe(&a(11)), Change::StateChanged);
        assert_eq!(t.observe(&a(11)), Change::Same);
        // GSN wraps to 1 rather than 0, and a wrap is still a change.
        assert_eq!(t.observe(&a(65535)), Change::StateChanged);
        assert_eq!(t.observe(&a(1)), Change::StateChanged, "a wrap must not read as Same");
        assert_eq!(t.len(), 1);
        t.forget("AA:BB:CC:DD:EE:FF");
        assert!(t.is_empty());
        assert_eq!(t.observe(&a(1)), Change::First, "after forgetting, it is new again");
    }

    /// A config-number bump is a different event: the attribute database changed, so cached
    /// instance ids are stale and — if it was a factory reset — so is the pairing.
    #[test]
    fn a_reconfiguration_is_not_just_a_state_change() {
        let mut t = GsnTracker::new();
        let a = |gsn, cfg| match parse(&regulatory(gsn, cfg, false)) {
            Parsed::Regulatory(a) => a,
            _ => unreachable!(),
        };
        assert_eq!(t.observe(&a(5, 1)), Change::First);
        assert_eq!(t.observe(&a(6, 2)), Change::Reconfigured, "config wins over the GSN move");
        assert_eq!(t.observe(&a(6, 2)), Change::Same);
    }

    /// "There is an event here I cannot read" is different information from "no event". A
    /// caller that conflated them would quietly stop noticing changes.
    #[test]
    fn an_encrypted_notification_is_reported_not_skipped() {
        let mut v = vec![];
        v.extend_from_slice(&APPLE_COMPANY_ID.to_le_bytes());
        v.push(TYPE_HAP_ENCRYPTED_NOTIFICATION);
        v.push(0x36);
        v.push(0x00);
        v.extend_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
        v.extend_from_slice(&[0u8; 8]);
        assert_eq!(
            parse(&v),
            Parsed::EncryptedNotification { device_id: "11:22:33:44:55:66".into() }
        );
    }

    /// Anything that is not a well-formed HAP advert is `NotHap` rather than a wrong
    /// reading. A malformed packet that parses is worse than one that does not.
    #[test]
    fn junk_and_other_vendors_are_refused() {
        assert_eq!(parse(&[]), Parsed::NotHap);
        assert_eq!(parse(&[0x4C]), Parsed::NotHap);
        // another vendor's manufacturer data
        let mut other = vec![0x59, 0x00, TYPE_HAP, 0x31];
        other.extend_from_slice(&[0u8; 11]);
        assert_eq!(parse(&other), Parsed::NotHap);
        // Apple, but not HomeKit (an iBeacon is type 0x02)
        let mut beacon = APPLE_COMPANY_ID.to_le_bytes().to_vec();
        beacon.push(0x02);
        beacon.extend_from_slice(&[0u8; 20]);
        assert_eq!(parse(&beacon), Parsed::NotHap);
        // truncated HAP
        let full = regulatory(3, 1, false);
        for cut in 3..full.len() {
            assert_eq!(parse(&full[..cut]), Parsed::NotHap, "a {cut}-byte advert parsed");
        }
        // a declared length that disagrees with the spec's 17
        let mut wrong = full.clone();
        wrong[3] = 0x30 | 9;
        assert_eq!(parse(&wrong), Parsed::NotHap, "a bad length byte was accepted");
    }

    /// btleplug hands manufacturer data keyed BY company id, so the two-byte prefix is
    /// already gone. Both shapes must reach the same answer.
    #[test]
    fn the_keyed_shape_matches_the_prefixed_one() {
        let full = regulatory(77, 3, false);
        assert_eq!(parse(&full), parse_keyed(APPLE_COMPANY_ID, &full[2..]));
        assert_eq!(parse_keyed(0x0059, &full[2..]), Parsed::NotHap);
        assert_eq!(parse_keyed(APPLE_COMPANY_ID, &[]), Parsed::NotHap);
    }
}
