//! Content-addressed, central, evictable store for the derived int8 weight cache.
//!
//! The int8 cache is a DERIVED artifact — a re-quantization of the GGUF, not a small diff of it — so
//! it should not live as an ever-growing sidecar next to every model in every project (the storage
//! blowup). Instead: key each blob by the model's CONTENT, so the same GGUF copied into two projects
//! shares ONE cache entry (dedup); keep them in a single central dir; and evict oldest entries once
//! the dir exceeds a size cap. A changed model has different content → a different key → an automatic
//! miss, and the stale blob is reclaimed by eviction. `.tmp` orphans from a crashed atomic-write are
//! swept too. std only — no deps, matching the engine's dependency-minimal policy.

use std::path::PathBuf;

/// Cheap content key: file length mixed with sampled 64 KiB chunks (head / middle / tail). Hashing a
/// whole multi-GB GGUF would dominate load; sampling is stable across copies and collision-safe for
/// telling models apart. std SipHash (`DefaultHasher`) — no deps.
pub fn content_key(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let n = bytes.len();
    n.hash(&mut h);
    let chunk = 64 * 1024;
    for &s in &[0usize, n / 2, n.saturating_sub(chunk)] {
        let e = (s + chunk).min(n);
        if s < e { bytes[s..e].hash(&mut h); }
    }
    h.finish()
}

fn home() -> Option<PathBuf> { std::env::var_os("HOME").map(PathBuf::from) }

/// OS-appropriate base data dir. Linux/BSD: `$XDG_DATA_HOME` or `~/.local/share`.
/// macOS: `~/Library/Application Support`.
fn data_root() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support")).unwrap_or_else(|| PathBuf::from("."))
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// Central dir for derived int8 blobs. Override with `LOGSCOPE_CACHE_DIR`.
pub fn cache_root() -> PathBuf {
    if let Some(d) = std::env::var_os("LOGSCOPE_CACHE_DIR") { return PathBuf::from(d); }
    data_root().join("logscope").join("int8cache")
}

/// Unified models dir for the user's OWN GGUFs, shared across projects. Override with
/// `LOGSCOPE_MODELS_DIR`. (Ollama stores are discovered separately — see `ollama_roots`.)
pub fn models_root() -> PathBuf {
    if let Some(d) = std::env::var_os("LOGSCOPE_MODELS_DIR") { return PathBuf::from(d); }
    data_root().join("logscope").join("models")
}

/// Every existing ollama model store. Ollama on Linux commonly runs as a systemd service with models
/// under /usr/share/ollama or /var/lib/ollama — NOT ~/.ollama — so probe several (plus the
/// $OLLAMA_MODELS override). A path qualifies only if it actually has a `manifests/` subdir.
pub fn ollama_roots() -> Vec<PathBuf> {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("OLLAMA_MODELS") { cands.push(PathBuf::from(d)); }
    if let Some(h) = home() { cands.push(h.join(".ollama/models")); }
    for p in ["/usr/share/ollama/.ollama/models", "/var/lib/ollama/.ollama/models", "/usr/local/share/ollama/.ollama/models"] {
        cands.push(PathBuf::from(p));
    }
    let mut out = Vec::new();
    for c in cands { if c.join("manifests").is_dir() && !out.contains(&c) { out.push(c); } }
    out
}

/// All roots model discovery should scan: the logscope dir + every ollama store found.
pub fn all_model_roots() -> Vec<PathBuf> {
    let mut v = vec![models_root()];
    for o in ollama_roots() { if !v.contains(&o) { v.push(o); } }
    v
}

/// Cache blob path for a content key: `<cache_root>/<key>.int8`.
pub fn cache_path(key: u64) -> PathBuf { cache_root().join(format!("{key:016x}.int8")) }

pub fn ensure_cache_dir() -> std::io::Result<()> { std::fs::create_dir_all(cache_root()) }

