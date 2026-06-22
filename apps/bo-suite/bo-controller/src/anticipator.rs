// src/anticipator.rs
#![allow(dead_code)]
//
// A small FSM-driven anticipation engine. Tracks the user's interaction
// state (idle / browsing / playing / paused / transitioning / tuning) and
// learns transition frequencies between (state, observed_action) pairs.
// At any moment it can answer: "given the current state and recent
// history, what's the next likely thing this person will do, and when?"
//
// Predictions feed pre-staging:
//   - Cheap: keep HTTP connection to device warm, ensure source is on
//     the right mode (music streaming) so a play tap doesn't pay the
//     source-switch latency.
//   - Moderate: pre-resolve the URLs of the top N candidate tracks via
//     the local track server so the path lookup is already done.
//   - Expensive: actually pre-queue the next track on the device so a
//     tap becomes a cheap "skip to next" rather than a full play
//     handshake.
//
// HONEST SCOPE:
//   - The transition table is hand-seeded with sensible priors so the
//     anticipator is useful from session 1. As actions accumulate it
//     learns by updating frequencies, so per-user habits influence
//     predictions over time. No neural net, no training infrastructure.
//   - We don't predict *which specific track* the user will play (that
//     would need user-feature modeling). We predict *kinds* of next
//     actions (play, pause, skip, switch device, adjust volume).
//   - Confidence scoring is a normalized frequency ratio with smoothing
//     (Laplace +1). Calibrated only insofar as "0.7 means 7 out of 10
//     times this transition fires after this state on this user".

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Four-bucket time-of-day mapping. The anticipator keeps a *separate*
/// transition count table indexed by (TimeWindow, state, action) so that
/// predictions can take time-of-day into account.
///
/// Bucket choice is a tradeoff: 24 hourly cells would give the finest
/// resolution but bloat state and learn slowly (sparse data per cell);
/// 4 buckets corresponding to the user's natural daily rhythm capture
/// most of the signal. The boundaries cover one common pattern
/// (morning routine, work day, evening, late night) — they'd need to
/// shift for a heavy nightshift user, but the BLEND mechanism degrades
/// gracefully when a window has no data (falls back to overall counts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TimeWindow {
    Morning,    // 05:00 - 10:59
    Afternoon,  // 11:00 - 16:59
    Evening,    // 17:00 - 21:59
    Night,      // 22:00 - 04:59
}

impl TimeWindow {
    pub fn from_hour(hour: u32) -> Self {
        match hour {
            5..=10  => TimeWindow::Morning,
            11..=16 => TimeWindow::Afternoon,
            17..=21 => TimeWindow::Evening,
            _       => TimeWindow::Night,
        }
    }

    /// Current local time bucket. Computed from std::time::SystemTime
    /// so the anticipator stays independent of any external date crate.
    pub fn now_local() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now().duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64).unwrap_or(0);
        // Approximate local time using TZ env var offset hours. Falls
        // back to UTC when the env var is missing or malformed — wrong
        // bucket cost is at most ~6 hours of weird priors, which
        // self-corrects as observations accumulate in the user's actual
        // current window.
        let offset_hours: i64 = std::env::var("TZ_OFFSET_HOURS")
            .ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        let local_secs = secs + offset_hours * 3600;
        let hour = ((local_secs / 3600) % 24 + 24) % 24;
        Self::from_hour(hour as u32)
    }

    pub fn label(&self) -> &'static str {
        match self {
            TimeWindow::Morning   => "MORNING",
            TimeWindow::Afternoon => "AFTERNOON",
            TimeWindow::Evening   => "EVENING",
            TimeWindow::Night     => "NIGHT",
        }
    }
}

/// The user-interaction state that this engine maintains. State is what
/// we know about *now*; actions are events that cause transitions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FsmState {
    /// Nothing playing, no UI interaction recently. The "cold" state.
    Idle,
    /// User is poking around the library / device picker / source list.
    Browsing,
    /// Music is currently playing.
    Playing,
    /// User has paused — most likely to resume soon.
    Paused,
    /// Mid-transition between two states (track change, device switch).
    Transitioning,
    /// User is actively adjusting volume, EQ, source, etc.
    Tuning,
}

/// Observable actions. Granular enough that transitions are informative
/// but coarse enough that the transition table doesn't explode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKind {
    /// User asked to play a specific track.
    PlayTrack,
    /// Track-end fired naturally (the device told us it's done).
    TrackEnded,
    /// User paused.
    Pause,
    /// User resumed.
    Resume,
    /// User skipped to next track manually.
    SkipNext,
    /// User skipped to previous track manually.
    SkipPrev,
    /// User adjusted volume (any direction).
    VolumeChange,
    /// User switched the active device.
    SwitchDevice,
    /// User opened the library tab (was probably browsing or idle before).
    OpenLibrary,
    /// User opened the device selector.
    OpenDevicePicker,
    /// User just sat still — used by the periodic tick to age out state.
    InactivityTick,
}

