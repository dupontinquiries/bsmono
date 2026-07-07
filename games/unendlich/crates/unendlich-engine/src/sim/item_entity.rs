//! Item entities — items that live in the world as physical, thinking,
//! remembering things, not just inventory slots. The hero's blade you can send
//! flying, the spirit wolf summoned from a blade: these are world objects, and
//! they're built from three independent modules so each can be reused and
//! tested on its own:
//!
//! - **Body** ([`ItemBody`]) — where it is and how it moves and collides. The
//!   physics/collision systems act on this (and a jointed item — a chain-whip,
//!   a blade with a trailing ribbon — can hang a `creature::Anatomy` or a
//!   `fabric::Strand` off it to bend; the body is the anchor).
//! - **Brain** ([`ItemBrain`]) — what it does under the conditions it senses:
//!   held, flying out at a target, recalled and returning. The autonomy behind a
//!   sentient blade, fed perception (e.g. from `sim::vision`) by the caller.
//! - **Memory** ([`ItemMemory`]) — the persistent state special abilities draw
//!   on: its elemental charge/level/attunement plus an ability cooldown and the
//!   last target it locked. This is what lets it act *and remember* once summoned.
//!
//! Composing the three lets one item be inert in the pack, then be launched into
//! the world to hunt on its own, then come home — all deterministic fixed-point,
//! so it snapshots and rolls back with the sim.

use crate::sim::elemental::{CastProfile, ElementalItem, Element, ItemForm};
use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::item_brain::{BrainParams, ItemGenes};

// ── Body ──────────────────────────────────────────────────────────────────────

/// The physical presence of an item in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemBody {
    pub pos: Vec3,
    pub vel: Vec3,
    /// Half-extents of its collision box.
    pub half: Vec3,
}

impl ItemBody {
    pub fn new(pos: Vec3, half: Vec3) -> ItemBody {
        ItemBody { pos, vel: Vec3::ZERO, half }
    }
    /// Integrate one tick (kinematic; gravity/voxel collision is layered by the
    /// caller via the physics kernel when the item is subject to them).
    pub fn step(&mut self) {
        self.pos.x = self.pos.x + self.vel.x;
        self.pos.y = self.pos.y + self.vel.y;
        self.pos.z = self.pos.z + self.vel.z;
    }
    /// AABB overlap test against another box — the collision-detection hook
    /// (item vs target, item vs trigger volume).
    pub fn overlaps(&self, center: Vec3, half: Vec3) -> bool {
        (self.pos.x - center.x).abs() <= self.half.x + half.x
            && (self.pos.y - center.y).abs() <= self.half.y + half.y
            && (self.pos.z - center.z).abs() <= self.half.z + half.z
    }
}

// ── Brain ─────────────────────────────────────────────────────────────────────

/// What a (live) item is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemMode {
    /// In hand / stowed — inert.
    #[default]
    Held,
    /// Sent out, hunting a target on its own.
    Flying,
    /// Loitering near its owner as a watchful attendant — snaps to [`Flying`]
    /// the moment it perceives a target, returns when recalled or spent.
    Hover,
    /// Recalled or out of charge — coming back to its owner.
    Returning,
}

/// The autonomy module: mode + how fast it moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemBrain {
    pub mode: ItemMode,
    pub speed: Fixed,
}

// ── Memory ──────────────────────────────────────────────────────────────────--

/// Persistent state special abilities draw on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemMemory {
    /// Element/form/charge/level/attunement — the special-ability fuel.
    pub item: ElementalItem,
    /// The last target it locked onto (so it keeps pursuing a moment after sight
    /// is lost).
    pub last_target: Option<Vec3>,
    /// Ticks until it can strike again.
    pub cooldown: u16,
    /// The owner asked for it back.
    pub recalled: bool,
}

// ── The composed entity ─────────────────────────────────────────────────────--

/// What the item wants to do this tick (the caller resolves world effects).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemIntent {
    Idle,
    /// Pursuing toward this point.
    Pursue(Vec3),
    /// Loitering near the owner (no target in sight), watching this point.
    Hover(Vec3),
    /// Striking the target now (caller applies the element's effect).
    Strike(Vec3, CastProfile),
    /// Heading home to the owner.
    Returning(Vec3),
    /// Reached the owner — back in hand.
    Returned,
}

