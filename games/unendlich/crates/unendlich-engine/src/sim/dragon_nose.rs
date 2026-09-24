//! `sim::dragon_nose` — a dragon finds you (and its prey) by smell, and how
//! well it uses what it smells depends on its mind.
//!
//! ## The smell
//! Anything that moves lays **puffs** of scent ([`ScentTrail`]). A puff is
//! carried downwind and spreads as it ages ([`puff_at`]): so a trail is a
//! ribbon that drifts and fades, strongest where you were most recently, and
//! reaches a dragon only if the wind brings it there. A live animal standing
//! still is a continuous **plume** ([`plume_at`]): a cone downwind of it.
//!
//! [`sniff`] samples all of it at the dragon's nose and reports what a nose
//! can know: how strong, which way the smell is stronger, how old the freshest
//! part is, and — from the order of the puffs it can smell — which way the
//! trail *runs*.
//!
//! ## The mind ([`track`])
//! The same sniff means different things to different dragons:
//! * a **dull** dragon (cunning < 0.35) climbs the smell: it goes where the
//!   smell is strongest, which is where you *were*;
//! * a **sharp** one (0.35..0.7) reads the trail's direction and follows it
//!   the way you went, not back along it;
//! * a **cunning** one (≥ 0.7) dead-reckons: it takes the freshest point and
//!   your pace along the trail and heads for where you should be *now* — it
//!   cuts corners to intercept.
//! When the smell is lost, a dragon **casts**: it zig-zags across the wind
//! while working upwind (how real trackers reacquire a plume). Cunning sets how
//! long it keeps trying before it gives up.
//!
//! Nose acuity comes from the genome: a long muzzle holds more scent membrane
//! ([`nose_acuity`], this module's mapping — the genome doc has no smell gene).
//!
//! Pure: positions, times and wind in; no clock, RNG or world access.

use crate::sim::dragon_archetype::Genome;
use std::collections::VecDeque;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// One deposit of scent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Puff {
    pub pos: [f32; 3],
    /// When it was laid (caller's clock, s).
    pub t: f32,
    pub strength: f32,
}

/// A moving thing's scent trail: the last `cap` puffs, oldest first.
#[derive(Debug, Clone)]
pub struct ScentTrail {
    puffs: VecDeque<Puff>,
    cap: usize,
    /// Lay a new puff after moving this far (m)…
    pub spacing: f32,
    /// …or after standing this long (s).
    pub max_interval: f32,
}

impl ScentTrail {
    pub fn new(cap: usize) -> ScentTrail {
        ScentTrail { puffs: VecDeque::with_capacity(cap.max(2)), cap: cap.max(2), spacing: 2.0, max_interval: 2.0 }
    }

    /// Call every tick with where the emitter is. Lays a puff when it has
    /// moved `spacing` or `max_interval` has passed. Returns true if it laid.
    pub fn lay(&mut self, pos: [f32; 3], now: f32, strength: f32) -> bool {
        let due = match self.puffs.back() {
            None => true,
            Some(l) => dist(l.pos, pos) >= self.spacing || now - l.t >= self.max_interval,
        };
        if due && pos.iter().all(|v| v.is_finite()) && now.is_finite() {
            if self.puffs.len() == self.cap {
                self.puffs.pop_front();
            }
            self.puffs.push_back(Puff { pos, t: now, strength: strength.max(0.0) });
        }
        due
    }

    pub fn puffs(&self) -> impl Iterator<Item = &Puff> {
        self.puffs.iter()
    }

    pub fn len(&self) -> usize {
        self.puffs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.puffs.is_empty()
    }
}

#[inline]
fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[inline]
fn wrap(a: f32) -> f32 {
    if !a.is_finite() {
        return 0.0;
    }
    (a + PI).rem_euclid(TAU) - PI
}

