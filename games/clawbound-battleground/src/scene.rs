// Scene: uploads all voxel meshes once, then records per-frame draw calls
// from the game state (gods, minions, towers, titans, projectiles, props).

use glam::{Mat4, Vec3};

use crate::blockers::{Blockers, PropKind};
use crate::consts::{self, Lane, Team, ARENA_RADIUS};
use crate::gods::{ROSTER, ROSTER_LEN};
use crate::models;
use crate::props;
use crate::puppet::{LeapView, Probe, PuppetInput, StrikeStyle};
use crate::render::{sphere_in_frustum, terrain_height, Camera, Renderer};
use crate::rigged::{slot_transforms, QuadRig, Slot, SLOTS};
use crate::sim::{Game, ProjKind, Rng};
use crate::vfx::Vfx;
use crate::voxel::{merge_xform, VoxelMesh};

/// Cull radius for a minion-sized actor, in world units.
///
/// GENEROUS ON PURPOSE. A sphere fitted to the body pops when an arm, a tail
/// or a weapon swings outside it, and a pop at the screen edge is far more
/// noticeable than the handful of extra draws a loose radius lets through.
/// The cost of being wrong in each direction is not symmetric.
pub const ACTOR_CULL_R: f32 = 2.2;

/// Half the height of a tower, so the cull sphere sits at its middle.
///
/// A tower's `pos` is its BASE. Culling against a sphere there makes a tower
/// whose base is just off the bottom of the screen vanish while its crystal is
/// still plainly in view.
pub const TOWER_HALF_H: f32 = 3.0;

/// Cull radius for a tower, covering the structure and its crystal.
pub const TOWER_CULL_R: f32 = 5.0;

pub struct Scene {
    /// Ancestors cut into posable parts: `[ancestor][team][slot]`.
    pub god_parts: Vec<[[u64; SLOTS]; 2]>,
    /// The same parts as smooth, furred surfaces (the ultra preview;
    /// `smoothvox`); empty unless ultra was asked for at build.
    pub god_smooth: Vec<[[u64; SLOTS]; 2]>,
    /// Draw the ancestors smooth and furred, with coloured lights and
    /// traced shadows (`quality::Quality::Ultra`). A plain flag, so a
    /// harness can compare the two on one frame.
    pub ultra: bool,
    /// The fur style the smooth parts were built for (the face's fur
    /// depends on it): a different one needs a rebuild.
    pub fur_built: crate::render::FurStyle,
    /// One rig per ancestor (teams share anatomy).
    pub god_rig: Vec<QuadRig>,
    /// The uncut body. Still what the portrait renderer draws.
    pub god_body: [[u64; 2]; ROSTER_LEN],
    pub god_weapon: [[u64; 2]; ROSTER_LEN],
    pub minion_melee: [u64; 2],
    pub minion_caster: [u64; 2],
    pub tower_body: [u64; 2],
    pub tower_crystal: [u64; 2],
    pub titan_body: [u64; 2],
    pub titan_weapon: [u64; 2],
    pub chunks: Vec<PropChunk>,
    pub caves: Vec<CaveDraw>,
    /// The map this scene was built for, and the first mesh id it uploaded
    /// (`Renderer::free_meshes_from` gives its meshes back on a switch).
    pub map: crate::map::MapId,
    pub first_mesh: u64,
    /// A small flat tile: the dots a targeting guide is drawn with.
    pub guide_dot: u64,
    pub gate: [u64; 2],
    pub fountain: [u64; 2],
    pub pedestal: [u64; 2],
    pub bird: u64,
    pub hare: u64,
    pub stag: u64,
    pub glowfish: u64,
    pub boar: u64,
    pub drake: u64,
    pub heal_tree: u64,
    pub heal_fruit: u64,
    pub orb: u64,
    pub arrow: [u64; 2],
    pub boulder: u64,
    /// A sky-map ferry cloud, and the height of its deck above the mesh's
    /// feet (`skyprops::ferry_cloud`).
    pub cloud: u64,
    pub cloud_top: f32,
    /// The Moonwyrm (sky map), and one of its stars.
    pub moonwyrm: u64,
    /// Its body's segments: [with a crest fin?][variant].
    pub wyrm_seg: [[u64; 2]; 2],
    pub star: u64,
    /// Sproutkit's litter (his own body, drawn small), by team, and his
    /// glitter-ball.
    pub kitling: [u64; 2],
    pub disco: u64,
    /// The sky map's drifting stone and cloud (`skydrift`), and their meshes
    /// by kind (anchor, fragment, puff) and variant.
    pub drift: Vec<crate::skydrift::Drift>,
    pub drift_mesh: [[u64; DRIFT_VARIANTS]; 3],
    /// The crossing log: `props::fallen_log`, drawn per frame because it rolls.
    pub river_log: u64,
    pub total_voxels: usize,
    /// Static prop collision, built alongside the meshes in `build_world` so
    /// the two cannot disagree. Handed to `Game` by the app; see
    /// `Game::blockers`.
    pub blockers: std::sync::Arc<Blockers>,
}

/// Mesh variants per kind of drifting piece.
pub const DRIFT_VARIANTS: usize = 6;

/// One frustum-cullable batch of static world voxels.
/// A cave roof (`pass::caves`): drawn on its own, not batched, because it
/// is cut away while the player is under it -- a third-person camera sits
/// above the cat, and a solid roof would hide the player from themselves.
pub struct CaveDraw {
    pub mesh: u64,
    pub cave: crate::pass::Cave,
}

/// Is a cave roof drawn for a player at `player`? Not while they are under
/// it: a cutaway. Ghosting it (alpha 0.22) was tried first and rendered as
/// a soup of overlapping translucent cubes -- blending sorts per draw, not
/// per voxel.
pub fn cave_drawn(cave: &crate::pass::Cave, player: Vec3) -> bool {
    !cave.covers(player.x, player.z)
}

pub struct PropChunk {
    pub mesh: u64,
    pub center: Vec3,
    pub radius: f32,
    pub vsize: f32,
    /// The coarse copy (`voxel::downsample`, voxel `2 * vsize`), drawn
    /// beyond `LOD_DIST`. None for chunks already coarse.
    pub lod: Option<u64>,
}

/// Beyond this distance (camera to the chunk's sphere) a chunk draws its
/// coarse copy: at gameplay height most voxel instances were chunks far
/// down a lane, at full resolution.
pub const LOD_DIST: f32 = 70.0;
/// Fine-detail chunks (ferns, scrub, spray: voxel 0.35) are not drawn past
/// this: a few pixels each by then.
pub const DETAIL_DIST: f32 = 110.0;

/// Which mesh a chunk draws from a camera at `eye`, and at what voxel size:
/// None = not at all (fine detail, far away).
pub fn chunk_choice(ch: &PropChunk, eye: Vec3) -> Option<(u64, f32)> {
    let d = (eye - ch.center).length() - ch.radius;
    if ch.vsize < 0.5 && d > DETAIL_DIST {
        return None;
    }
    match ch.lod {
        Some(l) if d > LOD_DIST => Some((l, ch.vsize * 2.0)),
        _ => Some((ch.mesh, ch.vsize)),
    }
}

/// `puppet::Probe` over the real arena: the terrain height field, the
/// blockers' walkable humps and cover discs, and the river.
struct WorldProbe<'a> {
    blockers: &'a Blockers,
    logs: &'a [crate::logs::RollingLog],
    /// Ultra's snowpack, if any: how much loose snow a paw is in.
    snow: Option<&'a crate::snowpack::Snowpack>,
}

impl Probe for WorldProbe<'_> {
    fn ground(&self, x: f32, z: f32) -> f32 {
        // The sim walks the playable floor at y = 0 and sinks the DRAWN cat
        // by the water depth, so the river bed is not "ground" here.
        terrain_height(x, z).max(0.0)
    }
    fn step_top(&self, x: f32, z: f32) -> Option<f32> {
        // A crossing log is rounded footing: highest on the spine.
        // The exact surface: the voxel face under the paw on a seated log,
        // the rotated section's top on a turned one.
        let log = self.logs.iter().find_map(|l| l.surface_y(x, z));
        match (self.blockers.step_top(x, z), log) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }
    fn cover(&self, x: f32, z: f32) -> f32 {
        self.blockers.concealment_at(x, z)
    }
    fn water(&self, x: f32, z: f32) -> f32 {
        consts::water_depth(x, z)
    }
    fn snow(&self, x: f32, z: f32) -> f32 {
        self.snow.map_or(0.0, |s| s.surface_at(x, z))
    }
    fn stem(&self, x: f32, z: f32, within: f32) -> Option<(f32, f32)> {
        self.blockers.nearest_stem(x, z, within)
    }
    fn solid(&self, x: f32, z: f32) -> bool {
        self.blockers.blocked(x, z, 0.05)
    }
}

fn solid_tint(c: [f32; 3]) -> [f32; 4] {
    [c[0], c[1], c[2], 1.0]
}

fn alpha_tint(c: [f32; 3], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], a]
}

