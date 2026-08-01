//! Fold a body into a real vehicle's volume.
use model_verify::fold::*;
use std::{env, fs};

fn load_points(p: &str) -> Vec<[f64; 3]> {
    let b = fs::read(p).unwrap_or_default();
    if b.len() < 64 || &b[..15] != b"INTERQUAKEMODEL" { return vec![]; }
    let g = |o: usize| u32::from_le_bytes(b[o..o+4].try_into().unwrap()) as usize;
    let (nva, nv, ofs_va) = (g(44), g(48), g(52));
    for i in 0..nva {
        let e = ofs_va + i*20;
        let (ty, fmt, size, off) = (g(e), g(e+8), g(e+12), g(e+16));
        if ty == 0 && fmt == 7 {
            return (0..nv).map(|k| {
                let o = off + k*size*4;
                let f = |j: usize| f32::from_le_bytes(b[o+j*4..o+j*4+4].try_into().unwrap()) as f64;
                [f(0), f(1), f(2)]
            }).collect();
        }
    }
    vec![]
}

fn main() {
    let path = env::args().nth(1).expect("usage: fit_vehicle MODEL.iqm");
    let pts = load_points(&path);
    if pts.is_empty() { println!("no geometry"); return; }
    let vol = Volume::from_points(&pts, 12);
    // body scaled to the vehicle's largest dimension
    let s = vol.span.iter().cloned().fold(0.0f64, f64::max) * 0.45;
    let bn = |a: [f64;3], b: [f64;3], r: f64, p: i32| Bone {
        a: [a[0]*s, a[1]*s, a[2]*s], b: [b[0]*s, b[1]*s, b[2]*s], r: r*s, parent: p };
    let mid = [ vol.lo[0] + vol.span[0]*0.5, vol.lo[1] + vol.span[1]*0.5, vol.lo[2] + vol.span[2]*0.5 ];
    let mut body = vec![
        bn([0.0,0.0,0.0], [0.0,1.0,0.0], 0.18, -1),
        bn([0.0,1.0,0.0], [0.0,1.4,0.0], 0.14, 0),
        bn([0.0,0.9,0.0], [-0.7,0.9,0.0], 0.10, 0),
        bn([0.0,0.9,0.0], [0.7,0.9,0.0], 0.10, 0),
        bn([0.0,0.0,0.0], [-0.2,-1.1,0.0], 0.13, 0),
        bn([0.0,0.0,0.0], [0.2,-1.1,0.0], 0.13, 0),
    ];
    for b in body.iter_mut() {
        for p in [&mut b.a, &mut b.b] { for i in 0..3 { p[i] += mid[i]; } }
    }
    let q = std::f64::consts::FRAC_PI_2;
    let limits = vec![
        Limit { joint: 4, axis: [1.0,0.0,0.0], min: -0.2, max: 2.4 },
        Limit { joint: 5, axis: [1.0,0.0,0.0], min: -0.2, max: 2.4 },
        Limit { joint: 2, axis: [0.0,1.0,0.0], min: -q, max: q },
        Limit { joint: 3, axis: [0.0,1.0,0.0], min: -q, max: q },
        Limit { joint: 1, axis: [1.0,0.0,0.0], min: -0.9, max: 0.9 },
        Limit { joint: 0, axis: [0.0,0.0,1.0], min: -0.6, max: 0.6 },
    ];
    let before = evaluate(&body, &vol, 5);
    let f = optimise(&body, &vol, &limits, 900, 20260729);
    println!("{}", path.rsplit('/').next().unwrap());
    println!("  volume cells filled: {}", vol.count());
    println!("  standing : spill {:.3}  vacancy {:.3}  error {:.3}", before.spill, before.vacancy, before.error());
    println!("  folded   : spill {:.3}  vacancy {:.3}  error {:.3}  ({} candidates)",
             f.fit.spill, f.fit.vacancy, f.fit.error(), f.evaluated);
    print!("  angles (deg):");
    for a in &f.angles { print!(" {:.0}", a.to_degrees()); }
    println!();
}
