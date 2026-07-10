//! netview — expose a live game snapshot over a local socket so the shared **webhost-kit** (Python)
//! can spectate/organise a soldier match in the browser, even though soldier isn't a Python game.
//!
//! This is the reference "socket adapter" server the kit's `SocketAdapter` talks to
//! (`shared/webhost-kit/socket_adapter.py`). Protocol: newline-delimited JSON, one request → one
//! response:
//!     → {"cmd":"snapshot"}                     ← {"ok":true, ...state...}
//!     → {"cmd":"action","action":{…},"seat":0} ← {"ok":true}
//!
//! The native game keeps its own authoritative loop; this only reflects state (and, later, pokes
//! input). Serialization is serde_json (already a dep); the server is std::net (no new deps, no
//! graphics), so it runs in the default headless build.

use crate::game::Game;
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, ToSocketAddrs};
use std::sync::Arc;

/// A player/enemy as the browser needs it: floor position + hp.
#[derive(Serialize, Clone, Debug)]
pub struct Actor {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub hp: f32,
}

/// The live snapshot browsers render. JSON-shaped to match what the kit's generic viewer and any
/// soldier-specific renderer expect.
#[derive(Serialize, Clone, Debug)]
pub struct SnapshotView {
    pub ok: bool,
    pub title: &'static str,
    pub time: f32,
    pub score: u32,
    pub kills: u32,
    pub wave: u32,
    pub player: Actor,
    pub enemies: Vec<Actor>,
}

/// Build a browser snapshot from the live game — the whole game-specific part of the integration.
pub fn snapshot(g: &Game) -> SnapshotView {
    SnapshotView {
        ok: true,
        title: "soldier",
        time: g.time,
        score: g.score,
        kills: g.kills,
        wave: g.wave,
        player: Actor {
            x: g.player.pos.x,
            y: g.player.pos.y,
            z: g.player.pos.z,
            hp: g.player.health,
        },
        enemies: g
            .enemies
            .iter()
            .map(|e| Actor { x: e.pos.x, y: e.pos.y, z: e.pos.z, hp: e.health })
            .collect(),
    }
}

/// Serve snapshots over TCP. `get` is called per request to read the current state — the app hands a
/// closure that locks its `Game` (e.g. `move || netview::snapshot(&game.lock())`). Blocks; run it on
/// a thread. Spectate-only for now: an "action" is acked but not applied (soldier's own input stays
/// authoritative); wire it through `get`'s owner when input-over-web lands.
pub fn serve<A, F>(addr: A, get: F) -> std::io::Result<()>
where
    A: ToSocketAddrs,
    F: Fn() -> SnapshotView + Send + Sync + 'static,
{
    let get = Arc::new(get);
    let listener = TcpListener::bind(addr)?;
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };
        let get = Arc::clone(&get);
        std::thread::spawn(move || {
            let mut w = match stream.try_clone() {
                Ok(w) => w,
                Err(_) => return,
            };
            let mut r = BufReader::new(stream);
            let mut line = String::new();
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => return, // peer closed
                    Ok(_) => {}
                }
                let cmd = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|v| v.get("cmd").and_then(|c| c.as_str()).map(str::to_owned))
                    .unwrap_or_default();
                let resp = match cmd.as_str() {
                    "snapshot" => serde_json::to_string(&get()).unwrap_or_else(|_| "{\"ok\":false}".into()),
                    "action" => "{\"ok\":true}".into(), // spectate-only for now
                    _ => "{\"ok\":false,\"err\":\"unknown cmd\"}".into(),
                };
                if writeln!(w, "{resp}").is_err() || w.flush().is_err() {
                    return;
                }
            }
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_view_serializes_to_the_expected_json_shape() {
        let s = SnapshotView {
            ok: true,
            title: "soldier",
            time: 12.5,
            score: 300,
            kills: 3,
            wave: 2,
            player: Actor { x: 1.0, y: 2.0, z: 3.0, hp: 87.5 },
            enemies: vec![Actor { x: 5.0, y: 0.0, z: 6.0, hp: 40.0 }],
        };
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["title"], "soldier");
        assert_eq!(v["score"], 300);
        assert_eq!(v["player"]["hp"], 87.5);
        assert_eq!(v["enemies"][0]["hp"], 40.0);
        assert_eq!(v["enemies"].as_array().unwrap().len(), 1);
    }
}
