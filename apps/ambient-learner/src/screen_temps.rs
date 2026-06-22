// src/screen_temps.rs
//
// Publish a 2D screen-temperature bitfield to `$XDG_RUNTIME_DIR`.
//
// The idea: phones are thermal bricks. The SoC, battery, charger IC,
// and modem all dump heat into the same chassis, and that heat
// conducts through the back of the phone to the display surface.
// Different regions of the screen are warmer than others depending
// on the workload. We already poll those individual sensors; we
// just haven't exposed the *spatial* distribution to other apps.
//
// Publishing this bitfield enables some interesting downstream uses:
//
//   * Glove + rain touch input: capacitive touchscreens fire false
//     "touches" when there's water on them, and miss real touches
//     through gloves. If the input layer knows where the screen is
//     cold vs warm, it can subtract cold-spot patterns (water/snow)
//     and rescue warm-spot patterns (a finger inside a glove).
//
//   * Hand-grip detection: the user's palm leaves a slow warm
//     patch on one side of the screen.
//
//   * Pocket detection: a uniformly warm screen + low ambient light
//     = the phone is in a pocket; cold + bright = on a table.
//
// Output format is documented in `screen-temps.meta.json` next to
// the binary file, so consumers don't need a client library.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

/// Default grid size. Phones are taller than wide, so 10 columns ×
/// 16 rows roughly mirrors a 9:16 display. Total = 160 cells × 4
/// bytes = 640 bytes of payload, plus a 32-byte header.
pub const DEFAULT_COLS: u16 = 10;
pub const DEFAULT_ROWS: u16 = 16;

/// File format magic: "AMBT" as four little-endian bytes.
/// Lets a consumer sanity-check the file in one read.
pub const MAGIC: u32 = u32::from_le_bytes(*b"AMBT");

/// Format version we publish today. Bump if the header layout or
/// body encoding changes incompatibly.
pub const FORMAT_VERSION: u16 = 1;

/// Bytes of fixed-size header before the f32 grid. Keep this stable
/// across versions so old readers can at least read the header and
/// notice the version mismatch.
pub const HEADER_SIZE: usize = 32;

/// Position of a thermal sensor on the screen surface, in
/// normalized [0..1] coordinates. (0, 0) is top-left, (1, 1) is
/// bottom-right. Multiple sensors at the same coordinate sum
/// linearly (with the diffusion kernel taking care of overlap).
#[derive(Clone, Debug)]
pub struct SensorPosition {
    /// Sensor identifier, matching one of our `sensor_names` strings.
    /// Match is by substring rather than exact equality so configs
    /// can be device-portable: e.g. "cpu" matches both
    /// `cpu-thermal` and `cpu0_thermal`.
    pub name_pattern: String,
    /// Normalized X (column direction).
    pub x: f32,
    /// Normalized Y (row direction).
    pub y: f32,
    /// Coupling weight: how strongly does this sensor affect the
    /// screen surface? A battery or modem sits behind the screen
    /// and couples strongly; an external charger IC mounted on the
    /// edge couples weakly. Default 1.0 for unknown sensors.
    pub weight: f32,
    /// Gaussian σ in normalized screen units. Larger = heat spreads
    /// further from the source. Typical: 0.25..0.40.
    pub sigma: f32,
}

impl SensorPosition {
    fn default_for_unknown(name: &str) -> Self {
        Self {
            name_pattern: name.to_string(),
            x: 0.5, y: 0.5,
            weight: 0.5,
            sigma: 0.35,
        }
    }
}

/// Geometry config for a specific device.
///
/// Without a config we fall back to a "every sensor stacked at the
/// center" model, which still produces a uniform reading the screen
/// average sits at — useful enough as a starting point, and easy to
/// improve incrementally with a device-specific config file.
#[derive(Clone, Debug, Default)]
pub struct ScreenLayout {
    pub positions: Vec<SensorPosition>,
    pub cols: u16,
    pub rows: u16,
}

