//! cursor_demo <pupils.txt> <fps>: turn `pupil_frames` output (one pupil per frame) into cursor motion and print CSV:
//! `t, raw_x, raw_y, filtered_x, filtered_y, disc_x, disc_y` in screen fractions.
//!
//! The calibration here is a stand-in: the 5th-95th percentile box of where the pupil went is mapped onto the middle
//! 90% of the screen (a real session fits `Affine` from points the person looked at). `disc` is the iPadOS-style cursor
//! as tideshell-core's `FatCursor` eases it: an exponential approach to its target at RATE 22 per second.
use gaze_kit::{OneEuro2D, OneEuroParams, PupilGate, Verdict};
use orlosky_kit::Pupil;

const FAT_CURSOR_RATE: f64 = 22.0;

fn percentile(v: &mut Vec<f64>, p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p) as usize]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: cursor_demo <pupils.txt> <fps>");
    let fps: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(12.0);
    // one entry per frame: the pupil, or None when the detector found nothing
    let mut frames: Vec<Option<Pupil>> = Vec::new();
    for l in std::fs::read_to_string(path).unwrap().lines() {
        let t: Vec<&str> = l.split_whitespace().collect();
        let v: Vec<f64> = t.iter().skip(1).filter_map(|x| x.parse().ok()).collect();
        frames.push(if v.len() >= 4 { Some(Pupil { cx: v[0], cy: v[1], major: v[2], minor: v[3], angle_deg: v.get(4).copied().unwrap_or(0.0), area: 0, darkest: (0, 0), threshold: 0 }) } else { None });
    }
    // the gate: refuse blinks and lid highlights, and hold the last good point through them
    let mut gate = PupilGate::new();
    let mut pts: Vec<(f64, f64)> = Vec::new();
    let mut refused = 0usize;
    for fr in &frames {
        match fr.map(|p| (p, gate.judge(&p, 640.0))) {
            Some((p, Verdict::Accept)) => pts.push((p.cx, p.cy)),
            _ => { refused += 1; if let Some(last) = pts.last().copied() { pts.push(last); } }
        }
    }
    eprintln!("{} frames, {} refused by the gate", frames.len(), refused);
    let (mut xs, mut ys): (Vec<f64>, Vec<f64>) = pts.iter().cloned().unzip();
    let (x0, x1, y0, y1) = (percentile(&mut xs, 0.05), percentile(&mut xs, 0.95), percentile(&mut ys, 0.05), percentile(&mut ys, 0.95));
    let map = |p: (f64, f64)| (0.05 + 0.9 * ((p.0 - x0) / (x1 - x0)).clamp(0.0, 1.0), 0.05 + 0.9 * ((p.1 - y0) / (y1 - y0)).clamp(0.0, 1.0));
    let mut f = OneEuro2D::new(OneEuroParams::default());
    let mut disc: Option<(f64, f64)> = None;
    let dt = 1.0 / fps;
    println!("t,raw_x,raw_y,filtered_x,filtered_y,disc_x,disc_y");
    for (i, p) in pts.iter().enumerate() {
        let t = i as f64 * dt;
        let raw = map(*p);
        let fl = f.step(t, raw);
        // the disc eases after the filtered point; run the ease at a finer step than the camera so it reads as it would at 60 Hz
        let steps = (dt * 60.0).round().max(1.0) as usize;
        let mut d = disc.unwrap_or(fl);
        let mut rows = Vec::new();
        for k in 1..=steps {
            let h = dt / steps as f64;
            let keep = (-FAT_CURSOR_RATE * h).exp();
            d = (fl.0 + (d.0 - fl.0) * keep, fl.1 + (d.1 - fl.1) * keep);
            rows.push((t + k as f64 * h, d));
        }
        disc = Some(d);
        for (tt, dd) in rows { println!("{tt:.4},{:.5},{:.5},{:.5},{:.5},{:.5},{:.5}", raw.0, raw.1, fl.0, fl.1, dd.0, dd.1); }
    }
}
