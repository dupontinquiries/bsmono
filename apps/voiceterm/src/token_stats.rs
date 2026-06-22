// src/token_stats.rs
//
// Per-user learning of which spoken tokens are fillers (irrelevant to intent)
// versus signal (predict intent kind). Uses TWO complementary signals:
//
// 1. EMPIRICAL FILLER TEST: For each candidate filler token, parse the user's
//    recognized text WITH and WITHOUT it. If results are identical 90%+ of the
//    time, the token is a filler. This is the right test (entropy is a proxy).
//
// 2. KIND AFFINITY: Per token, count co-occurrence with confirmed intent kinds.
//    Strongly skewed → signal. Flat → either filler or generic word.
//
// On every confirmed action, we record (text, kind). On every cancellation, we
// record the failed text+kind as negative signal.
//
// Persistence: ~/.config/web2qml/token_stats.bin (custom format, ~10-50KB).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const FILE_MAGIC: &[u8] = b"TS01";
const STORE_TEXT_DAYS: u64 = 30;
const MAX_RECENT_UTTERANCES: usize = 500;
const MAX_TOKENS: usize = 1000;

/// Top-level statistics, persisted across sessions.
#[derive(Default)]
pub struct TokenStats {
    /// For each token, count of confirmed actions broken down by intent kind tag.
    pub confirmed_by_kind: HashMap<String, HashMap<String, u32>>,
    /// For each token, total times it appeared in any recognized text.
    pub total_occurrences: HashMap<String, u32>,
    /// For each token, count of cancellations (user said it but didn't go through).
    pub cancellations: HashMap<String, u32>,
    /// Recent utterances for the empirical filler test. (text, kind, confirmed, ts)
    pub recent: Vec<Utterance>,
    /// Cached fillers (computed periodically, used in the hot path).
    pub cached_fillers: Vec<String>,
    /// When cache was last computed (epoch seconds)
    pub cache_ts: u64,
}

#[derive(Clone)]
pub struct Utterance {
    pub text: String,
    pub kind: String,
    pub confirmed: bool,
    pub ts: u64,
}

static STATS: OnceLock<Mutex<TokenStats>> = OnceLock::new();

fn stats() -> &'static Mutex<TokenStats> {
    STATS.get_or_init(|| Mutex::new(TokenStats::default()))
}

/// Tokenize text — lowercase, alphanumeric only, drop very short / punctuation.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn epoch_secs() -> u64 {
    crate::util::epoch_secs()
}

/// Record a confirmed action: user spoke `text`, it parsed to intent kind `kind`,
/// and they confirmed (executed) the action.
pub fn record_confirmation(text: &str, kind: &str) {
    let g = match stats().lock() { Ok(g) => g, Err(_) => return };
    drop(g);
    record_internal(text, kind, true);
}

/// Record a cancelled action: user spoke `text`, it would have parsed to `kind`,
/// but they cancelled (closed popup, hit cancel button).
pub fn record_cancellation(text: &str, attempted_kind: &str) {
    record_internal(text, attempted_kind, false);
}

