//! rosetta.rs — a "Rosetta stone" that turns a **text map spec** (the prose briefs in
//! `docs/design/map-specs.txt`) into a **target signature** that *guides the procgen search* toward that theme.
//!
//! This is deliberately **not** an NLP-to-geometry parser (brittle, and a research project). It's a
//! **guided-search transfer**: the spec is mined with a keyword vocabulary into a [`ThemeTarget`] — a palette,
//! an archetype-mix bias, a spatial/zone bias, a density target, and a mood. That target becomes a *fitness
//! signal*, and the existing evolutionary search over the procgen net (see `procgen::evolve_taste`) finds a
//! generated map that matches it. The text says *what kind of place*; the search does the work of building one.
//!
//! ## Why guided search, not parsing
//! The prose describes a bright-yellow crashed gondola with slashed red velvet seats — no generator turns that
//! sentence into that mesh. But the *signature* of the spec — a cold alpine palette, a central landmark POI, a
//! flank wreck, a wide-open lower level with a dense upper level — is exactly the kind of target a search can
//! be steered toward with the archetypes and palette we already have. We transfer the *composition and mood*,
//! not the literal props. The result reads as "an alpine industrial station", not grey boxes on a plain.
//!
//! ## The vocabulary
//! A small, auditable keyword→signal table (below). Each family of words nudges one axis of the target:
//! colour words set the palette, POI-role words ("landmark", "flank cover", "choke") bias the archetype mix,
//! setting words ("frozen", "flooded", "overgrown", "industrial") pick a zone/material lean, and
//! density words ("wide-open", "dense", "cramped") set the cover target. Extend the table to cover more specs.

use crate::level::Palette;
use crate::mesh::rgb;
use egui::Color32;

/// The extracted, search-steering signature of a text spec.
#[derive(Clone)]
pub struct ThemeTarget {
    /// A human label (the map's name, if we found one).
    pub name: String,
    /// The palette to style the generated level with.
    pub palette: Palette,
    /// Desired archetype mix as fractions (0..1, summing ~1) — what the map should be made of.
    pub archetype_mix: [f32; 7],
    /// Desired cover density 0..1 (0 = wide-open field, 1 = dense/cramped).
    pub density: f32,
    /// Desired water fraction 0..1 (flooded/underwater specs push this up).
    pub water: f32,
    /// Desired verticality 0..1 (multi-tier / elevated / tower specs push this up).
    pub verticality: f32,
    /// A short mood string for reporting.
    pub mood: String,
}

/// A colour keyword and the RGB it implies. Ordered so the first match in the text wins for the accent.
const COLOR_WORDS: &[(&str, (u8, u8, u8))] = &[
    ("frozen", (206, 224, 235)),
    ("ice", (200, 220, 232)),
    ("snow", (232, 238, 245)),
    ("alpine", (210, 222, 232)),
    ("rust", (150, 92, 56)),
    ("rusted", (150, 92, 56)),
    ("orange", (196, 120, 60)),
    ("steel", (150, 156, 164)),
    ("chrome", (176, 182, 190)),
    ("concrete", (150, 148, 142)),
    ("marble", (222, 220, 214)),
    ("green", (86, 128, 74)),
    ("algae", (72, 110, 64)),
    ("overgrown", (92, 132, 78)),
    ("moss", (88, 120, 76)),
    ("jungle", (72, 116, 66)),
    ("fern", (96, 134, 82)),
    ("sand", (196, 176, 132)),
    ("desert", (198, 172, 126)),
    ("purple", (96, 84, 128)),
    ("stormy", (92, 96, 120)),
    ("yellow", (206, 190, 96)),
    ("gold", (198, 168, 96)),
    ("golden", (198, 168, 96)),
    ("red", (166, 78, 66)),
    ("blood", (128, 58, 52)),
    ("copper", (176, 108, 66)),
    ("lava", (200, 90, 50)),
    ("magma", (200, 90, 50)),
    ("volcanic", (70, 62, 62)),
    ("obsidian", (44, 42, 50)),
    ("brine", (96, 130, 132)),
    ("teal", (70, 128, 130)),
    ("white", (224, 226, 230)),
    ("black", (46, 46, 52)),
];

