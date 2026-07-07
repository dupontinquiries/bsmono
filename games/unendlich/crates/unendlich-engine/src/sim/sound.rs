//! Sound propagation + occlusion for mob hearing.
//!
//! A [`SoundSource`] (footstep, block break, collision) emits loudness at a
//! point. A listener hears it at `loudness × distance_attenuation ×
//! occlusion`, where occlusion traces the line from source to listener through
//! the voxel grid and muffles per solid block crossed. Mobs compare the result
//! to a hearing threshold to decide whether — and roughly where — they noticed
//! something.
//!
//! ## Why integer math
//!
//! Hearing feeds mob AI, which lives in the deterministic, rollback-safe sim.
//! So everything here is integer / [`Fixed`]: voxel coords via `floor_int`,
//! distance via an integer sqrt, occlusion as a right-shift per wall. No
//! floats, no allocation — the trace is O(longest-axis delta), cheap enough to
//! re-run every tick for moving sources.
//!
//! ## Occlusion trace
//!
//! [`count_solids_on_line`] is a 3D driving-axis Bresenham walk between the two
//! voxels. It visits one voxel per step along the dominant axis (not the full
//! supercover set) — accurate enough to tell "clear line" from "wall between
//! us" while staying a single cheap pass. Each solid voxel strictly between
//! source and listener halves the perceived loudness, capped at
//! [`MAX_OCCLUDING_BLOCKS`].

use super::fixed::Vec3;
use super::physics::WorldQuery;

/// Emitted loudness in arbitrary integer units. Rough guide: footstep ~200,
/// block break ~600, explosion ~1000.
pub type Loudness = i32;

/// Distance in blocks at which loudness halves (before occlusion). Larger =
/// the sound carries farther.
pub const HEAR_FALLOFF_BLOCKS: i32 = 24;

/// Cap on how many solid blocks count toward occlusion. Each counted block
/// halves perceived loudness (a right-shift); the cap keeps a thick wall from
/// shifting to zero pointlessly and bounds the (already cheap) trace's effect.
pub const MAX_OCCLUDING_BLOCKS: i32 = 8;

/// A noise emitted in the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundSource {
    pub pos: Vec3,
    pub loudness: Loudness,
}

impl SoundSource {
    pub fn new(pos: Vec3, loudness: Loudness) -> Self {
        Self { pos, loudness }
    }
}

/// Loudness a moving body emits as footfalls — the noise a hunter tracks. It
/// scales with `mass` (a heavier creature lands harder; player ≈ 70), with how
/// fast it's moving (`speed_milli` = blocks/tick × 1000), and with how noisy the
/// ground is (`ground_loudness` 0..=255: bare stone and dry leaves are loud,
/// moss and sand are quiet). A still or sneaking body on soft ground is nearly
/// silent. Calibrated so a typical stride (mass 70, ~0.2 blk/tick, mid ground)
/// lands near the documented footstep ≈ 200.
pub fn footstep_loudness(mass: u16, speed_milli: u32, ground_loudness: u8) -> Loudness {
    if speed_milli == 0 {
        return 0; // not moving → no footfall
    }
    let momentum = mass as i64 * speed_milli as i64;
    let ground = ground_loudness as i64 + 16; // soft floor so any step makes some sound
    ((momentum * ground) / 10_000) as Loudness
}

/// How loud a footfall on a given surface is, before mass/speed scaling — the
/// `ground_loudness` term of [`footstep_loudness`]. Soft naturals muffle a step;
/// loose or hard mineral surfaces ring out; leaves rustle; water splashes. This
/// is what makes the ground you choose part of stealth: cross grass to stay
/// quiet, and gravel or stone gives you away.
pub fn surface_loudness(block: crate::world::blocks::BlockId) -> u8 {
    use crate::world::blocks::BlockId::*;
    match block {
        Snow | SnowBlock => 4,
        Grass | Moss => 6,
        Clay | Sand => 10,
        Dirt => 12,
        OakLeaves | BirchLeaves => 16, // rustle
        Ice => 18,                     // squeak / crack
        OakLog | BirchLog => 26,       // hollow wood knock
        Water => 30,                   // splash
        Stone | Andesite | Basalt | Tuff | Sandstone | StoneBricks | Bricks => 34,
        Gravel => 42, // loose and loud
        _ => 20,      // unclassified mid surface
    }
}

