//! studio-media — audio decode, on purpose kept out of `studio-core` and
//! `studio-audio`. Both of those crates' own docs already promise to stay
//! codec-free (`studio-audio`: "keeping the core codec-free"); this crate is
//! what backs that promise, the same way `studio-io` is "the only crate
//! allowed to touch a sound card." `decode_file` is the whole surface: hand
//! it a path, get back a [`studio_audio::SourceAudio`] — the exact type
//! `mixdown`, `Peaks`, and everything else downstream already knows how to
//! use, so a decoded file and a synthesized rack bounce
//! (`studio_live::render_pattern`) are indistinguishable once they're in
//! memory. Nothing above this crate needs to know Symphonia exists.
//!
//! ## What's here and what isn't
//!
//! [Symphonia](https://github.com/pdeljanov/Symphonia) does the actual
//! bitstream work — demuxing, codec decode, sample-format conversion. This
//! crate is the plumbing around it: probe a file, find its first audio
//! track, decode every packet, and land the result as planar `f32`,
//! resampled to nothing (the source's own rate is preserved; resampling to
//! a project's rate, if that's ever needed, is a mixdown-time decision, not
//! a decode-time one — the same reason `SourceAudio` itself carries its own
//! `sample_rate` rather than assuming one).
//!
//! Video decode — the README's "`ffmpeg` via CLI" half of Media I/O — is
//! deliberately not here. Shelling out to `ffmpeg` needs a binary on the
//! `PATH` and a frame format decision (`studio-core`'s `has_video` flag on
//! `Source` already anticipates it); that's real, separate work with a
//! different failure mode (a missing binary vs. an unsupported codec
//! feature) and belongs in its own crate when it's built, not bolted onto
//! this one because they're both nominally "Media I/O."
//!
//! ## A decode error is not a missing-plugin error
//!
//! Every other "unknown thing in a project" case in this codebase — an
//! effect kind the build doesn't know, a `SynthKind::Beatspace` with no
//! model loaded — degrades to silence or a skip so the *project* still
//! opens. A source that can't be decoded is different: there is no
//! reasonable silent fallback for "the actual audio is gone," so
//! `decode_file` returns a real `Err` for the caller to surface, rather than
//! quietly handing back silence that would then get mixed, peaked, and
//! played as if it were the take. What IS treated leniently is a single bad
//! *packet* inside an otherwise-decodable stream (a corrupt frame partway
//! through a long file) — skipped, matching Symphonia's own recommended
//! handling, so one damaged packet doesn't cost the rest of an otherwise-fine
//! recording.

use std::fmt;
use std::path::Path;

use studio_audio::SourceAudio;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

#[derive(Debug)]
pub enum DecodeError {
    Io(std::io::Error),
    /// Symphonia's probe couldn't identify a container/codec it knows.
    UnsupportedFormat,
    /// The container was readable but held no audio track at all (or only
    /// video/subtitle tracks) — the video-decode gap this crate deliberately
    /// leaves for `ffmpeg`, surfaced honestly rather than as silence.
    NoAudioTrack,
    NoDecoder,
    Decode(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Io(e) => write!(f, "could not read the file: {e}"),
            DecodeError::UnsupportedFormat => write!(f, "unrecognised audio format or container"),
            DecodeError::NoAudioTrack => write!(f, "no decodable audio track in this file"),
            DecodeError::NoDecoder => write!(f, "no decoder available for this file's codec"),
            DecodeError::Decode(msg) => write!(f, "decode error: {msg}"),
        }
    }
}
impl std::error::Error for DecodeError {}
impl From<std::io::Error> for DecodeError {
    fn from(e: std::io::Error) -> Self { DecodeError::Io(e) }
}

/// Decode an entire audio file into memory as planar `f32`, at the file's
/// own sample rate. Intended for the same use `studio_live::Recorder::
/// to_audio` and a pattern bounce already serve: producing a
/// `SourceAudio` a caller registers under a `SourceId` and hands to
/// `studio_audio::mixdown` — decode-once-into-memory, not a streaming
/// decoder, which matches how every other source (a recorded take, a
/// bounced pattern) already reaches the mix.
pub fn decode_file(path: &Path) -> Result<SourceAudio, DecodeError> {
    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|_| DecodeError::UnsupportedFormat)?;

    decode_format(probed.format)
}

