//! sharemd — the cross-app data-share daemon.
//!
//! Binds the `share` socket and serves [`datashare_core::Hub`] to every app: namespaced
//! key/value with pub-sub, so shared state (contacts, sessions, now-playing, handoff) lives
//! once. Apps talk to it through ipc-kit and fall back to private state if it isn't running.

use datashare_core::wire::{to_msg, to_op};
use datashare_core::Hub;
use ipc_kit::{serve_hub, ConnId, Message, Service};

/// The Hub wrapped as an ipc-kit service. `ConnId` is used directly as the datashare
/// subscriber id, so pub-sub notifications route to the right connections.
struct ShareService {
    hub: Hub,
}

impl Service for ShareService {
    fn handle(&mut self, from: ConnId, msg: Message) -> Vec<(ConnId, Message)> {
        let eff = self.hub.handle(from, to_op(&msg));
        let mut out = Vec::new();
        if let Some(reply) = eff.reply {
            out.push((from, to_msg(&reply)));
        }
        for (sub, ev) in eff.notify {
            out.push((sub, to_msg(&ev)));
        }
        out
    }
    fn disconnect(&mut self, id: ConnId) {
        self.hub.drop_sub(id);
    }
}

/// Bind `name` and serve forever.
pub fn run(name: &str) -> std::io::Result<()> {
    let listener = ipc_kit::bind(name)?;
    serve_hub(listener, ShareService { hub: Hub::new() });
    Ok(())
}

fn main() {
    let name = "share";
    eprintln!("sharemd: serving on {}", ipc_kit::socket_path(name).display());
    if let Err(e) = run(name) {
        eprintln!("sharemd: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipc_kit::Client;
    use std::time::Duration;

    #[test]
    fn two_clients_share_data_with_pubsub() {
        let name = format!("sharemd-test-{}", std::process::id());
        let listener = ipc_kit::bind(&name).expect("bind");
        std::thread::spawn(move || serve_hub(listener, ShareService { hub: Hub::new() }));
        std::thread::sleep(Duration::from_millis(50));

        // subscriber connects and subscribes to "contacts"
        let mut sub = Client::connect(&name).expect("sub connect");
        sub.send(&Message::new("sub", vec!["contacts".into()])).unwrap();
        assert_eq!(sub.recv().unwrap().verb, "ok");

        // writer connects and puts a key
        let mut writer = Client::connect(&name).expect("writer connect");
        let put = writer.request(&Message::new("put", vec!["contacts".into(), "alice".into(), "online".into()])).unwrap();
        assert_eq!(put.verb, "ok");

        // subscriber receives the event (writer is a different connection)
        let ev = sub.recv().expect("event pushed to subscriber");
        assert_eq!(ev.verb, "event");
        assert_eq!(ev.args, vec!["contacts".to_string(), "alice".to_string(), "online".to_string()]);

        // a fresh reader can GET the value the writer put
        let mut reader = Client::connect(&name).expect("reader connect");
        let got = reader.request(&Message::new("get", vec!["contacts".into(), "alice".into()])).unwrap();
        assert_eq!(got.verb, "value");
        assert_eq!(got.args, vec!["contacts".to_string(), "alice".to_string(), "online".to_string()]);
    }
}
