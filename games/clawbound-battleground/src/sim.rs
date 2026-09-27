// Simulation state: entities, economy, damage pipeline, win conditions.

use crate::consts::*;
use crate::gods::{self, GodDef};
use crate::items::{self, StackKind};
use glam::Vec3;

pub use crate::consts::{Lane, Team};

// --- tiny deterministic RNG --------------------------------------------------

#[derive(Clone)]
pub struct Rng(pub u64);
impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }
    pub fn next_u(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn f32(&mut self) -> f32 {
        (self.next_u() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f32()
    }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[(self.next_u() % v.len() as u64) as usize]
    }
}

// --- events ------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Event {
    Cast { god: usize, slot: u32, aim: Vec3 },
    /// A cast began its wind-up (`abilities::cast_timing`); it fires as a
    /// `Cast` after `dur` seconds unless interrupted. For the anticipation
    /// pose and anyone watching.
    CastStart { god: usize, slot: u32, aim: Vec3, dur: f32 },
    /// The attack button pressed with nothing in range: an acknowledgement,
    /// not an attack (the cat's shoulder shuffle).
    Whiff { god: usize },
    Damage { pos: Vec3, amount: f32, big: bool },
    Kill { killer: String, victim: String, victim_team: Team, is_god: bool },
    LevelUp { pos: Vec3 },
    Explosion { pos: Vec3, radius: f32, color: [f32; 3] },
    BoltFx { from: Vec3, to: Vec3, color: [f32; 3], thick: f32 },
    /// A storm's lightning strike landing at `at` (a storm zone's strikes:
    /// jagged, forked, a blinding light that throws shadows).
    StrikeFx { at: Vec3, color: [f32; 3] },
    BeamFx { from: Vec3, dir: Vec3, length: f32, width: f32, color: [f32; 3], dur: f32 },
    ShieldFx { pos: Vec3, dur: f32, color: [f32; 3] },
    DashFx { pos: Vec3, dir: Vec3, color: [f32; 3] },
    RingTelegraph { pos: Vec3, radius: f32, delay: f32, color: [f32; 3] },
    PullFx { pos: Vec3, radius: f32, color: [f32; 3] },
    HealFx { pos: Vec3, radius: f32 },
    NovaFx { pos: Vec3, radius: f32, color: [f32; 3] },
    /// A star the flying Moonwyrm let fall has landed (`wyrmflight`).
    StarfallFx { at: Vec3, radius: f32 },
    StormFx { pos: Vec3, radius: f32, color: [f32; 3], dur: f32 },
    TowerDown { pos: Vec3, team: Team },
    TitanVulnerable { team: Team },
    Announce { text: String },
    WeatherChanged { kind: crate::weather::WeatherKind },
    MapEventStart { kind: crate::map_events::MapEventKind, pos: Vec3, radius: f32 },
    MapEventEnd { kind: crate::map_events::MapEventKind },
    PreyCaught { pos: Vec3, kind: crate::wild::PreyKind, gold: f32 },
    BossSpawn { kind: crate::bosses::BossKind, pos: Vec3 },
    DisasterWarning { kind: crate::disasters::DisasterKind },
    DisasterStart { kind: crate::disasters::DisasterKind },
    DisasterEnd { kind: crate::disasters::DisasterKind },
    FireIgnite { pos: Vec3 },
    FissureOpen { a: Vec3, b: Vec3 },
    TornadoDebris { pos: Vec3 },
    BossDown { kind: crate::bosses::BossKind, pos: Vec3, team: Option<Team> },
    /// One of Sproutkit's litter popped (a nip, or its time ran out).
    KitlingPop { pos: Vec3 },
    /// Sproutkit's laugh went round (`kit`).
    GiggleFx { pos: Vec3, radius: f32 },
    /// A stalker's footfall (Hollowstep or another of him, `stalker`):
    /// heard far off.
    HeavyStep { pos: Vec3 },
    /// Cat `god`'s heart, pounding in Hollowstep's terror radius; `fear`
    /// 0..=1.
    Heartbeat { god: usize, fear: f32 },
    /// Hollowstep stepped between: from where he was to where his other was.
    SwapFx { from: Vec3, to: Vec3 },
    /// An echo of him rose (Every Shadow).
    EchoRise { pos: Vec3, dmg: f32 },
    /// Brasshowl raised the lamp (`lantern`).
    LanternFx { pos: Vec3, radius: f32 },
    /// A shadow of Hollowstep's was driven out of the light.
    ShadowDriven { pos: Vec3 },
    /// A Sentinel's coat changed (`sentinel`): to what, and how far its
    /// field reaches.
    CoatShift { god: usize, pos: Vec3, material: crate::gods::Material, radius: f32 },
    /// A Sentinel's footfall.
    Tread { pos: Vec3 },
    /// The Engineer's Quarry's thorns bit a cat (`maze`); `alone`: the heavy bite.
    ThornBite { pos: Vec3, alone: bool },
    /// The Engineer's Quarry's labyrinth turned: gates rise and sink (`maze`).
    LabyrinthTurns,
    /// A boulder was let go (`machine`), and one rolled over a cat.
    BoulderRoll { pos: Vec3 },
    BoulderHit { pos: Vec3 },
    /// A stone landing off its chute's lip.
    BoulderLand { pos: Vec3 },
    /// A lock's pad swinging open as the stone passes it.
    PadDrop { pos: Vec3 },
    /// A cat falling through an open pad.
    CatFell { pos: Vec3 },
    /// Hourwatch (`hours`): a cat turned back in time (`back`: she turned
    /// herself; else she sent an enemy back), where from and to.
    TimeTurned { god: usize, from: Vec3, to: Vec3, back: bool },
    /// Her friends' hours wound forward, round her.
    WindFx { pos: Vec3, radius: f32 },
    /// The long look: up into the stars, and down where she lands.
    LookUp { god: usize, pos: Vec3, to: Vec3 },
    LookLand { god: usize, pos: Vec3 },
}

#[derive(Clone, Debug)]
pub struct KillEntry {
    pub text: String,
    pub t: f32,
    pub team_of_victim: Team,
}

// --- entities -----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TargetRef {
    God(usize),
    Minion(usize),
    Tower(usize),
    Titan(Team),
    /// neutral objective (bosses.rs)
    Boss(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AiState {
    Push,
    Fight,
    Retreat,
}

pub struct AiBrain {
    pub lane: Lane,
    pub aggression: f32,
    pub state: AiState,
    pub target: Option<TargetRef>,
    pub strafe_dir: f32,
    pub strafe_t: f32,
    pub buy_t: f32,
    pub wp: usize,
    /// How the current fight is being fought (`tactics`), and for how long.
    pub stance: crate::tactics::Stance,
    pub stance_t: f32,
    /// Where the current fight began: the leash is measured from here.
    pub anchor: Option<Vec3>,
    /// Somewhere it is walking to off its lane this tick (a boss, the
    /// Moonwell): what the floor router routes to, instead of the lane.
    pub goal: Option<Vec3>,
}

impl AiBrain {
    pub fn new(lane: Lane, aggression: f32) -> AiBrain {
        // a lane the active map does not have (Mid on the sky) is the
        // map's first
        let lanes = crate::map::lanes();
        let lane = if lanes.contains(&lane) { lane } else { lanes[0] };
        AiBrain {
            lane,
            aggression,
            state: AiState::Push,
            target: None,
            strafe_dir: 1.0,
            strafe_t: 0.0,
            buy_t: 1.5,
            wp: 0,
            stance: crate::tactics::Stance::Hold,
            stance_t: 0.0,
            anchor: None,
            goal: None,
        }
    }
}

/// A cast in its wind-up or channel (`GodUnit::casting`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Casting {
    pub slot: u32,
    pub aim: Vec3,
    pub t: f32,
    pub dur: f32,
    /// Movement multiplier while it lasts (0 = rooted: a channel).
    pub move_mul: f32,
}

