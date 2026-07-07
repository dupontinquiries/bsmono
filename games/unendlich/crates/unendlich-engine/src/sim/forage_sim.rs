//! Forage simulation — a pure, spatial proof that the foraging *loop* actually
//! keeps creatures alive before any of it touches the world. Grazers wander a
//! field of grass cells; each tick they remember grass they can see, and when
//! hungry they head for the nearest remembered patch and eat it. With grass
//! around they thrive; with none they starve. This pins the behaviour the bin
//! wiring (real grass blocks, real roamer positions) must reproduce.
//!
//! Pure + deterministic.

use crate::sim::feeding::{Vitality, GRASS_FOOD_VALUE};
use crate::sim::forage_memory::ForageMemory;
use std::collections::HashSet;

/// How long a remembered food site stays usable as a goal.
const MEMORY_HORIZON: u32 = 600;

/// One grazing creature on the field.
#[derive(Debug, Clone)]
pub struct Grazer {
    pub pos: (f32, f32),
    pub vit: Vitality,
    pub mem: ForageMemory,
    /// How far it can spot grass while looking around.
    pub vision: f32,
    /// Blocks per second when heading to food.
    pub speed: f32,
    /// Tally of grass eaten (for tests/telemetry).
    pub eaten: u32,
}

impl Grazer {
    pub fn new(pos: (f32, f32), vision: f32, speed: f32) -> Grazer {
        Grazer {
            pos,
            vit: Vitality::full(),
            mem: ForageMemory::new(32),
            vision,
            speed,
            eaten: 0,
        }
    }
}

/// The field: a set of present grass cells and the grazers on it.
#[derive(Debug, Clone)]
pub struct ForageWorld {
    pub grass: HashSet<(i32, i32)>,
    pub grazers: Vec<Grazer>,
    now: u32,
}

impl ForageWorld {
    pub fn new(grass: HashSet<(i32, i32)>, grazers: Vec<Grazer>) -> ForageWorld {
        ForageWorld { grass, grazers, now: 0 }
    }

    pub fn population(&self) -> usize {
        self.grazers.len()
    }
    pub fn grass_count(&self) -> usize {
        self.grass.len()
    }

    /// Advance one step of `dt` seconds with energy `decay`. Each grazer looks
    /// around (remembering visible grass), and if hungry walks to and eats the
    /// nearest remembered patch; then everyone burns energy and the starved die.
    pub fn step(&mut self, dt: f32, decay: f32) {
        self.now += 1;
        let n = self.grazers.len();
        for i in 0..n {
            let pos = self.grazers[i].pos;
            let vision = self.grazers[i].vision;
            // Look around: remember every grass cell currently in view.
            let visible: Vec<(i32, i32)> = self
                .grass
                .iter()
                .filter(|&&c| cell_dist(pos, c) <= vision)
                .copied()
                .collect();
            for c in visible {
                self.grazers[i].mem.remember(c, self.now);
            }
            // Hungry → head for the nearest remembered patch and eat it.
            if self.grazers[i].vit.is_hungry() {
                if let Some(goal) = self.grazers[i].mem.nearest(pos, self.now, MEMORY_HORIZON) {
                    let step = self.grazers[i].speed * dt;
                    let np = step_toward(pos, goal, step);
                    self.grazers[i].pos = np;
                    if cell_dist(np, goal) < 1.0 {
                        if self.grass.remove(&goal) {
                            self.grazers[i].vit.feed(GRASS_FOOD_VALUE);
                            self.grazers[i].eaten += 1;
                        }
                        // Eaten or gone: drop it from memory either way.
                        self.grazers[i].mem.forget(goal);
                    }
                }
            }
            self.grazers[i].vit.tick(dt, decay);
        }
        self.grazers.retain(|g| !g.vit.is_starving());
    }
}

fn cell_dist(from: (f32, f32), cell: (i32, i32)) -> f32 {
    let cx = cell.0 as f32 + 0.5;
    let cz = cell.1 as f32 + 0.5;
    ((from.0 - cx).powi(2) + (from.1 - cz).powi(2)).sqrt()
}

