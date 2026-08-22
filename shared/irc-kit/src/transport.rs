//! The only module that moves bytes, and it still does not open a socket.
//!
//! [`Client`] is generic over one `S: Read + Write`, so the caller decides
//! what the connection is: a `TcpStream` for a plaintext test server, a
//! rustls stream for OFTC's 6697, or [`Pipe`] in a unit test. That keeps the
//! crate zero-dependency — no TLS stack, no async runtime — and keeps the
//! interesting logic in [`crate::session`], which this module only drives.
//!
//! One stream rather than a separate `R` and `W`: a socket is both halves,
//! and a `rustls::StreamOwned` cannot be split the way `TcpStream::try_clone`
//! can. The two-type version forced every caller with a single stream to
//! invent a shim, and a shim that silently discards writes is exactly the bug
//! you would not notice until nothing you said reached the channel.
//!
//! The loop is deliberately visible: `read → parse → session.handle → log →
//! drain outbound → write`. One function, no callbacks, no hidden state
//! machine. If it misbehaves you can read it top to bottom.

use std::io::{Read, Write};
use std::path::PathBuf;

use crate::log::{self, Line, Origin};
use crate::proto;
use crate::session::{Event, Session};

/// Drives a [`Session`] over a byte stream, logging as it goes.
pub struct Client<S: Read + Write> {
    pub session: Session,
    stream: S,
    /// Bytes read but not yet a complete line.
    tail: String,
    /// Where to append. `None` disables logging entirely (useful for a
    /// connection you explicitly do not want recorded).
    pub log_dir: Option<PathBuf>,
    /// Which network name goes into log records.
    pub network: String,
}

impl<S: Read + Write> Client<S> {
    pub fn new(session: Session, stream: S, network: &str) -> Client<S> {
        Client {
            session,
            stream,
            tail: String::new(),
            log_dir: Some(log::default_dir()),
            network: network.to_string(),
        }
    }

    /// Write everything the session has queued.
    pub fn flush_outbound(&mut self) -> std::io::Result<()> {
        for m in self.session.take_outbound() {
            let line = proto::format(&m);
            self.stream.write_all(line.as_bytes())?;
            self.stream.write_all(b"\r\n")?;
        }
        self.stream.flush()
    }

    /// Read whatever is available, feed it through the session, log the
    /// messages, and return every event for the app. A zero-length read means
    /// the peer closed; that is reported as [`Event::Disconnected`] rather
    /// than an `Err`, because a clean close is not an error.
    pub fn pump(&mut self, buf: &mut [u8]) -> std::io::Result<Vec<Event>> {
        let n = self.stream.read(buf)?;
        if n == 0 {
            return Ok(vec![Event::Disconnected { reason: "peer closed the connection".into() }]);
        }
        // Lossy on purpose: a single malformed byte from a client using some
        // 1990s codepage must not kill the connection. IRC has no charset.
        self.tail.push_str(&String::from_utf8_lossy(&buf[..n]));
        let owned = std::mem::take(&mut self.tail);
        let (msgs, rest) = proto::split_lines(&owned);
        self.tail = rest.to_string();

        let mut events = Vec::new();
        for m in &msgs {
            for ev in self.session.handle(m) {
                self.record(&ev);
                events.push(ev);
            }
        }
        self.flush_outbound()?;
        Ok(events)
    }

    /// Send a message and log our own copy — the server does not echo it back.
    pub fn say(&mut self, target: &str, text: &str) -> std::io::Result<()> {
        let ev = self.session.privmsg(target, text);
        self.record(&ev);
        self.flush_outbound()
    }

