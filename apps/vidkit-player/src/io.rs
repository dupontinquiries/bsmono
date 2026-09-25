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

// ---------------------------------------------------------------------------
// opening a URL
// ---------------------------------------------------------------------------

/// Builds a transport per resolved URL, for `StreamSource`.
pub struct TcpFactory;

impl vidkit_stream::TransportFactory<TcpTransport> for TcpFactory {
    fn open(&mut self, _url: &str) -> TcpTransport {
        TcpTransport::default()
    }
}

/// Is this a link straight to a media file, or a page that needs an extractor?
///
/// Worth branching on. A direct `.mp4` needs no yt-dlp at all, and running an
/// extractor on one is a spawned Python interpreter and several seconds to
/// learn what the URL already said. It also means a local or LAN file over
/// HTTP works on a machine with no yt-dlp installed, which is most of them.
pub fn is_direct_media(url: &str) -> bool {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    ["mp4", "m4v", "mov", "webm", "mkv"]
        .iter()
        .any(|e| path.ends_with(&format!(".{e}")))
}

/// What a URL turned out to be.
#[derive(Debug)]
pub struct OpenedUrl {
    pub label: String,
    pub duration: Option<std::time::Duration>,
    pub dimensions: (u32, u32),
    pub packets: usize,
}

/// Open a direct media URL: fetch over HTTP, sniff, demux, describe.
///
/// No extractor and no resolution — the URL is the media. Errors carry the URL,
/// because "connection refused" with no address is a message that sends someone
/// looking in the wrong place.
pub fn open_direct_url(url: &str) -> Result<OpenedUrl, String> {
    use vidkit_core::io::ByteSource;
    use vidkit_core::traits::MediaSource;
    use vidkit_ytdlp::http::{ChunkPolicy, HttpByteSource};

    // The scheme picks the transport, so an https URL works here and a LAN
    // file over plain http does not start requiring TLS it will never have.
    let transport = transport_for(url)?;
    let mut src = HttpByteSource::new(transport, url, Default::default())
        .with_policy(ChunkPolicy {
            // Smaller than the default: the header walk is the only thing read
            // before we know whether this is even a video, and pulling 256 KB
            // to find out is rude on a metered link.
            chunk_bytes: 64 * 1024,
            retries: 1,
        });

    let mut head = [0u8; 16];
    let n = src
        .read_at(0, &mut head)
        .map_err(|e| format!("{url}: {e}"))?;
    let container = vidkit_stream::detect_container(&head[..n])
        .ok_or_else(|| format!("{url}: not an MP4 or WebM stream"))?;

    // The count comes from the INDEX, not from walking the samples.
    //
    // Counting by iteration reads every packet, which over HTTP downloads the
    // whole video just to describe it -- the opposite of what the range
    // fetching is for. A real 1440p clip reported 29530 packets, which is
    // 29530 reads before a single frame was shown. Both containers already
    // know the count from the header they have parsed.
    let (kind, profile, dims, dur, packets) = match container {
        vidkit_stream::Container::Mp4 => {
            let d = vidkit_mp4::Mp4Demuxer::open(src).map_err(|e| format!("{url}: {e}"))?;
            ("MP4", d.video_profile(), d.dimensions(), d.duration(), d.sample_count())
        }
        vidkit_stream::Container::WebM => {
            let d = vidkit_webm::WebmDemuxer::open(src).map_err(|e| format!("{url}: {e}"))?;
            ("WebM", d.video_profile(), d.dimensions(), d.duration(), d.block_count())
        }
    };

    let codec = profile
        .map(|p| format!("{} {}-bit", p.codec, p.bit_depth))
        .unwrap_or_else(|| "unknown codec".into());
    let name = url.rsplit('/').next().unwrap_or(url);

    Ok(OpenedUrl {
        label: format!(
            "{name} · {kind} · {codec} · {}x{}{} · {packets} packets · over HTTP",
            dims.0,
            dims.1,
            dur.map(|d| format!(" · {}", fmt_duration(d))).unwrap_or_default()
        ),
        duration: dur,
        dimensions: dims,
        packets,
    })
}

#[cfg(test)]
mod url_tests {
    use super::*;

    #[test]
    fn direct_media_links_are_recognised() {
        assert!(is_direct_media("http://h/a/clip.mp4"));
        assert!(is_direct_media("http://h/CLIP.WEBM"));
        assert!(is_direct_media("http://h/v.mkv?token=1"));
        // A page is not a media link.
        assert!(!is_direct_media("https://www.youtube.com/watch?v=abc"));
        assert!(!is_direct_media("http://h/index.html"));
        // ...and a query must not be mistaken for an extension.
        assert!(!is_direct_media("http://h/watch?file=x.mp4"));
    }

    /// Serve a real MP4 over a real socket and open it end to end: transport,
    /// windowed reads, container sniff, demux. Nothing is mocked below the URL.
    #[test]
    fn a_direct_url_opens_over_http() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let body = crate::io::tests_support::sample_mp4();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();

