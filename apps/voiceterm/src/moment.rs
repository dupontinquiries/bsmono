// src/moment.rs
//
// v0.1.175 — moment classifier (ship 1b-α of voice corpus arc).
//
// A small context-aware module that classifies "what kind of moment is
// this for the user." Designed to support context-aware UX decisions
// like:
//
//   - When to prompt for a corpus recording vs. stay silent
//   - When to use gentle tone vs. normal tone
//   - When to acknowledge the environment (e.g. "you're in a cafe")
//     instead of pushing a feature
//
// User direction (conversation preserved in changelog/0.1.1x.md and
// the top of docs/todo.md):
//   - "maybe just don't ask at 3am and instead say why are you reading
//     at 3am?"
//   - cafe-wink: acknowledge moments where features wouldn't work
//   - two-voice co-read: free onboarding when two people are reading
//
// This is the framework, not all the moment types. 1b-α ships only:
//   - Moment struct: holds the current context signals
//   - corpus_prompt_decision(): minimal classifier returning Silence
//     or SoloPrompt
//
// Future sub-ships will add:
//   - 1b-β: deep-night explicit handling + check-in option
//   - 1b-γ: two-voice detection + co-read prompt
//   - 1b-δ: cafe-wink and other contextual responses
//
// Logging-only for now. No UI fires from this; callers (eventually
// QML on a timer) can query the classifier and log its decision for
// threshold tuning. Once we trust the classifier, wire it to actual
// prompts.

use std::time::{SystemTime, UNIX_EPOCH};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Coarse time-of-day buckets. Maps from clock hour to a class that
/// downstream code can pattern-match. We use buckets rather than raw
/// hour because the meaningful distinctions are coarse: "is this a
/// time the user is probably tired/winding-down/should-be-asleep" is
/// what matters, not the specific hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeClass {
    /// 1am-5am. Most people should be asleep. Never prompt features
    /// here. If the system says anything, it's a check-in or silence.
    DeepNight,
    /// 5am-8am. Up early — could be working, could be insomnia.
    /// Gentle tone if any prompt fires.
    EarlyMorning,
    /// 8am-5pm. Normal day. Standard prompts are appropriate.
    Day,
    /// 5pm-9pm. Wind-down. Standard prompts still appropriate but
    /// the user is probably less focused.
    Evening,
    /// 9pm-1am. Late but not deep. Gentler than Day, but not as
    /// reserved as DeepNight.
    Night,
}

impl TimeClass {
    pub fn from_hour(hour: u8) -> Self {
        match hour {
            1..=4 => TimeClass::DeepNight,
            5..=7 => TimeClass::EarlyMorning,
            8..=16 => TimeClass::Day,
            17..=20 => TimeClass::Evening,
            // 21, 22, 23, 0 — Night.
            _ => TimeClass::Night,
        }
    }

    /// True for time classes where the system should default to
    /// quieter behavior. Other modules can check this to soften
    /// their copy or skip non-essential interactions.
    pub fn is_gentle(&self) -> bool {
        matches!(self, TimeClass::DeepNight | TimeClass::EarlyMorning | TimeClass::Night)
    }
}

/// Snapshot of the user's current context. Cheap to compute; each
/// field is either a clock read or an atomic load. Callers should
/// not cache this — it's intended to be queried fresh whenever a
/// classification decision is needed.
#[derive(Debug, Clone)]
pub struct Moment {
    pub hour: u8,
    pub time_class: TimeClass,
    /// Ambient noise floor from audio_profile. Higher = noisier
    /// environment. ~0.005-0.015 is typical for a quiet room;
    /// ~0.03+ suggests cafe/office/family-room.
    pub noise_floor: f32,
    /// Number of distinct voices the system has heard recently.
    /// 1 = solo, 2+ = multiple people present. 1b-α: hardcoded
    /// to 1 (real detection comes in 1b-γ).
    pub voices_heard: usize,
    /// Whether the user is scrolling at reading pace (slow,
    /// sustained) rather than skimming or jumping around.
    /// 1b-α: hardcoded to false (real detection comes in
    /// 1b-β; for now we err on "not reading-paced" so we
    /// don't prompt prematurely).
    pub is_reading_paced: bool,
    /// Seconds since app launch. Used to suppress prompts in
    /// the first minute of a session ("just opened the app,
    /// don't pester me yet").
    pub session_age_secs: u64,
}

/// Set at startup so `session_age_secs` can compute time-since-launch.
/// Initialized lazily on first read of Moment::current() if zero.
static SESSION_START_UNIX: AtomicU64 = AtomicU64::new(0);

