//! Stateful V4L2 M2M decoder (Qualcomm Venus, and most vendor decoders).
//!
//! All the difficulty in a stateful decoder is sequencing, so that is what
//! this file is: a state machine over [`V4l2Api`], with no ioctls in it. The
//! sequences below follow the kernel's `dev-decoder` uAPI documentation.
//!
//! # The four sequences
//!
//! **Initialization.** `S_FMT(OUTPUT)` -> `REQBUFS(OUTPUT)` -> `STREAMON(OUTPUT)`
//! -> feed bitstream -> wait for `SOURCE_CHANGE` -> `G_FMT(CAPTURE)` ->
//! `REQBUFS(CAPTURE, min + headroom)` -> queue all -> `STREAMON(CAPTURE)`.
//! You cannot touch the CAPTURE queue before the event; the driver does not
//! know the frame size yet.
//!
//! **Dynamic resolution change.** A second `SOURCE_CHANGE` mid-stream. Drain
//! CAPTURE until a buffer arrives with `LAST`, then `STREAMOFF(CAPTURE)`,
//! `REQBUFS(CAPTURE, 0)`, and redo the tail of initialization. Critically the
//! OUTPUT queue **stays streaming** — tearing it down loses the bitstream
//! position and forces a seek. DASH adaptive streams trigger this at segment
//! boundaries, so it is a normal event, not an error path.
//!
//! **Drain (end of stream).** `DEC_CMD_STOP`, then dequeue CAPTURE until
//! `LAST`. `DEC_CMD_START` to resume.
//!
//! **Seek.** `STREAMOFF(OUTPUT)` + `STREAMOFF(CAPTURE)`, requeue all CAPTURE
//! buffers, `STREAMON` both. Allocations survive; buffer contents do not.

use crate::api::{ApiError, CaptureFormat, Event, Queue, V4l2Api};
use std::collections::HashSet;
use vidkit_core::codec::Codec;
use vidkit_core::frame::{ColorInfo, Frame, FrameData, PlaneLayout, RecycleToken};
use vidkit_core::traits::{self as core_traits, Packet};

/// Extra CAPTURE buffers beyond the driver's stated minimum.
///
/// The minimum covers reference frames only. Anything the presenter, the frame
/// queue or the compositor is holding comes out of headroom, and with zero
/// headroom the decoder blocks the moment one frame is on screen. Four is
/// enough for a triple-buffered compositor plus a frame in flight; going much
/// higher just costs memory, which matters on a phone.
pub const CAPTURE_HEADROOM: u32 = 4;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum State {
    /// Nothing configured yet.
    Init,
    /// OUTPUT streaming, bitstream going in, waiting for the first
    /// `SOURCE_CHANGE` so we can size the CAPTURE queue.
    AwaitingSourceChange,
    Running,
    /// Mid-stream `SOURCE_CHANGE` seen; draining CAPTURE until `LAST`.
    ResolutionChanging,
    /// `DEC_CMD_STOP` issued; draining CAPTURE until `LAST`.
    Draining,
    /// Drain complete.
    Ended,
}

pub struct StatefulDecoder<A: V4l2Api> {
    api: A,
    state: State,
    codec: Codec,
    coded_width: u32,
    coded_height: u32,
    format: Option<CaptureFormat>,
    /// Buffers currently held by downstream. Not requeued until recycled.
    outstanding: HashSet<u32>,
    capture_count: u32,
    /// Bumped on every CAPTURE reallocation. Recycle tokens carry it so a
    /// token minted before a resolution change cannot requeue a buffer index
    /// that now means something else — silent frame corruption otherwise.
    generation: u16,
    /// Set when a reconfiguration completes, so the caller is told once.
    pending_format_change: Option<(u32, u32)>,
}

fn codec_to_pixfmt(c: Codec) -> Option<u32> {
    use crate::api::pixfmt;
    Some(match c {
        Codec::H264 => pixfmt::H264,
        Codec::Hevc => pixfmt::HEVC,
        Codec::Vp8 => pixfmt::VP8,
        Codec::Vp9 => pixfmt::VP9,
        // Reachable only if a caps probe claimed stateful AV1, which no
        // real driver reports. Fail loudly rather than feeding a stateless
        // fourcc to a stateful decoder and getting EINVAL from S_FMT.
        Codec::Av1 => return None,
    })
}

fn fourcc_to_layout(fourcc: u32) -> Option<PlaneLayout> {
    use crate::api::pixfmt;
    Some(match fourcc {
        pixfmt::NV12 => PlaneLayout::Nv12,
        pixfmt::P010 => PlaneLayout::P010,
        pixfmt::YUV420M => PlaneLayout::I420,
        _ => return None,
    })
}

fn pack_token(generation: u16, index: u32) -> RecycleToken {
    RecycleToken(((generation as u32) << 16) | (index & 0xFFFF))
}

fn unpack_token(t: &RecycleToken) -> (u16, u32) {
    (((t.0 >> 16) & 0xFFFF) as u16, t.0 & 0xFFFF)
}

