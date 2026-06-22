//! Per-account credential store for the w2q mail bridge.
//!
//! v0.1.861: This replaces hydroxide's single `auth.json` — a JSON map of
//! `username -> encrypted-blob`, all guarded by one process-wide bridge
//! password — with one isolated, self-framed file per account under
//! `~/.config/web2qml/mail/accounts/<id>.acct`. The motivations:
//!
//!   * **Isolation.** A corrupt or partially-written record can only ever
//!     lose one account, and each record is sealed under *its own* bridge
//!     key. There is no shared secret whose compromise unlocks everyone.
//!   * **Multi-user from the ground up.** Adding or removing an account is
//!     a single file create/unlink; no read-modify-write of a shared map
//!     and no cross-account locking.
//!   * **Testability.** The record <-> bytes serialization is pure and
//!     covered by unit tests; only the final seal/open step touches the
//!     injected [`SecretBox`], so the store's framing logic is verifiable
//!     with the crypto feature off.
//!
//! The sensitive material (login + mailbox passwords, the live `Auth`
//! tokens, the key salts) lives *inside* the sealed body, exactly as
//! hydroxide's `CachedAuth` does. The bridge password that unseals it is
//! never written to disk — it is handed to the user once at login and
//! presented again by their SMTP/IMAP/CardDAV client.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::mail_crypto::{b64_decode, b64_encode, SecretBox};
use crate::mail_error::{MailError, Result};
use crate::mail_json::{Json, Obj};
use crate::mail_proton::{Auth, PasswordModeRepr};

/// Stable, typed handle for one account. Wraps Proton's user id (or, before
/// the first successful unlock, the normalized login name). Using a newtype
/// instead of bare `String` keys means the compiler stops us from mixing an
/// account id up with an address id, a message id, or a raw username — a
/// class of bug that stringly-typed maps invite.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(String);

impl AccountId {
    pub fn new(s: impl Into<String>) -> Self {
        AccountId(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// File-system-safe slug for the on-disk filename. Proton ids are
    /// already `[A-Za-z0-9_-]`, but a login-derived id might not be, so we
    /// sanitize defensively.
    fn slug(&self) -> String {
        self.0
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect()
    }
}

impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Everything needed to resurrect a session without a fresh interactive
/// login: the live token set, the passwords required to re-run SRP when the
/// refresh token finally dies, the key salts, and a little w2q-side
/// presentation metadata. This is the Rust analogue of hydroxide's
/// `CachedAuth`, plus the UI niceties w2q wants.
#[derive(Clone, Debug)]
pub struct AccountRecord {
    pub id: AccountId,
    pub username: String,
    pub auth: Auth,
    /// The account (login) password. Needed to re-derive SRP proofs on a
    /// full re-auth. Stored only inside the sealed body.
    pub login_password: Vec<u8>,
    /// The mailbox password (two-password mode) or a copy of the login
    /// password (single-password mode). Needed to unlock the PGP keyring.
    pub mailbox_password: Vec<u8>,
    pub key_salts: HashMap<String, Vec<u8>>,
    // --- w2q presentation extras ---
    pub display_name: String,
    pub color: String,
    pub created_at: u64,
    pub updated_at: u64,
}

impl AccountRecord {
    /// Serialize to the plaintext JSON that will be sealed. Pure and
    /// reversible — see [`AccountRecord::from_json_bytes`].
    pub fn to_json_bytes(&self) -> Vec<u8> {
        let salts_obj = {
            let mut o = Obj::new();
            for (k, v) in &self.key_salts {
                o = o.str(k, &b64_encode(v));
            }
            o.build()
        };
        let auth_json = auth_to_json(&self.auth);
        let body = Obj::new()
            .str("Id", self.id.as_str())
            .str("Username", &self.username)
            .raw("Auth", &auth_json)
            .str("LoginPassword", &b64_encode(&self.login_password))
            .str("MailboxPassword", &b64_encode(&self.mailbox_password))
            .raw("KeySalts", &salts_obj)
            .str("DisplayName", &self.display_name)
            .str("Color", &self.color)
            .int("CreatedAt", self.created_at as i64)
            .int("UpdatedAt", self.updated_at as i64)
            .build();
        body.into_bytes()
    }

    /// Parse the plaintext JSON produced by [`AccountRecord::to_json_bytes`].
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| MailError::Decode("account record is not valid UTF-8".into()))?;
        let j = Json::parse(text).map_err(MailError::Decode)?;

        let id = AccountId::new(j.str_field("Id").unwrap_or_default());
        let mut salts = HashMap::new();
        if let Some(obj) = j.get("KeySalts") {
            if let Json::Obj(map) = obj {
                for (k, v) in map {
                    if let Some(s) = v.as_str() {
                        salts.insert(k.clone(), b64_decode(s)?);
                    }
                }
            }
        }

        Ok(AccountRecord {
            id,
            username: j.str_field("Username").unwrap_or_default(),
            auth: auth_from_json(j.get("Auth").unwrap_or(&Json::Null)),
            login_password: decode_b64_field(&j, "LoginPassword")?,
            mailbox_password: decode_b64_field(&j, "MailboxPassword")?,
            key_salts: salts,
            display_name: j.str_field("DisplayName").unwrap_or_default(),
            color: j.str_field("Color").unwrap_or_default(),
            created_at: j.int_field("CreatedAt").unwrap_or(0) as u64,
            updated_at: j.int_field("UpdatedAt").unwrap_or(0) as u64,
        })
    }

