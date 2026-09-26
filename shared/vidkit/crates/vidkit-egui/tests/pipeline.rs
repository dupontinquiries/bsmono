//! The pipeline, end to end, across crate boundaries.
//!
//! Everything below this line has been tested in isolation and never together:
//! `Mp4Demuxer` against synthetic files, `Player` against mocks defined inside
//! `vidkit-core`, `EguiPresenter` against hand-built frames. That leaves the
//! seams themselves unexercised, and a seam is exactly where an
//! independently-correct pair disagrees — a `MediaSource` whose `seek` lands
//! somewhere the player did not expect, a `duration` in the wrong units, a
//! `Presenter` whose buffer accounting the player does not match.
//!
//! So this uses the REAL demuxer, the REAL player loop and the REAL presenter,
//! with only the codec stubbed — there is no decoder that runs in this sandbox,
//! and stubbing it is honest because the codec is not what these seams are
//! about.
//!
//! The MP4 is built with `vidkit_mp4::build`, the same writer the demuxer's own
//! tests use, rather than a second copy free to drift from the parser it is
//! meant to exercise.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use vidkit_core::frame::{ColorInfo, CpuPlanes, Frame, FrameData, PlaneLayout, RecycleToken};
use vidkit_core::player::{Event, Player};
use vidkit_core::traits::{Clock, DecodeOutput, Decoder, MediaSource, Packet, Presenter, Result};
use vidkit_egui::EguiPresenter;
use vidkit_mp4::build::*;
use vidkit_mp4::{MemorySource, Mp4Demuxer};

const POOL: u32 = 4;
const W: u32 = 8;
const H: u32 = 8;

// ---------------------------------------------------------------------------
// a real MP4
// ---------------------------------------------------------------------------

/// H.264-shaped MP4: `n` samples at `timescale/30` ticks apart, every third a
/// sync sample, two samples per chunk so the stsc walk is exercised rather
/// than bypassed.
fn synthetic_mp4(n: u32) -> Vec<u8> {
    let sps: &[u8] = &[0x67, 0x64, 0x00, 0x28, 0xAA];
    let pps: &[u8] = &[0x68, 0xEE, 0x3C, 0xB0];
    let timescale = 30_000u32;
    let delta = 1000u32; // 30 fps

    // One length-prefixed NAL per sample, body keyed to the index so a
    // mis-mapped offset shows up as the wrong colour rather than as nothing.
    let mut payloads = Vec::new();
    for i in 0..n {
        let body = vec![0xA0 | (i as u8 & 0x0F); 6];
        let mut s = (body.len() as u32).to_be_bytes().to_vec();
        s.extend_from_slice(&body);
        payloads.push(s);
    }
    let sizes: Vec<u32> = payloads.iter().map(|p| p.len() as u32).collect();

    // Two samples per chunk, with a gap between chunks.
    const GAP: usize = 5;
    let mut mdat_body = Vec::new();
    let mut chunk_rel = Vec::new();
    for pair in payloads.chunks(2) {
        chunk_rel.push(mdat_body.len());
        for p in pair {
            mdat_body.extend_from_slice(p);
        }
        mdat_body.extend(std::iter::repeat(0xFF).take(GAP));
    }
    let mdat = bx(b"mdat", &mdat_body);
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41");

    let sync: Vec<u32> = (0..n).filter(|i| i % 3 == 0).map(|i| i + 1).collect();

    let make_moov = |mdat_at: u64| -> Vec<u8> {
        let stco = table(
            b"stco",
            &chunk_rel
                .iter()
                .map(|r| u32b((mdat_at + *r as u64) as u32))
                .collect::<Vec<_>>(),
        );
        let stbl = bx(
            b"stbl",
            &cat(&[
                stsd(visual_entry(b"avc1", W, H, avcc(4, sps, pps))),
                table(b"stts", &[cat(&[u32b(n), u32b(delta)])]),
                table(b"stsc", &[cat(&[u32b(1), u32b(2), u32b(1)])]),
                stsz(&sizes),
                stco,
                table(b"stss", &sync.iter().map(|s| u32b(*s)).collect::<Vec<_>>()),
            ]),
        );
        let minf = bx(b"minf", &cat(&[bx(b"vmhd", &[0u8; 12]), stbl]));
        let mdia = bx(
            b"mdia",
            &cat(&[mdhd(timescale, n * delta), hdlr(b"vide"), minf]),
        );
        let trak = bx(b"trak", &cat(&[tkhd(1, W, H), mdia]));
        bx(b"moov", &cat(&[bx(b"mvhd", &[0u8; 100]), trak]))
    };

    let probe = make_moov(0);
    let mdat_at = (ftyp.len() + probe.len() + 8) as u64;
    let moov = make_moov(mdat_at);
    assert_eq!(moov.len(), probe.len(), "moov length must be stable");
    cat(&[ftyp, moov, mdat])
}

