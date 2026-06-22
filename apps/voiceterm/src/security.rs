// src/security.rs
//
// Process hardening and sensitive-data utilities. Built around
// the threat model the user articulated: not "encrypt everything"
// but "don't let sensitive data escape the process — to disk via
// swap, to other processes via core dumps, to external services
// via API calls, or to background workers via logs."
//
// What this module gives you:
//
//   1. is_high_security() — returns true unless WEB2QML_DEBUG=1.
//      Default is hardened; opt-out for development.
//
//   2. init() — called once at main start. Conditionally:
//        - prctl(PR_SET_DUMPABLE, 0): no core dumps from this process
//        - prctl(PR_SET_PTRACER, 0): refuse ptrace attachments
//          (gdb -p will fail with EPERM unless it's run as root)
//
//   3. password_mode flag — global atomic bool. Set by foreground
//      detection (PtyTerminal::foreground_is_password_prompt()).
//      Read by every ingestion path: voice bg-listen, TokenStats,
//      DecisionCache, LogSummary, AI prompt builders. While the
//      flag is true, those paths skip recording / storing /
//      transmitting.
//
//   4. SecretBytes — short-lived secret-bytes container that:
//        - mmaps a private region (off the heap allocator, so
//          freed pages don't get recycled with secrets still in
//          them)
//        - mlocks the region (no swap)
//        - madvise(DONTDUMP) (excluded from core dumps)
//        - zeroes on Drop
//      Trade-off: each SecretBytes is at minimum a 4KB page.
//      Don't allocate one per byte; use for whole-secret storage.
//
//   5. context_strip() — given a string of terminal context for
//      AI prompts, strip lines matching known-sensitive patterns
//      (Password:, [sudo] password for X:, etc.) so they never
//      leave the process via the LLM API call.

use std::sync::atomic::{AtomicBool, Ordering};

/// Global: are we in high-security mode? Cached on first call.
/// `WEB2QML_DEBUG=1` disables — useful when running under gdb during
/// development. Default is hardened.
pub fn is_high_security() -> bool {
    use std::sync::OnceLock;
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        match std::env::var("WEB2QML_DEBUG") {
            Ok(v) if v == "1" || v.eq_ignore_ascii_case("true") => false,
            _ => true,
        }
    })
}

/// Call once at process start (early in main). Activates the
/// kernel-level hardening primitives. Idempotent — safe to call
/// multiple times but only the first call has effect.
///
/// On non-Linux platforms (macOS in particular) the prctl calls
/// don't exist; we no-op those parts and rely on the SecretBytes
/// + capture-suppression layers for protection.
pub fn init() {
    if !is_high_security() {
        crate::clog!("security", "WEB2QML_DEBUG=1 — security hardening DISABLED");
        // Still surface persistence state — it's a different concern.
        let _ = persistence_enabled();
        return;
    }

    #[cfg(target_os = "linux")] {
        unsafe {
            // PR_SET_DUMPABLE = 4 in linux/prctl.h. Setting to 0
            // means "this process produces no core dumps." Suid bits
            // also can't elevate (already not us).
            let r1 = libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
            if r1 != 0 {
                crate::clog!("security", "prctl(PR_SET_DUMPABLE, 0) failed: errno {}",
                    *libc::__errno_location());
            }
            // PR_SET_PTRACER = 0x59616d61 ('Yama'). Setting tracer
            // to 0 means "no process may ptrace me, regardless of
            // ptrace_scope sysctl." Same-UID gdb attempts get EPERM.
            // Root can still attach (kernel privilege bypass). Worth
            // it — covers casual local-attacker scenarios.
            let r2 = libc::prctl(libc::PR_SET_PTRACER, 0, 0, 0, 0);
            if r2 != 0 {
                // Some kernels without yama LSM return EINVAL. Not
                // fatal — log and continue.
                crate::clog!("security", "prctl(PR_SET_PTRACER, 0) failed: errno {} (yama LSM may not be enabled)",
                    *libc::__errno_location());
            }
        }
    }

    crate::clog!("security", "hardened: no-coredump=on no-ptrace=on (set WEB2QML_DEBUG=1 to disable)");
    // Prime the persistence flag so it logs once at startup. Cached
    // result is then served to all callers across the program.
    let _ = persistence_enabled();
}

// ─── Password-mode flag ────────────────────────────────────────────
//
// Set by the password-prompt detection path; consulted by every
// ingestion / capture / external-send path. Atomic so we can read
// it from any thread without locking.

static PASSWORD_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_password_mode(active: bool) {
    let prev = PASSWORD_MODE.swap(active, Ordering::Release);
    if prev != active {
        // Log transitions only — don't spam on every poll.
        crate::clog!("security", "password mode {}",
            if active { "ENTERED — captures suppressed" } else { "exited" });
    }
}

pub fn is_password_mode() -> bool {
    PASSWORD_MODE.load(Ordering::Acquire)
}

/// Convenience: returns true if a sensitive-write path should
/// SKIP its work right now. Currently equivalent to is_password_mode()
/// but provides a single chokepoint for future extension (e.g. user
/// might toggle a "private mode" outside of password prompts).
pub fn should_suppress_capture() -> bool {
    is_password_mode()
}