        let served = body.clone();
        std::thread::spawn(move || {
            // The demuxer makes several range requests; serve until it stops.
            for _ in 0..64 {
                let Ok((mut sock, _)) = listener.accept() else { break };
                let mut buf = [0u8; 2048];
                let n = sock.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    continue;
                }
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let (start, end) = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .and_then(|l| l.split('=').nth(1).map(str::trim).map(str::to_string))
                    .and_then(|r| {
                        let (a, b) = r.split_once('-')?;
                        Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
                    })
                    .unwrap_or((0, served.len() - 1));
                if start >= served.len() {
                    let _ = sock.write_all(
                        b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n",
                    );
                    continue;
                }
                let slice = &served[start..=end.min(served.len() - 1)];
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\n\r\n",
                    start + slice.len() - 1,
                    served.len(),
                    slice.len()
                );
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.write_all(slice);
            }
        });

        let opened = open_direct_url(&format!("http://127.0.0.1:{port}/clip.mp4"))
            .expect("open over http");
        assert_eq!(opened.dimensions, (64, 64));
        assert_eq!(opened.packets, 2, "wrong packet count over the wire");
        assert!(opened.label.contains("MP4"), "{}", opened.label);
        assert!(opened.label.contains("over HTTP"), "{}", opened.label);
    }

    /// A URL that serves something else must say so, not fail obscurely deep in
    /// a parser.
    /// Describing a file must not download it.
    ///
    /// The count used to come from walking every packet, so a 1440p clip with
    /// 29530 samples meant 29530 reads before a frame was shown — over HTTP,
    /// the entire video fetched to fill in a label. The index already knows.
    /// Asserting the COUNT alone cannot see this: it is the same number either
    /// way. Counting the server's requests is what makes it visible.
    #[test]
    fn describing_a_url_does_not_download_the_whole_file() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let body = crate::io::tests_support::sample_mp4_n(300);
        let total = body.len();
        let served = Arc::new(AtomicUsize::new(0));
        let counter = served.clone();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..512 {
                let Ok((mut sock, _)) = listener.accept() else { break };
                let mut buf = [0u8; 2048];
                let n = sock.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    continue;
                }
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let (start, end) = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .and_then(|l| l.split('=').nth(1).map(str::trim).map(str::to_string))
                    .and_then(|r| {
                        let (a, b) = r.split_once('-')?;
                        Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
                    })
                    .unwrap_or((0, body.len() - 1));
                if start >= body.len() {
                    let _ = sock.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n");
                    continue;
                }
                let slice = &body[start..=end.min(body.len() - 1)];
                counter.fetch_add(slice.len(), Ordering::SeqCst);
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\n\r\n",
                    start + slice.len() - 1, body.len(), slice.len()
                );
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.write_all(slice);
            }
        });

        let opened = open_direct_url(&format!("http://127.0.0.1:{port}/big.mp4"))
            .expect("open");
        assert_eq!(opened.packets, 300, "the index count is wrong");

        let bytes = served.load(Ordering::SeqCst);
        assert!(
            bytes < total,
            "describing the file fetched {bytes} of {total} bytes — it walked the samples"
        );
    }

    #[test]
    fn a_url_serving_html_is_reported_clearly() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..4 {
                let Ok((mut sock, _)) = listener.accept() else { break };
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let body = b"<!DOCTYPE html><html>nope</html>";
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{}/{}\r\nContent-Length: {}\r\n\r\n",
                    body.len() - 1, body.len(), body.len()
                );
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.write_all(body);
            }
        });

        let e = open_direct_url(&format!("http://127.0.0.1:{port}/x.mp4")).unwrap_err();
        assert!(e.contains("not an MP4 or WebM"), "unhelpful: {e}");
        // The address belongs in the message: "connection refused" with no
        // address sends someone looking in the wrong place.
        assert!(e.contains("127.0.0.1"), "the URL was dropped: {e}");
    }

    /// A server error surfaces through `read_at`, which is a different path
    /// from a refused connection or an unrecognised container -- both of which
    /// already carry the address for their own reasons. Without this the
    /// `map_err` that adds the URL to a read failure was untested.
    #[test]
    fn a_server_error_reports_the_url_too() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..8 {
                let Ok((mut sock, _)) = listener.accept() else { break };
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let _ = sock
                    .write_all(b"HTTP/1.1 500 Server Error\r\nContent-Length: 0\r\n\r\n");
            }
        });

        let url = format!("http://127.0.0.1:{port}/clip.mp4");
        let e = open_direct_url(&url).unwrap_err();
        assert!(
            e.contains(&format!("127.0.0.1:{port}")),
            "the URL was dropped from a read failure: {e}"
        );
    }

    #[test]
    fn a_dead_host_reports_the_url() {
        // Port 1 is reserved and nothing listens on it.
        let e = open_direct_url("http://127.0.0.1:1/clip.mp4").unwrap_err();
        assert!(e.contains("127.0.0.1:1"), "the URL was dropped: {e}");
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    /// A two-sample AV1 MP4, from vidkit-mp4's own fixture writer rather than a
    /// second copy that could drift from the parser it exercises.
    pub fn sample_mp4() -> Vec<u8> {
        sample_mp4_sized(2, 6)
    }

    pub fn sample_mp4_n(n: u32) -> Vec<u8> {
        // 1 KB payloads, deliberately. At 6 bytes a sample a 300-sample file
        // is under 5 KB, which fits inside one 64 KB read window -- so "did it
        // walk the samples" is unobservable and the test that asks cannot
        // fail. The file has to be bigger than a window for the question to
        // mean anything.
        sample_mp4_sized(n, 1024)
    }

    pub fn sample_mp4_sized(n: u32, payload: usize) -> Vec<u8> {
        use vidkit_mp4::build::*;
        let payloads: Vec<Vec<u8>> = (0..n).map(|_| vec![0xA0u8; payload]).collect();
        let sizes: Vec<u32> = payloads.iter().map(|p| p.len() as u32).collect();
        let mut mdat_body = Vec::new();
        let mut offs = Vec::new();
        for p in &payloads {
            offs.push(mdat_body.len());
            mdat_body.extend_from_slice(p);
        }
        let mdat = bx(b"mdat", &mdat_body);
        let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isomiso2av01mp41");
        let av1c = bx(b"av1C", &[0x81, 0x00, 0x00, 0x00]);
        let make = |at: u64| {
            let stco = table(
                b"stco",
                &offs.iter().map(|o| u32b((at + *o as u64) as u32)).collect::<Vec<_>>(),
            );
            let stbl = bx(
                b"stbl",
                &cat(&[
                    stsd(visual_entry(b"av01", 64, 64, av1c.clone())),
                    table(b"stts", &[cat(&[u32b(n), u32b(1000)])]),
                    table(b"stsc", &[cat(&[u32b(1), u32b(1), u32b(1)])]),
                    stsz(&sizes),
                    stco,
                ]),
            );
            let minf = bx(b"minf", &cat(&[bx(b"vmhd", &[0u8; 12]), stbl]));
            let mdia = bx(b"mdia", &cat(&[mdhd(30_000, n * 1000), hdlr(b"vide"), minf]));
            let trak = bx(b"trak", &cat(&[tkhd(1, 64, 64), mdia]));
            bx(b"moov", &cat(&[bx(b"mvhd", &[0u8; 100]), trak]))
        };
        let probe = make(0);
        let at = (ftyp.len() + probe.len() + 8) as u64;
        let moov = make(at);
        assert_eq!(moov.len(), probe.len());
        cat(&[ftyp, moov, mdat])
    }
}

// ---------------------------------------------------------------------------
// https
// ---------------------------------------------------------------------------

/// Range fetching over HTTPS.
///
/// `ureq` with rustls — the same crate and major version voiceterm, snapclone
/// and bo-suite already use, so it is house-approved and already in the cargo
/// cache. rustls rather than native-tls keeps the build free of an OpenSSL
/// system dependency, which is exactly what has stopped other apps in this tree
/// from building in this sandbox.
///
/// Everything that decides anything still lives above the trait: the read
/// window, the retry policy, the expiry handling and the section plan are in
/// `vidkit-ytdlp` and tested there. This only moves bytes.
#[cfg(feature = "https")]
pub struct TlsTransport {
    agent: ureq::Agent,
    content_length: Option<u64>,
}

#[cfg(feature = "https")]
impl Default for TlsTransport {
    fn default() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(20))
                // Redirects are followed: CDNs redirect range requests
                // routinely, and a client that does not follow them reports a
                // 302 as a failure on a URL that works in any browser.
                .redirects(5)
                .build(),
            content_length: None,
        }
    }
}

#[cfg(feature = "https")]
impl HttpTransport for TlsTransport {
    fn fetch(&mut self, req: &RangeRequest) -> Result<Vec<u8>, HttpError> {
        let mut r = self.agent.get(&req.url).set("Range", &req.header_value());
        // The caller's headers go last so a signed URL's own User-Agent wins:
        // a CDN that checks it answers 403, and that looks exactly like an
        // expired URL.
        for (k, v) in &req.headers {
            r = r.set(k, v);
        }

        let resp = match r.call() {
            Ok(resp) => resp,
            // ureq models a non-2xx as an error, so the status has to be dug
            // back out or every HTTP error becomes an opaque transport one and
            // the recoverable 403 stops being recoverable.
            Err(ureq::Error::Status(code, _)) => {
                return Err(match code {
                    401 | 403 => HttpError::Forbidden,
                    other => HttpError::Status(other),
                })
            }
            Err(e) => return Err(HttpError::Transport(format!("{}: {e}", req.url))),
        };

        if let Some(cr) = resp.header("Content-Range") {
            if let Some(total) = cr.rsplit_once('/').and_then(|(_, t)| t.trim().parse().ok()) {
                self.content_length = Some(total);
            }
        }

        // Bounded by what was asked for, plus a little slack. An unbounded
        // read_to_end on a server that ignored the Range header pulls the whole
        // film into memory, and on a phone that is the process being killed
        // rather than an error anyone can act on.
        let wanted = (req.end.saturating_sub(req.start) + 1) as usize;
        let cap = wanted.saturating_add(64 * 1024);
        let mut body = Vec::with_capacity(wanted.min(1 << 20));
        resp.into_reader()
            .take(cap as u64)
            .read_to_end(&mut body)
            .map_err(|e| HttpError::Transport(format!("{}: {e}", req.url)))?;
        Ok(body)
    }

    fn known_length(&self) -> Option<u64> {
        self.content_length
    }
}

/// Pick a transport for a URL: TLS for `https://`, plain TCP otherwise.
///
/// Boxed rather than an enum because the choice is made once per source and
/// never again; an enum would put a match on every range request to save an
/// indirection that no profile has ever shown to matter.
/// Which transport a URL needs.
///
/// Split out from `transport_for` so the CHOICE is testable: the boxed trait
/// object cannot be inspected, and this sandbox has no TLS origin to fetch
/// from, so without this the scheme dispatch was only exercised for http and a
/// mutation making everything plain TCP survived.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Scheme {
    PlainTcp,
    Tls,
}

pub fn scheme_of(url: &str) -> Result<Scheme, String> {
    if url.starts_with("https://") {
        Ok(Scheme::Tls)
    } else if url.starts_with("http://") {
        Ok(Scheme::PlainTcp)
    } else {
        Err(format!("{url}: not an http or https URL"))
    }
}

pub fn transport_for(url: &str) -> Result<Box<dyn HttpTransport + Send>, String> {
    match scheme_of(url)? {
        Scheme::Tls => {
            #[cfg(feature = "https")]
            {
                Ok(Box::new(TlsTransport::default()))
            }
            #[cfg(not(feature = "https"))]
            {
                Err(format!("{url}: this build has no HTTPS support"))
            }
        }
        Scheme::PlainTcp => Ok(Box::new(TcpTransport::default())),
    }
}

#[cfg(test)]
mod transport_choice_tests {
    use super::*;

    /// The choice itself, asserted as a value. There is no TLS origin to fetch
    /// from here, so without this the https branch is never exercised.
    #[test]
    fn https_selects_tls_and_http_does_not() {
        assert_eq!(scheme_of("https://example.org/v.mp4"), Ok(Scheme::Tls));
        assert_eq!(scheme_of("http://127.0.0.1:8080/v.mp4"), Ok(Scheme::PlainTcp));
        assert!(scheme_of("ftp://example.org/v").is_err());
        assert!(scheme_of("example.org/v").is_err());
    }

