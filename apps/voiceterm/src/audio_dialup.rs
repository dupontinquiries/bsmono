//! v0.1.711: acoustic "dial-up" handshake modem. Encodes a byte payload as a
//! sequence of audio tones (4-FSK, 2 bits/symbol) and decodes it back via a
//! Goertzel filter bank. This is the data-over-sound carrier for the peer
//! handshake — two machines exchange a small handshake blob over the speaker/
//! mic, dial-up style, with a voice-print spoken alongside for identity.
//!
//! Everything here is PURE and round-trip testable IN MEMORY (no audio device):
//! payload → symbols → f32 samples → symbols → payload. The actual playback
//! (samples → cpal) and capture (mic → samples) are thin wrappers added later.
//!
//! Frame:  [preamble ×PREAMBLE_SYMS] [len:2B] [payload] [crc16:2B]
//! Symbols carry 2 bits each, so each byte = 4 symbols, LSB pair first.

/// 4-FSK tone table (Hz). Spaced ~220 Hz apart in a band that survives laptop
/// speakers/mics and is above most room rumble. Index = symbol value 0..3.
pub const TONES: [f32; 4] = [1200.0, 1500.0, 1800.0, 2100.0];
/// A distinct preamble tone the receiver locks onto before the frame.
pub const PREAMBLE_TONE: f32 = 900.0;
/// Symbols of preamble (a steady "carrier" the receiver syncs on).
pub const PREAMBLE_SYMS: usize = 8;
/// Samples per symbol at the reference sample rate. ~50ms/symbol @ 8kHz.
pub const SAMPLES_PER_SYMBOL: usize = 400;
/// Reference sample rate the codec works in.
pub const SAMPLE_RATE: u32 = 8000;

/// Encode a payload into the symbol stream (0..3 each), framed with length +
/// CRC and prefixed with a preamble. Symbol 4 is reserved as the preamble
/// marker in the public stream (mapped to PREAMBLE_TONE).
pub fn encode_payload(payload: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(payload.len() + 4);
    let len = payload.len().min(0xFFFF) as u16;
    framed.extend_from_slice(&len.to_le_bytes());
    framed.extend_from_slice(&payload[..len as usize]);
    let crc = crc16(&framed);
    framed.extend_from_slice(&crc.to_le_bytes());

    let mut syms = Vec::with_capacity(PREAMBLE_SYMS + framed.len() * 4);
    for _ in 0..PREAMBLE_SYMS { syms.push(4u8); } // preamble marker
    for &b in &framed {
        for pair in 0..4 {
            syms.push((b >> (pair * 2)) & 0b11);
        }
    }
    syms
}

/// Decode a symbol stream (as produced by encode_payload or recovered from
/// samples) back into the payload. None if no valid frame / CRC mismatch.
pub fn decode_payload(symbols: &[u8]) -> Option<Vec<u8>> {
    // skip to first non-preamble symbol (drop the preamble run + any leading
    // marker symbols the detector emitted).
    let start = symbols.iter().position(|&s| s < 4)?;
    let data_syms = &symbols[start..];
    // need at least len(2B=8 syms) + crc(2B=8 syms) = 16 symbols
    if data_syms.len() < 16 { return None; }
    let bytes = symbols_to_bytes(data_syms);
    if bytes.len() < 4 { return None; }
    let len = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
    if bytes.len() < 2 + len + 2 { return None; }
    let framed = &bytes[..2 + len];
    let got_crc = u16::from_le_bytes([bytes[2 + len], bytes[2 + len + 1]]);
    if crc16(framed) != got_crc { return None; }
    Some(bytes[2..2 + len].to_vec())
}

/// Pack a run of 2-bit symbols (<4) into bytes, LSB pair first. Stops at the
/// first marker symbol (>=4) or end.
fn symbols_to_bytes(syms: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(syms.len() / 4);
    let mut cur = 0u8;
    let mut pair = 0;
    for &s in syms {
        if s >= 4 { break; }
        cur |= (s & 0b11) << (pair * 2);
        pair += 1;
        if pair == 4 { out.push(cur); cur = 0; pair = 0; }
    }
    out
}

// ── tone synthesis + detection (the actual sound) ──

/// Render a symbol stream to PCM f32 samples (mono) at SAMPLE_RATE. Preamble
/// markers (sym 4) use PREAMBLE_TONE; data symbols use TONES[sym].
pub fn symbols_to_samples(symbols: &[u8]) -> Vec<f32> {
    let mut out = Vec::with_capacity(symbols.len() * SAMPLES_PER_SYMBOL);
    let sr = SAMPLE_RATE as f32;
    for &s in symbols {
        let f = if s >= 4 { PREAMBLE_TONE } else { TONES[s as usize] };
        for n in 0..SAMPLES_PER_SYMBOL {
            let t = n as f32 / sr;
            out.push((2.0 * std::f32::consts::PI * f * t).sin());
        }
    }
    out
}