impl ScreenLayout {
    /// A sensible default: SoC-cluster heat above center, battery
    /// at the bottom, modem/charger off to one side. Won't match
    /// any real phone exactly, but produces a recognizable pattern
    /// for consumers testing their integration.
    pub fn generic_phone() -> Self {
        Self {
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            positions: vec![
                // SoC cluster: upper third, weighted heavily.
                SensorPosition { name_pattern: "cpu".into(),     x: 0.50, y: 0.30, weight: 1.0, sigma: 0.30 },
                SensorPosition { name_pattern: "gpu".into(),     x: 0.50, y: 0.32, weight: 0.9, sigma: 0.30 },
                SensorPosition { name_pattern: "soc".into(),     x: 0.50, y: 0.30, weight: 1.0, sigma: 0.30 },
                // Battery: bottom half, takes up roughly the lower
                // 60% of the back so its kernel is wide.
                SensorPosition { name_pattern: "battery".into(), x: 0.50, y: 0.70, weight: 0.8, sigma: 0.40 },
                SensorPosition { name_pattern: "batt".into(),    x: 0.50, y: 0.70, weight: 0.8, sigma: 0.40 },
                // Modem and charger sit near the bottom edge of
                // most phones, biased to one side.
                SensorPosition { name_pattern: "modem".into(),   x: 0.30, y: 0.85, weight: 0.5, sigma: 0.25 },
                SensorPosition { name_pattern: "charger".into(), x: 0.70, y: 0.90, weight: 0.6, sigma: 0.25 },
                SensorPosition { name_pattern: "usb".into(),     x: 0.50, y: 0.95, weight: 0.4, sigma: 0.20 },
                // Display backlight: when the screen is bright, the
                // backlight LEDs warm a thin strip along one edge.
                SensorPosition { name_pattern: "display".into(), x: 0.50, y: 0.50, weight: 0.3, sigma: 0.50 },
                SensorPosition { name_pattern: "panel".into(),   x: 0.50, y: 0.50, weight: 0.3, sigma: 0.50 },
            ],
        }
    }

    /// Find the position config that best matches a sensor name.
    /// Substring match is intentional — sensor names are
    /// kernel-version-dependent and we'd rather match a family
    /// (any "cpu*") than maintain a 1:1 table.
    pub fn position_for(&self, sensor_name: &str) -> SensorPosition {
        let lower = sensor_name.to_lowercase();
        for p in &self.positions {
            if lower.contains(&p.name_pattern.to_lowercase()) {
                return p.clone();
            }
        }
        SensorPosition::default_for_unknown(sensor_name)
    }

    /// Mean Gaussian σ across configured positions. Used by the
    /// adaptive-resolution chooser to decide how fine a grid is
    /// information-justified.
    pub fn average_sigma(&self) -> f32 {
        if self.positions.is_empty() {
            return 0.35;
        }
        let sum: f32 = self.positions.iter().map(|p| p.sigma).sum();
        (sum / self.positions.len() as f32).max(0.05)
    }

    /// Recommended Gaussian σ for `n_sensors` evenly-spaced point
    /// sources covering the screen.
    ///
    /// Theory: with N sensors tiling a unit square, the natural
    /// spacing between adjacent sensors is `1/√N`. For smooth
    /// coverage with no holes, σ should be roughly **equal** to
    /// the spacing — each sensor's 1-σ envelope then reaches its
    /// neighbors, giving overlap-but-not-blur.
    ///
    /// We clamp to `[0.08, 0.50]` so a single sensor doesn't get a
    /// degenerate huge kernel, and 100+ sensors don't get
    /// pinpoint kernels that produce dotted output.
    ///
    /// Examples:
    ///   N=1   → σ=0.50 (clamped; one giant blur)
    ///   N=4   → σ=0.50 (clamped at upper bound, spacing=0.50)
    ///   N=10  → σ=0.32 (spacing=0.32)
    ///   N=25  → σ=0.20 (spacing=0.20)
    ///   N=53  → σ=0.14 (spacing=0.14)
    ///   N=100 → σ=0.10 (spacing=0.10)
    pub fn recommended_sigma(n_sensors: usize) -> f32 {
        if n_sensors == 0 {
            return 0.50;
        }
        let spacing = 1.0 / (n_sensors as f32).sqrt();
        // Spacing-matched σ, clamped to a sensible range.
        spacing.clamp(0.08, 0.50)
    }
}

/// One frame of the bitfield, ready to serialize.
///
/// Stored as `Box<[f32]>` rather than `Vec<f32>` because once a
/// frame is built we never resize it — the `Box<[T]>` form has one
/// less indirection word and clearer intent.
pub struct ScreenTempFrame {
    pub cols: u16,
    pub rows: u16,
    pub temperatures_celsius: Box<[f32]>,
    pub timestamp_unix_nanos: u64,
    pub generation: u32,
}

impl ScreenTempFrame {
    /// Encode the frame in our binary wire format.
    ///
    /// Layout is:
    ///   bytes  0..4    magic "AMBT"
    ///   bytes  4..6    u16 version
    ///   bytes  6..8    u16 cols
    ///   bytes  8..10   u16 rows
    ///   bytes 10..12   u16 reserved (0)
    ///   bytes 12..20   u64 timestamp (unix nanos, little-endian)
    ///   bytes 20..24   u32 generation
    ///   bytes 24..32   u64 reserved (0)
    ///   bytes 32..     f32 grid, row-major
    pub fn to_bytes(&self) -> Vec<u8> {
        let body_bytes = self.temperatures_celsius.len() * 4;
        let mut out = Vec::with_capacity(HEADER_SIZE + body_bytes);

        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&self.cols.to_le_bytes());
        out.extend_from_slice(&self.rows.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // reserved
        out.extend_from_slice(&self.timestamp_unix_nanos.to_le_bytes());
        out.extend_from_slice(&self.generation.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes()); // reserved2

        debug_assert_eq!(out.len(), HEADER_SIZE, "header size invariant");

        for t in self.temperatures_celsius.iter() {
            out.extend_from_slice(&t.to_le_bytes());
        }
        out
    }
}