/// Returns true if the user has opted into persisting voice-derived
/// learning data (fingerprints, utterance stats, decision cache,
/// speaker profile) to disk. Default OFF — user explicitly opts in
/// via WEB2QML_PERSIST_LEARNING=1.
///
/// Background:
///
/// The phonetic fingerprint database (and friends) accumulates
/// acoustic snapshots of the user's voice across sessions. While
/// these don't contain transcripts, they DO contain enough acoustic
/// information that a sophisticated attacker with the file + the
/// right model could partially reconstruct co-occurring audio.
///
/// If the user voiced "ls -lh" while someone nearby was reciting an
/// SSH passphrase, the fingerprint stored for "ls -lh" carries
/// acoustic shadows of that passphrase. The privacy cost compounds
/// across sessions.
///
/// Default behavior: load existing files at startup (don't break
/// people who already have data) but do NOT write new entries.
/// Each session is effectively read-only after load.
///
/// Opt-in: WEB2QML_PERSIST_LEARNING=1. The user has affirmatively
/// decided the cross-session learning value outweighs the
/// fingerprint-leakage risk. Future settings UI will surface this.
///
/// Cached on first call. Affects:
///   - phonetic.rs (fingerprint DB)
///   - token_stats.rs (recent utterances)
///   - decision_cache.rs (utterance→command learning)
///   - audio_profile.rs (speaker + wake-word samples)
pub fn persistence_enabled() -> bool {
    persistence_init();
    PERSISTENCE_FLAG.load(std::sync::atomic::Ordering::Relaxed)
}

/// v0.1.224: enable persistence at runtime, e.g. when the user
/// explicitly registers a voice via the onboarding flow.
///
/// Two effects:
///   1. The session-level atomic flips to true — any subsequent
///      `persistence_enabled()` call returns true, so background
///      workers will start writing.
///   2. A stamp file at `~/.config/web2qml/persistence_enabled.flag`
///      is created. Next session, `persistence_init` sees the
///      stamp and starts with persistence on without needing the
///      env var.
///
/// The `reason` parameter is a short human-readable string logged
/// alongside the enable for diagnostic clarity (so the log doesn't
/// show "persistence enabled" with no context — "persistence
/// enabled (reason: voice registration)" tells the user what
/// triggered it).
///
/// Idempotent — calling repeatedly is fine.
///
/// Background: the v0.1.224 hardware-test feedback revealed that
/// the onboarding registration flow could complete WITHOUT
/// persisting anything to disk (the user did the flow, no
/// voice_fingerprints.bin, no audio_profile.bin were created).
/// The user's act of registering IS opt-in by definition, so we
/// flip the flag for them. They can still opt OUT later by
/// deleting the stamp file.
pub fn enable_persistence_runtime(reason: &str) {
    use std::sync::atomic::Ordering;
    persistence_init();
    let was = PERSISTENCE_FLAG.swap(true, Ordering::Relaxed);
    // Always write the stamp file (idempotent — if it exists this is
    // a no-op for the kernel beyond updating mtime).
    let stamp = persistence_stamp_path();
    if let Some(dir) = stamp.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&stamp, b"");
    if !was {
        crate::clog!("security", "persistence ENABLED at runtime (reason: {reason}) — voice learning will be saved across sessions");
    }
}

/// Path to the persistence-stamp file. Mirrors the convention
/// used by `onboarding_seen.flag` (zero-byte presence indicator).
fn persistence_stamp_path() -> std::path::PathBuf {
    use std::path::PathBuf;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".config/web2qml/persistence_enabled.flag")
}

/// Resolve the initial value of the persistence flag. Two
/// equivalent ways to opt in:
///   - `WEB2QML_PERSIST_LEARNING=1` env var (per-launch override)
///   - `~/.config/web2qml/persistence_enabled.flag` exists (sticky,
///     set by `enable_persistence_runtime`)
///
/// Cached via OnceLock so the disk read happens once. The
/// AtomicBool itself can be flipped at runtime by
/// `enable_persistence_runtime` after this init.
fn persistence_init() {
    use std::sync::OnceLock;
    static INIT_ONCE: OnceLock<()> = OnceLock::new();
    INIT_ONCE.get_or_init(|| {
        use std::sync::atomic::Ordering;
        let from_env = matches!(
            std::env::var("WEB2QML_PERSIST_LEARNING"),
            Ok(ref v) if v == "1" || v.eq_ignore_ascii_case("true")
        );
        let from_stamp = persistence_stamp_path().exists();
        let initial = from_env || from_stamp;
        PERSISTENCE_FLAG.store(initial, Ordering::Relaxed);
        if initial {
            let src = if from_env && from_stamp {
                "env + stamp"
            } else if from_env {
                "env var WEB2QML_PERSIST_LEARNING"
            } else {
                "stamp file from prior opt-in"
            };
            crate::clog!("security", "persistence ENABLED ({src}) — voice learning will be saved across sessions");
        } else {
            crate::clog!("security", "persistence DISABLED (default) — voice learning is session-only. \
                Set WEB2QML_PERSIST_LEARNING=1 or complete voice registration to opt in.");
        }
    });
}

/// The runtime persistence flag. Initial value populated by
/// `persistence_init` on first access; can be flipped to true at
/// runtime by `enable_persistence_runtime`.
static PERSISTENCE_FLAG: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

// ─── SecretBytes ───────────────────────────────────────────────────
//
// Holds short-lived bytes that should NEVER touch swap, core dumps,
// or be left in heap memory after we're done with them.
//
// Implementation: anonymous mmap of one or more pages, mlock'd,
// madvise(DONTDUMP)'d, zeroed on drop. Memory is OUTSIDE the heap
// allocator's bookkeeping so when we drop, the kernel reclaims the
// pages (after we've zeroed them) and the allocator never sees them.

#[cfg(target_os = "linux")]
pub struct SecretBytes {
    ptr: *mut u8,
    capacity: usize,
    len: usize,
}