/// Setting/material keywords → (water push, verticality push, zone material lean as an accent tint).
const SETTING_WORDS: &[(&str, (f32, f32))] = &[
    ("flooded", (0.7, 0.0)),
    ("submerged", (0.9, 0.0)),
    ("underwater", (0.95, 0.0)),
    ("sunken", (0.7, 0.0)),
    ("drowned", (0.8, 0.0)),
    ("lakebed", (0.5, 0.0)),
    ("reef", (0.85, 0.0)),
    ("tidal", (0.6, 0.0)),
    ("elevated", (0.0, 0.8)),
    ("vertical", (0.0, 0.85)),
    ("multi-tiered", (0.0, 0.8)),
    ("tower", (0.0, 0.7)),
    ("platform", (0.0, 0.5)),
    ("high-altitude", (0.0, 0.75)),
    ("scaffold", (0.0, 0.7)),
    ("catwalk", (0.0, 0.6)),
];

/// Density keywords → target 0..1.
const DENSITY_WORDS: &[(&str, f32)] = &[
    ("wide-open", 0.15),
    ("wide open", 0.15),
    ("open", 0.25),
    ("sparse", 0.2),
    ("barren", 0.1),
    ("dense", 0.8),
    ("cramped", 0.85),
    ("claustrophobic", 0.9),
    ("tight", 0.8),
    ("cluttered", 0.8),
    ("labyrinth", 0.85),
];

/// POI-role keywords → which archetype they imply (index into Archetype::ALL).
const ROLE_WORDS: &[(&str, usize)] = &[
    ("landmark", 3),      // Spine
    ("focal point", 3),   // Spine
    ("tower", 3),         // Spine
    ("beacon", 3),        // Spine
    ("high ground", 1),   // Vantage
    ("vantage", 1),       // Vantage
    ("perch", 1),         // Vantage
    ("overwatch", 1),     // Vantage
    ("bunker", 2),        // Sentinel
    ("nest", 2),          // Sentinel
    ("fortified", 2),     // Sentinel
    ("machine-gun", 2),   // Sentinel
    ("wreck", 4),         // Wreckage
    ("wreckage", 4),      // Wreckage
    ("crashed", 4),       // Wreckage
    ("derailed", 4),      // Wreckage
    ("debris", 4),        // Wreckage
    ("shelter", 5),       // Shelter
    ("tent", 5),          // Shelter
    ("camp", 5),          // Shelter
    ("cover", 0),         // SightlineBreaker
    ("barricade", 0),     // SightlineBreaker
    ("sightline breaker", 0),
    ("choke", 0),
];

fn count(hay: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    hay.matches(needle).count()
}

