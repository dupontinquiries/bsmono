//! theme.rs — the app's look, now palette-driven so it can switch between themes at runtime.
//!
//! Colors live in a `Palette` (a Copy struct of every themeable color). A thread-local holds the ACTIVE
//! palette; the `BG()`/`INK()`/`AMBER()`/… accessor functions read from it, so every widget recolors the
//! instant `set_palette` swaps it (followed by `install` to re-skin egui's own Visuals). The original
//! unendlich-dash dark look is `FOLDING_AT_HOME`; the research-journal palettes (`READING_ROOM`,
//! `ARCHIVE`, `SLATE`) sit in the mid-tone band — not as bright as paper, not as dark as a typical web
//! UI, and deliberately not Solarized. Layout (spacing, radii) and the widget kit are theme-neutral.
use egui::{
    Align, Color32, Context, CornerRadius, FontFamily, FontId, Frame, Label, Layout, Margin,
    Pos2, Response, RichText, Sense, Stroke, StrokeKind, TextStyle, TextWrapMode, Visuals,
};

// ── palette ──────────────────────────────────────────────────────────────────

/// The grained surface a palette emulates — baked into the ambience texture (see `paint_ambience`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Material { None, Paper, Wood, Stone, Marble }

/// Every themeable color, plus whether the palette reads as dark (so egui starts from the right base
/// Visuals) and a display name for the picker. Copy so it can live in a `Cell`.
#[derive(Clone, Copy)]
pub struct Palette {
    pub name: &'static str,
    pub dark: bool,
    /// the grained material baked into the background (wood/stone/marble/paper, or None for a clean fill).
    pub material: Material,
    /// paint soft global-illumination lighting — sun rays + fog + glows that track the cursor and model.
    pub lit: bool,
    /// paint the living-tree canopy: leaves that rustle/shimmer with the graph's generation activity, lit by
    /// sun rays whose angle, warmth, and brightness follow the real system clock (see `sun_from_clock`).
    pub canopy: bool,
    /// the warm sun/lamp light color (warm for the library themes).
    pub light: Color32,
    pub bg: Color32, pub window: Color32, pub panel: Color32, pub panel2: Color32, pub line: Color32,
    pub ink: Color32, pub dim: Color32, pub faint: Color32, pub track_bg: Color32,
    pub amber: Color32, pub amber_soft: Color32, pub amber_ink: Color32,
    pub blue: Color32, pub cyan: Color32, pub violet: Color32, pub green: Color32,
    pub yellow: Color32, pub red: Color32, pub red_soft: Color32,
    pub maxmark: Color32, pub whisker: Color32,
}

const fn c(r: u8, g: u8, b: u8) -> Color32 { Color32::from_rgb(r, g, b) }

/// The original analytics dark look (exact hex from unendlich-telemetry.html). `amber` is the warm
/// action accent; `cyan` the status/selection accent.
pub const FOLDING_AT_HOME: Palette = Palette {
    name: "Folding@home", dark: true, material: Material::Marble, lit: true, canopy: false, light: c(0x7a,0x86,0x9e),
    bg: c(0x0a,0x0c,0x10), window: c(0x10,0x14,0x1b), panel: c(0x10,0x14,0x1b), panel2: c(0x16,0x1b,0x24), line: c(0x22,0x2a,0x36),
    ink: c(0xcd,0xd6,0xe3), dim: c(0x7b,0x87,0x98), faint: c(0x5d,0x67,0x75), track_bg: c(0x0b,0x0e,0x13),
    amber: c(0xf7,0xa2,0x3b), amber_soft: c(0xff,0xce,0x8a), amber_ink: c(0x1a,0x12,0x06),
    blue: c(0x6f,0xb0,0xf0), cyan: c(0x57,0xd6,0xc9), violet: c(0x9a,0x8c,0xff), green: c(0x8f,0xd6,0x94),
    yellow: c(0xf5,0xc2,0x11), red: c(0xff,0x63,0x63), red_soft: c(0xff,0x8a,0x8a),
    maxmark: c(0xb4,0x46,0x46), whisker: c(0xff,0xff,0xff),
};

/// Dimmed, dirt-stained reading surface: a darkened khaki-parchment (noticeably deeper than bright
/// paper), warm near-black ink, muted earth accents — with a procedural grain/dirt texture over the
/// background. Reads aged and a little soiled, not clean paper, and not Solarized.
pub const READING_ROOM: Palette = Palette {
    name: "Reading Room", dark: false, material: Material::Paper, lit: true, canopy: false, light: c(0xff,0xee,0xcf),
    bg: c(0xbd,0xb0,0x94), window: c(0xc6,0xba,0x9f), panel: c(0xc6,0xba,0x9f), panel2: c(0xb1,0xa5,0x89), line: c(0x97,0x8b,0x70),
    ink: c(0x2a,0x26,0x20), dim: c(0x4a,0x44,0x39), faint: c(0x68,0x60,0x50), track_bg: c(0xa8,0x9c,0x80),
    amber: c(0x8f,0x5a,0x28), amber_soft: c(0xb8,0x8a,0x52), amber_ink: c(0xf3,0xea,0xd6),
    blue: c(0x3d,0x5d,0x7a), cyan: c(0x35,0x6e,0x68), violet: c(0x6d,0x54,0x78), green: c(0x54,0x72,0x4a),
    yellow: c(0x9c,0x78,0x26), red: c(0x9a,0x3c,0x2a), red_soft: c(0xb8,0x66,0x50),
    maxmark: c(0x7e,0x33,0x26), whisker: c(0x2a,0x26,0x20),
};

/// Dark-mid warm: a warm charcoal noticeably lighter than a typical near-black web UI, soft cream ink,
/// muted brass/olive/dusty-blue accents — an aged-archive feel without going to paper or to #111.
pub const ARCHIVE: Palette = Palette {
    name: "Archive", dark: true, material: Material::Wood, lit: true, canopy: false, light: c(0xf0,0xcf,0x92),
    bg: c(0x2f,0x2b,0x24), window: c(0x38,0x33,0x28), panel: c(0x38,0x33,0x28), panel2: c(0x42,0x3c,0x30), line: c(0x54,0x4d,0x3e),
    ink: c(0xe7,0xdd,0xc8), dim: c(0xaf,0xa3,0x8b), faint: c(0x8a,0x80,0x6c), track_bg: c(0x29,0x25,0x1f),
    amber: c(0xcd,0xa1,0x4e), amber_soft: c(0xe6,0xc4,0x89), amber_ink: c(0x2a,0x21,0x14),
    blue: c(0x7f,0x9c,0xc0), cyan: c(0x6d,0xb3,0xa0), violet: c(0xa8,0x90,0xc0), green: c(0x9b,0xb0,0x6a),
    yellow: c(0xd8,0xb5,0x4a), red: c(0xcf,0x6a,0x52), red_soft: c(0xe0,0x91,0x7c),
    maxmark: c(0xa0,0x50,0x38), whisker: c(0xe7,0xdd,0xc8),
};

/// Dark-mid cool: a balanced blue-grey slate (lighter than common dark themes), soft ink, restrained
/// clay/teal/indigo accents. The neutral middle option.
pub const SLATE: Palette = Palette {
    name: "Slate", dark: true, material: Material::Stone, lit: true, canopy: false, light: c(0x9a,0xa6,0xb6),
    bg: c(0x34,0x38,0x3f), window: c(0x3c,0x41,0x4a), panel: c(0x3c,0x41,0x4a), panel2: c(0x45,0x4b,0x55), line: c(0x56,0x5d,0x68),
    ink: c(0xd9,0xde,0xe5), dim: c(0xaf,0xb6,0xc0), faint: c(0x8a,0x91,0x9d), track_bg: c(0x2f,0x33,0x3a),
    amber: c(0xc9,0x8a,0x5a), amber_soft: c(0xe0,0xad,0x84), amber_ink: c(0x22,0x1a,0x12),
    blue: c(0x7a,0xa5,0xd6), cyan: c(0x5f,0xb0,0xb0), violet: c(0x9d,0x92,0xcf), green: c(0x82,0xb0,0x8a),
    yellow: c(0xd6,0xc0,0x5a), red: c(0xd9,0x70,0x5f), red_soft: c(0xe8,0x97,0x8a),
    maxmark: c(0xa8,0x5a,0x48), whisker: c(0xd9,0xde,0xe5),
};

/// Old Library: a warm, dim, wood-panelled reading room after hours — deep walnut-brown surfaces, candle-
/// cream ink, brass accents, and the full ambience: a low afternoon sun raking across the room as shafts
/// of light through hanging dust, settled fog in the corners, and wood grain underfoot. The showcase for
/// the dusty-library look (sun rays + fog + glows track your cursor and the model).
pub const LIBRARY: Palette = Palette {
    name: "Old Library", dark: true, material: Material::Wood, lit: true, canopy: false, light: c(0xff,0xdd,0x9c),
    bg: c(0x21,0x18,0x10), window: c(0x2b,0x20,0x15), panel: c(0x2b,0x20,0x15), panel2: c(0x35,0x28,0x1b), line: c(0x4a,0x39,0x27),
    ink: c(0xeb,0xdc,0xbf), dim: c(0xb0,0x9c,0x7c), faint: c(0x80,0x6f,0x54), track_bg: c(0x1c,0x14,0x0d),
    amber: c(0xe0,0xa9,0x55), amber_soft: c(0xf6,0xcf,0x8e), amber_ink: c(0x24,0x18,0x09),
    blue: c(0x8a,0xa6,0xc2), cyan: c(0x6f,0xb8,0xa8), violet: c(0xb4,0x98,0xc8), green: c(0xa6,0xb4,0x6e),
    yellow: c(0xe6,0xc4,0x5e), red: c(0xd9,0x6e,0x52), red_soft: c(0xe8,0x95,0x7c),
    maxmark: c(0xa6,0x58,0x3a), whisker: c(0xeb,0xdc,0xbf),
};

/// Every palette, in picker order. The first is the default.
pub const MANSION_GARDEN: Palette = Palette {
    name: "Mansion Garden", dark: false, material: Material::Stone, lit: true, canopy: false, light: c(0xff,0xf0,0xd2),
    bg: c(0xcf,0xc6,0xac), window: c(0xdc,0xd2,0xb8), panel: c(0xdc,0xd2,0xb8), panel2: c(0xc8,0xbe,0xa2), line: c(0xb0,0xa0,0x70),
    ink: c(0x32,0x2c,0x20), dim: c(0x5c,0x53,0x3e), faint: c(0x7b,0x71,0x53), track_bg: c(0xc2,0xb8,0x9a),
    amber: c(0xb0,0x86,0x32), amber_soft: c(0xd8,0xb0,0x5a), amber_ink: c(0xf6,0xee,0xd6),
    blue: c(0x35,0x86,0x92), cyan: c(0x1e,0x9c,0xa0), violet: c(0x6d,0x54,0x78), green: c(0x52,0x7a,0x46),
    yellow: c(0xb0,0x88,0x28), red: c(0x9a,0x3c,0x2a), red_soft: c(0xb8,0x66,0x50),
    maxmark: c(0x9c,0x78,0x2e), whisker: c(0x32,0x2c,0x20),
};


/// Persian Garden: a chahar bagh at the golden hour — sun-bleached mud-brick walls, packed-earth
/// paths, turquoise water in the rills, and the deep green of cypress against it. Warm, dry, and
/// bright; the complementary turquoise/terracotta pair is the palette Persian gardens are built on.
pub const PERSIAN_GARDEN: Palette = Palette {
    name: "Persian Garden", dark: false, material: Material::Stone, lit: true, canopy: false, light: c(0xff,0xe6,0xb4),
    bg: c(0xd6,0xc0,0x98), window: c(0xe2,0xcf,0xaa), panel: c(0xe2,0xcf,0xaa), panel2: c(0xcd,0xb6,0x8e), line: c(0xb4,0x94,0x62),
    ink: c(0x35,0x2a,0x1c), dim: c(0x60,0x50,0x38), faint: c(0x82,0x70,0x50), track_bg: c(0xc6,0xae,0x86),
    amber: c(0xc0,0x86,0x2c), amber_soft: c(0xe2,0xb0,0x54), amber_ink: c(0xf8,0xf0,0xdc),
    blue: c(0x1e,0x82,0x96), cyan: c(0x24,0xa2,0xa8), violet: c(0x6e,0x4c,0x74), green: c(0x3e,0x6e,0x3a),
    yellow: c(0xc2,0x92,0x24), red: c(0xa8,0x40,0x2c), red_soft: c(0xc4,0x6c,0x52),
    maxmark: c(0xa8,0x74,0x28), whisker: c(0x35,0x2a,0x1c),
};

/// Garden Cloister: cool grey limestone, moss-green garth, weathered lead and bronze. Overcast and
/// quiet — the light a north-facing arcade actually gets, and the restraint a cloister is for.
pub const GARDEN_CLOISTER: Palette = Palette {
    name: "Garden Cloister", dark: false, material: Material::Stone, lit: true, canopy: false, light: c(0xf0,0xf0,0xe8),
    bg: c(0xc4,0xc4,0xb8), window: c(0xd2,0xd2,0xc6), panel: c(0xd2,0xd2,0xc6), panel2: c(0xba,0xba,0xac), line: c(0x98,0x98,0x88),
    ink: c(0x2c,0x2e,0x2a), dim: c(0x54,0x58,0x50), faint: c(0x74,0x78,0x6e), track_bg: c(0xb2,0xb2,0xa4),
    amber: c(0x96,0x7e,0x40), amber_soft: c(0xbc,0xa4,0x66), amber_ink: c(0xf2,0xf2,0xea),
    blue: c(0x44,0x6e,0x86), cyan: c(0x3e,0x86,0x82), violet: c(0x64,0x54,0x76), green: c(0x46,0x6e,0x38),
    yellow: c(0x9c,0x88,0x38), red: c(0x8e,0x44,0x36), red_soft: c(0xac,0x6c,0x5c),
    maxmark: c(0x86,0x70,0x36), whisker: c(0x2c,0x2e,0x2a),
};

/// Madrassa Court: Fez zellij — cobalt and glazed green against ivory plaster, brass lamplight, and
/// carved cedar. The most saturated palette in the set, because that is exactly what the tilework is.
pub const MADRASSA: Palette = Palette {
    name: "Madrassa Court", dark: false, material: Material::Stone, lit: true, canopy: false, light: c(0xff,0xec,0xc8),
    bg: c(0xdc,0xd0,0xb8), window: c(0xe8,0xde,0xc8), panel: c(0xe8,0xde,0xc8), panel2: c(0xd0,0xc2,0xa6), line: c(0xa8,0x92,0x6a),
    ink: c(0x2a,0x26,0x1e), dim: c(0x56,0x4c,0x3c), faint: c(0x7a,0x6c,0x56), track_bg: c(0xcc,0xbe,0xa2),
    amber: c(0xc2,0x94,0x30), amber_soft: c(0xe6,0xbc,0x60), amber_ink: c(0xfa,0xf4,0xe4),
    blue: c(0x1d,0x5a,0x8e), cyan: c(0x1c,0x86,0x8a), violet: c(0x6a,0x46,0x7c), green: c(0x1e,0x6a,0x4a),
    yellow: c(0xc9,0xa3,0x3c), red: c(0x9c,0x3a,0x30), red_soft: c(0xbc,0x64,0x54),
    maxmark: c(0xb0,0x82,0x2c), whisker: c(0x2a,0x26,0x1e),
};

/// Bamboo Grove: green-filtered daylight, moss, wet stone, and the one warm lantern. Dark because a
/// mature stand of bamboo genuinely is — the canopy takes most of the light before it reaches you.
pub const BAMBOO_GROVE: Palette = Palette {
    name: "Bamboo Grove", dark: true, material: Material::Wood, lit: true, canopy: true, light: c(0xd8,0xf0,0xb0),
    bg: c(0x16,0x22,0x18), window: c(0x1e,0x2c,0x1e), panel: c(0x1e,0x2c,0x1e), panel2: c(0x26,0x36,0x26), line: c(0x3a,0x4e,0x36),
    ink: c(0xdc,0xe8,0xc8), dim: c(0xa2,0xb4,0x8c), faint: c(0x74,0x88,0x62), track_bg: c(0x12,0x1c,0x14),
    amber: c(0xd0,0xa8,0x50), amber_soft: c(0xe8,0xc8,0x80), amber_ink: c(0x18,0x14,0x08),
    blue: c(0x74,0xa8,0xb4), cyan: c(0x6c,0xbc,0x9c), violet: c(0xa8,0x94,0xc0), green: c(0x9a,0xc4,0x60),
    yellow: c(0xd8,0xc4,0x62), red: c(0xc8,0x70,0x54), red_soft: c(0xdc,0x96,0x7c),
    maxmark: c(0x9c,0x7c,0x38), whisker: c(0xdc,0xe8,0xc8),
};

pub const PALETTES: &[Palette] = &[FOLDING_AT_HOME, READING_ROOM, ARCHIVE, SLATE, LIBRARY, CANOPY, MISTFALL, ATLANTIS, CLOUDSCAPE, FOREST, TREEHOUSE, MANSION_GARDEN, ALLEY, FRACTAL_SPIRES, RECURSIVE_CITADEL, HOLLOW_JUNGLE, YELLOW_BRICK_ROAD, BIFROST, TITAN_RIBCAGE, PERSIAN_GARDEN, GARDEN_CLOISTER, MADRASSA, BAMBOO_GROVE];