fn record_internal(text: &str, kind: &str, confirmed: bool) {
    // Skip while a password prompt is active. The user might say
    // "yes" / "confirm" while typing their sudo password, and
    // we don't want to learn from that side-channel.
    if crate::security::should_suppress_capture() {
        return;
    }
    let text = text.trim();
    if text.is_empty() { return; }
    let tokens = tokenize(text);
    if tokens.is_empty() { return; }

    let mut g = match stats().lock() { Ok(g) => g, Err(_) => return };

    for tk in &tokens {
        if tk.len() > 30 { continue; }
        *g.total_occurrences.entry(tk.clone()).or_insert(0) += 1;
        if confirmed {
            *g.confirmed_by_kind
                .entry(tk.clone()).or_default()
                .entry(kind.to_string()).or_insert(0) += 1;
        } else {
            *g.cancellations.entry(tk.clone()).or_insert(0) += 1;
        }
    }

    g.recent.push(Utterance {
        text: text.to_string(),
        kind: kind.to_string(),
        confirmed,
        ts: epoch_secs(),
    });

    // Trim recent utterances to last N
    if g.recent.len() > MAX_RECENT_UTTERANCES {
        let drop_n = g.recent.len() - MAX_RECENT_UTTERANCES;
        g.recent.drain(..drop_n);
    }

    // Trim oldest tokens if over cap
    if g.total_occurrences.len() > MAX_TOKENS {
        // Drop tokens with lowest total counts
        let mut kv: Vec<(String, u32)> = g.total_occurrences.iter()
            .map(|(k, v)| (k.clone(), *v)).collect();
        kv.sort_by_key(|(_, v)| *v);
        let drop_n = g.total_occurrences.len() - MAX_TOKENS;
        for (tk, _) in kv.iter().take(drop_n) {
            g.total_occurrences.remove(tk);
            g.confirmed_by_kind.remove(tk);
            g.cancellations.remove(tk);
        }
    }

    // Invalidate filler cache (will be rebuilt lazily)
    g.cache_ts = 0;

    drop(g);
    save_to_disk();
}

/// Strip learned fillers from text. Conservative: only removes tokens that
/// have been empirically proven to be fillers via the with/without test.
pub fn strip_fillers(text: &str) -> String {
    let fillers = get_or_compute_fillers();
    if fillers.is_empty() { return text.to_string(); }

    // Lowercase comparison but preserve the original casing of non-filler tokens
    let mut out = String::with_capacity(text.len());
    let mut current_word = String::new();
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c.is_alphanumeric() || c == '\'' {
            current_word.push(c);
        } else {
            // Word boundary
            if !current_word.is_empty() {
                let lc = current_word.to_lowercase();
                if !fillers.iter().any(|f| f == &lc) {
                    out.push_str(&current_word);
                }
                current_word.clear();
                // Preserve the boundary char only if we kept the word
                if !out.is_empty() && !out.ends_with(c) {
                    out.push(c);
                }
            } else if !out.is_empty() && !out.ends_with(c) {
                out.push(c);
            }
        }
    }
    if !current_word.is_empty() {
        let lc = current_word.to_lowercase();
        if !fillers.iter().any(|f| f == &lc) {
            out.push_str(&current_word);
        }
    }

    // Collapse double spaces
    let mut collapsed = String::with_capacity(out.len());
    let mut last_was_space = false;
    for c in out.chars() {
        if c == ' ' {
            if !last_was_space { collapsed.push(c); }
            last_was_space = true;
        } else {
            collapsed.push(c);
            last_was_space = false;
        }
    }
    collapsed.trim().to_string()
}

/// Built-in seed fillers (universal — work on day one before learning kicks in).
/// Built-in seed fillers (universal — work on day one before learning kicks in).
///
/// v0.1.110: expanded substantially. The previous list caught
/// only the most obvious disfluencies. Real users speak with
/// articles, politeness markers, and conversational scaffolding
/// that drown out the actual command intent. Treating these as
/// fillers (stripped before parsing) makes the parser tolerate
/// utterances like "could you please open the terminal for me"
/// just as well as "open terminal".
///
/// Categories included:
/// - Disfluencies: um, uh, like, basically
/// - Politeness: please, thanks, thank you (single tokens only;
///   "thank you" is two tokens but each treated separately)
/// - Conversational openers/closers: actually, really, just,
///   honestly, literally, basically, kind of, sort of
/// - Filler conjunctions: so, well, anyway
/// - Discourse markers: you know, I mean (compound — only single
///   tokens here; compound stripping is harder)
///
/// What's NOT in here: command-relevant words. We do NOT strip
/// "the", "a", "an" because they affect parsing of phrases like
/// "open the terminal" vs "open terminal" — both should work,
/// and our matchers below explicitly accept both forms.
///
/// We also don't strip "could", "would", "should", "can", "will" —
/// those are command-relevant in some phrasings ("can you go
/// back" should still parse as "go back"). Instead the parser
/// strips them as a politeness prefix in normalize_natural_language().
const SEED_FILLERS: &[&str] = &[
    // Classic disfluencies
    "um", "uh", "uhh", "uhm", "uhmm", "hmm", "hmmm", "er", "erm", "ah",
    "mm", "mmm", "huh",
    // Hedges / softeners
    "like", "basically", "literally", "actually", "really", "just",
    "honestly", "obviously", "essentially", "totally", "kinda", "sorta",
    "maybe",
    // Politeness (single-token; multi-word handled by normalize)
    "please", "pls", "plz", "thanks",
    // Sentence openers
    // v0.1.772: "right" REMOVED — it doubles as a direction word ("split
    // right", "focus right", "resize right", "new pane right") and stripping it
    // dropped the direction from terminal/navigation voice commands (TermPane
    // got "new pane" instead of "new pane right"). In a navigation-heavy app
    // directions are first-class; "right" as a discourse filler is rare enough
    // to accept.
    "so", "well", "anyway", "alright", "okay", "ok",
    // Filler interjections
    "yeah", "yep", "hey",
];