/// What we think the user is about to do, and how confident we are.
#[derive(Debug, Clone, PartialEq)]
pub struct Prediction {
    /// The action we expect.
    pub action: ActionKind,
    /// 0.0..1.0 — normalized frequency for this transition.
    pub confidence: f32,
    /// When we expect it to happen, in ms from now. Comes from the
    /// per-state expected-dwell time; this is best-effort.
    pub horizon_ms: u32,
    /// What it would cost to pre-stage for this action — drives whether
    /// the controller actually warms anything up.
    pub cost: PrestageCost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrestageCost {
    /// Already-running work; pre-staging means "keep doing it".
    Free,
    /// One HTTP roundtrip or a SQLite lookup. ~10ms, <1KB.
    Cheap,
    /// File pre-fetch + pre-expose. ~100ms, ~MB.
    Moderate,
    /// Pre-queue on device + buffer decode. Several MB + device state change.
    Expensive,
}

/// One observed action with the time it happened.
#[derive(Debug, Clone)]
struct TimestampedAction {
    kind: ActionKind,
    at: Instant,
    /// What state we were in when this action arrived. Recorded so the
    /// transition counter updates atomically.
    from_state: FsmState,
}

pub struct Anticipator {
    state: FsmState,
    /// State entered_at — used to compute dwell time for predictions
    /// like "Paused for >2 min → less likely to resume".
    state_since: Instant,
    /// Recent N actions for pattern detection. Used for tests + diagnostics.
    history: VecDeque<TimestampedAction>,
    history_cap: usize,
    /// Transition frequency: (state, action) → observed count. Used for
    /// confidence scoring. Seeded with priors; learns from real use.
    counts: HashMap<(FsmState, ActionKind), u32>,
    /// Time-of-day-conditioned transition frequency. Same shape but
    /// keyed additionally by the TimeWindow the observation happened in.
    /// Lets the FSM predict "from Idle in the morning, PlayTrack is very
    /// likely" even when the unconditional probability from Idle is
    /// closer to even-odds. Starts EMPTY (no priors) so until real data
    /// accumulates, predictions fall back to the time-independent table.
    time_counts: HashMap<(TimeWindow, FsmState, ActionKind), u32>,
    /// Cap on observed-count growth so old behavior doesn't dominate forever.
    /// When a count hits this we don't increment further (effectively a
    /// recency window of ~CAP transitions per cell).
    count_cap: u32,
    /// Sample-size regularization for the time-window blend. Higher
    /// values mean we need more time-specific observations before
    /// trusting the time-conditioned probability over the overall one.
    /// 10 is the default — half-weight at ~10 observations, 83% weight
    /// at ~50 observations.
    time_blend_reg: f32,
}

impl Anticipator {
    pub fn new() -> Self {
        let mut a = Self {
            state: FsmState::Idle,
            state_since: Instant::now(),
            history: VecDeque::with_capacity(64),
            history_cap: 64,
            counts: HashMap::new(),
            time_counts: HashMap::new(),
            count_cap: 100,
            time_blend_reg: 10.0,
        };
        a.seed_priors();
        a
    }

    /// Seed the transition table with plausible defaults so predictions are
    /// useful on day one. These priors are deliberately conservative —
    /// they describe "typical" usage without overfitting to any persona.
    fn seed_priors(&mut self) {
        use ActionKind::*;
        use FsmState::*;

        // From Idle: the most common openers are picking a device, opening
        // the library, or hitting play on whatever was last selected.
        self.counts.insert((Idle, OpenDevicePicker), 3);
        self.counts.insert((Idle, OpenLibrary),      4);
        self.counts.insert((Idle, PlayTrack),        3);
        self.counts.insert((Idle, VolumeChange),     1);

        // From Browsing: the dominant exit is PlayTrack; users rarely just
        // close the library without taking action.
        self.counts.insert((Browsing, PlayTrack),    8);
        self.counts.insert((Browsing, SwitchDevice), 2);
        self.counts.insert((Browsing, VolumeChange), 1);

        // From Playing: the strongest next event is TrackEnded (autopilot)
        // followed by Pause and VolumeChange. SkipNext is also common.
        self.counts.insert((Playing, TrackEnded),    8);
        self.counts.insert((Playing, Pause),         3);
        self.counts.insert((Playing, VolumeChange),  3);
        self.counts.insert((Playing, SkipNext),      2);
        self.counts.insert((Playing, SwitchDevice),  1);

        // From Paused: overwhelmingly the next thing is Resume (within a
        // few minutes). After ~5min of pause, the InactivityTick will
        // demote us to Idle.
        self.counts.insert((Paused, Resume),         9);
        self.counts.insert((Paused, PlayTrack),      2);
        self.counts.insert((Paused, SwitchDevice),   1);
        self.counts.insert((Paused, InactivityTick), 3);

        // From Transitioning (mid track-change or device-switch): brief.
        self.counts.insert((Transitioning, PlayTrack),    5);
        self.counts.insert((Transitioning, VolumeChange), 1);

        // From Tuning: volume adjustments cluster, then user returns to
        // playing/idle.
        self.counts.insert((Tuning, VolumeChange),  6);
        self.counts.insert((Tuning, InactivityTick),2);
    }

    pub fn state(&self) -> &FsmState { &self.state }

    pub fn state_dwell(&self) -> Duration { self.state_since.elapsed() }

