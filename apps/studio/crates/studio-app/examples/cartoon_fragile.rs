//! A tiny synthetic "A Fragile Thing": the rooftop, in basic shapes.
//!
//!     cargo run -p studio-app --example cartoon_fragile -- OUT_DIR
//!
//! Built to exercise Studio's film tools without real footage (and without
//! storage cost -- the whole set is under a megabyte):
//!
//! * four shape characters on a couch -- two "women" left, two "men" right --
//!   and a lamp, at night;
//! * three cameras that roll at different moments (wide at scene 0 s, close
//!   left at 0.4 s, close right at 0.9 s) and a recorder that rolls 1.5 s
//!   before them all: sync by audio (dualsync), camera groups, multicam;
//! * a slate and a clap at scene 1.0 s in every mic;
//! * synthetic voices (a harmonic buzz with a syllable rhythm, lower for the
//!   men) LOUDER on the nearer camera's mic: who-is-speaking, comic sides;
//! * four dialogue lines with SRT transcripts beside each camera file, the
//!   last one the women's arrival: transcript gutter, insert by content,
//!   comic auto mode, signal lanes;
//! * a hand gesture (5-7 s) for follow-motion and track points; a short
//!   camera shake on the wide (7.5-8.5 s) for the stabilizer gate; a quiet
//!   kick drum for the kick lane;
//! * `fragile-cartoon.studio`: the three angles as a camera group, the
//!   recorder, a Mix bus feeding a silent Control channel, and markers.
//!
//! Deterministic: the same files every run.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const WW: usize = 640;       // world
const WH: usize = 360;
const W: usize = 320;        // every camera
const H: usize = 180;
const FPS: f64 = 24.0;
const SR: u32 = 16_000;
const SCENE: f64 = 14.0;

#[derive(Clone, Copy)]
struct Person { x: f32, color: [u8; 3], f0: f32, left: bool }
const PEOPLE: [Person; 4] = [
    Person { x: 120.0, color: [230, 120, 170], f0: 215.0, left: true },   // "woman 1"
    Person { x: 205.0, color: [170, 120, 220], f0: 235.0, left: true },   // "woman 2"
    Person { x: 410.0, color: [80, 190, 190], f0: 118.0, left: false },   // "Max"
    Person { x: 495.0, color: [230, 150, 70], f0: 128.0, left: false },   // "Jude"
];
/// (start, end, speaker, text), scene time.
const LINES: [(f64, f64, usize, &str); 4] = [
    (2.0, 4.2, 2, "Yeah, I found something I like more than that."),
    (5.0, 7.0, 3, "You like acting, come on."),
    (8.0, 9.2, 2, "Partying is great."),
    (10.5, 12.5, 0, "Hi guys, mind if we join the party?"),
];

fn speaking(p: usize, t: f64) -> Option<f64> {
    LINES.iter().find(|l| l.2 == p && t >= l.0 && t < l.1).map(|l| t - l.0)
}
/// A syllable rhythm: ~4.5 syllables a second, deterministic, never quite regular.
fn syllable(u: f64) -> f32 {
    let s = (u * 4.5 * std::f64::consts::TAU).sin() * 0.5 + 0.5;
    let wobble = (u * 1.7 * std::f64::consts::TAU).sin() * 0.2 + 0.8;
    (s * wobble).powf(1.5) as f32
}

struct Img { px: Vec<u8>, w: usize, h: usize }
impl Img {
    fn new(w: usize, h: usize, c: [u8; 3]) -> Img { Img { px: c.iter().copied().cycle().take(w * h * 3).collect(), w, h } }
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: [u8; 3]) {
        for y in (y0.max(0.0) as usize)..(y1.min(self.h as f32) as usize) {
            for x in (x0.max(0.0) as usize)..(x1.min(self.w as f32) as usize) { self.px[(y * self.w + x) * 3..][..3].copy_from_slice(&c); }
        }
    }
    fn circle(&mut self, cx: f32, cy: f32, r: f32, c: [u8; 3]) {
        for y in ((cy - r).max(0.0) as usize)..((cy + r).min(self.h as f32) as usize) {
            for x in ((cx - r).max(0.0) as usize)..((cx + r).min(self.w as f32) as usize) {
                if (x as f32 - cx).hypot(y as f32 - cy) <= r { self.px[(y * self.w + x) * 3..][..3].copy_from_slice(&c); }
            }
        }
    }
}