/// Frozen Bifrost Bunker: a tight circular arena — a raised basalt dais (Nordic knotwork packed with frost) ringed
/// by sunken glacial-ice trenches, its heart a shattered hexagonal pane of iridescent prismatic glass whose
/// upward dagger-shards glow with diffuse white energy bleeding from below. Shifting magenta/cyan aurora overhead
/// throws coloured light across the ice. Storytelling-lighting + verticality set-piece (not recursive).
pub const BIFROST: Palette = Palette {
    name: "Frozen Bifrost", dark: true, material: Material::Stone, lit: true, canopy: false, light: c(0xb4,0xdc,0xf0),
    bg: c(0x08,0x0c,0x1c), window: c(0x16,0x22,0x38), panel: c(0x16,0x22,0x38), panel2: c(0x20,0x30,0x4c), line: c(0x2e,0x42,0x64),
    ink: c(0xdc,0xe8,0xf6), dim: c(0x90,0xa6,0xc4), faint: c(0x5c,0x72,0x95), track_bg: c(0x0a,0x10,0x22),
    amber: c(0x66,0xc8,0xe0), amber_soft: c(0xa6,0xe6,0xf2), amber_ink: c(0x06,0x12,0x18),
    blue: c(0x60,0xa6,0xf0), cyan: c(0x5a,0xd6,0xe0), violet: c(0xc0,0x7c,0xf0), green: c(0x5a,0xc8,0xb0),
    yellow: c(0x9a,0xdc,0xe6), red: c(0xe0,0x66,0xc0), red_soft: c(0xf0,0x96,0xd6),
    maxmark: c(0x7a,0x9c,0xf0), whisker: c(0xd6,0xf0,0xff),
};

/// Titan's Ribcage Alleyway: a wide overgrown jungle lane straddled by the colossal fossilised ribs of a fallen
/// giant — chalky pale-yellow limestone bone, pitted and gouged to grey marrow — erupting from fractured marble
/// paving strangled by algae-green fig roots, flanked by dense recursive canopy. Knee-level yellow-green fog,
/// humid and rotting. Bone-and-decay POI set-piece (jungle uses the recursive tree).
pub const TITAN_RIBCAGE: Palette = Palette {
    name: "Titan's Ribcage", dark: false, material: Material::Stone, lit: true, canopy: true, light: c(0xe6,0xea,0xb6),
    bg: c(0x4a,0x52,0x36), window: c(0x32,0x3c,0x26), panel: c(0x32,0x3c,0x26), panel2: c(0x40,0x4a,0x30), line: c(0x4e,0x58,0x38),
    ink: c(0xe8,0xea,0xce), dim: c(0xa7,0xae,0x8a), faint: c(0x84,0x8a,0x6a), track_bg: c(0x3a,0x42,0x2c),
    amber: c(0xc8,0xb0,0x5e), amber_soft: c(0xe2,0xd0,0x8e), amber_ink: c(0x22,0x1e,0x0c),
    blue: c(0x52,0x86,0xa0), cyan: c(0x52,0xa8,0x92), violet: c(0x96,0x82,0xb0), green: c(0x6e,0xa0,0x44),
    yellow: c(0xd8,0xc8,0x66), red: c(0xc0,0x66,0x46), red_soft: c(0xd8,0x90,0x6e),
    maxmark: c(0x6e,0xa0,0x44), whisker: c(0x22,0x26,0x16),
};

/// Yellow Brick Road: a storybook Oz world. A recursive yellow-brick road winds and branches across rolling green
/// fields — scarlet poppies, golden corn, wildflower meadows — past recursive forests, toward a distant cluster of
/// glowing EMERALD CITY towers. Bright fairy-tale palette: meadow green, brick gold, emerald glow, poppy red,
/// cornflower sky.
pub const YELLOW_BRICK_ROAD: Palette = Palette {
    name: "Yellow Brick Road", dark: false, material: Material::Stone, lit: true, canopy: true, light: c(0xff,0xf0,0xc4),
    bg: c(0x9c,0xc4,0xdc), window: c(0x3a,0x52,0x34), panel: c(0x3a,0x52,0x34), panel2: c(0x48,0x62,0x40), line: c(0x54,0x70,0x4a),
    ink: c(0x2c,0x38,0x22), dim: c(0x5e,0x70,0x4c), faint: c(0x8a,0x9c,0x70), track_bg: c(0x86,0xae,0x6e),
    amber: c(0xe6,0xb8,0x44), amber_soft: c(0xf6,0xd8,0x84), amber_ink: c(0x28,0x1c,0x06),
    blue: c(0x5a,0x90,0xc8), cyan: c(0x46,0xb0,0x9a), violet: c(0x9a,0x7c,0xc0), green: c(0x4e,0xa0,0x50),
    yellow: c(0xe8,0xc8,0x54), red: c(0xd0,0x46,0x3e), red_soft: c(0xe6,0x78,0x66),
    maxmark: c(0x4e,0xa0,0x50), whisker: c(0x2c,0x38,0x22),
};

/// Hollow Jungle: a supersized world INSIDE a hollow rock planet. The ground is the inner surface of a vast
/// stone shell, so jungle curves up and over you in every direction; recursive trees grow inward from the crust
/// toward the open hollow at the heart, where a translucent OCEAN floats free, rippling and dripping, lit by a
/// small suspended sun. Lush greens, wet-stone browns, lagoon blue, warm interior light.
pub const HOLLOW_JUNGLE: Palette = Palette {
    name: "Hollow Jungle", dark: false, material: Material::Stone, lit: true, canopy: true, light: c(0xff,0xe2,0x9c),
    bg: c(0x24,0x3c,0x38), window: c(0x2a,0x3a,0x2c), panel: c(0x2a,0x3a,0x2c), panel2: c(0x36,0x48,0x36), line: c(0x40,0x54,0x40),
    ink: c(0xe6,0xee,0xd8), dim: c(0x9a,0xae,0x90), faint: c(0x75,0x8a,0x6e), track_bg: c(0x1c,0x2e,0x28),
    amber: c(0xd8,0xa6,0x54), amber_soft: c(0xf0,0xcc,0x86), amber_ink: c(0x18,0x14,0x08),
    blue: c(0x52,0x9c,0xc0), cyan: c(0x4e,0xb6,0xb6), violet: c(0x96,0x86,0xc0), green: c(0x5a,0x9c,0x46),
    yellow: c(0xc4,0xd6,0x6a), red: c(0xc0,0x5e,0x46), red_soft: c(0xd8,0x86,0x6a),
    maxmark: c(0x5a,0x9c,0x46), whisker: c(0x24,0x2c,0x1c),
};

/// Recursive Citadel: a supersized sunlit world of fractal towers. Each tower stacks a smaller drum on its crown
/// and sprouts corner turrets, which stack their own — generation after generation from one rule, so a whole
/// self-similar castle-city grows with no hand-planning. Warm sandstone + honeyed light, lapis pennants, a pale
/// desert haze. Subdivision-recursion counterpart to Fractal Spires' branching.
pub const RECURSIVE_CITADEL: Palette = Palette {
    name: "Recursive Citadel", dark: false, material: Material::Stone, lit: true, canopy: false, light: c(0xff,0xe2,0xa6),
    bg: c(0xc8,0xb4,0x90), window: c(0x6a,0x52,0x36), panel: c(0x6a,0x52,0x36), panel2: c(0x7e,0x62,0x42), line: c(0x8a,0x6e,0x4a),
    ink: c(0x3a,0x2c,0x1c), dim: c(0x3a,0x2c,0x1c), faint: c(0x9a,0x84,0x64), track_bg: c(0xb6,0xa2,0x80),
    amber: c(0xd0,0x96,0x44), amber_soft: c(0xf0,0xc6,0x82), amber_ink: c(0x24,0x18,0x0a),
    blue: c(0x4a,0x74,0xb0), cyan: c(0x52,0xa0,0xb0), violet: c(0x8a,0x6e,0xb0), green: c(0x86,0xa0,0x52),
    yellow: c(0xe0,0xc2,0x66), red: c(0xc0,0x5a,0x42), red_soft: c(0xd8,0x84,0x68),
    maxmark: c(0xc8,0x96,0x46), whisker: c(0x3a,0x2c,0x1c),
};

/// Fractal Spires: a supersized nocturnal world of self-similar crystalline towers. Each spire recursively
/// sprouts smaller spires from its crown, generation after generation, so detail STACKS fractally from a single
/// rule — no hand-placement. Deep indigo night; glacial teal + violet glow off the crystal tips lights a vast
/// dark plain. The dynamic LOD/cull keep the recursion affordable: far generations dissolve into the dark.
pub const FRACTAL_SPIRES: Palette = Palette {
    name: "Fractal Spires", dark: true, material: Material::Stone, lit: true, canopy: false, light: c(0x9a,0xc4,0xe6),
    bg: c(0x09,0x0d,0x1a), window: c(0x12,0x18,0x28), panel: c(0x12,0x18,0x28), panel2: c(0x1a,0x22,0x34), line: c(0x2a,0x36,0x4e),
    ink: c(0xd8,0xe4,0xf4), dim: c(0x8a,0x9a,0xb8), faint: c(0x5c,0x6a,0x87), track_bg: c(0x09,0x0d,0x16),
    amber: c(0x8a,0xd0,0xe6), amber_soft: c(0xbe,0xe8,0xf2), amber_ink: c(0x07,0x10,0x16),
    blue: c(0x6f,0xb0,0xf0), cyan: c(0x57,0xd6,0xc9), violet: c(0xa6,0x8c,0xff), green: c(0x6a,0xd6,0xb0),
    yellow: c(0x9a,0xe6,0xd2), red: c(0xc8,0x6a,0xb0), red_soft: c(0xe0,0x92,0xc8),
    maxmark: c(0x6a,0x8c,0xd0), whisker: c(0xcc,0xe6,0xff),
};

/// Alley: a rain-soaked overgrown service lane turned cozy mind-palace — damp timber + brick, climbing ivy,
/// wet stone with sky-mirroring puddles, trees leaning in for dappled afternoon light. Muted naturals: walnut
/// and cedar browns, mossy spring greens, cool wet-asphalt grays, soft puddle-blue. Warm 5000K sun shafts cut
/// through a cool 6800K overcast — painterly warm highlights over cool shadows.
pub const ALLEY: Palette = Palette {
    name: "Alley", dark: false, material: Material::Wood, lit: true, canopy: true, light: c(0xff,0xda,0x92),
    bg: c(0x52,0x56,0x4e), window: c(0x3a,0x3e,0x38), panel: c(0x3a,0x3e,0x38), panel2: c(0x46,0x4a,0x42), line: c(0x5e,0x64,0x54),
    ink: c(0xe8,0xea,0xdc), dim: c(0xad,0xb2,0x9d), faint: c(0x87,0x8e,0x7b), track_bg: c(0x2c,0x30,0x2a),
    amber: c(0xc8,0x9a,0x54), amber_soft: c(0xe0,0xc0,0x86), amber_ink: c(0x1a,0x18,0x10),
    blue: c(0x7a,0x9c,0xbc), cyan: c(0x6a,0xb0,0xb0), violet: c(0x8a,0x76,0x9c), green: c(0x58,0x8c,0x4a),
    yellow: c(0xb6,0xd1,0x84), red: c(0xb0,0x5a,0x42), red_soft: c(0xc8,0x82,0x6a),
    maxmark: c(0x6a,0x96,0x46), whisker: c(0x2c,0x30,0x28),
};

/// Canopy: a sun-dappled forest — deep leaf-litter floor, warm bark, and a living tree overhead. The
/// canopy's leaves grow with the conversation and rustle/shimmer with its generation activity; the sun
/// raking through them tracks the real time of day (golden at dawn/dusk, bright overhead at noon, cool and
/// dim at night). The showcase for the living-tree ambience.
pub const CANOPY: Palette = Palette {
    name: "Canopy", dark: true, material: Material::Wood, lit: true, canopy: true, light: c(0xff,0xe6,0x9c),
    bg: c(0x14,0x12,0x0a), window: c(0x20,0x1a,0x10), panel: c(0x20,0x1a,0x10), panel2: c(0x2c,0x23,0x15), line: c(0x46,0x37,0x20),
    ink: c(0xec,0xe2,0xc6), dim: c(0xb4,0xa2,0x78), faint: c(0x80,0x6e,0x4c), track_bg: c(0x10,0x0e,0x08),
    amber: c(0xe2,0xc2,0x5a), amber_soft: c(0xf2,0xda,0x90), amber_ink: c(0x16,0x18,0x06),
    blue: c(0x86,0xb4,0xc8), cyan: c(0x66,0xc2,0xa0), violet: c(0xb0,0x9c,0xcc), green: c(0x9c,0xc4,0x5e),
    yellow: c(0xe6,0xcf,0x5e), red: c(0xd8,0x72,0x52), red_soft: c(0xe8,0x98,0x7c),
    maxmark: c(0x6a,0x8a,0x3a), whisker: c(0xe9,0xe6,0xc8),
};

/// Forest: an open, sunlit woodland — brighter and greener than the enclosed Canopy. Daylight through trees.
pub const FOREST: Palette = Palette {
    name: "Forest", dark: false, material: Material::Wood, lit: true, canopy: true, light: c(0xff,0xf2,0xc0),
    bg: c(0x6e,0x86,0x6a), window: c(0x24,0x2e,0x1a), panel: c(0x24,0x2e,0x1a), panel2: c(0x32,0x40,0x24), line: c(0x4a,0x5e,0x34),
    ink: c(0xee,0xf0,0xd6), dim: c(0xb8,0xc2,0x96), faint: c(0x82,0x90,0x62), track_bg: c(0x1a,0x22,0x12),
    amber: c(0xc2,0xd2,0x5a), amber_soft: c(0xda,0xe6,0x90), amber_ink: c(0x16,0x1e,0x06),
    blue: c(0x86,0xc0,0xc8), cyan: c(0x7a,0xcc,0x9a), violet: c(0xb0,0x9c,0xcc), green: c(0x9c,0xcc,0x52),
    yellow: c(0xe6,0xd6,0x5e), red: c(0xe0,0x7a,0x52), red_soft: c(0xee,0xa0,0x7c),
    maxmark: c(0x6a,0x96,0x3a), whisker: c(0x2c,0x3a,0x1c),
};

/// Treehouse: a dusk canopy walk — warm lantern light among dark foliage, fireflies. Wood + twilight green.
pub const TREEHOUSE: Palette = Palette {
    name: "Treehouse", dark: true, material: Material::Wood, lit: true, canopy: true, light: c(0xff,0xc8,0x7a),
    bg: c(0x12,0x16,0x10), window: c(0x1e,0x18,0x10), panel: c(0x1e,0x18,0x10), panel2: c(0x2a,0x20,0x14), line: c(0x42,0x32,0x1e),
    ink: c(0xee,0xdc,0xb4), dim: c(0xb0,0x96,0x6a), faint: c(0x7a,0x66,0x47), track_bg: c(0x0e,0x10,0x0a),
    amber: c(0xf0,0xb0,0x52), amber_soft: c(0xf8,0xcf,0x88), amber_ink: c(0x16,0x12,0x06),
    blue: c(0x86,0xb4,0xc0), cyan: c(0x9a,0xd0,0x76), violet: c(0xb0,0x9c,0xcc), green: c(0x9c,0xc4,0x5a),
    yellow: c(0xe8,0xe0,0x6a), red: c(0xd8,0x72,0x52), red_soft: c(0xe8,0x98,0x7c),
    maxmark: c(0x6a,0x8a,0x3a), whisker: c(0xee,0xdc,0xb4),
};

/// Mistfall: a cool, fog-bound grotto — wet slate, pale moonlit haze, glacial teal accents. A serene
/// counterpart to the warm Canopy, after the foggy-waterfall inspiration. Stone material + heavy fog + the
/// drifting light read as mist rolling through a ruined garden.
pub const MISTFALL: Palette = Palette {
    name: "Mistfall", dark: true, material: Material::Stone, lit: true, canopy: false, light: c(0xc8,0xdc,0xe8),
    bg: c(0x0e,0x14,0x18), window: c(0x14,0x1e,0x24), panel: c(0x14,0x1e,0x24), panel2: c(0x1c,0x28,0x30), line: c(0x2c,0x3c,0x46),
    ink: c(0xd6,0xe2,0xe8), dim: c(0x8c,0x9e,0xa8), faint: c(0x5f,0x6f,0x78), track_bg: c(0x0a,0x10,0x14),
    amber: c(0x6f,0xc6,0xc2), amber_soft: c(0x9e,0xe2,0xde), amber_ink: c(0x04,0x14,0x14),
    blue: c(0x7c,0xb0,0xd8), cyan: c(0x66,0xd2,0xca), violet: c(0xa6,0x9c,0xd2), green: c(0x86,0xc2,0x9a),
    yellow: c(0xd8,0xcf,0x86), red: c(0xd8,0x80,0x80), red_soft: c(0xe8,0xa0,0xa0),
    maxmark: c(0x4c,0x7a,0x96), whisker: c(0xd6,0xe2,0xe8),
};

/// Cloudscape: an open, bright, airy sky-deck — high above weather, soft cumulus drifting. Shares the floating
/// globule idea with Mistfall's fog but light and roomy, for work that wants an unconfined idea-space.
pub const CLOUDSCAPE: Palette = Palette {
    name: "Cloudscape", dark: false, material: Material::Stone, lit: true, canopy: false, light: c(0xff,0xff,0xf6),
    bg: c(0xbe,0xd2,0xe4), window: c(0xe6,0xee,0xf6), panel: c(0xe6,0xee,0xf6), panel2: c(0xf0,0xf4,0xfa), line: c(0xa8,0xbc,0xce),
    ink: c(0x2c,0x3a,0x48), dim: c(0x55,0x66,0x78), faint: c(0x75,0x86,0x95), track_bg: c(0xd8,0xe2,0xec),
    amber: c(0x5c,0x9c,0xd2), amber_soft: c(0x9e,0xc6,0xe6), amber_ink: c(0x1a,0x30,0x42),
    blue: c(0x5c,0x9c,0xd2), cyan: c(0x6a,0xc2,0xd6), violet: c(0xa6,0x9c,0xd2), green: c(0x86,0xc2,0x9a),
    yellow: c(0xd8,0xcf,0x86), red: c(0xd8,0x80,0x80), red_soft: c(0xe8,0xa0,0xa0),
    maxmark: c(0x7c,0xa6,0xc6), whisker: c(0x2c,0x3a,0x48),
};

