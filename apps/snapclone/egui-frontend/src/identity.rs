//! identity — who this installation is, kept across restarts.
//!
//! `Snapclone::local_id` was `String::new()` at construction and nothing in the
//! running app ever set it. Three things quietly depended on it:
//!
//! * `my_code_ui` renders **your** code so someone can scan it back — with an
//!   empty id it drew a QR for the empty string, so the half of the exchange
//!   that does not need a camera silently did not work.
//! * `gossip::accept(&rec, len, &self.local_id)` drops records that came from
//!   yourself. With an empty id that filter never matches, so the app would
//!   have admitted its own broadcasts as a peer.
//! * `handshake::encode_v2(&self.local_id, ...)` put an empty sender field on
//!   the wire.
//!
//! None of the three fails loudly. That is the argument for doing identity
//! first: it is not a feature, it is the thing several features are quietly
//! reading.
//!
//! ## Why not the snapclone-identity crate
//!
//! `apps/snapclone/rust/snapclone-identity` has `generate_sc_id` and
//! `is_valid_sc_id` already, and reaching for it was the first instinct. It
//! depends on `rand`, which is not in this frontend's lock file, so taking it
//! means a lock update — and the format is eleven characters of hex whose
//! definition fits on one line. The duplication is real and is answered by
//! `the_format_agrees_with_what_the_scanner_accepts`, which checks this
//! module's own output against the QR parser rather than against a second copy
//! of the rule: if the two ever disagree, the failure is a code you cannot
//! scan, and that is the thing worth pinning.
//!
//! ## Randomness without a dependency
//!
//! Four bytes from `/dev/urandom`. The target set is Linux throughout —
//! postmarketOS, Fairphone, PinePhone, Asahi, desktop — so this is not a
//! portability compromise, it is the platform interface. The fallback when the
//! read fails is deliberately NOT silent-but-plausible: see
//! [`generate_sc_id`].

/// The file this lives in, under the XDG config dir. Separate from
/// `settings.conf` because it is not a setting: there is no correct alternative
/// value for a user to choose, and putting it on the settings page invites
/// editing the one field that must not casually change.
pub const FILE: &str = "identity.conf";

/// Everything durable about this installation's own identity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Identity {
    /// `sc-` followed by eight lowercase hex digits.
    pub sc_id: String,
}

/// Eleven characters: `sc-` and eight hex digits.
///
/// Case-insensitive on input because the QR parser accepts either — a code
/// printed in uppercase is still that person's code — but generation always
/// emits lowercase so the file has one spelling.
pub fn is_valid(s: &str) -> bool {
    s.len() == 11 && s.starts_with("sc-") && s[3..].chars().all(|c| c.is_ascii_hexdigit())
}

/// A fresh sc-ID.
///
/// Returns `None` rather than falling back to a clock- or pid-derived value
/// when the entropy source is unavailable. A degraded id would look completely
/// normal, be reproducible across two devices that booted alike, and the
/// consequence — two installations claiming the same identity, each dropping
/// the other's gossip as its own — would surface as "the other phone never
/// appears" with nothing anywhere pointing back here. Refusing is legible;
/// guessing is not.
pub fn generate_sc_id() -> Option<String> {
    let mut b = [0u8; 4];
    read_urandom(&mut b)?;
    Some(format!("sc-{:02x}{:02x}{:02x}{:02x}", b[0], b[1], b[2], b[3]))
}

fn read_urandom(buf: &mut [u8]) -> Option<()> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom").ok()?;
    f.read_exact(buf).ok()?;
    Some(())
}

/// Serialise. One key, and a comment saying what happens if you change it,
/// because someone will open this file.
pub fn render(id: &Identity) -> String {
    format!(
        "# snapclone identity. Changing sc_id makes this a different device to\n\
         # everyone who has already added you — they will not recognise the new\n\
         # one and you will not appear in their list.\n\
         [identity]\n\
         sc_id = {}\n",
        id.sc_id
    )
}

/// Parse. An absent or malformed id yields `None` so the caller can mint one;
/// this does NOT invent a value, because "the file said something unreadable"
/// and "there is no file" want the same handling but a different log line.
pub fn parse(text: &str) -> Option<Identity> {
    let ini = config_kit::Ini::parse(text);
    let raw = ini.get("identity", "sc_id")?.trim().to_ascii_lowercase();
    if is_valid(&raw) { Some(Identity { sc_id: raw }) } else { None }
}