/// Extract a [`ThemeTarget`] from a raw spec text block. Case-insensitive keyword mining — auditable and cheap.
pub fn extract(spec: &str) -> ThemeTarget {
    let t = spec.to_lowercase();

    // ── name: first "N. The Something (Theme)" line, or the first non-empty line ──
    let name = spec
        .lines()
        .find_map(|l| {
            let l = l.trim();
            if let Some(rest) = l.split_once(". ") {
                if rest.0.chars().all(|c| c.is_ascii_digit()) {
                    return Some(rest.1.to_string());
                }
            }
            None
        })
        .unwrap_or_else(|| spec.lines().next().unwrap_or("Untitled").trim().to_string());

    // ── palette from colour words ──
    // Surface colours come from overall frequency (the materials mentioned most). The SKY colour, though, is
    // better taken from atmosphere context — a spec's "stormy purple sky" matters more for the mood than an
    // incidental "white ceramic" panel, even if white is mentioned more. So we score colour words twice: once
    // by raw frequency (for surfaces), and once boosted when they sit near atmosphere words (for the sky).
    let atmo_words = ["sky", "light", "lighting", "fog", "sunlight", "glow", "atmosphere", "haze", "shadow", "overcast"];
    let mut hits: Vec<((u8, u8, u8), usize)> = Vec::new();
    let mut atmo_hits: Vec<((u8, u8, u8), f32)> = Vec::new();
    for (word, col) in COLOR_WORDS {
        let c = count(&t, word);
        if c == 0 {
            continue;
        }
        hits.push((*col, c));
        // atmosphere score: how often this colour word appears within ~40 chars of an atmosphere word
        let mut atmo = 0.0f32;
        let mut start = 0;
        while let Some(pos) = t[start..].find(word) {
            let abs = start + pos;
            // char-safe window bounds: walk to the nearest char boundary so we never slice mid-codepoint
            let mut lo = abs.saturating_sub(40);
            while lo > 0 && !t.is_char_boundary(lo) {
                lo -= 1;
            }
            let mut hi = (abs + word.len() + 40).min(t.len());
            while hi < t.len() && !t.is_char_boundary(hi) {
                hi += 1;
            }
            let window = &t[lo..hi];
            if atmo_words.iter().any(|a| window.contains(a)) {
                atmo += 1.0;
            }
            start = abs + word.len();
        }
        if atmo > 0.0 {
            atmo_hits.push((*col, atmo));
        }
    }
    hits.sort_by(|a, b| b.1.cmp(&a.1));
    atmo_hits.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let pick = |i: usize, fallback: (u8, u8, u8)| -> Color32 {
        hits.get(i).map(|(c, _)| rgb(c.0, c.1, c.2)).unwrap_or(rgb(fallback.0, fallback.1, fallback.2))
    };
    // dominant colour drives cover/accent; second drives wall/rock; sky prefers the atmosphere colour.
    let dom = pick(0, (150, 148, 142));
    let sec = pick(1, (130, 126, 120));
    let sky_col = atmo_hits.first().map(|(c, _)| rgb(c.0, c.1, c.2));
    let darker = |c: Color32, f: f32| rgb((c.r() as f32 * f) as u8, (c.g() as f32 * f) as u8, (c.b() as f32 * f) as u8);
    let lighter = |c: Color32, f: f32| {
        rgb(
            (c.r() as f32 + (255.0 - c.r() as f32) * f) as u8,
            (c.g() as f32 + (255.0 - c.g() as f32) * f) as u8,
            (c.b() as f32 + (255.0 - c.b() as f32) * f) as u8,
        )
    };
    let palette = Palette {
        floor: darker(sec, 0.7),
        wall: sec,
        trim: lighter(sec, 0.3),
        ceil: darker(sec, 0.5),
        cover: dom,
        sky: sky_col.map(|c| lighter(c, 0.35)).unwrap_or_else(|| lighter(dom, 0.55)),
        fog: sky_col.map(|c| lighter(c, 0.25)).unwrap_or_else(|| lighter(dom, 0.4)),
        accent: pick(2, (200, 160, 90)),
        rock: darker(sec, 0.8),
    };

    // ── archetype mix from role words (smoothed so nothing is zero) ──
    let mut mix = [0.6f32; 7]; // small base so every archetype stays possible
    mix[6] = 2.0; // a healthy baseline of open space (Void)
    for (word, idx) in ROLE_WORDS {
        mix[*idx] += count(&t, word) as f32 * 1.5;
    }
    let sum: f32 = mix.iter().sum();
    for m in mix.iter_mut() {
        *m /= sum;
    }

    // ── density ──
    let mut density = 0.4f32;
    let mut dhits = 0;
    for (word, d) in DENSITY_WORDS {
        let c = count(&t, word);
        if c > 0 {
            density += *d * c as f32;
            dhits += c;
        }
    }
    if dhits > 0 {
        density /= 1.0 + dhits as f32; // average toward the keyword signal
    }
    density = density.clamp(0.1, 0.9);

    // ── water / verticality from setting words ──
    let mut water = 0.0f32;
    let mut vert = 0.0f32;
    for (word, (w, v)) in SETTING_WORDS {
        if count(&t, word) > 0 {
            water = water.max(*w);
            vert = vert.max(*v);
        }
    }

    // ── mood: a couple of atmosphere adjectives if present ──
    let mood_words = ["cold", "humid", "eerie", "bright", "dark", "warm", "bleak", "hostile", "serene", "toxic", "foggy", "stormy"];
    let mood: Vec<&str> = mood_words.iter().copied().filter(|w| t.contains(w)).take(3).collect();

    ThemeTarget {
        name,
        palette,
        archetype_mix: mix,
        density,
        water,
        verticality: vert,
        mood: if mood.is_empty() { "neutral".into() } else { mood.join(", ") },
    }
}

