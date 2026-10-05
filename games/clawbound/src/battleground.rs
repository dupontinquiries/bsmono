//! battleground.rs — **"Battleground of the Ancestors"**: the clan-war mode, a
//! MOBA re-thought in the books' terms and run as pure, deterministic,
//! renderer-free logic like `sim`/`prowl`/`capture`.
//!
//! Two clans, each championed by legendary ancestors ([`crate::ancestor`]), fight
//! across a **large** map (camps in opposite corners, three trails between) to fell
//! the enemy camp's **Great Tree**. Every MOBA fixture is translated, not copied:
//!
//! * **Trees of Healing instead of towers.** The trail structures don't shoot —
//!   they *protect*: allies near a living Heal Tree are mended and take reduced
//!   harm. And while any of a clan's Heal Trees stand, their roots share strength
//!   with the Great Tree, hardening it. You don't dive a tower; you gnaw a
//!   sanctuary out from under its clan.
//! * **Freshkill instead of a shop.** There is no gold and nothing to buy. Prey
//!   lives at neutral hunting camps and at the central **contention ground**;
//!   killed prey must be *carried home* (slower, droppable on death) to the camp's
//!   freshkill pile. A well-fed clan grows stronger — the pile scales the whole
//!   clan's claws, with diminishing returns so a lead never snowballs unbounded.
//!   Holding the contention ground (more warriors present) makes its rich prey
//!   yours to hunt.
//! * **Patrols instead of minion waves.** Lanes carry cat AI patrols: *fewer,
//!   stronger*, and sensible. A patrol never seeks to fight to the death — once a
//!   cat is wounded past its nerve it breaks off, returns to camp, and is healed by
//!   the **medic cat** before rejoining the trail. The one exception is the books'
//!   exception: defending the Great Tree itself, a warrior stands.
//! * **Herbs as extra objectives.** Herb patches regrow in the leafy margins;
//!   gathering takes a held moment and the sprig must reach your medic cat. Each
//!   delivered herb quickens every kind of healing the clan does — medic, trees,
//!   and how fast the fallen return.
//!
//! **World note (a deliberate deviation from novel canon):** this world has *no
//! humans* — no thunderpaths, no monsters, no twoleg places, no kittypets. The
//! terrain generator builds only wild land (forest, moor, river, fen, crag), and
//! nothing in this module may reference a human-made feature. The clans' whole
//! world is the wild.

use crate::ancestor::{self, Ancestor, Verb};
use crate::junglemap;
use crate::world;

pub const CLAN_A: u8 = 0;
pub const CLAN_B: u8 = 1;

// ── the large map ───────────────────────────────────────────────────────────
const ARENA: f32 = 150.0;                       // half-extent (capture plays in 90)
pub const CAMP_ANCHOR: [(f32, f32); 2] = [(-118.0, -118.0), (118.0, 118.0)];
/// Waypoint each trail bends through: west trail, the straight middle, east trail.
pub const TRAIL_BEND: [(f32, f32); 3] = [(-85.0, 85.0), (0.0, 0.0), (85.0, -85.0)];
pub const LANES: usize = 3;
/// Which trail is which, now that the country makes them differ. The mountain
/// side is the **duo** lane — two warriors hold it, because it is where the
/// Skyrock and the high ground are and holding it wins the map's information
/// war; the river side is the **solo** lane, walked by one; the middle belongs
/// to the patrols, who get the extra waves for it.
///
/// This replaced one-ancestor-per-lane, which was tidy and wrong: it left the
/// map's duo lane with nobody duoing, so the crowd-control coordination in
/// `auto_cast` had no partner to coordinate with and measured as no effect
/// across 168 games.
pub const DUO_LANE: usize = 0;
pub const MID_LANE: usize = 1;
pub const SOLO_LANE: usize = 2;
/// Which of a clan's three ancestors walks the river alone: the least sociable
/// of them ([`ancestor::sociability`]). A clan does not send its pride-fighter
/// off alone and its ambusher into a crowd — the shields, heals and auras are
/// worth nothing on an empty trail, and a veil is worth less with a packmate
/// blundering alongside. Ties break on pool order, so the choice is
/// deterministic and identical for both clans.
pub fn solo_seat(picks: &[Ancestor]) -> usize {
    let mut best = 0usize;
    for s in 1..picks.len() {
        let (a, b) = (ancestor::sociability(picks[s]), ancestor::sociability(picks[best]));
        if a < b { best = s; }
    }
    best
}

/// How a clan decides who walks where.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LanePolicy {
    /// Draft order decides: seat 0 takes the river. Arbitrary, but it is the
    /// baseline every balance number before this was measured against.
    Seat,
    /// The kit decides ([`solo_seat`]). Better doctrine, and better for the
    /// ambushers; whether it is better for the *roster* is what the switch is
    /// here to answer.
    Kit,
}

/// Seat -> trail, given who else the clan fielded and how the clan decides.
///
/// A clan of three is the old shape: one down the river, two up the mountain.
/// A clan of **five** is the conquest shape the map was actually built for —
/// solo on the river, two together on the mountain, one holding the middle the
/// patrols walk, and a fifth with no lane at all, who starts in the jungle and
/// goes where the fight is. That fifth seat is the reason the middle stopped
/// being nobody's: with five there is somebody to hold it.
pub fn lane_for(seat: usize, picks: &[Ancestor], policy: LanePolicy) -> usize {
    let solo = match policy {
        LanePolicy::Seat => 0,
        LanePolicy::Kit => solo_seat(&picks),
    };
    if seat == solo {
        return SOLO_LANE;
    }
    if picks.len() < 4 {
        return DUO_LANE;
    }
    // the seats that are not the river-walker, in order, take: mid, duo, duo,
    // then the roamer — who is given mid as a starting trail and leaves it
    let rest: Vec<usize> = (0..picks.len()).filter(|&s| s != solo).collect();
    match rest.iter().position(|&s| s == seat).unwrap_or(0) {
        0 => MID_LANE,
        1 | 2 => DUO_LANE,
        _ => MID_LANE,
    }
}

/// Does this clan field a roamer — a warrior with no lane of its own, free to
/// join whichever fight is worth joining? Only a full five does.
pub fn roamer_seat(picks: &[Ancestor]) -> Option<usize> {
    if picks.len() < 5 {
        return None;
    }
    let solo = solo_seat(&picks);
    (0..picks.len()).filter(|&s| s != solo).nth(3)
}

// structures
const HEAL_TREE_HP: f32 = 550.0;
/// Freshkill spilled by a felled Heal Tree, when [`Battleground::spoils`] is on.
/// Sized against the economy it enters: a rich kill from the contention ground
/// is worth 3, so a border is worth about four of those — enough to notice in
/// the growth curve, not enough to end a match by itself.
const BORDER_SPOILS: f32 = 12.0;
const GREAT_TREE_HP: f32 = 1400.0;
const AURA_R: f32 = 14.0;
const AURA_HEAL: f32 = 5.0;          // hp/s to allies, before herb scaling
const AURA_GUARD: f32 = 0.35;        // damage reduction under a living Heal Tree
const ROOT_GUARD: f32 = 0.55;        // Great Tree damage reduction while any Heal Tree stands

// patrols — fewer, stronger, and they retreat
const PATROL_INTERVAL: f32 = 26.0;
const PATROL_SIZE: usize = 2;
/// The most warriors a clan may field. Five is the conquest shape: a river
/// solo, a mountain duo, a middle, and a roamer.
pub const MAX_CLAN: usize = 5;
const PATROL_HP: f32 = 130.0;
const PATROL_ATK: f32 = 8.0;
const PATROL_SPEED: f32 = 6.4;
const NERVE: f32 = 0.45;             // retreat below this hp fraction…
/// How far a warrior counts the odds before deciding a fight is not worth it.
const ODDS_R: f32 = 16.0;
/// How many more enemies than kin it takes to break off. Two rather than one:
/// at one the AI flaps, retreating and re-engaging as a packmate crosses the
/// radius, and a patrol that cannot commit to a fight is worse than one that
/// loses it.
const ODDS_MARGIN: i32 = 2;
const REJOIN: f32 = 0.9;             // …rejoin the trail above this
const LAST_STAND_R: f32 = 26.0;      // …unless the Great Tree itself is threatened

// camp & medic
const MEDIC_R: f32 = 10.0;
const MEDIC_HEAL: f32 = 16.0;        // hp/s at the medic's den, before herbs

// freshkill economy
/// Two hunting camps on clan A's side of the anti-diagonal; the other two are
/// their reflections, generated rather than typed, so the prey cannot drift out
/// of symmetry the way it had (the old list was half-turn symmetric while the
/// river and the land were mirror symmetric — fair under neither).
const PREY_CAMPS_BASE: [(f32, f32); 2] = [(-70.0, 20.0), (-20.0, -70.0)];
const PREY_HP: f32 = 26.0;
const PREY_RESPAWN: f32 = 22.0;
const RICH_WORTH: f32 = 3.0;         // the contention ground's prey
const CARRY_SLOW: f32 = 0.75;
const CONTEND_R: f32 = 20.0;
const GROWTH_CAP: f32 = 0.8;         // pile can add at most +80% power
/// The pile at which a clan has half the growth it can ever have. **Sized to the
/// economy that exists, not the one the number was guessed for**: the probe
/// found the mean growth multiplier running at 1.138 against 1.108 — a whole
/// clan's freshkill lead buying under three percent damage — because the
/// half-point was 16 while real piles hover around three. The curve's entire
/// interesting range sat above anything a match ever reached, so the economy
/// was decorative. At 6 a typical lead is worth something a fight can feel.
const GROWTH_HALF: f32 = 6.0;       // pile worth at which half the cap is reached

// herbs
/// One herb patch on clan A's side; the other is its reflection, generated —
/// the typed pair was half-turn symmetric like the old prey list, and the mirror
/// probe caught the consequence directly: both clans' seat-0 warriors decided to
/// gather on the same tick and walked to patches that were not each other's
/// mirror.
const HERB_SPOTS_BASE: [(f32, f32); 1] = [(-45.0, -5.0)];
const HERB_REGROW: f32 = 30.0;
const HERB_GATHER: f32 = 2.0;        // seconds crouched over the patch
const HERB_HEAL_BONUS: f32 = 0.15;   // each delivered herb: +15% to all clan healing…
const HERB_HEAL_MAX: f32 = 2.0;      // …capped at double
const HERB_RESPAWN_CUT: f32 = 0.8;   // and each herb multiplies respawn time by this

const DAY_LEN: f32 = 90.0;           // day and night alternate, this long each
const LEAFBARE_AT: f32 = 300.0;      // late game: auras thin, patrols grow desperate
const LEAFBARE_AURA: f32 = 0.5;      // aura healing multiplier once leaf-bare comes
const LEAFBARE_PATROL: usize = 3;    // patrol size in leaf-bare

const RESPAWN_BASE: f32 = 14.0;
const REACH: f32 = 2.8;              // claw range
/// What a ranged strike is worth while giving ground. See the kiting branch.
const KITE_STRIKE: f32 = 0.5;
/// How many lengths of distance a warrior will trade to strike a wounded enemy
/// instead of a hale one. At 8, a cat five lengths off at half strength outbids
/// one right beside it at full — pack behaviour, and the books' behaviour: the
/// patrol turns on the one that is already bleeding.
const FOCUS_WEIGHT: f32 = 8.0;
const ENGAGE_R: f32 = 16.0;          // how far a warrior will break trail to fight
const MATCH_LEN: f32 = 12.0 * 60.0;

fn hashf(mut h: u32) -> f32 {
    h ^= h >> 16; h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15; h = h.wrapping_mul(0x846ca68b);
    h ^= h >> 16; (h as f32) / (u32::MAX as f32)
}
fn dist(ax: f32, az: f32, bx: f32, bz: f32) -> f32 { ((ax - bx).powi(2) + (az - bz).powi(2)).sqrt() }
fn norm(x: f32, z: f32) -> (f32, f32) { let l = (x * x + z * z).sqrt(); if l > 1e-5 { (x / l, z / l) } else { (0.0, 0.0) } }

/// Nearest dry, flattish ground to an anchor (same idea as capture.rs).
fn land_near(ax: f32, az: f32) -> (f32, f32) {
    let (mut best, mut best_score) = ((ax, az), f32::MAX);
    for ring in 0..16 {
        let r = ring as f32 * 4.0;
        let steps = (6 + ring * 3).max(1);
        for s in 0..steps {
            let a = s as f32 / steps as f32 * std::f32::consts::TAU;
            let (x, z) = (ax + a.cos() * r, az + a.sin() * r);
            let h = world::height_at(x, z);
            if h < world::WATER_Y + 0.3 { continue; }
            let slope = (world::height_at(x + 2.0, z) - h).abs() + (world::height_at(x, z + 2.0) - h).abs();
            let score = slope * 3.0 + r * 0.05;
            if score < best_score { best_score = score; best = (x, z); }
        }
        if best_score < 0.5 && ring >= 2 { break; }
    }
    best
}

// ── entities ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role { Ancestor, Patrol }

/// The two patrols of the books: a **border patrol** walks the trail and holds the
/// line; a **hunting patrol** works the prey camps and carries freshkill home.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Duty { Border, Hunting }

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Carry { Nothing, Freshkill(f32), Herb }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mind {
    Trail,          // advance along the lane toward the enemy camp
    Fight,          // engaged with a nearby enemy
    Retreat,        // wounded past nerve — break off, head home
    AtMedic,        // healing at the medic cat
    Hunt,           // stalking prey at a hunting camp
    CarryHome,      // bringing freshkill / a herb sprig back to camp
    Gather,         // crouched over a herb patch
    LastStand,      // defending the Great Tree — no retreat
    Dead,
}

pub struct Warrior {
    pub team: u8,
    pub role: Role,
    pub ancestor: Option<Ancestor>,
    pub lane: usize,
    pub x: f32, pub z: f32, pub heading: f32,
    pub hp: f32, pub max_hp: f32,
    pub attack: f32, pub speed: f32,
    pub reach: f32,       // basic-attack reach vs warriors and prey (structures stay melee)
    pub mind: Mind,
    pub carry: Carry,
    pub shield: f32,      // absorbs damage first (Mane of Dawn / Stone Patience)
    /// How long the shield lasts. Every kit's shield declares a `dur` and the sim
    /// ignored it, so guard-hair held until something finally broke it — a lion
    /// could raise Mane of Dawn once and carry it for the rest of the match. The
    /// third free lunch of the week, after the kite and the veil, and found the
    /// same way: by asking what a kit gets without paying.
    pub shield_t: f32,
    pub stealth_t: f32,   // unseen/untargetable
    pub fear_t: f32,      // fleeing a Great Roar
    pub slow_t: f32,
    pub root_t: f32,      // held fast (Snowbind / The Long Winter): can claw, cannot move
    pub pulled_t: f32,    // dragged in a moment ago — the window a wombo aims at
    pub cds: [f32; 3],    // ability 1, ability 2, ult
    pub respawn_t: f32,
    pub gather_t: f32,
    /// Warriors felled. **Prey does not count** — see `prey_taken`. Skyward
    /// Hunger reads this, and a caracal that could stack its whole speed bonus
    /// off a hillside of mice was quickening its paws on nothing that fought
    /// back: the fastest kit on the roster, free, by farming.
    pub kills: u32,
    /// Prey brought down. Kept separate so the hunt is still legible, and so a
    /// future kit *can* be paid in prey if that is its identity.
    pub prey_taken: u32,
    /// Which seat this warrior holds **within its own clan** — 0..2 for the
    /// ancestors, then patrol cats by wave. Every per-warrior random roll hashes
    /// on the seat rather than on the index into `warriors`, so a warrior and
    /// its mirror counterpart make the same decisions. Hashing on the index was
    /// worth real games: clan A's warriors are indices 0..2 and clan B's 3..5,
    /// so the two clans rolled different economy decisions from the same state,
    /// and the economy decides matches.
    pub seat: u32,
    pub still_t: f32,     // seconds spent seated (The Seated Mountain gathers here)
    pub duty: Duty,
    pub is_player: bool,
    seg: usize,           // trail waypoint progress
}

impl Warrior {
    pub fn alive(&self) -> bool { self.mind != Mind::Dead }
    fn power_frac(&self) -> f32 { self.hp / self.max_hp }
}

pub struct HealTree { pub team: u8, pub lane: usize, pub x: f32, pub z: f32, pub hp: f32 }
impl HealTree { pub fn alive(&self) -> bool { self.hp > 0.0 } }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreyKind { Mouse, Rabbit, Fish, Rich }

impl PreyKind {
    pub fn worth(self) -> f32 { match self { PreyKind::Mouse => 1.0, PreyKind::Rabbit => 2.0, PreyKind::Fish => 2.0, PreyKind::Rich => RICH_WORTH } }
    pub fn hp(self) -> f32 { match self { PreyKind::Mouse => 18.0, PreyKind::Rabbit => 30.0, PreyKind::Fish => 24.0, PreyKind::Rich => PREY_HP * 1.5 } }
    /// Land prey bolts from a close hunter; fish just hold to deep water.
    pub fn flees(self) -> bool { matches!(self, PreyKind::Mouse | PreyKind::Rabbit) }
}

pub struct Prey {
    pub x: f32, pub z: f32,
    pub home: (f32, f32),     // spawn anchor it never strays far from
    pub kind: PreyKind,
    pub hp: f32, pub respawn_t: f32,
}
impl Prey {
    pub fn alive(&self) -> bool { self.respawn_t <= 0.0 }
    pub fn rich(&self) -> bool { self.kind == PreyKind::Rich }
    pub fn worth(&self) -> f32 { self.kind.worth() }
}

/// Scorched ground (Suneye's Noon Blaze / Eye of the Sun): enemies inside burn,
/// and the light sears every veil away — the built-in answer to the night kits.
pub struct Zone {
    pub x: f32, pub z: f32, pub team: u8, pub r: f32, pub dps: f32, pub t: f32,
    /// false: scorched ground (burns enemies, sears the veil). true: sweet rain
    /// (mends allies inside — warriors and *wood* alike: Heal Trees and even the
    /// Great Tree drink it, the roster's one answer to a half-gnawed sanctuary).
    pub mend: bool,
}

pub struct HerbPatch { pub x: f32, pub z: f32, pub regrow_t: f32 }
impl HerbPatch { pub fn ready(&self) -> bool { self.regrow_t <= 0.0 } }

pub struct ClanState {
    pub pile: f32,        // freshkill worth delivered
    pub herbs: u32,       // herb sprigs delivered to the medic
    pub great_tree_hp: f32,
}

pub struct Battleground {
    pub warriors: Vec<Warrior>,
    pub heal_trees: Vec<HealTree>,
    pub prey: Vec<Prey>,
    pub herb_patches: Vec<HerbPatch>,
    pub zones: Vec<Zone>,
    pub clans: [ClanState; 2],
    pub camps: [(f32, f32); 2],
    pub medics: [(f32, f32); 2],
    pub contention: (f32, f32),
    pub contention_holder: Option<u8>,
    /// The Skyrock: a lookout spire on the duo ridge's crest (on the anti-diagonal,
    /// so it is the clan-swap reflection's own fixed point — equally far from both
    /// camps). The clan holding it watches the whole territory from above: every
    /// warrior's spotting range grows by a quarter. The mountain lane is thin on
    /// freshkill; the Skyrock is why a clan climbs anyway.
    pub skyrock: (f32, f32),
    pub skyrock_holder: Option<u8>,
    pub time: f32,
    pub outcome: Option<u8>,          // winning clan, or 2 = draw at full time
    patrol_t: [f32; 2],
    wave_n: [u32; 2],     // per-team wave counter — duty/lane rotation must be symmetric
    tick: u32,
    /// Whether AI ancestors coordinate their crowd control (a controller saving
    /// its pull for a crowd, an area ult waiting for the gather).
    ///
    /// **Defaults off, on the evidence.** Measured twice, 168 games each way on
    /// identical drafts and lands: before lane pairing existed, clan A 48% off
    /// against 49% on; after it existed and the duo actually travelled as a
    /// pair, 47% against 47%. No aggregate effect either time — and the kit the
    /// coordination is *for* came out worse in both (Riptail 52 -> 49, then
    /// 56 -> 50). Holding an ability is a cost paid now for a payoff that may
    /// not arrive, and in fights this short it does not pay.
    ///
    /// Kept, switchable, and tested, because the behaviour is not wrong — it is
    /// waiting on an AI that can promise the follow-up rather than hope for it,
    /// and on a human player who can. Turn it on to measure that day's version.
    pub wombo: bool,
    /// Whether a clan collapses onto a broken border: healthy warriors abandon
    /// an intact trail for the lane whose Heal Tree has fallen.
    ///
    /// **Defaults off, on the evidence**, and it is the more interesting of the
    /// two switches because the behaviour is *correct play* — collapsing on a
    /// broken lane is what a conquest team does. Measured over 800 games against
    /// the same drafts and lands: dispersion 2.9 -> 4.2, kits outside two
    /// standard errors 1 -> 2, composition-decided matches 54% -> 52%. Both
    /// metrics flat-to-worse, so the default follows the measurement rather than
    /// the theory. Kept because a better AI — one that leaves a lane held while
    /// the rest collapse, rather than emptying all three — should get a
    /// different answer, and this is the switch that will test it.
    pub collapse_on_breach: bool,
    /// Whether felling a Heal Tree pays the clan that felled it: the defenders'
    /// cached prey spills from the roots ([`BORDER_SPOILS`] worth), straight
    /// onto the attacker's pile.
    ///
    /// This is a **design switch, not a bug fix**, and it is off until someone
    /// decides. The measurement that motivates it: the clan that breaks the
    /// enemy's first border wins 39.8% of its matches over 793 games, 5.7
    /// standard errors below even, because defending here is genuinely stronger
    /// — the defender fights beside its medic den, inside its own auras,
    /// respawns closer, and has the last-stand rule. Most games of this shape
    /// pay the attacker for taking that risk. Whether *this* one should is a
    /// question about what the mode is, so the switch exists and the default
    /// declines to answer it.
    pub spoils: bool,
    /// Which clans have a human at the paws, indexed by side. Set by the first
    /// `set_input` for that side; until then that clan's seat 0 is AI-driven,
    /// which is what lets every balance harness run a "player" slot without one.
    player_driven: [bool; 2],
    input: [Option<(f32, f32, bool, Option<usize>)>; 2],   // move dir, claw, ability index
}

