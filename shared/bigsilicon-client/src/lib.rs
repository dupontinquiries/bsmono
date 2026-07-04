//! bigsilicon-client — one SDK for the BigSilicon daemons.
//!
//! An app connects to [`Voice`] (voiced), [`Share`] (sharemd), and [`Coord`] (coordd) in a
//! few lines. Every `connect` is **socket-first**: it returns `None` when the daemon isn't
//! running, which is the app's signal to fall back to its own in-process code. All inbound
//! traffic — acks, replies, and pushed events (intents, data events, directives) — arrives
//! on a channel via [`Conn::events`], so the app polls with `try_next` from its UI loop and
//! never blocks.
//!
//! ```no_run
//! use bigsilicon_client::{Voice};
//! use ipc_kit::Scope;
//! // try the shared voice daemon; if it's down, do our own thing
//! if let Some(mut v) = Voice::connect("beatspace") {
//!     v.register("play_track", Scope::Active, &["play", "{track}"]).ok();
//!     // …later, in the UI loop:
//!     while let Some(ev) = v.try_next() { /* handle intent event */ let _ = ev; }
//! } else {
//!     // fall back to in-process voice
//! }
//! ```

use coord_core::{Directive, Mode, Resource};
use ipc_kit::{socket_path, Message, Scope};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{channel, Receiver};

/// A socket-first connection to one daemon. All inbound messages arrive on `events`.
pub struct Conn {
    stream: UnixStream,
    events: Receiver<Message>,
}

impl Conn {
    /// Connect to the named daemon socket, spawning a reader thread that funnels inbound
    /// messages to a channel. `None` if nothing is listening (the fallback signal).
    pub fn connect(name: &str) -> Option<Conn> {
        let stream = UnixStream::connect(socket_path(name)).ok()?;
        let read = stream.try_clone().ok()?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(read);
            let mut line = String::new();
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Some(m) = Message::decode(&line) {
                            if tx.send(m).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });
        Some(Conn { stream, events: rx })
    }

    pub fn send(&mut self, m: &Message) -> std::io::Result<()> {
        self.stream.write_all(m.encode().as_bytes())?;
        self.stream.flush()
    }

    /// Non-blocking: the next inbound message, if any.
    pub fn try_next(&self) -> Option<Message> {
        self.events.try_recv().ok()
    }

    /// The raw inbound channel (acks, replies, events) for apps that want to select on it.
    pub fn events(&self) -> &Receiver<Message> {
        &self.events
    }
}

/// Voice daemon client. Register grammars; receive `event`/`focus`/`open` pushes.
pub struct Voice(Conn);

impl Voice {
    pub fn connect(app: &str) -> Option<Voice> {
        let mut c = Conn::connect("voiced")?;
        c.send(&Message::new("hello", vec![app.to_string()])).ok()?;
        Some(Voice(c))
    }
    /// Register an intent at a scope, optionally with a spoken grammar phrase
    /// (`&["play", "{track}"]`). Empty phrase = routing-only (no grammar rule).
    pub fn register(&mut self, event_type: &str, scope: Scope, phrase: &[&str]) -> std::io::Result<()> {
        let mut args = vec![event_type.to_string(), scope.as_str().to_string()];
        args.extend(phrase.iter().map(|s| s.to_string()));
        self.0.send(&Message::new("register", args))
    }
    /// Tell the daemon this app is now focused (usually the compositor does this).
    pub fn focus(&mut self, app: &str) -> std::io::Result<()> {
        self.0.send(&Message::new("focus", vec![app.to_string()]))
    }
    /// Feed a finalized transcript from the audio stage (mic→whisper) for matching+routing.
    pub fn transcript(&mut self, text: &str) -> std::io::Result<()> {
        self.0.send(&Message::new("transcript", text.split_whitespace().map(String::from).collect()))
    }
    pub fn try_next(&self) -> Option<Message> {
        self.0.try_next()
    }
    pub fn events(&self) -> &Receiver<Message> {
        self.0.events()
    }
}

/// Cross-app data client. Put/get/subscribe; receive `value`/`listing`/`event`.
pub struct Share(Conn);

impl Share {
    pub fn connect() -> Option<Share> {
        Conn::connect("share").map(Share)
    }
    /// Connect to a specific KV/pub-sub socket (e.g. "settings" for settingsd).
    pub fn connect_named(name: &str) -> Option<Share> {
        Conn::connect(name).map(Share)
    }
    pub fn put(&mut self, ns: &str, key: &str, val: &str) -> std::io::Result<()> {
        self.0.send(&Message::new("put", vec![ns.into(), key.into(), val.into()]))
    }
    pub fn get(&mut self, ns: &str, key: &str) -> std::io::Result<()> {
        self.0.send(&Message::new("get", vec![ns.into(), key.into()]))
    }
    pub fn subscribe(&mut self, ns: &str) -> std::io::Result<()> {
        self.0.send(&Message::new("sub", vec![ns.into()]))
    }
    pub fn list(&mut self, ns: &str) -> std::io::Result<()> {
        self.0.send(&Message::new("list", vec![ns.into()]))
    }
    pub fn try_next(&self) -> Option<Message> {
        self.0.try_next()
    }
    pub fn events(&self) -> &Receiver<Message> {
        self.0.events()
    }
}

