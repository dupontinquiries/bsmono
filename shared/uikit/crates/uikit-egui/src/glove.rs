//! glove — cold-weather glove mode: a cursor you steer by jostling the phone.
//!
//! Gloves do not work on capacitive glass. In cold weather the choice is
//! normally between bare fingers and not using the phone, and the second is
//! what people actually pick. This is the third option: a ball on the screen
//! that rolls with the device's tilt, snaps to whatever is under it, and is
//! driven the rest of the way by voice.
//!
//! ## What is here and what is the host's
//!
//! The physics, the snapping, the vocabulary and the gate are here and pure.
//! Three things are seams the host fills, because each is a daemon that already
//! exists and none of them belongs in a view:
//!
//! * **Temperature** — `ambient-learner`'s `EnvSnapshot::aux_temp_c`, a real
//!   ambient reading from a BME280-style sensor rather than a CPU thermal zone.
//! * **Orientation** — `iio-sensor-proxy`'s `AccelerometerOrientation`, the
//!   plain descriptor strings. Nothing else. See the ratchet below for why that
//!   is enough, and why an earlier version of this module was wrong to say it
//!   was not.
//! * **Speech** — the recogniser. `Vocab` maps a transcript to an action; it
//!   does not listen.
//!
//! ## The ratchet, which is the whole motion model
//!
//! The cursor does NOT roll under continuous acceleration. It ratchets: each
//! shake moves it one fixed step, and it will not move again until the phone
//! comes back to rest. Shake, return, shake, return — you traverse by
//! repeating, the way you walk a ball through a tilt maze.
//!
//! This matters for three reasons, and the first is the one that changed the
//! design:
//!
//! 1. **The four descriptor strings are enough.** A continuous model needs an
//!    acceleration vector, which `iio-sensor-proxy` does not expose — an
//!    earlier version of this module concluded from that it could not be built
//!    on the daemon and needed a sysfs reader. That was wrong. A ratchet needs
//!    only to know WHICH WAY the phone was turned and WHEN it came back, and
//!    "left-up" / "normal" carries both.
//! 2. **A gloved hand cannot aim a rolling ball.** Fixed steps land on a grid;
//!    momentum lands wherever the shaking hand left off.
//! 3. **Returning to rest is free.** Face-up and face-down do nothing except
//!    re-arm, so the return stroke costs the user nothing and cannot overshoot.
//!
//! ## Flick timing, which is what makes it work on a bus
//!
//! The step fires on the RETURN to rest, not on the way out, and only if the
//! phone was away for less than `FLICK_MAX_MS`. That is the whole defence
//! against a moving environment: on a train or while walking, the orientation
//! descriptor drifts through the same values a flick passes through, but it
//! drifts SLOWLY and it stays there. A lean is not a flick, and a lean that is
//! held is not a flick that was slow.
//!
//! Firing on the return costs one return-stroke of latency, and that is a trade
//! made on purpose. A step that fires on the way out cannot be un-fired when
//! the phone turns out to have been leaning rather than flicking, and an
//! accidental step is far worse than a late one for someone wearing gloves who
//! cannot easily correct it.
//!
//! `REARM_MIN_MS` is the other half: the phone must rest before it can fire
//! again. Descriptor chatter at a boundary — the reading flapping between
//! "normal" and "left-up" as the hand shakes — would otherwise ratchet several
//! steps from one motion. Holding a tilt never repeats either: one flick, one
//! step, however long the pose is held.
//!
//! ## Why the cursor snaps rather than clicking where it is
//!
//! A tilt-driven ball is not precise, and it is least precise exactly when it
//! is needed — a gloved hand on a cold morning is shaking. Requiring the ball
//! to be *inside* a target would make small controls unreachable. So the
//! nearest target within a generous radius wins, the ball visibly latches to
//! it, and "select" acts on the latched target rather than on the ball's
//! position. The user aims roughly and the screen does the last inch.
//!
//! ## The keyboard deliberately does not open
//!
//! Snapping to a text field arms VOICE input, not the on-screen keyboard.
//! rusty-board raising itself here would cover the screen with keys that cannot
//! be pressed by the hand that needed this mode — which is the whole problem
//! restated. `FieldArm::Voice` is what the host forwards, and rusty-board's
//! cue is its absence: no key event is ever synthesised.

/// Below this ambient temperature, glove mode is offered.
///
/// 8°C is roughly where gloves come out. It is a default rather than a truth —
/// people differ, and the setting is the user's.
pub const COLD_C: f32 = 8.0;

/// How far from the ball a target may be and still capture it.
///
/// Generous on purpose: a gloved hand on a cold morning is shaking, and a
/// strict radius makes small controls unreachable exactly when precision is
/// worst.
pub const SNAP_RADIUS_PX: f32 = 72.0;

/// How far one flick moves the cursor, in pixels.
///
/// A grid step rather than a distance: fixed steps land somewhere predictable,
/// which is the point of ratcheting instead of rolling.
pub const STEP_PX: f32 = 88.0;

/// Longest a phone may be away from rest and still count as a flick.
///
/// Above this it is a lean, and a lean on a bus is exactly what must not fire.
pub const FLICK_MAX_MS: f32 = 700.0;

/// How long the phone must be held screen-down to escape a listening field.
///
/// A HOLD rather than a flick, and that is the point — see `Tick::escaped`.
pub const ESCAPE_HOLD_MS: f32 = 600.0;

/// Shortest a phone must rest before another flick can fire.
///
/// Descriptor chatter at a boundary — the reading flapping between "normal" and
/// "left-up" as a shaking hand crosses it — would otherwise ratchet several
/// steps from one motion.
pub const REARM_MIN_MS: f32 = 120.0;

/// The descriptor as `iio-sensor-proxy` reports it.
///
/// Parsed from the string rather than a vector, which is all the daemon gives —
/// and all a ratchet needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orient {
    /// Upright, or flat on its back. REST: re-arms the ratchet.
    Rest,
    /// Screen down. Rest for ratchet purposes — a brief flip re-arms like any
    /// other — but HELD it is the escape, which is why it is its own variant.
    FaceDown,
    LeftUp,
    RightUp,
    BottomUp,
    /// The daemon could not tell. Neither fires nor re-arms — an unknown
    /// reading is not evidence of anything, and treating it as rest would let
    /// a phone tumbling in a pocket re-arm between flaps.
    Unknown,
}

impl Orient {
    /// Parse `AccelerometerOrientation` / `AccelerometerTilt`.
    ///
    /// Face-up and face-down are REST, per the maze game this follows: turning
    /// the phone flat does nothing except make the next flick possible.
    pub fn parse(s: &str) -> Orient {
        match s.trim().to_lowercase().as_str() {
            "normal" | "face-up" | "face_up" | "faceup" | "flat" => Orient::Rest,
            "face-down" | "face_down" | "facedown" => Orient::FaceDown,
            "left-up" | "left_up" | "leftup" => Orient::LeftUp,
            "right-up" | "right_up" | "rightup" => Orient::RightUp,
            "bottom-up" | "bottom_up" | "bottomup" => Orient::BottomUp,
            _ => Orient::Unknown,
        }
    }

    pub fn is_rest(self) -> bool { matches!(self, Orient::Rest | Orient::FaceDown) }

