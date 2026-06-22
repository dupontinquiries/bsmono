//! Rollback engine over the deterministic [`super::state`] simulation.
//!
//! The problem rollback solves: in a peer-to-peer session, a peer must keep
//! simulating *now* even though remote players' inputs for the current tick
//! haven't arrived yet. So it **predicts** the missing inputs (repeat each
//! player's last confirmed input) and advances. When the real remote inputs
//! arrive — for a tick already simulated with a guess — the engine
//! **rolls back** to that tick and **re-simulates** forward with the
//! corrected inputs. Because [`step`] is pure, the re-simulated result is
//! exactly what a peer who'd had all inputs on time would have computed.
//!
//! Model used here (chosen for being obviously correct rather than maximally
//! efficient): we keep a single *trusted baseline* — the state at
//! `baseline_tick`, into which every input for earlier ticks has been baked
//! because they're all confirmed. The visible "current" state is always
//! `baseline → current_tick` re-simulated using known-or-predicted inputs.
//! As inputs confirm, the baseline marches forward and old inputs are
//! pruned, so a resim only ever spans the prediction window. A correction to
//! a predicted past tick simply changes what the next resim produces — the
//! rollback is implicit and can't be gotten subtly wrong.
//!
//! Determinism note: this engine performs *no* arithmetic of its own beyond
//! tick bookkeeping; all state changes flow through [`step`], so it inherits
//! the simulation's bit-exact determinism.

use std::collections::BTreeMap;

use super::physics::WorldQuery;
use super::state::{checksum, step, Input, SimState};

/// Index of a player within [`SimState::players`].
pub type PlayerId = usize;

/// Drives a deterministic simulation with input prediction + rollback.
pub struct Rollback {
    num_players: usize,
    /// State at `baseline_tick`; all inputs for earlier ticks are baked in.
    baseline_state: SimState,
    baseline_tick: u64,
    /// Furthest tick the simulation has advanced to (with prediction).
    current_tick: u64,
    /// Cached state at `current_tick` (= baseline resimmed forward).
    current: SimState,
    /// Known inputs for ticks `>= baseline_tick`. `inputs[t][p]` is `Some`
    /// once player `p`'s real input for tick `t` is known.
    inputs: BTreeMap<u64, Vec<Option<Input>>>,
    /// Each player's last confirmed input — the prediction base.
    last_input: Vec<Input>,
    /// How far prediction may outrun the confirmed baseline before
    /// [`advance`](Rollback::advance) refuses to go further.
    max_prediction: u64,
}

impl Rollback {
    /// Create an engine starting from `initial` (typically tick 0).
    pub fn new(num_players: usize, initial: SimState, max_prediction: u64) -> Rollback {
        let baseline_tick = initial.tick.0;
        Rollback {
            num_players,
            current: initial.clone(),
            baseline_state: initial,
            baseline_tick,
            current_tick: baseline_tick,
            inputs: BTreeMap::new(),
            last_input: vec![Input::default(); num_players],
            max_prediction,
        }
    }

    /// Record a (possibly late) input for a specific tick and player.
    ///
    /// Inputs for ticks already baked into the baseline are ignored as
    /// stale. After recording, the baseline advances over any newly-complete
    /// ticks and the visible state is refreshed — so a correction to a
    /// mispredicted tick takes effect immediately. `world` is the read-only
    /// terrain the resim collides against (the same on every peer).
    pub fn set_input<W: WorldQuery>(
        &mut self,
        tick: u64,
        player: PlayerId,
        input: Input,
        world: &W,
    ) {
        if tick < self.baseline_tick || player >= self.num_players {
            return;
        }
        let n = self.num_players;
        let slot = self
            .inputs
            .entry(tick)
            .or_insert_with(|| vec![None; n]);
        slot[player] = Some(input);

        self.advance_baseline(world);
        self.resim(world);
    }

    /// Convenience: set the local player's input for the current tick.
    pub fn set_local_input<W: WorldQuery>(&mut self, local: PlayerId, input: Input, world: &W) {
        self.set_input(self.current_tick, local, input, world);
    }

    /// Step the simulation forward one tick, predicting any inputs not yet
    /// known. Returns `false` (and does nothing) if prediction would exceed
    /// `max_prediction` ticks ahead of the confirmed baseline — the caller
    /// should stall a frame and wait for remote inputs.
    pub fn advance<W: WorldQuery>(&mut self, world: &W) -> bool {
        if self.current_tick - self.baseline_tick >= self.max_prediction {
            return false;
        }
        self.current_tick += 1;
        self.advance_baseline(world);
        self.resim(world);
        true
    }

    /// The current (possibly predicted) state.
    pub fn state(&self) -> &SimState {
        &self.current
    }

    /// The tick the current state is at.
    pub fn current_tick(&self) -> u64 {
        self.current_tick
    }

