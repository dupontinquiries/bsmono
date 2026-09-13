//! The video frame cache policy — what should be decoded and held, given
//! where the playhead is, where it's going, and how much room there is.
//!
//! ## The problem with a dense ring buffer
//!
//! The classic NLE cache is a fixed-length window of decoded frames that
//! follows the playhead. It has one knob — the window length — and that
//! knob is a straight trade between smoothness (window must be dense) and
//! reach (window must be long). It is also purely reactive: it starts
//! decoding frame N+1 once you're AT frame N, so a fast scrub always
//! outruns it, and it is usually forward-only to save memory, which is
//! exactly the wrong saving for a workload — scrubbing — that reverses
//! constantly.
//!
//! ## The shape used here instead
//!
//! Two independent falloffs with DISTANCE from the playhead, not one:
//!
//! * **Temporal density** — near the playhead, every frame; further out,
//!   every 2nd, then every 4th, doubling. This is a memory saving, not a
//!   decode-time saving: AV1's P-frames predict from the previous decoded
//!   frame, so producing frame 47 still means decoding 24..47 in order
//!   even if only the even ones are KEPT. Decimation without cheaper
//!   resolution just throws away decoded frames after paying for them.
//! * **Spatial resolution** — full near the playhead, proxy in the middle,
//!   thumbnail far out. This IS a decode-time saving (ffmpeg's own
//!   scaler decodes+scales for a fraction of full-res decode cost) and is
//!   what makes the temporal falloff worth anything: a far, sparse shell
//!   at THUMBNAIL resolution is what makes reaching a distant scrub cheap
//!   on both memory and CPU at once.
//!
//! Both falloffs are driven by a **predicted** position — playhead plus a
//! velocity estimate times a lookahead — not the current position, so
//! prefetch fires before the frame is needed rather than after. Velocity
//! also sets how the fetch budget splits between the two directions:
//! biased toward the direction of travel, but never zero on the other
//! side, because a reversal mid-scrub is the common case this whole
//! design exists for.
//!
//! ## Recompute, not LRU
//!
//! [`desired_set`] is a pure function: `(position, velocity, budget) ->
//! the exact set of (frame, tier) that should be cached right now`. The
//! caller diffs that against what it holds — evict what fell out, enqueue
//! what's missing — every time the playhead moves. No recency timestamps,
//! no cache-hit bookkeeping: the set is recomputed fresh each time, which
//! makes it possible to prove properties like "the far side is never
//! empty" and "a reversal is covered before it happens" as plain unit
//! tests rather than as emergent behaviour of an eviction heuristic.
//!
//! ## Hardware budget
//!
//! [`Budget`] is bytes, not frame count — a Full frame and a Thumbnail
//! frame differ by two orders of magnitude, and a byte budget is what
//! actually corresponds to "how much RAM this device has to give up for
//! video preview." [`Budget::for_device`] gives the two starting points
//! this was designed against: a phone-class budget forces a small,
//! spatially-biased shell (favour a wider thumbnail reach over a deep full-
//! res one, because reach protects against a jump-cut and full-res near
//! the playhead is what a still frame needs); a desktop-class budget can
//! afford both. Decode WORK is bounded separately, by [`JobQueue`] in
//! `prefetch.rs`, which is where cancellation lives — a device with one
//! decode thread must be able to kill a queued job the instant it falls
//! out of the desired set, or it spends its only thread on frames nobody
//! will see.

use std::collections::BTreeSet;

/// A frame's resolution class. Ascending cost order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SpatialTier {
    /// A few hundred pixels wide: a scrubber thumbnail, or the far shell.
    Thumbnail,
    /// A quarter to a third of native resolution: the mid shell, and what
    /// effects preview runs on while anything is moving.
    Proxy,
    /// Native resolution: only near the playhead, for a still frame.
    Full,
}

impl SpatialTier {
    /// Rough bytes per pixel at this tier's typical scale relative to a
    /// Full frame of `full_w x full_h` RGBA (4 bytes/px). Used to convert
    /// a byte budget into a frame count estimate; the real number depends
    /// on the source's actual resolution, which `CacheKey` accounting
    /// uses directly instead — this is only for `Budget::for_device`'s
    /// human-facing sizing.
    fn relative_area(self) -> f64 {
        match self {
            SpatialTier::Thumbnail => 1.0 / 64.0, // e.g. 1/8 linear scale
            SpatialTier::Proxy => 1.0 / 9.0,      // e.g. 1/3 linear scale
            SpatialTier::Full => 1.0,
        }
    }
}

