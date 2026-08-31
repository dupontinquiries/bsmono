//! control — the one client for the bo-muxd control socket.
//!
//! ## Why this exists
//!
//! The socket path was written out THREE times: once in the daemon that
//! binds it, once in `bo-controller`'s Qt bridge that connects to it,
//! and nowhere the egui port could reach — so this port had no daemon
//! connection at all, and `load_scene` was a Qt-only feature.
//!
//! That is the same shape as `scene_dir` one commit ago, and it went
//! wrong the same way: two private copies of a path that must agree, and
//! a third caller that could not join in.
//!
//! ## The protocol, as the daemon actually speaks it
//!
//! Line-delimited JSON over a unix socket. A request carries an `id` and
//! a `cmd`; the reply carries the same `id`. Read until the id matches
//! rather than taking the first line — the daemon can emit unsolicited
//! lines, and treating one of those as the answer would report a status
//! update as the result of a scene load.
//!
//! ## Timeouts are not optional here
//!
//! A GUI calls this from a frame. A daemon that accepted the connection
//! and then stalled would freeze the UI for as long as it felt like, so
//! both directions get a deadline and a miss is reported as "no daemon"
//! rather than waited out.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

/// How long any single call may take.
///
/// One second: long enough for a busy daemon to answer, short enough
/// that a frame which hits a dead socket is a stutter rather than a
/// hang.
pub const TIMEOUT: Duration = Duration::from_millis(1000);

/// Where the control socket lives.
///
/// The daemon binds this and every client connects to it, so it has to
/// be one function. `XDG_RUNTIME_DIR` first, `/tmp` as the fallback —
/// matching what the daemon already did.
pub fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("bo-mux")
        .join("control.sock")
}

/// Is the daemon listening?
///
/// A connect attempt, not a path check: the socket FILE outlives a
/// daemon that crashed, so `exists()` reports a dead daemon as running
/// and every later call fails with a confusing error instead of a
/// straight "not running".
pub fn is_running() -> bool {
    UnixStream::connect(socket_path()).is_ok()
}

/// Why a call did not produce an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// Nothing is listening. Distinct from every other case because it
    /// is the one a person can act on — start the daemon.
    NotRunning,
    /// Connected, but no reply with our id arrived in time.
    Timeout,
    /// The daemon answered with an error.
    Refused(String),
    /// Something else: a malformed reply, a closed stream mid-call.
    Broken(String),
}

impl CallError {
    pub fn message(&self) -> String {
        match self {
            CallError::NotRunning => "bo-muxd is not running".into(),
            CallError::Timeout => "bo-muxd did not answer".into(),
            CallError::Refused(e) => e.clone(),
            CallError::Broken(e) => format!("bo-muxd: {e}"),
        }
    }
}

/// Send one command and wait for its reply.
///
/// Takes `api::Command`, not a string and a JSON blob.
///
/// The daemon already deserialises into this enum — `Request` is
/// `{ id, #[serde(flatten)] cmd }` with `#[serde(tag = "cmd")]` — so a
/// hand-built object was a SECOND encoding of a schema that already
/// exists, free to drift from it and only discoverable at run time
/// against a live daemon. My first version did exactly that; the Qt
/// bridge does too, which is presumably where I got the idea.
///
/// With the enum, a command the daemon does not have is a compile
/// error.
pub fn send(id: u64, cmd: crate::api::Command)
    -> Result<serde_json::Value, CallError>
{
    let stream = UnixStream::connect(socket_path())
        .map_err(|_| CallError::NotRunning)?;
    // Best effort: a platform that refuses these is better served
    // without them than by failing the call outright. Linux supports
    // both.
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));

    let req = crate::api::Request { id, cmd };

    let mut w = stream.try_clone().map_err(|e| CallError::Broken(e.to_string()))?;
    let mut line = serde_json::to_string(&req).map_err(|e| CallError::Broken(e.to_string()))?;
    line.push('\n');
    w.write_all(line.as_bytes()).map_err(|e| CallError::Broken(e.to_string()))?;
    w.flush().map_err(|e| CallError::Broken(e.to_string()))?;

    let reader = BufReader::new(stream);
    for l in reader.lines() {
        let l = match l {
            Ok(l) => l,
            // A read timeout arrives as an error here, and it is the
            // ordinary "daemon went quiet" case rather than a fault.
            Err(_) => return Err(CallError::Timeout),
        };
        // Parsed as the daemon's own `Response`, so `ok`/`data`/`error`
        // are read the way it writes them rather than by poking at
        // field names.
        let Ok(r) = serde_json::from_str::<crate::api::Response>(&l) else {
            // A line we cannot parse is SKIPPED, not fatal. The daemon
            // may log to the same socket, and one noisy line must not
            // lose an answer that is right behind it.
            continue;
        };
        if r.id != id {
            // Not ours. Skipped for the reason in the module note:
            // taking the first line would report a status update as the
            // result of a scene load.
            continue;
        }
        if let Some(e) = r.error {
            return Err(CallError::Refused(e));
        }
        // `ok` without `data` is legitimate — `Response::ok_empty` is a
        // real constructor — so an absent payload is Null rather than an
        // error.
        return Ok(r.data.unwrap_or(serde_json::Value::Null));
    }
    // The stream closed without our id. Not a timeout — the daemon
    // hung up — and naming it separately means a person is not told to
    // wait for something that already gave up.
    Err(CallError::Broken("connection closed before a reply".into()))
}

