// src/acoustic_index.rs
//
// v0.1.284 (Tier1-1): acoustic exemplar index for the
// upstream-of-whisper recognition cache.
//
// The idea: every utterance the user has already confirmed
// (corpus/<speaker_id>/<unix_ms>.{wav,txt}) gets a compact
// acoustic fingerprint stored in a single packed file. At
// listen time, the new audio is fingerprinted, the nearest
// neighbor is found, and IF the distance is small enough
// AND the speaker matches AND the entry has been confirmed
// often enough, we emit the cached transcript and SKIP
// whisper entirely.
//
// Tier1-1 scope (this ship):
//   - Define on-disk format with version magic
//   - Build the index from corpus/ at startup (background thread)
//   - Expose a query API: find_best(audio, speaker_id) → Option<Hit>
//   - NO wiring into the listen pipeline yet — that's Tier1-2
//   - Persist incremental updates from future Whisper runs
//
// Acoustic fingerprint: MEAN of MFCC frames over the
// utterance. Single 13-dim vector. Captures the spectral
// envelope of phonemes used. Cheap O(13) cosine compare per
// candidate entry. Limitations:
//   - Doesn't capture word ORDER ("scroll up" vs "up scroll"
//     would have similar means; a future DTW pass fixes this)
//   - Sensitive to noise on very short utterances (<0.5s)
//   - Speaker-dependent — same speaker repeating a phrase
//     clusters tightly; cross-speaker matches are noisier
//
// File: ~/.config/web2qml/acoustic_index.bin
// Format:
//   [magic "AI01" 4 bytes]
//   [version u8 = 1]
//   [n_entries u32 LE]
//   [entry × n]
//
// Each entry:
//   [unix_ms u64 LE]
//   [speaker_id_len u8][speaker_id bytes]
//   [transcript_len u16 LE][transcript bytes]
//   [count u32 LE]
//   [last_used_ms u64 LE]
//   [mfcc_mean f32 LE × 13]
//
// Magic + version let future ships add fields (e.g. full MFCC
// matrix for DTW, audio-length, RMS) without breaking the
// reader. Unknown versions fall through to a clean error
// message; tier1 falls back to running Whisper as before.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const FILE_MAGIC: &[u8; 4] = b"AI02";
const FILE_VERSION: u8 = 2;
/// v0.1.647: speaker/utterance embedding dimension. Was 13 (MFCC mean
/// only, format AI01); now 39 = mean(13)+std(13)+delta-mean(13). The
/// loader rejects the old AI01/v1 files on the version check (logs
/// "version mismatch — ignoring" and starts fresh), so there's no mixed-
/// dimension DB; the user's index rebuilds from new utterances.
pub(crate) const EMBED_DIM: usize = 39;

/// Cap per speaker. LRU-evicted by last_used_ms on rebuild.
/// Cap chosen because matching cost is O(N) per query — at
/// 13 floats × 5000 entries × cosine compare, the per-listen
/// overhead is ~150µs, negligible vs Whisper's ~500ms.
const MAX_ENTRIES_PER_SPEAKER: usize = 5000;
const MAX_ENTRIES_TOTAL: usize = 20000;
const MAX_TRANSCRIPT_LEN: usize = 200;
const MAX_SPEAKER_ID_LEN: usize = 255;

/// One indexed exemplar: a fingerprint of a single past
/// utterance plus its known transcript.
#[derive(Clone, Debug)]
pub struct Exemplar {
    pub unix_ms: u64,
    pub speaker_id: String,
    pub transcript: String,
    /// Mean of MFCC frames across the utterance. 13-dim.
    pub mfcc_mean: Vec<f32>,
    /// How many times this exemplar has been matched
    /// successfully (used as a confidence weight; new
    /// exemplars start at 1).
    pub count: u32,
    /// Last time this exemplar was hit by a query, in unix
    /// ms. Used for LRU eviction on rebuild.
    pub last_used_ms: u64,
}

/// One match candidate returned from `find_best`.
#[derive(Clone, Debug)]
pub struct Hit {
    pub transcript: String,
    /// Cosine distance from the query (smaller = closer).
    /// Range 0.0 (identical) to 2.0 (opposite). For
    /// reference: same-utterance same-speaker is ~0.05,
    /// same-word same-speaker ~0.10, cross-speaker ~0.30+.
    pub distance: f32,
    pub speaker_id: String,
    pub unix_ms: u64,
    pub count: u32,
}

/// In-memory index. Loaded once at startup, mutated by
/// add_exemplar / rebuild_from_corpus.
#[derive(Default)]
struct Index {
    entries: Vec<Exemplar>,
}

static INDEX: OnceLock<Mutex<Index>> = OnceLock::new();
static BUILT: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();

fn index() -> &'static Mutex<Index> {
    INDEX.get_or_init(|| Mutex::new(Index::default()))
}

fn built() -> &'static std::sync::atomic::AtomicBool {
    BUILT.get_or_init(|| std::sync::atomic::AtomicBool::new(false))
}

