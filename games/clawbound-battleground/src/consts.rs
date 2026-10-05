use glam::Vec3;

// ---------------------------------------------------------------------------
// Arena layout — three lanes (TOP / MID / BOT), two bases, 12 towers, 2 titans
// Blue base sits south (negative Z), Red base north (positive Z).
// The valley is an expanded jungle map: dense playable jungle inside
// ARENA_RADIUS, then overgrown hills and colossal ruins beyond the bounds.
// ---------------------------------------------------------------------------

/// Global layout scale — expands the whole valley (lanes, bases, towers).
// TODO I just want to see this lol
pub const MAP_SCALE: f32 = 3.75;
// TODO: investigate why setting MAP_SCALE to 10.42 flips cat legs upside down
//       and also does not scale the jungle mountains slope as much as the travel dist
// pub const MAP_SCALE: f32 = 1.42;

/// Deep jungle added beyond the lanes, world units. The arena grows by this
/// much WITHOUT moving a lane, a base or a tower: MAP_SCALE stretches the
/// whole valley and so changes every walk in the match; this only adds
/// ground behind the side lanes and rivers to hunt, hide and flank through.
pub const JUNGLE_MARGIN: f32 = 32.0;

pub const ARENA_RADIUS: f32 = 95.0 * MAP_SCALE + JUNGLE_MARGIN; // playable floor (~167)

/// The playable radius of the active map. One function so a map can differ
/// in size (the jungle and the pass share `ARENA_RADIUS` today).
pub fn arena_radius() -> f32 {
    match crate::map::active() {
        crate::map::MapId::Jungle => ARENA_RADIUS,
        crate::map::MapId::Pass => crate::pass::ARENA_RADIUS,
        crate::map::MapId::Sky => crate::sky::ARENA_RADIUS,
        crate::map::MapId::Atlantis => crate::atlantis::ARENA_RADIUS,
        crate::map::MapId::Maze => crate::maze::ARENA_RADIUS,
    }
}

/// Half the side of the rendered terrain square, for the active map.
pub fn ground_half() -> f32 {
    match crate::map::active() {
        crate::map::MapId::Jungle => GROUND_HALF,
        crate::map::MapId::Pass => crate::pass::GROUND_HALF,
        crate::map::MapId::Sky => crate::sky::GROUND_HALF,
        crate::map::MapId::Atlantis => crate::atlantis::GROUND_HALF,
        crate::map::MapId::Maze => crate::maze::GROUND_HALF,
    }
}

/// The arena the outer ring was authored round, in world units: 95 u of lanes at the original
/// MAP_SCALE of 1.42. A number, not `95.0 * MAP_SCALE`: that expression equalled 135 only at
/// 1.42, and at any other scale it cancelled MAP_SCALE out of OUTER_SCALE -- at 3.75 the rim
/// began at 187 u inside a 388 u floor, the rivers ran up the mountainside (dry: the bed never
/// got below the water line), and the ground ended 140 u short of the camera's reach, which is
/// what 111256693's TODO ("does not scale the jungle mountains slope as much as the travel
/// dist") was seeing.
pub const AUTHORED_ARENA: f32 = 135.0;

/// Everything past the playable floor -- the hill ring, the landmarks, the
/// mountain rim, the rendered ground -- was authored in world units for a
/// 135 u arena. It is carried out by this ratio so the ring keeps its shape
/// round a bigger floor instead of the floor growing into it.
pub const OUTER_SCALE: f32 = ARENA_RADIUS / AUTHORED_ARENA;

/// Where the mountain rim starts rising.
pub const RIM_RADIUS: f32 = 172.0 * OUTER_SCALE;

