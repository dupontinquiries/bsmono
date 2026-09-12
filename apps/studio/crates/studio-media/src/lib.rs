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

pub mod flac;
#[cfg(feature = "opus")]
mod opus;

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

#[derive(Debug)]
pub enum EncodeError {
    /// Input shape this encoder does not take (rate, channel count) — a
    /// caller bug named plainly, never resampled over quietly.
    Unsupported(String),
    Encode(String),
}
impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::Unsupported(m) => write!(f, "cannot encode: {m}"),
            EncodeError::Encode(m) => write!(f, "opus encode failed: {m}"),
        }
    }
}
impl std::error::Error for EncodeError {}

/// Encode 48 kHz mono/stereo audio to Ogg Opus bytes — the export mirror of
/// this crate's `.opus` decode, sharing its trimming discipline (see the
/// `opus` module). `bitrate_bps`: 96_000 is transparent for most music;
/// 128_000 is a safe default for a master.
#[cfg(feature = "opus")]
pub fn encode_opus(audio: &SourceAudio, bitrate_bps: i32) -> Result<Vec<u8>, EncodeError> {
    opus::encode_opus(audio, bitrate_bps)
}
#[cfg(not(feature = "opus"))]
pub fn encode_opus(_audio: &SourceAudio, _bitrate_bps: i32) -> Result<Vec<u8>, EncodeError> {
    Err(EncodeError::Unsupported(
        "studio-media was built without the `opus` feature (needs system libopus)".into(),
    ))
}

/// Decode an entire audio file into memory as planar `f32`, at the file's
/// own sample rate. Intended for the same use `studio_live::Recorder::
/// to_audio` and a pattern bounce already serve: producing a
/// `SourceAudio` a caller registers under a `SourceId` and hands to
/// `studio_audio::mixdown` — decode-once-into-memory, not a streaming
/// decoder, which matches how every other source (a recorded take, a
/// bounced pattern) already reaches the mix.
pub fn decode_file(path: &Path) -> Result<SourceAudio, DecodeError> {
    if path.extension().and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("opus"))
    {
        return decode_opus_or_explain(|| opus_impl::decode_opus_file(path));
    }
    let file = std::fs::File::open(path)?;
    let hint = extension_hint(path);
    decode_source(Box::new(file), hint)
}

// The two build flavours share one dispatch point, so the no-libopus build
// FAILS a .opus with a sentence naming the fix rather than Symphonia's
// generic "unrecognised container" — which would send someone debugging a
// perfectly good file instead of a build flag.
#[cfg(feature = "opus")]
use opus as opus_impl;
#[cfg(feature = "opus")]
fn decode_opus_or_explain(
    f: impl FnOnce() -> Result<SourceAudio, DecodeError>,
) -> Result<SourceAudio, DecodeError> {
    f()
}
#[cfg(not(feature = "opus"))]
fn decode_opus_or_explain(
    _f: impl FnOnce() -> Result<SourceAudio, DecodeError>,
) -> Result<SourceAudio, DecodeError> {
    Err(DecodeError::Decode(
        "this is an Opus file, but studio-media was built without the `opus`          feature (needs system libopus)".into(),
    ))
}
#[cfg(not(feature = "opus"))]
mod opus_impl {
    use super::*;
    pub(crate) fn decode_opus_file(_: &Path) -> Result<SourceAudio, DecodeError> {
        unreachable!("gated by decode_opus_or_explain")
    }
    pub(crate) fn decode_opus_bytes(_: Vec<u8>) -> Result<SourceAudio, DecodeError> {
        unreachable!("gated by decode_opus_or_explain")
    }
    pub(crate) fn sniff_opus(_: &[u8]) -> bool { false }
}

/// Decode audio already in memory — e.g. a subprocess's captured stdout
/// (`studio_video::extract_audio` pipes ffmpeg's WAV output straight here
/// rather than round-tripping through a temp file), or bytes fetched over
/// the network. Same decode loop as `decode_file`, minus the filesystem.
pub fn decode_bytes(bytes: Vec<u8>, extension_hint_str: Option<&str>) -> Result<SourceAudio, DecodeError> {
    // Route Opus by hint OR by sniffing the first page: bytes arriving from
    // `studio-video`'s extraction or a collab asset have no filename, and
    // Symphonia would otherwise fail them as an unknown-codec ogg.
    if extension_hint_str.is_some_and(|e| e.eq_ignore_ascii_case("opus"))
        || opus_impl::sniff_opus(&bytes)
    {
        return decode_opus_or_explain(move || opus_impl::decode_opus_bytes(bytes));
    }
    let mut hint = Hint::new();
    if let Some(ext) = extension_hint_str {
        hint.with_extension(ext);
    }
    decode_source(Box::new(std::io::Cursor::new(bytes)), hint)
}

