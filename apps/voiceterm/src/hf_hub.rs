// src/hf_hub.rs
//
// v0.1.462 (llama.cpp backend arc): HuggingFace Hub model
// discovery for the GGUF model picker. PURE LOGIC + thin HTTP.
//
// This module is deliberately split so the parsing and URL
// construction can be unit-tested standalone (no Qt, no
// llama.cpp, no network) — the same approach that caught the
// sequence_diagram Note bug. The only function that touches
// the network is `fetch_json`, and it's a thin ureq wrapper;
// everything else is testable in isolation.
//
// What the Hub gives us (no auth needed for public models):
//   - Search:  GET /api/models?search=<q>&filter=gguf&sort=downloads&limit=N
//   - Repo files: GET /api/models/<repo_id>?expand=siblings
//   - Download:  GET /<repo_id>/resolve/main/<filename>
//
// JSON is hand-rolled (matching the project's no-serde stance,
// see archive.rs / llm_provider.rs). The Hub's JSON is
// well-structured and we only pluck a few fields, so a small
// targeted extractor is enough and keeps the dep surface flat.

#![allow(dead_code)]

const HF_API_BASE: &str = "https://huggingface.co/api";
const HF_RESOLVE_BASE: &str = "https://huggingface.co";

/// A model repo as returned by the search endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct HfModel {
    /// e.g. "bartowski/Qwen2.5-7B-Instruct-GGUF"
    pub repo_id: String,
    /// Download count (popularity sort signal). 0 if absent.
    pub downloads: u64,
    /// Like count. 0 if absent.
    pub likes: u64,
}

/// A single file inside a repo (from the siblings list).
#[derive(Debug, Clone, PartialEq)]
pub struct GgufFile {
    /// Filename within the repo, e.g.
    /// "Qwen2.5-7B-Instruct-Q4_K_M.gguf"
    pub filename: String,
    /// Detected quantization tier, parsed from the filename.
    /// None if the filename has no recognizable quant tag.
    pub quant: Option<QuantTier>,
}

/// Quantization tiers, ordered loosely by size/quality. Used
/// to recommend a default download for a given RAM budget and
/// to sort the file list sensibly in the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QuantTier {
    /// ~2-bit. Smallest, lowest quality.
    Q2,
    /// 3-bit family (Q3_K_S/M/L).
    Q3,
    /// 4-bit family (Q4_0, Q4_K_S/M). The usual sweet spot.
    Q4,
    /// 5-bit family.
    Q5,
    /// 6-bit.
    Q6,
    /// 8-bit. Near-lossless, large.
    Q8,
    /// f16 / full precision. Largest.
    F16,
}

impl QuantTier {
    /// Human label for the picker UI.
    pub fn label(&self) -> &'static str {
        match self {
            QuantTier::Q2 => "Q2 (smallest, lowest quality)",
            QuantTier::Q3 => "Q3 (small)",
            QuantTier::Q4 => "Q4 (recommended balance)",
            QuantTier::Q5 => "Q5 (better quality)",
            QuantTier::Q6 => "Q6 (high quality)",
            QuantTier::Q8 => "Q8 (near-lossless, large)",
            QuantTier::F16 => "F16 (full precision, largest)",
        }
    }

    /// Rough bytes-per-parameter for this tier. Used to
    /// estimate whether a model fits a RAM budget. These are
    /// approximate — real GGUF sizes vary with the K-quant
    /// mixing — but close enough to steer a default pick.
    pub fn bytes_per_param(&self) -> f32 {
        match self {
            QuantTier::Q2 => 0.33,
            QuantTier::Q3 => 0.43,
            QuantTier::Q4 => 0.56,
            QuantTier::Q5 => 0.68,
            QuantTier::Q6 => 0.82,
            QuantTier::Q8 => 1.06,
            QuantTier::F16 => 2.0,
        }
    }
}