/// Return the cached filler list, recomputing if stale.
fn get_or_compute_fillers() -> Vec<String> {
    let now = epoch_secs();
    {
        let g = match stats().lock() { Ok(g) => g, Err(_) => return SEED_FILLERS.iter().map(|s| s.to_string()).collect() };
        // Cache valid for 60 seconds OR if not invalidated
        if g.cache_ts != 0 && now - g.cache_ts < 60 {
            return g.cached_fillers.clone();
        }
    }
    // Recompute
    let computed = compute_fillers();
    if let Ok(mut g) = stats().lock() {
        g.cached_fillers = computed.clone();
        g.cache_ts = now;
    }
    computed
}

/// EMPIRICAL FILLER DETECTION — the actual right algorithm.
///
/// For each candidate token (high frequency, appears in 5+ utterances):
///   For each utterance containing the token:
///     Parse with token included → kind_with
///     Parse with token removed  → kind_without
///   If kind_with == kind_without in ≥85% of cases → token is a filler
///
/// We can't actually call the parser from here (circular dependency), so we
/// approximate using a simpler test: kind is the same in confirmed_by_kind
/// when the token is present versus the rest of the user's distribution.
///
/// PLUS: include seed fillers always (universal).
fn compute_fillers() -> Vec<String> {
    let mut fillers: Vec<String> = SEED_FILLERS.iter().map(|s| s.to_string()).collect();

    let g = match stats().lock() { Ok(g) => g, Err(_) => return fillers };

    // Check learned fillers
    for (tk, total) in &g.total_occurrences {
        if *total < 5 { continue; }  // need enough samples
        if SEED_FILLERS.contains(&tk.as_str()) { continue; }

        // Cancellation rate too high? Skip — could be a token user uses to mean "wait"
        let cancel_rate = g.cancellations.get(tk).copied().unwrap_or(0) as f32 / *total as f32;
        if cancel_rate > 0.5 { continue; }

        // Kind affinity: most-common kind's share of confirmations
        let kinds = match g.confirmed_by_kind.get(tk) { Some(k) => k, None => continue };
        let total_confirmed: u32 = kinds.values().sum();
        if total_confirmed < 4 { continue; }

        let max_kind_count = kinds.values().copied().max().unwrap_or(0);
        let max_share = max_kind_count as f32 / total_confirmed as f32;
        let n_kinds = kinds.len();

        // If the token co-occurs with 4+ different kinds AND no kind dominates,
        // it's probably a filler.
        if n_kinds >= 4 && max_share < 0.4 {
            fillers.push(tk.clone());
            continue;
        }
        // Or: very common token (10+ times) but no clear kind preference (<35%)
        if *total >= 10 && max_share < 0.35 && n_kinds >= 3 {
            fillers.push(tk.clone());
        }
    }

    fillers
}