/// [`footstep_loudness`] with the surface looked up from the block underfoot.
pub fn footstep_loudness_on(mass: u16, speed_milli: u32, ground: crate::world::blocks::BlockId) -> Loudness {
    footstep_loudness(mass, speed_milli, surface_loudness(ground))
}

/// A mover's body stance, which scales how loud its footfalls are. Crouching and
/// crawling deliberately trade speed for quiet — the core of stalking or hiding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stance {
    Stand,
    Crouch,
    Prone,
}

impl Stance {
    /// Footfall loudness multiplier, in permille (1000 = unchanged). Standing is
    /// full volume; a crouch muffles most of it; going prone (a belly-crawl) is
    /// quieter still.
    pub fn noise_permille(self) -> i64 {
        match self {
            Stance::Stand => 1000,
            Stance::Crouch => 350,
            Stance::Prone => 150,
        }
    }
}

/// [`footstep_loudness`] scaled by the mover's [`Stance`]. A still body is silent
/// in any stance (the scaling of zero is still zero), so crouching only helps
/// while you're actually moving — it makes a moving body much harder to hear.
pub fn footstep_loudness_stanced(mass: u16, speed_milli: u32, ground_loudness: u8, stance: Stance) -> Loudness {
    (footstep_loudness(mass, speed_milli, ground_loudness) as i64 * stance.noise_permille() / 1000)
        as Loudness
}

/// [`footstep_loudness_stanced`] with the surface looked up from the block underfoot.
pub fn footstep_loudness_on_stanced(
    mass: u16,
    speed_milli: u32,
    ground: crate::world::blocks::BlockId,
    stance: Stance,
) -> Loudness {
    footstep_loudness_stanced(mass, speed_milli, surface_loudness(ground), stance)
}

/// The background noise floor of a biome — the ambient hum a listener is always
/// steeped in. A footfall has to rise above this (and the listener's threshold)
/// to register, so the same step that's obvious in a dead-silent cave is washed
/// out under crashing surf or a thermal vent. This is what wires masking to the
/// world: pass the listener's biome floor into [`noticed_over_ambient`].
pub fn biome_ambient_loudness(biome: crate::world::biomes::Biome) -> Loudness {
    use crate::world::biomes::Biome::*;
    match biome {
        IceCave => 2,   // a dripping, near-silent cave
        FungalCave => 6,
        AlpineHighlands | MistyMountains | Tundra => 8, // thin wind, sparse life
        Desert | RedSandstoneCanyon => 10,
        Plains | TemperateHighlands | HighlandSteppe => 12,
        Skylands => 14, // steady high wind
        BloomingCanyon => 18,
        TemperateForest | BirchForest | MapleForest | CloudForest => 20, // rustle + birdsong
        Savanna => 13,    // open grassland, dry wind
        Taiga => 16,      // conifer wind + sparse birds
        Swamp => 24,      // insects, frogs
        Jungle => 28,     // dense insects + birdsong, loudest forest
        CrystalCave => 3, // faint chiming, near-silent
        LushCave => 9,    // dripping water + cave life
        ThermalCave => 28, // hissing vents
        Beach => 30,       // breaking surf
        Ocean => 34,       // open waves
        Mesa => 10,        // r336-biomes: dry eroded badlands wind
        VolcanicWastes => 12, // r336-biomes: ash flats, hissing vents
        Mangrove => 24,    // r336-biomes: tidal swamp insects/frogs
        FrozenOcean => 26, // r336-biomes: muffled icy sea
    }
}

/// Whether a sound is *noticed*: it must clear both the listener's hearing
/// `threshold` and the ambient noise `floor` it competes with. Below either, the
/// listener doesn't react.
pub fn noticed_over_ambient(perceived: i32, ambient_floor: Loudness, threshold: i32) -> bool {
    perceived >= threshold && perceived > ambient_floor
}

