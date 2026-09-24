//! `roar-render` — hear a dragon's roar without running the game.
//!
//! Renders one dragon's roar (by genome seed) through the same chain the game
//! uses (`roar_synth` → `land_echo` → `sound_render`) over a few synthetic
//! landscapes, and writes an Ogg-Opus file per scene. Prints the echo plan for each, so
//! what you hear can be checked against what the model decided.
//!
//!     roar-render [OUT_DIR] [GENOME_SEED] [DIST_M]
//!
//! Defaults: `roar_renders/`, seed 7, 90 m. Scenes: open plain; a cliff 150 m
//! off to one side; a walled valley; a ridge between you and the dragon; and
//! the dry roar alone.

use unendlich_engine::sim::dragon_archetype::{self as arche, Genome};
use unendlich_engine::sim::{land_echo, roar_synth, sound_render};

const RATE: u32 = 24_000;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().unwrap_or_else(|| "roar_renders".into()));
    let seed: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(7);
    let dist: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(90.0);
    std::fs::create_dir_all(&out).expect("create output dir");

    let g = Genome::from_seed(seed);
    let a = arche::classify(&g);
    let v = roar_synth::RoarVoice::from_genome(&g);
    println!(
        "dragon seed {seed}: {} {:?} size {} ({:.0} kg) — roar {:.0} Hz, {:.1} s, growl {:.2}, formant {:.0} Hz, brightness {:.2}",
        arche::ELEMENT_NAMES[v.element as usize % 6], a.build, a.size_class, v.mass_kg,
        v.pitch_hz, v.duration_s, v.growl, v.formant_hz, v.brightness
    );
    let t = std::time::Instant::now();
    let dry = roar_synth::render(&v, RATE, seed);
    let synth_ms = t.elapsed().as_secs_f32() * 1000.0;
    write(&out.join("0_dry.opus"), &dry.iter().flat_map(|s| [*s, *s]).collect::<Vec<_>>());
    println!("  0_dry.opus: the roar at the dragon, no propagation (zero-cross {:.0}/s) — synth {synth_ms:.1} ms", roar_synth::zero_cross_rate(&dry, RATE));

    // Listener at the origin facing +x; the dragon ahead-left on the plain.
    let listener = [0.0, 61.6, 0.0];
    let fwd = [1.0, 0.0, 0.0];
    let src = [dist * 0.8, 70.0, dist * 0.6];
    type H = Box<dyn Fn(i32, i32) -> Option<f32>>;
    let scenes: Vec<(&str, H)> = vec![
        ("1_open_plain", Box::new(|_, _| Some(60.0))),
        ("2_cliff_150m", Box::new(|_, z| Some(if z <= -150 { 110.0 } else { 60.0 }))),
        ("3_walled_valley", Box::new(|_, z| Some(if z.abs() > 70 { 115.0 } else { 60.0 }))),
        ("4_ridge_between", Box::new(move |x, _| Some(if (x as f32 - dist * 0.4).abs() < 5.0 { 88.0 } else { 60.0 }))),
    ];
    for (name, h) in scenes {
        let t = std::time::Instant::now();
        let params = land_echo::EchoParams { ref_dist: 20.0, ..land_echo::EchoParams::default() };
        let plan = land_echo::plan(src, listener, fwd, &|x, z| h(x, z), &params);
        let plan_ms = t.elapsed().as_secs_f32() * 1000.0;
        let t = std::time::Instant::now();
        let wet = sound_render::render(&dry, RATE, &plan, sound_render::RenderOpts { trim_lead: true, lead_keep_s: 0.15, ..sound_render::RenderOpts::default() });
        let render_ms = t.elapsed().as_secs_f32() * 1000.0;
        write(&out.join(format!("{name}.opus")), &wet);
        println!("  {name}.opus: {} — plan {plan_ms:.2} ms (synthetic terrain), render {render_ms:.1} ms", land_echo::describe(&plan));
    }
}

fn write(path: &std::path::Path, stereo: &[f32]) {
    let bytes = unendlich_engine::audio::encode_ogg_opus(stereo, 2, RATE).expect("opus encode");
    std::fs::write(path, bytes).expect("write opus");
}
