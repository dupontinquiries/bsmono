//! jitter.rs — receive-side jitter buffer for group voice.
//!
//! Frames arrive over the network out of order, duplicated, late, or not at all.
//! Play-out, by contrast, must be steady: one frame every frame-interval. This
//! buffer absorbs that mismatch. The sender stamps each frame with a sequence
//! number (see `wire::VoiceFrame`); the receiver `push`es arriving frames (in
//! any order) and the audio play-out thread `pop`s exactly one result per frame
//! interval. `pop` returns the next in-order frame, a `Conceal` signal when a
//! frame is missing (the decoder runs Opus PLC and we skip it), or `Silence`
//! when the buffer isn't primed / has run dry.
//!
//! It is codec-agnostic and transport-agnostic: it stores opaque payloads keyed
//! by sequence number, so it has no audio or network dependency and is fully
//! unit-testable. Decoding happens after `pop`, not inside.

use std::collections::BTreeMap;

/// Jitter buffer tuning.
#[derive(Debug, Clone, Copy)]
pub struct JitterConfig {
    /// Frames to accumulate before play-out starts (and re-starts after a dry
    /// spell). Higher = more latency but fewer concealments under jitter.
    pub target_frames: u32,
    /// Hard cap on buffered frames; arrivals beyond this drop the oldest, so a
    /// burst or a stalled consumer can't grow latency/memory without bound.
    pub max_frames: u32,
}

impl Default for JitterConfig {
    fn default() -> Self {
        // 3-frame prime @ 20 ms ≈ 60 ms of buffering; cap at ~1 s.
        Self { target_frames: 3, max_frames: 50 }
    }
}

impl JitterConfig {
    fn sane(self) -> Self {
        let target = self.target_frames.max(1);
        let max = self.max_frames.max(target);
        Self { target_frames: target, max_frames: max }
    }
}

/// One play-out result.
#[derive(Debug, PartialEq, Eq)]
pub enum JitterOut {
    /// A frame is ready — its encoded payload (decode + play it).
    Frame(Vec<u8>),
    /// The next frame is missing but later frames exist — conceal (Opus PLC) and
    /// advance past the gap.
    Conceal,
    /// Buffer isn't primed yet, or has run dry — play silence; the play cursor
    /// does not advance.
    Silence,
}

/// Running counters, for diagnostics / adaptive tuning.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct JitterStats {
    pub received: u64,  // push() calls that were accepted
    pub played: u64,    // Frame results returned
    pub concealed: u64, // Conceal results (gaps)
    pub late: u64,      // arrivals dropped for being before the play cursor
    pub duplicate: u64, // arrivals dropped as duplicates
    pub overflow: u64,  // frames evicted by the max_frames cap
}

/// Reorders + paces voice frames between an unreliable network and steady play-out.
#[derive(Debug)]
pub struct JitterBuffer {
    cfg: JitterConfig,
    frames: BTreeMap<u32, Vec<u8>>,
    /// Next sequence number to play; `None` until primed.
    next_seq: Option<u32>,
    primed: bool,
    stats: JitterStats,
}

impl JitterBuffer {
    pub fn new(cfg: JitterConfig) -> Self {
        Self {
            cfg: cfg.sane(),
            frames: BTreeMap::new(),
            next_seq: None,
            primed: false,
            stats: JitterStats::default(),
        }
    }

    /// Insert an arriving frame. Silently drops duplicates and frames already
    /// played past (late), and evicts the oldest if over the cap.
    pub fn push(&mut self, seq: u32, payload: Vec<u8>) {
        // Already played past this sequence → too late to use.
        if let Some(next) = self.next_seq {
            if seq < next {
                self.stats.late += 1;
                return;
            }
        }
        if self.frames.contains_key(&seq) {
            self.stats.duplicate += 1;
            return;
        }
        self.frames.insert(seq, payload);
        self.stats.received += 1;

        // Enforce the cap by evicting the oldest. If we evict what would have
        // been the next frame to play, advance the cursor past it so we don't
        // later conceal a frame we deliberately dropped.
        while self.frames.len() as u32 > self.cfg.max_frames {
            let oldest = match self.frames.keys().next().copied() {
                Some(k) => k,
                None => break,
            };
            self.frames.remove(&oldest);
            self.stats.overflow += 1;
            if let Some(next) = self.next_seq {
                if oldest >= next {
                    self.next_seq = Some(oldest + 1);
                }
            }
        }
    }

