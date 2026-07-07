//! sim::net::client_reconcile — local-player prediction + server reconciliation.
//!
//! The complement to two pieces already here: `snapshot_interp` smooths *remote*
//! entities, and `input_prediction` guesses a *remote* peer's absent inputs for
//! rollback. This is the **local** player's client-side model (Valve/Gaffer
//! style): apply your own inputs to your own state *immediately* so movement
//! feels instant, tag each input with a sequence number, and keep the un-acked
//! ones. When the host's authoritative snapshot arrives — carrying the last input
//! seq it processed — you snap to that authoritative state and **replay** every
//! input the host hasn't seen yet on top of it. If the replay lands where you
//! already predicted, the correction is invisible; if it diverges (a
//! misprediction), the returned error lets the renderer smooth it away.
//!
//! The movement step is a caller-supplied closure `Fn(&State, &Input) -> State`,
//! so this shares the *exact* integrator the host runs (that shared determinism
//! is what makes prediction correct). Pure, deterministic, std-only.

use std::collections::VecDeque;

/// The predicted local transform. `pos` is world-space; `yaw` radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PredState {
    pub pos: [f32; 3],
    pub yaw: f32,
}

impl PredState {
    pub fn new(pos: [f32; 3], yaw: f32) -> PredState {
        PredState { pos, yaw }
    }
    fn dist_to(self, other: PredState) -> f32 {
        let d = [
            self.pos[0] - other.pos[0],
            self.pos[1] - other.pos[1],
            self.pos[2] - other.pos[2],
        ];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    }
}

/// Result of reconciling against an authoritative snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Correction {
    /// The re-predicted local state (authoritative + replayed un-acked inputs).
    pub corrected: PredState,
    /// Positional divergence between the old prediction and the corrected one —
    /// `0` means the host agreed and the correction is invisible. The renderer
    /// can blend this away rather than snapping.
    pub error: f32,
    /// How many un-acked inputs were replayed on top of the authoritative state.
    pub replayed: usize,
}

/// Buffers the local player's un-acked inputs and the current prediction.
#[derive(Clone, Debug)]
pub struct LocalPredictor<I> {
    pending: VecDeque<(u32, I)>,
    predicted: PredState,
    last_acked: u32,
    have_acked: bool,
}

impl<I: Clone> LocalPredictor<I> {
    pub fn new(initial: PredState) -> LocalPredictor<I> {
        LocalPredictor {
            pending: VecDeque::new(),
            predicted: initial,
            last_acked: 0,
            have_acked: false,
        }
    }