/// An item that lives in the world: body + brain + memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemEntity {
    pub body: ItemBody,
    pub brain: ItemBrain,
    pub memory: ItemMemory,
    /// Heritable temperament — what the forge breeds and gems tune.
    pub genes: ItemGenes,
    /// The behaviour the genome expresses (perception range, steering gains,
    /// charge economy). Derived from `genes`; the brain reads it to act.
    pub params: BrainParams,
}

/// Below this charge the item breaks off and returns to recharge.
const RETURN_CHARGE: u16 = 10;
/// Ticks between strikes.
const STRIKE_COOLDOWN: u16 = 18;

impl ItemEntity {
    /// A sentient blade (or any live item) held by its owner, with a balanced
    /// genome and an explicit motion `speed`.
    pub fn new_blade(element: Element, form: ItemForm, pos: Vec3, half: Vec3, speed: Fixed) -> ItemEntity {
        let genes = ItemGenes::balanced();
        let params = BrainParams::from_genes(&genes);
        ItemEntity {
            body: ItemBody::new(pos, half),
            brain: ItemBrain { mode: ItemMode::Held, speed },
            memory: ItemMemory { item: ElementalItem::new_form(element, form), last_target: None, cooldown: 0, recalled: false },
            genes,
            params,
        }
    }

    /// A sentient blade whose flight speed (and, in time, all of its behaviour)
    /// is expressed by its genome — what the forge hands you.
    pub fn new_blade_with_genes(element: Element, form: ItemForm, pos: Vec3, half: Vec3, genes: ItemGenes) -> ItemEntity {
        let params = BrainParams::from_genes(&genes);
        ItemEntity {
            body: ItemBody::new(pos, half),
            brain: ItemBrain { mode: ItemMode::Held, speed: params.speed },
            memory: ItemMemory { item: ElementalItem::new_form(element, form), last_target: None, cooldown: 0, recalled: false },
            genes,
            params,
        }
    }

    /// "Send it out" — launch the held item to hunt.
    pub fn launch(&mut self, target: Vec3) {
        self.brain.mode = ItemMode::Flying;
        self.memory.last_target = Some(target);
        self.memory.recalled = false;
    }

    /// Call it back.
    pub fn recall(&mut self) {
        self.memory.recalled = true;
    }

    /// Send it out to loiter near its owner as a watchful attendant. It holds
    /// station until it perceives a target, then snaps to the hunt.
    pub fn attend(&mut self) {
        self.brain.mode = ItemMode::Hover;
        self.memory.recalled = false;
    }

    /// One decision+motion tick. `perceived_target` is what its vision reports
    /// this tick (caller computes, e.g. via `sim::vision`); `owner_pos` is where
    /// home is. Steers the body, manages charge/cooldown, returns its intent.
    pub fn update(&mut self, perceived_target: Option<Vec3>, owner_pos: Vec3) -> ItemIntent {
        if self.memory.cooldown > 0 {
            self.memory.cooldown -= 1;
        }
        match self.brain.mode {
            ItemMode::Held => {
                self.body.vel = Vec3::ZERO;
                ItemIntent::Idle
            }
            ItemMode::Flying => {
                // Keep a live lock; fall back to the remembered fix.
                if perceived_target.is_some() {
                    self.memory.last_target = perceived_target;
                }
                // Break off if recalled or nearly spent.
                if self.memory.recalled || self.memory.item.charge < RETURN_CHARGE {
                    self.brain.mode = ItemMode::Returning;
                    return self.steer_home(owner_pos);
                }
                let Some(target) = self.memory.last_target else {
                    // Nothing to chase → loiter near the owner as an attendant
                    // (don't aimlessly drift — the bench's idle-circles fix).
                    self.brain.mode = ItemMode::Hover;
                    return self.hover(owner_pos);
                };
                // In range + off cooldown → strike (spends charge via the element).
                if self.body.overlaps(target, self.body.half) && self.memory.cooldown == 0 {
                    if let Some(profile) = self.memory.item.cast() {
                        self.memory.cooldown = STRIKE_COOLDOWN;
                        self.body.vel = Vec3::ZERO;
                        return ItemIntent::Strike(target, profile);
                    }
                }
                self.seek(target);
                ItemIntent::Pursue(target)
            }
            ItemMode::Hover => {
                // Watchful idle: a sighted target ends the loiter instantly.
                if let Some(t) = perceived_target {
                    self.memory.last_target = Some(t);
                    self.brain.mode = ItemMode::Flying;
                    self.seek(t);
                    return ItemIntent::Pursue(t);
                }
                if self.memory.recalled || self.memory.item.charge < RETURN_CHARGE {
                    self.brain.mode = ItemMode::Returning;
                    return self.steer_home(owner_pos);
                }
                self.hover(owner_pos)
            }
            ItemMode::Returning => self.steer_home(owner_pos),
        }
    }

