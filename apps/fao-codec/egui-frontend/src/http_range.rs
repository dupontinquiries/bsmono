//! http_range — read a remote `.fao` over HTTP byte ranges, so the streaming
//! player can play a file on a dufs server without downloading it.
//!
//! ## Two layers
//!
//! * `RangeCache<F>` -- PURE. `Read + Seek` over any `fetch(start, end)`
//!   function, with an LRU of fixed-size blocks. No network; tested with an
//!   in-memory fetcher that counts calls.
//! * `HttpRangeReader` -- the transport: a `RangeCache` whose fetch is an HTTP
//!   `Range: bytes=a-b` request. dufs serves those (verified against a real
//!   dufs 0.46.0 earlier: 206 with `Content-Range: bytes 2-5/10`).
//!
//! ## Why the cache is the whole point
//!
//! `stream::FaoReader` makes many tiny scattered reads: a 16-byte index entry
//! per frame looked up, ONE byte per frame when `LazyAnchors` walks for a
//! keyframe, a short tail scan for the trailers. Unbuffered, each would be an
//! HTTP round trip -- a seek near the end of a long file walking a few hundred
//! markers would be a few hundred requests. With 256 KiB blocks, markers and
//! index entries that sit close together cost one request between them.
//!
//! ## What a server without ranges gets
//!
//! An error, not a silent full download. A 200 in answer to a range request
//! means the server ignored the header and is sending the WHOLE file -- for a
//! multi-gigabyte recording, exactly what this module exists to avoid.

use std::io::{self, Read, Seek, SeekFrom};

/// Block size for the cache. Large enough that a keyframe walk's scattered
/// one-byte marker reads and neighbouring index entries share a fetch; small
/// enough that a single random seek does not pull megabytes.
pub const BLOCK: u64 = 256 * 1024;
/// Blocks kept: 16 x 256 KiB = 4 MiB, bounded however long playback runs.
pub const CACHED_BLOCKS: usize = 16;

/// `Read + Seek` over `fetch(start, end_inclusive) -> bytes`, block-cached.
pub struct RangeCache<F: FnMut(u64, u64) -> io::Result<Vec<u8>> + Send> {
    fetch: F,
    len: u64,
    pos: u64,
    /// (block index, bytes), most recently used LAST.
    blocks: Vec<(u64, Vec<u8>)>,
    /// Fetches made, for tests and diagnostics.
    pub fetches: usize,
}

/// The inclusive byte span of block `i` in a file of `len` bytes.
pub fn block_span(i: u64, len: u64) -> (u64, u64) {
    let start = i * BLOCK;
    (start, (start + BLOCK).min(len) - 1)
}

impl<F: FnMut(u64, u64) -> io::Result<Vec<u8>> + Send> RangeCache<F> {
    pub fn new(len: u64, fetch: F) -> Self {
        RangeCache { fetch, len, pos: 0, blocks: Vec::new(), fetches: 0 }
    }

    pub fn len(&self) -> u64 { self.len }

    fn block(&mut self, i: u64) -> io::Result<&[u8]> {
        if let Some(k) = self.blocks.iter().position(|(b, _)| *b == i) {
            let hit = self.blocks.remove(k);
            self.blocks.push(hit);
        } else {
            let (s, e) = block_span(i, self.len);
            let data = (self.fetch)(s, e)?;
            self.fetches += 1;
            // A short or long reply is a protocol error, not something to
            // paper over: a wrong length silently shifts every later read.
            if data.len() as u64 != e - s + 1 {
                return Err(io::Error::new(io::ErrorKind::InvalidData,
                    format!("range {s}-{e}: got {} bytes, expected {}", data.len(), e - s + 1)));
            }
            if self.blocks.len() >= CACHED_BLOCKS {
                self.blocks.remove(0);
            }
            self.blocks.push((i, data));
        }
        Ok(&self.blocks.last().unwrap().1)
    }
}

impl<F: FnMut(u64, u64) -> io::Result<Vec<u8>> + Send> Read for RangeCache<F> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let i = self.pos / BLOCK;
        let off = (self.pos - i * BLOCK) as usize;
        let pos = self.pos;
        let block = self.block(i)?;
        let n = (block.len() - off).min(buf.len());
        buf[..n].copy_from_slice(&block[off..off + n]);
        self.pos = pos + n as u64;
        Ok(n)
    }
}

