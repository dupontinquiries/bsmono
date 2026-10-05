//! castguide.rs — showing a player what an ability is about to do.
//!
//! Every kit's abilities already declare a verb, a range and a power
//! ([`crate::ancestor`]), which is enough to say what shape belongs on the
//! ground before the paw comes down: a ring around you, a circle where you are
//! aiming, a line you will travel, a cone you will yowl down. This module turns
//! a kit into those shapes, into the words that explain them, and into the small
//! state machine that decides *when* a press becomes a cast.
//!
//! Headless, like the rest of the player-facing logic here, and for the same
//! reason: the interesting parts are the rules — does a press cast immediately
//! or arm a reticle, can it be cancelled, what does the guide say — and rules
//! are worth testing without a renderer. What is left for `app.rs` is drawing a
//! disc, a quad and some text where this says to.
//!
//! ## Cast modes, and why all three exist
//!
//! * [`CastMode::Quick`] — press and it happens, aimed wherever you were
//!   already facing. What most players end up on, and the only one that feels
//!   right with a controller's face buttons.
//! * [`CastMode::Normal`] — press to arm, move to aim, press again to throw,
//!   and a cancel that does not spend the cooldown. Slower and more accurate;
//!   the mode a phone needs, because a thumb cannot aim and press at once.
//! * [`CastMode::Instant`] — like Quick, but it also ignores the guide. For
//!   players who have learned the ranges and want the ground clean.
//!
//! The guides can be turned off independently of the cast mode, because those
//! are two different preferences: some players want the reticle and quick cast,
//! some want normal cast and a bare field.

use crate::ancestor::{self, Ancestor, AbilitySpec, Verb};

/// The shape to draw on the ground.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GuideShape {
    /// A ring centred on the caster: everything within `r` is caught.
    SelfRing { r: f32 },
    /// A disc at the aim point, `r` across, no further than `max` away.
    Circle { x: f32, z: f32, r: f32, max: f32 },
    /// A path from the caster to the aim point, `w` wide — a dash or a lunge.
    Line { tx: f32, tz: f32, w: f32 },
    /// A wedge from the caster, `half` radians either side of the aim.
    Cone { half: f32, r: f32 },
    /// A single target under the reticle, up to `max` away.
    Target { x: f32, z: f32, max: f32 },
}

/// A guide, ready to draw: a shape, whether it is castable right now, and the
/// words that go beside it.
#[derive(Clone, Debug)]
pub struct Guide {
    pub shape: GuideShape,
    /// False when the ability is on cooldown — the renderer greys the shape
    /// rather than hiding it, because a range you cannot see is a range you
    /// cannot learn.
    pub ready: bool,
    pub cooldown: f32,
    pub tooltip: Tooltip,
}

/// What a player is told about an ability, in the order they want it: what it is
/// called, what it does, and then the numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct Tooltip {
    pub name: &'static str,
    pub desc: &'static str,
    /// "range 13 · 2.2x claw · 1.8s · cooldown 58s" — assembled here so two
    /// screens cannot describe the same ability differently.
    pub stats: String,
    /// The passive line, on the third slot only, where there is room for it.
    pub note: Option<String>,
}

/// How a press becomes a cast.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CastMode {
    #[default]
    Quick,
    Normal,
    Instant,
}

impl CastMode {
    pub fn name(self) -> &'static str {
        match self {
            CastMode::Quick => "Quick (press to cast)",
            CastMode::Normal => "Normal (press, aim, press)",
            CastMode::Instant => "Instant (no guide)",
        }
    }
    pub fn next(self) -> CastMode {
        match self {
            CastMode::Quick => CastMode::Normal,
            CastMode::Normal => CastMode::Instant,
            CastMode::Instant => CastMode::Quick,
        }
    }
    /// Does this mode draw a guide while aiming?
    pub fn shows_guide(self) -> bool {
        self != CastMode::Instant
    }
}

/// What a press did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CastEvent {
    /// Nothing yet — the reticle is up and waiting for the second press.
    Armed(usize),
    /// Send this slot to the sim.
    Fire(usize),
    /// The armed slot was dropped; no cooldown was spent.
    Cancelled(usize),
    Nothing,
}

/// The little machine between a button and `Battleground::set_input`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Caster {
    pub mode: CastMode,
    pub armed: Option<usize>,
    pub show_guides: bool,
}

impl Caster {
    pub fn new() -> Caster {
        Caster { mode: CastMode::Quick, armed: None, show_guides: true }
    }

    /// A press of ability `slot`.
    pub fn press(&mut self, slot: usize) -> CastEvent {
        let slot = slot.min(2);
        match self.mode {
            CastMode::Quick | CastMode::Instant => CastEvent::Fire(slot),
            CastMode::Normal => match self.armed {
                // pressing the armed slot again throws it
                Some(s) if s == slot => {
                    self.armed = None;
                    CastEvent::Fire(slot)
                }
                // pressing a *different* slot re-aims rather than casting both,
                // which is what a player means by it and what stops a fumbled
                // thumb from spending two cooldowns
                _ => {
                    self.armed = Some(slot);
                    CastEvent::Armed(slot)
                }
            },
        }
    }

