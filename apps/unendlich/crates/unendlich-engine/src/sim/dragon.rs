//! The wood dragon — an earth/wood creature whose body is an [`Articulation`] of
//! stick-slip joints (so it creaks and unblocks as it flies, made of tree) and
//! whose breath is a gout of vines: a Wood [`Attack`] the caller resolves through
//! `combat` (mitigated by shield/armor, inflicting a Wood affliction). The brain
//! is an aerial state machine — it slumbers until roused, circles, swoops, breathes
//! vines, then banks away to recover — and grows more aggressive as its health
//! falls (phases). Returns intents; the caller moves the body and applies the
//! breath, keeping this pure and deterministic for the rollback sim.

use crate::sim::articulation::Articulation;
use crate::sim::combat::Attack;
use crate::sim::combat_reclaim;
use crate::sim::head_scan;
use crate::sim::roar_sample;
use crate::sim::dragon_archetype::{self, Archetype, Build, Genome};
use crate::sim::dragon_attunement;
use crate::sim::elemental::Element;
use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::joint::JointMaterial;

/// Health-driven aggression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Full-ish health: slow, long cooldowns, weak breath.
    Slumbering,
    Roused,
    /// Low health: fast, short cooldowns, fierce breath.
    Frenzied,
}

/// Aerial behaviour state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Roost,
    Circle,
    Swoop,
    Breathe,
    Bank,
}

/// What the dragon wants to do this tick.
#[derive(Debug, Clone, Copy)]
pub struct DragonIntent {
    /// Direction it wants to move (un-normalised; caller scales by speed).
    pub move_dir: Vec3,
    /// A vine-breath attack to resolve against the target, if breathing now.
    pub breath: Option<Attack>,
    pub state: State,
}

/// One captured moment of a recurrent dragon's combat state, for the
/// time-fold (`combat_reclaim`). `pos`/`hp` are the real values to restore;
/// `age_ticks` ages each tick and is converted to seconds for the decision.
#[derive(Debug, Clone, Copy)]
pub struct DragonSnap {
    pub age_ticks: i32,
    pub hp: i32,
    pub pos: Vec3,
    pub anchor: bool,
}

/// ~30 s of fold window at 60 Hz (matches `combat_reclaim::MAX_FOLD_AGE`).
const RECLAIM_WINDOW_TICKS: i32 = 1800;
/// Capture a snapshot every N ticks (~4 Hz) — bounds the ring (~120 entries).
const SNAP_INTERVAL_TICKS: i32 = 15;
/// Tick rate used to convert snapshot age to the seconds `combat_reclaim` wants.
const TICKS_PER_SEC: f32 = 60.0;

#[derive(Debug, Clone)]
pub struct Dragon {
    pub pos: Vec3,
    pub vel: Vec3,
    pub health: i32,
    pub max_health: i32,
    pub state: State,
    pub body: Articulation,
    pub state_timer: i32,
    pub breath_cooldown: i32,
    /// Genome-derived archetype, when this dragon was built from a genome
    /// (`from_genome`). `None` for the legacy fixed body.
    pub archetype: Option<Archetype>,
    /// How strongly this dragon's breath tracks its element's field (0..=1);
    /// drives `breath_scale`. Defaults to 0.5 for a genome-less dragon.
    pub attunement01: f32,
    /// The morphology genome this dragon was built from (`from_genome`), kept so
    /// genome-derived traits (roar acoustics, etc.) can be sampled on demand.
    pub genome: Option<Genome>,
    /// Kaironyx lineage: when set, the dragon snapshots its combat state and
    /// folds to a favourable past one on heavy damage (`combat_reclaim`).
    pub recurrent: bool,
    /// Rolling window of self-state snapshots (only populated when `recurrent`).
    pub reclaim_ring: Vec<DragonSnap>,
    /// Ticks since the last snapshot capture.
    snap_tick: i32,
}

/// Per-phase tuning: (circle_ticks, breath_magnitude, cooldown_ticks).
fn phase_params(p: Phase) -> (i32, u16, i32) {
    match p {
        Phase::Slumbering => (20, 10, 40),
        Phase::Roused => (12, 16, 24),
        Phase::Frenzied => (6, 24, 10),
    }
}