impl<F: FnMut(u64, u64) -> io::Result<Vec<u8>> + Send> Seek for RangeCache<F> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let new = match to {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if new < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = new as u64;
        Ok(self.pos)
    }
}

/// The total from a `Content-Range: bytes a-b/TOTAL` header. `None` for a
/// missing total (`*`) or anything malformed.
pub fn parse_content_range_total(h: &str) -> Option<u64> {
    let rest = h.trim().strip_prefix("bytes")?.trim_start();
    let (_, total) = rest.split_once('/')?;
    total.trim().parse().ok()
}

/// A `.fao` over HTTP: the transport half.
pub type HttpRangeReader = RangeCache<Box<dyn FnMut(u64, u64) -> io::Result<Vec<u8>> + Send>>;

/// Open `url` for ranged reading. One small request (`bytes=0-0`) learns the
/// total length and proves the server honours ranges.
pub fn open_http(url: &str) -> io::Result<HttpRangeReader> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(30))
        .build();
    let get = |agent: &ureq::Agent, url: &str, s: u64, e: u64| -> io::Result<ureq::Response> {
        agent.get(url).set("Range", &format!("bytes={s}-{e}")).call()
            .map_err(|err| io::Error::new(io::ErrorKind::Other, format!("GET {url} bytes={s}-{e}: {err}")))
    };
    let probe = get(&agent, url, 0, 0)?;
    if probe.status() != 206 {
        return Err(io::Error::new(io::ErrorKind::Unsupported, format!(
            "{url}: server answered {} to a range request -- it would send the whole file; refusing",
            probe.status())));
    }
    let len = probe.header("content-range").and_then(parse_content_range_total).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, format!("{url}: no total length in Content-Range"))
    })?;
    let url = url.to_string();
    let fetch: Box<dyn FnMut(u64, u64) -> io::Result<Vec<u8>> + Send> = Box::new(move |s, e| {
        let r = get(&agent, &url, s, e)?;
        if r.status() != 206 {
            return Err(io::Error::new(io::ErrorKind::Other, format!("{url}: {} for bytes={s}-{e}", r.status())));
        }
        let mut body = Vec::with_capacity((e - s + 1) as usize);
        r.into_reader().take(e - s + 1).read_to_end(&mut body)?;
        Ok(body)
    });
    Ok(RangeCache::new(len, fetch))
}

