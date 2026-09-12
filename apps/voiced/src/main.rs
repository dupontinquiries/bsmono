//! voiced — the unified voice daemon.
//!
//! Binds the `voiced` socket and owns intent routing via [`voiced_core::Registry`]. The
//! protocol an app speaks:
//!
//! * `hello <app>` — identify this connection's app (once, on connect).
//! * `register <event_type> <scope>` — subscribe to an intent type at Always/Active scope.
//! * `focus <app>` — the compositor tells the daemon which app is active.
//! * `intent <event_type> [slots…]` / `say <utterance…>` — fed by the audio stage
//!   (mic → whisper → intent, from voice-kit/audio-kit/voiceterm). `say` also catches the
//!   universal system phrases ("switch to X", "open Y in X").
//!
//! The daemon pushes `event <event_type> [slots…]`, `focus`, or `open <document>` to the
//! target app's connection. Apps try this socket first and fall back to their own in-process
//! voice code if `voiced` isn't running.

use ipc_kit::{serve_hub, ConnId, Message, Scope, Service};
use std::collections::HashMap;
use voiced_core::{Dispatch, Intent, Match, Matcher, Registry};

/// Pure: true if any dispatch in this routing round delivered to an app
/// other than the currently active one — pulled out of
/// `route_and_deliver` specifically so this decision is unit-testable
/// directly. Real haptic firing can't be observed in a build sandbox (no
/// vibrator, no way to check one buzzed); this at least confirms the
/// DECISION of when it would fire is correct, independent of the I/O it
/// triggers.
fn is_backgrounded_delivery(dispatches: &[Dispatch], active: Option<&str>) -> bool {
    dispatches.iter().any(|d| matches!(d, Dispatch::Deliver { app, .. } if Some(app.as_str()) != active))
}