/// Has the initial corpus → index build completed?
/// QML / status code can read this to show a progress
/// indicator if needed.
pub fn is_built() -> bool {
    built().load(std::sync::atomic::Ordering::Relaxed)
}

/// Number of indexed exemplars. 0 before initial build.
pub fn entry_count() -> usize {
    index().lock().map(|g| g.entries.len()).unwrap_or(0)
}

/// v0.1.300 (Tier1-3 tuning): SHADOW-mode distance histogram.
/// Buckets are 0.05-wide on [0.0, 1.0+). Each bucket counts
/// (match_count, diverge_count) for hits that landed there.
/// Used by `record_shadow_observation` and read by
/// `shadow_histogram_summary` for the diagnostic toast.
///
/// 22 buckets total: 0..0.05, 0.05..0.10, ..., 0.95..1.0,
/// 1.0..1.05, 1.05+. Index 21 is the overflow.
const HISTOGRAM_BUCKETS: usize = 22;
const HISTOGRAM_BUCKET_WIDTH: f32 = 0.05;

static SHADOW_MATCH: [std::sync::atomic::AtomicU32; HISTOGRAM_BUCKETS] = [
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
];

static SHADOW_DIVERGE: [std::sync::atomic::AtomicU32; HISTOGRAM_BUCKETS] = [
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
];

fn distance_bucket(d: f32) -> usize {
    if d < 0.0 || !d.is_finite() {
        return 0;
    }
    let idx = (d / HISTOGRAM_BUCKET_WIDTH) as usize;
    if idx >= HISTOGRAM_BUCKETS { HISTOGRAM_BUCKETS - 1 } else { idx }
}

/// v0.1.302 (Tier1-4): runtime PROMOTE state. Initialized
/// from `WEB2QML_TIER1_PROMOTE` env var at first access;
/// can be flipped at runtime via `set_promote_enabled` so
/// the user can promote-from-shadow without restarting.
///
/// Default false (SHADOW). Once flipped on, stays on for
/// the process lifetime unless explicitly turned off.
static PROMOTE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PROMOTE_INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn ensure_promote_initialized() {
    PROMOTE_INIT.get_or_init(|| {
        let from_env = std::env::var("WEB2QML_TIER1_PROMOTE")
            .map(|v| v == "1" || v == "true" || v == "yes")
            .unwrap_or(false);
        PROMOTE.store(from_env, std::sync::atomic::Ordering::Relaxed);
    });
}

/// v0.1.302: query current PROMOTE state. Initialized lazily
/// from env var on first read.
pub fn promote_enabled() -> bool {
    ensure_promote_initialized();
    PROMOTE.load(std::sync::atomic::Ordering::Relaxed)
}

/// v0.1.302: set PROMOTE state explicitly. Use this when
/// the user opts in/out at runtime (e.g., via the "promote"
/// voice intent).
pub fn set_promote_enabled(on: bool) {
    ensure_promote_initialized();
    PROMOTE.store(on, std::sync::atomic::Ordering::Relaxed);
    crate::clog!("tier1", "promote set to {}", on);
}

/// v0.1.304 (Tier1 threshold tuning): runtime PROMOTE
/// threshold for SAME-speaker matches. Stored as
/// (distance * 1000) in an AtomicU32. Default 100 = 0.10.
static PROMOTE_THRESHOLD_SAME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(100);
/// v0.1.304: runtime PROMOTE threshold for CROSS-speaker
/// matches. Stored as (distance * 1000). Default 200 = 0.20.
static PROMOTE_THRESHOLD_CROSS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(200);

/// v0.1.304: get the current PROMOTE thresholds as
/// (same_speaker, cross_speaker) in distance units.
pub fn promote_thresholds() -> (f32, f32) {
    let same = PROMOTE_THRESHOLD_SAME.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
    let cross = PROMOTE_THRESHOLD_CROSS.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
    (same, cross)
}

/// v0.1.304: set the PROMOTE thresholds. Clamped to
/// reasonable bounds (0.0..=1.0). Setting same=0 effectively
/// disables PROMOTE for that case.
pub fn set_promote_thresholds(same_speaker: f32, cross_speaker: f32) {
    let s = (same_speaker.clamp(0.0, 1.0) * 1000.0) as u32;
    let c = (cross_speaker.clamp(0.0, 1.0) * 1000.0) as u32;
    PROMOTE_THRESHOLD_SAME.store(s, std::sync::atomic::Ordering::Relaxed);
    PROMOTE_THRESHOLD_CROSS.store(c, std::sync::atomic::Ordering::Relaxed);
    crate::clog!("tier1", "thresholds set: same={:.3}, cross={:.3}",
        same_speaker, cross_speaker);
}