/// Is this "path" actually a URL the player should stream over HTTP?
pub fn is_url(path: &str) -> bool {
    let p = path.trim();
    p.starts_with("http://") || p.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An in-memory "server": serves exact byte ranges, counting nothing
    /// itself -- `RangeCache::fetches` does that.
    fn mem(data: Vec<u8>) -> RangeCache<impl FnMut(u64, u64) -> io::Result<Vec<u8>> + Send> {
        let len = data.len() as u64;
        RangeCache::new(len, move |s, e| Ok(data[s as usize..=e as usize].to_vec()))
    }

    fn pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 + i / 251) as u8).collect()
    }

    #[test]
    fn reads_across_block_boundaries_match_the_source() {
        let data = pattern(3 * BLOCK as usize + 1234);
        let mut r = mem(data.clone());
        let mut out = Vec::new();
        r.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
        assert_eq!(r.fetches, 4, "four blocks, each fetched once");
    }

    /// THE CASE THE CACHE EXISTS FOR: many one-byte reads scattered within a
    /// block -- LazyAnchors' marker walk -- cost ONE fetch, not one each.
    #[test]
    fn scattered_small_reads_in_one_block_cost_one_fetch() {
        let data = pattern(BLOCK as usize);
        let mut r = mem(data.clone());
        for p in (0..BLOCK).step_by(997) {
            r.seek(SeekFrom::Start(p)).unwrap();
            let mut b = [0u8; 1];
            r.read_exact(&mut b).unwrap();
            assert_eq!(b[0], data[p as usize]);
        }
        assert_eq!(r.fetches, 1);
    }

    /// Bounded: sweeping more blocks than the cache holds evicts the oldest,
    /// and revisiting an evicted block fetches it again.
    #[test]
    fn the_cache_is_bounded_and_evicts_least_recently_used() {
        let blocks = CACHED_BLOCKS as u64 + 4;
        let mut r = mem(pattern((blocks * BLOCK) as usize));
        for i in 0..blocks {
            r.seek(SeekFrom::Start(i * BLOCK)).unwrap();
            r.read_exact(&mut [0u8; 1]).unwrap();
        }
        assert_eq!(r.fetches, blocks as usize);
        assert!(r.blocks.len() <= CACHED_BLOCKS);
        r.seek(SeekFrom::Start(0)).unwrap(); // block 0 was evicted
        r.read_exact(&mut [0u8; 1]).unwrap();
        assert_eq!(r.fetches, blocks as usize + 1);
        r.seek(SeekFrom::Start((blocks - 1) * BLOCK)).unwrap(); // most recent: still cached
        r.read_exact(&mut [0u8; 1]).unwrap();
        assert_eq!(r.fetches, blocks as usize + 1);
    }

    #[test]
    fn a_short_reply_is_an_error_not_a_silent_shift() {
        let mut r = RangeCache::new(1000, |s, e| Ok(vec![0u8; (e - s) as usize])); // one byte short
        assert_eq!(r.read(&mut [0u8; 10]).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn seeks_and_end_of_file_behave_like_a_file() {
        let data = pattern(5000);
        let mut r = mem(data.clone());
        assert_eq!(r.seek(SeekFrom::End(-10)).unwrap(), 4990);
        let mut t = Vec::new();
        r.read_to_end(&mut t).unwrap();
        assert_eq!(t, &data[4990..]);
        assert_eq!(r.read(&mut [0u8; 4]).unwrap(), 0, "at EOF");
        assert!(r.seek(SeekFrom::Current(-6000)).is_err(), "before the start");
    }

    #[test]
    fn content_range_totals_parse() {
        assert_eq!(parse_content_range_total("bytes 0-0/10"), Some(10));
        assert_eq!(parse_content_range_total("bytes 2-5/5368709120"), Some(5_368_709_120), "past 4 GB");
        assert_eq!(parse_content_range_total("bytes 0-0/*"), None, "unknown total");
        assert_eq!(parse_content_range_total("garbage"), None);
    }

    #[test]
    fn urls_are_told_from_paths() {
        assert!(is_url("http://nas:5000/a.fao"));
        assert!(is_url("  https://x/y.fao"));
        assert!(!is_url("/home/u/a.fao"));
        assert!(!is_url("httpfile.fao"));
    }

    /// Spawn a server on a free-ish port and wait until it answers; returns the
    /// child (killed by its handle, never by a pattern) and the base URL.
    fn serve(cmd: &mut std::process::Command, port: u16) -> (std::process::Child, String) {
        let child = cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .spawn().expect("server starts");
        let base = format!("http://127.0.0.1:{port}");
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() { break; }
        }
        (child, base)
    }

    fn fixture_fao(dir: &std::path::Path) -> Vec<u8> {
        let (w, h) = (48usize, 32usize);
        let frames: Vec<Vec<u8>> = (0..8u8).map(|k| {
            (0..w * h * 3).map(|i| ((i as u32 * 5 + k as u32 * 31) % 256) as u8).collect()
        }).collect();
        let bytes = crate::export::write_dct_fao(&frames, w, h, 30.0, 80).unwrap();
        std::fs::write(dir.join("match #1.fao"), &bytes).unwrap();
        bytes
    }

    /// END TO END against a REAL dufs: a `.fao` played straight off the
    /// server decodes identically to the local file, in a handful of requests.
    /// The name has a space and a '#' -- the characters `media::encode_path`
    /// exists for.
    #[test]
    #[ignore = "needs a dufs binary: DUFS_BIN=/path/to/dufs cargo test -- --ignored real_dufs"]
    fn real_dufs_streams_a_fao_without_downloading_it() {
        let bin = std::env::var("DUFS_BIN").expect("set DUFS_BIN");
        let dir = std::env::temp_dir().join(format!("hr_dufs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bytes = fixture_fao(&dir);
        let port = 40_000 + (std::process::id() % 10_000) as u16;
        let (mut child, base) = serve(std::process::Command::new(&bin).arg(&dir)
            .args(["-p", &port.to_string(), "-b", "127.0.0.1"]), port);
        let result = std::panic::catch_unwind(|| {
            let url = crate::media::file_url(&base, "match #1.fao");
            let reader = open_http(&url).expect("open over http");
            assert_eq!(reader.len(), bytes.len() as u64, "length from Content-Range");
            let local = crate::decode::FaoFile::parse(bytes.clone()).unwrap();
            let remote = crate::decode::FaoFile::open_stream(Box::new(reader)).unwrap();
            for i in 0..local.frame_count() {
                assert_eq!(remote.decode_rgb(i).unwrap(), local.decode_rgb(i).unwrap(), "frame {i}");
            }
        });
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
        if let Err(e) = result { std::panic::resume_unwind(e); }
    }

    /// THE PLAYER, given a dufs URL, streams it: `Deck::open` takes the URL
    /// branch (lazy anchors, no in-memory map), and what it shows matches the
    /// local file. This is the wiring, not just the module.
    #[test]
    #[ignore = "needs a dufs binary: DUFS_BIN=/path/to/dufs cargo test -- --ignored real_dufs"]
    fn real_dufs_the_player_opens_a_url() {
        let bin = std::env::var("DUFS_BIN").expect("set DUFS_BIN");
        let dir = std::env::temp_dir().join(format!("hr_deck_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bytes = fixture_fao(&dir);
        let port = 45_000 + (std::process::id() % 5_000) as u16;
        let (mut child, base) = serve(std::process::Command::new(&bin).arg(&dir)
            .args(["-p", &port.to_string(), "-b", "127.0.0.1"]), port);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut deck = crate::player::Deck::default();
            deck.path = crate::media::file_url(&base, "match #1.fao");
            deck.open();
            assert!(deck.error.is_none(), "{:?}", deck.error);
            assert!(deck.lazy.is_some() && deck.anchor_map.is_none(), "a URL must stream, never load whole");
            let local = crate::decode::FaoFile::parse(bytes.clone()).unwrap();
            let f = deck.file.as_ref().expect("opened");
            assert_eq!(f.frame_count(), local.frame_count());
            assert_eq!(f.decode_rgb(f.frame_count() - 1).unwrap(), local.decode_rgb(local.frame_count() - 1).unwrap());
        }));
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
        if let Err(e) = result { std::panic::resume_unwind(e); }
    }

    /// A server that IGNORES ranges -- Python's http.server answers 200 with
    /// the whole file -- must be refused, not silently downloaded in full.
    #[test]
    #[ignore = "spawns python3 -m http.server: cargo test -- --ignored without_range"]
    fn a_server_without_range_support_is_refused_not_downloaded() {
        let dir = std::env::temp_dir().join(format!("hr_py_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        fixture_fao(&dir);
        let port = 50_000 + (std::process::id() % 10_000) as u16;
        let (mut child, base) = serve(std::process::Command::new("python3")
            .args(["-m", "http.server", &port.to_string(), "--bind", "127.0.0.1", "--directory"]).arg(&dir), port);
        let result = std::panic::catch_unwind(|| {
            let url = crate::media::file_url(&base, "match #1.fao");
            let err = open_http(&url).err().expect("must refuse");
            assert_eq!(err.kind(), io::ErrorKind::Unsupported, "{err}");
            assert!(err.to_string().contains("200"), "{err}");
        });
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
        if let Err(e) = result { std::panic::resume_unwind(e); }
    }

    /// THE REAL DECODER over the cache: a `.fao` read through `RangeCache`
    /// decodes every frame identically to the same bytes parsed in memory.
    /// This is what proves the cache is a faithful `Read + Seek` for the
    /// access pattern that matters, not just for synthetic reads.
    #[test]
    fn a_fao_through_the_cache_decodes_identically() {
        let (w, h) = (48usize, 32usize);
        let frames: Vec<Vec<u8>> = (0..6u8).map(|k| {
            (0..w * h * 3).map(|i| ((i as u32 * 3 + k as u32 * 40) % 256) as u8).collect()
        }).collect();
        let bytes = crate::export::write_dct_fao(&frames, w, h, 30.0, 80).unwrap();
        let local = crate::decode::FaoFile::parse(bytes.clone()).unwrap();
        let remote = crate::decode::FaoFile::open_stream(Box::new(mem(bytes))).unwrap();
        assert_eq!(remote.frame_count(), local.frame_count());
        for i in 0..local.frame_count() {
            assert_eq!(remote.decode_rgb(i).unwrap(), local.decode_rgb(i).unwrap(), "frame {i}");
        }
    }
}
