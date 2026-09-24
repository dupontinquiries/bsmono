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
    /// Can't see you, but smells or hears you: going after the lead.
    Track,
    /// Bending: gathering one of its `dragon_abilities` (see `dragon_cast`).
    Cast,
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
            Tactic::Track => "track",
            Tactic::Cast => "cast",
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
    /// A non-visual lead on the player (smell or hearing), from
    /// `dragon_nose::track` / `dragon_ears::hear` in the shell. Used only
    /// while the dragon cannot see you and is not already fighting.
    pub lead: Option<Lead>,
    /// What its bending offers right now (from `dragon_cast::Caster` in the
    /// shell): `None` if nothing is ready and nothing is being gathered.
    pub cast: Option<CastOption>,
}

/// A bending option, summarised for the tactical choice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CastOption {
    /// An offensive ability is ready and in range.
    pub offence_ready: bool,
    /// A defensive ability (Rampart, Mist Veil) is ready.
    pub defence_ready: bool,
    /// 0..1 how strong the ready offensive ability is.
    pub value: f32,
    /// It is gathering a cast right now (the Cast tactic holds until release).
    pub busy: bool,
}

/// What kind of non-visual lead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LeadKind {
    /// Smell, with the nose's mode (`dragon_nose::TrackMode::name`).
    Scent(&'static str),
    /// Hearing; `metal` = it heard armour.
    Sound { metal: bool },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lead {
    /// World yaw to go.
    pub heading: f32,
    /// Its guess at how far you are (m), if the sense gives one.
    pub est_dist: Option<f32>,
    pub speed01: f32,
    /// 0..1.
    pub confidence: f32,
    pub kind: LeadKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DuelState {
    pub tactic: Tactic,
    /// Seconds in the current tactic.
    pub held: f32,
    pub engaged: bool,
    /// Could see the player last tick (for the sighting edge).
    pub saw_player: bool,
    /// Seconds until it may roar again.
    pub roar_cd: f32,
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
    /// Where it last knew you to be (distance, world yaw), refreshed by sight
    /// or by a lead. While it cannot see you mid-fight, it fights THIS, not
    /// your true position.
    pub belief: Option<(f32, f32)>,
    /// Seconds without sight of you.
    pub unseen_for: f32,
    /// The current Breath tactic has fired. Breath stays committed until it
    /// fires or [`BREATH_AIM_TIMEOUT`] passes: a slow-turning (flying) dragon
    /// needs longer than the draw to bring its snout round, and the first
    /// in-world soak showed one abandoning every breath before it could aim.
    pub breath_fired: bool,
    pub mem: DuelMemory,
}

impl DuelState {
    pub fn new(seed: u64) -> DuelState {
        DuelState {
            tactic: Tactic::Unaware,
            held: 0.0,
            engaged: false,
            saw_player: false,
            roar_cd: 0.0,
            fuel: 1.0,
            lunge_cd: 0.0,
            opening: 0.0,
            orbit: if seed & 1 == 0 { 1.0 } else { -1.0 },
            feint_in: true,
            belief: None,
            unseen_for: 0.0,
            breath_fired: false,
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
    /// Roar this tick: a dreadful dragon has just caught sight of you (any
    /// distance it can see, well beyond engage range), at most once per
    /// [`ROAR_COOLDOWN`].
    pub roar: bool,
    /// Nerve pressure on the player this tick, 0..1.
    pub dread: f32,
    pub why: Why,
    /// Every tactic's score this tick, so a log or HUD can show the runner-up
    /// and how close it was. All `-1.0` while unaware.
    pub scores: [(Tactic, f32); 7],
    /// `Some(defensive)` on the tick it decides to cast: the shell picks the
    /// ability (`dragon_cast::Caster::choose`) and starts the wind-up.
    pub begin_cast: Option<bool>,
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
/// Least time between two roars of one dragon (s).
pub const ROAR_COOLDOWN: f32 = 45.0;
/// Radius of a dragon's nerve pressure (m).
pub const DREAD_RADIUS: f32 = 30.0;
const MIN_HOLD: f32 = 0.6;
const SWITCH_MARGIN: f32 = 0.15;
const LUNGE_TIME: f32 = 0.55;
const LUNGE_COOLDOWN: f32 = 2.2;
/// Longest a Breath tactic waits to line up its shot before giving up.
pub const BREATH_AIM_TIMEOUT: f32 = 3.0;

#[inline]
fn wrap(a: f32) -> f32 {
    if !a.is_finite() {
        return 0.0;
    }
    (a + PI).rem_euclid(TAU) - PI
}

/// One tick. See the module docs for the shape of the decision.
/// How long an engaged dragon keeps fighting a belief with no sight or lead
/// before it decides it has lost you (s).
pub fn lose_time(cunning: f32) -> f32 {
    4.0 + 12.0 * cunning.clamp(0.0, 1.0)
}

/// The senses as the dragon believes them. With sight: the truth, and the
/// belief is refreshed. Without: a lead's heading and distance guess, or else
/// the last belief -- never the true position (the shell computes that for
/// every dragon; before this, one that had lost sight of you mid-fight still
/// steered by where you really were).
fn believed(st: &mut DuelState, s: &DuelSenses, dt: f32) -> DuelSenses {
    if s.can_see {
        st.unseen_for = 0.0;
        st.belief = Some((s.dist, s.to_player_yaw));
        return *s;
    }
    st.unseen_for += dt;
    if let Some(l) = s.lead.filter(|l| l.heading.is_finite()) {
        let d = l.est_dist.filter(|d| d.is_finite()).or(st.belief.map(|b| b.0)).unwrap_or(ENGAGE_BASE);
        st.belief = Some((d, l.heading));
    }
    let mut b = *s;
    match st.belief {
        Some((d, yaw)) => {
            b.dist = d;
            b.to_player_yaw = yaw;
        }
        None => {
            b.dist = f32::MAX;
            b.to_player_yaw = s.self_yaw;
        }
    }
    b.player_facing = 0.0; // it cannot see which way you face
    b
}

pub fn step(st: &mut DuelState, p: &DuelProfile, s: &DuelSenses, tune: DuelTuning, dt: f32) -> DuelOut {
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.5) } else { 0.0 };
    let s = &believed(st, s, dt);
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
    let engage_at = ENGAGE_BASE + 20.0 * ferocity;
    // A dragon that was already tracking you (by smell or ear) engages the
    // moment it sees you, at any distance: it came for you. Seen from the
    // first soak with armour, where a dragon tracked the clank 120 m, saw the
    // player at 64 m, roared -- and then stood watching from 50 m.
    let hunting_you = st.tactic == Tactic::Track;
    // Being hit engages too, toward where the blow came from (take_hit sets
    // the belief). Once it has lost you the belief is gone, so an old hit
    // does not drag it back into a fight with no one.
    let hit_from = st.mem.hits_taken > 0 && st.belief.is_some();
    if !st.engaged && ((s.can_see && (dist < engage_at || hunting_you)) || hit_from) {
        st.engaged = true;
    }
    if st.engaged && dist > DISENGAGE && st.mem.total_pain() < 0.05 {
        st.engaged = false;
    }
    // Out of sight, no lead, past its patience: it has lost you.
    if st.engaged && !s.can_see && s.lead.is_none() && st.unseen_for > lose_time(cunning) {
        st.engaged = false;
        st.belief = None;
    }
    // Roar on SIGHTING, not on engaging: the moment you come into view, from
    // as far as it can see you. Losing you and spotting you again roars again,
    // but not more often than ROAR_COOLDOWN.
    st.roar_cd = (st.roar_cd - dt).max(0.0);
    let spotted = s.can_see && !st.saw_player;
    st.saw_player = s.can_see;
    let roar = spotted && st.roar_cd <= 0.0 && dread > 0.3;
    if roar {
        st.roar_cd = ROAR_COOLDOWN;
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
        // Not fighting. If it cannot see you but has a lead, it follows it.
        let lead = s.lead.filter(|l| !s.can_see && l.confidence > 0.0 && l.heading.is_finite());
        let next = if lead.is_some() { Tactic::Track } else { Tactic::Unaware };
        let changed = st.tactic != next;
        if changed {
            st.tactic = next;
            st.held = 0.0;
        }
        let (heading, speed01, reason) = match lead {
            Some(l) => (
                wrap(l.heading),
                l.speed01.clamp(0.0, 1.0),
                match l.kind {
                    LeadKind::Scent(mode) => mode,
                    LeadKind::Sound { metal: true } => "heard armour clanking",
                    LeadKind::Sound { metal: false } => "heard footsteps",
                },
            ),
            None => (s.self_yaw, 0.0, "not engaged"),
        };
        return DuelOut {
            tactic: next,
            changed,
            heading,
            speed01,
            want_air: None,
            breathe: false,
            bite: false,
            roar,
            dread: 0.0,
            why: Why { boldness, counter, respect_reach, safe_gap, reason },
            scores: [
                (Tactic::Stalk, -1.0),
                (Tactic::Circle, -1.0),
                (Tactic::Feint, -1.0),
                (Tactic::Lunge, -1.0),
                (Tactic::Breath, -1.0),
                (Tactic::Retreat, -1.0),
                (Tactic::Cast, -1.0),
            ],
            begin_cast: None,
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

    let mut score = [(Tactic::Stalk, f32::MIN); 7];
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
    // Bending. Offence: a ready ability in range is a strong option, stronger
    // when the item in your hand makes closing in costly, and when you
    // out-reach it. Defence: a wall or veil when it is hurt and your item
    // counters it -- put the land between you. A cast in progress holds
    // through `committed` below.
    let c = s.cast;
    let defend = c.map_or(false, |c| c.defence_ready) && s.hp01 < 0.55 && (counter > 1.0 || pain_now > 0.08);
    score[6] = (
        Tactic::Cast,
        if defend {
            1.1 + cunning * 0.3
        } else if c.map_or(false, |c| c.offence_ready) {
            0.5 + 0.35 * c.map_or(0.0, |c| c.value)
                + (counter - 1.0).max(0.0) * 0.3 * cunning
                + (respect_reach - 1.5).max(0.0) * 0.08
        } else {
            -1.0
        },
    );

    // Actions finish before anything else is considered.
    let committed = match st.tactic {
        Tactic::Lunge => st.held < LUNGE_TIME,
        Tactic::Breath => st.held < 0.8 || (!st.breath_fired && st.held < BREATH_AIM_TIMEOUT),
        Tactic::Cast => s.cast.map_or(false, |c| c.busy),
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
    let mut begin_cast = None;
    let mut want_air = None;
    let (heading, speed01, reason) = match st.tactic {
        // Close fast, then creep: a fixed 0.45 lost ground to a walking player
        // for two minutes in the second in-world soak, and a speed that eased
        // off with distance settled into trailing just outside lunge range.
        Tactic::Stalk => {
            let v = if dist > lunge_range + 1.0 { 0.9 } else { 0.45 };
            (toward, v, "your back is turned")
        }
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
            if changed {
                st.breath_fired = false;
            }
            if st.held < 0.4 {
                (toward, 0.1, "drawing breath")
            } else {
                if aim_off < p.breath.cone_half_angle
                    && breath_attack::try_breathe(&mut st.fuel, BREATH_COST)
                {
                    breathe = true;
                    st.breath_fired = true;
                }
                (toward, 0.0, if counter > 1.0 { "your item counters it: fighting from range" } else if respect_reach > 2.5 { "your reach is long: fighting from range" } else { "breathing" })
            }
        }
        Tactic::Retreat => {
            want_air = Some(true);
            (toward + PI, 1.0, "wounded, breaking off")
        }
        Tactic::Cast => {
            if changed && !s.cast.map_or(false, |c| c.busy) {
                begin_cast = Some(defend);
            }
            (toward, 0.1, if defend { "raising the land between you" } else { "gathering its bending" })
        }
        Tactic::Unaware | Tactic::Track => (s.self_yaw, 0.0, "not engaged"),
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
        begin_cast,
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

// ─────────────────────────────────────────────────────────────────────────────
// World adapter: the geometry between a game loop and the stepper
// ─────────────────────────────────────────────────────────────────────────────
//
// Everything here used to be inline in the game binary, where no test could
// reach it: which way the player faces relative to the dragon, the breath's
// angle off the dragon's snout, whether a bite reaches, whether a swing's ray
// touches a big body. A shell hands in plain positions and gets back plain
// numbers; `arena` tests below drive a whole fight through these functions.

/// A dragon's body as the adapter needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragonBody {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub half_extents: [f32; 3],
}

impl DragonBody {
    /// Horizontal body radius for reach arithmetic and hit tests.
    pub fn radius(&self) -> f32 {
        self.half_extents[0].max(self.half_extents[2]).max(1.0)
    }
}

/// The player as the dragon perceives them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerView {
    pub eye: [f32; 3],
    /// View direction (need not be normalised; only x/z are used for facing).
    pub forward: [f32; 3],
    pub hidden: bool,
    pub vitality01: f32,
}

/// Beyond this a dragon cannot see you however bright the day.
pub const SIGHT_RANGE: f32 = 64.0;

/// Build the senses for one tick.
pub fn senses_for(
    body: &DragonBody,
    hp: i32,
    max_hp: i32,
    player: &PlayerView,
    item: ItemThreat,
    whiffed: bool,
) -> DuelSenses {
    let dx = player.eye[0] - body.pos[0];
    let dy = player.eye[1] - body.pos[1];
    let dz = player.eye[2] - body.pos[2];
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
    let flen = (player.forward[0].powi(2) + player.forward[2].powi(2)).sqrt().max(1e-4);
    let hlen = (dx * dx + dz * dz).sqrt().max(1e-4);
    // Player -> dragon is (-dx, -dz).
    let player_facing = (player.forward[0] * -dx + player.forward[2] * -dz) / (flen * hlen);
    DuelSenses {
        dist,
        self_yaw: body.yaw,
        to_player_yaw: dz.atan2(dx),
        player_facing,
        can_see: !player.hidden && dist < SIGHT_RANGE,
        item: Some(item),
        hp01: hp.max(0) as f32 / max_hp.max(1) as f32,
        player_vitality01: player.vitality01,
        player_whiffed: whiffed,
        body_radius: body.radius(),
        lead: None,
        cast: None,
    }
}

/// What one tick's output does to the player, in the profile's damage units
/// (the shell scales these onto its own health model).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Strike {
    /// Breath landed: damage after cone and range falloff.
    Breath { damage: f32, off_axis: f32, dist: f32 },
    BreathMiss { off_axis: f32, dist: f32 },
    Bite { damage: f32 },
    BiteMiss { dist: f32 },
}

/// Resolve the breath and bite in `o` against where the player actually is.
/// At most two strikes (a breath and a bite cannot share a tick in practice,
/// but nothing here assumes it).
pub fn strikes(p: &DuelProfile, o: &DuelOut, body: &DragonBody, player_eye: [f32; 3]) -> Vec<Strike> {
    let dx = player_eye[0] - body.pos[0];
    let dy = player_eye[1] - body.pos[1];
    let dz = player_eye[2] - body.pos[2];
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
    let mut out = Vec::new();
    if o.breathe {
        let off_axis = wrap(dz.atan2(dx) - body.yaw).abs();
        let damage = breath_attack::damage_at(&p.breath, off_axis, dist);
        out.push(if damage > 0.5 {
            Strike::Breath { damage, off_axis, dist }
        } else {
            Strike::BreathMiss { off_axis, dist }
        });
    }
    if o.bite {
        out.push(if dist <= body.radius() + LUNGE_REACH {
            Strike::Bite { damage: p.bite_damage }
        } else {
            Strike::BiteMiss { dist }
        });
    }
    out
}

/// Does a swing from `eye` along `dir` with `reach` touch this body? Returns
/// the distance along the ray. The body is treated as a vertical capsule of
/// the body radius; the old shell test (centre within weapon reach, 1.2 m
/// lateral) could not touch most of a large dragon, whose centre is metres
/// inside it.
pub fn swing_touches(body: &DragonBody, eye: [f32; 3], dir: [f32; 3], reach: f32) -> Option<f32> {
    let dl = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if !(dl > 1e-6) {
        return None;
    }
    let d = [dir[0] / dl, dir[1] / dl, dir[2] / dl];
    let to = [body.pos[0] - eye[0], body.pos[1] - eye[1], body.pos[2] - eye[2]];
    let r = body.radius() + 0.3;
    let along = to[0] * d[0] + to[1] * d[1] + to[2] * d[2];
    if !(0.3..=reach + r).contains(&along) {
        return None;
    }
    let c = [eye[0] + d[0] * along, eye[1] + d[1] * along, eye[2] + d[2] * along];
    let lat_h = ((c[0] - body.pos[0]).powi(2) + (c[2] - body.pos[2]).powi(2)).sqrt();
    let lat_v = (c[1] - body.pos[1]).abs();
    (lat_h < r && lat_v < body.half_extents[1].max(0.5) + 0.3).then_some(along)
}

/// A landed blow, end to end: zone, damage, and what the dragon learns.
pub fn take_hit(p: &DuelProfile, st: &mut DuelState, body: &DragonBody, eye: [f32; 3], item: &ItemThreat) -> (HitZone, ItemHit) {
    let zone = hit_zone(body.pos, body.yaw, body.half_extents[1], body.radius(), eye);
    let h = resolve_item_hit(p, item, zone);
    let from = ((eye[0] - body.pos[0]).powi(2) + (eye[1] - body.pos[1]).powi(2) + (eye[2] - body.pos[2]).powi(2)).sqrt();
    // What the dragon remembers is how far from its BODY you hit it, not from
    // its centre: a spear jab and a dagger stab at a big dragon differ by the
    // weapon, not by the dragon's size.
    st.mem.learn_hit(item.element, h.damage, p.max_hp, (from - body.radius()).max(0.0));
    // A blow tells it where you are, even if it never saw you.
    st.belief = Some((from, (eye[2] - body.pos[2]).atan2(eye[0] - body.pos[0])));
    st.unseen_for = 0.0;
    (zone, h)
}

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
            lead: None,
            cast: None,
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
    fn being_hit_engages_even_unseen_and_it_knows_where_from() {
        let p = DuelProfile::from_genome(&genome(1));
        let mut st = DuelState::new(1);
        let body = DragonBody { pos: [0.0, 70.0, 0.0], yaw: 0.0, half_extents: [2.0, 1.5, 2.0] };
        let item = ItemThreat { element: Some(2), damage: 12.0, reach: 3.0 };
        take_hit(&p, &mut st, &body, [0.0, 70.5, 4.0], &item); // from +z, behind its flank
        let mut s = senses(40.0);
        s.can_see = false;
        let o = step(&mut st, &p, &s, DuelTuning::default(), 0.05);
        assert!(st.engaged);
        assert_ne!(o.tactic, Tactic::Unaware);
        let (_, yaw) = st.belief.unwrap();
        assert!((yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-3, "turns toward the blow: {yaw}");
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
        /// Dragon turn rate (rad/s): 3 on foot; flying dragons in the game turn at 1.2.
        turn: f32,
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
                lead: None,
                cast: None,
            };
            let o = step(st, p, &s, DuelTuning::default(), dt);
            let e = wrap(o.heading - yaw);
            yaw = wrap(yaw + e.clamp(-f.turn * dt, f.turn * dt));
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
        let base = Fight { track: 1.5, swing_at: 0.0, item: None, start: 14.0, secs: 20.0 , turn: 3.0 };
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
        let base = Fight { track: 1.5, swing_at: 0.0, item: None, start: 14.0, secs: 20.0 , turn: 3.0 };
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
        let base = Fight { track: 4.0, swing_at: 0.0, item: None, start: 12.0, secs: 12.0 , turn: 3.0 };
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
        let base = Fight { track: 4.0, swing_at: 0.0, item: dagger, start: 10.0, secs: 15.0 , turn: 3.0 };
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

    // ── world adapter ───────────────────────────────────────────────────────

    fn big_body() -> DragonBody {
        DragonBody { pos: [0.0, 70.0, 0.0], yaw: 0.0, half_extents: [5.0, 2.5, 3.0] }
    }

    fn player_at(eye: [f32; 3], forward: [f32; 3]) -> PlayerView {
        PlayerView { eye, forward, hidden: false, vitality01: 1.0 }
    }

    #[test]
    fn senses_for_reads_facing_sight_and_bearing() {
        let b = big_body();
        let item = ItemThreat::BARE_HANDS;
        // Player 20 m out on +x, looking back at the dragon (-x).
        let s = senses_for(&b, 100, 200, &player_at([20.0, 71.0, 0.0], [-1.0, 0.0, 0.0]), item, false);
        assert!(s.player_facing > 0.99, "{}", s.player_facing);
        assert!(s.to_player_yaw.abs() < 1e-3);
        assert!(s.can_see);
        assert!((s.hp01 - 0.5).abs() < 1e-6);
        assert_eq!(s.body_radius, 5.0);
        // Back turned.
        let s = senses_for(&b, 100, 200, &player_at([20.0, 71.0, 0.0], [1.0, 0.0, 0.0]), item, false);
        assert!(s.player_facing < -0.99);
        // Hidden, or out of sight range.
        let mut hid = player_at([20.0, 71.0, 0.0], [-1.0, 0.0, 0.0]);
        hid.hidden = true;
        assert!(!senses_for(&b, 1, 1, &hid, item, false).can_see);
        assert!(!senses_for(&b, 1, 1, &player_at([SIGHT_RANGE + 1.0, 70.0, 0.0], [-1.0, 0.0, 0.0]), item, false).can_see);
    }

    #[test]
    fn strikes_use_the_real_angle_and_reach() {
        let p = DuelProfile::from_genome(&genome(4));
        let b = big_body();
        let breath = DuelOut { breathe: true, ..idle_out() };
        let ahead = [p.breath.max_range * 0.5, 70.0, 0.0];
        let behind = [-p.breath.max_range * 0.5, 70.0, 0.0];
        assert!(matches!(strikes(&p, &breath, &b, ahead)[..], [Strike::Breath { .. }]));
        assert!(matches!(strikes(&p, &breath, &b, behind)[..], [Strike::BreathMiss { .. }]));
        let bite = DuelOut { bite: true, ..idle_out() };
        // Reach is measured from the body: 5 m radius + LUNGE_REACH.
        assert!(matches!(strikes(&p, &bite, &b, [5.0 + LUNGE_REACH - 0.2, 70.0, 0.0])[..], [Strike::Bite { .. }]));
        assert!(matches!(strikes(&p, &bite, &b, [5.0 + LUNGE_REACH + 0.5, 70.0, 0.0])[..], [Strike::BiteMiss { .. }]));
        assert!(strikes(&p, &idle_out(), &b, ahead).is_empty());
    }

    fn idle_out() -> DuelOut {
        DuelOut {
            tactic: Tactic::Circle,
            changed: false,
            heading: 0.0,
            speed01: 0.0,
            want_air: None,
            breathe: false,
            bite: false,
            roar: false,
            dread: 0.0,
            why: Why { boldness: 0.0, counter: 1.0, respect_reach: 1.0, safe_gap: 1.0, reason: "" },
            scores: [(Tactic::Circle, 0.0); 7],
            begin_cast: None,
        }
    }

    #[test]
    fn a_swing_reaches_a_big_body_from_outside_it() {
        let b = big_body();
        // Eye 6 m from centre = 1 m from the flank-to-snout surface; 1.5 m blade.
        let eye = [6.0, 70.5, 0.0];
        assert!(swing_touches(&b, eye, [-1.0, 0.0, 0.0], 1.5).is_some());
        // The old test (centre within weapon reach) could not: 6 m > 1.5 m.
        // Swinging at the sky, or away, touches nothing.
        assert!(swing_touches(&b, eye, [0.0, 1.0, 0.0], 1.5).is_none());
        assert!(swing_touches(&b, eye, [1.0, 0.0, 0.0], 1.5).is_none());
        // Too far out for the blade.
        assert!(swing_touches(&b, [9.0, 70.5, 0.0], [-1.0, 0.0, 0.0], 1.5).is_none());
        // Well above its back.
        assert!(swing_touches(&b, [6.0, 76.0, 0.0], [-1.0, 0.0, 0.0], 1.5).is_none());
        assert!(swing_touches(&b, eye, [0.0, 0.0, 0.0], 1.5).is_none());
    }

    #[test]
    fn take_hit_remembers_distance_from_the_body_not_the_centre() {
        let p = DuelProfile::from_genome(&genome(1));
        let mut st = DuelState::new(0);
        let b = big_body();
        let spear = ItemThreat { element: None, damage: 10.0, reach: 3.5 };
        let (zone, h) = take_hit(&p, &mut st, &b, [8.0, 70.5, 0.0], &spear);
        assert_eq!(zone, HitZone::Front);
        assert!(h.damage > 0);
        assert!((st.mem.reach_seen - 3.0).abs() < 0.1, "remembered {}", st.mem.reach_seen);
        assert_eq!(st.mem.hits_taken, 1);
    }

    /// A whole fight through the adapter: the shell's geometry, the stepper,
    /// strikes both ways. The player walks at the dragon, keeps facing it, and
    /// swings a spear whenever `swing_touches`; a swing that touches nothing
    /// within a few metres of the body is a whiff.
    struct Bout {
        dragon_hp: i32,
        player_hp: f32,
        player_hits: u32,
        whiffs: u32,
        breath_hits: u32,
        bites: u32,
        retreated_at: Option<f32>,
        hp_at_retreat: i32,
        hits_after_retreat: u32,
        last_dist: f32,
        seen: std::collections::BTreeSet<&'static str>,
    }

    fn bout(p: &DuelProfile, item: ItemThreat, secs: f32) -> (Bout, DuelState) {
        let dt = 0.05;
        let mut st = DuelState::new(3);
        let mut body = DragonBody { pos: [30.0, 70.0, 0.0], yaw: std::f32::consts::PI, half_extents: [3.0, 2.0, 2.0] };
        let mut eye = [0.0f32, 70.5, 0.0];
        let mut b = Bout {
            dragon_hp: p.max_hp,
            player_hp: 100.0,
            player_hits: 0,
            whiffs: 0,
            breath_hits: 0,
            bites: 0,
            retreated_at: None,
            hp_at_retreat: 0,
            hits_after_retreat: 0,
            last_dist: 0.0,
            seen: Default::default(),
        };
        let mut swing_cd = 0.0f32;
        let mut whiffed = false;
        for i in 0..(secs / dt) as usize {
            let t = i as f32 * dt;
            let to = [body.pos[0] - eye[0], 0.0, body.pos[2] - eye[2]];
            let hd = (to[0] * to[0] + to[2] * to[2]).sqrt().max(1e-3);
            let fwd = [to[0] / hd, 0.0, to[2] / hd];
            // Player: close to just inside spear reach of the body, then hold.
            let want = body.radius() + item.reach * 0.8;
            if hd > want {
                eye[0] += fwd[0] * 4.0 * dt;
                eye[2] += fwd[2] * 4.0 * dt;
            }
            let pv = PlayerView { eye, forward: fwd, hidden: false, vitality01: b.player_hp / 100.0 };
            let s = senses_for(&body, b.dragon_hp, p.max_hp, &pv, item, whiffed);
            let o = step(&mut st, p, &s, DuelTuning::default(), dt);
            b.seen.insert(o.tactic.name());
            if o.tactic == Tactic::Retreat && b.retreated_at.is_none() {
                b.retreated_at = Some(t);
                b.hp_at_retreat = b.dragon_hp;
            }
            for k in strikes(p, &o, &body, eye) {
                match k {
                    Strike::Breath { damage, .. } => {
                        b.breath_hits += 1;
                        b.player_hp -= damage * 0.4;
                    }
                    Strike::Bite { damage } => {
                        b.bites += 1;
                        b.player_hp -= damage * 0.4;
                    }
                    _ => {}
                }
            }
            // Dragon moves by its own output.
            let e = wrap(o.heading - body.yaw);
            body.yaw = wrap(body.yaw + e.clamp(-2.5 * dt, 2.5 * dt));
            body.pos[0] += body.yaw.cos() * 7.0 * o.speed01 * dt;
            body.pos[2] += body.yaw.sin() * 7.0 * o.speed01 * dt;
            // Player swings on a cooldown whenever the dragon is anywhere near.
            swing_cd = (swing_cd - dt).max(0.0);
            whiffed = false;
            if swing_cd <= 0.0 && hd < body.radius() + item.reach + 3.0 {
                swing_cd = 0.7;
                let dir = [fwd[0], 0.0, fwd[2]];
                if swing_touches(&body, eye, dir, item.reach).is_some() {
                    let (_, h) = take_hit(p, &mut st, &body, eye, &item);
                    b.dragon_hp -= h.damage;
                    b.player_hits += 1;
                    if b.retreated_at.is_some() {
                        b.hits_after_retreat += 1;
                    }
                } else {
                    whiffed = true;
                    b.whiffs += 1;
                }
            }
            b.last_dist = hd;
            if b.dragon_hp <= 0 || b.player_hp <= 0.0 {
                break;
            }
        }
        (b, st)
    }

    #[test]
    fn a_whole_fight_runs_both_ways_through_the_adapter() {
        let mut p = DuelProfile::from_genome(&genome(4)); // Fire
        p.cunning = 0.6;
        p.ferocity = 0.7;
        let spear = ItemThreat { element: Some(3), damage: 30.0, reach: 3.5 }; // Dust: counters Fire
        let (b, st) = bout(&p, spear, 90.0);
        assert!(b.player_hits > 0, "the player never landed a blow");
        assert!(b.breath_hits + b.bites > 0, "the dragon never landed anything");
        assert!(b.dragon_hp < p.max_hp);
        assert!(st.mem.hits_taken as u32 == b.player_hits);
        // It learned the spear from the body surface, near the spear's reach.
        assert!(st.mem.reach_seen > 1.5 && st.mem.reach_seen < spear.reach + 1.0, "reach_seen {}", st.mem.reach_seen);
        assert!(b.seen.len() >= 3, "tactics used: {:?}", b.seen);
    }

    #[test]
    fn a_losing_dragon_breaks_off() {
        let mut p = DuelProfile::from_genome(&genome(4));
        p.cunning = 0.7;
        p.ferocity = 0.9; // it will come to you, and pay for it
        p.max_hp = 300; // several blows to get there, so there is a choice to make
        let spear = ItemThreat { element: Some(3), damage: 25.0, reach: 3.5 };
        let (b, _) = bout(&p, spear, 120.0);
        let t = b.retreated_at.expect("a dragon taking counter-element spear hits should retreat");
        assert!(t > 1.0);
        assert!(b.dragon_hp > 0, "it should break off before dying (hp {})", b.dragon_hp);
        // It breaks off inside its threshold (0.18 + 0.2·cunning) plus one blow of slack...
        assert!(b.hp_at_retreat as f32 / p.max_hp as f32 <= 0.18 + 0.2 * p.cunning + 0.01, "{}", b.hp_at_retreat);
        // ...and actually gets away from a pursuer on foot.
        assert!(b.hits_after_retreat <= 2, "hit {} times while fleeing", b.hits_after_retreat);
        assert!(b.last_dist > 30.0, "only {:.1} m away at the end", b.last_dist);
    }

    #[test]
    fn a_committed_breath_waits_for_a_slow_turn() {
        // Reproduces the first in-world soak: a flying dragon (1.2 rad/s)
        // entered Breath facing away; before its snout came round the player
        // turned their back, Stalk outscored Breath, and the breath was
        // abandoned unfired -- twice. Breath now stays committed until it fires
        // or BREATH_AIM_TIMEOUT.
        let mut p = DuelProfile::from_genome(&genome(1));
        p.cunning = 1.0; // stalk scores 0.8 against breath's 0.45
        let mut st = DuelState::new(1);
        st.engaged = true;
        st.roar_cd = ROAR_COOLDOWN;
        st.saw_player = true;
        st.tactic = Tactic::Breath;
        let mut s = senses(p.breath.max_range * 0.6);
        s.player_facing = -1.0;
        let mut yaw = PI; // facing directly away
        let (mut shots, mut left_unfired) = (0, false);
        for _ in 0..(4.0 / 0.05) as usize {
            s.self_yaw = yaw;
            let o = step(&mut st, &p, &s, DuelTuning::default(), 0.05);
            if o.changed && shots == 0 {
                left_unfired = true;
            }
            shots += o.breathe as usize;
            yaw = wrap(yaw + wrap(o.heading - yaw).clamp(-1.2 * 0.05, 1.2 * 0.05));
        }
        assert!(!left_unfired, "abandoned the breath before it could aim");
        assert_eq!(shots, 1);
        assert!(BREATH_AIM_TIMEOUT > PI / 1.2, "timeout must cover a half-turn at flight turn rate");
    }

    #[test]
    fn a_stalker_catches_someone_walking_away() {
        // Player walks away at 4 m/s with their back turned; the dragon runs at
        // 8 m/s × speed01 (the game's DUEL_GROUND_SPEED).
        let mut p = DuelProfile::from_genome(&genome(2));
        p.cunning = 0.8;
        p.ferocity = 0.5;
        p.breath.max_range = 0.0;
        let mut st = DuelState::new(1);
        let (mut px, mut dx, mut yaw) = (0.0f32, -12.0f32, 0.0f32);
        let mut bit = false;
        for _ in 0..(30.0 / 0.05) as usize {
            px += 4.0 * 0.05;
            let dist = (px - dx).abs();
            let mut s = senses(dist);
            s.self_yaw = yaw;
            s.to_player_yaw = 0.0;
            s.player_facing = -1.0;
            let o = step(&mut st, &p, &s, DuelTuning::default(), 0.05);
            bit |= o.bite;
            yaw = wrap(yaw + wrap(o.heading - yaw).clamp(-3.0 * 0.05, 3.0 * 0.05));
            dx = (dx + yaw.cos() * 8.0 * o.speed01 * 0.05).min(px - 2.0);
        }
        assert!(bit, "never caught up; ended {:.1} m behind", px - dx);
    }

    #[test]
    fn it_roars_when_it_first_sees_you_far_beyond_engage_range() {
        let mut p = DuelProfile::from_genome(&genome(4));
        p.dread = 0.8;
        p.ferocity = 0.0; // engages at ENGAGE_BASE
        let mut st = DuelState::new(1);
        let o = step(&mut st, &p, &senses(60.0), DuelTuning::default(), 0.05);
        assert!(o.roar, "should roar on sighting at 60 m");
        assert!(!st.engaged, "…without engaging at 60 m");
        assert_eq!(o.tactic, Tactic::Unaware);
    }

    #[test]
    fn losing_and_regaining_sight_roars_again_after_the_cooldown() {
        let mut p = DuelProfile::from_genome(&genome(4));
        p.dread = 0.8;
        p.ferocity = 0.0;
        let mut st = DuelState::new(1);
        let seen = senses(55.0);
        let mut hidden = seen;
        hidden.can_see = false;
        let tick = |s: &DuelSenses, secs: f32, st: &mut DuelState| -> usize {
            (0..(secs / 0.5) as usize).map(|_| step(st, &p, s, DuelTuning::default(), 0.5).roar as usize).sum()
        };
        let mut roars = tick(&seen, 2.0, &mut st); // spotted: 1
        roars += tick(&hidden, 5.0, &mut st);
        roars += tick(&seen, 2.0, &mut st); // spotted again inside the cooldown: no roar
        assert_eq!(roars, 1);
        roars += tick(&hidden, ROAR_COOLDOWN, &mut st);
        roars += tick(&seen, 2.0, &mut st); // spotted after the cooldown: roar
        assert_eq!(roars, 2);
        // Staying in view never re-roars.
        roars += tick(&seen, ROAR_COOLDOWN * 2.0, &mut st);
        assert_eq!(roars, 2);
    }

    #[test]
    fn unseen_but_smelled_it_tracks_the_lead() {
        let p = DuelProfile::from_genome(&genome(1));
        let mut st = DuelState::new(1);
        let mut s = senses(90.0);
        s.can_see = false;
        s.lead = Some(Lead { est_dist: None, heading: 1.0, speed01: 0.7, confidence: 0.5, kind: LeadKind::Scent("following the trail") });
        let o = step(&mut st, &p, &s, DuelTuning::default(), 0.05);
        assert_eq!(o.tactic, Tactic::Track);
        assert!(o.changed);
        assert!((o.heading - 1.0).abs() < 1e-5 && (o.speed01 - 0.7).abs() < 1e-5);
        assert_eq!(o.why.reason, "following the trail");
        assert!(!st.engaged, "a lead is not a fight");
        // Heard armour instead.
        s.lead = Some(Lead { est_dist: None, heading: -2.0, speed01: 0.6, confidence: 0.4, kind: LeadKind::Sound { metal: true } });
        let o = step(&mut st, &p, &s, DuelTuning::default(), 0.05);
        assert_eq!(o.why.reason, "heard armour clanking");
        // Lead gone: back to unaware.
        s.lead = None;
        let o = step(&mut st, &p, &s, DuelTuning::default(), 0.05);
        assert_eq!(o.tactic, Tactic::Unaware);
    }

    #[test]
    fn sight_beats_a_lead() {
        let p = DuelProfile::from_genome(&genome(1));
        let mut st = DuelState::new(1);
        let mut s = senses(90.0); // visible, beyond engage range
        s.lead = Some(Lead { est_dist: None, heading: 1.0, speed01: 0.7, confidence: 0.9, kind: LeadKind::Sound { metal: false } });
        let o = step(&mut st, &p, &s, DuelTuning::default(), 0.05);
        assert_eq!(o.tactic, Tactic::Unaware, "it can see you: the lead adds nothing");
        let mut near = s;
        near.dist = 10.0;
        let o = step(&mut st, &p, &near, DuelTuning::default(), 0.05);
        assert!(st.engaged && o.tactic != Tactic::Track);
    }

    #[test]
    fn a_dragon_that_tracked_you_engages_on_sight_at_any_range() {
        let mut p = DuelProfile::from_genome(&genome(1));
        p.ferocity = 0.0; // engage range = ENGAGE_BASE (28 m)
        let mut st = DuelState::new(1);
        let mut s = senses(55.0);
        s.can_see = false;
        s.lead = Some(Lead { est_dist: None, heading: 0.0, speed01: 0.6, confidence: 0.5, kind: LeadKind::Sound { metal: true } });
        step(&mut st, &p, &s, DuelTuning::default(), 0.05);
        assert_eq!(st.tactic, Tactic::Track);
        s.can_see = true;
        step(&mut st, &p, &s, DuelTuning::default(), 0.05);
        assert!(st.engaged, "tracked you, then saw you at 55 m: engages");
        // A dragon that merely sees you at 55 m does not.
        let mut idle = DuelState::new(1);
        step(&mut idle, &p, &senses(55.0), DuelTuning::default(), 0.05);
        assert!(!idle.engaged);
    }

    #[test]
    fn out_of_sight_mid_fight_it_fights_its_belief_not_your_position() {
        let mut p = DuelProfile::from_genome(&genome(2));
        p.cunning = 0.5;
        let mut st = DuelState::new(1);
        // Seen at 20 m, bearing 0.3: engaged.
        let mut s = senses(20.0);
        s.to_player_yaw = 0.3;
        step(&mut st, &p, &s, DuelTuning::default(), 0.05);
        assert!(st.engaged);
        // You slip behind a hill and actually run the other way.
        let mut hid = s;
        hid.can_see = false;
        hid.to_player_yaw = -2.5;
        hid.dist = 35.0;
        let mut outs = vec![];
        for _ in 0..40 {
            outs.push(step(&mut st, &p, &hid, DuelTuning::default(), 0.05));
        }
        assert!(st.engaged, "2 s unseen: still in the fight");
        // It cannot see which way you face, so it stalks -- straight at where
        // it believes you are (0.3), not where you are (-2.5).
        for o in &outs[20..] {
            assert_eq!(o.tactic, Tactic::Stalk);
            assert!(wrap(o.heading - 0.3).abs() < 0.05, "heading {} is not the belief", o.heading);
        }
        assert_eq!(st.belief, Some((20.0, 0.3)));
    }

    #[test]
    fn with_no_sight_and_no_lead_it_loses_you_later_if_cunning() {
        let lost_after = |cunning: f32| {
            let mut p = DuelProfile::from_genome(&genome(2));
            p.cunning = cunning;
            let mut st = DuelState::new(1);
            step(&mut st, &p, &senses(20.0), DuelTuning::default(), 0.05);
            let mut hid = senses(20.0);
            hid.can_see = false;
            let mut t = 0.0;
            while st.engaged && t < 60.0 {
                step(&mut st, &p, &hid, DuelTuning::default(), 0.1);
                t += 0.1;
            }
            t
        };
        let (dull, sharp) = (lost_after(0.1), lost_after(0.9));
        assert!((dull - lose_time(0.1)).abs() < 0.3, "dull gave up at {dull}");
        assert!(sharp > dull + 8.0, "sharp {sharp} vs dull {dull}");
    }

    #[test]
    fn a_lead_mid_fight_updates_the_belief() {
        let p = DuelProfile::from_genome(&genome(2));
        let mut st = DuelState::new(1);
        step(&mut st, &p, &senses(20.0), DuelTuning::default(), 0.05);
        let mut hid = senses(20.0);
        hid.can_see = false;
        hid.lead = Some(Lead { est_dist: Some(30.0), heading: 1.2, speed01: 0.6, confidence: 0.5, kind: LeadKind::Sound { metal: true } });
        for _ in 0..200 {
            step(&mut st, &p, &hid, DuelTuning::default(), 0.1);
        }
        assert!(st.engaged, "a live lead keeps it in the fight");
        assert_eq!(st.belief, Some((30.0, 1.2)));
    }

    #[test]
    fn a_ready_ability_gets_cast_and_the_cast_holds_until_release() {
        let mut p = DuelProfile::from_genome(&genome(3));
        p.ferocity = 0.3;
        let mut st = DuelState::new(1);
        st.fuel = 0.0;
        let mut s = senses(25.0);
        s.cast = Some(CastOption { offence_ready: true, defence_ready: false, value: 0.8, busy: false });
        let o = step(&mut st, &p, &s, DuelTuning::default(), 0.05);
        assert_eq!(o.tactic, Tactic::Cast);
        assert_eq!(o.begin_cast, Some(false));
        // Gathering: nothing else may interrupt, and it does not re-begin.
        s.cast = Some(CastOption { offence_ready: false, defence_ready: false, value: 0.0, busy: true });
        let mut near = s;
        near.dist = 3.0;
        near.player_facing = -1.0; // a juicy flank for a lunge
        for _ in 0..30 {
            let o = step(&mut st, &p, &near, DuelTuning::default(), 0.05);
            assert_eq!(o.tactic, Tactic::Cast);
            assert_eq!(o.begin_cast, None);
        }
        // Released and on cooldown: something else takes over.
        near.cast = None;
        let o = step(&mut st, &p, &near, DuelTuning::default(), 0.05);
        assert_ne!(o.tactic, Tactic::Cast);
    }

    #[test]
    fn hurt_by_a_countering_item_it_walls_itself_off() {
        let mut p = DuelProfile::from_genome(&genome(4)); // Fire
        p.cunning = 0.7;
        let mut st = DuelState::new(1);
        st.fuel = 0.0;
        let mut s = senses(12.0);
        s.hp01 = 0.45;
        s.item = Some(ItemThreat { element: Some(3), damage: 20.0, reach: 3.0 }); // Dust counters Fire
        s.cast = Some(CastOption { offence_ready: true, defence_ready: true, value: 0.5, busy: false });
        let o = step(&mut st, &p, &s, DuelTuning::default(), 0.05);
        assert_eq!(o.tactic, Tactic::Cast);
        assert_eq!(o.begin_cast, Some(true), "defensive");
        assert_eq!(o.why.reason, "raising the land between you");
        // Healthy and facing a neutral item: offence instead.
        let mut st2 = DuelState::new(1);
        st2.fuel = 0.0;
        let mut calm = s;
        calm.hp01 = 1.0;
        calm.item = None;
        let o = step(&mut st2, &p, &calm, DuelTuning::default(), 0.05);
        assert_eq!(o.begin_cast, Some(false));
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