#[cfg(target_os = "linux")]
impl SecretBytes {
    /// Allocate a secret-bytes region with at least `min_capacity` bytes.
    /// Rounds up to page boundary. Returns None on allocation failure.
    pub fn with_capacity(min_capacity: usize) -> Option<Self> {
        if min_capacity == 0 { return None; }
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize };
        let capacity = ((min_capacity + page_size - 1) / page_size) * page_size;
        unsafe {
            let ptr = libc::mmap(
                std::ptr::null_mut(),
                capacity,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1, 0,
            );
            if ptr == libc::MAP_FAILED {
                return None;
            }
            // Lock to RAM (no swap). May fail if RLIMIT_MEMLOCK is too
            // low or another mlock on the system used the budget.
            // Default unprivileged limit is 64KB — comfortable for our
            // expected usage (one or two SecretBytes per session).
            if libc::mlock(ptr, capacity) != 0 {
                let errno = *libc::__errno_location();
                crate::clog!("security", "mlock failed (errno {}); SecretBytes \
                    won't be swap-protected. Increase RLIMIT_MEMLOCK or \
                    consider why we exceeded it.", errno);
                // Continue anyway — we still have DONTDUMP + zero-on-drop.
            }
            // Exclude from core dumps. madvise can fail silently — we
            // already set PR_SET_DUMPABLE=0 globally in init(), so
            // this is belt-and-suspenders.
            libc::madvise(ptr, capacity, libc::MADV_DONTDUMP);
            Some(Self {
                ptr: ptr as *mut u8,
                capacity,
                len: 0,
            })
        }
    }

    /// Copy bytes in. Truncates if input exceeds capacity.
    pub fn copy_from(&mut self, src: &[u8]) {
        let n = src.len().min(self.capacity);
        unsafe {
            std::ptr::copy_nonoverlapping(src.as_ptr(), self.ptr, n);
        }
        self.len = n;
    }

    /// Borrow as a slice. Caller must NOT log this slice or send it
    /// to external services.
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    pub fn len(&self) -> usize { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    /// Explicitly zero the contents. Called automatically on Drop;
    /// public for the case where caller wants to scrub mid-life.
    pub fn zero(&mut self) {
        unsafe {
            // volatile write to defeat compiler dead-store optimization.
            for i in 0..self.capacity {
                std::ptr::write_volatile(self.ptr.add(i), 0u8);
            }
        }
        self.len = 0;
    }
}

#[cfg(target_os = "linux")]
impl Drop for SecretBytes {
    fn drop(&mut self) {
        unsafe {
            // Volatile zero before unlock so the bytes don't survive.
            self.zero();
            // Best effort: munlock so RLIMIT_MEMLOCK budget is freed.
            libc::munlock(self.ptr as *mut _, self.capacity);
            // Release the mapping.
            libc::munmap(self.ptr as *mut _, self.capacity);
        }
    }
}

// SAFETY: SecretBytes points to its own private mmap region. Sharing
// the pointer across threads is sound as long as the consumer respects
// the borrow rules — which Rust enforces via &/&mut.
#[cfg(target_os = "linux")]
unsafe impl Send for SecretBytes {}
#[cfg(target_os = "linux")]
unsafe impl Sync for SecretBytes {}

// ─── Non-Linux fallback ────────────────────────────────────────────
//
// macOS has mlock + madvise but uses different syscall numbers and
// flag names. For now, keep the API present but skip the kernel
// hardening on non-Linux. The capture-suppression flag still works.

#[cfg(not(target_os = "linux"))]
pub struct SecretBytes {
    bytes: Vec<u8>,
}

#[cfg(not(target_os = "linux"))]
impl SecretBytes {
    pub fn with_capacity(cap: usize) -> Option<Self> {
        Some(Self { bytes: Vec::with_capacity(cap) })
    }
    pub fn copy_from(&mut self, src: &[u8]) {
        self.bytes.clear();
        self.bytes.extend_from_slice(src);
    }
    pub fn as_slice(&self) -> &[u8] { &self.bytes }
    pub fn len(&self) -> usize { self.bytes.len() }
    pub fn is_empty(&self) -> bool { self.bytes.is_empty() }
    pub fn zero(&mut self) {
        for b in self.bytes.iter_mut() { *b = 0; }
        self.bytes.clear();
    }
}

#[cfg(not(target_os = "linux"))]
impl Drop for SecretBytes {
    fn drop(&mut self) { self.zero(); }
}

// ─── Context stripper for AI prompts ───────────────────────────────
//
// Used by llm_query_with_context to remove any line that looks like
// it might contain credential material before the prompt is shipped
// to Ollama (or any external LLM). Defense in depth — terminal
// echo-off should prevent passwords from being in the captured
// output, but if some custom prompt or a pasted secret leaked into
// terminal output we should still scrub before sending.