fn ensure_session_start() -> u64 {
    let cur = SESSION_START_UNIX.load(Ordering::Relaxed);
    if cur != 0 { return cur; }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // CAS: only the first thread to set this wins. Subsequent
    // calls keep the original value.
    let _ = SESSION_START_UNIX.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
    SESSION_START_UNIX.load(Ordering::Relaxed)
}

/// Current local-time hour (0-23). Uses chrono if available, else
/// falls back to UTC. For 1b-α we're conservative: use UTC and
/// accept some imprecision around timezone transitions. Future
/// ships could pull timezone from the OS.
fn current_hour() -> u8 {
    // Total seconds since UNIX epoch.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Hour of day in UTC. Local-time correction is a TODO.
    // For users in Quebec (EST/EDT), UTC is ahead by 4-5 hours.
    // We apply a fixed offset from env var WEB2QML_TZ_OFFSET_HOURS
    // (default: -5 for EST). Crude but better than no offset at all.
    let tz_offset_hours: i32 = std::env::var("WEB2QML_TZ_OFFSET_HOURS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(-5);
    let utc_hour = ((secs / 3600) % 24) as i32;
    let local = (utc_hour + tz_offset_hours + 24) % 24;
    local as u8
}

impl Moment {
    /// Snapshot the current context. Cheap; safe to call frequently.
    pub fn current() -> Self {
        let hour = current_hour();
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let session_start = ensure_session_start();
        let session_age_secs = now_unix.saturating_sub(session_start);

        Moment {
            hour,
            time_class: TimeClass::from_hour(hour),
            noise_floor: crate::audio_profile::noise_floor(),
            // v0.1.188 (1b-γ): real voices_heard from speakers ring.
            // 30s window matches typical conversational rhythm —
            // long enough that a brief pause doesn't drop a speaker
            // out of the count, short enough that an hour-old
            // conversation doesn't still register.
            //
            // Caveat: relies on voice fingerprint identification,
            // which can misclassify. False positives → spurious
            // CoreadPrompt → user dismisses → cooldown engages.
            // Acceptable for now.
            voices_heard: crate::speakers::voices_heard_recent(30).max(1),
            // v0.1.183 (1b-β.1): real reading-pace from scroll_signals.
            // Returns true when velocity has been in the slow-reading
            // range (5-200 px/sec) sustained for 8+ seconds AND user
            // hasn't been at rest for >30s.
            is_reading_paced: crate::scroll_signals::is_reading_paced(),
            session_age_secs,
        }
    }

    /// Classify whether this moment is appropriate for a corpus
    /// prompt ("want to read this aloud?"). The default is
    /// Silence — we only return SoloPrompt when several signals
    /// align positively.
    ///
    /// This is intentionally conservative. False positives (prompting
    /// when we shouldn't) are much worse than false negatives
    /// (staying quiet when we could have prompted). The user can
    /// always invoke the corpus capture manually.
    pub fn corpus_prompt_decision(&self) -> CorpusPromptDecision {
        // Defenses, in priority order:

        // 1. Just-opened-the-app: give the user at least 60 seconds
        //    to settle in. Prompting immediately on launch feels
        //    pushy and gives no time to assess context anyway.
        //    Applies to ALL decision types including CafeWink — we
        //    don't even acknowledge environment in the first minute.
        if self.session_age_secs < 60 {
            return CorpusPromptDecision::Silence;
        }

        // 2. Noisy environment. v0.1.189 (1b-δ): now branches to
        //    CafeWink when sustained. The sustainment check
        //    (NOISY_POLLS_CONSECUTIVE) happens in
        //    `current_decision_effective` since the classifier
        //    itself is stateless. Here we just note: if noise is
        //    high, return CafeWink as the raw classifier output;
        //    the effective layer gates on sustainment.
        //
        //    Corpus recording would be poor in a noisy environment,
        //    so we never reach SoloPrompt/CoreadPrompt with high
        //    noise. The wink IS the response.
        if self.noise_floor > 0.02 {
            return CorpusPromptDecision::CafeWink;
        }

        // 3. Not reading-paced: user is doing something active,
        //    not absorbed in long-form. Prompting would interrupt.
        //    Applies to ALL prompt types — we only want to surface
        //    a moment when the user is genuinely reading. Skimming
        //    or skipping isn't a moment to engage.
        if !self.is_reading_paced {
            return CorpusPromptDecision::Silence;
        }

        // 4. v0.1.187 (1b-β.3): DeepNight branches off here.
        //    User is reading-paced + quiet, but at 1-4am.
        //    v0.1.188 (1b-γ): if multiple voices ALSO at 3am, that's
        //    an unusual social moment we don't have a good response
        //    to — staying silent is the right move. Two people up
        //    reading at 3am is not for the system to comment on.
        if self.time_class == TimeClass::DeepNight {
            if self.voices_heard > 1 {
                return CorpusPromptDecision::Silence;
            }
            return CorpusPromptDecision::DeepNightCheckIn;
        }

        // 5. v0.1.188 (1b-γ): multiple voices in normal hours →
        //    CoreadPrompt. Two people both reading-paced suggests
        //    they're in the same reading context (sharing a screen,
        //    or co-located reading similar material). Offer to read
        //    together — a natural moment to engage two voices in the
        //    corpus capture flow.
        //
        //    Caveat: false positives are possible if voice
        //    fingerprinting misclassifies (single user with vocal
        //    variation read as two speakers). The user dismisses,
        //    cooldown applies. Acceptable.
        if self.voices_heard > 1 {
            return CorpusPromptDecision::CoreadPrompt;
        }

        // 6. All defenses passed → standard solo prompt.
        CorpusPromptDecision::SoloPrompt
    }

    /// Human-readable summary for logging. Used by the eventual
    /// log-this-moment caller for threshold tuning.
    pub fn describe(&self) -> String {
        format!(
            "hour={} class={:?} noise={:.4} voices={} reading={} session_age={}s scroll[{}]",
            self.hour, self.time_class, self.noise_floor,
            self.voices_heard, self.is_reading_paced, self.session_age_secs,
            crate::scroll_signals::describe(),
        )
    }
}

/// Decisions the classifier can return for the corpus-prompt question.
/// 1b-α has just Silence and SoloPrompt. Future variants:
///   - CoreadPrompt (1b-γ): two voices present, both reading-paced
///   - CafeWink (1b-δ): noisy environment, acknowledge with humor
///   - DeepNightCheckIn (1b-β optional): "still up?" gentle check-in
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorpusPromptDecision {
    /// Default. Do nothing.
    Silence,
    /// Standard "want to read aloud?" prompt — single user, quiet
    /// environment, sustained reading.
    SoloPrompt,
    /// v0.1.187 (1b-β.3): DeepNight check-in. User is reading-paced
    /// during 1-4am hours. Not a feature prompt — a gentle
    /// acknowledgment that the system has noticed the unusual time.
    /// Single dismiss button, no yes/no/never structure. Auto-fires
    /// once per process lifetime then stays quiet.
    DeepNightCheckIn,
    /// v0.1.188 (1b-γ): co-read prompt. Two or more distinct voices
    /// heard in the recent window AND user is reading-paced. Offers
    /// "read this together?" — natural moment to engage a second
    /// person without explicit setup.
    CoreadPrompt,
    /// v0.1.189 (1b-δ): cafe-wink. Environment is sustained-noisy
    /// (cafe, busy room, ambient chatter). Corpus recording would
    /// be poor here. Instead of staying silent, acknowledge the
    /// moment lightly — "noisy out there." Single dismiss button,
    /// one-shot per process. Like DeepNightCheckIn but for a
    /// different context.
    CafeWink,
}

impl CorpusPromptDecision {
    /// String representation for the QML bridge. Strings rather
    /// than ints so QML can do simple text comparisons and new
    /// decision variants don't require numeric coordination.
    pub fn as_str(&self) -> &'static str {
        match self {
            CorpusPromptDecision::Silence => "silence",
            CorpusPromptDecision::SoloPrompt => "solo",
            CorpusPromptDecision::DeepNightCheckIn => "checkin",
            CorpusPromptDecision::CoreadPrompt => "coread",
            CorpusPromptDecision::CafeWink => "wink",
        }
    }
}