    /// Lightweight, non-secret view for listing accounts in a UI.
    pub fn summary(&self) -> AccountSummary {
        AccountSummary {
            id: self.id.clone(),
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            color: self.color.clone(),
            updated_at: self.updated_at,
        }
    }
}

/// Non-secret projection of an [`AccountRecord`] safe to hand to the UI
/// layer (or expose over QML) without ever surfacing tokens or passwords.
#[derive(Clone, Debug)]
pub struct AccountSummary {
    pub id: AccountId,
    pub username: String,
    pub display_name: String,
    pub color: String,
    pub updated_at: u64,
}

// ---------------------------------------------------------------------
// On-disk framing
//
// Each file is:  "W2QM" (4) ++ version (1) ++ sealed-body
// where sealed-body is SecretBox::seal(bridge_key, record.to_json_bytes()).
// The header is plaintext so a future migration can detect the format
// without a key. The seal is self-framed (nonce ++ ct ++ tag), so we do
// not store a nonce ourselves.
// ---------------------------------------------------------------------

const MAGIC: &[u8; 4] = b"W2QM";
const FORMAT_VERSION: u8 = 1;

/// Frame an already-sealed body. Pure; tested without crypto.
fn frame(sealed: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + sealed.len());
    out.extend_from_slice(MAGIC);
    out.push(FORMAT_VERSION);
    out.extend_from_slice(sealed);
    out
}

/// Inverse of [`frame`]: validate the header, return the sealed body slice.
fn unframe(raw: &[u8]) -> Result<&[u8]> {
    if raw.len() < 5 || &raw[0..4] != MAGIC {
        return Err(MailError::Store("not a w2q account file".into()));
    }
    if raw[4] != FORMAT_VERSION {
        return Err(MailError::Store(format!(
            "unsupported account file version {}",
            raw[4]
        )));
    }
    Ok(&raw[5..])
}

/// Filesystem-backed, per-account credential store.
///
/// The store owns the directory and the [`SecretBox`] used to seal/open
/// records; it does *not* own bridge keys — those are supplied per call by
/// whoever is authenticating, mirroring how an IMAP client presents its
/// password on every connection.
pub struct AccountStore {
    dir: PathBuf,
    secret_box: Box<dyn SecretBox>,
}