    /// The step this flick would make, in (dx, dy) units.
    ///
    /// Left-up means the left edge lifted — the phone rotated anticlockwise —
    /// and the ball runs downhill, to the left. Bottom-up lifts the bottom
    /// edge, so the ball runs up the screen.
    pub fn direction(self) -> Option<(f32, f32)> {
        match self {
            Orient::LeftUp => Some((-1.0, 0.0)),
            Orient::RightUp => Some((1.0, 0.0)),
            Orient::BottomUp => Some((0.0, -1.0)),
            Orient::Rest | Orient::FaceDown | Orient::Unknown => None,
        }
    }
}

/// The flick ratchet: one step per out-and-back, gated on timing.
#[derive(Debug, Clone, PartialEq)]
pub struct Ratchet {
    /// Where the phone is now.
    at: Orient,
    /// How long it has been there.
    dwell_ms: f32,
    /// The direction it left rest in, if it is away.
    away_in: Option<Orient>,
    /// True once it has rested long enough to fire again.
    armed: bool,
}

impl Default for Ratchet {
    fn default() -> Self {
        // Starts UNARMED: the first reading after switching the mode on should
        // not fire a step just because the phone happens to be tilted.
        Ratchet { at: Orient::Unknown, dwell_ms: 0.0, away_in: None, armed: false }
    }
}

impl Ratchet {
    pub fn armed(&self) -> bool { self.armed }
    pub fn at(&self) -> Orient { self.at }
    pub fn away(&self) -> bool { self.away_in.is_some() }

    /// True once the phone has been held screen-down long enough to escape.
    pub fn escaping(&self) -> bool {
        self.at == Orient::FaceDown && self.dwell_ms >= ESCAPE_HOLD_MS
    }

    /// Feed a descriptor reading. Returns the ORIENTATION that was flicked, on
    /// the frame the flick completes.
    ///
    /// Returns the orientation rather than a vector so both consumers can use
    /// one event: the cursor turns it into a step, the command layer matches it
    /// against a binding. Two detectors watching the same descriptor stream
    /// independently would be a second source of truth about what a flick is.
    pub fn feed(&mut self, o: Orient, dt_ms: f32) -> Option<Orient> {
        if o == self.at {
            self.dwell_ms += dt_ms;
            // Resting long enough re-arms; being AWAY long enough turns the
            // flick into a lean, which will never fire.
            if self.at.is_rest() && self.dwell_ms >= REARM_MIN_MS { self.armed = true; }
            if self.away_in.is_some() && self.dwell_ms > FLICK_MAX_MS { self.away_in = None; }
            return None;
        }

        // An unknown reading changes nothing: it is not evidence of rest or of
        // a direction, and letting it re-arm would let a tumbling phone ratchet.
        if o == Orient::Unknown {
            self.at = o;
            self.dwell_ms = 0.0;
            return None;
        }

        let was = self.at;
        let was_dwell = self.dwell_ms;
        self.at = o;
        self.dwell_ms = 0.0;

        if o.direction().is_some() {
            // Leaving rest. Only records the intent — the step fires on the way
            // back, when we know how long it took.
            if was.is_rest() && self.armed { self.away_in = Some(o); }
            return None;
        }

        // Arrived at rest. If we left rest in a direction and got back inside
        // the flick window, that was a flick.
        if o.is_rest() {
            let fired = self.away_in.take().filter(|_| was_dwell <= FLICK_MAX_MS);
            self.armed = false;   // must rest REARM_MIN_MS before the next one
            return fired;
        }
        None
    }
}

/// Longest gap between the two halves of a double flick.
///
/// A double flick is a deliberate command, so the window can be generous — but
/// not so generous that two ordinary cursor steps in the same direction become
/// one. 550ms is comfortably longer than a shake-and-return and comfortably
/// shorter than two considered moves.
pub const DOUBLE_FLICK_MS: f32 = 550.0;

/// Recognises two flicks in the same direction as one command.
///
/// Sits ON TOP of the ratchet rather than beside it, so there is one source of
/// truth about what a flick is. Single flicks still move the cursor; the double
/// is what carries app commands, which is the split that keeps them from
/// fighting: "new tab" cannot be produced by someone walking a cursor across
/// the screen, because that is single flicks with rests between.
///
/// The cost is honest and worth stating: the FIRST flick of a double has
/// already moved the cursor by the time the second arrives. A caller that binds
/// both must undo the step, or bind doubles only in contexts where the cursor
/// is not in play — which is what the app bindings here do.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DoubleFlick {
    last: Option<(Orient, f32)>,
    since_ms: f32,
}

impl DoubleFlick {
    /// Feed the ratchet's output. Returns the direction on the second flick.
    pub fn feed(&mut self, flick: Option<Orient>, dt_ms: f32) -> Option<Orient> {
        self.since_ms += dt_ms;
        if let Some((_, _)) = self.last {
            if self.since_ms > DOUBLE_FLICK_MS { self.last = None; }
        }
        let Some(dir) = flick else { return None };
        match self.last.take() {
            // Same direction, inside the window: that is the command.
            Some((prev, _)) if prev == dir => {
                // Cleared rather than kept, so a THIRD flick starts a fresh
                // pair instead of firing again immediately.
                self.since_ms = 0.0;
                Some(dir)
            }
            // A different direction restarts the pair rather than being
            // ignored: someone who flicks left then right has changed their
            // mind, not made a command.
            _ => {
                self.last = Some((dir, 0.0));
                self.since_ms = 0.0;
                None
            }
        }
    }
}

/// An app command bound to a double flick.
///
/// The binding lives with the app, not here: "new tab" means nothing to
/// snapclone and "swap camera" means nothing to a terminal. What is shared is
/// the recognition and the settings gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShakeBinding {
    pub dir: Orient,
    /// The app's own action id.
    pub action: String,
    /// Shown in settings, so a user can find out what the gesture does without
    /// performing it.
    pub label: String,
}

/// The app-command layer: a double-flick recogniser plus a binding table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShakeCommands {
    /// Independent of glove mode. Someone may want shake-for-new-tab without
    /// wanting a cursor, and someone in gloves may want the cursor without
    /// having their tabs multiply.
    pub enabled: bool,
    detector: DoubleFlick,
    bindings: Vec<ShakeBinding>,
}

impl ShakeCommands {
    pub fn with(bindings: Vec<ShakeBinding>) -> Self {
        ShakeCommands { enabled: false, detector: DoubleFlick::default(), bindings }
    }

    pub fn bindings(&self) -> &[ShakeBinding] { &self.bindings }

    /// Feed the ratchet's flick output. Returns an action id on a double.
    pub fn feed(&mut self, flick: Option<Orient>, dt_ms: f32) -> Option<String> {
        if !self.enabled { return None; }
        let dir = self.detector.feed(flick, dt_ms)?;
        self.bindings.iter().find(|b| b.dir == dir).map(|b| b.action.clone())
    }

    /// What a direction is bound to, for the settings list.
    pub fn describe(&self, dir: Orient) -> Option<&str> {
        self.bindings.iter().find(|b| b.dir == dir).map(|b| b.label.as_str())
    }
}

/// What kind of thing the cursor can land on. The action vocabulary differs per
/// kind, because "louder" means nothing to a checkbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Button,
    /// A text field. Snapping here arms voice, never the keyboard.
    Field,
    /// A row in a list.
    ListItem,
    Slider,
    Toggle,
}

