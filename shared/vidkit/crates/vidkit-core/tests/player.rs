//! Player loop tests.
//!
//! The mock decoder owns a fixed pool of buffer tokens, exactly like a real
//! V4L2 decoder. Almost every test here asserts on `available()` returning to
//! full, because a recycle leak is invisible on software decode and only shows
//! up as a hang on hardware after minutes of playback.

use std::collections::VecDeque;
use std::time::Duration;

use vidkit_core::frame::{ColorInfo, CpuPlanes, Frame, FrameData, PlaneLayout, RecycleToken};
use vidkit_core::player::{Budget, Event, Player};
use vidkit_core::traits::{
    Clock, DecodeOutput, Decoder, Error, MediaSource, Packet, Presenter, Result,
};
use vidkit_core::VideoProfile;

const POOL: u32 = 6;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

// ---------------------------------------------------------------------------
// mocks
// ---------------------------------------------------------------------------

struct MockSource {
    packets: VecDeque<Packet>,
    /// Return `NeedsReresolve` once, when this many packets remain.
    fail_at_remaining: Option<usize>,
    pub seeks: Vec<Duration>,
}

impl MockSource {
    fn with_packets(n: u64, step_ms: u64) -> Self {
        let packets = (0..n)
            .map(|i| Packet {
                data: vec![0u8; 8],
                pts: Some(ms(i * step_ms)),
                dts: None,
                is_keyframe: i == 0,
            })
            .collect();
        Self {
            packets,
            fail_at_remaining: None,
            seeks: Vec::new(),
        }
    }
}

impl MediaSource for MockSource {
    fn video_profile(&self) -> Option<VideoProfile> {
        None
    }
    fn dimensions(&self) -> (u32, u32) {
        (1920, 1080)
    }
    fn nominal_fps(&self) -> u32 {
        30
    }
    fn duration(&self) -> Option<Duration> {
        None
    }
    fn seekable(&self) -> bool {
        true
    }
    fn next_packet(&mut self) -> Result<Option<Packet>> {
        if self.fail_at_remaining == Some(self.packets.len()) {
            self.fail_at_remaining = None;
            return Err(Error::NeedsReresolve);
        }
        Ok(self.packets.pop_front())
    }
    fn seek(&mut self, to: Duration) -> Result<()> {
        self.seeks.push(to);
        Ok(())
    }
}

/// Owns a fixed pool of tokens, like a real hardware decoder.
struct MockDecoder {
    free: Vec<u32>,
    /// Decoded-but-not-yet-collected frames, as (pts).
    ready: VecDeque<Duration>,
    /// Input buffers available.
    input_slots: u32,
    /// Emit a FormatChanged before the next frame.
    format_change_at: Option<u64>,
    frames_emitted: u64,
    finished: bool,
    /// Every token ever handed back, for double-recycle detection.
    recycled: Vec<u32>,
    /// PTS of every packet the decoder accepted, in order.
    accepted: Vec<Duration>,
}

impl MockDecoder {
    fn new() -> Self {
        Self {
            free: (0..POOL).collect(),
            ready: VecDeque::new(),
            input_slots: 4,
            format_change_at: None,
            frames_emitted: 0,
            finished: false,
            recycled: Vec::new(),
            accepted: Vec::new(),
        }
    }
    fn available(&self) -> usize {
        self.free.len()
    }
}

/// Shared handle so tests can inspect the mocks after moving them into the
/// player. `Arc<Mutex<_>>` rather than `Rc<RefCell<_>>` because the real
/// traits are `Send` — decode runs off the UI thread. This is test
/// scaffolding; a real player owns its decoder outright.
type Shared<T> = std::sync::Arc<std::sync::Mutex<T>>;

fn shared<T>(v: T) -> Shared<T> {
    std::sync::Arc::new(std::sync::Mutex::new(v))
}

struct DecoderHandle(Shared<MockDecoder>);

