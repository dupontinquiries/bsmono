//! bsettings-cli — command-line front-end to settingsd.
//!
//! `list` / `get` / `set` the shared Prefs. Changes are published to the `settings` namespace,
//! which settingsd persists (~/.config/gitmind/settings.conf) and fans out to every running app —
//! the same daemon the graphical `bsettings` talks to, so the two stay in lock-step.

use bigsilicon_client::Settings;

fn usage() -> ! {
    eprintln!(
        "bsettings-cli — talk to settingsd\n\n\
         USAGE:\n\
         \x20 bsettings-cli list                 print all current settings\n\
         \x20 bsettings-cli get <key>            print one setting\n\
         \x20 bsettings-cli set <key> <value>    change a setting (persists + broadcasts)\n\n\
         keys are settings-kit dotted keys, e.g. gesture.sensitivity, display.color_filter, palette"
    );
    std::process::exit(2);
}

/// Connect and pull the current snapshot (poll briefly so the daemon's listing is ingested).
fn connect_or_die() -> Settings {
    match Settings::connect() {
        Some(mut s) => {
            for _ in 0..40 {
                s.poll();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            s
        }
        None => {
            eprintln!("bsettings: couldn't reach settingsd on the 'settings' socket — is it running? (gitmind installs it, or start it with `settingsd`.)");
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("list") => {
            let s = connect_or_die();
            for (k, v) in s.prefs().to_kv() {
                println!("{k} = {v}");
            }
        }
        Some("get") => {
            let key = args.get(1).unwrap_or_else(|| usage());
            let s = connect_or_die();
            match s.prefs().to_kv().into_iter().find(|(k, _)| k == key) {
                Some((_, v)) => println!("{v}"),
                None => {
                    eprintln!("no such setting: {key}");
                    std::process::exit(1);
                }
            }
        }
        Some("set") => {
            let key = args.get(1).unwrap_or_else(|| usage());
            let val = args.get(2).unwrap_or_else(|| usage());
            let mut s = connect_or_die();
            match s.set(key, val) {
                Ok(()) => {
                    // let the write settle so it's persisted before we exit
                    std::thread::sleep(std::time::Duration::from_millis(60));
                    println!("set {key} = {val}");
                }
                Err(e) => {
                    eprintln!("failed to set {key}: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => usage(),
    }
}
