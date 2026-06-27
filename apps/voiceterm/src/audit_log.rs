// src/audit_log.rs
//
// GITLOG-1 foundation (v0.1.252). Append-only structured log of
// every command-block, with cryptographic chain (each entry
// references the SHA-256 of the previous entry). The chain gives
// integrity verification, branching primitives for later GITLOG
// phases, and a stable substrate for the rollback UI to navigate.
//
// Storage: line-delimited records at
//   $XDG_STATE_HOME/web2qml/scrollback.log
// with fallback to ~/.config/web2qml/scrollback.log when
// XDG_STATE_HOME isn't set. Append-only writes, fsync per entry,
// no truncation. Rotation at 10MB; old logs are renamed with
// timestamp suffix, never deleted.
//
// Each entry is a single line of key=value pairs (the values are
// escaped — no literal newlines, tabs, or pipes inside values).
// Format chosen to be:
//   1. Trivially greppable from a terminal
//   2. Trivially parseable in any language
//   3. Future-compatible with a migration to real git tree storage
//      when GITLOG-2 adds filesystem snapshotting
//
// Why NOT a real git repo for v1:
//   - Spawning `git commit` per command adds 20-50ms latency.
//     At interactive rates (~100 commands/hour during active use)
//     that's seconds/hour of overhead.
//   - Adds a hard dep on the `git` binary being installed. Voice
//     shell should run on minimal systems where git might be
//     absent.
//   - The metadata-only use case (no filesystem snapshots yet)
//     doesn't actually benefit from git's tree storage. We're
//     using git purely for its commit chain, which is just a
//     hash-linked list — trivial to implement directly.
//
// GITLOG-2 (filesystem snapshotting) will likely migrate to real
// git because at that point we DO want tree storage. The line
// format here is designed to convert losslessly to git commits
// (one line → one commit message).

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::term_blocks::CommandBlock;

/// Rotation threshold. When the log exceeds this size at the start
/// of a write, the current log is renamed to scrollback-YYYYMMDD-HHMMSS.log
/// and a fresh log begins (chain restarts; we record that fact in
/// the first entry of the new log so the chain can be reconstructed
/// across rotations if needed).
const ROTATE_BYTES: u64 = 10 * 1024 * 1024;

/// Maximum length of output stored per entry. Output above this
/// threshold is truncated with a marker. The full output stays in
/// the BlockTracker's memory; the audit log is for the chain, not
/// for output archival.
const OUTPUT_LIMIT: usize = 4096;

/// Single audit entry. Built from a CommandBlock plus optional
/// speaker attribution. Serialized as one line in the log.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Unix ms timestamp when the entry was written.
    pub ts_ms: u64,
    /// CommandBlock.id — local, monotonic per session.
    pub block_id: u32,
    /// The command text (escaped for line format).
    pub command: String,
    /// Working directory at execution time, if known.
    pub cwd: Option<String>,
    /// Exit code, if the command has finished.
    pub exit_code: Option<i32>,
    /// Duration in ms.
    pub duration_ms: Option<u32>,
    /// Output (possibly truncated to OUTPUT_LIMIT).
    pub output: String,
    /// `true` if output was truncated for storage.
    pub output_truncated: bool,
    /// Speaker identifier (stable, from speakers.rs) if this
    /// command was voice-attested. None for typed commands.
    pub speaker_id: Option<String>,
    /// Speaker display name at time of attestation.
    pub speaker_name: Option<String>,
    /// Embedding similarity at time of speaker identification
    /// (range 0..1, where 0.65+ is the speakers.rs accept
    /// threshold). None for typed commands or when not available.
    pub speaker_confidence: Option<f32>,
    /// How the speaker was attested: "voice" | "typed" | "tap_confirm".
    pub attestation: String,
    /// v0.1.254 (GITLOG-2): SHA-256 of the snapshot tree taken
    /// before this command ran. Resolves through snapshot_store
    /// to the file listing at that moment. None when snapshot
    /// wasn't available.
    pub tree_hash: Option<String>,
    /// v0.1.275 (GITLOG-5 persistence): id of the parent block
    /// in the branch tree. Normally the previous block; after a
    /// rollback, the rollback target's parent (so the new block
    /// is a SIBLING of the rolled-back one, not its successor).
    /// None for the first block of a session.
    pub parent_block_id: Option<u32>,
    /// SHA-256 hex of the previous entry's full serialized form.
    /// Empty string for the chain's first entry (or after rotation).
    pub prev_hash: String,
    /// v0.1.379 (session arc, step 1): session id (unix
    /// seconds at session start). Groups command entries that
    /// belong to the same period of work. New session triggered
    /// by:
    ///   - 15 minutes of audit-log inactivity
    ///   - Speaker change (different person at the keyboard)
    ///   - Process restart
    /// Pre-existing log entries (written before v0.1.379) have
    /// no `session=` field; readers should treat that as
    /// "pre-session-tracking" pseudo-session 0.
    pub session_id: u64,
}

impl Entry {
    /// Build an Entry from a finished CommandBlock plus optional
    /// speaker attribution. Caller supplies the prev_hash.
    pub fn from_block(
        block: &CommandBlock,
        speaker_id: Option<String>,
        speaker_name: Option<String>,
        speaker_confidence: Option<f32>,
        attestation: &str,
        prev_hash: String,
        session_id: u64,
    ) -> Self {
        let (output, output_truncated) = if block.output.len() > OUTPUT_LIMIT {
            let mut cut = OUTPUT_LIMIT;
            while cut > 0 && !block.output.is_char_boundary(cut) { cut -= 1; }
            (block.output[..cut].to_string(), true)
        } else {
            (block.output.clone(), false)
        };
        Self {
            ts_ms: now_ms(),
            block_id: block.id,
            command: block.command.clone(),
            cwd: block.cwd.clone(),
            exit_code: block.exit_code,
            duration_ms: block.duration_ms,
            output,
            output_truncated,
            speaker_id,
            speaker_name,
            speaker_confidence,
            attestation: attestation.to_string(),
            tree_hash: block.tree_hash.clone(),
            parent_block_id: block.parent_block_id,
            prev_hash,
            session_id,
        }
    }

