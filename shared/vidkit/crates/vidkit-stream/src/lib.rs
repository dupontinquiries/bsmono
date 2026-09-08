//! Turning a page URL into a `MediaSource`.
//!
//! Everything needed for this existed and nothing joined it: `Resolver`
//! produced a URL and headers, `HttpByteSource` read bytes over range
//! requests, and the demuxers turned bytes into packets. This is the assembly.
//!
//! It is its own crate rather than a module in `vidkit-ytdlp` because it is the
//! only place that has to know about *both* the resolver and the container
//! parsers, and pushing that knowledge into either would make yt-dlp depend on
//! MP4 or MP4 depend on yt-dlp. Neither is true.
//!
//! # The expiry loop
//!
//! Signed media URLs die, routinely and mid-playback. The sequence is:
//!
//! 1. A read gets a 403; `HttpByteSource` marks itself expired and refuses
//!    further reads rather than returning short data a demuxer would report as
//!    a corrupt file.
//! 2. `next_packet` surfaces `Error::NeedsReresolve`, which `Player` already
//!    treats as an event rather than a failure.
//! 3. The caller calls [`StreamSource::refresh`], which re-resolves and
//!    rebinds the byte source in place.
//!
//! The demuxer is **not** rebuilt. Its sample tables and read position are
//! still valid — only the socket's address changed — and rebuilding would
//! re-download the header and lose the playback position for what is meant to
//! be an invisible swap.

use std::time::Duration;

use vidkit_core::caps::DeviceCaps;
use vidkit_core::codec::VideoProfile;
use vidkit_core::io::ByteSource;
use vidkit_core::traits::{self as core_traits, MediaSource, Packet};
use vidkit_mp4::Mp4Demuxer;
use vidkit_webm::WebmDemuxer;
use vidkit_ytdlp::http::{ChunkPolicy, HttpByteSource, HttpTransport};
use vidkit_ytdlp::{CommandRunner, ResolveOptions, Resolver};

/// Which container the bytes actually are.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Container {
    Mp4,
    WebM,
}

/// Detect the container from the first bytes of the stream.
///
/// Sniffed rather than taken from the format's `ext` or its MIME type: both
/// are extractor metadata and both are wrong often enough to matter, whereas
/// the magic is the file. Costs nothing — the bytes are the ones the demuxer
/// is about to read anyway, and they are already in the byte source's window.
pub fn detect_container(head: &[u8]) -> Option<Container> {
    // EBML magic sits at offset 0.
    if head.len() >= 4 && head[0..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return Some(Container::WebM);
    }
    // ISOBMFF: a box header, with the type at offset 4. `ftyp` normally, but
    // a file that leads with `styp`, `free` or `skip` is still MP4 and a
    // stricter check would reject perfectly good files.
    if head.len() >= 8 {
        match &head[4..8] {
            b"ftyp" | b"styp" | b"moov" | b"free" | b"skip" | b"mdat" => {
                return Some(Container::Mp4)
            }
            _ => {}
        }
    }
    None
}

enum Demux<T: HttpTransport> {
    Mp4(Box<Mp4Demuxer<HttpByteSource<T>>>),
    WebM(Box<WebmDemuxer<HttpByteSource<T>>>),
}

