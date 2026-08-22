//! The pure session state machine: protocol before transport.
//!
//! [`Session`] owns every decision about registration, capability
//! negotiation, nick collision, and channel membership. It performs no IO and
//! reads no clock. You feed it [`Message`]s with [`Session::handle`] and it
//! returns [`Event`]s for the app plus queues outbound messages you drain with
//! [`Session::take_outbound`]. Time enters only as [`Session::tick`], called
//! by the transport with an elapsed-seconds count.
//!
//! This is the same shape as `dialer-core`'s split: all judgement here, pure
//! and unit-tested; the backend only moves bytes and reports facts.
//!
//! ## What it deliberately does not do
//!
//! * **No CTCP, no DCC.** DCC is a file-transfer protocol that opens sockets
//!   on request from a remote party; it has no place in a logging client and
//!   its absence is a security property, not a gap.
//! * **No auto-reconnect policy.** The state machine reports
//!   [`Event::Disconnected`]; how long to back off is the transport's
//!   business, because only the transport knows whether the socket died or
//!   the user asked to quit.
//! * **No flood queue timing.** [`Session`] marks messages it wants sent;
//!   pacing them is transport-side for the same reason.

use crate::proto::{self, Message};

/// Everything needed to register with a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub nick: String,
    /// Fallbacks tried in order on ERR_NICKNAMEINUSE before giving up.
    pub alt_nicks: Vec<String>,
    pub user: String,
    pub realname: String,
    /// Server password (`PASS`), not SASL. Empty = none.
    pub pass: String,
    /// Channels joined once registration completes.
    pub channels: Vec<String>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            nick: "guest".into(),
            alt_nicks: vec!["guest_".into(), "guest__".into()],
            user: "guest".into(),
            realname: "irc-kit".into(),
            pass: String::new(),
            channels: Vec::new(),
        }
    }
}

/// Where registration has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Nothing sent yet.
    Fresh,
    /// CAP LS sent, waiting for the list.
    Negotiating,
    /// NICK/USER sent, waiting for 001.
    Registering,
    /// 001 received; JOINs sent.
    Registered,
    /// The server said goodbye, or the socket is gone.
    Closed,
}

/// What the app is told about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Registration completed; `nick` is what the server actually gave us,
    /// which is not always what we asked for.
    Registered { nick: String },
    /// A channel message. `from` is a nick; `target` is a channel or our nick.
    /// `server_time` is the IRCv3 `time` tag verbatim when present — the
    /// server's own timestamp beats our arrival time for a log.
    Message {
        from: String,
        target: String,
        text: String,
        server_time: Option<String>,
        /// A `NOTICE` rather than a `PRIVMSG`. Bots and services use notices;
        /// keeping the distinction lets a reader filter them.
        notice: bool,
        /// A CTCP ACTION (`/me`), unwrapped. The `\x01ACTION …\x01` framing is
        /// stripped and this flag set, so the log stores readable text.
        action: bool,
    },
    /// Someone joined a channel we are in (including us).
    Join { nick: String, channel: String },
    /// Someone left. `reason` is the part/quit message.
    Part { nick: String, channel: String, reason: String },
    /// The channel topic, from JOIN's 332 or a later TOPIC.
    Topic { channel: String, topic: String },
    /// A nick change, which we must track to attribute later messages.
    NickChange { from: String, to: String },
    /// The server rejected our nick and we are trying another. Surfaced
    /// because silently becoming `guest__` confuses a person reading a log.
    NickInUse { tried: String, next: Option<String> },
    /// A numeric or command we chose not to interpret, passed through so the
    /// app can show raw protocol without the kit having to know everything.
    Raw(Message),
    /// The link is finished. `reason` is a human sentence.
    Disconnected { reason: String },
}

/// The pure state machine.
#[derive(Debug, Clone)]
pub struct Session {
    cfg: Config,
    stage: Stage,
    nick: String,
    nick_attempt: usize,
    outbound: Vec<Message>,
    /// Channels the server has confirmed we are in, RFC1459-lowercased.
    joined: Vec<String>,
    /// Seconds since we last heard anything from the server.
    idle_secs: u64,
    /// Seconds since we sent a PING that has not been answered; `None` if no
    /// ping is outstanding.
    awaiting_pong: Option<u64>,
}

/// Silence before we probe the server with a PING.
pub const IDLE_PING_SECS: u64 = 120;
/// Silence after an unanswered PING before we call the link dead.
pub const PONG_TIMEOUT_SECS: u64 = 60;