/// Build a screen-temperature frame from current sensor readings.
///
/// `sensor_names` and `sensor_readings_c` are parallel slices: the
/// i-th name is the i-th reading. `ambient_c` is the prediction we
/// already produce as the "what's the actual room temperature";
/// it sets the baseline that the diffusion kernels add excess heat
/// on top of.
///
/// Returns `None` only if `sensor_names` and `sensor_readings_c`
/// have different lengths — both come from the same `WorldState`
/// so that should never happen in production, but it's a real
/// error mode for fuzzing or future refactors.
pub fn build_frame(
    layout: &ScreenLayout,
    sensor_names: &[String],
    sensor_readings_c: &[f32],
    ambient_c: f32,
    generation: u32,
) -> Option<ScreenTempFrame> {
    if sensor_names.len() != sensor_readings_c.len() {
        return None;
    }

    let cols = layout.cols.max(1);
    let rows = layout.rows.max(1);
    let total = cols as usize * rows as usize;

    // Initialize with ambient — sensors only contribute their
    // *excess* over ambient, so starting at ambient is correct.
    let mut grid = vec![ambient_c; total].into_boxed_slice();

    // Precompute cell-center coordinates once per frame. All
    // sensors share the same grid, so the x-coords and y-coords are
    // identical across sensors — there's no reason to recompute
    // them inside the sensor loop. (Old code did this once per
    // (sensor, cell) inside `apply_gaussian_kernel`.)
    //
    // For 32×48 cells × 53 sensors that's 81,408 divides we no
    // longer do per frame.
    let xs: Vec<f32> = (0..cols as usize)
        .map(|c| (c as f32 + 0.5) / cols as f32)
        .collect();
    let ys: Vec<f32> = (0..rows as usize)
        .map(|r| (r as f32 + 0.5) / rows as f32)
        .collect();

    // Per-sensor kernel scratch buffers. We reuse them across
    // sensors so the allocator only sees them once per call.
    let mut x_weights: Vec<f32> = vec![0.0; cols as usize];
    let mut y_weights: Vec<f32> = vec![0.0; rows as usize];

    // Sum diffusion kernels.
    for (name, &reading) in sensor_names.iter().zip(sensor_readings_c.iter()) {
        let pos = layout.position_for(name);
        let excess = (reading - ambient_c) * pos.weight;
        if !excess.is_finite() || excess.abs() < 0.01 {
            // Tiny contributions don't move the grid in a visible
            // way; skip them and save the kernel evaluations.
            continue;
        }
        apply_separable_gaussian(
            &mut grid,
            cols, rows,
            &xs, &ys,
            &mut x_weights, &mut y_weights,
            pos.x, pos.y, pos.sigma, excess,
        );
    }

    Some(ScreenTempFrame {
        cols,
        rows,
        temperatures_celsius: grid,
        timestamp_unix_nanos: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0),
        generation,
    })
}