    /// First tick whose inputs are *not* all confirmed. Everything before it
    /// is final and will never roll back.
    pub fn confirmed_tick(&self) -> u64 {
        self.baseline_tick
    }

    /// Checksum of the current state (for desync detection / tests).
    pub fn checksum(&self) -> u64 {
        checksum(&self.current)
    }

    /// Checksum of the *confirmed* baseline state (the final, never-rolled-back
    /// state at [`confirmed_tick`](Rollback::confirmed_tick)). Peers exchange
    /// this — not the predicted [`checksum`](Rollback::checksum) — to detect
    /// divergence, since only confirmed ticks are guaranteed identical.
    pub fn confirmed_checksum(&self) -> u64 {
        checksum(&self.baseline_state)
    }

    // ── internals ──────────────────────────────────────────────────────

    /// Inputs to feed [`step`] for `tick`: each player's known input if
    /// present, otherwise the prediction (their last confirmed input).
    fn gather(&self, tick: u64) -> Vec<Input> {
        let known = self.inputs.get(&tick);
        (0..self.num_players)
            .map(|p| {
                known
                    .and_then(|v| v[p])
                    .unwrap_or(self.last_input[p])
            })
            .collect()
    }

    /// Bake every contiguous fully-known tick into the baseline, pruning its
    /// inputs and updating the prediction base.
    fn advance_baseline<W: WorldQuery>(&mut self, world: &W) {
        while self.baseline_tick < self.current_tick {
            let all_known = self
                .inputs
                .get(&self.baseline_tick)
                .map_or(false, |v| v.iter().all(|o| o.is_some()));
            if !all_known {
                break;
            }
            let ins: Vec<Input> = self.inputs[&self.baseline_tick]
                .iter()
                .map(|o| o.expect("all_known checked above"))
                .collect();
            for (p, i) in ins.iter().enumerate() {
                self.last_input[p] = *i;
            }
            self.baseline_state = step(&self.baseline_state, &ins, world);
            self.inputs.remove(&self.baseline_tick);
            self.baseline_tick += 1;
        }
    }

