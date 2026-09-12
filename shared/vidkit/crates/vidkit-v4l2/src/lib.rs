//! Stateful V4L2 decoding, with the state machine separated from the ioctls so
//! the sequencing can be tested without hardware.

pub mod sysprobe;
pub mod api;
pub mod probe;
#[cfg(target_os = "linux")]
pub mod device;
pub mod raw;
pub mod stateful;

pub use api::{ApiError, CaptureFormat, DequeuedCapture, Event, Queue, V4l2Api};
pub use stateful::{DecodeError, Output, State, StatefulDecoder, CAPTURE_HEADROOM};

#[cfg(test)]
mod mock {
    use crate::api::*;
    use std::collections::{HashSet, VecDeque};
    use std::time::Duration;
    use vidkit_core::frame::{CpuPlanes, FrameData};

    /// A scripted decoder device. Records every ioctl in order so tests can
    /// assert on sequencing, which is the entire point.
    pub struct MockDevice {
        pub log: Vec<String>,
        pub events: VecDeque<Event>,
        /// Frames the driver will hand back, in order.
        pub pending: VecDeque<DequeuedCapture>,
        pub free_capture: HashSet<u32>,
        pub capture_allocated: u32,
        pub output_free: u32,
        /// Buffers the driver is still chewing on. `dequeue_output` can only
        /// return these — modelling it as "always reclaimable" hides
        /// back-pressure entirely, which is the whole thing under test.
        pub output_queued: u32,
        pub fmt: CaptureFormat,
        pub min_buffers: u32,
        pub output_streaming: bool,
        pub capture_streaming: bool,
    }

    impl MockDevice {
        pub fn new(w: u32, h: u32) -> Self {
            Self {
                log: Vec::new(),
                events: VecDeque::new(),
                pending: VecDeque::new(),
                free_capture: HashSet::new(),
                capture_allocated: 0,
                output_free: 8,
                output_queued: 0,
                fmt: CaptureFormat {
                    width: w,
                    height: h,
                    fourcc: pixfmt::NV12,
                    modifier: 0,
                },
                min_buffers: 8,
                output_streaming: false,
                capture_streaming: false,
            }
        }

        fn rec(&mut self, s: impl Into<String>) {
            self.log.push(s.into());
        }

        pub fn ops(&self) -> Vec<&str> {
            self.log.iter().map(|s| s.as_str()).collect()
        }

        /// Position of the first op starting with `prefix`.
        pub fn index_of(&self, prefix: &str) -> Option<usize> {
            self.log.iter().position(|s| s.starts_with(prefix))
        }

        pub fn push_frame(&mut self, index: u32, pts_ms: u64) {
            self.pending.push_back(DequeuedCapture {
                index,
                data: FrameData::Cpu(CpuPlanes {
                    data: vec![vec![7u8; 64]],
                    strides: vec![8],
                }),
                pts: Duration::from_millis(pts_ms),
                last: false,
                error: false,
            });
        }

        /// The empty buffer with `LAST` set that terminates a drain.
        pub fn push_last(&mut self, index: u32) {
            self.pending.push_back(DequeuedCapture {
                index,
                data: FrameData::Cpu(CpuPlanes {
                    data: vec![],
                    strides: vec![],
                }),
                pts: Duration::ZERO,
                last: true,
                error: false,
            });
        }

        pub fn push_errored(&mut self, index: u32) {
            self.pending.push_back(DequeuedCapture {
                index,
                data: FrameData::Cpu(CpuPlanes {
                    data: vec![vec![0u8; 64]],
                    strides: vec![8],
                }),
                pts: Duration::ZERO,
                last: false,
                error: true,
            });
        }
    }

    impl V4l2Api for MockDevice {
        fn set_output_format(&mut self, fourcc: u32, w: u32, h: u32) -> ApiResult<()> {
            self.rec(format!("s_fmt_output({fourcc:#x},{w}x{h})"));
            Ok(())
        }