pub struct GodUnit {
    pub def_id: u32,
    pub team: Team,
    pub is_player: bool,
    pub pos: Vec3,
    pub yaw: f32,
    pub level: u32,
    pub xp: f32,
    pub gold: f32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub items: Vec<u16>,
    pub stacks: Vec<f32>, // parallel to items — stack counters
    /// The Curios' own state (`curios`): the Ninth Life's recharge, the
    /// pounce count.
    pub curio: crate::curios::CurioState,
    pub hp: f32,
    pub max_hp: f32,
    pub mana: f32,
    pub max_mana: f32,
    pub shield: f32,
    pub shield_t: f32,
    pub cds: [f32; 4],
    pub atk_timer: f32,
    pub atk_anim: f32,
    pub walk_anim: f32,
    pub buff_atkspd: f32,
    pub buff_ms: f32,
    pub buff_t: f32,
    /// Riverblessing (from a glowfish): seconds left. Grants speed, attack
    /// speed, mana regen, and free passage through the rivers.
    pub blessing_t: f32,
    pub slow: f32,
    pub slow_t: f32,
    pub respawn_t: f32,
    pub last_dmg_t: f32,
    pub dash: Option<DashState>,
    /// An aimed leap in progress (see `leap`).
    pub leap: Option<crate::leap::LeapState>,
    pub leap_cd: f32,
    /// Standing on a prop it leapt onto: that prop's top.
    pub perch: Option<f32>,
    /// ...and it is a ridge (a wall top, a pillar cap), not a platform.
    pub perch_narrow: bool,
    /// HP last tick, to notice a hit big enough to take a cat's footing.
    pub perch_hp: f32,
    /// Riding a cloud ferry (`ferry`): which one (`Game::ferries`).
    pub riding: Option<u8>,
    /// The cloud the leap under way is aimed at.
    pub ride_plan: Option<u8>,
    /// Falling into the cloud sea: seconds until the Starwell sets the cat
    /// down at home.
    pub skyfall: Option<f32>,
    /// On a starlight lift between the islands and the Aerie (`lift`), and
    /// the pad it is standing on (it rides again only once off it).
    pub lift: Option<crate::lift::Ride>,
    pub lift_pad: Option<u8>,
    /// A Moonwell boon (`moonwell`) and the seconds it has left, and the
    /// number of the claim it came from (one per cat per claim).
    pub boon: Option<(crate::moonwell::Boon, f32)>,
    pub boon_claim: u32,
    /// Airborne without a leap: stepped, dragged or knocked off something.
    pub fall: Option<crate::leap::Fall>,
    pub recent_damagers: Vec<(usize, f32)>, // (god idx, time)
    pub move_dir: Vec3,
    pub attack_held: bool,
    /// Stalk INTENT: a toggle, pad LB, or a gentle stick. Whether the cat is
    /// actually stalking is `Game::stalking` -- fighting suspends it.
    pub sneak: bool,
    /// Seconds of ability-granted quiet left: full speed, low and silent.
    pub hush_t: f32,
    /// Giggling (Sproutkit, `kit`): no attacks or casts, wandering paws.
    pub giggle_t: f32,
    /// Dancing under a glitter-ball (`kit`), and the slow (share, seconds)
    /// it drags off with when the music stops.
    pub dance_t: f32,
    pub dance_after: (f32, f32),
    /// Afraid (Hollowstep's terror radius, `stalker`), 0..=1, and the
    /// seconds to the next heartbeat.
    pub fear: f32,
    pub beat_t: f32,
    /// Units walked since the last heavy footfall (a stalker's tread).
    pub stride: f32,
    /// Fleeing Brasshowl's lamp (`lantern`): seconds left, and from where.
    pub flee_t: f32,
    pub flee_from: Vec3,
    /// Shown up by the lamp: seconds left (seen through cloud, no stalking).
    pub revealed_t: f32,
    /// Where it has been, the last few seconds (`hours`).
    pub trail: std::collections::VecDeque<crate::hours::Mark>,
    /// Hourwatch's long look (`hours`): seconds left aloft (out of reach),
    /// and what it shows up (for how long) and how long she is seen after.
    pub aloft_t: f32,
    pub look_reveal: f32,
    pub look_exposed: f32,
    /// A Sentinel's coat (`sentinel`): the key she wears, the one a lasting
    /// coat goes back to, and how long the lasting one has left.
    pub field_slot: Option<u32>,
    pub field_back: Option<u32>,
    pub field_t: f32,
    /// Seconds toward her next magma burn.
    pub burn_t: f32,
    /// The share of each blow this cat takes, this tick: under 1 inside a
    /// friend's granite (`sentinel`) or in a pack on the Quarry
    /// (`maze`), 1 otherwise. Reset by `sentinel_tick`, which runs first.
    pub ward: f32,
    /// Seconds toward the next thorn bite (`maze`).
    pub thorn_t: f32,
    /// The last boulder run that hit this cat (`machine`: once a run).
    pub boulder_run: u32,
    /// A cast winding up or channelling: fires when `t` reaches `dur`.
    pub casting: Option<Casting>,
    /// Seconds during which movement does not turn the cat: after a cast
    /// turns it to its aim, running on must not swing it straight back.
    pub face_hold: f32,
    /// The direction the player is looking (the camera's forward, flat).
    /// Movement turns the cat only within a cone round it; zero for bots,
    /// which face where they go.
    pub face_ref: Vec3,
    /// When the last whiff acknowledgement played, to rate-limit it.
    pub whiff_t: f32,
    /// When this cat last attacked or cast.
    pub last_act_t: f32,
    pub ai: Option<AiBrain>,
    // cached derived
    pub prot: f32,
    pub pen_pct: f32,
    pub pen_flat: f32,
    pub power: f32,
    pub atk_speed: f32,
    pub cdr: f32,
    pub ms_bonus: f32,
}

pub struct DashState {
    pub from: Vec3,
    pub to: Vec3,
    pub t: f32,
    pub dur: f32,
    /// Damage to everything along the path, dealt as the dash ends.
    pub dmg: f32,
    /// Height of the hop at the middle (0.35 for a dash; a meteor's vault
    /// is far higher).
    pub arc: f32,
    /// A blast where it lands: (radius, damage). A meteor's.
    pub blast: Option<(f32, f32)>,
}

pub struct Minion {
    pub team: Team,
    pub lane: Lane,
    pub caster: bool,
    pub pos: Vec3,
    pub hp: f32,
    pub max_hp: f32,
    pub atk_timer: f32,
    pub atk_anim: f32,
    pub walk_anim: f32,
    pub wp: usize,
    pub last_hit: Option<usize>,
    pub yaw: f32,
    /// Stable identity (ids only grow, and removal keeps order, so the
    /// `minions` Vec is sorted by id). Targets hold this, not the index.
    pub id: u32,
    /// What it is fighting (`minionai`), when it re-thinks next, and where
    /// it stepped off its lane to do so.
    pub foe: Option<crate::minionai::Foe>,
    pub think_t: f32,
    pub left_at: Option<Vec3>,
    /// Its place across the lane, u from the centre-line: a wave keeps its
    /// ranks down the lane instead of threading onto one line of
    /// waypoints (thirty-two a side on the sky map became a single file).
    pub lane_off: f32,
    /// Giggling / dancing (Sproutkit, `kit`): no attacks; a dancer shuffles.
    pub giggle_t: f32,
    pub dance_t: f32,
}