fn h_dist2(a: Vec3, b: Vec3) -> Fixed {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    dx * dx + dz * dz
}

/// Map an archetype element index (`0=Wood..5=Storm`, the `elemental_multiplier`
/// palette) to the attunement element enum.
fn element_of(e: u8) -> dragon_attunement::Element {
    use dragon_attunement::Element::*;
    match e {
        0 => Wood,
        1 => Water,
        2 => Air,
        3 => Dust,
        4 => Fire,
        _ => Storm,
    }
}

impl Dragon {
    /// A small branching body of woody (rusted) joints.
    fn build_body() -> Articulation {
        let mut a = Articulation::new();
        let m = JointMaterial::rusted();
        let len = Fixed::from_int(2);
        let root = a.add(None, len, m);
        let mid = a.add(Some(root), len, m);
        a.add(Some(mid), len, m); // tail
        a.add(Some(mid), len, m); // wing/branch
        a.add(Some(root), len, m); // neck/branch
        a
    }

    /// Body plan shaped by the genome's [`Build`]: a Serpentine dragon is a long
    /// segmented chain, a Bruiser is squat with stubby limbs, a Flyer has broad
    /// wing branches, a Stalker is lithe and long-legged, a Sovereign is balanced
    /// with a crest branch. Same joint primitives as `build_body`, different plan.
    fn build_body_for(build: Build) -> Articulation {
        let mut a = Articulation::new();
        let m = JointMaterial::rusted();
        let short = Fixed::from_int(1);
        let unit = Fixed::from_int(2);
        let long = Fixed::from_int(3);
        match build {
            Build::Serpentine => {
                // Long segmented spine: neck + body + tail in one chain.
                let mut prev = a.add(None, long, m);
                for _ in 0..5 {
                    prev = a.add(Some(prev), long, m);
                }
            }
            Build::Bruiser => {
                // Squat: short thick root with four stubby limbs.
                let root = a.add(None, short, m);
                for _ in 0..4 {
                    a.add(Some(root), short, m);
                }
            }
            Build::Flyer => {
                // Light spine, two broad wing branches, a tail.
                let root = a.add(None, unit, m);
                let mid = a.add(Some(root), unit, m);
                a.add(Some(mid), long, m); // wing
                a.add(Some(mid), long, m); // wing
                a.add(Some(mid), unit, m); // tail
            }
            Build::Stalker => {
                // Lithe spine with four long legs.
                let root = a.add(None, unit, m);
                let mid = a.add(Some(root), unit, m);
                for _ in 0..4 {
                    a.add(Some(mid), long, m);
                }
            }
            Build::Sovereign => {
                // Balanced quadruped + neck + wing + a crest branch.
                let root = a.add(None, unit, m);
                let mid = a.add(Some(root), unit, m);
                a.add(Some(mid), unit, m); // tail
                a.add(Some(mid), unit, m); // wing
                a.add(Some(root), unit, m); // neck
                a.add(Some(root), short, m); // crest
            }
        }
        a
    }

    pub fn new(pos: Vec3, max_health: i32) -> Self {
        Dragon {
            pos,
            vel: Vec3::ZERO,
            health: max_health,
            max_health,
            state: State::Roost,
            body: Self::build_body(),
            state_timer: 0,
            breath_cooldown: 0,
            archetype: None,
            attunement01: 0.5,
            genome: None,
            recurrent: false,
            reclaim_ring: Vec::new(),
            snap_tick: 0,
        }
    }

    /// Build a dragon whose BODY PLAN comes from its genome: the genome is
    /// classified into an [`Archetype`] (`dragon_archetype`), the body is shaped
    /// to that archetype's [`Build`], and the breath-attunement is taken from how
    /// dominant the chosen element is in the genome. This is the wiring from the
    /// `dragon_forge_3d` viewer space to the live entity body.
    pub fn from_genome(pos: Vec3, max_health: i32, g: &Genome) -> Self {
        let arch = dragon_archetype::classify(g);
        let body = Self::build_body_for(arch.build);
        // Attunement proxy: strength of the dominant element gene (0..=1).
        let dom = g.element[arch.element as usize % g.element.len()];
        let attunement01 = if dom.is_finite() { dom.clamp(0.0, 1.0) } else { 0.0 };
        Dragon {
            pos,
            vel: Vec3::ZERO,
            health: max_health,
            max_health,
            state: State::Roost,
            body,
            state_timer: 0,
            breath_cooldown: 0,
            archetype: Some(arch),
            attunement01,
            genome: Some(*g),
            recurrent: false,
            reclaim_ring: Vec::new(),
            snap_tick: 0,
        }
    }

