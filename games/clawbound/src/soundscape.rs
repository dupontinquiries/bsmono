//! soundscape.rs — what the territory sounds like.
//!
//! Two halves, because ambience and information are different jobs and a single
//! mechanism does one of them badly.
//!
//! **Beds** are the AmbientSounds idea, kept: a handful of looping layers chosen
//! from what is *around* the listener — how much water, how much canopy, how
//! high, whether it is night, whether leaf-bare has come — each fading in and
//! out rather than switching, so walking from the river up onto the crags is a
//! change you hear happening instead of a cut. Sampling the world beats tagging
//! it, and this world already answers questions about itself (`world::biome_at`,
//! `junglemap::concealment_at`) so nothing has to be authored twice.
//!
//! **Nodes** are the half a MOBA needs and a Minecraft mod does not. They are
//! emitters that live at a place — a prey camp, a herb patch, a mistfall, a Heal
//! Tree, the Skyrock — and they are not only atmosphere: **a node's voice is
//! state**. A hunting camp murmurs while its prey is alive and goes quiet when
//! it has been taken, so a player learns to hear a camp that somebody else has
//! already cleared. A Heal Tree hums while it stands and creaks while it is
//! being gnawed, which is a warning arriving through the ears from a lane you
//! cannot see. That is the "signal jungle camps and events" part, and it is why
//! nodes carry a `Voice` rather than just a file name.
//!
//! **Events** are one-shots on top: a border falling, the Skyrock changing paws,
//! night coming, your own death. They duck the beds for a moment, the way a
//! thunderclap flattens a room, so a cue that matters is never lost inside the
//! wind.
//!
//! The whole module is pure: it takes a world and a listener and returns a mix —
//! a list of (cue, gain, pan) — and never touches an audio device. A backend
//! plays what it is handed; every rule about *what* should be heard, and how
//! loudly, is here where it can be tested.

use crate::battleground::{Battleground, Mind, Role};
use crate::junglemap;
use crate::world;

/// A sound the game can ask for. Ids rather than paths: the asset layout is the
/// backend's business, and a missing file should be a silent cue rather than a
/// crash.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Cue {
    // beds
    BedForest,
    BedMoor,
    BedRiver,
    BedCrag,
    BedNight,
    BedLeafbare,
    // nodes
    CampPrey,
    CampEmpty,
    HerbPatch,
    Mistfall,
    HealTreeHum,
    HealTreeCreak,
    GreatTree,
    Skyrock,
    Thicket,
    // events
    BorderFell,
    SkyrockTaken,
    NightFalls,
    PreyTaken,
    HerbGathered,
    WarriorDown,
    ClanVictory,
}

impl Cue {
    /// A stable name for the asset table and for logs.
    pub fn id(self) -> &'static str {
        match self {
            Cue::BedForest => "bed.forest",
            Cue::BedMoor => "bed.moor",
            Cue::BedRiver => "bed.river",
            Cue::BedCrag => "bed.crag",
            Cue::BedNight => "bed.night",
            Cue::BedLeafbare => "bed.leafbare",
            Cue::CampPrey => "node.camp.prey",
            Cue::CampEmpty => "node.camp.empty",
            Cue::HerbPatch => "node.herb",
            Cue::Mistfall => "node.mistfall",
            Cue::HealTreeHum => "node.healtree.hum",
            Cue::HealTreeCreak => "node.healtree.creak",
            Cue::GreatTree => "node.greattree",
            Cue::Skyrock => "node.skyrock",
            Cue::Thicket => "node.thicket",
            Cue::BorderFell => "event.border-fell",
            Cue::SkyrockTaken => "event.skyrock",
            Cue::NightFalls => "event.night",
            Cue::PreyTaken => "event.prey",
            Cue::HerbGathered => "event.herb",
            Cue::WarriorDown => "event.down",
            Cue::ClanVictory => "event.victory",
        }
    }
    /// One-shots do not loop and are not ducked by other one-shots.
    pub fn is_event(self) -> bool {
        matches!(
            self,
            Cue::BorderFell
                | Cue::SkyrockTaken
                | Cue::NightFalls
                | Cue::PreyTaken
                | Cue::HerbGathered
                | Cue::WarriorDown
                | Cue::ClanVictory
        )
    }
}

/// What a node is saying right now. The same place, different news.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Voice {
    /// Nothing worth saying — the node is silent this frame.
    Quiet,
    /// Its ordinary sound.
    Idle,
    /// Something is happening here: prey up, a tree being gnawed, ground held.
    Active,
}

