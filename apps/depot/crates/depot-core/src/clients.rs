//! Paired clients and pending pairing requests, as plain files, so the
//! server and the CLI (another process: `depot pair approve`) share them
//! without a database:
//!
//! ```text
//! keys/clients/<name>.pub   a paired client: public key, scopes (depot-auth)
//! keys/pending/<id>         a pairing request waiting for approval
//! keys/paired/<id>          the outcome, for the client polling its request
//! ```
//! The protocol (codes, signatures, scopes) is `shared/depot-auth`; this
//! module only stores it.

use depot_auth::{ClientKey, PairRequest, Scope};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// More pending requests than this and new ones are refused: pairing is
/// open (no auth), so the queue must be bounded.
pub const MAX_PENDING: usize = 16;

fn clients_dir(root: &Path) -> PathBuf {
    root.join("keys/clients")
}
fn pending_dir(root: &Path) -> PathBuf {
    root.join("keys/pending")
}
fn outcome_dir(root: &Path) -> PathBuf {
    root.join("keys/paired")
}

fn random_hex(n: usize) -> String {
    use std::io::Read;
    let mut b = vec![0u8; n];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
        // Not a secret (ids and code nonces); a clock fallback is enough.
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        b.iter_mut().enumerate().for_each(|(i, x)| *x = (t >> ((i % 16) * 8)) as u8 ^ i as u8);
    }
    depot_auth::hex(&b)
}

fn write_private(p: &Path, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("tmp");
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(text.as_bytes())?;
    }
    std::fs::rename(tmp, p)
}

pub fn load(root: &Path, name: &str) -> Option<ClientKey> {
    if !depot_auth::valid_name(name) {
        return None;
    }
    let text = std::fs::read_to_string(clients_dir(root).join(format!("{name}.pub"))).ok()?;
    ClientKey::from_file(name, &text)
}

pub fn list(root: &Path) -> Vec<ClientKey> {
    let mut out: Vec<ClientKey> = std::fs::read_dir(clients_dir(root))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let name = n.strip_suffix(".pub")?.to_string();
            load(root, &name)
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn revoke(root: &Path, name: &str) -> Result<(), String> {
    if !depot_auth::valid_name(name) {
        return Err(format!("bad client name '{name}'"));
    }
    std::fs::remove_file(clients_dir(root).join(format!("{name}.pub"))).map_err(|e| format!("{name}: {e}"))
}

/// Pending requests, oldest first; expired ones are removed on the way.
pub fn pending(root: &Path, now: u64) -> Vec<PairRequest> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(pending_dir(root)).into_iter().flatten().flatten() {
        let id = e.file_name().to_string_lossy().into_owned();
        let Some(r) = std::fs::read_to_string(e.path()).ok().and_then(|t| PairRequest::from_file(&id, &t)) else { continue };
        if r.expired(now) {
            let _ = std::fs::remove_file(e.path());
            continue;
        }
        out.push(r);
    }
    out.sort_by_key(|r| r.created);
    out
}

/// File a request. The name must be free (re-pairing a name means revoking
/// it first, deliberately).
pub fn request(root: &Path, name: &str, public_hex: &str, scopes: &str, now: u64) -> Result<PairRequest, String> {
    if !depot_auth::valid_name(name) {
        return Err(format!("client name '{name}': letters, digits, - _ . only, up to 48"));
    }
    let public = depot_auth::unhex::<32>(public_hex).ok_or("public: 64 hex digits (an ed25519 key)")?;
    let scopes: BTreeSet<Scope> = Scope::parse_list(scopes)?;
    if scopes.is_empty() {
        return Err("ask for at least one scope (ingest, packs, approve, term)".into());
    }
    if load(root, name).is_some() {
        return Err(format!("'{name}' is already paired; `depot pair revoke {name}` first to replace its key"));
    }
    if pending(root, now).len() >= MAX_PENDING {
        return Err(format!("{MAX_PENDING} pairing requests already waiting; approve or deny some first"));
    }
    let r = PairRequest { id: random_hex(6), name: name.into(), public, scopes, nonce: random_hex(8), created: now };
    write_private(&pending_dir(root).join(&r.id), &r.to_file()).map_err(|e| e.to_string())?;
    Ok(r)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Pending,
    Approved(String),
    Denied,
    Unknown,
}