/// v0.1.300: record a SHADOW-mode observation. Voice path
/// calls this whenever it has a (distance, agreement) pair.
/// Lock-free; uses atomics. matched=true means the cached
/// transcript matched whisper's output for the same audio.
pub fn record_shadow_observation(distance: f32, matched: bool) {
    let bucket = distance_bucket(distance);
    if matched {
        SHADOW_MATCH[bucket].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    } else {
        SHADOW_DIVERGE[bucket].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// v0.1.300: return a compact summary of the SHADOW histogram.
/// Format: "<total> observations; suggested threshold=<X.XX>
/// (95% match below)". Returns "no observations yet" when
/// histogram is empty.
///
/// The "suggested threshold" is the largest bucket boundary
/// where cumulative (matches / (matches+diverges)) is ≥ 0.95.
/// Useful for the user to pick a PROMOTE threshold.
/// v0.1.305: raw shadow histogram data. Returns
/// (total_observations, suggested_threshold_or_neg1,
/// overall_match_rate). suggested_threshold is -1.0 when
/// no bucket reaches the 95% cumulative match rate.
///
/// Used by both the human-readable summary and the
/// auto-tune intent (which needs structured access).
pub fn shadow_histogram_data() -> (u32, f32, f32) {
    let mut total_matches: u32 = 0;
    let mut total_diverges: u32 = 0;
    let mut matches = [0u32; HISTOGRAM_BUCKETS];
    let mut diverges = [0u32; HISTOGRAM_BUCKETS];
    for i in 0..HISTOGRAM_BUCKETS {
        matches[i] = SHADOW_MATCH[i].load(std::sync::atomic::Ordering::Relaxed);
        diverges[i] = SHADOW_DIVERGE[i].load(std::sync::atomic::Ordering::Relaxed);
        total_matches += matches[i];
        total_diverges += diverges[i];
    }
    let total = total_matches + total_diverges;
    if total == 0 {
        return (0, -1.0, 0.0);
    }
    let mut cum_m: u32 = 0;
    let mut cum_d: u32 = 0;
    let mut suggested_thresh: f32 = -1.0;
    let mut found = false;
    for i in 0..HISTOGRAM_BUCKETS {
        cum_m += matches[i];
        cum_d += diverges[i];
        let cum = cum_m + cum_d;
        if cum == 0 { continue; }
        let rate = cum_m as f32 / cum as f32;
        if rate >= 0.95 {
            suggested_thresh = (i + 1) as f32 * HISTOGRAM_BUCKET_WIDTH;
            found = true;
        } else if found {
            break;
        }
    }
    let overall_rate = total_matches as f32 / total as f32;
    (total, suggested_thresh, overall_rate)
}

/// v0.1.300: return a compact summary of the SHADOW histogram.
/// Format: "<total> observations; suggested threshold=<X.XX>
/// (95% match below)". Returns "no observations yet" when
/// histogram is empty.
///
/// The "suggested threshold" is the largest bucket boundary
/// where cumulative (matches / (matches+diverges)) is ≥ 0.95.
/// Useful for the user to pick a PROMOTE threshold.
pub fn shadow_histogram_summary() -> String {
    let (total, suggested, overall_rate) = shadow_histogram_data();
    if total == 0 {
        return "no observations yet (run in SHADOW mode to collect)".to_string();
    }
    if suggested >= 0.0 {
        format!("{} obs ({:.0}% match overall); suggested threshold={:.2}",
            total, overall_rate * 100.0, suggested)
    } else {
        format!("{} obs ({:.0}% match overall); no threshold reaches 95%",
            total, overall_rate * 100.0)
    }
}

/// Compute the acoustic fingerprint for a single utterance.
/// v0.1.647: richer speaker/utterance embedding — 39-dim:
/// [ mean(13) | std-dev(13) | delta-mean(13) ]. The old 13-dim MEAN threw
/// away within-utterance variance and spectral DYNAMICS, the cues that
/// most separate speakers — measured on synthetic voices, the mean gave
/// only a 1.10× cross/same separation ratio (near-useless), while this
/// 39-dim form gave ~20×. Better separation → less shadow divergence →
/// the whisper-skip fast path can eventually turn on, and cleaner speaker
/// attribution. Still pure + cheap (39 floats, µs cosine). File format is
/// bumped AI01→AI02 so old 13-dim DBs fall back cleanly (length mismatch
/// in cosine_distance → +inf → no false match → whisper runs as before).
/// Returns an empty Vec if the audio is too short to MFCC.
pub fn fingerprint(audio_16k_mono: &[f32]) -> Vec<f32> {
    let mfcc = crate::phonetic::compute_mfcc(audio_16k_mono);
    let n_mfcc = crate::phonetic::N_MFCC;
    if mfcc.is_empty() || mfcc.len() % n_mfcc != 0 {
        return Vec::new();
    }
    let n_frames = mfcc.len() / n_mfcc;
    // mean
    let mut mean = vec![0.0f32; n_mfcc];
    for f in 0..n_frames {
        for c in 0..n_mfcc {
            mean[c] += mfcc[f * n_mfcc + c];
        }
    }
    let inv = 1.0 / (n_frames as f32);
    for v in &mut mean { *v *= inv; }
    // std-dev (population) — within-utterance spectral spread
    let mut sd = vec![0.0f32; n_mfcc];
    for f in 0..n_frames {
        for c in 0..n_mfcc {
            let d = mfcc[f * n_mfcc + c] - mean[c];
            sd[c] += d * d;
        }
    }
    for v in &mut sd { *v = (*v * inv).sqrt(); }
    // delta-mean — average absolute frame-to-frame change (dynamics)
    let mut dmean = vec![0.0f32; n_mfcc];
    if n_frames > 1 {
        for f in 1..n_frames {
            for c in 0..n_mfcc {
                dmean[c] += (mfcc[f * n_mfcc + c] - mfcc[(f - 1) * n_mfcc + c]).abs();
            }
        }
        let invd = 1.0 / ((n_frames - 1) as f32);
        for v in &mut dmean { *v *= invd; }
    }
    let mut out = mean;
    out.extend(sd);
    out.extend(dmean);
    out // 39-dim
}

/// Cosine distance between two fingerprints (lower = closer).
/// Returns +inf for mismatched lengths or zero-magnitude vecs.
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() { return f32::INFINITY; }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if !na.is_finite() || !nb.is_finite() || na <= 0.0 || nb <= 0.0 {
        // v0.1.559: NaN norms (from a NaN in the fingerprint) slip past
        // a bare `<= 0.0` since `NaN <= 0.0` is false. Treat any non-
        // finite or zero-norm vector as "no match" (infinite distance)
        // rather than letting a NaN distance poison the nearest-neighbor
        // search. Defense in depth alongside the compute_mfcc floor fix.
        return f32::INFINITY;
    }
    let cos_sim = dot / (na.sqrt() * nb.sqrt());
    let d = 1.0 - cos_sim;
    if d.is_finite() { d } else { f32::INFINITY }
}

/// Find the best-matching exemplar for the given audio,
/// optionally restricted to a speaker. Returns None when the
/// index is empty or no entries exist for the speaker.
///
/// `prefer_speaker_id`: when Some, exemplars from that speaker
/// are searched FIRST. If the best speaker-specific match has
/// distance below `prefer_threshold`, we return it directly.
/// Otherwise we fall back to a global search.
pub fn find_best(
    audio_16k_mono: &[f32],
    prefer_speaker_id: Option<&str>,
) -> Option<Hit> {
    if audio_16k_mono.is_empty() { return None; }
    let query = fingerprint(audio_16k_mono);
    if query.is_empty() { return None; }
    find_best_by_fingerprint(&query, prefer_speaker_id)
}

/// Lower-level: query by precomputed fingerprint. Used when
/// the caller already has the MFCC mean (avoids recomputing).
pub fn find_best_by_fingerprint(
    query: &[f32],
    prefer_speaker_id: Option<&str>,
) -> Option<Hit> {
    let g = index().lock().ok()?;
    if g.entries.is_empty() { return None; }

    // Two-pass: speaker-preferred, then global.
    let mut best: Option<(f32, &Exemplar)> = None;
    if let Some(spk_id) = prefer_speaker_id {
        for ex in g.entries.iter().filter(|e| e.speaker_id == spk_id) {
            let d = cosine_distance(query, &ex.mfcc_mean);
            if best.as_ref().map_or(true, |(bd, _)| d < *bd) {
                best = Some((d, ex));
            }
        }
        // If speaker-specific match is tight, return it without
        // searching the global pool. Threshold is conservative
        // — 0.15 is "very close" for same-speaker MFCC means.
        if let Some((d, ex)) = best.as_ref() {
            if *d < 0.15 {
                return Some(Hit {
                    transcript: ex.transcript.clone(),
                    distance: *d,
                    speaker_id: ex.speaker_id.clone(),
                    unix_ms: ex.unix_ms,
                    count: ex.count,
                });
            }
        }
    }

    // Global pass. Walks every entry; for 20000 entries × 13
    // floats × cosine ≈ 1ms. Acceptable for the listen
    // pipeline where Whisper-tier latency is 500ms+.
    for ex in g.entries.iter() {
        if Some(ex.speaker_id.as_str()) == prefer_speaker_id { continue; }
        let d = cosine_distance(query, &ex.mfcc_mean);
        if best.as_ref().map_or(true, |(bd, _)| d < *bd) {
            best = Some((d, ex));
        }
    }
    best.map(|(d, ex)| Hit {
        transcript: ex.transcript.clone(),
        distance: d,
        speaker_id: ex.speaker_id.clone(),
        unix_ms: ex.unix_ms,
        count: ex.count,
    })
}

/// Add or update an exemplar. If an exemplar with the same
/// (speaker_id, unix_ms) exists, bump its count and refresh
/// last_used_ms. Otherwise insert.
///
/// Caller is responsible for fingerprinting; pass the
/// pre-computed mean for efficiency.
///
/// v0.1.289: debounced save_to_disk after every 5 mutations
/// so feedback survives restart. Disk write is small
/// (~80 bytes × N entries); 5-mutation batching means a
/// pathological burst of utterances pays at most 1 write
/// per 5 utterances.
pub fn add_exemplar(ex: Exemplar) {
    let mut g = match index().lock() { Ok(g) => g, Err(_) => return };
    // Idempotent merge by (speaker, ms).
    if let Some(existing) = g.entries.iter_mut().find(|e|
        e.speaker_id == ex.speaker_id && e.unix_ms == ex.unix_ms)
    {
        existing.count = existing.count.saturating_add(1);
        existing.last_used_ms = ex.last_used_ms;
        drop(g);
        maybe_save_debounced();
        return;
    }
    g.entries.push(ex);
    // Bound. Reaching the cap is rare in normal use; when
    // it happens, drop the oldest-by-last_used.
    if g.entries.len() > MAX_ENTRIES_TOTAL {
        g.entries.sort_by_key(|e| std::cmp::Reverse(e.last_used_ms));
        g.entries.truncate(MAX_ENTRIES_TOTAL);
    }
    drop(g);
    maybe_save_debounced();
}

/// Counter for debounced save. Increments on every
/// add_exemplar / record_hit. When it hits the threshold,
/// save_to_disk runs and the counter resets.
fn maybe_save_debounced() {
    static MUT_COUNT: std::sync::atomic::AtomicU32 =
        std::sync::atomic::AtomicU32::new(0);
    const SAVE_EVERY: u32 = 5;
    let n = MUT_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n >= SAVE_EVERY {
        MUT_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
        save_to_disk();
    }
}

/// Record that a query found a hit on this exemplar. Bumps
/// its count and refreshes last_used. Caller passes the
/// (speaker_id, unix_ms) tuple from the Hit struct.
pub fn record_hit(speaker_id: &str, unix_ms: u64) {
    let mut g = match index().lock() { Ok(g) => g, Err(_) => return };
    if let Some(ex) = g.entries.iter_mut().find(|e|
        e.speaker_id == speaker_id && e.unix_ms == unix_ms)
    {
        ex.count = ex.count.saturating_add(1);
        ex.last_used_ms = now_ms();
        drop(g);
        maybe_save_debounced();
    }
}

fn now_ms() -> u64 { core_kit::time::now_millis() }

fn db_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/web2qml/acoustic_index.bin"))
}