/// A cached (or wanted) unit: one frame at one tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheKey {
    pub frame: i64,
    pub tier: SpatialTier,
}

/// Bytes available for the whole video frame cache. Bytes, not frame
/// count, because a Full frame and a Thumbnail frame differ by orders of
/// magnitude and "how much RAM to give up" is the question a device
/// actually answers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    pub bytes: u64,
}

impl Budget {
    /// Two starting points. Real tuning wants a device probe (available
    /// RAM, core count); these are the floor and a comfortable middle so
    /// the policy has honest numbers to be tested against.
    pub fn for_device(class: DeviceClass) -> Self {
        match class {
            // A Fairphone/PinePhone-class budget: tens of MB, not
            // hundreds. Spend it on REACH (thumbnail shell) over depth
            // (full-res shell), because a jump-cut with an empty far side
            // is a worse experience than a slightly softer near side.
            DeviceClass::Phone => Budget { bytes: 32 * 1024 * 1024 },
            DeviceClass::Desktop => Budget { bytes: 512 * 1024 * 1024 },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    Phone,
    Desktop,
}

/// One ring of the falloff: how far it reaches, at what temporal stride
/// and spatial tier, on each side of the predicted position.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ring {
    /// Frames from the predicted position where this ring ends (exclusive
    /// of the next ring's start).
    end_distance: i64,
    stride: i64,
    tier: SpatialTier,
}

/// The falloff table. Distances are in FRAMES, which makes it frame-rate-
/// independent in the sense that matters: a 60fps source's "near" ring
/// covers the same wall-clock time as a 24fps source's if the caller
/// scales `near_frames` by fps — `Shells::for_fps` does that scaling so
/// the ring boundaries are specified once, in seconds, here.
#[derive(Debug, Clone, PartialEq)]
pub struct Shells {
    rings: Vec<Ring>,
}

impl Shells {
    /// Ring boundaries in SECONDS of reach, scaled to `fps` frames. Doubling
    /// stride at each ring is the "temporal mipmap"; tier coarsens with it.
    pub fn for_fps(fps: f64) -> Self {
        let f = |secs: f64| (secs * fps).round() as i64;
        Self {
            rings: vec![
                Ring { end_distance: f(0.6), stride: 1, tier: SpatialTier::Full },
                Ring { end_distance: f(3.0), stride: 2, tier: SpatialTier::Proxy },
                Ring { end_distance: f(10.0), stride: 4, tier: SpatialTier::Proxy },
                Ring { end_distance: f(40.0), stride: 8, tier: SpatialTier::Thumbnail },
                Ring { end_distance: f(160.0), stride: 16, tier: SpatialTier::Thumbnail },
            ],
        }
    }

    fn ring_at(&self, distance: i64) -> Option<&Ring> {
        self.rings.iter().find(|r| distance < r.end_distance)
    }

    pub fn max_reach(&self) -> i64 {
        self.rings.last().map(|r| r.end_distance).unwrap_or(0)
    }
}

/// A short history of playhead positions, for a velocity estimate.
///
/// Exponentially-weighted rather than a plain last-two-samples delta: a
/// single noisy sample (a UI frame drop, a stutter in the drag event
/// stream) must not flip the predicted direction and yank the whole
/// prefetch shell to the wrong side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Velocity {
    /// Frames per second, signed: positive is forward.
    fps: f64,
    last_pos: Option<f64>,
    last_t: Option<f64>,
}

impl Default for Velocity {
    fn default() -> Self {
        Self { fps: 0.0, last_pos: None, last_t: None }
    }
}