// v0.1.186 (1b-β.2): prompt suppression state. The classifier may
// return SoloPrompt continuously while the user is reading-paced;
// the toast should appear once and then stay quiet until either
// the user dismisses it (cooldown) or silences the rest of the
// session.
//
// Storage as atomic Unix-millis timestamps so reads are cheap
// and lock-free. 0 = no cooldown set.
use std::sync::atomic::AtomicBool;

static PROMPT_COOLDOWN_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
static SESSION_SUPPRESSED: AtomicBool = AtomicBool::new(false);

/// v0.1.187 (1b-β.3): once the DeepNight check-in has been shown
/// for this process lifetime, don't show it again. The point is
/// "I noticed once" — repeating would be nagging. Resets on
/// process restart, so if the user restarts at 3am they get a
/// fresh acknowledgment (arguably appropriate).
static CHECKIN_SHOWN: AtomicBool = AtomicBool::new(false);

/// v0.1.189 (1b-δ): once the cafe-wink has been shown for this
/// process lifetime, stay quiet. Same one-shot semantics as
/// CHECKIN_SHOWN — the wink IS the acknowledgment, repeating
/// would defeat the point.
static WINK_SHOWN: AtomicBool = AtomicBool::new(false);

/// v0.1.189 (1b-δ): count of consecutive ticks where noise has
/// been above threshold. The CafeWink only fires after sustained
/// noise — a single noisy tick (door slam, brief background) is
/// not enough. After ~2-3 consecutive noisy ticks (~6-9 seconds
/// at the 3s poll interval), we conclude the environment is
/// genuinely noisy and the wink is appropriate.
///
/// Resets to 0 whenever noise drops below threshold, so the
/// counter doesn't accumulate across quiet intervals.
static NOISY_POLLS_CONSECUTIVE: AtomicU32 = AtomicU32::new(0);

