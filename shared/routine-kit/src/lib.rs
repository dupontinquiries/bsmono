//! routine-kit — learn the week's rhythm so the step counter can borrow a prior from it.
//!
//! The idea: a person's context is a sequence of composite states — (motion class, wifi
//! association) — and the *transitions* between them are strongly conditioned on time of week.
//! "Home-AP drops at 08:30 on a weekday, walking follows" is a first-order Markov edge that
//! recurs; nothing here needs to know it is a commute. So the model is small and honest:
//! one transition-count matrix **per time-of-week bucket**, learned online, with exponential
//! forgetting so a changed routine fades out instead of haunting the model.
//!
//! Layers (each a seam, per the house rule):
//!
//! * [`TimeGrid`] — minute-of-week → bucket. Pure arithmetic; the caller owns the clock, so
//!   tests run a synthetic fortnight in microseconds and nothing here touches `SystemTime`.
//! * [`RoutineModel`] — the per-bucket matrices: `observe`, `decay`, `predict` (Laplace-
//!   smoothed row distribution). Alphabet is `u8` codes; it does not know what they mean.
//! * [`RoutineLearner`] — the **timings loop** made pure: the daemon calls [`RoutineLearner::tick`]
//!   on its interval (and on every wifi edge) with the current minute, motion code, and wifi
//!   association; the learner detects state changes and bucket rollovers and feeds the model.
//!   The `loop { sleep(interval); tick(...) }` itself lives in the daemon shell, not here.
//! * [`parse_nmcli_monitor`] — the postmarketOS wifi source, same pattern as sensor-kit's
//!   `monitor-sensor` parser: pipe `nmcli monitor` in, get [`WifiEvent`]s out. NetworkManager's
//!   monitor lines are not a stable API; the parser is deliberately loose and the exact strings
//!   on the fleet's NM version need an on-device check.
//!
//! What the prediction is *for*: `predict(bucket, current)` ranks likely next composite states.
//! If (walking, off-wifi) dominates the 08:30 weekday row, the step daemon can (a) pre-arm the
//! accelerometer poll instead of idling, and (b) treat a `Contended`/gap window during that slot
//! as probable-walking for gap-filling — a prior, never a substitute for counting.

/// Minutes in a week; the whole model is periodic over this.
pub const WEEK_MIN: u32 = 7 * 24 * 60;

/// Minute-of-week → bucket index. `bucket_minutes` of 30–60 is the useful range: finer buckets
/// see too few visits per week to learn from, coarser ones blur the morning-leave edge away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeGrid {
    pub bucket_minutes: u32,
}

impl TimeGrid {
    pub fn new(bucket_minutes: u32) -> TimeGrid {
        TimeGrid { bucket_minutes: bucket_minutes.clamp(1, WEEK_MIN) }
    }
    pub fn n_buckets(&self) -> usize {
        WEEK_MIN.div_ceil(self.bucket_minutes) as usize
    }
    pub fn bucket_of(&self, minute_of_week: u32) -> usize {
        ((minute_of_week % WEEK_MIN) / self.bucket_minutes) as usize
    }
}

/// Wifi association, coarsened to what routine structure actually needs. SSIDs are hashed to a
/// small stable class rather than stored: the model wants "the usual network vs another vs none",
/// not a name (and the learner's persisted counts then contain no readable SSID).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiAssoc {
    Off,
    /// Associated; `class` is a stable small hash of the SSID (see [`ssid_class`]).
    On { class: u8 },
}

/// Number of SSID classes. Small on purpose: a person's routine spans a handful of networks and
/// collisions between two rarely-visited cafés cost nothing.
pub const SSID_CLASSES: u8 = 4;

/// FNV-1a folded into `SSID_CLASSES`. Stable across runs (no RandomState), so persisted counts
/// stay meaningful.
pub fn ssid_class(ssid: &str) -> u8 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in ssid.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h % SSID_CLASSES as u64) as u8
}

/// A wifi edge from the platform watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WifiEvent {
    Connected { ssid: Option<String> },
    Disconnected,
}