impl Scene {
    pub fn new(r: &mut Renderer) -> Scene {
        let first_mesh = r.mesh_mark();
        let mut total = 0usize;

        let mut god_body = [[0u64; 2]; ROSTER_LEN];
        let mut god_weapon = [[0u64; 2]; ROSTER_LEN];
        let mut god_parts = Vec::with_capacity(ROSTER_LEN);
        let mut god_rig = Vec::with_capacity(ROSTER_LEN);
        for (i, def) in ROSTER.iter().enumerate() {
            let mut parts = [[0u64; SLOTS]; 2];
            for team in [Team::Blue, Team::Red] {
                let ti = team as usize;
                let body = models::build_god(def, team);
                total += body.inst.len();
                god_body[i][ti] = r.upload_mesh(&body);
                let rigged = models::build_god_rigged(def, team);
                for (s, m) in rigged.meshes.iter().enumerate() {
                    total += m.inst.len();
                    parts[ti][s] = r.upload_mesh(m);
                }
                if ti == 0 {
                    god_rig.push(rigged.rig);
                }
                if let Some(w) = models::build_weapon(def.weapon, def, team) {
                    total += w.inst.len();
                    god_weapon[i][ti] = r.upload_mesh(&w);
                }
            }
            god_parts.push(parts);
        }
        assert_eq!((god_parts.len(), god_rig.len()), (ROSTER_LEN, ROSTER_LEN), "one cut body and one rig per ancestor");
        // the ultra preview: every part smoothed and furred, once
        let ultra = crate::quality::Quality::effective().smooth_characters();
        let mut god_smooth = Vec::new();
        if ultra {
            let t = std::time::Instant::now();
            let mut tris = 0;
            for def in ROSTER.iter() {
                let mut parts = [[0u64; SLOTS]; 2];
                for team in [Team::Blue, Team::Red] {
                    let rigged = models::build_god_rigged(def, team);
                    for (s, m) in rigged.meshes.iter().enumerate() {
                        let mut sm = crate::smoothvox::smooth(m, models::GOD_DRAW_VOXEL, 2);
                        crate::smoothvox::fur_for_part(def, s, &mut sm, crate::render::FurStyle::from_env() == crate::render::FurStyle::Locks);
                        tris += sm.tris();
                        parts[team as usize][s] = r.upload_smooth(&sm);
                    }
                }
                god_smooth.push(parts);
            }
            eprintln!("[clawbound] ultra preview: {tris} smooth triangles built in {:.1} s", t.elapsed().as_secs_f32());
        }

        let mut minion_melee = [0u64; 2];
        let mut minion_caster = [0u64; 2];
        let mut tower_body = [0u64; 2];
        let mut tower_crystal = [0u64; 2];
        let mut titan_body = [0u64; 2];
        let mut titan_weapon = [0u64; 2];
        let mut gate = [0u64; 2];
        let mut fountain = [0u64; 2];
        let mut pedestal = [0u64; 2];
        let mut arrow = [0u64; 2];

        for team in [Team::Blue, Team::Red] {
            let ti = team as usize;
            let m = models::build_minion(false, team);
            total += m.inst.len();
            minion_melee[ti] = r.upload_mesh(&m);
            let c = models::build_minion(true, team);
            total += c.inst.len();
            minion_caster[ti] = r.upload_mesh(&c);

            let (tb, tc) = models::build_tower(team);
            total += tb.inst.len() + tc.inst.len();
            tower_body[ti] = r.upload_mesh(&tb);
            tower_crystal[ti] = r.upload_mesh(&tc);

            let (xb, xw) = models::build_titan(team);
            total += xb.inst.len() + xw.inst.len();
            titan_body[ti] = r.upload_mesh(&xb);
            titan_weapon[ti] = r.upload_mesh(&xw);

            let g = props::base_gate(team.rgb());
            total += g.inst.len();
            gate[ti] = r.upload_mesh(&g);

            let f = props::fountain(team.rgb());
            total += f.inst.len();
            fountain[ti] = r.upload_mesh(&f);

            let p = props::titan_pedestal(team.rgb());
            total += p.inst.len();
            pedestal[ti] = r.upload_mesh(&p);

            let a = models::build_arrow(team);
            total += a.inst.len();
            arrow[ti] = r.upload_mesh(&a);
        }

        let (chunk_builds, bird_mesh, blockers) = build_world();
        let mut chunks = Vec::new();
        for (mesh, center, radius, vsize) in chunk_builds {
            total += mesh.inst.len();
            let id = r.upload_mesh(&mesh);
            let lod = (vsize < 1.0).then(|| {
                let coarse = crate::voxel::downsample(&mesh, vsize);
                r.upload_mesh(&coarse)
            });
            chunks.push(PropChunk { mesh: id, center, radius, vsize, lod });
        }
        total += bird_mesh.inst.len();
        let bird = r.upload_mesh(&bird_mesh);
        let guide_dot = {
            let mut g = crate::voxel::Grid::new(3, 1, 3);
            g.filled_box(0, 0, 0, 2, 0, 2, [1.0, 1.0, 1.0, 1.0]);
            r.upload_mesh(&g.mesh(0.12, true))
        };
        let caves: Vec<CaveDraw> = if crate::map::active() == crate::map::MapId::Pass {
            crate::pass::caves()
                .into_iter()
                .enumerate()
                .map(|(i, cave)| {
                    let d = cave.b - cave.a;
                    let m = props::cave_roof(d.length(), cave.half * 2.0, cave.height, d.x.atan2(d.z), i as i32);
                    total += m.inst.len();
                    CaveDraw { mesh: r.upload_mesh(&m), cave }
                })
                .collect()
        } else {
            Vec::new()
        };

        let orb = models::build_orb([1.0, 1.0, 1.0]);
        total += orb.inst.len();
        let orb = r.upload_mesh(&orb);
        let hare_m = models::build_hare();
        let stag_m = models::build_stag();
        let fish_m = models::build_glowfish();
        total += fish_m.inst.len();
        let glowfish = r.upload_mesh(&fish_m);
        let boar_m = models::build_boar();
        let drake_m = models::build_drake();
        total += boar_m.inst.len() + drake_m.inst.len();
        let boar = r.upload_mesh(&boar_m);
        let drake = r.upload_mesh(&drake_m);
        let (tree_m, fruit_m) = models::build_heal_tree();
        total += hare_m.inst.len() + stag_m.inst.len() + tree_m.inst.len() + fruit_m.inst.len();
        let hare = r.upload_mesh(&hare_m);
        let stag = r.upload_mesh(&stag_m);
        let heal_tree = r.upload_mesh(&tree_m);
        let heal_fruit = r.upload_mesh(&fruit_m);
        let boulder = models::build_boulder();
        total += boulder.inst.len();
        let boulder = r.upload_mesh(&boulder);
        let (cloud_m, cloud_top) = crate::skyprops::ferry_cloud(crate::ferry::CLOUD_R, 1);
        total += cloud_m.inst.len();
        let cloud = r.upload_mesh(&cloud_m);
        let wyrm_m = crate::skyprops::moonwyrm();
        total += wyrm_m.inst.len();
        let moonwyrm = r.upload_mesh(&wyrm_m);
        let mut wyrm_seg = [[0u64; 2]; 2];
        for fin in [false, true] {
            for v in 0..2 {
                let m = crate::skyprops::wyrm_segment(fin, v as i32);
                total += m.inst.len();
                wyrm_seg[fin as usize][v] = r.upload_mesh(&m);
            }
        }
        let star = r.upload_mesh(&crate::skyprops::star_mote());
        let kit_def = ROSTER.iter().find(|d| matches!(d.pelt, crate::gods::PeltKind::Kit)).expect("Sproutkit on the roster");
        let kitling = [r.upload_mesh(&models::build_god(kit_def, Team::Blue)), r.upload_mesh(&models::build_god(kit_def, Team::Red))];
        let disco = r.upload_mesh(&models::disco_ball());
        let sky = crate::map::active() == crate::map::MapId::Sky;
        let drift = if sky { crate::skydrift::placements() } else { Vec::new() };
        let mut drift_mesh = [[0u64; DRIFT_VARIANTS]; 3];
        if sky {
            for v in 0..DRIFT_VARIANTS {
                for (k, m) in [crate::skyprops::anchor_tile(v as i32), crate::skyprops::edge_fragment(v as i32), crate::skyprops::cloud_puff(v as i32)].iter().enumerate() {
                    total += m.inst.len();
                    drift_mesh[k][v] = r.upload_mesh(m);
                }
            }
        }
        let river_log_mesh = props::crossing_log();
        total += river_log_mesh.inst.len();
        let river_log = r.upload_mesh(&river_log_mesh);

        Scene {
            god_parts,
            god_smooth,
            ultra,
            fur_built: crate::render::FurStyle::from_env(),
            god_rig,
            god_body,
            god_weapon,
            minion_melee,
            minion_caster,
            tower_body,
            tower_crystal,
            titan_body,
            titan_weapon,
            chunks,
            caves,
            map: crate::map::active(),
            first_mesh,
            guide_dot,
            gate,
            fountain,
            pedestal,
            bird,
            hare,
            stag,
            glowfish,
            boar,
            drake,
            heal_tree,
            heal_fruit,
            orb,
            arrow,
            boulder,
            cloud,
            cloud_top,
            moonwyrm,
            wyrm_seg,
            star,
            kitling,
            disco,
            drift,
            drift_mesh,
            river_log,
            total_voxels: total,
            blockers: std::sync::Arc::new(blockers),
        }
    }

