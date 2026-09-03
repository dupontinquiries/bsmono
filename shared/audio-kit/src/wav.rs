//! wav — RIFF/WAVE in and out, as bytes.
//!
//! Seven hand-rolled implementations in this repository before this one:
//! `apps/voiceterm/src/corpus_recorder.rs` (`write_wav`, building the RIFF
//! header by hand), `apps/beatspace/src/audio_io.rs` (`save_wav`),
//! `tools/kbm3/gpu/src/audio.rs` (`write_wav_pcm16`, `write_wav_f32`,
//! `decode_wav`), `apps/voiceterm/src/acoustic_index.rs`
//! (`load_wav_16k_mono`), and `apps/studio/crates/studio-audio/src/export.rs`
//! (`export_wav_dithered`). This crate's header already says its job is the
//! leaf blocks the apps kept re-implementing.
//!
//! **Bytes, not files.** `audio-kit` does no IO and this does not change that:
//! a caller reads the file. That also makes the whole module testable from a
//! literal.
//!
//! ## The bug this found
//!
//! RIFF chunks are word-aligned: a chunk with an ODD size is followed by a pad
//! byte that is not counted in the size. `acoustic_index.rs`'s parser walks
//! chunks correctly in every other respect — it handles `fmt ` size variants
//! and skips `LIST` — but advances by `p += size` with no pad. One odd-sized
//! chunk and every subsequent chunk id reads from one byte off, so `data` is
//! never found and the function returns `None`. The file does not fail loudly;
//! it silently does not load, and odd-sized `LIST`/`INFO` chunks are exactly
//! what a metadata-writing DAW or recorder produces.
//!
//! `a_pad_byte_after_an_odd_chunk_does_not_desynchronise_the_walk` is that
//! case.
//!
//! ## What it accepts
//!
//! PCM 8/16/24/32-bit integer and 32/64-bit float, mono or many channels,
//! including `WAVE_FORMAT_EXTENSIBLE` (0xFFFE) — which is what most modern
//! recorders write and which a parser checking `format == 1` rejects outright.
//! Samples come back interleaved as `f32`, because that is what everything
//! downstream in this repo already speaks.

/// Interleaved float samples with their format.
#[derive(Clone, PartialEq, Debug)]
pub struct Wav {
    pub channels: u16,
    pub sample_rate: u32,
    /// Interleaved, `[-1, 1]` for integer sources.
    pub samples: Vec<f32>,
}

impl Wav {
    /// Frames, i.e. samples per channel.
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
    pub fn seconds(&self) -> f32 {
        self.frames() as f32 / self.sample_rate.max(1) as f32
    }
    /// Average the channels down to one. Correct for analysis; the caller
    /// decides whether it is correct for their purpose.
    pub fn mono(&self) -> Vec<f32> {
        let ch = self.channels.max(1) as usize;
        if ch == 1 {
            return self.samples.clone();
        }
        self.samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect()
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Error {
    /// No `RIFF....WAVE`.
    NotRiff,
    /// No `fmt ` chunk, or one too short to read.
    NoFormat,
    /// No `data` chunk.
    NoData,
    /// A codec this does not decode — ADPCM, mu-law, MP3-in-WAV.
    UnsupportedFormat(u16),
    /// A bit depth this does not decode.
    UnsupportedDepth(u16),
    /// A chunk header runs off the end.
    Truncated,
}

const FMT_PCM: u16 = 1;
const FMT_FLOAT: u16 = 3;
const FMT_EXTENSIBLE: u16 = 0xFFFE;

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Parse a RIFF/WAVE buffer.
pub fn parse(bytes: &[u8]) -> Result<Wav, Error> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(Error::NotRiff);
    }
    let mut p = 12usize;
    let (mut format, mut channels, mut rate, mut bits) = (0u16, 0u16, 0u32, 0u16);
    let mut data: Option<(usize, usize)> = None;

    while p + 8 <= bytes.len() {
        let id = &bytes[p..p + 4];
        let size = u32le(bytes, p + 4) as usize;
        let body = p + 8;
        if body + size > bytes.len() {
            // A truncated final chunk is common on an interrupted recording.
            // Take what is there rather than losing the whole take.
            if id == b"data" {
                data = Some((body, bytes.len()));
            }
            break;
        }
        if id == b"fmt " && size >= 16 {
            format = u16le(bytes, body);
            channels = u16le(bytes, body + 2);
            rate = u32le(bytes, body + 4);
            bits = u16le(bytes, body + 14);
            if format == FMT_EXTENSIBLE && size >= 40 {
                // The real codec is the first two bytes of the SubFormat GUID.
                // A parser that stops at `format == 1` rejects most modern
                // recorders outright.
                format = u16le(bytes, body + 24);
            }
        } else if id == b"data" {
            data = Some((body, body + size));
        }
        // RIFF IS WORD-ALIGNED: an odd size is followed by a pad byte that the
        // size does not count. Omitting this is the desync described above.
        p = body + size + (size & 1);
    }

