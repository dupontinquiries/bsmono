//! The whole pipeline on one vehicle: fold the skeleton in, assign parts by
//! containment, unfold, and write a five-stage sheet as a PPM.
use model_verify::{assign::*, fold::*};
use std::{env, fs};

fn load(p: &str) -> (Vec<[f64;3]>, Vec<u32>) {
    let b = fs::read(p).unwrap_or_default();
    if b.len() < 64 || &b[..15] != b"INTERQUAKEMODEL" { return (vec![], vec![]); }
    let g = |o: usize| u32::from_le_bytes(b[o..o+4].try_into().unwrap()) as usize;
    let (nva, nv, ofs_va) = (g(44), g(48), g(52));
    let (nt, ofs_tri) = (g(56), g(60));
    let mut pos = vec![];
    for i in 0..nva {
        let e = ofs_va + i*20;
        let (ty, fmt, size, off) = (g(e), g(e+8), g(e+12), g(e+16));
        if ty == 0 && fmt == 7 {
            pos = (0..nv).map(|k| { let o = off + k*size*4;
                let f = |j: usize| f32::from_le_bytes(b[o+j*4..o+j*4+4].try_into().unwrap()) as f64;
                [f(0), f(1), f(2)] }).collect();
        }
    }
    let tris = (0..nt*3).filter_map(|i| {
        let o = ofs_tri + i*4;
        (o+4 <= b.len()).then(|| u32::from_le_bytes(b[o..o+4].try_into().unwrap()))
    }).collect();
    (pos, tris)
}

fn comps_of(v: &[[f64;3]], t: &[u32]) -> Vec<Vec<usize>> {
    let mut lo=[f64::MAX;3]; let mut hi=[f64::MIN;3];
    for p in v { for i in 0..3 { lo[i]=lo[i].min(p[i]); hi[i]=hi[i].max(p[i]); } }
    let span = (0..3).map(|i| hi[i]-lo[i]).fold(0.0f64, f64::max).max(1e-6);
    let remap = model_verify::weld(v, span*1e-5);
    model_verify::components(v.len(), t, &remap).into_iter().filter(|c| c.len() >= 12).collect()
}