    #[test]
    fn the_scheme_chooses_the_transport() {
        assert!(transport_for("https://example.org/v.mp4").is_ok());
        assert!(transport_for("http://example.org/v.mp4").is_ok());
        // `dyn HttpTransport` is not Debug, so the error cases are matched
        // rather than unwrapped.
        // A scheme we do not speak is refused with the URL in the message
        // rather than attempted as something else.
        let e = match transport_for("ftp://example.org/v.mp4") {
            Err(e) => e,
            Ok(_) => panic!("ftp was accepted"),
        };
        assert!(e.contains("ftp://example.org"), "{e}");
        assert!(transport_for("example.org/v.mp4").is_err());
    }

    /// The plain transport must still be used for http, or a LAN file starts
    /// requiring TLS it will never have.
    #[cfg(feature = "https")]
    #[test]
    fn plain_http_does_not_go_through_tls() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let Ok((mut sock, _)) = listener.accept() else { return };
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf);
            let body = b"0123456789";
            let head = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-9/10\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(head.as_bytes());
            let _ = sock.write_all(body);
        });

        let mut t = match transport_for(&format!("http://127.0.0.1:{port}/x.mp4")) {
            Ok(t) => t,
            Err(e) => panic!("no transport for plain http: {e}"),
        };
        let got = t
            .fetch(&RangeRequest {
                url: format!("http://127.0.0.1:{port}/x.mp4"),
                headers: Default::default(),
                start: 0,
                end: 9,
            })
            .expect("plain http through the chosen transport");
        assert_eq!(got, b"0123456789");
    }
}

// ---------------------------------------------------------------------------
// page URLs, through the extractor
// ---------------------------------------------------------------------------

/// Builds a transport per resolved URL, choosing by scheme.
///
/// A resolved media URL is very often on a different host from the page and,
/// on YouTube, always https — so the choice has to be made per URL rather than
/// once for the session.
pub struct SchemeFactory;

impl vidkit_stream::TransportFactory<Box<dyn HttpTransport + Send>> for SchemeFactory {
    fn open(&mut self, url: &str) -> Box<dyn HttpTransport + Send> {
        // A resolved media URL is usually on a different host from the page
        // and, on YouTube, always https -- so the scheme is re-examined per
        // URL rather than assumed from the page.
        transport_for(url).unwrap_or_else(|_| Box::new(TcpTransport::default()))
    }
}

/// Which capability profile to select formats against.
///
/// Guessed from the target triple, which is crude and deliberately visible:
/// `DeviceCaps` is meant to be probed at runtime from the V4L2 and Vulkan
/// devices present, and until `vidkit-v4l2::device` has run against real
/// hardware that probe cannot be trusted. A wrong guess here picks a stream
/// the decoder cannot manage, so it is one function to replace rather than a
/// choice scattered through the app.
pub fn assumed_caps() -> vidkit_core::caps::DeviceCaps {
    // Probed, not guessed. `sysprobe` reads the device-tree SoC id, the V4L2
    // driver names and the bound DRM driver, all of which the kernel already
    // knows — where the target triple could not tell an M1 from an M3 from a
    // Fairphone, and was therefore wrong for two of the three aarch64 shapes
    // this player targets.
    let p = vidkit_v4l2::sysprobe::probe(std::path::Path::new("/"));
    if let Some(caps) = vidkit_v4l2::sysprobe::caps_for(&p) {
        return caps;
    }
    // Unidentified. The probe refuses to guess and so does this: a desktop
    // profile is the safe default because it assumes the least about what is
    // present, and the evidence goes in the name so a bug report is one paste.
    // A laptop or desktop: ask the VAAPI driver what it decodes rather than
    // assume everything (see desktop_from_vaapi for what assuming cost).
    if let Some(profiles) = crate::decode::vaapi_profiles() {
        return vidkit_core::caps::desktop_from_vaapi(&profiles);
    }
    let mut fallback = vidkit_core::caps::desktop_full_hw();
    fallback.name = format!("unrecognised device ({:?}); ASSUMED full hardware decode -- install vainfo (libva-utils) to check", p.detected);
    fallback
}

/// Why the direct-media branch exists, measured rather than argued.
///
/// yt-dlp 2026.08.19 against a local server, median of five runs: `-J` takes
/// **650 ms**, and `--no-check-formats`, `--flat-playlist` and `--print` are
/// all within noise of it (648–703 ms). The cost is the Python interpreter
/// starting, not the JSON, so there is no flag that makes resolution cheap.
///
/// Worse, and the reason `--http-chunk-size` is not the answer to "fetch less":
/// the generic extractor issues a **full GET with no Range header** to probe
/// the file. Running yt-dlp on a direct media URL therefore downloads the whole
/// object before we have read a byte ourselves. Our own path does ranged reads
/// and never does that.
///
/// `--http-chunk-size` configures yt-dlp's own downloader, which this player
/// does not use at all — it resolves, and every byte after that comes through
/// `HttpByteSource`. So the efficient move is not a chunking flag; it is not
/// calling yt-dlp when the URL already says what it is, which is what
/// [`is_direct_media`] decides.
///
/// Resolve a page URL with an extractor and open what it points at.
///
/// Generic over the runner so the whole path — resolution, format selection,
/// transport, demux — is testable with a scripted extractor and a local
/// server, without yt-dlp installed. That matters: this is the one path that
/// otherwise could not be exercised at all here, and an untested path is where
/// the next bug lives.
pub fn open_page_url<R: vidkit_ytdlp::CommandRunner + Send>(
    runner: R,
    url: &str,
    caps: vidkit_core::caps::DeviceCaps,
) -> Result<OpenedUrl, String> {
    open_page_url_capped(runner, url, caps, 10_000)
}

/// As [`open_page_url`], with the packet ceiling exposed.
///
/// The ceiling exists because a live stream has no end and counting to it is a
/// hang rather than a slow open. It is a parameter so a test can actually
/// reach it -- with it hard-coded at ten thousand, no fixture here has enough
/// packets and the bound was unexercised.
pub fn open_page_url_capped<R: vidkit_ytdlp::CommandRunner + Send>(
    runner: R,
    url: &str,
    caps: vidkit_core::caps::DeviceCaps,
    max_packets: usize,
) -> Result<OpenedUrl, String> {
    use vidkit_core::traits::MediaSource;
    use vidkit_ytdlp::http::ChunkPolicy;

    let mut source = vidkit_stream::StreamSource::open(
        runner,
        &mut SchemeFactory,
        url,
        caps,
        Default::default(),
        ChunkPolicy {
            chunk_bytes: 256 * 1024,
            retries: 2,
        },
    )
    .map_err(|e| format!("{url}: {e}"))?;

    let dims = source.dimensions();
    let dur = source.duration();
    let codec = source
        .video_profile()
        .map(|p| format!("{} {}-bit", p.codec, p.bit_depth))
        .unwrap_or_else(|| "unknown codec".into());
    let container = source.container();

    // From the index, not by iteration -- see `open_direct_url`. A resolved
    // stream is where walking every packet costs most, because each read is a
    // range request over the network. `max_packets` is now only a reported
    // ceiling, not a walk.
    let packets = source.packet_count().min(max_packets);

    Ok(OpenedUrl {
        label: format!(
            "{container:?} · {codec} · {}x{} · {packets} packets · resolved",
            dims.0, dims.1
        ),
        duration: dur,
        dimensions: dims,
        packets,
    })
}