    /// Mark this dragon as a Recurrent (Kaironyx lineage): it will snapshot its
    /// combat state and fold to a favourable past one on heavy damage. Builder
    /// style so a spawner can write `Dragon::from_genome(..).make_recurrent()`.
    pub fn make_recurrent(mut self) -> Self {
        self.recurrent = true;
        self
    }

    /// How alert the dragon is (0..1), driving how widely it scans. Rises as it
    /// rouses through its health phases.
    pub fn alertness(&self) -> f32 {
        match self.phase() {
            Phase::Slumbering => 0.15,
            Phase::Roused => 0.6,
            Phase::Frenzied => 1.0,
        }
    }

    /// Head/neck scan pose at time `t` (seconds). Sweeps wider and faster when
    /// alert, raising effective vision coverage (`head_scan::vision_gain`).
    pub fn head_scan(&self, t: f32) -> head_scan::ScanState {
        head_scan::scan(t, self.alertness(), 0.0)
    }

    /// Effective vision multiplier from the current scanning behaviour.
    pub fn vision_gain(&self) -> f32 {
        head_scan::vision_gain(self.alertness())
    }

    /// Sample this dragon's roar from its genome (`None` for a genome-less
    /// dragon).
    pub fn roar(&self) -> Option<roar_sample::Roar> {
        self.genome.as_ref().map(roar_sample::roar)
    }

    /// Capture one tick of self-state into the fold ring (no-op unless
    /// `recurrent`). Ages existing snapshots, evicts those past the 30 s window,
    /// and captures a fresh one every `SNAP_INTERVAL_TICKS`.
    fn record_snapshot(&mut self) {
        if !self.recurrent {
            return;
        }
        for s in &mut self.reclaim_ring {
            s.age_ticks += 1;
        }
        self.reclaim_ring.retain(|s| s.age_ticks <= RECLAIM_WINDOW_TICKS);
        self.snap_tick += 1;
        if self.snap_tick >= SNAP_INTERVAL_TICKS {
            self.snap_tick = 0;
            self.reclaim_ring.push(DragonSnap {
                age_ticks: 0,
                hp: self.health,
                pos: self.pos,
                anchor: false,
            });
        }
    }

    /// Given the damage just taken, ask `combat_reclaim` whether to fold and to
    /// which past snapshot; on a fold, restore HP + position from it and drop a
    /// Tier-0 anchor so it can't rewind past this moment again. Returns whether
    /// it folded.
    fn try_fold(&mut self, dmg: i32) -> bool {
        if !self.recurrent || self.reclaim_ring.is_empty() || self.max_health <= 0 {
            return false;
        }
        let pressure = dmg.max(0) as f32 / self.max_health as f32;
        let norm = |hp: i32| (hp as f32 / self.max_health as f32).clamp(0.0, 1.0);
        // snaps[0] is the CURRENT (post-damage) state at t=0 — the baseline
        // `combat_reclaim` compares against; the ring entries are the pasts it
        // may fold back to.
        let mut snaps: Vec<combat_reclaim::Snapshot> = Vec::with_capacity(self.reclaim_ring.len() + 1);
        snaps.push(combat_reclaim::Snapshot {
            t: 0.0,
            hp: norm(self.health),
            pos_score: self.pos.y.to_f32(),
            anchor: false,
        });
        for s in &self.reclaim_ring {
            snaps.push(combat_reclaim::Snapshot {
                // strictly older than "now" so ordering is unambiguous
                t: (s.age_ticks as f32 / TICKS_PER_SEC).max(0.001),
                hp: norm(s.hp),
                pos_score: s.pos.y.to_f32(),
                anchor: s.anchor,
            });
        }
        let d = combat_reclaim::decide(&snaps, pressure, 1.0, 0.25);
        if let (true, Some(i)) = (d.fold, d.index) {
            if i == 0 {
                return false; // chose "now"; nothing to rewind to
            }
            let snap = self.reclaim_ring[i - 1];
            self.health = snap.hp;
            self.pos = snap.pos;
            // Anchor "now": the next fold cannot rewind past this point.
            self.reclaim_ring.clear();
            self.reclaim_ring.push(DragonSnap {
                age_ticks: 0,
                hp: self.health,
                pos: self.pos,
                anchor: true,
            });
            self.snap_tick = 0;
            true
        } else {
            false
        }
    }

