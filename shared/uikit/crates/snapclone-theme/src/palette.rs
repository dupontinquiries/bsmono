//! Color palettes — one struct, many themes.
//!
//! Each palette is a `const` value so it can be used in const contexts and
//! requires zero allocation. All colors are CSS hex strings (#rrggbb or
//! rgba(...)) compatible with Qt stylesheets, egui Color32::from_hex,
//! Tauri CSS variables, and Slint color literals.

use serde::{Deserialize, Serialize};

/// Identifies a palette in settings storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PaletteId {
    #[default]
    Dark,
    PitchBlack,
    Paper,
    Neon,
    Minimal,
    Modern,
    Earth,
    Pastels,
    Metallic,
    Night,
    System,
    Voice,
    Amber,
    Arena,
    /// Pale warm limestone — the one light palette in the mansion set.
    Travertine,
    /// Dark green-black marble with brass hardware.
    Obsidian,
    /// Indigo status board with a cyan-teal active state.
    Lattice,
}

impl PaletteId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark       => "dark",
            Self::PitchBlack => "pitch_black",
            Self::Paper      => "paper",
            Self::Neon       => "neon",
            Self::Minimal    => "minimal",
            Self::Modern     => "modern",
            Self::Earth      => "earth",
            Self::Pastels    => "pastels",
            Self::Metallic   => "metallic",
            Self::Night      => "night",
            Self::System     => "system",
            Self::Voice      => "voice",
            Self::Amber      => "amber",
            Self::Arena      => "arena",
            Self::Travertine => "travertine",
            Self::Obsidian   => "obsidian",
            Self::Lattice    => "lattice",
        }
    }
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Dark       => "Dark",
            Self::PitchBlack => "Pitch Black",
            Self::Paper      => "Paper",
            Self::Neon       => "Neon",
            Self::Minimal    => "Minimal",
            Self::Modern     => "Modern",
            Self::Earth      => "Earth",
            Self::Pastels    => "Pastels",
            Self::Metallic   => "Metallic",
            Self::Night      => "Night",
            Self::System     => "System",
            Self::Voice      => "Voice",
            Self::Amber      => "Amber",
            Self::Arena      => "Arena",
            Self::Travertine => "Travertine",
            Self::Obsidian   => "Obsidian",
            Self::Lattice    => "Lattice",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "pitch_black" => Self::PitchBlack,
            "paper"       => Self::Paper,
            "neon"        => Self::Neon,
            "minimal"     => Self::Minimal,
            "modern"      => Self::Modern,
            "earth"       => Self::Earth,
            "pastels"     => Self::Pastels,
            "metallic"    => Self::Metallic,
            "night"       => Self::Night,
            "system"      => Self::System,
            "voice"       => Self::Voice,
            "amber"       => Self::Amber,
            "arena"       => Self::Arena,
            "travertine"  => Self::Travertine,
            "obsidian"    => Self::Obsidian,
            "lattice"     => Self::Lattice,
            _             => Self::Dark,
        }
    }
    pub fn all() -> &'static [Self] {
        &[Self::Dark, Self::PitchBlack, Self::Paper, Self::Neon, Self::Minimal, Self::Modern,
          Self::Earth, Self::Pastels, Self::Metallic, Self::Night, Self::System, Self::Voice, Self::Amber, Self::Arena,
          Self::Travertine, Self::Obsidian, Self::Lattice]
    }
}