/// Number of consecutive noisy ticks before CafeWink fires.
/// At 3s polling interval, 2 ticks = ~6 seconds of sustained
/// noise. Filters out brief noisy moments (door, cough, single
/// loud word) while still firing quickly enough to feel
/// responsive to a real cafe-context.
const SUSTAINED_NOISY_TICKS: u32 = 2;

/// Default cooldown after a "not now" dismiss. 15 minutes feels
/// long enough not to harass; short enough that if the user
/// settles into reading again later in the same session, the
/// system can offer again.
const DISMISS_COOLDOWN_MS: u64 = 15 * 60 * 1000;

/// After "yes" — much longer cooldown. The user has expressed
/// interest; even if they don't actually do the capture yet
/// (the capture flow is ship 2 of the corpus arc, not built),
/// we shouldn't ask again soon. ~hours feels right.
const ACCEPTED_COOLDOWN_MS: u64 = 4 * 60 * 60 * 1000;

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Called by QML's periodic moment-check. Returns the effective
/// decision: the raw classifier output unless suppression is in
/// effect, in which case Silence. Single source of truth for
/// QML's "should I show a prompt?" question.
///
/// v0.1.189: also drives the CafeWink sustainment counter — each
/// call increments the counter when noise is high, resets it
/// when noise drops. The wink fires only after sustained noise.
/// v0.1.193: pure suppression-application logic, extracted from
/// `current_decision_effective` for testability. Takes the raw
/// classifier output plus the suppression state (read from
/// atomics by the caller) and returns the effective decision
/// plus the new sustainment-counter value the caller should
/// store.
///
/// This is the only place the suppression branching lives — both
/// the production path (`current_decision_effective`) and the
/// tests below use it. Keeps the behavior verifiable without
/// having to coax the atomics into specific states.
#[derive(Clone, Copy, Debug)]
pub struct SuppressionState {
    pub session_suppressed: bool,
    pub checkin_shown: bool,
    pub wink_shown: bool,
    pub prompt_cooldown_active: bool,
    pub noisy_polls_before: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct SuppressionResult {
    pub effective: CorpusPromptDecision,
    /// New value the caller should store in NOISY_POLLS_CONSECUTIVE.
    /// Either incremented (raw was CafeWink) or reset to 0
    /// (raw was anything else).
    pub noisy_polls_after: u32,
}

pub fn apply_suppression(raw: CorpusPromptDecision, state: SuppressionState) -> SuppressionResult {
    // Session-wide silence trumps everything.
    if state.session_suppressed {
        return SuppressionResult {
            effective: CorpusPromptDecision::Silence,
            // Don't increment the counter when session-suppressed;
            // we're not exercising the wink path at all.
            noisy_polls_after: 0,
        };
    }

    // CafeWink sustainment handling. Increment-or-reset counter,
    // gate on threshold + one-shot.
    if raw == CorpusPromptDecision::CafeWink {
        let new_count = state.noisy_polls_before.saturating_add(1);
        if new_count < SUSTAINED_NOISY_TICKS {
            return SuppressionResult {
                effective: CorpusPromptDecision::Silence,
                noisy_polls_after: new_count,
            };
        }
        if state.wink_shown {
            return SuppressionResult {
                effective: CorpusPromptDecision::Silence,
                noisy_polls_after: new_count,
            };
        }
        return SuppressionResult {
            effective: CorpusPromptDecision::CafeWink,
            noisy_polls_after: new_count,
        };
    }
    // Raw isn't CafeWink — reset the sustainment counter.
    let noisy_polls_after = 0;

    // DeepNightCheckIn one-shot suppression.
    if raw == CorpusPromptDecision::DeepNightCheckIn && state.checkin_shown {
        return SuppressionResult {
            effective: CorpusPromptDecision::Silence,
            noisy_polls_after,
        };
    }

    // SoloPrompt and CoreadPrompt both honor the prompt cooldown.
    if (raw == CorpusPromptDecision::SoloPrompt
        || raw == CorpusPromptDecision::CoreadPrompt)
        && state.prompt_cooldown_active
    {
        return SuppressionResult {
            effective: CorpusPromptDecision::Silence,
            noisy_polls_after,
        };
    }

    SuppressionResult {
        effective: raw,
        noisy_polls_after,
    }
}

/// Called by QML's periodic moment-check. Returns the effective
/// decision: the raw classifier output unless suppression is in
/// effect, in which case Silence. Single source of truth for
/// QML's "should I show a prompt?" question.
///
/// v0.1.189: also drives the CafeWink sustainment counter — each
/// call increments the counter when noise is high, resets it
/// when noise drops. The wink fires only after sustained noise.
///
/// v0.1.193: implementation delegated to the pure
/// `apply_suppression` helper; this function just reads the
/// atomics, calls it, and writes the new counter value back.
pub fn current_decision_effective() -> CorpusPromptDecision {
    let raw = Moment::current().corpus_prompt_decision();
    let now = unix_ms();
    let cooldown_until = PROMPT_COOLDOWN_UNTIL_MS.load(Ordering::Relaxed);
    let state = SuppressionState {
        session_suppressed: SESSION_SUPPRESSED.load(Ordering::Relaxed),
        checkin_shown: CHECKIN_SHOWN.load(Ordering::Relaxed),
        wink_shown: WINK_SHOWN.load(Ordering::Relaxed),
        prompt_cooldown_active: now < cooldown_until,
        noisy_polls_before: NOISY_POLLS_CONSECUTIVE.load(Ordering::Relaxed),
    };
    let r = apply_suppression(raw, state);
    NOISY_POLLS_CONSECUTIVE.store(r.noisy_polls_after, Ordering::Relaxed);
    r.effective
}

/// User tapped "yes" on the prompt. Records acceptance and
/// applies a long cooldown. (The actual corpus-capture flow
/// isn't built yet — that's ship 2 of the voice corpus arc.
/// For now, "yes" just acknowledges and quiets things down.)
pub fn prompt_accepted() {
    let now = unix_ms();
    PROMPT_COOLDOWN_UNTIL_MS.store(now + ACCEPTED_COOLDOWN_MS, Ordering::Relaxed);
    crate::clog!("moment", "prompt accepted; cooldown {} hours",
        ACCEPTED_COOLDOWN_MS / 3_600_000);
}

/// User tapped "not now." Short cooldown — we may ask again
/// later this session if conditions align.
pub fn prompt_dismissed() {
    let now = unix_ms();
    PROMPT_COOLDOWN_UNTIL_MS.store(now + DISMISS_COOLDOWN_MS, Ordering::Relaxed);
    crate::clog!("moment", "prompt dismissed; cooldown {} minutes",
        DISMISS_COOLDOWN_MS / 60_000);
}

/// User tapped "never this session." Suppresses for the rest of
/// the process lifetime. Resets on restart.
pub fn prompt_session_silenced() {
    SESSION_SUPPRESSED.store(true, Ordering::Relaxed);
    crate::clog!("moment", "prompts silenced for rest of session");
}

/// v0.1.187 (1b-β.3): user dismissed the DeepNight check-in. The
/// system has been acknowledged. Mark CHECKIN_SHOWN so the
/// check-in stays quiet for the rest of the process lifetime.
/// Doesn't affect SoloPrompt or other decision types.
pub fn checkin_dismissed() {
    CHECKIN_SHOWN.store(true, Ordering::Relaxed);
    crate::clog!("moment", "deep-night check-in acknowledged");
}

/// v0.1.189 (1b-δ): user dismissed the cafe-wink. Mark WINK_SHOWN
/// so further sustained-noise stretches stay quiet for the rest
/// of the process. Same one-shot semantics as checkin_dismissed —
/// the wink IS the acknowledgment, repeating would defeat it.
pub fn wink_dismissed() {
    WINK_SHOWN.store(true, Ordering::Relaxed);
    crate::clog!("moment", "cafe-wink acknowledged");
}

/// Log the current moment + decision to ~/.config/web2qml/moments.log.
/// Used for threshold tuning during 1b-α before we wire the
/// classifier to user-facing UI. Callers (eventually a QML timer or
/// a Rust periodic task) invoke this every N seconds during normal
/// use; we accumulate a record of what the classifier would have
/// decided, and look at the log to confirm classifications are
/// sensible before letting them drive UI.
///
/// Gated on `WEB2QML_MOMENT_LOG=1` env var so this is opt-in. Same
/// pattern as WEB2QML_PROFILE for profiling.log.
pub fn log_current() {
    if std::env::var("WEB2QML_MOMENT_LOG").map(|v| v != "0").unwrap_or(false) == false {
        return;
    }
    let m = Moment::current();
    let decision = m.corpus_prompt_decision();
    let path = match std::env::var("HOME") {
        Ok(h) => std::path::PathBuf::from(h).join(".config/web2qml/moments.log"),
        Err(_) => return,
    };
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true).append(true).open(&path)
    {
        let unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "t={} decision={:?} {}", unix, decision, m.describe());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_class_hours() {
        assert_eq!(TimeClass::from_hour(0), TimeClass::Night);
        assert_eq!(TimeClass::from_hour(3), TimeClass::DeepNight);
        assert_eq!(TimeClass::from_hour(6), TimeClass::EarlyMorning);
        assert_eq!(TimeClass::from_hour(12), TimeClass::Day);
        assert_eq!(TimeClass::from_hour(18), TimeClass::Evening);
        assert_eq!(TimeClass::from_hour(22), TimeClass::Night);
        assert_eq!(TimeClass::from_hour(23), TimeClass::Night);
    }

