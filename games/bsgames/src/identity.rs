//! identity — the local player's persisted profile-editing state: a stable random id (generated
//! once, the "friend code" others add you by) and an editable display name. Separate from
//! `netcode::profile::Profile` (which is the *published* wire shape); this is what's saved to disk
//! and what the Profile screen edits before publishing.

use std::io::Read;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalIdentity {
    pub id: String,
    pub name: String,
}

fn config_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(x).join("bsgames");
    }
    if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h).join(".config").join("bsgames");
    }
    if let Ok(h) = std::env::var("USERPROFILE") {
        return PathBuf::from(h).join("bsgames");
    }
    PathBuf::from("bsgames-config")
}

fn path() -> PathBuf {
    config_dir().join("identity.txt")
}

/// A short, friend-code-shaped random id: 10 lowercase-hex chars from `/dev/urandom` when
/// available (Linux, the fleet's target), falling back to a time+pid mix elsewhere/if the read
/// fails — good enough for a non-secret "add me by this code" token, not a security credential.
fn gen_id() -> String {
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let mut buf = [0u8; 5];
        if f.read_exact(&mut buf).is_ok() {
            return buf.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:010x}", (t as u64) ^ (std::process::id() as u64))
}

impl LocalIdentity {
    /// Load the saved identity, or mint a fresh one (new random id, a placeholder name the Profile
    /// screen prompts the wizard to fill in) and save it immediately so the id is stable from now on.
    pub fn load_or_create() -> Self {
        if let Ok(text) = std::fs::read_to_string(path()) {
            if let Some(me) = Self::parse(&text) {
                return me;
            }
        }
        let fresh = LocalIdentity { id: gen_id(), name: whoami_fallback() };
        fresh.save();
        fresh
    }

    fn parse(text: &str) -> Option<Self> {
        let mut id = None;
        let mut name = None;
        for line in text.lines() {
            let (k, v) = line.split_once('=')?;
            match k {
                "id" => id = Some(v.to_string()),
                "name" => name = Some(v.to_string()),
                _ => {}
            }
        }
        Some(LocalIdentity { id: id?, name: name? })
    }

    pub fn save(&self) {
        let dir = config_dir();
        if std::fs::create_dir_all(&dir).is_ok() {
            let text = format!("id={}\nname={}\n", self.id, self.name);
            let _ = std::fs::write(path(), text);
        }
    }

    /// True on a brand-new install — no saved file yet — so the caller can auto-open the setup wizard.
    pub fn is_first_run() -> bool {
        !path().exists()
    }
}

fn whoami_fallback() -> String {
    std::env::var("USER").unwrap_or_else(|_| "player".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_id_and_name() {
        let me = LocalIdentity { id: "abc123".into(), name: "Kessler".into() };
        let text = format!("id={}\nname={}\n", me.id, me.name);
        assert_eq!(LocalIdentity::parse(&text), Some(me));
    }

    #[test]
    fn parse_rejects_missing_fields() {
        assert_eq!(LocalIdentity::parse("id=abc123\n"), None);
        assert_eq!(LocalIdentity::parse("name=Kessler\n"), None);
        assert_eq!(LocalIdentity::parse(""), None);
    }

    #[test]
    fn generated_ids_are_ten_lowercase_hex_chars_and_not_constant() {
        let a = gen_id();
        let b = gen_id();
        assert_eq!(a.len(), 10);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, b, "two calls should not mint the same id");
    }
}
