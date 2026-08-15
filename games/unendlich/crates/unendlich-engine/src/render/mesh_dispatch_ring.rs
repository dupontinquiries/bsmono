//! `render::mesh_dispatch_ring` — pure in-flight slot lifecycle for a
//! pipelined (async) octree-mesher dispatch.
//!
//! Companion to `docs/speculative/async-mesher-dispatch.md`. This module is
//! the hardware-independent core, written and tested ahead of the Vulkan
//! wiring (same seam discipline as `octree_pin`): slot acquire/submit/
//! complete/release, in-flight caps, completion-driven GPU-time budget
//! accounting, and the backpressure decision. The eventual Vulkan session
//! wires per-slot `vk::CommandBuffer`/`vk::Fence`/output regions into this
//! state machine instead of inventing lifecycle logic next to `unsafe` code.
//!
//! NOT wired into the dispatcher yet — nothing constructs it outside tests.
//! The dispatcher stays synchronous until the gating measurements in the
//! memo pass on hardware.
//!
//! Time is passed in as `u64` microseconds rather than `Instant` so tests
//! drive a virtual clock and the budget math is exactly reproducible.

/// State of one dispatch slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    /// Free for acquire.
    Idle,
    /// Acquired: batch is being uploaded/recorded. Not yet on the GPU.
    Recording,
    /// Submitted at `submit_us`; fence not yet observed signaled.
    InFlight { submit_us: u64 },
}

/// Per-slot bookkeeping the Vulkan layer attaches its resources to by index.
#[derive(Debug)]
struct Slot {
    state: SlotState,
    /// Pinned octree-pool span offsets for the batch in this slot; handed
    /// back at completion so the caller can `unpin_spans` them.
    pinned_offsets: Vec<u64>,
}

/// Pure in-flight ring. `capacity` = max overlapped dispatches (the memo's
/// `UNENDLICH_GPU_MESH_INFLIGHT`; 1 reproduces today's synchronous behavior).
#[derive(Debug)]
pub struct DispatchRing {
    slots: Vec<Slot>,
    /// Fence-signal time of the most recently COMPLETED dispatch, for
    /// overlap-aware GPU-time accounting (see [`Self::complete`]).
    last_signal_us: u64,
    /// Total estimated GPU time consumed, overlap-deduplicated. This is the
    /// number the r708-style frame budget should charge against — NOT the
    /// sum of per-dispatch wall times, which double-counts overlap.
    gpu_time_consumed_us: u64,
}

/// What the dispatcher should do next, from [`DispatchRing::admission`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// A slot is free: acquire and submit.
    Submit { slot: usize },
    /// All slots in flight: poll/wait the oldest in-flight slot (natural
    /// backpressure), then drain completions and retry.
    WaitOldest { slot: usize },
}