/// Save the in-memory index to disk. Idempotent — calling
/// while a build is running is safe (the in-memory state at
/// call time is what gets written).
pub fn save_to_disk() {
    let path = match db_path() { Some(p) => p, None => return };
    let g = match index().lock() { Ok(g) => g, Err(_) => return };

    // Apply per-speaker LRU caps BEFORE serializing so the
    // on-disk file stays bounded too.
    let mut per_speaker: std::collections::HashMap<String, Vec<&Exemplar>> =
        std::collections::HashMap::new();
    for ex in &g.entries {
        per_speaker.entry(ex.speaker_id.clone()).or_default().push(ex);
    }
    let mut keep: Vec<&Exemplar> = Vec::new();
    for (_, mut exs) in per_speaker {
        exs.sort_by_key(|e| std::cmp::Reverse(e.last_used_ms.max(e.unix_ms)));
        exs.truncate(MAX_ENTRIES_PER_SPEAKER);
        keep.extend(exs);
    }
    let n = keep.len().min(u32::MAX as usize) as u32;

    let mut out: Vec<u8> = Vec::with_capacity(64 + n as usize * 80);
    out.extend_from_slice(FILE_MAGIC);
    out.push(FILE_VERSION);
    out.extend_from_slice(&n.to_le_bytes());

    for ex in keep.iter().take(n as usize) {
        out.extend_from_slice(&ex.unix_ms.to_le_bytes());

        let spk_bytes = ex.speaker_id.as_bytes();
        let spk_n = spk_bytes.len().min(MAX_SPEAKER_ID_LEN) as u8;
        out.push(spk_n);
        out.extend_from_slice(&spk_bytes[..spk_n as usize]);

        let txt_bytes = ex.transcript.as_bytes();
        let txt_n = txt_bytes.len().min(MAX_TRANSCRIPT_LEN) as u16;
        out.extend_from_slice(&txt_n.to_le_bytes());
        out.extend_from_slice(&txt_bytes[..txt_n as usize]);

        out.extend_from_slice(&ex.count.to_le_bytes());
        out.extend_from_slice(&ex.last_used_ms.to_le_bytes());

        // mfcc_mean: pad/truncate to EMBED_DIM floats so the on-disk
        // format is fixed-width per entry. Defensive against bugs that
        // produce wrong-length vectors. (v0.1.647: EMBED_DIM=39 — was
        // N_MFCC=13; writing 13 here would truncate the richer embedding.)
        for i in 0..EMBED_DIM {
            let v = *ex.mfcc_mean.get(i).unwrap_or(&0.0);
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, &out) {
        crate::clog!("acoustic_index", "save failed: {}", e);
    }
}

/// Load the index from disk into memory. Returns silently
/// (and leaves the in-memory index empty) on missing file,
/// magic mismatch, or version mismatch. The runtime path
/// falls back to running Whisper as before.
pub fn load_from_disk() {
    let path = match db_path() { Some(p) => p, None => return };
    if !path.exists() { return; }
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => { crate::clog!("acoustic_index", "read failed: {}", e); return; }
    };
    if bytes.len() < 9 || &bytes[0..4] != FILE_MAGIC {
        crate::clog!("acoustic_index", "bad magic — ignoring");
        return;
    }
    let version = bytes[4];
    if version != FILE_VERSION {
        crate::clog!("acoustic_index", "version mismatch (got {}, expected {}) — ignoring",
            version, FILE_VERSION);
        return;
    }
    let mut p = 5;

    fn read_u8(b: &[u8], p: &mut usize) -> Option<u8> {
        let v = *b.get(*p)?; *p += 1; Some(v)
    }
    fn read_u16(b: &[u8], p: &mut usize) -> Option<u16> {
        if *p + 2 > b.len() { return None; }
        let v = u16::from_le_bytes([b[*p], b[*p+1]]);
        *p += 2; Some(v)
    }
    fn read_u32(b: &[u8], p: &mut usize) -> Option<u32> {
        if *p + 4 > b.len() { return None; }
        let v = u32::from_le_bytes([b[*p], b[*p+1], b[*p+2], b[*p+3]]);
        *p += 4; Some(v)
    }
    fn read_u64(b: &[u8], p: &mut usize) -> Option<u64> {
        if *p + 8 > b.len() { return None; }
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&b[*p..*p+8]);
        *p += 8; Some(u64::from_le_bytes(arr))
    }
    fn read_f32(b: &[u8], p: &mut usize) -> Option<f32> {
        if *p + 4 > b.len() { return None; }
        let v = f32::from_le_bytes([b[*p], b[*p+1], b[*p+2], b[*p+3]]);
        *p += 4; Some(v)
    }

    let n = match read_u32(&bytes, &mut p) { Some(v) => v, None => return };
    let mut loaded: Vec<Exemplar> = Vec::with_capacity(n as usize);

    for _ in 0..n {
        let unix_ms = match read_u64(&bytes, &mut p) { Some(v) => v, None => break };
        let spk_n = match read_u8(&bytes, &mut p) { Some(v) => v as usize, None => break };
        if p + spk_n > bytes.len() { break; }
        let speaker_id = String::from_utf8_lossy(&bytes[p..p+spk_n]).to_string();
        p += spk_n;
        let txt_n = match read_u16(&bytes, &mut p) { Some(v) => v as usize, None => break };
        if p + txt_n > bytes.len() { break; }
        let transcript = String::from_utf8_lossy(&bytes[p..p+txt_n]).to_string();
        p += txt_n;
        let count = match read_u32(&bytes, &mut p) { Some(v) => v, None => break };
        let last_used_ms = match read_u64(&bytes, &mut p) { Some(v) => v, None => break };
        let mut mfcc_mean = Vec::with_capacity(EMBED_DIM);
        let mut ok = true;
        for _ in 0..EMBED_DIM {
            match read_f32(&bytes, &mut p) {
                Some(v) => mfcc_mean.push(v),
                None => { ok = false; break; }
            }
        }
        if !ok { break; }
        loaded.push(Exemplar {
            unix_ms, speaker_id, transcript,
            mfcc_mean, count, last_used_ms,
        });
    }

    if let Ok(mut g) = index().lock() {
        g.entries = loaded;
        crate::clog!("acoustic_index", "loaded {} exemplars", g.entries.len());
    }
    built().store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Rebuild the index by scanning the corpus directory. Called