impl Battleground {
    /// Freshkill growth: how much stronger this clan's claws are for what it has
    /// eaten. Saturating, so hoarding freshkill has diminishing returns.
    pub fn growth(&self, team: u8) -> f32 {
        let p = self.clans[team as usize].pile;
        1.0 + GROWTH_CAP * (p / (p + GROWTH_HALF))
    }
    /// Herb healing multiplier for a clan (medic, Heal Tree auras, ancestor heals).
    pub fn herb_heal(&self, team: u8) -> f32 {
        (1.0 + self.clans[team as usize].herbs as f32 * HERB_HEAL_BONUS).min(HERB_HEAL_MAX)
    }
    /// Night falls every other watch; the ambush ancestors were made for it.
    pub fn is_night(&self) -> bool { (self.time / DAY_LEN) as u32 % 2 == 1 }
    /// Leaf-bare: the late game. Auras thin and border patrols grow desperate.
    pub fn leafbare(&self) -> bool { self.time >= LEAFBARE_AT }

    /// A warrior's melee/ability damage per point of attack — where the passives
    /// live. Freshkill growth, Goldenmane's Pride's Warmth beside a packmate,
    /// Night's Hunger / Silent Footfall in the dark or from the veil.
    fn strike_of(&self, i: usize) -> f32 {
        let w = &self.warriors[i];
        let mut m = self.growth(w.team);
        let near_lion = self.warriors.iter().any(|a|
            a.team == w.team && a.alive() && a.ancestor == Some(Ancestor::Lion)
            && dist(a.x, a.z, w.x, w.z) < 8.0);
        if near_lion { m *= 1.15; }
        if matches!(w.ancestor, Some(Ancestor::Tiger) | Some(Ancestor::Panther))
            && (self.is_night() || w.stealth_t > 0.0
                || junglemap::concealment_at(w.x, w.z).is_some()) { m *= 1.3; }
        // Suneye is Night's Hunger mirrored: the day belongs to the sun-priest
        if w.ancestor == Some(Ancestor::SunCat) && !self.is_night() { m *= 1.3; }
        // The Seated Mountain: Stillsmoke's stillness, released into every strike
        if w.ancestor == Some(Ancestor::Suanni) {
            m *= 1.0 + (w.still_t / 4.0).min(1.0) * 0.5;
        }
        // Oldroar's Deep Wounds, Deep Fury: the closer to falling, the harder the blows
        if w.ancestor == Some(Ancestor::CaveLion) {
            m *= 1.0 + 0.6 * (1.0 - w.hp / w.max_hp);
        }
        // leaf-bare desperation: with prey thin and auras failing, patrols press the
        // attack — this is what finally fells trees instead of a full-time stalemate
        if w.role == Role::Patrol && self.leafbare() { m *= 1.5; }
        w.attack * m
    }

    /// How hard this warrior bites wood. Patrol cats gnaw like anyone else; an
    /// ancestor brings its kit's siege trait.
    fn siege_of(&self, i: usize) -> f32 {
        self.warriors[i].ancestor.map(|a| ancestor::kit(a).siege).unwrap_or(1.0)
    }

    /// Per-strike bonus that depends on the *target* — Twofang's finishing bite,
    /// Faryowl's Far Song landing hardest from a distance.
    fn strike_vs(&self, i: usize, target: usize) -> f32 {
        let w = &self.warriors[i];
        let t = &self.warriors[target];
        let mut m = 1.0;
        if w.ancestor == Some(Ancestor::Smilodon) && t.hp < t.max_hp * 0.5 { m *= 1.4; }
        if w.ancestor == Some(Ancestor::Serval) && dist(w.x, w.z, t.x, t.z) > 4.0 { m *= 1.15; }
        if w.ancestor == Some(Ancestor::Jaguar) && (t.slow_t > 0.0 || t.root_t > 0.0 || t.fear_t > 0.0) {
            m *= 1.2;   // The River Remembers: the held are already caught
        }
        if w.ancestor == Some(Ancestor::Lynx) && (t.slow_t > 0.0 || t.root_t > 0.0) {
            // Hush of Snows put the cold there; the lynx should be allowed to
            // collect on it. Measured: Snowhush was the roster's one significant
            // underperformer (44% over 800 games) and its whole passive was value
            // it handed to packmates the AI never spent — a kit paying a bill
            // somebody else forgot to collect.
            m *= 1.25;
        }
        m
    }

    /// On-hit riders that come with a warrior's blows — Snowhush's chill, and
    /// the veil breaking.
    fn on_hit(&mut self, i: usize, target: usize) {
        if self.warriors[i].ancestor == Some(Ancestor::Lynx) {
            let t = &mut self.warriors[target];
            t.slow_t = t.slow_t.max(0.5);   // Hush of Snows
        }
        // **Striking breaks the veil.** You cannot rake a warrior and remain
        // unseen: the blow is where the ambush ends. The bonus is already banked
        // — `strike_of` read the veil when the damage was computed — so a kit
        // still gets its whole opening strike from the dark, and then has to be
        // somewhere. Without this, stealth was the same free lunch the kite was:
        // untargetable in `nearest_enemy` while still landing every blow.
        self.warriors[i].stealth_t = 0.0;
    }

    fn respawn_time(&self, team: u8) -> f32 {
        // MOBA death timers: they grow as the match ages, so a late lost fight is a
        // real window at the camp — herbs claw some of that time back
        let age = 1.0 + self.time / 240.0;
        RESPAWN_BASE * age * HERB_RESPAWN_CUT.powi(self.clans[team as usize].herbs.min(6) as i32)
    }
    fn enemy(team: u8) -> u8 { 1 - team }

    /// Default rosters: the player leads clan A; packmates and the enemy clan fill
    /// deterministically from the pool in order, starting after the player's pick —
    /// so which five ancestors you face varies with who you bring.
    pub fn new(player: Ancestor) -> Battleground {
        let pool = ancestor::ALL;
        let start = pool.iter().position(|&p| p == player).unwrap_or(0);
        let mut order = (1..pool.len()).map(|k| pool[(start + k) % pool.len()]);
        let a = [player, order.next().unwrap(), order.next().unwrap()];
        let b = [order.next().unwrap(), order.next().unwrap(), order.next().unwrap()];
        Battleground::new_draft(a, b)
    }

    /// A drafted match: each clan fields exactly the three ancestors picked for it.
    /// Clan A's first pick is the player's warrior.
    /// The battleground bakes a **craggier land** than the other modes: the war is
    /// fought over broken country — ridgelines, deeper draws, real relief — using
    /// the same per-mode terrain-pool knob prowl already rolls (house idiom, held
    /// under the terrain lock like every other mode's bake).
    pub const MOUNTAIN_BIAS: f32 = 1.35;
    /// Paired with the bias: lift the land so craggier does not mean *wetter* —
    /// the deepened basins drain back to marshes and draws, the carved river stays.
    pub const LAND_LIFT: f32 = 1.2;

    pub fn new_draft(picks_a: [Ancestor; 3], picks_b: [Ancestor; 3]) -> Battleground {
        Battleground::new_on(picks_a, picks_b, 0)
    }

    /// A match between clans of any size from one to [`MAX_CLAN`]. The
    /// three-a-side constructors above are this with a fixed roster; five a
    /// side is what the lobby drafts and what the map's lanes were shaped for.
    pub fn new_roster(
        picks_a: &[Ancestor],
        picks_b: &[Ancestor],
        seed: u32,
        policy: LanePolicy,
    ) -> Battleground {
        Battleground::build(picks_a, picks_b, seed, policy)
    }

    /// A drafted match on a re-rolled land. `seed` salts the terrain noise (see
    /// [`world::set_terrain_seed`]) — the same camps, trails, river course and
    /// jungle layout, over different country. This is what lets a balance run
    /// vary the map instead of replaying one map thirteen times.
    pub fn new_on(picks_a: [Ancestor; 3], picks_b: [Ancestor; 3], seed: u32) -> Battleground {
        Battleground::new_with(picks_a, picks_b, seed, LanePolicy::Kit)
    }

    /// As [`new_on`], with the laning doctrine chosen explicitly — so a balance
    /// run can play both doctrines over the same drafts on the same lands
    /// instead of comparing two runs that differ in more than one thing.
    pub fn new_with(
        picks_a: [Ancestor; 3],
        picks_b: [Ancestor; 3],
        seed: u32,
        policy: LanePolicy,
    ) -> Battleground {
        Battleground::build(&picks_a, &picks_b, seed, policy)
    }

    fn build(
        picks_a: &[Ancestor],
        picks_b: &[Ancestor],
        seed: u32,
        policy: LanePolicy,
    ) -> Battleground {
        let _terrain_guard = world::TERRAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        world::set_terrain_pool(Self::MOUNTAIN_BIAS, 1);
        world::set_land_lift(Self::LAND_LIFT);
        world::set_river_course(world::RIVER_U_EAST);   // the U on the solo-lane side; the camp line stays dry
        world::set_terrain_seed(seed);
        world::init_world();
        // Clan B's camp is clan A's, reflected — not a second independent search.
        // `land_near`'s slope probe is one-sided (it looks +x and +z), so two
        // searches on mirrored ground can still settle a metre or two apart, and
        // every Heal Tree and medic den is placed relative to a camp.
        let a_camp = land_near(CAMP_ANCHOR[0].0, CAMP_ANCHOR[0].1);
        let camps = [a_camp, junglemap::reflect(a_camp.0, a_camp.1)];
        // The guard is held to the end of the build. It used to be dropped here and taken
        // again for the fishing runs below, and in that gap a bake elsewhere (a parallel
        // test's) swapped the grid: two battlegrounds built back to back found their fish at
        // different water, and a lockstep pair desynced at its first checkpoint ("ticks: 30
        // and 30", one suite run in eight). Everything between reads the country.

        let medic_a = (camps[0].0 + 4.0, camps[0].1);
        let medics = [medic_a, junglemap::reflect(medic_a.0, medic_a.1)];

        // Heal Trees: one per lane, a third of the way out from each camp.
        let mut heal_trees = Vec::new();
        for team in 0..2u8 {
            for lane in 0..LANES {
                let (cx, cz) = camps[team as usize];
                let (bx, bz) = TRAIL_BEND[lane];
                let (x, z) = (cx + (bx - cx) * 0.55, cz + (bz - cz) * 0.55);
                heal_trees.push(HealTree { team, lane, x, z, hp: HEAL_TREE_HP });
            }
        }

        // Prey camps (mice and rabbits alternating), a fish run at the nearest
        // water to the centre, and the contention ground's rich prey.
        let camps_of_prey: Vec<(f32, f32)> = PREY_CAMPS_BASE
            .iter()
            .copied()
            .chain(PREY_CAMPS_BASE.iter().map(|&(x, z)| junglemap::reflect(x, z)))
            .collect();
        let mut prey: Vec<Prey> = camps_of_prey.iter().enumerate()
            .map(|(i, &(x, z))| {
                let kind = if i % 2 == 0 { PreyKind::Mouse } else { PreyKind::Rabbit };
                Prey { x, z, home: (x, z), kind, hp: kind.hp(), respawn_t: 0.0 }
            })
            .collect();
        {
            // two fishing runs, sought from mirrored anchors so neither clan's side
            // of the river eats better by construction (under the build's guard)
            for anchor in [(60.0, -110.0), junglemap::reflect(60.0, -110.0)] {   // the U's two arms, mirrored
                if let Some((fx, fz)) = world::nearest_water(anchor, 100.0) {
                    prey.push(Prey { x: fx, z: fz, home: (fx, fz), kind: PreyKind::Fish, hp: PreyKind::Fish.hp(), respawn_t: 0.0 });
                }
            }
        }
        prey.push(Prey { x: 0.0, z: 0.0, home: (0.0, 0.0), kind: PreyKind::Rich, hp: PreyKind::Rich.hp(), respawn_t: 0.0 });

        let herb_patches = HERB_SPOTS_BASE
            .iter()
            .copied()
            .chain(HERB_SPOTS_BASE.iter().map(|&(x, z)| junglemap::reflect(x, z)))
            .map(|(x, z)| HerbPatch { x, z, regrow_t: 0.0 })
            .collect();

        // Rosters: whatever each clan drafted, up to five; the player leads clan A.
        let mut warriors = Vec::new();
        for team in 0..2u8 {
            let side: &[Ancestor] = if team == CLAN_A { picks_a } else { picks_b };
            for slot in 0..side.len().min(MAX_CLAN) {
                let a = side[slot];
                let k = ancestor::kit(a);
                let (cx, cz) = camps[team as usize];
                // Spawn offsets are REFLECTED for clan B, not copied. The mirror
                // probe caught this at tick zero: both clans fanning out along
                // +x put clan B's line across its own camp instead of along it,
                // a 2.8 m break in the mirror before a single step was taken.
                let d = slot as f32 * 2.0 - (side.len().min(MAX_CLAN) as f32 - 1.0);
                let (sx, sz) = if team == CLAN_A { (cx + d, cz) } else { (cx, cz - d) };
                warriors.push(Warrior {
                    team, role: Role::Ancestor, ancestor: Some(a), lane: lane_for(slot, side, policy),
                    x: sx, z: sz, heading: 0.0,
                    hp: k.hp, max_hp: k.hp, attack: k.attack, speed: k.speed, reach: k.reach,
                    mind: Mind::Trail, carry: Carry::Nothing,
                    shield: 0.0, shield_t: 0.0, stealth_t: 0.0, fear_t: 0.0, slow_t: 0.0, root_t: 0.0, pulled_t: 0.0,
                    cds: [0.0; 3], respawn_t: 0.0, gather_t: 0.0, kills: 0, prey_taken: 0, seat: slot as u32, still_t: 0.0, duty: Duty::Border,
                    // Both clans' seat 0 can be driven — clan A's by the local
                    // player, clan B's by a human across a lockstep link. Until
                    // somebody actually drives one it is an ordinary AI warrior.
                    is_player: slot == 0, seg: 0,
                });
            }
        }

        Battleground {
            warriors, heal_trees, prey, herb_patches, zones: Vec::new(),
            clans: [
                ClanState { pile: 0.0, herbs: 0, great_tree_hp: GREAT_TREE_HP },
                ClanState { pile: 0.0, herbs: 0, great_tree_hp: GREAT_TREE_HP },
            ],
            camps, medics, contention: (0.0, 0.0), contention_holder: None,
            skyrock: (-92.0, 92.0), skyrock_holder: None,
            time: 0.0, outcome: None, patrol_t: [PATROL_INTERVAL * 0.3; 2], wave_n: [0; 2], tick: 0,
            wombo: false, collapse_on_breach: false, spoils: false,
            player_driven: [false; 2], input: [None; 2],
        }
    }

    /// Drive clan A's leader — the local player in a single-clan game.
    pub fn set_input(&mut self, mx: f32, mz: f32, claw: bool, ability: Option<usize>) {
        self.set_input_for(CLAN_A, mx, mz, claw, ability);
    }

    /// Drive either clan's leader. The second side exists for
    /// [`crate::lockstep`], which was already carrying and comparing clan B's
    /// frame across the wire while the sim had nowhere to put it.
    pub fn set_input_for(&mut self, side: u8, mx: f32, mz: f32, claw: bool, ability: Option<usize>) {
        let s = (side as usize).min(1);
        self.player_driven[s] = true;
        self.input[s] = Some((mx, mz, claw, ability));
    }

    /// The warrior a given clan's human drives, if anyone is driving.
    pub fn driven(&self, side: u8) -> Option<usize> {
        if !self.player_driven[(side as usize).min(1)] {
            return None;
        }
        self.warriors
            .iter()
            .position(|w| w.team == side && w.is_player && w.role == Role::Ancestor)
    }

    fn spawn_patrol(&mut self, team: u8, lane: usize, duty: Duty) {
        let (cx, cz) = self.camps[team as usize];
        let wave_seat = self.wave_n[team as usize];
        let size = if self.leafbare() && duty == Duty::Border { LEAFBARE_PATROL } else { PATROL_SIZE };
        for i in 0..size {
            // reflected for clan B, like the ancestors' spawns
            let (px, pz) = if team == CLAN_A {
                (cx + i as f32 * 1.5, cz + 1.5)
            } else {
                (cx - 1.5, cz - i as f32 * 1.5)
            };
            self.warriors.push(Warrior {
                team, role: Role::Patrol, ancestor: None, lane,
                x: px, z: pz, heading: 0.0,
                hp: PATROL_HP, max_hp: PATROL_HP, attack: PATROL_ATK, speed: PATROL_SPEED, reach: REACH,
                mind: if duty == Duty::Hunting { Mind::Hunt } else { Mind::Trail },
                carry: Carry::Nothing,
                shield: 0.0, shield_t: 0.0, stealth_t: 0.0, fear_t: 0.0, slow_t: 0.0, root_t: 0.0, pulled_t: 0.0,
                cds: [0.0; 3], respawn_t: 0.0, gather_t: 0.0, kills: 0, prey_taken: 0,
                seat: 100 + wave_seat * 8 + i as u32, still_t: 0.0, duty,
                is_player: false, seg: 0,
            });
        }
    }

    /// Where the border broke: the lane of a fallen enemy Heal Tree, if any. Once a
    /// breach exists the clan masses its attack there — patrols spawn onto it and
    /// warriors returning to the trail adopt it — which is what turns a three-lane
    /// stalemate into a decisive push on the Great Tree.
    fn breach_lane(&self, team: u8) -> Option<usize> {
        self.heal_trees.iter().find(|t| t.team != team && !t.alive()).map(|t| t.lane)
    }

    /// The trail waypoints for a lane, in this team's marching order.
    fn trail(&self, team: u8, lane: usize) -> [(f32, f32); 3] {
        let home = self.camps[team as usize];
        let away = self.camps[Self::enemy(team) as usize];
        [home, TRAIL_BEND[lane], away]
    }

    fn under_aura(&self, team: u8, x: f32, z: f32) -> bool {
        self.heal_trees.iter().any(|t| t.team == team && t.alive() && dist(t.x, t.z, x, z) < AURA_R)
    }

    /// Damage a warrior, shields first; the aura of a friendly Heal Tree softens it.
    fn hurt(&mut self, idx: usize, mut dmg: f32) {
        let (team, x, z) = { let w = &self.warriors[idx]; (w.team, w.x, w.z) };
        if self.under_aura(team, x, z) { dmg *= 1.0 - AURA_GUARD; }
        let respawn = self.respawn_time(team);
        let w = &mut self.warriors[idx];
        let absorbed = dmg.min(w.shield);
        w.shield -= absorbed;
        w.hp -= dmg - absorbed;
        if w.hp <= 0.0 && w.mind != Mind::Dead {
            w.hp = 0.0;
            w.mind = Mind::Dead;
            // a carried kill or sprig is dropped and lost
            w.carry = Carry::Nothing;
            w.shield = 0.0; w.shield_t = 0.0; w.stealth_t = 0.0;
            if w.role == Role::Ancestor {
                w.respawn_t = respawn;
            }
        }
    }

