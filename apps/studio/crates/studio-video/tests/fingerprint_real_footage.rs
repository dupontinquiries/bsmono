//! Real-world check for the fingerprint module: does it actually survive a
//! genuine transcode (different codec, resolution, bitrate), on real
//! footage, not just synthetic test data. Gated behind env vars so it
//! never runs in a normal `cargo test` -- needs real files and takes real
//! wall-clock time (ffmpeg per sample).
//!
//!     STUDIO_TEST_ORIGINAL=/path/original.mkv \
//!     STUDIO_TEST_TRANSCODED=/path/transcoded.mp4 \
//!     cargo test --test fingerprint_real_footage -- --nocapture

use studio_video::fingerprint::{best_alignment, fingerprint_clip};

#[test]
fn a_real_transcode_still_matches_its_original() {
    let (Some(original), Some(transcoded)) = (std::env::var_os("STUDIO_TEST_ORIGINAL"), std::env::var_os("STUDIO_TEST_TRANSCODED")) else {
        eprintln!("skipping: set STUDIO_TEST_ORIGINAL and STUDIO_TEST_TRANSCODED to run this");
        return;
    };

    let fp_original = fingerprint_clip(std::path::Path::new(&original), 2.0).expect("fingerprinting the original failed");
    let fp_transcoded = fingerprint_clip(std::path::Path::new(&transcoded), 2.0).expect("fingerprinting the transcode failed");

    println!("original: {} samples, transcoded: {} samples", fp_original.hashes.len(), fp_transcoded.hashes.len());

    let m = best_alignment(&fp_transcoded.hashes, &fp_original.hashes, 12).expect("must find some alignment");
    println!("best offset: {}, matched {}/{} ({:.1}%)", m.offset, m.matched, m.compared, m.fraction() * 100.0);

    assert_eq!(m.offset, 0, "the transcode starts at the same point as the original -- offset should be 0");
    assert!(m.fraction() > 0.7, "expected most samples to match within the hamming threshold, got {:.2}", m.fraction());
}
