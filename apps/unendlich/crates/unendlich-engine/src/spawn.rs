//! Spawning — turning a spawn egg's `CreatureKind` into an actual `Creature`
//! with a body and a breakable `Anatomy`. This is the bridge from the inventory
//! item (`ItemKind::SpawnEgg`) to a living thing in the world: pick the egg,
//! use it, and `spawn` assembles the right body plan from the parts in
//! `creature.rs`.
//!
//! Each species gets a distinct size, mass, and part set, so its capabilities
//! fall out procedurally (the dragon has wings → it flies; the Lauscher has big
//! eyes but no wings → it sees but stays grounded) and every part has its own
//! HP, so limbs can be broken off individually. Placing the returned creature
//! in the world (registering its body, hooking its brain) is engine wiring;
//! this builder is pure.

use crate::creature::{Anatomy, Creature, Part, PartState};
use crate::item::{CreatureKind, ItemKind};
use crate::physics::PhysicsBody;
use crate::sim::dragon_archetype::Genome;
use crate::world::blocks::BlockId;

/// Make a body of a given half-extent size at a world position.
fn body_of(position: [f32; 3], half_extents: [f32; 3]) -> PhysicsBody {
    let mut b = PhysicsBody::new_player(position);
    b.half_extents = half_extents;
    b
}

#[inline]
fn lerp01(t: f32, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * t.clamp(0.0, 1.0)
}

