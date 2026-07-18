//! lan_skirmish — host or join a LAN lockstep skirmish (net-kit auth+wire).
//!
//!   lan_skirmish --host 7777 --fac swarm --key sekrit
//!   lan_skirmish --join 192.168.1.20:7777 --fac rootbound --key sekrit

use cli_kit::Cli;
use paperbound_core::data::load_factbook;
use paperbound_core::net_skirmish::{host, join, NetOutcome};
use std::net::TcpListener;
use std::path::Path;

fn main() {
    let cli = Cli::from_env();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let book = load_factbook(&dir).expect("game data");
    let fac = cli.opt("--fac").unwrap_or("court").to_string();
    let key = cli.opt("--key").unwrap_or("paperbound").to_string();
    let out = if let Some(port) = cli.opt("--host") {
        let listener = TcpListener::bind(("0.0.0.0", port.parse().expect("port")))
            .expect("bind");
        println!("🛡  hosting on :{port} as {} — waiting for a challenger…", book[&fac].name);
        host(&book, listener, &fac, 900.0, &key)
    } else if let Some(addr) = cli.opt("--join") {
        println!("⚔  joining {addr} as {}…", book[&fac].name);
        join(&book, addr, &fac, &key)
    } else {
        eprintln!("usage: lan_skirmish (--host PORT | --join ADDR) [--fac F] [--key K]");
        std::process::exit(2);
    };
    match out {
        NetOutcome::Finished { winner, t, syncs } => {
            let name = match winner {
                Some(0) => "the host",
                Some(1) => "the challenger",
                _ => "nobody (draw)",
            };
            println!("🏆 {name} wins at {:.0}:{:02.0} — {syncs} lockstep checkpoints verified",
                     t / 60.0, t % 60.0);
        }
        NetOutcome::Desync { t } => {
            eprintln!("💥 DESYNC at t={t:.2}s — peers diverged (report this seed)");
            std::process::exit(1);
        }
        NetOutcome::Denied(why) => {
            eprintln!("🚫 denied: {why}");
            std::process::exit(1);
        }
    }
}
