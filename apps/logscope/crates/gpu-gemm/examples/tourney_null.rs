//! tourney_null.rs — run the layer tournament on 32 arms that all carry nothing.
//!
//! Answers one question: does successive halving over layers tell you a layer is real?
//!
//! Run: `cargo run --example tourney_null`

use gpu_kernels::probe::{LayerSweep, Lcg};
use gpu_kernels::tourney::{expected_null_survivors, null_survival, Tournament};

fn captures(layer: usize, d: usize, n: usize, effect: f32, rng: &mut Lcg) -> LayerSweep {
    let mut s = LayerSweep::new(layer, d);
    for i in 0..n {
        let ok = i % 2 == 0;
        let mut h: Vec<f32> = (0..d).map(|_| rng.normal()).collect();
        h[0] += if ok { effect } else { -effect };
        s.push(i, &h, ok);
    }
    s
}

fn main() {
    let d: usize = std::env::var("D").ok().and_then(|s| s.parse().ok()).unwrap_or(1024);
    let n: usize = std::env::var("N").ok().and_then(|s| s.parse().ok()).unwrap_or(32);
    let reps: usize = std::env::var("REPS").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    let perms: usize = std::env::var("PERMS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    // Set SIGNAL_LAYER to plant a real effect at one layer and watch the tournament find it.
    let signal_layer: i64 = std::env::var("SIGNAL_LAYER").ok().and_then(|s| s.parse().ok()).unwrap_or(-1);
    let effect: f32 = std::env::var("EFFECT").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let keep: f32 = std::env::var("KEEP").ok().and_then(|s| s.parse().ok()).unwrap_or(0.5);
    let floor: usize = std::env::var("FLOOR").ok().and_then(|s| s.parse().ok()).unwrap_or(2);

    println!("layer tournament: 32 arms, d={d} n={n} per round, keep={keep} floor={floor}, reps={reps}");
    if signal_layer >= 0 { println!("planted effect {effect} at layer {signal_layer}"); }
    else { println!("NO signal at any layer"); }
    println!();

    let (mut champions, mut concluded, mut champ_is_planted) = (0usize, 0usize, 0usize);
    let mut rounds_taken = 0usize;

    for rep in 0..reps {
        let mut rng = Lcg::new(0xA11CE + rep as u64 * 7919);
        let mut t = Tournament::new(0..32usize, keep, floor);
        while !t.finished() {
            // FRESH captures every round — survivors are never re-scored on the rows that kept them.
            let scores: Vec<(usize, f32)> = t.alive().iter().map(|l| {
                let e = if *l as i64 == signal_layer { effect } else { 0.0 };
                let sw = captures(*l, d, n, e, &mut rng);
                (*l, sw.evaluate(4, 0, 1).map(|r| r.cv_auc).unwrap_or(0.5))
            }).collect();
            t.settle(&scores).unwrap();
        }
        rounds_taken = t.cuts().len();
        let live = t.alive();
        champions += 1; // a tournament ALWAYS ends with survivors
        if live.iter().any(|l| *l as i64 == signal_layer) { champ_is_planted += 1; }

        // Fresh-data null on the leading survivor.
        let best = t.leader().unwrap();
        let e = if best as i64 == signal_layer { effect } else { 0.0 };
        let fresh = captures(best, d, n, e, &mut rng).evaluate(4, perms, 31).unwrap();
        if t.conclude(&fresh).is_ok() { concluded += 1; }
    }

    println!("tournaments that produced survivors      : {champions}/{reps}  (100% by construction)");
    if signal_layer >= 0 {
        println!("survivors included the planted layer     : {champ_is_planted}/{reps}");
    }
    println!("survivors that passed the FRESH-data null: {concluded}/{reps}");
    println!("readbacks per tournament (32 branches)   : {} vs {} for a flat sweep",
             Tournament::new(0..32usize, keep, floor).downloads_projected(32).0,
             Tournament::new(0..32usize, keep, floor).downloads_projected(32).1);
    println!();
    println!("after {rounds_taken} cuts at keep={keep} from 32 arms:");
    println!("  P(a given signal-free arm survives) = {:.4}", null_survival(keep, rounds_taken));
    println!("  expected signal-free survivors      = {:.2}",
             expected_null_survivors(32, keep, rounds_taken));
}
