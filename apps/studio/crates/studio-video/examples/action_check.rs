//! Does `action::accents` find the moments an editor actually cut on?
//!
//!     cargo run -p studio-video --example action_check -- VIDEO CUTS_CSV
//!
//! CUTS_CSV is the cut times in seconds, comma-separated. For each shot, the
//! accents are found and the shot's real out-point is compared with them; the
//! same is done for random points in the same shots as the baseline.
use studio_video::action::{accents, body_motion, cut_windows};
use studio_video::framepipe::{Pipe, Size};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cuts: Vec<f64> = a[1].split(',').map(|s| s.trim().parse().unwrap()).collect();
    let fps = 30.0;
    let pipe = Pipe { input: a[0].clone().into(), range: None, size: Size { w: 160, h: 90 }, fps };
    let frames = pipe.gray().expect("decodes");
    let motion = body_motion(&frames, 10);
    let bounds: Vec<usize> = std::iter::once(0).chain(cuts.iter().map(|c| (c * fps).round() as usize)).collect();
    // how far before each out-point is the nearest accent (an accent AT or just
    // before the cut = riding out on the action)
    let (mut real, mut rand_d) = (Vec::new(), Vec::new());
    let mut seed = 12345u64;
    let mut rnd = |n: usize| { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; (seed % n.max(1) as u64) as usize };
    for w in bounds.windows(2) {
        let (s, e) = (w[0], w[1]);
        if e <= s + 12 { continue; }
        // judge the shot without its last 2 frames (the cut itself smears motion)
        let acc = accents(&motion[s..e - 2], fps, 1.3, 0.3);
        if acc.is_empty() { continue; }
        let dist = |t: usize| acc.iter().map(|a| (t as i64 - (s + a.frame) as i64).abs()).min().unwrap() as f64 / fps;
        real.push(dist(e - 2));
        for _ in 0..50 { rand_d.push(dist(s + 6 + rnd(e - s - 8))); }
    }
    let med = |v: &mut Vec<f64>| { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[v.len() / 2] };
    let within = |v: &[f64], x: f64| v.iter().filter(|d| **d <= x).count() as f64 / v.len() as f64;
    println!("{} shots with accents", real.len());
    println!("out-point -> nearest accent: median {:.2}s (random points in the same shots: {:.2}s)",
             med(&mut real.clone()), med(&mut rand_d.clone()));
    println!("within 0.25 s of an accent: real cuts {:.0}% | random {:.0}%", within(&real, 0.25) * 100.0, within(&rand_d, 0.25) * 100.0);

    // rising windows: is the real out-point INSIDE a window (from take-off to peak)?
    let (mut hits, mut total, mut rhits, mut rtotal) = (0, 0, 0, 0);
    let mut seed2 = 999u64;
    let mut rnd2 = |n: usize| { seed2 ^= seed2 << 13; seed2 ^= seed2 >> 7; seed2 ^= seed2 << 17; (seed2 % n.max(1) as u64) as usize };
    for w in bounds.windows(2) {
        let (s, e) = (w[0], w[1]);
        if e <= s + 12 { continue; }
        let win = cut_windows(&motion[s..e - 2], fps, 1.2);
        let inside = |t: usize| win.iter().any(|w| t + 2 >= s + w.start && t <= s + w.peak + 2);
        total += 1; if inside(e - 3) { hits += 1; }
        for _ in 0..50 { rtotal += 1; if inside(s + 6 + rnd2(e - s - 9)) { rhits += 1; } }
    }
    println!("out-point inside a rising window: real cuts {}/{} = {:.0}% | random points {:.0}%",
             hits, total, 100.0 * hits as f64 / total as f64, 100.0 * rhits as f64 / rtotal as f64);
}