/// Per-system autotune cache: maps (adapter, k×n shape) → the winning decode tile, so the one-time
/// benchmark on first model load is skipped on every load thereafter. A tiny TSV in the cache dir
/// (adapter \t k \t n \t tile per line). Best-effort: any IO/parse failure just means "re-benchmark".
fn autotune_file() -> PathBuf { cache_root().join("autotune-v3.tsv") } // v2: value is a packed (kernel,tile)
pub fn autotune_get(adapter: &str, k: usize, n: usize) -> Option<u32> {
    let txt = std::fs::read_to_string(autotune_file()).ok()?;
    for line in txt.lines() {
        let mut f = line.split('\t');
        if f.next() == Some(adapter)
            && f.next().and_then(|v| v.parse::<usize>().ok()) == Some(k)
            && f.next().and_then(|v| v.parse::<usize>().ok()) == Some(n) {
            return f.next().and_then(|v| v.parse::<u32>().ok());
        }
    }
    None
}
pub fn autotune_put(adapter: &str, k: usize, n: usize, tile: u32) {
    let _ = ensure_cache_dir();
    // drop any stale row for this (adapter,k,n), then append the fresh one.
    let prev = std::fs::read_to_string(autotune_file()).unwrap_or_default();
    let mut out = String::new();
    for line in prev.lines() {
        let mut f = line.split('\t');
        let same = f.next() == Some(adapter)
            && f.next().and_then(|v| v.parse::<usize>().ok()) == Some(k)
            && f.next().and_then(|v| v.parse::<usize>().ok()) == Some(n);
        if !same && !line.trim().is_empty() { out.push_str(line); out.push('\n'); }
    }
    out.push_str(&format!("{adapter}\t{k}\t{n}\t{tile}\n"));
    let _ = std::fs::write(autotune_file(), out);
}

fn flush_file() -> PathBuf { cache_root().join("submit-flush.tsv") }
/// Cached passes-per-submit for this adapter (the SubmitTuner's settled value), if any.
pub fn flush_get(adapter: &str) -> Option<u32> {
    let txt = std::fs::read_to_string(flush_file()).ok()?;
    for line in txt.lines() {
        let mut f = line.split('\t');
        if f.next() == Some(adapter) { return f.next().and_then(|v| v.parse::<u32>().ok()); }
    }
    None
}
/// Persist the settled passes-per-submit for this adapter (replaces any prior row).
pub fn flush_put(adapter: &str, flush: u32) {
    let _ = ensure_cache_dir();
    let prev = std::fs::read_to_string(flush_file()).unwrap_or_default();
    let mut out = String::new();
    for line in prev.lines() {
        if line.split('\t').next() != Some(adapter) && !line.trim().is_empty() { out.push_str(line); out.push('\n'); }
    }
    out.push_str(&format!("{adapter}\t{flush}\n"));
    let _ = std::fs::write(flush_file(), out);
}

fn fast_ab_file() -> std::path::PathBuf { cache_root().join("fast_ab.tsv") }
/// The persisted dispatch-lever A/B verdict for this adapter: Some(true)=ON, Some(false)=OFF, None=never
/// measured (probe on next load). Keyed by adapter so each GPU keeps its own answer.
pub fn fast_ab_get(adapter: &str) -> Option<bool> {
    let txt = std::fs::read_to_string(fast_ab_file()).ok()?;
    for line in txt.lines() {
        let mut f = line.split('\t');
        if f.next() == Some(adapter) { return f.next().map(|v| v == "1"); }
    }
    None
}
/// Persist the A/B verdict for this adapter (replaces any prior row).
pub fn fast_ab_put(adapter: &str, on: bool) {
    let _ = ensure_cache_dir();
    let prev = std::fs::read_to_string(fast_ab_file()).unwrap_or_default();
    let mut out = String::new();
    for line in prev.lines() {
        if line.split('\t').next() != Some(adapter) && !line.trim().is_empty() { out.push_str(line); out.push('\n'); }
    }
    out.push_str(&format!("{adapter}\t{}\n", if on { 1 } else { 0 }));
    let _ = std::fs::write(fast_ab_file(), out);
}

/// Size cap for the cache dir, in bytes. `LOGSCOPE_CACHE_MAX_GB` overrides (default 10 GiB).
pub fn cache_cap_bytes() -> u64 {
    std::env::var("LOGSCOPE_CACHE_MAX_GB").ok().and_then(|s| s.trim().parse::<f64>().ok())
        .map(|gb| (gb * (1u64 << 30) as f64) as u64)
        .unwrap_or(10 * (1u64 << 30))
}