/// Deterministic per-spawn seed from a world position (FNV-1a over the bit
/// patterns), so the same spot always yields the same dragon.
fn dragon_seed(p: [f32; 3]) -> u64 {
    let mut h = 0xCBF2_9CE4_8422_2325u64;
    for &c in &p {
        h ^= c.to_bits() as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// Genome-scaled physics box for a dragon. Always comfortably larger than the
/// small creatures (the smallest `bulk` still dwarfs a Lauscher).
pub fn dragon_body_half_extents(g: &Genome) -> [f32; 3] {
    // wide, genome-driven size: small runts (~0.6×) to hulking adults (~2.4×),
    // so a bred population reads as a real range of sizes, not one stamped body.
    // r502: max raised 1.7 → 2.4 so the collision box keeps up with the larger
    // forge visuals (SIZE_SCALE_MAX bump).
    let s = lerp01(g.bulk, 0.6, 2.4);
    [2.0 * s, 1.5 * s, 4.0 * s]
}

/// Genome-scaled mass; monotone in `bulk`, always heavier than small creatures.
pub fn dragon_mass(g: &Genome) -> f32 {
    lerp01(g.bulk, 28.0, 75.0)
}

/// Build a dragon's breakable anatomy from its genome. Keeps the canonical
/// eight parts (Jaw, two Eyes, two Wings, two Feet, Tail) in the same order and
/// with the same part types/HP as the legacy body — only the offsets and sizes
/// vary with the genes — so capability, pose and meshing code is unaffected.
/// Body-local offsets: X forward, Y up, Z right.
pub fn dragon_anatomy(g: &Genome) -> Anatomy {
    let head_x = lerp01(g.neck_len, 2.2, 4.3); // head reach from the neck
    let jaw_len = lerp01(g.muzzle_len, 0.5, 1.2);
    let jaw_depth = lerp01(g.jaw_depth, 0.4, 0.9);
    let jaw_wide = lerp01(g.cheek, 0.5, 1.0);

    let eye_x = head_x - 0.2;
    let eye_up = lerp01(g.brow, 0.4, 0.9);
    let eye_lat = lerp01(g.cheek, 0.45, 0.8);

    let wing_lat = lerp01(g.wing_span, 1.9, 3.3);
    let wing_up = lerp01(g.pose_bias, 0.5, 1.1);
    let wing_span = lerp01(g.wing_span, 1.8, 3.4); // half-extent along X (span)
    let wing_chord = lerp01(g.wing_spread, 1.0, 2.0); // half-extent along Z

    let foot_x = lerp01(g.pose_bias, 0.6, 1.4);
    let foot_down = -lerp01(g.leg_len, 0.8, 1.9);
    let foot_lat = lerp01(g.bulk, 0.8, 1.3);
    let foot_h = lerp01(g.leg_len, 0.45, 0.9);

    let tail_back = -lerp01(g.tail_len, 2.2, 4.6);
    let tail_len = lerp01(g.tail_len, 1.4, 2.8); // half-extent along X

    Anatomy::new()
        .with(PartState::new(Part::Jaw, 60, [head_x, 0.0, 0.0], [jaw_len, jaw_depth, jaw_wide]))
        .with(PartState::new(Part::Eye, 10, [eye_x, eye_up, eye_lat], [0.2, 0.2, 0.2]))
        .with(PartState::new(Part::Eye, 10, [eye_x, eye_up, -eye_lat], [0.2, 0.2, 0.2]))
        .with(PartState::new(Part::Wing, 40, [0.0, wing_up, wing_lat], [wing_span, 0.2, wing_chord]))
        .with(PartState::new(Part::Wing, 40, [0.0, wing_up, -wing_lat], [wing_span, 0.2, wing_chord]))
        .with(PartState::new(Part::Foot, 30, [foot_x, foot_down, foot_lat], [0.5, foot_h, 0.5]))
        .with(PartState::new(Part::Foot, 30, [foot_x, foot_down, -foot_lat], [0.5, foot_h, 0.5]))
        .with(PartState::new(Part::Tail, 50, [tail_back, 0.0, 0.0], [tail_len, 0.5, 0.5]))
}

/// A colour multiplier from the genome's element mix. Elements (Wood, Water,
/// Air, Dust, Fire, Storm) each have a characteristic palette; the genome's
/// per-element weights blend them. `[1,1,1]` if the genome has no element.
pub fn dragon_tint(g: &Genome) -> [f32; 3] {
    // Index order matches `dragon_archetype::ELEMENT_NAMES` (Wood..Storm).
    const PALETTE: [[f32; 3]; 6] = [
        [0.50, 1.30, 0.50], // Wood  — green
        [0.50, 0.80, 1.40], // Water — blue
        [1.05, 1.10, 1.15], // Air   — pale
        [1.15, 1.00, 0.78], // Dust  — tan
        [1.50, 0.70, 0.40], // Fire  — red/orange
        [1.00, 0.60, 1.30], // Storm — violet
    ];
    let sum: f32 = g.element.iter().sum();
    if sum < 1e-3 {
        return [1.0, 1.0, 1.0];
    }
    let mut t = [0.0f32; 3];
    for (w, pal) in g.element.iter().zip(PALETTE.iter()) {
        for k in 0..3 {
            t[k] += (w / sum) * pal[k];
        }
    }
    for c in &mut t {
        *c = c.clamp(0.0, 2.0);
    }
    t
}

/// In-world voxel resolution for the forge dragon. The forge slider tops out at
/// 140; 256 is the high-fidelity value (looks near-perfect). The Z-run merge in
/// `dragon_forge` keeps the rendered box count manageable at this resolution.
pub const DRAGON_FORGE_RES: u32 = 256;

/// Spawn a dragon from an explicit genome at a chosen forge resolution.
pub fn spawn_dragon_with_genome_res(position: [f32; 3], g: &Genome, res: u32) -> Creature {
    let forge = std::sync::Arc::new(crate::sim::dragon_forge::build(g, res));
    Creature::new(body_of(position, dragon_body_half_extents(g)))
        .with_mass(dragon_mass(g))
        .with_anatomy(dragon_anatomy(g))
        .with_tint(dragon_tint(g))
        .with_forge(forge)
}

/// Spawn a dragon from an explicit genome (e.g. a `SPAWN_ARCHE`-selected or
/// bred genome from the bin), rather than the position-derived default. The
/// dragon keeps its anatomy (for capabilities + hitboxes) but renders as the
/// canonical voxel forge model at [`DRAGON_FORGE_RES`].
pub fn spawn_dragon_with_genome(position: [f32; 3], g: &Genome) -> Creature {
    spawn_dragon_with_genome_res(position, g, DRAGON_FORGE_RES)
}

/// The OLD pre-forge dragon (genome-scaled box anatomy, no forge model) — kept
/// only behind a beta gate so it is no longer in production. Renders as the
/// box silhouette. The bin should route a `beta` flag here; everything else
/// uses the forge dragon.
pub fn spawn_beta_dragon(position: [f32; 3]) -> Creature {
    let g = Genome::from_seed(dragon_seed(position));
    Creature::new(body_of(position, dragon_body_half_extents(&g)))
        .with_mass(dragon_mass(&g))
        .with_anatomy(dragon_anatomy(&g))
        .with_tint(dragon_tint(&g))
    // intentionally no `.with_forge(..)` — the beta look is the box anatomy.
}

/// The dragon produced by the `UNENDLICH_SPAWN_ARCHE=dragon` observation flag:
/// the NEW genome-driven body + element tint, varied by `seed`. This replaces
/// the old blueprint dragon in production while leaving `from_blueprint` in
/// place for the other archetypes / the design viewer.
pub fn spawn_dragon_arche(seed: u64, position: [f32; 3]) -> Creature {
    spawn_dragon_with_genome(position, &Genome::from_seed(seed))
}

/// Spawn a dragon of a specific element (`0=Wood..5=Storm`) — the basis for
/// trait-selected spawn eggs (a Wood egg yields a green vine dragon, etc.).
pub fn spawn_dragon_element(seed: u64, element_idx: u8, position: [f32; 3]) -> Creature {
    spawn_dragon_with_genome(position, &Genome::from_seed_element(seed, element_idx))
}

/// Seeded spawn for any kind: dragons route through the new genome body (so an
/// egg carrying a seed breeds a varied dragon); other kinds use the fixed plan.
pub fn spawn_seeded(kind: CreatureKind, seed: u64, position: [f32; 3]) -> Creature {
    match kind {
        CreatureKind::Dragon => spawn_dragon_arche(seed, position),
        other => spawn(other, position),
    }
}

/// Build a living creature of `kind` at `position`, with a breakable anatomy.
pub fn spawn(kind: CreatureKind, position: [f32; 3]) -> Creature {
    match kind {
        CreatureKind::Dragon => {
            // Genome-driven body. Each spawn derives a deterministic genome from
            // its position, so dragons vary in build (neck/tail/wing/leg/bulk)
            // while a mid genome reproduces the classic winged flyer. Replaces
            // the former hard-coded anatomy; same eight part types/order so
            // every capability and pose consumer keeps working.
            let g = Genome::from_seed(dragon_seed(position));
            spawn_dragon_with_genome(position, &g)
        }
        CreatureKind::Lauscher => {
            // Small, light, grounded — big ears/eyes, no wings. The listener.
            let anatomy = Anatomy::new()
                .with(PartState::new(Part::Eye, 15, [0.3, 0.2, 0.2], [0.15, 0.15, 0.15]))
                .with(PartState::new(Part::Eye, 15, [0.3, 0.2, -0.2], [0.15, 0.15, 0.15]))
                .with(PartState::new(Part::Jaw, 10, [0.35, -0.1, 0.0], [0.2, 0.15, 0.2]))
                .with(PartState::new(Part::Foot, 8, [0.1, -0.4, 0.2], [0.12, 0.2, 0.12]))
                .with(PartState::new(Part::Foot, 8, [0.1, -0.4, -0.2], [0.12, 0.2, 0.12]));
            Creature::new(body_of(position, [0.4, 0.4, 0.4]))
                .with_mass(2.0)
                .with_anatomy(anatomy)
        }
        CreatureKind::Spaeher => {
            // The winged-biped sentinel (candidate #0). A light bird-thing with
            // keen eyes AND prominent ears, two wings, two legs, a jaw, a tail —
            // every sense it needs to watch and sound the alarm. Body-local
            // offsets: X forward, Y up, Z right.
            let anatomy = Anatomy::new()
                .with(PartState::new(Part::Eye, 8, [0.5, 0.7, 0.18], [0.08, 0.08, 0.08]))
                .with(PartState::new(Part::Eye, 8, [0.5, 0.7, -0.18], [0.08, 0.08, 0.08]))
                .with(PartState::new(Part::Ear, 8, [0.35, 0.85, 0.22], [0.07, 0.12, 0.06]))
                .with(PartState::new(Part::Ear, 8, [0.35, 0.85, -0.22], [0.07, 0.12, 0.06]))
                .with(PartState::new(Part::Jaw, 10, [0.62, 0.55, 0.0], [0.12, 0.08, 0.12]))
                .with(PartState::new(Part::Wing, 20, [0.0, 0.3, 0.55], [0.3, 0.05, 0.45]))
                .with(PartState::new(Part::Wing, 20, [0.0, 0.3, -0.55], [0.3, 0.05, 0.45]))
                .with(PartState::new(Part::Foot, 12, [0.0, -0.6, 0.18], [0.1, 0.3, 0.1]))
                .with(PartState::new(Part::Foot, 12, [0.0, -0.6, -0.18], [0.1, 0.3, 0.1]))
                .with(PartState::new(Part::Tail, 15, [-0.45, 0.0, 0.0], [0.25, 0.12, 0.12]));
            Creature::new(body_of(position, [0.35, 0.45, 0.3]))
                .with_mass(1.5)
                .with_anatomy(anatomy)
        }
        CreatureKind::Fauna => {
            // Generic grounded wild animal — a medium quadruped frame with eyes,
            // a jaw, four feet, and a tail. The visual body plan is rolled from
            // the general archetypes at the design layer; this is its gameplay
            // (breakable) anatomy.
            let anatomy = Anatomy::new()
                .with(PartState::new(Part::Eye, 8, [0.5, 0.35, 0.18], [0.08, 0.08, 0.08]))
                .with(PartState::new(Part::Eye, 8, [0.5, 0.35, -0.18], [0.08, 0.08, 0.08]))
                .with(PartState::new(Part::Jaw, 12, [0.6, 0.1, 0.0], [0.16, 0.12, 0.16]))
                .with(PartState::new(Part::Foot, 10, [0.45, -0.5, 0.25], [0.12, 0.3, 0.12]))
                .with(PartState::new(Part::Foot, 10, [0.45, -0.5, -0.25], [0.12, 0.3, 0.12]))
                .with(PartState::new(Part::Foot, 10, [-0.45, -0.5, 0.25], [0.12, 0.3, 0.12]))
                .with(PartState::new(Part::Foot, 10, [-0.45, -0.5, -0.25], [0.12, 0.3, 0.12]))
                .with(PartState::new(Part::Tail, 12, [-0.7, 0.1, 0.0], [0.4, 0.12, 0.12]));
            Creature::new(body_of(position, [0.7, 0.5, 0.4]))
                .with_mass(8.0)
                .with_anatomy(anatomy)
        }
        CreatureKind::Dinosaur => {
            // Great reptilian megafauna — a large heavy frame, big jaw, four
            // pillar feet, and a long counterweight tail.
            let anatomy = Anatomy::new()
                .with(PartState::new(Part::Jaw, 50, [2.4, 0.4, 0.0], [0.7, 0.5, 0.7]))
                .with(PartState::new(Part::Eye, 10, [2.4, 1.0, 0.5], [0.18, 0.18, 0.18]))
                .with(PartState::new(Part::Eye, 10, [2.4, 1.0, -0.5], [0.18, 0.18, 0.18]))
                .with(PartState::new(Part::Foot, 28, [0.9, -1.3, 0.9], [0.45, 0.7, 0.45]))
                .with(PartState::new(Part::Foot, 28, [0.9, -1.3, -0.9], [0.45, 0.7, 0.45]))
                .with(PartState::new(Part::Foot, 28, [-0.9, -1.3, 0.9], [0.45, 0.7, 0.45]))
                .with(PartState::new(Part::Foot, 28, [-0.9, -1.3, -0.9], [0.45, 0.7, 0.45]))
                .with(PartState::new(Part::Tail, 45, [-3.2, 0.2, 0.0], [2.4, 0.5, 0.6]));
            Creature::new(body_of(position, [1.8, 1.4, 3.2]))
                .with_mass(55.0)
                .with_anatomy(anatomy)
        }
    }
}

/// Bridge a designer [`CreatureBlueprint`](crate::creature_design::CreatureBlueprint)
/// — the body plans the creature picker shows — into a living world [`Creature`].
/// The `Body` box becomes the physics torso, every other box becomes a breakable
/// anatomy part, and mass is estimated from total box volume. Box roles map to the
/// capability `Part` that fits (legs walk, wings fly, eyes see, jaws eat); purely
/// structural/cosmetic boxes (neck, head, snout, horn, ridge, segment, hair)
/// render as neutral parts. This is what lets the picker's archetypes — including
/// the super-tall builds — be spawned into the world, not just previewed in 2D.
///
/// Note: anatomy parts are coloured by their `Part`, not the blueprint's per-box
/// colour, so the world creature keeps the blueprint's *shape and height* but
/// renders in capability colours rather than the picker's exact palette.
pub fn from_blueprint(bp: &crate::creature_design::CreatureBlueprint, position: [f32; 3]) -> Creature {
    use crate::creature_design::BoxRole;
    let role_to_part = |r: BoxRole| -> Option<Part> {
        match r {
            BoxRole::Body => None, // the torso → the physics body
            BoxRole::Leg => Some(Part::Foot),
            BoxRole::Wing => Some(Part::Wing),
            BoxRole::Eye => Some(Part::Eye),
            BoxRole::Jaw => Some(Part::Jaw),
            BoxRole::Tail => Some(Part::Tail),
            // structural / cosmetic boxes: rendered for shape, no hard capability
            _ => Some(Part::Tail),
        }
    };
    let body_half = bp
        .boxes
        .iter()
        .find(|b| b.role == BoxRole::Body)
        .map(|b| b.half)
        .unwrap_or([0.5, 0.5, 0.5]);
    let mut anatomy = Anatomy::new();
    let mut volume = 0.0_f32;
    for b in &bp.boxes {
        let v = 8.0 * b.half[0].abs() * b.half[1].abs() * b.half[2].abs();
        volume += v;
        if let Some(part) = role_to_part(b.role) {
            let hp = (v * 6.0).clamp(8.0, 250.0) as u16;
            anatomy = anatomy.with(PartState::new(part, hp, b.offset, b.half));
        }
    }
    let mass = (volume * 8.0).max(0.5); // ~8 mass-units per block³
    Creature::new(body_of(position, body_half))
        .with_mass(mass)
        .with_anatomy(anatomy)
}

/// What right-clicking ("using") the held item should do in the world. This is
/// the decision the bin's place path currently lacks — it hard-codes `Stone`
/// and ignores the held item, so a held block places the wrong thing and a spawn
/// egg places a stone cube instead of a creature.
pub enum UseAction {
    /// Place `block` at `cell` (the empty cell adjacent to the face hit).
    PlaceBlock { cell: [i32; 3], block: BlockId },
    /// A creature built at its spawn point, ready for the world to register.
    SpawnCreature(Creature),
    /// Nothing placeable/usable yet (ingots, tools).
    Nothing,
}

/// Resolve a right-click with `held` aimed at `target_cell`. Pure: a held block
/// places itself, a spawn egg builds its creature at the cell centre via
/// [`spawn`], and other items do nothing. The bin matches on the result —
/// `PlaceBlock` -> `set_block`, `SpawnCreature` -> register in the entity system,
/// `Nothing` -> no-op — replacing the hard-coded `Stone`.
pub fn resolve_use(held: ItemKind, target_cell: [i32; 3]) -> UseAction {
    match held {
        ItemKind::Block(b) => UseAction::PlaceBlock { cell: target_cell, block: b },
        ItemKind::SpawnEgg(kind) => {
            let pos = [
                target_cell[0] as f32 + 0.5,
                target_cell[1] as f32 + 0.5,
                target_cell[2] as f32 + 0.5,
            ];
            UseAction::SpawnCreature(spawn(kind, pos))
        }
        ItemKind::Ingot(_) | ItemKind::Tool(_, _) | ItemKind::Legendary(_) => UseAction::Nothing,
    }
}

/// Like [`resolve_use`] but for a trait-selected spawn egg: the inventory passes
/// a `seed` (fresh per egg) and a chosen dragon `element_idx` (`0=Wood..5=Storm`,
/// e.g. Wood for a green vine dragon). Non-dragon eggs ignore the element and
/// fall back to the seeded body.
pub fn resolve_use_seeded(
    held: ItemKind,
    target_cell: [i32; 3],
    seed: u64,
    element_idx: u8,
) -> UseAction {
    match held {
        ItemKind::SpawnEgg(CreatureKind::Dragon) => {
            let pos = [
                target_cell[0] as f32 + 0.5,
                target_cell[1] as f32 + 0.5,
                target_cell[2] as f32 + 0.5,
            ];
            UseAction::SpawnCreature(spawn_dragon_element(seed, element_idx, pos))
        }
        ItemKind::SpawnEgg(kind) => {
            let pos = [
                target_cell[0] as f32 + 0.5,
                target_cell[1] as f32 + 0.5,
                target_cell[2] as f32 + 0.5,
            ];
            UseAction::SpawnCreature(spawn_seeded(kind, seed, pos))
        }
        other => resolve_use(other, target_cell),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::MetalKind;

    #[test]
    fn using_a_block_places_exactly_that_block() {
        match resolve_use(ItemKind::Block(BlockId::OakLog), [3, 4, 5]) {
            UseAction::PlaceBlock { cell, block } => {
                assert_eq!(cell, [3, 4, 5]);
                assert_eq!(block, BlockId::OakLog, "places the held block, not hard-coded Stone");
            }
            _ => panic!("a held block should place"),
        }
    }

    #[test]
    fn using_a_spawn_egg_builds_its_creature_at_the_cell_centre() {
        match resolve_use(ItemKind::SpawnEgg(CreatureKind::Dragon), [10, 64, -5]) {
            UseAction::SpawnCreature(c) => {
                assert_eq!(c.body.position, [10.5, 64.5, -4.5], "creature centred in the target cell");
                assert!(c.capabilities().fly, "a dragon egg yields a flyer");
            }
            _ => panic!("a spawn egg should spawn a creature, not place a block"),
        }
    }

    #[test]
    fn using_a_tool_or_ingot_does_nothing() {
        assert!(matches!(resolve_use(ItemKind::Ingot(MetalKind::Iron), [0, 0, 0]), UseAction::Nothing));
        assert!(matches!(
            resolve_use(ItemKind::Tool(crate::item::ToolKind::Pickaxe, MetalKind::Iron), [0, 0, 0]),
            UseAction::Nothing
        ));
    }

    #[test]
    fn dragon_is_large_winged_and_flies() {
        let d = spawn(CreatureKind::Dragon, [10.0, 64.0, -5.0]);
        assert!(d.capabilities().fly, "a winged dragon should fly");
        assert!(d.capabilities().see && d.capabilities().eat);
        let wings = d.anatomy.as_ref().unwrap().parts.iter()
            .filter(|p| p.part == Part::Wing).count();
        assert_eq!(wings, 2);
        assert_eq!(d.body.position, [10.0, 64.0, -5.0]);
    }

    #[test]
    fn lauscher_sees_but_does_not_fly() {
        let l = spawn(CreatureKind::Lauscher, [0.0, 0.0, 0.0]);
        assert!(l.capabilities().see, "big-eyed listener should see");
        assert!(!l.capabilities().fly, "the Lauscher is grounded");
        assert!(l.anatomy.as_ref().unwrap().parts.iter().all(|p| p.part != Part::Wing));
    }

    #[test]
    fn spaeher_has_every_sense_a_sentinel_needs() {
        let s = spawn(CreatureKind::Spaeher, [2.0, 3.0, 4.0]);
        let caps = s.capabilities();
        // Sees AND hears — the two senses that drive its reveal.
        assert!(caps.see, "the Späher must see");
        assert!(caps.hear, "the Späher must hear — that's the new part");
        assert!(caps.fly, "winged → flies");
        assert!(caps.walk && caps.eat);
        // Graded perception is live (eyes + ears at full health).
        let g = s.anatomy.as_ref().unwrap().graded();
        assert!(g.vision > 0.0 && g.hearing > 0.0);
        // Lighter than the Lauscher (a small flyer).
        assert!(s.mass < spawn(CreatureKind::Lauscher, [0.0; 3]).mass);
    }

    #[test]
    fn the_dragon_dwarfs_the_lauscher() {
        let d = spawn(CreatureKind::Dragon, [0.0; 3]);
        let l = spawn(CreatureKind::Lauscher, [0.0; 3]);
        assert!(d.mass > l.mass);
        assert!(d.body.half_extents[1] > l.body.half_extents[1]);
    }

    #[test]
    fn every_part_starts_at_full_health_and_is_breakable() {
        let d = spawn(CreatureKind::Dragon, [0.0; 3]);
        let anatomy = d.anatomy.as_ref().unwrap();
        assert!(!anatomy.parts.is_empty());
        // All parts start intact...
        assert!(anatomy.parts.iter().all(|p| !p.is_destroyed() && p.hp > 0));
        // ...and a wing can be torn off independently.
        let mut a2 = anatomy.clone();
        if let Some(w) = a2.parts.iter_mut().find(|p| p.part == Part::Wing) {
            w.hp = 0;
            assert!(w.is_destroyed());
        }
    }

    #[test]
    fn both_kinds_can_eat() {
        for k in CreatureKind::ALL {
            assert!(spawn(k, [0.0; 3]).capabilities().eat, "{k:?} has a jaw");
        }
    }

    fn vertex_height(c: &Creature) -> f32 {
        let (v, _) = crate::world::entity::creature_to_vertices(c);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for vert in &v {
            lo = lo.min(vert.position[1]);
            hi = hi.max(vert.position[1]);
        }
        hi - lo
    }

    #[test]
    fn a_blueprint_bridges_to_a_living_creature() {
        use crate::creature_design::BoxRole;
        let bp = crate::creature_design::generate_dragons(42, 1).pop().unwrap();
        let c = from_blueprint(&bp, [0.0, 64.0, 0.0]);
        let nonbody = bp.boxes.iter().filter(|b| b.role != BoxRole::Body).count();
        assert_eq!(c.anatomy.as_ref().unwrap().parts.len(), nonbody, "every non-body box becomes a part");
        assert!(c.mass > 0.0, "should have mass");
        let (v, _) = crate::world::entity::creature_to_vertices(&c);
        assert!(!v.is_empty(), "should mesh to geometry");
        assert!(c.capabilities().see, "a creature with eyes should see");
    }

    #[test]
    fn the_bridge_preserves_the_blueprints_height() {
        let bp = crate::creature_design::generate_dragons(7, 1).pop().unwrap();
        let c = from_blueprint(&bp, [0.0, 64.0, 0.0]);
        let h = vertex_height(&c);
        assert!((h - bp.standing_height()).abs() < 0.05, "world height {h} should match blueprint {}", bp.standing_height());
    }

    #[test]
    fn part_lod_culls_fine_detail_on_a_big_creature() {
        let c = spawn(CreatureKind::Dinosaur, [0.0; 3]);
        let (full, _) = crate::world::entity::creature_to_vertices(&c);
        let (full0, _) = crate::world::entity::creature_to_vertices_lod(&c, 0.0);
        assert_eq!(full.len(), full0.len(), "lod(0) is full detail");
        let (coarse, _) = crate::world::entity::creature_to_vertices_lod(&c, 0.3);
        assert!(coarse.len() < full.len(), "a higher LOD threshold sheds fine parts (eyes)");
        assert!(!coarse.is_empty(), "torso and feet are always kept");
    }
}

#[cfg(test)]
mod genome_body_tests {
    use super::*;

    fn low_bulk() -> Genome {
        let mut g = Genome::from_seed(1);
        g.bulk = 0.05;
        g
    }
    fn high_bulk() -> Genome {
        let mut g = Genome::from_seed(1);
        g.bulk = 0.95;
        g
    }

    #[test]
    fn genome_dragon_keeps_eight_parts_and_two_wings() {
        let d = spawn(CreatureKind::Dragon, [12.0, 64.0, -3.0]);
        let parts = &d.anatomy.as_ref().unwrap().parts;
        assert_eq!(parts.len(), 8, "canonical eight-part body preserved");
        assert_eq!(parts.iter().filter(|p| p.part == Part::Wing).count(), 2);
        assert_eq!(parts.iter().filter(|p| p.part == Part::Eye).count(), 2);
        assert_eq!(parts.iter().filter(|p| p.part == Part::Foot).count(), 2);
        assert_eq!(parts.iter().filter(|p| p.part == Part::Jaw).count(), 1);
        assert_eq!(parts.iter().filter(|p| p.part == Part::Tail).count(), 1);
    }

    #[test]
    fn genome_dragon_flies_sees_eats() {
        let caps = spawn(CreatureKind::Dragon, [0.0, 0.0, 0.0]).capabilities();
        assert!(caps.fly && caps.see && caps.eat);
    }

    #[test]
    fn bulkier_genome_is_bigger_and_heavier() {
        let small = spawn_dragon_with_genome([0.0; 3], &low_bulk());
        let big = spawn_dragon_with_genome([0.0; 3], &high_bulk());
        assert!(big.mass > small.mass);
        assert!(big.body.half_extents[1] > small.body.half_extents[1]);
    }

    #[test]
    fn different_genomes_make_different_bodies() {
        let a = spawn_dragon_with_genome([0.0; 3], &Genome::from_seed(11));
        let b = spawn_dragon_with_genome([0.0; 3], &Genome::from_seed(999));
        let wing_lat = |c: &Creature| {
            c.anatomy.as_ref().unwrap().parts.iter()
                .find(|p| p.part == Part::Wing).unwrap().offset[2]
        };
        assert!(wing_lat(&a) != wing_lat(&b) || a.mass != b.mass);
    }

    #[test]
    fn default_creature_tint_is_identity() {
        // A non-dragon creature keeps the identity tint, so its colours are
        // exactly as before this change.
        let l = spawn(CreatureKind::Lauscher, [0.0; 3]);
        assert_eq!(l.tint, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn fire_genome_tints_red() {
        let mut g = Genome::from_seed(3);
        g.element = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0]; // pure Fire
        let t = dragon_tint(&g);
        assert!(t[0] > t[2], "fire should push red above blue: {t:?}");
        let d = spawn_dragon_with_genome([0.0; 3], &g);
        assert_eq!(d.tint, t, "spawn applies the element tint");
    }

    #[test]
    fn water_genome_tints_blue() {
        let mut g = Genome::from_seed(4);
        g.element = [0.0, 1.0, 0.0, 0.0, 0.0, 0.0]; // pure Water
        let t = dragon_tint(&g);
        assert!(t[2] > t[0], "water should push blue above red: {t:?}");
    }

    #[test]
    fn elementless_genome_is_untinted() {
        let mut g = Genome::from_seed(5);
        g.element = [0.0; 6];
        assert_eq!(dragon_tint(&g), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn seeded_egg_spawns_chosen_element_dragon() {
        match resolve_use_seeded(ItemKind::SpawnEgg(CreatureKind::Dragon), [2, 64, -3], 5, 0) {
            UseAction::SpawnCreature(c) => {
                assert_eq!(c.body.position, [2.5, 64.5, -2.5]);
                assert!(c.tint[1] > c.tint[0], "Wood egg → green dragon");
                assert_eq!(c.anatomy.as_ref().unwrap().parts.len(), 8);
            }
            _ => panic!("dragon egg should spawn a creature"),
        }
    }

    #[test]
    fn seeded_block_still_places() {
        match resolve_use_seeded(ItemKind::Block(BlockId::OakLog), [1, 2, 3], 0, 0) {
            UseAction::PlaceBlock { cell, block } => {
                assert_eq!(cell, [1, 2, 3]);
                assert_eq!(block, BlockId::OakLog);
            }
            _ => panic!("a block should place"),
        }
    }

    #[test]
    fn wood_vine_dragon_reads_green() {
        // Element 0 = Wood. Its tint should be green-dominant (g > r, g > b).
        let d = spawn_dragon_element(7, 0, [0.0; 3]);
        let t = d.tint;
        assert!(t[1] > t[0] && t[1] > t[2], "vine/Wood dragon should be green: {t:?}");
    }

    #[test]
    fn fire_dragon_reads_red_not_green() {
        let d = spawn_dragon_element(7, 4, [0.0; 3]); // Fire
        assert!(d.tint[0] > d.tint[1], "fire dragon is red-dominant");
    }

    #[test]
    fn spawn_seeded_dragon_uses_new_genome_body() {
        // The new body keeps the 8 canonical parts and carries an element tint.
        let d = spawn_seeded(CreatureKind::Dragon, 99, [0.0; 3]);
        assert_eq!(d.anatomy.as_ref().unwrap().parts.len(), 8);
        assert!(d.tint != [1.0, 1.0, 1.0], "a seeded dragon is element-tinted");
        // Different seeds → different dragons (variety at overspawn).
        let a = spawn_dragon_arche(1, [0.0; 3]);
        let b = spawn_dragon_arche(2, [0.0; 3]);
        assert!(a.tint != b.tint || a.mass != b.mass);
    }

    #[test]
    fn dragon_dwarfs_lauscher_across_seeds() {
        let l = spawn(CreatureKind::Lauscher, [0.0; 3]);
        for s in 0..40u64 {
            let d = spawn_dragon_with_genome([0.0; 3], &Genome::from_seed(s.wrapping_mul(2_654_435_761)));
            assert!(d.mass > l.mass, "seed {s}: dragon should outweigh the lauscher");
            assert!(d.body.half_extents[1] > l.body.half_extents[1]);
        }
    }
}


#[cfg(test)]
mod forge_size_regression {
    use super::*;
    fn diag(cr: &Creature) -> f32 {
        let (v, _) = crate::world::entity::creature_to_vertices(cr);
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for vt in &v {
            for k in 0..3 {
                if vt.position[k] < lo[k] { lo[k] = vt.position[k] }
                if vt.position[k] > hi[k] { hi[k] = vt.position[k] }
            }
        }
        ((hi[0]-lo[0]).powi(2) + (hi[1]-lo[1]).powi(2) + (hi[2]-lo[2]).powi(2)).sqrt()
    }
    #[test]
    fn new_dragon_size_range_matches_old() {
        // Old (box) dragons measured an overall diagonal of ~9.7..17.1. The forge
        // dragons must land in the same envelope: none tiny, none gigantic, with
        // real bulk-driven variety.
        let mut mn = f32::MAX;
        let mut mx = f32::MIN;
        for i in 0..200u64 {
            let g = Genome::from_seed(i.wrapping_mul(0x9E3779B97F4A7C15));
            let d = diag(&spawn_dragon_with_genome_res([0.0, 50.0, 0.0], &g, 48));
            assert!(d.is_finite() && d > 6.5, "dragon too small (diag {d}) — size regression");
            assert!(d < 23.0, "dragon too large (diag {d}) — size regression");
            mn = mn.min(d);
            mx = mx.max(d);
        }
        assert!(mx / mn >= 2.0, "dragons should span a wide size range with bulk (min {mn}, max {mx})");
    }
}
