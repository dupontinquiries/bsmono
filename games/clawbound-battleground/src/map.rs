//! map — which map this thread is playing.
//!
//! The jungle map was the only map, so its shape lives in free functions
//! (`Lane::polyline`, `Team::base_pos`, `tower_pos`, `river_dist`,
//! `render::terrain_height`) called from some two hundred places. A second
//! map changes what those functions answer, not who calls them: each checks
//! [`active`] and hands over to the map's own module (`pass` for Twinfang
//! Pass) when it is not the jungle.
//!
//! ## Why thread-local, and the alternative
//!
//! Set once, before the `Scene` and `Game` are built, by whatever starts a
//! match (the app, the headless harness, a test). Thread-local rather than
//! global so parallel tests can play different maps. Every consumer of map
//! geometry -- sim, scene build, draw -- runs on the thread that set it.
//!
//! The cleaner end state is an explicit `Arc<Map>` on `Game` and `Scene`,
//! threaded through every call; this seam is where that refactor would
//! start, and the map modules would not change when it happens.

use std::cell::Cell;

use glam::Vec3;

/// When the sky map's overtime begins, match seconds (see [`MapId::overtime`]).
pub const OVERTIME_AT: f32 = 18.0 * 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum MapId {
    /// The 5v5 jungle valley.
    #[default]
    Jungle,
    /// Twinfang Pass: the mountain canyon map, 15 a side.
    Pass,
    /// Starwell Glacier: two wide lanes over glacier islands in the sky,
    /// 15 a side (`sky`).
    Sky,
    /// The Drowned Halls: 3 a side on the sea floor, one lane hall and one
    /// jungle rotunda, walled and domed (`atlantis`).
    Atlantis,
    /// The Thornmaze: 5 a side in a great walled labyrinth, two winding
    /// lanes, and sections that wake and turn on whoever walks them alone
    /// (`maze`).
    Maze,
}

impl MapId {
    pub const ALL: [MapId; 5] = [MapId::Jungle, MapId::Pass, MapId::Sky, MapId::Atlantis, MapId::Maze];