    /// The cancel button, or a tap outside the reticle on a phone.
    pub fn cancel(&mut self) -> CastEvent {
        match self.armed.take() {
            Some(s) => CastEvent::Cancelled(s),
            None => CastEvent::Nothing,
        }
    }

    /// Which slot's guide to draw right now, if any: the armed one in normal
    /// cast, or the one under the player's finger while a button is held.
    pub fn guide_slot(&self, held: Option<usize>) -> Option<usize> {
        if !self.show_guides || !self.mode.shows_guide() {
            return None;
        }
        self.armed.or(held)
    }
}

fn spec(a: Ancestor, slot: usize) -> AbilitySpec {
    let k = ancestor::kit(a);
    if slot >= 2 {
        k.ult
    } else {
        k.abilities[slot]
    }
}

/// The words for one ability.
pub fn tooltip(a: Ancestor, slot: usize) -> Tooltip {
    let s = spec(a, slot);
    let k = ancestor::kit(a);
    let mut stats = String::new();
    if s.range > 0.0 {
        stats.push_str(&format!("range {:.0}", s.range));
    }
    if s.power > 0.0 {
        if !stats.is_empty() {
            stats.push_str(" · ");
        }
        // power is in claws, because that is the unit a player can feel: it
        // scales with the same attack their basic strike uses
        stats.push_str(&format!("{:.1}x claw", s.power));
    }
    if s.dur > 0.0 {
        if !stats.is_empty() {
            stats.push_str(" · ");
        }
        stats.push_str(&format!("{:.1}s", s.dur));
    }
    if !stats.is_empty() {
        stats.push_str(" · ");
    }
    stats.push_str(&format!("cooldown {:.0}s", s.cd));
    Tooltip {
        name: s.name,
        desc: s.desc,
        stats,
        note: if slot >= 2 {
            Some(format!("{} — {}", k.passive_name, k.passive))
        } else {
            None
        },
    }
}

/// The shape an ability puts on the ground, given where the caster stands and
/// where they are aiming. `aim` is a world point — a cursor, a stick direction
/// projected out, or a thumb's drag.
pub fn shape(a: Ancestor, slot: usize, from: (f32, f32), aim: (f32, f32)) -> GuideShape {
    let s = spec(a, slot);
    let (dx, dz) = (aim.0 - from.0, aim.1 - from.1);
    let d = (dx * dx + dz * dz).sqrt();
    // clamp the aim into range: a guide that shows a target it cannot reach is
    // a guide that lies
    let (cx, cz) = if d > s.range && d > 1e-4 {
        (from.0 + dx / d * s.range, from.1 + dz / d * s.range)
    } else {
        aim
    };
    match s.verb {
        // things that happen around you
        Verb::Aoe | Verb::Knockback | Verb::Fear | Verb::Slow | Verb::Shield | Verb::Heal => {
            GuideShape::SelfRing { r: s.range }
        }
        // ground you choose
        Verb::Zone | Verb::Rain => GuideShape::Circle { x: cx, z: cz, r: s.range * 0.6, max: s.range },
        // you travel
        Verb::Dash => GuideShape::Line { tx: cx, tz: cz, w: 2.2 },
        // the yowl goes where you look
        Verb::Root | Verb::Pull => GuideShape::Cone { half: 0.6, r: s.range },
        // one cat, under the reticle
        Verb::Strike | Verb::Execute | Verb::Stealth => GuideShape::Target { x: cx, z: cz, max: s.range },
    }
}

/// Everything a renderer needs for one slot.
pub fn guide(
    a: Ancestor,
    slot: usize,
    from: (f32, f32),
    aim: (f32, f32),
    cooldown: f32,
) -> Guide {
    Guide {
        shape: shape(a, slot, from, aim),
        ready: cooldown <= 0.0,
        cooldown,
        tooltip: tooltip(a, slot),
    }
}

/// How far back the camera sits. A clan war is read at a distance — you are
/// watching a lane, not a cat — but the old capture-mode 34 was far enough that
/// a Heal Tree and the warrior gnawing it were the same handful of pixels.
/// Closer, with room to pull back out for the player who wants the lane.
pub const CAM_NEAR: f32 = 20.0;
pub const CAM_FAR: f32 = 34.0;
pub const CAM_DEFAULT: f32 = 24.0;

/// Camera distance for a zoom setting of 0..=4, so the menu row and the
/// renderer agree on what "zoom 2" means.
pub fn camera_dist(zoom: u8) -> f32 {
    let t = (zoom.min(4) as f32) / 4.0;
    CAM_NEAR + (CAM_FAR - CAM_NEAR) * t
}

