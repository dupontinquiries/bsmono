#pragma once
// ─── UI/Theme.hpp ─────────────────────────────────────────────────────────────
//
//  Runtime-switchable palette system.
//
//  Usage:
//    Theme::setPalette(Theme::PaletteId::Neon);
//    QApplication::instance()->setStyleSheet(Theme::globalStyleSheet());
//
//  The active palette is stored in QSettings under "ui/palette".
// ─────────────────────────────────────────────────────────────────────────────

#include <QString>
#include <QSettings>
#include <QGuiApplication>   // V32.30: viewport-relative sizing (vhPx/vwPx)
#include <QApplication>      // V32.32: qApp macro for applyGlobalStyleSheetIfChanged
#include <QScreen>           // V32.30: viewport-relative sizing
#include <string>
#include <array>

namespace SnapClone::UI::Theme {

// ── Palette IDs ──────────────────────────────────────────────────────────────
enum class PaletteId { Dark, PitchBlack, Paper, Neon, Minimal, Modern };

struct Palette {
    const char *id;
    const char *name;
    // Backgrounds
    const char *BG_DEEP;
    const char *BG_BASE;
    const char *BG_CARD;
    const char *BG_RAISED;
    const char *BG_HOVER;
    const char *BG_SCRIM;
    // Text
    const char *TEXT_PRIMARY;
    const char *TEXT_SECONDARY;
    const char *TEXT_DISABLED;
    const char *TEXT_INVERSE;
    // Accent
    const char *ACCENT;
    const char *ACCENT_SOFT;
    const char *ACCENT_DIM;
    // Semantic
    const char *DANGER;
    const char *SUCCESS;
    const char *WARNING;
    // Borders
    const char *BORDER;
    const char *BORDER_FOCUS;
    // Gradient stops [from, to]
    const char *GRAD_HERO_FROM;
    const char *GRAD_HERO_TO;
    const char *GRAD_BAR_FROM;
    const char *GRAD_BAR_TO;
    const char *GLOW_ONLINE;
};

// ── Palette definitions ───────────────────────────────────────────────────────

inline constexpr Palette kPalettes[] = {
    { "dark", "Dark",
      "#111110","#1A1917","#1A1917","#242220","#2E2C29","rgba(17,17,16,0.72)",
      "#EAE6E0","#8A8480","#4C4945","#111110",
      "#C9B99A","#E8DDD0","#7A6E60",
      "#BD5A44","#5E9470","#C49A3C",
      "#2C2A27","#C9B99A",
      "#1A1917","#111110","#1A1917","#111110","#5E9470" },

    { "pitch_black", "Pitch Black",
      "#000000","#0A0A0A","#111111","#181818","#202020","rgba(0,0,0,0.85)",
      "#FFFFFF","#999999","#444444","#000000",
      "#E0C97A","#F0E0A0","#806E30",
      "#FF4444","#44CC44","#FFAA00",
      "#1C1C1C","#E0C97A",
      "#0A0A0A","#000000","#0D0D0D","#000000","#44CC44" },

    { "paper", "Paper",
      "#F8F6F2","#FDFCF9","#F0EDE7","#E8E4DC","#DDD9D0","rgba(200,196,188,0.60)",
      "#1C1A16","#6B6560","#B0ABA4","#FDFCF9",
      "#7C5C2E","#A07840","#C4A870",
      "#B33A28","#3D7A4E","#A8782A",
      "#D4CFC6","#7C5C2E",
      "#FDFCF9","#F0EDE7","#F0EDE7","#E8E4DC","#3D7A4E" },

    { "neon", "Neon",
      "#05060F","#0A0D1A","#0F1222","#151829","#1C2038","rgba(5,6,15,0.80)",
      "#E0F0FF","#7090B0","#304060","#05060F",
      "#00E5FF","#80F0FF","#006080",
      "#FF2080","#00FF80","#FFD700",
      "#1A2040","#00E5FF",
      "#0A0D1A","#050610","#0F1222","#08091A","#00FF80" },

    { "minimal", "Minimal",
      "#F2F2F2","#FAFAFA","#EEEEEE","#E5E5E5","#DCDCDC","rgba(180,180,180,0.55)",
      "#18181B","#71717A","#A1A1AA","#FAFAFA",
      "#3F3F46","#52525B","#A1A1AA",
      "#DC2626","#16A34A","#D97706",
      "#D4D4D8","#3F3F46",
      "#FAFAFA","#F0F0F0","#F2F2F2","#E8E8E8","#16A34A" },

    { "modern", "Modern",
      "#0F1117","#161B22","#1C2128","#21262D","#2D333B","rgba(15,17,23,0.75)",
      "#CDD9E5","#768390","#444C56","#0F1117",
      "#58A6FF","#8AC4FF","#1F6FEB",
      "#F85149","#3FB950","#D29922",
      "#30363D","#58A6FF",
      "#161B22","#0D1117","#161B22","#0F1117","#3FB950" },

    { "earth", "Earth",
      "#1A1208","#231A0C","#2C2010","#372818","#433222","rgba(26,18,8,0.78)",
      "#F0E6CE","#9C8B6A","#5A4A2E","#1A1208",
      "#8B6914","#C49A3C","#5C420A",
      "#8B2A1A","#4A8A3A","#B8820A",
      "#332518","#8B6914",
      "#231A0C","#1A1208","#231A0C","#1A1208","#4A8A3A" },

    { "pastels", "Pastels",
      "#F5F0FF","#FAF7FF","#EDE5FF","#E3D8FF","#D8CEFF","rgba(220,210,240,0.60)",
      "#3D2B5C","#7A6A99","#B8A8D4","#F5F0FF",
      "#A88FDB","#C4AAFF","#7A5CB8",
      "#DB8FA8","#8FDBAA","#DBBC8F",
      "#CBBFF0","#A88FDB",
      "#FAF7FF","#EDE5FF","#EDE5FF","#E3D8FF","#8FDBAA" },

    { "metallic", "Metallic",
      "#0D0D0F","#141418","#1C1C22","#24242C","#2E2E38","rgba(13,13,15,0.82)",
      "#D8DCE8","#8890A8","#484E62","#0D0D0F",
      "#A0AACC","#C8D0E8","#606888",
      "#CC5555","#55AA88","#CCAA44",
      "#2A2A36","#A0AACC",
      "#141418","#0D0D0F","#141418","#0D0D0F","#55AA88" },

    // Pastels Dark — soft lavender night; dreamy but easy on the eyes in low light.
    { "pastels_dark", "Pastels Night",
      "#1A1520","#211A2A","#2A2038","#342848","#3E3056","rgba(26,21,32,0.80)",
      "#EDE0FF","#9A88BB","#5A4A7A","#1A1520",
      "#B899FF","#D4BAFF","#8860CC",
      "#FF8AAF","#88DDAA","#DDBB88",
      "#322545","#B899FF",
      "#211A2A","#1A1520","#211A2A","#1A1520","#88DDAA" },

    // Minimal Dark — almost-black with single cool-grey accent.
    // Like the light Minimal but inverted: near-white text on near-black ground.
    // For users who want maximum contrast with zero colour distraction.
    { "minimal_dark", "Minimal Dark",
      // True grayscale — every color is R=G=B. No tints, no warmth, no
      // accents. Suitable for users who want a perfectly neutral canvas.
      // Status colors are distinguished by brightness only:
      //   ERROR=#FFFFFF (brightest white, alarming)
      //   SUCCESS=#B0B0B0 (mid-bright gray)
      //   WARNING=#888888 (dim gray)
      "#0E0E0E","#141414","#1C1C1C","#242424","#2C2C2C","rgba(14,14,14,0.88)",
      "#E8E8E8","#909090","#4A4A4A","#0E0E0E",
      "#C0C0C0","#E0E0E0","#707070",
      "#FFFFFF","#B0B0B0","#888888",
      "#202020","#C0C0C0",
      "#141414","#0E0E0E","#141414","#0E0E0E","#B0B0B0" },

    // Earth Dark — deep forest floor. Rich greens with warm stone undertones.
    // Deep slate-olive base, moss accent. Not Nordic blue — greener and warmer.
    // Inspired by old-growth forest at dusk: dark earth, lit moss, warm bark.
    { "earth_dark", "Forest Stone",
      "#111A12","#182018","#1E281E","#263026","#2E3A2E","rgba(17,26,18,0.85)",
      "#D4E8C8","#7EA870","#3A5E32","#111A12",
      "#6DBF5C","#9AD688","#448040",
      "#CC6644","#88CC66","#C8A844",
      "#1E2A1E","#6DBF5C",
      "#182018","#111A12","#182018","#111A12","#88CC66" },

    // ── Additional themes ────────────────────────────────────────────────────
    { "ocean", "Ocean",
      "#0A1929","#0F2238","#13283F","#1C3450","#264360","rgba(10,25,41,0.85)",
      "#E0F0FF","#7AA0C8","#3D5878","#0A1929",
      "#5DB7F0","#90D0FF","#2A6FA8",
      "#FF6B6B","#5EE6A0","#FFC857",
      "#1C3450","#5DB7F0",
      "#0F2238","#0A1929","#0F2238","#0A1929","#5EE6A0" },

    { "rose", "Rose Gold",
      "#FBF5F2","#FDF8F5","#F5E8E2","#EBD5CC","#DEC0B5","rgba(238,210,200,0.60)",
      "#3D2820","#7C5448","#B59085","#FDF8F5",
      "#C97B5E","#D89478","#A05A40",
      "#C03830","#5A8A5A","#D4A040",
      "#EBD5CC","#C97B5E",
      "#FDF8F5","#FBF5F2","#FDF8F5","#FBF5F2","#5A8A5A" },

    { "monokai", "Monokai",
      "#272822","#2D2E27","#34352E","#3E3F37","#4A4B41","rgba(39,40,34,0.88)",
      "#F8F8F2","#A6A39A","#5C5D55","#272822",
      "#A6E22E","#C4F050","#6B9A1E",
      "#F92672","#A6E22E","#FD971F",
      "#3E3F37","#A6E22E",
      "#2D2E27","#272822","#2D2E27","#272822","#F92672" },

    { "dracula", "Dracula",
      "#282A36","#2F3142","#3B3E52","#44475A","#535668","rgba(40,42,54,0.88)",
      "#F8F8F2","#BFC1D2","#6272A4","#282A36",
      "#BD93F9","#D6B5FF","#7858B8",
      "#FF5555","#50FA7B","#FFB86C",
      "#44475A","#BD93F9",
      "#2F3142","#282A36","#2F3142","#282A36","#50FA7B" },

    { "solarized_light", "Solarized Light",
      "#FDF6E3","#F5EFD3","#EEE8C9","#E4DEB9","#D8D2A8","rgba(238,232,201,0.60)",
      "#073642","#586E75","#93A1A1","#FDF6E3",
      "#268BD2","#5DA9DC","#1A6BA0",
      "#DC322F","#859900","#B58900",
      "#E4DEB9","#268BD2",
      "#F5EFD3","#FDF6E3","#F5EFD3","#FDF6E3","#859900" },

    { "midnight", "Midnight Blue",
      "#0D1421","#131B2C","#1A2438","#222D44","#2C3852","rgba(13,20,33,0.90)",
      "#E8EBF0","#9098A8","#4A5268","#0D1421",
      "#7EA8D8","#A0C0E8","#4A6B98",
      "#E47878","#7DC990","#E0B070",
      "#222D44","#7EA8D8",
      "#131B2C","#0D1421","#131B2C","#0D1421","#7DC990" },

    { "high_contrast", "High Contrast",
      "#000000","#0A0A0A","#1A1A1A","#2A2A2A","#3A3A3A","rgba(0,0,0,0.95)",
      "#FFFFFF","#E0E0E0","#A0A0A0","#000000",
      "#FFD700","#FFE060","#B89500",
      "#FF3030","#30FF30","#FFB800",
      "#404040","#FFD700",
      "#0A0A0A","#000000","#0A0A0A","#000000","#30FF30" },

    // ── Additional themes (V32.14) ────────────────────────────────────────────
    //
    // Nord — popular cool-Scandinavian palette by Arctic Ice Studio.
    // Frosty blue-grey ground (Polar Night) with icy blue accents (Frost).
    // Famously balanced — used by ~everyone with a terminal customization habit.
    { "nord", "Nord",
      "#2E3440","#3B4252","#434C5E","#4C566A","#5C6878","rgba(46,52,64,0.85)",
      "#ECEFF4","#D8DEE9","#7B8694","#2E3440",
      "#88C0D0","#A3D2DC","#5E81AC",
      "#BF616A","#A3BE8C","#EBCB8B",
      "#434C5E","#88C0D0",
      "#3B4252","#2E3440","#3B4252","#2E3440","#A3BE8C" },

    // Gruvbox Dark — retro-warm developer favorite. Sepia-tinged dark
    // background with mustard/orange accents. Inspired by old paper.
    // Tribute to Pavel Pertsev's original Vim colorscheme.
    { "gruvbox", "Gruvbox",
      "#282828","#32302F","#3C3836","#504945","#665C54","rgba(40,40,40,0.88)",
      "#EBDBB2","#A89984","#665C54","#282828",
      "#FABD2F","#FFD866","#B57614",
      "#FB4934","#B8BB26","#FE8019",
      "#504945","#FABD2F",
      "#32302F","#282828","#32302F","#282828","#B8BB26" },

    // Solarized Dark — the dark companion to existing Solarized Light.
    // Same palette as Ethan Schoonover's classic, dark side. Low-contrast
    // by design — easy on the eyes for long sessions, color-blind friendly.
    { "solarized_dark", "Solarized Dark",
      "#002B36","#073642","#0A4452","#1A5562","#2A6675","rgba(0,43,54,0.88)",
      "#FDF6E3","#93A1A1","#586E75","#002B36",
      "#268BD2","#5DA9DC","#1A6BA0",
      "#DC322F","#859900","#B58900",
      "#073642","#268BD2",
      "#073642","#002B36","#073642","#002B36","#859900" },

    // Cyberpunk — true neon-noir. Magenta + cyan + electric purple on
    // near-black with deep magenta undertones. Different from existing
    // Neon (which is cyan/blue only) — this leans into the magenta side
    // of synthwave for a more aggressive vibe.
    { "cyberpunk", "Cyberpunk",
      "#0A0014","#10001F","#180029","#221038","#30185A","rgba(10,0,20,0.85)",
      "#F0E0FF","#B080D0","#604080","#0A0014",
      "#FF00AA","#FF66CC","#B0008C",
      "#FF4060","#00FFAA","#FFE000",
      "#2A1A4A","#FF00AA",
      "#10001F","#0A0014","#10001F","#0A0014","#00FFAA" },

    // Sepia — old-photograph warm browns. Light theme. For nostalgic
    // vibes; reads like aged paper. Less yellow than Solarized Light,
    // less orange than Gruvbox — purely brown-on-cream.
    { "sepia", "Sepia",
      "#F0E4D0","#F6ECD8","#EBDCBF","#DECDAB","#CDB892","rgba(220,200,170,0.60)",
      "#3A2818","#6B4F30","#A0856A","#F6ECD8",
      "#8B4513","#A65A28","#6B3410",
      "#A02828","#5A7038","#B07028",
      "#CDB892","#8B4513",
      "#F6ECD8","#F0E4D0","#F6ECD8","#F0E4D0","#5A7038" },

    // Terracotta — warm desert palette. Earthen reds, terracotta orange,
    // dusty sand. Inspired by southwestern adobe. Different from existing
    // Earth (which leans olive/forest) and Rose Gold (which is pink).
    // Light theme with strong warm accent.
    { "terracotta", "Terracotta",
      "#FAEDE0","#FFF4E6","#F2DDC8","#E8C5A8","#DCB088","rgba(232,197,168,0.62)",
      "#3A1F10","#7A4828","#B89070","#FFF4E6",
      "#C75B3C","#DD7858","#9A4028",
      "#A82820","#788848","#D89028",
      "#E8C5A8","#C75B3C",
      "#FFF4E6","#FAEDE0","#FFF4E6","#FAEDE0","#788848" },

    // Lavender — soft purple light theme. Lighter and cooler than
    // existing Pastels (which is multicolor pastel). Pure lavender +
    // lilac + soft mauve. For users who want something gentle.
    { "lavender", "Lavender",
      "#F2EDFA","#F8F4FC","#EBE2F5","#DDCFEC","#CCBADE","rgba(220,207,236,0.58)",
      "#2D1A4A","#6B5288","#A89AC8","#F8F4FC",
      "#7B5BB0","#9878C8","#5A3C8B",
      "#B83A5A","#5C9B7A","#B58A4A",
      "#DDCFEC","#7B5BB0",
      "#F8F4FC","#F2EDFA","#F8F4FC","#F2EDFA","#5C9B7A" },

    // Carbon — IBM Carbon Design inspired. Pure technical dark with
    // electric blue accent. Distinguished from Modern (which is more
    // GitHub-like) by harsher contrast and a colder, more industrial
    // blue. For users who want a corporate-tech vibe.
    { "carbon", "Carbon",
      "#161616","#1F1F1F","#262626","#393939","#525252","rgba(22,22,22,0.92)",
      "#F4F4F4","#A8A8A8","#6F6F6F","#161616",
      "#0F62FE","#4589FF","#0043CE",
      "#DA1E28","#42BE65","#F1C21B",
      "#393939","#0F62FE",
      "#1F1F1F","#161616","#1F1F1F","#161616","#42BE65" },

    // ── Additional themes (V32.32) ────────────────────────────────────────────
    // Eight new palettes filling identified gaps in the existing set.
    // Each chosen for being either widely-loved-in-dev-community OR
    // covering a color region not previously represented.

    // Tokyo Night — popular VSCode/Neovim theme. Deep midnight blue
    // with neon purple/cyan accents. Different from Dracula (more blue,
    // less saturation) and Midnight (more vibrant, less corporate).
    { "tokyo_night", "Tokyo Night",
      "#1A1B26","#1F2335","#24283B","#2A2F45","#363B54","rgba(26,27,38,0.88)",
      "#C0CAF5","#9AA5CE","#565F89","#1A1B26",
      "#7AA2F7","#BB9AF7","#3D59A1",
      "#F7768E","#9ECE6A","#E0AF68",
      "#2A2F45","#7AA2F7",
      "#1F2335","#1A1B26","#1F2335","#1A1B26","#9ECE6A" },

    // Catppuccin Mocha — soft warm dark with rose-pink-mauve accents.
    // The most popular "pastel dark" in modern dev community. Distinct
    // from existing Pastels Night (which is more lavender-led).
    { "catppuccin_mocha", "Catppuccin Mocha",
      "#1E1E2E","#181825","#11111B","#313244","#45475A","rgba(30,30,46,0.88)",
      "#CDD6F4","#A6ADC8","#6C7086","#1E1E2E",
      "#CBA6F7","#DDB6F2","#9078C2",
      "#F38BA8","#A6E3A1","#F9E2AF",
      "#313244","#CBA6F7",
      "#181825","#1E1E2E","#181825","#1E1E2E","#A6E3A1" },

    // Catppuccin Latte — light counterpart to Mocha. Warm cream/sand
    // base with same accent family. Fills the "warm light pastel" gap
    // that Paper (more neutral) and Rose Gold (warmer/more chromatic)
    // don't quite cover.
    { "catppuccin_latte", "Catppuccin Latte",
      "#EFF1F5","#E6E9EF","#DCE0E8","#CCD0DA","#BCC0CC","rgba(220,224,232,0.60)",
      "#4C4F69","#6C6F85","#9CA0B0","#EFF1F5",
      "#8839EF","#A570F4","#6E2BB8",
      "#D20F39","#40A02B","#DF8E1D",
      "#CCD0DA","#8839EF",
      "#E6E9EF","#EFF1F5","#E6E9EF","#EFF1F5","#40A02B" },

    // Synthwave — purple+pink+cyan 80s aesthetic. Distinct from
    // Cyberpunk (which leans yellow+magenta) and Neon (which is
    // cooler/more sci-fi). For users who want explicit retro-future.
    { "synthwave", "Synthwave",
      "#1A0B2E","#241040","#2D1A52","#3B2566","#4A317C","rgba(26,11,46,0.85)",
      "#F0E4FF","#B099D6","#6A4F94","#1A0B2E",
      "#FF6AD5","#FF8FE0","#C040A0",
      "#FF3060","#26FFB7","#FFB050",
      "#3B2566","#FF6AD5",
      "#241040","#1A0B2E","#241040","#1A0B2E","#26FFB7" },

    // Matcha — green light theme. The current set has lots of greens
    // in DARK themes (Forest Stone, Earth Dark) but no green LIGHT
    // theme. This fills that gap — soft cream base with rich matcha
    // green accent, like a cafe menu card.
    { "matcha", "Matcha",
      "#F5F2E8","#FAF7EE","#EDE7D4","#DDD3BA","#CABE9A","rgba(220,212,180,0.60)",
      "#1F2D1A","#5A6B48","#A8B498","#FAF7EE",
      "#5C8A3A","#7BA858","#3F6520",
      "#B83D2A","#7BA858","#C58F35",
      "#DDD3BA","#5C8A3A",
      "#FAF7EE","#F5F2E8","#FAF7EE","#F5F2E8","#7BA858" },

    // Sunset — warm orange→pink gradient light theme. The current set
    // has earthy warm (Terracotta, Rose Gold) but no actual sunset
    // gradient feel. Distinct enough to be worth its own slot.
    { "sunset", "Sunset",
      "#FFF2E8","#FFF8F0","#FFE5D0","#FFD0B0","#FFB890","rgba(255,200,170,0.58)",
      "#3D1F12","#7C4528","#B88870","#FFF8F0",
      "#FF6B4A","#FF8B6B","#D4452A",
      "#C8281A","#5A9568","#E8902A",
      "#FFD0B0","#FF6B4A",
      "#FFF8F0","#FFF2E8","#FFF8F0","#FFF2E8","#5A9568" },

    // Ice — cool clinical white+blue. Distinct from Paper (warm cream)
    // and Minimal (pure gray). For users who want crisp clarity with
    // a hint of cool color, like fresh snow with a slate sky.
    { "ice", "Ice",
      "#F0F4F8","#F8FAFC","#E2E8F0","#CBD5E1","#94A3B8","rgba(203,213,225,0.58)",
      "#0F172A","#475569","#94A3B8","#F8FAFC",
      "#0EA5E9","#38BDF8","#0369A1",
      "#DC2626","#16A34A","#D97706",
      "#CBD5E1","#0EA5E9",
      "#F8FAFC","#F0F4F8","#F8FAFC","#F0F4F8","#16A34A" },

    // Forest Day — light counterpart to Forest Stone (earth_dark). Sage
    // and moss greens with soft cream highlights, evoking dappled light
    // through leaves. Fills the "green light theme that isn't Matcha"
    // niche — Matcha is more vivid; this is more muted/restful.
    { "forest_day", "Forest Day",
      "#F4F1E8","#F9F7F0","#E8E4D5","#D8D0BA","#C0B89A","rgba(216,208,186,0.60)",
      "#1F2D1F","#52684C","#9CB098","#F9F7F0",
      "#4A7C3D","#6B9858","#345E2A",
      "#A8423A","#6B9858","#B58A3F",
      "#D8D0BA","#4A7C3D",
      "#F9F7F0","#F4F1E8","#F9F7F0","#F4F1E8","#6B9858" },
};

// ── Active palette ─────────────────────────────────────────────────────────────

// File-scope palette index — can be reset by setPalette() within the same session.
// Sentinels:
//   -1 = unloaded, re-read from QSettings on next activePalette() call
//   -2 = synthetic palette is active (see syntheticPalette() below)
//   ≥0 = index into kPalettes[]
inline int &paletteIndex()
{
    static int idx = -1;
    return idx;
}

// ── Synthetic palette storage ─────────────────────────────────────────────────
//
// V32.29: a runtime-derived palette for ambient/dynamic theming modes.
// Inbox-derived (sample edge bands of recent photos), camera-derived (live
// viewfinder), and similar generators all funnel into ONE synthetic palette
// slot. Only one can be "active" at a time. The user picking any static
// palette in Settings clears the synthetic and falls back to that pick.
//
// Storage model: a parallel array of std::string (durable across rebuilds)
// holds the color literals; the Palette struct's const char* fields point
// into those strings. The Palette is rebuilt on every setSyntheticPalette()
// call so the string addresses can't drift mid-flight.
//
// Thread safety: like the rest of Theme::, ALL setters/getters run on the
// main thread. Generators that compute palettes on a worker should hop back
// via QMetaObject::invokeMethod before calling setSyntheticPalette.
struct SyntheticPaletteStorage {
    // 24 color slots match the Palette struct's field count. Ordered by
    // their appearance in the struct definition above so the rebuild loop
    // can map them in order. If Palette gains/loses fields, update kSlotCount.
    static constexpr int kSlotCount = 24;
    std::array<std::string, kSlotCount> colors{};
    std::string id;     // synthetic id, e.g. "ambient_inbox"
    std::string name;   // human label, e.g. "Ambient: Inbox"
    Palette palette{};  // rebuilt to point into the strings above