    pub fn name(self) -> &'static str {
        match self {
            MapId::Jungle => "The Jungle",
            MapId::Pass => "Twinfang Pass",
            MapId::Sky => "Starwell Glacier",
            MapId::Atlantis => "The Drowned Halls",
            MapId::Maze => "The Thornmaze",
        }
    }
    /// Command-line / config key.
    pub fn key(self) -> &'static str {
        match self {
            MapId::Jungle => "jungle",
            MapId::Pass => "pass",
            MapId::Sky => "starwell",
            MapId::Atlantis => "atlantis",
            MapId::Maze => "thornmaze",
        }
    }
    pub fn from_key(s: &str) -> Option<MapId> {
        MapId::ALL.into_iter().find(|m| m.key().eq_ignore_ascii_case(s.trim()))
    }
    /// Cats per team.
    pub fn team_size(self) -> usize {
        match self {
            MapId::Jungle => 5,
            MapId::Pass => crate::pass::TEAM_SIZE,
            MapId::Sky => crate::sky::TEAM_SIZE,
            MapId::Atlantis => crate::atlantis::TEAM_SIZE,
            MapId::Maze => crate::maze::TEAM_SIZE,
        }
    }
    /// The lanes minions walk and towers stand on. The sky map has two:
    /// `Lane::Mid` exists there only as a camera route.
    pub fn lanes(self) -> &'static [crate::consts::Lane] {
        use crate::consts::Lane;
        match self {
            MapId::Jungle | MapId::Pass => &crate::consts::LANES,
            MapId::Sky | MapId::Maze => &[Lane::Top, Lane::Bot],
            MapId::Atlantis => &[Lane::Mid],
        }
    }
    /// Minions per side in each lane's wave, (melee, casters), for wave
    /// number `n` (`minionai::wave_for` on the smaller maps).
    pub fn wave(self, n: u32) -> (usize, usize) {
        match self {
            MapId::Jungle | MapId::Pass | MapId::Atlantis | MapId::Maze => crate::minionai::wave_for(n),
            // thirty-two a side in each lane (sixty-four when the waves meet)
            MapId::Sky => (24, 8),
        }
    }
    /// Towers and Elders are this much tougher than `TOWER_HP`/`TITAN_HP`.
    /// The sky map's waves are eight times the jungle's (32 a side): at x1
    /// a broken lane let one wave kill the Elder (AI-only matches ended at
    /// three minutes, with 16-a-side waves); at x3 with 32 a side, all
    /// three stalled to the cap.
    pub fn structure_hp_mul(self) -> f32 {
        match self {
            MapId::Jungle | MapId::Pass | MapId::Atlantis | MapId::Maze => 1.0,
            MapId::Sky => 2.0,
        }
    }
    /// Minion damage to towers and Elders, on top of the usual x2.4. The sky
    /// map's waves are eight times the jungle's; at x1 a wave that reached
    /// an open Elder killed it in seconds (AI-only matches won at 2:13).
    /// At 0.3 a sky wave hits a structure about twice as hard as a jungle
    /// wave does, against twice the health.
    pub fn minion_structure_mul(self) -> f32 {
        match self {
            MapId::Jungle | MapId::Pass | MapId::Atlantis | MapId::Maze => 1.0,
            MapId::Sky => 0.3,
        }
    }
    /// Siege surge: a team's minions in a lane hit and last this much
    /// harder for each enemy tower already fallen in that lane. On the sky
    /// map, 32-a-side waves otherwise cancel each other in the middle
    /// forever: AI-only matches stalled at 30 min with two towers a side
    /// standing (the super-minion answer, cut down). 0 elsewhere.
    pub fn siege_surge(self) -> f32 {
        match self {
            MapId::Jungle | MapId::Pass | MapId::Atlantis | MapId::Maze => 0.0,
            MapId::Sky => 0.4,
        }
    }
    /// Overtime: every blow on a tower or Elder is multiplied by this at
    /// match time `t`. On the sky map, 32-a-side waves and full-health
    /// Elders let AI-only matches run to the 30-minute cap even with every
    /// tower down (probe: Elders at 7290 and 2592 of 9200 at 30:00). From
    /// `OVERTIME_AT` the multiplier climbs 12% a minute, to x4. 1 elsewhere.
    pub fn overtime(self, t: f32) -> f32 {
        match self {
            MapId::Jungle | MapId::Pass | MapId::Atlantis | MapId::Maze => 1.0,
            MapId::Sky => (1.0 + (t - OVERTIME_AT).max(0.0) / 60.0 * 0.12).min(4.0),
        }
    }
    /// The play camera's pitch at the start of a match (radians down from
    /// level). Steeper in the Thornmaze: at the usual 0.38 the camera sat
    /// in its walls whenever a cat backed toward one; from higher it looks
    /// down into the rooms and over the walls (`camera_boom` does the rest).
    pub fn cam_pitch(self) -> f32 {
        match self {
            MapId::Maze => 0.62,
            _ => 0.38,
        }
    }
    /// A team's Elder is shielded while it has at least this many towers
    /// standing: all but three of six on the three-lane maps, all but two
    /// of four on the sky.
    pub fn titan_towers_req(self) -> usize {
        match self {
            MapId::Jungle | MapId::Pass => crate::consts::TITAN_TOWERS_REQ,
            // two lanes, two towers a side each: open when two fall
            MapId::Sky | MapId::Maze => 3,
            // one lane, two towers a side: the Elder opens when both fall
            MapId::Atlantis => 1,
        }
    }
}

/// The active map's lanes (`MapId::lanes`).
pub fn lanes() -> &'static [crate::consts::Lane] {
    active().lanes()
}

thread_local! {
    static ACTIVE: Cell<MapId> = const { Cell::new(MapId::Jungle) };
}

/// The map this thread is playing.
pub fn active() -> MapId {
    ACTIVE.with(|a| a.get())
}

/// Choose the map for this thread. Call before building a `Scene` or `Game`.
pub fn set_active(m: MapId) {
    ACTIVE.with(|a| a.set(m));
}

/// Run `f` with `m` active, restoring the previous map after. For tests and
/// tools that visit a map without leaving the thread on it.
pub fn with<R>(m: MapId, f: impl FnOnce() -> R) -> R {
    let prev = active();
    set_active(m);
    struct Restore(MapId);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_active(self.0);
        }
    }
    let _r = Restore(prev);
    f()
}

/// Is `(x, z)` open ground a unit of `margin` fits on? Everywhere inside
/// the arena on the jungle map (props are `Blockers`' business); the carved
/// floor on the pass. For anything that spawns or places at a random point:
/// wildlife, camps, boss grounds.
pub fn open_ground(x: f32, z: f32, margin: f32) -> bool {
    match active() {
        MapId::Jungle => true,
        MapId::Pass => crate::pass::floor().fits(x, z, margin),
        MapId::Sky => crate::sky::floor().fits(x, z, margin),
        MapId::Atlantis => crate::atlantis::floor().fits(x, z, margin),
        MapId::Maze => crate::maze::floor().fits(x, z, margin),
    }
}

