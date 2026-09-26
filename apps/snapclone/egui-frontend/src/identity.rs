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
//! ## Randomness: a canary read, then crypto_kit's own
//!
//! Key generation itself is delegated to `crypto_kit::ed25519_keypair`
//! (`sc_id` is [`fingerprint`]'s hash of the resulting public key, not
//! raw random bytes assembled here) — an earlier version of this doc
//! described minting the id from bytes read directly out of
//! `/dev/urandom`, which is no longer what `create()` does; this section
//! drifted out of sync with that refactor until now.
//!
//! What `create()` DOES still do itself is `read_urandom(&mut [0u8; 1])`
//! before calling into `crypto_kit` — a one-byte canary read of
//! `/dev/urandom`, refusing to mint an identity (`None`, not a silent
//! fallback to a weaker source) if that file can't be read at all. Worth
//! stating honestly what this canary does and does not guarantee:
//! `crypto_kit::ed25519_keypair` gets its randomness from `rand_core::
//! OsRng` with the `getrandom` feature, which on any Linux kernel newer
//! than 3.17 (every target here — postmarketOS, Fairphone, PinePhone,
//! Asahi, desktop — is well past that) calls the `getrandom(2)` SYSCALL
//! directly, not the `/dev/urandom` FILE this canary opens. The two are
//! backed by the same kernel entropy pool and fail together on any
//! ordinary system, so the canary is a reasonable correlated signal in
//! practice — but it is a DIFFERENT code path, not a guarantee: a sandbox
//! that blocks one and not the other (seccomp denying `getrandom(2)`
//! specifically, or a chroot with no `/dev`, either of which describes
//! some CI/dev-container environments) could pass this check and then
//! have `OsRng` panic anyway, since `crypto_kit` exposes no fallible
//! variant this module could catch instead.

/// The file this lives in, under the XDG config dir. Separate from
/// `settings.conf` because it is not a setting: there is no correct alternative
/// value for a user to choose, and putting it on the settings page invites
/// editing the one field that must not casually change.
pub const FILE: &str = "identity.conf";

/// Everything durable about this installation's own identity.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Identity {
    /// `sc-` followed by eight lowercase hex digits. **Derived from
    /// [`public`](Self::public)** — see [`fingerprint`].
    pub sc_id: String,
    /// Ed25519 verifying key. Travels in every announcement.
    pub public: [u8; 32],
    /// Ed25519 signing key. Never leaves this machine.
    pub secret: [u8; 32],
    /// X25519 key for the DHT mailbox (`mailbox.rs`). A SEPARATE key rather than
    /// the Ed25519 one converted: reusing a signing key for key agreement is
    /// what ed25519-dalek's own docs advise against. It is bound to the identity
    /// by a signature in the presence record (`#xsig:`), made with `secret`.
    pub x_public: [u8; 32],
    /// X25519 private key. Never leaves this machine.
    pub x_secret: [u8; 32],
}

impl Identity {
    /// The `#xsig:` signature: the identity key vouching for the mailbox key.
    pub fn xsig(&self) -> [u8; 64] {
        crypto_kit::sign(&self.secret, &crate::mailbox::xsig_message(&self.x_public))
    }
}

/// A fresh X25519 pair, as `(secret, public)`.
fn x25519_pair() -> ([u8; 32], [u8; 32]) {
    let (s, p) = crypto_kit::x25519_keypair();
    (*s, p)
}

/// Redacted by hand rather than derived, so a stray `{:?}` in a log line
/// cannot print a signing key. The derive was there before the struct had a
/// secret in it; leaving it would have been the kind of change nothing
/// notices.
impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("sc_id", &self.sc_id)
            .field("public", &hex(&self.public))
            .field("secret", &"<redacted>")
            .field("x_public", &hex(&self.x_public))
            .field("x_secret", &"<redacted>")
            .finish()
    }
}