/// Sweep the cache dir: always reclaim orphaned `.tmp` half-writes, and if total `.int8` size
/// exceeds `cap`, delete oldest-by-mtime blobs (LRU) until under it. Returns (files_removed,
/// bytes_freed). Used right after writing a fresh blob so the dir self-bounds.
pub fn evict_to_cap(cap: u64) -> (usize, u64) {
    let dir = cache_root();
    let rd = match std::fs::read_dir(&dir) { Ok(r) => r, Err(_) => return (0, 0) };
    let mut blobs: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total = 0u64;
    let (mut removed, mut freed) = (0usize, 0u64);
    for e in rd.flatten() {
        let md = match e.metadata() { Ok(m) => m, Err(_) => continue };
        if !md.is_file() { continue; }
        let p = e.path();
        match p.extension().and_then(|s| s.to_str()) {
            Some("tmp") => { if std::fs::remove_file(&p).is_ok() { removed += 1; freed += md.len(); } }
            Some("int8") => {
                total += md.len();
                blobs.push((p, md.len(), md.modified().unwrap_or(std::time::UNIX_EPOCH)));
            }
            _ => {}
        }
    }
    if total > cap {
        blobs.sort_by_key(|(_, _, t)| *t); // oldest first
        for (p, len, _) in blobs {
            if total <= cap { break; }
            if std::fs::remove_file(&p).is_ok() { total -= len; freed += len; removed += 1; }
        }
    }
    (removed, freed)
}

/// Total bytes the cache dir occupies right now (every regular file: `.int8` blobs plus the small
/// autotune sidecars and any `.tmp` orphan). For a "cache is using X GiB" readout in settings.
pub fn cache_size_bytes() -> u64 {
    let rd = match std::fs::read_dir(cache_root()) { Ok(r) => r, Err(_) => return 0 };
    rd.flatten().filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file()).map(|m| m.len()).sum()
}

/// Evict so a blob of `incoming` bytes can be written WITHOUT the dir exceeding `cap` — i.e. make room
/// FIRST. The old order (write, then `evict_to_cap`) briefly held old+new, so peak ≈ 2× a big blob
/// (with a 10 GiB cap and ~10 GiB blobs that's ~19 GiB momentarily); evicting to `cap − incoming`
/// up front bounds the peak to ≈ cap. `.tmp` orphans are always swept. Returns (removed, freed).
pub fn make_room_for(cap: u64, incoming: u64) -> (usize, u64) {
    evict_to_cap(cap.saturating_sub(incoming))
}

/// Drop the heavy derived blobs: remove every `.int8` cache file and any `.tmp` orphan, KEEPING the
/// cheap autotune sidecars (those cost a benchmark to regenerate, not a dequant). Returns
/// (files_removed, bytes_freed). Backs a "clear cache" action — the next load re-dequantizes, so this
/// trades load time for disk and never corrupts state (a miss just rebuilds the blob).
pub fn purge() -> (usize, u64) {
    let rd = match std::fs::read_dir(cache_root()) { Ok(r) => r, Err(_) => return (0, 0) };
    let (mut removed, mut freed) = (0usize, 0u64);
    for e in rd.flatten() {
        let md = match e.metadata() { Ok(m) => m, Err(_) => continue };
        if !md.is_file() { continue; }
        let p = e.path();
        if matches!(p.extension().and_then(|s| s.to_str()), Some("int8") | Some("tmp")) {
            if std::fs::remove_file(&p).is_ok() { removed += 1; freed += md.len(); }
        }
    }
    (removed, freed)
}

// ── pre-flight memory check: refuse a too-big model instead of OOM-killing mid-dequant ─────────

/// int8-resident footprint (bytes) for llama dims: weights ≈1 byte/param + per-32 row scales, plus
/// the embedding which stays f32 (host gather), plus an untied output projection if present.
pub fn resident_bytes_for(d: u64, h: u64, n: u64, n_heads: u64, n_kv: u64, vocab: u64, untied_output: bool) -> u64 {
    let kv = n_kv * (d / n_heads.max(1));
    let per_layer = 2 * d * d + 2 * kv * d + 3 * h * d; // attn (q,o = d*d; k,v = kv*d) + ffn (gate_up 2h*d, down h*d)
    let int8 = per_layer * n;
    let scales = int8 / 32 * 4;
    let embed_f32 = vocab * d * 4;
    let output = if untied_output { vocab * d } else { 0 };
    int8 + scales + embed_f32 + output
}