    #[test]
    fn gentle_classes() {
        assert!(TimeClass::DeepNight.is_gentle());
        assert!(TimeClass::EarlyMorning.is_gentle());
        assert!(TimeClass::Night.is_gentle());
        assert!(!TimeClass::Day.is_gentle());
        assert!(!TimeClass::Evening.is_gentle());
    }

    #[test]
    fn deep_night_reading_returns_checkin() {
        // v0.1.187 change: DeepNight + reading-paced = check-in,
        // not silence. The check-in is "I see you reading at 3am";
        // it's not a feature prompt.
        let m = Moment {
            hour: 3,
            time_class: TimeClass::DeepNight,
            noise_floor: 0.005,
            voices_heard: 1,
            is_reading_paced: true,
            session_age_secs: 600,
        };
        assert_eq!(m.corpus_prompt_decision(), CorpusPromptDecision::DeepNightCheckIn);
    }

    #[test]
    fn deep_night_not_reading_silences() {
        // At 3am but skimming or doing something active: silence.
        // The check-in is for actual reading at unusual hours,
        // not for any-app-use at unusual hours.
        let m = Moment {
            hour: 3,
            time_class: TimeClass::DeepNight,
            noise_floor: 0.005,
            voices_heard: 1,
            is_reading_paced: false,
            session_age_secs: 600,
        };
        assert_eq!(m.corpus_prompt_decision(), CorpusPromptDecision::Silence);
    }

