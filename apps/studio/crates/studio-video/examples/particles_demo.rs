//! Smoke and dirt in a real shot, driven by a synth.
//!
//!     cargo run -p studio-video --example particles_demo -- SHOT.mkv START_S SYNTH.wav OUT.mp4
use std::path::PathBuf;
use studio_video::framepipe::{Pipe, Size};
use studio_video::particles::{envelope, Params, System};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (shot, start, synth, out) = (PathBuf::from(&a[0]), a[1].parse::<f64>().unwrap(), PathBuf::from(&a[2]), PathBuf::from(&a[3]));
    let (fps, secs) = (24.0, 8.0);
    let size = Size { w: 480, h: 270 };
    // the synth's loudness is the drive
    let pcm = std::process::Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&synth)
        .args(["-ac", "1", "-ar", "48000", "-f", "f32le", "-"]).output().unwrap().stdout;
    let mono: Vec<f32> = pcm.chunks(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let drive = envelope(&mono, 48_000, fps);
    // smoke from the lamp on the table, dirt lifting off the floor
    let mut smoke = System::new(Params { size: (4.0, 16.0), swirl_scale: 30.0, ..Params::creepy_smoke() }, 11);
    let mut dirt = System::new(Params { emit_radius: (200.0, 6.0), size: (0.9, 0.6), ..Params::floating_dirt() }, 12);
    let dt = 1.0 / fps as f32;
    let silent = out.with_extension("silent.mp4");
    let n = Pipe { input: shot, range: Some((start, secs)), size, fps }.map(&silent, 18, |i, mut f| {
        let d = drive.get(i).copied().unwrap_or(0.0);
        smoke.step(dt, (205.0, 176.0), d);
        dirt.step(dt, (240.0, 262.0), d);
        dirt.draw(&mut f);
        smoke.draw(&mut f);
        f
    }).expect("renders");
    let r = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-i"]).arg(&silent).arg("-i").arg(&synth)
        .args(["-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "128k", "-shortest"]).arg(&out).status().unwrap();
    let _ = std::fs::remove_file(&silent);
    let peak = drive.iter().cloned().fold(0.0f32, f32::max);
    println!("{n} frames, synth drive peak {peak:.2}, mux ok: {}", r.success());
}