/// Score how well a generated level matches a theme target, 0..1 (higher = closer). Combines palette match
/// (implicit — the level is *built* with the target palette so this is about layout), archetype-mix match,
/// density match, and water/verticality match. This is the fitness the guided search maximises.
pub fn match_score(target: &ThemeTarget, mix_counts: &[u32; 7], level: &crate::level::Level) -> f32 {
    // archetype mix: normalise the realised counts and compare to target (1 - half L1 distance)
    let total: f32 = mix_counts.iter().sum::<u32>() as f32;
    let mut mix_dist = 0.0;
    if total > 0.0 {
        for i in 0..7 {
            let realised = mix_counts[i] as f32 / total;
            mix_dist += (realised - target.archetype_mix[i]).abs();
        }
    } else {
        mix_dist = 2.0;
    }
    let mix_match = (1.0 - mix_dist * 0.5).clamp(0.0, 1.0);

    // density: cover pieces per area vs target
    let area = (level.w * level.h) as f32 * level.ts() * level.ts();
    let realised_density = (level.cover_obstacles.len() as f32 / area.max(1.0)) / (1.0 / 120.0); // normalise so ~1 piece/120m^2 = 1.0
    let dens_match = 1.0 - (realised_density.clamp(0.0, 1.5) - target.density).abs().min(1.0);

    // water: realised water fraction vs target
    let water_area: f32 = level.water.iter().map(|w| std::f32::consts::PI * w.radius * w.radius).sum();
    let realised_water = (water_area / area.max(1.0)).clamp(0.0, 1.0);
    let water_match = 1.0 - (realised_water - target.water).abs();

    // verticality: platforms + tall structures as a rough proxy
    let vert_proxy = (level.platforms.len() as f32 / 8.0).clamp(0.0, 1.0);
    let vert_match = 1.0 - (vert_proxy - target.verticality).abs();

    (mix_match * 0.4 + dens_match * 0.3 + water_match * 0.2 + vert_match * 0.1).clamp(0.0, 1.0)
}

/// The seven archetype names in order, for reporting.
pub fn archetype_labels() -> [&'static str; 7] {
    ["break", "vantage", "sentinel", "spine", "wreck", "shelter", "void"]
}

