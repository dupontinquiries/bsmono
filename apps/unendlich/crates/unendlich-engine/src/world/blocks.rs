//! Block type definitions and properties.
//!
//! Block IDs are stable u8 values. Adding new ones is fine; renumbering
//! existing ones breaks save compatibility (and every chunk on disk).
//! IDs 36-55 are reserved for the "natural elements" pass — rocks and
//! minerals beyond the basic stone/dirt set, used by JSON-driven biome
//! surface decoration.

use std::convert::TryFrom;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockId {
    Air = 0,
    Stone = 1,
    Grass = 2,
    Dirt = 3,
    Cobblestone = 4,
    Bedrock = 7,
    Water = 9,
    Lava = 11,
    Sand = 12,
    Gravel = 13,
    GoldOre = 14,
    IronOre = 15,
    CoalOre = 16,
    OakLog = 17,
    OakLeaves = 18,
    BirchLog = 19,
    BirchLeaves = 20,
    Cactus = 21,
    Clay = 22,
    Moss = 23,
    RedSandstone = 24,
    Flower = 25,
    TallGrass = 26,
    Fern = 27,
    DeadBush = 28,
    Vine = 29,
    Dripstone = 30,
    Ice = 31,
    Snow = 32,
    Mushroom = 33,
    Cobweb = 34,
    Lichen = 35,

    // Natural-element pass: rocks, minerals, biological materials. Picked to
    // span the palette range needed by biome JSON files (white pale rock,
    // dark volcanic, polished veined, organic bone, sparkling crystal, etc.).
    Limestone = 36,    // pale white-grey
    Marble = 37,       // polished white with veining
    Basalt = 38,       // dark volcanic
    Tuff = 39,         // pale volcanic, lighter than basalt
    Granite = 40,      // pinkish speckled
    Slate = 41,        // dark blue-grey
    Calcite = 42,      // chalky white-blue
    Andesite = 43,     // medium grey
    Schist = 44,       // dark mica with sparkle
    Bone = 45,         // organic off-white
    Amber = 46,        // golden translucent
    Crystal = 47,      // pale violet
    Sandstone = 48,    // pale buff sandstone
    Obsidian = 49,     // black volcanic glass, very hard
    CopperOre = 50,    // orange-flecked ore
    Quartz = 51,       // white crystalline

    DiamondOre = 56,
    // Crafted / building / decorative blocks (placed, not naturally generated).
    OakPlanks = 57,
    StoneBricks = 58,
    Bricks = 59,        // fired clay brick
    Glass = 61,         // transparent
    IronBlock = 62,     // solid metal
    GoldBlock = 63,
    CoalBlock = 64,
    Bookshelf = 65,     // wood + decorative
    Pumpkin = 66,
    SnowBlock = 80,
    // r456: player-placeable building/decorative blocks (not worldgen-generated, so no shader
    // change needed — the chunk shader derives detail from each block's albedo). Solid + opaque;
    // the default predicate arms (is_solid via non-solid list, is_transparent/motion `_ =>`) handle
    // them. Each needs a mesher colour arm (before the catch-all) + a creative_tabs tab.
    MossyCobblestone = 81,
    PolishedStone = 82,
    SmoothSandstone = 83,
    Terracotta = 84,
    ChiseledStoneBricks = 85,
    DarkOakPlanks = 86,
    PolishedBasalt = 87,
}

impl BlockId {
    /// Every block variant, in declaration order. Lets tests and tooling
    /// quantify invariants over the whole palette (e.g. "every gate-skippable
    /// block is non-solid"). Kept exhaustive by `all_const_is_exhaustive`.
    pub const ALL: [BlockId; 66] = [
        BlockId::Air, BlockId::Stone, BlockId::Grass, BlockId::Dirt,
        BlockId::Cobblestone, BlockId::Bedrock, BlockId::Water, BlockId::Lava,
        BlockId::Sand, BlockId::Gravel, BlockId::GoldOre, BlockId::IronOre,
        BlockId::CoalOre, BlockId::OakLog, BlockId::OakLeaves, BlockId::BirchLog,
        BlockId::BirchLeaves, BlockId::Cactus, BlockId::Clay, BlockId::Moss,
        BlockId::RedSandstone, BlockId::Flower, BlockId::TallGrass, BlockId::Fern,
        BlockId::DeadBush, BlockId::Vine, BlockId::Dripstone, BlockId::Ice,
        BlockId::Snow, BlockId::Mushroom, BlockId::Cobweb, BlockId::Lichen,
        BlockId::Limestone, BlockId::Marble, BlockId::Basalt, BlockId::Tuff,
        BlockId::Granite, BlockId::Slate, BlockId::Calcite, BlockId::Andesite,
        BlockId::Schist, BlockId::Bone, BlockId::Amber, BlockId::Crystal,
        BlockId::Sandstone, BlockId::Obsidian, BlockId::CopperOre, BlockId::Quartz,
        BlockId::DiamondOre, BlockId::OakPlanks, BlockId::StoneBricks, BlockId::Bricks,
        BlockId::Glass, BlockId::IronBlock, BlockId::GoldBlock, BlockId::CoalBlock,
        BlockId::Bookshelf, BlockId::Pumpkin, BlockId::SnowBlock,
        BlockId::MossyCobblestone, BlockId::PolishedStone, BlockId::SmoothSandstone,
        BlockId::Terracotta, BlockId::ChiseledStoneBricks, BlockId::DarkOakPlanks,
        BlockId::PolishedBasalt,
    ];

