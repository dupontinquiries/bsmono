//! Lossless discrete-action edge buffer — the fix for keybinds getting eaten when
//! the frame rate drops. The rollback input path (`input_capture::InputBuffer`)
//! samples at tick boundaries and is deliberately lossy: it keeps "state at end
//! of frame", so a quick tap that presses *and* releases inside one long frame
//! collapses to nothing. That's fine for held movement, but it silently drops
//! discrete keybinds (open inventory, drop item, hotbar select) exactly when a
//! stall makes them feel unresponsive.
//!
//! This buffer sits beside that path and is the opposite: it records every
//! genuine press/release *edge* the instant it arrives (in the GLFW callback,
//! decoupled from the frame), and the frame drains them in order. So a tap during
//! a 500ms frame still arrives as Press→Release next drain — nothing is lost. Two
//! safeguards keep it cheap: key-repeat is de-duplicated against held state (a
//! held key enqueues one Press, not a flood), and the queue is bounded with an
//! overflow counter while held state stays authoritative.
//!
//! Generic over the action type so it stays pure/testable; the bin instantiates
//! `ActionQueue<GameAction>`. Deterministic.

use std::collections::{HashSet, VecDeque};
use std::hash::Hash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Press,
    Release,
}

/// A single genuine state transition for an action, in arrival order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionEdge<A> {
    pub action: A,
    pub edge: Edge,
    pub seq: u64,
}

/// A bounded, lossless FIFO of action edges plus authoritative held state.
#[derive(Debug, Clone)]
pub struct ActionQueue<A: Copy + Eq + Hash> {
    queue: VecDeque<ActionEdge<A>>,
    held: HashSet<A>,
    cap: usize,
    seq: u64,
    dropped: u64,
}

impl<A: Copy + Eq + Hash> ActionQueue<A> {
    pub const DEFAULT_CAPACITY: usize = 256;

    pub fn new() -> Self {
        Self::with_capacity(Self::DEFAULT_CAPACITY)
    }

    pub fn with_capacity(cap: usize) -> Self {
        ActionQueue {
            queue: VecDeque::with_capacity(cap),
            held: HashSet::new(),
            cap: cap.max(1),
            seq: 0,
            dropped: 0,
        }
    }

    fn enqueue(&mut self, action: A, edge: Edge) {
        if self.queue.len() >= self.cap {
            // Overflow (pathological stall): shed the oldest edge. Held state is
            // tracked separately, so this never strands a key — only old replay
            // is lost, which is the right thing to drop under pressure.
            self.queue.pop_front();
            self.dropped += 1;
        }
        let seq = self.seq;
        self.seq += 1;
        self.queue.push_back(ActionEdge { action, edge, seq });
    }

    /// Record a press. A no-op if the action is already held (de-dups GLFW
    /// key-repeat so a held key doesn't flood the buffer during a stall).
    pub fn press(&mut self, action: A) {
        if self.held.insert(action) {
            self.enqueue(action, Edge::Press);
        }
    }

    /// Record a release. A no-op if the action wasn't held.
    pub fn release(&mut self, action: A) {
        if self.held.remove(&action) {
            self.enqueue(action, Edge::Release);
        }
    }

    /// Take all buffered edges in arrival order, emptying the queue. Held state
    /// persists across drains.
    pub fn drain(&mut self) -> Vec<ActionEdge<A>> {
        self.queue.drain(..).collect()
    }

    pub fn is_held(&self, action: A) -> bool {
        self.held.contains(&action)
    }

    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// How many edges were shed to overflow over this queue's life (telemetry).
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl<A: Copy + Eq + Hash> Default for ActionQueue<A> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum TestAction {
        OpenInventory,
        DropItem,
        MoveForward,
        Sprint,
        Jump,
        Hotbar1,
    }
    use TestAction::*;

    #[test]
    fn a_tap_inside_a_single_stalled_frame_is_not_lost() {
        // The whole point: press AND release happen between two drains. The lossy
        // tick path would collapse this to "not held, nothing happened". Here both
        // edges survive.
        let mut q = ActionQueue::new();
        q.press(OpenInventory);
        q.release(OpenInventory);
        let edges = q.drain();
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].edge, Edge::Press);
        assert_eq!(edges[1].edge, Edge::Release);
        assert!(!q.is_held(OpenInventory));
    }

    #[test]
    fn many_taps_in_one_batch_are_delivered_in_order() {
        let mut q = ActionQueue::new();
        for _ in 0..3 {
            q.press(DropItem);
            q.release(DropItem);
        }
        let edges = q.drain();
        assert_eq!(edges.len(), 6, "three full taps, all delivered");
        assert!(edges.windows(2).all(|w| w[0].seq < w[1].seq));
    }

    #[test]
    fn key_repeat_does_not_flood_the_buffer() {
        let mut q = ActionQueue::new();
        for _ in 0..100 {
            q.press(MoveForward); // GLFW auto-repeat while held
        }
        assert_eq!(q.pending(), 1, "a held key enqueues one Press, not 100");
        assert!(q.is_held(MoveForward));
    }

    #[test]
    fn held_state_persists_across_drains() {
        let mut q = ActionQueue::new();
        q.press(Sprint);
        q.drain();
        assert!(q.is_held(Sprint), "draining edges doesn't clear held state");
        q.release(Sprint);
        assert!(!q.is_held(Sprint));
    }

    #[test]
    fn a_release_without_a_press_is_ignored() {
        let mut q = ActionQueue::new();
        q.release(Jump);
        assert!(q.drain().is_empty(), "stray release produces no edge");
    }

    #[test]
    fn overflow_is_bounded_and_counted_but_held_stays_correct() {
        let mut q = ActionQueue::with_capacity(4);
        for _ in 0..10 {
            q.press(Hotbar1);
            q.release(Hotbar1);
        }
        assert_eq!(q.pending(), 4, "queue capped");
        assert!(q.dropped() >= 16, "the rest counted as dropped: {}", q.dropped());
        assert!(!q.is_held(Hotbar1), "held state correct despite dropped edges");
    }
}
