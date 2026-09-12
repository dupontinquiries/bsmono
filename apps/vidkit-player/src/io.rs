//! The IO the kit deliberately leaves out.
//!
//! `shared/vidkit` puts every unavailable thing behind a trait so its policy is
//! testable with no process, network or device. These are the two
//! implementations that were missing, and they are here rather than in the kit
//! because they are the parts that actually touch the machine.
//!
//! Both are deliberately thin. Anything resembling a decision — retry counts,
//! which format to pick, when a URL has expired — lives above them and is
//! already tested there.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use vidkit_ytdlp::http::{HttpError, HttpTransport, RangeRequest};
use vidkit_ytdlp::{CommandRunner, RunError};

/// Runs a real subprocess.
pub struct ProcessRunner {
    pub timeout: Duration,
}

impl Default for ProcessRunner {
    fn default() -> Self {
        Self {
            // yt-dlp routinely takes seconds on a cold extractor.
            timeout: Duration::from_secs(45),
        }
    }
}

impl CommandRunner for ProcessRunner {
    fn run(&mut self, program: &str, args: &[String]) -> Result<Vec<u8>, RunError> {
        // Spawned and polled rather than `output()`, which has no timeout: an
        // extractor that hangs -- a captive portal, a site that stalls the
        // connection -- would otherwise freeze the UI thread forever with no
        // way back. The pipes are drained on their own threads because a child
        // that fills its stdout buffer blocks until someone reads it, so
        // polling `try_wait` while nothing drains deadlocks on exactly the
        // large output this is fetching.
        let mut child = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                // Distinguished because it is the overwhelmingly common one and
                // the fix is different: "yt-dlp is not installed" is a message a
                // user can act on, where "os error 2" is not.
                std::io::ErrorKind::NotFound => {
                    RunError::Io(format!("{program} is not installed or not on PATH"))
                }
                _ => RunError::Io(format!("{program}: {e}")),
            })?;

        let mut cout = child.stdout.take();
        let mut cerr = child.stderr.take();
        let out_t = std::thread::spawn(move || {
            let mut v = Vec::new();
            if let Some(h) = cout.as_mut() {
                let _ = h.read_to_end(&mut v);
            }
            v
        });
        let err_t = std::thread::spawn(move || {
            let mut v = Vec::new();
            if let Some(h) = cerr.as_mut() {
                let _ = h.read_to_end(&mut v);
            }
            v
        });

        let deadline = std::time::Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st,
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(RunError::Io(format!(
                            "{program} did not finish within {:?}",
                            self.timeout
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(RunError::Io(format!("{program}: {e}"))),
            }
        };

        let stdout = out_t.join().unwrap_or_default();
        let stderr = err_t.join().unwrap_or_default();
        let out = std::process::Output {
            status,
            stdout,
            stderr,
        };

        if !out.status.success() {
            // yt-dlp puts the useful part on stderr; the exit code alone says
            // nothing a user can act on.
            let msg = String::from_utf8_lossy(&out.stderr);
            let tail: String = msg.lines().rev().take(3).collect::<Vec<_>>().join(" / ");
            return Err(RunError::Io(format!(
                "{program} exited {}: {}",
                out.status.code().unwrap_or(-1),
                if tail.is_empty() { "no output" } else { &tail }
            )));
        }
        Ok(out.stdout)
    }
}

/// Fetches byte ranges over plain HTTP.
///
/// # No TLS, and that is a real limit
///
/// This speaks `http://` only. It is written on `std::net::TcpStream` because
/// the monorepo has no TLS client — `shared/net-kit`'s `http` is plain TCP, and
/// its `roundtrip` returns a `String`, which would mangle the binary body of a
/// media range request anyway.
///
/// So this is enough for a local server, a LAN share, or a plain-HTTP origin,
/// and **not** enough for YouTube, which is HTTPS-only. Swapping in a
/// TLS-capable client is a change to `fetch` and nothing else: everything that
/// decides anything — the read window, the expiry handling, the section plan —
/// is above this trait and already tested.
///
/// It is written and tested rather than skipped because a transport that works
/// against a real socket, even a plain one, proves the seam fits. A stub that
/// only satisfies the trait would not.
pub struct TcpTransport {
    pub timeout: Duration,
    pub user_agent: String,
    content_length: Option<u64>,
}

impl Default for TcpTransport {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(20),
            user_agent: "vidkit".into(),
            content_length: None,
        }
    }
}

/// Split `http://host[:port]/path` into the parts a request needs.
///
/// Returned rather than parsed inline so it can be tested: a URL with a port,
/// with no path, or with a query is the sort of thing that works on the three
/// URLs someone tried and fails on the fourth.
pub fn split_url(url: &str) -> Result<(String, u16, String), HttpError> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| HttpError::Transport(format!("not a plain-http URL: {url}")))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        // No path at all is legal and means "/".
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(HttpError::Transport(format!("no host in {url}")));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|_| HttpError::Transport(format!("bad port in {url}")))?,
        ),
        None => (authority.to_string(), 80),
    };
    Ok((host, port, path.to_string()))
}

