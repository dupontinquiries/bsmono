//! Audio decoding to mono f32 at a target sample rate, plus wav export.
//!
//! `.opus` files: ogg crate (container) + opus crate (codec, needs system
//! libopus). Everything else (wav/flac/mp3/m4a/aac/ogg-vorbis) goes through
//! symphonia. After decoding we downmix to mono and linearly resample to the
//! configured sample rate.

use anyhow::{anyhow, Context, Result};
use std::fs::File;
use std::path::Path;

/// Decode any supported file to mono f32 at `target_sr`.
pub fn decode_to_mono(path: &Path, target_sr: u32) -> Result<Vec<f32>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (samples, sr) = if ext == "opus" {
        decode_opus(path)?
    } else {
        decode_symphonia(path)?
    };
    Ok(resample_linear(&samples, sr, target_sr))
}

// ----------------------------------------------------------------------------
// Opus (.opus in an ogg container)
// ----------------------------------------------------------------------------

fn decode_opus(path: &Path) -> Result<(Vec<f32>, u32)> {
    use ogg::reading::PacketReader;
    use opus::{Channels, Decoder};

    let file = File::open(path).with_context(|| format!("open {path:?}"))?;
    let mut reader = PacketReader::new(file);

    // Opus is always decoded at 48 kHz; max 120 ms frame = 5760 samples/channel.
    const DEC_SR: u32 = 48_000;
    const MAX_FRAME: usize = 5760;

    let mut channels = 1usize;
    let mut pre_skip = 0usize;
    let mut decoder: Option<Decoder> = None;
    let mut out_mono: Vec<f32> = Vec::new();
    let mut header_seen = false;
    let mut tags_seen = false;

    while let Some(packet) = reader.read_packet()? {
        let data = &packet.data;

        // First packet: OpusHead. Second: OpusTags. Both are non-audio.
        if !header_seen && data.starts_with(b"OpusHead") && data.len() >= 19 {
            channels = data[9] as usize;
            pre_skip = u16::from_le_bytes([data[10], data[11]]) as usize;
            let ch = if channels >= 2 {
                Channels::Stereo
            } else {
                Channels::Mono
            };
            decoder = Some(
                Decoder::new(DEC_SR, ch).map_err(|e| anyhow!("opus decoder init: {e}"))?,
            );
            header_seen = true;
            continue;
        }
        if header_seen && !tags_seen && data.starts_with(b"OpusTags") {
            tags_seen = true;
            continue;
        }
        if !header_seen {
            // Stream didn't start with OpusHead — not an opus logical stream.
            continue;
        }

        let dec = decoder
            .as_mut()
            .ok_or_else(|| anyhow!("opus audio before header"))?;
        let mut buf = vec![0i16; MAX_FRAME * channels.max(1)];
        let n = dec
            .decode(data, &mut buf, false)
            .map_err(|e| anyhow!("opus decode: {e}"))?; // samples per channel
        // downmix interleaved -> mono f32
        for frame in 0..n {
            let mut acc = 0f32;
            for c in 0..channels {
                acc += buf[frame * channels + c] as f32 / 32768.0;
            }
            out_mono.push(acc / channels as f32);
        }
    }

    if !header_seen {
        return Err(anyhow!("no OpusHead found in {path:?}"));
    }
    // Trim encoder pre-skip (defined at 48 kHz).
    if pre_skip > 0 && pre_skip < out_mono.len() {
        out_mono.drain(0..pre_skip);
    }
    Ok((out_mono, DEC_SR))
}

// ----------------------------------------------------------------------------
// Everything else via symphonia
// ----------------------------------------------------------------------------

fn decode_symphonia(path: &Path) -> Result<(Vec<f32>, u32)> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let file = File::open(path).with_context(|| format!("open {path:?}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    // symphonia 0.6: `probe` returns the format reader directly.
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .with_context(|| format!("probe {path:?}"))?;

    // First track that is audio and has codec params we can decode.
    let track = format
        .tracks()
        .iter()
        .find(|t| {
            t.track_type() == Some(TrackType::Audio)
                && t.codec_params.as_ref().and_then(|p| p.audio()).is_some()
        })
        .ok_or_else(|| anyhow!("no decodable audio track in {path:?}"))?;
    let track_id = track.id;
    let audio_params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| anyhow!("missing audio codec params in {path:?}"))?;
    let sr = audio_params.sample_rate.unwrap_or(44_100);

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(audio_params, &AudioDecoderOptions::default())
        .with_context(|| "make audio decoder")?;

    let mut mono: Vec<f32> = Vec::new();
    let mut interleaved: Vec<f32> = Vec::new();

    // next_packet() now yields Result<Option<Packet>>; None == clean EOF.
    while let Some(packet) = format.next_packet().transpose() {
        let packet = match packet {
            Ok(p) => p,
            Err(_) => break,
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(_) => break,
        };

        let ch = decoded.spec().channels().count().max(1);
        decoded.copy_to_vec_interleaved::<f32>(&mut interleaved);
        // Downmix interleaved -> mono.
        for frame in interleaved.chunks_exact(ch) {
            let acc: f32 = frame.iter().copied().sum();
            mono.push(acc / ch as f32);
        }
    }

    if mono.is_empty() {
        return Err(anyhow!("decoded no audio from {path:?}"));
    }
    Ok((mono, sr))
}

// ----------------------------------------------------------------------------
// Linear resampler (good enough for training data prep)
// ----------------------------------------------------------------------------

pub fn resample_linear(input: &[f32], from_sr: u32, to_sr: u32) -> Vec<f32> {
    if from_sr == to_sr || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to_sr as f64 / from_sr as f64;
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    let step = from_sr as f64 / to_sr as f64; // input samples per output sample
    let mut pos = 0f64;
    for _ in 0..out_len {
        let i = pos.floor() as usize;
        let frac = (pos - i as f64) as f32;
        let a = input.get(i).copied().unwrap_or(0.0);
        let b = input.get(i + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
        pos += step;
    }
    out
}

// ----------------------------------------------------------------------------
// WAV export (16-bit PCM)
// ----------------------------------------------------------------------------

pub fn save_wav(path: &Path, samples: &[f32], sr: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sr,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        writer.write_sample(v)?;
    }
    writer.finalize()?;
    Ok(())
}
