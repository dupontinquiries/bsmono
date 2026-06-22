// sim_phonetic.rs — standalone harness: real synthesized speech → the
// ACTUAL phonetic pipeline (compute_mfcc + acoustic_index::fingerprint).
//
// This is a DEV harness, not a unit test — it depends on espeak-ng having
// written WAVs (run scripts/sim-workflow.sh first, or point it at any
// 16k/22k mono PCM WAVs). Build standalone:
//
//   # stub the few crate:: deps, then:
//   rustc -O --edition 2021 scripts/sim_phonetic.rs -o /tmp/sim_phonetic
//
// It exists to PROVE the plumbing: synthesized speech produces sensible
// MFCC frame counts (scaling with phrase length) and 39-dim fingerprints.
//
// HONEST SCOPE (see scripts/sim-workflow.sh header): validates plumbing +
// relative behaviour. espeak voices barely separate (one engine + pitch
// shift, not different vocal tracts), so this is NOT a speaker-ID accuracy
// test — real recorded voices are needed for that.
//
// The body is intentionally a thin shell; the real logic lives in the
// project's phonetic + acoustic_index modules. This file documents HOW to
// drive them from synthesized audio so the technique is reproducible.

fn read_wav_16k(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("read wav");
    let rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]) as f32;
    // locate the data chunk
    let mut i = 12usize;
    let (mut doff, mut dlen) = (0usize, 0usize);
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let sz = u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
        if id == b"data" { doff = i + 8; dlen = sz; break; }
        i += 8 + sz + (sz & 1);
    }
    let mut s = Vec::new();
    let mut j = doff;
    while j + 1 < doff + dlen && j + 1 < bytes.len() {
        s.push(i16::from_le_bytes([bytes[j], bytes[j + 1]]) as f32 / 32768.0);
        j += 2;
    }
    // linear resample to 16 kHz (the pipeline's expected rate)
    let ratio = 16000.0 / rate;
    let outn = (s.len() as f32 * ratio) as usize;
    (0..outn).map(|k| {
        let src = k as f32 / ratio;
        let i0 = src as usize;
        let frac = src - i0 as f32;
        let a = s.get(i0).copied().unwrap_or(0.0);
        let b = s.get(i0 + 1).copied().unwrap_or(a);
        a + (b - a) * frac
    }).collect()
}

// To run for real, paste the project's phonetic::compute_mfcc + N_MFCC and
// acoustic_index::fingerprint here (with crate:: deps stubbed:
// security::persistence_enabled→false, util::epoch_secs→0, voice::
// is_app_backgrounded→false, clog!→noop, cpu_efficiency::EcoGuard::new→
// drop). The recipe is documented in docs/design/sim_harness.md.
fn main() {
    let dir = std::env::var("TTS_DIR").unwrap_or_else(|_| "/tmp/sim-tts".into());
    let mut any = false;
    for k in 0..16 {
        let p = format!("{}/p{}.wav", dir, k);
        if std::path::Path::new(&p).exists() {
            any = true;
            let audio = read_wav_16k(&p);
            println!("p{}: {} samples @16k ({:.2}s) — feed to compute_mfcc + fingerprint",
                     k, audio.len(), audio.len() as f32 / 16000.0);
        }
    }
    if !any {
        eprintln!("no WAVs in {} — run scripts/sim-workflow.sh first", dir);
    }
}
