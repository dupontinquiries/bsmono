//! The **Sentinel** — a prey/bird brain that turns senses into a *tell*.
//!
//! Where the Lauscher is a hunter that listens, the Sentinel is the opposite end
//! of the food web: a watchful animal whose job (for the player) is to *give
//! things away*. It composes the engine's existing senses — `vision::can_see`
//! off its eyes, `sound::heard_loudness_world` off its ears — to notice a threat
//! (a player creeping up, a hunting horror moving through the trees), then
//! broadcasts an [`Alarm`] and strikes a [`Posture`]. That posture is the reveal:
//! a flock that freezes and scatters is how you learn something is out there
//! before you can see it yourself.
//!
//! This module is the deterministic decision core (pure, integer/fixed-point,
//! rolls back with the sim). It is world-free on purpose: the caller runs
//! `vision::can_see` per living eye (see `Anatomy::eye_facings`) and
//! `sound::heard_loudness_world`, then feeds the results in. Food-source memory
//! is a separate, already-built concern (`sim::cognition::memory`).

use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::sound::Loudness;

/// How a threat was noticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sense {
    #[default]
    Sight,
    Hearing,
}

/// What the sentinel is doing — the visible tell, escalating with confidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Posture {
    /// Feeding / idle — nothing noticed. (No reveal.)
    #[default]
    Calm,
    /// A faint cue — head up, alert. A subtle tell to an attentive player.
    Wary,
    /// A clear threat — freeze then flee/scatter. The loud, unmistakable tell.
    Alarmed,
}

/// What the sentinel is broadcasting this tick (its reveal). `None` when calm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Alarm {
    /// Where the threat was last placed.
    pub source: Vec3,
    /// Which sense placed it.
    pub via: Sense,
    /// How strongly it's reacting.
    pub posture: Posture,
}

/// One sentinel's sense tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Senses {
    /// Sight range (passed to `vision::can_see`).
    pub sight_range: Fixed,
    /// cos² of the half-FOV (passed to `vision::can_see`).
    pub fov_cos2: Fixed,
    /// Perceived loudness at/above which a sound registers; ≥ 2× this is a
    /// startling sound that alarms outright rather than merely worrying.
    pub hear_threshold: Loudness,
}

impl Senses {
    /// A jumpy bird: long, wide vision and a low hearing threshold.
    pub fn bird() -> Senses {
        Senses {
            sight_range: Fixed::from_int(24),
            fov_cos2: Fixed::from_frac(67, 1000), // ~150° total FOV
            hear_threshold: 30,
        }
    }
}

/// Ticks the sentinel stays alarmed after a clear detection before settling.
pub const ALARM_TICKS: u16 = 60;
/// Ticks it stays merely wary after a faint cue.
pub const WARY_TICKS: u16 = 30;

/// The sentinel's persistent state. `Copy` + integer-only so it snapshots and
/// rolls back with the rest of the sim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sentinel {
    pub posture: Posture,
    pub via: Sense,
    pub last_threat: Option<Vec3>,
    /// Ticks of reaction left before it calms.
    pub alert: u16,
}

impl Sentinel {
    pub fn new() -> Sentinel {
        Sentinel::default()
    }

    /// Fold this tick's perception in. `sighted` is the threat position if any
    /// living eye saw it (caller ran `vision::can_see`); `heard` is the loudest
    /// occluded sound + its source (caller ran `sound::heard_loudness_world`).
    /// Sight is high-confidence and wins; loud sound alarms; a faint-but-audible
    /// sound only worries. Call once per tick, then [`tick`](Self::tick).
    pub fn perceive(&mut self, sighted: Option<Vec3>, heard: Option<(Vec3, Loudness)>, senses: &Senses) {
        if let Some(p) = sighted {
            self.last_threat = Some(p);
            self.via = Sense::Sight;
            self.alert = ALARM_TICKS;
            self.posture = Posture::Alarmed;
            return;
        }
        if let Some((p, loud)) = heard {
            if loud >= senses.hear_threshold {
                self.last_threat = Some(p);
                self.via = Sense::Hearing;
                if loud >= senses.hear_threshold.saturating_mul(2) {
                    self.alert = ALARM_TICKS;
                    self.posture = Posture::Alarmed;
                } else {
                    self.alert = self.alert.max(WARY_TICKS);
                    if self.posture == Posture::Calm {
                        self.posture = Posture::Wary;
                    }
                }
            }
        }
    }

