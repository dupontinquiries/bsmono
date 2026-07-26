// Benchmark: soldier's has_los (Bresenham, per-tile test) vs the same walk with a
// shocovox-style coarse occupancy summary for empty-space skipping.
// Grid + blocker density chosen to mimic soldier's levels (walls + tall cover).
use std::time::Instant;

const W: i32 = 128;
const H: i32 = 128;

struct Map { blocked: Vec<bool>, bits: Vec<u64>, coarse: Vec<u64>, cw: i32 }

impl Map {
    fn new(seed: u64) -> Map {
        let mut s = seed;
        let mut rnd = || { s ^= s << 13; s ^= s >> 7; s ^= s << 17; s };
        let mut blocked = vec![false; (W * H) as usize];
        // clustered obstacles, like authored walls/cover rather than uniform noise
        for _ in 0..180 {
            let cx = (rnd() % W as u64) as i32;
            let cz = (rnd() % H as u64) as i32;
            let rw = 1 + (rnd() % 5) as i32;
            let rh = 1 + (rnd() % 5) as i32;
            for z in cz..(cz + rh).min(H) { for x in cx..(cx + rw).min(W) {
                blocked[(z * W + x) as usize] = true;
            }}
        }
        // coarse summary: one bit per 8x8 tile block, packed into u64 rows
        let cw = (W + 7) / 8;
        let ch = (H + 7) / 8;
        let mut coarse = vec![0u64; ch as usize];
        for cz in 0..ch { for cx in 0..cw {
            let mut any = false;
            'o: for z in cz*8..((cz+1)*8).min(H) { for x in cx*8..((cx+1)*8).min(W) {
                if blocked[(z * W + x) as usize] { any = true; break 'o; }
            }}
            if any { coarse[cz as usize] |= 1u64 << cx; }
        }}
        // bit-packed twin of `blocked`: 16 KB of Vec<bool> becomes 2 KB, which fits L1
        // alongside everything else the pass touches.
        let mut bits = vec![0u64; ((W*H) as usize + 63)/64];
        for i in 0..(W*H) as usize { if blocked[i] { bits[i>>6] |= 1u64 << (i & 63); } }
        Map { blocked, bits, coarse, cw }
    }
    #[inline] fn is_blocked(&self, x: i32, z: i32) -> bool { self.blocked[(z * W + x) as usize] }
    #[inline] fn is_blocked_bit(&self, x: i32, z: i32) -> bool {
        let i = (z * W + x) as usize;
        (self.bits[i>>6] >> (i & 63)) & 1 == 1
    }
    #[inline] fn coarse_any(&self, cx: i32, cz: i32) -> bool {
        if cx < 0 || cz < 0 || cx >= self.cw { return true; }
        (self.coarse[cz as usize] >> cx) & 1 == 1
    }
}

// --- soldier's shape: step every tile, test every tile ---
fn los_naive(m: &Map, a: (i32,i32), b: (i32,i32)) -> bool {
    let (mut x0, mut z0) = a; let (x1, z1) = b;
    let dx = (x1-x0).abs(); let dz = -(z1-z0).abs();
    let sx = if x0<x1 {1} else {-1}; let sz = if z0<z1 {1} else {-1};
    let mut err = dx + dz;
    loop {
        if (x0,z0) != a && m.is_blocked(x0,z0) { return false; }
        if (x0,z0) == (x1,z1) { return true; }
        let e2 = 2*err;
        if e2 >= dz { err += dz; x0 += sx; }
        if e2 <= dx { err += dx; z0 += sz; }
    }
}

// --- same walk, but skip the per-tile test while inside an empty coarse block ---
fn los_coarse(m: &Map, a: (i32,i32), b: (i32,i32)) -> bool {
    let (mut x0, mut z0) = a; let (x1, z1) = b;
    let dx = (x1-x0).abs(); let dz = -(z1-z0).abs();
    let sx = if x0<x1 {1} else {-1}; let sz = if z0<z1 {1} else {-1};
    let mut err = dx + dz;
    let (mut ccx, mut ccz) = (-999, -999);
    let mut cur_empty = false;
    loop {
        let (bx, bz) = (x0 >> 3, z0 >> 3);
        if bx != ccx || bz != ccz { ccx = bx; ccz = bz; cur_empty = !m.coarse_any(bx, bz); }
        if !cur_empty && (x0,z0) != a && m.is_blocked(x0,z0) { return false; }
        if (x0,z0) == (x1,z1) { return true; }
        let e2 = 2*err;
        if e2 >= dz { err += dz; x0 += sx; }
        if e2 <= dx { err += dx; z0 += sz; }
    }
}

fn los_bitset(m: &Map, a: (i32,i32), b: (i32,i32)) -> bool {
    let (mut x0, mut z0) = a; let (x1, z1) = b;
    let dx = (x1-x0).abs(); let dz = -(z1-z0).abs();
    let sx = if x0<x1 {1} else {-1}; let sz = if z0<z1 {1} else {-1};
    let mut err = dx + dz;
    loop {
        if (x0,z0) != a && m.is_blocked_bit(x0,z0) { return false; }
        if (x0,z0) == (x1,z1) { return true; }
        let e2 = 2*err;
        if e2 >= dz { err += dz; x0 += sx; }
        if e2 <= dx { err += dx; z0 += sz; }
    }
}

fn main() {
    let m = Map::new(0x9E3779B97F4A7C15);
    // soldier caps the sample set at 260 and does the full upper triangle
    let mut sample = Vec::new();
    let mut s: u64 = 12345;
    let mut rnd = || { s ^= s << 13; s ^= s >> 7; s ^= s << 17; s };
    while sample.len() < 260 {
        let x = (rnd() % W as u64) as i32; let z = (rnd() % H as u64) as i32;
        if !m.is_blocked(x,z) { sample.push((x,z)); }
    }
    let mut agree = 0usize; let mut pairs = 0usize;
    for i in 0..sample.len() { for j in (i+1)..sample.len() {
        pairs += 1;
        if los_naive(&m, sample[i], sample[j]) == los_coarse(&m, sample[i], sample[j]) { agree += 1; }
    }}
    println!("pairs={pairs} agree={agree} (must be equal — the skip must not change results)");

    for i in 0..sample.len() { for j in (i+1)..sample.len() {
        assert_eq!(los_naive(&m, sample[i], sample[j]), los_bitset(&m, sample[i], sample[j]));
    }}
    for (name, f) in [("naive", los_naive as fn(&Map,(i32,i32),(i32,i32))->bool), ("coarse-skip", los_coarse), ("bitset", los_bitset)] {
        let t = Instant::now();
        let mut acc = 0u64;
        for _ in 0..20 {
            for i in 0..sample.len() { for j in (i+1)..sample.len() {
                if f(&m, sample[i], sample[j]) { acc += 1; }
            }}
        }
        println!("{name:12} {:>8.1} ms   (visible={acc})", t.elapsed().as_secs_f64()*1000.0);
    }
}