    fn steer_home(&mut self, owner_pos: Vec3) -> ItemIntent {
        let reach = Fixed::from_frac(1, 2);
        if reached(self.body.pos, owner_pos, reach) {
            self.brain.mode = ItemMode::Held;
            self.body.vel = Vec3::ZERO;
            self.memory.recalled = false;
            return ItemIntent::Returned;
        }
        self.seek(owner_pos);
        ItemIntent::Returning(owner_pos)
    }

    /// Loiter at a short standoff from the owner, holding station when close.
    fn hover(&mut self, owner_pos: Vec3) -> ItemIntent {
        let standoff = Fixed::from_int(2); // blocks
        if reached(self.body.pos, owner_pos, standoff) {
            self.body.vel = Vec3::ZERO; // hold station
        } else {
            self.seek(owner_pos);
        }
        ItemIntent::Hover(owner_pos)
    }

    /// Deterministic homing: move toward `target`, clamped to `speed` per axis
    /// (slows to land exactly on it). No sqrt → bit-exact.
    fn seek(&mut self, target: Vec3) {
        let s = self.brain.speed;
        let neg = Fixed::ZERO - s;
        self.body.vel = Vec3 {
            x: (target.x - self.body.pos.x).clamp(neg, s),
            y: (target.y - self.body.pos.y).clamp(neg, s),
            z: (target.z - self.body.pos.z).clamp(neg, s),
        };
        self.body.step();
    }
}