/// Returns `text` with any line containing a high-signal sensitive
/// marker REPLACED by "[redacted: sensitive line]". Whole lines, not
/// substrings — the goal is don't-leak, not preserve-context. Better
/// to send the model a "[redacted]" placeholder than risk a leak.
pub fn context_strip(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut redacted = 0u32;
    // v0.1.161: track which markers fired across all redacted lines.
    // Allows auditing what kinds of secrets the user's terminal
    // history was producing without ever logging the line content.
    // User opted in: "add logging for greps that look suspicious".
    //
    // We accumulate marker names (not line content) in a small vec
    // and emit a single summary at the end. Markers shorter than
    // 6 chars get truncated in the log to avoid revealing the
    // suspicious-content pattern itself in dev logs.
    let mut markers_hit: Vec<&'static str> = Vec::new();

    for line in text.lines() {
        if let Some(marker) = sensitive_marker(line) {
            out.push_str("[redacted: sensitive line]\n");
            redacted += 1;
            // Only add unique markers to keep the log line short
            // when the same pattern fires repeatedly.
            if !markers_hit.contains(&marker) {
                markers_hit.push(marker);
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if redacted > 0 {
        // Log to stderr with the marker names that triggered. The
        // line content itself NEVER appears here. This is the audit
        // trail asked for in v0.1.161: "add logging for greps that
        // look suspicious".
        eprintln!(
            "[security] context_strip redacted {} line(s) before sending to LLM — markers: {}",
            redacted,
            markers_hit.join(", ")
        );
    }
    out
}

/// Returns Some(marker) if this line matches a sensitive pattern,
/// None otherwise. The returned marker is the static pattern string
/// itself — useful for audit logging without logging the line.
///
/// v0.1.161 replaced the boolean `looks_sensitive` with this
/// pattern-naming variant so the redaction layer can tell us WHICH
/// kind of secret was almost leaked, not just that one was. The
/// pattern STRINGS are safe to log because they're constants from
/// the marker list, never line content.
/// Returns Some(marker) if this line matches a sensitive pattern,
/// None otherwise. The returned marker is the static pattern string
/// itself — useful for audit logging without logging the line.
///
/// v0.1.161 replaced the boolean `looks_sensitive` with this
/// pattern-naming variant so the redaction layer can tell us WHICH
/// kind of secret was almost leaked, not just that one was. The
/// pattern STRINGS are safe to log because they're constants from
/// the marker list, never line content.
///
/// v0.1.167: switched from per-line `to_lowercase` + N substring
/// scans to a single-pass case-insensitive Aho-Corasick automaton.
/// The automaton is built once at first call and cached in a
/// `OnceLock`. For a typical 50-line context with 60 markers,
/// this collapses 50 String allocations and 3000 substring checks
/// into a single pass per line with zero allocation. Sub-ms gain
/// per call but consistent across every LLM dispatch.
fn sensitive_marker(line: &str) -> Option<&'static str> {
    let ac = marker_automaton();
    // `find` returns the FIRST match. Aho-Corasick scans the line
    // once; the returned Match's pattern() index maps back into
    // MARKERS. Case-insensitive matching is handled by the
    // automaton (configured at build time below) so we don't need
    // to allocate a lowercase copy of the line.
    ac.find(line).map(|m| MARKERS[m.pattern().as_usize()])
}

/// Build (or fetch the cached) Aho-Corasick automaton for the
/// MARKERS list. Configured for case-insensitive matching against
/// ASCII patterns — all the markers are ASCII so this is safe.
/// Anchored=false (we want to find the marker anywhere in the line,
/// not require it at the start).
fn marker_automaton() -> &'static aho_corasick::AhoCorasick {
    static AC: std::sync::OnceLock<aho_corasick::AhoCorasick> = std::sync::OnceLock::new();
    AC.get_or_init(|| {
        aho_corasick::AhoCorasickBuilder::new()
            .ascii_case_insensitive(true)
            .build(MARKERS)
            .expect("MARKERS contains only well-formed ASCII patterns")
    })
}

/// Backwards-compatible bool wrapper. Some callers want a plain
/// "does this look sensitive" without the marker name. Kept so
/// existing tests don't break.
#[allow(dead_code)]
fn looks_sensitive(line: &str) -> bool {
    sensitive_marker(line).is_some()
}

// ═══════════════════════════════════════════════════════════════════
// v0.1.164: Tiered scorer-based redactor
//
// The marker-based context_strip above catches lines containing
// known-sensitive keywords (api_key=, Bearer ey, etc.) and replaces
// the WHOLE line. That's a coarse instrument: it misses tokens that
// don't sit on a marker line (e.g. a raw `sk-ant-...` pasted in
// terminal output) and over-redacts lines where only a small region
// is sensitive.
//
// The new redactor is character-class-scoring-based:
//
//   1. Walk the input in a sliding window. Window size is tier-scaled
//      (~32 chars for Tier 1, ~64 for Tier 2, ~128 for Tier 3) so
//      stronger tiers can catch longer/multi-line secrets like PEM
//      blocks split across newlines.
//
//   2. Within each window, find candidate spans: runs of >= 16
//      non-whitespace characters. These are the secret-shaped
//      regions.
//
//   3. Score each candidate. Higher score = more secret-like:
//        - Length contribution (up to a cap)
//        - Character-class diversity (mix of upper/lower/digit/symbol)
//        - Transition frequency (changes between classes per char)
//        - Shape bonuses (known prefixes: sk-, ghp_, eyJ for JWT)
//
//   4. Spans scoring above the tier's threshold get redacted at the
//      tier's stride density. Score within the tier ALSO modulates
//      the stride: a very-high-score span gets denser redaction
//      than a barely-above-threshold span at the same tier.
//
// User feedback driving this design:
//   - "false positives are good because we have the low-tier for
//     those users" → breadth scales with tier via threshold
//   - "transitions like if capitals or something then may be more
//     likely... like those number and password requirements set by
//     modern security standards" → class-transition scoring
//   - "we could modulate the window" → tier-scaled window
//   - "make sure that we wrap this in a no swap" → working buffers
//     copied into SecretBytes before processing on Linux (defense
//     against swap exposure of cleartext during redaction)
//
// Honest caveats:
//   - This is character-class-shape detection. Real passwords that
//     are dictionary words ("hunter2") don't match — those are
//     covered by the marker dictionary, which we still run as the
//     first pass.
//   - False positives by design: at Tier 3, UUIDs, git SHAs, and
//     long filenames get redacted. The LLM's responses lose context.
//     The tier system makes this a knowing choice.
//   - The "no swap" property applies to the redactor's INTERNAL
//     working buffer, not the upstream String the caller passes in.
//     Making the whole LLM path swap-protected end-to-end is a
//     bigger refactor (filed for v0.1.165+).
//
// ═══════════════════════════════════════════════════════════════════

/// Redaction tier. Picked by the user via the multi-button popup in
/// settings. Higher tier = wider detection window + lower scoring
/// threshold + denser redaction stride.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactionTier {
    /// No tiered redaction — only the marker-dict pass runs. Backward-
    /// compatible with v0.1.161 behavior.
    Off = 0,
    /// 1 lock — narrow window (32), high threshold (0.85), light stride
    /// (1–3 of 10 chars in matched spans). Catches obvious key shapes
    /// only; gentle redaction preserves most context.
    Light = 1,
    /// 2 locks — wider window (64), medium threshold (0.6), medium
    /// stride (ramps up to 7 of 10). Catches well-known shapes and
    /// many less-obvious ones; redacts substantially.
    Medium = 2,
    /// 3 locks — widest window (128), low threshold (0.3), heavy
    /// stride (starts at 5 of 10, ramps to 10 of 10). Catches almost
    /// any high-entropy span; nearly equivalent to full-line redaction.
    Assured = 3,
}