/// Live cross-app settings over sharemd. Subscribes to the `settings` namespace, seeds from
/// a snapshot, and keeps a [`Prefs`](settings_kit::Prefs) in sync: when the central settings
/// app changes something, `poll()` picks it up and every app re-applies it — the gesture
/// feel, palette, touch targets, etc. change live. An app layers its own overrides on top.
pub struct Settings {
    share: Share,
    kv: std::collections::HashMap<String, String>,
    prefs: settings_kit::Prefs,
    overrides: Vec<(String, String)>,
}

impl Settings {
    /// Connect, subscribe to `settings`, and request the current snapshot. `None` if sharemd
    /// isn't running (the app falls back to compiled defaults).
    pub fn connect() -> Option<Settings> {
        let mut share = Share::connect_named(settings_kit::NAMESPACE)?;
        share.subscribe(settings_kit::NAMESPACE).ok()?;
        share.list(settings_kit::NAMESPACE).ok()?;
        Some(Settings { share, kv: std::collections::HashMap::new(), prefs: settings_kit::Prefs::default(), overrides: Vec::new() })
    }

    /// This app's own overrides (its uniqueness), applied on top of the central settings.
    pub fn with_overrides(mut self, overrides: Vec<(String, String)>) -> Settings {
        self.overrides = overrides;
        self.rebuild();
        self
    }

    /// Publish a central setting change (used by the settings app). Fans out to every other
    /// subscribed app via sharemd.
    pub fn set(&mut self, key: &str, value: &str) -> std::io::Result<()> {
        self.kv.insert(key.to_string(), value.to_string());
        self.rebuild();
        self.share.put(settings_kit::NAMESPACE, key, value)
    }

    /// Drain pending bus traffic and update prefs. Returns true if anything changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Some(m) = self.share.try_next() {
            match m.verb.as_str() {
                // a single key changed: event <ns> <key> [val]
                "event" if m.args.first().map(|s| s.as_str()) == Some(settings_kit::NAMESPACE) => {
                    if let Some(key) = m.args.get(1) {
                        let val = m.args.get(2).cloned().unwrap_or_default();
                        self.kv.insert(key.clone(), val);
                        changed = true;
                    }
                }
                // snapshot: listing <ns> k1 v1 k2 v2 …
                "listing" if m.args.first().map(|s| s.as_str()) == Some(settings_kit::NAMESPACE) => {
                    let mut i = 1;
                    while i + 1 < m.args.len() {
                        self.kv.insert(m.args[i].clone(), m.args[i + 1].clone());
                        i += 2;
                    }
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            self.rebuild();
        }
        changed
    }

    fn rebuild(&mut self) {
        let mut kv: Vec<(String, String)> = self.kv.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        kv.sort();
        let mut p = settings_kit::Prefs::from_kv(&kv);
        p.apply_kv(&self.overrides); // app overrides win
        self.prefs = p;
    }

    /// The current effective preferences (central + this app's overrides).
    pub fn prefs(&self) -> &settings_kit::Prefs {
        &self.prefs
    }
}

/// Coordination client. Claim/release shared resources; receive pushed `directive`s.
pub struct Coord(Conn);

impl Coord {
    pub fn connect(app: &str) -> Option<Coord> {
        let mut c = Conn::connect("coord")?;
        c.send(&Message::new("hello", vec![app.to_string()])).ok()?;
        Some(Coord(c))
    }
    pub fn claim(&mut self, res: Resource, priority: u8, mode: Mode) -> std::io::Result<()> {
        self.0.send(&Message::new("claim", vec![res.as_str().into(), priority.to_string(), mode.as_str().into()]))
    }
    pub fn release(&mut self, res: Resource) -> std::io::Result<()> {
        self.0.send(&Message::new("release", vec![res.as_str().into()]))
    }
    pub fn activity(&mut self, activity: &str) -> std::io::Result<()> {
        self.0.send(&Message::new("activity", vec![activity.to_string()]))
    }
    /// Poll the next pushed directive as a typed `(Resource, Directive)`, skipping non-directive
    /// traffic (acks/presence). Returns `None` if nothing is queued.
    pub fn next_directive(&self) -> Option<(Resource, Directive)> {
        while let Some(m) = self.0.try_next() {
            if m.verb == "directive" {
                if let (Some(res), Some(dir)) = (m.args.first().and_then(|s| Resource::parse(s)), m.args.get(1)) {
                    let d = match dir.as_str() {
                        "own" => Directive::Own,
                        "yield" => Directive::Yield,
                        "duck" => Directive::Duck,
                        "share" => Directive::Share,
                        _ => Directive::Idle,
                    };
                    return Some((res, d));
                }
            }
        }
        None
    }
    pub fn try_next(&self) -> Option<Message> {
        self.0.try_next()
    }
}

