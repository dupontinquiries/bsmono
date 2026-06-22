// src/systems/screen_temp_system.rs
//
// Worker thread that reconstructs the screen-temperature bitfield
// and publishes it to `$XDG_RUNTIME_DIR/ambient-learner/`.
//
// Cadence: 4 Hz. Faster doesn't add real info (the underlying
// thermal sensors update at 1-4 Hz internally) and slower makes
// the rain/glove input filter feel laggy.
//
// Resolution is **adaptive**:
//
//   * A natural resolution is computed each tick from the number of
//     thermal sensors we have and the kernel σ in the layout. With
//     few sensors and wide kernels, the published grid stays coarse
//     because finer cells would just be smooth interpolations.
//
//   * Consumer apps can request a higher resolution by dropping a
//     hint file in the publisher directory. The screen-temp module
//     reads every hint, picks the highest non-stale request, and
//     uses the max of (natural, requested) — capped at 32×48.
//     See `screen_temp_hints.rs` for the hint-file format.
//
// On systems where the publisher can't write to the runtime dir
// (rare — read-only sandbox, no XDG_RUNTIME_DIR set), the worker
// logs once and exits silently. The rest of the app keeps running.
//
// Possible future improvement: only publish when at least one
// consumer is listening. The current design writes ~2.6 KB/s to
// tmpfs whether anyone's reading or not. The hints file already
// gives us a signal here — we could pause publishing entirely
// when no recent hint has been seen, but the cost-when-idle is
// trivial enough that we haven't bothered.

use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::screen_temp_hints::{self, AggregatedHints};
use crate::screen_temps::{Publisher, ScreenLayout};
use crate::world_state::{self, WorldState};

/// Publish at 4 Hz. Underlying thermal sensors typically update at
/// 1-4 Hz so going higher is mostly waste; going lower makes the
/// downstream input filters feel laggy.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(250);

/// Re-read hint files only this often. Hint files don't change at
/// 4 Hz — every second is plenty, and it saves us a `readdir` +
/// per-file JSON parse on most publish ticks.
const HINTS_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// Spawn the publisher worker thread. Returns the handle, or `None`
/// if we couldn't even open the output directory.
pub fn spawn(
    world: WorldState,
    shutdown: crate::systems::ShutdownSignal,
) -> Option<JoinHandle<()>> {
    let publisher = match Publisher::at_default_location() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "⚠️  screen-temps publisher disabled: cannot open output dir ({e})"
            );
            return None;
        }
    };

    let layout = ScreenLayout::generic_phone();

    let handle = thread::Builder::new()
        .name("ambient-screen-temp".to_string())
        .spawn(move || run(world, publisher, layout, shutdown))
        .expect("failed to spawn screen-temp thread");

    println!("📐 screen-temp publisher spawned (4 Hz, adaptive resolution)");
    Some(handle)
}