/// in a background thread at app startup AFTER load_from_disk
/// so the matcher works immediately from cached entries while
/// the rebuild fills in anything new (or rebuilds from
/// scratch on first run).
///
/// Cost: O(N_corpus) MFCC computations. ~50ms per recording
/// on a modern CPU. A corpus of 200 recordings rebuilds in
/// ~10s. Runs in a worker thread so the UI isn't blocked.
///
/// Idempotent — exemplars already in memory by (speaker, ms)
/// are skipped via add_exemplar's merge semantics.
pub fn rebuild_from_corpus_background() {
    std::thread::Builder::new()
        .name("acoustic-index-build".to_string())
        .spawn(|| {
            // v0.1.516: this is an O(N_corpus) MFCC rebuild (~10s for
            // 200 recordings) whose speed never improves the user
            // experience. If the app is backgrounded (screen off),
            // bias it onto efficiency cores for system power; if the
            // user is present, run unthrottled. Guard lifts on drop.
            let _eco = crate::cpu_efficiency::EcoGuard::new(
                crate::voice::is_app_backgrounded());
            let t_start = std::time::Instant::now();
            let added = rebuild_from_corpus_sync();
            built().store(true, std::sync::atomic::Ordering::Relaxed);
            // Persist whatever was added so future restarts
            // are fast — load_from_disk picks up the file.
            if added > 0 {
                save_to_disk();
            }
            crate::clog!("acoustic_index", "rebuild done: +{} exemplars in {:.2}s",
                added, t_start.elapsed().as_secs_f32());
        })
        .ok();
}

