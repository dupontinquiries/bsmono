//! examples/plant_homogeneity_probe.rs — r190: prove the per-plant stagger's
//! foundation directly against the pub API: advance_n_ticks(s, m, 8) must
//! equal eight advance_one_tick calls, for every (stage-flag, age, moisture)
//! lattice point. The 8x-window stagger is exactly this property applied.
use unendlich_engine::world::chunk::BlockInstanceState;
use unendlich_engine::world::plant_lifecycle::{advance_n_ticks, advance_one_tick};

fn main() {
    let mut checked = 0u64;
    let mut mismatches = 0u64;
    for flags in 0u8..=31 {
        for age in (0u8..=255).step_by(3) {
            for mi in 0..=10 {
                let m = mi as f32 / 10.0;
                let mut a = BlockInstanceState { age_ticks: age, flags };
                let mut b = a;
                let sa = advance_n_ticks(&mut a, m, 8);
                let mut sb = unendlich_engine::world::plant_lifecycle::Stage::from_flags(b.flags);
                for _ in 0..8 {
                    sb = advance_one_tick(&mut b, m);
                }
                checked += 1;
                if a != b || sa as u8 != sb as u8 {
                    mismatches += 1;
                    if mismatches <= 5 {
                        println!(
                            "MISMATCH flags={flags} age={age} m={m}: n8={:?}/{:?} vs 8x1={:?}/{:?}",
                            a, sa, b, sb
                        );
                    }
                }
            }
        }
    }
    println!(
        "time-homogeneity x8: {checked} lattice points, {mismatches} mismatches -> {}",
        if mismatches == 0 { "PASS" } else { "FAIL" }
    );
    std::process::exit(if mismatches == 0 { 0 } else { 1 });
}