    /// Serialize to the line format. Trailing newline included.
    /// Fields are key=value pairs separated by '\t'. Values are
    /// escaped so they contain no \t, \n, or \r.
    pub fn serialize(&self) -> String {
        let mut s = String::with_capacity(512 + self.output.len());
        s.push_str(&format!("ts={}", self.ts_ms));
        s.push('\t');
        s.push_str(&format!("block={}", self.block_id));
        s.push('\t');
        s.push_str(&format!("cmd={}", escape(&self.command)));
        if let Some(cwd) = &self.cwd {
            s.push('\t');
            s.push_str(&format!("cwd={}", escape(cwd)));
        }
        if let Some(code) = self.exit_code {
            s.push('\t');
            s.push_str(&format!("exit={}", code));
        }
        if let Some(d) = self.duration_ms {
            s.push('\t');
            s.push_str(&format!("dur_ms={}", d));
        }
        if let Some(id) = &self.speaker_id {
            s.push('\t');
            s.push_str(&format!("spk_id={}", escape(id)));
        }
        if let Some(name) = &self.speaker_name {
            s.push('\t');
            s.push_str(&format!("spk={}", escape(name)));
        }
        if let Some(conf) = self.speaker_confidence {
            s.push('\t');
            s.push_str(&format!("spk_conf={:.3}", conf));
        }
        s.push('\t');
        s.push_str(&format!("attest={}", escape(&self.attestation)));
        if let Some(tree) = &self.tree_hash {
            s.push('\t');
            s.push_str(&format!("tree={}", tree));
        }
        // v0.1.275: parent_block_id for branch reconstruction.
        // Always emit if set so branches survive restarts. The
        // viewer ignores missing values gracefully (treats as
        // root).
        if let Some(parent) = self.parent_block_id {
            s.push('\t');
            s.push_str(&format!("parent={}", parent));
        }
        if self.output_truncated {
            s.push('\t');
            s.push_str("trunc=1");
        }
        if !self.output.is_empty() {
            s.push('\t');
            s.push_str(&format!("out={}", escape(&self.output)));
        }
        // v0.1.379 (session arc, step 1): session id for
        // grouping entries by work period. Always emit so
        // session queries don't have to handle "this entry
        // was written by an old binary" — going forward,
        // every entry has it.
        s.push('\t');
        s.push_str(&format!("session={}", self.session_id));
        s.push('\t');
        s.push_str(&format!("prev={}", self.prev_hash));
        s.push('\n');
        s
    }
}

/// Escape a value for the line format. Replaces tab/newline/CR with
/// printable sequences and backslash with double-backslash. The
/// inverse is unescape(); both are pure.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some(other) => { out.push('\\'); out.push(other); }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn now_ms() -> u64 { core_kit::time::now_millis() }

/// Compute SHA-256 of a string, return lowercase hex.
///
/// We don't pull in a crypto crate for this; we implement SHA-256
/// directly. ~80 lines, no external dependency. The audit log is
/// foundational enough that an extra dep is more cost than the
/// implementation. Standard FIPS 180-4 algorithm.
fn sha256_hex(input: &str) -> String {
    sha256_hex_bytes(input.as_bytes())
}

/// v0.1.254: bytes variant. Same algorithm, accepts arbitrary
/// byte input (used by snapshot_store for file contents).
pub(crate) fn sha256_hex_bytes(input: &[u8]) -> String {
    // Initial hash values (first 32 bits of the fractional parts
    // of the square roots of the first 8 primes).
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    // Round constants (first 32 bits of the fractional parts of
    // the cube roots of the first 64 primes).
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    // Pre-processing: append the bit '1' to the message, then '0' bits
    // until length is congruent to 448 mod 512, then 64-bit length.
    let bytes = input;
    let bit_len = (bytes.len() as u64) * 8;
    let mut msg: Vec<u8> = Vec::with_capacity(bytes.len() + 72);
    msg.extend_from_slice(bytes);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    // Process each 512-bit chunk.
    for chunk in msg.chunks(64) {
        let mut w: [u32; 64] = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i*4], chunk[i*4+1], chunk[i*4+2], chunk[i*4+3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e;
            e = d.wrapping_add(temp1);
            d = c; c = b; b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
    }
    let mut out = String::with_capacity(64);
    for word in &h {
        out.push_str(&format!("{:08x}", word));
    }
    out
}

/// Resolve the audit log path. Prefers XDG_STATE_HOME with
/// web2qml subdir, falls back to ~/.config/web2qml/. Creates the
/// parent directory if needed. Returns None when neither HOME
/// nor XDG_STATE_HOME is set (build/CI environments without
/// $HOME); callers treat None as a no-op.
fn log_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                let mut p = PathBuf::from(h);
                p.push(".local"); p.push("state");
                p
            })
        })
        .or_else(|| {
            // Last-resort fallback: ~/.config/web2qml.
            // Matches where other state already lives if
            // XDG_STATE_HOME isn't conventional on this system.
            std::env::var_os("HOME").map(|h| {
                let mut p = PathBuf::from(h);
                p.push(".config");
                p
            })
        })?;
    let mut p = base;
    p.push("web2qml");
    let _ = std::fs::create_dir_all(&p);
    p.push("scrollback.log");
    Some(p)
}

/// Last-known prev_hash for chaining. Mutex-protected so writes
/// from any thread serialize. Lazy-initialized on first record()
/// call: we read the last line of the existing log and hash it,
/// so the chain continues across process restarts.
static CHAIN_TAIL: Mutex<Option<String>> = Mutex::new(None);

/// v0.1.379 (session arc, step 1): session segmentation state.
/// A "session" is a bounded period of work — commands that
/// belong together because they happened in close temporal
/// succession with the same speaker. The build needs this
/// concept to answer "what did I do last session" and later
/// to detect command combos worth turning into workflows.
///
/// Segmentation signals (any one triggers a new session):
///   1. SESSION_IDLE_MS without writing an audit entry. 15
///      minutes is the default — long enough to span a lunch
///      break, short enough that a forgotten terminal across
///      days doesn't merge weeks of work.
///   2. Speaker change (different person at the keyboard).
///   3. Process restart — the in-memory state is reset on
///      module load, so the very first entry after start is
///      always a new session.
///
/// Session IDs are unix seconds at session-start. Human-
/// readable in raw log lines; collision-free given the idle
/// gap requirement.
const SESSION_IDLE_MS: u64 = 15 * 60 * 1000;

/// State protected by SESSION_STATE: (session_id_secs,
/// last_activity_ms, last_speaker_id). All set when a new
/// session begins.
struct SessionState {
    id: u64,
    last_activity_ms: u64,
    last_speaker_id: Option<String>,
}

static SESSION_STATE: Mutex<Option<SessionState>> = Mutex::new(None);