/// Collision radius of a god, for prop push-out.
///
/// 0.55 rather than the 1.1 the god-vs-god separation uses: that 1.1 is a
/// *personal space* between two moving units, deliberately wider than either
/// body so a melee scrum does not overlap. Against static geometry the same
/// number would hold a cat half a body-width off every tree trunk, which
/// reads as an invisible wall. This is the body.
pub const GOD_RADIUS: f32 = 0.55;
/// A stalk covers ground at about a quarter of a run. Before this existed
/// there was no way to move slowly at all: `move_gods` normalises the move
/// direction, so a gentle stick and a full one were the same 12 u/s.
pub const SNEAK_SPEED: f32 = 0.26;
/// Attacking, casting or being hit suspends a stalk for this long.
pub const STALK_BREAK_SECS: f32 = 1.5;
/// Below this fraction of max HP a cat slinks: lower, quieter, no slower.
pub const WOUNDED_FRAC: f32 = 0.35;
/// Quiet seconds granted by a blink or a dash. A buff grants its own duration.
pub const HUSH_BLINK_SECS: f32 = 2.5;
pub const HUSH_DASH_SECS: f32 = 1.5;

/// Collision radius of a minion. Smaller because a minion is a mini cat.
pub const MINION_RADIUS: f32 = 0.32;
pub const GROUND_HALF: f32 = 225.0 * OUTER_SCALE; // rendered terrain half-extent (~279)

/// Base microvoxel edge length (world units) for character voxels.
pub const VOXEL_SIZE: f32 = 0.14;

// --- River -----------------------------------------------------------------
// Two winding rivers run north–south through the side jungle. They dip under
// the side lanes (wooden bridges span the crossings).

pub const RIVER_CENTER_X: f32 = 62.0 * MAP_SCALE;
pub const RIVER_AMPLITUDE: f32 = 12.0 * MAP_SCALE;
pub const RIVER_FREQ: f32 = 0.050;
pub const RIVER_PHASE: f32 = 0.8;
pub const RIVER_HALF_WIDTH: f32 = 5.2;

/// Signed river center for the left (side = -1) / right (side = 1) river.
pub fn river_x(side: f32, z: f32) -> f32 {
    side * (RIVER_CENTER_X + RIVER_AMPLITUDE * (z * RIVER_FREQ + RIVER_PHASE).sin())
}

/// Distance from (x, z) to the nearest river centerline (both rivers).
pub fn river_dist(x: f32, z: f32) -> f32 {
    match crate::map::active() {
        crate::map::MapId::Pass => return crate::pass::river_dist(x, z),
        crate::map::MapId::Sky => return crate::sky::river_dist(x, z),
        crate::map::MapId::Atlantis => return crate::atlantis::river_dist(x, z),
        crate::map::MapId::Maze => return crate::maze::river_dist(x, z),
        crate::map::MapId::Jungle => {}
    }
    (x - river_x(-1.0, z)).abs().min((x - river_x(1.0, z)).abs())
}

/// Still-water surface height. The riverbed dips below it (see
/// `render::terrain_height`); the banks rise above it.
pub const WATER_LEVEL: f32 = -0.30;
/// Deepest the bed goes below the water, mid-channel.
pub const RIVER_DEPTH: f32 = 2.1;

/// How deep the water is at (x, z), in world units (0 on dry ground).
pub fn water_depth(x: f32, z: f32) -> f32 {
    // over the sky map's void the "terrain" is the cloud sea, thirty units
    // down: that is not water. Only the Moonwell is.
    if crate::map::active() == crate::map::MapId::Sky && !crate::sky::in_pool(x, z) {
        return 0.0;
    }
    // the Drowned Halls are dry inside; the sea floor beyond the walls is
    // not walked
    if matches!(crate::map::active(), crate::map::MapId::Atlantis | crate::map::MapId::Maze) {
        return 0.0;
    }
    (WATER_LEVEL - crate::render::terrain_height(x, z)).max(0.0)
}

/// Riverblessing: what a caught glowfish grants, and for how long.
pub const BLESSING_SECS: f32 = 45.0;
pub const BLESSING_MS: f32 = 0.12;
pub const BLESSING_AS: f32 = 0.10;
pub const BLESSING_MP_PER_S: f32 = 0.01; // fraction of max mana