/// Estimate a llama GGUF's int8-resident footprint from its metadata. None if the metadata is missing.
pub fn estimate_int8_resident(g: &crate::gguf::Gguf) -> Option<u64> {
    let d = g.get_u32("llama.embedding_length")? as u64;
    let h = g.get_u32("llama.feed_forward_length")? as u64;
    let n = g.get_u32("llama.block_count")? as u64;
    let n_heads = g.get_u32("llama.attention.head_count").unwrap_or(1) as u64;
    let n_kv = g.get_u32("llama.attention.head_count_kv").unwrap_or(n_heads as u32) as u64;
    let vocab = g.get_u32("llama.vocab_size").map(|v| v as u64)
        .or_else(|| g.tensor("token_embd.weight").map(|t| t.dims.iter().product::<u64>() / d))
        .unwrap_or(0);
    Some(resident_bytes_for(d, h, n, n_heads, n_kv, vocab, g.tensor("output.weight").is_some()))
}

/// Available system memory in bytes (Linux: /proc/meminfo MemAvailable). None where unknown.
pub fn available_memory_bytes() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            return rest.trim().trim_end_matches("kB").trim().parse::<u64>().ok().map(|kb| kb * 1024);
        }
    }
    None
}

/// Resident budget for a load: LOGSCOPE_MAX_RESIDENT_GB overrides; else 90% of available memory
/// (headroom for the KV cache + compute buffers + OS). None if memory can't be determined.
pub fn resident_budget_bytes() -> Option<u64> {
    if let Ok(s) = std::env::var("LOGSCOPE_MAX_RESIDENT_GB") {
        if let Ok(gb) = s.trim().parse::<f64>() { return Some((gb * (1u64 << 30) as f64) as u64); }
    }
    available_memory_bytes().map(|a| a / 10 * 9)
}

// ── model-level unification: discover + dedup GGUFs across the shared folder ──────────────────
use std::path::Path;

/// A discovered model file. `name` is the file stem for loose GGUFs, or "repo:tag" for ollama models.
#[derive(Clone, Debug)]
pub struct ModelEntry { pub name: String, pub path: PathBuf, pub len: u64, pub key: u64 }

/// Same value as `content_key(&fs::read(path))` but read by seeking the sampled ranges, so listing
/// many multi-GB models stays cheap (a few 64 KiB reads each, not a full load). It MUST match the
/// in-memory key byte-for-byte, or a cache entry keyed at load time wouldn't match one keyed from a
/// listing — `gate_store` asserts the equality directly.
pub fn content_key_file(path: &Path) -> std::io::Result<u64> {
    use std::hash::{Hash, Hasher};
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let n = f.metadata()?.len() as usize;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    n.hash(&mut h);
    let chunk = 64 * 1024;
    let mut buf = vec![0u8; chunk];
    for &s in &[0usize, n / 2, n.saturating_sub(chunk)] {
        let e = (s + chunk).min(n);
        if s < e {
            f.seek(SeekFrom::Start(s as u64))?;
            let want = e - s;
            f.read_exact(&mut buf[..want])?;
            buf[..want].hash(&mut h);
        }
    }
    Ok(h.finish())
}

fn walk_gguf(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth == 0 { return; }
    let rd = match std::fs::read_dir(dir) { Ok(r) => r, Err(_) => return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { walk_gguf(&p, depth - 1, out); }
        else if p.extension().and_then(|s| s.to_str()).map(|x| x.eq_ignore_ascii_case("gguf")).unwrap_or(false) {
            out.push(p);
        }
    }
}

/// Discover GGUFs under `roots`, deduped by content key: the same model present in two project
/// folders collapses to ONE entry (a path under the FIRST root — the canonical models dir — wins).
/// This is the unification win: stop carrying N copies; reference one. Recurses a bounded depth.
/// A root in ollama layout (has a `manifests/` subdir) is read via its manifests, not by extension —
/// ollama stores GGUFs as extension-less `blobs/sha256-*`, so a plain walk would miss them all.
pub fn list_models_in(roots: &[PathBuf]) -> Vec<ModelEntry> {
    let mut seen: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut out: Vec<ModelEntry> = Vec::new();
    let add = |e: ModelEntry, seen: &mut std::collections::HashMap<u64, usize>, out: &mut Vec<ModelEntry>| {
        if !seen.contains_key(&e.key) { seen.insert(e.key, out.len()); out.push(e); }
    };
    for r in roots {
        if r.join("manifests").is_dir() {
            for e in list_ollama(r) { add(e, &mut seen, &mut out); }
        } else {
            let mut paths = Vec::new();
            walk_gguf(r, 6, &mut paths);
            for p in paths {
                let (len, key) = match (std::fs::metadata(&p), content_key_file(&p)) {
                    (Ok(m), Ok(k)) => (m.len(), k), _ => continue,
                };
                let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("model").to_string();
                add(ModelEntry { name, path: p, len, key }, &mut seen, &mut out);
            }
        }
    }
    out
}
pub fn list_models() -> Vec<ModelEntry> { list_models_in(&[models_root()]) }