    pub fn is_air(self) -> bool {
        self == BlockId::Air
    }

    /// Coarse "solid-like" hint. NOT load-bearing — consumers should
    /// prefer the narrow predicates below for their actual question.
    ///
    /// History: this used to be the universal "is this a thing in space?"
    /// predicate, recruited by collision, meshing, raycast filters, ore
    /// placement, fluid logic, etc. The right answer differs per consumer
    /// (water doesn't block movement but does block face emission against
    /// air; glass blocks movement but is meshed transparent; cobwebs slow
    /// you but don't stop you), so a single predicate trying to satisfy
    /// all consumers ends up wrong somewhere.
    ///
    /// Today `is_solid` survives as a *coarse intuition* — "feels like
    /// rock or wood, not air or fluid or grass." Narrow consumers should
    /// migrate to: `blocks_collision()`, `blocks_light()` (TODO),
    /// `produces_face_against(neighbor)` (TODO), `is_meshable_volume()`
    /// (TODO), etc. Old call sites still using `is_solid` are technical
    /// debt — flag with `// TODO migrate to narrower predicate` when
    /// touching them.
    pub fn is_solid(self) -> bool {
        !matches!(self, BlockId::Air | BlockId::Water | BlockId::Lava
            | BlockId::Flower | BlockId::TallGrass | BlockId::Fern
            | BlockId::DeadBush | BlockId::Vine | BlockId::Cobweb
            | BlockId::Mushroom | BlockId::Lichen)
    }

    /// Does this block stop a body's translational motion (collision)?
    /// True for stone/dirt/etc.; false for fluids, plants, cobwebs (which
    /// instead slow motion via `motion_factor()`), and air.
    ///
    /// This is the predicate physics should use, NOT `is_solid()`. Today
    /// they happen to agree, but they will diverge: glass should block
    /// collision but be transparent for face-emission; an inflatable
    /// barrier might not be `is_solid()` in the rocky-feel sense but
    /// still block collision.
    pub fn blocks_collision(self) -> bool {
        // Sub-voxel-only blocks (leaves) don't block collision — the
        // visible clumps are sparse decoration over otherwise-empty
        // space, so the player should pass through them like grass.
        // Without this, jumping into a leaf canopy hits an invisible
        // wall (cube AABB blocks but cube faces aren't rendered),
        // which feels broken.
        if self.is_subvoxel_only() { return false; }
        self.is_solid()
    }

    /// Should the renderer emit an opaque face here when meshing the
    /// boundary toward `neighbor`? Captures the "internal block faces
    /// don't show through, but air/transparent neighbors do" rule.
    ///
    /// This is the predicate the mesher should use, NOT `is_solid()`.
    /// Differs from `blocks_collision()` for things like glass (blocks
    /// collision, but produces transparent face — needs a separate path).
    pub fn produces_face_against(self, neighbor: BlockId) -> bool {
        if !self.is_solid() { return false; }   // air/fluid/plants emit nothing
        // Sub-voxel-only blocks (leaves) draw their visual entirely
        // through the decoration pass — no cube faces. Otherwise the
        // outer green cube would hide the interesting sub-voxel clumps
        // inside.
        if self.is_subvoxel_only() { return false; }
        if neighbor.is_air() { return true; }
        if neighbor.is_transparent() && neighbor != self { return true; }
        false
    }