/// How scent moves and fades. Defaults are for open air.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScentPhysics {
    /// Seconds for a puff's strength to halve.
    pub half_life: f32,
    /// Initial puff radius (m).
    pub spread0: f32,
    /// Diffusive growth: sigma += diffusion·√age.
    pub diffusion: f32,
    /// Turbulent growth with wind: sigma += gust·|wind|·age.
    pub gust: f32,
}

impl Default for ScentPhysics {
    fn default() -> Self {
        ScentPhysics { half_life: 60.0, spread0: 1.5, diffusion: 1.2, gust: 0.12 }
    }
}

/// Concentration of one puff at `at`, `now`, in `wind` (m/s, x/z).
pub fn puff_at(p: &Puff, at: [f32; 3], now: f32, wind: [f32; 2], ph: &ScentPhysics) -> f32 {
    let age = (now - p.t).max(0.0);
    let w = (wind[0] * wind[0] + wind[1] * wind[1]).sqrt();
    let c = [p.pos[0] + wind[0] * age, p.pos[1], p.pos[2] + wind[1] * age];
    let sigma = ph.spread0 + ph.diffusion * age.sqrt() + ph.gust * w * age;
    let r2 = (at[0] - c[0]).powi(2) + (at[2] - c[2]).powi(2) + ((at[1] - c[1]) * 1.5).powi(2);
    let fade = 0.5f32.powf(age / ph.half_life.max(1e-3));
    p.strength * fade * (ph.spread0 / sigma).powi(2) * (-r2 / (2.0 * sigma * sigma)).exp()
}

/// Steady plume of a live source at `src`: strongest close, a cone downwind,
/// barely anything upwind. `wind` in m/s.
pub fn plume_at(src: [f32; 3], strength: f32, at: [f32; 3], wind: [f32; 2]) -> f32 {
    let dx = at[0] - src[0];
    let dz = at[2] - src[2];
    let dy = (at[1] - src[1]) * 1.5;
    let w = (wind[0] * wind[0] + wind[1] * wind[1]).sqrt();
    let r = (dx * dx + dz * dz + dy * dy).sqrt();
    if w < 0.3 {
        // Still air: plain diffusion.
        return strength / (1.0 + r * r / 16.0);
    }
    let (ux, uz) = (wind[0] / w, wind[1] / w);
    let down = dx * ux + dz * uz; // distance downwind
    let cross = (-dx * uz + dz * ux).abs();
    if down < -2.0 {
        return strength * 0.01 / (1.0 + r * r / 4.0);
    }
    let x = down.max(0.5);
    // Cone widens with distance; stronger wind stretches and thins it.
    let sigma = 1.0 + 0.25 * x;
    strength * 4.0 / (w.max(1.0) * sigma * sigma) * (-(cross * cross + dy * dy) / (2.0 * sigma * sigma)).exp()
}

/// A long muzzle is more scent membrane; a heavy brow crowds the nasal
/// passage a little. 0..1. (This module's mapping.)
pub fn nose_acuity(g: &Genome) -> f32 {
    (0.35 + 0.5 * g.muzzle_len.clamp(0.0, 1.0) + 0.15 * (1.0 - g.brow.clamp(0.0, 1.0))).clamp(0.0, 1.0)
}

/// Faintest concentration a nose of `acuity` notices.
pub fn threshold(acuity: f32) -> f32 {
    0.02 * (1.0 - 0.85 * acuity.clamp(0.0, 1.0))
}

/// What one sniff tells a dragon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sniff {
    /// Total concentration at the nose.
    pub level: f32,
    /// `level / threshold` — above 1 it smells you.
    pub over: f32,
    /// World yaw toward where the smell comes from (concentration-weighted).
    pub toward: Option<f32>,
    /// Age (s) of the freshest puff it can smell.
    pub freshest_age: f32,
    /// Where that freshest puff was laid.
    pub freshest_pos: Option<[f32; 3]>,
    /// World yaw the trail runs (older → newer), if two or more puffs reach it.
    pub trail_heading: Option<f32>,
    /// Pace of the emitter along the trail (m/s), from the puffs it can smell.
    pub trail_speed: f32,
}