/// Split a raw response into its status code and body.
///
/// The header/body split is `\r\n\r\n`, and searching for `\n\n` instead finds
/// a false boundary inside a body often enough to matter.
pub fn split_response(raw: &[u8]) -> Result<(u16, &[u8]), HttpError> {
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| HttpError::Transport("no header terminator".into()))?;
    let head = std::str::from_utf8(&raw[..head_end])
        .map_err(|_| HttpError::Transport("non-utf8 headers".into()))?;
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| HttpError::Transport(format!("no status in {:?}", head.lines().next())))?;
    Ok((status, &raw[head_end + 4..]))
}

/// Total object size from a `Content-Range: bytes a-b/total` header.
pub fn total_from_content_range(head: &str) -> Option<u64> {
    let line = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-range:"))?;
    line.rsplit_once('/')?.1.trim().parse().ok()
}

impl HttpTransport for TcpTransport {
    fn fetch(&mut self, req: &RangeRequest) -> Result<Vec<u8>, HttpError> {
        let (host, port, path) = split_url(&req.url)?;

        let mut stream = TcpStream::connect((host.as_str(), port))
            .map_err(|e| HttpError::Transport(format!("connect {host}:{port}: {e}")))?;
        stream.set_read_timeout(Some(self.timeout)).ok();
        stream.set_write_timeout(Some(self.timeout)).ok();

        let mut head = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {}\r\nRange: {}\r\nConnection: close\r\n",
            self.user_agent,
            req.header_value(),
        );
        // The caller's headers go last so a signed URL's own User-Agent wins
        // over the default: a CDN that checks it rejects the wrong one, and a
        // 403 from a header mismatch looks exactly like an expired URL.
        for (k, v) in &req.headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");

        stream
            .write_all(head.as_bytes())
            .map_err(|e| HttpError::Transport(format!("write: {e}")))?;

        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .map_err(|e| HttpError::Transport(format!("read: {e}")))?;