/// The decode loop proper, taking an already-probed [`FormatReader`] rather
/// than a path — the seam that lets `decode_file`'s file-opening and
/// format-detection be tested separately from packet decoding, and the
/// thing the tests below actually call (through a `Cursor` in memory, no
/// temp file needed).
fn decode_format(mut format: Box<dyn FormatReader>) -> Result<SourceAudio, DecodeError> {
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or(DecodeError::NoAudioTrack)?;
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| DecodeError::NoDecoder)?;

    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(44_100);
    let mut channels: Vec<Vec<f32>> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // Symphonia's own documented way to say "no more packets" —
            // not every backend returns a distinct EOF variant, so an
            // IO error whose kind is UnexpectedEof is the actual signal.
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            // A track change mid-stream (rare; some containers allow it).
            // Nothing downstream of this crate has a notion of a source
            // changing shape partway through, so stop with what was
            // decoded so far rather than trying to represent it.
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio_buf) => append_planar(&audio_buf, &mut channels, &mut sample_rate),
            // A single corrupt packet inside an otherwise-good stream —
            // Symphonia's own examples treat DecodeError as skippable and
            // continue; one damaged frame should not cost the rest of an
            // otherwise-fine recording. IoError/ResetRequired here (as
            // opposed to from next_packet above) are genuinely fatal.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        }
    }

    if channels.is_empty() || channels[0].is_empty() {
        return Err(DecodeError::NoAudioTrack);
    }
    Ok(SourceAudio { sample_rate, channels })
}

/// Append one decoded buffer's samples onto `channels` (allocating it to the
/// right channel count on the first call), converting whatever native
/// sample format Symphonia decoded to `f32` via its own conversion — every
/// `AudioBufferRef` variant implements this uniformly, so this function
/// doesn't need to match on the sample format itself.
fn append_planar(buf: &AudioBufferRef, channels: &mut Vec<Vec<f32>>, sample_rate: &mut u32) {
    let spec = *buf.spec();
    *sample_rate = spec.rate;
    let n_ch = spec.channels.count().max(1);
    if channels.is_empty() {
        channels.resize(n_ch, Vec::new());
    }
    let frames = buf.frames();
    for ch in 0..n_ch.min(channels.len()) {
        let plane = plane_as_f32(buf, ch, frames);
        channels[ch].extend(plane);
    }
}

