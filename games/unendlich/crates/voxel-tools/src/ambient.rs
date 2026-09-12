//! Ambient: what the surroundings say about what belongs here.
//!
//! Every default in the plan parser is a constant. `cylinder @0,0` with
//! no block builds STONE, on a glacier and in a jungle alike, and a
//! model that does not name a material gets the same grey tower
//! everywhere. That is the single biggest reason a generated build
//! looks dropped in rather than grown in place.
//!
//! The engine already knows better. Its climate field reports
//! temperature and humidity per cell (`climate: T=0.50 H=0.73` in the
//! frame log), the water sim reports wind, and a probe of the ground
//! reports what the surface is actually made of. None of that reached
//! the paintbrush.
//!
//! So: one struct the engine fills, which answers three questions.
//!
//! * **What is it made of here?** A palette, and the default block for
//!   an op that did not name one.
//! * **What grows here?** A species for `tree` and `scatter`.
//! * **What should the model be told?** One line of context, so a
//!   request for "a hut" in a snowy biome comes back with a steep roof
//!   and spruce rather than a flat stone box.
//!
//! Deliberately NOT a biome enum. The engine's biome list is its own
//! and will change; temperature and humidity are physical, stable, and
//! already computed. A palette derived from two floats survives a biome
//! rename, and works in a modded world with biomes this crate has never
//! heard of.

/// A reading of the world at the build site.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ambient {
    /// Engine climate T, 0.0 (polar) .. 1.0 (desert).
    pub temperature: f32,
    /// Engine climate H, 0.0 (arid) .. 1.0 (saturated).
    pub humidity: f32,
    /// Ground level at the site, for altitude judgements.
    pub surface_y: i32,
    /// Sea level, so altitude is relative to something.
    pub sea_level: i32,
    /// The block the ground is actually made of. Beats any inference:
    /// if the player is standing on sand, sandstone belongs here
    /// whatever the climate model says.
    pub surface_block: u16,
    /// Fraction of the read region that was water, 0..1.
    pub water: f32,
    /// Time of day in ticks, 0 = dawn. Only used to say whether a build
    /// wants lighting.
    pub time_of_day: f32,
}

impl Default for Ambient {
    fn default() -> Self {
        Self { temperature: 0.5, humidity: 0.5, surface_y: 64, sea_level: 64,
               surface_block: 2, water: 0.0, time_of_day: 0.0 }
    }
}

/// What the site suggests. Every field is a SUGGESTION: an explicit
/// `block=` in the plan always wins, because the player asking for a
/// glass tower in a swamp should get one.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// Palette name from `PALETTES`.
    pub palette: &'static str,
    /// Default structural block when an op names none.
    pub structure: u16,
    /// Default accent, for trim and roofs.
    pub accent: u16,
    /// Default path surface.
    pub path: u16,
    /// Species for `tree`.
    pub species: &'static str,
    /// A human-readable reason, shown in the preview so the choice is
    /// visible and arguable rather than mysterious.
    pub because: String,
}

impl Ambient {
    pub fn altitude(&self) -> i32 { self.surface_y - self.sea_level }

    /// Rough label for the prompt. Not a biome: a description.
    pub fn describe_site(&self) -> String {
        let temp = match self.temperature {
            t if t < 0.2 => "freezing",
            t if t < 0.4 => "cold",
            t if t < 0.65 => "temperate",
            t if t < 0.85 => "warm",
            _ => "hot",
        };
        let wet = match self.humidity {
            h if h < 0.25 => "arid",
            h if h < 0.5 => "dry",
            h if h < 0.75 => "damp",
            _ => "wet",
        };
        let alt = match self.altitude() {
            a if a < -8 => "below sea level",
            a if a < 8 => "near sea level",
            a if a < 40 => "on high ground",
            _ => "at altitude",
        };
        let water = if self.water > 0.15 { ", water nearby" } else { "" };
        format!("{temp}, {wet}, {alt}{water}")
    }

    /// What to build with here.
    /// The suggestion a QUARRY would make: the palette from the rock
    /// under this spot rather than the block on top of it. Stand on
    /// grass with stone two down and every real building here is
    /// stone, because that is what came out of the cellar.
    pub fn suggest_quarried(&self, x: i32, z: i32, seed: u64) -> Suggestion {
        let base = self.suggest();
        let profile = crate::strata::for_palette(base.palette);
        let q = crate::quarry::quarry_palette(profile, x, z, seed, 14);
        Suggestion { structure: q.primary, accent: q.secondary, ..base }
    }

