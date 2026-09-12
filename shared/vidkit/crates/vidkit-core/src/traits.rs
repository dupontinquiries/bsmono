//! The seams.
//!
//! Four of them, chosen because each isolates something that genuinely varies:
//!
//! - `MediaSource` varies by *where bytes come from* (yt-dlp URL, local file,
//!   DASH segment fetcher). Swapping it must not touch decode.
//! - `Decoder` varies by *hardware* (V4L2 stateless, V4L2 stateful, VAAPI,
//!   Vulkan Video, dav1d). This is the one that actually differs per target
//!   device, and the whole reason for the caps model.
//! - `Presenter` varies by *GPU stack and toolkit* (Vulkan+egui, Vulkan+GTK4
//!   `GtkGLArea`/dmabuf paintable). Your eguikit shell plugs in here.
//! - `Clock` varies by *what is master* (audio clock normally, system clock
//!   when muted or audio-less).
//!
//! Note there is deliberately **no** `Player` trait. There is one player; it
//! is a concrete struct that owns these four. A trait there would be
//! speculative generality.

use crate::caps::DeviceCaps;
use crate::codec::VideoProfile;
use crate::frame::Frame;
use std::time::Duration;

#[derive(Debug)]
pub enum Error {
    /// The URL expired or the CDN rejected us. Callers should re-resolve via
    /// the source rather than tearing down playback — this is routine on
    /// YouTube, not exceptional.
    NeedsReresolve,
    Unsupported(String),
    Io(std::io::Error),
    Backend(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NeedsReresolve => write!(f, "media URL expired; re-resolve"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Backend(s) => write!(f, "backend: {s}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// A compressed access unit plus its timing.
pub struct Packet {
    pub data: Vec<u8>,
    pub pts: Option<Duration>,
    pub dts: Option<Duration>,
    pub is_keyframe: bool,
}

/// Where compressed bytes come from.
///
/// Object safe on purpose: the player holds a `Box<dyn MediaSource>` so that a
/// local file and a yt-dlp stream are interchangeable at runtime, chosen from
/// what the user typed.
pub trait MediaSource: Send {
    fn video_profile(&self) -> Option<VideoProfile>;
    fn dimensions(&self) -> (u32, u32);
    fn nominal_fps(&self) -> u32;
    fn duration(&self) -> Option<Duration>;
    fn seekable(&self) -> bool;

    fn next_packet(&mut self) -> Result<Option<Packet>>;
    fn seek(&mut self, to: Duration) -> Result<()>;
}

/// What a decoder produced this turn.
///
/// This is wider than `Option<Frame>` because a successful *reconfiguration*
/// is neither a frame nor an error, and squeezing it into `Err` loses that.
/// Mid-stream resolution changes are routine on DASH — they happen at segment
/// boundaries — so the type that describes normal operation has to include
/// them.
#[derive(Debug)]
pub enum DecodeOutput {
    Frame(Frame),
    /// The decoder reconfigured. Flush the frame queue and resize presenter
    /// resources, then keep pumping.
    FormatChanged { width: u32, height: u32 },
    /// Drain complete. No further frames, ever, without a `flush`.
    EndOfStream,
    /// Nothing ready right now. Feed more packets or wait.
    Idle,
}

/// Turns packets into frames. One instance per stream.
pub trait Decoder: Send {
    fn backend_name(&self) -> &str;

    /// Offer one packet.
    ///
    /// `Ok(false)` means the decoder has no free input buffer: drain with
    /// `receive_frame` and retry the **same** packet. Hardware decoders all
    /// behave this way (V4L2 returns no free OUTPUT buffer, VAAPI blocks on
    /// surfaces), so back-pressure belongs in the signature rather than being
    /// hidden behind an internal queue that can only grow.
    fn send_packet(&mut self, pkt: &Packet) -> Result<bool>;

    /// Drain. Call until it returns `Idle` after every `send_packet`.
    fn receive_frame(&mut self) -> Result<DecodeOutput>;

    /// Return a frame's buffer to the decoder's pool. Must be called for every
    /// frame whose `recycle` field was `Some`, or the decoder starves.
    fn recycle(&mut self, token: crate::frame::RecycleToken) -> Result<()>;

    /// Drop everything in flight. Must be cheap and must not error on a
    /// decoder that has never been fed.
    fn flush(&mut self) -> Result<()>;
}

/// Builds decoders, given what the machine can do.
///
/// Split from `Decoder` so the selection policy is testable without any
/// hardware present — construct a `DeviceCaps` by hand and assert on which
/// backend name comes back.
pub trait DecoderFactory {
    fn caps(&self) -> &DeviceCaps;
    fn open(&self, profile: &VideoProfile, width: u32, height: u32, fps: u32)
        -> Result<Box<dyn Decoder>>;
}

/// Gets frames onto the screen. Implemented once per GPU/toolkit combination.
pub trait Presenter {
    /// Hand over a frame. The presenter owns it from here, including closing
    /// any DMA-BUF fds.
    ///
    /// Returns the recycle token of a frame it has *finished* with — normally
    /// the previously submitted one, since a presenter must keep the current
    /// frame alive until the next replaces it. The caller returns that token
    /// to the decoder. This is the honest shape: a double-buffered presenter
    /// holds exactly one frame at a time, and pretending `submit` releases the
    /// frame immediately would let the compositor sample freed memory.
    fn submit(&mut self, frame: Frame) -> Result<Option<crate::frame::RecycleToken>>;

    /// Give up every frame currently held. Called on seek, on decoder
    /// reconfiguration, and at teardown. After this the presenter must not
    /// reference any previously submitted frame.
    fn release_all(&mut self) -> Vec<crate::frame::RecycleToken>;

    /// The deadline the UI should wake up at to show the next queued frame.
    /// This is what keeps the UI smooth: the shell repaints *on this*, not on
    /// a free-running 60 Hz loop. On a phone that is the difference between
    /// idling at 24 wakeups/sec and 90.
    fn next_deadline(&self) -> Option<std::time::Instant>;

    /// Frames dropped since last call, for the stats overlay.
    fn take_drop_count(&mut self) -> u32;
}

/// Master timeline. Audio-mastered when there is audio, monotonic otherwise.
pub trait Clock: Send {
    fn now(&self) -> Duration;
    fn set_rate(&mut self, rate: f64);
    fn pause(&mut self);
    fn resume(&mut self);
    fn is_paused(&self) -> bool;
}
