//! `sim::dragon_duel` — one dragon's fight with the player, as a pure stepper.
//!
//! Three layers, each usable alone:
//!
//! 1. **Identity** — [`DuelProfile::from_genome`]: everything combat needs from
//!    the dragon's `dragon_archetype::Genome`, computed once at spawn. Element,
//!    hit points, hide, the build's weak spot, breath, bite, and three
//!    temperament scalars (`cunning`, `ferocity`, `dread`).
//! 2. **Items** — [`resolve_item_hit`]: what a player's item does to *this*
//!    dragon. Element matchup (`elemental_multiplier`, the same 6-cycle the
//!    genome and the forged weapons share), where the blow landed
//!    ([`hit_zone`]) against the build's weak spot, and hide mitigation.
//! 3. **Tactics** — [`step`]: a utility-scored choice between stalking,
//!    circling for the flank, feinting to bait a swing, lunging, breathing
//!    from outside the player's reach, and retreating. The dragon *learns* from
//!    the fight ([`DuelMemory`]): which element hurt it, how far the player's
//!    item reaches, whether its feints drew whiffs. Every step returns a
//!    [`Why`] so the shell can log the decision and the numbers behind it.
//!
//! The genome carries no intellect gene (`DRAGON_GENOME.md`), so the three
//! temperament scalars are a *mapping this module defines* from skull and
//! body genes; see [`DuelProfile::from_genome`]. They are deterministic in the
//! genome, so a bred dragon inherits its parents' temperament through the
//! same genes that shape its face.
//!
//! Pure: dt + senses in, no clock, RNG, or IO. Never panics on non-finite input.

use crate::sim::breath_attack::{self, BreathParams};
use crate::sim::dragon_archetype::{self as arche, Build, Genome, NUM_ELEMENTS};
use crate::sim::elemental_multiplier;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

// ─────────────────────────────────────────────────────────────────────────────
// Identity
// ─────────────────────────────────────────────────────────────────────────────

/// Where a build is soft. A blow landing in the matching [`HitZone`] gets the
/// weak-spot multiplier and half the hide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeakSpot {
    /// Front — you must stand in front of the jaws to reach it.
    Throat,
    /// Flank — where the wing meets the shoulder.
    WingRoot,
    /// Underneath — reachable when it is above you (landing, low pass).
    Belly,
    /// Rear — the tail base, for the one who gets behind it.
    TailBase,
}

/// Which side of the dragon a blow arrived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitZone {
    Front,
    Flank,
    Rear,
    Under,
}

impl WeakSpot {
    pub fn zone(self) -> HitZone {
        match self {
            WeakSpot::Throat => HitZone::Front,
            WeakSpot::WingRoot => HitZone::Flank,
            WeakSpot::Belly => HitZone::Under,
            WeakSpot::TailBase => HitZone::Rear,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            WeakSpot::Throat => "throat",
            WeakSpot::WingRoot => "wing root",
            WeakSpot::Belly => "belly",
            WeakSpot::TailBase => "tail base",
        }
    }
}

/// Everything the fight needs from a genome. Computed once; cheap to copy.
#[derive(Debug, Clone, Copy)]
pub struct DuelProfile {
    /// Dominant element, `0=Wood..5=Storm` (genome / weapon palette).
    pub element: u8,
    pub build: Build,
    pub size_class: u8,
    pub max_hp: i32,
    /// Armor points for `raw·K/(hide+K)` mitigation, `K =` [`HIDE_K`].
    pub hide: f32,
    pub weak_spot: WeakSpot,
    /// 0..1. Gates feints and flanking; makes the dragon read your item.
    pub cunning: f32,
    /// 0..1. Baseline willingness to close and bite.
    pub ferocity: f32,
    /// 0..1. How frightening its presence is (roar, nerve pressure).
    pub dread: f32,
    pub breath: BreathParams,
    /// Damage of one landed bite/claw (same units as breath `base_damage`).
    pub bite_damage: f32,
}

/// Mitigation constant for hide, matching `damage_after_armor`'s shape.
pub const HIDE_K: f32 = 20.0;
/// Multiplier for a blow that lands on the build's weak spot.
pub const WEAK_SPOT_MULT: f32 = 1.75;
/// Fuel one breath costs (fuel is 0..1).
pub const BREATH_COST: f32 = 0.45;
/// Fuel regained per second.
pub const BREATH_RECHARGE: f32 = 0.08;

