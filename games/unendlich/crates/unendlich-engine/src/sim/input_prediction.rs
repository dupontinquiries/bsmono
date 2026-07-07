//! `sim::input_prediction` — the input side of rollback netcode: a fixed-size
//! ring buffer of recent inputs, and a predictor for ticks that haven't arrived
//! yet from a remote peer.
//!
//! Rollback works by *guessing* an absent player's input, simulating forward,
//! and re-simulating if the guess turns out wrong. [`predict`] is that guess:
//! it repeats the last known input (players tend to keep holding what they were
//! holding) until a hold has run long enough to look unrealistic, after which it
//! relaxes to neutral. [`InputBuffer`] stores confirmed inputs by tick in a ring
//! of [`BUFFER_TICKS`] slots — old ticks wrap and overwrite — and
//! [`InputBuffer::get_or_predict`] serves a stored input when present, or a
//! prediction when the tick is missing.
//!
//! Pure and deterministic: integer-only, no clock, no randomness, fixed memory.

/// Ring-buffer capacity in ticks; older ticks wrap and overwrite.
pub const BUFFER_TICKS: usize = 64;
/// How many ticks an input may be predicted as "held" before relaxing.
pub const MAX_HELD_TICKS: u32 = 20;

/// One frame of player input. Integer-only so it compares and replays exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Input {
    /// Button bitmask.
    pub buttons: u8,
    /// Horizontal move axis, `-127..=127`.
    pub move_x: i8,
    /// Vertical move axis, `-127..=127`.
    pub move_y: i8,
}

impl Input {
    /// No buttons, no movement.
    pub fn neutral() -> Self {
        Input {
            buttons: 0,
            move_x: 0,
            move_y: 0,
        }
    }
}

/// Predict the input for a tick we haven't received.
///
/// Repeats `last` while `held_ticks <= MAX_HELD_TICKS`; beyond that the held
/// input is assumed released and the prediction relaxes to [`Input::neutral`].
pub fn predict(last: &Input, held_ticks: u32) -> Input {
    if held_ticks <= MAX_HELD_TICKS {
        *last
    } else {
        Input::neutral()
    }
}

/// A fixed-size ring buffer of confirmed inputs, keyed by tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputBuffer {
    slots: [Option<(u32, Input)>; BUFFER_TICKS],
    last: Input,
    last_tick: u32,
    has_last: bool,
}

impl Default for InputBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBuffer {
    /// An empty buffer.
    pub fn new() -> Self {
        InputBuffer {
            slots: [None; BUFFER_TICKS],
            last: Input::neutral(),
            last_tick: 0,
            has_last: false,
        }
    }

    /// Fixed capacity in ticks.
    pub const fn capacity(&self) -> usize {
        BUFFER_TICKS
    }

    fn slot(tick: u32) -> usize {
        (tick as usize) % BUFFER_TICKS
    }

    /// Store a confirmed input for `tick`, overwriting whatever shared its ring
    /// slot. Also tracks the most recent tick for prediction.
    pub fn store(&mut self, tick: u32, input: Input) {
        self.slots[Self::slot(tick)] = Some((tick, input));
        if !self.has_last || tick >= self.last_tick {
            self.last = input;
            self.last_tick = tick;
            self.has_last = true;
        }
    }

    /// The input stored for `tick`, or `None` if that tick isn't held (never
    /// stored, or wrapped out by a newer tick in the same slot).
    pub fn get(&self, tick: u32) -> Option<Input> {
        match self.slots[Self::slot(tick)] {
            Some((t, input)) if t == tick => Some(input),
            _ => None,
        }
    }

    /// The stored input for `tick`, or a prediction from the latest known input
    /// when it's missing.
    pub fn get_or_predict(&self, tick: u32) -> Input {
        match self.get(tick) {
            Some(input) => input,
            None => {
                let held = tick.saturating_sub(self.last_tick);
                predict(&self.last, held)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_input(seed: &mut u32) -> Input {
        Input {
            buttons: (lcg(seed) & 0xFF) as u8,
            move_x: (lcg(seed) & 0xFF) as u8 as i8,
            move_y: (lcg(seed) & 0xFF) as u8 as i8,
        }
    }

    #[test]
    fn predict_repeats_held_input() {
        let held = Input {
            buttons: 0b101,
            move_x: 30,
            move_y: -10,
        };
        for k in 0..=MAX_HELD_TICKS {
            assert_eq!(predict(&held, k), held, "should repeat at {} ticks", k);
        }
        // beyond the cap it relaxes to neutral
        assert_eq!(predict(&held, MAX_HELD_TICKS + 1), Input::neutral());
    }

    #[test]
    fn buffer_get_returns_stored() {
        let mut b = InputBuffer::new();
        let i = Input {
            buttons: 7,
            move_x: 1,
            move_y: 2,
        };
        b.store(42, i);
        assert_eq!(b.get(42), Some(i));
        assert_eq!(b.get(43), None);
    }

    #[test]
    fn overwrite_wraps() {
        let mut b = InputBuffer::new();
        let a = Input {
            buttons: 1,
            move_x: 0,
            move_y: 0,
        };
        let c = Input {
            buttons: 2,
            move_x: 0,
            move_y: 0,
        };
        b.store(5, a);
        b.store(5 + BUFFER_TICKS as u32, c); // same ring slot
        assert_eq!(b.get(5), None, "old tick should have wrapped out");
        assert_eq!(b.get(5 + BUFFER_TICKS as u32), Some(c));
    }

    #[test]
    fn missing_tick_is_predicted() {
        let mut b = InputBuffer::new();
        let held = Input {
            buttons: 4,
            move_x: 12,
            move_y: 0,
        };
        b.store(100, held);
        // a stored tick comes back verbatim
        assert_eq!(b.get_or_predict(100), held);
        // a near-future missing tick repeats the held input
        assert_eq!(b.get_or_predict(105), held);
        // a far-future missing tick relaxes to neutral
        assert_eq!(b.get_or_predict(100 + MAX_HELD_TICKS + 5), Input::neutral());
    }

    #[test]
    fn bounded_size() {
        let mut b = InputBuffer::new();
        assert_eq!(b.capacity(), BUFFER_TICKS);
        // store far more ticks than the buffer holds
        let mut t = 0u32;
        while t < 1000 {
            b.store(t, Input { buttons: (t & 0xFF) as u8, move_x: 0, move_y: 0 });
            t += 1;
        }
        // only the most recent BUFFER_TICKS window is retrievable
        let newest = 999u32;
        let mut got = 0;
        let mut k = 0u32;
        while k < 2000 {
            if b.get(k).is_some() {
                got += 1;
            }
            k += 1;
        }
        assert_eq!(got, BUFFER_TICKS, "exactly one window should survive");
        assert!(b.get(newest).is_some());
        assert!(b.get(newest - BUFFER_TICKS as u32).is_none());
    }

    #[test]
    fn deterministic() {
        fn build(seed_init: u32) -> InputBuffer {
            let mut seed = seed_init;
            let mut b = InputBuffer::new();
            let mut n = 0;
            while n < 300 {
                let tick = lcg(&mut seed) % 500;
                b.store(tick, rand_input(&mut seed));
                n += 1;
            }
            b
        }
        let b1 = build(0xABCD);
        let b2 = build(0xABCD);
        assert_eq!(b1, b2);
        // and get_or_predict agrees across a range
        let mut t = 0u32;
        while t < 500 {
            assert_eq!(b1.get_or_predict(t), b2.get_or_predict(t));
            t += 1;
        }
    }
}
