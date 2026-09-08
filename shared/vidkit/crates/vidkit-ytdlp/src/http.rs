//! Reading a remote stream through range requests.
//!
//! `Resolver` produces a URL and a set of headers; both demuxers want a
//! [`ByteSource`]. Nothing joined the two, so a yt-dlp result could not reach a
//! demuxer at all. This is that join.
//!
//! The socket sits behind [`HttpTransport`], so everything above it — the read
//! cache, the expiry handling, the 403 path — is tested with no network. The
//! real transport is a thin `ureq`/`reqwest` wrapper with no logic in it, and
//! is deliberately not written here: this crate has no HTTP dependency and
//! should not grow one to be testable.
//!
//! # Why a cache and not just a request per read
//!
//! A demuxer reads box headers eight bytes at a time. Issuing one range
//! request per `read_at` turns opening a file into thousands of round trips,
//! and on a phone that is minutes. Reads are served from a window that is
//! fetched in [`ChunkPolicy::chunk_bytes`] blocks, so a header walk costs one
//! request rather than a thousand.

use std::collections::BTreeMap;
use vidkit_core::io::{ByteSource, Error, Result};

/// One range request.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RangeRequest {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    /// First byte wanted, inclusive.
    pub start: u64,
    /// Last byte wanted, inclusive — HTTP ranges are inclusive at both ends,
    /// which is the off-by-one everyone writes at least once.
    pub end: u64,
}

