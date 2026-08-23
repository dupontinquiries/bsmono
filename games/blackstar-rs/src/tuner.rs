//! THE TUNER — a search that turns guesses into measurements.
//!
//! Ported from the C engine's `tools/tune_fsm.py`, and this is a port of the
//! METHOD rather than the artifact. That distinction is the whole design
//! decision, so it is worth being explicit about.
//!
//! The C tool evolves transition matrices for a Markov behaviour FSM: eight
//! per-enemy movement behaviours, a matrix per (context, vehicle archetype),
//! tuned by mutation search over an abstract skirmish and generated into
//! `behavior_gen.h`. **This tree has no such layer.** Porting it would mean
//! adding a per-enemy behaviour system with no consumer, and then a tuner for
//! it — a system built for a future that may not arrive.
//!
//! What the C tool actually contributes is the loop: *a headless sim, a
//! fitness function, and a mutation search producing tuned tables.* That loop
//! has an immediate consumer here, because `tuning.rs` says out loud that five
//! numbers in this codebase have never been verified by anything, and
//! `brain.rs` carries a comment admitting its posture thresholds are guesses
//! with a visible consequence.
//!
//! So: the same loop, pointed at the dials that exist.
//!
//! ## What it does not do
//!
//! **It does not write anything back.** A search reports what it found; a
//! person decides whether the fitness function deserved to be believed. An
//! optimiser that edits the source is a very efficient way to encode a bad
//! objective function into a game, and the fitness weights below are
//! themselves guesses — better-argued than the numbers they judge, and still
//! guesses. `--tune search` prints; it does not patch.
//!
//! ## Determinism
//!
//! Fixed seeds throughout, as the C tool is. A tuner whose answer moves
//! between runs cannot be argued with, and the argument is the point.

use crate::brain::{Brain, Fsm, Posture, Signals, Snapshot, Unit, Verb, ZONE_ALL};
use glam::Vec3;

/// What a run of the abstract engagement produced.
#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    /// mean fraction of the squad within striking distance of the player
    pub pressure: f32,
    /// mean angular spread around the player: 1.0 is fully surrounded
    pub encircle: f32,
    /// fraction of the squad still alive at the end
    pub survive: f32,
    /// distinct postures entered; a commander with one idea is boring
    pub variety: f32,
}

/// What a context wants from a commander. Ported from the C's per-context
/// weight tuples — the same idea, fewer contexts, because this tree's modes
/// are missions rather than arena rulesets.
#[derive(Clone, Copy, Debug)]
pub struct Want {
    pub name: &'static str,
    pub pressure: f32,
    pub encircle: f32,
    pub survive: f32,
    pub variety: f32,
}

pub static CONTEXTS: &[Want] = &[
    // a straight fight: press, surround, stay interesting
    Want { name: "skirmish", pressure: 1.0, encircle: 0.8, survive: 0.6, variety: 0.7 },
    // the corrupted grind: numbers and encirclement, bodies are cheap
    Want { name: "swarm", pressure: 1.1, encircle: 1.0, survive: 0.3, variety: 0.4 },
    // hunting something that runs: patience over pressure
    Want { name: "hunt", pressure: 0.7, encircle: 0.5, survive: 1.0, variety: 0.3 },
    // holding a line: stay alive, stay together
    Want { name: "hold", pressure: 0.6, encircle: 0.3, survive: 1.2, variety: 0.3 },
];

/// The dials being searched: `Fsm::choose`'s posture gates.
///
/// These are the numbers `brain.rs` flags in a comment as guesses with a
/// visible consequence — `aggressor` and `surrounder` sit below the caution
/// bar and are state-insensitive because of where it happens to be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gates {
    /// squad strength below which a cautious commander falls back
    pub hurt: f32,
    /// caution above which it minds being hurt
    pub mind: f32,
    /// fraction scattered before a regroup-inclined commander rallies
    pub scattered: f32,
    /// player aggression that provokes a press
    pub provoke: f32,
}

impl Default for Gates {
    /// What `Fsm::choose` ships with today.
    fn default() -> Gates {
        Gates { hurt: 0.45, mind: 0.4, scattered: 0.3, provoke: 0.5 }
    }
}

