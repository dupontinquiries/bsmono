//! Character drawing on the spacebar.
//!
//! When CHARACTER_DRAW_MODE is on, swipes starting on the spacebar
//! accumulate into a stroke buffer. The user can draw multi-stroke
//! characters (e.g. χ has 2 strokes; ξ has 1; ∑ has 1; ∮ has 2)
//! and finalize with a tap-and-hold on the spacebar. The strokes
//! are then matched against a built-in template database.
//!
//! ## Design
//!
//! ### Storage
//! A process-wide `Mutex<StrokeBuffer>` holds the in-progress drawing.
//! Each stroke is a Vec<(x, y)> in normalized 0..1 coordinates.
//! Coordinates are normalized at drag-end time using the bounding
//! box of the accumulated strokes-so-far so the whole drawing
//! stays inside [0,1]² regardless of where on the spacebar the
//! user drew it.
//!
//! ### Recognition (v0)
//! Simple "1-NN with elastic distance" matching:
//!   1. Resample each input stroke to 32 evenly-spaced points
//!   2. For each template character, do the same resampling
//!   3. Compute per-stroke point-pair Euclidean distance
//!      (sum over all strokes; stroke order matters in v0)
//!   4. Return the template with the smallest total distance
//!
//! Templates are stored as `(name: &str, char: &str, strokes:
//! Vec<Vec<(f64, f64)>>)` — same format as the input. A small
//! built-in set covers Greek lowercase + common math operators.
//!
//! ### Future
//!   - Stroke-order invariance: try all permutations of templates
//!     up to ~3-4 strokes
//!   - Submit to remote OCR if local match score is poor
//!   - Per-user training: append user-drawn strokes to a
//!     fine-tuning corpus

use parking_lot::Mutex;

/// One stroke = an ordered list of points in 0..1 coordinates.
pub type Stroke = Vec<(f64, f64)>;

/// All strokes drawn since the last finalize/clear.
#[derive(Debug, Default, Clone)]
pub struct StrokeBuffer {
    pub strokes: Vec<Stroke>,
}

impl StrokeBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_stroke(&mut self, stroke: Stroke) {
        if stroke.is_empty() {
            return;
        }
        self.strokes.push(stroke);
    }

    pub fn clear(&mut self) {
        self.strokes.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty()
    }

    /// Total number of points across all strokes.
    pub fn total_points(&self) -> usize {
        self.strokes.iter().map(|s| s.len()).sum()
    }

    /// Compute the bounding box of all points across all strokes.
    /// Returns None if there are no points.
    pub fn bbox(&self) -> Option<(f64, f64, f64, f64)> {
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        let mut any = false;
        for stroke in &self.strokes {
            for &(x, y) in stroke {
                any = true;
                if x < min_x {
                    min_x = x;
                }
                if y < min_y {
                    min_y = y;
                }
                if x > max_x {
                    max_x = x;
                }
                if y > max_y {
                    max_y = y;
                }
            }
        }
        if any {
            Some((min_x, min_y, max_x, max_y))
        } else {
            None
        }
    }

    /// Return a normalized copy where the bounding box of all
    /// strokes maps to [0, 1]² (preserving aspect ratio by using
    /// the larger of width/height as the divisor).
    pub fn normalized(&self) -> StrokeBuffer {
        let bbox = match self.bbox() {
            Some(b) => b,
            None => return self.clone(),
        };
        let (min_x, min_y, max_x, max_y) = bbox;
        let w = (max_x - min_x).max(1e-6);
        let h = (max_y - min_y).max(1e-6);
        let scale = w.max(h);
        // Center the smaller axis so the drawing isn't visually
        // squashed against one side.
        let pad_x = (scale - w) * 0.5;
        let pad_y = (scale - h) * 0.5;
        let strokes = self
            .strokes
            .iter()
            .map(|s| {
                s.iter()
                    .map(|&(x, y)| ((x - min_x + pad_x) / scale, (y - min_y + pad_y) / scale))
                    .collect()
            })
            .collect();
        StrokeBuffer { strokes }
    }
}