pub struct Tower {
    pub team: Team,
    pub lane: Lane,
    pub inner: bool,
    pub pos: Vec3,
    pub hp: f32,
    pub max_hp: f32,
    pub atk_timer: f32,
    pub ramp: u32,
    pub last_target: Option<TargetRef>,
    /// Who it is looking at RIGHT NOW, updated every tick -- including
    /// while it reloads, and cleared the moment they leave. `last_target`
    /// only changes when it fires, so it cannot answer "am I being aimed at".
    pub aim: Option<TargetRef>,
    pub fire_anim: f32,
}

#[derive(Clone, Copy)]
pub struct Titan {
    pub team: Team,
    pub pos: Vec3,
    pub hp: f32,
    pub atk_timer: f32,
    pub atk_anim: f32,
    /// Its pedestal: where it stands guard and goes back to (`titan_zone`).
    pub home: Vec3,
    /// Facing (radians, 0 = +x), and how fast it is walking now (u/s).
    pub yaw: f32,
    pub stride: f32,
    /// The walk cycle's phase (radians), for the scene.
    pub walk: f32,
}

impl Titan {
    pub fn new(team: Team, hp: f32) -> Titan {
        let home = team.titan_pos();
        // it stands facing the enemy
        let yaw = if team.push_z() > 0.0 { std::f32::consts::FRAC_PI_2 } else { -std::f32::consts::FRAC_PI_2 };
        Titan { team, pos: home, hp, atk_timer: 0.0, atk_anim: 0.0, home, yaw, stride: 0.0, walk: 0.0 }
    }
    /// The way it faces.
    pub fn fwd(&self) -> Vec3 {
        Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin())
    }
}

pub struct Projectile {
    pub team: Team,
    pub pos: Vec3,
    pub vel: Vec3,
    pub dmg: f32,
    pub radius: f32,
    pub life: f32,
    pub kind: ProjKind,
    pub owner: usize,
    pub pen_pct: f32,
    pub pen_flat: f32,
    pub color: [f32; 3],
    /// A ranged cat's basic attack (not an ability's): see `basic_attack`.
    pub auto: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ProjKind {
    Bolt,
    Arrow,
    Lob { start: Vec3, target: Vec3, t: f32, dur: f32, radius: f32 },
}

pub struct Zone {
    pub team: Team,
    pub kind: ZoneKind,
    pub pos: Vec3,
    pub radius: f32,
    pub color: [f32; 3],
}

pub enum ZoneKind {
    Storm { until: f32, dps: f32, strike_t: f32, owner: usize },
    Ring { trigger_t: f32, dmg: f32, owner: usize, done: bool },
    /// Sproutkit's glitter-ball (`kit`): seconds left, and the slow its
    /// dancers drag off with.
    Dance { until: f32, slow: f32, slow_secs: f32, owner: usize },
}

// --- game ---------------------------------------------------------------------

pub struct Game {
    pub t: f32,
    pub gods: Vec<GodUnit>,
    pub minions: Vec<Minion>,
    pub towers: Vec<Tower>,
    pub titans: [Titan; 2],
    pub projectiles: Vec<Projectile>,
    pub zones: Vec<Zone>,
    /// Sproutkit's litters, running (`kit`).
    pub kitlings: Vec<crate::kit::Kitling>,
    /// Hollowstep's others -- his standing one and his echoes (`stalker`).
    pub others: Vec<crate::stalker::Other>,
    pub events: Vec<Event>,
    pub killfeed: Vec<KillEntry>,
    pub next_wave_t: f32,
    pub over: Option<Team>,
    pub rng: Rng,
    pub cast_queue: Vec<(usize, u32, Vec3)>, // (god idx, slot, aim)
    /// Leap requests: (god idx, aim direction). Drained by `move_gods`.
    pub leap_queue: Vec<(usize, Vec3)>,
    /// Set around a basic attack's damage (melee, or an auto's arrow
    /// landing): the siphon (brawl) and the Tidecaller Pearl pay back
    /// ability damage only; the Bramble Lash slows on basic attacks only.
    pub basic_attack: bool,
    /// Set while a Thornpelt Mantle's damage goes back, so it is not
    /// thrown back again.
    pub reflecting: bool,
    /// The river crossings. See `logs`.
    pub logs: Vec<crate::logs::RollingLog>,
    pub(crate) logs_seen: usize,
    pub tower_alive: [usize; 2],
    pub titan_vuln_announced: [bool; 2],
    pub weather: crate::weather::Weather,
    pub map_events: crate::map_events::MapEvents,
    pub wild: crate::wild::Wild,
    pub bosses: crate::bosses::Bosses,
    pub disasters: crate::disasters::Disasters,
    /// Static prop collision. EMPTY by default and filled in by the app from
    /// `Scene::blockers` once the world meshes exist, because the props are
    /// generated on the render side and `Game::new` runs before a `Scene`
    /// does. An empty set is a no-op resolve, so headless runs and tests keep
    /// the pre-collision behaviour rather than silently depending on a world
    /// they never built.
    pub blockers: std::sync::Arc<crate::blockers::Blockers>,
    /// Per-god steering memory (`steer::Memory`): a bot's planned detour.
    /// Sim state, so a replay reproduces the same detours.
    pub steer_mem: Vec<crate::steer::Memory>,
    /// Plans made this tick, against `steer::PLANS_PER_TICK`.
    pub plans_this_tick: u32,
    /// Each side's page of the playbook (`playbook`), indexed by team.
    pub styles: [crate::playbook::Page; 2],
    /// Stars the Moonwyrm has thrown, in flight (`moonwell`).
    pub star_bolts: Vec<crate::moonwell::StarBolt>,
    /// Stars the flying Moonwyrm let fall, on their way down (`wyrmflight`).
    pub starfalls: Vec<crate::wyrmflight::Starfall>,
    /// The sky map's drifting cloud (`skycloud`): its seed, and who each
    /// team can see through it.
    pub cloud_seed: u64,
    pub vision: crate::skycloud::Vision,
    /// The sky map's cloud ferries (`ferry`); empty elsewhere.
    pub ferries: Vec<crate::ferry::Ferry>,
    /// Minion ids handed out so far, and waves spawned (`minionai::wave_for`).
    pub next_minion_id: u32,
    pub waves: u32,
    /// A bot's dive on an enemy god, for its teammates to answer:
    /// (target god, when). Indexed by the diving team.
    pub calls: [Option<(usize, f32)>; 2],
    /// The Engineer's Quarry's waking order (`maze`); unused on other maps.
    pub wake: crate::maze::Wake,
}

impl Game {
    /// Create a 5v5 match. `player_god` = roster id the player controls (index 0),
    /// `ally_picks` / `enemy_picks` = roster ids for the 4 allies / 5 enemies.
    pub fn new(player_god: u32, ally_picks: [u32; 4], enemy_picks: [u32; 5]) -> Game {
        let mut blue = vec![player_god];
        blue.extend(ally_picks);
        Game::new_teams(&blue, &enemy_picks)
    }

