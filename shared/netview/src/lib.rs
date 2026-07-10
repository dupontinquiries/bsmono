//! netview — the Rust server side of **webhost-kit**.
//!
//! A native (Rust/C) game can't be imported into the Python browser/LAN host, so it fronts it by
//! running a tiny endpoint that speaks the host's protocol. This crate is that endpoint, factored out
//! of the first consumer (soldier) so every native game shares one server and only supplies its own
//! snapshot view + mapping. It pairs with `shared/webhost-kit/socket_adapter.py` (the client).
//!
//! Protocol — newline-delimited JSON, one request → one response:
//! ```text
//! → {"cmd":"snapshot"}                     ← {"ok":true, ...game state...}
//! → {"cmd":"action","action":{…},"seat":0} ← {"ok":true}
//! ```
//! Generic over any [`serde::Serialize`] snapshot; the game passes a `Fn() -> S` that reads its live
//! state (typically `move || snapshot(&game.lock())`). Spectate-only for now: an "action" is
//! acknowledged but not applied, so the game's own input stays authoritative.

use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;

/// Serve snapshots over TCP until the listener errors. Blocks; run on a thread. `get` is called once
/// per request to read the current state.
pub fn serve<A, F, S>(addr: A, get: F) -> std::io::Result<()>
where
    A: ToSocketAddrs,
    F: Fn() -> S + Send + Sync + 'static,
    S: Serialize,
{
    let get = Arc::new(get);
    let listener = TcpListener::bind(addr)?;
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };
        let get = Arc::clone(&get);
        std::thread::spawn(move || serve_conn(stream, get.as_ref()));
    }
    Ok(())
}

/// Handle one client connection: read newline-JSON requests, answer each.
fn serve_conn<F, S>(stream: TcpStream, get: &F)
where
    F: Fn() -> S,
    S: Serialize,
{
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[derive(Serialize)]
    struct Demo {
        ok: bool,
        n: u32,
    }

    #[test]
    fn serves_a_snapshot_over_the_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // one-shot server thread using the same conn handler
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                serve_conn(stream, &|| Demo { ok: true, n: 7 });
            }
        });
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(b"{\"cmd\":\"snapshot\"}\n").unwrap();
        let mut buf = [0u8; 256];
        let n = s.read(&mut buf).unwrap();
        let line = std::str::from_utf8(&buf[..n]).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["n"], 7);
    }
}