/// The world at scene time `t`.
fn world(t: f64) -> Img {
    let mut g = Img::new(WW, WH, [14, 18, 38]);
    g.rect(0.0, 150.0, WW as f32, 154.0, [60, 64, 80]);                 // the railing
    for k in 0..24 { let x = (k * 97 % WW) as f32; let y = (k * 53 % 130) as f32 + 8.0; g.rect(x, y, x + 2.0, y + 2.0, [200, 200, 170]); }
    g.rect(60.0, 205.0, 580.0, 300.0, [70, 74, 86]);                     // the couch
    g.rect(300.0, 250.0, 345.0, 300.0, [95, 90, 80]);                    // the table
    g.rect(313.0, 225.0, 332.0, 250.0, [240, 210, 120]);                 // the lamp
    for (i, p) in PEOPLE.iter().enumerate() {
        // the women turn toward whoever speaks: a head shift, not a mouth
        let turn = if p.left && t > 2.5 && t < 9.5 { 7.0 * ((t * 0.9).sin() as f32 * 0.5 + 0.5) } else { 0.0 };
        let arrive = if p.left && t >= 10.3 { 10.0 } else { 0.0 };
        let talk = speaking(i, t).map(syllable).unwrap_or(0.0);
        let bob = talk * 2.0;
        g.rect(p.x - 26.0, 165.0 + bob, p.x + 26.0, 245.0, p.color);    // body
        let hx = p.x + turn + arrive;
        g.circle(hx, 138.0 + bob, 22.0, [p.color[0].saturating_add(20), p.color[1].saturating_add(20), p.color[2].saturating_add(20)]);
        g.rect(hx - 9.0, 128.0 + bob, hx - 5.0, 132.0 + bob, [20, 20, 30]);   // eyes
        g.rect(hx + 5.0, 128.0 + bob, hx + 9.0, 132.0 + bob, [20, 20, 30]);
        g.rect(hx - 7.0, 146.0 + bob, hx + 7.0, 146.0 + bob + 2.0 + talk * 7.0, [30, 10, 20]);  // mouth
    }
    // Jude's hand: a gesture while he talks (5-7 s)
    let gest = if t >= 5.0 && t < 7.0 { ((t - 5.0) * 3.0).sin().abs() as f32 } else { 0.0 };
    g.circle(540.0 + gest * 10.0, 205.0 - gest * 55.0, 9.0, [240, 190, 150]);
    // the slate: 0.3-1.2 s, the stick closing at 1.0 s
    if (0.3..1.2).contains(&t) {
        g.rect(250.0, 120.0, 390.0, 210.0, [235, 235, 235]);
        let open = if t < 1.0 { ((1.0 - t) * 60.0) as f32 } else { 0.0 };
        g.rect(250.0, 104.0 - open, 390.0, 118.0 - open, [30, 30, 30]);
        for k in 0..5 { let x = 255.0 + k as f32 * 27.0; g.rect(x, 106.0 - open, x + 12.0, 116.0 - open, [235, 235, 235]); }
        g.rect(262.0, 140.0, 378.0, 146.0, [40, 40, 40]);
        g.rect(262.0, 165.0, 330.0, 171.0, [40, 40, 40]);
    }
    g
}

