// bo-mux — distributed multi-speaker channel mux & coordinator.
//
// See DESIGN.md for the architecture and the honest sync-reality discussion.
//
// Module map:
//   protocol  — pure: messages, term-based election, assignment resolver
//               (unit-tested; logic verified)
//   sync      — pure: clock-offset estimation + scheduled-start math
//               (unit-tested; logic verified)
//   upmix     — pure: 2.0 → surround DSP (unit-tested; logic verified)
//   mesh      — I/O: mDNS + UDP-multicast transport driving the election
//               (structured sketch, depends on the verified pure modules)
//   mux       — I/O: per-channel HTTP splitter (sketch; TODOs mark the
//               audio/HTTP crates that complete it)

pub mod protocol;
/// The one client for the control socket — see the module note on why
/// there were three copies of the path.
pub mod control;

/// Serialises tests that mutate `XDG_RUNTIME_DIR`.
///
/// `control.rs` points it at a nonexistent path to prove a dead socket reads
/// as "not running", and `client.rs` points it at `/run/user/1000`. Both are
/// process-global `set_var` calls in one test binary that cargo runs in
/// parallel, so one test restoring the variable mid-way through the other's
/// call lets the dead-socket test reach the REAL daemon — which answers, and
/// the test that expected an error gets a status object instead.
#[cfg(test)]
pub(crate) static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub mod peer_auth;
pub mod drone_canceller;
pub mod passby_canceller;
pub mod sync;
pub mod upmix;
pub mod channel_layout;
pub mod mesh;
pub mod mesh_runner;
pub mod mux;
pub mod chan_server;
pub mod masker;
pub mod cancel_tones;
pub mod soundscape;
pub mod soundscape_config;
pub mod player;
pub mod transport;
pub mod sink_system;
pub mod delivery;
pub mod api;
pub mod deploy;
pub mod client;
pub mod activity_watch;
pub mod power_state;

#[cfg(test)]
mod bootstrap_scene_validation {
    //! Regression guard: every scene file shipped in examples/ (the
    //! bootstrap presets) MUST load through the same path the daemon
    //! uses (config-first parse + compile, raw Soundscape fallback).
    //! This caught a real bug where the bootstrapped presets used the
    //! rich config schema (start_state as a string) but the daemon
    //! only parsed the raw runtime schema (start_state as a usize),
    //! so none of the presets actually loaded at runtime.
    use super::*;
    #[test]
    fn all_example_scenes_load() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples");
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
            let json = std::fs::read_to_string(&path).unwrap();
            // Mirror the daemon loader: config-first, raw fallback.
            let r = soundscape_config::parse(&json)
                .and_then(|cfg| soundscape_config::compile(&cfg).map(|_| ()))
                .or_else(|_| serde_json::from_str::<soundscape::Soundscape>(&json)
                    .map(|_| ()).map_err(|e| e.to_string()));
            assert!(r.is_ok(), "scene {:?} failed to load: {:?}", path, r.err());
            checked += 1;
        }
        assert!(checked > 0, "no scenes checked");
        eprintln!("validated {} example scenes", checked);
    }
}