#[derive(Debug)]
pub enum OpenError {
    Resolve(String),
    /// The bytes are neither MP4 nor WebM.
    UnknownContainer,
    Demux(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Resolve(s) => write!(f, "resolve: {s}"),
            OpenError::UnknownContainer => {
                write!(f, "not an MP4 or WebM stream")
            }
            OpenError::Demux(s) => write!(f, "demux: {s}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// A `MediaSource` backed by a resolved remote stream.
pub struct StreamSource<R: CommandRunner, T: HttpTransport> {
    resolver: Resolver<R>,
    demux: Demux<T>,
    caps: DeviceCaps,
    container: Container,
    pub refreshes: u32,
}

/// Builds an `HttpByteSource` for a resolved URL. Injected so tests can supply
/// a mock server without this crate depending on an HTTP client.
pub trait TransportFactory<T: HttpTransport> {
    fn open(&mut self, url: &str) -> T;
}

impl<R: CommandRunner, T: HttpTransport> StreamSource<R, T> {
    /// Resolve `page_url`, sniff the container, and open the matching demuxer.
    pub fn open<F: TransportFactory<T>>(
        runner: R,
        factory: &mut F,
        page_url: &str,
        caps: DeviceCaps,
        opts: ResolveOptions,
        chunks: ChunkPolicy,
    ) -> Result<Self, OpenError> {
        let mut resolver = Resolver::new(runner, page_url, opts);
        let resolved = resolver
            .resolve(&caps)
            .map_err(|e| OpenError::Resolve(format!("{e:?}")))?
            .clone();

        let transport = factory.open(&resolved.video_url);
        // The chunk size is the caller's: a phone on a metered link wants
        // smaller windows than a desktop, and it is also what decides whether
        // a short file is fetched in one request or several.
        let mut bytes = HttpByteSource::new(transport, &resolved.video_url, resolved.headers.clone())
            .with_policy(chunks);

        let mut head = [0u8; 16];
        let n = bytes
            .read_at(0, &mut head)
            .map_err(|e| OpenError::Demux(e.to_string()))?;
        let container = detect_container(&head[..n]).ok_or(OpenError::UnknownContainer)?;

        let demux = match container {
            Container::Mp4 => Demux::Mp4(Box::new(
                Mp4Demuxer::open(bytes).map_err(|e| OpenError::Demux(e.to_string()))?,
            )),
            Container::WebM => Demux::WebM(Box::new(
                WebmDemuxer::open(bytes).map_err(|e| OpenError::Demux(e.to_string()))?,
            )),
        };

        Ok(Self {
            resolver,
            demux,
            caps,
            container,
            refreshes: 0,
        })
    }

    pub fn container(&self) -> Container {
        self.container
    }

    fn byte_source(&mut self) -> &mut HttpByteSource<T> {
        match &mut self.demux {
            Demux::Mp4(d) => d.source_mut(),
            Demux::WebM(d) => d.source_mut(),
        }
    }

    /// True once a read has been rejected and before `refresh`.
    pub fn is_expired(&mut self) -> bool {
        self.byte_source().is_expired()
    }

    /// Re-resolve the expired URL and rebind in place.
    ///
    /// Keeps the demuxer, and therefore the read position and the parsed
    /// tables: the content did not change, only where it is served from.
    pub fn refresh<F: TransportFactory<T>>(&mut self, factory: &mut F) -> Result<(), OpenError> {
        let caps = self.caps.clone();
        let resolved = self
            .resolver
            .refresh(&caps)
            .map_err(|e| OpenError::Resolve(format!("{e:?}")))?
            .clone();
        let transport = factory.open(&resolved.video_url);
        // The transport is replaced too: a new URL may be a different host.
        let bs = self.byte_source();
        bs.replace_transport(transport);
        bs.rebind(&resolved.video_url, resolved.headers.clone());
        self.refreshes += 1;
        Ok(())
    }

    /// Should the URL be refreshed before the next read?
    pub fn needs_refresh(&self, now: std::time::SystemTime) -> bool {
        self.resolver.needs_refresh(now)
    }
}

macro_rules! on_demux {
    ($self:expr, $d:ident => $body:expr) => {
        match &$self.demux {
            Demux::Mp4($d) => $body,
            Demux::WebM($d) => $body,
        }
    };
    (mut $self:expr, $d:ident => $body:expr) => {
        match &mut $self.demux {
            Demux::Mp4($d) => $body,
            Demux::WebM($d) => $body,
        }
    };
}

impl<R: CommandRunner + Send, T: HttpTransport + Send> MediaSource for StreamSource<R, T> {
    fn video_profile(&self) -> Option<VideoProfile> {
        on_demux!(self, d => d.video_profile())
    }
    fn dimensions(&self) -> (u32, u32) {
        on_demux!(self, d => d.dimensions())
    }
    fn nominal_fps(&self) -> u32 {
        on_demux!(self, d => d.nominal_fps())
    }
    fn duration(&self) -> Option<Duration> {
        on_demux!(self, d => d.duration())
    }
    fn seekable(&self) -> bool {
        true
    }

    fn next_packet(&mut self) -> core_traits::Result<Option<Packet>> {
        let expired = self.byte_source().is_expired();
        if expired {
            // Surfaced as the recoverable event, not as a backend error: the
            // player keeps its decoder and buffer pool and the caller calls
            // refresh(). Reporting this as a generic failure would tear down
            // playback for something that happens every few hours by design.
            return Err(core_traits::Error::NeedsReresolve);
        }
        match on_demux!(mut self, d => d.next_packet()) {
            Ok(p) => Ok(p),
            Err(e) => {
                // A read failure that expired the source mid-call is the same
                // recoverable case, arriving one layer down.
                if self.byte_source().is_expired() {
                    Err(core_traits::Error::NeedsReresolve)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn seek(&mut self, to: Duration) -> core_traits::Result<()> {
        on_demux!(mut self, d => d.seek(to))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    use vidkit_core::caps::desktop_full_hw;
    use vidkit_ytdlp::http::{HttpError, RangeRequest};
    use vidkit_ytdlp::RunError;

    // ---- container sniffing ------------------------------------------

    #[test]
    fn ebml_magic_is_webm() {
        assert_eq!(
            detect_container(&[0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0]),
            Some(Container::WebM)
        );
    }

    #[test]
    fn an_isobmff_box_type_is_mp4() {
        let mut head = vec![0, 0, 0, 0x18];
        head.extend_from_slice(b"ftyp");
        assert_eq!(detect_container(&head), Some(Container::Mp4));
    }

    /// A file that leads with something other than `ftyp` is still MP4. A
    /// stricter check rejects files that play perfectly.
    #[test]
    fn other_leading_boxes_are_still_mp4() {
        for kind in [b"styp", b"free", b"skip", b"moov", b"mdat"] {
            let mut head = vec![0, 0, 0, 0x10];
            head.extend_from_slice(kind);
            assert_eq!(
                detect_container(&head),
                Some(Container::Mp4),
                "{} rejected",
                String::from_utf8_lossy(kind)
            );
        }
    }

    #[test]
    fn garbage_and_short_reads_are_not_guessed_at() {
        assert_eq!(detect_container(b""), None);
        assert_eq!(detect_container(b"\x00\x01"), None);
        assert_eq!(detect_container(b"<!DOCTYPE html>"), None);
        // A truncated EBML magic must not match.
        assert_eq!(detect_container(&[0x1A, 0x45, 0xDF]), None);
    }

    // ---- the assembled source ------------------------------------------

    /// Serves one body over "HTTP", and can be told to 403 from a given
    /// request onward — which is how a signed URL expires mid-playback.
    /// Shared switch, so a test can let `open` succeed and THEN kill the URL.
    /// Counting requests instead means guessing how many the header walk
    /// takes, and that number changes with the chunk size.
    type Forbid = std::sync::Arc<std::sync::atomic::AtomicBool>;

    struct MockServer {
        body: Vec<u8>,
        forbid: Forbid,
    }

    impl HttpTransport for MockServer {
        fn fetch(&mut self, req: &RangeRequest) -> Result<Vec<u8>, HttpError> {
            if self.forbid.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(HttpError::Forbidden);
            }
            let start = req.start as usize;
            if start >= self.body.len() {
                return Ok(Vec::new());
            }
            let end = ((req.end as usize) + 1).min(self.body.len());
            Ok(self.body[start..end].to_vec())
        }
        fn known_length(&self) -> Option<u64> {
            Some(self.body.len() as u64)
        }
    }

    struct Factory {
        body: Vec<u8>,
        forbid: Forbid,
        pub opened: Vec<String>,
    }

    impl TransportFactory<MockServer> for Factory {
        fn open(&mut self, url: &str) -> MockServer {
            self.opened.push(url.to_string());
            MockServer {
                body: self.body.clone(),
                forbid: self.forbid.clone(),
            }
        }
    }

    struct ScriptedRunner(VecDeque<String>);

    impl CommandRunner for ScriptedRunner {
        fn run(&mut self, _p: &str, _a: &[String]) -> Result<Vec<u8>, RunError> {
            match self.0.pop_front() {
                Some(s) => Ok(s.into_bytes()),
                None => Err(RunError::Io("script exhausted".into())),
            }
        }
    }

    fn json(url: &str) -> String {
        format!(
            r#"{{"formats":[
              {{"format_id":"137","url":"{url}","vcodec":"avc1.640028","acodec":"none",
                "width":1920,"height":1080,"fps":30,"vbr":4200.0,"protocol":"https",
                "http_headers":{{"User-Agent":"Mozilla/5.0 test"}}}}]}}"#
        )
    }

    /// A minimal but real MP4, from the demuxer's own fixture writer.
    fn mp4_body_n(n: u32) -> Vec<u8> {
        use vidkit_mp4::build::*;
        let sps: &[u8] = &[0x67, 0x64, 0x00, 0x28, 0xAA];
        let pps: &[u8] = &[0x68, 0xEE, 0x3C, 0xB0];
        let mut payloads = Vec::new();
        for i in 0..n {
            let b = vec![0xA0 | (i as u8 & 0x0F); 6];
            let mut s = (b.len() as u32).to_be_bytes().to_vec();
            s.extend_from_slice(&b);
            payloads.push(s);
        }
        let sizes: Vec<u32> = payloads.iter().map(|p| p.len() as u32).collect();
        let mut mdat_body = Vec::new();
        let mut chunk_rel = Vec::new();
        for pair in payloads.chunks(2) {
            chunk_rel.push(mdat_body.len());
            for p in pair {
                mdat_body.extend_from_slice(p);
            }
        }
        let mdat = bx(b"mdat", &mdat_body);
        let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41");
        let make = |at: u64| {
            let stco = table(
                b"stco",
                &chunk_rel.iter().map(|r| u32b((at + *r as u64) as u32)).collect::<Vec<_>>(),
            );
            let stbl = bx(
                b"stbl",
                &cat(&[
                    stsd(visual_entry(b"avc1", 8, 8, avcc(4, sps, pps))),
                    table(b"stts", &[cat(&[u32b(n), u32b(1000)])]),
                    table(b"stsc", &[cat(&[u32b(1), u32b(2), u32b(1)])]),
                    stsz(&sizes),
                    stco,
                ]),
            );
            let minf = bx(b"minf", &cat(&[bx(b"vmhd", &[0u8; 12]), stbl]));
            let mdia = bx(b"mdia", &cat(&[mdhd(30_000, n * 1000), hdlr(b"vide"), minf]));
            let trak = bx(b"trak", &cat(&[tkhd(1, 8, 8), mdia]));
            bx(b"moov", &cat(&[bx(b"mvhd", &[0u8; 100]), trak]))
        };
        let probe = make(0);
        let at = (ftyp.len() + probe.len() + 8) as u64;
        let moov = make(at);
        assert_eq!(moov.len(), probe.len());
        cat(&[ftyp, moov, mdat])
    }

    fn mp4_body() -> Vec<u8> {
        mp4_body_n(6)
    }

    fn open_stream_with(
        forbid: Forbid,
        chunk_bytes: u64,
        samples: u32,
    ) -> (StreamSource<ScriptedRunner, MockServer>, Factory) {
        let mut factory = Factory {
            body: mp4_body_n(samples),
            forbid,
            opened: Vec::new(),
        };
        let runner = ScriptedRunner(
            vec![json("https://cdn/one"), json("https://cdn/two")]
                .into_iter()
                .collect(),
        );
        let s = StreamSource::open(
            runner,
            &mut factory,
            "https://site/watch?v=1",
            desktop_full_hw(),
            ResolveOptions::default(),
            ChunkPolicy { chunk_bytes, retries: 0 },
        )
        .expect("open");
        (s, factory)
    }

    fn open_stream() -> (StreamSource<ScriptedRunner, MockServer>, Factory) {
        open_stream_with(Forbid::default(), 256 * 1024, 6)
    }

    #[test]
    fn a_page_url_becomes_a_playable_media_source() {
        let (mut s, factory) = open_stream();
        assert_eq!(s.container(), Container::Mp4);
        assert_eq!(s.dimensions(), (8, 8));

        // The resolved media URL is what gets fetched, not the page URL.
        assert_eq!(factory.opened, vec!["https://cdn/one".to_string()]);

        let mut n = 0;
        while let Some(p) = s.next_packet().unwrap() {
            if n == 0 {
                assert!(p.is_keyframe);
                assert_eq!(&p.data[0..4], &[0, 0, 0, 1], "not Annex-B");
            }
            n += 1;
        }
        assert_eq!(n, 6, "read {n} packets over HTTP");
    }

    /// The whole point: a 403 mid-playback is recoverable, and must arrive as
    /// NeedsReresolve so the player keeps its decoder and buffer pool.
    #[test]
    fn an_expired_url_surfaces_as_needs_reresolve() {
        // A 128-byte window, deliberately. The fixture is 678 bytes, so with
        // the default 256 KB chunk the WHOLE FILE arrives in one request and
        // no later read can ever 403 -- the expiry this test is about becomes
        // unreachable and the test passes without exercising anything. Small
        // windows force real re-reads.
        let forbid = Forbid::default();
        // 60 samples and a 128-byte window: the sample data then spans many
        // windows, so playback genuinely re-reads. With a short file every
        // sample lands in the window fetched during open, no later read can
        // 403, and the expiry this test is about is unreachable.
        let (mut s, _f) = open_stream_with(forbid.clone(), 128, 60);
        // Let the header parse, THEN kill the URL.
        forbid.store(true, std::sync::atomic::Ordering::SeqCst);

        let mut saw = None;
        for _ in 0..20 {
            match s.next_packet() {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    saw = Some(e);
                    break;
                }
            }
        }
        assert!(
            matches!(saw, Some(core_traits::Error::NeedsReresolve)),
            "expired URL reported as {saw:?}, which tears down playback"
        );
    }

    #[test]
    fn refreshing_rebinds_to_the_new_url_and_resumes() {
        let forbid = Forbid::default();
        let mut factory = Factory {
            // Big enough that playback re-reads: with a small window and a
            // short file, every sample lands in the window fetched during open
            // and no later read can ever 403.
            body: mp4_body_n(60),
            forbid: forbid.clone(),
            opened: Vec::new(),
        };
        let runner = ScriptedRunner(
            vec![json("https://cdn/one"), json("https://cdn/two")]
                .into_iter()
                .collect(),
        );
        let mut s = StreamSource::open(
            runner,
            &mut factory,
            "https://site/watch?v=1",
            desktop_full_hw(),
            ResolveOptions::default(),
            ChunkPolicy { chunk_bytes: 128, retries: 0 },
        )
        .expect("open");

        // Kill the URL mid-file, then drive until a read hits it.
        //
        // `while ... .is_ok() {}` is a HANG, not a failure: Ok(None) is ok, so
        // once the clip ends the loop never terminates. It has to stop on both
        // arms.
        forbid.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut hit_eof = false;
        loop {
            match s.next_packet() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    hit_eof = true;
                    break;
                }
                Err(_) => break,
            }
        }
        assert!(!hit_eof, "reached the end before the URL died: nothing to resume");
        assert!(s.is_expired(), "the URL never actually died");

        // The replacement must not be dead on arrival, or the test cannot tell
        // a working refresh from a broken one.
        forbid.store(false, std::sync::atomic::Ordering::SeqCst);
        s.refresh(&mut factory).expect("refresh");

        assert_eq!(s.refreshes, 1);
        assert!(!s.is_expired(), "still expired after a refresh");
        assert_eq!(
            factory.opened.last().map(String::as_str),
            Some("https://cdn/two"),
            "refresh did not switch to the newly resolved URL"
        );

        // ...and playback continues rather than restarting.
        assert!(
            s.next_packet().is_ok(),
            "no packets after a refresh: the rebind did not take"
        );
    }

    /// Rebuilding the demuxer on refresh would re-download the header and lose
    /// the position. The read position must survive.
    #[test]
    fn a_refresh_keeps_the_playback_position() {
        let forbid = Forbid::default();
        let (mut s, mut factory) = open_stream_with(forbid.clone(), 128, 60);

        // Play a little, then kill the URL mid-file. Playing to EOF first
        // leaves nothing to resume and the assertion below cannot bind.
        let mut before = Vec::new();
        for _ in 0..3 {
            match s.next_packet() {
                Ok(Some(p)) => before.push(p.pts),
                Ok(None) => panic!("clip ended during warm-up"),
                Err(e) => panic!("warm-up failed: {e}"),
            }
        }

        forbid.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut hit_eof = false;
        loop {
            match s.next_packet() {
                Ok(Some(p)) => before.push(p.pts),
                Ok(None) => {
                    hit_eof = true;
                    break;
                }
                Err(_) => break,
            }
        }
        assert!(!hit_eof, "reached the end before the URL died: nothing to resume");
        assert!(s.is_expired(), "the URL never actually died");

        forbid.store(false, std::sync::atomic::Ordering::SeqCst);
        s.refresh(&mut factory).unwrap();

        let after = match s.next_packet() {
            Ok(Some(p)) => p.pts,
            Ok(None) => panic!("nothing played after the refresh"),
            Err(e) => panic!("read failed after the refresh: {e}"),
        };
        assert!(
            after > *before.last().unwrap(),
            "playback restarted at {after:?} after {:?}: the demuxer was rebuilt \
             instead of rebound, losing the position",
            before.last().unwrap()
        );
    }

    #[test]
    fn a_body_that_is_neither_container_is_refused() {
        let mut factory = Factory {
            body: b"<!DOCTYPE html><html>not a video</html>".to_vec(),
            forbid: Forbid::default(),
            opened: Vec::new(),
        };
        let runner = ScriptedRunner(vec![json("https://cdn/one")].into_iter().collect());
        let e = match StreamSource::open(
            runner,
            &mut factory,
            "https://site/x",
            desktop_full_hw(),
            ResolveOptions::default(),
            ChunkPolicy::default(),
        ) {
            Err(e) => e,
            Ok(_) => panic!("opened an HTML error page as a video"),
        };
        assert!(
            matches!(e, OpenError::UnknownContainer),
            "misreported as {e}"
        );
    }
}