    pub fn draw(&self, r: &mut Renderer, game: &Game, vfx: &mut Vfx, cam: Camera) {
        // the ultra preview's coloured lights (ability effects, the ones
        // brightest where the camera looks) and the spheres their shadows
        // are traced against (every cat standing)
        r.ultra_post = self.ultra;
        if self.ultra {
            r.lights = vfx.lights(cam.target, 8);
            r.occluders = game.gods.iter().filter(|g| g.respawn_t <= 0.0).map(|g| [g.pos.x, g.pos.y + 0.9, g.pos.z, 0.95]).collect();
        } else {
            r.lights.clear();
            r.occluders.clear();
        }
        r.clear_queues();

        // static world chunks — frustum culled. Snow cover frosts the flora
        // (a cool, brightened tint; the terrain shader whitens the ground).
        let atm = game.weather.atmosphere();
        let frost = atm.snow * 0.55 + game.weather.wetness * atm.snow * 0.25;
        let chunk_tint = [1.0 + frost * 0.35, 1.0 + frost * 0.42, 1.0 + frost * 0.55, 1.0];
        let vp = cam.view_proj(r.width as f32 / r.height.max(1) as f32);
        let any_char = !game.disasters.charred.is_empty();
        for ch in &self.chunks {
            let Some((mesh, vox)) = chunk_choice(ch, cam.eye) else { continue };
            if sphere_in_frustum(&vp, ch.center, ch.radius) {
                // burnt jungle: the flora in a charred chunk goes black-brown
                let tint = if any_char {
                    let c = game.disasters.char_near(ch.center, ch.radius.max(8.0));
                    [chunk_tint[0] * (1.0 - c * 0.8), chunk_tint[1] * (1.0 - c * 0.85), chunk_tint[2] * (1.0 - c * 0.88), 1.0]
                } else {
                    chunk_tint
                };
                r.queue_draw(mesh, Mat4::IDENTITY, tint, 0.0, vox, false);
            }
        }

        // cave roofs: solid, or cut away while the player is under one
        if let Some(me) = game.gods.iter().find(|g| g.is_player).or(game.gods.first()) {
            for cd in &self.caves {
                let c = cd.cave.mid() + Vec3::Y * cd.cave.height;
                let rad = (cd.cave.b - cd.cave.a).length() * 0.5 + cd.cave.half + 4.0;
                if !sphere_in_frustum(&vp, c, rad) {
                    continue;
                }
                if cave_drawn(&cd.cave, me.pos) {
                    r.queue_draw(cd.mesh, Mat4::from_translation(cd.cave.mid()), chunk_tint, 0.0, 0.6, false);
                }
            }
        }

        // objectives: the beasts in their pits, breathing when idle, lunging when angry
        for b in &game.bosses.list {
            if !b.alive {
                continue;
            }
            let (mesh, vs) = match b.kind {
                crate::bosses::BossKind::Boar => (self.boar, models::BOAR_VOXEL),
                crate::bosses::BossKind::Drake => (self.drake, models::DRAKE_VOXEL),
                crate::bosses::BossKind::Moonwyrm => (self.moonwyrm, crate::skyprops::WYRM_VOX),
            };
            let breathe = (b.anim * 0.7).sin() * 0.03;
            let lunge = b.atk_anim * 0.8;
            let hover = match b.kind {
                crate::bosses::BossKind::Drake => 0.4 + (b.anim * 0.5).sin() * 0.25,
                // sunk to the water line, rising and settling as it breathes
                crate::bosses::BossKind::Moonwyrm => -0.2 + (b.anim * 0.4).sin() * 0.25,
                crate::bosses::BossKind::Boar => 0.0,
            };
            let fwd = Vec3::new(b.yaw.cos(), 0.0, b.yaw.sin());
            if b.kind == crate::bosses::BossKind::Moonwyrm {
                // 400 u of it: the head, then the body laid along its spine
                // (coiled over the well, or streaming behind the head in
                // flight -- wyrmbody.rs)
                let since_land = game.t - (b.next_flight_t - crate::wyrmflight::FLIGHT_EVERY);
                let segs = crate::wyrmbody::segments(b.home, b.flight, since_land, fwd, game.t);
                let glow = 0.06 + atm.night * 0.08;
                let frame = |f: Vec3| {
                    let z = f.normalize_or(Vec3::Z);
                    let x = Vec3::Y.cross(z).normalize_or(Vec3::X);
                    (x, z.cross(x), z)
                };
                for (i, sg) in segs.iter().enumerate().skip(1) {
                    let rad = crate::wyrmbody::BODY_R * sg.r;
                    if !sg.shown || !sphere_in_frustum(&vp, sg.pos, rad + 3.5) {
                        continue;
                    }
                    let (x, y, z) = frame(sg.fwd);
                    let m = Mat4::from_cols((x * sg.r).extend(0.0), (y * sg.r).extend(0.0), z.extend(0.0), sg.pos.extend(1.0));
                    r.queue_draw(self.wyrm_seg[(i % 3 == 0) as usize][i % 2], m, [1.0; 4], glow, crate::skyprops::WYRM_SEG_VOX, false);
                }
                if let Some(h) = segs.first().filter(|h| h.shown) {
                    // at rest it looks where it is angry, lunging when it bites;
                    // in flight, where it is going
                    let look = if b.flight.is_some() { h.fwd } else { fwd };
                    let (x, y, z) = frame(look);
                    let pos = h.pos + z * lunge + Vec3::Y * (breathe * 8.0);
                    let m = Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), pos.extend(1.0));
                    r.queue_draw(mesh, m, [1.0; 4], glow, vs, false);
                }
                continue;
            }
            let _ = hover;
            let m = Mat4::from_translation(b.pos + fwd * lunge + Vec3::Y * (breathe + hover))
                * Mat4::from_axis_angle(Vec3::Y, -b.yaw + std::f32::consts::FRAC_PI_2)
                * Mat4::from_scale(Vec3::splat(1.0 + breathe));
            let glow = match b.kind {
                crate::bosses::BossKind::Drake => 0.25,
                _ => 0.0,
            };
            r.queue_draw(mesh, m, [1.0; 4], glow, vs, false);
        }

        // the Moonwyrm's stars: the ring it has ready, turning round it, and
        // the ones it has thrown, spinning in flight
        for b in game.bosses.list.iter().filter(|b| b.alive && b.kind == crate::bosses::BossKind::Moonwyrm) {
            // round its head: aloft, the ring flies with it
            let centre = if b.flight.is_some() { b.pos - Vec3::Y * 6.0 } else { b.home + crate::wyrmbody::REST_HEAD - Vec3::Y * 4.0 };
            for k in 0..b.stars as usize {
                let p = crate::moonwell::orbit_star(centre, game.t, k, crate::moonwell::STARS as usize);
                let m = Mat4::from_translation(p) * Mat4::from_axis_angle(Vec3::Y, game.t * 2.0 + k as f32) * Mat4::from_scale(Vec3::splat(1.0 + 0.15 * (game.t * 5.0 + k as f32).sin()));
                r.queue_draw(self.star, m, [1.0; 4], 1.2, crate::skyprops::STAR_VOX, false);
            }
        }
        // the stars it lets fall as it flies, dropping onto their marks
        for s in &game.starfalls {
            let m = Mat4::from_translation(s.pos()) * Mat4::from_axis_angle(Vec3::Y, game.t * 7.0) * Mat4::from_scale(Vec3::splat(2.0));
            r.queue_draw(self.star, m, [1.0; 4], 2.0, crate::skyprops::STAR_VOX, false);
        }
        for s in &game.star_bolts {
            let m = Mat4::from_translation(s.pos) * Mat4::from_axis_angle(s.vel.normalize_or_zero().cross(Vec3::Y).normalize_or(Vec3::X), game.t * 9.0) * Mat4::from_scale(Vec3::splat(1.3));
            r.queue_draw(self.star, m, [1.0; 4], 1.6, crate::skyprops::STAR_VOX, false);
        }

        // heal trees: the crown frosts like the rest, the fruit glows —
        // brighter while it is mending someone, and at night
        for tree in &game.wild.trees {
            let m = Mat4::from_translation(tree.pos);
            r.queue_draw(self.heal_tree, m, chunk_tint, 0.0, models::HEAL_TREE_VOXEL, false);
            let glow = 0.35 + tree.glow * 0.9 + atm.night * 0.4 + (vfx.time * 2.0).sin() * 0.08;
            r.queue_draw(self.heal_fruit, m, [1.0; 4], glow, models::HEAL_TREE_VOXEL, false);
        }

        // prey: hares hop, stags trot, the glowfish wriggles just under the
        // surface and lights the water around it
        for p in &game.wild.prey {
            let speed = p.vel.length();
            if p.kind == crate::wild::PreyKind::Glowfish {
                let y = consts::WATER_LEVEL - 0.22 + (p.anim * 1.5).sin() * 0.06;
                let m = Mat4::from_translation(Vec3::new(p.pos.x, y, p.pos.z))
                    * Mat4::from_axis_angle(Vec3::Y, -p.yaw + std::f32::consts::FRAC_PI_2 + (p.anim * 3.0).sin() * 0.25)
                    * Mat4::from_axis_angle(Vec3::Z, (p.anim * 3.0).cos() * 0.15);
                r.queue_draw(self.glowfish, m, [1.0; 4], 1.4 + (vfx.time * 3.0).sin() * 0.3, models::FISH_VOXEL, false);
                continue;
            }
            let (mesh, vsize, hop) = match p.kind {
                crate::wild::PreyKind::Hare => (self.hare, models::HARE_VOXEL, if speed > 0.5 { (p.anim * 1.6).sin().abs() * 0.22 } else { 0.0 }),
                _ => (self.stag, models::STAG_VOXEL, if speed > 0.5 { (p.anim * 1.2).sin().abs() * 0.12 } else { 0.0 }),
            };
            let pos = Vec3::new(p.pos.x, crate::render::terrain_height(p.pos.x, p.pos.z).max(consts::WATER_LEVEL - 0.6) + hop, p.pos.z);
            let m = Mat4::from_translation(pos)
                * Mat4::from_axis_angle(Vec3::Y, -p.yaw + std::f32::consts::FRAC_PI_2)
                * Mat4::from_axis_angle(Vec3::X, -(speed * 0.02).min(0.25));
            r.queue_draw(mesh, m, [1.0; 4], 0.0, vsize, false);
        }
        for team in [Team::Blue, Team::Red] {
            let ti = team as usize;
            // At the base, not the origin: all three were drawn with the
            // identity matrix from the first commit, so both teams' gates,
            // fountains and pedestals stood stacked at the map's centre (on
            // the sky map, in the middle of the Moonwell). The gate faces
            // down the lanes, a little past the Elder; the mesh faces +z.
            let face = if team == Team::Blue { 0.0 } else { std::f32::consts::PI };
            let gate_at = team.titan_pos() + Vec3::new(0.0, 0.0, team.push_z() * 12.0);
            let ground = |p: Vec3| Vec3::new(p.x, crate::map::ground_y(p.x, p.z), p.z);
            r.queue_draw(self.gate[ti], Mat4::from_translation(ground(gate_at)) * Mat4::from_axis_angle(Vec3::Y, face), [1.0; 4], 0.0, 0.24, false);
            r.queue_draw(self.fountain[ti], Mat4::from_translation(ground(team.fountain())), [1.0; 4], 0.25, 0.2, false);
            r.queue_draw(self.pedestal[ti], Mat4::from_translation(ground(team.titan_pos())), [1.0; 4], 0.0, 0.22, false);
        }

        // birds circling high above the valley + the ziggurats
        let t = vfx.time;
        let flocks: [(f32, f32, f32, f32, usize); 3] = [
            (150.0, 52.0, 0.05, 1.0, 5),   // valley orbit
            (176.0, 64.0, -0.032, 2.6, 4), // outer orbit, opposite direction
            (24.0, 76.0, 0.22, 0.6, 3),    // circling the north ziggurat
        ];
        for (orbit_r, orbit_y, speed, phase, count) in flocks {
            let cx = t * speed + phase;
            let center = Vec3::new(cx.cos() * orbit_r, orbit_y + (t * 0.13 + phase).sin() * 4.0, cx.sin() * orbit_r);
            let tangent = Vec3::new(-(cx + 0.1).sin() * speed.signum(), 0.0, (cx + 0.1).cos() * speed.signum()).normalize();
            let yaw = -tangent.z.atan2(tangent.x);
            for i in 0..count {
                let back = i as f32 * 2.6;
                let side = if i % 2 == 0 { (i as f32 / 2.0) * 2.2 } else { -((i + 1) as f32 / 2.0) * 2.2 };
                let pos = center - tangent * back + Vec3::new(-tangent.z, 0.0, tangent.x) * side + Vec3::Y * (i as f32 * 0.5);
                let flap = (t * 9.0 + i as f32 * 1.7).sin() * 0.45;
                let m = Mat4::from_translation(pos)
                    * Mat4::from_axis_angle(Vec3::Y, yaw)
                    * Mat4::from_axis_angle(Vec3::Z, flap);
                r.queue_draw(self.bird, m, [1.0; 4], 0.0, 0.5, false);
            }
        }

        // towers
        let pulse = 0.55 + 0.45 * (vfx.time * 2.6).sin();
        for tw in &game.towers {
            // Towers are tall, so their centre is well below their top: cull
            // against a sphere around the MIDDLE of the structure, or a tower
            // whose base is off-screen vanishes while its crystal is still in
            // view.
            if !sphere_in_frustum(&vp, tw.pos + Vec3::Y * TOWER_HALF_H, TOWER_CULL_R) {
                continue;
            }
            // WHO it is after: a tether from the crystal to whatever it is
            // aiming at, live -- it appears the tick you cross the line, not
            // when the first bolt lands. Hot and thick when it is the player.
            if let (true, Some(aim)) = (tw.hp > 0.0, tw.aim) {
                let at_me = aim == crate::sim::TargetRef::God(0);
                let from = tw.pos + Vec3::Y * 5.75;
                let to = game.target_pos(aim) + Vec3::Y * 1.1;
                let c = if at_me { [1.0, 0.18, 0.10] } else { [1.0, 0.70, 0.30] };
                let n = if at_me { 14 } else { 8 };
                for k in 1..n {
                    let t = k as f32 / n as f32;
                    let sag = Vec3::Y * (-(t * (1.0 - t)) * 1.2);
                    let beat = 1.0 + 0.35 * ((vfx.time * 9.0 - t * 8.0).sin());
                    r.queue_draw(self.orb, Mat4::from_translation(from.lerp(to, t) + sag), alpha_tint(c, if at_me { 0.9 } else { 0.5 }), 2.2, if at_me { 0.085 * beat } else { 0.05 }, true);
                }
            }
            let ti = tw.team as usize;
            let m = Mat4::from_translation(tw.pos);
            r.queue_draw(self.tower_body[ti], m, [1.0; 4], 0.0, 0.16, false);
            let cm = Mat4::from_translation(tw.pos + Vec3::Y * 5.75)
                * Mat4::from_axis_angle(Vec3::Y, vfx.time * 1.2);
            r.queue_draw(self.tower_crystal[ti], cm, [1.0; 4], 0.9 + pulse * 0.9, 0.16, false);
        }

        // titans
        for t in 0..2 {
            let titan = &game.titans[t];
            if titan.hp <= 0.0 {
                continue;
            }
            let ti = titan.team as usize;
            // Its walk: a heavy rolling gait -- a bob with each footfall, a
            // sway side to side, a lean into the stride -- and at rest,
            // breathing and a slow look round. It leans into a blow as it
            // swings. (The model faces +z: turn by pi/2 - yaw.)
            let go = (titan.stride / crate::consts::TITAN_SPEED).clamp(0.0, 1.0);
            let bob = titan.walk.sin().abs() * 0.35 * go;
            let sway = titan.walk.cos() * 0.05 * go;
            let look = (1.0 - go) * (vfx.time * 0.23 + ti as f32).sin() * 0.12 * (1.0 - titan.atk_anim);
            let breathe = (vfx.time * 1.1).sin() * 0.02 * (1.0 - go);
            let lean = 0.08 * go + 0.18 * titan.atk_anim;
            let base = Mat4::from_translation(titan.pos + Vec3::Y * bob)
                * Mat4::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2 - titan.yaw + look)
                * Mat4::from_axis_angle(Vec3::X, lean)
                * Mat4::from_axis_angle(Vec3::Z, breathe + sway);
            r.queue_draw(self.titan_body[ti], base, [1.0; 4], 0.0, 0.145, false);
            let swing = -titan.atk_anim * 1.6 - (titan.walk.sin() * 0.25 * go);
            let wm = base
                * Mat4::from_translation(Vec3::new(1.85, 2.4, 0.0))
                * Mat4::from_axis_angle(Vec3::X, swing)
                * Mat4::from_translation(Vec3::new(0.0, 0.6, 0.0));
            r.queue_draw(self.titan_weapon[ti], wm, [1.0; 4], 0.0, 0.145, false);

            // its ground (systems::titan_zone), marked faintly with stones
            // of light: the circle round the pedestal and the push lane
            // out toward the enemy -- step inside and it comes for you
            {
                let (home, pz) = (titan.home, titan.team.push_z());
                let c = titan.team.rgb();
                let mut mark = |p: Vec3| {
                    if crate::systems::titan_zone(home, titan.team, p - Vec3::new(0.0, 0.0, pz * 0.01)) {
                        r.queue_draw(self.orb, Mat4::from_translation(Vec3::new(p.x, crate::map::ground_y(p.x, p.z) + 0.12, p.z)), alpha_tint(c, 0.35), 1.0, 0.06, true);
                    }
                };
                let rr = crate::consts::TITAN_MOVE_R;
                for k in 0..36 {
                    let a = k as f32 / 36.0 * std::f32::consts::TAU;
                    let p = home + Vec3::new(a.cos(), 0.0, a.sin()) * rr;
                    // only where the circle is the edge (not inside the lane)
                    if (p.z - home.z) * pz <= 0.0 || (p.x - home.x).abs() > crate::consts::TITAN_PUSH_HALF - 0.5 {
                        mark(p);
                    }
                }
                let (hx, len) = (crate::consts::TITAN_PUSH_HALF, crate::consts::TITAN_PUSH);
                let n = 12;
                for k in 0..=n {
                    let f = k as f32 / n as f32;
                    for side in [-1.0f32, 1.0] {
                        mark(home + Vec3::new(side * hx, 0.0, pz * (rr * 0.5 + f * (len - rr * 0.5))));
                    }
                    mark(home + Vec3::new((f * 2.0 - 1.0) * hx, 0.0, pz * len));
                }
            }

            // invulnerability shell
            if game.titan_shielded(titan.team) {
                let c = titan.team.rgb();
                for i in 0..40 {
                    let a = i as f32 / 40.0 * std::f32::consts::TAU + vfx.time * 0.7;
                    let y = 0.8 + (i % 8) as f32 * 0.72;
                    r.queue_draw(
                        self.orb,
                        Mat4::from_translation(titan.pos + Vec3::new(a.cos() * 5.6, y, a.sin() * 5.6)),
                        alpha_tint(c, 0.4),
                        1.4,
                        0.10,
                        true,
                    );
                }
            }
        }

        // gods — posed per part. `puppet` decides what the body is doing,
        // `creature-rig` poses the boxes, `rigged` turns boxes into matrices.
        let world = WorldProbe { blockers: &game.blockers, logs: &game.logs, snow: vfx.snow.as_ref() };
        let dt = (vfx.time - vfx.puppets.last_time).clamp(0.0, 0.1);
        vfx.puppets.last_time = vfx.time;
        vfx.puppets.ensure(game.gods.len());
        let mut pevents = std::mem::take(&mut vfx.puppets.events);
        pevents.clear();
        let mut posed = Vec::new();
        // GODS ARE DELIBERATELY NOT CULLED. There are ten of them, so the
        // saving is negligible, and they carry effects -- tethers, auras, the
        // beam a channelled ability draws -- that legitimately extend far past
        // the body. A god culled by their own bounds takes their ability's
        // visuals with them, and an ability that stops rendering while it is
        // still hitting you is worse than ten extra draw calls.
        // an enemy the cloud hides from the player's team is not drawn
        // (skycloud.rs; spectators and bot-only matches see everyone)
        let viewer = game.gods.iter().find(|g| g.is_player).map(|g| g.team);
        for i in 0..game.gods.len() {
            let g = &game.gods[i];
            if g.respawn_t > 0.0 {
                continue;
            }
            let hidden = viewer.is_some_and(|t| !game.sees(t, i));
            let ti = g.team as usize;
            let def = game.def(i);
            let di = g.def_id as usize;
            let rig = &self.god_rig[di];
            let leap = g.leap.as_ref().map(|l| l.view()).or_else(|| g.fall.as_ref().map(|f| f.view())).or_else(|| g.dash.as_ref().map(|d| {
                // The sim's dash arc is y = 1.4 a (1 - a); its slope over
                // ground covered is what the body pitches along.
                let a = (d.t / d.dur.max(1e-3)).clamp(0.0, 1.0);
                let run = Vec3::new(d.to.x - d.from.x, 0.0, d.to.z - d.from.z).length().max(0.5);
                LeapView::Flight { t: a, climb: 1.4 * (1.0 - 2.0 * a) / run * 2.0 }
            })).or_else(|| g.lift.as_ref().map(|r| LeapView::Flight { t: r.t / crate::lift::RIDE_SECS, climb: r.climb() }));
            let inp = PuppetInput {
                pos: g.pos,
                yaw: g.yaw,
                hp01: (g.hp / g.max_hp.max(1.0)).clamp(0.0, 1.0),
                atk_pulse: g.atk_anim,
                strike_style: if def.auto_range > 6.0 { StrikeStyle::Snap } else { StrikeStyle::Swipe },
                leap,
                perched: g.perch.is_some(),
                narrow: g.perch.is_some() && g.perch_narrow,
                quiet: game.quiet(i),
            };
            let puppet = &mut vfx.puppets.puppets[i];
            let before = pevents.len();
            puppet.advance(dt, &inp, rig, &world, &mut pevents);
            // hidden: its body keeps moving (so it is not stale when it
            // steps out), but nothing of it is drawn and it leaves no marks
            if hidden {
                pevents.truncate(before);
                continue;
            }
            let pose = puppet.pose(&inp, rig);
            creature_rig::pose(rig.parts(), rig.torso_half, &pose.motion, None, &mut posed);
            // wading: sink toward the bed (capped so the head stays clear)
            let sink = consts::water_depth(g.pos.x, g.pos.z).min(0.9);
            // Model faces +Z; glam's rotation_y(t) sends +Z to (sin t, 0,
            // cos t), so facing (cos yaw, 0, sin yaw) needs t = pi/2 - yaw.
            // This read `yaw - pi/2`, which mirrors the facing in X: right
            // on the north-south lanes, backwards on any east-west run.
            // dancing (Sproutkit's party): a spin and a hop; giggling: a
            // helpless wobble side to side (`kit`)
            let (spin, hop, roll) = if g.dance_t > 0.0 {
                let k = vfx.time * 6.0 + i as f32;
                (k, (k * 1.5).sin().abs() * 0.35, 0.0)
            } else if g.giggle_t > 0.0 {
                (0.0, (vfx.time * 18.0).sin().abs() * 0.08, (vfx.time * 11.0 + i as f32).sin() * 0.18)
            } else {
                (0.0, 0.0, 0.0)
            };
            let base = Mat4::from_translation(g.pos + Vec3::Y * (pose.lift - sink + hop))
                * Mat4::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2 - g.yaw + spin)
                * Mat4::from_axis_angle(Vec3::Z, roll);
            // Planted paws (`footfall`): world soles into the rig frame. The
            // sim's floor is y = 0 through the river too and the drawn cat
            // is sunk, so the soles sink with it.
            let mut legs = pose.legs;
            for (l, f) in legs.iter_mut().zip(pose.feet) {
                l.plant = f.map(|p| crate::rigged::world_to_rig(rig, base, p - Vec3::Y * sink));
            }
            let mats = slot_transforms(rig, &posed, &legs, &pose.body);
            let smooth = self.ultra && di < self.god_smooth.len();
            for (s, m) in mats.iter().enumerate() {
                if smooth {
                    // smooth and furred; the voxel part casts its shadow
                    r.queue_smooth(self.god_smooth[di][ti][s], base * *m, [1.0; 4]);
                    r.queue_shadow_only(self.god_parts[di][ti][s], base * *m, models::GOD_DRAW_VOXEL);
                } else {
                    r.queue_draw(self.god_parts[di][ti][s], base * *m, [1.0; 4], 0.0, models::GOD_DRAW_VOXEL, false);
                }
            }
            // afterimages of a dash or a blink: this pose, where the cat
            // just was, transparent and fading in its accent colour
            for gh in vfx.afterimages.iter().filter(|a| a.god == i) {
                let k = (gh.life / gh.max).clamp(0.0, 1.0);
                let ghost = Mat4::from_translation(gh.pos + Vec3::Y * (pose.lift - sink)) * Mat4::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2 - gh.yaw);
                let tint = [gh.color[0] * 1.3, gh.color[1] * 1.3, gh.color[2] * 1.3, 0.42 * k];
                for (s, m) in mats.iter().enumerate() {
                    r.queue_draw(self.god_parts[di][ti][s], ghost * *m, tint, 0.8 * k, models::GOD_DRAW_VOXEL, true);
                }
            }

            if self.god_weapon[di][ti] != 0 {
                // The weapon rides the torso, so it lunges and rears with it.
                let anchor = models::weapon_anchor(def);
                let swing = -g.atk_anim * 1.5 + (vfx.time * 1.4).sin() * 0.06;
                let wm = base
                    * mats[Slot::Torso as usize]
                    * Mat4::from_translation(Vec3::from_array(anchor))
                    * Mat4::from_axis_angle(Vec3::X, swing)
                    * Mat4::from_axis_angle(Vec3::Z, if def.auto_range > 6.0 { -0.5 } else { 0.2 })
                    * Mat4::from_translation(Vec3::new(0.0, -0.35, 0.0));
                r.queue_draw(self.god_weapon[di][ti], wm, [1.0; 4], 0.0, models::GOD_DRAW_VOXEL, false);
            }
        }
        // Leaves bend away from whoever is pushing through them, and swing
        // back through where they were. A stalking cat contributes ~nothing.
        vfx.puppets.trail(dt);
        r.brushes = vfx.puppets.brushes();
        r.towers = game.tower_rings(0);
        (r.blast_a, r.blast_b) = vfx.ground.blast_uniform();
        r.cracks = vfx.ground.crack_uniform();
        vfx.puppet_events(&pevents);
        vfx.puppets.events = pevents;

        // the crossing logs: on the lane, or wherever the river left them
        // the sky map's clouds, drifting over the chasms on their timetable;
        // a slow bob and turn so they read as cloud, not platform
        for (k, f) in game.ferries.iter().enumerate() {
            let at = f.at(game.t).pos;
            if !sphere_in_frustum(&vp, at, crate::ferry::CLOUD_R + 3.0) {
                continue;
            }
            let bob = (vfx.time * 0.9 + k as f32 * 1.7).sin() * 0.06;
            let m = Mat4::from_translation(at + Vec3::Y * (bob - self.cloud_top))
                * Mat4::from_axis_angle(Vec3::Y, vfx.time * 0.05 + k as f32);
            r.queue_draw(self.cloud, m, [1.0; 4], 0.35 + atm.night * 0.25, crate::skyprops::CLOUD_VOX, false);
        }
        // ultra's snowpack, for the terrain to draw (snowpack.rs)
        r.snow = vfx.snow.as_ref().map(|s| {
            let (pack, marks) = s.upload();
            (pack, marks, s.params())
        });
        // the sky map's drifting cloud banks and wisps (skycloud.rs) -- the
        // ones that hide cats. Soft, see-through billows; thinned near the
        // camera and round the player's own cat, so a cat in a bank still
        // sees what is at arm's length.
        if crate::map::active() == crate::map::MapId::Sky {
            let me = game.gods.iter().find(|g| g.is_player).map(|g| g.pos);
            for c in crate::skycloud::clouds(game.cloud_seed, game.t) {
                if c.thick < 0.02 || !sphere_in_frustum(&vp, c.pos + Vec3::Y * 3.0, c.r + 12.0) {
                    continue;
                }
                let yaw = c.dir.z.atan2(c.dir.x);
                // forming: each billow condenses on its own (staggered), small
                // at first; dissipating: they spread, rise and thin. Both from
                // the cloud's own clock.
                let smooth = |x: f32| {
                    let x = x.clamp(0.0, 1.0);
                    x * x * (3.0 - 2.0 * x)
                };
                let leaving = smooth((c.age - (c.life - 20.0)) / 20.0);
                for (k, (off, scale, edge)) in crate::skycloud::puffs(&c).into_iter().enumerate() {
                    let stagger = ((k as f32 * 0.618 + c.seed as f32 * 0.001).fract()) * 0.5;
                    let formed = if leaving > 0.0 { 1.0 } else { smooth((c.thick - stagger) / (1.0 - stagger).max(0.2)) };
                    let grow = 0.35 + 0.65 * formed + 0.45 * leaving;
                    // a dash through it pushes the billows aside (vfx::Gust)
                    let mut at = c.pos + off + Vec3::Y * (1.5 * leaving);
                    let mut parted = 0.0f32;
                    for gu in &vfx.gusts {
                        let d = gu.push(at);
                        parted = parted.max(d.length() / 8.0);
                        at += d;
                    }
                    let near_cam = ((at.distance(cam.eye) - 4.0) / 10.0).clamp(0.0, 1.0);
                    let near_me = me.map_or(1.0, |p| ((Vec3::new(at.x - p.x, 0.0, at.z - p.z).length() - 3.0) / 9.0).clamp(0.0, 1.0));
                    let scale = scale * grow;
                    let a = c.thick * edge * 0.75 * near_cam * near_me * formed.max(0.2) * (1.0 - 0.5 * parted.min(1.0));
                    if a < 0.02 {
                        continue;
                    }
                    let m = Mat4::from_translation(at) * Mat4::from_axis_angle(Vec3::Y, -yaw + k as f32 * 0.7) * Mat4::from_scale(scale);
                    r.queue_draw(self.drift_mesh[2][(c.seed as usize + k) % DRIFT_VARIANTS], m, [1.0, 1.0, 1.0, a], 0.3 + atm.night * 0.2, crate::skyprops::PUFF_VOX, true);
                }
            }
        }
        // the sky map's drifting stone and cloud, rising and falling on the
        // slow air (skydrift.rs); an anchor stone rests on a little cloud
        for d in &self.drift {
            use crate::skydrift::DriftKind;
            let at = d.at(vfx.time);
            let (k, vox, reach) = match d.kind {
                DriftKind::Anchor => (0, crate::skyprops::DRIFT_VOX, 4.0),
                DriftKind::Fragment => (1, crate::skyprops::DRIFT_VOX, 3.0),
                DriftKind::Puff => (2, crate::skyprops::PUFF_VOX, 8.0),
            };
            if !sphere_in_frustum(&vp, at.pos, reach * at.scale.max_element() + 2.0) {
                continue;
            }
            let mesh = self.drift_mesh[k][d.variant.rem_euclid(DRIFT_VARIANTS as i32) as usize];
            let m = Mat4::from_translation(at.pos) * Mat4::from_axis_angle(Vec3::Y, at.yaw) * Mat4::from_scale(at.scale);
            let glow = if d.kind == DriftKind::Puff { 0.25 + atm.night * 0.2 } else { 0.0 };
            r.queue_draw(mesh, m, [1.0; 4], glow, vox, false);
            if d.kind == DriftKind::Anchor {
                let under = Mat4::from_translation(at.pos - Vec3::Y * 3.2 * at.scale.y)
                    * Mat4::from_axis_angle(Vec3::Y, at.yaw * 0.5 + 1.0)
                    * Mat4::from_scale(at.scale * Vec3::new(0.55, 0.4, 0.55));
                r.queue_draw(self.drift_mesh[2][(d.variant as usize + 1) % DRIFT_VARIANTS], under, [1.0; 4], 0.25 + atm.night * 0.2, crate::skyprops::PUFF_VOX, false);
            }
        }
        for l in &game.logs {
            use crate::logs::{LOG_RADIUS, LOG_TOP};
            let c = l.centre();
            // The mesh's section axis is at the middle of its grid; put that at
            // the height `logs::surface_y` assumes, and turn about it.
            let ny = (crate::logs::SECTION_B as usize) * 2 + 3;
            let mesh_axis = ((ny / 2) as f32 + 0.5) * crate::logs::LOG_VOXEL;
            let axis_y = LOG_TOP - crate::logs::column_top(0.0);
            let _ = LOG_RADIUS;
            let m = Mat4::from_translation(Vec3::new(c.x, axis_y, c.z))
                * Mat4::from_axis_angle(Vec3::Y, (-l.axis.z).atan2(l.axis.x))
                * Mat4::from_axis_angle(Vec3::X, l.angle)
                * Mat4::from_translation(Vec3::new(0.0, -mesh_axis, 0.0));
            r.queue_draw(self.river_log, m, [1.0; 4], 0.0, crate::logs::LOG_VOXEL, false);
        }

        // minions
        for m in &game.minions {
            // ACTORS WERE NOT CULLED AT ALL. The static chunks above have gone
            // through `sphere_in_frustum` since it was written; the minions,
            // gods and towers never did. In a three-lane MOBA roughly two
            // thirds of the minions are in lanes the camera is not looking at,
            // and every one of them was posed, skinned and submitted.
            //
            // The radius is generous on purpose -- see `ACTOR_CULL_R`. A sphere
            // fitted to the body pops when an arm or a tail swings outside it,
            // and a pop at the screen edge is far more noticeable than the
            // handful of extra draws a loose radius lets through.
            if !sphere_in_frustum(&vp, m.pos, ACTOR_CULL_R) {
                continue;
            }
            let ti = m.team as usize;
            let bob = (m.walk_anim * 2.0).sin().abs() * 0.10;
            let lean = (m.walk_anim * 2.0).sin() * 0.06;
            let pos = m.pos + Vec3::Y * (bob + if game.on_a_log(m.pos.x, m.pos.z) { crate::logs::LOG_TOP } else { 0.0 });
            let base = Mat4::from_translation(pos)
                * Mat4::from_axis_angle(Vec3::Y, -m.yaw + std::f32::consts::FRAC_PI_2)
                * Mat4::from_axis_angle(Vec3::Z, lean);
            // dancing under the glitter-ball: a spin and a hop
            let base = if m.dance_t > 0.0 {
                let k = vfx.time * 7.0 + m.id as f32;
                Mat4::from_translation(Vec3::Y * (k * 1.3).sin().abs() * 0.3) * Mat4::from_translation(pos) * Mat4::from_axis_angle(Vec3::Y, k) * Mat4::from_translation(-pos) * base
            } else {
                base
            };
            let mesh = if m.caster { self.minion_caster[ti] } else { self.minion_melee[ti] };
            r.queue_draw(mesh, base, [1.0; 4], 0.0, 0.09, false);
        }

        // Sproutkit's litter: his own body, small, hopping as it runs
        for k in &game.kitlings {
            if !sphere_in_frustum(&vp, k.pos, ACTOR_CULL_R) {
                continue;
            }
            let hop = ((k.age * 11.0).sin().abs()) * 0.35;
            let yaw = k.dir.z.atan2(k.dir.x);
            let m = Mat4::from_translation(k.pos + Vec3::Y * hop)
                * Mat4::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2 - yaw)
                * Mat4::from_scale(Vec3::splat(crate::kit::KITLING_SCALE));
            r.queue_draw(self.kitling[k.team as usize], m, [1.0; 4], 0.15, models::GOD_DRAW_VOXEL, false);
        }
        // and his glitter-ball, turning over the dance floor
        for z in &game.zones {
            if let crate::sim::ZoneKind::Dance { until, .. } = z.kind {
                let _ = until;
                let at = z.pos + Vec3::Y * (crate::kit::BALL_HEIGHT + (vfx.time * 1.3).sin() * 0.15);
                let m = Mat4::from_translation(at) * Mat4::from_axis_angle(Vec3::Y, vfx.time * 1.4);
                r.queue_draw(self.disco, m, [1.0; 4], 0.0, models::DISCO_VOX, false);
                // the chain it hangs from, up into the sky
                for s in 1..10 {
                    let p = at + Vec3::Y * (1.3 + s as f32 * 0.6);
                    r.queue_draw(self.orb, Mat4::from_translation(p), [0.8, 0.8, 0.85, 1.0], 0.2, 0.05, false);
                }
            }
        }

        // projectiles
        for p in &game.projectiles {
            let dir = p.vel.normalize_or_zero();
            let yaw = dir.z.atan2(dir.x);
            let m = Mat4::from_translation(p.pos) * Mat4::from_axis_angle(Vec3::Y, -yaw);
            match p.kind {
                ProjKind::Bolt => {
                    r.queue_draw(self.orb, m, alpha_tint(p.color, 1.0), 1.8, 0.09, true);
                }
                ProjKind::Arrow => {
                    r.queue_draw(self.arrow[p.team as usize], m, [1.0; 4], 0.6, 0.07, false);
                }
                ProjKind::Lob { .. } => {
                    r.queue_draw(self.boulder, m, [1.0; 4], 0.1, 0.11, false);
                }
            }
        }

        // particle VFX (transparent pass)
        let gpu = vfx.build_gpu(game, cam.eye);
        r.set_particles(gpu);
    }
}