// ---------------------------------------------------------------------------
// the one stub: a codec
// ---------------------------------------------------------------------------

#[derive(Default)]
struct DecoderState {
    free: Vec<u32>,
    ready: Vec<(Duration, u8)>,
    accepted: Vec<Duration>,
    double_recycled: Vec<u32>,
}

/// Turns each packet into a flat NV12 frame whose luma encodes the packet's
/// payload byte, and owns a fixed buffer pool exactly as a V4L2 decoder does.
struct StubDecoder(Arc<Mutex<DecoderState>>);

impl Decoder for StubDecoder {
    fn backend_name(&self) -> &str {
        "stub"
    }

    fn send_packet(&mut self, pkt: &Packet) -> Result<bool> {
        let mut d = self.0.lock().unwrap();
        if d.ready.len() >= 2 {
            return Ok(false); // back-pressure, same as a real input queue
        }
        // The demuxer converted to Annex-B, so the payload begins with a start
        // code and the parameter sets ride on keyframes. Read the last byte,
        // which is the index marker the fixture wrote.
        let marker = *pkt.data.last().unwrap_or(&0);
        let pts = pkt.pts.unwrap_or(Duration::ZERO);
        d.accepted.push(pts);
        d.ready.push((pts, marker));
        Ok(true)
    }

    fn receive_frame(&mut self) -> Result<DecodeOutput> {
        let mut d = self.0.lock().unwrap();
        if d.ready.is_empty() {
            return Ok(DecodeOutput::Idle);
        }
        let Some(token) = d.free.pop() else {
            // Pool exhausted: exactly what a real decoder does when buffers are
            // not returned.
            return Ok(DecodeOutput::Idle);
        };
        let (pts, marker) = d.ready.remove(0);
        Ok(DecodeOutput::Frame(Frame {
            data: FrameData::Cpu(CpuPlanes {
                data: vec![
                    vec![marker; (W * H) as usize],
                    vec![128u8; (W * H / 2) as usize],
                ],
                strides: vec![W, W],
            }),
            width: W,
            height: H,
            layout: PlaneLayout::Nv12,
            color: ColorInfo::default(),
            pts,
            duration: Some(Duration::from_millis(33)),
            recycle: Some(RecycleToken(token)),
            grain: None,
        }))
    }

    fn recycle(&mut self, token: RecycleToken) -> Result<()> {
        let mut d = self.0.lock().unwrap();
        if d.free.contains(&token.0) {
            d.double_recycled.push(token.0);
        }
        d.free.push(token.0);
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.0.lock().unwrap().ready.clear();
        Ok(())
    }
}

/// Forwards to a real `Mp4Demuxer`, so the player drives the actual parser.
struct SourceHandle(Mp4Demuxer<MemorySource>);

impl MediaSource for SourceHandle {
    fn video_profile(&self) -> Option<vidkit_core::VideoProfile> {
        self.0.video_profile()
    }
    fn dimensions(&self) -> (u32, u32) {
        self.0.dimensions()
    }
    fn nominal_fps(&self) -> u32 {
        self.0.nominal_fps()
    }
    fn duration(&self) -> Option<Duration> {
        self.0.duration()
    }
    fn seekable(&self) -> bool {
        self.0.seekable()
    }
    fn next_packet(&mut self) -> Result<Option<Packet>> {
        self.0.next_packet()
    }
    fn seek(&mut self, to: Duration) -> Result<()> {
        self.0.seek(to)
    }
}

