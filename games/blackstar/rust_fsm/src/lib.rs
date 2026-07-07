//! rust_fsm — C-ABI bridge that runs the real early-fps `fsm.rs` gating logic
//! and hands the C engine a chunk-spawn decision.
//!
//! The gameplay-affecting part of `fsm.rs` is the *gating*: given a `Site`,
//! which feature (if any) grows, and how big/tough it is. That logic is
//! reproduced faithfully here behind `extern "C"` so the C engine's `mapfsm`
//! module can defer to Rust as the source of truth, while the heavy mesh
//! "dressing" stays on whichever side owns rendering.
//!
//! The structs are laid out `#[repr(C)]` to match mapfsm.h exactly, so a
//! pointer from C can be read directly with no marshalling.

use std::os::raw::c_int;

/// Mirrors `ex_site_t` in mapfsm.h (field order + types must match).
#[repr(C)]
pub struct Site {
    pub x: f32,
    pub z: f32,
    pub ground_y: f32,
    pub sightline: f32,
    pub danger_yaw: f32,
    pub high_traffic: c_int,
    pub is_high_point: c_int,
    pub slope: f32,
    pub wet: c_int,
    pub indoor: c_int,
    pub seed: u32,
}

/// Mirrors `ex_feature_t` in mapfsm.h.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Feature {
    None = 0,
    SightlineBreaker = 1,
    Vantage = 2,
    Sentinel = 3,
    LogisticsSpine = 4,
    Wreckage = 5,
    Shelter = 6,
}

/// Mirrors `ex_fsm_result_t` in mapfsm.h.
#[repr(C)]
pub struct FsmResult {
    pub feature: Feature,
    pub spawn_chunk: c_int,
    pub destructible: c_int,
    pub half_extents: [f32; 3],
    pub hp: f32,
    pub hit_mask: u32,
}

// hit-mask bits, matching ex_chunk_hit_mask in coll_chunk.h
const HIT_HEAVY: u32 = 1 << 2;
const HIT_TITAN: u32 = 1 << 3;
const HIT_VEHICLE_RAM: u32 = 1 << 4;
const HIT_ANY: u32 = 0xFF;

impl FsmResult {
    fn none() -> Self {
        FsmResult { feature: Feature::None, spawn_chunk: 0, destructible: 0,
                    half_extents: [0.0; 3], hp: 0.0, hit_mask: 0 }
    }
}

// ---- gating rules, faithful to fsm.rs ----

const SIGHTLINE_BREAKER_MIN: f32 = 18.0;
const SIGHTLINE_BREAKER_LONG: f32 = 34.0;
const WRECKAGE_MIN_SIGHTLINE: f32 = 24.0;
const SENTINEL_MIN_SIGHTLINE: f32 = 22.0;
const SENTINEL_MIN_SLOPE: f32 = 0.12;

fn grow_logistics_spine(s: &Site) -> FsmResult {
    if s.indoor != 0 || s.is_high_point == 0 { return FsmResult::none(); }
    FsmResult { feature: Feature::LogisticsSpine, spawn_chunk: 1, destructible: 0,
                half_extents: [1.4, 7.0, 1.4], hp: 0.0, hit_mask: 0 }
}

fn grow_vantage(s: &Site) -> FsmResult {
    if s.is_high_point == 0 { return FsmResult::none(); }
    FsmResult { feature: Feature::Vantage, spawn_chunk: 1, destructible: 0,
                half_extents: [1.6, 2.4, 1.6], hp: 0.0, hit_mask: 0 }
}

fn grow_sentinel(s: &Site) -> FsmResult {
    if s.slope < SENTINEL_MIN_SLOPE || s.sightline < SENTINEL_MIN_SIGHTLINE {
        return FsmResult::none();
    }
    FsmResult { feature: Feature::Sentinel, spawn_chunk: 1, destructible: 1,
                half_extents: [1.7, 0.7, 1.5], hp: 200.0, hit_mask: HIT_HEAVY | HIT_TITAN }
}

fn grow_wreckage(s: &Site) -> FsmResult {
    if s.sightline < WRECKAGE_MIN_SIGHTLINE { return FsmResult::none(); }
    FsmResult { feature: Feature::Wreckage, spawn_chunk: 1, destructible: 1,
                half_extents: [3.0, 0.9, 3.0], hp: 120.0,
                hit_mask: HIT_HEAVY | HIT_TITAN | HIT_VEHICLE_RAM }
}