/// Floor integer sqrt of a non-negative value. Deterministic (no float).
pub fn isqrt(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    // Newton's method on integers, seeded from a bit-length estimate.
    let mut x = 1i64 << ((64 - n.leading_zeros() + 1) / 2);
    loop {
        let nx = (x + n / x) / 2;
        if nx >= x {
            return x;
        }
        x = nx;
    }
}

/// Integer voxel coordinates containing `p`.
pub fn voxel_of(p: Vec3) -> [i32; 3] {
    [p.x.floor_int() as i32, p.y.floor_int() as i32, p.z.floor_int() as i32]
}

/// Euclidean block distance (floored) between two voxels.
fn block_distance(a: [i32; 3], b: [i32; 3]) -> i32 {
    let dx = (b[0] - a[0]) as i64;
    let dy = (b[1] - a[1]) as i64;
    let dz = (b[2] - a[2]) as i64;
    isqrt(dx * dx + dy * dy + dz * dz) as i32
}

/// Count solid voxels strictly between `a` and `b` along a 3D Bresenham line.
/// Endpoints themselves are NOT counted (a listener standing in a wall, or a
/// source embedded in one, shouldn't occlude itself). O(max axis delta).
pub fn count_solids_on_line(
    a: [i32; 3],
    b: [i32; 3],
    is_solid: impl Fn(i32, i32, i32) -> bool,
) -> i32 {
    let d = [(b[0] - a[0]).abs(), (b[1] - a[1]).abs(), (b[2] - a[2]).abs()];
    let s = [
        (b[0] - a[0]).signum(),
        (b[1] - a[1]).signum(),
        (b[2] - a[2]).signum(),
    ];
    // Dominant axis = largest delta.
    let axis = if d[0] >= d[1] && d[0] >= d[2] {
        0
    } else if d[1] >= d[2] {
        1
    } else {
        2
    };
    let dm = d[axis];
    if dm == 0 {
        return 0; // same voxel
    }
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let mut p = a;
    let mut e1 = 2 * d[u] - dm;
    let mut e2 = 2 * d[v] - dm;
    let mut solids = 0;
    for _ in 0..dm {
        if e1 > 0 {
            p[u] += s[u];
            e1 -= 2 * dm;
        }
        if e2 > 0 {
            p[v] += s[v];
            e2 -= 2 * dm;
        }
        e1 += 2 * d[u];
        e2 += 2 * d[v];
        p[axis] += s[axis];
        // Count this voxel unless it's the listener's own (the final step).
        if p != b && is_solid(p[0], p[1], p[2]) {
            solids += 1;
        }
    }
    solids
}

/// Perceived loudness from raw distance + occlusion. Distance attenuation is
/// `loudness · F / (F + dist)`; each occluding block halves the result.
pub fn perceived_loudness(loudness: Loudness, dist_blocks: i32, solid_blocks: i32) -> i32 {
    if loudness <= 0 {
        return 0;
    }
    let dist = dist_blocks.max(0);
    let atten = (loudness as i64 * HEAR_FALLOFF_BLOCKS as i64
        / (HEAR_FALLOFF_BLOCKS + dist) as i64) as i32;
    let occ = solid_blocks.clamp(0, MAX_OCCLUDING_BLOCKS);
    atten >> occ
}

/// Loudness a listener perceives from a source, given a solidity sampler.
/// Pure + testable without a world (pass a closure for walls).
pub fn heard_loudness(
    source: SoundSource,
    listener: Vec3,
    is_solid: impl Fn(i32, i32, i32) -> bool,
) -> i32 {
    let a = voxel_of(source.pos);
    let b = voxel_of(listener);
    let dist = block_distance(a, b);
    let solids = count_solids_on_line(a, b, is_solid);
    perceived_loudness(source.loudness, dist, solids)
}

/// [`heard_loudness`] against a live [`WorldQuery`] (a voxel is occluding iff
/// its block physics is solid).
pub fn heard_loudness_world<W: WorldQuery>(
    source: SoundSource,
    listener: Vec3,
    world: &W,
) -> i32 {
    heard_loudness(source, listener, |x, y, z| world.physics_at(x, y, z).is_solid())
}