#[inline]
fn c01(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

impl DuelProfile {
    /// The temperament mapping (this module's, not the genome doc's):
    ///
    /// * `cunning`  = heavy brow (a big braincase) + a long neck (wide gaze)
    ///   + a light frame that must win by wit; Stalkers +0.15.
    /// * `ferocity` = teeth + jaw depth + bulk.
    /// * `dread`    = size + horns + teeth + crest — the things you see first.
    pub fn from_genome(g: &Genome) -> DuelProfile {
        let a = arche::classify(g);
        let size01 = a.size_class as f32 / arche::SIZE_CLASS_MAX as f32;
        let stalker = if a.build == Build::Stalker { 0.15 } else { 0.0 };
        let cunning = c01(0.5 * c01(g.brow) + 0.2 * c01(g.neck_len) + 0.3 * (1.0 - c01(g.bulk)) + stalker);
        let ferocity = c01(0.4 * c01(g.teeth) + 0.3 * c01(g.jaw_depth) + 0.3 * c01(g.bulk));
        let dread = c01(0.35 * size01 + 0.25 * c01(g.horns) + 0.25 * c01(g.teeth) + 0.15 * c01(g.crest));
        let weak_spot = match a.build {
            Build::Serpentine => WeakSpot::Throat,
            Build::Bruiser | Build::Sovereign => WeakSpot::Belly,
            Build::Flyer => WeakSpot::WingRoot,
            Build::Stalker => WeakSpot::TailBase,
        };
        let breath = breath_attack::params_from(g);
        DuelProfile {
            element: a.element,
            build: a.build,
            size_class: a.size_class,
            max_hp: (120.0 + 90.0 * a.size_class as f32 + 60.0 * c01(g.bulk)).round() as i32,
            hide: 6.0 + 6.0 * a.size_class as f32 + 6.0 * c01(g.brow),
            weak_spot,
            cunning,
            ferocity,
            dread,
            breath,
            bite_damage: (6.0 + 14.0 * ferocity) * (0.7 + 0.15 * a.size_class as f32),
        }
    }

    pub fn element_name(&self) -> &'static str {
        arche::ELEMENT_NAMES[self.element as usize % NUM_ELEMENTS]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Items against the dragon
// ─────────────────────────────────────────────────────────────────────────────

/// What the player is holding, as far as the dragon is concerned. The shell
/// maps its weapon type onto this; the core never sees the item system.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemThreat {
    /// `0=Wood..5=Storm`, or `None` for an unaligned item / bare hands.
    pub element: Option<u8>,
    pub damage: f32,
    pub reach: f32,
}

impl ItemThreat {
    pub const BARE_HANDS: ItemThreat = ItemThreat { element: None, damage: 4.0, reach: 1.2 };
}

/// Which zone a blow from `attacker` lands in. `yaw` follows the engine's
/// convention: forward is `(cos yaw, sin yaw)` in x/z. `half_height` is the
/// body's vertical half-extent; an attacker more than 60% of it below the
/// centre and within `radius` horizontally is striking the underside.
pub fn hit_zone(dragon_pos: [f32; 3], dragon_yaw: f32, half_height: f32, radius: f32, attacker: [f32; 3]) -> HitZone {
    let dx = attacker[0] - dragon_pos[0];
    let dz = attacker[2] - dragon_pos[2];
    let dy = attacker[1] - dragon_pos[1];
    let horiz = (dx * dx + dz * dz).sqrt();
    if dy < -0.6 * half_height.max(0.1) && horiz <= radius.max(0.1) {
        return HitZone::Under;
    }
    if horiz < 1e-4 {
        return HitZone::Under;
    }
    let off = wrap(dz.atan2(dx) - dragon_yaw).abs();
    if off < 0.9 {
        HitZone::Front
    } else if off > PI - 0.9 {
        HitZone::Rear
    } else {
        HitZone::Flank
    }
}

/// The resolved blow, with the parts shown so a HUD or log can say *why*.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemHit {
    /// `elemental_multiplier(item, dragon)`: 2.0 counters, 0.5 is resisted.
    pub element_mult: f32,
    pub weak_spot: bool,
    /// Hide that applied after the weak-spot halving.
    pub hide: f32,
    pub damage: i32,
}

pub fn resolve_item_hit(p: &DuelProfile, item: &ItemThreat, zone: HitZone) -> ItemHit {
    let element_mult = item.element.map_or(1.0, |e| elemental_multiplier::multiplier(e, p.element));
    let weak_spot = zone == p.weak_spot.zone();
    let hide = if weak_spot { p.hide * 0.5 } else { p.hide };
    let raw = item.damage.max(0.0) * element_mult * if weak_spot { WEAK_SPOT_MULT } else { 1.0 };
    let dealt = raw * HIDE_K / (hide.max(0.0) + HIDE_K);
    ItemHit { element_mult, weak_spot, hide, damage: dealt.round().max(1.0) as i32 }
}

// ─────────────────────────────────────────────────────────────────────────────
// Memory: what the fight has taught this dragon
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DuelMemory {
    /// Recent pain per incoming element, as a fraction of max hp. Decays.
    pub pain: [f32; NUM_ELEMENTS],
    /// Pain from unaligned blows (bare hands, plain tools).
    pub pain_plain: f32,
    /// Longest reach it has been struck from, remembered (slow decay).
    pub reach_seen: f32,
    pub hits_taken: u16,
    /// Swings a feint drew that hit nothing. Raises feint preference.
    pub whiffs_baited: u16,
}

/// Seconds for remembered pain to halve.
pub const PAIN_HALF_LIFE: f32 = 25.0;

impl DuelMemory {
    pub const fn new() -> DuelMemory {
        DuelMemory { pain: [0.0; NUM_ELEMENTS], pain_plain: 0.0, reach_seen: 0.0, hits_taken: 0, whiffs_baited: 0 }
    }

    /// Record a landed blow: `dmg` against `max_hp`, from `dist` away.
    pub fn learn_hit(&mut self, element: Option<u8>, dmg: i32, max_hp: i32, dist: f32) {
        let frac = dmg.max(0) as f32 / max_hp.max(1) as f32;
        match element {
            Some(e) => self.pain[e as usize % NUM_ELEMENTS] += frac,
            None => self.pain_plain += frac,
        }
        if dist.is_finite() {
            self.reach_seen = self.reach_seen.max(dist);
        }
        self.hits_taken = self.hits_taken.saturating_add(1);
    }

    pub fn decay(&mut self, dt: f32) {
        let k = 0.5f32.powf(dt.max(0.0) / PAIN_HALF_LIFE);
        for p in self.pain.iter_mut() {
            *p *= k;
        }
        self.pain_plain *= k;
        // Reach is a fact about the player's kit, not a feeling: fade slowly.
        self.reach_seen *= 0.5f32.powf(dt.max(0.0) / (PAIN_HALF_LIFE * 8.0));
    }

    pub fn total_pain(&self) -> f32 {
        self.pain.iter().sum::<f32>() + self.pain_plain
    }
}

impl Default for DuelMemory {
    fn default() -> Self {
        DuelMemory::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tactics
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tactic {
    /// Not engaged; the shell's ordinary behaviour runs.
    Unaware,
    /// Closing while you are not looking.
    Stalk,
    /// Orbiting outside your reach, working toward your flank.
    Circle,
    /// Darting in and out to draw a swing.
    Feint,
    /// Committing to a bite.
    Lunge,
    /// Holding range and breathing.
    Breath,
    /// Breaking off to live.
    Retreat,
}

impl Tactic {
    pub fn name(self) -> &'static str {
        match self {
            Tactic::Unaware => "unaware",
            Tactic::Stalk => "stalk",
            Tactic::Circle => "circle",
            Tactic::Feint => "feint",
            Tactic::Lunge => "lunge",
            Tactic::Breath => "breath",
            Tactic::Retreat => "retreat",
        }
    }
}

/// Steering knobs, for a person tuning the fight live. `1.0` everywhere is
/// the genome's own dragon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DuelTuning {
    pub cunning: f32,
    pub aggression: f32,
    pub dread: f32,
}

impl Default for DuelTuning {
    fn default() -> Self {
        DuelTuning { cunning: 1.0, aggression: 1.0, dread: 1.0 }
    }
}

