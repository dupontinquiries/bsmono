//! Crash breadcrumbs (r466) — an fsync'd "last step reached" marker that survives a HARD power-off.
//!
//! The Fairphone-5/Alpine GPU path can power the whole device off (driver/power fault), which never
//! flushes normal logs. `drop_crumb` writes one line, flushes, and `sync_all` (fsync) so the bytes
//! hit storage before the next risky GPU call — after a power-off, the LAST line of the breadcrumb
//! file is the last stage the program reached. Call it at sparse high-risk points (device/swapchain/
//! pipeline create, first worldgen/mesh dispatch, first submit) + a slow heartbeat; NOT every frame
//! (fsync is slow). Gated by UNENDLICH_BREADCRUMB (default on); path via UNENDLICH_BREADCRUMB_FILE.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn enabled() -> bool {
    std::env::var("UNENDLICH_BREADCRUMB").map(|v| v != "0").unwrap_or(true)
}
fn path() -> String {
    std::env::var("UNENDLICH_BREADCRUMB_FILE").unwrap_or_else(|_| "unendlich_breadcrumb.log".to_string())
}

/// Record reaching `stage`, flushed + fsync'd so a power-off can't lose it.
pub fn drop_crumb(stage: &str) {
    if !enabled() {
        return;
    }
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path()) {
        let _ = writeln!(f, "{ts} #{seq} {stage}");
        let _ = f.flush();
        let _ = f.sync_all(); // fsync: force to storage so a hard power-off can't drop it
    }
}

/// Start a fresh breadcrumb log (truncate) at process start.
pub fn reset() {
    if !enabled() {
        return;
    }
    SEQ.store(0, Ordering::Relaxed);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open(path()) {
        let _ = writeln!(f, "--- unendlich breadcrumbs (fsync'd; LAST line = last stage reached before any crash) ---");
        let _ = f.sync_all();
    }
}
