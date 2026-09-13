//! flac — a lossless encoder, as bytes.
//!
//! Lifted from `apps/studio/crates/studio-media/src/flac.rs`, where it was
//! written for the library's bounces, because beatspace's arrangement
//! projects wanted the same thing and the repo could otherwise only write
//! lossless as WAV — three times the bytes for the same samples. Same
//! posture as `wav.rs`: bytes in, bytes out, no IO, so the whole thing is
//! testable from a literal.
//!
//! Deliberately the honest subset:
//!
//! * **fixed predictors** (orders 0–4, best-per-frame by residual energy)
//!   with **Rice coding**, one partition per subframe — no LPC, no
//!   mid-side. That costs some ratio against a tuned reference encoder
//!   (fixed predictors typically land ~55–65% where LPC lands ~50%) and
//!   buys an encoder small enough to read whole and verify completely;
//! * 16-bit, mono or stereo, dithered from f32 with the same TPDF rule the
//!   WAV path uses — "lossless" here means lossless FROM THE 16-BIT
//!   SAMPLES WRITTEN, said plainly rather than implied away;
//! * escape hatch to verbatim Rice partitions when a pathological frame
//!   would make unary quotients explode — always legal, never clever.
//!
//! Verified against symphonia's OWN FLAC decoder — an independent, strict
//! implementation, a dev-dependency only — with sample-exact equality,
//! multi-byte frame numbers exercised on purpose, and the compression
//! actually measured. An encoder validated only by its own decoder proves
//! nothing.

/// Why an encode was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Empty,
    UnequalChannels,
    BadRate(u32),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Empty => write!(f, "no audio to encode"),
            Error::UnequalChannels => write!(f, "channels differ in length"),
            Error::BadRate(r) => write!(f, "sample rate {r} is outside FLAC's 20-bit field"),
        }
    }
}
impl std::error::Error for Error {}

const BLOCK: usize = 4096;

struct Bw {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}
impl Bw {
    fn new() -> Bw { Bw { out: Vec::new(), acc: 0, n: 0 } }
    fn put(&mut self, bits: u32, v: u64) {
        debug_assert!(bits <= 57);
        self.acc = (self.acc << bits) | (v & ((1u64 << bits) - 1).max(u64::from(bits == 64)));
        self.n += bits;
        while self.n >= 8 {
            self.n -= 8;
            self.out.push((self.acc >> self.n) as u8);
        }
    }
    fn align(&mut self) {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put(pad, 0);
        }
    }
}

fn crc8(bytes: &[u8]) -> u8 {
    let mut c: u8 = 0;
    for b in bytes {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
        }
    }
    c
}
fn crc16(bytes: &[u8]) -> u16 {
    let mut c: u16 = 0;
    for b in bytes {
        c ^= (*b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
        }
    }
    c
}

/// FLAC's UTF-8-style coded number, for the frame index. Written the
/// boring way — explicit thresholds, one arm per width — because the
/// clever accumulator version of this function shipped first and broke at
/// exactly frame 128, the first index needing two bytes, which the
/// long-audio test exists to catch.
fn put_utf8(out: &mut Vec<u8>, v: u64) {
    let cont = |out: &mut Vec<u8>, v: u64, shifts: &[u32]| {
        for sh in shifts { out.push(0x80 | ((v >> sh) & 0x3F) as u8); }
    };
    match v {
        0..=0x7F => out.push(v as u8),
        0x80..=0x7FF => {
            out.push(0xC0 | (v >> 6) as u8);
            cont(out, v, &[0]);
        }
        0x800..=0xFFFF => {
            out.push(0xE0 | (v >> 12) as u8);
            cont(out, v, &[6, 0]);
        }
        0x1_0000..=0x1F_FFFF => {
            out.push(0xF0 | (v >> 18) as u8);
            cont(out, v, &[12, 6, 0]);
        }
        0x20_0000..=0x3FF_FFFF => {
            out.push(0xF8 | (v >> 24) as u8);
            cont(out, v, &[18, 12, 6, 0]);
        }
        0x400_0000..=0x7FFF_FFFF => {
            out.push(0xFC | (v >> 30) as u8);
            cont(out, v, &[24, 18, 12, 6, 0]);
        }
        _ => {
            out.push(0xFE);
            cont(out, v, &[30, 24, 18, 12, 6, 0]);
        }
    }
}

fn tpdf_dither_i16(samples: &[f32], seed: u64) -> Vec<i16> {
    // The WAV exporter's rule, restated: triangular dither at one LSB,
    // deterministic per seed — the same audio quantizes to the same words.
    let mut s = seed | 1;
    let mut rng = move || {
        s ^= s << 13; s ^= s >> 7; s ^= s << 17;
        (s as u32 as f32 / u32::MAX as f32) - 0.5
    };
    samples.iter().map(|x| {
        let d = (rng() + rng()) / 32768.0;
        ((x + d).clamp(-1.0, 1.0) * 32767.0).round() as i16
    }).collect()
}