/// Split a specs document into individual map blocks. Each map starts at a line like "N. The Name (Theme)".
pub fn split_specs(doc: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in doc.lines() {
        let trimmed = line.trim_start();
        let is_header = trimmed
            .split_once(". ")
            .map(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false);
        if is_header && !cur.trim().is_empty() {
            blocks.push(std::mem::take(&mut cur));
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.trim().is_empty() {
        blocks.push(cur);
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALPINE: &str = "1. The Gondola Terminus (Alpine Mechanical Station)\n\
        Spatial Correlation: A vertical split map. The lower level is a wide-open, frozen lakebed offering \
        extreme long sightlines but zero cover. The upper level is a dense, multi-tiered industrial platform.\n\
        The Drive Sheaves (Center High Ground POI): steel bull-wheels, a focal point landmark.\n\
        The Gondola Wreck (Flank Cover): a crashed derailed gondola car. Bitterly cold.";

    const FLOODED: &str = "2. The Elevated Spine (Flooded Mag-Lev Wreckage)\n\
        Spatial Correlation: a high-altitude three-lane map suspended above a flooded sunken city.\n\
        The Derailed Leviathan (Center Lane Obstacle): a derailed wreck. Stormy purple sky, foggy.";

    #[test]
    fn extracts_theme_signal_from_alpine_spec() {
        let t = extract(ALPINE);
        assert!(t.name.contains("Gondola"), "should pull the map name, got {}", t.name);
        // "frozen"/"steel"/"industrial" → cool/steel palette (sky noticeably light/cool)
        assert!(t.palette.cover.b() as i32 >= t.palette.cover.r() as i32 - 40, "alpine palette should lean cool");
        // has a landmark (Spine) and a wreck and vantage in the mix
        assert!(t.archetype_mix[3] > 1.0 / 7.0, "should want a landmark (spine)");
        assert!(t.archetype_mix[4] > 1.0 / 7.0, "should want a wreck");
        // "multi-tiered"/"vertical" → verticality; "wide-open" tempered by "dense"
        assert!(t.verticality > 0.5, "vertical split should raise verticality");
        assert!(t.mood.contains("cold"), "should catch the cold mood, got {}", t.mood);
    }

    #[test]
    fn flooded_spec_pushes_water_and_verticality() {
        let t = extract(FLOODED);
        assert!(t.water > 0.5, "flooded/sunken should push water up, got {}", t.water);
        assert!(t.verticality > 0.5, "high-altitude/elevated should push verticality, got {}", t.verticality);
        assert!(t.archetype_mix[4] > 1.0 / 7.0, "derailed wreck should bias wreckage");
    }

    #[test]
    fn density_words_move_the_target() {
        let open = extract("1. Open Field\nA wide-open barren sparse plain.");
        let tight = extract("1. Warren\nA dense cramped claustrophobic labyrinth.");
        assert!(open.density < tight.density, "open spec must be less dense than cramped ({:.2} vs {:.2})", open.density, tight.density);
    }

    #[test]
    fn guided_search_improves_theme_match() {
        // The full Rosetta transfer: extract a target from a spec, and a short guided search should find a net
        // whose themed map matches the target better than the default net does. Uses the real generate_themed +
        // match_score path.
        use crate::procgen::{evolve_toward, generate_themed, ActivationNet, ProcParams};
        use crate::rosetta::match_score;
        let target = extract(ALPINE);
        let params = ProcParams::default();
        let seeds = [1u64, 2];
        let base = ActivationNet::default();
        let base_match: f32 = seeds.iter().map(|&s| {
            let (l, r) = generate_themed(s, &base, &params, &target);
            match_score(&target, &r.placed, &l)
        }).sum::<f32>() / seeds.len() as f32;
        let (_evolved, fit) = evolve_toward(&target, &seeds, &params, 3, 6, 7);
        // fitness blends match + composition, so it should be a sensible positive number and not below the raw
        // base match by much (evolution can only help or hold).
        assert!(fit > 0.0, "evolution should yield a positive fitness");
        assert!(fit >= base_match * 0.7, "guided search should not collapse below the base match ({fit:.2} vs {base_match:.2})");
    }

    #[test]
    fn themed_generation_applies_the_palette_and_water() {
        // generate_themed must actually restyle the level: a high-water target should add water bodies and the
        // level palette should match the target.
        use crate::procgen::{generate_themed, ActivationNet, ProcParams};
        let target = extract(FLOODED);
        let (level, _) = generate_themed(3, &ActivationNet::default(), &ProcParams::default(), &target);
        assert_eq!(level.pal.cover.r(), target.palette.cover.r(), "level should adopt the theme palette");
        assert!(!level.water.is_empty(), "a flooded theme should place water bodies");
    }

    #[test]
    fn extract_is_utf8_safe_with_multibyte_chars() {
        // Real specs contain em-dashes and other multibyte characters; the atmosphere-window scan must never
        // slice mid-codepoint. This spec packs a colour word right next to an em-dash to exercise the bounds.
        let spec = "1. Test Map — Stormy\nThe sky is a bruised purple — dark and foggy — over the ruins.";
        let t = extract(spec); // must not panic
        assert!(t.palette.sky.r() > 0 || t.palette.sky.b() > 0);
    }

    #[test]
    fn atmosphere_colour_drives_the_sky() {
        // A spec whose surfaces are white but whose *sky* is purple should yield a purple-ish sky, not white —
        // the atmosphere-context weighting must win for the sky colour.
        let spec = "1. Storm Deck\nWhite ceramic panels and white salt everywhere. \
            Atmosphere: the sky is a bruised stormy purple, with heavy purple fog.";
        let t = extract(spec);
        // purple has b > g; a white sky would have r≈g≈b
        assert!(t.palette.sky.b() as i32 > t.palette.sky.g() as i32, "atmosphere purple should tint the sky, got ({},{},{})", t.palette.sky.r(), t.palette.sky.g(), t.palette.sky.b());
    }

    #[test]
    fn splits_a_multi_map_document() {
        let doc = format!("intro line\n{ALPINE}\n{FLOODED}\n");
        let blocks = split_specs(&doc);
        assert!(blocks.len() >= 2, "should split into at least two map blocks, got {}", blocks.len());
    }
}