/// Atlantis: an underwater dome archive — deep teal abyss, blue-obsidian floor, glass-and-stone. Pressure
/// and protected silence under immensity. Glass chrome (most translucent) so the ocean reads through.
pub const ATLANTIS: Palette = Palette {
    name: "Atlantis", dark: true, material: Material::Stone, lit: true, canopy: false, light: c(0x88,0xe0,0xd8),
    bg: c(0x02,0x0c,0x12), window: c(0x06,0x16,0x1e), panel: c(0x06,0x16,0x1e), panel2: c(0x0c,0x22,0x2a), line: c(0x16,0x3a,0x44),
    ink: c(0xcc,0xe8,0xe4), dim: c(0x7c,0xa6,0xa2), faint: c(0x4a,0x6e,0x6a), track_bg: c(0x02,0x0e,0x14),
    amber: c(0x00,0xdd,0xbb), amber_soft: c(0x66,0xf0,0xdd), amber_ink: c(0x02,0x16,0x14),
    blue: c(0x44,0x99,0xdd), cyan: c(0x33,0xdd,0xcc), violet: c(0x88,0x99,0xdd), green: c(0x55,0xcc,0xaa),
    yellow: c(0xcc,0xdd,0x88), red: c(0xd8,0x80,0x80), red_soft: c(0xe8,0xa0,0xa0),
    maxmark: c(0x1b,0x55,0x60), whisker: c(0xcc,0xe8,0xe4),
};

thread_local! { static ACTIVE: std::cell::Cell<Palette> = const { std::cell::Cell::new(FOLDING_AT_HOME) }; }
thread_local! { static BASE: std::cell::Cell<Palette> = const { std::cell::Cell::new(FOLDING_AT_HOME) }; }
thread_local! { static DAY_BUCKET: std::cell::Cell<i64> = const { std::cell::Cell::new(-1) }; }

/// Swap the active palette (call `install` afterwards to re-skin egui's own widgets/Visuals).
/// The palette handed in is the BASE; what actually activates is its day/night adaptation for
/// the current clock (see `day_night_adjust_at`), and `tick_day_night` keeps it tracking.
pub fn set_palette(p: Palette) {
    BASE.with(|b| b.set(p));
    DAY_BUCKET.with(|d| d.set(day_bucket_now()));
    ACTIVE.with(|a| a.set(day_night_adjust_at(p, daylight01())));
}
/// The active palette.
pub fn active() -> Palette { ACTIVE.with(|a| a.get()) }
/// Find a palette by name (e.g. a persisted setting); falls back to the default.
pub fn palette_by_name(name: &str) -> Palette { PALETTES.iter().copied().find(|p| p.name == name).unwrap_or(FOLDING_AT_HOME) }

thread_local! { static VARIANT: std::cell::Cell<u8> = const { std::cell::Cell::new(1) }; }
/// The selected design variant (1..=10), set by the header dropdown so a round of candidate looks can be
/// compared live in the running app without a recompile per idea. Its *meaning* is redefined each round —
/// see `variant_label` for what the numbers currently are.
pub fn variant() -> u8 { VARIANT.with(|v| v.get()) }

/// POWER mode for Linux phones / battery saving: `LOGSCOPE_POWER=full|half|quarter` (or a 0..1 number).
/// Returns the resolution/work factor — `half`→0.5, `quarter`→0.25 — cutting fragment cost by factor² and
/// capping texture tier + mesh density downstream so a phone GPU sips battery.
pub fn power_mode() -> f32 {
    match std::env::var("LOGSCOPE_POWER").ok().map(|v| v.trim().to_lowercase()) {
        Some(ref v) if v == "half" => 0.5,
        Some(ref v) if v == "quarter" => 0.25,
        Some(ref v) if v == "full" || v.is_empty() => 1.0,
        Some(ref v) => v.parse::<f32>().unwrap_or(1.0).clamp(0.25, 1.0),
        None => 1.0,
    }
}

/// The texture detail tier actually baked, after the POWER cap: half-power caps at tier 6, quarter at tier 3, so
/// battery-saver modes bake far smaller textures (less VRAM + bandwidth) regardless of the chosen quality dial.
pub fn effective_variant() -> u8 {
    let v = variant();
    let p = power_mode();
    if p <= 0.3 { v.min(3) } else if p <= 0.6 { v.min(6) } else { v }
}
pub fn set_variant(v: u8) { VARIANT.with(|c| c.set(v.clamp(1, 16))); }

/// The style tokens for the carved-material chrome — the stable surface the look is expressed in (sandwich
/// layer 4). Iterating the *look* means editing these numbers (or swapping in a different `Skin`); the paint
/// code (`paint_carved`/`carved_label`) and the widget API (`ui::button`…) read them and never change. So a
/// restyle is one struct, not a sweep through call sites.
#[derive(Clone, Copy, Debug)]
pub struct Skin {
    pub radius: u8,                                            // corner radius
    pub fill_rest: u8, pub fill_hover: u8, pub fill_down: u8, pub fill_off: u8, // translucent base opacity
    pub accent_tint: f32,                                     // how much the accent tints the fill
    pub grain_alpha: u8,                                      // material speckle strength
    pub bevel_lit: u8, pub bevel_shadow: u8,                  // relief edges
    pub sheen: f32, pub sheen_speed: f32,                     // wet specular highlight
    pub vein: f32,                                            // mineral-vein text-glow scale
    pub engrave_shadow: u8, pub engrave_lip: u8,              // carved-text relief
}
impl Skin {
    /// The carved-stone look the app currently ships.
    pub const fn carved() -> Self {
        Skin {
            radius: 8, fill_rest: 104, fill_hover: 174, fill_down: 165, fill_off: 70,
            accent_tint: 0.20, grain_alpha: 26, bevel_lit: 26, bevel_shadow: 58,
            sheen: 1.0, sheen_speed: 1.3, vein: 1.0, engrave_shadow: 130, engrave_lip: 40,
        }
    }
}
thread_local! { static SKIN: std::cell::Cell<Skin> = const { std::cell::Cell::new(Skin::carved()) }; }
/// The active style tokens (layer 4). Read by the paint code; swap with `set_skin` to restyle wholesale.
pub fn skin() -> Skin { SKIN.with(|s| s.get()) }
pub fn set_skin(s: Skin) { SKIN.with(|c| c.set(s)); }

/// THIS ROUND the variant sweeps the **scene atmosphere / colour-grade**, applied to the GPU scene uniforms.
/// Returns (exposure, fog ×, bloom amount, bloom threshold, ambient ×, sun warmth: −1 cool … +1 warm).
pub fn grade(v: u8) -> (f32, f32, f32, f32, f32, f32) {
    match v {
        1  => (1.35, 1.0, 0.85, 0.62, 1.00,  0.00), // baseline — where we were
        2  => (1.60, 0.9, 0.85, 0.62, 1.12,  0.05), // brighter
        3  => (1.85, 0.7, 0.75, 0.66, 1.25,  0.00), // airy / bright, clearer air
        4  => (1.15, 1.4, 1.10, 0.55, 0.85, -0.05), // moody, dramatic, deep shadows
        5  => (1.50, 1.0, 0.80, 0.60, 1.05,  0.22), // warm golden hour
        6  => (1.42, 1.0, 0.85, 0.62, 1.05, -0.22), // cool moonlit
        7  => (1.55, 0.9, 1.35, 0.50, 1.08,  0.08), // dreamy, heavy bloom
        8  => (1.45, 0.7, 0.40, 0.72, 1.02,  0.00), // crisp, low bloom, clear
        9  => (1.50, 2.0, 1.00, 0.58, 1.05, -0.06), // heavy atmosphere / fog
        10 => (1.58, 1.0, 0.95, 0.70, 0.92,  0.06), // punchy high-contrast
        _  => (1.35, 1.0, 0.85, 0.62, 1.00,  0.00),
    }
}
/// The atmosphere/grade locked in while the variant sweeps detail this round (change to lock a chosen look).
pub const LOCKED_GRADE: u8 = 2;

/// THIS ROUND the variant sweeps surface + geometry DETAIL: how finely the procedural textures are baked
/// (re-uploaded live), how densely they tile, the normal-relief strength, and how finely the scene objects
/// are tessellated. Returns (texture px, texscale, bump/relief, tessellation ×). The atmosphere/grade is
/// locked (see `LOCKED_GRADE`) so only detail changes between candidates.
pub fn detail(v: u8) -> (usize, f32, f32, f32) {
    match v {
        1  => (128, 0.35, 0.70, 1.0), // baseline — where we were
        2  => (128, 0.45, 0.80, 1.2),
        3  => (192, 0.45, 0.85, 1.4),
        4  => (192, 0.60, 0.95, 1.6),
        5  => (256, 0.55, 0.95, 1.8),
        6  => (256, 0.70, 1.05, 2.0),
        7  => (320, 0.70, 1.10, 2.3),
        8  => (384, 0.85, 1.20, 2.6),
        9  => (448, 0.95, 1.30, 3.0),
        10 => (512, 1.10, 1.40, 3.4), // (was the old maximum)
        11 => (576, 1.15, 1.45, 3.8),
        12 => (640, 1.20, 1.50, 4.2),
        13 => (1024, 1.25, 1.55, 4.6),
        14 => (1280, 1.35, 1.62, 5.0),
        15 => (1536, 1.45, 1.68, 5.3),
        16 => (2048, 1.55, 1.75, 5.7), // ultra — 2k textures + densest geometry, real-GPU only
        _  => (128, 0.35, 0.70, 1.0),
    }
}
/// Short label for each variant in the dropdown, so it's clear what's being compared this round (detail).
pub fn variant_label(v: u8) -> &'static str {
    match v {
        1 => "1 · 128px ×1.0", 2 => "2 · 128px ×1.2", 3 => "3 · 192px ×1.4", 4 => "4 · 192px ×1.6",
        5 => "5 · 256px ×1.8", 6 => "6 · 256px ×2.0", 7 => "7 · 320px ×2.3", 8 => "8 · 384px ×2.6",
        9 => "9 · 448px ×3.0", 10 => "10 · 512px ×3.4",
        11 => "11 · 576px ×3.8", 12 => "12 · 640px ×4.2", 13 => "13 · 1k ×4.6",
        14 => "14 · 1280px ×5.0", 15 => "15 · 1536px ×5.3", 16 => "16 · 2k ×5.7",
        _ => "1 · 128px ×1.0",
    }
}

/// Whether the active palette has ANY ambience to paint (a grained material or the lighting).
pub fn has_ambience() -> bool { let p = active(); p.lit || p.material != Material::None }

// Light sources the ambience tracks: the pointer (a moving sun-catcher) and the model's home in the UI
// (a warm hearth). app.rs feeds these each frame; `paint_light` reads them.
thread_local! {
    static CURSOR_LIGHT: std::cell::Cell<Option<Pos2>> = const { std::cell::Cell::new(None) };
    static MODEL_LIGHT:  std::cell::Cell<Option<Pos2>> = const { std::cell::Cell::new(None) };
}
/// Point the cursor light at `p` (or `None` when the pointer leaves the area).
pub fn set_cursor_light(p: Option<Pos2>) { CURSOR_LIGHT.with(|c| c.set(p)); }
/// Anchor the model's warm glow at `p` (e.g. the model picker, or the active node in the spatial view).
pub fn set_model_light(p: Option<Pos2>) { MODEL_LIGHT.with(|c| c.set(p)); }

// Living-tree cues from the graph: the canopy's fullness tracks `nodes` (the tree grows with the chat) and
// the leaves rustle/shimmer with `active` (generations in flight + queued). CANOPY_ANIM tracks the last
// seen node count + the start of a "new growth" brightness pulse (fired when `nodes` increases).
thread_local! {
    static CANOPY_CUES: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
    static CANOPY_ANIM: std::cell::Cell<(u32, f32)> = const { std::cell::Cell::new((0, -10.0)) };
}
/// Feed the living-tree theme the graph's state each frame: `nodes` = graph size, `active` = generations in
/// flight + queued (stream + queue + fan). The canopy grows with `nodes`; the leaves rustle with `active`.
pub fn set_canopy(nodes: usize, active: usize) { CANOPY_CUES.with(|c| c.set((nodes as u32, active as u32))); }

// The ambience layer — high-res grain + global-illumination lighting + boreal aurora shine — is baked into
// ONE cached texture, rebuilt only when the (device-pixel) size or palette changes, and drawn as a single
// textured quad per frame. So the rich look costs one image at paint time, not thousands of shapes.
struct Ambience { key: (u32, u32, &'static str), tex: egui::TextureHandle }
thread_local! { static AMBIENCE: std::cell::RefCell<Option<Ambience>> = const { std::cell::RefCell::new(None) }; }

#[inline] fn rgb01(c: Color32) -> [f32; 3] { [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0] }
#[inline] fn premul(c: [f32; 3], a: f32) -> [f32; 4] { [c[0]*a, c[1]*a, c[2]*a, a] }
#[inline] fn over(src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
    let ia = 1.0 - src[3];
    [src[0] + dst[0]*ia, src[1] + dst[1]*ia, src[2] + dst[2]*ia, src[3] + dst[3]*ia]
}
#[inline] fn alpha_col(c: Color32, a: u8) -> Color32 { Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a) }
#[inline] fn clear_col(c: Color32) -> Color32 { Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 0) }

// Cheap value noise → fractal Brownian motion, the basis for the wood/stone/marble grain and the fog.
#[inline] fn hash2(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x1657_4D2B) ^ (y as u32).wrapping_mul(0x68E3_1DA4);
    h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 12; h = h.wrapping_mul(0x297A_2D39); h ^= h >> 15;
    h as f32 / u32::MAX as f32
}
#[inline] fn vnoise(x: f32, y: f32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (xf, yf) = (x - xi, y - yi);
    let (x0, y0) = (xi as i32, yi as i32);
    let u = xf * xf * (3.0 - 2.0 * xf);
    let v = yf * yf * (3.0 - 2.0 * yf);
    let (a, b) = (hash2(x0, y0), hash2(x0 + 1, y0));
    let (c, d) = (hash2(x0, y0 + 1), hash2(x0 + 1, y0 + 1));
    let ab = a + (b - a) * u; let cd = c + (d - c) * u;
    ab + (cd - ab) * v
}
#[inline] fn fbm(mut x: f32, mut y: f32, oct: u32) -> f32 {
    let (mut amp, mut sum, mut norm) = (0.5f32, 0.0f32, 0.0f32);
    for _ in 0..oct { sum += amp * vnoise(x, y); norm += amp; x *= 2.0; y *= 2.0; amp *= 0.5; }
    if norm > 0.0 { sum / norm } else { 0.0 }
}
/// Signed grain intensity for a material at device pixel (x,y): negative darkens, positive lightens.
fn material_grain(mat: Material, x: f32, y: f32) -> f32 {
    match mat {
        Material::None => 0.0,
        Material::Paper => 0.15 * (vnoise(x * 0.5, y * 0.06) * 2.0 - 1.0), // faint horizontal fibres
        Material::Wood => {                                                 // wavy vertical grain lines
            let warp = fbm(x * 0.004, y * 0.02, 2) * 8.0;
            let ring = (x * 0.05 + warp).sin() * 0.5 + 0.5;
            -0.6 * (1.0 - ring).powf(3.0) + 0.10 * (fbm(x * 0.02, y * 0.02, 2) * 2.0 - 1.0)
        }
        Material::Stone => {                                                // mottled + thin cracks
            let m = fbm(x * 0.012, y * 0.012, 3);
            let crack = (1.0 - (fbm(x * 0.02 + 5.0, y * 0.02 + 9.0, 2) * 2.0 - 1.0).abs() * 6.0).clamp(0.0, 1.0);
            (m - 0.5) * 0.7 - 0.5 * crack
        }
        Material::Marble => {                                               // turbulent veins
            let turb = fbm(x * 0.01, y * 0.02, 3) * 6.0;
            let v = (x * 0.012 + y * 0.004 + turb).sin();
            0.5 * (1.0 - v.abs()).powf(4.0) + 0.08 * (fbm(x * 0.03, y * 0.03, 2) * 2.0 - 1.0)
        }
    }
}
// Soft radial glow (triangle fan: bright centre → transparent rim).
fn add_glow(mesh: &mut egui::epaint::Mesh, c: Pos2, r: f32, col: Color32, a: u8) {
    let segs = 28u32;
    let ctr = mesh.vertices.len() as u32;
    mesh.colored_vertex(c, alpha_col(col, a));
    for k in 0..=segs {
        let ang = k as f32 / segs as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(c + egui::vec2(ang.cos(), ang.sin()) * r, clear_col(col));
    }
    for k in 0..segs { mesh.add_triangle(ctr, ctr + 1 + k, ctr + 2 + k); }
}
// A soft widening light shaft from `src` along unit `dir` (bright at source, transparent at the far end).
fn add_shaft(mesh: &mut egui::epaint::Mesh, src: Pos2, dir: egui::Vec2, len: f32, w0: f32, w1: f32, col: Color32, a: u8) {
    let perp = egui::vec2(-dir.y, dir.x);
    let i = mesh.vertices.len() as u32;
    mesh.colored_vertex(src + perp * w0, alpha_col(col, a));
    mesh.colored_vertex(src - perp * w0, alpha_col(col, a));
    mesh.colored_vertex(src + dir * len - perp * w1, clear_col(col));
    mesh.colored_vertex(src + dir * len + perp * w1, clear_col(col));
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i, i + 2, i + 3);
}
// A small leaf quad (diamond) centred at `c`, long axis rotated by `rot`.
fn add_leaf(mesh: &mut egui::epaint::Mesh, c: Pos2, sz: f32, rot: f32, col: Color32) {
    let (s, co) = (rot.sin(), rot.cos());
    let ax = egui::vec2(co, s) * sz;
    let ay = egui::vec2(-s, co) * (sz * 0.5);
    let i = mesh.vertices.len() as u32;
    mesh.colored_vertex(c + ax, col);
    mesh.colored_vertex(c + ay, col);
    mesh.colored_vertex(c - ax, col);
    mesh.colored_vertex(c - ay, col);
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i, i + 2, i + 3);
}
#[inline] fn warm_tint(c: Color32, f: f32) -> Color32 { lerp_col(c, Color32::from_rgb(0xff, 0xa0, 0x46), f) } // → sunset orange
#[inline] fn cool_tint(c: Color32, f: f32) -> Color32 { lerp_col(c, Color32::from_rgb(0x78, 0x96, 0xd2), f) } // → moonlight blue