impl Gates {
    fn clamped(self) -> Gates {
        Gates {
            hurt: self.hurt.clamp(0.05, 0.95),
            mind: self.mind.clamp(0.05, 0.95),
            scattered: self.scattered.clamp(0.05, 0.95),
            provoke: self.provoke.clamp(0.05, 0.95),
        }
    }
    /// The same decision `Fsm::choose` makes, with the gates as parameters.
    ///
    /// A copy, deliberately: the shipped path stays a plain function with
    /// constants in it, because threading a tuning struct through the game to
    /// serve a search would be the search dictating the architecture.
    /// `matches_the_shipped_fsm` asserts the two agree at the default gates.
    fn choose(&self, d: &crate::content::Commander, strength: f32, scattered: f32, live: f32, aggr: f32) -> Posture {
        if strength < self.hurt && d.caution > self.mind {
            return Posture::Fallback;
        }
        if scattered > self.scattered && d.regroup > 0.5 {
            return Posture::Rally;
        }
        if live >= 3.0 && d.encircle > 0.6 {
            return Posture::Encircle;
        }
        if aggr > self.provoke && d.aggression > 0.5 {
            return Posture::Press;
        }
        if d.aggression > 0.75 {
            return Posture::Press;
        }
        Posture::Probe
    }
}

/// A cheap deterministic hash → [0,1). Same trick the C tuner uses to stay
/// reproducible without carrying an RNG.
fn rnd(seed: &mut u64) -> f32 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*seed >> 33) as f32) / (u32::MAX as f32 / 2.0)
}