/// Light smoothing: 3-point moving average across the stroke.
/// Reduces touchscreen jitter and finger tremor without
/// significantly distorting the shape. Endpoints are left in place
/// since they often carry meaningful features (e.g. the tail of γ).
fn smooth(stroke: &Stroke) -> Stroke {
    if stroke.len() < 3 {
        return stroke.clone();
    }
    let mut out: Stroke = Vec::with_capacity(stroke.len());
    out.push(stroke[0]);
    for i in 1..stroke.len() - 1 {
        let (x0, y0) = stroke[i - 1];
        let (x1, y1) = stroke[i];
        let (x2, y2) = stroke[i + 1];
        out.push(((x0 + x1 + x2) / 3.0, (y0 + y1 + y2) / 3.0));
    }
    out.push(*stroke.last().unwrap());
    out
}

/// Resample a stroke to exactly `n` evenly-spaced points along its
/// path. Returns the original if it has fewer than 2 points. Uses
/// linear interpolation between consecutive points. Internally
/// applies a light smoothing pass first.
fn resample(stroke: &Stroke, n: usize) -> Stroke {
    let stroke = smooth(stroke);
    let stroke = &stroke;
    if stroke.len() < 2 || n < 2 {
        return stroke.clone();
    }
    // Total arc length of the stroke.
    let mut seg_lens: Vec<f64> = Vec::with_capacity(stroke.len() - 1);
    let mut total_len = 0.0;
    for w in stroke.windows(2) {
        let dx = w[1].0 - w[0].0;
        let dy = w[1].1 - w[0].1;
        let len = (dx * dx + dy * dy).sqrt();
        seg_lens.push(len);
        total_len += len;
    }
    if total_len < 1e-9 {
        return vec![stroke[0]; n];
    }
    let step = total_len / (n - 1) as f64;
    let mut result: Stroke = Vec::with_capacity(n);
    result.push(stroke[0]);
    let mut accumulated = 0.0;
    let mut target = step;
    let mut i = 0;
    while result.len() < n - 1 && i < seg_lens.len() {
        let seg_len = seg_lens[i];
        if seg_len < 1e-9 {
            i += 1;
            continue;
        }
        if accumulated + seg_len >= target {
            let t = (target - accumulated) / seg_len;
            let x = stroke[i].0 + (stroke[i + 1].0 - stroke[i].0) * t;
            let y = stroke[i].1 + (stroke[i + 1].1 - stroke[i].1) * t;
            result.push((x, y));
            target += step;
        } else {
            accumulated += seg_len;
            i += 1;
        }
    }
    while result.len() < n {
        result.push(*stroke.last().unwrap());
    }
    result
}

/// One template character in the database.
pub struct Template {
    pub display: &'static str,
    pub name: &'static str,
    pub strokes: Vec<Stroke>,
}

/// Match `input` against the template database; return up to N
/// best matches sorted by descending score.
///
/// Scoring policy:
///   - Strokes must match in count (v0 doesn't try permutations).
///   - Each stroke is resampled to 32 points, then per-point
///     Euclidean distance summed.
///   - Score = max(0, 1 - mean_distance / 0.5)
///     (a mean per-point distance of 0.5 in normalized space is
///      essentially uncorrelated and rated 0)
pub fn recognize_candidates(input: &StrokeBuffer, top_n: usize) -> Vec<(String, f64)> {
    let normalized = input.normalized();
    if normalized.is_empty() {
        return Vec::new();
    }
    let resampled_input: Vec<Stroke> = normalized
        .strokes
        .iter()
        .map(|s| resample(s, RESAMPLE_N))
        .collect();

    let mut scored: Vec<(String, f64)> = Vec::new();
    // Built-in templates (resampled at startup, immutable for life
    // of the process).
    for entry in TEMPLATES_RESAMPLED.iter() {
        if let Some(score) = match_score(&resampled_input, &entry.strokes) {
            scored.push((entry.display.clone(), score));
        }
    }
    // User-trained templates. These are loaded once at startup
    // from disk (~/.local/share/rusty-board/user_templates.json)
    // and resampled into USER_TEMPLATES_RESAMPLED. New entries
    // pushed via `train_user_template` also resample on the spot.
    for entry in USER_TEMPLATES_RESAMPLED.lock().iter() {
        if let Some(score) = match_score(&resampled_input, &entry.strokes) {
            // User templates get a slight bonus (+0.05) so a
            // close match against the user's own handwriting
            // wins over a tied match against the built-in.
            scored.push((entry.display.clone(), (score + 0.05).min(1.0)));
        }
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_n);
    scored
}