// ── ollama layout ────────────────────────────────────────────────────────────────────────────
// Ollama keeps models content-addressed: `manifests/<host>/<ns>/<repo>/<tag>` is a small JSON whose
// model layer (mediaType ".model") names a `blobs/sha256-<hex>` blob that IS a GGUF. We read the
// manifests (so names match `ollama ls`) and point at the blob — no copy, the blob loads directly.

/// Pull the model-layer (digest, size) out of an ollama manifest JSON. std-only: find the ".model"
/// media type, then the `sha256:` digest that follows it in the same layer object.
fn ollama_model_layer(json: &str) -> Option<(String, u64)> {
    let m = json.find(".model\"")?;                 // mediaType: application/vnd.ollama.image.model
    let after = &json[m..];
    let dpos = after.find("sha256:")? + "sha256:".len();
    let hex: String = after[dpos..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    if hex.is_empty() { return None; }
    // optional "size": <n> in the same layer object (best-effort)
    let size = after.find("\"size\":").and_then(|s| {
        after[s + 7..].trim_start().chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().ok()
    }).unwrap_or(0);
    Some((hex, size))
}

/// `manifests/<host>/<ns>/<repo>/<tag>` → display name like `ollama ls`: "repo:tag", or
/// "ns/repo:tag" when the namespace isn't the default `library`.
fn ollama_name(manifest: &Path, manifests_root: &Path) -> String {
    let rel = manifest.strip_prefix(manifests_root).unwrap_or(manifest);
    let parts: Vec<&str> = rel.components().filter_map(|c| c.as_os_str().to_str()).collect();
    // parts = [host, ns, repo, tag]
    match parts.as_slice() {
        [_, ns, repo, tag] if *ns == "library" => format!("{repo}:{tag}"),
        [_, ns, repo, tag] => format!("{ns}/{repo}:{tag}"),
        _ => rel.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"),
    }
}

/// List ollama-managed models under `root` (root = `.../models`, containing `manifests/` + `blobs/`).
pub fn list_ollama(root: &Path) -> Vec<ModelEntry> {
    let manifests = root.join("manifests");
    let blobs = root.join("blobs");
    let mut files = Vec::new();
    walk_files(&manifests, 8, &mut files);
    let mut out = Vec::new();
    for mf in files {
        let json = match std::fs::read_to_string(&mf) { Ok(s) => s, Err(_) => continue };
        let (hex, size) = match ollama_model_layer(&json) { Some(x) => x, None => continue };
        let blob = blobs.join(format!("sha256-{hex}"));
        if !blob.is_file() { continue; }
        let len = if size > 0 { size } else { std::fs::metadata(&blob).map(|m| m.len()).unwrap_or(0) };
        let key = match content_key_file(&blob) { Ok(k) => k, Err(_) => continue };
        out.push(ModelEntry { name: ollama_name(&mf, &manifests), path: blob, len, key });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn walk_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth == 0 { return; }
    let rd = match std::fs::read_dir(dir) { Ok(r) => r, Err(_) => return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { walk_files(&p, depth - 1, out); }
        else { out.push(p); }
    }
}

/// Redundant copies: groups (key, [paths]) where the same content lives at >1 path. The bytes in the
/// tail of each group are reclaimable (keep one, drop/link the rest). Reported, never auto-deleted.
pub fn duplicate_models(roots: &[PathBuf]) -> Vec<(u64, Vec<PathBuf>)> {
    let mut paths = Vec::new();
    for r in roots { walk_gguf(r, 6, &mut paths); }
    let mut by_key: std::collections::HashMap<u64, Vec<PathBuf>> = std::collections::HashMap::new();
    for p in paths { if let Ok(k) = content_key_file(&p) { by_key.entry(k).or_default().push(p); } }
    let mut dups: Vec<(u64, Vec<PathBuf>)> = by_key.into_iter().filter(|(_, v)| v.len() > 1).collect();
    for (_, v) in dups.iter_mut() { v.sort(); }
    dups.sort_by_key(|(k, _)| *k);
    dups
}

/// Resolve a model by absolute path (returned as-is if it exists), by file name within the unified
/// folder, or by an ollama "repo:tag" name — so a project can reference "llama3.2:1b" or
/// "deepseek-r1.gguf" without an absolute path.
pub fn resolve_model(name: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(name);
    if direct.is_file() { return Some(direct); }
    // ollama "repo:tag" across every detected store
    for r in ollama_roots() {
        if let Some(e) = list_ollama(&r).into_iter().find(|e| e.name == name) { return Some(e.path); }
    }
    // loose .gguf by file name in the logscope dir
    let mut paths = Vec::new();
    walk_gguf(&models_root(), 6, &mut paths);
    paths.into_iter().find(|p| p.file_name().and_then(|s| s.to_str()) == Some(name))
}

/// Self-test (`--gate-store`): content-key determinism + distinctness, env-overridable dirs,
/// LRU+`.tmp` eviction, and model discovery/dedup — all against scratch dirs. No GPU.
pub fn gate_store() {
    use std::io::Write;
    let mut ok = true;
    let mut check = |label: &str, cond: bool| { ok &= cond; println!("  [{}] {label}", if cond { "\u{2713}" } else { "\u{2717} FAIL" }); };

    // content_key: same bytes → same key; one byte different → different key; size matters.
    let a = vec![7u8; 200_000];
    let mut b = a.clone(); b[123_456] = 8;
    let c = vec![7u8; 200_001];
    check("content_key deterministic", content_key(&a) == content_key(&a));
    check("content_key sees a middle-chunk change", content_key(&a) != content_key(&b));
    check("content_key sees a length change", content_key(&a) != content_key(&c));
    check("tiny inputs don't panic", { let _ = content_key(b"hi"); let _ = content_key(b""); true });

    // dir overrides
    std::env::set_var("LOGSCOPE_CACHE_DIR", "/tmp/ls-cache-test");
    std::env::set_var("LOGSCOPE_MODELS_DIR", "/tmp/ls-models-test");
    check("LOGSCOPE_CACHE_DIR honored", cache_root() == PathBuf::from("/tmp/ls-cache-test"));
    check("LOGSCOPE_MODELS_DIR honored", models_root() == PathBuf::from("/tmp/ls-models-test"));
    check("cache_path is <root>/<hex>.int8", cache_path(0xdead_beef).ends_with("00000000deadbeef.int8"));

    // eviction: 3 blobs of 1MB, oldest-first removal under a 2.5MB cap; a .tmp is always swept.
    let dir = PathBuf::from("/tmp/ls-cache-evict-test");
    std::env::set_var("LOGSCOPE_CACHE_DIR", &dir);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mb = vec![0u8; 1 << 20];
    for name in ["a.int8", "b.int8", "c.int8"].iter() {
        std::fs::File::create(dir.join(name)).unwrap().write_all(&mb).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15)); // distinct, ordered mtimes a<b<c
    }
    std::fs::File::create(dir.join("stale.tmp")).unwrap().write_all(&mb).unwrap();
    let (removed, freed) = evict_to_cap(2_700_000); // keep ~2 of 3 blobs + drop the tmp
    check("eviction removed the .tmp + oldest blob", removed >= 2);
    check("eviction freed >= 2 MiB", freed >= 2 << 20);
    check("newest blob (c) survives", dir.join("c.int8").exists());
    check("oldest blob (a) evicted", !dir.join("a.int8").exists());
    let _ = std::fs::remove_dir_all(&dir);
    std::env::remove_var("LOGSCOPE_CACHE_DIR"); std::env::remove_var("LOGSCOPE_MODELS_DIR");

    // make-room-before-write + size readout + purge, against a fresh scratch dir.
    let rdir = PathBuf::from("/tmp/ls-cache-room-test");
    std::env::set_var("LOGSCOPE_CACHE_DIR", &rdir);
    let _ = std::fs::remove_dir_all(&rdir);
    std::fs::create_dir_all(&rdir).unwrap();
    for name in ["x.int8", "y.int8"].iter() {
        std::fs::File::create(rdir.join(name)).unwrap().write_all(&mb).unwrap(); // 1 MiB each
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    check("cache_size_bytes sums the blobs", cache_size_bytes() == 2 << 20);
    // cap 3 MiB, an incoming 2 MiB blob → must free down to ≤1 MiB first so write peak stays ≤ cap.
    let (rm, _f) = make_room_for(3 << 20, 2 << 20);
    check("make_room_for evicts to fit the incoming blob", rm >= 1 && cache_size_bytes() <= 1 << 20);
    std::fs::File::create(rdir.join("orphan.tmp")).unwrap().write_all(&mb).unwrap();
    let (pn, pf) = purge();
    check("purge clears .int8 + .tmp", pn >= 1 && pf >= 1 << 20 && cache_size_bytes() == 0);
    let _ = std::fs::remove_dir_all(&rdir);
    std::env::remove_var("LOGSCOPE_CACHE_DIR");

    // model discovery + dedup: a models dir with two IDENTICAL ggufs (different names) + one distinct.
    let mdir = PathBuf::from("/tmp/ls-models-dedup-test");
    let _ = std::fs::remove_dir_all(&mdir);
    std::fs::create_dir_all(mdir.join("projB")).unwrap();
    // distinct content per model; >chunk so head/mid/tail sampling is exercised.
    let mut m1 = vec![1u8; 300_000]; m1[150_000] = 9;
    let m2 = vec![2u8; 320_000];
    std::fs::File::create(mdir.join("alpha.gguf")).unwrap().write_all(&m1).unwrap();
    std::fs::File::create(mdir.join("projB/alpha-copy.gguf")).unwrap().write_all(&m1).unwrap(); // dup of alpha
    std::fs::File::create(mdir.join("beta.gguf")).unwrap().write_all(&m2).unwrap();
    // the cheap seeked key MUST equal the full-bytes key, or load-time and listing-time keys diverge.
    check("content_key_file == content_key(bytes)",
        content_key_file(&mdir.join("alpha.gguf")).unwrap() == content_key(&m1));
    let listed = list_models_in(&[mdir.clone()]);
    check("list_models dedups identical copies (3 files → 2 models)", listed.len() == 2);
    let dups = duplicate_models(&[mdir.clone()]);
    check("duplicate_models finds the one redundant pair", dups.len() == 1 && dups[0].1.len() == 2);
    std::env::set_var("LOGSCOPE_MODELS_DIR", &mdir);
    check("resolve_model finds by file name", resolve_model("beta.gguf") == Some(mdir.join("beta.gguf")));
    check("resolve_model misses an absent name", resolve_model("ghost.gguf").is_none());
    std::env::remove_var("LOGSCOPE_MODELS_DIR");
    let _ = std::fs::remove_dir_all(&mdir);

    // ollama layout: a manifest naming a .model blob → discovered by name, resolvable as repo:tag.
    let oroot = PathBuf::from("/tmp/ls-ollama-test");
    let _ = std::fs::remove_dir_all(&oroot);
    let hex = "abc123def456";
    let mdir2 = oroot.join("manifests/registry.ollama.ai/library/llama3.2/1b");
    std::fs::create_dir_all(mdir2.parent().unwrap()).unwrap();
    std::fs::create_dir_all(oroot.join("blobs")).unwrap();
    std::fs::write(&mdir2, format!(
        "{{\"schemaVersion\":2,\"layers\":[\
         {{\"mediaType\":\"application/vnd.ollama.image.model\",\"digest\":\"sha256:{hex}\",\"size\":320000}},\
         {{\"mediaType\":\"application/vnd.ollama.image.params\",\"digest\":\"sha256:0000\"}}]}}")).unwrap();
    std::fs::File::create(oroot.join(format!("blobs/sha256-{hex}"))).unwrap().write_all(&vec![3u8; 320_000]).unwrap();
    let oll = list_models_in(&[oroot.clone()]);
    check("ollama manifest → one model", oll.len() == 1);
    check("ollama name is repo:tag (library dropped)", oll.first().map(|e| e.name.as_str()) == Some("llama3.2:1b"));
    check("ollama entry points at the blob", oll.first().map(|e| e.path.ends_with(format!("sha256-{hex}"))).unwrap_or(false));
    std::env::set_var("OLLAMA_MODELS", &oroot);
    check("ollama_roots detects an OLLAMA_MODELS store", ollama_roots().contains(&oroot));
    check("all_model_roots includes the ollama store", all_model_roots().contains(&oroot));
    check("resolve_model finds an ollama model by repo:tag", resolve_model("llama3.2:1b").is_some());
    std::env::remove_var("OLLAMA_MODELS");
    let _ = std::fs::remove_dir_all(&oroot);

    // resident estimate: 8B-ish dims land ~10 GiB; bigger dims strictly larger; meminfo readable here.
    let est8 = resident_bytes_for(4096, 14336, 32, 32, 8, 128256, false);
    let est70 = resident_bytes_for(8192, 28672, 80, 64, 8, 128256, false);
    check("8B int8-resident estimate ~9-11 GiB", est8 > 9*(1u64<<30) && est8 < 11*(1u64<<30));
    check("70B estimate strictly larger than 8B", est70 > est8);
    check("LOGSCOPE_MAX_RESIDENT_GB overrides budget", { std::env::set_var("LOGSCOPE_MAX_RESIDENT_GB", "16"); let b = resident_budget_bytes() == Some(16*(1u64<<30)); std::env::remove_var("LOGSCOPE_MAX_RESIDENT_GB"); b });
    check("available memory is readable (Linux)", available_memory_bytes().map(|m| m > 0).unwrap_or(true));

    println!("gate-store: {}", if ok { "content-addressed store checks \u{2713}" } else { "FAIL \u{2717}" });
}

// ── Measured device bandwidth ───────────────────────────────────────────────────────────────────
//
// `--gate-vram` and `--gate-perf` measure what a device actually does, and until now nothing carried
// those figures back to a running engine. The consequence was concrete: perfverdict could never return
// AtBandwidthLimit — the one verdict that says STOP OPTIMISING, because the run is at the physical
// limit and the compute/submit/sync split is irrelevant.
//
// Stored per adapter, in the same tab-separated file style as the autotune tiles, because a measurement
// belongs to a device and not to a build.

fn bandwidth_file() -> PathBuf { cache_root().join("measured-bandwidth.tsv") }

/// Record a measured device bandwidth, in GiB/s.
///
/// Refuses a non-positive figure rather than storing it: 0 is the sentinel for "never measured", and
/// writing one would make an unmeasured device indistinguishable from a device measured at zero.
pub fn bandwidth_put(adapter: &str, gib_per_s: f32) {
    if !(gib_per_s > 0.0) { return; }
    let _ = ensure_cache_dir();
    let mut out = String::new();
    if let Ok(txt) = std::fs::read_to_string(bandwidth_file()) {
        for line in txt.lines() {
            if line.split('\t').next() != Some(adapter) {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out.push_str(&format!("{adapter}\t{gib_per_s}\n"));
    let _ = std::fs::write(bandwidth_file(), out);
}

/// The measured bandwidth for this adapter, if one was ever recorded.
///
/// `None` means unmeasured, and every caller must treat that as "cannot evaluate" rather than
/// substituting a default — a plausible default here produces a confident efficiency figure from no
/// measurement, which is exactly the 24 GiB constant's failure in a new place.
pub fn bandwidth_get(adapter: &str) -> Option<f32> {
    let txt = std::fs::read_to_string(bandwidth_file()).ok()?;
    for line in txt.lines() {
        let mut f = line.split('\t');
        if f.next() == Some(adapter) {
            return f.next().and_then(|v| v.parse::<f32>().ok()).filter(|v| *v > 0.0);
        }
    }
    None
}

#[cfg(test)]
mod bandwidth_tests {
    use super::*;

    #[test]
    fn an_unmeasured_adapter_returns_None_not_a_default() {
        // The whole point. A default here would produce a confident efficiency figure from no
        // measurement — the 24 GiB constant's failure in a new place.
        assert!(bandwidth_get("no-such-adapter-9f3a2b").is_none());
    }

    #[test]
    fn a_non_positive_measurement_is_REFUSED_rather_than_stored() {
        // 0 is the sentinel for "never measured". Storing one would make an unmeasured device
        // indistinguishable from a device measured at zero, and the caller could not tell.
        let a = "test-adapter-refuses-zero-8c1d";
        bandwidth_put(a, 0.0);
        bandwidth_put(a, -5.0);
        assert!(bandwidth_get(a).is_none());
    }

    #[test]
    fn a_measurement_round_trips_and_the_latest_wins() {
        let a = "test-adapter-roundtrip-4e7f";
        bandwidth_put(a, 258.1);
        assert_eq!(bandwidth_get(a), Some(258.1));
        // Re-measuring replaces rather than appending, or the file grows and the first line wins
        // forever — which would pin a device to whatever it measured on its worst day.
        bandwidth_put(a, 301.4);
        assert_eq!(bandwidth_get(a), Some(301.4));
    }

    #[test]
    fn adapters_do_not_collide() {
        bandwidth_put("test-adapter-A-11", 100.0);
        bandwidth_put("test-adapter-B-11", 200.0);
        assert_eq!(bandwidth_get("test-adapter-A-11"), Some(100.0));
        assert_eq!(bandwidth_get("test-adapter-B-11"), Some(200.0));
    }
}