    pub fn suggest(&self) -> Suggestion {
        // The surface block is the strongest signal available and it
        // needs no model: it is what the world already chose for this
        // spot. Climate only decides when the ground is generic.
        let from_ground: Option<(&'static str, &'static str)> = match self.surface_block {
            12 | 48 | 83 => Some(("desert", "cactus")),        // sand, sandstone
            32 | 80 | 31 => Some(("frozen", "pine")),          // snow, ice
            84 | 24 => Some(("mesa", "cactus")),               // terracotta
            38 | 49 | 96 => Some(("volcanic", "pine")),        // basalt, obsidian, magma
            23 => Some(("jungle", "jungle")),                  // moss
            _ => None,
        };

        let (palette, species, why) = match from_ground {
            Some((p, s)) => (p, s, format!("the ground here is {}", block_name(self.surface_block))),
            None => {
                // Two floats, no biome enum: this survives a biome rename
                // and works in a modded world we have never heard of.
                let t = self.temperature;
                let h = self.humidity;
                let (p, s) = if t < 0.25 {
                    ("snowy", "pine")
                } else if t > 0.8 && h < 0.3 {
                    ("desert", "cactus")
                } else if t > 0.7 && h > 0.7 {
                    ("jungle", "jungle")
                } else if h > 0.7 {
                    ("darkwood", "oak")
                } else if t < 0.45 {
                    ("stone", "pine")
                } else {
                    ("plains", "oak")
                };
                (p, s, format!("climate T={t:.2} H={h:.2}"))
            }
        };

        let pals = crate::palettes();
        let (structure, accent) = match pals.get(palette) {
            Some(p) => (p.primary, p.accent),
            None => (1, 58),
        };
        // High ground gets stone regardless of what grows there: timber
        // at altitude reads wrong, and every real vernacular agrees.
        let (structure, why) = if self.altitude() > 48 && from_ground.is_none() {
            (1, format!("{why}, and high enough that stone reads better than timber"))
        } else {
            (structure, why)
        };

        Suggestion {
            palette: leak(palette),
            structure,
            accent,
            path: if self.humidity > 0.6 { 4 } else { 13 },   // cobble when wet, gravel when dry
            species: leak(species),
            because: why,
        }
    }

    /// The line handed to the model. Short: this competes for context
    /// with the region digest, and a small model drowns.
    pub fn prompt_line(&self) -> String {
        self.prompt_line_at(0, 0, 1)
    }

    /// The same, naming what the GROUND HERE is made of. A builder
    /// quarries what is underfoot, so the model is told what is
    /// underfoot rather than only what the surface looks like.
    pub fn prompt_line_at(&self, x: i32, z: i32, seed: u64) -> String {
        let s = self.suggest();
        let profile = crate::strata::for_palette(s.palette);
        let q = crate::quarry::quarry_palette(profile, x, z, seed, 14);
        format!("SITE: {} -- palette {} suits it ({}). The rock here is {} over {} \
                 (block {} and {}); build with it unless the player asks otherwise.",
                self.describe_site(), s.palette, s.because, profile.name,
                profile.at(x, z, 12, seed), q.primary, q.secondary)
    }
}

/// The palette and species names are compile-time strings in every
/// branch; this keeps the signature `&'static str` without threading
/// lifetimes through the caller for no benefit.
fn leak(s: &str) -> &'static str {
    match s {
        "desert" => "desert", "frozen" => "frozen", "mesa" => "mesa",
        "volcanic" => "volcanic", "jungle" => "jungle", "snowy" => "snowy",
        "darkwood" => "darkwood", "stone" => "stone", "plains" => "plains",
        "cactus" => "cactus", "pine" => "pine", "oak" => "oak",
        _ => "stone",
    }
}