impl Session {
    pub fn new(cfg: Config) -> Session {
        let nick = cfg.nick.clone();
        Session {
            cfg,
            stage: Stage::Fresh,
            nick,
            nick_attempt: 0,
            outbound: Vec::new(),
            joined: Vec::new(),
            idle_secs: 0,
            awaiting_pong: None,
        }
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// Our current nick, which may differ from the configured one.
    pub fn nick(&self) -> &str {
        &self.nick
    }

    /// Channels the server has confirmed, lowercased.
    pub fn channels(&self) -> &[String] {
        &self.joined
    }

    /// Drain everything the session wants written. The transport writes these
    /// in order, appending CRLF.
    pub fn take_outbound(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.outbound)
    }

    /// Queue a message whose last param is a plain middle param.
    fn send(&mut self, command: &str, params: &[&str]) {
        self.outbound.push(Message::new(command, params));
    }

    /// Queue a message whose last param is a `:`-prefixed trailing param —
    /// the spelling every other client uses for these commands.
    fn send_t(&mut self, command: &str, params: &[&str]) {
        self.outbound.push(Message::trailing(command, params));
    }

    /// Begin registration. Idempotent-ish: calling it twice re-sends, which is
    /// what you want after a reconnect on the same Session.
    pub fn start(&mut self) {
        self.stage = Stage::Negotiating;
        self.nick_attempt = 0;
        self.nick = self.cfg.nick.clone();
        self.joined.clear();
        self.idle_secs = 0;
        self.awaiting_pong = None;

        // Ask only for what a logger actually uses. server-time gives the
        // server's own timestamp (correct across reconnects and netsplits);
        // message-tags is its prerequisite on most servers.
        self.send("CAP", &["LS", "302"]);
        if !self.cfg.pass.is_empty() {
            let pass = self.cfg.pass.clone();
            self.send("PASS", &[&pass]);
        }
        let nick = self.nick.clone();
        let user = self.cfg.user.clone();
        let realname = self.cfg.realname.clone();
        self.send("NICK", &[&nick]);
        self.send_t("USER", &[&user, "0", "*", &realname]);
        self.stage = Stage::Registering;
    }

    /// Advance time. Returns events (a timeout produces `Disconnected`) and
    /// may queue a PING.
    pub fn tick(&mut self, elapsed_secs: u64) -> Vec<Event> {
        self.idle_secs += elapsed_secs;
        if let Some(w) = self.awaiting_pong.as_mut() {
            *w += elapsed_secs;
            if *w >= PONG_TIMEOUT_SECS {
                self.stage = Stage::Closed;
                return vec![Event::Disconnected {
                    reason: format!("no PONG within {PONG_TIMEOUT_SECS}s"),
                }];
            }
            return Vec::new();
        }
        if self.stage == Stage::Registered && self.idle_secs >= IDLE_PING_SECS {
            self.send_t("PING", &["irc-kit"]);
            self.awaiting_pong = Some(0);
        }
        Vec::new()
    }

    /// Send a message to a channel or nick. Returns the [`Event`] the app
    /// should log for its own line — the server does not echo our own
    /// PRIVMSGs back (absent `echo-message`), so if we don't log it here it
    /// never appears.
    pub fn privmsg(&mut self, target: &str, text: &str) -> Event {
        self.send_t("PRIVMSG", &[target, text]);
        Event::Message {
            from: self.nick.clone(),
            target: target.to_string(),
            text: text.to_string(),
            server_time: None,
            notice: false,
            action: false,
        }
    }

    pub fn join(&mut self, channel: &str) {
        self.send("JOIN", &[channel]);
    }

    pub fn part(&mut self, channel: &str, reason: &str) {
        self.send_t("PART", &[channel, reason]);
    }

    pub fn quit(&mut self, reason: &str) {
        self.send_t("QUIT", &[reason]);
        self.stage = Stage::Closed;
    }

