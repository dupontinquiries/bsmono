//! playback — the thing that makes a sound.
//!
//! Everything around this exists and is tested. `Transport` holds the
//! position, the repeat mode and the pause-is-really-stop rule. `Queue`
//! holds what plays next. `SinkPicker` produces a [`Route`] — one speaker
//! or several. And nothing anywhere in the tree opens an audio device, so
//! the play button has never produced a sound: the binary is 66 lines with
//! three hardcoded rooms in it.
//!
//! ## A subprocess, for the third time in this fleet and the same reasons
//!
//! `mpv` (or `ffplay`, or `paplay`) spawned through `sh`, its sink chosen
//! by an environment variable. Not a linked decoder:
//!
//! * A UI crate that links an audio stack cannot be tested anywhere
//!   without a sound card, and this crate's tests run in CI.
//! * The command is a string you can read in a log and paste into a
//!   terminal, which is the difference between "no sound" and "no sound,
//!   and here is exactly what I asked for".
//! * Cross-building for ARM stays as cheap as it is; the runtime
//!   dependency is a binary every distribution ships.
//!
//! ## Multi-speaker is a routing decision, not a second player
//!
//! Playing the same track from two processes gives two clocks and an echo
//! — they drift within seconds, and the drift is audible in a way nobody
//! can fix from the UI. So a multi-room route builds ONE stream and points
//! it at a combine sink that fans out. The sink has to exist first, which
//! is [`Plan::setup`]: the command that creates it, separate from the one
//! that plays, because creating it is a system change that outlives the
//! track.
//!
//! ## What is verified here and what is not
//!
//! The plan — which commands, in which order, with which sink — is pure
//! and tested. The subprocess mechanics are tested against a stand-in
//! launcher, as in `snapclone::capture`. NOT verified: that `mpv` and
//! `pactl` accept these flags, and that any sound has come out. This
//! sandbox has no audio device.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Where a stream should go.
///
/// Mirrors `settings_page::Route` rather than reusing it, so the engine
/// does not depend on the settings page — the routing decision belongs to
/// the picker, and the engine only needs to know the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sink {
    /// The system default. What you get before anyone has chosen.
    Default,
    /// One named PulseAudio/PipeWire sink.
    One(String),
    /// Several, through a combine sink built for them.
    Many(Vec<String>),
}