    if channels == 0 || rate == 0 {
        return Err(Error::NoFormat);
    }
    let (start, end) = data.ok_or(Error::NoData)?;
    let raw = &bytes[start..end];

    let samples = match (format, bits) {
        (FMT_PCM, 8) => raw.iter().map(|b| (*b as f32 - 128.0) / 128.0).collect(),
        (FMT_PCM, 16) => raw
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect(),
        (FMT_PCM, 24) => raw
            .chunks_exact(3)
            .map(|c| {
                let v = ((c[2] as i32) << 24 | (c[1] as i32) << 16 | (c[0] as i32) << 8) >> 8;
                v as f32 / 8_388_608.0
            })
            .collect(),
        (FMT_PCM, 32) => raw
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0)
            .collect(),
        (FMT_FLOAT, 32) => raw
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        (FMT_FLOAT, 64) => raw
            .chunks_exact(8)
            .map(|c| {
                let mut v = [0u8; 8];
                v.copy_from_slice(c);
                f64::from_le_bytes(v) as f32
            })
            .collect(),
        (FMT_PCM | FMT_FLOAT, d) => return Err(Error::UnsupportedDepth(d)),
        (f, _) => return Err(Error::UnsupportedFormat(f)),
    };
    Ok(Wav { channels, sample_rate: rate, samples })
}

fn header(out: &mut Vec<u8>, format: u16, channels: u16, rate: u32, bits: u16, data_len: usize) {
    let block_align = channels * bits / 8;
    let byte_rate = rate * block_align as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&format.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
}