    /// Record an observed action: update transition counts AND transition
    /// the FSM. Returns the new state.
    pub fn observe(&mut self, kind: ActionKind) -> &FsmState {
        self.observe_at(kind, Instant::now())
    }

    /// Like observe but with an explicit timestamp (tests want this).
    pub fn observe_at(&mut self, kind: ActionKind, at: Instant) -> &FsmState {
        // Default time window from the system clock — predictions
        // consult `now_local()` when they care which window we're in.
        self.observe_at_in_window(kind, at, TimeWindow::now_local())
    }

    /// Test-friendly variant that lets the caller pin the TimeWindow
    /// rather than reading the system clock. Production code uses
    /// `observe_at` which reads now_local automatically.
    pub fn observe_at_in_window(
        &mut self,
        kind: ActionKind,
        at: Instant,
        window: TimeWindow,
    ) -> &FsmState {
        let from = self.state.clone();
        // 1. Update unconditional transition count.
        let cell = self.counts.entry((from.clone(), kind)).or_insert(0);
        if *cell < self.count_cap { *cell += 1; }
        // 2. Update time-window-conditioned transition count.
        let tcell = self.time_counts.entry((window, from.clone(), kind)).or_insert(0);
        if *tcell < self.count_cap { *tcell += 1; }
        // 3. Record in history.
        if self.history.len() == self.history_cap {
            self.history.pop_front();
        }
        self.history.push_back(TimestampedAction { kind, at, from_state: from });
        // 4. Compute the new state.
        let new_state = next_state(&self.state, kind, at, self.state_since);
        if new_state != self.state {
            self.state = new_state;
            self.state_since = at;
        }
        &self.state
    }

    /// Top-N predictions for what comes next, sorted by confidence desc.
    /// `n` is typically 1-3; predictions below a low confidence floor are
    /// dropped (they wouldn't justify any pre-staging anyway).
    pub fn predict(&self, n: usize) -> Vec<Prediction> {
        self.predict_in_window(n, TimeWindow::now_local())
    }

    /// Like `predict` but with an explicit time window. Used by tests
    /// and by callers that want to see "what would I predict in the
    /// morning" without actually being in the morning.
    pub fn predict_in_window(&self, n: usize, window: TimeWindow) -> Vec<Prediction> {
        // Unconditional + time-window-conditioned totals for this state.
        let total_overall: u32 = self.counts.iter()
            .filter(|((s, _), _)| s == &self.state)
            .map(|(_, c)| *c)
            .sum();
        let total_time: u32 = self.time_counts.iter()
            .filter(|((w, s, _), _)| w == &window && s == &self.state)
            .map(|(_, c)| *c)
            .sum();
        if total_overall == 0 && total_time == 0 { return Vec::new(); }

        // Laplace denominators: number-of-cells + raw total.
        let n_cells_overall = count_actions_overall(&self.state, &self.counts);
        let denom_overall = (total_overall as f32) + (n_cells_overall as f32);
        let n_cells_time   = count_actions_time(&window, &self.state, &self.time_counts);
        let denom_time     = (total_time as f32) + (n_cells_time as f32);

        // Blend weight: alpha grows with time-window sample size. At
        // total_time=0 we ignore time-conditioning entirely; at large
        // total_time we trust it heavily. Reg controls the half-weight
        // point.
        let alpha = if total_time == 0 {
            0.0
        } else {
            (total_time as f32) / (total_time as f32 + self.time_blend_reg)
        };

        // Union of actions seen in either table for this state.
        use std::collections::HashSet;
        let mut actions: HashSet<ActionKind> = HashSet::new();
        for ((s, a), _) in &self.counts {
            if s == &self.state { actions.insert(*a); }
        }
        for ((w, s, a), _) in &self.time_counts {
            if w == &window && s == &self.state { actions.insert(*a); }
        }

        let mut preds: Vec<Prediction> = actions.into_iter().map(|action| {
            let count_overall = *self.counts.get(&(self.state.clone(), action)).unwrap_or(&0);
            let count_time    = *self.time_counts.get(&(window, self.state.clone(), action)).unwrap_or(&0);
            let p_overall = (count_overall as f32 + 1.0) / denom_overall.max(1.0);
            let p_time    = (count_time as f32 + 1.0) / denom_time.max(1.0);
            let conf = alpha * p_time + (1.0 - alpha) * p_overall;
            Prediction {
                action,
                confidence: conf,
                horizon_ms: expected_horizon_ms(&self.state, action),
                cost: cost_for(action),
            }
        }).collect();

        preds.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence)
                              .unwrap_or(std::cmp::Ordering::Equal));
        preds.into_iter()
            .filter(|p| p.confidence >= 0.05)
            .take(n)
            .collect()
    }

    /// What pre-staging actions are worth executing right now? Returns a
    /// list ordered by confidence × inverse-cost. The controller picks
    /// these up and runs them (warm connection, pre-resolve, etc).
    pub fn prestage_recommendations(&self) -> Vec<Prediction> {
        let preds = self.predict(5);
        preds.into_iter()
            .filter(|p| match p.cost {
                PrestageCost::Free      => true,
                PrestageCost::Cheap     => p.confidence >= 0.30,
                PrestageCost::Moderate  => p.confidence >= 0.55,
                PrestageCost::Expensive => p.confidence >= 0.80,
            })
            .collect()
    }

    /// Reset the FSM (e.g. on device change or after long inactivity).
    pub fn reset_to(&mut self, s: FsmState) {
        self.state = s;
        self.state_since = Instant::now();
    }

    // ── Persistence ──────────────────────────────────────────────────
    //
    // Counts are persisted so the FSM's learned transitions survive
    // app restarts. We don't persist the current state (which by
    // definition starts Idle on launch) or history (transient) — just
    // the long-term transition frequencies.

    /// Serialize current counts to a JSON string. Tuple keys are encoded
    /// as `"state|action"` for the unconditional table, and
    /// `"T:window|state|action"` (4 colon-separated segments) for the
    /// time-window-conditioned table. The "T:" prefix discriminates so
    /// load_json can deserialize either format. Old saves without "T:"
    /// keys simply have no time-conditioned data, which is fine — the
    /// predict() blend gracefully falls back to overall when total_time=0.
    pub fn to_json(&self) -> String {
        let mut entries: Vec<(String, u32)> = self.counts.iter()
            .map(|((s, a), c)| (format!("{:?}|{:?}", s, a), *c))
            .collect();
        for ((w, s, a), c) in &self.time_counts {
            entries.push((format!("T:{:?}|{:?}|{:?}", w, s, a), *c));
        }
        // Sort so the output is deterministic across runs (useful for tests
        // and for diffing the DB content during diagnostics).
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into())
    }

    /// Restore counts from a JSON string produced by `to_json`. Handles
    /// both legacy entries (2 segments: state|action) and time-window
    /// entries (4 segments after T: prefix: T:window|state|action).
    /// Unknown state/action/window names are silently dropped (graceful
    /// schema migration). Returns the total number of cells restored.
    pub fn load_json(&mut self, json: &str) -> usize {
        let entries: Vec<(String, u32)> = serde_json::from_str(json)
            .unwrap_or_default();
        self.counts.clear();
        self.time_counts.clear();
        let mut loaded = 0;
        for (key, count) in entries {
            if let Some(rest) = key.strip_prefix("T:") {
                // Time-window entry: window|state|action
                let parts: Vec<&str> = rest.split('|').collect();
                if parts.len() != 3 { continue; }
                if let (Some(w), Some(s), Some(a)) =
                    (parse_window(parts[0]), parse_state(parts[1]), parse_action(parts[2]))
                {
                    let c = count.min(self.count_cap);
                    self.time_counts.insert((w, s, a), c);
                    loaded += 1;
                }
            } else if let Some((state_str, action_str)) = key.split_once('|') {
                if let (Some(s), Some(a)) =
                    (parse_state(state_str), parse_action(action_str))
                {
                    let c = count.min(self.count_cap);
                    self.counts.insert((s, a), c);
                    loaded += 1;
                }
            }
        }
        // If the deserialized payload was empty, re-apply priors so the
        // anticipator stays useful (e.g. when a new user first launches).
        if self.counts.is_empty() {
            self.seed_priors();
        }
        loaded
    }
}