    /// Create a match of any size: `blue[0]` is the player (god index 0),
    /// the rest of `blue` its allies, `red` the enemies. Gods are laid out
    /// blue first, then red. Lanes: the 5v5 assignments for the first five
    /// of each side (so `new` is unchanged), then round-robin.
    ///
    /// Roster ids may repeat -- 15 a side is more cats than there are
    /// ancestors -- and a repeat is just another cat of that line.
    pub fn new_teams(blue: &[u32], red: &[u32]) -> Game {
        let player_god = blue.first().copied().unwrap_or(0);
        let mut rng = Rng::new(0xC0FFEE ^ (player_god as u64 + 1));
        let total = blue.len() + red.len();
        let mut gods = Vec::with_capacity(total);

        // lanes: player mid by default, allies top/bot/mid, enemies top/mid/bot/mid/bot
        // (the map's own lanes only: the sky map has no Mid for minions)
        let map_lanes = crate::map::lanes();
        let three = map_lanes.len() == 3;
        let ally_lanes: &[Lane] = if three { &[Lane::Top, Lane::Mid, Lane::Bot, Lane::Mid] } else { &[] };
        let enemy_lanes: &[Lane] = if three { &[Lane::Top, Lane::Mid, Lane::Bot, Lane::Mid, Lane::Bot] } else { &[] };
        let round = map_lanes;

        for i in 0..total {
            let (def_id, team, is_player, lane) = if i == 0 {
                (player_god, Team::Blue, true, Lane::Mid)
            } else if i < blue.len() {
                let k = i - 1;
                (blue[i], Team::Blue, false, ally_lanes.get(k).copied().unwrap_or(round[k % round.len()]))
            } else {
                let k = i - blue.len();
                (red[k], Team::Red, false, enemy_lanes.get(k).copied().unwrap_or(round[k % round.len()]))
            };
            let def = gods::god(def_id);
            let (hp, mana, _, _, prot) = def.at_level(1);
            let spawn = Self::spawn_pos(team, lane, &mut rng);
            gods.push(GodUnit {
                def_id,
                team,
                is_player,
                pos: spawn,
                yaw: -team.push_z() * std::f32::consts::FRAC_PI_2,
                level: 1,
                xp: 0.0,
                gold: START_GOLD,
                kills: 0,
                deaths: 0,
                assists: 0,
                items: Vec::new(),
                stacks: Vec::new(),
                curio: Default::default(),
                hp,
                max_hp: hp,
                mana,
                max_mana: mana,
                shield: 0.0,
                shield_t: 0.0,
                cds: [0.0; 4],
                atk_timer: 0.0,
                atk_anim: 0.0,
                walk_anim: 0.0,
                buff_atkspd: 0.0,
                buff_ms: 0.0,
                buff_t: 0.0,
                blessing_t: 0.0,
                slow: 0.0,
                slow_t: 0.0,
                respawn_t: 0.0,
                last_dmg_t: -99.0,
                dash: None,
                leap: None,
                leap_cd: 0.0,
                perch: None,
                perch_narrow: false,
                perch_hp: 0.0,
                riding: None,
                ride_plan: None,
                skyfall: None,
                lift: None,
                lift_pad: None,
                boon: None,
                boon_claim: 0,
                fall: None,
                recent_damagers: Vec::new(),
                move_dir: Vec3::ZERO,
                attack_held: false,
                casting: None,
                face_hold: 0.0,
                face_ref: Vec3::ZERO,
                whiff_t: -10.0,
                sneak: false,
                hush_t: 0.0,
                giggle_t: 0.0,
                dance_t: 0.0,
                dance_after: (0.0, 0.0),
                fear: 0.0,
                beat_t: 0.0,
                stride: 0.0,
                flee_t: 0.0,
                flee_from: Vec3::ZERO,
                revealed_t: 0.0,
                trail: Default::default(),
                aloft_t: 0.0,
                look_reveal: 0.0,
                look_exposed: 0.0,
                field_slot: None,
                field_back: None,
                field_t: 0.0,
                burn_t: 0.0,
                ward: 1.0,
                thorn_t: 0.0,
                boulder_run: 0,
                last_act_t: -99.0,
                ai: if is_player { None } else { Some(AiBrain::new(lane, 0.45 + rng.f32() * 0.5)) },
                prot,
                pen_pct: 0.0,
                pen_flat: 0.0,
                power: def.power,
                atk_speed: 0.0,
                cdr: 0.0,
                ms_bonus: 0.0,
            });
        }

        // towers
        let mut towers = Vec::new();
        for &lane in crate::map::lanes() {
            for team in [Team::Blue, Team::Red] {
                for inner in [false, true] {
                    towers.push(Tower {
                        team,
                        lane,
                        inner,
                        pos: tower_pos(lane, team, inner),
                        hp: TOWER_HP * crate::map::active().structure_hp_mul(),
                        max_hp: TOWER_HP * crate::map::active().structure_hp_mul(),
                        atk_timer: 0.0,
                        ramp: 0,
                        last_target: None,
                        aim: None,
                        fire_anim: 0.0,
                    });
                }
            }
        }

        let titans = [
            Titan::new(Team::Blue, TITAN_HP * crate::map::active().structure_hp_mul()),
            Titan::new(Team::Red, TITAN_HP * crate::map::active().structure_hp_mul()),
        ];

        let wild = crate::wild::Wild::new(&mut rng);
        let mut g = Game {
            t: 0.0,
            gods,
            minions: Vec::new(),
            towers,
            titans,
            projectiles: Vec::new(),
            zones: Vec::new(),
            kitlings: Vec::new(),
            others: Vec::new(),
            events: Vec::new(),
            killfeed: Vec::new(),
            next_wave_t: WAVE_FIRST,
            over: None,
            rng,
            cast_queue: Vec::new(),
            leap_queue: Vec::new(),
            basic_attack: false,
            reflecting: false,
            logs: crate::logs::crossings().iter().map(crate::logs::RollingLog::across).collect(),
            logs_seen: 0,
            tower_alive: [0, 0], // counted below
            titan_vuln_announced: [false, false],
            weather: crate::weather::Weather::new(),
            map_events: crate::map_events::MapEvents::new(),
            wild,
            bosses: crate::bosses::Bosses::new(),
            disasters: crate::disasters::Disasters::default(),
            blockers: std::sync::Arc::new(crate::blockers::Blockers::default()),
            steer_mem: vec![crate::steer::Memory::default(); total],
            plans_this_tick: 0,
            styles: [crate::playbook::even_keel(), crate::playbook::even_keel()],
            calls: [None, None],
            // its own seed from the match's (not drawn from `rng`, so every
            // other random thing in a match stays as it was)
            wake: crate::maze::Wake::new(&mut Rng::new((player_god as u64 + 1).wrapping_mul(0xD6E8_FEB8_6659_FD93) ^ 0x7A0E)),
            ferries: crate::ferry::for_map(),
            star_bolts: Vec::new(),
            starfalls: Vec::new(),
            // from the match's own seed, without drawing on `rng` (so every
            // other random thing in the match stays as it was)
            cloud_seed: (player_god as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xC10D,
            vision: Default::default(),
            next_minion_id: 0,
            waves: 0,
        };
        for t in &g.towers {
            g.tower_alive[t.team as usize] += 1;
        }
        // starter items for AI gods (player buys via shop)
        for i in 0..total {
            if !g.gods[i].is_player {
                g.auto_buy(i);
            }
        }
        g
    }

    pub fn spawn_pos(team: Team, lane: Lane, rng: &mut Rng) -> Vec3 {
        let base = team.fountain() + Vec3::new(0.0, 0.0, team.push_z() * 8.0);
        let jitter = Vec3::new(rng.range(-3.0, 3.0), 0.0, rng.range(-2.0, 2.0));
        let lane_off = match lane {
            Lane::Top => Vec3::new(-4.0, 0.0, 0.0),
            Lane::Bot => Vec3::new(4.0, 0.0, 0.0),
            Lane::Mid => Vec3::ZERO,
        };
        base + jitter + lane_off
    }

    /// Give each side its page of the playbook, and move the bots to the
    /// lanes the page plays (`playbook::lanes_for`). Announced at the start.
    pub fn set_styles(&mut self, pages: [crate::playbook::Page; 2], rng: &mut Rng) {
        for team in [Team::Blue, Team::Red] {
            let genes = pages[team as usize].genes;
            let bots: Vec<usize> = (0..self.gods.len()).filter(|&i| self.gods[i].team == team && self.gods[i].ai.is_some() && !self.gods[i].is_player).collect();
            let now: Vec<Lane> = bots.iter().map(|&i| self.gods[i].ai.as_ref().unwrap().lane).collect();
            let want = crate::playbook::lanes_for(&genes, &now, rng);
            for (k, &i) in bots.iter().enumerate() {
                let b = self.gods[i].ai.as_mut().unwrap();
                if b.lane != want[k] {
                    b.lane = want[k];
                    b.wp = 0;
                }
            }
        }
        self.styles = pages;
    }

    /// Draw both sides' pages from the booklet.
    pub fn open_playbook(&mut self, rng: &mut Rng) {
        let pages = crate::playbook::draw_pair(rng);
        self.set_styles(pages, rng);
    }

    /// An Elder's full health on this map.
    pub fn titan_max_hp(&self) -> f32 {
        TITAN_HP * crate::map::active().structure_hp_mul()
    }

    pub fn def(&self, i: usize) -> &'static GodDef {
        gods::god(self.gods[i].def_id)
    }