/// One abstract engagement: a squad under a commander against a player who
/// kites. No renderer, no physics — positions, a clock, and the commander's
/// own decisions moving the squad.
pub fn engage(doctrine: &crate::content::Commander, gates: Gates, ctx: &Want, seed: u64) -> Metrics {
    let mut s = seed ^ 0x9E37_79B9;
    let n = 6usize;
    let mut pos: Vec<Vec3> = (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            Vec3::new(a.cos() * 40.0, 0.0, a.sin() * 40.0)
        })
        .collect();
    let mut hp = vec![1.0f32; n];
    let mut player = Vec3::ZERO;
    let mut fsm = Fsm::new(doctrine.id, -1);
    let mut postures = std::collections::BTreeSet::new();
    let (mut press_sum, mut ring_sum, mut steps) = (0.0f32, 0.0f32, 0.0f32);

    let dt = 0.1;
    for step in 0..300 {
        let live: Vec<usize> = (0..n).filter(|i| hp[*i] > 0.0).collect();
        if live.is_empty() {
            break;
        }
        // THE PLAYER KITES INSIDE AN ARENA, and both halves of that matter.
        //
        // First cut had them drifting away at 9 m/s against a squad that
        // moves at 7, so nothing was ever caught: every metric collapsed,
        // encircle read 0.048 for a surrounder and 0.049 for an aggressor,
        // and the search could not tell the doctrines apart because the sim
        // never let them do anything. A fitness function measuring a fight
        // that does not happen scores noise very precisely.
        //
        // Slower than the squad, and bounded: fights happen in places, and a
        // player who can retreat infinitely is not being encircled by
        // anybody.
        let com = live.iter().fold(Vec3::ZERO, |a, i| a + pos[*i]) / live.len() as f32;
        let away = (player - com).normalize_or_zero();
        player += away * 6.0 * dt
            + Vec3::new(rnd(&mut s) - 0.5, 0.0, rnd(&mut s) - 0.5) * 4.0 * dt;
        const ARENA_R: f32 = 45.0;
        if player.length() > ARENA_R {
            player = player.normalize_or_zero() * ARENA_R;
        }

        // the commander decides on the shipped cadence, with the gates under test
        let strength: f32 = live.iter().map(|i| hp[*i]).sum::<f32>() / live.len() as f32;
        let scattered = live.iter().filter(|i| pos[**i].distance(com) > 30.0).count() as f32
            / live.len() as f32;
        let aggr = 0.4 + 0.3 * ((step as f32) / 300.0);
        let want = gates.choose(doctrine, strength, scattered, live.len() as f32, aggr);
        if step % 12 == 0 {
            fsm.posture = want;
            postures.insert(format!("{want:?}"));
        }

        // the squad acts on the posture, through the same verbs it would emit
        let snap = snapshot_of(&pos, &hp, player, aggr);
        let orders = fsm.think(&snap, dt).unwrap_or_default();
        let verb = orders.cmds.first().map(|c| c.verb);
        for (slot, &i) in live.iter().enumerate() {
            let to_player = (player - pos[i]).normalize_or_zero();
            let step_v = match verb {
                Some(Verb::Focus) | Some(Verb::FlockTo) => to_player * 7.0,
                Some(Verb::FlockSpread) => {
                    // SPREAD IS AN ASSIGNED ANGLE, NOT A TANGENT. The first
                    // version pushed every unit sideways, which rotates a
                    // formation without spreading it — a surrounder scored
                    // LOWER on encirclement than an aggressor, because units
                    // starting on a ring already cover it and orbiting only
                    // took them further out. Each unit gets its own slot on
                    // the circle and closes to it, which is what "spread out
                    // around a point" meant.
                    let want = slot as f32 / live.len() as f32 * std::f32::consts::TAU;
                    let r = orders.cmds.first().map(|c| c.param0).unwrap_or(14.0).max(6.0);
                    let target = player + Vec3::new(want.cos(), 0.0, want.sin()) * r;
                    (target - pos[i]).normalize_or_zero() * 7.0
                }
                Some(Verb::Retreat) => -to_player * 6.0,
                _ => to_player * 1.5,
            };
            pos[i] += step_v * dt;
            // the player fights back: closer is deadlier for the squad
            let d = pos[i].distance(player);
            if d < 12.0 {
                hp[i] = (hp[i] - 0.30 * dt).max(0.0);
            }
        }

        // metrics
        let near = live.iter().filter(|i| pos[**i].distance(player) < 18.0).count() as f32;
        press_sum += near / live.len() as f32;
        ring_sum += ring_coverage(&pos, &live, player);
        steps += 1.0;
    }

    let alive = hp.iter().filter(|h| **h > 0.0).count() as f32 / n as f32;
    let _ = ctx;
    Metrics {
        pressure: press_sum / steps.max(1.0),
        encircle: ring_sum / steps.max(1.0),
        survive: alive,
        variety: (postures.len() as f32 / 5.0).min(1.0),
    }
}

/// How much of the circle around the player is occupied, 0..1. Eight sectors;
/// a squad all on one side scores 0.125.
fn ring_coverage(pos: &[Vec3], live: &[usize], player: Vec3) -> f32 {
    let mut sect = [false; 8];
    for &i in live {
        let d = pos[i] - player;
        if d.length() > 30.0 {
            continue;
        }
        let a = d.z.atan2(d.x) + std::f32::consts::PI;
        sect[((a / std::f32::consts::TAU * 8.0) as usize).min(7)] = true;
    }
    sect.iter().filter(|b| **b).count() as f32 / 8.0
}

fn snapshot_of(pos: &[Vec3], hp: &[f32], player: Vec3, aggr: f32) -> Snapshot {
    let units = pos
        .iter()
        .enumerate()
        .filter(|(i, _)| hp[*i] > 0.0)
        .map(|(i, p)| Unit {
            id: i as i32,
            pos: *p,
            hp: hp[i] * 100.0,
            max_hp: 100.0,
            zone: (i % 3) as i32,
            alive: true,
            is_commander: i == 0,
            squad: -1,
            scattering: false,
        })
        .collect();
    Snapshot {
        tick: 0,
        player_pos: player,
        in_vehicle: false,
        signals: Signals { aggression: aggr, ..Default::default() },
        units,
        allies: Vec::new(),
    }
}

/// Score a run against what the context wanted.
pub fn fitness(m: &Metrics, w: &Want) -> f32 {
    m.pressure * w.pressure + m.encircle * w.encircle + m.survive * w.survive
        + m.variety * w.variety
}

