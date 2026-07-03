//! End-to-end parametric vocoder you can hear.
//!
//! Reads a 16-bit mono WAV, runs each 20 ms frame through the full parametric
//! stack — prosody (pitch/energy/voicing) + LPC (spectral envelope) → robotic
//! source-filter synthesis — and writes the reconstructed WAV. Also prints the
//! parametric bitrate so you can see the bandwidth picture.
//!
//! Run:
//!   # convert your clip to 16 kHz mono 16-bit if needed:
//!   ffmpeg -i voice.wav -ar 16000 -ac 1 -c:a pcm_s16le in16.wav
//!   cargo run --release --example lpc_roundtrip --offline -- in16.wav out.wav
//!   # then just play out.wav — it'll sound robotic; that's the LPC vocoder.
//!
//! This is analysis→synthesis on one machine (no quantisation, no network) so
//! you can judge the *voice quality ceiling* of this approach before investing
//! in the codebook/transport. The codebook-quantised, networked version sounds
//! the same or slightly worse — this is the best case for the parameters.

use std::fs;
use std::process::exit;

use snapclone_audio::lpc::{self, LpcSynth, LPC_ORDER};
use snapclone_audio::prosody;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: lpc_roundtrip <in.wav> <out.wav>   (16-bit mono WAV)");
        exit(2);
    }
    let (sr, pcm) = match read_wav(&args[1]) {
        Ok(v) => v,
        Err(e) => { eprintln!("error reading {}: {e}", args[1]); exit(1); }
    };
    if pcm.is_empty() {
        eprintln!("no samples in {}", args[1]);
        exit(1);
    }

    let frame_len = ((sr as usize) * 20 / 1000).max(1); // 20 ms frames
    let mut synth = LpcSynth::new();
    let mut out: Vec<i16> = Vec::with_capacity(pcm.len());
    let (mut frames, mut voiced) = (0u64, 0u64);

    for chunk in pcm.chunks(frame_len) {
        // pad the last partial frame with zeros for analysis
        let frame: Vec<i16> = if chunk.len() == frame_len {
            chunk.to_vec()
        } else {
            let mut f = chunk.to_vec();
            f.resize(frame_len, 0);
            f
        };
        let pf = prosody::analyze(&frame, sr);
        let (coeffs, gain) = lpc::analyze(&frame, sr);
        let y = synth.synthesize(&coeffs, gain, pf.f0_hz, pf.voiced, frame.len(), sr, pf.energy);
        out.extend_from_slice(&y[..chunk.len().min(y.len())]); // trim padding
        frames += 1;
        if pf.voiced { voiced += 1; }
    }

    if let Err(e) = write_wav(&args[2], sr, &out) {
        eprintln!("error writing {}: {e}", args[2]);
        exit(1);
    }

    let fps = sr as f64 / frame_len as f64;
    // scalar-quantised parameter budget per frame (a rough, honest estimate)
    let bits_per_frame = LPC_ORDER as f64 * 6.0 /*coeffs*/ + 5.0 /*gain*/ + 7.0 /*pitch*/ + 1.0 /*voiced*/;
    let kbps = bits_per_frame * fps / 1000.0;
    eprintln!(
        "ok: {frames} frames ({voiced} voiced), {sr} Hz, {} samples in → {} out",
        pcm.len(), out.len()
    );
    eprintln!(
        "parametric rate ≈ {kbps:.1} kbps  (scalar-quantised: {LPC_ORDER} coeffs + gain + pitch + voicing/frame)"
    );
    eprintln!(
        "  reference: Opus wideband ≈ 16–24 kbps, raw PCM = {} kbps",
        sr * 16 / 1000
    );
    eprintln!("  codebook-quantising the coeff vector (1 index vs {LPC_ORDER} coeffs) drops this further");
    eprintln!("wrote {}", args[2]);
}

/// Minimal 16-bit mono WAV reader (scans chunks; word-aligned).
fn read_wav(path: &str) -> Result<(u32, Vec<i16>), String> {
    let b = fs::read(path).map_err(|e| e.to_string())?;
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let (mut sr, mut channels, mut bits) = (0u32, 0u16, 0u16);
    let mut data: Vec<i16> = Vec::new();
    let le16 = |s: &[u8]| u16::from_le_bytes([s[0], s[1]]);
    let le32 = |s: &[u8]| u32::from_le_bytes([s[0], s[1], s[2], s[3]]);

    let mut pos = 12usize;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let sz = le32(&b[pos + 4..pos + 8]) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= b.len() {
            channels = le16(&b[body + 2..body + 4]);
            sr = le32(&b[body + 4..body + 8]);
            bits = le16(&b[body + 14..body + 16]);
        } else if id == b"data" {
            let end = (body + sz).min(b.len());
            let mut i = body;
            while i + 1 < end {
                data.push(i16::from_le_bytes([b[i], b[i + 1]]));
                i += 2;
            }
        }
        pos = body + sz + (sz & 1); // pad to even
    }
    if bits != 16 || channels != 1 {
        return Err(format!(
            "need 16-bit mono (got {bits}-bit, {channels}ch). Convert: \
             ffmpeg -i in.wav -ar 16000 -ac 1 -c:a pcm_s16le out.wav"
        ));
    }
    Ok((sr, data))
}

/// Minimal canonical 16-bit mono WAV writer.
fn write_wav(path: &str, sr: u32, data: &[i16]) -> std::io::Result<()> {
    let data_bytes = (data.len() * 2) as u32;
    let mut o: Vec<u8> = Vec::with_capacity(44 + data_bytes as usize);
    o.extend_from_slice(b"RIFF");
    o.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    o.extend_from_slice(b"WAVE");
    o.extend_from_slice(b"fmt ");
    o.extend_from_slice(&16u32.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes()); // PCM
    o.extend_from_slice(&1u16.to_le_bytes()); // mono
    o.extend_from_slice(&sr.to_le_bytes());
    o.extend_from_slice(&(sr * 2).to_le_bytes()); // byte rate
    o.extend_from_slice(&2u16.to_le_bytes()); // block align
    o.extend_from_slice(&16u16.to_le_bytes()); // bits
    o.extend_from_slice(b"data");
    o.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in data {
        o.extend_from_slice(&s.to_le_bytes());
    }
    fs::write(path, o)
}
