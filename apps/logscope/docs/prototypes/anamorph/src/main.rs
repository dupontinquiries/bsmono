//! Refractive pearl array — multi-node anamorphic clusters connected by a flow that enters/exits each
//! cluster at the points nearest its edge vectors, refracts through the sphere, splits into prism dispersion,
//! and is occluded by dense regions. Plus a greatest-impact substring cache so building many pearls is cheap.
//!
//! This is the deterministic construction (NOT an emergent solver): each node's word is built from cached
//! glyph layouts; the reveal axis comes from the node's inbound/outbound edge vectors; text lights up only
//! when the camera looks along that axis. Renders a "read" angle and a "3/4 array" angle to PPM.

use fontdue::{Font, FontSettings};
use std::collections::HashMap;

const W: usize = 1200;
const H: usize = 800;

type V3 = [f32; 3];
fn add(a: V3, b: V3) -> V3 { [a[0]+b[0], a[1]+b[1], a[2]+b[2]] }
fn sub(a: V3, b: V3) -> V3 { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
fn mul(a: V3, s: f32) -> V3 { [a[0]*s, a[1]*s, a[2]*s] }
fn dot(a: V3, b: V3) -> f32 { a[0]*b[0]+a[1]*b[1]+a[2]*b[2] }
fn norm(a: V3) -> V3 { let l = dot(a,a).sqrt().max(1e-6); mul(a, 1.0/l) }
fn cross(a: V3, b: V3) -> V3 { [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]] }

// ───────────────────────────── greatest-impact substring cache ─────────────────────────────
/// Rasterizes each unique glyph once, then caches whole-token layouts. Tokens are ranked by IMPACT
/// (frequency × length) across the corpus so the highest-payoff substrings are precomputed; the rest are
/// composed on demand from the per-glyph cache. This is what "eases global compute of the walks": repeated
/// substrings ("Build", "Fix", "ok") are laid out once and reused across every pearl.
struct GlyphCache {
    font: Font,
    px: f32,
    chars: HashMap<char, (Vec<[f32; 2]>, f32)>, // stroke points (origin at pen) + advance
    tokens: HashMap<String, Vec<[f32; 2]>>,      // precomputed high-impact token layouts
    hits: usize,
    misses: usize,
}
impl GlyphCache {
    fn new(px: f32) -> Self {
        let bytes = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf").unwrap();
        Self { font: Font::from_bytes(bytes, FontSettings::default()).unwrap(), px,
               chars: HashMap::new(), tokens: HashMap::new(), hits: 0, misses: 0 }
    }
    fn glyph(&mut self, ch: char) -> &(Vec<[f32; 2]>, f32) {
        let px = self.px; let font = &self.font;
        self.chars.entry(ch).or_insert_with(|| {
            let (m, bmp) = font.rasterize(ch, px);
            let mut pts = Vec::new();
            let step = ((m.width * m.height) / 90).max(1);
            for i in (0..m.width * m.height).step_by(step) {
                if bmp[i] > 128 {
                    let (xx, yy) = (i % m.width, i / m.width);
                    pts.push([m.xmin as f32 + xx as f32, m.ymin as f32 + (m.height as f32 - yy as f32)]);
                }
            }
            (pts, m.advance_width)
        })
    }
    fn layout_token(&mut self, tok: &str) -> Vec<[f32; 2]> {
        let mut out = Vec::new(); let mut pen = 0.0;
        for ch in tok.chars() {
            let (pts, adv) = self.glyph(ch).clone();
            for p in pts { out.push([p[0] + pen, p[1]]); }
            pen += adv;
        }
        out
    }
    /// Precompute layouts for the highest-impact tokens in a corpus of node texts.
    fn prime(&mut self, corpus: &[String], top_k: usize) {
        let mut freq: HashMap<&str, usize> = HashMap::new();
        for t in corpus { for w in t.split_whitespace() { *freq.entry(w).or_insert(0) += 1; } }
        let mut ranked: Vec<(&str, usize)> = freq.into_iter().collect();
        ranked.sort_by_key(|&(w, f)| std::cmp::Reverse(f * w.len())); // impact = freq × length
        for (w, _) in ranked.into_iter().take(top_k) {
            let l = self.layout_token(w); self.tokens.insert(w.to_string(), l);
        }
    }
    /// Lay out a full string, reusing cached token layouts where possible.
    fn layout(&mut self, text: &str) -> Vec<[f32; 2]> {
        let mut out = Vec::new(); let mut pen = 0.0;
        let space = self.glyph(' ').1;
        for tok in text.split_whitespace() {
            let toklay = if let Some(l) = self.tokens.get(tok) { self.hits += 1; l.clone() }
                         else { self.misses += 1; self.layout_token(tok) };
            for p in &toklay { out.push([p[0] + pen, p[1]]); }
            let w = toklay.iter().map(|p| p[0]).fold(0.0_f32, f32::max);
            pen += w + space;
        }
        out
    }
}

