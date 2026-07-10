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
            _             => Self::Dark,
        }
    }
    pub fn all() -> &'static [Self] {
        &[Self::Dark, Self::PitchBlack, Self::Paper, Self::Neon, Self::Minimal, Self::Modern,
          Self::Earth, Self::Pastels, Self::Metallic, Self::Night]
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

