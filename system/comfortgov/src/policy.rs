//! The comfort governor's policy, pure. Inputs are plain numbers sampled by
//! `sysfs.rs`; outputs are per-cluster frequency windows and memory knobs.
//!
//! The model from the design notes, "sleep vs work":
//!
//! ```text
//!   SLEEP        nothing worth doing: floor at the OPP minimum, ceiling at
//!                the knee, so wakeups are cheap and idle is deep
//!   INTERACTIVE  recent input: floor at the knee (the highest frequency at
//!                the minimum voltage: below it nothing is saved and jitter
//!                appears), ceiling at max so a frame never waits
//!   BURST        heavy load that started recently: max, race to idle;
//!                if the queue drains it is over quickly
//!   SUSTAINED    heavy load for longer than BURST_MAX_S with no drain:
//!                cap at the efficiency ceiling (the top OPPs cost far more
//!                energy and heat per unit of work than the ones below)
//! ```
//!
//! On top of that, two ceilings that scale every state's maximum:
//! * **skin temperature** (the hand, not the die): 1.0 up to SKIN_WARM,
//!   falling to SKIN_HOT_FACTOR at SKIN_HOT, with hysteresis and a slow
//!   slew, because the case lags the chip by a minute or two;
//! * **battery**: below LOW_BATTERY, the efficiency cap applies everywhere.
//!
//! Memory policy: swappiness by state, MGLRU working-set protection while
//! interacting, and proactive reclaim of background cgroups only when idle
//! and quiet, so compression happens at the knee frequency instead of as
//! reclaim stalls under a finger.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Sleep,
    Interactive,
    Burst,
    Sustained,
}

/// One cpufreq policy (a cluster), as discovered.
#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    pub name: String,
    /// Ascending, kHz.
    pub freqs: Vec<u32>,
    /// Configured floor, kHz (the knee); None = heuristic.
    pub floor_khz: Option<u32>,
}

impl Cluster {
    pub fn min(&self) -> u32 {
        self.freqs.first().copied().unwrap_or(0)
    }
    pub fn max(&self) -> u32 {
        self.freqs.last().copied().unwrap_or(0)
    }
    /// The knee: configured, else the lowest OPP at or above KNEE_FRAC of max.
    pub fn knee(&self) -> u32 {
        if let Some(f) = self.floor_khz {
            return self.nearest(f);
        }
        let want = (self.max() as f32 * KNEE_FRAC) as u32;
        self.freqs.iter().copied().find(|f| *f >= want).unwrap_or(self.max())
    }
    /// Snap a target to an available OPP (never above `cap`).
    pub fn nearest(&self, khz: u32) -> u32 {
        self.freqs.iter().copied().min_by_key(|f| f.abs_diff(khz)).unwrap_or(khz)
    }
    /// The highest OPP at or below a fraction of max.
    pub fn at_most_frac(&self, frac: f32) -> u32 {
        let cap = (self.max() as f32 * frac) as u32;
        self.freqs.iter().copied().filter(|f| *f <= cap).last().unwrap_or(self.min())
    }
}

pub const KNEE_FRAC: f32 = 0.30;
/// Sustained loads and low battery run under this fraction of max.
pub const EFFICIENCY_FRAC: f32 = 0.70;
/// Input within this counts as interacting.
pub const INTERACT_HOLD_MS: u64 = 2000;
/// Utilization that counts as heavy.
pub const HEAVY_UTIL: f32 = 0.75;
/// A burst older than this with no drain is sustained.
pub const BURST_MAX_S: f32 = 8.0;
/// Below this utilization a burst has drained.
pub const DRAIN_UTIL: f32 = 0.25;
pub const SKIN_WARM_C: f32 = 38.0;
pub const SKIN_HOT_C: f32 = 43.0;
pub const SKIN_HOT_FACTOR: f32 = 0.5;
pub const SKIN_HYST_C: f32 = 1.0;
/// The ceiling factor moves at most this much per second (the case is slow).
pub const SKIN_SLEW_PER_S: f32 = 0.05;
pub const LOW_BATTERY_PCT: u32 = 20;
/// Proactive reclaim per tick, bytes, when idle and quiet.
pub const RECLAIM_STEP: u64 = 64 << 20;
/// PSI memory "some" (fraction of time) above which reclaim is not proactive.
pub const RECLAIM_PSI_MAX: f32 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub now_ms: u64,
    /// Time of the last input (touch, key), ms; None = never.
    pub last_input_ms: Option<u64>,
    /// Whole-system utilization 0..1 since the previous sample.
    pub util: f32,
    /// Skin/case temperature, °C; None = no sensor.
    pub skin_c: Option<f32>,
    pub on_battery: bool,
    pub battery_pct: u32,
    /// PSI memory "some" avg10 as a fraction 0..1.
    pub psi_mem_some: f32,
    /// Fraction of RAM available.
    pub mem_avail_frac: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub min_khz: u32,
    pub max_khz: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub state: State,
    pub windows: Vec<Window>,
    /// Thermal + battery ceiling factor applied to the maxima.
    pub ceiling: f32,
    pub swappiness: u32,
    /// MGLRU min_ttl_ms: working-set protection while interacting.
    pub min_ttl_ms: u32,
    /// Bytes of background reclaim to request this tick (0 = none).
    pub reclaim_bytes: u64,
    /// One-line reason, for the log.
    pub why: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    pub clusters: Vec<Cluster>,
    state: State,
    burst_since: Option<u64>,
    skin_factor: f32,
    /// Hysteresis: once hot-limited, stay so until SKIN_HYST below warm.
    skin_limiting: bool,
    last_ms: Option<u64>,
}

