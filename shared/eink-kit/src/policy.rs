//! The refresh policy: which waveform each damaged region gets, and when.
//!
//! This is the part of the stack that makes an immediate-mode UI tolerable
//! on e-ink, and the part most likely to need tuning per panel, so it is
//! pure (time is passed in), every decision carries a reason, and the
//! runner prints decisions when `EINK_TRACE=1`.
//!
//! ## Model
//!
//! * Interaction (a finger is down, or input arrived in the last
//!   `settle`) is drawn with the FAST waveform. Fast waveforms ghost.
//! * Once input goes quiet for `settle`, the union of everything drawn fast
//!   since the last settle is redrawn with a QUALITY waveform. That is the
//!   ghost clean-up, and it is deferred so it never competes with the
//!   finger for the panel.
//! * Every `full_every` settles, the settle is a FULL refresh instead,
//!   because partial quality refreshes accumulate ghosting too.
//!
//! Profiles change only the fast path: `fast-color` sends fast updates with
//! the content's colour flag; `fast-mono` always sends them as monochrome
//! (cheaper waveforms on a colour panel); `quality` never uses fast.
//!
//! Backends that cannot change waveform per update (qtfb: a mode change
//! stalls the client for a second) report that in `Caps`, and the policy
//! then emits every update at the backend's one mode and skips settles.

use crate::damage::Damage;
use crate::geom::IRect;
use std::time::Duration;

/// Abstract waveform classes. Backends map these to their own enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Fast,
    Quality,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    FastColor,
    FastMono,
    Quality,
}