/// Something the cursor can snap to.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub id: String,
    pub kind: TargetKind,
    /// Centre, in screen coordinates.
    pub at: (f32, f32),
    /// Spoken label, for "select the volume slider" and for the readout.
    pub label: String,
}

/// What the mode asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum GloveAction {
    Activate(String),
    /// Arm voice entry for a field. NOT a request to raise a keyboard.
    ArmField(String),
    Toggle(String),
    /// Nudge a slider by a fraction of its range, positive or negative.
    Nudge { id: String, delta: f32 },
    /// Move the snap to the next/previous target — for lists, where rolling to
    /// each row is slower than saying "next".
    Step(i32),
    Dismiss,
    /// Text spoken into an armed field. NOT a command — see `Mode::Dictating`.
    Dictated(String),
    /// The user held the phone screen-down to leave a listening field.
    Escaped,
}

/// What the cursor is doing with speech.
///
/// The distinction is not cosmetic. While a field is armed, EVERY SPOKEN WORD
/// IS CONTENT: "next" is a word someone might dictate into a message, and
/// resolving it as a command would jump the cursor and silently swallow the
/// word. So the vocabulary is SUSPENDED while dictating, and the only way out
/// is out-of-band.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Navigating,
    Dictating { field: String },
}

/// A confirmation the user must answer by NAMING the action.
///
/// Confirmations are where an accidental step is worst, and glove mode makes
/// every accident more likely: the recogniser is hearing a person through a
/// scarf, and the cursor is being aimed by shaking a phone. Two rules follow.
///
/// **"Yes" is not accepted.** It is one short syllable, it is what a
/// half-heard "yes?" or a background conversation sounds like, and it carries
/// no information about WHAT is being agreed to. The user must say the verb —
/// "delete", "leave", "send" — which is longer, more distinctive to a
/// recogniser, and impossible to say by accident in answer to the wrong prompt.
///
/// **The safe choice is where the cursor starts.** As with the context menu,
/// nothing destructive is pre-selected, so a stray flick or a misheard "select"
/// lands on Cancel.
#[derive(Debug, Clone, PartialEq)]
pub struct Confirm {
    pub prompt: String,
    /// The verb that confirms — "delete", not "yes".
    pub verb: String,
    /// True when going ahead cannot be undone.
    pub destructive: bool,
}

/// What a spoken answer to a confirmation resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Confirmed,
    Cancelled,
    /// Heard, but not an answer. The prompt stays up.
    Unclear,
}

impl Confirm {
    pub fn new(prompt: &str, verb: &str, destructive: bool) -> Self {
        Confirm { prompt: prompt.into(), verb: verb.trim().to_lowercase(), destructive }
    }

    /// Resolve a spoken answer.
    pub fn answer(&self, phrase: &str) -> Answer {
        let p = phrase.trim().to_lowercase();
        if p.is_empty() { return Answer::Unclear; }
        // Cancelling is easy on purpose: several ways to say it, all accepted.
        // Backing out of a destructive prompt should never require precision.
        if matches!(p.as_str(),
            "cancel" | "no" | "stop" | "never mind" | "nevermind" | "go back" | "back") {
            return Answer::Cancelled;
        }
        // Confirming requires the verb. "yes" is refused even here.
        if p == self.verb || p == format!("{} it", self.verb) {
            return Answer::Confirmed;
        }
        Answer::Unclear
    }

    /// What the prompt should read out, so the verb is discoverable — a user
    /// who does not know the word cannot say it.
    pub fn readout(&self) -> String {
        format!("{} — say \"{}\" to confirm, or \"cancel\"", self.prompt, self.verb)
    }
}

/// Maps a transcript to an action against the current target.
///
/// Deliberately small and literal. A big grammar fails in the cold, where the
/// user is speaking through a scarf into a phone at arm's length, and a
/// recogniser that needs a clean sentence is a recogniser that needs bare
/// hands.
pub struct Vocab;

impl Vocab {
    /// Words that act on whatever is snapped, whatever its kind.
    pub fn universal(phrase: &str, target: &Target) -> Option<GloveAction> {
        match phrase {
            "select" | "tap" | "press" | "go" | "okay" | "ok" =>
                Some(match target.kind {
                    // Selecting a FIELD arms voice; it does not "press" it.
                    TargetKind::Field => GloveAction::ArmField(target.id.clone()),
                    TargetKind::Toggle => GloveAction::Toggle(target.id.clone()),
                    _ => GloveAction::Activate(target.id.clone()),
                }),
            "next" | "down" => Some(GloveAction::Step(1)),
            "back" | "previous" | "up" => Some(GloveAction::Step(-1)),
            "cancel" | "dismiss" | "stop" | "never mind" => Some(GloveAction::Dismiss),
            _ => None,
        }
    }

    /// Words that only mean something for a particular kind.
    pub fn resolve(phrase: &str, target: &Target) -> Option<GloveAction> {
        let p = phrase.trim().to_lowercase();
        if let Some(a) = Self::universal(&p, target) { return Some(a); }
        match target.kind {
            TargetKind::Slider => match p.as_str() {
                // Coarse and fine, because "a bit" is how people talk about
                // volume and "set it to 47" is not.
                "up" | "louder" | "more" | "higher" =>
                    Some(GloveAction::Nudge { id: target.id.clone(), delta: 0.10 }),
                "down" | "quieter" | "less" | "lower" =>
                    Some(GloveAction::Nudge { id: target.id.clone(), delta: -0.10 }),
                "a bit more" | "nudge up" =>
                    Some(GloveAction::Nudge { id: target.id.clone(), delta: 0.02 }),
                "a bit less" | "nudge down" =>
                    Some(GloveAction::Nudge { id: target.id.clone(), delta: -0.02 }),
                "max" | "maximum" | "all the way up" =>
                    Some(GloveAction::Nudge { id: target.id.clone(), delta: 1.0 }),
                "off" | "mute" | "all the way down" =>
                    Some(GloveAction::Nudge { id: target.id.clone(), delta: -1.0 }),
                _ => None,
            },
            TargetKind::Toggle => match p.as_str() {
                "on" | "enable" | "turn on" | "off" | "disable" | "turn off" =>
                    Some(GloveAction::Toggle(target.id.clone())),
                _ => None,
            },
            _ => None,
        }
    }
}

/// What one orientation reading produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tick {
    pub snapped: Option<String>,
    /// The direction flicked this frame, for a command layer to consume. Passed
    /// out rather than handled here so the cursor does not have to know what an
    /// app command is.
    pub flicked: Option<Orient>,
    /// The phone was held screen-down long enough to leave a listening field.
    pub escaped: bool,
}

/// How glove mode decides whether to be on.
///
/// Three states rather than a bool plus a threshold, because "manual on" and
/// "auto, currently cold" are different intents and a bool cannot tell them
/// apart. Someone who has switched it on deliberately — indoors, with an injury,
/// wearing gloves for a reason the thermometer knows nothing about — must not
/// have it turned off by a warm reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColdMode {
    /// Never.
    Off,
    /// Always, whatever the weather. The thermometer is not consulted.
    On,
    /// On when the ambient reading is at or below the threshold.
    Auto { threshold_c: f32 },
}

impl Default for ColdMode {
    fn default() -> Self { ColdMode::Auto { threshold_c: COLD_C } }
}

