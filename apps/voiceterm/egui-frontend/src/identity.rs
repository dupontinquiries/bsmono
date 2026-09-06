//! identity — who this voiceterm installation is, kept across restarts, and the digest of
//! what it is offering to share.
//!
//! `TODO_multi_device_share.md` names this as the prerequisite, and says why the panel was
//! left unreachable rather than wired to placeholders: *"Inventing placeholder strings for
//! a panel whose whole purpose is a TRUST decision would be worse than leaving it
//! unreachable — it would look like a real identity while being fabricated."* So this is the
//! real thing, on snapclone's proven pattern (`snapclone-egui/src/identity.rs`):
//!
//! * an **Ed25519 keypair** from `crypto_kit`, minted once and persisted under the app's
//!   config dir as `identity.conf`, chmod 0600 after the write — a signing key under the
//!   default 0644 is readable by every other account on a shared machine;
//! * a **fingerprint**, `vt-` + the first four bytes of the public key in hex, which is
//!   what `PeerPanel::my_key_fp` shows and what a person reads aloud to compare;
//! * a **handle**, the id the other side sees, defaulting to the fingerprint and
//!   overridable by the operator (a name is for people, a fingerprint is for verification);
//! * a **workspace digest** over the notes being shared, so both sides can confirm they are
//!   looking at the same snapshot before trusting anything.
//!
//! What the fingerprint is NOT: eight hex characters is about 2^32 to grind, which is
//! minutes. It is a convenience handle for comparing out of band, not the cryptographic
//! identity — the public key is. Pinning a peer's key on first contact is what makes
//! grinding useless against someone you have already met, and voiceterm has nowhere to pin
//! one yet; a first sighting is unauthenticated, and the panel says so rather than this
//! module pretending otherwise.
//!
//! Pure except for the two IO functions at the bottom, so the derivation, the format and
//! the digest are all testable without a filesystem.

/// The file, under `config_kit::config_dir("voiceterm")`.
pub const FILE: &str = "identity.conf";

/// This installation.
#[derive(Clone, PartialEq, Eq)]
pub struct Identity {
    /// `vt-xxxxxxxx`, derived from `public`.
    pub vt_id: String,
    /// The operator's handle, shown to the peer. Defaults to `vt_id`.
    pub handle: String,
    pub public: [u8; 32],
    secret: [u8; 32],
}

impl std::fmt::Debug for Identity {
    /// The secret is never in a log line, a panic message or a test failure.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("vt_id", &self.vt_id)
            .field("handle", &self.handle)
            .field("public", &hex(&self.public))
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl Identity {
    /// Sign with this identity's key — what a handshake payload is authenticated with.
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        crypto_kit::sign(&self.secret, msg)
    }
}

/// `vt-` plus the first four bytes of the public key. Lowercase hex, twelve characters.
pub fn fingerprint(public: &[u8; 32]) -> String {
    format!("vt-{}", hex(&public[..4]))
}

/// Does this look like a voiceterm id? Used when parsing the file back.
pub fn is_valid(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("vt-") else { return false };
    rest.len() == 8 && rest.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// A digest of the notes this device is offering, so both sides can confirm the same
/// snapshot. Titles and bodies, order-independent (two decks with the same notes in a
/// different order ARE the same workspace) — FNV-1a over the sorted `title\u{1f}body`
/// records, sixteen hex characters.
pub fn workspace_digest<'a>(notes: impl Iterator<Item = (&'a str, &'a str)>) -> String {
    let mut recs: Vec<String> = notes.map(|(t, b)| format!("{}\u{1f}{}", t.trim(), b.trim())).collect();
    recs.sort();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for r in &recs {
        for b in r.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        // a separator inside the hash too, so ["ab",""] and ["a","b"] differ
        h ^= 0xff;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    format!("{h:016x}")
}

/// Mint one. `None` when the system has no usable randomness — see the canary read.
pub fn create() -> Option<Identity> {
    // A one-byte canary read of /dev/urandom BEFORE calling into crypto_kit, whose
    // `OsRng` panics rather than returning an error. On a machine where randomness is
    // unavailable this returns `None` and the caller says so; without it the app aborts.
    read_urandom(&mut [0u8; 1])?;
    let (secret, public) = crypto_kit::ed25519_keypair();
    let vt_id = fingerprint(&public);
    Some(Identity { handle: vt_id.clone(), vt_id, public, secret: *secret })
}

fn read_urandom(buf: &mut [u8]) -> Option<()> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom").ok()?;
    f.read_exact(buf).ok()?;
    Some(())
}

/// Serialise, with a comment saying what happens if you edit it — someone will open it.
pub fn render(id: &Identity) -> String {
    format!(
        "# voiceterm identity. SECRET — this file contains a signing key.\n\
         # vt_id is derived from public; editing either alone makes the file invalid and a\n\
         # fresh identity is minted, which makes this a different device to every peer who\n\
         # has already verified you. `handle` is yours to change; it is a name, not proof.\n\
         [identity]\n\
         vt_id = {}\n\
         handle = {}\n\
         public = {}\n\
         secret = {}\n",
        id.vt_id,
        id.handle,
        hex(&id.public),
        hex(&id.secret)
    )
}

/// Parse. `None` for absent, malformed, or — the case worth naming — an id that does not
/// match its own public key, which is a hand-edited file rather than one of ours.
pub fn parse(text: &str) -> Option<Identity> {
    let ini = config_kit::Ini::parse(text);
    let vt_id = ini.get("identity", "vt_id")?.trim().to_ascii_lowercase();
    if !is_valid(&vt_id) {
        return None;
    }
    let public: [u8; 32] = unhex(ini.get("identity", "public")?)?;
    let secret: [u8; 32] = unhex(ini.get("identity", "secret")?)?;
    if fingerprint(&public) != vt_id {
        return None;
    }
    let handle = ini
        .get("identity", "handle")
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| vt_id.clone());
    Some(Identity { vt_id, handle, public, secret })
}

