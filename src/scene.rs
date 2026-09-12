// Scene: uploads all voxel meshes once, then records per-frame draw calls
// from the game state (gods, minions, towers, titans, projectiles, props).

use glam::{Mat4, Vec3};

use crate::consts::{self, Lane, Team, ARENA_RADIUS};
use crate::gods::{ROSTER, ROSTER_LEN};
use crate::models;
use crate::props;
use crate::render::{sphere_in_frustum, terrain_height, Camera, Renderer};
use crate::sim::{Game, ProjKind, Rng};
use crate::vfx::Vfx;
use crate::voxel::{merge_xform, VoxelMesh};

pub struct Scene {
    pub god_body: [[u64; 2]; ROSTER_LEN],
    pub god_weapon: [[u64; 2]; ROSTER_LEN],
    pub minion_melee: [u64; 2],
    pub minion_caster: [u64; 2],
    pub tower_body: [u64; 2],
    pub tower_crystal: [u64; 2],
    pub titan_body: [u64; 2],
    pub titan_weapon: [u64; 2],
    pub chunks: Vec<PropChunk>,
    pub gate: [u64; 2],
    pub fountain: [u64; 2],
    pub pedestal: [u64; 2],
    pub bird: u64,
    pub hare: u64,
    pub stag: u64,
    pub glowfish: u64,
    pub heal_tree: u64,
    pub heal_fruit: u64,
    pub orb: u64,
    pub arrow: [u64; 2],
    pub boulder: u64,
    pub total_voxels: usize,
}

/// One frustum-cullable batch of static world voxels.
pub struct PropChunk {
    pub mesh: u64,
    pub center: Vec3,
    pub radius: f32,
    pub vsize: f32,
}

fn solid_tint(c: [f32; 3]) -> [f32; 4] {
    [c[0], c[1], c[2], 1.0]
}

fn alpha_tint(c: [f32; 3], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], a]
}