// ───────────────────────────── anamorphic cluster ─────────────────────────────
struct Particle { p: V3, glyph: bool, col: V3 }
struct Cluster { center: V3, r: f32, axis: V3, u: V3, v: V3, parts: Vec<Particle> }

fn rngf(s: &mut u32) -> f32 { *s ^= *s << 13; *s ^= *s >> 17; *s ^= *s << 5; (*s as f32 / u32::MAX as f32) * 2.0 - 1.0 }

/// Build a cluster at `center` whose word is read from `axis` (its reveal direction). u,v span the glyph plane.
fn build_cluster(center: V3, r: f32, axis: V3, glyph_pts: &[[f32; 2]], seed: u32) -> Cluster {
    let axis = norm(axis);
    let u = norm(cross(if axis[1].abs() > 0.9 { [1.0,0.0,0.0] } else { [0.0,1.0,0.0] }, axis));
    let v = cross(axis, u);
    // normalize glyph points to the projected disk
    let (mut mnx, mut mny, mut mxx, mut mxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for q in glyph_pts { mnx = mnx.min(q[0]); mny = mny.min(q[1]); mxx = mxx.max(q[0]); mxy = mxy.max(q[1]); }
    let (cx, cy) = ((mnx+mxx)*0.5, (mny+mxy)*0.5);
    let half = ((mxx-mnx).max(mxy-mny)) * 0.5;
    let s = r * 0.9 / half.max(1e-3);
    let mut seed = seed | 1;
    let mut parts = Vec::new();
    for q in glyph_pts {
        let (gx, gy) = ((q[0]-cx)*s, (q[1]-cy)*s);
        for _ in 0..2 {
            let d = rngf(&mut seed) * 0.14 * r;                 // thin glyph layer
            let pos = add(center, add(add(mul(u, gx), mul(v, gy)), mul(axis, d)));
            parts.push(Particle { p: pos, glyph: true, col: [0.7,1.0,0.96] });
        }
    }
    let mut made = 0;                                            // sphere-fill decoys
    while made < 1800 {
        let (a,b,c) = (rngf(&mut seed), rngf(&mut seed), rngf(&mut seed));
        if a*a+b*b+c*c <= 1.0 { parts.push(Particle{ p: add(center,[a*r,b*r,c*r]), glyph:false, col:[0.12,0.5,0.5] }); made+=1; }
    }
    Cluster { center, r, axis, u, v, parts }
}

/// The point on the cluster surface nearest direction `dir` from the center — the natural entry/exit for flow.
fn port(c: &Cluster, dir: V3) -> V3 { add(c.center, mul(norm(dir), c.r)) }

// ───────────────────────────── render ─────────────────────────────
fn main() {
    let mut cache = GlyphCache::new(140.0);
    // a few graph nodes with words drawn from a build-spine corpus (so tokens repeat → cache pays off)
    let corpus: Vec<String> = vec![
        "Build r177 ocean".into(), "Build r178 fsm".into(), "Fix forward Build".into(),
        "ok Build punchlist".into(), "Build biomes".into(),
    ];
    cache.prime(&corpus, 6);

    // nodes along a flow (chain A→B→C with a branch B→D); reveal axis = perpendicular to local flow
    let pos = [[-3.0,0.0,0.0], [0.0,0.0,0.0], [3.0,0.4,0.0], [1.4,-2.4,0.6]];
    let words = ["Build r177 ocean", "Fix forward Build", "ok Build punchlist", "Build biomes"];
    let edges = [(0usize,1usize),(1,2),(1,3)];
    // flow tangent per node (avg of incident edge dirs); reveal axis = tangent × up (faces across flow)
    let mut clusters = Vec::new();
    for i in 0..pos.len() {
        let mut flow = [0.0f32;3]; let mut k=0.0;
        for &(a,b) in &edges { if a==i { flow = add(flow, sub(pos[b],pos[a])); k+=1.0; } if b==i { flow = add(flow, sub(pos[b],pos[a])); k+=1.0; } }
        if k>0.0 { flow = mul(flow, 1.0/k); }
        let axis = norm(cross(norm(flow), [0.0,1.0,0.0]));      // text faces across the flow
        let gp = cache.layout(words[i]);
        clusters.push(build_cluster(pos[i], 1.0, axis, &gp, 0x1234 + i as u32 * 777));
    }
    eprintln!("substring cache: {} hits, {} misses, {} unique glyphs, {} primed tokens",
              cache.hits, cache.misses, cache.chars.len(), cache.tokens.len());

    for (tag, yaw, pitch) in [("read", 0.0_f32, 0.05_f32), ("array", 0.9, 0.35)] {
        let (cyaw, syaw) = (yaw.cos(), yaw.sin());
        let (cp, sp) = (pitch.cos(), pitch.sin());
        // view direction (camera → scene) for alignment-gated reveal
        let view = norm([syaw*cp, sp, cyaw*cp]);
        let scale = 150.0;
        let (ox, oy) = (W as f32*0.5, H as f32*0.5);
        let proj = |w: V3| -> (f32,f32,f32) {
            let x1 = w[0]*cyaw - w[2]*syaw;
            let z1 = w[0]*syaw + w[2]*cyaw;
            let y1 = w[1]*cp - z1*sp;
            let z2 = w[1]*sp + z1*cp;
            (ox + x1*scale, oy - y1*scale, z2)
        };
        let mut fb = vec![0.0f32; W*H*3];

        // ── flow with optical rules: enter nearest inbound, refract through, exit nearest outbound,
        //    prism-split into RGB, occlude where it crosses behind a dense (near-center) region ──
        for &(a,b) in &edges {
            let ca = &clusters[a]; let cb = &clusters[b];
            let exit = port(ca, sub(cb.center, ca.center));      // leave A toward B
            let entry = port(cb, sub(ca.center, cb.center));     // enter B from A
            // refraction: bend the path through B toward its onward exit (toward B's own children avg)
            let mut onward = [0.0f32;3];
            for &(x,y) in &edges { if x==b { onward = add(onward, sub(clusters[y].center, cb.center)); } }
            let refract_to = if dot(onward,onward) > 1e-4 { port(cb, onward) } else { cb.center };
            let bend = add(mul(add(entry, refract_to),0.5), mul(sub(cb.center, add(mul(add(entry,refract_to),0.5),[0.0,0.0,0.0])), 0.4));
            let pts3 = [exit, entry, bend, refract_to];           // exit → entry → (refract) → onward
            let prism = [([1.0,0.2,0.2], 0.03f32), ([0.3,1.0,0.4], 0.0), ([0.4,0.5,1.0], -0.03)]; // RGB dispersion offsets
            for (col, off) in prism {
                let n = 120;
                for s in 0..n {
                    let t = s as f32 / n as f32;
                    // piecewise along the 4 control points
                    let seg = (t*3.0).floor().min(2.0) as usize; let lt = t*3.0 - seg as f32;
                    let p = add(mul(pts3[seg], 1.0-lt), mul(pts3[seg+1], lt));
                    let pofs = add(p, mul(cross(view,[0.0,1.0,0.0]), off)); // chromatic lateral offset
                    let (sx,sy,sz) = proj(pofs);
                    // occlusion: dim if this point sits inside a cluster's dense core (behind pearls)
                    let mut occ = 1.0f32;
                    for c in &clusters { let d = sub(pofs, c.center); if dot(d,d) < (c.r*0.55).powi(2) { occ *= 0.4; } }
                    let depth = (sz+6.0)/12.0;
                    splat(&mut fb, sx, sy, 1.5, mul(col, 0.22*occ*(0.5+0.5*depth)));
                }
            }
        }

        // ── clusters: decoys (orb body) + glyph (alignment-gated to each cluster's reveal axis) ──
        for c in &clusters {
            let gain = dot(view, c.axis).abs().powf(3.0);         // text blazes only when looked at along its axis
            for pt in &c.parts {
                let (sx,sy,sz) = proj(pt.p);
                let depth = (sz+6.0)/12.0;
                if pt.glyph {
                    let bb = (0.55+0.45*depth)*gain; if bb<0.02 { continue; }
                    splat(&mut fb, sx, sy, 2.0+1.5*depth, mul(pt.col, bb));
                } else {
                    let bb = 0.09+0.14*depth;
                    splat(&mut fb, sx, sy, 1.3+1.0*depth, mul(pt.col, bb));
                }
            }
        }

        let mut out = format!("P6\n{W} {H}\n255\n").into_bytes();
        for v in &fb { let t=(v/(1.0+v)).clamp(0.0,1.0); out.push((t.powf(1.0/2.2)*255.0) as u8); }
        std::fs::write(format!("/home/claude/anamorph/arr_{tag}.ppm"), out).unwrap();
        eprintln!("wrote arr_{tag} (yaw {yaw}, pitch {pitch})");
    }
}

fn splat(fb: &mut [f32], cx: f32, cy: f32, r: f32, col: V3) {
    if cx < -r || cy < -r || cx > W as f32 + r || cy > H as f32 + r { return; }
    let (x0,x1) = ((cx-r).floor().max(0.0) as usize, (cx+r).ceil().min(W as f32-1.0) as usize);
    let (y0,y1) = ((cy-r).floor().max(0.0) as usize, (cy+r).ceil().min(H as f32-1.0) as usize);
    for y in y0..=y1 { for x in x0..=x1 {
        let d2=(x as f32-cx).powi(2)+(y as f32-cy).powi(2);
        let a=(-d2/(r*r*0.5)).exp();
        if a>0.003 { let i=(y*W+x)*3; fb[i]+=col[0]*a; fb[i+1]+=col[1]*a; fb[i+2]+=col[2]*a; }
    }}
}