/// Ask the daemon to load a scene by name.
///
/// The NAME, not a path: the daemon resolves it against its own
/// `scene_dir`, which is the same one `soundscape_config::scene_dir`
/// returns. Sending a path would let a caller point the daemon at a file
/// outside that directory, and the two would stop agreeing about what a
/// scene name means.
pub fn load_scene(id: u64, name: &str) -> Result<serde_json::Value, CallError> {
    send(id, crate::api::Command::LoadScene {
        name: Some(name.to_string()),
        inline_json: None,
    })
}

/// Scene names the DAEMON knows about.
///
/// Different from `soundscape_config::scene_names`, which reads the
/// directory directly, and both are wanted: the directory listing works
/// with no daemon running, and this one reflects what the daemon has
/// actually loaded — including anything it bootstrapped on start that
/// is not on disk yet.
pub fn list_scenes(id: u64) -> Result<serde_json::Value, CallError> {
    send(id, crate::api::Command::ListScenes)
}

pub fn status(id: u64) -> Result<serde_json::Value, CallError> {
    send(id, crate::api::Command::Status)
}

pub fn play(id: u64, seed: Option<u64>) -> Result<serde_json::Value, CallError> {
    send(id, crate::api::Command::Play { seed })
}

pub fn stop(id: u64) -> Result<serde_json::Value, CallError> {
    send(id, crate::api::Command::Stop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_is_under_the_runtime_dir() {
        // The daemon binds this and every client connects to it, so the
        // shape has to be one thing. Asserted as a shape, not a literal:
        // the prefix depends on XDG_RUNTIME_DIR.
        let p = socket_path();
        assert!(p.ends_with("bo-mux/control.sock"), "{}", p.display());
    }

    #[test]
    fn a_dead_socket_is_not_running_rather_than_an_error() {
        // The one case a person can act on, so it gets its own variant.
        let old = std::env::var_os("XDG_RUNTIME_DIR");
        std::env::set_var("XDG_RUNTIME_DIR", "/nonexistent-bo-mux-sock-test");
        let running = is_running();
        let e = status(1).unwrap_err();
        match old {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
        assert!(!running);
        assert_eq!(e, CallError::NotRunning);
        assert!(e.message().contains("not running"), "{}", e.message());
    }

    #[test]
    fn a_request_round_trips_through_the_daemons_own_types() {
        // The property the typed client buys: what this sends is what
        // the daemon parses, checked here rather than against a live
        // socket. A hand-built JSON object could drift from the schema
        // and only fail at run time on somebody's machine.
        let req = crate::api::Request {
            id: 7,
            cmd: crate::api::Command::LoadScene {
                name: Some("forest_quiet".into()), inline_json: None,
            },
        };
        let line = serde_json::to_string(&req).unwrap();
        // `cmd` is flattened and snake_cased by the tag attribute, which
        // is exactly what the daemon reads.
        assert!(line.contains("\"cmd\":\"load_scene\""), "{line}");
        let back: crate::api::Request = serde_json::from_str(&line).unwrap();
        assert_eq!(back.id, 7);
        assert!(matches!(back.cmd, crate::api::Command::LoadScene { .. }));
    }

    #[test]
    fn an_ok_reply_with_no_data_is_not_an_error() {
        // `Response::ok_empty` is a real constructor, so an absent
        // payload is a success with nothing to say — treating it as a
        // failure would make every Stop and LeaveSession look broken.
        let line = serde_json::to_string(&crate::api::Response::ok_empty(3)).unwrap();
        let r: crate::api::Response = serde_json::from_str(&line).unwrap();
        assert!(r.ok && r.error.is_none() && r.data.is_none());
    }

    #[test]
    fn every_error_says_something_a_person_can_read() {
        // These reach a toast. An empty or debug-formatted message is a
        // toast that tells somebody nothing.
        for e in [
            CallError::NotRunning,
            CallError::Timeout,
            CallError::Refused("no such scene".into()),
            CallError::Broken("closed".into()),
        ] {
            assert!(!e.message().is_empty(), "{e:?}");
        }
        assert_eq!(CallError::Refused("no such scene".into()).message(), "no such scene",
                   "the daemon's own words should pass through unwrapped");
    }
}