/// Inner match score: returns Some(score) if stroke counts agree,
/// None otherwise. Factored out so built-in and user templates
/// share the implementation.
fn match_score(
    resampled_input: &[Stroke],
    template_strokes: &[Stroke],
) -> Option<f64> {
    if template_strokes.len() != resampled_input.len() {
        return None;
    }
    let mut total_dist = 0.0;
    let mut total_pts = 0;
    for (a, b) in resampled_input.iter().zip(template_strokes.iter()) {
        for (&(ax, ay), &(bx, by)) in a.iter().zip(b.iter()) {
            let dx = ax - bx;
            let dy = ay - by;
            total_dist += (dx * dx + dy * dy).sqrt();
            total_pts += 1;
        }
    }
    if total_pts == 0 {
        return None;
    }
    let mean = total_dist / total_pts as f64;
    Some((1.0 - mean / 0.5).max(0.0))
}

/// Return the single best match (top-1). Equivalent to
/// `recognize_candidates(input, 1).first().cloned()`.
pub fn recognize(input: &StrokeBuffer) -> Option<(String, f64)> {
    recognize_candidates(input, 1).into_iter().next()
}

const RESAMPLE_N: usize = 32;

/// Pre-resampled version of each template, computed once at first
/// access. Recognition just iterates this — no template resampling
/// on the hot path. Big perf win if the database grows.
struct ResampledTemplate {
    display: String,
    strokes: Vec<Stroke>,
}

static TEMPLATES_RESAMPLED: Lazy<Vec<ResampledTemplate>> = Lazy::new(|| {
    TEMPLATES
        .iter()
        .map(|t| ResampledTemplate {
            display: t.display.to_string(),
            strokes: t.strokes.iter().map(|s| resample(s, RESAMPLE_N)).collect(),
        })
        .collect()
});

/// User-trained templates, mutable at runtime. Populated from
/// disk at startup via `load_user_templates`. Use a parking_lot
/// Mutex<Vec<...>> so we can both read (recognition path) and
/// write (training path) without &mut self requirements.
static USER_TEMPLATES_RESAMPLED: Lazy<Mutex<Vec<ResampledTemplate>>> =
    Lazy::new(|| Mutex::new(Vec::new()));

/// Path where user-trained templates are persisted. Lives in the
/// XDG data dir so it survives across sessions but is per-user.
fn user_templates_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
            home.join(".local/share")
        });
    base.join("rusty-board").join("user_templates.json")
}

/// Serializable form of a user template — same shape as the
/// built-in `Template` but with owned strings/strokes so it can
/// round-trip through serde_json.
#[derive(serde::Serialize, serde::Deserialize)]
struct UserTemplateOnDisk {
    display: String,
    /// Optional human-readable name (e.g. "alpha"); not used for
    /// recognition but useful when debugging the JSON file.
    #[serde(default)]
    name: String,
    /// Strokes as point-pair arrays. Each stroke is a Vec<[f64; 2]>.
    strokes: Vec<Vec<[f64; 2]>>,
}