    /// Produce one play-out result. Call once per frame interval.
    pub fn pop(&mut self) -> JitterOut {
        // Prime: wait until enough is buffered before we begin (and after a dry
        // spell we re-prime, which is why this is checked every pop).
        if !self.primed {
            if (self.frames.len() as u32) < self.cfg.target_frames {
                return JitterOut::Silence;
            }
            self.primed = true;
            self.next_seq = self.frames.keys().next().copied();
        }

        let next = match self.next_seq {
            Some(n) => n,
            None => return JitterOut::Silence,
        };

        if let Some(payload) = self.frames.remove(&next) {
            self.next_seq = Some(next.wrapping_add(1));
            self.stats.played += 1;
            JitterOut::Frame(payload)
        } else if self.frames.is_empty() {
            // Underrun: nothing buffered at all → silence and re-prime so we
            // rebuild a cushion before resuming (avoids choppy single-frame play).
            self.primed = false;
            self.next_seq = None;
            JitterOut::Silence
        } else {
            // The expected frame is missing but later ones exist → it was lost.
            // Conceal it and step over the gap.
            self.next_seq = Some(next.wrapping_add(1));
            self.stats.concealed += 1;
            JitterOut::Conceal
        }
    }

    /// Frames currently buffered.
    pub fn buffered(&self) -> usize {
        self.frames.len()
    }

    /// Whether play-out has primed and is running.
    pub fn is_primed(&self) -> bool {
        self.primed
    }

    pub fn stats(&self) -> JitterStats {
        self.stats
    }

    /// Clear everything and un-prime — call at the start of a new talk-spurt
    /// (the sequence numbering restarts), or on channel teardown. Keeps stats.
    pub fn reset(&mut self) {
        self.frames.clear();
        self.next_seq = None;
        self.primed = false;
    }
}

// ─── FFI ─────────────────────────────────────────────────────────────────────
//
// The C++ receive path pushes opened+decoded wire frames (seq + Opus payload)
// as they arrive, and the audio play-out timer pops once per frame interval:
// 0 → decode+play the payload, 1 → conceal (Opus PLC), 2 → silence.

use std::os::raw::c_int;

/// Create a jitter buffer. Free with `sc_jitter_free`.
#[no_mangle]
pub extern "C" fn sc_jitter_new(target_frames: u32, max_frames: u32) -> *mut JitterBuffer {
    Box::into_raw(Box::new(JitterBuffer::new(JitterConfig {
        target_frames,
        max_frames,
    })))
}

/// Free a jitter buffer.
///
/// # Safety
/// `handle` from `sc_jitter_new`, not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_jitter_free(handle: *mut JitterBuffer) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// Push an arriving frame: its sequence number and encoded payload. Null-safe.
///
/// # Safety
/// `payload` valid for `payload_len` (or null); `handle` from `sc_jitter_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_jitter_push(
    handle: *mut JitterBuffer,
    seq: u32,
    payload: *const u8,
    payload_len: usize,
) {
    if handle.is_null() {
        return;
    }
    let p = if payload.is_null() || payload_len == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(payload, payload_len).to_vec()
    };
    (*handle).push(seq, p);
}