/// An emitter that lives at a place.
#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub cue: Cue,
    pub x: f32,
    pub z: f32,
    /// Silent beyond this.
    pub radius: f32,
    /// Loudness at the centre, before falloff.
    pub gain: f32,
    pub voice: Voice,
}

/// One line of the mix a backend should be playing.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Voiced {
    pub cue: Cue,
    /// 0..1 after falloff, bed weighting and ducking.
    pub gain: f32,
    /// -1 (left) .. 1 (right), relative to where the listener is facing.
    pub pan: f32,
}

/// How fast a bed fades in and out, in gain per second. Slow enough that
/// crossing a treeline is a slope rather than a step.
pub const BED_FADE: f32 = 0.6;
/// How long an event ducks the beds, and by how much.
pub const DUCK_SECS: f32 = 1.2;
pub const DUCK_TO: f32 = 0.45;
/// Node audibility. Generous, because these are signals as much as atmosphere:
/// hearing a camp you cannot see is the point.
pub const NODE_R: f32 = 34.0;

const BEDS: [Cue; 6] = [
    Cue::BedForest,
    Cue::BedMoor,
    Cue::BedRiver,
    Cue::BedCrag,
    Cue::BedNight,
    Cue::BedLeafbare,
];

/// The listener: where the ears are and which way they point.
#[derive(Clone, Copy, Debug)]
pub struct Ears {
    pub x: f32,
    pub z: f32,
    pub heading: f32,
}

pub struct Soundscape {
    /// Current gain of each bed, fading toward its target.
    bed: [f32; 6],
    /// Events fired *this frame*. A one-shot appears in the mix once, on the
    /// frame it happens: a backend plays a cue when it is handed one, so
    /// repeating it every frame for the length of the duck would retrigger the
    /// same thunderclap thirty-six times.
    fired: Vec<Cue>,
    duck: f32,
    /// Last-seen state, for spotting the transitions that make events.
    seen_trees: Vec<bool>,
    seen_skyrock: Option<u8>,
    seen_night: bool,
    seen_pile: [f32; 2],
    seen_herbs: [u32; 2],
    seen_down: Vec<bool>,
    started: bool,
}

impl Default for Soundscape {
    fn default() -> Self {
        Soundscape::new()
    }
}

impl Soundscape {
    pub fn new() -> Soundscape {
        Soundscape {
            bed: [0.0; 6],
            fired: Vec::new(),
            duck: 0.0,
            seen_trees: Vec::new(),
            seen_skyrock: None,
            seen_night: false,
            seen_pile: [0.0; 2],
            seen_herbs: [0; 2],
            seen_down: Vec::new(),
            started: false,
        }
    }

