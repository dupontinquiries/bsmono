//! examples/headless_run.rs — r190 harness v1: deterministic headless sim
//! with FUZZED player input, per-tick DIGESTS, multi-dimensional COVERAGE
//! scoring, optional BUTTERFLY divergence pair, and a top-down PPM so the
//! operator can eyeball the run. See docs/TESTING_DIVERGENCE.md.
//!
//! Run:  cargo run --example headless_run --release
//! Env:  HR_SEED=1337 HR_TICKS=1000 HR_BUTTERFLY=1 HR_OUT=/tmp
use std::collections::HashSet;
use std::sync::Arc;
use unendlich_engine::world::biomes::Biome;
use unendlich_engine::world::chunk::ChunkCoord;
use unendlich_engine::world::cpu_generator::WorldGenerator;
use unendlich_engine::world::ticks::{TickId, TickRegistry};

const CS: i32 = unendlich_engine::constants::CHUNK_SIZE;

fn fnv(h: u64, v: u64) -> u64 {
    (h ^ v).wrapping_mul(0x100000001b3)
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 11
    }
    fn f01(&mut self) -> f32 {
        (self.next() % 10_000) as f32 / 10_000.0
    }
}

struct RunResult {
    digests: Vec<u64>,
    handler_fire_ticks: Vec<u64>,
    biomes_seen: HashSet<u8>,
    edits_total: usize,
    cat: [usize; 4],
    corals: usize,
    final_pos: [f32; 3],
}

fn run(seed: u32, ticks: u64, flip_at: Option<u64>) -> RunResult {
    let gen = WorldGenerator::new(seed);
    // 5x5 chunk patch around origin at surface band: enough residency for
    // plant/leaf handlers to have material, small enough for speed.
    // HR_OCEAN=1 recenters the patch on the nearest ocean column so the
    // reef_seeder has seafloor to work with.
    let chunks = dashmap::DashMap::new();
    let (mut bx, mut bz) = (0i32, 0i32);
    if std::env::var("HR_OCEAN").ok().as_deref() == Some("1") {
        'search: for gz in (-6400i32..6400).step_by(64) {
            for gx in (-6400i32..6400).step_by(64) {
                let y = gen.surface_y_at(gx, gz, -64, 200).unwrap_or(64);
                let slv = gen.sea_level_at(gx, gz);
                if (y as f64) < slv - 12.0
                    && !gen.is_cave(gx, slv as i32 - 2, gz) // dry-crater bug guard (r192 ledger)
                    && matches!(gen.biome_at_3d(gx, y, gz).biome, Biome::Ocean)
                {
                    bx = gx; bz = gz;
                    break 'search;
                }
            }
        }
        println!("ocean recenter: ({bx},{bz})");
    }
    let sy = gen.surface_y_at(bx, bz, -64, 200).unwrap_or(64);
    let cy = sy.div_euclid(CS);
    for cz in -2..=2 {
        for cx in -2..=2 {
            for dy in -1..=1 {
                let coord = ChunkCoord::new(cx + bx.div_euclid(CS), cy + dy, cz + bz.div_euclid(CS));
                chunks.insert(coord, Arc::new(gen.generate_chunk(coord)));
            }
        }
    }

    let mut reg = TickRegistry::new();
    reg.register(
        "plant_lifecycle",
        unendlich_engine::world::plant_lifecycle::lifecycle_handler(),
    );
    reg.register(
        "leaf_decay",
        unendlich_engine::world::leaf_decay::decay_handler(),
    );
    reg.register(
        "reef_seeder",
        unendlich_engine::world::reef_seeder::reef_handler(),
    );
    let names = reg.handler_names();

    // Fuzzed player: deterministic walk; the ONLY butterfly source.
    let mut rng = Lcg(seed as u64 ^ 0x9e3779b97f4a7c15);
    let mut pos = [0.5f32, sy as f32 + 1.5, 0.5f32];
    let mut digests = Vec::with_capacity(ticks as usize);
    let mut handler_fire_ticks = vec![0u64; names.len()];
    let mut biomes_seen: HashSet<u8> = HashSet::new();
    let mut edits_total = 0usize;
    let mut cat = [0usize; 4];
    let mut corals = 0usize;

    for t in 0..ticks {
        // input fuzz: 8 directions + idle, occasionally sprint-length steps.
        let mut bits = rng.next() & 0xF;
        if Some(t) == flip_at {
            bits ^= 0b0001; // the butterfly: one flipped input bit
        }
        let step = if rng.f01() < 0.1 { 1.6 } else { 0.45 };
        if bits & 1 != 0 {
            pos[0] += step;
        }
        if bits & 2 != 0 {
            pos[0] -= step;
        }
        if bits & 4 != 0 {
            pos[2] += step;
        }
        if bits & 8 != 0 {
            pos[2] -= step;
        }
        // keep inside the resident patch
        let lim = (CS * 2) as f32 - 2.0;
        pos[0] = pos[0].clamp(-lim, lim);
        pos[2] = pos[2].clamp(-lim, lim);

        let pchunk = ChunkCoord::new(
            (pos[0] as i32).div_euclid(CS),
            cy,
            (pos[2] as i32).div_euclid(CS),
        );
        let edits = reg.run_tick(
            &chunks,
            None,
            None,
            Some(pchunk),
            TickId(t),
            0.5,
            0.5,
            None,
            None,
        );
        edits_total += edits.block_edits.len() + edits.state_changes.len();
        cat[0] += edits.block_edits.len();
        corals += edits
            .block_edits
            .iter()
            .filter(|e| {
                matches!(
                    e.new_block,
                    unendlich_engine::world::blocks::BlockId::Bone
                        | unendlich_engine::world::blocks::BlockId::Flower
                        | unendlich_engine::world::blocks::BlockId::Moss
                )
            })
            .count();
        cat[1] += edits.state_changes.len();
        cat[2] += edits.transition_ops.len();
        cat[3] += edits.tree_root_ops.len();
        for (i, (_n, us)) in edits.handler_timings.iter().enumerate() {
            if *us > 0 {
                handler_fire_ticks[i] += 1;
            }
        }
        let b = gen.biome_at_3d(pos[0] as i32, sy, pos[2] as i32).biome;
        biomes_seen.insert(b as u8);

        // per-tick digest: the divergence unit.
        let mut h = 0xcbf29ce484222325u64;
        h = fnv(h, (pos[0] * 16.0) as i64 as u64);
        h = fnv(h, (pos[2] * 16.0) as i64 as u64);
        h = fnv(h, edits.block_edits.len() as u64);
        h = fnv(h, edits.state_changes.len() as u64);
        h = fnv(h, b as u64);
        digests.push(h);
    }
    RunResult {
        digests,
        handler_fire_ticks,
        biomes_seen,
        edits_total,
        cat,
        corals,
        final_pos: pos,
    }
}