fn reached(a: Vec3, b: Vec3, eps: Fixed) -> bool {
    (a.x - b.x).abs() <= eps && (a.y - b.y).abs() <= eps && (a.z - b.z).abs() <= eps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3 { x: Fixed::from_f64(x), y: Fixed::from_f64(y), z: Fixed::from_f64(z) }
    }
    fn blade() -> ItemEntity {
        ItemEntity::new_blade(Element::Lightning, ItemForm::Blade, v(0.0, 0.0, 0.0), v(0.3, 0.3, 0.3), Fixed::from_int(1))
    }

    #[test]
    fn held_is_inert_until_launched() {
        let mut b = blade();
        assert_eq!(b.update(None, v(0.0, 0.0, 0.0)), ItemIntent::Idle);
        assert_eq!(b.brain.mode, ItemMode::Held);
        b.launch(v(10.0, 0.0, 0.0));
        assert_eq!(b.brain.mode, ItemMode::Flying);
    }

    #[test]
    fn flying_blade_homes_in_on_its_target() {
        let mut b = blade();
        let target = v(8.0, 0.0, 0.0);
        b.launch(target);
        let owner = v(0.0, 0.0, 0.0);
        let mut last_x = b.body.pos.x;
        for _ in 0..20 {
            let intent = b.update(Some(target), owner);
            if let ItemIntent::Strike(_, _) = intent {
                break;
            }
            assert!(b.body.pos.x >= last_x, "moving toward the target");
            last_x = b.body.pos.x;
        }
        assert!(b.body.pos.x > v(6.0, 0.0, 0.0).x, "closed most of the distance");
    }

    #[test]
    fn it_strikes_in_range_then_respects_cooldown() {
        let mut b = blade();
        let target = v(0.2, 0.0, 0.0); // already within reach
        b.launch(target);
        let owner = v(0.0, 0.0, 0.0);
        let charge_before = b.memory.item.charge;
        match b.update(Some(target), owner) {
            ItemIntent::Strike(_, _) => {}
            other => panic!("expected a strike in range, got {other:?}"),
        }
        assert!(b.memory.item.charge < charge_before, "the strike spent charge");
        assert!(b.memory.cooldown > 0, "cooldown set");
        // Next immediate tick can't strike again — it's on cooldown (pursues).
        assert!(matches!(b.update(Some(target), owner), ItemIntent::Pursue(_)));
    }

    #[test]
    fn recall_brings_it_home_and_back_to_hand() {
        let mut b = blade();
        b.body.pos = v(5.0, 0.0, 0.0);
        b.launch(v(20.0, 0.0, 0.0));
        b.recall();
        let owner = v(0.0, 0.0, 0.0);
        let mut returned = false;
        for _ in 0..30 {
            match b.update(None, owner) {
                ItemIntent::Returning(_) => assert_eq!(b.brain.mode, ItemMode::Returning),
                ItemIntent::Returned => {
                    returned = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(returned, "it came home");
        assert_eq!(b.brain.mode, ItemMode::Held, "back in hand");
    }

    #[test]
    fn running_out_of_charge_forces_a_return() {
        let mut b = blade();
        b.body.pos = v(8.0, 0.0, 0.0); // out in the field, away from its owner
        b.memory.item.charge = RETURN_CHARGE - 1; // nearly spent
        b.launch(v(20.0, 0.0, 0.0));
        let intent = b.update(Some(v(20.0, 0.0, 0.0)), v(0.0, 0.0, 0.0));
        assert!(matches!(intent, ItemIntent::Returning(_)), "spent blade breaks off and returns");
        assert_eq!(b.brain.mode, ItemMode::Returning);
    }

    #[test]
    fn the_body_collides() {
        // The collision-detection hook works independently of the brain.
        let body = ItemBody::new(v(1.0, 1.0, 1.0), v(0.5, 0.5, 0.5));
        assert!(body.overlaps(v(1.4, 1.0, 1.0), v(0.5, 0.5, 0.5)), "overlapping boxes");
        assert!(!body.overlaps(v(5.0, 1.0, 1.0), v(0.5, 0.5, 0.5)), "far apart");
    }

    #[test]
    fn attend_loiters_then_pounces_on_a_sighted_target() {
        let mut b = blade();
        b.body.pos = v(10.0, 0.0, 0.0);
        b.attend();
        assert_eq!(b.brain.mode, ItemMode::Hover);
        let owner = v(0.0, 0.0, 0.0);
        // No target in sight → it drifts back toward its owner to loiter.
        let before = b.body.pos.x;
        let intent = b.update(None, owner);
        assert!(matches!(intent, ItemIntent::Hover(_)));
        assert!(b.body.pos.x < before, "drifts toward its owner while attending");
        // A target appears → it snaps to the hunt.
        let intent = b.update(Some(v(5.0, 0.0, 0.0)), owner);
        assert_eq!(b.brain.mode, ItemMode::Flying);
        assert!(matches!(intent, ItemIntent::Pursue(_)));
    }

    #[test]
    fn losing_the_target_settles_into_attendant_hover_not_a_return() {
        let mut b = blade();
        b.launch(v(5.0, 0.0, 0.0));
        b.memory.last_target = None; // never locked anything
        // Charged and not recalled, with nothing to chase → hover, not return.
        let intent = b.update(None, v(0.0, 0.0, 0.0));
        assert_eq!(b.brain.mode, ItemMode::Hover);
        assert!(matches!(intent, ItemIntent::Hover(_)));
    }

    #[test]
    fn gene_built_blade_moves_at_its_agility_speed() {
        use crate::sim::item_brain::{ItemGenes, Temper};
        let h = Fixed::from_frac(5, 10);
        let nimble = ItemGenes::new(h, h, h, h, Fixed::ONE, h, Temper::Nearest);
        let b = ItemEntity::new_blade_with_genes(
            Element::Lightning, ItemForm::Blade, v(0.0, 0.0, 0.0), v(0.3, 0.3, 0.3), nimble,
        );
        // agility = 1 → speed ≈ 0.56 blocks/tick (between 0.5 and 1.0).
        assert!(b.brain.speed > h && b.brain.speed < Fixed::from_int(1));
        assert_eq!(b.params.speed, b.brain.speed, "motion speed comes from the genome");
    }
}

// ── service registration (S3) ──
use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use linkme::distributed_slice;

// Wires the item physics body (the per-tick state inside this module); the
// sentient ItemEntity::update needs a perceived target + owner, supplied later.
struct ItemEntityService(ItemBody);
impl Service for ItemEntityService {
    fn name(&self) -> &'static str {
        "item_entity"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        self.0.step();
    }
}
#[distributed_slice(SERVICES)]
static ITEM_ENTITY_SERVICE: ServiceEntry = ServiceEntry {
    name: "item_entity",
    make: || Box::new(ItemEntityService(ItemBody::new(Vec3::ZERO, Vec3::ZERO))),
};