/// Plays a haptic pattern on its own thread — a daemon's message-handling
/// loop cannot block for a pattern's whole duration (haptic-kit's own
/// `PatternPlayer` is pollable specifically so nothing has to), and this
/// is fire-and-forget: nothing here reports back to the caller, matching
/// `bt_send.rs`/`handshake_audio.rs`'s own reasoning elsewhere in this
/// fleet for isolating real hardware I/O onto its own thread rather than
/// running it inline. A device with no discoverable vibrator (most build
/// environments, some hardware) silently plays nothing — this is a
/// confirmation nicety, not something that should ever fail intent
/// routing itself.
fn play_haptic_async(pattern: haptic_kit::Pattern) {
    std::thread::spawn(move || {
        let motor = match haptic_kit::Motor::discover() {
            Ok(m) => m,
            Err(_) => return,
        };
        let total = pattern.total_duration_ms();
        let mut elapsed = 0u32;
        let mut last_state = None;
        // 5ms polling granularity: coarse enough to be cheap on a
        // background thread, fine enough that no real pulse boundary
        // (the shortest defined pattern pulse is 15ms) gets missed.
        while elapsed <= total {
            let state = haptic_kit::PatternPlayer::state_at(&pattern, elapsed);
            if Some(state) != last_state {
                let _ = motor.set(if state == haptic_kit::MotorState::On { 1.0 } else { 0.0 });
                last_state = Some(state);
            }
            if state == haptic_kit::MotorState::Finished {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            elapsed += 5;
        }
        let _ = motor.off(); // regardless of how the loop above exited, never leave the motor running
    });
}

struct VoiceService {
    reg: Registry,
    matcher: Matcher,
    app_of: HashMap<ConnId, String>,
    conn_of: HashMap<String, ConnId>,
}

impl VoiceService {
    fn new() -> Self {
        VoiceService { reg: Registry::new(), matcher: Matcher::new(), app_of: HashMap::new(), conn_of: HashMap::new() }
    }

    /// Routes an intent and delivers it, firing a haptic confirmation if
    /// (and only if) any resulting delivery went to an app OTHER than the
    /// currently active one — the `Scope::Always`-while-backgrounded case
    /// haptic-kit exists for, not every single intent regardless of
    /// whether the user is already looking right at the app that handled
    /// it. `deliver()` itself stays exactly as it was: pure, tested,
    /// message-building only — the haptic decision and the real I/O it
    /// triggers both live here, one layer up, not inside it.
    fn route_and_deliver(&mut self, intent: &Intent) -> Vec<(ConnId, Message)> {
        let dispatches = self.reg.route(intent);
        if is_backgrounded_delivery(&dispatches, self.reg.active()) {
            play_haptic_async(haptic_kit::Pattern::notification());
        }
        self.deliver(dispatches)
    }

    /// Turn a routing decision into an addressed message to the target app's connection.
    fn deliver(&self, dispatches: Vec<Dispatch>) -> Vec<(ConnId, Message)> {
        let mut out = Vec::new();
        for d in dispatches {
            match d {
                Dispatch::Deliver { app, event_type, slots } => {
                    if let Some(&cid) = self.conn_of.get(&app) {
                        let mut args = vec![event_type];
                        args.extend(slots);
                        out.push((cid, Message::new("event", args)));
                    }
                }
                Dispatch::Focus { app } => {
                    if let Some(&cid) = self.conn_of.get(&app) {
                        out.push((cid, Message::verb("focus")));
                    }
                }
                Dispatch::Open { app, document } => {
                    if let Some(&cid) = self.conn_of.get(&app) {
                        out.push((cid, Message::new("open", vec![document])));
                    }
                }
                Dispatch::Dropped => {}
            }
        }
        out
    }
}

impl Service for VoiceService {
    fn handle(&mut self, from: ConnId, msg: Message) -> Vec<(ConnId, Message)> {
        let a = |i: usize| msg.args.get(i).cloned().unwrap_or_default();
        match msg.verb.as_str() {
            "hello" => {
                let app = a(0);
                if !app.is_empty() {
                    self.app_of.insert(from, app.clone());
                    self.conn_of.insert(app, from);
                }
                vec![(from, Message::verb("ok"))]
            }
            "register" => {
                if let Some(app) = self.app_of.get(&from).cloned() {
                    let event_type = a(0);
                    self.reg.register(&app, &event_type, Scope::parse(&a(1)));
                    // args after scope are the grammar phrase (e.g. play {track})
                    let phrase: Vec<String> = msg.args.iter().skip(2).cloned().collect();
                    if !phrase.is_empty() {
                        self.matcher.register_rule(&app, &event_type, &phrase);
                    }
                    vec![(from, Message::verb("ok"))]
                } else {
                    vec![(from, Message::new("err", vec!["say hello first".into()]))]
                }
            }
            "focus" => {
                self.reg.set_active(&a(0));
                self.matcher.set_active(&a(0));
                vec![(from, Message::verb("ok"))]
            }
            "intent" => {
                let event_type = a(0);
                let slots: Vec<String> = msg.args.iter().skip(1).cloned().collect();
                self.route_and_deliver(&Intent::App { event_type, slots })
            }
            "transcript" => {
                // the audio stage's cleaned/finalized transcript → match → route
                let text = msg.args.join(" ");
                match self.matcher.match_utterance(&text) {
                    Match::System(intent) => self.route_and_deliver(&intent),
                    Match::App { event_type, slots } => self.route_and_deliver(&Intent::App { event_type, slots }),
                    Match::None => vec![],
                }
            }
            "say" => {
                let utterance = msg.args.join(" ");
                match Intent::parse_system(&utterance) {
                    Some(intent) => self.route_and_deliver(&intent),
                    None => vec![], // real NL is the apps' grammars; nothing system-level matched
                }
            }
            _ => vec![(from, Message::new("err", vec!["unknown verb".into()]))],
        }
    }

    fn disconnect(&mut self, id: ConnId) {
        if let Some(app) = self.app_of.remove(&id) {
            self.reg.unregister_app(&app);
            self.matcher.remove_app(&app);
            if self.conn_of.get(&app) == Some(&id) {
                self.conn_of.remove(&app);
            }
        }
    }
}

pub fn run(name: &str) -> std::io::Result<()> {
    let listener = ipc_kit::bind(name)?;
    serve_hub(listener, VoiceService::new());
    Ok(())
}

/// `--help` output.
///
/// A daemon still needs one. `systemctl status` shows the ExecStart line, and
/// the first thing anyone does with an unfamiliar binary is run it with
/// --help — getting a socket bind error instead is how someone concludes the
/// service is broken when it is merely already running.
fn usage() -> String {
    format!("\
voiced \u{2014} the voice daemon

USAGE:
    voiced [OPTIONS]

routes recognised speech to the focused app over the `voiced` socket.

OPTIONS:
    -h, --help       show this and exit
    -V, --version    show the version and exit

NOTE:
    voiced is socket-activated. systemd creates the socket at login and starts
    this on the first connection, so there is normally no reason to run it by
    hand \u{2014} and doing so while voiced.socket is enabled will fail to bind.

    Enable the SOCKET, not the service:
        systemctl --user enable --now voiced.socket

SEE ALSO:
    man voiced(1)
")
}

fn main() {
    // --help and --version before anything touches the socket: a binary that
    // has to bind before it can tell you what it is cannot be introspected on a
    // machine where it is already running, which is every machine where it
    // matters.
    let cli = cli_kit::Cli::from_env();
    if cli.wants_help() {
        print!("{}", usage());
        return;
    }
    if cli.flag("version") || cli.flag("V") {
        println!("voiced {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let name = "voiced";
    eprintln!("voiced: serving on {}", ipc_kit::socket_path(name).display());
    if let Err(e) = run(name) {
        eprintln!("voiced: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipc_kit::Client;
    use std::time::Duration;

    #[test]
    fn a_delivery_to_the_active_app_needs_no_haptic() {
        let dispatches = vec![Dispatch::Deliver {
            app: "logscope".into(),
            event_type: "next_page".into(),
            slots: vec![],
        }];
        assert!(!is_backgrounded_delivery(&dispatches, Some("logscope")));
    }

    #[test]
    fn a_delivery_to_a_backgrounded_app_needs_haptic() {
        let dispatches = vec![Dispatch::Deliver {
            app: "voiceterm".into(),
            event_type: "mail_incoming".into(),
            slots: vec![],
        }];
        assert!(is_backgrounded_delivery(&dispatches, Some("logscope")));
    }

    #[test]
    fn no_active_app_at_all_still_counts_any_delivery_as_backgrounded() {
        // No app has ever called `focus` yet — every delivery is, by
        // definition, not to "the active app" because there isn't one.
        let dispatches = vec![Dispatch::Deliver { app: "voiceterm".into(), event_type: "x".into(), slots: vec![] }];
        assert!(is_backgrounded_delivery(&dispatches, None));
    }

    #[test]
    fn non_deliver_dispatches_never_trigger_haptic_on_their_own() {
        let dispatches = vec![Dispatch::Focus { app: "logscope".into() }, Dispatch::Dropped];
        assert!(!is_backgrounded_delivery(&dispatches, Some("something-else")));
    }

    #[test]
    fn a_mixed_batch_with_even_one_backgrounded_delivery_needs_haptic() {
        // Real routing can produce several dispatches from one intent
        // (Scope::Active to the focused app AND Scope::Always to another) —
        // one backgrounded delivery among several must not get averaged
        // away or require ALL of them to be backgrounded.
        let dispatches = vec![
            Dispatch::Deliver { app: "logscope".into(), event_type: "x".into(), slots: vec![] },
            Dispatch::Deliver { app: "voiceterm".into(), event_type: "x".into(), slots: vec![] },
        ];
        assert!(is_backgrounded_delivery(&dispatches, Some("logscope")), "the SECOND delivery is backgrounded");
    }

    fn hello_register(name: &str, app: &str, event: &str, scope: &str) -> Client {
        let mut c = Client::connect(name).expect("connect");
        assert_eq!(c.request(&Message::new("hello", vec![app.into()])).unwrap().verb, "ok");
        assert_eq!(c.request(&Message::new("register", vec![event.into(), scope.into()])).unwrap().verb, "ok");
        c
    }

    #[test]
    fn routes_active_always_and_system_intents() {
        let name = format!("voiced-test-{}", std::process::id());
        let listener = ipc_kit::bind(&name).expect("bind");
        std::thread::spawn(move || serve_hub(listener, VoiceService::new()));
        std::thread::sleep(Duration::from_millis(50));

        let mut logscope = hello_register(&name, "logscope", "next_page", "active");
        let mut voiceterm = hello_register(&name, "voiceterm", "mail_incoming", "always");

        // a control connection stands in for the audio stage
        let mut audio = Client::connect(&name).expect("audio");
        audio.send(&Message::new("hello", vec!["_audio".into()])).unwrap();
        audio.recv();
        // focus logscope
        audio.send(&Message::new("focus", vec!["logscope".into()])).unwrap();

        // active-scope intent reaches the focused app…
        audio.send(&Message::new("intent", vec!["next_page".into()])).unwrap();
        let ev = logscope.recv().expect("logscope gets next_page");
        assert_eq!(ev.verb, "event");
        assert_eq!(ev.args, vec!["next_page".to_string()]);

        // …always-scope intent reaches voiceterm even though it's backgrounded
        audio.send(&Message::new("intent", vec!["mail_incoming".into(), "from:alice".into()])).unwrap();
        let ev = voiceterm.recv().expect("voiceterm gets mail_incoming");
        assert_eq!(ev.args, vec!["mail_incoming".to_string(), "from:alice".to_string()]);

        // system intent: "switch to voiceterm" → voiceterm gets a focus push
        audio.send(&Message::new("say", vec!["switch".into(), "to".into(), "voiceterm".into()])).unwrap();
        let f = voiceterm.recv().expect("voiceterm focused");
        assert_eq!(f.verb, "focus");
    }

    #[test]
    fn transcript_runs_clean_match_route() {
        let name = format!("voiced-tr-{}", std::process::id());
        let listener = ipc_kit::bind(&name).expect("bind");
        std::thread::spawn(move || serve_hub(listener, VoiceService::new()));
        std::thread::sleep(Duration::from_millis(50));

        // beatspace registers a slot grammar and is focused
        let mut beatspace = Client::connect(&name).expect("connect");
        beatspace.request(&Message::new("hello", vec!["beatspace".into()])).unwrap();
        beatspace
            .request(&Message::new("register", vec!["play_track".into(), "active".into(), "play".into(), "{track}".into()]))
            .unwrap();

        let mut audio = Client::connect(&name).expect("audio");
        audio.request(&Message::new("hello", vec!["_audio".into()])).unwrap();
        audio.send(&Message::new("focus", vec!["beatspace".into()])).unwrap();

        // a whisper hallucination is dropped (no event)…
        audio.send(&Message::new("transcript", vec!["thank".into(), "you".into()])).unwrap();
        // …and a real utterance matches the slot grammar and routes with the captured slot
        audio.send(&Message::new("transcript", vec!["play".into(), "koyopper".into(), "bass".into()])).unwrap();
        let ev = beatspace.recv().expect("beatspace gets play_track");
        assert_eq!(ev.verb, "event");
        assert_eq!(ev.args, vec!["play_track".to_string(), "koyopper bass".to_string()]);
    }
}
