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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum MapId {
    /// The 5v5 jungle valley.
    #[default]
    Jungle,
    /// Twinfang Pass: the mountain canyon map, 15 a side.
    Pass,
}

impl MapId {
    pub const ALL: [MapId; 2] = [MapId::Jungle, MapId::Pass];

    pub fn name(self) -> &'static str {
        match self {
            MapId::Jungle => "The Jungle",
            MapId::Pass => "Twinfang Pass",
        }
    }
    /// Command-line / config key.
    pub fn key(self) -> &'static str {
        match self {
            MapId::Jungle => "jungle",
            MapId::Pass => "pass",
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
        }
    }
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
    }
}

/// Height of the ground at (x, z): where a grounded unit's feet are. 0
/// everywhere on the jungle; the pass's climbing Top lane rises onto a
/// shelf (`pass::floor_height`).
pub fn ground_y(x: f32, z: f32) -> f32 {
    match active() {
        MapId::Jungle => 0.0,
        MapId::Pass => crate::pass::floor_height(x, z),
    }
}

/// How far above the ground a grounded unit may find itself and be SET on
/// it rather than fall: walking down a ramp. 0 on the jungle (unchanged:
/// anything above the floor falls).
pub fn step_down() -> f32 {
    match active() {
        MapId::Jungle => 0.0,
        MapId::Pass => 0.8,
    }
}

/// The floor plan that is the map's collision boundary, if it has one.
pub fn floor() -> Option<&'static crate::floor::FloorPlan> {
    match active() {
        MapId::Jungle => None,
        MapId::Pass => Some(crate::pass::floor()),
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
            for line in 0..n {
                let cb = b.iter().filter(|&&x| x == line).count() as i32;
                let cr = r.iter().filter(|&&x| x == line).count() as i32;
                assert!(cb + cr >= 2 && cb + cr <= 4, "line {line}: {cb}+{cr}");
                assert!((cb - cr).abs() <= 1, "seed {seed}: line {line} lopsided: {cb} v {cr}");
            }
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