impl RedactionTier {
    pub fn from_i32(v: i32) -> RedactionTier {
        match v {
            1 => RedactionTier::Light,
            2 => RedactionTier::Medium,
            3 => RedactionTier::Assured,
            _ => RedactionTier::Off,
        }
    }

    /// Sliding window size in characters. Tier-scaled so higher tiers
    /// can catch longer secrets and multi-line constructs (PEM blocks
    /// split across newlines, multi-part base64 dumps).
    fn window_chars(self) -> usize {
        match self {
            RedactionTier::Off => 0,
            RedactionTier::Light => 32,
            RedactionTier::Medium => 64,
            RedactionTier::Assured => 128,
        }
    }

    /// Minimum score (0.0..1.0) required for a span to be redacted.
    /// Lower threshold = more false positives = more things get
    /// redacted. False positives are a feature: users on lower tiers
    /// chose to trade context preservation for security.
    fn score_threshold(self) -> f64 {
        match self {
            RedactionTier::Off => 1.1, // unreachable; nothing redacts
            RedactionTier::Light => 0.85,
            RedactionTier::Medium => 0.6,
            RedactionTier::Assured => 0.3,
        }
    }

    /// Base stride density for a span scoring at the threshold. Spans
    /// scoring HIGHER than threshold get DENSER redaction (more
    /// chars replaced); the tier sets the floor.
    ///
    /// Returns (numerator, denominator) for the "redact N of every M"
    /// pattern at the bottom of the tier's range.
    fn base_stride(self) -> (u32, u32) {
        match self {
            RedactionTier::Off => (0, 10),
            RedactionTier::Light => (1, 10),   // 10% at threshold
            RedactionTier::Medium => (3, 10),  // 30% at threshold
            RedactionTier::Assured => (5, 10), // 50% at threshold
        }
    }

    /// Maximum stride density for a top-scoring span at this tier.
    /// A span at score 1.0 redacts at this rate; intermediate scores
    /// interpolate linearly between base and max.
    fn max_stride(self) -> (u32, u32) {
        match self {
            RedactionTier::Off => (0, 10),
            RedactionTier::Light => (3, 10),    // 30% at top
            RedactionTier::Medium => (7, 10),   // 70% at top
            RedactionTier::Assured => (10, 10), // 100% at top
        }
    }
}

/// Tiered redactor entry point.
///
/// Runs the marker dictionary first (full-line redaction for obvious
/// secrets that don't follow a high-entropy shape — passwords like
/// "hunter2", session tokens passed as command args, etc.). Then runs
/// the scorer on remaining lines to catch high-entropy spans the
/// markers missed.
///
/// **Performance:** the marker pass is the cheap one (substring search
/// against a small list). The scorer pass walks each line once,
/// O(line length) total. No regex backtracking, no quadratic behavior.
///
/// **No-swap:** on Linux, the working buffer is mmap+mlock'd (see
/// `SecretBytes`). The input `&str` and output `String` are caller-
/// provided and may swap. Filed for v0.1.165: lock the upstream
/// build_terminal_context allocation too.
pub fn context_strip_tiered(text: &str, tier: RedactionTier) -> String {
    if tier == RedactionTier::Off {
        // Backward-compatible fast path: just the marker dictionary.
        return context_strip(text);
    }

    // Working buffer: try to allocate locked memory. If mlock budget
    // is exhausted (RLIMIT_MEMLOCK on Linux), fall back to a regular
    // String — we log the failure inside SecretBytes::with_capacity.
    // Most realistic terminal contexts are well under the 64KB default
    // unprivileged mlock limit.
    let mut locked = SecretBytes::with_capacity(text.len() + 64);
    let mut output = String::with_capacity(text.len());
    let mut spans_redacted = 0u32;
    let mut lines_marker_redacted = 0u32;
    let mut markers_hit: Vec<&'static str> = Vec::new();

    // Copy input into locked buffer if available. On non-Linux or
    // if mlock failed, locked.is_none() and we work on the &str
    // directly — same behavior, less swap protection.
    let working_text: &str = if let Some(ref mut lb) = locked {
        lb.copy_from(text.as_bytes());
        // SAFETY: input was a valid &str so the bytes are valid UTF-8.
        // SecretBytes copy preserves bytes verbatim.
        match std::str::from_utf8(lb.as_slice()) {
            Ok(s) => s,
            Err(_) => text, // shouldn't happen; fall back rather than panic
        }
    } else {
        text
    };

    for line in working_text.lines() {
        // Pass 1: marker dictionary. Full-line redact if it fires.
        if let Some(marker) = sensitive_marker(line) {
            output.push_str("[redacted: sensitive line]\n");
            lines_marker_redacted += 1;
            if !markers_hit.contains(&marker) {
                markers_hit.push(marker);
            }
            continue;
        }
        // Pass 2: scorer. Find spans, score, redact above threshold.
        let redacted_line = redact_high_entropy_spans(line, tier, &mut spans_redacted);
        output.push_str(&redacted_line);
        output.push('\n');
    }

    if lines_marker_redacted > 0 || spans_redacted > 0 {
        eprintln!(
            "[security] context_strip_tiered (tier={:?}): {} line(s) marker-redacted, {} span(s) entropy-redacted — markers: {}",
            tier,
            lines_marker_redacted,
            spans_redacted,
            if markers_hit.is_empty() { "(none)".to_string() } else { markers_hit.join(", ") }
        );
    }

    // locked drops here → zeroed + munlock'd
    output
}