// ── day/night palette adaptation ─────────────────────────────────────────────
//
// The scenes were already clock-lit (`sun_from_clock`), but the UI PALETTE underneath stayed
// frozen: a bright paper theme at 2am glares, a cave theme at noon fights the room. So the
// active palette now adapts: LIGHT palettes darken as night deepens (surfaces dim toward a
// lamplit version of themselves — ink stays put, and stays readable because the dimming is
// bounded), and DARK palettes lift slightly through the day (surfaces gain a little daylight so
// they don't read as a void at noon). The adaptation is a pure function of (palette, daylight),
// so it's unit-tested across every palette; the clock only picks the input.

/// Daylight at hour-of-day `h` (0..24): 0 at night, 1 at solar noon — the same half-sine
/// `sun_from_clock` uses, so the UI dims exactly when the canopy sun does.
pub fn daylight01_at(h: f32) -> f32 {
    (std::f32::consts::PI * ((h - 6.0) / 12.0)).sin().clamp(0.0, 1.0)
}

/// Daylight for the current wall clock (same TZ convention as `sun_from_clock`).
pub fn daylight01() -> f32 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(43_200);
    daylight01_at(((secs.rem_euclid(86_400)) as f32) / 3600.0)
}

/// 5-minute clock bucket — the granularity at which the adaptation re-evaluates.
fn day_bucket_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) / 300
}

#[inline] fn scale_col(c: Color32, f: f32) -> Color32 {
    Color32::from_rgb(
        ((c.r() as f32) * f).round().clamp(0.0, 255.0) as u8,
        ((c.g() as f32) * f).round().clamp(0.0, 255.0) as u8,
        ((c.b() as f32) * f).round().clamp(0.0, 255.0) as u8,
    )
}
#[inline] fn lift_col(c: Color32, amt: u8) -> Color32 {
    Color32::from_rgb(c.r().saturating_add(amt), c.g().saturating_add(amt), c.b().saturating_add(amt))
}

/// The day/night adaptation, pure: `day` in [0,1] (1 = noon). Light palettes dim their SURFACES
/// toward a 0.42× lamplit version by up to 55% at deep night (ink/accents untouched — the
/// bounded dim keeps dark-ink-on-light readable, verified by the contrast tests). Dark palettes
/// lift surfaces by up to +26 luma at noon — subtle, so the identity survives.
pub fn day_night_adjust_at(p: Palette, day: f32) -> Palette {
    let mut q = p;
    let surf = |c: Color32, out: &mut Color32, f: f32, dark: bool| {
        *out = if dark { lerp_col(c, lift_col(c, 26), f) } else { lerp_col(c, scale_col(c, 0.42), f) };
    };
    let mut f = if p.dark { 0.9 * day } else { 0.55 * (1.0 - day) };
    // bound the night dim by the palette's own headroom: never dim a light surface past the
    // point its ink drops under 4.5:1 (Reading Room is already lamplit — it dims less than
    // bright paper does). Palettes whose ink starts below 4.5 don't dim at all.
    if !p.dark && f > 0.0 {
        let dim_bg = |f: f32| lerp_col(p.bg, scale_col(p.bg, 0.42), f);
        if contrast_ratio(p.ink, dim_bg(f)) < 4.5 {
            let (mut lo, mut hi) = (0.0f32, f);
            for _ in 0..12 {
                let mid = (lo + hi) * 0.5;
                if contrast_ratio(p.ink, dim_bg(mid)) >= 4.5 { lo = mid; } else { hi = mid; }
            }
            f = lo;
        }
    }
    surf(p.bg, &mut q.bg, f, p.dark);
    surf(p.window, &mut q.window, f, p.dark);
    surf(p.panel, &mut q.panel, f, p.dark);
    surf(p.panel2, &mut q.panel2, f, p.dark);
    surf(p.track_bg, &mut q.track_bg, f, p.dark);
    surf(p.line, &mut q.line, f, p.dark);
    // night also cools/warms the lamp a touch, matching the canopy sun's mood
    q.light = if p.dark { p.light } else { lerp_col(p.light, cool_tint(p.light, 0.5), (1.0 - day) * 0.6) };

    // ── RAW-PAIR LEGIBILITY ──────────────────────────────────────────────────────────────────────
    //
    // The markdown layer asks for contrast-ensured colours, but most of the app does not: it calls
    // theme::INK()/DIM()/FAINT()/CYAN() straight against a panel. Measured across all 19 palettes,
    // 87 of 190 such pairs were under 4.5:1, several catastrophically — Yellow Brick Road's INK on
    // PANEL was 1.24:1, which is body text you cannot read.
    //
    // So the palette itself is repaired here, once, rather than at thousands of call sites. Text roles
    // are nudged toward legibility against BOTH surfaces they appear on (bg and panel), because a fix
    // that satisfies one can break the other. Accents go through the HUE-PRESERVING adjuster so a
    // palette keeps its identity — Cloudscape's cyan stays cyan, it just stops being invisible.
    //
    // Targets follow WCAG's own tiering rather than one blanket number: body text 4.5, secondary 4.0,
    // and hints 3.0 (the large-text/UI-component level). Demanding 4.5 of a deliberately-faint hint
    // would flatten the visual hierarchy into a wall of equally-loud text, which is its own
    // readability problem.
    // ENSURE AGAINST THE PANEL, not against both surfaces.
    //
    // Satisfying bg AND panel simultaneously was tried and is impossible on several palettes: when the
    // two surfaces sit on opposite sides of mid-grey, the best achievable worst-case contrast is ~2.4:1
    // no matter what colour is chosen (measured — every failing pair collapsed to the same pole value).
    // No single text colour can be readable on both, which is a property of those palettes, not a bug
    // to code around.
    //
    // The panel is the surface that matters: message frames, side panels and every subpanel draw their
    // text on it. `bg` is the window backdrop, which carries chrome and very little reading text. So the
    // repair targets the panel and the bg case is left to `ensure_contrast` at the few call sites that
    // paint text directly on the backdrop.
    let on_panel = |fg: Color32, target: f32, accent: bool| -> Color32 {
        if accent { ensure_contrast_accent(fg, q.panel, target) } else { ensure_contrast(fg, q.panel, target) }
    };
    q.ink   = on_panel(q.ink,   4.5, false);
    q.dim   = on_panel(q.dim,   4.0, false);
    q.faint = on_panel(q.faint, 3.0, false);
    q.cyan  = on_panel(q.cyan,  4.5, true);
    q.amber = on_panel(q.amber, 4.5, true);
    // STATUS COLOURS at 3.0, not 4.5. These are graphical objects under WCAG 1.4.11 rather than body
    // text: green and red usually appear as a dot, a bar or a border, and pushing them to a text-grade
    // ratio over-saturates them into something that shouts. 3:1 is the level at which a reader can tell
    // them apart from the surface and from each other, which is all they need to do.
    //
    // Measured before this: 11 palettes had a status colour under 3:1, the worst being Cloudscape's
    // green at 1.37 — a success indicator very nearly invisible on its own panel.
    q.green = on_panel(q.green, 3.0, true);
    q.red   = on_panel(q.red,   3.0, true);


    q
}

/// Per-frame hook (piggybacked on `paint_ambience`): when the 5-minute clock bucket rolls over,
/// recompute the active palette from the BASE and re-install visuals. Cheap the rest of the time.
pub fn tick_day_night(ctx: &Context) {
    let now = day_bucket_now();
    let prev = DAY_BUCKET.with(|d| d.get());
    if now != prev {
        DAY_BUCKET.with(|d| d.set(now));
        let base = BASE.with(|b| b.get());
        ACTIVE.with(|a| a.set(day_night_adjust_at(base, daylight01())));
        install(ctx);
    }
}

// ── contrast machinery (markdown legibility on every palette) ────────────────
//
// Cloudscape reads because near-black ink on near-white paper has ~15:1 contrast; several other
// palettes hand markdown colors that fail on THEIR backgrounds (neon cyan inline code on a pale
// panel, dark ink over dark wood). Instead of hand-retuning 19 palettes, the markdown layer asks
// for CONTRAST-ENSURED colors: the palette's color, nudged toward black or white (whichever side
// can actually reach the target ratio against the current background) just far enough to hit a
// WCAG-style ratio. Palettes keep their identity; text keeps its legibility. Pure + tested
// across every palette in `PALETTES`.

/// A mark that carries status WITHOUT colour.
///
/// Nudging the palette to separate red from green was tried and reverted: lightening a status colour and
/// then re-establishing its 3:1 contrast against the panel are opposing moves, and they cancel — the
/// same two-passes-fighting failure as the earlier text-contrast attempt. Colour cannot solve this on
/// every palette.
///
/// So status is never encoded in colour ALONE. A caller pairs the colour with this mark, and a reader
/// who cannot distinguish the hues reads the glyph instead. This is also simply better for everyone:
/// a ✓ is legible at a glance in a way a green dot is not.
pub fn status_mark(ok: bool) -> &'static str { if ok { "✓" } else { "✗" } }

/// As above for a three-state indicator, where "in progress" is its own thing rather than a dimmer
/// version of success — a spinner that looks like a faded tick reads as "done, weakly".
pub fn status_mark3(state: Option<bool>) -> &'static str {
    match state { Some(true) => "✓", Some(false) => "✗", None => "…" }
}