    /// What each bed *should* be, given where the ears are. Sampled from the
    /// world rather than authored: the terrain already knows whether it is
    /// water, canopy or bare rock.
    pub fn bed_targets(bg: &Battleground, ears: Ears) -> [f32; 6] {
        let mut t = [0.0f32; 6];
        // sample a small ring around the listener — one point under the paws
        // says "in a puddle", nine say "at the river"
        let (mut wet, mut tree, mut high) = (0.0f32, 0.0f32, 0.0f32);
        let n = 9;
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (sx, sz) = (ears.x + a.cos() * 9.0, ears.z + a.sin() * 9.0);
            let h = world::height_at(sx, sz);
            if h < world::WATER_Y {
                wet += 1.0 / n as f32;
            }
            tree += world::biome_at(sx, sz).tree / n as f32;
            high += (h / 8.0).clamp(0.0, 1.0) / n as f32;
        }
        t[0] = tree.clamp(0.0, 1.0); // forest
        t[1] = (1.0 - tree - wet).clamp(0.0, 1.0) * 0.8; // open moor
        t[2] = (wet * 2.0).clamp(0.0, 1.0); // river
        t[3] = high; // crag
        t[4] = if bg.is_night() { 0.8 } else { 0.0 };
        t[5] = if bg.leafbare() { 0.7 } else { 0.0 };
        t
    }

    /// Every node in earshot, with the voice its state gives it. This is the
    /// half that carries information: a camp with prey sounds different from a
    /// camp somebody has already cleared, and a Heal Tree being gnawed is a
    /// warning from a lane you cannot see.
    pub fn nodes(bg: &Battleground, ears: Ears) -> Vec<Node> {
        let mut out = Vec::new();
        let near = |x: f32, z: f32| {
            let d = ((x - ears.x).powi(2) + (z - ears.z).powi(2)).sqrt();
            d < NODE_R
        };
        for p in &bg.prey {
            if !near(p.x, p.z) {
                continue;
            }
            out.push(Node {
                cue: if p.alive() { Cue::CampPrey } else { Cue::CampEmpty },
                x: p.home.0,
                z: p.home.1,
                radius: NODE_R,
                gain: if p.rich() { 0.9 } else { 0.55 },
                voice: if p.alive() { Voice::Active } else { Voice::Idle },
            });
        }
        for h in &bg.herb_patches {
            if !near(h.x, h.z) {
                continue;
            }
            out.push(Node {
                cue: Cue::HerbPatch,
                x: h.x,
                z: h.z,
                radius: NODE_R * 0.6,
                gain: 0.5,
                voice: if h.ready() { Voice::Active } else { Voice::Quiet },
            });
        }
        for t in &bg.heal_trees {
            if !near(t.x, t.z) {
                continue;
            }
            // gnawed below full and still standing: it is being worked on
            let hurt = t.alive() && t.hp < 545.0;
            out.push(Node {
                cue: if hurt { Cue::HealTreeCreak } else { Cue::HealTreeHum },
                x: t.x,
                z: t.z,
                radius: NODE_R,
                gain: if hurt { 0.85 } else { 0.5 },
                voice: if t.alive() { if hurt { Voice::Active } else { Voice::Idle } } else { Voice::Quiet },
            });
        }
        for (i, c) in bg.camps.iter().enumerate() {
            if near(c.0, c.1) {
                out.push(Node {
                    cue: Cue::GreatTree,
                    x: c.0,
                    z: c.1,
                    radius: NODE_R,
                    gain: 0.6,
                    voice: if bg.clans[i].great_tree_hp < 1395.0 { Voice::Active } else { Voice::Idle },
                });
            }
        }
        let (sx, sz) = bg.skyrock;
        if near(sx, sz) {
            out.push(Node {
                cue: Cue::Skyrock,
                x: sx,
                z: sz,
                radius: NODE_R,
                gain: 0.55,
                voice: if bg.skyrock_holder.is_some() { Voice::Active } else { Voice::Idle },
            });
        }
        for m in junglemap::mists_ref() {
            if near(m.x, m.z) {
                out.push(Node {
                    cue: Cue::Mistfall,
                    x: m.x,
                    z: m.z,
                    radius: NODE_R,
                    gain: 0.8,
                    voice: Voice::Idle,
                });
            }
        }
        for t in junglemap::thickets_ref() {
            if near(t.x, t.z) {
                out.push(Node {
                    cue: Cue::Thicket,
                    x: t.x,
                    z: t.z,
                    radius: t.r + 6.0,
                    gain: 0.4,
                    voice: Voice::Idle,
                });
            }
        }
        out
    }

    /// Watch the world for the moments worth a one-shot. Called from `update`;
    /// public because a replay viewer may want the cue list without the mix.
    pub fn detect_events(&mut self, bg: &Battleground, side: u8) -> Vec<Cue> {
        let mut fired = Vec::new();
        let trees: Vec<bool> = bg.heal_trees.iter().map(|t| t.alive()).collect();
        if self.started {
            if trees.len() == self.seen_trees.len()
                && trees.iter().zip(&self.seen_trees).any(|(now, was)| *was && !now)
            {
                fired.push(Cue::BorderFell);
            }
            if bg.skyrock_holder != self.seen_skyrock && bg.skyrock_holder.is_some() {
                fired.push(Cue::SkyrockTaken);
            }
            if bg.is_night() && !self.seen_night {
                fired.push(Cue::NightFalls);
            }
            let s = side as usize;
            if bg.clans[s].pile > self.seen_pile[s] {
                fired.push(Cue::PreyTaken);
            }
            if bg.clans[s].herbs > self.seen_herbs[s] {
                fired.push(Cue::HerbGathered);
            }
            let down: Vec<bool> = bg.warriors.iter().map(|w| w.mind == Mind::Dead).collect();
            if down.len() == self.seen_down.len()
                && bg
                    .warriors
                    .iter()
                    .zip(&self.seen_down)
                    .enumerate()
                    .any(|(i, (w, was))| {
                        w.role == Role::Ancestor && !was && down[i] && w.team == side
                    })
            {
                fired.push(Cue::WarriorDown);
            }
            if bg.outcome.is_some() {
                fired.push(Cue::ClanVictory);
            }
        }
        self.seen_trees = trees;
        self.seen_skyrock = bg.skyrock_holder;
        self.seen_night = bg.is_night();
        self.seen_pile = [bg.clans[0].pile, bg.clans[1].pile];
        self.seen_herbs = [bg.clans[0].herbs, bg.clans[1].herbs];
        self.seen_down = bg.warriors.iter().map(|w| w.mind == Mind::Dead).collect();
        self.started = true;
        fired
    }

    /// Advance the soundscape and return what should be audible this frame.
    pub fn update(&mut self, bg: &Battleground, side: u8, ears: Ears, dt: f32) -> Vec<Voiced> {
        // events first: they set the duck the beds will be heard through
        self.fired = self.detect_events(bg, side);
        if !self.fired.is_empty() {
            self.duck = DUCK_SECS;
        }
        self.duck = (self.duck - dt).max(0.0);
        let duck_gain = if self.duck > 0.0 { DUCK_TO } else { 1.0 };

        // beds fade toward their targets rather than switching
        let want = Soundscape::bed_targets(bg, ears);
        for i in 0..BEDS.len() {
            let step = BED_FADE * dt;
            self.bed[i] += (want[i] - self.bed[i]).clamp(-step, step);
        }

        let mut mix: Vec<Voiced> = Vec::new();
        for (i, &c) in BEDS.iter().enumerate() {
            if self.bed[i] > 0.01 {
                mix.push(Voiced { cue: c, gain: self.bed[i] * duck_gain, pan: 0.0 });
            }
        }
        for n in Soundscape::nodes(bg, ears) {
            if n.voice == Voice::Quiet {
                continue;
            }
            let (dx, dz) = (n.x - ears.x, n.z - ears.z);
            let d = (dx * dx + dz * dz).sqrt();
            if d > n.radius {
                continue;
            }
            // linear-ish falloff with a soft knee; an emitter you are standing on
            // is not four times louder than one a stride away
            let fall = (1.0 - d / n.radius).clamp(0.0, 1.0).powf(1.4);
            let quiet = if n.voice == Voice::Idle { 0.6 } else { 1.0 };
            mix.push(Voiced {
                cue: n.cue,
                gain: n.gain * fall * quiet * duck_gain,
                pan: pan_of(ears, n.x, n.z),
            });
        }
        for &cue in &self.fired {
            mix.push(Voiced { cue, gain: 1.0, pan: 0.0 });
        }

        // Never hand a backend a mix that clips. Beds and nodes are scaled
        // together if the sum runs over; events are left alone, because a cue
        // that matters should not be quietened by a busy field.
        let loud: f32 = mix.iter().filter(|v| !v.cue.is_event()).map(|v| v.gain).sum();
        if loud > 1.0 {
            let k = 1.0 / loud;
            for v in mix.iter_mut() {
                if !v.cue.is_event() {
                    v.gain *= k;
                }
            }
        }
        mix.retain(|v| v.gain > 0.005);
        mix
    }
}