/// Detect the quant tier from a GGUF filename. Case-insensitive.
/// Checks longer/more-specific patterns implicitly via the
/// substring set. Returns None if nothing matches.
///
/// Examples:
///   "model-Q4_K_M.gguf"  -> Q4
///   "model.Q8_0.gguf"    -> Q8
///   "model-f16.gguf"     -> F16
pub fn detect_quant(filename: &str) -> Option<QuantTier> {
    let lc = filename.to_lowercase();
    // Order matters only for disjoint checks; these substrings
    // don't overlap across tiers (q4 vs q40 etc. don't collide
    // because the tier digit directly follows 'q').
    if lc.contains("q2_") || lc.contains("q2k") || lc.contains("-q2") || lc.contains(".q2") {
        Some(QuantTier::Q2)
    } else if lc.contains("q3_") || lc.contains("q3k") || lc.contains("-q3") || lc.contains(".q3") {
        Some(QuantTier::Q3)
    } else if lc.contains("q4_") || lc.contains("q4k") || lc.contains("-q4") || lc.contains(".q4") {
        Some(QuantTier::Q4)
    } else if lc.contains("q5_") || lc.contains("q5k") || lc.contains("-q5") || lc.contains(".q5") {
        Some(QuantTier::Q5)
    } else if lc.contains("q6_") || lc.contains("q6k") || lc.contains("-q6") || lc.contains(".q6") {
        Some(QuantTier::Q6)
    } else if lc.contains("q8_") || lc.contains("q8k") || lc.contains("-q8") || lc.contains(".q8") {
        Some(QuantTier::Q8)
    } else if lc.contains("f16") || lc.contains("fp16") || lc.contains("-16b") {
        Some(QuantTier::F16)
    } else {
        None
    }
}

/// v0.1.473: a hand-curated "quick pick" model for the picker, so
/// the user doesn't have to know what to search for to get a fast,
/// known-good local model. Each entry is verified to:
///   - have a template-embedded GGUF (works with the bundled
///     llama.cpp in llama-cpp-2 0.1.146 — avoids the Gemma-4 /
///     Qwen3.5 chat-template gap that produces garbage tokens),
///   - come from a reliable quantizer (bartowski / official),
///   - exist at the stated repo + filename.
/// Ordered fastest → highest-quality so the picker can present a
/// speed tier. For web2qml's workload (page/terminal summarization
/// + voice action-routing — short in, short out), the smaller
/// models are the right default: benchmarks put a 1-3B at 2-3x the
/// tokens/s of a 7B on the same hardware, and the quality delta is
/// negligible for routing/summary. llama.cpp itself is also ~3-10%
/// faster than Ollama on the same box, so just moving off the
/// Ollama fallback is a win before model choice even matters.
#[derive(Debug, Clone, PartialEq)]
pub struct CuratedModel {
    /// Short display label for the quick-pick button.
    pub label: &'static str,
    /// One-line "why pick this" for the picker subtitle.
    pub blurb: &'static str,
    /// HF repo, e.g. "bartowski/Llama-3.2-3B-Instruct-GGUF".
    pub repo_id: &'static str,
    /// The specific Q4_K_M file to download (the sweet-spot quant).
    pub filename: &'static str,
    /// Approx params in billions (for the RAM-fit hint in the UI).
    pub params_billions: f32,
    /// Approx Q4_K_M download size in GB (rounded), for the UI.
    pub approx_gb: f32,
}

/// The curated quick-pick list, fastest first. These are the
/// "just give me something fast that works" options surfaced at
/// the top of the picker, above the live HF search.
pub fn curated_models() -> &'static [CuratedModel] {
    CURATED
}

static CURATED: &[CuratedModel] = &[
    CuratedModel {
        label: "Llama 3.2 3B (fastest)",
        blurb: "Best speed for summaries & routing. ~2-3x faster than 7B.",
        repo_id: "bartowski/Llama-3.2-3B-Instruct-GGUF",
        filename: "Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        params_billions: 3.0,
        approx_gb: 2.0,
    },
    CuratedModel {
        label: "Qwen2.5 3B (fast)",
        blurb: "Fast, strong instruction-following at 3B. Great default.",
        repo_id: "bartowski/Qwen2.5-3B-Instruct-GGUF",
        filename: "Qwen2.5-3B-Instruct-Q4_K_M.gguf",
        params_billions: 3.0,
        approx_gb: 2.0,
    },
    CuratedModel {
        label: "Qwen2.5 7B (balanced)",
        blurb: "Best all-around quality; comfortable on 16GB+ machines.",
        repo_id: "bartowski/Qwen2.5-7B-Instruct-GGUF",
        filename: "Qwen2.5-7B-Instruct-Q4_K_M.gguf",
        params_billions: 7.0,
        approx_gb: 4.7,
    },
    CuratedModel {
        label: "Llama 3.1 8B (Meta)",
        blurb: "Meta's 8B; strong all-rounder if you prefer the Llama family.",
        repo_id: "bartowski/Llama-3.1-8B-Instruct-GGUF",
        filename: "Llama-3.1-8B-Instruct-Q4_K_M.gguf",
        params_billions: 8.0,
        approx_gb: 4.9,
    },
];

