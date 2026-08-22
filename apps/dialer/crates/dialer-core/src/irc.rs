//! IRC backend (feature `irc`) — a fourth transport on the same [`Link`]
//! protocol, so a channel is a thread in the same Messages UI as SMS and
//! Matrix.
//!
//! The README's mmsd-tng note says a fourth backend "would be a fourth
//! backend on the same Link, emitting the same `MessageIn`/`MessageSent`".
//! This is that, for IRC: no new protocol, no UI change, no `Command` variant
//! added. A channel is a peer spelled `irc:<network>/<channel>`, the same way
//! Matrix rooms are `matrix/!room:server`.
//!
//! ## Scope, stated plainly
//!
//! * **Messaging only.** IRC has no calls, so `Dial`/`Accept`/`Hangup`/`Dtmf`
//!   are ignored rather than faked. The backend reports `ModemUp` with an
//!   `irc:` label purely so the shell's status bar has something true to say.
//! * **No handoff payloads over IRC.** A [`Payload`] other than `Text` is
//!   flattened to its display text before sending. IRC has no custom event
//!   type, and encoding `run:` commands into a public channel where anyone
//!   can speak is a threat model the guard should never have to meet. Inbound
//!   IRC text is likewise always parsed as [`Payload::Text`] — never through
//!   `message::parse_sms`, so a channel member typing `run: rm -rf ~` arrives
//!   as the plain text it is.
//! * **Channel logging is irc-kit's**, not the conversation store's. The JSONL
//!   channel log is the searchable archive; the dialer thread list gets the
//!   same lines as messages so they show up in the UI, but the two stores
//!   have different lifetimes on purpose.
//!
//! ## TLS
//!
//! `irc-kit` is generic over `Read + Write` and carries no crypto. This module
//! supplies the stream: rustls when the port is TLS (6697 and friends), a raw
//! `TcpStream` otherwise. rustls is already in the monorepo's dependency tree
//! (snapclone's camera-bridge), so this adds no new vendor.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use irc_kit::log;
use irc_kit::session::{Config as IrcConfig, Event as IrcEvent, Session, Stage};
use irc_kit::transport::Client;

use crate::link::{Command, Event, Link};
use crate::message::Payload;

/// Everything needed to connect. Persisted by the shell next to `matrix.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// Short name used in peer ids and log filenames: "oftc".
    pub network: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub nick: String,
    pub alt_nicks: Vec<String>,
    pub user: String,
    pub realname: String,
    /// Server password / ZNC-style `user:pass`. Empty = none.
    pub pass: String,
    pub channels: Vec<String>,
}

impl Default for Account {
    fn default() -> Account {
        Account {
            network: "oftc".into(),
            host: "irc.oftc.net".into(),
            port: 6697,
            tls: true,
            nick: String::new(),
            alt_nicks: Vec::new(),
            user: "dialer".into(),
            realname: "dialer".into(),
            pass: String::new(),
            channels: Vec::new(),
        }
    }
}

/// `irc:oftc/#asahi-dev` — the peer spelling used in the thread list.
pub fn peer_id(network: &str, channel: &str) -> String {
    format!("irc:{network}/{channel}")
}

/// Split a peer id back into (network, channel). `None` if it is not ours,
/// which is how the shell routes a send to the right backend.
pub fn parse_peer(peer: &str) -> Option<(String, String)> {
    let rest = peer.strip_prefix("irc:")?;
    let slash = rest.find('/')?;
    let (net, chan) = (&rest[..slash], &rest[slash + 1..]);
    if net.is_empty() || chan.is_empty() {
        return None;
    }
    Some((net.to_string(), chan.to_string()))
}

/// Flatten a payload to the one thing IRC can carry. Non-text payloads keep
/// their human-readable rendering and lose the machine-readable framing —
/// deliberately, so nothing arriving from a public channel can be read as a
/// handoff instruction.
pub fn flatten(payload: &Payload) -> String {
    match payload {
        Payload::Text { body } => body.clone(),
        Payload::CommandSuggestion { command, note, .. } => match note {
            Some(n) if !n.is_empty() => format!("[suggestion] {command} — {n}"),
            _ => format!("[suggestion] {command}"),
        },
        Payload::FileOffer { name, uri, .. } => format!("[file] {name} <{uri}>"),
        Payload::FileView { path } => format!("[open] {path}"),
    }
}