/// A presenter the test can inspect, wrapping the real `EguiPresenter`.
struct PresenterHandle {
    inner: EguiPresenter,
    shown: Arc<Mutex<Vec<Duration>>>,
}

impl Presenter for PresenterHandle {
    fn submit(&mut self, frame: Frame) -> Result<Option<RecycleToken>> {
        self.shown.lock().unwrap().push(frame.pts);
        self.inner.submit(frame)
    }
    fn release_all(&mut self) -> Vec<RecycleToken> {
        self.inner.release_all()
    }
    fn next_deadline(&self) -> Option<std::time::Instant> {
        self.inner.next_deadline()
    }
    fn take_drop_count(&mut self) -> u32 {
        self.inner.take_drop_count()
    }
}

/// A clock the test drives by hand.
///
/// A real `MonotonicClock` makes this depend on wall time: the tick loop
/// finishes hundreds of iterations in the time one 33ms frame interval takes,
/// so nothing is ever due and playback looks stalled. Advancing by exactly the
/// `wake_in` the player asks for is deterministic AND a stronger check — it
/// verifies the player's own scheduling advice is sufficient to make progress.
#[derive(Clone)]
struct FakeClock(Arc<Mutex<Duration>>);

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        *self.0.lock().unwrap()
    }
    fn set_rate(&mut self, _r: f64) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn is_paused(&self) -> bool {
        false
    }
}

struct Rig {
    player: Player,
    dec: Arc<Mutex<DecoderState>>,
    shown: Arc<Mutex<Vec<Duration>>>,
    now: Arc<Mutex<Duration>>,
}

fn rig(samples: u32) -> Rig {
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput::default());

    let demux = Mp4Demuxer::open(MemorySource(synthetic_mp4(samples))).expect("open mp4");
    assert_eq!(demux.sample_count(), samples as usize, "fixture is wrong");

    let dec = Arc::new(Mutex::new(DecoderState {
        free: (0..POOL).collect(),
        ..Default::default()
    }));
    let shown = Arc::new(Mutex::new(Vec::new()));

    let now = Arc::new(Mutex::new(Duration::ZERO));

    Rig {
        player: Player::new(
            Box::new(SourceHandle(demux)),
            Box::new(StubDecoder(dec.clone())),
            Box::new(PresenterHandle {
                inner: EguiPresenter::new(&ctx),
                shown: shown.clone(),
            }),
            Box::new(FakeClock(now.clone())),
            60,
            3,
        ),
        dec,
        shown,
        now,
    }
}

/// Tick until `f` says stop or the budget runs out.
fn run(r: &mut Rig, max: usize, mut f: impl FnMut(&Event) -> bool) -> Vec<Event> {
    let mut evs = Vec::new();
    for _ in 0..max {
        let e = r.player.tick().expect("tick");
        if let Event::Idle { wake_in: Some(d) } = e {
            *r.now.lock().unwrap() += d;
        }
        let done = f(&e);
        evs.push(e);
        if done {
            break;
        }
    }
    evs
}

// ---------------------------------------------------------------------------

#[test]
fn a_real_mp4_plays_through_the_real_player_into_the_real_presenter() {
    let mut r = rig(12);
    run(&mut r, 600, |_| false);

    let shown = r.shown.lock().unwrap().clone();
    assert!(
        shown.len() >= 6,
        "only {} frames reached the presenter",
        shown.len()
    );

    let mut sorted = shown.clone();
    sorted.sort();
    assert_eq!(shown, sorted, "frames presented out of order across the stack");

    // Timestamps must arrive in media time, not ticks. 30 fps at a 30000
    // timescale means 33ms apart; a unit mix-up shows up here as 1000x.
    assert!(
        shown[1] < Duration::from_millis(100),
        "second frame at {:?} -- timescale is not being applied",
        shown[1]
    );
}