    /// Recompute `current` by resimulating from the baseline to `current_tick`.
    fn resim<W: WorldQuery>(&mut self, world: &W) {
        let mut s = self.baseline_state.clone();
        let mut t = self.baseline_tick;
        while t < self.current_tick {
            let ins = self.gather(t);
            s = step(&s, &ins, world);
            t += 1;
        }
        self.current = s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixed::Fixed;
    use crate::sim::physics::FlatWorld;
    use crate::sim::state::BTN_JUMP;

    fn mv(mx: i64, mz: i64, buttons: u32) -> Input {
        Input {
            buttons,
            move_x: Fixed::from_int(mx),
            move_z: Fixed::from_int(mz),
            yaw: 0,
        }
    }

    /// Reference: simulate a full known input log offline, no prediction.
    fn offline(seqs: &[Vec<Input>], ticks: usize) -> u64 {
        let w = FlatWorld::new();
        let n = seqs.len();
        let mut s = SimState::new(n);
        for t in 0..ticks {
            let ins: Vec<Input> = seqs.iter().map(|s| s[t]).collect();
            s = step(&s, &ins, &w);
        }
        checksum(&s)
    }

    #[test]
    fn no_delay_matches_offline_and_confirms_fully() {
        let w = FlatWorld::new();
        let ticks = 200;
        let p0: Vec<Input> = (0..ticks).map(|t| mv(1, 0, if t % 30 == 0 { BTN_JUMP } else { 0 })).collect();
        let p1: Vec<Input> = (0..ticks).map(|t| mv(0, if (t / 10) % 2 == 0 { 1 } else { -1 }, 0)).collect();

        let mut rb = Rollback::new(2, SimState::new(2), 16);
        for t in 0..ticks {
            rb.set_input(t as u64, 0, p0[t], &w);
            rb.set_input(t as u64, 1, p1[t], &w);
            assert!(rb.advance(&w));
        }
        assert_eq!(rb.current_tick(), ticks as u64);
        assert_eq!(rb.confirmed_tick(), ticks as u64, "all inputs known → fully confirmed");
        assert_eq!(rb.checksum(), offline(&[p0, p1], ticks));
    }

    #[test]
    fn delayed_remote_input_with_misprediction_still_matches_offline() {
        // Player 1's inputs arrive DELAY ticks late, so the engine predicts
        // (repeat-last) and is frequently WRONG at direction changes —
        // forcing rollbacks. Final state must still equal the offline run.
        const DELAY: usize = 5;
        let ticks = 300;
        let p0: Vec<Input> = (0..ticks).map(|t| mv(1, 0, if t % 25 == 0 { BTN_JUMP } else { 0 })).collect();
        // A zig-zag that changes often, so "repeat last input" mispredicts.
        let p1: Vec<Input> = (0..ticks).map(|t| mv(if (t / 3) % 2 == 0 { 1 } else { -1 }, 0, 0)).collect();

        let mut rb = Rollback::new(2, SimState::new(2), (DELAY + 4) as u64);
        let w = FlatWorld::new();
        for t in 0..ticks {
            rb.set_input(t as u64, 0, p0[t], &w); // local, on time
            if t >= DELAY {
                rb.set_input((t - DELAY) as u64, 1, p1[t - DELAY], &w); // remote, late
            }
            assert!(rb.advance(&w), "prediction window should be large enough");
        }
        // Flush the still-outstanding tail of player 1's inputs.
        for t in ticks.saturating_sub(DELAY)..ticks {
            rb.set_input(t as u64, 1, p1[t], &w);
        }

        assert_eq!(rb.confirmed_tick(), ticks as u64, "everything now confirmed");
        assert_eq!(
            rb.checksum(),
            offline(&[p0, p1], ticks),
            "rollback-resim must reproduce the all-inputs-on-time result"
        );
    }

    #[test]
    fn advance_refuses_past_the_prediction_window() {
        // With no inputs ever confirmed for the remote, the baseline can't
        // advance, so prediction maxes out and advance() must stall.
        let mut rb = Rollback::new(2, SimState::new(2), 4);
        let w = FlatWorld::new();
        // Only player 0 ever provides input; player 1 stays unknown.
        let mut advanced = 0;
        for t in 0..20 {
            rb.set_input(t, 0, mv(1, 0, 0), &w);
            if rb.advance(&w) {
                advanced += 1;
            }
        }
        assert_eq!(advanced, 4, "should advance exactly max_prediction ticks then stall");
        assert_eq!(rb.current_tick(), 4);
    }

    #[test]
    fn correcting_a_prediction_changes_the_visible_state() {
        // Predict player 1 idle, then reveal they actually moved — the
        // visible state must differ from the mispredicted one.
        let mut rb = Rollback::new(2, SimState::new(2), 16);
        let w = FlatWorld::new();
        for t in 0..10u64 {
            rb.set_input(t, 0, mv(0, 0, 0), &w);
            rb.advance(&w);
        }
        let mispredicted = rb.checksum();
        // Now reveal player 1 was sprinting forward the whole time.
        for t in 0..10u64 {
            rb.set_input(t, 1, mv(1, 0, 0), &w);
        }
        assert_ne!(rb.checksum(), mispredicted, "correction should alter state");
        assert_eq!(rb.confirmed_tick(), 10);
    }

    #[test]
    fn two_peers_with_different_arrival_orders_converge() {
        // The guarantee netcode actually leans on: two peers that receive the
        // SAME input log in DIFFERENT arrival orders and timings must reach a
        // byte-identical confirmed state. If they didn't, confirmed_checksum
        // desync detection would false-positive and clients would drift apart.
        let w = FlatWorld::new();
        let ticks = 200usize;
        let p0: Vec<Input> =
            (0..ticks).map(|t| mv(1, 0, if t % 20 == 0 { BTN_JUMP } else { 0 })).collect();
        let p1: Vec<Input> =
            (0..ticks).map(|t| mv(if (t / 4) % 2 == 0 { 1 } else { -1 }, 0, 0)).collect();

        let max_pred = 24u64;

        // Peer A: local = player 0 on time; player 1 arrives DA ticks late.
        let mut a = Rollback::new(2, SimState::new(2), max_pred);
        // Peer B: local = player 1 on time; player 0 arrives DB ticks late —
        // a different cadence, so the two peers predict-and-correct on
        // opposite players at different ticks.
        let mut b = Rollback::new(2, SimState::new(2), max_pred);

        const DA: usize = 6;
        const DB: usize = 3;
        for t in 0..ticks {
            a.set_input(t as u64, 0, p0[t], &w);
            if t >= DA {
                a.set_input((t - DA) as u64, 1, p1[t - DA], &w);
            }
            a.advance(&w);

            b.set_input(t as u64, 1, p1[t], &w);
            if t >= DB {
                b.set_input((t - DB) as u64, 0, p0[t - DB], &w);
            }
            b.advance(&w);
        }
        // Flush each peer's still-outstanding remote tail.
        for t in ticks.saturating_sub(DA)..ticks {
            a.set_input(t as u64, 1, p1[t], &w);
        }
        for t in ticks.saturating_sub(DB)..ticks {
            b.set_input(t as u64, 0, p0[t], &w);
        }

        let off = offline(&[p0, p1], ticks);
        assert_eq!(a.confirmed_tick(), ticks as u64, "peer A fully confirmed");
        assert_eq!(b.confirmed_tick(), ticks as u64, "peer B fully confirmed");
        assert_eq!(a.confirmed_checksum(), off, "peer A confirmed == offline");
        assert_eq!(b.confirmed_checksum(), off, "peer B confirmed == offline");
        assert_eq!(
            a.confirmed_checksum(),
            b.confirmed_checksum(),
            "the two peers must agree on the confirmed state"
        );
    }
}