impl ColdMode {
    /// Should the mode be active, given what the thermometer says?
    ///
    /// `ambient_c` of `None` means no reading. In `Auto` that is NOT cold — a
    /// phone with no ambient sensor would otherwise show a cursor indoors in
    /// July — but in `On` it is irrelevant, which is the point of having the
    /// state at all.
    pub fn active(self, ambient_c: Option<f32>) -> bool {
        match self {
            ColdMode::Off => false,
            ColdMode::On => true,
            ColdMode::Auto { threshold_c } =>
                ambient_c.map(|c| c <= threshold_c).unwrap_or(false),
        }
    }

    /// What the settings row should say, so the state is legible without
    /// opening it.
    pub fn summary(self, ambient_c: Option<f32>) -> String {
        match self {
            ColdMode::Off => "Off".into(),
            ColdMode::On => "On".into(),
            ColdMode::Auto { threshold_c } => match ambient_c {
                // Naming the reading matters: "Auto (off)" with no number
                // leaves the user unable to tell a warm day from a dead sensor.
                Some(c) => format!("Auto — {c:.0}°C, on below {threshold_c:.0}°C"),
                None => format!("Auto — no reading, on below {threshold_c:.0}°C"),
            },
        }
    }
}

/// The ratcheting cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct GloveCursor {
    /// Off / On / Auto with a threshold — see `ColdMode`.
    pub mode_setting: ColdMode,
    /// Last ambient temperature the host reported.
    pub ambient_c: Option<f32>,
    pos: (f32, f32),
    /// The flick ratchet. No velocity: the cursor does not coast.
    ratchet: Ratchet,
    mode: Mode,
    snapped: Option<String>,
    bounds: (f32, f32),
}

impl Default for GloveCursor {
    fn default() -> Self {
        GloveCursor {
            mode_setting: ColdMode::Off, ambient_c: None,
            pos: (0.0, 0.0), ratchet: Ratchet::default(), mode: Mode::Navigating,
            snapped: None, bounds: (0.0, 0.0),
        }
    }
}

impl GloveCursor {
    /// Build with a setting. Needed because the internals are private, and a
    /// caller in another crate cannot reach for struct-update syntax — which is
    /// the right way round: the ratchet and the cursor position are not things
    /// an app should set.
    pub fn with_mode(mode_setting: ColdMode) -> Self {
        GloveCursor { mode_setting, ..Default::default() }
    }

    /// Is the mode on right now?
    pub fn active(&self) -> bool { self.mode_setting.active(self.ambient_c) }

    /// Is the thermometer the reason? Used by the settings summary and by
    /// anything that wants to explain itself.
    pub fn is_cold(&self) -> bool {
        matches!(self.mode_setting, ColdMode::Auto { .. }) && self.active()
    }

    pub fn set_bounds(&mut self, w: f32, h: f32) {
        self.bounds = (w, h);
        if self.pos == (0.0, 0.0) { self.pos = (w * 0.5, h * 0.5); }
    }

    pub fn pos(&self) -> (f32, f32) { self.pos }
    pub fn snapped(&self) -> Option<&str> { self.snapped.as_deref() }
    /// Is the ratchet ready to fire? Shown in the UI, because a user shaking a
    /// phone that will not respond needs to know it is waiting for rest.
    pub fn armed(&self) -> bool { self.ratchet.armed() }

    pub fn mode(&self) -> &Mode { &self.mode }
    pub fn dictating(&self) -> bool { matches!(self.mode, Mode::Dictating { .. }) }

    /// Arm a field for voice entry. Suspends the vocabulary.
    pub fn arm_field(&mut self, id: &str) {
        self.mode = Mode::Dictating { field: id.to_string() };
    }

    /// Leave dictation. Returns true if it was actually listening.
    pub fn escape(&mut self) -> bool {
        let was = self.dictating();
        self.mode = Mode::Navigating;
        was
    }

    /// Feed one orientation reading.
    ///
    /// `o` is `AccelerometerOrientation` parsed by `Orient::parse` — the daemon
    /// string, nothing more.
    pub fn feed(&mut self, o: Orient, dt_ms: f32, targets: &[Target]) -> Tick {
        if !self.active() { return Tick::default(); }
        let mut out = Tick::default();

        // The escape is checked FIRST and works in either mode. It has to be
        // reachable from dictation, which is the state it exists to leave.
        let flicked = self.ratchet.feed(o, dt_ms);
        out.flicked = flicked;
        if self.ratchet.escaping() {
            out.escaped = self.escape();
        }

        // While dictating the cursor does not move. A phone held to the mouth
        // sways, and a cursor that wandered off the field during dictation
        // would deliver the text somewhere else.
        if self.dictating() {
            out.snapped = self.snapped.clone();
            return out;
        }

        if let Some((dx, dy)) = flicked.and_then(|f| f.direction()) {
            // A fixed step, clamped to the screen. No momentum: the cursor
            // arrives where it was sent and stays there, which is what makes it
            // aimable with a shaking hand.
            self.pos.0 = (self.pos.0 + dx * STEP_PX).clamp(0.0, self.bounds.0);
            self.pos.1 = (self.pos.1 + dy * STEP_PX).clamp(0.0, self.bounds.1);
        }
        self.snapped = self.nearest(targets).map(|t| t.id.clone());
        out.snapped = self.snapped.clone();
        out
    }

    /// The nearest target within the snap radius.
    pub fn nearest<'a>(&self, targets: &'a [Target]) -> Option<&'a Target> {
        targets.iter()
            .map(|t| (t, (t.at.0 - self.pos.0).hypot(t.at.1 - self.pos.1)))
            .filter(|(_, d)| *d <= SNAP_RADIUS_PX)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(t, _)| t)
    }

    /// Move the snap by hand — "next" and "back", which beat rolling to each
    /// row of a list.
    pub fn step(&mut self, targets: &[Target], delta: i32) -> Option<String> {
        if targets.is_empty() { return None; }
        let cur = self.snapped.as_ref()
            .and_then(|id| targets.iter().position(|t| &t.id == id));
        let n = targets.len() as i32;
        let next = match cur {
            Some(i) => (i as i32 + delta).rem_euclid(n),
            // With nothing snapped, "next" means the first — not a no-op the
            // user has to guess at.
            None => if delta >= 0 { 0 } else { n - 1 },
        } as usize;
        // Stepping moves the BALL too, so the cursor stays where the attention
        // is rather than leaving a latched highlight somewhere off-ball.
        self.pos = targets[next].at;
        self.snapped = Some(targets[next].id.clone());
        self.snapped.clone()
    }

    /// Resolve a spoken phrase against whatever is snapped.
    pub fn speak(&mut self, phrase: &str, targets: &[Target]) -> Option<GloveAction> {
        // Dictating: the vocabulary is suspended and the words are content.
        // Resolving "next" here would jump the cursor and swallow the word.
        if self.dictating() {
            if phrase.trim().is_empty() { return None; }
            return Some(GloveAction::Dictated(phrase.to_string()));
        }
        let target = targets.iter().find(|t| Some(t.id.as_str()) == self.snapped())?;
        let action = Vocab::resolve(phrase, target)?;
        if let GloveAction::Step(d) = action {
            self.step(targets, d);
        }
        if let GloveAction::ArmField(id) = &action {
            self.arm_field(id);
        }
        Some(action)
    }

    /// What the readout says, so the user can hear-check the snap before
    /// speaking. Without it, "select" on a shaking screen is a guess.
    pub fn readout(&self, targets: &[Target]) -> String {
        match targets.iter().find(|t| Some(t.id.as_str()) == self.snapped()) {
            Some(t) => t.label.clone(),
            None => "nothing selected".into(),
        }
    }
}