/// Curated quick-pick list as a JSON array for the QML picker.
/// Each object: {label, blurb, repo_id, filename, params, gb}.
/// Built here (not in QML) so the list + its escaping live in one
/// tested place.
pub fn curated_models_json() -> String {
    let mut out = String::from("[");
    for (i, m) in CURATED.iter().enumerate() {
        if i > 0 { out.push(','); }
        out.push_str(&format!(
            "{{\"label\":\"{}\",\"blurb\":\"{}\",\"repo_id\":\"{}\",\
             \"filename\":\"{}\",\"params\":{},\"gb\":{}}}",
            json_escape(m.label), json_escape(m.blurb),
            json_escape(m.repo_id), json_escape(m.filename),
            m.params_billions, m.approx_gb));
    }
    out.push(']');
    out
}

/// Minimal JSON string escaping for the curated list (quotes +
/// backslashes). The curated strings are ASCII and contain none of
/// these in practice, but escape defensively so the JSON is always
/// valid.
fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Build the search URL. `query` is the user's text; `limit`
/// caps results. We always filter to GGUF and sort by
/// downloads (popularity) so the most-used models surface
/// first — the picker shows a sensible default list.
pub fn build_search_url(query: &str, limit: u32) -> String {
    let q = url_encode(query.trim());
    format!(
        "{}/models?search={}&filter=gguf&sort=downloads&direction=-1&limit={}",
        HF_API_BASE, q, limit
    )
}

/// Build the repo-info URL that includes the file (siblings)
/// listing.
pub fn build_repo_files_url(repo_id: &str) -> String {
    format!("{}/models/{}?expand=siblings", HF_API_BASE, repo_id)
}

/// Build the direct download URL for a file in a repo's main
/// revision.
pub fn build_download_url(repo_id: &str, filename: &str) -> String {
    format!("{}/{}/resolve/main/{}", HF_RESOLVE_BASE, repo_id,
        url_encode_path(filename))
}

/// Minimal percent-encoding for query-string values. Encodes
/// everything that isn't an unreserved char. Keeps the dep
/// surface flat (no `url` crate). Sufficient for model search
/// terms.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{:02X}", other)),
        }
    }
    out
}

/// Like url_encode but preserves '/' since filenames may live
/// in subdirectories within a repo.
fn url_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{:02X}", other)),
        }
    }
    out
}

/// Parse the JSON array returned by the search endpoint into a
/// list of HfModel. Hand-rolled scan: the search response is a
/// top-level JSON array of objects, each with at least an "id"
/// (the repo_id), and optionally "downloads" and "likes".
///
/// We don't fully parse JSON — we locate object boundaries and
/// pluck the three fields we need. Robust to field reordering
/// and unknown extra fields, which is what the Hub actually
/// sends.
pub fn parse_search_results(json: &str) -> Vec<HfModel> {
    let mut models = Vec::new();
    // Walk top-level array objects by brace matching.
    for obj in iter_top_level_objects(json) {
        let repo_id = match json_string_field(obj, "id") {
            Some(s) => s,
            None => continue,  // no id → not a usable result
        };
        let downloads = json_number_field(obj, "downloads").unwrap_or(0);
        let likes = json_number_field(obj, "likes").unwrap_or(0);
        models.push(HfModel { repo_id, downloads, likes });
    }
    models
}

/// Parse the repo-info JSON (with expand=siblings) into the
/// list of GGUF files. The siblings array holds objects with
/// "rfilename" keys; we keep only those ending in ".gguf".
pub fn parse_repo_files(json: &str) -> Vec<GgufFile> {
    let mut files = Vec::new();
    // Find the "siblings" array region. Everything we want is
    // inside it.
    let siblings_region = match find_array_after_key(json, "siblings") {
        Some(r) => r,
        None => return files,
    };
    for obj in iter_top_level_objects(siblings_region) {
        if let Some(fname) = json_string_field(obj, "rfilename") {
            if fname.to_lowercase().ends_with(".gguf") {
                let quant = detect_quant(&fname);
                files.push(GgufFile { filename: fname, quant });
            }
        }
    }
    // Sort by quant tier ascending (smallest first), unknown
    // quant tiers sort last.
    files.sort_by(|a, b| match (a.quant, b.quant) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.filename.cmp(&b.filename),
    });
    files
}

/// Estimated RAM (GB) reserved by everything that ISN'T the LLM,
/// for the recommendation budget. Models the real resident set so
/// recommend_curated doesn't pretend the LLM is the only thing in
/// memory. Composed of:
///   - whisper model(s): ~0.2 GB for single base.en-class; ~0.45 GB
///     if dual-model (tiny.en + base.en) is active (TODO #5).
///   - runtime + context + Qt/WebEngine + OS headroom: ~2.0 GB.
///
/// `dual_whisper` reflects whether parallel whisper inference is on
/// — so when TODO #5 lands it just passes `true` here and the model
/// recommendation automatically accounts for the second resident
/// whisper model. This is the hook that keeps the recommendation
/// honest as features add resident models.
pub fn whisper_reserve_gb(dual_whisper: bool) -> f32 {
    let whisper = if dual_whisper { 0.45 } else { 0.20 };
    whisper + 2.0
}