/// What happened when the app asked who it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    /// Read from disk, as before.
    Existing(Identity),
    /// Freshly minted and written. Worth distinguishing: this is the moment to
    /// tell the user their code, and the only moment when losing the file has
    /// no cost.
    Created(Identity),
    /// No id on disk and none could be made. The app runs — refusing to start
    /// over identity would be worse — but it cannot be added by anyone, so the
    /// caller has to say so rather than presenting an empty QR code.
    Unavailable,
}

impl Loaded {
    pub fn sc_id(&self) -> Option<&str> {
        match self {
            Loaded::Existing(i) | Loaded::Created(i) => Some(&i.sc_id),
            Loaded::Unavailable => None,
        }
    }
}

/// Read the identity, minting and persisting one on first run.
///
/// A write failure does not discard the id: the app still gets a usable
/// identity for this session, it simply will not be the same one next launch.
/// Losing the session to a read-only config dir would be the worse trade.
pub fn load_or_create() -> Loaded {
    if let Some(id) = config_kit::load(crate::settings_store::APP, FILE).as_deref().and_then(parse) {
        return Loaded::Existing(id);
    }
    match generate_sc_id() {
        Some(sc_id) => {
            let id = Identity { sc_id };
            let _ = config_kit::save(crate::settings_store::APP, FILE, &render(&id));
            Loaded::Created(id)
        }
        None => Loaded::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_id_has_the_shape_the_format_promises() {
        let id = generate_sc_id().expect("/dev/urandom is the platform interface here");
        assert!(is_valid(&id), "{id}");
        assert_eq!(id.len(), 11);
        assert!(id[3..].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "generation emits one spelling: {id}");
    }

    #[test]
    fn the_format_agrees_with_what_the_scanner_accepts() {
        // The duplication this module admits to in its header, pinned against
        // the thing that matters: if generation and the QR parser ever
        // disagree, the symptom is a code that cannot be scanned, and neither
        // module's own tests would notice.
        let id = generate_sc_id().unwrap();
        let parsed = crate::qr::parse_payload(&id).expect("the scanner rejected our own id");
        assert_eq!(parsed.sc_id, id);
        // and through the URI form, which is what my_code_ui actually renders
        let uri = format!("snapclone://{id}");
        assert_eq!(crate::qr::parse_payload(&uri).unwrap().sc_id, id);
    }

    #[test]
    fn two_generated_ids_differ() {
        // Weak as a randomness test and not trying to be one. It catches the
        // specific failure that matters: a generator that returns a constant,
        // which would make every installation the same device.
        let a = generate_sc_id().unwrap();
        let b = generate_sc_id().unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn validation_rejects_the_near_misses() {
        assert!(is_valid("sc-0123abcd"));
        assert!(!is_valid("sc-0123abc"), "seven digits");
        assert!(!is_valid("sc-0123abcde"), "nine digits");
        assert!(!is_valid("sc-0123abcg"), "g is not hex");
        assert!(!is_valid("xc-0123abcd"), "wrong prefix");
        assert!(!is_valid(""), "the value local_id actually had");
    }

    #[test]
    fn an_id_round_trips_through_the_file() {
        let id = Identity { sc_id: "sc-deadbeef".into() };
        assert_eq!(parse(&render(&id)), Some(id));
    }

    #[test]
    fn an_unreadable_file_is_reported_rather_than_repaired() {
        // Both cases mint a new id at the call site, but they are different
        // events: a missing file is first run, a malformed one means the id
        // that other people have for you is gone. Collapsing them here would
        // lose the ability to say which happened.
        assert_eq!(parse(""), None);
        assert_eq!(parse("[identity]\nsc_id = \n"), None);
        assert_eq!(parse("[identity]\nsc_id = not-an-id\n"), None);
        assert_eq!(parse("[identity]\nsc_id = sc-0123abcg\n"), None);
    }

    #[test]
    fn an_uppercase_id_on_disk_is_accepted_and_normalised() {
        // Hand-edited, or copied from a printed code. Rejecting it would mint a
        // second identity for someone who thought they were restoring one.
        let got = parse("[identity]\nsc_id = SC-DEADBEEF\n");
        assert_eq!(got, Some(Identity { sc_id: "sc-deadbeef".into() }));
    }

    #[test]
    fn unavailable_reports_no_id_rather_than_an_empty_one() {
        // The empty string was the bug. A caller that unwraps this into
        // local_id must not be handed "" dressed up as an answer.
        assert_eq!(Loaded::Unavailable.sc_id(), None);
        assert_eq!(Loaded::Created(Identity { sc_id: "sc-00000001".into() }).sc_id(),
                   Some("sc-00000001"));
    }
}