/// Parse one `nmcli monitor` line. Loose by design — matched against the phrasings NM has used
/// across versions — and returns `None` for the many lines that aren't wifi association edges.
/// The exact strings on the fleet's NetworkManager build are an on-device check.
pub fn parse_nmcli_monitor(line: &str) -> Option<WifiEvent> {
    let l = line.trim();
    // "wlan0: connected to MyNetwork" / "wlp3s0: connected"
    if let Some(i) = l.find(": connected") {
        let rest = l[i + ": connected".len()..].trim();
        let ssid = rest.strip_prefix("to ").map(|s| s.trim_matches('\'').to_string());
        return Some(WifiEvent::Connected { ssid });
    }
    // "wlan0: disconnected" / "wlan0: deactivated"
    if l.contains(": disconnected") || l.contains(": deactivated") {
        return Some(WifiEvent::Disconnected);
    }
    None
}

/// A composite context state: a motion class (e.g. `carry::CarryState::code()`, or any small
/// alphabet the caller defines) × wifi association. Packs to a dense `usize` for the matrices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    pub motion: u8,
    pub wifi: WifiAssoc,
}

impl Context {
    fn wifi_index(w: WifiAssoc) -> usize {
        match w {
            WifiAssoc::Off => 0,
            WifiAssoc::On { class } => 1 + (class % SSID_CLASSES) as usize,
        }
    }
    /// Dense index in `0..n_motion * (1 + SSID_CLASSES)`.
    pub fn index(&self, n_motion: u8) -> usize {
        (self.motion % n_motion.max(1)) as usize * (1 + SSID_CLASSES as usize)
            + Self::wifi_index(self.wifi)
    }
    pub fn n_states(n_motion: u8) -> usize {
        n_motion.max(1) as usize * (1 + SSID_CLASSES as usize)
    }
}

/// Per-time-bucket first-order Markov transition counts with exponential forgetting.
#[derive(Debug, Clone)]
pub struct RoutineModel {
    grid: TimeGrid,
    n: usize,
    /// `counts[bucket][from * n + to]`, f64 so decay is a multiply.
    counts: Vec<Vec<f64>>,
}

impl RoutineModel {
    pub fn new(grid: TimeGrid, n_states: usize) -> RoutineModel {
        let n = n_states.max(1);
        RoutineModel { grid, n, counts: vec![vec![0.0; n * n]; grid.n_buckets()] }
    }

    pub fn grid(&self) -> TimeGrid {
        self.grid
    }

    /// Record one observed transition in the bucket the *departure* minute falls in.
    pub fn observe(&mut self, minute_of_week: u32, from: usize, to: usize) {
        if from >= self.n || to >= self.n {
            return; // out-of-alphabet observations are dropped, not panicked on
        }
        let b = self.grid.bucket_of(minute_of_week);
        self.counts[b][from * self.n + to] += 1.0;
    }

    /// Exponential forgetting: multiply every count by `factor` (0 < factor ≤ 1). The daemon
    /// calls this once per day; 0.97/day halves a habit's weight in ~3 weeks, so a moved office
    /// stops dominating within a month while one skipped Tuesday changes almost nothing.
    pub fn decay(&mut self, factor: f64) {
        let f = factor.clamp(0.0, 1.0);
        for b in &mut self.counts {
            for c in b.iter_mut() {
                *c *= f;
            }
        }
    }

    /// Laplace-smoothed distribution over next states from `from` in this minute's bucket.
    /// The smoothing mass is **one pseudo-count for the whole row** (per-cell 1/n), not per
    /// cell: routine evidence is sparse — a bucket sees at most a couple of real transitions
    /// per week — and a per-cell α would drown two weeks of genuine walk-outs in 30 states'
    /// worth of pseudo-counts. With no evidence the row is exactly uniform — the model never
    /// pretends to know.
    pub fn predict(&self, minute_of_week: u32, from: usize) -> Vec<f64> {
        let b = self.grid.bucket_of(minute_of_week);
        let alpha = 1.0 / self.n as f64;
        let row = &self.counts[b][from * self.n..(from + 1) * self.n];
        let total: f64 = row.iter().sum::<f64>() + 1.0;
        row.iter().map(|c| (c + alpha) / total).collect()
    }