/// v0.1.475 (setup flow): pick which curated model to RECOMMEND for
/// a given RAM budget. Returns an index into curated_models().
///
/// IMPORTANT — this is workload-aware, not just RAM-fit. web2qml's
/// LLM use is summarization + voice action-routing: short in, short
/// out, latency-sensitive. Benchmarks put a 1-3B model at ~2-3x the
/// tokens/s of a 7-8B for negligible quality loss on THIS kind of
/// task. So the default recommendation is the fastest model that
/// fits, NOT the largest — speed is the point (getting off the slow
/// Ollama fallback). Bigger curated models remain one tap away in
/// the UI for users who want more general quality; we just don't
/// default them into a latency-sensitive routing path.
///
/// Single-model convenience wrapper over recommend_curated_with_reserve
/// using the single-whisper reserve. Preserves the original
/// signature + behavior for existing callers.
pub fn recommend_curated(ram_gb: f32) -> usize {
    recommend_curated_with_reserve(ram_gb, whisper_reserve_gb(false))
}

/// v0.1.478: the reserve-aware core. `reserve_gb` is RAM already
/// spoken for by non-LLM resident things (see whisper_reserve_gb).
/// Recommends the fastest curated model whose estimated resident
/// size fits in the LLM budget = ram_gb - reserve_gb (never letting
/// the budget go negative). Speed-first: walk smallest→largest, take
/// the first that fits. Falls back to index 0 (smallest/fastest) if
/// nothing fits — better a model that runs than none.
///
/// Returns 0 for an empty list so the caller always has a valid idx.
pub fn recommend_curated_with_reserve(ram_gb: f32, reserve_gb: f32) -> usize {
    let list = curated_models();
    if list.is_empty() { return 0; }
    // Budget for the LLM = total minus what's already resident.
    // Clamp to >= 0 so a tiny machine doesn't get a negative budget
    // that the fit check would treat oddly.
    let llm_budget = (ram_gb - reserve_gb).max(0.0);
    for (i, m) in list.iter().enumerate() {
        // Q4_K_M resident estimate: ~0.6 GB per billion params.
        // (The fixed runtime/context overhead now lives in the
        // reserve, not here, so this is just the weights.)
        let est_gb = m.params_billions * 0.6;
        if est_gb <= llm_budget {
            return i;
        }
    }
    0
}

/// Recommend a specific GGUF file from a repo's file list for a
/// given RAM budget and model size. Picks the highest-quality quant
/// tier that's estimated to fit in ~70% of the budget (leaving
/// headroom for context + OS). Falls back to the smallest available
/// if nothing fits, or None if the list is empty.
///
/// `param_billions` can be estimated from the model name
/// (e.g. "7B" → 7.0) by the caller; if unknown, pass a
/// conservative high value so the recommendation errs small.
pub fn recommend_file<'a>(
    files: &'a [GgufFile],
    ram_gb: f32,
    param_billions: f32,
) -> Option<&'a GgufFile> {
    if files.is_empty() { return None; }
    let budget_bytes = (ram_gb * 0.70) * 1e9;
    // Walk from highest quality to lowest, pick first that fits.
    let mut best_fit: Option<&GgufFile> = None;
    for f in files {
        if let Some(q) = f.quant {
            let est = param_billions * 1e9 * q.bytes_per_param();
            if est <= budget_bytes {
                // Higher tier is better; since `files` is sorted
                // ascending, keep updating to grab the largest
                // that still fits.
                best_fit = Some(f);
            }
        }
    }
    // If nothing with a known quant fits, fall back to the
    // smallest file (first in the ascending-sorted list).
    best_fit.or_else(|| files.first())
}