/// The sc-ID a public key implies: `sc-` and the first four bytes, in hex.
///
/// **Why derive it at all.** Signing alone proves "some key signed this". It
/// does not prove that key owns `sc-32a51661` — an impersonator generates their
/// own keypair, puts your sc-ID in the `id` field, signs with theirs, and it
/// verifies. Binding the id to the key closes that, and it is why the previous
/// commit's random sc-IDs are regenerated on upgrade.
///
/// **And why that is not enough.** Four bytes is thirty-two bits, fixed by the
/// format Qt defined. Grinding a keypair whose fingerprint matches a chosen
/// sc-ID is about 2^32 work, which is minutes. So the sc-ID is a convenience
/// handle, not a cryptographic identity: the full public key in the datagram is
/// what verification uses. Pinning a peer's key on first contact is what makes
/// grinding useless against someone you have already met, and that is the next
/// piece — `contact_book` is where the pin belongs, and it does not have one
/// yet. Until it does, treat a first sighting as unauthenticated.
pub fn fingerprint(public: &[u8; 32]) -> String {
    format!("sc-{}", hex(&public[..4]))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 { return None; }
    let mut out = [0u8; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Eleven characters: `sc-` and eight hex digits.
///
/// Case-insensitive on input because the QR parser accepts either — a code
/// printed in uppercase is still that person's code — but generation always
/// emits lowercase so the file has one spelling.
/// This device's DHT node id, derived from its sc-ID.
///
/// Deterministic, so a device keeps its place in the keyspace across
/// restarts — `node_host::spawn` explains why that matters: a node that
/// re-randomises its identity on every launch makes every other node's
/// routing table stale.
///
/// The same hash the rest of the app uses for keys, not a new one. A
/// second hashing scheme here would be a second thing to keep in step
/// with dht-kit's key derivation, and they must agree.
/// The value id this device publishes its presence record under.
///
/// One per sc-ID, which is right BECAUSE there is now exactly one record
/// per peer: a single value carrying every address. It was wrong while the
/// app published one value per address — OpenDHT keys a value SET by this
/// id, so three addresses under one id were two silent overwrites.
///
/// Stable across restarts on purpose. A fresh id each launch would leave
/// the previous record live until its TTL, so a peer looking us up would
/// find this morning's addresses beside this afternoon's with nothing to
/// choose between them but the `#ts` stamp.
pub fn dht_value_id(sc_id: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in sc_id.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn dht_node_id(sc_id: &str) -> snapcore::routing::NodeId {
    let mut out = [0u8; 20];
    // FNV-1a over the sc-ID, expanded across the id. Not a security
    // boundary: a node id is a POSITION in the keyspace, and an attacker
    // who wants a particular position can simply pick one — Kademlia's
    // defence against that is redundancy, not id secrecy.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, b) in sc_id.bytes().enumerate() {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
        out[i % 20] ^= (h >> 32) as u8;
    }
    // A short sc-ID would otherwise leave most bytes zero, clustering
    // every short id into the same corner of the keyspace.
    for i in 0..20 {
        h = h.wrapping_mul(0x1000_0000_01b3).wrapping_add(i as u64);
        out[i] ^= (h >> 24) as u8;
    }
    snapcore::routing::NodeId(out)
}

pub fn is_valid(s: &str) -> bool {
    s.len() == 11 && s.starts_with("sc-") && s[3..].chars().all(|c| c.is_ascii_hexdigit())
}

/// A fresh identity: a keypair, and the sc-ID its public half implies.
///
/// Returns `None` rather than a plausible-looking fallback when the OS has no
/// entropy. `crypto_kit::ed25519_keypair` uses `OsRng`, which PANICS in that
/// case, so `/dev/urandom` is probed first and a broken machine gets a reported
/// failure instead of a crash on the first frame. A degraded identity would be
/// worse than either: reproducible across two devices that booted alike, and
/// the consequence — two installations each dropping the other's gossip as its
/// own — surfaces as "the other phone never appears" with nothing pointing back
/// here.
///
/// The probe and `OsRng` are two different syscalls (see the module doc's
/// "Randomness" section) that fail together on any ordinary system but are
/// not the same code path — a correlated signal this crate can act on, not
/// an airtight guarantee `OsRng` cannot still panic despite this passing.
pub fn create() -> Option<Identity> {
    read_urandom(&mut [0u8; 1])?;
    let (secret, public) = crypto_kit::ed25519_keypair();
    let (x_secret, x_public) = x25519_pair();
    Some(Identity { sc_id: fingerprint(&public), public, secret: *secret, x_public, x_secret })
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
        "# snapclone identity. SECRET — this file contains a signing key.\n\
         # sc_id is derived from public; editing either alone makes the file\n\
         # invalid and a fresh identity is minted, which makes this a different\n\
         # device to everyone who has already added you.\n\
         [identity]\n\
         sc_id = {}\n\
         public = {}\n\
         secret = {}\n\
         x_public = {}\n\
         x_secret = {}\n",
        id.sc_id, hex(&id.public), hex(&id.secret), hex(&id.x_public), hex(&id.x_secret)
    )
}

/// Parse. An absent or malformed id yields `None` so the caller can mint one;
/// this does NOT invent a value, because "the file said something unreadable"
/// and "there is no file" want the same handling but a different log line.
pub fn parse(text: &str) -> Option<Identity> {
    let ini = config_kit::Ini::parse(text);
    let raw = ini.get("identity", "sc_id")?.trim().to_ascii_lowercase();
    if !is_valid(&raw) { return None; }
    let public: [u8; 32] = unhex(ini.get("identity", "public")?)?;
    let secret: [u8; 32] = unhex(ini.get("identity", "secret")?)?;
    // The binding is re-checked on every load, not just at creation. A file
    // where sc_id and public disagree is either hand-edited or half-written,
    // and honouring the stored sc_id would announce an id this key cannot
    // claim — every peer would refuse us, with the failure appearing on their
    // side rather than ours.
    if fingerprint(&public) != raw { return None; }
    // The mailbox key arrived after the identity format: a file from before it
    // has none, and gets one here (`load_or_create` writes it back). A file
    // whose x_public does not match its x_secret is repaired the same way -- a
    // mismatched pair would publish a key nobody can seal to us under.
    let x = (|| {
        let xs: [u8; 32] = unhex(ini.get("identity", "x_secret")?)?;
        let xp: [u8; 32] = unhex(ini.get("identity", "x_public")?)?;
        let derived = x25519_dalek_public(&xs);
        (derived == xp).then_some((xs, xp))
    })();
    let (x_secret, x_public) = x.unwrap_or_else(x25519_pair);
    Some(Identity { sc_id: raw, public, secret, x_public, x_secret })
}

/// The X25519 public key for a secret, via the DH with the base point: the
/// shared secret of `secret` with the generator IS its public key.
fn x25519_dalek_public(secret: &[u8; 32]) -> [u8; 32] {
    let mut base = [0u8; 32];
    base[0] = 9;
    *crypto_kit::x25519_shared(secret, &base)
}

/// Does this file already carry a mailbox key? A `false` means `parse` minted
/// one, and the file should be rewritten so it survives the next launch.
fn has_mailbox_key(text: &str) -> bool {
    text.contains("x_secret")
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
    if let Some(text) = config_kit::load(crate::settings_store::APP, FILE) {
        if let Some(id) = parse(&text) {
            if !has_mailbox_key(&text)
                && config_kit::save(crate::settings_store::APP, FILE, &render(&id)).is_ok()
            {
                restrict(&config_kit::config_dir(crate::settings_store::APP).join(FILE));
            }
            return Loaded::Existing(id);
        }
    }
    match create() {
        Some(id) => {
            if config_kit::save(crate::settings_store::APP, FILE, &render(&id)).is_ok() {
                // 0600 AFTER the write, because the file has to exist first.
                // A signing key under the default 0644 is readable by every
                // other account on a shared machine, and config-kit has no
                // reason to know some of its callers write secrets.
                restrict(&config_kit::config_dir(crate::settings_store::APP).join(FILE));
            }
            Loaded::Created(id)
        }
        None => Loaded::Unavailable,
    }
}

/// Owner-only. Best effort: a filesystem that cannot express it (a FAT card, a
/// bind mount) is not a reason to refuse to run, but it IS a reason the caller
/// might want to warn, so the result is returned rather than dropped.
pub fn restrict(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).is_ok();
    }
    #[allow(unreachable_code)]
    { let _ = path; false }
}

