//! Input snapshots for rollback netcode foundations.
//!
//! Each frame, the simulation captures its inputs into an `InputFrame`
//! tagged with the `TickId` they were captured at. These frames are
//! stored in a ring buffer of recent ticks. When a server correction
//! arrives saying "your local prediction was wrong at tick T", we roll
//! the simulation back to T's saved state and replay forward using the
//! cached inputs from T+1 through current.
//!
//! Rollback isn't actually wired yet — there's no transport, no server,
//! no network. But the input capture machinery is the foundation. By
//! capturing inputs into structured snapshots NOW, we keep the door
//! open: every frame's input becomes a deterministic record that can
//! be replayed.
//!
//! ## What lives in an InputFrame
//!
//! Bitset of currently-held actions (move forward / left / etc.),
//! current frame's mouse delta, current frame's button transitions
//! (just-pressed jump, just-pressed toggle-flight). These are all
//! the inputs the simulation cares about. Camera position is NOT an
//! input — it's derived state.
//!
//! Multiple presses/releases within a single frame collapse to a
//! single state-at-end-of-frame plus the transition flag. That's
//! lossy but matches Minecraft-style simulation: input is sampled at
//! tick boundaries.
//!
//! ## Tick alignment
//!
//! The bin runs render frames at variable rate (often >60 Hz). The
//! simulation runs ticks at a fixed rate (~2 Hz today, will go up
//! when physics moves to its own tick rate). Multiple render frames
//! map to one tick → we need to AGGREGATE input across those frames
//! into one `InputFrame`. Aggregation: union the held-actions
//! bitset, sum the mouse deltas, OR the just-pressed flags.

use crate::keybindings::GameAction;
use crate::world::ticks::TickId;

/// Bitset of game actions. Compact representation suitable for
/// serialization across the wire. Hand-rolled to avoid pulling in the
/// bitflags crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ActionBits(pub u32);

impl ActionBits {
    pub const MOVE_FORWARD: Self     = Self(1 << 0);
    pub const MOVE_BACKWARD: Self    = Self(1 << 1);
    pub const MOVE_LEFT: Self        = Self(1 << 2);
    pub const MOVE_RIGHT: Self       = Self(1 << 3);
    pub const JUMP: Self             = Self(1 << 4);
    pub const SNEAK: Self            = Self(1 << 5);
    pub const SPRINT: Self           = Self(1 << 6);
    pub const FLY_UP: Self           = Self(1 << 7);
    pub const FLY_DOWN: Self         = Self(1 << 8);
    pub const TOGGLE_FLIGHT: Self    = Self(1 << 9);
    pub const PRIMARY_ACTION: Self   = Self(1 << 10);
    pub const SECONDARY_ACTION: Self = Self(1 << 11);

    pub const fn empty() -> Self { Self(0) }
    pub const fn is_empty(self) -> bool { self.0 == 0 }
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
    pub fn insert(&mut self, other: Self) { self.0 |= other.0; }
    pub fn remove(&mut self, other: Self) { self.0 &= !other.0; }

    /// Convert a `GameAction` to its single-bit representation. Returns
    /// `Self::empty()` for actions that don't have a network bit
    /// (debug toggles, screenshots — those are local-only).
    pub fn from_action(a: GameAction) -> Self {
        match a {
            GameAction::MoveForward => Self::MOVE_FORWARD,
            GameAction::MoveBackward => Self::MOVE_BACKWARD,
            GameAction::MoveLeft => Self::MOVE_LEFT,
            GameAction::MoveRight => Self::MOVE_RIGHT,
            GameAction::Jump => Self::JUMP,
            GameAction::Sneak => Self::SNEAK,
            GameAction::Sprint => Self::SPRINT,
            GameAction::FlyUp => Self::FLY_UP,
            GameAction::FlyDown => Self::FLY_DOWN,
            GameAction::ToggleFlight => Self::TOGGLE_FLIGHT,
            // Hotbar / inventory / debug actions don't get net bits —
            // they're local-only or will get their own message types.
            _ => Self::empty(),
        }
    }
}

impl std::ops::BitOr for ActionBits {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
}
impl std::ops::BitAnd for ActionBits {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self { Self(self.0 & rhs.0) }
}

