//! capture — the negotiation half of `ext-image-copy-capture-v1`: what
//! buffer shape a source can produce, tracked purely, before any real
//! buffer ever gets allocated.
//!
//! ## Scope, stated plainly rather than implied
//!
//! This module (and `wayland_io`'s wiring for it) covers session creation
//! through buffer-constraint discovery — a caller learns what size and
//! `wl_shm` formats a capture source can produce. It deliberately stops
//! there. Actually allocating a real `wl_shm`/`wl_buffer` (a `memfd`, an
//! `mmap`, computing stride and format correctly, attaching, requesting a
//! frame, and reading the pixels back) is genuine buffer/memory-management
//! work — a different kind of task than the protocol-state tracking this
//! crate does everywhere else, and one worth scoping as its own piece
//! rather than rushing here to declare this protocol "done". A caller
//! that's already decided how it wants to allocate GPU/SHM buffers
//! (an egui backend already has opinions about this) is better positioned
//! to write that half than a protocol-integration kit guessing at it.
//!
//! Two capture sources this crate cares about, matching what a shell
//! actually wants: a specific toplevel (a live thumbnail for a switcher —
//! `CaptureSource::Toplevel` carries the SAME `u64` id `toplevel.rs`
//! assigns, so a caller can go from "this switcher tile" to "capture its
//! current contents" without a second id space to keep in sync) or the
//! whole output (a screenshot).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureSource {
    /// The same `u64` id space as `toplevel::ToplevelTracker` — not a
    /// coincidence, a deliberate shared identity so a switcher tile and
    /// its thumbnail capture are unambiguously the same window.
    Toplevel(u64),
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferConstraints {
    pub width: u32,
    pub height: u32,
    /// Raw `wl_shm::Format` enum values, in the order the compositor sent
    /// them — not deduplicated or reordered, since "which format the
    /// compositor listed first" can be a real preference signal a caller
    /// might want to respect rather than lose.
    pub shm_formats: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureEvent {
    BufferSize { width: u32, height: u32 },
    ShmFormat(u32),
    /// All constraint events for this negotiation round have been sent —
    /// the protocol's own signal that `BufferConstraints` can now be
    /// considered complete, not a fixed count of expected events (the
    /// number of `ShmFormat` events genuinely varies by compositor/source).
    Done,
    /// The source stopped being capturable (a toplevel closed, an output
    /// was unplugged) — distinct from `Done`, which just means "this round
    /// of negotiation finished", not "this source is gone for good".
    Stopped,
}

#[derive(Debug, Default)]
pub struct CaptureSession {
    // Accumulated across BufferSize/ShmFormat events until Done finalizes
    // them into `constraints` — mirrors the protocol's own double-buffered
    // intent for THIS negotiation, unlike toplevel.rs's per-field apply
    // (recorded there as a known simplification). Worth doing properly
    // here specifically because a caller reading `constraints` mid-update
    // would see a nonsensical partial buffer size, not just a stale value.
    pending_width: Option<u32>,
    pending_height: Option<u32>,
    pending_formats: Vec<u32>,
    pub constraints: Option<BufferConstraints>,
    pub stopped: bool,
}

impl CaptureSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, event: CaptureEvent) {
        match event {
            CaptureEvent::BufferSize { width, height } => {
                self.pending_width = Some(width);
                self.pending_height = Some(height);
            }
            CaptureEvent::ShmFormat(fmt) => self.pending_formats.push(fmt),
            CaptureEvent::Done => {
                // A Done with no BufferSize ever received (a malformed or
                // reordered stream) leaves `constraints` at whatever it
                // was before — None on the first round, or the last
                // COMPLETE negotiation's result on a later round — rather
                // than finalizing a half-formed, meaningless buffer size.
                if let (Some(w), Some(h)) = (self.pending_width.take(), self.pending_height.take()) {
                    self.constraints =
                        Some(BufferConstraints { width: w, height: h, shm_formats: std::mem::take(&mut self.pending_formats) });
                } else {
                    self.pending_formats.clear();
                }
            }
            CaptureEvent::Stopped => self.stopped = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_negotiation_round_produces_complete_constraints() {
        let mut s = CaptureSession::new();
        s.apply(CaptureEvent::BufferSize { width: 640, height: 480 });
        s.apply(CaptureEvent::ShmFormat(1)); // e.g. Xrgb8888
        s.apply(CaptureEvent::ShmFormat(0)); // e.g. Argb8888
        assert!(s.constraints.is_none(), "not finalized until Done");
        s.apply(CaptureEvent::Done);
        assert_eq!(
            s.constraints,
            Some(BufferConstraints { width: 640, height: 480, shm_formats: vec![1, 0] })
        );
    }

    #[test]
    fn format_order_is_preserved_not_deduplicated_or_sorted() {
        let mut s = CaptureSession::new();
        s.apply(CaptureEvent::BufferSize { width: 1, height: 1 });
        s.apply(CaptureEvent::ShmFormat(5));
        s.apply(CaptureEvent::ShmFormat(5)); // a compositor listing the same format twice is plausible, not a bug to hide
        s.apply(CaptureEvent::ShmFormat(2));
        s.apply(CaptureEvent::Done);
        assert_eq!(s.constraints.unwrap().shm_formats, vec![5, 5, 2]);
    }

    #[test]
    fn a_done_with_no_buffer_size_leaves_constraints_untouched() {
        let mut s = CaptureSession::new();
        s.apply(CaptureEvent::ShmFormat(1)); // formats with no size -- malformed/reordered stream
        s.apply(CaptureEvent::Done);
        assert!(s.constraints.is_none(), "a half-formed buffer size must not finalize into something meaningless");
    }

    #[test]
    fn a_second_negotiation_round_replaces_the_first_not_merges_with_it() {
        // A real compositor renegotiates on output/scale changes -- the
        // NEW round's formats must not silently append to the old round's.
        let mut s = CaptureSession::new();
        s.apply(CaptureEvent::BufferSize { width: 640, height: 480 });
        s.apply(CaptureEvent::ShmFormat(1));
        s.apply(CaptureEvent::Done);

        s.apply(CaptureEvent::BufferSize { width: 1280, height: 720 });
        s.apply(CaptureEvent::ShmFormat(2));
        s.apply(CaptureEvent::Done);

        assert_eq!(
            s.constraints,
            Some(BufferConstraints { width: 1280, height: 720, shm_formats: vec![2] }),
            "the second round's own formats only, not [1, 2] merged with the first"
        );
    }

    #[test]
    fn stopped_is_independent_of_whether_constraints_were_ever_received() {
        let mut s = CaptureSession::new();
        s.apply(CaptureEvent::Stopped);
        assert!(s.stopped);
        assert!(s.constraints.is_none(), "stopped before any negotiation completed is a real, valid state");
    }

    #[test]
    fn a_toplevel_capture_source_carries_the_same_id_space_as_toplevel_rs() {
        // Not a behavioral test so much as a compile-time promise: this
        // must accept the exact same u64 a ToplevelInfo::id already is,
        // with no conversion step a caller could get wrong in between.
        let id: u64 = 42;
        let source = CaptureSource::Toplevel(id);
        assert_eq!(source, CaptureSource::Toplevel(42));
    }
}