/// Movement multiplier for wading: waist-deep water costs ~40 %. Riverborn
/// ancestors and anyone carrying the Riverblessing ignore it.
pub fn wade_speed_mul(depth: f32) -> f32 {
    1.0 - 0.40 * (depth / 1.2).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Team {
    Blue = 0,
    Red = 1,
}

impl Team {
    pub fn other(self) -> Team {
        match self {
            Team::Blue => Team::Red,
            Team::Red => Team::Blue,
        }
    }
    /// Direction this team pushes along Z (Blue pushes north/+Z, Red south/-Z).
    pub fn push_z(self) -> f32 {
        match self {
            Team::Blue => 1.0,
            Team::Red => -1.0,
        }
    }
    /// Fountain (heal / respawn) position.
    pub fn fountain(self) -> Vec3 {
        match crate::map::active() {
            crate::map::MapId::Pass => return crate::pass::fountain(self),
            crate::map::MapId::Sky => return crate::sky::fountain(self),
            crate::map::MapId::Atlantis => return crate::atlantis::fountain(self),
            crate::map::MapId::Maze => return crate::maze::fountain(self),
            crate::map::MapId::Jungle => {}
        }
        match self {
            Team::Blue => Vec3::new(0.0, 0.0, -88.0 * MAP_SCALE),
            Team::Red => Vec3::new(0.0, 0.0, 88.0 * MAP_SCALE),
        }
    }
    pub fn titan_pos(self) -> Vec3 {
        match crate::map::active() {
            crate::map::MapId::Pass => return crate::pass::titan_pos(self),
            crate::map::MapId::Sky => return crate::sky::titan_pos(self),
            crate::map::MapId::Atlantis => return crate::atlantis::titan_pos(self),
            crate::map::MapId::Maze => return crate::maze::titan_pos(self),
            crate::map::MapId::Jungle => {}
        }
        match self {
            Team::Blue => Vec3::new(0.0, 0.0, -74.0 * MAP_SCALE),
            Team::Red => Vec3::new(0.0, 0.0, 74.0 * MAP_SCALE),
        }
    }
    pub fn base_pos(self) -> Vec3 {
        match crate::map::active() {
            crate::map::MapId::Pass => return crate::pass::base_pos(self),
            crate::map::MapId::Sky => return crate::sky::base_pos(self),
            crate::map::MapId::Atlantis => return crate::atlantis::base_pos(self),
            crate::map::MapId::Maze => return crate::maze::base_pos(self),
            crate::map::MapId::Jungle => {}
        }
        match self {
            Team::Blue => Vec3::new(0.0, 0.0, -82.0 * MAP_SCALE),
            Team::Red => Vec3::new(0.0, 0.0, 82.0 * MAP_SCALE),
        }
    }
    /// The two sides. Two-part clan-style names (the Warriors books'
    /// naming pattern, none of their actual clans), each saying its colour:
    /// ice-blue and ember-red. One table -- the HUD, scoreboard, announcer
    /// and intro card all read it.
    pub fn name(self) -> &'static str {
        match self {
            Team::Blue => "FROSTFANG",
            Team::Red => "EMBERCLAW",
        }
    }
    /// What the intro card says under the name.
    pub fn motto(self) -> &'static str {
        match self {
            Team::Blue => "cold, patient, and never alone",
            Team::Red => "quick to the hunt, first to the fire",
        }
    }
    pub fn rgb(self) -> [f32; 3] {
        match self {
            Team::Blue => [0.25, 0.55, 1.0],
            Team::Red => [1.0, 0.30, 0.24],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Lane {
    Top = 0,
    Mid = 1,
    Bot = 2,
}

pub const LANES: [Lane; 3] = [Lane::Top, Lane::Mid, Lane::Bot];

impl Lane {
    pub fn index(self) -> usize {
        self as usize
    }
    pub fn name(self) -> &'static str {
        if crate::map::active() == crate::map::MapId::Sky {
            // the sky map's two lanes run west and east
            match self {
                Lane::Top => return "WEST",
                Lane::Bot => return "EAST",
                Lane::Mid => return "WELL",
            }
        }
        match self {
            Lane::Top => "TOP",
            Lane::Mid => "MID",
            Lane::Bot => "BOT",
        }
    }
    /// Canonical polyline from Blue base toward Red base.
    pub fn polyline(self) -> Vec<Vec3> {
        match crate::map::active() {
            crate::map::MapId::Pass => return crate::pass::lane_polyline(self),
            crate::map::MapId::Sky => return crate::sky::lane_polyline(self),
            crate::map::MapId::Atlantis => return crate::atlantis::lane_polyline(self),
            crate::map::MapId::Maze => return crate::maze::lane_polyline(self),
            crate::map::MapId::Jungle => {}
        }
        match self {
            Lane::Mid => vec![
                Vec3::new(0.0, 0.0, -68.0),
                Vec3::new(0.0, 0.0, -34.0),
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 34.0),
                Vec3::new(0.0, 0.0, 68.0),
            ],
            Lane::Top => vec![
                Vec3::new(0.0, 0.0, -64.0),
                Vec3::new(-34.0, 0.0, -56.0),
                Vec3::new(-52.0, 0.0, -34.0),
                Vec3::new(-58.0, 0.0, 0.0),
                Vec3::new(-52.0, 0.0, 34.0),
                Vec3::new(-34.0, 0.0, 56.0),
                Vec3::new(0.0, 0.0, 64.0),
            ],
            Lane::Bot => vec![
                Vec3::new(0.0, 0.0, -64.0),
                Vec3::new(34.0, 0.0, -56.0),
                Vec3::new(52.0, 0.0, -34.0),
                Vec3::new(58.0, 0.0, 0.0),
                Vec3::new(52.0, 0.0, 34.0),
                Vec3::new(34.0, 0.0, 56.0),
                Vec3::new(0.0, 0.0, 64.0),
            ],
        }
        .into_iter()
        .map(|v| v * MAP_SCALE)
        .collect()
    }
    /// Dense waypoints (sampled every ~5 units) from this team's base to the enemy base.
    pub fn path_for(self, team: Team) -> Vec<Vec3> {
        let mut pts = self.polyline();
        if team == Team::Red {
            pts.reverse();
        }
        // prepend / append the bases so minions walk out of the fountain
        let start = team.base_pos() + Vec3::new(0.0, 0.0, team.push_z() * 10.0);
        let enemy = team.other();
        let end = enemy.base_pos() + Vec3::new(0.0, 0.0, enemy.push_z() * 10.0);
        let mut out = Vec::with_capacity(64);
        out.push(start);
        for p in pts {
            let last = *out.last().unwrap();
            push_sampled(&mut out, last, p, 5.0);
        }
        let last = *out.last().unwrap();
        push_sampled(&mut out, last, end, 5.0);
        out
    }
}