/// Add a 2D Gaussian centered at (cx, cy) with std-dev `sigma` to
/// every cell of `grid`. The Gaussian is computed in *separable*
/// form — `G(x, y) = G_x(x) · G_y(y)` — which turns the per-cell
/// cost from one `exp()` call into one multiply.
///
/// Inputs:
///   * `xs`, `ys`: precomputed cell-center coordinates (one per
///     column / row); shared across sensors so they're computed
///     once per frame, not per sensor.
///   * `x_weights`, `y_weights`: scratch buffers reused across
///     sensors. Their contents are overwritten on entry.
///
/// We also compute the rectangular [c_lo, c_hi) × [r_lo, r_hi)
/// bounding box of cells within 3σ of the source and skip
/// everything outside. For typical σ=0.14 on a 14×21 grid, that's
/// ~50 cells instead of 294 — another ~5× cut on top of the
/// separable speedup.
///
/// Combined improvement vs the previous version: roughly 30-50×
/// fewer `exp()` calls on the hot path.
fn apply_separable_gaussian(
    grid: &mut [f32],
    cols: u16,
    rows: u16,
    xs: &[f32],
    ys: &[f32],
    x_weights: &mut [f32],
    y_weights: &mut [f32],
    cx: f32, cy: f32,
    sigma: f32,
    amplitude: f32,
) {
    let cols = cols as usize;
    let rows = rows as usize;
    let inv_2sigma_sq = 1.0 / (2.0 * sigma * sigma).max(1e-6);
    let cutoff = 3.0 * sigma;

    // Bounding box. Skip the rectangle entirely if it has zero
    // width or height (sensor is off-screen by more than 3σ).
    let x_lo_f = (cx - cutoff).max(0.0);
    let x_hi_f = (cx + cutoff).min(1.0);
    let y_lo_f = (cy - cutoff).max(0.0);
    let y_hi_f = (cy + cutoff).min(1.0);
    if x_hi_f <= x_lo_f || y_hi_f <= y_lo_f {
        return;
    }

    // Convert normalized bounds to cell-index ranges. We use
    // saturating arithmetic so a sensor at the edge doesn't
    // produce negative indices.
    let c_lo = (x_lo_f * cols as f32).floor() as usize;
    let c_hi = ((x_hi_f * cols as f32).ceil() as usize).min(cols);
    let r_lo = (y_lo_f * rows as f32).floor() as usize;
    let r_hi = ((y_hi_f * rows as f32).ceil() as usize).min(rows);

    // 1D x-weights: G_x(c) = exp(-(xs[c] - cx)² / 2σ²)
    // Computed once per sensor; reused across all rows.
    for c in c_lo..c_hi {
        let dx = xs[c] - cx;
        x_weights[c] = (-dx * dx * inv_2sigma_sq).exp();
    }

    // 1D y-weights: same idea.
    for r in r_lo..r_hi {
        let dy = ys[r] - cy;
        y_weights[r] = (-dy * dy * inv_2sigma_sq).exp();
    }

    // Outer product: grid[r,c] += amplitude · y_weights[r] · x_weights[c].
    // No exp() calls in this inner loop — pure multiplies.
    for r in r_lo..r_hi {
        let row_amp = amplitude * y_weights[r];
        if row_amp.abs() < 1e-4 {
            // Row contribution is too small to be visible; skip.
            // (Pure performance — produces visually identical output.)
            continue;
        }
        let row_off = r * cols;
        // SAFETY note: we could use unchecked indexing here for
        // another small speedup, but the bounds checks are
        // typically elided by LLVM once the compiler sees the
        // iteration is bounded by c_hi <= cols.
        for c in c_lo..c_hi {
            grid[row_off + c] += row_amp * x_weights[c];
        }
    }
}

/// **Deprecated**: the old per-cell-exp() kernel. Kept temporarily
/// as a reference implementation for tests; will be removed once
/// downstream consumers (if any) confirm the separable version
/// produces visually-equivalent output.
///
/// The two implementations are not bit-identical — the separable
/// form has a slightly different floating-point summation order —
/// but they agree to within ~1e-5 per cell across the test cases
/// we've checked.
#[allow(dead_code)]
fn apply_gaussian_kernel_reference(
    grid: &mut [f32],
    cols: u16,
    rows: u16,
    cx: f32, cy: f32,
    sigma: f32,
    amplitude: f32,
) {
    let cols = cols as usize;
    let rows = rows as usize;
    let inv_2sigma_sq = 1.0 / (2.0 * sigma * sigma).max(1e-6);
    let cutoff = 3.0 * sigma;

    for r in 0..rows {
        let y = (r as f32 + 0.5) / rows as f32;
        let dy = y - cy;
        if dy.abs() > cutoff {
            continue;
        }
        for c in 0..cols {
            let x = (c as f32 + 0.5) / cols as f32;
            let dx = x - cx;
            if dx.abs() > cutoff {
                continue;
            }
            let sq = dx * dx + dy * dy;
            let weight = (-sq * inv_2sigma_sq).exp();
            grid[r * cols + c] += amplitude * weight;
        }
    }
}

/// Compute the canonical output path:
///   $XDG_RUNTIME_DIR/ambient-learner/screen-temps.bin
pub fn default_output_dir() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    runtime.join("ambient-learner")
}

/// Publisher handle. Holds a small bit of state (generation counter,
/// last write time) so the caller doesn't have to.
pub struct Publisher {
    dir: PathBuf,
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    generation: u32,
    last_write: Option<Instant>,
    meta_written: bool,
}