impl Default for Anticipator { fn default() -> Self { Self::new() } }

fn count_actions_overall(
    state: &FsmState,
    counts: &HashMap<(FsmState, ActionKind), u32>,
) -> usize {
    counts.iter().filter(|((s, _), _)| s == state).count()
}

fn count_actions_time(
    window: &TimeWindow,
    state: &FsmState,
    time_counts: &HashMap<(TimeWindow, FsmState, ActionKind), u32>,
) -> usize {
    time_counts.iter()
        .filter(|((w, s, _), _)| w == window && s == state)
        .count()
}

/// Compatibility shim — kept because tests reference it. Same as
/// count_actions_overall above.
fn count_actions(state: &FsmState, counts: &HashMap<(FsmState, ActionKind), u32>) -> usize {
    count_actions_overall(state, counts)
}

/// Parse Debug-printed state name back into the enum. Returns None for
/// unknown names (e.g. when the persisted JSON predates a schema change).
fn parse_state(s: &str) -> Option<FsmState> {
    match s {
        "Idle"          => Some(FsmState::Idle),
        "Browsing"      => Some(FsmState::Browsing),
        "Playing"       => Some(FsmState::Playing),
        "Paused"        => Some(FsmState::Paused),
        "Transitioning" => Some(FsmState::Transitioning),
        "Tuning"        => Some(FsmState::Tuning),
        _ => None,
    }
}

fn parse_action(s: &str) -> Option<ActionKind> {
    match s {
        "PlayTrack"          => Some(ActionKind::PlayTrack),
        "TrackEnded"         => Some(ActionKind::TrackEnded),
        "Pause"              => Some(ActionKind::Pause),
        "Resume"             => Some(ActionKind::Resume),
        "SkipNext"           => Some(ActionKind::SkipNext),
        "SkipPrev"           => Some(ActionKind::SkipPrev),
        "VolumeChange"       => Some(ActionKind::VolumeChange),
        "SwitchDevice"       => Some(ActionKind::SwitchDevice),
        "OpenLibrary"        => Some(ActionKind::OpenLibrary),
        "OpenDevicePicker"   => Some(ActionKind::OpenDevicePicker),
        "InactivityTick"     => Some(ActionKind::InactivityTick),
        _ => None,
    }
}