    pub fn god_name(&self, i: usize) -> &'static str {
        self.def(i).name
    }

    // --- derived stats ---------------------------------------------------------

    pub fn item_stats(god: &GodUnit) -> items::Stats {
        let cat = items::all();
        let mut s = items::Stats::ZERO;
        for (k, id) in god.items.iter().enumerate() {
            let item = &cat[*id as usize];
            s.add(&item.stats);
            // a trophy's growth, on the stat it names. (This matched on the
            // item's shelf, and every trophy sits on the Stacking shelf, so
            // no stack ever reached a stat.)
            if let Some(st) = &item.stack {
                let gained = st.gained(god.stacks.get(k).copied().unwrap_or(0.0));
                match st.grows {
                    items::Grow::Strength => s.strength += gained,
                    items::Grow::Intelligence => s.intelligence += gained,
                    items::Grow::Power => s.power += gained,
                    items::Grow::Protection => s.protection += gained,
                    items::Grow::Health => s.health += gained,
                }
            }
        }
        s
    }

    pub fn recalc(&mut self, i: usize) {
        let def = self.def(i);
        let g = &mut self.gods[i];
        let (base_hp, base_mana, _, base_power, base_prot) = def.at_level(g.level);
        let st = Self::item_stats(g);
        let new_max = base_hp + st.health;
        let hp_frac = if g.max_hp > 0.0 { g.hp / g.max_hp } else { 1.0 };
        g.max_hp = new_max;
        g.hp = (new_max * hp_frac).max(g.hp.min(new_max));
        let new_mmax = base_mana + st.mana;
        g.max_mana = new_mmax;
        g.mana = g.mana.min(new_mmax);
        g.power = base_power + st.power + st.intelligence * 0.9 + st.strength * 0.6;
        g.prot = base_prot + st.protection;
        g.pen_pct = st.pen_pct;
        g.pen_flat = st.pen_flat;
        g.atk_speed = st.attack_speed;
        g.cdr = st.cdr.min(0.4);
        g.ms_bonus = st.move_speed;
    }

    /// What `viewer` should be shown about each tower: `[x, z, range, state]`.
    ///
    /// States answer the two questions a tower leaves open. WHERE is the
    /// line: every standing tower draws its range on the ground. WHO is it
    /// after: the ring changes the moment it matters to you.
    ///   1  yours: calm, the ground you are safe on
    ///   2  theirs, and you are outside it: a dashed warning
    ///   3  theirs, and you are INSIDE it: solid -- it has not picked you yet
    ///      (minions draw fire first), but it can
    ///   4  theirs, and it is aiming at YOU
    ///   0  fallen: nothing drawn
    pub fn tower_rings(&self, viewer: usize) -> [[f32; 4]; crate::render::MAX_TOWER_RINGS] {
        let mut out = [[0.0; 4]; crate::render::MAX_TOWER_RINGS];
        let Some(me) = self.gods.get(viewer) else { return out };
        for (slot, tw) in out.iter_mut().zip(self.towers.iter()) {
            if tw.hp <= 0.0 {
                continue;
            }
            let dist = (tw.pos.x - me.pos.x).hypot(tw.pos.z - me.pos.z);
            // A line you are nowhere near is not information, it is a scratch
            // across the scenery (seen in a render 40 u from an allied tower).
            if dist > TOWER_RANGE + TOWER_RING_SHOW {
                continue;
            }
            let inside = me.respawn_t <= 0.0 && dist < TOWER_RANGE;
            let state = if tw.team == me.team {
                1.0
            } else if tw.aim == Some(TargetRef::God(viewer)) {
                4.0
            } else if inside {
                3.0
            } else {
                2.0
            };
            *slot = [tw.pos.x, tw.pos.z, TOWER_RANGE, state];
        }
        out
    }

    /// Is this cat stalking right now? Intent, minus a fight: attacking,
    /// casting or being hit suspends it for STALK_BREAK_SECS. The intent is
    /// left alone, so the cat settles back into the stalk when the fight is
    /// over -- and gets its full speed back the instant one starts.
    pub fn stalking(&self, i: usize) -> bool {
        let g = &self.gods[i];
        let b = crate::consts::STALK_BREAK_SECS;
        // shown up by Brasshowl's lamp (`lantern`): nothing to hide in
        g.sneak && g.revealed_t <= 0.0 && self.t - g.last_dmg_t > b && self.t - g.last_act_t > b
    }

    /// How quietly this cat carries itself, `0..=1`, from every source.
    ///
    /// Deliberately not a key you hold. Stalking is balanced against
    /// fighting by what it is tied to: intent that a fight interrupts, a
    /// window some abilities grant (the fast low run before a pounce, at
    /// full speed), and the way a badly hurt animal slinks.
    /// Can `team` see cat `j`? Always for its own cats, on the open maps,
    /// and before the first refresh (`vision_tick`); through the sky map's
    /// clouds and the Quarry's walls, only as `vision_tick` found.
    pub fn sees(&self, team: Team, j: usize) -> bool {
        // shown up by Brasshowl's lamp (`lantern`): seen by everyone
        if self.gods.get(j).is_some_and(|g| g.revealed_t > 0.0) {
            return true;
        }
        self.vision.seen[team as usize].get(j).copied().unwrap_or(true)
    }