/// Synchronous rebuild — exposed for tests / future explicit
/// rebuild commands. Returns the number of new exemplars
/// added. Existing entries (same speaker+ms) are skipped.
pub fn rebuild_from_corpus_sync() -> usize {
    let home = match std::env::var("HOME") { Ok(h) => h, Err(_) => return 0 };
    let corpus_dir = PathBuf::from(&home).join(".config/web2qml/corpus");
    if !corpus_dir.exists() { return 0; }

    // Snapshot existing (speaker, ms) pairs so we don't
    // re-MFCC entries we already have.
    let existing: std::collections::HashSet<(String, u64)> = match index().lock() {
        Ok(g) => g.entries.iter().map(|e| (e.speaker_id.clone(), e.unix_ms)).collect(),
        Err(_) => return 0,
    };

    let mut added = 0;
    let speaker_dirs = match std::fs::read_dir(&corpus_dir) {
        Ok(rd) => rd, Err(_) => return 0,
    };
    for spk_entry in speaker_dirs.flatten() {
        let spk_path = spk_entry.path();
        if !spk_path.is_dir() { continue; }
        let speaker_id = match spk_path.file_name().and_then(|n| n.to_str()) {
            Some(s) => s.to_string(), None => continue,
        };
        let files = match std::fs::read_dir(&spk_path) {
            Ok(rd) => rd, Err(_) => continue,
        };
        // Collect (stem, audio_path, txt_path) per recording.
        let mut by_stem: std::collections::HashMap<String, (Option<PathBuf>, Option<PathBuf>)> =
            std::collections::HashMap::new();
        for f in files.flatten() {
            let p = f.path();
            let stem = match p.file_stem().and_then(|n| n.to_str()) {
                Some(s) => s.to_string(), None => continue,
            };
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            let entry = by_stem.entry(stem).or_insert((None, None));
            match ext {
                "wav" | "opus" => entry.0 = Some(p),
                "txt" => entry.1 = Some(p),
                _ => {}
            }
        }

        for (stem, (audio_opt, txt_opt)) in by_stem {
            let unix_ms: u64 = match stem.parse() { Ok(v) => v, Err(_) => continue };
            if existing.contains(&(speaker_id.clone(), unix_ms)) { continue; }
            let audio_path = match audio_opt { Some(p) => p, None => continue };
            let txt_path = match txt_opt { Some(p) => p, None => continue };

            // Load transcript text first — cheap. Skip if
            // empty so we don't index junk.
            let transcript = match std::fs::read_to_string(&txt_path) {
                Ok(s) => s.trim().to_string(),
                Err(_) => continue,
            };
            if transcript.is_empty() { continue; }
            if transcript.len() > MAX_TRANSCRIPT_LEN { continue; }

            // Load audio. WAV-only for v1; opus support filed.
            // The corpus_recorder writes opus when configured;
            // we skip opus here and pick it up when an opus
            // decoder is wired in (v0.1.231 has opus on the
            // recorder side but the read side doesn't exist
            // yet for our use case).
            if audio_path.extension().and_then(|e| e.to_str()) != Some("wav") {
                continue;
            }
            let audio = match load_wav_16k_mono(&audio_path) {
                Some(a) => a, None => continue,
            };
            let mfcc_mean = fingerprint(&audio);
            if mfcc_mean.is_empty() { continue; }

            add_exemplar(Exemplar {
                unix_ms,
                speaker_id: speaker_id.clone(),
                transcript,
                mfcc_mean,
                count: 1,
                last_used_ms: unix_ms,
            });
            added += 1;
        }
    }
    added
}

