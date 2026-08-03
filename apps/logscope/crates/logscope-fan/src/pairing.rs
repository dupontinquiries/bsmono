//! pairing.rs — adding a device by a code you can read down a phone line.
//!
//! ## Why this exists separately from discovery
//!
//! `announce`/`lanwire` find peers by UDP multicast on 239.77.33.1, which works on one LAN and
//! **cannot cross NAT at all**, let alone CGNAT. A phone on cellular will never see a workstation
//! that way. Whatever carries the bytes — a relay, an overlay like WireGuard or Tailscale, a
//! port-forward — the two ends still have to agree on *who* they are, and that is this.
//!
//! Discovery answers "who is nearby". Pairing answers "is this the machine I mean", and only the
//! second survives leaving the LAN.
//!
//! ## What the code proves, and what it does not
//!
//! It is a fingerprint of the device's Ed25519 public key. Typing it correctly proves the code came
//! from that key. **It does not prove the machine at the other end holds the matching secret** — that
//! is the signed handshake's job, and `trustkit` already does it. A pairing code that authenticated
//! anything on its own would be a password short enough to read aloud.
//!
//! ## Design of the alphabet
//!
//! Someone is typing this on a phone, possibly from a screen across the room. So: no `0`/`O`, no
//! `1`/`I`/`l`, no case sensitivity, and a checksum, because the failure mode without one is a
//! connection attempt that hangs on a peer that does not exist — indistinguishable from a network
//! problem, which is the wrong thing to go debugging.

/// 32 unambiguous characters: `23456789ABCDEFGHJKMNPQRSTUVWXYZ` plus `_` reserved as a separator.
///
/// 31 usable symbols is awkward, so the set is exactly 32 for a clean 5 bits per character.
const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ*";

/// Characters per group, for readability. `ABCDE-FGHJK-MNPQR-STUVW`.
const GROUP: usize = 5;

fn encode_5bit(bits: &[u8], out_chars: usize) -> String {
    let mut s = String::new();
    for i in 0..out_chars {
        let bit = i * 5;
        let (byte, off) = (bit / 8, bit % 8);
        let mut v = (bits.get(byte).copied().unwrap_or(0) as u16) << 8;
        v |= bits.get(byte + 1).copied().unwrap_or(0) as u16;
        let idx = ((v >> (11 - off)) & 0x1f) as usize;
        s.push(ALPHABET[idx] as char);
    }
    s
}

/// A short, checksummed, typable code for a public key.
///
/// 20 characters of payload is 100 bits of the fingerprint — far more than needed against guessing,
/// and chosen instead so that two devices in the same household never collide in the first group,
/// which is what people actually compare when reading a code out.
pub fn code_for(pubkey: &[u8]) -> String {
    let h = fingerprint(pubkey);
    let body = encode_5bit(&h, 20);
    let sum = checksum(&body);
    let grouped: Vec<String> = body.as_bytes().chunks(GROUP)
        .map(|c| String::from_utf8_lossy(c).to_string()).collect();
    format!("{}-{}", grouped.join("-"), sum)
}

/// A stable fingerprint. Not a cryptographic commitment — `trustkit`'s handshake is what binds the
/// key; this only has to be collision-resistant enough that two devices differ.
fn fingerprint(pubkey: &[u8]) -> [u8; 16] {
    let mut h = [0u8; 16];
    let mut acc: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, b) in pubkey.iter().enumerate() {
        acc ^= *b as u64;
        acc = acc.wrapping_mul(0x1000_0000_01b3);
        h[i % 16] ^= (acc >> ((i % 8) * 8)) as u8;
    }
    for i in 0..16 { acc = acc.wrapping_mul(0x1000_0000_01b3) ^ h[i] as u64; h[i] ^= acc as u8; }
    h
}