        fn request_buffers(&mut self, queue: Queue, count: u32) -> ApiResult<u32> {
            self.rec(format!("reqbufs({queue:?},{count})"));
            if queue == Queue::Capture {
                self.capture_allocated = count;
                self.free_capture.clear();
                if count == 0 {
                    self.pending.clear();
                }
            }
            Ok(count)
        }

        fn stream_on(&mut self, queue: Queue) -> ApiResult<()> {
            self.rec(format!("streamon({queue:?})"));
            match queue {
                Queue::Output => self.output_streaming = true,
                Queue::Capture => self.capture_streaming = true,
            }
            Ok(())
        }

        fn stream_off(&mut self, queue: Queue) -> ApiResult<()> {
            self.rec(format!("streamoff({queue:?})"));
            match queue {
                Queue::Output => self.output_streaming = false,
                Queue::Capture => {
                    self.capture_streaming = false;
                    self.pending.clear();
                    // Per the uAPI: STREAMOFF returns *every* buffer to
                    // userspace, including ones the driver was holding and
                    // ones we had dequeued. So none are queued afterwards.
                    self.free_capture.clear();
                }
            }
            Ok(())
        }

        fn capture_format(&mut self) -> ApiResult<CaptureFormat> {
            self.rec("g_fmt_capture");
            Ok(self.fmt)
        }

        fn min_capture_buffers(&mut self) -> ApiResult<u32> {
            self.rec("min_capture_buffers");
            Ok(self.min_buffers)
        }

        fn queue_output(&mut self, data: &[u8], _pts: Option<Duration>) -> ApiResult<bool> {
            if !self.output_streaming {
                return Err(ApiError::Ioctl("qbuf_output", 22));
            }
            if self.output_free == 0 {
                self.rec("qbuf_output(FULL)");
                return Ok(false);
            }
            self.output_free -= 1;
            self.output_queued += 1;
            self.rec(format!("qbuf_output({} bytes)", data.len()));
            Ok(true)
        }

        fn dequeue_output(&mut self) -> ApiResult<bool> {
            if self.output_queued > 0 {
                self.output_queued -= 1;
                self.output_free += 1;
                self.rec("dqbuf_output");
                return Ok(true);
            }
            Ok(false)
        }

        fn queue_capture(&mut self, index: u32) -> ApiResult<()> {
            if !self.free_capture.insert(index) {
                // Double-requeue: exactly the bug the generation counter exists
                // to prevent. Make it loud in tests.
                return Err(ApiError::Ioctl("qbuf_capture(double)", 22));
            }
            self.rec(format!("qbuf_capture({index})"));
            Ok(())
        }

        fn dequeue_capture(&mut self) -> ApiResult<Option<DequeuedCapture>> {
            if !self.capture_streaming {
                return Ok(None);
            }
            match self.pending.pop_front() {
                Some(b) => {
                    self.free_capture.remove(&b.index);
                    self.rec(format!("dqbuf_capture({})", b.index));
                    Ok(Some(b))
                }
                None => Ok(None),
            }
        }

        fn dequeue_event(&mut self) -> ApiResult<Option<Event>> {
            Ok(self.events.pop_front())
        }

        fn decoder_cmd_stop(&mut self) -> ApiResult<()> {
            self.rec("dec_cmd_stop");
            Ok(())
        }

