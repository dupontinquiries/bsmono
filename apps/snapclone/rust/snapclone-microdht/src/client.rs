//! client.rs — a tiny blocking client for talking to a micro-DHT node.
//!
//! This is what the app-side integration (or a test) uses to PUT its endpoint
//! blob and GET a peer's. Synchronous request/response over UDP with a
//! timeout; one datagram out, one in. No retries beyond the caller's loop —
//! a TEST harness favors simplicity and obvious failure over robustness.

use crate::protocol::{self, hex_encode};
use std::net::{ToSocketAddrs, UdpSocket};
use std::time::Duration;

/// A connected micro-DHT client. `node_addr` is where the node listens.
pub struct Client {
    socket: UdpSocket,
}

#[derive(Debug)]
pub enum ClientError {
    Io(std::io::Error),
    Timeout,
    BadResponse(String),
}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::WouldBlock
            || e.kind() == std::io::ErrorKind::TimedOut
        {
            ClientError::Timeout
        } else {
            ClientError::Io(e)
        }
    }
}

impl Client {
    /// Connect to a node. Binds an ephemeral local socket and `connect`s it to
    /// the node so send/recv don't need the address each call.
    pub fn connect<A: ToSocketAddrs>(node_addr: A, timeout: Duration) -> std::io::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_read_timeout(Some(timeout))?;
        socket.connect(node_addr)?;
        Ok(Self { socket })
    }

    fn round_trip(&self, request: &str) -> Result<String, ClientError> {
        self.socket.send(request.as_bytes())?;
        let mut buf = [0u8; 64 * 1024];
        let n = self.socket.recv(&mut buf)?;
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }

    /// Liveness check.
    pub fn ping(&self) -> Result<bool, ClientError> {
        Ok(self.round_trip("PING")?.trim() == "PONG")
    }

    /// Publish `value` under `key` for `ttl`. Returns Ok(()) on "OK".
    pub fn put(&self, key: &str, value: &[u8], ttl: Duration) -> Result<(), ClientError> {
        let req = format!("PUT {} {} {}", key, hex_encode(value), ttl.as_secs().max(1));
        let resp = self.round_trip(&req)?;
        if resp.trim() == "OK" {
            Ok(())
        } else {
            Err(ClientError::BadResponse(resp))
        }
    }

    /// Look up all live values under `key`. Empty vec = key absent / expired.
    pub fn get(&self, key: &str) -> Result<Vec<Vec<u8>>, ClientError> {
        let resp = self.round_trip(&format!("GET {key}"))?;
        let resp = resp.trim();
        // Expected: "VALUES <key> <hex> <hex> ..."
        let mut it = resp.split_whitespace();
        match it.next() {
            Some("VALUES") => {}
            _ => return Err(ClientError::BadResponse(resp.to_string())),
        }
        let _echoed_key = it.next(); // skip echoed key
        let mut out = Vec::new();
        for hexv in it {
            match protocol::hex_decode(hexv) {
                Some(bytes) => out.push(bytes),
                None => return Err(ClientError::BadResponse(resp.to_string())),
            }
        }
        Ok(out)
    }

    /// Raw STATS line from the node.
    pub fn stats(&self) -> Result<String, ClientError> {
        self.round_trip("STATS")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::Node;
    use std::sync::atomic::Ordering;

    #[test]
    fn client_put_get_against_real_node() {
        let node = Node::bind("127.0.0.1:0", false).unwrap();
        let addr = node.local_addr().unwrap();
        let shutdown = node.shutdown_handle();
        let server = std::thread::spawn(move || node.run().unwrap());

        let client = Client::connect(addr, Duration::from_secs(2)).unwrap();
        assert!(client.ping().unwrap());
        client
            .put("sc-client", b"endpoint", Duration::from_secs(60))
            .unwrap();
        let got = client.get("sc-client").unwrap();
        assert_eq!(got, vec![b"endpoint".to_vec()]);
        assert!(client.get("sc-absent").unwrap().is_empty());

        shutdown.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }
}