fn extension_hint(path: &Path) -> Hint {
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    hint
}

fn decode_source(source: Box<dyn symphonia::core::io::MediaSource>, hint: Hint) -> Result<SourceAudio, DecodeError> {
    let mss = MediaSourceStream::new(source, Default::default());
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
/// Can this machine run the ffmpeg-backed checks, with the encoders they need?
///
/// The comment on `ffmpeg_agrees_our_files_are_opus_of_the_right_length`
/// used to say "ffprobe is already a hard requirement of studio-video's
/// tests". That was true and is now false: studio-video skips cleanly when
/// ffmpeg cannot encode what its fixture needs, and these two tests were
/// the remaining places that assume presence. Both `expect()` on the
/// spawn, so on a box with a stripped ffmpeg they fail rather than skip —
/// the same shape that made the fleet report studio-video "bugs".
///
/// `want` is checked against `-encoders`, since a build can know OF a codec
/// it cannot encode with, and encoding is what these tests do.
pub(crate) fn ffmpeg_can(want: &[&str]) -> bool {
    use std::process::Command;
    let ok = |c: &str| Command::new(c).arg("-version").output()
        .map(|o| o.status.success()).unwrap_or(false);
    if !ok("ffmpeg") || !ok("ffprobe") { return false; }
    let Ok(o) = Command::new("ffmpeg").arg("-hide_banner").arg("-encoders").output() else {
        return false;
    };
    if !o.status.success() { return false; }
    let enc = String::from_utf8_lossy(&o.stdout).into_owned();
    for w in want {
        if !enc.lines().any(|l| l.split_whitespace().nth(1) == Some(*w)) {
            eprintln!("studio-media: ffmpeg cannot encode {w} — skipping this test");
            return false;
        }
    }
    true
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
    fn decode_bytes_matches_decode_file_for_the_same_wav_content() {
        // The seam studio_video::extract_audio actually needs: bytes that
        // never touched a file (a subprocess's captured stdout, here stood
        // in for by writing to a temp file only so decode_file has
        // something to compare against — decode_bytes itself never reads
        // one) must decode identically to the same bytes read from disk.
        let samples: Vec<i16> = vec![0, 12_345, -12_345, 32_767, -32_768];
        let spec = hound::WavSpec { channels: 1, sample_rate: 44_100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = hound::WavWriter::new(std::io::Cursor::new(&mut buf), spec).unwrap();
            for s in &samples { w.write_sample(*s).unwrap(); }
            w.finalize().unwrap();
        }

        let from_bytes = decode_bytes(buf.clone(), Some("wav")).unwrap();

        let tmp = std::env::temp_dir().join(format!("studio-media-test-{}.wav", std::process::id()));
        std::fs::write(&tmp, &buf).unwrap();
        let from_file = decode_file(&tmp).unwrap();
        let _ = std::fs::remove_file(&tmp);

        assert_eq!(from_bytes.sample_rate, from_file.sample_rate);
        assert_eq!(from_bytes.channels, from_file.channels);
    }

    #[test]
    fn decode_bytes_without_an_extension_hint_still_probes_a_wav() {
        // Symphonia's probe doesn't strictly need the hint for a well-formed
        // WAV (it sniffs the RIFF/WAVE magic) — worth pinning, since
        // studio_video won't always have a meaningful extension to pass
        // (ffmpeg's stdout has none).
        let samples: Vec<i16> = vec![100, -100, 200, -200];
        let spec = hound::WavSpec { channels: 1, sample_rate: 22_050, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = hound::WavWriter::new(std::io::Cursor::new(&mut buf), spec).unwrap();
            for s in &samples { w.write_sample(*s).unwrap(); }
            w.finalize().unwrap();
        }
        let audio = decode_bytes(buf, None).unwrap();
        assert_eq!(audio.sample_rate, 22_050);
        assert_eq!(audio.frames(), 4);
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

#[cfg(all(test, feature = "opus"))]
mod opus_tests {
    use super::*;
    use std::process::Command;

    /// Encode a lavfi-described signal to a temp .opus with the real ffmpeg,
    /// the same convention `studio-video`'s tests set: fixtures from a real
    /// encoder, not hand-built bytes a hand-built parser would tautologically
    /// pass.
    fn ffmpeg_opus(name: &str, filter: &str, channels: u32) -> std::path::PathBuf {
        let out = std::env::temp_dir().join(format!("studio-media-{name}.opus"));
        let status = Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi", "-i", filter, "-ac", &channels.to_string(),
                   "-c:a", "libopus", "-b:a", "96k"])
            .arg(&out)
            .output()
            .expect("ffmpeg present (studio-video's tests already require it)");
        assert!(status.status.success(), "ffmpeg failed: {}",
                String::from_utf8_lossy(&status.stderr));
        out
    }

    #[test]
    fn a_stereo_opus_decodes_at_48k_stereo_and_exactly_its_length() {
        let p = ffmpeg_opus("stereo-sine", "sine=frequency=440:duration=1:sample_rate=48000", 2);
        let a = decode_file(&p).expect("decodes");
        assert_eq!(a.sample_rate, 48_000, "opus is a 48k codec by contract");
        assert_eq!(a.channels.len(), 2, "stereo stays stereo — this is a sampler, not a mel front-end");
        // BOTH trims proven by one number: without the pre-skip trim this
        // would be ~312 samples long; without the granule end-trim, up to a
        // frame of padding long. 48_000 ± a muxer's rounding means both cut.
        let n = a.frames();
        assert!((47_900..=48_100).contains(&n), "1.0s should be ~48000 frames, got {n}");
        // 440 Hz sine crosses zero ~880 times a second.
        let zc = a.channels[0].windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count();
        assert!((850..=910).contains(&zc), "440Hz sine, got {zc} crossings");
    }

    #[test]
    fn the_loop_seam_is_audio_not_padding() {
        // The reason the granule trim exists: a melody loop's last
        // milliseconds must be the music, not codec padding, or the loop
        // ticks at every bar line. A constant sine's tail RMS must match its
        // overall RMS; trailing zeros would crater it.
        let p = ffmpeg_opus("loop-tail", "sine=frequency=220:duration=1:sample_rate=48000", 1);
        let a = decode_file(&p).expect("decodes");
        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
        let whole = rms(&a.channels[0]);
        let tail = rms(&a.channels[0][a.channels[0].len() - 240..]);
        assert!(tail > whole * 0.7, "tail rms {tail} vs whole {whole}: padding survived the trim");
    }

    #[test]
    fn mono_stays_mono() {
        let p = ffmpeg_opus("mono", "sine=frequency=330:duration=0.25:sample_rate=48000", 1);
        let a = decode_file(&p).expect("decodes");
        assert_eq!(a.channels.len(), 1);
    }

    #[test]
    fn bytes_with_no_name_decode_the_same_as_the_file() {
        // Bytes from a collab asset or a video's extracted track arrive
        // nameless; the OggS/OpusHead sniff has to carry them to the same
        // decoder the path took.
        let p = ffmpeg_opus("sniff", "sine=frequency=550:duration=0.25:sample_rate=48000", 2);
        let by_path = decode_file(&p).expect("file decodes");
        let bytes = std::fs::read(&p).unwrap();
        assert!(super::opus::sniff_opus(&bytes), "first page carries OpusHead");
        let by_bytes = decode_bytes(bytes, None).expect("bytes decode");
        assert_eq!(by_path.channels, by_bytes.channels, "one decoder, one answer");
    }


    #[test]
    fn a_vorbis_ogg_is_not_mistaken_for_opus() {
        // libvorbis is optional in many builds, so this needs the guard even
        // where ffmpeg itself is present.
        if !ffmpeg_can(&["libvorbis"]) { return; }
        // Same container, different codec: the sniff must let Symphonia keep
        // the .ogg files it already handles.
        let out = std::env::temp_dir().join("studio-media-vorbis.ogg");
        let ok = Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=0.25:sample_rate=48000",
                   "-c:a", "libvorbis"])
            .arg(&out).output().expect("ffmpeg runs");
        assert!(ok.status.success());
        let bytes = std::fs::read(&out).unwrap();
        assert!(!super::opus::sniff_opus(&bytes));
        assert!(decode_bytes(bytes, Some("ogg")).is_ok(), "vorbis still decodes via symphonia");
    }
}