/// Minimal WAV reader: 16-bit PCM, mono, 16kHz. Other formats
/// return None; the rebuild path skips them. Caller can ALSO
/// use this for opus once we decode upstream.
fn load_wav_16k_mono(path: &PathBuf) -> Option<Vec<f32>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 44 { return None; }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" { return None; }
    // Find "data" chunk. Robust against fmt-chunk size variants
    // and optional LIST/INFO chunks.
    let mut p = 12;
    let mut channels: u16 = 1;
    let mut sample_rate: u32 = 16000;
    let mut bits_per_sample: u16 = 16;
    while p + 8 <= bytes.len() {
        let id = &bytes[p..p+4];
        let size = u32::from_le_bytes([bytes[p+4], bytes[p+5], bytes[p+6], bytes[p+7]]) as usize;
        p += 8;
        if id == b"fmt " {
            if size >= 16 && p + 16 <= bytes.len() {
                channels = u16::from_le_bytes([bytes[p+2], bytes[p+3]]);
                sample_rate = u32::from_le_bytes([bytes[p+4], bytes[p+5], bytes[p+6], bytes[p+7]]);
                bits_per_sample = u16::from_le_bytes([bytes[p+14], bytes[p+15]]);
            }
            p += size;
        } else if id == b"data" {
            // Read sample data. 16-bit signed → f32 in [-1, 1].
            if channels != 1 || sample_rate != 16000 || bits_per_sample != 16 {
                return None;
            }
            let end = (p + size).min(bytes.len());
            let mut out = Vec::with_capacity((end - p) / 2);
            let mut i = p;
            while i + 1 < end {
                let s = i16::from_le_bytes([bytes[i], bytes[i+1]]);
                out.push(s as f32 / 32768.0);
                i += 2;
            }
            return Some(out);
        } else {
            p += size;
        }
    }
    None
}

