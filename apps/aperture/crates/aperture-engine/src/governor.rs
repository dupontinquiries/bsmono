//! GPU sharing: who gets the queue, and when.
//!
//! THE PROBLEM
//!
//! An in-process inference engine and the game renderer share one GPU
//! and one queue. Every decode step is a submission; the inference
//! thread issues them back to back; the renderer's frame waits behind
//! them; the game goes choppy for exactly as long as generation runs.
//! On top of both sits the compositor, which has to present the frame
//! and cannot be starved either, and beside the game may sit another
//! engine -- the logscope editor's -- that the player is looking at
//! and so outranks a background build.
//!
//! There is no portable GPU priority to ask for. Mesa on the mobile
//! parts this engine targets has no `VK_EXT_global_priority` to speak
//! of, and wgpu exposes none. So the sharing is COOPERATIVE, at the
//! only point that is ours: between submissions.
//!
//! THE MECHANISM
//!
//! A [`Governor`] is a slot counter the FRAME LOOP refills. Each frame
//! the loop reports how long the last frame took; the governor turns
//! headroom into a number of slots for this frame -- none if the frame
//! was late, one if it was tight, several if there was room -- and
//! wakes the waiters. An inference thread calls [`Governor::acquire`]
//! before each step and blocks until a slot exists. Steps therefore
//! interleave with frames instead of saturating the queue ahead of
//! them, and a late frame stalls generation rather than the reverse.
//!
//! TIERS
//!
//! * `Tier::Interactive` -- an engine the player is looking at (the
//!   logscope editor). Takes the first slot each frame, and its share
//!   of the rest.
//! * `Tier::Background` -- the game's build model. Only what is left
//!   after interactive engines, and nothing on a tight frame.
//!
//! The COMPOSITOR is served by something simpler than a tier: the
//! queue is never deep. Every decode step already reads its logits
//! back, which drains the queue, so between any two steps the
//! compositor's present gets in. The governor's job is to keep "any
//! two steps" from being "as many as fit".
//!
//! SAFETY VALVE
//!
//! If the frame loop stops calling [`Governor::frame`] -- the window is
//! minimised, the game is paused, a debugger has it -- waiters must not
//! hang forever. `acquire` gives up waiting after a bounded time and
//! proceeds, which degrades to "unpaced" rather than "stuck".
//!
//! ACROSS PROCESSES
//!
//! Tiers order engines in ONE process. The logscope editor is another
//! process, and the kernel's GPU scheduler arbitrates between processes
//! without a priority this engine can set. Two honest answers:
//!
//! * Now: the game's engine paces itself against the game's own frames
//!   (this module), so its submissions are already sparse and short
//!   -- one decode step per granted slot, queue depth one -- which is
//!   the best a process can do for a neighbour without a kernel
//!   priority. It is a lower duty cycle, not a lower priority.
//! * Next: the daemon seam in `lib.rs`. One engine process owning the
//!   model, with the game and the editor as CLIENTS, each with a tier
//!   on the daemon's governor. Then "lower than logscope" is a real
//!   ordering, enforced where the submissions actually happen.
//!
//! Nothing here touches a GPU. It is a counter, a condvar, and tests.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Background = 0,
    Interactive = 1,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Policy {
    /// Frame time the renderer is aiming for.
    pub target_ms: f32,
    /// Slots granted per frame at full headroom.
    pub max_slots: u32,
    /// A frame slower than target * this gets no slots at all.
    pub stall_factor: f32,
    /// How long a waiter tolerates no frame before proceeding anyway.
    pub orphan_timeout: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self { target_ms: 16.6, max_slots: 4, stall_factor: 1.5,
               orphan_timeout: Duration::from_millis(60) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameGrant {
    pub slots: u32,
    /// Slots reserved for interactive engines before background ones.
    pub reserved_interactive: u32,
}

/// Turn a frame time into a grant. Pure, so it is testable without a
/// clock or a thread.
pub fn grant_for(policy: &Policy, last_frame_ms: f32, interactive_waiting: bool) -> FrameGrant {
    let headroom = policy.target_ms - last_frame_ms;
    let slots = if last_frame_ms > policy.target_ms * policy.stall_factor {
        0
    } else if headroom <= 0.0 {
        // over target but not stalled: one step, so generation still
        // moves on a machine that can never quite make 60
        1
    } else {
        // proportional to headroom, at least one
        (1.0 + (headroom / policy.target_ms) * (policy.max_slots as f32 - 1.0)).round() as u32
    }.min(policy.max_slots);
    // Interactive engines take the first slot when present. Background
    // work gets what remains, which on a tight frame is nothing.
    let reserved = if interactive_waiting { slots.min(1) } else { 0 };
    FrameGrant { slots, reserved_interactive: reserved }
}

#[derive(Debug)]
struct State {
    epoch: u64,
    grant: FrameGrant,
    used_interactive: u32,
    used_background: u32,
    waiting_interactive: u32,
    last_frame_at: Option<Instant>,
}

/// The shared pacing object. One per process; every engine in the
/// process acquires from the same one.
#[derive(Debug)]
pub struct Governor {
    policy: Policy,
    state: Mutex<State>,
    cv: Condvar,
}

impl Default for Governor {
    fn default() -> Self { Self::new(Policy::default()) }
}

impl Governor {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            state: Mutex::new(State {
                epoch: 0, grant: FrameGrant { slots: 0, reserved_interactive: 0 },
                used_interactive: 0, used_background: 0, waiting_interactive: 0,
                last_frame_at: None,
            }),
            cv: Condvar::new(),
        }
    }

    /// The frame loop's call, once per frame, with the previous frame's
    /// UNCLAMPED time. Refills the slots and wakes the waiters.
    pub fn frame(&self, last_frame_ms: f32) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.grant = grant_for(&self.policy, last_frame_ms, s.waiting_interactive > 0);
        s.used_interactive = 0;
        s.used_background = 0;
        s.epoch += 1;
        s.last_frame_at = Some(Instant::now());
        drop(s);
        self.cv.notify_all();
    }

    /// Block until this tier may submit one step. Returns how long it
    /// waited, so a caller can log pacing.
    pub fn acquire(&self, tier: Tier) -> Duration {
        let t0 = Instant::now();
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if tier == Tier::Interactive { s.waiting_interactive += 1; }
        loop {
            // Orphaned: no frame for a while. Proceed unpaced rather
            // than hang; the game may be minimised or paused.
            let orphaned = s.last_frame_at
                .map(|t| t.elapsed() > self.policy.orphan_timeout)
                .unwrap_or(true);
            let available = match tier {
                Tier::Interactive => s.used_interactive + s.used_background < s.grant.slots,
                Tier::Background => {
                    let for_bg = s.grant.slots.saturating_sub(s.grant.reserved_interactive
                        .saturating_sub(s.used_interactive));
                    s.used_interactive + s.used_background < for_bg
                }
            };
            if available || orphaned {
                match tier {
                    Tier::Interactive => { s.used_interactive += 1; s.waiting_interactive -= 1; }
                    Tier::Background => { s.used_background += 1; }
                }
                return t0.elapsed();
            }
            let (guard, _) = self.cv.wait_timeout(s, self.policy.orphan_timeout)
                .unwrap_or_else(|e| e.into_inner());
            s = guard;
        }
    }

    pub fn policy(&self) -> Policy { self.policy }

    /// What the current frame granted and how much is used, for a HUD.
    pub fn snapshot(&self) -> (FrameGrant, u32, u32) {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (s.grant, s.used_interactive, s.used_background)
    }
}

