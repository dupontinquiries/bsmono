//! `sim::world_history` — world-history points that hold a world, not a timestamp.
//!
//! The menu's WORLD HISTORY said "World-history points snapshot your world so you can step
//! back" and its restore button wrote a `RESTORE` marker "for next launch". Behind it, a
//! point was `save_all()` -- the live chunk files overwritten in place -- plus a `.point`
//! file with a reason and a count, and nothing ever read the marker. There was no earlier
//! state anywhere on disk to go back to: the button could not have worked.
//!
//! A point here is a COPY of the world's `region/` tree (the sparse chunk-edit files,
//! `storage::chunk_path`) under `rollbacks/<ts>/region`, beside the `<ts>.point` marker the
//! menu lists. A restore is staged (`RESTORE` holds the ts) and applied the next time the
//! world is opened (`apply_staged`, before the chunk manager reads the directory): the
//! current `region/` is itself kept as a point first ("before-restore"), so a restore is
//! never a loss, then the point's copy replaces it. `prune` keeps the newest N and the
//! staged one.
//!
//! All paths are relative to the world root (`server_config::world_dir_for`): the points
//! live with the world they hold. Pure file operations; no engine state.

use std::io;
use std::path::{Path, PathBuf};

/// One history point: when, why, and how many chunk files it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Point {
    pub ts: u64,
    pub reason: String,
    pub chunks: usize,
    /// The copy is present on disk (a marker from before this module has none).
    pub restorable: bool,
}

/// How many points `prune` keeps by default.
pub const KEEP: usize = 12;

pub fn dir(root: &Path) -> PathBuf {
    root.join("rollbacks")
}

fn point_dir(root: &Path, ts: u64) -> PathBuf {
    dir(root).join(ts.to_string())
}

fn marker(root: &Path, ts: u64) -> PathBuf {
    dir(root).join(format!("{ts}.point"))
}

fn staged_marker(root: &Path) -> PathBuf {
    dir(root).join("RESTORE")
}

/// Copy every `*.mks` under `from/region` to `to/region`, keeping the region layout.
/// Returns how many files were copied; a missing `from/region` is zero, not an error.
fn copy_region(from: &Path, to: &Path) -> io::Result<usize> {
    let src = from.join("region");
    let Ok(regions) = std::fs::read_dir(&src) else { return Ok(0) };
    let mut n = 0;
    for region in regions.flatten() {
        if !region.path().is_dir() {
            continue;
        }
        let out_dir = to.join("region").join(region.file_name());
        for f in std::fs::read_dir(region.path())?.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some("mks") {
                continue;
            }
            std::fs::create_dir_all(&out_dir)?;
            std::fs::copy(&p, out_dir.join(f.file_name()))?;
            n += 1;
        }
    }
    Ok(n)
}

/// Take a point of the world as it is on disk now (call after `save_all`).
pub fn take(root: &Path, ts: u64, reason: &str) -> io::Result<Point> {
    let pd = point_dir(root, ts);
    let _ = std::fs::remove_dir_all(&pd);
    std::fs::create_dir_all(&pd)?;
    let chunks = copy_region(root, &pd)?;
    let reason: String = reason.chars().filter(|c| !c.is_control()).collect();
    std::fs::write(marker(root, ts), format!("reason={reason}\nchunks={chunks}\n"))?;
    Ok(Point { ts, reason, chunks, restorable: true })
}

/// The world's points, newest first, from the markers; a marker without its copy (written
/// before this module held anything) is listed and marked not restorable.
pub fn list(root: &Path) -> Vec<Point> {
    let Ok(rd) = std::fs::read_dir(dir(root)) else { return Vec::new() };
    let mut pts = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("point") {
            continue;
        }
        let Some(ts) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u64>().ok()) else { continue };
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        let mut reason = String::from("point");
        let mut chunks = 0usize;
        for line in body.lines() {
            if let Some(r) = line.strip_prefix("reason=") {
                reason = r.to_string();
            }
            if let Some(c) = line.strip_prefix("chunks=") {
                chunks = c.trim().parse().unwrap_or(0);
            }
        }
        let restorable = point_dir(root, ts).join("region").is_dir() || chunks == 0 && point_dir(root, ts).is_dir();
        pts.push(Point { ts, reason, chunks, restorable });
    }
    pts.sort_by(|a, b| b.ts.cmp(&a.ts));
    pts
}

