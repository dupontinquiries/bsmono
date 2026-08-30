//! # catalog — remember what lives on drives that aren't plugged in
//!
//! The operator's real problem: a drawer of external NVMe drives and no
//! way, on a phone, to answer "which drive has the 2023 photo dump?"
//! without plugging each one in. A catalog is a persisted snapshot of a
//! mounted filesystem — device identity, when it was taken, every file's
//! path/size/mtime (content hashes optional; a full-drive BLAKE3 pass is
//! an explicit opt-in, not the default, because phone-attached NVMe over
//! USB is where scan time hurts most) — plus an operator-supplied
//! PROFILE PHOTO of the physical drive, so "plug in the one with the red
//! sticker" is something the app can literally show.
//!
//! Storage layout, one directory per device under the store root
//! (default `$XDG_DATA_HOME/drivemind/catalogs`):
//!   <device-key>/meta.json    — identity, timestamps, photo name, totals
//!   <device-key>/files.jsonl  — one integrity::Entry per line (b3 may be
//!                               empty when unhashed)
//!   <device-key>/photo.<ext>  — copied in by `set_photo`
//!
//! The device key is the filesystem UUID when available (stable across
//! mounts and machines), else a slug of label+model. Everything here is
//! pure std + serde; no daemon, no database — files an operator can back
//! up, inspect, or delete with any tool.

use crate::integrity::Entry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeviceIdentity {
    /// Filesystem UUID (blkid/lsblk); the preferred stable key.
    pub fs_uuid: Option<String>,
    pub label: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
}

impl DeviceIdentity {
    /// Stable directory key for the store.
    pub fn key(&self) -> String {
        if let Some(u) = &self.fs_uuid {
            if !u.is_empty() {
                return u.clone();
            }
        }
        let raw = format!(
            "{}-{}",
            self.label.as_deref().unwrap_or("nolabel"),
            self.model.as_deref().unwrap_or("nomodel")
        );
        raw.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
            .collect()
    }

    /// Human one-liner for lists.
    pub fn display(&self) -> String {
        let label = self.label.as_deref().unwrap_or("(no label)");
        match &self.model {
            Some(m) => format!("{label} [{m}]"),
            None => label.to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogMeta {
    pub identity: DeviceIdentity,
    /// Unix seconds when the snapshot was taken.
    pub taken_at: i64,
    /// Where it was mounted when scanned (informational).
    pub mount_root: String,
    /// File name of the profile photo inside the catalog dir, if set.
    pub photo_file: Option<String>,
    pub entry_count: u64,
    pub total_bytes: u64,
}

pub struct Catalog {
    pub meta: CatalogMeta,
    /// rel_path → Entry (b3 empty when the scan didn't hash).
    pub entries: BTreeMap<String, Entry>,
}

/// Default store root. Honors XDG_DATA_HOME; falls back to ~/.local/share.
pub fn default_store() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_DATA_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("drivemind").join("catalogs");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("drivemind")
        .join("catalogs")
}

/// Walk `root` and produce catalog entries. `hash` = compute BLAKE3 per
/// file (slow, opt-in); otherwise b3 is left empty. Symlinks are
/// recorded as themselves (not followed) to keep the walk loop-safe;
/// unreadable subtrees are skipped, not fatal — a half-catalog of a
/// flaky drive still answers "what's on it".
pub fn scan(root: &Path, hash: bool) -> Result<BTreeMap<String, Entry>, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for ent in rd.flatten() {
            let p = ent.path();
            let ft = match ent.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(p);
                continue;
            }
            let md = match ent.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let rel = match p.strip_prefix(root) {
                Ok(r) => r.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let b3 = if hash {
                match crate::integrity::hash_file(&p) {
                    Ok((_, h)) => h,
                    Err(_) => String::new(),
                }
            } else {
                String::new()
            };
            out.insert(
                rel.clone(),
                Entry { path: rel, size: md.len(), mtime_unix: mtime, b3 },
            );
        }
    }
    Ok(out)
}

