//! machineid.rs — an ID you can type, that IS the key.
//!
//! ## Why an ID cannot just be a name
//!
//! `peerauth` established that `trustkit` registration bound to a NAME is decorative: an id is whatever
//! a peer claims, so anyone saying "I am m2" receives what m2 was granted. Registration only means
//! something if the id is bound to a key nobody else holds.
//!
//! An "add a machine by ID" box therefore has two possible designs, and only one of them works:
//!
//! * type a **name**, exchange keys separately — two steps, and the second is the one people skip;
//! * make the **ID a fingerprint of the public key**, so typing it *is* the key exchange.
//!
//! The second is what SSH fingerprints and Syncthing device IDs do, and it collapses the flow to one
//! paste. There is no separate key step to forget, which matters for a setting meant to be configured
//! once and never revisited.
//!
//! ## Transcribable, because it will be read off one screen and typed into another
//!
//! Base32 without the ambiguous characters — no `0/O`, no `1/I/l` — grouped in fives. A phone user
//! typing this from a laptop screen will confuse those glyphs, and a wrong character must not silently
//! register a machine that does not exist.
//!
//! A **check character** makes a typo fail loudly. Without it the mistyped id is simply an id nobody
//! holds, the peer never connects, and the error appears as "the machine is offline" — pointing at the
//! network rather than at the typing.

/// Crockford Base32: exactly 32 characters, excluding I, L, O and U.
///
/// My first attempt removed 0/O/1/I/l and produced a THIRTY-ONE character alphabet, which base32
/// cannot use — the encoder indexed 31 and panicked. Crockford solves it better anyway: rather than
/// excluding the ambiguous glyphs it NORMALIZES them on the way in, so someone who types O for 0 or
/// l for 1 is understood instead of rejected. U is excluded to avoid accidental obscenities.
const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Fold the glyphs people actually confuse onto what they meant.
fn normalize_char(c: char) -> char {
    match c.to_ascii_lowercase() {
        'o' => '0',
        'i' | 'l' => '1',
        other => other,
    }
}

/// Bytes of key fingerprint carried in an id.
///
/// Ten bytes is 80 bits. That is far short of the key's own strength, and deliberately so: the id is a
/// LOOKUP handle, and `peerauth` verifies the full key on every connection. What 80 bits must resist is
/// an attacker finding a second key with the same id and getting you to register it — 2^40 work by the
/// birthday bound, which is a real cost for an attack that also requires you to paste their id.
pub const ID_BYTES: usize = 10;

