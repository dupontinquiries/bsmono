//! Recording a sequence of frames — the admission rules, with no encoder.
//!
//! C14_TODO §15 step 1. The egui port's shutter produces one `RawFrame`; a
//! video needs a sequence with timestamps, and the frame pump already supplies
//! both (`RawFrame` carries `captured_ms` and `seq`). So this is a sink that
//! accepts frames as they arrive, not a new capture path.
//!
//! # The rule that shapes everything: frames are NOT accumulated
//!
//! A `Vec<RawFrame>` is the obvious design and it is unusable. RGBA at 1080p is
//! 8.3 MB a frame; at 30 fps that is 249 MB per second of video, so a ten
//! second clip is 2.5 GB of resident memory before anything is encoded.
//!
//! This is exactly the bug fixed in `voice-kit` at 366b8354 — an unbounded
//! buffer that only shrank on a flush, reported from a device as 9.9 GiB
//! resident on an on-screen keyboard. Writing the same shape again a day later
//! would be inexcusable, so [`Recorder::offer`] hands each admitted frame
//! straight to a caller-supplied sink and keeps NOTHING but counters.
//!
//! The sink is where an encoder goes — a gstreamer `appsrc` push, an ffmpeg
//! stdin write, a temp file. All three are streaming, which is why this shape
//! costs nothing to adopt and why the alternative was never necessary.
//!
//! # What it does keep
//!
//! Enough to answer "is this recording still valid, and should it stop":
//! frame count, first and last capture time, admitted bytes, and the drops it
//! refused. Counters, not pixels.

use crate::frame_pump::RawFrame;

/// Why a frame was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// `seq` is not greater than the last admitted one.
    ///
    /// A duplicate or a reordered frame. Admitting either corrupts the
    /// timeline: a duplicate makes the clip run long, and a frame from the
    /// past makes the encoder's timestamps go backwards, which most muxers
    /// either reject or silently clamp.
    NotNewer,
    /// The dimensions changed mid-recording.
    ///
    /// A rotation or a format renegotiation. AV1 cannot change resolution
    /// mid-stream without starting a new sequence, so admitting this produces
    /// either a corrupt file or a silently cropped one.
    SizeChanged,
    /// The recording has hit [`Limits::max_secs`].
    TooLong,
    /// The pixel buffer does not match the stated dimensions.
    ///
    /// A torn frame. `pixels_plausible` exists on `RawFrame` for the same
    /// reason and the stills path already checks it; a video that admits one
    /// gets a single garbage frame in the middle rather than an error.
    Torn,
}

/// Bounds on a recording.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Longest recording, in seconds. Beyond this `offer` refuses.
    ///
    /// A cap rather than "until the user stops", because the sink is a
    /// subprocess or a file and the failure mode of an unattended recording is
    /// a full disk on a phone.
    pub max_secs: u32,
}

impl Default for Limits {
    fn default() -> Limits {
        // 60 seconds. Long enough for what this app is for — a snap, not a
        // film — and at AV1 CRF 32 it is a few megabytes rather than the
        // hundreds a raw buffer would have been.
        Limits { max_secs: 60 }
    }
}

/// A recording in progress.
#[derive(Debug, Clone)]
pub struct Recorder {
    limits: Limits,
    size: Option<(u32, u32)>,
    last_seq: Option<u64>,
    first_ms: Option<u64>,
    last_ms: u64,
    frames: u64,
    bytes: u64,
    refused: u64,
}

impl Recorder {
    pub fn new(limits: Limits) -> Recorder {
        Recorder {
            limits,
            size: None,
            last_seq: None,
            first_ms: None,
            last_ms: 0,
            frames: 0,
            bytes: 0,
            refused: 0,
        }
    }

    /// Offer a frame. On acceptance it is passed to `sink` and forgotten.
    ///
    /// The sink runs only for ADMITTED frames, so an encoder never sees a
    /// duplicate, a reorder, a resize or a torn buffer — which is the point of
    /// putting these rules in front of it rather than inside it.
    pub fn offer(
        &mut self,
        f: &RawFrame,
        sink: &mut dyn FnMut(&RawFrame),
    ) -> Result<(), Refused> {
        let expect = f.width as usize * f.height as usize * 4;
        if f.rgba.len() != expect || f.width == 0 || f.height == 0 {
            self.refused += 1;
            return Err(Refused::Torn);
        }
        if let Some(sz) = self.size {
            if sz != (f.width, f.height) {
                self.refused += 1;
                return Err(Refused::SizeChanged);
            }
        }
        if let Some(last) = self.last_seq {
            if f.seq <= last {
                self.refused += 1;
                return Err(Refused::NotNewer);
            }
        }
        // Checked AFTER the frame is known good, so a torn frame at the limit
        // reports why it is torn rather than why the clip is long.
        if self.first_ms.is_some() && self.duration_secs() >= self.limits.max_secs as f32 {
            self.refused += 1;
            return Err(Refused::TooLong);
        }

        self.size = Some((f.width, f.height));
        self.last_seq = Some(f.seq);
        self.first_ms.get_or_insert(f.captured_ms);
        self.last_ms = f.captured_ms;
        self.frames += 1;
        self.bytes += expect as u64;
        sink(f);
        Ok(())
    }