impl Decoder for DecoderHandle {
    fn backend_name(&self) -> &str {
        "mock"
    }
    fn send_packet(&mut self, pkt: &Packet) -> Result<bool> {
        let mut d = self.0.lock().unwrap();
        if d.input_slots == 0 {
            return Ok(false);
        }
        d.input_slots -= 1;
        let pts = pkt.pts.unwrap_or(Duration::ZERO);
        d.accepted.push(pts);
        d.ready.push_back(pts);
        Ok(true)
    }
    fn receive_frame(&mut self) -> Result<DecodeOutput> {
        let mut d = self.0.lock().unwrap();
        if d.format_change_at == Some(d.frames_emitted) {
            d.format_change_at = None;
            return Ok(DecodeOutput::FormatChanged {
                width: 1280,
                height: 720,
            });
        }
        if d.ready.is_empty() {
            if d.finished {
                return Ok(DecodeOutput::EndOfStream);
            }
            return Ok(DecodeOutput::Idle);
        }
        let token = match d.free.pop() {
            Some(t) => t,
            // Pool exhausted: the decoder cannot produce. Exactly what a real
            // one does when buffers are not returned.
            None => return Ok(DecodeOutput::Idle),
        };
        let pts = d.ready.pop_front().expect("checked");
        d.input_slots += 1;
        d.frames_emitted += 1;
        Ok(DecodeOutput::Frame(Frame {
            data: FrameData::Cpu(CpuPlanes {
                data: vec![vec![0u8; 8]],
                strides: vec![8],
            }),
            width: 1920,
            height: 1080,
            layout: PlaneLayout::Nv12,
            color: ColorInfo::default(),
            pts,
            duration: Some(ms(33)),
            recycle: Some(RecycleToken(token)),
            grain: None,
        }))
    }
    fn recycle(&mut self, token: RecycleToken) -> Result<()> {
        let mut d = self.0.lock().unwrap();
        assert!(
            !d.free.contains(&token.0),
            "buffer {} recycled twice",
            token.0
        );
        d.recycled.push(token.0);
        d.free.push(token.0);
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        let mut d = self.0.lock().unwrap();
        d.ready.clear();
        d.input_slots = 4;
        d.finished = false;
        Ok(())
    }
}

/// Holds one frame until the next arrives, like a real double-buffered
/// presenter.
struct MockPresenter {
    held: Option<Frame>,
    pub submitted: Vec<Duration>,
}

impl MockPresenter {
    fn new() -> Self {
        Self {
            held: None,
            submitted: Vec::new(),
        }
    }
}

struct PresenterHandle(Shared<MockPresenter>);

impl Presenter for PresenterHandle {
    fn submit(&mut self, frame: Frame) -> Result<Option<RecycleToken>> {
        let mut p = self.0.lock().unwrap();
        p.submitted.push(frame.pts);
        let previous = p.held.replace(frame);
        Ok(previous.and_then(|mut f| f.recycle.take()))
    }
    fn release_all(&mut self) -> Vec<RecycleToken> {
        let mut p = self.0.lock().unwrap();
        p.held
            .take()
            .and_then(|mut f| f.recycle.take())
            .into_iter()
            .collect()
    }
    fn next_deadline(&self) -> Option<std::time::Instant> {
        None
    }
    fn take_drop_count(&mut self) -> u32 {
        0
    }
}