/// Remove a point: its marker and its copy. Unstages it if it was the staged restore.
pub fn delete(root: &Path, ts: u64) -> io::Result<()> {
    if staged(root) == Some(ts) {
        let _ = std::fs::remove_file(staged_marker(root));
    }
    let _ = std::fs::remove_dir_all(point_dir(root, ts));
    match std::fs::remove_file(marker(root, ts)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Stage `ts` to be restored when the world is next opened. Refuses a point with no copy.
pub fn stage_restore(root: &Path, ts: u64) -> io::Result<()> {
    if !point_dir(root, ts).is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("point {ts} holds no chunks to restore")));
    }
    std::fs::create_dir_all(dir(root))?;
    std::fs::write(staged_marker(root), ts.to_string())
}

/// The staged restore, if one is waiting.
pub fn staged(root: &Path) -> Option<u64> {
    std::fs::read_to_string(staged_marker(root)).ok()?.trim().parse().ok()
}

/// What `apply_staged` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    /// The point restored.
    pub ts: u64,
    /// Chunk files now in `region/`.
    pub chunks: usize,
    /// The point taken of the state being replaced, so the restore can itself be undone.
    pub safety_ts: u64,
}

/// Apply a staged restore, if any: keep the current `region/` as a "before-restore" point
/// stamped `now_ts`, replace `region/` with the point's copy, clear the stage. Call before
/// the chunk manager opens the world. A staged point whose copy is gone is unstaged and
/// reported as an error rather than wiping the world.
pub fn apply_staged(root: &Path, now_ts: u64) -> io::Result<Option<Applied>> {
    let Some(ts) = staged(root) else { return Ok(None) };
    let _ = std::fs::remove_file(staged_marker(root));
    let src = point_dir(root, ts);
    if !src.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("staged point {ts} is gone")));
    }
    // the safety point: never stamp over an existing point
    let mut safety_ts = now_ts.max(1);
    while point_dir(root, safety_ts).exists() || marker(root, safety_ts).exists() {
        safety_ts += 1;
    }
    take(root, safety_ts, "before-restore")?;
    let live = root.join("region");
    let _ = std::fs::remove_dir_all(&live);
    let chunks = copy_region(&src, root)?;
    Ok(Some(Applied { ts, chunks, safety_ts }))
}

/// Keep the newest `keep` points (and the staged one, whatever its age); delete the rest.
/// Returns how many were deleted.
pub fn prune(root: &Path, keep: usize) -> usize {
    let pts = list(root);
    let keep_ts = staged(root);
    let mut deleted = 0;
    for p in pts.iter().skip(keep) {
        if Some(p.ts) == keep_ts {
            continue;
        }
        if delete(root, p.ts).is_ok() {
            deleted += 1;
        }
    }
    deleted
}

