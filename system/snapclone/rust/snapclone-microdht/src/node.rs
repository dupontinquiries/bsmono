//! node.rs — the micro-DHT node: a UDP server wrapping the TTL store.
//!
//! Single-socket, single-thread request loop plus a background sweeper thread.
//! No async runtime — `recv_from` blocks with a read timeout so the loop can
//! also notice a shutdown flag. This is deliberately the simplest thing that
//! faithfully reproduces the app-visible DHT behavior (publish, look up,
//! expire) for beta testing on loopback or a LAN.

use crate::protocol::{self, Request};
use crate::store::Store;

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Default TTL ceiling. The app re-announces every ~55 min and OpenDHT values
/// live ~1 hour; we cap any client-requested TTL at this so a buggy client
/// can't pin a value forever, matching the real DHT's expiry discipline.
pub const MAX_TTL_SECS: u64 = 3600;

/// How often the background sweeper reaps expired values.
const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// A running micro-DHT node. Holds the shared store and a shutdown flag so a
/// test (or the standalone binary's signal handler) can stop it cleanly.
pub struct Node {
    socket: UdpSocket,
    store: Arc<Mutex<Store>>,
    shutdown: Arc<AtomicBool>,
    verbose: bool,
}

impl Node {
    /// Bind a node to `addr` (e.g. "127.0.0.1:4544"). Use port 0 to let the OS
    /// pick — handy in tests; read it back with `local_addr()`.
    pub fn bind(addr: &str, verbose: bool) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        // Read timeout so the recv loop can poll the shutdown flag.
        socket.set_read_timeout(Some(Duration::from_millis(250)))?;
        Ok(Self {
            socket,
            store: Arc::new(Mutex::new(Store::new())),
            shutdown: Arc::new(AtomicBool::new(false)),
            verbose,
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// A handle that lets another thread request shutdown.
    pub fn shutdown_handle(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    /// Shared store handle (tests inspect counts through this).
    pub fn store_handle(&self) -> Arc<Mutex<Store>> {
        self.store.clone()
    }

    /// Spawn the background sweeper. Returns its JoinHandle so the caller can
    /// join it on shutdown.
    fn spawn_sweeper(&self) -> std::thread::JoinHandle<()> {
        let store = self.store.clone();
        let shutdown = self.shutdown.clone();
        let verbose = self.verbose;
        std::thread::spawn(move || {
            while !shutdown.load(Ordering::Relaxed) {
                std::thread::sleep(SWEEP_INTERVAL);
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                let reaped = {
                    let mut s = store.lock().unwrap();
                    s.sweep()
                };
                if verbose && reaped > 0 {
                    eprintln!("[microdht] swept {reaped} expired value(s)");
                }
            }
        })
    }

    /// Run the request loop until shutdown is signaled. Blocks the calling
    /// thread. Spawns + joins the sweeper internally.
    pub fn run(&self) -> std::io::Result<()> {
        let sweeper = self.spawn_sweeper();
        if self.verbose {
            eprintln!(
                "[microdht] listening on {} (max ttl {}s, sweep every {}s)",
                self.local_addr()?,
                MAX_TTL_SECS,
                SWEEP_INTERVAL.as_secs()
            );
        }
        let mut buf = [0u8; 64 * 1024];
        while !self.shutdown.load(Ordering::Relaxed) {
            let (n, peer) = match self.socket.recv_from(&mut buf) {
                Ok(v) => v,
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue; // read timeout → poll shutdown flag, loop
                }
                Err(e) => return Err(e),
            };
            let line = String::from_utf8_lossy(&buf[..n]);
            let reply = self.handle_line(&line, peer);
            // Best-effort reply; a test client that already closed is fine.
            let _ = self.socket.send_to(reply.as_bytes(), peer);
        }
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = sweeper.join();
        Ok(())
    }

    /// Process one request line, returning the response line. Pure-ish: only
    /// touches the shared store. Exposed for unit testing without a socket.
    pub fn handle_line(&self, line: &str, peer: SocketAddr) -> String {
        match protocol::parse_request(line) {
            Ok(Request::Ping) => "PONG".to_string(),
            Ok(Request::Stats) => {
                let s = self.store.lock().unwrap();
                format!(
                    "STATS keys={} values={} puts={} gets={} expired={}",
                    s.key_count(),
                    s.value_count(),
                    s.total_puts,
                    s.total_gets,
                    s.total_expired
                )
            }
            Ok(Request::Put { key, value, ttl_secs }) => {
                let ttl = Duration::from_secs(ttl_secs.min(MAX_TTL_SECS).max(1));
                self.store.lock().unwrap().put(&key, value, ttl);
                if self.verbose {
                    eprintln!(
                        "[microdht] PUT {} ({}s) from {peer}",
                        String::from_utf8_lossy(&key),
                        ttl.as_secs()
                    );
                }
                "OK".to_string()
            }
            Ok(Request::Get { key }) => {
                let values = self.store.lock().unwrap().get(&key);
                if self.verbose {
                    eprintln!(
                        "[microdht] GET {} -> {} value(s) for {peer}",
                        String::from_utf8_lossy(&key),
                        values.len()
                    );
                }
                protocol::encode_values(&key, &values)
            }
            Err(e) => format!("ERR {e:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn loopback() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
    }

    #[test]
    fn ping_pong() {
        let node = Node::bind("127.0.0.1:0", false).unwrap();
        assert_eq!(node.handle_line("PING", loopback()), "PONG");
    }

    #[test]
    fn put_then_get() {
        let node = Node::bind("127.0.0.1:0", false).unwrap();
        // value "hi" = 6869
        assert_eq!(node.handle_line("PUT sc-abc 6869 60", loopback()), "OK");
        assert_eq!(
            node.handle_line("GET sc-abc", loopback()),
            "VALUES sc-abc 6869"
        );
    }

    #[test]
    fn get_missing_is_empty() {
        let node = Node::bind("127.0.0.1:0", false).unwrap();
        assert_eq!(node.handle_line("GET sc-none", loopback()), "VALUES sc-none");
    }

    #[test]
    fn ttl_is_capped() {
        let node = Node::bind("127.0.0.1:0", false).unwrap();
        // request a 10-year TTL; store should cap it but still hold short-term
        node.handle_line("PUT sc-x 6869 999999999", loopback());
        assert_eq!(node.handle_line("GET sc-x", loopback()), "VALUES sc-x 6869");
    }

    #[test]
    fn bad_request_errors() {
        let node = Node::bind("127.0.0.1:0", false).unwrap();
        assert!(node.handle_line("NONSENSE", loopback()).starts_with("ERR"));
    }

    #[test]
    fn end_to_end_over_udp() {
        // Bind a node, run it on a thread, talk to it over a real UDP socket.
        let node = Node::bind("127.0.0.1:0", false).unwrap();
        let addr = node.local_addr().unwrap();
        let shutdown = node.shutdown_handle();
        let server = std::thread::spawn(move || {
            node.run().unwrap();
        });

        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        client.send_to(b"PUT sc-e2e 6869 60", addr).unwrap();
        let mut buf = [0u8; 1024];
        let (n, _) = client.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"OK");

        client.send_to(b"GET sc-e2e", addr).unwrap();
        let (n, _) = client.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"VALUES sc-e2e 6869");

        shutdown.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }
}