        fn decoder_cmd_start(&mut self) -> ApiResult<()> {
            self.rec("dec_cmd_start");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockDevice;
    use crate::api::Event;
    use crate::stateful::{Output, State, StatefulDecoder, CAPTURE_HEADROOM};
    use std::time::Duration;
    use vidkit_core::codec::Codec;

    fn dec(w: u32, h: u32) -> StatefulDecoder<MockDevice> {
        StatefulDecoder::new(MockDevice::new(w, h), Codec::Vp9, w, h)
    }

    /// Bring a decoder up to `Running` the way a real stream would.
    fn bring_up(d: &mut StatefulDecoder<MockDevice>) {
        d.send_packet(b"headers", Some(Duration::ZERO)).unwrap();
        d.device_mut().events.push_back(Event::SourceChange);
        // First pump consumes the event and reports the format.
        match d.pump().unwrap() {
            Output::FormatChanged { .. } => {}
            other => panic!("expected FormatChanged, got {other:?}"),
        }
    }

    #[test]
    fn capture_queue_is_untouched_until_the_source_change_event() {
        let mut d = dec(1920, 1080);
        d.send_packet(b"headers", None).unwrap();
        assert_eq!(d.state(), State::AwaitingSourceChange);

        let ops = d.device().log.clone();
        assert!(
            !ops.iter().any(|o| o.contains("Capture")),
            "touched CAPTURE before SOURCE_CHANGE: {ops:?}"
        );
        // And pumping before the event must not invent frames.
        assert!(matches!(d.pump().unwrap(), Output::Idle));
    }

    #[test]
    fn initialization_sequence_is_in_the_documented_order() {
        let mut d = dec(1920, 1080);
        bring_up(&mut d);
        assert_eq!(d.state(), State::Running);

        let dev = d.device();
        let i_sfmt = dev.index_of("s_fmt_output").unwrap();
        let i_on_out = dev.index_of("streamon(Output)").unwrap();
        let i_gfmt = dev.index_of("g_fmt_capture").unwrap();
        let i_reqcap = dev.index_of("reqbufs(Capture").unwrap();
        let i_on_cap = dev.index_of("streamon(Capture)").unwrap();
        let i_first_qcap = dev.index_of("qbuf_capture(").unwrap();

        assert!(i_sfmt < i_on_out, "S_FMT must precede STREAMON(OUTPUT)");
        assert!(i_on_out < i_gfmt, "OUTPUT must stream before G_FMT(CAPTURE)");
        assert!(i_gfmt < i_reqcap, "G_FMT must precede REQBUFS(CAPTURE)");
        assert!(
            i_reqcap < i_first_qcap && i_first_qcap < i_on_cap,
            "buffers must be queued before STREAMON(CAPTURE): {:?}",
            dev.ops()
        );
    }

    #[test]
    fn capture_allocation_adds_headroom_over_the_driver_minimum() {
        let mut d = dec(1920, 1080);
        d.device_mut().min_buffers = 11;
        bring_up(&mut d);
        assert_eq!(d.capture_count(), 11 + CAPTURE_HEADROOM);
    }

    #[test]
    fn frames_carry_a_recycle_token_and_are_counted_as_outstanding() {
        let mut d = dec(1920, 1080);
        bring_up(&mut d);
        d.device_mut().push_frame(3, 100);

        let f = match d.pump().unwrap() {
            Output::Frame(f) => f,
            other => panic!("expected Frame, got {other:?}"),
        };
        assert_eq!(f.pts, Duration::from_millis(100));
        assert_eq!(f.width, 1920);
        assert_eq!(d.outstanding(), 1);

        d.recycle(f.recycle.unwrap()).unwrap();
        assert_eq!(d.outstanding(), 0);
        assert!(d.device().free_capture.contains(&3));
    }

    #[test]
    fn errored_buffers_are_requeued_and_not_handed_out() {
        let mut d = dec(1920, 1080);
        bring_up(&mut d);
        d.device_mut().push_errored(2);
        assert!(matches!(d.pump().unwrap(), Output::Idle));
        assert_eq!(d.outstanding(), 0);
        assert!(d.device().free_capture.contains(&2));
    }

    // ---- dynamic resolution change -------------------------------------

    #[test]
    fn resolution_change_drains_capture_and_leaves_output_streaming() {
        let mut d = dec(1280, 720);
        bring_up(&mut d);

        // Mid-stream event, then the drain terminator.
        d.device_mut().events.push_back(Event::SourceChange);
        d.device_mut().push_last(0);
        d.device_mut().fmt.width = 1920;
        d.device_mut().fmt.height = 1080;

        let before = d.device().log.len();
        let out = d.pump().unwrap();
        assert!(matches!(out, Output::Idle | Output::EndOfStream) || matches!(out, Output::Frame(_)));
        assert_eq!(d.state(), State::Running, "must return to Running");

        let tail: Vec<String> = d.device().log[before..].to_vec();
        assert!(
            !tail.iter().any(|o| o == "streamoff(Output)"),
            "OUTPUT must keep streaming across a resolution change: {tail:?}"
        );
        assert!(
            tail.iter().any(|o| o == "streamoff(Capture)"),
            "CAPTURE must be torn down: {tail:?}"
        );
        assert!(
            tail.iter().any(|o| o == "reqbufs(Capture,0)"),
            "CAPTURE allocation must be freed: {tail:?}"
        );
        assert!(d.device().output_streaming);
    }

    #[test]
    fn resolution_change_reports_the_new_size_once() {
        let mut d = dec(1280, 720);
        bring_up(&mut d);
        d.device_mut().events.push_back(Event::SourceChange);
        d.device_mut().push_last(0);
        d.device_mut().fmt.width = 1920;
        d.device_mut().fmt.height = 1080;

        let _ = d.pump().unwrap();
        match d.pump().unwrap() {
            Output::FormatChanged { width, height } => {
                assert_eq!((width, height), (1920, 1080));
            }
            other => panic!("expected FormatChanged, got {other:?}"),
        }
        // Reported exactly once.
        assert!(matches!(d.pump().unwrap(), Output::Idle));
    }

    /// The bug the generation counter exists for: a frame held across a
    /// resolution change is recycled late, and its index now refers to a
    /// different buffer in a freshly allocated pool.
    #[test]
    fn a_stale_recycle_token_is_discarded_not_requeued() {
        let mut d = dec(1280, 720);
        bring_up(&mut d);
        d.device_mut().push_frame(1, 0);
        let stale = match d.pump().unwrap() {
            Output::Frame(f) => f.recycle.unwrap(),
            other => panic!("expected Frame, got {other:?}"),
        };

        d.device_mut().events.push_back(Event::SourceChange);
        d.device_mut().push_last(0);
        let _ = d.pump().unwrap();
        assert_eq!(d.state(), State::Running);

        // Hand out a NEW frame that reuses index 1 from the fresh pool. This is
        // the case the generation counter actually protects: clearing
        // `outstanding` on reconfiguration is not sufficient, because a live
        // buffer can occupy the same index the stale token names.
        // Consume the FormatChanged report the reconfiguration queued up.
        assert!(matches!(d.pump().unwrap(), Output::FormatChanged { .. }));

        d.device_mut().push_frame(1, 100);
        let live = match d.pump().unwrap() {
            Output::Frame(f) => f.recycle.unwrap(),
            other => panic!("expected Frame, got {other:?}"),
        };
        assert_eq!(d.outstanding(), 1);

        let queued_before = d.device().log.len();
        // Must be a no-op. Without the generation guard this requeues buffer 1
        // while downstream is still reading it — the driver then decodes into
        // memory being sampled by the compositor.
        d.recycle(stale).unwrap();
        assert_eq!(
            d.device().log.len(),
            queued_before,
            "stale token must not touch the driver"
        );
        assert_eq!(
            d.outstanding(),
            1,
            "stale token must not release the live buffer that reused its index"
        );

        // The live token still works.
        d.recycle(live).unwrap();
        assert_eq!(d.outstanding(), 0);
    }

    // ---- drain and flush ------------------------------------------------

    #[test]
    fn drain_ends_the_stream_on_the_last_flag() {
        let mut d = dec(1920, 1080);
        bring_up(&mut d);
        d.device_mut().push_frame(0, 0);
        d.device_mut().push_frame(1, 33);
        d.device_mut().push_last(2);

        d.drain().unwrap();
        assert_eq!(d.state(), State::Draining);
        assert!(d.device().log.iter().any(|o| o == "dec_cmd_stop"));

        let mut frames = 0;
        loop {
            match d.pump().unwrap() {
                Output::Frame(f) => {
                    frames += 1;
                    d.recycle(f.recycle.unwrap()).unwrap();
                }
                Output::EndOfStream => break,
                Output::Idle => panic!("went idle before EOS"),
                Output::FormatChanged { .. } => {}
            }
        }
        assert_eq!(frames, 2);
        assert_eq!(d.state(), State::Ended);
    }

    #[test]
    fn feeding_during_a_drain_is_a_state_error_not_silent_reordering() {
        let mut d = dec(1920, 1080);
        bring_up(&mut d);
        d.drain().unwrap();
        assert!(d.send_packet(b"more", None).is_err());
    }

    #[test]
    fn flush_restarts_both_queues_and_requeues_everything() {
        let mut d = dec(1920, 1080);
        bring_up(&mut d);
        d.device_mut().push_frame(0, 0);
        let held = match d.pump().unwrap() {
            Output::Frame(f) => f.recycle.unwrap(),
            other => panic!("expected Frame, got {other:?}"),
        };
        assert_eq!(d.outstanding(), 1);

        let n = d.capture_count();
        d.flush().unwrap();

        assert_eq!(d.state(), State::Running);
        assert_eq!(d.outstanding(), 0, "STREAMOFF returns held buffers to us");
        assert_eq!(
            d.device().free_capture.len() as u32,
            n,
            "every capture buffer must be requeued after a flush"
        );
        // The pre-seek token is now stale and must be inert.
        d.recycle(held).unwrap();
    }

    #[test]
    fn flush_after_end_of_stream_restarts_the_decoder() {
        let mut d = dec(1920, 1080);
        bring_up(&mut d);
        d.device_mut().push_last(0);
        d.drain().unwrap();
        let _ = d.pump().unwrap();
        assert_eq!(d.state(), State::Ended);

        d.flush().unwrap();
        assert_eq!(d.state(), State::Running);
        assert!(
            d.device().log.iter().any(|o| o == "dec_cmd_start"),
            "DEC_CMD_START is required to resume after a completed drain"
        );
    }

    #[test]
    fn flush_before_initialization_is_a_no_op() {
        let mut d = dec(1920, 1080);
        d.flush().unwrap();
        assert!(d.device().log.is_empty());
    }

    // ---- back-pressure ---------------------------------------------------

    #[test]
    fn a_full_output_queue_returns_false_rather_than_dropping_the_packet() {
        let mut d = dec(1920, 1080);
        bring_up(&mut d);
        // Every OUTPUT buffer is in the driver's hands and none has finished:
        // nothing to reclaim, nothing free.
        d.device_mut().output_free = 0;
        d.device_mut().output_queued = 0;
        assert_eq!(d.send_packet(b"data", None).unwrap(), false);

        // ...and once one comes back, the same packet goes through.
        d.device_mut().output_queued = 1;
        assert_eq!(d.send_packet(b"data", None).unwrap(), true);
    }

    /// No stateful decoder does AV1, so the state machine must refuse rather
    /// than send the stateless fourcc to S_FMT and get a confusing EINVAL.
    #[test]
    fn av1_is_refused_by_the_stateful_path() {
        let mut d = StatefulDecoder::new(MockDevice::new(1920, 1080), Codec::Av1, 1920, 1080);
        assert!(d.send_packet(b"h", None).is_err());
        assert!(d.device().log.is_empty(), "must not touch the driver at all");
    }

    #[test]
    fn unsupported_codec_fails_at_setup() {
        let mut d = StatefulDecoder::new(MockDevice::new(640, 480), Codec::Vp8, 640, 480);
        // VP8 is mapped, so use an unmapped capture format instead to prove
        // the pixel-format check fires.
        d.device_mut().fmt.fourcc = 0xDEAD_BEEF;
        d.send_packet(b"h", None).unwrap();
        d.device_mut().events.push_back(Event::SourceChange);
        assert!(d.pump().is_err());
    }
}