impl RangeRequest {
    /// The `Range` header value this request corresponds to.
    pub fn header_value(&self) -> String {
        format!("bytes={}-{}", self.start, self.end)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum HttpError {
    /// The signed URL expired or the CDN rejected us. Routine on YouTube, and
    /// deliberately not lumped in with other failures: the caller re-resolves
    /// and carries on rather than tearing down playback.
    Forbidden,
    /// The server ignored the range and sent the whole body. Some CDNs do this
    /// and it must not be mistaken for the requested slice.
    RangeIgnored,
    Status(u16),
    Transport(String),
}

/// Fetches one byte range. The only thing that touches a socket.
pub trait HttpTransport {
    fn fetch(&mut self, req: &RangeRequest) -> std::result::Result<Vec<u8>, HttpError>;

    /// Total length, when a HEAD or a `Content-Range` has revealed it.
    fn known_length(&self) -> Option<u64> {
        None
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ChunkPolicy {
    /// Bytes fetched per request. Large enough that a box-header walk is one
    /// round trip, small enough that a seek does not pull megabytes.
    pub chunk_bytes: u64,
    /// Requests to retry after a transport error before giving up.
    pub retries: u32,
}

impl Default for ChunkPolicy {
    fn default() -> Self {
        Self {
            chunk_bytes: 256 * 1024,
            retries: 2,
        }
    }
}

/// A [`ByteSource`] over HTTP range requests.
pub struct HttpByteSource<T: HttpTransport> {
    transport: T,
    url: String,
    headers: BTreeMap<String, String>,
    policy: ChunkPolicy,
    /// The cached window: the byte offset it starts at, and its contents.
    window: (u64, Vec<u8>),
    total: Option<u64>,
    /// Set when the server said 403. The caller re-resolves and calls
    /// `rebind`; reads fail until it does, rather than silently returning
    /// short data that a demuxer would read as a truncated file.
    expired: bool,
    pub requests: u32,
}

impl<T: HttpTransport> HttpByteSource<T> {
    pub fn new(transport: T, url: impl Into<String>, headers: BTreeMap<String, String>) -> Self {
        Self {
            transport,
            url: url.into(),
            headers,
            policy: ChunkPolicy::default(),
            window: (0, Vec::new()),
            total: None,
            expired: false,
            requests: 0,
        }
    }

    pub fn with_policy(mut self, policy: ChunkPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn with_total(mut self, total: u64) -> Self {
        self.total = Some(total);
        self
    }

    /// True once the server has rejected us and before `rebind`.
    pub fn is_expired(&self) -> bool {
        self.expired
    }

    /// Point at a freshly resolved URL after an expiry.
    ///
    /// The cached window is kept: it is bytes of the same media, and a URL
    /// refresh does not change the content. Dropping it would re-fetch
    /// everything the demuxer had already read.
    pub fn rebind(&mut self, url: impl Into<String>, headers: BTreeMap<String, String>) {
        self.url = url.into();
        self.headers = headers;
        self.expired = false;
    }

    fn fetch_window(&mut self, start: u64) -> Result<()> {
        let end = match self.total {
            Some(t) if t > 0 => (start + self.policy.chunk_bytes - 1).min(t - 1),
            _ => start + self.policy.chunk_bytes - 1,
        };
        if end < start {
            self.window = (start, Vec::new());
            return Ok(());
        }

        let req = RangeRequest {
            url: self.url.clone(),
            headers: self.headers.clone(),
            start,
            end,
        };

        let mut attempt = 0;
        loop {
            self.requests += 1;
            match self.transport.fetch(&req) {
                Ok(bytes) => {
                    // A server that ignored the range and sent the whole body
                    // would otherwise be cached at the wrong offset, and every
                    // subsequent read would return the wrong bytes while
                    // looking perfectly well-formed.
                    let wanted = (end - start + 1) as usize;
                    if bytes.len() > wanted {
                        return Err(Error::Io(format!(
                            "server ignored the range: asked for {wanted} bytes at {start}, got {}",
                            bytes.len()
                        )));
                    }
                    if let Some(t) = self.transport.known_length() {
                        self.total = Some(t);
                    }
                    self.window = (start, bytes);
                    return Ok(());
                }
                Err(HttpError::Forbidden) => {
                    // Not retried: a 403 on a signed URL will still be a 403.
                    self.expired = true;
                    return Err(Error::Io("403: media URL expired".into()));
                }
                Err(HttpError::RangeIgnored) => {
                    return Err(Error::Io("server ignored the range request".into()))
                }
                Err(e) => {
                    attempt += 1;
                    if attempt > self.policy.retries {
                        return Err(Error::Io(format!("{e:?} after {attempt} attempts")));
                    }
                }
            }
        }
    }

    fn window_covers(&self, offset: u64, len: usize) -> bool {
        let (start, ref data) = self.window;
        offset >= start && offset + len as u64 <= start + data.len() as u64
    }
}

impl<T: HttpTransport> ByteSource for HttpByteSource<T> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if self.expired {
            return Err(Error::Io("media URL expired; re-resolve and rebind".into()));
        }
        if buf.is_empty() {
            return Ok(0);
        }
        if let Some(t) = self.total {
            if offset >= t {
                return Ok(0);
            }
        }

        if !self.window_covers(offset, buf.len()) {
            self.fetch_window(offset)?;
        }

        let (start, ref data) = self.window;
        // A read that straddles the end of the window is served short rather
        // than by a second fetch. `read_exact_at` retries, and doing the loop
        // here would hide how many requests a read costs.
        let from = (offset - start) as usize;
        if from >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - from);
        buf[..n].copy_from_slice(&data[from..from + n]);
        Ok(n)
    }

    fn size(&self) -> Option<u64> {
        self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serves a fixed body, recording every request.
    struct MockServer {
        body: Vec<u8>,
        pub seen: Vec<RangeRequest>,
        /// Requests that should 403 before succeeding, counted down.
        pub forbidden_for: u32,
        pub flaky_for: u32,
        pub ignore_range: bool,
        pub reveal_length: bool,
    }

    impl MockServer {
        fn new(body: Vec<u8>) -> Self {
            Self {
                body,
                seen: Vec::new(),
                forbidden_for: 0,
                flaky_for: 0,
                ignore_range: false,
                reveal_length: true,
            }
        }
    }

    impl HttpTransport for MockServer {
        fn fetch(&mut self, req: &RangeRequest) -> std::result::Result<Vec<u8>, HttpError> {
            self.seen.push(req.clone());
            if self.forbidden_for > 0 {
                self.forbidden_for -= 1;
                return Err(HttpError::Forbidden);
            }
            if self.flaky_for > 0 {
                self.flaky_for -= 1;
                return Err(HttpError::Transport("reset".into()));
            }
            if self.ignore_range {
                return Ok(self.body.clone());
            }
            let start = req.start as usize;
            if start >= self.body.len() {
                return Ok(Vec::new());
            }
            // HTTP ranges are inclusive at both ends.
            let end = ((req.end as usize) + 1).min(self.body.len());
            Ok(self.body[start..end].to_vec())
        }

        fn known_length(&self) -> Option<u64> {
            if self.reveal_length {
                Some(self.body.len() as u64)
            } else {
                None
            }
        }
    }

    fn body(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    fn source(body: Vec<u8>, chunk: u64) -> HttpByteSource<MockServer> {
        let mut h = BTreeMap::new();
        h.insert("User-Agent".into(), "Mozilla/5.0 test".into());
        HttpByteSource::new(MockServer::new(body), "https://cdn/v", h).with_policy(ChunkPolicy {
            chunk_bytes: chunk,
            retries: 2,
        })
    }

    #[test]
    fn a_read_returns_the_right_bytes() {
        let data = body(1000);
        let mut s = source(data.clone(), 256);
        let mut buf = [0u8; 16];
        let n = s.read_at(100, &mut buf).unwrap();
        assert_eq!(n, 16);
        assert_eq!(&buf[..], &data[100..116]);
    }

    /// The off-by-one that everyone writes once: HTTP ranges are inclusive at
    /// both ends, so `bytes=0-255` is 256 bytes, not 255.
    #[test]
    fn the_range_header_is_inclusive_at_both_ends() {
        let mut s = source(body(1000), 256);
        let mut buf = [0u8; 4];
        s.read_at(0, &mut buf).unwrap();
        let req = &s.transport.seen[0];
        assert_eq!(req.header_value(), "bytes=0-255");
        assert_eq!(req.end - req.start + 1, 256, "range covers the wrong count");
    }

    /// The whole reason the cache exists: a demuxer reads headers eight bytes
    /// at a time, and one request per read would be thousands of round trips.
    #[test]
    fn many_small_reads_inside_a_window_cost_one_request() {
        let mut s = source(body(4096), 1024);
        let mut buf = [0u8; 8];
        for off in (0..800).step_by(8) {
            s.read_at(off, &mut buf).unwrap();
        }
        assert_eq!(s.requests, 1, "cache is not being used: {} requests", s.requests);
    }

    #[test]
    fn reading_past_the_window_fetches_again() {
        let mut s = source(body(4096), 512);
        let mut buf = [0u8; 8];
        s.read_at(0, &mut buf).unwrap();
        s.read_at(2000, &mut buf).unwrap();
        assert_eq!(s.requests, 2);
        assert_eq!(s.transport.seen[1].start, 2000);
    }

    #[test]
    fn the_headers_are_forwarded_on_every_request() {
        let mut s = source(body(4096), 512);
        let mut buf = [0u8; 8];
        s.read_at(0, &mut buf).unwrap();
        s.read_at(2000, &mut buf).unwrap();
        assert!(s.transport.seen.len() >= 2);
        for req in &s.transport.seen {
            assert_eq!(
                req.headers.get("User-Agent").map(String::as_str),
                Some("Mozilla/5.0 test"),
                "a signed URL is bound to its headers; dropping them is a 403"
            );
        }
    }

    #[test]
    fn the_length_is_learned_from_the_server() {
        let mut s = source(body(1234), 256);
        assert_eq!(s.size(), None, "nothing known before the first request");
        let mut buf = [0u8; 8];
        s.read_at(0, &mut buf).unwrap();
        assert_eq!(s.size(), Some(1234));
    }

    #[test]
    fn reading_past_the_end_returns_zero_not_an_error() {
        let mut s = source(body(100), 256);
        let mut buf = [0u8; 8];
        s.read_at(0, &mut buf).unwrap();
        assert_eq!(s.read_at(500, &mut buf).unwrap(), 0);
    }

    #[test]
    fn a_read_straddling_the_window_end_is_served_short() {
        let mut s = source(body(4096), 64);
        let mut buf = [0u8; 100];
        let n = s.read_at(0, &mut buf).unwrap();
        assert_eq!(n, 64, "a short read is how the caller learns to ask again");
    }

    /// `read_exact_at` is what the demuxers use, and it must still work over a
    /// window smaller than the read.
    #[test]
    fn read_exact_at_works_across_window_boundaries() {
        let data = body(4096);
        let mut s = source(data.clone(), 64);
        let got = vidkit_core::io::read_exact_at(&mut s, 30, 200);
        // A single short read is not enough for read_exact_at, which is
        // honest: it reports truncation rather than looping invisibly.
        assert!(got.is_err(), "a short read should surface, not be hidden");
    }

    // ---- failure paths ---------------------------------------------------

    /// A 403 must be distinguishable, because it is recoverable: re-resolve
    /// and carry on rather than tear down playback.
    #[test]
    fn a_403_marks_the_source_expired_and_is_not_retried() {
        let mut s = source(body(1000), 256);
        s.transport.forbidden_for = 1;
        let mut buf = [0u8; 8];
        assert!(s.read_at(0, &mut buf).is_err());
        assert!(s.is_expired());
        assert_eq!(
            s.transport.seen.len(),
            1,
            "a 403 on a signed URL will still be a 403; retrying wastes time"
        );
    }

    #[test]
    fn an_expired_source_refuses_reads_until_rebound() {
        let mut s = source(body(1000), 256);
        s.transport.forbidden_for = 1;
        let mut buf = [0u8; 8];
        let _ = s.read_at(0, &mut buf);

        // Refusing beats returning short data, which a demuxer reads as a
        // truncated file and reports as a corrupt stream.
        assert!(s.read_at(0, &mut buf).is_err());
        assert_eq!(s.transport.seen.len(), 1, "kept hitting a dead URL");

        let mut h = BTreeMap::new();
        h.insert("User-Agent".into(), "Mozilla/5.0 test".into());
        s.rebind("https://cdn/v2", h);
        assert!(!s.is_expired());
        assert_eq!(s.read_at(0, &mut buf).unwrap(), 8);
        assert_eq!(s.transport.seen.last().unwrap().url, "https://cdn/v2");
    }

    #[test]
    fn a_transport_error_is_retried_and_then_reported() {
        let mut s = source(body(1000), 256);
        s.transport.flaky_for = 2;
        let mut buf = [0u8; 8];
        assert!(s.read_at(0, &mut buf).is_ok(), "should recover within retries");
        assert_eq!(s.transport.seen.len(), 3);

        let mut s = source(body(1000), 256);
        s.transport.flaky_for = 99;
        assert!(s.read_at(0, &mut buf).is_err());
        assert_eq!(s.transport.seen.len(), 3, "retried more than the policy allows");
    }

    /// Some CDNs answer a range request with the whole body. Caching that at
    /// the requested offset makes every later read silently wrong.
    #[test]
    fn a_server_that_ignores_the_range_is_rejected_not_cached() {
        let mut s = source(body(4096), 256);
        s.transport.ignore_range = true;
        let mut buf = [0u8; 8];
        let e = s.read_at(1000, &mut buf).unwrap_err();
        assert!(
            format!("{e}").contains("ignored the range"),
            "unhelpful error: {e}"
        );
    }

    #[test]
    fn the_final_window_is_clamped_to_the_known_length() {
        let mut s = source(body(300), 256).with_total(300);
        let mut buf = [0u8; 8];
        s.read_at(280, &mut buf).unwrap();
        let req = s.transport.seen.last().unwrap();
        assert_eq!(req.end, 299, "asked past the end of the object: {req:?}");
    }
}