impl Sniff {
    pub fn smells(&self) -> bool {
        self.over >= 1.0
    }
}

/// Sniff a trail from `nose`, `now`, in `wind`.
pub fn sniff(trail: &ScentTrail, nose: [f32; 3], now: f32, wind: [f32; 2], acuity: f32, ph: &ScentPhysics) -> Sniff {
    let mut total = 0.0f32;
    let mut dir = [0.0f32; 2];
    let mut contrib: Vec<(Puff, f32)> = Vec::new();
    for p in trail.puffs() {
        let c = puff_at(p, nose, now, wind, ph);
        if !(c > 0.0) {
            continue;
        }
        total += c;
        // The smell of a puff arrives from its drifted centre.
        let age = (now - p.t).max(0.0);
        let cx = p.pos[0] + wind[0] * age - nose[0];
        let cz = p.pos[2] + wind[1] * age - nose[2];
        let l = (cx * cx + cz * cz).sqrt().max(1e-3);
        dir[0] += c * cx / l;
        dir[1] += c * cz / l;
        contrib.push((*p, c));
    }
    let th = threshold(acuity);
    let over = total / th;
    let toward = (dir[0].abs() + dir[1].abs() > 1e-9).then(|| dir[1].atan2(dir[0]));
    // Only puffs that are a real share of what it smells inform the trail.
    let cmax = contrib.iter().map(|x| x.1).fold(0.0, f32::max);
    contrib.retain(|x| x.1 >= cmax * 0.05 && x.1 >= th * 0.25);
    let freshest = contrib.iter().max_by(|a, b| a.0.t.partial_cmp(&b.0.t).unwrap_or(std::cmp::Ordering::Equal));
    let (freshest_age, freshest_pos) = match freshest {
        Some((p, _)) => ((now - p.t).max(0.0), Some(p.pos)),
        None => (f32::INFINITY, None),
    };
    let (mut trail_heading, mut trail_speed) = (None, 0.0);
    if contrib.len() >= 2 {
        // Weighted regression of position on time: the trail's velocity.
        let wsum: f32 = contrib.iter().map(|x| x.1).sum();
        let tm = contrib.iter().map(|x| x.0.t * x.1).sum::<f32>() / wsum;
        let xm = contrib.iter().map(|x| x.0.pos[0] * x.1).sum::<f32>() / wsum;
        let zm = contrib.iter().map(|x| x.0.pos[2] * x.1).sum::<f32>() / wsum;
        let (mut vx, mut vz, mut tt) = (0.0f32, 0.0f32, 0.0f32);
        for (p, c) in &contrib {
            let dt = p.t - tm;
            vx += c * dt * (p.pos[0] - xm);
            vz += c * dt * (p.pos[2] - zm);
            tt += c * dt * dt;
        }
        if tt > 1e-6 {
            let (vx, vz) = (vx / tt, vz / tt);
            let sp = (vx * vx + vz * vz).sqrt();
            if sp > 0.05 {
                trail_heading = Some(vz.atan2(vx));
                trail_speed = sp;
            }
        }
    }
    Sniff { level: total, over, toward, freshest_age, freshest_pos, trail_heading, trail_speed }
}

