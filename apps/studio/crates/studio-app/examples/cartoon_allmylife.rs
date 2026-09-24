//! A tiny synthetic "All My Life": the edit's habits, in basic shapes.
//!
//!     cargo run -p studio-app --example cartoon_allmylife -- OUT_DIR
//!
//! A cartoon, not a downscale: the real video's soundtrack is the artist's
//! song, and a fixture should carry no one's music; and because every cut,
//! look and effect here is generated, a truth file says exactly what is where
//! -- tests can check Studio against it instead of against estimates.
//!
//! What the analysis of the real edit found, reproduced:
//! * a white-outfit dancer across three places (field, car roof, parking lot);
//! * two looks: neutral daylight, and the pink split tone (magenta-lavender
//!   highlights over warm shadows);
//! * kaleidoscope shots (Studio's own kaleidoscope) and a clone shot (the
//!   dancer three times);
//! * a glitch transition (four frames of shuffled blocks) and a fade to the
//!   credits on black;
//! * a 92 BPM synthetic track -- kicks, hats, a pad, a sung line -- with some
//!   cuts on kicks and some off (the real edit followed the performer more
//!   than the beat), and the cuts marked `on_move` coming out of a surge in
//!   the dancer's arms, the one habit that measured true.
//!
//! Output: `allmylife-cartoon.mp4` (320x180, 24 fps, 24 s) and `truth.json`.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const W: usize = 320;
const H: usize = 180;
const FPS: f64 = 24.0;
const SR: u32 = 16_000;
const LEN: f64 = 24.0;
const BPM: f64 = 92.0;
const BEAT0: f64 = 0.25;

fn beat(k: usize) -> f64 { BEAT0 + k as f64 * 60.0 / BPM }

#[derive(Clone, Copy, PartialEq, Debug)]
enum Place { Field, Roof, Lot }
#[derive(Clone, Copy, PartialEq, Debug)]
enum Look { Daylight, Pink }
#[derive(Clone, Copy, PartialEq, Debug)]
enum Fx { None, Kaleido(u32), Clone, Close }

struct Shot { start: f64, end: f64, place: Place, look: Look, fx: Fx, on_move: bool }

fn shots() -> Vec<Shot> {
    // cut times: on the beat grid where noted, off it elsewhere
    let c = [beat(4), 5.05, beat(11), 9.30, beat(18), 13.60, beat(23), 17.90, beat(30), 21.80];
    let spec: [(Place, Look, Fx, bool); 11] = [
        (Place::Field, Look::Daylight, Fx::None, false),
        (Place::Roof, Look::Pink, Fx::None, true),
        (Place::Field, Look::Daylight, Fx::None, true),
        (Place::Lot, Look::Daylight, Fx::None, false),
        (Place::Roof, Look::Pink, Fx::None, true),
        (Place::Roof, Look::Pink, Fx::Kaleido(6), false),
        (Place::Roof, Look::Pink, Fx::Close, true),
        (Place::Field, Look::Daylight, Fx::Clone, true),
        (Place::Roof, Look::Pink, Fx::Kaleido(8), false),
        (Place::Lot, Look::Daylight, Fx::None, false),
        (Place::Lot, Look::Daylight, Fx::None, false),   // fades to the credits
    ];
    let mut out = Vec::new();
    let mut t = 0.0;
    for (i, s) in spec.iter().enumerate() {
        let end = if i < c.len() { c[i] } else { LEN };
        out.push(Shot { start: t, end, place: s.0, look: s.1, fx: s.2, on_move: s.3 });
        t = end;
    }
    out
}

struct Img { px: Vec<u8> }
impl Img {
    fn new() -> Img { Img { px: vec![0; W * H * 3] } }
    fn set(&mut self, x: i32, y: i32, c: [f32; 3]) {
        if x < 0 || y < 0 || x >= W as i32 || y >= H as i32 { return; }
        let i = (y as usize * W + x as usize) * 3;
        for k in 0..3 { self.px[i + k] = c[k].clamp(0.0, 255.0) as u8; }
    }
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 3]) {
        for y in y0 as i32..y1 as i32 { for x in x0 as i32..x1 as i32 { self.set(x, y, c); } }
    }
    fn disc(&mut self, cx: f32, cy: f32, r: f32, c: [f32; 3]) {
        for y in (cy - r) as i32..=(cy + r) as i32 { for x in (cx - r) as i32..=(cx + r) as i32 {
            if (x as f32 - cx).hypot(y as f32 - cy) <= r { self.set(x, y, c); }
        }}
    }
    fn line(&mut self, a: (f32, f32), b: (f32, f32), r: f32, c: [f32; 3]) {
        let n = ((b.0 - a.0).hypot(b.1 - a.1) / (r * 0.5)).ceil().max(1.0) as usize;
        for k in 0..=n { let u = k as f32 / n as f32; self.disc(a.0 + (b.0 - a.0) * u, a.1 + (b.1 - a.1) * u, r, c); }
    }
}

