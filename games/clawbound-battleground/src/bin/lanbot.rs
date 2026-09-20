//! lanbot -- a LAN player with no window.
//!
//!   lanbot host <name> <secs>      host a den and report who turns up
//!   lanbot join <name> <secs>      find the first joinable den and join it
//!
//! It runs the SAME `LanLobby` the LAN screen paints, so what this does on a
//! network is what the game does. Made for `maintenance/tools/lan_sim.sh`,
//! which gives each bot its own network namespace on a bridge -- separate
//! stacks, real broadcast -- because loopback is not a LAN.

use clawbound::lanlobby::{Action, LanLobby};
use std::time::{Duration, Instant};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (role, name, secs) = (a.get(1).map(String::as_str).unwrap_or("join"), a.get(2).cloned().unwrap_or_else(|| "Bot".into()), a.get(3).and_then(|s| s.parse().ok()).unwrap_or(6.0f32));
    let mut lobby = LanLobby::open(&name);
    if role == "host" {
        lobby.set_hosting(true);
    }
    let (t0, mut last, mut asked) = (Instant::now(), String::new(), false);
    while t0.elapsed().as_secs_f32() < secs {
        lobby.tick(Instant::now());
        if role == "join" && !asked {
            if let Some(i) = lobby.rows.iter().position(|r| r.joinable) {
                if let Action::Join(addr) = lobby.click_join(i) {
                    println!("[{name}] sees {:?} at {addr} -- joining", lobby.rows[i].name);
                    lobby.join(addr);
                    asked = true;
                }
            }
        }
        if lobby.status != last {
            last = lobby.status.clone();
            println!("[{name}] {last}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let who: Vec<&str> = lobby.roster.iter().map(|m| m.1.as_str()).collect();
    println!("[{name}] FINAL players={} roster={who:?} rows={}", lobby.players(), lobby.rows.len());
}