    /// Append an event to the channel log, if it is the kind that belongs
    /// there and logging is on. A log write failure is not fatal to the
    /// connection — it is reported on stderr and the session continues,
    /// because dropping off a channel because a disk filled is worse.
    fn record(&self, ev: &Event) {
        let Some(dir) = &self.log_dir else { return };
        let Event::Message { from, target, text, server_time, notice, action } = ev else {
            return;
        };
        // Private messages to us are not channel history; they are one
        // person's DM and are not logged here at all.
        if !proto::is_channel(target) {
            return;
        }
        let line = Line {
            ts: server_time
                .as_deref()
                .and_then(parse_iso8601_secs)
                .unwrap_or_else(core_kit::time::now_secs),
            server_time: server_time.clone(),
            network: self.network.clone(),
            channel: target.clone(),
            nick: from.clone(),
            text: text.clone(),
            notice: *notice,
            action: *action,
            origin: Origin::Participated,
        };
        if let Err(e) = log::append(dir, &line) {
            eprintln!("irc-kit: log append failed ({e}); staying connected");
        }
    }

    /// Advance the keepalive clock. Call from the caller's own timer.
    pub fn tick(&mut self, elapsed_secs: u64) -> std::io::Result<Vec<Event>> {
        let evs = self.session.tick(elapsed_secs);
        self.flush_outbound()?;
        Ok(evs)
    }
}

/// A scripted in-memory stream: reads drain `input`, writes accumulate in
/// `output`. Exists so [`Client`] can be exercised end to end with no socket,
/// and so a caller can replay a captured session against the state machine.
pub struct Pipe {
    pub input: std::io::Cursor<Vec<u8>>,
    pub output: Vec<u8>,
}

impl Pipe {
    pub fn new(input: &str) -> Pipe {
        Pipe { input: std::io::Cursor::new(input.as_bytes().to_vec()), output: Vec::new() }
    }
    /// Everything written so far, as a string.
    pub fn written(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }
    /// Append more bytes for the next read, simulating a second packet.
    pub fn feed(&mut self, more: &str) {
        let pos = self.input.position();
        let mut buf = self.input.get_ref().clone();
        buf.extend_from_slice(more.as_bytes());
        self.input = std::io::Cursor::new(buf);
        self.input.set_position(pos);
    }
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(buf)
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.output.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Parse the IRCv3 `time` tag (`2026-08-22T15:54:22.000Z`) to unix seconds.
///
/// Hand-rolled because the crate is zero-dependency and the format is fixed
/// by the spec to exactly this shape — always UTC, always `YYYY-MM-DDThh:mm:ss`
/// with an optional fractional part. Returns `None` for anything else rather
/// than guessing, and the caller falls back to arrival time.
pub fn parse_iso8601_secs(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<i64> { s.get(a..z)?.parse().ok() };
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }

    // Days from the civil calendar to the unix epoch (Howard Hinnant's
    // days_from_civil): exact, no leap-year table, no lookup.
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    let secs = days * 86_400 + h * 3600 + mi * 60 + se;
    u64::try_from(secs).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Config;

    fn client(input: &str) -> Client<Pipe> {
        let cfg = Config {
            nick: "tester".into(),
            channels: vec!["#asahi-dev".into()],
            ..Config::default()
        };
        let mut s = Session::new(cfg);
        s.start();
        s.take_outbound();
        let mut c = Client::new(s, Pipe::new(input), "oftc");
        c.log_dir = None; // unit tests do not touch the filesystem
        c
    }

    #[test]
    fn a_ping_in_the_stream_is_answered_on_the_same_stream() {
        let mut c = client("PING :abc\r\n");
        let mut buf = [0u8; 4096];
        let evs = c.pump(&mut buf).unwrap();
        assert!(evs.is_empty());
        assert_eq!(c.stream.written(), "PONG :abc\r\n");
    }

    #[test]
    fn what_we_say_actually_reaches_the_stream() {
        // The regression this replaces: a write-half shim that returned
        // Ok(len) and dropped the bytes. Nothing failed; nothing was sent.
        let mut c = client("");
        c.say("#asahi-dev", "hello").unwrap();
        assert_eq!(c.stream.written(), "PRIVMSG #asahi-dev :hello\r\n");
    }

    #[test]
    fn a_message_split_across_two_reads_is_reassembled() {
        let mut c = client(":n!u@h PRIVMSG #c :hel");
        let mut buf = [0u8; 4096];
        let evs = c.pump(&mut buf).unwrap();
        assert!(evs.is_empty(), "a partial line yields nothing yet");

        c.stream.feed("lo there\r\n");
        let evs = c.pump(&mut buf).unwrap();
        match &evs[0] {
            Event::Message { text, .. } => assert_eq!(text, "hello there"),
            e => panic!("expected a message, got {e:?}"),
        }
    }