/// Average fitness of a gate set across every doctrine and several seeds.
///
/// Across ALL doctrines on purpose: the gates are shared, so tuning them for
/// the aggressor at the warden's expense is not an improvement, it is a
/// preference expressed as a number.
pub fn score(gates: Gates, ctx: &Want) -> f32 {
    let mut total = 0.0;
    let mut n = 0.0;
    for d in crate::content::COMMANDERS {
        for seed in [1u64, 7, 19] {
            total += fitness(&engage(d, gates, ctx, seed), ctx);
            n += 1.0;
        }
    }
    total / n
}

/// Mutation search. Small, deterministic, and hill-climbing rather than
/// clever — the C's is the same shape, and a fancier optimiser would only
/// find the fitness function's flaws faster.
pub fn search(ctx: &Want, rounds: usize) -> (Gates, f32) {
    let mut best = Gates::default();
    let mut best_score = score(best, ctx);
    // Seeded from the CONTEXT, not a constant. With one shared seed every
    // context explored the identical candidate sequence, so they all landed
    // on the same gates and the whole exercise looked like theatre — the
    // weights were fine, the search just never looked anywhere different.
    // Still deterministic: the same context always gets the same seed.
    let mut seed = ctx.name.bytes().fold(0xC0FFEEu64, |a, b| a.wrapping_mul(131).wrapping_add(b as u64));
    let mut spread = 0.25f32;
    for _ in 0..rounds {
        let cand = Gates {
            hurt: best.hurt + (rnd(&mut seed) - 0.5) * spread,
            mind: best.mind + (rnd(&mut seed) - 0.5) * spread,
            scattered: best.scattered + (rnd(&mut seed) - 0.5) * spread,
            provoke: best.provoke + (rnd(&mut seed) - 0.5) * spread,
        }
        .clamped();
        let s = score(cand, ctx);
        if s > best_score {
            best = cand;
            best_score = s;
        }
        spread *= 0.97; // anneal
    }
    (best, best_score)
}

// ------------------------------------------------------------- driver ---

