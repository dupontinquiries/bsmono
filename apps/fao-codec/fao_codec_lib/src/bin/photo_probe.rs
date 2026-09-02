//! photo_probe — run the photometric marker bank on real footage.
//!
//! Prediction = previous frame shifted by the phase-correlation
//! translation (integer), target = current frame. On engine footage
//! the bank was validated against one stage at a time; here nobody
//! knows the answer, which is the point.
//!
//!   photo_probe <clip.y4m> [every_n]

use fao_codec::v11_plate::{self as plate, Yuv420};

fn read_y4m(path: &str) -> (usize, usize, Vec<Yuv420>) {
    let data = std::fs::read(path).expect("read y4m");
    let hdr_end = data.iter().position(|&b| b == b'\n').expect("y4m header");
    let hdr = std::str::from_utf8(&data[..hdr_end]).unwrap();
    let (mut w, mut h) = (0usize, 0usize);
    for tok in hdr.split(' ') {
        if let Some(v) = tok.strip_prefix('W') { w = v.parse().unwrap(); }
        else if let Some(v) = tok.strip_prefix('H') { h = v.parse().unwrap(); }
    }
    let (ysz, csz) = (w * h, (w / 2) * (h / 2));
    let mut frames = Vec::new();
    let mut pos = hdr_end + 1;
    while pos + 6 <= data.len() {
        let fe = data[pos..].iter().position(|&b| b == b'\n').unwrap() + pos + 1;
        let mut f = Yuv420::new(w, h);
        f.y.copy_from_slice(&data[fe..fe + ysz]);
        f.u.copy_from_slice(&data[fe + ysz..fe + ysz + csz]);
        f.v.copy_from_slice(&data[fe + ysz + csz..fe + ysz + 2 * csz]);
        frames.push(f);
        pos = fe + ysz + 2 * csz;
    }
    (w, h, frames)
}

fn shift(f: &Yuv420, dx: i32, dy: i32) -> Yuv420 {
    let (w, h) = (f.w, f.h);
    let mut o = Yuv420::new(w, h);
    let at = |p: &[u8], pw: usize, ph: usize, x: i64, y: i64| -> u8 {
        p[(y.clamp(0, ph as i64 - 1) as usize) * pw + x.clamp(0, pw as i64 - 1) as usize]
    };
    for y in 0..h {
        for x in 0..w {
            o.y[y * w + x] = at(&f.y, w, h, x as i64 - dx as i64, y as i64 - dy as i64);
        }
    }
    let (cw, ch) = (w / 2, h / 2);
    for y in 0..ch {
        for x in 0..cw {
            o.u[y * cw + x] = at(&f.u, cw, ch, x as i64 - (dx / 2) as i64, y as i64 - (dy / 2) as i64);
            o.v[y * cw + x] = at(&f.v, cw, ch, x as i64 - (dx / 2) as i64, y as i64 - (dy / 2) as i64);
        }
    }
    o
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (_, _, frames) = read_y4m(&args[0]);
    let every: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(4);
    let motions = plate::estimate_motions(&frames);
    println!("{}: {} frames, pred = prev shifted by phase-correlation translation", args[0], frames.len());
    for i in 1..frames.len() {
        if i % every != 1 && i != frames.len() - 1 {
            continue;
        }
        let dx = (motions[i].dx - motions[i - 1].dx).round() as i32;
        let dy = (motions[i].dy - motions[i - 1].dy).round() as i32;
        let pred = shift(&frames[i - 1], dx, dy);
        let sig = fao_codec::photometric::signature(&pred, &frames[i]);
        println!("  f{i:>2} shift ({dx:+},{dy:+}) {}", sig.row());
    }
}