/// Compute the session id for an entry about to be written.
/// Mutates the state: if a new session begins, the in-memory
/// state advances. Returns the session id to stamp on the
/// entry.
///
/// Called from write_entry under CHAIN_TAIL lock; tests can
/// also call this directly with the now_ms parameter.
fn session_id_for(now_ms: u64, speaker_id: Option<&str>) -> u64 {
    let mut guard = SESSION_STATE.lock().expect("SESSION_STATE poisoned");
    let needs_new = match guard.as_ref() {
        None => true,
        Some(prev) => {
            let gap = now_ms.saturating_sub(prev.last_activity_ms);
            let speaker_changed = match (&prev.last_speaker_id, speaker_id) {
                // BOTH set with different ids → speaker change
                (Some(a), Some(b)) => a != b,
                // Previously typed, now voice (or vice versa) is
                // NOT itself a session break — the same human
                // can switch input modality without changing
                // identity.
                _ => false,
            };
            gap > SESSION_IDLE_MS || speaker_changed
        }
    };
    if needs_new {
        let new_id = now_ms / 1000;
        *guard = Some(SessionState {
            id: new_id,
            last_activity_ms: now_ms,
            last_speaker_id: speaker_id.map(|s| s.to_string()),
        });
        new_id
    } else {
        let st = guard.as_mut().expect("guard is Some, just checked above");
        st.last_activity_ms = now_ms;
        // Update last_speaker_id only when an attested speaker
        // is present — preserves identity continuity across
        // typed commands that intersperse voice commands.
        if speaker_id.is_some() {
            st.last_speaker_id = speaker_id.map(|s| s.to_string());
        }
        st.id
    }
}

/// Returns the current session id (the id of the most recent
/// session), or None if no entries have been written yet this
/// process lifetime. Used by the `web2qml://sessions` page and
/// voice intents that query session state.
///
/// Future ships: a persistent-recent-sessions lookup that
/// scans the audit log rather than relying on in-memory state
/// (so "what did I do last session" survives a restart).
pub fn current_session_id() -> Option<u64> {
    SESSION_STATE.lock().ok().and_then(|g| g.as_ref().map(|s| s.id))
}


/// v0.1.253 (GITLOG-1b): pending voice attribution for the NEXT
/// command-block. The voice intent path calls
/// `set_pending_attribution()` after the voice identifies a
/// speaker; the term_blocks `submit_command` consumes it via
/// `take_pending_attribution()` when constructing the new block.
///
/// Single-shot semantics: `take_*` clears the state. If a typed
/// command arrives without voice having fired first, take_*
/// returns None and the block gets `attestation=typed`.
///
/// Stale-timeout: pending attributions older than
/// PENDING_ATTRIBUTION_STALE_MS are silently discarded on take.
/// This handles cases where voice identifies a speaker but the
/// resulting intent is non-terminal (navigation, settings popup,
/// etc.), so no submit_command fires — without timeout, the
/// stale attribution would falsely attribute the NEXT typed
/// command to the previous voice speaker.
///
/// Why module-static rather than threaded through call sites:
/// the voice path produces the attribution in one part of the
/// codebase (voice / phonetic / main.rs voice handler), and the
/// block creation happens in a different module (term_blocks)
/// triggered by PTY echo. Threading the attribution through QML
/// → pty_write → PTY → echo → BlockTracker would require either
/// a new backend method or out-of-band metadata. Module-static
/// is the smaller incision; single-shot semantics keep it safe.
#[derive(Debug, Clone)]
pub struct PendingAttribution {
    pub speaker_id: String,
    pub speaker_name: String,
    pub confidence: f32,
    pub attestation: String,
    /// Unix ms when this attribution was set. Used to detect
    /// staleness: if a non-term-cmd intent intervenes, the
    /// attribution shouldn't apply to a much-later typed command.
    pub set_at_ms: u64,
}

/// Discard pending attributions older than this. A voice utterance
/// followed by a 30-second pause is well past the "user fired a
/// command" envelope; safer to drop and require fresh voice.
const PENDING_ATTRIBUTION_STALE_MS: u64 = 30_000;

static PENDING_ATTRIBUTION: Mutex<Option<PendingAttribution>> = Mutex::new(None);

/// Record a speaker as the attribution for the next command-block
/// submitted. Called from the voice intent path right after
/// `speakers::identify()` succeeds and the intent is dispatched.
///
/// If a previous pending attribution wasn't consumed (e.g. voice
/// fired twice in quick succession without a command landing
/// between them), it's overwritten. The most recent voice
/// attribution wins. This is intentional: if the user said
/// something, started a command, then voice-corrected themselves,
/// the corrected attribution is more honest.
pub fn set_pending_attribution(
    speaker_id: String,
    speaker_name: String,
    confidence: f32,
    attestation: &str,
) {
    if let Ok(mut g) = PENDING_ATTRIBUTION.lock() {
        *g = Some(PendingAttribution {
            speaker_id,
            speaker_name,
            confidence,
            attestation: attestation.to_string(),
            set_at_ms: now_ms(),
        });
    }
}

/// Consume pending attribution. Returns Some if a voice
/// attribution was pending AND fresh enough, None otherwise.
/// Always clears the state — single-shot semantics, so a typed
/// command following a voice command doesn't get mis-attributed.
/// Discards stale attributions (older than
/// PENDING_ATTRIBUTION_STALE_MS) silently.
pub fn take_pending_attribution() -> Option<PendingAttribution> {
    let pending = PENDING_ATTRIBUTION.lock().ok().and_then(|mut g| g.take())?;
    let now = now_ms();
    if now.saturating_sub(pending.set_at_ms) > PENDING_ATTRIBUTION_STALE_MS {
        // Discard stale; caller gets None as if nothing was set.
        crate::clog!("audit", "discarding stale pending attribution for {:?} ({}ms old)",
            pending.speaker_name, now.saturating_sub(pending.set_at_ms));
        return None;
    }
    Some(pending)
}

/// Read the last entry's hash from the existing log file. Returns
/// empty string if the file doesn't exist or is empty. Called once
/// at first record() to bootstrap the chain.
fn read_chain_tail(path: &PathBuf) -> String {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return String::new(),
    };
    let reader = BufReader::new(file);
    // Walk to last non-empty line, then hash it. For typical log
    // sizes this is O(n); for very large logs we'd want to seek
    // from the end, but we rotate at 10MB so this stays cheap.
    let mut last: Option<String> = None;
    for line in reader.lines().flatten() {
        if !line.is_empty() { last = Some(line); }
    }
    last.map(|l| sha256_hex(&l)).unwrap_or_default()
}