#[cfg(test)]
mod tests {
    /// The mailbox key: minted with the identity, persisted, signed by it, and
    /// added to a file written before it existed.
    #[test]
    fn the_mailbox_key_is_minted_persisted_and_vouched_for() {
        let id = create().unwrap();
        assert_ne!(id.x_public, [0u8; 32]);
        assert_eq!(x25519_dalek_public(&id.x_secret), id.x_public, "a mismatched X25519 pair");
        let back = parse(&render(&id)).unwrap();
        assert_eq!(back, id, "the mailbox key did not survive a round trip");
        assert!(crypto_kit::verify(&id.public, &crate::mailbox::xsig_message(&id.x_public), &id.xsig()));
        // A pre-mailbox file: same identity, a new valid mailbox key.
        let old = format!("[identity]\nsc_id = {}\npublic = {}\nsecret = {}\n",
                          id.sc_id, hex(&id.public), hex(&id.secret));
        assert!(!has_mailbox_key(&old));
        let up = parse(&old).unwrap();
        assert_eq!((up.sc_id.as_str(), up.public, up.secret), (id.sc_id.as_str(), id.public, id.secret));
        assert_eq!(x25519_dalek_public(&up.x_secret), up.x_public);
        assert!(has_mailbox_key(&render(&up)));
        // The secret stays out of Debug.
        assert!(!format!("{id:?}").contains(&hex(&id.x_secret)));
    }


    #[test]
    fn a_node_id_is_stable_across_restarts() {
        // The whole reason it is derived rather than random: a node that
        // re-randomises on each launch makes every other node's routing
        // table stale.
        assert_eq!(super::dht_node_id("sc-abcd"), super::dht_node_id("sc-abcd"));
    }