    /// Is this block valid anchorage for a tree's root marker?
    ///
    /// A tree's root marker must touch a block that "feels like ground":
    /// natural mineral or earth materials. Wood and leaves don't qualify
    /// (a tree can't anchor to its own canopy or to another tree). Air,
    /// fluids, and plants don't qualify (no structural contact). Ore
    /// blocks DO qualify (they sit inside stone — coal-ore-floor is
    /// valid ground for a tree).
    ///
    /// Used by the lifecycle handler's tree-root discovery and
    /// stickiness passes. Conservative: when in doubt, false. False
    /// positives (anchoring to a non-ground material) are visually
    /// odd ("tree growing on a glass platform"); false negatives (a
    /// tree refuses to anchor to a real ground material) are merely
    /// "this tree never gets a root marker and skips growth/shrink."
    pub fn is_tree_anchorage(self) -> bool {
        matches!(self,
            BlockId::Stone | BlockId::Cobblestone | BlockId::Bedrock
            | BlockId::Grass | BlockId::Dirt | BlockId::Sand | BlockId::Gravel
            | BlockId::Clay | BlockId::Moss | BlockId::RedSandstone
            | BlockId::SnowBlock | BlockId::Ice
            | BlockId::Limestone | BlockId::Marble | BlockId::Basalt
            | BlockId::Tuff | BlockId::Granite | BlockId::Slate
            | BlockId::Calcite | BlockId::Andesite | BlockId::Schist
            | BlockId::Bone | BlockId::Amber | BlockId::Crystal
            | BlockId::GoldOre | BlockId::IronOre | BlockId::CoalOre
            | BlockId::DiamondOre | BlockId::Dripstone)
    }

    pub fn is_transparent(self) -> bool {
        matches!(self, BlockId::Air | BlockId::Water | BlockId::Ice
            | BlockId::OakLeaves | BlockId::BirchLeaves | BlockId::Flower
            | BlockId::TallGrass | BlockId::Fern | BlockId::DeadBush
            | BlockId::Vine | BlockId::Cobweb | BlockId::Mushroom
            | BlockId::Lichen | BlockId::Cactus
            // Crystal and amber are translucent natural materials.
            | BlockId::Crystal | BlockId::Amber
            | BlockId::Glass)
    }

    pub fn is_replaceable(self) -> bool {
        matches!(self, BlockId::Air | BlockId::Water | BlockId::Lava
            | BlockId::TallGrass | BlockId::Flower | BlockId::Fern
            | BlockId::Snow | BlockId::DeadBush | BlockId::Vine)
    }

    pub fn is_fluid(self) -> bool {
        matches!(self, BlockId::Water | BlockId::Lava)
    }

    /// Surface "slipperiness" in [0, 1].
    ///   0.0 = perfect grip (entity stops instantly when input ceases — soil, grass)
    ///   1.0 = frictionless (entity slides indefinitely — pure ice)
    /// Used by physics to compute ground friction. Most surfaces are 0.0 or
    /// near it. Ice is 0.7-0.9. Polished metal floors would be 0.5-0.6.
    ///
    /// One of the "API glue buzzwords" — exposed so future block additions
    /// can be made slippery without touching physics code.
    pub fn slipperiness(self) -> f32 {
        match self {
            BlockId::Ice => 0.85,
            BlockId::SnowBlock => 0.30,
            // Default: standard ground friction, no slip.
            _ => 0.0,
        }
    }

    /// Restitution / "bounciness" in [0, 1] when an entity collides with this
    /// block. 0.0 = inelastic (default — entity stops), 1.0 = perfectly
    /// elastic (entity rebounds with full velocity). Useful for trampolines,
    /// slime blocks, mushroom caps later. Currently 0.0 for everything.
    pub fn bounce(self) -> f32 {
        match self {
            BlockId::Mushroom => 0.4, // mushroom blocks have a bit of give
            _ => 0.0,
        }
    }

    /// Motion factor in [0, ∞) — multiplier applied to entity velocity while
    /// they are *inside* this cell (for non-solid blocks like water, tall
    /// grass, lava). 1.0 = no effect. <1.0 = slows them down. Useful for
    /// fluids and dense vegetation.
    pub fn motion_factor(self) -> f32 {
        match self {
            BlockId::Water => 0.45,
            BlockId::Lava => 0.20,
            BlockId::TallGrass | BlockId::Fern => 0.85,
            BlockId::Cobweb => 0.10,
            _ => 1.0,
        }
    }

    /// Whether this block is rendered. Most blocks are visible. Future
    /// invisible-barrier or air-with-collision blocks would return false here.
    /// Currently every renderable block is visible.
    pub fn is_visible(self) -> bool {
        // Air is "not visible" in the sense that nothing is rendered.
        !matches!(self, BlockId::Air)
    }