    /// Total evidence (decayed visit count) behind a row — callers gate on this so a
    /// two-observation row doesn't drive behaviour.
    pub fn evidence(&self, minute_of_week: u32, from: usize) -> f64 {
        let b = self.grid.bucket_of(minute_of_week);
        self.counts[b][from * self.n..(from + 1) * self.n].iter().sum()
    }
}

/// The timings-loop core: turns periodic ticks into model observations.
///
/// The daemon's loop is `every interval (and on every wifi/carry edge): learner.tick(now, motion,
/// wifi)`. The learner records a transition when the composite state *changes*, and additionally
/// records self-transitions on bucket rollover — so "still on the home network for three straight
/// buckets" is evidence too, and prediction from a quiet evening correctly favours staying put.
#[derive(Debug, Clone)]
pub struct RoutineLearner {
    model: RoutineModel,
    n_motion: u8,
    last: Option<(u32, usize)>, // (minute_of_week, state index)
}

impl RoutineLearner {
    pub fn new(grid: TimeGrid, n_motion: u8) -> RoutineLearner {
        let n = Context::n_states(n_motion);
        RoutineLearner { model: RoutineModel::new(grid, n), n_motion, last: None }
    }

    pub fn model(&self) -> &RoutineModel {
        &self.model
    }

    /// Feed the current observation. `minute_of_week` must be monotone modulo the week between
    /// calls (the daemon owns the clock).
    pub fn tick(&mut self, minute_of_week: u32, motion: u8, wifi: WifiAssoc) {
        let idx = Context { motion, wifi }.index(self.n_motion);
        match self.last {
            None => {}
            Some((prev_min, prev_idx)) => {
                let changed = prev_idx != idx;
                let rolled = self.model.grid.bucket_of(prev_min)
                    != self.model.grid.bucket_of(minute_of_week);
                if changed || rolled {
                    // Departure-bucket attribution: the 08:29 → 08:31 walk-out belongs to the
                    // bucket you left from.
                    self.model.observe(prev_min, prev_idx, idx);
                }
            }
        }
        self.last = Some((minute_of_week, idx));
    }

    /// Once per day from the daemon.
    pub fn daily_decay(&mut self, factor: f64) {
        self.model.decay(factor);
    }