    #[test]
    fn deep_night_noisy_returns_wink() {
        // v0.1.189 change: noise dominates time-class. Even at
        // 3am, sustained noise → CafeWink, not DeepNightCheckIn.
        // The wink is the right response when the environment is
        // the salient signal — time-of-day matters less when there
        // is commotion. The raw classifier returns CafeWink;
        // current_decision_effective will gate on sustainment.
        let m = Moment {
            hour: 3,
            time_class: TimeClass::DeepNight,
            noise_floor: 0.05,
            voices_heard: 1,
            is_reading_paced: true,
            session_age_secs: 600,
        };
        assert_eq!(m.corpus_prompt_decision(), CorpusPromptDecision::CafeWink);
    }

    #[test]
    fn just_launched_silences() {
        let m = Moment {
            hour: 14,
            time_class: TimeClass::Day,
            noise_floor: 0.005,
            voices_heard: 1,
            is_reading_paced: true,
            session_age_secs: 30, // < 60
        };
        assert_eq!(m.corpus_prompt_decision(), CorpusPromptDecision::Silence);
    }

    #[test]
    fn just_launched_noisy_silences() {
        // The just-launched grace overrides noise. CafeWink
        // shouldn't fire in the first minute either.
        let m = Moment {
            hour: 14,
            time_class: TimeClass::Day,
            noise_floor: 0.05,
            voices_heard: 1,
            is_reading_paced: true,
            session_age_secs: 30,
        };
        assert_eq!(m.corpus_prompt_decision(), CorpusPromptDecision::Silence);
    }

    #[test]
    fn noisy_returns_wink() {
        // v0.1.189: noisy environment in normal hours → CafeWink
        // (raw classifier). The effective layer requires sustained
        // noise (2+ consecutive ticks) before this becomes a UI
        // event.
        let m = Moment {
            hour: 14,
            time_class: TimeClass::Day,
            noise_floor: 0.05,
            voices_heard: 1,
            is_reading_paced: true,
            session_age_secs: 600,
        };
        assert_eq!(m.corpus_prompt_decision(), CorpusPromptDecision::CafeWink);
    }

    #[test]
    fn noisy_not_reading_still_wink() {
        // CafeWink doesn't require reading-paced — the environment
        // is the trigger, not the user's activity. Noise gate is
        // before the reading gate, so this returns CafeWink even
        // when reading-paced=false.
        let m = Moment {
            hour: 14,
            time_class: TimeClass::Day,
            noise_floor: 0.05,
            voices_heard: 1,
            is_reading_paced: false,
            session_age_secs: 600,
        };
        assert_eq!(m.corpus_prompt_decision(), CorpusPromptDecision::CafeWink);
    }