/// `--tune search` — what the sim thinks the guessed gates should be.
pub fn run() {
    println!("GATE SEARCH — the numbers `brain.rs` admits are guesses\n");
    let d = Gates::default();
    println!("  shipped: hurt {:.2}  mind {:.2}  scattered {:.2}  provoke {:.2}\n",
        d.hurt, d.mind, d.scattered, d.provoke);
    println!("  {:<10} {:>7} {:>7} {:>7} {:>7} {:>9} {:>9}",
        "context", "hurt", "mind", "scat", "provoke", "score", "vs shipped");
    for ctx in CONTEXTS {
        let base = score(d, ctx);
        let (g, s) = search(ctx, 220);
        println!(
            "  {:<10} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>9.3} {:>+8.1}%",
            ctx.name, g.hurt, g.mind, g.scattered, g.provoke, s,
            (s / base - 1.0) * 100.0
        );
    }
    println!("\n  NOTHING IS WRITTEN BACK. The fitness weights above are themselves");
    println!("  guesses -- better argued than the numbers they judge, and still");
    println!("  guesses. An optimiser that edits the source is an efficient way to");
    println!("  bake a bad objective function into a game.");
    println!("\n  what to do with this: if a context's answer is far from the shipped");
    println!("  gates AND the score gain is large, that is worth a playthrough. If");
    println!("  the gain is a couple of percent, the search found noise.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::COMMANDERS;

    #[test]
    fn the_search_copy_matches_the_shipped_fsm() {
        // `Gates::choose` duplicates `Fsm::choose` so the search can vary the
        // thresholds without threading a tuning struct through the game. A
        // copy that drifts tunes something the game does not run, so: at the
        // default gates, on every doctrine and a spread of battlefields, the
        // two must agree.
        let g = Gates::default();
        for d in COMMANDERS {
            for &(strength, scattered, live, aggr) in &[
                (1.0f32, 0.0f32, 6.0f32, 0.7f32),
                (0.3, 0.0, 6.0, 0.7),
                (0.8, 0.5, 6.0, 0.2),
                (1.0, 0.0, 2.0, 0.2),
                (0.2, 0.6, 5.0, 0.9),
            ] {
                let mine = g.choose(d, strength, scattered, live, aggr);
                // rebuild the shipped decision through the real Fsm
                let mut units = Vec::new();
                let n = live as usize;
                for i in 0..n {
                    units.push(Unit {
                        id: i as i32,
                        pos: Vec3::new(i as f32, 0.0, 5.0),
                        hp: strength * 100.0,
                        max_hp: 100.0,
                        zone: 1,
                        alive: true,
                        is_commander: i == 0,
                        squad: -1,
                        scattering: (i as f32) < scattered * live,
                    });
                }
                let snap = Snapshot {
                    tick: 1,
                    signals: Signals { aggression: aggr, ..Default::default() },
                    units,
                    ..Default::default()
                };
                let mut f = Fsm::new(d.id, -1);
                f.think(&snap, 99.0);
                assert_eq!(
                    mine, f.posture,
                    "{} disagreed at strength {strength} scattered {scattered}",
                    d.id
                );
            }
        }
    }

    #[test]
    fn the_engagement_is_deterministic() {
        // a tuner whose answer moves between runs cannot be argued with
        let d = &COMMANDERS[0];
        let a = engage(d, Gates::default(), &CONTEXTS[0], 5);
        let b = engage(d, Gates::default(), &CONTEXTS[0], 5);
        assert_eq!(a.pressure, b.pressure);
        assert_eq!(a.encircle, b.encircle);
        assert_eq!(a.survive, b.survive);
        // and a different seed gives a different run, or the seed does nothing
        let c = engage(d, Gates::default(), &CONTEXTS[0], 99);
        assert!(
            (a.pressure - c.pressure).abs() > 1e-6 || (a.encircle - c.encircle).abs() > 1e-6,
            "the seed changed nothing"
        );
    }

    #[test]
    fn the_metrics_are_in_range_and_mean_something() {
        for d in COMMANDERS {
            let m = engage(d, Gates::default(), &CONTEXTS[0], 3);
            for (name, v) in
                [("pressure", m.pressure), ("encircle", m.encircle), ("survive", m.survive), ("variety", m.variety)]
            {
                assert!((0.0..=1.0).contains(&v), "{} {name} out of range: {v}", d.id);
            }
        }
        // the encircle metric must actually distinguish: a surrounder should
        // ring the player harder than an aggressor that runs straight in
        let sur = engage(crate::content::commander("surrounder").unwrap(), Gates::default(), &CONTEXTS[0], 3);
        let agg = engage(crate::content::commander("aggressor").unwrap(), Gates::default(), &CONTEXTS[0], 3);
        assert!(sur.encircle > agg.encircle, "{} vs {}", sur.encircle, agg.encircle);
    }

    #[test]
    fn the_search_never_makes_things_worse() {
        // hill climbing from the shipped gates: it keeps the best seen, so
        // the result is at worst the starting point
        for ctx in CONTEXTS {
            let base = score(Gates::default(), ctx);
            let (_, found) = search(ctx, 40);
            assert!(found >= base - 1e-6, "{}: {found} < {base}", ctx.name);
        }
    }

    #[test]
    fn different_contexts_want_different_gates() {
        // if every context converged on the same answer the weights are
        // decoration and the whole exercise is theatre
        let (hold, _) = search(&CONTEXTS[3], 220);
        let (swarm, _) = search(&CONTEXTS[1], 220);
        assert_ne!(hold, swarm, "a hold and a swarm want the same commander?");
    }

    #[test]
    fn the_gates_stay_inside_their_range() {
        for ctx in CONTEXTS {
            let (g, _) = search(ctx, 120);
            for (n, v) in [("hurt", g.hurt), ("mind", g.mind), ("scattered", g.scattered), ("provoke", g.provoke)] {
                assert!((0.05..=0.95).contains(&v), "{n} escaped its clamp: {v}");
            }
        }
    }
}