    /// Feed one inbound message. Returns zero or more events.
    pub fn handle(&mut self, m: &Message) -> Vec<Event> {
        self.idle_secs = 0;
        let mut out = Vec::new();

        match m.command.as_str() {
            // Answer immediately and never surface it. A PING we fail to
            // answer is the single most common reason a bot silently drops
            // off a network.
            "PING" => {
                let token = m.param(0).to_string();
                self.send_t("PONG", &[&token]);
            }
            "PONG" => {
                self.awaiting_pong = None;
            }

            "CAP" => {
                // :server CAP * LS :cap1 cap2 …   /  CAP * ACK :cap1
                match m.param(1) {
                    "LS" => {
                        let offered = m.params.last().cloned().unwrap_or_default();
                        let want: Vec<&str> = ["server-time", "message-tags"]
                            .into_iter()
                            .filter(|c| offered.split_whitespace().any(|o| {
                                // 302 sends `name=value`; match on the name.
                                o.split('=').next() == Some(*c)
                            }))
                            .collect();
                        if want.is_empty() {
                            self.send("CAP", &["END"]);
                        } else {
                            let joined = want.join(" ");
                            self.send_t("CAP", &["REQ", &joined]);
                        }
                    }
                    "ACK" | "NAK" => self.send("CAP", &["END"]),
                    _ => {}
                }
            }

            // 001 RPL_WELCOME — the only reliable "you are registered" signal.
            "001" => {
                self.stage = Stage::Registered;
                // The server's idea of our nick wins; it may have truncated it.
                let given = m.param(0);
                if !given.is_empty() && given != "*" {
                    self.nick = given.to_string();
                }
                for ch in self.cfg.channels.clone() {
                    self.send("JOIN", &[&ch]);
                }
                out.push(Event::Registered { nick: self.nick.clone() });
            }

            // 433 ERR_NICKNAMEINUSE
            "433" => {
                let tried = self.nick.clone();
                let next = self.cfg.alt_nicks.get(self.nick_attempt).cloned();
                self.nick_attempt += 1;
                if let Some(n) = next.clone() {
                    self.nick = n.clone();
                    self.send("NICK", &[&n]);
                } else {
                    // Out of fallbacks. Say so rather than sitting silently in
                    // Registering forever, which looks like a network hang.
                    self.stage = Stage::Closed;
                    out.push(Event::NickInUse { tried: tried.clone(), next: None });
                    out.push(Event::Disconnected {
                        reason: "every configured nick was in use".into(),
                    });
                    return out;
                }
                out.push(Event::NickInUse { tried, next });
            }

            // 332 RPL_TOPIC: <me> <channel> :<topic>
            "332" => out.push(Event::Topic {
                channel: m.param(1).to_string(),
                topic: m.param(2).to_string(),
            }),
            "TOPIC" => out.push(Event::Topic {
                channel: m.param(0).to_string(),
                topic: m.param(1).to_string(),
            }),

            "PRIVMSG" | "NOTICE" => {
                if let Some(from) = m.nick() {
                    let raw = m.param(1);
                    let (text, action) = unwrap_action(raw);
                    out.push(Event::Message {
                        from: from.to_string(),
                        target: m.param(0).to_string(),
                        text,
                        server_time: m.tag("time").map(|s| s.to_string()),
                        notice: m.command == "NOTICE",
                        action,
                    });
                }
            }

            "JOIN" => {
                if let Some(nick) = m.nick() {
                    let ch = m.param(0).to_string();
                    if nick == self.nick {
                        let low = proto::lower(&ch);
                        if !self.joined.contains(&low) {
                            self.joined.push(low);
                        }
                    }
                    out.push(Event::Join { nick: nick.to_string(), channel: ch });
                }
            }

            "PART" => {
                if let Some(nick) = m.nick() {
                    let ch = m.param(0).to_string();
                    if nick == self.nick {
                        let low = proto::lower(&ch);
                        self.joined.retain(|c| c != &low);
                    }
                    out.push(Event::Part {
                        nick: nick.to_string(),
                        channel: ch,
                        reason: m.param(1).to_string(),
                    });
                }
            }

            "QUIT" => {
                if let Some(nick) = m.nick() {
                    // A QUIT carries no channel — it is a departure from the
                    // network. Report it once, unattached to a channel, and
                    // let the log decide which channels it affects.
                    out.push(Event::Part {
                        nick: nick.to_string(),
                        channel: String::new(),
                        reason: m.param(0).to_string(),
                    });
                }
            }

            "NICK" => {
                if let Some(from) = m.nick() {
                    let to = m.param(0).to_string();
                    if from == self.nick {
                        self.nick = to.clone();
                    }
                    out.push(Event::NickChange { from: from.to_string(), to });
                }
            }

            "ERROR" => {
                self.stage = Stage::Closed;
                out.push(Event::Disconnected { reason: m.param(0).to_string() });
            }

            _ => out.push(Event::Raw(m.clone())),
        }

        out
    }
}