/// Draw the ball and its snap ring.
pub fn glove_ui(ui: &mut egui::Ui, g: &GloveCursor, targets: &[Target],
                c: &crate::PaletteColors) {
    if !g.active() { return; }
    let p = ui.painter();
    // The snap ring is drawn on the TARGET, not the ball: the user needs to
    // know what will be acted on, and the ball is the thing they are already
    // watching.
    if let Some(t) = targets.iter().find(|t| Some(t.id.as_str()) == g.snapped()) {
        p.circle_stroke(egui::pos2(t.at.0, t.at.1), 26.0,
                        egui::Stroke::new(2.0, c.accent));
    }
    let at = egui::pos2(g.pos().0, g.pos().1);
    p.circle_filled(at, 14.0, c.accent);
    p.circle_stroke(at, 14.0, egui::Stroke::new(2.0, c.bg_base));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: &str, kind: TargetKind, x: f32, y: f32) -> Target {
        Target { id: id.into(), kind, at: (x, y), label: id.into() }
    }

    fn cold_cursor() -> GloveCursor {
        let mut g = GloveCursor { mode_setting: ColdMode::Auto { threshold_c: COLD_C }, ambient_c: Some(2.0), ..Default::default() };
        g.set_bounds(400.0, 800.0);
        g
    }

    #[test]
    fn an_unknown_temperature_is_not_cold() {
        // A phone with no ambient sensor would otherwise roll a ball across the
        // screen indoors in July, and a mode that appears uninvited is one
        // people switch off permanently.
        let mut g = GloveCursor { mode_setting: ColdMode::Auto { threshold_c: COLD_C }, ..Default::default() };
        assert!(!g.is_cold());
        assert!(!g.active());
        g.ambient_c = Some(20.0);
        assert!(!g.active(), "and a warm reading is not cold either");
    }

    #[test]
    fn manual_on_ignores_the_thermometer_entirely() {
        // The reason this is three states and not a bool plus a threshold.
        // Someone who switched it on deliberately — indoors, with an injury,
        // wearing gloves for a reason the thermometer knows nothing about —
        // must not have it turned off by a warm reading.
        let warm = GloveCursor { mode_setting: ColdMode::On, ambient_c: Some(28.0),
                                 ..Default::default() };
        assert!(warm.active());
        assert!(!warm.is_cold(), "on, but not BECAUSE it is cold");

        let no_sensor = GloveCursor { mode_setting: ColdMode::On, ambient_c: None,
                                      ..Default::default() };
        assert!(no_sensor.active(), "and a missing sensor is irrelevant here");
    }

    #[test]
    fn off_means_off_however_cold_it_is() {
        let g = GloveCursor { mode_setting: ColdMode::Off, ambient_c: Some(-20.0),
                              ..Default::default() };
        assert!(!g.active());
    }

    #[test]
    fn auto_uses_the_threshold_the_user_chose() {
        // A custom threshold is the point: people differ about what is cold,
        // and 8 is a default rather than a truth.
        let chilly = ColdMode::Auto { threshold_c: 15.0 };
        assert!(chilly.active(Some(12.0)), "12 is cold if you said 15");
        assert!(!chilly.active(Some(18.0)));
        let hardy = ColdMode::Auto { threshold_c: 0.0 };
        assert!(!hardy.active(Some(5.0)), "5 is not cold if you said 0");
    }

    #[test]
    fn auto_with_no_reading_is_not_cold() {
        // A phone with no ambient sensor would otherwise show a cursor indoors
        // in July, and a mode that appears uninvited is switched off for good.
        assert!(!ColdMode::Auto { threshold_c: COLD_C }.active(None));
    }

    #[test]
    fn the_settings_summary_names_the_reading_not_just_the_state() {
        // "Auto (off)" with no number leaves the user unable to tell a warm day
        // from a dead sensor.
        let m = ColdMode::Auto { threshold_c: 8.0 };
        assert!(m.summary(Some(14.0)).contains("14"));
        assert!(m.summary(None).contains("no reading"));
        assert_eq!(ColdMode::Off.summary(Some(1.0)), "Off");
    }

    /// Complete a flick: rest, dwell, leave in `dir`, return.
    ///
    /// Two rest readings because arriving at rest clears the arm and the dwell
    /// restores it — which is what a real poller emits.
    fn flick(g: &mut GloveCursor, dir: Orient, away_ms: f32, ts: &[Target]) {
        g.feed(Orient::Rest, 0.0, ts);
        g.feed(Orient::Rest, REARM_MIN_MS, ts);
        g.feed(dir, 0.0, ts);
        g.feed(dir, away_ms, ts);
        g.feed(Orient::Rest, 0.0, ts);
    }

    #[test]
    fn an_inactive_cursor_does_not_move() {
        let mut g = GloveCursor { mode_setting: ColdMode::Off, ambient_c: Some(0.0), ..Default::default() };
        g.set_bounds(400.0, 800.0);
        let before = g.pos();
        flick(&mut g, Orient::LeftUp, 200.0, &[]);
        assert_eq!(g.pos(), before);
    }

    #[test]
    fn the_descriptor_strings_are_all_this_needs() {
        // An earlier version of this module concluded the daemon could not
        // drive it because it exposes no vector. A ratchet needs only which way
        // and when, and these strings carry both.
        assert_eq!(Orient::parse("left-up"), Orient::LeftUp);
        assert_eq!(Orient::parse("RIGHT_UP"), Orient::RightUp);
        assert_eq!(Orient::parse("bottom-up"), Orient::BottomUp);
        assert_eq!(Orient::parse("normal"), Orient::Rest);
        assert_eq!(Orient::parse("nonsense"), Orient::Unknown);
    }

    #[test]
    fn face_up_and_face_down_are_rest_and_do_nothing_but_re_arm() {
        // Per the maze game: turning the phone flat does not move the ball, it
        // makes the next flick possible.
        for flat in ["face-up", "face-down", "flat", "normal"] {
            assert!(Orient::parse(flat).is_rest(), "{flat}");
            assert_eq!(Orient::parse(flat).direction(), None);
        }
    }

    #[test]
    fn one_flick_moves_exactly_one_step() {
        let mut g = cold_cursor();
        let before = g.pos();
        flick(&mut g, Orient::LeftUp, 200.0, &[]);
        assert_eq!(g.pos().0, before.0 - STEP_PX);
        assert_eq!(g.pos().1, before.1, "and not vertically");
    }

    #[test]
    fn shaking_repeatedly_traverses() {
        // The whole point: you walk the cursor across the screen by repeating.
        // Two steps, deliberately: a third would hit the right edge of the
        // 400px test screen and be clamped, which the wall test covers
        // separately.
        let mut g = cold_cursor();
        let before = g.pos();
        for _ in 0..2 { flick(&mut g, Orient::RightUp, 150.0, &[]); }
        assert_eq!(g.pos().0, before.0 + 2.0 * STEP_PX);
    }

    #[test]
    fn holding_a_tilt_never_repeats() {
        // A held pose turning into a runaway scroll is unrecoverable for
        // someone who cannot easily tap.
        let mut g = cold_cursor();
        let before = g.pos();
        g.feed(Orient::Rest, 0.0, &[]);
        g.feed(Orient::Rest, REARM_MIN_MS, &[]);
        g.feed(Orient::LeftUp, 0.0, &[]);
        for _ in 0..100 { g.feed(Orient::LeftUp, 16.0, &[]); }
        assert_eq!(g.pos(), before, "no step while held");
    }

    #[test]
    fn a_slow_lean_is_not_a_flick() {
        // THE reason to gate on timing. On a bus the descriptor drifts through
        // the same values a flick passes through — slowly, and it stays there.
        let mut g = cold_cursor();
        let before = g.pos();
        flick(&mut g, Orient::LeftUp, FLICK_MAX_MS + 100.0, &[]);
        assert_eq!(g.pos(), before, "held too long to be a flick");
    }

    #[test]
    fn a_flick_just_inside_the_window_still_counts() {
        let mut g = cold_cursor();
        let before = g.pos();
        flick(&mut g, Orient::LeftUp, FLICK_MAX_MS - 1.0, &[]);
        assert_eq!(g.pos().0, before.0 - STEP_PX);
    }

    #[test]
    fn the_step_fires_on_the_return_not_on_the_way_out() {
        // It cannot be un-fired, so it waits until the timing is known. An
        // accidental step is worse than a late one for someone in gloves.
        let mut g = cold_cursor();
        let before = g.pos();
        g.feed(Orient::Rest, 0.0, &[]);
        g.feed(Orient::Rest, REARM_MIN_MS, &[]);
        g.feed(Orient::LeftUp, 0.0, &[]);
        g.feed(Orient::LeftUp, 100.0, &[]);
        assert_eq!(g.pos(), before, "still out");
        g.feed(Orient::Rest, 0.0, &[]);
        assert_eq!(g.pos().0, before.0 - STEP_PX, "fired on the return");
    }

    #[test]
    fn chatter_at_the_boundary_does_not_ratchet_several_steps() {
        // The reading flapping between normal and left-up as a shaking hand
        // crosses the boundary is one motion, not four.
        let mut g = cold_cursor();
        let before = g.pos();
        g.feed(Orient::Rest, 0.0, &[]);
        g.feed(Orient::Rest, REARM_MIN_MS, &[]);
        for _ in 0..4 {
            g.feed(Orient::LeftUp, 10.0, &[]);
            g.feed(Orient::Rest, 10.0, &[]);   // far shorter than REARM_MIN_MS
        }
        assert_eq!(g.pos().0, before.0 - STEP_PX, "one step, not four");
    }

    #[test]
    fn an_unknown_reading_neither_fires_nor_re_arms() {
        // A phone tumbling in a pocket must not ratchet between flaps.
        let mut g = cold_cursor();
        let before = g.pos();
        g.feed(Orient::Rest, 0.0, &[]);
        g.feed(Orient::Rest, REARM_MIN_MS, &[]);
        g.feed(Orient::Unknown, 500.0, &[]);
        g.feed(Orient::LeftUp, 0.0, &[]);
        g.feed(Orient::Rest, 0.0, &[]);
        assert_eq!(g.pos(), before, "the flick did not start from a known rest");
    }

    #[test]
    fn it_starts_unarmed_so_switching_the_mode_on_while_tilted_does_nothing() {
        let mut g = cold_cursor();
        let before = g.pos();
        g.feed(Orient::LeftUp, 0.0, &[]);
        g.feed(Orient::Rest, 0.0, &[]);
        assert_eq!(g.pos(), before);
    }

    #[test]
    fn the_cursor_stops_at_the_screen_edge_and_stays_there() {
        // No momentum to carry it off, and no bounce to chase.
        let mut g = cold_cursor();
        for _ in 0..20 { flick(&mut g, Orient::LeftUp, 100.0, &[]); }
        assert_eq!(g.pos().0, 0.0);
        flick(&mut g, Orient::RightUp, 100.0, &[]);
        assert_eq!(g.pos().0, STEP_PX, "and comes straight back");
    }

    #[test]
    fn bottom_up_runs_the_cursor_up_the_screen() {
        let mut g = cold_cursor();
        let before = g.pos();
        flick(&mut g, Orient::BottomUp, 150.0, &[]);
        assert_eq!(g.pos().1, before.1 - STEP_PX);
    }

    #[test]
    fn the_nearest_target_within_the_radius_wins() {
        let mut g = cold_cursor();
        let ts = vec![target("far", TargetKind::Button, 200.0, 500.0),
                      target("near", TargetKind::Button, 200.0, 410.0)];
        g.feed(Orient::Rest, 16.0, &ts);
        assert_eq!(g.snapped(), Some("near"));
    }

    #[test]
    fn nothing_outside_the_radius_captures_the_ball() {
        let mut g = cold_cursor();
        let ts = vec![target("miles", TargetKind::Button, 20.0, 20.0)];
        assert_eq!(g.feed(Orient::Rest, 16.0, &ts).snapped, None);
        assert_eq!(g.readout(&ts), "nothing selected");
    }

    #[test]
    fn the_snap_radius_is_generous_because_a_cold_hand_shakes() {
        // A strict radius makes small controls unreachable exactly when
        // precision is worst.
        assert!(SNAP_RADIUS_PX >= 60.0);
    }

    #[test]
    fn selecting_a_field_arms_voice_and_never_a_keyboard() {
        // rusty-board raising itself here would cover the screen with keys that
        // cannot be pressed by the hand that needed this mode.
        let mut g = cold_cursor();
        let ts = vec![target("name", TargetKind::Field, 200.0, 400.0)];
        g.feed(Orient::Rest, 16.0, &ts);
        assert_eq!(g.speak("select", &ts), Some(GloveAction::ArmField("name".into())));
        // Nothing in the action vocabulary can ask for a keyboard.
        let dbg = format!("{:?}", g.speak("press", &ts));
        assert!(!dbg.to_lowercase().contains("keyboard"));
    }

    #[test]
    fn selecting_a_button_activates_it() {
        let mut g = cold_cursor();
        let ts = vec![target("send", TargetKind::Button, 200.0, 400.0)];
        g.feed(Orient::Rest, 16.0, &ts);
        assert_eq!(g.speak("go", &ts), Some(GloveAction::Activate("send".into())));
    }

    #[test]
    fn a_toggle_flips_on_select_and_on_its_own_words() {
        let mut g = cold_cursor();
        let ts = vec![target("wifi", TargetKind::Toggle, 200.0, 400.0)];
        g.feed(Orient::Rest, 16.0, &ts);
        assert_eq!(g.speak("select", &ts), Some(GloveAction::Toggle("wifi".into())));
        assert_eq!(g.speak("turn off", &ts), Some(GloveAction::Toggle("wifi".into())));
    }

    #[test]
    fn a_slider_takes_coarse_and_fine_nudges_because_that_is_how_people_talk() {
        // "a bit more" is a real thing to say about volume; "set it to 47" is
        // not.
        let mut g = cold_cursor();
        let ts = vec![target("vol", TargetKind::Slider, 200.0, 400.0)];
        g.feed(Orient::Rest, 16.0, &ts);
        assert_eq!(g.speak("louder", &ts),
                   Some(GloveAction::Nudge { id: "vol".into(), delta: 0.10 }));
        assert_eq!(g.speak("a bit less", &ts),
                   Some(GloveAction::Nudge { id: "vol".into(), delta: -0.02 }));
        assert_eq!(g.speak("mute", &ts),
                   Some(GloveAction::Nudge { id: "vol".into(), delta: -1.0 }));
    }

    #[test]
    fn slider_words_mean_nothing_to_a_button() {
        // "louder" against a Send button should fail rather than doing
        // something arbitrary.
        let mut g = cold_cursor();
        let ts = vec![target("send", TargetKind::Button, 200.0, 400.0)];
        g.feed(Orient::Rest, 16.0, &ts);
        assert_eq!(g.speak("louder", &ts), None);
    }

    #[test]
    fn stepping_beats_rolling_to_each_row_of_a_list() {
        let mut g = cold_cursor();
        let ts = vec![target("a", TargetKind::ListItem, 200.0, 400.0),
                      target("b", TargetKind::ListItem, 200.0, 460.0),
                      target("c", TargetKind::ListItem, 200.0, 520.0)];
        g.feed(Orient::Rest, 16.0, &ts);
        assert_eq!(g.snapped(), Some("a"));
        g.speak("next", &ts);
        assert_eq!(g.snapped(), Some("b"));
        g.speak("back", &ts);
        assert_eq!(g.snapped(), Some("a"));
    }

    #[test]
    fn stepping_moves_the_ball_to_the_new_target() {
        // Otherwise the highlight is somewhere the ball is not, and the next
        // roll snaps back to where the ball actually sits.
        let mut g = cold_cursor();
        let ts = vec![target("a", TargetKind::ListItem, 200.0, 400.0),
                      target("b", TargetKind::ListItem, 200.0, 460.0)];
        g.feed(Orient::Rest, 16.0, &ts);
        g.speak("next", &ts);
        assert_eq!(g.pos(), (200.0, 460.0));
        
    }

    #[test]
    fn stepping_wraps_and_starts_sensibly_from_nothing() {
        let mut g = cold_cursor();
        let ts = vec![target("a", TargetKind::ListItem, 10.0, 10.0),
                      target("b", TargetKind::ListItem, 10.0, 70.0)];
        // Nothing snapped: "next" means the first, not a no-op to guess at.
        assert_eq!(g.step(&ts, 1).as_deref(), Some("a"));
        g.step(&ts, 1);
        assert_eq!(g.step(&ts, 1).as_deref(), Some("a"), "wrapped");
    }

    #[test]
    fn speaking_with_nothing_snapped_does_nothing() {
        let mut g = cold_cursor();
        assert_eq!(g.speak("select", &[]), None);
    }

    #[test]
    fn an_unknown_phrase_is_refused_rather_than_guessed() {
        // Acting on a misheard word is worse than doing nothing: the user is
        // wearing gloves and cannot easily undo.
        let mut g = cold_cursor();
        let ts = vec![target("send", TargetKind::Button, 200.0, 400.0)];
        g.feed(Orient::Rest, 16.0, &ts);
        assert_eq!(g.speak("banana", &ts), None);
    }

    #[test]
    fn the_vocabulary_is_case_and_whitespace_forgiving() {
        // Someone is speaking through a scarf; the transcript will be messy.
        let mut g = cold_cursor();
        let ts = vec![target("send", TargetKind::Button, 200.0, 400.0)];
        g.feed(Orient::Rest, 16.0, &ts);
        assert!(g.speak("  SELECT ", &ts).is_some());
    }

    #[test]
    fn the_readout_names_what_will_be_acted_on() {
        // Without it, "select" on a shaking screen is a guess.
        let mut g = cold_cursor();
        let mut t = target("vol", TargetKind::Slider, 200.0, 400.0);
        t.label = "Volume".into();
        let ts = vec![t];
        g.feed(Orient::Rest, 16.0, &ts);
        assert_eq!(g.readout(&ts), "Volume");
    }

    #[test]
    fn cancel_works_from_any_kind() {
        let mut g = cold_cursor();
        for kind in [TargetKind::Button, TargetKind::Field, TargetKind::Slider,
                     TargetKind::Toggle, TargetKind::ListItem] {
            let ts = vec![target("x", kind, 200.0, 400.0)];
            g.feed(Orient::Rest, 16.0, &ts);
            assert_eq!(g.speak("cancel", &ts), Some(GloveAction::Dismiss), "{kind:?}");
        }
    }
}