#[derive(Debug)]
pub enum DecodeError {
    Api(ApiError),
    UnsupportedCodec(Codec),
    UnsupportedPixelFormat(u32),
    /// Called in a state where the operation makes no sense. A bug here, not
    /// a driver problem.
    BadState(State),
}

impl From<ApiError> for DecodeError {
    fn from(e: ApiError) -> Self {
        DecodeError::Api(e)
    }
}

pub type DecResult<T> = Result<T, DecodeError>;

/// `pump` produces the core decoder output type directly; there is no reason
/// for this crate to have its own.
pub use vidkit_core::traits::DecodeOutput as Output;

impl<A: V4l2Api> StatefulDecoder<A> {
    pub fn new(api: A, codec: Codec, coded_width: u32, coded_height: u32) -> Self {
        Self {
            api,
            state: State::Init,
            codec,
            coded_width,
            coded_height,
            format: None,
            outstanding: HashSet::new(),
            capture_count: 0,
            generation: 0,
            pending_format_change: None,
        }
    }

    /// Escape hatch for tests and for the probe, which needs the raw device.
    pub fn device(&self) -> &A {
        &self.api
    }

    pub fn device_mut(&mut self) -> &mut A {
        &mut self.api
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn format(&self) -> Option<CaptureFormat> {
        self.format
    }

    /// Buffers downstream is holding. If this reaches `capture_count` the
    /// decoder is starved and the bug is a missing `recycle` call.
    pub fn outstanding(&self) -> usize {
        self.outstanding.len()
    }

    pub fn capture_count(&self) -> u32 {
        self.capture_count
    }

    fn start_output(&mut self) -> DecResult<()> {
        let fourcc = codec_to_pixfmt(self.codec).ok_or(DecodeError::UnsupportedCodec(self.codec))?;
        self.api
            .set_output_format(fourcc, self.coded_width, self.coded_height)?;
        self.api.request_buffers(Queue::Output, 8)?;
        self.api.stream_on(Queue::Output)?;
        self.state = State::AwaitingSourceChange;
        Ok(())
    }

    /// The tail of initialization, shared with the resolution-change path.
    fn configure_capture(&mut self) -> DecResult<()> {
        let fmt = self.api.capture_format()?;
        if fourcc_to_layout(fmt.fourcc).is_none() {
            return Err(DecodeError::UnsupportedPixelFormat(fmt.fourcc));
        }
        let min = self.api.min_capture_buffers()?;
        let want = min + CAPTURE_HEADROOM;
        let got = self.api.request_buffers(Queue::Capture, want)?;

        // Any token minted before this point is now stale.
        self.generation = self.generation.wrapping_add(1);
        self.outstanding.clear();
        self.capture_count = got;
        self.format = Some(fmt);

        for i in 0..got {
            self.api.queue_capture(i)?;
        }
        self.api.stream_on(Queue::Capture)?;
        self.pending_format_change = Some((fmt.width, fmt.height));
        Ok(())
    }

    /// Tear down and rebuild the CAPTURE queue. OUTPUT is left alone.
    fn reconfigure_capture(&mut self) -> DecResult<()> {
        self.api.stream_off(Queue::Capture)?;
        // REQBUFS(0) frees the old allocation. Buffers downstream still holds
        // are now dangling as far as the driver is concerned; the generation
        // bump in configure_capture is what stops us requeueing them.
        self.api.request_buffers(Queue::Capture, 0)?;
        self.configure_capture()
    }

    /// Offer a packet. `Ok(false)` means no free OUTPUT buffer: drain frames
    /// with `pump` and retry the *same* packet. Dropping it instead corrupts
    /// the bitstream.
    pub fn send_packet(&mut self, data: &[u8], pts: Option<std::time::Duration>) -> DecResult<bool> {
        if self.state == State::Init {
            self.start_output()?;
        }
        match self.state {
            State::AwaitingSourceChange | State::Running => {}
            // Feeding during a drain or reconfiguration would be queued behind
            // the flush and arrive out of order.
            other => return Err(DecodeError::BadState(other)),
        }
        // Reclaim consumed input buffers first, or we run out for no reason.
        while self.api.dequeue_output().unwrap_or(false) {}
        Ok(self.api.queue_output(data, pts)?)
    }

    fn pump_events(&mut self) -> DecResult<()> {
        while let Some(ev) = self.api.dequeue_event()? {
            match (ev, self.state) {
                (Event::SourceChange, State::AwaitingSourceChange) => {
                    self.configure_capture()?;
                    self.state = State::Running;
                }
                (Event::SourceChange, State::Running) => {
                    // Do NOT stream off OUTPUT here.
                    self.state = State::ResolutionChanging;
                }
                // A SOURCE_CHANGE arriving while already reconfiguring is
                // redundant; the drain in progress covers it.
                (Event::SourceChange, _) => {}
                // EOS is advisory. The LAST buffer flag is what we act on.
                (Event::Eos, _) => {}
            }
        }
        Ok(())
    }

    /// Drive the decoder. Call until it returns `Idle`.
    pub fn pump(&mut self) -> DecResult<Output> {
        self.pump_events()?;

        if let Some((w, h)) = self.pending_format_change.take() {
            return Ok(Output::FormatChanged {
                width: w,
                height: h,
            });
        }

        if self.state == State::Ended {
            return Ok(Output::EndOfStream);
        }
        if self.state == State::AwaitingSourceChange || self.state == State::Init {
            return Ok(Output::Idle);
        }

        let buf = match self.api.dequeue_capture()? {
            Some(b) => b,
            None => return Ok(Output::Idle),
        };

        let terminal = buf.last;
        let index = buf.index;

        // A buffer flagged LAST during a drain carries no picture, but it is
        // still ours and still has to go back.
        let usable = !buf.error && !(terminal && matches!(buf.data, FrameData::Cpu(ref p) if p.data.is_empty()));

        let out = if usable {
            let fmt = self.format.expect("running implies a known format");
            let layout = fourcc_to_layout(fmt.fourcc)
                .ok_or(DecodeError::UnsupportedPixelFormat(fmt.fourcc))?;
            self.outstanding.insert(index);
            Some(Frame {
                data: buf.data,
                width: fmt.width,
                height: fmt.height,
                layout,
                color: ColorInfo::default(),
                pts: buf.pts,
                duration: None,
                recycle: Some(pack_token(self.generation, index)),
                grain: None,
            })
        } else {
            // Errored or empty: straight back into the pool.
            self.api.queue_capture(index)?;
            None
        };

        if terminal {
            match self.state {
                State::ResolutionChanging => {
                    self.reconfigure_capture()?;
                    self.state = State::Running;
                }
                State::Draining => {
                    self.state = State::Ended;
                }
                _ => {}
            }
        }

        Ok(match out {
            Some(f) => Output::Frame(f),
            None if self.state == State::Ended => Output::EndOfStream,
            None => Output::Idle,
        })
    }

    /// Return a buffer to the pool. Tokens from a previous generation are
    /// silently discarded — the buffer they named no longer exists.
    pub fn recycle(&mut self, token: RecycleToken) -> DecResult<()> {
        let (gen, index) = unpack_token(&token);
        if gen != self.generation {
            return Ok(());
        }
        if self.outstanding.remove(&index) {
            self.api.queue_capture(index)?;
        }
        Ok(())
    }

    /// Begin end-of-stream drain.
    pub fn drain(&mut self) -> DecResult<()> {
        match self.state {
            State::Running => {
                self.api.decoder_cmd_stop()?;
                self.state = State::Draining;
                Ok(())
            }
            State::Ended | State::Draining => Ok(()),
            other => Err(DecodeError::BadState(other)),
        }
    }

    /// Seek / flush. Both queues stop and restart; allocations are kept.
    pub fn flush(&mut self) -> DecResult<()> {
        if self.state == State::Init {
            return Ok(());
        }
        self.api.stream_off(Queue::Output)?;
        self.api.stream_off(Queue::Capture)?;

        // STREAMOFF returns every buffer to us, including ones downstream was
        // holding. Requeue the lot and bump the generation so late recycles
        // from before the seek are ignored rather than double-queueing.
        self.generation = self.generation.wrapping_add(1);
        self.outstanding.clear();
        for i in 0..self.capture_count {
            self.api.queue_capture(i)?;
        }

        self.api.stream_on(Queue::Output)?;
        self.api.stream_on(Queue::Capture)?;
        if self.state == State::Ended || self.state == State::Draining {
            self.api.decoder_cmd_start()?;
        }
        self.state = State::Running;
        Ok(())
    }
}


impl From<DecodeError> for core_traits::Error {
    fn from(e: DecodeError) -> Self {
        match e {
            // Back-pressure and "nothing ready" are not errors at this layer;
            // they are handled inside the state machine and never escape.
            DecodeError::Api(ApiError::WouldBlock) | DecodeError::Api(ApiError::Drained) => {
                core_traits::Error::Backend("spurious would-block".into())
            }
            DecodeError::Api(e) => core_traits::Error::Backend(e.to_string()),
            DecodeError::UnsupportedCodec(c) => {
                core_traits::Error::Unsupported(format!("codec {c}"))
            }
            DecodeError::UnsupportedPixelFormat(f) => {
                core_traits::Error::Unsupported(format!("pixel format {f:#x}"))
            }
            DecodeError::BadState(s) => {
                core_traits::Error::Backend(format!("decoder in state {s:?}"))
            }
        }
    }
}

impl<A: V4l2Api + Send> core_traits::Decoder for StatefulDecoder<A> {
    fn backend_name(&self) -> &str {
        "v4l2-stateful"
    }

    fn send_packet(&mut self, pkt: &Packet) -> core_traits::Result<bool> {
        Ok(StatefulDecoder::send_packet(self, &pkt.data, pkt.pts)?)
    }

    fn receive_frame(&mut self) -> core_traits::Result<Output> {
        Ok(self.pump()?)
    }

    fn recycle(&mut self, token: RecycleToken) -> core_traits::Result<()> {
        Ok(StatefulDecoder::recycle(self, token)?)
    }

    fn flush(&mut self) -> core_traits::Result<()> {
        Ok(StatefulDecoder::flush(self)?)
    }
}