/// Estimate parameter count (billions) from a repo id or
/// filename by looking for an "<n>B" token. Returns None if no
/// size token found. Handles "7B", "7b", "1.5B", "70B",
/// "8x7B" (returns the per-expert 7, conservative). Decimal
/// sizes like "1.5B" are parsed.
pub fn estimate_params_billions(name: &str) -> Option<f32> {
    let lc = name.to_lowercase();
    let bytes = lc.as_bytes();
    // Scan for a digit-run (with optional decimal) immediately
    // followed by 'b' at a token boundary.
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            let mut saw_dot = false;
            while i < bytes.len()
                && (bytes[i].is_ascii_digit()
                    || (bytes[i] == b'.' && !saw_dot))
            {
                if bytes[i] == b'.' { saw_dot = true; }
                i += 1;
            }
            // Must be followed by 'b'
            if i < bytes.len() && bytes[i] == b'b' {
                // And the char after 'b' must not be a letter
                // (so "btrfs" etc. don't match; "7b" or "7b-"
                // or "7b." do).
                let after_ok = i + 1 >= bytes.len()
                    || !bytes[i + 1].is_ascii_alphabetic();
                if after_ok {
                    if let Ok(v) = lc[start..i].parse::<f32>() {
                        // Sanity bound: ignore absurd values
                        // (years like 2024b won't appear, but
                        // guard anyway).
                        if v > 0.0 && v < 2000.0 {
                            return Some(v);
                        }
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

// ─── Hand-rolled JSON helpers ───
//
// These mirror the targeted-extraction style used in
// archive.rs and llm_provider.rs. Not a general JSON parser —
// just enough to pluck flat string/number fields and walk
// arrays of objects by brace matching.

/// Iterate top-level `{...}` objects within `s`, yielding each
/// object's inner text (including the braces). Brace-matched,
/// quote-aware (braces inside strings don't count). Used both
/// for the search array and the siblings array.
fn iter_top_level_objects(s: &str) -> Vec<&str> {
    let mut objs = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut depth = 0;
    let mut start = 0;
    let mut in_string = false;
    let mut escaped = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 { start = i; }
                depth += 1;
            }
            b'}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        // i is the closing brace; slice inclusive.
                        objs.push(&s[start..=i]);
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    objs
}

/// Given JSON text, find the array `[...]` that follows
/// `"key":` and return its inner region (including brackets).
/// Quote- and brace-aware. Returns None if the key or a
/// following array isn't found.
fn find_array_after_key<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{}\"", key);
    let key_pos = s.find(&needle)?;
    let after = &s[key_pos + needle.len()..];
    // Find the opening '[' after the colon.
    let open_rel = after.find('[')?;
    let arr = &after[open_rel..];
    // Bracket-match to find the close, quote-aware.
    let bytes = arr.as_bytes();
    let mut i = 0;
    let mut depth = 0;
    let mut in_string = false;
    let mut escaped = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if escaped { escaped = false; }
            else if c == b'\\' { escaped = true; }
            else if c == b'"' { in_string = false; }
            i += 1;
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&arr[..=i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Extract a string field `"key":"value"` from a flat JSON
/// object fragment. Returns the unescaped value, or None if
/// the key is absent or not a string.
fn json_string_field(obj: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    // Find the key, then the colon, then the opening quote.
    let mut search_from = 0;
    while let Some(rel) = obj[search_from..].find(&needle) {
        let key_pos = search_from + rel;
        let after = &obj[key_pos + needle.len()..];
        // Skip whitespace + colon.
        let trimmed = after.trim_start();
        if !trimmed.starts_with(':') {
            // Not a "key": — could be a substring match inside
            // another value. Advance and keep looking.
            search_from = key_pos + needle.len();
            continue;
        }
        let after_colon = trimmed[1..].trim_start();
        if !after_colon.starts_with('"') {
            // Value isn't a string (could be null/number).
            return None;
        }
        // Walk the string value, handling escapes.
        let val_bytes = after_colon.as_bytes();
        let mut i = 1; // past opening quote
        let start = i;
        while i < val_bytes.len() {
            if val_bytes[i] == b'\\' && i + 1 < val_bytes.len() {
                i += 2;
                continue;
            }
            if val_bytes[i] == b'"' {
                let raw = &after_colon[start..i];
                return Some(unescape_json(raw));
            }
            i += 1;
        }
        return None;
    }
    None
}

/// Extract a numeric field `"key":<number>` as u64. Truncates
/// decimals. Returns None if absent or non-numeric.
fn json_number_field(obj: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{}\"", key);
    let mut search_from = 0;
    while let Some(rel) = obj[search_from..].find(&needle) {
        let key_pos = search_from + rel;
        let after = &obj[key_pos + needle.len()..];
        let trimmed = after.trim_start();
        if !trimmed.starts_with(':') {
            search_from = key_pos + needle.len();
            continue;
        }
        let after_colon = trimmed[1..].trim_start();
        // Read a number (digits, optional decimal). Stop at
        // first non-numeric.
        let nbytes = after_colon.as_bytes();
        let mut i = 0;
        while i < nbytes.len()
            && (nbytes[i].is_ascii_digit() || nbytes[i] == b'.')
        {
            i += 1;
        }
        if i == 0 { return None; }
        // Parse the integer part only.
        let num_str = &after_colon[..i];
        let int_part = num_str.split('.').next().unwrap_or("0");
        return int_part.parse::<u64>().ok();
    }
    None
}

/// Minimal JSON string unescape — handles the common escapes.
/// Mirrors llm_provider::unescape_json_string but local so this
/// module stays standalone-testable.
fn unescape_json(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Ok(code) = u32::from_str_radix(&hex, 16) {
                    if let Some(ch) = char::from_u32(code) {
                        out.push(ch);
                    }
                }
            }
            Some(other) => { out.push('\\'); out.push(other); }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_quant_common_forms() {
        assert_eq!(detect_quant("model-Q4_K_M.gguf"), Some(QuantTier::Q4));
        assert_eq!(detect_quant("model.Q8_0.gguf"), Some(QuantTier::Q8));
        assert_eq!(detect_quant("Model-q5_k_s.gguf"), Some(QuantTier::Q5));
        assert_eq!(detect_quant("foo-Q2_K.gguf"), Some(QuantTier::Q2));
        assert_eq!(detect_quant("bar-Q6_K.gguf"), Some(QuantTier::Q6));
        assert_eq!(detect_quant("baz-Q3_K_L.gguf"), Some(QuantTier::Q3));
        assert_eq!(detect_quant("model-f16.gguf"), Some(QuantTier::F16));
        assert_eq!(detect_quant("model-fp16.gguf"), Some(QuantTier::F16));
    }

    #[test]
    fn detect_quant_no_match() {
        assert_eq!(detect_quant("model.gguf"), None);
        assert_eq!(detect_quant("readme.txt"), None);
    }

    #[test]
    fn build_search_url_encodes_query() {
        let url = build_search_url("qwen 2.5 coder", 20);
        assert!(url.contains("search=qwen%202.5%20coder"));
        assert!(url.contains("filter=gguf"));
        assert!(url.contains("sort=downloads"));
        assert!(url.contains("limit=20"));
    }

    #[test]
    fn build_download_url_shape() {
        let url = build_download_url(
            "bartowski/Qwen2.5-7B-Instruct-GGUF",
            "Qwen2.5-7B-Instruct-Q4_K_M.gguf");
        assert_eq!(url,
            "https://huggingface.co/bartowski/Qwen2.5-7B-Instruct-GGUF/resolve/main/Qwen2.5-7B-Instruct-Q4_K_M.gguf");
    }

    #[test]
    fn parse_search_results_basic() {
        let json = r#"[
            {"id":"bartowski/Qwen2.5-7B-Instruct-GGUF","downloads":150000,"likes":320},
            {"id":"TheBloke/Llama-2-7B-GGUF","downloads":89000,"likes":210}
        ]"#;
        let models = parse_search_results(json);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].repo_id, "bartowski/Qwen2.5-7B-Instruct-GGUF");
        assert_eq!(models[0].downloads, 150000);
        assert_eq!(models[0].likes, 320);
        assert_eq!(models[1].repo_id, "TheBloke/Llama-2-7B-GGUF");
    }

    #[test]
    fn parse_search_results_missing_optional_fields() {
        // downloads/likes absent → default to 0.
        let json = r#"[{"id":"foo/bar-GGUF"}]"#;
        let models = parse_search_results(json);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].repo_id, "foo/bar-GGUF");
        assert_eq!(models[0].downloads, 0);
        assert_eq!(models[0].likes, 0);
    }

    #[test]
    fn parse_search_results_ignores_objects_without_id() {
        let json = r#"[{"downloads":5},{"id":"a/b-GGUF","downloads":10}]"#;
        let models = parse_search_results(json);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].repo_id, "a/b-GGUF");
    }

    #[test]
    fn parse_repo_files_extracts_gguf_only() {
        let json = r#"{
            "id":"bartowski/Foo-GGUF",
            "siblings":[
                {"rfilename":"README.md"},
                {"rfilename":"Foo-Q4_K_M.gguf"},
                {"rfilename":"Foo-Q8_0.gguf"},
                {"rfilename":".gitattributes"}
            ]
        }"#;
        let files = parse_repo_files(json);
        assert_eq!(files.len(), 2);
        // Sorted ascending by quant: Q4 before Q8.
        assert_eq!(files[0].filename, "Foo-Q4_K_M.gguf");
        assert_eq!(files[0].quant, Some(QuantTier::Q4));
        assert_eq!(files[1].filename, "Foo-Q8_0.gguf");
        assert_eq!(files[1].quant, Some(QuantTier::Q8));
    }

    #[test]
    fn parse_repo_files_no_siblings() {
        let json = r#"{"id":"foo/bar"}"#;
        assert_eq!(parse_repo_files(json).len(), 0);
    }

    #[test]
    fn estimate_params_basic() {
        assert_eq!(estimate_params_billions("Qwen2.5-7B-Instruct"), Some(7.0));
        assert_eq!(estimate_params_billions("Llama-2-70b-chat"), Some(70.0));
        assert_eq!(estimate_params_billions("model-1.5B"), Some(1.5));
    }

    #[test]
    fn estimate_params_no_false_positive() {
        // "btrfs" must not match (b followed by letter).
        assert_eq!(estimate_params_billions("some-btrfs-model"), None);
        // No size token at all.
        assert_eq!(estimate_params_billions("tinyllama-chat"), None);
    }

    #[test]
    fn recommend_file_picks_largest_that_fits() {
        let files = vec![
            GgufFile { filename: "m-Q4_K_M.gguf".into(), quant: Some(QuantTier::Q4) },
            GgufFile { filename: "m-Q6_K.gguf".into(), quant: Some(QuantTier::Q6) },
            GgufFile { filename: "m-Q8_0.gguf".into(), quant: Some(QuantTier::Q8) },
        ];
        // 7B model, 16GB RAM → budget ~11.2GB. Q8 ≈ 7.4GB fits,
        // so it should pick Q8 (the largest that fits).
        let rec = recommend_file(&files, 16.0, 7.0).unwrap();
        assert_eq!(rec.quant, Some(QuantTier::Q8));
    }

    #[test]
    fn recommend_file_constrained_ram() {
        let files = vec![
            GgufFile { filename: "m-Q4_K_M.gguf".into(), quant: Some(QuantTier::Q4) },
            GgufFile { filename: "m-Q8_0.gguf".into(), quant: Some(QuantTier::Q8) },
        ];
        // 70B model, 8GB RAM → budget ~5.6GB. Nothing fits;
        // falls back to smallest (Q4, first in ascending sort).
        let rec = recommend_file(&files, 8.0, 70.0).unwrap();
        assert_eq!(rec.quant, Some(QuantTier::Q4));
    }

    #[test]
    fn recommend_file_empty() {
        assert!(recommend_file(&[], 16.0, 7.0).is_none());
    }

    #[test]
    fn iter_top_level_objects_handles_nested() {
        // Nested braces inside an object must not split it.
        let json = r#"[{"a":1,"nested":{"x":2}},{"b":3}]"#;
        let objs = iter_top_level_objects(json);
        assert_eq!(objs.len(), 2);
        assert!(objs[0].contains("nested"));
    }

    #[test]
    fn iter_top_level_objects_ignores_braces_in_strings() {
        // A brace inside a string value must not break matching.
        let json = r#"[{"name":"a {weird} name","id":"x"}]"#;
        let objs = iter_top_level_objects(json);
        assert_eq!(objs.len(), 1);
        assert_eq!(json_string_field(objs[0], "id"), Some("x".to_string()));
    }

    #[test]
    fn json_string_field_skips_substring_keys() {
        // "downloadsAllTime" contains "downloads" — make sure
        // we match the real key, not the prefix.
        let obj = r#"{"downloadsAllTime":"never","id":"real"}"#;
        // Looking for "id" should find "real".
        assert_eq!(json_string_field(obj, "id"), Some("real".to_string()));
    }

    // ── v0.1.473: curated quick-pick list ──

    #[test]
    fn curated_list_nonempty_and_fastest_first() {
        let list = curated_models();
        assert!(!list.is_empty());
        // Ordered fastest → largest by param count (non-decreasing).
        for w in list.windows(2) {
            assert!(w[0].params_billions <= w[1].params_billions,
                "curated list must be ordered smallest/fastest first: \
                 {} ({}B) before {} ({}B)",
                w[0].label, w[0].params_billions,
                w[1].label, w[1].params_billions);
        }
        // First entry should be a small/fast model (<=3B) — the
        // speed-first default for summarization/routing.
        assert!(list[0].params_billions <= 3.0,
            "fastest pick should be <=3B, got {}B", list[0].params_billions);
    }

    #[test]
    fn curated_entries_are_q4km_gguf() {
        // Every curated file is a Q4_K_M .gguf (the sweet-spot quant)
        // and detect_quant agrees it's Q4.
        for m in curated_models() {
            assert!(m.filename.ends_with(".gguf"),
                "{} not a .gguf", m.filename);
            assert!(m.filename.contains("Q4_K_M"),
                "{} not Q4_K_M", m.filename);
            assert_eq!(detect_quant(m.filename), Some(QuantTier::Q4),
                "detect_quant disagrees for {}", m.filename);
            // repo_id looks like "owner/Name-GGUF".
            assert!(m.repo_id.contains('/'), "{} missing owner", m.repo_id);
            assert!(m.repo_id.ends_with("-GGUF"),
                "{} not a -GGUF repo", m.repo_id);
        }
    }

    #[test]
    fn curated_json_is_wellformed() {
        let json = curated_models_json();
        assert!(json.starts_with('['));
        assert!(json.ends_with(']'));
        // One object per curated entry (count by "repo_id" keys).
        let n = json.matches("\"repo_id\"").count();
        assert_eq!(n, curated_models().len());
        // The fastest pick's repo must appear.
        assert!(json.contains(curated_models()[0].repo_id));
        // No stray unescaped issues: balanced braces.
        let opens = json.matches('{').count();
        let closes = json.matches('}').count();
        assert_eq!(opens, closes);
    }

    #[test]
    fn curated_download_urls_build() {
        // Each curated entry must produce a valid resolve URL.
        for m in curated_models() {
            let url = build_download_url(m.repo_id, m.filename);
            assert!(url.starts_with("https://huggingface.co/"));
            assert!(url.contains(m.filename));
        }
    }

    // ── v0.1.475: setup-flow RAM → recommendation ──

    #[test]
    fn recommend_curated_returns_valid_index() {
        // Always in-bounds, for any plausible RAM.
        let n = curated_models().len();
        for ram in [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0] {
            let idx = recommend_curated(ram);
            assert!(idx < n, "idx {} out of bounds for ram {}", idx, ram);
        }
    }

    #[test]
    fn recommend_curated_is_speed_first() {
        // Workload-aware: the default is the FASTEST model that fits,
        // not the largest. The fastest pick (index 0) fits any
        // realistic machine, so every plausible RAM recommends it.
        for ram in [4.0, 8.0, 16.0, 32.0, 64.0] {
            assert_eq!(recommend_curated(ram), 0,
                "ram {} should recommend the fast pick (idx 0)", ram);
        }
        // The recommended model is small/fast (<=3B).
        assert!(curated_models()[recommend_curated(64.0)].params_billions <= 3.0);
    }

    #[test]
    fn recommend_curated_never_panics_tiny_ram() {
        // Absurdly small RAM still returns a valid fallback index.
        assert_eq!(recommend_curated(1.0), 0);
        assert_eq!(recommend_curated(0.5), 0);
    }

    // ── v0.1.478: reserve-aware budget (dual-model groundwork) ──

    #[test]
    fn whisper_reserve_accounts_for_dual() {
        // Dual whisper reserves more than single (second model
        // resident). Both include the ~2GB runtime/OS headroom.
        let single = whisper_reserve_gb(false);
        let dual = whisper_reserve_gb(true);
        assert!(dual > single, "dual reserve must exceed single");
        assert!(single >= 2.0, "reserve must include runtime/OS headroom");
    }

    #[test]
    fn reserve_aware_shrinks_budget() {
        // A bigger reserve can only push toward a smaller/equal pick,
        // never a larger one (less RAM left for the LLM).
        let small_reserve = recommend_curated_with_reserve(16.0, 2.0);
        let big_reserve = recommend_curated_with_reserve(16.0, 12.0);
        let p_small = curated_models()[small_reserve].params_billions;
        let p_big = curated_models()[big_reserve].params_billions;
        assert!(p_big <= p_small,
            "larger reserve ({}B) must not recommend bigger than smaller reserve ({}B)",
            p_big, p_small);
    }

    #[test]
    fn reserve_aware_speed_first_still_holds() {
        // With the real single-whisper reserve, every realistic
        // machine still gets the fast pick (idx 0) — the contract
        // the setup flow depends on.
        let r = whisper_reserve_gb(false);
        for ram in [4.0, 8.0, 16.0, 32.0] {
            assert_eq!(recommend_curated_with_reserve(ram, r), 0,
                "ram {} (reserve {}) should still recommend the fast pick", ram, r);
        }
    }

    #[test]
    fn reserve_aware_never_negative_budget() {
        // reserve > ram must not panic or misbehave — clamps to 0
        // and returns the fallback fast pick.
        assert_eq!(recommend_curated_with_reserve(2.0, 8.0), 0);
        assert_eq!(recommend_curated_with_reserve(0.0, 2.2), 0);
    }

    #[test]
    fn recommend_curated_wrapper_matches_single_reserve() {
        // The convenience wrapper == the reserve-aware core with the
        // single-whisper reserve, for a range of RAM.
        for ram in [4.0, 8.0, 16.0, 32.0] {
            assert_eq!(
                recommend_curated(ram),
                recommend_curated_with_reserve(ram, whisper_reserve_gb(false)),
                "wrapper must match core for ram {}", ram);
        }
    }
}