    /// Advance one tick *after* `perceive`. Decays the reaction; calms when the
    /// trail runs out. Returns the current posture.
    pub fn tick(&mut self) -> Posture {
        if self.alert > 0 {
            self.alert -= 1;
            if self.alert == 0 {
                self.posture = Posture::Calm;
                self.last_threat = None;
            }
        }
        self.posture
    }

    /// What the sentinel is revealing right now — `None` when calm.
    pub fn alarm(&self) -> Option<Alarm> {
        match self.posture {
            Posture::Calm => None,
            p => self.last_threat.map(|source| Alarm { source, via: self.via, posture: p }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i64, z: i64) -> Vec3 {
        Vec3::new(Fixed::from_int(x), Fixed::ZERO, Fixed::from_int(z))
    }

    #[test]
    fn sight_alarms_and_reveals_the_source() {
        let s = Senses::bird();
        let mut sen = Sentinel::new();
        sen.perceive(Some(p(10, 0)), None, &s);
        assert_eq!(sen.posture, Posture::Alarmed);
        let a = sen.alarm().unwrap();
        assert_eq!(a.via, Sense::Sight);
        assert_eq!(a.source, p(10, 0));
    }

    #[test]
    fn loud_sound_alarms_faint_sound_only_worries() {
        let s = Senses::bird();
        let mut faint = Sentinel::new();
        faint.perceive(None, Some((p(5, 5), s.hear_threshold)), &s);
        assert_eq!(faint.posture, Posture::Wary);

        let mut loud = Sentinel::new();
        loud.perceive(None, Some((p(5, 5), s.hear_threshold * 3)), &s);
        assert_eq!(loud.posture, Posture::Alarmed);
        assert_eq!(loud.alarm().unwrap().via, Sense::Hearing);
    }

    #[test]
    fn subthreshold_sound_is_ignored() {
        let s = Senses::bird();
        let mut sen = Sentinel::new();
        sen.perceive(None, Some((p(5, 5), s.hear_threshold - 1)), &s);
        assert_eq!(sen.posture, Posture::Calm);
        assert!(sen.alarm().is_none());
    }

    #[test]
    fn sight_beats_hearing_when_both_fire() {
        let s = Senses::bird();
        let mut sen = Sentinel::new();
        sen.perceive(Some(p(2, 0)), Some((p(9, 9), s.hear_threshold * 3)), &s);
        assert_eq!(sen.via, Sense::Sight);
        assert_eq!(sen.alarm().unwrap().source, p(2, 0));
    }

    #[test]
    fn alarm_decays_to_calm() {
        let s = Senses::bird();
        let mut sen = Sentinel::new();
        sen.perceive(Some(p(1, 1)), None, &s);
        for _ in 0..ALARM_TICKS {
            assert_ne!(sen.posture, Posture::Calm);
            sen.tick();
        }
        assert_eq!(sen.posture, Posture::Calm);
        assert!(sen.alarm().is_none());
    }

    #[test]
    fn state_is_copy_for_rollback() {
        let s = Senses::bird();
        let mut sen = Sentinel::new();
        sen.perceive(Some(p(3, 0)), None, &s);
        let snapshot = sen; // Copy
        sen.tick();
        // The snapshot is independent of later mutation.
        assert_eq!(snapshot.alert, ALARM_TICKS);
    }
}

// ── service_registry wiring (S2) ───────────────────────────────────
// Wires the sentinel watcher. Neutral: default senses, tick() returns a
// Posture each frame — proves wiring.
use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use linkme::distributed_slice;

struct SentinelService(Sentinel);
impl Service for SentinelService {
    fn name(&self) -> &'static str {
        "sentinel"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        let _ = self.0.tick();
    }
}
#[distributed_slice(SERVICES)]
static SENTINEL_SERVICE: ServiceEntry = ServiceEntry {
    name: "sentinel",
    make: || Box::new(SentinelService(Sentinel::new())),
};