/// Add a new user template from the current stroke buffer with
/// the given display string. Resamples on the spot and appends
/// to the in-memory database. Persists to disk asynchronously
/// (best-effort — failures are logged but not surfaced to the
/// user, since a successful in-memory add is the user-visible
/// behavior).
///
/// The user can call this via the `/train <char>` slash command
/// after drawing strokes that didn't match well.
pub fn train_user_template(display_str: &str, strokes: &StrokeBuffer) -> Result<(), String> {
    if display_str.is_empty() {
        return Err("empty display string".into());
    }
    if strokes.is_empty() {
        return Err("no strokes to train on".into());
    }
    let normalized = strokes.normalized();
    let resampled: Vec<Stroke> = normalized
        .strokes
        .iter()
        .map(|s| resample(s, RESAMPLE_N))
        .collect();
    // Append in-memory immediately.
    USER_TEMPLATES_RESAMPLED.lock().push(ResampledTemplate {
        display: display_str.to_string(),
        strokes: resampled,
    });
    // Persist to disk. We re-serialize the whole user templates
    // list each time; the file is small (a few KB per character)
    // and writes are infrequent.
    if let Err(e) = save_user_templates() {
        tracing::warn!("character-draw: failed to persist user template: {}", e);
        return Err(format!("saved in memory but not to disk: {}", e));
    }
    tracing::info!(
        "character-draw: trained user template for {:?} ({} strokes)",
        display_str,
        strokes.strokes.len()
    );
    Ok(())
}

/// Save all user-trained templates to disk. Serialized as JSON
/// (one object per template).
fn save_user_templates() -> Result<(), String> {
    let path = user_templates_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    // Pull from the in-memory store. We snapshot under lock then
    // serialize without the lock held.
    let snapshot: Vec<UserTemplateOnDisk> = USER_TEMPLATES_RESAMPLED
        .lock()
        .iter()
        .map(|t| UserTemplateOnDisk {
            display: t.display.clone(),
            name: String::new(),
            strokes: t
                .strokes
                .iter()
                .map(|s| s.iter().map(|&(x, y)| [x, y]).collect())
                .collect(),
        })
        .collect();
    let json = serde_json::to_string_pretty(&snapshot)
        .map_err(|e| format!("serialize: {}", e))?;
    // Atomic write — without this, a crash mid-write would lose
    // ALL trained user templates (the file would be truncated to
    // zero bytes by the open(O_TRUNC) before fresh bytes land).
    // See util::io::atomic_write for the contract.
    crate::util::io::atomic_write(&path, json.as_bytes())
        .map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(())
}

/// Load user templates from disk. Called once at startup. Missing
/// or empty file is not an error; corrupt file logs and continues
/// with empty templates.
pub fn load_user_templates() {
    let path = user_templates_path();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!(
                "character-draw: couldn't read user templates from {}: {}",
                path.display(),
                e
            );
            return;
        }
    };
    let parsed: Vec<UserTemplateOnDisk> = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "character-draw: user templates file is corrupt ({}); ignoring",
                e
            );
            return;
        }
    };
    let mut store = USER_TEMPLATES_RESAMPLED.lock();
    store.clear();
    for t in parsed {
        // Templates on disk are already-resampled (we resample
        // at train time, not load time). But we re-resample
        // here defensively in case the on-disk format gets
        // updated or RESAMPLE_N changes.
        let resampled: Vec<Stroke> = t
            .strokes
            .iter()
            .map(|s| resample(&s.iter().map(|p| (p[0], p[1])).collect::<Stroke>(), RESAMPLE_N))
            .collect();
        store.push(ResampledTemplate {
            display: t.display,
            strokes: resampled,
        });
    }
    tracing::info!(
        "character-draw: loaded {} user templates from {}",
        store.len(),
        path.display()
    );
}

/// Remove all user-trained templates from memory and delete the
/// on-disk store. Used by /forget command and Reset-to-defaults.
pub fn forget_user_templates() {
    USER_TEMPLATES_RESAMPLED.lock().clear();
    let path = user_templates_path();
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!(
                "character-draw: couldn't delete user templates file: {}",
                e
            );
        }
    }
}


// ---- The process-wide stroke buffer -------------------------------