/// WCAG relative luminance of an sRGB color.
pub fn rel_luminance(c: Color32) -> f32 {
    fn lin(u: u8) -> f32 {
        let s = u as f32 / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    }
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio (≥ 1.0; 21 max).
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (rel_luminance(a), rel_luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// `fg`, nudged toward black or white until it reaches `min` contrast against `bg` (or the best
/// that side can do). The side is chosen by which pole achieves more contrast against `bg`, so
/// light palettes darken failing colors and dark palettes lighten them — the hue survives, only
/// the value moves. Binary search on the blend factor keeps the nudge minimal.
pub fn ensure_contrast(fg: Color32, bg: Color32, min: f32) -> Color32 {
    if contrast_ratio(fg, bg) >= min { return fg; }
    let toward = if contrast_ratio(Color32::WHITE, bg) >= contrast_ratio(Color32::BLACK, bg) { Color32::WHITE } else { Color32::BLACK };
    if contrast_ratio(toward, bg) < min { return toward; } // bg is mid-grey; give it the pole
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..12 {
        let mid = (lo + hi) * 0.5;
        if contrast_ratio(lerp_col(fg, toward, mid), bg) >= min { hi = mid; } else { lo = mid; }
    }
    lerp_col(fg, toward, hi)
}

/// Like `ensure_contrast` but HUE-PRESERVING — for accent colors (inline code, links) that must keep their
/// identity. Instead of lerping toward pure black/white (which desaturates a mid-tone accent into mud — the
/// washed-out backticks), it moves the accent's LIGHTNESS: lighter (scale up toward white-ish) when the
/// surface is dark, darker (scale down) when the surface is light, keeping hue+saturation. Steps until it
/// clears `min` against `bg`, or returns the strongest step it can reach. `bg` is the LIVE (day/night
/// adjusted) surface, so this tracks the Catalina-style brightness curve automatically.
pub fn ensure_contrast_accent(fg: Color32, bg: Color32, min: f32) -> Color32 {
    if contrast_ratio(fg, bg) >= min { return fg; }
    // Which way improves contrast? Compare a lighter vs a darker nudge of the accent itself.
    let lighter = lift_col(fg, 40);
    let darker = scale_col(fg, 0.6);
    let go_lighter = contrast_ratio(lighter, bg) >= contrast_ratio(darker, bg);
    let mut best = fg;
    // up to 24 fine steps in the chosen direction; keep the first that clears, else the strongest.
    for i in 1..=24u32 {
        let t = i as f32 / 24.0;
        let cand = if go_lighter {
            // toward a light, still-saturated version: lift toward white but retain the hue by blending
            // with a lightened copy rather than pure white.
            lerp_col(fg, lift_col(fg, 200), t)
        } else {
            // toward a dark version: scale down (keeps hue, drops value).
            scale_col(fg, 1.0 - 0.85 * t)
        };
        best = cand;
        if contrast_ratio(cand, bg) >= min { return cand; }
    }
    // couldn't reach the target with hue-preserving moves (very mid-tone surface): fall back to the
    // guaranteed pole so text is at least legible, accepting some desaturation only in that corner case.
    ensure_contrast(fg, bg, min)
}

/// Nudge `fg` until it clears `min` contrast against the surface markdown actually renders on. Assistant/
/// user messages render inside a frame filled with PANEL, so that's the surface to guarantee. PANEL() reads
/// the LIVE day/night-adjusted palette, so these track the brightness curve. Accents use the hue-preserving
/// adjuster (backticks stay cyan, not mud); body/ink use the plain one (a value shift is fine for text).
fn md_on_panel(fg: Color32, min: f32) -> Color32 { ensure_contrast(fg, PANEL(), min) }
fn md_accent_on_panel(fg: Color32, min: f32) -> Color32 { ensure_contrast_accent(fg, PANEL(), min) }
/// Markdown body ink, ≥ 4.5:1 (WCAG AA) on the message panel.
pub fn md_ink() -> Color32 { md_on_panel(INK(), 4.5) }
/// Markdown accent (inline code / links), ≥ 4.5:1 on the panel — hue-preserving so `code` stays its accent
/// color across light/dark themes and the day/night curve instead of collapsing toward black/white.
pub fn md_accent() -> Color32 { md_accent_on_panel(CYAN(), 4.5) }
/// Markdown secondary (bullets, quote text), ≥ 4.5:1 on the panel — real reading content gets the full
/// body floor on the surface it lands on.
pub fn md_dim() -> Color32 { md_on_panel(DIM(), 4.5) }
/// Markdown BOLD ink, ≥ 7.0:1 on the panel (ceiling-capped). egui's default fonts carry no
/// bold weight in this pipeline, so COLOR is the entire bold signal — **bold** must land
/// visibly HEAVIER than body ink (4.5) on every palette and every point of the day/night
/// curve, or emphasis reads as de-emphasis. That was the reported bug: bold went through
/// egui's generic `visuals().strong_text_color()`, the one markdown color that never passed
/// through the contrast machinery, and washed out against several panels.
pub fn md_strong() -> Color32 { md_on_panel(INK(), 7.0) }
/// Blockquote bar/wash accent, guaranteed ≥ 3.0:1 (a decorative element needs less).
pub fn md_quote_accent() -> Color32 { md_accent_on_panel(AMBER(), 3.0) }
/// A colored inline run (e.g. the workflow-word sequence RECONSTRUCT→MODEL→…) that keeps the violet hue but
/// is guaranteed to read on the message panel.
pub fn md_run_violet() -> Color32 { md_accent_on_panel(VIOLET(), 4.5) }

/// The sun for the canopy theme, derived from the real system clock: a low warm sun at dawn/dusk, bright
/// and near-overhead at noon, dim and cool at night. Returns (shaft direction, colour, intensity 0..1).
/// Set `SUN_TZ_OFFSET_SECS` to your local UTC offset (e.g. -5*3600 for EST) so noon lines up with midday.
fn sun_from_clock(base: Color32) -> (egui::Vec2, Color32, f32) {
    const SUN_TZ_OFFSET_SECS: i64 = 0;
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(43_200);
    let h = (((secs + SUN_TZ_OFFSET_SECS).rem_euclid(86_400)) as f32) / 3600.0; // hour of day [0,24)
    let elev = (std::f32::consts::PI * ((h - 6.0) / 12.0)).sin();                // 0@6h 1@12h 0@18h, <0 at night
    let above = elev.max(0.0);
    let az = ((h - 6.0) / 12.0).clamp(0.0, 1.0);                                 // 0 dawn (from left) → 1 dusk (right)
    let dir = egui::vec2((az - 0.5) * 1.4, 0.55 + 0.45 * above).normalized();    // steeper at noon
    if above > 0.02 { (dir, warm_tint(base, (1.0 - above) * 0.8), 0.25 + 0.75 * above) }       // golden when low
    else            { (dir, cool_tint(base, 0.6), 0.10 + 0.12 * (elev + 0.4).max(0.0)) }       // dim cool night
}

/// Paint the living-tree canopy over the upper band of `rect`: leaves that grow in number with the graph,
/// sway in the wind, and shimmer/rustle harder while the graph is generating (a new turn fires a brightness
/// pulse). `sun_dir`/`sun_col` come from `sun_from_clock` so the foliage is backlit by the time-of-day sun.
fn paint_canopy(painter: &egui::Painter, rect: egui::Rect, t: f32, _sun_dir: egui::Vec2, sun_col: Color32) {
    let (nodes, active) = CANOPY_CUES.with(|c| c.get());
    // brightness pulse on new growth (node count went up)
    let (last, mut pulse_start) = CANOPY_ANIM.with(|c| c.get());
    if nodes > last { pulse_start = t; }
    CANOPY_ANIM.with(|c| c.set((nodes, pulse_start)));
    let pulse = (1.0 - (t - pulse_start) / 0.7).clamp(0.0, 1.0); // linear decay over 0.7s
    let load = (active as f32 / 4.0).min(1.0);                   // generation load 0..1
    let rustle = 0.35 + 0.65 * load + 0.5 * pulse;               // sway/shimmer amplitude

    let count = (70 + (nodes as usize * 4).min(230)).min(300);   // canopy fills out as the chat grows
    let band = rect.height() * 0.42;                            // foliage occupies the upper ~42%
    let greens = [(0x3a, 0x55, 0x20u32), (0x52, 0x6e, 0x2a), (0x6e, 0x88, 0x34), (0x97, 0xa4, 0x44)];
    let mut mesh = egui::epaint::Mesh::default();
    for k in 0..count {
        let (hx, hy, hp) = (hash2(k as i32, 11), hash2(k as i32, 29), hash2(k as i32, 47));
        let cluster = if hp > 0.62 { rect.width() * 0.18 } else { 0.0 }; // a denser mass on the right (the tree)
        let x = rect.left() + rect.width() * hx * (1.0 - 0.18 * (hp > 0.62) as i32 as f32) + cluster;
        let y = rect.top() + band * hy.powf(1.6);                          // biased toward the top
        let sway = (t * (0.6 + hp) + hp * 6.28).sin() * (6.0 + 10.0 * rustle);
        let bob = (t * (0.4 + hp * 0.5) + hp * 3.0).cos() * (3.0 + 5.0 * rustle);
        let pos = Pos2::new(x + sway, y + bob);
        let shimmer = (0.5 + 0.5 * (t * (1.4 + hp * 2.0) + hp * 9.0).sin() * (0.3 + 0.6 * load) + 0.5 * pulse).clamp(0.0, 1.0);
        let g = greens[(hp * 4.0) as usize % 4];
        let col = lerp_col(Color32::from_rgb(g.0 as u8, g.1 as u8, g.2 as u8), sun_col, 0.30 * shimmer); // backlit rim
        add_leaf(&mut mesh, pos, 4.0 + 6.0 * hp, t * 0.3 + hp * 6.28, alpha_col(col, (110.0 + 110.0 * shimmer) as u8));
    }
    painter.add(mesh);

    // dappled sunlight on the floor — a few soft spots drifting + flickering as the leaves move overhead
    for k in 0..6 {
        let hp = hash2(k as i32 + 200, 71);
        let x = rect.left() + rect.width() * (0.12 + 0.76 * hp) + (t * 4.0 + hp * 10.0).sin() * 14.0;
        let y = rect.bottom() - rect.height() * (0.10 + 0.10 * hp);
        let fl = (0.5 + 0.5 * (t * (0.8 + hp) + hp * 6.0).sin()).clamp(0.0, 1.0);
        painter.circle_filled(Pos2::new(x, y), 16.0 + 10.0 * fl, alpha_col(sun_col, (8.0 + 26.0 * fl * (0.4 + 0.6 * load)) as u8));
    }
}

/// Bake the active palette's STATIC surface into one premultiplied RGBA image (unmultiplied `ColorImage`
/// out): a soft top-down ambient light, an edge vignette, the grained material (wood/stone/marble/paper),
/// and fine dust speckle. The drifting fog (animated frames) and the moving sun rays are NOT here — they're
/// layered live in `paint_ambience`/`paint_light`.
fn build_surface_image(w: usize, h: usize, p: &Palette) -> egui::ColorImage {
    let (wf, hf) = (w as f32, h as f32);
    let mut acc = vec![[0f32; 4]; w * h]; // premultiplied, composited with `over`

    let light = rgb01(p.light);
    let darkv = { let b = rgb01(p.bg); [b[0]*0.40, b[1]*0.40, b[2]*0.40] }; // vignette sink
    let gdark = { let b = rgb01(p.bg); [b[0]*0.50, b[1]*0.50, b[2]*0.50] }; // grain shadow

    for y in 0..h {
        let yf = y as f32; let ty = yf / hf;
        let cy = (ty - 0.5).abs() * 2.0;
        for x in 0..w {
            let xf = x as f32; let tx = xf / wf;
            let mut a = [0f32; 4];
            if p.lit {
                let topl = (1.0 - ty).clamp(0.0, 1.0).powf(1.5);          // light falls from above
                a = over(premul(light, 0.10 * topl), a);
                let edge = (((tx - 0.5).abs() * 2.0).max(cy)).powi(3);    // 0 centre → 1 corner
                a = over(premul(darkv, 0.20 * edge), a);
            }
            let g = material_grain(p.material, xf, yf);
            if g < 0.0 { a = over(premul(gdark, (-g * 0.12).min(0.30)), a); }
            else if g > 0.0 { a = over(premul(light, (g * 0.10).min(0.28)), a); }
            acc[y * w + x] = a;
        }
    }

    // fine dust speckle on top of the material (skipped for the clean None surface)
    if p.material != Material::None {
        let mut s: u64 = 0xD1B5_4A32_D192_ED03 ^ (w as u64).rotate_left(17) ^ (h as u64).wrapping_mul(0x9E37_79B1);
        let mut rnd = || { s ^= s << 13; s ^= s >> 7; s ^= s << 17; ((s >> 40) as u32) as f32 / (1u32 << 24) as f32 };
        let fine = ((w * h) / 26).min(150_000);
        for _ in 0..fine {
            let i = ((rnd() * hf) as usize % h) * w + ((rnd() * wf) as usize % w);
            acc[i] = over(premul(gdark, 0.03 + 0.05 * rnd()), acc[i]);
        }
    }

    let mut bytes = vec![0u8; w * h * 4];
    for (i, pm) in acc.iter().enumerate() {
        let a = pm[3].clamp(0.0, 1.0);
        let (r, g, b) = if a > 0.0001 { (pm[0] / a, pm[1] / a, pm[2] / a) } else { (0.0, 0.0, 0.0) };
        let o = i * 4;
        bytes[o]     = (r.clamp(0.0, 1.0) * 255.0) as u8;
        bytes[o + 1] = (g.clamp(0.0, 1.0) * 255.0) as u8;
        bytes[o + 2] = (b.clamp(0.0, 1.0) * 255.0) as u8;
        bytes[o + 3] = (a * 255.0) as u8;
    }
    egui::ColorImage::from_rgba_unmultiplied([w, h], &bytes)
}

/// Bake ONE drifting-fog frame (a variant) as a small, low-frequency haze image in the light colour,
/// denser toward the floor. Cheap; several of these are crossfaded over time in `paint_ambience`, so the
/// fog billows without per-frame per-pixel cost.
fn build_fog_image(w: usize, h: usize, p: &Palette, variant: u32) -> egui::ColorImage {
    let (wf, hf) = (w as f32, h as f32);
    let dust = rgb01(p.light);
    let off = variant as f32 * 17.3 + 3.0; // decorrelate the variants
    let mut bytes = vec![0u8; w * h * 4];
    for y in 0..h {
        let ty = y as f32 / hf;
        for x in 0..w {
            let tx = x as f32 / wf;
            let f = fbm(tx * 3.0 + off, ty * 3.0 + off * 0.7, 4);
            let a = (0.18 * (0.30 + 0.70 * ty) * f).clamp(0.0, 1.0); // settles toward the floor
            let o = (y * w + x) * 4;
            bytes[o]     = (dust[0] * 255.0) as u8;
            bytes[o + 1] = (dust[1] * 255.0) as u8;
            bytes[o + 2] = (dust[2] * 255.0) as u8;
            bytes[o + 3] = (a * 255.0) as u8;
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([w, h], &bytes)
}

// Drifting-fog frame set (low-res) + a tiny random crossfade schedule, so the haze billows for cheap.
const FOG_FRAMES: usize = 4;
struct FogCache { key: (u32, u32, &'static str), frames: Vec<egui::TextureHandle> }
thread_local! {
    static FOG_CACHE: std::cell::RefCell<Option<FogCache>> = const { std::cell::RefCell::new(None) };
    // (current frame, next frame, segment start time, segment duration, rng state)
    static FOG_ANIM: std::cell::Cell<(usize, usize, f64, f64, u64)> = const { std::cell::Cell::new((0, 1, 0.0, 0.0, 0x2545_F491_4F6C_DD1D)) };
}

/// Paint the active palette's ambience over `rect`: the cached STATIC surface (material/light/vignette/
/// dust) plus the DRIFTING fog (two of several low-res variant frames crossfaded on a random schedule).
/// `time` is seconds (for the fog drift). Cheap per frame; bakes only on resize/theme change. Draw behind
/// content, then call `paint_light`. Opaque cards sit on top of all of it.
pub fn paint_ambience(ctx: &Context, painter: &egui::Painter, rect: egui::Rect, time: f64) {
    tick_day_night(ctx); // per-frame hook: cheap bucket check; re-adapts the palette as the clock moves
    let p = active();
    if !(p.lit || p.material != Material::None) { return; }
    if rect.width() < 4.0 || rect.height() < 4.0 { return; }
    let ppp = ctx.pixels_per_point().clamp(1.0, 3.0);
    let cap = 1536u32; // long-side texel cap — material tolerates slight blur; raise for crisper grain
    let mut iw = ((rect.width()  * ppp).round() as u32).max(8);
    let mut ih = ((rect.height() * ppp).round() as u32).max(8);
    iw = (iw / 32).max(1) * 32; ih = (ih / 32).max(1) * 32; // coarsen so dragging doesn't rebuild constantly
    let long = iw.max(ih);
    if long > cap { let sc = cap as f32 / long as f32; iw = ((iw as f32 * sc) as u32).max(8); ih = ((ih as f32 * sc) as u32).max(8); }
    let uv = egui::Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));

    // static surface (cached, one image)
    let key = (iw, ih, p.name);
    let tex = AMBIENCE.with(|c| {
        if let Some(a) = c.borrow().as_ref() { if a.key == key { return a.tex.clone(); } }
        let img = build_surface_image(iw as usize, ih as usize, &p);
        let tex = ctx.load_texture(format!("surface::{}", p.name), img, egui::TextureOptions::LINEAR);
        *c.borrow_mut() = Some(Ambience { key, tex: tex.clone() });
        tex
    });
    painter.image(tex.id(), rect, uv, Color32::WHITE);

    // drifting fog — crossfade two low-res variant frames, retargeted on a random schedule
    if p.lit {
        let fw = 384u32.min(iw).max(8);
        let fh = (((fw as f32) * (ih as f32 / iw as f32)) as u32).max(8);
        let fkey = (fw, fh, p.name);
        let frames = FOG_CACHE.with(|c| {
            if let Some(f) = c.borrow().as_ref() { if f.key == fkey { return f.frames.clone(); } }
            let frames: Vec<egui::TextureHandle> = (0..FOG_FRAMES).map(|v| {
                let img = build_fog_image(fw as usize, fh as usize, &p, v as u32);
                ctx.load_texture(format!("fog::{}::{v}", p.name), img, egui::TextureOptions::LINEAR)
            }).collect();
            *c.borrow_mut() = Some(FogCache { key: fkey, frames: frames.clone() });
            frames
        });
        let n = frames.len();
        let (mut cur, mut next, mut start, mut dur, mut rng) = FOG_ANIM.with(|c| c.get());
        if dur <= 0.0 { start = time; dur = 3.5; }
        let mut phase = ((time - start) / dur) as f32;
        if phase >= 1.0 { // segment done: advance + pick a new random target/duration
            cur = next;
            rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17;
            next = (cur + 1 + (rng as usize % (n - 1).max(1))) % n;
            rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17;
            dur = 2.5 + (rng % 1000) as f64 / 1000.0 * 3.5;
            start = time; phase = 0.0;
        }
        FOG_ANIM.with(|c| c.set((cur, next, start, dur, rng)));
        let x = phase * phase * (3.0 - 2.0 * phase); // smoothstep; the two frames' densities sum to ~1
        let wa = |a: f32| Color32::from_rgba_unmultiplied(255, 255, 255, (a.clamp(0.0, 1.0) * 255.0) as u8);
        painter.image(frames[cur].id(),  rect, uv, wa(1.0 - x));
        painter.image(frames[next].id(), rect, uv, wa(x));
    }
}

/// Paint the live light layer over `rect`: a window sunbeam (parallel shafts raking down-right, swaying
/// gently), a sunburst that tracks the cursor (slowly rotating + pulsing), a warm hearth glow at the
/// model's position, and a few drifting sunlit dust motes. Light sources get a two-layer bloom (wide halo +
/// bright core) for an HDR-ish feel. `time` is seconds. Cheap; draw it behind content.
pub fn paint_light(painter: &egui::Painter, rect: egui::Rect, time: f64) {
    let p = active();
    if !p.lit { return; }
    let warm = p.light;
    let amber = p.amber;
    let diag = rect.width().hypot(rect.height());
    let t = time as f32;
    let mut mesh = egui::epaint::Mesh::default();

    // 1) the sun — for the canopy theme its angle/warmth/brightness follow the system clock; otherwise a
    //    fixed warm window beam that sways gently.
    let (dir, sun_col, sun_i) = if p.canopy { sun_from_clock(p.light) }
        else { (egui::vec2(0.52 + (t * 0.05).sin() * 0.03, 0.85).normalized(), warm, 1.0) };
    let n = if p.canopy { 10 } else { 7 };
    let src_fx = (0.5 - dir.x * 0.5).clamp(0.06, 0.94); // horizontally up-sun
    for k in 0..n {
        let tt = k as f32 / (n as f32 - 1.0);
        let sx = rect.left() + rect.width() * (src_fx - 0.18 + 0.36 * tt);
        add_shaft(&mut mesh, Pos2::new(sx, rect.top() - 10.0), dir, diag * 0.95, 9.0 + 14.0 * (1.0 - tt), 42.0, sun_col, (16.0 * sun_i) as u8);
    }
    let wp = Pos2::new(rect.left() + rect.width() * src_fx, rect.top() + rect.height() * 0.05);
    add_glow(&mut mesh, wp, diag * 0.55, sun_col, (16.0 * sun_i) as u8); // bloom: wide soft halo
    add_glow(&mut mesh, wp, diag * 0.22, sun_col, (22.0 * sun_i) as u8); //        + tighter brighter core

    // 2) a roaming sun-catcher — the global light drifts slowly on its own (paired with the view's
    //    parallax), so illumination plays across the scene instead of sticking to the cursor. Soft + wide.
    let drift_p = Pos2::new(
        rect.left() + rect.width()  * (0.5 + 0.30 * (t * 0.035).sin() + 0.10 * (t * 0.011 + 1.0).sin()),
        rect.top()  + rect.height() * (0.40 + 0.22 * (t * 0.026 + 0.7).cos()),
    );
    {
        let pulse = 1.0 + 0.05 * (t * 0.5).sin();
        add_glow(&mut mesh, drift_p, 205.0 * pulse, warm, 12);
        add_glow(&mut mesh, drift_p, 110.0 * pulse, warm, 20);
        let rays = 9;
        for k in 0..rays {
            let ang = k as f32 / rays as f32 * std::f32::consts::TAU + 0.3 + t * 0.05; // a lazy turn
            let d = egui::vec2(ang.cos(), ang.sin());
            let len = (140.0 + 70.0 * ((k as f32 * 1.7).sin() * 0.5 + 0.5)) * pulse;
            add_shaft(&mut mesh, drift_p, d, len, 4.0, 18.0, warm, 9);
        }
    }

    // 3) model hearth — gently pulsing warm glow + soft rays
    if let Some(mp) = MODEL_LIGHT.with(|c| c.get()) {
        let pulse = 1.0 + 0.08 * (t * 0.9 + 1.0).sin();
        add_glow(&mut mesh, mp, 140.0 * pulse, amber, 14);
        add_glow(&mut mesh, mp, 80.0 * pulse, amber, 26);
        let rays = 7;
        for k in 0..rays {
            let ang = k as f32 / rays as f32 * std::f32::consts::TAU + t * 0.05;
            let d = egui::vec2(ang.cos(), ang.sin());
            add_shaft(&mut mesh, mp, d, 90.0, 2.5, 12.0, amber, 12);
        }
    }
    painter.add(mesh);
    if p.canopy { paint_canopy(painter, rect, t, dir, sun_col); } // foliage dapples the shafts

    // 4) sunlit dust motes — drifting specks, brighter where the roaming light passes
    for k in 0..16 {
        let ph = k as f32 * 2.39996; // golden-angle spread
        let px = rect.left() + rect.width()  * (0.5 + 0.46 * (t * 0.05 + ph).sin());
        let py = rect.top()  + rect.height() * (0.5 + 0.42 * (t * 0.037 + ph * 1.7).cos());
        let pt = Pos2::new(px, py);
        let a = 9.0 + 60.0 * (1.0 - pt.distance(drift_p) / 200.0).clamp(0.0, 1.0);
        let r = 0.8 + 0.7 * ((ph * 3.0).sin() * 0.5 + 0.5);
        painter.circle_filled(pt, r, alpha_col(warm, a as u8));
    }
}

// ── SCENE → UI LIGHT BLEED ─────────────────────────────────────────────────────────────────────────────
// The 3D palace shouldn't feel like a picture behind a separately-skinned UI: its light should spill OUT of
// the viewport onto the surrounding chrome so the whole app reads as lit by the same world. `scene_light`
// exposes the live time-of-day sun (the SAME `sun_from_clock` the scene render uses) tinted toward the
// world's accent, and `paint_light_bleed` washes it across the UI.

/// The live scene light this frame: its dominant `color`, `intensity` in [0,1], and the screen-space `dir`
/// it comes from. Derived from the active palette + time-of-day sun, tinted toward the world accent so the
/// bleed carries the world's identity (cyan Atlantis, gold Mansion, green Forest…).
#[derive(Clone, Copy)]
pub struct SceneLight { pub color: Color32, pub intensity: f32, pub dir: egui::Vec2 }

pub fn scene_light() -> SceneLight {
    let p = active();
    let (dir, sun_col, sun_i) = sun_from_clock(p.light);
    let color = lerp_col(sun_col, p.cyan, 0.32);            // sun + a third of the world accent → identity
    let intensity = (0.36 + 0.64 * sun_i).clamp(0.0, 1.0);  // floor so a dim/night world still bleeds its colour
    SceneLight { color, intensity, dir }
}

// a vertical gradient quad (colour `ctop` at y0 → `cbot` at y1) — for the edge bleeds onto the chrome bars.
fn grad_band(mesh: &mut egui::epaint::Mesh, x0: f32, x1: f32, y0: f32, y1: f32, ctop: Color32, cbot: Color32) {
    let i = mesh.vertices.len() as u32;
    mesh.colored_vertex(Pos2::new(x0, y0), ctop);
    mesh.colored_vertex(Pos2::new(x1, y0), ctop);
    mesh.colored_vertex(Pos2::new(x1, y1), cbot);
    mesh.colored_vertex(Pos2::new(x0, y1), cbot);
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i, i + 2, i + 3);
}

/// Spill the live scene light out of the central 3D viewport onto the surrounding UI chrome: a directional
/// sun WASH from the sun side, soft glows BLEEDING onto the top header + bottom tab bar where they border the
/// scene, and a faint overall wash so the ENTIRE surface carries the world's light. Subtle, additive,
/// non-interactive (painted on a foreground layer over the chrome). `t` = seconds (breathes gently);
/// `strength` scales the whole effect (1.0 immersive glass mode, ~0.4 the gentler default-mode bleed).
pub fn paint_light_bleed(ctx: &Context, screen: egui::Rect, sl: SceneLight, t: f32, strength: f32) {
    if sl.intensity <= 0.001 || strength <= 0.001 { return; }
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("scene_light_bleed")));
    let k = sl.intensity * strength * (0.9 + 0.1 * (t * 0.5).sin());
    let c = sl.color;
    let (w, h) = (screen.width(), screen.height());
    let mut mesh = egui::epaint::Mesh::default();
    // 1) directional SUN WASH — a wide soft radial glow, positioned on the side the sun rises from, up high.
    let sx = (screen.center().x + sl.dir.x * w * 0.42).clamp(screen.left(), screen.right());
    let sy = screen.top() + h * (0.05 + 0.16 * (1.0 - sl.dir.y.clamp(0.0, 1.0)));
    add_glow(&mut mesh, Pos2::new(sx, sy), w * 0.58, c, (42.0 * k) as u8);
    // 2) TOP edge bleed onto the header glass — a band fading DOWN from the top edge into the scene.
    grad_band(&mut mesh, screen.left(), screen.right(), screen.top(), screen.top() + h * 0.16,
              alpha_col(c, (32.0 * k) as u8), clear_col(c));
    // 3) BOTTOM edge bleed onto the tab bar — a band fading UP from the bottom edge into the scene.
    grad_band(&mut mesh, screen.left(), screen.right(), screen.bottom() - h * 0.14, screen.bottom(),
              clear_col(c), alpha_col(c, (28.0 * k) as u8));
    painter.add(egui::Shape::mesh(mesh));
    // 4) faint overall wash so the WHOLE ui carries the light (kept low so text stays legible).
    painter.rect_filled(screen, r(0), alpha_col(c, (7.0 * sl.intensity * strength) as u8));
}

// Accessor functions named like the old consts, so every `theme::INK()` call reads the live palette.
macro_rules! pal_colors {
    ($($up:ident => $field:ident),* $(,)?) => {
        $( #[allow(non_snake_case)] #[inline] pub fn $up() -> Color32 { ACTIVE.with(|a| a.get().$field) } )*
    };
}
pal_colors! {
    BG=>bg, WINDOW=>window, PANEL=>panel, PANEL2=>panel2, LINE=>line, INK=>ink, DIM=>dim, FAINT=>faint, TRACK_BG=>track_bg,
    AMBER=>amber, AMBER_SOFT=>amber_soft, AMBER_INK=>amber_ink, BLUE=>blue, CYAN=>cyan, VIOLET=>violet, PURPLE=>violet,
    GREEN=>green, YELLOW=>yellow, RED=>red, RED_SOFT=>red_soft, MAXMARK=>maxmark, WHISKER=>whisker,
}

pub fn r(n: u8) -> CornerRadius { CornerRadius::same(n) }

/// Carved-stone treatment for a tab/button `rect`: a raised bevel always, and — for the active tab — a
/// glowing engraved outline that pulses plus a light sweep travelling across the inscription. `t` is
/// seconds; `glow` is the accent (pass the clock-driven light so it rides the day cycle).
pub fn carved_tab(painter: &egui::Painter, rect: egui::Rect, t: f32, active: bool, glow: Color32) {
    let q = rect.shrink(1.0);
    let seg = |a: Pos2, b: Pos2, c: Color32| painter.line_segment([a, b], Stroke::new(1.0, c));
    seg(q.left_bottom(), q.left_top(), Color32::from_white_alpha(22));   // bevel: lit top-left
    seg(q.left_top(), q.right_top(), Color32::from_white_alpha(22));
    seg(q.right_top(), q.right_bottom(), Color32::from_black_alpha(50)); //        shadowed bottom-right
    seg(q.left_bottom(), q.right_bottom(), Color32::from_black_alpha(50));
    if active {
        let pulse = 0.5 + 0.5 * (t * 2.2).sin();
        painter.rect_stroke(q, r(3), Stroke::new(1.3, alpha_col(glow, (45.0 + 55.0 * pulse) as u8)), StrokeKind::Inside);
        let sx = q.left() + q.width() * (t * 0.5).fract();                // light sweep across the engraving
        let (l, rg) = ((sx - 7.0).max(q.left()), (sx + 7.0).min(q.right()));
        if rg > l { painter.rect_filled(egui::Rect::from_min_max(Pos2::new(l, q.top()), Pos2::new(rg, q.bottom())), r(0), alpha_col(glow, 22)); }
    }
}

// ── animation + Adwaita-style motion kit ─────────────────────────────────────

/// Gamma-correct color lerp (for animated hover/active/track transitions).
/// Shared carved-material surface for forefront chrome (buttons, pills, chips). The active palette's grain
/// reads through a translucent, beveled inset, and a specular "wet" sheen tracks the sun and shimmers,
/// brightening on hover — so the chrome feels carved from the same stone/wood/paper as the room and plays
/// with light and moisture. `sheen` scales the highlight (≈1.0 for buttons, ≈0.4 for quiet tags).
struct Carved { hov: f32, down: bool, enabled: bool, accent: Option<Color32>, sheen: f32 }

fn paint_carved(p: &egui::Painter, rect: egui::Rect, t: f32, c: Carved) {
    let pal = active();
    let sk = skin();
    let rr = r(sk.radius);
    let accent = c.accent;

    // translucent base so the background material reads through — a polished inset of the same material
    let base = match accent { Some(a) => lerp_col(PANEL2(), a, sk.accent_tint + 0.30 * c.hov), None => PANEL2() };
    let fill_a = if !c.enabled { sk.fill_off } else if c.down { sk.fill_down }
                 else { (sk.fill_rest as f32 + (sk.fill_hover as f32 - sk.fill_rest as f32) * c.hov) as u8 };
    p.rect_filled(rect, rr, alpha_col(base, fill_a));

    // a sparse scatter of the material's own grain, so the surface is textured even over flat backgrounds
    if pal.material != Material::None {
        let cols = (rect.width() / 16.0).round().clamp(3.0, 9.0) as usize;
        let rows = (rect.height() / 12.0).round().clamp(2.0, 4.0) as usize;
        let ga = sk.grain_alpha as f32;
        for iy in 0..rows {
            for ix in 0..cols {
                let fx = rect.left() + rect.width() * (ix as f32 + 0.5) / cols as f32;
                let fy = rect.top() + rect.height() * (iy as f32 + 0.5) / rows as f32;
                let n = material_grain(pal.material, fx, fy);
                let a = (n.abs() * ga).min(ga) as u8;
                let col = if n >= 0.0 { Color32::from_white_alpha(a) } else { Color32::from_black_alpha(a) };
                p.circle_filled(egui::pos2(fx, fy), 1.3, col);
            }
        }
    }

    // relief: a lit cap up top, a shadowed foot below
    let cap = egui::Rect::from_min_max(rect.left_top(), Pos2::new(rect.right(), rect.top() + rect.height() * 0.5));
    p.rect_filled(cap, rr, Color32::from_white_alpha((8.0 + 10.0 * c.hov) as u8));
    let foot = egui::Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - rect.height() * 0.42), rect.right_bottom());
    p.rect_filled(foot, rr, Color32::from_black_alpha(24));

    // directional bevel — lit top-left, shadowed bottom-right (same idiom as carved_tab)
    let q = rect.shrink(0.7);
    let seg = |a: Pos2, b: Pos2, col: Color32| p.line_segment([a, b], Stroke::new(1.0, col));
    let lit = Color32::from_white_alpha(if c.enabled { sk.bevel_lit } else { sk.bevel_lit / 2 + 1 });
    seg(q.left_bottom(), q.left_top(), lit);
    seg(q.left_top(), q.right_top(), lit);
    seg(q.right_top(), q.right_bottom(), Color32::from_black_alpha(sk.bevel_shadow));
    seg(q.left_bottom(), q.right_bottom(), Color32::from_black_alpha(sk.bevel_shadow));
    let rim = lerp_col(pal.line, accent.unwrap_or(pal.line), c.hov);
    p.rect_stroke(rect, rr, Stroke::new(1.0, rim), StrokeKind::Inside);

    // specular / moisture sheen — soft bright spot toward the sun, shimmering, brighter on hover
    let sheen = c.sheen * sk.sheen;
    if sheen > 0.0 && c.enabled {
        let (sun, sun_col, _) = sun_from_clock(pal.light);
        let shimmer = 0.5 + 0.5 * (t * sk.sheen_speed + rect.left() * 0.012).sin();
        let sheen_c = lerp_col(Color32::WHITE, sun_col, 0.5);
        let sx = rect.center().x - sun.x * rect.width() * 0.30;
        let sy = rect.top() + rect.height() * 0.34;
        let glow = ((18.0 + 26.0 * c.hov + 16.0 * shimmer) * sheen) as u8;
        let rad = (rect.height() * 0.6).min(rect.width() * 0.5);
        p.circle_filled(Pos2::new(sx, sy), rad, alpha_col(sheen_c, (glow as f32 * 0.33) as u8));
        p.circle_filled(Pos2::new(sx, sy), rad * 0.58, alpha_col(sheen_c, (glow as f32 * 0.6) as u8));
        p.circle_filled(Pos2::new(sx, sy), rad * 0.26, alpha_col(sheen_c, glow));
        // a thin drifting glint — the wet highlight — strongest while hovered/pressed
        if c.hov > 0.01 || c.down {
            let gx = q.left() + q.width() * (t * 0.33).fract();
            let (l, rg) = ((gx - 5.0).max(q.left()), (gx + 5.0).min(q.right()));
            if rg > l {
                p.rect_filled(egui::Rect::from_min_max(Pos2::new(l, q.top()), Pos2::new(rg, q.bottom())), r(0),
                    alpha_col(sheen_c, ((20.0 + 18.0 * c.hov) * sheen) as u8));
            }
        }
    }

    // accent glow ring when lively
    if let (Some(a), true) = (accent, c.hov > 0.01 || c.down) {
        let pulse = 0.5 + 0.5 * (t * 2.4).sin();
        let amt = if c.down { 1.0 } else { c.hov };
        p.rect_stroke(rect, rr, Stroke::new(1.3, alpha_col(a, ((45.0 + 55.0 * pulse) * amt) as u8)), StrokeKind::Inside);
    }
}