// ---------------------------------------------------------------------------
// World dressing: dense jungle flora, overgrown ruins, rivers + bridges and
// colossal beyond-the-bounds landmarks. Pieces are batched into spatial
// chunks (per voxel-size class) for frustum culling.
// ---------------------------------------------------------------------------

pub(crate) const V_SM: f32 = 0.35;
pub(crate) const V_MED: f32 = 0.6;
const V_BIG: f32 = 1.2;
const V_BRAZ: f32 = 0.12;

pub(crate) struct Piece {
    pub(crate) mesh: VoxelMesh,
    pub(crate) pos: Vec3,
    pub(crate) quarter: u8,
    pub(crate) vsize: f32,
    /// wind sway weight baked into each voxel's misc.y (0 = rigid)
    pub(crate) sway: f32,
}

fn mesh_radius(m: &VoxelMesh) -> f32 {
    m.inst
        .iter()
        .map(|i| (i.off_ao[0] * i.off_ao[0] + i.off_ao[1] * i.off_ao[1] + i.off_ao[2] * i.off_ao[2]).sqrt())
        .fold(0.0f32, f32::max)
}

/// Random spot in the playable jungle, clear of lanes, rivers and bases.
fn jungle_spot(rng: &mut Rng, min_clear: f32) -> Option<(f32, f32)> {
    for _ in 0..40 {
        let a = rng.f32() * std::f32::consts::TAU;
        // Uniform by AREA, not by radius. `22 + u * span` put as many props
        // in the thin inner annulus as in the wide outer one, so the jungle
        // thinned out exactly where the map has the most of it -- and the
        // outer ring is where the deep jungle now is.
        let (r0, r1) = (22.0f32, ARENA_RADIUS - 8.0);
        let r = (r0 * r0 + rng.f32() * (r1 * r1 - r0 * r0)).sqrt();
        let x = a.cos() * r;
        let z = a.sin() * r;
        if jungle_ok(x, z, min_clear) {
            return Some((x, z));
        }
    }
    None
}