    fn heal(&mut self, idx: usize, amount: f32) {
        let team = self.warriors[idx].team;
        let m = self.herb_heal(team);
        let w = &mut self.warriors[idx];
        w.hp = (w.hp + amount * m).min(w.max_hp);
    }

    /// Spotting-range multiplier for looking DOWN a slope: from `dh` metres above
    /// the target (3+), eyes carry up to 40% farther. Pure, so it unit-tests.
    fn downhill_bonus(dh: f32) -> f32 {
        if dh >= 3.0 { 1.0 + ((dh - 3.0) / 6.0).clamp(0.0, 1.0) * 0.2 + 0.2 } else { 1.0 }
    }
    /// From high enough above (4m+), and close enough (30m), a watcher sees down
    /// INTO brush and mist — cover is a roof, not a dome. Stone is still stone.
    fn pierces_cover(dh: f32, d: f32) -> bool { dh >= 4.0 && d < 30.0 }

    /// The enemy a warrior picks out of the ones it can see. Not simply the
    /// nearest: among everything visible and in range, a warrior prefers the one
    /// **closest to falling**, because three cats each chewing a different
    /// healthy enemy kill nobody and three cats finishing one kill a warrior.
    ///
    /// This is also the change that makes kit power *visible*. Measured, over
    /// half of all matches were decided by which orientation the chaos favoured
    /// rather than by which composition was stronger — a roster cannot express
    /// itself through an AI that spreads its damage at random. Distance still
    /// matters (a target three lengths away beats one across the clearing), it
    /// just no longer decides alone.
    fn nearest_enemy(&self, of: usize, within: f32) -> Option<usize> {
        let (team, x, z) = { let w = &self.warriors[of]; (w.team, w.x, w.z) };
        let vh = world::height_at(x, z);
        // the Skyrock's watchers yowl warnings: the holding clan spots a quarter farther
        let clan_mult = if self.skyrock_holder == Some(team) { 1.25 } else { 1.0 };
        let mut best: Option<(usize, f32)> = None;
        for (i, e) in self.warriors.iter().enumerate() {
            if e.team == team || !e.alive() || e.stealth_t > 0.0 { continue; }
            let d = dist(x, z, e.x, e.z);
            let dh = vh - world::height_at(e.x, e.z);
            if !junglemap::sees(x, z, e.x, e.z) {
                // elevation can look down into brush — never through stone
                let over_cover = junglemap::concealment_at(e.x, e.z).is_some()
                    && !junglemap::wall_between(x, z, e.x, e.z)
                    && Self::pierces_cover(dh, d);
                if !over_cover { continue; }
            }
            let reach = within * clan_mult * Self::downhill_bonus(dh);
            if d >= reach {
                continue;
            }
            // score: lower is better. Distance in lengths, plus a penalty for how
            // hale the target is — a wounded cat within a few lengths outbids a
            // whole one nearby, and nothing outbids something about to drop.
            let frac = (e.hp / e.max_hp).clamp(0.0, 1.0);
            let score = d + frac * FOCUS_WEIGHT;
            if best.map_or(true, |b| score < b.1) {
                best = Some((i, score));
            }
        }
        best.map(|b| b.0)
    }

    /// Fire ability `slot` (0/1 = techniques, 2 = ult) for warrior `i` if off cooldown.
    fn cast(&mut self, i: usize, slot: usize) {
        let (a, team, x, z) = {
            let w = &self.warriors[i];
            match (w.alive(), w.ancestor) { (true, Some(a)) => (a, w.team, w.x, w.z), _ => return }
        };
        if self.warriors[i].cds[slot] > 0.0 { return; }
        let k = ancestor::kit(a);
        let spec = if slot == 2 { k.ult } else { k.abilities[slot] };
        let atk = self.strike_of(i);
        let target = self.nearest_enemy(i, spec.range.max(REACH));
        match spec.verb {
            Verb::Strike => {
                let Some(t) = target else { return };
                self.hurt(t, spec.power * atk * self.strike_vs(i, t));
                self.on_hit(i, t);
                self.warriors[i].kills += (!self.warriors[t].alive()) as u32;
            }
            Verb::Dash => {
                let Some(t) = target else { return };
                let (tx, tz) = (self.warriors[t].x, self.warriors[t].z);
                let d = dist(x, z, tx, tz);
                let (nx, nz) = norm(tx - x, tz - z);
                let step = spec.range.min(d - REACH * 0.5).max(0.0);
                let bonus = self.strike_vs(i, t);
                let w = &mut self.warriors[i];
                w.x += nx * step; w.z += nz * step;
                self.hurt(t, spec.power * atk * bonus);
            }
            Verb::Aoe => {
                let victims: Vec<usize> = self.warriors.iter().enumerate()
                    .filter(|(_, e)| e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range)
                    .map(|(j, _)| j).collect();
                if victims.is_empty() { return; }
                for v in victims { self.hurt(v, spec.power * atk); }
            }
            Verb::Shield => {
                let amount = spec.power * atk;
                for j in 0..self.warriors.len() {
                    let w = &self.warriors[j];
                    if w.team == team && w.alive() && dist(x, z, w.x, w.z) < spec.range.max(0.1) {
                        self.warriors[j].shield = self.warriors[j].shield.max(amount);
                        self.warriors[j].shield_t = self.warriors[j].shield_t.max(spec.dur);
                    }
                }
            }
            Verb::Heal => {
                let amount = spec.power * atk;
                for j in 0..self.warriors.len() {
                    let w = &self.warriors[j];
                    if w.team == team && w.alive() && dist(x, z, w.x, w.z) < spec.range.max(0.1) {
                        self.heal(j, amount);
                    }
                }
            }
            Verb::Stealth => {
                let dur = if a == Ancestor::Panther { spec.dur * 1.5 } else { spec.dur };   // Silent Footfall
                self.warriors[i].stealth_t = dur;
            }
            Verb::Fear => {
                for j in 0..self.warriors.len() {
                    let e = &self.warriors[j];
                    if e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range {
                        self.warriors[j].fear_t = spec.dur;
                        if spec.power > 0.0 { self.hurt(j, spec.power * atk); }   // the Great Caterwaul draws blood
                    }
                }
            }
            Verb::Knockback => {
                // smash everything close away from the caster — peel divers off a
                // tree, fling a brawl apart, buy a wounded packmate its retreat
                let victims: Vec<usize> = self.warriors.iter().enumerate()
                    .filter(|(_, e)| e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range)
                    .map(|(j, _)| j).collect();
                if victims.is_empty() { return; }
                for v in victims {
                    let (vx, vz) = (self.warriors[v].x, self.warriors[v].z);
                    let (mut nx, mut nz) = norm(vx - x, vz - z);
                    if nx == 0.0 && nz == 0.0 { nx = 1.0; nz = 0.0; }
                    let w = &mut self.warriors[v];
                    w.x = (w.x + nx * 5.0).clamp(-ARENA, ARENA);
                    w.z = (w.z + nz * 5.0).clamp(-ARENA, ARENA);
                    self.hurt(v, spec.power * atk);
                }
            }
            Verb::Execute => {
                // the killing bite: its power grows with the target's missing strength
                let Some(t) = target else { return };
                let missing = 1.0 - self.warriors[t].hp / self.warriors[t].max_hp;
                self.hurt(t, spec.power * atk * (1.0 + missing * 1.2) * self.strike_vs(i, t));
                self.warriors[i].kills += (!self.warriors[t].alive()) as u32;
            }
            Verb::Zone => {
                self.zones.push(Zone { x, z, team, r: spec.range, dps: spec.power * atk / spec.dur.max(0.5), t: spec.dur, mend: false });
            }
            Verb::Rain => {
                self.zones.push(Zone { x, z, team, r: spec.range, dps: spec.power * atk / spec.dur.max(0.5), t: spec.dur, mend: true });
            }
            Verb::Root => {
                for j in 0..self.warriors.len() {
                    let e = &self.warriors[j];
                    if e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range {
                        self.warriors[j].root_t = self.warriors[j].root_t.max(spec.dur);
                        if spec.power > 0.0 { self.hurt(j, spec.power * atk); }
                    }
                }
            }
            Verb::Pull => {
                // the current takes their legs: drag every enemy near in close
                let victims: Vec<usize> = self.warriors.iter().enumerate()
                    .filter(|(_, e)| e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range)
                    .map(|(j, _)| j).collect();
                if victims.is_empty() { return; }
                for v in victims {
                    let (vx, vz) = (self.warriors[v].x, self.warriors[v].z);
                    let d = dist(x, z, vx, vz);
                    let (nx, nz) = norm(x - vx, z - vz);
                    let step = (d - REACH * 0.6).max(0.0);   // in to the claw, not inside the caster
                    let w = &mut self.warriors[v];
                    w.x += nx * step; w.z += nz * step;
                    w.pulled_t = 1.5;
                    if spec.power > 0.0 { self.hurt(v, spec.power * atk); }
                }
            }
            Verb::Slow => {
                for j in 0..self.warriors.len() {
                    let e = &self.warriors[j];
                    if e.team != team && e.alive() && dist(x, z, e.x, e.z) < spec.range {
                        let dur = if e.ancestor == Some(Ancestor::Puma) { spec.dur * 0.5 } else { spec.dur };   // Sure-Footed
                        self.warriors[j].slow_t = dur;
                        if spec.power > 0.0 { self.hurt(j, spec.power * atk); }
                    }
                }
            }
        }
        // any ability that draws blood ends the ambush too — but Veilwalker
        // itself, and the shields and heals, do not
        let bloodies = !matches!(spec.verb, Verb::Stealth | Verb::Shield | Verb::Heal | Verb::Rain)
            && spec.power > 0.0;
        if bloodies {
            self.warriors[i].stealth_t = 0.0;
        }
        self.warriors[i].cds[slot] = spec.cd;
    }

    /// One warrior's move speed here and now: terrain, wounds, burdens, magic.
    fn speed_of(&self, i: usize) -> f32 {
        let w = &self.warriors[i];
        let mut s = w.speed;
        if world::height_at(w.x, w.z) < world::WATER_Y + 0.2 {
            // water slows everyone except the Riverborn; Sure-Footed paws barely notice
            s *= match w.ancestor {
                Some(Ancestor::Leopard) => 1.1,
                Some(Ancestor::Puma) => 0.75,
                _ => 0.55,
            };
        }
        if w.ancestor == Some(Ancestor::Panther) && self.is_night() { s *= 1.1; }   // Silent Footfall
        if w.ancestor == Some(Ancestor::Caracal) {                                   // Skyward Hunger
            s *= 1.0 + (w.kills as f32 * 0.04).min(0.4);
        }
        if junglemap::on_wall(w.x, w.z) { s *= junglemap::SCRAMBLE; }   // claws on stone
        if w.carry != Carry::Nothing { s *= CARRY_SLOW; }
        if w.slow_t > 0.0 { s *= 0.5; }
        s
    }

    fn walk_toward(&mut self, i: usize, tx: f32, tz: f32, dt: f32) {
        if self.warriors[i].root_t > 0.0 { return; }   // held fast in the drifts
        let s = self.speed_of(i);
        let w = &mut self.warriors[i];
        let (nx, nz) = norm(tx - w.x, tz - w.z);
        w.x = (w.x + nx * s * dt).clamp(-ARENA, ARENA);
        w.z = (w.z + nz * s * dt).clamp(-ARENA, ARENA);
        if nx != 0.0 || nz != 0.0 { w.heading = nz.atan2(nx); }
    }

    fn think(&mut self, i: usize, dt: f32) {
        let (team, role, mind, x, z, frac, carry, lane) = {
            let w = &self.warriors[i];
            (w.team, w.role, w.mind, w.x, w.z, w.power_frac(), w.carry, w.lane)
        };
        let home = self.camps[team as usize];
        let enemy_camp = self.camps[Self::enemy(team) as usize];

        // fear overrides thought: run from the roar, toward home (unless held fast)
        if self.warriors[i].fear_t > 0.0 {
            self.walk_toward(i, home.0, home.1, dt);   // no-op while rooted
            return;
        }

        // the Great Tree's defence overrides retreat — the one fight to the death
        let tree_threatened = self.warriors.iter().any(|e|
            e.team != team && e.alive() && dist(e.x, e.z, home.0, home.1) < LAST_STAND_R);
        if tree_threatened && dist(x, z, home.0, home.1) < LAST_STAND_R * 2.0 && mind != Mind::Dead {
            self.warriors[i].mind = Mind::LastStand;
            if let Some(t) = self.nearest_enemy(i, LAST_STAND_R * 2.0) {
                let (tx, tz) = (self.warriors[t].x, self.warriors[t].z);
                if dist(x, z, tx, tz) > self.warriors[i].reach { self.walk_toward(i, tx, tz, dt); }
                else {
                    let dmg = self.strike_of(i) * self.strike_vs(i, t) * dt;
                    self.hurt(t, dmg);
                    self.on_hit(i, t);
                }
                if role == Role::Ancestor { self.auto_cast(i); }
            }
            return;
        } else if mind == Mind::LastStand {
            self.warriors[i].mind = Mind::Trail;
        }

        // Wounded past nerve → break off and go home (patrol doctrine; ancestors
        // too). And the other half of the same doctrine, which was missing: a
        // patrol that meets a bigger patrol withdraws. Nerve is not only about
        // how hurt you are, it is about the odds — a cat at full strength facing
        // three does not have good odds, and the books' patrols know it.
        let outnumbered = {
            let mut foes = 0i32;
            let mut kin = 0i32;
            for w in self.warriors.iter() {
                if !w.alive() || dist(w.x, w.z, x, z) > ODDS_R {
                    continue;
                }
                if w.team == team {
                    kin += 1;   // counts self, which is right: you are one cat
                } else if w.stealth_t <= 0.0 && junglemap::sees(x, z, w.x, w.z) {
                    foes += 1;  // you cannot fear what you cannot see
                }
            }
            foes - kin >= ODDS_MARGIN
        };
        if (frac < NERVE || outnumbered)
            && !matches!(mind, Mind::Retreat | Mind::AtMedic | Mind::Dead)
        {
            self.warriors[i].mind = Mind::Retreat;
        }

        match self.warriors[i].mind {
            Mind::Dead => {}
            Mind::LastStand => {}   // handled above
            Mind::Retreat => {
                // A wounded cat makes for the nearest sanctuary, not the far
                // camp. This is what the Trees of Healing are FOR, and it was
                // measured missing: a diagnostic match had Oldroar spending 79s
                // retreating and 7s actually healing — the slow kits were dying
                // on the walk home, then dying again, 67s of a 506s match spent
                // dead, and never once reaching the economy. Shelter under a
                // living Heal Tree mends more slowly than the medic cat's den
                // but it is *reachable*, which for a heavy kit is the whole
                // difference.
                let (mx, mz) = self.medics[team as usize];
                let mut best = (mx, mz, dist(x, z, mx, mz));
                for t in self.heal_trees.iter() {
                    if t.team != team || !t.alive() { continue; }
                    let d = dist(x, z, t.x, t.z);
                    if d < best.2 { best = (t.x, t.z, d); }
                }
                let (tx, tz, d) = best;
                let at_camp = (tx, tz) == (mx, mz);
                if at_camp && d < MEDIC_R {
                    self.warriors[i].mind = Mind::AtMedic;
                } else if !at_camp && d < AURA_R * 0.5 {
                    // sheltering: the aura pass in step() does the mending
                    if self.warriors[i].power_frac() >= REJOIN {
                        if let Some(l) = self.breach_lane(team) { self.warriors[i].lane = l; }
                        self.warriors[i].mind = Mind::Trail;
                        self.warriors[i].seg = 0;
                    }
                } else {
                    self.walk_toward(i, tx, tz, dt);
                }
            }
            Mind::AtMedic => {
                // Only at the den. `think` never sets this state from far away,
                // so in play it never mattered — but the heal was unconditional,
                // which meant anything that put a warrior into AtMedic healed it
                // wherever it stood. Two tests were written around that quirk
                // before it was a bug; now it is a rule.
                let (mx, mz) = self.medics[team as usize];
                if dist(x, z, mx, mz) > MEDIC_R {
                    self.warriors[i].mind = Mind::Retreat;
                    return;
                }
                self.heal(i, MEDIC_HEAL * dt);
                if self.warriors[i].power_frac() >= REJOIN {
                    if let Some(l) = self.breach_lane(team) { self.warriors[i].lane = l; }
                    self.warriors[i].mind = Mind::Trail;
                    self.warriors[i].seg = 0;
                }
            }
            Mind::CarryHome => {
                if dist(x, z, home.0, home.1) < MEDIC_R {
                    match carry {
                        Carry::Freshkill(worth) => self.clans[team as usize].pile += worth,
                        Carry::Herb => self.clans[team as usize].herbs += 1,
                        Carry::Nothing => {}
                    }
                    self.warriors[i].carry = Carry::Nothing;
                    if let Some(l) = self.breach_lane(team) { self.warriors[i].lane = l; }
                    self.warriors[i].mind =
                        if role == Role::Patrol && self.warriors[i].duty == Duty::Hunting { Mind::Hunt }
                        else { Mind::Trail };
                    self.warriors[i].seg = 0;
                } else {
                    self.walk_toward(i, home.0, home.1, dt);
                }
            }
            Mind::Gather => {
                // nearest patch with a sprig ready — walk there, crouch, pick
                let patch = self.herb_patches.iter().enumerate().filter(|(_, p)| p.ready())
                    .min_by(|a, b| dist(a.1.x, a.1.z, x, z).total_cmp(&dist(b.1.x, b.1.z, x, z)))
                    .map(|(j, _)| j);
                match patch {
                    Some(p) => {
                        let (px, pz) = (self.herb_patches[p].x, self.herb_patches[p].z);
                        if dist(x, z, px, pz) > 2.5 {
                            self.warriors[i].gather_t = 0.0;
                            self.walk_toward(i, px, pz, dt);
                        } else {
                            self.warriors[i].gather_t += dt;
                            if self.warriors[i].gather_t >= HERB_GATHER {
                                self.herb_patches[p].regrow_t = HERB_REGROW;
                                self.warriors[i].gather_t = 0.0;
                                self.warriors[i].carry = Carry::Herb;
                                self.warriors[i].mind = Mind::CarryHome;
                            }
                        }
                    }
                    None => { self.warriors[i].mind = Mind::Trail; self.warriors[i].gather_t = 0.0; }
                }
            }
            Mind::Hunt => {
                // a hunting patrol ranges the whole territory; an ancestor only detours nearby
                let reach = if role == Role::Patrol { 1e9 } else { 40.0 };
                let target = self.prey.iter().enumerate()
                    .filter(|(_, p)| p.alive() && dist(p.x, p.z, x, z) < reach
                        && (!p.rich() || self.contention_holder == Some(team) || self.contention_holder.is_none()))
                    .min_by(|a, b| dist(a.1.x, a.1.z, x, z).total_cmp(&dist(b.1.x, b.1.z, x, z)))
                    .map(|(j, _)| j);
                match target {
                    Some(p) => {
                        let (px, pz) = (self.prey[p].x, self.prey[p].z);
                        if dist(x, z, px, pz) > self.warriors[i].reach { self.walk_toward(i, px, pz, dt); }
                        else {
                            self.prey[p].hp -= self.warriors[i].attack * dt;
                            if self.prey[p].hp <= 0.0 {
                                let kind = self.prey[p].kind;
                                self.prey[p].respawn_t = PREY_RESPAWN;
                                self.prey[p].hp = kind.hp();
                                let (hx0, hz0) = self.prey[p].home;
                                self.prey[p].x = hx0; self.prey[p].z = hz0;
                                self.warriors[i].carry = Carry::Freshkill(kind.worth());
                                self.warriors[i].mind = Mind::CarryHome;
                                self.warriors[i].prey_taken += 1;
                            }
                        }
                    }
                    None => self.warriors[i].mind = Mind::Trail,
                }
            }
            Mind::Trail | Mind::Fight => {
                // engage a nearby visible enemy first
                if let Some(t) = self.nearest_enemy(i, ENGAGE_R) {
                    self.warriors[i].mind = Mind::Fight;
                    let (tx, tz) = (self.warriors[t].x, self.warriors[t].z);
                    let reach = self.warriors[i].reach;
                    let d = dist(x, z, tx, tz);
                    if d > reach {
                        self.walk_toward(i, tx, tz, dt);
                    } else {
                        // A ranged cat keeps its distance — but backing away and
                        // biting are not the same motion. Measured: with the kite
                        // free, Faryowl outran every kit slower than 6.4 while
                        // still landing every yowl, and finished 61% across 160
                        // games, the last kit outside ±7. Now giving ground costs
                        // the blow, which is what it costs a real cat.
                        let kiting = reach > 4.0 && d < reach * 0.45;
                        if kiting {
                            let (nx, nz) = norm(x - tx, z - tz);
                            self.walk_toward(i, x + nx * 6.0, z + nz * 6.0, dt);
                        }
                        // A yowl thrown over the shoulder is a weaker yowl, not a
                        // silent one. Both extremes were measured across 160
                        // games: kiting free left Faryowl at 61% (dispersion sd
                        // 4.2 overall), kiting silent dropped it to 42% and took
                        // Stillsmoke to 37% with it (sd 6.2). Half is the version
                        // that keeps the trade real without deleting the archetype.
                        let mult = if kiting { KITE_STRIKE } else { 1.0 };
                        let dmg = self.strike_of(i) * self.strike_vs(i, t) * mult * dt;
                        let was_alive = self.warriors[t].alive();
                        self.hurt(t, dmg);
                        self.on_hit(i, t);
                        if was_alive && !self.warriors[t].alive() {
                            self.warriors[i].kills += 1;
                        }
                    }
                    if role == Role::Ancestor { self.auto_cast(i); }
                    return;
                }
                self.warriors[i].mind = Mind::Trail;

                // A broken border is where the clan goes. Patrol spawns and
                // respawning warriors already funnelled onto a breach; the cats
                // already out on an intact trail did not, so a clan could fell a
                // Heal Tree and then keep walking three separate lanes past the
                // hole it had just made. Wounded cats are exempt — they are
                // heading for a sanctuary, and a push is not worth a life.
                if self.collapse_on_breach
                    && role == Role::Ancestor
                    && self.warriors[i].power_frac() > NERVE * 1.5
                {
                    if let Some(b) = self.breach_lane(team) {
                        if self.warriors[i].lane != b {
                            // …but somebody holds the lane you are leaving. The
                            // first version of this sent *every* healthy warrior,
                            // so a clan traded one broken border for two empty
                            // ones and measured worse than not pushing at all.
                            // The lowest seat stays — deterministic, and it means
                            // the same cat holds rather than the lane flickering
                            // between whoever thought of it first this tick.
                            let my_seat = self.warriors[i].seat;
                            let my_lane = self.warriors[i].lane;
                            let holder = self
                                .warriors
                                .iter()
                                .filter(|w| {
                                    w.team == team
                                        && w.role == Role::Ancestor
                                        && w.alive()
                                        && w.lane == my_lane
                                })
                                .map(|w| w.seat)
                                .min();
                            if holder != Some(my_seat) {
                                self.warriors[i].lane = b;
                                self.warriors[i].seg = 0;
                            }
                        }
                    }
                }

                // ancestors sometimes peel off the trail for the economy objectives
                // `is_player` gates the AI economy detour so a human's warrior is
                // never yanked off to hunt — but when nobody is driving, that
                // gate is pure asymmetry: clan A's seat 0 was the only warrior on
                // the field that never left the trail to farm. The mirror probe
                // found it as a 0.6 m break at tick 34, and it is worth more than
                // that over a match, since the economy is what wins them.
                let hands_off =
                    self.warriors[i].is_player && self.player_driven[team as usize];
                if role == Role::Ancestor && !hands_off && carry == Carry::Nothing {
                    // One at a time. A lane held by nobody is a lane given away,
                    // so a warrior only peels off to hunt or gather if its
                    // lane-mate is still on the trail — which is also what keeps
                    // the duo a duo rather than two cats who happen to share a
                    // number.
                    let mate_away = self.warriors.iter().any(|m| {
                        m.team == team
                            && m.role == Role::Ancestor
                            && m.lane == lane
                            && !std::ptr::eq(m, &self.warriors[i])
                            && matches!(m.mind, Mind::Hunt | Mind::Gather | Mind::CarryHome)
                    });
                    let r = if mate_away {
                        1.0
                    } else {
                        hashf(
                            self.tick.wrapping_mul(2654435761)
                                .wrapping_add(self.warriors[i].seat.wrapping_mul(97)),
                        )
                    };
                    if r < 0.005 {
                        if r < 0.0015 && self.herb_patches.iter().any(|p| p.ready()) {
                            self.warriors[i].mind = Mind::Gather;
                            return;
                        } else if self.prey.iter().any(|p| p.alive()) {
                            self.warriors[i].mind = Mind::Hunt;
                            return;
                        }
                    }
                }

                // gnaw enemy structures on the way: heal tree on this lane, then the Great Tree
                let atk = self.strike_of(i);
                let etarget = self.heal_trees.iter().position(|t|
                    t.team != team && t.alive()
                    && (t.lane == lane || dist(t.x, t.z, x, z) < 6.0)
                    && dist(t.x, t.z, x, z) < REACH + 3.0);
                if let Some(t) = etarget {
                    let was_standing = self.heal_trees[t].alive();
                    self.heal_trees[t].hp -= atk * self.siege_of(i) * dt;
                    if was_standing && !self.heal_trees[t].alive() && self.spoils {
                        // the roots give up the prey cached under them
                        self.clans[team as usize].pile += BORDER_SPOILS;
                    }
                    return;
                }
                if dist(x, z, enemy_camp.0, enemy_camp.1) < REACH + 4.0 {
                    let e = Self::enemy(team) as usize;
                    // The Great Tree's hardness scales with how many of its
                    // clan's Heal Trees still stand, rather than being all-or-
                    // nothing. Measured: the clan that broke the enemy's FIRST
                    // border went on to win only 39-48% of the time — breaking a
                    // border was not an advantage, it was a bill. Of course it
                    // was: felling a tree cost time and blood deep in enemy
                    // ground and bought nothing, because two standing roots
                    // hardened the Great Tree exactly as much as three did.
                    // Now each felled tree is a third of that guard gone.
                    let standing = self
                        .heal_trees
                        .iter()
                        .filter(|t| t.team != team && t.alive())
                        .count();
                    let guard = ROOT_GUARD * (standing as f32 / LANES as f32).min(1.0);
                    let dmg = atk * self.siege_of(i) * dt * (1.0 - guard);
                    self.clans[e].great_tree_hp -= dmg;
                    return;
                }

                // otherwise: march the trail
                let wp = self.trail(team, lane);
                let seg = self.warriors[i].seg.min(wp.len() - 1);
                let (tx, tz) = wp[seg];
                if dist(x, z, tx, tz) < 4.0 && seg + 1 < wp.len() { self.warriors[i].seg = seg + 1; }
                let seg = self.warriors[i].seg.min(wp.len() - 1);
                let (tx, tz) = wp[seg];
                self.walk_toward(i, tx, tz, dt);
            }
        }
    }

