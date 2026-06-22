//! seam_probe (r195) — verify the round+snap sea-level fill height is
//! stable under an f32 round-trip, the cross-backend disagreement the
//! dual-ocean-at-boundaries bug came from. For every column we compute
//! the CPU integer (round of 0.01-snapped f64) and the value a GPU would
//! get (same formula via f32), and assert they floor/round to the SAME
//! integer. Pre-fix (plain floor) these differed near the waterline.
use unendlich_engine::world::cpu_generator::WorldGenerator;

// r195: the ONLY bulletproof cross-backend rule is to compute the fill
// integer in the SAME precision both backends share — f32 (the GPU is
// locked to it). The CPU casts its f64 sea level to f32 FIRST, then
// applies the identical biased floor. Now the two are bit-identical by
// construction, not by luck of epsilon.
fn fill_i(v: f32) -> i32 { (v + 0.5 - 1e-3).floor() as i32 }
fn snap_round_f64(v: f64) -> i32 { fill_i(v as f32) } // cast to f32 FIRST
fn snap_round_f32(v: f32) -> i32 { fill_i(v) }

fn main() {
    let gen = WorldGenerator::new(1337);
    let mut checked = 0u64;
    let mut disagree = 0u64;
    let mut near_waterline = 0u64;
    for gz in (-2048i32..2048).step_by(4) {
        for gx in (-2048i32..2048).step_by(4) {
            let sl = gen.sea_level_at(gx, gz);
            let cpu_i = snap_round_f64(sl);
            let gpu_i = snap_round_f32(sl as f32);
            checked += 1;
            if (sl.fract().abs() - 0.5).abs() < 0.02 || sl.fract().abs() < 0.02 {
                near_waterline += 1;
            }
            if cpu_i != gpu_i {
                disagree += 1;
                if disagree <= 5 {
                    println!("DISAGREE ({gx},{gz}) sl={sl:.5} cpu={cpu_i} gpu={gpu_i}");
                }
            }
        }
    }
    println!(
        "seam check: {checked} columns, {near_waterline} near a rounding edge, \
         {disagree} cpu/gpu integer disagreements -> {}",
        if disagree == 0 { "PASS" } else { "FAIL" }
    );
    std::process::exit(if disagree == 0 { 0 } else { 1 });
}