/// Rotate the log file if it exceeds ROTATE_BYTES. The rotated
/// file gets a timestamp suffix; no compression in v1 (deferred
/// to GITLOG-7 hardening). Chain restarts after rotation — the
/// first entry in the new log has prev_hash empty, with a note
/// in the entry's command field if rotated.
fn rotate_if_needed(path: &PathBuf) -> std::io::Result<bool> {
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return Ok(false), // doesn't exist yet, no rotation
    };
    if metadata.len() < ROTATE_BYTES { return Ok(false); }
    let stamp = chrono_stamp_or_epoch();
    let mut rotated = path.clone();
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("scrollback.log");
    // Strip ".log" suffix if present, append stamp + ".log".
    let stem = file_name.strip_suffix(".log").unwrap_or(file_name);
    rotated.set_file_name(format!("{}-{}.log", stem, stamp));
    std::fs::rename(path, &rotated)?;
    crate::clog!("audit", "rotated log to {:?}", rotated.file_name().unwrap_or_default());
    Ok(true)
}

/// Format a YYYYMMDD-HHMMSS timestamp without pulling in chrono.
/// We use unix epoch seconds in a simple manual format. Good
/// enough for filenames; precise calendar formatting can land
/// later if needed.
fn chrono_stamp_or_epoch() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0);
    // Plain epoch seconds is unambiguous, sortable, and tiny.
    // If we want calendar later, easy migration.
    format!("epoch{}", secs)
}

/// Write a single Entry to the audit log. Returns the hash of the
/// written entry (the new chain tail) on success. On any I/O
/// failure, logs the error to stderr and returns None — the audit
/// log is best-effort and must not interfere with foreground
/// command execution.
///
/// Thread-safe via the CHAIN_TAIL mutex. The mutex covers BOTH the
/// hash chain update AND the file write to prevent two concurrent
/// writes from getting the same prev_hash.
pub fn write_entry(
    block: &CommandBlock,
    speaker_id: Option<String>,
    speaker_name: Option<String>,
    speaker_confidence: Option<f32>,
    attestation: &str,
) -> Option<String> {
    let path = log_path()?;
    let mut tail_guard = CHAIN_TAIL.lock().ok()?;
    // Lazy init: read existing log's last entry once.
    if tail_guard.is_none() {
        *tail_guard = Some(read_chain_tail(&path));
    }
    let prev = tail_guard.clone().unwrap_or_default();

    // Rotate first if needed; if rotated, reset chain.
    let rotated = match rotate_if_needed(&path) {
        Ok(r) => r,
        Err(e) => { crate::clog!("audit", "rotate failed: {}", e); false }
    };
    let prev_to_use = if rotated { String::new() } else { prev };

    // v0.1.379: compute session id BEFORE building the entry.
    // session_id_for mutates SESSION_STATE — if 15+ minutes
    // since last activity OR speaker changed, this begins a
    // new session and the returned id is the new one. We
    // pass the speaker_id (not name; speakers can be renamed,
    // but speaker_id is stable).
    let session_id = session_id_for(now_ms(), speaker_id.as_deref());

    let entry = Entry::from_block(
        block, speaker_id, speaker_name, speaker_confidence,
        attestation, prev_to_use, session_id,
    );
    let line = entry.serialize();
    let new_hash = sha256_hex(line.trim_end_matches('\n'));

    let mut file = match OpenOptions::new()
        .create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => { crate::clog!("audit", "open failed: {}", e); return None; }
    };
    if let Err(e) = file.write_all(line.as_bytes()) {
        crate::clog!("audit", "write failed: {}", e);
        return None;
    }
    // fsync so a crash right after a sensitive command (sudo,
    // rm, etc.) still leaves an audit trail.
    if let Err(e) = file.sync_data() {
        crate::clog!("audit", "fsync failed: {}", e);
        // Don't fail the call — the write itself succeeded.
    }
    *tail_guard = Some(new_hash.clone());
    Some(new_hash)
}

/// v0.1.270: render recent audit-log entries as markdown for
/// in-app display. Page surface at `web2qml://audit`.
///
/// Format: most-recent-first, one section per command, with:
///   - Header: timestamp + speaker (if attested) + attestation method
///   - The command itself in a code block
///   - cwd, exit code, duration as a compact info line
///   - First line of output (if truncated, noted)
///   - Snapshot hash (if a rollback point exists)
///
/// Limits to MAX_VIEWER_ENTRIES entries to keep the page
/// renderable on phone-class devices. The log file may have
/// thousands of entries after long use; rendering all of them
/// would balloon the QML output. The user can `cat
/// $XDG_STATE_HOME/web2qml/scrollback.log` for the full text.
pub fn render_markdown() -> String {
    render_markdown_filtered(None)
}

/// v0.1.277: render with optional speaker name filter. When
/// `speaker_filter` is Some, only entries whose `speaker_name`
/// matches (case-insensitive) are shown. Used by
/// `web2qml://audit?speaker=Allison` query routing.
pub fn render_markdown_filtered(speaker_filter: Option<&str>) -> String {
    const MAX_VIEWER_ENTRIES: usize = 200;
    let all_entries = read_recent_entries(MAX_VIEWER_ENTRIES);

    // Apply filter. We filter AFTER read_recent_entries to keep
    // the read-cap interpretable ("most recent 200 entries"),
    // not "most recent 200 entries matching X." If a filter
    // matches very few entries, fewer show up. Documented in
    // the page header.
    let filter_lower = speaker_filter.map(|s| s.to_lowercase());
    let entries: Vec<&ParsedEntry> = all_entries.iter().filter(|e| {
        match (&filter_lower, &e.speaker_name) {
            (Some(want), Some(have)) => have.to_lowercase().contains(want.as_str()),
            (Some(_), None) => false,  // filter is set, entry has no speaker
            (None, _) => true,  // no filter
        }
    }).collect();

    let mut md = String::with_capacity(entries.len() * 200 + 512);
    md.push_str("# Audit Log");
    if let Some(want) = speaker_filter {
        md.push_str(&format!(" — filtered to **{}**", want));
    }
    md.push_str("\n\n");

    if entries.is_empty() {
        if speaker_filter.is_some() {
            md.push_str("No entries matched the speaker filter in the most recent 200 entries.\n\n");
            md.push_str("- Try a partial match (e.g. `?speaker=Al` for Allison)\n");
            md.push_str("- Or visit `web2qml://audit` (no filter) to see what speakers have been recorded\n\n");
        } else {
            md.push_str("No commands recorded yet. The log captures every command run in the terminal tab once you start using it.\n\n");
            md.push_str("Storage: `$XDG_STATE_HOME/web2qml/scrollback.log` (typically `~/.local/state/web2qml/scrollback.log`)\n\n");
        }
        return md;
    }

    md.push_str(&format!(
        "Showing {} entries{}. The log preserves the full history on disk; this view is for in-app browsing.\n\n",
        entries.len(),
        if speaker_filter.is_some() {
            format!(" matching the speaker filter (out of {} most recent)", all_entries.len())
        } else {
            String::new()
        }
    ));

    // Most-recent-first for browsing.
    for entry in entries.iter().rev() {
        md.push_str("## ");
        // Compact timestamp: YYYY-MM-DD HH:MM:SS.
        let ts_str = format_ts(entry.ts_ms);
        md.push_str(&ts_str);
        if let Some(name) = &entry.speaker_name {
            md.push_str(" — ");
            md.push_str(name);
            if let Some(conf) = entry.speaker_confidence {
                md.push_str(&format!(" ({:.0}%)", conf * 100.0));
            }
        }
        md.push_str("\n\n");

        // The command itself in a fenced code block.
        md.push_str("```\n");
        md.push_str(&entry.command);
        md.push_str("\n```\n");

        // Info line: cwd, exit, duration, attestation.
        let mut info_parts: Vec<String> = Vec::new();
        info_parts.push(format!("via **{}**", entry.attestation));
        if let Some(cwd) = &entry.cwd {
            info_parts.push(format!("in `{}`", short_cwd(cwd)));
        }
        if let Some(code) = entry.exit_code {
            if code == 0 {
                info_parts.push("exit 0 ✓".to_string());
            } else {
                info_parts.push(format!("exit {} ✗", code));
            }
        }
        if let Some(d) = entry.duration_ms {
            info_parts.push(format!("{}", format_duration_ms(d)));
        }
        if entry.tree_hash.is_some() {
            info_parts.push("↩ snapshot".to_string());
        }
        md.push_str(&info_parts.join(" · "));
        md.push_str("\n\n");

        // Optional output preview.
        if let Some(first) = &entry.output_first_line {
            md.push_str("> ");
            md.push_str(first);
            if entry.output_truncated {
                md.push_str(" …");
            }
            md.push_str("\n\n");
        }
    }

    md.push_str("\n---\n\n");
    md.push_str("Full log on disk: `$XDG_STATE_HOME/web2qml/scrollback.log` (hash-chained, append-only).\n");
    if speaker_filter.is_none() {
        // Tip: only show on the no-filter view.
        md.push_str("\n**Filter by speaker:** Navigate to `web2qml://audit?speaker=NAME` (partial match, case-insensitive).\n");
    } else {
        md.push_str("\n**Clear filter:** [web2qml://audit](web2qml://audit)\n");
    }
    md
}

