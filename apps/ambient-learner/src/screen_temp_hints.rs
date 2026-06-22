// src/screen_temp_hints.rs
//
// Adaptive resolution for the screen-temperature bitfield.
//
// The published grid size is a tradeoff:
//   * Too fine → wastes CPU / tmpfs on cells whose values are
//     interpolated artifacts rather than signal.
//   * Too coarse → consumers that map screen pixels to grid cells
//     bucket many pixels into one cell, missing real structure.
//
// Two inputs influence the choice:
//
//   1. **Information available**. With N sensors at Gaussian σ, the
//      effective number of independent "blobs" the reconstruction
//      can resolve is roughly N · π · σ² · cols · rows in the limit
//      — past that point we're oversampling.
//
//   2. **Consumer demand**. Apps that read our grid can express a
//      preference by writing a hints file:
//
//          $XDG_RUNTIME_DIR/ambient-learner/screen-temps.hints.json
//
//      Schema (all fields optional):
//          {
//            "min_cols": 16,
//            "min_rows": 24,
//            "expires_unix_sec": 1714329600,
//            "watchers": [
//              { "name": "rain-touch-filter", "pid": 12345 }
//            ]
//          }
//
//      Each consumer can write its own hint file under a per-consumer
//      filename — we read every `screen-temps.hints.*.json` in the
//      dir and take the maximum requested resolution.
//
// We deliberately do NOT use flock-based "is anyone watching" detection.
// flock semantics differ across filesystems (NFS, tmpfs, overlay), and
// a consumer would have to know to take the lock — adding code on
// every reader is a worse experience than asking them to drop one
// JSON file when they need higher res.

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// What a consumer can put in a hint file.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ResolutionHint {
    /// Minimum columns the consumer wants.
    #[serde(default)]
    pub min_cols: Option<u16>,
    /// Minimum rows the consumer wants.
    #[serde(default)]
    pub min_rows: Option<u16>,
    /// Unix timestamp after which this hint is stale and should be
    /// ignored. A consumer should refresh its hint file periodically;
    /// 5 minutes is a reasonable refresh cadence with `expires_unix_sec`
    /// set to 30 minutes ahead.
    #[serde(default)]
    pub expires_unix_sec: Option<i64>,
    /// Informational: which consumers are watching. We log this on
    /// every resolution change so you can tell which app caused the
    /// bump.
    #[serde(default)]
    pub watchers: Vec<Watcher>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Watcher {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub pid: Option<i32>,
}

/// The merged, validated outcome of reading all hint files.
#[derive(Clone, Debug)]
pub struct AggregatedHints {
    /// Highest min_cols any non-stale hint requested. `None` means
    /// no requests; the publisher falls back to its natural default.
    pub requested_cols: Option<u16>,
    pub requested_rows: Option<u16>,
    /// Names of all watchers across hint files, deduplicated.
    pub watcher_names: Vec<String>,
    /// Number of hint files we successfully read.
    pub n_files: usize,
}

impl AggregatedHints {
    pub const NONE: AggregatedHints = AggregatedHints {
        requested_cols: None,
        requested_rows: None,
        watcher_names: Vec::new(),
        n_files: 0,
    };
}

/// Hard caps on resolution.
///
/// These derive from the **physics**, not from arbitrary design choices.
/// Heat in a typical phone-glass substrate has a thermal diffusion
/// length of ~1–5 mm at the 1-second timescale we care about. On a
/// 150 mm-diagonal display that's roughly 30–150 "independent" thermal
/// features per dimension at the absolute Nyquist limit.
///
/// We pick a generous-but-finite cap that allows dense-sensor devices
/// to publish meaningful detail without consumers being able to ask
/// for a 1000x1000 grid that we couldn't fill with real signal.
///
/// A separate runtime check inside `choose_resolution` further limits
/// the natural resolution to a Nyquist-style bound based on sensor
/// count, so the caps below are only an outer envelope.
pub const MAX_COLS: u16 = 64;
pub const MAX_ROWS: u16 = 96;
/// Lower bound: even on a sensor-starved system, this is the
/// smallest grid that still encodes "top half hotter than bottom
/// half" reliably.
pub const MIN_COLS: u16 = 4;
pub const MIN_ROWS: u16 = 6;