#[cfg(test)]
mod escape_and_confirm_tests {
    use super::*;

    fn target(id: &str, kind: TargetKind, x: f32, y: f32) -> Target {
        Target { id: id.into(), kind, at: (x, y), label: id.into() }
    }

    fn cursor() -> GloveCursor {
        let mut g = GloveCursor { mode_setting: ColdMode::Auto { threshold_c: COLD_C }, ambient_c: Some(1.0), ..Default::default() };
        g.set_bounds(400.0, 800.0);
        g
    }

    #[test]
    fn while_dictating_the_vocabulary_is_suspended() {
        // THE bug this fixes. "next" is a word someone dictates into a message;
        // resolving it as a command jumps the cursor and swallows the word.
        let ts = vec![target("body", TargetKind::Field, 200.0, 400.0)];
        let mut g = cursor();
        g.feed(Orient::Rest, 16.0, &ts);
        g.speak("select", &ts);
        assert!(g.dictating());
        assert_eq!(g.speak("next", &ts), Some(GloveAction::Dictated("next".into())));
        // Even "cancel" is content while listening, which is exactly why the
        // escape has to be out of band.
        assert_eq!(g.speak("cancel", &ts), Some(GloveAction::Dictated("cancel".into())));
    }

    #[test]
    fn holding_the_phone_screen_down_escapes_dictation() {
        // A HOLD, not a double flick: escape must be reliable and can afford
        // latency, and a hold cannot be produced by a single ambient sway.
        let ts = vec![target("body", TargetKind::Field, 200.0, 400.0)];
        let mut g = cursor();
        g.feed(Orient::Rest, 16.0, &ts);
        g.speak("select", &ts);
        assert!(g.dictating());

        g.feed(Orient::FaceDown, 0.0, &ts);
        let out = g.feed(Orient::FaceDown, ESCAPE_HOLD_MS, &ts);
        assert!(out.escaped);
        assert!(!g.dictating());
    }