/// The pool is smaller than the clip, so playback only completes if buffers
/// come back through the presenter and the player on every tick.
#[test]
fn a_clip_longer_than_the_buffer_pool_does_not_deadlock() {
    let mut r = rig(24);
    run(&mut r, 1200, |_| false);

    let shown = r.shown.lock().unwrap().len();
    assert!(
        shown > POOL as usize,
        "stalled after {shown} frames with a pool of {POOL}: buffers are not \
         returning through the presenter"
    );
    assert!(
        r.dec.lock().unwrap().double_recycled.is_empty(),
        "a buffer was recycled twice: {:?}",
        r.dec.lock().unwrap().double_recycled
    );
}

/// The seam most likely to disagree: the demuxer seeks to a sync sample, the
/// player flushes, and the two must end up describing the same position.
#[test]
fn seeking_lands_on_a_keyframe_and_playback_resumes() {
    let mut r = rig(24);

    // Tick only until the decoder is holding undecoded packets. A seek issued
    // once the queue has drained cannot observe whether the player flushed the
    // decoder -- there is nothing stale left to leak -- and the flush check
    // below silently passes. This is the precondition, so it is asserted.
    let mut pending = 0;
    for _ in 0..300 {
        r.player.tick().expect("tick");
        if let Event::Idle { wake_in: Some(d) } = r.player.tick().expect("tick") {
            *r.now.lock().unwrap() += d;
        }
        pending = r.dec.lock().unwrap().ready.len();
        if pending > 0 && r.shown.lock().unwrap().len() >= 2 {
            break;
        }
    }
    assert!(
        pending > 0,
        "decoder queue was empty at seek time: the flush check cannot bind"
    );
    let before = r.shown.lock().unwrap().len();

    // 330ms deliberately: samples are 33ms apart and every third is a sync
    // sample, so 330ms is sample 10 and NOT a keyframe. Seeking to 300ms --
    // which is sample 9, a keyframe -- makes the correct and the broken
    // behaviour agree, and the assertion below cannot bind.
    r.player.seek(Duration::from_millis(330)).expect("seek");

    // Every buffer must be back after a seek; the presenter releasing and the
    // decoder reclaiming are two different crates agreeing.
    assert_eq!(
        r.dec.lock().unwrap().free.len(),
        POOL as usize,
        "seek left buffers outstanding across the crate boundary"
    );

    r.shown.lock().unwrap().clear();
    run(&mut r, 600, |_| false);
    let after = r.shown.lock().unwrap().clone();
    assert!(!after.is_empty(), "playback did not resume after a seek");

    // Exactly the keyframe at or before 330ms, which is sample 9 at 300ms.
    //
    // An equality, not a bound. `<=` passes on two different bugs: a demuxer
    // that does not rewind lands at 330ms (caught), but a player that skips
    // the decoder flush replays STALE frames from before the seek, whose
    // timestamps are earlier and satisfy any upper bound. Both were NOT CAUGHT
    // by the first version of this test.
    assert_eq!(
        after[0],
        Duration::from_millis(300),
        "resumed at {:?}: expected the keyframe at 300ms -- later means the \
         demuxer did not rewind, earlier means stale frames survived the flush",
        after[0]
    );
}

/// Back-pressure crosses the boundary intact: the stub refuses packets once
/// its queue is full, and the player must retry the same packet rather than
/// dropping it, or the bitstream is corrupt.
#[test]
fn back_pressure_does_not_lose_packets() {
    let mut r = rig(12);
    run(&mut r, 600, |_| false);

    let accepted = r.dec.lock().unwrap().accepted.clone();
    let mut sorted = accepted.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        accepted, sorted,
        "a packet was sent twice or out of order under back-pressure"
    );
    assert!(accepted.len() >= 6, "only {} packets accepted", accepted.len());
}

/// The demuxer's Annex-B conversion has to survive the trip: the first packet
/// the decoder sees must carry the parameter sets, or a real decoder emits
/// nothing.
#[test]
fn the_first_packet_across_the_seam_carries_its_parameter_sets() {
    let mut demux = Mp4Demuxer::open(MemorySource(synthetic_mp4(6))).unwrap();
    let first = demux.next_packet().unwrap().unwrap();
    assert!(first.is_keyframe);
    assert_eq!(&first.data[0..4], &[0, 0, 0, 1], "not Annex-B");
    assert!(
        first.data.windows(5).any(|w| w == [0x67, 0x64, 0x00, 0x28, 0xAA]),
        "SPS missing from the first packet"
    );
}