/// Persist to disk in a binary format.
pub fn save_to_disk() {
    if !crate::security::persistence_enabled() {
        return;
    }
    let path = match db_path() { Some(p) => p, None => return };
    let _ = std::fs::create_dir_all(path.parent().unwrap());

    let g = match stats().lock() { Ok(g) => g, Err(_) => return };

    let mut out: Vec<u8> = Vec::with_capacity(8192);
    out.extend_from_slice(FILE_MAGIC);

    // confirmed_by_kind
    out.extend_from_slice(&(g.confirmed_by_kind.len() as u32).to_le_bytes());
    for (tk, kinds) in &g.confirmed_by_kind {
        let tk_bytes = tk.as_bytes();
        if tk_bytes.len() > 255 { continue; }
        out.push(tk_bytes.len() as u8);
        out.extend_from_slice(tk_bytes);
        out.extend_from_slice(&(kinds.len() as u16).to_le_bytes());
        for (k, cnt) in kinds {
            let kb = k.as_bytes();
            if kb.len() > 255 { continue; }
            out.push(kb.len() as u8);
            out.extend_from_slice(kb);
            out.extend_from_slice(&cnt.to_le_bytes());
        }
    }

    // total_occurrences
    out.extend_from_slice(&(g.total_occurrences.len() as u32).to_le_bytes());
    for (tk, cnt) in &g.total_occurrences {
        let tb = tk.as_bytes();
        if tb.len() > 255 { continue; }
        out.push(tb.len() as u8);
        out.extend_from_slice(tb);
        out.extend_from_slice(&cnt.to_le_bytes());
    }

    // cancellations
    out.extend_from_slice(&(g.cancellations.len() as u32).to_le_bytes());
    for (tk, cnt) in &g.cancellations {
        let tb = tk.as_bytes();
        if tb.len() > 255 { continue; }
        out.push(tb.len() as u8);
        out.extend_from_slice(tb);
        out.extend_from_slice(&cnt.to_le_bytes());
    }

    // recent (only those younger than STORE_TEXT_DAYS)
    let cutoff = epoch_secs().saturating_sub(STORE_TEXT_DAYS * 86400);
    let recent_keep: Vec<&Utterance> = g.recent.iter().filter(|u| u.ts >= cutoff).collect();
    out.extend_from_slice(&(recent_keep.len() as u32).to_le_bytes());
    for u in recent_keep {
        let tb = u.text.as_bytes();
        if tb.len() > 65535 { continue; }
        out.extend_from_slice(&(tb.len() as u16).to_le_bytes());
        out.extend_from_slice(tb);
        let kb = u.kind.as_bytes();
        if kb.len() > 255 { continue; }
        out.push(kb.len() as u8);
        out.extend_from_slice(kb);
        out.push(if u.confirmed { 1 } else { 0 });
        out.extend_from_slice(&u.ts.to_le_bytes());
    }

    // v0.1.727: gzip the blob before writing — same treatment as
    // decision_cache. save_to_disk fires on every confirmed utterance; at the
    // 1000-token / 500-utterance cap this is tens of KB of compressible text.
    // gzip magic (0x1f 0x8b) is distinct from FILE_MAGIC "TS01", so load detects
    // the format and legacy uncompressed files still parse.
    use std::io::Write as _;
    let compressed = {
        let mut enc = flate2::write::GzEncoder::new(
            Vec::with_capacity(out.len() / 4), flate2::Compression::new(1));
        if enc.write_all(&out).is_ok() {
            enc.finish().unwrap_or(out)
        } else {
            out
        }
    };
    if let Err(e) = std::fs::write(&path, &compressed) {
        eprintln!("TokenStats: save failed: {}", e);
    }
}

