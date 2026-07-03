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
use voiced_core::{Dispatch, Intent, Registry};

struct VoiceService {
    reg: Registry,
    app_of: HashMap<ConnId, String>,
    conn_of: HashMap<String, ConnId>,
}

impl VoiceService {
    fn new() -> Self {
        VoiceService { reg: Registry::new(), app_of: HashMap::new(), conn_of: HashMap::new() }
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
                    self.reg.register(&app, &a(0), Scope::parse(&a(1)));
                    vec![(from, Message::verb("ok"))]
                } else {
                    vec![(from, Message::new("err", vec!["say hello first".into()]))]
                }
            }
            "focus" => {
                self.reg.set_active(&a(0));
                vec![(from, Message::verb("ok"))]
            }
            "intent" => {
                let event_type = a(0);
                let slots: Vec<String> = msg.args.iter().skip(1).cloned().collect();
                let dispatches = self.reg.route(&Intent::App { event_type, slots });
                self.deliver(dispatches)
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

fn main() {
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
}
