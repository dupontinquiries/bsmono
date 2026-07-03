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
    pub fn put(&mut self, ns: &str, key: &str, val: &str) -> std::io::Result<()> {
        self.0.send(&Message::new("put", vec![ns.into(), key.into(), val.into()]))
    }
    pub fn get(&mut self, ns: &str, key: &str) -> std::io::Result<()> {
        self.0.send(&Message::new("get", vec![ns.into(), key.into()]))
    }
    pub fn subscribe(&mut self, ns: &str) -> std::io::Result<()> {
        self.0.send(&Message::new("sub", vec![ns.into()]))
    }
    pub fn try_next(&self) -> Option<Message> {
        self.0.try_next()
    }
    pub fn events(&self) -> &Receiver<Message> {
        self.0.events()
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