/// Whether `listener` hears `source` above `threshold`.
pub fn can_hear<W: WorldQuery>(
    source: SoundSource,
    listener: Vec3,
    world: &W,
    threshold: i32,
) -> bool {
    heard_loudness_world(source, listener, world) >= threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixed::Fixed;

    fn v(x: i64, y: i64, z: i64) -> Vec3 {
        Vec3 { x: Fixed::from_int(x), y: Fixed::from_int(y), z: Fixed::from_int(z) }
    }

    #[test]
    fn isqrt_is_floor_sqrt() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(1), 1);
        assert_eq!(isqrt(3), 1);
        assert_eq!(isqrt(4), 2);
        assert_eq!(isqrt(8), 2);
        assert_eq!(isqrt(9), 3);
        assert_eq!(isqrt(100), 10);
        assert_eq!(isqrt(10_000), 100);
        for k in 1..200i64 {
            let r = isqrt(k * k);
            assert_eq!(r, k, "isqrt({}) should be {k}", k * k);
            assert_eq!(isqrt(k * k - 1), k - 1, "just below a square");
        }
    }

    #[test]
    fn closer_is_louder() {
        let near = perceived_loudness(1000, 4, 0);
        let far = perceived_loudness(1000, 80, 0);
        assert!(near > far, "closer should be louder: {near} vs {far}");
        // At the falloff distance loudness is ~halved.
        let half = perceived_loudness(1000, HEAR_FALLOFF_BLOCKS, 0);
        assert!((half - 500).abs() < 10, "half-loudness near falloff dist: {half}");
    }

    #[test]
    fn each_wall_halves_loudness() {
        let clear = perceived_loudness(1000, 10, 0);
        let one = perceived_loudness(1000, 10, 1);
        let two = perceived_loudness(1000, 10, 2);
        assert_eq!(one, clear / 2, "one wall halves");
        assert_eq!(two, clear / 4, "two walls quarter");
    }

    #[test]
    fn occlusion_is_capped() {
        let a = perceived_loudness(1000, 5, MAX_OCCLUDING_BLOCKS);
        let b = perceived_loudness(1000, 5, MAX_OCCLUDING_BLOCKS + 5);
        assert_eq!(a, b, "occlusion beyond the cap doesn't keep shrinking");
    }

    #[test]
    fn clear_line_crosses_no_solids() {
        // Empty world → no occluding blocks along any line.
        let n = count_solids_on_line([0, 64, 0], [20, 64, 7], |_, _, _| false);
        assert_eq!(n, 0);
    }

    #[test]
    fn a_wall_between_is_counted_endpoints_are_not() {
        // A solid plane at x = 5 between source (x=0) and listener (x=10).
        let wall = |x: i32, _y: i32, _z: i32| x == 5;
        let n = count_solids_on_line([0, 64, 0], [10, 64, 0], wall);
        assert_eq!(n, 1, "the single wall column should be counted once");

        // A wall AT the listener's own voxel must not count (self-occlusion).
        let at_listener = |x: i32, _: i32, _: i32| x == 10;
        let n2 = count_solids_on_line([0, 64, 0], [10, 64, 0], at_listener);
        assert_eq!(n2, 0, "the listener's own voxel is not an occluder");
    }

    #[test]
    fn thick_wall_counts_multiple() {
        // Three solid columns x ∈ {4,5,6} between 0 and 12.
        let wall = |x: i32, _: i32, _: i32| (4..=6).contains(&x);
        let n = count_solids_on_line([0, 64, 0], [12, 64, 0], wall);
        assert_eq!(n, 3, "a 3-block-thick wall counts 3");
    }

    #[test]
    fn heard_loudness_quiet_through_a_wall() {
        let src = SoundSource::new(v(0, 64, 0), 1000);
        let listener = v(10, 64, 0);
        let clear = heard_loudness(src, listener, |_, _, _| false);
        let walled = heard_loudness(src, listener, |x, _, _| x == 5);
        assert!(walled < clear, "a wall should muffle: walled {walled} vs clear {clear}");
        assert_eq!(walled, clear / 2, "one wall halves the clear-line loudness");
    }

    #[test]
    fn can_hear_respects_threshold_and_world() {
        use crate::sim::physics::FlatWorld;
        let w = FlatWorld::new();
        // Same height as the source, above the flat ground → clear horizontal line.
        let src = SoundSource::new(v(0, 40, 0), 1000);
        assert!(can_hear(src, v(5, 40, 0), &w, 100), "a loud nearby noise is heard");
        assert!(!can_hear(src, v(5, 40, 0), &w, 100_000), "not above an absurd threshold");
        // Very far away → falls below a modest threshold.
        assert!(!can_hear(src, v(5000, 40, 0), &w, 100), "distant noise fades out");
    }

    #[test]
    fn tracking_a_moving_source_is_monotone_as_it_approaches() {
        // As a source walks toward a fixed listener along a clear line, the
        // perceived loudness must rise monotonically — i.e. a mob could track it.
        let listener = v(0, 64, 0);
        let mut prev = -1;
        for x in (1..=40).rev() {
            let src = SoundSource::new(v(x, 64, 0), 800);
            let l = heard_loudness(src, listener, |_, _, _| false);
            assert!(l >= prev, "loudness should not drop as source nears: {l} < {prev}");
            prev = l;
        }
    }

    #[test]
    fn footstep_loudness_tracks_mass_speed_and_ground() {
        // Standing still makes no footfall.
        assert_eq!(footstep_loudness(70, 0, 200), 0);
        // A typical stride lands near the documented footstep ≈ 200.
        let typical = footstep_loudness(70, 200, 128);
        assert!((150..=300).contains(&typical), "typical stride ~200, got {typical}");
        // Monotonic in each input.
        assert!(footstep_loudness(140, 200, 128) > typical, "heavier → louder");
        assert!(footstep_loudness(70, 400, 128) > typical, "faster → louder");
        assert!(footstep_loudness(70, 200, 255) > typical, "noisier ground → louder");
        // Sneaking on soft ground is much quieter than striding on stone.
        let sneak = footstep_loudness(70, 40, 10);
        assert!(sneak < typical / 2, "a careful step on moss is quiet: {sneak}");
    }

    #[test]
    fn surface_loudness_orders_quiet_naturals_below_hard_minerals() {
        use crate::world::blocks::BlockId;
        assert!(surface_loudness(BlockId::Grass) < surface_loudness(BlockId::Gravel));
        assert!(surface_loudness(BlockId::Snow) < surface_loudness(BlockId::Grass));
        assert!(surface_loudness(BlockId::Moss) < surface_loudness(BlockId::Stone));
        assert!(surface_loudness(BlockId::Gravel) >= surface_loudness(BlockId::Stone), "loose gravel is loudest");
        assert!(surface_loudness(BlockId::OakLeaves) > surface_loudness(BlockId::Grass), "leaves rustle");
    }

    #[test]
    fn footstep_on_surface_makes_ground_choice_matter() {
        use crate::world::blocks::BlockId;
        // Same stride, different ground: gravel gives you away, grass hides you.
        let loud = footstep_loudness_on(70, 200, BlockId::Gravel);
        let quiet = footstep_loudness_on(70, 200, BlockId::Grass);
        assert!(loud > quiet * 2, "crossing gravel ({loud}) is far louder than grass ({quiet})");
        // Still no sound when not moving, whatever the surface.
        assert_eq!(footstep_loudness_on(70, 0, BlockId::Gravel), 0);
    }

    /// Diagnostic double-check of the forest scenario reported in-game: an animal
    /// hears a moving player through the trees, but loses the cue when the player
    /// freezes — which is why a still player gets approached rather than fled from.
    #[test]
    fn forest_listener_loses_a_player_who_freezes() {
        use crate::world::blocks::BlockId;
        // Player ~70 kg, an ambling stride (~0.2 blk/tick) on forest leaf litter.
        let walking = footstep_loudness_on(70, 200, BlockId::OakLeaves);
        assert!(walking > 0, "a walking player rustles the leaves ({walking})");

        // Listener 8 blocks away. Open sightline vs two tree trunks between.
        let p = |solids: i32| perceived_loudness(walking, 8, solids);
        let open = p(0);
        let one_tree = p(1);
        let two_trees = p(2);
        assert!(open > 0, "heard in the open at 8 blocks ({open})");
        // Each trunk on the line halves the sound — trees muffle but don't block.
        assert_eq!(one_tree, open / 2);
        assert_eq!(two_trees, open / 4);
        assert!(two_trees > 0, "still faintly audible behind two trees ({two_trees})");

        // Distance attenuates: the same step is quieter the farther off you are.
        assert!(perceived_loudness(walking, 4, 0) > perceived_loudness(walking, 16, 0));

        // THE KEY POINT: freeze and you emit nothing, so you're inaudible at any
        // range, through any number of trees. With no sound cue the listener stops
        // tracking and drifts — what reads in-game as it "wandering closer".
        let frozen = footstep_loudness_on(70, 0, BlockId::OakLeaves);
        assert_eq!(frozen, 0, "a motionless player makes no footfall");
        assert_eq!(perceived_loudness(frozen, 1, 0), 0, "and so is unheard even point-blank");

        // Stealth lever: creep on moss instead of leaves and you're much quieter
        // even while moving.
        let creeping_on_moss = footstep_loudness_on(70, 90, BlockId::Moss);
        assert!(creeping_on_moss < walking, "creeping on moss ({creeping_on_moss}) < walking on leaves ({walking})");
    }

    #[test]
    fn crouching_muffles_a_moving_body_but_silence_stays_silent() {
        use crate::world::blocks::BlockId;
        let (mass, speed, ground) = (70u16, 200u32, surface_loudness(BlockId::Gravel));
        let stand = footstep_loudness_stanced(mass, speed, ground, Stance::Stand);
        let crouch = footstep_loudness_stanced(mass, speed, ground, Stance::Crouch);
        let prone = footstep_loudness_stanced(mass, speed, ground, Stance::Prone);
        // Standing is the loudest; each lower stance is quieter.
        assert_eq!(stand, footstep_loudness(mass, speed, ground), "standing = full volume");
        assert!(crouch < stand, "crouch ({crouch}) muffles standing ({stand})");
        assert!(prone < crouch, "prone ({prone}) is quieter than crouch ({crouch})");
        // But stance can't make a *still* body audible: zero scales to zero.
        for st in [Stance::Stand, Stance::Crouch, Stance::Prone] {
            assert_eq!(footstep_loudness_stanced(mass, 0, ground, st), 0);
        }
        // A crouch-walk on gravel can be quieter than a stand-walk on grass — so
        // stance and ground choice stack into real stealth.
        let crouch_gravel = footstep_loudness_on_stanced(mass, speed, BlockId::Gravel, Stance::Crouch);
        let stand_grass = footstep_loudness_on_stanced(mass, speed, BlockId::Grass, Stance::Stand);
        assert!(crouch_gravel < stand_grass, "crouch-gravel {crouch_gravel} < stand-grass {stand_grass}");
    }

    #[test]
    fn ambient_masking_washes_out_quiet_sounds_in_loud_biomes() {
        use crate::world::biomes::Biome;
        // Caves are near-silent; open water roars.
        let cave = biome_ambient_loudness(Biome::IceCave);
        let forest = biome_ambient_loudness(Biome::TemperateForest);
        let ocean = biome_ambient_loudness(Biome::Ocean);
        assert!(cave < forest && forest < ocean, "quiet cave < forest < loud ocean ({cave},{forest},{ocean})");
        // A footstep perceived at 15: heard plainly in the silent cave, but masked
        // by the surf at the shoreline.
        let perceived = 15;
        let thresh = 5;
        assert!(noticed_over_ambient(perceived, cave, thresh), "noticed in a silent cave");
        assert!(!noticed_over_ambient(perceived, ocean, thresh), "masked by the ocean roar");
        // Still gated by the listener's own threshold even in silence.
        assert!(!noticed_over_ambient(3, cave, thresh), "below threshold isn't noticed");
    }
}