    /// Who each team can see through the sky map's clouds, refreshed
    /// `skycloud::VISION_HZ` times a second. Only cloud hides: there is no
    /// fog of war in clear air. So a team sees an enemy cat
    ///
    /// * if one of its own living cats within `skycloud::SIGHT_RANGE` has
    ///   a clear line to it (`skycloud::sees`); or
    /// * if it stands in clear air and none of the team's cats is that
    ///   close (far off in the open, it is seen as it always was); or
    /// * inside one of the team's standing towers' reach, or its base
    ///   (towers and Elders see through cloud: no base is taken blind).
    ///
    /// So a cat in a bank is hidden from everyone outside it, and a wisp
    /// between two groups hides each from the other while it is thick.
    /// Minions give no sight: with 32 a side in every lane, they would see
    /// everything and the cloud nothing.
    pub fn vision_tick(&mut self) {
        // the Quarry: its walls and shut gates hide (`maze::sight`)
        if crate::map::active() == crate::map::MapId::Maze {
            if self.t < self.vision.next_t && self.vision.seen[0].len() == self.gods.len() {
                return;
            }
            self.vision.next_t = self.t + 1.0 / crate::skycloud::VISION_HZ;
            self.vision.seen = [Team::Blue, Team::Red].map(|team| crate::maze::seen_by(self, team));
            return;
        }
        if crate::map::active() != crate::map::MapId::Sky {
            if !self.vision.seen[0].is_empty() {
                self.vision = Default::default();
            }
            return;
        }
        if self.t < self.vision.next_t && self.vision.seen[0].len() == self.gods.len() {
            return;
        }
        self.vision.next_t = self.t + 1.0 / crate::skycloud::VISION_HZ;
        let cs = crate::skycloud::clouds(self.cloud_seed, self.t);
        let n = self.gods.len();
        for team in [Team::Blue, Team::Red] {
            let mut seen = vec![true; n];
            if !cs.is_empty() {
                for j in 0..n {
                    let e = &self.gods[j];
                    if e.team == team || e.respawn_t > 0.0 {
                        continue;
                    }
                    let near = |v: &GodUnit| v.team == team && v.respawn_t <= 0.0 && v.pos.distance(e.pos) <= crate::skycloud::SIGHT_RANGE;
                    let by_cat = self.gods.iter().any(|v| near(v) && crate::skycloud::sees(&cs, v.pos, e.pos));
                    let open_air = crate::skycloud::density_at(&cs, e.pos) < 0.4 && !self.gods.iter().any(|v| near(v));
                    let by_tower = self.towers.iter().any(|t| t.team == team && t.hp > 0.0 && t.pos.distance(e.pos) < crate::consts::TOWER_RANGE + 2.0);
                    let at_base = team.base_pos().distance(e.pos) < 40.0;
                    seen[j] = by_cat || open_air || by_tower || at_base;
                }
            }
            self.vision.seen[team as usize] = seen;
        }
        self.vision.clouds = cs;
    }

    pub fn quiet(&self, i: usize) -> f32 {
        let g = &self.gods[i];
        let mut q: f32 = 0.0;
        if self.stalking(i) {
            q = 1.0;
        }
        if g.hush_t > 0.0 {
            q = q.max(0.9);
        }
        if g.hp < g.max_hp * crate::consts::WOUNDED_FRAC {
            q = q.max(0.5);
        }
        q
    }

    pub fn move_speed(&self, i: usize) -> f32 {
        let g = &self.gods[i];
        let def = self.def(i);
        let bless = if g.blessing_t > 0.0 { BLESSING_MS } else { 0.0 };
        // the Catnip Censer's air and the Wanderer's Whiskers (curios.rs)
        let curio = self.curio_move_bonus(i);
        let base = def.move_speed * (1.0 + g.ms_bonus + g.buff_ms + bless + curio);
        // wading: the rivers are slow going unless you were born to them
        let depth = self.water_depth_now(g.pos.x, g.pos.z);
        let wade = if g.blessing_t > 0.0 || crate::wild::is_riverborn(def) { 1.0 } else { wade_speed_mul(depth) };
        let stalking = self.stalking(i);
        let stalk = if stalking { crate::consts::SNEAK_SPEED } else { 1.0 };
        // Rough ground (`going`). Not while airborne or perched: a leap
        // clears the thicket, and a boulder's top is not its scree skirt.
        let going = if g.leap.is_some() || g.dash.is_some() || g.perch.is_some() {
            1.0
        } else {
            self.blockers.going_at(g.pos.x, g.pos.z, crate::going::Carriage { stalking })
        };
        let boon = crate::moonwell::move_mul(g.boon.map(|b| b.0));
        // dancing under Sproutkit's glitter-ball: a shuffle
        let dance = if g.dance_t > 0.0 { crate::kit::DANCE_MOVE } else { 1.0 };
        // afraid (`stalker`): the legs go heavy
        let fear = 1.0 - crate::stalker::FEAR_SLOW * g.fear;
        fear * dance * base * (1.0 - g.slow) * self.map_events.speed_mul(def.role, g.def_id) * wade * self.disasters.speed_mul() * stalk * going * boon
    }

    pub fn auto_interval(&self, i: usize) -> f32 {
        let g = &self.gods[i];
        let bless = if g.blessing_t > 0.0 { BLESSING_AS } else { 0.0 };
        self.def(i).auto_cd / (1.0 + g.atk_speed + g.buff_atkspd + bless + self.censer_at(i).1)
    }

    pub fn auto_damage(&self, i: usize) -> f32 {
        let g = &self.gods[i];
        let (_, _, auto, _, _) = self.def(i).at_level(g.level);
        auto + g.power * 0.22
    }

    // --- damage ----------------------------------------------------------------

    pub fn deal_damage(&mut self, target: TargetRef, mut dmg: f32, magic: bool, source: Option<usize>, pen_pct: f32, pen_flat: f32) {
        if self.over.is_some() {
            return;
        }
        let _ = magic;
        // Stormfire: the Drake's blessing multiplies everything its holders deal
        if let Some(s) = source {
            if s < self.gods.len() {
                let team = self.gods[s].team;
                dmg *= self.bosses.dmg_mul(team);
                // Wyrmfang (the Moonwell)
                dmg *= crate::moonwell::dealt_mul(self.gods[s].boon.map(|b| b.0));
                if matches!(target, TargetRef::Tower(_) | TargetRef::Titan(_)) {
                    dmg += self.bosses.struct_bonus(team);
                }
            }
        }
        // overtime (the sky map): structures give way as the match runs long
        if matches!(target, TargetRef::Tower(_) | TargetRef::Titan(_)) {
            dmg *= crate::map::active().overtime(self.t);
        }
        match target {
            TargetRef::Boss(bi) => {
                self.boss_take_damage(bi, dmg, source);
            }
            // up in the stars (Hourwatch's long look): out of reach
            TargetRef::God(ti) if self.gods.get(ti).is_some_and(|g| g.aloft_t > 0.0) => {}
            TargetRef::God(ti) => {
                let g = &mut self.gods[ti];
                if g.respawn_t > 0.0 {
                    return;
                }
                let eff = (g.prot * (1.0 - pen_pct) - pen_flat).max(0.0);
                dmg *= 100.0 / (100.0 + eff);
                // Moonshell (the Moonwell): nine-tenths of every blow turned
                dmg *= crate::moonwell::taken_mul(g.boon.map(|b| b.0));
                // a friend's granite (`sentinel`)
                dmg *= g.ward.clamp(0.0, 1.0);
                let pre = g.hp + g.shield;
                if g.shield > 0.0 {
                    let absorbed = g.shield.min(dmg);
                    g.shield -= absorbed;
                    dmg -= absorbed;
                }
                g.hp -= dmg;
                g.last_dmg_t = self.t;
                // the siphon: ability damage from an enemy CAT returns a share
                // as mana to the one who took it (brawl)
                let from_enemy_cat = !self.basic_attack && source.is_some_and(|s| s != ti && s < self.gods.len());
                let g = &mut self.gods[ti];
                if from_enemy_cat && g.hp > 0.0 {
                    let pct = crate::brawl::siphon_pct(&g.items);
                    g.mana = (g.mana + dmg * pct).min(g.max_mana);
                }
                let g = &mut self.gods[ti];
                if let Some(s) = source {
                    if s != ti {
                        g.recent_damagers.retain(|(idx, t)| *idx != s && self.t - *t < 8.0);
                        g.recent_damagers.push((s, self.t));
                    }
                }
                let _ = pre;
                // the Curios that answer a blow (curios.rs)
                self.curios_on_hit(ti, source, dmg);
                if self.gods[ti].hp <= 0.0 && !self.ninth_life(ti) {
                    self.on_god_death(ti, source);
                } else {
                    let pos = self.gods[ti].pos;
                    self.events.push(Event::Damage { pos: pos + Vec3::Y * 1.6, amount: dmg, big: dmg > 120.0 });
                }
            }
            TargetRef::Minion(mi) => {
                if mi >= self.minions.len() {
                    return;
                }
                let m = &mut self.minions[mi];
                m.hp -= dmg;
                if let Some(s) = source {
                    m.last_hit = Some(s);
                }
                self.events.push(Event::Damage { pos: m.pos + Vec3::Y * 0.9, amount: dmg, big: false });
                if m.hp <= 0.0 {
                    self.on_minion_death(mi);
                }
            }
            TargetRef::Tower(ti) => {
                if ti >= self.towers.len() || self.towers[ti].hp <= 0.0 {
                    return;
                }
                let tw = &mut self.towers[ti];
                tw.hp -= dmg;
                self.events.push(Event::Damage { pos: tw.pos + Vec3::Y * 3.0, amount: dmg, big: false });
                if tw.hp <= 0.0 {
                    self.on_tower_death(ti);
                }
            }
            TargetRef::Titan(team) => {
                let ti = team as usize;
                if self.titans[ti].hp <= 0.0 {
                    return;
                }
                if self.titan_shielded(team) {
                    return; // immune
                }
                self.titans[ti].hp -= dmg;
                self.events.push(Event::Damage { pos: self.titans[ti].pos + Vec3::Y * 3.5, amount: dmg, big: dmg > 150.0 });
                if self.titans[ti].hp <= 0.0 {
                    self.titans[ti].hp = 0.0;
                    let winner = team.other();
                    self.over = Some(winner);
                    self.events.push(Event::Announce { text: if winner == Team::Blue { "VICTORY".into() } else { "DEFEAT".into() } });
                }
            }
        }
    }

