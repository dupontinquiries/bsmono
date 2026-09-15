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

    #[test]
    fn key_falls_back_to_a_sanitized_label_model_slug_without_a_uuid() {
        // The ONLY path the existing test exercised was fs_uuid present.
        // The fallback -- used whenever a device has no UUID at all,
        // which real-world exFAT/NTFS-formatted drives commonly don't --
        // had zero coverage, including the exact sanitization rule
        // (non-alphanumeric-or-hyphen characters become '_').
        let id = DeviceIdentity {
            fs_uuid: None,
            label: Some("Red Sticker!".into()),
            model: Some("WD My Passport".into()),
            serial: None,
        };
        assert_eq!(id.key(), "Red_Sticker_-WD_My_Passport");
    }

    #[test]
    fn an_empty_fs_uuid_string_is_treated_as_absent_not_used_verbatim() {
        // `fs_uuid: Some("")` is a real shape lsblk/blkid can hand back
        // (a field present but blank) -- key() explicitly checks
        // `!u.is_empty()` before trusting it, which the existing test
        // (fs_uuid always a real non-empty string) never exercised. A
        // regression dropping that check would key every UUID-less-but-
        // present-empty-string device under the literal string "",
        // colliding every one of them into a single catalog slot.
        let id = DeviceIdentity {
            fs_uuid: Some(String::new()),
            label: Some("X".into()),
            model: None,
            serial: None,
        };
        assert_eq!(id.key(), "X-nomodel", "an empty fs_uuid must fall through to the label/model slug");
    }

    #[test]
    fn two_unlabeled_drives_with_the_same_model_and_no_uuid_collide_on_key() {
        // Real, documented limitation, not a hypothetical: two different
        // physical drives that both lack a filesystem UUID (common on
        // exFAT/NTFS media) AND share the same model string AND were
        // never given a label produce the IDENTICAL key -- meaning
        // save() for the second drive silently overwrites the first
        // drive's entire catalog under save()'s own documented "the
        // store holds the LATEST view per device" rule, with no warning
        // that "per device" was actually "per (label, model) pair" the
        // whole time these two conditions hold. Pinned explicitly as a
        // real gap so it stays visible rather than being rediscovered
        // as a support report; resolving it (e.g. falling back further
        // to a scan-time random id, or refusing to key two active
        // catalogs identically) is real follow-up work, not done here.
        let a = DeviceIdentity { fs_uuid: None, label: None, model: Some("Generic USB Disk".into()), serial: None };
        let b = DeviceIdentity { fs_uuid: None, label: None, model: Some("Generic USB Disk".into()), serial: Some("DIFFERENT-SERIAL".into()) };
        assert_eq!(a.key(), b.key(),
            "KNOWN GAP: serial is not part of the fallback key at all, so two drives \
             differing only by serial number collide -- if this assertion ever fails \
             because key() changed, that likely means someone fixed this; update this \
             test's expectation and comment together, don't just delete it");
    }

    #[test]
    fn display_falls_back_to_no_label_placeholder() {
        // Only the "has both label and model" shape was tested before.
        let id = DeviceIdentity { fs_uuid: None, label: None, model: Some("X1".into()), serial: None };
        assert_eq!(id.display(), "(no label) [X1]");
        let bare = DeviceIdentity { fs_uuid: None, label: None, model: None, serial: None };
        assert_eq!(bare.display(), "(no label)", "no model must not print an empty bracket pair");
    }

    #[test]
    fn scan_does_not_follow_a_symlinked_directory_cycle() {
        // Same real hazard as cleanup.rs's own scanner: a directory
        // containing a symlink back to itself (or an ancestor) makes a
        // naive recursive/stack walk loop forever. scan()'s `is_symlink`
        // check runs before the `is_dir` branch that would push it onto
        // the walk stack, which is what actually prevents this -- a hang
        // here doesn't fail an assertion, it fails by the test never
        // finishing at all.
        #[cfg(unix)]
        {
            let td = tempfile::tempdir().unwrap();
            let root = td.path();
            std::fs::create_dir(root.join("cycle")).unwrap();
            std::os::unix::fs::symlink(root.join("cycle"), root.join("cycle/self")).unwrap();
            std::fs::write(root.join("real.txt"), b"hi").unwrap();
            let entries = scan(root, false).unwrap(); // must return, not hang
            assert_eq!(entries.len(), 1);
            assert!(entries.contains_key("real.txt"));
        }
    }

    #[test]
    fn list_on_a_store_that_does_not_exist_yet_is_an_empty_list_not_an_error() {
        // A brand-new install has no catalogs directory at all yet --
        // the very first run of the app must show an empty list, not an
        // error dialog about a directory that was never expected to
        // exist before the first save().
        let td = tempfile::tempdir().unwrap();
        let never_created = td.path().join("does-not-exist");
        assert!(list(&never_created).is_empty());
    }

    #[test]
    fn load_a_key_with_no_saved_catalog_is_a_real_error() {
        let td = tempfile::tempdir().unwrap();
        let store = td.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        assert!(load(&store, "NEVER-SAVED").is_err());
    }

    #[test]
    fn blank_lines_in_files_jsonl_are_skipped_not_treated_as_a_corrupt_entry() {
        // load()'s own line loop explicitly skips a blank trimmed line
        // before attempting to parse it as an Entry -- protecting
        // against a trailing newline (which nearly every text-writing
        // tool produces) being misread as a corrupt record. Untested:
        // the existing round-trip test's files.jsonl was always written
        // by save() itself, which never happens to leave a genuinely
        // blank line in the middle.
        let td = tempfile::tempdir().unwrap();
        let store = td.path().join("store");
        let dir = store.join("KEY");
        std::fs::create_dir_all(&dir).unwrap();
        let meta = CatalogMeta {
            identity: DeviceIdentity { fs_uuid: Some("KEY".into()), label: None, model: None, serial: None },
            taken_at: 0, mount_root: String::new(), photo_file: None, entry_count: 1, total_bytes: 5,
        };
        std::fs::write(dir.join("meta.json"), serde_json::to_string(&meta).unwrap()).unwrap();
        let entry = Entry { path: "a.txt".into(), size: 5, mtime_unix: 0, b3: String::new() };
        std::fs::write(
            dir.join("files.jsonl"),
            format!("{}\n\n\n", serde_json::to_string(&entry).unwrap()),
        ).unwrap();
        let cat = load(&store, "KEY").unwrap();
        assert_eq!(cat.entries.len(), 1, "blank lines must not become bogus entries or a parse error");
    }
}