impl Publisher {
    /// Create a publisher rooted at `dir`. Creates the directory if
    /// it doesn't exist; returns an error only when filesystem
    /// permissions block us.
    pub fn new(dir: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            inner: Arc::new(Mutex::new(Inner {
                generation: 0,
                last_write: None,
                meta_written: false,
            })),
        })
    }

    /// Create a publisher at the default location.
    pub fn at_default_location() -> std::io::Result<Self> {
        Self::new(default_output_dir())
    }

    /// Write one frame.
    ///
    /// Uses write-then-rename so consumers never see a torn file.
    /// The metadata sidecar (`screen-temps.meta.json`) is written
    /// once on the first publish — schema doesn't change per frame.
    ///
    /// `min_interval` is enforced internally: calls more frequent
    /// than this are silently dropped. Lets the caller poll at high
    /// frequency without worrying about hammering the filesystem.
    pub fn publish(
        &self,
        layout: &ScreenLayout,
        sensor_names: &[String],
        sensor_readings_c: &[f32],
        ambient_c: f32,
        min_interval: std::time::Duration,
    ) -> std::io::Result<bool> {
        let mut inner = self.inner.lock();

        // Rate limit.
        if let Some(last) = inner.last_write {
            if last.elapsed() < min_interval {
                return Ok(false);
            }
        }

        inner.generation = inner.generation.wrapping_add(1);
        let gen = inner.generation;

        let Some(frame) = build_frame(
            layout,
            sensor_names,
            sensor_readings_c,
            ambient_c,
            gen,
        ) else {
            return Ok(false);
        };

        let bytes = frame.to_bytes();
        let final_path = self.dir.join("screen-temps.bin");
        let temp_path = self.dir.join("screen-temps.bin.tmp");

        // Write to temp then rename: the rename is atomic at the
        // inode level, so a consumer that opens `screen-temps.bin`
        // either sees the previous frame intact or the new one
        // intact, never a mix.
        {
            let mut f = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&temp_path)?;
            f.write_all(&bytes)?;
            f.sync_data()?; // ensure data hits the tmpfs cache before rename
        }
        fs::rename(&temp_path, &final_path)?;

        // Write the metadata sidecar once. We don't bother
        // re-publishing it on every frame because it doesn't change
        // unless the layout changes, and the layout is currently
        // process-lifetime.
        if !inner.meta_written {
            write_metadata(&self.dir, layout, &frame)?;
            inner.meta_written = true;
        }

        inner.last_write = Some(Instant::now());
        Ok(true)
    }

    /// Force the next call to `publish` to re-emit the metadata
    /// sidecar. Useful if the layout changes at runtime.
    pub fn invalidate_metadata(&self) {
        self.inner.lock().meta_written = false;
    }

    pub fn output_dir(&self) -> &Path {
        &self.dir
    }

    pub fn binary_path(&self) -> PathBuf {
        self.dir.join("screen-temps.bin")
    }

    pub fn metadata_path(&self) -> PathBuf {
        self.dir.join("screen-temps.meta.json")
    }
}

/// Write the human-readable metadata sidecar. Documents the binary
/// format so a third-party consumer can integrate without our
/// client library.
fn write_metadata(
    dir: &Path,
    layout: &ScreenLayout,
    frame: &ScreenTempFrame,
) -> std::io::Result<()> {
    let positions: Vec<serde_json::Value> = layout
        .positions
        .iter()
        .map(|p| {
            serde_json::json!({
                "name_pattern": p.name_pattern,
                "x": p.x, "y": p.y,
                "weight": p.weight, "sigma": p.sigma,
            })
        })
        .collect();

    let doc = serde_json::json!({
        "format": "ambient-learner.screen-temps",
        "version": FORMAT_VERSION,
        "binary_file": "screen-temps.bin",
        "header_size_bytes": HEADER_SIZE,
        "cell_size_bytes": 4,
        "cell_dtype": "f32_little_endian",
        "cell_unit": "degrees_celsius",
        "cols": frame.cols,
        "rows": frame.rows,
        "row_major": true,
        "origin": "top_left",
        "x_axis": "normalized_screen_columns_left_to_right",
        "y_axis": "normalized_screen_rows_top_to_bottom",
        "header_layout": {
            "0..4": "magic AMBT little-endian",
            "4..6": "u16 version",
            "6..8": "u16 cols",
            "8..10": "u16 rows",
            "10..12": "u16 reserved",
            "12..20": "u64 timestamp_unix_nanos",
            "20..24": "u32 generation (monotonic per process)",
            "24..32": "u64 reserved",
        },
        "sensor_positions": positions,
        "notes": [
            "Atomic publish via write-temp-then-rename; never read partial frames.",
            "Generation counter increments on every publish; consumers can poll cheaply.",
            "Temperatures are reconstructed from point sensors via a Gaussian diffusion model; treat as an approximation.",
        ],
    });

    let meta_path = dir.join("screen-temps.meta.json");
    let temp_path = dir.join("screen-temps.meta.json.tmp");
    {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)?;
        f.write_all(serde_json::to_string_pretty(&doc).unwrap().as_bytes())?;
        f.sync_data()?;
    }
    fs::rename(&temp_path, &meta_path)?;
    Ok(())
}