impl Policy {
    pub fn new(clusters: Vec<Cluster>) -> Policy {
        Policy { clusters, state: State::Sleep, burst_since: None, skin_factor: 1.0, skin_limiting: false, last_ms: None }
    }

    pub fn skin_factor(&self) -> f32 {
        self.skin_factor
    }

    fn update_skin(&mut self, skin: Option<f32>, dt_s: f32) {
        let Some(t) = skin else {
            self.skin_factor = 1.0;
            return;
        };
        // Enter limiting above WARM; leave only below WARM - HYST.
        if t > SKIN_WARM_C {
            self.skin_limiting = true;
        } else if t < SKIN_WARM_C - SKIN_HYST_C {
            self.skin_limiting = false;
        }
        // While limiting, the ramp runs from the RELEASE point (warm minus
        // the hysteresis), so the ceiling stays a little below 1.0 until the
        // case is back under it. (A ramp starting at the warm threshold gave
        // a factor of 1.0 there, so the hysteresis did nothing: found by the
        // test.) Entering at 38 C steps to ~0.92, slewed at 5%/s.
        let target = if !self.skin_limiting {
            1.0
        } else {
            let lo = SKIN_WARM_C - SKIN_HYST_C;
            let x = ((t - lo) / (SKIN_HOT_C - lo)).clamp(0.0, 1.0);
            1.0 - x * (1.0 - SKIN_HOT_FACTOR)
        };
        let step = SKIN_SLEW_PER_S * dt_s;
        self.skin_factor += (target - self.skin_factor).clamp(-step, step);
    }

    fn next_state(&mut self, s: &Sample) -> State {
        let interacting = s.last_input_ms.is_some_and(|t| s.now_ms.saturating_sub(t) <= INTERACT_HOLD_MS);
        let heavy = s.util >= HEAVY_UTIL;
        let drained = s.util <= DRAIN_UTIL;
        let st = match self.state {
            State::Burst | State::Sustained => {
                if drained {
                    self.burst_since = None;
                    if interacting { State::Interactive } else { State::Sleep }
                } else {
                    let age = self.burst_since.map(|t| (s.now_ms.saturating_sub(t)) as f32 / 1000.0).unwrap_or(0.0);
                    if age >= BURST_MAX_S { State::Sustained } else { State::Burst }
                }
            }
            _ => {
                if heavy {
                    self.burst_since = Some(s.now_ms);
                    State::Burst
                } else if interacting {
                    State::Interactive
                } else {
                    State::Sleep
                }
            }
        };
        self.state = st;
        st
    }

