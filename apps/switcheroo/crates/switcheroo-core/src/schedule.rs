//! Schedules: "make sure the lamp is off by 9pm", "turn the porch light on
//! when I get home", "when my wifi starts getting far, turn the hall light
//! on".
//!
//! # Shape
//!
//! A [`Rule`] is a [`Trigger`] and an [`Action`]. The [`Scheduler`] is
//! fed a clock reading and a [`Signals`] snapshot each tick and hands back
//! the actions that fire. It owns no clock and reads no `iw`; the shell
//! does, once per second or so, and passes the results in. That is what
//! makes "arrive home at 18:04 with the wifi at −71 dBm" a unit test.
//!
//! # Trigger semantics, because each one has a trap
//!
//! * [`Trigger::By`] — a deadline. Fires at that minute of the day if the
//!   target is not already in the wanted state, once per day. "By 9pm"
//!   means "not later than", so the shell may also apply it at startup if
//!   the time has passed and the device is still on; the rule itself is
//!   the minute.
//! * [`Trigger::At`] — the same minute, but fires regardless of state
//!   (a wake-up light that is already on should still be told "on").
//! * [`Trigger::ArriveHome`] / [`Trigger::LeaveHome`] — EDGES on "associated
//!   with the home network", not levels. Being home all evening is not an
//!   arrival, and a wifi hiccup is not a departure: a departure needs the
//!   network gone for [`LEAVE_GRACE_TICKS`] consecutive ticks.
//! * [`Trigger::WifiWeak`] — the signal has been below the threshold for
//!   [`WEAK_CONFIRM_TICKS`] consecutive ticks WHILE STILL CONNECTED. This
//!   is the "getting far but not disconnected" case: the point is to act
//!   before the connection drops. It re-arms only after the signal has
//!   come back above the threshold (hysteresis), so a walk along the edge
//!   of range does not fire it every tick.
//!
//! Wifi RSSI is what Linux gives cheaply (`iw dev wlan0 link`,
//! `/proc/net/wireless`); see [`crate::wifi`] for the parsers.

use crate::intent::Target;

/// Minutes since local midnight, and which day, so "once per day" has a
/// key. The shell derives this from the wall clock; tests hand it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    pub minute_of_day: u16,
    pub day: u32,
}

impl Clock {
    pub fn new(day: u32, hour: u8, minute: u8) -> Self {
        Clock { minute_of_day: hour as u16 * 60 + minute as u16, day }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Wifi {
    /// Associated with any network.
    pub connected: bool,
    /// Associated with the network the user named "home". `false` when
    /// connected elsewhere or not at all.
    pub at_home: bool,
    /// Signal level, dBm, when connected. Typical: −30 next to the router,
    /// −70 across the house, −85 about to drop.
    pub rssi_dbm: Option<i16>,
}

/// Everything a tick needs to know about the world besides the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Signals {
    pub wifi: Wifi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// Deadline: at this minute, if not already in the wanted state.
    By { minute_of_day: u16 },
    /// At this minute, always.
    At { minute_of_day: u16 },
    ArriveHome,
    LeaveHome,
    /// Signal below `below_dbm` while still connected.
    WifiWeak { below_dbm: i16 },
}

/// Ticks a weak signal must persist before the rule fires. At one tick a
/// second, three is enough to skip a single bad sample and short enough
/// that the light is on before the phone drops off the network.
pub const WEAK_CONFIRM_TICKS: u8 = 3;
/// Ticks without the home network before that counts as leaving. Ten
/// seconds rides out an AP reboot.
pub const LEAVE_GRACE_TICKS: u8 = 10;
/// Default for "getting far": −72 dBm is well inside "still works" for
/// most phones and well outside "sitting next to the router".
pub const DEFAULT_WEAK_DBM: i16 = -72;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub target: Target,
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub id: u32,
    pub trigger: Trigger,
    pub action: Action,
    pub enabled: bool,
}

impl Rule {
    /// A one-line human sentence, for the schedules panel.
    pub fn describe(&self) -> String {
        let what = format!("turn {} {}", self.action.target.describe(), if self.action.on { "on" } else { "off" });
        match &self.trigger {
            Trigger::By { minute_of_day } => format!("{what} by {}", fmt_time(*minute_of_day)),
            Trigger::At { minute_of_day } => format!("{what} at {}", fmt_time(*minute_of_day)),
            Trigger::ArriveHome => format!("{what} when I get home"),
            Trigger::LeaveHome => format!("{what} when I leave home"),
            Trigger::WifiWeak { below_dbm } => format!("{what} when wifi drops below {below_dbm} dBm"),
        }
    }