/// Strip CTCP ACTION framing (`\x01ACTION text\x01`) if present.
/// Any *other* CTCP is left framed and will be logged verbatim rather than
/// interpreted — replying to CTCP VERSION/PING is a fingerprinting surface a
/// logging client has no reason to expose.
fn unwrap_action(raw: &str) -> (String, bool) {
    let t = raw.trim_end_matches('\u{1}');
    match t.strip_prefix("\u{1}ACTION ") {
        Some(body) => (body.to_string(), true),
        None => (raw.to_string(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::parse;

    fn cfg() -> Config {
        Config {
            nick: "tester".into(),
            alt_nicks: vec!["tester_".into()],
            user: "tester".into(),
            realname: "irc-kit test".into(),
            pass: String::new(),
            channels: vec!["#asahi-dev".into()],
        }
    }

    fn started() -> Session {
        let mut s = Session::new(cfg());
        s.start();
        s.take_outbound();
        s
    }

    #[test]
    fn start_sends_cap_nick_user_in_order() {
        let mut s = Session::new(cfg());
        s.start();
        let out: Vec<String> = s.take_outbound().iter().map(crate::proto::format).collect();
        assert_eq!(out[0], "CAP LS 302");
        assert_eq!(out[1], "NICK tester");
        assert_eq!(out[2], "USER tester 0 * :irc-kit test");
        assert_eq!(s.stage(), Stage::Registering);
    }

    #[test]
    fn pass_is_sent_before_nick_when_configured() {
        let mut c = cfg();
        c.pass = "hunter2".into();
        let mut s = Session::new(c);
        s.start();
        let out: Vec<String> = s.take_outbound().iter().map(crate::proto::format).collect();
        assert_eq!(out[1], "PASS hunter2");
        assert_eq!(out[2], "NICK tester");
    }

    #[test]
    fn ping_is_answered_and_never_surfaced() {
        let mut s = started();
        let ev = s.handle(&parse("PING :abc123").unwrap());
        assert!(ev.is_empty(), "PING must not reach the app");
        let out: Vec<String> = s.take_outbound().iter().map(crate::proto::format).collect();
        assert_eq!(out, vec!["PONG :abc123"]);
    }

    #[test]
    fn cap_requests_only_what_the_server_offers() {
        let mut s = started();
        s.handle(&parse(":srv CAP * LS :server-time sasl multi-prefix").unwrap());
        let out: Vec<String> = s.take_outbound().iter().map(crate::proto::format).collect();
        // message-tags was not offered, so it is not requested.
        assert_eq!(out, vec!["CAP REQ :server-time"]);
    }

    #[test]
    fn cap_302_name_equals_value_still_matches() {
        let mut s = started();
        s.handle(&parse(":srv CAP * LS :server-time=x message-tags").unwrap());
        let out: Vec<String> = s.take_outbound().iter().map(crate::proto::format).collect();
        assert_eq!(out, vec!["CAP REQ :server-time message-tags"]);
    }

    #[test]
    fn cap_ends_when_nothing_useful_is_offered() {
        let mut s = started();
        s.handle(&parse(":srv CAP * LS :sasl multi-prefix").unwrap());
        let out: Vec<String> = s.take_outbound().iter().map(crate::proto::format).collect();
        assert_eq!(out, vec!["CAP END"]);
    }

    #[test]
    fn welcome_registers_and_joins_configured_channels() {
        let mut s = started();
        let ev = s.handle(&parse(":srv 001 tester :Welcome").unwrap());
        assert_eq!(ev, vec![Event::Registered { nick: "tester".into() }]);
        assert_eq!(s.stage(), Stage::Registered);
        let out: Vec<String> = s.take_outbound().iter().map(crate::proto::format).collect();
        assert_eq!(out, vec!["JOIN #asahi-dev"]);
    }

    #[test]
    fn the_servers_idea_of_our_nick_wins() {
        let mut s = started();
        s.handle(&parse(":srv 001 tester123 :Welcome").unwrap());
        assert_eq!(s.nick(), "tester123");
    }

    #[test]
    fn nick_in_use_walks_the_fallbacks_then_gives_up_loudly() {
        let mut s = started();
        let ev = s.handle(&parse(":srv 433 * tester :Nickname is already in use").unwrap());
        assert_eq!(ev, vec![Event::NickInUse { tried: "tester".into(), next: Some("tester_".into()) }]);
        assert_eq!(crate::proto::format(&s.take_outbound()[0]), "NICK tester_");

        let ev2 = s.handle(&parse(":srv 433 * tester_ :Nickname is already in use").unwrap());
        assert_eq!(ev2[0], Event::NickInUse { tried: "tester_".into(), next: None });
        assert!(matches!(ev2[1], Event::Disconnected { .. }), "must not hang silently");
        assert_eq!(s.stage(), Stage::Closed);
    }

    #[test]
    fn privmsg_carries_the_server_time_tag() {
        let mut s = started();
        let ev = s.handle(
            &parse("@time=2026-08-22T15:54:22.000Z :jc!u@h PRIVMSG #asahi-dev :sigh").unwrap(),
        );
        assert_eq!(
            ev,
            vec![Event::Message {
                from: "jc".into(),
                target: "#asahi-dev".into(),
                text: "sigh".into(),
                server_time: Some("2026-08-22T15:54:22.000Z".into()),
                notice: false,
                action: false,
            }]
        );
    }

    #[test]
    fn action_is_unwrapped_and_flagged() {
        let mut s = started();
        let ev = s.handle(&parse(":n!u@h PRIVMSG #c :\u{1}ACTION waves\u{1}").unwrap());
        match &ev[0] {
            Event::Message { text, action, .. } => {
                assert_eq!(text, "waves");
                assert!(*action);
            }
            e => panic!("expected a message, got {e:?}"),
        }
    }

    #[test]
    fn other_ctcp_is_left_framed_and_unanswered() {
        let mut s = started();
        let ev = s.handle(&parse(":n!u@h PRIVMSG tester :\u{1}VERSION\u{1}").unwrap());
        match &ev[0] {
            Event::Message { text, action, .. } => {
                assert!(text.starts_with('\u{1}'), "left framed, not interpreted");
                assert!(!*action);
            }
            e => panic!("expected a message, got {e:?}"),
        }
        assert!(s.take_outbound().is_empty(), "must not reply to CTCP VERSION");
    }

    #[test]
    fn notices_are_distinguishable_from_messages() {
        let mut s = started();
        let ev = s.handle(&parse(":bot!u@h NOTICE #c :ding").unwrap());
        match &ev[0] {
            Event::Message { notice, .. } => assert!(*notice),
            e => panic!("expected a message, got {e:?}"),
        }
    }

    #[test]
    fn our_own_join_updates_membership_others_do_not() {
        let mut s = started();
        s.handle(&parse(":srv 001 tester :hi").unwrap());
        s.handle(&parse(":tester!u@h JOIN #Asahi-Dev").unwrap());
        assert_eq!(s.channels(), &["#asahi-dev".to_string()]);
        s.handle(&parse(":someone!u@h JOIN #asahi-dev").unwrap());
        assert_eq!(s.channels().len(), 1, "someone else joining is not us joining");
        s.handle(&parse(":tester!u@h PART #asahi-dev :bye").unwrap());
        assert!(s.channels().is_empty());
    }

    #[test]
    fn our_own_nick_change_is_tracked() {
        let mut s = started();
        s.handle(&parse(":srv 001 tester :hi").unwrap());
        s.handle(&parse(":tester!u@h NICK newname").unwrap());
        assert_eq!(s.nick(), "newname");
    }

    #[test]
    fn privmsg_returns_the_event_to_log_because_the_server_wont_echo() {
        let mut s = started();
        let ev = s.privmsg("#asahi-dev", "hello");
        match ev {
            Event::Message { from, target, text, .. } => {
                assert_eq!((from.as_str(), target.as_str(), text.as_str()), ("tester", "#asahi-dev", "hello"));
            }
            e => panic!("expected a message, got {e:?}"),
        }
        assert_eq!(crate::proto::format(&s.take_outbound()[0]), "PRIVMSG #asahi-dev :hello");
    }

    #[test]
    fn idle_triggers_a_ping_and_a_dead_link_is_declared() {
        let mut s = started();
        s.handle(&parse(":srv 001 tester :hi").unwrap());
        s.take_outbound();

        assert!(s.tick(IDLE_PING_SECS - 1).is_empty());
        assert!(s.take_outbound().is_empty(), "not idle enough yet");

        assert!(s.tick(1).is_empty());
        assert_eq!(crate::proto::format(&s.take_outbound()[0]), "PING :irc-kit");

        assert!(s.tick(PONG_TIMEOUT_SECS - 1).is_empty());
        let ev = s.tick(1);
        assert!(matches!(ev[0], Event::Disconnected { .. }));
        assert_eq!(s.stage(), Stage::Closed);
    }

    #[test]
    fn a_pong_clears_the_outstanding_probe() {
        let mut s = started();
        s.handle(&parse(":srv 001 tester :hi").unwrap());
        s.tick(IDLE_PING_SECS);
        s.take_outbound();
        s.handle(&parse(":srv PONG srv :irc-kit").unwrap());
        // Well past the timeout, but the pong landed, so no disconnect.
        assert!(s.tick(PONG_TIMEOUT_SECS * 2).is_empty());
        assert_ne!(s.stage(), Stage::Closed);
    }

    #[test]
    fn unknown_numerics_pass_through_as_raw() {
        let mut s = started();
        let ev = s.handle(&parse(":srv 375 tester :- MOTD -").unwrap());
        assert!(matches!(ev[0], Event::Raw(_)));
    }
}