/// The stream, whichever kind it is. A tiny enum beats a boxed trait object
/// here: two variants, no dispatch cost, and `try_clone` stays concrete.
enum Stream {
    Plain(TcpStream),
    #[cfg(feature = "irc-tls")]
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            #[cfg(feature = "irc-tls")]
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            #[cfg(feature = "irc-tls")]
            Stream::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            #[cfg(feature = "irc-tls")]
            Stream::Tls(s) => s.flush(),
        }
    }
}

/// A read timeout, not a blocking read: the loop must also drain commands and
/// tick the keepalive, and merging those with a blocking socket needs either
/// a second thread or a poll. A 500 ms timeout is the same choice `mm.rs`
/// made for its ModemManager poll, for the same reason — one readable loop.
const READ_TIMEOUT: Duration = Duration::from_millis(500);

fn connect(acct: &Account) -> Result<Stream, String> {
    let addr = format!("{}:{}", acct.host, acct.port);
    let tcp = TcpStream::connect(&addr).map_err(|e| format!("connect {addr}: {e}"))?;
    tcp.set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|e| format!("set read timeout: {e}"))?;

    if !acct.tls {
        return Ok(Stream::Plain(tcp));
    }

    #[cfg(feature = "irc-tls")]
    {
        use std::sync::Arc;
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let cfg = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server = rustls::pki_types::ServerName::try_from(acct.host.clone())
            .map_err(|e| format!("bad server name {}: {e}", acct.host))?;
        let conn = rustls::ClientConnection::new(Arc::new(cfg), server)
            .map_err(|e| format!("tls setup: {e}"))?;
        return Ok(Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp))));
    }

    #[cfg(not(feature = "irc-tls"))]
    Err(format!(
        "{} wants TLS but this build has no `irc-tls` feature; \
         rebuild with it or set tls=false for a plaintext port",
        acct.host
    ))
}

/// Spawn the backend. Returns immediately with the Link; connection failures
/// arrive as `Event::ModemDown` on the channel, matching how `mm.rs` reports
/// a missing modem rather than failing the constructor.
pub fn spawn(acct: Account) -> Link {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let (ev_tx, ev_rx) = mpsc::channel::<Event>();

    std::thread::spawn(move || run(acct, cmd_rx, ev_tx));

    Link { tx: cmd_tx, rx: ev_rx, kind: "irc" }
}