/// Height of the ground at (x, z): where a grounded unit's feet are. 0
/// everywhere on the jungle; the pass's climbing Top lane rises onto a
/// shelf (`pass::floor_height`).
pub fn ground_y(x: f32, z: f32) -> f32 {
    match active() {
        MapId::Jungle => 0.0,
        MapId::Pass => crate::pass::floor_height(x, z),
        MapId::Sky => crate::sky::floor_height(x, z),
        MapId::Atlantis => crate::atlantis::floor_height(x, z),
        MapId::Maze => crate::maze::floor_height(x, z),
    }
}

/// How far above the ground a grounded unit may find itself and be SET on
/// it rather than fall: walking down a ramp. 0 on the jungle (unchanged:
/// anything above the floor falls).
pub fn step_down() -> f32 {
    match active() {
        MapId::Jungle | MapId::Sky | MapId::Atlantis | MapId::Maze => 0.0,
        MapId::Pass => 0.8,
    }
}

/// The floor plan that is the map's collision boundary, if it has one.
pub fn floor() -> Option<&'static crate::floor::FloorPlan> {
    match active() {
        MapId::Jungle => None,
        MapId::Pass => Some(crate::pass::floor()),
        MapId::Sky => Some(crate::sky::floor()),
        MapId::Atlantis => Some(crate::atlantis::floor()),
        MapId::Maze => Some(crate::maze::floor()),
    }
}

/// A point clamped onto open ground (identity on the jungle).
pub fn onto_floor(p: Vec3, radius: f32) -> Vec3 {
    match floor() {
        None => p,
        Some(f) => {
            let (x, z) = f.push_in(p.x, p.z, radius);
            Vec3::new(x, p.y, z)
        }
    }
}