/// Pop one play-out result. Returns: 0 = a frame was written to `out` (length in
/// `out_len`) — decode + play it; 1 = conceal (run Opus PLC); 2 = silence; -1 =
/// null handle; -2 = a frame is ready but `out` is too small (`out_len` holds the
/// required size; the frame IS consumed, so size `out_cap` to the largest
/// expected encoded frame, e.g. 4096).
///
/// # Safety
/// `out` writable for `out_cap`; `out_len` a valid `*mut usize`; `handle` from
/// `sc_jitter_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_jitter_pop(
    handle: *mut JitterBuffer,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> c_int {
    if handle.is_null() || out_len.is_null() {
        return -1;
    }
    match (*handle).pop() {
        JitterOut::Frame(payload) => {
            *out_len = payload.len();
            if out.is_null() || payload.len() > out_cap {
                return -2;
            }
            std::ptr::copy_nonoverlapping(payload.as_ptr(), out, payload.len());
            0
        }
        JitterOut::Conceal => {
            *out_len = 0;
            1
        }
        JitterOut::Silence => {
            *out_len = 0;
            2
        }
    }
}

/// Frames currently buffered (0 on null).
///
/// # Safety
/// `handle` from `sc_jitter_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_jitter_buffered(handle: *const JitterBuffer) -> c_int {
    if handle.is_null() {
        return 0;
    }
    (*handle).buffered() as c_int
}