impl DispatchRing {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity >= 1, "ring needs at least one slot");
        Self {
            slots: (0..capacity)
                .map(|_| Slot { state: SlotState::Idle, pinned_offsets: Vec::new() })
                .collect(),
            last_signal_us: 0,
            gpu_time_consumed_us: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    pub fn in_flight(&self) -> usize {
        self.slots.iter().filter(|s| matches!(s.state, SlotState::InFlight { .. })).count()
    }

    /// The submit-or-wait decision. Deterministic: prefers the lowest free
    /// slot index; when full, names the OLDEST in-flight slot (earliest
    /// submit time) as the one to wait on — FIFO completion order matches
    /// FIFO submission on a single hardware queue, so waiting on the oldest
    /// bounds the wait.
    pub fn admission(&self) -> Admission {
        if let Some(i) = self.slots.iter().position(|s| s.state == SlotState::Idle) {
            return Admission::Submit { slot: i };
        }
        let oldest = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s.state {
                SlotState::InFlight { submit_us } => Some((submit_us, i)),
                _ => None,
            })
            .min()
            .map(|(_, i)| i)
            .expect("no idle slot and no in-flight slot: a slot is stuck in Recording");
        Admission::WaitOldest { slot: oldest }
    }

    /// Acquire `slot` for recording. Caller got `slot` from
    /// [`Self::admission`]; acquiring a non-idle slot is a caller bug.
    pub fn acquire(&mut self, slot: usize, pinned_offsets: Vec<u64>) {
        let s = &mut self.slots[slot];
        assert_eq!(s.state, SlotState::Idle, "acquire of non-idle slot {slot}");
        s.state = SlotState::Recording;
        s.pinned_offsets = pinned_offsets;
    }

    /// Mark `slot` submitted to the queue at `now_us`.
    pub fn submitted(&mut self, slot: usize, now_us: u64) {
        let s = &mut self.slots[slot];
        assert_eq!(s.state, SlotState::Recording, "submit of non-recording slot {slot}");
        s.state = SlotState::InFlight { submit_us: now_us };
    }

    /// Abort a slot that was acquired (and possibly recorded) but will NOT
    /// be submitted — e.g. every chunk in the batch failed pool upload.
    /// Returns the pinned offsets so the caller can unpin immediately.
    #[must_use = "caller must unpin the returned offsets"]
    pub fn abort_recording(&mut self, slot: usize) -> Vec<u64> {
        let s = &mut self.slots[slot];
        assert_eq!(s.state, SlotState::Recording, "abort of non-recording slot {slot}");
        s.state = SlotState::Idle;
        std::mem::take(&mut s.pinned_offsets)
    }

    /// The caller observed `slot`'s fence signaled at `now_us`. Releases the
    /// slot and returns its pinned offsets for `unpin_spans`.
    ///
    /// GPU-time accounting: the dispatch is charged
    /// `now_us − max(submit_us, previous_signal_us)` — the interval this
    /// dispatch occupied the queue EXCLUSIVELY. On a single in-order queue,
    /// overlapped dispatches execute back-to-back, so summing raw
    /// (signal − submit) wall times would double-count the overlap; this
    /// formula partitions wall time so the total never exceeds it. That is
    /// the number a per-frame GPU-mesh budget must charge (memo sub-stage C).
    #[must_use = "caller must unpin the returned offsets"]
    pub fn complete(&mut self, slot: usize, now_us: u64) -> Vec<u64> {
        let submit_us = match self.slots[slot].state {
            SlotState::InFlight { submit_us } => submit_us,
            other => panic!("complete of slot {slot} in state {other:?}"),
        };
        debug_assert!(now_us >= submit_us, "fence signaled before submit");
        let exclusive_start = submit_us.max(self.last_signal_us);
        self.gpu_time_consumed_us += now_us.saturating_sub(exclusive_start);
        self.last_signal_us = self.last_signal_us.max(now_us);
        let s = &mut self.slots[slot];
        s.state = SlotState::Idle;
        std::mem::take(&mut s.pinned_offsets)
    }

    /// Total overlap-deduplicated GPU time consumed since construction (or
    /// last [`Self::take_gpu_time_consumed`]).
    pub fn gpu_time_consumed_us(&self) -> u64 {
        self.gpu_time_consumed_us
    }

    /// Drain the consumed-time counter (per-window budget accounting).
    pub fn take_gpu_time_consumed(&mut self) -> u64 {
        std::mem::take(&mut self.gpu_time_consumed_us)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_one_reproduces_synchronous_behavior() {
        // N=1: every admission after a submit is WaitOldest on that slot —
        // submit → wait → readback, exactly today's loop.
        let mut r = DispatchRing::new(1);
        assert_eq!(r.admission(), Admission::Submit { slot: 0 });
        r.acquire(0, vec![100]);
        r.submitted(0, 1_000);
        assert_eq!(r.admission(), Admission::WaitOldest { slot: 0 });
        let pins = r.complete(0, 6_000);
        assert_eq!(pins, vec![100]);
        assert_eq!(r.admission(), Admission::Submit { slot: 0 });
        assert_eq!(r.gpu_time_consumed_us(), 5_000);
    }

    #[test]
    fn two_slots_overlap_and_backpressure_names_oldest() {
        let mut r = DispatchRing::new(2);
        r.acquire(0, vec![1]);
        r.submitted(0, 0);
        assert_eq!(r.admission(), Admission::Submit { slot: 1 });
        r.acquire(1, vec![2]);
        r.submitted(1, 10);
        assert_eq!(r.in_flight(), 2);
        // Full: wait target is slot 0 (submitted earliest).
        assert_eq!(r.admission(), Admission::WaitOldest { slot: 0 });
        let _ = r.complete(0, 100);
        assert_eq!(r.admission(), Admission::Submit { slot: 0 });
    }

    #[test]
    fn overlapped_gpu_time_is_not_double_counted() {
        // Two dispatches on one in-order queue: A submits at 0, B at 10.
        // A signals at 100, B at 150. Raw wall sum = 100 + 140 = 240, but
        // the queue was only busy 150us total. Accounting: A charged
        // 100−max(0,0)=100; B charged 150−max(10,100)=50. Total 150 = wall.
        let mut r = DispatchRing::new(2);
        r.acquire(0, vec![]);
        r.submitted(0, 0);
        r.acquire(1, vec![]);
        r.submitted(1, 10);
        let _ = r.complete(0, 100);
        let _ = r.complete(1, 150);
        assert_eq!(r.gpu_time_consumed_us(), 150);
    }

    #[test]
    fn non_overlapped_dispatches_charge_full_wall_time() {
        let mut r = DispatchRing::new(2);
        r.acquire(0, vec![]);
        r.submitted(0, 0);
        let _ = r.complete(0, 40);
        // Queue idle 40..100.
        r.acquire(0, vec![]);
        r.submitted(0, 100);
        let _ = r.complete(0, 130);
        assert_eq!(r.gpu_time_consumed_us(), 40 + 30, "idle gaps are not charged");
    }

    #[test]
    fn abort_returns_pins_and_frees_slot() {
        let mut r = DispatchRing::new(1);
        r.acquire(0, vec![7, 8]);
        // Whole batch failed pool upload — nothing to submit.
        let pins = r.abort_recording(0);
        assert_eq!(pins, vec![7, 8]);
        assert_eq!(r.admission(), Admission::Submit { slot: 0 });
        assert_eq!(r.gpu_time_consumed_us(), 0);
    }

    #[test]
    fn complete_hands_back_exactly_the_batch_pins() {
        let mut r = DispatchRing::new(2);
        r.acquire(0, vec![10, 20]);
        r.submitted(0, 0);
        r.acquire(1, vec![30]);
        r.submitted(1, 5);
        // Out-of-order observation (poll saw slot 1 first) is legal for the
        // LEDGER even though the queue is FIFO — pins route to the right
        // batch regardless.
        let p1 = r.complete(1, 50);
        assert_eq!(p1, vec![30]);
        let p0 = r.complete(0, 60);
        assert_eq!(p0, vec![10, 20]);
        assert_eq!(r.in_flight(), 0);
    }

    #[test]
    fn take_gpu_time_supports_windowed_budget() {
        let mut r = DispatchRing::new(1);
        r.acquire(0, vec![]);
        r.submitted(0, 0);
        let _ = r.complete(0, 4_000);
        assert_eq!(r.take_gpu_time_consumed(), 4_000);
        assert_eq!(r.gpu_time_consumed_us(), 0, "window drained");
    }
}