    /// Canonical flat-shading base color (sRGB, 0–255) for this block — the
    /// color shown when no texture is bound, the swatch in the creative panel,
    /// and the tint the sub-voxel decoration pass should paint plant/leaf
    /// clumps with. This is the single source of truth for per-block color;
    /// the renderer's palette should read from here rather than keep its own
    /// array (where a missing entry silently falls through to grey — which is
    /// exactly why a placed leaf can render the color of stone).
    ///
    /// The match is intentionally **exhaustive** (no `_` arm): adding a new
    /// `BlockId` won't compile until it has been given a color, so no block
    /// can ever fall through to a default again.
    pub fn base_color(self) -> [u8; 3] {
        match self {
            BlockId::Air         => [0, 0, 0],       // never rendered
            BlockId::Stone       => [128, 128, 128],
            BlockId::Grass       => [122, 184, 86],
            BlockId::Dirt        => [134, 96, 67],
            BlockId::Cobblestone => [120, 120, 120],
            BlockId::Bedrock     => [40, 40, 44],
            BlockId::Water       => [64, 110, 200],
            BlockId::Lava        => [207, 86, 32],
            BlockId::Sand        => [219, 209, 160],
            BlockId::Gravel      => [150, 143, 134],
            BlockId::GoldOre     => [201, 173, 90],
            BlockId::IronOre     => [197, 180, 166],
            BlockId::CoalOre     => [82, 82, 86],
            BlockId::OakLog      => [150, 110, 70],
            BlockId::OakLeaves   => [86, 150, 70],   // GREEN — not stone-grey
            BlockId::BirchLog    => [200, 195, 180],
            BlockId::BirchLeaves => [110, 165, 84],
            BlockId::Cactus      => [90, 140, 80],
            BlockId::Clay        => [170, 172, 182],
            BlockId::Moss        => [92, 120, 60],
            BlockId::RedSandstone => [180, 100, 70],
            BlockId::Flower      => [222, 120, 150],
            BlockId::TallGrass   => [110, 170, 80],
            BlockId::Fern        => [90, 150, 80],
            BlockId::DeadBush    => [150, 120, 70],
            BlockId::Vine        => [92, 150, 80],
            BlockId::Dripstone   => [150, 120, 110],
            BlockId::Ice         => [170, 200, 230],
            BlockId::Snow        => [240, 245, 250],
            BlockId::Mushroom    => [200, 80, 70],
            BlockId::Cobweb      => [220, 220, 225],
            BlockId::Lichen      => [140, 170, 150],
            BlockId::Limestone   => [200, 200, 190],
            BlockId::Marble      => [225, 225, 225],
            BlockId::Basalt      => [60, 60, 68],
            BlockId::Tuff        => [120, 120, 115],
            BlockId::Granite     => [170, 120, 110],
            BlockId::Slate       => [70, 80, 95],
            BlockId::Calcite     => [220, 225, 230],
            BlockId::Andesite    => [140, 140, 140],
            BlockId::Schist      => [90, 85, 100],
            BlockId::Bone        => [225, 222, 205],
            BlockId::Amber       => [220, 160, 60],  // golden, r>g>b
            BlockId::Crystal     => [210, 190, 230], // pale violet
            BlockId::Sandstone   => [215, 200, 160],
            BlockId::Obsidian    => [25, 20, 35],
            BlockId::CopperOre   => [190, 130, 90],
            BlockId::Quartz      => [235, 235, 228],
            BlockId::DiamondOre  => [130, 200, 205],
            BlockId::OakPlanks   => [170, 135, 90],
            BlockId::StoneBricks => [125, 125, 125],
            BlockId::Bricks      => [160, 80, 65],
            BlockId::Glass       => [200, 225, 235],
            BlockId::IronBlock   => [200, 200, 205],
            BlockId::GoldBlock   => [230, 200, 90],
            BlockId::CoalBlock   => [45, 45, 50],
            BlockId::Bookshelf   => [150, 110, 70],
            BlockId::Pumpkin     => [220, 130, 40],
            BlockId::SnowBlock   => [245, 248, 252],
            BlockId::MossyCobblestone    => [107, 128, 97],
            BlockId::PolishedStone       => [148, 148, 153],
            BlockId::SmoothSandstone     => [224, 204, 148],
            BlockId::Terracotta          => [158, 87, 61],
            BlockId::ChiseledStoneBricks => [122, 120, 117],
            BlockId::DarkOakPlanks       => [82, 56, 33],
            BlockId::PolishedBasalt      => [56, 54, 61],
        }
    }

    /// Is this block's flat-shading color "greenish" (foliage-like)? Convenience
    /// used by tests and tinting heuristics: green channel dominates.
    pub fn is_foliage_colored(self) -> bool {
        let [r, g, b] = self.base_color();
        g > r && g > b
    }

    /// Should this block's top face be drawn with sub-block height variation?
    ///
    /// Returns true for soft natural materials (grass, dirt, sand, gravel,
    /// snow, moss) where a perfectly flat top looks artificially blocky.
    /// Returns false for hard or engineered materials (stone, marble, ice,
    /// concrete-like) where a crisp top edge is the right look.
    ///
    /// Implications for the mesher: top faces of these blocks are NOT greedy-
    /// merged with each other (since each one has unique corner heights from
    /// world-position hash). They're emitted per-block. All other faces
    /// continue to merge normally.
    ///
    /// Implications for physics: the variation amplitude is tight enough
    /// (±1/16 block) that block-aligned collision still feels right. Future
    /// work could query the same height function for "raise the player by
    /// X when standing on this corner".
    pub fn wants_top_jitter(self) -> bool {
        matches!(self,
            BlockId::Grass | BlockId::Dirt | BlockId::Sand | BlockId::Gravel
            | BlockId::Moss | BlockId::Snow | BlockId::SnowBlock | BlockId::Clay)
    }