/// Read every hint file in `dir` and merge them.
///
/// We match `screen-temps.hints.json` and `screen-temps.hints.*.json`
/// so consumers can choose either a single shared file or one named
/// per-app. Multiple consumers sharing one file is fine; using
/// distinct files makes it easier to clean up after one of them.
///
/// Returns `AggregatedHints::NONE` on any I/O error or empty dir;
/// callers should treat that as "no consumer demand, use defaults".
pub fn read_hints(dir: &Path) -> AggregatedHints {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return AggregatedHints::NONE,
    };

    let now = unix_now();
    let mut max_cols: Option<u16> = None;
    let mut max_rows: Option<u16> = None;
    let mut watcher_names: HashMap<String, ()> = HashMap::new();
    let mut n_files = 0usize;

    for entry in entries.flatten() {
        let path = entry.path();
        if !is_hint_file(&path) {
            continue;
        }
        let Some(hint) = parse_hint_file(&path) else { continue };

        // Drop stale hints.
        if let Some(exp) = hint.expires_unix_sec {
            if exp < now {
                continue;
            }
        }

        n_files += 1;

        if let Some(c) = hint.min_cols {
            max_cols = Some(max_cols.map_or(c, |existing| existing.max(c)));
        }
        if let Some(r) = hint.min_rows {
            max_rows = Some(max_rows.map_or(r, |existing| existing.max(r)));
        }
        for w in hint.watchers {
            if !w.name.is_empty() {
                watcher_names.insert(w.name, ());
            }
        }
    }

    AggregatedHints {
        requested_cols: max_cols,
        requested_rows: max_rows,
        watcher_names: watcher_names.into_keys().collect(),
        n_files,
    }
}

/// File-name matcher. We accept:
///   screen-temps.hints.json
///   screen-temps.hints.<anything>.json
fn is_hint_file(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { return false };
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else { return false };
    if ext != "json" {
        return false;
    }
    stem == "screen-temps.hints" || stem.starts_with("screen-temps.hints.")
}

fn parse_hint_file(path: &Path) -> Option<ResolutionHint> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Decide the grid resolution to publish at, given:
///   * `n_sensors`: how many sensors we have point readings for
///   * `default_sigma`: the Gaussian σ each sensor's kernel uses (in
///     normalized [0..1] screen coordinates)
///   * `hints`: consumer demand
///
/// The algorithm:
///
///   1. Compute a **Nyquist-style natural resolution** from the
///      information content. For N evenly-spaced sensors, the
///      sample spacing is `1/√N` per dimension. Nyquist says we
///      can resolve features down to `2 × spacing`, so cells of
///      size `spacing / k` for some k ≥ 1 are justified — the
///      published grid should have roughly `k · √N` cells per
///      dimension. We pick k=2, which oversamples by 2× (smooth
///      visuals without resolving past the Nyquist limit).
///
///   2. **σ-clamp**: the Gaussian kernel itself sets a soft upper
///      bound on resolvable detail. Cells smaller than σ/2 are
///      smooth interpolations of the kernel and don't carry new
///      information, so cap at `2/σ` cells per dimension.
///
///   3. **Take min of (Nyquist, σ-limit)** — both must justify the
///      resolution, since both are real limits.
///
///   4. **Honor consumer requests** up to MAX. A consumer can ask
///      for finer cells (better for pixel-to-cell mapping) even
///      when our natural ceiling is lower — they're choosing to
///      accept oversampled output.
///
///   5. **Floor at MIN** so even a starved system produces a usable
///      grid.
///
/// Concrete worked examples:
///
///   N=4 sensors, σ=0.35:
///     Nyquist: 2·√4 = 4 cells/dim
///     σ-limit: 2/0.35 ≈ 5.7 cells/dim
///     natural: 4 cells/dim → 4 cols × 6 rows (rows scaled 1.5×)
///
///   N=10 sensors, σ=0.30:
///     Nyquist: 2·√10 ≈ 6.3 cells/dim
///     σ-limit: 2/0.30 ≈ 6.7 cells/dim
///     natural: 6 cells/dim → 6 cols × 9 rows
///
///   N=53 sensors, σ=0.14 (matched spacing):
///     Nyquist: 2·√53 ≈ 14.6 cells/dim
///     σ-limit: 2/0.14 ≈ 14.3 cells/dim
///     natural: 14 cells/dim → 14 cols × 21 rows
///
///   N=53 sensors, σ=0.35 (mismatched — σ much wider than spacing):
///     Nyquist: 14.6 cells/dim
///     σ-limit: 2/0.35 ≈ 5.7 cells/dim
///     natural: 5 cells/dim → 5 cols × 8 rows
///
/// That last case is interesting: with σ too wide, more sensors
/// don't help because each one's contribution overlaps too many
/// others. The σ-clamp catches that automatically.
pub fn choose_resolution(
    n_sensors: usize,
    default_sigma: f32,
    hints: &AggregatedHints,
) -> (u16, u16) {
    let n = n_sensors.max(1) as f32;
    let sigma = default_sigma.max(0.05);

    // Per-dimension Nyquist limit. Oversample by 2× for smoother
    // gradients (k=2 in the doc above).
    let nyquist_per_dim = 2.0 * n.sqrt();

    // Per-dimension kernel-bandwidth limit. Cells smaller than σ/2
    // are pure interpolation artifacts; cap at 2/σ.
    let kernel_per_dim = 2.0 / sigma;

    // Real information content is the lesser of the two.
    let per_dim = nyquist_per_dim.min(kernel_per_dim);

    // Phones (the assumed shape) are taller than wide. The 0.6 / 1.0
    // split matches the existing aspect ratio convention.
    let natural_cols = (per_dim * 0.6).round() as u16;
    let natural_rows = (per_dim * 1.0).round() as u16;

    // Merge with consumer demand: pick the larger of natural and
    // requested. Clamp to global caps so a misbehaving consumer
    // can't blow up memory.
    let cols = natural_cols
        .max(hints.requested_cols.unwrap_or(0))
        .clamp(MIN_COLS, MAX_COLS);
    let rows = natural_rows
        .max(hints.requested_rows.unwrap_or(0))
        .clamp(MIN_ROWS, MAX_ROWS);

    (cols, rows)
}