impl AccountStore {
    /// Open (creating if needed) the store at the default location,
    /// `~/.config/web2qml/mail/accounts/`.
    pub fn open_default(secret_box: Box<dyn SecretBox>) -> Result<Self> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let dir = PathBuf::from(home)
            .join(".config/web2qml")
            .join("mail")
            .join("accounts");
        Self::open_at(dir, secret_box)
    }

    /// Open (creating if needed) the store at an explicit directory. Useful
    /// for tests and for callers that relocate the w2q config root.
    pub fn open_at(dir: impl Into<PathBuf>, secret_box: Box<dyn SecretBox>) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|e| MailError::Store(e.to_string()))?;
        Ok(AccountStore { dir, secret_box })
    }

    fn path_for(&self, id: &AccountId) -> PathBuf {
        self.dir.join(format!("{}.acct", id.slug()))
    }

    /// Seal and write a record under `bridge_key`. Atomic-ish: we write a
    /// temp file and rename, so a crash mid-write cannot truncate a good
    /// record.
    pub fn save(&self, record: &AccountRecord, bridge_key: &[u8; 32]) -> Result<()> {
        let sealed = self.secret_box.seal(bridge_key, &record.to_json_bytes())?;
        let framed = frame(&sealed);
        let final_path = self.path_for(&record.id);
        let tmp_path = final_path.with_extension("acct.tmp");
        std::fs::write(&tmp_path, &framed).map_err(|e| MailError::Store(e.to_string()))?;
        std::fs::rename(&tmp_path, &final_path).map_err(|e| MailError::Store(e.to_string()))?;
        Ok(())
    }

    /// Read and open the record for `id` using `bridge_key`. A wrong key
    /// surfaces as [`MailError::Unauthorized`] from the [`SecretBox`].
    pub fn load(&self, id: &AccountId, bridge_key: &[u8; 32]) -> Result<AccountRecord> {
        let raw = std::fs::read(self.path_for(id)).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => MailError::Store(format!("no such account: {id}")),
            _ => MailError::Store(e.to_string()),
        })?;
        let sealed = unframe(&raw)?;
        let plaintext = self.secret_box.open(bridge_key, sealed)?;
        AccountRecord::from_json_bytes(&plaintext)
    }

    /// List the account ids present on disk (no decryption — just the
    /// filenames). The order is filesystem-defined, then sorted for
    /// determinism.
    pub fn list_ids(&self) -> Result<Vec<AccountId>> {
        let mut ids = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ids),
            Err(e) => return Err(MailError::Store(e.to_string())),
        };
        for entry in entries {
            let entry = entry.map_err(|e| MailError::Store(e.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("acct") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    ids.push(AccountId::new(stem.to_string()));
                }
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// Permanently delete an account file. Idempotent: removing an absent
    /// account is not an error.
    pub fn remove(&self, id: &AccountId) -> Result<()> {
        match std::fs::remove_file(self.path_for(id)) {
            Ok(()) => Ok(()),
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(MailError::Store(e.to_string())),
        }
    }

    /// Does a file exist for this id? (No key required.)
    pub fn exists(&self, id: &AccountId) -> bool {
        self.path_for(id).exists()
    }

    #[doc(hidden)]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    // --- storage management ----------------------------------------------

    /// Total bytes used by all account files in the store directory.
    pub fn disk_usage(&self) -> u64 {
        self.account_files()
            .into_iter()
            .map(|(_, bytes)| bytes)
            .sum()
    }

    /// Per-account on-disk sizes (id, bytes), sorted by id. No decryption.
    pub fn account_files(&self) -> Vec<(AccountId, u64)> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("acct") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                        out.push((AccountId::new(stem.to_string()), size));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Permanently delete *every* account file (and the machine key is left
    /// untouched — callers that want a full wipe also remove
    /// `mail_secure::MachineKey::path`). Returns the number of files removed.
    pub fn wipe_all(&self) -> Result<usize> {
        let mut n = 0;
        for (id, _) in self.account_files() {
            self.remove(&id)?;
            n += 1;
        }
        Ok(n)
    }
}

/// Generate a fresh 32-byte bridge key and its base64 presentation, the way
/// hydroxide's `GeneratePassword` does. The base64 string is what the user
/// pastes into their mail client; the raw bytes are what seals the record.
/// The two are kept together so a caller can't accidentally seal under the
/// bytes of one key while handing out the text of another.
pub fn new_bridge_password() -> Result<BridgePassword> {
    let key = random_32()?;
    let text = b64_encode(&key);
    Ok(BridgePassword { key, text })
}

/// Reconstruct the raw key from the base64 a mail client presented.
pub fn bridge_key_from_text(text: &str) -> Result<[u8; 32]> {
    let bytes = b64_decode(text.trim())?;
    if bytes.len() != 32 {
        return Err(MailError::Unauthorized);
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// A freshly minted bridge credential: raw sealing key plus the base64 text
/// shown to the user once.
#[derive(Clone)]
pub struct BridgePassword {
    pub key: [u8; 32],
    pub text: String,
}

/// 32 cryptographically-random bytes from the OS CSPRNG. We read
/// `/dev/urandom` directly to avoid pulling in a `rand` dependency the w2q
/// tree doesn't carry; this is the same source `getrandom` uses on Linux.
fn random_32() -> Result<[u8; 32]> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom")
        .map_err(|e| MailError::Store(format!("CSPRNG unavailable: {e}")))?;
    let mut buf = [0u8; 32];
    f.read_exact(&mut buf)
        .map_err(|e| MailError::Store(format!("CSPRNG read failed: {e}")))?;
    Ok(buf)
}

// ---------------------------------------------------------------------
// Auth <-> JSON (kept here, next to the record, since Auth's on-disk shape
// is a storage concern rather than a transport one)
// ---------------------------------------------------------------------

fn password_mode_to_int(m: PasswordModeRepr) -> i64 {
    match m {
        PasswordModeRepr::Single => 1,
        PasswordModeRepr::Two => 2,
        PasswordModeRepr::Unknown(n) => n,
    }
}

fn password_mode_from_int(n: i64) -> PasswordModeRepr {
    match n {
        1 => PasswordModeRepr::Single,
        2 => PasswordModeRepr::Two,
        other => PasswordModeRepr::Unknown(other),
    }
}

fn auth_to_json(a: &Auth) -> String {
    let expires = a
        .expires_at
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Obj::new()
        .str("UID", &a.uid)
        .str("AccessToken", &a.access_token)
        .str("RefreshToken", &a.refresh_token)
        .str("UserID", &a.user_id)
        .str("EventID", &a.event_id)
        .str("Scope", &a.scope)
        .int("PasswordMode", password_mode_to_int(a.password_mode))
        .bool("TwoFactorEnabled", a.two_factor_enabled)
        .bool("Totp", a.totp)
        .int("ExpiresAt", expires)
        .build()
}

fn auth_from_json(j: &Json) -> Auth {
    let expires = j.int_field("ExpiresAt").unwrap_or(0);
    let expires_at = if expires > 0 {
        Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(expires as u64))
    } else {
        None
    };
    Auth {
        uid: j.str_field("UID").unwrap_or_default(),
        access_token: j.str_field("AccessToken").unwrap_or_default(),
        refresh_token: j.str_field("RefreshToken").unwrap_or_default(),
        user_id: j.str_field("UserID").unwrap_or_default(),
        event_id: j.str_field("EventID").unwrap_or_default(),
        scope: j.str_field("Scope").unwrap_or_default(),
        password_mode: password_mode_from_int(j.int_field("PasswordMode").unwrap_or(1)),
        two_factor_enabled: j.get("TwoFactorEnabled").and_then(Json::as_bool).unwrap_or(false),
        totp: j.get("Totp").and_then(Json::as_bool).unwrap_or(false),
        expires_at,
    }
}

fn decode_b64_field(j: &Json, key: &str) -> Result<Vec<u8>> {
    match j.str_field(key) {
        Some(s) if !s.is_empty() => b64_decode(&s),
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail_proton::PasswordModeRepr;

    fn sample_record() -> AccountRecord {
        let mut salts = HashMap::new();
        salts.insert("keyid-1".to_string(), vec![1, 2, 3, 4]);
        salts.insert("keyid-2".to_string(), vec![]);
        AccountRecord {
            id: AccountId::new("user-abc"),
            username: "alice@proton.me".into(),
            auth: Auth {
                uid: "uid123".into(),
                access_token: "atok".into(),
                refresh_token: "rtok".into(),
                user_id: "user-abc".into(),
                event_id: "evt0".into(),
                scope: "full".into(),
                password_mode: PasswordModeRepr::Two,
                two_factor_enabled: true,
                totp: true,
                expires_at: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)),
            },
            login_password: b"login-pw".to_vec(),
            mailbox_password: b"mbox-pw".to_vec(),
            key_salts: salts,
            display_name: "Alice".into(),
            color: "#33aaff".into(),
            created_at: 1_600_000_000,
            updated_at: 1_700_000_000,
        }
    }

    #[test]
    fn record_roundtrips_through_json() {
        let rec = sample_record();
        let bytes = rec.to_json_bytes();
        let back = AccountRecord::from_json_bytes(&bytes).unwrap();
        assert_eq!(back.id, rec.id);
        assert_eq!(back.username, rec.username);
        assert_eq!(back.auth.uid, rec.auth.uid);
        assert_eq!(back.auth.refresh_token, rec.auth.refresh_token);
        assert_eq!(back.auth.password_mode, PasswordModeRepr::Two);
        assert!(back.auth.two_factor_enabled);
        assert_eq!(back.login_password, b"login-pw");
        assert_eq!(back.mailbox_password, b"mbox-pw");
        assert_eq!(back.key_salts.get("keyid-1").unwrap(), &vec![1, 2, 3, 4]);
        assert_eq!(back.key_salts.get("keyid-2").unwrap(), &Vec::<u8>::new());
        assert_eq!(back.display_name, "Alice");
        assert_eq!(back.updated_at, 1_700_000_000);
    }

    #[test]
    fn frame_unframe_roundtrip() {
        let body = b"sealed-bytes-here";
        let framed = frame(body);
        assert_eq!(&framed[0..4], MAGIC);
        assert_eq!(framed[4], FORMAT_VERSION);
        assert_eq!(unframe(&framed).unwrap(), body);
    }

    #[test]
    fn unframe_rejects_foreign_files() {
        assert!(unframe(b"XXXX\x01body").is_err());
        assert!(unframe(b"W2QM\x09body").is_err()); // bad version
        assert!(unframe(b"W2").is_err()); // too short
    }

    #[test]
    fn bridge_key_text_roundtrips() {
        let bp = new_bridge_password().unwrap();
        let key2 = bridge_key_from_text(&bp.text).unwrap();
        assert_eq!(bp.key, key2);
        assert_eq!(bp.key.len(), 32);
    }

    #[test]
    fn bridge_key_rejects_wrong_length() {
        assert!(bridge_key_from_text("AAAA").is_err());
    }

    #[test]
    fn store_disk_usage_and_wipe() {
        struct RawBox;
        impl SecretBox for RawBox {
            fn seal(&self, _k: &[u8; 32], p: &[u8]) -> Result<Vec<u8>> {
                Ok(p.to_vec())
            }
            fn open(&self, _k: &[u8; 32], s: &[u8]) -> Result<Vec<u8>> {
                Ok(s.to_vec())
            }
        }
        let dir = std::env::temp_dir().join(format!("w2q_store_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = AccountStore::open_at(&dir, Box::new(RawBox)).unwrap();
        let key = [1u8; 32];
        let mut rec = sample_record();
        store.save(&rec, &key).unwrap();
        rec.id = AccountId::new("user-def");
        store.save(&rec, &key).unwrap();

        assert_eq!(store.list_ids().unwrap().len(), 2);
        assert!(store.disk_usage() > 0);
        assert_eq!(store.account_files().len(), 2);
        assert_eq!(store.wipe_all().unwrap(), 2);
        assert_eq!(store.list_ids().unwrap().len(), 0);
        assert_eq!(store.disk_usage(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