/// Draw text as if chiselled into the stone: a dark shadow pressed in below-right and a lit lip above-left
/// give the letters engraved relief, then the ink face on top. An optional `vein` adds a soft mineral glow
/// bleeding from the inscription (stronger `glow` = brighter). Laid out per-colour so each pass tints right.
fn carved_label(p: &egui::Painter, center: Pos2, text: &str, font: FontId, ink: Color32, vein: Option<Color32>, glow: f32) {
    let sk = skin();
    let lay = |c: Color32| p.layout_no_wrap(text.to_owned(), font.clone(), c);
    let pos = center - lay(ink).size() / 2.0;
    if let (Some(g), true) = (vein, glow * sk.vein > 0.01) {
        let (core, halo) = ((90.0 * glow * sk.vein) as u8, (52.0 * glow * sk.vein) as u8);
        for off in [egui::vec2(0.0, 0.0), egui::vec2(1.0, 0.0), egui::vec2(-1.0, 0.0), egui::vec2(0.0, 1.0), egui::vec2(0.0, -1.0)] {
            let a = if off == egui::vec2(0.0, 0.0) { core } else { halo };
            p.galley(pos + off, lay(alpha_col(g, a)), ink);
        }
    }
    p.galley(pos + egui::vec2(0.7, 0.8), lay(Color32::from_black_alpha(sk.engrave_shadow)), ink); // pressed into the rock
    p.galley(pos + egui::vec2(-0.6, -0.7), lay(Color32::from_white_alpha(sk.engrave_lip)), ink);  // the lit upper lip
    p.galley(pos, lay(ink), ink); // the inked face
}

/// A button carved from the active material: grain shows through a beveled inset with a specular sheen that
/// tracks the sun and brightens on hover. Returns the click `Response`.
pub fn carved_button(ui: &mut egui::Ui, text: &str) -> Response { carved_button_impl(ui, text, None) }

/// Like [`carved_button`] but tinted toward `accent` for a primary/lively action (the sheen borrows it too).
pub fn carved_button_tinted(ui: &mut egui::Ui, text: &str, accent: Color32) -> Response {
    carved_button_impl(ui, text, Some(accent))
}

fn carved_button_impl(ui: &mut egui::Ui, text: &str, accent: Option<Color32>) -> Response {
    let t = ui.input(|i| i.time) as f32;
    let font = TextStyle::Button.resolve(ui.style());
    let pad = egui::vec2(13.0, 7.0);
    let measure = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), INK());
    let size = measure.size() + pad * 2.0;
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let hov = ui.ctx().animate_bool(resp.id, resp.hovered());
        let down = resp.is_pointer_button_down_on();
        let r2 = if down { rect.shrink(1.0) } else { rect };
        paint_carved(ui.painter(), r2, t, Carved { hov, down, enabled: true, accent, sheen: 1.0 });
        let ink = lerp_col(INK(), accent.unwrap_or_else(AMBER_INK), hov * 0.6);
        // engraved into the stone; accented buttons light up as glowing mineral veins on hover
        let glow = if accent.is_some() { 0.25 + 0.75 * hov } else { 0.0 };
        carved_label(ui.painter(), r2.center(), text, font, ink, accent, glow);
    }
    resp
}

/// The accent that suits the current scene — warm amber for the wood/paper/marble libraries, cool teal for
/// the stone/fog scenes, biological green for Folding@home. Used for forefront highlights so a warm room
/// never shows a cold blue glow (and vice-versa).
pub fn accent() -> Color32 {
    let p = active();
    match p.name {
        "Mistfall" | "Slate" | "Atlantis" => p.cyan,
        "Folding@home" => p.green,
        _ => p.amber,
    }
}

pub fn lerp_col(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let (a, b) = (egui::Rgba::from(a), egui::Rgba::from(b));
    Color32::from(a * (1.0 - t) + b * t)
}

/// Adwaita-style toggle switch: a pill track with a knob that slides as it animates
/// between off/on. Flips `*on` on click; returns true if it changed this frame.
pub fn switch(ui: &mut egui::Ui, on: &mut bool, accent: Color32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(34.0, 18.0), Sense::click());
    let mut changed = false;
    if resp.clicked() { *on = !*on; changed = true; }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let p = ui.painter();
    p.rect_filled(rect, r(9), lerp_col(PANEL2(), accent, t));
    p.rect_stroke(rect, r(9), Stroke::new(1.0, lerp_col(LINE(), accent, t)), StrokeKind::Inside);
    let x = egui::lerp((rect.left() + 9.0)..=(rect.right() - 9.0), t);
    p.circle_filled(Pos2::new(x, rect.center().y), 6.5, lerp_col(INK(), Color32::WHITE, t));
    changed
}

/// A labelled switch row: text on the left, switch on the right, full row clickable.
pub fn switch_row(ui: &mut egui::Ui, label: &str, on: &mut bool, accent: Color32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(11.0).color(if *on { INK() } else { DIM() }));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            changed = switch(ui, on, accent);
        });
    });
    changed
}

/// A themed spinner: dots around a ring with a rotating brightness trail. Keeps the
/// frame repainting while visible (so it actually spins).
pub fn spinner(ui: &mut egui::Ui, size: f32, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    ui.ctx().request_repaint();
    let time = ui.ctx().input(|i| i.time) as f32;
    let (cc, rad, n) = (rect.center(), size * 0.36, 8usize);
    for k in 0..n {
        let ang = std::f32::consts::TAU * (k as f32 / n as f32);
        let phase = (time * 1.4 - k as f32 / n as f32).rem_euclid(1.0);
        let a = 0.15 + 0.85 * (1.0 - phase);
        let pos = Pos2::new(cc.x + rad * ang.cos(), cc.y + rad * ang.sin());
        ui.painter().circle_filled(pos, size * 0.085, accent.gamma_multiply(a));
    }
}

/// A pulsing dot (e.g. a "thinking"/streaming indicator), brightness eased by a sine.
pub fn pulse_dot(ui: &mut egui::Ui, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
    ui.ctx().request_repaint();
    let time = ui.ctx().input(|i| i.time) as f32;
    let a = 0.35 + 0.65 * (0.5 + 0.5 * (time * 3.0).sin());
    ui.painter().circle_filled(rect.center(), 3.5, accent.gamma_multiply(a));
}

/// A prominent action button whose fill, border, and ink ease toward `accent` on hover and
/// inset slightly on press — animated, unlike egui's instant state switch. Disabled buttons
/// read faint and don't react. Returns the Response (`.clicked()` etc.).
pub fn action_button(ui: &mut egui::Ui, label: &str, accent: Color32, enabled: bool) -> Response {
    let t = ui.input(|i| i.time) as f32;
    let font = FontId::new(11.0, FontFamily::Monospace);
    let pad = egui::vec2(15.0, 7.0);
    let measure = ui.painter().layout_no_wrap(label.to_string(), font.clone(), INK());
    let size = measure.size() + pad * 2.0;
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if ui.is_rect_visible(rect) {
        let hov = if enabled { ui.ctx().animate_bool(resp.id, resp.hovered()) } else { 0.0 };
        let down = enabled && resp.is_pointer_button_down_on();
        let r2 = if down { rect.shrink(1.0) } else { rect };
        paint_carved(ui.painter(), r2, t, Carved { hov, down, enabled, accent: Some(accent), sheen: 1.0 });
        let ink = if enabled { lerp_col(INK(), AMBER_INK(), hov) } else { FAINT() };
        // the primary action reads as a glowing mineral vein — faint always, brighter on hover
        let glow = if enabled { 0.4 + 0.6 * hov } else { 0.0 };
        carved_label(ui.painter(), r2.center(), label, font, ink, Some(accent), glow);
    }
    resp
}

