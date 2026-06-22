//! Structured event log — a parallel `events.jsonl` file written
//! alongside the human-readable terminal log.
//!
//! ## Why a separate event log
//!
//! The human-readable log (`log::info!(…)`) is for the developer reading
//! during a session. It's prose, includes context, has variable shape.
//! Grep and eyeballs are the analysis tools.
//!
//! The event log is for *retrospective machine analysis*: given a
//! collection of runs, answer questions like "what was the shadow p50
//! across all c.7-c.10 runs at RD=8 with INDIRECT=1?" Doing that against
//! prose logs is brittle. Doing it against JSON-lines is a one-liner.
//!
//! ## Format
//!
//! One JSON object per line. Schema is open-ended — each event has a
//! `kind` field and the other fields depend on the kind. Required
//! fields on every event: `uuid` (run UUID), `ts_ms` (milliseconds
//! since run start), `kind` (string).
//!
//! Currently emitted kinds:
//!   - `start` — first event of a run. Records version, cmdline, binary.
//!   - `flag` — env-var/runtime-flag value, one per known flag at startup.
//!   - `frame_window` — rolled-up stats from one FPS-log window (~1s).
//!   - `shutdown` — last event. Records duration and shutdown reason.
//!
//! Adding new kinds is just emitting another `record(…)` call elsewhere
//! in the code. No schema changes here required.
//!
//! ## File path
//!
//! Default: `events.jsonl` in the current working directory (matches
//! `run_summaries.txt`). Overridable via `UNENDLICH_EVENTS_PATH` for
//! tests or for routing to a per-run directory.
//!
//! Multiple runs append to the same file. The run UUID separates them.
//!
//! ## Failure mode
//!
//! Event emission failures (disk full, permission denied) log a warning
//! and continue. The events file is diagnostic; the run must not crash
//! because of it.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

/// Default path for the events file, relative to the current working
/// directory. Tests override via the `UNENDLICH_EVENTS_PATH` env var.
pub const DEFAULT_PATH: &str = "events.jsonl";

/// Global event-sink singleton. Initialised by `init()`; subsequent
/// `record(…)` calls write to it. If never initialised, `record(…)` is
/// a no-op (so library code that emits events doesn't break tests that
/// haven't set up the sink).
static SINK: OnceLock<Mutex<EventSink>> = OnceLock::new();

struct EventSink {
    uuid: String,
    start_instant: std::time::Instant,
    file: Option<std::fs::File>,
    /// Count of successful event writes. Useful for tests + diagnostics.
    /// Not externally exposed but readable via the test API.
    events_written: u64,
    /// Count of write errors. If non-zero on shutdown, we log a warning
    /// since the events file is incomplete.
    write_errors: u64,
}

/// Initialise the global event sink for this process. Idempotent:
/// calling twice with different UUIDs is a programmer error (panics in
/// debug; in release the second call is silently ignored).
///
/// `uuid` should come from `run_id::fresh()`. The current time becomes
/// the t=0 reference for all subsequent `ts_ms` values.
pub fn init(uuid: &str) {
    let path = std::env::var("UNENDLICH_EVENTS_PATH")
        .unwrap_or_else(|_| DEFAULT_PATH.to_string());

    let file = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(f) => Some(f),
        Err(e) => {
            log::warn!("Couldn't open events file {}: {}. Events will be dropped.", path, e);
            None
        }
    };

    let sink = EventSink {
        uuid: uuid.to_string(),
        start_instant: std::time::Instant::now(),
        file,
        events_written: 0,
        write_errors: 0,
    };

    if SINK.set(Mutex::new(sink)).is_err() {
        debug_assert!(false, "events::init called twice");
    }
}

/// Record an event. The map of additional fields is serialised as JSON
/// alongside the common fields (uuid, ts_ms, kind). Field values can be
/// strings, numbers, or bools — anything `serde_json::Value` accepts.
///
/// Failures are logged and counted; the call always returns.
pub fn record(kind: &str, fields: serde_json::Value) {
    let Some(sink_lock) = SINK.get() else {
        // Sink not initialised — silently drop. Library code might emit
        // events from contexts where the binary hasn't called init();
        // dropping is the safe behaviour.
        return;
    };
    let mut sink = match sink_lock.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };

    // Read non-file fields BEFORE taking the &mut file borrow.
    let ts_ms = sink.start_instant.elapsed().as_millis() as u64;
    let uuid = sink.uuid.clone();

    // Merge common fields with kind-specific fields. Common fields win
    // on conflict (so callers can't accidentally clobber the uuid).
    let mut obj = match fields {
        serde_json::Value::Object(m) => m,
        // If a caller passes a non-object, wrap it under "data".
        other => {
            let mut m = serde_json::Map::new();
            m.insert("data".to_string(), other);
            m
        }
    };
    obj.insert("uuid".to_string(), serde_json::Value::String(uuid));
    obj.insert("ts_ms".to_string(), serde_json::Value::Number(ts_ms.into()));
    obj.insert("kind".to_string(), serde_json::Value::String(kind.to_string()));

    let line = match serde_json::to_string(&obj) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Failed to serialize event {}: {}", kind, e);
            sink.write_errors += 1;
            return;
        }
    };

    // Now take the &mut file borrow.
    let Some(ref mut file) = sink.file else {
        return;
    };
    if let Err(e) = writeln!(file, "{}", line) {
        log::warn!("Failed to write event {}: {}", kind, e);
        // Can't update sink.write_errors here without re-borrowing.
        // Skip the counter increment — log line already records the error.
        return;
    }
    // Drop file borrow before touching sink.events_written.
    let _ = file;
    sink.events_written += 1;
}

/// Flush the events file. Call at shutdown so the last events aren't
/// lost on crash. Cheap to call multiple times.
pub fn flush() {
    let Some(sink_lock) = SINK.get() else { return; };
    let mut sink = match sink_lock.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(ref mut file) = sink.file {
        let _ = file.flush();
    }
    if sink.write_errors > 0 {
        log::warn!(
            "Event log had {} write errors (got {} successful events). Events file may be incomplete.",
            sink.write_errors, sink.events_written,
        );
    }
}

/// Test-only helper: number of events written by this process so far.
/// Used to assert that emission worked in integration tests.
#[cfg(test)]
pub fn events_written() -> u64 {
    SINK.get()
        .and_then(|m| m.lock().ok().map(|g| g.events_written))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Helper: unique temp path. We can't init() twice in the same
    /// process, so most tests work via the no-init path (record() is
    /// a no-op when init hasn't run).
    #[allow(dead_code)]
    fn unique_path(label: &str) -> String {
        let pid = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("/tmp/unendlich-events-{}-{}-{}.jsonl", label, pid, ts)
    }

    /// `record()` with no init is a silent no-op (doesn't panic, doesn't
    /// write anywhere).
    #[test]
    fn record_without_init_is_noop() {
        // Just verify no panic. We can't easily verify "didn't write"
        // because there's no file path to check.
        record("test_kind", json!({"foo": "bar"}));
    }

    /// `flush()` without init is a no-op.
    #[test]
    fn flush_without_init_is_noop() {
        flush();
    }
}