fn residual(order: usize, x: &[i32], i: usize) -> i64 {
    let v = |k: usize| x[i - k] as i64;
    match order {
        0 => v(0),
        1 => v(0) - v(1),
        2 => v(0) - 2 * v(1) + v(2),
        3 => v(0) - 3 * v(1) + 3 * v(2) - v(3),
        _ => v(0) - 4 * v(1) + 6 * v(2) - 4 * v(3) + v(4),
    }
}

fn zigzag(v: i64) -> u64 { ((v << 1) ^ (v >> 63)) as u64 }

fn write_subframe(bw: &mut Bw, x: &[i32]) {
    // Best fixed order by summed |residual| — the standard cheap proxy for
    // coded size, and exactly why order choice needs no second pass.
    let max_order = x.len().saturating_sub(1).min(4);
    let (mut best, mut best_cost) = (0usize, u64::MAX);
    for order in 0..=max_order {
        let cost: u64 = (order..x.len())
            .map(|i| residual(order, x, i).unsigned_abs()).sum();
        if cost < best_cost { best = order; best_cost = cost; }
    }
    let order = best;
    bw.put(1, 0);
    bw.put(6, 0b001000 | order as u64);
    bw.put(1, 0); // no wasted bits
    for i in 0..order {
        bw.put(16, x[i] as u16 as u64);
    }
    let res: Vec<i64> = (order..x.len()).map(|i| residual(order, x, i)).collect();
    bw.put(2, 0b00); // rice, 4-bit params
    bw.put(4, 0);    // partition order 0: one partition
    let mean = if res.is_empty() { 0 }
               else { res.iter().map(|r| zigzag(*r)).sum::<u64>() / res.len() as u64 };
    let mut p = 0u32;
    while (1u64 << (p + 1)) <= mean + 1 && p < 14 { p += 1; }
    let max_bits = res.iter().map(|r| 64 - zigzag(*r).leading_zeros()).max().unwrap_or(0);
    if max_bits > 24 {
        // The pathological frame: verbatim residuals, always legal.
        let bits = max_bits.min(31) + 1;
        bw.put(4, 0b1111);
        bw.put(5, bits as u64);
        for r in &res {
            bw.put(bits, *r as u64);
        }
    } else {
        bw.put(4, p as u64);
        for r in &res {
            let u = zigzag(*r);
            let q = u >> p;
            for _ in 0..q { bw.put(1, 0); }
            bw.put(1, 1);
            if p > 0 { bw.put(p, u); }
        }
    }
}

/// Encode to FLAC: f32 planar channels in (one `Vec` per channel, equal
/// lengths; the first one or two are used), a `.flac` any decoder reads
/// out. Lossless from the 16-bit dithered samples it writes (the WAV
/// path's own dither rule, same seed semantics), mono or stereo.
pub fn encode(channels: &[Vec<f32>], sample_rate: u32, seed: u64) -> Result<Vec<u8>, Error> {
    let n_ch = channels.len().clamp(1, 2);
    if channels.is_empty() || channels[0].is_empty() {
        return Err(Error::Empty);
    }
    if channels.iter().take(n_ch).any(|c| c.len() != channels[0].len()) {
        return Err(Error::UnequalChannels);
    }
    if !(1..=(1 << 20)).contains(&sample_rate) {
        return Err(Error::BadRate(sample_rate));
    }
    let channels_n = n_ch;
    let quantized: Vec<Vec<i16>> = channels.iter().take(n_ch).enumerate()
        .map(|(ci, ch)| tpdf_dither_i16(ch, seed.wrapping_add(ci as u64)))
        .collect();
    let frames_total = quantized[0].len();

    let mut out = Vec::new();
    out.extend(b"fLaC");
    // STREAMINFO, the only metadata block, marked last.
    let mut si = Vec::new();
    si.extend((BLOCK as u16).to_be_bytes());
    si.extend((BLOCK as u16).to_be_bytes());
    si.extend([0u8; 3]); // min frame size unknown
    si.extend([0u8; 3]); // max frame size unknown
    {
        let mut bw = Bw::new();
        bw.put(20, sample_rate as u64);
        bw.put(3, channels_n as u64 - 1);
        bw.put(5, 15); // bits per sample - 1
        bw.put(36, frames_total as u64);
        bw.align();
        si.extend(bw.out);
    }
    si.extend([0u8; 16]); // MD5 unset: legal, and honest about not computing it
    out.push(0x80); // last block, STREAMINFO
    out.extend(&(si.len() as u32).to_be_bytes()[1..]);
    out.extend(si);

    for (fi, start) in (0..frames_total).step_by(BLOCK).enumerate() {
        let n = BLOCK.min(frames_total - start);
        let mut header = vec![0xFF, 0xF8];
        header.push(0b0111_0000); // blocksize: 16-bit-1 follows · rate: STREAMINFO
        header.push(((channels_n as u8 - 1) << 4) | 0b100 << 1); // assignment · 16-bit · reserved
        put_utf8(&mut header, fi as u64);
        header.extend(((n - 1) as u16).to_be_bytes());
        header.push(crc8(&header));

        let mut bw = Bw::new();
        bw.out.extend(&header);
        for ch in &quantized {
            let x: Vec<i32> = ch[start..start + n].iter().map(|s| *s as i32).collect();
            write_subframe(&mut bw, &x);
        }
        bw.align();
        let c = crc16(&bw.out);
        bw.out.extend(c.to_be_bytes());
        out.extend(bw.out);
    }
    Ok(out)
}


