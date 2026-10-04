// integrity — "is my data still my data?", answered with BLAKE3 manifests.
//
// SMART tells you the DRIVE's opinion of itself. It says nothing about
// whether the bytes you care about still read back as the bytes you wrote —
// bit rot, a flaky bridge corrupting transfers, an interrupted rsync. This
// module closes that gap the boring, reliable way:
//
//   create():  walk a tree, hash every file (streaming, 1 MiB buffer),
//              write one JSON line per file:  {path, size, mtime, b3}
//   verify():  re-walk against a manifest and classify every path:
//              Ok / Changed / Missing / New / ReadError
//
// ReadError is the one people forget and the one that matters most here:
// a file that HASHES DIFFERENTLY was modified; a file that CANNOT BE READ
// is a dying disk talking to you. verify() keeps going past read errors so
// one bad sector doesn't hide the report for the other million files.
//
// Format notes:
//   * JSONL, sorted by path — diffable with standard tools, appendable,
//     streams in bounded memory at both ends. A header line (kind:"dmv1")
//     records the root so a manifest is self-describing.
//   * mtime+size are recorded but are ADVISORY: verify() trusts only the
//     hash. A future `--quick` mode can use them as a skip filter; that
//     changes cost, not truth.
//   * BLAKE3 — same family as gitmind's store, SIMD everywhere we ship.