/// One frame of player input, stamped with the tick it applies to.
///
/// Layout is intentionally compact: this struct will be sent over the
/// network at the simulation tick rate, so each byte counts. Today the
/// only consumer is local replay logic but the wire format is ready.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct InputFrame {
    /// Tick this input applies to. The receiver simulates this tick by
    /// applying these inputs to the world at tick `tick_id - 1`'s
    /// state.
    pub tick_id: TickId,
    /// Bitmask of currently-held actions.
    pub held: ActionBits,
    /// Bitmask of actions that transitioned from up to down THIS tick.
    /// Used for one-shot actions (jump, toggle flight, primary click)
    /// that shouldn't auto-repeat.
    pub just_pressed: ActionBits,
    /// Mouse pitch delta (y-axis), accumulated across all render
    /// frames that fell within this tick. In screen-space units.
    pub look_pitch_delta: f32,
    /// Mouse yaw delta (x-axis), accumulated.
    pub look_yaw_delta: f32,
}

impl InputFrame {
    pub fn new(tick_id: TickId) -> Self {
        Self {
            tick_id,
            held: ActionBits::empty(),
            just_pressed: ActionBits::empty(),
            look_pitch_delta: 0.0,
            look_yaw_delta: 0.0,
        }
    }
}

/// Ring buffer of recent input frames keyed by TickId. Used by the
/// rollback system to replay inputs forward when a server correction
/// arrives.
///
/// Capacity = how many ticks of history we keep. With 2 Hz tick rate
/// and ~250ms expected round-trip, we'd want maybe 4-8 ticks of
/// history. Picking 64 leaves comfortable headroom.
pub struct InputBuffer {
    /// Newest first. Frames are appended at the end; old frames are
    /// trimmed when length exceeds CAPACITY.
    frames: std::collections::VecDeque<InputFrame>,
    capacity: usize,
}

impl InputBuffer {
    pub const DEFAULT_CAPACITY: usize = 64;

    pub fn new() -> Self { Self::with_capacity(Self::DEFAULT_CAPACITY) }
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            frames: std::collections::VecDeque::with_capacity(cap),
            capacity: cap,
        }
    }

    /// Record one frame of input. If buffer is full, drops oldest.
    pub fn push(&mut self, frame: InputFrame) {
        if self.frames.len() >= self.capacity {
            self.frames.pop_front();
        }
        self.frames.push_back(frame);
    }

    /// Look up an input frame by tick. Returns None if outside the
    /// buffer's history window.
    pub fn get(&self, tick: TickId) -> Option<&InputFrame> {
        self.frames.iter().find(|f| f.tick_id == tick)
    }

    /// Most recent frame.
    pub fn latest(&self) -> Option<&InputFrame> { self.frames.back() }

    pub fn len(&self) -> usize { self.frames.len() }
    pub fn is_empty(&self) -> bool { self.frames.is_empty() }
}

impl Default for InputBuffer {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_bits_round_trip_through_actions() {
        for a in [GameAction::MoveForward, GameAction::Jump, GameAction::Sneak] {
            let bits = ActionBits::from_action(a);
            assert!(!bits.is_empty(), "action {:?} should have a bit", a);
        }
    }

    #[test]
    fn input_buffer_drops_oldest_when_full() {
        let mut buf = InputBuffer::with_capacity(3);
        for i in 0..5 {
            buf.push(InputFrame::new(TickId(i)));
        }
        assert_eq!(buf.len(), 3, "buffer should cap at capacity");
        assert!(buf.get(TickId(0)).is_none(), "tick 0 should have been dropped");
        assert!(buf.get(TickId(2)).is_some());
        assert!(buf.get(TickId(4)).is_some());
    }

    #[test]
    fn input_buffer_lookup_by_tick() {
        let mut buf = InputBuffer::new();
        let mut f = InputFrame::new(TickId(42));
        f.held = ActionBits::JUMP | ActionBits::MOVE_FORWARD;
        buf.push(f);
        let got = buf.get(TickId(42)).expect("inserted frame");
        assert!(got.held.contains(ActionBits::JUMP));
        assert!(got.held.contains(ActionBits::MOVE_FORWARD));
    }
}