    pub fn titan_shielded(&self, team: Team) -> bool {
        self.tower_alive[team as usize] >= crate::map::active().titan_towers_req()
    }

    fn on_minion_death(&mut self, mi: usize) {
        let m = &self.minions[mi];
        let gold = if m.caster { GOLD_CASTER } else { GOLD_MELEE };
        let xp = if m.caster { XP_CASTER } else { XP_MELEE };
        let pos = m.pos;
        let enemy_team = m.team.other();
        // gold to last hitter (if a god), xp shared
        if let Some(killer) = m.last_hit {
            if killer < self.gods.len() && self.gods[killer].team != m.team {
                let team = self.gods[killer].team;
                self.gods[killer].gold += gold * self.map_events.gold_mul() * self.bosses.gold_mul(team);
                self.add_stacks(killer, StackKind::MinionKill, 1.0);
            }
        }
        for i in 0..self.gods.len() {
            let g = &self.gods[i];
            if g.team == enemy_team && g.respawn_t <= 0.0 && g.pos.distance(pos) <= XP_SHARE_RADIUS {
                self.grant_xp(i, xp);
            }
        }
        self.minions.remove(mi);
    }

    fn on_tower_death(&mut self, ti: usize) {
        let tw = &self.towers[ti];
        let pos = tw.pos;
        let team = tw.team;
        let killer_team = team.other();
        // gold to all enemy gods alive nearby-ish; simplest: all enemy gods get share
        for i in 0..self.gods.len() {
            if self.gods[i].team == killer_team {
                self.gods[i].gold += GOLD_TOWER / 3.0;
                self.add_stacks(i, StackKind::TowerKill, 1.0);
            }
        }
        self.tower_alive[team as usize] -= 1;
        self.events.push(Event::TowerDown { pos, team });
        self.events.push(Event::Announce {
            text: format!("{} TOWER DESTROYED", if team == Team::Blue { "BLUE" } else { "RED" }),
        });
        if self.titan_shielded(team) == false && !self.titan_vuln_announced[team as usize] {
            self.titan_vuln_announced[team as usize] = true;
            self.events.push(Event::TitanVulnerable { team });
            self.events.push(Event::Announce { text: format!("{} ELDER IS VULNERABLE", if team == Team::Blue { "BLUE" } else { "RED" }) });
        }
        self.towers.remove(ti);
    }

    fn on_god_death(&mut self, gi: usize, source: Option<usize>) {
        // snapshot immutable data first
        let (pos, name, team, helpers) = {
            let g = &self.gods[gi];
            (
                g.pos,
                self.god_name(gi).to_string(),
                g.team,
                g.recent_damagers.iter().map(|(idx, _)| *idx).collect::<Vec<_>>(),
            )
        };
        {
            let g = &mut self.gods[gi];
            g.deaths += 1;
            // overtime (the sky map) stretches the wait too: a won fight then
            // opens the base long enough for a wave to reach the Elder
            g.respawn_t = respawn_time(g.level) * crate::map::active().overtime(self.t).min(2.0);
            g.hp = 0.0;
            g.shield = 0.0;
            g.dash = None;
            g.recent_damagers.clear();
        }
        self.events.push(Event::Damage { pos: pos + Vec3::Y * 1.5, amount: 0.0, big: false });

        let mut kill_text = String::new();
        if let Some(k) = source {
            if k < self.gods.len() && self.gods[k].team != team {
                let kg = &mut self.gods[k];
                kg.kills += 1;
                let kt = kg.team;
                kg.gold += GOLD_GOD * self.map_events.gold_mul() * self.bosses.gold_mul(kt);
                self.add_stacks(k, StackKind::GodKill, 1.0);
                self.grant_xp(k, XP_GOD);
                kill_text = format!("{} slew {}", self.god_name(k), name);
            }
        }
        if kill_text.is_empty() {
            kill_text = format!("{} has fallen", name);
        }
        for h in helpers {
            if h < self.gods.len() && h != source.unwrap_or(usize::MAX) && self.gods[h].team != team {
                self.gods[h].assists += 1;
                self.gods[h].gold += 120.0;
                self.grant_xp(h, XP_GOD * 0.4);
            }
        }
        self.killfeed.push(KillEntry { text: kill_text, t: self.t, team_of_victim: team });
        if self.killfeed.len() > 6 {
            self.killfeed.remove(0);
        }
    }

    /// Put cat `gi` at `level` (XP cleared), its stats and ability ranks
    /// with it. For tests and shots that need a kit unlocked.
    pub fn set_level(&mut self, gi: usize, level: u32) {
        self.gods[gi].level = level.clamp(1, MAX_LEVEL);
        self.gods[gi].xp = 0.0;
        self.recalc(gi);
    }

    pub fn grant_xp(&mut self, gi: usize, mut xp: f32) {
        xp *= 1.0 + 0.02 * (self.t / 60.0).min(8.0); // late-game scaling
        let g = &mut self.gods[gi];
        g.xp += xp;
        let mut leveled = false;
        while g.level < MAX_LEVEL && g.xp >= xp_for_level(g.level) {
            g.xp -= xp_for_level(g.level);
            g.level += 1;
            leveled = true;
        }
        if leveled {
            let pos = g.pos;
            self.events.push(Event::LevelUp { pos });
            self.recalc(gi);
            let g = &mut self.gods[gi];
            g.hp = (g.hp + g.max_hp * 0.22).min(g.max_hp);
            g.mana = (g.mana + g.max_mana * 0.3).min(g.max_mana);
        }
    }

    /// `n` more stacks on every held trophy of `kind` (to its cap), and the
    /// stats follow when a stack lands.
    pub fn add_stacks(&mut self, gi: usize, kind: StackKind, n: f32) {
        let cat = items::all();
        let g = &mut self.gods[gi];
        let mut changed = false;
        for (k, id) in g.items.clone().iter().enumerate() {
            if let Some(st) = &cat[*id as usize].stack {
                if st.kind == kind {
                    let cur = g.stacks.get(k).copied().unwrap_or(0.0);
                    let next = (cur + n).min(st.cap());
                    changed |= st.gained(next) != st.gained(cur);
                    if k < g.stacks.len() {
                        g.stacks[k] = next;
                    } else {
                        g.stacks.push(next);
                    }
                }
            }
        }
        if changed {
            self.recalc(gi);
        }
    }