/// Sniff a live source's plume (prey standing or grazing).
pub fn sniff_plume(src: [f32; 3], strength: f32, nose: [f32; 3], wind: [f32; 2], acuity: f32) -> Sniff {
    let level = plume_at(src, strength, nose, wind);
    let toward = Some((src[2] - nose[2]).atan2(src[0] - nose[0]));
    Sniff {
        level,
        over: level / threshold(acuity),
        toward,
        freshest_age: 0.0,
        freshest_pos: Some(src),
        trail_heading: None,
        trail_speed: 0.0,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The mind
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackMode {
    /// Nothing to go on.
    Lost,
    /// Up the smell toward where it is strongest (where you were).
    Climb,
    /// Along the trail the way it runs.
    Follow,
    /// Straight for where you should be now.
    Intercept,
    /// Lost the smell: zig-zag across the wind, working upwind.
    Cast,
}

impl TrackMode {
    pub fn name(self) -> &'static str {
        match self {
            TrackMode::Lost => "lost",
            TrackMode::Climb => "climbing the scent",
            TrackMode::Follow => "following the trail",
            TrackMode::Intercept => "cutting you off",
            TrackMode::Cast => "casting for the scent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackState {
    pub mode: TrackMode,
    /// Seconds since it last smelled anything.
    pub lost_for: f32,
    /// Last heading that had scent behind it.
    pub last_heading: Option<f32>,
    cast_t: f32,
    cast_sign: f32,
}

impl TrackState {
    pub fn new(seed: u64) -> TrackState {
        TrackState {
            mode: TrackMode::Lost,
            lost_for: f32::INFINITY,
            last_heading: None,
            cast_t: 0.0,
            cast_sign: if seed & 2 == 0 { 1.0 } else { -1.0 },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackPlan {
    pub mode: TrackMode,
    /// World yaw to move along.
    pub heading: f32,
    pub speed01: f32,
    /// Where it believes the quarry is (Intercept) or was (Climb/Follow).
    pub goal: Option<[f32; 3]>,
    /// 0..1 how sure it is.
    pub confidence: f32,
}

/// How long a dragon keeps casting before it gives up (s).
pub fn cast_patience(cunning: f32) -> f32 {
    3.0 + 17.0 * cunning.clamp(0.0, 1.0)
}

/// Decide what to do with a sniff. `wind` sets the casting direction.
pub fn track(st: &mut TrackState, s: &Sniff, cunning: f32, self_pos: [f32; 3], wind: [f32; 2], now_dt: f32) -> TrackPlan {
    let dt = if now_dt.is_finite() { now_dt.clamp(0.0, 1.0) } else { 0.0 };
    let cunning = cunning.clamp(0.0, 1.0);
    if s.smells() {
        st.lost_for = 0.0;
        st.cast_t = 0.0;
        let confidence = (s.over.ln() / 4.0).clamp(0.1, 1.0);
        let toward = s.toward.unwrap_or(0.0);
        let (mode, heading, goal) = if cunning >= 0.7 && s.trail_heading.is_some() && s.freshest_pos.is_some() {
            // Dead reckoning: freshest point + pace × how long ago.
            let (th, fp) = (s.trail_heading.unwrap(), s.freshest_pos.unwrap());
            let lead = s.trail_speed * s.freshest_age.min(30.0);
            let g = [fp[0] + th.cos() * lead, fp[1], fp[2] + th.sin() * lead];
            let h = (g[2] - self_pos[2]).atan2(g[0] - self_pos[0]);
            (TrackMode::Intercept, h, Some(g))
        } else if cunning >= 0.35 && s.trail_heading.is_some() {
            // On the trail: go the way it runs, drawn toward the smell.
            let th = s.trail_heading.unwrap();
            let h = wrap(toward + wrap(th - toward) * 0.6);
            (TrackMode::Follow, h, s.freshest_pos)
        } else {
            (TrackMode::Climb, toward, s.freshest_pos)
        };
        st.mode = mode;
        st.last_heading = Some(heading);
        return TrackPlan { mode, heading: wrap(heading), speed01: 0.4 + 0.5 * confidence, goal, confidence };
    }
    st.lost_for = if st.lost_for.is_finite() { st.lost_for + dt } else { f32::INFINITY };
    if st.last_heading.is_some() && st.lost_for <= cast_patience(cunning) {
        // Cast: legs across the wind, each ~2 s + 3 s·cunning, drifting upwind.
        st.cast_t += dt;
        let leg = 2.0 + 3.0 * cunning;
        if st.cast_t >= leg {
            st.cast_t = 0.0;
            st.cast_sign = -st.cast_sign;
        }
        let w = (wind[0] * wind[0] + wind[1] * wind[1]).sqrt();
        let base = if w > 0.3 { wind[1].atan2(wind[0]) + PI } else { st.last_heading.unwrap() };
        let heading = wrap(base + st.cast_sign * (FRAC_PI_2 * 0.8));
        st.mode = TrackMode::Cast;
        let left = 1.0 - st.lost_for / cast_patience(cunning);
        return TrackPlan { mode: TrackMode::Cast, heading, speed01: 0.55, goal: None, confidence: 0.1 * left };
    }
    st.mode = TrackMode::Lost;
    TrackPlan { mode: TrackMode::Lost, heading: st.last_heading.unwrap_or(0.0), speed01: 0.0, goal: None, confidence: 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PH: ScentPhysics = ScentPhysics { half_life: 60.0, spread0: 1.5, diffusion: 1.2, gust: 0.12 };

    /// A walker going +x at `speed` for `secs`, laying a trail; returns it.
    fn walk(from: [f32; 3], heading: f32, speed: f32, secs: f32) -> (ScentTrail, [f32; 3]) {
        let mut t = ScentTrail::new(256);
        let mut p = from;
        let dt = 0.1;
        for i in 0..(secs / dt) as usize {
            p[0] += heading.cos() * speed * dt;
            p[2] += heading.sin() * speed * dt;
            t.lay(p, i as f32 * dt, 1.0);
        }
        (t, p)
    }

    #[test]
    fn trail_lays_by_distance_and_time_and_caps() {
        let mut t = ScentTrail::new(4);
        assert!(t.lay([0.0; 3], 0.0, 1.0));
        assert!(!t.lay([0.5, 0.0, 0.0], 0.1, 1.0));
        assert!(t.lay([2.5, 0.0, 0.0], 0.2, 1.0)); // moved 2.5 m
        assert!(t.lay([2.5, 0.0, 0.0], 2.3, 1.0)); // stood 2.1 s
        for i in 0..10 {
            t.lay([10.0 * i as f32, 0.0, 0.0], 3.0 + i as f32, 1.0);
        }
        assert_eq!(t.len(), 4);
    }

    #[test]
    fn puffs_fade_and_drift_downwind() {
        let p = Puff { pos: [0.0; 3], t: 0.0, strength: 1.0 };
        let wind = [3.0, 0.0];
        // After 10 s the puff is ~30 m downwind: strong there, weak at the origin.
        let down = puff_at(&p, [30.0, 0.0, 0.0], 10.0, wind, &PH);
        let origin = puff_at(&p, [0.0, 0.0, 0.0], 10.0, wind, &PH);
        assert!(down > origin * 20.0, "down {down} origin {origin}");
        // And it fades with age.
        let later = puff_at(&p, [90.0, 0.0, 0.0], 30.0, wind, &PH);
        assert!(later < down);
    }

    #[test]
    fn old_scent_is_weaker_even_where_it_has_not_spread() {
        let ph = ScentPhysics { half_life: 30.0, spread0: 1.5, diffusion: 0.0, gust: 0.0 };
        let p = Puff { pos: [0.0; 3], t: 0.0, strength: 1.0 };
        let fresh = puff_at(&p, [0.0; 3], 0.0, [0.0, 0.0], &ph);
        let old = puff_at(&p, [0.0; 3], 60.0, [0.0, 0.0], &ph);
        assert!((old / fresh - 0.25).abs() < 1e-3, "two half-lives: {}", old / fresh);
    }

    #[test]
    fn downwind_smells_you_upwind_does_not() {
        let (trail, end) = walk([0.0; 3], 0.0, 1.5, 20.0);
        let wind = [0.0, 2.5]; // blowing toward +z
        let acu = 0.8;
        let down = sniff(&trail, [end[0], 0.0, end[2] + 40.0], 20.0, wind, acu, &PH);
        let up = sniff(&trail, [end[0], 0.0, end[2] - 40.0], 20.0, wind, acu, &PH);
        assert!(down.smells(), "downwind over {:.2}", down.over);
        assert!(!up.smells(), "upwind over {:.2}", up.over);
    }

    #[test]
    fn a_sharper_nose_smells_from_farther() {
        let (trail, end) = walk([0.0; 3], 0.0, 1.5, 20.0);
        let wind = [0.0, 2.0];
        let reach = |acu: f32| {
            (1..40)
                .map(|k| k as f32 * 5.0)
                .take_while(|d| sniff(&trail, [end[0], 0.0, end[2] + d], 20.0, wind, acu, &PH).smells())
                .last()
                .unwrap_or(0.0)
        };
        assert!(reach(0.95) > reach(0.2) + 10.0, "sharp {} dull {}", reach(0.95), reach(0.2));
    }

    #[test]
    fn nose_acuity_follows_the_muzzle() {
        let mut g = Genome::from_seed(3);
        g.muzzle_len = 1.0;
        let long = nose_acuity(&g);
        g.muzzle_len = 0.0;
        assert!(long > nose_acuity(&g) + 0.4);
    }

    #[test]
    fn the_trail_says_which_way_you_went() {
        let heading = 0.7f32;
        let (trail, end) = walk([0.0; 3], heading, 1.5, 20.0);
        // Nose beside the middle of the trail, still air.
        let mid = [end[0] * 0.5 + 3.0, 0.0, end[2] * 0.5 - 3.0];
        let s = sniff(&trail, mid, 20.0, [0.0, 0.0], 0.9, &PH);
        let th = s.trail_heading.expect("trail direction");
        assert!(wrap(th - heading).abs() < 0.15, "read {th:.2} vs {heading:.2}");
        assert!((s.trail_speed - 1.5).abs() < 0.3, "pace {:.2}", s.trail_speed);
    }

    /// The mind claim, geometrically: from the middle of a trail, a dull
    /// dragon heads for the smell; a sharp one goes the way you went; a
    /// cunning one aims ahead of the freshest point.
    #[test]
    fn the_mind_reads_the_same_sniff_differently() {
        let (trail, end) = walk([0.0; 3], 0.0, 1.5, 26.0);
        // The walker is still going +x (at x≈39 at t=26). The nose, beside the
        // trail back at x=12, smells the stretch around it, ~18 s old.
        let nose = [12.0, 0.0, 6.0];
        let s = sniff(&trail, nose, 26.0, [0.0, 0.0], 0.9, &PH);
        assert!(s.smells());
        let plan = |c: f32| track(&mut TrackState::new(0), &s, c, nose, [0.0, 0.0], 0.1);
        let dull = plan(0.1);
        let sharp = plan(0.5);
        let cunning = plan(0.9);
        assert_eq!(dull.mode, TrackMode::Climb);
        assert_eq!(sharp.mode, TrackMode::Follow);
        assert_eq!(cunning.mode, TrackMode::Intercept);
        // Sharp and cunning both point more along +x (the way you went) than dull.
        assert!(sharp.heading.cos() > dull.heading.cos());
        let g = cunning.goal.unwrap();
        let fp = s.freshest_pos.unwrap();
        assert!(g[0] > fp[0] + 3.0, "intercept {:.1} vs freshest {:.1}", g[0], fp[0]);
        assert!((g[0] - end[0]).abs() < (fp[0] - end[0]).abs(), "the guess is closer to where you are");
    }

    #[test]
    fn lost_scent_casts_across_the_wind_then_gives_up() {
        let smelled = Sniff { level: 1.0, over: 50.0, toward: Some(0.0), freshest_age: 1.0, freshest_pos: Some([5.0, 0.0, 0.0]), trail_heading: None, trail_speed: 0.0 };
        let nothing = Sniff { level: 0.0, over: 0.0, toward: None, freshest_age: f32::INFINITY, freshest_pos: None, trail_heading: None, trail_speed: 0.0 };
        let wind = [3.0, 0.0]; // upwind is -x (yaw π)
        let run = |cunning: f32| {
            let mut st = TrackState::new(0);
            track(&mut st, &smelled, cunning, [0.0; 3], wind, 0.1);
            let mut headings = vec![];
            let mut cast_secs = 0.0;
            for _ in 0..400 {
                let p = track(&mut st, &nothing, cunning, [0.0; 3], wind, 0.1);
                if p.mode == TrackMode::Cast {
                    cast_secs += 0.1;
                    headings.push(p.heading);
                }
            }
            (cast_secs, headings, st.mode)
        };
        let (dull_t, _, dull_end) = run(0.1);
        let (sharp_t, hs, _) = run(0.9);
        assert!(sharp_t > dull_t * 2.0, "cast {sharp_t:.1}s vs {dull_t:.1}s");
        assert_eq!(dull_end, TrackMode::Lost);
        // Zig-zags: both sides of upwind, and never downwind.
        assert!(hs.iter().any(|h| wrap(h - PI) > 0.5) && hs.iter().any(|h| wrap(h - PI) < -0.5));
        assert!(hs.iter().all(|h| h.cos() < 0.3), "casting drifts upwind (-x)");
    }

    #[test]
    fn a_plume_leads_downwind_noses_to_its_source() {
        let src = [0.0, 0.0, 0.0];
        let wind = [2.0, 0.0];
        let acu = 0.7;
        assert!(sniff_plume(src, 1.0, [30.0, 0.0, 0.0], wind, acu).smells());
        assert!(!sniff_plume(src, 1.0, [-30.0, 0.0, 0.0], wind, acu).smells());
        assert!(!sniff_plume(src, 1.0, [30.0, 0.0, 25.0], wind, acu).smells(), "outside the cone");
    }

    /// Closed loop: a cunning dragon starting 60 m downwind of a walker it
    /// cannot see finds them by smell; one starting upwind does not.
    #[test]
    fn a_nose_finds_a_walker_from_downwind() {
        let find = |start: [f32; 3], cunning: f32| {
            let wind = [0.0, 2.0];
            let mut trail = ScentTrail::new(256);
            let mut st = TrackState::new(1);
            let (mut px, mut d) = ([0.0f32, 0.0, 0.0], start);
            let dt = 0.1;
            for i in 0..(90.0 / dt) as usize {
                let now = i as f32 * dt;
                px[0] += 1.4 * dt; // walking +x
                trail.lay(px, now, 1.0);
                let s = sniff(&trail, d, now, wind, 0.8, &PH);
                let p = track(&mut st, &s, cunning, d, wind, dt);
                d[0] += p.heading.cos() * 5.0 * p.speed01 * dt;
                d[2] += p.heading.sin() * 5.0 * p.speed01 * dt;
                if ((d[0] - px[0]).powi(2) + (d[2] - px[2]).powi(2)).sqrt() < 8.0 {
                    return Some(now);
                }
            }
            None
        };
        assert!(find([0.0, 0.0, 60.0], 0.8).is_some(), "downwind: should find");
        assert!(find([0.0, 0.0, -60.0], 0.8).is_none(), "upwind: nothing to smell");
    }

    #[test]
    fn nothing_non_finite_leaks() {
        let (trail, _) = walk([0.0; 3], 0.0, 1.0, 5.0);
        let s = sniff(&trail, [f32::NAN, 0.0, 0.0], 5.0, [f32::INFINITY, 0.0], 0.5, &PH);
        let p = track(&mut TrackState::new(0), &s, 0.5, [0.0; 3], [0.0, 0.0], f32::NAN);
        assert!(p.heading.is_finite());
        let mut t = ScentTrail::new(4);
        assert!(!t.lay([f32::NAN, 0.0, 0.0], 0.0, 1.0) || t.is_empty());
    }
}