/// Move from `from` toward the center of `cell` by at most `step`.
fn step_toward(from: (f32, f32), cell: (i32, i32), step: f32) -> (f32, f32) {
    let cx = cell.0 as f32 + 0.5;
    let cz = cell.1 as f32 + 0.5;
    let (dx, dz) = (cx - from.0, cz - from.1);
    let d = (dx * dx + dz * dz).sqrt();
    if d <= step || d < 1e-6 {
        (cx, cz)
    } else {
        (from.0 + dx / d * step, from.1 + dz / d * step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(cells: &[(i32, i32)]) -> HashSet<(i32, i32)> {
        cells.iter().copied().collect()
    }

    #[test]
    fn a_hungry_grazer_walks_to_remembered_grass_and_eats_it() {
        // One grass patch 8 blocks away, in view. Grazer starts hungry.
        let mut w = ForageWorld::new(field(&[(8, 0)]), vec![Grazer::new((0.0, 0.0), 12.0, 2.0)]);
        w.grazers[0].vit = Vitality::new(0.3); // hungry
        for _ in 0..60 {
            w.step(0.5, 0.0); // no decay: isolate the navigate+eat behaviour
            if w.grazers[0].eaten > 0 {
                break;
            }
        }
        assert_eq!(w.grazers[0].eaten, 1, "found and ate the grass it had seen");
        assert_eq!(w.grass_count(), 0, "the patch is gone");
        assert!(w.grazers[0].vit.energy > 0.3, "eating restored energy");
    }

    #[test]
    fn grazers_thrive_on_a_rich_field_but_starve_on_a_bare_one() {
        // A dense grid of grass around the origin.
        let mut cells = Vec::new();
        for x in -10..=10 {
            for z in -10..=10 {
                cells.push((x, z));
            }
        }
        let grazers: Vec<Grazer> = (0..6).map(|i| Grazer::new((i as f32, 0.0), 14.0, 3.0)).collect();
        let mut fed = ForageWorld::new(field(&cells), grazers.clone());
        for _ in 0..200 {
            fed.step(0.5, 0.05);
        }
        assert!(fed.population() >= 5, "a rich field keeps grazers alive (survived {})", fed.population());
        assert!(fed.grass_count() < cells.len(), "and they actually grazed it down");

        // Same grazers, no grass anywhere → they starve out.
        let mut bare = ForageWorld::new(HashSet::new(), grazers);
        for _ in 0..400 {
            bare.step(0.5, 0.05);
        }
        assert_eq!(bare.population(), 0, "a bare field starves them all");
    }

    #[test]
    fn unseen_grass_is_never_used_no_omniscience() {
        // Grass exists but is far outside vision; the hungry grazer never learns
        // of it, so it can't path there and starves despite food "existing".
        let mut w = ForageWorld::new(field(&[(500, 500)]), vec![Grazer::new((0.0, 0.0), 6.0, 3.0)]);
        w.grazers[0].vit = Vitality::new(0.3);
        for _ in 0..400 {
            w.step(0.5, 0.05);
        }
        assert_eq!(w.population(), 0, "couldn't forage food it never saw");
        assert_eq!(w.grass_count(), 1, "the distant grass was never touched");
    }
}

// ── S1 (r94): register as a sim::service_registry service (co-located) ───────
// Adapter ticks a minimal NEUTRAL forage world: one grazer, no grass, no decay.
// Real grass field + grazer population are a later wiring pass.
struct ForageService(ForageWorld);
impl crate::sim::service_registry::Service for ForageService {
    fn name(&self) -> &'static str { "forage_sim" }
    fn tick(&mut self, ctx: &crate::sim::service_registry::ServiceCtx) {
        self.0.step(ctx.dt, 0.0);
    }
}
fn make_neutral_forage_world() -> ForageWorld {
    ForageWorld::new(std::collections::HashSet::new(), vec![Grazer::new((0.0, 0.0), 5.0, 1.0)])
}
#[linkme::distributed_slice(crate::sim::service_registry::SERVICES)]
static FORAGE_SERVICE: crate::sim::service_registry::ServiceEntry =
    crate::sim::service_registry::ServiceEntry {
        name: "forage_sim",
        make: || Box::new(ForageService(make_neutral_forage_world())),
    };

#[cfg(test)]
mod s1_wiring_tests {
    use crate::sim::service_registry::{Service, ServiceCtx};
    #[test]
    fn forage_service_ticks_panic_free() {
        let mut s = super::ForageService(super::make_neutral_forage_world());
        let ctx = ServiceCtx { dt: 1.0 / 60.0 };
        for _ in 0..240 { s.tick(&ctx); }
        assert_eq!(s.name(), "forage_sim");
    }
}