fn push_sampled(out: &mut Vec<Vec3>, from: Vec3, to: Vec3, step: f32) {
    let d = to - from;
    let len = d.length();
    if len < 0.01 {
        return;
    }
    let n = (len / step).ceil() as usize;
    for i in 1..=n {
        out.push(from + d * (i as f32 / n as f32));
    }
}

// TODO: the waypoints/minion system has to accomodate minions that are smarter and more natural that do patrols and then return to base
//       instead of running into the other territory. they watch the borders and prevent encroaching territory. the paths are more circular
//       for a PatrolType::LongCircular vs shorter in and out and back loops on PatrolType::OvalCenterAngle
//              .....................
//          ....                     ....
//      ....p1                           ....
//  ....      i                              ....
//                 o     
//          i            
//                       
//                    o  
//          i            
//                      o
//            i          
//                       
//                 i    o
//                       
//                     i 
//                      ...
//                      ...p0
//                      ...
//                      
//      this is PatrolType::OvalCenterAngle
//      it has a p0,p1
//      p0: center of territory (basecamp)
//      p1: patrol POI
//              .....................
//          ....                     ....
//      ....p1        i          i       ....
//  ....      i                          p2  ....
//                       
//          o                          i            
//                       
//                       
//          o            
//                                        i
//            o          
//                       
//                 o                i
//                       
//                     o       i
//                      ...
//                      ...p0
//                      ...
//                      
//      this is PatrolType::LongCircular
//      it has a p0,p1,p2
//      p0: center of territory (basecamp)
//      p1: patrol initial border contact POI
//      p2: start heading back POI
/// Cached per (lane, team) dense waypoints (6 combos, built once).
pub fn lane_path(lane: Lane, team: Team) -> &'static [Vec3] {
    use std::sync::OnceLock;
    // One cache per map: the paths are the map's.
    // (sized by MapId::ALL: a fixed 4 here would index out of bounds on a
    // fifth map at run time, which no compiler error flags)
    static PATHS: [OnceLock<Vec<Vec<Vec3>>>; crate::map::MapId::ALL.len()] = [const { OnceLock::new() }; crate::map::MapId::ALL.len()];
    let all = PATHS[crate::map::active() as usize].get_or_init(|| {
        let mut v = Vec::new();
        for lane in LANES {
            for team in [Team::Blue, Team::Red] {
                // Waypoints stand ON the ground: the pass's Top lane climbs a
                // 9 u shelf, and the AI and minions advance waypoints by 3D
                // distance (< 6 u, < 1.5 u) -- against points at y = 0 they
                // would never arrive up there, and stall. 0 on the jungle.
                let path = lane.path_for(team).into_iter().map(|p| Vec3::new(p.x, crate::map::ground_y(p.x, p.z), p.z)).collect();
                v.push(path);
            }
        }
        v
    });
    &all[lane.index() * 2 + team as usize]
}

