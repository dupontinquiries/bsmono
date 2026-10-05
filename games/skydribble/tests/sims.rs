//! The `--sim <name>` subcommands are this game's verification story (README, "The sim, the
//! striker, and the games centre"), and until now none of them ran under `cargo test`: the
//! suite was one WGSL check, and a sim that printed FAIL exited 0 like any other run, so a
//! regression in the physics, the bots, the menus or the loopback netcode showed up only when
//! somebody ran the sim by hand and read the line.
//!
//! Each test here runs the built binary with `--sim <name>` (cargo hands integration tests the
//! path as `CARGO_BIN_EXE_skydribble`) and checks THREE things: the sim's own `LABEL: PASS`
//! line is present, no `: FAIL` line is, and the process exit status agrees with the line
//! (`sim::verdict` records a FAIL and `main` exits 1). The status check is what makes the line
//! meaningful to a CI step that does not grep.
//!
//! Only the sims that are deterministic and finish in seconds are run by default; the
//! bot-play benchmarks (`all`, `air1`...) have no verdict and the long ones are `#[ignore]`d
//! with the reason, so `cargo test -- --ignored` still reaches them.

use std::process::Command;

fn run_sim(name: &str) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_skydribble"))
        .args(["--sim", name])
        .output()
        .unwrap_or_else(|e| panic!("could not spawn the skydribble binary for --sim {name}: {e}"));
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

/// `--sim name` must print `label: PASS`, nothing `: FAIL`, and exit 0.
fn passes(name: &str, label: &str) {
    let (ok, text) = run_sim(name);
    let pass_line = format!("{label}: PASS");
    assert!(
        text.contains(&pass_line),
        "--sim {name} did not report `{pass_line}`\n--- output ---\n{text}"
    );
    let fails: Vec<&str> = text.lines().filter(|l| l.contains(": FAIL")).collect();
    assert!(fails.is_empty(), "--sim {name} reported FAIL: {fails:?}\n--- output ---\n{text}");
    assert!(ok, "--sim {name} printed `{pass_line}` but exited non-zero\n--- output ---\n{text}");
}

macro_rules! sims {
    ($($test:ident => $name:literal, $label:literal;)*) => {
        $(
            #[test]
            fn $test() {
                passes($name, $label);
            }
        )*
    };
}

sims! {
    sim_plat => "plat", "PLATFORM+TOUCH";
    sim_carsdf => "carsdf", "CAR SDF";
    sim_predict => "predict", "PREDICT";
    sim_aerial => "aerial", "AERIAL";
    sim_redirect => "redirect", "REDIRECT";
    sim_carry => "carry", "CARRY";
    sim_hits => "hits", "HITS";
    sim_flick => "flick", "FLICK";
    sim_academy => "academy", "ACADEMY";
    sim_replay => "replay", "REPLAY";
    sim_ballspin => "ballspin", "BALL SPIN";
    sim_spawn => "spawn", "SPAWNS";
    sim_kuiper => "kuiper", "KUIPER";
    sim_planets => "planets", "PLANETS";
    sim_portal => "portal", "PORTALS";
    sim_platform => "platform", "PLATFORMS";
    sim_fps => "fps", "FPS";
    sim_menus => "menus", "MENUS";
    sim_garage => "garage", "GARAGE";
    sim_keys => "keys", "KEYS";
    sim_hover => "hover", "HOVER";
    sim_parts => "parts", "PARTS";
    sim_glb => "glb", "GLB";
    sim_cars => "cars", "CARS";
    sim_loop => "loop", "LOOPS";
    sim_touch => "touch", "TOUCH MENUS";
    sim_gyro => "gyro", "GYRO";
    sim_sanity => "sanity", "SANITY";
    sim_jumpflip => "jumpflip", "JUMPFLIP";
    sim_flip => "flip", "FLIPS";
    sim_mech => "mech", "MECHANICS";
    sim_mp => "mp", "MP LOOPBACK";
    sim_stream => "stream", "STREAM";
    sim_shell => "shell", "SHELL";
    sim_march => "march", "MARCH";
    sim_trials => "trials", "TRIALS";
    sim_render => "render", "RENDER CULL";
}

/// The exit status is the part a CI step reads, so it is checked on its own: an unknown
/// scenario name is not a FAIL (the bot benchmark prints "unknown scenario" and moves on),
/// and a sim that passes exits 0. Breaking `main`'s exit path fails this, not just the
/// per-sim tests above.
#[test]
fn a_passing_sim_exits_zero_and_the_verdict_line_is_the_last_word() {
    let (ok, text) = run_sim("carsdf");
    assert!(ok, "{text}");
    let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    assert_eq!(last, "CAR SDF: PASS", "the verdict closes the run");
}