#[cfg(all(test, feature = "opus"))]
mod encode_tests {
    use super::*;

    fn sine(hz: f32, secs: f32, channels: usize) -> SourceAudio {
        let n = (48_000.0 * secs) as usize;
        let plane: Vec<f32> = (0..n)
            .map(|i| (i as f32 / 48_000.0 * hz * std::f32::consts::TAU).sin() * 0.5)
            .collect();
        SourceAudio { sample_rate: 48_000, channels: vec![plane; channels] }
    }

    #[test]
    fn our_encode_roundtrips_through_our_decode_to_the_exact_length() {
        // The decoder demanded honest pre-skip and end-granule from
        // everyone else's files; this proves the encoder pays the same tax.
        // BOTH alignments, deliberately: an unaligned tail's padding can
        // hide an unflushed encoder (its zeros stand in for the missing
        // audio), and a frame-aligned input has no padding to hide behind —
        // the first draft passed on 1.003s and lost 312 samples on 1.0s.
        for secs in [1.003f32, 1.0] {
            let input = sine(440.0, secs, 2);
            let bytes = encode_opus(&input, 96_000).expect("encodes");
            let back = decode_bytes(bytes, None).expect("our own decoder reads it (sniff, no hint)");
            assert_eq!(back.channels.len(), 2);
            assert_eq!(back.frames(), input.frames(),
                       "pre-skip and end-trim cancel to the sample at {secs}s");
        }
        let input = sine(440.0, 1.0, 2);
        let bytes = encode_opus(&input, 96_000).expect("encodes");
        let back = decode_bytes(bytes, None).expect("decodes");
        // Lossy, but a mid-band sine at 96kbps comes back close: compare RMS
        // of the error against RMS of the signal, skipping the first 5ms
        // where the codec is still converging.
        let skip = 240;
        let (mut se, mut ss) = (0f64, 0f64);
        for i in skip..input.frames() {
            let d = (input.channels[0][i] - back.channels[0][i]) as f64;
            se += d * d;
            ss += (input.channels[0][i] as f64).powi(2);
        }
        let rel = (se / ss).sqrt();
        assert!(rel < 0.15, "error RMS {rel:.3} of signal RMS — not the same sound");
    }

