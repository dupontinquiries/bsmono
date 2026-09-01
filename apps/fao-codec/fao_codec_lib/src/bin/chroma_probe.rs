//! chroma_probe — where does a plate's chroma diverge from its frames?
//!
//! Weighted prediction (v11_plate::WeightedPred) fitted U' = 0.80 U + 30
//! against the 2D plate on SMITE while the SOURCE chroma was flat
//! across the GOP (U 122.8 +- 11 every frame). So the plate, not the
//! footage, carries a global chroma error, and a correction that
//! removes it was worth 25% of the bytes. This bin locates the stage.
//!
//!   chroma_probe <clip.y4m>

use fao_codec::residual::{self, ResidualOpts};
use fao_codec::v11_plate as plate;

fn read_y4m(path: &str) -> (usize, usize, Vec<plate::Yuv420>) {
    let data = std::fs::read(path).expect("read y4m");
    let hdr_end = data.iter().position(|&b| b == b'\n').expect("y4m header");
    let hdr = std::str::from_utf8(&data[..hdr_end]).unwrap();
    let mut w = 0;
    let mut h = 0;
    for tok in hdr.split(' ') {
        if let Some(v) = tok.strip_prefix('W') {
            w = v.parse().unwrap();
        } else if let Some(v) = tok.strip_prefix('H') {
            h = v.parse().unwrap();
        }
    }
    let ysz = w * h;
    let csz = (w / 2) * (h / 2);
    let mut frames = Vec::new();
    let mut pos = hdr_end + 1;
    while pos + 6 <= data.len() {
        let fe = data[pos..].iter().position(|&b| b == b'\n').unwrap() + pos + 1;
        let mut f = plate::Yuv420::new(w, h);
        f.y.copy_from_slice(&data[fe..fe + ysz]);
        f.u.copy_from_slice(&data[fe + ysz..fe + ysz + csz]);
        f.v.copy_from_slice(&data[fe + ysz + csz..fe + ysz + 2 * csz]);
        frames.push(f);
        pos = fe + ysz + 2 * csz;
    }
    (w, h, frames)
}

fn stats(p: &[u8]) -> (f64, f64) {
    let n = p.len() as f64;
    let m = p.iter().map(|&v| v as f64).sum::<f64>() / n;
    let s = (p.iter().map(|&v| (v as f64 - m).powi(2)).sum::<f64>() / n).sqrt();
    (m, s)
}

fn fit(pred: &[u8], target: &[u8]) -> (f64, f64) {
    let n = pred.len() as f64;
    let (mut sp, mut st, mut spp, mut spt) = (0.0f64, 0.0, 0.0, 0.0);
    for (&p, &t) in pred.iter().zip(target) {
        let (p, t) = (p as f64, t as f64);
        sp += p;
        st += t;
        spp += p * p;
        spt += p * t;
    }
    let a = (n * spt - sp * st) / (n * spp - sp * sp);
    (a, (st - a * sp) / n)
}

fn report(name: &str, pred: &plate::Yuv420, f: &plate::Yuv420) {
    for (pn, pp, fp) in [("Y", &pred.y, &f.y), ("U", &pred.u, &f.u), ("V", &pred.v, &f.v)] {
        let (pm, ps) = stats(pp);
        let (fm, fs) = stats(fp);
        let (a, b) = fit(pp, fp);
        println!(
            "{name:<26} {pn}: pred {pm:6.1}+-{ps:5.1}  frame {fm:6.1}+-{fs:5.1}  fit a {a:.3} b {b:+.1}"
        );
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("clip.y4m");
    let (w, h, frames) = read_y4m(&path);
    let n = frames.len().min(8);
    let frames = &frames[..n];
    let m = plate::estimate_motions(frames);
    let pl = plate::build_plate(frames, &m);
    println!("plate {}x{} from {} frames of {w}x{h}", pl.w, pl.h, n);
    // Stage 1: the raw plate, warped to frame 0.
    let p0 = plate::predict_frame(&pl, &m[0], w, h);
    report("raw plate -> frame 0", &p0, &frames[0]);
    // Stage 2: the plate as the encoder codes it (residual against
    // gray at dz/2), warped to frame 0.
    let p444 = plate::interleave_444(&plate::Yuv420 { w: pl.w, h: pl.h, y: pl.y.clone(), u: pl.u.clone(), v: pl.v.clone() });
    let gray = vec![128u8; pl.w * pl.h * 3];
    let opts = ResidualOpts { dead_zone: 16.0, ..ResidualOpts::default() };
    let t = residual::encode(&gray, &p444, pl.w, pl.h, &opts).expect("plate encode");
    let dec = plate::deinterleave_420(&residual::apply(gray, &t, pl.w, pl.h), pl.w, pl.h);
    println!("coded plate {} B", t.len());
    let coded = plate::Plate { w: pl.w, h: pl.h, ox: pl.ox, oy: pl.oy, y: dec.y, u: dec.u, v: dec.v };
    let c0 = plate::predict_frame(&coded, &m[0], w, h);
    report("coded plate -> frame 0", &c0, &frames[0]);
    // Stage 3: the round trip alone, no plate, no warp — frame 0
    // through interleave/encode/apply/deinterleave against gray.
    let f444 = plate::interleave_444(&frames[0]);
    let gray = vec![128u8; w * h * 3];
    let t = residual::encode(&gray, &f444, w, h, &opts).expect("frame encode");
    let rt = plate::deinterleave_420(&residual::apply(gray, &t, w, h), w, h);
    report("frame 0 coded vs gray", &rt, &frames[0]);
    // Stage 4: interleave/deinterleave only.
    let rt2 = plate::deinterleave_420(&f444, w, h);
    report("444 round trip only", &rt2, &frames[0]);
}