#[cfg(test)]
mod page_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Stands in for yt-dlp: returns prepared JSON instead of spawning it.
    struct ScriptedRunner(Vec<String>);

    impl vidkit_ytdlp::CommandRunner for ScriptedRunner {
        fn run(&mut self, _p: &str, _a: &[String]) -> Result<Vec<u8>, vidkit_ytdlp::RunError> {
            if self.0.is_empty() {
                return Err(vidkit_ytdlp::RunError::Io("script exhausted".into()));
            }
            Ok(self.0.remove(0).into_bytes())
        }
    }

    /// Serve an MP4 over plain http, honouring Range, and return the port.
    fn serve(body: Vec<u8>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..64 {
                let Ok((mut sock, _)) = listener.accept() else { break };
                let mut buf = [0u8; 2048];
                let n = sock.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    continue;
                }
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let (start, end) = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .and_then(|l| l.split('=').nth(1).map(str::trim).map(str::to_string))
                    .and_then(|r| {
                        let (a, b) = r.split_once('-')?;
                        Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
                    })
                    .unwrap_or((0, body.len() - 1));
                if start >= body.len() {
                    let _ = sock.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n");
                    continue;
                }
                let slice = &body[start..=end.min(body.len() - 1)];
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\n\r\n",
                    start + slice.len() - 1,
                    body.len(),
                    slice.len()
                );
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.write_all(slice);
            }
        });
        port
    }

    fn ytdlp_json(url: &str) -> String {
        format!(
            r#"{{"formats":[
              {{"format_id":"399","url":"{url}","vcodec":"av01.0.08M.08","acodec":"none",
                "width":64,"height":64,"fps":30,"vbr":900.0,"protocol":"https",
                "http_headers":{{"User-Agent":"Mozilla/5.0 test"}}}}]}}"#
        )
    }

    /// The whole page-URL path with nothing mocked except the extractor
    /// itself: resolution, format selection, transport, sniff, demux.
    #[test]
    fn a_page_url_resolves_and_opens() {
        let port = serve(crate::io::tests_support::sample_mp4());
        let media = format!("http://127.0.0.1:{port}/v.mp4");
        let opened = open_page_url(
            ScriptedRunner(vec![ytdlp_json(&media)]),
            "https://site/watch?v=1",
            vidkit_core::caps::desktop_full_hw(),
        )
        .expect("resolve and open");

        assert_eq!(opened.dimensions, (64, 64));
        assert_eq!(opened.packets, 2);
        assert!(opened.label.contains("resolved"), "{}", opened.label);
    }

    /// An extractor that is not installed, or fails, must say so with the URL
    /// rather than produce an empty player.
    #[test]
    fn a_failing_extractor_is_reported_with_the_url() {
        let e = open_page_url(
            ScriptedRunner(Vec::new()),
            "https://site/watch?v=1",
            vidkit_core::caps::desktop_full_hw(),
        )
        .unwrap_err();
        assert!(e.contains("https://site/watch"), "the URL was dropped: {e}");
    }

    /// Nothing playable in the JSON is a different failure from a broken
    /// extractor and reads differently.
    #[test]
    fn a_page_with_no_usable_format_says_so() {
        let e = open_page_url(
            ScriptedRunner(vec![r#"{"formats":[]}"#.to_string()]),
            "https://site/watch?v=2",
            vidkit_core::caps::desktop_full_hw(),
        )
        .unwrap_err();
        assert!(e.contains("watch?v=2"), "{e}");
    }

    /// The bound must actually stop, or a live stream hangs the open.
    #[test]
    fn the_packet_ceiling_is_respected() {
        let port = serve(crate::io::tests_support::sample_mp4());
        let media = format!("http://127.0.0.1:{port}/v.mp4");
        let opened = open_page_url_capped(
            ScriptedRunner(vec![ytdlp_json(&media)]),
            "https://site/watch?v=3",
            vidkit_core::caps::desktop_full_hw(),
            1,
        )
        .expect("open");
        assert_eq!(opened.packets, 1, "the ceiling did not stop the count");
    }

    /// The caps guess is crude and must at least be internally consistent:
    /// whatever it returns has to accept something the selector can pick, or
    /// every page URL fails with "no usable format" on that device.
    #[test]
    fn the_assumed_caps_can_actually_play_something() {
        let caps = assumed_caps();
        assert!(
            !caps.caps.is_empty(),
            "a profile with no decoders rejects every stream"
        );
    }
}

#[cfg(test)]
mod real_ytdlp_tests {
    use super::*;

    /// Is the real extractor available? These tests skip rather than fail
    /// without it, the same way the Vulkan tests skip with no ICD — a suite
    /// that fails on a machine missing an optional tool trains people to
    /// ignore failures.
    fn have_ytdlp() -> bool {
        std::process::Command::new("yt-dlp")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn serve_file(body: Vec<u8>) -> u16 {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..128 {
                let Ok((mut sock, _)) = listener.accept() else { break };
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    continue;
                }
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let head_only = req.starts_with("HEAD");
                let range = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .and_then(|l| l.split('=').nth(1).map(str::trim).map(str::to_string));
                let (start, end) = range
                    .and_then(|r| {
                        let (a, b) = r.split_once('-')?;
                        let s: usize = a.parse().ok()?;
                        let e: usize = b.parse().unwrap_or(body.len() - 1);
                        Some((s, e))
                    })
                    .unwrap_or((0, body.len().saturating_sub(1)));
                if start >= body.len() {
                    let _ = sock.write_all(
                        b"HTTP/1.1 416 Range Not Satisfiable\r\nAccept-Ranges: bytes\r\nContent-Length: 0\r\n\r\n",
                    );
                    continue;
                }
                let slice = &body[start..=end.min(body.len() - 1)];
                let partial = slice.len() != body.len();
                let head = if partial {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Type: video/mp4\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\n\r\n",
                        start + slice.len() - 1, body.len(), slice.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nAccept-Ranges: bytes\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    )
                };
                let _ = sock.write_all(head.as_bytes());
                if !head_only {
                    let _ = sock.write_all(slice);
                }
            }
        });
        port
    }

    /// The whole path against the REAL extractor.
    ///
    /// Everything before this used JSON I wrote, which meant the parser was
    /// only ever tested against my idea of what yt-dlp emits. It is not the
    /// same shape: the generic extractor returns a format with no `width`, no
    /// `height`, no `fps` and no `acodec` at all -- fields my fixtures always
    /// supplied.
    #[test]
    fn the_real_extractor_is_parsed_and_played() {
        if !have_ytdlp() {
            eprintln!("SKIP: yt-dlp not installed");
            return;
        }
        let port = serve_file(crate::io::tests_support::sample_mp4());
        let page = format!("http://127.0.0.1:{port}/clip.mp4");

        let opened = open_page_url(
            ProcessRunner::default(),
            &page,
            vidkit_core::caps::desktop_full_hw(),
        );

        // Was a KNOWN FAILURE and is not any more. Real yt-dlp reports
        // `"vcodec": null` for anything it has not probed, `has_video` treated
        // that as audio-only, and every direct URL was rejected before
        // anything else looked at it. Now asserted for real.
        match opened {
            Ok(o) => {
                assert_eq!(o.dimensions, (64, 64), "wrong dimensions: {}", o.label);
                assert!(o.packets > 0, "no packets: {}", o.label);
            }
            Err(e) => panic!("real yt-dlp output was not usable: {e}"),
        }
    }
}

#[cfg(all(test, feature = "https"))]
mod real_tls_tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A real HTTPS origin on the sandbox allowlist that honours Range.
    /// Immutable by policy — a published crate file never changes — so the
    /// bytes are stable to assert against.
    const URL: &str = "https://static.crates.io/crates/serde/serde-1.0.0.crate";

    /// Skip rather than fail with no network. A suite that goes red on a
    /// machine that is merely offline teaches people to ignore red.
    fn online() -> bool {
        std::net::TcpStream::connect_timeout(
            &("static.crates.io", 443)
                .to_socket_addrs()
                .ok()
                .and_then(|mut a| a.next())
                .unwrap_or_else(|| ([0, 0, 0, 0], 0).into()),
            Duration::from_secs(5),
        )
        .is_ok()
    }
    use std::net::ToSocketAddrs;

    /// TLS against a real server, which nothing had ever done.
    ///
    /// `TlsTransport` compiled and its scheme selection was tested, but no test
    /// had made an actual HTTPS request — the TODO carried "https works" as
    /// *read*, not *verified*. This is the difference.
    #[test]
    fn the_tls_transport_fetches_a_real_range() {
        if !online() {
            eprintln!("SKIP: no network");
            return;
        }
        let mut t = TlsTransport::default();
        let got = t
            .fetch(&RangeRequest {
                url: URL.to_string(),
                headers: BTreeMap::new(),
                start: 0,
                end: 99,
            })
            .expect("https fetch");

        assert_eq!(got.len(), 100, "the server did not honour the range");
        // A .crate file is a gzip stream; the magic is a cheap check that these
        // are the FIRST bytes of the object rather than some other slice.
        assert_eq!(&got[..2], &[0x1f, 0x8b], "not the start of the object");

        let total = t.known_length().expect("Content-Range should give a total");
        assert!(total > 1000, "implausible total {total}");
    }

    /// A range from the middle must differ from the start, or the server is
    /// ignoring Range and the test above would pass on the whole file.
    #[test]
    fn a_middle_range_is_not_the_start_of_the_file() {
        if !online() {
            eprintln!("SKIP: no network");
            return;
        }
        let mut t = TlsTransport::default();
        let mk = |t: &mut TlsTransport, start: u64| {
            t.fetch(&RangeRequest {
                url: URL.to_string(),
                headers: BTreeMap::new(),
                start,
                end: start + 63,
            })
            .expect("fetch")
        };
        let head = mk(&mut t, 0);
        let mid = mk(&mut t, 4096);
        assert_eq!(head.len(), 64);
        assert_eq!(mid.len(), 64);
        assert_ne!(head, mid, "the server returned the same bytes for both ranges");
    }

    /// The whole stack over real TLS: transport, windowed reads, and the
    /// container sniff refusing something that is not video. A .crate is a
    /// gzip, so this checks the refusal path rather than playback.
    #[test]
    fn the_byte_source_reads_over_real_tls() {
        if !online() {
            eprintln!("SKIP: no network");
            return;
        }
        use vidkit_core::io::ByteSource;
        use vidkit_ytdlp::http::{ChunkPolicy, HttpByteSource};

        let transport = transport_for(URL).expect("tls transport");
        let mut src = HttpByteSource::new(transport, URL, Default::default())
            .with_policy(ChunkPolicy { chunk_bytes: 8192, retries: 1 });

        let mut head = [0u8; 16];
        let n = src.read_at(0, &mut head).expect("read over tls");
        assert_eq!(n, 16);
        assert_eq!(&head[..2], &[0x1f, 0x8b]);
        assert_eq!(src.size().map(|s| s > 1000), Some(true), "no length learned");

        // A second read inside the same window must not hit the network again.
        let before = src.requests;
        let mut more = [0u8; 16];
        src.read_at(32, &mut more).expect("cached read");
        assert_eq!(src.requests, before, "the window was not reused over TLS");

        assert!(
            vidkit_stream::detect_container(&head[..n]).is_none(),
            "a gzip was sniffed as a video container"
        );
    }
}