fn format_ts(ms: u64) -> String {
    // Minimal timestamp formatter. Avoid pulling in chrono for
    // this — handful of arithmetic ops are all we need.
    let secs = ms / 1000;
    let (year, month, day, h, m, s) = ts_breakdown(secs);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", year, month, day, h, m, s)
}

/// Break down unix seconds into (year, month, day, h, m, s).
/// Uses the proleptic Gregorian calendar; correct for any date
/// after 1970-01-01. Algorithm: count up days from epoch,
/// account for leap years.
fn ts_breakdown(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let mut days = (secs / 86400) as u32;

    let mut year = 1970u32;
    loop {
        let leap = is_leap_year(year);
        let year_days = if leap { 366 } else { 365 };
        if days < year_days { break; }
        days -= year_days;
        year += 1;
    }

    let mut month = 1u32;
    loop {
        let dim = days_in_month(year, month);
        if days < dim { break; }
        days -= dim;
        month += 1;
    }

    (year, month, days + 1, h as u32, m as u32, s as u32)
}

fn is_leap_year(y: u32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap_year(y) { 29 } else { 28 },
        _ => 30,  // shouldn't happen
    }
}

fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 { return format!("{}ms", ms); }
    let secs = ms / 1000;
    if secs < 60 { return format!("{}s", secs); }
    let mins = secs / 60;
    let rem = secs % 60;
    if mins < 60 { return format!("{}m{:02}s", mins, rem); }
    let hours = mins / 60;
    let rem_m = mins % 60;
    format!("{}h{:02}m", hours, rem_m)
}

/// Shorten cwd by replacing $HOME with ~. Works without env
/// access — we look for known home prefixes. If HOME is set,
/// substitute it. Otherwise pass through unchanged.
fn short_cwd(cwd: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        if cwd.starts_with(&home) {
            let rest = &cwd[home.len()..];
            if rest.is_empty() {
                return "~".to_string();
            } else if rest.starts_with('/') {
                return format!("~{}", rest);
            }
        }
    }
    cwd.to_string()
}

/// v0.1.276: result of chain verification.
#[derive(Debug, Clone)]
pub struct VerifyResult {
    /// Number of lines successfully verified.
    pub verified: usize,
    /// Number of lines in the log total.
    pub total: usize,
    /// First line index where verification failed (1-based),
    /// or None if the entire chain checks out. None also when
    /// the log is empty — see `total`.
    pub first_failure: Option<usize>,
    /// Human-readable description of the failure point or
    /// success. For UI display.
    pub message: String,
}

/// v0.1.276: walk the entire audit log file, verifying that
/// each line's `prev=` field equals SHA-256(previous line).
/// Returns a VerifyResult summarizing what was checked.
///
/// Verification scope: only the linear chain in the current
/// log file. After log rotation, each rotated file has its
/// own chain (the new log starts fresh with prev_hash empty).
/// A full cross-rotation audit would need to walk archived
/// files — filed for GITLOG-7 hardening.
///
/// Cost: O(n) hash computations. The log rotates at 10MB,
/// so even a fully-saturated log is sub-second.
pub fn verify_chain() -> VerifyResult {
    let path = match log_path() {
        Some(p) => p,
        None => return VerifyResult {
            verified: 0,
            total: 0,
            first_failure: None,
            message: "Cannot determine log path (no XDG_STATE_HOME or HOME)".to_string(),
        },
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return VerifyResult {
            verified: 0,
            total: 0,
            first_failure: None,
            message: "Audit log not found — nothing to verify yet".to_string(),
        },
    };
    let lines: Vec<&str> = content.lines()
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return VerifyResult {
            verified: 0,
            total: 0,
            first_failure: None,
            message: "Audit log is empty".to_string(),
        };
    }

    // Walk lines, comparing each line's `prev=` to SHA-256
    // of the previous line. The first line should have
    // prev=<empty> (chain start).
    let mut expected_prev = String::new();
    for (idx, line) in lines.iter().enumerate() {
        let prev_claim = extract_prev_field(line);
        if prev_claim != expected_prev {
            return VerifyResult {
                verified: idx,
                total: lines.len(),
                first_failure: Some(idx + 1),  // 1-based for UI
                message: format!(
                    "Hash chain broken at line {}: expected prev={}, got prev={}",
                    idx + 1,
                    if expected_prev.is_empty() { "<empty>" } else { &expected_prev[..8.min(expected_prev.len())] },
                    if prev_claim.is_empty() { "<empty>" } else { &prev_claim[..8.min(prev_claim.len())] },
                ),
            };
        }
        // For the NEXT iteration, compute this line's hash.
        // Note: serialize() trims trailing newline; we already
        // stripped via .lines() above.
        expected_prev = sha256_hex(line);
    }

    VerifyResult {
        verified: lines.len(),
        total: lines.len(),
        first_failure: None,
        message: format!("✓ All {} entries verified — hash chain intact", lines.len()),
    }
}

