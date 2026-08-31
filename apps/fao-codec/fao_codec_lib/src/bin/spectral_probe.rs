//! spectral_probe — can the codec TELL, cheaply, which representation
//! a piece of content wants?
//!
//! The measured pattern across this corpus is that a model-based coder
//! wins where pixel statistics are hostile and loses where they are
//! friendly, and that provenance does not predict which: two
//! engine-rendered clips landed on opposite sides (procedural fBm
//! ground, v11 -23% bytes vs AV1; soldier's flat-shaded geometry, AV1
//! 2.1x smaller). If that is a real law rather than a story fitted to
//! four data points, a cheap statistic computed BEFORE encoding should
//! separate them, and a router could use it.
//!
//! Two axes, both computed from quantities the encoder already touches:
//!
//! * SPATIAL HF FRACTION — share of 4x4 DCT energy outside the DC and
//!   first two AC coefficients, averaged over blocks. Low on flat
//!   shading and smooth gradients; high on noise and texture. This is
//!   what decides whether a conventional coder's big transforms and
//!   intra prediction get the block nearly free.
//!
//! * TEMPORAL PREDICTABILITY — mean absolute frame difference after
//!   compensating the best whole-frame integer shift, normalised by the
//!   frame's own spatial activity. Low when motion compensation works;
//!   high when it cannot, which is the perspective-motion case where a
//!   static pattern has no matching block in the previous frame.
//!
//! The hypothesis this exists to test: a model-based representation
//! wins when HF is HIGH and temporal predictability is LOW, because
//! that is precisely the corner where a block coder must transcribe
//! pixels and a world model does not. Anything else and the
//! conventional path should win.
//!
//! Usage: spectral_probe <file.y4m> [more.y4m ...]

use fao_codec::transform::dct2d_4x4;

fn read_y4m(path: &str) -> (usize, usize, Vec<Vec<u8>>) {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let hdr_end = data.iter().position(|&b| b == b'\n').expect("y4m header");
    let hdr = String::from_utf8_lossy(&data[..hdr_end]).to_string();
    let (mut w, mut h) = (0usize, 0usize);
    for tok in hdr.split_whitespace() {
        match tok.as_bytes().first() {
            Some(b'W') => w = tok[1..].parse().unwrap_or(0),
            Some(b'H') => h = tok[1..].parse().unwrap_or(0),
            _ => {}
        }
    }
    let fsz = w * h * 3 / 2;
    let mut off = hdr_end + 1;
    let mut frames = Vec::new();
    while off < data.len() {
        let Some(rel) = data[off..].iter().position(|&b| b == b'\n') else { break };
        off += rel + 1;
        if off + fsz > data.len() {
            break;
        }
        frames.push(data[off..off + w * h].to_vec());
        off += fsz;
    }
    (w, h, frames)
}

fn hf_fraction(w: usize, h: usize, f: &[u8]) -> f32 {
    let (mut hf, mut tot) = (0.0f64, 0.0f64);
    let mut buf = [0f32; 16];
    let (bw, bh) = (w / 4, h / 4);
    for by in 0..bh {
        for bx in 0..bw {
            for ly in 0..4 {
                for lx in 0..4 {
                    buf[ly * 4 + lx] = f[(by * 4 + ly) * w + bx * 4 + lx] as f32 - 128.0;
                }
            }
            dct2d_4x4(&mut buf);
            for (k, c) in buf.iter().enumerate() {
                let e = (*c as f64) * (*c as f64);
                // k = 0 is DC; 1 and 4 are the two lowest AC terms. A
                // block a conventional coder gets nearly free is one
                // whose energy lives entirely in those three.
                if k != 0 {
                    tot += e;
                    if k != 1 && k != 4 {
                        hf += e;
                    }
                }
            }
        }
    }
    if tot < 1e-9 {
        0.0
    } else {
        (hf / tot) as f32
    }
}

