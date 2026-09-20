//! Debug-related caches: find them, measure them, and prune them in two steps
//! (plan, then apply) so nothing is deleted that was not first shown.
//!
//! This matters more on the target devices than on a desktop: a debuginfod cache
//! after one session inside GTK is several hundred MB, and a phone has a 32 GB
//! eMMC shared with everything else.

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheKind {
    /// Debug info and sources fetched by gdb/elfutils from debuginfod servers.
    Debuginfod,
    /// gdb's `index-cache`: saved symbol indexes that make the second load fast.
    GdbIndex,
    /// systemd-coredump storage. Usually root-owned; measured if readable.
    Coredumps,
    /// A cargo target directory. Not strictly a debug cache, but on these devices
    /// it is where the debug info bytes actually are.
    CargoTarget,
}

impl CacheKind {
    pub fn label(self) -> &'static str {
        match self {
            CacheKind::Debuginfod => "debuginfod downloads",
            CacheKind::GdbIndex => "gdb index cache",
            CacheKind::Coredumps => "core dumps",
            CacheKind::CargoTarget => "cargo target dir",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Usage {
    /// Allocated bytes (st_blocks), not apparent size: core dumps are sparse and
    /// btrfs/f2fs compress, so file lengths overstate what pruning would free.
    pub bytes: u64,
    pub files: u64,
    pub newest: Option<SystemTime>,
    pub unreadable: bool,
}

pub fn locate(project_root: Option<&Path>) -> Vec<(CacheKind, PathBuf)> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let xdg_cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".cache")));
    let mut v = Vec::new();
    // Same precedence as elfutils' debuginfod-client.
    if let Some(p) = std::env::var_os("DEBUGINFOD_CACHE_PATH") {
        v.push((CacheKind::Debuginfod, PathBuf::from(p)));
    } else {
        let legacy = home.as_ref().map(|h| h.join(".debuginfod_client_cache"));
        match (legacy, &xdg_cache) {
            (Some(l), _) if l.is_dir() => v.push((CacheKind::Debuginfod, l)),
            (_, Some(c)) => v.push((CacheKind::Debuginfod, c.join("debuginfod_client"))),
            _ => {}
        }
    }
    if let Some(c) = &xdg_cache {
        v.push((CacheKind::GdbIndex, c.join("gdb")));
    }
    v.push((CacheKind::Coredumps, PathBuf::from("/var/lib/systemd/coredump")));
    if let Some(t) = std::env::var_os("CARGO_TARGET_DIR") {
        v.push((CacheKind::CargoTarget, PathBuf::from(t)));
    } else if let Some(root) = project_root {
        v.push((CacheKind::CargoTarget, root.join("target")));
    }
    v.retain(|(_, p)| p.is_dir());
    v
}

fn visit(dir: &Path, f: &mut dyn FnMut(&Path, &std::fs::Metadata)) -> io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let Ok(md) = e.path().symlink_metadata() else { continue };
        if md.is_dir() {
            let _ = visit(&e.path(), f);
        } else if md.is_file() {
            f(&e.path(), &md);
        }
    }
    Ok(())
}

fn last_used(md: &std::fs::Metadata) -> SystemTime {
    // relatime only bumps atime once a day, which is fine at this granularity;
    // on noatime mounts this degrades to mtime.
    let secs = md.atime().max(md.mtime()).max(0) as u64;
    UNIX_EPOCH + Duration::from_secs(secs)
}

pub fn measure(dir: &Path) -> Usage {
    let mut u = Usage::default();
    let r = visit(dir, &mut |_, md| {
        u.bytes += md.blocks() * 512;
        u.files += 1;
        let t = last_used(md);
        u.newest = Some(u.newest.map_or(t, |n| n.max(t)));
    });
    u.unreadable = r.is_err();
    u
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrunePlan {
    pub remove: Vec<(PathBuf, u64)>,
    pub bytes: u64,
}

/// Everything not read or written for `unused_for`.
pub fn plan_prune(dir: &Path, unused_for: Duration) -> PrunePlan {
    let cutoff = SystemTime::now().checked_sub(unused_for).unwrap_or(UNIX_EPOCH);
    let mut plan = PrunePlan::default();
    let _ = visit(dir, &mut |p, md| {
        if last_used(md) < cutoff {
            let b = md.blocks() * 512;
            plan.remove.push((p.to_path_buf(), b));
            plan.bytes += b;
        }
    });
    plan.remove.sort();
    plan
}

/// Returns bytes freed and the files that could not be removed.
pub fn apply(plan: &PrunePlan) -> (u64, Vec<(PathBuf, io::Error)>) {
    let mut freed = 0;
    let mut failed = Vec::new();
    for (p, b) in &plan.remove {
        match std::fs::remove_file(p) {
            Ok(()) => freed += b,
            Err(e) => failed.push((p.clone(), e)),
        }
    }
    (freed, failed)
}

pub fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}