impl Scene {
    pub fn new(r: &mut Renderer) -> Scene {
        let mut total = 0usize;

        let mut god_body = [[0u64; 2]; ROSTER_LEN];
        let mut god_weapon = [[0u64; 2]; ROSTER_LEN];
        for (i, def) in ROSTER.iter().enumerate() {
            for team in [Team::Blue, Team::Red] {
                let ti = team as usize;
                let body = models::build_god(def, team);
                total += body.inst.len();
                god_body[i][ti] = r.upload_mesh(&body);
                if let Some(w) = models::build_weapon(def.weapon, def, team) {
                    total += w.inst.len();
                    god_weapon[i][ti] = r.upload_mesh(&w);
                }
            }
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

        let (chunk_builds, bird_mesh) = build_world();
        let mut chunks = Vec::new();
        for (mesh, center, radius, vsize) in chunk_builds {
            total += mesh.inst.len();
            let id = r.upload_mesh(&mesh);
            chunks.push(PropChunk { mesh: id, center, radius, vsize });
        }
        total += bird_mesh.inst.len();
        let bird = r.upload_mesh(&bird_mesh);

        let orb = models::build_orb([1.0, 1.0, 1.0]);
        total += orb.inst.len();
        let orb = r.upload_mesh(&orb);
        let hare_m = models::build_hare();
        let stag_m = models::build_stag();
        let fish_m = models::build_glowfish();
        total += fish_m.inst.len();
        let glowfish = r.upload_mesh(&fish_m);
        let (tree_m, fruit_m) = models::build_heal_tree();
        total += hare_m.inst.len() + stag_m.inst.len() + tree_m.inst.len() + fruit_m.inst.len();
        let hare = r.upload_mesh(&hare_m);
        let stag = r.upload_mesh(&stag_m);
        let heal_tree = r.upload_mesh(&tree_m);
        let heal_fruit = r.upload_mesh(&fruit_m);
        let boulder = models::build_boulder();
        total += boulder.inst.len();
        let boulder = r.upload_mesh(&boulder);

        Scene {
            god_body,
            god_weapon,
            minion_melee,
            minion_caster,
            tower_body,
            tower_crystal,
            titan_body,
            titan_weapon,
            chunks,
            gate,
            fountain,
            pedestal,
            bird,
            hare,
            stag,
            glowfish,
            heal_tree,
            heal_fruit,
            orb,
            arrow,
            boulder,
            total_voxels: total,
        }
    }

    pub fn draw(&self, r: &mut Renderer, game: &Game, vfx: &mut Vfx, cam: Camera) {
        r.clear_queues();

        // static world chunks — frustum culled. Snow cover frosts the flora
        // (a cool, brightened tint; the terrain shader whitens the ground).
        let atm = game.weather.atmosphere();
        let frost = atm.snow * 0.55 + game.weather.wetness * atm.snow * 0.25;
        let chunk_tint = [1.0 + frost * 0.35, 1.0 + frost * 0.42, 1.0 + frost * 0.55, 1.0];
        let vp = cam.view_proj(r.width as f32 / r.height.max(1) as f32);
        for ch in &self.chunks {
            if sphere_in_frustum(&vp, ch.center, ch.radius) {
                r.queue_draw(ch.mesh, Mat4::IDENTITY, chunk_tint, 0.0, ch.vsize, false);
            }
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
            r.queue_draw(self.gate[ti], Mat4::IDENTITY, [1.0; 4], 0.0, 0.24, false);
            r.queue_draw(self.fountain[ti], Mat4::IDENTITY, [1.0; 4], 0.25, 0.2, false);
            r.queue_draw(self.pedestal[ti], Mat4::IDENTITY, [1.0; 4], 0.0, 0.22, false);
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
            let flip = if titan.team == Team::Blue { 0.0 } else { std::f32::consts::PI };
            let base = Mat4::from_translation(titan.pos)
                * Mat4::from_axis_angle(Vec3::Y, flip)
                * Mat4::from_axis_angle(Vec3::Z, (vfx.time * 1.1).sin() * 0.02);
            r.queue_draw(self.titan_body[ti], base, [1.0; 4], 0.0, 0.145, false);
            let swing = -titan.atk_anim * 1.6;
            let wm = base
                * Mat4::from_translation(Vec3::new(1.85, 2.4, 0.0))
                * Mat4::from_axis_angle(Vec3::X, swing)
                * Mat4::from_translation(Vec3::new(0.0, 0.6, 0.0));
            r.queue_draw(self.titan_weapon[ti], wm, [1.0; 4], 0.0, 0.145, false);

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

        // gods
        for i in 0..game.gods.len() {
            let g = &game.gods[i];
            if g.respawn_t > 0.0 {
                continue;
            }
            let ti = g.team as usize;
            let def = game.def(i);
            let di = g.def_id as usize;
            let moving = g.move_dir.length_squared() > 0.01;
            let bob = if moving { (g.walk_anim * 2.0).sin().abs() * 0.09 } else { (g.walk_anim * 0.7).sin() * 0.02 };
            let lean = if moving { (g.walk_anim * 2.0).sin() * 0.045 } else { 0.0 };
            let lunge = g.atk_anim * 0.45;
            let fwd = Vec3::new(g.yaw.cos(), 0.0, g.yaw.sin());
            // wading: sink toward the bed (capped so the head stays clear)
            let sink = consts::water_depth(g.pos.x, g.pos.z).min(0.9);
            let pos = g.pos + fwd * lunge + Vec3::Y * (bob - sink);
            let base = Mat4::from_translation(pos)
                * Mat4::from_axis_angle(Vec3::Y, g.yaw - std::f32::consts::FRAC_PI_2)
                * Mat4::from_axis_angle(Vec3::Z, lean);
            r.queue_draw(self.god_body[di][ti], base, [1.0; 4], 0.0, models::GOD_DRAW_VOXEL, false);

            if self.god_weapon[di][ti] != 0 {
                let anchor = models::weapon_anchor(def);
                let swing = -g.atk_anim * 1.5
                    + if moving { (g.walk_anim * 2.0).sin() * 0.25 } else { (vfx.time * 1.4).sin() * 0.06 };
                let wm = base
                    * Mat4::from_translation(Vec3::from_array(anchor))
                    * Mat4::from_axis_angle(Vec3::X, swing)
                    * Mat4::from_axis_angle(Vec3::Z, if def.auto_range > 6.0 { -0.5 } else { 0.2 })
                    * Mat4::from_translation(Vec3::new(0.0, -0.35, 0.0));
                r.queue_draw(self.god_weapon[di][ti], wm, [1.0; 4], 0.0, models::GOD_DRAW_VOXEL, false);
            }
        }

        // minions
        for m in &game.minions {
            let ti = m.team as usize;
            let bob = (m.walk_anim * 2.0).sin().abs() * 0.10;
            let lean = (m.walk_anim * 2.0).sin() * 0.06;
            let pos = m.pos + Vec3::Y * bob;
            let base = Mat4::from_translation(pos)
                * Mat4::from_axis_angle(Vec3::Y, -m.yaw + std::f32::consts::FRAC_PI_2)
                * Mat4::from_axis_angle(Vec3::Z, lean);
            let mesh = if m.caster { self.minion_caster[ti] } else { self.minion_melee[ti] };
            r.queue_draw(mesh, base, [1.0; 4], 0.0, 0.09, false);
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

const V_SM: f32 = 0.35;
const V_MED: f32 = 0.6;
const V_BIG: f32 = 1.2;
const V_BRAZ: f32 = 0.12;

struct Piece {
    mesh: VoxelMesh,
    pos: Vec3,
    quarter: u8,
    vsize: f32,
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
        let r = 22.0 + rng.f32() * (ARENA_RADIUS - 30.0);
        let x = a.cos() * r;
        let z = a.sin() * r;
        if consts::lane_clearance(x, z) > min_clear && consts::river_dist(x, z) > 9.5 {
            return Some((x, z));
        }
    }
    None
}

/// Random spot on the outer hills, clear of landmark structures.
fn hill_spot(rng: &mut Rng, landmarks: &[Vec3], keep_out: f32) -> Option<(f32, f32)> {
    for _ in 0..40 {
        let a = rng.f32() * std::f32::consts::TAU;
        let r = ARENA_RADIUS + 5.0 + rng.f32() * 45.0;
        let x = a.cos() * r;
        let z = a.sin() * r;
        if x.abs() > 190.0 || z.abs() > 190.0 {
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

/// Find where a side lane crosses its river (for the wooden bridge).
fn bridge_crossing(side: f32, lane: Lane) -> Option<Vec3> {
    let pts = lane.polyline();
    let mut best: Option<(Vec3, f32)> = None;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = b - a;
        for i in 0..=48 {
            let t = i as f32 / 48.0;
            let p = a + ab * t;
            let diff = (p.x - consts::river_x(side, p.z)).abs();
            if diff > 9.0 {
                continue;
            }
            // keep clear of the lane towers
            let mut ok = true;
            for team in [Team::Blue, Team::Red] {
                for inner in [false, true] {
                    if (consts::tower_pos(lane, team, inner) - p).length() < 13.0 {
                        ok = false;
                    }
                }
            }
            if !ok {
                continue;
            }
            if best.map_or(true, |(_, d)| diff < d) {
                best = Some((p, diff));
            }
        }
    }
    best.map(|(p, _)| p)
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

fn build_world() -> (Vec<(VoxelMesh, Vec3, f32, f32)>, VoxelMesh) {
    const TAU: f32 = std::f32::consts::TAU;
    let mut pieces: Vec<Piece> = Vec::new();
    let mut rng = Rng::new(0x5EED_1234);
    let mut v = 0i32;

    macro_rules! put {
        ($mesh:expr, $x:expr, $z:expr, $y:expr, $q:expr, $vs:expr) => {
            pieces.push(Piece { mesh: $mesh, pos: Vec3::new($x, $y, $z), quarter: $q, vsize: $vs })
        };
    }

    // --- playable jungle flora ---
    for _ in 0..56 {
        if let Some((x, z)) = jungle_spot(&mut rng, 8.5) {
            put!(props::jungle_tree(0.85 + rng.f32() * 0.35, v), x, z, 0.0, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    for _ in 0..18 {
        if let Some((x, z)) = jungle_spot(&mut rng, 7.0) {
            put!(props::palm(0.9 + rng.f32() * 0.3, v), x, z, 0.0, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    for _ in 0..12 {
        if let Some((x, z)) = jungle_spot(&mut rng, 9.0) {
            put!(props::strangler_fig(0.9 + rng.f32() * 0.25, v), x, z, 0.0, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    for _ in 0..22 {
        if let Some((x, z)) = jungle_spot(&mut rng, 6.0) {
            put!(props::bamboo(1.0, v), x, z, 0.0, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    for _ in 0..120 {
        if let Some((x, z)) = jungle_spot(&mut rng, 5.2) {
            put!(props::fern(v), x, z, 0.0, 0, V_SM);
        }
        v += 1;
    }
    for _ in 0..100 {
        if let Some((x, z)) = jungle_spot(&mut rng, 5.6) {
            put!(props::bush(v), x, z, 0.0, 0, V_MED);
        }
        v += 1;
    }
    for _ in 0..46 {
        if let Some((x, z)) = jungle_spot(&mut rng, 7.0) {
            put!(props::rock(0.7 + rng.f32() * 0.6), x, z, 0.0, 0, V_MED);
        }
        v += 1;
    }
    // boulder clusters: in the jungle, and worn ones along the river banks
    for _ in 0..18 {
        if let Some((x, z)) = jungle_spot(&mut rng, 8.0) {
            put!(props::boulder_cluster(0.8 + rng.f32() * 0.5, v), x, z, 0.0, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    for _ in 0..40 {
        let a = rng.f32() * std::f32::consts::TAU;
        let r = 20.0 + rng.f32() * (ARENA_RADIUS - 30.0);
        let (x, z) = (a.cos() * r, a.sin() * r);
        let rd = consts::river_dist(x, z);
        if rd > 6.5 && rd < 10.5 && consts::lane_clearance(x, z) > 7.0 {
            put!(props::boulder_cluster(0.55 + rng.f32() * 0.4, v), x, z, terrain_height(x, z) - 0.3, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    // vine curtains strung between the trees
    for _ in 0..34 {
        if let Some((x, z)) = jungle_spot(&mut rng, 6.5) {
            put!(props::vine_curtain(v), x, z, 0.0, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    for _ in 0..12 {
        if let Some((x, z)) = jungle_spot(&mut rng, 8.0) {
            put!(props::crystal([0.5, 0.85, 1.0]), x, z, 0.0, 0, V_SM);
        }
        v += 1;
    }
    for _ in 0..11 {
        if let Some((x, z)) = jungle_spot(&mut rng, 7.5) {
            put!(props::fallen_log(0.85 + rng.f32() * 0.3), x, z, 0.0, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    for _ in 0..14 {
        if let Some((x, z)) = jungle_spot(&mut rng, 8.0) {
            put!(props::pine(0.9 + rng.f32() * 0.5), x, z, 0.0, 0, V_MED);
        }
        v += 1;
    }
    for _ in 0..14 {
        if let Some((x, z)) = jungle_spot(&mut rng, 8.0) {
            put!(props::oak(0.9 + rng.f32() * 0.4), x, z, 0.0, 0, V_MED);
        }
        v += 1;
    }

    // --- ruin camps deep in the jungle ---
    let mut camps: Vec<(f32, f32)> = Vec::new();
    let mut tries = 0usize;
    while camps.len() < 7 && tries < 400 {
        tries += 1;
        let a = rng.f32() * TAU;
        let r = 30.0 + rng.f32() * 85.0;
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
            0 => put!(props::ruin_arch(1.0, vv), *cx, *cz, 0.0, (vv % 4) as u8, V_MED),
            1 => put!(props::ruin_wall(1.0, vv), *cx, *cz, 0.0, (vv % 4) as u8, V_MED),
            _ => put!(props::stone_head(1.0, vv), *cx, *cz, 0.0, (vv % 4) as u8, V_MED),
        }
        for k in 0..2 {
            let a = rng.f32() * TAU;
            let rr = 4.5 + rng.f32() * 3.5;
            put!(props::ruin_pillar(1.0, vv + k as i32), cx + a.cos() * rr, cz + a.sin() * rr, 0.0, (k * 2) as u8, V_MED);
        }
        for k in 0..2 {
            let a = rng.f32() * TAU;
            let rr = 5.0 + rng.f32() * 4.0;
            put!(props::bush(v + k), cx + a.cos() * rr, cz + a.sin() * rr, 0.0, 0, V_MED);
        }
        for k in 0..2 {
            let a = rng.f32() * TAU;
            let rr = 3.5 + rng.f32() * 4.0;
            put!(props::fern(v + k), cx + a.cos() * rr, cz + a.sin() * rr, 0.0, 0, V_SM);
        }
        v += 5;
    }

    // --- bridges where the side lanes cross the rivers ---
    for (side, lane) in [(-1.0f32, Lane::Top), (1.0f32, Lane::Bot)] {
        if let Some(p) = bridge_crossing(side, lane) {
            put!(props::bridge(32), p.x, p.z, -0.35, 0, V_MED);
        }
    }

    // --- braziers flanking the base gates ---
    for team in [Team::Blue, Team::Red] {
        for sx in [-7.0f32, 7.0] {
            let z = team.base_pos().z + team.push_z() * 16.0;
            put!(props::brazier(), sx, z, 0.0, 0, V_BRAZ);
        }
    }

    // --- beyond the bounds: colossal landmarks ---
    let zg = 196.0f32;
    let zign = Vec3::new(0.0, terrain_height(0.0, zg) - 3.0, zg);
    let zigs = Vec3::new(0.0, terrain_height(0.0, -zg) - 3.0, -zg);
    put!(props::ziggurat(), zign.x, zign.z, zign.y, 2, V_BIG); // stair faces the valley
    put!(props::ziggurat(), zigs.x, zigs.z, zigs.y, 0, V_BIG);
    let mut landmarks = vec![zign, zigs];

    let h1 = Vec3::new(-164.0, terrain_height(-164.0, 66.0) - 2.0, 66.0);
    let h2 = Vec3::new(164.0, terrain_height(164.0, -66.0) - 2.0, -66.0);
    put!(props::colossus_head(), h1.x, h1.z, h1.y, 1, V_BIG); // face +X, toward the valley
    put!(props::colossus_head(), h2.x, h2.z, h2.y, 3, V_BIG); // face -X
    landmarks.push(h1);
    landmarks.push(h2);

    for k in 0..5 {
        let a = TAU * (k as f32 / 5.0) + 0.45;
        let x = a.cos() * 178.0;
        let z = a.sin() * 178.0;
        let p = Vec3::new(x, 0.0, z);
        if landmarks.iter().any(|l| (Vec3::new(l.x, 0.0, l.z) - p).length() < 46.0) {
            continue;
        }
        let q = (((a.to_degrees() / 90.0).round() as i32) + 1).rem_euclid(4) as u8;
        put!(props::aqueduct_arch(), x, z, terrain_height(x, z) - 2.0, q, V_BIG);
        landmarks.push(p);
    }

    // --- overgrown hills ring ---
    for i in 0..20 {
        if let Some((x, z)) = hill_spot(&mut rng, &landmarks, 40.0) {
            put!(props::mega_tree(0.9 + rng.f32() * 0.2, i), x, z, terrain_height(x, z) - 0.5, (i % 4) as u8, V_BIG);
        }
        v += 1;
    }
    for _ in 0..14 {
        if let Some((x, z)) = hill_spot(&mut rng, &landmarks, 24.0) {
            put!(props::jungle_tree(1.05 + rng.f32() * 0.25, v), x, z, terrain_height(x, z) - 0.2, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    for _ in 0..14 {
        if let Some((x, z)) = hill_spot(&mut rng, &landmarks, 18.0) {
            put!(props::bamboo(1.0, v), x, z, terrain_height(x, z) - 0.2, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    for _ in 0..8 {
        if let Some((x, z)) = hill_spot(&mut rng, &landmarks, 20.0) {
            put!(props::palm(0.95 + rng.f32() * 0.2, v), x, z, terrain_height(x, z) - 0.2, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    for _ in 0..10 {
        if let Some((x, z)) = hill_spot(&mut rng, &landmarks, 14.0) {
            put!(props::bush(v), x, z, terrain_height(x, z), 0, V_MED);
        }
        v += 1;
    }

    // --- batch into spatial chunks per voxel-size class ---
    let mut cells: std::collections::HashMap<(i32, i32, u32), Vec<&Piece>> = std::collections::HashMap::new();
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
        let meshes: Vec<VoxelMesh> = plist.iter().map(|p| p.mesh.clone()).collect();
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
        let mut radius = 6.0f32;
        for p in &plist {
            let dxz = (Vec3::new(p.pos.x, 0.0, p.pos.z) - Vec3::new(center.x, 0.0, center.z)).length();
            radius = radius.max(mesh_radius(&p.mesh) + dxz);
        }
        center.y = 8.0;
        radius += 14.0;
        out.push((merged, center, radius, plist[0].vsize));
    }
    eprintln!(
        "[clawbound] world: {} props → {} chunks ({} voxels)",
        pieces.len(),
        out.len(),
        out.iter().map(|(m, _, _, _)| m.inst.len()).sum::<usize>()
    );
    (out, bird_mesh())
}
