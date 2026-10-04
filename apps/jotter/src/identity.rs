//! identity -- who this device is: a name and two keypairs. The secrets live in one small text file in the app's
//! own data directory (mode 0600), never in a repository, a log or a QR.

use crypto_kit::{ed25519_keypair, ed25519_public_from_seed, x25519_keypair};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct Identity {
    pub name: String,
    ed_seed: Zeroizing<[u8; 32]>,
    pub ed_pub: [u8; 32],
    x_secret: Zeroizing<[u8; 32]>,
    pub x_pub: [u8; 32],
}

impl std::fmt::Debug for Identity {
    // never print a secret
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Identity({:?}, {})", self.name, fingerprint(&self.ed_pub))
    }
}

pub fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

pub fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 || !s.is_ascii() { return None; }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() { *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?; }
    Some(out)
}

/// The short form two people compare: the first 12 bytes of the signing key as `ABCD-EF01-...` (six groups).
pub fn fingerprint(ed_pub: &[u8; 32]) -> String {
    let h = hex(&ed_pub[..12]).to_uppercase();
    h.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("-")
}

impl Identity {
    pub fn generate(name: &str) -> Identity {
        let (ed_seed, ed_pub) = ed25519_keypair();
        let (x_secret, x_pub) = x25519_keypair();
        Identity { name: clean_name(name), ed_seed, ed_pub, x_secret, x_pub }
    }

    pub fn fingerprint(&self) -> String { fingerprint(&self.ed_pub) }

    /// A stable short id for lists and filenames: the first 8 bytes of the signing key, hex.
    pub fn id(&self) -> String { hex(&self.ed_pub[..8]) }

    pub(crate) fn sign(&self, msg: &[u8]) -> [u8; 64] { crypto_kit::sign(&self.ed_seed, msg) }
    /// Sign for the sync handshake (the same key, a different message).
    pub(crate) fn sign_public(&self, msg: &[u8]) -> [u8; 64] { self.sign(msg) }
    pub(crate) fn x_secret(&self) -> &[u8; 32] { &self.x_secret }

    pub fn to_text(&self) -> String {
        format!("jotter identity 1\nname = {}\ned_seed = {}\nx_secret = {}\n", self.name, hex(&*self.ed_seed), hex(&*self.x_secret))
    }

    pub fn from_text(t: &str) -> Option<Identity> {
        let get = |k: &str| t.lines().find_map(|l| l.strip_prefix(&format!("{k} = ")).map(str::to_string));
        if !t.starts_with("jotter identity 1") { return None; }
        let ed_seed: [u8; 32] = unhex(&get("ed_seed")?)?;
        let x_secret: [u8; 32] = unhex(&get("x_secret")?)?;
        let x_pub = x25519_public(&x_secret);
        Some(Identity { name: clean_name(&get("name")?), ed_pub: ed25519_public_from_seed(&ed_seed), ed_seed: Zeroizing::new(ed_seed), x_pub, x_secret: Zeroizing::new(x_secret) })
    }

    /// The identity in `path`, or a new one saved there (readable by this user only).
    pub fn load_or_create(path: &Path, default_name: &str) -> std::io::Result<Identity> {
        if let Ok(t) = std::fs::read_to_string(path) {
            if let Some(i) = Identity::from_text(&t) { return Ok(i); }
        }
        let id = Identity::generate(default_name);
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
        write_private(path, &id.to_text())?;
        Ok(id)
    }

    /// `$XDG_DATA_HOME/jotter/identity`, else `~/.local/share/jotter/identity`.
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))?;
        Some(base.join("jotter").join("identity"))
    }
}

fn x25519_public(secret: &[u8; 32]) -> [u8; 32] {
    // the public half of an X25519 secret: the base point scaled by it (what crypto-kit's keypair does at generation)
    crypto_kit::x25519_public_from_secret(secret)
}

/// A device name safe to show and to carry in a QR: no control characters, at most 32 characters.
pub fn clean_name(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control()).take(32).collect();
    let t = t.trim().to_string();
    if t.is_empty() { "device".into() } else { t }
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        f.write_all(text.as_bytes())
    }
    #[cfg(not(unix))]
    { let mut f = std::fs::File::create(path)?; f.write_all(text.as_bytes()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_round_trips_through_its_file_and_keeps_its_keys() {
        let a = Identity::generate("Tablet");
        let b = Identity::from_text(&a.to_text()).unwrap();
        assert_eq!((a.ed_pub, a.x_pub, &a.name), (b.ed_pub, b.x_pub, &b.name));
        assert_eq!(a.id(), b.id());
        assert!(Identity::from_text("junk").is_none());
        assert!(Identity::from_text("jotter identity 1\nname = x\ned_seed = 00\nx_secret = 00\n").is_none(), "short keys are refused");
    }

    #[test]
    fn the_fingerprint_is_six_groups_and_the_debug_form_shows_no_secret() {
        let a = Identity::generate("Tablet");
        let f = a.fingerprint();
        assert_eq!(f.len(), 6 * 4 + 5, "{f}");
        assert!(f.chars().all(|c| c.is_ascii_hexdigit() || c == '-') && f == f.to_uppercase());
        let dbg = format!("{a:?}");
        assert!(!dbg.contains(&hex(&*a.ed_seed)) && !dbg.contains(&hex(&*a.x_secret)), "{dbg}");
        assert_ne!(Identity::generate("x").fingerprint(), f, "two devices differ");
    }

    #[test]
    #[cfg(unix)]
    fn the_file_is_private_and_a_second_load_returns_the_same_device() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub").join("identity");
        let a = Identity::load_or_create(&p, "My Tablet").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600, "readable by this user only");
        let b = Identity::load_or_create(&p, "ignored").unwrap();
        assert_eq!((a.ed_pub, &a.name), (b.ed_pub, &b.name));
    }

    #[test]
    fn names_are_cleaned_and_hex_is_strict() {
        assert_eq!(clean_name("  Mara\u{7}'s tablet \n"), "Mara's tablet");
        assert_eq!(clean_name("\u{7}\u{8}"), "device");
        assert_eq!(clean_name(&"x".repeat(100)).len(), 32);
        assert_eq!(unhex::<2>("0aFf"), Some([0x0a, 0xff]));
        assert_eq!(unhex::<2>("0a"), None);
        assert_eq!(unhex::<2>("zzzz"), None);
    }
}