    #[test]
    fn multiple_voices_returns_coread() {
        // v0.1.188 change: multi-voice during day → CoreadPrompt,
        // not silence. The previous behavior (silence) became
        // CoreadPrompt now that we have a real response variant.
        let m = Moment {
            hour: 14,
            time_class: TimeClass::Day,
            noise_floor: 0.005,
            voices_heard: 2,
            is_reading_paced: true,
            session_age_secs: 600,
        };
        assert_eq!(m.corpus_prompt_decision(), CorpusPromptDecision::CoreadPrompt);
    }

    #[test]
    fn multiple_voices_at_deep_night_silences() {
        // Two people up reading at 3am: don't presume to comment.
        // Silence, not CheckIn (which is for solitary late-night).
        // Silence, not CoreadPrompt (the moment is unusual; don't
        // push a feature into it).
        let m = Moment {
            hour: 3,
            time_class: TimeClass::DeepNight,
            noise_floor: 0.005,
            voices_heard: 2,
            is_reading_paced: true,
            session_age_secs: 600,
        };
        assert_eq!(m.corpus_prompt_decision(), CorpusPromptDecision::Silence);
    }

    #[test]
    fn multiple_voices_not_reading_silences() {
        // Two voices but neither is reading-paced (e.g. having
        // a conversation but not engaged with the screen): silence.
        // CoreadPrompt is for two people both reading.
        let m = Moment {
            hour: 14,
            time_class: TimeClass::Day,
            noise_floor: 0.005,
            voices_heard: 2,
            is_reading_paced: false,
            session_age_secs: 600,
        };
        assert_eq!(m.corpus_prompt_decision(), CorpusPromptDecision::Silence);
    }

    #[test]
    fn not_reading_silences() {
        let m = Moment {
            hour: 14,
            time_class: TimeClass::Day,
            noise_floor: 0.005,
            voices_heard: 1,
            is_reading_paced: false,
            session_age_secs: 600,
        };
        assert_eq!(m.corpus_prompt_decision(), CorpusPromptDecision::Silence);
    }

    #[test]
    fn all_signals_align_returns_solo() {
        let m = Moment {
            hour: 14,
            time_class: TimeClass::Day,
            noise_floor: 0.005,
            voices_heard: 1,
            is_reading_paced: true,
            session_age_secs: 600,
        };
        assert_eq!(m.corpus_prompt_decision(), CorpusPromptDecision::SoloPrompt);
    }

    // ─── Suppression layer tests (v0.1.193) ──────────────────
    //
    // These exercise `apply_suppression`, the pure helper that
    // gates the raw classifier output on session state (cooldowns,
    // one-shot markers, sustainment counter). Pure function, so
    // no need to manipulate atomics.

    fn state(
        session_suppressed: bool,
        checkin_shown: bool,
        wink_shown: bool,
        prompt_cooldown_active: bool,
        noisy_polls_before: u32,
    ) -> SuppressionState {
        SuppressionState {
            session_suppressed,
            checkin_shown,
            wink_shown,
            prompt_cooldown_active,
            noisy_polls_before,
        }
    }

    #[test]
    fn session_suppressed_silences_everything() {
        // Even with a SoloPrompt + no cooldown, session-suppressed
        // returns Silence.
        let r = apply_suppression(
            CorpusPromptDecision::SoloPrompt,
            state(true, false, false, false, 0),
        );
        assert_eq!(r.effective, CorpusPromptDecision::Silence);
        // Counter reset when session is suppressed.
        assert_eq!(r.noisy_polls_after, 0);
    }

    #[test]
    fn session_suppressed_silences_wink_too() {
        let r = apply_suppression(
            CorpusPromptDecision::CafeWink,
            state(true, false, false, false, 0),
        );
        assert_eq!(r.effective, CorpusPromptDecision::Silence);
    }

    #[test]
    fn solo_passes_when_no_suppression() {
        let r = apply_suppression(
            CorpusPromptDecision::SoloPrompt,
            state(false, false, false, false, 0),
        );
        assert_eq!(r.effective, CorpusPromptDecision::SoloPrompt);
    }

    #[test]
    fn cooldown_silences_solo() {
        let r = apply_suppression(
            CorpusPromptDecision::SoloPrompt,
            state(false, false, false, true, 0),
        );
        assert_eq!(r.effective, CorpusPromptDecision::Silence);
    }

    #[test]
    fn cooldown_silences_coread() {
        // v0.1.189 fix: cooldown applies to CoreadPrompt too.
        // Pre-v0.1.189 only SoloPrompt was gated; this test pins
        // down the corrected behavior.
        let r = apply_suppression(
            CorpusPromptDecision::CoreadPrompt,
            state(false, false, false, true, 0),
        );
        assert_eq!(r.effective, CorpusPromptDecision::Silence);
    }