/// A shared governor, comparable by IDENTITY so a config holding one
/// can still derive PartialEq: two configs are equal if they pace on
/// the same governor, which is the only question anyone asks.
#[derive(Clone, Debug)]
pub struct Handle(pub std::sync::Arc<Governor>);

impl PartialEq for Handle {
    fn eq(&self, o: &Self) -> bool { std::sync::Arc::ptr_eq(&self.0, &o.0) }
}

impl std::ops::Deref for Handle {
    type Target = Governor;
    fn deref(&self) -> &Governor { &self.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_late_frame_grants_nothing_and_a_fast_one_grants_more() {
        let p = Policy::default();
        assert_eq!(grant_for(&p, 40.0, false).slots, 0, "stalled: nothing");
        assert_eq!(grant_for(&p, 20.0, false).slots, 1, "over target: one step");
        assert_eq!(grant_for(&p, 16.0, false).slots, 1, "tight: one");
        assert_eq!(grant_for(&p, 4.0, false).slots, 3, "lots of room: several");
        assert!(grant_for(&p, 0.5, false).slots <= p.max_slots);
    }

    #[test]
    fn interactive_takes_the_first_slot_on_a_tight_frame() {
        let p = Policy::default();
        let g = grant_for(&p, 16.0, true);
        assert_eq!(g.slots, 1);
        assert_eq!(g.reserved_interactive, 1, "the one slot is the editor's");
        let g2 = grant_for(&p, 16.0, false);
        assert_eq!(g2.reserved_interactive, 0, "no editor waiting: the game's build may use it");
    }

    #[test]
    fn background_waits_when_the_frame_is_tight_and_an_editor_is_waiting() {
        // Deterministic: drive the governor by hand, no timing.
        let gov = Governor::new(Policy { orphan_timeout: Duration::from_secs(10), ..Default::default() });
        {
            let mut s = gov.state.lock().unwrap();
            s.waiting_interactive = 1;
            s.last_frame_at = Some(Instant::now());
        }
        gov.frame(16.0);   // one slot, reserved for interactive
        let s = gov.state.lock().unwrap();
        let for_bg = s.grant.slots.saturating_sub(s.grant.reserved_interactive);
        assert_eq!(for_bg, 0, "background gets nothing this frame");
    }

    #[test]
    fn acquire_blocks_until_a_frame_grants_a_slot() {
        let gov = Arc::new(Governor::new(Policy { orphan_timeout: Duration::from_secs(5), ..Default::default() }));
        gov.frame(40.0);   // stalled: zero slots
        let g = gov.clone();
        let worker = std::thread::spawn(move || g.acquire(Tier::Background));
        std::thread::sleep(Duration::from_millis(80));
        assert!(!worker.is_finished(), "no slot, so it waits");
        gov.frame(4.0);    // room: several slots
        let waited = worker.join().unwrap();
        assert!(waited >= Duration::from_millis(70), "it waited for the frame: {waited:?}");
    }

    #[test]
    fn slots_are_consumed_and_refilled_per_frame() {
        let gov = Governor::new(Policy { orphan_timeout: Duration::from_secs(5), ..Default::default() });
        gov.frame(4.0);
        let n = gov.snapshot().0.slots;
        for _ in 0..n { gov.acquire(Tier::Background); }
        let (_, _, used) = gov.snapshot();
        assert_eq!(used, n, "all consumed");
        // the next acquire would block; a frame refills
        gov.frame(4.0);
        assert_eq!(gov.snapshot().2, 0, "refilled");
        gov.acquire(Tier::Background);
        assert_eq!(gov.snapshot().2, 1);
    }

    #[test]
    fn an_orphaned_waiter_proceeds_rather_than_hanging() {
        // No frame loop at all: the game is minimised or paused. A
        // worker must still make progress, unpaced.
        let gov = Governor::new(Policy { orphan_timeout: Duration::from_millis(30), ..Default::default() });
        let t0 = Instant::now();
        gov.acquire(Tier::Background);
        assert!(t0.elapsed() < Duration::from_millis(200), "gave up waiting: {:?}", t0.elapsed());
    }

    #[test]
    fn interactive_is_served_before_background_within_a_frame() {
        let gov = Arc::new(Governor::new(Policy { orphan_timeout: Duration::from_secs(5), ..Default::default() }));
        gov.frame(40.0);   // nothing yet
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut hs = Vec::new();
        for (tier, name) in [(Tier::Background, "bg"), (Tier::Interactive, "ui")] {
            let (g, o) = (gov.clone(), order.clone());
            hs.push(std::thread::spawn(move || {
                g.acquire(tier);
                o.lock().unwrap().push(name);
            }));
        }
        std::thread::sleep(Duration::from_millis(60));
        gov.frame(16.0);   // ONE slot, and an interactive waiter is present
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(order.lock().unwrap().as_slice(), ["ui"], "the editor got the tight frame's slot");
        gov.frame(4.0);    // now room for both
        for h in hs { h.join().unwrap(); }
        assert_eq!(order.lock().unwrap().len(), 2);
    }
}
