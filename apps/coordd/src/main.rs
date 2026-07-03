//! coordd — the coordination daemon.
//!
//! Binds the `coord` socket and serves [`coord_core::Arbiter`]. Apps speak:
//!
//! * `hello <app>` — identify the connection.
//! * `claim <resource> <priority> <mode>` — claim a shared resource.
//! * `release <resource>` — drop a claim.
//! * `activity <activity>` — announce presence (relayed to interested apps).
//!
//! After every claim/release/disconnect, coordd recomputes each connected app's directive
//! for every resource and **pushes `directive <resource> <directive>` only where it
//! changed** — so snapclone gets a `duck` when voiceterm ducks the call, and an `own` back
//! when voiceterm releases. Apps act on the pushed directive (mute, show busy, pipe, …).

use coord_core::{Arbiter, Claim, Directive, Mode, Resource};
use ipc_kit::{serve_hub, ConnId, Message, Service};
use std::collections::HashMap;

struct CoordService {
    arb: Arbiter,
    app_of: HashMap<ConnId, String>,
    conn_of: HashMap<String, ConnId>,
    /// last directive pushed to each (app, resource), to send only deltas.
    last: HashMap<(String, Resource), Directive>,
}

impl CoordService {
    fn new() -> Self {
        CoordService { arb: Arbiter::new(), app_of: HashMap::new(), conn_of: HashMap::new(), last: HashMap::new() }
    }

    /// Recompute directives for every connected app × resource and return the pushes for
    /// those that changed since last time.
    fn recompute(&mut self) -> Vec<(ConnId, Message)> {
        let mut out = Vec::new();
        // stable order for determinism
        let mut apps: Vec<(String, ConnId)> = self.conn_of.iter().map(|(a, c)| (a.clone(), *c)).collect();
        apps.sort();
        for (app, cid) in apps {
            for res in Resource::all() {
                let d = self.arb.directive(&app, res);
                let key = (app.clone(), res);
                if self.last.get(&key) != Some(&d) {
                    self.last.insert(key, d);
                    // don't spam Idle on first sight (app isn't involved with this resource)
                    if d != Directive::Idle {
                        out.push((cid, Message::new("directive", vec![res.as_str().into(), d.as_str().into()])));
                    }
                }
            }
        }
        out
    }
}

impl Service for CoordService {
    fn handle(&mut self, from: ConnId, msg: Message) -> Vec<(ConnId, Message)> {
        let a = |i: usize| msg.args.get(i).cloned().unwrap_or_default();
        match msg.verb.as_str() {
            "hello" => {
                let app = a(0);
                if !app.is_empty() {
                    self.app_of.insert(from, app.clone());
                    self.conn_of.insert(app, from);
                }
                let mut out = vec![(from, Message::verb("ok"))];
                out.extend(self.recompute());
                out
            }
            "claim" => {
                let Some(app) = self.app_of.get(&from).cloned() else {
                    return vec![(from, Message::new("err", vec!["say hello first".into()]))];
                };
                let (Some(res), Some(mode)) = (Resource::parse(&a(0)), Mode::parse(&a(2))) else {
                    return vec![(from, Message::new("err", vec!["bad resource/mode".into()]))];
                };
                let prio: u8 = a(1).parse().unwrap_or(0);
                self.arb.claim(Claim::new(&app, res, prio, mode));
                let mut out = vec![(from, Message::verb("ok"))];
                out.extend(self.recompute());
                out
            }
            "release" => {
                let Some(app) = self.app_of.get(&from).cloned() else {
                    return vec![(from, Message::new("err", vec!["say hello first".into()]))];
                };
                if let Some(res) = Resource::parse(&a(0)) {
                    self.arb.release(&app, res);
                }
                let mut out = vec![(from, Message::verb("ok"))];
                out.extend(self.recompute());
                out
            }
            "activity" => {
                // presence relay: tell every other app "<app> is <activity>"
                let app = self.app_of.get(&from).cloned().unwrap_or_default();
                let activity = a(0);
                let mut out = vec![(from, Message::verb("ok"))];
                for (&cid, other) in self.app_of.iter() {
                    if cid != from && !other.is_empty() {
                        out.push((cid, Message::new("presence", vec![app.clone(), activity.clone()])));
                    }
                }
                out
            }
            _ => vec![(from, Message::new("err", vec!["unknown verb".into()]))],
        }
    }

    fn disconnect(&mut self, id: ConnId) {
        if let Some(app) = self.app_of.remove(&id) {
            self.arb.release_app(&app);
            if self.conn_of.get(&app) == Some(&id) {
                self.conn_of.remove(&app);
            }
            self.last.retain(|(a, _), _| a != &app);
            // note: pushes from this recompute go to *remaining* apps; the disconnecting
            // connection is already gone, so its own entries are simply dropped.
            let _ = self.recompute();
        }
    }
}

pub fn run(name: &str) -> std::io::Result<()> {
    let listener = ipc_kit::bind(name)?;
    serve_hub(listener, CoordService::new());
    Ok(())
}

fn main() {
    let name = "coord";
    eprintln!("coordd: serving on {}", ipc_kit::socket_path(name).display());
    if let Err(e) = run(name) {
        eprintln!("coordd: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coord_core::prio;
    use ipc_kit::Client;
    use std::time::Duration;

    fn hello(name: &str, app: &str) -> Client {
        let mut c = Client::connect(name).expect("connect");
        assert_eq!(c.request(&Message::new("hello", vec![app.into()])).unwrap().verb, "ok");
        c
    }
    fn claim(c: &mut Client, res: &str, prio: u8, mode: &str) {
        c.send(&Message::new("claim", vec![res.into(), prio.to_string(), mode.into()])).unwrap();
    }
    /// drain messages until one with `verb` arrives (skipping acks), or panic.
    fn wait_for(c: &mut Client, verb: &str) -> Message {
        for _ in 0..20 {
            if let Some(m) = c.recv() {
                if m.verb == verb {
                    return m;
                }
            } else {
                break;
            }
        }
        panic!("did not receive '{verb}'");
    }

    #[test]
    fn call_duck_resume_pushes_directives() {
        let name = format!("coordd-test-{}", std::process::id());
        let listener = ipc_kit::bind(&name).expect("bind");
        std::thread::spawn(move || serve_hub(listener, CoordService::new()));
        std::thread::sleep(Duration::from_millis(50));

        // snapclone starts a call → owns call audio
        let mut snapclone = hello(&name, "snapclone");
        claim(&mut snapclone, "call_audio", prio::VOICE_CALL, "exclusive");
        assert_eq!(snapclone.recv().unwrap().verb, "ok");
        let own = wait_for(&mut snapclone, "directive");
        assert_eq!(own.args, vec!["call_audio".to_string(), "own".to_string()]);

        // voiceterm ducks the call to let the user compose
        let mut voiceterm = hello(&name, "voiceterm");
        claim(&mut voiceterm, "call_audio", prio::DICTATION, "duck");
        assert_eq!(voiceterm.recv().unwrap().verb, "ok");
        // snapclone is pushed a 'duck' directive → it mutes momentarily
        let duck = wait_for(&mut snapclone, "directive");
        assert_eq!(duck.args, vec!["call_audio".to_string(), "duck".to_string()]);

        // voiceterm finishes → releases → snapclone is pushed 'own' again (call resumes)
        voiceterm.send(&Message::new("release", vec!["call_audio".into()])).unwrap();
        let resume = wait_for(&mut snapclone, "directive");
        assert_eq!(resume.args, vec!["call_audio".to_string(), "own".to_string()]);
    }
}
