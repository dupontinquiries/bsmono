//! The snap encoder, checked by something that did not write it.
//!
//! `ppm_bytes` has unit tests, and they were written by the same person
//! who wrote the encoder — so they agree about the format by construction.
//! A header field in the wrong order, or a channel swap, would satisfy
//! both and produce a file nothing else will open.
//!
//! This asks `ffmpeg` instead: it decodes the file, reports what it
//! thinks the dimensions and pixel format are, and hands back raw pixels
//! this test compares against what went in. That is an independent
//! opinion about the format, which is the only kind worth having here.
//!
//! ## It skips where ffmpeg is absent
//!
//! A machine without it is not a broken encoder, and a test that failed
//! there would be turned off — after which it finds nothing.

use std::path::PathBuf;
use std::process::Command;

use snapclone_egui::frame_pump::{ppm_bytes, RawFrame};

fn have(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("snapclone-ppm-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

/// A frame whose pixels are a function of their position, so a transposed
/// or reordered image is visible as wrong VALUES rather than as noise.
fn frame(w: u32, h: u32) -> RawFrame {
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&[(x * 4) as u8, (y * 5) as u8, 200, 255]);
        }
    }
    RawFrame { width: w, height: h, captured_ms: 0, seq: 0, rgba: px.into() }
}

#[test]
fn a_real_decoder_agrees_about_the_size_and_the_pixels() {
    if !have("ffprobe") || !have("ffmpeg") {
        eprintln!("no ffmpeg here — skipping");
        return;
    }
    let (w, h) = (64u32, 48u32);
    let f = frame(w, h);
    let path = tmp("snap.ppm");
    std::fs::write(&path, ppm_bytes(&f)).unwrap();

    let probe = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=width,height,pix_fmt",
               "-of", "default=nw=1"])
        .arg(&path)
        .output()
        .expect("ffprobe did not run");
    let info = String::from_utf8_lossy(&probe.stdout);
    assert!(info.contains(&format!("width={w}")), "ffprobe said: {info}");
    assert!(info.contains(&format!("height={h}")), "ffprobe said: {info}");
    // rgb24 rather than bgr24 — a channel swap makes a file that opens
    // perfectly and looks wrong, which is worse than one that fails.
    assert!(info.contains("pix_fmt=rgb24"), "ffprobe said: {info}");

    let raw = tmp("snap.raw");
    let dec = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&path)
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-y"])
        .arg(&raw)
        .output()
        .expect("ffmpeg did not run");
    assert!(dec.status.success(), "ffmpeg refused the file: {}",
            String::from_utf8_lossy(&dec.stderr));

    let px = std::fs::read(&raw).unwrap();
    assert_eq!(px.len(), (w * h * 3) as usize, "decoded to the wrong size");
    assert_eq!(&px[..3], &[0, 0, 200], "the first pixel came back changed");
    // Row 1, column 2 — checks that rows are not transposed and that the
    // stride is right, which a square image would hide.
    let i = ((1 * w + 2) * 3) as usize;
    assert_eq!(&px[i..i + 3], &[8, 5, 200], "pixel (2,1) is wrong");

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&raw);
}

#[test]
fn a_non_square_frame_is_not_transposed() {
    // Width and height in the wrong order in the header produces a file a
    // decoder accepts and draws sideways. Only a non-square frame shows
    // it, which is why the sizes here differ.
    if !have("ffprobe") {
        eprintln!("no ffprobe here — skipping");
        return;
    }
    let f = frame(30, 10);
    let path = tmp("wide.ppm");
    std::fs::write(&path, ppm_bytes(&f)).unwrap();
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(&path)
        .output()
        .expect("ffprobe did not run");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "30,10");
    let _ = std::fs::remove_file(&path);
}