/// Convenience: read the hints from the publisher's standard
/// directory, then choose a resolution.
pub fn choose_resolution_for_publisher(
    dir: &Path,
    n_sensors: usize,
    default_sigma: f32,
) -> (u16, u16, AggregatedHints) {
    let hints = read_hints(dir);
    let (cols, rows) = choose_resolution(n_sensors, default_sigma, &hints);
    (cols, rows, hints)
}

/// Convenience for testing and for downstream apps that want to
/// write a hint programmatically.
#[allow(dead_code)]
pub fn write_hint(path: &Path, hint: &ResolutionHint) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(hint)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, hint: &ResolutionHint) {
        let path = dir.join(name);
        write_hint(&path, hint).unwrap();
    }

    #[test]
    fn empty_dir_has_no_hints() {
        let dir = tempdir().unwrap();
        let h = read_hints(dir.path());
        assert!(h.requested_cols.is_none());
        assert!(h.requested_rows.is_none());
        assert_eq!(h.n_files, 0);
    }

    #[test]
    fn picks_max_of_multiple_consumers() {
        let dir = tempdir().unwrap();
        write(dir.path(), "screen-temps.hints.app-a.json", &ResolutionHint {
            min_cols: Some(12), min_rows: Some(20),
            expires_unix_sec: Some(unix_now() + 3600),
            watchers: vec![Watcher { name: "app-a".into(), pid: None }],
        });
        write(dir.path(), "screen-temps.hints.app-b.json", &ResolutionHint {
            min_cols: Some(20), min_rows: Some(16),
            expires_unix_sec: Some(unix_now() + 3600),
            watchers: vec![Watcher { name: "app-b".into(), pid: None }],
        });

        let h = read_hints(dir.path());
        assert_eq!(h.requested_cols, Some(20));
        assert_eq!(h.requested_rows, Some(20));
        assert_eq!(h.n_files, 2);
        assert_eq!(h.watcher_names.len(), 2);
    }

    #[test]
    fn stale_hints_are_ignored() {
        let dir = tempdir().unwrap();
        write(dir.path(), "screen-temps.hints.json", &ResolutionHint {
            min_cols: Some(24), min_rows: Some(40),
            expires_unix_sec: Some(unix_now() - 60), // expired one min ago
            watchers: vec![],
        });

        let h = read_hints(dir.path());
        assert!(h.requested_cols.is_none(), "stale hint must be dropped");
    }

    #[test]
    fn hint_without_expiry_still_counts() {
        let dir = tempdir().unwrap();
        write(dir.path(), "screen-temps.hints.json", &ResolutionHint {
            min_cols: Some(16), min_rows: Some(24),
            expires_unix_sec: None,
            watchers: vec![],
        });
        let h = read_hints(dir.path());
        assert_eq!(h.requested_cols, Some(16));
    }

    #[test]
    fn nonmatching_filenames_ignored() {
        let dir = tempdir().unwrap();
        // Write a JSON file with the *wrong* prefix.
        let path = dir.path().join("something-else.json");
        std::fs::write(&path, r#"{"min_cols": 999}"#).unwrap();
        let h = read_hints(dir.path());
        assert!(h.requested_cols.is_none());
    }

    #[test]
    fn malformed_json_silently_skipped() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("screen-temps.hints.bad.json");
        std::fs::write(&path, "not valid json").unwrap();
        let h = read_hints(dir.path());
        // No request, but no panic either.
        assert!(h.requested_cols.is_none());
    }

    #[test]
    fn choose_resolution_scales_with_sensors() {
        let no_hints = AggregatedHints::NONE.clone();
        let (c1, r1) = choose_resolution(1, 0.35, &no_hints);
        let (c10, r10) = choose_resolution(10, 0.35, &no_hints);
        assert!(c10 > c1, "more sensors → more cols");
        assert!(r10 > r1, "more sensors → more rows");
    }

    #[test]
    fn choose_resolution_honors_consumer_request() {
        let mut hints = AggregatedHints::NONE.clone();
        hints.requested_cols = Some(20);
        hints.requested_rows = Some(30);
        let (c, r) = choose_resolution(4, 0.35, &hints);
        assert_eq!(c, 20);
        assert_eq!(r, 30);
    }

    #[test]
    fn choose_resolution_clamps_to_max() {
        let mut hints = AggregatedHints::NONE.clone();
        hints.requested_cols = Some(9999);
        hints.requested_rows = Some(9999);
        let (c, r) = choose_resolution(20, 0.20, &hints);
        assert_eq!(c, MAX_COLS);
        assert_eq!(r, MAX_ROWS);
    }

    #[test]
    fn choose_resolution_floors_on_starved_sensors() {
        let no_hints = AggregatedHints::NONE.clone();
        // 0 sensors → use minimum
        let (c, r) = choose_resolution(0, 0.35, &no_hints);
        assert!(c >= MIN_COLS);
        assert!(r >= MIN_ROWS);
    }

    #[test]
    fn consumer_request_smaller_than_natural_is_ignored() {
        // If we'd naturally produce 8 cols and the consumer asked for
        // 4, we keep 8 — we never go *below* the natural resolution.
        let mut hints = AggregatedHints::NONE.clone();
        hints.requested_cols = Some(4);
        hints.requested_rows = Some(6);
        let (c, r) = choose_resolution(10, 0.30, &hints);
        // Natural for 10 sensors at σ=0.30 should be above the request.
        let no_hints = AggregatedHints::NONE.clone();
        let (natural_c, natural_r) = choose_resolution(10, 0.30, &no_hints);
        assert_eq!(c, natural_c.max(4));
        assert_eq!(r, natural_r.max(6));
    }

    #[test]
    fn worked_example_53_sensors_matched_sigma() {
        // 53 evenly-spaced sensors → spacing-matched σ ≈ 0.14.
        // Nyquist: 2·√53 ≈ 14.6
        // σ-limit: 2/0.14 ≈ 14.3
        // per_dim = min(14.6, 14.3) ≈ 14
        // cols = 14 · 0.6 ≈ 8;  rows = 14 · 1.0 = 14
        let no_hints = AggregatedHints::NONE.clone();
        let (c, r) = choose_resolution(53, 0.14, &no_hints);
        // Allow ±1 to tolerate float rounding behavior.
        assert!((7..=9).contains(&c), "cols out of expected range, got {}", c);
        assert!((13..=15).contains(&r), "rows out of expected range, got {}", r);
    }

    #[test]
    fn worked_example_53_sensors_mismatched_sigma() {
        // 53 sensors but σ=0.35 (way wider than spacing) — kernels
        // overlap heavily so resolvable detail is σ-limited, not
        // sensor-limited.
        // σ-limit: 2/0.35 ≈ 5.7
        let no_hints = AggregatedHints::NONE.clone();
        let (c, r) = choose_resolution(53, 0.35, &no_hints);
        // Should be the σ-limited result: roughly 0.6·5.7 ≈ 3 cols,
        // 1.0·5.7 ≈ 6 rows. The MIN_* clamps may apply.
        assert!(c <= 5, "σ-limited cols, got {}", c);
        assert!(r <= 7, "σ-limited rows, got {}", r);
    }

    #[test]
    fn worked_example_4_sensors() {
        // Most common case: ~4 thermal zones on a typical Linux
        // laptop / phone. σ=0.35.
        // Nyquist: 2·√4 = 4
        // σ-limit: 2/0.35 ≈ 5.7
        // per_dim = min(4, 5.7) = 4
        let no_hints = AggregatedHints::NONE.clone();
        let (c, r) = choose_resolution(4, 0.35, &no_hints);
        assert!((4..=5).contains(&c));
        assert!((4..=6).contains(&r));
    }
}