/// v0.1.284 (Tier1-1): kick off the initial index build at app
/// startup. Called from main. Loads any persisted index from
/// disk synchronously (fast), then spawns a background
/// rebuild pass for any corpus entries added since the file
/// was written. The matcher can be used immediately from the
/// loaded entries; the rebuild fills in the gaps.
pub fn init_at_startup() {
    load_from_disk();
    rebuild_from_corpus_background();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_distance_identical() {
        let a = vec![1.0, 0.5, -0.3];
        let d = cosine_distance(&a, &a);
        assert!(d.abs() < 1e-6, "identical vectors should have distance ~0");
    }

    #[test]
    fn cosine_distance_opposite() {
        let a = vec![1.0, 0.5, -0.3];
        let b: Vec<f32> = a.iter().map(|x| -x).collect();
        let d = cosine_distance(&a, &b);
        assert!((d - 2.0).abs() < 1e-6, "opposite vectors should have distance ~2");
    }

    #[test]
    fn cosine_distance_mismatched_length() {
        let a = vec![1.0, 0.5];
        let b = vec![1.0, 0.5, 0.0];
        assert_eq!(cosine_distance(&a, &b), f32::INFINITY);
    }

    #[test]
    fn cosine_distance_nan_or_zero_vector_is_infinite() {
        // v0.1.559: a NaN in a fingerprint must NOT yield a NaN distance
        // (which would poison nearest-neighbor selection). A bare
        // `na <= 0.0` guard let NaN norms through since NaN <= 0.0 is
        // false. All pathological vectors → INFINITY (no match).
        let good = vec![1.0, 0.5, -0.3];
        let zero = vec![0.0, 0.0, 0.0];
        let nan = vec![f32::NAN, 0.5, -0.3];
        let inf = vec![f32::INFINITY, 0.5, -0.3];
        for bad in [&zero, &nan, &inf] {
            let d = cosine_distance(&good, bad);
            assert_eq!(d, f32::INFINITY, "pathological vector must be no-match");
            let d2 = cosine_distance(bad, &good);
            assert_eq!(d2, f32::INFINITY, "order-independent");
        }
        // and a finite result is always finite + in [0, 2]
        let d = cosine_distance(&good, &good);
        assert!(d.is_finite() && (-1e-6..=2.0 + 1e-6).contains(&d));
    }

    #[test]
    fn fingerprint_empty_audio() {
        let fp = fingerprint(&[]);
        assert!(fp.is_empty());
    }

    #[test]
    fn fingerprint_returns_embed_dim_on_real_audio() {
        // 1 second of 440Hz tone at 16kHz.
        let mut audio = Vec::with_capacity(16000);
        for i in 0..16000 {
            let t = i as f32 / 16000.0;
            audio.push((2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5);
        }
        let fp = fingerprint(&audio);
        // v0.1.772: since v0.1.647 the fingerprint is the 39-dim embedding
        // (mean + std + delta-mean over N_MFCC=13 coefficients), i.e. EMBED_DIM
        // — not a single MFCC frame. The old assertion expected N_MFCC (13)
        // from when fingerprint returned only the per-utterance mean.
        assert_eq!(fp.len(), EMBED_DIM);
        assert_eq!(fp.len(), crate::phonetic::N_MFCC * 3);
    }

    #[test]
    fn add_and_query_roundtrip() {
        // Use unique speaker id so this test doesn't collide
        // with anything else running in parallel.
        let spk = format!("test_spk_{}", now_ms());
        let mfcc = vec![0.5; crate::phonetic::N_MFCC];
        add_exemplar(Exemplar {
            unix_ms: 12345,
            speaker_id: spk.clone(),
            transcript: "scroll down".to_string(),
            mfcc_mean: mfcc.clone(),
            count: 1,
            last_used_ms: 12345,
        });
        // Querying with the same fingerprint should hit it.
        let hit = find_best_by_fingerprint(&mfcc, Some(&spk));
        assert!(hit.is_some());
        let hit = hit.unwrap();
        assert_eq!(hit.transcript, "scroll down");
        assert!(hit.distance < 0.01);
    }
}