/// "3m ago · craft · 12 chunks": the menu's row for a point.
pub fn label(p: &Point, now_ts: u64) -> String {
    let ago = now_ts.saturating_sub(p.ts);
    let when = if ago < 90 {
        format!("{ago}s ago")
    } else if ago < 5400 {
        format!("{}m ago", ago / 60)
    } else if ago < 172_800 {
        format!("{}h ago", ago / 3600)
    } else {
        format!("{}d ago", ago / 86_400)
    };
    if p.restorable {
        format!("{when}  ·  {}  ·  {} chunks", p.reason, p.chunks)
    } else {
        format!("{when}  ·  {}  ·  no copy (older build)", p.reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("unendlich-world-history-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("region").join("0_0_0")).unwrap();
        d
    }

    fn write_chunk(root: &Path, region: &str, name: &str, body: &str) {
        let rd = root.join("region").join(region);
        std::fs::create_dir_all(&rd).unwrap();
        std::fs::write(rd.join(name), body).unwrap();
    }

    fn read_chunk(root: &Path, region: &str, name: &str) -> Option<String> {
        std::fs::read_to_string(root.join("region").join(region).join(name)).ok()
    }

    /// A point holds the chunk files as they were; editing on after it does not change it;
    /// the list reads it back newest first.
    #[test]
    fn a_point_is_a_copy_of_the_region_tree() {
        let root = world("copy");
        write_chunk(&root, "0_0_0", "chunk_1_2_3.mks", "v1");
        write_chunk(&root, "0_0_0", "notes.txt", "not a chunk");
        write_chunk(&root, "1_0_0", "chunk_9_0_0.mks", "far");
        let p = take(&root, 1000, "craft").unwrap();
        assert_eq!(p.chunks, 2, "two chunk files, the stray text file is not one");
        write_chunk(&root, "0_0_0", "chunk_1_2_3.mks", "v2 edited after the point");
        assert_eq!(std::fs::read_to_string(point_dir(&root, 1000).join("region/0_0_0/chunk_1_2_3.mks")).unwrap(), "v1");
        assert!(!point_dir(&root, 1000).join("region/0_0_0/notes.txt").exists());
        take(&root, 2000, "timer").unwrap();
        let l = list(&root);
        assert_eq!(l.iter().map(|p| p.ts).collect::<Vec<_>>(), vec![2000, 1000], "newest first");
        assert_eq!(l[1].reason, "craft");
        assert!(l.iter().all(|p| p.restorable));
        assert_eq!(label(&l[1], 1000 + 125), "2m ago  ·  craft  ·  2 chunks");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Restore: staged, then applied on open -- the live tree becomes the point's, and the
    /// state being replaced is kept as a point of its own, so the restore can be undone.
    #[test]
    fn a_staged_restore_is_applied_on_open_and_keeps_what_it_replaced() {
        let root = world("restore");
        write_chunk(&root, "0_0_0", "chunk_0_0_0.mks", "old house");
        take(&root, 100, "timer").unwrap();
        write_chunk(&root, "0_0_0", "chunk_0_0_0.mks", "the house is gone");
        write_chunk(&root, "0_0_0", "chunk_0_0_1.mks", "a new hole");
        assert_eq!(staged(&root), None);
        stage_restore(&root, 100).unwrap();
        assert_eq!(staged(&root), Some(100));
        let a = apply_staged(&root, 500).unwrap().expect("a restore was staged");
        assert_eq!(a.ts, 100);
        assert_eq!(a.chunks, 1);
        assert_eq!(a.safety_ts, 500);
        assert_eq!(read_chunk(&root, "0_0_0", "chunk_0_0_0.mks").as_deref(), Some("old house"));
        assert_eq!(read_chunk(&root, "0_0_0", "chunk_0_0_1.mks"), None, "the hole dug after the point is gone");
        assert_eq!(staged(&root), None, "applied once");
        assert_eq!(apply_staged(&root, 600).unwrap(), None);
        // the safety point holds the replaced state, and restoring IT undoes the restore
        let l = list(&root);
        let safety = l.iter().find(|p| p.ts == 500).expect("the before-restore point is listed");
        assert_eq!(safety.reason, "before-restore");
        assert_eq!(safety.chunks, 2);
        stage_restore(&root, 500).unwrap();
        apply_staged(&root, 700).unwrap();
        assert_eq!(read_chunk(&root, "0_0_0", "chunk_0_0_0.mks").as_deref(), Some("the house is gone"));
        assert_eq!(read_chunk(&root, "0_0_0", "chunk_0_0_1.mks").as_deref(), Some("a new hole"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A staged point whose copy was deleted is reported, not applied, and the world is not
    /// wiped; a marker from before this module (no copy) cannot be staged at all.
    #[test]
    fn a_missing_copy_is_refused_and_an_old_marker_is_not_restorable() {
        let root = world("missing");
        write_chunk(&root, "0_0_0", "chunk_0_0_0.mks", "live");
        take(&root, 100, "timer").unwrap();
        stage_restore(&root, 100).unwrap();
        std::fs::remove_dir_all(point_dir(&root, 100)).unwrap();
        let err = apply_staged(&root, 200).unwrap_err();
        assert!(err.to_string().contains("gone"), "{err}");
        assert_eq!(read_chunk(&root, "0_0_0", "chunk_0_0_0.mks").as_deref(), Some("live"), "the world is untouched");
        assert_eq!(staged(&root), None, "and the stage is cleared");
        std::fs::write(marker(&root, 50), "reason=timer\nchunks=4\n").unwrap();
        let old = list(&root).into_iter().find(|p| p.ts == 50).unwrap();
        assert!(!old.restorable);
        assert!(label(&old, 60).contains("no copy"));
        assert!(stage_restore(&root, 50).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Delete removes the marker and the copy (and unstages); prune keeps the newest N and
    /// never the staged one, whatever its age.
    #[test]
    fn delete_and_prune_keep_the_newest_and_the_staged() {
        let root = world("prune");
        write_chunk(&root, "0_0_0", "chunk_0_0_0.mks", "x");
        for ts in [10u64, 20, 30, 40, 50] {
            take(&root, ts, "timer").unwrap();
        }
        stage_restore(&root, 10).unwrap();
        delete(&root, 30).unwrap();
        assert!(!point_dir(&root, 30).exists() && !marker(&root, 30).exists());
        assert_eq!(prune(&root, 2), 1, "of 10,20,40,50: keep 50,40 and the staged 10; 20 goes");
        assert_eq!(list(&root).iter().map(|p| p.ts).collect::<Vec<_>>(), vec![50, 40, 10]);
        delete(&root, 10).unwrap();
        assert_eq!(staged(&root), None, "deleting the staged point unstages it");
        assert_eq!(prune(&root, 5), 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
