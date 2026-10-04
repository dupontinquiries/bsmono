//! lensmove -- move the lens motor without a click.
//!
//! **What was measured** (the phone's microphone, the `dw9719` motor on its own sub-device, nothing else running):
//! a single large step (0 -> 1000, or 1000 -> 0) makes a sharp click at the moment of the command, 3-6x the loudest
//! noise in a silent recording; the SAME travel in steps of 10 or 50 steps, a few milliseconds apart, is
//! indistinguishable from silence; steps of 100 and 200 click again. Opening and closing the motor's device for
//! every tiny move does not click, so it is the size of the step, not the power-cycling. The focus search used to jump
//! 127 steps at a time, and a full-range jump back to 0 at the start of every search.
//!
//! **What this does.** The motor is open-loop (nothing reports where the glass really is), so the PD law from the
//! owner's embedded-controls coursework runs on a VIRTUAL position instead: each 4 ms tick
//! `step = KP * error + KD * (error - previous_error)`, capped at `MAX_STEP`, and the motor is commanded to follow.
//! The derivative term shrinks the step as the error shrinks (the damping that keeps a spring-mass from ringing), the
//! cap keeps every step under the audible size, and a full-range move takes under 200 ms. Pure; the thread that
//! applies it to the real device is `tapfocus::V4l2Lens`.

/// The largest step commanded at once. Steps of 10 and 50 measured silent, 100 and 200 audible.
pub const MAX_STEP: f64 = 40.0;
/// Proportional and derivative gains, per tick (tuned in simulation: no overshoot for any distance, full range < 200 ms).
pub const KP: f64 = 0.25;
pub const KD: f64 = 0.2;
/// The pause between steps.
pub const TICK_MS: u64 = 4;

#[derive(Debug, Clone)]
pub struct Mover {
    x: f64,
    prev_error: f64,
    target: i64,
    last: i64,
}

impl Mover {
    /// A mover whose lens is at `at`.
    pub fn new(at: i64) -> Mover {
        Mover { x: at as f64, prev_error: 0.0, target: at, last: at }
    }

    /// The position last commanded.
    pub fn position(&self) -> i64 { self.last }

    pub fn at_rest(&self, target: i64) -> bool { self.last == target }

    /// One tick toward `target`: the position to command now.
    pub fn step(&mut self, target: i64) -> i64 {
        if target != self.target {
            // a new target: no derivative kick from the jump in the error
            self.target = target;
            self.prev_error = target as f64 - self.x;
        }
        let error = target as f64 - self.x;
        let step = (KP * error + KD * (error - self.prev_error)).clamp(-MAX_STEP, MAX_STEP);
        self.prev_error = error;
        self.x += step;
        // within one step and nearly stopped: land exactly (the exponential tail would otherwise never arrive)
        if (target as f64 - self.x).abs() < 1.0 && step.abs() < 1.0 {
            self.x = target as f64;
        }
        let want = self.x.round() as i64;
        let cmd = want.clamp(self.last - MAX_STEP as i64, self.last + MAX_STEP as i64); // rounding never makes a bigger step
        self.last = cmd;
        cmd
    }
}

/// Somewhere to write a lens position.
pub trait FocusWrite {
    fn write(&mut self, pos: i64) -> Result<(), String>;
}

/// Move from where `mover` is to `target` in small steps. `retarget` is asked each tick for the current target (it may
/// change mid-move); `pause` waits `TICK_MS` between steps. Returns how many writes it made.
pub fn glide<W: FocusWrite>(mover: &mut Mover, w: &mut W, mut target: i64, mut retarget: impl FnMut(i64) -> i64, mut pause: impl FnMut(u64)) -> Result<usize, String> {
    let mut writes = 0;
    for _ in 0..2000 {
        target = retarget(target);
        if mover.at_rest(target) { return Ok(writes); }
        let p = mover.step(target);
        w.write(p)?;
        writes += 1;
        if mover.at_rest(target) { return Ok(writes); }
        pause(TICK_MS);
    }
    Err("the lens did not arrive".into())
}

