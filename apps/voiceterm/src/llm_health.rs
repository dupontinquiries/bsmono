// src/llm_health.rs
//
// v0.1.441: cached LLM-availability probe.
//
// The problem this solves: callers (summary pipeline, voice
// fragment classifier, future python-NLP) currently attempt an
// LLM call and only discover unavailability by EATING the
// timeout — up to 8s of dead wait spawning ollama against a
// daemon that isn't running, or hitting the Claude API with no
// key. On the timeout-prone local machines that's the single
// worst latency cliff.
//
// `available()` answers "is ANY LLM reachable right now?" cheaply
// and cached, so a caller can skip the LLM path entirely (fall
// back to the instant extractive/heuristic result) instead of
// blocking on a doomed call.
//
// Distinction that matters: system_info::has_ollama() only checks
// the BINARY is on PATH. The daemon can be installed-but-down. A
// real health check pings the daemon's API. We keep both: a
// binary check is the cheap precondition, the daemon probe is the
// truth.
//
// Caching: a successful/failed probe result is cached for
// HEALTH_TTL so we don't probe on every fragment classification.
// `invalidate()` forces a re-probe (e.g. after the user sets an
// API key or starts ollama).

#![allow(dead_code)]

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a probe result stays valid. 30s balances "notice
/// ollama coming up / going down reasonably fast" against "don't
/// probe constantly". Shorter than list_installed_models' 60s
/// because availability flips more meaningfully than the model
/// list.
const HEALTH_TTL: Duration = Duration::from_secs(30);

/// Daemon probe timeout. Must be SHORT — the whole point is to
/// fail fast. 300ms is plenty for a localhost daemon to answer
/// /api/tags; if it can't, treat it as down.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// What LLM backend (if any) is reachable. Reported so callers
/// can tailor behavior (e.g. Claude available but Ollama not).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Health {
    pub ollama: bool,
    pub claude: bool,
}

impl Health {
    pub fn any(&self) -> bool {
        self.ollama || self.claude
    }
    fn none() -> Self {
        Health { ollama: false, claude: false }
    }
}

static CACHE: Mutex<Option<(Instant, Health)>> = Mutex::new(None);

/// Probe the Ollama daemon (not just the binary). Returns true
/// only if the daemon answers its tags endpoint within
/// PROBE_TIMEOUT. We shell out to `ollama list` with a hard
/// timeout rather than open a TCP socket so we don't add an HTTP
/// client dependency and we match how the rest of the code talks
/// to ollama. A binary-absent or daemon-down case both return
/// false fast.
fn probe_ollama() -> bool {
    use std::process::{Command, Stdio};
    // Cheap precondition: if the binary isn't even on PATH, the
    // daemon definitionally isn't reachable. Avoids spawning.
    if !crate::system_info::has_ollama() {
        return false;
    }
    // Spawn `ollama list` and wait with a hard timeout. If it
    // hasn't returned within PROBE_TIMEOUT, kill it and call it
    // down — a healthy daemon answers near-instantly; a hung one
    // is functionally unavailable for our latency budget.
    let child = Command::new("ollama")
        .arg("list")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(_) => return false,
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if start.elapsed() >= PROBE_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return false,
        }
    }
}

/// Run a fresh probe of all backends. `claude_key_present` is
/// passed in by the caller (the key lives on the Backend, not
/// here) — we don't network-probe Claude (that would cost a real
/// request); presence of a non-empty key is treated as "Claude
/// available", which the actual call will confirm/deny.
pub fn probe(claude_key_present: bool) -> Health {
    Health {
        ollama: probe_ollama(),
        claude: claude_key_present,
    }
}

/// Cached availability. Returns the last probe result if still
/// within HEALTH_TTL, otherwise re-probes. `claude_key_present`
/// is consulted on every call (it's cheap and can change without
/// a probe), but the expensive ollama probe is TTL-gated.
pub fn health(claude_key_present: bool) -> Health {
    // Fast path: fresh cache. But the Claude side can flip
    // independently of the (cached) ollama probe, so overlay the
    // current key presence onto a cached ollama result.
    if let Ok(g) = CACHE.lock() {
        if let Some((when, cached)) = *g {
            if when.elapsed() < HEALTH_TTL {
                return Health { ollama: cached.ollama, claude: claude_key_present };
            }
        }
    }
    let fresh = probe(claude_key_present);
    if let Ok(mut g) = CACHE.lock() {
        *g = Some((Instant::now(), fresh));
    }
    fresh
}

/// Convenience: is ANY LLM reachable? Cached.
pub fn available(claude_key_present: bool) -> bool {
    health(claude_key_present).any()
}

/// Force the next health()/available() call to re-probe. Call
/// after the user sets/clears an API key or is known to have
/// started/stopped ollama.
pub fn invalidate() {
    if let Ok(mut g) = CACHE.lock() {
        *g = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests mutate the shared CACHE static; serialize them
    // so parallel test execution can't interleave the seed/assert
    // critical sections.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn claude_key_flips_without_reprobe() {
        let _guard = TEST_LOCK.lock().unwrap();
        // Seed the cache with ollama=false so we exercise the
        // overlay path deterministically (no real probe).
        {
            let mut g = CACHE.lock().unwrap();
            *g = Some((Instant::now(), Health { ollama: false, claude: false }));
        }
        // With a key present, available() should be true purely
        // from the Claude overlay — even though cached ollama is
        // false and we did NOT re-probe.
        assert!(available(true), "claude key present → available");
        assert!(!available(false), "no key + cached ollama down → unavailable");
        invalidate();
    }

    #[test]
    fn any_logic() {
        // Pure value logic — no CACHE touch, no lock needed.
        assert!(Health { ollama: true, claude: false }.any());
        assert!(Health { ollama: false, claude: true }.any());
        assert!(Health { ollama: true, claude: true }.any());
        assert!(!Health::none().any());
    }

    #[test]
    fn invalidate_clears_cache() {
        let _guard = TEST_LOCK.lock().unwrap();
        {
            let mut g = CACHE.lock().unwrap();
            *g = Some((Instant::now(), Health { ollama: true, claude: true }));
        }
        invalidate();
        let g = CACHE.lock().unwrap();
        assert!(g.is_none(), "invalidate clears the cached probe");
    }
}
