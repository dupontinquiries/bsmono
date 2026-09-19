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

pub struct Scene {
    /// Ancestors cut into posable parts: `[ancestor][team][slot]`.
    pub god_parts: Vec<[[u64; SLOTS]; 2]>,
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
    pub total_voxels: usize,
    /// Static prop collision, built alongside the meshes in `build_world` so
    /// the two cannot disagree. Handed to `Game` by the app; see
    /// `Game::blockers`.
    pub blockers: std::sync::Arc<Blockers>,
}

/// One frustum-cullable batch of static world voxels.
pub struct PropChunk {
    pub mesh: u64,
    pub center: Vec3,
    pub radius: f32,
    pub vsize: f32,
}

/// `puppet::Probe` over the real arena: the terrain height field, the
/// blockers' walkable humps and cover discs, and the river.
struct WorldProbe<'a> {
    blockers: &'a Blockers,
}

impl Probe for WorldProbe<'_> {
    fn ground(&self, x: f32, z: f32) -> f32 {
        // The sim walks the playable floor at y = 0 and sinks the DRAWN cat
        // by the water depth, so the river bed is not "ground" here.
        terrain_height(x, z).max(0.0)
    }
    fn step_top(&self, x: f32, z: f32) -> Option<f32> {
        self.blockers.step_top(x, z)
    }
    fn cover(&self, x: f32, z: f32) -> f32 {
        self.blockers.concealment_at(x, z)
    }
    fn water(&self, x: f32, z: f32) -> f32 {
        consts::water_depth(x, z)
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

        Scene {
            god_parts,
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
            total_voxels: total,
            blockers: std::sync::Arc::new(blockers),
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
        let any_char = !game.disasters.charred.is_empty();
        for ch in &self.chunks {
            if sphere_in_frustum(&vp, ch.center, ch.radius) {
                // burnt jungle: the flora in a charred chunk goes black-brown
                let tint = if any_char {
                    let c = game.disasters.char_near(ch.center, ch.radius.max(8.0));
                    [chunk_tint[0] * (1.0 - c * 0.8), chunk_tint[1] * (1.0 - c * 0.85), chunk_tint[2] * (1.0 - c * 0.88), 1.0]
                } else {
                    chunk_tint
                };
                r.queue_draw(ch.mesh, Mat4::IDENTITY, tint, 0.0, ch.vsize, false);
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
            };
            let breathe = (b.anim * 0.7).sin() * 0.03;
            let lunge = b.atk_anim * 0.8;
            let hover = if b.kind == crate::bosses::BossKind::Drake { 0.4 + (b.anim * 0.5).sin() * 0.25 } else { 0.0 };
            let fwd = Vec3::new(b.yaw.cos(), 0.0, b.yaw.sin());
            let m = Mat4::from_translation(b.pos + fwd * lunge + Vec3::Y * (breathe + hover))
                * Mat4::from_axis_angle(Vec3::Y, -b.yaw + std::f32::consts::FRAC_PI_2)
                * Mat4::from_scale(Vec3::splat(1.0 + breathe));
            r.queue_draw(mesh, m, [1.0; 4], if b.kind == crate::bosses::BossKind::Drake { 0.25 } else { 0.0 }, vs, false);
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

        // gods — posed per part. `puppet` decides what the body is doing,
        // `creature-rig` poses the boxes, `rigged` turns boxes into matrices.
        let world = WorldProbe { blockers: &game.blockers };
        let dt = (vfx.time - vfx.puppets.last_time).clamp(0.0, 0.1);
        vfx.puppets.last_time = vfx.time;
        vfx.puppets.ensure(game.gods.len());
        let mut pevents = std::mem::take(&mut vfx.puppets.events);
        pevents.clear();
        let mut posed = Vec::new();
        for i in 0..game.gods.len() {
            let g = &game.gods[i];
            if g.respawn_t > 0.0 {
                continue;
            }
            let ti = g.team as usize;
            let def = game.def(i);
            let di = g.def_id as usize;
            let rig = &self.god_rig[di];
            let leap = g.leap.as_ref().map(|l| l.view()).or_else(|| g.dash.as_ref().map(|d| {
                // The sim's dash arc is y = 1.4 a (1 - a); its slope over
                // ground covered is what the body pitches along.
                let a = (d.t / d.dur.max(1e-3)).clamp(0.0, 1.0);
                let run = Vec3::new(d.to.x - d.from.x, 0.0, d.to.z - d.from.z).length().max(0.5);
                LeapView::Flight { t: a, climb: 1.4 * (1.0 - 2.0 * a) / run * 2.0 }
            }));
            let inp = PuppetInput {
                pos: g.pos,
                yaw: g.yaw,
                hp01: (g.hp / g.max_hp.max(1.0)).clamp(0.0, 1.0),
                atk_pulse: g.atk_anim,
                strike_style: if def.auto_range > 6.0 { StrikeStyle::Snap } else { StrikeStyle::Swipe },
                leap,
                perched: g.perch.is_some(),
            };
            let puppet = &mut vfx.puppets.puppets[i];
            puppet.advance(dt, &inp, rig, &world, &mut pevents);
            let pose = puppet.pose(&inp, rig);
            creature_rig::pose(rig.parts(), rig.torso_half, &pose.motion, None, &mut posed);
            let mats = slot_transforms(rig, &posed, &pose.legs, &pose.body);

            // wading: sink toward the bed (capped so the head stays clear)
            let sink = consts::water_depth(g.pos.x, g.pos.z).min(0.9);
            // Model faces +Z; glam's rotation_y(t) sends +Z to (sin t, 0,
            // cos t), so facing (cos yaw, 0, sin yaw) needs t = pi/2 - yaw.
            // This read `yaw - pi/2`, which mirrors the facing in X: right
            // on the north-south lanes, backwards on any east-west run.
            let base = Mat4::from_translation(g.pos + Vec3::Y * (pose.lift - sink))
                * Mat4::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2 - g.yaw);
            for (s, m) in mats.iter().enumerate() {
                r.queue_draw(self.god_parts[di][ti][s], base * *m, [1.0; 4], 0.0, models::GOD_DRAW_VOXEL, false);
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
        vfx.puppet_events(&pevents);
        vfx.puppets.events = pevents;

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
    /// wind sway weight baked into each voxel's misc.y (0 = rigid)
    sway: f32,
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
        if consts::lane_clearance(x, z) > min_clear && consts::river_dist(x, z) > 9.5 && !crate::bosses::in_pit(x, z) {
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

fn build_world() -> (Vec<(VoxelMesh, Vec3, f32, f32)>, VoxelMesh, Blockers) {
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
            blockers.push_prop(PropKind::$kind, $x, $z, $y, 1.0);
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

    // --- playable jungle flora ---
    for _ in 0..56 {
        if let Some((x, z)) = jungle_spot(&mut rng, 8.5) {
            plant!(JungleTree; props::jungle_tree(0.85 + rng.f32() * 0.35, v), x, z, 0.0, (v % 4) as u8, V_MED, 1.0);
            blockers.push_jungle_roots(x, z, 0.0, v);
        }
        v += 1;
    }
    for _ in 0..18 {
        if let Some((x, z)) = jungle_spot(&mut rng, 7.0) {
            plant!(Palm; props::palm(0.9 + rng.f32() * 0.3, v), x, z, 0.0, (v % 4) as u8, V_MED, 1.6);
        }
        v += 1;
    }
    for _ in 0..12 {
        if let Some((x, z)) = jungle_spot(&mut rng, 9.0) {
            plant!(StranglerFig; props::strangler_fig(0.9 + rng.f32() * 0.25, v), x, z, 0.0, (v % 4) as u8, V_MED, 0.8);
        }
        v += 1;
    }
    for _ in 0..22 {
        if let Some((x, z)) = jungle_spot(&mut rng, 6.0) {
            plant!(Bamboo; props::bamboo(1.0, v), x, z, 0.0, (v % 4) as u8, V_MED, 2.2);
        }
        v += 1;
    }
    for _ in 0..120 {
        if let Some((x, z)) = jungle_spot(&mut rng, 5.2) {
            plant!(Fern; props::fern(v), x, z, 0.0, 0, V_SM, 2.6);
        }
        v += 1;
    }
    for _ in 0..100 {
        if let Some((x, z)) = jungle_spot(&mut rng, 5.6) {
            plant!(Bush; props::bush(v), x, z, 0.0, 0, V_MED, 1.8);
        }
        v += 1;
    }
    for _ in 0..46 {
        if let Some((x, z)) = jungle_spot(&mut rng, 7.0) {
            put!(Rock; props::rock(0.7 + rng.f32() * 0.6), x, z, 0.0, 0, V_MED);
        }
        v += 1;
    }
    // boulder clusters: in the jungle, and worn ones along the river banks
    for _ in 0..18 {
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
    for _ in 0..34 {
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
    for _ in 0..11 {
        if let Some((x, z)) = jungle_spot(&mut rng, 7.5) {
            put!(FallenLog; props::fallen_log(0.85 + rng.f32() * 0.3), x, z, 0.0, (v % 4) as u8, V_MED);
        }
        v += 1;
    }
    for _ in 0..14 {
        if let Some((x, z)) = jungle_spot(&mut rng, 8.0) {
            plant!(Pine; props::pine(0.9 + rng.f32() * 0.5), x, z, 0.0, 0, V_MED, 0.9);
        }
        v += 1;
    }
    for _ in 0..14 {
        if let Some((x, z)) = jungle_spot(&mut rng, 8.0) {
            plant!(Oak; props::oak(0.9 + rng.f32() * 0.4), x, z, 0.0, 0, V_MED, 1.0);
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

    // --- bridges where the side lanes cross the rivers ---
    for (side, lane) in [(-1.0f32, Lane::Top), (1.0f32, Lane::Bot)] {
        if let Some(p) = bridge_crossing(side, lane) {
            put!(Bridge; props::bridge(32), p.x, p.z, -0.35, 0, V_MED);
        }
    }

    // --- braziers flanking the base gates ---
    for team in [Team::Blue, Team::Red] {
        for sx in [-7.0f32, 7.0] {
            let z = team.base_pos().z + team.push_z() * 16.0;
            put!(Brazier; props::brazier(), sx, z, 0.0, 0, V_BRAZ);
        }
    }

    // --- beyond the bounds: colossal landmarks ---
    let zg = 196.0f32;
    let zign = Vec3::new(0.0, terrain_height(0.0, zg) - 3.0, zg);
    let zigs = Vec3::new(0.0, terrain_height(0.0, -zg) - 3.0, -zg);
    put!(Ziggurat; props::ziggurat(), zign.x, zign.z, zign.y, 2, V_BIG); // stair faces the valley
    put!(Ziggurat; props::ziggurat(), zigs.x, zigs.z, zigs.y, 0, V_BIG);
    let mut landmarks = vec![zign, zigs];

    let h1 = Vec3::new(-164.0, terrain_height(-164.0, 66.0) - 2.0, 66.0);
    let h2 = Vec3::new(164.0, terrain_height(164.0, -66.0) - 2.0, -66.0);
    put!(ColossusHead; props::colossus_head(), h1.x, h1.z, h1.y, 1, V_BIG); // face +X, toward the valley
    put!(ColossusHead; props::colossus_head(), h2.x, h2.z, h2.y, 3, V_BIG); // face -X
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
        "[clawbound] world: {} props → {} chunks ({} voxels), {} blockers",
        pieces.len(),
        out.len(),
        out.iter().map(|(m, _, _, _)| m.inst.len()).sum::<usize>(),
        blockers.len()
    );
    (out, bird_mesh(), blockers)
}