/// A usage bar whose fill *glides* to its target fraction (animated), keyed by `id`.
pub fn usage_bar_anim(ui: &mut egui::Ui, id: impl std::hash::Hash, width: f32, frac: f32, accent: Color32) {
    let aid = egui::Id::new("ubar").with(id);
    let shown = ui.ctx().animate_value_with_time(aid, frac.clamp(0.0, 1.0), 0.35);
    metric_track(ui, width, 14.0, shown, None, None, None, accent, frac >= 1.0);
}

/// A toast: transient text that fades in, holds, then fades out (Adwaita-style).
struct Toast { text: String, accent: Color32, born: f64 }
/// A small bottom-centre toast stack. Push on events; call `ui` each frame.
#[derive(Default)]
pub struct Toaster { items: Vec<Toast> }
impl Toaster {
    pub fn push(&mut self, ctx: &Context, text: impl Into<String>, accent: Color32) {
        let born = ctx.input(|i| i.time);
        self.items.push(Toast { text: text.into(), accent, born });
    }
    pub fn ui(&mut self, ctx: &Context) {
        const HOLD: f64 = 3.0;
        const FIN: f64 = 0.22;
        const FOUT: f64 = 0.5;
        let now = ctx.input(|i| i.time);
        self.items.retain(|t| now - t.born < HOLD + FOUT);
        if self.items.is_empty() { return; }
        ctx.request_repaint();
        egui::Area::new("logscope_toaster".into())
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -26.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for t in &self.items {
                    let age = now - t.born;
                    let a = if age < FIN { (age / FIN) as f32 }
                        else if age > HOLD { (1.0 - (age - HOLD) / FOUT).clamp(0.0, 1.0) as f32 }
                        else { 1.0 };
                    Frame {
                        inner_margin: Margin::symmetric(15, 11),
                        // clearer separation between stacked toasts so a burst doesn't read as one slab
                        outer_margin: Margin { bottom: 10, ..Default::default() },
                        fill: PANEL2().gamma_multiply(a),
                        stroke: Stroke::new(1.0, t.accent.gamma_multiply(0.7 * a)),
                        corner_radius: r(10),
                        shadow: glow(t.accent, 0.4 * a),
                        ..Default::default()
                    }
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.painter(); // ensure layout
                            // accent bar height tracks the text cap-height; centred by the row layout
                            let (bar, _) = ui.allocate_exact_size(egui::vec2(3.0, 13.0), Sense::hover());
                            ui.painter().rect_filled(bar, r(1), t.accent.gamma_multiply(a));
                            ui.add_space(9.0);
                            ui.label(RichText::new(&t.text).size(11.5).color(INK().gamma_multiply(a)));
                        });
                        // breathing room before the lifetime bar so it doesn't crowd the text baseline
                        ui.add_space(7.0);
                        // thin lifetime bar that depletes left→right over the hold window
                        let remain = (1.0 - ((age) / (HOLD + FOUT)) as f32).clamp(0.0, 1.0);
                        let full = ui.min_rect().width();
                        let (line, _) = ui.allocate_exact_size(egui::vec2(full, 2.0), Sense::hover());
                        let mut fr = line; fr.set_width(full * remain);
                        ui.painter().rect_filled(fr, r(1), t.accent.gamma_multiply(0.5 * a));
                    });
                }
            });
    }
}

/// Apply the active palette to egui's global Style/Visuals. Call once at startup and again after any
/// `set_palette` so egui's own widgets (scrollbars, menus, text selection) match the chosen theme.
pub fn install(ctx: &Context) {
    let pal = active();
    let mut style = (*ctx.global_style()).clone();
    let mut v = if pal.dark { Visuals::dark() } else { Visuals::light() };
    v.override_text_color = Some(INK());
    v.panel_fill = BG();
    v.window_fill = PANEL();
    v.window_stroke = Stroke::new(1.0, LINE());
    v.window_corner_radius = r(12);
    v.menu_corner_radius = r(10);
    v.extreme_bg_color = TRACK_BG();
    v.faint_bg_color = PANEL2();
    v.hyperlink_color = CYAN();
    v.warn_fg_color = AMBER();
    v.error_fg_color = RED();
    // selection reads cyan (status colour), distinct from the amber action accent.
    v.selection.bg_fill = CYAN().gamma_multiply(0.22);
    v.selection.stroke = Stroke::new(1.0, CYAN());

    // Idle panels read as the card colour; hover glows amber; active flips to the
    // amber "button" look. Mild rounding keeps the chrome consistent with cards.
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive] {
        w.bg_fill = PANEL();
        w.weak_bg_fill = PANEL2();
        w.bg_stroke = Stroke::new(1.0, LINE());
        w.corner_radius = r(6);
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, INK());
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, DIM());
    v.widgets.hovered.bg_fill = AMBER_INK();
    v.widgets.hovered.weak_bg_fill = AMBER_INK();
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, AMBER());
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, AMBER_SOFT());
    v.widgets.hovered.corner_radius = r(6);
    v.widgets.active.bg_fill = AMBER();
    v.widgets.active.weak_bg_fill = AMBER();
    v.widgets.active.bg_stroke = Stroke::new(1.0, AMBER());
    v.widgets.active.fg_stroke = Stroke::new(1.0, AMBER_INK());
    v.widgets.active.corner_radius = r(6);
    style.visuals = v;

    // Monospace everywhere, sized to echo the web's 13px-base ramp.
    let m = FontFamily::Monospace;
    style.text_styles = [
        (TextStyle::Heading, FontId::new(15.0, m.clone())),
        (TextStyle::Body, FontId::new(12.0, m.clone())),
        (TextStyle::Monospace, FontId::new(11.5, m.clone())),
        (TextStyle::Button, FontId::new(11.0, m.clone())),
        (TextStyle::Small, FontId::new(9.5, m)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(9.0, 7.0);
    style.spacing.button_padding = egui::vec2(11.0, 6.0);
    style.spacing.interact_size.y = 24.0;
    style.spacing.menu_margin = Margin::same(8);
    style.spacing.window_margin = Margin::same(10);
    ctx.set_global_style(style);
}

/// Switch palette and re-skin in one call (for the theme picker).
pub fn install_palette(ctx: &Context, p: Palette) { set_palette(p); install(ctx); }

// ── spacing scale + fade/transition helpers ──────────────────────────────────

/// Padding scale (px) — use these instead of magic numbers so spacing stays consistent.
pub const PAD_XS: i8 = 4;
pub const PAD_SM: i8 = 8;
pub const PAD_MD: i8 = 12;
pub const PAD_LG: i8 = 16;

/// Render `add` at opacity `alpha` (0..1) in its own scope — the building block for fades.
pub fn fade(ui: &mut egui::Ui, alpha: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.scope(|ui| { ui.set_opacity(alpha.clamp(0.0, 1.0)); add(ui); });
}

/// Eased 0→1 ramp over `secs` since `since` (an `Instant`), for fade-/slide-in on appear.
pub fn appear(since: std::time::Instant, secs: f32) -> f32 {
    let t = (since.elapsed().as_secs_f32() / secs.max(1e-3)).clamp(0.0, 1.0);
    // smoothstep for a softer landing than linear
    t * t * (3.0 - 2.0 * t)
}

/// A soft *coloured* glow (bloom around the element, not a drop shadow).
/// `intensity` ∈ [0,1] scales blur + alpha — animate it for hover states.
pub fn glow(accent: Color32, intensity: f32) -> egui::epaint::Shadow {
    let i = intensity.clamp(0.0, 1.0);
    egui::epaint::Shadow {
        offset: [0, 0],
        blur: (12.0 + 10.0 * i) as u8,
        spread: 1,
        color: accent.gamma_multiply(0.30 * i),
    }
}

/// Analytics card: rounded panel, hairline, soft accent glow, accent lead bar +
/// dim title. Mirrors `unendlich-ui::card`.
pub fn card(ui: &mut egui::Ui, accent: Color32, title: Option<&str>, body: impl FnOnce(&mut egui::Ui)) {
    Frame {
        inner_margin: Margin::same(14),
        outer_margin: Margin { bottom: 12, ..Default::default() },
        fill: PANEL(),
        stroke: Stroke::new(1.0, LINE()),
        corner_radius: r(12),
        shadow: glow(accent, 0.45),
        ..Default::default()
    }
    .show(ui, |ui| {
        if let Some(t) = title {
            ui.horizontal(|ui| {
                let (bar, _) = ui.allocate_exact_size(egui::vec2(3.0, 13.0), Sense::hover());
                ui.painter().rect_filled(bar, r(1), accent);
                ui.add_space(7.0);
                ui.add(Label::new(RichText::new(t).color(DIM()).size(11.0)).wrap_mode(TextWrapMode::Extend));
            });
            ui.add_space(8.0);
        }
        body(ui);
    });
}

/// Dashboard `.panel`: a thicker (5px) lead bar + UPPERCASE title and an optional
/// right-aligned copy button that drops `copy` onto the clipboard.
#[allow(dead_code)]
/// A small etched rune glyph — a few angular strokes with a soft glow, in the scene's accent. Decorative;
/// echoes the glowing glyphs carved into the stone in the reference frames. `glow` (0..1) breathes the light.
fn rune_mark(p: &egui::Painter, c: Pos2, s: f32, col: Color32, seed: u32, glow: f32) {
    // a tiny deterministic alphabet of strokes so each corner reads as a different mark
    let strokes = [
        [(-0.5, -0.6), (0.0, -0.6)], [(0.0, -0.7), (0.0, 0.7)], [(0.0, -0.2), (0.45, -0.5)],
        [(0.0, 0.25), (0.4, 0.5)], [(-0.45, 0.55), (0.0, 0.3)], [(-0.4, -0.4), (0.0, 0.0)],
    ];
    let n = 3 + (seed % 3) as usize;
    let halo = alpha_col(col, (44.0 * glow) as u8);
    let core = alpha_col(col, (150.0 * glow) as u8);
    for k in 0..n {
        let seg = strokes[(seed as usize + k) % strokes.len()];
        let a = c + egui::vec2(seg[0].0 * s, seg[0].1 * s);
        let b = c + egui::vec2(seg[1].0 * s, seg[1].1 * s);
        p.line_segment([a, b], Stroke::new(2.6, halo));
        p.line_segment([a, b], Stroke::new(1.0, core));
    }
}

/// Dress `rect` as a carved-stone FRAME: a double beveled relief lip (lit top-left, shadowed bottom-right),
/// the active material's grain scattered along the border band, and faint breathing runes in the corners.
/// Painted only in the edge band (the panel's inner margin), so it never draws over content. Theme-adaptive.
fn carved_frame(p: &egui::Painter, rect: egui::Rect, t: f32, accent: Color32) {
    let pal = active();
    let sk = skin();
    let lit = Color32::from_white_alpha(sk.bevel_lit);
    let shadow = Color32::from_black_alpha(sk.bevel_shadow);
    for inset in [0.8_f32, 2.6] {
        let q = rect.shrink(inset);
        p.line_segment([q.left_bottom(), q.left_top()], Stroke::new(1.0, lit));
        p.line_segment([q.left_top(), q.right_top()], Stroke::new(1.0, lit));
        p.line_segment([q.right_top(), q.right_bottom()], Stroke::new(1.0, shadow));
        p.line_segment([q.left_bottom(), q.right_bottom()], Stroke::new(1.0, shadow));
    }
    // grain along the top + bottom border bands so the frame reads as stone, not a drawn line
    if pal.material != Material::None {
        let band = 9.0;
        let cols = (rect.width() / 18.0).clamp(6.0, 40.0) as usize;
        for ix in 0..cols {
            let fx = rect.left() + rect.width() * (ix as f32 + 0.5) / cols as f32;
            for fy in [rect.top() + band * 0.5, rect.bottom() - band * 0.5] {
                let g = material_grain(pal.material, fx, fy);
                let a = (g.abs() * 22.0).min(22.0) as u8;
                let col = if g >= 0.0 { Color32::from_white_alpha(a) } else { Color32::from_black_alpha(a) };
                p.circle_filled(egui::pos2(fx, fy), 1.1, col);
            }
        }
    }
    // glowing runes at the corners — only on panels big enough to carry them, gently breathing
    if rect.width() > 90.0 && rect.height() > 70.0 {
        let glow = 0.45 + 0.30 * (0.5 + 0.5 * (t * 0.7).sin());
        let m = 13.0;
        rune_mark(p, rect.left_top() + egui::vec2(m, m), 6.0, accent, 1, glow);
        rune_mark(p, rect.right_top() + egui::vec2(-m, m), 6.0, accent, 4, glow);
        rune_mark(p, rect.left_bottom() + egui::vec2(m, -m), 6.0, accent, 2, glow);
        rune_mark(p, rect.right_bottom() + egui::vec2(-m, -m), 6.0, accent, 7, glow);
    }
}

/// Dress `rect` as a carved TROUGH — relief inverted from a raised slab (shadowed top/left lip, lit
/// bottom/right floor) plus a soft inner cast-shadow under the top edge, so it reads as pressed INTO the
/// stone. For the composer and other recessed inputs. Theme-adaptive grain along the edges; no runes
/// (inputs stay calm). Painted in the margin band after layout, so it never overdraws the text.
pub fn carved_trough(p: &egui::Painter, rect: egui::Rect, _t: f32) {
    let pal = active();
    let sk = skin();
    let shadow = Color32::from_black_alpha(sk.bevel_shadow);
    let lit = Color32::from_white_alpha(sk.bevel_lit);
    // inverted bevel: shadow on the top/left lip you look over, light on the bottom/right floor
    for inset in [0.8_f32, 2.6] {
        let q = rect.shrink(inset);
        p.line_segment([q.left_top(), q.right_top()], Stroke::new(1.0, shadow));
        p.line_segment([q.left_bottom(), q.left_top()], Stroke::new(1.0, shadow));
        p.line_segment([q.right_top(), q.right_bottom()], Stroke::new(1.0, lit));
        p.line_segment([q.left_bottom(), q.right_bottom()], Stroke::new(1.0, lit));
    }
    // soft inner cast-shadow band under the top lip — sells the depth of the recess
    let q = rect.shrink(2.6);
    for i in 0..5 {
        let a = (34.0 * (1.0 - i as f32 / 5.0)) as u8;
        let y = q.top() + i as f32 * 1.5;
        p.line_segment([egui::pos2(q.left(), y), egui::pos2(q.right(), y)], Stroke::new(1.5, Color32::from_black_alpha(a)));
    }
    // material grain along the top + bottom edge bands so the recess reads as stone (not over the text)
    if pal.material != Material::None {
        let band = 8.0;
        let cols = (rect.width() / 22.0).clamp(6.0, 48.0) as usize;
        for ix in 0..cols {
            let fx = rect.left() + rect.width() * (ix as f32 + 0.5) / cols as f32;
            for fy in [rect.top() + band, rect.bottom() - band * 0.5] {
                let g = material_grain(pal.material, fx, fy);
                let aa = (g.abs() * 16.0).min(16.0) as u8;
                let col = if g >= 0.0 { Color32::from_white_alpha(aa) } else { Color32::from_black_alpha(aa) };
                p.circle_filled(egui::pos2(fx, fy), 1.0, col);
            }
        }
    }
}

pub fn panel(ui: &mut egui::Ui, accent: Color32, title: &str, copy: Option<String>, body: impl FnOnce(&mut egui::Ui)) {
    let inner = Frame {
        inner_margin: Margin::same(16),
        outer_margin: Margin { bottom: 14, ..Default::default() },
        fill: alpha_col(PANEL(), 236),     // a touch translucent so the scene reads faintly through the stone
        stroke: Stroke::new(1.0, LINE()),
        corner_radius: r(12),
        shadow: glow(accent, 0.45),
        ..Default::default()
    }
    .show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (bar, _) = ui.allocate_exact_size(egui::vec2(5.0, 15.0), Sense::hover());
            ui.painter().rect_filled(bar, r(1), accent);
            ui.label(RichText::new(title.to_uppercase()).size(10.5).color(DIM()));
            if let Some(text) = copy {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(RichText::new("copy").size(9.5).color(DIM())).clicked() {
                        ui.ctx().copy_text(text.clone());
                    }
                });
            }
        });
        ui.add_space(9.0);
        body(ui);
    });
    // carve the stone frame into the margin band after the content is laid out
    let t = ui.input(|i| i.time) as f32;
    carved_frame(ui.painter(), inner.response.rect, t, accent);
}

/// Section header: an amber lead glyph (▌) + UPPERCASE dim title.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("\u{258c}").size(14.0).color(AMBER()));
        ui.label(RichText::new(title.to_uppercase()).size(10.5).color(DIM()));
    });
    ui.add_space(4.0);
}

/// Outline pill (transparent fill, coloured hairline + text).
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    Frame {
        inner_margin: Margin { left: 9, right: 9, top: 3, bottom: 3 },
        fill: Color32::TRANSPARENT,
        stroke: Stroke::new(1.0, color),
        corner_radius: r(10),
        ..Default::default()
    }
    .show(ui, |ui| {
        ui.label(RichText::new(text).size(10.5).strong().color(color));
    });
}