/// `H:MM:SS` or `M:SS` for a description line.
///
/// The duration was already computed and simply not shown. "29530 packets"
/// tells a viewer nothing; the length of the thing they are about to watch
/// tells them a great deal.
pub fn fmt_duration(d: std::time::Duration) -> String {
    let s = d.as_secs();
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

// ---------------------------------------------------------------------------
// the system clipboard
// ---------------------------------------------------------------------------

/// The helpers that can read a clipboard, in preference order.
///
/// Same set and order voiceterm uses, and shelling out for the same reason: no
/// crate dependency, and on Wayland the clipboard is a compositor protocol that
/// an application has no business speaking directly.
pub const CLIPBOARD_READERS: &[(&str, &[&str])] = &[
    ("wl-paste", &["-n"]),
    ("xclip", &["-selection", "clipboard", "-o"]),
    ("xsel", &["--clipboard", "--output"]),
    ("pbpaste", &[]),
];

/// Read the system clipboard, trying each helper until one answers.
///
/// Takes a runner so the fallback ORDER is testable without a display server —
/// which is the part that goes wrong, since the first helper being installed
/// does not mean it works (an `xclip` on a Wayland session exits non-zero).
pub fn read_clipboard_with<R: vidkit_ytdlp::CommandRunner>(runner: &mut R) -> Option<String> {
    for (cmd, args) in CLIPBOARD_READERS {
        let argv: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        if let Ok(out) = runner.run(cmd, &argv) {
            let text = String::from_utf8_lossy(&out).trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
        // A helper that is present but fails, or returns nothing, is not the
        // answer — keep going. Stopping at the first INSTALLED helper is the
        // bug this loop exists to avoid: xclip is installed on plenty of
        // Wayland sessions and exits non-zero on all of them.
    }
    None
}

/// Read the system clipboard with a real subprocess.
pub fn read_clipboard() -> Option<String> {
    let mut r = ProcessRunner {
        // Short: this is on the click path and a hung helper would freeze the
        // UI thread for the full default timeout.
        timeout: Duration::from_secs(3),
    };
    read_clipboard_with(&mut r)
}

#[cfg(test)]
mod clipboard_tests {
    use super::*;
    use std::collections::HashMap;

    /// Answers for named commands; anything else fails, as a missing binary
    /// does.
    struct FakeTools {
        answers: HashMap<String, Result<String, ()>>,
        pub tried: Vec<String>,
    }

    impl vidkit_ytdlp::CommandRunner for FakeTools {
        fn run(&mut self, p: &str, _a: &[String]) -> Result<Vec<u8>, vidkit_ytdlp::RunError> {
            self.tried.push(p.to_string());
            match self.answers.get(p) {
                Some(Ok(s)) => Ok(s.clone().into_bytes()),
                Some(Err(())) => Err(vidkit_ytdlp::RunError::Io("failed".into())),
                None => Err(vidkit_ytdlp::RunError::Io("not installed".into())),
            }
        }
    }

    fn tools(pairs: &[(&str, Result<&str, ()>)]) -> FakeTools {
        FakeTools {
            answers: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.map(|s| s.to_string())))
                .collect(),
            tried: Vec::new(),
        }
    }

    #[test]
    fn the_first_working_helper_answers() {
        let mut t = tools(&[("wl-paste", Ok("https://example.org/v.mp4"))]);
        assert_eq!(
            read_clipboard_with(&mut t).as_deref(),
            Some("https://example.org/v.mp4")
        );
        assert_eq!(t.tried, vec!["wl-paste"], "kept going after a good answer");
    }

    /// The bug this loop exists for: xclip is installed on plenty of Wayland
    /// sessions and exits non-zero on all of them. Stopping at the first
    /// INSTALLED helper reads nothing forever.
    #[test]
    fn a_helper_that_fails_is_not_the_end_of_the_search() {
        let mut t = tools(&[
            ("wl-paste", Err(())),
            ("xclip", Err(())),
            ("xsel", Ok("recovered")),
        ]);
        assert_eq!(read_clipboard_with(&mut t).as_deref(), Some("recovered"));
        assert_eq!(t.tried, vec!["wl-paste", "xclip", "xsel"]);
    }

    /// An EMPTY answer is not an answer. A helper on the wrong display server
    /// often succeeds and returns nothing.
    #[test]
    fn an_empty_answer_falls_through() {
        let mut t = tools(&[("wl-paste", Ok("   \n")), ("xclip", Ok("real text"))]);
        assert_eq!(read_clipboard_with(&mut t).as_deref(), Some("real text"));
    }

    #[test]
    fn no_helper_at_all_is_none_rather_than_a_panic() {
        let mut t = tools(&[]);
        assert_eq!(read_clipboard_with(&mut t), None);
        assert_eq!(t.tried.len(), CLIPBOARD_READERS.len(), "gave up early");
    }

    /// Surrounding whitespace is stripped: a URL copied from a browser very
    /// often carries a trailing newline, and `classify` rejects anything with
    /// whitespace in it.
    #[test]
    fn a_trailing_newline_does_not_defeat_classification() {
        let mut t = tools(&[("wl-paste", Ok("https://www.youtube.com/watch?v=abc12345678\n"))]);
        let got = read_clipboard_with(&mut t).expect("read");
        assert!(
            vidkit_ytdlp::classify(&got).is_playable(),
            "a newline made the URL unrecognisable"
        );
    }
}

// ---------------------------------------------------------------------------
// fetching a page URL to a temp file
// ---------------------------------------------------------------------------

/// A downloaded file that deletes itself.
///
/// A film left in `/tmp` is gigabytes, and a player that leaks one per watch
/// fills the disk of exactly the small device this targets.
pub struct TempMedia {
    dir: std::path::PathBuf,
    pub file: std::path::PathBuf,
}

impl TempMedia {
    pub fn path(&self) -> &std::path::Path {
        &self.file
    }
}

