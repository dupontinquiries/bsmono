// src/client.rs
//
// A thin synchronous client for the bo-mux control socket. The Qt GUI's
// Rust side uses this to talk to the daemon: connect, send a Request line,
// read the Response line. Kept tiny and dependency-free so it can live
// inside the controller app's process.
//
// Event stream: a client that wants the daemon's unsolicited Events
// (scene state changes, peer updates) keeps a second long-lived connection
// and reads lines, dispatching those where `event` is present. The simple
// request/response path below is for commands.

use crate::api::{Request, Response, Command};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

pub struct Client {
    socket_path: PathBuf,
    next_id: u64,
}

impl Client {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self { socket_path: socket_path.into(), next_id: 1 }
    }

    /// Default user socket: $XDG_RUNTIME_DIR/bo-mux/control.sock
    pub fn default_socket() -> Self {
        let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Self::new(base.join(crate::api::DEFAULT_SOCKET_REL))
    }

    /// Is the daemon reachable? (socket exists AND answers ping)
    pub fn is_up(&mut self) -> bool {
        matches!(self.send(Command::Ping), Ok(r) if r.ok)
    }

    /// Send one command, block for the matching response.
    pub fn send(&mut self, cmd: Command) -> std::io::Result<Response> {
        let id = self.next_id;
        self.next_id += 1;
        let req = Request { id, cmd };

        let stream = UnixStream::connect(&self.socket_path)?;
        let mut writer = stream.try_clone()?;
        let mut line = serde_json::to_string(&req)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        line.push('\n');
        writer.write_all(line.as_bytes())?;
        writer.flush()?;

        // Read response lines until we get one matching our id (skipping any
        // interleaved events, which carry no "id"/"ok").
        let reader = BufReader::new(stream);
        for l in reader.lines() {
            let l = l?;
            if l.trim().is_empty() { continue; }
            if let Ok(resp) = serde_json::from_str::<Response>(&l) {
                if resp.id == id { return Ok(resp); }
            }
            // else: an Event line — ignore on the request path.
        }
        Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof,
            "daemon closed connection before responding"))
    }
}

/// Convenience: does a socket file exist at the default path? (cheap check
/// the GUI can use to decide whether to offer "install daemon".)
pub fn socket_present(socket_path: &Path) -> bool {
    socket_path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_increments_per_send_attempt() {
        // We can't reach a real daemon in tests, but we can verify the id
        // bookkeeping advances even on connection failure paths by checking
        // the field directly.
        let mut c = Client::new("/nonexistent/bo-mux.sock");
        assert_eq!(c.next_id, 1);
        let _ = c.send(Command::Ping); // will Err (no socket) but...
        // next_id advanced before the connect attempt
        assert_eq!(c.next_id, 2);
    }

    #[test]
    fn default_socket_path_uses_runtime_dir() {
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        let c = Client::default_socket();
        assert!(c.socket_path.ends_with("bo-mux/control.sock"));
    }
}