/// May scatter put something at `(x, z)`? Off the lanes, out of the river,
/// out of the pits, off the authored centre and out of the tower clearings.
fn jungle_ok(x: f32, z: f32, min_clear: f32) -> bool {
    // ...and not on the authored centre: a random trunk in a run's floor or
    // a doorway undoes the one part of the map that is designed.
    x.hypot(z) < ARENA_RADIUS - 6.0
        && consts::lane_clearance(x, z) > min_clear
        && consts::river_dist(x, z) > 9.5
        && !crate::bosses::in_pit(x, z)
        && !crate::midjungle::reserved(x, z)
        && !consts::in_tower_clearing(x, z)
}

/// Does a fallen log at `(x, z)` fit? `jungle_spot` checks a point, and a
/// log is 13 u long: a centre in the clear put one end across a Run's floor
/// (`scatter_leaves_the_authored_centre_alone` found it when the scatter's
/// random stream moved). Every point along the spine, padded by the log's
/// thickness, has to be jungle.
fn log_fits(x: f32, z: f32, quarter: u8, scale: f32) -> bool {
    let r = PropKind::FallenLog.footprint().radius * scale;
    Blockers::long_spines(PropKind::FallenLog, scale).iter().all(|&(x0, z0, x1, z1)| {
        let turn = |mut px: f32, mut pz: f32| {
            for _ in 0..(quarter % 4) {
                (px, pz) = (pz, -px);
            }
            (x + px, z + pz)
        };
        let (a, b) = (turn(x0, z0), turn(x1, z1));
        (0..=10).all(|k| {
            let t = k as f32 / 10.0;
            let (px, pz) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            jungle_ok(px, pz, 3.0 + r) && [(r, 0.0), (-r, 0.0), (0.0, r), (0.0, -r)].iter().all(|(dx, dz)| !crate::midjungle::reserved(px + dx, pz + dz))
        })
    })
}

/// A grove: a ring of big trees round a bramble bed. Returned as data so
/// the placement rules are testable without meshing anything; `build_world`
/// turns it into meshes, blockers and rough ground.
#[derive(Debug, Clone, PartialEq)]
pub struct Grove {
    pub x: f32,
    pub z: f32,
    /// Radius of the bramble bed at the centre.
    pub bed: f32,
    /// `(kind, x, z, scale)` -- trees on the ring, undergrowth in the bed.
    pub parts: Vec<(PropKind, f32, f32, f32)>,
}

/// The rough ground at the heart of a grove. Slower than any single bush:
/// this is the thing a chase goes AROUND.
pub const BRAMBLE_MUL: f32 = 0.55;

/// Lay out `n` groves. Spaced so two never merge into one wall of trunks,
/// and every part checked against `jungle_ok` individually -- a grove near a
/// lane loses the trees that would stand in it rather than moving the lane.
pub fn groves(rng: &mut Rng, n: usize) -> Vec<Grove> {
    const TAU: f32 = std::f32::consts::TAU;
    let mut out: Vec<Grove> = Vec::new();
    let mut tries = 0;
    while out.len() < n && tries < n * 30 {
        tries += 1;
        let Some((x, z)) = jungle_spot(rng, 11.0) else { continue };
        if out.iter().any(|g| (g.x - x).hypot(g.z - z) < 20.0) {
            continue;
        }
        let bed = 2.8 + rng.f32() * 1.4;
        let mut parts = Vec::new();
        let bamboo_stand = rng.f32() < 0.3;
        let ring = 5.2 + rng.f32() * 1.6;
        let trees = 3 + (rng.f32() * 2.0) as usize;
        let a0 = rng.f32() * TAU;
        for k in 0..trees {
            // Irregular spacing, and never a full ring: a grove you cannot
            // enter is a pillar, and a gap is where the ambush comes from.
            let a = a0 + (k as f32 + 0.25 * rng.f32()) / (trees as f32 + 0.8) * TAU;
            let (tx, tz) = (x + a.cos() * ring, z + a.sin() * ring);
            if !jungle_ok(tx, tz, 7.0) {
                continue;
            }
            let kind = if bamboo_stand { PropKind::Bamboo } else if k % 3 == 2 { PropKind::StranglerFig } else { PropKind::JungleTree };
            parts.push((kind, tx, tz, 0.9 + rng.f32() * 0.3));
        }
        // Dense enough to read as a bed from a raised camera: the first
        // render at 5..8 small plants showed a bush and a fern on bare grass.
        let fill = 10 + (rng.f32() * 5.0) as usize;
        for k in 0..fill {
            let a = rng.f32() * TAU;
            let d = bed * rng.f32().sqrt() * 0.85;
            let (ux, uz) = (x + a.cos() * d, z + a.sin() * d);
            if !jungle_ok(ux, uz, 5.0) {
                continue;
            }
            parts.push((if k % 2 == 0 { PropKind::Bush } else { PropKind::Fern }, ux, uz, 1.0));
        }
        out.push(Grove { x, z, bed, parts });
    }
    out
}