/// (lane, team) → tower world position. Two towers per lane per team.
pub fn tower_pos(lane: Lane, team: Team, inner: bool) -> Vec3 {
    match crate::map::active() {
        crate::map::MapId::Pass => return crate::pass::tower_pos(lane, team, inner),
        crate::map::MapId::Sky => return crate::sky::tower_pos(lane, team, inner),
        crate::map::MapId::Atlantis => return crate::atlantis::tower_pos(lane, team, inner),
        crate::map::MapId::Maze => return crate::maze::tower_pos(lane, team, inner),
        crate::map::MapId::Jungle => {}
    }
    use Lane::*;
    let zflip = |v: Vec3| -> Vec3 {
        match team {
            Team::Blue => v,
            Team::Red => Vec3::new(v.x, v.y, -v.z),
        }
    };
    let pos = match (lane, inner) {
        (Mid, false) => Vec3::new(0.0, 0.0, -38.0),
        (Mid, true) => Vec3::new(0.0, 0.0, -60.0),
        (Top, false) => Vec3::new(-43.0, 0.0, -42.0),
        (Top, true) => Vec3::new(-50.0, 0.0, -52.0),
        (Bot, false) => Vec3::new(43.0, 0.0, -42.0),
        (Bot, true) => Vec3::new(50.0, 0.0, -52.0),
    };
    zflip(pos * MAP_SCALE)
}

// ---------------------------------------------------------------------------
// Tuning
// ---------------------------------------------------------------------------

pub const WAVE_FIRST: f32 = 12.0;
pub const WAVE_INTERVAL: f32 = 30.0;