#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, secs: f32) -> Vec<f32> {
        (0..(48_000.0 * secs) as usize)
            .map(|i| (i as f32 / 48_000.0 * hz * std::f32::consts::TAU).sin() * 0.4)
            .collect()
    }

    /// symphonia's strict FLAC reader, on bytes. The validator is
    /// INDEPENDENT of the code under test.
    fn decode(bytes: Vec<u8>) -> (Vec<Vec<i16>>, u32) {
        use symphonia::core::codecs::audio::AudioDecoderOptions;
        use symphonia::core::formats::probe::Hint;
        use symphonia::core::formats::{FormatOptions, TrackType};
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("flac");
        let mut format = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .expect("symphonia accepts the stream");
        let track = format.tracks().iter().find(|t| t.track_type() == Some(TrackType::Audio)).expect("an audio track");
        let track_id = track.id;
        let params = track.codec_params.as_ref().and_then(|p| p.audio()).expect("audio params");
        let sr = params.sample_rate.unwrap_or(0);
        let mut decoder = symphonia::default::get_codecs().make_audio_decoder(params, &AudioDecoderOptions::default()).expect("decoder");
        let mut chans: Vec<Vec<i16>> = Vec::new();
        let mut inter: Vec<i16> = Vec::new();
        while let Some(packet) = format.next_packet().transpose() {
            let packet = packet.expect("packet");
            if packet.track_id != track_id {
                continue;
            }
            let decoded = decoder.decode(&packet).expect("decode");
            let n = decoded.spec().channels().count();
            chans.resize(n, Vec::new());
            decoded.copy_to_vec_interleaved::<i16>(&mut inter);
            for frame in inter.chunks_exact(n) {
                for (c, v) in frame.iter().enumerate() {
                    chans[c].push(*v);
                }
            }
        }
        (chans, sr)
    }

    #[test]
    fn a_flac_round_trips_sample_exact_through_symphonias_own_decoder() {
        let planar = vec![tone(440.0, 1.0), tone(660.0, 1.0)];
        let bytes = encode(&planar, 48_000, 7).unwrap();
        assert_eq!(&bytes[..4], b"fLaC");
        let (back, sr) = decode(bytes);
        assert_eq!(sr, 48_000);
        assert_eq!(back.len(), 2);
        let want: Vec<Vec<i16>> = planar.iter().enumerate().map(|(ci, ch)| tpdf_dither_i16(ch, 7 + ci as u64)).collect();
        assert_eq!(back, want, "sample-exact from the words written");
    }

    #[test]
    fn long_audio_crosses_the_multi_byte_frame_number_boundary() {
        // Frame 128 is the first needing two bytes of the coded number;
        // 130 frames of 4096 is ~11 s at 48 kHz.
        let planar = vec![tone(110.0, 11.2)];
        let bytes = encode(&planar, 48_000, 3).unwrap();
        let (back, _) = decode(bytes);
        assert_eq!(back[0].len(), planar[0].len());
        assert_eq!(back[0], tpdf_dither_i16(&planar[0], 3));
    }

    #[test]
    fn a_tone_compresses_and_noise_does_not_break_it() {
        let planar = vec![tone(220.0, 2.0)];
        let bytes = encode(&planar, 48_000, 1).unwrap();
        let wav_bytes = planar[0].len() * 2;
        assert!(bytes.len() < wav_bytes * 7 / 10, "tone: {} of {} wav bytes", bytes.len(), wav_bytes);
        let mut s = 99u64;
        let noise: Vec<f32> = (0..48_000).map(|_| { s ^= s << 13; s ^= s >> 7; s ^= s << 17; (s as u32 as f32 / u32::MAX as f32) * 2.0 - 1.0 }).collect();
        let bytes = encode(&vec![noise.clone()], 48_000, 1).unwrap();
        let (back, _) = decode(bytes);
        assert_eq!(back[0], tpdf_dither_i16(&noise, 1));
    }

    #[test]
    fn refusals_are_named() {
        assert_eq!(encode(&[], 48_000, 0).unwrap_err(), Error::Empty);
        assert_eq!(encode(&[vec![]], 48_000, 0).unwrap_err(), Error::Empty);
        assert_eq!(encode(&[vec![0.0; 10], vec![0.0; 9]], 48_000, 0).unwrap_err(), Error::UnequalChannels);
        assert_eq!(encode(&[vec![0.0; 10]], 0, 0).unwrap_err(), Error::BadRate(0));
        // Other sample rates go into STREAMINFO as given.
        let (_, sr) = decode(encode(&[vec![0.1; 5000]], 22_050, 0).unwrap());
        assert_eq!(sr, 22_050);
    }
}