    /// Breath power multiplier for the current environment, via
    /// `dragon_attunement`. A high-attunement dragon spikes in a matched field
    /// and is muted in an opposed one; a genome-less dragon is neutral (1.0).
    /// The combat layer multiplies the breath magnitude by this.
    pub fn breath_scale(&self, fields: &dragon_attunement::Fields) -> f32 {
        match self.archetype {
            Some(a) => {
                dragon_attunement::attune(element_of(a.element), self.attunement01, fields).scale
            }
            None => 1.0,
        }
    }

    pub fn phase(&self) -> Phase {
        // thirds of max health
        let third = self.max_health / 3;
        if self.health > 2 * third {
            Phase::Slumbering
        } else if self.health > third {
            Phase::Roused
        } else {
            Phase::Frenzied
        }
    }

    pub fn take_damage(&mut self, dmg: i32) {
        self.health = (self.health - dmg.max(0)).max(0);
        // A Recurrent dragon may fold the last ~30 s away to escape a heavy hit.
        if self.recurrent {
            self.try_fold(dmg);
        }
    }

    pub fn is_dead(&self) -> bool {
        self.health <= 0
    }

    /// Advance the brain one tick toward `target`. Drives the body so it animates,
    /// and returns the movement + breath intent.
    pub fn update(&mut self, target: Vec3, dt: Fixed) -> DragonIntent {
        // Recurrent dragons capture their combat state for the time-fold.
        self.record_snapshot();
        let (circle_ticks, breath_mag, cooldown) = phase_params(self.phase());
        let wake = Fixed::from_int(2500); // ~50 units horizontal
        let strike = Fixed::from_int(400); // ~20 units
        let d2 = h_dist2(self.pos, target);

        let dx = target.x - self.pos.x;
        let dy = target.y - self.pos.y;
        let dz = target.z - self.pos.z;
        let up = Fixed::from_int(3);

        if self.breath_cooldown > 0 {
            self.breath_cooldown -= 1;
        }

        let mut breath = None;
        let mut body_drive = Fixed::from_int(8);

        let move_dir = match self.state {
            State::Roost => {
                if d2 < wake {
                    self.state = State::Circle;
                    self.state_timer = circle_ticks;
                }
                Vec3::ZERO
            }
            State::Circle => {
                self.state_timer -= 1;
                if self.state_timer <= 0 && self.breath_cooldown <= 0 {
                    self.state = State::Swoop;
                }
                // Orbit: perpendicular to the target in the horizontal plane.
                Vec3::new(dz, Fixed::ZERO, Fixed::ZERO - dx)
            }
            State::Swoop => {
                body_drive = Fixed::from_int(20);
                if d2 < strike {
                    self.state = State::Breathe;
                    self.state_timer = 4;
                }
                Vec3::new(dx, dy, dz) // dive at the target
            }
            State::Breathe => {
                body_drive = Fixed::from_int(20);
                self.state_timer -= 1;
                breath = Some(Attack {
                    element: Element::Wood,
                    magnitude: breath_mag,
                    affliction_ticks: 6,
                });
                if self.state_timer <= 0 {
                    self.state = State::Bank;
                    self.state_timer = 8;
                    self.breath_cooldown = cooldown;
                }
                Vec3::new(dx, Fixed::ZERO, dz) // hold facing while breathing
            }
            State::Bank => {
                self.state_timer -= 1;
                if self.state_timer <= 0 {
                    self.state = State::Circle;
                    self.state_timer = circle_ticks;
                }
                // Pull away and climb to recover.
                Vec3::new(Fixed::ZERO - dx, up, Fixed::ZERO - dz)
            }
        };

        // The woody body unblocks/animates as it acts.
        self.body.drive_all(body_drive, dt);

        DragonIntent { move_dir, breath, state: self.state }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt() -> Fixed {
        Fixed::from_frac(1, 10)
    }

    fn at(x: i64, y: i64, z: i64) -> Vec3 {
        Vec3::new(Fixed::from_int(x), Fixed::from_int(y), Fixed::from_int(z))
    }

    #[test]
    fn starts_slumbering_at_full_health_and_frenzies_as_it_falls() {
        let mut d = Dragon::new(at(0, 30, 0), 300);
        assert_eq!(d.phase(), Phase::Slumbering);
        d.take_damage(120); // 180 left → between thirds
        assert_eq!(d.phase(), Phase::Roused);
        d.take_damage(120); // 60 left → < third
        assert_eq!(d.phase(), Phase::Frenzied);
    }

    #[test]
    fn it_stays_roosting_until_the_target_is_near() {
        let mut d = Dragon::new(at(0, 30, 0), 300);
        let far = at(500, 30, 500); // way past wake radius
        let intent = d.update(far, dt());
        assert_eq!(intent.state, State::Roost);
        assert_eq!(intent.move_dir, Vec3::ZERO, "asleep, holding position");
    }

    #[test]
    fn a_near_target_rouses_it_into_circling() {
        let mut d = Dragon::new(at(0, 30, 0), 300);
        let near = at(10, 25, 0); // within wake
        d.update(near, dt());
        assert_eq!(d.state, State::Circle, "woke and began to circle");
    }

    #[test]
    fn it_eventually_swoops_and_breathes_vines() {
        let mut d = Dragon::new(at(0, 30, 0), 90); // frenzied: short circle
        let near = at(2, 29, 0); // inside strike range
        let mut breathed = false;
        let mut element = None;
        for _ in 0..60 {
            let intent = d.update(near, dt());
            if let Some(atk) = intent.breath {
                breathed = true;
                element = Some(atk.element);
                break;
            }
        }
        assert!(breathed, "it closed in and breathed");
        assert_eq!(element, Some(Element::Wood), "vines are a Wood attack");
    }

    #[test]
    fn breath_is_fiercer_when_frenzied_than_slumbering() {
        let slumber = phase_params(Phase::Slumbering).1;
        let frenzy = phase_params(Phase::Frenzied).1;
        assert!(frenzy > slumber, "low-health frenzy breathes harder: {frenzy} > {slumber}");
    }

    #[test]
    fn after_a_breath_bout_it_banks_and_goes_on_cooldown() {
        let mut d = Dragon::new(at(0, 30, 0), 90);
        let near = at(2, 29, 0);
        // Run until it has been through a breathe bout into bank.
        let mut saw_bank = false;
        for _ in 0..80 {
            d.update(near, dt());
            if d.state == State::Bank {
                saw_bank = true;
                assert!(d.breath_cooldown > 0, "cooling down after breathing");
                break;
            }
        }
        assert!(saw_bank, "it disengaged after breathing");
    }

    #[test]
    fn the_body_animates_as_it_acts() {
        let mut d = Dragon::new(at(0, 30, 0), 90);
        let near = at(2, 29, 0);
        for _ in 0..40 {
            d.update(near, dt());
        }
        assert!(d.body.total_curl() > Fixed::ZERO, "the woody body unblocked and moved");
    }

    #[test]
    fn the_brain_is_deterministic() {
        let run = || {
            let mut d = Dragon::new(at(0, 30, 0), 90);
            let near = at(2, 29, 0);
            for _ in 0..50 {
                d.update(near, dt());
            }
            (d.state, d.health, d.body.total_curl())
        };
        assert_eq!(run(), run());
    }
}

#[cfg(test)]
mod genome_body_tests {
    use super::Dragon;
    use crate::sim::dragon_archetype::{Build, Genome};
    use crate::sim::dragon_attunement::Fields;
    use crate::sim::fixed::Vec3;