impl Sink {
    /// The combine sink's name, for a multi-room route.
    ///
    /// Derived from the members rather than random or fixed: a fixed name
    /// collides when a second app does the same thing, and a random one
    /// leaks a sink per route change because nothing knows the old name to
    /// tear it down.
    pub fn combine_name(members: &[String]) -> String {
        let mut ids: Vec<&str> = members.iter().map(|s| s.as_str()).collect();
        // Sorted, so the same set of speakers in a different order is the
        // same sink — otherwise picking A then B and B then A leaves two.
        ids.sort_unstable();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for id in &ids {
            for b in id.as_bytes() {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        format!("bo_combine_{h:016x}")
    }
}

/// The default player command. `--no-video` because a music file with
/// cover art embedded otherwise opens a window.
pub const DEFAULT_PLAYER: &str = concat!(
    "mpv --no-video --really-quiet ",
    // THE POSITION, on stdout, in a format this app defined.
    //
    // `Transport::report` is the sync point between the player's clock and
    // the progress bar, and nothing called it: the bar extrapolated from
    // zero forever and `duration` was never set, so `fraction()` was
    // permanently 0 and the handle never moved.
    //
    // mpv will print a status line of our choosing, so the channel costs
    // no IPC socket and no second connection — the process we already
    // spawned tells us on the pipe we already have. A prefix nothing else
    // emits, so a player that ignores the flag simply reports nothing
    // rather than having its ordinary chatter misread as a timestamp.
    // The prefix is spelled out here because `concat!` takes literals
    // only; `STATUS_PREFIX` below is the same string and a test holds the
    // two together, so a change to one that misses the other fails here
    // rather than as a progress bar that silently stops moving.
    "--term-status-msg='BOPOS ${=time-pos} ${=duration}'");

/// The marker a status line starts with.
pub const STATUS_PREFIX: &str = "BOPOS";

/// Parse one status line into (position, duration), both seconds.
///
/// Anything else is `None`, including a line that starts with the prefix
/// and does not carry two numbers: mpv prints `${=duration}` literally for
/// a stream that has none, and reading that as a length would put the
/// handle at the far end of a bar with no end.
pub fn parse_status(line: &str) -> Option<(f64, f64)> {
    let rest = line.trim().strip_prefix(STATUS_PREFIX)?;
    let mut it = rest.split_whitespace();
    let pos: f64 = it.next()?.parse().ok()?;
    let dur: f64 = it.next()?.parse().ok()?;
    if !pos.is_finite() || !dur.is_finite() || pos < 0.0 || dur <= 0.0 {
        return None;
    }
    Some((pos, dur))
}

/// The commands to run for one track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Run first, once — creates the combine sink. Empty for a single
    /// speaker, which needs no setup at all.
    pub setup: Vec<String>,
    /// The player, with its sink already chosen.
    pub play: String,
    /// Run when the route is abandoned. Tearing a combine sink down is not
    /// optional: they persist in the audio server, and a session that
    /// creates one per route change leaves a list of dead sinks in every
    /// other app's output picker.
    pub teardown: Vec<String>,
}

/// Build the plan for a track on a route.
///
/// `url` is quoted for the shell. It comes from a library that scanned a
/// disk, so it contains apostrophes and spaces routinely, and an unquoted
/// path is both a bug and an injection.
pub fn plan(player: &str, url: &str, sink: &Sink) -> Plan {
    let q = shell_quote(url);
    match sink {
        Sink::Default => Plan { setup: Vec::new(), play: format!("{player} {q}"),
                                teardown: Vec::new() },
        Sink::One(name) => Plan {
            setup: Vec::new(),
            // The sink travels as an environment variable rather than a
            // player flag: every player spells the flag differently and
            // PULSE_SINK is understood by all of them through libpulse.
            // Swapping mpv for ffplay then costs nothing.
            play: format!("PULSE_SINK={} {player} {q}", shell_quote(name)),
            teardown: Vec::new(),
        },
        Sink::Many(members) => {
            let combine = Sink::combine_name(members);
            let slaves = members.iter().map(|m| m.as_str()).collect::<Vec<_>>().join(",");
            Plan {
                setup: vec![format!(
                    "pactl load-module module-combine-sink sink_name={} slaves={} \
                     sink_properties=device.description={}",
                    combine, shell_quote(&slaves), combine)],
                play: format!("PULSE_SINK={combine} {player} {q}"),
                teardown: vec![format!("pactl unload-module module-combine-sink")],
            }
        }
    }
}

/// Single-quote for `sh`, the way that survives every character.
///
/// A path from a music library contains apostrophes — `Don't Stop`, every
/// second album — and the naive version breaks on exactly those.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// What the player is doing, for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Idle,
    Playing { url: String },
    /// The player exited with something to say. Its last stderr line,
    /// because "playback failed" tells nobody anything and "Failed to open
    /// audio device" tells them everything.
    Failed(String),
}

/// Shared between the player thread and the UI.
#[derive(Default)]
pub struct Shared {
    state: Mutex<Option<State>>,
    /// The player's own clock: (position, duration). Latest wins — a
    /// backlog of timestamps is worse than the newest one, since every
    /// older entry is a position the track has already passed.
    progress: Mutex<Option<(f64, f64)>>,
    /// The track finished on its own, as opposed to being stopped. Only
    /// this should advance the queue — a track stopped because the user
    /// pressed pause must not skip to the next one.
    ended: std::sync::atomic::AtomicBool,
}