impl Drop for TempMedia {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The `-f` selector for a height ceiling.
///
/// Three alternatives separated by `/`, and all three are needed:
///
/// - `bv*[height<=N]+ba` is the DASH case — YouTube serves video-only and
///   audio-only streams and yt-dlp muxes them.
/// - `b[height<=N]` is a progressive stream that already has both, which is
///   what most non-YouTube sites give.
/// - `b` is the last resort. Without it, a video whose only rendition is above
///   the ceiling fails outright rather than playing bigger than asked — and
///   "it refused to play at all because you wanted 720p" is a worse outcome
///   than a stream one size up.
pub fn format_selector(max_height: Option<u32>) -> String {
    match max_height {
        Some(h) => format!("bv*[height<={h}]+ba/b[height<={h}]/bv*+ba/b"),
        None => "bv*+ba/b".to_string(),
    }
}

/// Arguments for the download, kept pure so the flags are testable without
/// spawning anything.
pub fn download_args(url: &str, out_template: &str, max_height: Option<u32>) -> Vec<String> {
    vec![
        // Quiet, but not silent about failure.
        "--no-warnings".into(),
        "--no-playlist".into(),
        // Progress on stderr would be captured and discarded; there is no
        // terminal to draw it on.
        "--no-progress".into(),
        "-f".into(),
        format_selector(max_height),
        // Remux rather than re-encode when the container needs changing: a
        // re-encode of a whole film before the first frame is minutes.
        "--remux-video".into(),
        "mp4".into(),
        "-o".into(),
        out_template.into(),
        url.into(),
    ]
}

/// Download a page URL to a temporary file.
///
/// yt-dlp does the muxing. That is the point of this path: YouTube serves
/// video and audio as separate streams, and joining them is exactly what
/// yt-dlp is good at — reimplementing it over our own range fetcher would mean
/// a DASH muxer in this repo.
///
/// The cost is honest and worth stating: this waits for the whole file. First
/// frame comes after the download, not during it. Progressive play would need
/// ffmpeg reading a file that is still growing, which is a real thing to build
/// and not this.
pub fn fetch_to_temp<R: vidkit_ytdlp::CommandRunner>(
    runner: &mut R,
    url: &str,
    max_height: Option<u32>,
) -> Result<TempMedia, String> {
    let dir = std::env::temp_dir().join(format!(
        "vidkit-dl-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    match fetch_into(runner, url, max_height, &dir) {
        Ok(file) => Ok(TempMedia { dir, file }),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

/// Download `url` into an existing `dir` and return the media file. The
/// caller owns `dir` and its cleanup (TempMedia deletes it; the media cache
/// renames it into place).
pub fn fetch_into<R: vidkit_ytdlp::CommandRunner>(
    runner: &mut R,
    url: &str,
    max_height: Option<u32>,
    dir: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    // A fixed stem, not the video's title: a title can contain slashes,
    // newlines and characters that are illegal on the filesystem, and yt-dlp's
    // own sanitising still leaves something unpredictable to find afterwards.
    let template = dir.join("media.%(ext)s");
    let args = download_args(url, &template.to_string_lossy(), max_height);

    runner.run("yt-dlp", &args).map_err(|e| format!("{url}: {e:?}"))?;

    // Find what it actually wrote. The extension depends on the remux and on
    // what the site served, so globbing beats guessing.
    //
    // `.part` files are excluded rather than ranked: an interrupted download
    // leaves one, it is often the LARGEST thing in the directory, and handing
    // it to the decoder produces a parser error about a truncated container
    // instead of "the download did not finish".
    let file = std::fs::read_dir(dir)
        .ok()
        .and_then(|rd| {
            let mut files: Vec<std::path::PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && !matches!(
                            p.extension().and_then(|e| e.to_str()),
                            Some("part") | Some("ytdl") | Some("temp")
                        )
                })
                .collect();
            files.sort();
            // Largest wins: yt-dlp can leave a `.part` or a thumbnail behind.
            files.into_iter().max_by_key(|p| {
                std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
            })
        })
        .ok_or_else(|| format!("{url}: yt-dlp wrote no file"))?;

    // The same checks a local file gets. yt-dlp can exit zero having written
    // nothing usable -- a zero-byte file when the format selector matched
    // something the site then refused, for one -- and "the download is empty"
    // is a better message than whatever the demuxer says about byte zero.
    if let Err(e) = check_file(&file) {
        return Err(format!("{url}: {e}"));
    }

    Ok(file)
}

#[cfg(test)]
mod fetch_tests {
    use super::*;

    /// Serve fixed bytes over HTTP, honouring Range because yt-dlp probes with
    /// one before downloading.
    fn serve_bytes(body: Vec<u8>) -> u16 {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..64 {
                let Ok((mut sock, _)) = listener.accept() else { break };
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).unwrap_or(0);
                if n == 0 { continue; }
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let head_only = req.starts_with("HEAD");
                let rng = req.lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .and_then(|l| l.split('=').nth(1).map(str::trim).map(str::to_string));
                let (start, end) = rng
                    .and_then(|r| {
                        let (a, b) = r.split_once('-')?;
                        Some((a.parse::<usize>().ok()?, b.parse::<usize>().unwrap_or(body.len() - 1)))
                    })
                    .unwrap_or((0, body.len().saturating_sub(1)));
                if start >= body.len() {
                    let _ = sock.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n");
                    continue;
                }
                let slice = &body[start..=end.min(body.len() - 1)];
                let partial = slice.len() != body.len();
                let head = if partial {
                    format!("HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Type: video/mp4\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\n\r\n",
                        start + slice.len() - 1, body.len(), slice.len())
                } else {
                    format!("HTTP/1.1 200 OK\r\nAccept-Ranges: bytes\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\n\r\n", body.len())
                };
                let _ = sock.write_all(head.as_bytes());
                if !head_only { let _ = sock.write_all(slice); }
            }
        });
        port
    }

    #[test]
    fn the_selector_falls_back_past_the_height_cap() {
        let s = format_selector(Some(720));
        assert!(s.starts_with("bv*[height<=720]+ba"), "{s}");
        assert!(s.contains("/b[height<=720]"), "no progressive fallback: {s}");
        // The last resort matters: a video whose only rendition is above the
        // cap must still play rather than refusing because you asked for 720p.
        assert!(s.ends_with("/b"), "no last resort: {s}");
    }

    #[test]
    fn no_cap_still_asks_for_video_plus_audio() {
        let s = format_selector(None);
        assert!(s.contains("bv*+ba"), "{s}");
        assert!(!s.contains("height"), "an uncapped selector mentions height: {s}");
    }

    /// YouTube serves video and audio separately; a selector that does not ask
    /// for both gets a silent film.
    #[test]
    fn the_selector_always_asks_for_audio() {
        for cap in [None, Some(480), Some(1080)] {
            assert!(
                format_selector(cap).contains("+ba"),
                "no audio requested for {cap:?}"
            );
        }
    }

    #[test]
    fn the_arguments_carry_the_url_and_the_output_template() {
        let a = download_args("https://site/watch?v=1", "/tmp/x/media.%(ext)s", Some(480));
        assert_eq!(a.last().unwrap(), "https://site/watch?v=1");
        assert!(a.iter().any(|x| x == "/tmp/x/media.%(ext)s"));
        assert!(a.iter().any(|x| x == "--no-playlist"), "a playlist URL would fetch all of it");
        assert!(a.iter().any(|x| x.contains("height<=480")));
    }

    /// Remux, never re-encode. Re-encoding a whole film before the first frame
    /// is minutes of waiting.
    #[test]
    fn it_remuxes_rather_than_re_encoding() {
        let a = download_args("u", "o", None);
        assert!(a.iter().any(|x| x == "--remux-video"));
        assert!(!a.iter().any(|x| x == "--recode-video"), "that re-encodes");
    }

    /// The temp directory goes away with the handle. A film left in /tmp is
    /// gigabytes and fills the disk of the device this targets.
    #[test]
    fn the_temp_directory_is_removed_on_drop() {
        let dir = std::env::temp_dir().join(format!("vidkit-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("media.mp4");
        std::fs::write(&file, b"x").unwrap();
        {
            let _t = TempMedia { dir: dir.clone(), file };
            assert!(dir.exists());
        }
        assert!(!dir.exists(), "the download was left behind");
    }

    /// yt-dlp leaves `.part` files and thumbnails around; the media is the
    /// biggest thing in the directory.
    #[test]
    fn the_largest_file_is_taken_as_the_media() {
        struct Fake(std::path::PathBuf);
        impl vidkit_ytdlp::CommandRunner for Fake {
            fn run(&mut self, _p: &str, a: &[String]) -> Result<Vec<u8>, vidkit_ytdlp::RunError> {
                // Write what yt-dlp would: the media plus some litter.
                let tmpl = a.iter().find(|x| x.contains("media.")).unwrap();
                let dir = std::path::Path::new(tmpl).parent().unwrap();
                std::fs::write(dir.join("media.mp4"), vec![0u8; 4096]).unwrap();
                std::fs::write(dir.join("media.jpg"), vec![0u8; 16]).unwrap();
                let _ = &self.0;
                Ok(Vec::new())
            }
        }
        let mut f = Fake(std::path::PathBuf::new());
        let got = fetch_to_temp(&mut f, "https://site/v", Some(720)).expect("fetch");
        assert_eq!(
            got.path().extension().and_then(|e| e.to_str()),
            Some("mp4"),
            "picked the thumbnail: {}", got.path().display()
        );
    }

    /// The whole new path against the REAL yt-dlp: download to a temp file,
    /// then decode it. No YouTube needed — the generic extractor handles a
    /// plain HTTP URL, which exercises the same download-then-play flow.
    #[test]
    fn a_real_download_produces_a_decodable_file() {
        let have = std::process::Command::new("yt-dlp")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !have {
            eprintln!("SKIP: no yt-dlp");
            return;
        }

        // A real encoded clip, served over HTTP.
        let clip = std::env::temp_dir().join(format!("vidkit-src-{}.mp4", std::process::id()));
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-v", "error", "-y",
                "-f", "lavfi", "-i", "color=c=0x20C040:size=320x240:rate=10:duration=2",
                "-pix_fmt", "yuv420p", "-c:v", "libx264",
            ])
            .arg(&clip)
            .status();
        if !made.map(|s| s.success()).unwrap_or(false) {
            eprintln!("SKIP: no ffmpeg");
            return;
        }
        let body = std::fs::read(&clip).unwrap();
        let port = serve_bytes(body);

        let mut runner = ProcessRunner::default();
        let media = fetch_to_temp(
            &mut runner,
            &format!("http://127.0.0.1:{port}/clip.mp4"),
            Some(720),
        )
        .expect("download");
        assert!(media.path().is_file(), "no file at {}", media.path().display());

        let mut dec = crate::decode::FfmpegFrames::open(media.path(), None)
            .expect("decode the download");
        let f = dec.next_frame().expect("a frame from the downloaded file");
        assert_eq!((f.width, f.height), (320, 240));

        let _ = std::fs::remove_file(&clip);
    }

    /// An interrupted download leaves a `.part`, and it is often the largest
    /// thing in the directory — so "take the biggest file" picks it and the
    /// decoder reports a truncated container instead of an unfinished
    /// download.
    #[test]
    fn a_part_file_is_not_mistaken_for_the_media() {
        struct Interrupted;
        impl vidkit_ytdlp::CommandRunner for Interrupted {
            fn run(&mut self, _p: &str, a: &[String]) -> Result<Vec<u8>, vidkit_ytdlp::RunError> {
                let tmpl = a.iter().find(|x| x.contains("media.")).unwrap();
                let dir = std::path::Path::new(tmpl).parent().unwrap();
                // The part file is deliberately the biggest.
                std::fs::write(dir.join("media.mp4.part"), vec![0u8; 8192]).unwrap();
                std::fs::write(dir.join("media.mp4"), vec![7u8; 4096]).unwrap();
                Ok(Vec::new())
            }
        }
        let got = fetch_to_temp(&mut Interrupted, "https://site/v", None).expect("fetch");
        assert_eq!(
            got.path().extension().and_then(|e| e.to_str()),
            Some("mp4"),
            "picked the .part: {}",
            got.path().display()
        );
    }

    /// yt-dlp can exit zero having written nothing usable.
    #[test]
    fn an_empty_download_is_reported_as_one() {
        struct WroteNothing;
        impl vidkit_ytdlp::CommandRunner for WroteNothing {
            fn run(&mut self, _p: &str, a: &[String]) -> Result<Vec<u8>, vidkit_ytdlp::RunError> {
                let tmpl = a.iter().find(|x| x.contains("media.")).unwrap();
                let dir = std::path::Path::new(tmpl).parent().unwrap();
                std::fs::write(dir.join("media.mp4"), b"").unwrap();
                Ok(Vec::new())
            }
        }
        let e = match fetch_to_temp(&mut WroteNothing, "https://site/v", None) {
            Err(e) => e,
            Ok(_) => panic!("an empty download was accepted"),
        };
        assert!(e.contains("empty"), "unhelpful: {e}");
    }

    /// And it must not leave the directory behind when it refuses.
    #[test]
    fn a_rejected_download_is_cleaned_up() {
        struct Tiny;
        impl vidkit_ytdlp::CommandRunner for Tiny {
            fn run(&mut self, _p: &str, a: &[String]) -> Result<Vec<u8>, vidkit_ytdlp::RunError> {
                let tmpl = a.iter().find(|x| x.contains("media.")).unwrap();
                let dir = std::path::Path::new(tmpl).parent().unwrap();
                std::fs::write(dir.join("media.mp4"), b"tiny").unwrap();
                Ok(Vec::new())
            }
        }
        let before = std::fs::read_dir(std::env::temp_dir())
            .map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("vidkit-dl-")).count())
            .unwrap_or(0);
        assert!(fetch_to_temp(&mut Tiny, "https://site/v", None).is_err());
        let after = std::fs::read_dir(std::env::temp_dir())
            .map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("vidkit-dl-")).count())
            .unwrap_or(0);
        assert_eq!(after, before, "a rejected download left its directory behind");
    }

    /// A failing yt-dlp must not leave its directory behind either.
    #[test]
    fn a_failed_download_cleans_up_after_itself() {
        struct Failing;
        impl vidkit_ytdlp::CommandRunner for Failing {
            fn run(&mut self, _p: &str, _a: &[String]) -> Result<Vec<u8>, vidkit_ytdlp::RunError> {
                Err(vidkit_ytdlp::RunError::Io("nope".into()))
            }
        }
        let before = std::fs::read_dir(std::env::temp_dir())
            .map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("vidkit-dl-")).count())
            .unwrap_or(0);
        assert!(fetch_to_temp(&mut Failing, "https://site/v", None).is_err());
        let after = std::fs::read_dir(std::env::temp_dir())
            .map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("vidkit-dl-")).count())
            .unwrap_or(0);
        assert_eq!(after, before, "a failed download left its directory behind");
    }
}