    /// Genome with everything zero except the named overrides.
    fn genome(set: impl Fn(&mut Genome)) -> Genome {
        let mut g = Genome {
            bulk: 0.0, neck_len: 0.0, neck_arch: 0.0, tail_len: 0.0, leg_len: 0.0,
            wing_span: 0.0, wing_spread: 0.0, pose_bias: 0.0,
            muzzle_len: 0.0, jaw_depth: 0.0, snout_curve: 0.0, brow: 0.0,
            cheek: 0.0, teeth: 0.0, crest: 0.0, horns: 0.0, horn_sweep: 0.0,
            element: [0.0; 6],
        };
        set(&mut g);
        g
    }

    #[test]
    fn genomeless_dragon_is_neutral() {
        let d = Dragon::new(Vec3::ZERO, 100);
        assert!(d.archetype.is_none());
        assert_eq!(d.breath_scale(&Fields::zero()), 1.0);
        assert_eq!(d.breath_scale(&Fields::full()), 1.0);
    }

    #[test]
    fn from_genome_assigns_an_archetype() {
        let g = genome(|g| { g.wing_span = 0.9; g.wing_spread = 0.9; g.element[4] = 1.0; });
        let d = Dragon::from_genome(Vec3::ZERO, 100, &g);
        assert!(d.archetype.is_some());
    }