    /// Should grass tufts / sub-block decoration be emitted in the air space
    /// above this block? Used by the mesher's decoration pass.
    ///
    /// Right now only Grass triggers tufts. Adding Moss → "moss puffs", Sand
    /// → "tiny pebbles", etc. is a one-line change here + a render-side
    /// recipe in the decoration pass. Density/look is per-decoration-type.
    pub fn spawns_grass_tufts(self) -> bool {
        matches!(self, BlockId::Grass | BlockId::Moss)
    }

    /// Does this block host an internal sub-voxel grid? Sub-voxel host
    /// blocks have a fine-resolution (1/8 block) sparse grid of mini-cells
    /// that the renderer turns into actual voxel geometry — used for
    /// detailed plants where individual blades / fronds / petals matter.
    ///
    /// Architectural note: this is the "path C" multi-scale model. Most
    /// blocks are single-scale (1×1×1), but plant-bearing blocks recurse
    /// into a sub-grid for visuals AND eventually for interactions
    /// (raycast, fire, individual harvesting). The recursion is limited:
    /// only host blocks pay the sub-cell cost; everywhere else stays
    /// the fast block-only path.
    pub fn has_subvoxels(self) -> bool {
        matches!(self,
            BlockId::Grass        // tufts grow from the top
            | BlockId::TallGrass
            | BlockId::Fern
            | BlockId::Flower
            | BlockId::OakLeaves   // canopy detail — clusters of mini-leaves
            | BlockId::BirchLeaves)
    }

    /// **Sub-voxel-only** blocks: the sub-voxel grid IS the entire visual.
    /// No underlying cube faces should be emitted by the block-level
    /// mesher — only the decoration pass paints them.
    ///
    /// Distinct from `has_subvoxels`: grass blocks (Grass) HAVE sub-
    /// voxels (the tufts) but ALSO need their dirt/grass cube emitted
    /// because that cube IS the ground. Leaves don't have anything
    /// "underneath" the clumps — emitting a green cube produces a
    /// solid wall that hides the interesting clump detail.
    ///
    /// Today: leaves. As we add more pure-decoration blocks (cobweb,
    /// fancy vines, wisp), they go here too.
    pub fn is_subvoxel_only(self) -> bool {
        matches!(self,
            BlockId::OakLeaves
            | BlockId::BirchLeaves)
    }

    /// "Softness" of a block's edges — how much its top corners sag at
    /// terrain cliffs / steps. 0 = sharp 90° corners (stone, marble).
    /// Higher values produce a slumped, eroded look as the block's top
    /// corners pull DOWN toward adjacent air space. 1.0 sags by half a
    /// block at a fully-exposed corner.
    ///
    /// This is the visual side of "loose materials don't form clean
    /// cliffs." Sand drifts. Dirt slumps. Snow piles softly. Stone holds
    /// its edge.
    ///
    /// Mesher behavior: the existing `wants_top_jitter()` blocks get an
    /// additional sag offset per top corner based on which of the 4
    /// horizontally-adjacent blocks at this corner are air. More air
    /// neighbors at a corner = more sag. Adjacent blocks meeting at a
    /// shared corner agree on the sag because they query the same world
    /// neighbors — no T-junctions, no visual cracks.
    pub fn softness(self) -> f32 {
        match self {
            // Loose/granular: strong sag.
            BlockId::Sand        => 0.85,
            BlockId::Gravel      => 0.55,
            // Soil-like: moderate sag — dirt and grass slump at edges,
            // making cliff faces look weathered rather than cleaved.
            BlockId::Dirt        => 0.50,
            BlockId::Grass       => 0.45,
            BlockId::Clay        => 0.35,
            // Snow-like: gentle pile-up.
            BlockId::Snow        => 0.70,
            BlockId::SnowBlock   => 0.40,
            BlockId::Moss        => 0.30,
            // Hard materials: no sag. The blocky edge IS the right look.
            _ => 0.0,
        }
    }

    pub fn hardness(self) -> f32 {
        match self {
            BlockId::Air | BlockId::Water | BlockId::Lava => 0.0,
            BlockId::Bedrock => -1.0,
            BlockId::Sand | BlockId::Gravel | BlockId::Dirt
            | BlockId::Grass | BlockId::Snow | BlockId::Clay => 0.5,
            BlockId::Stone | BlockId::Cobblestone | BlockId::CoalOre
            | BlockId::IronOre | BlockId::GoldOre | BlockId::DiamondOre
            | BlockId::RedSandstone | BlockId::Limestone | BlockId::Marble
            | BlockId::Basalt | BlockId::Tuff | BlockId::Granite
            | BlockId::Slate | BlockId::Calcite | BlockId::Andesite
            | BlockId::Schist => 1.5,
            BlockId::OakLog | BlockId::BirchLog => 2.0,
            BlockId::OakLeaves | BlockId::BirchLeaves => 0.2,
            BlockId::Bone => 1.2,
            BlockId::Amber => 1.0,
            BlockId::Crystal => 2.5,
            _ => 1.0,
        }
    }