// ---------------------------------------------------------------------------
// the same seams, over WebM
// ---------------------------------------------------------------------------
//
// Not a copy of the MP4 cases for symmetry's sake. WebM is the FAIRPHONE's
// path -- YouTube's VP9 renditions are WebM and VP9 is what Venus decodes in
// hardware -- and it reaches the player through a completely different set of
// answers: nanosecond timestamps scaled by TimestampScale rather than
// timescale ticks, keyframe flags read from a SimpleBlock header rather than
// an stss table, and no Annex-B rewrite at all because VP9 frames pass through
// untouched. Every one of those is a place the two demuxers could disagree
// with the player, and only one of them had ever been driven by it.

mod webm_pipeline {
    use super::*;
    use vidkit_webm::build as w;
    use vidkit_webm::ebml::id;
    use vidkit_webm::WebmDemuxer;

    /// `n` VP9 frames, 33ms apart, every third a keyframe, split across two
    /// clusters so the cluster-timestamp arithmetic is exercised rather than
    /// bypassed.
    fn synthetic_webm(n: u32) -> Vec<u8> {
        let per_cluster = 4u32;
        let mut clusters = Vec::new();
        let mut i = 0u32;
        while i < n {
            let base = (i as u64) * 33;
            let mut body = w::uint(id::TIMESTAMP, base);
            for k in 0..per_cluster {
                if i + k >= n {
                    break;
                }
                let idx = i + k;
                let data = vec![0xB0 | (idx as u8 & 0x0F); 6];
                body.extend_from_slice(&w::simple_block(
                    1,
                    (k as i16) * 33,
                    idx % 3 == 0,
                    &data,
                ));
            }
            clusters.push(w::elem(id::CLUSTER, &body));
            i += per_cluster;
        }

        let video = w::elem(
            id::VIDEO,
            &w::cat(&[
                w::uint(id::PIXEL_WIDTH, W as u64),
                w::uint(id::PIXEL_HEIGHT, H as u64),
            ]),
        );
        let entry = w::cat(&[
            w::uint(id::TRACK_NUMBER, 1),
            w::uint(id::TRACK_TYPE, 1),
            w::string(id::CODEC_ID, "V_VP9"),
            video,
        ]);
        let info = w::cat(&[
            w::uint(id::TIMESTAMP_SCALE, 1_000_000),
            w::float64(id::DURATION, (n as f64) * 33.0),
        ]);
        let body = w::cat(&[
            w::elem(id::INFO, &info),
            w::elem(id::TRACKS, &w::elem(id::TRACK_ENTRY, &entry)),
            w::cat(&clusters),
        ]);
        w::cat(&[
            w::elem(id::EBML_HEADER, &[0x42, 0x86, 0x81, 0x01]),
            w::elem(id::SEGMENT, &body),
        ])
    }

    struct WebmHandle(WebmDemuxer<vidkit_mp4::MemorySource>);

    impl MediaSource for WebmHandle {
        fn video_profile(&self) -> Option<vidkit_core::VideoProfile> {
            self.0.video_profile()
        }
        fn dimensions(&self) -> (u32, u32) {
            self.0.dimensions()
        }
        fn nominal_fps(&self) -> u32 {
            self.0.nominal_fps()
        }
        fn duration(&self) -> Option<Duration> {
            self.0.duration()
        }
        fn seekable(&self) -> bool {
            self.0.seekable()
        }
        fn next_packet(&mut self) -> Result<Option<Packet>> {
            self.0.next_packet()
        }
        fn seek(&mut self, to: Duration) -> Result<()> {
            self.0.seek(to)
        }
    }

    fn webm_rig(n: u32) -> Rig {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());

        let demux = WebmDemuxer::open(vidkit_mp4::MemorySource(synthetic_webm(n)))
            .expect("open webm");
        assert_eq!(demux.block_count(), n as usize, "fixture is wrong");

        let dec = Arc::new(Mutex::new(DecoderState {
            free: (0..POOL).collect(),
            ..Default::default()
        }));
        let shown = Arc::new(Mutex::new(Vec::new()));
        let now = Arc::new(Mutex::new(Duration::ZERO));

