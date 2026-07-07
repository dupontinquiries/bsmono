//! Batch registrations — wires every remaining tick-shaped stateful module into
//! the [`crate::sim::service_registry`] in one place, so `ServiceHost::from_registry()`
//! (and through it the `unendlich-fullgame` bin) instantiates and ticks them all.
//!
//! Each adapter constructs its service with **neutral defaults** (calm senses, no
//! world, zero wind) — the same pattern as the central `dragon_thirst` registration.
//! That proves the wiring (constructed + ticked panic-free every frame); behavior
//! against real world signals stays proven by each module's own tests, and feeding
//! real signals is a per-service follow-up via `ServiceCtx`.
//!
//! Modules that need real world data to construct (generic `populate`/`from_stem`
//! seams like `world::cave_flora` / `world::chunk_flora`) are deliberately NOT
//! here — they are chunk-populate seams, not free-running services.

use linkme::distributed_slice;

use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};

use crate::sim::fixed::{Fixed, Vec3};

const FIXED_DT: Fixed = Fixed::from_frac(1, 60);

// ── fabric: a hanging strand under gravity ──────────────────────────────────
struct FabricService(crate::fabric::Strand);
impl Service for FabricService {
    fn name(&self) -> &'static str {
        "fabric"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        self.0.step(ctx.dt, [0.0, -9.8, 0.0], [0.0, 0.0, 0.0], 4);
    }
}
#[distributed_slice(SERVICES)]
static FABRIC: ServiceEntry = ServiceEntry {
    name: "fabric",
    make: || Box::new(FabricService(crate::fabric::Strand::hanging([0.0, 10.0, 0.0], 8, 0.25))),
};

// ── sim::cognition::forager ─────────────────────────────────────────────────
struct ForagerService(crate::sim::cognition::forager::Forager);
impl Service for ForagerService {
    fn name(&self) -> &'static str {
        "cognition_forager"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        self.0.tick();
    }
}
#[distributed_slice(SERVICES)]
static FORAGER: ServiceEntry = ServiceEntry {
    name: "cognition_forager",
    make: || Box::new(ForagerService(crate::sim::cognition::forager::Forager::new(40))),
};

// ── sim::dragon: idle dragon seeking its own position ───────────────────────
struct DragonService(crate::sim::dragon::Dragon);
impl Service for DragonService {
    fn name(&self) -> &'static str {
        "dragon"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        let _ = self.0.update(Vec3::ZERO, FIXED_DT);
    }
}
#[distributed_slice(SERVICES)]
static DRAGON: ServiceEntry = ServiceEntry {
    name: "dragon",
    make: || Box::new(DragonService(crate::sim::dragon::Dragon::new(Vec3::ZERO, 100))),
};

// ── sim::fauna_mind: calm creature with neutral senses ──────────────────────
struct FaunaMindService(crate::sim::fauna_mind::FaunaMind);
impl Service for FaunaMindService {
    fn name(&self) -> &'static str {
        "fauna_mind"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        let sense = crate::sim::fauna_mind::WorldSense::neutral();
        let _ = self.0.step(&sense, ctx.dt);
    }
}
#[distributed_slice(SERVICES)]
static FAUNA_MIND: ServiceEntry = ServiceEntry {
    name: "fauna_mind",
    make: || Box::new(FaunaMindService(crate::sim::fauna_mind::FaunaMind::new([0.0; 3], 10.0, 0.5))),
};

// ── sim::forage_sim: tiny grazing world ─────────────────────────────────────
struct ForageSimService(crate::sim::forage_sim::ForageWorld);
impl Service for ForageSimService {
    fn name(&self) -> &'static str {
        "forage_sim"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        self.0.step(ctx.dt, 0.01);
    }
}
#[distributed_slice(SERVICES)]
static FORAGE_SIM: ServiceEntry = ServiceEntry {
    name: "forage_sim",
    make: || {
        let grass: std::collections::HashSet<(i32, i32)> =
            [(0, 0), (1, 0), (0, 1), (2, 2)].into_iter().collect();
        let grazers = vec![crate::sim::forage_sim::Grazer::new((0.0, 0.0), 8.0, 2.0)];
        Box::new(ForageSimService(crate::sim::forage_sim::ForageWorld::new(grass, grazers)))
    },
};

// ── sim::locomotion_sim: one agent on flat ground ───────────────────────────
struct LocomotionService(crate::sim::locomotion_sim::World);
impl Service for LocomotionService {
    fn name(&self) -> &'static str {
        "locomotion_sim"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        self.0.step(ctx.dt);
    }
}
#[distributed_slice(SERVICES)]
static LOCOMOTION: ServiceEntry = ServiceEntry {
    name: "locomotion_sim",
    make: || {
        let leap = crate::sim::leap::Leap::from_genes(1.0, 1.0);
        let agent = crate::sim::locomotion_sim::Agent::new(0, [0.0; 3], 2.0, leap, 1.0);
        Box::new(LocomotionService(crate::sim::locomotion_sim::World::new(
            |_, _| Some(0),
            std::collections::HashSet::new(),
            vec![agent],
        )))
    },
};