/// What happened when the app asked for its identity — a different line for each, because
/// "minted a new one" is news and "loaded the existing one" is not.
#[derive(Debug)]
pub enum Loaded {
    Existing(Identity),
    Created(Identity),
    /// No randomness, or the file could not be written. The peer panel stays unreachable,
    /// which is the honest state — see the module docs.
    Unavailable,
}

impl Loaded {
    pub fn identity(&self) -> Option<&Identity> {
        match self {
            Loaded::Existing(i) | Loaded::Created(i) => Some(i),
            Loaded::Unavailable => None,
        }
    }
    /// One line for the log or the panel.
    pub fn note(&self) -> String {
        match self {
            Loaded::Existing(i) => format!("identity {}", i.vt_id),
            Loaded::Created(i) => format!("new identity {} — identity.conf holds a signing key, keep it", i.vt_id),
            Loaded::Unavailable => "no identity: this system has no usable randomness, so peering is off".into(),
        }
    }
}

/// The app's entry point: load, else mint and persist.
pub fn load_or_create(app: &str) -> Loaded {
    if let Some(id) = config_kit::load(app, FILE).as_deref().and_then(parse) {
        return Loaded::Existing(id);
    }
    match create() {
        Some(id) => {
            if config_kit::save(app, FILE, &render(&id)).is_ok() {
                // 0600 AFTER the write, because the file has to exist first.
                restrict(&config_kit::config_dir(app).join(FILE));
            }
            Loaded::Created(id)
        }
        None => Loaded::Unavailable,
    }
}

/// Owner-only. Best effort: a filesystem that cannot express it (a FAT card, a bind mount)
/// is not a reason to refuse to run, but it is a reason a caller might warn.
pub fn restrict(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).is_ok();
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed() -> Identity {
        let public = [0xab; 32];
        Identity { vt_id: fingerprint(&public), handle: "kessler".into(), public, secret: [0x11; 32] }
    }

    #[test]
    fn an_id_is_derived_from_the_key_and_a_file_that_disagrees_is_refused() {
        let id = fixed();
        assert_eq!(id.vt_id, "vt-abababab");
        assert!(is_valid(&id.vt_id));
        assert!(!is_valid("vt-ABABABAB"), "uppercase is not our form");
        assert!(!is_valid("sc-abababab") && !is_valid("vt-abab") && !is_valid("abababab"));
        let text = render(&id);
        let back = parse(&text).expect("round trip");
        assert_eq!(back.vt_id, id.vt_id);
        assert_eq!(back.handle, "kessler");
        assert_eq!(back.public, id.public);
        // a hand-edited id that no longer matches its key is not an identity
        let tampered = text.replace("vt_id = vt-abababab", "vt_id = vt-deadbeef");
        assert!(parse(&tampered).is_none(), "the id must match its own public key");
        // and neither is a truncated key
        assert!(parse(&text.replace(&hex(&id.public), "abcd")).is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn the_handle_defaults_to_the_id_and_is_the_only_editable_field() {
        let mut id = fixed();
        id.handle = String::new();
        let text = render(&id);
        assert_eq!(parse(&text).unwrap().handle, "vt-abababab", "an empty handle falls back to the id");
        let renamed = text.replace("handle = ", "handle = ada");
        let p = parse(&renamed).unwrap();
        assert_eq!(p.handle, "ada");
        assert_eq!(p.vt_id, "vt-abababab", "renaming does not change who you are");
    }

    #[test]
    fn the_secret_never_appears_in_a_debug_line() {
        let id = fixed();
        let d = format!("{id:?}");
        assert!(d.contains("vt-abababab") && d.contains("<redacted>"));
        assert!(!d.contains(&hex(&id.secret)), "the signing key was printed");
    }

    #[test]
    fn the_workspace_digest_ignores_order_but_not_content() {
        let a = workspace_digest([("Gardens", "compost"), ("Kitchen", "bread")].into_iter());
        let b = workspace_digest([("Kitchen", "bread"), ("Gardens", "compost")].into_iter());
        assert_eq!(a, b, "the same notes in another order are the same workspace");
        let c = workspace_digest([("Gardens", "compost"), ("Kitchen", "bread!")].into_iter());
        assert_ne!(a, c);
        // the record separator matters: these two decks are different
        assert_ne!(
            workspace_digest([("ab", "")].into_iter()),
            workspace_digest([("a", "b")].into_iter())
        );
        assert_eq!(a.len(), 16);
        assert_eq!(workspace_digest(std::iter::empty()).len(), 16, "an empty deck still has a digest");
    }

    #[test]
    fn a_minted_identity_signs_with_its_own_key() {
        let Some(id) = create() else { return }; // no randomness here: nothing to assert
        assert!(is_valid(&id.vt_id));
        assert_eq!(id.handle, id.vt_id);
        let sig = id.sign(b"handshake");
        assert!(crypto_kit::verify(&id.public, b"handshake", &sig));
        assert!(!crypto_kit::verify(&id.public, b"handshake!", &sig));
        // and it round-trips through the file format with its key intact
        let back = parse(&render(&id)).unwrap();
        assert!(crypto_kit::verify(&back.public, b"handshake", &back.sign(b"handshake")));
    }
}