/// Clear + un-prime (new talk-spurt / teardown). Null-safe.
///
/// # Safety
/// `handle` from `sc_jitter_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_jitter_reset(handle: *mut JitterBuffer) {
    if !handle.is_null() {
        (*handle).reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(target: u32, max: u32) -> JitterConfig {
        JitterConfig { target_frames: target, max_frames: max }
    }
    fn p(b: u8) -> Vec<u8> {
        vec![b]
    }

    #[test]
    fn silence_until_primed_then_plays_in_order() {
        let mut j = JitterBuffer::new(cfg(3, 50));
        j.push(0, p(0));
        assert_eq!(j.pop(), JitterOut::Silence); // 1 < 3 buffered
        j.push(1, p(1));
        assert_eq!(j.pop(), JitterOut::Silence); // 2 < 3
        j.push(2, p(2));
        // primed: plays 0,1,2 in order
        assert_eq!(j.pop(), JitterOut::Frame(p(0)));
        assert_eq!(j.pop(), JitterOut::Frame(p(1)));
        assert_eq!(j.pop(), JitterOut::Frame(p(2)));
    }

    #[test]
    fn reorders_out_of_order_arrivals() {
        let mut j = JitterBuffer::new(cfg(3, 50));
        j.push(2, p(2));
        j.push(0, p(0));
        j.push(1, p(1)); // primed now (3 buffered), arrived scrambled
        assert_eq!(j.pop(), JitterOut::Frame(p(0)));
        assert_eq!(j.pop(), JitterOut::Frame(p(1)));
        assert_eq!(j.pop(), JitterOut::Frame(p(2)));
    }

    #[test]
    fn conceals_a_lost_frame() {
        let mut j = JitterBuffer::new(cfg(1, 50));
        // seq 1 never arrives; 0 then 2,3 do.
        j.push(0, p(0));
        assert_eq!(j.pop(), JitterOut::Frame(p(0)));
        j.push(2, p(2));
        j.push(3, p(3));
        // expected seq 1 missing, later frames exist → conceal + skip
        assert_eq!(j.pop(), JitterOut::Conceal);
        assert_eq!(j.pop(), JitterOut::Frame(p(2)));
        assert_eq!(j.pop(), JitterOut::Frame(p(3)));
        assert_eq!(j.stats().concealed, 1);
    }

    #[test]
    fn drops_late_arrival() {
        let mut j = JitterBuffer::new(cfg(1, 50));
        j.push(5, p(5));
        assert_eq!(j.pop(), JitterOut::Frame(p(5))); // cursor now at 6
        j.push(4, p(4)); // 4 < 6 → too late
        assert_eq!(j.stats().late, 1);
        assert_eq!(j.buffered(), 0);
    }

    #[test]
    fn drops_duplicate() {
        let mut j = JitterBuffer::new(cfg(2, 50));
        j.push(0, p(0));
        j.push(0, p(99)); // duplicate seq
        assert_eq!(j.stats().duplicate, 1);
        assert_eq!(j.buffered(), 1);
    }

    #[test]
    fn underrun_reprimes() {
        let mut j = JitterBuffer::new(cfg(2, 50));
        j.push(0, p(0));
        j.push(1, p(1));
        assert_eq!(j.pop(), JitterOut::Frame(p(0)));
        assert_eq!(j.pop(), JitterOut::Frame(p(1)));
        // dry now → silence + un-prime
        assert_eq!(j.pop(), JitterOut::Silence);
        assert!(!j.is_primed());
        // need target_frames again before resuming
        j.push(2, p(2));
        assert_eq!(j.pop(), JitterOut::Silence); // 1 < 2
        j.push(3, p(3));
        assert_eq!(j.pop(), JitterOut::Frame(p(2)));
    }

    #[test]
    fn cap_evicts_oldest_and_advances_cursor() {
        let mut j = JitterBuffer::new(cfg(1, 3));
        // fill beyond cap while NOT primed; oldest gets evicted
        for s in 0..5u32 {
            j.push(s, p(s as u8));
        }
        assert_eq!(j.buffered(), 3); // capped
        assert!(j.stats().overflow >= 2);
        // the survivors are the newest: 2,3,4 → first pop is 2 (not a stale 0/1)
        assert_eq!(j.pop(), JitterOut::Frame(p(2)));
        assert_eq!(j.pop(), JitterOut::Frame(p(3)));
        assert_eq!(j.pop(), JitterOut::Frame(p(4)));
    }

    #[test]
    fn reset_clears_and_unprimes() {
        let mut j = JitterBuffer::new(cfg(2, 50));
        j.push(10, p(1));
        j.push(11, p(2));
        assert_eq!(j.pop(), JitterOut::Frame(p(1)));
        j.reset();
        assert_eq!(j.buffered(), 0);
        assert!(!j.is_primed());
        // a fresh talk-spurt starting at seq 0 plays cleanly
        j.push(0, p(7));
        j.push(1, p(8));
        assert_eq!(j.pop(), JitterOut::Frame(p(7)));
    }

    #[test]
    fn ffi_roundtrip() {
        unsafe {
            let h = sc_jitter_new(2, 50);
            assert!(!h.is_null());
            let mut out = [0u8; 64];
            let mut n: usize = 0;

            // not primed → silence (2)
            sc_jitter_push(h, 0, [0xAAu8].as_ptr(), 1);
            assert_eq!(sc_jitter_pop(h, out.as_mut_ptr(), out.len(), &mut n), 2);

            // prime with a 2nd frame, then frames come out in order
            sc_jitter_push(h, 1, [0xBBu8].as_ptr(), 1);
            assert_eq!(sc_jitter_buffered(h), 2);
            assert_eq!(sc_jitter_pop(h, out.as_mut_ptr(), out.len(), &mut n), 0);
            assert_eq!(&out[..n], &[0xAA]);
            assert_eq!(sc_jitter_pop(h, out.as_mut_ptr(), out.len(), &mut n), 0);
            assert_eq!(&out[..n], &[0xBB]);
            // dry → silence
            assert_eq!(sc_jitter_pop(h, out.as_mut_ptr(), out.len(), &mut n), 2);

            // null-safety
            assert_eq!(sc_jitter_pop(std::ptr::null_mut(), out.as_mut_ptr(), out.len(), &mut n), -1);
            assert_eq!(sc_jitter_buffered(std::ptr::null()), 0);
            sc_jitter_reset(h);
            assert_eq!(sc_jitter_buffered(h), 0);
            sc_jitter_free(h);

            // conceal path: gap between played frames → 1
            let h2 = sc_jitter_new(1, 50);
            sc_jitter_push(h2, 0, [1u8].as_ptr(), 1);
            assert_eq!(sc_jitter_pop(h2, out.as_mut_ptr(), out.len(), &mut n), 0);
            sc_jitter_push(h2, 2, [2u8].as_ptr(), 1); // seq 1 lost
            assert_eq!(sc_jitter_pop(h2, out.as_mut_ptr(), out.len(), &mut n), 1);
            sc_jitter_free(h2);
        }
    }
}