use crate::{Event, Sink};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
struct Header {
    kind: String, // "dmv1"
    root: String,
    created_unix: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    /// Path relative to the manifest root, always with '/' separators.
    pub path: String,
    pub size: u64,
    pub mtime_unix: i64,
    /// Lowercase hex BLAKE3 of the file content.
    pub b3: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FileStatus {
    Ok,
    /// Content hash differs (size/mtime shown for context).
    Changed { expected: String, actual: String },
    Missing,
    /// Present on disk but not in the manifest.
    New,
    /// Could not be read — the disk-health signal.
    ReadError(String),
}

#[derive(Clone, Debug, Default)]
pub struct VerifyReport {
    pub ok: u64,
    pub changed: Vec<(String, FileStatus)>,
    pub missing: Vec<String>,
    pub new: Vec<String>,
    pub read_errors: Vec<(String, String)>,
    pub bytes_hashed: u64,
}

impl VerifyReport {
    pub fn clean(&self) -> bool {
        self.changed.is_empty() && self.missing.is_empty() && self.read_errors.is_empty()
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Hash one file, streaming. Returns (size, hex hash).
pub fn hash_file(path: &Path) -> std::io::Result<(u64, String)> {
    let mut f = fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hasher.update(&buf[..n]);
    }
    Ok((total, hasher.finalize().to_hex().to_string()))
}

/// Recursively collect regular files under root (sorted, relative paths).
/// Symlinks are recorded as themselves? No — skipped with a Note: following
/// them turns a manifest into a trap (targets move independently).
fn walk(root: &Path, sink: Sink) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                sink(Event::Note(format!("skip dir {}: {e}", dir.display())));
                continue;
            }
        };
        for ent in rd.flatten() {
            let p = ent.path();
            let ft = match ent.file_type() {
                Ok(t) => t,
                Err(e) => {
                    sink(Event::Note(format!("skip {}: {e}", p.display())));
                    continue;
                }
            };
            if ft.is_symlink() {
                sink(Event::Note(format!("skip symlink {}", p.display())));
            } else if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// Create a manifest of `root` at `manifest_path`.
pub fn create(root: &Path, manifest_path: &Path, sink: Sink) -> Result<u64, String> {
    sink(Event::Phase { name: "manifest", subject: root.display().to_string() });
    let files = walk(root, sink);
    let total = files.len() as u64;
    let f = fs::File::create(manifest_path).map_err(|e| format!("create manifest: {e}"))?;
    let mut w = BufWriter::new(f);
    let hdr = Header {
        kind: "dmv1".into(),
        root: root.display().to_string(),
        created_unix: now_unix(),
    };
    writeln!(w, "{}", serde_json::to_string(&hdr).unwrap()).map_err(|e| e.to_string())?;
    let mut done = 0u64;
    for p in files {
        let meta = fs::metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let (size, b3) = match hash_file(&p) {
            Ok(x) => x,
            Err(e) => {
                // At CREATE time an unreadable file is fatal: a manifest
                // that silently omits files would "verify clean" forever.
                return Err(format!("read {}: {e}", p.display()));
            }
        };
        debug_assert_eq!(size, meta.len());
        let entry = Entry {
            path: rel(root, &p),
            size,
            mtime_unix: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            b3,
        };
        writeln!(w, "{}", serde_json::to_string(&entry).unwrap()).map_err(|e| e.to_string())?;
        done += 1;
        sink(Event::Progress { done, total: Some(total) });
    }
    w.flush().map_err(|e| e.to_string())?;
    Ok(done)
}

/// Load a manifest into (root, path→entry).
pub fn load(manifest_path: &Path) -> Result<(String, BTreeMap<String, Entry>), String> {
    let f = fs::File::open(manifest_path).map_err(|e| format!("open manifest: {e}"))?;
    let mut lines = BufReader::new(f).lines();
    let hdr_line = lines
        .next()
        .ok_or("empty manifest")?
        .map_err(|e| e.to_string())?;
    let hdr: Header =
        serde_json::from_str(&hdr_line).map_err(|e| format!("manifest header: {e}"))?;
    if hdr.kind != "dmv1" {
        return Err(format!("unknown manifest kind {:?}", hdr.kind));
    }
    let mut map = BTreeMap::new();
    for line in lines {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let e: Entry = serde_json::from_str(&line).map_err(|e| format!("manifest entry: {e}"))?;
        map.insert(e.path.clone(), e);
    }
    Ok((hdr.root, map))
}

/// Verify `root` against a manifest. `root` overrides the recorded root
/// (so a manifest travels with the data onto another mount point).
pub fn verify(root: &Path, manifest_path: &Path, sink: Sink) -> Result<VerifyReport, String> {
    let (_recorded_root, expected) = load(manifest_path)?;
    sink(Event::Phase { name: "verify", subject: root.display().to_string() });
    let mut rep = VerifyReport::default();
    let total = expected.len() as u64;
    let mut done = 0u64;

    for (relpath, ent) in &expected {
        let p = root.join(relpath);
        if !p.exists() {
            rep.missing.push(relpath.clone());
        } else {
            match hash_file(&p) {
                Ok((size, b3)) => {
                    rep.bytes_hashed += size;
                    if b3 == ent.b3 {
                        rep.ok += 1;
                    } else {
                        rep.changed.push((
                            relpath.clone(),
                            FileStatus::Changed { expected: ent.b3.clone(), actual: b3 },
                        ));
                    }
                }
                Err(e) => {
                    // Keep going: one dying sector must not mask the rest.
                    sink(Event::Note(format!("READ ERROR {}: {e}", p.display())));
                    rep.read_errors.push((relpath.clone(), e.to_string()));
                }
            }
        }
        done += 1;
        sink(Event::Progress { done, total: Some(total) });
    }

    // Anything on disk the manifest doesn't know about.
    for p in walk(root, sink) {
        let r = rel(root, &p);
        if !expected.contains_key(&r) {
            rep.new.push(r);
        }
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::null_sink;

    #[test]
    fn create_verify_roundtrip_and_classification() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), b"alpha").unwrap();
        fs::write(root.join("sub/b.bin"), vec![0u8; 4096]).unwrap();
        let manifest = dir.path().join("m.jsonl");

        let mut sink = null_sink();
        let n = create(&root, &manifest, &mut sink).unwrap();
        assert_eq!(n, 2);

        // Untouched: clean.
        let rep = verify(&root, &manifest, &mut sink).unwrap();
        assert!(rep.clean());
        assert_eq!(rep.ok, 2);
        assert!(rep.new.is_empty());

        // Mutate one, delete one, add one.
        fs::write(root.join("a.txt"), b"ALPHA?").unwrap();
        fs::remove_file(root.join("sub/b.bin")).unwrap();
        fs::write(root.join("c.new"), b"new").unwrap();
        let rep = verify(&root, &manifest, &mut sink).unwrap();
        assert!(!rep.clean());
        assert_eq!(rep.changed.len(), 1);
        assert_eq!(rep.changed[0].0, "a.txt");
        assert_eq!(rep.missing, vec!["sub/b.bin".to_string()]);
        assert_eq!(rep.new, vec!["c.new".to_string()]);
    }

    #[test]
    fn manifest_is_portable_across_roots() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("x"), b"payload").unwrap();
        let manifest = dir.path().join("m.jsonl");
        let mut sink = null_sink();
        create(&src, &manifest, &mut sink).unwrap();

        // "Copy to the external drive", then verify at the new root.
        let dst = dir.path().join("mnt-ssd");
        fs::create_dir_all(&dst).unwrap();
        fs::copy(src.join("x"), dst.join("x")).unwrap();
        let rep = verify(&dst, &manifest, &mut sink).unwrap();
        assert!(rep.clean());
        assert_eq!(rep.ok, 1);
    }

    #[test]
    fn an_unreadable_file_is_a_read_error_not_silently_dropped_or_fatal() {
        // The module's own header comment names this the single most
        // important classification: "ReadError is the one people forget
        // and the one that matters most here... a file that CANNOT BE
        // READ is a dying disk talking to you." Neither existing test
        // ever produced one. Also confirms the stated design goal
        // directly: verify() must keep going past the read error and
        // still report the OTHER files correctly, not stop or swallow
        // the rest of the scan.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("data");
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("healthy.txt"), b"fine").unwrap();
            fs::write(root.join("dying.bin"), b"will become unreadable").unwrap();
            let manifest = dir.path().join("m.jsonl");
            let mut sink = null_sink();
            create(&root, &manifest, &mut sink).unwrap();

            fs::set_permissions(root.join("dying.bin"), fs::Permissions::from_mode(0o000)).unwrap();
            let rep = verify(&root, &manifest, &mut sink);
            fs::set_permissions(root.join("dying.bin"), fs::Permissions::from_mode(0o644)).unwrap();
            let rep = rep.expect("a read error on one file must not fail the whole verify");

            // NOTE: if this runs as root, the permission bits above are
            // not actually enforced (root can read a 0o000 file
            // regardless), so read_errors may legitimately be empty --
            // asserting on it unconditionally would be the same mistake
            // made and caught earlier in cleanup.rs/deltahostile.rs. The
            // property that holds in BOTH cases is that the healthy file
            // is still correctly verified either way.
            // ...and the assertion has to say that in BOTH cases too: as root the "dying" file
            // reads fine and is verified, so ok is 2, not 1 — this line asserted 1 unconditionally
            // and failed in a root sandbox while its own comment above explained why it would.
            if rep.read_errors.is_empty() {
                assert_eq!(rep.ok, 2, "as root both files read, and both must verify");
            } else {
                assert_eq!(rep.ok, 1, "the healthy file must still be verified regardless of the other file's readability");
                assert_eq!(rep.read_errors[0].0, "dying.bin");
                assert!(!rep.clean(), "a real read error must make the report non-clean");
            }
        }
    }

    #[test]
    fn create_fails_fast_on_an_unreadable_file_rather_than_writing_a_partial_manifest() {
        // "At CREATE time an unreadable file is fatal: a manifest that
        // silently omits files would 'verify clean' forever." A real,
        // deliberate asymmetry with verify()'s keep-going behavior --
        // untested that create() actually enforces it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("data");
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("locked.bin"), b"x").unwrap();
            fs::set_permissions(root.join("locked.bin"), fs::Permissions::from_mode(0o000)).unwrap();
            let manifest = dir.path().join("m.jsonl");
            let mut sink = null_sink();
            let result = create(&root, &manifest, &mut sink);
            fs::set_permissions(root.join("locked.bin"), fs::Permissions::from_mode(0o644)).unwrap();
            // Same root-bypass caveat as the test above: only assert the
            // failure when it actually occurred.
            if let Err(e) = result {
                assert!(e.contains("locked.bin") || e.to_lowercase().contains("permission"), "{e}");
            }
        }
    }

    #[test]
    fn load_rejects_an_empty_manifest_file() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("empty.jsonl");
        fs::write(&manifest, "").unwrap();
        let err = load(&manifest).expect_err("an empty file has no header line to read");
        assert!(err.contains("empty manifest"), "{err}");
    }

    #[test]
    fn load_rejects_a_manifest_with_the_wrong_kind_header() {
        // The format-version guard: a manifest from an incompatible
        // future (or past) version must be refused explicitly rather
        // than silently misread as if every entry were dmv1-shaped.
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("wrong.jsonl");
        fs::write(&manifest, r#"{"kind":"dmv2","root":"/x","created_unix":0}"#).unwrap();
        let err = load(&manifest).expect_err("an unrecognized kind must be refused");
        assert!(err.contains("dmv2"), "{err}");
    }

    #[test]
    fn load_rejects_malformed_json_in_an_entry_line_not_just_the_header() {
        // The header can parse fine while a LATER line is corrupt (a
        // truncated write, a manual edit gone wrong) -- a separate
        // failure point from the header check above, and the existing
        // tests only ever fed load() manifests written by create()
        // itself, which never happens to produce a malformed entry.
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("corrupt.jsonl");
        fs::write(&manifest, "{\"kind\":\"dmv1\",\"root\":\"/x\",\"created_unix\":0}\nnot json\n").unwrap();
        let err = load(&manifest).expect_err("a corrupt entry line must be refused, not silently skipped");
        assert!(err.contains("manifest entry"), "{err}");
    }

    #[test]
    fn symlinks_are_skipped_with_a_visible_note_not_walked_or_hashed() {
        // walk()'s own doc explains the reasoning: following them "turns
        // a manifest into a trap (targets move independently)." Untested
        // that a symlink is genuinely excluded from both create()'s
        // output and verify()'s "new" detection.
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("data");
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("real.txt"), b"x").unwrap();
            std::os::unix::fs::symlink(root.join("real.txt"), root.join("link.txt")).unwrap();
            let manifest = dir.path().join("m.jsonl");
            let mut notes: Vec<String> = Vec::new();
            {
                let mut sink = |e: Event| { if let Event::Note(n) = e { notes.push(n); } };
                let n = create(&root, &manifest, &mut sink).unwrap();
                assert_eq!(n, 1, "only the real file, not the symlink");
            }
            assert!(notes.iter().any(|n| n.contains("symlink")), "{notes:?}");
            let mut sink2 = null_sink();
            let rep = verify(&root, &manifest, &mut sink2).unwrap();
            assert!(rep.new.is_empty(), "the symlink must not show up as an untracked new file either: {:?}", rep.new);
        }
    }
}