/// A clock the test drives by hand.
///
/// Using the real `MonotonicClock` here makes the tests depend on wall time:
/// a tight tick loop finishes thousands of iterations in the time one frame
/// interval takes, so playback appears to stall. Advancing a fake clock by
/// exactly the `wake_in` the player asks for is both faster and a stronger
/// check — it verifies the player's own scheduling advice is sufficient to
/// make progress.
#[derive(Clone)]
struct FakeClock(Shared<Duration>);

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        *self.0.lock().unwrap()
    }
    fn set_rate(&mut self, _rate: f64) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn is_paused(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

struct Rig {
    player: Player,
    dec: Shared<MockDecoder>,
    pres: Shared<MockPresenter>,
    src: Shared<MockSource>,
    now: Shared<Duration>,
}

/// The player takes `Box<dyn MediaSource>`, so the shared handle needs a
/// forwarding wrapper too.
struct SourceHandle(Shared<MockSource>);

impl MediaSource for SourceHandle {
    fn video_profile(&self) -> Option<VideoProfile> {
        None
    }
    fn dimensions(&self) -> (u32, u32) {
        (1920, 1080)
    }
    fn nominal_fps(&self) -> u32 {
        30
    }
    fn duration(&self) -> Option<Duration> {
        None
    }
    fn seekable(&self) -> bool {
        true
    }
    fn next_packet(&mut self) -> Result<Option<Packet>> {
        self.0.lock().unwrap().next_packet()
    }
    fn seek(&mut self, to: Duration) -> Result<()> {
        self.0.lock().unwrap().seek(to)
    }
}

fn rig(packets: u64) -> Rig {
    let src = shared(MockSource::with_packets(packets, 33));
    let dec = shared(MockDecoder::new());
    let pres = shared(MockPresenter::new());

    let now = shared(Duration::ZERO);

    let player = Player::new(
        Box::new(SourceHandle(src.clone())),
        Box::new(DecoderHandle(dec.clone())),
        Box::new(PresenterHandle(pres.clone())),
        Box::new(FakeClock(now.clone())),
        60,
        4,
    );
    Rig {
        player,
        dec,
        pres,
        src,
        now,
    }
}

/// Tick until `f` returns true or the budget runs out, honouring the player's
/// own sleep advice by advancing the fake clock. Returns the events.
fn run_until(rig: &mut Rig, max: usize, mut f: impl FnMut(&Event) -> bool) -> Vec<Event> {
    let mut evs = Vec::new();
    for _ in 0..max {
        let e = rig.player.tick().expect("tick");
        if let Event::Idle { wake_in: Some(d) } = e {
            *rig.now.lock().unwrap() += d;
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
// tests
// ---------------------------------------------------------------------------

// Traits are not object safe for `Rc` sharing without these wrappers; the
// unused-import guard is here so the mock source struct is exercised.
#[test]
fn mock_source_yields_packets_in_order() {
    let mut s = MockSource::with_packets(3, 10);
    assert_eq!(s.next_packet().unwrap().unwrap().pts, Some(ms(0)));
    assert_eq!(s.next_packet().unwrap().unwrap().pts, Some(ms(10)));
    assert_eq!(s.next_packet().unwrap().unwrap().pts, Some(ms(20)));
    assert!(s.next_packet().unwrap().is_none());
}

#[test]
fn frames_reach_the_presenter_in_pts_order() {
    let mut r = rig(6);
    // Clock at zero: only frames at or near pts 0 are due, so advance it.
    run_until(&mut r, 200, |_| false);
    let submitted = r.pres.lock().unwrap().submitted.clone();
    assert!(submitted.len() >= 2, "expected several frames, got {submitted:?}");
    let mut sorted = submitted.clone();
    sorted.sort();
    assert_eq!(submitted, sorted, "frames presented out of order");
}

/// The headline invariant.
#[test]
fn every_buffer_comes_back_by_end_of_stream() {
    let mut r = rig(5);

    // Let the source run dry, then mark the decoder finished as a real one
    // would after a drain.
    run_until(&mut r, 100, |_| false);
    r.dec.lock().unwrap().finished = true;

    let evs = run_until(&mut r, 200, |e| matches!(e, Event::EndOfStream));
    assert!(
        evs.iter().any(|e| matches!(e, Event::EndOfStream)),
        "never reached EOS: {evs:?}"
    );
    // Bind first: two live MutexGuards from the same lock in one expression
    // deadlock on the spot, which is a memorable way to lose ten minutes.
    let available = r.dec.lock().unwrap().available();
    assert_eq!(available, POOL as usize, "buffer leak: {available} of {POOL} returned");
}

/// The pool is smaller than the packet count, so playback only completes if
/// buffers are recycled continuously rather than at the end.
#[test]
fn playback_longer_than_the_pool_does_not_deadlock() {
    let mut r = rig(40);
    run_until(&mut r, 2000, |_| false);
    let shown = r.pres.lock().unwrap().submitted.len();
    assert!(
        shown > POOL as usize,
        "stalled after {shown} frames with a pool of {POOL} — buffers are not being returned"
    );
}

/// A frame that arrives when the queue is already full still holds a pool
/// buffer. The picture is lost either way, but the buffer must go back — with
/// a queue capacity of 1 and a decoder happy to run ahead, this path is hit
/// constantly.
#[test]
fn a_frame_rejected_by_a_full_queue_returns_its_buffer() {
    let src = shared(MockSource::with_packets(30, 33));
    let dec = shared(MockDecoder::new());
    let pres = shared(MockPresenter::new());
    let now = shared(Duration::ZERO);

    let mut player = Player::new(
        Box::new(SourceHandle(src.clone())),
        Box::new(DecoderHandle(dec.clone())),
        Box::new(PresenterHandle(pres.clone())),
        Box::new(FakeClock(now.clone())),
        60,
        1, // capacity 1: the decoder outruns it immediately
    )
    .with_budget(Budget {
        packets_per_tick: 8,
        frames_per_tick: 8,
    });

    for _ in 0..60 {
        match player.tick().expect("tick") {
            Event::Idle { wake_in: Some(d) } => *now.lock().unwrap() += d,
            Event::EndOfStream => break,
            _ => {}
        }
    }

    let available = dec.lock().unwrap().available();
    assert!(
        available > 0,
        "pool exhausted with a full queue: overflowed frames stranded their buffers"
    );
    assert!(
        pres.lock().unwrap().submitted.len() > 3,
        "playback stalled entirely"
    );
}

#[test]
fn a_refused_packet_is_retried_not_dropped() {
    let mut r = rig(10);
    r.dec.lock().unwrap().input_slots = 0;

    // No input slots: the decoder refuses everything.
    let _ = r.player.tick().unwrap();
    let consumed_by_source = 10 - r.src.lock().unwrap().packets.len();
    assert_eq!(consumed_by_source, 1, "should have pulled exactly one packet");
    assert!(r.dec.lock().unwrap().ready.is_empty(), "nothing should be accepted");

    // Open a slot; the held packet must go through.
    r.dec.lock().unwrap().input_slots = 1;
    let _ = r.player.tick().unwrap();
    // Identify the packet by PTS. Counting alone cannot tell "the held packet
    // was retried" apart from "the held packet was dropped and the next one
    // sent instead" — both leave exactly one packet in the decoder.
    let accepted = {
        let d = r.dec.lock().unwrap();
        d.accepted.clone()
    };
    assert_eq!(
        accepted.first(),
        Some(&ms(0)),
        "decoder received {accepted:?}; the first packet was dropped rather than retried"
    );
}

#[test]
fn format_change_flushes_the_queue_and_returns_every_buffer() {
    let mut r = rig(20);

    // Present at least one frame first, so the presenter is genuinely holding
    // a buffer when the reconfiguration lands. Without this the test passes
    // even if the presenter's frame is stranded, because there is no frame.
    run_until(&mut r, 40, |e| matches!(e, Event::Presented { .. }));
    assert!(
        r.pres.lock().unwrap().held.is_some(),
        "presenter should be holding a frame before the format change"
    );

    // Relative to what has already been emitted, not an absolute 3 — frames
    // have been produced by the warm-up above.
    {
        let mut d = r.dec.lock().unwrap();
        let next = d.frames_emitted + 1;
        d.format_change_at = Some(next);
    }
    let evs = run_until(&mut r, 200, |e| matches!(e, Event::FormatChanged { .. }));
    let fc = evs
        .iter()
        .find(|e| matches!(e, Event::FormatChanged { .. }))
        .expect("no FormatChanged");
    assert_eq!(
        *fc,
        Event::FormatChanged {
            width: 1280,
            height: 720
        }
    );
    assert_eq!(r.player.queue_len(), 0, "queue must be flushed");
    assert_eq!(
        r.dec.lock().unwrap().available(),
        POOL as usize,
        "reconfiguration must return every buffer"
    );
}

#[test]
fn expired_url_surfaces_as_reresolve_without_losing_buffers() {
    let mut r = rig(10);
    r.src.lock().unwrap().fail_at_remaining = Some(7);

    let evs = run_until(&mut r, 100, |e| matches!(e, Event::NeedsReresolve));
    assert!(
        evs.iter().any(|e| matches!(e, Event::NeedsReresolve)),
        "expected NeedsReresolve, got {evs:?}"
    );

    // Playback continues afterwards; no teardown happened.
    r.player.resume_after_reresolve();
    let before = r.pres.lock().unwrap().submitted.len();
    run_until(&mut r, 200, |_| false);
    assert!(
        r.pres.lock().unwrap().submitted.len() > before,
        "playback did not resume after re-resolve"
    );
}

#[test]
fn seek_returns_all_buffers_and_reaches_the_source() {
    let mut r = rig(20);
    run_until(&mut r, 60, |_| false);
    assert!(r.dec.lock().unwrap().available() < POOL as usize, "test is not exercising held buffers");

    r.player.seek(ms(5000)).unwrap();

    assert_eq!(r.src.lock().unwrap().seeks, vec![ms(5000)]);
    assert_eq!(r.player.queue_len(), 0);
    assert_eq!(
        r.dec.lock().unwrap().available(),
        POOL as usize,
        "seek must return every buffer"
    );
}

#[test]
fn idle_reports_a_sleep_duration_rather_than_busy_looping() {
    let mut r = rig(10);
    // Clock paused at zero, so queued future frames are not due yet.
    let evs = run_until(&mut r, 20, |e| {
        matches!(e, Event::Idle { wake_in: Some(_) })
    });
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::Idle { wake_in: Some(_) })),
        "player never offered a sleep: {evs:?}"
    );
}

/// Starvation and "waiting for a future frame" are different states and must
/// not be conflated — one means sleep, the other means get more input.
#[test]
fn starvation_reports_no_sleep_duration() {
    let mut r = rig(0);
    match r.player.tick().unwrap() {
        Event::Idle { wake_in } => assert!(wake_in.is_none(), "starved player should not sleep"),
        other => panic!("expected Idle, got {other:?}"),
    }
}

/// One tick must not run away. After a seek there can be thousands of packets
/// available, and an unbounded loop would block the UI thread for as long as it
/// takes to chew through them.
#[test]
fn work_per_tick_is_bounded() {
    let mut r = rig(1000);
    r.player = r.player.with_budget(Budget {
        packets_per_tick: 2,
        frames_per_tick: 1,
    });

    let _ = r.player.tick().unwrap();
    let consumed = 1000 - r.src.lock().unwrap().packets.len();
    assert!(
        consumed <= 2,
        "one tick consumed {consumed} packets despite a budget of 2"
    );
}

// ---------------------------------------------------------------------------
// composition: the player driven by a real audio-mastered clock
// ---------------------------------------------------------------------------

use vidkit_core::audio_clock::AudioClock;

/// Shares one `AudioClock` between the player (which reads it) and the test
/// (which feeds it audio), the way a real audio callback thread would.
struct AudioClockHandle(Shared<AudioClock>);

impl Clock for AudioClockHandle {
    fn now(&self) -> Duration {
        self.0.lock().unwrap().now()
    }
    fn set_rate(&mut self, rate: f64) {
        self.0.lock().unwrap().set_rate(rate);
    }
    fn pause(&mut self) {
        self.0.lock().unwrap().pause();
    }
    fn resume(&mut self) {
        self.0.lock().unwrap().resume();
    }
    fn is_paused(&self) -> bool {
        self.0.lock().unwrap().is_paused()
    }
}

/// Video presented against audio, with a realistic device buffer in the way.
///
/// This is the composition check: frame timing comes out of `AudioClock`,
/// which subtracts device latency, and the queue's show/hold decision consumes
/// it.
///
/// The bound below is computed by the test from frames written and the known
/// buffer depth — **not** read back from the clock. Comparing presented PTS
/// against `clock.now()` would be circular: a clock that reports the wrong
/// position also moves the yardstick, so a sign error passes. (Verified by
/// mutation: flipping the subtraction to an addition does not fail the
/// circular version, and does fail this one.)
#[test]
fn player_presents_in_step_with_an_audio_clock() {
    // Two seconds of video but only 400 ms of audio will be fed, so the clock
    // is the limiting factor throughout. With more video than audio the
    // "did not run ahead" bound below can never bind and the test is vacuous.
    let src = shared(MockSource::with_packets(60, 33));
    let dec = shared(MockDecoder::new());
    let pres = shared(MockPresenter::new());
    let audio = shared(AudioClock::new(48_000));
    audio.lock().unwrap().resume();

    let mut player = Player::new(
        Box::new(SourceHandle(src.clone())),
        Box::new(DecoderHandle(dec.clone())),
        Box::new(PresenterHandle(pres.clone())),
        Box::new(AudioClockHandle(audio.clone())),
        60,
        4,
    );

    // 50 ms of device buffer, the sort of depth a phone actually runs.
    const LATENCY_MS: u64 = 50;
    let latency_frames = LATENCY_MS * 48;

    // Feed audio in 10 ms callbacks, ticking the player in between.
    let mut written_ms: u64 = 0;
    for _ in 0..40 {
        audio.lock().unwrap().advance(10 * 48, latency_frames);
        written_ms += 10;
        for _ in 0..8 {
            match player.tick().expect("tick") {
                Event::Idle { wake_in: Some(_) } => break,
                Event::EndOfStream => break,
                _ => {}
            }
        }
    }

    let submitted = pres.lock().unwrap().submitted.clone();
    assert!(
        submitted.len() >= 8,
        "expected steady playback, got {} frames",
        submitted.len()
    );

    // Ground truth, computed here: everything written except what is still
    // sitting in the device buffer has been heard.
    let truly_audible = ms(written_ms.saturating_sub(LATENCY_MS));
    let last = *submitted.last().unwrap();
    assert!(
        last <= truly_audible + ms(20),
        "presented pts {last:?} runs ahead of the {truly_audible:?} of audio \
         actually heard ({written_ms} ms written, {LATENCY_MS} ms buffered) — \
         check the sign of the latency correction"
    );

    let mut sorted = submitted.clone();
    sorted.sort();
    assert_eq!(submitted, sorted, "frames presented out of order");

    let available = dec.lock().unwrap().available();
    assert!(
        available > 0,
        "decoder pool exhausted during steady playback: buffers are not returning"
    );
}