    /// Simple ancestor ability doctrine while fighting: ult into a crowd, shield or
    /// heal when pressed, otherwise spend a technique.
    fn auto_cast(&mut self, i: usize) {
        let Some(a) = self.warriors[i].ancestor else { return };
        if self.warriors[i].is_player && self.player_driven[self.warriors[i].team as usize] {
            return; // the player casts by input
        }
        let (team, x, z, frac) = { let w = &self.warriors[i]; (w.team, w.x, w.z, w.power_frac()) };
        let k = ancestor::kit(a);
        let in_ult_range = |bg: &Battleground| {
            bg.warriors.iter()
                .filter(|e| e.team != team && e.alive() && dist(x, z, e.x, e.z) < k.ult.range.max(8.0))
                .count()
        };
        let crowd = in_ult_range(self);

        // ── the wombo ──────────────────────────────────────────────────────
        // A pull that nobody follows up is a wasted pull. Riptail's Undertow
        // and Snowhush's Snowbind exist to make a crowd stand still in one
        // place; an ally holding an area ultimate should spend it on THAT
        // moment rather than on whatever two cats happen to be near it later.
        // So: a controller opens if a packmate has an area ult ready, and the
        // packmate holds its ult a few seconds for a gather that is coming.
        // An ultimate is a legend, not a rotation button: it wants a real crowd,
        // one more cat than a technique is worth spending on. Without this the
        // gather kits burn The River Takes on any two scouts and never have it
        // for the fight that decides the lane.
        const ULT_CROWD: usize = 3;
        const TECH_CROWD: usize = 2;
        let has_area_ult = self.wombo
            && matches!(k.ult.verb, Verb::Aoe | Verb::Zone | Verb::Fear | Verb::Slow);
        let gathered = self.warriors.iter().any(|e| {
            e.team != team && e.alive() && (e.root_t > 0.0 || e.pulled_t > 0.0)
                && dist(x, z, e.x, e.z) < k.ult.range.max(8.0)
        });
        if has_area_ult && self.warriors[i].cds[2] <= 0.0 {
            if gathered && crowd >= TECH_CROWD {
                self.cast(i, 2);   // the follow-up: land it on the held
                return;
            }
            // a controller nearby whose opener is up? hold, briefly, for it
            let opener_coming = self.warriors.iter().any(|m| {
                m.team == team
                    && m.alive()
                    && dist(x, z, m.x, m.z) < 14.0
                    && m.ancestor.map_or(false, |ma| {
                        let mk = ancestor::kit(ma);
                        mk.abilities.iter().enumerate().any(|(s, ab)| {
                            matches!(ab.verb, Verb::Pull | Verb::Root) && m.cds[s] <= 0.5
                        })
                    })
            });
            if crowd >= ULT_CROWD && !opener_coming {
                self.cast(i, 2);
                return;
            }
        } else if crowd >= ULT_CROWD && self.warriors[i].cds[2] <= 0.0 {
            self.cast(i, 2);
            return;
        }

        for slot in 0..2 {
            let ab = k.abilities[slot];
            let defensive = matches!(ab.verb, Verb::Shield | Verb::Heal | Verb::Stealth);
            if self.warriors[i].cds[slot] > 0.0 {
                continue;
            }
            // the opener half of the wombo: a pull or a root is worth far more
            // when a packmate can immediately drop an area ult on the result,
            // so fire it into a crowd rather than at the first lone scout
            if matches!(ab.verb, Verb::Pull | Verb::Root) {
                let ally_ready = self.warriors.iter().any(|m| {
                    m.team == team
                        && m.alive()
                        && m.cds[2] <= 0.0
                        && dist(x, z, m.x, m.z) < 14.0
                        && m.ancestor.map_or(false, |ma| {
                            matches!(ancestor::kit(ma).ult.verb, Verb::Aoe | Verb::Zone | Verb::Fear | Verb::Slow)
                        })
                });
                if self.wombo && ally_ready && crowd < TECH_CROWD {
                    continue;   // save it for a gather worth gathering
                }
            }
            if defensive == (frac < 0.7) || !defensive {
                self.cast(i, slot);
                return;
            }
        }
    }

    pub fn step(&mut self, dt: f32) {
        if self.outcome.is_some() { return; }
        self.tick = self.tick.wrapping_add(1);
        self.time += dt;

        // timers
        for w in self.warriors.iter_mut() {
            for c in w.cds.iter_mut() { *c = (*c - dt).max(0.0); }
            w.stealth_t = (w.stealth_t - dt).max(0.0);
            w.fear_t = (w.fear_t - dt).max(0.0);
            w.slow_t = (w.slow_t - dt).max(0.0);
            w.root_t = (w.root_t - dt).max(0.0);
            w.pulled_t = (w.pulled_t - dt).max(0.0);
            w.shield_t = (w.shield_t - dt).max(0.0);
            if w.shield_t <= 0.0 {
                w.shield = 0.0;   // guard-hair settles
            }
        }
        for p in self.prey.iter_mut() { p.respawn_t = (p.respawn_t - dt).max(0.0); }
        // land prey bolts from a close hunter, leashed to its home ground
        for pi in 0..self.prey.len() {
            let p = &self.prey[pi];
            if !p.alive() || !p.kind.flees() { continue; }
            let threat = self.warriors.iter()
                .filter(|w| w.alive() && dist(w.x, w.z, p.x, p.z) < 8.0
                    && junglemap::sees(p.x, p.z, w.x, w.z))   // a hunter in the brush spooks nothing
                .map(|w| (w.x, w.z))
                .min_by(|a, b| dist(a.0, a.1, p.x, p.z).total_cmp(&dist(b.0, b.1, p.x, p.z)));
            let p = &mut self.prey[pi];
            if let Some((tx, tz)) = threat {
                let (mut nx, mut nz) = norm(p.x - tx, p.z - tz);
                if nx == 0.0 && nz == 0.0 {   // paw right on its tail: bolt any which way
                    let a = hashf(self.tick.wrapping_add(pi as u32 * 131)) * std::f32::consts::TAU;
                    nx = a.cos(); nz = a.sin();
                }
                p.x += nx * 4.0 * dt; p.z += nz * 4.0 * dt;
            } else if dist(p.x, p.z, p.home.0, p.home.1) > 1.0 {
                let (nx, nz) = norm(p.home.0 - p.x, p.home.1 - p.z);   // drift home
                p.x += nx * 1.5 * dt; p.z += nz * 1.5 * dt;
            }
            let d = dist(p.x, p.z, p.home.0, p.home.1);
            if d > 12.0 {   // the leash: prey never abandons its ground
                let (nx, nz) = norm(p.x - p.home.0, p.z - p.home.1);
                p.x = p.home.0 + nx * 12.0; p.z = p.home.1 + nz * 12.0;
            }
        }
        for h in self.herb_patches.iter_mut() { h.regrow_t = (h.regrow_t - dt).max(0.0); }

        // patrols march out, one lane after another
        for team in 0..2u8 {
            self.patrol_t[team as usize] -= dt;
            if self.patrol_t[team as usize] <= 0.0 {
                self.patrol_t[team as usize] = PATROL_INTERVAL;
                let wave = self.wave_n[team as usize] as usize;
                self.wave_n[team as usize] += 1;
                // mid every other wave — nobody's ancestors hold it
                let rota = [MID_LANE, DUO_LANE, MID_LANE, SOLO_LANE];
                let normal = rota[wave % rota.len()];
                // A breach pulls patrols, but not *every* patrol. Measured: the
                // clan that broke the enemy's first border won only 34% of its
                // matches, and the probe found why — deaths were even and no
                // deeper on either side, but the breacher ended with fewer of its
                // OWN borders standing. It had funnelled every wave into the hole
                // it made while the defender's waves still covered three lanes,
                // so it traded the map for a lane. Half the waves press the
                // breach now; the rest walk the rota.
                let lane = match self.breach_lane(team) {
                    Some(b) if wave % 2 == 0 => b,
                    _ => normal,
                };
                let duty = if wave % 3 == 2 { Duty::Hunting } else { Duty::Border };
                self.spawn_patrol(team, lane, duty);
            }
        }

        // contention ground: held by whoever brought more living warriors
        let (cx, cz) = self.contention;
        let count = |team: u8| self.warriors.iter()
            .filter(|w| w.team == team && w.alive() && dist(w.x, w.z, cx, cz) < CONTEND_R).count();
        let (a, b) = (count(CLAN_A), count(CLAN_B));
        self.contention_holder =
            if a > b { Some(CLAN_A) } else if b > a { Some(CLAN_B) } else { self.contention_holder };
        let (sx, sz) = self.skyrock;
        let scount = |team: u8| self.warriors.iter()
            .filter(|w| w.team == team && w.alive() && dist(w.x, w.z, sx, sz) < 14.0).count();
        let (sa, sb) = (scount(CLAN_A), scount(CLAN_B));
        self.skyrock_holder =
            if sa > sb { Some(CLAN_A) } else if sb > sa { Some(CLAN_B) } else { self.skyrock_holder };

        // player input drives each clan's leader — one side in a solo game, both
        // in a lockstep match
        for side in [CLAN_A, CLAN_B] {
            let Some((mx, mz, claw, ability)) = self.input[side as usize].take() else { continue };
            let Some(i) = self.driven(side) else { continue };
            if !self.warriors[i].alive() {
                continue;
            }
            if (mx != 0.0 || mz != 0.0) && self.warriors[i].root_t <= 0.0 {
                let (nx, nz) = norm(mx, mz);
                let sp = self.speed_of(i);
                let w = &mut self.warriors[i];
                w.x = (w.x + nx * sp * dt).clamp(-ARENA, ARENA);
                w.z = (w.z + nz * sp * dt).clamp(-ARENA, ARENA);
                w.heading = nz.atan2(nx);
            }
            if let Some(slot) = ability {
                self.cast(i, slot.min(2));
            }
            if claw {
                if let Some(t) = self.nearest_enemy(i, self.warriors[i].reach) {
                    // the same blow an AI ancestor throws — target-dependent
                    // riders, on-hit effects, and the kill credited. A player's
                    // claw that quietly skipped Hush of Snows or The Saber
                    // Finishes would make the kits play differently in a human's
                    // paws than in the sim they were balanced in.
                    let dmg = self.strike_of(i) * self.strike_vs(i, t) * dt;
                    let alive_before = self.warriors[t].alive();
                    self.hurt(t, dmg);
                    self.on_hit(i, t);
                    if alive_before && !self.warriors[t].alive() {
                        self.warriors[i].kills += 1;
                    }
                }
            }
        }

        // The Seated Mountain: note where every cat stood before this tick's minds run
        let seats: Vec<(f32, f32)> = self.warriors.iter().map(|w| (w.x, w.z)).collect();

        // Minds. The ORDER alternates every tick, and that is not a detail: a
        // warrior who thinks second sees the positions the first one just moved
        // to, so a fixed order hands one clan a permanent half-tick of fresher
        // information. Clan A's ancestors are indices 0..2 and clan B's are
        // 3..5, so "always ascending" meant clan B always reacted to clan A.
        // Measured over 140 games, that alone was worth about thirteen points of
        // win rate. Alternating cancels it without introducing randomness, which
        // a deterministic sim cannot afford.
        // (The player's warrior is skipped — the player is its mind.)
        let n = self.warriors.len();
        let ascending = self.tick % 2 == 0;
        for k in 0..n {
            let i = if ascending { k } else { n - 1 - k };
            let driven = self.warriors[i].is_player
                && self.player_driven[self.warriors[i].team as usize];
            if !self.warriors[i].alive() || driven { continue; }
            self.think(i, dt);
        }

        // stillness gathers for those who kept their seat this tick
        for (i, &(px, pz)) in seats.iter().enumerate() {
            let w = &mut self.warriors[i];
            if !w.alive() { w.still_t = 0.0; continue; }
            if dist(px, pz, w.x, w.z) < 0.05 { w.still_t += dt; } else { w.still_t = 0.0; }
        }

        // scorched ground burns intruders and sears every veil away;
        // sweet rain mends the packmates — and the wood — beneath it
        for zi in 0..self.zones.len() {
            let (zx, zz, zteam, zr, zdps, mend) = {
                let z = &self.zones[zi]; (z.x, z.z, z.team, z.r, z.dps, z.mend)
            };
            if mend {
                let allies: Vec<usize> = self.warriors.iter().enumerate()
                    .filter(|(_, w)| w.team == zteam && w.alive() && dist(w.x, w.z, zx, zz) < zr)
                    .map(|(j, _)| j).collect();
                for a in allies { self.heal(a, zdps * dt); }
                for t in self.heal_trees.iter_mut() {
                    if t.team == zteam && t.alive() && dist(t.x, t.z, zx, zz) < zr {
                        t.hp = (t.hp + zdps * dt * 2.0).min(HEAL_TREE_HP);   // wood drinks deep
                    }
                }
                let (cx, cz) = self.camps[zteam as usize];
                if dist(cx, cz, zx, zz) < zr {
                    let g = &mut self.clans[zteam as usize].great_tree_hp;
                    if *g > 0.0 { *g = (*g + zdps * dt * 2.0).min(GREAT_TREE_HP); }
                }
            } else {
                let victims: Vec<usize> = self.warriors.iter().enumerate()
                    .filter(|(_, w)| w.team != zteam && w.alive() && dist(w.x, w.z, zx, zz) < zr)
                    .map(|(j, _)| j).collect();
                for v in victims {
                    self.warriors[v].stealth_t = 0.0;   // nothing hides on burning sand
                    self.hurt(v, zdps * dt);
                }
            }
        }
        for z in self.zones.iter_mut() { z.t -= dt; }
        self.zones.retain(|z| z.t > 0.0);

        // Heal Tree auras mend every ally sheltering under them (thinner in leaf-bare);
        // the Riverborn mends wherever there is water
        let aura_heal = AURA_HEAL * if self.leafbare() { LEAFBARE_AURA } else { 1.0 };
        for i in 0..self.warriors.len() {
            let (team, x, z, alive, anc) = { let w = &self.warriors[i]; (w.team, w.x, w.z, w.alive(), w.ancestor) };
            if !alive { continue; }
            if self.under_aura(team, x, z) { self.heal(i, aura_heal * dt); }
            if anc == Some(Ancestor::Leopard) && world::height_at(x, z) < world::WATER_Y + 0.2 {
                // Riverborn mends *between* fights, not during one. Standing in
                // the shallows regenerating with an enemy on the bank was a heal
                // with no cost, no cooldown and no cap — the one kit that could
                // simply outlast anything that came to the water.
                let pressed = self.warriors.iter().any(|e| {
                    e.team != team && e.alive() && dist(e.x, e.z, x, z) < ENGAGE_R
                });
                if !pressed {
                    self.heal(i, 3.0 * dt);
                }
            }
        }

        // respawns (ancestors return; fallen patrol cats are mourned, not replaced)
        for i in 0..self.warriors.len() {
            let w = &self.warriors[i];
            if w.mind == Mind::Dead && w.role == Role::Ancestor {
                let t = (self.warriors[i].respawn_t - dt).max(0.0);
                self.warriors[i].respawn_t = t;
                if t <= 0.0 {
                    let team = self.warriors[i].team;
                    let (cx, cz) = self.camps[team as usize];
                    let lane = self.breach_lane(team).unwrap_or(self.warriors[i].lane);
                    let w = &mut self.warriors[i];
                    w.x = cx; w.z = cz; w.lane = lane;
                    w.hp = w.max_hp; w.mind = Mind::Trail; w.seg = 0;
                }
            }
        }
        self.warriors.retain(|w| w.alive() || w.role == Role::Ancestor);

        // outcome
        if self.clans[CLAN_B as usize].great_tree_hp <= 0.0 { self.outcome = Some(CLAN_A); }
        else if self.clans[CLAN_A as usize].great_tree_hp <= 0.0 { self.outcome = Some(CLAN_B); }
        else if self.time >= MATCH_LEN {
            // full time: the clan whose wood still stands strongest holds the ground
            let strength = |t: usize| self.clans[t].great_tree_hp
                + self.heal_trees.iter().filter(|h| h.team as usize == t).map(|h| h.hp.max(0.0)).sum::<f32>();
            let (sa, sb) = (strength(0), strength(1));
            self.outcome = Some(if sa > sb { CLAN_A } else if sb > sa { CLAN_B } else { 2 });
        }
    }