    pub fn buy_item(&mut self, gi: usize, item_id: u16) -> bool {
        let cat = items::all();
        let g = &self.gods[gi];
        if g.items.len() >= 6 {
            return false;
        }
        let item = &cat[item_id as usize];
        if g.gold < item.cost {
            return false;
        }
        let g = &mut self.gods[gi];
        g.gold -= item.cost;
        g.items.push(item_id);
        g.stacks.push(0.0);
        self.recalc(gi);
        true
    }

    /// Sell the item in build slot `slot` for a fraction of its cost
    /// (`SELL_REFUND`) — a mispurchase or a build pivot shouldn't be
    /// permanent. Returns the gold refunded, or `None` if the slot is empty.
    pub fn sell_item(&mut self, gi: usize, slot: usize) -> Option<f32> {
        let g = &mut self.gods[gi];
        if slot >= g.items.len() {
            return None;
        }
        let item_id = g.items.remove(slot);
        if slot < g.stacks.len() {
            g.stacks.remove(slot);
        }
        let refund = (items::all()[item_id as usize].cost * SELL_REFUND).round();
        g.gold += refund;
        self.recalc(gi);
        Some(refund)
    }

    /// AI auto-buy: walk the role build order, buy the best affordable item.
    pub fn auto_buy(&mut self, gi: usize) {
        let role = self.def(gi).role;
        let order = items::role_build_order(role);
        let cat = items::all();
        let g = &self.gods[gi];
        if g.items.len() >= 6 {
            return;
        }
        let mut best: Option<u16> = None;
        'outer: for wanted in order {
            // prefer highest tier affordable in this category
            let mut cands: Vec<&items::Item> = cat.iter().filter(|it| it.cat == wanted).collect();
            cands.sort_by(|a, b| b.cost.partial_cmp(&a.cost).unwrap());
            for it in cands {
                if it.cost <= g.gold {
                    best = Some(it.id);
                    break 'outer;
                }
            }
        }
        if let Some(id) = best {
            self.buy_item(gi, id);
        }
    }

    /// God positions of enemies of `team` that are alive.
    pub fn enemy_gods_near(&self, pos: Vec3, team: Team, radius: f32) -> Vec<usize> {
        (0..self.gods.len())
            .filter(|&i| self.gods[i].team != team && self.gods[i].respawn_t <= 0.0 && self.gods[i].pos.distance(pos) <= radius)
            .collect()
    }
}

#[cfg(test)]
mod shop_tests {
    use super::*;

    #[test]
    fn selling_refunds_a_fraction_and_frees_the_slot() {
        let mut g = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        let item = items::all().iter().find(|i| i.cat == items::Cat::Health).unwrap();
        g.gods[0].gold = item.cost;
        assert!(g.buy_item(0, item.id));
        assert_eq!(g.gods[0].items.len(), 1);
        let refund = g.sell_item(0, 0).expect("a real slot");
        // the refund is rounded to whole gold (425 x 0.7 = 297.5 -> 298)
        assert_eq!(refund, (item.cost * SELL_REFUND).round());
        assert_eq!(g.gods[0].gold, refund, "sold with nothing else bought, gold is exactly the refund");
        assert!(g.gods[0].items.is_empty());
        assert!(g.sell_item(0, 0).is_none(), "an empty slot sells nothing");
    }

    /// A fresh `Game` must collide with nothing, so headless runs and every
    /// other test here keep the pre-collision behaviour rather than silently
    /// depending on a world they never built.
    #[test]
    fn a_fresh_game_has_no_prop_collision() {
        let g = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        assert!(g.blockers.is_empty());
        let p = glam::Vec3::new(10.0, 0.0, 10.0);
        assert_eq!(g.blockers.resolve(p, 0.55), p, "an empty set is a no-op");
    }

    /// Movement must not push a unit outside the arena. The prop resolve runs
    /// BEFORE the rim clamp precisely so the clamp has the last word.
    #[test]
    fn prop_pushout_cannot_eject_a_unit_past_the_arena_rim() {
        use crate::blockers::{Blocker, Blockers, Shape};
        let mut g = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        let rim = ARENA_RADIUS - 2.5;
        let mut b = Blockers::new(ARENA_RADIUS + 20.0, 6.0);
        // A big blocker right where the god stands, pushing it outward.
        b.push(Blocker { shape: Shape::Circle { x: rim, z: 0.0, r: 4.0 }, top: 5.0,
                         cover: crate::blockers::Cover::NONE });
        g.blockers = std::sync::Arc::new(b);
        g.gods[0].pos = glam::Vec3::new(rim, 0.0, 0.0);
        for _ in 0..60 {
            g.tick(1.0 / 60.0);
        }
        let r = glam::Vec3::new(g.gods[0].pos.x, 0.0, g.gods[0].pos.z).length();
        assert!(r <= ARENA_RADIUS - 2.0 + 1e-3, "pushed outside the arena: r={r}");
        assert!(g.gods[0].pos.is_finite());
    }

    /// The god radius must be the BODY, not the god-vs-god personal space.
    /// Reusing the separation's 1.1 against static geometry would hold a cat
    /// half a body-width off every trunk, which reads as an invisible wall.
    #[test]
    fn unit_radii_are_body_sized() {
        assert!(GOD_RADIUS < 1.1);
        assert!(MINION_RADIUS < GOD_RADIUS);
        assert!(MINION_RADIUS > 0.0);
    }

    /// THE REPORTED CRASH: "index out of bounds: the len is 10 but the index
    /// is 10", panicking inside `Scene::draw` on an M2 Max.
    ///
    /// A `TargetRef` is an INDEX and `on_minion_death` does
    /// `minions.remove(mi)`, which shifts every later index down. A ref taken
    /// before that removal — an AI target held across frames, a beam being
    /// drawn — then names a different minion, and a ref to the LAST one names
    /// nothing at all. That last case is the panic.
    #[test]
    fn a_target_ref_past_the_end_is_absent_rather_than_a_panic() {
        let g = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        let n = g.minions.len();
        // Exactly the reported shape: index == len.
        assert_eq!(g.target_pos_opt(TargetRef::Minion(n)), None);
        assert_eq!(g.target_pos_opt(TargetRef::Minion(n + 5)), None);
        // Every variant, because every one of them indexes a Vec that can
        // shrink and the panic was only ever seen in one.
        assert_eq!(g.target_pos_opt(TargetRef::God(g.gods.len())), None);
        assert_eq!(g.target_pos_opt(TargetRef::Tower(g.towers.len())), None);
        assert_eq!(g.target_pos_opt(TargetRef::Boss(g.bosses.list.len())), None);
    }

    /// A live target still answers — a guard that returned None for everything
    /// would pass the test above and silently stop the game working.
    #[test]
    fn a_live_target_still_resolves() {
        let g = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        if !g.minions.is_empty() {
            assert!(g.target_pos_opt(TargetRef::Minion(0)).is_some());
        }
        assert!(g.target_pos_opt(TargetRef::God(0)).is_some(), "gods always exist");
    }

    /// Removing a minion is what invalidates the refs after it, which is the
    /// mechanism rather than the symptom.
    #[test]
    fn removing_a_minion_invalidates_the_last_index() {
        // `Minion` is not Clone, so the vector is shortened rather than
        // padded — same mechanism, no fabricated state.
        let mut g = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        if g.minions.is_empty() { return; }
        let last = g.minions.len() - 1;
        assert!(g.target_pos_opt(TargetRef::Minion(last)).is_some(),
                "the last minion resolves before the removal");
        g.minions.remove(0);
        assert_eq!(g.target_pos_opt(TargetRef::Minion(last)), None,
                   "the index that was valid before the removal must now be absent");
    }
}