/// The pink look as a grade: highlights toward magenta-lavender, shadows warm,
/// brighter -- the split tone measured on the real edit.
fn pink(c: [f32; 3]) -> [f32; 3] {
    let l = (0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]) / 255.0;
    let lift = 1.15;
    let hi = [18.0, -10.0, 16.0];   // magenta-lavender
    let sh = [16.0, 8.0, -12.0];    // warm
    let mut o = [0.0f32; 3];
    for k in 0..3 { o[k] = c[k] * lift + hi[k] * l + sh[k] * (1.0 - l) + 12.0; }
    o
}

fn background(g: &mut Img, place: Place) {
    for y in 0..H {
        let v = y as f32 / H as f32;
        for x in 0..W {
            let c = match place {
                Place::Field if v < 0.55 => [110.0 + 60.0 * v, 160.0 + 40.0 * v, 235.0],
                Place::Field => { let row = ((x as f32 * 0.04 + y as f32 * 0.35).sin() * 0.5 + 0.5) * 22.0; [60.0 + row, 130.0 + row, 50.0] }
                Place::Roof if v < 0.62 => [150.0 + 50.0 * v, 170.0 + 30.0 * v, 215.0],
                Place::Roof => [60.0, 62.0, 78.0],
                Place::Lot if v < 0.5 => [120.0 + 50.0 * v, 165.0 + 30.0 * v, 230.0],
                Place::Lot => { let line = if (x / 40) % 2 == 0 && y % 30 < 2 { 90.0 } else { 0.0 }; [100.0 + line, 100.0 + line, 104.0 + line] }
            };
            g.set(x as i32, y as i32, c);
        }
    }
    match place {
        Place::Roof => { // trees, and the car the dancer stands on
            for k in 0..5 { let cx = 20.0 + k as f32 * 72.0; g.disc(cx, 95.0, 24.0, [150.0, 150.0, 60.0]); g.rect(cx - 3.0, 100.0, cx + 3.0, 112.0, [90.0, 70.0, 50.0]); }
            g.rect(90.0, 128.0, 230.0, 150.0, [35.0, 35.0, 48.0]);
            g.rect(115.0, 115.0, 205.0, 130.0, [45.0, 48.0, 62.0]);
        }
        Place::Lot => { g.rect(0.0, 80.0, W as f32, 90.0, [150.0, 110.0, 90.0]); }   // a low brick building
        Place::Field => { for k in 0..8 { let cx = k as f32 * 44.0 + 10.0; g.disc(cx, 99.0, 9.0, [40.0, 100.0, 40.0]); } }
    }
}

/// The dancer at `x`, feet at `feet`, scale `s`; arms swing with phase `ph`.
fn dancer(g: &mut Img, x: f32, feet: f32, s: f32, ph: f64) {
    let white = [245.0, 245.0, 240.0];
    let skin = [215.0, 175.0, 145.0];
    let hip = (x, feet - 34.0 * s);
    let neck = (x, feet - 62.0 * s);
    g.line(hip, (x - 8.0 * s, feet), 3.2 * s, white);
    g.line(hip, (x + 8.0 * s + 3.0 * s * ph.sin() as f32, feet), 3.2 * s, white);
    g.line(hip, neck, 5.0 * s, white);
    let (a1, a2) = (ph.sin() as f32 * 1.2 - 0.6, (ph * 1.3 + 1.0).cos() as f32 * 1.1 + 0.6);
    g.line(neck, (x - 20.0 * s * a1.cos(), neck.1 - 20.0 * s * a1.sin()), 2.6 * s, white);
    g.line(neck, (x + 20.0 * s * a2.cos(), neck.1 - 20.0 * s * a2.sin()), 2.6 * s, white);
    g.disc(x, neck.1 - 9.0 * s, 7.5 * s, skin);
    g.rect(x - 6.0 * s, neck.1 - 17.0 * s, x + 6.0 * s, neck.1 - 13.0 * s, [60.0, 40.0, 30.0]);
}