    /// Resolve a string identifier (used in JSON biome definitions) to a
    /// BlockId. Recognises both bare names ("limestone") and namespaced
    /// names ("unendlich:limestone", "minecraft:stone"). Returns None on
    /// unknown — callers handle the fallback.
    ///
    /// Case-insensitive on the local part, namespace ignored.
    pub fn from_id_str(s: &str) -> Option<Self> {
        let local = s.split(':').last().unwrap_or(s);
        let lower = local.to_ascii_lowercase();
        let id = match lower.as_str() {
            "air"            => BlockId::Air,
            "stone"          => BlockId::Stone,
            "grass" | "grass_block" => BlockId::Grass,
            "dirt"           => BlockId::Dirt,
            "cobblestone"    => BlockId::Cobblestone,
            "bedrock"        => BlockId::Bedrock,
            "water"          => BlockId::Water,
            "lava"           => BlockId::Lava,
            "sand"           => BlockId::Sand,
            "gravel"         => BlockId::Gravel,
            "gold_ore"       => BlockId::GoldOre,
            "iron_ore"       => BlockId::IronOre,
            "coal_ore"       => BlockId::CoalOre,
            "oak_log"        => BlockId::OakLog,
            "oak_leaves"     => BlockId::OakLeaves,
            "birch_log"      => BlockId::BirchLog,
            "birch_leaves"   => BlockId::BirchLeaves,
            "cactus"         => BlockId::Cactus,
            "clay"           => BlockId::Clay,
            "moss" | "moss_block" => BlockId::Moss,
            "red_sandstone"  => BlockId::RedSandstone,
            "flower"         => BlockId::Flower,
            "tall_grass"     => BlockId::TallGrass,
            "fern"           => BlockId::Fern,
            "dead_bush"      => BlockId::DeadBush,
            "vine"           => BlockId::Vine,
            "dripstone"      => BlockId::Dripstone,
            "ice"            => BlockId::Ice,
            "snow"           => BlockId::Snow,
            "mushroom"       => BlockId::Mushroom,
            "cobweb"         => BlockId::Cobweb,
            "lichen"         => BlockId::Lichen,
            "limestone"      => BlockId::Limestone,
            "marble"         => BlockId::Marble,
            "basalt"         => BlockId::Basalt,
            "tuff"           => BlockId::Tuff,
            "granite"        => BlockId::Granite,
            "slate"          => BlockId::Slate,
            "calcite"        => BlockId::Calcite,
            "andesite"       => BlockId::Andesite,
            "schist"         => BlockId::Schist,
            "bone" | "bone_block" => BlockId::Bone,
            "amber"          => BlockId::Amber,
            "crystal"        => BlockId::Crystal,
            "sandstone"      => BlockId::Sandstone,
            "obsidian"       => BlockId::Obsidian,
            "copper_ore"     => BlockId::CopperOre,
            "quartz" | "quartz_block" => BlockId::Quartz,
            "diamond_ore"    => BlockId::DiamondOre,
            "oak_planks" | "planks" => BlockId::OakPlanks,
            "stone_bricks" | "stonebrick" => BlockId::StoneBricks,
            "bricks" | "brick" => BlockId::Bricks,
            "glass"          => BlockId::Glass,
            "iron_block"     => BlockId::IronBlock,
            "gold_block"     => BlockId::GoldBlock,
            "coal_block"     => BlockId::CoalBlock,
            "bookshelf"      => BlockId::Bookshelf,
            "pumpkin"        => BlockId::Pumpkin,
            "snow_block"     => BlockId::SnowBlock,
            _ => return None,
        };
        Some(id)
    }
}