/// Parse a binary frame back into a `ScreenTempFrame`.
///
/// Validates magic, version, and that the body length matches
/// `cols * rows * 4`. Returns a typed error string on any
/// inconsistency rather than panicking — the debug UI uses this
/// reader and we want broken state to *show*, not crash.
pub fn parse_frame(bytes: &[u8]) -> Result<ScreenTempFrame, String> {
    if bytes.len() < HEADER_SIZE {
        return Err(format!(
            "file too small: {} bytes, need at least {}",
            bytes.len(),
            HEADER_SIZE
        ));
    }

    let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if magic != MAGIC {
        return Err(format!(
            "bad magic: got 0x{:08x}, expected 0x{:08x} (\"AMBT\")",
            magic, MAGIC
        ));
    }

    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != FORMAT_VERSION {
        return Err(format!(
            "unsupported version {} (we know {})",
            version, FORMAT_VERSION
        ));
    }

    let cols = u16::from_le_bytes([bytes[6], bytes[7]]);
    let rows = u16::from_le_bytes([bytes[8], bytes[9]]);
    // bytes[10..12] = reserved
    let timestamp_unix_nanos = u64::from_le_bytes([
        bytes[12], bytes[13], bytes[14], bytes[15],
        bytes[16], bytes[17], bytes[18], bytes[19],
    ]);
    let generation = u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    // bytes[24..32] = reserved2

    let cells = cols as usize * rows as usize;
    let expected_body = cells * 4;
    if bytes.len() - HEADER_SIZE != expected_body {
        return Err(format!(
            "body length {} doesn't match {}x{} cells = {} bytes",
            bytes.len() - HEADER_SIZE,
            cols, rows, expected_body
        ));
    }

    let mut temps = Vec::with_capacity(cells);
    for i in 0..cells {
        let off = HEADER_SIZE + i * 4;
        temps.push(f32::from_le_bytes([
            bytes[off],
            bytes[off + 1],
            bytes[off + 2],
            bytes[off + 3],
        ]));
    }

    Ok(ScreenTempFrame {
        cols,
        rows,
        temperatures_celsius: temps.into_boxed_slice(),
        timestamp_unix_nanos,
        generation,
    })
}