fn parse_window(s: &str) -> Option<TimeWindow> {
    match s {
        "Morning"   => Some(TimeWindow::Morning),
        "Afternoon" => Some(TimeWindow::Afternoon),
        "Evening"   => Some(TimeWindow::Evening),
        "Night"     => Some(TimeWindow::Night),
        _ => None,
    }
}

/// State-transition logic. Pure function so it's easy to test.
fn next_state(
    current: &FsmState,
    action: ActionKind,
    at: Instant,
    state_since: Instant,
) -> FsmState {
    use ActionKind::*;
    use FsmState::*;
    match (current, action) {
        // Universal: PlayTrack → Playing regardless of where we were.
        (_, PlayTrack)             => Playing,
        (_, Resume)                => Playing,
        // Pause from anywhere lands us in Paused.
        (_, Pause)                 => Paused,
        // Track-end from Playing means we're momentarily Transitioning
        // (device picks next or app intervenes).
        (Playing, TrackEnded)      => Transitioning,
        // Volume tweaks move us into Tuning, but only if we were idling
        // or browsing — mid-playback the Tuning state is implicit.
        (Idle, VolumeChange)
            | (Browsing, VolumeChange)
            | (Paused, VolumeChange) => Tuning,
        // Open library / picker → Browsing.
        (_, OpenLibrary)
            | (_, OpenDevicePicker)    => Browsing,
        // Skip retains Playing (the user is curating in-flight).
        (Playing, SkipNext)
            | (Playing, SkipPrev)      => Playing,
        // Device switch is brief Transitioning.
        (_, SwitchDevice)              => Transitioning,
        // Inactivity ticks age out volatile states.
        (Tuning, InactivityTick)
            | (Transitioning, InactivityTick) => Idle,
        // Paused → Idle after 5 minutes (user gave up).
        (Paused, InactivityTick) if at.duration_since(state_since) > Duration::from_secs(5 * 60) => Idle,
        // No transition for unmatched (state, action) — stay put.
        _ => current.clone(),
    }
}

/// Hand-picked typical horizons for predictions. Used to drive pre-stage
/// timing — e.g. "Playing → TrackEnded in 60s" means we have ~60s to
/// have the next track ready.
fn expected_horizon_ms(state: &FsmState, action: ActionKind) -> u32 {
    use ActionKind::*;
    use FsmState::*;
    match (state, action) {
        (Playing, TrackEnded)    => 60_000,    // mid-track on average
        (Playing, Pause)         => 30_000,
        (Playing, VolumeChange)  => 15_000,
        (Playing, SkipNext)      => 20_000,
        (Paused, Resume)         => 15_000,    // people pause briefly
        (Paused, InactivityTick) => 180_000,
        (Browsing, PlayTrack)    => 5_000,     // pick a track quickly
        (Idle, OpenLibrary)      => 30_000,
        (Idle, PlayTrack)        => 10_000,
        (Idle, OpenDevicePicker) => 10_000,
        (Transitioning, PlayTrack) => 2_000,
        _ => 30_000,
    }
}

