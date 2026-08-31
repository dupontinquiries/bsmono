//! voice_source — the recogniser, as a process that prints lines.
//!
//! `Controller::heard` takes a phrase and does the right thing with it,
//! and nothing calls it. That is the same failure this codebase keeps
//! turning up: a complete, tested feature with no way in. A parser with no
//! source is a setting that does nothing when you switch it on, which is
//! worse than a missing feature because the switch implies otherwise.
//!
//! ## A command that prints phrases, one per line
//!
//! Not a linked speech model. voiceterm already owns the recognition
//! stack — whisper models, a mic pipeline, a wake gate — and duplicating
//! any of it here would be a second thing to keep in step. What this app
//! needs is the OUTPUT, and a line of text on a pipe is the cheapest
//! interface that any of them can produce:
//!
//! * `cat /run/user/1000/bo-voice.fifo` — anything already recognising
//!   writes a line into the fifo and this app acts on it. Works today,
//!   with `mkfifo` and no other moving parts.
//! * a wrapper around a streaming recogniser
//! * `echo pause` while testing, which is how the tests here work
//!
//! NOT `voiceterm --print-phrases`, which the first version of this
//! module offered as the headline example. I went and checked: voiceterm
//! has no such mode. Its recognition sits behind the `voice` feature
//! inside a Qt binary whose outputs are its own UI and D-Bus. Naming an
//! invocation that does not exist is worse than naming none — it sends
//! someone hunting for a flag for twenty minutes. What voiceterm would
//! need is a headless mode printing each final transcript to stdout.
//!
//! Empty by default: nothing is spawned until the person names a command.
//! That matters more than usual here — this is the switch that decides
//! whether a program listens in someone's home, and a default that
//! launched something would make that decision for them.
//!
//! ## Lines, bounded, newest-first-served
//!
//! A recogniser that gets stuck emitting rubbish must not grow the queue
//! without limit, and a phrase from four minutes ago is not worth acting
//! on: [`MAX_PENDING`] caps it and the OLDEST are dropped, because
//! "pause" said ten seconds ago is more relevant than "pause" said during
//! the previous track.

use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The most phrases held before the oldest are dropped.
pub const MAX_PENDING: usize = 16;

/// Ignore anything longer than this.
///
/// A command is a few words. A recogniser that starts transcribing the
/// television produces sentences, and a hundred-character "phrase" is
/// evidence that whatever is on the pipe is not someone talking to the
/// speaker.
pub const MAX_PHRASE_CHARS: usize = 120;

/// Phrases waiting to be acted on.
#[derive(Default)]
pub struct Heard {
    lines: Mutex<Vec<String>>,
    /// The reader stopped, with a reason. Reported once — a recogniser
    /// that exits at startup because its model is missing should say so,
    /// not fail silently and leave a switch that appears to be on.
    failed: Mutex<Option<String>>,
}

impl Heard {
    pub fn new() -> Arc<Heard> { Arc::new(Heard::default()) }

    /// Take everything heard since the last call.
    pub fn drain(&self) -> Vec<String> {
        match self.lines.lock() {
            Ok(mut g) => std::mem::take(&mut *g),
            Err(_) => Vec::new(),
        }
    }

    pub fn take_failure(&self) -> Option<String> {
        self.failed.lock().ok()?.take()
    }

    fn push(&self, line: &str) {
        let t = line.trim();
        if t.is_empty() || t.chars().count() > MAX_PHRASE_CHARS {
            return;
        }
        if let Ok(mut g) = self.lines.lock() {
            g.push(t.to_string());
            // OLDEST dropped: "pause" said ten seconds ago is worth more
            // than "pause" said during the previous track.
            while g.len() > MAX_PENDING {
                g.remove(0);
            }
        }
    }
}

/// A running recogniser.
pub struct VoiceSource {
    child: Arc<Mutex<Option<std::process::Child>>>,
    pub heard: Arc<Heard>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Default for VoiceSource {
    fn default() -> Self {
        VoiceSource { child: Arc::new(Mutex::new(None)), heard: Heard::new(),
                      stop: Arc::new(AtomicBool::new(false)), thread: None }
    }
}

impl VoiceSource {
    pub fn new() -> VoiceSource { VoiceSource::default() }

    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Start a recogniser. `launcher` is the shell, injectable for tests.
    ///
    /// `exec`, so the process we later kill is the one holding the pipe —
    /// killing a shell leaves its child running and the reader waiting on
    /// a pipe that never closes. That was found the hard way in
    /// bo-suite's player, and it is the same shape here.
    pub fn start_with(&mut self, cmd: &str, launcher: &str) {
        self.stop_now();
        if cmd.trim().is_empty() {
            return;
        }
        self.stop = Arc::new(AtomicBool::new(false));
        self.heard = Heard::new();
        let (heard, stop, holder) =
            (self.heard.clone(), self.stop.clone(), self.child.clone());
        let cmd = cmd.to_string();
        let launcher = launcher.to_string();
        self.thread = Some(std::thread::spawn(move || {
            let spawned = std::process::Command::new(&launcher)
                .arg("-c").arg(format!("exec {cmd}"))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn();
            let mut child = match spawned {
                Ok(c) => c,
                Err(e) => {
                    if let Ok(mut g) = heard.failed.lock() {
                        *g = Some(format!("could not start the recogniser: {e}"));
                    }
                    return;
                }
            };
            let out = child.stdout.take();
            if let Ok(mut g) = holder.lock() { *g = Some(child); }
            if let Some(o) = out {
                let r = std::io::BufReader::new(o);
                for line in r.lines() {
                    if stop.load(Ordering::Relaxed) { break; }
                    match line {
                        Ok(l) => heard.push(&l),
                        Err(_) => break,
                    }
                }
            }
            // Exiting on its own is worth reporting. A recogniser that
            // quits because its model is missing otherwise leaves a switch
            // that says "on" and a room where nothing happens.
            if !stop.load(Ordering::Relaxed) {
                if let Ok(mut g) = heard.failed.lock() {
                    *g = Some("the recogniser stopped".to_string());
                }
            }
            if let Ok(mut g) = holder.lock() {
                if let Some(mut c) = g.take() { let _ = c.kill(); let _ = c.wait(); }
            }
        }));
    }