impl Shared {
    pub fn new() -> Arc<Shared> {
        Arc::new(Shared::default())
    }
    /// Edge-triggered, so the host does not re-apply the same state every
    /// frame.
    pub fn take_state(&self) -> Option<State> {
        self.state.lock().ok()?.take()
    }
    /// The newest position the player reported, if it has since the last
    /// call.
    pub fn take_progress(&self) -> Option<(f64, f64)> {
        self.progress.lock().ok()?.take()
    }
    pub fn take_ended(&self) -> bool {
        self.ended.swap(false, Ordering::Relaxed)
    }
    fn set(&self, s: State) {
        if let Ok(mut g) = self.state.lock() { *g = Some(s); }
    }
}

/// A running player.
pub struct Player {
    child: Arc<Mutex<Option<std::process::Child>>>,
    pub shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The teardown for the route currently loaded, run when it changes.
    teardown: Vec<String>,
}

impl Default for Player {
    fn default() -> Self {
        Player { child: Arc::new(Mutex::new(None)), shared: Shared::new(),
                 stop: Arc::new(AtomicBool::new(false)), thread: None,
                 teardown: Vec::new() }
    }
}

impl Player {
    pub fn new() -> Player { Player::default() }

    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Start a track. Stops whatever was playing first.
    ///
    /// `launcher` is the shell used, injectable so the subprocess path can
    /// be tested against a stand-in — the same seam `snapclone::capture`
    /// uses, and for the same reason: without it the only test possible is
    /// "does it compile".
    pub fn start_with(&mut self, plan: &Plan, launcher: &str) {
        self.stop_now();
        for cmd in &plan.setup {
            // Setup failures are NOT fatal: a combine sink that already
            // exists returns an error, and refusing to play because of it
            // would break the second track on the same route.
            let _ = std::process::Command::new(launcher).arg("-c").arg(cmd).status();
        }
        self.teardown = plan.teardown.clone();
        self.stop = Arc::new(AtomicBool::new(false));
        self.shared = Shared::new();
        let (shared, stop, holder) =
            (self.shared.clone(), self.stop.clone(), self.child.clone());
        let cmd = plan.play.clone();
        let launcher = launcher.to_string();
        let url = plan.play.clone();
        self.thread = Some(std::thread::spawn(move || {
            let spawned = std::process::Command::new(&launcher)
                .arg("-c").arg(format!("exec {cmd}"))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn();
            let mut child = match spawned {
                Ok(c) => c,
                Err(e) => { shared.set(State::Failed(format!("could not start: {e}"))); return; }
            };
            let err = child.stderr.take();
            let out = child.stdout.take();
            if let Ok(mut g) = holder.lock() { *g = Some(child); }
            shared.set(State::Playing { url });

            // The status reader, on its own thread. On the wait thread it
            // would block the exit check between lines, and mpv prints a
            // status line about once a second — so a track would be up to
            // a second late noticing it had ended.
            let reader = out.map(|o| {
                let shared = shared.clone();
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    // Status lines end with a carriage return, not a
                    // newline: mpv redraws one line in place. Splitting on
                    // '\n' would yield the whole session as one line at
                    // the end, which is a progress bar that jumps from 0
                    // to done.
                    let mut buf = Vec::new();
                    let mut r = std::io::BufReader::new(o);
                    while let Ok(n) = r.read_until(b'\r', &mut buf) {
                        if n == 0 { break; }
                        let line = String::from_utf8_lossy(&buf).into_owned();
                        buf.clear();
                        for part in line.split(['\n', '\r']) {
                            if let Some(p) = parse_status(part) {
                                if let Ok(mut g) = shared.progress.lock() { *g = Some(p); }
                            }
                        }
                    }
                })
            });

            // Wait on the child OUTSIDE the mutex: holding it across a wait
            // means `stop_now` blocks until the track ends, which is a UI
            // freeze for the length of a song.
            let status = loop {
                let done = {
                    let Ok(mut g) = holder.lock() else { return };
                    match g.as_mut() {
                        Some(c) => c.try_wait().ok().flatten(),
                        None => break None,
                    }
                };
                if let Some(s) = done { break Some(s); }
                if stop.load(Ordering::Relaxed) { break None; }
                std::thread::sleep(std::time::Duration::from_millis(50));
            };
            if let Ok(mut g) = holder.lock() { let _ = g.take(); }
            // Joined, not detached: a detached reader holding a `Shared`
            // would keep writing positions into a player that has been
            // replaced. This returns promptly because the pipe closes when
            // the child goes — which is only true because the command is
            // built with `exec`, so the process we kill is the one holding
            // the pipe rather than a shell wrapping it.
            if let Some(t) = reader { let _ = t.join(); }

            if stop.load(Ordering::Relaxed) {
                // Stopped on purpose. NOT an end: advancing the queue here
                // would make pause skip a track.
                return;
            }
            match status {
                Some(s) if s.success() => {
                    shared.ended.store(true, Ordering::Relaxed);
                    shared.set(State::Idle);
                }
                Some(_) | None => {
                    let mut msg = String::new();
                    if let Some(mut e) = err {
                        use std::io::Read;
                        let _ = e.read_to_string(&mut msg);
                    }
                    let last = msg.trim().lines().last().unwrap_or("player exited").to_string();
                    shared.set(State::Failed(last));
                }
            }
        }));
    }

    /// Start with the system shell.
    pub fn start(&mut self, plan: &Plan) {
        self.start_with(plan, "sh");
    }

    /// Stop the player. Both halves: the flag ends the wait loop, the kill
    /// ends the child — a flag alone leaves a song playing to the end of a
    /// track nobody is listening to.
    pub fn stop_now(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut g) = self.child.lock() {
            if let Some(c) = g.as_mut() { let _ = c.kill(); }
        }
        if let Some(t) = self.thread.take() { let _ = t.join(); }
        if let Ok(mut g) = self.child.lock() {
            if let Some(mut c) = g.take() { let _ = c.wait(); }
        }
    }

    /// Give up the route's resources. Called when the route CHANGES, not
    /// per track: a combine sink torn down between two tracks on the same
    /// speakers is a gap of silence and a rebuild for nothing.
    pub fn release_route(&mut self, launcher: &str) {
        self.stop_now();
        for cmd in std::mem::take(&mut self.teardown) {
            let _ = std::process::Command::new(launcher).arg("-c").arg(&cmd).status();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // A player left running after the app closes is a song that keeps
        // playing with nothing to stop it.
        self.stop_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_with_an_apostrophe_survives_the_shell() {
        // Every second album has one. The naive quoting breaks on exactly
        // those, and the failure is a track that silently does not play.
        let p = plan("mpv", "/music/Don't Stop/01 - Hey, You.flac", &Sink::Default);
        assert!(p.play.contains(r#"'/music/Don'\''t Stop/01 - Hey, You.flac'"#),
                "{}", p.play);
    }

    #[test]
    fn one_speaker_needs_no_setup_and_no_teardown() {
        let p = plan("mpv", "/a.flac", &Sink::One("bluez_sink.AA".into()));
        assert!(p.setup.is_empty() && p.teardown.is_empty());
        assert!(p.play.starts_with("PULSE_SINK='bluez_sink.AA' mpv"), "{}", p.play);
    }

    #[test]
    fn several_speakers_get_one_stream_through_a_combine_sink() {
        // Two players is two clocks and an echo — they drift within
        // seconds, audibly, and nothing in the UI can fix it.
        let p = plan("mpv", "/a.flac",
                     &Sink::Many(vec!["one".into(), "two".into()]));
        assert_eq!(p.setup.len(), 1);
        assert!(p.setup[0].contains("module-combine-sink"), "{}", p.setup[0]);
        assert!(p.setup[0].contains("slaves='one,two'"), "{}", p.setup[0]);
        assert_eq!(p.play.matches("mpv").count(), 1, "more than one player: {}", p.play);
        assert!(!p.teardown.is_empty(), "a combine sink that is never unloaded");
    }

    #[test]
    fn the_same_speakers_in_a_different_order_are_the_same_sink() {
        // Otherwise picking A then B and B then A leaves two combine sinks
        // behind, and every other app's output list fills with them.
        let a = Sink::combine_name(&["one".into(), "two".into()]);
        let b = Sink::combine_name(&["two".into(), "one".into()]);
        assert_eq!(a, b);
        assert_ne!(a, Sink::combine_name(&["one".into(), "three".into()]));
    }

    #[cfg(unix)]
    fn fake_player(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir()
            .join(format!("bo-play-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sh");
        // Stands in for `sh`: ignores -c and runs the body instead, so the
        // spawn, the wait, the exit code and the stderr capture are all
        // exercised without a player or a sound card.
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_status_line_parses_and_anything_else_does_not() {
        assert_eq!(parse_status("BOPOS 12.5 240.0"), Some((12.5, 240.0)));
        assert_eq!(parse_status("  BOPOS 0 1  "), Some((0.0, 1.0)));
        // mpv prints the placeholder literally for a stream with no
        // length. Reading that as a duration puts the handle at the far
        // end of a bar with no end.
        assert_eq!(parse_status("BOPOS 3.0 ${=duration}"), None);
        assert_eq!(parse_status("BOPOS 3.0 0"), None, "a zero-length track");
        assert_eq!(parse_status("AV: 00:00:03 / 00:04:00"), None);
        assert_eq!(parse_status(""), None);
    }

    #[test]
    fn the_prefix_in_the_command_is_the_prefix_the_parser_looks_for() {
        // `concat!` takes literals, so the command spells it out while the
        // parser uses the constant. A change to one that misses the other
        // is a progress bar that silently stops moving, which is a hard
        // thing to attribute later.
        assert!(DEFAULT_PLAYER.contains(STATUS_PREFIX),
                "the player command does not emit {STATUS_PREFIX}");
    }

    #[test]
    #[cfg(unix)]
    fn the_players_clock_reaches_the_app() {
        // `Transport::report` is the sync point and had no caller: the bar
        // extrapolated from zero forever and `duration` was never set, so
        // the handle never moved.
        //
        // Carriage returns, not newlines — mpv redraws one line in place,
        // and a reader splitting on '\n' gets the whole session at once
        // at the end.
        let sh = fake_player("status",
            "printf 'BOPOS 1.0 200.0\rBOPOS 2.0 200.0\r'; exit 0");
        let mut p = Player::new();
        p.start_with(&plan("mpv", "/a.flac", &Sink::Default), &sh.display().to_string());
        let mut got = None;
        for _ in 0..200 {
            if let Some(v) = p.shared.take_progress() { got = Some(v); }
            if !p.is_running() && got.is_some() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (pos, dur) = got.expect("no position ever reached the app");
        assert_eq!(dur, 200.0);
        assert!(pos >= 1.0, "the position was never updated: {pos}");
    }

    #[test]
    #[cfg(unix)]
    fn a_track_that_finishes_reports_an_end_the_queue_can_act_on() {
        let sh = fake_player("ok", "exit 0");
        let mut p = Player::new();
        p.start_with(&plan("mpv", "/a.flac", &Sink::Default), &sh.display().to_string());
        for _ in 0..200 {
            if !p.is_running() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(p.shared.take_ended(), "a finished track did not report an end");
        assert!(!p.shared.take_ended(), "…and reported it exactly once");
    }

    #[test]
    #[cfg(unix)]
    fn a_player_that_fails_says_what_it_said_rather_than_playback_failed() {
        let sh = fake_player("bad", "echo 'Failed to open audio device' >&2; exit 1");
        let mut p = Player::new();
        p.start_with(&plan("mpv", "/a.flac", &Sink::Default), &sh.display().to_string());
        let mut got = None;
        for _ in 0..200 {
            if let Some(s) = p.shared.take_state() {
                if matches!(s, State::Failed(_)) { got = Some(s); break; }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        match got {
            Some(State::Failed(m)) => assert!(m.contains("audio device"), "{m}"),
            other => panic!("expected a failure with its reason, got {other:?}"),
        }
        assert!(!p.shared.take_ended(), "a failed track advanced the queue");
    }

    #[test]
    #[cfg(unix)]
    fn stopping_is_not_ending_so_pause_does_not_skip_a_track() {
        // The distinction the whole `ended` flag exists for.
        // `exec`, mirroring the real command. Without it the stand-in
        // stays alive as sleep's parent, the kill takes the wrapper and
        // not the sleeper, and the sleeper keeps the stdout pipe open —
        // which held the status reader for the full thirty seconds when I
        // first wrote this. That is exactly why `start_with` builds
        // `exec <player>`: the process we kill has to be the one holding
        // the pipe.
        let sh = fake_player("long", "exec sleep 30");
        let mut p = Player::new();
        p.start_with(&plan("mpv", "/a.flac", &Sink::Default), &sh.display().to_string());
        std::thread::sleep(std::time::Duration::from_millis(60));
        p.stop_now();
        assert!(!p.is_running(), "the player thread outlived the stop");
        assert!(!p.shared.take_ended(), "a stop was reported as a finished track");
    }
}

// ── the seam between the view and the host ──────────────────────────────────

/// What the view asks the host to do about audio.
///
/// Returned as data rather than performed, for the reason every seam in
/// this fleet is: the controller is constructed and tested with no process
/// table, and a `Player` on it would end that. It also keeps the decision
/// — which sink, which track — where the state is, and the mechanics where
/// the operating system is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Play this, here.
    Play { url: String, sink: Sink },
    /// Stop, and remember where. The position is the transport's business;
    /// this only asks for silence.
    Stop,
    /// The route changed: give up the old one's resources before the next
    /// play builds new ones.
    ReleaseRoute,
}

/// Turn a picker route into a playback sink.
///
/// The two types are deliberately separate — see [`Sink`] — and this is the
/// one place they meet, so a third spelling of "which speakers" cannot
/// appear.
pub fn sink_of(route: &crate::settings_page::Route) -> Sink {
    match route {
        crate::settings_page::Route::Bluetooth(mac) => Sink::One(mac.clone()),
        // A one-member multi-room route is a single speaker taking the
        // combine-sink's latency for nothing. `SinkPicker::can_combine`
        // already refuses to produce one; this is the second guard,
        // because the cost of being wrong is a permanent latency penalty
        // nobody would think to look for.
        crate::settings_page::Route::MultiRoom(macs) if macs.len() < 2 =>
            macs.first().cloned().map(Sink::One).unwrap_or(Sink::Default),
        crate::settings_page::Route::MultiRoom(macs) => Sink::Many(macs.clone()),
    }
}

#[cfg(test)]
mod seam_tests {
    use super::*;
    use crate::settings_page::Route;

    #[test]
    fn a_route_becomes_a_sink_in_exactly_one_place() {
        assert_eq!(sink_of(&Route::Bluetooth("AA".into())), Sink::One("AA".into()));
        assert_eq!(sink_of(&Route::MultiRoom(vec!["AA".into(), "BB".into()])),
                   Sink::Many(vec!["AA".into(), "BB".into()]));
    }

    #[test]
    fn a_multiroom_route_of_one_does_not_pay_for_a_combine_sink() {
        // The picker refuses to make one; this is the second guard, because
        // the cost is a latency penalty nobody would think to look for.
        assert_eq!(sink_of(&Route::MultiRoom(vec!["AA".into()])), Sink::One("AA".into()));
        assert_eq!(sink_of(&Route::MultiRoom(vec![])), Sink::Default);
    }
}
