// gitmind-core/src/bridge.rs
//
// The "in a pinch" 2-way partner-code bridge: stream a whole folder between
// two machines with tar + netcat. No daemon, no setup — one side listens,
// the other sends, the working tree lands on the peer.
//
// This is intentionally the LOW-tech path. It is plaintext and
// unauthenticated, so it's for a trusted LAN or a quick pair-programming
// hand-off, NOT the durable sync path (that's remote.rs + encryption +
// Noise/QUIC). The GUI/backend can run these pipelines, or you can paste
// them into two terminals.
//
// We build the pipelines as strings meant for `sh -c`, because a tar|nc
// pipe is inherently a shell pipeline; the parts are also exposed for
// callers that want to wire the pipe themselves.

/// Sender: `tar -c[z]f - -C <dir> . | nc <host> <port>`.
pub fn send_pipeline(dir: &str, host: &str, port: u16, compress: bool) -> String {
    let z = if compress { "z" } else { "" };
    format!("tar -c{z}f - -C {dir} . | nc {host} {port}")
}

/// Receiver: `nc -l -p <port> | tar -x[z]f - -C <dir>`. Note: traditional
/// netcat uses `-l -p PORT`; nmap's ncat uses `-l PORT` — adjust if needed.
pub fn recv_pipeline(dir: &str, port: u16, compress: bool) -> String {
    let z = if compress { "z" } else { "" };
    format!("mkdir -p {dir} && nc -l -p {port} | tar -x{z}f - -C {dir}")
}

/// The tar side alone (for callers wiring their own transport, e.g. piping
/// over the GUI's existing peer connection instead of nc).
pub fn tar_send_argv(dir: &str, compress: bool) -> Vec<String> {
    let flag = if compress { "-czf" } else { "-cf" };
    vec!["tar".into(), flag.into(), "-".into(), "-C".into(), dir.into(), ".".into()]
}

pub fn tar_recv_argv(dir: &str, compress: bool) -> Vec<String> {
    let flag = if compress { "-xzf" } else { "-xf" };
    vec!["tar".into(), flag.into(), "-".into(), "-C".into(), dir.into()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_pipeline_shape() {
        let p = send_pipeline("/repo", "192.168.1.5", 9000, true);
        assert!(p.starts_with("tar -czf - -C /repo ."));
        assert!(p.contains("| nc 192.168.1.5 9000"));
    }

    #[test]
    fn recv_pipeline_listens_and_extracts() {
        let p = recv_pipeline("/repo", 9000, true);
        assert!(p.contains("nc -l -p 9000"));
        assert!(p.contains("tar -xzf - -C /repo"));
    }

    #[test]
    fn uncompressed_drops_z() {
        assert!(send_pipeline("/d", "h", 1, false).contains("tar -cf -"));
        assert_eq!(tar_send_argv("/d", false)[1], "-cf");
    }
}