impl DuelTuning {
    /// Parse `"cunning=1.5,aggression=0.8,dread=0"`. Unknown keys and bad
    /// numbers are skipped and returned, so the shell can say what it ignored.
    pub fn parse(s: &str) -> (DuelTuning, Vec<String>) {
        let mut t = DuelTuning::default();
        let mut ignored = Vec::new();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (k, v) = match part.split_once('=').or_else(|| part.split_once(':')) {
                Some(kv) => kv,
                None => {
                    ignored.push(part.to_string());
                    continue;
                }
            };
            let v: f32 = match v.trim().parse() {
                Ok(v) if f32::is_finite(v) && v >= 0.0 => v,
                _ => {
                    ignored.push(part.to_string());
                    continue;
                }
            };
            match k.trim() {
                "cunning" => t.cunning = v,
                "aggression" | "aggr" => t.aggression = v,
                "dread" => t.dread = v,
                _ => ignored.push(part.to_string()),
            }
        }
        (t, ignored)
    }
}

/// What the dragon perceives this tick.
#[derive(Debug, Clone, Copy)]
pub struct DuelSenses {
    /// Straight-line distance to the player (m).
    pub dist: f32,
    /// World yaw of the dragon (engine convention, see [`hit_zone`]).
    pub self_yaw: f32,
    /// World yaw from the dragon toward the player.
    pub to_player_yaw: f32,
    /// How squarely the player faces the dragon: `1` staring, `-1` back turned.
    pub player_facing: f32,
    /// Line of sight + not hidden.
    pub can_see: bool,
    /// What the player holds (`None` = the dragon can't tell; assume hands).
    pub item: Option<ItemThreat>,
    pub hp01: f32,
    pub player_vitality01: f32,
    /// The player swung this tick and hit nothing.
    pub player_whiffed: bool,
    /// The dragon's body radius (m), for reach arithmetic.
    pub body_radius: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DuelState {
    pub tactic: Tactic,
    /// Seconds in the current tactic.
    pub held: f32,
    pub engaged: bool,
    pub roared: bool,
    /// Breath fuel 0..1.
    pub fuel: f32,
    /// Seconds until another lunge may start.
    pub lunge_cd: f32,
    /// Seconds of opening after a baited whiff (lunges score high).
    pub opening: f32,
    /// +1 / -1: which way it circles. Flips when it has circled too long.
    pub orbit: f32,
    /// Feint phase: `true` while darting in.
    pub feint_in: bool,
    pub mem: DuelMemory,
}

impl DuelState {
    pub fn new(seed: u64) -> DuelState {
        DuelState {
            tactic: Tactic::Unaware,
            held: 0.0,
            engaged: false,
            roared: false,
            fuel: 1.0,
            lunge_cd: 0.0,
            opening: 0.0,
            orbit: if seed & 1 == 0 { 1.0 } else { -1.0 },
            feint_in: true,
            mem: DuelMemory::new(),
        }
    }
}

/// The numbers behind a decision, for the trace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Why {
    pub boldness: f32,
    /// What the player's item does to this dragon (element multiplier).
    pub counter: f32,
    /// Reach the dragon is respecting (m): the item's, or remembered.
    pub respect_reach: f32,
    /// Distance it wants to hold outside that reach.
    pub safe_gap: f32,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DuelOut {
    pub tactic: Tactic,
    pub changed: bool,
    /// World yaw to move along.
    pub heading: f32,
    /// 0..1 of the dragon's own top speed.
    pub speed01: f32,
    /// `Some(true)` take off, `Some(false)` land, `None` no opinion.
    pub want_air: Option<bool>,
    /// Breath fires this tick (fuel already spent).
    pub breathe: bool,
    /// A bite lands this tick if the player is within `body_radius + LUNGE_REACH`.
    pub bite: bool,
    /// Roar this tick (first engagement of a dreadful dragon).
    pub roar: bool,
    /// Nerve pressure on the player this tick, 0..1.
    pub dread: f32,
    pub why: Why,
    /// Every tactic's score this tick, so a log or HUD can show the runner-up
    /// and how close it was. All `-1.0` while unaware.
    pub scores: [(Tactic, f32); 6],
}

impl DuelOut {
    pub fn score(&self, t: Tactic) -> f32 {
        self.scores.iter().find(|(x, _)| *x == t).map_or(-1.0, |x| x.1)
    }
}

/// How far past its body a lunge reaches (m).
pub const LUNGE_REACH: f32 = 2.5;
/// Distance at which a dragon that can see you engages, before ferocity.
pub const ENGAGE_BASE: f32 = 28.0;
/// Distance past which an engaged dragon gives up (unless it was hurt).
pub const DISENGAGE: f32 = 70.0;
/// Radius of a dragon's nerve pressure (m).
pub const DREAD_RADIUS: f32 = 30.0;
const MIN_HOLD: f32 = 0.6;
const SWITCH_MARGIN: f32 = 0.15;
const LUNGE_TIME: f32 = 0.55;
const LUNGE_COOLDOWN: f32 = 2.2;

#[inline]
fn wrap(a: f32) -> f32 {
    if !a.is_finite() {
        return 0.0;
    }
    (a + PI).rem_euclid(TAU) - PI
}

/// One tick. See the module docs for the shape of the decision.
pub fn step(st: &mut DuelState, p: &DuelProfile, s: &DuelSenses, tune: DuelTuning, dt: f32) -> DuelOut {
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.5) } else { 0.0 };
    let dist = if s.dist.is_finite() { s.dist.max(0.0) } else { f32::MAX };
    let cunning = c01(p.cunning * tune.cunning);
    let ferocity = c01(p.ferocity * tune.aggression);
    let dread = c01(p.dread * tune.dread);

    st.mem.decay(dt);
    st.fuel = breath_attack::recharge(st.fuel, BREATH_RECHARGE, dt);
    st.lunge_cd = (st.lunge_cd - dt).max(0.0);
    st.opening = (st.opening - dt).max(0.0);
    st.held += dt;

    // ── engagement ──────────────────────────────────────────────────────────
    let was_engaged = st.engaged;
    let engage_at = ENGAGE_BASE + 20.0 * ferocity;
    if !st.engaged && ((s.can_see && dist < engage_at) || st.mem.hits_taken > 0) {
        st.engaged = true;
    }
    if st.engaged && dist > DISENGAGE && st.mem.total_pain() < 0.05 {
        st.engaged = false;
    }
    let roar = st.engaged && !was_engaged && !st.roared && dread > 0.3;
    if roar {
        st.roared = true;
    }

    // ── what the player's item means to *this* dragon ─────────────────────
    let item = s.item.unwrap_or(ItemThreat::BARE_HANDS);
    let counter = item.element.map_or(1.0, |e| elemental_multiplier::multiplier(e, p.element));
    // A cunning dragon respects the longest reach it has been hit from, even
    // if the player has switched to something shorter.
    let respect_reach = item.reach.max(st.mem.reach_seen * cunning);
    let safe_gap = s.body_radius.max(0.5) + respect_reach + 1.5;
    let lunge_range = s.body_radius.max(0.5) + LUNGE_REACH;
    // Pain from the element in hand matters more than pain in general.
    let pain_now = item.element.map_or(st.mem.pain_plain, |e| st.mem.pain[e as usize % NUM_ELEMENTS]);

    let boldness = ferocity
        + (c01(s.hp01) - 0.5) * 0.6
        - (counter - 1.0) * 0.35 * (0.3 + cunning)
        - pain_now * 1.5 * cunning
        - st.mem.total_pain() * 0.4
        + (1.0 - c01(s.player_vitality01)) * 0.4;

    let dread_out = if st.engaged {
        dread * c01(1.0 - dist / DREAD_RADIUS) * if st.tactic == Tactic::Stalk { 0.3 } else { 1.0 }
    } else {
        0.0
    };

    if !st.engaged {
        let changed = st.tactic != Tactic::Unaware;
        if changed {
            st.tactic = Tactic::Unaware;
            st.held = 0.0;
        }
        return DuelOut {
            tactic: Tactic::Unaware,
            changed,
            heading: s.self_yaw,
            speed01: 0.0,
            want_air: None,
            breathe: false,
            bite: false,
            roar: false,
            dread: 0.0,
            why: Why { boldness, counter, respect_reach, safe_gap, reason: "not engaged" },
            scores: [
                (Tactic::Stalk, -1.0),
                (Tactic::Circle, -1.0),
                (Tactic::Feint, -1.0),
                (Tactic::Lunge, -1.0),
                (Tactic::Breath, -1.0),
                (Tactic::Retreat, -1.0),
            ],
        };
    }

    if s.player_whiffed && st.tactic == Tactic::Feint {
        st.mem.whiffs_baited = st.mem.whiffs_baited.saturating_add(1);
        st.opening = 1.2;
    }

    // ── score every tactic ──────────────────────────────────────────────────
    let watched = s.player_facing > 0.5;
    let flanked = s.player_facing < 0.2;
    let breath_ok = st.fuel >= BREATH_COST && dist <= p.breath.max_range && dist > lunge_range * 0.8;
    let retreat_hp = 0.18 + 0.2 * cunning;

    let mut score = [(Tactic::Stalk, f32::MIN); 6];
    score[0] = (Tactic::Stalk, if !watched && dist > lunge_range { 0.4 + 0.4 * cunning } else { -1.0 });
    score[1] = (Tactic::Circle, if watched { 0.35 + 0.3 * cunning - boldness * 0.2 } else { 0.1 });
    score[2] = (
        Tactic::Feint,
        if cunning > 0.45 && watched && dist < safe_gap + 5.0 && st.lunge_cd <= 0.0 {
            // A feint that draws nothing goes stale: after a few seconds
            // without a whiff, something more direct wins.
            let stale = if st.tactic == Tactic::Feint { st.held * 0.15 } else { 0.0 };
            // Feinting brings it to the edge of your reach; an item that
            // counters it makes that edge a worse place to be.
            0.3 + cunning * 0.5 + st.mem.whiffs_baited.min(4) as f32 * 0.08
                - stale
                - (counter - 1.0).max(0.0) * 0.3
        } else {
            -1.0
        },
    );
    score[3] = (
        Tactic::Lunge,
        if st.lunge_cd <= 0.0 && dist < lunge_range + 2.0 {
            // Lunging at a player who is facing it is lunging onto the weapon.
            boldness + if flanked { 0.4 } else if watched { -0.35 } else { 0.0 }
                + if st.opening > 0.0 { 0.8 } else { 0.0 }
        } else {
            -1.0
        },
    );
    score[4] = (
        Tactic::Breath,
        if breath_ok {
            // Prefer breath when the item out-reaches it or counters it: fight
            // from where the item can't answer.
            0.45 + (respect_reach - 1.5).max(0.0) * 0.15 + (counter - 1.0).max(0.0) * 0.4 * cunning
        } else {
            -1.0
        },
    );
    score[5] = (
        Tactic::Retreat,
        if s.hp01 < retreat_hp { 1.2 + (retreat_hp - s.hp01) * 3.0 } else { -1.0 + (-boldness).max(0.0) },
    );

    // Actions finish before anything else is considered.
    let committed = match st.tactic {
        Tactic::Lunge => st.held < LUNGE_TIME,
        Tactic::Breath => st.held < 0.8,
        _ => false,
    };
    let current = score.iter().find(|(t, _)| *t == st.tactic).map_or(f32::MIN, |x| x.1);
    let best = score
        .iter()
        .copied()
        .fold((Tactic::Circle, f32::MIN), |a, b| if b.1 > a.1 { b } else { a });
    let switch = !committed
        && best.0 != st.tactic
        && (st.tactic == Tactic::Unaware || st.held >= MIN_HOLD || current < 0.0)
        && best.1 > current + SWITCH_MARGIN;
    let prev = st.tactic;
    if switch {
        st.tactic = best.0;
        st.held = 0.0;
    }
    let changed = st.tactic != prev;

    // ── act ─────────────────────────────────────────────────────────────────
    let toward = s.to_player_yaw;
    let aim_off = wrap(toward - s.self_yaw).abs();
    let mut breathe = false;
    let mut bite = false;
    let mut want_air = None;
    let (heading, speed01, reason) = match st.tactic {
        Tactic::Stalk => (toward, 0.45, "your back is turned"),
        Tactic::Circle => {
            if st.held > 6.0 + 4.0 * cunning {
                st.orbit = -st.orbit;
                st.held = 0.0;
            }
            // Tangent plus a radial term that pulls toward `safe_gap + 2`.
            let radial = ((dist - (safe_gap + 2.0)) / 4.0).clamp(-1.0, 1.0);
            let h = toward + st.orbit * (FRAC_PI_2 - radial * 0.7);
            (h, 0.7, "holding outside your reach, working to your flank")
        }
        Tactic::Feint => {
            // By distance, not by clock: dart to just outside the reach it
            // respects, then back out past the safe gap. A timed feint drifts
            // inward whenever turning eats part of the out-phase.
            let edge = s.body_radius.max(0.5) + respect_reach + 0.5;
            if changed {
                st.feint_in = true;
            }
            if st.feint_in && dist <= edge {
                st.feint_in = false;
            } else if !st.feint_in && dist >= safe_gap + 1.0 {
                st.feint_in = true;
            }
            if st.feint_in { (toward, 0.9, "feinting in to draw a swing") } else { (toward + PI, 0.9, "pulling back from the feint") }
        }
        Tactic::Lunge => {
            if st.held < dt + 1e-6 || changed {
                st.lunge_cd = LUNGE_COOLDOWN;
            }
            if dist <= lunge_range && st.held >= LUNGE_TIME * 0.5 && st.held - dt < LUNGE_TIME * 0.5 {
                bite = true;
            }
            (toward, 1.0, if st.opening > 0.0 { "you swung at nothing — opening" } else if flanked { "you are not facing it" } else { "it thinks it can take you" })
        }
        Tactic::Breath => {
            if st.held < 0.4 {
                (toward, 0.1, "drawing breath")
            } else {
                if aim_off < p.breath.cone_half_angle && breath_attack::try_breathe(&mut st.fuel, BREATH_COST) {
                    breathe = true;
                }
                (toward, 0.0, if counter > 1.0 { "your item counters it: fighting from range" } else if respect_reach > 2.5 { "your reach is long: fighting from range" } else { "breathing" })
            }
        }
        Tactic::Retreat => {
            want_air = Some(true);
            (toward + PI, 1.0, "wounded, breaking off")
        }
        Tactic::Unaware => (s.self_yaw, 0.0, "not engaged"),
    };

    DuelOut {
        tactic: st.tactic,
        changed,
        heading: wrap(heading),
        speed01,
        want_air,
        breathe,
        bite,
        roar,
        dread: dread_out,
        why: Why { boldness, counter, respect_reach, safe_gap, reason },
        scores: score,
    }
}