/// Extract the `prev=` field value from a serialized log line.
/// Returns empty string if the field is missing or empty (which
/// is normal for the chain's first entry).
fn extract_prev_field(line: &str) -> String {
    for field in line.split('\t') {
        if let Some(("prev", v)) = field.split_once('=') {
            return v.to_string();
        }
    }
    String::new()
}


/// v0.1.275 (GITLOG-5 persistence): render the branch tree
/// from PERSISTED parent pointers in the audit log. Used when
/// the in-memory BlockTracker is empty (e.g. after a session
/// restart) so the user can still see the branch structure
/// from prior sessions. Returns markdown.
///
/// Falls back to a helpful message when no entries have
/// `parent=` fields (older log entries before v0.1.275).
pub fn render_branches_from_log(limit: usize) -> String {
    use std::collections::HashMap;
    let entries = read_recent_entries(limit);

    let mut md = String::with_capacity(entries.len() * 160 + 512);
    md.push_str("# Command Branches (from disk)\n\n");

    if entries.is_empty() {
        md.push_str("No commands recorded yet. Once you start running commands, this view shows the branching structure created by rollbacks.\n\n");
        return md;
    }

    // Filter to entries with block_id (older entries lack it).
    let entries_with_id: Vec<&ParsedEntry> = entries.iter()
        .filter(|e| e.block_id.is_some())
        .collect();
    if entries_with_id.is_empty() {
        md.push_str("Branch reconstruction needs entries from v0.1.275 or later. Older log entries don't carry block-id and parent-pointer fields. New commands will appear with branch structure as they're recorded.\n\n");
        md.push_str("See `web2qml://audit` for the linear command log in the meantime.\n");
        return md;
    }

    md.push_str(&format!(
        "Reconstructed from the audit log ({} entries with branch metadata).\n\n",
        entries_with_id.len()
    ));

    // Build children index: parent_id → Vec<&ParsedEntry>.
    let mut children: HashMap<u32, Vec<&ParsedEntry>> = HashMap::new();
    let mut roots: Vec<&ParsedEntry> = Vec::new();
    let known_ids: std::collections::HashSet<u32> = entries_with_id.iter()
        .filter_map(|e| e.block_id)
        .collect();
    for e in &entries_with_id {
        match e.parent_block_id {
            Some(p) if known_ids.contains(&p) => {
                children.entry(p).or_default().push(e);
            }
            _ => roots.push(e),
        }
    }

    for root in &roots {
        render_branch_entry(&mut md, root, &children, 0);
    }
    md.push_str("\n---\n\n");
    md.push_str("**Markers:**\n\n");
    md.push_str("- ↩ — block has a rollback snapshot available\n");
    md.push_str("- ✓ / ✗ — last exit code\n");
    md
}

fn render_branch_entry(
    md: &mut String,
    entry: &ParsedEntry,
    children: &std::collections::HashMap<u32, Vec<&ParsedEntry>>,
    depth: usize,
) {
    for _ in 0..depth { md.push_str("  "); }
    md.push_str("- ");

    let cmd: String = if entry.command.chars().count() > 80 {
        let mut s: String = entry.command.chars().take(80).collect();
        s.push('…');
        s
    } else {
        entry.command.clone()
    };
    md.push('`');
    md.push_str(&cmd);
    md.push('`');

    if let Some(code) = entry.exit_code {
        if code == 0 {
            md.push_str(" ✓");
        } else {
            md.push_str(&format!(" ✗ exit {}", code));
        }
    }
    if entry.tree_hash.is_some() {
        md.push_str(" ↩");
    }
    if let Some(name) = &entry.speaker_name {
        md.push_str(&format!(" — _{}_", name));
    }
    md.push('\n');

    if let Some(id) = entry.block_id {
        if let Some(kids) = children.get(&id) {
            for kid in kids {
                render_branch_entry(md, kid, children, depth + 1);
            }
        }
    }
}


/// Subset of the full Entry struct — we only surface the
/// fields the in-app viewer renders. Reading the log file
/// from disk and reconstructing this is the inverse of
/// `Entry::serialize` for those fields.
#[derive(Debug, Clone)]
pub struct ParsedEntry {
    pub ts_ms: u64,
    /// v0.1.275: block id for branch reconstruction. None if
    /// the log line lacks it (older entries before this field
    /// existed).
    pub block_id: Option<u32>,
    pub command: String,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub speaker_name: Option<String>,
    pub speaker_confidence: Option<f32>,
    pub attestation: String,
    pub tree_hash: Option<String>,
    /// v0.1.275: parent block id for branch reconstruction.
    /// None for root blocks (or older log entries).
    pub parent_block_id: Option<u32>,
    pub output_truncated: bool,
    pub output_first_line: Option<String>,
    /// v0.1.379 (session arc): session id stamped at write
    /// time. None for entries written by older builds that
    /// didn't have this field — treat None as
    /// "pre-session-tracking pseudo-session 0" when grouping.
    pub session_id: Option<u64>,
}

/// v0.1.270: read up to `limit` most-recent entries from the
/// audit log file. Returns oldest-first within the returned
/// window (so callers can render top-down chronologically).
///
/// This is intended for the in-app audit viewer, not for
/// programmatic auditing — it tolerates parse failures
/// (skips malformed lines) and silently caps at the log file's
/// available length. For real auditing use a separate path
/// that validates the hash chain.
pub fn read_recent_entries(limit: usize) -> Vec<ParsedEntry> {
    if limit == 0 { return Vec::new(); }
    let path = match log_path() { Some(p) => p, None => return Vec::new() };
    let content = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    // Walk lines from the end. Standard `lines()` doesn't give
    // us a reverse iterator over &str; collect then slice.
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(limit);
    let mut out = Vec::with_capacity(limit.min(lines.len() - start));
    for line in &lines[start..] {
        if let Some(entry) = parse_log_line(line) {
            out.push(entry);
        }
    }
    out
}