// ---------------------------------------------------------------------------
// opening a local file without swallowing it
// ---------------------------------------------------------------------------

/// A [`ByteSource`] over a file on disk.
///
/// `open_local` used `std::fs::read`, which loads the WHOLE FILE into memory to
/// read sixteen bytes of header. A two-hour lecture is gigabytes and the phone
/// this targets has a few; the process is killed before a frame is shown, and
/// the failure looks like a crash rather than a mistake. The demuxers only ever
/// wanted random access, which is what this gives them.
pub struct FileSource {
    file: std::fs::File,
    len: u64,
}

impl FileSource {
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| describe_open_error(path, &e))?;
        let len = file.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
        Ok(Self { file, len })
    }
}

impl vidkit_core::io::ByteSource for FileSource {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> vidkit_core::io::Result<usize> {
        use std::io::{Read, Seek, SeekFrom};
        if offset >= self.len {
            return Ok(0);
        }
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| vidkit_core::io::Error::Io(e.to_string()))?;
        // A short read at the end of a file is normal, not an error; the
        // caller retries or stops.
        self.file
            .read(buf)
            .map_err(|e| vidkit_core::io::Error::Io(e.to_string()))
    }

    fn size(&self) -> Option<u64> {
        Some(self.len)
    }
}

/// What is wrong with a path, in words someone can act on.
///
/// `std::io::Error`'s own text is "No such file or directory (os error 2)",
/// which names neither the file nor what to do. Every branch here says which
/// path and what the user can change.
pub fn describe_open_error(path: &std::path::Path, e: &std::io::Error) -> String {
    let p = path.display();
    match e.kind() {
        std::io::ErrorKind::NotFound => format!("{p}: no such file"),
        std::io::ErrorKind::PermissionDenied => {
            format!("{p}: no permission to read it")
        }
        _ if e.raw_os_error() == Some(21) => format!("{p}: that is a directory"),
        _ => format!("{p}: {e}"),
    }
}

/// Facts about a file worth knowing before trying to demux it.
#[derive(Clone, PartialEq, Debug)]
pub struct FileFacts {
    pub len: u64,
    /// The first bytes, for a container sniff.
    pub head: Vec<u8>,
}

/// The smallest thing that could plausibly be a video.
///
/// An MP4 needs an `ftyp`, a `moov` with a sample table and an `mdat`; a WebM
/// needs an EBML header, Info, Tracks and a Cluster. Neither fits in a few
/// hundred bytes. The point is not the exact number — it is to reject a
/// zero-byte or truncated download with "that file is empty" instead of a
/// parser error about a malformed box.
pub const MIN_PLAUSIBLE_LEN: u64 = 256;

/// Check a path before opening it.
pub fn check_file(path: &std::path::Path) -> Result<FileFacts, String> {
    let meta = std::fs::metadata(path).map_err(|e| describe_open_error(path, &e))?;
    if meta.is_dir() {
        return Err(format!("{}: that is a directory", path.display()));
    }
    if !meta.is_file() {
        // A socket, fifo or device node. Reading one may block forever, which
        // is a worse outcome than refusing.
        return Err(format!("{}: not a regular file", path.display()));
    }
    let len = meta.len();
    if len == 0 {
        return Err(format!("{}: the file is empty", path.display()));
    }
    if len < MIN_PLAUSIBLE_LEN {
        return Err(format!(
            "{}: only {len} bytes — too small to be a video, probably a truncated download",
            path.display()
        ));
    }

    use vidkit_core::io::ByteSource;
    let mut src = FileSource::open(path)?;
    let mut head = [0u8; 16];
    let n = src
        .read_at(0, &mut head)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(FileFacts {
        len,
        head: head[..n].to_vec(),
    })
}