        let (status, body) = split_response(&raw)?;
        match status {
            // 206 is the expected answer to a range request; 200 means the
            // server ignored it and sent the whole object, which the caller
            // detects by length and refuses.
            200 | 206 => {
                if let Ok(head_text) = std::str::from_utf8(&raw[..raw.len() - body.len()]) {
                    if let Some(t) = total_from_content_range(head_text) {
                        self.content_length = Some(t);
                    }
                }
                Ok(body.to_vec())
            }
            401 | 403 => Err(HttpError::Forbidden),
            other => Err(HttpError::Status(other)),
        }
    }

    fn known_length(&self) -> Option<u64> {
        self.content_length
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // ---- process runner --------------------------------------------------

    #[test]
    fn a_command_returns_its_stdout() {
        let mut r = ProcessRunner::default();
        let out = r.run("/bin/echo", &["hello".to_string()]).unwrap();
        assert_eq!(String::from_utf8_lossy(&out).trim(), "hello");
    }

    /// "yt-dlp is not installed" is a message someone can act on; "os error 2"
    /// is not.
    #[test]
    fn a_missing_program_says_so_plainly() {
        let mut r = ProcessRunner::default();
        let e = r.run("definitely-not-a-real-binary-xyz", &[]).unwrap_err();
        let msg = format!("{e:?}");
        assert!(
            msg.contains("not installed"),
            "unhelpful error for a missing binary: {msg}"
        );
    }

    /// A hung extractor must not freeze the UI thread forever.
    #[test]
    fn a_hung_command_is_killed_at_the_deadline() {
        let mut r = ProcessRunner {
            timeout: Duration::from_millis(200),
        };
        let start = std::time::Instant::now();
        let e = r
            .run("/bin/sh", &["-c".into(), "sleep 30".into()])
            .unwrap_err();
        let took = start.elapsed();
        assert!(
            format!("{e:?}").contains("did not finish"),
            "wrong error: {e:?}"
        );
        assert!(took < Duration::from_secs(5), "waited {took:?} for a 200ms timeout");
    }

    /// A child that fills its stdout buffer blocks until someone reads it, so
    /// polling while nothing drains deadlocks on exactly the large output this
    /// exists to fetch.
    #[test]
    fn a_large_output_does_not_deadlock() {
        let mut r = ProcessRunner {
            timeout: Duration::from_secs(20),
        };
        let out = r
            .run(
                "/bin/sh",
                &["-c".into(), "yes abcdefghij | head -c 400000".into()],
            )
            .expect("large output");
        assert_eq!(out.len(), 400_000, "output was truncated or the read stalled");
    }

    /// yt-dlp puts the useful part on stderr; an exit code alone says nothing.
    #[test]
    fn a_failing_command_reports_its_stderr() {
        let mut r = ProcessRunner::default();
        let e = r
            .run("/bin/sh", &["-c".into(), "echo the-real-reason >&2; exit 3".into()])
            .unwrap_err();
        let msg = format!("{e:?}");
        assert!(msg.contains("the-real-reason"), "stderr was dropped: {msg}");
        assert!(msg.contains('3'), "exit code missing: {msg}");
    }

    // ---- url parsing ------------------------------------------------------

    #[test]
    fn urls_split_into_host_port_and_path() {
        assert_eq!(
            split_url("http://example.org/a/b.mp4").unwrap(),
            ("example.org".into(), 80, "/a/b.mp4".into())
        );
        assert_eq!(
            split_url("http://127.0.0.1:8080/v").unwrap(),
            ("127.0.0.1".into(), 8080, "/v".into())
        );
        // No path is legal and means "/".
        assert_eq!(
            split_url("http://example.org").unwrap(),
            ("example.org".into(), 80, "/".into())
        );
        // A query belongs to the path, not the host.
        assert_eq!(
            split_url("http://h/v?id=1&t=2").unwrap().2,
            "/v?id=1&t=2".to_string()
        );
    }

    /// HTTPS is refused with a clear message rather than silently attempted as
    /// plaintext, which would leak and then fail confusingly.
    #[test]
    fn https_is_refused_not_attempted() {
        assert!(split_url("https://example.org/v").is_err());
        assert!(split_url("example.org/v").is_err());
        assert!(split_url("http://").is_err());
    }

    // ---- response parsing -------------------------------------------------

    /// Searching for "\\n\\n" finds a false boundary inside a body often
    /// enough to matter.
    #[test]
    fn the_body_starts_after_the_crlf_pair() {
        let raw = b"HTTP/1.1 206 Partial\r\nContent-Length: 3\r\n\r\n\n\nabc";
        let (status, body) = split_response(raw).unwrap();
        assert_eq!(status, 206);
        assert_eq!(body, b"\n\nabc", "split on the wrong terminator");
    }

    #[test]
    fn a_truncated_response_is_an_error_not_an_empty_body() {
        assert!(split_response(b"HTTP/1.1 206 Partial\r\nContent-Len").is_err());
    }

    #[test]
    fn the_total_size_is_read_from_content_range() {
        let head = "HTTP/1.1 206\r\nContent-Range: bytes 0-255/4096\r\n";
        assert_eq!(total_from_content_range(head), Some(4096));
        // Case varies between servers.
        assert_eq!(
            total_from_content_range("HTTP/1.1 206\r\ncontent-range: bytes 0-1/9\r\n"),
            Some(9)
        );
        // An unknown total is "*", which must not parse as a number.
        assert_eq!(
            total_from_content_range("Content-Range: bytes 0-1/*"),
            None
        );
        assert_eq!(total_from_content_range("HTTP/1.1 200\r\n"), None);
    }

    // ---- against a real socket --------------------------------------------

    /// A transport that works against a real socket proves the seam fits; a
    /// stub that only satisfies the trait would not.
    #[test]
    fn it_fetches_a_range_over_a_real_connection() {
        use std::net::TcpListener;

        let body: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let served = body.clone();

        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 2048];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();

            // Honour the Range header, so the test exercises the real path
            // rather than a server that always sends everything.
            let (start, end) = req
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                .and_then(|l| l.split('=').nth(1).map(str::trim).map(str::to_string))
                .and_then(|r| {
                    let (a, b) = r.split_once('-')?;
                    Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
                })
                .unwrap_or((0, served.len() - 1));
            let slice = &served[start..=end.min(served.len() - 1)];

            let head = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\n\r\n",
                served.len(),
                slice.len()
            );
            let _ = sock.write_all(head.as_bytes());
            let _ = sock.write_all(slice);
            req
        });

        let mut t = TcpTransport::default();
        let mut headers = BTreeMap::new();
        headers.insert("User-Agent".to_string(), "test-agent".to_string());
        let got = t
            .fetch(&RangeRequest {
                url: format!("http://127.0.0.1:{port}/clip.mp4"),
                headers,
                start: 100,
                end: 199,
            })
            .expect("fetch");

        assert_eq!(got.len(), 100, "wrong byte count");
        assert_eq!(got, body[100..200], "wrong bytes");
        assert_eq!(t.known_length(), Some(1000), "Content-Range was not read");

        let req = server.join().unwrap();
        assert!(req.contains("Range: bytes=100-199"), "no range header: {req}");
        assert!(
            req.contains("test-agent"),
            "the caller's headers were dropped: {req}"
        );
    }

    /// A 403 must arrive as `Forbidden`, not a generic status, because the
    /// layer above treats it as recoverable and re-resolves.
    #[test]
    fn a_403_is_distinguishable() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
        });

        let mut t = TcpTransport::default();
        let e = t
            .fetch(&RangeRequest {
                url: format!("http://127.0.0.1:{port}/x"),
                headers: BTreeMap::new(),
                start: 0,
                end: 10,
            })
            .unwrap_err();
        assert_eq!(e, HttpError::Forbidden, "a 403 arrived as {e:?}");
    }
}