/// Mean |difference| after the best whole-frame integer shift, over the
/// frame's own mean |spatial gradient|. Normalising by spatial activity
/// is what makes the number comparable across clips: a busy frame has
/// large differences for trivial reasons.
fn temporal_cost(w: usize, h: usize, a: &[u8], b: &[u8]) -> f32 {
    let mut best = f64::MAX;
    let step = 4i32;
    let range = 16i32;
    let mut dy = -range;
    while dy <= range {
        let mut dx = -range;
        while dx <= range {
            let (mut acc, mut n) = (0.0f64, 0u64);
            let mut y = 8usize;
            while y < h - 8 {
                let mut x = 8usize;
                while x < w - 8 {
                    let sy = (y as i32 + dy).clamp(0, h as i32 - 1) as usize;
                    let sx = (x as i32 + dx).clamp(0, w as i32 - 1) as usize;
                    acc += (b[y * w + x] as i32 - a[sy * w + sx] as i32).abs() as f64;
                    n += 1;
                    x += 3;
                }
                y += 3;
            }
            let m = acc / n.max(1) as f64;
            if m < best {
                best = m;
            }
            dx += step;
        }
        dy += step;
    }
    let (mut g, mut gn) = (0.0f64, 0u64);
    for y in 1..h {
        let mut x = 1usize;
        while x < w {
            g += (b[y * w + x] as i32 - b[y * w + x - 1] as i32).abs() as f64;
            g += (b[y * w + x] as i32 - b[(y - 1) * w + x] as i32).abs() as f64;
            gn += 2;
            x += 3;
        }
    }
    let ga = (g / gn.max(1) as f64).max(1e-6);
    (best / ga) as f32
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: spectral_probe <file.y4m> [more.y4m ...]");
        std::process::exit(2);
    }
    println!("{:<22} {:>8} {:>10} {:>8} {:>12}", "file", "HF frac", "temporal", "hf*tc", "verdict");
    for path in &args {
        let (w, h, frames) = read_y4m(path);
        if frames.len() < 2 {
            eprintln!("{path}: need >=2 frames");
            continue;
        }
        let take = frames.len().min(12);
        let hf: f32 =
            frames[..take].iter().map(|f| hf_fraction(w, h, f)).sum::<f32>() / take as f32;
        let tc: f32 = (1..take)
            .map(|i| temporal_cost(w, h, &frames[i - 1], &frames[i]))
            .sum::<f32>()
            / (take - 1) as f32;
        // The PRODUCT, not two thresholds. Measured on five clips with
        // known v11-vs-AV1 outcomes, hf*tc ranks them in exactly the
        // order the rate comparisons did:
        //   ground   2.77  v11 WINS by 23%
        //   soldier  1.63  AV1 2.1x
        //   w1 film  0.38  AV1 ~18x
        //   SMITE    0.29  AV1 ~15x
        //   blackstar 0.08 AV1 43x
        // So the crossover on this corpus is BRACKETED between 1.63 and
        // 2.77 and cannot be located more precisely from five points.
        // Reported as an ordering statistic with an interval, not as a
        // threshold to trust: n=5, and every clip is a single GOP.
        let score = hf * tc;
        let verdict = if score >= 2.77 {
            "MODEL"
        } else if score > 1.63 {
            "uncertain"
        } else {
            "conventional"
        };
        let name = path.rsplit('/').next().unwrap_or(path);
        println!(
            "{:<22} {:>8.3} {:>10.3} {:>8.2} {:>12}",
            name, hf, tc, score, verdict
        );
    }
    println!(
        "\nHF frac: share of 4x4 DCT energy above the lowest two AC terms.\n\
         temporal: post-global-shift frame difference / spatial activity.\n\
         hf*tc ordered five clips exactly as their measured v11-vs-AV1\n\
         outcomes did; the crossover is bracketed in [1.63, 2.77]."
    );
}