    /// Line format, one rule per line: `id|enabled|trigger|on|target`.
    /// Tab-free, pipe-separated, no quoting: targets are device names and
    /// device names come from BLE local names, which cannot contain `|`
    /// in any advertisement anyone ships. Hand-editable on purpose.
    pub fn to_line(&self) -> String {
        let trig = match &self.trigger {
            Trigger::By { minute_of_day } => format!("by:{minute_of_day}"),
            Trigger::At { minute_of_day } => format!("at:{minute_of_day}"),
            Trigger::ArriveHome => "arrive".to_string(),
            Trigger::LeaveHome => "leave".to_string(),
            Trigger::WifiWeak { below_dbm } => format!("weak:{below_dbm}"),
        };
        format!("{}|{}|{}|{}|{}", self.id, if self.enabled { 1 } else { 0 }, trig, if self.action.on { "on" } else { "off" }, self.action.target.to_token())
    }

    pub fn from_line(line: &str) -> Option<Rule> {
        let mut it = line.trim().splitn(5, '|');
        let id = it.next()?.parse().ok()?;
        let enabled = it.next()? == "1";
        let trig = it.next()?;
        let on = match it.next()? { "on" => true, "off" => false, _ => return None };
        let target = Target::from_token(it.next()?)?;
        let trigger = match trig.split_once(':') {
            Some(("by", m)) => Trigger::By { minute_of_day: m.parse().ok()? },
            Some(("at", m)) => Trigger::At { minute_of_day: m.parse().ok()? },
            Some(("weak", d)) => Trigger::WifiWeak { below_dbm: d.parse().ok()? },
            None if trig == "arrive" => Trigger::ArriveHome,
            None if trig == "leave" => Trigger::LeaveHome,
            _ => return None,
        };
        Some(Rule { id, trigger, action: Action { target, on }, enabled })
    }
}

pub fn fmt_time(minute_of_day: u16) -> String {
    let h = minute_of_day / 60;
    let m = minute_of_day % 60;
    let (h12, ampm) = match h { 0 => (12, "am"), 1..=11 => (h, "am"), 12 => (12, "pm"), _ => (h - 12, "pm") };
    if m == 0 { format!("{h12}{ampm}") } else { format!("{h12}:{m:02}{ampm}") }
}

/// What the scheduler needs to know about a target's current state to
/// honour a `By` deadline. `None` = unknown; a deadline fires on unknown.
pub type StateLookup<'a> = &'a dyn Fn(&Target) -> Option<bool>;

/// Per-rule state for a `WifiWeak` trigger: how many consecutive weak
/// ticks, and whether it may fire (re-armed by the signal recovering).
#[derive(Debug, Clone, Copy)]
struct WeakState { ticks: u8, armed: bool }

/// The engine. Feed it every tick.
#[derive(Debug, Default)]
pub struct Scheduler {
    pub rules: Vec<Rule>,
    next_id: u32,
    /// (rule id, day) pairs already fired, for once-per-day time rules.
    fired_today: Vec<(u32, u32)>,
    /// Debounced "at home": `None` until the first grace period settles it.
    confirmed_home: Option<bool>,
    away_ticks: u8,
    weak: std::collections::BTreeMap<u32, WeakState>,
}

impl Scheduler {
    pub fn new() -> Self { Scheduler::default() }

    pub fn add(&mut self, trigger: Trigger, action: Action) -> u32 {
        self.next_id += 1;
        let id = self.next_id;
        self.rules.push(Rule { id, trigger, action, enabled: true });
        id
    }

    pub fn remove(&mut self, id: u32) {
        self.rules.retain(|r| r.id != id);
        self.weak.remove(&id);
    }

    pub fn set_enabled(&mut self, id: u32, on: bool) {
        if let Some(r) = self.rules.iter_mut().find(|r| r.id == id) { r.enabled = on; }
    }

    /// Persist: one line per rule.
    pub fn to_lines(&self) -> String {
        self.rules.iter().map(Rule::to_line).collect::<Vec<_>>().join("\n")
    }

    /// Load; unparsable lines are skipped rather than failing the whole
    /// file, and `next_id` continues above the highest loaded id.
    pub fn from_lines(text: &str) -> Self {
        let mut s = Scheduler::new();
        s.rules = text.lines().filter_map(Rule::from_line).collect();
        s.next_id = s.rules.iter().map(|r| r.id).max().unwrap_or(0);
        s
    }