impl Profile {
    pub fn parse(s: &str) -> Option<Profile> {
        match s.trim() {
            "fast-color" => Some(Profile::FastColor),
            "fast-mono" => Some(Profile::FastMono),
            "quality" => Some(Profile::Quality),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::FastColor => "fast-color",
            Profile::FastMono => "fast-mono",
            Profile::Quality => "quality",
        }
    }
    /// Feathering (anti-aliasing) default: off for fast profiles, where fast
    /// waveforms turn grey fringes into speckle; on for quality.
    pub fn feathering(self) -> bool {
        matches!(self, Profile::Quality)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Update {
    pub rect: IRect,
    pub waveform: Waveform,
    pub color: bool,
    pub reason: &'static str,
}

#[derive(Debug, Clone)]
pub struct Policy {
    pub profile: Profile,
    pub settle: Duration,
    pub full_every: u32,
    /// False when the backend can only run one waveform (qtfb).
    pub per_update_waveform: bool,
    pending: IRect,
    pending_color: bool,
    deadline: Option<Duration>,
    settles: u32,
    full_requested: bool,
    screen: IRect,
}

impl Policy {
    pub fn new(profile: Profile, screen: IRect, per_update_waveform: bool) -> Policy {
        Policy {
            profile,
            settle: Duration::from_millis(450),
            full_every: 16,
            per_update_waveform,
            pending: IRect::default(),
            pending_color: false,
            deadline: None,
            settles: 0,
            full_requested: false,
            screen,
        }
    }

    /// Next time `on_tick` has work, if any. The runner sleeps until then.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.deadline
    }

    /// Ask for a full refresh at the next settle (e.g. a "clean screen" button).
    pub fn request_full(&mut self) {
        self.full_requested = true;
        self.pending = self.screen;
        self.deadline.get_or_insert(Duration::ZERO);
    }

    /// New damage from a frame.
    pub fn on_damage(&mut self, now: Duration, damage: &[Damage], interacting: bool) -> Vec<Update> {
        if damage.is_empty() {
            return Vec::new();
        }
        if !self.per_update_waveform {
            let wf = if self.profile == Profile::Quality { Waveform::Quality } else { Waveform::Fast };
            return damage
                .iter()
                .map(|d| Update { rect: d.rect, waveform: wf, color: d.color, reason: "single-mode backend" })
                .collect();
        }
        let fast = self.profile != Profile::Quality && interacting;
        if !fast {
            return damage
                .iter()
                .map(|d| Update {
                    rect: d.rect,
                    waveform: Waveform::Quality,
                    color: d.color,
                    reason: if self.profile == Profile::Quality { "quality profile" } else { "idle: no input in flight" },
                })
                .collect();
        }
        let mono = self.profile == Profile::FastMono;
        let mut out = Vec::with_capacity(damage.len());
        for d in damage {
            self.pending = self.pending.union(&d.rect);
            self.pending_color |= d.color;
            out.push(Update {
                rect: d.rect,
                waveform: Waveform::Fast,
                color: d.color && !mono,
                reason: if mono { "interacting, fast-mono" } else { "interacting, fast-color" },
            });
        }
        self.deadline = Some(now + self.settle);
        out
    }

    /// Input arrived without (yet) producing damage: push the settle back so
    /// the clean-up never lands mid-gesture.
    pub fn on_input(&mut self, now: Duration) {
        if self.deadline.is_some() {
            self.deadline = Some(now + self.settle);
        }
    }

    /// Time passed. Emits the settle refresh when due.
    pub fn on_tick(&mut self, now: Duration, interacting: bool) -> Vec<Update> {
        match self.deadline {
            Some(t) if now >= t && !interacting => {}
            _ => return Vec::new(),
        }
        self.deadline = None;
        if self.pending.is_empty() {
            return Vec::new();
        }
        self.settles += 1;
        let full = self.full_requested || self.settles % self.full_every == 0;
        self.full_requested = false;
        let u = if full {
            Update { rect: self.screen, waveform: Waveform::Full, color: true, reason: "settle: periodic/requested full refresh" }
        } else {
            Update { rect: self.pending, waveform: Waveform::Quality, color: self.pending_color, reason: "settle: clean up fast-drawn region" }
        };
        self.pending = IRect::default();
        self.pending_color = false;
        vec![u]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: IRect = IRect::new(0, 0, 1000, 1000);
    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }
    fn dmg(x: i32, color: bool) -> Damage {
        Damage { rect: IRect::new(x, 0, x + 10, 10), color }
    }

    #[test]
    fn interaction_is_fast_then_settles_to_quality_over_the_union() {
        let mut p = Policy::new(Profile::FastColor, S, true);
        let u = p.on_damage(ms(0), &[dmg(0, true)], true);
        assert_eq!((u[0].waveform, u[0].color), (Waveform::Fast, true));
        p.on_damage(ms(100), &[dmg(500, false)], true);
        assert!(p.on_tick(ms(300), false).is_empty(), "too early");
        assert_eq!(p.next_deadline(), Some(ms(550)));
        let s = p.on_tick(ms(560), false);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].waveform, Waveform::Quality);
        assert_eq!(s[0].rect, IRect::new(0, 0, 510, 10));
        assert!(s[0].color);
        assert!(p.on_tick(ms(2000), false).is_empty(), "settles once");
    }

    #[test]
    fn settle_waits_while_a_finger_is_down() {
        let mut p = Policy::new(Profile::FastColor, S, true);
        p.on_damage(ms(0), &[dmg(0, false)], true);
        assert!(p.on_tick(ms(1000), true).is_empty());
        p.on_input(ms(1000));
        assert!(p.on_tick(ms(1200), false).is_empty(), "input pushed it back");
        assert_eq!(p.on_tick(ms(1500), false).len(), 1);
    }

    #[test]
    fn fast_mono_strips_colour_from_fast_updates_only() {
        let mut p = Policy::new(Profile::FastMono, S, true);
        let u = p.on_damage(ms(0), &[dmg(0, true)], true);
        assert!(!u[0].color);
        assert!(p.on_tick(ms(1000), false)[0].color, "the settle shows the real colour");
    }

    #[test]
    fn every_nth_settle_is_a_full_refresh() {
        let mut p = Policy::new(Profile::FastColor, S, true);
        p.full_every = 3;
        let mut kinds = vec![];
        for i in 0..6u64 {
            p.on_damage(ms(i * 1000), &[dmg(0, false)], true);
            kinds.push(p.on_tick(ms(i * 1000 + 900), false)[0].waveform);
        }
        assert_eq!(kinds.iter().filter(|k| **k == Waveform::Full).count(), 2);
        assert_eq!(kinds[2], Waveform::Full);
    }

    #[test]
    fn idle_damage_goes_straight_to_quality() {
        let mut p = Policy::new(Profile::FastColor, S, true);
        let u = p.on_damage(ms(0), &[dmg(0, false)], false);
        assert_eq!(u[0].waveform, Waveform::Quality);
        assert_eq!(p.next_deadline(), None);
    }

    #[test]
    fn single_mode_backends_never_schedule_settles() {
        let mut p = Policy::new(Profile::FastColor, S, false);
        let u = p.on_damage(ms(0), &[dmg(0, true)], true);
        assert_eq!(u[0].waveform, Waveform::Fast);
        assert_eq!(p.next_deadline(), None);
    }

    #[test]
    fn requested_full_refresh_happens_at_the_next_settle() {
        let mut p = Policy::new(Profile::Quality, S, true);
        p.request_full();
        let u = p.on_tick(ms(0), false);
        assert_eq!(u[0].waveform, Waveform::Full);
        assert_eq!(u[0].rect, S);
    }
}