    #[test]
    fn cooldown_does_not_silence_checkin() {
        // The DeepNight check-in has its own one-shot suppression
        // (checkin_shown) — the prompt cooldown shouldn't gate it.
        let r = apply_suppression(
            CorpusPromptDecision::DeepNightCheckIn,
            state(false, false, false, true, 0),
        );
        assert_eq!(r.effective, CorpusPromptDecision::DeepNightCheckIn);
    }

    #[test]
    fn checkin_silenced_after_shown() {
        let r = apply_suppression(
            CorpusPromptDecision::DeepNightCheckIn,
            state(false, true, false, false, 0),
        );
        assert_eq!(r.effective, CorpusPromptDecision::Silence);
    }

    #[test]
    fn checkin_shown_does_not_affect_solo() {
        // SoloPrompt and DeepNightCheckIn have independent
        // suppression. Acknowledging the check-in doesn't silence
        // subsequent solo prompts during the daytime.
        let r = apply_suppression(
            CorpusPromptDecision::SoloPrompt,
            state(false, true, false, false, 0),
        );
        assert_eq!(r.effective, CorpusPromptDecision::SoloPrompt);
    }

    #[test]
    fn wink_requires_sustainment() {
        // First noisy tick: counter increments but stays below
        // threshold → Silence.
        let r = apply_suppression(
            CorpusPromptDecision::CafeWink,
            state(false, false, false, false, 0),
        );
        assert_eq!(r.effective, CorpusPromptDecision::Silence);
        assert_eq!(r.noisy_polls_after, 1);
    }

    #[test]
    fn wink_fires_at_sustainment_threshold() {
        // SUSTAINED_NOISY_TICKS = 2. Counter is 1 (one prior
        // noisy tick); this tick brings it to 2 → CafeWink.
        let r = apply_suppression(
            CorpusPromptDecision::CafeWink,
            state(false, false, false, false, 1),
        );
        assert_eq!(r.effective, CorpusPromptDecision::CafeWink);
        assert_eq!(r.noisy_polls_after, 2);
    }

    #[test]
    fn wink_silenced_after_shown_even_when_sustained() {
        // Past the threshold but wink already shown → Silence.
        let r = apply_suppression(
            CorpusPromptDecision::CafeWink,
            state(false, false, true, false, 5),
        );
        assert_eq!(r.effective, CorpusPromptDecision::Silence);
        // Counter still increments — irrelevant once wink_shown
        // but consistent with how the function operates.
        assert_eq!(r.noisy_polls_after, 6);
    }

    #[test]
    fn non_wink_resets_counter() {
        // If raw is anything other than CafeWink, the counter
        // resets to 0. Important: a noise spike followed by a
        // quiet moment should not preserve the count.
        let r = apply_suppression(
            CorpusPromptDecision::SoloPrompt,
            state(false, false, false, false, 5),
        );
        assert_eq!(r.effective, CorpusPromptDecision::SoloPrompt);
        assert_eq!(r.noisy_polls_after, 0);
    }

    #[test]
    fn silence_also_resets_counter() {
        // Silence from the raw classifier also resets the
        // sustainment counter.
        let r = apply_suppression(
            CorpusPromptDecision::Silence,
            state(false, false, false, false, 5),
        );
        assert_eq!(r.effective, CorpusPromptDecision::Silence);
        assert_eq!(r.noisy_polls_after, 0);
    }

    #[test]
    fn coread_with_no_cooldown_passes() {
        let r = apply_suppression(
            CorpusPromptDecision::CoreadPrompt,
            state(false, false, false, false, 0),
        );
        assert_eq!(r.effective, CorpusPromptDecision::CoreadPrompt);
    }

    #[test]
    fn wink_independent_of_checkin_state() {
        // Acknowledging the check-in shouldn't affect wink path.
        let r = apply_suppression(
            CorpusPromptDecision::CafeWink,
            state(false, true, false, false, 1),
        );
        assert_eq!(r.effective, CorpusPromptDecision::CafeWink);
    }

    #[test]
    fn checkin_independent_of_wink_state() {
        // Acknowledging the wink shouldn't affect check-in path.
        let r = apply_suppression(
            CorpusPromptDecision::DeepNightCheckIn,
            state(false, false, true, false, 0),
        );
        assert_eq!(r.effective, CorpusPromptDecision::DeepNightCheckIn);
    }

    #[test]
    fn cooldown_does_not_silence_wink() {
        // The prompt cooldown (set by accept/dismiss on solo/coread)
        // doesn't gate wink. Wink has its own one-shot suppression.
        let r = apply_suppression(
            CorpusPromptDecision::CafeWink,
            state(false, false, false, true, 1),
        );
        assert_eq!(r.effective, CorpusPromptDecision::CafeWink);
    }
}