// ── sim::mob_forage_memory: forager with calm senses, no food in view ───────
struct MobForageService(crate::sim::mob_forage_memory::MobForager);
impl Service for MobForageService {
    fn name(&self) -> &'static str {
        "mob_forage_memory"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        let sense = crate::sim::fauna_mind::WorldSense::neutral();
        let forage = crate::sim::mob_forage_memory::ForageSense::none();
        let _ = self.0.step(&sense, &forage, ctx.dt);
    }
}
#[distributed_slice(SERVICES)]
static MOB_FORAGE: ServiceEntry = ServiceEntry {
    name: "mob_forage_memory",
    make: || {
        Box::new(MobForageService(crate::sim::mob_forage_memory::MobForager::new(
            [0.0; 3],
            10.0,
            0.5,
            crate::sim::mob_forage_memory::SpeciesParams::default(),
        )))
    },
};

// ── sim::popdyn: small seeded colony ────────────────────────────────────────
struct PopdynService(crate::sim::popdyn::Colony);
impl Service for PopdynService {
    fn name(&self) -> &'static str {
        "popdyn"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        self.0.step(ctx.dt, 0.01, 0.001, 64);
    }
}
#[distributed_slice(SERVICES)]
static POPDYN: ServiceEntry = ServiceEntry {
    name: "popdyn",
    make: || Box::new(PopdynService(crate::sim::popdyn::Colony::seeded(8, 42))),
};

// ── sim::projectile: a fire bolt re-launched whenever it expires ────────────
struct ProjectileService(crate::sim::projectile::Projectile);
impl ProjectileService {
    fn fresh() -> crate::sim::projectile::Projectile {
        crate::sim::projectile::Projectile::fireball(Vec3::ZERO, Vec3::ZERO, 100, 600)
    }
}
impl Service for ProjectileService {
    fn name(&self) -> &'static str {
        "projectile"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        let gravity = Fixed::from_frac(98, 10);
        if self.0.step(gravity, Vec3::ZERO) {
            self.0 = ProjectileService::fresh();
        }
    }
}
#[distributed_slice(SERVICES)]
static PROJECTILE: ServiceEntry = ServiceEntry {
    name: "projectile",
    make: || Box::new(ProjectileService(ProjectileService::fresh())),
};

// ── sim::summon: an air companion heeling at the origin ─────────────────────
struct SummonService(crate::sim::summon::Companion);
impl SummonService {
    fn fresh() -> crate::sim::summon::Companion {
        let half = Vec3::new(Fixed::from_frac(1, 2), Fixed::from_frac(1, 2), Fixed::from_frac(1, 2));
        crate::sim::summon::Companion::summon(
            crate::sim::elemental::Element::Air,
            Vec3::ZERO,
            half,
            600,
            Fixed::from_frac(1, 10),
        )
    }
}
impl Service for SummonService {
    fn name(&self) -> &'static str {
        "summon"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        if matches!(self.0.update(Vec3::ZERO, None), crate::sim::summon::CompanionAction::Faded) {
            self.0 = SummonService::fresh();
        }
    }
}
#[distributed_slice(SERVICES)]
static SUMMON: ServiceEntry = ServiceEntry {
    name: "summon",
    make: || Box::new(SummonService(SummonService::fresh())),
};

// ── sim::swim: idle swimmer, zero throttle ──────────────────────────────────
struct SwimService(crate::sim::swim::SwimState);
impl Service for SwimService {
    fn name(&self) -> &'static str {
        "swim"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        self.0.step(0.0, ctx.dt, &crate::sim::swim::SwimParams::default());
    }
}
#[distributed_slice(SERVICES)]
static SWIM: ServiceEntry = ServiceEntry {
    name: "swim",
    make: || Box::new(SwimService(crate::sim::swim::SwimState::new([0.0; 3]))),
};

// ── storm_field: 16x16 field under a steady breeze ──────────────────────────
struct StormService(crate::storm_field::StormField);
impl Service for StormService {
    fn name(&self) -> &'static str {
        "storm_field"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        self.0.step(&crate::storm_field::StormParams::default(), [1.0, 0.0]);
    }
}
#[distributed_slice(SERVICES)]
static STORM: ServiceEntry = ServiceEntry {
    name: "storm_field",
    make: || Box::new(StormService(crate::storm_field::StormField::new(16, 16))),
};

// ── thermal: empty heat field over air ──────────────────────────────────────
struct ThermalService(crate::thermal::HeatField);
impl Service for ThermalService {
    fn name(&self) -> &'static str {
        "thermal"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        self.0.step(&crate::thermal::ThermalParams::default(), &[], |_| {
            crate::world::blocks::BlockId::Air
        });
    }
}
#[distributed_slice(SERVICES)]
static THERMAL: ServiceEntry = ServiceEntry {
    name: "thermal",
    make: || Box::new(ThermalService(crate::thermal::HeatField::new())),
};