    /// One tick. `state_of` answers "is this target on right now?" for
    /// deadlines. Returns the actions to perform, in rule order.
    pub fn tick(&mut self, clock: Clock, signals: &Signals, state_of: StateLookup<'_>) -> Vec<Action> {
        // Day rollover: forget yesterday's fired set.
        self.fired_today.retain(|(_, d)| *d == clock.day);

        // Home edges, debounced, computed once per tick regardless of rules.
        let w = signals.wifi;
        let mut arrived = false;
        let mut left = false;
        if w.at_home {
            self.away_ticks = 0;
            if self.confirmed_home == Some(false) { arrived = true; }
            self.confirmed_home = Some(true);
        } else {
            self.away_ticks = self.away_ticks.saturating_add(1);
            if self.away_ticks >= LEAVE_GRACE_TICKS {
                if self.confirmed_home == Some(true) { left = true; }
                self.confirmed_home = Some(false);
            }
        }

        let mut out = Vec::new();
        for r in &self.rules {
            if !r.enabled { continue; }
            let fire = match &r.trigger {
                Trigger::By { minute_of_day } | Trigger::At { minute_of_day } => {
                    let due = clock.minute_of_day >= *minute_of_day;
                    let already = self.fired_today.contains(&(r.id, clock.day));
                    if !due || already {
                        false
                    } else {
                        self.fired_today.push((r.id, clock.day));
                        match r.trigger {
                            Trigger::By { .. } => state_of(&r.action.target) != Some(r.action.on),
                            _ => true,
                        }
                    }
                }
                Trigger::ArriveHome => arrived,
                Trigger::LeaveHome => left,
                Trigger::WifiWeak { below_dbm } => {
                    let st = self.weak.entry(r.id).or_insert(WeakState { ticks: 0, armed: true });
                    match (w.connected, w.rssi_dbm) {
                        (true, Some(d)) if d < *below_dbm => {
                            st.ticks = st.ticks.saturating_add(1);
                            if st.armed && st.ticks >= WEAK_CONFIRM_TICKS {
                                st.armed = false; // until the signal recovers
                                true
                            } else { false }
                        }
                        (true, Some(_)) => { st.ticks = 0; st.armed = true; false }
                        // Disconnected or no reading is not "weak but connected".
                        _ => { st.ticks = 0; false }
                    }
                }
            };
            if fire {
                out.push(r.action.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::Target;

    fn lamp() -> Target { Target::Named("lamp".into()) }
    fn quiet() -> Signals { Signals::default() }
    fn home(rssi: i16) -> Signals { Signals { wifi: Wifi { connected: true, at_home: true, rssi_dbm: Some(rssi) } } }
    fn away() -> Signals { Signals { wifi: Wifi { connected: false, at_home: false, rssi_dbm: None } } }
    fn elsewhere(rssi: i16) -> Signals { Signals { wifi: Wifi { connected: true, at_home: false, rssi_dbm: Some(rssi) } } }

    #[test]
    fn a_deadline_fires_once_at_the_minute_only_if_the_state_is_wrong() {
        let mut s = Scheduler::new();
        s.add(Trigger::By { minute_of_day: 21 * 60 }, Action { target: lamp(), on: false });
        let still_on = |_: &Target| Some(true);
        let already_off = |_: &Target| Some(false);
        assert!(s.tick(Clock::new(1, 20, 59), &quiet(), &still_on).is_empty(), "not due");
        assert_eq!(s.tick(Clock::new(1, 21, 0), &quiet(), &still_on).len(), 1, "due and on → fires");
        assert!(s.tick(Clock::new(1, 21, 1), &quiet(), &still_on).is_empty(), "once per day");
        assert!(s.tick(Clock::new(2, 21, 0), &quiet(), &already_off).is_empty(), "next day, already off → nothing");
        assert_eq!(s.tick(Clock::new(3, 23, 30), &quiet(), &still_on).len(), 1, "late start after the minute still honours the deadline");
    }

    #[test]
    fn an_at_rule_fires_regardless_of_state() {
        let mut s = Scheduler::new();
        s.add(Trigger::At { minute_of_day: 7 * 60 }, Action { target: lamp(), on: true });
        let already_on = |_: &Target| Some(true);
        assert_eq!(s.tick(Clock::new(1, 7, 0), &quiet(), &already_on).len(), 1);
    }

    #[test]
    fn arriving_home_is_an_edge_from_confirmed_away_not_a_level() {
        let mut s = Scheduler::new();
        s.add(Trigger::ArriveHome, Action { target: lamp(), on: true });
        let none = |_: &Target| None;
        let c = Clock::new(1, 18, 0);
        // Start away: the first ticks settle "away" without an event.
        for _ in 0..LEAVE_GRACE_TICKS { assert!(s.tick(c, &away(), &none).is_empty()); }
        assert_eq!(s.tick(c, &home(-50), &none).len(), 1, "arrival");
        assert!(s.tick(c, &home(-50), &none).is_empty(), "staying home is not arriving again");
        // A short hiccup does not create a second arrival.
        s.tick(c, &away(), &none);
        s.tick(c, &away(), &none);
        assert!(s.tick(c, &home(-50), &none).is_empty(), "hiccup shorter than the grace is not an edge");
    }

    #[test]
    fn leaving_needs_the_grace_period_and_starting_at_home_is_not_an_arrival() {
        let mut s = Scheduler::new();
        s.add(Trigger::LeaveHome, Action { target: lamp(), on: false });
        s.add(Trigger::ArriveHome, Action { target: lamp(), on: true });
        let none = |_: &Target| None;
        let c = Clock::new(1, 8, 0);
        assert!(s.tick(c, &home(-40), &none).is_empty(), "app started at home: no arrival event");
        for i in 1..LEAVE_GRACE_TICKS {
            assert!(s.tick(c, &away(), &none).is_empty(), "tick {i} away is not yet leaving");
        }
        let fired = s.tick(c, &away(), &none);
        assert_eq!(fired.len(), 1);
        assert!(!fired[0].on, "the leave rule, not the arrive rule");
        // Connected somewhere else counts as not home.
        assert!(s.tick(c, &elsewhere(-40), &none).is_empty());
    }

    #[test]
    fn weak_wifi_fires_after_three_weak_ticks_while_connected_then_rearms_on_recovery() {
        let mut s = Scheduler::new();
        s.add(Trigger::WifiWeak { below_dbm: DEFAULT_WEAK_DBM }, Action { target: lamp(), on: true });
        let none = |_: &Target| None;
        let c = Clock::new(1, 19, 0);
        assert!(s.tick(c, &home(-50), &none).is_empty());
        assert!(s.tick(c, &home(-80), &none).is_empty(), "1 weak tick");
        assert!(s.tick(c, &home(-80), &none).is_empty(), "2");
        assert_eq!(s.tick(c, &home(-80), &none).len(), 1, "3 → fires");
        assert!(s.tick(c, &home(-85), &none).is_empty(), "still weak: does not re-fire");
        assert!(s.tick(c, &away(), &none).is_empty(), "disconnected is not 'weak but connected'");
        assert!(s.tick(c, &home(-55), &none).is_empty(), "recovered: re-armed, silent");
        for _ in 0..2 { s.tick(c, &home(-80), &none); }
        assert_eq!(s.tick(c, &home(-80), &none).len(), 1, "fires again after recovery");
    }

    #[test]
    fn a_single_bad_sample_does_not_fire_weak() {
        let mut s = Scheduler::new();
        s.add(Trigger::WifiWeak { below_dbm: -72 }, Action { target: lamp(), on: true });
        let none = |_: &Target| None;
        let c = Clock::new(1, 19, 0);
        s.tick(c, &home(-90), &none);
        s.tick(c, &home(-50), &none);
        s.tick(c, &home(-90), &none);
        assert!(s.tick(c, &home(-50), &none).is_empty());
    }

    #[test]
    fn rules_round_trip_through_the_line_format_and_describe_themselves() {
        let mut s = Scheduler::new();
        s.add(Trigger::By { minute_of_day: 21 * 60 }, Action { target: Target::Named("kitchen light".into()), on: false });
        s.add(Trigger::ArriveHome, Action { target: Target::Lights, on: true });
        s.add(Trigger::WifiWeak { below_dbm: -70 }, Action { target: Target::Named("porch".into()), on: true });
        s.add(Trigger::At { minute_of_day: 6 * 60 + 30 }, Action { target: Target::Music, on: true });
        s.set_enabled(2, false);
        let text = s.to_lines();
        assert!(text.contains("1|1|by:1260|off|name:kitchen light"), "{text}");
        assert!(text.contains("2|0|arrive|on|lights"), "{text}");
        let back = Scheduler::from_lines(&text);
        assert_eq!(back.rules, s.rules);
        assert_eq!(back.rules[0].describe(), "turn kitchen light off by 9pm");
        assert_eq!(back.rules[1].describe(), "turn the lights on when I get home");
        assert_eq!(back.rules[2].describe(), "turn porch on when wifi drops below -70 dBm");
        assert_eq!(back.rules[3].describe(), "turn the music on at 6:30am");
        // A garbage line is skipped, ids continue from the highest.
        let mut back2 = Scheduler::from_lines(&format!("{text}\nnot a rule\n"));
        assert_eq!(back2.rules.len(), 4);
        assert_eq!(back2.add(Trigger::LeaveHome, Action { target: Target::All, on: false }), 5);
    }

    #[test]
    fn time_formatting_is_twelve_hour() {
        assert_eq!(fmt_time(0), "12am");
        assert_eq!(fmt_time(9 * 60 + 5), "9:05am");
        assert_eq!(fmt_time(12 * 60), "12pm");
        assert_eq!(fmt_time(21 * 60), "9pm");
        assert_eq!(fmt_time(23 * 60 + 59), "11:59pm");
    }
}