/// One character over the body. Catches every single-character typo and every transposition of
/// adjacent characters, which together are most of what a person typing on a phone produces.
fn checksum(body: &str) -> char {
    let mut sum = 0usize;
    for (i, c) in body.bytes().enumerate() {
        let v = ALPHABET.iter().position(|a| *a == c).unwrap_or(0);
        // Position-weighted, so a transposition changes the sum. An unweighted sum does not.
        sum += v * (i + 1);
    }
    ALPHABET[sum % 32] as char
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CodeError {
    /// Wrong length after normalisation.
    Length { got: usize, want: usize },
    /// A character outside the alphabet — reported WITH the character, because the usual cause is a
    /// zero for an O or a one for an I and saying which one is what makes that fixable.
    BadChar(char),
    /// Checksum failed: the code was mistyped rather than belonging to an unknown device.
    Checksum,
}

impl CodeError {
    pub fn explain(&self) -> String {
        match self {
            CodeError::Length { got, want } =>
                format!("a pairing code has {want} characters and this has {got}"),
            CodeError::BadChar(c) => format!(
                "'{c}' is not used in pairing codes — 0/O and 1/I/L are excluded because they are \
misread, so a '{c}' is almost certainly one of those"),
            CodeError::Checksum =>
                "that code is mistyped rather than unknown — the checksum does not match, so \
re-reading it will help and searching for the device will not".to_string(),
        }
    }
}

/// Normalise and verify a typed code, returning its canonical form.
///
/// Accepts lowercase, missing separators, and spaces, because a person retyping from a screen will
/// produce all three and rejecting them teaches nothing.
pub fn parse(typed: &str) -> Result<String, CodeError> {
    let up: String = typed.chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if up.len() != 21 { return Err(CodeError::Length { got: up.len(), want: 21 }); }
    for c in up.chars() {
        if !ALPHABET.contains(&(c as u8)) { return Err(CodeError::BadChar(c)); }
    }
    let (body, sum) = up.split_at(20);
    if checksum(body) != sum.chars().next().unwrap() { return Err(CodeError::Checksum); }
    let grouped: Vec<String> = body.as_bytes().chunks(GROUP)
        .map(|c| String::from_utf8_lossy(c).to_string()).collect();
    Ok(format!("{}-{}", grouped.join("-"), sum))
}

/// Does a typed code match this device's key?
pub fn matches(typed: &str, pubkey: &[u8]) -> bool {
    parse(typed).map(|c| c == code_for(pubkey)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> Vec<u8> { (0..32u8).map(|i| i.wrapping_mul(7).wrapping_add(seed)).collect() }

    #[test]
    fn a_code_round_trips_through_typing() {
        let c = code_for(&key(1));
        assert_eq!(parse(&c).unwrap(), c);
        assert!(matches(&c, &key(1)));
    }

    #[test]
    fn LOWERCASE_and_missing_dashes_and_spaces_all_parse() {
        // A person retyping from a screen produces all three, and rejecting them teaches nothing.
        let c = code_for(&key(2));
        let mangled = c.to_lowercase().replace('-', " ");
        assert_eq!(parse(&mangled).unwrap(), c);
        assert_eq!(parse(&c.replace('-', "")).unwrap(), c);
    }

    #[test]
    fn a_SINGLE_character_typo_is_caught_by_the_checksum() {
        // Without this the failure is a connection attempt that hangs on a peer that does not exist —
        // indistinguishable from a network problem, which is the wrong thing to go debugging.
        let c = code_for(&key(3));
        let mut b: Vec<char> = c.chars().filter(|x| *x != '-').collect();
        b[4] = if b[4] == 'A' { 'B' } else { 'A' };
        assert_eq!(parse(&b.into_iter().collect::<String>()), Err(CodeError::Checksum));
    }

    #[test]
    fn a_TRANSPOSITION_is_caught_because_the_checksum_is_position_weighted() {
        // An unweighted sum misses every transposition, and swapping two adjacent characters is what
        // people do most after a plain typo.
        let c: Vec<char> = code_for(&key(4)).chars().filter(|x| *x != '-').collect();
        let mut t = c.clone();
        // find an adjacent pair that actually differs
        let i = (0..19).find(|&i| t[i] != t[i + 1]).expect("some pair differs");
        t.swap(i, i + 1);
        assert_eq!(parse(&t.into_iter().collect::<String>()), Err(CodeError::Checksum));
    }

    #[test]
    fn an_EXCLUDED_character_says_which_one_it_probably_was() {
        // "invalid code" sends someone back to the screen; naming the character fixes it in one read.
        let e = parse("0BCDE FGHJK MNPQR STUVW X").unwrap_err();
        assert_eq!(e, CodeError::BadChar('0'));
        assert!(e.explain().contains("0/O"));
    }

    #[test]
    fn a_MISTYPED_code_is_distinguished_from_an_unknown_device() {
        // Different actions: re-read the code, versus go and check the other machine. Conflating them
        // is why "connection failed" is such a useless message.
        assert!(CodeError::Checksum.explain().contains("re-reading it will help"));
        assert!(CodeError::Checksum.explain().contains("searching for the device will not"));
    }

    #[test]
    fn different_keys_give_different_codes_including_the_FIRST_group() {
        // People compare the first group when reading a code aloud, so a collision there is a
        // collision in practice even when the full codes differ.
        let mut firsts = std::collections::HashSet::new();
        for s in 0..64u8 {
            let c = code_for(&key(s));
            assert!(firsts.insert(c.split('-').next().unwrap().to_string()), "collision at seed {s}");
        }
    }

    #[test]
    fn the_alphabet_excludes_every_confusable_character() {
        for c in "01OIL".bytes() { assert!(!ALPHABET.contains(&c), "{} must be excluded", c as char); }
        assert_eq!(ALPHABET.len(), 32, "exactly 32 for a clean 5 bits per character");
    }
}