// ── wetness: empty wetness field, moderate humidity, over air ───────────────
struct WetnessService(crate::wetness::WetnessField);
impl Service for WetnessService {
    fn name(&self) -> &'static str {
        "wetness"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        self.0.step(&crate::wetness::WetnessParams::default(), 128, &[], |_| {
            crate::world::blocks::BlockId::Air
        });
    }
}
#[distributed_slice(SERVICES)]
static WETNESS: ServiceEntry = ServiceEntry {
    name: "wetness",
    make: || Box::new(WetnessService(crate::wetness::WetnessField::new())),
};

// ── ui_feel: hold-to-commit easing, never held ──────────────────────────────
struct UiFeelService(crate::ui_feel::HoldToCommit);
impl Service for UiFeelService {
    fn name(&self) -> &'static str {
        "ui_feel"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        let _ = self.0.update(false, ctx.dt);
    }
}
#[distributed_slice(SERVICES)]
static UI_FEEL: ServiceEntry = ServiceEntry {
    name: "ui_feel",
    make: || Box::new(UiFeelService(crate::ui_feel::HoldToCommit::new(0.5))),
};

// ── world::adaptive_stream: frame timer pinned at 60Hz target ───────────────
struct AdaptiveStreamService(crate::world::adaptive_stream::FrameTimer);
impl Service for AdaptiveStreamService {
    fn name(&self) -> &'static str {
        "adaptive_stream"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        let _ = self.0.sample(16_000);
    }
}
#[distributed_slice(SERVICES)]
static ADAPTIVE_STREAM: ServiceEntry = ServiceEntry {
    name: "adaptive_stream",
    make: || Box::new(AdaptiveStreamService(crate::world::adaptive_stream::FrameTimer::new(16_000))),
};

// ── sim::fluid: 8x8x8 water box settling under gravity, no solids ────────────
// (Hand adapter — the generated one indexed out of bounds. UNVERIFIED in-sandbox;
// operator-checked.)
struct FluidService(crate::sim::fluid::FluidField, u64);
impl Service for FluidService {
    fn name(&self) -> &'static str {
        "fluid"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        self.1 = self.1.wrapping_add(1);
        // Drip a unit into the top center every ~2s so flow has work to do.
        if self.1 % 120 == 0 {
            let v = self.0.get(4, 7, 4).saturating_add(1);
            self.0.set(4, 7, 4, v);
        }
        self.0 = crate::sim::fluid::flow_step(&self.0, |_, _, _| false);
    }
}
#[distributed_slice(SERVICES)]
static FLUID: ServiceEntry = ServiceEntry {
    name: "fluid",
    make: || Box::new(FluidService(crate::sim::fluid::FluidField::new(8, 8, 8), 0)),
};

// ── moisture_field: 16x16 vapour grid, steady breeze, mild climate ──────────
// (Hand adapter — generated one violated grid preconditions. UNVERIFIED
// in-sandbox; operator-checked.)
struct MoistureService(crate::moisture_field::MoistureField);
impl Service for MoistureService {
    fn name(&self) -> &'static str {
        "moisture_field"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        self.0.advect(ctx.dt.min(0.1), true, |_, _| [1.0, 0.0]);
        self.0.step_climate(
            &crate::moisture_field::ClimateParams::default(),
            |_, _| crate::moisture_field::ClimateSample { temperature: 0.5, is_water: 0.25 },
        );
    }
}
#[distributed_slice(SERVICES)]
static MOISTURE: ServiceEntry = ServiceEntry {
    name: "moisture_field",
    make: || {
        Box::new(MoistureService(crate::moisture_field::MoistureField::new(
            16,
            16,
            1.0,
            [0.0, 0.0],
        )))
    },
};

// ── world::lumen: one pearl glowing through the day cycle ───────────────────
struct LumenService(crate::world::lumen::LumenField);
impl Service for LumenService {
    fn name(&self) -> &'static str {
        "lumen"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        self.0.tick(0.5);
    }
}
#[distributed_slice(SERVICES)]
static LUMEN: ServiceEntry = ServiceEntry {
    name: "lumen",
    make: || {
        let pearls = vec![crate::world::lumen::Pearl::new((0, 0, 0), 1.0)];
        Box::new(LumenService(crate::world::lumen::LumenField::new(pearls, &[])))
    },
};

#[cfg(test)]
mod tests {
    use crate::sim::service_registry::{ServiceCtx, ServiceHost};

    /// Every batch-registered service constructs and survives ~2s of ticking.
    #[test]
    fn batch_services_register_and_tick() {
        let mut host = ServiceHost::from_registry();
        let names = host.names();
        for expect in [
            "adaptive_stream",
            "cognition_forager",
            "dragon",
            "fabric",
            "fauna_mind",
            "forage_sim",
            "locomotion_sim",
            "lumen",
            "mob_forage_memory",
            "popdyn",
            "projectile",
            "storm_field",
            "summon",
            "swim",
            "thermal",
            "ui_feel",
            "wetness",
        ] {
            assert!(names.contains(&expect), "missing batch service {expect}; have {names:?}");
        }
        let ctx = ServiceCtx { dt: 1.0 / 60.0 };
        for _ in 0..120 {
            host.tick_all(&ctx);
        }
    }
}
