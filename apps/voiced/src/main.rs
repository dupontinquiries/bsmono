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
                let dispatches = self.reg.route(&Intent::App { event_type, slots });
                self.deliver(dispatches)
            }
            "transcript" => {
                // the audio stage's cleaned/finalized transcript → match → route
                let text = msg.args.join(" ");
                match self.matcher.match_utterance(&text) {
                    Match::System(intent) => {
                        let dispatches = self.reg.route(&intent);
                        self.deliver(dispatches)
                    }
                    Match::App { event_type, slots } => {
                        let dispatches = self.reg.route(&Intent::App { event_type, slots });
                        self.deliver(dispatches)
                    }
                    Match::None => vec![],
                }
            }
            "say" => {
                let utterance = msg.args.join(" ");
                match Intent::parse_system(&utterance) {
                    Some(intent) => {
                        let dispatches = self.reg.route(&intent);
                        self.deliver(dispatches)
                    }
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