impl TryFrom<u8> for BlockId {
    type Error = u8;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(BlockId::Air),
            1 => Ok(BlockId::Stone),
            2 => Ok(BlockId::Grass),
            3 => Ok(BlockId::Dirt),
            4 => Ok(BlockId::Cobblestone),
            7 => Ok(BlockId::Bedrock),
            9 => Ok(BlockId::Water),
            11 => Ok(BlockId::Lava),
            12 => Ok(BlockId::Sand),
            13 => Ok(BlockId::Gravel),
            14 => Ok(BlockId::GoldOre),
            15 => Ok(BlockId::IronOre),
            16 => Ok(BlockId::CoalOre),
            17 => Ok(BlockId::OakLog),
            18 => Ok(BlockId::OakLeaves),
            19 => Ok(BlockId::BirchLog),
            20 => Ok(BlockId::BirchLeaves),
            21 => Ok(BlockId::Cactus),
            22 => Ok(BlockId::Clay),
            23 => Ok(BlockId::Moss),
            24 => Ok(BlockId::RedSandstone),
            25 => Ok(BlockId::Flower),
            26 => Ok(BlockId::TallGrass),
            27 => Ok(BlockId::Fern),
            28 => Ok(BlockId::DeadBush),
            29 => Ok(BlockId::Vine),
            30 => Ok(BlockId::Dripstone),
            31 => Ok(BlockId::Ice),
            32 => Ok(BlockId::Snow),
            33 => Ok(BlockId::Mushroom),
            34 => Ok(BlockId::Cobweb),
            35 => Ok(BlockId::Lichen),
            36 => Ok(BlockId::Limestone),
            37 => Ok(BlockId::Marble),
            38 => Ok(BlockId::Basalt),
            39 => Ok(BlockId::Tuff),
            40 => Ok(BlockId::Granite),
            41 => Ok(BlockId::Slate),
            42 => Ok(BlockId::Calcite),
            43 => Ok(BlockId::Andesite),
            44 => Ok(BlockId::Schist),
            45 => Ok(BlockId::Bone),
            46 => Ok(BlockId::Amber),
            47 => Ok(BlockId::Crystal),
            48 => Ok(BlockId::Sandstone),
            49 => Ok(BlockId::Obsidian),
            50 => Ok(BlockId::CopperOre),
            51 => Ok(BlockId::Quartz),
            56 => Ok(BlockId::DiamondOre),
            57 => Ok(BlockId::OakPlanks),
            58 => Ok(BlockId::StoneBricks),
            59 => Ok(BlockId::Bricks),
            61 => Ok(BlockId::Glass),
            62 => Ok(BlockId::IronBlock),
            63 => Ok(BlockId::GoldBlock),
            64 => Ok(BlockId::CoalBlock),
            65 => Ok(BlockId::Bookshelf),
            66 => Ok(BlockId::Pumpkin),
            80 => Ok(BlockId::SnowBlock),
            other => Err(other),
        }
    }
}