// trying out a zero gold start
pub const START_GOLD: f32 = 0.0;
// pub const START_GOLD: f32 = 700.0;
pub const PASSIVE_GOLD: f32 = 1.9;
// Waves went from three minions to four (five every third, `minionai::
// wave_for`): per-minion gold and XP were cut by 0.7 so a wave is worth
// what it was (69 gold / 204 XP on average, before and after).
pub const GOLD_MELEE: f32 = 15.0;
pub const GOLD_CASTER: f32 = 19.0;
pub const GOLD_GOD: f32 = 300.0;
pub const GOLD_TOWER: f32 = 400.0;
// pub const GOLD_TOWER: f32 = 250.0;
/// Fraction of an item's cost refunded by `Game::sell_item`.
pub const SELL_REFUND: f32 = 0.7;

pub const XP_MELEE: f32 = 43.0;
pub const XP_CASTER: f32 = 56.0;
pub const XP_GOD: f32 = 340.0;
pub const XP_SHARE_RADIUS: f32 = 14.0;
/// Levels run 1 to 15 (`ranks`: every level ranks up an ability).
pub const MAX_LEVEL: u32 = crate::ranks::MAX;

pub const MINION_HP: f32 = 320.0;
pub const MINION_HP_SCALE: f32 = 1.0; // per minute factor
pub const MINION_DMG: f32 = 14.0;
pub const MINION_CASTER_DMG: f32 = 22.0;
pub const MINION_RANGE_MELEE: f32 = 2.2;
pub const MINION_RANGE_CASTER: f32 = 9.0;
pub const MINION_ATK_CD: f32 = 1.4;

/// Towers: 25% tougher and 25% harder-hitting than the 1450 / 168 they had
/// (a lane held a while longer, a dive under one paid for).
pub const TOWER_BUFF: f32 = 1.35; // the 1.75 felt too strong
// pub const TOWER_BUFF: f32 = 1.75; // the 1.25 felt weak after more gods were added and the ways to dive or group up increased
// pub const TOWER_BUFF: f32 = 1.25;
pub const TOWER_HP: f32 = 1450.0 * TOWER_BUFF;
pub const TOWER_DMG: f32 = 168.0 * TOWER_BUFF;
/// Was 16. Measured against what it has to answer: ranged autos reach 14.4
/// (1.6 u inside the old line), lobs 20, beams 22-32, and at 12.4 u/s a cat
/// crossed the old 32 u zone in 2.6 s -- two shots. At 24 the zone is 48 u
/// across: 3.9 s and four ramping shots to run past, and every auto and lob
/// is answered. Bolts (48-64 u) still out-range it, as they should: that is
/// poke at the DEFENDER, who now has cleared ground to stand on.
pub const TOWER_RANGE: f32 = 24.0;
/// Nothing that hides or lifts a cat -- no bush, fern, vine, rock or log --
/// grows inside this. The line a tower draws should be crossed in the open,
/// on purpose, not skimmed from a bush three paces outside it.
pub const TOWER_CLEARING: f32 = TOWER_RANGE + 4.0;
/// A tower's line is drawn once the player is within this of it.
pub const TOWER_RING_SHOW: f32 = 14.0;
/// A tower's shot slows a cat: diving past is a decision, not a jog.
pub const TOWER_SLOW: f32 = 0.25;
pub const TOWER_SLOW_SECS: f32 = 1.2;
// TODO: Towers need a buff in protections vs abilities because ultimates do too much damage... same with titans

/// Inside some tower's cleared ground?
pub fn in_tower_clearing(x: f32, z: f32) -> bool {
    crate::map::lanes().iter().any(|l| {
        [Team::Blue, Team::Red].iter().any(|t| [false, true].iter().any(|i| {
            let p = tower_pos(*l, *t, *i);
            (p.x - x).hypot(p.z - z) < TOWER_CLEARING
        }))
    })
}
pub const TOWER_ATK_CD: f32 = 1.1;
pub const TOWER_RAMP: f32 = 1.25;