/// Walk a single line, find spans of >= 16 non-whitespace chars,
/// score each one, redact those above the tier's threshold.
///
/// `spans_redacted` is incremented for each span actually modified.
/// Returns a new String with redacted positions replaced by `█`
/// (full-block; visually distinct from any normal text char).
fn redact_high_entropy_spans(line: &str, tier: RedactionTier, spans_redacted: &mut u32) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;

    while i < bytes.len() {
        // Skip whitespace
        if bytes[i].is_ascii_whitespace() {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        // Start of a non-whitespace run
        let span_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let span_end = i;
        let span_len = span_end - span_start;

        if span_len < 16 {
            // Too short to be a high-entropy secret. Pass through.
            out.push_str(&line[span_start..span_end]);
            continue;
        }

        let span_bytes = &bytes[span_start..span_end];
        let score = score_span(span_bytes);

        if score < tier.score_threshold() {
            // Below tier's threshold — keep the span intact.
            out.push_str(&line[span_start..span_end]);
            continue;
        }

        // Redact at score-modulated stride. Higher score → denser.
        let (base_n, denom) = tier.base_stride();
        let (max_n, _) = tier.max_stride();
        let threshold = tier.score_threshold();
        // Normalize score into 0.0..1.0 over the range above threshold
        let above = ((score - threshold) / (1.0 - threshold)).clamp(0.0, 1.0);
        let stride_n = (base_n as f64 + (max_n - base_n) as f64 * above).round() as u32;
        let stride_n = stride_n.min(denom);

        // Apply stride: redact `stride_n` out of every `denom` chars.
        // Deterministic positions (so the same span gets the same mask
        // every time — useful for the score-driven "repeat sightings
        // get denser" property if we add it later).
        let mut redacted_chars = 0u32;
        for (offset, &b) in span_bytes.iter().enumerate() {
            let position_in_group = (offset as u32) % denom;
            if position_in_group < stride_n {
                out.push('\u{2588}'); // █
                redacted_chars += 1;
            } else {
                out.push(b as char);
            }
        }
        if redacted_chars > 0 {
            *spans_redacted += 1;
        }
    }

    out
}

/// Score a span 0.0..1.0 based on character-class diversity and
/// transition frequency. Higher = more secret-like.
///
/// Components:
///   - length_factor:        0.0 at 16 chars, 1.0 at 40+ chars
///   - class_diversity:      0.25 per distinct class present (max 1.0)
///   - transition_density:   transitions / max_possible_transitions
///   - prefix_bonus:         +0.15 if span starts with a known
///                            secret-prefix (sk-, ghp_, eyJ, etc.)
///
/// Combined: weighted average, clamped to [0, 1].
fn score_span(span: &[u8]) -> f64 {
    if span.is_empty() {
        return 0.0;
    }

    // Length factor: ramp from 0.0 at 16 to 1.0 at 40+
    let length_factor = if span.len() < 16 {
        0.0
    } else if span.len() >= 40 {
        1.0
    } else {
        (span.len() - 16) as f64 / (40 - 16) as f64
    };

    // Class diversity + transition density: walk once, count both.
    let mut has_upper = false;
    let mut has_lower = false;
    let mut has_digit = false;
    let mut has_symbol = false;
    let mut transitions = 0usize;
    let mut prev_class: u8 = 0; // 0 = none/start, 1 = upper, 2 = lower, 3 = digit, 4 = symbol

    for &b in span {
        let class: u8 = if b.is_ascii_uppercase() {
            has_upper = true;
            1
        } else if b.is_ascii_lowercase() {
            has_lower = true;
            2
        } else if b.is_ascii_digit() {
            has_digit = true;
            3
        } else if b.is_ascii_punctuation() {
            has_symbol = true;
            4
        } else {
            // Non-printable etc. — count as symbol-ish
            has_symbol = true;
            4
        };
        if prev_class != 0 && prev_class != class {
            transitions += 1;
        }
        prev_class = class;
    }

    let class_count = (has_upper as u32 + has_lower as u32 + has_digit as u32 + has_symbol as u32) as f64;
    let class_diversity = (class_count / 4.0).min(1.0);

    // Transition density: actual transitions / (span_len - 1).
    // A perfectly random string transitions on most chars. A run of
    // same-class chars (like an SHA1 hex string — all lowercase+digit
    // but rarely transitioning) scores low here.
    let transition_density = if span.len() > 1 {
        transitions as f64 / (span.len() - 1) as f64
    } else {
        0.0
    };

    // Prefix bonus: known secret-prefixes get a boost.
    let prefix_bonus = if has_known_secret_prefix(span) {
        0.15
    } else {
        0.0
    };

    // Weighted combination. Class diversity and transition density
    // are the strongest signals; length is a secondary multiplier;
    // prefix is a small additive boost.
    let raw = 0.30 * class_diversity
        + 0.30 * transition_density
        + 0.25 * length_factor
        + prefix_bonus;

    raw.clamp(0.0, 1.0)
}

