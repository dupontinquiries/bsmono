//! A local mirror of `audio_kit::wav::Error`.
//!
//! `ImportError` is `PartialEq` so a caller can match on it in a test, and
//! wrapping the audio-kit error directly would make this crate's public API
//! depend on that crate's error type being comparable — a constraint on a
//! shared crate imposed by one consumer. Mirroring is three lines and keeps
//! the dependency one-directional.

/// Why the file did not parse.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WavError {
    NotRiff,
    NoFormat,
    NoData,
    UnsupportedFormat(u16),
    UnsupportedDepth(u16),
    Truncated,
}

impl From<audio_kit::wav::Error> for WavError {
    fn from(e: audio_kit::wav::Error) -> WavError {
        use audio_kit::wav::Error as E;
        match e {
            E::NotRiff => WavError::NotRiff,
            E::NoFormat => WavError::NoFormat,
            E::NoData => WavError::NoData,
            E::UnsupportedFormat(f) => WavError::UnsupportedFormat(f),
            E::UnsupportedDepth(d) => WavError::UnsupportedDepth(d),
            E::Truncated => WavError::Truncated,
        }
    }
}