        Rig {
            player: Player::new(
                Box::new(WebmHandle(demux)),
                Box::new(StubDecoder(dec.clone())),
                Box::new(PresenterHandle {
                    inner: EguiPresenter::new(&ctx),
                    shown: shown.clone(),
                }),
                Box::new(FakeClock(now.clone())),
                60,
                3,
            ),
            dec,
            shown,
            now,
        }
    }

    #[test]
    fn a_real_webm_plays_through_the_whole_stack() {
        let mut r = webm_rig(12);
        run(&mut r, 600, |_| false);

        let shown = r.shown.lock().unwrap().clone();
        assert!(shown.len() >= 6, "only {} frames presented", shown.len());

        let mut sorted = shown.clone();
        sorted.sort();
        assert_eq!(shown, sorted, "frames presented out of order");

        // Nanosecond timestamps scaled by TimestampScale, not raw ticks. A
        // unit mix-up shows up here as a factor of a million.
        assert_eq!(shown[0], Duration::ZERO);
        assert_eq!(
            shown[1],
            Duration::from_millis(33),
            "second frame at {:?}: TimestampScale is not being applied",
            shown[1]
        );
    }

    /// The cluster boundary is the arithmetic worth checking: a block's
    /// timestamp is its cluster's plus a signed relative offset, so a demuxer
    /// that forgets the cluster base restarts the timeline every four frames
    /// and the frame queue drops everything after the first cluster.
    #[test]
    fn timestamps_keep_climbing_across_a_cluster_boundary() {
        let mut r = webm_rig(12);
        run(&mut r, 600, |_| false);
        let shown = r.shown.lock().unwrap().clone();

        // Four blocks per cluster, so index 4 is the first of cluster two.
        assert!(shown.len() > 5, "not enough frames to cross a cluster");
        assert!(
            shown[4] > shown[3],
            "timeline restarted at the cluster boundary: {:?} then {:?}",
            shown[3],
            shown[4]
        );
        assert_eq!(shown[4], Duration::from_millis(132));
    }

    /// VP9 needs no Annex-B rewrite. The demuxer must hand the frame over
    /// byte-identical; "helpfully" prefixing a start code corrupts it.
    #[test]
    fn vp9_frames_cross_the_seam_untouched() {
        let mut demux =
            WebmDemuxer::open(vidkit_mp4::MemorySource(synthetic_webm(3))).unwrap();
        let p = demux.next_packet().unwrap().unwrap();
        assert_eq!(p.data, vec![0xB0u8; 6], "VP9 payload was rewritten");
        assert!(p.is_keyframe);
    }

    #[test]
    fn seeking_a_webm_lands_on_a_keyframe_and_resumes() {
        let mut r = webm_rig(24);

        let mut pending = 0;
        for _ in 0..300 {
            r.player.tick().expect("tick");
            if let Event::Idle { wake_in: Some(d) } = r.player.tick().expect("tick") {
                *r.now.lock().unwrap() += d;
            }
            pending = r.dec.lock().unwrap().ready.len();
            if pending > 0 && r.shown.lock().unwrap().len() >= 2 {
                break;
            }
        }
        assert!(pending > 0, "decoder queue empty: the flush check cannot bind");

        // 330ms is block 10, NOT a keyframe -- blocks 0/3/6/9 are. Seeking to
        // a keyframe would let correct and broken behaviour agree.
        r.player.seek(Duration::from_millis(330)).expect("seek");
        assert_eq!(
            r.dec.lock().unwrap().free.len(),
            POOL as usize,
            "seek left buffers outstanding across the crate boundary"
        );

        r.shown.lock().unwrap().clear();
        run(&mut r, 600, |_| false);
        let after = r.shown.lock().unwrap().clone();
        assert!(!after.is_empty(), "playback did not resume");
        assert_eq!(
            after[0],
            Duration::from_millis(297),
            "resumed at {:?}: expected the keyframe at 297ms -- later means no \
             rewind, earlier means stale frames survived the flush",
            after[0]
        );
    }
}