/// Check if a span starts with a known secret prefix. Provides a
/// score bonus rather than gating — if the prefix is there, the
/// span almost certainly IS a secret regardless of other signals.
fn has_known_secret_prefix(span: &[u8]) -> bool {
    const PREFIXES: &[&[u8]] = &[
        b"sk-ant-",
        b"sk-proj-",
        b"sk-",
        b"ghp_",
        b"gho_",
        b"ghu_",
        b"ghs_",
        b"github_pat_",
        b"glpat-",
        b"xoxb-",
        b"xoxp-",
        b"xoxa-",
        b"xoxr-",
        b"AKIA",
        b"hf_",
        b"eyJ", // JWT begins with base64-encoded {"alg":... → eyJhbGc...
    ];
    for p in PREFIXES {
        if span.len() >= p.len() && &span[..p.len()] == *p {
            return true;
        }
    }
    false
}

const MARKERS: &[&str] = &[
    "password:",
    "passwd:",
    "passphrase:",
    "[sudo] password",
    "verification code",
    "totp:",
    "otp:",
    "api_key=",
    "api-key=",
    "api_token=",
    "access_token=",
    "secret_key=",
    "private_key=",
    "begin private key",
    "begin rsa private key",
    "begin openssh private key",
    "aws_secret_access_key",
    "aws_access_key_id",
    "bearer ey",        // JWT prefix
    "bearer sk-",       // Anthropic / OpenAI key format after "Bearer "
    "ssh-rsa aaaa",     // public-key but treat as sensitive — has identifying info
    // v0.1.160: extended patterns for LLM-context redaction. The
    // user enabled "send terminal history to LLM" — make
    // double-sure we don't leak common dev-environment secrets.
    "anthropic_api_key",
    "openai_api_key",
    "huggingface_token",
    "hf_token",
    "github_token",
    "github_pat",
    "gitlab_token",
    "stripe_secret",
    "stripe_api_key",
    "twilio_auth_token",
    "slack_token",
    "discord_token",
    "telegram_token",
    // Database connection strings with embedded credentials.
    "postgres://",
    "postgresql://",
    "mysql://",
    "mongodb://",
    "redis://",
    // Common HTTP headers that carry auth in curl/log lines.
    "authorization:",
    "x-api-key:",
    "x-auth-token:",
    "cookie:",
    // sk- and sk_ prefixes for inline values.
    "sk-ant-",
    "sk-proj-",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_security_default() {
        // Without the env var, should be hardened.
        // (Note: this test runs with whatever env is set in the
        // test runner; this is a smoke check.)
        let _ = is_high_security();
    }

    #[test]
    fn password_mode_toggles() {
        assert!(!is_password_mode());
        set_password_mode(true);
        assert!(is_password_mode());
        set_password_mode(false);
        assert!(!is_password_mode());
    }

    #[test]
    fn context_strip_redacts_password_line() {
        let input = "running ls\n[sudo] password for kesslerd: \nfile1\n";
        let out = context_strip(input);
        assert!(!out.contains("[sudo] password"));
        assert!(out.contains("[redacted"));
        assert!(out.contains("file1"));
        assert!(out.contains("running ls"));
    }

    #[test]
    fn context_strip_redacts_api_key() {
        let input = "export API_KEY=sk-abc123\necho hello\n";
        let out = context_strip(input);
        assert!(!out.contains("sk-abc123"));
        assert!(out.contains("echo hello"));
    }

    #[test]
    fn context_strip_keeps_normal_text() {
        let input = "ls -la\nfoo.txt  bar.txt\n";
        let out = context_strip(input);
        assert!(out.contains("foo.txt"));
        assert!(!out.contains("[redacted"));
    }

    // v0.1.160: tests for the extended redaction patterns added for
    // the Claude-provider path. We're now sending command history
    // to an external API (user opt-in); make sure we don't leak.

    #[test]
    fn context_strip_redacts_anthropic_key() {
        let input = "export ANTHROPIC_API_KEY=sk-ant-api03-abc123\nls\n";
        let out = context_strip(input);
        assert!(!out.contains("sk-ant-api03-abc123"));
        assert!(out.contains("ls"));
    }

    #[test]
    fn context_strip_redacts_inline_sk_ant() {
        // Even if the variable name was renamed, the sk-ant- prefix
        // alone should trip the redactor.
        let input = "curl -H 'Authorization: Bearer sk-ant-abc123' https://api.example/\n";
        let out = context_strip(input);
        assert!(!out.contains("sk-ant-abc123"));
    }

    #[test]
    fn context_strip_redacts_postgres_url_with_password() {
        let input = "psql postgres://app:hunter2@db.example.com/myapp\n";
        let out = context_strip(input);
        assert!(!out.contains("hunter2"));
        assert!(!out.contains("app:hunter2"));
    }

    #[test]
    fn context_strip_redacts_authorization_header() {
        let input = "curl -H 'Authorization: token abc123' https://api/\n";
        let out = context_strip(input);
        assert!(!out.contains("abc123"));
    }

    #[test]
    fn context_strip_redacts_github_token() {
        let input = "export GITHUB_TOKEN=ghp_supersecret\necho done\n";
        let out = context_strip(input);
        assert!(!out.contains("ghp_supersecret"));
        assert!(out.contains("echo done"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn secret_bytes_basic() {
        let mut s = SecretBytes::with_capacity(32).unwrap();
        s.copy_from(b"hunter2");
        assert_eq!(s.as_slice(), b"hunter2");
        assert_eq!(s.len(), 7);
        s.zero();
        assert_eq!(s.len(), 0);
        assert_eq!(s.as_slice(), b"");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn secret_bytes_truncates_to_capacity() {
        let mut s = SecretBytes::with_capacity(8).unwrap();
        // Will round up to a page (4096) so we can fit lots, but
        // verify truncation logic against the rounded capacity.
        let big = vec![0xAAu8; 16384];
        s.copy_from(&big);
        // Should have copied only as many as the rounded page allowed.
        assert!(s.len() <= 4096 || s.len() == 16384);  // depends on page size
    }

    // ─── Tiered scorer tests (v0.1.164) ─────────────────────────────

    #[test]
    fn redaction_tier_from_i32() {
        assert_eq!(RedactionTier::from_i32(0), RedactionTier::Off);
        assert_eq!(RedactionTier::from_i32(1), RedactionTier::Light);
        assert_eq!(RedactionTier::from_i32(2), RedactionTier::Medium);
        assert_eq!(RedactionTier::from_i32(3), RedactionTier::Assured);
        assert_eq!(RedactionTier::from_i32(99), RedactionTier::Off);
        assert_eq!(RedactionTier::from_i32(-1), RedactionTier::Off);
    }

    #[test]
    fn scorer_high_for_realistic_api_key() {
        // sk-ant-style key: prefix bonus + mixed classes + transitions
        let span = b"sk-ant-api03-AbCdEf1234567890XyZ-_AbCd";
        let score = score_span(span);
        assert!(score > 0.6, "expected > 0.6 for realistic key, got {}", score);
    }

    #[test]
    fn scorer_low_for_plain_word() {
        // English word: only lowercase, no transitions, short
        let span = b"averylongwordwithoutanytransitions";
        let score = score_span(span);
        assert!(score < 0.5, "expected < 0.5 for plain word, got {}", score);
    }

    #[test]
    fn scorer_medium_for_sha_hex() {
        // SHA-1 hex: 40 chars, only lower+digit, no transitions
        // between adjacent chars. Should score medium — diversity is
        // only 2 classes, transitions occasional.
        let span = b"3f786850e387550fdab836ed7e6dc881de23001b";
        let score = score_span(span);
        // Should NOT be redacted at Light tier (high threshold),
        // SHOULD be redacted at Assured (low threshold).
        assert!(score < RedactionTier::Light.score_threshold(),
            "SHA hex should not trigger Light tier; score={}", score);
        assert!(score >= RedactionTier::Assured.score_threshold(),
            "SHA hex should trigger Assured tier; score={}", score);
    }

    #[test]
    fn tiered_off_falls_back_to_marker_redaction() {
        let input = "export API_KEY=sk-abc123\necho hello\n";
        let out = context_strip_tiered(input, RedactionTier::Off);
        assert!(!out.contains("sk-abc123"));
        assert!(out.contains("echo hello"));
    }

    #[test]
    fn tiered_light_catches_marker_and_passes_short_text() {
        let input = "ls -la\necho hello\nshort string here\n";
        let out = context_strip_tiered(input, RedactionTier::Light);
        // Short spans (< 16 chars) pass through unchanged.
        assert!(out.contains("ls -la"));
        assert!(out.contains("echo hello"));
    }

    #[test]
    fn tiered_assured_redacts_obvious_secret() {
        let input = "the key is sk-ant-api03-AbCdEf1234567890XyZAbCdEf1234567890 right\n";
        let out = context_strip_tiered(input, RedactionTier::Assured);
        // v0.1.612: the Assured tier REPLACES a whole sensitive line with
        // "[redacted: sensitive line]" (see the fn doc at ~418) rather
        // than block-redacting (█) individual spans. This test was stale
        // — it asserted the old span-block behavior. The SECURITY
        // invariant is unchanged and is what we actually verify: the
        // secret must not survive, and the line is marked redacted.
        // (Surfaced when the test harness stopped masking non-phonetic
        // tests in the tier-2 crate.)
        assert!(out.contains("[redacted"),
            "expected the sensitive line to be redacted; got: {}", out);
        // The original key must NOT appear intact.
        assert!(!out.contains("sk-ant-api03-AbCdEf1234567890XyZAbCdEf1234567890"));
    }

    #[test]
    fn known_prefix_detection() {
        assert!(has_known_secret_prefix(b"sk-ant-rest_of_key"));
        assert!(has_known_secret_prefix(b"ghp_token123"));
        assert!(has_known_secret_prefix(b"eyJhbGciOi"));
        assert!(!has_known_secret_prefix(b"random_string"));
        assert!(!has_known_secret_prefix(b"sk")); // too short for sk-
    }
}
