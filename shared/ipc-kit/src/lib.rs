//! ipc-kit — local app↔daemon IPC over a Unix domain socket.
//!
//! Two ideas the portfolio needs repeatedly:
//!
//! 1. **Socket-first fallback.** An app should prefer a shared system daemon (one voice
//!    pipeline, one data/mailbox layer) but keep working if it isn't running.
//!    [`Client::try_connect`] returns `None` when no daemon is listening, which is the
//!    app's cue to fall back to its own in-process code — "try the socket first, then do
//!    it yourself."
//!
//! 2. **Per-event listen scope.** An app registers interest in specific event types and,
//!    per type, whether it wants them [`Scope::Always`] (even backgrounded — wake words,
//!    incoming mail) or only [`Scope::Active`] (when focused — most commands). The daemon
//!    honours the scope when it dispatches.
//!
//! Message framing reuses `net_kit::wire` (percent-escaped tab-separated fields), so the
//! wire format is the same one the LAN transport already uses. Zero non-kit dependencies;
//! std sockets + threads, so it cross-compiles to aarch64/musl.

use net_kit::wire::{decode_fields, encode_fields};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

/// Where a named daemon's socket lives: `$XDG_RUNTIME_DIR/gitmind/<name>.sock`, falling
/// back to `/tmp/gitmind-<name>.sock` when there's no runtime dir.
pub fn socket_path(name: &str) -> PathBuf {
    if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
        if !rt.is_empty() {
            return PathBuf::from(rt).join("gitmind").join(format!("{name}.sock"));
        }
    }
    PathBuf::from(format!("/tmp/gitmind-{name}.sock"))
}

/// When should the daemon deliver an event type to a registered app?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Deliver even when the app is not the active/foreground one.
    Always,
    /// Deliver only while the app is active/foreground.
    Active,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Always => "always",
            Scope::Active => "active",
        }
    }
    pub fn parse(s: &str) -> Scope {
        match s {
            "always" => Scope::Always,
            _ => Scope::Active,
        }
    }
}

/// A framed message: a `verb` plus string arguments. Encodes to one line via net-kit's
/// field codec, so embedded tabs/newlines are escaped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub verb: String,
    pub args: Vec<String>,
}

impl Message {
    pub fn new(verb: impl Into<String>, args: Vec<String>) -> Self {
        Message { verb: verb.into(), args }
    }
    pub fn verb(verb: impl Into<String>) -> Self {
        Message { verb: verb.into(), args: vec![] }
    }
    pub fn encode(&self) -> String {
        let mut fields: Vec<&str> = Vec::with_capacity(self.args.len() + 1);
        fields.push(&self.verb);
        for a in &self.args {
            fields.push(a);
        }
        let mut line = encode_fields(&fields);
        line.push('\n');
        line
    }
    pub fn decode(line: &str) -> Option<Message> {
        let mut fields = decode_fields(line);
        if fields.is_empty() {
            return None;
        }
        let verb = fields.remove(0);
        Some(Message { verb, args: fields })
    }
    /// Build the standard registration message an app sends to subscribe to an event type.
    pub fn register(event_type: &str, scope: Scope) -> Message {
        Message::new("register", vec![event_type.to_string(), scope.as_str().to_string()])
    }
}

/// A connected client. `None` from [`Client::try_connect`] means no daemon — fall back.
pub struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    /// Connect if the daemon is up; `None` if not (the socket-first fallback signal).
    pub fn try_connect(name: &str) -> Option<Client> {
        Self::connect(name).ok()
    }
    pub fn connect(name: &str) -> std::io::Result<Client> {
        let stream = UnixStream::connect(socket_path(name))?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Client { stream, reader })
    }
    pub fn send(&mut self, msg: &Message) -> std::io::Result<()> {
        self.stream.write_all(msg.encode().as_bytes())?;
        self.stream.flush()
    }
    /// Read one message (blocking). `None` on EOF/error.
    pub fn recv(&mut self) -> Option<Message> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Message::decode(&line),
        }
    }
    /// Send then read one reply.
    pub fn request(&mut self, msg: &Message) -> Option<Message> {
        self.send(msg).ok()?;
        self.recv()
    }
    /// Register interest in an event type at a scope (see [`Scope`]).
    pub fn register(&mut self, event_type: &str, scope: Scope) -> std::io::Result<()> {
        self.send(&Message::register(event_type, scope))
    }
}

/// A per-connection handle the server handler uses to reply/push.
pub struct Conn {
    stream: UnixStream,
}
impl Conn {
    pub fn send(&mut self, msg: &Message) -> std::io::Result<()> {
        self.stream.write_all(msg.encode().as_bytes())?;
        self.stream.flush()
    }
}

/// Bind the daemon socket (creating the parent dir, clearing a stale socket).
pub fn bind(name: &str) -> std::io::Result<UnixListener> {
    let path = socket_path(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(&path); // clear stale socket
    UnixListener::bind(path)
}

/// Serve: for each connection, spawn a thread that reads messages line-by-line and calls
/// `handler(msg, &mut Conn)`. The handler owns dispatch/registration semantics — ipc-kit
/// only carries the frames. Blocks forever.
pub fn serve(listener: UnixListener, handler: impl Fn(Message, &mut Conn) + Send + Sync + 'static) {
    let handler = std::sync::Arc::new(handler);
    for incoming in listener.incoming() {
        let Ok(stream) = incoming else { continue };
        let h = handler.clone();
        std::thread::spawn(move || {
            let read_half = match stream.try_clone() {
                Ok(s) => s,
                Err(_) => return,
            };
            let mut conn = Conn { stream };
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Some(msg) = Message::decode(&line) {
                            h(msg, &mut conn);
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn socket_first_fallback_returns_none_without_daemon() {
        // no daemon bound for this name → app should fall back to its own code
        assert!(Client::try_connect("definitely-not-running-xyz").is_none());
    }

    #[test]
    fn message_roundtrips_through_net_kit_codec() {
        let m = Message::new("intent", vec!["switch_app".into(), "gitmind".into(), "tab\twith\ttabs".into()]);
        let line = m.encode();
        assert!(line.ends_with('\n'));
        let back = Message::decode(&line).unwrap();
        assert_eq!(back, m, "tabs in args survive the escape codec");
    }

    #[test]
    fn scope_roundtrip() {
        assert_eq!(Scope::parse(Scope::Always.as_str()), Scope::Always);
        assert_eq!(Scope::parse(Scope::Active.as_str()), Scope::Active);
        let r = Message::register("wake_word", Scope::Always);
        assert_eq!(r.verb, "register");
        assert_eq!(r.args, vec!["wake_word".to_string(), "always".to_string()]);
    }

    #[test]
    fn real_socket_register_and_dispatch() {
        // unique name so the test is isolated
        let name = format!("ipckit-test-{}", std::process::id());
        let listener = bind(&name).expect("bind");
        let (tx, rx) = mpsc::channel();
        // daemon: echo registrations back as an ack, record the event type
        std::thread::spawn(move || {
            serve(listener, move |msg, conn| {
                if msg.verb == "register" {
                    let _ = tx.send(msg.args.clone());
                    let _ = conn.send(&Message::new("ack", msg.args.clone()));
                }
            });
        });
        // give the listener a moment
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut client = Client::connect(&name).expect("connect");
        client.register("mail_incoming", Scope::Always).expect("register");
        let ack = client.recv().expect("ack");
        assert_eq!(ack.verb, "ack");
        let recorded = rx.recv_timeout(std::time::Duration::from_secs(2)).expect("server saw registration");
        assert_eq!(recorded, vec!["mail_incoming".to_string(), "always".to_string()]);
    }
}