    /// A cheap digest of the whole match state, for determinism tests.
    pub fn digest(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        let mut mix = |v: u32| { h ^= v as u64; h = h.wrapping_mul(0x100000001b3); };
        for w in &self.warriors {
            mix(w.x.to_bits()); mix(w.z.to_bits()); mix(w.hp.to_bits());
            mix(w.team as u32 ^ ((w.mind == Mind::Dead) as u32) << 8);
            mix(w.root_t.to_bits() ^ w.slow_t.to_bits().rotate_left(16));
        }
        for t in &self.heal_trees { mix(t.hp.to_bits()); }
        for z in &self.zones { mix(z.x.to_bits()); mix(z.t.to_bits()); }
        for c in &self.clans { mix(c.pile.to_bits()); mix(c.herbs); mix(c.great_tree_hp.to_bits()); }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet(bg: &mut Battleground) {
        // strip the opposing clan so a mechanism can be tested without interference
        bg.warriors.retain(|w| w.team == CLAN_A);
        bg.patrol_t = [1e9; 2];
    }

    #[test]
    fn map_is_large_with_three_trails_and_six_heal_trees() {
        let bg = Battleground::new(Ancestor::Lion);
        assert!(dist(bg.camps[0].0, bg.camps[0].1, bg.camps[1].0, bg.camps[1].1) > 200.0,
            "camps should sit a long march apart");
        assert_eq!(bg.heal_trees.len(), 2 * LANES);
        for team in 0..2u8 {
            for lane in 0..LANES {
                assert!(bg.heal_trees.iter().any(|t| t.team == team && t.lane == lane));
            }
        }
    }

    #[test]
    fn a_patrol_retreats_when_wounded_and_heals_at_the_medic() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        bg.spawn_patrol(CLAN_A, 1, Duty::Border);
        let p = bg.warriors.iter().position(|w| w.role == Role::Patrol).unwrap();
        // wound it past its nerve, away from home
        bg.warriors[p].x = 0.0; bg.warriors[p].z = 0.0;
        bg.warriors[p].hp = bg.warriors[p].max_hp * 0.3;
        for _ in 0..30 * 60 {
            bg.step(1.0 / 30.0);
            if !bg.warriors[p].alive() { panic!("patrol died while retreating unopposed"); }
            if bg.warriors[p].mind == Mind::Trail && bg.warriors[p].power_frac() >= REJOIN { break; }
        }
        // it broke off, reached the medic, healed up, and went back on the trail
        assert_eq!(bg.warriors[p].mind, Mind::Trail);
        assert!(bg.warriors[p].power_frac() >= REJOIN,
            "patrol should rejoin healed, got {}", bg.warriors[p].power_frac());
    }

    #[test]
    fn defending_the_great_tree_is_the_exception_to_retreat() {
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        bg.warriors.retain(|w| w.team == CLAN_A);
        bg.spawn_patrol(CLAN_A, 1, Duty::Border);
        let p = bg.warriors.iter().position(|w| w.role == Role::Patrol).unwrap();
        let (cx, cz) = bg.camps[CLAN_A as usize];
        bg.warriors[p].x = cx + 2.0; bg.warriors[p].z = cz;
        bg.warriors[p].hp = bg.warriors[p].max_hp * 0.2;   // far past its nerve
        // an enemy at the camp's edge
        bg.warriors.push(Warrior {
            team: CLAN_B, role: Role::Patrol, ancestor: None, lane: 1,
            x: cx + 8.0, z: cz, heading: 0.0,
            hp: PATROL_HP, max_hp: PATROL_HP, attack: 0.0, speed: 0.0, reach: REACH,
            mind: Mind::Trail, carry: Carry::Nothing,
            shield: 0.0, shield_t: 0.0, stealth_t: 0.0, fear_t: 0.0, slow_t: 0.0, root_t: 0.0, pulled_t: 0.0,
            cds: [0.0; 3], respawn_t: 0.0, gather_t: 0.0, kills: 0, prey_taken: 0, seat: 999, still_t: 0.0, duty: Duty::Border, is_player: false, seg: 0,
        });
        bg.step(1.0 / 30.0);
        assert_eq!(bg.warriors[p].mind, Mind::LastStand, "at the Great Tree a warrior stands");
    }

    #[test]
    fn freshkill_carried_home_feeds_the_clan_and_growth_saturates() {
        let mut bg = Battleground::new(Ancestor::Lion);
        assert_eq!(bg.growth(CLAN_A), 1.0);
        bg.clans[CLAN_A as usize].pile = GROWTH_HALF;
        let g1 = bg.growth(CLAN_A);
        assert!((g1 - (1.0 + GROWTH_CAP * 0.5)).abs() < 1e-4, "half the cap at the half point");
        bg.clans[CLAN_A as usize].pile = 1e6;
        assert!(bg.growth(CLAN_A) < 1.0 + GROWTH_CAP + 1e-4, "growth must saturate");
        // and the pile fills by *carrying home*, not by the kill itself
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        let (hx, hz) = bg.camps[CLAN_A as usize];
        bg.warriors[1].carry = Carry::Freshkill(RICH_WORTH);
        bg.warriors[1].mind = Mind::CarryHome;
        bg.warriors[1].x = hx + 30.0; bg.warriors[1].z = hz;
        for _ in 0..30 * 30 { bg.step(1.0 / 30.0); if bg.clans[0].pile > 0.0 { break; } }
        assert_eq!(bg.clans[CLAN_A as usize].pile, RICH_WORTH);
        assert_eq!(bg.warriors[1].carry, Carry::Nothing);
    }