/// Read one channel of an `AudioBufferRef` as `f32`, regardless of the
/// buffer's native sample type. `AudioBufferRef` is an enum over every
/// sample type Symphonia can decode to (`U8`..`F64`); matching it here once
/// is simpler and more obviously correct than threading a generic sample
/// type through this whole module for a function called once per packet.
fn plane_as_f32(buf: &AudioBufferRef, ch: usize, frames: usize) -> Vec<f32> {
    use symphonia::core::conv::IntoSample;
    macro_rules! plane {
        ($variant:path) => {
            if let $variant(b) = buf {
                let src = b.chan(ch);
                return src[..frames.min(src.len())].iter().map(|&s| IntoSample::<f32>::into_sample(s)).collect();
            }
        };
    }
    plane!(AudioBufferRef::U8);
    plane!(AudioBufferRef::U16);
    plane!(AudioBufferRef::U24);
    plane!(AudioBufferRef::U32);
    plane!(AudioBufferRef::S8);
    plane!(AudioBufferRef::S16);
    plane!(AudioBufferRef::S24);
    plane!(AudioBufferRef::S32);
    plane!(AudioBufferRef::F32);
    plane!(AudioBufferRef::F64);
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use symphonia::core::io::MediaSourceStream;

    /// Build a mono or stereo WAV in memory (via `hound`, already a dev-dep)
    /// and hand it back as a probed `FormatReader` — the seam `decode_format`
    /// tests against directly, no temp file and no real audio file fixture
    /// needed to prove the decode loop itself is correct.
    fn probe_wav(samples: &[i16], channels: u16, rate: u32) -> Box<dyn FormatReader> {
        let mut buf: Vec<u8> = Vec::new();
        {
            let spec = hound::WavSpec {
                channels,
                sample_rate: rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::new(Cursor::new(&mut buf), spec).unwrap();
            for s in samples {
                w.write_sample(*s).unwrap();
            }
            w.finalize().unwrap();
        }
        let mss = MediaSourceStream::new(Box::new(Cursor::new(buf)), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("wav");
        symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
            .expect("hound-written WAV must probe cleanly")
            .format
    }

    #[test]
    fn a_mono_wav_round_trips_sample_values_and_rate() {
        // Known i16 values, chosen to expose an off-by-a-scale-factor or a
        // channel-order bug immediately rather than being lost in noise.
        let samples: Vec<i16> = vec![0, 16_384, -16_384, 32_767, -32_768, 100, -100];
        let format = probe_wav(&samples, 1, 44_100);
        let audio = decode_format(format).unwrap();

        assert_eq!(audio.sample_rate, 44_100);
        assert_eq!(audio.channels.len(), 1, "mono file must decode to exactly one plane");
        assert_eq!(audio.frames(), samples.len());
        for (i, &s16) in samples.iter().enumerate() {
            let expected = s16 as f32 / 32_768.0;
            assert!(
                (audio.channels[0][i] - expected).abs() < 1e-3,
                "sample {i}: expected ~{expected}, got {}",
                audio.channels[0][i]
            );
        }
    }

    #[test]
    fn a_stereo_wav_keeps_channels_separate_not_interleaved_into_one_plane() {
        // Interleaved input L0 R0 L1 R1 ... — if de-interleaving is wrong,
        // channel 0 would contain a mix of L and R samples instead of pure L.
        let interleaved: Vec<i16> = vec![
            10_000, -10_000, // frame 0: L, R
            20_000, -20_000, // frame 1
            30_000, -30_000, // frame 2
        ];
        let format = probe_wav(&interleaved, 2, 48_000);
        let audio = decode_format(format).unwrap();

        assert_eq!(audio.channels.len(), 2);
        assert_eq!(audio.frames(), 3);
        let l: Vec<i16> = audio.channels[0].iter().map(|&f| (f * 32_768.0).round() as i16).collect();
        let r: Vec<i16> = audio.channels[1].iter().map(|&f| (f * 32_768.0).round() as i16).collect();
        assert_eq!(l, vec![10_000, 20_000, 30_000], "left channel must be purely left samples");
        assert_eq!(r, vec![-10_000, -20_000, -30_000], "right channel must be purely right samples");
    }

    #[test]
    fn silence_decodes_to_silence_not_an_error() {
        let samples = vec![0i16; 1000];
        let format = probe_wav(&samples, 1, 48_000);
        let audio = decode_format(format).unwrap();
        assert_eq!(audio.frames(), 1000);
        assert!(audio.channels[0].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn an_empty_file_is_a_clean_error_not_a_panic() {
        let mss = MediaSourceStream::new(Box::new(Cursor::new(Vec::<u8>::new())), Default::default());
        let err = symphonia::default::get_probe()
            .format(&Hint::new(), mss, &FormatOptions::default(), &MetadataOptions::default())
            .err();
        assert!(err.is_some(), "an empty stream must not probe as a known format");
    }

    #[test]
    fn decode_file_reports_io_errors_for_a_missing_path() {
        let err = decode_file(Path::new("/nonexistent/path/does-not-exist.wav")).unwrap_err();
        assert!(matches!(err, DecodeError::Io(_)));
    }

    #[test]
    fn decode_error_display_is_human_readable_not_a_debug_dump() {
        let msg = format!("{}", DecodeError::UnsupportedFormat);
        assert_eq!(msg, "unrecognised audio format or container");
        let msg = format!("{}", DecodeError::NoAudioTrack);
        assert!(!msg.is_empty() && !msg.contains("NoAudioTrack"), "Display must not just echo the variant name");
    }

    #[test]
    fn a_long_file_decodes_without_truncation() {
        // Long enough to span many packets, so this also exercises the
        // next_packet loop and EOF handling beyond a single-packet file.
        let n = 200_000;
        let samples: Vec<i16> = (0..n).map(|i| ((i % 2000) as i32 - 1000) as i16).collect();
        let format = probe_wav(&samples, 1, 48_000);
        let audio = decode_format(format).unwrap();
        assert_eq!(audio.frames(), n as usize, "every sample must survive, not just the first packet");
    }
}
