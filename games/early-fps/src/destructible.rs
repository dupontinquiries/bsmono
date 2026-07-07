//! destructible.rs — **destructible objects**. Cover that isn't permanent: barriers, crate stacks and glass
//! panes that take damage, crack, and finally blow apart — removing the cover (or opening the route) they
//! occupied. Each is a tiny state machine (Intact → Damaged → Destroyed) driven by weapon and explosion
//! damage. While it isn't destroyed it blocks like any prop (height-aware) and you can stand on it; once
//! destroyed it stops blocking and turns to rubble (or, for glass, vanishes into shards).

use crate::math::Vec3;
use crate::mesh::{boxx, dither, rgb, set_alpha, Mesh3, FACE_ALL};
use egui::Color32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DestructibleKind {
    /// Concrete barrier — tough, leaves a rubble pile.
    Barrier,
    /// Wooden crate stack — medium, splinters into planks.
    CrateStack,
    /// Glass pane — fragile, shatters to nothing (vision opens immediately).
    GlassPane,
    /// Red fuel drum — fragile, and detonates when destroyed: an area blast that damages everything nearby
    /// (including other barrels, so lines of them chain). The classic shoot-the-barrel interaction.
    ExplosiveBarrel,
    /// A **building wall segment** — a large, tough structural panel that *deforms progressively* as it takes
    /// damage: it cracks, bows, sheds facing panels and finally caves in a ragged hole, rather than snapping
    /// between three discrete prop states. The mesh is rebuilt from the live damage fraction so a wall you're
    /// shooting visibly crumbles. Bigger and much tougher than a barrier; leaves a jagged rubble stub.
    Wall,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DState {
    Intact,
    Damaged,
    Destroyed,
}

#[derive(Clone, Copy, Debug)]
pub struct Destructible {
    pub center: Vec3,
    pub half: Vec3,
    pub hp: f32,
    pub max_hp: f32,
    pub kind: DestructibleKind,
    pub state: DState,
    pub seed: u32,
    /// For `ExplosiveBarrel`: whether its blast has already been triggered. The game scans each frame for
    /// barrels that are Destroyed-but-not-detonated and fires the blast exactly once — which is also what
    /// lets barrels chain: one blast damages the next barrel, it dies, and the *next* scan detonates it.
    pub detonated: bool,
}

impl Destructible {
    pub fn new(kind: DestructibleKind, center: Vec3, half: Vec3, seed: u32) -> Self {
        let max_hp = match kind {
            DestructibleKind::Barrier => 65.0,
            DestructibleKind::CrateStack => 32.0,
            DestructibleKind::GlassPane => 8.0,
            DestructibleKind::ExplosiveBarrel => 18.0,
            DestructibleKind::Wall => 220.0, // structural — soaks a lot of fire before it caves
        };
        Destructible { center, half, hp: max_hp, max_hp, kind, state: DState::Intact, seed, detonated: false }
    }

    /// Still a solid collision object?
    pub fn blocks(&self) -> bool {
        self.state != DState::Destroyed
    }

    /// Standable top while it blocks (so you can perch on a barrier).
    pub fn top(&self) -> f32 {
        self.center.y + self.half.y
    }

    /// Apply damage; advances the state machine. Returns true if this hit *just* destroyed it (so the caller
    /// can spawn debris, open a route, etc.).
    pub fn damage(&mut self, dmg: f32) -> bool {
        if self.state == DState::Destroyed {
            return false;
        }
        self.hp -= dmg;
        let prev = self.state;
        self.state = if self.hp <= 0.0 {
            DState::Destroyed
        } else if self.hp < self.max_hp * 0.5 {
            DState::Damaged
        } else {
            DState::Intact
        };
        self.state == DState::Destroyed && prev != DState::Destroyed
    }

    /// The base albedo for this object's material.
    fn base_color(&self) -> Color32 {
        match self.kind {
            DestructibleKind::Barrier => rgb(176, 172, 164),
            DestructibleKind::CrateStack => rgb(140, 100, 56),
            DestructibleKind::GlassPane => rgb(150, 200, 220),
            DestructibleKind::ExplosiveBarrel => rgb(168, 44, 36),
            DestructibleKind::Wall => rgb(150, 146, 138),
        }
    }

    /// Does this kind detonate when destroyed?
    pub fn explosive(&self) -> bool {
        self.kind == DestructibleKind::ExplosiveBarrel
    }

    /// The colour physics debris chunks should take when this object is blown apart.
    pub fn base_tumble_color(&self) -> Color32 {
        self.base_color()
    }

    /// The current visual for the object, reflecting its state. Intact is the clean prop; Damaged darkens and
    fn wall_mesh(&self) -> Vec<Mesh3> {
        // Damage fraction 0 (pristine) .. 1 (caved in). The wall is a grid of panel cells; as damage rises,
        // cells are progressively knocked out (leaving a ragged hole), surviving cells crack and bow inward,
        // and knocked-out cells at the bottom leave a rubble stub. This gives a continuously-deforming wall
        // instead of three discrete states.
        let dmg = (1.0 - (self.hp / self.max_hp)).clamp(0.0, 1.0);
        let col = self.base_color();
        let mut out: Vec<Mesh3> = Vec::new();
        // panel grid dimensions from the wall's aspect (wider walls get more columns)
        let cols = ((self.half.x / 0.5).round() as i32).clamp(3, 8);
        let rows = ((self.half.y / 0.5).round() as i32).clamp(2, 6);
        let cw = self.half.x * 2.0 / cols as f32;
        let ch = self.half.y * 2.0 / rows as f32;
        let x0 = self.center.x - self.half.x;
        let y0 = self.center.y - self.half.y;
        let thick = self.half.z;
        // deterministic per-cell "toughness" so the same wall always crumbles the same way; the weakest cells
        // (centre, higher up) fail first — a hole punches through the middle and grows.
        for r in 0..rows {
            for c in 0..cols {
                let idx = (r * cols + c) as u32;
                let h = self.seed.wrapping_mul(2654435761).wrapping_add(idx.wrapping_mul(40503));
                let jitter = (h & 0xff) as f32 / 255.0; // 0..1 per-cell randomness
                // failure threshold: centre + upper cells fail earlier (lower threshold)
                let cx_frac = (c as f32 + 0.5) / cols as f32; // 0..1 across
                let cy_frac = (r as f32 + 0.5) / rows as f32; // 0..1 up
                let centrality = 1.0 - (cx_frac - 0.5).abs() * 2.0; // 1 at centre
                let weakness = centrality * 0.5 + cy_frac * 0.3 + 0.2 * (1.0 - jitter);
                let threshold = 1.0 - weakness; // low threshold = fails early
                let cell_c = Vec3::new(x0 + (c as f32 + 0.5) * cw, y0 + (r as f32 + 0.5) * ch, self.center.z);
                if dmg > threshold {
                    // this cell is knocked out — leave a small rubble nub near the base for the lowest row only
                    if r == 0 {
                        let mut nub = boxx(
                            Vec3::new(cell_c.x, y0 + ch * 0.18, self.center.z),
                            Vec3::new(cw * 0.42, ch * 0.18, thick * 0.9),
                            dither(scale(col, 0.5), idx as i32, 0.2),
                            FACE_ALL,
                        );
                        nub.material = Some(crate::mesh::Material::Concrete);
                        out.push(nub);
                    }
                    continue;
                }
                // surviving cell: cracks grow with local damage, and it bows inward (−z) as the wall weakens
                let local = ((dmg - threshold.max(0.0)) / 0.4).clamp(0.0, 1.0);
                let bow = -(dmg * dmg) * thick * 1.3 * centrality;
                let mut cell = boxx(
                    Vec3::new(cell_c.x, cell_c.y, self.center.z + bow),
                    Vec3::new(cw * 0.5 * (1.0 - local * 0.12), ch * 0.5 * (1.0 - local * 0.12), thick),
                    dither(scale(col, 1.0 - dmg * 0.4), idx as i32, 0.08 + local * 0.4),
                    FACE_ALL,
                );
                cell.material = Some(crate::mesh::Material::Concrete);
                out.push(cell);
            }
        }
        if out.is_empty() {
            // fully caved: a low jagged rubble line where the wall stood
            let mut pile = Mesh3::new(scale(col, 0.55));
            for k in 0..(cols.max(4)) {
                let a = k as f32 / cols as f32;
                pile.merge(&boxx(
                    Vec3::new(x0 + a * self.half.x * 2.0 + cw * 0.5, y0 + 0.14, self.center.z),
                    Vec3::new(cw * 0.5, 0.14 + (self.seed >> k & 3) as f32 * 0.05, thick),
                    dither(scale(col, 0.55), k, 0.2),
                    FACE_ALL,
                ));
            }
            pile.material = Some(crate::mesh::Material::Concrete);
            out.push(pile);
        }
        out
    }

    /// The current visual for the object, reflecting its state. Intact is the clean prop; Damaged darkens and
    /// chips it; Destroyed becomes a low rubble pile (or nothing, for glass).
    pub fn mesh(&self) -> Vec<Mesh3> {
        // A building wall deforms *continuously* from its live damage fraction rather than snapping between
        // three prop states — so route it to its own progressive builder in every state.
        if self.kind == DestructibleKind::Wall {
            return self.wall_mesh();
        }
        let col = self.base_color();
        match self.state {
            DState::Intact => vec![self.body(col, 0.0)],
            DState::Damaged => {
                // darker, chipped, and a couple of fallen chunks at the base
                let mut out = vec![self.body(scale(col, 0.78), 0.22)];
                let s = self.seed;
                for k in 0..2 {
                    let ox = ((s >> (k * 3)) & 7) as f32 / 7.0 - 0.5;
                    let oz = ((s >> (k * 3 + 4)) & 7) as f32 / 7.0 - 0.5;
                    out.push(boxx(
                        Vec3::new(self.center.x + ox * self.half.x * 1.3, self.center.y - self.half.y + 0.12, self.center.z + oz * self.half.z * 1.3),
                        Vec3::new(0.18, 0.12, 0.18),
                        scale(col, 0.6),
                        FACE_ALL,
                    ));
                }
                out
            }
            DState::Destroyed => {
                if self.kind == DestructibleKind::GlassPane {
                    return Vec::new(); // shattered away; shards are handled by particles
                }
                if self.kind == DestructibleKind::ExplosiveBarrel {
                    // blown apart: a scorched patch and a couple of torn shell fragments, not a rubble pile
                    let base = self.center.y - self.half.y;
                    let mut out = Vec::new();
                    let mut burn = crate::mesh::disc(Vec3::new(self.center.x, base + 0.03, self.center.z), self.half.x * 2.6, 12, rgb(38, 32, 28));
                    burn.texturize(0.6, 0.2, self.seed);
                    out.push(burn);
                    for k in 0..3u32 {
                        let a = k as f32 / 3.0 * std::f32::consts::TAU + self.seed as f32 * 0.1;
                        out.push(boxx(
                            Vec3::new(self.center.x + a.cos() * 0.5, base + 0.1, self.center.z + a.sin() * 0.5),
                            Vec3::new(0.14, 0.1, 0.1),
                            scale(self.base_color(), 0.45),
                            FACE_ALL,
                        ));
                    }
                    return out;
                }
                // a low rubble/debris pile where the object stood
                let base = self.center.y - self.half.y;
                let mut pile = Mesh3::new(scale(col, 0.62));
                let s = self.seed;
                for k in 0..6u32 {
                    let a = k as f32 / 6.0 * std::f32::consts::TAU;
                    let r = 0.4 + ((s >> (k * 2)) & 3) as f32 * 0.18;
                    let h = 0.1 + ((s >> (k + 1)) & 3) as f32 * 0.06;
                    pile.merge(&boxx(
                        Vec3::new(self.center.x + a.cos() * self.half.x * r, base + h, self.center.z + a.sin() * self.half.z * r),
                        Vec3::new(0.22, h, 0.22),
                        dither(scale(col, 0.62), k as i32, 0.18),
                        FACE_ALL,
                    ));
                }
                vec![pile]
            }
        }
    }

    fn body(&self, col: Color32, crack: f32) -> Mesh3 {
        if self.kind == DestructibleKind::ExplosiveBarrel {
            // a drum: red cylinder shell with a pale hazard band and a darker lid — reads as "shoot me"
            let base = self.center.y - self.half.y;
            let r = self.half.x.min(self.half.z);
            let mut m = crate::mesh::tube(
                Vec3::new(self.center.x, base, self.center.z),
                Vec3::new(self.center.x, base + self.half.y * 2.0, self.center.z),
                r,
                10,
                dither(col, self.seed as i32, 0.08 + crack),
            );
            let band = crate::mesh::tube(
                Vec3::new(self.center.x, base + self.half.y * 0.85, self.center.z),
                Vec3::new(self.center.x, base + self.half.y * 1.15, self.center.z),
                r * 1.02,
                10,
                rgb(224, 202, 130),
            );
            m.merge(&band);
            let lid = crate::mesh::disc(Vec3::new(self.center.x, base + self.half.y * 2.0 + 0.01, self.center.z), r, 10, scale(col, 0.7));
            m.merge(&lid);
            return m;
        }
        let mut m = boxx(self.center, self.half, dither(col, self.seed as i32, 0.1 + crack), FACE_ALL);
        match self.kind {
            DestructibleKind::Barrier => m.material = Some(crate::mesh::Material::Concrete),
            DestructibleKind::CrateStack => m.material = Some(crate::mesh::Material::Planks),
            DestructibleKind::GlassPane => {
                let a = if self.state == DState::Damaged { 150 } else { 110 };
                set_alpha(&mut m, a);
                m.reflect = 0.4;
            }
            DestructibleKind::ExplosiveBarrel => unreachable!("handled above"),
            DestructibleKind::Wall => unreachable!("walls use wall_mesh()"),
        }
        m
    }
}

fn scale(c: Color32, f: f32) -> Color32 {
    Color32::from_rgb((c.r() as f32 * f) as u8, (c.g() as f32 * f) as u8, (c.b() as f32 * f) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_deforms_progressively_shedding_cells_as_damage_rises() {
        // A building wall's mesh should lose panel cells continuously as it takes damage — the count of solid
        // cells at 30% damage should exceed the count at 70%, which should exceed the near-destroyed count.
        let mk = |frac: f32| {
            let mut d = Destructible::new(DestructibleKind::Wall, Vec3::new(0.0, 1.5, 0.0), Vec3::new(2.0, 1.5, 0.25), 7);
            d.hp = d.max_hp * (1.0 - frac);
            d.damage(0.0);
            d.mesh().len()
        };
        let pristine = mk(0.0);
        let light = mk(0.3);
        let heavy = mk(0.7);
        assert!(pristine >= light, "a pristine wall has all its cells ({pristine} vs {light})");
        assert!(light > heavy, "a wall should shed cells as damage rises ({light} vs {heavy})");
        assert!(heavy >= 1, "even a battered wall renders something (rubble)");
    }

    #[test]
    fn damage_walks_the_state_machine_and_opens_the_route() {
        let mut d = Destructible::new(DestructibleKind::CrateStack, Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.6, 0.5, 0.6), 1);
        assert_eq!(d.state, DState::Intact);
        assert!(d.blocks());
        assert!(!d.damage(10.0)); // 32 → 22, still intact-ish (>half)
        let _ = d.damage(8.0); // 22 → 14, below half → Damaged
        assert_eq!(d.state, DState::Damaged);
        assert!(d.blocks());
        let destroyed = d.damage(50.0); // overkill → Destroyed
        assert!(destroyed, "the killing hit reports destruction");
        assert_eq!(d.state, DState::Destroyed);
        assert!(!d.blocks(), "a destroyed object stops blocking — the route is open");
        assert!(!d.damage(10.0), "already destroyed → no further destruction event");
    }

    #[test]
    fn glass_is_fragile_and_vanishes_when_broken() {
        let mut g = Destructible::new(DestructibleKind::GlassPane, Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.05, 1.0, 1.0), 2);
        assert!(g.max_hp < 12.0, "glass should be fragile");
        assert!(g.damage(20.0));
        assert!(g.mesh().is_empty(), "shattered glass leaves no solid mesh");
    }

    #[test]
    fn intact_and_rubble_states_produce_geometry() {
        let d = Destructible::new(DestructibleKind::Barrier, Vec3::new(0.0, 0.6, 0.0), Vec3::new(1.2, 0.6, 0.4), 5);
        assert!(d.mesh().iter().map(|m| m.tri_count()).sum::<usize>() > 0, "intact barrier has geometry");
        let mut wrecked = d;
        wrecked.damage(999.0);
        assert!(wrecked.mesh().iter().map(|m| m.tri_count()).sum::<usize>() > 0, "rubble pile has geometry");
    }
}