/// Persist a catalog into the store (overwrites any previous snapshot of
/// the same device — the store holds the LATEST view per device; history
/// belongs to backups of the store itself).
pub fn save(store: &Path, cat: &Catalog) -> Result<PathBuf, String> {
    let dir = store.join(cat.meta.identity.key());
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let meta_path = dir.join("meta.json");
    let f = std::fs::File::create(&meta_path).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(f, &cat.meta).map_err(|e| e.to_string())?;
    let files_path = dir.join("files.jsonl");
    let mut w = std::io::BufWriter::new(std::fs::File::create(&files_path).map_err(|e| e.to_string())?);
    for e in cat.entries.values() {
        writeln!(w, "{}", serde_json::to_string(e).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    Ok(dir)
}

pub fn load(store: &Path, key: &str) -> Result<Catalog, String> {
    let dir = store.join(key);
    let meta: CatalogMeta = serde_json::from_reader(
        std::fs::File::open(dir.join("meta.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("meta.json: {e}"))?;
    let f = std::fs::File::open(dir.join("files.jsonl")).map_err(|e| e.to_string())?;
    let mut entries = BTreeMap::new();
    for line in std::io::BufReader::new(f).lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let e: Entry = serde_json::from_str(&line).map_err(|e| format!("files.jsonl: {e}"))?;
        entries.insert(e.path.clone(), e);
    }
    Ok(Catalog { meta, entries })
}

/// All catalogs in the store (metas only — cheap; load() per key for
/// entries).
pub fn list(store: &Path) -> Vec<CatalogMeta> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(store) else {
        return out;
    };
    for ent in rd.flatten() {
        let mp = ent.path().join("meta.json");
        if let Ok(f) = std::fs::File::open(&mp) {
            if let Ok(m) = serde_json::from_reader::<_, CatalogMeta>(f) {
                out.push(m);
            }
        }
    }
    out.sort_by(|a, b| a.identity.key().cmp(&b.identity.key()));
    out
}

/// Attach/replace the profile photo: copies the image into the catalog
/// dir (extension preserved) and records it in meta.json. The operator
/// photographs the PHYSICAL drive — sticker, enclosure, scratches — so
/// finding which one to plug in stops being archaeology.
pub fn set_photo(store: &Path, key: &str, photo: &Path) -> Result<PathBuf, String> {
    let dir = store.join(key);
    let mut meta: CatalogMeta = serde_json::from_reader(
        std::fs::File::open(dir.join("meta.json")).map_err(|e| format!("no catalog {key}: {e}"))?,
    )
    .map_err(|e| e.to_string())?;
    let ext = photo
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_else(|| "jpg".into());
    let dest_name = format!("photo.{ext}");
    let dest = dir.join(&dest_name);
    std::fs::copy(photo, &dest).map_err(|e| format!("copy photo: {e}"))?;
    meta.photo_file = Some(dest_name);
    let f = std::fs::File::create(dir.join("meta.json")).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(f, &meta).map_err(|e| e.to_string())?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_tree(root: &Path) {
        std::fs::create_dir_all(root.join("docs/tax")).unwrap();
        std::fs::write(root.join("docs/a.txt"), b"hello").unwrap();
        std::fs::write(root.join("docs/tax/2023.pdf"), vec![7u8; 1000]).unwrap();
        std::fs::write(root.join("top.bin"), vec![1u8; 10]).unwrap();
    }

    #[test]
    fn scan_save_load_roundtrip_with_photo() {
        let td = tempfile::tempdir().unwrap();
        let mount = td.path().join("mnt");
        mk_tree(&mount);
        let store = td.path().join("store");
        let entries = scan(&mount, false).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries["docs/tax/2023.pdf"].size, 1000);
        assert!(entries["docs/a.txt"].b3.is_empty(), "no hashing by default");
        let cat = Catalog {
            meta: CatalogMeta {
                identity: DeviceIdentity {
                    fs_uuid: Some("ABCD-1234".into()),
                    label: Some("RedSticker".into()),
                    model: Some("Samsung T7".into()),
                    serial: None,
                },
                taken_at: 1_700_000_000,
                mount_root: mount.display().to_string(),
                photo_file: None,
                entry_count: entries.len() as u64,
                total_bytes: entries.values().map(|e| e.size).sum(),
            },
            entries,
        };
        save(&store, &cat).unwrap();
        // photo attach
        let img = td.path().join("drive.jpg");
        std::fs::write(&img, b"\xFF\xD8fakejpeg").unwrap();
        set_photo(&store, "ABCD-1234", &img).unwrap();

        let back = load(&store, "ABCD-1234").unwrap();
        assert_eq!(back.entries.len(), 3);
        assert_eq!(back.meta.total_bytes, 1015);
        assert_eq!(back.meta.photo_file.as_deref(), Some("photo.jpg"));
        let metas = list(&store);
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].identity.display(), "RedSticker [Samsung T7]");
    }
}