    #[test]
    fn distinct_genomes_distinct_builds() {
        // Strongly winged/light vs strongly bulky/short — should not classify the same.
        let flyer = genome(|g| { g.wing_span = 1.0; g.wing_spread = 1.0; g.bulk = 0.0; g.leg_len = 0.1; g.element[4] = 1.0; });
        let bruiser = genome(|g| { g.bulk = 1.0; g.neck_len = 0.0; g.wing_span = 0.0; g.element[4] = 1.0; });
        let bf = Dragon::from_genome(Vec3::ZERO, 100, &flyer).archetype.unwrap().build;
        let bb = Dragon::from_genome(Vec3::ZERO, 100, &bruiser).archetype.unwrap().build;
        assert_ne!(bf.index(), bb.index(), "extreme winged vs bulky genomes should differ in build");
    }

    #[test]
    fn each_build_yields_a_nonempty_body() {
        for b in [Build::Serpentine, Build::Bruiser, Build::Flyer, Build::Stalker, Build::Sovereign] {
            let body = Dragon::build_body_for(b);
            assert!(body.len() > 0, "{:?} should produce joints", b);
        }
    }

    #[test]
    fn fire_dragon_breath_scales_with_heat() {
        // A Fire-dominant, fully-attuned dragon: hot field should not be weaker than no field.
        let g = genome(|g| { g.element[4] = 1.0; g.bulk = 0.6; });
        let d = Dragon::from_genome(Vec3::ZERO, 100, &g);
        let cold = d.breath_scale(&Fields::zero());
        let hot = d.breath_scale(&Fields { r: 1.0, ..Fields::zero() });
        assert!(hot >= cold, "fire breath should scale up (or hold) with heat: hot={hot} cold={cold}");
        assert!(hot.is_finite() && hot > 0.0);
    }

    #[test]
    fn from_genome_is_deterministic() {
        let g = genome(|g| { g.wing_span = 0.7; g.bulk = 0.3; g.element[1] = 0.8; });
        let a = Dragon::from_genome(Vec3::ZERO, 100, &g).archetype.unwrap();
        let b = Dragon::from_genome(Vec3::ZERO, 100, &g).archetype.unwrap();
        assert_eq!(a, b);
    }
}

#[cfg(test)]
mod reclaim_tests {
    use super::*;

    fn pos(x: i64, y: i64, z: i64) -> Vec3 {
        Vec3::new(Fixed::from_int(x), Fixed::from_int(y), Fixed::from_int(z))
    }

    // Tick the brain enough times to capture at least one fold snapshot.
    fn warm_snapshots(d: &mut Dragon, ticks: i32) {
        let dt = Fixed::from_int(0);
        for _ in 0..ticks {
            let _ = d.update(Vec3::ZERO, dt);
        }
    }

    #[test]
    fn recurrent_dragon_folds_back_from_heavy_hit() {
        let mut d = Dragon::new(pos(0, 30, 0), 1000).make_recurrent();
        warm_snapshots(&mut d, SNAP_INTERVAL_TICKS * 3); // a few full-HP snapshots
        assert!(!d.reclaim_ring.is_empty(), "snapshots should accumulate");
        d.take_damage(950); // pressure 0.95 >> FOLD_THRESHOLD
        assert!(d.health > 600, "fold should restore HP from a healthy past, got {}", d.health);
    }