    /// Seconds between the first and last admitted frame.
    ///
    /// From capture TIMESTAMPS, not from a frame count over an assumed rate: a
    /// camera that drops frames under load produces a clip whose real duration
    /// is what the clock says, and dividing count by nominal fps would report
    /// a ten second recording as seven.
    pub fn duration_secs(&self) -> f32 {
        match self.first_ms {
            Some(first) => (self.last_ms.saturating_sub(first)) as f32 / 1000.0,
            None => 0.0,
        }
    }

    /// Frames per second actually achieved, or `None` before two frames.
    ///
    /// For choosing the encoder's `fps` — `snapclone_av1policy::plan_for` takes
    /// one and derives the GOP length from it, so passing a nominal 30 for a
    /// stream running at 12 produces keyframes two and a half times further
    /// apart than intended.
    pub fn measured_fps(&self) -> Option<f32> {
        let d = self.duration_secs();
        if self.frames < 2 || d <= 0.0 {
            return None;
        }
        Some((self.frames - 1) as f32 / d)
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Raw bytes admitted. What a `Vec<RawFrame>` design would have been
    /// holding, and the reason this one holds a counter instead.
    pub fn raw_bytes(&self) -> u64 {
        self.bytes
    }

    pub fn refused(&self) -> u64 {
        self.refused
    }

    pub fn size(&self) -> Option<(u32, u32)> {
        self.size
    }

    /// Has the recording reached its limit?
    pub fn at_limit(&self) -> bool {
        self.first_ms.is_some() && self.duration_secs() >= self.limits.max_secs as f32
    }

    /// The encoder plan for this recording, at `tier`.
    ///
    /// Uses the SHARED policy crate rather than a second copy of the rules.
    /// `Av1Policy.hpp` on the Qt side says in its own header that it mirrors
    /// `snapclone_av1policy::plan_for`; reimplementing it here would make a
    /// third version of numbers that are meant to be one version.
    pub fn plan(&self, tier: snapclone_av1policy::Tier) -> snapclone_av1policy::Plan {
        // Measured fps where known, and the policy's own default otherwise —
        // it treats 0 as "use 30", so a one-frame recording does not need a
        // special case here.
        let fps = self.measured_fps().map(|f| f.round() as u32).unwrap_or(0);
        snapclone_av1policy::plan_for(tier, /*seekable=*/ true, fps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(seq: u64, ms: u64, w: u32, h: u32) -> RawFrame {
        RawFrame {
            width: w,
            height: h,
            captured_ms: ms,
            seq,
            rgba: vec![0u8; (w * h * 4) as usize].into(),
        }
    }

    /// A sink that counts rather than keeps — the same discipline the recorder
    /// itself follows.
    fn counting() -> (std::rc::Rc<std::cell::Cell<u32>>, impl FnMut(&RawFrame)) {
        let n = std::rc::Rc::new(std::cell::Cell::new(0));
        let c = n.clone();
        (n, move |_: &RawFrame| c.set(c.get() + 1))
    }

    #[test]
    fn frames_are_passed_to_the_sink_and_not_retained() {
        // The rule the module exists for. RGBA at 1080p is 8.3 MB a frame and
        // 249 MB a second; the voice-kit leak (366b8354) was this shape and
        // reached 9.9 GiB on a device.
        let (n, mut sink) = counting();
        let mut r = Recorder::new(Limits::default());
        for i in 0..100 {
            r.offer(&f(i, i * 33, 64, 48), &mut sink).expect("admitted");
        }
        assert_eq!(n.get(), 100);
        assert_eq!(r.frames(), 100);
        // The recorder's own size must not grow with the recording. If it held
        // frames this would be megabytes.
        assert!(std::mem::size_of::<Recorder>() < 128,
                "Recorder is {} bytes — is it holding frames?", std::mem::size_of::<Recorder>());
        // And it still knows what it took in, without having kept it.
        assert_eq!(r.raw_bytes(), 100 * 64 * 48 * 4);
    }

    #[test]
    fn a_duplicate_or_reordered_frame_is_refused() {
        // A duplicate makes the clip run long; a frame from the past makes the
        // encoder's timestamps go backwards, which muxers reject or silently
        // clamp.
        let (n, mut sink) = counting();
        let mut r = Recorder::new(Limits::default());
        r.offer(&f(5, 100, 32, 32), &mut sink).unwrap();
        assert_eq!(r.offer(&f(5, 133, 32, 32), &mut sink), Err(Refused::NotNewer));
        assert_eq!(r.offer(&f(4, 66, 32, 32), &mut sink), Err(Refused::NotNewer));
        assert_eq!(n.get(), 1, "a refused frame reached the sink");
        assert_eq!(r.refused(), 2);
    }

    #[test]
    fn a_resize_mid_recording_is_refused() {
        // A rotation or a format renegotiation. AV1 cannot change resolution
        // mid-stream, so admitting this gives a corrupt or silently cropped
        // file rather than an error.
        let (_, mut sink) = counting();
        let mut r = Recorder::new(Limits::default());
        r.offer(&f(1, 0, 64, 48), &mut sink).unwrap();
        assert_eq!(r.offer(&f(2, 33, 48, 64), &mut sink), Err(Refused::SizeChanged));
        assert_eq!(r.size(), Some((64, 48)), "the refused size was adopted anyway");
    }

    #[test]
    fn a_torn_frame_is_refused_and_says_so() {
        // The stills path checks `pixels_plausible` for the same reason. A
        // video that admits a torn frame gets one garbage frame in the middle,
        // which is harder to diagnose than a refusal.
        let (_, mut sink) = counting();
        let mut r = Recorder::new(Limits::default());
        let mut bad = f(1, 0, 64, 48);
        bad.rgba = vec![0u8; 10].into();
        assert_eq!(r.offer(&bad, &mut sink), Err(Refused::Torn));
        // Zero-sized too, which would divide by zero downstream.
        assert_eq!(r.offer(&f(2, 0, 0, 48), &mut sink), Err(Refused::Torn));
    }

    #[test]
    fn the_duration_comes_from_timestamps_not_from_a_frame_count() {
        // A camera that drops frames under load produces a clip whose real
        // duration is what the clock says. Count over nominal fps would report
        // a 3-second recording as 0.3.
        let (_, mut sink) = counting();
        let mut r = Recorder::new(Limits::default());
        r.offer(&f(1, 1_000, 32, 32), &mut sink).unwrap();
        r.offer(&f(2, 4_000, 32, 32), &mut sink).unwrap();
        assert!((r.duration_secs() - 3.0).abs() < 1e-6);
        // Ten frames would be 0.33s at 30fps; the clock says 3.
        assert!(r.measured_fps().unwrap() < 1.0);
    }

    #[test]
    fn a_recording_stops_at_its_limit() {
        // Unattended recording fills a phone. Both directions: it must refuse
        // past the cap and admit below it.
        let (n, mut sink) = counting();
        let mut r = Recorder::new(Limits { max_secs: 2 });
        r.offer(&f(1, 0, 32, 32), &mut sink).unwrap();
        r.offer(&f(2, 1_000, 32, 32), &mut sink).expect("1s is under a 2s cap");
        assert!(!r.at_limit());
        r.offer(&f(3, 2_000, 32, 32), &mut sink).unwrap();
        assert!(r.at_limit());
        assert_eq!(r.offer(&f(4, 2_100, 32, 32), &mut sink), Err(Refused::TooLong));
        assert_eq!(n.get(), 3);
    }

    #[test]
    fn the_plan_uses_the_measured_rate_and_the_shared_policy() {
        // `plan_for` derives the GOP from fps, so a nominal 30 on a stream
        // running at 12 puts keyframes two and a half times further apart than
        // intended.
        let (_, mut sink) = counting();
        let mut r = Recorder::new(Limits::default());
        // 10 fps: ten frames 100 ms apart.
        for i in 0..10u64 {
            r.offer(&f(i + 1, i * 100, 32, 32), &mut sink).unwrap();
        }
        let fps = r.measured_fps().expect("two frames in");
        assert!((fps - 10.0).abs() < 0.5, "measured {fps}");
        let p = r.plan(snapclone_av1policy::Tier::Standard);
        // seekable = ~1s GOP, so keyint should track the MEASURED rate.
        assert_eq!(p.keyint, 10, "the plan used a nominal rate, not the measured one");

        // And an empty recorder still produces a usable plan rather than
        // dividing by zero — the policy treats fps 0 as 30.
        let empty = Recorder::new(Limits::default());
        assert_eq!(empty.plan(snapclone_av1policy::Tier::Standard).keyint, 30);
    }
}