/// Random spot on the outer hills, clear of landmark structures.
fn hill_spot(rng: &mut Rng, landmarks: &[Vec3], keep_out: f32) -> Option<(f32, f32)> {
    for _ in 0..40 {
        let a = rng.f32() * std::f32::consts::TAU;
        let r = ARENA_RADIUS + 5.0 + rng.f32() * 45.0 * consts::OUTER_SCALE;
        let x = a.cos() * r;
        let z = a.sin() * r;
        if x.abs() > 190.0 * consts::OUTER_SCALE || z.abs() > 190.0 * consts::OUTER_SCALE {
            continue;
        }
        let p = Vec3::new(x, 0.0, z);
        if landmarks.iter().any(|l| (Vec3::new(l.x, 0.0, l.z) - p).length() < keep_out) {
            continue;
        }
        return Some((x, z));
    }
    None
}

fn bird_mesh() -> VoxelMesh {
    let mut g = crate::voxel::Grid::new(5, 2, 5);
    g.filled_box(2, 0, 1, 2, 0, 3, solid_tint([0.10, 0.09, 0.08]));
    g.set(1, 1, 2, solid_tint([0.09, 0.08, 0.07]));
    g.set(3, 1, 2, solid_tint([0.09, 0.08, 0.07]));
    g.set(2, 0, 0, solid_tint([0.12, 0.10, 0.09]));
    g.set(2, 0, 4, solid_tint([0.12, 0.10, 0.09]));
    g.mesh(0.5, true)
}

/// The real world's collision and going, without a renderer: what a
/// headless match must run against to be the match anyone plays. Builds the
/// meshes too (they are what the placements come from) and drops them; a few
/// seconds of CPU.
pub fn world_blockers() -> Blockers {
    build_world().2
}