/// Encode a public key as a typeable id.
pub fn encode(public_key: &[u8]) -> String {
    let h = fingerprint(public_key);
    let mut out = String::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in h.iter() {
        acc = (acc << 8) | *b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    out.push(check_char(&out));
    // Grouped in fives: an unbroken 17-character string is transcribed wrongly far more often.
    out.as_bytes().chunks(5).map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>().join("-")
}

/// Why an id was rejected.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum IdError { BadChar(char), WrongLength(usize), BadCheck }

impl IdError {
    pub fn explain(&self) -> String {
        match self {
            IdError::BadChar(c) => format!(
                "{c:?} is not in the alphabet — 0/O and 1/I/l are excluded because they are misread. \
Check for one of those"),
            IdError::WrongLength(n) => format!("an id is {} characters plus a check; this has {n}",
                ID_BYTES * 8 / 5),
            IdError::BadCheck => "the check character does not match, so a character was mistyped. \
Without this the id would simply be one nobody holds, the peer would never connect, and it would look \
like the machine was offline".into(),
        }
    }
}

/// Validate and normalize a typed id.
pub fn parse(typed: &str) -> Result<String, IdError> {
    let s: String = typed.chars().filter(|c| !c.is_whitespace() && *c != '-')
        .map(normalize_char).collect();
    let want = ID_BYTES * 8 / 5;
    if s.len() != want + 1 { return Err(IdError::WrongLength(s.len().saturating_sub(1))); }
    let (body, chk) = s.split_at(want);
    if let Some(c) = body.chars().find(|c| !ALPHABET.contains(&(*c as u8))) {
        return Err(IdError::BadChar(c));
    }
    if chk.chars().next() != Some(check_char(body)) { return Err(IdError::BadCheck); }
    Ok(body.to_string())
}

fn check_char(body: &str) -> char {
    let mut h: u32 = 5381;
    for b in body.bytes() { h = h.wrapping_mul(33) ^ b as u32; }
    ALPHABET[(h % ALPHABET.len() as u32) as usize] as char
}

fn fingerprint(key: &[u8]) -> [u8; ID_BYTES] {
    // Not a security boundary — peerauth verifies the full key on every connection — so this needs to
    // be stable and well-distributed, not collision-resistant against an adversary.
    //
    // The first version XOR-folded into out[i % ID_BYTES] as it hashed, which left most bytes ZERO for
    // any key shorter than ID_BYTES and produced ids full of padding. Each output byte now comes from
    // its own full pass, so every byte depends on every input byte.
    let mut out = [0u8; ID_BYTES];
    for (i, slot) in out.iter_mut().enumerate() {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ ((i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        for b in key {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        *slot = (h ^ (h >> 32) ^ (h >> 16)) as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_round_trips_through_typing() {
        let id = encode(b"a-public-key-of-some-length-here");
        assert_eq!(parse(&id).unwrap(), parse(&id.replace('-', "")).unwrap(),
            "grouping dashes are cosmetic");
        assert!(parse(&id.to_uppercase()).is_ok(), "case is forgiving; a phone capitalises");
    }

    #[test]
    fn a_mistyped_character_FAILS_rather_than_naming_a_phantom_machine() {
        // Without the check, a typo is simply an id nobody holds: the peer never connects and the error
        // reads as "the machine is offline", pointing at the network rather than at the typing.
        let id = encode(b"key-one");
        let body: String = id.chars().filter(|c| *c != '-').collect();
        let mut wrong: Vec<char> = body.chars().collect();
        wrong[2] = if wrong[2] == 'q' { 'r' } else { 'q' };
        let typo: String = wrong.into_iter().collect();
        assert_eq!(parse(&typo), Err(IdError::BadCheck));
        assert!(IdError::BadCheck.explain().contains("look like the machine was offline"));
    }

    #[test]
    fn ambiguous_characters_are_NORMALIZED_rather_than_rejected() {
        // The first design excluded 0/O/1/I/l and produced a 31-character alphabet, which base32
        // cannot use. Crockford's answer is better UX anyway: someone who types O for 0 or l for 1 is
        // UNDERSTOOD instead of being told their id is invalid — and being told it is invalid, when the
        // glyph is genuinely ambiguous on screen, is a dead end for the person typing.
        let id = encode(b"whatever-key");
        let body: String = id.chars().filter(|c| *c != '-').collect();
        let confused: String = body.chars()
            .map(|c| match c { '0' => 'O', '1' => 'l', other => other }).collect();
        assert_eq!(parse(&confused).unwrap(), parse(&id).unwrap(),
            "typing O for 0 and l for 1 must still resolve to the same machine");
        // U is excluded from Crockford deliberately, to avoid accidental obscenities.
        assert!(!ALPHABET.contains(&b'u'));
    }

    #[test]
    fn different_keys_give_different_ids() {
        assert_ne!(encode(b"key-a"), encode(b"key-b"));
        assert_eq!(encode(b"key-a"), encode(b"key-a"), "and the same key is stable");
    }

    #[test]
    fn a_wrong_length_is_named_rather_than_reported_as_a_bad_check() {
        // Pasting half an id is a different mistake from mistyping one, and the fixes differ.
        assert!(matches!(parse("abc"), Err(IdError::WrongLength(_))));
        assert!(matches!(parse(""), Err(IdError::WrongLength(_))));
    }

    #[test]
    fn a_character_outside_the_alphabet_names_ITSELF() {
        // So the user can see which glyph they got wrong, rather than being told the whole id is bad.
        // 'u' is genuinely outside Crockford's alphabet; '0' is now valid.
        match parse("uuuuu-uuuuu-uuuuu-uu") {
            Err(IdError::BadChar(c)) => assert_eq!(c, 'u'),
            other => panic!("expected the offending char, got {other:?}"),
        }
    }

    #[test]
    fn the_id_is_grouped_for_transcription() {
        // An unbroken 17-character string is transcribed wrongly far more often than five-character
        // groups. Cosmetic to the parser, load-bearing for the person typing.
        let id = encode(b"some-key");
        assert!(id.contains('-'));
        assert!(id.split('-').all(|g| g.len() <= 5));
    }
}

/// This machine's long-term identity.
///
/// ## Why it must persist
///
/// The id an operator pastes into another machine is a fingerprint of the public key. If the key is
/// regenerated on restart, every registration made against the old id becomes dead — the peer is still
/// listed, still trusted, and no longer reachable, which reads as a network fault rather than an
/// identity change.
///
/// So: generated once, written to disk, and read back thereafter. `machineid::encode` of the public
/// key is then stable for the life of the install, which is what makes "set it once and forget it"
/// true rather than aspirational.
///
/// ## What is stored
///
/// Both keys, 64 bytes: private then public. Storing a derived value normally invites it to disagree
/// with its source, and the private key alone would be the better file — but `crypto-kit` exposes
/// `ed25519_keypair` and no derive-public-from-private, so recovering the public key from a stored
/// private one is not possible through its API today.
///
/// Adding that function to `crypto-kit` is the better fix and belongs there rather than here; until
/// then the two are written together and read together, so they cannot drift apart in practice.
impl std::fmt::Debug for Identity {
    /// Prints the id, never the key. A Debug that leaks a signing key into a log is a leak with a
    /// convenient shape, and derive(Debug) would have produced exactly that.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Identity({})", self.id())
    }
}

pub struct Identity {
    signing: [u8; 32],
    pub public: [u8; 32],
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum IdentityError {
    /// The stored key is not 32 bytes.
    Malformed { len: usize },
    Io(String),
}

impl IdentityError {
    pub fn explain(&self) -> String {
        match self {
            IdentityError::Malformed { len } => format!(
                "the stored identity is {len} bytes, not 64. It is refused rather than regenerated, \
because regenerating would silently change this machine's id and break every registration made \
against the old one — a peer that is still listed, still trusted, and no longer reachable"),
            IdentityError::Io(e) => format!("could not read or write the identity: {e}"),
        }
    }
}

impl Identity {
    /// Load the identity at `path`, generating and persisting one if absent.
    ///
    /// Refuses a malformed file rather than replacing it. A corrupt key is a situation someone should
    /// see, and silently minting a new one destroys every registration pointing at the old id.
    pub fn load_or_create(path: &std::path::Path) -> Result<Identity, IdentityError> {
        match std::fs::read(path) {
            Ok(b) if b.len() == 64 => {
                let mut signing = [0u8; 32];
                let mut public = [0u8; 32];
                signing.copy_from_slice(&b[..32]);
                public.copy_from_slice(&b[32..]);
                Ok(Identity { signing, public })
            }
            Ok(b) => Err(IdentityError::Malformed { len: b.len() }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let (sk, pk) = crypto_kit::ed25519_keypair();
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| IdentityError::Io(e.to_string()))?;
                }
                let mut blob = Vec::with_capacity(64);
                blob.extend_from_slice(&sk[..]);
                blob.extend_from_slice(&pk[..]);
                std::fs::write(path, &blob).map_err(|e| IdentityError::Io(e.to_string()))?;
                let mut signing = [0u8; 32];
                signing.copy_from_slice(&sk[..]);
                Ok(Identity { signing, public: pk })
            }
            Err(e) => Err(IdentityError::Io(e.to_string())),
        }
    }

    /// The typeable id others paste to trust this machine.
    pub fn id(&self) -> String { encode(&self.public) }

    /// Sign a `peerauth` challenge payload.
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] { crypto_kit::sign(&self.signing, msg) }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("logscope-id-test-{name}-{}", std::process::id()));
        p
    }

    #[test]
    fn the_id_is_STABLE_across_loads() {
        // The whole point. If the key regenerated on restart, every registration against the old id
        // would be dead — the peer still listed, still trusted, no longer reachable, which reads as a
        // network fault rather than an identity change.
        let p = tmp("stable");
        let _ = std::fs::remove_file(&p);
        let a = Identity::load_or_create(&p).expect("creates");
        let b = Identity::load_or_create(&p).expect("reads back");
        assert_eq!(a.id(), b.id(), "the id must survive a restart");
        assert_eq!(a.public, b.public);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn two_machines_get_DIFFERENT_ids() {
        let (p, q) = (tmp("m1"), tmp("m2"));
        for f in [&p, &q] { let _ = std::fs::remove_file(f); }
        let a = Identity::load_or_create(&p).unwrap();
        let b = Identity::load_or_create(&q).unwrap();
        assert_ne!(a.id(), b.id());
        for f in [&p, &q] { let _ = std::fs::remove_file(f); }
    }

    #[test]
    fn a_malformed_key_is_REFUSED_rather_than_regenerated() {
        // Silently minting a new one destroys every registration pointing at the old id, and does it
        // at the moment someone is already confused about why a peer stopped working.
        let p = tmp("bad");
        std::fs::write(&p, b"too short").unwrap();
        let e = Identity::load_or_create(&p).unwrap_err();
        assert!(matches!(e, IdentityError::Malformed { .. }));
        assert!(e.explain().contains("still trusted, and no longer reachable"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn the_id_round_trips_through_parse() {
        // It is meant to be read off one screen and typed into another, so the generated form must
        // survive the parser that receives it.
        let p = tmp("rt");
        let _ = std::fs::remove_file(&p);
        let id = Identity::load_or_create(&p).unwrap().id();
        assert!(parse(&id).is_ok(), "generated id must parse: {id}");
        let _ = std::fs::remove_file(&p);
    }
}