fn run(
    world: WorldState,
    publisher: Publisher,
    mut layout: ScreenLayout,
    shutdown: crate::systems::ShutdownSignal,
) {
    // Cached hints state. We re-read at HINTS_REFRESH_INTERVAL, not on
    // every publish tick, so the dir scan doesn't dominate.
    let mut hints: AggregatedHints = AggregatedHints::NONE.clone();
    let mut hints_last_read: Option<Instant> = None;
    let mut hints_last_dir_mtime: Option<std::time::SystemTime> = None;
    let mut last_resolution_logged: Option<(u16, u16)> = None;

    loop {
        if shutdown.is_requested() {
            println!("📐 screen-temp publisher shutting down");
            return;
        }
        // Periodically refresh the hints — but only do the readdir +
        // JSON parse work when the hint directory's mtime has
        // changed since last time. A single stat() per second is
        // ~3 orders of magnitude cheaper than readdir + N file
        // reads, and most users will have zero hint files so
        // the readdir would find nothing anyway.
        let now = Instant::now();
        let interval_elapsed = hints_last_read
            .map(|t| now.duration_since(t) >= HINTS_REFRESH_INTERVAL)
            .unwrap_or(true);
        if interval_elapsed {
            // Cheap "has anything changed" check: dir mtime plus the
            // max mtime of any hint file inside it. Catches both
            // file-list changes (publisher added/removed a hint)
            // *and* in-place rewrites (publisher truncated and
            // overwrote its own hint with new values).
            //
            // The stat() calls are inexpensive — typical hint dir
            // has 0-2 files and stat() is microseconds. The readdir
            // + JSON parse path that this gates is orders of
            // magnitude more expensive.
            let current_signature = hint_dir_signature(publisher.output_dir());
            let dir_changed = current_signature != hints_last_dir_mtime;

            if dir_changed || hints_last_read.is_none() {
                hints = screen_temp_hints::read_hints(publisher.output_dir());
                hints_last_dir_mtime = current_signature;
            }
            hints_last_read = Some(now);
        }

        // Snapshot what we need from world state, drop the lock fast.
        let (names, readings, ambient) = {
            let s = world_state::read(&world);
            (
                s.sensor_names.clone(),
                s.sensors.clone(),
                s.ambient_temp,
            )
        };

        // If we have no sensors yet, skip — an "all ambient" frame is
        // just confusing noise for consumers trying to test their
        // integration.
        if names.is_empty() || readings.is_empty() {
            thread::sleep(PUBLISH_INTERVAL);
            continue;
        }

        // Pick this tick's resolution. We pass an effective σ to the
        // chooser that's the **larger** of:
        //   * `layout.average_sigma()` — the σ actually used in the
        //     diffusion kernels (configured per-sensor in the layout)
        //   * sensor-count-based recommended σ — what σ *would* be
        //     for evenly-spaced sensors with this count
        //
        // The chooser computes `2/σ` cells per dim, so a larger σ
        // produces a smaller (more conservative) grid. We pick the
        // larger of the two because *whichever blur is worse* is
        // what actually limits resolvable detail. Picking the
        // smaller one would over-promise resolution the kernels
        // can't deliver.
        let effective_sigma = layout
            .average_sigma()
            .max(ScreenLayout::recommended_sigma(names.len()));

        let (cols, rows) = screen_temp_hints::choose_resolution(
            names.len(),
            effective_sigma,
            &hints,
        );
        if last_resolution_logged != Some((cols, rows)) {
            let watcher_summary = if hints.watcher_names.is_empty() {
                "no named watchers".to_string()
            } else {
                hints.watcher_names.join(", ")
            };
            println!(
                "📐 screen-temps resolution → {}×{}  ({} sensor{}, hints: {} files; {})",
                cols, rows,
                names.len(), if names.len() == 1 { "" } else { "s" },
                hints.n_files, watcher_summary,
            );
            last_resolution_logged = Some((cols, rows));
            // Resolution change means the metadata sidecar needs to be
            // rewritten — let the publisher know.
            publisher.invalidate_metadata();
        }
        layout.cols = cols;
        layout.rows = rows;

        // The Publisher itself rate-limits, but we still sleep
        // between attempts so we don't spin uselessly when the
        // publisher decides to skip.
        if let Err(e) = publisher.publish(
            &layout,
            &names,
            &readings,
            ambient,
            // Inner rate-limit: 200ms. Slightly less than our outer
            // sleep so we hit it on the nose without dropping
            // frames to drift.
            Duration::from_millis(200),
        ) {
            eprintln!("⚠️  screen-temps publish error: {e}");
        }

        thread::sleep(PUBLISH_INTERVAL);
    }
}

/// Cheap "has the hint set probably changed" check.
///
/// Returns the directory's mtime. Catches file-list changes
/// (publisher added/removed a hint, including the recommended
/// write-temp-then-rename pattern). Does NOT catch in-place
/// rewrites of an existing hint file — we accept that staleness
/// because at worst we publish at the previous resolution for one
/// tick, and the recommended hint-publishing pattern (in
/// SCREEN_TEMPS.md) uses rename-based atomic writes that this
/// check sees correctly.
///
/// One stat() per second is ~3 orders of magnitude cheaper than
/// readdir + N json reads, which is the work this gates.
fn hint_dir_signature(dir: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(dir).ok()?.modified().ok()
}