fn build_world() -> (Vec<(VoxelMesh, Vec3, f32, f32)>, VoxelMesh, Blockers) {
    match crate::map::active() {
        crate::map::MapId::Pass => {
            let (pieces, blockers) = crate::passworld::build();
            return batch(pieces, blockers);
        }
        crate::map::MapId::Sky => {
            let (pieces, blockers) = crate::skyworld::build();
            return batch(pieces, blockers);
        }
        crate::map::MapId::Jungle => {}
    }
    const TAU: f32 = std::f32::consts::TAU;
    let mut pieces: Vec<Piece> = Vec::new();
    // Cell pitch of 6 world units: most props sit in one cell, and a unit's
    // query touches a 3x3 neighbourhood rather than scanning all ~400.
    let mut blockers = Blockers::new(crate::consts::ARENA_RADIUS + 20.0, 6.0);
    let mut rng = Rng::new(0x5EED_1234);
    let mut v = 0i32;

    macro_rules! put {
        ($kind:ident; $mesh:expr, $x:expr, $z:expr, $y:expr, $q:expr, $vs:expr) => {{
            // One call site, two outputs: the mesh the renderer draws and the
            // blocker the sim collides against. Threading the PropKind through
            // the macro rather than maintaining a second list of positions is
            // what stops the two drifting -- a prop that moves takes its
            // collision with it, and a new prop cannot be added without the
            // compiler asking which PropKind it is.
            blockers.push_placed(PropKind::$kind, $x, $z, $y, $q, 1.0);
            pieces.push(Piece { mesh: $mesh, pos: Vec3::new($x, $y, $z), quarter: $q, vsize: $vs, sway: 0.0 })
        }};
    }
    // flora: sways in the wind (weight scales the shader's height-based bend)
    macro_rules! plant {
        ($kind:ident; $mesh:expr, $x:expr, $z:expr, $y:expr, $q:expr, $vs:expr, $sw:expr) => {{
            blockers.push_prop(PropKind::$kind, $x, $z, $y, 1.0);
            pieces.push(Piece { mesh: $mesh, pos: Vec3::new($x, $y, $z), quarter: $q, vsize: $vs, sway: $sw })
        }};
    }

    // --- the authored centre: the Hollow and the two screened Runs ---
    // First, so that everything scattered afterwards is placed around it.
    for (i, a) in crate::midjungle::layout().into_iter().enumerate() {
        let (x, z, q, vi) = (a.x, a.z, a.quarter, i as i32);
        match a.kind {
            PropKind::Rock => put!(Rock; props::rock(a.scale), x, z, 0.0, q, V_MED),
            PropKind::BoulderCluster => put!(BoulderCluster; props::boulder_cluster(a.scale, vi), x, z, 0.0, q, V_MED),
            PropKind::RuinPillar => put!(RuinPillar; props::ruin_pillar(a.scale, vi), x, z, 0.0, q, V_MED),
            PropKind::StoneHead => put!(StoneHead; props::stone_head(a.scale, vi), x, z, 0.0, q, V_MED),
            PropKind::FallenLog => {
                blockers.push_placed(PropKind::FallenLog, x, z, 0.0, q, a.scale);
                pieces.push(Piece { mesh: props::fallen_log(a.scale), pos: Vec3::new(x, 0.0, z), quarter: q, vsize: V_MED, sway: 0.0 });
            }
            PropKind::JungleTree => {
                plant!(JungleTree; props::jungle_tree(a.scale, vi), x, z, 0.0, q, V_MED, 1.0);
                blockers.push_jungle_roots(x, z, 0.0, vi);
            }
            PropKind::StranglerFig => plant!(StranglerFig; props::strangler_fig(a.scale, vi), x, z, 0.0, q, V_MED, 0.8),
            PropKind::Palm => plant!(Palm; props::palm(a.scale, vi), x, z, 0.0, q, V_MED, 1.6),
            PropKind::Fern => plant!(Fern; props::fern(vi), x, z, 0.0, 0, V_SM, 2.6),
            PropKind::Bush => plant!(Bush; props::bush(vi), x, z, 0.0, 0, V_SM, 1.8),
            // A kind the layout uses and this match does not build would be
            // a prop that collides and cannot be seen.
            other => panic!("midjungle::layout places {other:?}, which build_world has no mesh arm for"),
        }
    }

    // --- groves: the deep jungle's rooms. A ring of trunks (or a bamboo
    // stand) round a bramble bed you can push through at half speed. Most of
    // the added jungle is this rather than more lone trees: it makes the
    // ground dense without adding walls, which is what `going` is for.
    // Placed BEFORE the scatter, and the scatter kept out of them: placed
    // after, the scatter had already dropped boulders and rocks into the
    // beds, and a grove read as more of the same jungle rather than a room.
    let grove_list = groves(&mut rng, 26);
    let rooms: Vec<(f32, f32, f32)> = grove_list.iter().map(|g| (g.x, g.z, g.bed + 1.5)).collect();
    for gr in grove_list {
        blockers.push_rough(crate::going::Rough {
            shape: crate::blockers::Shape::Circle { x: gr.x, z: gr.z, r: gr.bed },
            mul: BRAMBLE_MUL,
            edge: 1.2,
            ground: crate::going::Ground::Thicket,
        });
        for (kind, x, z, sc) in gr.parts {
            let q = (v % 4) as u8;
            match kind {
                PropKind::JungleTree => {
                    plant!(JungleTree; props::jungle_tree(sc, v), x, z, 0.0, q, V_MED, 1.0);
                    blockers.push_jungle_roots(x, z, 0.0, v);
                }
                PropKind::StranglerFig => plant!(StranglerFig; props::strangler_fig(sc, v), x, z, 0.0, q, V_MED, 0.8),
                PropKind::Bamboo => plant!(Bamboo; props::bamboo(sc, v), x, z, 0.0, q, V_MED, 2.2),
                PropKind::Bush => plant!(Bush; props::bush(v), x, z, 0.0, 0, V_MED, 1.8),
                PropKind::Fern => plant!(Fern; props::fern(v), x, z, 0.0, 0, V_SM, 2.6),
                other => panic!("groves() places {other:?}, which build_world has no mesh arm for"),
            }
            v += 1;
        }
    }

    // Every scatter pass below goes through this: `jungle_spot`, minus the
    // grove beds.
    let jungle_spot = |rng: &mut Rng, min_clear: f32| -> Option<(f32, f32)> {
        for _ in 0..8 {
            let (x, z) = jungle_spot(rng, min_clear)?;
            if !rooms.iter().any(|&(gx, gz, r)| (x - gx).hypot(z - gz) < r) {
                return Some((x, z));
            }
        }
        None
    };

    // --- playable jungle flora ---
    for _ in 0..70 {
        if let Some((x, z)) = jungle_spot(&mut rng, 8.5) {
            plant!(JungleTree; props::jungle_tree(0.85 + rng.f32() * 0.35, v), x, z, 0.0, (v % 4) as u8, V_MED, 1.0);
            blockers.push_jungle_roots(x, z, 0.0, v);
        }
        v += 1;
    }
    for _ in 0..24 {
        if let Some((x, z)) = jungle_spot(&mut rng, 7.0) {
            plant!(Palm; props::palm(0.9 + rng.f32() * 0.3, v), x, z, 0.0, (v % 4) as u8, V_MED, 1.6);
        }
        v += 1;
    }
    for _ in 0..16 {
        if let Some((x, z)) = jungle_spot(&mut rng, 9.0) {
            plant!(StranglerFig; props::strangler_fig(0.9 + rng.f32() * 0.25, v), x, z, 0.0, (v % 4) as u8, V_MED, 0.8);
        }
        v += 1;
    }
    for _ in 0..30 {
        if let Some((x, z)) = jungle_spot(&mut rng, 6.0) {
            plant!(Bamboo; props::bamboo(1.0, v), x, z, 0.0, (v % 4) as u8, V_MED, 2.2);
        }
        v += 1;
    }
    for _ in 0..190 {
        if let Some((x, z)) = jungle_spot(&mut rng, 5.2) {
            plant!(Fern; props::fern(v), x, z, 0.0, 0, V_SM, 2.6);
        }
        v += 1;
    }
    for _ in 0..160 {
        if let Some((x, z)) = jungle_spot(&mut rng, 5.6) {
            plant!(Bush; props::bush(v), x, z, 0.0, 0, V_MED, 1.8);
        }
        v += 1;
    }
    for _ in 0..60 {
        if let Some((x, z)) = jungle_spot(&mut rng, 7.0) {
            put!(Rock; props::rock(0.7 + rng.f32() * 0.6), x, z, 0.0, 0, V_MED);
        }
        v += 1;
    }
    // boulder clusters: in the jungle, and worn ones along the river banks
    for _ in 0..24 {
        if let Some((x, z)) = jungle_spot(&mut rng, 8.0) {
            put!(BoulderCluster; props::boulder_cluster(0.8 + rng.f32() * 0.5, v), x, z, 0.0, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    for _ in 0..40 {
        let a = rng.f32() * std::f32::consts::TAU;
        let r = 20.0 + rng.f32() * (ARENA_RADIUS - 30.0);
        let (x, z) = (a.cos() * r, a.sin() * r);
        let rd = consts::river_dist(x, z);
        if rd > 6.5 && rd < 10.5 && consts::lane_clearance(x, z) > 7.0 {
            put!(BoulderCluster; props::boulder_cluster(0.55 + rng.f32() * 0.4, v), x, z, terrain_height(x, z) - 0.3, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    // --- the objective pits: a ring of boulders and old pillars with two
    // gaps facing the lanes, a stone floor pad in the middle
    for (k, pit) in crate::bosses::pit_positions().iter().enumerate() {
        let n = 18;
        for i in 0..n {
            let a = i as f32 / n as f32 * TAU + k as f32 * 0.3;
            // gaps: toward the map centre and away from it
            let facing = (a.cos() * pit.x.signum()).abs();
            if facing > 0.93 {
                continue;
            }
            let rr = crate::bosses::PIT_RADIUS + 1.5;
            let (x, z) = (pit.x + a.cos() * rr, pit.z + a.sin() * rr);
            if i % 4 == 0 {
                put!(RuinPillar; props::ruin_pillar(1.0, i as i32 + k as i32 * 5), x, z, 0.0, (i % 4) as u8, V_MED);
            } else {
                put!(BoulderCluster; props::boulder_cluster(1.1 + (i % 3) as f32 * 0.25, i as i32), x, z, 0.0, (i % 4) as u8, V_MED);
            }
        }
        put!(LanePad; props::lane_pad(), pit.x, pit.z, -0.05, 0, V_MED);
        for i in 0..3 {
            let a = i as f32 * 2.1 + k as f32;
            put!(Brazier; props::brazier(), pit.x + a.cos() * 6.0, pit.z + a.sin() * 6.0, 0.0, 0, V_BRAZ);
        }
    }

    // vine curtains strung between the trees
    for _ in 0..54 {
        if let Some((x, z)) = jungle_spot(&mut rng, 6.5) {
            plant!(VineCurtain; props::vine_curtain(v), x, z, 0.0, (v % 4) as u8, V_MED, 2.4);
        }
        v += 1;
    }
    for _ in 0..12 {
        if let Some((x, z)) = jungle_spot(&mut rng, 8.0) {
            put!(Crystal; props::crystal([0.5, 0.85, 1.0]), x, z, 0.0, 0, V_SM);
        }
        v += 1;
    }
    for _ in 0..15 {
        if let Some((x, z)) = jungle_spot(&mut rng, 7.5) {
            // Not `put!`: the log's length is its scale, and the capsule
            // has to be as long as the mesh.
            let (ls, q) = (0.85 + rng.f32() * 0.3, (v % 4) as u8);
            if log_fits(x, z, q, ls) {
                blockers.push_placed(PropKind::FallenLog, x, z, 0.0, q, ls);
                pieces.push(Piece { mesh: props::fallen_log(ls), pos: Vec3::new(x, 0.0, z), quarter: q, vsize: V_MED, sway: 0.0 });
            }
        }
        v += 1;
    }
    for _ in 0..18 {
        if let Some((x, z)) = jungle_spot(&mut rng, 8.0) {
            plant!(Pine; props::pine(0.9 + rng.f32() * 0.5), x, z, 0.0, 0, V_MED, 0.9);
        }
        v += 1;
    }
    for _ in 0..18 {
        if let Some((x, z)) = jungle_spot(&mut rng, 8.0) {
            plant!(Oak; props::oak(0.9 + rng.f32() * 0.4), x, z, 0.0, 0, V_MED, 1.0);
        }
        v += 1;
    }

    // --- ruin camps deep in the jungle ---
    let mut camps: Vec<(f32, f32)> = Vec::new();
    let mut tries = 0usize;
    while camps.len() < 9 && tries < 400 {
        tries += 1;
        let a = rng.f32() * TAU;
        let r = 30.0 + rng.f32() * (ARENA_RADIUS - 50.0);
        let (x, z) = (a.cos() * r, a.sin() * r);
        if consts::lane_clearance(x, z) < 17.0 || consts::river_dist(x, z) < 14.0 {
            continue;
        }
        if camps.iter().any(|(cx, cz)| ((cx - x).powi(2) + (cz - z).powi(2)).sqrt() < 34.0) {
            continue;
        }
        camps.push((x, z));
    }
    for (ci, (cx, cz)) in camps.iter().enumerate() {
        let vv = (ci as i32) * 3 + 1;
        match ci % 3 {
            0 => put!(RuinArch; props::ruin_arch(1.0, vv), *cx, *cz, 0.0, (vv % 4) as u8, V_MED),
            1 => put!(RuinWall; props::ruin_wall(1.0, vv), *cx, *cz, 0.0, (vv % 4) as u8, V_MED),
            _ => put!(StoneHead; props::stone_head(1.0, vv), *cx, *cz, 0.0, (vv % 4) as u8, V_MED),
        }
        for k in 0..2 {
            let a = rng.f32() * TAU;
            let rr = 4.5 + rng.f32() * 3.5;
            put!(RuinPillar; props::ruin_pillar(1.0, vv + k as i32), cx + a.cos() * rr, cz + a.sin() * rr, 0.0, (k * 2) as u8, V_MED);
        }
        for k in 0..2 {
            let a = rng.f32() * TAU;
            let rr = 5.0 + rng.f32() * 4.0;
            plant!(Bush; props::bush(v + k), cx + a.cos() * rr, cz + a.sin() * rr, 0.0, 0, V_MED, 1.8);
        }
        for k in 0..2 {
            let a = rng.f32() * TAU;
            let rr = 3.5 + rng.f32() * 4.0;
            plant!(Fern; props::fern(v + k), cx + a.cos() * rr, cz + a.sin() * rr, 0.0, 0, V_SM, 2.6);
        }
        v += 5;
    }

    // The bridges that stood here are gone. The crossings are logs now, and
    // logs move, so they are not part of the static world: see `logs` and
    // the dynamic draw in `Scene::draw`.

    // --- braziers flanking the base gates ---
    for team in [Team::Blue, Team::Red] {
        for sx in [-7.0f32, 7.0] {
            let z = team.base_pos().z + team.push_z() * 16.0;
            put!(Brazier; props::brazier(), sx, z, 0.0, 0, V_BRAZ);
        }
    }

    // --- beyond the bounds: colossal landmarks ---
    let o = consts::OUTER_SCALE;
    let zg = 196.0f32 * o;
    let zign = Vec3::new(0.0, terrain_height(0.0, zg) - 3.0, zg);
    let zigs = Vec3::new(0.0, terrain_height(0.0, -zg) - 3.0, -zg);
    put!(Ziggurat; props::ziggurat(), zign.x, zign.z, zign.y, 2, V_BIG); // stair faces the valley
    put!(Ziggurat; props::ziggurat(), zigs.x, zigs.z, zigs.y, 0, V_BIG);
    let mut landmarks = vec![zign, zigs];

    let (hx, hz) = (164.0 * o, 66.0 * o);
    let h1 = Vec3::new(-hx, terrain_height(-hx, hz) - 2.0, hz);
    let h2 = Vec3::new(hx, terrain_height(hx, -hz) - 2.0, -hz);
    put!(ColossusHead; props::colossus_head(), h1.x, h1.z, h1.y, 1, V_BIG); // face +X, toward the valley
    put!(ColossusHead; props::colossus_head(), h2.x, h2.z, h2.y, 3, V_BIG); // face -X
    landmarks.push(h1);
    landmarks.push(h2);

    for k in 0..5 {
        let a = TAU * (k as f32 / 5.0) + 0.45;
        let x = a.cos() * 178.0 * o;
        let z = a.sin() * 178.0 * o;
        let p = Vec3::new(x, 0.0, z);
        if landmarks.iter().any(|l| (Vec3::new(l.x, 0.0, l.z) - p).length() < 46.0) {
            continue;
        }
        let q = (((a.to_degrees() / 90.0).round() as i32) + 1).rem_euclid(4) as u8;
        put!(AqueductArch; props::aqueduct_arch(), x, z, terrain_height(x, z) - 2.0, q, V_BIG);
        landmarks.push(p);
    }

    // --- overgrown hills ring ---
    for i in 0..20 {
        if let Some((x, z)) = hill_spot(&mut rng, &landmarks, 40.0) {
            plant!(MegaTree; props::mega_tree(0.9 + rng.f32() * 0.2, i), x, z, terrain_height(x, z) - 0.5, (i % 4) as u8, V_BIG, 0.6);
        }
        v += 1;
    }
    for _ in 0..14 {
        if let Some((x, z)) = hill_spot(&mut rng, &landmarks, 24.0) {
            plant!(JungleTree; props::jungle_tree(1.05 + rng.f32() * 0.25, v), x, z, terrain_height(x, z) - 0.2, (v % 4) as u8, V_MED, 1.0);
        }
        v += 1;
    }
    for _ in 0..14 {
        if let Some((x, z)) = hill_spot(&mut rng, &landmarks, 18.0) {
            plant!(Bamboo; props::bamboo(1.0, v), x, z, terrain_height(x, z) - 0.2, (v % 4) as u8, V_MED, 2.2);
        }
        v += 1;
    }
    for _ in 0..8 {
        if let Some((x, z)) = hill_spot(&mut rng, &landmarks, 20.0) {
            plant!(Palm; props::palm(0.95 + rng.f32() * 0.2, v), x, z, terrain_height(x, z) - 0.2, (v % 4) as u8, V_MED, 1.6);
        }
        v += 1;
    }
    for _ in 0..10 {
        if let Some((x, z)) = hill_spot(&mut rng, &landmarks, 14.0) {
            plant!(Bush; props::bush(v), x, z, terrain_height(x, z), 0, V_MED, 1.8);
        }
        v += 1;
    }

    // --- the treeline: the arena's edge as jungle, not as a colour change.
    // Cheap props only (palm ~170 instances, bamboo ~160, bush ~130, pine
    // ~380 against a jungle tree's ~1000): the ring is 1080 u round and has
    // to be dense without spending the instance budget the playable jungle
    // needs. Outside the floor, so it is scenery and blocks nobody who can
    // reach it; skipped where the rivers leave the map.
    {
        let n = 150;
        for i in 0..n {
            let a = (i as f32 + rng.f32() * 0.8) / n as f32 * TAU;
            let r = ARENA_RADIUS + 1.5 + rng.f32() * 9.0;
            let (x, z) = (a.cos() * r, a.sin() * r);
            if consts::river_dist(x, z) < 9.0 {
                continue;
            }
            let y = terrain_height(x, z) - 0.2;
            let q = (v % 4) as u8;
            match i % 5 {
                0 | 3 => plant!(Palm; props::palm(0.9 + rng.f32() * 0.35, v), x, z, y, q, V_MED, 1.6),
                1 => plant!(Bamboo; props::bamboo(1.0 + rng.f32() * 0.3, v), x, z, y, q, V_MED, 2.2),
                2 => plant!(Pine; props::pine(0.9 + rng.f32() * 0.5), x, z, y, 0, V_MED, 0.9),
                _ => plant!(Bush; props::bush(v), x, z, y, 0, V_MED, 1.8),
            }
            v += 1;
        }
    }

    batch(pieces, blockers)
}

/// Batch placed pieces into spatial chunks per voxel-size class: what the
/// renderer uploads. Shared by every map's world builder.
fn batch(mut pieces: Vec<Piece>, mut blockers: Blockers) -> (Vec<(VoxelMesh, Vec3, f32, f32)>, VoxelMesh, Blockers) {
    // every map: the clans' camps behind the bases (clancamp.rs)
    crate::clancamp::build(&mut pieces, &mut blockers);
    // Ordered, not hashed: a HashMap iterates in a fresh random order per
    // build, so chunks came out -- and drew -- in a different order every
    // time the world was built, and z-fighting between coplanar voxels
    // resolved differently. Found by the map switch's round trip (a rebuilt
    // jungle differed from the first by 65 pixels; a re-render without a
    // rebuild by 0).
    let mut cells: std::collections::BTreeMap<(i32, i32, u32), Vec<&Piece>> = std::collections::BTreeMap::new();
    for p in &pieces {
        let cls = if p.vsize == V_SM {
            0u32
        } else if p.vsize == V_MED {
            1
        } else if p.vsize == V_BIG {
            2
        } else {
            3
        };
        let key = ((p.pos.x / 34.0).floor() as i32, (p.pos.z / 34.0).floor() as i32, cls);
        cells.entry(key).or_default().push(p);
    }
    let mut out = Vec::new();
    for (_, plist) in cells {
        let meshes: Vec<VoxelMesh> = plist
            .iter()
            .map(|p| {
                let mut m = p.mesh.clone();
                if p.sway > 0.0 {
                    for i in m.inst.iter_mut() {
                        i.misc[1] = p.sway;
                    }
                }
                m
            })
            .collect();
        let xf: Vec<(Vec3, u8)> = plist.iter().map(|p| (p.pos, p.quarter)).collect();
        let merged = merge_xform(&meshes, &xf);
        if merged.inst.is_empty() {
            continue;
        }
        let mut center = Vec3::ZERO;
        for p in &plist {
            center += p.pos;
        }
        center /= plist.len() as f32;
        // The sphere must hold every piece in 3D. It assumed everything
        // sat near the ground (centre y = 8, horizontal distance only):
        // true on the jungle, false on the pass, whose crags and pines stand
        // 30-60 u up the plateau -- outside the sphere, so culled while on
        // screen or drawn while off it.
        center.y += 8.0;
        let mut radius = 6.0f32;
        for p in &plist {
            radius = radius.max(mesh_radius(&p.mesh) + (p.pos - center).length());
        }
        radius += 6.0;
        out.push((merged, center, radius, plist[0].vsize));
    }
    eprintln!(
        "[clawbound] world: {} props → {} chunks ({} voxels), {} blockers",
        pieces.len(),
        out.len(),
        out.iter().map(|(m, _, _, _)| m.inst.len()).sum::<usize>(),
        blockers.len()
    );
    (out, bird_mesh(), blockers)
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::blockers::Perch;

    /// An ASCII plan of the real world's blockers. `cargo test plan_of -- --nocapture`.
    /// `#` wall (unstandable or tall), `o` perch, `"` cover, `.` lane, ` ` open.
    pub fn plan(b: &Blockers, half: f32, cell: f32) -> String {
        let n = (half * 2.0 / cell) as i32;
        let mut out = String::new();
        for iz in (0..n).rev() {
            for ix in 0..n {
                let (x, z) = (-half + (ix as f32 + 0.5) * cell, -half + (iz as f32 + 0.5) * cell);
                let solid = b.blocked(x, z, 0.0);
                let perch = b.with_perch().any(|(k, p)| {
                    let (ax, az, r) = k.shape.nearest_axis(x, z);
                    p != Perch::None && (x - ax).hypot(z - az) < r
                });
                out.push(if solid && perch { 'o' } else if solid { '#' } else if b.concealment_at(x, z) > 0.3 { '"' } else if consts::lane_clearance(x, z) < 3.5 { '.' } else { ' ' });
            }
            out.push('\n');
        }
        out
    }

    /// The authored centre, checked in the REAL world: after every scatter
    /// pass, camp ring and ruin has been placed round it, the runs are still
    /// open, the doorways still doors, and the lane still clear.
    #[test]
    fn scatter_leaves_the_authored_centre_alone() {
        use crate::midjungle::{DOORS, LANE_KEEP, RUN_X, RUN_Z};
        let (_, _, b) = build_world();
        let body = consts::GOD_RADIUS + 0.2;
        let mut checked = 0;
        for (sx, sz) in [(1.0f32, 1.0f32), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            let mut z = RUN_Z.0;
            while z <= RUN_Z.1 {
                assert!(!b.blocked(sx * RUN_X, sz * z, body), "run ({sx},{sz}) plugged at z = {}", sz * z);
                z += 0.5;
                checked += 1;
            }
            for d in DOORS {
                let mut x = LANE_KEEP - 2.0;
                while x <= RUN_X {
                    assert!(!b.blocked(sx * x, sz * d, body), "doorway z = {} plugged at x = {}", sz * d, sx * x);
                    x += 0.5;
                }
            }
        }
        assert!(checked > 100, "{checked}");
        let mut z = -60.0;
        while z <= 60.0 {
            assert!(!b.blocked(0.0, z, consts::GOD_RADIUS), "mid lane blocked at z = {z}");
            z += 0.5;
        }
        // And the root steps registered for the screen's trees do not push.
        assert!(b.steps().len() > 224, "the screen's jungle trees brought their roots: {}", b.steps().len());
    }

    /// The performance claim, measured on the REAL world rather than argued:
    /// how many instances the static world uploads. Floors on both sides --
    /// a big drop, and not so big that visible surface went with it.
    #[test]
    fn the_world_uploads_its_hull_not_its_insides() {
        let (pieces, _, _) = build_world();
        let hull: usize = pieces.iter().map(|p| p.0.inst.len()).sum();
        println!("static world: {hull} instances in {} pieces", pieces.len());
        assert!(hull > 120_000, "{hull}: did the cull eat the surface?");
        assert!(hull < 520_000, "{hull}: the world is still being uploaded solid (it was ~877k)");
        // And per shape, against the solid: a boulder is mostly inside.
        let mut g = crate::voxel::Grid::new(24, 24, 24);
        g.ellipsoid(12.0, 12.0, 12.0, 10.0, 10.0, 10.0, [0.5, 0.5, 0.5, 1.0]);
        let (h, s) = (g.mesh(0.5, true).inst.len(), g.mesh_solid(0.5, true).inst.len());
        assert!((h as f32) < s as f32 * 0.45, "hull {h} of solid {s}");
        // Leaves with alpha keep what is behind them.
        let mut a = crate::voxel::Grid::new(5, 5, 5);
        a.filled_box(0, 0, 0, 4, 4, 4, [0.2, 0.6, 0.2, 0.5]);
        assert_eq!(a.mesh(0.5, true).inst.len(), 125, "nothing behind a translucent voxel is culled");
    }

    /// Every voxel of every chunk lies inside that chunk's culling sphere,
    /// on both maps. The spheres assumed everything stood near the ground;
    /// the pass's plateau crags and pines did not, so they could be culled
    /// while on screen.
    #[test]
    fn every_voxel_is_inside_its_chunks_culling_sphere() {
        for m in crate::map::MapId::ALL {
            crate::map::with(m, || {
                let (chunks, _, _) = build_world();
                for (mesh, center, radius, _) in &chunks {
                    for i in &mesh.inst {
                        let p = Vec3::new(i.off_ao[0], i.off_ao[1], i.off_ao[2]);
                        assert!((p - *center).length() <= *radius + 1e-3, "{m:?}: voxel at {p} outside its chunk's sphere ({center}, r {radius})");
                    }
                }
            });
        }
    }

    /// Near: the full mesh. Past LOD_DIST: the coarse copy at twice the
    /// voxel size. Fine detail past DETAIL_DIST: nothing.
    #[test]
    fn chunks_choose_their_detail_by_distance() {
        let ch = |vsize: f32| PropChunk { mesh: 1, center: Vec3::ZERO, radius: 10.0, vsize, lod: Some(2) };
        let at = |d: f32| Vec3::new(10.0 + d, 0.0, 0.0);
        assert_eq!(chunk_choice(&ch(0.6), at(LOD_DIST - 1.0)), Some((1, 0.6)));
        assert_eq!(chunk_choice(&ch(0.6), at(LOD_DIST + 1.0)), Some((2, 1.2)));
        assert_eq!(chunk_choice(&ch(0.35), at(DETAIL_DIST + 1.0)), None);
        assert_eq!(chunk_choice(&ch(0.6), at(DETAIL_DIST + 50.0)), Some((2, 1.2)), "only fine detail is dropped");
        let coarse = PropChunk { lod: None, ..ch(1.2) };
        assert_eq!(chunk_choice(&coarse, at(300.0)), Some((1, 1.2)));
    }

    /// Building a world twice gives the same chunks in the same order: draw
    /// order decides z-fighting, and a HashMap-ordered build drew a rebuilt
    /// map differently from the first build (the map switch's round trip).
    #[test]
    fn a_world_builds_the_same_every_time() {
        let key = |w: &(Vec<(VoxelMesh, Vec3, f32, f32)>, VoxelMesh, Blockers)| -> Vec<(Vec3, usize, u32)> {
            w.0.iter().map(|(m, c, _, v)| (*c, m.inst.len(), (*v * 100.0) as u32)).collect()
        };
        let (a, b) = (build_world(), build_world());
        assert_eq!(key(&a), key(&b));
    }

    /// Twinfang Pass within its budget. The crags first came to 1.07M
    /// instances; wider spacing and coarse (1.2 u) voxels for the high and
    /// plateau ones brought them to ~375k with the same silhouette.
    #[test]
    fn the_pass_uploads_within_its_budget() {
        let hull: usize = crate::map::with(crate::map::MapId::Pass, || build_world().0.iter().map(|p| p.0.inst.len()).sum());
        println!("pass static world: {hull} instances");
        assert!(hull > 150_000, "{hull}: where did the crags go?");
        assert!(hull < 450_000, "{hull}: over the pass's budget");
    }

    /// The lanes are the fast ground and stay that way: no rough patch, no
    /// grove, no bramble anywhere on a lane's walk. Minions path by
    /// waypoint and never steer, so this is also what keeps waves on time.
    #[test]
    fn the_lanes_are_open_going_and_the_jungle_is_not() {
        use crate::going::Carriage;
        let (_, _, b) = build_world();
        let run = Carriage::default();
        for lane in consts::LANES {
            for p in consts::lane_path(lane, Team::Blue) {
                assert_eq!(b.going_at(p.x, p.z, run), 1.0, "{lane:?} lane is rough at {p}");
            }
        }
        // And the jungle is: sample the floor off the lanes.
        let (mut n, mut rough) = (0, 0);
        let mut rng = Rng::new(77);
        while n < 4000 {
            if let Some((x, z)) = jungle_spot(&mut rng, 6.0) {
                n += 1;
                rough += (b.going_at(x, z, run) < 0.99) as usize;
            }
        }
        let frac = rough as f32 / n as f32;
        println!("rough fraction of the jungle floor: {frac:.3} ({} patches)", b.going().len());
        assert!(frac > 0.10 && frac < 0.55, "{frac}: the jungle should cost something, and not everywhere");
    }

    /// Groves are rooms, not pillars: every one has a gap wide enough for a
    /// cat, and none crowds a lane or another grove.
    #[test]
    fn groves_have_a_way_in_and_keep_off_the_lanes() {
        let mut rng = Rng::new(0x5EED_1234);
        let gs = groves(&mut rng, 26);
        assert!(gs.len() >= 20, "only {} groves fit", gs.len());
        for (i, g) in gs.iter().enumerate() {
            assert!(consts::lane_clearance(g.x, g.z) > 11.0);
            for h in &gs[i + 1..] {
                assert!((g.x - h.x).hypot(g.z - h.z) >= 20.0);
            }
            let mut angles: Vec<f32> = g.parts.iter().filter(|p| !matches!(p.0, PropKind::Bush | PropKind::Fern)).map(|p| (p.2 - g.z).atan2(p.1 - g.x)).collect();
            angles.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if angles.len() >= 2 {
                let mut widest = angles[0] + std::f32::consts::TAU - angles[angles.len() - 1];
                for w in angles.windows(2) {
                    widest = widest.max(w[1] - w[0]);
                }
                // ring radius >= 5.2: an arc of 1.2 rad there is > 6 u.
                assert!(widest > 1.2, "grove at ({:.0},{:.0}) is closed: widest gap {widest}", g.x, g.z);
            }
            for p in &g.parts {
                assert!(jungle_ok(p.1, p.2, 5.0), "{:?} placed off the jungle at ({}, {})", p.0, p.1, p.2);
            }
        }
    }

    #[test]
    fn plan_of_the_centre() {
        let (_, _, b) = build_world();
        println!("{}", plan(&b, 62.0, 2.0));
    }
}

impl Scene {
    /// Draw a targeting guide: glowing dots on the terrain at `points`
    /// (ground plane positions), in the ability's colour.
    pub fn draw_guide(&self, r: &mut Renderer, points: &[Vec3], color: [f32; 3]) {
        for p in points {
            let y = crate::render::terrain_height(p.x, p.z).max(crate::map::ground_y(p.x, p.z)) + 0.06;
            r.queue_draw(self.guide_dot, Mat4::from_translation(Vec3::new(p.x, y, p.z)), [color[0], color[1], color[2], 1.0], 1.6, 0.12, false);
        }
    }
}

/// Put the world on map `m`: the active map (thread-local), the renderer's
/// terrain/water/flood meshes, and a scene built for it -- the old scene's
/// GPU meshes freed first. Returns the scene to use (the old one untouched
/// when everything is already on `m`). The app's map switch and the
/// headless switch check both run this.
pub fn switch_world(r: &mut Renderer, old: Option<Scene>, m: crate::map::MapId) -> Scene {
    if let Some(sc) = old {
        if crate::map::active() == m && r.world_map == m && sc.map == m {
            return sc;
        }
        crate::map::set_active(m);
        r.rebuild_world_geometry();
        r.free_meshes_from(sc.first_mesh);
    } else {
        crate::map::set_active(m);
        r.rebuild_world_geometry();
    }
    Scene::new(r)
}