    pub fn decide(&mut self, s: &Sample) -> Decision {
        let dt_s = self.last_ms.map(|l| s.now_ms.saturating_sub(l) as f32 / 1000.0).unwrap_or(0.0).min(10.0);
        self.last_ms = Some(s.now_ms);
        self.update_skin(s.skin_c, dt_s);
        let state = self.next_state(s);
        let low_battery = s.on_battery && s.battery_pct <= LOW_BATTERY_PCT;
        let ceiling = self.skin_factor * if low_battery { EFFICIENCY_FRAC } else { 1.0 };
        let windows = self
            .clusters
            .iter()
            .map(|c| {
                let knee = c.knee();
                let (min, max_frac) = match state {
                    State::Sleep => (c.min(), KNEE_FRAC),
                    State::Interactive => (knee, 1.0),
                    State::Burst => (knee, 1.0),
                    State::Sustained => (knee, EFFICIENCY_FRAC),
                };
                let max = c.at_most_frac(max_frac * ceiling).max(knee.min(c.at_most_frac(ceiling)));
                let min = min.min(max);
                Window { min_khz: min, max_khz: max }
            })
            .collect();
        let interacting = state == State::Interactive || state == State::Burst && s.last_input_ms.is_some_and(|t| s.now_ms.saturating_sub(t) <= INTERACT_HOLD_MS);
        let (swappiness, min_ttl_ms) = match state {
            State::Interactive | State::Burst => (40, 1000),
            State::Sustained => (60, 200),
            State::Sleep => (100, 0),
        };
        // Reclaim only when nothing is waiting on anything: asleep, quiet
        // PSI, and either on the charger or with little RAM left.
        let reclaim_bytes = if state == State::Sleep && s.psi_mem_some <= RECLAIM_PSI_MAX && (!s.on_battery || s.mem_avail_frac < 0.15) {
            RECLAIM_STEP
        } else {
            0
        };
        let why = format!(
            "{state:?} util={:.2} skin={} x{:.2}{}{}",
            s.util,
            s.skin_c.map(|t| format!("{t:.1}C")).unwrap_or_else(|| "n/a".into()),
            ceiling,
            if low_battery { " low-battery" } else { "" },
            if interacting { " input" } else { "" }
        );
        Decision { state, windows, ceiling, swappiness, min_ttl_ms, reclaim_bytes, why }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big() -> Cluster {
        Cluster { name: "policy4".into(), freqs: vec![691_200, 940_800, 1_228_800, 1_516_800, 1_804_800, 2_054_400, 2_400_000], floor_khz: None }
    }
    fn little() -> Cluster {
        Cluster { name: "policy0".into(), freqs: vec![300_000, 576_000, 768_000, 1_017_600, 1_248_000, 1_516_800, 1_804_800], floor_khz: None }
    }
    fn sample(now: u64) -> Sample {
        Sample { now_ms: now, last_input_ms: None, util: 0.05, skin_c: Some(30.0), on_battery: true, battery_pct: 80, psi_mem_some: 0.0, mem_avail_frac: 0.4 }
    }

    #[test]
    fn knee_is_the_lowest_opp_at_or_above_the_fraction_and_configurable() {
        let c = little();
        assert_eq!(c.knee(), 576_000, "30% of 1.8 GHz = 541 MHz -> next OPP up");
        let mut c2 = c.clone();
        c2.floor_khz = Some(800_000);
        assert_eq!(c2.knee(), 768_000, "configured floor snaps to the nearest OPP");
        assert_eq!(big().at_most_frac(0.7), 1_516_800);
    }

    #[test]
    fn sleep_keeps_the_floor_at_minimum_and_input_raises_it_to_the_knee() {
        let mut p = Policy::new(vec![little(), big()]);
        let d = p.decide(&sample(0));
        assert_eq!(d.state, State::Sleep);
        assert_eq!(d.windows[0], Window { min_khz: 300_000, max_khz: 576_000 });
        let mut s = sample(1000);
        s.last_input_ms = Some(900);
        let d = p.decide(&s);
        assert_eq!(d.state, State::Interactive);
        assert_eq!(d.windows[0], Window { min_khz: 576_000, max_khz: 1_804_800 }, "knee floor, full ceiling");
        assert_eq!(d.windows[1].min_khz, 940_800);
        // Input gone for a while: back to sleep.
        let d = p.decide(&sample(900 + INTERACT_HOLD_MS + 1));
        assert_eq!(d.state, State::Sleep);
    }

    #[test]
    fn a_burst_races_to_idle_and_a_load_that_does_not_drain_becomes_sustained() {
        let mut p = Policy::new(vec![big()]);
        let mut s = sample(0);
        s.util = 0.9;
        let d = p.decide(&s);
        assert_eq!((d.state, d.windows[0].max_khz), (State::Burst, 2_400_000));
        // Drains after 2 s: sleep.
        let mut s = sample(2000);
        s.util = 0.1;
        assert_eq!(p.decide(&s).state, State::Sleep);
        // Starts again and never drains: sustained after BURST_MAX_S, capped.
        let mut t = 3000;
        let mut s = sample(t);
        s.util = 0.95;
        assert_eq!(p.decide(&s).state, State::Burst);
        for _ in 0..40 {
            t += 250;
            s.now_ms = t;
            p.decide(&s);
        }
        let d = p.decide(&s);
        assert_eq!(d.state, State::Sustained);
        assert_eq!(d.windows[0], Window { min_khz: 940_800, max_khz: 1_516_800 }, "efficiency cap, knee floor");
    }

    #[test]
    fn skin_temperature_lowers_the_ceiling_slowly_with_hysteresis() {
        let mut p = Policy::new(vec![big()]);
        let mut s = sample(0);
        s.last_input_ms = Some(0);
        p.decide(&s);
        // Jump to hot: the factor slews, it does not jump.
        s.skin_c = Some(43.0);
        s.now_ms = 1000;
        s.last_input_ms = Some(1000);
        let d = p.decide(&s);
        assert!(d.ceiling > 0.9, "after 1 s only a little: {}", d.ceiling);
        for i in 2..=20 {
            s.now_ms = i * 1000;
            s.last_input_ms = Some(s.now_ms);
            p.decide(&s);
        }
        let d = p.decide(&s);
        assert!((d.ceiling - SKIN_HOT_FACTOR).abs() < 0.02, "settled at the hot factor: {}", d.ceiling);
        assert!(d.windows[0].max_khz <= 1_228_800, "ceiling applied: {}", d.windows[0].max_khz);
        assert_eq!(d.windows[0].min_khz, 940_800, "the knee floor survives (min never above max)");
        // Cooling to 37.5: still limiting (hysteresis: needs < 37).
        s.skin_c = Some(37.5);
        for i in 21..=60 {
            s.now_ms = i * 1000;
            s.last_input_ms = Some(s.now_ms);
            p.decide(&s);
        }
        assert!(p.skin_factor() < 1.0 - 1e-3, "still limiting at 37.5");
        s.skin_c = Some(36.5);
        for i in 61..=100 {
            s.now_ms = i * 1000;
            s.last_input_ms = Some(s.now_ms);
            p.decide(&s);
        }
        assert!((p.skin_factor() - 1.0).abs() < 1e-3, "released below the hysteresis band");
    }

    #[test]
    fn no_skin_sensor_means_no_thermal_limit_and_low_battery_caps() {
        let mut p = Policy::new(vec![big()]);
        let mut s = sample(0);
        s.skin_c = None;
        s.last_input_ms = Some(0);
        assert_eq!(p.decide(&s).ceiling, 1.0);
        s.battery_pct = 15;
        let d = p.decide(&s);
        assert_eq!(d.ceiling, EFFICIENCY_FRAC);
        assert_eq!(d.windows[0].max_khz, 1_516_800);
        s.on_battery = false;
        assert_eq!(p.decide(&s).ceiling, 1.0, "on the charger the battery cap is off");
    }

    #[test]
    fn memory_policy_protects_the_working_set_while_interacting_and_reclaims_only_when_idle_and_quiet() {
        let mut p = Policy::new(vec![big()]);
        let mut s = sample(0);
        s.last_input_ms = Some(0);
        let d = p.decide(&s);
        assert_eq!((d.swappiness, d.min_ttl_ms, d.reclaim_bytes), (40, 1000, 0));
        // Asleep on battery with plenty of RAM: no reclaim work.
        let d = p.decide(&sample(INTERACT_HOLD_MS + 10));
        assert_eq!((d.state, d.reclaim_bytes), (State::Sleep, 0));
        // Asleep on the charger, quiet: reclaim a step.
        let mut s = sample(INTERACT_HOLD_MS + 20);
        s.on_battery = false;
        assert_eq!(p.decide(&s).reclaim_bytes, RECLAIM_STEP);
        // ...but not under memory pressure.
        s.psi_mem_some = 0.05;
        assert_eq!(p.decide(&s).reclaim_bytes, 0);
        // Asleep on battery but RAM is short: reclaim anyway.
        let mut s = sample(INTERACT_HOLD_MS + 30);
        s.mem_avail_frac = 0.1;
        assert_eq!(p.decide(&s).reclaim_bytes, RECLAIM_STEP);
    }
}