/// 16-bit PCM. Clamped, not wrapped — a sample over 1.0 becomes full scale
/// rather than the opposite polarity, which is the difference between a hot
/// take and a destroyed one.
///
/// Note this does NOT dither. `studio-audio::export_wav_dithered` exists for
/// mastering; for a VO master the source is already the master and requantising
/// with noise would be the wrong default.
pub fn write_pcm16(samples: &[f32], rate: u32, channels: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    header(&mut out, FMT_PCM, channels.max(1), rate, 16, samples.len() * 2);
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// 32-bit float. Lossless for a float pipeline and the right choice for a
/// studio master.
pub fn write_f32(samples: &[f32], rate: u32, channels: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + samples.len() * 4);
    header(&mut out, FMT_FLOAT, channels.max(1), rate, 32, samples.len() * 4);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| (i as f32 / n as f32) * 2.0 - 1.0).collect()
    }

    #[test]
    fn f32_round_trips_exactly() {
        let s = ramp(500);
        let w = parse(&write_f32(&s, 48_000, 1)).unwrap();
        assert_eq!(w.sample_rate, 48_000);
        assert_eq!(w.channels, 1);
        assert_eq!(w.samples, s, "float is lossless and must come back bit-identical");
    }

    #[test]
    fn pcm16_round_trips_within_the_scaling_asymmetry() {
        // NOT one quantisation step, and the reason is worth stating because
        // the first version of this test asserted one and failed by 1.4.
        //
        // Signed 16-bit runs -32768..=32767, so the two directions cannot
        // share a scale factor. Encoding multiplies by 32767 (32768 would
        // overflow on +1.0); decoding divides by 32768 (the true full-scale
        // magnitude, so -32768 maps to exactly -1.0). That asymmetry is
        // deliberate and standard, and it costs up to ~1.5 LSB on a round
        // trip. Tightening the tolerance would mean changing one of the two
        // scale factors, and both are the conventional choice.
        let s = ramp(500);
        let w = parse(&write_pcm16(&s, 44_100, 1)).unwrap();
        assert_eq!(w.sample_rate, 44_100);
        let lsb = 1.0 / 32768.0;
        for (a, b) in s.iter().zip(&w.samples) {
            assert!((a - b).abs() < 2.0 * lsb, "{a} -> {b} is more than 2 LSB");
        }
        // and the error is a scaling bias rather than noise: everything shrinks
        // slightly toward zero, never away from it
        assert!(s.iter().zip(&w.samples).all(|(a, b)| b.abs() <= a.abs() + lsb));
    }

    #[test]
    fn a_hot_sample_clamps_rather_than_wrapping() {
        // The difference between a hot take and a destroyed one: wrapping
        // flips polarity and sounds like a gunshot.
        let w = parse(&write_pcm16(&[2.0, -2.0, 0.0], 48_000, 1)).unwrap();
        assert!(w.samples[0] > 0.99, "{}", w.samples[0]);
        assert!(w.samples[1] < -0.99, "{}", w.samples[1]);
    }

    #[test]
    fn a_pad_byte_after_an_odd_chunk_does_not_desynchronise_the_walk() {
        // THE BUG THIS MODULE EXISTS FOR. acoustic_index.rs advances by
        // `p += size` with no pad, so one odd-sized LIST/INFO chunk — exactly
        // what a metadata-writing recorder produces — puts every later chunk
        // id one byte off, `data` is never found, and the load silently
        // returns None.
        let audio = write_f32(&ramp(64), 48_000, 1);
        let mut odd = Vec::new();
        odd.extend_from_slice(&audio[0..12]); // RIFF/WAVE
        // an odd-sized LIST chunk, 5 bytes + 1 pad
        odd.extend_from_slice(b"LIST");
        odd.extend_from_slice(&5u32.to_le_bytes());
        odd.extend_from_slice(b"INFOx");
        odd.push(0); // the pad byte the size does not count
        odd.extend_from_slice(&audio[12..]);
        let fixed = odd.len() - 8;
        odd[4..8].copy_from_slice(&(fixed as u32).to_le_bytes());

        let w = parse(&odd).expect("a padded odd chunk defeated the walk");
        assert_eq!(w.samples.len(), 64);

        // and the naive walk really does fail on it, so the test is testing
        // something
        let naive = {
            let b = &odd;
            let mut p = 12usize;
            let mut found = false;
            while p + 8 <= b.len() {
                let id = &b[p..p + 4];
                let size = u32le(b, p + 4) as usize;
                if id == b"data" {
                    found = true;
                    break;
                }
                p += 8 + size; // no pad — the bug
            }
            found
        };
        assert!(!naive, "the fixture no longer reproduces the desync");
    }

    #[test]
    fn extensible_format_is_read_from_the_subformat_guid() {
        // 0xFFFE is what most modern recorders write. A parser checking
        // `format == 1` rejects them outright.
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&40u32.to_le_bytes());
        b.extend_from_slice(&FMT_EXTENSIBLE.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // channels
        b.extend_from_slice(&48_000u32.to_le_bytes());
        b.extend_from_slice(&96_000u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes()); // bits
        b.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        b.extend_from_slice(&16u16.to_le_bytes()); // valid bits
        b.extend_from_slice(&0u32.to_le_bytes()); // channel mask
        b.extend_from_slice(&FMT_PCM.to_le_bytes()); // SubFormat GUID, first 2
        b.extend_from_slice(&[0u8; 14]);
        b.extend_from_slice(b"data");
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&0i16.to_le_bytes());
        b.extend_from_slice(&16384i16.to_le_bytes());
        let n = b.len() - 8;
        b[4..8].copy_from_slice(&(n as u32).to_le_bytes());

        let w = parse(&b).expect("extensible was rejected");
        assert_eq!(w.samples.len(), 2);
        assert!((w.samples[1] - 0.5).abs() < 0.001);
    }

    #[test]
    fn an_interrupted_recording_keeps_what_it_has() {
        // A data chunk whose declared size exceeds the file: the recorder
        // died. Losing the whole take because the header is optimistic would
        // be the wrong call.
        let mut b = write_f32(&ramp(100), 48_000, 1);
        b.truncate(b.len() - 120);
        let w = parse(&b).expect("a truncated take was thrown away");
        assert!(w.samples.len() > 50 && w.samples.len() < 100, "{}", w.samples.len());
    }

    #[test]
    fn stereo_interleaves_and_folds_down() {
        let s = vec![1.0, -1.0, 0.5, -0.5];
        let w = parse(&write_f32(&s, 48_000, 2)).unwrap();
        assert_eq!(w.channels, 2);
        assert_eq!(w.frames(), 2);
        assert_eq!(w.mono(), vec![0.0, 0.0]);
    }

    #[test]
    fn refusals_are_specific() {
        assert_eq!(parse(b"not a wav at all"), Err(Error::NotRiff));
        let mut b = write_f32(&ramp(8), 48_000, 1);
        // claim a codec we do not decode
        b[20..22].copy_from_slice(&17u16.to_le_bytes()); // ADPCM
        assert_eq!(parse(&b), Err(Error::UnsupportedFormat(17)));
    }

    #[test]
    fn every_depth_this_claims_to_read_actually_reads() {
        for (fmt, bits) in [(FMT_PCM, 8u16), (FMT_PCM, 16), (FMT_PCM, 24), (FMT_PCM, 32), (FMT_FLOAT, 32)] {
            let mut b = Vec::new();
            let frames = 8usize;
            let bytes = frames * (bits as usize / 8);
            header(&mut b, fmt, 1, 48_000, bits, bytes);
            b.extend(std::iter::repeat(0u8).take(bytes));
            let w = parse(&b).unwrap_or_else(|e| panic!("{fmt}/{bits} refused: {e:?}"));
            assert_eq!(w.samples.len(), frames, "{fmt}/{bits} decoded the wrong count");
        }
    }
}