/// Rosters for a match on the active map. The player's ancestor is
/// `blue[0]`. On the jungle every cat is a different ancestor; on the pass,
/// 30 cats from 13 lines, each line spread as evenly as the numbers allow
/// (two or three of each), and no side fields three of one line while the
/// other fields one.
pub fn team_picks(player: u32, rng: &mut crate::sim::Rng) -> (Vec<u32>, Vec<u32>) {
    let n = crate::gods::ROSTER_LEN as u32;
    let size = active().team_size();
    if size <= 5 {
        let mut pool: Vec<u32> = (0..n).filter(|&i| i != player).collect();
        let mut draw = |rng: &mut crate::sim::Rng| pool.remove((rng.next_u() % pool.len() as u64) as usize);
        let mut blue = vec![player];
        for _ in 1..size {
            blue.push(draw(rng));
        }
        let red = (0..size).map(|_| draw(rng)).collect();
        return (blue, red);
    }
    // Every line once per side, then the remainder (30 - 2 * 13 = 4 extra
    // cats) from distinct lines split evenly between the sides. A line is
    // then within one cat of even across the two sides BY CONSTRUCTION;
    // dealing alternately off a shuffled deck, the first version, fielded a
    // line three to nil.
    let shuffle = |v: &mut Vec<u32>, rng: &mut crate::sim::Rng| {
        for i in (1..v.len()).rev() {
            let j = (rng.next_u() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    };
    let per_side_once = (size as u32).min(n);
    let mut lines: Vec<u32> = (0..n).collect();
    shuffle(&mut lines, rng);
    let mut blue_rest: Vec<u32> = lines.iter().copied().take(per_side_once as usize).filter(|&l| l != player).collect();
    let mut red: Vec<u32> = lines.iter().copied().take(per_side_once as usize).collect();
    if !lines[..per_side_once as usize].contains(&player) {
        // the player's line was not in the once-each set: it replaces one
        blue_rest.pop();
    }
    // extras from lines not yet doubled, alternating sides
    let mut extra: Vec<u32> = (0..n).collect();
    shuffle(&mut extra, rng);
    let mut to_blue = true;
    for l in extra {
        if blue_rest.len() + 1 >= size && red.len() >= size {
            break;
        }
        if to_blue && blue_rest.len() + 1 < size {
            blue_rest.push(l);
        } else if red.len() < size {
            red.push(l);
        } else {
            blue_rest.push(l);
        }
        to_blue = !to_blue;
    }
    shuffle(&mut blue_rest, rng);
    shuffle(&mut red, rng);
    let mut blue = vec![player];
    blue.extend(blue_rest);
    (blue, red)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_and_the_default_is_the_jungle() {
        for m in MapId::ALL {
            assert_eq!(MapId::from_key(m.key()), Some(m));
        }
        assert_eq!(MapId::from_key(" PASS "), Some(MapId::Pass));
        assert_eq!(MapId::from_key("moon"), None);
        assert_eq!(active(), MapId::Jungle);
    }

    /// The sky map: two lanes, four towers a side that take more of a
    /// beating, an Elder that opens when two fall, and waves of 32 a side.
    #[test]
    fn the_sky_map_has_its_own_rules() {
        use crate::consts::{Lane, TITAN_HP, TOWER_HP};
        with(MapId::Sky, || {
            let g = crate::sim::Game::new_teams(&[0], &[1]);
            assert_eq!(lanes(), &[Lane::Top, Lane::Bot]);
            assert_eq!(g.towers.len(), 8);
            assert_eq!(g.tower_alive, [4, 4]);
            let mul = MapId::Sky.structure_hp_mul();
            assert!(mul > 1.0);
            assert!(g.towers.iter().all(|t| (t.max_hp - TOWER_HP * mul).abs() < 1e-3));
            assert!((g.titans[0].hp - TITAN_HP * mul).abs() < 1e-3 && (g.titan_max_hp() - TITAN_HP * mul).abs() < 1e-3);
            // the siege surge: none while every tower stands
            assert_eq!(g.lane_surge(crate::consts::Team::Blue, Lane::Top), 1.0);
            let (m, c) = MapId::Sky.wave(0);
            assert_eq!(m + c, 32, "thirty-two a side in each lane");
            assert!(g.titan_shielded(crate::consts::Team::Blue));
        });
        let g = crate::sim::Game::new_teams(&[0], &[1]);
        assert_eq!((g.towers.len(), g.tower_alive), (12, [6, 6]), "the jungle is unchanged");
        assert!((g.titans[0].hp - crate::consts::TITAN_HP).abs() < 1e-3);
    }

    #[test]
    fn overtime_weakens_structures_on_the_sky_only() {
        use crate::sim::TargetRef;
        let hit = |t: f32| {
            let mut g = crate::sim::Game::new_teams(&[0], &[1]);
            g.t = t;
            let before = g.towers[0].hp;
            g.deal_damage(TargetRef::Tower(0), 100.0, false, None, 0.0, 0.0);
            before - g.towers[0].hp
        };
        let (early, late, very) = with(MapId::Sky, || (hit(60.0), hit(OVERTIME_AT + 300.0), hit(OVERTIME_AT + 3600.0)));
        assert!((early - 100.0).abs() < 1e-3, "no overtime before {OVERTIME_AT}s: {early}");
        assert!((late - 160.0).abs() < 1e-2, "five minutes in, x1.6: {late}");
        assert!((very - 400.0).abs() < 1e-2, "capped at x4: {very}");
        assert!((hit(OVERTIME_AT + 300.0) - 100.0).abs() < 1e-3, "the jungle has no overtime");
    }

    #[test]
    fn with_restores_even_on_panic() {
        let r = std::panic::catch_unwind(|| with(MapId::Pass, || panic!("inside")));
        assert!(r.is_err());
        assert_eq!(active(), MapId::Jungle);
        assert_eq!(with(MapId::Pass, active), MapId::Pass);
        assert_eq!(active(), MapId::Jungle);
    }

    #[test]
    fn team_picks_fill_the_map_size_and_spread_the_lines() {
        let mut rng = crate::sim::Rng::new(3);
        let (b, r) = team_picks(4, &mut rng);
        assert_eq!((b.len(), r.len()), (5, 5));
        let mut all: Vec<u32> = b.iter().chain(&r).copied().collect();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 10, "jungle: ten different ancestors");
        with(MapId::Pass, || {
            for seed in 0..50 {
            let mut rng = crate::sim::Rng::new(seed);
            let (b, r) = team_picks((seed % 13) as u32, &mut rng);
            assert_eq!((b.len(), r.len()), (15, 15));
            assert_eq!(b[0], (seed % 13) as u32);
            let n = crate::gods::ROSTER_LEN as u32;
            // with more lines than a side has cats (16 to 15), a line sits
            // the match out -- and when it is the player's own line that
            // was left out of the once-each deal, the player's cat takes a
            // blue seat and one more line is on red alone
            let (mut absent, mut one_sided) = (0, 0);
            for line in 0..n {
                let cb = b.iter().filter(|&&x| x == line).count() as i32;
                let cr = r.iter().filter(|&&x| x == line).count() as i32;
                assert!(cb + cr <= 4, "line {line}: {cb}+{cr}");
                absent += (cb + cr == 0) as u32;
                one_sided += (cb + cr == 1) as u32;
                assert!((cb - cr).abs() <= 1, "seed {seed}: line {line} lopsided: {cb} v {cr}");
            }
            assert!(absent <= n.saturating_sub(15) && one_sided <= 2, "seed {seed}: {absent} lines sat out, {one_sided} on one side");
            }
        });
    }

    #[test]
    fn open_ground_is_the_floor_on_the_pass_only() {
        assert!(open_ground(60.0, 0.0, 1.0), "jungle: the peak's site is open");
        with(MapId::Pass, || {
            assert!(!open_ground(60.0, 0.0, 1.0), "pass: that is the east peak");
            assert!(open_ground(0.0, 0.0, 1.0), "the gorge");
        });
    }
}

/// The visible surface's height here: the terrain (rock included) or the
/// walkable floor, whichever is higher.
pub fn surface_y(x: f32, z: f32) -> f32 {
    crate::render::terrain_height(x, z).max(ground_y(x, z))
}

/// Where a ray from `eye` along `dir` first meets the surface, within
/// `max_t`: marched in steps, the crossing then bisected. Aiming used the
/// plane y = 0 -- on the pass's shelf (9 u up) and slopes the aim landed
/// somewhere else entirely.
pub fn ray_ground(eye: Vec3, dir: Vec3, max_t: f32) -> Option<Vec3> {
    if !(eye.is_finite() && dir.is_finite()) || dir.length_squared() < 1e-8 {
        return None;
    }
    let dir = dir.normalize();
    let above = |t: f32| {
        let p = eye + dir * t;
        p.y - surface_y(p.x, p.z)
    };
    if above(0.0) < 0.0 {
        return None; // the eye is inside the ground
    }
    let (mut lo, mut t) = (0.0f32, 0.4f32);
    while t <= max_t {
        if above(t) <= 0.0 {
            let mut hi = t;
            for _ in 0..10 {
                let mid = (lo + hi) * 0.5;
                if above(mid) > 0.0 {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let p = eye + dir * hi;
            return Some(Vec3::new(p.x, surface_y(p.x, p.z), p.z));
        }
        lo = t;
        t += 0.4 + t * 0.01;
    }
    None
}

/// How far behind the cat the play camera may sit: `dist`, or less if the
/// terrain rises between the cat's head (`target`) and the camera, so the
/// camera comes in over the cat rather than sitting inside a wall. `dir` is
/// the view direction (from the camera to the target). Never nearer than
/// [`BOOM_MIN`].
pub fn camera_boom(target: Vec3, dir: Vec3, dist: f32) -> f32 {
    if !(target.is_finite() && dir.is_finite()) || dist <= BOOM_MIN {
        return dist;
    }
    let back = -dir.normalize_or_zero();
    let mut d = 0.3;
    while d <= dist {
        let p = target + back * d;
        if p.y < surface_y(p.x, p.z) + BOOM_CLEAR {
            return (d - 0.6).max(BOOM_MIN);
        }
        d += 0.25;
    }
    dist
}

/// The camera boom's shortest, and how far it keeps above the surface.
pub const BOOM_MIN: f32 = 2.5;
pub const BOOM_CLEAR: f32 = 0.5;

/// Is the straight line from `a` to `b` clear of the terrain? Sampled
/// along its length (the ends excluded, so a point ON the ground still
/// sees). For health bars behind rock, and cameras looking at a fight.
pub fn line_clear(a: Vec3, b: Vec3) -> bool {
    let n = ((b - a).length() / 1.5).ceil().clamp(4.0, 120.0) as usize;
    (1..n).all(|k| {
        let p = a.lerp(b, k as f32 / n as f32);
        p.y > surface_y(p.x, p.z) - 0.2
    })
}

#[cfg(test)]
mod ray_tests {
    use super::*;

    /// On flat ground the march agrees with the plane; on the pass's shelf
    /// it finds the shelf (9 u up), where the plane found a point far past it.
    #[test]
    fn the_aim_ray_meets_the_real_ground() {
        with(MapId::Jungle, || {
            let eye = Vec3::new(3.0, 12.0, -60.0);
            let dir = Vec3::new(0.0, -0.6, 1.0);
            let hit = ray_ground(eye, dir, 200.0).unwrap();
            let t = -(eye.y - surface_y(hit.x, hit.z)) / dir.normalize().y;
            assert!((hit - (eye + dir.normalize() * t)).length() < 0.6, "flat: agrees with the plane: {hit}");
        });
        with(MapId::Pass, || {
            // a point on the Top lane's raised shelf
            let pts = crate::pass::lane_polyline(crate::consts::Lane::Top);
            let mut shelf = None;
            let n = pts.len();
            for k in 0..n * 20 {
                let a = pts[(k / 20).min(n - 2)];
                let b = pts[(k / 20 + 1).min(n - 1)];
                let p = a.lerp(b, (k % 20) as f32 / 20.0);
                if ground_y(p.x, p.z) > 8.0 {
                    shelf = Some(p);
                    break;
                }
            }
            let target = shelf.expect("a shelf point");
            let target = Vec3::new(target.x, ground_y(target.x, target.z), target.z);
            let eye = target + Vec3::new(0.0, 14.0, -10.0);
            let hit = ray_ground(eye, target - eye, 200.0).unwrap();
            assert!((hit - target).length() < 1.0, "on the shelf: {hit} vs {target}");
            // the old plane intersection: far off
            let d = (target - eye).normalize();
            let plane = eye + d * (-eye.y / d.y);
            assert!((plane - target).length() > 5.0, "the plane missed by {}", (plane - target).length());
        });
    }

    /// The camera boom: out in the open it keeps its length; with a
    /// maze wall behind the cat it comes in short of the wall, and no
    /// nearer than its minimum.
    #[test]
    fn the_camera_does_not_sit_in_a_wall() {
        with(MapId::Maze, || {
            let f = crate::maze::floor();
            // a spot on the floor with a wall close behind it along -z
            let mut found = None;
            'scan: for x in (-200..=200).step_by(2) {
                for z in (-200..=200).step_by(2) {
                    let (x, z) = (x as f32, z as f32);
                    if f.sdf(x, z) < -2.0 && f.sdf(x, z - 5.0) > 1.0 {
                        found = Some(Vec3::new(x, 1.5, z));
                        break 'scan;
                    }
                }
            }
            let target = found.expect("a wall behind some floor");
            let dir = Vec3::new(0.0, -0.38f32.sin(), 0.38f32.cos()); // looking +z
            let d = camera_boom(target, dir, 8.5);
            assert!(d < 5.0 && d >= BOOM_MIN, "boom {d} with a wall 5 u behind");
            let eye = target - dir.normalize() * d;
            assert!(eye.y > surface_y(eye.x, eye.z), "the camera is in the wall at {eye}");
        });
        with(MapId::Jungle, || {
            let dir = Vec3::new(0.0, -0.38f32.sin(), 0.38f32.cos());
            assert_eq!(camera_boom(Vec3::new(0.0, 1.5, 0.0), dir, 8.5), 8.5, "open ground: full length");
        });
    }

    #[test]
    fn a_line_through_rock_is_not_clear() {
        with(MapId::Pass, || {
            // from open floor, through the massif at the centre of the map
            let f = crate::pass::floor();
            let mut open = None;
            for x in (-150..=150).step_by(5) {
                for z in (-150..=150).step_by(5) {
                    if open.is_none() && f.sdf(x as f32, z as f32) < -8.0 {
                        open = Some(Vec3::new(x as f32, ground_y(x as f32, z as f32) + 1.5, z as f32));
                    }
                }
            }
            let a = open.unwrap();
            assert!(line_clear(a, a + Vec3::new(0.0, 0.0, 0.5)), "a short open line");
            // straight at a point deep in rock: a peak's height
            let peak = Vec3::new(0.0, 1.0, 0.0);
            let mut into_rock = None;
            for x in (-200..=200).step_by(4) {
                for z in (-200..=200).step_by(4) {
                    if into_rock.is_none() && surface_y(x as f32, z as f32) > 40.0 {
                        into_rock = Some(Vec3::new(x as f32, 1.0, z as f32));
                    }
                }
            }
            let _ = peak;
            assert!(!line_clear(a, into_rock.expect("a peak")), "a line into a peak is blocked");
        });
    }
}