/// The same zoom as an OFFSET on a mode's own tuned distance: zoom 1 (the
/// menu's default) is that distance exactly, 0 pulls in 3.5 m, 4 pushes out
/// 10.5 m. The hunt and Prowl sit at 20 m and Pop Island at 34 m by design,
/// so one absolute distance would move all three off their defaults.
pub fn camera_offset(zoom: u8) -> f32 {
    camera_dist(zoom) - camera_dist(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_cast_fires_and_normal_cast_arms_then_fires() {
        let mut c = Caster::new();
        assert_eq!(c.press(0), CastEvent::Fire(0));
        assert_eq!(c.armed, None, "quick cast never leaves a reticle up");

        c.mode = CastMode::Normal;
        assert_eq!(c.press(1), CastEvent::Armed(1));
        assert_eq!(c.press(1), CastEvent::Fire(1));
        assert_eq!(c.armed, None);

        // a fumbled thumb on a different slot re-aims rather than casting both
        assert_eq!(c.press(0), CastEvent::Armed(0));
        assert_eq!(c.press(2), CastEvent::Armed(2));
        assert_eq!(c.press(2), CastEvent::Fire(2));

        // cancelling spends nothing
        assert_eq!(c.press(0), CastEvent::Armed(0));
        assert_eq!(c.cancel(), CastEvent::Cancelled(0));
        assert_eq!(c.cancel(), CastEvent::Nothing);
        assert_eq!(c.armed, None);
    }

    #[test]
    fn the_guide_can_be_turned_off_without_changing_how_casting_works() {
        // two preferences, not one: a player may want quick cast WITH a reticle,
        // or normal cast with a bare field
        let mut c = Caster::new();
        assert_eq!(c.guide_slot(Some(1)), Some(1));
        c.show_guides = false;
        assert_eq!(c.guide_slot(Some(1)), None);
        assert_eq!(c.press(1), CastEvent::Fire(1), "casting is unaffected");

        c.show_guides = true;
        c.mode = CastMode::Instant;
        assert_eq!(c.guide_slot(Some(1)), None, "instant means a clean field");
        c.mode = CastMode::Normal;
        c.press(2);
        assert_eq!(c.guide_slot(None), Some(2), "an armed slot shows its guide");
        // and the mode cycles round for a settings row
        assert_eq!(CastMode::Quick.next().next().next(), CastMode::Quick);
    }

    #[test]
    fn every_ability_in_the_roster_has_a_shape_and_words() {
        for a in ancestor::ALL {
            for slot in 0..3 {
                let g = guide(a, slot, (0.0, 0.0), (5.0, 5.0), 0.0);
                let t = &g.tooltip;
                assert!(!t.name.is_empty() && !t.desc.is_empty(), "{a:?} slot {slot}");
                assert!(t.stats.contains("cooldown"), "{}: {}", t.name, t.stats);
                assert!(g.ready);
                // the ultimate carries the passive, where there is room for it
                assert_eq!(t.note.is_some(), slot == 2, "{}: note on slot {slot}", t.name);
            }
        }
    }

    #[test]
    fn a_guide_never_shows_a_reach_the_ability_does_not_have() {
        // aiming past the end of an ability must clamp, or the guide is lying
        let a = Ancestor::SunCat; // Sunspear, range 14
        let far = shape(a, 0, (0.0, 0.0), (500.0, 0.0));
        match far {
            GuideShape::Target { x, max, .. } => {
                assert!((x - max).abs() < 0.01, "clamped to the edge: {x} vs {max}");
            }
            s => panic!("a strike should be a target, got {s:?}"),
        }
        // and a self-centred ability ignores the aim entirely
        let ring = shape(Ancestor::Lion, 1, (3.0, 4.0), (99.0, 99.0));
        assert!(matches!(ring, GuideShape::SelfRing { .. }), "{ring:?}");
        // a dash draws the path it will travel
        assert!(matches!(
            shape(Ancestor::Leopard, 0, (0.0, 0.0), (3.0, 0.0)),
            GuideShape::Line { .. }
        ));
        // scorched ground is a disc you place
        assert!(matches!(
            shape(Ancestor::SunCat, 1, (0.0, 0.0), (4.0, 0.0)),
            GuideShape::Circle { .. }
        ));
    }

    #[test]
    fn a_cooling_ability_still_shows_its_range() {
        // greyed, not hidden: a range you cannot see is a range you cannot learn
        let g = guide(Ancestor::Lion, 2, (0.0, 0.0), (1.0, 0.0), 12.5);
        assert!(!g.ready);
        assert_eq!(g.cooldown, 12.5);
        assert!(matches!(g.shape, GuideShape::SelfRing { r } if r > 0.0));
    }

    #[test]
    fn the_camera_zoom_row_and_the_renderer_agree() {
        assert_eq!(camera_dist(0), CAM_NEAR);
        assert_eq!(camera_dist(4), CAM_FAR);
        assert!(camera_dist(2) > CAM_NEAR && camera_dist(2) < CAM_FAR);
        assert_eq!(camera_dist(9), CAM_FAR, "a bad setting clamps rather than flies");
        // the offset form: the default zoom leaves a mode's own distance alone
        assert_eq!(camera_offset(1), 0.0);
        assert!(camera_offset(0) < 0.0 && camera_offset(4) > camera_offset(2) && camera_offset(2) > 0.0);
        assert_eq!(camera_offset(9), camera_offset(4));
        // the default sits nearer than the old capture-mode distance
        assert!(CAM_DEFAULT < 34.0);
    }
}
