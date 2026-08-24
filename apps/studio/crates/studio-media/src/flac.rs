//! flac — a lossless encoder for the library's bounces.
//!
//! The repo could DECODE FLAC from day one (symphonia) and could only ever
//! write lossless as WAV — three times the bytes for the same samples,
//! which on a phone is the difference between a sample library and a full
//! disk. This is a real FLAC encoder, deliberately the honest subset:
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
//! Every claim is checked against symphonia's OWN decoder — an
//! independent, strict implementation this crate already ships — with
//! sample-exact equality, multi-byte frame numbers exercised on purpose,
//! and the compression actually measured. An encoder validated only by
//! its own decoder proves nothing.

use crate::SourceAudio;

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

/// Encode to FLAC: f32 planar in, a `.flac` any decoder reads out.
/// Lossless from the 16-bit dithered samples it writes (the WAV path's own
/// dither rule, same seed semantics), mono or stereo.
pub fn encode_flac(audio: &SourceAudio, seed: u64) -> Result<Vec<u8>, String> {
    let channels = audio.channels.len().clamp(1, 2);
    if audio.frames() == 0 { return Err("no audio to encode".into()); }
    let quantized: Vec<Vec<i16>> = audio.channels.iter().take(channels).enumerate()
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
        bw.put(20, audio.sample_rate as u64);
        bw.put(3, channels as u64 - 1);
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
        header.push(((channels as u8 - 1) << 4) | 0b100 << 1); // assignment · 16-bit · reserved
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

    #[test]
    fn a_flac_round_trips_sample_exact_through_symphonias_own_decoder() {
        // The validator is INDEPENDENT: symphonia's strict FLAC reader,
        // not this encoder's mirror. Sample-exact against the quantized
        // words — lossless meaning exactly what it says.
        let a = SourceAudio { sample_rate: 48_000,
                              channels: vec![tone(220.0, 0.7), tone(333.0, 0.7)] };
        let bytes = encode_flac(&a, 7).expect("encodes");
        let back = crate::decode_bytes(bytes, Some("flac")).expect("symphonia accepts it");
        assert_eq!(back.sample_rate, 48_000);
        assert_eq!(back.channels.len(), 2);
        assert_eq!(back.frames(), a.frames());
        let want: Vec<Vec<i16>> = a.channels.iter().enumerate()
            .map(|(ci, ch)| super::tpdf_dither_i16(ch, 7 + ci as u64)).collect();
        for ch in 0..2 {
            for (i, s) in back.channels[ch].iter().enumerate() {
                let got = (s * 32767.0).round() as i16;
                assert_eq!(got, want[ch][i], "channel {ch}, sample {i}");
            }
        }
    }

    #[test]
    fn long_audio_exercises_multibyte_frame_numbers_and_still_decodes() {
        // 200 frames of 4096: the frame index outgrows one UTF-8 byte at
        // 128, which is exactly where a hand-rolled coded-number writer
        // breaks if it is going to.
        let n = BLOCK * 200;
        let mut s = 0x1234_5678_9abc_def1u64;
        let noise: Vec<f32> = (0..n).map(|_| {
            s ^= s << 13; s ^= s >> 7; s ^= s << 17;
            ((s as u32 as f32 / u32::MAX as f32) - 0.5) * 0.02
        }).collect();
        let a = SourceAudio { sample_rate: 48_000, channels: vec![noise] };
        let bytes = encode_flac(&a, 3).expect("encodes");
        let back = crate::decode_bytes(bytes, Some("flac")).expect("decodes whole");
        assert_eq!(back.frames(), n, "every frame arrived, index 0 through 199");
    }

    #[test]
    fn tonal_audio_actually_compresses() {
        let a = SourceAudio { sample_rate: 48_000, channels: vec![tone(110.0, 2.0)] };
        let flac = encode_flac(&a, 1).unwrap().len();
        let raw = a.frames() * 2;
        assert!(flac < raw * 7 / 10,
                "fixed predictors earn their keep on tonal material: {flac} vs {raw} raw");
    }
}