fn topdown_ppm(seed: u32, path: &str) {
    let gen = WorldGenerator::new(seed);
    let (w, h) = (128usize, 128usize);
    let mut out = format!("P3\n{} {}\n255\n", w, h);
    for py in 0..h {
        for px in 0..w {
            let wx = (px as i32 - w as i32 / 2) * 3;
            let wz = (py as i32 - h as i32 / 2) * 3;
            let sy = gen.surface_y_at(wx, wz, -64, 200).unwrap_or(64);
            let b = gen.biome_at_3d(wx, sy, wz).biome;
            let base: [f32; 3] = match b {
                Biome::Ocean => [38.0, 84.0, 148.0],
                Biome::Beach => [214.0, 198.0, 148.0],
                Biome::Desert | Biome::RedSandstoneCanyon => [212.0, 170.0, 96.0],
                Biome::Tundra | Biome::AlpineHighlands => [228.0, 234.0, 240.0],
                Biome::Swamp => [86.0, 108.0, 70.0],
                _ => [88.0, 140.0, 76.0],
            };
            let shade = 0.6 + 0.55 * ((sy.clamp(0, 160) as f32) / 160.0);
            out.push_str(&format!(
                "{} {} {} ",
                (base[0] * shade).min(255.0) as u8,
                (base[1] * shade).min(255.0) as u8,
                (base[2] * shade).min(255.0) as u8
            ));
        }
        out.push('\n');
    }
    std::fs::write(path, out).expect("write ppm");
}

fn main() {
    let seed: u32 = std::env::var("HR_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1337);
    let ticks: u64 = std::env::var("HR_TICKS").ok().and_then(|v| v.parse().ok()).unwrap_or(1000);
    let butterfly = std::env::var("HR_BUTTERFLY").ok().as_deref() == Some("1");
    let outdir = std::env::var("HR_OUT").unwrap_or_else(|_| ".".into());

    let t0 = std::time::Instant::now();
    let a = run(seed, ticks, None);
    let base_ms = t0.elapsed().as_millis();

    let mut divergence_tick: i64 = -1;
    let mut div_curve = String::new();
    if butterfly {
        let b = run(seed, ticks, Some(ticks / 4));
        for (i, (x, y)) in a.digests.iter().zip(b.digests.iter()).enumerate() {
            if x != y {
                divergence_tick = i as i64;
                break;
            }
        }
        // hamming-of-digests curve sampled every ticks/40
        let stridec = (ticks / 40).max(1) as usize;
        for i in (0..a.digests.len()).step_by(stridec) {
            let d = (a.digests[i] ^ b.digests[i]).count_ones();
            div_curve.push(if d == 0 { '.' } else if d < 8 { '+' } else { '#' });
        }
    }

    // coverage matrix
    let names = ["plant_lifecycle", "leaf_decay", "reef_seeder"];
    let parts: Vec<String> = names.iter().enumerate().map(|(i, n)| format!(
        "    {{\"handler\": \"{}\", \"fired_ticks\": {}, \"of\": {}}}",
        n, a.handler_fire_ticks.get(i).copied().unwrap_or(0), ticks
    )).collect();
    let handler_cov = parts.join(",\n") + "\n";
    let report = format!(
        "{{\n  \"seed\": {seed},\n  \"ticks\": {ticks},\n  \"wall_ms\": {base_ms},\n  \"ticks_per_sec\": {:.0},\n  \"edits_total\": {},\n  \"edits_by_cat\": {{\"block\": {}, \"state\": {}, \"transition\": {}, \"tree_root\": {}}},\n  \"coral_blocks\": {},\n  \"final_pos\": [{:.1}, {:.1}, {:.1}],\n  \"biomes_visited\": {},\n  \"handler_coverage\": [\n{}  ],\n  \"butterfly\": {{\"enabled\": {}, \"flip_at\": {}, \"divergence_tick\": {}, \"curve\": \"{}\"}}\n}}\n",
        ticks as f64 / (base_ms.max(1) as f64 / 1000.0),
        a.edits_total,
        a.cat[0], a.cat[1], a.cat[2], a.cat[3],
        a.corals,
        a.final_pos[0], a.final_pos[1], a.final_pos[2],
        a.biomes_seen.len(),
        handler_cov,
        butterfly, ticks / 4, divergence_tick, div_curve
    );
    let jpath = format!("{outdir}/headless_report_seed{seed}.json");
    std::fs::write(&jpath, &report).expect("write report");
    topdown_ppm(seed, &format!("{outdir}/world_topdown_seed{seed}.ppm"));
    println!("{report}");
    println!("wrote {jpath} + world_topdown_seed{seed}.ppm");
}