fn block_name(b: u16) -> &'static str {
    match b {
        12 => "sand", 48 => "sandstone", 32 => "snow", 80 => "snow",
        31 => "ice", 84 => "terracotta", 38 => "basalt", 49 => "obsidian",
        23 => "moss", 2 => "grass", _ => "stone",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ground_beats_the_climate_model() {
        // Standing on sand in a cold reading still means sandstone. The
        // world already decided what this spot is; a climate inference
        // that contradicts the block underfoot is inference losing to
        // evidence.
        let a = Ambient { temperature: 0.1, humidity: 0.9, surface_block: 12, ..Default::default() };
        let s = a.suggest();
        assert_eq!(s.palette, "desert");
        assert!(s.because.contains("sand"), "{}", s.because);
    }

    #[test]
    fn climate_decides_when_the_ground_is_generic() {
        let grass = 2;
        let hot_dry = Ambient { temperature: 0.9, humidity: 0.1, surface_block: grass,
                                ..Default::default() };
        assert_eq!(hot_dry.suggest().palette, "desert");
        let hot_wet = Ambient { temperature: 0.85, humidity: 0.85, surface_block: grass,
                                ..Default::default() };
        assert_eq!(hot_wet.suggest().palette, "jungle");
        let polar = Ambient { temperature: 0.1, humidity: 0.5, surface_block: grass,
                              ..Default::default() };
        assert_eq!(polar.suggest().palette, "snowy");
        let mild = Ambient { temperature: 0.55, humidity: 0.45, surface_block: grass,
                             ..Default::default() };
        assert_eq!(mild.suggest().palette, "plains");
    }

    #[test]
    fn a_suggestion_always_explains_itself() {
        // A default the player cannot argue with is a default they will
        // fight. The reason goes in the preview.
        for a in [Ambient::default(),
                  Ambient { surface_block: 32, ..Default::default() },
                  Ambient { temperature: 0.95, humidity: 0.05, ..Default::default() }] {
            let s = a.suggest();
            assert!(!s.because.is_empty());
            assert!(a.prompt_line().contains(s.palette), "{}", a.prompt_line());
        }
    }

    #[test]
    fn altitude_prefers_stone_over_timber() {
        let low = Ambient { temperature: 0.55, humidity: 0.45, surface_y: 66,
                            ..Default::default() };
        let high = Ambient { surface_y: 200, ..low };
        assert_ne!(low.suggest().structure, 1, "plains at sea level is not bare stone");
        assert_eq!(high.suggest().structure, 1, "at altitude, stone");
        assert!(high.suggest().because.contains("high enough"));
    }

    #[test]
    fn the_site_description_reads_like_a_place() {
        let a = Ambient { temperature: 0.1, humidity: 0.8, surface_y: 130,
                          sea_level: 64, water: 0.3, ..Default::default() };
        let d = a.describe_site();
        assert!(d.contains("freezing") && d.contains("wet") && d.contains("altitude"));
        assert!(d.contains("water nearby"), "{d}");
    }

    #[test]
    fn a_wet_site_paths_in_cobble_and_a_dry_one_in_gravel() {
        let wet = Ambient { humidity: 0.9, ..Default::default() };
        let dry = Ambient { humidity: 0.2, ..Default::default() };
        assert_eq!(wet.suggest().path, 4);
        assert_eq!(dry.suggest().path, 13);
        assert_ne!(wet.suggest().path, dry.suggest().path);
    }

    #[test]
    fn every_suggested_palette_actually_exists() {
        // A suggestion naming a palette the resolver does not know would
        // silently fall back to stone, which is the bug this module
        // exists to fix.
        let pals = crate::palettes();
        for t in [0.0, 0.2, 0.5, 0.8, 1.0] {
            for h in [0.0, 0.3, 0.6, 1.0] {
                for g in [2u16, 12, 32, 84, 38, 23] {
                    let a = Ambient { temperature: t, humidity: h, surface_block: g,
                                      ..Default::default() };
                    let s = a.suggest();
                    assert!(pals.contains_key(s.palette),
                            "T={t} H={h} ground={g} suggested unknown palette '{}'", s.palette);
                }
            }
        }
    }

    #[test]
    fn species_are_ones_the_tree_op_accepts() {
        const KNOWN: &[&str] = &["oak", "birch", "pine", "cactus", "jungle"];
        for t in [0.0, 0.5, 1.0] {
            for h in [0.0, 0.5, 1.0] {
                let a = Ambient { temperature: t, humidity: h, ..Default::default() };
                assert!(KNOWN.contains(&a.suggest().species),
                        "unknown species '{}'", a.suggest().species);
            }
        }
    }

    #[test]
    fn the_prompt_line_names_the_rock_underfoot() {
        let l = Ambient { surface_block: 2, ..Default::default() }.prompt_line_at(0, 0, 1);
        assert!(l.contains("The rock here is temperate"), "{l}");
        assert!(l.contains("build with it"), "{l}");
        assert!(l.len() < 300, "still short enough for a small model: {} chars", l.len());
    }

    #[test]
    fn the_quarried_suggestion_uses_the_rock_not_the_turf() {
        let grass = Ambient { surface_block: 2, ..Default::default() };
        let plain = grass.suggest();
        let quarried = grass.suggest_quarried(0, 0, 1);
        assert_eq!(quarried.palette, plain.palette, "the same site, the same palette name");
        assert_eq!(quarried.structure, 1, "stone from below, not the grass on top");
        assert_ne!(quarried.structure, quarried.accent);
        // in the desert both agree: the surface IS the rock there
        let sand = Ambient { surface_block: 12, ..Default::default() };
        assert_eq!(sand.suggest_quarried(0, 0, 1).structure, 24, "sandstone under the sand");
    }
}
