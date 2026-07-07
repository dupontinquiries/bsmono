// Standalone microbench replicating the per-frame creature-AI patterns found in
// unendlich_fullgame.rs at the operator's scale: ~100 dragons + ~100 fauna,
// render distance 4. No engine dep — pure rustc — to get RELATIVE timings that
// localize which pattern dominates a frame. Numbers are llvmpipe-host CPU
// (slower than the operator's M2, but RELATIVE ordering transfers).
use std::time::Instant;

const DRAGONS: usize = 100;
const FAUNA: usize = 100;
const ROAMERS: usize = DRAGONS + FAUNA; // 200 total
const FRAMES: usize = 2000;

#[derive(Clone, Copy)]
struct Body { pos: [f32; 3], yaw: f32 }
#[derive(Clone, Copy)]
struct Roamer { body: Body, is_dragon: bool, energy: f32, flying: bool }

fn mk() -> Vec<Roamer> {
    let mut v = Vec::with_capacity(ROAMERS);
    let mut s: u64 = 0x1234;
    let mut rnd = || { s ^= s << 13; s ^= s >> 7; s ^= s << 17; (s >> 11) as f32 / (1u64<<53) as f32 };
    for i in 0..ROAMERS {
        let dragon = i < DRAGONS;
        v.push(Roamer{
            body: Body{ pos:[rnd()*256.0-128.0, rnd()*64.0, rnd()*256.0-128.0], yaw: rnd()*6.28 },
            is_dragon: dragon,
            energy: rnd(),       // random hunger
            flying: dragon && rnd() > 0.3,
        });
    }
    v
}

fn main() {
    let base = mk();

    // ---- A) DRAGON HUNT scan: per flying+hungry dragon, scan all fauna (the
    //         O(dragons × fauna) nested loop at fullgame line ~5126). ----
    {
        let roamers = base.clone();
        // snapshot of fauna positions (herd_snapshot), as the bin builds
        let herd: Vec<[f32;3]> = roamers.iter().filter(|r| !r.is_dragon).map(|r| r.body.pos).collect();
        let mut sink = 0f32;
        let t = Instant::now();
        for _ in 0..FRAMES {
            for r in roamers.iter() {
                if r.is_dragon && r.energy < 0.6 && r.flying {
                    let mypos = r.body.pos;
                    let mut nearest: Option<(usize,f32)> = None;
                    for (oi, op) in herd.iter().enumerate() {
                        let dx2 = (op[0]-mypos[0]).powi(2) + (op[2]-mypos[2]).powi(2);
                        if dx2 < 64.0*64.0 && nearest.map_or(true,|(_,d)| dx2<d) { nearest = Some((oi,dx2)); }
                    }
                    if let Some((_,d)) = nearest { sink += d; }
                }
            }
        }
        let per_frame = t.elapsed().as_secs_f64()*1e3 / FRAMES as f64;
        let hungry = roamers.iter().filter(|r| r.is_dragon && r.energy<0.6 && r.flying).count();
        println!("A) dragon-hunt scan      : {:.4} ms/frame  ({} hungry-flying dragons × {} fauna)  sink={:.1}", per_frame, hungry, herd.len(), sink);
    }

    // ---- A') WORST CASE: ALL dragons hungry+flying at once (the p95 burst when
    //          a batch crosses the energy threshold together). ----
    {
        let mut roamers = base.clone();
        for r in roamers.iter_mut() { if r.is_dragon { r.energy = 0.3; r.flying = true; } }
        let herd: Vec<[f32;3]> = roamers.iter().filter(|r| !r.is_dragon).map(|r| r.body.pos).collect();
        let mut sink = 0f32;
        let t = Instant::now();
        for _ in 0..FRAMES {
            for r in roamers.iter() {
                if r.is_dragon && r.energy < 0.6 && r.flying {
                    let mypos = r.body.pos;
                    let mut nearest: Option<(usize,f32)> = None;
                    for (oi, op) in herd.iter().enumerate() {
                        let dx2 = (op[0]-mypos[0]).powi(2) + (op[2]-mypos[2]).powi(2);
                        if dx2 < 64.0*64.0 && nearest.map_or(true,|(_,d)| dx2<d) { nearest = Some((oi,dx2)); }
                    }
                    if let Some((_,d)) = nearest { sink += d; }
                }
            }
        }
        let per_frame = t.elapsed().as_secs_f64()*1e3 / FRAMES as f64;
        println!("A') dragon-hunt WORST    : {:.4} ms/frame  (ALL {} dragons × {} fauna = {} checks)  sink={:.1}", per_frame, DRAGONS, herd.len(), DRAGONS*herd.len(), sink);
    }

    // ---- B) proximity any() scans (too_close / predator_near): O(n) per check,
    //         but if done per-roamer it's O(n²). Bench the per-roamer variant. ----
    {
        let roamers = base.clone();
        let mut hits = 0u64;
        let t = Instant::now();
        for _ in 0..FRAMES {
            for r in roamers.iter() {
                let pp = r.body.pos;
                let near = roamers.iter().any(|o| {
                    let dx=o.body.pos[0]-pp.x_(); let dz=o.body.pos[2]-pp.z_();
                    dx*dx+dz*dz < 24.0*24.0
                });
                if near { hits+=1; }
            }
        }
        let per_frame = t.elapsed().as_secs_f64()*1e3 / FRAMES as f64;
        println!("B) per-roamer any() scan : {:.4} ms/frame  ({}×{} = {} checks)  hits={}", per_frame, ROAMERS, ROAMERS, ROAMERS*ROAMERS, hits);
    }

    // ---- C) steady per-roamer AI work (cheap math per roamer: steering +
    //         distance + a few trig — stand-in for the gated per-roamer tick). ----
    {
        let roamers = base.clone();
        let mut sink = 0f32;
        let t = Instant::now();
        for _ in 0..FRAMES {
            for r in roamers.iter() {
                // a handful of float ops + trig, ~ what a wander/steering step costs
                let h = (r.body.yaw.sin()*0.5 + r.body.pos[0]*0.01).cos();
                let d = (r.body.pos[0].powi(2)+r.body.pos[2].powi(2)).sqrt();
                sink += h + d*0.001 + r.body.yaw.atan2(d.max(1.0));
            }
        }
        let per_frame = t.elapsed().as_secs_f64()*1e3 / FRAMES as f64;
        println!("C) steady per-roamer tick: {:.4} ms/frame  ({} roamers, gated by ai_budget in-engine)  sink={:.1}", per_frame, ROAMERS, sink);
    }
}

// tiny helpers so the any() closure reads like the bin's pp.x / pp.z
trait Xyz { fn x_(&self)->f32; fn z_(&self)->f32; }
impl Xyz for [f32;3] { fn x_(&self)->f32{self[0]} fn z_(&self)->f32{self[2]} }