    #[test]
    fn a_brief_flip_face_down_does_not_escape_it_just_re_arms() {
        // Otherwise the escape would fire every time the phone was turned over
        // in passing, and face-down is a resting pose people use constantly.
        let ts = vec![target("body", TargetKind::Field, 200.0, 400.0)];
        let mut g = cursor();
        g.feed(Orient::Rest, 16.0, &ts);
        g.speak("select", &ts);
        g.feed(Orient::FaceDown, 0.0, &ts);
        let out = g.feed(Orient::FaceDown, ESCAPE_HOLD_MS - 50.0, &ts);
        assert!(!out.escaped);
        assert!(g.dictating(), "still listening");
    }

    #[test]
    fn the_cursor_does_not_wander_while_dictating() {
        // A phone held near the mouth sways. A cursor that drifted off the
        // field during dictation would deliver the text somewhere else.
        let ts = vec![target("body", TargetKind::Field, 200.0, 400.0)];
        let mut g = cursor();
        g.feed(Orient::Rest, 16.0, &ts);
        g.speak("select", &ts);
        let before = g.pos();
        for _ in 0..5 {
            g.feed(Orient::Rest, 200.0, &ts);
            g.feed(Orient::LeftUp, 100.0, &ts);
            g.feed(Orient::Rest, 0.0, &ts);
        }
        assert_eq!(g.pos(), before);
    }