/// The arm phase at `t` in `shot`: a steady dance, surging in the last 0.35 s
/// of shots marked on_move -- the cut rides out on the movement.
fn phase(t: f64, shot: &Shot) -> f64 {
    let base = t * 5.0;
    if shot.on_move && t > shot.end - 0.35 {
        let u = t - (shot.end - 0.35);
        base + u * u * 60.0          // accelerating: ~4x the arm speed at the cut
    } else { base }
}

fn frame(t: f64, all: &[Shot]) -> Vec<u8> {
    let s = all.iter().find(|s| t >= s.start && t < s.end).unwrap_or(all.last().unwrap());
    let mut g = Img::new();
    background(&mut g, s.place);
    let ph = phase(t, s);
    let feet = match s.place { Place::Roof => 118.0, Place::Field => 150.0, Place::Lot => 160.0 };
    match s.fx {
        Fx::Clone => { for (k, x) in [90.0f32, 160.0, 230.0].iter().enumerate() { dancer(&mut g, *x, feet, 1.1, ph + k as f64 * 0.7); } }
        Fx::Close => dancer(&mut g, 160.0, 205.0, 2.3, ph),
        _ => dancer(&mut g, 160.0, feet, 1.2, ph),
    }
    if s.look == Look::Pink { for p in g.px.chunks_mut(3) { let o = pink([p[0] as f32, p[1] as f32, p[2] as f32]); for k in 0..3 { p[k] = o[k].clamp(0.0, 255.0) as u8; } } }
    let mut px = g.px;
    if let Fx::Kaleido(n) = s.fx {
        px = studio_video::kaleido::apply(&px, W as u32, H as u32, &studio_video::kaleido::Kaleido { segments: n, angle: (t * 20.0) as f32, cx: 0.5, cy: 0.5 });
    }
    // the credits: a fade from the last shot to black, then a title bar on black
    let fade0 = all.last().unwrap().start;
    if t >= fade0 {
        let k = ((t - fade0) / 0.6).min(1.0) as f32;
        for p in px.iter_mut() { *p = (*p as f32 * (1.0 - k)) as u8; }
        if t > fade0 + 1.0 { for y in 85..95 { for x in 110..210 { px[(y * W + x) * 3..][..3].copy_from_slice(&[230, 230, 230]); } } }
    }
    px
}

/// The glitch: four frames of shuffled 16x16 blocks mixing the two shots.
fn glitch(a: &[u8], b: &[u8], k: usize) -> Vec<u8> {
    let mut out = vec![0u8; W * H * 3];
    let mut seed = 0x9E37u64 + k as u64;
    for by in 0..H / 12 { for bx in 0..W / 16 {
        seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17;
        let from = if (seed >> 20) % 4 < k as u64 + 1 { b } else { a };
        let (dx, dy) = (((seed >> 8) % 5) as i32 - 2, ((seed >> 12) % 3) as i32 - 1);
        for y in 0..12 { for x in 0..16 {
            let (tx, ty) = (bx * 16 + x, by * 12 + y);
            let sx = ((tx as i32 + dx * 16).rem_euclid(W as i32)) as usize;
            let sy = ((ty as i32 + dy * 12).rem_euclid(H as i32)) as usize;
            out[(ty * W + tx) * 3..][..3].copy_from_slice(&from[(sy * W + sx) * 3..][..3]);
        }}
    }}
    out
}