/// Recover the symbol stream from PCM samples by Goertzel-detecting the
/// dominant tone in each symbol-length window. Returns symbol 4 for windows
/// whose dominant tone is the preamble, 0..3 for data tones.
pub fn samples_to_symbols(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::new();
    let win = SAMPLES_PER_SYMBOL;
    let mut off = 0;
    while off + win <= samples.len() {
        let chunk = &samples[off..off + win];
        out.push(detect_symbol(chunk));
        off += win;
    }
    out
}

/// Goertzel power of `freq` in `chunk` at SAMPLE_RATE.
fn goertzel(chunk: &[f32], freq: f32) -> f32 {
    let n = chunk.len() as f32;
    let k = (0.5 + (n * freq) / SAMPLE_RATE as f32).floor();
    let w = (2.0 * std::f32::consts::PI / n) * k;
    let cosine = w.cos();
    let coeff = 2.0 * cosine;
    let (mut s_prev, mut s_prev2) = (0.0f32, 0.0f32);
    for &x in chunk {
        let s = x + coeff * s_prev - s_prev2;
        s_prev2 = s_prev;
        s_prev = s;
    }
    s_prev2 * s_prev2 + s_prev * s_prev - coeff * s_prev * s_prev2
}

/// Which tone dominates this window? 0..3 for data, 4 for preamble.
fn detect_symbol(chunk: &[f32]) -> u8 {
    let mut best = 4u8;
    let mut best_pow = goertzel(chunk, PREAMBLE_TONE);
    for (i, &f) in TONES.iter().enumerate() {
        let p = goertzel(chunk, f);
        if p > best_pow { best_pow = p; best = i as u8; }
    }
    best
}

// ── CRC-16/CCITT (pure, no deps) ──

pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_known_vector() {
        // CRC-16/CCITT-FALSE of "123456789" = 0x29B1
        assert_eq!(crc16(b"123456789"), 0x29B1);
    }

    #[test]
    fn symbol_roundtrip_bytes() {
        let payload = b"peer:alice";
        let syms = encode_payload(payload);
        // preamble present
        assert!(syms.iter().take(PREAMBLE_SYMS).all(|&s| s == 4));
        let back = decode_payload(&syms).expect("decode");
        assert_eq!(&back, payload);
    }

    #[test]
    fn empty_payload_roundtrips() {
        let syms = encode_payload(b"");
        assert_eq!(decode_payload(&syms), Some(vec![]));
    }

    #[test]
    fn crc_mismatch_rejected() {
        let mut syms = encode_payload(b"hello");
        // corrupt a data symbol after the preamble
        let i = PREAMBLE_SYMS + 9;
        syms[i] = (syms[i] + 1) & 0b11;
        assert_eq!(decode_payload(&syms), None);
    }

    #[test]
    fn full_acoustic_roundtrip() {
        // the real thing: payload → symbols → SOUND samples → Goertzel → symbols
        // → payload, entirely in memory.
        let payload = b"web2qml-handshake-v1 fingerprint=ab12cd34";
        let syms = encode_payload(payload);
        let samples = symbols_to_samples(&syms);
        assert_eq!(samples.len(), syms.len() * SAMPLES_PER_SYMBOL);
        let recovered = samples_to_symbols(&samples);
        let back = decode_payload(&recovered).expect("acoustic decode");
        assert_eq!(&back, payload);
    }

    #[test]
    fn acoustic_roundtrip_with_noise() {
        // add mild gaussian-ish noise; CRC + tone separation should survive.
        let payload = b"noisy channel test 0123";
        let syms = encode_payload(payload);
        let mut samples = symbols_to_samples(&syms);
        // deterministic pseudo-noise
        let mut seed: u32 = 0x1234_5678;
        for s in samples.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = ((seed >> 16) as f32 / 65535.0 - 0.5) * 0.30; // ±0.15
            *s += n;
        }
        let recovered = samples_to_symbols(&samples);
        let back = decode_payload(&recovered).expect("decode under noise");
        assert_eq!(&back, payload);
    }

    #[test]
    fn detect_each_tone() {
        for (i, &f) in TONES.iter().enumerate() {
            let chunk: Vec<f32> = (0..SAMPLES_PER_SYMBOL)
                .map(|n| (2.0 * std::f32::consts::PI * f * n as f32 / SAMPLE_RATE as f32).sin())
                .collect();
            assert_eq!(detect_symbol(&chunk), i as u8);
        }
    }
}