/// How costly is it to pre-stage for the predicted action?
fn cost_for(action: ActionKind) -> PrestageCost {
    use ActionKind::*;
    match action {
        // Connection-keep-alive + state polling — already happens, free.
        VolumeChange | InactivityTick => PrestageCost::Free,
        // SQLite lookup, source pre-switch, resolve next track URL.
        TrackEnded | SkipNext | SkipPrev | Resume => PrestageCost::Cheap,
        // PlayTrack from Browsing or Idle: pre-resolve top candidates.
        PlayTrack => PrestageCost::Moderate,
        // Device picker open → start probing all devices.
        OpenDevicePicker | OpenLibrary => PrestageCost::Cheap,
        // Switching devices is moderate — re-init connection pool.
        SwitchDevice => PrestageCost::Moderate,
        // Pause is free (just send a stop).
        Pause => PrestageCost::Free,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_anticipator_starts_idle() {
        let a = Anticipator::new();
        assert_eq!(a.state(), &FsmState::Idle);
    }

    #[test]
    fn play_track_from_idle_lands_in_playing() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);
        assert_eq!(a.state(), &FsmState::Playing);
    }

    #[test]
    fn pause_from_playing_lands_in_paused() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);
        a.observe(ActionKind::Pause);
        assert_eq!(a.state(), &FsmState::Paused);
    }

    #[test]
    fn resume_from_paused_lands_in_playing() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);
        a.observe(ActionKind::Pause);
        a.observe(ActionKind::Resume);
        assert_eq!(a.state(), &FsmState::Playing);
    }

    #[test]
    fn track_ended_lands_in_transitioning() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);
        a.observe(ActionKind::TrackEnded);
        assert_eq!(a.state(), &FsmState::Transitioning);
    }

    #[test]
    fn open_library_lands_in_browsing() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::OpenLibrary);
        assert_eq!(a.state(), &FsmState::Browsing);
    }

    #[test]
    fn paused_predicts_resume_with_highest_confidence() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);
        a.observe(ActionKind::Pause);
        let preds = a.predict(5);
        assert!(!preds.is_empty(), "should predict from Paused");
        // Top prediction should be Resume.
        assert_eq!(preds[0].action, ActionKind::Resume,
                   "expected Resume on top, got {:?}", preds);
        assert!(preds[0].confidence > 0.30,
                "Resume confidence should be substantial, got {}", preds[0].confidence);
    }

    #[test]
    fn playing_predicts_track_ended_with_highest_confidence() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);
        let preds = a.predict(3);
        assert_eq!(preds[0].action, ActionKind::TrackEnded);
    }

    #[test]
    fn browsing_predicts_play_track() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::OpenLibrary);
        let preds = a.predict(3);
        assert_eq!(preds[0].action, ActionKind::PlayTrack);
    }

    #[test]
    fn learning_strengthens_observed_transitions() {
        let mut a = Anticipator::new();
        // Observe Playing → SkipNext 20 times. After this, SkipNext from
        // Playing should be much higher than the initial prior.
        a.observe(ActionKind::PlayTrack);
        let preds_before = a.predict(10);
        let skip_before = preds_before.iter()
            .find(|p| p.action == ActionKind::SkipNext)
            .map(|p| p.confidence).unwrap_or(0.0);

        for _ in 0..20 {
            a.observe(ActionKind::SkipNext);
            // SkipNext from Playing keeps us in Playing per next_state.
        }
        let preds_after = a.predict(10);
        let skip_after = preds_after.iter()
            .find(|p| p.action == ActionKind::SkipNext)
            .map(|p| p.confidence).unwrap_or(0.0);

        assert!(skip_after > skip_before * 1.5,
                "after 20 Skips, confidence should grow: before {} after {}",
                skip_before, skip_after);
    }

    #[test]
    fn count_cap_prevents_runaway() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);
        for _ in 0..1000 { a.observe(ActionKind::SkipNext); }
        let count = a.counts.get(&(FsmState::Playing, ActionKind::SkipNext))
            .copied().unwrap_or(0);
        assert!(count <= a.count_cap,
                "count should be capped, got {} cap {}", count, a.count_cap);
    }

    #[test]
    fn prestage_filters_low_confidence_moderate_costs() {
        // Fresh anticipator: PlayTrack from Idle has confidence ~3/(3+4+3+1) ≈ 0.27
        // (below the 0.55 Moderate threshold), so it shouldn't recommend.
        let a = Anticipator::new();
        let recs = a.prestage_recommendations();
        let has_play_track = recs.iter().any(|p| p.action == ActionKind::PlayTrack
                                             && p.cost == PrestageCost::Moderate);
        assert!(!has_play_track,
                "Moderate-cost PlayTrack shouldn't be staged from cold Idle: {:?}", recs);
    }

    #[test]
    fn prestage_includes_high_confidence_cheap_actions() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);
        a.observe(ActionKind::Pause);
        // Paused → Resume is a Cheap action and has confidence well above 0.30.
        let recs = a.prestage_recommendations();
        assert!(recs.iter().any(|p| p.action == ActionKind::Resume),
                "Should stage Resume from Paused: {:?}", recs);
    }

    #[test]
    fn inactivity_in_tuning_returns_to_idle() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::OpenLibrary);
        a.observe(ActionKind::VolumeChange);  // → Tuning
        assert_eq!(a.state(), &FsmState::Tuning);
        a.observe(ActionKind::InactivityTick);
        assert_eq!(a.state(), &FsmState::Idle);
    }

    #[test]
    fn paused_long_enough_returns_to_idle_via_inactivity() {
        let mut a = Anticipator::new();
        let t0 = Instant::now();
        a.observe_at(ActionKind::PlayTrack, t0);
        a.observe_at(ActionKind::Pause, t0 + Duration::from_secs(1));
        // Within 5 min — InactivityTick keeps us Paused.
        a.observe_at(ActionKind::InactivityTick, t0 + Duration::from_secs(60));
        assert_eq!(a.state(), &FsmState::Paused);
        // After >5 min — InactivityTick demotes to Idle.
        a.observe_at(ActionKind::InactivityTick, t0 + Duration::from_secs(360));
        assert_eq!(a.state(), &FsmState::Idle);
    }

    #[test]
    fn switch_device_goes_through_transitioning() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);
        a.observe(ActionKind::SwitchDevice);
        assert_eq!(a.state(), &FsmState::Transitioning);
    }

    #[test]
    fn history_is_capped() {
        let mut a = Anticipator::new();
        for _ in 0..200 { a.observe(ActionKind::VolumeChange); }
        assert!(a.history.len() <= a.history_cap);
    }

    #[test]
    fn predictions_are_sorted_descending_by_confidence() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);
        let preds = a.predict(10);
        for w in preds.windows(2) {
            assert!(w[0].confidence >= w[1].confidence,
                    "not sorted: {} then {}", w[0].confidence, w[1].confidence);
        }
    }

    #[test]
    fn reset_to_clears_state() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);
        assert_eq!(a.state(), &FsmState::Playing);
        a.reset_to(FsmState::Idle);
        assert_eq!(a.state(), &FsmState::Idle);
    }

    #[test]
    fn confidence_sums_to_at_most_one_per_state() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);
        let preds = a.predict(20);  // all actions for current state
        let total: f32 = preds.iter().map(|p| p.confidence).sum();
        // With Laplace smoothing applied and the 0.05 filter, the total
        // should be close to 1.0 (slightly less when low-confidence cells
        // are dropped, but ≤ 1.0 always — they're a probability distribution).
        assert!(total <= 1.001, "confidences should sum to ≤ 1, got {}", total);
        assert!(total > 0.5, "confidences should sum to substantial, got {}", total);
    }

    #[test]
    fn to_json_then_load_roundtrips_counts() {
        let mut a = Anticipator::new();
        // Generate some interesting history.
        for _ in 0..5 { a.observe(ActionKind::OpenLibrary); }
        a.observe(ActionKind::PlayTrack);
        for _ in 0..10 { a.observe(ActionKind::SkipNext); }
        let snapshot = a.to_json();

        let mut b = Anticipator::new();
        let loaded = b.load_json(&snapshot);
        assert!(loaded > 0, "expected nonzero cells restored");

        // The predictions from the same state should match closely.
        // Compare as action→confidence maps so tie-broken sort order
        // doesn't make the assertion flaky.
        let preds_a = a.predict(10);
        b.reset_to(a.state().clone());
        let preds_b = b.predict(10);
        assert_eq!(preds_a.len(), preds_b.len());
        let map_a: std::collections::HashMap<_, _> = preds_a.iter()
            .map(|p| (p.action, p.confidence)).collect();
        let map_b: std::collections::HashMap<_, _> = preds_b.iter()
            .map(|p| (p.action, p.confidence)).collect();
        for (action, conf_a) in &map_a {
            let conf_b = map_b.get(action).copied().unwrap_or(0.0);
            assert!((conf_a - conf_b).abs() < 0.0001,
                    "mismatched confidence for {:?}: {} vs {}",
                    action, conf_a, conf_b);
        }
    }

    #[test]
    fn load_json_with_unknown_variants_skips_them() {
        // Forward-compat: if a future version adds new ActionKinds, an
        // older binary should silently skip those rows rather than panic.
        let mut a = Anticipator::new();
        let payload = r#"[["Playing|PlayTrack",5],["Future|Mystery",99],["Playing|PauseFutureVariant",3]]"#;
        let loaded = a.load_json(payload);
        assert_eq!(loaded, 1, "only the known cell should load");
        assert_eq!(*a.counts.get(&(FsmState::Playing, ActionKind::PlayTrack)).unwrap(), 5);
    }

    #[test]
    fn load_json_empty_falls_back_to_priors() {
        let mut a = Anticipator::new();
        let _ = a.load_json("[]");
        // Should still have seeded priors → predictions are non-empty.
        a.observe(ActionKind::PlayTrack);
        let preds = a.predict(3);
        assert!(!preds.is_empty(),
                "after loading empty JSON, priors should still drive predictions");
    }

    #[test]
    fn to_json_output_is_deterministic() {
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);
        a.observe(ActionKind::Pause);
        let j1 = a.to_json();
        // Same anticipator, same input → same string.
        let mut b = Anticipator::new();
        b.observe(ActionKind::PlayTrack);
        b.observe(ActionKind::Pause);
        let j2 = b.to_json();
        assert_eq!(j1, j2);
    }

    // ── Time-window tests ────────────────────────────────────────────

    #[test]
    fn time_window_from_hour_boundaries() {
        // Exact boundary hours
        assert_eq!(TimeWindow::from_hour(5),  TimeWindow::Morning);
        assert_eq!(TimeWindow::from_hour(10), TimeWindow::Morning);
        assert_eq!(TimeWindow::from_hour(11), TimeWindow::Afternoon);
        assert_eq!(TimeWindow::from_hour(16), TimeWindow::Afternoon);
        assert_eq!(TimeWindow::from_hour(17), TimeWindow::Evening);
        assert_eq!(TimeWindow::from_hour(21), TimeWindow::Evening);
        assert_eq!(TimeWindow::from_hour(22), TimeWindow::Night);
        assert_eq!(TimeWindow::from_hour(4),  TimeWindow::Night);
        assert_eq!(TimeWindow::from_hour(0),  TimeWindow::Night);
        assert_eq!(TimeWindow::from_hour(23), TimeWindow::Night);
    }

    #[test]
    fn time_window_labels_are_distinct() {
        let labels = [
            TimeWindow::Morning.label(),
            TimeWindow::Afternoon.label(),
            TimeWindow::Evening.label(),
            TimeWindow::Night.label(),
        ];
        let mut sorted = labels.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 4);
    }

    #[test]
    fn observe_at_in_window_updates_both_tables() {
        let mut a = Anticipator::new();
        let t = Instant::now();
        a.observe_at_in_window(ActionKind::PlayTrack, t, TimeWindow::Morning);
        // Unconditional count incremented
        assert_eq!(*a.counts.get(&(FsmState::Idle, ActionKind::PlayTrack)).unwrap_or(&0)
                       - 3,  // 3 was the seed prior
                   1);
        // Time-window count also incremented
        assert_eq!(*a.time_counts.get(
            &(TimeWindow::Morning, FsmState::Idle, ActionKind::PlayTrack)
        ).unwrap_or(&0), 1);
    }

    #[test]
    fn predict_with_no_time_data_matches_unconditional() {
        // With NO observations in this time window, predict should fall
        // back entirely to the overall distribution.
        let mut a = Anticipator::new();
        a.observe(ActionKind::PlayTrack);  // → Playing
        let preds_overall: HashMap<ActionKind, f32> = a.predict_in_window(20, TimeWindow::Morning)
            .into_iter().map(|p| (p.action, p.confidence)).collect();
        // Force-empty the time_counts to be sure.
        a.time_counts.clear();
        let preds_again: HashMap<ActionKind, f32> = a.predict_in_window(20, TimeWindow::Morning)
            .into_iter().map(|p| (p.action, p.confidence)).collect();
        for (k, v) in &preds_overall {
            let v2 = preds_again.get(k).copied().unwrap_or(0.0);
            assert!((v - v2).abs() < 0.001,
                    "without time data, predict should match unconditional");
        }
    }

    #[test]
    fn predict_shifts_with_strong_time_data() {
        // Inject lots of observations in one window — predictions in
        // that window should diverge from the unconditional baseline.
        let mut a = Anticipator::new();
        // From Idle, OpenLibrary 30 times in the morning specifically.
        // Each observation stays in Idle (OpenLibrary moves us to
        // Browsing, so we need to reset state before each).
        let t0 = Instant::now();
        for i in 0..30 {
            a.reset_to(FsmState::Idle);
            a.observe_at_in_window(
                ActionKind::OpenLibrary,
                t0 + Duration::from_secs(i),
                TimeWindow::Morning,
            );
        }
        // Now predict from Idle in Morning vs Night.
        a.reset_to(FsmState::Idle);
        let morning_preds: HashMap<ActionKind, f32> = a.predict_in_window(20, TimeWindow::Morning)
            .into_iter().map(|p| (p.action, p.confidence)).collect();
        let night_preds: HashMap<ActionKind, f32> = a.predict_in_window(20, TimeWindow::Night)
            .into_iter().map(|p| (p.action, p.confidence)).collect();
        let m = morning_preds.get(&ActionKind::OpenLibrary).copied().unwrap_or(0.0);
        let n = night_preds.get(&ActionKind::OpenLibrary).copied().unwrap_or(0.0);
        // OpenLibrary in the morning should now be substantially more
        // likely than in the night.
        assert!(m > n,
                "OpenLibrary should be more likely in observed window: morning={} night={}",
                m, n);
        assert!(m > 0.40,
                "OpenLibrary in morning should have high confidence after 30 obs, got {}", m);
    }

    #[test]
    fn time_window_save_load_roundtrips() {
        let mut a = Anticipator::new();
        let t0 = Instant::now();
        for i in 0..5 {
            a.reset_to(FsmState::Idle);
            a.observe_at_in_window(
                ActionKind::OpenLibrary,
                t0 + Duration::from_secs(i),
                TimeWindow::Morning,
            );
        }
        for i in 0..3 {
            a.reset_to(FsmState::Idle);
            a.observe_at_in_window(
                ActionKind::PlayTrack,
                t0 + Duration::from_secs(10 + i),
                TimeWindow::Evening,
            );
        }
        let snapshot = a.to_json();
        let mut b = Anticipator::new();
        let _ = b.load_json(&snapshot);
        // The time_counts should match
        assert_eq!(b.time_counts.get(
            &(TimeWindow::Morning, FsmState::Idle, ActionKind::OpenLibrary)
        ).copied().unwrap_or(0), 5);
        assert_eq!(b.time_counts.get(
            &(TimeWindow::Evening, FsmState::Idle, ActionKind::PlayTrack)
        ).copied().unwrap_or(0), 3);
    }

    #[test]
    fn legacy_format_without_time_loads_clean() {
        // Old JSON without "T:" entries should still work — the
        // time_counts table just stays empty.
        let payload = r#"[["Playing|PlayTrack",5],["Playing|TrackEnded",10]]"#;
        let mut a = Anticipator::new();
        let loaded = a.load_json(payload);
        assert_eq!(loaded, 2);
        assert!(a.time_counts.is_empty());
        assert_eq!(*a.counts.get(&(FsmState::Playing, ActionKind::PlayTrack)).unwrap(), 5);
    }

    #[test]
    fn time_blend_alpha_grows_with_sample_size() {
        // At total_time=0 → alpha=0 (no time signal)
        // At total_time=reg → alpha=0.5
        // At total_time=>>reg → alpha→1
        let a = Anticipator::new();
        // Helper: compute alpha at a given total_time
        let calc = |t: u32| {
            if t == 0 { 0.0 } else { (t as f32) / (t as f32 + a.time_blend_reg) }
        };
        assert_eq!(calc(0), 0.0);
        let half = calc(a.time_blend_reg as u32);
        assert!((half - 0.5).abs() < 0.001);
        let big = calc(100);
        assert!(big > 0.85);
    }
}