    pub fn predicted(&self) -> PredState {
        self.predicted
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Apply one local input immediately: advance the prediction and remember the
    /// input (by `seq`) so it can be replayed after reconciliation. `seq` must be
    /// strictly increasing.
    pub fn apply<F>(&mut self, seq: u32, input: I, step: F) -> PredState
    where
        F: Fn(&PredState, &I) -> PredState,
    {
        // ignore a non-increasing seq (duplicate / reorder)
        if let Some(&(back, _)) = self.pending.back() {
            if seq <= back {
                return self.predicted;
            }
        } else if self.have_acked && seq <= self.last_acked {
            return self.predicted;
        }
        self.predicted = step(&self.predicted, &input);
        self.pending.push_back((seq, input));
        self.predicted
    }

    /// Reconcile against the host's authoritative state, which reflects inputs up
    /// to and including `acked_seq`. Drops acked inputs, replays the rest on top
    /// of `authoritative`, and returns the correction (with the misprediction
    /// error). A stale `acked_seq` (older than one already seen) is ignored.
    pub fn reconcile<F>(&mut self, authoritative: PredState, acked_seq: u32, step: F) -> Correction
    where
        F: Fn(&PredState, &I) -> PredState,
    {
        if self.have_acked && acked_seq < self.last_acked {
            // out-of-order/stale ack: keep current prediction, report no change
            return Correction { corrected: self.predicted, error: 0.0, replayed: self.pending.len() };
        }
        self.last_acked = acked_seq;
        self.have_acked = true;

        // drop every input the host has already applied
        while let Some(&(seq, _)) = self.pending.front() {
            if seq <= acked_seq {
                self.pending.pop_front();
            } else {
                break;
            }
        }

        // replay the un-acked inputs on top of the authoritative state
        let mut state = authoritative;
        for (_, input) in &self.pending {
            state = step(&state, input);
        }

        let old = self.predicted;
        self.predicted = state;
        Correction {
            corrected: state,
            error: old.dist_to(state),
            replayed: self.pending.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A simple deterministic integrator shared by "client" and "server":
    // the input is a per-tick world-space displacement + yaw delta.
    #[derive(Clone, Copy)]
    struct Move {
        d: [f32; 3],
        dyaw: f32,
    }
    fn step(s: &PredState, m: &Move) -> PredState {
        PredState {
            pos: [s.pos[0] + m.d[0], s.pos[1] + m.d[1], s.pos[2] + m.d[2]],
            yaw: s.yaw + m.dyaw,
        }
    }
    fn mv(x: f32) -> Move {
        Move { d: [x, 0.0, 0.0], dyaw: 0.0 }
    }

    #[test]
    fn prediction_advances_immediately() {
        let mut p = LocalPredictor::new(PredState::new([0.0; 3], 0.0));
        p.apply(1, mv(1.0), step);
        p.apply(2, mv(1.0), step);
        assert_eq!(p.predicted().pos[0], 2.0);
        assert_eq!(p.pending_count(), 2);
    }

    #[test]
    fn correct_prediction_reconciles_with_no_error() {
        let mut p = LocalPredictor::new(PredState::new([0.0; 3], 0.0));
        p.apply(1, mv(1.0), step);
        p.apply(2, mv(1.0), step); // predicted x = 2
        // host processed input 1 → authoritative x = 1; input 2 still un-acked
        let c = p.reconcile(PredState::new([1.0, 0.0, 0.0], 0.0), 1, step);
        assert_eq!(c.replayed, 1, "input 2 replayed");
        assert!((c.corrected.pos[0] - 2.0).abs() < 1e-6, "replay reproduces x=2");
        assert!(c.error < 1e-6, "host agreed → invisible correction");
        assert_eq!(p.pending_count(), 1);
    }

    #[test]
    fn misprediction_surfaces_error_and_corrects() {
        let mut p = LocalPredictor::new(PredState::new([0.0; 3], 0.0));
        p.apply(1, mv(1.0), step);
        p.apply(2, mv(1.0), step); // predicted x = 2
        // host says input 1 actually landed at x = 0.5 (blocked/knockback), acked 1
        let c = p.reconcile(PredState::new([0.5, 0.0, 0.0], 0.0), 1, step);
        // corrected = 0.5 + replay(input2 = +1) = 1.5
        assert!((c.corrected.pos[0] - 1.5).abs() < 1e-6);
        assert!((c.error - 0.5).abs() < 1e-6, "0.5 misprediction to smooth");
        assert_eq!(p.predicted().pos[0], 1.5);
    }

    #[test]
    fn all_acked_snaps_to_authoritative() {
        let mut p = LocalPredictor::new(PredState::new([0.0; 3], 0.0));
        p.apply(1, mv(1.0), step);
        p.apply(2, mv(1.0), step);
        let c = p.reconcile(PredState::new([2.0, 0.0, 0.0], 0.0), 2, step);
        assert_eq!(c.replayed, 0, "nothing un-acked");
        assert_eq!(c.corrected.pos[0], 2.0);
        assert_eq!(p.pending_count(), 0);
    }

    #[test]
    fn stale_ack_is_ignored() {
        let mut p = LocalPredictor::new(PredState::new([0.0; 3], 0.0));
        p.apply(1, mv(1.0), step);
        p.apply(2, mv(1.0), step);
        p.reconcile(PredState::new([2.0, 0.0, 0.0], 0.0), 2, step);
        let before = p.predicted();
        let c = p.reconcile(PredState::new([99.0, 0.0, 0.0], 0.0), 1, step); // older ack
        assert_eq!(c.error, 0.0);
        assert_eq!(p.predicted(), before, "stale ack didn't move us");
    }

    #[test]
    fn non_increasing_seq_ignored() {
        let mut p = LocalPredictor::new(PredState::new([0.0; 3], 0.0));
        p.apply(5, mv(1.0), step);
        p.apply(5, mv(1.0), step); // duplicate seq
        p.apply(3, mv(1.0), step); // older seq
        assert_eq!(p.pending_count(), 1);
        assert_eq!(p.predicted().pos[0], 1.0);
    }

    #[test]
    fn deterministic() {
        let run = || {
            let mut p = LocalPredictor::new(PredState::new([0.0; 3], 0.0));
            p.apply(1, mv(1.0), step);
            p.apply(2, mv(2.0), step);
            p.reconcile(PredState::new([1.0, 0.0, 0.0], 0.0), 1, step)
        };
        assert_eq!(run(), run());
    }
}