/// Measured pill: allocates exactly what it paints (never a doubled border),
/// stays on one line while it fits a row, wraps its text only if a single pill is
/// wider than the whole row. `filled` → tinted fill + coloured ink.
#[allow(dead_code)]
pub fn pill(ui: &mut egui::Ui, text: &str, color: Color32, filled: bool) -> Response {
    let t = ui.input(|i| i.time) as f32;
    let ink = if filled { color } else { DIM() };
    let font = FontId::monospace(10.0);
    let pad = egui::vec2(8.0, 3.0);
    let max_text_w = (ui.max_rect().width() - pad.x * 2.0 - 2.0).max(24.0);
    let single = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), ink);
    let galley = if single.size().x <= max_text_w { single } else { ui.painter().layout(text.to_owned(), font, ink, max_text_w) };
    let size = galley.size() + pad * 2.0;
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let hov = ui.ctx().animate_bool(resp.id, resp.hovered());
        paint_carved(ui.painter(), rect, t, Carved { hov, down: false, enabled: true,
            accent: if filled { Some(color) } else { None }, sheen: if filled { 0.7 } else { 0.3 } });
        ui.painter().galley(rect.min + pad, galley, ink);
    }
    resp
}

/// Compact stat tile (big value + UPPERCASE dim label) that fills its column.
pub fn stat_tile(ui: &mut egui::Ui, label: &str, value: &str, accent: Color32) {
    Frame {
        inner_margin: Margin::symmetric(14, 10),
        fill: PANEL2(),
        stroke: Stroke::new(1.0, LINE()),
        corner_radius: r(10),
        ..Default::default()
    }
    .show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical(|ui| {
            ui.label(RichText::new(value).color(accent).monospace().size(18.0).strong());
            ui.label(RichText::new(label.to_uppercase()).color(DIM()).size(9.5));
        });
    });
}

/// A run-list/sidebar item card. Idle reads as PANEL2; selected wears an amber
/// border + warm fill; hovering fades an amber border in. Returns true on click.
#[allow(dead_code)]
pub fn list_item(ui: &mut egui::Ui, selected: bool, body: impl FnOnce(&mut egui::Ui)) -> bool {
    let (fill, stroke) = if selected { (lerp_col(PANEL2(), AMBER(), 0.10), AMBER()) } else { (PANEL2(), LINE()) };
    let inner = Frame {
        inner_margin: Margin { left: 10, right: 10, top: 8, bottom: 8 },
        outer_margin: Margin { bottom: 5, ..Default::default() },
        fill,
        stroke: Stroke::new(1.0, stroke),
        corner_radius: r(8),
        ..Default::default()
    }
    .show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
    let resp = inner.response.interact(Sense::click());
    let t = ui.ctx().animate_bool(resp.id, resp.hovered());
    if t > 0.0 && !selected {
        ui.painter().rect_stroke(resp.rect, r(8), Stroke::new(1.0, AMBER().gamma_multiply(0.45 * t)), StrokeKind::Inside);
    }
    resp.clicked()
}

/// A small event/count pill (kind + ×count); reddens on `spike`.
#[allow(dead_code)]
pub fn event_pill(ui: &mut egui::Ui, kind: &str, count: usize, spike: bool) {
    let cc = if spike { RED() } else { AMBER() };
    Frame {
        inner_margin: Margin { left: 9, right: 9, top: 4, bottom: 4 },
        fill: TRACK_BG(),
        stroke: Stroke::new(1.0, LINE()),
        corner_radius: r(6),
        ..Default::default()
    }
    .show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            ui.label(RichText::new(kind).size(11.0).strong().color(cc));
            ui.label(RichText::new(format!("\u{00d7}{count}")).size(11.0).color(DIM()));
        });
    });
}

/// The signature metric track: a TRACK_BG bar with a `frac` fill (reddens when
/// `over`), an optional white p99 whisker, red max tick, and dashed budget line.
pub fn metric_track(
    ui: &mut egui::Ui, width: f32, height: f32, frac: f32,
    whisker: Option<f32>, maxf: Option<f32>, budget: Option<f32>, accent: Color32, over: bool,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width.max(48.0), height), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, r(4), TRACK_BG());
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        let mut fr = rect;
        fr.set_width(rect.width() * f);
        p.rect_filled(fr, r(4), if over { RED() } else { accent });
    }
    p.rect_stroke(rect, r(4), Stroke::new(1.0, LINE()), StrokeKind::Inside);
    let x_at = |v: f32| rect.left() + v.clamp(0.0, 1.0) * rect.width();
    if let Some(b) = budget {
        let x = x_at(b);
        p.line_segment([Pos2::new(x, rect.top() - 3.0), Pos2::new(x, rect.bottom() + 3.0)], Stroke::new(1.0, FAINT()));
    }
    if let Some(mx) = maxf {
        let x = x_at(mx);
        p.line_segment([Pos2::new(x, rect.top() + 3.0), Pos2::new(x, rect.bottom() - 3.0)], Stroke::new(2.0, MAXMARK()));
    }
    if let Some(wk) = whisker {
        let x = x_at(wk);
        p.line_segment([Pos2::new(x, rect.top() - 2.0), Pos2::new(x, rect.bottom() + 2.0)], Stroke::new(2.0, WHISKER()));
    }
}

/// A plain usage bar (fill only), reddening once it reaches full.
pub fn usage_bar(ui: &mut egui::Ui, width: f32, frac: f32, accent: Color32) {
    metric_track(ui, width, 14.0, frac, None, None, None, accent, frac >= 1.0);
}

/// Lay `count` items into a responsive grid that fills the width: as many equal
/// columns as fit at >= `min_col_w`, fewer (more rows) as space shrinks. Cells
/// stretch to share the width. `render(ui, i)` draws item `i`.
pub fn grid_fit(ui: &mut egui::Ui, min_col_w: f32, gap: f32, count: usize, mut render: impl FnMut(&mut egui::Ui, usize)) {
    if count == 0 { return; }
    let avail = ui.available_width().max(min_col_w);
    let cols = (((avail + gap) / (min_col_w + gap)).floor() as usize).clamp(1, count);
    let mut i = 0;
    while i < count {
        let row_n = cols.min(count - i);
        ui.columns(cols, |columns| {
            for cc in 0..row_n { render(&mut columns[cc], i + cc); }
        });
        i += row_n;
        ui.add_space(gap);
    }
}

#[cfg(test)]
mod day_night_tests {
    use super::*;

    #[test]
    fn daylight_curve_hits_night_and_noon() {
        assert_eq!(daylight01_at(0.0), 0.0);
        assert_eq!(daylight01_at(3.0), 0.0);
        assert!((daylight01_at(12.0) - 1.0).abs() < 1e-5);
        assert!(daylight01_at(9.0) > 0.5 && daylight01_at(9.0) < 1.0);
        assert_eq!(daylight01_at(22.0), 0.0);
    }

    #[test]
    fn light_palettes_darken_at_night_dark_palettes_lift_by_day() {
        for p in PALETTES {
            let night = day_night_adjust_at(*p, 0.0);
            let noon = day_night_adjust_at(*p, 1.0);
            let lum = rel_luminance;
            if p.dark {
                // vice versa: dark palettes gain a little daylight at noon, untouched at night
                assert!(lum(noon.bg) >= lum(p.bg), "{} should lift by day", p.name);
                assert_eq!(night.bg, p.bg, "{} unchanged at night", p.name);
            } else {
                // light palettes dim at night, untouched at noon
                assert!(lum(night.bg) < lum(p.bg), "{} should dim at night", p.name);
                assert_eq!(noon.bg, p.bg, "{} unchanged at noon", p.name);
                // the dim is bounded: dark ink still reads on the dimmed surface
                assert!(contrast_ratio(p.ink, night.bg) >= 4.5, "{} ink stays readable at night ({})", p.name, contrast_ratio(p.ink, night.bg));
            }
        }
    }

    #[test]
    fn contrast_helpers_and_md_colors_reach_targets_on_every_palette() {
        assert!((contrast_ratio(Color32::WHITE, Color32::BLACK) - 21.0).abs() < 0.1);
        assert_eq!(contrast_ratio(Color32::WHITE, Color32::WHITE), 1.0);
        for p in PALETTES {
            for day in [0.0f32, 0.5, 1.0] {
                let q = day_night_adjust_at(*p, day);
                // a target is only demandable up to what the background can support: the
                // stronger pole's ratio is the ceiling; ensure_contrast must reach
                // min(target, ceiling)
                let demand = |fg: Color32, bg: Color32, target: f32, what: &str| {
                    let ceiling = contrast_ratio(Color32::WHITE, bg).max(contrast_ratio(Color32::BLACK, bg));
                    let got = contrast_ratio(ensure_contrast(fg, bg, target), bg);
                    assert!(got >= target.min(ceiling) - 0.1, "{} {}: got {got:.2}, target {target}, ceiling {ceiling:.2}", p.name, what);
                };
                // accent demand uses the HUE-PRESERVING adjuster (what inline code / links / colored runs
                // actually render through), verified on the DAY/NIGHT-ADJUSTED panel — the surface + curve
                // the earlier pass failed to check.
                let demand_accent = |fg: Color32, bg: Color32, target: f32, what: &str| {
                    let ceiling = contrast_ratio(Color32::WHITE, bg).max(contrast_ratio(Color32::BLACK, bg));
                    let got = contrast_ratio(ensure_contrast_accent(fg, bg, target), bg);
                    assert!(got >= target.min(ceiling) - 0.2, "{} {}: got {got:.2}, target {target}, ceiling {ceiling:.2}", p.name, what);
                };
                demand(q.ink, q.bg, 4.5, "ink");
                demand(q.cyan, q.bg, 4.5, "cyan (neon-fade fix)");
                // Inline `code`, links, and colored runs render ON THE PANEL (message frames) and through the
                // hue-preserving adjuster, across the full day/night brightness curve.
                demand_accent(q.cyan, q.panel, 4.5, "cyan on panel (inline code)");
                demand_accent(q.violet, q.panel, 4.5, "violet on panel (colored run)");
                demand_accent(q.amber, q.panel, 3.0, "amber on panel (quote accent)");
                demand(q.ink, q.panel, 4.5, "ink on panel (md body)");
                // **bold** on the panel: no bold font weight exists in this pipeline, so the
                // 7.0-vs-4.5 contrast differential IS the emphasis. Two guarantees: the strong
                // floor holds (ceiling-capped), and bold NEVER lands lighter than body ink.
                demand(q.ink, q.panel, 7.0, "ink on panel STRONG (md bold)");
                let body_got = contrast_ratio(ensure_contrast(q.ink, q.panel, 4.5), q.panel);
                let strong_got = contrast_ratio(ensure_contrast(q.ink, q.panel, 7.0), q.panel);
                assert!(strong_got >= body_got - 0.05,
                    "{} bold ({strong_got:.2}) must not be lighter than body ({body_got:.2})", p.name);
                demand(q.dim, q.bg, 4.5, "dim (md body — raised from 4.0)");
                demand(q.amber, q.bg, 3.0, "quote accent");
                demand(q.ink, q.panel2, 4.5, "ink on panel2");
                // dim/faint are also used directly on panel surfaces (bullets, labels); they must clear the
                // body floor (dim) and the large-text floor (faint) there too, per the contrast pass over
                // the palettes. Only assert on non-inverted palettes (dark ink on light panel is a separate
                // regime the palette author tunes by hand).
                if rel_luminance(q.ink) > rel_luminance(q.panel) {
                    demand(q.dim, q.panel, 4.5, "dim on panel (bullets)");
                    demand(q.faint, q.panel, 3.0, "faint on panel (labels)");
                }
            }
        }
    }

    #[test]
    fn ensure_contrast_moves_minimally_and_keeps_passing_colors() {
        let bg = Color32::from_rgb(0xf6, 0xf3, 0xea); // paper
        let ink = Color32::from_rgb(0x22, 0x1e, 0x18);
        assert_eq!(ensure_contrast(ink, bg, 6.0), ink); // already passing: untouched
        let neon = Color32::from_rgb(0x57, 0xd6, 0xc9); // the fading cyan
        let fixed = ensure_contrast(neon, bg, 4.5);
        assert!(contrast_ratio(fixed, bg) >= 4.5);
        assert!(rel_luminance(fixed) < rel_luminance(neon), "on paper the fix darkens");
    }

    #[test]
    fn raw_palette_colours_are_legible_on_the_panel() {
        // Most of the app uses theme::INK()/DIM()/FAINT()/CYAN() straight against a panel, WITHOUT the
        // ensure_contrast adjuster the markdown layer goes through — so these raw pairs are what a reader
        // actually sees nearly everywhere. Measured before the fix: 87 of 190 under 4.5:1, the worst
        // being Yellow Brick Road INK on PANEL at 1.24:1, which is body text you cannot read.
        //
        // Tiered targets rather than one number: demanding 4.5 of a deliberately-faint hint would flatten
        // the hierarchy into a wall of equally-loud text, which is its own readability problem.
        let mut worst: Vec<(String, f32)> = Vec::new();
        for p in PALETTES {
            let q = day_night_adjust_at(*p, 0.5);
            for (name, fg) in [("ink", q.ink), ("dim", q.dim), ("faint", q.faint),
                               ("cyan", q.cyan), ("amber", q.amber)] {
                // PANEL only: bg is the window backdrop and carries almost no reading text, and several
                // palettes cannot satisfy both surfaces with one colour (see day_night_adjust_at).
                let target = match name { "faint" => 3.0, "dim" => 4.0, _ => 4.5 };
                let r = contrast_ratio(fg, q.panel);
                if r < target - 0.05 { worst.push((format!("{} {}/panel (want {target})", p.name, name), r)); }
            }
        }
        worst.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let shown: Vec<String> = worst.iter().take(8).map(|(k, r)| format!("{k} = {r:.2}")).collect();
        assert!(worst.is_empty(), "{} palette/role pairs are illegible on their panel:\n  {}",
            worst.len(), shown.join("\n  "));
    }

    #[test]
    fn status_is_never_colour_alone() {
        // Red/green is the most common colour-vision deficiency, and several palettes render success and
        // failure at nearly identical luminance — Reading Room had them EXACTLY equal, so a deuteranope
        // sees two identical grey dots.
        //
        // Separating the luminances in the palette was tried and reverted: lightening a status colour and
        // then restoring its 3:1 panel contrast are opposing moves that cancel. Colour cannot fix this
        // everywhere, so the answer is a mark that does not depend on hue at all.
        assert_ne!(status_mark(true), status_mark(false));
        // and the in-progress state is its own glyph, not a faded tick, which would read as "done, weakly"
        assert_eq!(status_mark3(None), "…");
        assert_eq!(status_mark3(Some(true)), status_mark(true));
        assert_ne!(status_mark3(None), status_mark3(Some(false)));

        // Record how many palettes actually need it, so the reason this helper exists stays visible.
        let mut close = 0;
        for p in PALETTES {
            let q = day_night_adjust_at(*p, 0.5);
            let (lg, lr) = (rel_luminance(q.green), rel_luminance(q.red));
            if (lg.max(lr) + 0.05) / (lg.min(lr) + 0.05) < 1.2 { close += 1; }
        }
        assert!(close > 0,
            "if no palette needed this any more, the helper could be reconsidered — currently {close} do");
    }


    #[test]
    fn measure_meaning_carrying_contrast() {
        // WCAG 2.1 SC 1.4.11 asks 3:1 for UI components and graphical objects — but it applies to things
        // you must PERCEIVE TO OPERATE or that CARRY MEANING, not to decorative layering.
        //
        // A first pass measured every non-text pair and found 86 of 114 under 3:1. Most of those should
        // NOT be changed: `panel2` against `panel` is a nested surface, and forcing 3:1 between them
        // would turn calm hierarchy into a checkerboard. Applying a rule where it does not belong is how
        // you damage a design while believing you are fixing it.
        //
        // So this measures only the pairs where the colour IS the information: a status indicator the
        // reader must distinguish, and the accent used for a focused/selected control.
        let mut bad: Vec<(String, f32)> = Vec::new();
        for p in PALETTES {
            let q = day_night_adjust_at(*p, 0.5);
            for (name, fg) in [("green", q.green), ("red", q.red), ("amber", q.amber), ("cyan", q.cyan)] {
                let r = contrast_ratio(fg, q.panel);
                if r < 3.0 { bad.push((format!("{} {name}/panel", p.name), r)); }
            }
        }
        bad.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let shown: Vec<String> = bad.iter().take(8).map(|(k, r)| format!("{k} = {r:.2}")).collect();
        assert!(bad.is_empty(),
            "{} status colours are indistinguishable from the surface they sit on:\n  {}",
            bad.len(), shown.join("\n  "));
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;

    #[test]
    fn variant_clamps_and_grades_are_distinct() {
        set_variant(0); assert_eq!(variant(), 1, "clamps below 1");
        set_variant(99); assert_eq!(variant(), 16, "clamps above 16");
        set_variant(5); assert_eq!(variant(), 5);
        // grade (locked atmosphere) still well-formed
        let (exp, fogm, bloom, thr, ambm, warm) = grade(LOCKED_GRADE);
        assert!(exp > 0.0 && fogm > 0.0 && bloom >= 0.0 && (0.0..=1.0).contains(&thr) && ambm > 0.0 && (-1.0..=1.0).contains(&warm));
        // detail sweep: texture resolution non-decreasing, tessellation strictly increasing, all distinct
        let mut prev = (0usize, 0.0f32);
        let mut seen = std::collections::HashSet::new();
        for v in 1..=16u8 {
            let (texsz, texscale, bump, tess) = detail(v);
            assert!(texsz >= prev.0, "texture resolution does not shrink as the tier rises");
            assert!(tess > prev.1, "tessellation rises every tier (got {tess} after {})", prev.1);
            assert!(texsz >= 64 && texscale > 0.0 && bump > 0.0);
            assert!(!variant_label(v).is_empty());
            prev = (texsz, tess);
            seen.insert((texsz, texscale.to_bits(), tess.to_bits()));
        }
        assert_eq!(seen.len(), 16, "all sixteen detail tiers are distinct");
        set_variant(1);
    }
}