    #[test]
    fn non_recurrent_dragon_just_takes_damage() {
        let mut d = Dragon::new(pos(0, 30, 0), 1000);
        warm_snapshots(&mut d, SNAP_INTERVAL_TICKS * 3);
        assert!(d.reclaim_ring.is_empty(), "non-recurrent captures nothing");
        d.take_damage(700);
        assert_eq!(d.health, 300, "no fold, plain subtraction");
    }

    #[test]
    fn light_hit_does_not_fold() {
        let mut d = Dragon::new(pos(0, 30, 0), 1000).make_recurrent();
        warm_snapshots(&mut d, SNAP_INTERVAL_TICKS * 3);
        d.take_damage(100); // pressure 0.1 < FOLD_THRESHOLD
        assert_eq!(d.health, 900, "below pressure threshold, no fold");
    }

    #[test]
    fn fold_drops_anchor_and_resets_ring() {
        let mut d = Dragon::new(pos(0, 30, 0), 1000).make_recurrent();
        warm_snapshots(&mut d, SNAP_INTERVAL_TICKS * 4);
        d.take_damage(950);
        // After a fold the ring is reset to a single Tier-0 anchor at "now".
        assert_eq!(d.reclaim_ring.len(), 1);
        assert!(d.reclaim_ring[0].anchor, "post-fold snapshot must be an anchor");
    }

    #[test]
    fn fold_is_deterministic() {
        let run = || {
            let mut d = Dragon::new(pos(0, 25, 0), 800).make_recurrent();
            warm_snapshots(&mut d, SNAP_INTERVAL_TICKS * 3);
            d.take_damage(780);
            d.health
        };
        assert_eq!(run(), run());
    }
}

#[cfg(test)]
mod motion_tests {
    use super::*;

    fn at(x: i64, y: i64, z: i64) -> Vec3 {
        Vec3::new(Fixed::from_int(x), Fixed::from_int(y), Fixed::from_int(z))
    }

    #[test]
    fn genome_dragon_can_roar() {
        let g = crate::sim::dragon_archetype::Genome::from_seed(42);
        let d = Dragon::from_genome(at(0, 30, 0), 1000, &g);
        let r = d.roar().expect("genome dragon roars");
        assert!(r.pitch_hz > 0.0 && r.duration_s > 0.0);
    }

    #[test]
    fn genomeless_dragon_has_no_roar() {
        let d = Dragon::new(at(0, 30, 0), 300);
        assert!(d.roar().is_none());
    }

    #[test]
    fn alertness_and_vision_rise_as_health_drops() {
        let g = crate::sim::dragon_archetype::Genome::from_seed(1);
        let mut d = Dragon::from_genome(at(0, 30, 0), 900, &g); // full → Slumbering
        let calm = d.alertness();
        let calm_vision = d.vision_gain();
        d.health = 100; // Frenzied
        assert!(d.alertness() > calm);
        assert!(d.vision_gain() > calm_vision);
    }

    #[test]
    fn head_scan_is_deterministic() {
        let d = Dragon::new(at(0, 30, 0), 300);
        assert_eq!(d.head_scan(1.5), d.head_scan(1.5));
    }
}

#[cfg(test)]
mod versus_tests {
    use super::*;

    #[derive(Default, Debug)]
    struct Tally {
        breaths: u32,
        swoop_enter: u32,
        breathe_enter: u32,
        bank_enter: u32,
        circle_enter: u32,
    }

    fn step_pos(p: Vec3, dir: Vec3, speed: f32) -> Vec3 {
        let (x, y, z) = (dir.x.to_f32(), dir.y.to_f32(), dir.z.to_f32());
        let len = (x * x + y * y + z * z).sqrt();
        if len < 1e-3 {
            return p;
        }
        let k = (speed / len) as f64;
        Vec3::new(
            p.x + Fixed::from_f64(x as f64 * k),
            p.y + Fixed::from_f64(y as f64 * k),
            p.z + Fixed::from_f64(z as f64 * k),
        )
    }

    fn note_enter(t: &mut Tally, prev: State, now: State) {
        if prev != now {
            match now {
                State::Swoop => t.swoop_enter += 1,
                State::Breathe => t.breathe_enter += 1,
                State::Bank => t.bank_enter += 1,
                State::Circle => t.circle_enter += 1,
                State::Roost => {}
            }
        }
    }