    #[test]
    fn different_ids_land_in_different_places() {
        assert_ne!(super::dht_node_id("sc-abcd"), super::dht_node_id("sc-abce"));
    }

    #[test]
    fn a_short_sc_id_still_fills_the_keyspace() {
        // A naive per-byte hash leaves most of a 20-byte id zero for a
        // 4-character input, clustering every short id into one corner
        // of the keyspace — where they all become each other's nearest
        // neighbours.
        let id = super::dht_node_id("ab");
        let zeros = id.0.iter().filter(|b| **b == 0).count();
        assert!(zeros < 8, "{} of 20 bytes are zero: {:?}", zeros, id.0);
    }
    use super::*;

    #[test]
    fn a_generated_id_has_the_shape_the_format_promises() {
        let id = create().expect("/dev/urandom is the platform interface here");
        assert!(is_valid(&id.sc_id), "{}", id.sc_id);
        assert_eq!(id.sc_id.len(), 11);
        assert!(id.sc_id[3..].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "generation emits one spelling: {}", id.sc_id);
    }

    #[test]
    fn the_id_is_the_keys_fingerprint_and_not_an_independent_random() {
        // The whole reason signing is worth anything. Without this binding an
        // impersonator generates their own keypair, puts your sc-ID in the id
        // field, signs with theirs, and it verifies.
        let id = create().unwrap();
        assert_eq!(id.sc_id, fingerprint(&id.public));
        assert_eq!(id.sc_id, format!("sc-{:02x}{:02x}{:02x}{:02x}",
                                     id.public[0], id.public[1], id.public[2], id.public[3]));
    }

    #[test]
    fn the_keypair_actually_signs_and_verifies() {
        let id = create().unwrap();
        let sig = crypto_kit::sign(&id.secret, b"hello");
        assert!(crypto_kit::verify(&id.public, b"hello", &sig));
        assert!(!crypto_kit::verify(&id.public, b"hellp", &sig), "a changed message verifies");
        let other = create().unwrap();
        assert!(!crypto_kit::verify(&other.public, b"hello", &sig), "anyone's key verifies");
    }

    #[test]
    fn a_secret_key_does_not_appear_in_debug_output() {
        // The derive was there before the struct had a secret in it. A stray
        // {:?} in a log line is how a signing key ends up in a bug report.
        let id = create().unwrap();
        let shown = format!("{id:?}");
        assert!(shown.contains("redacted"), "{shown}");
        assert!(!shown.contains(&super::hex(&id.secret)), "the key is in the output");
    }

    #[test]
    fn the_format_agrees_with_what_the_scanner_accepts() {
        // The duplication this module admits to in its header, pinned against
        // the thing that matters: if generation and the QR parser ever
        // disagree, the symptom is a code that cannot be scanned, and neither
        // module's own tests would notice.
        let id = create().unwrap().sc_id;
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
        let a = create().unwrap().sc_id;
        let b = create().unwrap().sc_id;
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
    fn an_identity_round_trips_through_the_file_keys_and_all() {
        let id = create().unwrap();
        assert_eq!(parse(&render(&id)), Some(id));
    }

    #[test]
    fn a_file_whose_id_and_key_disagree_is_refused() {
        // Hand-edited, or half-written. Honouring the stored sc_id would
        // announce an id this key cannot claim: every peer refuses us, and the
        // failure appears on their side rather than ours.
        let id = create().unwrap();
        let good = render(&id);
        let bad = good.replace(&id.sc_id, "sc-00000000");
        assert_eq!(parse(&bad), None);
        // and a truncated key is refused rather than zero-padded
        assert_eq!(parse(&good.replace(&super::hex(&id.public), "abcd")), None);
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
        // and a valid id with no key at all — which is exactly what the
        // previous commit wrote, so this is the upgrade path: the old file is
        // unreadable and a fresh keyed identity is minted.
        assert_eq!(parse("[identity]\nsc_id = sc-0123abcd\n"), None);
    }

    #[test]
    fn an_uppercase_id_on_disk_is_accepted_and_normalised() {
        // Hand-edited, or copied from a printed code. Rejecting it would mint a
        // second identity for someone who thought they were restoring one.
        let id = create().unwrap();
        let shouty = render(&id).replace(&id.sc_id, &id.sc_id.to_ascii_uppercase());
        assert_eq!(parse(&shouty), Some(id));
    }

    #[test]
    fn unavailable_reports_no_id_rather_than_an_empty_one() {
        // The empty string was the bug. A caller that unwraps this into
        // local_id must not be handed "" dressed up as an answer.
        assert_eq!(Loaded::Unavailable.sc_id(), None);
        let id = create().unwrap();
        assert_eq!(Loaded::Created(id.clone()).sc_id(), Some(id.sc_id.as_str()));
    }
}