/// A complete color palette. All fields are CSS color strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Palette {
    pub id:   PaletteId,
    pub name: &'static str,

    // ── Backgrounds ──────────────────────────────────────────
    pub bg_deep:   &'static str,   // darkest / base surface
    pub bg_base:   &'static str,   // default page background
    pub bg_card:   &'static str,   // card / list item surface
    pub bg_raised: &'static str,   // elevated elements (sheets, dropdowns)
    pub bg_hover:  &'static str,   // hover / pressed state
    pub bg_scrim:  &'static str,   // modal overlay (rgba)

    // ── Text ─────────────────────────────────────────────────
    pub text_primary:   &'static str,
    pub text_secondary: &'static str,
    pub text_disabled:  &'static str,
    pub text_inverse:   &'static str,  // text on accent color

    // ── Accent / action ──────────────────────────────────────
    pub accent:      &'static str,
    pub accent_soft: &'static str,
    pub accent_dim:  &'static str,

    // ── Semantic ─────────────────────────────────────────────
    pub danger:  &'static str,
    pub success: &'static str,
    pub warning: &'static str,

    // ── Borders ──────────────────────────────────────────────
    pub border:       &'static str,
    pub border_focus: &'static str,

    // ── Gradient stops (CSS linear-gradient compatible) ───────
    /// Two-stop gradient for header/hero areas. [from, to]
    pub gradient_hero: [&'static str; 2],
    /// Tab bar gradient. [from, to]
    pub gradient_bar:  [&'static str; 2],
    /// Online badge glow color (for pulse animation)
    pub glow_online: &'static str,
}

// ────────────────────────────────────────────────────────────────────────────
// Palette definitions
// ────────────────────────────────────────────────────────────────────────────

/// Warm sepia-dark — the original SnapClone aesthetic.
/// Inspired by aged parchment and old photographs. Cosy, calm productivity.
pub const DARK: Palette = Palette {
    id: PaletteId::Dark, name: "Dark",
    bg_deep:   "#111110",
    bg_base:   "#1A1917",
    bg_card:   "#1A1917",
    bg_raised: "#242220",
    bg_hover:  "#2E2C29",
    bg_scrim:  "rgba(17,17,16,0.72)",
    text_primary:   "#EAE6E0",
    text_secondary: "#8A8480",
    text_disabled:  "#4C4945",
    text_inverse:   "#111110",
    accent:      "#C9B99A",
    accent_soft: "#E8DDD0",
    accent_dim:  "#7A6E60",
    danger:  "#BD5A44",
    success: "#5E9470",
    warning: "#C49A3C",
    border:       "#2C2A27",
    border_focus: "#C9B99A",
    gradient_hero: ["#1A1917", "#111110"],
    gradient_bar:  ["#1A1917", "#111110"],
    glow_online:   "#5E9470",
};

/// Tokyo-Night cool-blue dark — matches gitmind's Qt/QML surface (the egui apps' colour north-star).
pub const NIGHT: Palette = Palette {
    id: PaletteId::Night, name: "Night",
    bg_deep:   "#0c0f1a",
    bg_base:   "#0f1320",
    bg_card:   "#171c2b",
    bg_raised: "#2a3350",
    bg_hover:  "#3a4566",
    bg_scrim:  "rgba(12,15,26,0.72)",
    text_primary:   "#cdd6f4",
    text_secondary: "#8a93a6",
    text_disabled:  "#5b6477",
    text_inverse:   "#0c0f1a",
    accent:      "#7aa2f7",
    accent_soft: "#c9a3ff",
    accent_dim:  "#3a4566",
    danger:  "#f7768e",
    success: "#56d364",
    warning: "#f9e2af",
    border:       "#2a3350",
    border_focus: "#7aa2f7",
    gradient_hero: ["#171c2b", "#0c0f1a"],
    gradient_bar:  ["#171c2b", "#0f1320"],
    glow_online:   "#56d364",
};

/// Neutral minimal dark — the canonical "system utilities" theme (gitmind, rusty-board). Low chrome, a
/// restrained steel accent (no strong hue), text-forward. This is the "fully dark minimal" look, refined
/// off DARK but de-warmed so it reads as a functional tool rather than a styled app.
pub const SYSTEM: Palette = Palette {
    id: PaletteId::System, name: "System",
    bg_deep:   "#0f0f11",
    bg_base:   "#161619",
    bg_card:   "#161619",
    bg_raised: "#1e1e22",
    bg_hover:  "#26262b",
    bg_scrim:  "rgba(15,15,17,0.72)",
    text_primary:   "#e6e6e9",
    text_secondary: "#8e8e97",
    text_disabled:  "#4e4e57",
    text_inverse:   "#0f0f11",
    accent:      "#aab0bb",
    accent_soft: "#cdd2da",
    accent_dim:  "#5a5f68",
    danger:  "#c26356",
    success: "#6a9c7a",
    warning: "#c2a04e",
    border:       "#26262b",
    border_focus: "#aab0bb",
    gradient_hero: ["#161619", "#0f0f11"],
    gradient_bar:  ["#161619", "#0f0f11"],
    glow_online:   "#6a9c7a",
};

/// Terminal orange/green — matches voiceterm's Qt/QML surface (the egui apps' colour north-star).
/// Distinct from Night/System: voiceterm's own identity (a terminal), not a shared utility theme.
pub const VOICE: Palette = Palette {
    id: PaletteId::Voice, name: "Voice",
    bg_deep:   "#1c1c22",
    bg_base:   "#23232a",
    bg_card:   "#26262e",
    bg_raised: "#33333d",
    bg_hover:  "#3a3a44",
    bg_scrim:  "rgba(28,28,34,0.72)",
    text_primary:   "#e8e8ee",
    text_secondary: "#9a9aa6",
    text_disabled:  "#55555f",
    text_inverse:   "#1c1c22",
    accent:      "#e0552e",
    accent_soft: "#bfe3cf",
    accent_dim:  "#3a3a44",
    danger:  "#dc2626",
    success: "#7ddc7d",
    warning: "#e0a050",
    border:       "#3a3a44",
    border_focus: "#e0552e",
    gradient_hero: ["#26262e", "#1c1c22"],
    gradient_bar:  ["#26262e", "#23232a"],
    glow_online:   "#7ddc7d",
};

/// Warm gold/amber — matches bo-controller's Qt/QML surface (the egui apps' colour north-star).
/// bo-suite's own identity (a B&O-style ambient audio control panel), distinct from System/Night/Voice.
pub const AMBER: Palette = Palette {
    id: PaletteId::Amber, name: "Amber",
    bg_deep:   "#000000",
    bg_base:   "#1a1a1a",
    bg_card:   "#33291a",
    bg_raised: "#3a3a3a",
    bg_hover:  "#4a3f28",
    bg_scrim:  "rgba(0,0,0,0.72)",
    text_primary:   "#e8dcc0",
    text_secondary: "#b8a888",
    text_disabled:  "#6a6155",
    text_inverse:   "#1a1a1a",
    accent:      "#c8a04b",
    accent_soft: "#d4c89a",
    accent_dim:  "#4a3f28",
    danger:  "#8e4a3d",
    success: "#4a7c59",
    warning: "#c0a060",
    border:       "#3a3a3a",
    border_focus: "#c8a04b",
    gradient_hero: ["#33291a", "#1a1a1a"],
    gradient_bar:  ["#33291a", "#000000"],
    glow_online:   "#4a7c59",
};

/// Matches bot-arena's real GTK CSS (main.rs's .bot-* status classes: idle/provisioning/running/pass/
/// fail/culled/spawn cell colours) — the egui apps' colour north star. Distinct identity: a status-grid
/// tool, not a themed "brand" app, so the palette centres on the RUNNING blue as the active/interactive
/// colour with pass/fail/spawn mapped onto success/danger/warning.
pub const ARENA: Palette = Palette {
    id: PaletteId::Arena, name: "Arena",
    bg_deep:   "#1a1a1a",
    bg_base:   "#222222",
    bg_card:   "#2a2a2a",
    bg_raised: "#333333",
    bg_hover:  "#3a3a3a",
    bg_scrim:  "rgba(26,26,26,0.72)",
    text_primary:   "#e8e8e8",
    text_secondary: "#cfcfcf",
    text_disabled:  "#6a6a6a",
    text_inverse:   "#1a1a1a",
    accent:      "#1c5fa8",
    accent_soft: "#4d9de0",
    accent_dim:  "#3a3a3a",
    danger:  "#7a2f2f",
    success: "#1f7a3d",
    warning: "#8a7a1f",
    border:       "#3a3a3a",
    border_focus: "#1c5fa8",
    gradient_hero: ["#2a2a2a", "#1a1a1a"],
    gradient_bar:  ["#2a2a2a", "#222222"],
    glow_online:   "#1f7a3d",
};






/// OLED pitch-black — maximum contrast, minimum eye strain in dark rooms.
/// Pure #000000 base for OLED displays where black = zero power.
pub const PITCH_BLACK: Palette = Palette {
    id: PaletteId::PitchBlack, name: "Pitch Black",
    bg_deep:   "#000000",
    bg_base:   "#0A0A0A",
    bg_card:   "#111111",
    bg_raised: "#181818",
    bg_hover:  "#202020",
    bg_scrim:  "rgba(0,0,0,0.85)",
    text_primary:   "#FFFFFF",
    text_secondary: "#999999",
    text_disabled:  "#444444",
    text_inverse:   "#000000",
    accent:      "#E0C97A",  // gold — stands out on pure black
    accent_soft: "#F0E0A0",
    accent_dim:  "#806E30",
    danger:  "#FF4444",
    success: "#44CC44",
    warning: "#FFAA00",
    border:       "#1C1C1C",
    border_focus: "#E0C97A",
    gradient_hero: ["#0A0A0A", "#000000"],
    gradient_bar:  ["#0D0D0D", "#000000"],
    glow_online:   "#44CC44",
};

/// Paper — off-white notebook, ink typography, bright work environment.
/// Light theme for daytime use or users who prefer high ambient brightness.
pub const PAPER: Palette = Palette {
    id: PaletteId::Paper, name: "Paper",
    bg_deep:   "#F8F6F2",
    bg_base:   "#FDFCF9",
    bg_card:   "#F0EDE7",
    bg_raised: "#E8E4DC",
    bg_hover:  "#DDD9D0",
    bg_scrim:  "rgba(200,196,188,0.60)",
    text_primary:   "#1C1A16",
    text_secondary: "#6B6560",
    text_disabled:  "#B0ABA4",
    text_inverse:   "#FDFCF9",
    accent:      "#7C5C2E",  // warm brown ink
    accent_soft: "#A07840",
    accent_dim:  "#C4A870",
    danger:  "#B33A28",
    success: "#3D7A4E",
    warning: "#A8782A",
    border:       "#D4CFC6",
    border_focus: "#7C5C2E",
    gradient_hero: ["#FDFCF9", "#F0EDE7"],
    gradient_bar:  ["#F0EDE7", "#E8E4DC"],
    glow_online:   "#3D7A4E",
};

/// Neon — deep navy + electric cyan and magenta. Cyberpunk / synthwave.
/// High contrast, vivid colors, for users who like their UI to glow.
pub const NEON: Palette = Palette {
    id: PaletteId::Neon, name: "Neon",
    bg_deep:   "#05060F",
    bg_base:   "#0A0D1A",
    bg_card:   "#0F1222",
    bg_raised: "#151829",
    bg_hover:  "#1C2038",
    bg_scrim:  "rgba(5,6,15,0.80)",
    text_primary:   "#E0F0FF",
    text_secondary: "#7090B0",
    text_disabled:  "#304060",
    text_inverse:   "#05060F",
    accent:      "#00E5FF",  // electric cyan
    accent_soft: "#80F0FF",
    accent_dim:  "#006080",
    danger:  "#FF2080",      // hot magenta for danger
    success: "#00FF80",      // neon green
    warning: "#FFD700",
    border:       "#1A2040",
    border_focus: "#00E5FF",
    gradient_hero: ["#0A0D1A", "#050610"],
    gradient_bar:  ["#0F1222", "#08091A"],
    glow_online:   "#00FF80",
};

/// Minimal — near-white with slate accents. Clean, corporate, distraction-free.
/// For users who want the UI to disappear and the content to dominate.
pub const MINIMAL: Palette = Palette {
    id: PaletteId::Minimal, name: "Minimal",
    bg_deep:   "#F2F2F2",
    bg_base:   "#FAFAFA",
    bg_card:   "#EEEEEE",
    bg_raised: "#E5E5E5",
    bg_hover:  "#DCDCDC",
    bg_scrim:  "rgba(180,180,180,0.55)",
    text_primary:   "#18181B",
    text_secondary: "#71717A",
    text_disabled:  "#A1A1AA",
    text_inverse:   "#FAFAFA",
    accent:      "#3F3F46",  // near-black accent — very minimal
    accent_soft: "#52525B",
    accent_dim:  "#A1A1AA",
    danger:  "#DC2626",
    success: "#16A34A",
    warning: "#D97706",
    border:       "#D4D4D8",
    border_focus: "#3F3F46",
    gradient_hero: ["#FAFAFA", "#F0F0F0"],
    gradient_bar:  ["#F2F2F2", "#E8E8E8"],
    glow_online:   "#16A34A",
};

/// Modern — slate-dark with vivid indigo/blue accent. Material Design 3 feel.
/// Familiar to Android users; sharp, purposeful, energetic.
pub const MODERN: Palette = Palette {
    id: PaletteId::Modern, name: "Modern",
    bg_deep:   "#0F1117",
    bg_base:   "#161B22",
    bg_card:   "#1C2128",
    bg_raised: "#21262D",
    bg_hover:  "#2D333B",
    bg_scrim:  "rgba(15,17,23,0.75)",
    text_primary:   "#CDD9E5",
    text_secondary: "#768390",
    text_disabled:  "#444C56",
    text_inverse:   "#0F1117",
    accent:      "#58A6FF",  // GitHub-style blue
    accent_soft: "#8AC4FF",
    accent_dim:  "#1F6FEB",
    danger:  "#F85149",
    success: "#3FB950",
    warning: "#D29922",
    border:       "#30363D",
    border_focus: "#58A6FF",
    gradient_hero: ["#161B22", "#0D1117"],
    gradient_bar:  ["#161B22", "#0F1117"],
    glow_online:   "#3FB950",
};

/// Earth — rich dark soil tones with golden amber accent.
/// Warm and grounded; great for outdoor / nature apps.
pub const EARTH: Palette = Palette {
    id: PaletteId::Earth, name: "Earth",
    bg_deep:   "#1A1208",
    bg_base:   "#231A0C",
    bg_card:   "#2C2010",
    bg_raised: "#372818",
    bg_hover:  "#433222",
    bg_scrim:  "rgba(26,18,8,0.78)",
    text_primary:   "#F0E6CE",
    text_secondary: "#9C8B6A",
    text_disabled:  "#5A4A2E",
    text_inverse:   "#1A1208",
    accent:      "#8B6914",
    accent_soft: "#C49A3C",
    accent_dim:  "#5C420A",
    danger:  "#8B2A1A",
    success: "#4A8A3A",
    warning: "#B8820A",
    border:       "#332518",
    border_focus: "#8B6914",
    gradient_hero: ["#231A0C", "#1A1208"],
    gradient_bar:  ["#231A0C", "#1A1208"],
    glow_online:   "#4A8A3A",
};

/// Pastels — soft lilac and lavender, gentle and airy.
/// Light theme with dreamy purple tones and a warm violet accent.
pub const PASTELS: Palette = Palette {
    id: PaletteId::Pastels, name: "Pastels",
    bg_deep:   "#F5F0FF",
    bg_base:   "#FAF7FF",
    bg_card:   "#EDE5FF",
    bg_raised: "#E3D8FF",
    bg_hover:  "#D8CEFF",
    bg_scrim:  "rgba(220,210,240,0.60)",
    text_primary:   "#3D2B5C",
    text_secondary: "#7A6A99",
    text_disabled:  "#B8A8D4",
    text_inverse:   "#F5F0FF",
    accent:      "#A88FDB",
    accent_soft: "#C4AAFF",
    accent_dim:  "#7A5CB8",
    danger:  "#DB8FA8",
    success: "#8FDBAA",
    warning: "#DBBC8F",
    border:       "#CBBFF0",
    border_focus: "#A88FDB",
    gradient_hero: ["#FAF7FF", "#EDE5FF"],
    gradient_bar:  ["#EDE5FF", "#E3D8FF"],
    glow_online:   "#8FDBAA",
};

/// Metallic — deep gunmetal with cool steel-blue accent.
/// Polished, industrial, precise — like brushed aluminium.
pub const METALLIC: Palette = Palette {
    id: PaletteId::Metallic, name: "Metallic",
    bg_deep:   "#0D0D0F",
    bg_base:   "#141418",
    bg_card:   "#1C1C22",
    bg_raised: "#24242C",
    bg_hover:  "#2E2E38",
    bg_scrim:  "rgba(13,13,15,0.82)",
    text_primary:   "#D8DCE8",
    text_secondary: "#8890A8",
    text_disabled:  "#484E62",
    text_inverse:   "#0D0D0F",
    accent:      "#A0AACC",
    accent_soft: "#C8D0E8",
    accent_dim:  "#606888",
    danger:  "#CC5555",
    success: "#55AA88",
    warning: "#CCAA44",
    border:       "#2A2A36",
    border_focus: "#A0AACC",
    gradient_hero: ["#141418", "#0D0D0F"],
    gradient_bar:  ["#141418", "#0D0D0F"],
    glow_online:   "#55AA88",
};


// ── the mansion set ──────────────────────────────────────────────────────────
//
// Three palettes for the physical-UI surfaces, built to the brief: interactive
// stone in a chic mansion, expensive rather than merely dark.
//
// What makes a palette read as cheap, from looking at the ones here that do:
// pure black backgrounds (#000000 has no material — real stone in a lit room
// never reaches it), greys with no hue (a neutral #3a3a3a beside a warm accent
// reads as unfinished plastic), and a single accent doing every job so nothing
// has a hierarchy. Each of these three keeps a consistent hue through the whole
// background ramp, so the surfaces look like ONE material lit unevenly rather
// than several materials that happen to be similar values.
//
// Amber is the reference point — it already does the warm ramp
// (#1a1a1a → #33291a → #4a3f28) and that is why it reads better than its
// neighbours. These extend the idea rather than departing from it.

/// Travertine under low warm light: the Observatory reference's material.
/// Pale, dusty, gold-veined. The one LIGHT palette in the set, because a
/// mansion at midday is not a dark room and the physical UI should be able to
/// look like limestone rather than only like basalt.
pub const TRAVERTINE: Palette = Palette {
    id: PaletteId::Travertine, name: "Travertine",
    // A warm off-white ramp. Nothing here is neutral: every step carries the
    // same faint ochre, which is what stops it reading as "grey with a filter".
    bg_deep:   "#d9d0c2",
    bg_base:   "#e6ded1",
    bg_card:   "#efe8dc",
    bg_raised: "#f6f1e8",
    bg_hover:  "#e3d9c8",
    bg_scrim:  "rgba(58,50,38,0.38)",
    text_primary:   "#3a3226",
    text_secondary: "#6b6152",
    text_disabled:  "#a89e8d",
    text_inverse:   "#f6f1e8",
    // Old gold, not yellow. Desaturated enough to sit on pale stone without
    // vibrating against it.
    accent:      "#9a7b3f",
    accent_soft: "#c4ab74",
    accent_dim:  "#cfc4ae",
    danger:  "#9c4f3f",
    success: "#5d7a52",
    warning: "#b08a44",
    border:       "#cabfa9",
    border_focus: "#9a7b3f",
    gradient_hero: ["#f6f1e8", "#d9d0c2"],
    gradient_bar:  ["#e6ded1", "#cfc4ae"],
    glow_online:   "#5d7a52",
};

/// Polished dark marble with brass. The night counterpart to Travertine and the
/// closest to the "interactive stone" brief: deep green-black with a cool
/// undertone, lifted by brass rather than gold so the accent reads as hardware
/// (a handle, an inlay) instead of decoration.
pub const OBSIDIAN: Palette = Palette {
    id: PaletteId::Obsidian, name: "Obsidian",
    // Note the floor is #0d100f, NOT #000000. Stone in a lit room has a
    // minimum luminance; black holes in a panel are what make dark themes look
    // like a terminal rather than a surface.
    bg_deep:   "#0d100f",
    bg_base:   "#141917",
    bg_card:   "#1c2321",
    bg_raised: "#242c29",
    bg_hover:  "#2d3733",
    bg_scrim:  "rgba(13,16,15,0.74)",
    text_primary:   "#e4e6e0",
    text_secondary: "#a3a89f",
    text_disabled:  "#5e645c",
    text_inverse:   "#0d100f",
    accent:      "#b08d57",
    accent_soft: "#d8c49a",
    accent_dim:  "#3b342a",
    danger:  "#a05548",
    success: "#5f8a68",
    warning: "#c0994e",
    border:       "#2d3733",
    border_focus: "#b08d57",
    gradient_hero: ["#1c2321", "#0d100f"],
    gradient_bar:  ["#141917", "#0d100f"],
    glow_online:   "#5f8a68",
};

/// Distributed-compute status board: the Folding@home lineage the operator
/// named. Deep indigo ground with a saturated cyan-teal active state, kept
/// legible because the ramp stays blue rather than drifting to grey. Its job
/// is many small status cells that must be distinguishable at a glance, so
/// success/warning/danger are pushed further apart here than in the two stone
/// palettes above.
pub const LATTICE: Palette = Palette {
    id: PaletteId::Lattice, name: "Lattice",
    bg_deep:   "#0a0f1a",
    bg_base:   "#101828",
    bg_card:   "#16203a",
    bg_raised: "#1d2a49",
    bg_hover:  "#26365c",
    bg_scrim:  "rgba(10,15,26,0.76)",
    text_primary:   "#dde6f5",
    text_secondary: "#93a4c4",
    text_disabled:  "#55648a",
    text_inverse:   "#0a0f1a",
    accent:      "#3fb8c4",
    accent_soft: "#8fdce4",
    accent_dim:  "#1b3b47",
    danger:  "#c25b6a",
    success: "#4fb07a",
    warning: "#d9a441",
    border:       "#26365c",
    border_focus: "#3fb8c4",
    gradient_hero: ["#16203a", "#0a0f1a"],
    gradient_bar:  ["#101828", "#0a0f1a"],
    glow_online:   "#4fb07a",
};