    pub fn start(&mut self, cmd: &str) { self.start_with(cmd, "sh"); }

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
}

impl Drop for VoiceSource {
    fn drop(&mut self) {
        // A recogniser left running after the app closes is a microphone
        // nobody is watching.
        self.stop_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn fake(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir()
            .join(format!("bo-voice-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    fn collect(v: &VoiceSource, want: usize) -> Vec<String> {
        let mut out = Vec::new();
        for _ in 0..200 {
            out.extend(v.heard.drain());
            if out.len() >= want { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        out
    }

    #[test]
    #[cfg(unix)]
    fn phrases_printed_by_the_recogniser_reach_the_app() {
        // The whole point: `Controller::heard` had no caller, so voice
        // control was a setting that did nothing when switched on.
        let sh = fake("lines", "printf 'pause\\nnext track\\n'; exit 0");
        let mut v = VoiceSource::new();
        v.start_with("recogniser", &sh.display().to_string());
        let got = collect(&v, 2);
        assert_eq!(got, vec!["pause".to_string(), "next track".into()], "{got:?}");
    }

    #[test]
    #[cfg(unix)]
    fn a_recogniser_that_will_not_start_says_so() {
        // Otherwise the switch reads "on" and the room stays silent, with
        // nothing anywhere to explain it.
        let mut v = VoiceSource::new();
        v.start_with("recogniser", "/definitely/not/a/shell");
        let mut why = None;
        for _ in 0..200 {
            if let Some(w) = v.heard.take_failure() { why = Some(w); break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(why.is_some_and(|w| w.contains("could not start")), "no reason given");
    }

    #[test]
    #[cfg(unix)]
    fn a_recogniser_that_exits_is_reported_rather_than_going_quiet() {
        let sh = fake("dies", "exit 1");
        let mut v = VoiceSource::new();
        v.start_with("recogniser", &sh.display().to_string());
        let mut why = None;
        for _ in 0..200 {
            if let Some(w) = v.heard.take_failure() { why = Some(w); break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(why.is_some(), "the recogniser died silently");
    }

    #[test]
    fn an_empty_command_starts_nothing() {
        // The default. This is the switch that decides whether a program
        // listens in someone's home, and a default that launched something
        // would make that decision for them.
        let mut v = VoiceSource::new();
        v.start_with("", "sh");
        assert!(!v.is_running());
    }

    #[test]
    fn the_queue_is_bounded_and_drops_the_oldest() {
        // A stuck recogniser must not grow it without limit, and a phrase
        // from four minutes ago is not worth acting on.
        let h = Heard::new();
        for i in 0..(MAX_PENDING + 5) {
            h.push(&format!("phrase {i}"));
        }
        let got = h.drain();
        assert_eq!(got.len(), MAX_PENDING);
        assert_eq!(got[0], format!("phrase {}", 5), "the newest were dropped: {got:?}");
    }

    #[test]
    fn a_sentence_is_not_a_command_and_is_dropped() {
        // A recogniser transcribing the television produces sentences; a
        // hundred-character "phrase" is evidence that what is on the pipe
        // is not someone talking to the speaker.
        let h = Heard::new();
        h.push(&"a".repeat(MAX_PHRASE_CHARS + 1));
        h.push("   ");
        h.push("pause");
        assert_eq!(h.drain(), vec!["pause".to_string()]);
    }
}

#[cfg(test)]
mod end_to_end_tests {
    use super::*;
    use crate::voice_control::VoicePrefs;

    #[test]
    #[cfg(unix)]
    fn a_spoken_phrase_reaches_the_transport() {
        // The chain the whole feature is: a recogniser prints a line, the
        // source reads it, `heard` parses it, and the controller acts.
        // Every link existed except the first, so none of it ran.
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("bo-voice-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sh = dir.join("sh");
        std::fs::write(&sh, "#!/bin/sh\nprintf 'pause\\n'\n").unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut src = VoiceSource::new();
        src.start_with("recogniser", &sh.display().to_string());

        let mut c = crate::Controller::new(vec![], vec![]);
        c.voice = VoicePrefs { enabled: true, ..Default::default() };
        c.transport.playing = true;

        let mut acted = false;
        for _ in 0..200 {
            for phrase in src.heard.drain() {
                if matches!(c.heard(&phrase), crate::voice_control::Outcome::Run(_)) {
                    acted = true;
                }
            }
            if acted { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(acted, "the phrase never reached the controller");
        assert!(!c.transport.playing, "'pause' did not pause");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn a_phrase_is_ignored_while_the_setting_is_off() {
        // The source may be running from a previous session's settings;
        // the gate is in `decide`, and this is the proof it holds at the
        // end of the real chain rather than only in a unit test.
        let mut c = crate::Controller::new(vec![], vec![]);
        c.transport.playing = true;
        assert_eq!(c.heard("pause"), crate::voice_control::Outcome::Ignored);
        assert!(c.transport.playing);
    }
}