    /// Probability that the next state's motion class is `motion` (marginalised over wifi),
    /// given now and the current context. This is the step daemon's prior.
    pub fn expect_motion(&self, minute_of_week: u32, current: Context, motion: u8) -> f64 {
        let dist = self.model.predict(minute_of_week, current.index(self.n_motion));
        let per_wifi = 1 + SSID_CLASSES as usize;
        let m = (motion % self.n_motion.max(1)) as usize;
        dist[m * per_wifi..(m + 1) * per_wifi].iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STILL: u8 = 0;
    const WALK: u8 = 3; // e.g. carry::CarryState::PocketWaist.code()
    const N_MOTION: u8 = 6;

    fn home() -> WifiAssoc {
        WifiAssoc::On { class: ssid_class("HomeNet") }
    }

    /// Simulate `days` weekdays: still-at-home all morning, wifi drops at 08:30 and walking
    /// follows for 20 min, one learner tick per minute.
    fn train_commute(l: &mut RoutineLearner, days: u32) {
        for d in 0..days {
            let day0 = (d % 7) * 24 * 60;
            for m in 0..(11 * 60) {
                let t = day0 + m;
                let (motion, wifi) = if m < 8 * 60 + 30 {
                    (STILL, home())
                } else if m < 8 * 60 + 50 {
                    (WALK, WifiAssoc::Off)
                } else {
                    (STILL, WifiAssoc::Off)
                };
                l.tick(t, motion, wifi);
            }
            l.daily_decay(0.98);
        }
    }

    #[test]
    fn grid_is_periodic_and_covers_the_week() {
        let g = TimeGrid::new(30);
        assert_eq!(g.n_buckets(), 336);
        assert_eq!(g.bucket_of(0), 0);
        assert_eq!(g.bucket_of(WEEK_MIN), 0); // wraps
        assert_eq!(g.bucket_of(8 * 60 + 29), g.bucket_of(8 * 60 + 1));
        assert_ne!(g.bucket_of(8 * 60 + 31), g.bucket_of(8 * 60 + 29));
    }

    #[test]
    fn learns_the_morning_walk_out() {
        let mut l = RoutineLearner::new(TimeGrid::new(30), N_MOTION);
        train_commute(&mut l, 14);
        // 08:29 Monday, still, on home wifi: what comes next?
        let now = 8 * 60 + 29;
        let cur = Context { motion: STILL, wifi: home() };
        let p_walk = l.expect_motion(now, cur, WALK);
        let p_still = l.expect_motion(now, cur, STILL);
        assert!(
            p_walk > p_still && p_walk > 0.4,
            "walk-out should dominate the 08:30 bucket: walk={p_walk:.3} still={p_still:.3}"
        );
        // …and the same question at 20:00 does NOT predict walking.
        let evening = 20 * 60;
        let p_walk_evening = l.expect_motion(evening, cur, WALK);
        assert!(
            p_walk_evening < p_walk / 2.0,
            "time conditioning: evening walk prior {p_walk_evening:.3} vs morning {p_walk:.3}"
        );
    }

    #[test]
    fn unvisited_buckets_predict_uniform_not_confident() {
        let l = RoutineLearner::new(TimeGrid::new(60), N_MOTION);
        let cur = Context { motion: STILL, wifi: WifiAssoc::Off };
        let dist = l.model().predict(3 * 60, cur.index(N_MOTION));
        let n = dist.len() as f64;
        for p in &dist {
            assert!((p - 1.0 / n).abs() < 1e-9, "no evidence → uniform");
        }
        assert_eq!(l.model().evidence(3 * 60, cur.index(N_MOTION)), 0.0);
    }

    #[test]
    fn decay_forgets_a_dropped_habit() {
        let mut l = RoutineLearner::new(TimeGrid::new(30), N_MOTION);
        train_commute(&mut l, 7);
        let now = 8 * 60 + 29;
        let cur = Context { motion: STILL, wifi: home() };
        let before = l.expect_motion(now, cur, WALK);
        // 60 days of the new routine: never leaves, stays on wifi all morning.
        for d in 0..60u32 {
            let day0 = (d % 7) * 24 * 60;
            for m in 0..(11 * 60) {
                l.tick(day0 + m, STILL, home());
            }
            l.daily_decay(0.98);
        }
        let after = l.expect_motion(now, cur, WALK);
        assert!(
            after < before / 3.0,
            "old commute fades: {before:.3} -> {after:.3}"
        );
    }

    #[test]
    fn nmcli_lines_parse_loosely() {
        assert_eq!(
            parse_nmcli_monitor("wlan0: connected to 'HomeNet'"),
            Some(WifiEvent::Connected { ssid: Some("HomeNet".into()) })
        );
        assert_eq!(
            parse_nmcli_monitor("wlp3s0: connected"),
            Some(WifiEvent::Connected { ssid: None })
        );
        assert_eq!(parse_nmcli_monitor("wlan0: disconnected"), Some(WifiEvent::Disconnected));
        assert_eq!(parse_nmcli_monitor("wlan0: deactivated"), Some(WifiEvent::Disconnected));
        assert_eq!(parse_nmcli_monitor("ip4: address 192.168.1.7/24"), None);
        assert_eq!(parse_nmcli_monitor("Connectivity is now 'full'"), None);
    }

    #[test]
    fn ssid_classes_are_stable_and_small() {
        let a = ssid_class("HomeNet");
        assert_eq!(a, ssid_class("HomeNet"));
        assert!(a < SSID_CLASSES);
    }

    #[test]
    fn self_transitions_on_rollover_teach_staying_put() {
        let mut l = RoutineLearner::new(TimeGrid::new(30), N_MOTION);
        // three quiet evening hours on home wifi, one tick a minute
        for m in (20 * 60)..(23 * 60) {
            l.tick(m, STILL, home());
        }
        let cur = Context { motion: STILL, wifi: home() };
        let p_stay = l.expect_motion(21 * 60, cur, STILL);
        assert!(p_stay > 0.5, "quiet evenings predict staying still: {p_stay:.3}");
    }
}