/// The Elders (titans): 75% more health and damage than the 4600 / 260
/// they had -- the last stand should be one.
/// TODO: Titans need to be more fun to play against and use abilities and have phases
///       like how the starry sky map center enemy is when it flies around and bombs the lanes
///       maybe titans should also scale with number of cats in lobby
pub const TITAN_BUFF: f32 = 2.00;
// pub const TITAN_BUFF: f32 = 1.75;
pub const TITAN_HP: f32 = 4600.0 * TITAN_BUFF;
pub const TITAN_DMG: f32 = 260.0 * TITAN_BUFF;
pub const TITAN_RANGE: f32 = 6.5;
pub const TITAN_ATK_CD: f32 = 1.0;
/// The Elder's ground (`Game::titan_zone`): it may step this far from its
/// pedestal in any direction...
pub const TITAN_MOVE_R: f32 = 10.0;
/// ...and push forward, toward the enemy, this far, in a lane this wide
/// (half-width): it comes out to meet a siege, it does not chase.
// pub const TITAN_PUSH: f32 = 24.0;
// pub const TITAN_PUSH_HALF: f32 = 11.0;
pub const TITAN_PUSH: f32 = 32.0;
pub const TITAN_PUSH_HALF: f32 = 15.0;
/// It notices what is within this of it (inside its ground), walks at
/// this pace, and turns this fast (rad/s); it strikes only what it faces.
pub const TITAN_SIGHT: f32 = 22.0;
pub const TITAN_SPEED: f32 = 3.4;
pub const TITAN_TURN: f32 = 1.9;
pub const TITAN_FACE: f32 = 0.5;
/// Titan is shielded while the owning team still has this many towers standing.
/// TODO: try this 1 tree-tower gives immunity idea
pub const TITAN_TOWERS_REQ: usize = 1;
// pub const TITAN_TOWERS_REQ: usize = 4;

pub const FOUNTAIN_RADIUS: f32 = 10.0;
pub const FOUNTAIN_REGEN: f32 = 0.08; // fraction per second
pub const FOUNTAIN_DPS: f32 = 520.0;

pub const AGGRO_RANGE: f32 = 13.0;
pub const RESPAWN_BASE: f32 = 6.0;
pub const RESPAWN_PER_LEVEL: f32 = 2.2;

/// Distance from (x, z) to the nearest lane centerline, and negative distance
/// to each base (props should stay clear of both). Shared by world dressing.
///
/// The centreline is the lane's polyline PLUS the stretch each team's minions walk from the
/// fountain to it (`Lane::path_for` prepends and appends the same two points). That stretch
/// was not counted: 24 u at MAP_SCALE 1.42, where the scatter's 11 u keep-out happened to
/// cover it, and 80 u at 3.75, where a grove's bramble bed sat on the Top lane's first
/// waypoints and `the_lanes_are_open_going_and_the_jungle_is_not` found minions wading it.
pub fn lane_clearance(x: f32, z: f32) -> f32 {
    let p = Vec3::new(x, 0.0, z);
    let mut best = f32::MAX;
    for &lane in crate::map::lanes() {
        let mut pts = lane.polyline();
        pts.insert(0, Team::Blue.base_pos() + Vec3::new(0.0, 0.0, Team::Blue.push_z() * 10.0));
        pts.push(Team::Red.base_pos() + Vec3::new(0.0, 0.0, Team::Red.push_z() * 10.0));
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(0.001)).clamp(0.0, 1.0);
            best = best.min((a + ab * t - p).length());
        }
    }
    for team in [Team::Blue, Team::Red] {
        best = best.min(p.distance(team.base_pos()) - 16.0);
    }
    best
}

pub fn xp_for_level(level: u32) -> f32 {
    // cumulative XP needed to go from `level` to `level + 1`
    100.0 + 62.0 * level as f32 + 7.0 * level as f32 * level as f32
}

pub fn respawn_time(level: u32) -> f32 {
    RESPAWN_BASE + RESPAWN_PER_LEVEL * level as f32
}