/// How long a move of this size takes (so a caller knows when to trust the next frame).
pub fn travel_ms(from: i64, to: i64) -> u64 {
    let mut m = Mover::new(from);
    let mut ticks = 0u64;
    while !m.at_rest(to) && ticks < 2000 {
        m.step(to);
        ticks += 1;
    }
    ticks * TICK_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MEASURED with the phone's microphone, independent of `MAX_STEP`: 50-step moves were silent, 100-step moves clicked.
    const MEASURED_SILENT_STEP: i64 = 50;

    struct Rec { w: Vec<i64> }
    impl FocusWrite for Rec { fn write(&mut self, p: i64) -> Result<(), String> { self.w.push(p); Ok(()) } }

    fn run(from: i64, to: i64) -> Vec<i64> {
        let mut m = Mover::new(from);
        let mut r = Rec { w: vec![] };
        glide(&mut m, &mut r, to, |t| t, |_| {}).unwrap();
        r.w
    }

    #[test]
    fn no_step_is_big_enough_to_click_for_any_distance_in_either_direction() {
        for (from, to) in (1..=1023).flat_map(|d| [(0, d), (1023, 1023 - d)]) {
            let w = run(from, to);
            let mut prev = from;
            for p in &w {
                assert!((p - prev).abs() <= MEASURED_SILENT_STEP, "{from}->{to}: a step of {} at {p} is bigger than the largest one measured silent", p - prev);
                prev = *p;
            }
            assert_eq!(*w.last().unwrap(), to, "{from}->{to}: it must land exactly");
        }
    }

    #[test]
    fn the_step_cap_is_inside_what_was_measured_silent() {
        assert!(MAX_STEP <= MEASURED_SILENT_STEP as f64, "MAX_STEP {MAX_STEP} must not exceed the largest step measured silent");
    }

    #[test]
    fn it_never_crosses_the_target_so_there_is_no_overshoot_to_ring() {
        for d in [1, 2, 5, 17, 40, 41, 100, 127, 333, 700, 1023] {
            let up = run(100, 100 + d.min(900));
            assert!(up.iter().all(|p| *p <= 100 + d.min(900)), "up {d}: passed the target: {:?}", up.iter().max());
            let down = run(1000, 1000 - d.min(900));
            assert!(down.iter().all(|p| *p >= 1000 - d.min(900)), "down {d}: passed the target: {:?}", down.iter().min());
            // and it moves one way only
            assert!(up.windows(2).all(|w| w[1] >= w[0]) && down.windows(2).all(|w| w[1] <= w[0]), "{d}: a monotone approach");
        }
    }

    #[test]
    fn a_full_range_move_takes_well_under_a_fifth_of_a_second_and_a_small_one_is_quick() {
        assert!(travel_ms(0, 1023) <= 250, "full range: {} ms", travel_ms(0, 1023));
        assert!(travel_ms(0, 1023) >= 100, "and it is a slew, not a jump: {} ms", travel_ms(0, 1023));
        assert!(travel_ms(500, 510) <= 80, "a nudge: {} ms", travel_ms(500, 510));
        assert_eq!(travel_ms(300, 300), 0, "no move, no time");
        let mut last = 0;
        for d in [10, 50, 127, 400, 1023] {
            let t = travel_ms(0, d);
            assert!(t >= last, "farther takes at least as long: {d} -> {t} ms");
            last = t;
        }
    }

    #[test]
    fn changing_the_target_mid_move_stays_gentle_and_still_lands() {
        let mut m = Mover::new(0);
        let mut r = Rec { w: vec![] };
        let mut tick = 0;
        // head for 1000, then turn back for 100 after 6 ticks
        glide(&mut m, &mut r, 1000, |t| { tick += 1; if tick > 6 { 100 } else { t } }, |_| {}).unwrap();
        let mut prev = 0;
        for p in &r.w { assert!((p - prev).abs() <= MEASURED_SILENT_STEP, "a step of {} at {p}", p - prev); prev = *p; }
        assert_eq!(*r.w.last().unwrap(), 100);
        assert!(*r.w.iter().max().unwrap() < 1000, "it turned before reaching the old target: {:?}", r.w.iter().max());
    }

    #[test]
    fn a_failing_write_stops_the_move_with_the_reason() {
        struct Bad;
        impl FocusWrite for Bad { fn write(&mut self, _: i64) -> Result<(), String> { Err("no such device".into()) } }
        let mut m = Mover::new(0);
        assert_eq!(glide(&mut m, &mut Bad, 500, |t| t, |_| {}), Err("no such device".to_string()));
    }

    #[test]
    fn already_there_writes_nothing() {
        let mut m = Mover::new(321);
        let mut r = Rec { w: vec![] };
        assert_eq!(glide(&mut m, &mut r, 321, |t| t, |_| {}), Ok(0));
        assert!(r.w.is_empty());
    }
}