    // Rebuild palette's const char* fields to point into our owned strings.
    // Called after any mutation of `colors` / `id` / `name`. The strings
    // themselves must outlive the Palette — guaranteed because we're storing
    // them as members and the whole struct lives in a function-static.
    void rebuildPalette() {
        palette.id              = id.c_str();
        palette.name            = name.c_str();
        palette.BG_DEEP         = colors[0].c_str();
        palette.BG_BASE         = colors[1].c_str();
        palette.BG_CARD         = colors[2].c_str();
        palette.BG_RAISED       = colors[3].c_str();
        palette.BG_HOVER        = colors[4].c_str();
        palette.BG_SCRIM        = colors[5].c_str();
        palette.TEXT_PRIMARY    = colors[6].c_str();
        palette.TEXT_SECONDARY  = colors[7].c_str();
        palette.TEXT_DISABLED   = colors[8].c_str();
        palette.TEXT_INVERSE    = colors[9].c_str();
        palette.ACCENT          = colors[10].c_str();
        palette.ACCENT_SOFT     = colors[11].c_str();
        palette.ACCENT_DIM      = colors[12].c_str();
        palette.DANGER          = colors[13].c_str();
        palette.SUCCESS         = colors[14].c_str();
        palette.WARNING         = colors[15].c_str();
        palette.BORDER          = colors[16].c_str();
        palette.BORDER_FOCUS    = colors[17].c_str();
        palette.GRAD_HERO_FROM  = colors[18].c_str();
        palette.GRAD_HERO_TO    = colors[19].c_str();
        palette.GRAD_BAR_FROM   = colors[20].c_str();
        palette.GRAD_BAR_TO     = colors[21].c_str();
        palette.GLOW_ONLINE     = colors[22].c_str();
        // slot 23 reserved for future expansion — currently unused
    }
};

inline SyntheticPaletteStorage &syntheticStorage()
{
    static SyntheticPaletteStorage s;
    return s;
}

/// Activate a runtime-derived palette. The Palette argument is COPIED into
/// the synthetic storage (so the caller can let their source Palette go out
/// of scope). The palette stays active until either:
///   - setPalette(staticId) is called with one of kPalettes[] ids
///   - clearSyntheticPalette() is called
///
/// Does NOT touch QSettings — the user's last static-palette choice
/// remains the persisted default and is restored on next launch unless
/// the application explicitly re-applies a synthetic at startup.
inline void setSyntheticPalette(const Palette &src,
                                  const char *syntheticId,
                                  const char *syntheticName)
{
    auto &s = syntheticStorage();
    s.id   = syntheticId   ? syntheticId   : "synthetic";
    s.name = syntheticName ? syntheticName : "Synthetic";
    // Copy each color literal into our owned storage. Strings can be nullptr
    // if the source Palette is partially populated; default to "#000000" for
    // safety so the stylesheet generator doesn't choke.
    auto cp = [](const char *p) -> std::string {
        return p ? std::string(p) : std::string("#000000");
    };
    s.colors[ 0] = cp(src.BG_DEEP);
    s.colors[ 1] = cp(src.BG_BASE);
    s.colors[ 2] = cp(src.BG_CARD);
    s.colors[ 3] = cp(src.BG_RAISED);
    s.colors[ 4] = cp(src.BG_HOVER);
    s.colors[ 5] = cp(src.BG_SCRIM);
    s.colors[ 6] = cp(src.TEXT_PRIMARY);
    s.colors[ 7] = cp(src.TEXT_SECONDARY);
    s.colors[ 8] = cp(src.TEXT_DISABLED);
    s.colors[ 9] = cp(src.TEXT_INVERSE);
    s.colors[10] = cp(src.ACCENT);
    s.colors[11] = cp(src.ACCENT_SOFT);
    s.colors[12] = cp(src.ACCENT_DIM);
    s.colors[13] = cp(src.DANGER);
    s.colors[14] = cp(src.SUCCESS);
    s.colors[15] = cp(src.WARNING);
    s.colors[16] = cp(src.BORDER);
    s.colors[17] = cp(src.BORDER_FOCUS);
    s.colors[18] = cp(src.GRAD_HERO_FROM);
    s.colors[19] = cp(src.GRAD_HERO_TO);
    s.colors[20] = cp(src.GRAD_BAR_FROM);
    s.colors[21] = cp(src.GRAD_BAR_TO);
    s.colors[22] = cp(src.GLOW_ONLINE);
    s.rebuildPalette();
    paletteIndex() = -2; // synthetic active
}

/// Clear the synthetic-palette override and revert to the user's saved
/// static palette. Settings UI calls this when the user picks any
/// non-synthetic carousel entry (in addition to setPalette(staticId)).
inline void clearSyntheticPalette()
{
    paletteIndex() = -1; // forces re-read of QSettings on next activePalette()
}

/// True if a synthetic palette is currently active (i.e. paletteIndex == -2).
inline bool isSyntheticActive()
{
    return paletteIndex() == -2;
}

inline const Palette &activePalette()
{
    int &idx = paletteIndex();
    if (idx == -2) {
        return syntheticStorage().palette;
    }
    if (idx < 0) {
        const QString saved = QSettings(QStringLiteral("SnapCloneProject"),
                                        QStringLiteral("SnapClone"))
            .value(QStringLiteral("ui/palette"), QStringLiteral("dark"))
            .toString();
        idx = 0;
        for (int i = 0; i < static_cast<int>(sizeof(kPalettes)/sizeof(kPalettes[0])); ++i)
            if (QLatin1String(kPalettes[i].id) == saved) { idx = i; break; }
    }
    return kPalettes[idx];
}

inline void setPalette(const QString &id)
{
    QSettings(QStringLiteral("SnapCloneProject"), QStringLiteral("SnapClone"))
        .setValue(QStringLiteral("ui/palette"), id);
    // Reset index so activePalette() re-reads QSettings on the next call.
    // This ensures Theme::ACCENT() etc. return new values immediately.
    paletteIndex() = -1;
}

// ── Convenience accessors (forward to active palette) ─────────────────────────

#define T(field) activePalette().field

inline const char *BG_DEEP    () { return T(BG_DEEP);    }
inline const char *BG_BASE    () { return T(BG_BASE);    }
inline const char *BG_CARD    () { return T(BG_CARD);    }
inline const char *BG_RAISED  () { return T(BG_RAISED);  }
inline const char *BG_HOVER   () { return T(BG_HOVER);   }
inline const char *BG_SCRIM   () { return T(BG_SCRIM);   }
inline const char *TEXT_PRIMARY   () { return T(TEXT_PRIMARY);   }
inline const char *TEXT_SECONDARY () { return T(TEXT_SECONDARY); }
inline const char *TEXT_DISABLED  () { return T(TEXT_DISABLED);  }
inline const char *TEXT_INVERSE   () { return T(TEXT_INVERSE);   }
inline const char *ACCENT      () { return T(ACCENT);      }
inline const char *ACCENT_SOFT () { return T(ACCENT_SOFT); }
inline const char *ACCENT_DIM  () { return T(ACCENT_DIM);  }
inline const char *DANGER      () { return T(DANGER);      }
inline const char *SUCCESS     () { return T(SUCCESS);     }
inline const char *WARNING     () { return T(WARNING);     }
inline const char *BORDER      () { return T(BORDER);      }
inline const char *BORDER_FOCUS() { return T(BORDER_FOCUS);}
inline const char *GLOW_ONLINE () { return T(GLOW_ONLINE); }
#undef T

// ── Layout constants (palette-independent) ────────────────────────────────────
//
// V32.34+: 10% global scale-down total — first 5% in earlier V32.34+ pass,
// another 5% in this iteration after device testing showed everything
// still reading "a touch too big" relative to Phosh / squeekboard /
// notification surfaces on FP5. The values below are the SECOND 5%
// reduction applied to the first set (i.e. roughly 0.95² ≈ 0.9025 of
// the original V32.33 sizes).
//
// Touch targets still respect the WCAG / Material 32px minimum:
// TOUCH_MIN at 40 is comfortable for index-finger taps and above the
// 32px accessibility floor. Going below would start to compromise
// fingertip reliability.
//
// If any specific component looks too tight after this scale, override
// the local font/spacing at that site rather than restoring the global
// value — keeps the global scale consistent across the app.
inline constexpr int TOUCH_MIN   = 40;   // was 42, was 44 (V32.33)
inline constexpr int TOUCH_LARGE = 58;   // was 61, was 64
inline constexpr int TOUCH_ICON  = 47;   // was 49, was 52
inline constexpr int FONT_XS   = 10;     // held at 10 — below this gets unreadable
inline constexpr int FONT_SM   = 11;     // was 12, was 13
inline constexpr int FONT_BASE = 13;     // was 14, was 15
inline constexpr int FONT_LG   = 16;     // was 17, was 18
inline constexpr int FONT_XL   = 20;     // was 21, was 22
inline constexpr int FONT_2XL  = 25;     // was 26, was 27
inline constexpr int RADIUS_SM   =  8;
inline constexpr int RADIUS_MD   = 14;
inline constexpr int RADIUS_LG   = 20;
inline constexpr int RADIUS_XL   = 28;
inline constexpr int RADIUS_FULL = 999;
inline constexpr int SPACE_XS  =  6;     // floor — already minimal
inline constexpr int SPACE_SM  = 10;     // was 11, was 12
inline constexpr int SPACE_MD  = 18;     // was 19, was 20
inline constexpr int SPACE_LG  = 28;     // was 30, was 32
inline constexpr int SPACE_XL  = 43;     // was 46, was 48
inline constexpr int ANIM_FAST   = 110;
inline constexpr int ANIM_NORMAL = 220;
inline constexpr int ANIM_SLOW   = 340;

// ── V32.30: viewport-relative sizing (HTML-vh/vw analogues) ──────────────────
//
// Qt has no direct equivalent to HTML `vh` / `vw`. At very high OS scaling
// (e.g. 300%), the screen's *logical* size shrinks, but our hard-coded
// constants stay the same physical-pixel value — content overflows. These
// helpers return values that scale with the actual usable canvas.
//
// Reading: vh(N) returns N% of screen logical height. vw(N) likewise for
// width. Use for margins / paddings that should breathe with screen size,
// NOT for fonts or touch targets (those have their own DPI scaling via
// the system).
//
// Recipe pattern (HTML calc analogue):
//   const int sideMargin = vminPx(2) + SPACE_MD; // 2vmin + 20px constant
// This combines a screen-relative term with a fixed baseline — same as
//   width: calc(50px + 5vmin);
// in CSS. Use vmin/vmax to stay device-orientation-independent.
//
// All helpers return ints (pixels). They re-query the primary screen
// each call — cheap (Qt caches QScreen pointers) and tolerant of
// monitor changes / window relocation.

inline int vhPx(qreal pct)
{
    const auto *s = QGuiApplication::primaryScreen();
    if (!s) return static_cast<int>(pct * 8); // ~800 logical default
    return static_cast<int>(s->size().height() * pct / 100.0);
}

inline int vwPx(qreal pct)
{
    const auto *s = QGuiApplication::primaryScreen();
    if (!s) return static_cast<int>(pct * 4);
    return static_cast<int>(s->size().width() * pct / 100.0);
}

/// Smaller of vw/vh — orientation-independent (matches CSS `vmin`).
/// Useful when a side margin should respect BOTH dimensions; on a
/// portrait phone vmin tracks width, on a landscape tablet vmin
/// tracks height.
inline int vminPx(qreal pct)
{
    return qMin(vhPx(pct), vwPx(pct));
}

/// V32.30: standard "page side margin" — used as the outer padding on
/// the major page widgets (Inbox, Contacts, Settings) so that at high
/// OS scaling they don't run their content edge-to-edge. Formula:
/// `vmin(3) + SPACE_SM` ≈ 3% of the shorter screen side plus 12px.
/// Examples:
///   - 1920×1080 desktop: 3% of 1080 = 32 + 12 = 44px
///   - 720×360 phone (after 300% scale of 2160×1080):
///       3% of 360 = 11 + 12 = 23px
///   - 400×800 narrow phone: 3% of 400 = 12 + 12 = 24px
/// Constant baseline (SPACE_SM) ensures even tiny screens get *some*
/// inset; the percentage term grows it on roomier surfaces.
inline int pageSideMarginPx()
{
    return vminPx(3.0) + SPACE_SM;
}

/// V32.30: standard "page top margin" — similar but tracks vertical
/// breathing room. `vh(1.5) + SPACE_SM` gives less than the side margin
/// because vertical real estate is usually scarcer (status bars,
/// tab bars take chunks).
inline int pageTopMarginPx()
{
    return vhPx(1.5) + SPACE_SM;
}

// ── Global stylesheet (generated from active palette) ─────────────────────────

[[nodiscard]] inline QString globalStyleSheet()
{
    const Palette &p = activePalette();
    // Header gradient as a QLinearGradient in stylesheet syntax
    const bool isLight = (QLatin1String(p.BG_DEEP)[1] > '8'); // rough lightness check
    const QString scrollHandleColor = isLight ? QLatin1String(p.ACCENT_DIM)
                                               : QLatin1String(p.BG_HOVER);
    return QStringLiteral(R"(
        QMainWindow, QDialog, QWidget {
            background-color: %1;
            color: %2;
            font-family: "Inter","SF Pro Display","Helvetica Neue","Segoe UI",Arial,sans-serif;
            font-size: 15px;
        }
        QPushButton {
            background-color: %3;
            color: %2;
            border: 1px solid %11;
            border-radius: 14px;
            padding: 13px 26px;
            font-size: 15px;
            font-weight: 500;
            min-height: 48px;
        }
        QPushButton:hover   { background-color: %4; border-color: %12; }
        QPushButton:pressed { background-color: %1; }
        QPushButton:disabled{ color: %7; border-color: %1; background-color: %1; }
        QPushButton#accentBtn {
            background-color: %8;
            color: %10;
            border: none;
            font-weight: 600;
        }
        QPushButton#accentBtn:hover   { background-color: %9; }
        QPushButton#accentBtn:pressed { background-color: %13; }
        QPushButton#iconBtn {
            background-color: rgba(36,34,32,0.88);
            border: 1px solid %11;
            border-radius: 26px;
            padding: 0;
            min-width: 52px; min-height: 52px;
            max-width: 52px; max-height: 52px;
            font-size: 22px;
        }
        QPushButton#iconBtn:hover   { background-color: %4; }
        QPushButton#iconBtn:pressed { background-color: %1; }
        QPushButton#shutterBtn {
            background-color: %2;
            color: %1;
            border: 3px solid %8;
            border-radius: 32px;
            min-width: 64px; min-height: 64px;
            max-width: 64px; max-height: 64px;
            padding: 0;
            font-size: 28px;
        }
        QPushButton#shutterBtn:pressed { background-color: %8; }
        QPushButton#dangerBtn {
            background-color: transparent;
            color: %14;
            border: 1px solid %14;
        }
        QPushButton#dangerBtn:hover { background-color: rgba(189,90,68,0.10); }
        QLineEdit, QTextEdit {
            background-color: %5;
            color: %2;
            border: 1.5px solid %11;
            border-radius: 12px;
            padding: 13px 16px;
            font-size: 15px;
            selection-background-color: %8;
            selection-color: %10;
            min-height: 44px;
        }
        QLineEdit:focus, QTextEdit:focus {
            border-color: %12;
            background-color: %3;
        }
        QListWidget {
            background-color: %5;
            border: none;
            border-radius: 14px;
            padding: 6px;
            outline: none;
        }
        QListWidget::item {
            padding: 15px 16px;
            border-radius: 10px;
            min-height: 48px;
            color: %2;
        }
        QListWidget::item:selected { background-color: %11; color: %8; }
        QListWidget::item:hover:!selected { background-color: %3; }
        QComboBox {
            background-color: %5;
            color: %2;
            border: 1.5px solid %11;
            border-radius: 10px;
            padding: 9px 12px;
            min-height: 40px;
        }
        QComboBox:focus { border-color: %12; }
        QComboBox::drop-down { border: none; width: 24px; }
        QComboBox QAbstractItemView {
            background-color: %5;
            border: 1.5px solid %11;
            border-radius: 10px;
            selection-background-color: %11;
            color: %2;
        }
        QScrollBar:vertical {
            background: transparent; width: 4px; margin: 0;
        }
        QScrollBar::handle:vertical {
            background: %4;
            border-radius: 2px;
            min-height: 28px;
        }
        QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { height: 0; }
        QStatusBar {
            background-color: %1;
            color: %7;
            font-size: 11px;
            border-top: 1px solid %5;
        }
        QGroupBox {
            border: 1.5px solid %11;
            border-radius: 14px;
            margin-top: 10px;
            padding: 14px;
            color: %6;
            font-size: 12px;
            font-weight: 500;
        }
        QGroupBox::title { subcontrol-origin: margin; left: 14px; padding: 0 5px; }
        QLabel#titleLabel    { font-size: 22px; font-weight: 600; color: %2; }
        QLabel#subtitleLabel { font-size: 13px; color: %6; }
        QLabel#errorLabel    { color: %14; font-size: 13px; }
        QLabel#accentLabel   { color: %8; font-size: 13px; font-weight: 500; }
        QPushButton#copyBtn {
            background-color: %3;
            color: %8;
            border: 1px solid %8;
            border-radius: 10px;
            padding: 7px 14px;
            font-size: 13px;
            min-height: 36px;
        }
        QPushButton#copyBtn:hover   { background-color: rgba(201,185,154,0.12); }
        QPushButton#copyBtn:pressed { background-color: rgba(201,185,154,0.20); }
    )")
    .arg(QLatin1String(p.BG_DEEP))           // 1
    .arg(QLatin1String(p.TEXT_PRIMARY))       // 2
    .arg(QLatin1String(p.BG_RAISED))          // 3
    .arg(QLatin1String(p.BG_HOVER))           // 4
    .arg(QLatin1String(p.BG_CARD))            // 5
    .arg(QLatin1String(p.TEXT_SECONDARY))     // 7
    .arg(QLatin1String(p.TEXT_DISABLED))      // 8
    .arg(QLatin1String(p.ACCENT))             // 9
    .arg(QLatin1String(p.ACCENT_SOFT))        // 10
    .arg(QLatin1String(p.TEXT_INVERSE))       // 11
    .arg(QLatin1String(p.BORDER))             // 12
    .arg(QLatin1String(p.BORDER_FOCUS))       // 13
    .arg(QLatin1String(p.ACCENT_DIM))         // 14
    .arg(QLatin1String(p.DANGER));            // 15
}

/// V32.32: applies globalStyleSheet() to qApp **only if it actually
/// differs from the last-applied sheet**. This is a meaningful perf
/// optimization during ambient mode: AmbientCameraDriver fires every
/// 2s and AmbientInboxDriver fires per snap arrival, and each call
/// to `qApp->setStyleSheet()` triggers a StyleChange event delivered
/// to EVERY widget in the app (50-200ms of CPU on a phone).
///
/// With EMA smoothing α=0.15, consecutive ambient-camera ticks
/// frequently produce IDENTICAL palettes (when the camera is pointed
/// at a stable scene). A string-equality check is microseconds; if
/// equal, we skip the expensive Qt restyle entirely.
///
/// Even when the palette IS changing, comparing against the last
/// applied sheet costs almost nothing — and on devices where the
/// scene is moving constantly we just pay the same restyle cost
/// as before.
inline void applyGlobalStyleSheetIfChanged()
{
    if (!qApp) return;
    static QString s_lastApplied;
    const QString next = globalStyleSheet();
    if (next == s_lastApplied) return; // no-op — palette unchanged
    s_lastApplied = next;
    qApp->setStyleSheet(next);
}

} // namespace SnapClone::UI::Theme