    #[test]
    fn a_kill_carried_is_a_kill_lost_on_death() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        bg.warriors[1].carry = Carry::Freshkill(1.0);
        let idx = 1;
        bg.hurt(idx, 1e6);
        assert!(!bg.warriors[idx].alive());
        assert_eq!(bg.warriors[idx].carry, Carry::Nothing, "the carcass drops with the carrier");
        assert_eq!(bg.clans[CLAN_A as usize].pile, 0.0);
    }

    #[test]
    fn heal_tree_aura_protects_and_mends() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        let t = bg.heal_trees.iter().position(|t| t.team == CLAN_A).unwrap();
        let (tx, tz) = (bg.heal_trees[t].x, bg.heal_trees[t].z);
        // under the tree, a blow lands softer
        bg.warriors[1].x = tx; bg.warriors[1].z = tz;
        bg.warriors[2].x = tx + 200.0_f32.min(ARENA - tx - 1.0).max(60.0); bg.warriors[2].z = tz;
        let (h1, h2) = (bg.warriors[1].hp, bg.warriors[2].hp);
        bg.hurt(1, 50.0);
        bg.hurt(2, 50.0);
        let (d1, d2) = (h1 - bg.warriors[1].hp, h2 - bg.warriors[2].hp);
        assert!(d1 < d2, "aura should soften blows: {d1} vs {d2}");
        assert!((d1 - 50.0 * (1.0 - AURA_GUARD)).abs() < 1e-3);
        // and it mends over time
        let before = bg.warriors[1].hp;
        for _ in 0..30 { bg.step(1.0 / 30.0); }
        assert!(bg.warriors[1].hp > before, "the Tree of Healing should mend its shelterers");
    }

    #[test]
    fn the_great_tree_is_hard_while_its_heal_trees_stand() {
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        bg.warriors.retain(|w| w.team == CLAN_B);   // only clan B attackers
        let (ax, az) = bg.camps[CLAN_A as usize];
        for w in bg.warriors.iter_mut() { w.x = ax + 2.0; w.z = az; w.mind = Mind::Trail; }
        let full = bg.clans[CLAN_A as usize].great_tree_hp;
        for _ in 0..30 * 5 { bg.step(1.0 / 30.0); }
        let guarded = full - bg.clans[CLAN_A as usize].great_tree_hp;
        // now fell every Heal Tree and gnaw again for the same span
        for t in bg.heal_trees.iter_mut().filter(|t| t.team == CLAN_A) { t.hp = 0.0; }
        for w in bg.warriors.iter_mut() { w.x = ax + 2.0; w.z = az; w.hp = w.max_hp; w.mind = Mind::Trail; }
        let mid = bg.clans[CLAN_A as usize].great_tree_hp;
        for _ in 0..30 * 5 { bg.step(1.0 / 30.0); }
        let bare = mid - bg.clans[CLAN_A as usize].great_tree_hp;
        assert!(bare > guarded * 1.5, "roots should harden the Great Tree: {guarded} vs {bare}");
    }

    #[test]
    fn herbs_delivered_quicken_the_clans_healing_and_returns() {
        let mut bg = Battleground::new(Ancestor::Lion);
        let slow = bg.respawn_time(CLAN_A);
        let m0 = bg.herb_heal(CLAN_A);
        bg.clans[CLAN_A as usize].herbs = 3;
        assert!(bg.herb_heal(CLAN_A) > m0);
        assert!(bg.respawn_time(CLAN_A) < slow);
        assert!(bg.herb_heal(CLAN_A) <= HERB_HEAL_MAX + 1e-6);
        // gathering: crouch at a ready patch, then carry the sprig home
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        let (px, pz) = (bg.herb_patches[0].x, bg.herb_patches[0].z);
        bg.warriors[1].x = px; bg.warriors[1].z = pz;
        bg.warriors[1].mind = Mind::Gather;
        for _ in 0..30 * 10 { bg.step(1.0 / 30.0); if bg.warriors[1].carry == Carry::Herb { break; } }
        assert_eq!(bg.warriors[1].carry, Carry::Herb, "crouching long enough picks the sprig");
        assert!(!bg.herb_patches[0].ready(), "a picked patch regrows on a timer");
        assert_eq!(bg.clans[CLAN_A as usize].herbs, 0, "picking alone delivers nothing");
        for _ in 0..30 * 60 { bg.step(1.0 / 30.0); if bg.clans[0].herbs > 0 { break; } }
        assert_eq!(bg.clans[CLAN_A as usize].herbs, 1, "the sprig must reach the medic cat");
    }

    #[test]
    fn contention_ground_follows_presence() {
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        for w in bg.warriors.iter_mut() { w.x = -140.0; w.z = -140.0; w.mind = Mind::Retreat; }
        // clan B masses at the centre
        let n = bg.warriors.len();
        for i in 0..n {
            if bg.warriors[i].team == CLAN_B { bg.warriors[i].x = 1.0; bg.warriors[i].z = 1.0; bg.warriors[i].mind = Mind::Retreat; }
        }
        bg.step(1.0 / 30.0);
        assert_eq!(bg.contention_holder, Some(CLAN_B));
    }

    #[test]
    fn felling_the_great_tree_ends_the_match() {
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.clans[CLAN_B as usize].great_tree_hp = 0.5;
        bg.clans[CLAN_B as usize].great_tree_hp -= 1.0;
        bg.step(1.0 / 30.0);
        assert_eq!(bg.outcome, Some(CLAN_A));
        let d = bg.digest();
        bg.step(1.0 / 30.0);
        assert_eq!(bg.digest(), d, "a finished match is frozen");
    }

    #[test]
    fn ability_verbs_do_what_they_say() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        // a shielded lion shrugs off exactly the absorbed amount
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        bg.warriors[1].x = x; bg.warriors[1].z = z;   // packmate in shield range
        bg.cast(0, 1);   // Mane of Dawn
        assert!(bg.warriors[0].shield > 0.0 && bg.warriors[1].shield > 0.0);
        let hp = bg.warriors[0].hp;
        let small = bg.warriors[0].shield * 0.5;
        // move off any aura so the arithmetic is exact
        bg.heal_trees.clear();
        bg.hurt(0, small);
        assert_eq!(bg.warriors[0].hp, hp, "the shield eats the whole small blow");
        // stealth hides a tiger from targeting
        let mut bg = Battleground::new(Ancestor::Tiger);
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].stealth_t = 5.0;
        let seen = bg.warriors.iter().enumerate()
            .filter(|(_, w)| w.team == CLAN_B).map(|(i, _)| i).next().unwrap();
        bg.warriors[seen].x = bg.warriors[0].x; bg.warriors[seen].z = bg.warriors[0].z;
        assert!(bg.nearest_enemy(seen, 50.0).map(|t| bg.warriors[t].stealth_t <= 0.0).unwrap_or(true),
            "a veiled cat cannot be picked as a target");
    }

    #[test]
    fn hunting_patrols_feed_the_pile_without_orders() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        bg.warriors.clear();   // only the patrol works
        bg.spawn_patrol(CLAN_A, 0, Duty::Hunting);
        for _ in 0..30 * 180 {
            bg.step(1.0 / 30.0);
            if bg.clans[CLAN_A as usize].pile > 0.0 { break; }
        }
        assert!(bg.clans[CLAN_A as usize].pile > 0.0,
            "a hunting patrol should kill, carry home, and deposit on its own");
        assert!(bg.warriors.iter().any(|w| w.duty == Duty::Hunting && matches!(w.mind, Mind::Hunt | Mind::CarryHome)),
            "after depositing, a hunting patrol goes back out");
    }

    #[test]
    fn prey_flees_the_hunter_but_never_abandons_its_ground() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        let p = bg.prey.iter().position(|p| p.kind.flees()).unwrap();
        let (hx, hz) = bg.prey[p].home;
        bg.warriors[1].x = hx; bg.warriors[1].z = hz;
        bg.warriors[1].mind = Mind::Retreat;   // stand there menacingly, don't hunt
        bg.warriors[1].hp = bg.warriors[1].max_hp;
        for w in bg.warriors.iter_mut() { if !w.is_player { w.mind = Mind::Retreat; } }
        bg.warriors[1].mind = Mind::AtMedic;   // hold position (medic check fails far away → stays)
        bg.warriors[1].x = hx + 1.0; bg.warriors[1].z = hz;
        let d0 = dist(bg.prey[p].x, bg.prey[p].z, hx, hz);
        for _ in 0..30 * 10 { bg.warriors[1].x = hx + 1.0; bg.warriors[1].z = hz; bg.step(1.0 / 30.0); }
        let d1 = dist(bg.prey[p].x, bg.prey[p].z, hx, hz);
        assert!(d1 > d0 + 2.0, "prey should bolt from a lurking cat: {d0} -> {d1}");
        assert!(d1 <= 12.0 + 1e-3, "but the leash holds it to its ground");
        assert!(!bg.prey.iter().any(|q| q.kind == PreyKind::Fish && q.kind.flees()),
            "fish hold to the water instead of bolting overland");
    }

    #[test]
    fn night_belongs_to_the_ambush_ancestors() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Tiger, Ancestor::Lion, Ancestor::Leopard],
            [Ancestor::Puma, Ancestor::Caracal, Ancestor::Panther]);
        quiet(&mut bg);
        assert!(!bg.is_night());
        let day = bg.strike_of(0);
        bg.time = DAY_LEN + 1.0;   // the first night watch
        assert!(bg.is_night());
        let night = bg.strike_of(0);
        assert!(night > day * 1.2, "Night's Hunger: {day} vs {night}");
        // and the veil counts as night even at noon
        bg.time = 0.0;
        bg.warriors[0].stealth_t = 2.0;
        assert!(bg.strike_of(0) > day * 1.2);
    }

    #[test]
    fn prides_warmth_strengthens_packmates_near_goldenmane() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Lion, Ancestor::Puma, Ancestor::Caracal],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        quiet(&mut bg);
        let (lx, lz) = (bg.warriors[0].x, bg.warriors[0].z);
        bg.warriors[1].x = lx + 100.0; bg.warriors[1].z = lz;
        let far = bg.strike_of(1);
        bg.warriors[1].x = lx + 2.0; bg.warriors[1].z = lz;
        let near = bg.strike_of(1);
        assert!(near > far * 1.1, "beside the lion, allies strike harder: {far} vs {near}");
    }

    #[test]
    fn leafbare_thins_the_auras_and_swells_the_patrols() {
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        assert!(!bg.leafbare());
        bg.time = LEAFBARE_AT + 1.0;
        assert!(bg.leafbare());
        let before = bg.warriors.len();
        bg.spawn_patrol(CLAN_A, 0, Duty::Border);
        assert_eq!(bg.warriors.len() - before, LEAFBARE_PATROL,
            "leaf-bare border patrols march {LEAFBARE_PATROL} strong");
        // aura mending is thinner in leaf-bare
        let t = bg.heal_trees.iter().position(|t| t.team == CLAN_A).unwrap();
        let (tx, tz) = (bg.heal_trees[t].x, bg.heal_trees[t].z);
        bg.warriors[1].x = tx; bg.warriors[1].z = tz;
        bg.warriors[1].mind = Mind::AtMedic;   // hold still (far from the medic → stays put)
        bg.warriors[1].hp = 10.0;
        bg.step(1.0);
        let gained_bare = bg.warriors[1].hp - 10.0 - MEDIC_HEAL * 0.0;
        bg.time = 0.0;
        bg.warriors[1].hp = 10.0;
        bg.step(1.0);
        let gained_green = bg.warriors[1].hp - 10.0;
        assert!(gained_bare < gained_green, "leaf-bare aura {gained_bare} vs greenleaf {gained_green}");
    }

    #[test]
    fn a_draft_fields_exactly_the_picked_ancestors() {
        let a = [Ancestor::Caracal, Ancestor::Caracal, Ancestor::Puma];
        let b = [Ancestor::Lion, Ancestor::Tiger, Ancestor::Leopard];
        let bg = Battleground::new_draft(a, b);
        let picks = |team: u8| -> Vec<Ancestor> {
            bg.warriors.iter().filter(|w| w.team == team && w.role == Role::Ancestor)
                .map(|w| w.ancestor.unwrap()).collect()
        };
        assert_eq!(picks(CLAN_A), a.to_vec());
        assert_eq!(picks(CLAN_B), b.to_vec());
        assert!(bg.warriors[0].is_player);
    }

    #[test]
    fn a_yowler_strikes_from_across_the_clearing() {
        use crate::ancestor::kit;
        let mut bg = Battleground::new_draft(
            [Ancestor::Serval, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        bg.patrol_t = [1e9; 2];
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        let d = kit(Ancestor::Serval).reach - 1.0;   // in yowl range, far past claw range
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = x + d; bg.warriors[e].z = z;
        bg.warriors[e].mind = Mind::AtMedic;   // hold still
        let hp0 = bg.warriors[e].hp;
        bg.step(1.0 / 30.0);
        assert!(bg.warriors[e].hp < hp0, "the yowl should land without closing");
        assert!(dist(bg.warriors[0].x, bg.warriors[0].z, x, z) < 0.5,
            "a ranged cat in range should not have charged in");
        // and the Far Song bites harder from a distance than muzzle-to-muzzle
        assert!(bg.strike_vs(0, e) > 1.0);
        bg.warriors[e].x = x + 1.0;
        assert!((bg.strike_vs(0, e) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn no_cat_ever_yowled_down_a_tree() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Serval, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        quiet(&mut bg);
        // park the yowler in yowl range of an enemy Heal Tree — but beyond the claw
        bg.warriors.retain(|w| w.role == Role::Ancestor);
        let t = bg.heal_trees.iter().position(|t| t.team == CLAN_B).unwrap();
        let (tx, tz) = (bg.heal_trees[t].x, bg.heal_trees[t].z);
        let lane = bg.heal_trees[t].lane;
        bg.warriors[0].x = tx + 8.0; bg.warriors[0].z = tz;
        bg.warriors[0].lane = lane;
        let hp0 = bg.heal_trees[t].hp;
        for _ in 0..15 {
            bg.warriors[0].x = tx + 8.0; bg.warriors[0].z = tz;   // hold it at range
            bg.warriors[0].mind = Mind::Trail;
            bg.step(1.0 / 30.0);
        }
        assert_eq!(bg.heal_trees[t].hp, hp0, "structures must be gnawed up close, never yowled");
    }

    #[test]
    fn boulder_paw_flings_the_close_pressed_away() {
        let mut bg = Battleground::new_draft(
            [Ancestor::CaveLion, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = x + 1.5; bg.warriors[e].z = z;
        let d0 = dist(bg.warriors[e].x, bg.warriors[e].z, x, z);
        let hp0 = bg.warriors[e].hp;
        bg.cast(0, 0);   // Boulder Paw
        let d1 = dist(bg.warriors[e].x, bg.warriors[e].z, x, z);
        assert!(d1 > d0 + 3.0, "the struck are hurled away: {d0} -> {d1}");
        assert!(bg.warriors[e].hp < hp0);
    }

    #[test]
    fn deep_wounds_deep_fury_and_the_saber_finishing() {
        let mut bg = Battleground::new_draft(
            [Ancestor::CaveLion, Ancestor::Smilodon, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        quiet(&mut bg);
        // Oldroar hits harder the closer it is to falling
        let whole = bg.strike_of(0);
        bg.warriors[0].hp = bg.warriors[0].max_hp * 0.2;
        let bleeding = bg.strike_of(0);
        assert!(bleeding > whole * 1.3, "fury should rise with the wounds: {whole} vs {bleeding}");
        // Twofang bites deeper below half strength — and the Killing Bite deeper still
        let mut bg = Battleground::new_draft(
            [Ancestor::Smilodon, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = x + 2.0; bg.warriors[e].z = z;
        assert!((bg.strike_vs(0, e) - 1.0).abs() < 1e-6, "no bonus against the hale");
        bg.warriors[e].hp = bg.warriors[e].max_hp * 0.3;
        assert!(bg.strike_vs(0, e) > 1.3, "the saber finishes the wounded");
        let hp_hurt = bg.warriors[e].hp;
        bg.warriors[e].shield = 0.0;
        bg.cast(0, 2);   // The Killing Bite
        let bite_wounded = hp_hurt - bg.warriors[e].hp.max(0.0);
        assert!(bite_wounded > 0.0);
        // against a hale target the same bite is much smaller
        bg.warriors[e].hp = bg.warriors[e].max_hp;
        bg.warriors[0].cds[2] = 0.0;
        bg.cast(0, 2);
        let bite_hale = bg.warriors[e].max_hp - bg.warriors[e].hp;
        assert!(bite_wounded > bite_hale * 1.5,
            "the execute must scale with missing strength: hale {bite_hale} vs wounded {bite_wounded}");
    }

    #[test]
    fn nothing_hides_on_burning_sand() {
        let mut bg = Battleground::new_draft(
            [Ancestor::SunCat, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Panther, Ancestor::Tiger, Ancestor::Leopard]);
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = x + 2.0; bg.warriors[e].z = z;
        bg.warriors[e].stealth_t = 10.0;   // Nightpelt under the veil
        bg.warriors[e].mind = Mind::AtMedic;
        bg.cast(0, 1);   // Noon Blaze
        assert!(!bg.zones.is_empty(), "the ground should be scorched");
        let hp0 = bg.warriors[e].hp;
        bg.step(1.0 / 30.0);
        assert_eq!(bg.warriors[e].stealth_t, 0.0, "the veil must not survive the light");
        assert!(bg.warriors[e].hp < hp0, "burning sand burns");
        // the scorch expires
        for _ in 0..30 * 10 { bg.step(1.0 / 30.0); }
        assert!(bg.zones.is_empty(), "scorched ground cools");
    }

    #[test]
    fn the_sun_and_the_night_mirror_each_other() {
        let mut bg = Battleground::new_draft(
            [Ancestor::SunCat, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther]);
        quiet(&mut bg);
        let day = bg.strike_of(0);
        bg.time = DAY_LEN + 1.0;   // night falls
        let night = bg.strike_of(0);
        assert!(day > night * 1.2, "the day belongs to Suneye: {night} vs {day}");
    }

    #[test]
    fn snowbind_holds_them_fast_but_not_forever() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Lynx, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Caracal, Ancestor::Leopard, Ancestor::Panther]);
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = x + 4.0; bg.warriors[e].z = z;
        bg.cast(0, 0);   // Snowbind
        assert!(bg.warriors[e].root_t > 0.0);
        // held fast: the fastest ancestor alive cannot move a whisker
        let (ex, ez) = (bg.warriors[e].x, bg.warriors[e].z);
        bg.warriors[e].mind = Mind::Retreat;   // it *wants* to run home
        bg.step(1.0 / 30.0);
        assert!(dist(bg.warriors[e].x, bg.warriors[e].z, ex, ez) < 1e-4,
            "a rooted cat cannot move");
        // but hard CC always expires
        for _ in 0..30 * 3 { bg.step(1.0 / 30.0); }
        assert_eq!(bg.warriors[e].root_t, 0.0);
        // and Hush of Snows: an ordinary blow from the lynx chills
        bg.warriors[e].x = x + 1.0; bg.warriors[e].z = z;
        bg.warriors[e].slow_t = 0.0;
        bg.on_hit(0, e);
        assert!(bg.warriors[e].slow_t > 0.0, "every blow carries the cold");
    }

    #[test]
    fn undertow_gathers_the_scattered_fight() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Jaguar, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Caracal]);
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        // scatter the enemy clan around the jaguar at pull range
        let enemies: Vec<usize> = bg.warriors.iter().enumerate()
            .filter(|(_, w)| w.team == CLAN_B).map(|(i, _)| i).collect();
        for (k, &e) in enemies.iter().enumerate() {
            let a = k as f32 * 2.0;
            bg.warriors[e].x = x + a.cos() * 8.0;
            bg.warriors[e].z = z + a.sin() * 8.0;
        }
        let before: f32 = enemies.iter().map(|&e| dist(bg.warriors[e].x, bg.warriors[e].z, x, z)).sum();
        bg.cast(0, 0);   // Undertow
        let after: f32 = enemies.iter().map(|&e| dist(bg.warriors[e].x, bg.warriors[e].z, x, z)).sum();
        assert!(after < before * 0.5, "the current gathers them in: {before} -> {after}");
        for &e in &enemies {
            assert!(dist(bg.warriors[e].x, bg.warriors[e].z, x, z) > 1.0,
                "dragged to the claw, not inside the caster");
        }
        // The River Remembers: a held target is bitten deeper
        let e = enemies[0];
        assert!((bg.strike_vs(0, e) - 1.0).abs() < 1e-6);
        bg.warriors[e].root_t = 1.0;
        assert!(bg.strike_vs(0, e) > 1.1, "the held are already caught");
    }

    #[test]
    fn the_jungle_hides_a_waiting_warrior() {
        use crate::junglemap;
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        let th = junglemap::thickets()[1];   // clean eastern sightline (no wall)
        // an enemy lurks in the brush; a clan A warrior stands in the open nearby
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = th.x; bg.warriors[e].z = th.z;
        bg.warriors[0].x = th.x + 20.0; bg.warriors[0].z = th.z;
        assert!(bg.nearest_enemy(0, 50.0).is_none(),
            "from the open, the brush shows nothing");
        // step to the thicket's edge and the rustling gives them away
        bg.warriors[0].x = th.x + junglemap::CLOSE_SENSE - 1.0;
        assert_eq!(bg.nearest_enemy(0, 50.0), Some(e));
        // and the hidden cat has seen the open-ground warrior the whole time
        bg.warriors[0].x = th.x + 20.0;
        assert_eq!(bg.nearest_enemy(e, 50.0), Some(0), "ambush sight is one-way");
        // the ambush kits treat the brush as their darkness
        let mut bg2 = Battleground::new_draft(
            [Ancestor::Tiger, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Leopard, Ancestor::Caracal, Ancestor::Panther]);
        quiet(&mut bg2);
        // baseline away from the camp: at spawn the Tiger stands beside its Lion
        // packmate, and Pride's Warmth would pollute the comparison
        bg2.warriors[0].x = th.x + 40.0; bg2.warriors[0].z = th.z;
        let open = bg2.strike_of(0);
        bg2.warriors[0].x = th.x; bg2.warriors[0].z = th.z;
        assert!(bg2.strike_of(0) > open * 1.2, "Night's Hunger feeds in the brush too");
    }

    #[test]
    fn stone_walls_cut_sight_and_slow_the_scramble() {
        use crate::junglemap;
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        let w = junglemap::walls()[0];
        let q = (w.ax + (w.bx - w.ax) * 0.2, w.az + (w.bz - w.az) * 0.2);
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[0].x = q.0 + 10.0; bg.warriors[0].z = q.1 - 10.0;
        bg.warriors[e].x = q.0 - 10.0; bg.warriors[e].z = q.1 + 10.0;
        assert!(bg.nearest_enemy(0, 60.0).is_none(), "sight does not cross stone");
        // through the pass, the same spacing sees fine
        let (mx, mz) = ((w.ax + w.bx) * 0.5, (w.az + w.bz) * 0.5);
        bg.warriors[0].x = mx + 10.0; bg.warriors[0].z = mz - 10.0;
        bg.warriors[e].x = mx - 10.0; bg.warriors[e].z = mz + 10.0;
        assert_eq!(bg.nearest_enemy(0, 60.0), Some(e));
        // and stone underfoot is a scramble
        bg.warriors[0].x = q.0; bg.warriors[0].z = q.1;
        let on = bg.speed_of(0);
        bg.warriors[0].x = mx; bg.warriors[0].z = mz;
        let pass = bg.speed_of(0);
        assert!(on < pass * 0.6, "scrambling over stone: {on} vs {pass} through the pass");
    }

    #[test]
    fn prey_does_not_fear_the_hunter_it_cannot_see() {
        use crate::junglemap;
        let mut bg = Battleground::new(Ancestor::Lion);
        quiet(&mut bg);
        // a prey camp with brush beside it — the layout placed it there on purpose
        let th = junglemap::thickets()[1];
        let p = bg.prey.iter().position(|p| p.kind.flees()
            && dist(p.home.0, p.home.1, th.x, th.z) < 20.0).expect("brush beside a prey camp");
        let (hx, hz) = bg.prey[p].home;
        // hunter crouched in the brush, well inside spook range of the prey — and
        // as a control, the same crouch in the open must send it bolting
        let (cx, cz) = (th.x, th.z - 2.5);
        assert!(dist(cx, cz, hx, hz) < 7.0, "the crouch spot must be in spook range");
        bg.warriors[1].x = cx; bg.warriors[1].z = cz;
        bg.warriors[1].mind = Mind::AtMedic;
        let d0 = dist(bg.prey[p].x, bg.prey[p].z, hx, hz);
        for _ in 0..30 * 5 { bg.warriors[1].x = cx; bg.warriors[1].z = cz; bg.step(1.0 / 30.0); }
        let d1 = dist(bg.prey[p].x, bg.prey[p].z, hx, hz);
        assert!(d1 < d0 + 1.0, "a hunter in the brush spooks nothing: {d0} -> {d1}");
    }

    #[test]
    fn the_seated_mountain_gathers_and_movement_spends_it() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Suanni, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Caracal]);
        quiet(&mut bg);
        // seat him away from packmates so Pride's Warmth can't pollute the baseline
        bg.warriors[0].x = 40.0; bg.warriors[0].z = -40.0;
        bg.warriors[0].mind = Mind::AtMedic;   // far from the medic → holds his seat
        let restless = bg.strike_of(0);
        for _ in 0..30 * 5 {
            bg.warriors[0].x = 40.0; bg.warriors[0].z = -40.0;
            bg.warriors[0].mind = Mind::AtMedic;
            bg.step(1.0 / 30.0);
        }
        assert!(bg.warriors[0].still_t > 4.0, "he kept his seat");
        let seated = bg.strike_of(0);
        assert!(seated > restless * 1.4, "stillness gathers the storm: {restless} vs {seated}");
        // one step and the gathering is spent
        bg.warriors[0].mind = Mind::Retreat;
        bg.step(1.0 / 30.0);
        assert_eq!(bg.warriors[0].still_t, 0.0, "movement spends the stillness");
    }

    #[test]
    fn sweetrain_mends_the_pack_and_the_wounded_wood() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Suanni, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Caracal]);
        bg.patrol_t = [1e9; 2];   // no patrols — but keep clan B: one of them is the control
        for w in bg.warriors.iter_mut() { w.x = -140.0; w.z = 140.0; w.mind = Mind::Retreat; }
        bg.warriors[0].is_player = false;
        // stand the ninth son at a half-gnawed Heal Tree with a wounded packmate
        let t = bg.heal_trees.iter().position(|t| t.team == CLAN_A).unwrap();
        let (tx, tz) = (bg.heal_trees[t].x, bg.heal_trees[t].z);
        bg.heal_trees[t].hp = HEAL_TREE_HP * 0.4;
        bg.warriors[0].x = tx; bg.warriors[0].z = tz;
        bg.warriors[1].x = tx + 2.0; bg.warriors[1].z = tz;
        bg.warriors[1].hp = 30.0;
        // an enemy in the same rain must NOT drink it
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = tx - 2.0; bg.warriors[e].z = tz;
        bg.warriors[e].hp = 30.0;
        bg.warriors[e].root_t = 10.0;   // held in place — AtMedic would self-heal and spoil the control
        bg.cast(0, 1);   // Sweetrain
        assert!(bg.zones.iter().any(|z| z.mend), "the rain should be falling");
        let (w1, wood, we) = (bg.warriors[1].hp, bg.heal_trees[t].hp, bg.warriors[e].hp);
        // hold everyone in the rain a moment (aura also mends allies — the enemy is the control)
        for _ in 0..30 {
            bg.warriors[1].x = tx + 2.0; bg.warriors[1].z = tz;
            bg.warriors[e].x = tx - 2.0; bg.warriors[e].z = tz; bg.warriors[e].root_t = 10.0;
            bg.step(1.0 / 30.0);
        }
        assert!(bg.warriors[1].hp > w1, "the packmate mends beneath the rain");
        assert!(bg.heal_trees[t].hp > wood, "and the wounded wood drinks");
        assert!(bg.warriors[e].hp <= we, "the enemy stands in the same rain and gains nothing");
        // the rain passes
        for _ in 0..30 * 10 { bg.step(1.0 / 30.0); }
        assert!(bg.zones.iter().all(|z| !z.mend), "sweet rain always passes");
        // and only the ninth son calls it: a scorch zone must not mend wood
        let mut bg2 = Battleground::new_draft(
            [Ancestor::SunCat, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Caracal]);
        quiet(&mut bg2);
        bg2.warriors[0].is_player = false;
        bg2.cast(0, 1);   // Noon Blaze
        assert!(bg2.zones.iter().all(|z| !z.mend));
    }

    #[test]
    fn the_river_bows_around_the_solo_lane_and_the_camp_line_stays_dry() {
        // hold the terrain lock for the whole test so no parallel bake swaps the
        // world out from under the sampling
        let _guard = world::TERRAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        world::set_terrain_pool(Battleground::MOUNTAIN_BIAS, 1);
        world::set_land_lift(Battleground::LAND_LIFT);
        world::set_river_course(world::RIVER_U_EAST);
        world::init_world();
        // the line between the two Great Trees: dry the whole march
        for k in 1..20 {
            let t = -100.0 + k as f32 * 10.0;
            assert!(world::height_at(t, t) > world::WATER_Y,
                "the camp-to-camp line must not be a river: wet at ({t:.0},{t:.0})");
        }
        // the U itself holds water on both arms and at the bottom of the bend
        let on_arc = |ang: f32| {
            (world::U_RIVER_C.0 + ang.cos() * world::U_RIVER_R,
             world::U_RIVER_C.1 + ang.sin() * world::U_RIVER_R)
        };
        let mut wet = 0;
        for k in 0..24 {
            let a = std::f32::consts::PI * (0.5 + k as f32 / 24.0);   // the map-side half of the circle
            let (x, z) = on_arc(a);
            if x.abs() > 148.0 || z.abs() > 148.0 { continue; }
            if world::U_FORDS.iter().any(|f| dist(x, z, f.0, f.1) < world::FORD_R + 3.0) { continue; }
            if world::height_at(x, z) < world::WATER_Y { wet += 1; }
        }
        assert!(wet >= 6, "the U-bend should hold real water along its course (wet {wet})");
        // and each ford is a paw-deep crossing: dry gravel where the trail meets it
        for &(fx, fz) in &world::U_FORDS {
            assert!(world::height_at(fx, fz) > world::WATER_Y,
                "the ford at ({fx:.0},{fz:.0}) must be crossable dry-pawed");
        }
        // restore the default spec for whoever bakes next
        world::set_terrain_pool(1.0, 1);
        world::init_world();
    }

    #[test]
    fn the_duo_lane_climbs_and_the_river_keeps_the_valley_floor() {
        let _guard = world::TERRAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        world::set_terrain_pool(Battleground::MOUNTAIN_BIAS, 1);
        world::set_land_lift(Battleground::LAND_LIFT);
        world::set_river_course(world::RIVER_U_EAST);
        world::init_world();
        let duo = world::height_at(-85.0, 85.0);
        let solo = world::height_at(85.0, -85.0);
        let mid = world::height_at(0.0, 0.0);
        assert!(duo > solo + 3.5, "the duo bend must stand on the mountain: duo {duo:.1} solo {solo:.1}");
        assert!(duo > mid + 2.0, "and above the causeway: duo {duo:.1} mid {mid:.1}");
        // the river bed is the lowest ground of the three heights of the land
        let (rx, rz) = (world::U_RIVER_C.0 - world::U_RIVER_R * 0.707, world::U_RIVER_C.1 + world::U_RIVER_R * 0.707);
        let river = world::height_at(rx, rz);
        assert!(river < world::WATER_Y && river < solo && river < mid,
            "the river keeps the bottom: river {river:.1}");
        world::set_terrain_pool(1.0, 1);
        world::init_world();
    }

    #[test]
    fn high_ground_spots_farther_and_looks_down_into_brush() {
        // the pure rules first
        assert_eq!(Battleground::downhill_bonus(0.0), 1.0);
        assert_eq!(Battleground::downhill_bonus(-5.0), 1.0, "uphill grants nothing");
        assert!(Battleground::downhill_bonus(4.0) > 1.2);
        assert!(Battleground::downhill_bonus(20.0) <= 1.4 + 1e-6, "the bonus caps");
        assert!(Battleground::pierces_cover(5.0, 20.0));
        assert!(!Battleground::pierces_cover(2.0, 20.0), "not high enough");
        assert!(!Battleground::pierces_cover(5.0, 40.0), "too far even from above");
    }

    #[test]
    fn the_skyrock_watchers_widen_the_whole_clans_eyes() {
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        // stand a scout on the Skyrock and everyone else far away
        for w in bg.warriors.iter_mut() { w.x = -140.0; w.z = -140.0; w.mind = Mind::Retreat; }
        let (sx, sz) = bg.skyrock;
        // a clan A viewer and a clan B target on the SAME open flat ground, spaced
        // just past plain engage range — invisible without the Skyrock's warning
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        let (vx, vz) = (30.0, 30.0);   // on the causeway flat, no cover, no wall
        let d = ENGAGE_R * 1.12;
        bg.warriors[0].x = vx; bg.warriors[0].z = vz;
        bg.warriors[e].x = vx + d; bg.warriors[e].z = vz;
        bg.skyrock_holder = None;
        let dh = world::height_at(vx, vz) - world::height_at(vx + d, vz);
        if dh.abs() < 2.9 {   // flat enough that no downhill bonus muddies the comparison
            assert!(bg.nearest_enemy(0, ENGAGE_R).is_none(), "past plain range, unseen");
        }
        // now clan A's scout holds the rock: the whole clan spots a quarter farther
        bg.warriors[1].x = sx; bg.warriors[1].z = sz; bg.warriors[1].mind = Mind::Retreat;
        bg.step(1.0 / 30.0);
        assert_eq!(bg.skyrock_holder, Some(CLAN_A));
        bg.warriors[0].x = vx; bg.warriors[0].z = vz;
        bg.warriors[e].x = vx + d; bg.warriors[e].z = vz;
        assert_eq!(bg.nearest_enemy(0, ENGAGE_R), Some(e), "the watchers yowl their warning");
        // and the rock is the reflection's fixed point: equally far from both camps
        let da = dist(sx, sz, bg.camps[0].0, bg.camps[0].1);
        let db = dist(sx, sz, bg.camps[1].0, bg.camps[1].1);
        assert!((da - db).abs() < 12.0, "the Skyrock favors neither clan: {da:.0} vs {db:.0}");
    }

    #[test]
    fn the_heavy_kits_bring_the_wood_down_faster() {
        // Their whole compensation for having no tempo — so it is asserted, not
        // assumed. Same tree, same seconds, a bruiser against a skirmisher.
        let gnaw = |a: Ancestor| {
            let mut bg = Battleground::new_draft(
                [a, Ancestor::Lion, Ancestor::Puma],
                [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther],
            );
            bg.patrol_t = [1e9; 2];
            bg.warriors.retain(|w| w.team == CLAN_A);
            let t = bg.heal_trees.iter().position(|t| t.team == CLAN_B).unwrap();
            let (tx, tz, lane) =
                (bg.heal_trees[t].x, bg.heal_trees[t].z, bg.heal_trees[t].lane);
            let before = bg.heal_trees[t].hp;
            for _ in 0..30 * 3 {
                bg.warriors[0].x = tx + 1.0;
                bg.warriors[0].z = tz;
                bg.warriors[0].lane = lane;
                bg.warriors[0].mind = Mind::Trail;
                bg.warriors[0].is_player = false;
                bg.step(1.0 / 30.0);
            }
            before - bg.heal_trees[t].hp
        };
        let hulk = gnaw(Ancestor::CaveLion);
        let skirmisher = gnaw(Ancestor::Caracal);
        assert!(
            hulk > skirmisher * 1.4,
            "the Ice Age hulk should out-gnaw the skirmisher: {hulk} vs {skirmisher}"
        );
        // and it is wood-specific: warriors take the same claws from either
        let strike = |a: Ancestor| {
            let mut bg = Battleground::new_draft(
                [a, Ancestor::Lion, Ancestor::Puma],
                [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther],
            );
            quiet(&mut bg);
            bg.warriors[0].x = 60.0;
            bg.warriors[0].z = -60.0; // away from the lion's Pride's Warmth
            bg.strike_of(0) / ancestor::kit(a).attack
        };
        assert!(
            (strike(Ancestor::CaveLion) - strike(Ancestor::Caracal)).abs() < 1e-5,
            "siege must not leak into flesh damage"
        );
    }

    #[test]
    fn a_controller_saves_its_opener_for_a_crowd_worth_gathering() {
        // Riptail beside a packmate holding a live area ult should not spend
        // Undertow on one lone scout — the pull is the setup, and a setup with
        // nothing behind it is just a shove.
        let mut bg = Battleground::new_draft(
            [Ancestor::Jaguar, Ancestor::CaveLion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Caracal],
        );
        bg.patrol_t = [1e9; 2];
        bg.wombo = true;
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        bg.warriors[1].x = x + 2.0; // Oldroar, Earthshaker ready
        bg.warriors[1].z = z;
        let enemies: Vec<usize> =
            bg.warriors.iter().enumerate().filter(|(_, w)| w.team == CLAN_B).map(|(i, _)| i).collect();
        // one lone enemy in pull range: hold
        for (n, &e) in enemies.iter().enumerate() {
            bg.warriors[e].x = if n == 0 { x + 6.0 } else { x + 200.0 };
            bg.warriors[e].z = z;
        }
        bg.auto_cast(0);
        assert_eq!(bg.warriors[0].cds[0], 0.0, "Undertow should be held for a crowd");
        // now a second enemy walks in: the gather is worth it
        bg.warriors[enemies[1]].x = x + 7.0;
        bg.warriors[enemies[1]].z = z + 1.0;
        bg.auto_cast(0);
        assert!(bg.warriors[0].cds[0] > 0.0, "with two in range the pull should fire");
        assert!(
            bg.warriors[enemies[0]].pulled_t > 0.0,
            "and the pulled are marked, so the follow-up can see the moment"
        );
    }

    #[test]
    fn an_area_ult_waits_for_the_gather_then_lands_on_it() {
        let mut bg = Battleground::new_draft(
            [Ancestor::CaveLion, Ancestor::Jaguar, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Caracal],
        );
        bg.patrol_t = [1e9; 2];
        bg.wombo = true;
        bg.warriors[0].is_player = false;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        bg.warriors[1].x = x + 2.0; // Riptail beside it, Undertow ready
        bg.warriors[1].z = z;
        let enemies: Vec<usize> =
            bg.warriors.iter().enumerate().filter(|(_, w)| w.team == CLAN_B).map(|(i, _)| i).collect();
        for (n, &e) in enemies.iter().enumerate() {
            bg.warriors[e].x = x + 6.0 + n as f32;
            bg.warriors[e].z = z;
        }
        // Oldroar holds Earthshaker while the jaguar's opener is up
        bg.auto_cast(0);
        assert_eq!(bg.warriors[0].cds[2], 0.0, "the ult should wait for the pull");
        // the jaguar pulls; now the ult has its moment
        bg.warriors[1].is_player = false;
        bg.cast(1, 0);
        assert!(bg.warriors[enemies[0]].pulled_t > 0.0);
        bg.auto_cast(0);
        assert!(bg.warriors[0].cds[2] > 0.0, "the follow-up should land on the held");
        // and with no controller nearby at all, nothing is waited for
        let mut solo = Battleground::new_draft(
            [Ancestor::CaveLion, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Caracal],
        );
        solo.patrol_t = [1e9; 2];
        solo.wombo = true;
        solo.warriors[0].is_player = false;
        let (sx, sz) = (solo.warriors[0].x, solo.warriors[0].z);
        let foes: Vec<usize> = (0..solo.warriors.len())
            .filter(|&i| solo.warriors[i].team == CLAN_B)
            .collect();
        for (n, e) in foes.into_iter().enumerate() {
            solo.warriors[e].x = sx + 5.0 + n as f32;
            solo.warriors[e].z = sz;
        }
        solo.auto_cast(0);
        assert!(solo.warriors[0].cds[2] > 0.0, "with nobody to set up, take the fight you have");
    }

    #[test]
    fn the_loner_takes_the_river_and_the_pride_climbs_together() {
        // Doctrine, asserted: whoever's kit is worth least beside a packmate
        // walks the solo lane, whatever seat they were drafted into.
        let picks = [Ancestor::Lion, Ancestor::Panther, Ancestor::Leopard];
        assert_eq!(solo_seat(&picks), 1, "the shadow-walker walks alone");
        // and the old doctrine is still reachable, because it is the baseline
        // every balance number before this was measured against
        assert_eq!(lane_for(0, &picks, LanePolicy::Seat), SOLO_LANE);
        assert_eq!(lane_for(1, &picks, LanePolicy::Seat), DUO_LANE);
        assert_eq!(lane_for(1, &picks, LanePolicy::Kit), SOLO_LANE);
        let bg = Battleground::new_draft(picks, picks);
        for team in [CLAN_A, CLAN_B] {
            let lane_of = |a: Ancestor| {
                bg.warriors
                    .iter()
                    .find(|w| w.team == team && w.ancestor == Some(a))
                    .map(|w| w.lane)
                    .unwrap()
            };
            assert_eq!(lane_of(Ancestor::Panther), SOLO_LANE);
            assert_eq!(lane_of(Ancestor::Lion), DUO_LANE);
            assert_eq!(lane_of(Ancestor::Leopard), DUO_LANE, "the mender goes where the wounded are");
        }
        // and with three loners somebody still has to take the river
        let all_alone = [Ancestor::Panther, Ancestor::Tiger, Ancestor::Smilodon];
        let bg2 = Battleground::new_draft(all_alone, all_alone);
        let solo = bg2
            .warriors
            .iter()
            .filter(|w| w.team == CLAN_A && w.role == Role::Ancestor && w.lane == SOLO_LANE)
            .count();
        assert_eq!(solo, 1, "exactly one river-walker, always");
    }

    #[test]
    fn the_clans_lane_like_the_country_is_shaped() {
        // One warrior down the river, two up the mountain, patrols holding the
        // middle. The map spent three commits growing a duo side and a solo
        // side; this is the part that makes a clan use them.
        let bg = Battleground::new(Ancestor::Lion);
        for team in [CLAN_A, CLAN_B] {
            let lanes: Vec<usize> = bg
                .warriors
                .iter()
                .filter(|w| w.team == team && w.role == Role::Ancestor)
                .map(|w| w.lane)
                .collect();
            assert_eq!(lanes.iter().filter(|&&l| l == SOLO_LANE).count(), 1, "one on the river");
            // (which one is decided by kit, not seat — see the test above)
            assert_eq!(lanes.iter().filter(|&&l| l == DUO_LANE).count(), 2, "two on the mountain");
            assert_eq!(lanes.iter().filter(|&&l| l == MID_LANE).count(), 0, "mid is the patrols'");
        }
        // and the duo pair should actually end up near each other, not merely
        // share a number — coordination reads distance, not lane ids
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        for _ in 0..30 * 20 {
            bg.step(1.0 / 30.0);
        }
        // The pair does drift — one of them peels off to hunt or fetch herbs,
        // which is the economy working. What must hold is the doctrine: never
        // BOTH at once, or the lane is unheld.
        let away = bg
            .warriors
            .iter()
            .filter(|w| {
                w.team == CLAN_A
                    && w.role == Role::Ancestor
                    && w.lane == DUO_LANE
                    && w.alive()
                    && matches!(w.mind, Mind::Hunt | Mind::Gather | Mind::CarryHome)
            })
            .count();
        assert!(away <= 1, "the duo must not both wander off: {away} away");
    }

    #[test]
    fn the_middle_gets_the_patrols_nobody_else_walks() {
        let mut bg = Battleground::new(Ancestor::Lion);
        let mut seen = [0u32; LANES];
        for _ in 0..30 * 60 * 3 {
            bg.step(1.0 / 30.0);
            for w in bg.warriors.iter().filter(|w| w.role == Role::Patrol) {
                seen[w.lane] += 1;
            }
        }
        assert!(
            seen[MID_LANE] > seen[SOLO_LANE],
            "the middle should carry the most patrol traffic: {seen:?}"
        );
    }

    #[test]
    fn the_player_drives_their_own_warrior_and_the_ai_lets_go() {
        // The whole `set_input` path had no test: every other test in this file
        // drives the AI. A human's warrior has to move where it is told, cast
        // what it is told, and stop being second-guessed by `think`.
        let mut bg = Battleground::new(Ancestor::Serval);
        bg.patrol_t = [1e9; 2];
        bg.warriors.retain(|w| w.team == CLAN_A);
        let start = (bg.warriors[0].x, bg.warriors[0].z);
        // walk east for a second
        for _ in 0..30 {
            bg.set_input(1.0, 0.0, false, None);
            bg.step(1.0 / 30.0);
        }
        let moved = bg.warriors[0].x - start.0;
        assert!(moved > 3.0, "the player's warrior should have walked east, got {moved}");
        assert!((bg.warriors[0].z - start.1).abs() < 0.01, "and not drifted north");
        assert!(bg.player_driven[CLAN_A as usize], "the first input hands the seat over");
        // the AI must not be steering it any more: hold no input and it holds still
        let held = (bg.warriors[0].x, bg.warriors[0].z);
        for _ in 0..30 {
            bg.set_input(0.0, 0.0, false, None);
            bg.step(1.0 / 30.0);
        }
        assert!(
            dist(bg.warriors[0].x, bg.warriors[0].z, held.0, held.1) < 0.01,
            "an idle player is idle, not auto-piloted"
        );
        // a rooted player cannot walk, and knows it
        bg.warriors[0].root_t = 1.0;
        let rooted = (bg.warriors[0].x, bg.warriors[0].z);
        for _ in 0..15 {
            bg.set_input(1.0, 0.0, false, None);
            bg.step(1.0 / 30.0);
        }
        assert!(dist(bg.warriors[0].x, bg.warriors[0].z, rooted.0, rooted.1) < 0.01);
    }

    #[test]
    fn the_players_claw_is_the_same_blow_an_ancestor_throws() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Lynx, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Puma, Ancestor::Leopard, Ancestor::Panther],
        );
        bg.patrol_t = [1e9; 2];
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        bg.warriors[e].x = x + 1.5;
        bg.warriors[e].z = z;
        bg.warriors[e].root_t = 100.0; // hold it still; this is about the blow
        let hp0 = bg.warriors[e].hp;
        bg.set_input(0.0, 0.0, true, None);
        bg.step(1.0 / 30.0);
        assert!(bg.warriors[e].hp < hp0, "the claw should land");
        // Snowhush's Hush of Snows must fire from a human's paw too
        assert!(bg.warriors[e].slow_t > 0.0, "on-hit riders apply to player blows");
        // and the kill is credited
        bg.warriors[e].hp = 0.01;
        bg.set_input(0.0, 0.0, true, None);
        bg.step(1.0 / 30.0);
        assert!(!bg.warriors[e].alive());
        assert_eq!(bg.warriors[0].kills, 1, "a player's kill counts as a kill");
    }

    #[test]
    fn the_player_casts_by_slot_and_pays_the_cooldown() {
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        // Nobody within reach — the first draft of this test parked a rooted
        // Nightpelt five metres away and watched the shield vanish inside the
        // same tick: at night its Pounce of Midnight lands 29 against a 28-point
        // Mane of Dawn. The kits are working; the test was measuring a duel.
        for i in 0..bg.warriors.len() {
            if bg.warriors[i].team == CLAN_B {
                bg.warriors[i].x = 140.0;
                bg.warriors[i].z = 140.0;
            }
        }
        bg.set_input(0.0, 0.0, false, Some(1)); // Mane of Dawn
        bg.step(1.0 / 30.0);
        assert!(bg.warriors[0].shield > 0.0, "slot 1 should have gone off");
        assert!(bg.warriors[0].cds[1] > 0.0, "and started its cooldown");
        // an out-of-range slot index is clamped rather than panicking
        bg.set_input(0.0, 0.0, false, Some(9));
        bg.step(1.0 / 30.0);
        assert!(bg.warriors[0].cds[2] > 0.0, "slot 9 clamps to the ultimate");
    }

    #[test]
    fn giving_ground_costs_the_blow() {
        use crate::ancestor::kit;
        // A yowler pressed to muzzle range backs off instead of striking; the
        // same yowler at comfortable range strikes and holds. Melee kits never
        // kite at all, so their blow is never traded away.
        let step = |a: Ancestor, gap: f32| -> (f32, f32) {
            let mut bg = Battleground::new_draft(
                [a, Ancestor::Lion, Ancestor::Puma],
                [Ancestor::Puma, Ancestor::Leopard, Ancestor::Panther],
            );
            bg.patrol_t = [1e9; 2];
            bg.warriors[0].is_player = false;
            // out in the middle: within LAST_STAND_R of a camp a warrior defends
            // the Great Tree instead, and that branch does not kite — the first
            // draft of this test staged the duel on the doorstep and measured
            // the last stand
            bg.warriors[0].x = 10.0;
            bg.warriors[0].z = -10.0;
            let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
            let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
            bg.warriors[e].x = x + gap;
            bg.warriors[e].z = z;
            bg.warriors[e].root_t = 100.0; // hold the target still
            bg.warriors[e].hp = bg.warriors[e].max_hp;
            // abilities on cooldown: this is about the BASIC blow. A yowler
            // backing off can still spend Piercing Yowl, and should — spacing is
            // what its abilities are paid for; it is the free auto-attack while
            // retreating that was the problem.
            bg.warriors[0].cds = [30.0; 3];
            let hp0 = bg.warriors[e].hp;
            bg.step(1.0 / 30.0);
            let moved = dist(bg.warriors[0].x, bg.warriors[0].z, x, z);
            (hp0 - bg.warriors[e].hp, moved)
        };
        let close = kit(Ancestor::Serval).reach * 0.3; // inside the kite threshold
        let far = kit(Ancestor::Serval).reach * 0.8;
        let (dmg_close, moved_close) = step(Ancestor::Serval, close);
        let (dmg_far, moved_far) = step(Ancestor::Serval, far);
        assert!(moved_close > 0.0, "pressed close, the yowler gives ground");
        assert!(dmg_far > 0.0, "at range it strikes");
        assert!(
            dmg_close < dmg_far * 0.75,
            "a yowl thrown over the shoulder lands softer: {dmg_close} against {dmg_far}"
        );
        assert!(dmg_close > 0.0, "but it is not silent");
        assert!(moved_far < 0.01, "and holds its ground");
        // a melee kit at the same close distance strikes and stays
        let (dmg_melee, moved_melee) = step(Ancestor::Smilodon, 2.0);
        assert!(dmg_melee > 0.0 && moved_melee < 0.01, "melee never kites");
    }

    #[test]
    fn the_blow_from_the_dark_lands_and_then_the_veil_is_gone() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Panther, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Puma, Ancestor::Leopard, Ancestor::Caracal],
        );
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].is_player = false;
        bg.warriors[0].x = 10.0;
        bg.warriors[0].z = -10.0;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = x + 2.0;
        bg.warriors[e].z = z;
        bg.warriors[e].root_t = 100.0;
        bg.warriors[0].cds = [30.0; 3];
        // struck from the veil: the bonus is banked into THIS blow…
        bg.warriors[0].stealth_t = 5.0;
        let veiled = bg.strike_of(0);
        bg.warriors[0].stealth_t = 0.0;
        let plain = bg.strike_of(0);
        assert!(veiled > plain * 1.2, "Night's Hunger should feed the opening strike");
        // …and then the ambush is over
        bg.warriors[0].stealth_t = 5.0;
        bg.step(1.0 / 30.0);
        assert_eq!(bg.warriors[0].stealth_t, 0.0, "the blow ends the ambush");
        assert!(bg.warriors[e].hp < bg.warriors[e].max_hp, "and it still landed");
        // a warder's own shield does not give them away
        let mut lion = Battleground::new(Ancestor::Lion);
        lion.patrol_t = [1e9; 2];
        lion.warriors[0].stealth_t = 5.0;
        lion.cast(0, 1); // Mane of Dawn
        assert_eq!(lion.warriors[0].stealth_t, 5.0, "shielding is not a blow");
        // nor does casting Veilwalker itself
        let mut cat = Battleground::new(Ancestor::Tiger);
        cat.patrol_t = [1e9; 2];
        cat.cast(0, 1); // Stripes in the Reeds
        assert!(cat.warriors[0].stealth_t > 0.0, "the veil may still be raised");
    }

    #[test]
    fn guard_hair_settles_when_its_moment_passes() {
        use crate::ancestor::kit;
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        bg.warriors.retain(|w| w.team == CLAN_A);
        bg.heal_trees.clear();
        bg.warriors[0].is_player = false;
        bg.cast(0, 1); // Mane of Dawn
        let dur = kit(Ancestor::Lion).abilities[1].dur;
        assert!(bg.warriors[0].shield > 0.0 && bg.warriors[0].shield_t > 0.0);
        // it holds for its declared moment…
        for _ in 0..((dur - 0.5) * 30.0) as usize {
            bg.step(1.0 / 30.0);
        }
        assert!(bg.warriors[0].shield > 0.0, "the guard-hair should still be up at {dur}s");
        // …and not a match longer
        for _ in 0..30 {
            bg.step(1.0 / 30.0);
        }
        assert_eq!(bg.warriors[0].shield, 0.0, "and then it settles");
        assert_eq!(bg.warriors[0].shield_t, 0.0);
    }

    #[test]
    fn a_hillside_of_mice_does_not_make_a_war_hero() {
        // Skyward Hunger quickens the caracal's paws with every kill. Prey are
        // not kills: a kit should not stack a war passive off things that do not
        // fight back.
        let mut bg = Battleground::new(Ancestor::Caracal);
        quiet(&mut bg);
        bg.warriors[0].x = 20.0;
        bg.warriors[0].z = -20.0;
        let base = bg.speed_of(0);
        bg.warriors[0].prey_taken = 10;
        assert_eq!(bg.speed_of(0), base, "prey must not quicken the paws");
        bg.warriors[0].kills = 10;
        assert!(bg.speed_of(0) > base * 1.3, "warriors do");
        // and a hunt credits prey_taken, not kills
        let mut bg = Battleground::new(Ancestor::Caracal);
        quiet(&mut bg);
        let p = bg.prey.iter().position(|p| p.alive()).unwrap();
        let (px, pz) = (bg.prey[p].x, bg.prey[p].z);
        bg.warriors[1].x = px;
        bg.warriors[1].z = pz;
        bg.warriors[1].mind = Mind::Hunt;
        for _ in 0..30 * 20 {
            bg.step(1.0 / 30.0);
            if bg.warriors[1].carry != Carry::Nothing {
                break;
            }
        }
        assert!(bg.warriors[1].prey_taken > 0, "the hunt should be counted");
        assert_eq!(bg.warriors[1].kills, 0, "but not as a war record");
    }

    #[test]
    fn the_river_mends_between_fights_not_during_one() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Leopard, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Puma, Ancestor::Caracal, Ancestor::Panther],
        );
        bg.patrol_t = [1e9; 2];
        bg.heal_trees.clear(); // isolate Riverborn from the aura
        let Some((wx, wz)) = world::nearest_water((0.0, 0.0), 140.0) else { return };
        bg.warriors[0].x = wx;
        bg.warriors[0].z = wz;
        bg.warriors[0].hp = 50.0;
        bg.warriors[0].mind = Mind::AtMedic; // hold position
        bg.warriors[0].is_player = false;
        // an enemy on the bank: no mending
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = wx + 5.0;
        bg.warriors[e].z = wz;
        bg.warriors[e].root_t = 100.0;
        bg.warriors[e].attack = 0.0;
        let before = bg.warriors[0].hp;
        for _ in 0..30 {
            bg.warriors[0].x = wx;
            bg.warriors[0].z = wz;
            bg.warriors[0].mind = Mind::AtMedic;
            bg.step(1.0 / 30.0);
        }
        assert!(bg.warriors[0].hp <= before + 0.01, "pressed, the river gives nothing");
        // send the enemy away and the shallows close the wounds
        bg.warriors[e].x = wx + 120.0;
        let mid = bg.warriors[0].hp;
        for _ in 0..30 {
            bg.warriors[0].x = wx;
            bg.warriors[0].z = wz;
            bg.warriors[0].mind = Mind::AtMedic;
            bg.step(1.0 / 30.0);
        }
        assert!(bg.warriors[0].hp > mid + 1.0, "alone in the shallows, it mends");
    }

    #[test]
    fn the_cold_bites_deeper_for_the_cat_that_made_it() {
        let mut bg = Battleground::new_draft(
            [Ancestor::Lynx, Ancestor::Lion, Ancestor::Puma],
            [Ancestor::Puma, Ancestor::Leopard, Ancestor::Panther],
        );
        bg.patrol_t = [1e9; 2];
        let e = bg.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        bg.warriors[e].x = bg.warriors[0].x + 2.0;
        bg.warriors[e].z = bg.warriors[0].z;
        let warm = bg.strike_vs(0, e);
        bg.warriors[e].slow_t = 1.0;
        assert!(bg.strike_vs(0, e) > warm * 1.2, "the lynx collects on its own chill");
        // and it is the lynx's alone — a lion gets nothing from a chilled target
        let mut lion = Battleground::new(Ancestor::Lion);
        lion.patrol_t = [1e9; 2];
        let le = lion.warriors.iter().position(|w| w.team == CLAN_B).unwrap();
        lion.warriors[le].x = lion.warriors[0].x + 2.0;
        lion.warriors[le].z = lion.warriors[0].z;
        let plain = lion.strike_vs(0, le);
        lion.warriors[le].slow_t = 1.0;
        assert_eq!(lion.strike_vs(0, le), plain);
    }

    #[test]
    fn the_patrol_turns_on_the_one_already_bleeding() {
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        bg.warriors[0].x = 20.0;
        bg.warriors[0].z = -20.0;
        let (x, z) = (bg.warriors[0].x, bg.warriors[0].z);
        let foes: Vec<usize> =
            (0..bg.warriors.len()).filter(|&i| bg.warriors[i].team == CLAN_B).collect();
        // a hale cat right beside it, a wounded one a few lengths off
        bg.warriors[foes[0]].x = x + 2.0;
        bg.warriors[foes[0]].z = z;
        bg.warriors[foes[1]].x = x + 6.0;
        bg.warriors[foes[1]].z = z;
        for &f in &foes[2..] {
            bg.warriors[f].x = 140.0;
            bg.warriors[f].z = 140.0;
        }
        assert_eq!(bg.nearest_enemy(0, 30.0), Some(foes[0]), "all else equal, the near one");
        bg.warriors[foes[1]].hp = bg.warriors[foes[1]].max_hp * 0.2;
        assert_eq!(
            bg.nearest_enemy(0, 30.0),
            Some(foes[1]),
            "a wounded cat a few lengths off outbids a hale one underfoot"
        );
        // but not from across the clearing: distance still counts
        bg.warriors[foes[1]].x = x + 28.0;
        assert_eq!(bg.nearest_enemy(0, 30.0), Some(foes[0]));
    }

    #[test]
    fn a_patrol_that_meets_a_bigger_patrol_withdraws() {
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        // out in the open, away from either camp's last-stand radius
        let (x, z) = (20.0f32, -20.0f32);
        bg.warriors[1].x = x;
        bg.warriors[1].z = z;
        bg.warriors[1].hp = bg.warriors[1].max_hp; // whole and hearty
        let foes: Vec<usize> =
            (0..bg.warriors.len()).filter(|&i| bg.warriors[i].team == CLAN_B).collect();
        // one enemy: a fair fight, and it stays
        for (n, &f) in foes.iter().enumerate() {
            bg.warriors[f].x = if n == 0 { x + 4.0 } else { 140.0 };
            bg.warriors[f].z = if n == 0 { z } else { 140.0 };
        }
        bg.warriors[0].x = 140.0;
        bg.warriors[0].z = 140.0;
        bg.step(1.0 / 30.0);
        assert_ne!(bg.warriors[1].mind, Mind::Retreat, "one against one is a fight");
        // three of them: the odds are the other half of nerve
        for (n, &f) in foes.iter().enumerate() {
            bg.warriors[f].x = x + 4.0 + n as f32;
            bg.warriors[f].z = z;
        }
        bg.warriors[1].x = x;
        bg.warriors[1].z = z;
        bg.warriors[1].mind = Mind::Trail;
        bg.step(1.0 / 30.0);
        assert_eq!(bg.warriors[1].mind, Mind::Retreat, "three against one is a retreat");
        // and a veiled enemy cannot frighten anyone: you do not count what you
        // cannot see
        for &f in &foes {
            bg.warriors[f].stealth_t = 5.0;
        }
        bg.warriors[1].x = x;
        bg.warriors[1].z = z;
        bg.warriors[1].mind = Mind::Trail;
        bg.step(1.0 / 30.0);
        assert_ne!(bg.warriors[1].mind, Mind::Retreat, "unseen cats do not count against the odds");
    }

    #[test]
    fn the_clan_collapses_onto_a_broken_border() {
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        bg.collapse_on_breach = true;
        // fell one of clan B's Heal Trees: clan A now has a breach to push
        let t = bg.heal_trees.iter().position(|t| t.team == CLAN_B).unwrap();
        let breach = bg.heal_trees[t].lane;
        bg.heal_trees[t].hp = 0.0;
        // put clan A's ancestors out on their own intact trails, whole and healthy
        for i in 0..bg.warriors.len() {
            if bg.warriors[i].team == CLAN_A {
                bg.warriors[i].x = -40.0;
                bg.warriors[i].z = -40.0;
                bg.warriors[i].hp = bg.warriors[i].max_hp;
                bg.warriors[i].mind = Mind::Trail;
            } else {
                bg.warriors[i].x = 140.0;
                bg.warriors[i].z = 140.0;
            }
        }
        bg.step(1.0 / 30.0);
        // the duo pair sends one and keeps one: a push is not an abandonment
        let a_lanes: Vec<(u32, usize)> = bg
            .warriors
            .iter()
            .filter(|w| w.team == CLAN_A && w.role == Role::Ancestor)
            .map(|w| (w.seat, w.lane))
            .collect();
        assert!(
            a_lanes.iter().any(|&(_, l)| l == breach),
            "somebody heads for the hole in their border"
        );
        for lane in [DUO_LANE, SOLO_LANE] {
            if lane == breach {
                continue;
            }
            let started_there: Vec<u32> = a_lanes
                .iter()
                .filter(|&&(_, l)| l == lane)
                .map(|&(s, _)| s)
                .collect();
            // whichever lanes still have somebody, that somebody is the lowest
            // seat of the group — the holder, not a leftover
            if let Some(&min) = started_there.iter().min() {
                assert_eq!(started_there.len(), 1, "one holder, not a crowd");
                let _ = min;
            }
        }
        // a wounded one does not: it is going to a sanctuary, and a push is not
        // worth a life
        let hurt = bg.warriors.iter().position(|w| w.team == CLAN_A).unwrap();
        bg.warriors[hurt].lane = (breach + 1) % LANES;
        bg.warriors[hurt].hp = bg.warriors[hurt].max_hp * 0.3;
        bg.warriors[hurt].mind = Mind::Trail;
        bg.step(1.0 / 30.0);
        assert_ne!(bg.warriors[hurt].lane, breach);
    }

    #[test]
    fn every_felled_border_softens_the_great_tree() {
        // Felling a Heal Tree has to buy something, or taking objectives is a
        // bill rather than an advantage — which is exactly what the harness
        // measured before this: the clan that broke the first border won under
        // half its matches.
        let gnaw_rate = |fell: usize| {
            let mut bg = Battleground::new(Ancestor::CaveLion);
            bg.patrol_t = [1e9; 2];
            bg.warriors.retain(|w| w.team == CLAN_B);
            for (n, t) in bg
                .heal_trees
                .iter_mut()
                .filter(|t| t.team == CLAN_A)
                .enumerate()
            {
                if n < fell {
                    t.hp = 0.0;
                }
            }
            let (cx, cz) = bg.camps[CLAN_A as usize];
            let before = bg.clans[CLAN_A as usize].great_tree_hp;
            for _ in 0..30 * 3 {
                for i in 0..bg.warriors.len() {
                    bg.warriors[i].x = cx + 2.0;
                    bg.warriors[i].z = cz;
                    bg.warriors[i].hp = bg.warriors[i].max_hp;
                    bg.warriors[i].mind = Mind::Trail;
                }
                bg.step(1.0 / 30.0);
            }
            before - bg.clans[CLAN_A as usize].great_tree_hp
        };
        let (none, one, all) = (gnaw_rate(0), gnaw_rate(1), gnaw_rate(3));
        assert!(one > none * 1.1, "the first border must buy something: {none} -> {one}");
        assert!(all > one * 1.1, "and the next ones more: {one} -> {all}");
    }

    #[test]
    fn a_breach_pulls_patrols_but_not_every_patrol() {
        let mut bg = Battleground::new(Ancestor::Lion);
        // fell a clan B border: clan A now has somewhere to press
        let t = bg.heal_trees.iter().position(|t| t.team == CLAN_B).unwrap();
        let breach = bg.heal_trees[t].lane;
        bg.heal_trees[t].hp = 0.0;
        let mut on_breach = 0;
        let mut elsewhere = 0;
        for wave in 0..8 {
            let before = bg.warriors.len();
            bg.spawn_patrol(CLAN_A, {
                let rota = [MID_LANE, DUO_LANE, MID_LANE, SOLO_LANE];
                match bg.breach_lane(CLAN_A) {
                    Some(b) if wave % 2 == 0 => b,
                    _ => rota[wave % rota.len()],
                }
            }, Duty::Border);
            let lane = bg.warriors[before].lane;
            if lane == breach { on_breach += 1 } else { elsewhere += 1 }
        }
        assert!(on_breach > 0, "the breach should pull patrols");
        assert!(elsewhere > 0, "but the other trails must not be abandoned");
    }

    #[test]
    fn a_felled_border_can_be_made_to_pay_and_only_once() {
        let mut bg = Battleground::new(Ancestor::CaveLion);
        bg.patrol_t = [1e9; 2];
        bg.spoils = true;
        bg.warriors.retain(|w| w.team == CLAN_A);
        let t = bg.heal_trees.iter().position(|t| t.team == CLAN_B).unwrap();
        let (tx, tz, lane) = (bg.heal_trees[t].x, bg.heal_trees[t].z, bg.heal_trees[t].lane);
        bg.heal_trees[t].hp = 1.0; // one gnaw from falling
        let before = bg.clans[CLAN_A as usize].pile;
        for _ in 0..30 * 3 {
            bg.warriors[0].x = tx + 1.0;
            bg.warriors[0].z = tz;
            bg.warriors[0].lane = lane;
            bg.warriors[0].mind = Mind::Trail;
            bg.warriors[0].is_player = false;
            bg.step(1.0 / 30.0);
        }
        assert!(!bg.heal_trees[t].alive());
        let gained = bg.clans[CLAN_A as usize].pile - before;
        assert!(
            (gained - BORDER_SPOILS).abs() < 1e-3,
            "the roots pay once, not per gnaw: {gained}"
        );
        // and with the switch off the border pays nothing
        let mut plain = Battleground::new(Ancestor::CaveLion);
        plain.patrol_t = [1e9; 2];
        plain.warriors.retain(|w| w.team == CLAN_A);
        let t2 = plain.heal_trees.iter().position(|t| t.team == CLAN_B).unwrap();
        let (px, pz, plane) =
            (plain.heal_trees[t2].x, plain.heal_trees[t2].z, plain.heal_trees[t2].lane);
        plain.heal_trees[t2].hp = 1.0;
        let p0 = plain.clans[CLAN_A as usize].pile;
        for _ in 0..30 * 3 {
            plain.warriors[0].x = px + 1.0;
            plain.warriors[0].z = pz;
            plain.warriors[0].lane = plane;
            plain.warriors[0].mind = Mind::Trail;
            plain.warriors[0].is_player = false;
            plain.step(1.0 / 30.0);
        }
        assert!(!plain.heal_trees[t2].alive());
        assert_eq!(plain.clans[CLAN_A as usize].pile, p0);
    }

    #[test]
    fn the_growth_curve_bites_inside_the_piles_a_match_actually_reaches() {
        // A curve whose interesting range sits above anything the mode produces
        // is a decoration. Real piles run single digits, so that is where the
        // difference has to live.
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.clans[CLAN_A as usize].pile = 3.0;
        let modest = bg.growth(CLAN_A);
        bg.clans[CLAN_A as usize].pile = 9.0;
        let fed = bg.growth(CLAN_A);
        assert!(
            fed - modest > 0.1,
            "three against nine should be worth more than a tenth: {modest:.3} -> {fed:.3}"
        );
        // and it still saturates rather than running away
        bg.clans[CLAN_A as usize].pile = 1e6;
        assert!(bg.growth(CLAN_A) < 1.0 + GROWTH_CAP + 1e-4);
    }

    #[test]
    fn both_clans_can_have_a_human_at_the_paws() {
        // The lockstep protocol was already carrying clan B's frame across the
        // wire while the sim had nowhere to put it. Now it does.
        let mut bg = Battleground::new(Ancestor::Serval);
        bg.patrol_t = [1e9; 2];
        let (a, b) = (bg.driven(CLAN_A), bg.driven(CLAN_B));
        assert_eq!((a, b), (None, None), "nobody is driving until somebody drives");
        // move each clan's leader in a different direction and watch both go
        let ai = bg.warriors.iter().position(|w| w.team == CLAN_A && w.is_player).unwrap();
        let bi = bg.warriors.iter().position(|w| w.team == CLAN_B && w.is_player).unwrap();
        let (ax, az) = (bg.warriors[ai].x, bg.warriors[ai].z);
        let (bx, bz) = (bg.warriors[bi].x, bg.warriors[bi].z);
        for _ in 0..30 {
            bg.set_input_for(CLAN_A, 1.0, 0.0, false, None);
            bg.set_input_for(CLAN_B, 0.0, -1.0, false, None);
            bg.step(1.0 / 30.0);
        }
        assert_eq!(bg.driven(CLAN_A), Some(ai));
        assert_eq!(bg.driven(CLAN_B), Some(bi));
        assert!(bg.warriors[ai].x - ax > 3.0, "clan A's leader walked east");
        assert!((bg.warriors[ai].z - az).abs() < 0.01);
        assert!(bz - bg.warriors[bi].z > 3.0, "clan B's leader walked north");
        assert!((bg.warriors[bi].x - bx).abs() < 0.01);
    }

    #[test]
    fn an_undriven_clan_still_thinks_for_itself() {
        // Driving one side must not switch the other side's AI off — the common
        // case is one human against a clan of ancestors.
        let mut bg = Battleground::new(Ancestor::Lion);
        bg.patrol_t = [1e9; 2];
        let bi = bg.warriors.iter().position(|w| w.team == CLAN_B && w.is_player).unwrap();
        let before = (bg.warriors[bi].x, bg.warriors[bi].z);
        for _ in 0..60 {
            bg.set_input(0.0, 0.0, false, None);
            bg.step(1.0 / 30.0);
        }
        assert!(
            dist(bg.warriors[bi].x, bg.warriors[bi].z, before.0, before.1) > 1.0,
            "clan B's leader should have got on with its patrol"
        );
    }

    #[test]
    fn a_clan_of_five_takes_the_conquest_shape() {
        use crate::ancestor::ALL;
        let five: Vec<Ancestor> = ALL[..5].to_vec();
        let bg = Battleground::new_roster(&five, &five, 0, LanePolicy::Kit);
        for team in [CLAN_A, CLAN_B] {
            let lanes: Vec<usize> = bg
                .warriors
                .iter()
                .filter(|w| w.team == team && w.role == Role::Ancestor)
                .map(|w| w.lane)
                .collect();
            assert_eq!(lanes.len(), 5, "five a side");
            assert_eq!(lanes.iter().filter(|&&l| l == SOLO_LANE).count(), 1, "one on the river");
            assert_eq!(lanes.iter().filter(|&&l| l == DUO_LANE).count(), 2, "two on the mountain");
            assert_eq!(
                lanes.iter().filter(|&&l| l == MID_LANE).count(),
                2,
                "the middle and the roamer — with five there is finally somebody to hold it"
            );
        }
        assert!(roamer_seat(&five).is_some(), "a full five fields a roamer");
        assert!(roamer_seat(&five[..3]).is_none(), "three does not");
        // and a five-a-side match still resolves, which is the real question:
        // every rule in here was tuned against three
        let mut bg = Battleground::new_roster(&five, &ALL[5..10].to_vec(), 1, LanePolicy::Kit);
        let mut steps = 0;
        while bg.outcome.is_none() && steps < 30 * 60 * 13 {
            bg.step(1.0 / 30.0);
            steps += 1;
        }
        assert!(bg.outcome.is_some(), "a 5v5 must end");
    }

    #[test]
    fn a_short_roster_still_fields_a_clan() {
        // The lobby fills empty seats with bots, but nothing should explode if a
        // caller hands over two, or one.
        for n in 1..=5usize {
            let r: Vec<Ancestor> = crate::ancestor::ALL[..n].to_vec();
            let mut bg = Battleground::new_roster(&r, &r, 0, LanePolicy::Kit);
            assert_eq!(
                bg.warriors.iter().filter(|w| w.team == CLAN_A && w.role == Role::Ancestor).count(),
                n
            );
            for _ in 0..30 * 10 {
                bg.step(1.0 / 30.0);
            }
        }
    }

    #[test]
    fn matches_are_deterministic() {
        let run = || {
            let mut bg = Battleground::new(Ancestor::Panther);
            for _ in 0..30 * 40 { bg.step(1.0 / 30.0); }
            bg.digest()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn a_full_match_resolves() {
        let mut bg = Battleground::new(Ancestor::Lion);
        let mut steps = 0u32;
        while bg.outcome.is_none() && steps < (MATCH_LEN as u32 + 5) * 30 {
            bg.step(1.0 / 30.0);
            steps += 1;
        }
        assert!(bg.outcome.is_some(), "a match must end by felling or full time");
        // the economy actually ran: somebody ate
        assert!(bg.clans[0].pile + bg.clans[1].pile > 0.0, "no freshkill was ever brought home");
    }
}