// ── cross-app actions: one app asking another to do something ────────────────

/// A small deep-link vocabulary so apps can help the user through each other. An app serves
/// these on its own ipc-kit socket (named after the app); any other app sends one via
/// [`send_action`]. This is how gitmind says "oh, you need to run this" and hands the command
/// straight to the terminal.
pub mod action {
    /// Open a fresh tab/view.
    pub const OPEN_TAB: &str = "open_tab";
    /// Place a command on the prompt but **do not run it** — the user reviews and presses
    /// enter. The safe default for "you need to run this".
    pub const STAGE_COMMAND: &str = "stage_command";
    /// Place a command and run it (use sparingly; prefer staging for anything mutating).
    pub const RUN_COMMAND: &str = "run_command";
    /// Open a document/path.
    pub const OPEN_DOC: &str = "open_doc";
    /// Open a URL.
    pub const OPEN_URL: &str = "open_url";
}

/// Send a one-shot action to another app's socket. **Socket-first**: returns `false` if the
/// target app isn't running, so the caller falls back (e.g. gitmind just prints the command).
/// Fire-and-forget — no reply is awaited.
pub fn send_action(target_socket: &str, verb: &str, args: &[&str]) -> bool {
    use std::io::Write;
    match UnixStream::connect(socket_path(target_socket)) {
        Ok(mut s) => {
            let m = Message::new(verb, args.iter().map(|x| x.to_string()).collect());
            s.write_all(m.encode().as_bytes()).and_then(|_| s.flush()).is_ok()
        }
        Err(_) => false,
    }
}

/// Client for driving the terminal app (voiceterm) from another app.
pub struct Terminal;

impl Terminal {
    /// The socket voiceterm serves its actions on.
    pub fn socket() -> &'static str {
        "voiceterm"
    }
    /// Open a new terminal tab and place `cmd` on the prompt **without running it** — the
    /// user reviews and hits enter. Returns `false` if voiceterm isn't running (fall back to
    /// showing the command). This is the "gitmind → run this for me" hand-off.
    pub fn stage(cmd: &str) -> bool {
        send_action(Self::socket(), action::STAGE_COMMAND, &[cmd])
    }
    /// Open a new tab and run `cmd` immediately (prefer [`stage`] for anything that changes
    /// the system).
    pub fn run(cmd: &str) -> bool {
        send_action(Self::socket(), action::RUN_COMMAND, &[cmd])
    }
    /// Just open a new tab.
    pub fn open_tab() -> bool {
        send_action(Self::socket(), action::OPEN_TAB, &[])
    }
}

/// Server-side: a decoded terminal action, for the app *receiving* the request (voiceterm).
/// voiceterm's ipc-kit service calls this on each inbound message and acts on the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalAction {
    OpenTab,
    /// Prefill this command (do not run).
    Stage(String),
    /// Run this command.
    Run(String),
}

/// Decode an inbound action `Message` into a [`TerminalAction`] (or `None` if it isn't one).
pub fn parse_terminal_action(m: &Message) -> Option<TerminalAction> {
    match m.verb.as_str() {
        action::OPEN_TAB => Some(TerminalAction::OpenTab),
        action::STAGE_COMMAND => m.args.first().map(|c| TerminalAction::Stage(c.clone())),
        action::RUN_COMMAND => m.args.first().map(|c| TerminalAction::Run(c.clone())),
        _ => None,
    }
}

/// Connect to all three daemons at once; each field is `None` if that daemon is down.
pub struct BigSilicon {
    pub voice: Option<Voice>,
    pub share: Option<Share>,
    pub coord: Option<Coord>,
}