/// Best-effort parse of one serialized log line back into a
/// ParsedEntry. Returns None on missing required fields
/// (ts, cmd) or malformed structure. Tolerant of unknown
/// fields (they're ignored). Inverse of Entry::serialize.
fn parse_log_line(line: &str) -> Option<ParsedEntry> {
    let mut ts_ms: Option<u64> = None;
    let mut block_id: Option<u32> = None;
    let mut command: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut exit_code: Option<i32> = None;
    let mut duration_ms: Option<u64> = None;
    let mut speaker_name: Option<String> = None;
    let mut speaker_confidence: Option<f32> = None;
    let mut attestation = String::from("typed");
    let mut tree_hash: Option<String> = None;
    let mut parent_block_id: Option<u32> = None;
    let mut output_truncated = false;
    let mut output_first_line: Option<String> = None;
    let mut session_id: Option<u64> = None;

    for field in line.split('\t') {
        let (k, v) = match field.split_once('=') {
            Some(pair) => pair,
            None => continue,
        };
        match k {
            "ts" => ts_ms = v.parse().ok(),
            "block" => block_id = v.parse().ok(),
            "cmd" => command = Some(unescape_simple(v)),
            "cwd" => cwd = Some(unescape_simple(v)),
            "exit" => exit_code = v.parse().ok(),
            "dur_ms" => duration_ms = v.parse().ok(),
            "spk" => speaker_name = Some(unescape_simple(v)),
            "spk_conf" => speaker_confidence = v.parse().ok(),
            "attest" => attestation = unescape_simple(v),
            "tree" => tree_hash = Some(v.to_string()),
            "parent" => parent_block_id = v.parse().ok(),
            "out_trunc" | "trunc" => output_truncated = v == "1" || v == "true",
            "session" => session_id = v.parse().ok(),
            "out" => {
                let full = unescape_simple(v);
                // Extract just the first line for compact display.
                let first = full.lines().next().unwrap_or("").to_string();
                if !first.is_empty() {
                    output_first_line = Some(first);
                }
            }
            _ => {}  // tolerate unknown fields
        }
    }
    Some(ParsedEntry {
        ts_ms: ts_ms?,
        block_id,
        command: command?,
        cwd,
        exit_code,
        duration_ms,
        speaker_name,
        speaker_confidence,
        attestation,
        tree_hash,
        parent_block_id,
        output_truncated,
        output_first_line,
        session_id,
    })
}