/// A camera: which part of the world it sees, and when it rolled.
struct Cam { name: &'static str, x0: usize, y0: usize, scale: usize, roll: f64, shake: bool, gains: [f32; 4], music: f32, noise: f32 }

fn view(wd: &Img, c: &Cam, t: f64) -> Vec<u8> {
    // the wide's loose tripod: a shake from 7.5 to 8.5 s
    let (sx, sy) = if c.shake && (7.5..8.5).contains(&t) {
        (((t * 37.0).sin() * 4.0) as isize, ((t * 29.0).cos() * 3.0) as isize)
    } else { (0, 0) };
    let mut out = vec![0u8; W * H * 3];
    for y in 0..H { for x in 0..W {
        let wx = (c.x0 as isize + (x * c.scale) as isize + sx).clamp(0, WW as isize - 1) as usize;
        let wy = (c.y0 as isize + (y * c.scale) as isize + sy).clamp(0, WH as isize - 1) as usize;
        out[(y * W + x) * 3..][..3].copy_from_slice(&wd.px[(wy * WW + wx) * 3..][..3]);
    }}
    out
}

/// One mic's sound at scene time `t`: voices by distance, the clap, the kick, noise.
fn mic(t: f64, gains: [f32; 4], music: f32, noise: f32, seed: &mut u64) -> f32 {
    let mut v = 0.0f32;
    for (i, p) in PEOPLE.iter().enumerate() {
        if let Some(u) = speaking(i, t) {
            let env = syllable(u);
            let ph = std::f64::consts::TAU * p.f0 as f64 * t;
            let buzz = (ph.sin() + 0.6 * (2.0 * ph).sin() + 0.35 * (3.0 * ph).sin() + 0.2 * (5.0 * ph).sin()) as f32;
            v += gains[i] * 0.22 * env * buzz;
        }
    }
    let dc = t - 1.0;                                                    // the clap
    if (0.0..0.04).contains(&dc) { *seed ^= *seed << 13; *seed ^= *seed >> 7; *seed ^= *seed << 17;
        v += 0.7 * (1.0 - dc as f32 / 0.04) * (((*seed >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0); }
    let kt = t.rem_euclid(0.5);                                          // the kick, 120 bpm
    v += music * (-kt * 25.0).exp() as f32 * (std::f64::consts::TAU * 58.0 * kt).sin() as f32;
    *seed ^= *seed << 13; *seed ^= *seed >> 7; *seed ^= *seed << 17;
    v + noise * (((*seed >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0)
}

fn write_wav(path: &Path, samples: &[f32]) {
    let mut f = std::fs::File::create(path).unwrap();
    let data: Vec<u8> = samples.iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
    let mut h = Vec::new();
    h.extend(b"RIFF"); h.extend(&(36 + data.len() as u32).to_le_bytes()); h.extend(b"WAVEfmt ");
    h.extend(&16u32.to_le_bytes()); h.extend(&1u16.to_le_bytes()); h.extend(&1u16.to_le_bytes());
    h.extend(&SR.to_le_bytes()); h.extend(&(SR * 2).to_le_bytes()); h.extend(&2u16.to_le_bytes()); h.extend(&16u16.to_le_bytes());
    h.extend(b"data"); h.extend(&(data.len() as u32).to_le_bytes());
    f.write_all(&h).unwrap(); f.write_all(&data).unwrap();
}

fn srt_time(s: f64) -> String {
    let ms = (s.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "fixtures/cartoon-fragile".into()));
    std::fs::create_dir_all(&out).unwrap();
    let cams = [
        Cam { name: "A001_wide", x0: 0, y0: 0, scale: 2, roll: 0.0, shake: true, gains: [0.45, 0.45, 0.45, 0.45], music: 0.12, noise: 0.012 },
        Cam { name: "A002_close_left", x0: 40, y0: 90, scale: 1, roll: 0.4, shake: false, gains: [1.0, 1.0, 0.3, 0.3], music: 0.08, noise: 0.015 },
        Cam { name: "A003_close_right", x0: 300, y0: 90, scale: 1, roll: 0.9, shake: false, gains: [0.3, 0.3, 1.0, 1.0], music: 0.08, noise: 0.015 },
    ];
    for c in &cams {
        let len = SCENE - c.roll;
        let mut seed = 0x51ED_u64 + c.roll.to_bits();
        let audio: Vec<f32> = (0..(len * SR as f64) as usize).map(|i| mic(c.roll + i as f64 / SR as f64, c.gains, c.music, c.noise, &mut seed)).collect();
        let wav = out.join(format!("{}.tmp.wav", c.name));
        write_wav(&wav, &audio);
        let mp4 = out.join(format!("{}.mp4", c.name));
        let mut ff = Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &format!("{W}x{H}"),
            "-r", &format!("{FPS}"), "-i", "-", "-i"]).arg(&wav)
            .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "30", "-pix_fmt", "yuv420p", "-g", "48",
                   "-c:a", "aac", "-b:a", "40k", "-shortest", "-movflags", "+faststart"]).arg(&mp4)
            .stdin(Stdio::piped()).spawn().expect("ffmpeg");
        let mut si = ff.stdin.take().unwrap();
        for f in 0..(len * FPS) as usize {
            let t = c.roll + f as f64 / FPS;
            si.write_all(&view(&world(t), c, t)).unwrap();
        }
        drop(si);
        assert!(ff.wait().unwrap().success(), "encode {}", c.name);
        let _ = std::fs::remove_file(&wav);
        // what whisper would write beside the file: every line, in THIS file's time
        let mut srt = String::new();
        for (k, l) in LINES.iter().enumerate() {
            if l.1 <= c.roll { continue; }
            srt.push_str(&format!("{}\n{} --> {}\n{}\n\n", k + 1, srt_time(l.0 - c.roll), srt_time(l.1 - c.roll), l.3));
        }
        std::fs::write(out.join(format!("{}.srt", c.name)), srt).unwrap();
        println!("{}: rolled at {:.1} s, {:.1} s long", mp4.display(), c.roll, len);
    }
    // the recorder: rolled 1.5 s before the cameras, clean, everyone at one level
    let rec_roll = -1.5;
    let rec_len = SCENE + 1.5 + 0.5;
    let mut seed = 0xFEED_u64;
    let rec: Vec<f32> = (0..(rec_len * SR as f64) as usize).map(|i| mic(rec_roll + i as f64 / SR as f64, [0.8; 4], 0.05, 0.003, &mut seed)).collect();
    let wav = out.join("R001_recorder.tmp.wav");
    write_wav(&wav, &rec);
    let flac = out.join("R001_recorder.flac");
    assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-i"]).arg(&wav).args(["-c:a", "flac", "-compression_level", "8"]).arg(&flac)
        .status().unwrap().success());
    let _ = std::fs::remove_file(&wav);
    println!("{}: rolled at {rec_roll:.1} s, {rec_len:.1} s long", flac.display());

    // the project: everything in sync on the timeline (scene 0 s at timeline 2 s)
    use studio_core::*;
    let base = 2.0;
    let mut p = Project { name: "fragile (cartoon)".into(), ..Default::default() };
    p.apply(&Op::AddBus { bus: Bus { id: BusId(1), name: "Mix".into(), parent: None, gain: 1.0, chain: vec![],
        sends: vec![Send { to: BusId(2), level: 1.0, pre_fader: true }], muted: false, silent: false } }).unwrap();
    p.apply(&Op::AddBus { bus: Bus { id: BusId(2), name: "Control".into(), parent: None, gain: 1.0, chain: vec![],
        sends: vec![], muted: false, silent: true } }).unwrap();
    for (i, c) in cams.iter().enumerate() {
        let id = i as u64 + 1;
        let len = Ticks::from_seconds(SCENE - c.roll);
        p.apply(&Op::AddTrack { track: Track { id: TrackId(id), kind: TrackKind::Video, name: c.name.into(), muted: false,
            locked: false, gain: 1.0, route: Some(BusId(1)) }, at: i }).unwrap();
        // SR, not 48 kHz: the cameras' audio is encoded at SR, and a wrong rate
        // here made every waveform read the wrong span of samples
        p.apply(&Op::AddSource { source: Source::file(SourceId(id), format!("{}.mp4", c.name), len, SR, 1, true) }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(id), track: TrackId(id),
            pos: Ticks::from_seconds(base + c.roll), src_in: Ticks::ZERO, len, gain: 1.0, effects: vec![] } }).unwrap();
    }
    let rl = Ticks::from_seconds(rec_len);
    p.apply(&Op::AddTrack { track: Track { id: TrackId(4), kind: TrackKind::Audio, name: "recorder".into(), muted: true,
        locked: false, gain: 1.0, route: Some(BusId(1)) }, at: 3 }).unwrap();
    p.apply(&Op::AddSource { source: Source::file(SourceId(4), "R001_recorder.flac", rl, SR, 1, false) }).unwrap();
    p.apply(&Op::AddClip { clip: Clip { id: ClipId(4), source: SourceId(4), track: TrackId(4),
        pos: Ticks::from_seconds(base + rec_roll), src_in: Ticks::ZERO, len: rl, gain: 1.0, effects: vec![] } }).unwrap();
    p.apply(&Op::SetGroup { id: GroupId(1), name: "rooftop angles".into(), clips: vec![ClipId(1), ClipId(2), ClipId(3)] }).unwrap();
    for (at, name) in [(base, "N7: rooftop (cartoon)"), (base + 1.0, "check: slate clap at 1.0 s"), (base + 10.5, "no cut — the girls arrive")] {
        p.apply(&Op::SetMarker { at: Ticks::from_seconds(at), name: name.into() }).unwrap();
    }
    let f = studio_collab::file::SaveFile::from_project(&p, Vec::new(), None);
    std::fs::write(out.join("fragile-cartoon.studio"), f.encode()).unwrap();
    let total: u64 = std::fs::read_dir(&out).unwrap().filter_map(|e| e.ok()).filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum();
    println!("fragile-cartoon.studio written; the set is {:.0} KB", total as f64 / 1024.0);
}