fn run(acct: Account, cmd_rx: Receiver<Command>, ev_tx: Sender<Event>) {
    let stream = match connect(&acct) {
        Ok(s) => s,
        Err(e) => {
            let _ = ev_tx.send(Event::ModemDown { detail: e });
            return;
        }
    };
    let session = Session::new(IrcConfig {
        nick: acct.nick.clone(),
        alt_nicks: acct.alt_nicks.clone(),
        user: acct.user.clone(),
        realname: acct.realname.clone(),
        pass: acct.pass.clone(),
        channels: acct.channels.clone(),
    });

    let mut client = Client::new(session, stream, &acct.network);
    client.log_dir = Some(log::default_dir());
    client.session.start();
    if let Err(e) = client.flush_outbound() {
        let _ = ev_tx.send(Event::ModemDown { detail: format!("write during registration: {e}") });
        return;
    }

    let _ = ev_tx.send(Event::ModemUp { label: format!("irc:{}", acct.network) });

    let mut buf = [0u8; 8192];
    let mut last_tick = Instant::now();

    loop {
        // 1. Drain UI commands.
        loop {
            match cmd_rx.try_recv() {
                Ok(Command::SendMessage { peer, payload }) => {
                    match parse_peer(&peer) {
                        Some((net, chan)) if net == acct.network => {
                            let text = flatten(&payload);
                            if let Err(e) = client.say(&chan, &text) {
                                let _ = ev_tx.send(Event::Error(format!("irc send: {e}")));
                            } else {
                                let _ = ev_tx.send(Event::MessageSent { peer: peer.clone() });
                            }
                        }
                        // Not addressed to us. Silence, not an error: the
                        // shell fans a send at every live backend.
                        _ => {}
                    }
                }
                Ok(Command::Quit) => {
                    client.session.quit("dialer");
                    let _ = client.flush_outbound();
                    return;
                }
                // IRC has no calls. Ignoring is the honest response; a fake
                // CallAdded would put a ringing screen on a channel.
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        // 2. Read whatever arrived; a timeout is not an error.
        match client.pump(&mut buf) {
            Ok(events) => {
                for ev in events {
                    if let Some(out) = to_link_event(&acct.network, &client.session, ev) {
                        if ev_tx.send(out).is_err() {
                            return;
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => {
                let _ = ev_tx.send(Event::ModemDown { detail: format!("irc read: {e}") });
                return;
            }
        }

        // 3. Advance the keepalive clock with real elapsed time.
        let elapsed = last_tick.elapsed();
        if elapsed >= Duration::from_secs(1) {
            last_tick = Instant::now();
            match client.tick(elapsed.as_secs()) {
                Ok(evs) => {
                    for ev in evs {
                        if let Some(out) = to_link_event(&acct.network, &client.session, ev) {
                            let _ = ev_tx.send(out);
                        }
                    }
                }
                Err(e) => {
                    let _ = ev_tx.send(Event::ModemDown { detail: format!("irc write: {e}") });
                    return;
                }
            }
        }

        if client.session.stage() == Stage::Closed {
            return;
        }
    }
}

/// Map an irc-kit event onto the Link protocol. `None` for events the phone UI
/// has no place for — joins, parts, topics and raw numerics belong in the
/// channel log, not in a thread list that is meant to show conversations.
fn to_link_event(network: &str, session: &Session, ev: IrcEvent) -> Option<Event> {
    match ev {
        IrcEvent::Registered { nick } => Some(Event::Info(format!("irc: registered as {nick}"))),
        IrcEvent::Message { from, target, text, server_time, notice, action } => {
            // A DM to us is a thread with the sender; a channel message is a
            // thread with the channel.
            let peer_target = if irc_kit::proto::is_channel(&target) { &target } else { &from };
            let body = if action { format!("* {from} {text}") } else { text };
            let body = if notice { format!("[notice] {body}") } else { body };
            Some(Event::MessageIn {
                peer: peer_id(network, peer_target),
                // Always Text. Never parse_sms: a public channel is the last
                // place a `run:` prefix should be promoted to a suggestion.
                payload: Payload::Text { body },
                ts: server_time
                    .as_deref()
                    .and_then(irc_kit::transport::parse_iso8601_secs)
                    .unwrap_or_else(now_secs),
            })
        }
        IrcEvent::NickInUse { tried, next } => Some(Event::Error(match next {
            Some(n) => format!("irc: {tried} is taken, trying {n}"),
            None => format!("irc: {tried} is taken and no fallbacks are left"),
        })),
        IrcEvent::Disconnected { reason } => Some(Event::ModemDown { detail: format!("irc: {reason}") }),
        IrcEvent::NickChange { from, to } if from == session.nick() || to == session.nick() => {
            Some(Event::Info(format!("irc: now known as {to}")))
        }
        _ => None,
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_ids_round_trip() {
        let p = peer_id("oftc", "#asahi-dev");
        assert_eq!(p, "irc:oftc/#asahi-dev");
        assert_eq!(parse_peer(&p), Some(("oftc".into(), "#asahi-dev".into())));
    }

    #[test]
    fn other_transports_peer_ids_are_not_ours() {
        assert_eq!(parse_peer("tel:+15551234"), None);
        assert_eq!(parse_peer("matrix:@a:x"), None);
        assert_eq!(parse_peer("irc:"), None);
        assert_eq!(parse_peer("irc:oftc"), None);
        assert_eq!(parse_peer("irc:oftc/"), None);
        assert_eq!(parse_peer("irc:/#chan"), None);
    }

    #[test]
    fn a_channel_containing_a_slash_keeps_it() {
        // Rare but legal; splitting on the LAST slash would corrupt it.
        assert_eq!(
            parse_peer("irc:oftc/#weird/name"),
            Some(("oftc".into(), "#weird/name".into()))
        );
    }

    #[test]
    fn handoff_payloads_flatten_to_inert_text() {
        let p = Payload::CommandSuggestion {
            command: "rm -rf /".into(),
            cwd: None,
            note: Some("do not".into()),
        };
        let s = flatten(&p);
        assert!(s.starts_with("[suggestion] "));
        assert!(s.contains("rm -rf /"));
        assert!(!s.starts_with("run:"), "must not re-encode as a parseable command");
    }

    #[test]
    fn file_payloads_flatten_without_the_uri_scheme_being_actionable() {
        let s = flatten(&Payload::FileOffer {
            name: "x.png".into(),
            mime: None,
            size: None,
            uri: "https://host/x.png".into(),
        });
        assert!(s.starts_with("[file] x.png"));
        assert!(!s.starts_with("file:"));
    }
}
