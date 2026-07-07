//! enhance_report — validate that the recursive enhancer actually makes maps more cinematic.
//!
//! For each map it compares the base against enhanced versions at several tuning levels, reporting BOTH the
//! static objective the hill-climb optimises AND the real cinematic-liveliness score from the full Game
//! simulation harness — the independent arbiter. If enhancement works, the real liveliness should climb (or at
//! least not regress) as tuning rises, confirming the static proxy is steering the map the right way.
//!
//! Usage: cargo run --example enhance_report --release [-- <name-substr>]

use early_fps::camera::FpsCam;
use early_fps::enhance::{cinematic_objective, enhance, CinematicTuning};
use early_fps::game::Game;
use early_fps::level::{Level, PRESETS};
use early_fps::liveliness::analyze_game;
use early_fps::loadout::Loadout;
use early_fps::math::Vec3;

fn real_liveliness(level_seed: u64, preset_idx: usize, tuning: f32) -> f32 {
    // build a game whose level has been enhanced at `tuning`, then run the real liveliness harness on it
    let mut game = Game::new_match_tuned(
        level_seed,
        Some(preset_idx),
        Loadout::default(),
        Default::default(),
        12,
        early_fps::enemy::Difficulty::Regular,
        None,
        early_fps::creatures::HuntRole::Predator,
        CinematicTuning(tuning),
    );
    let lvl = &game.level;
    let c = Vec3::new(lvl.w as f32 * lvl.ts() * 0.5, lvl.floor_y + 2.0, lvl.h as f32 * lvl.ts() * 0.5);
    let cam = FpsCam { pos: c, yaw: 0.7, pitch: -0.08 };
    analyze_game(&mut game, &cam, 360).score
}

fn main() {
    let filter: Option<String> = std::env::args().skip(1).find(|a| !a.starts_with("--")).map(|s| s.to_lowercase());
    let seed = 7u64;
    let tunings = [0.0f32, 0.5, 1.0];

    println!("Recursive cinematic enhancer — objective (static) and real liveliness (Game sim) vs tuning\n");
    println!(
        "{:<20} {:>18} {:>28}",
        "MAP", "objective 0/.5/1", "real liveliness 0/.5/1"
    );
    println!("{}", "─".repeat(70));

    let names = ["Concrete Slab", "Flood Zone", "Redwood Canyon", "Malecon Breakwater", "Desert Town"];
    for (pi, p) in PRESETS.iter().enumerate() {
        if !names.contains(&p.name) {
            continue;
        }
        if let Some(f) = &filter {
            if !p.name.to_lowercase().contains(f.as_str()) {
                continue;
            }
        }

        // static objective at each tuning
        let mut objs = [0f32; 3];
        for (k, &t) in tunings.iter().enumerate() {
            let mut lvl = Level::generate(p.config(seed));
            enhance(&mut lvl, CinematicTuning(t), seed, 60);
            objs[k] = cinematic_objective(&lvl);
        }
        // real liveliness at each tuning
        let mut live = [0f32; 3];
        for (k, &t) in tunings.iter().enumerate() {
            live[k] = real_liveliness(seed, pi, t);
        }

        println!(
            "{:<20} {:>5.2} {:>5.2} {:>5.2} {:>12.2} {:>5.2} {:>5.2}",
            p.name, objs[0], objs[1], objs[2], live[0], live[1], live[2]
        );
    }

    println!("{}", "─".repeat(70));
    println!("objective = static hill-climb target; real liveliness = full-sim cinematic score (independent).");
    println!("A working enhancer lifts both from tuning 0 → 1 (or holds them steady where a map is already rich).");
}