pub fn status(root: &Path, id: &str, now: u64) -> Status {
    if !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Status::Unknown;
    }
    if let Ok(t) = std::fs::read_to_string(outcome_dir(root).join(id)) {
        return match t.split_once(' ') {
            Some(("approved", name)) => Status::Approved(name.trim().to_string()),
            _ => Status::Denied,
        };
    }
    if pending(root, now).iter().any(|r| r.id == id) {
        Status::Pending
    } else {
        Status::Unknown
    }
}

pub fn approve(root: &Path, id: &str, code: &str, now: u64) -> Result<ClientKey, String> {
    let r = pending(root, now).into_iter().find(|r| r.id == id).ok_or_else(|| format!("no pending request '{id}' (expired, or already decided)"))?;
    let key = r.approve(code, now)?;
    if load(root, &key.name).is_some() {
        return Err(format!("'{}' was paired meanwhile", key.name));
    }
    write_private(&clients_dir(root).join(format!("{}.pub", key.name)), &key.to_file()).map_err(|e| e.to_string())?;
    let _ = write_private(&outcome_dir(root).join(id), &format!("approved {}\n", key.name));
    let _ = std::fs::remove_file(pending_dir(root).join(id));
    Ok(key)
}

pub fn deny(root: &Path, id: &str) -> Result<(), String> {
    if !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("bad id".into());
    }
    std::fs::remove_file(pending_dir(root).join(id)).map_err(|e| format!("{id}: {e}"))?;
    let _ = write_private(&outcome_dir(root).join(id), "denied\n");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use depot_auth::Identity;

    #[test]
    fn request_approve_status_revoke() {
        let root = std::env::temp_dir().join(format!("depot-clients-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let id = Identity::generate("gitmind-egui");
        let r = request(&root, "gitmind-egui", &id.public_hex(), "ingest,approve", 1000).unwrap();
        assert_eq!(status(&root, &r.id, 1001), Status::Pending);
        assert!(request(&root, "../evil", &id.public_hex(), "ingest", 1000).is_err());
        assert!(request(&root, "x", "abcd", "ingest", 1000).is_err());
        assert!(request(&root, "x", &id.public_hex(), "root", 1000).unwrap_err().contains("unknown scope"));
        // The client computes the code itself from its key and the nonce.
        let code = depot_auth::pair_code(&id.public(), &r.nonce);
        let wrong = if code == "000000" { "000001" } else { "000000" };
        assert!(approve(&root, &r.id, wrong, 1002).unwrap_err().contains("does not match"));
        let k = approve(&root, &r.id, &code, 1002).unwrap();
        assert_eq!(status(&root, &r.id, 1003), Status::Approved("gitmind-egui".into()));
        assert_eq!(load(&root, "gitmind-egui"), Some(k));
        assert!(request(&root, "gitmind-egui", &id.public_hex(), "ingest", 1004).unwrap_err().contains("already paired"));
        let mode = std::fs::metadata(root.join("keys/clients/gitmind-egui.pub")).map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o777).unwrap();
        assert_eq!(mode, 0o600);
        // Expiry, deny, the queue bound.
        let r2 = request(&root, "late", &id.public_hex(), "ingest", 1000).unwrap();
        assert_eq!(status(&root, &r2.id, 1000 + depot_auth::PAIR_TTL_S + 5), Status::Unknown, "expired requests vanish");
        let r3 = request(&root, "no", &id.public_hex(), "ingest", 5000).unwrap();
        deny(&root, &r3.id).unwrap();
        assert_eq!(status(&root, &r3.id, 5001), Status::Denied);
        for i in 0..MAX_PENDING {
            request(&root, &format!("c{i}"), &id.public_hex(), "ingest", 6000).unwrap();
        }
        assert!(request(&root, "one-more", &id.public_hex(), "ingest", 6000).unwrap_err().contains("already waiting"));
        revoke(&root, "gitmind-egui").unwrap();
        assert!(load(&root, "gitmind-egui").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