/// One line for a log:
/// `circle (holding outside your reach…) bold 0.31 item×2.0 reach 3.4 gap 6.1 | next: feint 0.52`.
pub fn describe(o: &DuelOut) -> String {
    let runner_up = o
        .scores
        .iter()
        .filter(|(t, _)| *t != o.tactic)
        .fold((Tactic::Unaware, f32::MIN), |a, b| if b.1 > a.1 { *b } else { a });
    format!(
        "{} ({}) bold {:.2} item×{:.1} reach {:.1} gap {:.1} | next: {} {:.2}",
        o.tactic.name(),
        o.why.reason,
        o.why.boldness,
        o.why.counter,
        o.why.respect_reach,
        o.why.safe_gap,
        runner_up.0.name(),
        runner_up.1
    )
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn genome(el: u8) -> Genome {
        Genome::from_seed_element(0xD2A6_0000 + el as u64, el)
    }

    fn senses(dist: f32) -> DuelSenses {
        DuelSenses {
            dist,
            self_yaw: 0.0,
            to_player_yaw: 0.0,
            player_facing: 1.0,
            can_see: true,
            item: None,
            hp01: 1.0,
            player_vitality01: 1.0,
            player_whiffed: false,
            body_radius: 2.0,
        }
    }

    fn run(st: &mut DuelState, p: &DuelProfile, s: &DuelSenses, t: DuelTuning, secs: f32) -> Vec<DuelOut> {
        let n = (secs / 0.05) as usize;
        (0..n).map(|_| step(st, p, s, t, 0.05)).collect()
    }

    #[test]
    fn profile_is_deterministic_and_in_range() {
        for seed in 0..500u64 {
            let g = Genome::from_seed(seed);
            let a = DuelProfile::from_genome(&g);
            let b = DuelProfile::from_genome(&g);
            assert_eq!(a.max_hp, b.max_hp);
            assert_eq!(a.element, b.element);
            assert!((120..=540).contains(&a.max_hp), "hp {}", a.max_hp);
            for v in [a.cunning, a.ferocity, a.dread] {
                assert!((0.0..=1.0).contains(&v));
            }
            assert_eq!(a.element, breath_attack::dominant_element(&g));
        }
    }

    #[test]
    fn temperament_actually_varies_across_genomes() {
        // A mapping that collapsed to a constant would pass the range test.
        let ps: Vec<_> = (0..400u64).map(|s| DuelProfile::from_genome(&Genome::from_seed(s))).collect();
        let span = |f: &dyn Fn(&DuelProfile) -> f32| {
            let lo = ps.iter().map(f).fold(1.0, f32::min);
            let hi = ps.iter().map(f).fold(0.0, f32::max);
            hi - lo
        };
        assert!(span(&|p| p.cunning) > 0.4);
        assert!(span(&|p| p.ferocity) > 0.4);
        assert!(span(&|p| p.dread) > 0.4);
        let feinters = ps.iter().filter(|p| p.cunning > 0.45).count();
        assert!((40..=360).contains(&feinters), "feint-capable dragons: {feinters}/400");
    }

    #[test]
    fn countering_element_hits_twice_as_hard_and_resisted_half() {
        let p = DuelProfile::from_genome(&genome(4)); // Fire
        let base = ItemThreat { element: None, damage: 20.0, reach: 1.5 };
        let plain = resolve_item_hit(&p, &base, HitZone::Front).damage as f32;
        // Dust (3) beats the next element, Fire (4).
        let dust = resolve_item_hit(&p, &ItemThreat { element: Some(3), ..base }, HitZone::Front);
        // Storm (5) is beaten by... Fire is the PREVIOUS of Storm -> Storm resisted by Fire? multiplier(5,4): diff = -1 -> 5 -> 0.5
        let storm = resolve_item_hit(&p, &ItemThreat { element: Some(5), ..base }, HitZone::Front);
        assert_eq!(dust.element_mult, 2.0);
        assert_eq!(storm.element_mult, 0.5);
        assert!((dust.damage as f32 / plain - 2.0).abs() < 0.2);
        assert!((storm.damage as f32 / plain - 0.5).abs() < 0.2);
    }

    #[test]
    fn weak_spot_needs_the_right_zone() {
        let p = DuelProfile::from_genome(&genome(0));
        let it = ItemThreat { element: None, damage: 20.0, reach: 1.5 };
        let zones = [HitZone::Front, HitZone::Flank, HitZone::Rear, HitZone::Under];
        let hits: Vec<_> = zones.iter().map(|z| resolve_item_hit(&p, &it, *z)).collect();
        assert_eq!(hits.iter().filter(|h| h.weak_spot).count(), 1);
        let weak = hits.iter().find(|h| h.weak_spot).unwrap().damage;
        for h in hits.iter().filter(|h| !h.weak_spot) {
            assert!(weak as f32 > h.damage as f32 * 1.9, "weak {weak} vs {}", h.damage);
        }
    }

    #[test]
    fn hit_zone_geometry() {
        let c = [0.0, 10.0, 0.0];
        assert_eq!(hit_zone(c, 0.0, 2.0, 3.0, [5.0, 10.0, 0.0]), HitZone::Front);
        assert_eq!(hit_zone(c, 0.0, 2.0, 3.0, [-5.0, 10.0, 0.0]), HitZone::Rear);
        assert_eq!(hit_zone(c, 0.0, 2.0, 3.0, [0.0, 10.0, 5.0]), HitZone::Flank);
        assert_eq!(hit_zone(c, 0.0, 2.0, 3.0, [0.5, 7.0, 0.5]), HitZone::Under);
        // Below but far out to the side is a flank shot, not the belly.
        assert_eq!(hit_zone(c, 0.0, 2.0, 3.0, [0.0, 7.0, 8.0]), HitZone::Flank);
        // Yaw rotates the frame.
        assert_eq!(hit_zone(c, FRAC_PI_2, 2.0, 3.0, [0.0, 10.0, 5.0]), HitZone::Front);
    }

    #[test]
    fn unseen_player_far_away_stays_unaware() {
        let p = DuelProfile::from_genome(&genome(1));
        let mut st = DuelState::new(1);
        let mut s = senses(200.0);
        s.can_see = false;
        let outs = run(&mut st, &p, &s, DuelTuning::default(), 2.0);
        assert!(outs.iter().all(|o| o.tactic == Tactic::Unaware && !o.breathe && !o.bite));
    }

    #[test]
    fn being_hit_engages_even_unseen() {
        let p = DuelProfile::from_genome(&genome(1));
        let mut st = DuelState::new(1);
        st.mem.learn_hit(Some(2), 10, p.max_hp, 2.0);
        let mut s = senses(40.0);
        s.can_see = false;
        let o = step(&mut st, &p, &s, DuelTuning::default(), 0.05);
        assert!(st.engaged);
        assert_ne!(o.tactic, Tactic::Unaware);
    }

    #[test]
    fn dreadful_dragon_roars_exactly_once() {
        let mut p = DuelProfile::from_genome(&genome(4));
        p.dread = 0.9;
        let mut st = DuelState::new(3);
        let outs = run(&mut st, &p, &senses(15.0), DuelTuning::default(), 5.0);
        assert_eq!(outs.iter().filter(|o| o.roar).count(), 1);
        // Tuned to zero dread: no roar, no pressure.
        let mut st2 = DuelState::new(3);
        let quiet = DuelTuning { dread: 0.0, ..DuelTuning::default() };
        let outs2 = run(&mut st2, &p, &senses(15.0), quiet, 5.0);
        assert!(outs2.iter().all(|o| !o.roar && o.dread == 0.0));
    }

    /// A closed loop: the player stands at the origin, the dragon moves by its
    /// own output (turning toward `heading` at 3 rad/s, running at 7 m/s ×
    /// `speed01`), and the senses are recomputed from the geometry each tick.
    /// Static senses cannot test tactics — a dragon told "you are 8 m away"
    /// forever never gets to lunge range.
    struct Fight {
        /// Player turns toward the dragon at this rate (rad/s); 0 = frozen.
        track: f32,
        /// Player swings at anything that comes within this distance.
        swing_at: f32,
        item: Option<ItemThreat>,
        start: f32,
        secs: f32,
    }

    struct Tally {
        outs: Vec<DuelOut>,
        bites: usize,
        breaths: usize,
        whiffs: usize,
        min_dist: f32,
    }

    fn fight(p: &DuelProfile, st: &mut DuelState, f: &Fight) -> Tally {
        let dt = 0.05;
        let (mut x, mut z, mut yaw) = (f.start, 0.0f32, PI); // facing the player
        let mut pyaw = 0.0f32; // player faces +x, at the dragon
        let reach = f.item.unwrap_or(ItemThreat::BARE_HANDS).reach;
        let mut t = Tally { outs: vec![], bites: 0, breaths: 0, whiffs: 0, min_dist: f32::MAX };
        let mut swing_cd = 0.0f32;
        for _ in 0..(f.secs / dt) as usize {
            let dist = (x * x + z * z).sqrt();
            let to_dragon = z.atan2(x);
            if f.track > 0.0 {
                let e = wrap(to_dragon - pyaw);
                pyaw += e.clamp(-f.track * dt, f.track * dt);
            }
            let facing = wrap(to_dragon - pyaw).cos();
            swing_cd = (swing_cd - dt).max(0.0);
            let swings = swing_cd <= 0.0 && dist < f.swing_at && facing > 0.7;
            let whiffed = swings && dist > reach + 2.0;
            if swings {
                swing_cd = 0.8;
            }
            if whiffed {
                t.whiffs += 1;
            }
            let s = DuelSenses {
                dist,
                self_yaw: yaw,
                to_player_yaw: (-z).atan2(-x),
                player_facing: facing,
                can_see: true,
                item: f.item,
                hp01: 1.0,
                player_vitality01: 1.0,
                player_whiffed: whiffed,
                body_radius: 2.0,
            };
            let o = step(st, p, &s, DuelTuning::default(), dt);
            let e = wrap(o.heading - yaw);
            yaw = wrap(yaw + e.clamp(-3.0 * dt, 3.0 * dt));
            let v = 7.0 * o.speed01 * dt;
            x += yaw.cos() * v;
            z += yaw.sin() * v;
            // Bodies don't overlap.
            let d = (x * x + z * z).sqrt();
            if d < 2.0 {
                x *= 2.0 / d.max(1e-3);
                z *= 2.0 / d.max(1e-3);
            }
            t.min_dist = t.min_dist.min(d.max(2.0));
            t.bites += o.bite as usize;
            t.breaths += o.breathe as usize;
            t.outs.push(o);
        }
        t
    }

    fn share(t: &Tally, tac: Tactic) -> f32 {
        t.outs.iter().filter(|o| o.tactic == tac).count() as f32 / t.outs.len() as f32
    }

    /// The intelligence claim: the *same* dragon fights a countering spear
    /// from range and closes on a resisted dagger.
    #[test]
    fn reads_the_item_counter_spear_vs_resisted_dagger() {
        let mut p = DuelProfile::from_genome(&genome(4)); // Fire
        p.cunning = 0.8;
        p.ferocity = 0.6;
        let spear = ItemThreat { element: Some(3), damage: 14.0, reach: 3.5 }; // Dust beats Fire
        let dagger = ItemThreat { element: Some(5), damage: 6.0, reach: 1.0 }; // Fire resists Storm
        let base = Fight { track: 1.5, swing_at: 0.0, item: None, start: 14.0, secs: 20.0 };
        let s = fight(&p, &mut DuelState::new(7), &Fight { item: Some(spear), ..base });
        let d = fight(&p, &mut DuelState::new(7), &Fight { item: Some(dagger), ..base });
        assert!(s.breaths > d.breaths, "breaths: spear {} dagger {}", s.breaths, d.breaths);
        assert!(d.bites > s.bites, "bites: dagger {} spear {}", d.bites, s.bites);
        assert!(s.min_dist > d.min_dist, "closest: spear {:.1} dagger {:.1}", s.min_dist, d.min_dist);
    }

    #[test]
    fn a_countering_element_alone_pushes_it_to_range() {
        // Same reach, same damage: only the element differs.
        let mut p = DuelProfile::from_genome(&genome(4)); // Fire
        p.cunning = 0.8;
        p.ferocity = 0.6;
        let counter = ItemThreat { element: Some(3), damage: 12.0, reach: 1.5 }; // Dust ×2
        let neutral = ItemThreat { element: Some(0), damage: 12.0, reach: 1.5 }; // Wood ×1
        let base = Fight { track: 1.5, swing_at: 0.0, item: None, start: 14.0, secs: 20.0 };
        let c = fight(&p, &mut DuelState::new(11), &Fight { item: Some(counter), ..base });
        let n = fight(&p, &mut DuelState::new(11), &Fight { item: Some(neutral), ..base });
        assert!(
            c.min_dist > n.min_dist + 1.0,
            "closest approach: countered {:.1} vs neutral {:.1}",
            c.min_dist,
            n.min_dist
        );
        assert!(share(&c, Tactic::Lunge) < share(&n, Tactic::Lunge));
    }

    #[test]
    fn circles_a_watchful_player_and_bites_a_frozen_one() {
        let mut p = DuelProfile::from_genome(&genome(2));
        p.cunning = 0.3; // under the feint gate: the choice is circle vs lunge
        p.ferocity = 0.35;
        p.breath.max_range = 0.0; // breath off: this test is about closing in
        let base = Fight { track: 4.0, swing_at: 0.0, item: None, start: 12.0, secs: 12.0 };
        let mut st = DuelState::new(9);
        let watched = fight(&p, &mut st, &base);
        assert!(share(&watched, Tactic::Circle) > 0.5, "circle share {:.2}", share(&watched, Tactic::Circle));
        assert_eq!(watched.bites, 0, "a player who keeps turning to face it is not bitten");
        let mut st = DuelState::new(9);
        let frozen = fight(&p, &mut st, &Fight { track: 0.0, ..base });
        assert!(frozen.bites > 0, "circling should reach the flank of a player who never turns");
    }

    #[test]
    fn feints_draw_whiffs_and_whiffs_open_bites() {
        let mut p = DuelProfile::from_genome(&genome(0));
        p.cunning = 0.9;
        p.ferocity = 0.1; // on its own it would not bite a watching player
        p.breath.max_range = 0.0;
        let dagger = Some(ItemThreat { element: None, damage: 6.0, reach: 1.0 });
        let base = Fight { track: 4.0, swing_at: 0.0, item: dagger, start: 10.0, secs: 15.0 };
        let mut st = DuelState::new(5);
        let patient = fight(&p, &mut st, &base);
        assert!(share(&patient, Tactic::Feint) > 0.0, "a cunning dragon should feint");
        assert_eq!(patient.bites, 0, "a player who never swings gives it no opening");
        let mut st = DuelState::new(5);
        let eager = fight(&p, &mut st, &Fight { swing_at: 6.5, ..base });
        assert!(eager.whiffs > 0 && st.mem.whiffs_baited > 0, "whiffs {}", eager.whiffs);
        assert!(eager.bites > 0, "a baited whiff should be punished");
    }

    /// Score-level checks: each input the tactics claim to read moves the
    /// score it should, with everything else held fixed. The arena tests show
    /// the behaviour; these pin the individual terms, several of which the
    /// arena alone could not tell apart (found by deleting them one at a time).
    fn one(p: &DuelProfile, st: &DuelState, s: &DuelSenses) -> DuelOut {
        let mut st = *st;
        step(&mut st, p, s, DuelTuning::default(), 0.05)
    }

    fn scoring_profile() -> DuelProfile {
        let mut p = DuelProfile::from_genome(&genome(4)); // Fire
        p.cunning = 0.8;
        p.ferocity = 0.6;
        p
    }

    #[test]
    fn a_countering_item_lowers_boldness_and_raises_breath() {
        let p = scoring_profile();
        let st = DuelState::new(1);
        let mut s = senses(p.breath.max_range * 0.6);
        s.item = Some(ItemThreat { element: Some(0), damage: 12.0, reach: 1.5 }); // ×1
        let neutral = one(&p, &st, &s);
        s.item = Some(ItemThreat { element: Some(3), damage: 12.0, reach: 1.5 }); // ×2
        let counter = one(&p, &st, &s);
        assert_eq!(counter.why.counter, 2.0);
        assert!(counter.why.boldness < neutral.why.boldness - 0.1);
        assert!(counter.score(Tactic::Breath) > neutral.score(Tactic::Breath) + 0.1);
    }

    #[test]
    fn a_longer_reach_raises_breath() {
        let p = scoring_profile();
        let st = DuelState::new(1);
        let mut s = senses(p.breath.max_range * 0.6);
        s.item = Some(ItemThreat { element: None, damage: 12.0, reach: 1.0 });
        let short = one(&p, &st, &s);
        s.item = Some(ItemThreat { element: None, damage: 12.0, reach: 4.0 });
        let long = one(&p, &st, &s);
        assert!(long.score(Tactic::Breath) > short.score(Tactic::Breath) + 0.2);
    }

    #[test]
    fn pain_from_the_element_in_hand_counts_more_than_other_pain() {
        let p = scoring_profile();
        let mut s = senses(10.0);
        s.item = Some(ItemThreat { element: Some(1), damage: 12.0, reach: 1.5 });
        let mut hurt_by_this = DuelState::new(1);
        hurt_by_this.mem.learn_hit(Some(1), 40, p.max_hp, 1.5);
        let mut hurt_by_other = DuelState::new(1);
        hurt_by_other.mem.learn_hit(Some(2), 40, p.max_hp, 1.5);
        let a = one(&p, &hurt_by_this, &s);
        let b = one(&p, &hurt_by_other, &s);
        assert!(a.why.boldness < b.why.boldness - 0.05, "{} vs {}", a.why.boldness, b.why.boldness);
    }

    #[test]
    fn any_pain_sobers_it_and_a_weak_player_emboldens_it() {
        let p = scoring_profile();
        let s = senses(10.0);
        let fresh = one(&p, &DuelState::new(1), &s);
        let mut hurt = DuelState::new(1);
        hurt.mem.learn_hit(Some(2), 60, p.max_hp, 1.5); // not the element in hand
        assert!(one(&p, &hurt, &s).why.boldness < fresh.why.boldness - 0.05);
        let mut weak = s;
        weak.player_vitality01 = 0.2;
        assert!(one(&p, &DuelState::new(1), &weak).why.boldness > fresh.why.boldness + 0.2);
    }

    #[test]
    fn describe_names_the_runner_up() {
        let p = scoring_profile();
        let mut st = DuelState::new(1);
        let o = step(&mut st, &p, &senses(10.0), DuelTuning::default(), 0.05);
        let line = describe(&o);
        assert!(line.starts_with(o.tactic.name()), "{line}");
        assert!(line.contains("| next: "), "{line}");
        assert!(!line.contains(&format!("next: {} ", o.tactic.name())), "{line}");
    }

    #[test]
    fn remembers_long_reach_after_switching_items() {
        let mut p = DuelProfile::from_genome(&genome(1));
        p.cunning = 1.0;
        let mut st = DuelState::new(2);
        st.mem.learn_hit(None, 5, p.max_hp, 3.8); // hit from 3.8 m
        let mut s = senses(10.0);
        s.item = Some(ItemThreat { element: None, damage: 5.0, reach: 1.0 });
        let o = step(&mut st, &p, &s, DuelTuning::default(), 0.05);
        assert!(o.why.respect_reach >= 3.7, "respects {}", o.why.respect_reach);
        // A witless dragon only sees what is in your hand.
        p.cunning = 0.0;
        let mut st2 = DuelState::new(2);
        st2.mem.learn_hit(None, 5, p.max_hp, 3.8);
        let o2 = step(&mut st2, &p, &s, DuelTuning::default(), 0.05);
        assert!((o2.why.respect_reach - 1.0).abs() < 1e-4);
    }

    #[test]
    fn wounded_dragon_retreats_and_asks_for_the_air() {
        let p = DuelProfile::from_genome(&genome(3));
        let mut st = DuelState::new(4);
        let mut s = senses(6.0);
        s.hp01 = 0.08;
        let outs = run(&mut st, &p, &s, DuelTuning::default(), 1.0);
        let last = outs.last().unwrap();
        assert_eq!(last.tactic, Tactic::Retreat);
        assert_eq!(last.want_air, Some(true));
        assert!(wrap(last.heading - (s.to_player_yaw + PI)).abs() < 1e-3);
    }

    #[test]
    fn breath_spends_fuel_and_waits_for_recharge() {
        let mut p = DuelProfile::from_genome(&genome(4));
        p.ferocity = 0.0;
        let mut st = DuelState::new(6);
        let mut s = senses(p.breath.max_range * 0.7);
        s.item = Some(ItemThreat { element: Some(3), damage: 14.0, reach: 3.5 });
        let outs = run(&mut st, &p, &s, DuelTuning::default(), 6.0);
        let shots = outs.iter().filter(|o| o.breathe).count();
        // 1.0 fuel: two shots up front, then 0.08/s buys at most one more in 6 s.
        assert!((1..=3).contains(&shots), "shots {shots}");
    }

    #[test]
    fn no_flip_flop_on_a_steady_scene() {
        let p = DuelProfile::from_genome(&genome(5));
        let mut st = DuelState::new(8);
        st.fuel = 0.0;
        let outs = run(&mut st, &p, &senses(9.0), DuelTuning::default(), 3.0);
        let switches = outs.iter().filter(|o| o.changed).count();
        assert!(switches <= 4, "switched {switches} times in 3 s");
    }

    #[test]
    fn tuning_parses_and_reports_what_it_ignored() {
        let (t, bad) = DuelTuning::parse("cunning=1.5, aggr:0.5,dread=0,speed=9,cunning=x");
        assert_eq!(t.cunning, 1.5);
        assert_eq!(t.aggression, 0.5);
        assert_eq!(t.dread, 0.0);
        assert_eq!(bad, vec!["speed=9".to_string(), "cunning=x".to_string()]);
    }

    #[test]
    fn non_finite_senses_do_not_panic_or_act() {
        let p = DuelProfile::from_genome(&genome(0));
        let mut st = DuelState::new(0);
        let mut s = senses(f32::NAN);
        s.to_player_yaw = f32::INFINITY;
        s.hp01 = f32::NAN;
        let o = step(&mut st, &p, &s, DuelTuning::default(), f32::NAN);
        assert!(o.heading.is_finite());
        assert!(!o.bite);
    }
}