impl Velocity {
    /// `pos` in frames, `t` a monotonic clock in seconds.
    pub fn observe(&mut self, pos: f64, t: f64) {
        if let (Some(lp), Some(lt)) = (self.last_pos, self.last_t) {
            let dt = (t - lt).max(1.0 / 240.0); // guard against a zero/negative tick
            let inst = (pos - lp) / dt;
            // A jump (seek) is not a velocity sample: it would report an
            // enormous instantaneous speed that then decays away slowly.
            // 500 fps of PLAYHEAD movement is already an extreme scrub —
            // 20 seconds of 24fps content covered in one second — so
            // anything past it reads as a jump, not a fast drag.
            if inst.abs() > 500.0 {
                self.fps = 0.0;
            } else {
                let alpha = 0.35; // smoothing: higher = trusts new samples more
                self.fps = alpha * inst + (1.0 - alpha) * self.fps;
            }
        }
        self.last_pos = Some(pos);
        self.last_t = Some(t);
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    /// Reset to stationary — call on an explicit seek/jump, where the
    /// smoothed velocity from before the jump has no bearing on where the
    /// user is headed next.
    pub fn reset(&mut self) {
        self.fps = 0.0;
    }

    /// Where the playhead is predicted to be in `lookahead` seconds.
    pub fn predict(&self, pos: f64, lookahead: f64) -> f64 {
        pos + self.fps * lookahead
    }

    /// Fraction of prefetch budget to spend FORWARD, in `[0.1, 0.9]`.
    /// Clamped well short of the ends on purpose: even a fast, confident
    /// scrub in one direction gets its budget capped at 90/10, never
    /// 100/0, because the direction that gets zero is exactly the one a
    /// reversal needs.
    pub fn forward_share(&self) -> f64 {
        // A logistic in fps, saturating by ~30 fps (a big, fast scrub).
        let x = self.fps / 30.0;
        let s = 1.0 / (1.0 + (-x).exp());
        s.clamp(0.1, 0.9)
    }
}

/// The exact set of (frame, tier) that should be cached right now.
/// Recomputed fresh on every call — see the module docs for why.
///
/// `position` is the CURRENT playhead in frames; `lookahead_secs` sets how
/// far ahead of it the shell is centred (0.0 centres on the current
/// position exactly, for a caller with no velocity estimate yet).
/// `valid_range` clamps the result to frames that exist in the source.
pub fn desired_set(
    position: i64,
    velocity: Velocity,
    fps: f64,
    lookahead_secs: f64,
    shells: &Shells,
    valid_range: std::ops::Range<i64>,
) -> BTreeSet<CacheKey> {
    let predicted = velocity.predict(position as f64, lookahead_secs).round() as i64;
    let forward = velocity.forward_share();
    let mut set = BTreeSet::new();

    // Walk outward on both sides independently so each side's stride and
    // tier come from ITS OWN distance, then stop early on whichever side
    // the budget-derived reach (folded into `shells` by the caller sizing
    // `Shells` to the device) says to stop at. The forward/backward SPLIT
    // here is expressed as how far each side is allowed to reach relative
    // to the full table, not as a hard frame cap, so a stationary playhead
    // (`forward_share` == 0.5) still gets the FULL table on both sides.
    let reach = shells.max_reach();
    let fwd_reach = (reach as f64 * (2.0 * forward).min(1.0)).round() as i64;
    let bwd_reach = (reach as f64 * (2.0 * (1.0 - forward)).min(1.0)).round() as i64;

    for dist in 0..reach {
        let Some(ring) = shells.ring_at(dist) else { break };
        if dist % ring.stride != 0 {
            continue;
        }
        if dist <= fwd_reach {
            let f = predicted + dist;
            if valid_range.contains(&f) {
                set.insert(CacheKey { frame: f, tier: ring.tier });
            }
        }
        if dist > 0 && dist <= bwd_reach {
            let f = predicted - dist;
            if valid_range.contains(&f) {
                set.insert(CacheKey { frame: f, tier: ring.tier });
            }
        }
    }
    let _ = fps; // reserved: a future revision biases lookahead by fps directly
    set
}

/// What changed between two desired sets, for a caller that holds actual
/// decoded bytes: cancel/evict what's gone, fetch what's new. Frames
/// present in both sets are left alone even if their priority order
/// shifted — no needless re-decode for a frame that's still wanted.
pub struct Diff {
    pub evict: Vec<CacheKey>,
    pub fetch: Vec<CacheKey>,
}

pub fn diff(have: &BTreeSet<CacheKey>, want: &BTreeSet<CacheKey>) -> Diff {
    Diff {
        evict: have.difference(want).copied().collect(),
        fetch: want.difference(have).copied().collect(),
    }
}

/// Rough byte cost of a desired set, for sizing against a [`Budget`] before
/// committing to fetch it (a caller can shrink `lookahead_secs` or the
/// shell table if this exceeds budget, rather than fetch-then-evict).
pub fn estimate_bytes(set: &BTreeSet<CacheKey>, full_w: u32, full_h: u32) -> u64 {
    let full_px = full_w as f64 * full_h as f64 * 4.0; // RGBA
    set.iter().map(|k| (full_px * k.tier.relative_area()).round() as u64).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(fps: f64) -> Velocity {
        Velocity { fps, last_pos: Some(0.0), last_t: Some(0.0) }
    }

    #[test]
    fn a_stationary_playhead_gets_a_symmetric_shell_that_reaches_both_ways() {
        let shells = Shells::for_fps(24.0);
        // Far enough from 0 that the backward shell (reach up to
        // max_reach() ~3840 frames at 24fps) isn't clipped by the source's
        // start — a clipped shell isn't asymmetric by policy, only by the
        // valid range, which `the_valid_range_clamps_a_shell...` tests
        // separately.
        let want = desired_set(5000, v(0.0), 24.0, 0.0, &shells, 0..1_000_000);
        let ahead = want.iter().filter(|k| k.frame > 5000).count();
        let behind = want.iter().filter(|k| k.frame < 5000).count();
        assert!(ahead > 0 && behind > 0, "ahead {ahead} behind {behind}");
        assert!((ahead as f64 / behind as f64 - 1.0).abs() < 0.15, "ahead {ahead} behind {behind}");
    }

    #[test]
    fn fast_forward_playback_biases_the_shell_forward_but_never_empties_the_back() {
        let shells = Shells::for_fps(24.0);
        let want = desired_set(1000, v(60.0), 24.0, 0.0, &shells, 0..100_000);
        let ahead = want.iter().filter(|k| k.frame > 1000).count();
        let behind = want.iter().filter(|k| k.frame < 1000).count();
        assert!(ahead > behind, "ahead {ahead} should exceed behind {behind} at 60 fps forward");
        assert!(behind > 0, "the back must never be empty — a reversal must find something");
    }

    #[test]
    fn prefetch_is_centred_on_the_predicted_position_not_the_current_one() {
        // At 48 fps (2x realtime for a 24fps source) with a half-second
        // lookahead, the shell should be centred ~24 frames ahead of the
        // current position — "fire ahead of time".
        let shells = Shells::for_fps(24.0);
        let want = desired_set(1000, v(48.0), 24.0, 0.5, &shells, 0..100_000);
        let center = {
            let mut fs: Vec<i64> = want.iter().map(|k| k.frame).collect();
            fs.sort();
            fs[fs.len() / 2]
        };
        assert!(center > 1010, "shell should be centred well ahead of 1000, got median {center}");
    }

    #[test]
    fn near_the_playhead_is_dense_and_full_resolution_farther_out_is_sparse_and_coarse() {
        let shells = Shells::for_fps(24.0);
        let want = desired_set(1000, v(0.0), 24.0, 0.0, &shells, 0..100_000);
        let at = |f: i64| want.iter().find(|k| k.frame == f).map(|k| k.tier);
        assert_eq!(at(1000), Some(SpatialTier::Full));
        assert_eq!(at(1005), Some(SpatialTier::Full), "within the 0.6s full ring at 24fps (~14 frames)");
        assert_eq!(at(1030), Some(SpatialTier::Proxy), "past the full ring, within the 3s proxy ring");
        assert_eq!(at(1496), Some(SpatialTier::Thumbnail), "well out (distance 496), in the coarse thumbnail shell");
    }

    #[test]
    fn temporal_stride_widens_before_switching_to_the_next_ring() {
        let shells = Shells::for_fps(24.0);
        let want: BTreeSet<i64> = desired_set(0, v(0.0), 24.0, 0.0, &shells, -1_000_000..1_000_000)
            .into_iter().filter(|k| k.frame > 0 && k.frame < 40).map(|k| k.frame).collect();
        // Full ring (stride 1) covers 0..~14; proxy ring (stride 2) then
        // covers even offsets from 14 to ~72. In 15..40 every kept frame
        // must be reachable at stride 1 (ring 0) or an even distance
        // (ring 1's stride 2), never an odd distance past the full ring.
        for &f in &want {
            let dist = f; // distance from playhead 0
            if dist >= 14 {
                assert_eq!(dist % 2, 0, "frame {f} at distance {dist} should fall on the stride-2 grid");
            }
        }
        assert!(want.len() < 39, "the far frames are sparser than 'every frame': got {}", want.len());
    }

    #[test]
    fn the_valid_range_clamps_a_shell_near_either_end_of_the_source() {
        let shells = Shells::for_fps(24.0);
        let want = desired_set(2, v(-10.0), 24.0, 0.0, &shells, 0..5);
        assert!(want.iter().all(|k| (0..5).contains(&k.frame)));
        assert!(!want.is_empty());
    }

    #[test]
    fn a_seek_resets_velocity_so_the_old_direction_does_not_linger() {
        let mut vel = Velocity::default();
        vel.observe(0.0, 0.0);
        vel.observe(5000.0, 1.0); // 5000 fps instantaneous: a click-to-seek, not a scroll
        assert_eq!(vel.fps(), 0.0, "an implausible jump must not read as velocity");
        let mut vel2 = Velocity::default();
        vel2.observe(0.0, 0.0);
        vel2.observe(24.0, 1.0); // a real 1x scrub
        assert!(vel2.fps() > 0.0);
        vel2.reset();
        assert_eq!(vel2.fps(), 0.0);
    }

    #[test]
    fn diff_only_reports_what_actually_changed() {
        let a: BTreeSet<CacheKey> = [
            CacheKey { frame: 1, tier: SpatialTier::Full },
            CacheKey { frame: 2, tier: SpatialTier::Full },
        ].into_iter().collect();
        let b: BTreeSet<CacheKey> = [
            CacheKey { frame: 2, tier: SpatialTier::Full },
            CacheKey { frame: 3, tier: SpatialTier::Proxy },
        ].into_iter().collect();
        let d = diff(&a, &b);
        assert_eq!(d.evict, vec![CacheKey { frame: 1, tier: SpatialTier::Full }]);
        assert_eq!(d.fetch, vec![CacheKey { frame: 3, tier: SpatialTier::Proxy }]);
    }

    #[test]
    fn a_phone_budget_is_smaller_and_estimate_bytes_respects_tier_area() {
        let phone = Budget::for_device(DeviceClass::Phone);
        let desktop = Budget::for_device(DeviceClass::Desktop);
        assert!(phone.bytes < desktop.bytes);
        let set: BTreeSet<CacheKey> = [
            CacheKey { frame: 0, tier: SpatialTier::Full },
            CacheKey { frame: 1, tier: SpatialTier::Thumbnail },
        ].into_iter().collect();
        let bytes = estimate_bytes(&set, 1920, 1080);
        let full_bytes = 1920.0 * 1080.0 * 4.0;
        assert!((bytes as f64 - full_bytes * (1.0 + 1.0 / 64.0)).abs() < full_bytes * 0.01);
    }

    #[test]
    fn a_phone_sized_shell_table_still_reaches_far_just_coarser() {
        // The falloff table itself doesn't know about Budget — sizing the
        // table to a byte budget is the caller's job (shrink the ring
        // boundaries / bias tiers coarser until estimate_bytes fits). This
        // test pins that a NAIVE full-table shell already costs far less
        // than a dense ring buffer of the same reach would, which is the
        // whole point of coarsening with distance.
        let shells = Shells::for_fps(24.0);
        let want = desired_set(1000, v(0.0), 24.0, 0.0, &shells, 0..100_000);
        let naive_dense_bytes = estimate_bytes(
            &(1000 - shells.max_reach()..1000 + shells.max_reach())
                .map(|f| CacheKey { frame: f, tier: SpatialTier::Full })
                .collect(),
            1920, 1080,
        );
        let actual_bytes = estimate_bytes(&want, 1920, 1080);
        assert!(actual_bytes < naive_dense_bytes / 10, "{actual_bytes} vs dense {naive_dense_bytes}");
    }
}