    #[test]
    fn a_closed_peer_is_a_disconnect_event_not_an_error() {
        let mut c = client("");
        let mut buf = [0u8; 64];
        let evs = c.pump(&mut buf).unwrap();
        assert!(matches!(evs[0], Event::Disconnected { .. }));
    }

    #[test]
    fn invalid_utf8_does_not_kill_the_connection() {
        let cfg = Config::default();
        let mut s = Session::new(cfg);
        s.start();
        s.take_outbound();
        let mut bytes = b":n!u@h PRIVMSG #c :caf\xE9 time\r\n".to_vec(); // latin-1 e-acute
        bytes.extend_from_slice(b"PING :x\r\n");
        let pipe = Pipe { input: std::io::Cursor::new(bytes), output: Vec::new() };
        let mut c = Client::new(s, pipe, "oftc");
        c.log_dir = None;

        let mut buf = [0u8; 4096];
        let evs = c.pump(&mut buf).unwrap();
        assert!(matches!(evs[0], Event::Message { .. }));
        assert!(c.stream.written().contains("PONG"));
    }

    #[test]
    fn a_full_registration_exchange_runs_end_to_end() {
        let mut c = client(concat!(
            ":srv CAP * LS :server-time message-tags\r\n",
            ":srv CAP * ACK :server-time message-tags\r\n",
            ":srv 001 tester :Welcome\r\n",
            ":tester!u@h JOIN #asahi-dev\r\n",
            "@time=2026-08-22T15:54:22.000Z :jc!u@h PRIVMSG #asahi-dev :sigh\r\n",
        ));
        let mut buf = [0u8; 8192];
        let mut all = Vec::new();
        for _ in 0..4 {
            all.extend(c.pump(&mut buf).unwrap());
        }
        assert!(all.iter().any(|e| matches!(e, Event::Registered { .. })));
        assert!(all.iter().any(|e| matches!(e, Event::Message { .. })));
        assert_eq!(c.session.channels(), &["#asahi-dev".to_string()]);

        let sent = c.stream.written();
        assert!(sent.contains("CAP REQ :server-time message-tags\r\n"));
        assert!(sent.contains("CAP END\r\n"));
        assert!(sent.contains("JOIN #asahi-dev\r\n"));
    }

    #[test]
    fn private_messages_are_not_written_to_the_channel_log() {
        // Not a channel target, so `record` must skip it. Verified by the
        // absence of a panic with log_dir set to a path that does not exist:
        // if record ran, create_dir_all would fail and print, not append.
        let mut c = client(":n!u@h PRIVMSG tester :a dm\r\n");
        c.log_dir = Some(std::path::PathBuf::from("/proc/definitely-not-writable"));
        let mut buf = [0u8; 4096];
        let evs = c.pump(&mut buf).unwrap();
        assert!(matches!(evs[0], Event::Message { .. }));
    }

    #[test]
    fn iso8601_parses_the_ircv3_time_tag() {
        // 2026-08-22 is 20687 days after the epoch (not 20688 — the first
        // version of this test had the arithmetic wrong and the code right).
        let t = parse_iso8601_secs("2026-08-22T15:54:22.000Z").unwrap();
        assert_eq!(t, 20687 * 86_400 + 15 * 3600 + 54 * 60 + 22);
        assert_eq!(t, 1_787_414_062);
    }

    #[test]
    fn iso8601_known_anchors() {
        assert_eq!(parse_iso8601_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601_secs("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_iso8601_secs("2024-02-29T12:00:00Z"), Some(1_709_208_000));
    }

    #[test]
    fn a_malformed_time_tag_returns_none_rather_than_a_wrong_number() {
        for bad in ["", "not a time", "2026-13-01T00:00:00Z", "2026-08-22 15:54:22", "2026-08-22T25:00:00Z"] {
            assert_eq!(parse_iso8601_secs(bad), None, "{bad} should not parse");
        }
    }
}