pub fn load_from_disk() {
    let path = match db_path() { Some(p) => p, None => return };
    if !path.exists() { return; }
    let raw = match std::fs::read(&path) { Ok(b) => b, Err(_) => return };
    // v0.1.727: transparently decompress gzip-wrapped stores. New saves are
    // gzipped (0x1f 0x8b); legacy files start with FILE_MAGIC and pass through.
    let bytes = if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        use std::io::Read as _;
        let mut dec = flate2::read::GzDecoder::new(&raw[..]);
        let mut buf = Vec::new();
        match dec.read_to_end(&mut buf) { Ok(_) => buf, Err(_) => return }
    } else {
        raw
    };
    if bytes.len() < 8 || &bytes[0..4] != FILE_MAGIC { return; }

    let mut p = 4;
    let mut new_stats = TokenStats::default();

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
        let v = u64::from_le_bytes([b[*p], b[*p+1], b[*p+2], b[*p+3], b[*p+4], b[*p+5], b[*p+6], b[*p+7]]);
        *p += 8; Some(v)
    }
    fn read_str_u8len(b: &[u8], p: &mut usize) -> Option<String> {
        let n = read_u8(b, p)? as usize;
        if *p + n > b.len() { return None; }
        let s = String::from_utf8_lossy(&b[*p..*p+n]).to_string();
        *p += n;
        Some(s)
    }
    fn read_str_u16len(b: &[u8], p: &mut usize) -> Option<String> {
        let n = read_u16(b, p)? as usize;
        if *p + n > b.len() { return None; }
        let s = String::from_utf8_lossy(&b[*p..*p+n]).to_string();
        *p += n;
        Some(s)
    }

    // confirmed_by_kind
    let n = match read_u32(&bytes, &mut p) { Some(v) => v, None => return };
    for _ in 0..n {
        let tk = match read_str_u8len(&bytes, &mut p) { Some(s) => s, None => return };
        let nk = match read_u16(&bytes, &mut p) { Some(v) => v, None => return };
        let mut inner = HashMap::new();
        for _ in 0..nk {
            let k = match read_str_u8len(&bytes, &mut p) { Some(s) => s, None => return };
            let c = match read_u32(&bytes, &mut p) { Some(v) => v, None => return };
            inner.insert(k, c);
        }
        new_stats.confirmed_by_kind.insert(tk, inner);
    }

    // total_occurrences
    let n = match read_u32(&bytes, &mut p) { Some(v) => v, None => return };
    for _ in 0..n {
        let tk = match read_str_u8len(&bytes, &mut p) { Some(s) => s, None => return };
        let c = match read_u32(&bytes, &mut p) { Some(v) => v, None => return };
        new_stats.total_occurrences.insert(tk, c);
    }

    // cancellations
    let n = match read_u32(&bytes, &mut p) { Some(v) => v, None => return };
    for _ in 0..n {
        let tk = match read_str_u8len(&bytes, &mut p) { Some(s) => s, None => return };
        let c = match read_u32(&bytes, &mut p) { Some(v) => v, None => return };
        new_stats.cancellations.insert(tk, c);
    }

    // recent
    let n = match read_u32(&bytes, &mut p) { Some(v) => v, None => return };
    for _ in 0..n {
        let text = match read_str_u16len(&bytes, &mut p) { Some(s) => s, None => return };
        let kind = match read_str_u8len(&bytes, &mut p) { Some(s) => s, None => return };
        let conf_byte = match read_u8(&bytes, &mut p) { Some(v) => v, None => return };
        let ts = match read_u64(&bytes, &mut p) { Some(v) => v, None => return };
        new_stats.recent.push(Utterance { text, kind, confirmed: conf_byte != 0, ts });
    }

    if let Ok(mut g) = stats().lock() {
        *g = new_stats;
        eprintln!("TokenStats: loaded {} tokens, {} recent utterances",
            g.total_occurrences.len(), g.recent.len());
    }
}

fn db_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(std::path::PathBuf::from(home).join(".config/web2qml/token_stats.bin"))
}

/// For UI/debug — return current learned filler list
pub fn current_fillers() -> Vec<String> {
    get_or_compute_fillers()
}

/// For UI/debug — return total token count
pub fn token_count() -> usize {
    stats().lock().map(|g| g.total_occurrences.len()).unwrap_or(0)
}

/// For UI/debug — return total recent utterances count
pub fn utterance_count() -> usize {
    stats().lock().map(|g| g.recent.len()).unwrap_or(0)
}