    #[test]
    fn two_dragons_fight_and_use_abilities() {
        let g = crate::sim::dragon_archetype::Genome::from_seed(1234);
        let mut a = Dragon::from_genome(
            Vec3::new(Fixed::from_int(-20), Fixed::from_int(30), Fixed::ZERO), 600, &g);
        let mut b = Dragon::from_genome(
            Vec3::new(Fixed::from_int(20), Fixed::from_int(30), Fixed::ZERO), 600, &g);
        let dt = Fixed::from_frac(1, 60);
        let strike = Fixed::from_int(400); // matches the FSM's strike range
        let (mut ta, mut tb) = (Tally::default(), Tally::default());
        let (mut pa, mut pb) = (a.state, b.state);

        let mut ticks = 0u32;
        for _ in 0..3000 {
            ticks += 1;
            let ia = a.update(b.pos, dt);
            let ib = b.update(a.pos, dt);

            if let Some(atk) = &ia.breath {
                ta.breaths += 1;
                if h_dist2(a.pos, b.pos) < strike {
                    b.take_damage(atk.magnitude as i32);
                }
            }
            if let Some(atk) = &ib.breath {
                tb.breaths += 1;
                if h_dist2(b.pos, a.pos) < strike {
                    a.take_damage(atk.magnitude as i32);
                }
            }
            note_enter(&mut ta, pa, a.state);
            note_enter(&mut tb, pb, b.state);
            pa = a.state;
            pb = b.state;

            a.pos = step_pos(a.pos, ia.move_dir, 1.5);
            b.pos = step_pos(b.pos, ib.move_dir, 1.5);

            if a.is_dead() || b.is_dead() {
                break;
            }
        }

        eprintln!("=== dragon vs dragon ({ticks} ticks) ===");
        eprintln!("A hp={:>4} {:?}", a.health, ta);
        eprintln!("B hp={:>4} {:?}", b.health, tb);
        eprintln!("winner: {}", if a.is_dead() { "B" } else if b.is_dead() { "A" } else { "draw" });

        let total_breaths = ta.breaths + tb.breaths;
        let total_swoops = ta.swoop_enter + tb.swoop_enter;
        assert!(total_breaths > 0, "dragons must use the breath ability at least once");
        assert!(total_swoops > 0, "dragons must enter swoop attacks");
    }
}

// ── S1 (r94): register as a sim::service_registry service (co-located) ───────
// Thin adapter mapping ServiceCtx(dt) onto Dragon::update with NEUTRAL inputs:
// no chase target (origin) and dt mapped to Fixed at ms precision (clamped).
// Real target/world signals are a later wiring pass. Vec3/Fixed are already in
// scope (use crate::sim::fixed::{Fixed, Vec3} above).
struct DragonService(Dragon);
impl crate::sim::service_registry::Service for DragonService {
    fn name(&self) -> &'static str { "dragon" }
    fn tick(&mut self, ctx: &crate::sim::service_registry::ServiceCtx) {
        let dt = if ctx.dt.is_finite() { ctx.dt.clamp(0.0, 1.0) } else { 0.0 };
        let _ = self.0.update(Vec3::ZERO, Fixed::from_frac((dt * 1000.0) as i64, 1000));
    }
}
#[linkme::distributed_slice(crate::sim::service_registry::SERVICES)]
static DRAGON_SERVICE: crate::sim::service_registry::ServiceEntry =
    crate::sim::service_registry::ServiceEntry {
        name: "dragon",
        make: || Box::new(DragonService(Dragon::new(Vec3::ZERO, 100))),
    };

#[cfg(test)]
mod s1_wiring_tests {
    use crate::sim::service_registry::{Service, ServiceCtx};
    #[test]
    fn dragon_service_ticks_panic_free() {
        let mut s = super::DragonService(super::Dragon::new(super::Vec3::ZERO, 100));
        let ctx = ServiceCtx { dt: 1.0 / 60.0 };
        for _ in 0..240 { s.tick(&ctx); } // ~4s of frames
        assert_eq!(s.name(), "dragon");
    }
}