    #[test]
    fn mono_encodes_as_mono() {
        let input = sine(220.0, 0.25, 1);
        let bytes = encode_opus(&input, 64_000).expect("encodes");
        let back = decode_bytes(bytes, Some("opus")).expect("decodes");
        assert_eq!(back.channels.len(), 1);
        assert_eq!(back.frames(), input.frames());
    }

    #[test]
    fn ffmpeg_agrees_our_files_are_opus_of_the_right_length() {
        // Only ffprobe is used here, but the same presence question applies.
        if !ffmpeg_can(&[]) { return; }
        // Roundtripping through our own decoder proves self-consistency; a
        // third-party reader proves the files leave the building. ffprobe
        // is already a hard requirement of studio-video's tests.
        let input = sine(330.0, 1.0, 2);
        let bytes = encode_opus(&input, 96_000).expect("encodes");
        let path = std::env::temp_dir().join("studio-media-encoded.opus");
        std::fs::write(&path, &bytes).unwrap();
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "stream=codec_name,duration",
                   "-of", "default=noprint_wrappers=1"])
            .arg(&path)
            .output().expect("ffprobe runs");
        let text = String::from_utf8_lossy(&probe.stdout);
        assert!(text.contains("codec_name=opus"), "ffprobe said: {text}");
        let dur: f32 = text.lines()
            .find_map(|l| l.strip_prefix("duration=")).and_then(|d| d.parse().ok())
            .expect("a duration line");
        assert!((dur - 1.0).abs() < 0.02, "ffprobe duration {dur} vs 1.0s");
    }

    #[test]
    fn the_wrong_rate_is_refused_with_the_fix_in_the_message() {
        let wrong = SourceAudio { sample_rate: 44_100, channels: vec![vec![0.0; 100]] };
        let err = encode_opus(&wrong, 96_000).unwrap_err();
        assert!(err.to_string().contains("48"), "message was: {err}");
    }
}