    #[test]
    fn escaping_when_not_listening_reports_nothing_happened() {
        let mut g = cursor();
        g.feed(Orient::FaceDown, 0.0, &[]);
        let out = g.feed(Orient::FaceDown, ESCAPE_HOLD_MS, &[]);
        assert!(!out.escaped, "there was nothing to escape from");
    }

    #[test]
    fn after_escaping_the_vocabulary_works_again() {
        let ts = vec![target("body", TargetKind::Field, 200.0, 400.0)];
        let mut g = cursor();
        g.feed(Orient::Rest, 16.0, &ts);
        g.speak("select", &ts);
        g.feed(Orient::FaceDown, 0.0, &ts);
        g.feed(Orient::FaceDown, ESCAPE_HOLD_MS, &ts);
        g.feed(Orient::Rest, 16.0, &ts);
        assert_eq!(g.speak("select", &ts), Some(GloveAction::ArmField("body".into())));
    }

    // ── confirmations ───────────────────────────────────────────────────────

    #[test]
    fn yes_does_not_confirm_anything() {
        // One short syllable, indistinguishable from a half-heard "yes?" or a
        // background conversation, and carrying no information about what is
        // being agreed to.
        let c = Confirm::new("Delete this snap?", "delete", true);
        assert_eq!(c.answer("yes"), Answer::Unclear);
        assert_eq!(c.answer("yeah"), Answer::Unclear);
        assert_eq!(c.answer("ok"), Answer::Unclear);
    }

    #[test]
    fn naming_the_verb_confirms() {
        let c = Confirm::new("Delete this snap?", "delete", true);
        assert_eq!(c.answer("delete"), Answer::Confirmed);
        assert_eq!(c.answer("  DELETE  "), Answer::Confirmed);
        assert_eq!(c.answer("delete it"), Answer::Confirmed);
    }

    #[test]
    fn the_verb_of_one_prompt_does_not_answer_another() {
        // Saying "delete" at a "Leave the call?" prompt must not confirm it —
        // which is the real reason to require the verb rather than "yes".
        let leave = Confirm::new("Leave the call?", "leave", false);
        assert_eq!(leave.answer("delete"), Answer::Unclear);
        assert_eq!(leave.answer("leave"), Answer::Confirmed);
    }

    #[test]
    fn cancelling_is_easy_and_has_several_words() {
        // Backing out of a destructive prompt should never require precision.
        let c = Confirm::new("Delete this snap?", "delete", true);
        for phrase in ["cancel", "no", "stop", "never mind", "go back"] {
            assert_eq!(c.answer(phrase), Answer::Cancelled, "{phrase}");
        }
    }

    #[test]
    fn an_unclear_answer_leaves_the_prompt_up() {
        // Neither confirming nor cancelling on a mishearing: the prompt is the
        // safe state, so staying there is the safe failure.
        let c = Confirm::new("Delete this snap?", "delete", true);
        assert_eq!(c.answer("banana"), Answer::Unclear);
        assert_eq!(c.answer(""), Answer::Unclear);
    }

    #[test]
    fn the_readout_teaches_the_word_it_requires() {
        // A user who does not know the verb cannot say it, so the prompt has to
        // say it.
        let c = Confirm::new("Delete this snap?", "delete", true);
        let r = c.readout();
        assert!(r.contains("delete"));
        assert!(r.contains("cancel"));
    }
}

#[cfg(test)]
mod shake_command_tests {
    use super::*;

    fn cmds() -> ShakeCommands {
        let mut c = ShakeCommands::with(vec![
            ShakeBinding { dir: Orient::BottomUp, action: "new_tab".into(),
                           label: "New tab".into() },
            ShakeBinding { dir: Orient::RightUp, action: "swap_camera".into(),
                           label: "Swap camera".into() },
        ]);
        c.enabled = true;
        c
    }

    #[test]
    fn a_single_flick_is_not_a_command() {
        // Single flicks move a cursor. If one also opened a tab, walking the
        // cursor across the screen would open a row of them.
        let mut c = cmds();
        assert_eq!(c.feed(Some(Orient::BottomUp), 16.0), None);
    }

    #[test]
    fn two_flicks_the_same_way_are_a_command() {
        let mut c = cmds();
        c.feed(Some(Orient::BottomUp), 16.0);
        assert_eq!(c.feed(Some(Orient::BottomUp), 200.0).as_deref(), Some("new_tab"));
    }

    #[test]
    fn the_second_flick_must_arrive_inside_the_window() {
        // Two considered moves minutes apart are not one gesture.
        let mut c = cmds();
        c.feed(Some(Orient::BottomUp), 16.0);
        c.feed(None, DOUBLE_FLICK_MS + 100.0);
        assert_eq!(c.feed(Some(Orient::BottomUp), 16.0), None, "the pair timed out");
    }

    #[test]
    fn changing_direction_restarts_the_pair_rather_than_firing() {
        // Someone who flicks left then right has changed their mind, not made a
        // command.
        let mut c = cmds();
        c.feed(Some(Orient::BottomUp), 16.0);
        assert_eq!(c.feed(Some(Orient::RightUp), 100.0), None);
        // And that right flick is now the first of a NEW pair.
        assert_eq!(c.feed(Some(Orient::RightUp), 100.0).as_deref(), Some("swap_camera"));
    }

    #[test]
    fn a_third_flick_does_not_fire_again_immediately() {
        // Otherwise a vigorous shake ratchets a command per flick after the
        // first pair.
        let mut c = cmds();
        c.feed(Some(Orient::BottomUp), 16.0);
        assert!(c.feed(Some(Orient::BottomUp), 100.0).is_some());
        assert_eq!(c.feed(Some(Orient::BottomUp), 100.0), None, "starts a fresh pair");
    }

    #[test]
    fn an_unbound_direction_does_nothing() {
        let mut c = cmds();
        c.feed(Some(Orient::LeftUp), 16.0);
        assert_eq!(c.feed(Some(Orient::LeftUp), 100.0), None);
    }

    #[test]
    fn the_toggle_is_independent_of_glove_mode() {
        // Someone may want shake-for-new-tab without a cursor, and someone in
        // gloves may want the cursor without their tabs multiplying.
        let mut c = cmds();
        c.enabled = false;
        c.feed(Some(Orient::BottomUp), 16.0);
        assert_eq!(c.feed(Some(Orient::BottomUp), 100.0), None);
    }

    #[test]
    fn settings_can_name_what_a_gesture_does_without_performing_it() {
        let c = cmds();
        assert_eq!(c.describe(Orient::BottomUp), Some("New tab"));
        assert_eq!(c.describe(Orient::LeftUp), None);
    }

    #[test]
    fn the_window_is_wider_than_a_shake_and_narrower_than_two_moves() {
        assert!(DOUBLE_FLICK_MS > FLICK_MAX_MS - 200.0);
        assert!(DOUBLE_FLICK_MS < 1_000.0);
    }
}
