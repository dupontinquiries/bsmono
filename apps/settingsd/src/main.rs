//! settingsd — the persistent settings daemon.
//!
//! Serves the cross-app `settings` namespace (datashare-core's KV + pub-sub) on the
//! `settings` socket, and — unlike the general sharemd — **persists** every change to
//! `~/.config/gitmind/settings.conf` via config-kit, so preferences survive restarts. On
//! first run it seeds sane defaults from settings-kit. The central settings app publishes
//! changes here; every egui-kit app subscribes.

use datashare_core::wire::{to_msg, to_op};
use datashare_core::{Hub, Op};
use ipc_kit::{serve_hub, ConnId, Message, Service};

const APP: &str = "gitmind";
const FILE: &str = "settings.conf";
const NS: &str = "settings";

/// Load persisted settings (config-kit Ini `[settings]` section), or seed defaults.
fn load_into(hub: &mut Hub) {
    let persisted = config_kit::load(APP, FILE)
        .map(|text| {
            let ini = config_kit::Ini::parse(&text);
            ini.sections.get(NS).map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>()).unwrap_or_default()
        })
        .unwrap_or_default();

    let kv = if persisted.is_empty() {
        settings_kit::Prefs::default().to_kv() // first run → defaults
    } else {
        persisted
    };
    for (k, v) in kv {
        hub.handle(0, Op::Put { ns: NS.to_string(), key: k, val: v });
    }
}

/// Write the current settings back to disk as `[settings] key = value`.
fn persist(hub: &Hub) {
    let mut out = String::from("[settings]\n");
    for (k, v) in hub.snapshot(NS) {
        out.push_str(&format!("{k} = {v}\n"));
    }
    let _ = config_kit::save(APP, FILE, &out);
}

struct SettingsService {
    hub: Hub,
}

impl Service for SettingsService {
    fn handle(&mut self, from: ConnId, msg: Message) -> Vec<(ConnId, Message)> {
        let op = to_op(&msg);
        let mutating = matches!(op, Op::Put { .. } | Op::Delete { .. });
        let eff = self.hub.handle(from, op);
        if mutating {
            persist(&self.hub); // durability: settings survive restarts
        }
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

pub fn run(name: &str) -> std::io::Result<()> {
    let mut hub = Hub::new();
    load_into(&mut hub);
    let listener = ipc_kit::bind(name)?;
    serve_hub(listener, SettingsService { hub });
    Ok(())
}

fn main() {
    let name = NS;
    eprintln!("settingsd: serving on {} (persisting to ~/.config/{APP}/{FILE})", ipc_kit::socket_path(name).display());
    if let Err(e) = run(name) {
        eprintln!("settingsd: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_reloads_across_restart() {
        // isolate config to a temp XDG_CONFIG_HOME
        let dir = std::env::temp_dir().join(format!("settingsd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("XDG_CONFIG_HOME", &dir);

        // "first run": defaults seeded, then a change is made + persisted
        let mut hub = Hub::new();
        load_into(&mut hub);
        // default sensitivity is present
        assert!(hub.snapshot(NS).iter().any(|(k, _)| k == "gesture.sensitivity"));
        // change it and persist (as the service would)
        hub.handle(1, Op::Put { ns: NS.into(), key: "gesture.sensitivity".into(), val: "2.5".into() });
        persist(&hub);

        // "restart": a fresh hub loads from disk and sees the change
        let mut hub2 = Hub::new();
        load_into(&mut hub2);
        let got: Vec<_> = hub2.snapshot(NS).into_iter().filter(|(k, _)| k == "gesture.sensitivity").collect();
        assert_eq!(got, vec![("gesture.sensitivity".to_string(), "2.5".to_string())], "change survived restart");

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_fanout_still_works() {
        use ipc_kit::Client;
        use std::time::Duration;
        let dir = std::env::temp_dir().join(format!("settingsd-fan-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let name = format!("settingsd-test-{}", std::process::id());
        let listener = ipc_kit::bind(&name).expect("bind");
        std::thread::spawn(move || {
            let mut hub = Hub::new();
            load_into(&mut hub);
            serve_hub(listener, SettingsService { hub });
        });
        std::thread::sleep(Duration::from_millis(50));

        let mut sub = Client::connect(&name).unwrap();
        sub.send(&Message::new("sub", vec![NS.into()])).unwrap();
        sub.recv(); // ok
        let mut writer = Client::connect(&name).unwrap();
        writer.request(&Message::new("put", vec![NS.into(), "theme.palette".into(), "Nord".into()])).unwrap();
        let ev = sub.recv().expect("event");
        assert_eq!(ev.verb, "event");
        assert_eq!(ev.args, vec![NS.to_string(), "theme.palette".to_string(), "Nord".to_string()]);

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