fn main() {
    let path = env::args().nth(1).expect("usage: unfold MODEL.iqm OUT.ppm");
    let out = env::args().nth(2).unwrap_or_else(|| "unfold.ppm".into());
    let (v, t) = load(&path);
    if v.is_empty() { println!("no geometry"); return; }
    let comps = comps_of(&v, &t);
    let vol = Volume::from_points(&v, 12);
    let s = vol.span.iter().cloned().fold(0.0f64, f64::max) * 0.42;
    let mid = [vol.lo[0]+vol.span[0]*0.5, vol.lo[1]+vol.span[1]*0.5, vol.lo[2]+vol.span[2]*0.5];
    let bn = |a: [f64;3], b: [f64;3], r: f64, p: i32| Bone {
        a: [mid[0]+a[0]*s, mid[1]+a[1]*s, mid[2]+a[2]*s],
        b: [mid[0]+b[0]*s, mid[1]+b[1]*s, mid[2]+b[2]*s], r: r*s, parent: p };
    // upright bot: this is the TARGET, and folding takes it into the vehicle
    let upright = vec![
        bn([0.0,0.0,0.0],[0.0,1.0,0.0],0.22,-1),
        bn([0.0,1.0,0.0],[0.0,1.35,0.0],0.16,0),
        bn([0.0,0.85,0.0],[-0.75,0.4,0.0],0.12,0),
        bn([0.0,0.85,0.0],[0.75,0.4,0.0],0.12,0),
        bn([0.0,0.0,0.0],[-0.25,-1.15,0.0],0.15,0),
        bn([0.0,0.0,0.0],[0.25,-1.15,0.0],0.15,0),
    ];
    let q = std::f64::consts::FRAC_PI_2;
    let limits = vec![
        Limit{joint:4,axis:[1.0,0.0,0.0],min:-0.2,max:2.6},
        Limit{joint:5,axis:[1.0,0.0,0.0],min:-0.2,max:2.6},
        Limit{joint:2,axis:[0.0,1.0,0.0],min:-q,max:q},
        Limit{joint:3,axis:[0.0,1.0,0.0],min:-q,max:q},
        Limit{joint:1,axis:[1.0,0.0,0.0],min:-1.0,max:1.0},
        Limit{joint:0,axis:[0.0,0.0,1.0],min:-0.7,max:0.7},
    ];
    let f = optimise(&upright, &vol, &limits, 1200, 20260729);
    let folded = f.bones.clone();
    println!("fold: spill {:.3} vacancy {:.3} error {:.3}", f.fit.spill, f.fit.vacancy, f.fit.error());

    let pts: Vec<Vec<[f64;3]>> = comps.iter().map(|c| c.iter().map(|&i| v[i]).collect()).collect();
    // which component owns each vertex, so triangles can be routed to a bone
    let mut owner = vec![usize::MAX; v.len()];
    for (ci, c) in comps.iter().enumerate() { for &i in c { owner[i] = ci; } }
    // triangles grouped by component (only those wholly inside one)
    let mut ctris: Vec<Vec<[usize;3]>> = vec![vec![]; comps.len()];
    for tri in t.chunks_exact(3) {
        let (a,b,c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        if a>=v.len()||b>=v.len()||c>=v.len() { continue; }
        let o = owner[a];
        if o != usize::MAX && owner[b]==o && owner[c]==o {
            // local indices within the component
            let li = |g: usize| comps[o].iter().position(|&x| x==g).unwrap_or(0);
            ctris[o].push([li(a), li(b), li(c)]);
        }
    }
    let a = assign(&pts, &folded, 0.3);
    let cov = coverage(&a, folded.len(), s * 0.9);
    println!("assign: {} parts, per-bone {:?}, empty {}, orphans {}",
             a.len(), cov.per_bone, cov.empty_bones, cov.orphans);

    // five stages: folded (vehicle) -> upright (bot)
    const N: usize = 5;
    const W: usize = 260; const H: usize = 260;
    let mut tiles = vec![];
    for stage in 0..N {
        let u = stage as f64 / (N-1) as f64;
        let e = u*u*(3.0-2.0*u);
        let inter: Vec<Bone> = folded.iter().zip(&upright).map(|(fb, ub)| Bone {
            a: [fb.a[0]+(ub.a[0]-fb.a[0])*e, fb.a[1]+(ub.a[1]-fb.a[1])*e, fb.a[2]+(ub.a[2]-fb.a[2])*e],
            b: [fb.b[0]+(ub.b[0]-fb.b[0])*e, fb.b[1]+(ub.b[1]-fb.b[1])*e, fb.b[2]+(ub.b[2]-fb.b[2])*e],
            r: fb.r, parent: fb.parent }).collect();
        let mut moved: Vec<(usize, Vec<[f64;3]>)> = vec![];
        for (ci, asg) in a.iter().enumerate() {
            if let Some(m) = carry(&pts[ci], &folded[asg.bone], &inter[asg.bone]) {
                moved.push((asg.bone, m));
            }
        }
        // project side profile
        let (mut lo, mut hi) = ([f64::MAX;3], [f64::MIN;3]);
        for (_, m) in &moved { for p in m { for i in 0..3 { lo[i]=lo[i].min(p[i]); hi[i]=hi[i].max(p[i]); } } }
        let sp: Vec<f64> = (0..3).map(|i| (hi[i]-lo[i]).max(1e-6)).collect();
        let ax: Vec<usize> = { let mut o: Vec<usize> = (0..3).collect();
            o.sort_by(|&x,&y| sp[y].partial_cmp(&sp[x]).unwrap()); o };
        let (la, ua) = (ax[0], ax[1]);
        let mut img = vec![[14u8,16,22]; W*H];
        let mut zbuf = vec![f64::MAX; W*H];
        let pal = [[236u8,132,98],[112,192,232],[146,214,126],[226,186,96],[196,146,234],[234,138,186]];
        // Solid shaded triangles with a depth buffer — a point cloud is
        // unreadable at this density and cannot show whether parts travel as
        // coherent bodies, which is the only question that matters here.
        let da = ax[2];   // the axis we are looking along
        for (ci, (bone, m)) in moved.iter().enumerate() {
            let base = pal[bone % pal.len()];
            for tri in &ctris[ci] {
                let (p0, p1, p2) = (m[tri[0]], m[tri[1]], m[tri[2]]);
                let sx = |p: [f64;3]| (((p[la]-lo[la])/sp[la])*(W as f64-8.0)) + 4.0;
                let sy = |p: [f64;3]| ((1.0-(p[ua]-lo[ua])/sp[ua])*(H as f64-8.0)) + 4.0;
                let (x0,y0,x1,y1,x2,y2) = (sx(p0),sy(p0),sx(p1),sy(p1),sx(p2),sy(p2));
                // facing, for shading: a flat colour hides all the form
                let e1 = [x1-x0, y1-y0]; let e2 = [x2-x0, y2-y0];
                let cross = e1[0]*e2[1] - e1[1]*e2[0];
                let depth = (p0[da]+p1[da]+p2[da])/3.0;
                let shade = 0.45 + 0.55 * (cross.abs() / (cross.abs()+40.0)).min(1.0);
                let col = [ (base[0] as f64*shade) as u8,
                            (base[1] as f64*shade) as u8,
                            (base[2] as f64*shade) as u8 ];
                let (mnx, mxx) = (x0.min(x1).min(x2).max(0.0) as usize, x0.max(x1).max(x2).min(W as f64-1.0) as usize);
                let (mny, mxy) = (y0.min(y1).min(y2).max(0.0) as usize, y0.max(y1).max(y2).min(H as f64-1.0) as usize);
                if cross.abs() < 1e-9 { continue; }
                for py in mny..=mxy { for px in mnx..=mxx {
                    let (fx, fy) = (px as f64 + 0.5, py as f64 + 0.5);
                    let w0 = ((y1-y2)*(fx-x2) + (x2-x1)*(fy-y2)) / ((y1-y2)*(x0-x2)+(x2-x1)*(y0-y2));
                    let w1 = ((y2-y0)*(fx-x2) + (x0-x2)*(fy-y2)) / ((y1-y2)*(x0-x2)+(x2-x1)*(y0-y2));
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -0.002 || w1 < -0.002 || w2 < -0.002 { continue; }
                    let k = py*W + px;
                    if depth < zbuf[k] { zbuf[k] = depth; img[k] = col; }
                }}
            }
        }
        tiles.push(img);
    }
    let mut f2 = Vec::new();
    f2.extend_from_slice(format!("P6\n{} {}\n255\n", W*N, H).as_bytes());
    for y in 0..H { for t in &tiles { for x in 0..W {
        f2.extend_from_slice(&t[y*W+x]); } } }
    fs::write(&out, f2).unwrap();
    println!("-> {out}");
}