fn music(t: f64, seed: &mut u64) -> f32 {
    let bl = 60.0 / BPM;
    let k = ((t - BEAT0) / bl).floor();
    let tb = t - (BEAT0 + k * bl);
    let mut v = 0.0f32;
    if tb >= 0.0 {
        // kick on every beat, a little heavier on 1 and 3
        let acc = if (k as i64).rem_euclid(2) == 0 { 0.9 } else { 0.6 };
        v += acc * (-tb * 22.0).exp() as f32 * (std::f64::consts::TAU * (55.0 + 60.0 * (-tb * 40.0).exp()) * tb).sin() as f32;
        // hat on the off-beat
        let th = tb - bl / 2.0;
        if th >= 0.0 && th < 0.05 { *seed ^= *seed << 13; *seed ^= *seed >> 7; *seed ^= *seed << 17;
            v += 0.12 * (1.0 - th as f32 / 0.05) * (((*seed >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0); }
    }
    // a pad and a sung line (phrases of four beats, rests between)
    v += 0.05 * (std::f64::consts::TAU * 110.0 * t).sin() as f32;
    let phrase = ((t - BEAT0) / (bl * 4.0)).floor() as i64;
    if phrase.rem_euclid(2) == 0 && t < 21.8 {
        let notes = [196.0, 220.0, 247.0, 220.0];
        let f0 = notes[(((t - BEAT0) / bl) as usize) % 4];
        let syl = ((t * 4.0 * std::f64::consts::TAU).sin() * 0.5 + 0.5).powf(1.4) as f32;
        let ph = std::f64::consts::TAU * f0 * t;
        v += 0.18 * syl * (ph.sin() + 0.5 * (2.0 * ph).sin() + 0.25 * (3.0 * ph).sin()) as f32;
    }
    v * if t > 21.8 { ((24.0 - t) / 2.2).max(0.0) as f32 } else { 1.0 }
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "fixtures/cartoon-allmylife".into()));
    std::fs::create_dir_all(&out).unwrap();
    let all = shots();
    // audio
    let mut seed = 0xA11u64;
    let pcm: Vec<u8> = (0..(LEN * SR as f64) as usize).flat_map(|i| {
        ((music(i as f64 / SR as f64, &mut seed).clamp(-1.0, 1.0) * 30000.0) as i16).to_le_bytes() }).collect();
    let wav = out.join("audio.tmp.raw");
    std::fs::write(&wav, &pcm).unwrap();
    // picture
    let mp4 = out.join("allmylife-cartoon.mp4");
    let mut ff = Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &format!("{W}x{H}"),
        "-r", &format!("{FPS}"), "-i", "-", "-f", "s16le", "-ar", &SR.to_string(), "-ac", "1", "-i"]).arg(&wav)
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "30", "-pix_fmt", "yuv420p", "-g", "48",
               "-c:a", "aac", "-b:a", "40k", "-shortest", "-movflags", "+faststart"]).arg(&mp4)
        .stdin(Stdio::piped()).spawn().expect("ffmpeg");
    let mut si = ff.stdin.take().unwrap();
    let glitch_at = 13.60;
    let n = (LEN * FPS) as usize;
    for f in 0..n {
        let t = f as f64 / FPS;
        let gf = ((t - glitch_at) * FPS).round() as i64;
        let px = if (0..4).contains(&gf) {
            glitch(&frame(glitch_at - 1.0 / FPS, &all), &frame(glitch_at + 4.0 / FPS, &all), gf as usize)
        } else { frame(t, &all) };
        si.write_all(&px).unwrap();
    }
    drop(si);
    assert!(ff.wait().unwrap().success());
    let _ = std::fs::remove_file(&wav);
    // the truth
    let cuts: Vec<String> = all.iter().skip(1).map(|s| {
        let on_kick = (0..40).any(|k| (beat(k) - s.start).abs() < 0.5 / FPS);
        let kind = if (s.start - 21.8).abs() < 1e-9 { "fade" } else if (s.start - glitch_at).abs() < 1e-9 { "glitch" } else { "hard" };
        format!("    {{\"at\": {:.4}, \"frame\": {}, \"kind\": \"{kind}\", \"on_kick\": {on_kick}, \"on_move\": {}}}",
                s.start, (s.start * FPS).round() as usize, all.iter().find(|p| (p.end - s.start).abs() < 1e-9).map(|p| p.on_move).unwrap_or(false))
    }).collect();
    let shots_json: Vec<String> = all.iter().map(|s| format!(
        "    {{\"start\": {:.4}, \"end\": {:.4}, \"place\": \"{:?}\", \"look\": \"{:?}\", \"fx\": \"{:?}\"}}", s.start, s.end, s.place, s.look, s.fx)).collect();
    let kicks: Vec<String> = (0..40).map(beat).filter(|t| *t < 21.8).map(|t| format!("{t:.4}")).collect();
    let json = format!("{{\n  \"fps\": {FPS}, \"length\": {LEN}, \"bpm\": {BPM},\n  \"cuts\": [\n{}\n  ],\n  \"shots\": [\n{}\n  ],\n  \"kicks\": [{}]\n}}\n",
                       cuts.join(",\n"), shots_json.join(",\n"), kicks.join(", "));
    std::fs::write(out.join("truth.json"), json).unwrap();
    let size = std::fs::metadata(&mp4).unwrap().len();
    println!("{} ({} KB), {} shots, truth.json", mp4.display(), size / 1024, all.len());
}