impl Default for BlockId {
    fn default() -> Self {
        BlockId::Air
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards `BlockId::ALL` against drift. The no-wildcard match is a compile
    /// error if a variant is added without being handled here, which is the
    /// reminder to also add it to `ALL`; the count + round-trip then confirm
    /// `ALL` actually contains every variant exactly once.
    #[test]
    fn all_const_is_exhaustive() {
        fn _forces_update(b: BlockId) {
            match b {
                BlockId::Air | BlockId::Stone | BlockId::Grass | BlockId::Dirt
                | BlockId::Cobblestone | BlockId::Bedrock | BlockId::Water | BlockId::Lava
                | BlockId::Sand | BlockId::Gravel | BlockId::GoldOre | BlockId::IronOre
                | BlockId::CoalOre | BlockId::OakLog | BlockId::OakLeaves | BlockId::BirchLog
                | BlockId::BirchLeaves | BlockId::Cactus | BlockId::Clay | BlockId::Moss
                | BlockId::RedSandstone | BlockId::Flower | BlockId::TallGrass | BlockId::Fern
                | BlockId::DeadBush | BlockId::Vine | BlockId::Dripstone | BlockId::Ice
                | BlockId::Snow | BlockId::Mushroom | BlockId::Cobweb | BlockId::Lichen
                | BlockId::Limestone | BlockId::Marble | BlockId::Basalt | BlockId::Tuff
                | BlockId::Granite | BlockId::Slate | BlockId::Calcite | BlockId::Andesite
                | BlockId::Schist | BlockId::Bone | BlockId::Amber | BlockId::Crystal
                | BlockId::Sandstone | BlockId::Obsidian | BlockId::CopperOre | BlockId::Quartz
                | BlockId::DiamondOre | BlockId::OakPlanks | BlockId::StoneBricks | BlockId::Bricks
                | BlockId::Glass | BlockId::IronBlock | BlockId::GoldBlock | BlockId::CoalBlock
                | BlockId::Bookshelf | BlockId::Pumpkin | BlockId::SnowBlock => {}
            }
        }
        for b in BlockId::ALL { _forces_update(b); }
        // No duplicates, and every variant's discriminant is distinct.
        let mut ids: Vec<u8> = BlockId::ALL.iter().map(|b| *b as u8).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), BlockId::ALL.len(), "ALL must have no duplicates");
    }

    #[test]
    fn leaves_are_green_not_stone_colored() {
        // The placed-leaf-looks-like-stone bug: leaves must read green, and
        // must NOT share stone's grey.
        assert!(BlockId::OakLeaves.is_foliage_colored());
        assert!(BlockId::BirchLeaves.is_foliage_colored());
        assert_ne!(BlockId::OakLeaves.base_color(), BlockId::Stone.base_color());
        assert_ne!(BlockId::OakLeaves.base_color(), BlockId::Cobblestone.base_color());
    }

    #[test]
    fn foliage_blocks_read_green() {
        for b in [BlockId::Grass, BlockId::OakLeaves, BlockId::BirchLeaves,
                  BlockId::Vine, BlockId::Fern, BlockId::TallGrass, BlockId::Moss] {
            assert!(b.is_foliage_colored(), "{b:?} should be green-dominant");
        }
    }

    #[test]
    fn amber_is_golden_and_air_is_zero() {
        let [r, g, b] = BlockId::Amber.base_color();
        assert!(r > g && g > b, "amber warms r>g>b");
        assert_eq!(BlockId::Air.base_color(), [0, 0, 0]);
    }

    #[test]
    fn base_color_is_deterministic() {
        assert_eq!(BlockId::OakLeaves.base_color(), BlockId::OakLeaves.base_color());
    }

    #[test]
    fn from_id_str_basic_blocks() {
        assert_eq!(BlockId::from_id_str("stone"), Some(BlockId::Stone));
        assert_eq!(BlockId::from_id_str("grass"), Some(BlockId::Grass));
        assert_eq!(BlockId::from_id_str("limestone"), Some(BlockId::Limestone));
    }

    #[test]
    fn from_id_str_handles_namespaces() {
        assert_eq!(BlockId::from_id_str("minecraft:stone"), Some(BlockId::Stone));
        assert_eq!(BlockId::from_id_str("unendlich:limestone"), Some(BlockId::Limestone));
        assert_eq!(BlockId::from_id_str("relics:limestone"), Some(BlockId::Limestone));
    }

    #[test]
    fn from_id_str_case_insensitive() {
        assert_eq!(BlockId::from_id_str("LIMESTONE"), Some(BlockId::Limestone));
        assert_eq!(BlockId::from_id_str("Marble"), Some(BlockId::Marble));
    }

    #[test]
    fn from_id_str_unknown_returns_none() {
        assert_eq!(BlockId::from_id_str("vex_milk_pool"), None);
        assert_eq!(BlockId::from_id_str("witness_monolith_shard"), None);
    }

    #[test]
    fn natural_blocks_round_trip_via_u8() {
        for id in [BlockId::Limestone, BlockId::Marble, BlockId::Basalt,
                   BlockId::Tuff, BlockId::Granite, BlockId::Slate,
                   BlockId::Calcite, BlockId::Andesite, BlockId::Schist,
                   BlockId::Bone, BlockId::Amber, BlockId::Crystal] {
            let raw = id as u8;
            let recovered = BlockId::try_from(raw).unwrap();
            assert_eq!(id, recovered);
        }
    }

    /// Regression: leaves must NOT block collision. Player should be
    /// able to walk/jump through leaf clumps. Earlier versions had
    /// leaves as blocks_collision=true while emitting no cube faces —
    /// invisible walls.
    #[test]
    fn leaves_do_not_block_collision() {
        for leaf in [BlockId::OakLeaves, BlockId::BirchLeaves] {
            assert!(!leaf.blocks_collision(),
                "{:?} should NOT block collision (sub-voxel-only blocks pass-through)",
                leaf);
        }
    }

    /// Solid terrain still blocks collision.
    #[test]
    fn terrain_blocks_collision() {
        for solid in [BlockId::Stone, BlockId::Dirt, BlockId::Grass,
                      BlockId::Sand, BlockId::OakLog] {
            assert!(solid.blocks_collision(),
                "{:?} should block collision", solid);
        }
    }

    #[test]
    fn new_blocks_round_trip_and_parse() {
        let new = [
            (BlockId::Sandstone, "sandstone"), (BlockId::Obsidian, "obsidian"),
            (BlockId::CopperOre, "copper_ore"), (BlockId::Quartz, "quartz"),
            (BlockId::OakPlanks, "oak_planks"), (BlockId::StoneBricks, "stone_bricks"),
            (BlockId::Bricks, "bricks"),
            (BlockId::Glass, "glass"), (BlockId::IronBlock, "iron_block"),
            (BlockId::GoldBlock, "gold_block"), (BlockId::CoalBlock, "coal_block"),
            (BlockId::Bookshelf, "bookshelf"), (BlockId::Pumpkin, "pumpkin"),
        ];
        for (id, name) in new {
            assert_eq!(BlockId::try_from(id as u8).unwrap(), id, "{name} u8 round-trip");
            assert_eq!(BlockId::from_id_str(name), Some(id), "{name} parse");
        }
        // Glass is transparent; the building blocks are solid + collide.
        assert!(BlockId::Glass.is_transparent());
        assert!(BlockId::OakPlanks.blocks_collision());
        assert!(BlockId::Obsidian.blocks_collision());
    }
}