fn grow_sightline_breaker(s: &Site) -> FsmResult {
    if s.sightline < SIGHTLINE_BREAKER_MIN { return FsmResult::none(); }
    let long_lane = s.sightline > SIGHTLINE_BREAKER_LONG;
    let bump = if s.high_traffic != 0 { 0.25 } else { 0.0 };
    let sy = if long_lane { 1.5 } else { 0.6 };
    FsmResult {
        feature: Feature::SightlineBreaker, spawn_chunk: 1, destructible: 1,
        half_extents: [1.0 + bump, sy, 0.7 + bump],
        hp: if long_lane { 140.0 } else { 80.0 },
        hit_mask: HIT_HEAVY | HIT_TITAN | HIT_VEHICLE_RAM,
    }
}

fn grow_shelter(s: &Site) -> FsmResult {
    if s.slope > 0.5 || s.indoor != 0 { return FsmResult::none(); }
    FsmResult { feature: Feature::Shelter, spawn_chunk: 1, destructible: 1,
                half_extents: [1.5, 0.9, 1.5], hp: 40.0, hit_mask: HIT_ANY }
}

/// The C entry point. Runs every feature FSM against the site in priority
/// order; the first that grows claims the spot. `site` must be a valid,
/// non-null pointer to a `Site`/`ex_site_t`.
///
/// # Safety
/// `site` must point to a valid `Site` for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn ex_rust_fsm_grow(site: *const Site) -> FsmResult {
    if site.is_null() {
        return FsmResult::none();
    }
    let s = &*site;

    let r = grow_logistics_spine(s);   if r.feature != Feature::None { return r; }
    let r = grow_vantage(s);           if r.feature != Feature::None { return r; }
    let r = grow_sentinel(s);          if r.feature != Feature::None { return r; }
    let r = grow_wreckage(s);          if r.feature != Feature::None { return r; }
    let r = grow_sightline_breaker(s); if r.feature != Feature::None { return r; }
    let r = grow_shelter(s);           if r.feature != Feature::None { return r; }

    FsmResult::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(sightline: f32, slope: f32, high: bool, indoor: bool) -> Site {
        Site { x: 0.0, z: 0.0, ground_y: 0.0, sightline, danger_yaw: 0.7,
               high_traffic: (sightline > 30.0) as c_int, is_high_point: high as c_int,
               slope, wet: 0, indoor: indoor as c_int, seed: 7 }
    }

    #[test]
    fn breaker_gates_on_sightline() {
        unsafe {
            // steep, low-sightline, not-high ground -> every feature declines
            let s = site(6.0, 0.9, false, false);
            assert_eq!(ex_rust_fsm_grow(&s).feature, Feature::None);
            // flat open low-sightline ground -> shelter fills it opportunistically
            let s = site(6.0, 0.0, false, false);
            assert_eq!(ex_rust_fsm_grow(&s).feature, Feature::Shelter);
            // medium lane (>=18, below the 24 wreckage threshold) -> breaker claims it
            let s = site(20.0, 0.0, false, false);
            assert_eq!(ex_rust_fsm_grow(&s).feature, Feature::SightlineBreaker);
            // long open lane (>=24) -> wreckage wins by priority (breaks the lane harder)
            let s = site(40.0, 0.0, false, false);
            assert_eq!(ex_rust_fsm_grow(&s).feature, Feature::Wreckage);
        }
    }
    #[test]
    fn high_point_prefers_spine_then_vantage() {
        unsafe {
            // outdoor high point -> spine wins (checked first)
            let s = site(20.0, 0.0, true, false);
            assert_eq!(ex_rust_fsm_grow(&s).feature, Feature::LogisticsSpine);
            // indoor high point -> spine declines, vantage claims it
            let s = site(20.0, 0.0, true, true);
            assert_eq!(ex_rust_fsm_grow(&s).feature, Feature::Vantage);
        }
    }
    #[test]
    fn sentinel_wants_slope() {
        unsafe {
            let s = site(30.0, 0.0, false, false);
            assert_ne!(ex_rust_fsm_grow(&s).feature, Feature::Sentinel);
            let s = site(30.0, 0.3, false, false);
            assert_eq!(ex_rust_fsm_grow(&s).feature, Feature::Sentinel);
        }
    }
}