#[cfg(test)]
mod file_check_tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("vidkit-fc-{}-{name}", std::process::id()));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    /// "No such file or directory (os error 2)" names neither the file nor
    /// anything the reader can do about it.
    #[test]
    fn a_missing_file_is_named_in_plain_words() {
        let e = check_file(std::path::Path::new("/nope/missing.mp4")).unwrap_err();
        assert!(e.contains("/nope/missing.mp4"), "{e}");
        assert!(e.contains("no such file"), "{e}");
        assert!(!e.contains("os error"), "raw errno leaked to the user: {e}");
    }

    #[test]
    fn a_directory_is_reported_as_one() {
        let e = check_file(&std::env::temp_dir()).unwrap_err();
        assert!(e.contains("directory"), "{e}");
    }

    /// A zero-byte file is an interrupted download, and saying so beats a
    /// parser complaining about a malformed box.
    #[test]
    fn an_empty_file_says_it_is_empty() {
        let p = tmp("empty.mp4", b"");
        let e = check_file(&p).unwrap_err();
        assert!(e.contains("empty"), "{e}");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn a_truncated_file_is_called_out_as_truncated() {
        let p = tmp("tiny.mp4", b"\x00\x00\x00\x18ftypisom");
        let e = check_file(&p).unwrap_err();
        assert!(e.contains("too small"), "{e}");
        assert!(e.contains("truncated"), "{e}");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn a_plausible_file_returns_its_head_and_length() {
        let mut bytes = vec![0u8; 1024];
        bytes[4..8].copy_from_slice(b"ftyp");
        let p = tmp("ok.mp4", &bytes);
        let facts = check_file(&p).expect("should pass");
        assert_eq!(facts.len, 1024);
        assert_eq!(&facts.head[4..8], b"ftyp");
        let _ = std::fs::remove_file(p);
    }

    /// THE POINT: reading a header must not read the file. `std::fs::read` on
    /// a two-hour lecture allocates gigabytes to look at sixteen bytes.
    #[test]
    fn checking_a_large_file_does_not_load_it() {
        use vidkit_core::io::ByteSource;
        // 8 MB is enough to make the difference measurable without making the
        // test slow; the real files are three orders of magnitude larger.
        let big = vec![0xABu8; 8 * 1024 * 1024];
        let p = tmp("big.mp4", &big);

        let mut src = FileSource::open(&p).expect("open");
        assert_eq!(src.size(), Some(8 * 1024 * 1024));
        let mut buf = [0u8; 16];
        assert_eq!(src.read_at(0, &mut buf).unwrap(), 16);
        assert!(buf.iter().all(|&b| b == 0xAB));

        // Random access, not a cursor: the demuxers jump around sample tables.
        let mut mid = [0u8; 4];
        assert_eq!(src.read_at(4 * 1024 * 1024, &mut mid).unwrap(), 4);
        assert_eq!(src.read_at(0, &mut buf).unwrap(), 16, "cannot read backwards");

        // Past the end is zero, not an error, which is how `read_exact_at`
        // detects truncation.
        assert_eq!(src.read_at(99 * 1024 * 1024, &mut buf).unwrap(), 0);
        let _ = std::fs::remove_file(p);
    }

    /// A fifo would block forever on read; refusing is the better outcome.
    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_rather_than_read() {
        let p = std::env::temp_dir().join(format!("vidkit-fifo-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let made = std::process::Command::new("mkfifo").arg(&p).status();
        if !made.map(|s| s.success()).unwrap_or(false) {
            eprintln!("SKIP: no mkfifo");
            return;
        }
        let e = check_file(&p).unwrap_err();
        assert!(e.contains("not a regular file"), "{e}");
        let _ = std::fs::remove_file(p);
    }
}

/// The prefix every download directory uses.
pub const TEMP_PREFIX: &str = "vidkit-dl-";

/// Delete download directories left by earlier runs.
///
/// `TempMedia`'s `Drop` handles a clean exit. It does not run on SIGKILL, on
/// an OOM kill, or when a phone's session manager stops the app — and those
/// are the cases that matter, because what is left behind is a whole film. A
/// few of those fill the storage of the device this targets, and nothing else
/// in the system would ever clean them up.
///
/// Directories belonging to a LIVE process are left alone: two copies of the
/// player running at once must not delete each other's download mid-playback.
pub fn sweep_stale_downloads(tmp: &std::path::Path, is_running: impl Fn(u32) -> bool) -> usize {
    let mut removed = 0;
    let Ok(entries) = std::fs::read_dir(tmp) else {
        return 0;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix(TEMP_PREFIX) else {
            continue;
        };
        // Belt and braces, and NOT load-bearing: `remove_dir_all` refuses a
        // plain file, so deleting this check changes no behaviour and no test
        // catches it. Kept because relying on that stdlib detail to protect
        // someone's file is a worse bet than one explicit line.
        if !e.path().is_dir() {
            continue;
        }
        // `vidkit-dl-<pid>-<nanos>`.
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) else {
            continue;
        };
        if is_running(pid) {
            continue;
        }
        if std::fs::remove_dir_all(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Is a process alive? `/proc` on Linux.
pub fn pid_is_running(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Sweep the real temp directory at startup.
pub fn sweep_on_start() -> usize {
    sweep_stale_downloads(&std::env::temp_dir(), pid_is_running)
}

#[cfg(test)]
mod sweep_tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("vidkit-sweep-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn download_dir(root: &std::path::Path, pid: u32, bytes: usize) -> std::path::PathBuf {
        let d = root.join(format!("{TEMP_PREFIX}{pid}-12345"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("media.mp4"), vec![0u8; bytes]).unwrap();
        d
    }

    /// What Drop cannot cover: a killed process leaves a whole film behind,
    /// and a few of those fill a phone.
    #[test]
    fn a_dead_processes_download_is_removed() {
        let root = scratch("dead");
        let d = download_dir(&root, 999_999, 4096);
        assert_eq!(sweep_stale_downloads(&root, |_| false), 1);
        assert!(!d.exists(), "the film was left behind");
        let _ = std::fs::remove_dir_all(root);
    }

    /// Two players running at once must not delete each other's download
    /// mid-playback.
    #[test]
    fn a_live_processes_download_is_left_alone() {
        let root = scratch("live");
        let d = download_dir(&root, 4242, 4096);
        assert_eq!(sweep_stale_downloads(&root, |pid| pid == 4242), 0);
        assert!(d.exists(), "deleted a running player's download");
        let _ = std::fs::remove_dir_all(root);
    }

    /// Only our own directories. Deleting somebody else's /tmp entry because
    /// the name looked close enough is a far worse bug than leaking one.
    #[test]
    fn unrelated_temp_entries_are_untouched() {
        let root = scratch("others");
        std::fs::create_dir_all(root.join("someone-elses-work")).unwrap();
        std::fs::write(root.join("vidkit-dl-notapid"), b"x").unwrap();
        // A FILE whose name parses as a dead pid. The previous fixture's pid
        // did not parse, so it was rejected before the is-a-directory check
        // and that check was never exercised.
        std::fs::write(root.join("vidkit-dl-999998-1"), b"x").unwrap();
        std::fs::create_dir_all(root.join("vidkit-dl-notapid-dir")).unwrap();
        assert_eq!(sweep_stale_downloads(&root, |_| false), 0);
        assert!(root.join("someone-elses-work").exists());
        assert!(root.join("vidkit-dl-notapid").exists(), "deleted a file, not a dir");
        assert!(
            root.join("vidkit-dl-999998-1").exists(),
            "deleted a plain file whose name looked like a download directory"
        );
        assert!(
            root.join("vidkit-dl-notapid-dir").exists(),
            "deleted a directory whose pid does not parse"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// The current process is alive, so its own in-flight download survives a
    /// sweep — otherwise starting a second window deletes the first's film.
    #[test]
    fn our_own_live_download_survives_a_real_sweep() {
        let root = scratch("self");
        let mine = download_dir(&root, std::process::id(), 1024);
        assert_eq!(sweep_stale_downloads(&root, pid_is_running), 0);
        assert!(mine.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_temp_directory_is_not_an_error() {
        assert_eq!(
            sweep_stale_downloads(std::path::Path::new("/nope/nothing"), |_| false),
            0
        );
    }
}
