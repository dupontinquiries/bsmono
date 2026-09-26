//! Headless playback core. No GPU API, no toolkit, no process spawning.
//!
//! Everything here is pure data + traits so that the interesting policy —
//! which stream to fetch, which decoder to open, when to show a frame — can be
//! unit tested with no hardware and no network.

pub mod audio_clock;
pub mod caps;
pub mod clock;
pub mod codec;
pub mod av1_grain;
pub mod convert_cpu;
pub mod damage;
pub mod frame;
pub mod io;
pub mod player;
pub mod queue;
pub mod quality;
pub mod traits;
pub mod wakelock;

pub use audio_clock::AudioClock;
pub use caps::{Backend, Cost, DecodeCap, Delivery, DeviceCaps, PlaybackBudget};
pub use convert_cpu::{
    frame_to_rgba8, frame_to_rgba8_into, planes_to_rgba8, planes_to_rgba8_into, ConvertError,
};
pub use codec::{parse_codec_string, Chroma, Codec, Transfer, VideoProfile};
pub use io::{read_exact_at, ByteSource, MemorySource};
pub use frame::{ColorInfo, Frame, FrameData, Matrix, PlaneLayout, Range, RecycleToken};
pub use queue::{FrameQueue, Poll, Pressure};
pub use quality::{FpsCap, Quality};
pub use wakelock::{Activity, Inhibitor, Lock, WakeLock};
pub use traits::{
    Clock, DecodeOutput, Decoder, DecoderFactory, Error, MediaSource, Packet, Presenter, Result,
};