/// Where a sound sits between the ears, given which way they point.
fn pan_of(ears: Ears, x: f32, z: f32) -> f32 {
    let (dx, dz) = (x - ears.x, z - ears.z);
    let d = (dx * dx + dz * dz).sqrt().max(1e-4);
    // the listener's right-hand vector
    let (rx, rz) = (-ears.heading.sin(), ears.heading.cos());
    ((dx / d) * rx + (dz / d) * rz).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ancestor::Ancestor;
    use crate::battleground::CLAN_A;

    fn ears_at(bg: &Battleground) -> Ears {
        let w = &bg.warriors[0];
        Ears { x: w.x, z: w.z, heading: w.heading }
    }

    #[test]
    fn beds_fade_rather_than_switch() {
        let mut bg = Battleground::new(Ancestor::Lion);
        let mut s = Soundscape::new();
        let e = ears_at(&bg);
        let first = s.update(&bg, CLAN_A, e, 1.0 / 30.0);
        // One frame in, no BED is at volume — a bed arrives over about a second
        // and a half, not in 33 milliseconds. Nodes are exempt on purpose: an
        // emitter you are standing beside should be audible the moment you are
        // beside it, and fading those in would mean walking into silence.
        for v in first.iter().filter(|v| BEDS.contains(&v.cue)) {
            assert!(v.gain < 0.15, "{:?} came in at {}", v.cue, v.gain);
        }
        assert!(
            first.iter().any(|v| !BEDS.contains(&v.cue) && !v.cue.is_event()),
            "a node near the camp should already be audible: {first:?}"
        );
        for _ in 0..90 {
            bg.step(1.0 / 30.0);
            s.update(&bg, CLAN_A, e, 1.0 / 30.0);
        }
        let settled = s.update(&bg, CLAN_A, e, 1.0 / 30.0);
        let beds: f32 = settled
            .iter()
            .filter(|v| !v.cue.is_event() && matches!(v.cue, Cue::BedForest | Cue::BedMoor | Cue::BedRiver | Cue::BedCrag))
            .map(|v| v.gain)
            .sum();
        assert!(beds > 0.1, "after three seconds something should be audible: {settled:?}");
    }

    #[test]
    fn night_and_leafbare_are_layers_over_the_land() {
        let mut bg = Battleground::new(Ancestor::Lion);
        let e = ears_at(&bg);
        let day = Soundscape::bed_targets(&bg, e);
        assert_eq!(day[4], 0.0, "no night bed at noon");
        while !bg.is_night() {
            bg.step(1.0);
        }
        let night = Soundscape::bed_targets(&bg, e);
        assert!(night[4] > 0.5, "night should bring its own layer");
        // and the land underneath is unchanged — a layer, not a replacement
        assert_eq!(day[0], night[0]);
    }

    #[test]
    fn a_camp_sounds_different_once_its_prey_is_taken() {
        // the signal half: a player should be able to hear that somebody has
        // already cleared a camp
        let mut bg = Battleground::new(Ancestor::Lion);
        let p = bg.prey.iter().position(|p| p.alive()).unwrap();
        let (px, pz) = (bg.prey[p].x, bg.prey[p].z);
        let e = Ears { x: px + 4.0, z: pz, heading: 0.0 };
        let live: Vec<Cue> = Soundscape::nodes(&bg, e).iter().map(|n| n.cue).collect();
        assert!(live.contains(&Cue::CampPrey));
        assert!(!live.contains(&Cue::CampEmpty));
        bg.prey[p].respawn_t = 20.0; // taken
        let taken: Vec<Cue> = Soundscape::nodes(&bg, e).iter().map(|n| n.cue).collect();
        assert!(taken.contains(&Cue::CampEmpty));
        assert!(!taken.contains(&Cue::CampPrey));
    }

    #[test]
    fn a_tree_being_gnawed_can_be_heard_from_a_lane_you_cannot_see() {
        let mut bg = Battleground::new(Ancestor::Lion);
        let t = 0;
        let (tx, tz) = (bg.heal_trees[t].x, bg.heal_trees[t].z);
        let e = Ears { x: tx + 20.0, z: tz, heading: 0.0 };
        let calm: Vec<Cue> = Soundscape::nodes(&bg, e).iter().map(|n| n.cue).collect();
        assert!(calm.contains(&Cue::HealTreeHum));
        bg.heal_trees[t].hp = 300.0;
        let gnawed = Soundscape::nodes(&bg, e);
        let creak = gnawed.iter().find(|n| n.cue == Cue::HealTreeCreak).expect("creaks");
        assert_eq!(creak.voice, Voice::Active);
        assert!(creak.gain > 0.8, "a warning should not be a whisper");
        // and a felled tree says nothing at all
        bg.heal_trees[t].hp = 0.0;
        let dead: Vec<Voice> = Soundscape::nodes(&bg, e)
            .iter()
            .filter(|n| (n.x - tx).abs() < 0.1)
            .map(|n| n.voice)
            .collect();
        assert!(dead.iter().all(|v| *v == Voice::Quiet));
    }

    #[test]
    fn sounds_are_placed_between_the_ears() {
        let e = Ears { x: 0.0, z: 0.0, heading: 0.0 }; // facing +x
        let right = pan_of(e, 0.0, 10.0);
        let left = pan_of(e, 0.0, -10.0);
        let ahead = pan_of(e, 10.0, 0.0);
        assert!(right > 0.9, "to the right: {right}");
        assert!(left < -0.9, "to the left: {left}");
        assert!(ahead.abs() < 0.1, "straight ahead: {ahead}");
        // turning the head moves the world
        let turned = Ears { heading: std::f32::consts::FRAC_PI_2, ..e };
        assert!(pan_of(turned, 0.0, 10.0).abs() < 0.1, "now it is in front");
    }

    #[test]
    fn the_moments_worth_hearing_fire_once_each() {
        let mut bg = Battleground::new(Ancestor::Lion);
        let mut s = Soundscape::new();
        let e = ears_at(&bg);
        // the first update only learns the state; it must not fire a cue for
        // every tree that exists
        let first = s.update(&bg, CLAN_A, e, 1.0 / 30.0);
        assert!(first.iter().all(|v| !v.cue.is_event()), "no events on the first frame");
        // fell a border
        bg.heal_trees[0].hp = 0.0;
        let mix = s.update(&bg, CLAN_A, e, 1.0 / 30.0);
        assert!(mix.iter().any(|v| v.cue == Cue::BorderFell));
        // it does not fire again next frame for the same tree
        let again = s.update(&bg, CLAN_A, e, 1.0 / 30.0);
        assert!(!again.iter().any(|v| v.cue == Cue::BorderFell));
        // a herb delivered is its own small cue
        bg.clans[0].herbs += 1;
        let mix = s.update(&bg, CLAN_A, e, 1.0 / 30.0);
        assert!(mix.iter().any(|v| v.cue == Cue::HerbGathered));
    }

    #[test]
    fn an_event_ducks_the_field_but_is_not_ducked_itself() {
        let mut bg = Battleground::new(Ancestor::Lion);
        let mut s = Soundscape::new();
        let e = ears_at(&bg);
        for _ in 0..120 {
            bg.step(1.0 / 30.0);
            s.update(&bg, CLAN_A, e, 1.0 / 30.0);
        }
        let calm: f32 = s
            .update(&bg, CLAN_A, e, 1.0 / 30.0)
            .iter()
            .filter(|v| !v.cue.is_event())
            .map(|v| v.gain)
            .sum();
        bg.heal_trees[0].hp = 0.0;
        let loud = s.update(&bg, CLAN_A, e, 1.0 / 30.0);
        let ducked: f32 = loud.iter().filter(|v| !v.cue.is_event()).map(|v| v.gain).sum();
        let event = loud.iter().find(|v| v.cue == Cue::BorderFell).unwrap();
        assert!(ducked < calm * 0.75, "the field should drop under a cue: {calm} -> {ducked}");
        assert_eq!(event.gain, 1.0, "and the cue itself is not ducked");
    }

    #[test]
    fn the_mix_never_clips() {
        // a listener standing where everything is audible at once
        let mut bg = Battleground::new(Ancestor::Lion);
        let mut s = Soundscape::new();
        for _ in 0..30 * 60 {
            bg.step(1.0 / 30.0);
        }
        for w in 0..bg.warriors.len().min(6) {
            let e = Ears { x: bg.warriors[w].x, z: bg.warriors[w].z, heading: 0.4 };
            for _ in 0..60 {
                let mix = s.update(&bg, CLAN_A, e, 1.0 / 30.0);
                let bedsum: f32 = mix.iter().filter(|v| !v.cue.is_event()).map(|v| v.gain).sum();
                assert!(bedsum <= 1.001, "looping mix summed to {bedsum}");
                for v in &mix {
                    assert!(v.gain >= 0.0 && v.gain <= 1.0);
                    assert!(v.pan >= -1.0 && v.pan <= 1.0);
                }
            }
        }
    }

    #[test]
    fn carrying_prey_home_is_audible_the_whole_way() {
        // an end-to-end sanity check that the nodes really are attached to the
        // world rather than to the frame: walk from a camp to the camp's owner
        // and the mix should change without anyone telling it to
        let mut bg = Battleground::new(Ancestor::Lion);
        let mut s = Soundscape::new();
        let p = bg.prey.iter().position(|p| p.alive()).unwrap();
        let (px, pz) = (bg.prey[p].x, bg.prey[p].z);
        let home = bg.camps[0];
        let mut seen_camp = false;
        let mut seen_home = false;
        for i in 0..=20 {
            let t = i as f32 / 20.0;
            let e = Ears {
                x: px + (home.0 - px) * t,
                z: pz + (home.1 - pz) * t,
                heading: 0.0,
            };
            let mix = s.update(&bg, CLAN_A, e, 1.0 / 30.0);
            seen_camp |= mix.iter().any(|v| matches!(v.cue, Cue::CampPrey | Cue::CampEmpty));
            seen_home |= mix.iter().any(|v| v.cue == Cue::GreatTree);
        }
        assert!(seen_camp, "the camp should be audible from the camp");
        assert!(seen_home, "and the Great Tree from the camp's doorstep");

    }
}
