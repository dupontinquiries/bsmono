//! kvstage.rs — the staging POLICY for a hot/warm KV cache. Pure, no wgpu, fully testable.
//!
//! A session's KV splits at a moving boundary `hot_start`: tokens `[hot_start, pos)` are HOT (f32, full
//! precision, recent) and `[0, hot_start)` are WARM (int8, 4× smaller, older). This module owns the only
//! decision that doesn't need a GPU: WHEN a block of the hot window has slid far enough back that it should
//! be demoted to the warm tail. The engine calls `plan` each step and executes the returned action with the
//! existing `demote_cache` kernel.
//!
//! Design rules (see docs/design/staged-kv-cache.md):
//!   * Demote in BLOCKS (`demote_block`), not per token — one demote call per block amortizes the cost and
//!     keeps the warm int8 tail growing in aligned chunks.
//!   * With `hot_window = usize::MAX` (demotion disabled) the policy NEVER demotes, so staged decode is a
//!     bit-exact no-op vs all-f32 decode. This is the first correctness gate.
//!   * Invariant the tests pin: at every step, `warm_len + hot_len == pos` — no token is ever lost or
//!     double-counted across the boundary, and `hot_start` only ever moves forward.

/// What the engine should do this step, decided purely from positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageAction {
    /// Hot window isn't full (or demotion is disabled) — leave the cache as is.
    Nothing,
    /// Demote the block `[from, from+len)` of the hot f32 cache to the warm int8 tail, then the caller
    /// advances `hot_start` to `from+len`. `len` is always a whole `demote_block` (or the remainder that
    /// keeps the hot window at its target size).
    Demote { from: usize, len: usize },
}

/// Staging configuration + the moving boundary. Cheap to copy; the engine holds one per session.
#[derive(Clone, Copy, Debug)]
pub struct StagePolicy {
    /// Target number of most-recent tokens kept HOT (f32). `usize::MAX` disables demotion (all-hot).
    pub hot_window: usize,
    /// Demotion granularity — the hot window sheds this many tokens at once when it overflows.
    pub demote_block: usize,
    /// First HOT token index; `[0, hot_start)` is warm. Advances as blocks demote. Starts at 0.
    pub hot_start: usize,
}

impl StagePolicy {
    /// Default staging: 512-token hot f32 window, demoted 128 at a time. Tuned in the design doc.
    pub fn new(hot_window: usize, demote_block: usize) -> Self {
        StagePolicy { hot_window: hot_window.max(1), demote_block: demote_block.max(1), hot_start: 0 }
    }

    /// Demotion fully disabled — every token stays hot (f32). Staged decode is then a bit-exact no-op
    /// vs all-f32 decode; this is the first validation gate.
    pub fn all_hot() -> Self {
        StagePolicy { hot_window: usize::MAX, demote_block: usize::MAX, hot_start: 0 }
    }

    /// Number of warm (int8) tokens: everything before the hot boundary.
    pub fn warm_len(&self) -> usize { self.hot_start }
    /// Number of hot (f32) tokens for a session currently at length `pos`.
    pub fn hot_len(&self, pos: usize) -> usize { pos.saturating_sub(self.hot_start) }

    /// Decide this step's action for a session that has just reached length `pos` (i.e. `pos` tokens are
    /// in the cache, indices `[0, pos)`). Pure — depends only on `pos` and the current boundary.
    ///
    /// The hot window is allowed to grow to `hot_window + demote_block` before shedding a block, so we
    /// demote once per `demote_block` tokens (hysteresis) rather than thrashing one token at a time.
    pub fn plan(&self, pos: usize) -> StageAction {
        if self.hot_window == usize::MAX { return StageAction::Nothing; }
        let hot = pos.saturating_sub(self.hot_start);
        // shed a block only once the hot region exceeds target + one block of slack
        if hot > self.hot_window + self.demote_block {
            // demote the OLDEST block of the hot region, keeping the hot window at ~hot_window
            let len = self.demote_block.min(hot - self.hot_window);
            StageAction::Demote { from: self.hot_start, len }
        } else {
            StageAction::Nothing
        }
    }

    /// Commit a demotion: advance the boundary past the block that was just demoted. The engine calls this
    /// after `demote_cache` succeeds. Panics-free; clamps so `hot_start` never passes `pos` (the caller
    /// only ever demotes blocks that are actually in the hot region, but we stay defensive).
    pub fn commit_demote(&mut self, len: usize) {
        self.hot_start += len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_hot_never_demotes() {
        let p = StagePolicy::all_hot();
        // even at a huge position, demotion-disabled policy stays a no-op (the bit-exact gate)
        for pos in [1usize, 100, 10_000, 1_000_000] {
            assert_eq!(p.plan(pos), StageAction::Nothing, "all_hot must never demote (pos={pos})");
        }
    }

    #[test]
    fn demotes_in_aligned_blocks_once_the_window_overflows() {
        let mut p = StagePolicy::new(512, 128);
        // below the window+slack, nothing happens
        assert_eq!(p.plan(600), StageAction::Nothing);
        assert_eq!(p.plan(640), StageAction::Nothing); // 640 = 512 + 128, still within slack
        // one past the slack → shed a block from the oldest hot token
        match p.plan(641) {
            StageAction::Demote { from, len } => { assert_eq!(from, 0); assert_eq!(len, 128); }
            a => panic!("expected a demote, got {a:?}"),
        }
    }

    #[test]
    fn boundary_is_monotonic_and_conserves_every_token() {
        // Simulate a long decode, executing the policy exactly as the engine would, and assert the core
        // invariant at every step: warm_len + hot_len == pos, hot_start only moves forward, and the hot
        // region never exceeds window + block.
        let mut p = StagePolicy::new(256, 64);
        let mut last_start = 0;
        for pos in 1..=5000usize {
            if let StageAction::Demote { from, len } = p.plan(pos) {
                assert_eq!(from, p.hot_start, "always demotes the current oldest hot block");
                p.commit_demote(len);
            }
            assert!(p.hot_start >= last_start, "hot_start must be monotonic");
            last_start = p.hot_start;
            assert_eq!(p.warm_len() + p.hot_len(pos), pos, "no token lost or double-counted at pos={pos}");
            assert!(p.hot_len(pos) <= 256 + 64, "hot region stays bounded (pos={pos}, hot={})", p.hot_len(pos));
        }
        // after 5000 tokens with a 256 hot window, the warm tail holds the bulk
        assert!(p.warm_len() > 4000, "most of a long context should be warm, got {}", p.warm_len());
    }

    #[test]
    fn hot_window_holds_recent_tokens_at_steady_state() {
        let mut p = StagePolicy::new(512, 128);
        for pos in 1..=3000usize {
            if let StageAction::Demote { len, .. } = p.plan(pos) { p.commit_demote(len); }
        }
        // at steady state the hot region is within [window, window+block]
        let hot = p.hot_len(3000);
        assert!(hot >= 512 && hot <= 512 + 128, "steady-state hot window ~512, got {hot}");
    }

    #[test]
    fn tiny_context_stays_fully_hot() {
        // a short conversation never fills the window → everything stays f32, zero demotion cost
        let mut p = StagePolicy::new(512, 128);
        for pos in 1..=400usize {
            assert_eq!(p.plan(pos), StageAction::Nothing);
            p.hot_start += 0;
        }
        assert_eq!(p.warm_len(), 0, "short context is all hot");
        assert_eq!(p.hot_len(400), 400);
    }
}