/// Public-API unescape mirroring the private `unescape` in
/// the test module. Same algorithm; we expose it here so
/// the viewer can use it without a cfg(test) gate.
fn unescape_simple(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some(other) => { out.push('\\'); out.push(other); }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_block(id: u32, cmd: &str) -> CommandBlock {
        CommandBlock {
            id,
            command: cmd.to_string(),
            output: String::new(),
            cwd: Some("/home/test".to_string()),
            started_at: 1000,
            duration_ms: Some(42),
            exit_code: Some(0),
            start_row: 0,
            speaker_id: None,
            speaker_name: None,
            speaker_confidence: None,
            attestation: "typed".to_string(),
            tree_hash: None,
            parent_block_id: None,
        }
    }

    #[test]
    fn sha256_known_vector() {
        // Empty string SHA-256 (RFC 4634 test vector).
        assert_eq!(sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        // "abc" SHA-256 (RFC 4634 test vector).
        assert_eq!(sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn sha256_longer_vector() {
        // RFC 4634 §B.2: "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
        let v = "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(sha256_hex(v),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
    }

    #[test]
    fn escape_roundtrip() {
        let cases = vec![
            "simple",
            "with\ttab",
            "with\nnewline",
            "with\\backslash",
            "mixed \t\n\\ here",
            "",
        ];
        for s in cases {
            let escaped = escape(s);
            assert!(!escaped.contains('\t') || escaped.contains("\\t"));
            assert!(!escaped.contains('\n') || escaped.contains("\\n"));
            let back = unescape(&escaped);
            assert_eq!(back, s, "escape/unescape roundtrip for {:?}", s);
        }
    }

    #[test]
    fn entry_serializes_to_single_line() {
        let block = make_block(1, "ls /tmp");
        let entry = Entry::from_block(
            &block, None, None, None, "typed", "abc123".to_string(), 0,
        );
        let line = entry.serialize();
        // Exactly one trailing newline; no embedded newlines.
        assert!(line.ends_with('\n'));
        let body = &line[..line.len()-1];
        assert!(!body.contains('\n'),
            "serialized line must have no embedded newline: {:?}", body);
    }

    #[test]
    fn entry_truncates_long_output() {
        let mut block = make_block(2, "yes");
        block.output = "x".repeat(OUTPUT_LIMIT + 100);
        let entry = Entry::from_block(
            &block, None, None, None, "typed", String::new(), 0,
        );
        assert!(entry.output_truncated);
        assert_eq!(entry.output.len(), OUTPUT_LIMIT);
        // trunc=1 should appear in serialization.
        let line = entry.serialize();
        assert!(line.contains("trunc=1"));
    }

    #[test]
    fn entry_includes_speaker_when_attested() {
        let block = make_block(3, "rm /foo");
        let entry = Entry::from_block(
            &block,
            Some("spk_abc".to_string()),
            Some("Allison".to_string()),
            Some(0.94),
            "voice",
            "p".to_string(),
            0,
        );
        let line = entry.serialize();
        assert!(line.contains("spk_id=spk_abc"));
        assert!(line.contains("spk=Allison"));
        assert!(line.contains("spk_conf=0.940"));
        assert!(line.contains("attest=voice"));
    }

    #[test]
    fn entry_omits_speaker_fields_when_typed() {
        let block = make_block(4, "ls");
        let entry = Entry::from_block(
            &block, None, None, None, "typed", "p".to_string(), 0,
        );
        let line = entry.serialize();
        assert!(!line.contains("spk_id="));
        assert!(!line.contains("spk="));
        assert!(!line.contains("spk_conf="));
        assert!(line.contains("attest=typed"));
    }

    #[test]
    fn entry_includes_tree_hash_when_snapshotted() {
        let mut block = make_block(5, "rm /foo");
        block.tree_hash = Some("a".repeat(64));
        let entry = Entry::from_block(
            &block, None, None, None, "typed", "p".to_string(), 0,
        );
        let line = entry.serialize();
        assert!(line.contains(&format!("tree={}", "a".repeat(64))),
            "expected tree= field in serialized line: {}", line);
    }

    #[test]
    fn entry_omits_tree_when_no_snapshot() {
        let block = make_block(6, "ls");
        // tree_hash is None by default in make_block
        let entry = Entry::from_block(
            &block, None, None, None, "typed", "p".to_string(), 0,
        );
        let line = entry.serialize();
        assert!(!line.contains("tree="),
            "expected no tree= field: {}", line);
    }

    #[test]
    fn chain_hash_changes_when_input_changes() {
        let h1 = sha256_hex("entry1");
        let h2 = sha256_hex("entry1 ");  // trailing space
        assert_ne!(h1, h2);
    }

    // Tests of the static PENDING_ATTRIBUTION need to be serialized
    // to avoid races between parallel cargo test runs.
    // Production code is single-threaded per voice utterance so
    // doesn't have this problem.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn pending_attribution_set_and_take_roundtrip() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // Clean slate.
        let _ = take_pending_attribution();

        set_pending_attribution(
            "spk_test_1".to_string(),
            "Allison".to_string(),
            0.94,
            "voice",
        );
        let p = take_pending_attribution()
            .expect("pending should be set");
        assert_eq!(p.speaker_id, "spk_test_1");
        assert_eq!(p.speaker_name, "Allison");
        assert!((p.confidence - 0.94).abs() < 0.001);
        assert_eq!(p.attestation, "voice");
    }

    #[test]
    fn pending_attribution_take_is_single_shot() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _ = take_pending_attribution();
        set_pending_attribution(
            "spk_test_2".to_string(),
            "Bob".to_string(),
            0.85,
            "voice",
        );
        let first = take_pending_attribution();
        assert!(first.is_some(), "first take should return Some");
        let second = take_pending_attribution();
        assert!(second.is_none(),
            "second take should return None — single-shot");
    }

    #[test]
    fn pending_attribution_overwrites_on_double_set() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _ = take_pending_attribution();
        set_pending_attribution(
            "spk_first".to_string(),
            "First".to_string(),
            0.7,
            "voice",
        );
        set_pending_attribution(
            "spk_second".to_string(),
            "Second".to_string(),
            0.9,
            "voice",
        );
        let p = take_pending_attribution().expect("should have pending");
        assert_eq!(p.speaker_name, "Second",
            "newer set should win over older");
    }

    #[test]
    fn pending_attribution_discards_stale() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _ = take_pending_attribution();
        // Manually inject a stale attribution by reaching into the
        // mutex with a fake old timestamp.
        if let Ok(mut g) = PENDING_ATTRIBUTION.lock() {
            *g = Some(PendingAttribution {
                speaker_id: "spk_stale".to_string(),
                speaker_name: "Stale".to_string(),
                confidence: 0.8,
                attestation: "voice".to_string(),
                set_at_ms: now_ms().saturating_sub(PENDING_ATTRIBUTION_STALE_MS + 1000),
            });
        }
        let p = take_pending_attribution();
        assert!(p.is_none(),
            "stale attribution should be discarded as None");
    }

    // v0.1.379 (session arc, step 1): session_id_for tests.
    //
    // These tests mutate the SESSION_STATE static, so they MUST
    // serialize via TEST_LOCK to avoid stomping on each other
    // and on the pending-attribution tests above. They also
    // reset SESSION_STATE to None at start so each test runs
    // in a known-clean baseline regardless of test order.

    fn reset_session_state() {
        if let Ok(mut g) = SESSION_STATE.lock() {
            *g = None;
        }
    }

    #[test]
    fn session_id_for_first_call_starts_new_session() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_session_state();
        let id = session_id_for(1_700_000_000_000, None);
        // Session id is the seconds part of the unix-ms input.
        assert_eq!(id, 1_700_000_000);
    }

    #[test]
    fn session_id_for_within_window_keeps_same_session() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_session_state();
        let first = session_id_for(1_700_000_000_000, None);
        // 5 minutes later — well within the 15-min idle window.
        let second = session_id_for(1_700_000_000_000 + 5 * 60 * 1000, None);
        assert_eq!(first, second,
            "session should persist across short gaps");
    }

    #[test]
    fn session_id_for_after_idle_starts_new_session() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_session_state();
        let first = session_id_for(1_700_000_000_000, None);
        // 16 minutes later — just past the 15-min idle window.
        let second = session_id_for(1_700_000_000_000 + 16 * 60 * 1000, None);
        assert_ne!(first, second,
            "session should reset after idle gap > 15min");
        // The new id should be the seconds part of the new
        // call's timestamp.
        assert_eq!(second, 1_700_000_000 + 16 * 60);
    }

    #[test]
    fn session_id_for_speaker_change_starts_new_session() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_session_state();
        let first = session_id_for(1_700_000_000_000, Some("spk_alice"));
        // Different speaker, no idle gap — new session.
        let second = session_id_for(1_700_000_000_000 + 30_000, Some("spk_bob"));
        assert_ne!(first, second,
            "speaker change should start new session");
    }

    #[test]
    fn session_id_for_typed_after_voice_does_not_break_session() {
        // The same human can switch input modality. A voice
        // command followed by typing must NOT count as a
        // session boundary — only changing speaker IDENTITY
        // does. session_id_for receives None for typed
        // commands; we treat that as "same identity, just
        // typed."
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_session_state();
        let voice = session_id_for(1_700_000_000_000, Some("spk_alice"));
        let typed = session_id_for(1_700_000_000_000 + 30_000, None);
        assert_eq!(voice, typed,
            "typed command after voice should stay in same session");
    }

    #[test]
    fn session_id_for_voice_then_voice_same_speaker_no_break() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_session_state();
        let a = session_id_for(1_700_000_000_000, Some("spk_alice"));
        let b = session_id_for(1_700_000_000_000 + 30_000, Some("spk_alice"));
        assert_eq!(a, b);
    }

    #[test]
    fn session_id_for_speaker_change_after_typed_baseline() {
        // Subtle case: first call is typed (None), then a
        // voice command with speaker_id arrives. This should
        // NOT count as a speaker change — there was no prior
        // speaker to change FROM. Only an established speaker
        // identity followed by a different established
        // speaker identity counts.
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_session_state();
        let typed = session_id_for(1_700_000_000_000, None);
        let voice = session_id_for(1_700_000_000_000 + 30_000, Some("spk_alice"));
        assert_eq!(typed, voice,
            "voice command after typed should stay in same session \
             (no prior speaker identity to change from)");
    }

    #[test]
    fn current_session_id_returns_none_before_any_writes() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_session_state();
        assert_eq!(current_session_id(), None);
    }

    #[test]
    fn current_session_id_returns_active_id_after_writes() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_session_state();
        let id = session_id_for(1_700_000_000_000, None);
        assert_eq!(current_session_id(), Some(id));
    }

    #[test]
    fn entry_serialize_includes_session_field() {
        // Regression test: every serialized entry must carry
        // session=<id>. The audit-log reader / sessions-page
        // depends on this field's presence.
        let block = make_block(99, "ls");
        let entry = Entry::from_block(
            &block, None, None, None, "typed", "p".to_string(), 1700000000,
        );
        let line = entry.serialize();
        assert!(line.contains("session=1700000000"),
            "expected session=1700000000 in: {}", line);
    }
}