impl BigSilicon {
    pub fn connect(app: &str) -> BigSilicon {
        BigSilicon { voice: Voice::connect(app), share: Share::connect(), coord: Coord::connect(app) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipc_kit::{bind, serve_hub, ConnId, Service};
    use std::time::Duration;

    #[test]
    fn connect_returns_none_without_daemon() {
        assert!(Voice::connect("no-voiced-xyz").is_none());
        assert!(Share::connect().is_none() || true); // share may exist in env; just ensure no panic
        assert!(Coord::connect("no-coord-xyz").is_none());
    }

    // a tiny stub daemon that acks hello/register and, on "fire", pushes a directive
    struct Stub;
    impl Service for Stub {
        fn handle(&mut self, from: ConnId, msg: Message) -> Vec<(ConnId, Message)> {
            match msg.verb.as_str() {
                "hello" | "register" | "claim" => vec![(from, Message::verb("ok"))],
                "fire" => vec![(from, Message::new("directive", vec!["call_audio".into(), "duck".into()]))],
                _ => vec![],
            }
        }
    }

    #[test]
    fn settings_sync_live_across_apps() {
        use std::collections::HashMap;
        // a tiny sharemd stand-in: kv + per-namespace subscribers, fans out on put
        #[derive(Default)]
        struct SettingsHub {
            kv: HashMap<String, String>,
            subs: HashMap<String, Vec<ConnId>>,
        }
        impl Service for SettingsHub {
            fn handle(&mut self, from: ConnId, msg: Message) -> Vec<(ConnId, Message)> {
                let a = |i: usize| msg.args.get(i).cloned().unwrap_or_default();
                match msg.verb.as_str() {
                    "sub" => {
                        self.subs.entry(a(0)).or_default().push(from);
                        vec![(from, Message::verb("ok"))]
                    }
                    "put" => {
                        let (ns, key, val) = (a(0), a(1), a(2));
                        self.kv.insert(format!("{ns}/{key}"), val.clone());
                        let mut out = vec![(from, Message::verb("ok"))];
                        for &s in self.subs.get(&ns).into_iter().flatten() {
                            if s != from {
                                out.push((s, Message::new("event", vec![ns.clone(), key.clone(), val.clone()])));
                            }
                        }
                        out
                    }
                    "list" => {
                        let ns = a(0);
                        let mut args = vec![ns.clone()];
                        for (k, v) in &self.kv {
                            if let Some(key) = k.strip_prefix(&format!("{ns}/")) {
                                args.push(key.to_string());
                                args.push(v.clone());
                            }
                        }
                        vec![(from, Message::new("listing", args))]
                    }
                    _ => vec![],
                }
            }
        }

        let listener = bind(settings_kit::NAMESPACE).expect("bind");
        std::thread::spawn(move || serve_hub(listener, SettingsHub::default()));
        std::thread::sleep(std::time::Duration::from_millis(50));

        // the central settings app and a regular app
        let mut central = Settings::connect().expect("central");
        let mut app = Settings::connect().expect("app");
        std::thread::sleep(std::time::Duration::from_millis(30));
        central.poll();
        app.poll();
        assert_eq!(app.prefs().gesture.sensitivity, 1.0, "starts at default");

        // update the touch gesture feel centrally …
        central.set("gesture.sensitivity", "2.0").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(40));
        // … the other app picks it up live
        assert!(app.poll());
        assert_eq!(app.prefs().gesture.sensitivity, 2.0, "app updated from the central change");
    }

    #[test]
    fn terminal_stage_delivers_and_decodes() {
        use std::sync::{Arc, Mutex};
        // stub voiceterm: records the terminal actions it receives
        #[derive(Clone)]
        struct TermStub(Arc<Mutex<Vec<TerminalAction>>>);
        impl Service for TermStub {
            fn handle(&mut self, _from: ConnId, msg: Message) -> Vec<(ConnId, Message)> {
                if let Some(a) = parse_terminal_action(&msg) {
                    self.0.lock().unwrap().push(a);
                }
                vec![]
            }
        }
        // socket-first: nothing running → stage returns false (caller falls back)
        assert!(!Terminal::stage("echo before-server"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let listener = bind(Terminal::socket()).expect("bind voiceterm");
        let stub = TermStub(seen.clone());
        std::thread::spawn(move || serve_hub(listener, stub));
        std::thread::sleep(std::time::Duration::from_millis(50));

        // gitmind hands the install command to the terminal
        assert!(Terminal::stage("rustup target add aarch64-unknown-linux-musl"));
        std::thread::sleep(std::time::Duration::from_millis(40));

        let got = seen.lock().unwrap().clone();
        assert_eq!(got, vec![TerminalAction::Stage("rustup target add aarch64-unknown-linux-musl".to_string())]);
    }

    #[test]
    fn coord_client_receives_typed_directive() {
        // bind the real coord socket name so Coord::connect finds it
        let listener = bind("coord").expect("bind");
        std::thread::spawn(move || serve_hub(listener, Stub));
        std::thread::sleep(Duration::from_millis(50));

        let mut coord = Coord::connect("snapclone").expect("connect");
        // drain the hello ack
        std::thread::sleep(Duration::from_millis(20));
        while coord.try_next().is_some() {}
        // trigger a pushed directive
        coord.0.send(&Message::verb("fire")).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let got = coord.next_directive();
        assert_eq!(got, Some((Resource::CallAudio, Directive::Duck)));
    }
}