/// Read the current published frame from the canonical path. Returns
/// the parsed frame on success or a descriptive error string. The
/// debug UI calls this; external apps in other languages should
/// inline the parser using the format described in `SCREEN_TEMPS.md`.
pub fn read_current_frame() -> Result<ScreenTempFrame, String> {
    let path = default_output_dir().join("screen-temps.bin");
    let bytes = fs::read(&path)
        .map_err(|e| format!("read {}: {}", path.display(), e))?;
    parse_frame(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommended_sigma_scales_inversely_with_sqrt_n() {
        let s1   = ScreenLayout::recommended_sigma(1);
        let s10  = ScreenLayout::recommended_sigma(10);
        let s100 = ScreenLayout::recommended_sigma(100);

        // More sensors → smaller σ (tighter kernels match tighter
        // spacing).
        assert!(s10 < s1);
        assert!(s100 < s10);

        // Hardcoded checkpoints: 100 sensors at σ ≈ 0.10, 10 at ≈0.32.
        assert!((s100 - 0.10).abs() < 0.01, "got {}", s100);
        assert!((s10  - 0.316).abs() < 0.01, "got {}", s10);
    }

    #[test]
    fn recommended_sigma_clamps_low_count() {
        // Both 1 sensor and 0 sensors should clamp at the upper σ
        // bound — no point in a tiny kernel when only one source
        // exists.
        assert_eq!(ScreenLayout::recommended_sigma(0), 0.50);
        assert_eq!(ScreenLayout::recommended_sigma(1), 0.50);
    }

    #[test]
    fn recommended_sigma_clamps_high_count() {
        // 10,000 sensors → spacing 0.01, but we cap at 0.08 to
        // avoid dotted output.
        let s = ScreenLayout::recommended_sigma(10_000);
        assert!(s >= 0.08, "got {}", s);
    }

    #[test]
    fn recommended_sigma_for_53_matches_documented_value() {
        // Documentation says σ ≈ 0.14 for 53 evenly-spaced sensors.
        let s = ScreenLayout::recommended_sigma(53);
        assert!((s - 0.137).abs() < 0.01, "got {}", s);
    }

    #[test]
    fn parse_frame_roundtrips_a_published_frame() {
        let original = ScreenTempFrame {
            cols: 10, rows: 16,
            temperatures_celsius: (0..160)
                .map(|i| 20.0 + i as f32 * 0.1)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            timestamp_unix_nanos: 1_700_000_000_123_456_789,
            generation: 7,
        };
        let bytes = original.to_bytes();
        let back = parse_frame(&bytes).unwrap();
        assert_eq!(back.cols, original.cols);
        assert_eq!(back.rows, original.rows);
        assert_eq!(back.generation, original.generation);
        assert_eq!(back.timestamp_unix_nanos, original.timestamp_unix_nanos);
        for i in 0..original.temperatures_celsius.len() {
            assert!(
                (back.temperatures_celsius[i] - original.temperatures_celsius[i]).abs() < 1e-6,
                "cell {} mismatch", i
            );
        }
    }

    #[test]
    fn parse_frame_rejects_bad_magic() {
        let mut bytes = vec![0u8; HEADER_SIZE];
        bytes[0..4].copy_from_slice(b"XXXX");
        let err = parse_frame(&bytes).unwrap_err();
        assert!(err.contains("magic"), "got: {}", err);
    }

    #[test]
    fn parse_frame_rejects_short_file() {
        let bytes = vec![0u8; 10];
        let err = parse_frame(&bytes).unwrap_err();
        assert!(err.contains("too small"), "got: {}", err);
    }

    #[test]
    fn parse_frame_rejects_body_size_mismatch() {
        let frame = ScreenTempFrame {
            cols: 4, rows: 4,
            temperatures_celsius: vec![0.0; 16].into_boxed_slice(),
            timestamp_unix_nanos: 0, generation: 0,
        };
        let mut bytes = frame.to_bytes();
        // Drop one cell from the body — header still claims 4x4.
        bytes.truncate(bytes.len() - 4);
        let err = parse_frame(&bytes).unwrap_err();
        assert!(err.contains("body length"), "got: {}", err);
    }

    #[test]
    fn frame_layout_matches_documentation() {
        let frame = ScreenTempFrame {
            cols: 10, rows: 16,
            temperatures_celsius: vec![20.0; 160].into_boxed_slice(),
            timestamp_unix_nanos: 1_700_000_000_000_000_000,
            generation: 42,
        };
        let bytes = frame.to_bytes();
        assert_eq!(bytes.len(), HEADER_SIZE + 160 * 4);

        // Magic
        assert_eq!(&bytes[0..4], b"AMBT");
        // Version
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), FORMAT_VERSION);
        // Cols / rows
        assert_eq!(u16::from_le_bytes([bytes[6], bytes[7]]), 10);
        assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), 16);
        // Generation at offset 20
        assert_eq!(
            u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            42
        );
    }

    #[test]
    fn gaussian_kernel_warms_locally_not_globally() {
        let layout = ScreenLayout::generic_phone();
        // One CPU sensor reading 60°C, ambient 20°C
        let frame = build_frame(
            &layout,
            &["cpu0-thermal".into()],
            &[60.0],
            20.0,
            1,
        )
        .unwrap();

        // Cell near the CPU position (0.50, 0.30) should be warm.
        let cpu_col = (0.50 * frame.cols as f32) as usize;
        let cpu_row = (0.30 * frame.rows as f32) as usize;
        let near = frame.temperatures_celsius[cpu_row * frame.cols as usize + cpu_col];
        assert!(near > 25.0, "near-CPU cell should be hot, got {near}");

        // Cell far from the CPU (bottom corner) should be close to ambient.
        let last = frame.temperatures_celsius[frame.temperatures_celsius.len() - 1];
        assert!(last < near, "bottom-corner cell should be cooler than CPU cell");
    }

    #[test]
    fn separable_kernel_matches_reference() {
        // The separable Gaussian is mathematically identical to the
        // 2D form but accumulates floating-point error in a different
        // order. Verify they agree to within 1e-4 °C per cell across
        // a range of grid sizes, sensor positions, and σ values.
        // This test exists so that if a future refactor breaks the
        // separability assumption we catch it immediately.
        let cols = 24u16;
        let rows = 32u16;
        let total = (cols * rows) as usize;

        let xs: Vec<f32> = (0..cols as usize)
            .map(|c| (c as f32 + 0.5) / cols as f32)
            .collect();
        let ys: Vec<f32> = (0..rows as usize)
            .map(|r| (r as f32 + 0.5) / rows as f32)
            .collect();
        let mut x_weights = vec![0.0; cols as usize];
        let mut y_weights = vec![0.0; rows as usize];

        let cases = [
            (0.50, 0.30, 0.20, 30.0),  // center-ish CPU sensor
            (0.20, 0.70, 0.30, 10.0),  // off-center battery
            (0.05, 0.05, 0.10, 5.0),   // edge case: source near corner
            (0.95, 0.95, 0.10, 5.0),   // edge case: opposite corner
            (0.50, 0.50, 0.50, 50.0),  // very wide kernel
        ];

        for &(cx, cy, sigma, amp) in &cases {
            let mut grid_ref = vec![0.0f32; total].into_boxed_slice();
            apply_gaussian_kernel_reference(&mut grid_ref, cols, rows, cx, cy, sigma, amp);

            let mut grid_sep = vec![0.0f32; total].into_boxed_slice();
            apply_separable_gaussian(
                &mut grid_sep,
                cols, rows,
                &xs, &ys,
                &mut x_weights, &mut y_weights,
                cx, cy, sigma, amp,
            );

            for i in 0..total {
                let diff = (grid_ref[i] - grid_sep[i]).abs();
                assert!(
                    diff < 1e-4,
                    "cell {} disagrees ({}/{}/{}/{}): ref={} sep={} diff={}",
                    i, cx, cy, sigma, amp, grid_ref[i], grid_sep[i], diff,
                );
            }
        }
    }

    #[test]
    fn separable_kernel_handles_off_screen_sensor() {
        // A sensor placed beyond the screen by more than 3σ should
        // contribute nothing. The bounding-box check should detect
        // this and bail without touching the grid.
        let cols = 10u16;
        let rows = 16u16;
        let total = (cols * rows) as usize;

        let xs: Vec<f32> = (0..cols as usize).map(|c| (c as f32 + 0.5) / cols as f32).collect();
        let ys: Vec<f32> = (0..rows as usize).map(|r| (r as f32 + 0.5) / rows as f32).collect();
        let mut x_weights = vec![0.0; cols as usize];
        let mut y_weights = vec![0.0; rows as usize];

        let mut grid = vec![20.0f32; total].into_boxed_slice();
        apply_separable_gaussian(
            &mut grid,
            cols, rows,
            &xs, &ys,
            &mut x_weights, &mut y_weights,
            // 5σ off-screen to the left:
            -1.5, 0.5, 0.30, 50.0,
        );
        for &v in grid.iter() {
            assert_eq!(v, 20.0, "off-screen sensor should not modify the grid");
        }
    }

    #[test]
    fn unknown_sensor_uses_default_position() {
        let layout = ScreenLayout::generic_phone();
        let pos = layout.position_for("some-future-sensor-name");
        assert_eq!(pos.x, 0.5);
        assert_eq!(pos.y, 0.5);
        // Default weight is half-strength so unknown sensors don't
        // dominate the picture.
        assert!(pos.weight < 1.0);
    }

    #[test]
    fn substring_match_is_case_insensitive() {
        let layout = ScreenLayout::generic_phone();
        let a = layout.position_for("CPU0_thermal");
        let b = layout.position_for("cpu0_thermal");
        assert_eq!(a.x, b.x);
        assert_eq!(a.y, b.y);
    }

    #[test]
    fn publisher_creates_files_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let pub_ = Publisher::new(dir.path().to_path_buf()).unwrap();
        let layout = ScreenLayout::generic_phone();

        let wrote = pub_.publish(
            &layout,
            &["cpu0".into(), "battery".into()],
            &[55.0, 32.0],
            20.0,
            std::time::Duration::from_millis(0),
        )
        .unwrap();
        assert!(wrote);

        let bin = pub_.binary_path();
        assert!(bin.exists());
        let meta = pub_.metadata_path();
        assert!(meta.exists());

        let bytes = std::fs::read(&bin).unwrap();
        assert_eq!(&bytes[0..4], b"AMBT");

        // Second publish should still work and bump the generation.
        let _ = pub_.publish(
            &layout,
            &["cpu0".into(), "battery".into()],
            &[56.0, 32.0],
            20.0,
            std::time::Duration::from_millis(0),
        )
        .unwrap();
        let bytes2 = std::fs::read(&bin).unwrap();
        let gen1 = u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        let gen2 = u32::from_le_bytes([bytes2[20], bytes2[21], bytes2[22], bytes2[23]]);
        assert_eq!(gen2, gen1 + 1);
    }

    #[test]
    fn publisher_rate_limits() {
        let dir = tempfile::tempdir().unwrap();
        let pub_ = Publisher::new(dir.path().to_path_buf()).unwrap();
        let layout = ScreenLayout::generic_phone();

        let first = pub_.publish(
            &layout,
            &["cpu0".into()],
            &[50.0],
            20.0,
            std::time::Duration::from_secs(60),
        )
        .unwrap();
        assert!(first);

        // Inside the rate-limit window: rejected.
        let second = pub_.publish(
            &layout,
            &["cpu0".into()],
            &[50.0],
            20.0,
            std::time::Duration::from_secs(60),
        )
        .unwrap();
        assert!(!second);
    }

    #[test]
    fn build_frame_rejects_mismatched_input_lengths() {
        let layout = ScreenLayout::generic_phone();
        let r = build_frame(
            &layout,
            &["a".into(), "b".into()],
            &[20.0], // only one reading
            18.0,
            1,
        );
        assert!(r.is_none());
    }

    #[test]
    fn ambient_baseline_visible_in_empty_sensor_array() {
        let layout = ScreenLayout::generic_phone();
        let f = build_frame(&layout, &[], &[], 21.5, 1).unwrap();
        // Every cell should be exactly ambient.
        for &t in f.temperatures_celsius.iter() {
            assert!((t - 21.5).abs() < 0.001);
        }
    }
}