static STROKE_BUFFER: Mutex<StrokeBuffer> = Mutex::new(StrokeBuffer {
    strokes: Vec::new(),
});

static DRAW_MODE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn draw_mode_enabled() -> bool {
    DRAW_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_draw_mode(on: bool) {
    DRAW_MODE.store(on, std::sync::atomic::Ordering::Relaxed);
    if !on {
        clear_strokes();
    }
}

/// One-shot canvas open flag. Set when the user swipes up on the
/// spacebar to invoke a full-keyboard-size drawing canvas. Unlike
/// the persistent `DRAW_MODE` toggle, this exits after a single
/// recognize/cancel. The keyboard_view consults this every drag
/// to decide whether to route ALL touches into the stroke buffer
/// (when CANVAS_OPEN is true, no key taps are processed).
static CANVAS_OPEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn canvas_open() -> bool {
    CANVAS_OPEN.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn open_canvas() {
    CANVAS_OPEN.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn close_canvas() {
    CANVAS_OPEN.store(false, std::sync::atomic::Ordering::Relaxed);
    clear_strokes();
}

pub fn push_stroke(stroke: Stroke) {
    STROKE_BUFFER.lock().push_stroke(stroke);
}

pub fn clear_strokes() {
    STROKE_BUFFER.lock().clear();
}

pub fn current_strokes() -> StrokeBuffer {
    STROKE_BUFFER.lock().clone()
}

/// Finalize: clone the current buffer, clear it, return the buffer
/// for recognition. Kept for API completeness; the spacebar long-
/// press path now uses `current_strokes` + `clear_strokes` so it
/// can keep the buffer alive for /train on low-confidence matches.
#[allow(dead_code)]
pub fn take_strokes() -> StrokeBuffer {
    let mut buf = STROKE_BUFFER.lock();
    let owned = buf.clone();
    buf.clear();
    owned
}

// ---- Template database --------------------------------------------

use once_cell::sync::Lazy;

/// Built-in character templates. Coordinates are in normalized
/// 0..1 space (origin top-left). Each stroke is a list of points
/// approximating how a typical user draws the character; we
/// resample at match time so input doesn't need to match the
/// template's exact sample count.
static TEMPLATES: Lazy<Vec<Template>> = Lazy::new(|| {
    vec![
        // Greek lowercase ---------------------------------------
        Template {
            display: "α",
            name: "alpha",
            strokes: vec![vec![
                // Loop: top-right → down-left → bottom → up to
                // start → right tail.
                (0.7, 0.2),
                (0.3, 0.3),
                (0.2, 0.5),
                (0.3, 0.7),
                (0.5, 0.75),
                (0.7, 0.65),
                (0.75, 0.5),
                (0.7, 0.3),
                (0.7, 0.7),
                (0.9, 0.85),
            ]],
        },
        Template {
            display: "β",
            name: "beta",
            strokes: vec![vec![
                // Vertical down stroke, then bulge.
                (0.3, 0.05),
                (0.3, 0.95),
                (0.3, 0.5),
                (0.6, 0.4),
                (0.7, 0.5),
                (0.6, 0.6),
                (0.3, 0.55),
                (0.65, 0.6),
                (0.75, 0.75),
                (0.6, 0.85),
                (0.3, 0.85),
            ]],
        },
        Template {
            display: "γ",
            name: "gamma",
            strokes: vec![vec![
                (0.1, 0.3),
                (0.4, 0.5),
                (0.5, 0.8),
                (0.55, 1.0),
                (0.5, 0.8),
                (0.6, 0.55),
                (0.9, 0.3),
            ]],
        },
        Template {
            display: "δ",
            name: "delta",
            strokes: vec![vec![
                (0.7, 0.15),
                (0.4, 0.25),
                (0.5, 0.4),
                (0.7, 0.5),
                (0.8, 0.65),
                (0.6, 0.8),
                (0.35, 0.75),
                (0.25, 0.6),
                (0.35, 0.5),
            ]],
        },
        Template {
            display: "ε",
            name: "epsilon",
            strokes: vec![vec![
                (0.75, 0.25),
                (0.4, 0.25),
                (0.25, 0.45),
                (0.55, 0.5),
                (0.25, 0.55),
                (0.4, 0.75),
                (0.75, 0.75),
            ]],
        },
        Template {
            display: "θ",
            name: "theta",
            strokes: vec![
                // Oval
                vec![
                    (0.5, 0.1),
                    (0.2, 0.3),
                    (0.15, 0.5),
                    (0.2, 0.7),
                    (0.5, 0.9),
                    (0.8, 0.7),
                    (0.85, 0.5),
                    (0.8, 0.3),
                    (0.5, 0.1),
                ],
                // Horizontal bar
                vec![(0.25, 0.5), (0.75, 0.5)],
            ],
        },
        Template {
            display: "λ",
            name: "lambda",
            strokes: vec![
                // Down-right diagonal
                vec![(0.3, 0.1), (0.9, 0.95)],
                // Down-left diagonal
                vec![(0.6, 0.5), (0.1, 0.95)],
            ],
        },
        Template {
            display: "μ",
            name: "mu",
            strokes: vec![vec![
                (0.2, 0.25),
                (0.25, 0.7),
                (0.3, 0.85),
                (0.35, 1.0),
                (0.35, 0.85),
                (0.35, 0.5),
                (0.55, 0.4),
                (0.7, 0.45),
                (0.75, 0.65),
                (0.75, 0.85),
                (0.85, 0.85),
            ]],
        },
        Template {
            display: "π",
            name: "pi",
            strokes: vec![
                // Top horizontal
                vec![(0.15, 0.3), (0.85, 0.3)],
                // Left leg
                vec![(0.3, 0.3), (0.3, 0.85)],
                // Right leg
                vec![(0.7, 0.3), (0.7, 0.85)],
            ],
        },
        Template {
            display: "ρ",
            name: "rho",
            strokes: vec![vec![
                (0.3, 1.0),
                (0.3, 0.6),
                (0.4, 0.4),
                (0.6, 0.35),
                (0.75, 0.5),
                (0.75, 0.65),
                (0.6, 0.8),
                (0.4, 0.75),
                (0.3, 0.6),
            ]],
        },
        Template {
            display: "σ",
            name: "sigma",
            strokes: vec![vec![
                (0.85, 0.3),
                (0.5, 0.3),
                (0.3, 0.45),
                (0.3, 0.65),
                (0.5, 0.8),
                (0.7, 0.7),
                (0.7, 0.45),
                (0.5, 0.3),
            ]],
        },
        Template {
            display: "τ",
            name: "tau",
            strokes: vec![
                vec![(0.15, 0.3), (0.85, 0.3)],
                vec![(0.5, 0.3), (0.5, 0.85), (0.7, 0.9)],
            ],
        },
        Template {
            display: "φ",
            name: "phi",
            strokes: vec![
                // Oval
                vec![
                    (0.5, 0.25),
                    (0.3, 0.4),
                    (0.3, 0.65),
                    (0.5, 0.8),
                    (0.7, 0.65),
                    (0.7, 0.4),
                    (0.5, 0.25),
                ],
                // Vertical stem
                vec![(0.5, 0.05), (0.5, 0.95)],
            ],
        },
        Template {
            display: "χ",
            name: "chi",
            strokes: vec![
                vec![(0.15, 0.25), (0.85, 0.95)],
                vec![(0.85, 0.25), (0.15, 0.95)],
            ],
        },
        Template {
            display: "ψ",
            name: "psi",
            strokes: vec![
                // U-shape
                vec![(0.2, 0.3), (0.2, 0.65), (0.5, 0.8), (0.8, 0.65), (0.8, 0.3)],
                // Vertical stem
                vec![(0.5, 0.05), (0.5, 0.95)],
            ],
        },
        Template {
            display: "ω",
            name: "omega",
            strokes: vec![vec![
                (0.1, 0.4),
                (0.15, 0.7),
                (0.3, 0.85),
                (0.4, 0.7),
                (0.4, 0.5),
                (0.5, 0.7),
                (0.6, 0.5),
                (0.6, 0.7),
                (0.7, 0.85),
                (0.85, 0.7),
                (0.9, 0.4),
            ]],
        },
        // Math operators ---------------------------------------
        Template {
            display: "∑",
            name: "sum",
            strokes: vec![vec![
                (0.85, 0.2),
                (0.15, 0.2),
                (0.55, 0.5),
                (0.15, 0.8),
                (0.85, 0.8),
            ]],
        },
        Template {
            display: "∏",
            name: "product",
            strokes: vec![
                vec![(0.1, 0.2), (0.9, 0.2)],
                vec![(0.25, 0.2), (0.25, 0.8)],
                vec![(0.75, 0.2), (0.75, 0.8)],
            ],
        },
        Template {
            display: "∫",
            name: "integral",
            strokes: vec![vec![
                (0.75, 0.1),
                (0.6, 0.15),
                (0.55, 0.3),
                (0.5, 0.5),
                (0.45, 0.7),
                (0.4, 0.85),
                (0.25, 0.9),
            ]],
        },
        Template {
            display: "√",
            name: "sqrt",
            strokes: vec![vec![
                (0.05, 0.5),
                (0.2, 0.45),
                (0.35, 0.85),
                (0.55, 0.2),
                (0.9, 0.2),
            ]],
        },
        Template {
            display: "∞",
            name: "infinity",
            strokes: vec![vec![
                (0.5, 0.5),
                (0.3, 0.35),
                (0.15, 0.5),
                (0.3, 0.65),
                (0.5, 0.5),
                (0.7, 0.35),
                (0.85, 0.5),
                (0.7, 0.65),
                (0.5, 0.5),
            ]],
        },
        Template {
            display: "≈",
            name: "approx",
            strokes: vec![
                vec![(0.15, 0.4), (0.35, 0.3), (0.55, 0.4), (0.85, 0.3)],
                vec![(0.15, 0.7), (0.35, 0.6), (0.55, 0.7), (0.85, 0.6)],
            ],
        },
        Template {
            display: "≠",
            name: "neq",
            strokes: vec![
                vec![(0.15, 0.4), (0.85, 0.4)],
                vec![(0.15, 0.6), (0.85, 0.6)],
                vec![(0.7, 0.2), (0.3, 0.8)],
            ],
        },
        Template {
            display: "≤",
            name: "leq",
            strokes: vec![
                vec![(0.85, 0.25), (0.15, 0.5), (0.85, 0.6)],
                vec![(0.15, 0.85), (0.85, 0.85)],
            ],
        },
        Template {
            display: "≥",
            name: "geq",
            strokes: vec![
                vec![(0.15, 0.25), (0.85, 0.5), (0.15, 0.6)],
                vec![(0.15, 0.85), (0.85, 0.85)],
            ],
        },
        Template {
            display: "→",
            name: "right_arrow",
            strokes: vec![vec![
                (0.1, 0.5),
                (0.9, 0.5),
                (0.7, 0.35),
                (0.9, 0.5),
                (0.7, 0.65),
            ]],
        },
        Template {
            display: "∇",
            name: "nabla",
            strokes: vec![vec![
                (0.1, 0.2),
                (0.9, 0.2),
                (0.5, 0.9),
                (0.1, 0.2),
            ]],
        },
        Template {
            display: "∂",
            name: "partial",
            strokes: vec![vec![
                (0.85, 0.15),
                (0.55, 0.2),
                (0.4, 0.35),
                (0.3, 0.55),
                (0.35, 0.75),
                (0.55, 0.85),
                (0.7, 0.7),
                (0.75, 0.5),
                (0.7, 0.35),
                (0.55, 0.3),
                (0.4, 0.35),
            ]],
        },
    ]
});
