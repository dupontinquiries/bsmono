//! level.rs — the procedural arena. This is the `shooter_map.rs` of the GLM-5 port, kept faithful:
//! a tile grid of `Cell`s, a seeded xorshift `Rng`, `Rect`/`Room`, a `MapConfig` with the same knobs,
//! BSP room placement, corridor carving, and a `build_meshes()` that turns the grid into themed `Mesh3`
//! walls / floors / ceilings — emitting wall faces only where a floor tile meets non-floor, so we never
//! waste triangles on hidden backs.
//!
//! Everything downstream (player collision, enemy navigation, bullet raycasts) reads this one grid, and
//! everything visible is `Vec<Mesh3>` that flows through the same renderer as the reference.

use crate::math::{Vec2, Vec3};
use crate::mesh::{
    boxx, cairn, crystal, disc, dither, hash01, leaf_cluster, lerp_c, light_shaft, panel, push_quad_grid, ramp, rgb, rock, rotate_y_about, scale_xz, set_alpha, shade_ao, tint_height,
    tint_up, tree, tree_styled, tube, uv_sphere, Mesh3, FACE_ALL, FACE_NO_BOTTOM, FACE_TOP,
};
use egui::Color32;
use std::cell::Cell as StdCell;

thread_local! {
    /// Procgen "texture resolution": how finely floor tiles are subdivided for per-vertex tinting/dither.
    /// 1.0 = one quad per tile; higher splits each tile into an N×N grid for finer surface detail.
    static TEX_DETAIL: StdCell<f32> = const { StdCell::new(1.0) };
}
/// Set the procedural-texture resolution multiplier (clamped).
pub fn set_texture_detail(d: f32) {
    TEX_DETAIL.with(|c| c.set(d.clamp(1.0, 8.0)));
}
fn tex_subdiv() -> i32 {
    TEX_DETAIL.with(|c| c.get().round() as i32).clamp(1, 8)
}

// ── RNG ──────────────────────────────────────────────────────────────────────────

/// A small deterministic xorshift* generator — same role as the reference `Rng`.
#[derive(Clone)]
pub struct Rng {
    state: u64,
}
impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed ^ 0x9E37_79B9_7F4A_7C15 | 1 }
    }
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform 0.0..1.0.
    #[inline]
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Inclusive-low, exclusive-high integer range; returns `lo` if the range is empty.
    pub fn i32_range(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % (hi - lo) as u64) as i32
    }
    /// True with probability `p`.
    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }
}

// ── grid ───────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    /// Unexcavated solid — outside the playable space (blocks movement & bullets).
    Solid,
    /// Walkable ground.
    Floor,
    /// A bounding wall tile (blocks movement & bullets, gets wall geometry).
    Wall,
    /// An open pit — no floor; step off the edge and you fall.
    Void,
}
impl Cell {
    #[inline]
    pub fn walkable(self) -> bool {
        matches!(self, Cell::Floor)
    }
    /// Does this cell stop a bullet / a body?
    #[inline]
    pub fn blocks(self) -> bool {
        matches!(self, Cell::Wall | Cell::Solid)
    }
}

/// A tile-space rectangle `[x0,x1) × [z0,z1)`.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x0: i32,
    pub z0: i32,
    pub x1: i32,
    pub z1: i32,
}
impl Rect {
    pub fn new(x0: i32, z0: i32, x1: i32, z1: i32) -> Self {
        Self { x0, z0, x1, z1 }
    }
    pub fn w(&self) -> i32 {
        self.x1 - self.x0
    }
    pub fn h(&self) -> i32 {
        self.z1 - self.z0
    }
    pub fn center(&self) -> (i32, i32) {
        ((self.x0 + self.x1) / 2, (self.z0 + self.z1) / 2)
    }
    pub fn inset(&self, n: i32) -> Rect {
        Rect::new(self.x0 + n, self.z0 + n, self.x1 - n, self.z1 - n)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MapTheme {
    Industrial,
    Bunker,
    Warehouse,
    Urban,
    Military,
    Volcanic,
    Ice,
    Alien,
    /// Logscope "Canopy": a mossy petrified-stone grove lit by coloured skylight.
    Canopy,
    /// Logscope "Mistfall": deep teal water terraces drowned in fog.
    Mistfall,
    /// "Toxic": an overgrown swamp of acid-green sludge terraces — an organic biome whose hazard burns.
    Toxic,
    /// "Meadow": a bright sunlit grassland — rolling green hills, wildflowers, sparse trees.
    Meadow,
    /// "Redwood": a towering old-growth forest — dim, deep-shadowed, immense rust-red trunks.
    Redwood,
    /// "Station": the exterior deck of a vast spaceship in orbit — hull plating, service modules, and
    /// landing platforms, ringed by the void of space.
    Station,
    /// "Carrier": the flight deck of a vast warship at sea — a long steel runway ringed by ocean, with a
    /// superstructure island, deck markings, and cable netting at the edge.
    Carrier,
}

/// A curated, named map: a base biome plus size/density overrides. The map selector lists these so players
/// pick "Copper District" or "Glacier Watch" rather than a raw biome — 24 hand-tuned variations over the
/// ten biomes, differing in scale, openness, and pit/cover density so each plays distinctly.
#[derive(Clone, Copy)]
pub struct MapPreset {
    pub name: &'static str,
    pub theme: MapTheme,
    pub size: i32,
    pub rooms: usize,
    pub cover: f32,
    pub pit: f32,
    pub lanes: u8,
}

impl MapPreset {
    /// Bake this preset (plus a seed) into a concrete MapConfig.
    pub fn config(&self, seed: u64) -> MapConfig {
        let mut c = MapConfig::default();
        c.seed = seed;
        c.theme = self.theme;
        c.size = self.size;
        c.room_count = self.rooms;
        c.cover_density = self.cover;
        c.pit_chance = self.pit;
        c.lanes = self.lanes;
        // Named archetype maps pin their signature centerpiece (the index matches the biome's landmark set).
        c.landmark = match self.name {
            "Frozen Bifrost" => Some(0),
            "Titan's Ribcage" => Some(4),
            "Dragon Canyon" => Some(5),
            "Sandstone Gauntlet" => Some(5),
            "Colosseum Trenches" => Some(6),
            "Culvert Breach" => None,          // the two-tier wall IS the centerpiece (built in build_culvert)
            "Fallen Capital" => Some(8),       // fallen dome (Urban/Military set)
            "Slaughter Camp" => Some(7),       // toppled siege camp (open-biome set)
            "Blighted Watchtower" => Some(8),  // rune watchtower ring (open-biome set)
            "Hollow Forest" => Some(9),        // glowing gel core (open-biome set)
            "Cryo Lab" => Some(2),             // overturned cryo-tank (Ice set)
            "Salt Mine" => Some(5),            // rusted ore cart (industrial set)
            "Sunken Villa" => Some(9),         // dry fountain (Urban/Military set)
            "Server Tomb" => Some(1),          // cooling vat (Station set)
            "Rusting Midway" => Some(10),      // dangling gondola (open-biome set)
            "Ghost Terminus" => Some(10),      // derailed subway car (Urban/Military set)
            "Chasm Span" => Some(4),           // a compact central waystone on the span (reactor plinth)
            "Spillway Dam" => Some(2),         // concrete gravity dam (Volcanic set)
            "Rail Yard" => Some(7),            // railway + platform (industrial set)
            "Plaza Fountain" => Some(11),      // working tiered fountain (Urban/Military set)
            "Frozen Lake" => Some(3),          // ice floes + snowdrifts (Ice set)
            "Cliff Pass" => Some(10),          // rock cliff with mid-ledge (open-biome set)
            _ => None,
        };
        // The Colosseum is a true circular arena, walled high so the ring of arches reads as a grand stadium.
        if self.name == "Colosseum Trenches" {
            c.arena = true;
            c.wall_height = 8.5;
        }
        if self.name == "Flood Zone" {
            c.authored = Some(0); // hand-built dam/basin map with timed flood channels
        }
        if self.name == "Reference Town" {
            c.authored = Some(1); // blueprint stamped from a real town build
        }
        if self.name == "Desert Town" {
            c.authored = Some(2); // authored clay town: courtyard, lanes, market, quake, sandstorm
        }
        if self.name == "Pyre Garage" {
            c.authored = Some(3); // burning multi-story car park: spiral ramps, car cover, fire & smoke
        }
        if self.name == "Concrete Slab" {
            c.authored = Some(4); // brutalist housing: external walkways, rubble courtyard, CQB maze
        }
        if self.name == "Redwood Canyon" {
            c.authored = Some(5); // forested river gorge: giant redwoods, log bridges, a fordable river
        }
        if self.name == "Malecon Breakwater" {
            c.authored = Some(6); // Havana seawall promenade: ocean lane, pastel colonial flank, burned car
        }
        if self.name == "Spire Foundry" {
            c.authored = Some(7); // multi-level foundry: stacked catwalk floors, atrium, ramps & stairs
        }
        // The Chasm Span is the bridge-over-the-abyss layout; the Watchtower is a circular summit arena.
        if self.name == "Chasm Span" {
            c.bridge = true;
        }
        if self.name == "Blighted Watchtower" {
            c.arena = true;
            c.side_lanes = true; // the role-model arena: open bowl + two covered flanking lanes
        }
        if matches!(self.name, "Sunken Villa" | "Rusting Midway") {
            c.arena = true; // sunken courtyard / deadly central loop = an exposed bowl with rim cover
        }
        // The Culvert Breach is the two-tier siege layout — a tall wall-walk over a sunken flooded trench.
        if self.name == "Culvert Breach" {
            c.culvert = true;
            c.wall_height = 5.2;
        }
        // Factory maps: their plan is skinned from a high-scoring topology graph instead of biome carving.
        if matches!(self.name, "Proving Ground" | "Crossfire Mesa") {
            c.graph = true;
        }
        if self.name == "Berthing Docks" {
            c.docks = true;
        }
        if self.name == "Contested Ground" {
            c.agents = true;
        }
        if matches!(self.name, "Triad Foundry" | "Vanguard Reach" | "Pinnacle") {
            c.triplane = true;
        }
        c
    }
}

pub const PRESETS: [MapPreset; 91] = [
    MapPreset { name: "Copper District", theme: MapTheme::Industrial, size: 152, rooms: 34, cover: 2.8, pit: 0.16, lanes: 0 },
    MapPreset { name: "Foundry Night", theme: MapTheme::Industrial, size: 120, rooms: 26, cover: 3.2, pit: 0.10, lanes: 0 },
    MapPreset { name: "Iron Works", theme: MapTheme::Industrial, size: 168, rooms: 40, cover: 2.4, pit: 0.20, lanes: 0 },
    MapPreset { name: "Deep Bunker", theme: MapTheme::Bunker, size: 112, rooms: 28, cover: 3.4, pit: 0.08, lanes: 0 },
    MapPreset { name: "Silo Complex", theme: MapTheme::Bunker, size: 152, rooms: 34, cover: 2.6, pit: 0.14, lanes: 0 },
    MapPreset { name: "Cargo Bay", theme: MapTheme::Warehouse, size: 136, rooms: 30, cover: 3.6, pit: 0.06, lanes: 0 },
    MapPreset { name: "Container Yard", theme: MapTheme::Warehouse, size: 168, rooms: 38, cover: 3.0, pit: 0.12, lanes: 0 },
    MapPreset { name: "Downtown", theme: MapTheme::Urban, size: 160, rooms: 36, cover: 2.6, pit: 0.10, lanes: 0 },
    MapPreset { name: "Market Row", theme: MapTheme::Urban, size: 128, rooms: 30, cover: 3.2, pit: 0.06, lanes: 0 },
    MapPreset { name: "Skyline", theme: MapTheme::Urban, size: 184, rooms: 44, cover: 2.2, pit: 0.14, lanes: 0 },
    MapPreset { name: "Forward Outpost", theme: MapTheme::Military, size: 144, rooms: 32, cover: 2.8, pit: 0.16, lanes: 0 },
    MapPreset { name: "Firebase", theme: MapTheme::Military, size: 176, rooms: 40, cover: 2.4, pit: 0.20, lanes: 0 },
    MapPreset { name: "Pyrefall", theme: MapTheme::Volcanic, size: 168, rooms: 34, cover: 2.6, pit: 0.22, lanes: 0 },
    MapPreset { name: "Magma Vents", theme: MapTheme::Volcanic, size: 128, rooms: 26, cover: 2.2, pit: 0.18, lanes: 0 },
    MapPreset { name: "Glacier Watch", theme: MapTheme::Ice, size: 160, rooms: 32, cover: 2.4, pit: 0.18, lanes: 0 },
    MapPreset { name: "Frostbite", theme: MapTheme::Ice, size: 136, rooms: 28, cover: 2.0, pit: 0.24, lanes: 0 },
    MapPreset { name: "Monastery", theme: MapTheme::Ice, size: 184, rooms: 36, cover: 2.8, pit: 0.12, lanes: 0 },
    MapPreset { name: "Verdant Ruins", theme: MapTheme::Alien, size: 160, rooms: 30, cover: 2.6, pit: 0.20, lanes: 0 },
    MapPreset { name: "Deep Jungle", theme: MapTheme::Alien, size: 192, rooms: 36, cover: 3.0, pit: 0.16, lanes: 0 },
    MapPreset { name: "Canopy Reach", theme: MapTheme::Canopy, size: 168, rooms: 32, cover: 2.8, pit: 0.18, lanes: 0 },
    MapPreset { name: "Hanging Gardens", theme: MapTheme::Canopy, size: 136, rooms: 26, cover: 3.2, pit: 0.12, lanes: 0 },
    MapPreset { name: "Mistfall", theme: MapTheme::Mistfall, size: 176, rooms: 32, cover: 2.4, pit: 0.24, lanes: 0 },
    MapPreset { name: "Glass Tide", theme: MapTheme::Mistfall, size: 200, rooms: 38, cover: 2.0, pit: 0.20, lanes: 0 },
    MapPreset { name: "Drowned Quarter", theme: MapTheme::Mistfall, size: 144, rooms: 28, cover: 2.8, pit: 0.28, lanes: 0 },
    MapPreset { name: "The Mire", theme: MapTheme::Toxic, size: 168, rooms: 30, cover: 2.6, pit: 0.30, lanes: 0 },
    MapPreset { name: "Acid Wash", theme: MapTheme::Toxic, size: 144, rooms: 26, cover: 2.9, pit: 0.34, lanes: 0 },
    // ── Open-world maps: big and sparse, so the wilds (terrain, thickets, trails, streams) dominate. ──
    MapPreset { name: "Greenwild Expanse", theme: MapTheme::Canopy, size: 212, rooms: 16, cover: 2.0, pit: 0.10, lanes: 0 },
    MapPreset { name: "The Sunken Vast", theme: MapTheme::Mistfall, size: 212, rooms: 16, cover: 1.8, pit: 0.16, lanes: 0 },
    MapPreset { name: "Mire Frontier", theme: MapTheme::Toxic, size: 208, rooms: 15, cover: 2.0, pit: 0.20, lanes: 0 },
    MapPreset { name: "Wildwood", theme: MapTheme::Alien, size: 212, rooms: 16, cover: 2.2, pit: 0.12, lanes: 0 },
    MapPreset { name: "Sunfield", theme: MapTheme::Meadow, size: 176, rooms: 20, cover: 1.8, pit: 0.08, lanes: 0 },
    MapPreset { name: "Open Prairie", theme: MapTheme::Meadow, size: 212, rooms: 15, cover: 1.5, pit: 0.06, lanes: 0 },
    MapPreset { name: "Redwood Grove", theme: MapTheme::Redwood, size: 184, rooms: 22, cover: 2.4, pit: 0.10, lanes: 0 },
    MapPreset { name: "Old Growth", theme: MapTheme::Redwood, size: 212, rooms: 16, cover: 2.2, pit: 0.12, lanes: 0 },
    // ── Large-scale tower-defense lane maps (multi-lane clear paths through connecting jungle) ──────────
    MapPreset { name: "Triple Canopy", theme: MapTheme::Canopy, size: 340, rooms: 22, cover: 2.6, pit: 0.06, lanes: 3 },
    MapPreset { name: "Jungle Gauntlet", theme: MapTheme::Redwood, size: 320, rooms: 20, cover: 2.8, pit: 0.05, lanes: 3 },
    MapPreset { name: "Verdant Lanes", theme: MapTheme::Meadow, size: 300, rooms: 22, cover: 1.8, pit: 0.04, lanes: 4 },
    MapPreset { name: "Overgrowth Siege", theme: MapTheme::Toxic, size: 320, rooms: 20, cover: 2.6, pit: 0.05, lanes: 3 },
    // ── Space-station exterior battle decks ────────────────────────────────────────────────────────────
    MapPreset { name: "Orbital Drydock", theme: MapTheme::Station, size: 200, rooms: 5, cover: 2.4, pit: 0.0, lanes: 0 },
    MapPreset { name: "Hammerhead Station", theme: MapTheme::Station, size: 260, rooms: 5, cover: 2.8, pit: 0.0, lanes: 0 },
    // ── Extra variety: size / texture / cover mixes across the biome families ────────────────────────────
    MapPreset { name: "Hairpin Foundry", theme: MapTheme::Industrial, size: 184, rooms: 46, cover: 3.6, pit: 0.22, lanes: 0 },
    MapPreset { name: "Tideworks", theme: MapTheme::Warehouse, size: 172, rooms: 30, cover: 2.2, pit: 0.10, lanes: 0 },
    MapPreset { name: "Black Glass", theme: MapTheme::Volcanic, size: 200, rooms: 14, cover: 2.0, pit: 0.30, lanes: 0 },
    MapPreset { name: "Whiteout", theme: MapTheme::Ice, size: 188, rooms: 12, cover: 1.6, pit: 0.18, lanes: 0 },
    MapPreset { name: "Spore Reach", theme: MapTheme::Toxic, size: 196, rooms: 16, cover: 2.4, pit: 0.12, lanes: 0 },
    MapPreset { name: "Hollow Crown", theme: MapTheme::Alien, size: 208, rooms: 15, cover: 2.6, pit: 0.16, lanes: 0 },
    MapPreset { name: "Wildflower Run", theme: MapTheme::Meadow, size: 240, rooms: 18, cover: 1.4, pit: 0.04, lanes: 0 },
    MapPreset { name: "Cathedral Pines", theme: MapTheme::Redwood, size: 232, rooms: 16, cover: 2.0, pit: 0.08, lanes: 0 },
    MapPreset { name: "Foglands", theme: MapTheme::Mistfall, size: 216, rooms: 17, cover: 2.2, pit: 0.10, lanes: 0 },
    MapPreset { name: "Switchback Ridge", theme: MapTheme::Military, size: 176, rooms: 32, cover: 3.0, pit: 0.20, lanes: 0 },
    // larger lane decks for big tower-defense fights
    MapPreset { name: "Canopy Highway", theme: MapTheme::Canopy, size: 360, rooms: 24, cover: 2.6, pit: 0.05, lanes: 4 },
    MapPreset { name: "Drydock Approach", theme: MapTheme::Station, size: 300, rooms: 5, cover: 2.6, pit: 0.0, lanes: 0 },
    // ── Carrier flight decks (linear long-range duels over open ocean) ──────────────────────────────────
    MapPreset { name: "Flight Deck", theme: MapTheme::Carrier, size: 220, rooms: 3, cover: 2.0, pit: 0.0, lanes: 0 },
    MapPreset { name: "Leviathan", theme: MapTheme::Carrier, size: 280, rooms: 3, cover: 2.2, pit: 0.0, lanes: 0 },
    // ── Signature archetypes: each pins a bespoke centerpiece (forced in MapPreset::config by name) ───────
    MapPreset { name: "Frozen Bifrost", theme: MapTheme::Ice, size: 120, rooms: 22, cover: 2.2, pit: 0.20, lanes: 0 },
    MapPreset { name: "Titan's Ribcage", theme: MapTheme::Canopy, size: 168, rooms: 26, cover: 2.4, pit: 0.14, lanes: 0 },
    MapPreset { name: "Dragon Canyon", theme: MapTheme::Mistfall, size: 176, rooms: 28, cover: 2.6, pit: 0.18, lanes: 0 },
    MapPreset { name: "Sandstone Gauntlet", theme: MapTheme::Military, size: 160, rooms: 28, cover: 2.2, pit: 0.16, lanes: 0 },
    MapPreset { name: "Colosseum Trenches", theme: MapTheme::Military, size: 96, rooms: 30, cover: 3.2, pit: 0.10, lanes: 0 },
    // ── Tolkien-grit archetypes: ruined fantasy locales as gritty arenas (signature setpieces + layouts) ──
    MapPreset { name: "Culvert Breach", theme: MapTheme::Military, size: 140, rooms: 26, cover: 3.0, pit: 0.08, lanes: 0 },
    MapPreset { name: "Chasm Span", theme: MapTheme::Bunker, size: 120, rooms: 22, cover: 2.2, pit: 0.0, lanes: 0 },
    MapPreset { name: "Fallen Capital", theme: MapTheme::Urban, size: 168, rooms: 36, cover: 2.6, pit: 0.10, lanes: 0 },
    MapPreset { name: "Slaughter Camp", theme: MapTheme::Toxic, size: 150, rooms: 28, cover: 3.0, pit: 0.12, lanes: 0 },
    MapPreset { name: "Blighted Watchtower", theme: MapTheme::Meadow, size: 112, rooms: 24, cover: 2.6, pit: 0.10, lanes: 0 },
    MapPreset { name: "Hollow Forest", theme: MapTheme::Canopy, size: 184, rooms: 22, cover: 2.4, pit: 0.12, lanes: 0 },
    MapPreset { name: "Cryo Lab", theme: MapTheme::Ice, size: 132, rooms: 20, cover: 2.8, pit: 0.06, lanes: 0 },
    MapPreset { name: "Proving Ground", theme: MapTheme::Military, size: 150, rooms: 16, cover: 2.6, pit: 0.10, lanes: 0 },
    MapPreset { name: "Crossfire Mesa", theme: MapTheme::Canopy, size: 156, rooms: 16, cover: 2.4, pit: 0.12, lanes: 0 },
    MapPreset { name: "Salt Mine", theme: MapTheme::Bunker, size: 140, rooms: 22, cover: 2.7, pit: 0.14, lanes: 0 },
    MapPreset { name: "Berthing Docks", theme: MapTheme::Industrial, size: 138, rooms: 18, cover: 2.0, pit: 0.0, lanes: 0 },
    MapPreset { name: "Contested Ground", theme: MapTheme::Meadow, size: 144, rooms: 12, cover: 2.4, pit: 0.0, lanes: 0 },
    MapPreset { name: "Sunken Villa", theme: MapTheme::Urban, size: 132, rooms: 16, cover: 2.6, pit: 0.10, lanes: 0 },
    MapPreset { name: "Server Tomb", theme: MapTheme::Station, size: 140, rooms: 26, cover: 3.0, pit: 0.0, lanes: 0 },
    MapPreset { name: "Rusting Midway", theme: MapTheme::Mistfall, size: 150, rooms: 14, cover: 2.4, pit: 0.06, lanes: 0 },
    MapPreset { name: "Ghost Terminus", theme: MapTheme::Urban, size: 146, rooms: 20, cover: 2.7, pit: 0.0, lanes: 2 },
    MapPreset { name: "Triad Foundry", theme: MapTheme::Industrial, size: 120, rooms: 8, cover: 2.0, pit: 0.0, lanes: 0 },
    MapPreset { name: "Vanguard Reach", theme: MapTheme::Meadow, size: 128, rooms: 8, cover: 2.0, pit: 0.0, lanes: 0 },
    MapPreset { name: "Pinnacle", theme: MapTheme::Military, size: 124, rooms: 8, cover: 2.0, pit: 0.0, lanes: 0 },
    MapPreset { name: "Spillway Dam", theme: MapTheme::Volcanic, size: 150, rooms: 18, cover: 2.4, pit: 0.06, lanes: 0 },
    MapPreset { name: "Rail Yard", theme: MapTheme::Industrial, size: 150, rooms: 22, cover: 2.7, pit: 0.0, lanes: 0 },
    MapPreset { name: "Plaza Fountain", theme: MapTheme::Urban, size: 140, rooms: 20, cover: 2.6, pit: 0.0, lanes: 0 },
    MapPreset { name: "Frozen Lake", theme: MapTheme::Ice, size: 150, rooms: 16, cover: 2.3, pit: 0.0, lanes: 0 },
    MapPreset { name: "Cliff Pass", theme: MapTheme::Redwood, size: 150, rooms: 14, cover: 2.2, pit: 0.05, lanes: 0 },
    MapPreset { name: "Flood Zone", theme: MapTheme::Industrial, size: 112, rooms: 8, cover: 2.0, pit: 0.0, lanes: 0 },
    MapPreset { name: "Reference Town", theme: MapTheme::Urban, size: 132, rooms: 8, cover: 2.0, pit: 0.0, lanes: 0 },
    MapPreset { name: "Desert Town", theme: MapTheme::Urban, size: 116, rooms: 8, cover: 2.0, pit: 0.0, lanes: 0 },
    MapPreset { name: "Pyre Garage", theme: MapTheme::Industrial, size: 108, rooms: 8, cover: 2.0, pit: 0.0, lanes: 0 },
    MapPreset { name: "Concrete Slab", theme: MapTheme::Urban, size: 108, rooms: 8, cover: 2.0, pit: 0.0, lanes: 0 },
    MapPreset { name: "Redwood Canyon", theme: MapTheme::Redwood, size: 116, rooms: 8, cover: 2.0, pit: 0.0, lanes: 0 },
    MapPreset { name: "Malecon Breakwater", theme: MapTheme::Urban, size: 120, rooms: 8, cover: 2.0, pit: 0.0, lanes: 2 },
    MapPreset { name: "Spire Foundry", theme: MapTheme::Industrial, size: 108, rooms: 8, cover: 2.0, pit: 0.0, lanes: 0 },
];

/// Per-theme low-poly palette (no textures — just tinted vertex colour, the reference's approach).
/// `accent` is the emissive trim/lamp colour (glowing strips, crystal glow, lava); `rock` tints the
/// Roll a subtle seed-derived palette variant so two maps of the same biome don't look identical: a warm,
/// cool, weathered, or verdant cast over the surface roles. Floor↔wall contrast is checked with WCAG
/// luminance (logscope palette.rs) and the variant is dropped back to base if it would crush legibility.
pub fn palette_variant(base: Palette, seed: u64) -> Palette {
    let pick = (seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 61) % 5;
    if pick == 0 {
        return base; // some maps stay canonical
    }
    let (rm, gm, bm, desat) = match pick {
        1 => (1.10f32, 1.02f32, 0.90f32, 0.0f32), // warm / sunlit
        2 => (0.92, 0.99, 1.13, 0.0),             // cool / overcast
        3 => (1.0, 1.0, 1.0, 0.34),               // weathered / desaturated
        _ => (0.95, 1.08, 0.95, 0.10),            // verdant / mossy
    };
    let shift = |c: Color32| -> Color32 {
        let lum = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
        let mix = |ch: f32, m: f32| {
            let v = ch * m;
            (v + (lum - v) * desat).clamp(0.0, 255.0) as u8
        };
        Color32::from_rgb(mix(c.r() as f32, rm), mix(c.g() as f32, gm), mix(c.b() as f32, bm))
    };
    let mut p = base;
    p.floor = shift(base.floor);
    p.wall = shift(base.wall);
    p.trim = shift(base.trim);
    p.ceil = shift(base.ceil);
    p.cover = shift(base.cover);
    p.rock = shift(base.rock);
    p.accent = shift(base.accent);
    if wcag_contrast(p.floor, p.wall) < wcag_contrast(base.floor, base.wall) * 0.7 {
        return base; // would crush the floor/wall read — keep the canonical scheme
    }
    p
}

/// WCAG relative-luminance contrast ratio (1..21). Used to keep palette variants legible.
fn wcag_contrast(a: Color32, b: Color32) -> f32 {
    let lum = |c: Color32| {
        let lin = |v: u8| {
            let s = v as f32 / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    };
    let (la, lb) = (lum(a), lum(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// faceted stone props.
#[derive(Clone, Copy)]
pub struct Palette {
    pub floor: Color32,
    pub wall: Color32,
    pub trim: Color32,
    pub ceil: Color32,
    pub cover: Color32,
    pub sky: Color32,
    pub fog: Color32,
    pub accent: Color32,
    pub rock: Color32,
}
impl MapTheme {
    /// Every biome, for the map selector.
    pub const ALL: [MapTheme; 15] = [
        MapTheme::Industrial,
        MapTheme::Bunker,
        MapTheme::Warehouse,
        MapTheme::Urban,
        MapTheme::Military,
        MapTheme::Volcanic,
        MapTheme::Ice,
        MapTheme::Alien,
        MapTheme::Canopy,
        MapTheme::Mistfall,
        MapTheme::Toxic,
        MapTheme::Meadow,
        MapTheme::Redwood,
        MapTheme::Station,
        MapTheme::Carrier,
    ];

    /// Lush organic biomes that read as open-air wilderness: no enclosing ceiling, full-height trees, the
    /// skybox showing overhead. These route through the layered "wilds" scatter rather than the room kit.
    /// The tree silhouette this biome grows — so canopy broadleaf, redwood/ice conifers, toxic snags and alien
    /// fronds don't all share one shape.
    pub fn tree_kind(self) -> crate::mesh::TreeKind {
        use crate::mesh::TreeKind as T;
        match self {
            MapTheme::Redwood | MapTheme::Ice | MapTheme::Mistfall => T::Conifer,
            MapTheme::Toxic => T::Snag,
            MapTheme::Alien => T::Frond,
            _ => T::Broadleaf,
        }
    }

    /// The (floor, wall) materials this biome's big surfaces wear. Two candidates each, chosen by seed so maps
    /// of the same biome don't all look identical — the source of cross-map surface variety.
    pub fn materials(self, seed: u64) -> (crate::mesh::Material, crate::mesh::Material) {
        use crate::mesh::Material as M;
        let pick = (seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 60) & 1 == 1;
        match self {
            MapTheme::Industrial => (if pick { M::Gravel } else { M::Concrete }, if pick { M::RustPlate } else { M::Corrugated }),
            MapTheme::Bunker => (M::Concrete, if pick { M::StoneBlock } else { M::Concrete }),
            MapTheme::Warehouse => (if pick { M::Concrete } else { M::Planks }, M::Corrugated),
            MapTheme::Urban => (if pick { M::Tile } else { M::Concrete }, if pick { M::Brick } else { M::Concrete }),
            MapTheme::Military => (if pick { M::Gravel } else { M::Dirt }, M::Concrete),
            MapTheme::Volcanic => (M::StoneBlock, if pick { M::StoneBlock } else { M::RustPlate }),
            MapTheme::Ice => (M::Snow, if pick { M::StoneBlock } else { M::Concrete }),
            MapTheme::Alien => (M::Organic, M::Organic),
            MapTheme::Canopy => (if pick { M::Grass } else { M::Dirt }, M::StoneBlock),
            MapTheme::Mistfall => (M::Dirt, if pick { M::StoneBlock } else { M::Cobble }),
            MapTheme::Toxic => (if pick { M::Dirt } else { M::Concrete }, M::RustPlate),
            MapTheme::Meadow => (M::Grass, if pick { M::StoneBlock } else { M::Cobble }),
            MapTheme::Redwood => (if pick { M::Dirt } else { M::Grass }, M::Planks),
            MapTheme::Station => (M::Tile, if pick { M::Corrugated } else { M::Tile }),
            MapTheme::Carrier => (M::Tile, M::Corrugated),
        }
    }

    pub fn open_sky(self) -> bool {
        matches!(self, MapTheme::Canopy | MapTheme::Mistfall | MapTheme::Alien | MapTheme::Toxic | MapTheme::Meadow | MapTheme::Redwood | MapTheme::Station | MapTheme::Carrier)
    }

    /// Human-readable map name (Alien renders as a night jungle).
    pub fn name(self) -> &'static str {
        match self {
            MapTheme::Industrial => "Foundry",
            MapTheme::Bunker => "Bunker",
            MapTheme::Warehouse => "Warehouse",
            MapTheme::Urban => "Downtown",
            MapTheme::Military => "Outpost",
            MapTheme::Volcanic => "Magma",
            MapTheme::Ice => "Glacier",
            MapTheme::Alien => "Jungle",
            MapTheme::Canopy => "Canopy",
            MapTheme::Mistfall => "Mistfall",
            MapTheme::Toxic => "Mire",
            MapTheme::Meadow => "Meadow",
            MapTheme::Redwood => "Redwood",
            MapTheme::Station => "Station",
            MapTheme::Carrier => "Carrier",
        }
    }

    /// Rotate biomes by seed so successive launches feel like different places.
    pub fn from_seed(seed: u64) -> MapTheme {
        MapTheme::ALL[(seed % MapTheme::ALL.len() as u64) as usize]
    }

    /// Stable index into `ALL`, for saving themes to disk.
    pub fn id(self) -> u8 {
        MapTheme::ALL.iter().position(|&t| t == self).unwrap_or(0) as u8
    }

    /// Inverse of `id` (clamped to a valid biome).
    pub fn from_id(i: u8) -> MapTheme {
        MapTheme::ALL[(i as usize).min(MapTheme::ALL.len() - 1)]
    }

    /// How strongly the cover props glow (only the lava world keeps a faint ember glow now).
    pub fn cover_emissive(self) -> f32 {
        match self {
            MapTheme::Volcanic => 0.35,
            _ => 0.0,
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            MapTheme::Industrial => Palette {
                floor: rgb(70, 72, 78),
                wall: rgb(96, 98, 104),
                trim: rgb(150, 120, 60),
                ceil: rgb(48, 50, 56),
                cover: rgb(120, 90, 48),
                sky: rgb(40, 44, 54),
                fog: rgb(28, 30, 38),
                accent: rgb(255, 176, 64),
                rock: rgb(96, 96, 102),
            },
            MapTheme::Bunker => Palette {
                floor: rgb(78, 76, 70),
                wall: rgb(104, 100, 92),
                trim: rgb(70, 100, 70),
                ceil: rgb(56, 54, 50),
                cover: rgb(96, 100, 86),
                sky: rgb(34, 36, 38),
                fog: rgb(24, 26, 26),
                accent: rgb(90, 220, 120),
                rock: rgb(98, 96, 88),
            },
            MapTheme::Warehouse => Palette {
                floor: rgb(96, 92, 84),
                wall: rgb(120, 116, 108),
                trim: rgb(150, 84, 60),
                ceil: rgb(66, 64, 60),
                cover: rgb(150, 110, 60),
                sky: rgb(60, 64, 72),
                fog: rgb(40, 42, 48),
                accent: rgb(220, 120, 70),
                rock: rgb(112, 106, 98),
            },
            MapTheme::Urban => Palette {
                floor: rgb(84, 86, 90),
                wall: rgb(110, 108, 104),
                trim: rgb(80, 110, 140),
                ceil: rgb(120, 128, 150),
                cover: rgb(96, 100, 104),
                sky: rgb(120, 140, 175),
                fog: rgb(150, 165, 195),
                accent: rgb(120, 190, 255),
                rock: rgb(104, 104, 110),
            },
            MapTheme::Military => Palette {
                floor: rgb(72, 78, 64),
                wall: rgb(92, 98, 82),
                trim: rgb(120, 110, 60),
                ceil: rgb(54, 58, 48),
                cover: rgb(100, 104, 78),
                sky: rgb(70, 80, 70),
                fog: rgb(46, 52, 44),
                accent: rgb(200, 210, 90),
                rock: rgb(90, 94, 80),
            },
            MapTheme::Volcanic => Palette {
                floor: rgb(46, 38, 36),
                wall: rgb(58, 44, 42),
                trim: rgb(170, 60, 30),
                ceil: rgb(30, 24, 24),
                cover: rgb(40, 32, 30),
                sky: rgb(40, 20, 16),
                fog: rgb(34, 16, 12),
                accent: rgb(255, 120, 40),
                rock: rgb(44, 36, 34),
            },
            MapTheme::Ice => Palette {
                floor: rgb(250, 252, 254),
                wall: rgb(176, 196, 216),
                trim: rgb(120, 180, 220),
                ceil: rgb(150, 172, 196),
                cover: rgb(170, 196, 216),
                sky: rgb(176, 206, 230),
                fog: rgb(196, 216, 232),
                accent: rgb(150, 226, 255),
                rock: rgb(168, 192, 214),
            },
            MapTheme::Alien => Palette {
                floor: rgb(42, 48, 36),
                wall: rgb(56, 62, 46),
                trim: rgb(96, 112, 72),
                ceil: rgb(28, 32, 24),
                cover: rgb(52, 58, 40),
                sky: rgb(40, 52, 42),
                fog: rgb(28, 36, 28),
                accent: rgb(150, 178, 110),
                rock: rgb(58, 62, 48),
            },
            // Logscope Canopy: dark polished stone floor, mossy petrified walls, warm stained-glass accent.
            MapTheme::Canopy => Palette {
                floor: rgb(58, 50, 40),
                wall: rgb(104, 100, 90),
                trim: rgb(110, 144, 64),
                ceil: rgb(36, 32, 28),
                cover: rgb(92, 96, 76),
                sky: rgb(72, 92, 70),
                fog: rgb(40, 46, 38),
                accent: rgb(230, 194, 84),
                rock: rgb(104, 100, 90),
            },
            // Logscope Mistfall: deep teal terraces, pale stone rings, drowned in cool fog.
            MapTheme::Mistfall => Palette {
                floor: rgb(28, 48, 54),
                wall: rgb(44, 70, 78),
                trim: rgb(120, 168, 178),
                ceil: rgb(18, 32, 38),
                cover: rgb(40, 62, 70),
                sky: rgb(96, 134, 142),
                fog: rgb(70, 104, 112),
                accent: rgb(154, 208, 220),
                rock: rgb(46, 66, 72),
            },
            // Toxic mire: sickly green sludge, dark mottled banks, ochre haze.
            MapTheme::Toxic => Palette {
                floor: rgb(46, 56, 34),
                wall: rgb(52, 60, 40),
                trim: rgb(150, 178, 70),
                ceil: rgb(24, 30, 20),
                cover: rgb(58, 66, 40),
                sky: rgb(120, 130, 78),
                fog: rgb(96, 110, 60),
                accent: rgb(176, 230, 90),
                rock: rgb(60, 64, 46),
            },
            // Meadow: bright sunlit grassland — vivid green floor, soft blue sky, warm haze.
            MapTheme::Meadow => Palette {
                floor: rgb(86, 120, 56),
                wall: rgb(120, 130, 96),
                trim: rgb(150, 180, 90),
                ceil: rgb(150, 190, 210),
                cover: rgb(120, 150, 80),
                sky: rgb(150, 190, 225),
                fog: rgb(178, 200, 200),
                accent: rgb(245, 220, 120),
                rock: rgb(132, 130, 116),
            },
            // Redwood: dim old-growth forest — deep duff floor, rust-red trunks, shafted green gloom.
            MapTheme::Redwood => Palette {
                floor: rgb(54, 42, 32),
                wall: rgb(96, 60, 44),
                trim: rgb(120, 72, 50),
                ceil: rgb(40, 48, 36),
                cover: rgb(80, 56, 40),
                sky: rgb(70, 90, 64),
                fog: rgb(48, 56, 44),
                accent: rgb(180, 120, 70),
                rock: rgb(92, 84, 74),
            },
            // Station: dark metallic hull plating, cyan guide-lights, the black of space with a faint nebula.
            MapTheme::Station => Palette {
                floor: rgb(58, 64, 74),
                wall: rgb(70, 78, 90),
                trim: rgb(120, 134, 150),
                ceil: rgb(16, 18, 26),
                cover: rgb(78, 86, 98),
                sky: rgb(10, 12, 22),
                fog: rgb(14, 16, 28),
                accent: rgb(120, 220, 255),
                rock: rgb(74, 80, 92),
            },
            // Carrier: navy-grey non-skid deck, yellow guide-lines, slate maritime overcast over teal ocean.
            MapTheme::Carrier => Palette {
                floor: rgb(74, 82, 92),
                wall: rgb(96, 104, 114),
                trim: rgb(150, 158, 166),
                ceil: rgb(150, 160, 170),
                cover: rgb(104, 112, 122),
                sky: rgb(150, 162, 172),
                fog: rgb(132, 150, 158),
                accent: rgb(222, 200, 96),
                rock: rgb(84, 92, 102),
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct Room {
    pub rect: Rect,
    pub id: usize,
    pub center: Vec3,
    pub floor_y: f32,
    pub ceil_y: f32,
}

#[derive(Clone, Debug)]
pub struct MapConfig {
    pub seed: u64,
    pub size: i32,
    pub room_count: usize,
    pub room_min: i32,
    pub room_max: i32,
    pub corridor_width: i32,
    pub wall_height: f32,
    pub tile_size: f32,
    pub cover_density: f32,
    pub pit_chance: f32,
    pub theme: MapTheme,
    /// Tower-defense lanes: 0 = normal layout; N>0 carves N parallel clear lanes through dense jungle.
    pub lanes: u8,
    /// Force a specific centerpiece (index into the biome's landmark set) instead of rolling from the seed.
    /// Lets a named preset pin its signature setpiece. `None` = seed-rolled.
    pub landmark: Option<u8>,
    /// Circular-arena layout: a big open disc kill-zone ringed by perimeter cover, instead of rooms/corridors.
    pub arena: bool,
    /// Chasm-span layout: a narrow bridge over a bottomless void with flanking guard chambers at the midpoint.
    pub bridge: bool,
    /// Culvert-breach layout: a two-tier siege — an elevated wall-walk above a sunken waterlogged trench,
    /// joined by a breach blown through the wall (the only ground crossing) and a rubble ramp up to the walk.
    pub culvert: bool,
    /// Graph-skinned layout: the floor plan comes from a top-ranked topology graph (the flow-first factory)
    /// rather than BSP/biome carving — rooms at graph nodes, corridors along graph edges.
    pub graph: bool,
    /// Arena side lanes: carve two flanking N–S routes outside the arena disc so the open killbox has covered
    /// approaches to outmanoeuvre on, instead of being a single exposed bowl.
    pub side_lanes: bool,
    /// Submerged-docks split-level layout: a shallow-flooded basin with a shipping-container labyrinth below
    /// and raised steel walkways above — long sightlines up top, tight ones down in the water.
    pub docks: bool,
    /// Agent-simulated layout: terrain shaped by competing Sniper/Rusher/Camper agents (emergent hills, worn
    /// lanes, cover pockets) skinned onto a connected base.
    pub agents: bool,
    /// Three-lane competitive layout: flank/mid/flank lanes joined by cross-connectors for rotations, a
    /// contested central power position on high ground, deliberate cover rhythm, and a power pickup — the
    /// readable, high-action architecture of arena shooters.
    pub triplane: bool,
    /// Authored hand-built map selector: Some(kind) bypasses procedural generation. 0 = Flood Zone.
    pub authored: Option<u8>,
}
impl Default for MapConfig {
    fn default() -> Self {
        Self {
            seed: 0,
            size: 152,
            room_count: 34,
            room_min: 8,
            room_max: 22,
            corridor_width: 3,
            wall_height: 4.2,
            tile_size: 2.0,
            cover_density: 2.6,
            pit_chance: 0.16,
            theme: MapTheme::Industrial,
            lanes: 0,
            landmark: None,
            arena: false,
            bridge: false,
            culvert: false,
            graph: false,
            side_lanes: false,
            docks: false,
            agents: false,
            triplane: false,
            authored: None,
        }
    }
}

impl MapConfig {
    /// A seed-derived config that varies structurally — size, room density, cover, ceiling height and pits —
    /// so back-to-back random maps feel like different places, not the same grid re-skinned.
    pub fn varied(seed: u64) -> Self {
        let mut c = MapConfig::default();
        c.seed = seed;
        c.theme = MapTheme::from_seed(seed);
        // Pull a few decorrelated 0..1 values out of the seed.
        let r = |salt: u64| {
            let h = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(salt.wrapping_mul(0x1234_5679_9E37));
            ((h >> 33) as u32 as f32) / (u32::MAX as f32)
        };
        // Size 120..=200 in steps; bigger maps get more rooms.
        c.size = 120 + (r(1) * 84.0) as i32;
        c.room_count = (c.size as f32 * (0.20 + r(2) * 0.10)) as usize;
        c.room_min = 7 + (r(3) * 3.0) as i32; // 7..=9
        c.room_max = c.room_min + 10 + (r(4) * 8.0) as i32; // span
        c.corridor_width = 2 + (r(5) * 1.99) as i32; // 2..=3
        c.wall_height = 3.6 + r(6) * 2.6; // 3.6..=6.2 — varies the sense of enclosure
        c.cover_density = 1.8 + r(7) * 2.0; // 1.8..=3.8
        c.pit_chance = 0.08 + r(8) * 0.26; // 0.08..=0.34
        c
    }
}

/// A solid axis-aligned obstacle (a crate / barrel) in world space, for movement & bullet collision.
#[derive(Clone, Copy)]
pub struct Obstacle {
    pub center: Vec3,
    pub half: Vec3, // half extents
}
impl Obstacle {
    pub fn contains_xz(&self, p: Vec2, pad: f32) -> bool {
        (p.x - self.center.x).abs() <= self.half.x + pad && (p.y - self.center.z).abs() <= self.half.z + pad
    }
}

pub struct Level {
    pub cfg: MapConfig,
    pub pal: Palette,
    pub w: i32,
    pub h: i32,
    pub cells: Vec<Cell>,
    /// Per-tile floor elevation above `floor_y` (the terrain height field — the source of verticality).
    pub heights: Vec<f32>,
    pub rooms: Vec<Room>,
    pub obstacles: Vec<Obstacle>,
    /// Walkable raised decks (mezzanines / catwalks). You can stand on top when above one and walk beneath
    /// it when below — the second-floor system.
    pub platforms: Vec<Platform>,
    pub meshes: Vec<Mesh3>,
    pub floor_y: f32,
    pub ceil_y: f32,
    /// Standing lava/water hazard: its resting surface height, whether it's lava (damaging) and whether the
    /// map has one at all. A surge event can raise the effective surface above this baseline.
    pub hazard_y: f32,
    pub hazard_lava: bool,
    pub has_hazard: bool,
    /// Damage per second when fully submerged in the hazard (0 = harmless water).
    pub hazard_dps: f32,
    /// A meandering worn footpath across the wilds (world XZ control points). Empty on non-wild biomes. The
    /// floor tints to dirt along it and the scatter clears, so there's a route the eye and feet can follow.
    pub trail: Vec<Vec2>,
    /// A meandering stream/sludge channel (world XZ control points), water biomes only. Its bed is dipped
    /// into the terrain and a translucent ribbon follows it, with reeds on the banks.
    pub stream: Vec<Vec2>,
    /// Still water pools (centre, radius) dipped into the open biomes — decorative ponds with reedy edges.
    pub ponds: Vec<(Vec2, f32)>,
    /// Man-cannon launch pads placed in custom maps: (pad position, launch strength).
    pub cannons: Vec<(Vec3, f32)>,
    /// Collectibles placed in custom maps: (floor position, kind) — the game spawns real pickups from these.
    pub custom_pickups: Vec<(Vec3, crate::pickups::PickupKind)>,
    /// Tower-defense lane centre-lines (each a polyline of world XZ points); empty on normal maps.
    pub lane_paths: Vec<Vec<Vec2>>,
    /// Dynamic level-of-detail field for very large maps: holds chunked geometry with per-chunk Full/Coarse/
    /// Impostor tiers selected each frame by camera distance. `None` on small maps (which draw `meshes`).
    pub lod_field: Option<crate::chunk::ChunkField>,
    /// Ground-level air currents (jetstreams) on open-sky biomes: ride them to cross the map fast.
    pub air_currents: Vec<AirCurrent>,
    /// Roof openings punched into built biomes (tile rects) so sky/light pours in — kills the sealed-dungeon
    /// feel. Open-sky biomes leave this empty (they have no roof to begin with).
    pub skylights: Vec<Rect>,
    /// Animated water surfaces (ponds, hazard sheets). Stored as descriptors and tessellated fresh each
    /// frame by the game with traveling waves + reflections, instead of a frozen flat sheet.
    pub water: Vec<WaterBody>,
    /// Per-room target elevation for graph-skinned maps (parallel to the carve's room list). Empty on normal
    /// maps; when present, `assign_heights` uses it so the graph's towers actually tower and trenches sink.
    pub graph_elev: Vec<f32>,
    /// Full emergent elevation field for agent-simulated maps (level-grid sized). When present, `assign_heights`
    /// uses it directly instead of room tiers, so the agents' hills and worn lanes become the terrain.
    pub agent_height: Vec<f32>,
    /// Functional-space plan: rooms tagged with the function they perform, stamped into coherent structure by
    /// `build_spaces` after heights are known. Filled by `carve_graph` from each node's role.
    pub space_plan: Vec<(Rect, crate::spaces::SpaceFunction)>,
    /// Destructible objects (breakable barriers, crates, glass) — queried by collision, raycast and render.
    pub destructibles: Vec<crate::destructible::Destructible>,
    /// Timed flood channels (authored Flood Zone map) — trenches that surge and drain on a cycle.
    pub flood: Vec<FloodChannel>,
    /// Optional sweeping sandstorm (authored Desert Town map).
    pub sandstorm: Option<Sandstorm>,
    /// Freestanding cover props only (barrels, crates, barricades, stalls) — used by the scorer to
    /// measure cover density independently from building shell obstacles. Empty on procedural maps,
    /// where the scorer falls back to counting all obstacles. Authored maps populate this so the
    /// building shells don't inflate the cover reading to an unplayable density.
    pub cover_obstacles: Vec<Obstacle>,
    /// Authored outdoor maps (town streets under open sky, dam basin) set this so the scorer treats them
    /// as open-air rather than sealed dungeons — their high `ceil_y` is a skybox, not a real roof.
    pub open_air: bool,
    /// Fire source points (Pyre Garage): world positions of burning cars/wreckage where the game emits a
    /// continuous plume of fire particles, embers and smoke, and casts a flickering orange dynamic light.
    pub fire_sources: Vec<Vec3>,
}

/// A trench that floods on a timed cycle: dry and walkable, then a surge pours in from the dam, holds, and
/// drains away — temporarily cutting the low route and pushing players to higher ground. Drives one
/// `WaterBody`'s surface height each frame; the game advances `phase`.
#[derive(Clone, Debug)]
pub struct FloodChannel {
    /// Tile footprint of the trench.
    pub region: Rect,
    /// Trench floor world-Y.
    pub bed_y: f32,
    /// Water surface world-Y at full flood.
    pub crest_y: f32,
    /// Index into `Level::water` of the surface this channel raises.
    pub water_idx: usize,
    /// Seconds dry at the start of each cycle.
    pub delay: f32,
    /// Seconds to rise from dry to crest.
    pub rise: f32,
    /// Seconds held at crest.
    pub hold: f32,
    /// Seconds to drain back to dry.
    pub fall: f32,
    /// Seconds dry between cycles (after draining).
    pub gap: f32,
    /// Phase within the cycle, advanced by the game each frame.
    pub phase: f32,
}

impl FloodChannel {
    fn dry_y(&self) -> f32 {
        self.bed_y - 0.7 // parked just below the bed so the dry trench reads empty
    }

    /// Current water surface for the channel's phase — a trapezoid between dry and crest.
    pub fn surface(&self) -> f32 {
        let total = self.delay + self.rise + self.hold + self.fall + self.gap;
        if total <= 0.0 {
            return self.dry_y();
        }
        let mut t = self.phase.rem_euclid(total);
        if t < self.delay {
            return self.dry_y();
        }
        t -= self.delay;
        if t < self.rise {
            return self.dry_y() + (self.crest_y - self.dry_y()) * (t / self.rise);
        }
        t -= self.rise;
        if t < self.hold {
            return self.crest_y;
        }
        t -= self.hold;
        if t < self.fall {
            return self.crest_y + (self.dry_y() - self.crest_y) * (t / self.fall);
        }
        self.dry_y()
    }

    /// Is there meaningful water in the trench right now (deep enough to wade/impede)?
    pub fn flooded(&self) -> bool {
        self.surface() > self.bed_y + 0.3
    }

    /// World-space test: is (wx, wz) over this channel's footprint?
    pub fn contains_world(&self, wx: f32, wz: f32, ts: f32) -> bool {
        wx >= self.region.x0 as f32 * ts
            && wx <= self.region.x1 as f32 * ts
            && wz >= self.region.z0 as f32 * ts
            && wz <= self.region.z1 as f32 * ts
    }
}

/// A sandstorm front that sweeps the map on a cycle (Desert Town): clear, then a wall of dust ramps in,
/// holds, and clears — dropping visibility and shoving the player along the wind vector while it blows. The
/// game advances `phase` and reads `intensity` (0..1).
#[derive(Clone, Copy, Debug)]
pub struct Sandstorm {
    pub delay: f32,
    pub ramp: f32,
    pub hold: f32,
    pub ease: f32,
    pub gap: f32,
    /// Wind push direction & strength (world XZ).
    pub wind: Vec3,
    pub phase: f32,
}

impl Sandstorm {
    /// Storm intensity 0..1 for the current phase — a trapezoid: ramp up, hold, ease down, then clear.
    pub fn intensity(&self) -> f32 {
        let total = self.delay + self.ramp + self.hold + self.ease + self.gap;
        if total <= 0.0 {
            return 0.0;
        }
        let mut t = self.phase.rem_euclid(total);
        if t < self.delay {
            return 0.0;
        }
        t -= self.delay;
        if t < self.ramp {
            return t / self.ramp;
        }
        t -= self.ramp;
        if t < self.hold {
            return 1.0;
        }
        t -= self.hold;
        if t < self.ease {
            return 1.0 - t / self.ease;
        }
        0.0
    }
}

/// A water (or lava) surface the renderer animates each frame. A disc for ponds, a rectangular span for the
/// big hazard sheets. Carries its look (colour/alpha/reflect/emissive) and a `lava` flag for the molten pulse.
#[derive(Clone, Copy, Debug)]
pub struct WaterBody {
    pub cx: f32,
    pub cz: f32,
    /// Disc radius, or half-extent of the square span when `disc == false`.
    pub radius: f32,
    pub y: f32,
    pub col: egui::Color32,
    pub alpha: u8,
    pub reflect: f32,
    pub emissive: f32,
    pub lava: bool,
    pub disc: bool,
}

/// A ground-hugging air current (jetstream). While the player is within `radius` of the polyline, a steady
/// push of `strength` is applied along the flow direction — step into it to be carried across the map.
#[derive(Clone, Debug)]
pub struct AirCurrent {
    /// Ordered world-XZ control points the stream flows along.
    pub path: Vec<Vec2>,
    pub radius: f32,
    pub strength: f32,
    /// World-Y the streamer ribbon floats at (near the ground).
    pub y: f32,
}

impl AirCurrent {
    /// If `p` (world XZ) is inside the current, return the unit flow direction there; else `None`.
    pub fn flow_at(&self, p: Vec2) -> Option<Vec2> {
        let mut best = f32::MAX;
        let mut dir = None;
        for w in self.path.windows(2) {
            let (a, b) = (w[0], w[1]);
            let ab = Vec2::new(b.x - a.x, b.y - a.y);
            let len2 = (ab.x * ab.x + ab.y * ab.y).max(1e-4);
            let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0);
            let cp = Vec2::new(a.x + ab.x * t, a.y + ab.y * t);
            let d = ((p.x - cp.x).powi(2) + (p.y - cp.y).powi(2)).sqrt();
            if d < best {
                best = d;
                let l = (ab.x * ab.x + ab.y * ab.y).sqrt().max(1e-4);
                dir = Some(Vec2::new(ab.x / l, ab.y / l));
            }
        }
        if best <= self.radius {
            dir
        } else {
            None
        }
    }
}

/// A walkable raised surface: a world-space AABB footprint at height `top`.
#[derive(Clone, Copy, Debug)]
pub struct Platform {
    pub x0: f32,
    pub z0: f32,
    pub x1: f32,
    pub z1: f32,
    pub top: f32,
}
impl Platform {
    fn contains(&self, x: f32, z: f32) -> bool {
        x >= self.x0 && x <= self.x1 && z >= self.z0 && z <= self.z1
    }
}

impl Level {
    #[inline]
    pub fn ts(&self) -> f32 {
        self.cfg.tile_size
    }
    /// **Analysis-guided authoring helper.** Ask the sightline analysis where the most over-exposed floor is,
    /// and drop low crate/planter cover there to break the worst shooting-gallery lanes — the analysis feeding
    /// back into generation. Returns how many pieces it placed. Call this near the end of an authored builder
    /// (after obstacles exist, before `build_meshes`/`chunk_meshes`) to auto-articulate an over-open map.
    /// `budget` caps the pieces; each is registered as both a physical and a cover obstacle.
    pub fn scatter_exposure_cover(&mut self, budget: usize, color: Color32) -> usize {
        let spots = crate::sightline::suggest_cover(self, budget);
        let mut placed = 0;
        for (wx, wz, half) in spots {
            let hy = 0.6 + half * 0.3; // low, waist-to-chest cover you can shoot over from range but hug up close
            let c = Vec3::new(wx, self.floor_y + hy, wz);
            let h = Vec3::new(half, hy, half);
            // a weathered crate: base box + a lid lip, dithered so a row of them doesn't read identical
            let mut m = boxx(c, h, dither(color, (wx * 7.0 + wz * 13.0) as i32, 0.1), FACE_ALL);
            m.material = Some(crate::mesh::Material::WeatheredWood);
            crate::mesh::tint_height(&mut m, self.floor_y, self.floor_y + hy * 2.0, scale_rgb(color, 0.7), color);
            self.meshes.push(m);
            self.meshes.push(boxx(c + Vec3::new(0.0, hy, 0.0), Vec3::new(half * 1.05, 0.08, half * 1.05), scale_rgb(color, 0.85), FACE_ALL));
            self.obstacles.push(Obstacle { center: c, half: h });
            self.cover_obstacles.push(Obstacle { center: c, half: h });
            placed += 1;
        }
        placed
    }
    #[inline]
    pub fn in_bounds(&self, x: i32, z: i32) -> bool {
        x >= 0 && z >= 0 && x < self.w && z < self.h
    }
    /// Is (x,z) within the map's outer boundary margin (or off the grid entirely)? Used to tell "this solid
    /// cell is the map's edge" apart from "this solid cell is interior architecture" — the former gets the
    /// skybox + cliff/asset boundary treatment, the latter keeps rendering as a normal wall.
    #[inline]
    pub fn on_boundary(&self, x: i32, z: i32) -> bool {
        !self.in_bounds(x, z) || x <= 1 || z <= 1 || x >= self.w - 2 || z >= self.h - 2
    }
    #[inline]
    pub fn cell(&self, x: i32, z: i32) -> Cell {
        if self.in_bounds(x, z) {
            self.cells[(z * self.w + x) as usize]
        } else {
            Cell::Solid
        }
    }
    #[inline]
    fn set(&mut self, x: i32, z: i32, c: Cell) {
        if self.in_bounds(x, z) {
            self.cells[(z * self.w + x) as usize] = c;
        }
    }
    /// World position of a tile centre on the (elevated) floor surface.
    pub fn tile_center(&self, x: i32, z: i32) -> Vec3 {
        let ts = self.ts();
        let wx = (x as f32 + 0.5) * ts;
        let wz = (z as f32 + 0.5) * ts;
        Vec3::new(wx, self.floor_h(wx, wz), wz)
    }
    /// Raw per-tile elevation (0 outside walkable tiles).
    #[inline]
    pub fn tile_h(&self, x: i32, z: i32) -> f32 {
        if self.in_bounds(x, z) {
            self.heights[(z * self.w + x) as usize]
        } else {
            0.0
        }
    }
    /// Elevation at a grid corner (x in 0..=w, z in 0..=h): the average of the walkable tiles meeting
    /// there, so the floor surface is continuous and ramps read smoothly.
    fn corner_h(&self, cx: i32, cz: i32) -> f32 {
        let mut sum = 0.0;
        let mut n = 0.0;
        for (dx, dz) in [(-1, -1), (0, -1), (-1, 0), (0, 0)] {
            let (tx, tz) = (cx + dx, cz + dz);
            if self.in_bounds(tx, tz) && self.cell(tx, tz).walkable() {
                sum += self.tile_h(tx, tz);
                n += 1.0;
            }
        }
        if n > 0.0 {
            sum / n
        } else {
            // no walkable neighbour: fall back to any tile's height so walls meet the ground
            let mut s2 = 0.0;
            let mut n2 = 0.0;
            for (dx, dz) in [(-1, -1), (0, -1), (-1, 0), (0, 0)] {
                if self.in_bounds(cx + dx, cz + dz) {
                    s2 += self.tile_h(cx + dx, cz + dz);
                    n2 += 1.0;
                }
            }
            if n2 > 0.0 {
                s2 / n2
            } else {
                0.0
            }
        }
    }
    /// Absolute world Y of the walkable floor surface under a world XZ point (bilinear over tile corners).
    pub fn floor_h(&self, wx: f32, wz: f32) -> f32 {
        let ts = self.ts();
        let gx = wx / ts;
        let gz = wz / ts;
        let tx = gx.floor() as i32;
        let tz = gz.floor() as i32;
        let fx = gx - tx as f32;
        let fz = gz - tz as f32;
        let h00 = self.corner_h(tx, tz);
        let h10 = self.corner_h(tx + 1, tz);
        let h01 = self.corner_h(tx, tz + 1);
        let h11 = self.corner_h(tx + 1, tz + 1);
        let a = h00 * (1.0 - fx) + h10 * fx;
        let b = h01 * (1.0 - fx) + h11 * fx;
        self.floor_y + a * (1.0 - fz) + b * fz
    }
    /// Lowest corner elevation anywhere — used so wall skirts reach below sunken floors with no gaps.
    pub fn min_h(&self) -> f32 {
        self.heights.iter().copied().fold(0.0_f32, f32::min)
    }
    /// Tile coordinates containing a world XZ point.
    pub fn world_to_tile(&self, p: Vec2) -> (i32, i32) {
        ((p.x / self.ts()).floor() as i32, (p.y / self.ts()).floor() as i32)
    }
    /// Is the tile under this world point one a body / bullet can't be inside?
    pub fn blocked_at(&self, p: Vec2) -> bool {
        let (x, z) = self.world_to_tile(p);
        self.cell(x, z).blocks()
    }
    /// Is there solid floor under this world point (false over a pit / outside)?
    pub fn has_floor_at(&self, p: Vec2) -> bool {
        let (x, z) = self.world_to_tile(p);
        self.cell(x, z).walkable()
    }
    pub fn obstacle_at(&self, p: Vec2, pad: f32) -> bool {
        self.obstacles.iter().any(|o| o.contains_xz(p, pad))
    }

    /// The height of the (non-lava) water surface covering world XZ `(wx, wz)`, if any body contains the
    /// point. Returns the *highest* such surface (bodies can stack, e.g. a flooded trench under a pool).
    /// This is the single query the player's water physics reads: feet-below-surface = buoyancy + swim drag.
    /// Lava bodies are excluded — those are handled by the hazard/burn path, not swimming.
    pub fn water_surface_at(&self, wx: f32, wz: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        for w in &self.water {
            if w.lava {
                continue;
            }
            let inside = if w.disc {
                let (dx, dz) = (wx - w.cx, wz - w.cz);
                dx * dx + dz * dz <= w.radius * w.radius
            } else {
                (wx - w.cx).abs() <= w.radius && (wz - w.cz).abs() <= w.radius
            };
            if inside {
                best = Some(best.map_or(w.y, |b: f32| b.max(w.y)));
            }
        }
        best
    }

    /// The walkable surface height under a point, considering raised platforms the body can currently stand
    /// on. A platform counts only if its deck is at or below `y + STEP` (so you can step/walk up onto it but
    /// walk *under* it when you're below) — this is what makes the second floor work over the terrain floor.
    pub fn support_y(&self, wx: f32, wz: f32, y: f32) -> f32 {
        const STEP: f32 = 0.65;
        let mut best = self.floor_h(wx, wz);
        for p in &self.platforms {
            if p.contains(wx, wz) && p.top <= y + STEP && p.top > best {
                best = p.top;
            }
        }
        // Obstacle tops are standable surfaces too. Without this a body that jumps or jetpacks onto a crate
        // falls straight through to the floor and ends up embedded inside it — you can stand ON cover now.
        for o in &self.obstacles {
            let top = o.center.y + o.half.y;
            if wx >= o.center.x - o.half.x
                && wx <= o.center.x + o.half.x
                && wz >= o.center.z - o.half.z
                && wz <= o.center.z + o.half.z
                && top <= y + STEP
                && top > best
            {
                best = top;
            }
        }
        // Intact destructibles are standable too (until they're rubble).
        for d in &self.destructibles {
            if !d.blocks() {
                continue;
            }
            let top = d.top();
            if wx >= d.center.x - d.half.x
                && wx <= d.center.x + d.half.x
                && wz >= d.center.z - d.half.z
                && wz <= d.center.z + d.half.z
                && top <= y + STEP
                && top > best
            {
                best = top;
            }
        }
        best
    }

    /// Is there something to stand on under this point at the body's current height (terrain floor or a
    /// platform within stepping range)? Lets you cross a pit on a catwalk.
    /// Underside of the lowest platform whose slab sits above `head_y` at (wx, wz) — the ceiling a rising body
    /// will bonk into. Returns `f32::INFINITY` when nothing overhangs. Platform decks are ~0.16 thick, so the
    /// underside is a touch below the walkable `top`. This is what stops you passing up through an upper floor.
    pub fn ceiling_at(&self, wx: f32, wz: f32, head_y: f32) -> f32 {
        const DECK: f32 = 0.18;
        let mut best = f32::INFINITY;
        for p in &self.platforms {
            if p.contains(wx, wz) {
                let underside = p.top - DECK;
                if underside >= head_y - 0.05 && underside < best {
                    best = underside;
                }
            }
        }
        best
    }

    pub fn supported_at(&self, p: Vec2, y: f32) -> bool {
        if self.has_floor_at(p) {
            return true;
        }
        const STEP: f32 = 0.65;
        self.platforms.iter().any(|pl| pl.contains(p.x, p.y) && pl.top <= y + STEP && pl.top > self.floor_y - 50.0)
    }

    /// The highest walkable surface Y under a world XZ point — the grid floor, or a platform top if a
    /// freestanding platform (walkway deck, canyon bank, log bridge) covers the point above the grid floor.
    /// Returns `None` if nothing walkable is under the point (open void / over water). This is what spawn
    /// placement must use on authored maps whose walkable surfaces are box-platforms, not grid cells — using
    /// the grid `floor_h` alone drops enemies through the deck (the wave-spam bug).
    pub fn surface_at(&self, p: Vec2) -> Option<f32> {
        let mut best: Option<f32> = None;
        if self.has_floor_at(p) {
            best = Some(self.floor_h(p.x, p.y));
        }
        for pl in &self.platforms {
            if pl.contains(p.x, p.y) {
                best = Some(best.map_or(pl.top, |b| b.max(pl.top)));
            }
        }
        best
    }
}

// ── BSP generation ────────────────────────────────────────────────────────────

enum Axis {
    X,
    Z,
}
enum BspNode {
    Leaf { rect: Rect },
    Split { left: Box<BspNode>, right: Box<BspNode> },
}

fn bsp_split(rng: &mut Rng, rect: Rect, depth: i32, max_depth: i32, min_size: i32) -> BspNode {
    let can_x = rect.w() > min_size * 2;
    let can_z = rect.h() > min_size * 2;
    if depth >= max_depth || (!can_x && !can_z) {
        return BspNode::Leaf { rect };
    }
    let axis = if can_x && can_z {
        if rect.w() as f32 > rect.h() as f32 * 1.3 {
            Axis::X
        } else if rect.h() as f32 > rect.w() as f32 * 1.3 {
            Axis::Z
        } else if rng.chance(0.5) {
            Axis::X
        } else {
            Axis::Z
        }
    } else if can_x {
        Axis::X
    } else {
        Axis::Z
    };
    let (lo, hi) = match axis {
        Axis::X => (rect.x0 + min_size, rect.x1 - min_size),
        Axis::Z => (rect.z0 + min_size, rect.z1 - min_size),
    };
    if lo >= hi {
        return BspNode::Leaf { rect };
    }
    let pos = rng.i32_range(lo, hi);
    let (l, r) = match axis {
        Axis::X => (Rect::new(rect.x0, rect.z0, pos, rect.z1), Rect::new(pos, rect.z0, rect.x1, rect.z1)),
        Axis::Z => (Rect::new(rect.x0, rect.z0, rect.x1, pos), Rect::new(rect.x0, pos, rect.x1, rect.z1)),
    };
    BspNode::Split {
        left: Box::new(bsp_split(rng, l, depth + 1, max_depth, min_size)),
        right: Box::new(bsp_split(rng, r, depth + 1, max_depth, min_size)),
    }
}
fn bsp_leaves(node: &BspNode, out: &mut Vec<Rect>) {
    match node {
        BspNode::Leaf { rect } => out.push(*rect),
        BspNode::Split { left, right } => {
            bsp_leaves(left, out);
            bsp_leaves(right, out);
        }
    }
}

/// Carve a room (floor tiles) inside a BSP leaf, returning its placed rect or None if it won't fit.
fn place_room(rng: &mut Rng, leaf: Rect, cfg: &MapConfig) -> Option<Rect> {
    let inner = leaf.inset(1);
    if inner.w() < cfg.room_min || inner.h() < cfg.room_min {
        return None;
    }
    let rw = rng.i32_range(cfg.room_min, (cfg.room_max + 1).min(inner.w() + 1));
    let rh = rng.i32_range(cfg.room_min, (cfg.room_max + 1).min(inner.h() + 1));
    let rx = rng.i32_range(inner.x0, (inner.x1 - rw + 1).max(inner.x0 + 1));
    let rz = rng.i32_range(inner.z0, (inner.z1 - rh + 1).max(inner.z0 + 1));
    Some(Rect::new(rx, rz, rx + rw, rz + rh))
}

impl Level {
    pub fn generate(cfg: MapConfig) -> Level {
        if cfg.authored.is_some() {
            return Self::build_authored(cfg);
        }
        let mut rng = Rng::new(cfg.seed);
        let (w, h) = (cfg.size, cfg.size);
        let mut lvl = Level {
            pal: palette_variant(cfg.theme.palette(), cfg.seed),
            w,
            h,
            cells: vec![Cell::Solid; (w * h) as usize],
            heights: vec![0.0; (w * h) as usize],
            rooms: Vec::new(),
            obstacles: Vec::new(),
            platforms: Vec::new(),
            meshes: Vec::new(),
            floor_y: 0.0,
            ceil_y: cfg.wall_height,
            hazard_y: 0.0,
            hazard_lava: false,
            has_hazard: false,
            hazard_dps: 0.0,
            trail: Vec::new(),
            stream: Vec::new(),
            ponds: Vec::new(),
            cannons: Vec::new(),
            air_currents: Vec::new(),
            skylights: Vec::new(),
            water: Vec::new(),
            graph_elev: Vec::new(),
            agent_height: Vec::new(),
            space_plan: Vec::new(),
            destructibles: Vec::new(),
            flood: Vec::new(),
            sandstorm: None,
            cover_obstacles: Vec::new(),
            open_air: false,
            fire_sources: Vec::new(),
            custom_pickups: Vec::new(),
            lane_paths: Vec::new(),
            lod_field: None,
            cfg,
        };
        let cfg = lvl.cfg.clone();

        // 1+2) Carve the floor plan. Each biome family uses a structurally different layout — interior
        // compound (BSP rooms), an open-block street grid, container-aisle bays, or organic clearings —
        // so maps differ in *plan*, not just palette. Every carver returns its rooms; the rest of the
        // pipeline (walls, terrain, landmark, props) is shared.
        let mut rects: Vec<Rect> = if cfg.triplane {
            carve_triplane(&mut lvl, &mut rng, &cfg)
        } else if cfg.graph {
            carve_graph(&mut lvl, &mut rng, &cfg)
        } else if cfg.agents {
            carve_agents(&mut lvl, &mut rng, &cfg)
        } else if cfg.docks {
            carve_docks(&mut lvl, &mut rng, &cfg)
        } else if cfg.culvert {
            carve_culvert(&mut lvl, &mut rng, &cfg)
        } else if cfg.bridge {
            carve_bridge(&mut lvl, &mut rng, &cfg)
        } else if cfg.arena {
            carve_arena(&mut lvl, &mut rng, &cfg)
        } else {
            match cfg.theme {
                MapTheme::Station => carve_station(&mut lvl, &mut rng, &cfg),
                MapTheme::Carrier => carve_carrier(&mut lvl, &mut rng, &cfg),
                MapTheme::Urban => carve_streets(&mut lvl, &mut rng, &cfg),
                MapTheme::Warehouse => carve_bays(&mut lvl, &mut rng, &cfg),
                MapTheme::Volcanic | MapTheme::Ice | MapTheme::Alien | MapTheme::Canopy | MapTheme::Mistfall | MapTheme::Toxic | MapTheme::Meadow | MapTheme::Redwood => {
                    carve_clearings(&mut lvl, &mut rng, &cfg)
                }
                _ => carve_rooms(&mut lvl, &mut rng, &cfg),
            }
        };
        if rects.is_empty() {
            rects = carve_rooms(&mut lvl, &mut rng, &cfg); // safety net
        }
        if !cfg.arena && !cfg.bridge && !cfg.culvert && !cfg.docks && !cfg.agents && !cfg.triplane {
            lvl.flank_funnel(); // break long straight lanes with Z-jog flank alcoves (add-only, safe)
        }

        // 3) Wall ring: any Solid cell 4-adjacent to a Floor becomes a Wall.
        let mut to_wall = Vec::new();
        for z in 0..h {
            for x in 0..w {
                if lvl.cell(x, z) == Cell::Solid {
                    let near_floor = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)]
                        .iter()
                        .any(|(dx, dz)| lvl.cell(x + dx, z + dz) == Cell::Floor);
                    if near_floor {
                        to_wall.push((x, z));
                    }
                }
            }
        }
        for (x, z) in to_wall {
            lvl.set(x, z, Cell::Wall);
        }

        // 3b) Terrain: give rooms discrete elevation tiers, ramp the corridors between them, and add a
        // little rolling noise outdoors. This is what turns a flat grid into high ground, sunken lanes,
        // and sightlines — the backbone of the layout's feel.
        lvl.assign_heights(&cfg, &mut rng, &rects);

        // 4) Build Room records (world space) and decorate with cover + a few pits.
        for (id, r) in rects.iter().enumerate() {
            let (cx, cz) = r.center();
            let center = lvl.tile_center(cx, cz);
            lvl.rooms.push(Room {
                rect: *r,
                id,
                floor_y: center.y,
                ceil_y: lvl.ceil_y,
                center,
            });
        }
        lvl.decorate(&cfg, &mut rng, &rects);

        // 4b) Guarantee a walkable pad at every room centre — corridors land there and players spawn
        // there, so a pit (or anything) must never sit on it. Keeps the map connected by construction.
        for r in rects.iter() {
            let (cx, cz) = r.center();
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if lvl.cell(cx + dx, cz + dz) == Cell::Void {
                        lvl.set(cx + dx, cz + dz, Cell::Floor);
                    }
                }
            }
        }

        // 4c) Re-thread a 1-wide walkable spine between consecutive room centres, bridging over any pits
        // the decorator carved, so larger maps stay fully connected by construction.
        for i in 1..rects.len() {
            let (ax, az) = rects[i - 1].center();
            let (bx, bz) = rects[i].center();
            bridge_h(&mut lvl, ax, bx, az);
            bridge_v(&mut lvl, az, bz, bx);
        }

        // 5) Carve a meandering footpath across the wilds + a stream channel on water biomes, then bake.
        lvl.build_trail(&mut rng);
        lvl.build_stream(&mut rng);
        lvl.build_ponds(&mut rng);
        if lvl.cfg.lanes > 0 {
            lvl.build_lanes(&mut rng);
        }
        lvl.plan_skylights(&mut rng);
        lvl.build_meshes();
        lvl.build_landmark();        // 6) On the open biomes, dress the clearings as a logscope-style grove: leaning trees with leafy
        //    crowns (trunks block), drifting light shafts, and stepping stones.
        lvl.build_grove(&mut rng, &rects);
        // 7) Flood the low ground of water biomes with a translucent surface (only visible through pits and
        //    sunken clearings, since opaque floor sits above it).
        lvl.build_water();
        lvl.build_air_currents(&mut rng);
        // 8) Built biomes get multi-floor structures: raised mezzanine decks reached by stairs, with pillars
        //    and railings, that you can stand on or walk beneath — real verticality over the floor plan.
        lvl.build_structures(&mut rng);
        lvl.build_culvert();
        lvl.build_docks();
        lvl.build_triplane();
        lvl.build_spaces();
        lvl.build_destructibles(&mut rng, &rects);
        lvl.build_cliffs();
        lvl.dissolve_boundary();
        lvl.dress_boundary_margin(&mut rng);
        lvl.stamp_fsm_cover(cfg.seed as u32 ^ 0x5F1E);
        lvl.scatter_vignettes(&mut rng);
        lvl.scatter_detritus(&mut rng);
        if lvl.cfg.theme == MapTheme::Station {
            lvl.build_station(&mut rng);
        }
        if lvl.cfg.theme == MapTheme::Carrier {
            lvl.build_carrier(&mut rng);
        }
        // 9) Chunk the world geometry into a grid (each chunk carries a cached AABB) so the renderer can
        //    reject whole off-screen/far chunks before touching their triangles. This is a pure win at any
        //    map size — a small map has few chunks (near-zero overhead) and a big one gets massive savings —
        //    so we do it for everything but the very smallest arenas, and attach a dynamic LOD field on top
        //    for maps large enough that distant chunks should also thin their scatter / collapse to impostors.
        if lvl.cfg.size >= 96 || lvl.cfg.lanes > 0 {
            let reach = lvl.cfg.size as f32 * lvl.ts();
            let near = (reach * 0.18).clamp(40.0, 90.0);
            let far = (reach * 0.5).clamp(90.0, 200.0);
            // Only the larger maps get the dynamic LOD field (scatter-thinning + impostors); smaller ones
            // just get static whole-chunk culling, which is where nearly all the cost is anyway.
            if lvl.cfg.size > 200 || lvl.cfg.lanes > 0 {
                let mut field = crate::chunk::ChunkField::from_meshes(&lvl.meshes, 48.0, near, far);
                field.settle(lvl.spawn(0));
                lvl.lod_field = Some(field);
            }
            lvl.chunk_meshes(48.0);
        }
        lvl
    }

    /// Replace the world meshes with a chunked grid (per-chunk AABBs) for cheap whole-chunk frustum culling.
    /// Stamp organic rock **cliffs** along the map's outer boundary — every theme now terminates the play
    /// space this way instead of a flat perimeter wall. Each boundary Floor→solid edge rises into a
    /// battered, strata-banded rock face with eroded boulders piled at the base; the tops are jagged
    /// (noise) and tall enough to dissolve into fog, so the world reads as continuing up into hills/cliffs
    /// rather than stopping at a cage. Interior architecture (room/corridor walls) is untouched — this only
    /// fires where the solid neighbour is the map's true edge.
    fn build_cliffs(&mut self) {
        if matches!(self.cfg.theme, MapTheme::Station | MapTheme::Carrier) {
            return; // these build their own deck-edge look in build_station/build_carrier
        }
        let ts = self.ts();
        let pal = self.pal;
        let seed = self.cfg.seed as i32;
        let rock_mid = pal.rock;
        let rock_dark = scale_rgb(pal.rock, 0.5);
        let rock_top = lerp_c(pal.rock, rgb(180, 180, 176), 0.18);
        let mut cliff = Mesh3::new(rock_mid);
        let mut boulders = Mesh3::new(scale_rgb(pal.rock, 0.92));
        let mut stamp = 0u32;
        let mut boulder_count = 0u32;
        let edges: [(i32, i32, Vec3); 4] = [
            (-1, 0, Vec3::new(1.0, 0.0, 0.0)),
            (1, 0, Vec3::new(-1.0, 0.0, 0.0)),
            (0, -1, Vec3::new(0.0, 0.0, 1.0)),
            (0, 1, Vec3::new(0.0, 0.0, -1.0)),
        ];
        for z in 0..self.h {
            for x in 0..self.w {
                if self.cell(x, z) != Cell::Floor {
                    continue;
                }
                let fy = self.floor_h((x as f32 + 0.5) * ts, (z as f32 + 0.5) * ts);
                let wx = x as f32 * ts;
                let wz = z as f32 * ts;
                for (dx, dz, n) in edges {
                    let nb = self.cell(x + dx, z + dz);
                    if nb == Cell::Floor || nb == Cell::Void {
                        continue; // only solid boundaries become cliffs (pits stay as drops)
                    }
                    // Only the map's true outer edge gets the cliff treatment — an interior room/corridor
                    // wall stays a normal wall (built above); cliffs are the "assets blocking the map bounds"
                    // stand-in specifically at the boundary, not a replacement for interior architecture.
                    if !self.on_boundary(x + dx, z + dz) {
                        continue;
                    }
                    // the two ground corners of this edge
                    let (b0, b1) = match (dx, dz) {
                        (-1, 0) => (Vec3::new(wx, fy, wz), Vec3::new(wx, fy, wz + ts)),
                        (1, 0) => (Vec3::new(wx + ts, fy, wz + ts), Vec3::new(wx + ts, fy, wz)),
                        (0, -1) => (Vec3::new(wx + ts, fy, wz), Vec3::new(wx, fy, wz)),
                        _ => (Vec3::new(wx, fy, wz + ts), Vec3::new(wx + ts, fy, wz + ts)),
                    };
                    // jagged, tall cliff top — varies per position so the skyline isn't a flat ridge
                    let h0 = 7.0 + (vnoise(b0.x * 0.08, b0.z * 0.08, seed ^ 0x3C) + 0.5) * 7.0;
                    let h1 = 7.0 + (vnoise(b1.x * 0.08, b1.z * 0.08, seed ^ 0x3C) + 0.5) * 7.0;
                    // batter: lean the top outward (away from the floor) so it's a sloped face, not a wall
                    let lean = n * -1.6;
                    let t0 = Vec3::new(b0.x + lean.x, fy + h0, b0.z + lean.z);
                    let t1 = Vec3::new(b1.x + lean.x, fy + h1, b1.z + lean.z);
                    cliff.push_quad(b0, b1, t1, t0, n, rock_mid);
                    // eroded boulders piled at the base, sparsely (capped so the boundary stays cheap)
                    stamp = stamp.wrapping_add(1);
                    if stamp % 17 == 0 && boulder_count < 120 {
                        let mid = (b0 + b1) * 0.5 + n * 0.6;
                        let r = 0.8 + (vnoise(mid.x * 0.3, mid.z * 0.3, seed) + 0.5) * 1.4;
                        boulders.merge(&rock(Vec3::new(mid.x, fy + r * 0.3, mid.z), r, (x * 71 + z * 13) as u32, scale_rgb(pal.rock, 0.92)));
                        boulder_count += 1;
                    }
                }
            }
        }
        // strata: wet/eroded dark at the base lightening toward the sun-struck top
        if cliff.tri_count() > 0 {
            let (lo, hi) = (self.floor_y - 1.0, self.floor_y + 16.0);
            tint_height(&mut cliff, lo, hi, rock_dark, rock_top);
            // surface the rock face + boulders with stone so they read as fractured rock, not flat colour
            // (uv_sphere now provides proper UVs, so boulders map smoothly).
            cliff.apply_material(crate::mesh::Material::StoneBlock, 0xC11F);
            cliff.material = Some(crate::mesh::Material::StoneBlock);
            self.meshes.push(cliff);
        }
        if boulders.tri_count() > 0 {
            boulders.apply_material(crate::mesh::Material::StoneBlock, 0xB0D3);
            boulders.material = Some(crate::mesh::Material::StoneBlock);
            self.meshes.push(boulders);
        }
    }

    /// **Boundary Dissolver** — instead of letting the map just *end*, drop a vast themed hazard sheet far
    /// below the play area so that, past the cliff edge, the world reads as continuing into something lethal:
    /// a glowing lava sea on the fire biomes, a dark storm-ocean on the wet ones, a fog-choked abyss elsewhere.
    /// You hit a cliff, not an invisible wall, and below it is somewhere you clearly cannot go.
    fn dissolve_boundary(&mut self) {
        if matches!(self.cfg.theme, MapTheme::Station | MapTheme::Carrier) {
            return; // these build their own deck-edge look in build_station/build_carrier
        }
        let ts = self.ts();
        let extent = self.w as f32 * ts;
        let (cx, cz) = (extent * 0.5, self.h as f32 * ts * 0.5);
        let drop = self.floor_y + self.min_h() - 9.0;
        let (col, emissive, alpha): (Color32, f32, u8) = match self.cfg.theme {
            MapTheme::Volcanic => (rgb(220, 90, 30), 1.0, 255), // lava sea
            MapTheme::Toxic => (rgb(120, 180, 70), 0.5, 220),   // acid
            MapTheme::Mistfall => (rgb(40, 92, 110), 0.0, 210), // storm ocean
            MapTheme::Ice => (rgb(150, 180, 205), 0.1, 230),    // frozen depths
            _ => (scale_rgb(self.pal.fog, 0.5), 0.0, 235),       // fog abyss
        };
        let mut haz = Mesh3::new(col).emit(emissive);
        let r = extent * 1.4;
        haz.push_quad(
            Vec3::new(cx - r, drop, cz - r),
            Vec3::new(cx + r, drop, cz - r),
            Vec3::new(cx + r, drop, cz + r),
            Vec3::new(cx - r, drop, cz + r),
            Vec3::new(0.0, 1.0, 0.0),
            col,
        );
        if alpha < 255 {
            set_alpha(&mut haz, alpha);
        }
        // a couple of dim ledges of the same rock jutting toward the hazard sell the depth past the rim
        self.meshes.push(haz);
    }

    /// Give the boundary margin — the ring of cells that were always Solid, now sitting behind the cliff
    /// face instead of a flat wall — a floor and dense, mostly-unwalkable dressing instead of empty space.
    /// From ground level the cliffs hide it, but from any elevated vantage (a rooftop, a mezzanine, a hill)
    /// looking out past the play area's rim, the world used to just stop at a bare rock edge with nothing
    /// beyond. This scatters theme-appropriate dense terrain — rock fields and dead trees on the wilds,
    /// rubble and girder wreckage on the built biomes — so the horizon reads as "more world you can't reach"
    /// instead of a void. Everything placed here stays a blocking obstacle; none of it is meant to be
    /// walkable, and the margin is already outside the connectivity graph.
    fn dress_boundary_margin(&mut self, rng: &mut Rng) {
        if matches!(self.cfg.theme, MapTheme::Station | MapTheme::Carrier) {
            return; // no ground out there — it's open space / ocean, not terrain
        }
        let ts = self.ts();
        let pal = self.pal;
        let organic = matches!(
            self.cfg.theme,
            MapTheme::Alien | MapTheme::Volcanic | MapTheme::Ice | MapTheme::Canopy | MapTheme::Mistfall | MapTheme::Toxic | MapTheme::Meadow | MapTheme::Redwood
        );

        // floor slab across the whole margin ring, a shade duller than the play floor so it still reads as
        // "beyond the fence" rather than more walkable ground
        let mut floor = Mesh3::new(scale_rgb(pal.floor, 0.8));
        for z in 0..self.h {
            for x in 0..self.w {
                if !self.on_boundary(x, z) || self.cell(x, z) == Cell::Void {
                    continue;
                }
                let wx = x as f32 * ts;
                let wz = z as f32 * ts;
                let fy = self.floor_h(wx + ts * 0.5, wz + ts * 0.5) - 0.05; // a hair below the real floor
                // Organic low-frequency patch variation instead of a strict per-tile checkerboard: sampling a
                // coarse (3-tile) hash grid with a little bilinear-ish blend gives broad soft light/dark
                // patches that don't self-tile at tile scale — so the ground stops reading as a regular
                // checkerboard (which the scenic/variety scorer penalises as repetition).
                let patch = 0.86 + 0.14 * crate::mesh::hash01((x / 3).wrapping_mul(13) ^ (z / 3).wrapping_mul(7), 0x51);
                let c = dither(scale_rgb(pal.floor, 0.78 * patch), x.wrapping_mul(31) + z.wrapping_mul(17), 0.08);
                floor.push_quad(Vec3::new(wx, fy, wz + ts), Vec3::new(wx + ts, fy, wz + ts), Vec3::new(wx + ts, fy, wz), Vec3::new(wx, fy, wz), Vec3::UP, c);
            }
        }
        if floor.tri_count() > 0 {
            self.meshes.push(floor);
        }

        // dense, mostly-blocking clutter — sparse enough to stay cheap on a ring that runs the whole map
        // perimeter, dense enough to read as "terrain", not a sprinkle. Everything merges into a couple of
        // shared meshes instead of one Mesh3 per prop — thousands of tiny separate meshes is its own
        // performance problem for a painter's-algorithm renderer, independent of raw triangle count.
        let mut placed = 0u32;
        let mut rocks = Mesh3::new(pal.rock);
        let mut wood = Mesh3::new(scale_rgb(pal.wall, 0.7));
        for z in 0..self.h {
            for x in 0..self.w {
                if !self.on_boundary(x, z) || self.cell(x, z) == Cell::Void {
                    continue;
                }
                if rng.f32() > 0.18 || placed >= 350 {
                    continue;
                }
                let wx = (x as f32 + 0.5) * ts;
                let wz = (z as f32 + 0.5) * ts;
                let fy = self.floor_h(wx, wz);
                let seed = (x as u32).wrapping_mul(73).wrapping_add((z as u32).wrapping_mul(131)) ^ self.cfg.seed as u32;
                let mut r = Rng::new(seed as u64);
                if organic {
                    if r.f32() < 0.6 {
                        let radius = 0.5 + r.f32() * 0.9;
                        let mut rk = rock(Vec3::new(wx, fy + radius * 0.3, wz), radius, seed, pal.rock);
                        tint_up(&mut rk, lerp_c(pal.rock, rgb(110, 140, 90), 0.4), scale_rgb(pal.rock, 0.55));
                        rocks.merge(&rk);
                    } else {
                        // depth=1 (not the gameplay-hero depth=2/3) — this runs across hundreds of boundary
                        // cells, and the recursive branch count is exponential in depth (3 children per
                        // level), so depth=3 here was silently generating a 1M-triangle forest.
                        let h = 3.0 + r.f32() * 3.0;
                        for m in tree_styled(self.cfg.theme.tree_kind(), Vec3::new(wx, fy, wz), h, 0.22, 1, seed, scale_rgb(pal.wall, 0.7), lerp_c(pal.accent, rgb(70, 100, 60), 0.5)) {
                            wood.merge(&m);
                        }
                    }
                } else {
                    // built biomes: broken masonry / girder wreckage, styled from the wall/rock palette
                    if r.f32() < 0.55 {
                        let h = 0.6 + r.f32() * 1.6;
                        let mut chunk = boxx(Vec3::new(wx, fy + h * 0.5, wz), Vec3::new(0.5 + r.f32() * 0.5, h * 0.5, 0.5 + r.f32() * 0.5), scale_rgb(pal.wall, 0.7), FACE_ALL);
                        rotate_y_about(&mut chunk, Vec3::new(wx, fy, wz), r.f32() * 3.0);
                        wood.merge(&chunk);
                    } else {
                        let h = 2.0 + r.f32() * 3.0;
                        let a = r.f32() * 0.5 - 0.25;
                        let top = Vec3::new(wx + a.sin() * h * 0.3, fy + h, wz + a.cos() * h * 0.15);
                        rocks.merge(&tube(Vec3::new(wx, fy, wz), top, 0.12, 5, scale_rgb(pal.rock, 0.8)));
                    }
                }
                placed += 1;
            }
        }
        if rocks.tri_count() > 0 {
            self.meshes.push(rocks);
        }
        if wood.tri_count() > 0 {
            self.meshes.push(wood);
        }
    }

    /// **Flank Funnel** — find long straight corridors and punch a flank alcove off the midpoint of each, so a
    /// lane offers a Z-jog / side pocket instead of one boring straight shot (the Sightline Breaker then drops
    /// cover on the main run). Strictly *additive* — it only carves solid into floor, so it can never sever a
    /// route; connectivity is preserved by construction. Bounded to a handful per map.
    fn flank_funnel(&mut self) {
        let mut flanks = 0u32;
        let in_solid = |s: &Self, x: i32, z: i32| s.in_bounds(x, z) && s.cell(x, z) == Cell::Solid;
        // scan a coarse grid; at each Floor tile look for a long straight run (horizontal or vertical)
        let mut z = 6;
        while z < self.h - 6 && flanks < 5 {
            let mut x = 6;
            while x < self.w - 6 {
                if self.cell(x, z) != Cell::Floor {
                    x += 3;
                    continue;
                }
                // horizontal corridor: walled north & south, floor running east
                let horiz = !self.cell(x, z - 1).walkable() && !self.cell(x, z + 1).walkable();
                let vert = !self.cell(x - 1, z).walkable() && !self.cell(x + 1, z).walkable();
                let mut carved = false;
                if horiz {
                    let mut run = 0;
                    while run < 40 && self.cell(x + run, z).walkable() && !self.cell(x + run, z - 1).walkable() {
                        run += 1;
                    }
                    if run >= 12 {
                        let mx = x + run / 2;
                        // carve a 3×3 pocket on whichever side is solid (a side flank you can duck into)
                        let side = if in_solid(self, mx, z - 2) { -1 } else { 1 };
                        for dz in 1..4 {
                            for dx in -1..=1 {
                                let (ax, az) = (mx + dx, z + side * dz);
                                if in_solid(self, ax, az) {
                                    self.set(ax, az, Cell::Floor);
                                }
                            }
                        }
                        flanks += 1;
                        carved = true;
                        x += run;
                    }
                } else if vert {
                    let mut run = 0;
                    while run < 40 && self.cell(x, z + run).walkable() && !self.cell(x - 1, z + run).walkable() {
                        run += 1;
                    }
                    if run >= 12 {
                        let mz = z + run / 2;
                        let side = if in_solid(self, x - 2, mz) { -1 } else { 1 };
                        for dx in 1..4 {
                            for dz in -1..=1 {
                                let (ax, az) = (x + side * dx, mz + dz);
                                if in_solid(self, ax, az) {
                                    self.set(ax, az, Cell::Floor);
                                }
                            }
                        }
                        flanks += 1;
                        carved = true;
                    }
                }
                x += if carved { 4 } else { 3 };
            }
            z += 4;
        }
    }

    pub fn chunk_meshes(&mut self, cell: f32) {
        let chunked = crate::chunk::split(&self.meshes, cell);
        self.meshes = chunked;
    }

    /// Stamp a composed set of tile-areas (groves, ruins, yards…) onto this level, merging their geometry and
    /// collision. The high-level building-block path for authoring large, detailed maps: compose areas, stamp.
    pub fn stamp_areas(&mut self, composer: &crate::area::AreaComposer) {
        let ctx = crate::area::AreaCtx { pal: self.pal, ground_y: self.floor_y };
        let out = composer.build(&ctx);
        self.meshes.extend(out.meshes);
        self.obstacles.extend(out.obstacles);
    }

    /// Run the FSM toolkit across the open ground: at sampled points it builds a [`crate::fsm::Site`] (the
    /// local sightline, danger bearing, slope, high-ground and wetness), then lets the machines decide what
    /// belongs there — sightline-breaking cover most places, but a vantage on a peak, a bunker on a slope over
    /// open ground, a water-tower landmark on a high point, a wreck across a long lane, a tent on flat ground.
    /// High-traffic placements then get the Decay Cascader's directed battle damage. This is what makes the
    /// asset pool read as varied and intentional instead of copy-pasted.
    pub fn stamp_fsm_cover(&mut self, seed: u32) {
        if matches!(self.cfg.theme, MapTheme::Station | MapTheme::Carrier) {
            return;
        }
        use crate::fsm::{self, Fsm, Site};
        let ts = self.ts();
        let spawn = self.spawn(0);
        let indoor = !self.cfg.theme.open_sky();
        let mut s = seed | 1;
        let mut new_meshes = Vec::new();
        let mut new_obs = Vec::new();
        let mut placed = 0u32;
        let dirs = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)];
        let step = 6usize;
        'outer: for z in (4..self.h - 4).step_by(step) {
            for x in (4..self.w - 4).step_by(step) {
                if placed >= 28 {
                    break 'outer;
                }
                if self.cell(x, z) != Cell::Floor {
                    continue;
                }
                let wx = (x as f32 + 0.5) * ts;
                let wz = (z as f32 + 0.5) * ts;
                if ((wx - spawn.x).powi(2) + (wz - spawn.z).powi(2)).sqrt() < 16.0 {
                    continue;
                }
                if self.obstacle_at(Vec2::new(wx, wz), 1.6) {
                    continue;
                }
                // Build the Site: longest open cardinal run = sightline; its bearing = the danger angle.
                let (mut sightline, mut danger_yaw) = (0.0f32, 0.0f32);
                for (dx, dz) in dirs {
                    let mut d = 1;
                    while d < 40 && self.cell(x + dx * d, z + dz * d).walkable() {
                        d += 1;
                    }
                    let run = d as f32 * ts;
                    if run > sightline {
                        sightline = run;
                        danger_yaw = (dz as f32).atan2(dx as f32);
                    }
                }
                let jx = wx + (hash01(s as i32, 1) - 0.5) * ts * 0.7;
                let jz = wz + (hash01(s as i32, 2) - 0.5) * ts * 0.7;
                let gy = self.floor_h(jx, jz);
                // local slope + high-point from the heightfield
                let mut hi_here = true;
                let mut max_d = 0.0f32;
                for (dx, dz) in dirs {
                    let nh = self.floor_h(jx + dx as f32 * ts, jz + dz as f32 * ts);
                    if nh > gy + 0.05 {
                        hi_here = false;
                    }
                    max_d = max_d.max((nh - gy).abs());
                }
                let slope = (max_d / ts).min(1.0);
                let wet = self.pond_dist(jx, jz) < 3.0 || self.stream_dist(jx, jz) < 3.0 || self.has_hazard;
                let site = Site {
                    x: jx,
                    z: jz,
                    ground_y: gy,
                    sightline,
                    danger_yaw,
                    high_traffic: sightline > 30.0,
                    is_high_point: hi_here && slope > 0.04,
                    slope,
                    wet,
                    indoor,
                    pal: self.pal,
                    seed: s,
                };
                // Dispatch: mostly cover, with occasional special POIs where the site invites one.
                let wall_mat = self.cfg.theme.materials(self.cfg.seed).1;
                let roll = hash01(s as i32, 5);
                let (mut dressing, cover_mat) = if roll < 0.10 && site.is_high_point && !indoor {
                    (fsm::LogisticsSpine.grow(&site), crate::mesh::Material::WeatheredWood)
                } else if roll < 0.22 && site.slope > 0.12 {
                    (fsm::SentinelNode.grow(&site), crate::mesh::Material::HexPlate)
                } else if roll < 0.32 && site.sightline > 26.0 {
                    (fsm::KineticWreckage.grow(&site), crate::mesh::Material::RustPlate)
                } else if roll < 0.40 && !indoor && site.slope < 0.5 {
                    (fsm::AdHocShelter.grow(&site), crate::mesh::Material::WeatheredWood)
                } else if site.is_high_point {
                    (fsm::VerticalRiskReward.grow(&site), wall_mat)
                } else {
                    (fsm::SightlineBreaker.grow(&site), wall_mat)
                };
                dressing = dressing.or_else(|| fsm::SightlineBreaker.grow(&site));
                if let Some(ref mut d) = dressing {
                    // directed battle damage where the fighting is heaviest
                    if site.high_traffic && hash01(s as i32, 6) < 0.5 {
                        fsm::cascade_decay(d, gy, danger_yaw, Vec3::new(jx, gy, jz), s);
                    }
                    // surface the cover with its material (UV-less parts keep their flat colour, chunk-safe)
                    for m in d.meshes.iter_mut() {
                        m.apply_material(cover_mat, s ^ 0xC0DE);
                        m.material = Some(cover_mat);
                    }
                    new_meshes.append(&mut d.meshes);
                    new_obs.append(&mut d.obstacles);
                    placed += 1;
                }
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            }
        }
        self.meshes.extend(new_meshes);
        self.obstacles.extend(new_obs);
    }

    /// Stamp an abstract block-out (tagged volumes) onto this level, dressing each block against its own
    /// surroundings — material is chosen by where the block sits (near water/mud → rot/pitting, never clean
    /// metal). The "Volumetric Decomposition → Instructional Reversal" authoring path.
    pub fn stamp_blockout(&mut self, bo: &crate::blockout::Blockout) {
        let indoor = !self.cfg.theme.open_sky();
        let pal = self.pal;
        let ts = self.ts();
        let out = bo.dress_each(pal, indoor, |x, z| {
            let wet = self.pond_dist(x, z) < 2.5 || self.stream_dist(x, z) < 2.5 || self.has_hazard;
            let gy = self.floor_h(x, z).max(self.floor_y);
            let _ = ts;
            (wet, gy)
        });
        self.meshes.extend(out.meshes);
        self.obstacles.extend(out.obstacles);
    }

    /// Dress the station deck: the great hull body slung under the deck (engine nozzles, fins), service
    /// modules with lit windows, raised landing platforms ringed by approach lights, a command spine, edge
    /// railings with strip-lights, and scattered cargo as cover. Builds the "big spaceship" silhouette you
    /// see when you look out over the deck edge into space.
    fn build_station(&mut self, rng: &mut Rng) {
        let ts = self.ts();
        let gy = self.floor_y;
        let pal = self.pal;
        let mapw = self.w as f32 * ts;
        let maph = self.h as f32 * ts;
        let (cxw, czw) = (mapw * 0.5, maph * 0.5);
        let hull = lerp_c(pal.ceil, rgb(40, 46, 58), 0.5);
        let metal = pal.wall;
        let dark = scale_rgb(hull, 0.7);
        let mut body = Mesh3::new(hull);
        let mut mods = Mesh3::new(metal);
        let mut cyan = Mesh3::new(pal.accent).emit(1.0);
        let mut warm = Mesh3::new(rgb(255, 196, 120)).emit(1.0);

        // ── Great hull body under the deck — the ship's mass, seen over the edge ─────────────────────────
        let body_c = Vec3::new(cxw, gy - 3.4, czw);
        let mut hb = boxx(body_c, Vec3::new(mapw * 0.32, 2.8, maph * 0.24), hull, FACE_ALL);
        tint_up(&mut hb, lerp_c(hull, rgb(150, 160, 175), 0.12), dark);
        body.merge(&hb);
        // a lower keel block
        body.merge(&boxx(Vec3::new(cxw, gy - 6.2, czw), Vec3::new(mapw * 0.20, 1.6, maph * 0.12), dark, FACE_ALL));
        // engine nozzles at the aft (high-z) end, glowing cores
        for s in [-1.0_f32, 0.0, 1.0] {
            let ex = cxw + s * mapw * 0.16;
            let base = Vec3::new(ex, gy - 3.4, maph - 6.0);
            let end = Vec3::new(ex, gy - 3.4, maph + 2.0);
            body.merge(&tube(base, end, 1.7, 10, scale_rgb(metal, 0.8)));
            cyan.merge(&disc(Vec3::new(ex, gy - 3.4, maph + 1.9), 1.3, 10, pal.accent));
        }
        // side fins
        for s in [-1.0_f32, 1.0] {
            let mut fin = boxx(Vec3::new(cxw + s * mapw * 0.34, gy - 3.0, czw + maph * 0.18), Vec3::new(mapw * 0.04, 1.8, maph * 0.10), dark, FACE_ALL);
            rotate_y_about(&mut fin, Vec3::new(cxw, gy, czw), s * 0.5);
            body.merge(&fin);
        }

        // ── Service modules: lit habitation/utility blocks scattered on the main deck ────────────────────
        let mod_spots = [
            (0.36_f32, 0.40_f32), (0.62, 0.38), (0.40, 0.60), (0.66, 0.62), (0.50, 0.30), (0.30, 0.52), (0.70, 0.50),
        ];
        for (i, &(fx, fz)) in mod_spots.iter().enumerate() {
            let mx = mapw * fx;
            let mz = maph * fz;
            if !self.walkable_world(mx, mz) {
                continue;
            }
            let hgt = 1.6 + rng.f32() * 1.6;
            let hw = 1.4 + rng.f32() * 1.0;
            let hd = 1.4 + rng.f32() * 1.0;
            let mc = Vec3::new(mx, gy + hgt, mz);
            let mut blk = boxx(mc, Vec3::new(hw, hgt, hd), metal, FACE_NO_BOTTOM);
            tint_up(&mut blk, lerp_c(metal, rgb(180, 188, 200), 0.18), scale_rgb(metal, 0.6));
            mods.merge(&blk);
            // roof greeble + vent
            mods.merge(&boxx(Vec3::new(mx, gy + hgt * 2.0 + 0.2, mz), Vec3::new(hw * 0.5, 0.2, hd * 0.5), scale_rgb(metal, 0.8), FACE_ALL));
            // lit window strips on the four walls
            let glow = if i % 2 == 0 { &mut warm } else { &mut cyan };
            for (sx, sz) in [(1.0_f32, 0.0_f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                let wy = gy + hgt * 0.9;
                let wc = Vec3::new(mx + sx * (hw + 0.02), wy, mz + sz * (hd + 0.02));
                glow.merge(&boxx(wc, Vec3::new(if sx != 0.0 { 0.04 } else { hw * 0.7 }, 0.18, if sz != 0.0 { 0.04 } else { hd * 0.7 }), glow.color, FACE_ALL));
            }
            self.obstacles.push(Obstacle { center: Vec3::new(mx, gy + hgt, mz), half: Vec3::new(hw, hgt, hd) });
        }

        // ── Landing platforms at the wing ends: raised pad + approach-light ring + a parked dropship ─────
        for (k, fx) in [0.12_f32, 0.88].iter().enumerate() {
            let px = mapw * *fx;
            let pz = czw;
            if !self.walkable_world(px, pz) {
                continue;
            }
            let pc = Vec3::new(px, gy + 0.12, pz);
            let mut pad = disc(pc, 4.2, 24, lerp_c(metal, rgb(40, 44, 54), 0.4));
            tint_up(&mut pad, pal.accent, scale_rgb(metal, 0.5));
            mods.merge(&pad);
            // approach-light ring
            for j in 0..16 {
                let a = j as f32 / 16.0 * std::f32::consts::TAU;
                let lp = Vec3::new(px + a.cos() * 3.9, gy + 0.22, pz + a.sin() * 3.9);
                cyan.merge(&boxx(lp, Vec3::new(0.12, 0.06, 0.12), pal.accent, FACE_ALL));
            }
            // central landing 'H' glyph
            warm.merge(&boxx(Vec3::new(px, gy + 0.2, pz), Vec3::new(1.2, 0.04, 0.18), rgb(255, 210, 140), FACE_TOP));
            // a parked dropship on one pad
            if k == 0 {
                let (dm, _do) = lm_dropship(Vec3::new(px, gy + 0.3, pz), pal, 0x51A7);
                for m in dm {
                    self.meshes.push(m);
                }
                self.obstacles.push(Obstacle { center: Vec3::new(px, gy + 1.0, pz), half: Vec3::new(2.4, 1.0, 2.0) });
            }
        }

        // ── Command spine: a tall lit block near the deck centre (the antenna landmark sits on top) ───────
        let cmd = Vec3::new(cxw, gy + 2.2, czw - maph * 0.10);
        let mut tower = boxx(cmd, Vec3::new(2.6, 2.2, 2.0), metal, FACE_NO_BOTTOM);
        tint_up(&mut tower, lerp_c(metal, rgb(190, 198, 210), 0.2), scale_rgb(metal, 0.55));
        mods.merge(&tower);
        for fz in [-1.0_f32, 1.0] {
            cyan.merge(&boxx(Vec3::new(cxw, gy + 3.6, czw - maph * 0.10 + fz * 2.02), Vec3::new(1.8, 0.4, 0.03), pal.accent, FACE_ALL));
        }
        self.obstacles.push(Obstacle { center: cmd, half: Vec3::new(2.6, 2.2, 2.0) });

        // ── Edge railings + strip-lights around the main deck perimeter ──────────────────────────────────
        let (rx0, rx1) = (cxw - mapw * 0.29, cxw + mapw * 0.29);
        let (rz0, rz1) = (czw - maph * 0.21, czw + maph * 0.21);
        let post = |x: f32, z: f32, body: &mut Mesh3, glow: &mut Mesh3| {
            if !self.walkable_world(x, z) {
                return;
            }
            body.merge(&boxx(Vec3::new(x, gy + 0.5, z), Vec3::new(0.08, 0.5, 0.08), scale_rgb(metal, 0.7), FACE_ALL));
            glow.merge(&boxx(Vec3::new(x, gy + 1.0, z), Vec3::new(0.1, 0.05, 0.1), pal.accent, FACE_ALL));
        };
        let step = 3.0;
        let mut x = rx0;
        while x <= rx1 {
            post(x, rz0, &mut mods, &mut cyan);
            post(x, rz1, &mut mods, &mut cyan);
            x += step;
        }
        let mut z = rz0;
        while z <= rz1 {
            post(rx0, z, &mut mods, &mut cyan);
            post(rx1, z, &mut mods, &mut cyan);
            z += step;
        }

        // ── Cargo containers as cover ────────────────────────────────────────────────────────────────────
        let crate_cols = [rgb(150, 110, 60), rgb(70, 120, 140), rgb(120, 70, 80), rgb(90, 130, 90)];
        for _ in 0..14 {
            let cx = mapw * (0.30 + rng.f32() * 0.40);
            let cz = maph * (0.30 + rng.f32() * 0.40);
            if !self.walkable_world(cx, cz) {
                continue;
            }
            let h = 0.5 + rng.f32() * 0.4;
            let col = crate_cols[(rng.next_u64() as usize) % crate_cols.len()];
            let mut cr = boxx(Vec3::new(cx, gy + h, cz), Vec3::new(0.7, h, 0.9), col, FACE_ALL);
            tint_up(&mut cr, lerp_c(col, rgb(255, 255, 255), 0.12), scale_rgb(col, 0.6));
            rotate_y_about(&mut cr, Vec3::new(cx, gy, cz), rng.f32() * 1.5);
            mods.merge(&cr);
            self.obstacles.push(Obstacle { center: Vec3::new(cx, gy + h, cz), half: Vec3::new(0.9, h, 0.9) });
        }

        // ── Distant starfield / running-lights on the void horizon ───────────────────────────────────────
        let mut stars = Mesh3::new(rgb(220, 228, 255)).emit(1.0);
        for i in 0..70 {
            let a = crate::mesh::hash01(i, 1) * std::f32::consts::TAU;
            let r = mapw * (0.9 + crate::mesh::hash01(i, 2) * 1.4);
            let sy = gy + 6.0 + crate::mesh::hash01(i, 3) * 60.0;
            let sp = Vec3::new(cxw + a.cos() * r, sy, czw + a.sin() * r);
            stars.merge(&boxx(sp, Vec3::splat(0.6 + crate::mesh::hash01(i, 4) * 0.8), stars.color, FACE_ALL));
        }

        self.meshes.push(body);
        self.meshes.push(mods);
        self.meshes.push(cyan);
        self.meshes.push(warm);
        self.meshes.push(stars);
    }

    /// True if the world point sits over a walkable deck tile (used by Station dressing to stay on the deck).
    fn walkable_world(&self, wx: f32, wz: f32) -> bool {
        let ts = self.ts();
        let (tx, tz) = ((wx / ts) as i32, (wz / ts) as i32);
        self.in_bounds(tx, tz) && self.cell(tx, tz).walkable()
    }

    /// Dress the carrier deck: a teal ocean sheet ringing the hull, a starboard superstructure island (bridge
    /// windows, radar domes, antenna masts), yellow centreline + dashed guide-lines, deck-edge cable netting on
    /// bollards, and arrester wires across the stern.
    fn build_carrier(&mut self, _rng: &mut Rng) {
        let ts = self.ts();
        let gy = self.floor_y;
        let pal = self.pal;
        let mapw = self.w as f32 * ts;
        let maph = self.h as f32 * ts;
        let cxw = mapw * 0.5;
        let half = (self.w as f32 * 0.16).max(6.0) * ts;
        let mut steel = Mesh3::new(pal.wall);
        let mut paint = Mesh3::new(pal.accent).emit(0.18);
        let mut net = Mesh3::new(rgb(44, 46, 50));
        // ── Ocean: a vast faceted teal sheet below the deck, reflective, extending past the map ──────────
        let mut ocean = Mesh3::new(rgb(26, 70, 80));
        ocean.reflect = 0.5;
        let oy = gy - 3.0;
        let span = mapw * 3.0;
        let o0 = cxw - span * 0.5;
        let oz0 = maph * 0.5 - span * 0.5;
        let div = 16;
        let step = span / div as f32;
        for iz in 0..div {
            for ix in 0..div {
                let x0 = o0 + ix as f32 * step;
                let z0 = oz0 + iz as f32 * step;
                let wob = |x: f32, z: f32| oy + (x * 0.05).sin() * 0.3 + (z * 0.045).cos() * 0.3;
                ocean.push_quad(
                    Vec3::new(x0, wob(x0, z0), z0),
                    Vec3::new(x0 + step, wob(x0 + step, z0), z0),
                    Vec3::new(x0 + step, wob(x0 + step, z0 + step), z0 + step),
                    Vec3::new(x0, wob(x0, z0 + step), z0 + step),
                    Vec3::UP,
                    lerp_c(rgb(26, 70, 80), rgb(18, 52, 64), ((ix + iz) & 1) as f32 * 0.4),
                );
            }
        }
        set_alpha(&mut ocean, 235);
        ocean.lod_far = 1.0;
        // ── Superstructure island (starboard, aft of midship) ───────────────────────────────────────────
        let ix0 = cxw + half + 1.0;
        let iz0 = maph * 0.66;
        if self.walkable_world(ix0 - 0.5, iz0) || true {
            let isl_c = Vec3::new(ix0, gy + 3.4, iz0);
            let mut isl = boxx(isl_c, Vec3::new(2.4, 3.4, 5.0), pal.wall, FACE_NO_BOTTOM);
            tint_up(&mut isl, lerp_c(pal.wall, rgb(180, 188, 198), 0.18), scale_rgb(pal.wall, 0.6));
            steel.merge(&isl);
            // bridge windows
            paint.merge(&boxx(Vec3::new(ix0 - 2.42, gy + 4.6, iz0), Vec3::new(0.04, 0.4, 3.6), rgb(60, 80, 96), FACE_ALL));
            // radar domes + antenna masts on top
            steel.merge(&uv_sphere(Vec3::new(ix0, gy + 7.2, iz0 - 1.4), 0.7, 8, 5, scale_rgb(pal.trim, 0.9)));
            steel.merge(&uv_sphere(Vec3::new(ix0, gy + 7.0, iz0 + 1.6), 0.5, 8, 5, scale_rgb(pal.trim, 0.85)));
            for &mz in &[-1.5_f32, 0.0, 1.5] {
                let base = Vec3::new(ix0, gy + 6.8, iz0 + mz);
                let tip = Vec3::new(ix0, gy + 9.0 + mz.abs(), iz0 + mz);
                steel.merge(&tube(base, tip, 0.05, 4, pal.trim));
                paint.merge(&crystal(tip, 0.18, 0.05, 4, mz as u32 + 3, rgb(255, 70, 60)));
            }
            self.obstacles.push(Obstacle { center: isl_c, half: Vec3::new(2.4, 3.4, 5.0) });
        }
        // ── Deck markings: a solid yellow centreline + dashed touchdown stripes ──────────────────────────
        paint.merge(&boxx(Vec3::new(cxw, gy + 0.04, maph * 0.5), Vec3::new(0.18, 0.03, maph * 0.44), pal.accent, FACE_TOP));
        let mut z = maph * 0.1;
        while z < maph * 0.9 {
            paint.merge(&boxx(Vec3::new(cxw - half * 0.5, gy + 0.04, z), Vec3::new(0.5, 0.03, 0.16), pal.accent, FACE_TOP));
            z += 3.0;
        }
        // landing circle near the bow
        for k in 0..20 {
            let a = k as f32 / 20.0 * std::f32::consts::TAU;
            paint.merge(&boxx(Vec3::new(cxw + a.cos() * half * 0.6, gy + 0.04, maph * 0.2 + a.sin() * half * 0.6), Vec3::new(0.18, 0.03, 0.18), pal.accent, FACE_TOP));
        }
        // ── Deck-edge cable netting on bollards (both edges) ─────────────────────────────────────────────
        for &edge in &[-1.0_f32, 1.0] {
            let ex = cxw + edge * half;
            let mut zp = maph * 0.08;
            let mut prev: Option<Vec3> = None;
            while zp < maph * 0.92 {
                let post = Vec3::new(ex, gy, zp);
                net.merge(&boxx(Vec3::new(ex, gy + 0.4, zp), Vec3::new(0.08, 0.4, 0.08), rgb(60, 64, 70), FACE_ALL));
                if let Some(p) = prev {
                    net.merge(&tube(Vec3::new(p.x, gy + 0.55, p.z), Vec3::new(ex, gy + 0.55, zp), 0.03, 3, rgb(40, 42, 46)));
                    net.merge(&tube(Vec3::new(p.x, gy + 0.3, p.z), Vec3::new(ex, gy + 0.3, zp), 0.03, 3, rgb(40, 42, 46)));
                }
                prev = Some(post);
                zp += 3.0;
            }
        }
        // ── Arrester wires across the stern ──────────────────────────────────────────────────────────────
        for k in 0..4 {
            let z = maph * 0.78 + k as f32 * 1.6;
            net.merge(&tube(Vec3::new(cxw - half, gy + 0.06, z), Vec3::new(cxw + half, gy + 0.06, z), 0.04, 3, rgb(30, 30, 34)));
        }
        self.meshes.push(ocean);
        self.meshes.push(steel);
        self.meshes.push(paint);
        self.meshes.push(net);
    }


    /// Build a playable level from an author's `CustomMap`: an open biome arena (whole interior walkable, a
    /// perimeter wall, flat ground for predictable placement) with every placed asset instanced as the same
    /// mesh primitives the generator uses. No procedural rooms / grove / landmark — the author placed it all.
    pub fn from_custom(map: &crate::builder::CustomMap) -> Level {
        let theme = map.theme;
        let cfg = MapConfig { seed: 1, theme, size: map.size.clamp(64, 240), ..MapConfig::default() };
        let (w, h) = (cfg.size, cfg.size);
        let mut lvl = Level {
            pal: theme.palette(),
            w,
            h,
            cells: vec![Cell::Solid; (w * h) as usize],
            heights: vec![0.0; (w * h) as usize],
            rooms: Vec::new(),
            obstacles: Vec::new(),
            platforms: Vec::new(),
            meshes: Vec::new(),
            floor_y: 0.0,
            ceil_y: cfg.wall_height,
            hazard_y: 0.0,
            hazard_lava: false,
            has_hazard: false,
            hazard_dps: 0.0,
            trail: Vec::new(),
            stream: Vec::new(),
            ponds: Vec::new(),
            cannons: Vec::new(),
            air_currents: Vec::new(),
            skylights: Vec::new(),
            water: Vec::new(),
            graph_elev: Vec::new(),
            agent_height: Vec::new(),
            space_plan: Vec::new(),
            destructibles: Vec::new(),
            flood: Vec::new(),
            sandstorm: None,
            cover_obstacles: Vec::new(),
            open_air: false,
            fire_sources: Vec::new(),
            custom_pickups: Vec::new(),
            lane_paths: Vec::new(),
            lod_field: None,
            cfg,
        };
        // Open arena: whole interior is Floor; a 2-tile border ring is Wall.
        for z in 0..h {
            for x in 0..w {
                let idx = (z * w + x) as usize;
                lvl.cells[idx] = if x <= 1 || z <= 1 || x >= w - 2 || z >= h - 2 { Cell::Wall } else { Cell::Floor };
            }
        }
        // One room, a small footprint around the author's spawn so `spawn(0)` lands there.
        let ts = lvl.ts();
        let (sx, sz) = ((map.spawn.0 / ts) as i32, (map.spawn.1 / ts) as i32);
        let rx0 = (sx - 6).clamp(2, w - 4);
        let rz0 = (sz - 6).clamp(2, h - 4);
        let rx1 = (sx + 6).clamp(rx0 + 1, w - 2);
        let rz1 = (sz + 6).clamp(rz0 + 1, h - 2);
        lvl.rooms.push(Room {
            rect: Rect::new(rx0, rz0, rx1, rz1),
            id: 0,
            center: Vec3::new(map.spawn.0, 0.0, map.spawn.1),
            floor_y: 0.0,
            ceil_y: lvl.ceil_y,
        });
        // Bake floor + perimeter walls (no procedural cover/grove), then instance the placed assets.
        lvl.build_meshes();
        for p in &map.placed {
            lvl.add_placed(p);
        }
        lvl
    }

    /// Dispatch to a hand-authored map. These bypass procedural generation entirely — every structure is
    /// placed deliberately — but produce an ordinary `Level`, so spawning, combat, collision and the flood
    /// simulation all run unchanged.
    fn build_authored(cfg: MapConfig) -> Level {
        match cfg.authored {
            Some(0) => Self::build_flood_zone(cfg),
            Some(1) => Self::build_reference_town(cfg),
            Some(2) => Self::build_desert_town(cfg),
            Some(3) => Self::build_pyre_garage(cfg),
            Some(4) => Self::build_concrete_slab(cfg),
            Some(5) => Self::build_redwood_canyon(cfg),
            Some(6) => Self::build_malecon(cfg),
            Some(7) => Self::build_spire_foundry(cfg),
            _ => Self::build_flood_zone(cfg),
        }
    }

    /// **Reference-driven map.** Stamp a categorical blueprint extracted from a real human-built town (parsed
    /// from a Minecraft Anvil world via the `tools/` pipeline) into a playable arena: the street grid becomes
    /// the fightable network, building footprints become solid massing you weave between, yards stay open.
    /// This is the "reverse-engineer good maps" path — real spatial design instead of procedural guesswork.
    /// Skinned warm and dusty as a first step toward the earthquake-struck desert town.
    fn build_reference_town(cfg: MapConfig) -> Level {
        // Legend: '.' open  '#' building  '+' street  '~' water  '^' nature  ':' sand
        const BP: &str = include_str!("blueprints/town.txt");
        let rows: Vec<&str> = BP.lines().filter(|l| !l.is_empty()).collect();
        let gh = rows.len() as i32;
        let gw = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0) as i32;
        let border = 2;
        let w = gw + border * 2;
        let h = gh + border * 2;
        let pal = Palette {
            floor: rgb(150, 130, 102),  // dusty packed earth
            wall: rgb(176, 138, 100),   // sun-baked clay
            trim: rgb(196, 170, 140),
            ceil: rgb(70, 64, 58),
            cover: rgb(168, 150, 120),
            sky: rgb(206, 188, 150),
            fog: rgb(196, 176, 146),
            accent: rgb(120, 150, 150),
            rock: rgb(150, 134, 112),
        };
        let seed_u = cfg.seed;
        let mut acfg = cfg;
        acfg.theme = MapTheme::Urban; // brick/stone materials read as clay block
        let mut lvl = Self::authored_blank(acfg, w, h, pal, 14.0);
        let ts = lvl.ts();

        // stamp the blueprint
        let mut street_cells: Vec<(i32, i32)> = Vec::new();
        for (gz, row) in rows.iter().enumerate() {
            for (gx, c) in row.chars().enumerate() {
                let (x, z) = (gx as i32 + border, gz as i32 + border);
                match c {
                    '#' => lvl.set(x, z, Cell::Wall), // building massing = solid block
                    '~' => lvl.set(x, z, Cell::Floor), // water handled as low floor for now
                    _ => {
                        lvl.set(x, z, Cell::Floor);
                        if c == '+' {
                            street_cells.push((x, z));
                        }
                    }
                }
            }
        }

        // ── Building factory: turn qualifying '#' massing into real facades + furnished interiors ────────
        // Detect connected '#' regions, keep the ones that are reasonably rectangular and building-sized,
        // and hand them to `facade::build_facade` — a real shell with a role-appropriate window rhythm, a
        // door, a standable roof, and a furnished (and sim-verified reachable) interior — instead of leaving
        // them as inert massing. Irregular or oddly-sized clusters fall back to the old window-punch
        // treatment below so nothing regresses to a blind monolith.
        //
        // Cell carving happens now, but the actual facade/interior *geometry* is generated after
        // `build_meshes()` runs further down — that call replaces `self.meshes` wholesale (it bakes the
        // floor/wall visual pass from scratch), so anything pushed before it would be silently discarded.
        let mut converted: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();
        let mut building_plans: Vec<(f32, f32, f32, f32, i32, u8, crate::spaces::SpaceFunction, u32)> = Vec::new();
        {
            use crate::spaces::SpaceFunction;
            let mut seen = vec![false; (w * h) as usize];
            let mut building_idx: u32 = 0;
            for sz in border..h - border {
                for sx in border..w - border {
                    let sidx = (sz * w + sx) as usize;
                    if seen[sidx] || lvl.cell(sx, sz) != Cell::Wall {
                        continue;
                    }
                    // flood-fill this connected '#' component
                    let mut stack = vec![(sx, sz)];
                    seen[sidx] = true;
                    let mut cells: Vec<(i32, i32)> = Vec::new();
                    let (mut x0, mut z0, mut x1, mut z1) = (sx, sz, sx, sz);
                    while let Some((x, z)) = stack.pop() {
                        cells.push((x, z));
                        x0 = x0.min(x);
                        z0 = z0.min(z);
                        x1 = x1.max(x);
                        z1 = z1.max(z);
                        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                            let (nx, nz) = (x + dx, z + dz);
                            if nx < border || nz < border || nx >= w - border || nz >= h - border {
                                continue;
                            }
                            let nidx = (nz * w + nx) as usize;
                            if !seen[nidx] && lvl.cell(nx, nz) == Cell::Wall {
                                seen[nidx] = true;
                                stack.push((nx, nz));
                            }
                        }
                    }
                    let (bw, bh) = (x1 - x0 + 1, z1 - z0 + 1);
                    let fill_ratio = cells.len() as f32 / (bw * bh) as f32;
                    let qualifies = fill_ratio > 0.72 && bw >= 3 && bh >= 3 && bw <= 16 && bh <= 16;
                    if !qualifies {
                        continue;
                    }
                    // door side: whichever bbox edge has the most adjacent street/open Floor just outside it
                    let edge_open = |side: u8| -> i32 {
                        let mut n = 0;
                        match side {
                            0 => {
                                for x in x0..=x1 {
                                    if lvl.cell(x, z0 - 1) == Cell::Floor {
                                        n += 1;
                                    }
                                }
                            }
                            1 => {
                                for x in x0..=x1 {
                                    if lvl.cell(x, z1 + 1) == Cell::Floor {
                                        n += 1;
                                    }
                                }
                            }
                            2 => {
                                for z in z0..=z1 {
                                    if lvl.cell(x0 - 1, z) == Cell::Floor {
                                        n += 1;
                                    }
                                }
                            }
                            _ => {
                                for z in z0..=z1 {
                                    if lvl.cell(x1 + 1, z) == Cell::Floor {
                                        n += 1;
                                    }
                                }
                            }
                        }
                        n
                    };
                    let door_side = [0u8, 1, 2, 3].into_iter().max_by_key(|&s| edge_open(s)).unwrap_or(0);
                    if edge_open(door_side) == 0 {
                        continue; // fully landlocked component (shouldn't happen post-carve, but stay safe)
                    }
                    let bseed = (seed_u as u32).wrapping_add(building_idx.wrapping_mul(7919)) ^ 0xB111D;
                    let mut brng = Rng::new(bseed as u64);
                    let stories = 1 + (brng.f32() * 2.4) as i32; // 1..3, biased toward 1-2
                    let role = match building_idx % 6 {
                        0 => SpaceFunction::Stronghold,
                        1 => SpaceFunction::Plaza,
                        2 => SpaceFunction::Muster,
                        3 => SpaceFunction::Crossing,
                        4 => SpaceFunction::Nest,
                        _ => SpaceFunction::Gauntlet,
                    };
                    let (cx, cz) = ((x0 + x1 + 1) as f32 * 0.5 * ts, (z0 + z1 + 1) as f32 * 0.5 * ts);
                    let (hx, hz) = ((bw as f32) * 0.5 * ts - 0.15, (bh as f32) * 0.5 * ts - 0.15);
                    building_plans.push((cx, cz, hx, hz, stories, door_side, role, bseed));
                    // carve the whole footprint to Floor: the facade's own Obstacle list is the real wall
                    // collision, so the grid just needs to stop being solid mass here
                    for &(cxi, czi) in &cells {
                        lvl.set(cxi, czi, Cell::Floor);
                        converted.insert((cxi, czi));
                    }
                    building_idx += 1;
                }
            }
        }

        // Punch windows into whatever building massing *wasn't* converted to a real facade above (irregular
        // or oddly-sized clusters). Stamped straight from the blueprint, every '#' was a solid block with no
        // depth or transparency; only single-exposed-face cells qualify (exactly one Floor/street neighbour),
        // so this can only ever open a shallow street-facing nook, never carve a tunnel through a building —
        // purely additive to Floor, so it can't disconnect anything either.
        {
            let mut rng = Rng::new(seed_u ^ 0x1401);
            let mut punches: Vec<(i32, i32)> = Vec::new();
            for z in border..h - border {
                for x in border..w - border {
                    if lvl.cell(x, z) != Cell::Wall || converted.contains(&(x, z)) {
                        continue;
                    }
                    let sides = [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)];
                    let open_sides = sides.iter().filter(|&&(nx, nz)| lvl.cell(nx, nz) == Cell::Floor).count();
                    if open_sides == 1 && rng.chance(0.14) {
                        punches.push((x, z));
                    }
                }
            }
            for (x, z) in punches {
                lvl.set(x, z, Cell::Floor);
            }
        }

        // spawns: two street cells far apart (opposite ends of the grid)
        let pick_far = |cells: &[(i32, i32)], corner: (i32, i32)| -> (i32, i32) {
            cells
                .iter()
                .copied()
                .min_by_key(|&(x, z)| (x - corner.0).pow(2) + (z - corner.1).pow(2))
                .unwrap_or((w / 2, h / 2))
        };
        let s0 = pick_far(&street_cells, (border + 2, border + 2));
        let s1 = pick_far(&street_cells, (w - border - 2, h - border - 2));
        for (id, (sx, sz)) in [(0usize, s0), (1, s1)].into_iter() {
            lvl.rooms.push(Room {
                rect: Rect::new(sx - 2, sz - 2, sx + 2, sz + 2),
                id,
                center: Vec3::new(sx as f32 * ts, 0.0, sz as f32 * ts),
                floor_y: 0.0,
                ceil_y: lvl.ceil_y,
            });
        }

        lvl.build_meshes();

        // Now generate the actual facade/interior geometry for the buildings detected above — deferred past
        // `build_meshes()` since that call replaces `self.meshes` wholesale.
        {
            use crate::facade::{build_facade, furnish_interior, sim_interior_reachable};
            for (cx, cz, hx, hz, stories, door_side, role, bseed) in building_plans {
                let building = build_facade(cx, cz, hx, hz, stories, door_side, 0.0, role, &pal, bseed);
                let (fm, fo) = furnish_interior(&building, 0.0, &pal, role, bseed);
                let mut all_obs = building.obstacles.clone();
                all_obs.extend(fo.iter().copied());
                let (final_fm, final_fo) = if sim_interior_reachable(&building, &all_obs, 0.0) {
                    (fm, fo)
                } else {
                    (Vec::new(), Vec::new()) // furniture blocked the door — ship the empty shell instead
                };
                lvl.meshes.extend(building.meshes);
                lvl.obstacles.extend(building.obstacles);
                lvl.meshes.extend(final_fm);
                lvl.obstacles.extend(final_fo);
                // register the standable roof as a platform for vertical-interest scoring
                let (fx0, fz0, fx1, fz1) = building.footprint;
                lvl.platforms.push(Platform { x0: fx0, z0: fz0, x1: fx1, z1: fz1, top: building.roof_y });
            }
        }

        // snapshot: everything above is building-shell geometry; props added below are freestanding cover
        let ref_shell_obs = lvl.obstacles.len();

        // earthquake-struck dressing: a scatter of breakable rubble/stalls (overturned market) in the streets
        use crate::destructible::{Destructible, DestructibleKind};
        let mut s = seed_u | 1;
        let mut rnd = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f32 / (1u64 << 53) as f32
        };
        let mut placed = 0;
        let mut tries = 0;
        while placed < 22 && tries < 400 {
            tries += 1;
            if street_cells.is_empty() {
                break;
            }
            let (cx, cz) = street_cells[(rnd() * street_cells.len() as f32) as usize % street_cells.len()];
            let c = Vec3::new(cx as f32 * ts, 0.0, cz as f32 * ts);
            let dseed = (seed_u as u32).wrapping_add(placed * 211).wrapping_mul(2_654_435_761);
            let d = if rnd() < 0.55 {
                Destructible::new(DestructibleKind::CrateStack, Vec3::new(c.x, 0.55, c.z), Vec3::new(0.6, 0.55, 0.6), dseed)
            } else {
                Destructible::new(DestructibleKind::Barrier, Vec3::new(c.x, 0.5, c.z), Vec3::new(0.9, 0.5, 0.32), dseed)
            };
            lvl.destructibles.push(d);
            placed += 1;
        }

        // ── street furniture: solid cover props scattered through the street network ──────────────────────
        // Planters, low walls, and overturned stall frames give real fightable cover between the buildings —
        // the destructibles above are breakable, these are the persistent hard cover the scorer measures.
        let prop_col = scale_rgb(pal.trim, 0.9);
        let mut furn_placed = 0;
        let mut furn_tries = 0;
        while furn_placed < 34 && furn_tries < 600 {
            furn_tries += 1;
            if street_cells.is_empty() {
                break;
            }
            let (cx, cz) = street_cells[(rnd() * street_cells.len() as f32) as usize % street_cells.len()];
            let wx = cx as f32 * ts;
            let wz = cz as f32 * ts;
            if lvl.obstacles[ref_shell_obs..].iter().any(|o| (o.center.x - wx).abs() < 1.4 && (o.center.z - wz).abs() < 1.4) {
                continue;
            }
            let roll = rnd();
            let (half, cy) = if roll < 0.4 {
                (Vec3::new(0.55, 0.55, 0.55), 0.55) // stone planter
            } else if roll < 0.7 {
                (Vec3::new(0.85, 0.35, 0.32), 0.35) // low bench
            } else {
                (Vec3::new(0.7, 0.8, 0.5), 0.8) // overturned stall / crate wall
            };
            let c = Vec3::new(wx, cy, wz);
            let mut mesh = boxx(c, half, dither(prop_col, cx * 5 + cz, 0.08), FACE_ALL);
            mesh.material = Some(crate::mesh::Material::StoneBlock);
            lvl.meshes.push(mesh);
            lvl.obstacles.push(Obstacle { center: c, half });
            furn_placed += 1;
        }
        lvl.cover_obstacles = lvl.obstacles[ref_shell_obs..].to_vec();
        lvl.open_air = true; // town streets sit under open sky

        // the town's outer edge — same skybox + cliff/abyss boundary treatment as the procedural maps
        lvl.build_cliffs();
        lvl.dissolve_boundary();
        lvl.dress_boundary_margin(&mut Rng::new(seed_u ^ 0xB0DE));
        lvl.chunk_meshes(40.0); // whole-chunk frustum culling on the town's dense building geometry

        lvl
    }

    /// **MAP 3 — Desert Town.** A tight knot of 2–3 story clay buildings around a central market courtyard,
    /// with an open street down the west flank and a large carpeted hall (a ruined hotel lobby) down the east.
    /// Buildings have flat standable roofs, vaultable windows and enterable ground floors, so the fight runs
    /// across rooftops, through alleys, and in and out of doorways. An earthquake and fire have left rubble,
    /// scorch and overturned stalls — and a sandstorm sweeps through on a cycle, dropping visibility and
    /// shoving you off your line.
    fn build_desert_town(cfg: MapConfig) -> Level {
        let seed_u = cfg.seed;
        let w = 58;
        let h = 58;
        let pal = Palette {
            floor: rgb(168, 146, 112),
            wall: rgb(184, 144, 104),
            trim: rgb(150, 120, 88),
            ceil: rgb(72, 64, 56),
            cover: rgb(170, 150, 120),
            sky: rgb(214, 196, 158),
            fog: rgb(206, 188, 152),
            accent: rgb(150, 110, 70),
            rock: rgb(158, 138, 112),
        };
        let mut acfg = cfg;
        acfg.theme = MapTheme::Urban;
        let mut lvl = Self::authored_blank(acfg, w, h, pal, 16.0);
        let ts = lvl.ts();
        let mut rng = Rng::new(seed_u ^ 0xDE5E47);

        // ── Sand banks / dunes ───────────────────────────────────────────────────────────────────────────
        // The town sits in drifted desert: raise smooth wind-blown dunes around the map edges and pile sand
        // banks that lean against the outer buildings, so the ground reads as sand that has heaped up over the
        // quake-cracked streets rather than a dead-flat plane. The central plaza/streets stay low and walkable.
        {
            let cx = w as f32 * 0.5;
            let cz = h as f32 * 0.5;
            for z in 0..h {
                for x in 0..w {
                    let idx = (z * w + x) as usize;
                    let fx = x as f32;
                    let fz = z as f32;
                    // distance from town centre, normalised — dunes rise toward the edges
                    let d = (((fx - cx) / cx).powi(2) + ((fz - cz) / cz).powi(2)).sqrt().min(1.0);
                    // two octaves of value noise for a natural wind-rippled dune field
                    let n = crate::mesh::fbm2(fx * 0.12, fz * 0.12, 3, seed_u as u32 ^ 0x5A11D)
                        + 0.4 * crate::mesh::fbm2(fx * 0.3, fz * 0.3, 2, seed_u as u32 ^ 0xD00E5);
                    let dune = (n - 0.4).max(0.0); // only the crests rise
                    // edge falloff: near the centre (d<0.45) stay flat; ramp up toward the rim
                    let edge = ((d - 0.45) / 0.55).clamp(0.0, 1.0);
                    let bank = edge * (0.4 + dune * 1.8);
                    if bank > 0.02 {
                        lvl.heights[idx] = lvl.heights[idx].max(bank);
                    }
                }
            }
            // A few tall drifted banks heaped against random spots (as if sand blew up over cracked walls),
            // smoothed so you can scramble up them — these give real vertical interest at the edges.
            for _ in 0..5 {
                let bx = rng.i32_range(4, w - 4);
                let bz = rng.i32_range(4, h - 4);
                // keep banks out of the central town
                if ((bx as f32 - w as f32 * 0.5).abs() < 12.0) && ((bz as f32 - h as f32 * 0.5).abs() < 12.0) {
                    continue;
                }
                let peak = 1.6 + rng.f32() * 1.4;
                let rad = 5 + rng.i32_range(0, 4);
                for dz in -rad..=rad {
                    for dx in -rad..=rad {
                        let x = bx + dx;
                        let z = bz + dz;
                        if x < 0 || z < 0 || x >= w || z >= h {
                            continue;
                        }
                        let dist = ((dx * dx + dz * dz) as f32).sqrt() / rad as f32;
                        if dist >= 1.0 {
                            continue;
                        }
                        // a smooth cosine dune profile
                        let rise = peak * (0.5 + 0.5 * (dist * std::f32::consts::PI).cos());
                        let idx = (z * w + x) as usize;
                        lvl.heights[idx] = lvl.heights[idx].max(rise);
                    }
                }
            }
        }

        let court = Rect::new(22, 22, 36, 36);
        let left_street = (3, 10);
        let hall = Rect::new(44, 12, 54, 46);

        let in_rect = |r: &Rect, x: i32, z: i32| x >= r.x0 && x <= r.x1 && z >= r.z0 && z <= r.z1;
        let plot = 9;
        let inset = 1;
        let mut rooftops: Vec<(f32, f32, f32)> = Vec::new(); // (cx, cz, roof_top) for cable runs
        let mut pz = 3;
        while pz + plot < h - 3 {
            let mut px = 3;
            while px + plot < w - 3 {
                let (tx0, tz0, tx1, tz1) = (px + inset, pz + inset, px + plot - inset, pz + plot - inset);
                let ccx = (tx0 + tx1) / 2;
                let ccz = (tz0 + tz1) / 2;
                let skip = in_rect(&court, ccx, ccz) || (ccx >= left_street.0 && ccx <= left_street.1) || in_rect(&hall, ccx, ccz);
                if !skip {
                    let cx = (tx0 + tx1) as f32 * 0.5 * ts;
                    let cz = (tz0 + tz1) as f32 * 0.5 * ts;
                    let hx = (tx1 - tx0) as f32 * 0.5 * ts;
                    let hz = (tz1 - tz0) as f32 * 0.5 * ts;
                    let stories = 2 + (rng.f32() < 0.4) as i32;
                    let door = (rng.f32() * 4.0) as u8;
                    let bseed = rng.next_u64() as u32;
                    // Roughly 60% of buildings get an external staircase to the roof, on a side away from
                    // the doorway — making rooftops a real traversal lane, not a mantling puzzle.
                    let stair = if rng.f32() < 0.6 { Some(((door as usize + 2) % 4) as u8) } else { None };
                    let (bm, bo) = clay_building_ext(cx, cz, hx, hz, stories, door, stair, 0.0, pal, bseed);
                    lvl.meshes.extend(bm);
                    lvl.obstacles.extend(bo);
                    // rooftop clutter (AC unit, antenna, pipes) inside the parapet
                    let roof_top = stories as f32 * 3.0 + 0.32;
                    let (rm, ro) = roof_clutter(cx, cz, hx * 0.55, hz * 0.55, roof_top, bseed ^ 0xAC);
                    lvl.meshes.extend(rm);
                    lvl.obstacles.extend(ro);
                    // remember this rooftop so cables can be strung between neighbours after the grid pass
                    rooftops.push((cx, cz, roof_top));
                    // register the standable roof surface as a platform so the scorer sees vertical interest
                    lvl.platforms.push(Platform { x0: cx - hx, z0: cz - hz, x1: cx + hx, z1: cz + hz, top: roof_top });
                }
                px += plot;
            }
            pz += plot;
        }

        // snapshot: everything added so far is building-shell geometry; props added after are cover
        let building_shell_obs = lvl.obstacles.len();

        for (id, (sx, sz)) in [(0usize, (6i32, 6i32)), (1, (50, 52))].into_iter() {
            lvl.rooms.push(Room {
                rect: Rect::new(sx - 2, sz - 2, sx + 2, sz + 2),
                id,
                center: Vec3::new(sx as f32 * ts, 0.0, sz as f32 * ts),
                floor_y: 0.0,
                ceil_y: lvl.ceil_y,
            });
        }

        lvl.build_meshes();
        // desert skin: sandy ground is the one big textured surface; walls stay smooth (see build_meshes note)
        if let Some(f) = lvl.meshes.get_mut(0) {
            f.material = Some(crate::mesh::Material::Sand);
        }

        // carpeted hall (ruined hotel lobby)
        {
            let cx = (hall.x0 + hall.x1) as f32 * 0.5 * ts;
            let cz = (hall.z0 + hall.z1) as f32 * 0.5 * ts;
            let hx = (hall.x1 - hall.x0) as f32 * 0.5 * ts;
            let hz = (hall.z1 - hall.z0) as f32 * 0.5 * ts;
            // the landmark building always gets a staircase — its 3-story roof is the map's power position
            let (hm, ho) = clay_building_ext(cx, cz, hx, hz, 3, 2, Some(0), 0.0, pal, 0x404C);
            lvl.meshes.extend(hm);
            lvl.obstacles.extend(ho);
            let carpet = rgb(140, 52, 46);
            // A polished marble lobby floor spans the whole hall — a pale veined slab that reads as a grand
            // (now ruined) hotel foyer, with the carpet runner laid on top of it down the centre.
            {
                let mut marble = Mesh3::new(rgb(214, 210, 202));
                let (mw, mh) = (hx * 1.95, hz * 1.95);
                let (mu, mv) = ((mw / 2.2).ceil().max(1.0) as i32, (mh / 2.2).ceil().max(1.0) as i32);
                push_quad_grid(
                    &mut marble,
                    Vec3::new(cx - mw, 0.03, cz - mh),
                    Vec3::new(cx + mw, 0.03, cz - mh),
                    Vec3::new(cx + mw, 0.03, cz + mh),
                    Vec3::new(cx - mw, 0.03, cz + mh),
                    Vec3::new(0.0, 1.0, 0.0),
                    rgb(214, 210, 202),
                    mu,
                    mv,
                    0x40,
                );
                marble.material = Some(crate::mesh::Material::Marble);
                lvl.meshes.push(marble);
            }
            // A single quad stretches one atlas tile across the *entire* rug — on a surface this large that
            // reads as a blurry smear. Subdividing gives the cloth material real repeats at a sane real-world
            // scale, and (same as the wall fix above) more points for the PBR shading to actually vary across.
            let mut rug = Mesh3::new(carpet);
            let (rw, rh2) = (hx * 0.9, hz * 1.7); // a runner down the centre, not the whole floor now
            let (nu, nv) = ((rw / 1.3).ceil().max(1.0) as i32, (rh2 / 1.3).ceil().max(1.0) as i32);
            push_quad_grid(
                &mut rug,
                Vec3::new(cx - rw, 0.04, cz - rh2),
                Vec3::new(cx + rw, 0.04, cz - rh2),
                Vec3::new(cx + rw, 0.04, cz + rh2),
                Vec3::new(cx - rw, 0.04, cz + rh2),
                Vec3::new(0.0, 1.0, 0.0),
                carpet,
                nu,
                nv,
                7,
            );
            rug.texturize(0.4, 0.1, 7);
            rug.material = Some(crate::mesh::Material::Cloth);
            lvl.meshes.push(rug);
            // a battered HOTEL sign over the west door — a warm emissive box, half its letters dead, the
            // kind of landmark you orient by from across the map
            let sign_x = cx - hx - 0.5;
            let mut sign = boxx(Vec3::new(sign_x, 6.4, cz), Vec3::new(0.18, 0.55, 2.2), rgb(200, 120, 60), FACE_ALL);
            sign.emissive = 0.7;
            lvl.meshes.push(sign);
            let mut dead = boxx(Vec3::new(sign_x - 0.02, 6.4, cz + 1.1), Vec3::new(0.19, 0.56, 0.7), rgb(70, 48, 34), FACE_ALL);
            dead.emissive = 0.0; // the burned-out stretch of tubes
            lvl.meshes.push(dead);
            for sx in [-0.55f32, 0.55] {
                for sz in [-0.55f32, 0.55] {
                    let px = cx + sx * hx;
                    let pz = cz + sz * hz;
                    let mut col = tube(Vec3::new(px, 0.0, pz), Vec3::new(px, 7.5, pz), 0.35, 8, scale_rgb(pal.trim, 1.05));
                    col.material = Some(crate::mesh::Material::StoneBlock);
                    lvl.meshes.push(col);
                    lvl.obstacles.push(Obstacle { center: Vec3::new(px, 1.4, pz), half: Vec3::new(0.45, 1.4, 0.45) });
                }
            }
        }

        // central courtyard
        let ccx = (court.x0 + court.x1) as f32 * 0.5 * ts;
        let ccz = (court.z0 + court.z1) as f32 * 0.5 * ts;
        for k in 0..8 {
            let a = k as f32 / 8.0 * std::f32::consts::TAU;
            let px = ccx + a.cos() * 1.6;
            let pz = ccz + a.sin() * 1.6;
            let mut seg = boxx(Vec3::new(px, 0.55, pz), Vec3::new(0.5, 0.55, 0.32), scale_rgb(pal.trim, 0.95), FACE_ALL);
            rotate_y_about(&mut seg, Vec3::new(px, 0.0, pz), a);
            seg.material = Some(crate::mesh::Material::StoneBlock);
            lvl.meshes.push(seg);
            lvl.obstacles.push(Obstacle { center: Vec3::new(px, 0.55, pz), half: Vec3::new(0.5, 0.55, 0.32) });
        }
        let cloths = [rgb(180, 80, 70), rgb(90, 130, 150), rgb(200, 170, 80), rgb(120, 90, 150)];
        for (i, (ox, oz)) in [(-5.0f32, -4.0f32), (5.0, -4.0), (-5.0, 4.0), (5.0, 4.0)].into_iter().enumerate() {
            let (sm, so) = market_stall(ccx + ox, ccz + oz, 0.0, cloths[i % 4], rng.next_u64() as u32, i == 3);
            lvl.meshes.extend(sm);
            lvl.obstacles.extend(so);
        }
        for k in 0..3 {
            let z = ccz - 5.0 + k as f32 * 5.0;
            lvl.meshes.push(hanging_fabric(Vec3::new(ccx - 7.0, 4.2, z), Vec3::new(ccx + 7.0, 4.2, z), 0.8, cloths[k % 4]));
        }
        for (ox, oz, yaw) in [(-6.0f32, 0.0f32, 0.4f32), (6.0, 1.0, 2.1)] {
            let (mm, mo) = moped(ccx + ox, ccz + oz, 0.0, yaw, rgb(70, 110, 120));
            lvl.meshes.extend(mm);
            lvl.obstacles.extend(mo);
        }

        // ── detail pass: the density that sells a lived-in block ─────────────────────────────────────────
        // utility cables strung between neighbouring rooftops (nearest-neighbour pairs, capped)
        {
            let mut cables = 0;
            for i in 0..rooftops.len() {
                let (ax, az, ay) = rooftops[i];
                let mut best: Option<(usize, f32)> = None;
                for (j, &(bx, bz, _)) in rooftops.iter().enumerate().skip(i + 1) {
                    let d = ((ax - bx).powi(2) + (az - bz).powi(2)).sqrt();
                    if d < 22.0 && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                        best = Some((j, d));
                    }
                }
                if let Some((j, _)) = best {
                    let (bx, bz, by) = rooftops[j];
                    lvl.meshes.push(overhead_cable(Vec3::new(ax, ay + 0.4, az), Vec3::new(bx, by + 0.4, bz), 0.9));
                    cables += 1;
                    if cables >= 10 {
                        break;
                    }
                }
            }
        }
        // festival string lights criss-crossing the market courtyard (warm emissive bulbs)
        for k in 0..2 {
            let z = ccz - 3.0 + k as f32 * 6.0;
            for sm in string_lights(Vec3::new(ccx - 8.0, 4.6, z), Vec3::new(ccx + 8.0, 4.6, z + 2.0), 0.6, 9) {
                lvl.meshes.push(sm);
            }
        }
        // the open west street: lane paint, a manhole, and a sandbag mount halfway down
        {
            let sx = (left_street.0 + left_street.1) as f32 * 0.5 * ts;
            let s_len = (h - 12) as f32 * ts;
            for dm in road_details(sx, (h as f32 * 0.5) * ts, false, s_len, 0.0, seed_u as u32 ^ 0x57) {
                lvl.meshes.push(dm);
            }
            let (sbm, sbo) = sandbag_line(sx, (h as f32 * 0.42) * ts, true, 3.4, 0.0, seed_u as u32 ^ 0x5B);
            lvl.meshes.extend(sbm);
            lvl.obstacles.extend(sbo);
        }
        // alley clutter: inert drums and pallets tucked along building lines
        for k in 0..10u32 {
            let tx = 4 + (rng.f32() * 50.0) as i32;
            let tz = 4 + (rng.f32() * 50.0) as i32;
            if in_rect(&court, tx, tz) || in_rect(&hall, tx, tz) {
                continue;
            }
            let px = tx as f32 * ts;
            let pzz = tz as f32 * ts;
            if k % 2 == 0 {
                let (dm, dobs) = oil_drum(px, pzz, 0.0, rgb(86, 104, 96), rng.next_u64() as u32);
                lvl.meshes.extend(dm);
                lvl.obstacles.extend(dobs);
            } else {
                let (pm, pobs) = pallet(px, pzz, 0.0, rng.f32() * 3.1, rng.next_u64() as u32);
                lvl.meshes.extend(pm);
                lvl.obstacles.extend(pobs);
            }
        }

        // ── ground detail: sand drifts, weathered stones, and dry scrub scattered across the sand ──────────
        // A flat uniform sand plane is the other half of the "toy" look. Low wind-blown sand mounds, half-
        // buried rocks, and dry desert brush give the ground real relief and life without blocking movement
        // (all kept low and passable, registered as gentle terrain rather than hard cover).
        {
            let sand = pal.floor;
            for _ in 0..40 {
                let px = (3.0 + rng.f32() * 52.0) * ts;
                let pz = (3.0 + rng.f32() * 52.0) * ts;
                let (tx, tz) = ((px / ts) as i32, (pz / ts) as i32);
                if in_rect(&court, tx, tz) || in_rect(&hall, tx, tz) {
                    continue;
                }
                let kind = rng.f32();
                if kind < 0.45 {
                    // wind-blown sand drift: a low dome cap sitting on the ground. Built as a full low-poly
                    // sphere then flattened and clamped so only the top dome shows — no half-buried sphere with
                    // faces poking through the floor (which read as "holes"), and left double-sided so it can
                    // never be wrongly backface-culled from a grazing angle.
                    let r = 0.7 + rng.f32() * 1.3;
                    let hgt = 0.25 + rng.f32() * 0.3;
                    let mut mound = uv_sphere(Vec3::new(px, 0.0, pz), r, 10, 6, dither(lerp_c(sand, rgb(198, 176, 138), 0.4), rng.next_u64() as i32, 0.05));
                    // flatten to a low dome and clamp everything to at/above ground
                    for v in mound.verts.iter_mut() {
                        v.y = (v.y.max(0.0)) * (hgt / r);
                    }
                    crate::mesh::scale_xz(&mut mound, 1.0, 0.7 + rng.f32() * 0.5);
                    mound.material = Some(crate::mesh::Material::Sand);
                    mound.single_sided = false; // decorative dome — never risk a culling hole
                    lvl.meshes.push(mound);
                } else if kind < 0.75 {
                    // half-buried weathered stone
                    let r = 0.3 + rng.f32() * 0.5;
                    let mut rk = crate::mesh::rock(Vec3::new(px, r * 0.15, pz), r, rng.next_u64() as u32, dither(pal.rock, rng.next_u64() as i32, 0.08));
                    rk.material = Some(crate::mesh::Material::StoneBlock);
                    lvl.meshes.push(rk);
                    lvl.obstacles.push(Obstacle { center: Vec3::new(px, r * 0.4, pz), half: Vec3::new(r, r * 0.5, r) });
                } else {
                    // dry desert scrub: a few splayed thin brown stalks
                    let brush = rgb(120, 104, 62);
                    for _ in 0..5 {
                        let a = rng.f32() * std::f32::consts::TAU;
                        let lean = 0.15 + rng.f32() * 0.2;
                        let base = Vec3::new(px + (rng.f32() - 0.5) * 0.4, 0.0, pz + (rng.f32() - 0.5) * 0.4);
                        let tip = base + Vec3::new(a.cos() * lean, 0.5 + rng.f32() * 0.4, a.sin() * lean);
                        lvl.meshes.push(tube(base, tip, 0.02, 3, dither(brush, rng.next_u64() as i32, 0.12)));
                    }
                }
            }
        }

        // ── dangling items: loose wires, hung laundry lines, and broken signage strung between buildings and
        // off roof edges. Each hangs from an anchor and gets per-vertex sway weight that ramps from 0 at the
        // anchor to full at the free end, so the renderer bows them in the wind — the town reads as alive and
        // unsettled (and they twitch harder when a tremor hits). ────────────────────────────────────────────
        {
            for _ in 0..18 {
                let ax = (6.0 + rng.f32() * 46.0) * ts;
                let az = (6.0 + rng.f32() * 46.0) * ts;
                let anchor_y = 3.0 + rng.f32() * 4.0;
                let kind = rng.f32();
                if kind < 0.5 {
                    // a dangling wire/cable: a thin tube hanging down with a slight kink, swaying at the tip
                    let drop = 1.2 + rng.f32() * 2.0;
                    let a = Vec3::new(ax, anchor_y, az);
                    let b = a + Vec3::new((rng.f32() - 0.5) * 0.6, -drop, (rng.f32() - 0.5) * 0.6);
                    let mut wire = tube(a, b, 0.03, 3, rgb(30, 28, 30));
                    // sway weight: 0 at the top anchor, 1 at the bottom
                    let ys: Vec<f32> = wire.verts.iter().map(|v| ((anchor_y - v.y) / drop).clamp(0.0, 1.0)).collect();
                    wire.sway = ys;
                    lvl.meshes.push(wire);
                } else if kind < 0.8 {
                    // hung laundry / cloth: a small faded rag on a line, swaying
                    let col = [rgb(180, 90, 80), rgb(90, 120, 160), rgb(200, 190, 150), rgb(120, 150, 110)][(rng.f32() * 4.0) as usize % 4];
                    let cw = 0.4 + rng.f32() * 0.5;
                    let ch = 0.5 + rng.f32() * 0.6;
                    let top = anchor_y - 0.1;
                    let mut cloth = boxx(Vec3::new(ax, top - ch * 0.5, az), Vec3::new(cw, ch * 0.5, 0.03), dither(col, ax as i32, 0.06), FACE_ALL);
                    cloth.single_sided = false;
                    let ys: Vec<f32> = cloth.verts.iter().map(|v| ((top - v.y) / ch).clamp(0.0, 1.0)).collect();
                    cloth.sway = ys;
                    lvl.meshes.push(cloth);
                } else {
                    // a broken sign hanging by one corner, swinging
                    let sw = 0.5 + rng.f32() * 0.4;
                    let top = anchor_y;
                    let mut sign = boxx(Vec3::new(ax, top - 0.5, az), Vec3::new(sw, 0.3, 0.05), dither(rgb(150, 140, 110), ax as i32, 0.08), FACE_ALL);
                    let ys: Vec<f32> = sign.verts.iter().map(|v| ((top - v.y) / 0.8).clamp(0.0, 1.0) * 0.7).collect();
                    sign.sway = ys;
                    lvl.meshes.push(sign);
                    // the wire it hangs from
                    lvl.meshes.push(tube(Vec3::new(ax, top, az), Vec3::new(ax + 0.1, top + 0.4, az), 0.02, 3, rgb(40, 38, 40)));
                }
            }
        }

        // earthquake & fire damage
        use crate::destructible::{Destructible, DestructibleKind};
        for _ in 0..26 {
            let tx = 4 + (rng.f32() * 50.0) as i32;
            let tz = 4 + (rng.f32() * 50.0) as i32;
            if in_rect(&court, tx, tz) && rng.f32() < 0.5 {
                continue;
            }
            let c = Vec3::new(tx as f32 * ts, 0.0, tz as f32 * ts);
            let dseed = rng.next_u64() as u32;
            let d = if rng.f32() < 0.5 {
                Destructible::new(DestructibleKind::Barrier, Vec3::new(c.x, 0.5, c.z), Vec3::new(0.8, 0.5, 0.32), dseed)
            } else {
                Destructible::new(DestructibleKind::CrateStack, Vec3::new(c.x, 0.45, c.z), Vec3::new(0.5, 0.45, 0.5), dseed)
            };
            lvl.destructibles.push(d);
        }
        // fuel drums left out during the evacuation — shoot them: they blast, and pairs chain
        for k in 0..8u32 {
            let tx = 5 + (rng.f32() * 48.0) as i32;
            let tz = 5 + (rng.f32() * 48.0) as i32;
            if in_rect(&hall, tx, tz) {
                continue;
            }
            let px = tx as f32 * ts;
            let pz = tz as f32 * ts;
            let dseed = rng.next_u64() as u32;
            lvl.destructibles.push(Destructible::new(DestructibleKind::ExplosiveBarrel, Vec3::new(px, 0.5, pz), Vec3::new(0.34, 0.5, 0.34), dseed));
            // every third drop is a pair set close enough to chain
            if k % 3 == 0 {
                lvl.destructibles.push(Destructible::new(
                    DestructibleKind::ExplosiveBarrel,
                    Vec3::new(px + 1.0, 0.5, pz + 0.4),
                    Vec3::new(0.34, 0.5, 0.34),
                    dseed ^ 0x77,
                ));
            }
        }
        for _ in 0..10 {
            let px = (4.0 + rng.f32() * 50.0) * ts;
            let pz = (4.0 + rng.f32() * 50.0) * ts;
            let mut burn = disc(Vec3::new(px, 0.03, pz), 1.2 + rng.f32() * 1.4, 14, rgb(40, 34, 30));
            burn.texturize(0.7, 0.2, rng.next_u64() as u32);
            lvl.meshes.push(burn);
        }
        // ── FSM ground fractures: cracks radiating from the quake's stress points ─────────────────────────
        // The earthquake left the packed-earth streets split open. Each origin seeds a branching crack
        // network (a tiny state machine — each crack tip continues, forks, or dies by a hash of its id), so
        // the ground reads as genuinely fractured rather than just scorched. Concentrated near the courtyard
        // and along the main lanes where structural stress would show.
        {
            use crate::fsm::detail::crack_network;
            let mut crack_meshes = Mesh3::new(rgb(24, 22, 22));
            // a big central fracture through the courtyard, plus scattered secondary ones
            let ccx = (court.x0 + court.x1) as f32 * 0.5 * ts;
            let ccz = (court.z0 + court.z1) as f32 * 0.5 * ts;
            crack_meshes.merge(&crack_network(Vec3::new(ccx, 0.0, ccz), 9.0, 5, 0xEA47));
            for _ in 0..7 {
                let px = (5.0 + rng.f32() * 48.0) * ts;
                let pz = (5.0 + rng.f32() * 48.0) * ts;
                // don't crack under the hotel hall (its marble floor sits at 0.03)
                let (tx, tz) = ((px / ts) as i32, (pz / ts) as i32);
                if in_rect(&hall, tx, tz) {
                    continue;
                }
                crack_meshes.merge(&crack_network(Vec3::new(px, 0.0, pz), 4.0 + rng.f32() * 3.0, 2 + (rng.f32() * 2.0) as u32, rng.next_u64() as u32));
            }
            lvl.meshes.push(crack_meshes);
        }

        // populate cover_obstacles with only freestanding props (everything added after building shells)
        // so the map scorer can measure cover density independently from the building geometry
        lvl.cover_obstacles = lvl.obstacles[building_shell_obs..].to_vec();
        lvl.open_air = true; // desert town under open sky
        lvl.sandstorm = Some(Sandstorm { delay: 18.0, ramp: 6.0, hold: 12.0, ease: 6.0, gap: 16.0, wind: Vec3::new(0.7, 0.0, 0.5), phase: 0.0 });
        // the town's outer edge — same skybox + cliff/abyss boundary treatment as the procedural maps
        lvl.build_cliffs();
        lvl.dissolve_boundary();
        lvl.dress_boundary_margin(&mut rng);
        lvl.chunk_meshes(40.0); // whole-chunk frustum culling on the dense clay-block geometry
        lvl
    }

    /// **MAP 4 — The Pyre Garage.** A burning multi-story car park. Two split-level concrete decks are joined
    /// by spiralling ramps you fight up and across; late-model cars sit as heavy, explosive cover, several
    /// fully engulfed in fire that lights the deck and pours black smoke against the ceiling. A collapsed
    /// slab breaches the two ramps into a mid-level flank, and the top opens to a rooftop helipad with a
    /// crashed chopper and a ruptured fuel truck feeding a river of flame. Vertical, chaotic, apocalyptic.
    /// **MAP 5 — The Concrete Slab.** A brutalist housing project: a horizontal CQB maze of open-air external
    /// walkways ("streets in the sky") stacked over a U-shaped concrete block. The center is a collapsed
    /// rubble courtyard — a third-floor walkway has fallen in, its jagged aggregate chunks and twisted rebar
    /// breaking the long external sightlines. Graffiti-scarred inner walls, boarded windows, and enclosed
    /// board-formed stairwells connect the levels. Grey, windswept, shadowless — a single monolithic mass.
    fn build_concrete_slab(cfg: MapConfig) -> Level {
        use crate::mesh::Material;
        let seed_u = cfg.seed;
        let w = 56;
        let h = 56;
        let pal = Palette {
            floor: rgb(128, 126, 124),  // flat grey concrete
            wall: rgb(140, 138, 135),
            trim: rgb(108, 106, 104),
            ceil: rgb(150, 150, 152),   // flat overcast
            cover: rgb(124, 122, 120),
            sky: rgb(150, 150, 154),    // flat overcast grey — no shadows
            fog: rgb(146, 146, 150),
            accent: rgb(150, 90, 70),
            rock: rgb(118, 116, 114),
        };
        let mut acfg = cfg;
        acfg.theme = MapTheme::Urban;
        let mut lvl = Self::authored_blank(acfg, w, h, pal, 9.0);
        let ts = lvl.ts();
        let mut rng = Rng::new(seed_u ^ 0x5CAB);

        let concrete = rgb(128, 126, 124);
        let level_h = 3.2f32; // per-storey walkway height
        let graffiti_cols = [rgb(180, 60, 70), rgb(70, 120, 190), rgb(210, 180, 60), rgb(90, 170, 100), rgb(180, 90, 170)];

        // ── the U-shaped block: three wings (west, north, east) each carrying stacked external walkways ─────
        // Each wing is a long run of walkway tiles at two elevations (ground + upper), backed by a solid inner
        // wall (graffiti side) and fronted by a low parapet. The south side is open (the courtyard mouth).
        // wing spec: (x0, z0, x1, z1, along_x, inner_on_low_side)
        let wings: [(i32, i32, i32, i32, bool); 3] = [
            (6, 6, 12, 50, false),   // west wing (runs along Z)
            (6, 6, 50, 12, true),    // north wing (runs along X)
            (44, 6, 50, 50, false),  // east wing (runs along Z)
        ];

        for &(wx0, wz0, wx1, wz1, along_x) in &wings {
            for storey in 0..2 {
                let y = storey as f32 * level_h;
                let cx = (wx0 + wx1) as f32 * 0.5 * ts;
                let cz = (wz0 + wz1) as f32 * 0.5 * ts;
                let hx = (wx1 - wx0) as f32 * 0.5 * ts;
                let hz = (wz1 - wz0) as f32 * 0.5 * ts;
                // walkway deck
                let mut deck = boxx(Vec3::new(cx, y, cz), Vec3::new(hx, 0.28, hz), concrete, FACE_ALL);
                deck.material = Some(crate::mesh::Material::Concrete);
                lvl.meshes.push(deck);
                lvl.obstacles.push(Obstacle { center: Vec3::new(cx, y, cz), half: Vec3::new(hx, 0.28, hz) });
                lvl.platforms.push(Platform { x0: cx - hx, z0: cz - hz, x1: cx + hx, z1: cz + hz, top: y + 0.28 });

                // solid inner wall (backs the walkway) — the graffiti face, full storey height
                let (wall_c, wall_h) = if along_x {
                    // north wing: inner wall on the +z (south) edge
                    (Vec3::new(cx, y + level_h * 0.5, cz + hz - 0.2), Vec3::new(hx, level_h * 0.5, 0.2))
                } else if wx0 < 20 {
                    // west wing: inner wall on the +x (east) edge
                    (Vec3::new(cx + hx - 0.2, y + level_h * 0.5, cz), Vec3::new(0.2, level_h * 0.5, hz))
                } else {
                    // east wing: inner wall on the -x (west) edge
                    (Vec3::new(cx - hx + 0.2, y + level_h * 0.5, cz), Vec3::new(0.2, level_h * 0.5, hz))
                };
                let mut wall = boxx(wall_c, wall_h, concrete, FACE_ALL);
                wall.material = Some(crate::mesh::Material::Concrete);
                lvl.meshes.push(wall);
                lvl.obstacles.push(Obstacle { center: wall_c, half: wall_h });

                // graffiti: bright dripping patches sprayed on the inner wall face (pure dressing, no collision)
                let inner_face = if along_x { cz + hz - 0.42 } else { 0.0 };
                let n_tags = if along_x { (hx / ts) as i32 } else { (hz / ts) as i32 };
                for k in 0..n_tags {
                    if rng.f32() < 0.45 {
                        continue;
                    }
                    let col = graffiti_cols[(rng.next_u64() as usize) % graffiti_cols.len()];
                    let (gx, gz) = if along_x {
                        (cx - hx + (k as f32 + 0.5) * ts * 2.0, inner_face)
                    } else if wx0 < 20 {
                        (cx + hx - 0.42, cz - hz + (k as f32 + 0.5) * ts * 2.0)
                    } else {
                        (cx - hx + 0.42, cz - hz + (k as f32 + 0.5) * ts * 2.0)
                    };
                    let gy = y + 1.2 + (rng.f32() - 0.5) * 0.6;
                    let mut tag = boxx(Vec3::new(gx, gy, gz), Vec3::new(if along_x { 0.5 } else { 0.02 }, 0.45, if along_x { 0.02 } else { 0.5 }), col, FACE_ALL);
                    tag.emissive = 0.08; // faintly self-lit so colour reads under the flat grey light
                    lvl.meshes.push(tag);
                    // a drip below the tag
                    lvl.meshes.push(boxx(Vec3::new(gx, gy - 0.5, gz), Vec3::new(if along_x { 0.08 } else { 0.015 }, 0.3, if along_x { 0.015 } else { 0.08 }), scale_rgb(col, 0.8), FACE_ALL));
                }

                // low outer parapet along the exposed edge (waist-high cover)
                let (para_c, para_h) = if along_x {
                    (Vec3::new(cx, y + 0.6, cz - hz + 0.15), Vec3::new(hx, 0.6, 0.15))
                } else if wx0 < 20 {
                    (Vec3::new(cx - hx + 0.15, y + 0.6, cz), Vec3::new(0.15, 0.6, hz))
                } else {
                    (Vec3::new(cx + hx - 0.15, y + 0.6, cz), Vec3::new(0.15, 0.6, hz))
                };
                let mut para = boxx(para_c, para_h, scale_rgb(concrete, 0.94), FACE_ALL);
                para.material = Some(crate::mesh::Material::Concrete);
                lvl.meshes.push(para);
                lvl.obstacles.push(Obstacle { center: para_c, half: para_h });
            }
        }

        // ── three board-formed stairwells connecting ground↔upper walkways (the vertical chokes) ───────────
        // Placed at the wing junctions so you can rotate between levels. Each is a ramp box the player climbs,
        // enclosed by two side walls (the dark utility core contrasting the open walkways).
        let stair_spots: [(f32, f32, bool); 3] = [
            (9.0, 9.0, false),   // NW junction, climb along +Z
            (47.0, 9.0, false),  // NE junction
            (9.0, 30.0, true),   // mid-west, climb along +X into the courtyard mouth
        ];
        for &(sx, sz, along_x) in &stair_spots {
            let bx = sx * ts;
            let bz = sz * ts;
            let (run, side) = (4.0f32, 1.6f32);
            let (bl, br, tr, tl) = if along_x {
                (
                    Vec3::new(bx, 0.0, bz - side),
                    Vec3::new(bx, 0.0, bz + side),
                    Vec3::new(bx + run, level_h, bz + side),
                    Vec3::new(bx + run, level_h, bz - side),
                )
            } else {
                (
                    Vec3::new(bx - side, 0.0, bz),
                    Vec3::new(bx + side, 0.0, bz),
                    Vec3::new(bx + side, level_h, bz + run),
                    Vec3::new(bx - side, level_h, bz + run),
                )
            };
            let mut r = ramp(bl, br, tr, tl, scale_rgb(concrete, 0.9));
            r.material = Some(crate::mesh::Material::Concrete);
            lvl.meshes.push(r);
            // collision + platform chain up the ramp
            let steps = 8;
            for i in 0..steps {
                let t = (i as f32 + 0.5) / steps as f32;
                let (px, pz) = if along_x { (bx + run * t, bz) } else { (bx, bz + run * t) };
                let yc = level_h * t;
                lvl.obstacles.push(Obstacle { center: Vec3::new(px, yc * 0.5, pz), half: Vec3::new(side, yc * 0.5 + 0.15, side) });
                lvl.platforms.push(Platform { x0: px - side, z0: pz - side, x1: px + side, z1: pz + side, top: yc + 0.06 });
            }
        }

        // ── the rubble courtyard (center): a collapsed upper walkway fallen into the ground level ───────────
        // Jagged aggregate chunks + twisted rebar break the long external sightlines and give the open middle
        // some hard cover. A crushed shopping cart under a slab. This is the map's signature centrepiece.
        let court_cx = 28.0 * ts;
        let court_cz = 34.0 * ts;
        {
            // scattered big concrete chunks (angled slabs) forming a low rubble field
            for k in 0..14 {
                let a = hash01(k, 0x31) * std::f32::consts::TAU;
                let d = 1.0 + hash01(k, 0x77) * 6.0;
                let px = court_cx + a.cos() * d * ts * 0.5;
                let pz = court_cz + a.sin() * d * ts * 0.5;
                let sz = 0.5 + hash01(k, 0x9) * 0.9;
                let hgt = 0.4 + hash01(k, 0x5) * 0.8;
                let mut chunk = boxx(Vec3::new(px, hgt, pz), Vec3::new(sz, hgt, sz * 0.8), scale_rgb(concrete, 0.86 + hash01(k, 3) * 0.1), FACE_ALL);
                chunk.material = Some(crate::mesh::Material::Concrete);
                // cant the slab so it reads as fallen debris, not a neat box
                crate::mesh::rotate_y_about(&mut chunk, Vec3::new(px, hgt, pz), hash01(k, 0x2) * 1.2);
                lvl.meshes.push(chunk);
                lvl.obstacles.push(Obstacle { center: Vec3::new(px, hgt, pz), half: Vec3::new(sz, hgt, sz * 0.8) });
                lvl.cover_obstacles.push(Obstacle { center: Vec3::new(px, hgt, pz), half: Vec3::new(sz, hgt, sz * 0.8) });
                // rebar spikes jutting from some chunks
                if hash01(k, 0x4) > 0.5 {
                    for j in 0..3 {
                        let rb = Vec3::new(px + (hash01(k * 3 + j, 1) - 0.5) * sz, hgt, pz + (hash01(k * 3 + j, 2) - 0.5) * sz);
                        let tip = rb + Vec3::new((hash01(j, 5) - 0.5) * 0.4, 0.7 + hash01(j, 7) * 0.5, (hash01(j, 9) - 0.5) * 0.4);
                        lvl.meshes.push(tube(rb, tip, 0.03, 3, rgb(96, 66, 50)));
                    }
                }
            }
            // one big fallen slab spanning across, at an angle (the collapsed walkway section)
            let mut slab = boxx(Vec3::new(court_cx, 0.9, court_cz), Vec3::new(3.2, 0.25, 1.4), scale_rgb(concrete, 0.9), FACE_ALL);
            slab.material = Some(crate::mesh::Material::Concrete);
            crate::mesh::rotate_y_about(&mut slab, Vec3::new(court_cx, 0.9, court_cz), 0.5);
            lvl.meshes.push(slab);
            lvl.obstacles.push(Obstacle { center: Vec3::new(court_cx, 0.9, court_cz), half: Vec3::new(3.2, 0.9, 1.4) });
            lvl.cover_obstacles.push(Obstacle { center: Vec3::new(court_cx, 0.9, court_cz), half: Vec3::new(3.2, 0.9, 1.4) });
            // crushed shopping cart under the slab edge (a flattened rusty box + bent wheels)
            let cart = court_cx + 3.0;
            let mut cm = boxx(Vec3::new(cart, 0.18, court_cz + 1.2), Vec3::new(0.5, 0.18, 0.35), rgb(150, 120, 110), FACE_ALL);
            cm.material = Some(crate::mesh::Material::RustPlate);
            lvl.meshes.push(cm);
            for &(ox, oz) in &[(-0.4f32, -0.25f32), (0.4, -0.25), (-0.4, 0.25), (0.4, 0.25)] {
                lvl.meshes.push(tube(
                    Vec3::new(cart + ox, 0.08, court_cz + 1.2 + oz - 0.05),
                    Vec3::new(cart + ox + 0.1, 0.08, court_cz + 1.2 + oz + 0.05),
                    0.09,
                    6,
                    rgb(60, 58, 60),
                ));
            }
        }

        // ── some walkway clutter: a swaying broken fluorescent tube, boarded windows, litter piles ──────────
        // A few eye-height clutter props on the walkways to break the mid lanes and add lived-in texture.
        for &(tx, tz, tall) in &[(8i32, 20i32, true), (48, 24, true), (30, 10, false), (30, 48, false), (14, 46, true)] {
            let px = tx as f32 * ts;
            let pz = tz as f32 * ts;
            if tall {
                // a chunk of fallen wall / debris pile reaching eye height
                let mut m = boxx(Vec3::new(px, 0.9, pz), Vec3::new(0.7, 0.9, 0.7), scale_rgb(concrete, 0.85), FACE_ALL);
                m.material = Some(crate::mesh::Material::Concrete);
                lvl.meshes.push(m);
                lvl.obstacles.push(Obstacle { center: Vec3::new(px, 0.9, pz), half: Vec3::new(0.7, 0.9, 0.7) });
                lvl.cover_obstacles.push(Obstacle { center: Vec3::new(px, 0.9, pz), half: Vec3::new(0.7, 0.9, 0.7) });
            } else {
                // low litter/rubble mound (partial cover)
                let mut m = boxx(Vec3::new(px, 0.35, pz), Vec3::new(0.8, 0.35, 0.8), scale_rgb(concrete, 0.8), FACE_ALL);
                m.material = Some(Material::Gravel);
                lvl.meshes.push(m);
                lvl.obstacles.push(Obstacle { center: Vec3::new(px, 0.35, pz), half: Vec3::new(0.8, 0.35, 0.8) });
                lvl.cover_obstacles.push(Obstacle { center: Vec3::new(px, 0.35, pz), half: Vec3::new(0.8, 0.35, 0.8) });
            }
        }

        // ── ground-courtyard sightline breakers: scattered debris across the open floor ─────────────────────
        // The central rubble clusters cover the middle, but the surrounding ground courtyard is a long open
        // field. These scattered eye-height debris chunks (fallen slabs, planters, dumpsters) carve it into
        // fightable pockets with gaps, so the ground plays as broken cover rather than one long shooting lane.
        for &(tx, tz) in &[
            (18i32, 22i32), (38, 22), (24, 28), (34, 40), (20, 44),
            (40, 44), (28, 20), (16, 34), (40, 32), (24, 46),
        ] {
            let px = tx as f32 * ts;
            let pz = tz as f32 * ts;
            let kind = (tx + tz) % 3;
            let (hw, hgt, hd, col, mat) = match kind {
                0 => (0.9f32, 1.0f32, 0.6f32, scale_rgb(concrete, 0.88), Material::Concrete), // fallen slab
                1 => (0.7, 1.1, 0.7, rgb(90, 110, 90), Material::Corrugated),                  // dumpster
                _ => (1.0, 0.9, 1.0, scale_rgb(concrete, 0.82), Material::Gravel),             // rubble mound
            };
            let c = Vec3::new(px, hgt, pz);
            let mut m = boxx(c, Vec3::new(hw, hgt, hd), col, FACE_ALL);
            m.material = Some(mat);
            lvl.meshes.push(m);
            lvl.obstacles.push(Obstacle { center: c, half: Vec3::new(hw, hgt, hd) });
            lvl.cover_obstacles.push(Obstacle { center: c, half: Vec3::new(hw, hgt, hd) });
        }

        // ── spawns: opposite ends of the U (west-wing base and east-wing base) ──────────────────────────────
        for (id, (sx, sz)) in [(0usize, (9i32, 47i32)), (1, (47, 47))].into_iter() {
            lvl.rooms.push(Room {
                rect: Rect::new(sx - 2, sz - 2, sx + 2, sz + 2),
                id,
                center: Vec3::new(sx as f32 * ts, 0.0, sz as f32 * ts),
                floor_y: 0.0,
                ceil_y: lvl.ceil_y,
            });
        }

        lvl.build_meshes();
        // ground courtyard reads as flat grey concrete
        if let Some(f) = lvl.meshes.get_mut(0) {
            f.material = Some(crate::mesh::Material::Concrete);
        }
        lvl.open_air = true; // external walkways under open (overcast) sky

        lvl.build_cliffs();
        lvl.dissolve_boundary();
        lvl.dress_boundary_margin(&mut rng);
        lvl.chunk_meshes(40.0);
        lvl
    }

    /// **MAP 6 — Redwood Canyon.** A forested river gorge: a fordable river runs the length of the map
    /// between two raised canyon banks, crossed by fallen-log bridges. Immense old-growth redwoods rise from
    /// the banks (some standing, some toppled into walkable ramps), with boulder clusters and a ranger cabin
    /// for cover. Dim, green, deep-shadowed — a natural-terrain counterpoint to the built maps.
    /// **MAP 7 — The Malecón Breakwater.** Havana's seawall promenade: a linear two-lane map. The main lane is
    /// a wide oceanfront boulevard (devastating long sightlines) running beside a massive curving coral-
    /// aggregate seawall against a crashing turquoise ocean; the flank routes through the crumbling interiors
    /// of pastel colonial buildings (tight CQB). A rusted 1950s sedan is the only cover on the open road; an
    /// arched doorway leads to a fern-choked interior courtyard. Blinding tropical sun, ink-black awning shadow.
    fn build_malecon(cfg: MapConfig) -> Level {
        use crate::mesh::Material;
        let seed_u = cfg.seed;
        let w = 60;
        let h = 40;
        let pal = Palette {
            floor: rgb(150, 140, 120),   // sun-bleached concrete boulevard
            wall: rgb(150, 170, 180),    // pastel colonial (tinted per building below)
            trim: rgb(120, 110, 96),
            ceil: rgb(180, 190, 200),
            cover: rgb(140, 130, 116),
            sky: rgb(150, 200, 225),     // bright tropical sky
            fog: rgb(190, 210, 220),     // hazy salt air
            accent: rgb(210, 150, 90),
            rock: rgb(150, 140, 120),
        };
        let mut acfg = cfg;
        acfg.theme = MapTheme::Urban;
        let mut lvl = Self::authored_blank(acfg, w, h, pal, 12.0);
        let ts = lvl.ts();
        let mut rng = Rng::new(seed_u ^ 0xEA71);

        // The map runs along X (the boulevard). Z-bands: [2..14] seawall+ocean, [14..26] boulevard, [26..38]
        // colonial building flank.
        let _sea_z = 8.0 * ts;
        let boulevard_cz = 20.0 * ts;

        // ── the ocean: a long water body north of the seawall (a chain of discs so it reads as a sea edge) ──
        for i in 0..12 {
            let cx = (2.0 + i as f32 * 5.0) * ts;
            lvl.water.push(WaterBody {
                cx,
                cz: 4.0 * ts,
                radius: 6.0 * ts,
                y: 0.2,
                col: rgb(50, 150, 165), // turquoise
                alpha: 165,
                reflect: 0.6,
                emissive: 0.0,
                lava: false,
                disc: true,
            });
        }

        // ── the seawall: a massive continuous curving coral-aggregate barrier along z≈14, waist-to-chest high,
        // pitted with exposed rebar and algae-stained at the base. Waist-high so you vault/peek over it. ──────
        let coral = rgb(176, 168, 150);
        let algae = rgb(70, 96, 66);
        for i in 0..(w - 4) {
            let px = (2.0 + i as f32) * ts;
            // gentle curve in Z so the wall isn't a dead-straight line
            let curve = (i as f32 * 0.16).sin() * 0.6;
            let cz = 13.5 * ts + curve;
            let hgt = 1.15 + hash01(i, 0x3) * 0.2;
            let mut seg = boxx(Vec3::new(px, hgt * 0.5, cz), Vec3::new(ts * 0.55, hgt * 0.5, ts * 0.7), dither(coral, i, 0.05), FACE_ALL);
            seg.material = Some(Material::StoneBlock);
            // algae stain at the base, salt-bleach at the top
            crate::mesh::tint_height(&mut seg, 0.0, hgt, algae, lerp_c(coral, rgb(235, 232, 220), 0.4));
            lvl.meshes.push(seg);
            lvl.obstacles.push(Obstacle { center: Vec3::new(px, hgt * 0.5, cz), half: Vec3::new(ts * 0.55, hgt * 0.5, ts * 0.7) });
            lvl.cover_obstacles.push(Obstacle { center: Vec3::new(px, hgt * 0.5, cz), half: Vec3::new(ts * 0.55, hgt * 0.5, ts * 0.7) });
            // exposed rust-brown rebar where the sea has eaten through (some segments)
            if hash01(i, 0x9) > 0.6 {
                for k in 0..2 {
                    let rb = Vec3::new(px + (hash01(i * 2 + k, 1) - 0.5) * ts, hgt - 0.1, cz - 0.4);
                    lvl.meshes.push(tube(rb, rb + Vec3::new((hash01(k, 5) - 0.5) * 0.3, 0.4, -0.2), 0.03, 3, rgb(140, 74, 50)));
                }
            }
        }

        // ── the burned 1950s sedan: the only cover on the open boulevard (a rusted skeletal hulk) ───────────
        {
            let cx = 30.0 * ts;
            let cz = boulevard_cz;
            let pale_blue = rgb(150, 165, 168); // oxidized matte pale blue
            let rust = rgb(150, 84, 60);
            // body
            let mut body = boxx(Vec3::new(cx, 0.55, cz), Vec3::new(2.1, 0.45, 0.95), pale_blue, FACE_ALL);
            body.material = Some(crate::mesh::Material::RustPlate);
            lvl.meshes.push(body);
            lvl.obstacles.push(Obstacle { center: Vec3::new(cx, 0.55, cz), half: Vec3::new(2.1, 0.55, 0.95) });
            lvl.cover_obstacles.push(Obstacle { center: Vec3::new(cx, 0.55, cz), half: Vec3::new(2.1, 0.55, 0.95) });
            // cabin (roofless, windows blown out) — just the pillars
            for &(ox, oz) in &[(-0.7f32, -0.8f32), (0.7, -0.8), (-0.7, 0.8), (0.7, 0.8)] {
                lvl.meshes.push(boxx(Vec3::new(cx + ox, 1.15, cz + oz), Vec3::new(0.08, 0.35, 0.08), dither(pale_blue, ox as i32, 0.1), FACE_ALL));
            }
            // popped hood exposing a seized black engine block
            lvl.meshes.push(boxx(Vec3::new(cx + 1.7, 0.9, cz), Vec3::new(0.45, 0.05, 0.85), dither(pale_blue, 7, 0.1), FACE_ALL));
            lvl.meshes.push(boxx(Vec3::new(cx + 1.7, 0.6, cz), Vec3::new(0.35, 0.25, 0.55), rgb(38, 36, 34), FACE_ALL));
            // flaking red-rust bumpers
            lvl.meshes.push(boxx(Vec3::new(cx - 2.15, 0.4, cz), Vec3::new(0.08, 0.12, 0.9), rust, FACE_ALL));
            lvl.meshes.push(boxx(Vec3::new(cx + 2.15, 0.4, cz), Vec3::new(0.08, 0.12, 0.9), rust, FACE_ALL));
            // four flat/rotted wheels
            for &(ox, oz) in &[(-1.5f32, -0.85f32), (1.5, -0.85), (-1.5, 0.85), (1.5, 0.85)] {
                lvl.meshes.push(tube(Vec3::new(cx + ox, 0.22, cz + oz - 0.05), Vec3::new(cx + ox, 0.22, cz + oz + 0.05), 0.22, 8, rgb(30, 28, 30)));
            }
            lvl.fire_sources.push(Vec3::new(cx, 0.6, cz)); // faint smoulder from the wreck (drives embers)
        }

        // ── the pastel colonial facades (flank, z≈28-36): a run of 2-3 story buildings with peeling stucco,
        // ornate wrought-iron railings (some ripped away), arched doorways, and vaultable window voids. ──────
        let pastels = [rgb(150, 180, 195), rgb(210, 200, 130), rgb(180, 150, 160), rgb(150, 190, 160), rgb(200, 170, 140)];
        let mut px = 3.0f32;
        let mut door_x = 0.0f32; // remember one doorway for the courtyard link
        let mut bi = 0;
        while px < w as f32 - 6.0 {
            let bw = 5.0 + hash01(bi, 0x11) * 3.0;
            let stories = 2 + (hash01(bi, 0x5) > 0.5) as i32;
            let bh = stories as f32 * 3.0;
            let bcx = (px + bw * 0.5) * ts;
            let bcz = 32.0 * ts;
            let base = pastels[bi as usize % pastels.len()];
            // building block (solid, standable roof)
            let mut bldg = boxx(Vec3::new(bcx, bh * 0.5, bcz), Vec3::new(bw * 0.5 * ts, bh * 0.5, 3.0 * ts), dither(base, bi, 0.04), FACE_ALL);
            bldg.material = Some(Material::Plaster);
            // peeling: weather the stucco toward grey primer at the base
            crate::mesh::tint_height(&mut bldg, 0.0, bh, lerp_c(base, rgb(120, 116, 110), 0.5), base);
            lvl.meshes.push(bldg);
            lvl.obstacles.push(Obstacle { center: Vec3::new(bcx, bh * 0.5, bcz), half: Vec3::new(bw * 0.5 * ts, bh * 0.5, 3.0 * ts) });
            lvl.platforms.push(Platform { x0: bcx - bw * 0.5 * ts, z0: bcz - 3.0 * ts, x1: bcx + bw * 0.5 * ts, z1: bcz + 3.0 * ts, top: bh });
            // window voids (dark recesses) on the boulevard-facing side, per story
            for s in 0..stories {
                let wy = 1.4 + s as f32 * 3.0;
                let nwin = (bw / 2.2) as i32;
                for k in 0..nwin {
                    let wx = (px + 1.0 + k as f32 * 2.2) * ts;
                    lvl.meshes.push(boxx(Vec3::new(wx, wy, bcz - 3.0 * ts + 0.1), Vec3::new(0.5, 0.7, 0.1), rgb(30, 28, 30), FACE_ALL));
                    // wrought-iron railing across the ground-floor windows (chipping black enamel), some ripped
                    if s == 0 {
                        if hash01(bi * 7 + k, 0x2) > 0.35 {
                            for b in 0..5 {
                                let rx = wx - 0.4 + b as f32 * 0.2;
                                lvl.meshes.push(tube(Vec3::new(rx, 0.8, bcz - 3.0 * ts - 0.2), Vec3::new(rx, 1.7, bcz - 3.0 * ts - 0.2), 0.02, 3, rgb(40, 38, 40)));
                            }
                        } else {
                            // ripped away: jagged stubs + a hole in the plaster
                            lvl.meshes.push(tube(Vec3::new(wx - 0.3, 0.8, bcz - 3.0 * ts - 0.15), Vec3::new(wx - 0.25, 1.1, bcz - 3.0 * ts - 0.15), 0.02, 3, rgb(40, 38, 40)));
                            lvl.meshes.push(boxx(Vec3::new(wx, 1.0, bcz - 3.0 * ts + 0.05), Vec3::new(0.35, 0.4, 0.06), rgb(70, 64, 58), FACE_ALL));
                        }
                    }
                }
            }
            // an arched doorway on the first building → the courtyard
            if bi == 1 {
                door_x = bcx;
                lvl.meshes.push(boxx(Vec3::new(bcx, 1.3, bcz - 3.0 * ts + 0.1), Vec3::new(0.7, 1.3, 0.12), rgb(26, 22, 20), FACE_ALL));
            }
            px += bw + 0.5;
            bi += 1;
        }

        // ── interior courtyard behind the doorway: crumbling terracotta, ferns, a faded lawn chair, dry
        // fountain. A quiet CQB pocket off the flank. ───────────────────────────────────────────────────────
        {
            let ccx = if door_x > 0.0 { door_x } else { 20.0 * ts };
            let ccz = 37.0 * ts;
            // terracotta floor patch
            let mut tile = boxx(Vec3::new(ccx, 0.03, ccz), Vec3::new(3.5, 0.03, 2.5), rgb(170, 100, 74), FACE_ALL);
            tile.material = Some(Material::StoneBlock);
            lvl.meshes.push(tile);
            lvl.platforms.push(Platform { x0: ccx - 3.5, z0: ccz - 2.5, x1: ccx + 3.5, z1: ccz + 2.5, top: 0.06 });
            // dry fountain (a low ring)
            for k in 0..10 {
                let a = k as f32 / 10.0 * std::f32::consts::TAU;
                lvl.meshes.push(boxx(Vec3::new(ccx + a.cos() * 0.9, 0.3, ccz + a.sin() * 0.9), Vec3::new(0.2, 0.3, 0.2), rgb(150, 140, 130), FACE_ALL));
            }
            // ferns breaking through (bright green clusters)
            for _ in 0..8 {
                let fx = ccx + (rng.f32() - 0.5) * 6.0;
                let fz = ccz + (rng.f32() - 0.5) * 4.0;
                let mut fern = crate::mesh::leaf_cluster(Vec3::new(fx, 0.35, fz), 0.4, rng.next_u64() as u32, rgb(70, 140, 60));
                fern.material = Some(Material::Moss);
                lvl.meshes.push(fern);
            }
            // faded muddy-orange plastic lawn chair
            lvl.meshes.push(boxx(Vec3::new(ccx + 2.0, 0.35, ccz + 1.2), Vec3::new(0.3, 0.03, 0.3), rgb(180, 110, 70), FACE_ALL));
            lvl.meshes.push(boxx(Vec3::new(ccx + 2.0, 0.6, ccz + 1.5), Vec3::new(0.3, 0.25, 0.03), rgb(180, 110, 70), FACE_ALL));
        }

        // ── spawns: opposite ends of the boulevard ──────────────────────────────────────────────────────────
        for (id, sx) in [(0usize, 6i32), (1, 54)].into_iter() {
            lvl.rooms.push(Room {
                rect: Rect::new(sx - 2, 18, sx + 2, 22),
                id,
                center: Vec3::new(sx as f32 * ts, 0.0, boulevard_cz),
                floor_y: 0.0,
                ceil_y: lvl.ceil_y,
            });
        }

        lvl.build_meshes();
        if let Some(f) = lvl.meshes.get_mut(0) {
            f.material = Some(crate::mesh::Material::Concrete); // sun-bleached boulevard concrete
        }
        lvl.open_air = true;

        lvl.build_cliffs();
        lvl.dissolve_boundary();
        lvl.dress_boundary_margin(&mut rng);
        lvl.chunk_meshes(40.0);
        lvl
    }

    /// **Spire Foundry** — a genuinely multi-level industrial arena. A tall foundry hall with three stacked
    /// walkable floors wrapped around a central open atrium, joined by ramps and switchback stairs, with
    /// catwalks bridging across the void and standable machine housings. You fight up, down and across: hold
    /// the high catwalks for sightlines, or push the ground floor for cover. This is the map built specifically
    /// to exercise the engine's platform/`support_y` verticality (stand on a deck, walk beneath it).
    fn build_spire_foundry(cfg: MapConfig) -> Level {
        let seed_u = cfg.seed;
        let w = 54;
        let h = 54;
        let pal = Palette {
            floor: rgb(96, 98, 104),
            wall: rgb(84, 86, 92),
            trim: rgb(120, 116, 108),
            ceil: rgb(58, 58, 64),
            cover: rgb(110, 106, 98),
            sky: rgb(70, 74, 84),
            fog: rgb(60, 62, 70),
            accent: rgb(190, 140, 70),
            rock: rgb(90, 88, 92),
        };
        let mut acfg = cfg;
        acfg.theme = MapTheme::Industrial;
        let mut lvl = Self::authored_blank(acfg, w, h, pal, 20.0);
        let ts = lvl.ts();
        let mut rng = Rng::new(seed_u ^ 0x5217E);
        let span = w as f32 * ts;
        let steel = rgb(96, 100, 108);
        let deckc = rgb(84, 86, 92);
        let grate = rgb(70, 72, 78);

        // A small helper: add a rectangular walkable deck — a visible slab mesh + a Platform (so you can stand
        // on it and walk under it) + a thin obstacle so it also blocks/【supports】 from below the lip.
        // (cx,cz) world centre, (hx,hz) world half-extents, `top` deck surface height.
        let add_deck = |lvl: &mut Level, cx: f32, cz: f32, hx: f32, hz: f32, top: f32, col: Color32| {
            let mut slab = boxx(Vec3::new(cx, top - 0.08, cz), Vec3::new(hx, 0.08, hz), dither(col, (cx * 3.0 + cz) as i32, 0.06), FACE_ALL);
            slab.material = Some(crate::mesh::Material::HexPlate);
            lvl.meshes.push(slab);
            lvl.platforms.push(Platform { x0: cx - hx, z0: cz - hz, x1: cx + hx, z1: cz + hz, top });
        };

        // A switchback stair/ramp helper connecting a lower deck to an upper deck: a sloped ramp mesh plus a
        // line of short standable steps (platforms) climbing it, so the body walks up smoothly.
        let add_ramp = |lvl: &mut Level, x0: f32, z0: f32, x1: f32, z1: f32, y0: f32, y1: f32, wdt: f32| {
            let dir = Vec3::new(x1 - x0, 0.0, z1 - z0);
            let len = (dir.x * dir.x + dir.z * dir.z).sqrt().max(0.01);
            let perp = Vec3::new(-dir.z / len, 0.0, dir.x / len) * (wdt * 0.5);
            let bl = Vec3::new(x0 - perp.x, y0, z0 - perp.z);
            let br = Vec3::new(x0 + perp.x, y0, z0 + perp.z);
            let tr = Vec3::new(x1 + perp.x, y1, z1 + perp.z);
            let tl = Vec3::new(x1 - perp.x, y1, z1 - perp.z);
            let mut r = crate::mesh::ramp(bl, br, tr, tl, dither(steel, x0 as i32, 0.05));
            r.material = Some(crate::mesh::Material::RustPlate);
            lvl.meshes.push(r);
            // standable steps along the ramp
            let steps = (len / 1.2).ceil() as i32;
            for i in 0..steps {
                let t = (i as f32 + 0.5) / steps as f32;
                let px = x0 + (x1 - x0) * t;
                let pz = z0 + (z1 - z0) * t;
                let py = y0 + (y1 - y0) * t;
                lvl.platforms.push(Platform { x0: px - wdt * 0.5, z0: pz - wdt * 0.5, x1: px + wdt * 0.5, z1: pz + wdt * 0.5, top: py });
            }
        };

        // ── carve the walkable interior (authored_blank starts fully solid); keep a 1-tile perimeter wall ──
        for x in 1..w - 1 {
            for z in 1..h - 1 {
                lvl.set(x, z, Cell::Floor);
            }
        }

        // The three floor levels (deck surface heights). Ground is the terrain floor at 0.
        let f1 = 0.0f32; // ground
        let f2 = 4.2f32; // mezzanine
        let f3 = 8.4f32; // upper gantry

        // Central atrium: an open well the upper floors wrap around (no decks in the middle so you can see and
        // shoot between levels). Everything else on f2/f3 is a walkable ring/catwalk.
        let cx = span * 0.5;
        let cz = span * 0.5;
        let atrium = 8.0f32; // half-width of the open central void

        // ── mezzanine (f2): an L-ring of decking around two sides + a bridge across ────────────────────────
        add_deck(&mut lvl, cx, cz - atrium - 4.0, span * 0.36, 3.5, f2, deckc); // north gallery
        add_deck(&mut lvl, cx, cz + atrium + 4.0, span * 0.36, 3.5, f2, deckc); // south gallery
        add_deck(&mut lvl, cx - atrium - 4.0, cz, 3.5, span * 0.22, f2, deckc); // west gallery
        add_deck(&mut lvl, cx + atrium + 4.0, cz, 3.5, span * 0.22, f2, deckc); // east gallery
        // a catwalk bridging the atrium east–west at mezzanine height
        add_deck(&mut lvl, cx, cz, atrium + 4.0, 1.4, f2, grate);

        // ── upper gantry (f3): narrower catwalks, only on two opposite sides + a crossing ──────────────────
        add_deck(&mut lvl, cx, cz - atrium - 3.0, span * 0.30, 2.0, f3, grate); // north gantry
        add_deck(&mut lvl, cx, cz + atrium + 3.0, span * 0.30, 2.0, f3, grate); // south gantry
        add_deck(&mut lvl, cx, cz, 1.4, atrium + 3.0, f3, grate); // north–south crossing catwalk

        // ── ramps/stairs linking the levels ───────────────────────────────────────────────────────────────
        // ground → mezzanine on the west, and mezzanine → gantry on the east (forces movement around the ring)
        add_ramp(&mut lvl, cx - atrium - 8.0, cz - 6.0, cx - atrium - 4.0, cz - 1.0, f1, f2, 2.2);
        add_ramp(&mut lvl, cx + atrium + 8.0, cz + 6.0, cx + atrium + 4.0, cz + 1.0, f1, f2, 2.2);
        // mezzanine → gantry (two switchbacks on the north/south galleries)
        add_ramp(&mut lvl, cx - 6.0, cz - atrium - 6.0, cx - 1.0, cz - atrium - 3.0, f2, f3, 2.0);
        add_ramp(&mut lvl, cx + 6.0, cz + atrium + 6.0, cx + 1.0, cz + atrium + 3.0, f2, f3, 2.0);

        // ── ground-floor machinery: standable housings that double as cover and stepping stones up ─────────
        for _ in 0..10 {
            let mx = 6.0 + rng.f32() * (span - 12.0);
            let mz = 6.0 + rng.f32() * (span - 12.0);
            // keep the atrium centre and ramp footprints roughly clear
            if (mx - cx).abs() < atrium && (mz - cz).abs() < atrium {
                continue;
            }
            let mh = 0.8 + rng.f32() * 0.9;
            let hw = 0.8 + rng.f32() * 0.6;
            let c = Vec3::new(mx, mh, mz);
            let half = Vec3::new(hw, mh, hw);
            let mut m = boxx(c, half, dither(steel, mx as i32, 0.08), FACE_ALL);
            m.material = Some(crate::mesh::Material::RustPlate);
            lvl.meshes.push(m);
            lvl.obstacles.push(Obstacle { center: c, half });
            lvl.cover_obstacles.push(Obstacle { center: c, half });
            // a pipe stub or vent on top for silhouette
            lvl.meshes.push(tube(Vec3::new(mx, mh * 2.0, mz), Vec3::new(mx, mh * 2.0 + 0.8, mz), 0.12, 6, rgb(120, 90, 60)));
        }

        // ── support columns rising through all levels (visual anchors + they block LOS a little) ───────────
        for &(ox, oz) in &[(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            let px = cx + ox * (atrium + 4.0);
            let pz = cz + oz * (atrium + 4.0);
            let mut col = boxx(Vec3::new(px, 10.0, pz), Vec3::new(0.5, 10.0, 0.5), dither(rgb(74, 76, 82), px as i32, 0.05), FACE_ALL);
            col.material = Some(crate::mesh::Material::Concrete);
            lvl.meshes.push(col);
            lvl.obstacles.push(Obstacle { center: Vec3::new(px, 10.0, pz), half: Vec3::new(0.5, 10.0, 0.5) });
        }

        // ── railings along the catwalk edges (read the drop, don't fully block) ────────────────────────────
        let railing = |lvl: &mut Level, x0: f32, z0: f32, x1: f32, z1: f32, y: f32| {
            let n = 10;
            for i in 0..=n {
                let t = i as f32 / n as f32;
                let px = x0 + (x1 - x0) * t;
                let pz = z0 + (z1 - z0) * t;
                lvl.meshes.push(tube(Vec3::new(px, y, pz), Vec3::new(px, y + 1.0, pz), 0.03, 3, rgb(150, 120, 60)));
            }
            // a top rail
            lvl.meshes.push(tube(Vec3::new(x0, y + 1.0, z0), Vec3::new(x1, y + 1.0, z1), 0.04, 4, rgb(160, 130, 70)));
        };
        railing(&mut lvl, cx - atrium - 3.0, cz - 1.4, cx + atrium + 3.0, cz - 1.4, f2);
        railing(&mut lvl, cx - atrium - 3.0, cz + 1.4, cx + atrium + 3.0, cz + 1.4, f2);
        railing(&mut lvl, cx - 1.4, cz - atrium - 2.0, cx - 1.4, cz + atrium + 2.0, f3);
        railing(&mut lvl, cx + 1.4, cz - atrium - 2.0, cx + 1.4, cz + atrium + 2.0, f3);

        // ── overhead: a few hanging lamp housings casting into the atrium ──────────────────────────────────
        for k in 0..4 {
            let a = k as f32 / 4.0 * std::f32::consts::TAU;
            let lx = cx + a.cos() * atrium * 0.6;
            let lz = cz + a.sin() * atrium * 0.6;
            let mut lamp = boxx(Vec3::new(lx, 13.0, lz), Vec3::new(0.4, 0.2, 0.4), rgb(40, 40, 44), FACE_ALL);
            lamp.emissive = 0.0;
            lvl.meshes.push(lamp);
            let mut bulb = disc(Vec3::new(lx, 12.78, lz), 0.3, 8, rgb(255, 226, 170));
            bulb.emissive = 0.9;
            lvl.meshes.push(bulb);
        }

        // ── spawns: opposite corners on the ground floor ──────────────────────────────────────────────────
        for (id, (sx, sz)) in [(0usize, (7i32, 7i32)), (1, (w - 8, h - 8))].into_iter() {
            // clear a little spawn pocket of floor
            for dz in -3..=3 {
                for dx in -3..=3 {
                    lvl.set(sx + dx, sz + dz, Cell::Floor);
                }
            }
            lvl.rooms.push(Room {
                rect: Rect::new(sx - 3, sz - 3, sx + 3, sz + 3),
                id,
                center: lvl.tile_center(sx, sz),
                floor_y: 0.0,
                ceil_y: lvl.ceil_y,
            });
        }

        // articulate any over-open ground sightlines with a little more cover
        lvl.scatter_exposure_cover(6, rgb(110, 106, 98));

        lvl.build_meshes();
        lvl.chunk_meshes(40.0);
        lvl
    }

    fn build_redwood_canyon(cfg: MapConfig) -> Level {
        use crate::mesh::{Material, TreeKind};
        let seed_u = cfg.seed;
        let w = 58;
        let h = 58;
        let pal = Palette {
            floor: rgb(78, 92, 58),    // mossy forest floor
            wall: rgb(120, 84, 60),    // canyon rock / bark
            trim: rgb(96, 70, 52),
            ceil: rgb(120, 140, 120),
            cover: rgb(96, 108, 76),
            sky: rgb(150, 170, 158),   // pale overcast filtered through canopy
            fog: rgb(120, 140, 124),   // green-grey forest haze
            accent: rgb(150, 90, 60),
            rock: rgb(118, 112, 104),
        };
        let mut acfg = cfg;
        acfg.theme = MapTheme::Redwood;
        let mut lvl = Self::authored_blank(acfg, w, h, pal, 22.0);
        let ts = lvl.ts();
        let mut rng = Rng::new(seed_u ^ 0x8ED0);

        let bark = rgb(122, 66, 44);
        let needle = rgb(52, 82, 46);
        let river_z0 = 24.0f32 * ts;
        let river_z1 = 34.0f32 * ts;
        let river_cz = (river_z0 + river_z1) * 0.5;
        let bank_y = 1.4f32; // banks sit above the river channel

        // ── raised canyon banks (north and south of the river) ────────────────────────────────────────────
        // Two long raised shelves flanking the river; players fight along and across them. The river bed
        // between them sits a step lower (fordable — you can wade the shallow water).
        for (bz0, bz1) in [(3.0f32, 23.0f32), (35.0, 55.0)] {
            let cz = (bz0 + bz1) * 0.5 * ts;
            let hz = (bz1 - bz0) * 0.5 * ts;
            let cx = w as f32 * 0.5 * ts;
            let hx = (w as f32 * 0.5 - 3.0) * ts;
            let mut shelf = boxx(Vec3::new(cx, bank_y * 0.5, cz), Vec3::new(hx, bank_y * 0.5, hz), pal.floor, FACE_ALL);
            shelf.material = Some(Material::Grass);
            lvl.meshes.push(shelf);
            lvl.obstacles.push(Obstacle { center: Vec3::new(cx, bank_y * 0.5, cz), half: Vec3::new(hx, bank_y * 0.5, hz) });
            lvl.platforms.push(Platform { x0: cx - hx, z0: cz - hz, x1: cx + hx, z1: cz + hz, top: bank_y });
        }

        // ── the river: a shallow, wadeable channel formed by a chain of overlapping water discs down the
        // middle band (a single square body would flood the whole map, so we lay the river out as a ribbon). ─
        let channel_hw = (river_z1 - river_z0) * 0.5;
        let nseg = 10;
        for i in 0..nseg {
            let t = (i as f32 + 0.5) / nseg as f32;
            let cx = 2.0 * ts + t * (w as f32 - 4.0) * ts;
            lvl.water.push(WaterBody {
                cx,
                cz: river_cz,
                radius: channel_hw + 0.5,
                y: 0.35,
                col: rgb(70, 110, 118),
                alpha: 150,
                reflect: 0.5,
                emissive: 0.0,
                lava: false,
                disc: true,
            });
        }

        // ── fallen-log bridges across the river (the crossing chokes) ─────────────────────────────────────
        for &bx in &[16.0f32, 30.0, 44.0] {
            let px = bx * ts;
            let a = Vec3::new(px, bank_y + 0.3, river_z0 - 1.0);
            let b = Vec3::new(px, bank_y + 0.3, river_z1 + 1.0);
            // the log itself (a fat tube spanning the banks)
            let mut logm = tube(a, b, 0.55, 8, dither(bark, bx as i32, 0.08));
            logm.material = Some(Material::Bark);
            lvl.meshes.push(logm);
            // walkable collision along the log + platform
            let steps = 8;
            for i in 0..steps {
                let t = (i as f32 + 0.5) / steps as f32;
                let z = a.z + (b.z - a.z) * t;
                lvl.obstacles.push(Obstacle { center: Vec3::new(px, bank_y + 0.1, z), half: Vec3::new(0.55, 0.35, (b.z - a.z).abs() / steps as f32 * 0.5 + 0.1) });
                lvl.platforms.push(Platform { x0: px - 0.5, z0: z - 0.7, x1: px + 0.5, z1: z + 0.7, top: bank_y + 0.5 });
            }
            // stripped branch stubs on the log
            for _ in 0..3 {
                let t = 0.2 + rng.f32() * 0.6;
                let z = a.z + (b.z - a.z) * t;
                let dir = if rng.f32() < 0.5 { 1.0 } else { -1.0 };
                lvl.meshes.push(tube(Vec3::new(px, bank_y + 0.6, z), Vec3::new(px + dir * 0.8, bank_y + 1.1, z), 0.08, 4, bark));
            }
        }

        // ── giant redwoods on the banks (standing, immense) ───────────────────────────────────────────────
        let mut tree_spots: Vec<(f32, f32)> = Vec::new();
        for _ in 0..16 {
            let tx = 4.0 + rng.f32() * 50.0;
            let tz = {
                // keep trees off the river channel — put them on the banks
                let onnorth = rng.f32() < 0.5;
                if onnorth { 4.0 + rng.f32() * 17.0 } else { 36.0 + rng.f32() * 18.0 }
            };
            // spacing check
            if tree_spots.iter().any(|&(ox, oz)| (ox - tx).powi(2) + (oz - tz).powi(2) < 25.0) {
                continue;
            }
            tree_spots.push((tx, tz));
            let px = tx * ts;
            let pz = tz * ts;
            let len = 12.0 + rng.f32() * 8.0; // towering
            let rad = 0.7 + rng.f32() * 0.4;
            let mut tm = crate::mesh::tree_styled(TreeKind::Conifer, Vec3::new(px, bank_y, pz), len, rad, 3, rng.next_u64() as u32, dither(bark, tx as i32, 0.06), dither(needle, tz as i32, 0.08));
            // give the trunk pieces (the brown, non-foliage meshes) the bark material for furrowed relief
            for tmesh in tm.iter_mut() {
                let c = tmesh.color;
                // trunk pieces are brown (red > green); foliage is green — only bark the trunk
                if c.r() as i32 > c.g() as i32 + 10 {
                    tmesh.material = Some(Material::Bark);
                }
            }
            lvl.meshes.extend(tm);
            // the trunk is a hard obstacle (cover to hug)
            lvl.obstacles.push(Obstacle { center: Vec3::new(px, bank_y + len * 0.4, pz), half: Vec3::new(rad, len * 0.4, rad) });
            lvl.cover_obstacles.push(Obstacle { center: Vec3::new(px, bank_y + 1.0, pz), half: Vec3::new(rad, 1.0, rad) });
        }

        // ── thickets: dense stands of tall shrubs/saplings that occlude at eye height, placed to chop the
        // long open banks into mid-range (9–26m) engagement pockets. Sparse giant redwoods leave 90%+ of the
        // ground as long sightlines; these eye-height thickets are what actually break the shooting-gallery
        // lanes (they register as cover and block a standing sightline, unlike the low ground ferns). ───────
        let thicket_leaf = rgb(58, 88, 48);
        for k in 0..10 {
            let onnorth = k % 2 == 0;
            // stagger thickets down the length of each bank at its open centre-line, so they chop the long
            // lane a player runs along the bank — rather than clustering near the trees where cover already is.
            let tx = 8.0 + (k as f32 / 10.0) * 42.0 + (rng.f32() - 0.5) * 4.0;
            let tz = if onnorth { 11.0 + (rng.f32() - 0.5) * 6.0 } else { 46.0 + (rng.f32() - 0.5) * 6.0 };
            // don't stack on a tree trunk
            if tree_spots.iter().any(|&(ox, oz)| (ox - tx).powi(2) + (oz - tz).powi(2) < 9.0) {
                continue;
            }
            let px = tx * ts;
            let pz = tz * ts;
            // a clump of 3–5 tall bushy masses reaching ~2m (eye height), on trunk-like thin stems
            let n = 3 + (rng.f32() * 3.0) as i32;
            for _ in 0..n {
                let ox = (rng.f32() - 0.5) * 1.6;
                let oz = (rng.f32() - 0.5) * 1.6;
                let hgt = 1.7 + rng.f32() * 0.6;
                let base = Vec3::new(px + ox, bank_y, pz + oz);
                // thin woody stem
                lvl.meshes.push(tube(base, base + Vec3::new(0.0, hgt * 0.6, 0.0), 0.06, 4, dither(bark, px as i32, 0.1)));
                // bushy crown
                let mut crown = crate::mesh::leaf_cluster(base + Vec3::new(0.0, hgt, 0.0), 0.55 + rng.f32() * 0.3, rng.next_u64() as u32, dither(thicket_leaf, tz as i32, 0.1));
                crown.material = Some(Material::Moss);
                lvl.meshes.push(crown);
            }
            // one eye-height cover/collision volume for the whole thicket
            lvl.obstacles.push(Obstacle { center: Vec3::new(px, bank_y + 1.1, pz), half: Vec3::new(1.3, 1.1, 1.3) });
            lvl.cover_obstacles.push(Obstacle { center: Vec3::new(px, bank_y + 1.1, pz), half: Vec3::new(1.3, 1.1, 1.3) });
        }

        // ── toppled redwoods: massive fallen trunks that double as walkable ramps/cover on the banks ───────
        for _ in 0..4 {
            let onnorth = rng.f32() < 0.5;
            let cx = (10.0 + rng.f32() * 38.0) * ts;
            let cz = if onnorth { (8.0 + rng.f32() * 12.0) * ts } else { (38.0 + rng.f32() * 12.0) * ts };
            let along_x = rng.f32() < 0.5;
            let ln = 6.0 + rng.f32() * 4.0;
            let (a, b) = if along_x {
                (Vec3::new(cx - ln, bank_y + 0.7, cz), Vec3::new(cx + ln, bank_y + 0.7, cz))
            } else {
                (Vec3::new(cx, bank_y + 0.7, cz - ln), Vec3::new(cx, bank_y + 0.7, cz + ln))
            };
            let mut fallen = tube(a, b, 0.7, 8, dither(bark, cx as i32, 0.08));
            fallen.material = Some(Material::Bark);
            lvl.meshes.push(fallen);
            // root ball at one end
            lvl.meshes.push(crate::mesh::uv_sphere(a, 1.1, 7, 5, dither(rgb(90, 68, 50), cx as i32, 0.1)));
            let half = if along_x { Vec3::new(ln, 0.7, 0.7) } else { Vec3::new(0.7, 0.7, ln) };
            lvl.obstacles.push(Obstacle { center: Vec3::new((a.x + b.x) * 0.5, bank_y + 0.7, (a.z + b.z) * 0.5), half });
            lvl.cover_obstacles.push(Obstacle { center: Vec3::new((a.x + b.x) * 0.5, bank_y + 0.7, (a.z + b.z) * 0.5), half });
        }

        // ── boulder clusters for hard cover along the banks ───────────────────────────────────────────────
        for _ in 0..10 {
            let onnorth = rng.f32() < 0.5;
            let px = (5.0 + rng.f32() * 48.0) * ts;
            let pz = if onnorth { (5.0 + rng.f32() * 16.0) * ts } else { (37.0 + rng.f32() * 16.0) * ts };
            let r = 0.8 + rng.f32() * 0.9;
            let mut rk = crate::mesh::rock(Vec3::new(px, bank_y + r * 0.3, pz), r, rng.next_u64() as u32, dither(pal.rock, px as i32, 0.08));
            rk.material = Some(Material::StoneBlock);
            lvl.meshes.push(rk);
            lvl.obstacles.push(Obstacle { center: Vec3::new(px, bank_y + r * 0.5, pz), half: Vec3::new(r, r * 0.6, r) });
            lvl.cover_obstacles.push(Obstacle { center: Vec3::new(px, bank_y + r * 0.5, pz), half: Vec3::new(r, r * 0.6, r) });
        }

        // ── forest-floor undergrowth: ferns, mossy stones, low bushes and fallen branches scattered across
        // the banks. A bare mossy plane reads flat; this fine, varied ground clutter is what gives a forest
        // its lived-in texture (and what the scenic scorer rewards) without blocking movement. ─────────────
        let fern_green = rgb(64, 96, 52);
        let bush_green = rgb(56, 82, 46);
        for _ in 0..46 {
            let onnorth = rng.f32() < 0.5;
            let px = (4.0 + rng.f32() * 50.0) * ts;
            let pz = if onnorth { (4.0 + rng.f32() * 18.0) * ts } else { (36.0 + rng.f32() * 18.0) * ts };
            let kind = rng.f32();
            if kind < 0.4 {
                // fern: a low splay of thin fronds fanning up and out
                let n = 5 + (rng.f32() * 4.0) as i32;
                for _ in 0..n {
                    let a = rng.f32() * std::f32::consts::TAU;
                    let lean = 0.25 + rng.f32() * 0.35;
                    let base = Vec3::new(px + (rng.f32() - 0.5) * 0.3, bank_y, pz + (rng.f32() - 0.5) * 0.3);
                    let tip = base + Vec3::new(a.cos() * lean, 0.4 + rng.f32() * 0.35, a.sin() * lean);
                    let mut frond = tube(base, tip, 0.03, 3, dither(fern_green, base.x as i32, 0.1));
                    frond.material = Some(Material::Moss);
                    lvl.meshes.push(frond);
                }
            } else if kind < 0.66 {
                // low bush: a small mossy leaf cluster
                let r = 0.4 + rng.f32() * 0.4;
                let mut bush = crate::mesh::leaf_cluster(Vec3::new(px, bank_y + r * 0.6, pz), r, rng.next_u64() as u32, dither(bush_green, px as i32, 0.1));
                bush.material = Some(Material::Moss);
                lvl.meshes.push(bush);
            } else if kind < 0.85 {
                // small moss-covered stone
                let r = 0.25 + rng.f32() * 0.35;
                let mut rk = crate::mesh::rock(Vec3::new(px, bank_y + r * 0.2, pz), r, rng.next_u64() as u32, dither(lerp_c(pal.rock, fern_green, 0.3), px as i32, 0.1));
                rk.material = Some(Material::Moss);
                lvl.meshes.push(rk);
            } else {
                // fallen branch / twig on the ground
                let a = rng.f32() * std::f32::consts::TAU;
                let ln = 0.6 + rng.f32() * 0.8;
                let base = Vec3::new(px, bank_y + 0.08, pz);
                let end = base + Vec3::new(a.cos() * ln, 0.0, a.sin() * ln);
                let mut br = tube(base, end, 0.06, 4, dither(bark, px as i32, 0.1));
                br.material = Some(Material::Bark);
                lvl.meshes.push(br);
            }
        }

        // ── ranger cabin: a small timber structure on the north bank (a landmark + interior cover) ─────────
        {
            let cx = 46.0 * ts;
            let cz = 12.0 * ts;
            let (hx, hz) = (2.6f32, 2.2f32);
            // log walls (four runs of stacked-log look via a plank material box)
            for (wx, wz, whx, whz) in [
                (cx, cz - hz, hx, 0.2f32),
                (cx, cz + hz, hx, 0.2),
                (cx - hx, cz, 0.2, hz),
                (cx + hx, cz, 0.2, hz),
            ] {
                let mut wall = boxx(Vec3::new(wx, bank_y + 1.3, wz), Vec3::new(whx, 1.3, whz), dither(rgb(120, 86, 54), wx as i32, 0.06), FACE_ALL);
                wall.material = Some(Material::Planks);
                lvl.meshes.push(wall);
                lvl.obstacles.push(Obstacle { center: Vec3::new(wx, bank_y + 1.3, wz), half: Vec3::new(whx, 1.3, whz) });
            }
            // pitched plank roof (two slabs)
            for side in [-1.0f32, 1.0] {
                let mut r = boxx(Vec3::new(cx + side * hx * 0.5, bank_y + 2.9, cz), Vec3::new(hx * 0.6, 0.12, hz + 0.3), dither(rgb(96, 68, 46), side as i32, 0.05), FACE_ALL);
                crate::mesh::rotate_z_about(&mut r, Vec3::new(cx, bank_y + 2.9, cz), side * 0.4);
                r.material = Some(Material::Planks);
                lvl.meshes.push(r);
            }
            lvl.platforms.push(Platform { x0: cx - hx, z0: cz - hz, x1: cx + hx, z1: cz + hz, top: bank_y });
        }

        // ── spawns: opposite banks, opposite ends ─────────────────────────────────────────────────────────
        for (id, (sx, sz)) in [(0usize, (8i32, 12i32)), (1, (48, 46))].into_iter() {
            lvl.rooms.push(Room {
                rect: Rect::new(sx - 2, sz - 2, sx + 2, sz + 2),
                id,
                center: Vec3::new(sx as f32 * ts, bank_y, sz as f32 * ts),
                floor_y: bank_y,
                ceil_y: lvl.ceil_y,
            });
        }

        lvl.build_meshes();
        // forest floor reads as mossy ground
        if let Some(f) = lvl.meshes.get_mut(0) {
            f.material = Some(Material::Moss);
        }
        lvl.open_air = true; // open forest gorge under sky

        lvl.build_cliffs();
        lvl.dissolve_boundary();
        lvl.dress_boundary_margin(&mut rng);
        lvl.chunk_meshes(40.0);
        lvl
    }

    fn build_pyre_garage(cfg: MapConfig) -> Level {
        use crate::destructible::{Destructible, DestructibleKind};
        use crate::mesh::Material;
        let seed_u = cfg.seed;
        let w = 54;
        let h = 54;
        let pal = Palette {
            floor: rgb(96, 92, 92),   // soot-grey deck
            wall: rgb(104, 100, 100), // stained concrete
            trim: rgb(72, 68, 66),
            ceil: rgb(30, 26, 26),
            cover: rgb(90, 86, 86),
            sky: rgb(58, 40, 34),     // choking smoke-lit sky
            fog: rgb(46, 34, 30),     // thick black soot haze
            accent: rgb(255, 120, 40),
            rock: rgb(80, 74, 70),
        };
        let mut acfg = cfg;
        acfg.theme = MapTheme::Industrial;
        let mut lvl = Self::authored_blank(acfg, w, h, pal, 7.0);
        let ts = lvl.ts();
        let mut rng = Rng::new(seed_u ^ 0xF14E);

        // Low ceiling: a parking garage is a claustrophobic slab overhead (not open sky) — but we keep the
        // ceiling mesh empty per the engine's no-lid rule and imply it with the smoke fog + a dark palette.
        // Two decks: the ground deck at y=0, an upper deck at deck_y reached by two ramps.
        let deck_y = 4.0f32;

        // ── upper-deck slab: a raised standable platform over the north half of the map ────────────────────
        // Built as a thick concrete box the player stands on; the south half stays ground level.
        let upper_x0 = 6.0 * ts;
        let upper_x1 = (w as f32 - 6.0) * ts;
        let upper_z0 = 6.0 * ts;
        let upper_z1 = 26.0 * ts;
        let slab_col = rgb(90, 86, 86);
        {
            let cx = (upper_x0 + upper_x1) * 0.5;
            let cz = (upper_z0 + upper_z1) * 0.5;
            let hx = (upper_x1 - upper_x0) * 0.5;
            let hz = (upper_z1 - upper_z0) * 0.5;
            let mut slab = boxx(Vec3::new(cx, deck_y, cz), Vec3::new(hx, 0.3, hz), slab_col, FACE_ALL);
            slab.material = Some(Material::ScorchedConcrete);
            lvl.meshes.push(slab);
            lvl.obstacles.push(Obstacle { center: Vec3::new(cx, deck_y, cz), half: Vec3::new(hx, 0.3, hz) });
            lvl.platforms.push(Platform { x0: upper_x0, z0: upper_z0, x1: upper_x1, z1: upper_z1, top: deck_y + 0.3 });
            // support pillars under the slab (structural + cover on the ground deck below). Each carries a
            // painted parking-level hazard band (yellow/red) at a jittered height and a little colour/soot
            // variation, so the pillar grid reads as a worn garage rather than a repeated identical prefab —
            // this is what the scenic scorer flags when a map is too regular.
            let bands = [rgb(196, 160, 40), rgb(180, 60, 40), rgb(70, 110, 150)]; // faded yellow / red / blue
            for (gzi, gz) in [9, 15, 21].into_iter().enumerate() {
                for (gxi, gx) in [10, 20, 34, 44].into_iter().enumerate() {
                    let px = gx as f32 * ts;
                    let pz = gz as f32 * ts;
                    let idx = gzi * 4 + gxi;
                    let base = dither(lerp_c(rgb(84, 80, 80), rgb(70, 62, 58), hash01(idx as i32, 0x3) * 0.5), idx as i32, 0.06);
                    let mut pil = boxx(Vec3::new(px, deck_y * 0.5, pz), Vec3::new(0.5, deck_y * 0.5, 0.5), base, FACE_ALL);
                    pil.material = Some(crate::mesh::Material::Concrete);
                    // vertical soot/water staining down the pillar
                    crate::mesh::tint_height(&mut pil, 0.0, deck_y, lerp_c(base, rgb(40, 36, 34), 0.5), base);
                    lvl.meshes.push(pil);
                    lvl.obstacles.push(Obstacle { center: Vec3::new(px, deck_y * 0.5, pz), half: Vec3::new(0.5, deck_y * 0.5, 0.5) });
                    // painted hazard band at a per-pillar height
                    let band_y = 0.9 + hash01(idx as i32, 0x9) * 0.6;
                    let band_col = bands[idx % bands.len()];
                    let mut band = boxx(Vec3::new(px, band_y, pz), Vec3::new(0.52, 0.18, 0.52), band_col, FACE_ALL);
                    band.emissive = 0.06;
                    lvl.meshes.push(band);
                    // a stenciled level number plate (dark) above the band on the aisle-facing side
                    lvl.meshes.push(boxx(Vec3::new(px, band_y + 0.5, pz - 0.53), Vec3::new(0.22, 0.22, 0.02), rgb(30, 30, 34), FACE_ALL));
                }
            }
        }

        // ── two spiralling ramps up to the deck (east & west), switchback-flavoured ───────────────────────
        // Each ramp is a sloped box the player walks up; a landing at the top meets the slab.
        let ramp_col = rgb(88, 84, 84);
        for (side, rx) in [(-1i32, 12.0f32), (1, (w as f32 - 12.0))] {
            let rz0 = 30.0 * ts;
            let rz1 = 26.0 * ts;
            let rxw = rx * ts;
            // sloped ramp surface via the `ramp` primitive (bl, br, tr, tl)
            let hw = 2.4;
            let bl = Vec3::new(rxw - hw, 0.0, rz0);
            let br = Vec3::new(rxw + hw, 0.0, rz0);
            let tr = Vec3::new(rxw + hw, deck_y + 0.3, rz1);
            let tl = Vec3::new(rxw - hw, deck_y + 0.3, rz1);
            let mut r = ramp(bl, br, tr, tl, ramp_col);
            r.material = Some(Material::ScorchedConcrete);
            lvl.meshes.push(r);
            // collision: a chain of small boxes following the slope so the body walks up smoothly
            let steps = 10;
            for i in 0..steps {
                let t = (i as f32 + 0.5) / steps as f32;
                let zc = rz0 + (rz1 - rz0) * t;
                let yc = (deck_y + 0.3) * t;
                lvl.obstacles.push(Obstacle { center: Vec3::new(rxw, yc * 0.5, zc), half: Vec3::new(hw, yc * 0.5 + 0.15, (rz0 - rz1).abs() / steps as f32 * 0.5 + 0.1) });
                lvl.platforms.push(Platform { x0: rxw - hw, z0: zc - 0.6, x1: rxw + hw, z1: zc + 0.6, top: yc + 0.06 });
            }
            // low guard wall along the outer edge of the ramp
            let wall_x = rxw + side as f32 * (hw + 0.2);
            let mut gw = boxx(Vec3::new(wall_x, deck_y * 0.4, (rz0 + rz1) * 0.5), Vec3::new(0.16, deck_y * 0.4, (rz0 - rz1).abs() * 0.5), rgb(78, 74, 72), FACE_ALL);
            gw.material = Some(crate::mesh::Material::Concrete);
            lvl.meshes.push(gw);
        }

        // ── burning-car cover: scattered cars, several on fire (fire sources) ─────────────────────────────
        // Cars are destructible explosive cover. Fire ones register as fire_sources so the game emits a
        // continuous plume + flickering light. Placed on both decks and blocking the switchback chokes.
        let car_spots: [(f32, f32, f32, f32, bool); 9] = [
            // (tile_x, tile_z, base_y, yaw, on_fire)
            (16.0, 34.0, 0.0, 0.2, true),   // ground: broadside across a lane, engulfed
            (30.0, 38.0, 0.0, 1.4, false),
            (40.0, 44.0, 0.0, 0.0, true),
            (22.0, 46.0, 0.0, 2.6, false),
            (14.0, 12.0, deck_y + 0.3, 0.6, true),  // upper deck
            (28.0, 16.0, deck_y + 0.3, 1.9, false),
            (40.0, 10.0, deck_y + 0.3, 0.3, true),
            (34.0, 22.0, deck_y + 0.3, 2.2, false),
            (20.0, 20.0, deck_y + 0.3, 1.1, false),
        ];
        let car_cols = [rgb(150, 40, 36), rgb(40, 60, 120), rgb(180, 170, 160), rgb(60, 70, 74), rgb(120, 110, 40)];
        for (i, &(tx, tz, by, yaw, fire)) in car_spots.iter().enumerate() {
            let cx = tx * ts;
            let cz = tz * ts;
            let col = if fire { rgb(46, 40, 38) } else { car_cols[i % car_cols.len()] };
            let (cm, co) = burning_car(cx, cz, by, yaw, col, fire, rng.next_u64() as u32);
            lvl.meshes.extend(cm);
            lvl.obstacles.extend(co);
            // an explosive destructible core so shooting a car blasts it (and chains neighbours)
            lvl.destructibles.push(Destructible::new(
                DestructibleKind::ExplosiveBarrel,
                Vec3::new(cx, by + 0.6, cz),
                Vec3::new(1.5, 0.6, 0.9),
                rng.next_u64() as u32,
            ));
            if fire {
                lvl.fire_sources.push(Vec3::new(cx, by + 0.7, cz));
            }
        }

        // ── the collapsed slab: a breach between the decks (a rubble ramp you can climb the levels through) ─
        {
            let bx = 27.0 * ts;
            let bz = 27.0 * ts; // right at the deck seam
            // a chunk of fallen ceiling slab crushing a truck, angled as a climbable rubble ramp
            let mut chunk = ramp(
                Vec3::new(bx - 3.0, 0.0, bz + 2.5),
                Vec3::new(bx + 3.0, 0.0, bz + 2.5),
                Vec3::new(bx + 3.0, deck_y + 0.3, bz - 1.0),
                Vec3::new(bx - 3.0, deck_y + 0.3, bz - 1.0),
                rgb(86, 82, 82),
            );
            chunk.material = Some(crate::mesh::Material::Concrete);
            lvl.meshes.push(chunk);
            // rebar jutting out of the slab
            for k in 0..6 {
                let ox = (k as f32 - 3.0) * 0.7;
                let base = Vec3::new(bx + ox, deck_y * 0.5, bz + 0.5);
                let tip = base + Vec3::new((rng.f32() - 0.5) * 0.4, 0.9, (rng.f32() - 0.5) * 0.4);
                lvl.meshes.push(tube(base, tip, 0.03, 3, rgb(70, 50, 40)));
            }
            // collision ramp for the breach
            let steps = 8;
            for i in 0..steps {
                let t = (i as f32 + 0.5) / steps as f32;
                let zc = (bz + 2.5) + ((bz - 1.0) - (bz + 2.5)) * t;
                let yc = (deck_y + 0.3) * t;
                lvl.obstacles.push(Obstacle { center: Vec3::new(bx, yc * 0.5, zc), half: Vec3::new(3.0, yc * 0.5 + 0.2, 0.5) });
                lvl.platforms.push(Platform { x0: bx - 3.0, z0: zc - 0.6, x1: bx + 3.0, z1: zc + 0.6, top: yc + 0.06 });
            }
            // the crushed truck cab under the slab (flattened box)
            let mut cab = boxx(Vec3::new(bx, 0.3, bz + 3.0), Vec3::new(1.6, 0.3, 1.0), rgb(60, 66, 70), FACE_ALL);
            cab.material = Some(Material::CarPaint);
            lvl.meshes.push(cab);
            lvl.obstacles.push(Obstacle { center: Vec3::new(bx, 0.3, bz + 3.0), half: Vec3::new(1.6, 0.3, 1.0) });
            // safety-glass blanket (a scatter of tiny glittering cubes on the floor)
            let mut glass = Mesh3::new(rgb(150, 175, 185));
            for _ in 0..40 {
                let gx = bx + (rng.f32() - 0.5) * 6.0;
                let gz = bz + 3.0 + (rng.f32() - 0.5) * 4.0;
                glass.merge(&boxx(Vec3::new(gx, 0.03, gz), Vec3::new(0.04, 0.03, 0.04), rgb(150, 175, 185), FACE_ALL));
            }
            set_alpha(&mut glass, 180);
            lvl.meshes.push(glass);
        }

        // ── rooftop helipad implied by a raised pad on the upper deck's north edge, with crashed chopper ──
        {
            let hx = 27.0 * ts;
            let hz = 9.0 * ts;
            // faded yellow H painted on the deck
            let mut pad = disc(Vec3::new(hx, deck_y + 0.32, hz), 3.2, 20, rgb(120, 110, 60));
            pad.material = Some(Material::ScorchedConcrete);
            lvl.meshes.push(pad);
            // crashed helicopter: a blackened fuselage box on its side + sheared rotor
            let mut fus = boxx(Vec3::new(hx + 1.5, deck_y + 0.9, hz), Vec3::new(1.8, 0.6, 0.8), rgb(40, 36, 34), FACE_ALL);
            fus.material = Some(crate::mesh::Material::RustPlate);
            lvl.meshes.push(fus);
            lvl.obstacles.push(Obstacle { center: Vec3::new(hx + 1.5, deck_y + 0.9, hz), half: Vec3::new(1.8, 0.6, 0.8) });
            // tail boom
            lvl.meshes.push(boxx(Vec3::new(hx + 4.0, deck_y + 0.8, hz), Vec3::new(0.9, 0.2, 0.2), rgb(38, 34, 32), FACE_ALL));
            // sheared rotor blades on the deck
            for a in [0.4f32, 2.1] {
                let (s, c) = a.sin_cos();
                lvl.meshes.push(boxx(Vec3::new(hx + c * 2.0, deck_y + 0.35, hz + s * 2.0), Vec3::new(2.2, 0.04, 0.12), rgb(50, 48, 46), FACE_ALL));
            }
            lvl.fire_sources.push(Vec3::new(hx + 1.5, deck_y + 1.0, hz)); // the wreck burns
            // ruptured fuel truck nearby pouring flame
            let fxx = 20.0 * ts;
            let fzz = 8.0 * ts;
            let mut tank = boxx(Vec3::new(fxx, deck_y + 0.9, fzz), Vec3::new(2.2, 0.7, 0.9), rgb(120, 40, 36), FACE_ALL);
            tank.material = Some(Material::CarPaint);
            lvl.meshes.push(tank);
            lvl.obstacles.push(Obstacle { center: Vec3::new(fxx, deck_y + 0.9, fzz), half: Vec3::new(2.2, 0.7, 0.9) });
            lvl.fire_sources.push(Vec3::new(fxx + 2.4, deck_y + 0.4, fzz));
        }

        // ── ground-deck sightline breakers: low concrete stub walls + stacked wreckage ────────────────────
        // The open bottom deck plays as one long-range field without eye-height structure. These stub walls
        // and debris piles (tall enough to occlude) carve it into fightable mid-range pockets, with gaps so
        // no route is sealed.
        for &(tx, tz, along_x, len) in &[
            (10i32, 40i32, true, 7i32),
            (30, 42, true, 8),
            (12, 48, false, 4),
            (38, 40, true, 6),
            (24, 36, false, 5),
            (44, 46, false, 4),
        ] {
            for i in 0..len {
                if i % 4 == 3 {
                    continue; // gap so nothing is walled off
                }
                let (cxt, czt) = if along_x { (tx + i, tz) } else { (tx, tz + i) };
                let wx = cxt as f32 * ts;
                let wz = czt as f32 * ts;
                let (hw, hd) = if along_x { (ts * 0.5, 0.28) } else { (0.28, ts * 0.5) };
                let c = Vec3::new(wx, 1.0, wz);
                let mut m = boxx(c, Vec3::new(hw, 1.0, hd), rgb(84, 80, 80), FACE_ALL);
                m.material = Some(crate::mesh::Material::Concrete);
                lvl.meshes.push(m);
                lvl.obstacles.push(Obstacle { center: c, half: Vec3::new(hw, 1.0, hd) });
            }
        }
        // a couple of stacked-wreckage piles (crushed cars) as tall hard cover / mini high-ground
        for &(tx, tz) in &[(18i32, 44i32), (36, 46)] {
            let wx = tx as f32 * ts;
            let wz = tz as f32 * ts;
            for (k, (dy, dw)) in [(0.0f32, 1.4f32), (0.9, 1.1), (1.7, 0.8)].iter().enumerate() {
                let jitter = (k as f32 * 0.3).sin() * 0.3;
                let mut m = boxx(Vec3::new(wx + jitter, *dy + 0.45, wz), Vec3::new(*dw, 0.45, *dw * 0.7), scale_rgb(rgb(60, 56, 54), 0.9 - k as f32 * 0.08), FACE_ALL);
                m.material = Some(crate::mesh::Material::RustPlate);
                lvl.meshes.push(m);
                lvl.obstacles.push(Obstacle { center: Vec3::new(wx + jitter, *dy + 0.45, wz), half: Vec3::new(*dw, 0.45, *dw * 0.7) });
            }
            lvl.platforms.push(Platform { x0: wx - 0.8, z0: wz - 0.6, x1: wx + 0.8, z1: wz + 0.6, top: 2.15 });
        }

        // ── spawns: opposite ground-deck corners ──────────────────────────────────────────────────────────
        for (id, (sx, sz)) in [(0usize, (8i32, 48i32)), (1, (46, 48))].into_iter() {
            lvl.rooms.push(Room {
                rect: Rect::new(sx - 2, sz - 2, sx + 2, sz + 2),
                id,
                center: Vec3::new(sx as f32 * ts, 0.0, sz as f32 * ts),
                floor_y: 0.0,
                ceil_y: lvl.ceil_y,
            });
        }

        // ── emergency lighting & signage: green EXIT signs, red hazard strobes, and sodium ceiling lamps.
        // A smoke-choked garage is atmospheric but reads as a monotone brown murk; these saturated emissive
        // fixtures punch colour and depth through the haze (and double as wayfinding), lifting the scene from
        // "bland" without losing the mood. Each is a small self-lit panel; the strobes also seed a coloured
        // point light so their glow spills onto nearby concrete. ─────────────────────────────────────────
        {
            let exit_green = rgb(60, 210, 90);
            let hazard_red = rgb(220, 50, 40);
            let sodium = rgb(240, 200, 120);
            // EXIT signs over the ramp mouths and along the deck edges
            for &(sx, sz) in &[(12.0f32, 30.0f32), (w as f32 - 12.0, 30.0), (28.0, 6.0), (28.0, h as f32 - 6.0)] {
                let p = Vec3::new(sx * ts, deck_y + 2.2, sz * ts);
                let mut sign = boxx(p, Vec3::new(0.7, 0.28, 0.08), exit_green, FACE_ALL);
                sign.emissive = 0.95;
                lvl.meshes.push(sign);
            }
            // red hazard strobes near the burning wrecks (on the deck) — bright self-lit markers
            for &(hx, hz) in &[(38.0f32, 12.0f32), (20.0, 8.0), (44.0, 40.0)] {
                let p = Vec3::new(hx * ts, deck_y + 2.6, hz * ts);
                let mut strobe = boxx(p, Vec3::new(0.18, 0.18, 0.18), hazard_red, FACE_ALL);
                strobe.emissive = 1.0;
                lvl.meshes.push(strobe);
                // a small housing so the strobe reads as a mounted fixture
                lvl.meshes.push(boxx(p + Vec3::new(0.0, 0.2, 0.0), Vec3::new(0.24, 0.06, 0.24), rgb(40, 40, 44), FACE_ALL));
            }
            // sodium ceiling lamps under the upper slab — warm self-lit panels across the ground deck
            for gz in [10, 22] {
                for gx in [12, 24, 36] {
                    let p = Vec3::new(gx as f32 * ts, deck_y - 0.3, gz as f32 * ts);
                    let mut lamp = boxx(p, Vec3::new(0.4, 0.1, 0.4), sodium, FACE_ALL);
                    lamp.emissive = 0.8;
                    lvl.meshes.push(lamp);
                }
            }
        }

        // snapshot everything so far as cover for the scorer
        lvl.cover_obstacles = lvl.obstacles.clone();

        lvl.build_meshes();
        // deck skin: the ground floor reads as scorched, oil-stained concrete
        if let Some(f) = lvl.meshes.get_mut(0) {
            f.material = Some(Material::ScorchedConcrete);
        }
        lvl.open_air = false; // enclosed structure, smoke-choked

        // the structure's outer edge — cliff/skybox boundary treatment
        lvl.build_cliffs();
        lvl.dissolve_boundary();
        lvl.dress_boundary_margin(&mut rng);
        lvl.chunk_meshes(40.0);
        lvl
    }

    /// Seed a blank authored level: solid fill, a wall border ring, an open Floor interior, no procedural
    /// cover. Returns it ready for hand placement.
    fn authored_blank(cfg: MapConfig, w: i32, h: i32, pal: Palette, ceil_y: f32) -> Level {
        let mut lvl = Level {
            pal,
            w,
            h,
            cells: vec![Cell::Solid; (w * h) as usize],
            heights: vec![0.0; (w * h) as usize],
            rooms: Vec::new(),
            obstacles: Vec::new(),
            platforms: Vec::new(),
            meshes: Vec::new(),
            floor_y: 0.0,
            ceil_y,
            hazard_y: 0.0,
            hazard_lava: false,
            has_hazard: false,
            hazard_dps: 0.0,
            trail: Vec::new(),
            stream: Vec::new(),
            ponds: Vec::new(),
            cannons: Vec::new(),
            air_currents: Vec::new(),
            skylights: Vec::new(),
            water: Vec::new(),
            graph_elev: Vec::new(),
            agent_height: Vec::new(),
            space_plan: Vec::new(),
            destructibles: Vec::new(),
            flood: Vec::new(),
            sandstorm: None,
            cover_obstacles: Vec::new(),
            open_air: false,
            fire_sources: Vec::new(),
            custom_pickups: Vec::new(),
            lane_paths: Vec::new(),
            lod_field: None,
            cfg,
        };
        for z in 0..h {
            for x in 0..w {
                let idx = (z * w + x) as usize;
                lvl.cells[idx] = if x <= 1 || z <= 1 || x >= w - 2 || z >= h - 2 { Cell::Wall } else { Cell::Floor };
            }
        }
        lvl
    }

    /// **MAP 1 — Flood Zone.** You fight in the basin below a great dam. Two spillway trenches run the length
    /// of the arena; they're dry and walkable at first, then surge with water on staggered cycles — flooding
    /// the low routes, forcing you up onto the slabs and catwalks, then draining away. The dam towers over the
    /// north end with its reservoir held behind, spilling through gate notches into the channels.
    fn build_flood_zone(cfg: MapConfig) -> Level {
        let seed_u = cfg.seed;
        let w = 56;
        let h = 56;
        let pal = Palette {
            floor: rgb(98, 102, 106),
            wall: rgb(122, 114, 100),
            trim: rgb(150, 150, 156),
            ceil: rgb(60, 64, 70),
            cover: rgb(132, 130, 126),
            sky: rgb(150, 162, 178),
            fog: rgb(120, 132, 145),
            accent: rgb(66, 122, 152),
            rock: rgb(112, 106, 96),
        };
        let mut lvl = Self::authored_blank(cfg, w, h, pal, 40.0);
        let ts = lvl.ts();

        // ── terrain ──────────────────────────────────────────────────────────────────────────────────────
        // The dam occupies the north band as an impassable wall base; the basin opens south of it.
        for z in 0..7 {
            for x in 0..w {
                lvl.set(x, z, Cell::Wall);
            }
        }
        // Two spillway channels run south from the dam — carved as trenches (lower floor), dry to start.
        let chans = [(15i32, 19i32), (35i32, 39i32)];
        let bed = -1.7f32;
        for &(cx0, cx1) in &chans {
            for z in 7..52 {
                for x in cx0..=cx1 {
                    let idx = (z * w + x) as usize;
                    lvl.heights[idx] = bed; // trench bottom
                }
            }
        }
        // A gentle berm of higher ground down the centre spine (safe high route between the channels).
        for z in 14..44 {
            for x in 24..32 {
                let idx = (z * w + x) as usize;
                lvl.heights[idx] = 0.55;
            }
        }

        // ── spawn rooms (south corners) ──────────────────────────────────────────────────────────────────
        for (id, (sx, sz)) in [(0usize, (7i32, 49i32)), (1, (48, 49))].into_iter() {
            lvl.rooms.push(Room {
                rect: Rect::new(sx - 3, sz - 3, sx + 3, sz + 3),
                id,
                center: Vec3::new(sx as f32 * ts, 0.0, sz as f32 * ts),
                floor_y: 0.0,
                ceil_y: lvl.ceil_y,
            });
        }

        // Bake floor + perimeter/dam walls from the cells & heights we just set.
        lvl.build_meshes();

        // ── the dam (north backdrop) ─────────────────────────────────────────────────────────────────────
        let concrete = rgb(150, 146, 138);
        let dam_z = 6.0 * ts; // downstream face sits at the south edge of the dam band
        let dam_h = 13.0;
        let span0 = 2.0 * ts;
        let span1 = (w as f32 - 2.0) * ts;
        let segs = 10;
        for i in 0..segs {
            let x0 = span0 + (span1 - span0) * i as f32 / segs as f32;
            let x1 = span0 + (span1 - span0) * (i as f32 + 1.0) / segs as f32;
            let cxw = (x0 + x1) * 0.5;
            // leave gate notches where the channels exit (lower segment there)
            let in_gate = chans.iter().any(|&(a, b)| {
                let gx = ((a + b) as f32 * 0.5) * ts;
                (cxw - gx).abs() < ts * 2.2
            });
            let seg_h = if in_gate { dam_h * 0.46 } else { dam_h };
            let half = Vec3::new((x1 - x0) * 0.5 * 0.98, seg_h * 0.5, 1.6);
            let center = Vec3::new(cxw, seg_h * 0.5, dam_z);
            lvl.meshes.push(boxx(center, half, dither(concrete, i * 5, 0.08), FACE_ALL));
            // downstream battered face for mass
            lvl.meshes.push(ramp(
                Vec3::new(x0, 0.0, dam_z + 3.2),
                Vec3::new(x1, 0.0, dam_z + 3.2),
                Vec3::new(x1, seg_h * 0.72, dam_z + 1.6),
                Vec3::new(x0, seg_h * 0.72, dam_z + 1.6),
                scale_rgb(concrete, 0.84),
            ));
        }
        // crest walkway across the top
        lvl.meshes.push(boxx(Vec3::new((span0 + span1) * 0.5, dam_h + 0.2, dam_z - 0.3), Vec3::new((span1 - span0) * 0.5, 0.2, 1.1), scale_rgb(concrete, 1.06), FACE_ALL));
        // reservoir held behind the dam (north), a still high sheet
        let mut resv = panel(Vec3::new((span0 + span1) * 0.5, dam_h * 0.78, dam_z - 5.5), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, -1.0), span1 - span0, 11.0, scale_rgb(pal.accent, 0.9));
        set_alpha(&mut resv, 130);
        resv.reflect = 0.6;
        lvl.meshes.push(resv);

        // ── flood channels + their water surfaces ────────────────────────────────────────────────────────
        for (ci, &(cx0, cx1)) in chans.iter().enumerate() {
            let region = Rect::new(cx0, 7, cx1, 51);
            let cxw = (cx0 + cx1) as f32 * 0.5 * ts;
            let czw = (7 + 51) as f32 * 0.5 * ts;
            let half_x = (cx1 - cx0 + 1) as f32 * 0.5 * ts;
            let half_z = (51 - 7) as f32 * 0.5 * ts;
            let crest_y = 0.35; // floods to just above the basin floor, lapping the banks
            // a square water surface over the channel, parked below the bed (dry) initially
            lvl.water.push(WaterBody {
                cx: cxw,
                cz: czw,
                radius: half_x.max(half_z),
                y: bed - 0.7,
                col: rgb(58, 104, 130),
                alpha: 150,
                reflect: 0.5,
                emissive: 0.0,
                lava: false,
                disc: false,
            });
            // NOTE: square water spans a bounding box; clamp by drawing only over the trench is approximate but
            // the trench banks hide the overrun. Stagger the two channels so the timing varies across routes.
            let water_idx = lvl.water.len() - 1;
            lvl.flood.push(FloodChannel {
                region,
                bed_y: bed,
                crest_y,
                water_idx,
                delay: 10.0 + ci as f32 * 6.0,
                rise: 5.0,
                hold: 6.0,
                fall: 5.0,
                gap: 9.0 + (1 - ci) as f32 * 6.0,
                phase: 0.0,
            });
            // spillway outlet box at the dam end of each channel
            lvl.meshes.push(boxx(Vec3::new(cxw, 1.1, 7.0 * ts), Vec3::new(half_x * 0.9, 1.1, 1.2), scale_rgb(concrete, 0.92), FACE_ALL));
        }

        // ── cover & debris (concrete chunks = destructibles, slabs = ramps to high ground) ───────────────
        use crate::destructible::{Destructible, DestructibleKind};
        let mut s = seed_u | 1;
        let mut rnd = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f32 / (1u64 << 53) as f32
        };
        // standable raised slabs (high ground islands to escape the flood)
        for &(tx, tz, hwt, hht) in &[(10i32, 24i32, 2.2f32, 1.0f32), (45, 22, 2.0, 1.1), (28, 30, 1.6, 1.4), (12, 38, 1.8, 0.9), (43, 40, 2.0, 1.0)] {
            let c = Vec3::new(tx as f32 * ts, 0.0, tz as f32 * ts);
            let top = hht;
            lvl.meshes.push(boxx(Vec3::new(c.x, top, c.z), Vec3::new(hwt, top, hwt * 0.8), dither(scale_rgb(concrete, 0.9), tx, 0.1), FACE_ALL));
            lvl.obstacles.push(Obstacle { center: Vec3::new(c.x, top, c.z), half: Vec3::new(hwt, top, hwt * 0.8) });
            // a debris ramp up to it
            lvl.meshes.push(ramp(
                Vec3::new(c.x - hwt, 0.0, c.z + hwt * 0.8 + 1.6),
                Vec3::new(c.x + hwt, 0.0, c.z + hwt * 0.8 + 1.6),
                Vec3::new(c.x + hwt, top, c.z + hwt * 0.8),
                Vec3::new(c.x - hwt, top, c.z + hwt * 0.8),
                scale_rgb(concrete, 0.78),
            ));
        }
        // breakable concrete barriers scattered as cover through the basin
        let mut placed = 0;
        while placed < 16 {
            let tx = 4 + (rnd() * 48.0) as i32;
            let tz = 9 + (rnd() * 40.0) as i32;
            // keep cover out of the trenches and off the dam
            if chans.iter().any(|&(a, b)| tx >= a - 1 && tx <= b + 1) {
                continue;
            }
            let c = Vec3::new(tx as f32 * ts, lvl.floor_h(tx as f32 * ts, tz as f32 * ts), tz as f32 * ts);
            let roll = rnd();
            let dseed = (seed_u as u32).wrapping_add(placed as u32 * 131).wrapping_mul(2_654_435_761);
            let d = if roll < 0.6 {
                Destructible::new(DestructibleKind::Barrier, Vec3::new(c.x, c.y + 0.6, c.z), Vec3::new(1.0, 0.6, 0.35), dseed)
            } else {
                Destructible::new(DestructibleKind::CrateStack, Vec3::new(c.x, c.y + 0.55, c.z), Vec3::new(0.6, 0.55, 0.6), dseed)
            };
            lvl.destructibles.push(d);
            placed += 1;
        }
        // fuel drums staged along the works — shootable, and their blasts chain into nearby cover
        for k in 0..6u32 {
            let bx = (8.0 + (k as f32 * 7.3) % 40.0) * ts;
            let bz = (10.0 + (k as f32 * 11.7) % 36.0) * ts;
            let by = lvl.floor_h(bx, bz);
            if by.abs() > 2.5 {
                continue; // skip the trench beds — drums sit on the working ground
            }
            let dseed = (seed_u as u32).wrapping_add(k * 977).wrapping_mul(2_654_435_761);
            lvl.destructibles.push(Destructible::new(
                crate::destructible::DestructibleKind::ExplosiveBarrel,
                Vec3::new(bx, by + 0.5, bz),
                Vec3::new(0.34, 0.5, 0.34),
                dseed,
            ));
        }
        // ── ganged pipe runs spanning the channels (overhead sightline ribs) ─────────────────────────────
        // Real dam works route bundled penstock/service pipes overhead. `pipes_run` gangs several pipes with
        // hash-varied radii, rust, brackets, and the odd elbow branch — richer than a single bare tube.
        {
            use crate::fsm::detail::pipes_run;
            for &(cx0, cx1) in &chans {
                let cxw = (cx0 + cx1) as f32 * 0.5 * ts;
                for &zt in &[16.0f32, 30.0, 44.0] {
                    lvl.meshes.push(pipes_run(
                        Vec3::new(cxw - 5.5, 2.6, zt * ts),
                        Vec3::new(cxw + 5.5, 2.6, zt * ts),
                        3,
                        (zt as u32).wrapping_mul(2_654_435_761) ^ 0x91,
                    ));
                }
            }
            // slack service cables sagging between the two spillway outlet housings
            use crate::fsm::detail::cable_sag;
            let c0 = (chans[0].0 + chans[0].1) as f32 * 0.5 * ts;
            let c1 = (chans[1].0 + chans[1].1) as f32 * 0.5 * ts;
            for &zt in &[20.0f32, 34.0] {
                lvl.meshes.push(cable_sag(
                    Vec3::new(c0, 3.4, zt * ts),
                    Vec3::new(c1, 3.4, zt * ts),
                    1.4,
                    rgb(30, 28, 26),
                ));
            }
        }

        // ── bank-side concrete cover walls ── runs of low walls on each channel bank ──────────────────────
        // Left bank of west channel, right bank of east channel — creates a "behind cover" incentive
        let wall_c = scale_rgb(concrete, 0.88);
        for &(bx, bz, bwx, bwz) in &[
            // west channel left bank
            (chans[0].0 as f32 - 2.5, 18.0, 0.22, 2.8),
            (chans[0].0 as f32 - 2.5, 28.0, 0.22, 2.8),
            (chans[0].0 as f32 - 2.5, 38.0, 0.22, 2.8),
            // east channel right bank
            (chans[1].1 as f32 + 2.5, 18.0, 0.22, 2.8),
            (chans[1].1 as f32 + 2.5, 28.0, 0.22, 2.8),
            (chans[1].1 as f32 + 2.5, 38.0, 0.22, 2.8),
            // inner banks facing each other across the centre spine
            (chans[0].1 as f32 + 2.5, 22.0, 0.22, 2.4),
            (chans[0].1 as f32 + 2.5, 35.0, 0.22, 2.4),
            (chans[1].0 as f32 - 2.5, 22.0, 0.22, 2.4),
            (chans[1].0 as f32 - 2.5, 35.0, 0.22, 2.4),
        ] {
            let wx = bx * ts;
            let wz = bz * ts;
            let gy = lvl.floor_h(wx, wz).max(0.0);
            let wh = Vec3::new(bwx, 0.75, bwz);
            let wc = Vec3::new(wx, gy + 0.75, wz);
            lvl.meshes.push(boxx(wc, wh, dither(wall_c, bx as i32 * 7 + bz as i32, 0.06), FACE_ALL));
            lvl.obstacles.push(Obstacle { center: wc, half: wh });
        }

        // ── dam-face catwalks: two cantilevered walkways at mid-height on the dam's south face ──────────
        // These give elevated flanking positions along the north wall and make the dam feel functional.
        let cat_y = 5.5;
        let cat_col = dither(scale_rgb(concrete, 0.82), 99, 0.10);
        for &cx_cat in &[(span0 + (span1 - span0) * 0.28), (span0 + (span1 - span0) * 0.72)] {
            let hw = (span1 - span0) * 0.13;
            let cat_c = Vec3::new(cx_cat, cat_y + 0.15, dam_z + 1.2);
            let cat_h = Vec3::new(hw, 0.15, 1.1);
            let mut cat_mesh = boxx(cat_c, cat_h, cat_col, FACE_ALL);
            cat_mesh.material = Some(crate::mesh::Material::HexPlate); // industrial tread-plate walkway
            lvl.meshes.push(cat_mesh);
            lvl.obstacles.push(Obstacle { center: cat_c, half: cat_h });
            // railing on the south edge
            let rail_c = Vec3::new(cx_cat, cat_y + 0.7, dam_z + 2.1);
            let rail_h = Vec3::new(hw, 0.4, 0.08);
            lvl.meshes.push(boxx(rail_c, rail_h, dither(wall_c, 77, 0.04), FACE_ALL));
            lvl.obstacles.push(Obstacle { center: rail_c, half: rail_h });
            // access ladder (a few box steps) — just a visual stand-in, the player can vault up
            let step_x = cx_cat - hw + 0.4;
            for si in 0..4i32 {
                let sy = si as f32 * 1.4;
                let sc = Vec3::new(step_x, sy + 0.75, dam_z + 0.5);
                let sh = Vec3::new(0.28, 0.75, 0.28);
                lvl.meshes.push(boxx(sc, sh, dither(concrete, si, 0.08), FACE_ALL));
            }
            // register as platform for the scorer
            lvl.platforms.push(Platform { x0: cx_cat - hw, z0: dam_z + 0.1, x1: cx_cat + hw, z1: dam_z + 2.3, top: cat_y + 0.3 });
        }

        // ── register the 5 raised slabs as platforms ─────────────────────────────────────────────────────
        for &(tx, tz, hwt, hht) in &[(10i32, 24i32, 2.2f32, 1.0f32), (45, 22, 2.0, 1.1), (28, 30, 1.6, 1.4), (12, 38, 1.8, 0.9), (43, 40, 2.0, 1.0)] {
            let cx = tx as f32 * ts;
            let cz = tz as f32 * ts;
            lvl.platforms.push(Platform { x0: cx - hwt, z0: cz - hwt * 0.8, x1: cx + hwt, z1: cz + hwt * 0.8, top: hht });
        }

        // ── segmenting walls: partial barriers that carve the open basin into mid-range pockets ──────────
        // A wide-open basin plays as one long-range duel. These staggered walls (tall enough to occlude,
        // with gaps to keep every route open) break sightlines into the 9–26m mid band the game wants.
        let seg_col = dither(scale_rgb(concrete, 0.9), 63, 0.07);
        // Each entry: (x_tile, z_tile, along_x, length_tiles). Walls are 2.2 tall (above eye), 0.3 thick.
        for &(tx, tz, along_x, len) in &[
            // horizontal bands segmenting the north/south flow, offset so routes zig-zag
            (5i32, 15i32, true, 8i32),
            (33, 15, true, 9),
            (7, 27, true, 6),
            (40, 27, true, 7),
            (5, 39, true, 7),
            (33, 39, true, 9),
            // short verticals near the spawn approaches
            (9, 45, false, 5),
            (45, 45, false, 5),
        ] {
            let thick = 0.3f32;
            for i in 0..len {
                let (cxt, czt) = if along_x { (tx + i, tz) } else { (tx, tz + i) };
                // leave periodic gaps (every 4th tile) so nothing is sealed
                if i % 4 == 3 {
                    continue;
                }
                // skip trench tiles
                if chans.iter().any(|&(a, b)| cxt >= a - 1 && cxt <= b + 1) {
                    continue;
                }
                let wx = cxt as f32 * ts;
                let wz = czt as f32 * ts;
                let gy = lvl.floor_h(wx, wz).max(0.0);
                let (hw, hd) = if along_x { (ts * 0.5, thick) } else { (thick, ts * 0.5) };
                let c = Vec3::new(wx, gy + 1.1, wz);
                let half = Vec3::new(hw, 1.1, hd);
                let mut m = boxx(c, half, seg_col, FACE_ALL);
                m.material = Some(crate::mesh::Material::StoneBlock);
                lvl.meshes.push(m);
                lvl.obstacles.push(Obstacle { center: c, half });
            }
        }

        // ── tall sightline breakers: pump housings & container stacks across the basin ────────────────────
        // The basin is a wide-open field; without eye-height structure every fight is a long-range duel.
        // These break the space into mid-range pockets — reaching above eye level so they actually occlude.
        let hut_col = dither(scale_rgb(concrete, 0.94), 42, 0.08);
        for &(tx, tz, hw, hd, hh) in &[
            // pump houses flanking the basin
            (8i32, 30i32, 1.6f32, 1.8f32, 2.4f32),
            (47, 32, 1.7, 1.6, 2.4),
            (20, 44, 1.4, 1.4, 2.2),
            (36, 44, 1.4, 1.4, 2.2),
            // machinery blocks mid-basin, offset from the spine
            (18, 20, 1.2, 2.0, 2.0),
            (38, 18, 2.0, 1.2, 2.0),
            (14, 33, 1.3, 1.3, 1.9),
            (42, 27, 1.3, 1.3, 1.9),
        ] {
            let wx = tx as f32 * ts;
            let wz = tz as f32 * ts;
            // don't drop these into the trenches
            if chans.iter().any(|&(a, b)| tx >= a - 1 && tx <= b + 1) {
                continue;
            }
            let gy = lvl.floor_h(wx, wz).max(0.0);
            let c = Vec3::new(wx, gy + hh, wz);
            let half = Vec3::new(hw, hh, hd);
            let mut m = boxx(c, half, hut_col, FACE_ALL);
            m.material = Some(crate::mesh::Material::StoneBlock);
            lvl.meshes.push(m);
            lvl.obstacles.push(Obstacle { center: c, half });
            // a low roof lip so the top reads as a standable platform (accessible via nearby slabs/vault)
            lvl.platforms.push(Platform { x0: wx - hw, z0: wz - hd, x1: wx + hw, z1: wz + hd, top: gy + hh * 2.0 });
            // pipe/vent detail on top
            lvl.meshes.push(tube(Vec3::new(wx - hw * 0.4, gy + hh * 2.0, wz), Vec3::new(wx - hw * 0.4, gy + hh * 2.0 + 0.9, wz), 0.14, 6, rgb(80, 78, 74)));
        }

        // ── populate cover_obstacles: static walls + destructibles = playable cover density ───────────────
        // Snapshot all obstacles placed since the slabs — those are the cover walls; include the slabs too.
        // (Obstacles here are the concrete walls, catwalks, and slab surfaces — not building shells.)
        lvl.cover_obstacles = lvl.obstacles.clone();
        lvl.open_air = true; // dam basin is open to the sky

        // the works' outer edge — same skybox + cliff/abyss boundary treatment as the procedural maps
        lvl.build_cliffs();
        lvl.dissolve_boundary();
        lvl.dress_boundary_margin(&mut Rng::new(seed_u ^ 0xB0DE));
        lvl.chunk_meshes(40.0); // whole-chunk frustum culling on the basin + dam geometry

        lvl
    }

    /// Instance one placed asset: append its mesh(es) (rotated + scaled, seated on the ground) to the level
    /// and, if it's a solid kind, a matching collision obstacle. Used by `from_custom` and the live editor.
    pub fn add_placed(&mut self, p: &crate::builder::Placed) {
        use crate::builder::AssetKind as A;
        let gy = self.floor_h(p.x, p.z);
        let base = Vec3::new(p.x, gy, p.z);
        let up = Vec3::new(0.0, 1.0, 0.0);
        let s = p.scale.clamp(0.3, 4.0);
        let seed = (((p.x * 13.3) as i32) ^ ((p.z * 7.7) as i32) ^ 0x51A3) as u32;
        let wood = rgb(96, 70, 42);
        let leaf = match self.cfg.theme {
            MapTheme::Mistfall => rgb(120, 170, 160),
            MapTheme::Toxic => rgb(140, 170, 60),
            MapTheme::Volcanic => rgb(126, 70, 40),
            MapTheme::Ice => rgb(200, 225, 240),
            _ => rgb(104, 144, 64),
        };
        let stone = lerp_c(self.pal.rock, rgb(150, 150, 142), 0.2);
        let mut out: Vec<Mesh3> = Vec::new();
        match p.kind {
            A::Tree => {
                for m in tree(base, 4.0 * s, 0.3 * s, 2, seed, wood, leaf) {
                    out.push(m);
                }
            }
            A::Pine => {
                let mut trunk = tube(base, base + up * (1.0 * s), 0.16 * s, 8, wood);
                tint_up(&mut trunk, lerp_c(wood, rgb(150, 120, 80), 0.3), scale_rgb(wood, 0.6));
                out.push(trunk);
                for li in 0..3 {
                    let t = li as f32;
                    let cy = gy + (0.9 + t * 1.1) * s;
                    let mut cone = crystal(Vec3::new(p.x, cy, p.z), (2.0 - t * 0.5) * s, (1.0 - t * 0.22) * s, 7, seed + li, leaf);
                    tint_up(&mut cone, lerp_c(leaf, rgb(220, 230, 200), 0.2), scale_rgb(leaf, 0.55));
                    out.push(cone);
                }
            }
            A::Bush => {
                let mut b = leaf_cluster(base + up * (0.4 * s), 0.7 * s, seed, leaf);
                tint_up(&mut b, lerp_c(leaf, rgb(230, 235, 200), 0.2), scale_rgb(leaf, 0.5));
                out.push(b);
            }
            A::Rock => {
                let mut r = rock(base + up * (0.25 * s), 0.6 * s, seed, self.pal.rock);
                tint_up(&mut r, stone, scale_rgb(self.pal.rock, 0.5));
                out.push(r);
            }
            A::Boulder => {
                let mut r = rock(base + up * (0.6 * s), 1.4 * s, seed, self.pal.rock);
                tint_up(&mut r, stone, scale_rgb(self.pal.rock, 0.5));
                out.push(r);
            }
            A::Crate => {
                let mut c = boxx(base + up * (0.5 * s), Vec3::splat(0.5 * s), rgb(120, 92, 54), FACE_ALL);
                tint_up(&mut c, rgb(150, 120, 76), rgb(80, 60, 36));
                out.push(c);
            }
            A::Barrel => {
                let mut b = tube(base, base + up * (1.05 * s), 0.4 * s, 12, rgb(110, 116, 124));
                tint_up(&mut b, rgb(150, 156, 164), rgb(70, 74, 82));
                b.reflect = 0.22;
                out.push(b);
            }
            A::Pillar => {
                let mut c = boxx(base + up * (1.6 * s), Vec3::new(0.4 * s, 1.6 * s, 0.4 * s), stone, FACE_ALL);
                tint_up(&mut c, lerp_c(stone, rgb(255, 255, 255), 0.15), scale_rgb(stone, 0.55));
                out.push(c);
            }
            A::Crystal => {
                let mut c = crystal(base, 2.0 * s, 0.5 * s, 6, seed, self.pal.accent);
                c.emissive = 0.6;
                c.reflect = 0.3;
                out.push(c);
            }
            A::Cairn => {
                for mut m in cairn(base, 5, 0.6 * s, seed, self.pal.rock) {
                    m.material = Some(crate::mesh::Material::StoneBlock);
                    out.push(m);
                }
            }
            A::Lamp => {
                let mut post = tube(base, base + up * (2.2 * s), 0.08 * s, 6, rgb(60, 62, 68));
                tint_up(&mut post, rgb(90, 92, 100), rgb(40, 42, 48));
                out.push(post);
                let mut head = boxx(base + up * (2.3 * s), Vec3::splat(0.2 * s), self.pal.accent, FACE_ALL);
                head.emissive = 0.95;
                out.push(head);
            }
            A::Reeds => {
                for j in 0..4 {
                    let a = j as f32 * 1.7 + seed as f32 * 0.01;
                    let off = Vec3::new(a.cos() * 0.25 * s, 0.0, a.sin() * 0.25 * s);
                    let hgt = (0.9 + ((seed + j) % 5) as f32 * 0.12) * s;
                    let mut reed = leaf_cluster(base + off + up * (hgt * 0.5), 0.28 * s, seed + j, leaf);
                    scale_xz(&mut reed, 0.45, 0.45);
                    out.push(reed);
                }
            }
            A::ManCannon => {
                // a launch pad: a metal disc, a glowing energy ring, and an up-arrow chevron
                let mut pad = tube(base, base + up * (0.16 * s), 0.85 * s, 14, rgb(70, 76, 86));
                tint_up(&mut pad, rgb(120, 128, 140), rgb(44, 48, 56));
                pad.reflect = 0.3;
                out.push(pad);
                let mut ring = disc(base + up * (0.18 * s), 0.7 * s, 16, lerp_c(self.pal.accent, rgb(120, 220, 255), 0.5));
                ring.emissive = 0.95;
                out.push(ring);
                for j in 0..3 {
                    let y = 0.2 * s + j as f32 * 0.22 * s;
                    let wsz = (0.5 - j as f32 * 0.13) * s;
                    let mut chev = boxx(base + up * y, Vec3::new(wsz, 0.03 * s, 0.06 * s), rgb(150, 235, 255), FACE_ALL);
                    chev.emissive = 1.0;
                    out.push(chev);
                }
                // register the launcher so the game can fling the player upward when they stand on it
                self.cannons.push((Vec3::new(p.x, gy, p.z), p.param.max(3.0)));
            }
            A::HealthPack | A::ArmorPack | A::AmmoPack => {
                // a small glowing base pad; the collectible itself floats above it (spawned by the game)
                let col = p.kind.pickup().map(|k| k.color()).unwrap_or(self.pal.accent);
                let mut pad = disc(base + up * 0.03, 0.45 * s, 12, col);
                pad.emissive = 0.5;
                out.push(pad);
                let mut rim = tube(base, base + up * (0.08 * s), 0.45 * s, 12, scale_rgb(col, 0.7));
                rim.emissive = 0.3;
                out.push(rim);
                if let Some(k) = p.kind.pickup() {
                    self.custom_pickups.push((Vec3::new(p.x, gy, p.z), k));
                }
            }
        }
        for mut m in out {
            if p.rot.abs() > 0.001 {
                rotate_y_about(&mut m, base, p.rot);
            }
            self.meshes.push(m);
        }
        if p.kind.solid() {
            let (r, hh) = match p.kind {
                A::Tree | A::Pine => (0.4 * s, 2.0 * s),
                A::Pillar => (0.45 * s, 1.6 * s),
                A::Crystal => (0.5 * s, 1.4 * s),
                A::Boulder => (1.2 * s, 1.0 * s),
                A::Cairn => (0.7 * s, 1.0 * s),
                A::Barrel => (0.42 * s, 0.6 * s),
                A::Crate => (0.55 * s, 0.55 * s),
                _ => (0.6 * s, 0.7 * s),
            };
            self.obstacles.push(Obstacle { center: Vec3::new(p.x, gy + hh, p.z), half: Vec3::new(r, hh, r) });
            let mut sd = disc(Vec3::new(p.x, gy + 0.04, p.z), r * 1.25, 8, rgb(8, 10, 12));
            scale_xz(&mut sd, 1.0, 0.82);
            set_alpha(&mut sd, 76);
            self.meshes.push(sd);
        }
    }

    fn build_structures(&mut self, rng: &mut Rng) {
        // Organic clearings biomes keep their open ground; the built biomes get interior mezzanines.
        if matches!(
            self.cfg.theme,
            MapTheme::Volcanic | MapTheme::Ice | MapTheme::Alien | MapTheme::Canopy | MapTheme::Mistfall | MapTheme::Toxic | MapTheme::Meadow | MapTheme::Redwood | MapTheme::Station | MapTheme::Carrier
        ) {
            return;
        }
        let ts = self.ts();
        let pal = self.pal;
        let mut mesh = Mesh3::new(pal.wall);
        let rooms = self.rooms.clone();
        let mut platforms: Vec<Platform> = Vec::new();
        let mut obstacles: Vec<Obstacle> = Vec::new();
        for (i, room) in rooms.iter().enumerate() {
            if i == 0 || room.rect.w() < 8 || room.rect.h() < 9 {
                continue;
            }
            if !rng.chance(0.6) {
                continue;
            }
            let fy = room.floor_y;
            let r = room.rect.inset(1);
            // Deck over the back portion of the room.
            let dx0 = r.x0 as f32 * ts;
            let dx1 = r.x1 as f32 * ts;
            let dz0 = r.z0 as f32 * ts;
            let dz1 = (r.z0 + (r.h() / 2).max(3)) as f32 * ts;
            let top = fy + 3.2;
            let (cx, cz) = ((dx0 + dx1) * 0.5, (dz0 + dz1) * 0.5);
            let half = Vec3::new((dx1 - dx0) * 0.5, 0.12, (dz1 - dz0) * 0.5);
            let mut deck = boxx(Vec3::new(cx, top, cz), half, scale_rgb(pal.floor, 0.92), FACE_ALL);
            tint_up(&mut deck, lerp_c(pal.floor, rgb(255, 255, 255), 0.12), scale_rgb(pal.floor, 0.6));
            mesh.merge(&deck);
            platforms.push(Platform { x0: dx0, z0: dz0, x1: dx1, z1: dz1, top });

            // Corner support pillars (block at all heights).
            for (px, pz) in [(dx0, dz0), (dx1, dz0), (dx0, dz1), (dx1, dz1)] {
                mesh.merge(&tube(Vec3::new(px, fy, pz), Vec3::new(px, top, pz), 0.22, 6, scale_rgb(pal.trim, 0.8)));
                obstacles.push(Obstacle {
                    center: Vec3::new(px, (fy + top) * 0.5, pz),
                    half: Vec3::new(0.3, (top - fy) * 0.5, 0.3),
                });
            }
            // Back + side railings (visual only) — leave the front open for the stairs.
            let rail_c = scale_rgb(pal.trim, 0.9);
            let rh = 0.45;
            mesh.merge(&boxx(Vec3::new(cx, top + rh, dz0), Vec3::new(half.x, rh * 0.5, 0.05), rail_c, FACE_ALL));
            mesh.merge(&boxx(Vec3::new(dx0, top + rh, cz), Vec3::new(0.05, rh * 0.5, half.z), rail_c, FACE_ALL));
            mesh.merge(&boxx(Vec3::new(dx1, top + rh, cz), Vec3::new(0.05, rh * 0.5, half.z), rail_c, FACE_ALL));

            // Stairs climbing from the room floor up to the deck's front edge.
            let steps = 7;
            let sw = ((dx1 - dx0) * 0.5).min(2.4);
            for s in 0..steps {
                let f = (s + 1) as f32 / steps as f32;
                let sy = fy + f * 3.2;
                let sz = dz1 + (steps - s) as f32 * 0.55;
                mesh.merge(&boxx(Vec3::new(cx, sy - 0.1, sz), Vec3::new(sw, 0.1, 0.32), scale_rgb(pal.floor, 0.82), FACE_ALL));
                platforms.push(Platform { x0: cx - sw, z0: sz - 0.32, x1: cx + sw, z1: sz + 0.32, top: sy });
            }

            // A catwalk running from the deck along the right wall to the far side of the room — a raised
            // flanking route and vantage that turns the room into a real two-level space.
            let z_far = r.z1 as f32 * ts;
            if z_far - dz1 > 5.0 && rng.chance(0.65) {
                let cw_w = 1.8;
                let cw_x = dx1 - cw_w * 0.5 - 0.3;
                let cc = Vec3::new(cw_x, top, (dz1 + z_far) * 0.5);
                let chalf = Vec3::new(cw_w * 0.5, 0.12, (z_far - dz1) * 0.5);
                let mut cwk = boxx(cc, chalf, scale_rgb(pal.floor, 0.88), FACE_ALL);
                tint_up(&mut cwk, lerp_c(pal.floor, rgb(255, 255, 255), 0.1), scale_rgb(pal.floor, 0.55));
                mesh.merge(&cwk);
                platforms.push(Platform { x0: cw_x - cw_w * 0.5, z0: dz1, x1: cw_x + cw_w * 0.5, z1: z_far, top });
                for pz in [dz1, z_far] {
                    mesh.merge(&tube(Vec3::new(cw_x, fy, pz), Vec3::new(cw_x, top, pz), 0.18, 6, scale_rgb(pal.trim, 0.8)));
                    obstacles.push(Obstacle { center: Vec3::new(cw_x, (fy + top) * 0.5, pz), half: Vec3::new(0.24, (top - fy) * 0.5, 0.24) });
                }
                // inner-edge railing (visual)
                mesh.merge(&boxx(
                    Vec3::new(cw_x - cw_w * 0.5, top + 0.4, (dz1 + z_far) * 0.5),
                    Vec3::new(0.05, 0.2, (z_far - dz1) * 0.5),
                    scale_rgb(pal.trim, 0.9),
                    FACE_ALL,
                ));
            }
        }
        self.platforms.extend(platforms);
        self.obstacles.extend(obstacles);
        if mesh.tri_count() > 0 {
            self.meshes.push(mesh);
        }
    }

    /// Build the Culvert Breach's two tiers on top of the carved plan: a tall stone wall along the centre
    /// line (the Wall cells already block the ground), an elevated wall-walk *balcony* on its northern face
    /// (a platform over the court's floor, so the 2-D collision keeps it walkable) with a crenellated parapet
    /// to fire over, braziers of roaring coals, a defender stair up from the court and a rubble ramp up beside
    /// the breach, and a flooded timber-barricaded trench to the south. The breach is the only ground crossing.
    fn build_culvert(&mut self) {
        if !self.cfg.culvert {
            return;
        }
        let ts = self.ts();
        let pal = self.pal;
        let sz_t = self.cfg.size;
        let (cx_t, cz_t) = (sz_t / 2, sz_t / 2);
        let zc = (cz_t as f32 + 0.5) * ts;
        let court_y = self.floor_y;
        let trench_y = self.floor_y - 1.4;
        let wall_h = self.cfg.wall_height;
        let walk_top = court_y + wall_h;
        let (wmin, wmax) = (3.0 * ts, (sz_t - 3) as f32 * ts);
        let bx0 = (cx_t - 3) as f32 * ts;
        let bx1 = (cx_t + 4) as f32 * ts;
        let stone = scale_rgb(pal.wall, 1.0);
        let mut mesh = Mesh3::new(stone);
        let mut glow = Mesh3::new(rgb(240, 120, 40)).emit(1.0);
        let mut platforms: Vec<Platform> = Vec::new();

        // The wall + walk in two segments (west + east of the breach).
        for (segx0, segx1) in [(wmin, bx0), (bx1, wmax)] {
            if segx1 - segx0 < 2.0 {
                continue;
            }
            let cxw = (segx0 + segx1) * 0.5;
            let hx = (segx1 - segx0) * 0.5;
            // tall stone wall body along the centre line, rising from the court to the parapet
            let mut body = boxx(Vec3::new(cxw, court_y + (walk_top + 0.6 - court_y) * 0.5, zc), Vec3::new(hx, (walk_top + 0.6 - court_y) * 0.5, 0.9), stone, FACE_ALL);
            tint_height(&mut body, court_y, walk_top, scale_rgb(stone, 0.5), stone);
            mesh.merge(&body);
            // crenellated parapet on top of the wall (merlons with gaps, some shot away)
            let merlons = ((segx1 - segx0) / 1.4).max(1.0) as i32;
            for k in 0..merlons {
                if k % 2 == 1 {
                    continue;
                }
                let mx = segx0 + (k as f32 + 0.5) * 1.4;
                let mut m = boxx(Vec3::new(mx, walk_top + 0.55, zc), Vec3::new(0.5, 0.55, 0.95), stone, FACE_ALL);
                if hash01(k, self.cfg.seed as i32) < 0.3 {
                    for v in &mut m.verts {
                        if v.y > walk_top + 0.6 {
                            v.y -= 0.55; // chipped / shot away
                        }
                    }
                }
                mesh.merge(&m);
            }
            // the wall-walk balcony: a platform over the court floor on the NORTH face (walkable in 2-D collision)
            let wz0 = zc - 1.6 * ts;
            let wz1 = zc - 0.5 * ts;
            let mut deck = boxx(Vec3::new(cxw, walk_top, (wz0 + wz1) * 0.5), Vec3::new(hx, 0.14, (wz1 - wz0) * 0.5), scale_rgb(pal.floor, 0.88), FACE_ALL);
            tint_up(&mut deck, lerp_c(pal.floor, rgb(255, 255, 255), 0.1), scale_rgb(pal.floor, 0.55));
            mesh.merge(&deck);
            platforms.push(Platform { x0: segx0, z0: wz0, x1: segx1, z1: wz1, top: walk_top + 0.05 });
            // a brazier of roaring coals on the walk
            glow.merge(&disc(Vec3::new(cxw, walk_top + 0.35, (wz0 + wz1) * 0.5), 0.4, 8, rgb(255, 140, 50)));
            mesh.merge(&tube(Vec3::new(cxw, walk_top + 0.1, (wz0 + wz1) * 0.5), Vec3::new(cxw, walk_top + 0.3, (wz0 + wz1) * 0.5), 0.12, 4, rgb(40, 36, 34)));
        }

        // Defender stair: from the north court up to the west walk segment (steps over the court floor).
        let steps = (wall_h / 0.5).ceil() as i32;
        let sx = (cx_t - 10) as f32 * ts;
        let sw = 1.6;
        let wz_front = zc - 1.6 * ts;
        for s in 0..steps {
            let f = (s + 1) as f32 / steps as f32;
            let sy = court_y + f * wall_h;
            let sz = wz_front - 0.6 - (steps - s) as f32 * 0.6; // north of the walk, climbing toward it
            mesh.merge(&boxx(Vec3::new(sx, sy - 0.1, sz), Vec3::new(sw, 0.1, 0.34), scale_rgb(pal.floor, 0.8), FACE_ALL));
            platforms.push(Platform { x0: sx - sw, z0: sz - 0.34, x1: sx + sw, z1: sz + 0.34, top: sy });
        }

        // Rubble ramp: a debris pile beside the breach climbing from the court up to the east walk — the
        // attacker's contested route to the top tier once they're through the hole.
        let rsteps = (wall_h / 0.55).ceil() as i32;
        let rx = bx1 + 1.2;
        let rw = 1.4;
        for s in 0..rsteps {
            let f = (s + 1) as f32 / rsteps as f32;
            let sy = court_y + f * wall_h;
            let sz = wz_front - 0.4 - (rsteps - s) as f32 * 0.5;
            mesh.merge(&boxx(Vec3::new(rx, sy - 0.12, sz), Vec3::new(rw, 0.12, 0.4), scale_rgb(stone, 0.85), FACE_ALL));
            platforms.push(Platform { x0: rx - rw, z0: sz - 0.4, x1: rx + rw, z1: sz + 0.4, top: sy });
        }
        mesh.merge(&crate::fsm::detail::debris(Vec3::new((bx0 + bx1) * 0.5, court_y, zc + 1.2), 3.2, 16, scale_rgb(stone, 0.8), self.cfg.seed as u32));
        for ex in [bx0, bx1] {
            mesh.merge(&boxx(Vec3::new(ex, court_y + wall_h * 0.5, zc), Vec3::new(0.2, wall_h * 0.5, 0.95), rgb(26, 22, 22), FACE_ALL));
        }

        // Flooded trench to the south: a translucent water sheet + splintered timber barricades bowing in.
        let mut water = Mesh3::new(rgb(58, 64, 56));
        let (zt0, zt1) = (zc + 1.6 * ts, (sz_t - 3) as f32 * ts);
        water.push_quad(
            Vec3::new(wmin, trench_y + 0.16, zt0),
            Vec3::new(wmax, trench_y + 0.16, zt0),
            Vec3::new(wmax, trench_y + 0.16, zt1),
            Vec3::new(wmin, trench_y + 0.16, zt1),
            Vec3::new(0.0, 1.0, 0.0),
            rgb(58, 64, 56),
        );
        set_alpha(&mut water, 150);
        self.meshes.push(water);
        for i in 0..8 {
            let txp = wmin + (i as f32 + 0.5) / 8.0 * (wmax - wmin);
            mesh.merge(&boxx(Vec3::new(txp, trench_y + 0.6, zc + 3.0 * ts), Vec3::new(0.5, 0.6, 0.12), rgb(86, 66, 44), FACE_ALL));
        }

        self.platforms.extend(platforms);
        self.meshes.push(mesh);
        if glow.tri_count() > 0 {
            self.meshes.push(glow);
        }
    }

    /// Build the submerged-docks split level: a shallow emerald flood over the whole basin (animated by the
    /// water system), a labyrinth of half-submerged rust-red shipping containers (cover that limits sightlines
    /// down low), and a grid of raised steel-grating walkways with railings, support pillars and end ramps
    /// (long sightlines up top). Plus the doc's props — a corroded anchor on a chain and rubber dock fenders.
    fn build_docks(&mut self) {
        if !self.cfg.docks {
            return;
        }
        let ts = self.ts();
        let pal = self.pal;
        let sz = self.cfg.size;
        let base_y = self.floor_y;
        let (wmin, wmax) = (3.5 * ts, (sz - 3) as f32 * ts);
        let mut mesh = Mesh3::new(scale_rgb(pal.wall, 0.8));
        let mut rust = Mesh3::new(rgb(150, 70, 52));
        let mut platforms: Vec<Platform> = Vec::new();
        let mut obstacles: Vec<Obstacle> = Vec::new();
        let mut seed = self.cfg.seed as u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / 16_777_216.0
        };

        // 1) Shallow flood over the whole basin (animated, reflective emerald — non-lethal wading water).
        let span = sz as f32 * ts;
        self.water.push(WaterBody {
            cx: span * 0.5,
            cz: span * 0.5,
            radius: span * 0.5,
            y: base_y + 0.32,
            col: rgb(28, 78, 66),
            alpha: 150,
            reflect: 0.68,
            emissive: 0.0,
            lava: false,
            disc: false,
        });

        // 2) Container labyrinth: half-submerged rust-red boxes in staggered rows, leaving snaking lanes. Each
        //    is a cover obstacle; the lower band is darkened (waterline) with a barnacle/kelp tint.
        let rows = 5;
        let cols = 6;
        for rz in 0..rows {
            for cxi in 0..cols {
                if rnd() < 0.32 {
                    continue; // gaps make the labyrinth
                }
                let px = wmin + (cxi as f32 + 0.5 + (rnd() - 0.5) * 0.4) / cols as f32 * (wmax - wmin);
                let pz = wmin + (rz as f32 + 0.5) / rows as f32 * (wmax - wmin);
                let long = rnd() > 0.5;
                let (hx, hz) = if long { (1.9, 0.85) } else { (0.85, 1.9) };
                let cc = Vec3::new(px, base_y + 0.7, pz);
                let mut box_m = boxx(cc, Vec3::new(hx, 0.7, hz), dither(rgb(150, 70, 52), (rz * 7 + cxi) as i32, 0.18), FACE_ALL);
                tint_height(&mut box_m, base_y, base_y + 0.55, rgb(54, 70, 50), rgb(150, 70, 52)); // barnacled waterline
                rust.merge(&box_m);
                obstacles.push(Obstacle { center: cc, half: Vec3::new(hx, 0.7, hz) });
            }
        }

        // 3) Raised steel-grating walkways: a few N–S catwalks over the basin (walkable platforms over Floor),
        //    railed, on pillars, with stepped landings down to the basin at each end.
        let deck_y = base_y + 2.7;
        let grate = scale_rgb(pal.floor, 0.7);
        let lanes_x = [wmin + (wmax - wmin) * 0.28, wmin + (wmax - wmin) * 0.72];
        for &lx in &lanes_x {
            let hw = 1.1;
            let (z0, z1) = (5.0 * ts, (sz - 5) as f32 * ts);
            let deck = boxx(Vec3::new(lx, deck_y, (z0 + z1) * 0.5), Vec3::new(hw, 0.12, (z1 - z0) * 0.5), grate, FACE_ALL);
            mesh.merge(&deck);
            platforms.push(Platform { x0: lx - hw, z0, x1: lx + hw, z1, top: deck_y + 0.06 });
            for side in [-hw, hw] {
                mesh.merge(&tube(Vec3::new(lx + side, deck_y + 0.5, z0), Vec3::new(lx + side, deck_y + 0.5, z1), 0.05, 5, rgb(150, 168, 172)));
            }
            let posts = ((z1 - z0) / 4.0).max(2.0) as i32;
            for p in 0..=posts {
                let pz = z0 + p as f32 / posts as f32 * (z1 - z0);
                mesh.merge(&tube(Vec3::new(lx, base_y, pz), Vec3::new(lx, deck_y, pz), 0.12, 6, scale_rgb(grate, 0.7)));
            }
            for (zr, dir) in [(z0, 1.0f32), (z1, -1.0f32)] {
                for step in 0..3 {
                    let sy = deck_y - (step as f32 + 1.0) * (deck_y - base_y) / 3.0;
                    let szc = zr + dir * (step as f32 + 1.0) * 1.3;
                    let land = boxx(Vec3::new(lx, sy, szc), Vec3::new(hw, 0.12, 0.7), grate, FACE_ALL);
                    mesh.merge(&land);
                    platforms.push(Platform { x0: lx - hw, z0: szc - 0.7, x1: lx + hw, z1: szc + 0.7, top: sy + 0.06 });
                }
            }
        }

        // 4) Props: a corroded anchor on a chain hanging between the catwalks, and rubber dock fenders.
        let mid = Vec3::new((lanes_x[0] + lanes_x[1]) * 0.5, 0.0, span * 0.5);
        let chain_top = deck_y + 0.4;
        mesh.merge(&tube(Vec3::new(mid.x, chain_top, mid.z), Vec3::new(mid.x, base_y + 1.4, mid.z), 0.04, 4, rgb(60, 56, 52)));
        let anchor_c = rgb(74, 70, 64);
        let mut anchor = tube(Vec3::new(mid.x, base_y + 0.9, mid.z), Vec3::new(mid.x, base_y + 1.5, mid.z), 0.08, 6, anchor_c);
        anchor.merge(&tube(Vec3::new(mid.x - 0.5, base_y + 0.95, mid.z), Vec3::new(mid.x + 0.5, base_y + 0.95, mid.z), 0.07, 6, anchor_c));
        for s in [-1.0f32, 1.0] {
            anchor.merge(&tube(Vec3::new(mid.x, base_y + 0.9, mid.z), Vec3::new(mid.x + s * 0.42, base_y + 0.6, mid.z), 0.07, 5, anchor_c));
        }
        mesh.merge(&anchor);
        for &lx in &lanes_x {
            let fz = span * 0.5;
            let mut fender = tube(Vec3::new(lx, deck_y - 0.3, fz), Vec3::new(lx, deck_y - 0.9, fz), 0.22, 8, rgb(28, 28, 30));
            fender.merge(&disc(Vec3::new(lx, deck_y - 0.3, fz), 0.22, 8, rgb(40, 40, 42)));
            mesh.merge(&fender);
        }

        self.platforms.extend(platforms);
        self.obstacles.extend(obstacles);
        self.meshes.push(mesh);
        self.meshes.push(rust);
    }

    /// Furnish the three-lane competitive map: a deliberate cover rhythm down each lane (cover-to-cover, woven
    /// left/right so there's always a next position), and a contested central power tower on high ground —
    /// reached by stepped ramps from both ends (or vaulted), railed on its flanks, holding a power pickup. The
    /// railings sit at deck height, so thanks to 3-D collision they wall the high ground without blocking the
    /// lane running underneath.
    fn build_triplane(&mut self) {
        if !self.cfg.triplane {
            return;
        }
        let ts = self.ts();
        let sz = self.cfg.size as f32;
        let base = self.floor_y;
        let pal = self.pal;
        let lane_x = [sz * 0.22 * ts, sz * 0.5 * ts, sz * 0.78 * ts];
        let (cx, cz) = (sz * 0.5 * ts, sz * 0.5 * ts);
        let mut mesh = Mesh3::new(scale_rgb(pal.trim, 0.9));

        // 1) Cover rhythm: a crate every few metres down each lane, alternating side for a weave, and
        //    alternating height — waist-high you can shoot over, then chest-high full cover — so engagements
        //    mix close peeks with longer holds instead of one flat sightline.
        let (lz0, lz1) = (10.0 * ts, sz * ts - 10.0 * ts);
        let mut k = 0;
        let mut z = lz0;
        while z < lz1 {
            for (li, &lx) in lane_x.iter().enumerate() {
                if li == 1 && (z - cz).abs() < 9.0 {
                    continue;
                }
                let off = if (k + li) % 2 == 0 { -1.2 } else { 1.2 };
                let tall = (k + li) % 2 == 1;
                let hy = if tall { 0.85 } else { 0.55 };
                let cc = Vec3::new(lx + off, base + hy, z);
                let cube = boxx(cc, Vec3::new(0.8, hy, 0.8), dither(pal.cover, (k * 3 + li) as i32, 0.12), FACE_ALL);
                mesh.merge(&cube);
                self.obstacles.push(Obstacle { center: cc, half: Vec3::new(0.8, hy, 0.8) });
            }
            z += 6.5;
            k += 1;
        }

        // Sightline breakers: a tall, offset cover slab on each flank where a connector meets it, so you can't
        // hold a clean spawn-to-spawn line down a flank — you have to peek and rotate. This is what turns a
        // straight corridor into a sequence of duels.
        for &cz_j in &[sz * ts / 3.0, sz * ts * 2.0 / 3.0] {
            for &(li, lx) in [(0usize, lane_x[0]), (2usize, lane_x[2])].iter() {
                let side = if li == 0 { 1.0 } else { -1.0 };
                let bc = Vec3::new(lx + side * 1.3, base + 1.1, cz_j);
                mesh.merge(&boxx(bc, Vec3::new(0.7, 1.1, 1.5), dither(scale_rgb(pal.trim, 0.9), cz_j as i32, 0.1), FACE_ALL));
                self.obstacles.push(Obstacle { center: bc, half: Vec3::new(0.7, 1.1, 1.5) });
            }
        }

        // 2) Central power tower: a raised deck on supports, the dominant high ground over mid.
        let deck_y = base + 2.3;
        let hw = 3.2;
        mesh.merge(&boxx(Vec3::new(cx, deck_y, cz), Vec3::new(hw, 0.15, hw), scale_rgb(pal.floor, 0.82), FACE_ALL));
        self.platforms.push(Platform { x0: cx - hw, z0: cz - hw, x1: cx + hw, z1: cz + hw, top: deck_y + 0.07 });
        for sx in [-1.0f32, 1.0] {
            for sz2 in [-1.0f32, 1.0] {
                mesh.merge(&tube(
                    Vec3::new(cx + sx * hw * 0.72, base, cz + sz2 * hw * 0.72),
                    Vec3::new(cx + sx * hw * 0.72, deck_y, cz + sz2 * hw * 0.72),
                    0.18,
                    6,
                    scale_rgb(pal.trim, 0.7),
                ));
            }
        }
        // stepped ramps up to the deck from north and south (also vaultable with the climb system)
        for dir in [-1.0f32, 1.0] {
            for step in 0..3 {
                let sy = base + (step as f32 + 1.0) * (deck_y - base) / 3.0;
                let szc = cz + dir * (hw + 0.8 + step as f32 * 1.45);
                mesh.merge(&boxx(Vec3::new(cx, sy, szc), Vec3::new(hw * 0.62, 0.13, 0.72), scale_rgb(pal.floor, 0.76), FACE_ALL));
                self.platforms.push(Platform { x0: cx - hw * 0.62, z0: szc - 0.72, x1: cx + hw * 0.62, z1: szc + 0.72, top: sy + 0.07 });
            }
        }
        // railings on the EAST/WEST flanks of the deck (leave N/S open for the ramps); height-local obstacles
        for ox in [-hw, hw] {
            let rc = Vec3::new(cx + ox, deck_y + 0.55, cz);
            mesh.merge(&boxx(rc, Vec3::new(0.12, 0.5, hw), scale_rgb(pal.trim, 0.85), FACE_ALL));
            self.obstacles.push(Obstacle { center: rc, half: Vec3::new(0.12, 0.5, hw) });
        }
        // a little cover on the deck so holding it isn't suicidal
        for &(ox, oz) in &[(-1.5f32, 1.5f32), (1.5, -1.5)] {
            let cc = Vec3::new(cx + ox, deck_y + 0.5, cz + oz);
            mesh.merge(&boxx(cc, Vec3::new(0.6, 0.45, 0.6), scale_rgb(pal.cover, 1.0), FACE_ALL));
            self.obstacles.push(Obstacle { center: cc, half: Vec3::new(0.6, 0.45, 0.6) });
        }

        // 3) The power pickup — the objective that makes the high ground worth contesting — on a glowing pad.
        let kind = crate::pickups::PickupKind::Armor;
        self.custom_pickups.push((Vec3::new(cx, deck_y + 0.08, cz), kind));
        let mut pad = disc(Vec3::new(cx, deck_y + 0.1, cz), 0.5, 12, kind.color());
        pad.emissive = 0.55;
        mesh.merge(&pad);

        self.meshes.push(mesh);
    }

    /// Stamp the functional-space plan: for each planned room, lay down the coherent structure its function
    /// calls for, at that room's actual floor height (so cover sits on graph-raised towers and sunken trenches
    /// correctly, never floating or buried). This replaces scattered cover with designed places — and because
    /// each prop is a single mesh+collision box, what you see is exactly what you take cover behind and stand
    /// on. Skips a ring near each room's edge implicitly via the stamper's footprint margin, preserving the
    /// doorways the corridors connect to.
    fn build_spaces(&mut self) {
        if self.space_plan.is_empty() {
            return;
        }
        let ts = self.ts();
        let pal = self.pal;
        let plan = std::mem::take(&mut self.space_plan);
        for (i, (rect, function)) in plan.iter().enumerate() {
            let (tcx, tcz) = rect.center();
            let (cx, cz) = (tcx as f32 * ts, tcz as f32 * ts);
            let base = self.floor_h(cx, cz); // the room's real elevation after assign_heights
            let hw = (rect.w() as f32 * ts) * 0.5;
            let hh = (rect.h() as f32 * ts) * 0.5;
            let seed = self.cfg.seed.wrapping_add(i as u64 * 0x9E37).wrapping_mul(2_654_435_761) as u32;
            let (obs, meshes) = crate::spaces::stamp(*function, cx, cz, hw, hh, base, pal, seed);
            self.obstacles.extend(obs);
            for m in meshes {
                self.meshes.push(m);
            }
        }
    }

    /// Scatter a few destructible objects through the map — concrete barriers, crate stacks and the occasional
    /// glass pane — placed in rooms away from the spawn. They block and can be stood on like any prop until a
    /// few shots or a grenade reduce them to rubble, at which point the cover (or the sightline, for glass) is
    /// gone. Their meshes are emitted live each frame by the game, not baked here.
    fn build_destructibles(&mut self, rng: &mut Rng, rects: &[Rect]) {
        use crate::destructible::{Destructible, DestructibleKind};
        let ts = self.ts();
        let mut count = 0;
        for (i, r) in rects.iter().enumerate().skip(1) {
            if count >= 14 || r.w() < 5 || r.h() < 5 {
                continue;
            }
            if rng.f32() > 0.55 {
                continue;
            }
            let (tcx, tcz) = r.center();
            let base = self.floor_h(tcx as f32 * ts, tcz as f32 * ts);
            // offset from the room centre so it reads as cover, not a centrepiece
            let ox = (rng.f32() - 0.5) * (r.w() as f32 * ts) * 0.4;
            let oz = (rng.f32() - 0.5) * (r.h() as f32 * ts) * 0.4;
            let c = Vec3::new(tcx as f32 * ts + ox, 0.0, tcz as f32 * ts + oz);
            let roll = rng.f32();
            let seed = self.cfg.seed.wrapping_add(i as u64 * 131) as u32;
            let d = if roll < 0.5 {
                Destructible::new(DestructibleKind::Barrier, Vec3::new(c.x, base + 0.6, c.z), Vec3::new(1.1, 0.6, 0.35), seed)
            } else if roll < 0.85 {
                Destructible::new(DestructibleKind::CrateStack, Vec3::new(c.x, base + 0.55, c.z), Vec3::new(0.6, 0.55, 0.6), seed)
            } else {
                Destructible::new(DestructibleKind::GlassPane, Vec3::new(c.x, base + 1.0, c.z), Vec3::new(1.0, 1.0, 0.08), seed)
            };
            self.destructibles.push(d);
            count += 1;
        }
    }

    /// On open-sky biomes, lay one or two ground-hugging jetstreams sweeping across the map — gently curved
    /// polylines you can step into to be carried along (the doc's lower-atmosphere air currents). Each is a
    /// flow line with a ride radius and a streamer ribbon floated near the ground.
    fn build_air_currents(&mut self, rng: &mut Rng) {
        if !self.cfg.theme.open_sky() || matches!(self.cfg.theme, MapTheme::Station | MapTheme::Carrier) {
            return;
        }
        let ts = self.ts();
        let span = self.w as f32 * ts;
        let count = if self.w > 200 { 2 } else { 1 };
        for c in 0..count {
            let horiz = (c % 2 == 0) ^ (rng.f32() < 0.5);
            let base = 0.28 + rng.f32() * 0.44; // offset across the perpendicular axis
            let segs = 6;
            let mut path = Vec::with_capacity(segs + 1);
            for i in 0..=segs {
                let t = i as f32 / segs as f32;
                let wobble = (t * std::f32::consts::TAU * 1.3 + c as f32 * 2.0).sin() * span * 0.06;
                let along = 4.0 * ts + t * (span - 8.0 * ts);
                let across = (base * span + wobble).clamp(ts, span - ts);
                let p = if horiz { Vec2::new(along, across) } else { Vec2::new(across, along) };
                path.push(p);
            }
            let mid = path[segs / 2];
            let y = self.floor_h(mid.x, mid.y) + 1.4;
            self.air_currents.push(AirCurrent { path, radius: 4.5, strength: 16.0, y });
        }
        self.build_air_current_meshes();
    }

    /// Visual streamer ribbons for each air current: a chain of faint, drifting translucent panels following
    /// the flow line near the ground, with little arrow chevrons so the flow direction reads at a glance.
    fn build_air_current_meshes(&mut self) {
        let mut mesh = Mesh3::new(rgb(180, 210, 235));
        for ac in &self.air_currents {
            for w in ac.path.windows(2) {
                let (a, b) = (w[0], w[1]);
                let dir = Vec2::new(b.x - a.x, b.y - a.y);
                let len = (dir.x * dir.x + dir.y * dir.y).sqrt().max(1e-3);
                let (dx, dz) = (dir.x / len, dir.y / len);
                let perp = Vec2::new(-dz, dx);
                let steps = (len / 1.6).max(1.0) as i32;
                for s in 0..steps {
                    let t = (s as f32 + 0.5) / steps as f32;
                    let cx = a.x + dir.x * t;
                    let cz = a.y + dir.y * t;
                    // a small chevron pointing downstream, made of two thin angled quads
                    for sign in [-1.0f32, 1.0] {
                        let tip = Vec3::new(cx + dx * 0.5, ac.y, cz + dz * 0.5);
                        let tail = Vec3::new(cx - dx * 0.3 + perp.x * 0.5 * sign, ac.y, cz - dz * 0.3 + perp.y * 0.5 * sign);
                        let tail2 = Vec3::new(tail.x, ac.y + 0.12, tail.z);
                        let tip2 = Vec3::new(tip.x, ac.y + 0.12, tip.z);
                        mesh.push_quad(tail, tip, tip2, tail2, Vec3::UP, rgb(200, 225, 245));
                    }
                }
            }
        }
        if mesh.tri_count() > 0 {
            mesh.emissive = 0.25;
            set_alpha(&mut mesh, 90);
            self.meshes.push(mesh);
        }
    }

    /// A translucent water/lava sheet sitting just below the lowest walkable floor. It shows through pits
    /// and sunken areas, reading as standing water (Mistfall) or a magma pool (Volcanic).
    fn build_water(&mut self) {
        let (col, alpha, emissive, level_drop) = match self.cfg.theme {
            MapTheme::Mistfall => (rgb(40, 92, 110), 150u8, 0.0, 0.5),
            MapTheme::Volcanic => (rgb(210, 90, 30), 235, 0.6, 0.9),
            MapTheme::Toxic => (rgb(110, 150, 40), 200, 0.25, 0.5),
            _ => return,
        };
        let y = self.floor_y + self.min_h() - level_drop;
        self.hazard_y = y;
        self.hazard_lava = matches!(self.cfg.theme, MapTheme::Volcanic);
        self.has_hazard = true;
        self.hazard_dps = match self.cfg.theme {
            MapTheme::Volcanic => 26.0,
            MapTheme::Toxic => 15.0,
            _ => 0.0, // Mistfall water just drags you
        };
        let span = self.cfg.size as f32 * self.ts();
        // Hand the surface to the animated water system (tessellated with traveling waves each frame).
        self.water.push(WaterBody {
            cx: span * 0.5,
            cz: span * 0.5,
            radius: span * 0.5,
            y,
            col,
            alpha,
            reflect: if self.hazard_lava { 0.12 } else { 0.6 },
            emissive,
            lava: self.hazard_lava,
            disc: false,
        });
    }

    /// Give each room a discrete elevation tier, ramp the corridors between tiers by relaxation, and add
    /// gentle rolling noise on the outdoor biomes. Walkable tiles stay a continuous surface (no cliffs
    /// between neighbours), so movement just follows the floor — drama comes at wall/pit edges.
    fn assign_heights(&mut self, cfg: &MapConfig, rng: &mut Rng, rects: &[Rect]) {
        // Agent-simulated maps bring their own full emergent elevation field — use it as the terrain directly
        // (the agents' hills and worn lanes), with a couple of relax passes so every slope stays climbable.
        // Three-lane competitive maps want flat, readable ground — verticality comes from the built power
        // tower, not the terrain — so skip the tiering entirely and leave the floor level.
        if self.cfg.triplane {
            return;
        }
        if !self.agent_height.is_empty() {
            self.heights = self.agent_height.clone();
            // one light smoothing pass so slopes stay climbable without melting the agents' hills flat
            let mut next = self.heights.clone();
            for z in 1..self.h - 1 {
                for x in 1..self.w - 1 {
                    if !self.cell(x, z).walkable() {
                        continue;
                    }
                    let i = (z * self.w + x) as usize;
                    let s = self.tile_h(x - 1, z) + self.tile_h(x + 1, z) + self.tile_h(x, z - 1) + self.tile_h(x, z + 1);
                    next[i] = self.heights[i] * 0.65 + (s / 4.0) * 0.35;
                }
            }
            self.heights = next;
            return;
        }
        let amp = match cfg.theme {
            MapTheme::Bunker => 0.0, // tight, flat, claustrophobic
            MapTheme::Industrial | MapTheme::Warehouse => 1.0,
            MapTheme::Urban | MapTheme::Military => 0.7,
            _ => 0.9,
        };
        // Raised tiers are capped so a player still clears the uniform ceiling; sunken lanes can go deeper.
        let tiers = [-2.2, 0.0, 0.0, 1.1, 1.7];

        // Mark room tiles as fixed anchors and stamp each room's tier. Graph-skinned maps use the elevation
        // their topology was scored with (towers high, trenches low) instead of a random tier.
        let gelev = self.graph_elev.clone();
        let mut anchor = vec![false; (self.w * self.h) as usize];
        for (i, r) in rects.iter().enumerate() {
            let tier = if i == 0 {
                0.0
            } else if let Some(&ge) = gelev.get(i) {
                if ge.is_finite() {
                    ge // a graph-marked tower/trench: use its decisive elevation
                } else {
                    tiers[rng.i32_range(0, tiers.len() as i32) as usize] * amp // normal varied tier
                }
            } else {
                tiers[rng.i32_range(0, tiers.len() as i32) as usize] * amp
            };
            for z in r.z0..r.z1 {
                for x in r.x0..r.x1 {
                    if self.in_bounds(x, z) {
                        let idx = (z * self.w + x) as usize;
                        self.heights[idx] = tier;
                        anchor[idx] = true;
                    }
                }
            }
        }

        // Relax corridor (non-anchor) floor tiles toward the average of their walkable neighbours a few
        // times → smooth ramps that connect the tiers.
        for _ in 0..10 {
            let mut next = self.heights.clone();
            for z in 0..self.h {
                for x in 0..self.w {
                    let idx = (z * self.w + x) as usize;
                    if anchor[idx] || !self.cell(x, z).walkable() {
                        continue;
                    }
                    let mut sum = 0.0;
                    let mut n = 0.0;
                    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        if self.cell(x + dx, z + dz).walkable() {
                            sum += self.tile_h(x + dx, z + dz);
                            n += 1.0;
                        }
                    }
                    if n > 0.0 {
                        next[idx] = sum / n;
                    }
                }
            }
            self.heights = next;
        }

        // Outdoor rolling ground: layered value noise added on top. Open-sky wilds get a broad low-frequency
        // octave (real hills + valleys, the vertical spread that makes a landscape) plus finer detail; built
        // biomes keep it subtle so cover sits level.
        let (broad_amp, fine_amp) = match cfg.theme {
            MapTheme::Canopy => (2.2, 0.7),
            MapTheme::Toxic => (1.9, 0.6),
            MapTheme::Mistfall => (1.6, 0.5),
            MapTheme::Alien => (1.8, 0.7),
            MapTheme::Volcanic => (1.4, 0.6),
            MapTheme::Ice => (1.5, 0.45),
            MapTheme::Station => (0.0, 0.0),
            MapTheme::Carrier => (0.0, 0.0),
            MapTheme::Meadow => (2.4, 0.6),
            MapTheme::Redwood => (2.0, 0.8),
            MapTheme::Urban => (0.0, 0.22),
            _ => (0.0, 0.0),
        };
        if broad_amp > 0.0 || fine_amp > 0.0 {
            let s = (cfg.seed & 0x7fff_ffff) as i32;
            for z in 0..self.h {
                for x in 0..self.w {
                    let idx = (z * self.w + x) as usize;
                    if !self.cell(x, z).walkable() {
                        continue;
                    }
                    // broad hills (very low frequency) shape the land; two finer octaves add surface texture.
                    let broad = vnoise(x as f32 * 0.035, z as f32 * 0.035, s ^ 0x2B);
                    let fine = vnoise(x as f32 * 0.17, z as f32 * 0.17, s) + 0.45 * vnoise(x as f32 * 0.42, z as f32 * 0.42, s ^ 0x55);
                    self.heights[idx] += broad * broad_amp + fine * fine_amp;
                }
            }
        }

        // Deliberate high ground on the wilds: broad knolls, the odd deep hollow, and one dominant mesa that
        // gives a plunging sightline over the bowl — verticality that makes crossing the open ground perilous.
        if cfg.theme.open_sky() && !matches!(cfg.theme, MapTheme::Station | MapTheme::Carrier) {
            // Edge berm: the ground rises toward the map border so the play space sits in a bowl ringed by
            // higher ground (which the cliffs then crown) — no flat edge, and a high rim to fight over.
            let margin = (self.w as f32 * 0.20).max(8.0);
            let berm = 5.5;
            for z in 0..self.h {
                for x in 0..self.w {
                    if !self.cell(x, z).walkable() {
                        continue;
                    }
                    let ed = (x.min(self.w - 1 - x)).min(z.min(self.h - 1 - z)) as f32;
                    if ed < margin {
                        let t = 1.0 - ed / margin; // 0 interior → 1 at border
                        self.heights[(z * self.w + x) as usize] += t * t * berm;
                    }
                }
            }
            let knolls = 3 + rng.i32_range(0, 3); // 3–5 features
            for k in 0..knolls {
                let cx = rng.i32_range(self.w / 6, self.w * 5 / 6) as f32;
                let cz = rng.i32_range(self.h / 6, self.h * 5 / 6) as f32;
                // the first feature is a dominant mesa (big + tall); the rest are knolls/hollows
                let mesa = k == 0;
                let rad = if mesa { 16.0 + rng.f32() * 10.0 } else { 8.0 + rng.f32() * 9.0 };
                let hollow = !mesa && rng.chance(0.3);
                let hgt = if mesa {
                    6.5 + rng.f32() * 3.0
                } else {
                    (4.0 + rng.f32() * 4.0) * if hollow { -0.8 } else { 1.0 }
                };
                for z in 0..self.h {
                    for x in 0..self.w {
                        if !self.cell(x, z).walkable() {
                            continue;
                        }
                        let d = (((x as f32 - cx).powi(2) + (z as f32 - cz).powi(2)).sqrt()) / rad;
                        if d < 1.0 {
                            // mesa gets a flatter top (plateau) via a steeper shoulder; knolls stay rounded
                            let f = if mesa {
                                (0.5 * (1.0 + (d.powf(1.8) * std::f32::consts::PI).cos())).clamp(0.0, 1.0)
                            } else {
                                0.5 * (1.0 + (d * std::f32::consts::PI).cos())
                            };
                            self.heights[(z * self.w + x) as usize] += hgt * f;
                        }
                    }
                }
            }
        }

        // ── ADS-compliant smoothing pass ────────────────────────────────────────────────────────────────
        // Raw value-noise leaves high-frequency micro-bumps that make the camera stutter and snag the player
        // as they walk. We low-pass the walkable heightfield so deliberate features (hills, mesas, berms,
        // room tiers) survive but the jittery surface noise is flattened — and we bias *downward* (toward the
        // lowest neighbour) so we carve clean shelves rather than build awkward little bumps to trip on.
        if cfg.theme.open_sky() && !matches!(cfg.theme, MapTheme::Station | MapTheme::Carrier) {
            for _ in 0..3 {
                let mut next = self.heights.clone();
                for z in 0..self.h {
                    for x in 0..self.w {
                        let idx = (z * self.w + x) as usize;
                        if !self.cell(x, z).walkable() {
                            continue;
                        }
                        let here = self.heights[idx];
                        let mut sum = 0.0;
                        let mut n = 0.0;
                        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                            if self.cell(x + dx, z + dz).walkable() {
                                sum += self.heights[((z + dz) * self.w + (x + dx)) as usize];
                                n += 1.0;
                            }
                        }
                        if n == 0.0 {
                            continue;
                        }
                        let avg = sum / n;
                        let bump = here - avg;
                        // Shave down small bumps sticking up (the stutter-causing combat noise); leave dells
                        // (cover) and large intended steps untouched. Downward bias = clean shelves, no new bumps.
                        if bump > 0.05 && bump < 1.6 {
                            next[idx] = here * 0.35 + avg * 0.65;
                        }
                    }
                }
                self.heights = next;
            }

            // ── Prone Ledge Hardener ────────────────────────────────────────────────────────────────────
            // At sharp drops (mesa rims, knoll shoulders) carve a flat 1-tile bench just back from the lip, so
            // a player can drop into it and go prone with a stable horizon — a real sniper perch instead of a
            // rounded-off hill. We keep the lip sharp and only flatten the shelf directly behind it.
            let mut benched = self.heights.clone();
            for z in 1..self.h - 1 {
                for x in 1..self.w - 1 {
                    if !self.cell(x, z).walkable() {
                        continue;
                    }
                    let idx = (z * self.w + x) as usize;
                    let here = self.heights[idx];
                    // find the steepest downhill neighbour (the drop the lip faces)
                    let mut drop = 0.0f32;
                    let mut ddir = (0i32, 0i32);
                    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        if self.cell(x + dx, z + dz).walkable() {
                            let nh = self.heights[((z + dz) * self.w + (x + dx)) as usize];
                            if here - nh > drop {
                                drop = here - nh;
                                ddir = (dx, dz);
                            }
                        }
                    }
                    // a real ledge → flatten the tile one step *back* from the lip to the lip height
                    if drop > 2.2 {
                        let (bx, bz) = (x - ddir.0, z - ddir.1);
                        if self.cell(bx, bz).walkable() {
                            benched[((bz) * self.w + (bx)) as usize] = here;
                        }
                    }
                }
            }
            self.heights = benched;

            // ── Walled Garden Carver + Geometry Socket Localizer ────────────────────────────────────────
            // Route a shallow, bermed approach from spawn to the centerpiece (a covered lane that pulls
            // players toward the objective without a minimap), then flatten and grade a pad under the
            // centerpiece itself so it sits flush in the earth instead of floating on the noisy heightfield.
            if !rects.is_empty() {
                let (mcx, mcz) = (self.w as f32 * 0.5, self.h as f32 * 0.5);
                // central room = where the landmark will sit; spawn = room 0
                let mut central = rects[0];
                let mut bd = f32::MAX;
                for r in rects.iter().skip(1) {
                    let (cx, cz) = r.center();
                    let d = (cx as f32 - mcx).powi(2) + (cz as f32 - mcz).powi(2);
                    if d < bd {
                        bd = d;
                        central = *r;
                    }
                }
                let (lx, lz) = central.center();
                let (sx, sz) = rects[0].center();
                let (lxf, lzf, sxf, szf) = (lx as f32, lz as f32, sx as f32, sz as f32);
                // Walled Garden: lower walkable tiles near the spawn→landmark segment into a gentle trench.
                let seg = (lxf - sxf, lzf - szf);
                let seg_len2 = (seg.0 * seg.0 + seg.1 * seg.1).max(1.0);
                let width = 3.0;
                let depth = 1.3;
                for z in 0..self.h {
                    for x in 0..self.w {
                        if !self.cell(x, z).walkable() {
                            continue;
                        }
                        // distance from this tile to the approach segment
                        let t = (((x as f32 - sxf) * seg.0 + (z as f32 - szf) * seg.1) / seg_len2).clamp(0.0, 1.0);
                        let (px, pz) = (sxf + seg.0 * t, szf + seg.1 * t);
                        let d = (((x as f32 - px).powi(2) + (z as f32 - pz).powi(2)).sqrt()) / width;
                        if d < 1.0 {
                            let f = 0.5 * (1.0 + (d * std::f32::consts::PI).cos()); // 1 on the line → 0 at the berm
                            self.heights[(z * self.w + x) as usize] -= depth * f;
                        }
                    }
                }
                // Geometry Socket: flatten a pad under the centerpiece to the landmark tile's height, grading out.
                let pad_h = self.tile_h(lx, lz);
                let rad = 5.0;
                for z in 0..self.h {
                    for x in 0..self.w {
                        if !self.cell(x, z).walkable() {
                            continue;
                        }
                        let d = (((x - lx).pow(2) + (z - lz).pow(2)) as f32).sqrt() / rad;
                        if d < 1.0 {
                            // flat in the core, smoothly grading back to the natural ground at the rim
                            let t = (d * d).clamp(0.0, 1.0);
                            let idx = (z * self.w + x) as usize;
                            self.heights[idx] = pad_h * (1.0 - t) + self.heights[idx] * t;
                        }
                    }
                }
            }
        }

        // Culvert Breach: sink the southern trench a metre-plus below the northern court, grading across the
        // wall line so the breach reads as a short ramp up out of the flooded bottom tier into the court.
        if cfg.culvert {
            let cz = self.h / 2;
            let (lo, hi) = ((cz as f32) - 2.0, (cz as f32) + 2.0);
            for z in 0..self.h {
                let f = (((z as f32) - lo) / (hi - lo)).clamp(0.0, 1.0);
                let t = -1.4 * f;
                for x in 0..self.w {
                    self.heights[(z * self.w + x) as usize] = t;
                }
            }
        }
    }

    /// Drop a memorable signature structure at the most central room: a stepped plinth and a tall lit
    /// beacon tower, themed per biome (a reactor core, a watchtower, a monument, a ruined totem…). It
    /// anchors sightlines, gives the map a recognisable heart, and — as a blocked footprint — splits the
    /// central room into lanes you flow around.
    fn build_landmark(&mut self) {
        if self.rooms.len() < 2 {
            return;
        }
        if self.cfg.culvert {
            return; // the breached wall built in build_culvert is the centerpiece
        }
        if self.cfg.docks {
            return; // the docks build its own anchor/walkway centerpiece
        }
        if self.cfg.triplane {
            return; // the central power tower is the centerpiece (built in build_triplane)
        }
        let mcx = self.w as f32 * 0.5 * self.ts();
        let mcz = self.h as f32 * 0.5 * self.ts();
        let mut best = 1;
        let mut bestd = f32::MAX;
        for (i, r) in self.rooms.iter().enumerate().skip(1) {
            let d = (r.center.x - mcx).powi(2) + (r.center.z - mcz).powi(2);
            if d < bestd {
                bestd = d;
                best = i;
            }
        }
        let c = self.rooms[best].center;
        let pal = self.pal;
        let theme = self.cfg.theme;
        let top = self.ceil_y - 0.4; // world-Y the structure can rise to under the uniform ceiling
        let seed = (self.cfg.seed & 0xffff) as u32;

        // Each biome family gets a distinct silhouette, but the specific centerpiece rotates with the seed so
        // two Industrial maps don't share the same heart.
        let pick = self
            .cfg
            .landmark
            .map(|l| l as usize)
            .unwrap_or((seed.wrapping_mul(2_654_435_761) >> 13) as usize);
        let (meshes, obstacles) = match theme {
            MapTheme::Industrial | MapTheme::Bunker | MapTheme::Warehouse => match pick % 9 {
                0 => lm_silo_breach(c, pal, top, seed),
                1 => lm_tesla(c, pal, top, seed),
                2 => lm_generator_pit(c, pal, top, seed),
                3 => lm_clone_vats(c, pal, top, seed),
                4 => lm_reactor(c, pal, top),
                5 => lm_ore_cart(c, pal, top, seed),
                6 => lm_dam(c, pal, top, seed),
                7 => lm_railway(c, pal, seed),
                _ => lm_breach_wall(c, pal, top, seed),
            },
            MapTheme::Urban | MapTheme::Military => match pick % 14 {
                0 => lm_crashed_cargo(c, pal, seed),
                1 => lm_atomic_bus(c, pal, seed),
                2 => lm_killpit(c, pal, seed),
                3 => lm_manor(c, pal, top, seed),
                4 => lm_backlot(c, pal, seed),
                5 => lm_obelisk(c, pal, top, seed),
                6 => lm_colosseum(c, pal, top, seed),
                7 => lm_watchtower(c, pal, top),
                8 => lm_fallen_dome(c, pal, top, seed),
                9 => lm_dry_fountain(c, pal, top, seed),
                10 => lm_derailed_car(c, pal, top, seed),
                11 => lm_fountain(c, pal, seed),
                12 => lm_railway(c, pal, seed),
                _ => lm_breach_wall(c, pal, top, seed),
            },
            MapTheme::Volcanic => match pick % 5 {
                0 => lm_smelting(c, pal, top, seed),
                1 => lm_silo_breach(c, pal, top, seed),
                2 => lm_dam(c, pal, top, seed),
                3 => lm_cliff(c, pal, seed),
                _ => lm_reactor(c, pal, top),
            },
            MapTheme::Ice => match pick % 6 {
                0 => lm_bifrost(c, pal, top, seed),
                1 => lm_monolith(c, pal, top, seed),
                2 => lm_cryo_tank(c, pal, top, seed),
                3 => lm_ice_floes(c, pal, seed),
                4 => lm_cliff(c, pal, seed),
                _ => lm_totem(c, pal),
            },
            MapTheme::Station => match pick % 2 {
                0 => lm_comms(c, pal, top),
                _ => lm_cooling_vat(c, pal, top, seed),
            },
            MapTheme::Carrier => lm_watchtower(c, pal, top),
            MapTheme::Alien | MapTheme::Canopy | MapTheme::Mistfall | MapTheme::Toxic | MapTheme::Meadow | MapTheme::Redwood => match pick % 13 {
                0 => lm_totem(c, pal),
                1 => lm_stone_circle(c, pal, seed),
                2 => lm_great_log(c, pal, seed),
                3 => lm_waterfall(c, pal, seed),
                4 => lm_ribcage(c, pal, top, seed),
                5 => lm_dragon(c, pal, top, seed),
                6 => lm_monolith(c, pal, top, seed),
                7 => lm_siege_camp(c, pal, top, seed),
                8 => lm_watchtower_ring(c, pal, top, seed),
                9 => lm_gel_core(c, pal, top, seed),
                10 => lm_cliff(c, pal, seed),
                11 => lm_fountain(c, pal, seed),
                _ => lm_dangling_gondola(c, pal, top, seed),
            },
        };
        // Varied landmark scale: roll a size from the seed so the same landmark feels small one map, colossal
        // the next. Open-sky biomes have no ceiling so they can tower; built biomes are capped under the roof.
        let open = theme.open_sky();
        let sroll = crate::mesh::hash01((seed as i32).wrapping_mul(7919), 31); // 0..1
        let s = if open { 0.75 + sroll * 1.7 } else { 0.85 + sroll * 0.5 };
        let pivot = c; // scale about the landmark's base
        for mut m in meshes {
            if (s - 1.0).abs() > 0.02 {
                for v in &mut m.verts {
                    *v = pivot + (*v - pivot) * s;
                }
            }
            self.meshes.push(m);
        }
        for mut o in obstacles {
            if (s - 1.0).abs() > 0.02 {
                o.center.y = pivot.y + (o.center.y - pivot.y) * s;
                o.center.x = pivot.x + (o.center.x - pivot.x) * s;
                o.center.z = pivot.z + (o.center.z - pivot.z) * s;
                o.half = o.half * s;
            }
            self.obstacles.push(o);
        }
    }

    /// Scatter cover crates and carve the occasional pit. Cover becomes both an `Obstacle` and a mesh.
    fn decorate(&mut self, cfg: &MapConfig, rng: &mut Rng, rects: &[Rect]) {
        let ts = self.ts();
        for (i, r) in rects.iter().enumerate() {
            // Room 0 is the player spawn — leave it clear so you never start inside cover.
            if i == 0 {
                continue;
            }
            // crates: roughly cover_density per room, kept off the centre spawn point
            let n = (cfg.cover_density * 0.6 + rng.f32() * cfg.cover_density) as i32;
            for _ in 0..n {
                let tx = rng.i32_range(r.x0 + 1, (r.x1 - 1).max(r.x0 + 1));
                let tz = rng.i32_range(r.z0 + 1, (r.z1 - 1).max(r.z0 + 1));
                if self.cell(tx, tz) != Cell::Floor {
                    continue;
                }
                let c = self.tile_center(tx, tz);
                let hgt = 0.7 + rng.f32() * 0.9;
                self.obstacles.push(Obstacle {
                    center: Vec3::new(c.x, c.y + hgt, c.z),
                    half: Vec3::new(ts * 0.38, hgt, ts * 0.38),
                });
            }
            // pit: skip room 0 (player spawn) so you don't start over a hole
            if i != 0 && rng.chance(cfg.pit_chance) && r.w() >= 6 && r.h() >= 6 {
                let px0 = r.x0 + 2;
                let pz0 = r.z0 + 2;
                let px1 = (px0 + rng.i32_range(2, r.w() - 2)).min(r.x1 - 2);
                let pz1 = (pz0 + rng.i32_range(2, r.h() - 2)).min(r.z1 - 2);
                for z in pz0..pz1 {
                    for x in px0..px1 {
                        // Only ever carve a pit into actual floor. With non-rectangular rooms the rect bounds
                        // overlap carved-away Wall/Solid; converting those to Void would punch a hole through
                        // the boundary and let the player fall out of the world. Floor-only keeps pits sealed.
                        if self.cell(x, z) == Cell::Floor {
                            self.set(x, z, Cell::Void);
                        }
                    }
                }
            }
        }
    }

    /// Strew 1–2 hand-authored story tableaus through the map's rooms (away from spawn and the central
    /// landmark), choosing vignettes that fit the biome. They drop real cover obstacles, so they read as
    /// authored set-dressing AND play as micro-arenas. Connectivity is preserved by only placing in rooms
    /// large enough that a small prop can't seal them.
    fn scatter_vignettes(&mut self, rng: &mut Rng) {
        if self.rooms.len() < 3 {
            return;
        }
        let pal = self.pal;
        let open = self.cfg.theme.open_sky();
        let want = 1 + (rng.f32() * 2.0) as usize; // 1–2
        let mut placed = 0;
        let mut tries = 0;
        let mut used: Vec<usize> = Vec::new();
        while placed < want && tries < 24 {
            tries += 1;
            let ri = 1 + (rng.next_u64() as usize % (self.rooms.len() - 1));
            if used.contains(&ri) {
                continue;
            }
            let rect = self.rooms[ri].rect;
            if rect.w() < 5 || rect.h() < 5 {
                continue; // too cramped — a prop would choke it
            }
            let (cx, cz) = rect.center();
            let p = self.tile_center(cx, cz);
            let base = Vec3::new(p.x, self.tile_h(cx, cz), p.y);
            let seed = (ri as u32).wrapping_mul(0x9E37_79B9) ^ self.cfg.seed as u32;
            // Pick a vignette suited to the biome: graves/camps/drops fit the wilds; last stands & drops fit
            // built/military ground; everything can show a grave.
            let roll = rng.i32_range(0, 4);
            let (meshes, obs) = if open {
                match roll {
                    0 => vig_cold_camp(base, pal, seed),
                    1 => vig_grave(base, pal, seed),
                    2 => vig_supply_drop(base, pal, seed),
                    _ => vig_last_stand(base, pal, seed),
                }
            } else {
                match roll {
                    0 => vig_last_stand(base, pal, seed),
                    1 => vig_supply_drop(base, pal, seed),
                    2 => vig_grave(base, pal, seed),
                    _ => vig_cold_camp(base, pal, seed),
                }
            };
            self.meshes.extend(meshes);
            self.obstacles.extend(obs);
            used.push(ri);
            placed += 1;
        }
    }

    /// Strew small abandoned-place detritus across the floor: the discarded trash, shattered glass, oil
    /// slicks, scattered paper, loose cable and tipped-over containers that (per the mapmaking notes) is what
    /// actually tricks the eye into reading a space as lived-in and abandoned. Purely cosmetic and ground-
    /// hugging — nothing here blocks movement, so navigation is unchanged. Themed per biome; spawn room is
    /// left clean so the player doesn't start in a junk pile.
    fn scatter_detritus(&mut self, rng: &mut Rng) {
        if self.rooms.is_empty() {
            return;
        }
        let pal = self.pal;
        let (junk_c, paper_c, puddle_c, frozen) = match self.cfg.theme {
            MapTheme::Industrial | MapTheme::Bunker | MapTheme::Warehouse => (rgb(120, 86, 60), rgb(150, 146, 132), rgb(28, 26, 30), false),
            MapTheme::Urban | MapTheme::Military => (rgb(110, 104, 96), rgb(176, 170, 156), rgb(34, 32, 30), false),
            MapTheme::Volcanic => (rgb(60, 50, 48), rgb(96, 86, 80), rgb(40, 22, 16), false),
            MapTheme::Ice => (rgb(150, 178, 198), rgb(196, 214, 228), rgb(150, 195, 220), true),
            MapTheme::Station | MapTheme::Carrier => (rgb(96, 104, 116), rgb(150, 156, 166), rgb(26, 30, 36), false),
            _ => (scale_rgb(pal.rock, 0.8), scale_rgb(pal.floor, 1.1), scale_rgb(pal.floor, 0.5), false),
        };
        let spawn = self.rooms[0].rect;
        let mut junk = Mesh3::new(junk_c);
        let mut wet = Mesh3::new(puddle_c);
        let n = (self.w * self.h / 90).clamp(12, 90);
        for k in 0..n {
            let x = rng.i32_range(1, self.w - 1);
            let z = rng.i32_range(1, self.h - 1);
            if self.cell(x, z) != Cell::Floor {
                continue;
            }
            if x >= spawn.x0 - 1 && x < spawn.x1 + 1 && z >= spawn.z0 - 1 && z < spawn.z1 + 1 {
                continue; // keep spawn room + doorstep clear
            }
            let p = self.tile_center(x, z);
            let gy = self.tile_h(x, z);
            let base = Vec3::new(p.x, gy, p.y);
            match rng.i32_range(0, 6) {
                0 => {
                    junk.merge(&crate::fsm::detail::debris(base, 0.7, 4 + (rng.f32() * 4.0) as u32, junk_c, k as u32 * 17 + 3));
                }
                1 => {
                    let dir = Vec3::new((k as f32).cos(), 0.0, (k as f32).sin());
                    junk.merge(&crate::fsm::detail::shards(base, dir, 5, k as u32 * 7 + 1));
                }
                2 => {
                    for j in 0..3 {
                        let a = rng.f32() * std::f32::consts::TAU;
                        let pp = Vec3::new(base.x + a.cos() * 0.5, gy + 0.015, base.z + a.sin() * 0.5);
                        junk.merge(&boxx(pp, Vec3::new(0.12, 0.005, 0.16), dither(paper_c, j + k, 0.1), FACE_TOP));
                    }
                }
                3 => {
                    let mut pud = disc(Vec3::new(base.x, gy + 0.02, base.z), 0.6 + rng.f32() * 0.5, 14, puddle_c);
                    set_alpha(&mut pud, if frozen { 150 } else { 120 });
                    pud.reflect = if frozen { 0.5 } else { 0.65 };
                    wet.merge(&pud);
                }
                4 => {
                    let len = 0.35 + rng.f32() * 0.25;
                    let cc = Vec3::new(base.x, gy + 0.18, base.z);
                    junk.merge(&tube(Vec3::new(cc.x - len, cc.y, cc.z), Vec3::new(cc.x + len, cc.y, cc.z), 0.18, 8, scale_rgb(junk_c, 1.2)));
                }
                _ => {
                    let a = Vec3::new(base.x - 0.3, gy + 0.05, base.z - 0.2);
                    let b = Vec3::new(base.x + 0.2, gy + 0.05, base.z + 0.3);
                    junk.merge(&tube(a, b, 0.04, 4, rgb(30, 30, 34)));
                }
            }
        }
        if junk.tri_count() > 0 {
            self.meshes.push(junk);
        }
        if wet.tri_count() > 0 {
            self.meshes.push(wet);
        }
    }

    /// Collapse the roof over a fraction of the rooms on built (ceiling-bound) biomes so sky and daylight
    /// pour in — the maps read as bombed-out / weathered structures with open courtyards instead of sealed
    /// dungeons. Big rooms become open courtyards; on top of that an irregular "torn-roof" lattice opens
    /// patches across the rest of the floor (streets/plazas that aren't registered rooms), so every built
    /// map gets real sky exposure, not just the ones whose floor is mostly rooms. Open-sky biomes skip this.
    fn plan_skylights(&mut self, rng: &mut Rng) {
        self.skylights.clear();
        if self.cfg.theme.open_sky() || self.rooms.is_empty() {
            return;
        }
        // 1) Big room openings, biased toward the larger rooms (halls → courtyards).
        let mut idx: Vec<usize> = (0..self.rooms.len()).collect();
        idx.sort_by_key(|&i| -(self.rooms[i].rect.w() * self.rooms[i].rect.h()));
        let want = ((self.rooms.len() as f32 * 0.45).ceil() as usize).max(1);
        for &i in idx.iter() {
            if self.skylights.len() >= want {
                break;
            }
            let big = self.skylights.len() < want / 2;
            if big || rng.f32() < 0.7 {
                let r = self.rooms[i].rect.inset(1);
                if r.w() >= 2 && r.h() >= 2 {
                    self.skylights.push(r);
                }
            }
        }
        // 2) Torn-roof lattice over the rest: on a coarse grid, open a small patch wherever the floor extends,
        //    skipping some cells for an irregular collapsed-roof silhouette. Guarantees sky on street/plaza
        //    maps whose open ground isn't carved as rooms.
        let stride = 5;
        for gz in (1..self.h - 2).step_by(stride) {
            for gx in (1..self.w - 2).step_by(stride) {
                // jitter the patch a touch so the lattice doesn't read as a regular grid
                let jx = gx + (rng.f32() * 2.0) as i32;
                let jz = gz + (rng.f32() * 2.0) as i32;
                if self.cell(jx, jz) == Cell::Floor && rng.f32() < 0.72 {
                    let s = if rng.f32() < 0.35 { 3 } else { 2 };
                    self.skylights.push(Rect { x0: jx, z0: jz, x1: (jx + s).min(self.w - 1), z1: (jz + s).min(self.h - 1) });
                }
            }
        }
    }

    /// Turn the grid into themed low-poly geometry, logscope-style: a dithered floor + ceiling per tile,
    /// wall faces only where a floor borders non-floor (with a `tint_height` ambient-occlusion gradient and
    /// an emissive accent strip), hanging ceiling lamps and inlaid floor seams that actually motivate the
    /// lighting, and biome-specific cover props — crates, faceted rock, or glowing crystal clusters.
    pub fn build_meshes(&mut self) {
        let ts = self.ts();
        let pal = self.pal;
        let theme = self.cfg.theme;
        let open_sky = theme.open_sky();
        let y0 = self.floor_y;
        let y1 = self.ceil_y;

        let mut floor = Mesh3::new(pal.floor);
        // Ice/snow reads as a bright, faintly reflective sheet catching the sun; everything else is matte.
        if matches!(theme, MapTheme::Ice) {
            floor.reflect = 0.42;
        }
        let ceil = Mesh3::new(pal.ceil); // always empty now — no overhead lid; kept as a stable meshes[1] slot
        let mut walls = Mesh3::new(pal.wall);
        let mut trim = Mesh3::new(pal.trim); // diffuse structural band near the wall top
        let mut arch_glow = Mesh3::new(pal.accent).emit(0.8); // ceiling lamps + skylights (practical light)
        let lamp_col = lerp_c(pal.accent, rgb(255, 250, 240), 0.65);

        let wall_base_y = y0 + self.min_h() - 0.4; // skirt so walls meet sunken floors with no gaps

        // No overhead lid: every map now reads as open-sky (a real skybox renders behind the geometry —
        // see Game::frame_meshes / FpsApp::clear_color) rather than every level being capped by a flat
        // ceiling plane. `self.ceil_y` is kept purely as a numeric clearance bound for procedural gen
        // (structure heights, light-shaft placement, etc.) — nothing renders there anymore.

        for z in 0..self.h {
            for x in 0..self.w {
                if self.cell(x, z) != Cell::Floor {
                    continue;
                }
                let (wx, wz) = ((x as f32) * ts, (z as f32) * ts);
                let key = x * 31 + z * 17;
                // elevated floor quad from the four tile-corner heights → continuous, ramped surface. A coarse
                // organic patch value (sampled on a 3-tile hash grid) replaces the old strict checkerboard so
                // the floor doesn't self-tile at tile scale — the checker was the biggest repetition signal
                // the scenic/variety scorer flagged across every map.
                let patch = 0.88 + 0.12 * crate::mesh::hash01((x / 3).wrapping_mul(13) ^ (z / 3).wrapping_mul(7), 0x51);
                let fc = scale_rgb(dither(pal.floor, key, 0.10), patch);
                // Subdivide each tile into an N×N grid (procgen texture resolution) so the per-vertex
                // dither/tint reads as finer surface detail at higher settings.
                let nd = tex_subdiv();
                let inv = 1.0 / nd as f32;
                for sz in 0..nd {
                    for sx in 0..nd {
                        let (u0, u1) = (sx as f32 * inv, (sx + 1) as f32 * inv);
                        let (v0, v1) = (sz as f32 * inv, (sz + 1) as f32 * inv);
                        let lerp_h = |u: f32, v: f32| {
                            let a = self.corner_h(x, z) * (1.0 - u) + self.corner_h(x + 1, z) * u;
                            let b = self.corner_h(x, z + 1) * (1.0 - u) + self.corner_h(x + 1, z + 1) * u;
                            a * (1.0 - v) + b * v
                        };
                        let p00 = Vec3::new(wx + ts * u0, y0 + lerp_h(u0, v0), wz + ts * v0);
                        let p10 = Vec3::new(wx + ts * u1, y0 + lerp_h(u1, v0), wz + ts * v0);
                        let p11 = Vec3::new(wx + ts * u1, y0 + lerp_h(u1, v1), wz + ts * v1);
                        let p01 = Vec3::new(wx + ts * u0, y0 + lerp_h(u0, v1), wz + ts * v1);
                        let fnorm = (p10 - p00).cross(p01 - p00).normalized();
                        // Texture variation: large-scale value noise creates lighter/darker patches across
                        // the floor on top of the per-tile dither, so big surfaces don't read as flat.
                        let nx = (wx + ts * (u0 + u1) * 0.5) * 0.07;
                        let nz = (wz + ts * (v0 + v1) * 0.5) * 0.07;
                        let n = vnoise(nx, nz, self.cfg.seed as i32 ^ 0x51ED);
                        let mut varied = scale_rgb(fc, 0.82 + n * 0.34);
                        // On the wilds, shade the forest floor by the same thicket/glade field the scatter
                        // uses: glades read as brighter, grassier ground; thickets as darker leaf-shadow.
                        // Metal decks (Station/Carrier) are open-sky but get no forest shading.
                        if open_sky && !matches!(self.cfg.theme, MapTheme::Station | MapTheme::Carrier) {
                            let cxw = wx + ts * (u0 + u1) * 0.5;
                            let czw = wz + ts * (v0 + v1) * 0.5;
                            let d = (vnoise(cxw * 0.02, czw * 0.02, self.cfg.seed as i32 ^ 0x7A1D) + 0.5).clamp(0.0, 1.0);
                            varied = scale_rgb(varied, 1.14 - d * 0.42); // bright glade → dim thicket floor
                            // worn footpath: blend toward bare trodden dirt within the path width
                            let td = self.trail_dist(cxw, czw);
                            if td < 3.0 {
                                let dirt = lerp_c(pal.floor, rgb(104, 84, 58), 0.7);
                                let k = (1.0 - (td / 3.0)).clamp(0.0, 1.0);
                                varied = lerp_c(varied, dirt, k * 0.85);
                            }
                            // seasonal ground: a frosted snow dusting in winter, a warm tint in autumn
                            // (the station deck is in orbit — no weather, so it's exempt)
                            if self.cfg.theme != MapTheme::Station {
                                match self.season() {
                                    3 => varied = lerp_c(varied, rgb(228, 234, 242), 0.6),
                                    2 => varied = lerp_c(varied, rgb(150, 110, 60), 0.22),
                                    _ => {}
                                }
                            }
                        }
                        let sub_c = dither(varied, key.wrapping_add(sx * 7 + sz * 13), 0.06);
                        floor.push_quad(p00, p10, p11, p01, fnorm, sub_c);
                    }
                }
                // Open sky overhead everywhere now — no flat ceiling lid, no bulkhead fillers, no ceiling
                // lamps. Indoor-reading rooms get their light from wall sconces / prop glow instead (see
                // the sconce pass below); the skybox shows through above every tile.
                // wall faces toward each non-floor neighbour; normal points INTO this tile (toward player).
                // Bases sit on the skirt line so the wall covers any drop to a sunken floor.
                let edges = [
                    ((-1, 0), Vec3::new(1.0, 0.0, 0.0), [Vec3::new(wx, wall_base_y, wz), Vec3::new(wx, wall_base_y, wz + ts)]),
                    ((1, 0), Vec3::new(-1.0, 0.0, 0.0), [Vec3::new(wx + ts, wall_base_y, wz + ts), Vec3::new(wx + ts, wall_base_y, wz)]),
                    ((0, -1), Vec3::new(0.0, 0.0, 1.0), [Vec3::new(wx + ts, wall_base_y, wz), Vec3::new(wx, wall_base_y, wz)]),
                    ((0, 1), Vec3::new(0.0, 0.0, -1.0), [Vec3::new(wx, wall_base_y, wz + ts), Vec3::new(wx + ts, wall_base_y, wz + ts)]),
                ];
                for ((dx, dz), n, base) in edges {
                    let nb = self.cell(x + dx, z + dz);
                    if nb == Cell::Floor || nb == Cell::Void {
                        continue; // open between two floor tiles (or to a pit) — no wall
                    }
                    if open_sky || self.on_boundary(x + dx, z + dz) {
                        continue; // map edge terminates in cliffs/boundary assets, not a flat wall
                    }
                    let b0 = base[0];
                    let b1 = base[1];
                    // Per-zone tone: a coarse regional hash so the map isn't one flat grey field. Adjacent
                    // areas read as different bays/wings.
                    let zone = dither(pal.wall, (x / 5) * 23 + (z / 5) * 41, 0.14);
                    // Dado split: a darker lower course (wainscot) under a lighter field, so the wall has a
                    // horizontal line instead of a single blank plane.
                    let dado = (wall_base_y + 1.05).min(y1 - 0.6);
                    let lower = scale_rgb(zone, 0.72);
                    // Subdivide each band into a small grid instead of one giant quad. A single quad only has
                    // shading computed at its 4 corners and Gouraud-interpolates between them, so specular
                    // glints/fresnel rim/reflections wash out into one flat linear gradient across a whole
                    // 2m×3m panel — this is a big part of what makes flat-shaded surfaces read as "toaster
                    // Doom" instead of something with real material response. More vertices means the PBR
                    // terms in the renderer actually get evaluated at more points across the face.
                    push_quad_grid(&mut walls, b0, b1, b1.with_y(dado), b0.with_y(dado), n, lower, 1, 1, key);
                    push_quad_grid(&mut walls, b0.with_y(dado), b1.with_y(dado), b1.with_y(y1), b0.with_y(y1), n, zone, 2, 1, key.wrapping_add(97));
                    // Pilaster: a protruding vertical column at intervals, breaking the flat run. A wider
                    // capital caps it near the top.
                    if key % 3 == 0 {
                        let m = (b0 + b1) * 0.5;
                        let colh = (y1 - wall_base_y) * 0.5;
                        let along = Vec3::new((b1.x - b0.x).abs() * 0.24, 0.0, (b1.z - b0.z).abs() * 0.24);
                        let depth = Vec3::new(n.x.abs() * 0.16, 0.0, n.z.abs() * 0.16);
                        let pc = Vec3::new(m.x + n.x * 0.12, wall_base_y + colh, m.z + n.z * 0.12);
                        let half = Vec3::new(along.x + depth.x + 0.12, colh, along.z + depth.z + 0.12);
                        walls.merge(&boxx(pc, half, lerp_c(zone, rgb(255, 255, 255), 0.05), FACE_NO_BOTTOM));
                        // capital
                        let cap = Vec3::new(pc.x, wall_base_y + colh * 2.0 - 0.18, pc.z);
                        walls.merge(&boxx(cap, Vec3::new(half.x + 0.07, 0.12, half.z + 0.07), scale_rgb(pal.trim, 0.9), FACE_ALL));
                    }
                    // structural trim band near the top (diffuse, not glowing)
                    let ta = y1 - 0.55;
                    let tb = y1 - 0.32;
                    trim.push_quad(b0.with_y(ta), b1.with_y(ta), b1.with_y(tb), b0.with_y(tb), n, pal.trim);
                    // Wall sconce: a small warm emissive fixture on a coarse interval — the interior light
                    // source that used to be hanging ceiling lamps, now that there's no ceiling to hang from.
                    if key % 7 == 3 {
                        let m = (b0 + b1) * 0.5;
                        let sy = wall_base_y + (y1 - wall_base_y) * 0.62;
                        let sc = Vec3::new(m.x + n.x * 0.06, sy, m.z + n.z * 0.06);
                        let sh = Vec3::new((n.z.abs() * 0.22).max(0.08), 0.1, (n.x.abs() * 0.22).max(0.08));
                        arch_glow.merge(&boxx(sc, sh, lamp_col, FACE_ALL));
                    }
                }
            }
        }

        // Ambient occlusion by height — multiplicative so the per-zone wall tones and pilaster relief survive
        // (darker and grimier at the floor, brightening toward the ceiling light) instead of flattening to grey.
        shade_ao(&mut walls, wall_base_y, y1, 0.5, 1.12);
        tint_up(&mut trim, lerp_c(pal.trim, rgb(255, 255, 255), 0.12), scale_rgb(pal.trim, 0.6));

        // Structural greebles — ceiling beams across each room and, on the open-sky biomes, a translucent
        // glowing skylight panel under the ceiling. All of this sits above head height, so it changes the
        // look without touching the collision grid.
        let mut deco = Mesh3::new(scale_rgb(pal.wall, 0.55));
        let beam_col = scale_rgb(pal.trim, 0.5);
        let by = y1 - 0.22;
        for room in &self.rooms {
            if open_sky {
                break; // no ceiling overhead in the wilds — nothing to hang beams/skylights from
            }
            let rx0 = room.rect.x0 as f32 * ts;
            let rz0 = room.rect.z0 as f32 * ts;
            let rx1 = room.rect.x1 as f32 * ts;
            let rz1 = room.rect.z1 as f32 * ts;
            let (cxw, czw) = ((rx0 + rx1) * 0.5, (rz0 + rz1) * 0.5);
            let (wspan, dspan) = (rx1 - rx0, rz1 - rz0);
            if wspan >= dspan {
                let count = ((wspan / (ts * 3.0)).floor() as i32).clamp(1, 4);
                for i in 0..count {
                    let fx = rx0 + wspan * (i as f32 + 0.5) / count as f32;
                    deco.merge(&boxx(Vec3::new(fx, by, czw), Vec3::new(0.18, 0.16, dspan * 0.5), beam_col, FACE_ALL));
                }
            } else {
                let count = ((dspan / (ts * 3.0)).floor() as i32).clamp(1, 4);
                for i in 0..count {
                    let fz = rz0 + dspan * (i as f32 + 0.5) / count as f32;
                    deco.merge(&boxx(Vec3::new(cxw, by, fz), Vec3::new(wspan * 0.5, 0.16, 0.18), beam_col, FACE_ALL));
                }
            }
            if matches!(theme, MapTheme::Urban | MapTheme::Ice | MapTheme::Alien) {
                let s = ts * 1.1;
                let sy = y1 - 0.05;
                let sk = Color32::from_rgba_unmultiplied(pal.sky.r(), pal.sky.g(), pal.sky.b(), 150);
                arch_glow.push_quad(
                    Vec3::new(cxw - s, sy, czw + s),
                    Vec3::new(cxw + s, sy, czw + s),
                    Vec3::new(cxw + s, sy, czw - s),
                    Vec3::new(cxw - s, sy, czw - s),
                    Vec3::new(0.0, -1.0, 0.0),
                    sk,
                );
            }
        }

        // Cover props — biome-specific. `props` is diffuse, `prop_glow` self-illuminates (crystal/lava),
        // `stone_props` collects the homogeneous rock/cairn props so they can carry a stone texture.
        let mut props = Mesh3::new(pal.cover);
        let mut prop_glow = Mesh3::new(pal.accent).emit(theme.cover_emissive().max(0.9));
        let mut stone_props = Mesh3::new(pal.rock);
        for o in &self.obstacles {
            build_prop(theme, &pal, o, &mut props, &mut prop_glow, &mut stone_props);
        }
        if stone_props.tri_count() > 0 {
            stone_props.apply_material(crate::mesh::Material::StoneBlock, self.cfg.seed as u32 ^ 0xF104);
            stone_props.material = Some(crate::mesh::Material::StoneBlock);
        }

        let _ = (FACE_ALL, FACE_TOP); // kept available for props
        // Procedural material grain on the big surfaces — fine world-space fBm so floors and walls read as
        // worn concrete/stone/earth instead of flat tinted tiles. Continuous across tiles (world-space noise).
        // Biome materials give floors and walls distinct surface character — brick vs concrete vs ribbed steel
        // vs stone blocks vs snow — instead of one shared fBm mottle. Per-seed choice adds variety between maps
        // of the same biome.
        let tseed = self.cfg.seed as u32;
        let (floor_mat, wall_mat) = self.cfg.theme.materials(self.cfg.seed);
        floor.apply_material(floor_mat, tseed ^ 0xF100);
        walls.apply_material(wall_mat, tseed ^ 0xF101);
        // Texture both the floor and the walls with their theme's atlas material. (Walls were previously left
        // smooth to avoid a busy look, but that was also masking the chunk bug that dropped materials; with
        // materials + UVs preserved through chunking, walls now carry their surface at the tuned tile sizes.)
        floor.material = Some(floor_mat);
        walls.material = Some(wall_mat);
        // Trim caps are part of the wall architecture — surface them with the same material so ledges and
        // pillar caps read as the same stone/brick/steel as the wall they sit on rather than a flat band.
        trim.apply_material(wall_mat, tseed ^ 0xF102);
        trim.material = Some(wall_mat);
        // The grid walls/trim are generated with correct single-face outward winding (only the exposed face of
        // each border cell is emitted), so they're safe to backface-cull — skips the far side of every wall as
        // you look across a room. The floor is up-facing (never a backface) so flagging it is a no-op but
        // harmless; deco/props carry their own flags from their primitive constructors.
        walls.single_sided = true;
        trim.single_sided = true;
        self.meshes = vec![floor, ceil, walls, trim, deco, props, arch_glow, prop_glow, stone_props];
        // Daylight pours through the larger roof openings: a soft shaft + a bright floor pool at each, so a
        // skylight reads as a deliberate light source, not just a hole. Capped, and only for sizable openings.
        if !self.skylights.is_empty() {
            let shaft_col = lerp_c(pal.sky, rgb(255, 252, 235), 0.5);
            let mut shafts: Vec<Mesh3> = Vec::new();
            let mut placed = 0;
            for r in &self.skylights {
                if placed >= 14 {
                    break;
                }
                if (r.w() * r.h()) < 6 {
                    continue; // skip tiny patches
                }
                let (cx, cz) = r.center();
                let wx = cx as f32 * ts + ts * 0.5;
                let wz = cz as f32 * ts + ts * 0.5;
                let fy = self.tile_h(cx, cz);
                shafts.extend(crate::mesh::light_shaft(wx, wz, y1 + 0.2, fy, shaft_col, placed as u32));
                placed += 1;
            }
            self.meshes.extend(shafts);
        }
    }

    /// On the open biomes, dress the clearings as a logscope grove: leaning trees with leafy crowns
    /// (trunks block movement), coloured light shafts piercing the canopy, and flat stepping stones.
    /// Trees are placed on open floor away from the spawn and become trunk obstacles.
    /// Generate a meandering worn footpath across the open-sky wilds: a smooth sine-perturbed line crossing
    /// the map. Stored as world-XZ control points; the floor tints to dirt along it and the scatter clears.
    fn build_trail(&mut self, rng: &mut Rng) {
        if !self.cfg.theme.open_sky() {
            return;
        }
        let ts = self.ts();
        let wspan = self.w as f32 * ts;
        let hspan = self.h as f32 * ts;
        let horiz = rng.chance(0.5);
        let cross = if horiz { hspan } else { wspan };
        let along = if horiz { wspan } else { hspan };
        let amp = cross * (0.10 + rng.f32() * 0.14);
        let (p1, p2) = (rng.f32() * 6.28, rng.f32() * 6.28);
        let base = cross * (0.35 + rng.f32() * 0.30); // offset the centre line so it isn't dead-centre
        let n = 44;
        let mut pts = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / (n - 1) as f32;
            let meander = (t * 6.28 * 1.4 + p1).sin() * amp + (t * 6.28 * 3.1 + p2).sin() * amp * 0.4;
            let c = (base + meander).clamp(ts * 1.5, cross - ts * 1.5);
            let a = (t * along).clamp(0.0, along);
            pts.push(if horiz { Vec2::new(a, c) } else { Vec2::new(c, a) });
        }
        self.trail = pts;
    }

    /// Distance (world units) from a ground point to the nearest segment of the footpath; large if none.
    pub fn trail_dist(&self, wx: f32, wz: f32) -> f32 {
        if self.trail.len() < 2 {
            return f32::MAX;
        }
        let p = Vec2::new(wx, wz);
        let mut best = f32::MAX;
        for w in self.trail.windows(2) {
            best = best.min(seg_dist(p, w[0], w[1]));
        }
        best
    }

    /// Carve a meandering stream/sludge channel on water biomes: a second meander, crossing the map roughly
    /// perpendicular to nowhere in particular, whose bed is dipped into the terrain so water collects there.
    fn build_stream(&mut self, rng: &mut Rng) {
        if !matches!(self.cfg.theme, MapTheme::Mistfall | MapTheme::Toxic) {
            return;
        }
        let ts = self.ts();
        let wspan = self.w as f32 * ts;
        let hspan = self.h as f32 * ts;
        let horiz = rng.chance(0.5);
        let cross = if horiz { hspan } else { wspan };
        let along = if horiz { wspan } else { hspan };
        let amp = cross * (0.12 + rng.f32() * 0.16);
        let (p1, p2) = (rng.f32() * 6.28, rng.f32() * 6.28);
        let base = cross * (0.30 + rng.f32() * 0.40);
        let n = 48;
        let mut pts = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / (n - 1) as f32;
            let meander = (t * 6.28 * 1.7 + p1).sin() * amp + (t * 6.28 * 3.7 + p2).sin() * amp * 0.45;
            let c = (base + meander).clamp(ts * 1.5, cross - ts * 1.5);
            let a = (t * along).clamp(0.0, along);
            pts.push(if horiz { Vec2::new(a, c) } else { Vec2::new(c, a) });
        }
        self.stream = pts;
        // Dip the bed: lower every walkable tile near the channel, smoothly falling to the centre line.
        let half = 3.4_f32;
        let depth = 1.3_f32;
        for z in 0..self.h {
            for x in 0..self.w {
                if !self.cell(x, z).walkable() {
                    continue;
                }
                let (wx, wz) = ((x as f32 + 0.5) * ts, (z as f32 + 0.5) * ts);
                let d = self.stream_dist(wx, wz);
                if d < half {
                    let k = 1.0 - (d / half); // 1 at centre → 0 at bank
                    let idx = (z * self.w + x) as usize;
                    self.heights[idx] -= depth * (k * k); // quadratic basin
                }
            }
        }
    }

    /// Distance from a ground point to the nearest stream segment; large if none.
    pub fn stream_dist(&self, wx: f32, wz: f32) -> f32 {
        if self.stream.len() < 2 {
            return f32::MAX;
        }
        let p = Vec2::new(wx, wz);
        let mut best = f32::MAX;
        for w in self.stream.windows(2) {
            best = best.min(seg_dist(p, w[0], w[1]));
        }
        best
    }

    /// Carve a few still ponds into the open biomes: round basins dipped into the terrain, away from the
    /// spawn and any stream, that the grove fills with a reflective sheet and a reedy fringe.
    fn build_ponds(&mut self, rng: &mut Rng) {
        if !self.cfg.theme.open_sky() {
            return;
        }
        let ts = self.ts();
        let n = 1 + rng.i32_range(0, 3); // 1–3 ponds
        let span = self.w as f32 * ts;
        for _ in 0..n {
            for _try in 0..14 {
                let cx = rng.f32() * span * 0.7 + span * 0.15;
                let cz = rng.f32() * span * 0.7 + span * 0.15;
                let r = 4.0 + rng.f32() * 5.0;
                // keep ponds off the spawn room and apart from the stream
                let near_spawn = self.rooms.first().map(|rm| (Vec2::new(cx, cz).dist(Vec2::new(rm.center.x, rm.center.z))) < r + 8.0).unwrap_or(false);
                if near_spawn || self.stream_dist(cx, cz) < r + 4.0 {
                    continue;
                }
                // dip a smooth bowl
                let (tx, tz) = ((cx / ts) as i32, (cz / ts) as i32);
                let rad_t = (r / ts) as i32 + 2;
                for dz in -rad_t..=rad_t {
                    for dx in -rad_t..=rad_t {
                        let (x, z) = (tx + dx, tz + dz);
                        if !self.in_bounds(x, z) || !self.cell(x, z).walkable() {
                            continue;
                        }
                        let d = Vec2::new((x as f32 + 0.5) * ts, (z as f32 + 0.5) * ts).dist(Vec2::new(cx, cz)) / r;
                        if d < 1.0 {
                            let k = 1.0 - d;
                            self.heights[(z * self.w + x) as usize] -= 1.6 * (k * k);
                        }
                    }
                }
                self.ponds.push((Vec2::new(cx, cz), r));
                break;
            }
        }
    }

    /// Signed distance to the nearest pond edge (negative inside the water); large if none.
    pub fn pond_dist(&self, wx: f32, wz: f32) -> f32 {
        let p = Vec2::new(wx, wz);
        let mut best = f32::MAX;
        for &(c, r) in &self.ponds {
            best = best.min(p.dist(c) - r);
        }
        best
    }

    /// Carve N straight parallel **lanes** across the map for a tower-defense layout: each is a clear running
    /// path through dense jungle, with raised platform "tower" pads at intervals and cross-connectors so the
    /// lanes interlink. The lanes are stored as centre-lines so the grove keeps trees out of them.
    fn build_lanes(&mut self, _rng: &mut Rng) {
        let n = self.cfg.lanes.max(1) as i32;
        let ts = self.ts();
        let span = self.w as f32 * ts;
        let margin = span * 0.12;
        // lanes run along +Z, evenly spaced in X
        for i in 0..n {
            let x = margin + (span - 2.0 * margin) * (i as f32 + 0.5) / n as f32;
            let mut pts = Vec::new();
            let steps = 10;
            for s in 0..=steps {
                let z = margin + (span - 2.0 * margin) * s as f32 / steps as f32;
                // gentle serpentine so lanes aren't dead straight
                let wobble = ((z * 0.01) + i as f32 * 1.3).sin() * (span * 0.02);
                pts.push(Vec2::new(x + wobble, z));
            }
            // raised tower pads along the lane (every few nodes): a low platform dome
            for s in (2..steps).step_by(3) {
                let p = pts[s];
                self.raise_pad(p.x, p.y, 3.2, 1.1);
            }
            self.lane_paths.push(pts);
        }
        // cross-connectors: short rungs linking neighbouring lanes at a couple of heights
        let mut rungs: Vec<Vec<Vec2>> = Vec::new();
        for i in 0..n - 1 {
            let a = &self.lane_paths[i as usize];
            let b = &self.lane_paths[i as usize + 1];
            for &k in &[3usize, 7] {
                if k < a.len() && k < b.len() {
                    rungs.push(vec![a[k], b[k]]);
                }
            }
        }
        self.lane_paths.extend(rungs);
    }

    /// Raise a smooth circular platform (tower pad) into the heightfield at (cx,cz).
    fn raise_pad(&mut self, cx: f32, cz: f32, r: f32, h: f32) {
        let ts = self.ts();
        let rad_t = (r / ts) as i32 + 1;
        let (tx, tz) = ((cx / ts) as i32, (cz / ts) as i32);
        for dz in -rad_t..=rad_t {
            for dx in -rad_t..=rad_t {
                let (x, z) = (tx + dx, tz + dz);
                if !self.in_bounds(x, z) || !self.cell(x, z).walkable() {
                    continue;
                }
                let d = Vec2::new((x as f32 + 0.5) * ts, (z as f32 + 0.5) * ts).dist(Vec2::new(cx, cz)) / r;
                if d < 1.0 {
                    let k = 0.5 - 0.5 * (d * std::f32::consts::PI).cos(); // raised-cosine plateau
                    self.heights[(z * self.w + x) as usize] += h * (1.0 - k);
                }
            }
        }
    }

    /// Distance from a ground point to the nearest lane centre-line; large if there are no lanes.
    pub fn lane_dist(&self, wx: f32, wz: f32) -> f32 {
        if self.lane_paths.is_empty() {
            return f32::MAX;
        }
        let p = Vec2::new(wx, wz);
        let mut best = f32::MAX;
        for lane in &self.lane_paths {
            for w in lane.windows(2) {
                best = best.min(seg_dist(p, w[0], w[1]));
            }
        }
        best
    }

    /// Season rolled from the seed: 0 spring, 1 summer, 2 autumn, 3 winter. Drives foliage colour + snow.
    pub fn season(&self) -> u8 {
        ((self.cfg.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 30) % 4) as u8
    }


    /// litter, understory bushes, slender mid trees, a few hero trees, scattered boulders and fallen logs,
    /// and (open-sky biomes) drifting light shafts — all seated on the rolling terrain and sqrt-distributed
    /// for even areal density, so the organic maps read as a real, lived-in wood rather than a carved arena.
    /// Geometry is merged into a handful of big meshes so the painter's per-tri far-cull keeps it affordable.
    fn build_grove(&mut self, rng: &mut Rng, _rects: &[Rect]) {
        let theme = self.cfg.theme;
        if !matches!(
            theme,
            MapTheme::Volcanic | MapTheme::Ice | MapTheme::Alien | MapTheme::Canopy | MapTheme::Mistfall | MapTheme::Toxic | MapTheme::Meadow | MapTheme::Redwood
        ) {
            return;
        }
        let open = theme.open_sky();
        let prof = wilds_profile(theme);
        // Season rolls per seed (spring/summer look the same baseline; autumn warms the canopy; winter frosts
        // everything and kills the flowers). Applied to every leaf, bush, blade, and litter colour below.
        let season = self.season();
        let seasonify = |c: Color32| -> Color32 {
            match season {
                2 => lerp_c(c, rgb(216, 132, 52), 0.5),  // autumn gold/rust
                3 => lerp_c(c, rgb(226, 232, 240), 0.52), // winter frost
                _ => c,
            }
        };
        let flowers_ok = prof.flowers && season < 2;
        // Per-biome palettes: trunk, a few crown greens, ground-litter colours, understory, boulder tint, and
        // the coloured light-shaft hues.
        let (wood, crowns, litter, under, shafts): (Color32, &[Color32], &[Color32], &[Color32], [Color32; 2]) = match theme {
            MapTheme::Alien => (
                rgb(74, 54, 36),
                &[rgb(96, 140, 52), rgb(120, 168, 70), rgb(150, 120, 200)],
                &[rgb(60, 96, 40), rgb(78, 120, 50), rgb(150, 120, 200), rgb(96, 140, 52)],
                &[rgb(72, 110, 48), rgb(110, 90, 160)],
                [rgb(150, 214, 120), rgb(120, 200, 150)],
            ),
            MapTheme::Canopy => (
                rgb(91, 66, 40),
                &[rgb(108, 144, 64), rgb(132, 168, 78), rgb(200, 150, 80), rgb(190, 100, 110)],
                &[rgb(96, 128, 58), rgb(120, 150, 70), rgb(170, 140, 70), rgb(150, 110, 64)],
                &[rgb(96, 128, 58), rgb(130, 150, 72)],
                [rgb(230, 194, 84), rgb(200, 120, 110)],
            ),
            MapTheme::Mistfall => (
                rgb(54, 70, 74),
                &[rgb(120, 170, 168), rgb(96, 150, 156), rgb(150, 200, 200)],
                &[rgb(70, 110, 110), rgb(96, 140, 140), rgb(120, 160, 158)],
                &[rgb(84, 128, 126), rgb(110, 152, 150)],
                [rgb(154, 208, 220), rgb(120, 180, 220)],
            ),
            MapTheme::Toxic => (
                rgb(70, 62, 40),
                &[rgb(140, 170, 60), rgb(110, 150, 50), rgb(176, 200, 80)],
                &[rgb(96, 120, 48), rgb(130, 160, 60), rgb(150, 110, 50)],
                &[rgb(110, 140, 54), rgb(140, 170, 64)],
                [rgb(176, 230, 90), rgb(150, 200, 70)],
            ),
            MapTheme::Ice => (
                rgb(120, 122, 132),
                &[rgb(200, 225, 240), rgb(176, 206, 230), rgb(220, 235, 248)],
                &[rgb(206, 220, 234), rgb(228, 238, 248), rgb(180, 200, 220)],
                &[rgb(196, 216, 232), rgb(170, 196, 220)],
                [rgb(150, 200, 255), rgb(205, 228, 255)],
            ),
            MapTheme::Volcanic => (
                rgb(42, 34, 30),
                &[rgb(126, 70, 40), rgb(96, 54, 36), rgb(150, 80, 44)],
                &[rgb(60, 44, 38), rgb(40, 30, 28), rgb(120, 60, 36)],
                &[rgb(72, 48, 38), rgb(96, 56, 40)],
                [rgb(255, 140, 70), rgb(255, 100, 50)],
            ),
            MapTheme::Meadow => (
                rgb(96, 74, 46),
                &[rgb(120, 168, 80), rgb(150, 190, 96), rgb(200, 210, 110)],
                &[rgb(120, 160, 70), rgb(150, 185, 88), rgb(240, 210, 110), rgb(220, 150, 170)],
                &[rgb(120, 160, 72), rgb(150, 185, 90)],
                [rgb(250, 235, 150), rgb(220, 230, 170)],
            ),
            MapTheme::Redwood => (
                rgb(120, 64, 42),
                &[rgb(72, 104, 56), rgb(90, 120, 66), rgb(110, 96, 60)],
                &[rgb(70, 56, 40), rgb(96, 78, 52), rgb(110, 80, 56)],
                &[rgb(76, 100, 58), rgb(96, 116, 66)],
                [rgb(200, 170, 110), rgb(150, 180, 120)],
            ),
            _ => return,
        };

        // Collect walkable ground, then scale every layer's count to the available area.
        let mut tiles: Vec<(i32, i32)> = Vec::new();
        for z in 0..self.h {
            for x in 0..self.w {
                if self.cell(x, z) == Cell::Floor {
                    tiles.push((x, z));
                }
            }
        }
        if tiles.is_empty() {
            return;
        }
        // Soften leaf-blade density globally while we build the (very dense) scatter, so the software painter
        // stays affordable; restore afterwards.
        let saved_detail = crate::mesh::detail();
        crate::mesh::set_detail(0.6);
        let ts = self.ts();
        let area = tiles.len() as f32;
        let pick = |rng: &mut Rng, pal: &[Color32]| pal[(rng.next_u64() as usize) % pal.len()];
        // a random walkable world point, with a small sub-tile jitter
        let spot = |rng: &mut Rng, tiles: &[(i32, i32)]| -> (f32, f32) {
            let (tx, tz) = tiles[(rng.next_u64() as usize) % tiles.len()];
            let ox = (rng.f32() - 0.5) * ts;
            let oz = (rng.f32() - 0.5) * ts;
            ((tx as f32 + 0.5) * ts + ox, (tz as f32 + 0.5) * ts + oz)
        };
        // A low-frequency density field: woods aren't uniform — this carves natural thickets (dense stands)
        // and glades (open clearings) so sightlines and cover vary across the map. 0 = clearing, 1 = thicket.
        let dseed = (self.cfg.seed as i32) ^ 0x7A1D;
        let dens = move |wx: f32, wz: f32| -> f32 { (vnoise(wx * 0.02, wz * 0.02, dseed) + 0.5).clamp(0.0, 1.0) };

        // Merge everything into a few big meshes; the painter culls per-triangle past the fog wall.
        let mut flora = Mesh3::new(crowns[0]);
        let mut bark = Mesh3::new(wood);
        let mut glow = Mesh3::new(shafts[0]).emit(0.85);
        // Soft contact shadows pooled on the ground under the bigger objects — a cheap but convincing
        // grounding cue in a renderer with no shadow map. Merged into one translucent mesh.
        let mut shadows = Mesh3::new(rgb(8, 10, 12));
        let mut tree_obstacles = 0usize;

        // ── LAYER 1 — ground litter: hundreds of flat blades/leaves/pebbles hugging the terrain. ──────────
        let litter_n = ((area * 0.8 * prof.litter) as i32).min(1500);
        let flower_cols = [rgb(235, 90, 90), rgb(245, 220, 90), rgb(230, 230, 240), rgb(180, 120, 220), rgb(240, 150, 90)];
        for k in 0..litter_n {
            let (wx, wz) = spot(rng, &tiles);
            if self.trail_dist(wx, wz) < 1.4 && rng.chance(0.8) {
                continue; // mostly bare trodden earth on the path
            }
            let y = self.floor_h(wx, wz) + 0.02;
            // flowering biomes pop the odd bright bloom standing slightly proud of the grass
            if flowers_ok && rng.chance(0.14) {
                let fc = flower_cols[(rng.next_u64() as usize) % flower_cols.len()];
                let mut bloom = disc(Vec3::new(wx, y + 0.12 + rng.f32() * 0.12, wz), 0.07 + rng.f32() * 0.06, 5, fc);
                bloom.emissive = 0.12;
                flora.merge(&bloom);
                // a thin green stem
                let mut stem = disc(Vec3::new(wx, y + 0.04, wz), 0.03, 4, pick(rng, under));
                scale_xz(&mut stem, 0.4, 0.4);
                flora.merge(&stem);
                continue;
            }
            let r = 0.14 + rng.f32() * 0.24;
            let mut blade = disc(Vec3::new(wx, y, wz), r, 5, seasonify(pick(rng, litter)));
            scale_xz(&mut blade, 1.0, 0.5 + rng.f32() * 0.6); // elongate → leaf/blade, not a coin
            flora.merge(&blade);
            let _ = k;
        }

        // ── LAYER 2 — understory: low bushes / ferns / grass clumps, thicker inside thickets. ─────────────
        let under_n = ((area / 11.0 * prof.understory) as i32).min(240);
        for _ in 0..under_n {
            let (wx, wz) = spot(rng, &tiles);
            if self.trail_dist(wx, wz) < 1.8 || self.stream_dist(wx, wz) < 2.2 || self.pond_dist(wx, wz) < 0.6 || self.lane_dist(wx, wz) < 3.5 {
                continue;
            }
            if rng.f32() > 0.28 + dens(wx, wz) * 0.72 {
                continue; // sparse in glades, lush in thickets
            }
            let y = self.floor_h(wx, wz);
            match prof.under_kind {
                UnderKind::Grass => {
                    // a tall tuft of slim blades — reads as field grass
                    let h = 0.45 + rng.f32() * 0.55;
                    let mut g = grass_tuft(Vec3::new(wx, y, wz), h, rng.next_u64() as u32, seasonify(pick(rng, under)));
                    tint_up(&mut g, lerp_c(pick(rng, crowns), rgb(240, 240, 200), 0.25), scale_rgb(pick(rng, under), 0.5));
                    flora.merge(&g);
                }
                UnderKind::Fern => {
                    // a low, wide frond rosette hugging the floor
                    let r = 0.5 + rng.f32() * 0.6;
                    let mut fern = leaf_cluster(Vec3::new(wx, y + r * 0.22, wz), r, rng.next_u64() as u32, seasonify(scale_rgb(pick(rng, under), 0.92)));
                    scale_xz(&mut fern, 1.5, 1.5);
                    tint_up(&mut fern, lerp_c(pick(rng, crowns), rgb(220, 235, 200), 0.15), scale_rgb(pick(rng, under), 0.45));
                    flora.merge(&fern);
                }
                UnderKind::Bush => {
                    let r = 0.4 + rng.f32() * 0.6;
                    let mut bush = leaf_cluster(Vec3::new(wx, y + r * 0.5, wz), r, rng.next_u64() as u32, seasonify(pick(rng, under)));
                    scale_xz(&mut bush, 1.0, 0.85 + rng.f32() * 0.3);
                    tint_up(&mut bush, lerp_c(pick(rng, crowns), rgb(255, 255, 255), 0.18), scale_rgb(pick(rng, under), 0.55));
                    flora.merge(&bush);
                }
            }
        }

        // ── LAYER 3 — slender mid trees: the bulk of the canopy, clustered into stands. ───────────────────
        let cap = if open { prof.cap } else { (self.ceil_y - self.floor_y - 0.4).max(1.4).min(prof.cap) };
        let mid_n = ((area / 13.0 * prof.tree_density) as i32).min(110); // over-sample; the density field thins it into glades
        for _ in 0..mid_n {
            let (wx, wz) = spot(rng, &tiles);
            if self.trail_dist(wx, wz) < 2.0 || self.stream_dist(wx, wz) < 3.2 || self.pond_dist(wx, wz) < 1.0 || self.lane_dist(wx, wz) < 4.5 {
                continue; // trees never grow in the worn path, stream bed, pond, or a tower-defense lane
            }
            let d = dens(wx, wz);
            if rng.f32() > d * d {
                continue; // strong clustering: trees crowd the thickets, glades stay open for sightlines
            }
            if self.xz_obstructed(wx, wz, 0.8) {
                continue;
            }
            let y = self.floor_h(wx, wz);
            let len = (prof.mid_len.0 + rng.f32() * prof.mid_len.1).min(cap);
            let radb = (0.12 + rng.f32() * 0.10) * prof.mid_rad;
            let seed = rng.next_u64() as u32;
            let leaf = seasonify(pick(rng, crowns));
            for m in tree_styled(self.cfg.theme.tree_kind(), Vec3::new(wx, y, wz), len, radb, if len > 4.0 { 2 } else { 1 }, seed, wood, leaf) {
                if m.emissive > 0.0 {
                    glow.merge(&m);
                } else {
                    // route trunk-ish (narrow/dark) into bark, leafy into flora — both just merge, colour is per-vertex
                    bark.merge(&m);
                }
            }
            if tree_obstacles < 48 {
                let hw = 0.32 * prof.mid_rad;
                self.obstacles.push(Obstacle { center: Vec3::new(wx, y + len * 0.5, wz), half: Vec3::new(hw, len * 0.5, hw) });
                tree_obstacles += 1;
            }
            let sr = 0.5 + len * 0.12;
            let mut sd = disc(Vec3::new(wx, self.floor_h(wx, wz) + 0.04, wz), sr, 8, rgb(8, 10, 12));
            scale_xz(&mut sd, 1.0, 0.82);
            shadows.merge(&sd);
        }
        let hero_n = ((area / 70.0) as i32).clamp(0, prof.hero_max);
        for _ in 0..hero_n {
            let (wx, wz) = spot(rng, &tiles);
            if self.trail_dist(wx, wz) < 2.4 || self.stream_dist(wx, wz) < 3.6 || self.pond_dist(wx, wz) < 1.5 || self.lane_dist(wx, wz) < 5.0 {
                continue;
            }
            if dens(wx, wz) < 0.4 {
                continue; // a big tree belongs in a stand, not the middle of a clearing
            }
            if self.xz_obstructed(wx, wz, 1.1) {
                continue;
            }
            let y = self.floor_h(wx, wz);
            let len = (prof.hero_len.0 + rng.f32() * prof.hero_len.1).min(cap + 2.0);
            let seed = rng.next_u64() as u32;
            let leaf = seasonify(pick(rng, crowns));
            for m in tree_styled(self.cfg.theme.tree_kind(), Vec3::new(wx, y, wz), len, (0.3 + rng.f32() * 0.18) * prof.hero_rad, 2, seed, wood, leaf) {
                bark.merge(&m);
            }
            if tree_obstacles < 56 {
                let hw = 0.5 * prof.hero_rad;
                self.obstacles.push(Obstacle { center: Vec3::new(wx, y + len * 0.5, wz), half: Vec3::new(hw, len * 0.5, hw) });
                tree_obstacles += 1;
            }
            let mut sd = disc(Vec3::new(wx, self.floor_h(wx, wz) + 0.04, wz), 1.2 + len * 0.12, 9, rgb(8, 10, 12));
            scale_xz(&mut sd, 1.0, 0.82);
            shadows.merge(&sd);
        }
        let rock_n = ((area / 22.0 * prof.boulders) as i32).min(80);
        for _ in 0..rock_n {
            let (wx, wz) = spot(rng, &tiles);
            let y = self.floor_h(wx, wz);
            let r = 0.4 + rng.f32() * 0.8;
            let mut bo = rock(Vec3::new(wx, y + r * 0.3, wz), r, rng.next_u64() as u32, self.pal.rock);
            tint_up(&mut bo, lerp_c(self.pal.rock, rgb(150, 150, 140), 0.35), scale_rgb(self.pal.rock, 0.55));
            bark.merge(&bo);
            let mut sd = disc(Vec3::new(wx, y + 0.04, wz), r * 1.15, 8, rgb(8, 10, 12));
            scale_xz(&mut sd, 1.0, 0.8);
            shadows.merge(&sd);
        }
        let log_n = ((area / 60.0) as i32).clamp(0, 14);
        for _ in 0..log_n {
            let (wx, wz) = spot(rng, &tiles);
            let y = self.floor_h(wx, wz);
            let dir = rng.f32() * std::f32::consts::TAU;
            let len = 1.8 + rng.f32() * 2.6;
            let rr = 0.26 + rng.f32() * 0.16;
            let a = Vec3::new(wx, y + rr, wz);
            let b = Vec3::new(wx + dir.cos() * len, self.floor_h(wx + dir.cos() * len, wz + dir.sin() * len) + rr, wz + dir.sin() * len);
            let mut log = tube(a, b, rr, 7, lerp_c(wood, rgb(80, 60, 40), 0.4));
            tint_up(&mut log, lerp_c(wood, rgb(120, 150, 80), 0.4), scale_rgb(wood, 0.5));
            bark.merge(&log);
        }

        // ── LAYER 6 — drifting coloured light shafts in the open-sky canopy. ─────────────────────────────
        if open {
            let shaft_n = ((area / 140.0) as i32).clamp(2, 8);
            for _ in 0..shaft_n {
                let (wx, wz) = spot(rng, &tiles);
                let y = self.floor_h(wx, wz);
                let col = shafts[(rng.next_u64() as usize) % shafts.len()];
                for m in light_shaft(wx, wz, self.ceil_y + 2.0, y, col, rng.next_u64() as u32) {
                    glow.merge(&m);
                }
            }
        }

        // ── LAYER 7 — far tree-line: a ring of simple distant trees beyond the walls that recede into the
        // fog, so the wood has a horizon instead of ending at a hard wall (logscope's "blot-out" band). ─────
        if open {
            let cx = self.w as f32 * ts * 0.5;
            let cz = self.h as f32 * ts * 0.5;
            let half = (self.w.max(self.h) as f32) * ts * 0.5;
            let leafc = crowns[0];
            for k in 0..150 {
                let a = (k as f32 / 150.0) * std::f32::consts::TAU + rng.f32() * 0.4;
                let rad = half * (1.02 + rng.f32() * 0.7); // just outside the map, fading into fog
                let wx = cx + a.cos() * rad;
                let wz = cz + a.sin() * rad;
                let len = 5.0 + rng.f32() * 5.0;
                let leaf = seasonify(pick(rng, crowns));
                // depth-0 trees: a trunk + one crown puff, cheap, dissolving into the fog wall
                for m in tree(Vec3::new(wx, self.floor_y - 0.4, wz), len, 0.18 + rng.f32() * 0.12, 0, rng.next_u64() as u32, wood, leaf) {
                    bark.merge(&m);
                }
                let _ = leafc;
            }
        }

        // ── LAYER 8 — stream: a translucent ribbon following the dipped channel, with reeds on the banks. ─
        if self.stream.len() >= 2 {
            let (wcol, walpha, wemis): (Color32, u8, f32) = match theme {
                MapTheme::Toxic => (rgb(120, 160, 50), 205, 0.3),
                _ => (rgb(46, 100, 118), 155, 0.05), // Mistfall water
            };
            let mut ribbon = Mesh3::new(wcol);
            ribbon.emissive = wemis;
            ribbon.reflect = if matches!(theme, MapTheme::Toxic) { 0.32 } else { 0.55 };
            let nrm = Vec3::new(0.0, 1.0, 0.0);
            let half_w = 2.9_f32;
            let surf = |s: &Self, x: f32, z: f32| s.floor_h(x, z) + 0.42; // water sits above the bed
            for w in self.stream.windows(2) {
                let (a, b) = (w[0], w[1]);
                let dir = Vec2::new(b.x - a.x, b.y - a.y);
                let dl = dir.len().max(1e-3);
                let (px, pz) = (-dir.y / dl * half_w, dir.x / dl * half_w); // perpendicular offset
                let a0 = Vec3::new(a.x + px, surf(self, a.x + px, a.y + pz), a.y + pz);
                let a1 = Vec3::new(a.x - px, surf(self, a.x - px, a.y - pz), a.y - pz);
                let b0 = Vec3::new(b.x + px, surf(self, b.x + px, b.y + pz), b.y + pz);
                let b1 = Vec3::new(b.x - px, surf(self, b.x - px, b.y - pz), b.y - pz);
                ribbon.push_quad(a0, b0, b1, a1, nrm, wcol);
            }
            set_alpha(&mut ribbon, walpha);
            self.meshes.push(ribbon);

            // reeds along the banks — tall thin blades clustered just outside the water line
            let reed_col = match theme {
                MapTheme::Toxic => rgb(120, 150, 60),
                _ => rgb(96, 134, 110),
            };
            let steps = self.stream.len();
            for i in 0..steps {
                let c = self.stream[i];
                for _ in 0..3 {
                    let off = 2.6 + rng.f32() * 1.4;
                    let ang = rng.f32() * std::f32::consts::TAU;
                    let (rx, rz) = (c.x + ang.cos() * off, c.y + ang.sin() * off);
                    if self.stream_dist(rx, rz) < 2.4 {
                        continue; // not in the water itself
                    }
                    let y = self.floor_h(rx, rz);
                    let h = 0.8 + rng.f32() * 0.9;
                    let mut reed = leaf_cluster(Vec3::new(rx, y + h * 0.5, rz), 0.3 + rng.f32() * 0.2, rng.next_u64() as u32, reed_col);
                    scale_xz(&mut reed, 0.5, 0.5); // pinch to a slim upright tuft
                    tint_up(&mut reed, lerp_c(reed_col, rgb(220, 230, 180), 0.3), scale_rgb(reed_col, 0.5));
                    flora.merge(&reed);
                }
            }
        }

        // ── LAYER 8b — ponds: a reflective sheet over each dipped basin, ringed with reeds. ───────────────
        if !self.ponds.is_empty() {
            let (pcol, palpha): (Color32, u8) = match theme {
                MapTheme::Toxic => (rgb(120, 160, 50), 205),
                MapTheme::Mistfall => (rgb(46, 100, 118), 160),
                _ => (rgb(60, 110, 150), 150), // clear blue pond
            };
            let reed_col = match theme {
                MapTheme::Toxic => rgb(120, 150, 60),
                _ => rgb(96, 134, 110),
            };
            let ponds = self.ponds.clone();
            for (c, r) in ponds {
                let wy = self.floor_h(c.x, c.y) + 0.5; // water sits above the dipped bed
                self.water.push(WaterBody {
                    cx: c.x,
                    cz: c.y,
                    radius: r * 0.96,
                    y: wy,
                    col: pcol,
                    alpha: palpha,
                    reflect: if matches!(theme, MapTheme::Toxic) { 0.34 } else { 0.62 },
                    emissive: 0.0,
                    lava: false,
                    disc: true,
                });
                // reedy fringe
                let ring = (r * 2.2) as i32;
                for _ in 0..ring {
                    let a = rng.f32() * std::f32::consts::TAU;
                    let rr = r * (1.0 + rng.f32() * 0.18);
                    let (rx, rz) = (c.x + a.cos() * rr, c.y + a.sin() * rr);
                    if self.pond_dist(rx, rz) < 0.0 {
                        continue;
                    }
                    let gy = self.floor_h(rx, rz);
                    let h = 0.7 + rng.f32() * 0.8;
                    let mut reed = leaf_cluster(Vec3::new(rx, gy + h * 0.5, rz), 0.28 + rng.f32() * 0.18, rng.next_u64() as u32, reed_col);
                    scale_xz(&mut reed, 0.45, 0.45);
                    flora.merge(&reed);
                }
            }
        }

        // ── LAYER 9 — rock outcrops: a few tall stacked tors that anchor the eye, break sightlines, and add
        // real vertical mass to the landscape. Placed off the path/stream, they double as hard cover. ─────
        let formations = ((area / 2200.0) as i32).clamp(3, 7);
        for _ in 0..formations {
            for _try in 0..12 {
                let (wx, wz) = spot(rng, &tiles);
                if self.trail_dist(wx, wz) < 4.0 || self.stream_dist(wx, wz) < 4.0 {
                    continue;
                }
                if self.xz_obstructed(wx, wz, 1.6) {
                    continue;
                }
                let base_y = self.floor_h(wx, wz);
                let layers = 4 + (rng.next_u64() % 4) as i32; // 4–7 stacked boulders
                let base_r = 1.5 + rng.f32() * 1.1;
                let total_h = 2.6 + rng.f32() * 2.6;
                for li in 0..layers {
                    let t = li as f32 / layers as f32;
                    let r = base_r * (1.0 - t * 0.6);
                    let cy = base_y + total_h * t + r * 0.25;
                    let jx = (rng.f32() - 0.5) * base_r * 0.55;
                    let jz = (rng.f32() - 0.5) * base_r * 0.55;
                    let mut rk = rock(Vec3::new(wx + jx, cy, wz + jz), r, rng.next_u64() as u32, self.pal.rock);
                    tint_up(&mut rk, lerp_c(self.pal.rock, rgb(172, 172, 160), 0.4), scale_rgb(self.pal.rock, 0.5));
                    bark.merge(&rk);
                }
                // a flat capstone you could perch on
                let mut cap = disc(Vec3::new(wx, base_y + total_h + 0.12, wz), base_r * 0.5, 7, lerp_c(self.pal.rock, rgb(150, 150, 140), 0.3));
                tint_up(&mut cap, lerp_c(self.pal.rock, rgb(180, 180, 170), 0.4), scale_rgb(self.pal.rock, 0.55));
                bark.merge(&cap);
                self.obstacles.push(Obstacle { center: Vec3::new(wx, base_y + total_h * 0.5, wz), half: Vec3::new(base_r * 0.8, total_h * 0.5, base_r * 0.8) });
                let mut sd = disc(Vec3::new(wx, base_y + 0.05, wz), base_r * 1.25, 9, rgb(8, 10, 12));
                scale_xz(&mut sd, 1.0, 0.78);
                shadows.merge(&sd);
                break;
            }
        }

        // ── LAYER 10 — stepping stones where the footpath fords the stream. ──────────────────────────────
        if self.trail.len() >= 2 && self.stream.len() >= 2 {
            for w in self.trail.windows(2) {
                let mid = Vec2::new((w[0].x + w[1].x) * 0.5, (w[0].y + w[1].y) * 0.5);
                if self.stream_dist(mid.x, mid.y) < 3.2 {
                    for _ in 0..4 {
                        let sx = mid.x + (rng.f32() - 0.5) * 4.0;
                        let sz = mid.y + (rng.f32() - 0.5) * 4.0;
                        let y = self.floor_h(sx, sz) + 0.34; // up at the water surface
                        let mut stone = disc(Vec3::new(sx, y, sz), 0.45 + rng.f32() * 0.25, 6, lerp_c(self.pal.rock, rgb(150, 150, 140), 0.3));
                        scale_xz(&mut stone, 1.0, 0.8);
                        tint_up(&mut stone, lerp_c(self.pal.rock, rgb(180, 180, 170), 0.4), scale_rgb(self.pal.rock, 0.5));
                        bark.merge(&stone);
                    }
                }
            }
        }

        if shadows.tri_count() > 0 {
            set_alpha(&mut shadows, 78);
            shadows.lod_far = 0.5;
            self.meshes.push(shadows);
        }
        if flora.tri_count() > 0 {
            flora.lod_far = 0.62; // distant leaves dissolve into fog before the ground does
            self.meshes.push(flora);
        }
        if bark.tri_count() > 0 {
            bark.lod_far = 0.74;
            self.meshes.push(bark);
        }
        if glow.tri_count() > 0 {
            self.meshes.push(glow);
        }
        crate::mesh::set_detail(saved_detail);
    }

    /// A spawn-safe point in room `i`: the room centre, or the nearest open floor tile that isn't blocked
    /// by cover or the landmark, so the player never starts inside geometry.
    pub fn spawn(&self, i: usize) -> Vec3 {
        let r = &self.rooms[i % self.rooms.len().max(1)];
        let (cx, cz) = r.rect.center();
        for radius in 0i32..7 {
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs() != radius && dz.abs() != radius {
                        continue; // only the ring at this radius
                    }
                    let (tx, tz) = (cx + dx, cz + dz);
                    if self.cell(tx, tz) == Cell::Floor {
                        let p = self.tile_center(tx, tz);
                        if !self.xz_obstructed(p.x, p.z, 0.55) {
                            return p;
                        }
                    }
                }
            }
        }
        r.center.with_y(self.floor_h(r.center.x, r.center.z))
    }

    /// Is a circle of radius `rad` at world (x,z) overlapping any cover/landmark obstacle?
    pub fn xz_obstructed(&self, x: f32, z: f32, rad: f32) -> bool {
        self.obstacles.iter().any(|o| {
            let dx = (x - o.center.x).abs() - o.half.x;
            let dz = (z - o.center.z).abs() - o.half.z;
            dx < rad && dz < rad
        })
    }
}

/// Like `carve_h` but also fills pits (Void) along the way — a 1-wide walkway across a hole.
fn bridge_h(lvl: &mut Level, x0: i32, x1: i32, z: i32) {
    let (a, b) = (x0.min(x1), x0.max(x1));
    for x in a..=b {
        if lvl.cell(x, z) != Cell::Floor {
            lvl.set(x, z, Cell::Floor);
        }
    }
}

fn bridge_v(lvl: &mut Level, z0: i32, z1: i32, x: i32) {
    let (a, b) = (z0.min(z1), z0.max(z1));
    for z in a..=b {
        if lvl.cell(x, z) != Cell::Floor {
            lvl.set(x, z, Cell::Floor);
        }
    }
}

fn carve_h(lvl: &mut Level, x0: i32, x1: i32, z: i32, width: i32) {
    let (a, b) = (x0.min(x1), x0.max(x1));
    for x in a..=b {
        for w in 0..width {
            let zz = z - width / 2 + w;
            if lvl.cell(x, zz) == Cell::Solid {
                lvl.set(x, zz, Cell::Floor);
            }
        }
    }
}
fn carve_v(lvl: &mut Level, z0: i32, z1: i32, x: i32, width: i32) {
    let (a, b) = (z0.min(z1), z0.max(z1));
    for z in a..=b {
        for w in 0..width {
            let xx = x - width / 2 + w;
            if lvl.cell(xx, z) == Cell::Solid {
                lvl.set(xx, z, Cell::Floor);
            }
        }
    }
}

/// Interior compound: BSP rooms joined by L-corridors with a couple of loops. (Industrial/Bunker/Military.)
/// Skin a top-ranked topology graph (from the flow-first factory) into an actual floor plan: each graph node
/// becomes a room sized by its role, each edge a corridor sized by its kind. Because the graph was *selected*
/// for connectivity and balanced flow, the carved map inherits that gameplay by construction — node 0 is the
/// spawn, every node is reachable, and lanes/corners/ramps give a designed mix of engagement ranges. The rest
/// of the pipeline (terrain, walls, landmark, water, vignettes, detritus) then decorates it like any map.
/// Three-lane competitive carve — the readable backbone of arena shooters. Two opposing spawns, three N–S
/// lanes (two tight flanks and a wide central artery), and two lateral cross-connectors so players can rotate
/// between lanes instead of committing to one. Everything between the lanes stays solid, so the lanes read as
/// distinct routes with clear sightlines; `build_triplane` then adds the central power tower, cover rhythm and
/// the power pickup. Connected by construction (all lanes meet the spawn bands and the connectors).
fn carve_triplane(lvl: &mut Level, _rng: &mut Rng, cfg: &MapConfig) -> Vec<Rect> {
    let (w, h) = (cfg.size, cfg.size);
    let lane_x = [w * 22 / 100, w / 2, w * 78 / 100];
    let lane_w = [5, 7, 5]; // tight flanks, wide central artery
    let z0 = 7;
    let z1 = h - 7;
    // three lanes
    for i in 0..3 {
        carve_v(lvl, z0, z1, lane_x[i], lane_w[i]);
    }
    // two cross-connectors at the thirds — the rotation lattice
    for &cz in &[h / 3, 2 * h / 3] {
        carve_h(lvl, lane_x[0], lane_x[2], cz, 4);
    }
    // spawn bands span all three lanes at each end
    carve_h(lvl, lane_x[0], lane_x[2], z0 + 3, 6);
    carve_h(lvl, lane_x[0], lane_x[2], z1 - 3, 6);
    let cx = w / 2;
    vec![
        Rect::new(cx - 4, z1 - 6, cx + 4, z1 - 1), // south spawn (player)
        Rect::new(cx - 4, z0 + 1, cx + 4, z0 + 6), // north spawn
        // flank junctions at the connectors — light cover anchors for the rotation fights
        Rect::new(lane_x[0] - 2, h / 3 - 2, lane_x[0] + 2, h / 3 + 2),
        Rect::new(lane_x[2] - 2, 2 * h / 3 - 2, lane_x[2] + 2, 2 * h / 3 + 2),
    ]
}

fn carve_graph(lvl: &mut Level, _rng: &mut Rng, cfg: &MapConfig) -> Vec<Rect> {
    use crate::mapgraph::{select_best, EdgeKind, MapGraph, NodeRole};
    // Deterministically pull a strong graph from this map's seed: scan a budget of candidates, keep the best.
    let picked = select_best(1, 55.0, cfg.seed, 600);
    let gseed = picked.first().map(|(s, _)| *s).unwrap_or(cfg.seed);
    let g = MapGraph::generate(gseed);

    let (w, h) = (cfg.size, cfg.size);
    let margin = 7;
    let span = (w - margin * 2).max(1) as f32;
    let to_tile = |nx: f32, nz: f32| -> (i32, i32) { ((margin as f32 + nx * span) as i32, (margin as f32 + nz * span) as i32) };

    // Each node → a room rect sized by its role (junctions sprawl, towers/cover are tight). Stash each room's
    // target elevation, remapped to the full playable tier range so the layout always undulates regardless of
    // how the candidate's RNG distributed heights: lowest node → deepest trench, highest → tallest vantage,
    // with the graph's *relative* ordering preserved. Built biomes cap the high tier under the roof.
    let (lo, hi) = if cfg.theme.open_sky() { (-2.5f32, 2.4f32) } else { (-1.9f32, 1.6f32) };
    let (mut emin, mut emax) = (f32::MAX, f32::MIN);
    for n in &g.nodes {
        if n.role != NodeRole::Spawn {
            emin = emin.min(n.elevation);
            emax = emax.max(n.elevation);
        }
    }
    let erange = (emax - emin).max(0.1);
    let mut rects: Vec<Rect> = Vec::with_capacity(g.nodes.len());
    let mut elevs: Vec<f32> = Vec::with_capacity(g.nodes.len());
    for node in &g.nodes {
        let (cx, cz) = to_tile(node.x, node.z);
        let hs = match node.role {
            NodeRole::Junction => 6,
            NodeRole::Room | NodeRole::Spawn => 5,
            NodeRole::Trench => 4,
            NodeRole::Tower | NodeRole::Cover => 3,
        };
        let r = Rect::new((cx - hs).max(1), (cz - hs).max(1), (cx + hs).min(w - 1), (cz + hs).min(h - 1));
        let shape = if rects.is_empty() { RoomShape::Rect } else { RoomShape::pick(_rng, &r) };
        carve_shaped(lvl, &r, shape);
        rects.push(r);
        // the node's role determines what this space is *for*, which determines its coherent structure
        lvl.space_plan.push((r, crate::spaces::SpaceFunction::from_role(node.role)));
        // spawns stay level; every other room sits along the full elevation range by its node height
        let e = if node.role == NodeRole::Spawn {
            0.0
        } else {
            lo + ((node.elevation - emin) / erange) * (hi - lo)
        };
        elevs.push(e);
    }
    lvl.graph_elev = elevs;
    // Each edge → an L-corridor sized by kind (lanes wide & open, corners tight).
    for e in &g.edges {
        let cw = match e.kind {
            EdgeKind::Lane => 4,
            EdgeKind::Ramp => 3,
            EdgeKind::Corner => 2,
        };
        let (ax, az) = rects[e.a].center();
        let (bx, bz) = rects[e.b].center();
        carve_h(lvl, ax, bx, az, cw);
        carve_v(lvl, az, bz, bx, cw);
    }
    rects
}

/// Skin an agent-simulated territory into a playable map: carve a fully-connected floor base, run the
/// Sniper/Rusher/Camper simulation (best of a few coarse runs), then lay the emergent elevation field as the
/// terrain and drop cover obstacles where the campers piled cover. The base is open Floor so connectivity is
/// guaranteed; the agents only shape height and scatter point cover, which can't seal anything off.
fn carve_agents(lvl: &mut Level, rng: &mut Rng, cfg: &MapConfig) -> Vec<Rect> {
    use crate::mapagents::{score as ascore, Territory};
    let (w, h) = (cfg.size, cfg.size);
    for z in 3..h - 3 {
        for x in 3..w - 3 {
            lvl.set(x, z, Cell::Floor);
        }
    }
    // Simulate a coarse territory and keep the best of a few — the agent factory's curate step.
    let (sw, sh) = (56usize, 56usize);
    let mut best = Territory::simulate(sw, sh, cfg.seed, 18, 120);
    let mut bs = ascore(&best).total;
    for i in 1..4u64 {
        let t = Territory::simulate(sw, sh, cfg.seed.wrapping_add(i.wrapping_mul(0x9E37_79B9_7F4A_7C15)), 18, 120);
        let s = ascore(&t).total;
        if s > bs {
            bs = s;
            best = t;
        }
    }
    // Sample the emergent height field into the level grid (clamped to a climbable, under-ceiling range).
    let mut hf = vec![0.0f32; (w * h) as usize];
    for z in 0..h {
        for x in 0..w {
            let sx = ((x as f32 / w as f32) * sw as f32) as usize;
            let sz = ((z as f32 / h as f32) * sh as f32) as usize;
            let v = best.height[sz.min(sh - 1) * sw + sx.min(sw - 1)];
            hf[(z * w + x) as usize] = (v * 0.95).clamp(-2.1, 2.3);
        }
    }
    lvl.agent_height = hf;
    // Cover obstacles where the campers piled cover (sparse points in open floor → never seal a route).
    let ts = lvl.ts();
    let mut placed = 0;
    for sz in 0..sh {
        for sx in 0..sw {
            if best.cover[sz * sw + sx] > 1.4 && rng.f32() < 0.6 && placed < 160 {
                let wx = ((sx as f32 + 0.5) / sw as f32) * (w as f32) * ts;
                let wz = ((sz as f32 + 0.5) / sh as f32) * (h as f32) * ts;
                lvl.obstacles.push(Obstacle { center: Vec3::new(wx, lvl.floor_y + 0.6, wz), half: Vec3::new(0.75, 0.6, 0.75) });
                placed += 1;
            }
        }
    }
    // Spawns from the simulation's corner spawns; a centre anchor for the landmark.
    let to_rect = |sx: f32, sz: f32| {
        let tx = ((sx / sw as f32) * w as f32) as i32;
        let tz = ((sz / sh as f32) * h as f32) as i32;
        Rect::new((tx - 3).max(4), (tz - 3).max(4), (tx + 3).min(w - 4), (tz + 3).min(h - 4))
    };
    vec![
        to_rect(best.spawns[0].0, best.spawns[0].1),
        to_rect(best.spawns[1].0, best.spawns[1].1),
        Rect::new(w / 2 - 3, h / 2 - 3, w / 2 + 3, h / 2 + 3),
    ]
}

fn carve_rooms(lvl: &mut Level, rng: &mut Rng, cfg: &MapConfig) -> Vec<Rect> {
    let (w, h) = (cfg.size, cfg.size);
    let root = Rect::new(1, 1, w - 1, h - 1);
    let tree = bsp_split(rng, root, 0, 5, cfg.room_min + 2);
    let mut leaves = Vec::new();
    bsp_leaves(&tree, &mut leaves);
    let mut rects = Vec::new();
    for leaf in leaves {
        if rects.len() >= cfg.room_count {
            break;
        }
        if let Some(r) = place_room(rng, leaf, cfg) {
            // The first room (spawn) stays a clean rectangle; the rest take varied footprints — clipped
            // corners, rounded, or L-shaped — so the plan reads as architecture, not a grid of identical boxes.
            let shape = if rects.is_empty() { RoomShape::Rect } else { RoomShape::pick(rng, &r) };
            carve_shaped(lvl, &r, shape);
            rects.push(r);
        }
    }
    let cw = cfg.corridor_width.max(1);
    for i in 1..rects.len() {
        let (ax, az) = rects[i - 1].center();
        let (bx, bz) = rects[i].center();
        carve_h(lvl, ax, bx, az, cw);
        carve_v(lvl, az, bz, bx, cw);
    }
    if rects.len() > 3 {
        for _ in 0..2 {
            let a = rng.i32_range(0, rects.len() as i32) as usize;
            let b = rng.i32_range(0, rects.len() as i32) as usize;
            if a != b {
                let (ax, az) = rects[a].center();
                let (bx, bz) = rects[b].center();
                carve_h(lvl, ax, bx, az, cw);
                carve_v(lvl, az, bz, bx, cw);
            }
        }
    }
    rects
}

/// Non-rectangular room footprints. Each still has a guaranteed Floor core at its centre, so corridors land
/// and connectivity holds by construction; only the perimeter changes, which is where the walls — and the
/// "boxy dungeon" read — come from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RoomShape {
    Rect,
    Octagon, // clipped corners → angled walls
    Round,   // inscribed ellipse → curved walls
    Ell,     // one quadrant removed → an L-plan
}

impl RoomShape {
    fn pick(rng: &mut Rng, r: &Rect) -> RoomShape {
        // tiny rooms stay rectangular (clipping would leave too little floor)
        if (r.x1 - r.x0).min(r.z1 - r.z0) < 5 {
            return RoomShape::Rect;
        }
        match rng.i32_range(0, 100) {
            0..=44 => RoomShape::Rect,
            45..=68 => RoomShape::Octagon,
            69..=86 => RoomShape::Round,
            _ => RoomShape::Ell,
        }
    }
}

/// Carve a room's floor according to its shape, always keeping a 3×3 Floor core at the centre.
fn carve_shaped(lvl: &mut Level, r: &Rect, shape: RoomShape) {
    let (cx, cz) = r.center();
    let (rw, rh) = (r.x1 - r.x0, r.z1 - r.z0);
    let clip = (rw.min(rh) / 4).max(1);
    // pick which quadrant the L-plan removes, from the rect's own geometry (deterministic, no rng needed)
    let ell_right = (r.x0 + r.z0) & 1 == 0;
    let ell_top = (r.x0 + r.z0) & 2 == 0;
    for z in r.z0..r.z1 {
        for x in r.x0..r.x1 {
            let keep = match shape {
                RoomShape::Rect => true,
                RoomShape::Octagon => {
                    let dx = (x - r.x0).min(r.x1 - 1 - x);
                    let dz = (z - r.z0).min(r.z1 - 1 - z);
                    dx + dz >= clip
                }
                RoomShape::Round => {
                    let nx = (x as f32 + 0.5 - (r.x0 as f32 + rw as f32 * 0.5)) / (rw as f32 * 0.5);
                    let nz = (z as f32 + 0.5 - (r.z0 as f32 + rh as f32 * 0.5)) / (rh as f32 * 0.5);
                    nx * nx + nz * nz <= 1.02
                }
                RoomShape::Ell => {
                    let in_x = if ell_right { x >= cx } else { x < cx };
                    let in_z = if ell_top { z >= cz } else { z < cz };
                    !(in_x && in_z) // remove the chosen quadrant
                }
            };
            let core = (x - cx).abs() <= 1 && (z - cz).abs() <= 1; // always-walkable connect pad
            if keep || core {
                lvl.set(x, z, Cell::Floor);
            }
        }
    }
}

/// Street grid: parallel avenues and cross-streets between solid building blocks, with a central plaza.
/// You fight in the lanes; the buildings are hard cover. (Urban.)
fn carve_streets(lvl: &mut Level, rng: &mut Rng, cfg: &MapConfig) -> Vec<Rect> {
    let (w, h) = (cfg.size, cfg.size);
    let road = 3;
    let block = 7 + rng.i32_range(0, 3); // 7..9 tile building blocks
    let step = road + block;
    let mut xs = Vec::new();
    let mut zs = Vec::new();
    let mut x = 3;
    while x < w - 4 {
        xs.push(x);
        x += step;
    }
    let mut z = 3;
    while z < h - 4 {
        zs.push(z);
        z += step;
    }
    for &ax in &xs {
        for zz in 2..h - 2 {
            for k in 0..road {
                lvl.set(ax + k, zz, Cell::Floor);
            }
        }
    }
    for &az in &zs {
        for xx in 2..w - 2 {
            for k in 0..road {
                lvl.set(xx, az + k, Cell::Floor);
            }
        }
    }
    // Central plaza (the landmark room).
    let (pcx, pcz) = (w / 2, h / 2);
    let ph = block / 2 + 1;
    let plaza = Rect::new((pcx - ph).max(2), (pcz - ph).max(2), (pcx + ph).min(w - 2), (pcz + ph).min(h - 2));
    for zz in plaza.z0..plaza.z1 {
        for xx in plaza.x0..plaza.x1 {
            lvl.set(xx, zz, Cell::Floor);
        }
    }
    // Rooms at intersections (edge ones first → spawn at the edge), then the central plaza.
    let mut rects = Vec::new();
    for &az in &zs {
        for &ax in &xs {
            rects.push(Rect::new(ax, az, (ax + road).min(w - 2), (az + road).min(h - 2)));
        }
    }
    rects.push(plaza);
    rects
}

/// Open bays: one big floor with rows of solid container stacks (with gaps) forming aisles and sightlines.
/// Room centres are wired together so the aisles always connect. (Warehouse.)
fn carve_bays(lvl: &mut Level, rng: &mut Rng, cfg: &MapConfig) -> Vec<Rect> {
    let (w, h) = (cfg.size, cfg.size);
    for zz in 3..h - 3 {
        for xx in 3..w - 3 {
            lvl.set(xx, zz, Cell::Floor);
        }
    }
    // Container rows: 2-thick solid blocks with gaps so aisles cross between rows.
    let row_step = 5 + rng.i32_range(0, 2);
    let mut z = 6;
    while z < h - 6 {
        let mut x = 5;
        while x < w - 6 {
            let len = rng.i32_range(3, 8);
            if rng.chance(0.72) {
                for zz in z..(z + 2) {
                    for xx in x..(x + len).min(w - 4) {
                        lvl.set(xx, zz, Cell::Solid);
                    }
                }
            }
            x += len + rng.i32_range(3, 5);
        }
        z += row_step;
    }
    // Open-zone rooms on a coarse grid (only where the centre is clear).
    let mut rects = Vec::new();
    let mut z2 = 6;
    while z2 < h - 9 {
        let mut x2 = 6;
        while x2 < w - 9 {
            if lvl.cell(x2 + 3, z2 + 3) == Cell::Floor {
                rects.push(Rect::new(x2, z2, (x2 + 6).min(w - 3), (z2 + 6).min(h - 3)));
            }
            x2 += 10;
        }
        z2 += 10;
    }
    // Guarantee connectivity: punch aisles between successive room centres (carves through containers).
    for i in 1..rects.len() {
        let (ax, az) = rects[i - 1].center();
        let (bx, bz) = rects[i].center();
        carve_h(lvl, ax, bx, az, 3);
        carve_v(lvl, az, bz, bx, 3);
    }
    rects
}

/// Organic clearings: rough circular open areas (noisy edges) linked by wide natural paths — caverns,
/// jungle glades, lava fields. (Volcanic/Ice/Jungle.)
/// Carve the exterior **deck** of a space station: a large cross-shaped platform (a main deck with four
/// projecting arms ending in landing-pad spurs) floating in the void — everything off the deck is `Void`
/// (space), so the playable area is the ship's top surface.
fn carve_station(lvl: &mut Level, _rng: &mut Rng, cfg: &MapConfig) -> Vec<Rect> {
    let (w, h) = (cfg.size, cfg.size);
    // start as space
    for z in 0..h {
        for x in 0..w {
            lvl.set(x, z, Cell::Void);
        }
    }
    let cx = w / 2;
    let cz = h / 2;
    let half_w = (w as f32 * 0.30) as i32; // main deck half-width
    let half_h = (h as f32 * 0.22) as i32;
    let fill = |x0: i32, z0: i32, x1: i32, z1: i32, lvl: &mut Level| {
        for z in z0.max(1)..z1.min(h - 1) {
            for x in x0.max(1)..x1.min(w - 1) {
                lvl.set(x, z, Cell::Floor);
            }
        }
    };
    // main rectangular deck
    fill(cx - half_w, cz - half_h, cx + half_w, cz + half_h, lvl);
    // a long spine running fore-aft
    let spine = (w as f32 * 0.10) as i32;
    fill(cx - spine, 4, cx + spine, h - 4, lvl);
    // two wing decks (landing-pad arms) port & starboard
    let arm = (h as f32 * 0.12) as i32;
    fill(4, cz - arm, w - 4, cz + arm, lvl);

    let rects = vec![
        Rect::new(cx - half_w, cz - half_h, cx + half_w, cz + half_h), // main deck (room 0 = spawn)
        Rect::new(cx - spine, 6, cx + spine, cz - half_h),             // fore spine
        Rect::new(cx - spine, cz + half_h, cx + spine, h - 6),         // aft spine
        Rect::new(6, cz - arm, cx - half_w, cz + arm),                 // port wing
        Rect::new(cx + half_w, cz - arm, w - 6, cz + arm),             // starboard wing
    ];
    rects
}

/// Carve a carrier **flight deck**: a long, narrow steel runway running the length of the map (linear sight-
/// lines), with a midship elevator sponson, ringed entirely by `Void` (ocean). Returns deck rooms (room 0 =
/// midship spawn).
fn carve_carrier(lvl: &mut Level, _rng: &mut Rng, cfg: &MapConfig) -> Vec<Rect> {
    let (w, h) = (cfg.size, cfg.size);
    for z in 0..h {
        for x in 0..w {
            lvl.set(x, z, Cell::Void);
        }
    }
    let cx = w / 2;
    let half = (w as f32 * 0.16).max(6.0) as i32;
    for z in 4..h - 4 {
        for x in (cx - half).max(1)..(cx + half).min(w - 1) {
            lvl.set(x, z, Cell::Floor);
        }
    }
    // midship elevator sponson bulging to starboard
    let mz = h / 2;
    let bulge = (h as f32 * 0.09) as i32;
    let depth = (w as f32 * 0.06) as i32;
    for z in (mz - bulge).max(1)..(mz + bulge).min(h - 1) {
        for x in (cx + half).max(1)..(cx + half + depth).min(w - 1) {
            lvl.set(x, z, Cell::Floor);
        }
    }
    vec![
        Rect::new(cx - half, mz - 6, cx + half, mz + 6), // midship (spawn)
        Rect::new(cx - half, 6, cx + half, h / 3),       // bow
        Rect::new(cx - half, 2 * h / 3, cx + half, h - 6), // stern
    ]
}

/// Culvert Breach: a two-tier siege ground. A thick wall runs east–west across the middle; the only ground
/// crossing is a breach blown through its centre. South of the wall is the sunken waterlogged trench (the
/// attacker's claustrophobic bottom tier), north is the defender's court. The elevated wall-walk on top and
/// the rubble ramp up through the breach are added later in `build_culvert`. Connectivity: south ↔ breach ↔
/// north is one continuous floor.
/// Submerged berthing docks: a single open, shallow-flooded basin (fully connected by construction). The
/// container labyrinth, raised walkways, ramps and props are all added in `build_docks`; here we just carve
/// the floor and place spawns at opposite ends with a couple of cover anchors.
fn carve_docks(lvl: &mut Level, _rng: &mut Rng, cfg: &MapConfig) -> Vec<Rect> {
    let (w, h) = (cfg.size, cfg.size);
    for z in 3..h - 3 {
        for x in 3..w - 3 {
            lvl.set(x, z, Cell::Floor);
        }
    }
    let cx = w / 2;
    vec![
        Rect::new(cx - 4, h - 11, cx + 4, h - 6), // south spawn (player)
        Rect::new(cx - 4, 6, cx + 4, 11),         // north spawn
        Rect::new(cx - 12, h / 2 - 3, cx - 6, h / 2 + 3), // west cover anchor
        Rect::new(cx + 6, h / 2 - 3, cx + 12, h / 2 + 3), // east cover anchor
    ]
}

fn carve_culvert(lvl: &mut Level, _rng: &mut Rng, cfg: &MapConfig) -> Vec<Rect> {
    let (w, h) = (cfg.size, cfg.size);
    let cx = w / 2;
    let cz = h / 2;
    for z in 3..h - 3 {
        for x in 3..w - 3 {
            lvl.set(x, z, Cell::Floor);
        }
    }
    let (bx0, bx1) = (cx - 3, cx + 3);
    for z in (cz - 1)..=(cz + 1) {
        for x in 3..w - 3 {
            if x < bx0 || x > bx1 {
                lvl.set(x, z, Cell::Solid);
            }
        }
    }
    vec![
        Rect::new(cx - 4, h - 11, cx + 4, h - 6),  // south spawn (in the trench)
        Rect::new(cx - 4, 6, cx + 4, 11),          // north spawn (the court)
        Rect::new(cx - 3, cz + 4, cx + 3, cz + 8), // trench cover just south of the breach
        Rect::new(cx - 12, 8, cx - 6, 13),         // court cover pocket (landmark anchor, off the breach)
        Rect::new(cx + 8, h - 14, cx + 14, h - 9), // trench cover pocket
    ]
}

/// Chasm Span: a narrow granite bridge stretching across a bottomless void, flanked by cramped guard chambers
/// cut into the rock at the midpoint. The whole map starts as abyss (Void); we carve only the span, the
/// chambers, and the connecting necks — everything else stays a fatal drop. Connectivity is trivial: one
/// continuous span with the chambers hung off it, spawns at the two ends.
fn carve_bridge(lvl: &mut Level, _rng: &mut Rng, cfg: &MapConfig) -> Vec<Rect> {
    let (w, h) = (cfg.size, cfg.size);
    // The whole map is the abyss: a fatal void. We carve only what you can stand on.
    for z in 0..h {
        for x in 0..w {
            lvl.set(x, z, Cell::Void);
        }
    }
    let cx = w / 2;
    let span_half = 3; // ≈6 tiles across
    let z0 = 5;
    let z1 = h - 5;
    // the bridge span — edges drop straight into the void (no railings, sharp drops)
    for z in z0..z1 {
        for x in (cx - span_half)..=(cx + span_half) {
            lvl.set(x, z, Cell::Floor);
        }
    }
    // the missing centre block: a dangerous gap on the EAST edge, offset north of the chamber necks so it
    // never severs the span — you skirt it on the west side.
    let mz = h / 2;
    for z in (mz - 5)..=(mz - 3) {
        for x in (cx + 1)..=(cx + span_half) {
            lvl.set(x, z, Cell::Void);
        }
    }
    // flanking guard chambers cut into rock at the midpoint: lay solid rock, hollow a room, punch a neck to
    // the bridge. The rock becomes walls (Solid-near-Floor) in the wall-ring pass; the neck is the doorway.
    let mut chambers = Vec::new();
    for side in [-1i32, 1] {
        let ch_cx = cx + side * (span_half + 7);
        let (rx0, rx1) = if side < 0 { (ch_cx - 5, cx - span_half - 1) } else { (cx + span_half + 1, ch_cx + 5) };
        for z in (mz - 6)..=(mz + 6) {
            for x in rx0..=rx1 {
                lvl.set(x, z, Cell::Solid);
            }
        }
        for z in (mz - 4)..=(mz + 4) {
            for x in (ch_cx - 4)..=(ch_cx + 4) {
                lvl.set(x, z, Cell::Floor);
            }
        }
        let (nx0, nx1) = if side < 0 { (ch_cx + 4, cx - span_half) } else { (cx + span_half, ch_cx - 4) };
        for x in nx0..=nx1 {
            for z in (mz - 1)..=(mz + 1) {
                lvl.set(x, z, Cell::Floor);
            }
        }
        chambers.push(Rect::new(ch_cx - 4, mz - 4, ch_cx + 4, mz + 4));
    }
    let mut out = vec![
        Rect::new(cx - span_half, z0, cx + span_half, z0 + 5),
        Rect::new(cx - span_half, z1 - 5, cx + span_half, z1),
        Rect::new(cx - 2, mz - 2, cx + 2, mz + 2),
    ];
    out.extend(chambers);
    out
}


/// rim, ringed by the colosseum wall. A genuinely different *plan* from the room-and-corridor maps — combat
/// is about crossing the lethal open centre or working the perimeter. The whole disc is one connected floor,
/// so it's reachable by construction. Spawns sit at opposite edges; the centre anchors the landmark.
fn carve_arena(lvl: &mut Level, _rng: &mut Rng, cfg: &MapConfig) -> Vec<Rect> {
    let (w, h) = (cfg.size, cfg.size);
    let cx = w / 2;
    let cz = h / 2;
    let rad = (w.min(h) as f32 * 0.5 - 5.0).clamp(12.0, 26.0);
    // carve the arena disc
    for z in 0..h {
        for x in 0..w {
            let d = (((x - cx).pow(2) + (z - cz).pow(2)) as f32).sqrt();
            if d <= rad {
                lvl.set(x, z, Cell::Floor);
            }
        }
    }
    let edge = (rad as i32 - 4).max(6);
    let mut rects = Vec::new();
    // room 0 = south spawn alcove (player); a north spawn; the central landmark anchor.
    rects.push(Rect::new(cx - 3, cz + edge - 4, cx + 3, cz + edge)); // south spawn
    rects.push(Rect::new(cx - 3, cz - edge, cx + 3, cz - edge + 4)); // north spawn
    rects.push(Rect::new(cx - 2, cz - 2, cx + 3, cz + 3)); // centre (landmark)
    // a ring of perimeter cover pockets — cover clusters at the rim, leaving the centre an open no-man's-land.
    let pockets = 8;
    for k in 0..pockets {
        let a = k as f32 / pockets as f32 * std::f32::consts::TAU + 0.39;
        let rx = cx + (a.cos() * rad * 0.74) as i32;
        let rz = cz + (a.sin() * rad * 0.74) as i32;
        if lvl.cell(rx, rz) == Cell::Floor {
            rects.push(Rect::new(rx - 2, rz - 2, rx + 2, rz + 2));
        }
    }
    // Optional flanking lanes: two covered N–S routes running just outside the disc on the east and west, each
    // tied back into the arena at both ends. They give the open bowl a way to be outmanoeuvred — a player can
    // break contact, slip down a side lane, and re-enter from a different angle instead of crossing the killbox.
    if cfg.side_lanes {
        let lane_off = (rad as i32 + 6).min(cx - 4).min((w - cx) - 4);
        let lhw = 2; // lane half-width (~5 wide)
        let z0 = (cz - edge).max(3);
        let z1 = (cz + edge).min(h - 4);
        for &sx in &[cx - lane_off, cx + lane_off] {
            // the lane channel
            for z in z0..=z1 {
                for x in (sx - lhw)..=(sx + lhw) {
                    if lvl.in_bounds(x, z) {
                        lvl.set(x, z, Cell::Floor);
                    }
                }
            }
            // connectors tying each lane end back into the arena disc
            for &cz_end in &[z0, z1] {
                let (a, b) = (sx.min(cx), sx.max(cx));
                for x in a..=b {
                    for w2 in -1..=1 {
                        let zz = cz_end + w2;
                        if lvl.in_bounds(x, zz) && lvl.cell(x, zz) != Cell::Floor {
                            // only bridge the gap between lane and disc, not across the whole arena
                            let dd = (((x - cx).pow(2) + (zz - cz).pow(2)) as f32).sqrt();
                            if dd > rad - 1.0 {
                                lvl.set(x, zz, Cell::Floor);
                            }
                        }
                    }
                }
            }
            // a couple of cover pockets down the lane so it isn't a bare gallery
            rects.push(Rect::new(sx - lhw, cz - 6, sx + lhw, cz - 2));
            rects.push(Rect::new(sx - lhw, cz + 2, sx + lhw, cz + 6));
        }
    }
    rects
}

fn carve_clearings(lvl: &mut Level, rng: &mut Rng, cfg: &MapConfig) -> Vec<Rect> {
    let (w, h) = (cfg.size, cfg.size);
    let n = cfg.room_count.max(6);
    let mut centers: Vec<(i32, i32)> = Vec::new();
    let mut rects = Vec::new();
    let mut tries = 0;
    while rects.len() < n && tries < n * 6 {
        tries += 1;
        let rad = rng.i32_range(3, 6);
        let cx = rng.i32_range(rad + 2, w - rad - 2);
        let cz = rng.i32_range(rad + 2, h - rad - 2);
        if centers.iter().any(|&(ox, oz)| (ox - cx).pow(2) + (oz - cz).pow(2) < (rad + 2).pow(2)) {
            continue;
        }
        for dz in -rad..=rad {
            for dx in -rad..=rad {
                let jitter = lattice(cx + dx, cz + dz, cfg.seed as i32) * 1.5;
                if ((dx * dx + dz * dz) as f32) <= (rad as f32 + 0.6 + jitter).powi(2) {
                    lvl.set(cx + dx, cz + dz, Cell::Floor);
                }
            }
        }
        centers.push((cx, cz));
        rects.push(Rect::new((cx - rad).max(2), (cz - rad).max(2), (cx + rad).min(w - 2), (cz + rad).min(h - 2)));
    }
    let cw = cfg.corridor_width.max(2) + 1;
    for i in 1..centers.len() {
        let (ax, az) = centers[i - 1];
        let (bx, bz) = centers[i];
        carve_h(lvl, ax, bx, az, cw);
        carve_v(lvl, az, bz, bx, cw);
    }
    if centers.len() > 3 {
        for _ in 0..2 {
            let a = rng.i32_range(0, centers.len() as i32) as usize;
            let b = rng.i32_range(0, centers.len() as i32) as usize;
            if a != b {
                let (ax, az) = centers[a];
                let (bx, bz) = centers[b];
                carve_h(lvl, ax, bx, az, cw);
                carve_v(lvl, az, bz, bx, cw);
            }
        }
    }
    rects
}

// ── Landmark archetypes ────────────────────────────────────────────────────────────────────────────
// Each returns (meshes, blocking obstacles). They share a two-tier plinth helper where it fits.

fn lm_plinth(c: Vec3, pal: Palette, solid: &mut Mesh3) {
    let step_col = lerp_c(pal.trim, rgb(120, 124, 134), 0.3);
    let mut s0 = boxx(Vec3::new(c.x, c.y + 0.22, c.z), Vec3::new(2.0, 0.22, 2.0), step_col, FACE_ALL);
    tint_up(&mut s0, lerp_c(step_col, rgb(255, 255, 255), 0.18), scale_rgb(step_col, 0.6));
    solid.merge(&s0);
    let mut s1 = boxx(Vec3::new(c.x, c.y + 0.6, c.z), Vec3::new(1.5, 0.18, 1.5), step_col, FACE_ALL);
    tint_up(&mut s1, lerp_c(step_col, rgb(255, 255, 255), 0.18), scale_rgb(step_col, 0.6));
    solid.merge(&s1);
}

/// Reactor core: a tapered tower with an emissive band, glowing corner strips, and a beacon. (Interior.)
fn lm_reactor(c: Vec3, pal: Palette, top: f32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut solid = Mesh3::new(pal.wall);
    let mut glow = Mesh3::new(pal.accent).emit(1.0);
    lm_plinth(c, pal, &mut solid);
    let metal = lerp_c(pal.wall, rgb(170, 176, 188), 0.18);
    let base_y = c.y + 0.8;
    let top_y = top.max(base_y + 2.0);
    let h = top_y - base_y;
    let steps = 4;
    for k in 0..steps {
        let f0 = k as f32 / steps as f32;
        let r0 = 1.1 * (1.0 - f0 * 0.6);
        let cy = base_y + h * (k as f32 + 0.5) / steps as f32;
        let hy = h / steps as f32 * 0.5;
        let mut seg = boxx(Vec3::new(c.x, cy, c.z), Vec3::new(r0, hy, r0), metal, FACE_ALL);
        tint_height(&mut seg, cy - hy, cy + hy, scale_rgb(metal, 0.6), metal);
        solid.merge(&seg);
    }
    glow.merge(&boxx(Vec3::new(c.x, base_y + h * 0.5, c.z), Vec3::new(0.96, 0.14, 0.96), pal.accent, FACE_ALL));
    for (sx, sz) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
        let a = Vec3::new(c.x + sx * 0.95, base_y, c.z + sz * 0.95);
        let b = Vec3::new(c.x + sx * 0.55, top_y, c.z + sz * 0.55);
        glow.merge(&tube(a, b, 0.05, 4, pal.accent));
    }
    glow.merge(&crystal(Vec3::new(c.x, top_y + 0.2, c.z), 1.3, 0.4, 6, 7, lerp_c(pal.accent, rgb(255, 255, 255), 0.25)));
    (vec![solid, glow], vec![Obstacle { center: Vec3::new(c.x, c.y + 1.0, c.z), half: Vec3::new(1.6, 1.0, 1.6) }])
}

/// Watchtower: four legs up to a railed platform with a roof and a spotlight. You move *under* it; only
/// the legs block. (Military.)
fn lm_watchtower(c: Vec3, pal: Palette, top: f32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut solid = Mesh3::new(pal.wall);
    let mut glow = Mesh3::new(pal.accent).emit(0.9);
    let metal = lerp_c(pal.wall, rgb(150, 150, 138), 0.22);
    let plat_y = (top - 1.1).clamp(c.y + 2.2, c.y + 3.2);
    let legs = [(-1.4, -1.4), (1.4, -1.4), (-1.4, 1.4), (1.4, 1.4)];
    let mut obs = Vec::new();
    for (lx, lz) in legs {
        let a = Vec3::new(c.x + lx, c.y, c.z + lz);
        let b = Vec3::new(c.x + lx * 0.8, plat_y, c.z + lz * 0.8);
        solid.merge(&tube(a, b, 0.13, 5, metal));
        obs.push(Obstacle { center: Vec3::new(c.x + lx, c.y + 1.0, c.z + lz), half: Vec3::new(0.45, 1.0, 0.45) });
    }
    // a couple of cross-braces for a lattice read
    solid.merge(&tube(Vec3::new(c.x - 1.4, c.y + 1.4, c.z - 1.4), Vec3::new(c.x + 1.12, plat_y, c.z - 1.12), 0.05, 4, metal));
    solid.merge(&tube(Vec3::new(c.x + 1.4, c.y + 1.4, c.z + 1.4), Vec3::new(c.x - 1.12, plat_y, c.z + 1.12), 0.05, 4, metal));
    // platform + railing posts + roof
    let mut plat = boxx(Vec3::new(c.x, plat_y + 0.15, c.z), Vec3::new(1.5, 0.15, 1.5), metal, FACE_ALL);
    tint_up(&mut plat, lerp_c(metal, rgb(255, 255, 255), 0.15), scale_rgb(metal, 0.6));
    solid.merge(&plat);
    for (rx, rz) in [(-1.4, 0.0), (1.4, 0.0), (0.0, -1.4), (0.0, 1.4)] {
        solid.merge(&tube(Vec3::new(c.x + rx, plat_y + 0.3, c.z + rz), Vec3::new(c.x + rx, plat_y + 0.95, c.z + rz), 0.06, 4, metal));
    }
    let roof_y = plat_y + 1.3;
    solid.merge(&boxx(Vec3::new(c.x, roof_y, c.z), Vec3::new(1.65, 0.1, 1.65), scale_rgb(metal, 0.8), FACE_ALL));
    // searchlight under the roof
    glow.merge(&boxx(Vec3::new(c.x, roof_y - 0.25, c.z), Vec3::new(0.22, 0.16, 0.22), lerp_c(pal.accent, rgb(255, 250, 230), 0.5), FACE_ALL));
    (vec![solid, glow], obs)
}

/// Comms mast: a tall lattice mast with antenna cross-arms, a tilted dish, and a red aviation light. (Urban.)
fn lm_comms(c: Vec3, pal: Palette, top: f32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut solid = Mesh3::new(pal.wall);
    let mut glow = Mesh3::new(pal.accent).emit(1.0);
    lm_plinth(c, pal, &mut solid);
    let metal = lerp_c(pal.wall, rgb(160, 165, 175), 0.2);
    let base = Vec3::new(c.x, c.y + 0.7, c.z);
    let tip = Vec3::new(c.x, top, c.z);
    solid.merge(&tube(base, tip, 0.16, 6, metal));
    let span = top - base.y;
    for (f, ax, az) in [(0.55_f32, 1.4_f32, 0.0_f32), (0.72, -1.2, 0.0), (0.88, 0.0, 1.3)] {
        let ay = base.y + span * f;
        let end = Vec3::new(c.x + ax, ay, c.z + az);
        solid.merge(&tube(Vec3::new(c.x, ay, c.z), end, 0.06, 4, metal));
        glow.merge(&boxx(end, Vec3::new(0.12, 0.12, 0.12), pal.accent, FACE_ALL));
    }
    // a tilted dish panel near the top
    let mut dish = boxx(Vec3::new(c.x + 0.55, top - 0.5, c.z), Vec3::new(0.55, 0.55, 0.08), scale_rgb(metal, 0.9), FACE_ALL);
    rotate_y_about(&mut dish, Vec3::new(c.x, top - 0.5, c.z), 0.6);
    solid.merge(&dish);
    glow.merge(&boxx(Vec3::new(c.x, top + 0.12, c.z), Vec3::new(0.14, 0.22, 0.14), rgb(255, 60, 50), FACE_ALL));
    (vec![solid, glow], vec![Obstacle { center: Vec3::new(c.x, c.y + 1.0, c.z), half: Vec3::new(1.1, 1.0, 1.1) }])
}

/// Crashed dropship: a tilted fuselage with cockpit, tail fin, a broken wing, and engine embers. (Volcanic.)
fn lm_dropship(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut solid = Mesh3::new(pal.wall);
    let mut glow = Mesh3::new(rgb(255, 140, 60)).emit(0.9);
    let hull = lerp_c(pal.wall, rgb(72, 72, 78), 0.5);
    let cy = c.y + 0.75;
    let mut fuse = boxx(Vec3::new(c.x, cy, c.z), Vec3::new(2.6, 0.8, 1.0), hull, FACE_ALL);
    tint_up(&mut fuse, lerp_c(hull, rgb(255, 255, 255), 0.1), scale_rgb(hull, 0.5));
    solid.merge(&fuse);
    solid.merge(&boxx(Vec3::new(c.x + 2.4, cy + 0.2, c.z), Vec3::new(0.7, 0.6, 0.8), scale_rgb(hull, 0.65), FACE_ALL));
    solid.merge(&boxx(Vec3::new(c.x - 2.4, cy + 0.9, c.z), Vec3::new(0.3, 0.9, 0.7), hull, FACE_ALL));
    solid.merge(&boxx(Vec3::new(c.x - 0.3, cy + 0.2, c.z + 1.8), Vec3::new(1.4, 0.1, 1.1), scale_rgb(hull, 0.85), FACE_ALL));
    glow.merge(&crystal(Vec3::new(c.x - 0.3, c.y + 0.3, c.z + 2.4), 0.6, 0.25, 5, seed, rgb(255, 130, 50)));
    glow.merge(&crystal(Vec3::new(c.x + 1.0, c.y + 0.3, c.z - 0.6), 0.7, 0.22, 5, seed ^ 7, rgb(255, 110, 40)));
    let ang = 0.5 + (seed % 5) as f32 * 0.2;
    rotate_y_about(&mut solid, Vec3::new(c.x, cy, c.z), ang);
    rotate_y_about(&mut glow, Vec3::new(c.x, cy, c.z), ang);
    (vec![solid, glow], vec![Obstacle { center: Vec3::new(c.x, c.y + 1.0, c.z), half: Vec3::new(2.3, 1.0, 2.0) }])
}

/// Frozen monolith: a cluster of tall, faintly glowing translucent ice crystals on an ice plinth. (Ice.)
fn lm_monolith(c: Vec3, pal: Palette, top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut solid = Mesh3::new(pal.wall);
    let mut ice = Mesh3::new(lerp_c(pal.accent, rgb(220, 240, 255), 0.5)).emit(0.25);
    let mut base = boxx(Vec3::new(c.x, c.y + 0.3, c.z), Vec3::new(1.8, 0.3, 1.8), lerp_c(pal.floor, rgb(210, 230, 245), 0.4), FACE_ALL);
    tint_up(&mut base, rgb(235, 245, 255), scale_rgb(pal.floor, 0.7));
    solid.merge(&base);
    let h = (top - c.y - 0.6).max(1.6);
    let cols = lerp_c(pal.accent, rgb(225, 242, 255), 0.55);
    ice.merge(&crystal(Vec3::new(c.x, c.y + 0.5, c.z), h, 0.7, 6, seed, cols));
    ice.merge(&crystal(Vec3::new(c.x + 0.7, c.y + 0.5, c.z + 0.3), h * 0.7, 0.5, 5, seed ^ 3, cols));
    ice.merge(&crystal(Vec3::new(c.x - 0.6, c.y + 0.5, c.z - 0.5), h * 0.55, 0.45, 5, seed ^ 9, cols));
    set_alpha(&mut ice, 224);
    (vec![solid, ice], vec![Obstacle { center: Vec3::new(c.x, c.y + 1.0, c.z), half: Vec3::new(1.5, 1.0, 1.5) }])
}

/// Ruined totem arch: two mossy stone pillars and a carved lintel with a glowing rune — walk through the
/// gap. (Jungle.)
fn lm_totem(c: Vec3, pal: Palette) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut solid = Mesh3::new(pal.wall);
    let mut glow = Mesh3::new(lerp_c(pal.accent, rgb(180, 255, 160), 0.4)).emit(0.7);
    let stone = lerp_c(pal.rock, rgb(120, 130, 100), 0.3);
    let moss = lerp_c(stone, rgb(90, 140, 70), 0.5);
    let mut obs = Vec::new();
    for px in [-1.1_f32, 1.1] {
        let basep = Vec3::new(c.x + px, c.y, c.z);
        for st in cairn(basep, 3, 0.55, (px.abs() * 97.0) as u32 + 5, stone) {
            let mut s = st;
            tint_up(&mut s, moss, scale_rgb(stone, 0.6));
            solid.merge(&s);
        }
        obs.push(Obstacle { center: Vec3::new(c.x + px, c.y + 1.0, c.z), half: Vec3::new(0.62, 1.0, 0.62) });
    }
    let lin_y = c.y + 2.5;
    let mut lintel = boxx(Vec3::new(c.x, lin_y, c.z), Vec3::new(1.55, 0.32, 0.5), stone, FACE_ALL);
    tint_up(&mut lintel, moss, scale_rgb(stone, 0.6));
    solid.merge(&lintel);
    glow.merge(&boxx(Vec3::new(c.x, lin_y, c.z + 0.5), Vec3::new(0.4, 0.18, 0.05), lerp_c(pal.accent, rgb(180, 255, 160), 0.5), FACE_ALL));
    (vec![solid, glow], obs)
}

/// A ring of weathered standing stones around a low altar — a natural focal landmark for the wilds.
fn lm_stone_circle(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut solid = Mesh3::new(pal.rock);
    let stone = lerp_c(pal.rock, rgb(132, 128, 116), 0.3);
    let moss = lerp_c(stone, rgb(90, 140, 70), 0.42);
    let mut obs = Vec::new();
    let n = 7 + (seed % 4) as i32; // 7–10 stones
    let radius = 4.6;
    for i in 0..n {
        let a = (i as f32 / n as f32) * std::f32::consts::TAU;
        let (sx, sz) = (c.x + a.cos() * radius, c.z + a.sin() * radius);
        let h = 2.3 + crate::mesh::hash01(seed as i32, i) * 1.5;
        let mut slab = boxx(Vec3::new(sx, c.y + h * 0.5, sz), Vec3::new(0.46, h * 0.5, 0.3), stone, FACE_ALL);
        rotate_y_about(&mut slab, Vec3::new(sx, 0.0, sz), a + std::f32::consts::FRAC_PI_2);
        tint_up(&mut slab, moss, scale_rgb(stone, 0.6));
        solid.merge(&slab);
        obs.push(Obstacle { center: Vec3::new(sx, c.y + h * 0.5, sz), half: Vec3::new(0.55, h * 0.5, 0.55) });
    }
    let mut altar = boxx(Vec3::new(c.x, c.y + 0.32, c.z), Vec3::new(1.0, 0.32, 0.62), stone, FACE_ALL);
    tint_up(&mut altar, moss, scale_rgb(stone, 0.6));
    solid.merge(&altar);
    obs.push(Obstacle { center: Vec3::new(c.x, c.y + 0.32, c.z), half: Vec3::new(1.0, 0.32, 0.62) });
    (vec![solid], obs)
}

/// A colossal moss-covered fallen log with a torn root-ball and shelf fungi — cover and a sense of age.
fn lm_great_log(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let wood = lerp_c(rgb(96, 70, 44), pal.rock, 0.2);
    let moss = lerp_c(wood, rgb(90, 140, 70), 0.5);
    let mut solid = Mesh3::new(wood);
    let mut obs = Vec::new();
    let up = Vec3::new(0.0, 1.0, 0.0);
    let ang = (seed % 628) as f32 * 0.01;
    let dir = Vec3::new(ang.cos(), 0.0, ang.sin());
    let len = 9.0;
    let rad = 0.85;
    let a = c + dir * (-len * 0.5) + up * rad;
    let b = c + dir * (len * 0.5) + up * rad;
    let mut log = tube(a, b, rad, 10, wood);
    tint_up(&mut log, moss, scale_rgb(wood, 0.5));
    solid.merge(&log);
    // torn root ball
    let mut roots = rock(a - dir * 0.4, rad * 1.7, seed ^ 0x33, wood);
    tint_up(&mut roots, moss, scale_rgb(wood, 0.45));
    solid.merge(&roots);
    // shelf fungi stepping along the trunk
    for i in 0..6 {
        let t = 0.18 + i as f32 * 0.13;
        let p = a.lerp(b, t);
        let side = if i % 2 == 0 { 1.0 } else { -1.0 };
        let perp = Vec3::new(-dir.z, 0.0, dir.x) * (rad * 0.9 * side);
        let mut shelf = disc(Vec3::new(p.x + perp.x, p.y + rad * 0.3, p.z + perp.z), 0.3 + crate::mesh::hash01(seed as i32, i) * 0.2, 6, lerp_c(rgb(210, 196, 150), wood, 0.3));
        scale_xz(&mut shelf, 1.0, 0.5);
        solid.merge(&shelf);
    }
    for t in [0.2_f32, 0.5, 0.8] {
        let p = a.lerp(b, t);
        obs.push(Obstacle { center: Vec3::new(p.x, c.y + rad, p.z), half: Vec3::new(rad, rad, rad) });
    }
    (vec![solid], obs)
}

/// A rock cliff with a sheet of falling water spilling into a foaming pool — a dramatic natural centrepiece.
fn lm_waterfall(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let stone = lerp_c(pal.rock, rgb(112, 116, 122), 0.3);
    let moss = lerp_c(stone, rgb(82, 132, 72), 0.4);
    let mut solid = Mesh3::new(stone);
    let mut obs = Vec::new();
    let h = 6.2;
    let w = 4.2;
    let cliff_z = c.z - 1.6;
    // back wall of the cliff
    let mut wall = boxx(Vec3::new(c.x, c.y + h * 0.5, cliff_z), Vec3::new(w * 0.5, h * 0.5, 1.2), stone, FACE_ALL);
    tint_up(&mut wall, moss, scale_rgb(stone, 0.55));
    solid.merge(&wall);
    // rough rock ledges jutting from the face
    for i in 0..5 {
        let hh = crate::mesh::hash01(seed as i32, i);
        let lx = c.x + (hh - 0.5) * w;
        let ly = c.y + 0.8 + i as f32 * (h - 1.5) / 5.0;
        let mut led = rock(Vec3::new(lx, ly, cliff_z + 1.1), 0.5 + hh * 0.5, seed ^ (i as u32 * 7), stone);
        tint_up(&mut led, moss, scale_rgb(stone, 0.5));
        solid.merge(&led);
    }
    // the falling water — a translucent reflective sheet down the face into the pool
    let watercol = lerp_c(pal.sky, rgb(180, 210, 230), 0.5);
    let mut water = Mesh3::new(watercol);
    water.reflect = 0.55;
    let top = c.y + h - 0.3;
    let basey = c.y + 0.15;
    let zf = cliff_z + 1.3;
    let wxh = 1.1;
    water.push_quad(
        Vec3::new(c.x - wxh, top, zf),
        Vec3::new(c.x + wxh, top, zf),
        Vec3::new(c.x + wxh, basey, zf + 0.35),
        Vec3::new(c.x - wxh, basey, zf + 0.35),
        Vec3::new(0.0, 0.2, 1.0),
        watercol,
    );
    set_alpha(&mut water, 150);
    // foaming pool + churn at the base
    let mut foam = Mesh3::new(rgb(224, 238, 242)).emit(0.18);
    let mut pool = disc(Vec3::new(c.x, basey + 0.06, zf + 0.7), 1.7, 14, watercol);
    pool.reflect = 0.5;
    foam.merge(&pool);
    foam.merge(&disc(Vec3::new(c.x, basey + 0.12, zf + 0.45), 0.9, 10, rgb(232, 244, 248)));
    set_alpha(&mut foam, 175);
    obs.push(Obstacle { center: Vec3::new(c.x, c.y + h * 0.5, cliff_z), half: Vec3::new(w * 0.5, h * 0.5, 1.5) });
    (vec![solid, foam, water], obs)
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────────
// Signature arena setpieces — each gives its biome an unmistakable focal point so maps stop reading alike.
// ─────────────────────────────────────────────────────────────────────────────────────────────────────────

/// Industrial silo with a blown-out, smouldering breach: a towering rust-crusted cylinder, a violently torn
/// cavity near the base spilling ember light, and a worn diamond-plate service ring. (Industrial / Volcanic.)
fn lm_silo_breach(c: Vec3, pal: Palette, top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut steel = Mesh3::new(rgb(120, 74, 44));
    let mut ember = Mesh3::new(rgb(255, 120, 40)).emit(1.0);
    let h = (top - c.y).clamp(5.0, 16.0);
    let r = 2.6;
    let rust_a = rgb(150, 88, 46);
    let rust_b = rgb(78, 46, 30);
    // body as vertical panels around a ring, leaving a gap for the breach (facing -Z)
    let segs = 16;
    for i in 0..segs {
        let a0 = i as f32 / segs as f32 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / segs as f32 * std::f32::consts::TAU;
        // breach gap: skip a couple of low panels on the -Z side
        let mid = (a0 + a1) * 0.5;
        let facing_breach = mid.sin() < -0.7; // toward -Z
        let p0 = Vec3::new(c.x + a0.cos() * r, c.y, c.z + a0.sin() * r);
        let p1 = Vec3::new(c.x + a1.cos() * r, c.y, c.z + a1.sin() * r);
        let top_y = c.y + h;
        if facing_breach {
            // panel only above the breach height (torn opening below)
            let by = c.y + h * 0.42;
            let q0 = Vec3::new(p0.x, by, p0.z);
            let q1 = Vec3::new(p1.x, by, p1.z);
            steel.push_quad(q0, q1, Vec3::new(p1.x, top_y, p1.z), Vec3::new(p0.x, top_y, p0.z), (p1 - p0).cross(Vec3::UP).normalized(), lerp_c(rust_a, rust_b, 0.4));
            // jagged torn lip
            for k in 0..3 {
                let t = k as f32 / 3.0;
                let jx = p0.x + (p1.x - p0.x) * t;
                let jz = p0.z + (p1.z - p0.z) * t;
                steel.merge(&crystal(Vec3::new(jx, by - 0.3, jz), 0.5, 0.12, 4, seed ^ (k as u32 * 7), rust_b));
            }
        } else {
            let n = (p1 - p0).cross(Vec3::UP).normalized();
            steel.push_quad(p0, p1, Vec3::new(p1.x, top_y, p1.z), Vec3::new(p0.x, top_y, p0.z), n, if i & 1 == 0 { rust_a } else { rust_b });
        }
    }
    // ember cavity: a glowing recessed box inside the breach + uplight cone
    ember.merge(&boxx(Vec3::new(c.x, c.y + h * 0.2, c.z - r * 0.4), Vec3::new(r * 0.7, h * 0.18, r * 0.5), rgb(255, 140, 50), FACE_ALL));
    ember.merge(&boxx(Vec3::new(c.x, c.y + 0.1, c.z - r * 0.9), Vec3::new(r * 0.5, 0.1, 0.6), rgb(255, 90, 30), FACE_TOP));
    // rusted cap + a diamond-plate service ring partway up
    steel.merge(&tube(Vec3::new(c.x, c.y + h, c.z), Vec3::new(c.x, c.y + h + 0.5, c.z), r * 0.96, segs, rust_b));
    let ring_y = c.y + h * 0.6;
    steel.merge(&tube(Vec3::new(c.x, ring_y, c.z), Vec3::new(c.x, ring_y + 0.12, c.z), r + 0.5, segs, scale_rgb(pal.trim, 0.7)));
    (vec![steel, ember], vec![Obstacle { center: Vec3::new(c.x, c.y + h * 0.5, c.z), half: Vec3::new(r, h * 0.5, r) }])
}

/// Crashed heavy-lift cargo aircraft: an abraded olive fuselage nosed into the ground, tail boom snapped at an
/// angle, the rotor hub sheared off with carbon blade splinters littering the ground. (Urban.)
fn lm_crashed_cargo(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut hull = Mesh3::new(rgb(86, 92, 60));
    let mut dark = Mesh3::new(rgb(28, 28, 30));
    let olive = rgb(84, 90, 58);
    let bare = rgb(150, 152, 150);
    let cy = c.y + 1.0;
    // fuselage (long box) tilted nose-down
    let mut fuse = boxx(Vec3::new(c.x, cy, c.z), Vec3::new(3.4, 1.0, 1.3), olive, FACE_ALL);
    tint_up(&mut fuse, bare, scale_rgb(olive, 0.55)); // abraded top showing bare metal
    hull.merge(&fuse);
    // cockpit nose
    hull.merge(&boxx(Vec3::new(c.x + 3.2, cy - 0.3, c.z), Vec3::new(0.9, 0.7, 1.0), scale_rgb(olive, 0.8), FACE_ALL));
    // tail boom snapped at 45°
    let mut tail = boxx(Vec3::new(c.x - 3.6, cy + 0.6, c.z), Vec3::new(1.4, 0.5, 0.7), olive, FACE_ALL);
    rotate_y_about(&mut tail, Vec3::new(c.x - 2.6, cy, c.z), 0.7);
    hull.merge(&tail);
    hull.merge(&boxx(Vec3::new(c.x - 4.6, cy + 1.4, c.z), Vec3::new(0.2, 0.9, 0.7), scale_rgb(olive, 0.7), FACE_ALL)); // tail fin
    // sheared rotor hub on top + stub
    dark.merge(&tube(Vec3::new(c.x, cy + 1.0, c.z), Vec3::new(c.x, cy + 1.6, c.z), 0.35, 6, rgb(40, 40, 44)));
    // shattered carbon blade splinters on the ground
    for k in 0..12 {
        let a = hash01(k, seed as i32) * std::f32::consts::TAU;
        let d = 2.0 + hash01(k, seed as i32 ^ 9) * 5.0;
        let bx = c.x + a.cos() * d;
        let bz = c.z + a.sin() * d;
        let mut sp = boxx(Vec3::new(bx, c.y + 0.06, bz), Vec3::new(hash01(k, 3) * 0.8 + 0.3, 0.05, 0.06), rgb(24, 24, 26), FACE_ALL);
        rotate_y_about(&mut sp, Vec3::new(bx, c.y, bz), a);
        dark.merge(&sp);
    }
    let ang = 0.4 + (seed % 6) as f32 * 0.12;
    rotate_y_about(&mut hull, Vec3::new(c.x, cy, c.z), ang);
    rotate_y_about(&mut dark, Vec3::new(c.x, cy, c.z), ang);
    let _ = pal;
    (vec![hull, dark], vec![Obstacle { center: Vec3::new(c.x, cy, c.z), half: Vec3::new(3.6, 1.1, 1.6) }])
}

/// Three colossal resonant transformers throwing branching cyan-to-magenta arcs to grounding rods: bakelite
/// drums wound with tarnished copper, jagged plasma bridging the gaps. (Bunker / Warehouse foundry.)
fn lm_tesla(c: Vec3, pal: Palette, top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut drum = Mesh3::new(rgb(70, 46, 34));
    let mut copper = Mesh3::new(rgb(120, 74, 44));
    let mut arc = Mesh3::new(rgb(150, 230, 255)).emit(1.0);
    let h = (top - c.y).clamp(3.5, 9.0);
    let mut obs = Vec::new();
    let spots = [(-3.0_f32, 0.0_f32), (3.0, 0.0), (0.0, 3.0)];
    let mut tops = Vec::new();
    for (i, &(dx, dz)) in spots.iter().enumerate() {
        let bx = c.x + dx;
        let bz = c.z + dz;
        // bakelite drum
        drum.merge(&tube(Vec3::new(bx, c.y, bz), Vec3::new(bx, c.y + h * 0.7, bz), 0.9, 10, rgb(64, 42, 30)));
        // copper coil rings
        for k in 0..5 {
            let ry = c.y + h * 0.12 + k as f32 * h * 0.11;
            copper.merge(&tube(Vec3::new(bx, ry, bz), Vec3::new(bx, ry + 0.06, bz), 0.98, 10, lerp_c(rgb(150, 90, 50), rgb(50, 34, 26), 0.5)));
        }
        // top toroid + grounding rod
        let ty = c.y + h * 0.7 + 0.4;
        copper.merge(&tube(Vec3::new(bx, ty - 0.1, bz), Vec3::new(bx, ty, bz), 1.1, 10, rgb(110, 70, 44)));
        let rod = Vec3::new(bx + 1.6, c.y + h + 0.6, bz);
        copper.merge(&tube(Vec3::new(bx, ty, bz), rod, 0.05, 4, scale_rgb(pal.trim, 0.7)));
        tops.push((Vec3::new(bx, ty, bz), rod, i));
        obs.push(Obstacle { center: Vec3::new(bx, c.y + h * 0.35, bz), half: Vec3::new(1.0, h * 0.35, 1.0) });
    }
    // branching arcs: jagged emissive polylines from each toroid to its rod and to the next coil
    let zig = |arc: &mut Mesh3, a: Vec3, b: Vec3, s: u32, warm: bool| {
        let steps = 5;
        let mut prev = a;
        for k in 1..=steps {
            let t = k as f32 / steps as f32;
            let base = a + (b - a) * t;
            let off = if k == steps { Vec3::ZERO } else { Vec3::new((hash01(k, s as i32) - 0.5) * 0.8, (hash01(k, s as i32 ^ 5) - 0.5) * 0.8, (hash01(k, s as i32 ^ 9) - 0.5) * 0.8) };
            let p = base + off;
            let col = if warm { rgb(230, 120, 255) } else { rgb(150, 230, 255) };
            arc.merge(&tube(prev, p, 0.04, 3, col));
            prev = p;
        }
    };
    for (tp, rod, i) in &tops {
        zig(&mut arc, *tp, *rod, seed ^ (*i as u32 * 17), i % 2 == 1);
        let next = tops[(i + 1) % tops.len()].0;
        zig(&mut arc, *tp, next, seed ^ (*i as u32 * 31) ^ 0xB, i % 2 == 0);
    }
    (vec![drum, copper, arc], obs)
}

/// Atomic-test relic: a rusted transit bus with deflated tyres and a shattered windscreen, beside a flickering
/// dual-post neon marquee with half its tubes dead. (Urban / Military arid.)
fn lm_atomic_bus(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut body = Mesh3::new(rgb(168, 150, 64));
    let mut dark = Mesh3::new(rgb(30, 30, 32));
    let mut neon = Mesh3::new(rgb(255, 60, 50)).emit(1.0);
    let yellow = rgb(170, 150, 62);
    let cy = c.y + 1.1;
    // bus body (long rounded box) with rust streaks via tint
    let mut hull = boxx(Vec3::new(c.x, cy, c.z), Vec3::new(3.6, 1.1, 1.2), yellow, FACE_ALL);
    tint_up(&mut hull, lerp_c(yellow, rgb(120, 80, 45), 0.4), scale_rgb(yellow, 0.55));
    body.merge(&hull);
    body.merge(&boxx(Vec3::new(c.x, cy + 0.9, c.z), Vec3::new(3.2, 0.3, 1.1), scale_rgb(yellow, 0.85), FACE_ALL)); // roof
    // windscreen — pale shattered glass
    dark.merge(&boxx(Vec3::new(c.x + 3.6, cy + 0.3, c.z), Vec3::new(0.06, 0.6, 1.0), rgb(150, 165, 170), FACE_ALL));
    // deflated tyres sunk to rims
    for &dxr in &[-2.4_f32, 2.4] {
        dark.merge(&tube(Vec3::new(c.x + dxr, c.y + 0.3, c.z + 1.2), Vec3::new(c.x + dxr, c.y + 0.3, c.z + 1.45), 0.5, 8, rgb(26, 26, 28)));
        dark.merge(&tube(Vec3::new(c.x + dxr, c.y + 0.3, c.z - 1.2), Vec3::new(c.x + dxr, c.y + 0.3, c.z - 1.45), 0.5, 8, rgb(26, 26, 28)));
    }
    // marquee a few metres away
    let mx = c.x - 6.0;
    for &dzp in &[-1.2_f32, 1.2] {
        dark.merge(&tube(Vec3::new(mx, c.y, c.z + dzp), Vec3::new(mx, c.y + 3.4, c.z + dzp), 0.14, 6, rgb(96, 60, 40)));
    }
    body.merge(&boxx(Vec3::new(mx, c.y + 3.9, c.z), Vec3::new(0.3, 0.7, 2.0), scale_rgb(rgb(96, 60, 40), 0.9), FACE_ALL));
    // neon tubes — half dead (dark), half glowing red
    for k in 0..6 {
        let dz = -1.6 + k as f32 * 0.64;
        let dead = (seed >> k) & 1 == 0;
        if dead {
            dark.merge(&boxx(Vec3::new(mx + 0.32, c.y + 3.9, c.z + dz), Vec3::new(0.04, 0.5, 0.06), rgb(40, 30, 30), FACE_ALL));
        } else {
            neon.merge(&boxx(Vec3::new(mx + 0.32, c.y + 3.9, c.z + dz), Vec3::new(0.04, 0.5, 0.06), rgb(255, 60, 50), FACE_ALL));
        }
    }
    let _ = pal;
    (
        vec![body, dark, neon],
        vec![
            Obstacle { center: Vec3::new(c.x, cy, c.z), half: Vec3::new(3.6, 1.1, 1.3) },
            Obstacle { center: Vec3::new(mx, c.y + 1.6, c.z), half: Vec3::new(0.4, 1.6, 2.0) },
        ],
    )
}

/// CQB kill-pit: splinter-blasted plywood mock-up walls peppered with training-paint splatter, stacked red &
/// navy shipping containers, and a camo net stretched overhead. (Military.)
fn lm_killpit(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut ply = Mesh3::new(rgb(178, 148, 96));
    let mut steel = Mesh3::new(rgb(150, 70, 55));
    let mut splat = Mesh3::new(rgb(240, 130, 40)).emit(0.25);
    let mut net = Mesh3::new(rgb(96, 100, 62));
    let wood = rgb(180, 150, 95);
    let mut obs = Vec::new();
    // plywood mock-up: two walls with a cut-out doorway
    for (wx, wz, rot) in [(c.x - 2.5, c.z, 0.0_f32), (c.x, c.z - 2.5, 1.5708)] {
        // left + right of a doorway gap
        for side in [-1.0_f32, 1.0] {
            let mut panel = boxx(Vec3::new(wx + side * 1.4, c.y + 1.3, wz), Vec3::new(0.9, 1.3, 0.12), wood, FACE_ALL);
            rotate_y_about(&mut panel, Vec3::new(wx, c.y, wz), rot);
            ply.merge(&panel);
        }
        // lintel above the doorway
        let mut lintel = boxx(Vec3::new(wx, c.y + 2.4, wz), Vec3::new(2.3, 0.3, 0.12), scale_rgb(wood, 0.9), FACE_ALL);
        rotate_y_about(&mut lintel, Vec3::new(wx, c.y, wz), rot);
        ply.merge(&lintel);
        obs.push(Obstacle { center: Vec3::new(wx, c.y + 1.3, wz), half: Vec3::new(2.3, 1.3, 0.3) });
    }
    // paint-splatter pockmarks on the plywood
    for k in 0..40 {
        let wx = c.x - 2.5 + (hash01(k, seed as i32) - 0.5) * 4.0;
        let wy = c.y + 0.4 + hash01(k, seed as i32 ^ 3) * 2.2;
        let wz = c.z + 0.14;
        let blue = k & 1 == 0;
        let col = if blue { rgb(80, 120, 200) } else { rgb(240, 130, 40) };
        splat.merge(&boxx(Vec3::new(wx, wy, wz), Vec3::splat(0.06), col, FACE_ALL));
    }
    // stacked shipping containers (red oxide + navy)
    for (i, col) in [rgb(150, 70, 55), rgb(45, 55, 90)].iter().enumerate() {
        let bx = c.x + 3.4;
        let bz = c.z + 1.0 + i as f32 * 2.4;
        let mut cont = boxx(Vec3::new(bx, c.y + 1.2, bz), Vec3::new(2.6, 1.2, 1.1), *col, FACE_NO_BOTTOM);
        tint_up(&mut cont, lerp_c(*col, rgb(255, 255, 255), 0.08), scale_rgb(*col, 0.6));
        steel.merge(&cont);
        obs.push(Obstacle { center: Vec3::new(bx, c.y + 1.2, bz), half: Vec3::new(2.6, 1.2, 1.2) });
    }
    // camo net overhead — a translucent mottled plane on tall poles
    let ny = c.y + 4.4;
    for &(px, pz) in &[(c.x - 5.0, c.z - 5.0), (c.x + 5.0, c.z - 5.0), (c.x - 5.0, c.z + 5.0), (c.x + 5.0, c.z + 5.0)] {
        net.merge(&tube(Vec3::new(px, c.y, pz), Vec3::new(px, ny, pz), 0.12, 5, rgb(70, 60, 44)));
    }
    let cols = [rgb(150, 140, 100), rgb(96, 100, 62), rgb(80, 70, 50)];
    let div = 6;
    let span = 10.0;
    for iz in 0..div {
        for ix in 0..div {
            let x0 = c.x - span * 0.5 + ix as f32 * span / div as f32;
            let z0 = c.z - span * 0.5 + iz as f32 * span / div as f32;
            let s = span / div as f32;
            let col = cols[((ix + iz) as usize + (seed as usize)) % cols.len()];
            net.push_quad(
                Vec3::new(x0, ny, z0),
                Vec3::new(x0 + s, ny, z0),
                Vec3::new(x0 + s, ny, z0 + s),
                Vec3::new(x0, ny, z0 + s),
                Vec3::UP,
                col,
            );
        }
    }
    set_alpha(&mut net, 150);
    let _ = pal;
    (vec![ply, steel, splat, net], obs)
}

/// Hilltop granite manor on a raised terrace (high-ground spatial anchor): weathered grey block walls, a
/// moss-slate gable roof, shattered bay windows, and a wrought-iron spear-picket gate fronted by dead turf
/// and boxwood hedges — a tight courtyard chokepoint. (Urban / Military temperate.)
fn lm_manor(c: Vec3, pal: Palette, top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let granite = rgb(120, 122, 118);
    let mut stone = Mesh3::new(granite);
    let mut roof = Mesh3::new(rgb(70, 84, 68));
    let mut glass = Mesh3::new(rgb(46, 58, 64));
    let mut hedge = Mesh3::new(rgb(48, 74, 42));
    let mut iron = Mesh3::new(rgb(28, 28, 30));
    stone.merge(&boxx(Vec3::new(c.x, c.y + 0.25, c.z), Vec3::new(6.2, 0.25, 5.2), scale_rgb(granite, 0.82), FACE_ALL));
    let bh = (top - c.y).clamp(2.6, 5.2) * 0.5;
    let body_c = Vec3::new(c.x, c.y + 0.5 + bh, c.z);
    let mut body = boxx(body_c, Vec3::new(4.2, bh, 3.4), granite, FACE_ALL);
    tint_up(&mut body, lerp_c(granite, rgb(162, 162, 156), 0.2), scale_rgb(granite, 0.58));
    stone.merge(&body);
    let ry = c.y + 0.5 + bh * 2.0;
    let ridge = ry + 1.5;
    roof.merge(&ramp(Vec3::new(c.x - 4.6, ry, c.z - 3.7), Vec3::new(c.x - 4.6, ry, c.z + 3.7), Vec3::new(c.x, ridge, c.z + 3.7), Vec3::new(c.x, ridge, c.z - 3.7), rgb(72, 86, 70)));
    roof.merge(&ramp(Vec3::new(c.x, ridge, c.z - 3.7), Vec3::new(c.x, ridge, c.z + 3.7), Vec3::new(c.x + 4.6, ry, c.z + 3.7), Vec3::new(c.x + 4.6, ry, c.z - 3.7), rgb(64, 78, 62)));
    for row in 0..2 {
        for k in 0..3 {
            let wz = c.z - 2.0 + k as f32 * 2.0;
            let wy = c.y + 0.9 + row as f32 * bh;
            let shattered = (seed >> (row * 3 + k)) & 1 == 0;
            let col = if shattered { rgb(150, 158, 162) } else { rgb(46, 58, 64) };
            glass.merge(&boxx(Vec3::new(c.x + 4.22, wy, wz), Vec3::new(0.04, 0.6, 0.7), col, FACE_ALL));
        }
    }
    let gx = c.x + 7.0;
    iron.merge(&tube(Vec3::new(gx, c.y, c.z - 2.6), Vec3::new(gx, c.y + 2.2, c.z - 2.6), 0.12, 6, rgb(30, 30, 32)));
    iron.merge(&tube(Vec3::new(gx, c.y, c.z + 2.6), Vec3::new(gx, c.y + 2.2, c.z + 2.6), 0.12, 6, rgb(30, 30, 32)));
    for k in 0..9 {
        let pz = c.z - 2.2 + k as f32 * 0.55;
        iron.merge(&boxx(Vec3::new(gx, c.y + 1.0, pz), Vec3::new(0.04, 1.0, 0.04), rgb(24, 24, 26), FACE_ALL));
        iron.merge(&crystal(Vec3::new(gx, c.y + 2.0, pz), 0.26, 0.05, 4, seed ^ k as u32, rgb(24, 24, 26)));
    }
    stone.merge(&{
        let mut t = Mesh3::new(rgb(150, 142, 86));
        t.push_quad(Vec3::new(c.x + 3.0, c.y + 0.05, c.z - 2.0), Vec3::new(gx, c.y + 0.05, c.z - 2.0), Vec3::new(gx, c.y + 0.05, c.z + 2.0), Vec3::new(c.x + 3.0, c.y + 0.05, c.z + 2.0), Vec3::UP, rgb(150, 142, 86));
        t
    });
    for &sz in &[-3.4_f32, 3.4] {
        for k in 0..4 {
            let hx = c.x + 3.2 + k as f32 * 1.1;
            hedge.merge(&leaf_cluster(Vec3::new(hx, c.y + 0.5, c.z + sz), 0.7, seed ^ k as u32, rgb(48, 74, 42)));
        }
    }
    let _ = pal;
    (
        vec![stone, roof, glass, hedge, iron],
        vec![
            Obstacle { center: body_c, half: Vec3::new(4.4, bh, 3.6) },
            Obstacle { center: Vec3::new(gx, c.y + 1.1, c.z), half: Vec3::new(0.3, 1.1, 2.7) },
        ],
    )
}

/// Movie backlot: a sun-faded canvas saloon facade peeling to bare 2x4 framing, jammed flush against a warped
/// reflective plexiglass storefront, power cables snaking the dirt, and arc lights on C-stands throwing
/// visible beams — flat fakes that terminate sightlines. (Urban / Military arid.)
fn lm_backlot(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut canvas = Mesh3::new(rgb(150, 120, 80));
    let mut wood = Mesh3::new(rgb(150, 120, 80));
    let mut plex = Mesh3::new(rgb(120, 145, 155));
    plex.reflect = 0.6;
    let mut glow = Mesh3::new(rgb(255, 250, 228)).emit(1.0);
    let mut dark = Mesh3::new(rgb(26, 26, 28));
    let mut saloon = boxx(Vec3::new(c.x - 2.6, c.y + 2.2, c.z), Vec3::new(0.1, 2.2, 3.0), rgb(154, 122, 82), FACE_ALL);
    tint_up(&mut saloon, scale_rgb(rgb(154, 122, 82), 0.9), scale_rgb(rgb(154, 122, 82), 0.6));
    canvas.merge(&saloon);
    let timber = rgb(150, 118, 78);
    for k in 0..5 {
        wood.merge(&boxx(Vec3::new(c.x - 2.48, c.y + 0.6 + k as f32 * 0.9, c.z), Vec3::new(0.05, 0.07, 2.8), timber, FACE_ALL));
    }
    for k in 0..3 {
        wood.merge(&boxx(Vec3::new(c.x - 2.48, c.y + 2.0, c.z - 2.0 + k as f32 * 2.0), Vec3::new(0.05, 2.0, 0.07), timber, FACE_ALL));
    }
    plex.merge(&boxx(Vec3::new(c.x + 1.0, c.y + 1.9, c.z + 1.6), Vec3::new(0.08, 1.9, 1.9), rgb(122, 148, 158), FACE_ALL));
    let alu = rgb(150, 152, 150);
    wood.merge(&boxx(Vec3::new(c.x + 1.0, c.y + 1.9, c.z - 0.3), Vec3::new(0.1, 1.9, 0.06), alu, FACE_ALL));
    wood.merge(&boxx(Vec3::new(c.x + 1.0, c.y + 1.9, c.z + 3.5), Vec3::new(0.1, 1.9, 0.06), alu, FACE_ALL));
    for k in 0..4 {
        let z0 = c.z - 3.0 + k as f32 * 1.6;
        dark.merge(&tube(Vec3::new(c.x - 3.0, c.y + 0.06, z0), Vec3::new(c.x + 3.0, c.y + 0.06, z0 + 0.6), 0.08, 4, rgb(20, 20, 22)));
    }
    let mut obs = Vec::new();
    for &(lx, lz) in &[(c.x + 0.5, c.z - 4.0), (c.x + 3.5, c.z + 3.5)] {
        dark.merge(&tube(Vec3::new(lx, c.y, lz), Vec3::new(lx, c.y + 4.2, lz), 0.1, 5, rgb(30, 30, 32)));
        glow.merge(&boxx(Vec3::new(lx, c.y + 4.2, lz), Vec3::splat(0.4), rgb(255, 250, 228), FACE_ALL));
        for mut s in light_shaft(lx, lz, c.y + 4.0, c.y, rgb(255, 248, 220), seed ^ lx as u32) {
            set_alpha(&mut s, 60);
            glow.merge(&s);
        }
        obs.push(Obstacle { center: Vec3::new(lx, c.y + 1.0, lz), half: Vec3::new(0.4, 1.0, 0.4) });
    }
    obs.push(Obstacle { center: Vec3::new(c.x - 2.6, c.y + 2.0, c.z), half: Vec3::new(0.3, 2.0, 3.0) });
    obs.push(Obstacle { center: Vec3::new(c.x + 1.0, c.y + 1.9, c.z + 1.6), half: Vec3::new(0.3, 1.9, 2.0) });
    let _ = pal;
    (vec![canvas, wood, plex, glow, dark], obs)
}

/// A generator pit: a recessed iron-grating shaft sunk into the floor, around glowing neon-cyan plasma coils
/// on a slowly-rotating core, ringed by brushed-steel catwalks with sparking arc faults. (Industrial/Bunker.)
fn lm_generator_pit(c: Vec3, pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut steel = Mesh3::new(scale_rgb(pal.wall, 0.7));
    let mut iron = Mesh3::new(rgb(28, 30, 34));
    let mut plasma = Mesh3::new(rgb(90, 220, 255)).emit(1.0);
    // recessed rim grating (a dark ring disc) + a lip
    iron.merge(&disc(Vec3::new(c.x, c.y + 0.05, c.z), 6.5, 28, rgb(26, 28, 32)));
    iron.merge(&tube(Vec3::new(c.x, c.y, c.z), Vec3::new(c.x, c.y + 0.4, c.z), 6.5, 28, scale_rgb(pal.wall, 0.6)));
    // central core column with stacked glowing plasma coils
    let core_h = 5.0;
    iron.merge(&tube(Vec3::new(c.x, c.y - 2.5, c.z), Vec3::new(c.x, c.y + core_h, c.z), 1.4, 12, rgb(34, 36, 42)));
    for k in 0..6 {
        let ry = c.y - 1.0 + k as f32 * (core_h + 1.0) / 6.0;
        plasma.merge(&tube(Vec3::new(c.x, ry, c.z), Vec3::new(c.x, ry + 0.22, c.z), 1.9 + (k as f32 * 0.7).sin() * 0.3, 14, rgb(90, 220, 255)));
    }
    // two catwalks crossing the pit
    for rot in [0.0_f32, std::f32::consts::FRAC_PI_2] {
        let mut cw = boxx(Vec3::new(c.x, c.y + 0.35, c.z), Vec3::new(6.3, 0.12, 0.7), lerp_c(pal.trim, rgb(180, 188, 196), 0.3), FACE_ALL);
        rotate_y_about(&mut cw, c, rot);
        steel.merge(&cw);
    }
    // sparking arc faults around the rim
    for k in 0..10 {
        let a = k as f32 / 10.0 * std::f32::consts::TAU;
        if hash01(k, seed as i32) < 0.5 {
            plasma.merge(&crystal(Vec3::new(c.x + a.cos() * 6.0, c.y + 0.5, c.z + a.sin() * 6.0), 0.4, 0.06, 4, k as u32, rgb(150, 230, 255)));
        }
    }
    (vec![iron, steel, plasma], vec![Obstacle { center: Vec3::new(c.x, c.y + core_h * 0.5, c.z), half: Vec3::new(1.6, core_h * 0.5, 1.6) }])
}

/// A smelting line setpiece: spherical iron crucibles glowing cherry-red under a slag crust, overhead pipes
/// dripping molten metal, and a towering heat-blasted hydraulic press. (Volcanic / Industrial.)
fn lm_smelting(c: Vec3, pal: Palette, top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut steel = Mesh3::new(scale_rgb(pal.wall, 0.8));
    let mut slag = Mesh3::new(rgb(40, 34, 32));
    let mut molten = Mesh3::new(rgb(255, 110, 40)).emit(1.0);
    let mut obs = Vec::new();
    // crucibles
    for (i, &(dx, dz)) in [(-3.5_f32, 0.0_f32), (3.5, 0.5), (0.0, -3.5)].iter().enumerate() {
        let r = 1.6 + (i as f32 * 0.2);
        let cc = Vec3::new(c.x + dx, c.y + r, c.z + dz);
        // glowing sphere, then a darker slag cap over the top half
        molten.merge(&uv_sphere(cc, r, 12, 8, rgb(210, 70, 34)));
        slag.merge(&uv_sphere(Vec3::new(cc.x, cc.y + r * 0.4, cc.z), r * 0.92, 12, 6, rgb(34, 30, 30)));
        obs.push(Obstacle { center: cc, half: Vec3::new(r, r, r) });
        // a molten splatter pool at the base
        molten.merge(&disc(Vec3::new(cc.x, c.y + 0.04, cc.z), r * 0.8, 12, rgb(220, 90, 36)));
    }
    // overhead pipe arch dripping molten
    let py = (top - 0.5).min(c.y + 6.0);
    steel.merge(&tube(Vec3::new(c.x - 6.0, py, c.z), Vec3::new(c.x + 6.0, py, c.z), 0.35, 8, scale_rgb(pal.wall, 0.7)));
    for k in 0..4 {
        let dx = -4.0 + k as f32 * 2.6;
        molten.merge(&tube(Vec3::new(c.x + dx, py - 0.3, c.z), Vec3::new(c.x + dx, c.y + 0.5 + hash01(k, seed as i32) * 1.5, c.z), 0.08, 4, rgb(255, 130, 50)));
    }
    // heat-blasted press (grey casting with bare oxidised-yellow metal up top)
    let press_h = (top - c.y).clamp(2.5, 5.0);
    let mut press = boxx(Vec3::new(c.x, c.y + press_h, c.z + 5.0), Vec3::new(1.6, press_h, 1.6), rgb(96, 100, 104), FACE_ALL);
    tint_up(&mut press, rgb(150, 130, 60), scale_rgb(rgb(96, 100, 104), 0.6));
    steel.merge(&press);
    obs.push(Obstacle { center: Vec3::new(c.x, c.y + press_h, c.z + 5.0), half: Vec3::new(1.6, press_h, 1.6) });
    (vec![steel, slag, molten], obs)
}

/// Clone-vat core: a central stack of vertical transparent cylinders filled with backlit translucent green
/// fluid, ringed by a brushed walkway — a sickly underwater glow rising from the shaft. (Alien / Bunker.)
fn lm_clone_vats(c: Vec3, pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut steel = Mesh3::new(scale_rgb(pal.wall, 0.75));
    let mut glass = Mesh3::new(rgb(70, 200, 120));
    glass.reflect = 0.3;
    let mut fluid = Mesh3::new(rgb(60, 210, 130)).emit(0.85);
    // a ring of tall vat cylinders around a central column
    let ring = 10;
    for k in 0..ring {
        let a = k as f32 / ring as f32 * std::f32::consts::TAU;
        let vx = c.x + a.cos() * 2.6;
        let vz = c.z + a.sin() * 2.6;
        let h = 4.0 + hash01(k, seed as i32) * 1.5;
        fluid.merge(&tube(Vec3::new(vx, c.y + 0.2, vz), Vec3::new(vx, c.y + h, vz), 0.55, 8, rgb(60, 210, 130)));
        glass.merge(&tube(Vec3::new(vx, c.y + 0.1, vz), Vec3::new(vx, c.y + h + 0.2, vz), 0.62, 8, rgb(120, 220, 160)));
    }
    set_alpha(&mut glass, 120);
    // central machinery column + base ring walkway
    steel.merge(&tube(Vec3::new(c.x, c.y, c.z), Vec3::new(c.x, c.y + 5.5, c.z), 1.0, 10, scale_rgb(pal.wall, 0.7)));
    steel.merge(&disc(Vec3::new(c.x, c.y + 0.08, c.z), 4.2, 24, lerp_c(pal.trim, rgb(200, 206, 212), 0.3)));
    fluid.merge(&disc(Vec3::new(c.x, c.y + 0.05, c.z), 1.4, 12, rgb(80, 230, 150))); // glow up the shaft
    (vec![steel, glass, fluid], vec![Obstacle { center: Vec3::new(c.x, c.y + 2.7, c.z), half: Vec3::new(1.1, 2.7, 1.1) }])
}

/// Titan's ribcage: three colossal fossilised ribs arching over the lane as bone cover — chalky pale-yellow
/// limestone deeply pitted, sheared gouges exposing grey marrow, marble slabs and green fig roots at the base.
fn lm_ribcage(c: Vec3, _pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let bone = rgb(214, 206, 168);
    let marrow = rgb(120, 116, 108);
    let mut ribs = Mesh3::new(bone);
    let mut detail = Mesh3::new(marrow);
    let mut ground = Mesh3::new(rgb(196, 190, 176));
    let mut obs = Vec::new();
    ground.merge(&disc(Vec3::new(c.x, c.y + 0.04, c.z), 7.0, 20, rgb(188, 184, 176)));
    for k in 0..3 {
        let zo = -4.5 + k as f32 * 4.5; // three ribs spaced down the lane
        let r = 5.5 - (k as f32 - 1.0).abs() * 0.6; // middle rib tallest
        let segs = 9;
        let mut prev: Option<Vec3> = None;
        for i in 0..=segs {
            let t = i as f32 / segs as f32;
            let ang = t * std::f32::consts::PI; // 0..PI arc up and over the lane
            let p = Vec3::new(c.x + (-ang.cos()) * r, c.y + ang.sin() * r, c.z + zo);
            if let Some(pp) = prev {
                let mid = (p + pp) * 0.5;
                let len = (p - pp).len() * 0.5 + 0.2;
                let thick = 0.55 + (1.0 - (t - 0.5).abs() * 2.0) * 0.25; // thicker mid-span
                ribs.merge(&boxx(mid, Vec3::new(thick, len, thick), bone, FACE_ALL));
                if i == segs / 2 {
                    detail.merge(&boxx(mid + Vec3::new(0.0, 0.0, thick), Vec3::new(thick * 0.7, len * 0.5, 0.15), marrow, FACE_ALL));
                }
            }
            prev = Some(p);
        }
        obs.push(Obstacle { center: Vec3::new(c.x - r * 0.7, c.y + 1.4, c.z + zo), half: Vec3::new(0.9, 1.4, 0.9) });
        obs.push(Obstacle { center: Vec3::new(c.x + r * 0.7, c.y + 1.4, c.z + zo), half: Vec3::new(0.9, 1.4, 0.9) });
    }
    for k in 0..6 {
        let a = hash01(k, seed as i32) * std::f32::consts::TAU;
        let d = 2.0 + hash01(k, 9) * 4.0;
        let base = Vec3::new(c.x + a.cos() * d, c.y, c.z + a.sin() * d);
        ground.merge(&tube(base, base + Vec3::new(a.cos() * 1.5, 0.4, a.sin() * 1.5), 0.18, 4, rgb(86, 120, 66)));
    }
    tint_height(&mut ribs, c.y, c.y + 6.0, scale_rgb(bone, 0.7), lerp_c(bone, rgb(255, 255, 255), 0.08));
    (vec![ground, ribs, detail], obs)
}

/// Snapped obelisk: a massive four-sided black-granite monument toppled and broken across the choke, embedded
/// blackened shrapnel, drippy melted gold at the apex, tan ballistic barriers piled on the windward side.
fn lm_obelisk(c: Vec3, _pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let granite = rgb(30, 30, 38);
    let mut stone = Mesh3::new(granite);
    stone.reflect = 0.18;
    let mut gold = Mesh3::new(rgb(220, 180, 70)).emit(0.4);
    let mut barriers = Mesh3::new(rgb(176, 158, 120));
    let mut shrap = Mesh3::new(rgb(24, 22, 24));
    let mut obs = Vec::new();
    let segs = 6;
    let len = 9.0;
    for i in 0..segs {
        let t = i as f32 / segs as f32;
        let along = -len * 0.5 + len * (i as f32 + 0.5) / segs as f32;
        let w = 1.3 * (1.0 - t * 0.6);
        let cx = c.x + along * 0.92;
        let cy = c.y + 0.8 + along * 0.28;
        stone.merge(&boxx(Vec3::new(cx, cy, c.z), Vec3::new(w * 0.7, w, w), granite, FACE_ALL));
        if i % 2 == 0 {
            shrap.merge(&boxx(Vec3::new(cx, cy + w * 0.6, c.z + w * 0.7), Vec3::splat(0.12), rgb(24, 22, 24), FACE_ALL));
        }
    }
    obs.push(Obstacle { center: Vec3::new(c.x, c.y + 1.2, c.z), half: Vec3::new(len * 0.45, 1.3, 1.3) });
    gold.merge(&disc(Vec3::new(c.x - len * 0.45, c.y + 0.06, c.z), 0.9, 10, rgb(220, 180, 70)));
    for k in 0..4 {
        let bx = c.x + 1.5 + k as f32 * 0.1;
        let bz = c.z - 2.0 - k as f32 * 0.9;
        let mut b = boxx(Vec3::new(bx, c.y + 0.55, bz), Vec3::new(1.1, 0.55, 0.5), rgb(176, 158, 120), FACE_ALL);
        rotate_y_about(&mut b, Vec3::new(bx, c.y, bz), hash01(k, seed as i32) * 0.6);
        barriers.merge(&b);
        obs.push(Obstacle { center: Vec3::new(bx, c.y + 0.55, bz), half: Vec3::new(1.1, 0.55, 0.5) });
    }
    (vec![stone, gold, shrap, barriers], obs)
}

/// Bifrost dais: a raised dark-basalt drum crowned by a shattered hexagonal pane of iridescent prismatic
/// glass, jagged shards pointing up like caltrops, a diffuse white-magic glow bleeding through the cracks.
fn lm_bifrost(c: Vec3, _pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let basalt = rgb(52, 58, 72);
    let mut stone = Mesh3::new(basalt);
    let mut glow = Mesh3::new(rgb(220, 230, 255)).emit(0.9);
    let mut shards = Mesh3::new(rgb(150, 200, 240)).emit(0.7);
    stone.merge(&tube(Vec3::new(c.x, c.y, c.z), Vec3::new(c.x, c.y + 1.1, c.z), 4.2, 18, basalt));
    stone.merge(&disc(Vec3::new(c.x, c.y + 1.12, c.z), 4.2, 18, scale_rgb(basalt, 1.1)));
    glow.merge(&disc(Vec3::new(c.x, c.y + 1.16, c.z), 2.4, 14, rgb(230, 238, 255)));
    for k in 0..12 {
        let a = k as f32 / 12.0 * std::f32::consts::TAU;
        let d = 0.6 + hash01(k, seed as i32) * 1.7;
        let tint = if k % 2 == 0 { rgb(150, 210, 255) } else { rgb(220, 160, 240) };
        shards.merge(&crystal(Vec3::new(c.x + a.cos() * d, c.y + 1.16, c.z + a.sin() * d), 0.6 + hash01(k, 9) * 0.9, 0.18, 4, k as u32, tint));
    }
    (vec![stone, glow, shards], vec![Obstacle { center: Vec3::new(c.x, c.y + 0.55, c.z), half: Vec3::new(4.2, 0.55, 4.2) }])
}

/// Dragon carcass: the colossal decaying body of a reptilian beast spanning the gorge — a curved spine of
/// dull-green keratin over exposed pale marrow, rib arcs, talons sunk in as pillars, and toxic smoke from the maw.
fn lm_dragon(c: Vec3, _pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let scale = rgb(64, 92, 70);
    let flesh = rgb(150, 138, 128);
    let mut body = Mesh3::new(scale);
    let mut meat = Mesh3::new(flesh);
    let mut smoke = Mesh3::new(rgb(150, 170, 90));
    let mut obs = Vec::new();
    let segs = 12;
    let span = 11.0;
    for i in 0..segs {
        let t = i as f32 / (segs - 1) as f32;
        let along = -span * 0.5 + span * t;
        let sway = (t * std::f32::consts::PI * 1.5).sin() * 2.0;
        let sy = c.y + 1.6 + (t * std::f32::consts::PI).sin() * 1.2;
        let p = Vec3::new(c.x + along, sy, c.z + sway);
        body.merge(&uv_sphere(p, 0.9 - t * 0.3, 8, 6, scale));
        if i % 3 == 1 {
            meat.merge(&uv_sphere(p + Vec3::new(0.0, -0.2, 0.6), 0.5, 6, 5, flesh));
            body.merge(&tube(p, Vec3::new(p.x, c.y, p.z + 2.0), 0.18, 4, scale_rgb(scale, 0.8)));
            obs.push(Obstacle { center: Vec3::new(p.x, c.y + 0.9, p.z), half: Vec3::new(0.9, 0.9, 1.4) });
        }
    }
    let head = Vec3::new(c.x - span * 0.5 - 1.0, c.y + 1.4, c.z);
    body.merge(&uv_sphere(head, 1.3, 8, 6, scale_rgb(scale, 1.1)));
    for k in 0..5 {
        let h = k as f32 * 0.6;
        let mut puff = uv_sphere(head + Vec3::new(-1.2 - h * 0.4, 0.6 + h, 0.0), 0.6 + h * 0.25, 6, 4, rgb(150, 170, 90));
        set_alpha(&mut puff, 70);
        smoke.merge(&puff);
    }
    for &zs in &[-3.0_f32, 3.0] {
        let base = Vec3::new(c.x + 1.0, c.y, c.z + zs);
        body.merge(&tube(base, base + Vec3::new(-1.2, 3.0, 0.0), 0.35, 5, scale_rgb(scale, 0.7)));
    }
    let _ = seed;
    (vec![body, meat, smoke], obs)
}

/// Colosseum wall: a curved arc of weathered travertine arches ringing the arena, rusted iron clamps cracking
/// the stone, a portcullis frozen mid-descent, and tattered blood-dark velvet banners hanging from the spans.
fn lm_colosseum(c: Vec3, _pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let trav = rgb(196, 182, 150);
    let mut stone = Mesh3::new(trav);
    let mut iron = Mesh3::new(rgb(120, 78, 50));
    let mut banner = Mesh3::new(rgb(78, 28, 30));
    let mut obs = Vec::new();
    let radius = 8.0;
    let arches = 7;
    let arc_span = std::f32::consts::PI * 1.1;
    for k in 0..arches {
        let a = -arc_span * 0.5 + arc_span * k as f32 / (arches - 1) as f32;
        let px = c.x + a.cos() * radius;
        let pz = c.z + a.sin() * radius;
        stone.merge(&boxx(Vec3::new(px, c.y + 2.4, pz), Vec3::new(0.7, 2.4, 0.7), trav, FACE_ALL));
        obs.push(Obstacle { center: Vec3::new(px, c.y + 2.4, pz), half: Vec3::new(0.7, 2.4, 0.7) });
        if k + 1 < arches {
            let a2 = -arc_span * 0.5 + arc_span * (k as f32 + 1.0) / (arches - 1) as f32;
            let mx = c.x + ((a + a2) * 0.5).cos() * radius;
            let mz = c.z + ((a + a2) * 0.5).sin() * radius;
            stone.merge(&boxx(Vec3::new(mx, c.y + 4.5, mz), Vec3::new(1.6, 0.4, 0.5), scale_rgb(trav, 0.95), FACE_ALL));
            iron.merge(&boxx(Vec3::new(px, c.y + 3.4, pz), Vec3::new(0.12, 0.3, 0.74), rgb(120, 78, 50), FACE_ALL));
            if hash01(k, seed as i32) < 0.6 {
                let mut bn = boxx(Vec3::new(mx, c.y + 3.2, mz), Vec3::new(0.55, 1.1, 0.04), rgb(78, 28, 30), FACE_ALL);
                tint_height(&mut bn, c.y + 2.0, c.y + 4.3, scale_rgb(rgb(78, 28, 30), 0.5), rgb(78, 28, 30));
                banner.merge(&bn);
            }
        }
    }
    let cgz = c.z + radius;
    for b in 0..5 {
        let bx = c.x - 1.2 + b as f32 * 0.6;
        iron.merge(&boxx(Vec3::new(bx, c.y + 2.8, cgz), Vec3::new(0.06, 1.6, 0.06), rgb(110, 70, 46), FACE_ALL));
    }
    tint_height(&mut stone, c.y, c.y + 5.0, scale_rgb(trav, 0.7), lerp_c(trav, rgb(255, 255, 255), 0.06));
    (vec![stone, iron, banner], obs)
}

/// Blighted Watchtower ruin: a low circular ring of rough-cut rune-carved blocks crowning the summit, with
/// crumbling steps and an interior fire-pit of cold ash and blackened bones. (Open summit biomes.)
fn lm_watchtower_ring(c: Vec3, pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let stone_c = scale_rgb(pal.rock, 0.92);
    let mut stone = Mesh3::new(stone_c);
    let mut rune = Mesh3::new(rgb(20, 26, 18)); // black-lichen runes in the carving
    let mut hearth = Mesh3::new(rgb(150, 70, 30)).emit(0.5);
    let mut obs = Vec::new();
    let r = 4.5;
    let blocks = 14;
    for k in 0..blocks {
        let a = k as f32 / blocks as f32 * std::f32::consts::TAU;
        // leave a gap for crumbling steps
        if (a - 0.0).abs() < 0.45 {
            continue;
        }
        let bx = c.x + a.cos() * r;
        let bz = c.z + a.sin() * r;
        let h = 1.1 + hash01(k, seed as i32) * 0.4;
        let mut b = boxx(Vec3::new(bx, c.y + h, bz), Vec3::new(0.7, h, 0.7), stone_c, FACE_ALL);
        rotate_y_about(&mut b, Vec3::new(bx, c.y, bz), a);
        stone.merge(&b);
        rune.merge(&boxx(Vec3::new(bx + a.cos() * 0.72, c.y + h, bz + a.sin() * 0.72), Vec3::new(0.04, h * 0.6, 0.5), rgb(20, 26, 18), FACE_ALL));
        obs.push(Obstacle { center: Vec3::new(bx, c.y + h, bz), half: Vec3::new(0.7, h, 0.7) });
    }
    // crumbling steps up to the ring entrance
    for s in 0..3 {
        stone.merge(&boxx(Vec3::new(c.x + r + 0.6 + s as f32 * 0.5, c.y + 0.15 + s as f32 * 0.18, c.z), Vec3::new(0.5, 0.15, 0.7), scale_rgb(stone_c, 0.9), FACE_ALL));
    }
    // interior hearth: a shallow river-stone pit of grey ash + blackened bones
    hearth.merge(&disc(Vec3::new(c.x, c.y + 0.06, c.z), 1.0, 10, rgb(150, 70, 30)));
    stone.merge(&crate::fsm::detail::debris(Vec3::new(c.x, c.y, c.z), 1.4, 10, rgb(30, 28, 26), seed ^ 0xB0)); // bones/ash lumps
    tint_height(&mut stone, c.y, c.y + 1.5, scale_rgb(stone_c, 0.7), stone_c);
    (vec![stone, rune, hearth], obs)
}

/// Fallen Capital dome: a massive shattered stone dome lying face-down across the boulevard, its interior
/// ribs exposed like a ribcage, on cracked soot-stained marble. Blocks the central sightline. (Urban/Military.)
fn lm_fallen_dome(c: Vec3, _pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let marble = rgb(150, 146, 138); // soot-stained white marble (limestone biome palette ignored: marble is signature)
    let rib = scale_rgb(marble, 0.7);
    let mut shell = Mesh3::new(marble);
    let mut ribs = Mesh3::new(rib);
    // the dome: an inverted (face-down) hemisphere — take the lower half of a squashed sphere
    let mut dome = uv_sphere(Vec3::new(c.x, c.y + 2.6, c.z), 4.0, 14, 8, marble);
    for v in &mut dome.verts {
        let dy = v.y - (c.y + 2.6);
        v.y = c.y + 2.6 - dy.abs() * 0.8; // flatten + flip so the bowl opens downward
    }
    shell.merge(&dome);
    // exposed interior ribs radiating from the apex
    for k in 0..8 {
        let a = k as f32 / 8.0 * std::f32::consts::TAU;
        ribs.merge(&tube(Vec3::new(c.x, c.y + 2.4, c.z), Vec3::new(c.x + a.cos() * 3.6, c.y + 0.3, c.z + a.sin() * 3.6), 0.12, 4, rib));
    }
    // cracked marble paving + rubble with exposed rebar pins
    let mut ground = Mesh3::new(scale_rgb(marble, 1.05));
    ground.merge(&disc(Vec3::new(c.x, c.y + 0.04, c.z), 6.0, 18, scale_rgb(marble, 1.05)));
    ground.merge(&crate::fsm::detail::debris(Vec3::new(c.x + 4.0, c.y, c.z), 2.0, 10, marble, seed));
    ground.merge(&crate::fsm::detail::rebar(Vec3::new(c.x + 4.0, c.y + 0.2, c.z), Vec3::new(0.0, 0.0, 1.0), 4, seed ^ 0x9));
    tint_height(&mut shell, c.y, c.y + 4.0, scale_rgb(marble, 0.55), marble); // soot low, paler up top
    let obs = vec![Obstacle { center: Vec3::new(c.x, c.y + 1.4, c.z), half: Vec3::new(4.0, 1.4, 4.0) }];
    (vec![ground, shell, ribs], obs)
}

/// Slaughter Camp: a toppled rough-oak siege tower split into jagged sections with twisted iron bands, rows of
/// sharpened ash-wood stakes forming a choke, and scattered helmets/chainmail in the mire. (Mud biomes.)
fn lm_siege_camp(c: Vec3, pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let oak = rgb(96, 74, 46);
    let mut timber = Mesh3::new(oak);
    let mut iron = Mesh3::new(rgb(96, 70, 48));
    let mut obs = Vec::new();
    // toppled siege tower lying on its side — three jagged sections
    for k in 0..3 {
        let along = -3.0 + k as f32 * 3.0;
        let mut sec = boxx(Vec3::new(c.x + along, c.y + 1.0 + hash01(k, seed as i32) * 0.3, c.z), Vec3::new(1.4, 1.0, 1.2), scale_rgb(oak, 0.9 + hash01(k, 3) * 0.2), FACE_ALL);
        rotate_y_about(&mut sec, Vec3::new(c.x + along, c.y, c.z), (hash01(k, 7) - 0.5) * 0.5);
        timber.merge(&sec);
        // twisted iron reinforcing band
        iron.merge(&boxx(Vec3::new(c.x + along, c.y + 1.0, c.z), Vec3::new(1.45, 0.12, 1.25), rgb(96, 70, 48), FACE_ALL));
        obs.push(Obstacle { center: Vec3::new(c.x + along, c.y + 1.0, c.z), half: Vec3::new(1.4, 1.0, 1.2) });
    }
    // sharpened ash-wood stake rows forming a choke
    let mut stakes = Mesh3::new(rgb(150, 134, 96));
    for row in 0..2 {
        for i in 0..6 {
            let sx = c.x - 3.0 + i as f32 * 1.2;
            let sz = c.z + 4.0 + row as f32 * 1.0;
            stakes.merge(&tube(Vec3::new(sx, c.y, sz), Vec3::new(sx + 0.2, c.y + 1.3, sz), 0.1, 4, rgb(150, 134, 96)));
        }
    }
    // detritus: crushed helmets + chainmail half-submerged
    timber.merge(&crate::fsm::detail::debris(Vec3::new(c.x, c.y, c.z + 2.0), 3.5, 14, rgb(70, 70, 66), seed ^ 0x4C));
    tint_height(&mut timber, c.y, c.y + 2.0, scale_rgb(oak, 0.6), oak);
    let _ = pal;
    (vec![timber, iron, stakes], obs)
}

/// Culvert Breach: a thick stone siege wall blown open — a massive jagged breach with blackened, vitrified
/// edges, iron braziers of roaring coals on the wall-walk above, and splintered timber barricades at the base.
fn lm_breach_wall(c: Vec3, pal: Palette, top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let stone_c = scale_rgb(pal.wall, 1.0);
    let mut stone = Mesh3::new(stone_c);
    let mut char_c = Mesh3::new(rgb(26, 22, 22)); // blackened/obsidian breach edge
    let mut coals = Mesh3::new(rgb(240, 120, 40)).emit(1.0);
    let mut obs = Vec::new();
    let wall_h = (top - c.y).clamp(4.0, 7.0);
    let half_len = 9.0;
    let gap = 2.6; // the breach
    // wall in segments, leaving a ragged gap in the centre
    for seg in [-1.0_f32, 1.0] {
        let x0 = seg * gap;
        let x1 = seg * half_len;
        let cxw = c.x + (x0 + x1) * 0.5;
        let w = (x1 - x0).abs() * 0.5;
        let mut block = boxx(Vec3::new(cxw, c.y + wall_h, c.z), Vec3::new(w, wall_h, 1.2), stone_c, FACE_ALL);
        // jagged top via per-vertex noise
        for v in &mut block.verts {
            if v.y > c.y + wall_h * 1.5 {
                v.y -= hash01((v.x * 4.0) as i32, seed as i32) * 1.2;
            }
        }
        stone.merge(&block);
        // blackened breach edge facing the gap
        char_c.merge(&boxx(Vec3::new(c.x + seg * gap, c.y + wall_h * 0.6, c.z), Vec3::new(0.25, wall_h * 0.6, 1.25), rgb(26, 22, 22), FACE_ALL));
        obs.push(Obstacle { center: Vec3::new(cxw, c.y + wall_h, c.z), half: Vec3::new(w, wall_h, 1.2) });
        // an iron brazier of roaring coals on the wall-walk
        coals.merge(&disc(Vec3::new(c.x + seg * (half_len - 1.5), c.y + wall_h * 2.0 + 0.1, c.z), 0.4, 8, rgb(255, 140, 50)));
    }
    // splintered timber barricades bowing at the base of the breach
    let mut timber = Mesh3::new(rgb(90, 70, 46));
    for i in 0..5 {
        let tx = c.x - 2.0 + i as f32 * 1.0;
        timber.merge(&boxx(Vec3::new(tx, c.y + 0.7, c.z + 1.4), Vec3::new(0.12, 0.7, 0.1), rgb(90, 70, 46), FACE_ALL));
    }
    tint_height(&mut stone, c.y, c.y + wall_h * 2.0, scale_rgb(stone_c, 0.55), stone_c);
    (vec![stone, char_c, timber, coals], obs)
}

/// Glowing gel core (the hollow-forest world's heart): a rocky island cradling a great translucent gel orb
/// that pulses with inner light, gripped by woody root tendrils arcing up out of the ground. The orb is the
/// scene's light source, so it's strongly emissive; the island blocks movement so you fight around it.
fn lm_gel_core(c: Vec3, pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let glow_c = lerp_c(pal.accent, rgb(120, 255, 200), 0.55); // bright gel teal-green
    let rock_c = scale_rgb(pal.rock, 0.9);
    let root_c = scale_rgb(pal.wall, 0.5);
    let mut island = Mesh3::new(rock_c);
    let mut roots = Mesh3::new(root_c);
    let mut core = Mesh3::new(glow_c).emit(0.9);
    let mut nucleus = Mesh3::new(rgb(220, 255, 235)).emit(1.0);

    // rocky island base — a low faceted mound
    island.merge(&rock(Vec3::new(c.x, c.y + 0.3, c.z), 3.4, seed ^ 0xC0, rock_c));
    island.merge(&disc(Vec3::new(c.x, c.y + 0.06, c.z), 4.2, 16, scale_rgb(rock_c, 0.85)));

    // the gel orb floating just above the island, with a brighter nucleus inside
    let core_c = Vec3::new(c.x, c.y + 3.4, c.z);
    let mut orb = uv_sphere(core_c, 2.3, 14, 10, glow_c);
    set_alpha(&mut orb, 150);
    core.merge(&orb);
    nucleus.merge(&uv_sphere(core_c, 1.0, 10, 7, rgb(220, 255, 235)));

    // root tendrils gripping the orb: tubes arcing from the island rim up and over the core
    let tendrils = 6;
    for k in 0..tendrils {
        let a = k as f32 / tendrils as f32 * std::f32::consts::TAU + 0.3;
        let foot = Vec3::new(c.x + a.cos() * 3.6, c.y + 0.2, c.z + a.sin() * 3.6);
        let mid = Vec3::new(c.x + a.cos() * 2.0, c.y + 3.2, c.z + a.sin() * 2.0);
        let crown = Vec3::new(c.x + a.cos() * 0.7, c.y + 5.2, c.z + a.sin() * 0.7);
        roots.merge(&tube(foot, mid, 0.18, 5, root_c));
        roots.merge(&tube(mid, crown, 0.12, 5, scale_rgb(root_c, 1.1)));
    }
    tint_height(&mut roots, c.y, c.y + 5.2, scale_rgb(root_c, 0.7), root_c);

    let obs = vec![Obstacle { center: Vec3::new(c.x, c.y + 1.2, c.z), half: Vec3::new(3.6, 1.2, 3.6) }];
    (vec![island, roots, core, nucleus], obs)
}

/// Overturned cryo-tank (the Cryo-Lab centerpiece): a massive cylindrical brushed-steel tank torn off its
/// wall mounts and lying on its side with a deep buckled dent, hanging ribbed coaxial cables whose copper
/// ends are crystallised green, a frozen puddle of cloudy-blue agar with shattered glass shards, and a
/// vintage CRT whose spider-webbed screen still glows faintly green. Obsessive wear sells the abandonment.
fn lm_cryo_tank(c: Vec3, pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let steel = lerp_c(pal.trim, rgb(176, 186, 196), 0.5); // brushed stainless
    let dark = scale_rgb(steel, 0.55);
    let mut tank = Mesh3::new(steel);
    let mut detail = Mesh3::new(dark);
    let mut glow = Mesh3::new(rgb(120, 255, 150)).emit(0.8);

    // The tank: a big horizontal cylinder lying on its side, slightly skewed, with welded end caps.
    let axis = Vec3::new(0.82, 0.0, 0.34); // skewed lengthwise
    let a = Vec3::new(c.x - axis.x * 3.2, c.y + 1.15, c.z - axis.z * 3.2);
    let b = Vec3::new(c.x + axis.x * 3.2, c.y + 1.15, c.z + axis.z * 3.2);
    tank.merge(&tube(a, b, 1.15, 14, steel));
    tank.merge(&disc(a, 1.15, 14, scale_rgb(steel, 0.8)));
    tank.merge(&disc(b, 1.15, 14, scale_rgb(steel, 0.8)));
    // ribbing bands along the jacket
    for k in 1..6 {
        let t = k as f32 / 6.0;
        let p = Vec3::new(a.x + (b.x - a.x) * t, a.y, a.z + (b.z - a.z) * t);
        detail.merge(&tube(Vec3::new(p.x, p.y, p.z), Vec3::new(p.x, p.y + 0.01, p.z), 1.2, 12, dark));
    }
    // buckled dent: a darker crushed panel pressed into the side where it hit the floor
    let dent = Vec3::new(c.x + 0.6, c.y + 0.55, c.z - 0.4);
    detail.merge(&boxx(dent, Vec3::new(0.7, 0.45, 0.5), scale_rgb(steel, 0.4), FACE_ALL));

    // torn-off wall mounts: bent brackets near one end, snapped
    for side in [-1.0f32, 1.0] {
        let mp = Vec3::new(a.x + side * 0.4, c.y + 0.2, a.z + 0.9 * side);
        detail.merge(&boxx(mp, Vec3::new(0.18, 0.2, 0.5), dark, FACE_ALL));
    }

    // hanging ribbed coaxial cables from above, copper ends crystallised green
    for k in 0..7 {
        let hx = c.x - 1.6 + k as f32 * 0.5;
        let hz = c.z + 1.8 + (k as f32 * 1.3).sin() * 0.4;
        let topp = Vec3::new(hx, c.y + 5.0, hz);
        let endp = Vec3::new(hx + (k as f32).sin() * 0.3, c.y + 1.6 + (k % 3) as f32 * 0.3, hz);
        detail.merge(&tube(topp, endp, 0.05, 4, rgb(28, 28, 32)));
        glow.merge(&uv_sphere(endp, 0.07, 5, 4, rgb(90, 220, 120))); // oxidised copper tip
    }

    // frozen agar puddle: a translucent cloudy-blue disc with shattered glass shards frozen in
    let mut agar = disc(Vec3::new(c.x - 2.0, c.y + 0.04, c.z + 1.4), 1.3, 16, rgb(150, 200, 230));
    set_alpha(&mut agar, 130);
    agar.reflect = 0.4;
    let shards = crate::fsm::detail::shards(Vec3::new(c.x - 2.0, c.y, c.z + 1.4), Vec3::new(1.0, 0.0, 0.3), 7, seed ^ 0x5A);

    // vintage CRT on a low desk, screen spider-webbed but glowing a faint green scanline
    let desk = Vec3::new(c.x + 2.6, c.y + 0.45, c.z - 1.6);
    detail.merge(&boxx(desk, Vec3::new(0.7, 0.45, 0.5), scale_rgb(pal.wall, 0.7), FACE_ALL));
    let crt = Vec3::new(desk.x, desk.y + 0.7, desk.z);
    tank.merge(&boxx(crt, Vec3::new(0.4, 0.32, 0.36), rgb(196, 188, 168), FACE_ALL));
    let mut screen = boxx(Vec3::new(crt.x, crt.y + 0.02, crt.z + 0.36), Vec3::new(0.3, 0.24, 0.02), rgb(40, 120, 60), FACE_ALL);
    screen.emissive = 0.7;
    glow.merge(&screen);

    let obs = vec![Obstacle { center: Vec3::new(c.x, c.y + 1.15, c.z), half: Vec3::new(3.4, 1.15, 2.0) }];
    (vec![tank, detail, agar, shards, glow], obs)
}

/// Abandoned ore cart on rails (the salt-mine setpiece): a heavily rusted steel cart sitting on parallel
/// steel tracks, half-filled with jagged translucent pink crystal, with dark hydraulic fluid pooled beneath it
/// on the pale ground and creosote-soaked timber shoring beams streaking black nearby. Industrial decay with a
/// jarring splash of crystal colour and a stark fluid stain.
fn lm_ore_cart(c: Vec3, pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let rust = rgb(150, 78, 44); // oxidized orange-red
    let steel = scale_rgb(pal.trim, 0.8);
    let timber = rgb(58, 42, 30);
    let mut cart = Mesh3::new(rust);
    let mut rails = Mesh3::new(steel);
    let mut crystals = Mesh3::new(rgb(232, 150, 196)).emit(0.18); // translucent pink
    let mut fluid = Mesh3::new(rgb(34, 24, 18));
    let mut wood = Mesh3::new(timber);

    // parallel steel tracks running through the setpiece
    for side in [-0.7f32, 0.7] {
        rails.merge(&tube(Vec3::new(c.x - 4.0, c.y + 0.06, c.z + side), Vec3::new(c.x + 4.0, c.y + 0.06, c.z + side), 0.07, 5, steel));
    }
    for k in -7..=7 {
        let tx = c.x + k as f32 * 0.55;
        rails.merge(&boxx(Vec3::new(tx, c.y + 0.03, c.z), Vec3::new(0.06, 0.03, 0.95), scale_rgb(timber, 1.1), FACE_TOP));
    }

    // the cart: a rusted open box on small wheels, tilted slightly
    let body = Vec3::new(c.x, c.y + 0.62, c.z);
    cart.merge(&boxx(body, Vec3::new(1.1, 0.5, 0.78), rust, FACE_NO_BOTTOM));
    // hollow it a touch (inner darker lip)
    cart.merge(&boxx(Vec3::new(body.x, body.y + 0.46, body.z), Vec3::new(1.0, 0.06, 0.68), scale_rgb(rust, 0.6), FACE_TOP));
    for sx in [-0.85f32, 0.85] {
        for sz in [-0.6f32, 0.6] {
            cart.merge(&tube(Vec3::new(c.x + sx, c.y + 0.18, c.z + sz), Vec3::new(c.x + sx, c.y + 0.18, c.z + sz + 0.02), 0.2, 8, steel));
        }
    }

    // half-filled with jagged pink crystal formations spilling over the rim
    for k in 0..6 {
        let a = k as f32 / 6.0 * std::f32::consts::TAU + seed as f32 * 0.1;
        let px = c.x + a.cos() * 0.5;
        let pz = c.z + a.sin() * 0.32;
        let hgt = 0.4 + crate::mesh::hash01(k as i32, seed as i32) * 0.5;
        let mut cr = crystal(Vec3::new(px, c.y + 0.9, pz), hgt, 0.16, 5, seed + k, rgb(232, 150, 196));
        set_alpha(&mut cr, 200);
        cr.reflect = 0.3;
        crystals.merge(&cr);
    }

    // dark hydraulic fluid pooled under the cart — a stark stain on the pale ground
    let mut pool = disc(Vec3::new(c.x + 0.4, c.y + 0.03, c.z + 0.3), 1.1, 16, rgb(34, 24, 18));
    set_alpha(&mut pool, 180);
    pool.reflect = 0.5;
    fluid.merge(&pool);

    // creosote-soaked timber shoring a few metres off (a tunnel mouth), streaking black down the beams
    let mouth = Vec3::new(c.x - 3.4, c.y, c.z - 2.2);
    for side in [-0.9f32, 0.9] {
        wood.merge(&boxx(Vec3::new(mouth.x + side, c.y + 1.1, mouth.z), Vec3::new(0.16, 1.1, 0.16), timber, FACE_ALL));
    }
    wood.merge(&boxx(Vec3::new(mouth.x, c.y + 2.15, mouth.z), Vec3::new(1.1, 0.16, 0.16), timber, FACE_ALL));
    tint_height(&mut wood, c.y, c.y + 2.2, scale_rgb(timber, 0.5), timber); // tar drip darker at the base

    let obs = vec![Obstacle { center: Vec3::new(c.x, c.y + 0.6, c.z), half: Vec3::new(1.2, 0.6, 0.85) }];
    (vec![rails, cart, crystals, fluid, wood], obs)
}

/// The Dry Fountain (Sunken Villa POI): an octagonal pale-limestone basin, empty, its interior coated in
/// cracked lime-green algae, with a headless rearing-horse statue at the centre (jagged neck stump, no head)
/// and a toppled flaking-white wrought-iron chair against the rim. Circular cover with no escape — a focal
/// choke for the courtyard.
fn lm_dry_fountain(c: Vec3, _pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let stone = rgb(214, 206, 188);
    let algae = rgb(120, 150, 92);
    let grey = rgb(98, 94, 92);
    let iron = rgb(220, 216, 208);
    let mut basin = Mesh3::new(stone);
    let mut inner = Mesh3::new(algae);
    let mut statue = Mesh3::new(grey);
    let mut chair = Mesh3::new(iron);
    let r = 2.7;
    for k in 0..8 {
        let a = k as f32 / 8.0 * std::f32::consts::TAU;
        let (bx, bz) = (c.x + a.cos() * r, c.z + a.sin() * r);
        basin.merge(&boxx(Vec3::new(bx, c.y + 0.45, bz), Vec3::new(0.62, 0.45, 0.62), dither(stone, k as i32, 0.1), FACE_ALL));
    }
    let mut algae_floor = disc(Vec3::new(c.x, c.y + 0.07, c.z), r - 0.55, 16, algae);
    algae_floor.texturize(0.8, 0.18, seed); // cracked, mottled
    inner.merge(&algae_floor);
    // rearing horse: body, hind leg, two raised forelegs, and a broken neck stump (no head)
    statue.merge(&boxx(Vec3::new(c.x, c.y + 1.15, c.z), Vec3::new(0.32, 0.6, 0.92), grey, FACE_ALL));
    statue.merge(&tube(Vec3::new(c.x, c.y + 0.1, c.z + 0.55), Vec3::new(c.x, c.y + 1.05, c.z + 0.35), 0.13, 6, grey));
    for fx in [-0.22f32, 0.22] {
        statue.merge(&tube(Vec3::new(c.x + fx, c.y + 1.5, c.z - 0.45), Vec3::new(c.x + fx, c.y + 2.05, c.z - 0.95), 0.1, 6, grey));
    }
    statue.merge(&tube(Vec3::new(c.x, c.y + 1.65, c.z - 0.2), Vec3::new(c.x, c.y + 2.15, c.z - 0.5), 0.17, 6, scale_rgb(grey, 0.78)));
    // toppled iron chair on its side against the basin
    let ch = Vec3::new(c.x + r - 0.1, c.y, c.z + 0.7);
    for off in [-0.22f32, 0.22] {
        chair.merge(&tube(Vec3::new(ch.x, c.y + 0.06, ch.z + off), Vec3::new(ch.x + 0.55, c.y + 0.06, ch.z + off), 0.03, 4, iron));
        chair.merge(&tube(Vec3::new(ch.x + 0.55, c.y + 0.06, ch.z + off), Vec3::new(ch.x + 0.55, c.y + 0.5, ch.z + off), 0.03, 4, iron));
    }
    let obs = vec![Obstacle { center: Vec3::new(c.x, c.y + 0.5, c.z), half: Vec3::new(r, 0.5, r) }];
    (vec![basin, inner, statue, chair], obs)
}

/// The Cooling Vat (Server Tomb POI): a massive mirror-finish stainless cylinder dominating the silo, sweating
/// condensation, ringed by a grated catwalk, with a glass porthole near the top glowing neon-blue from the
/// coolant churning inside. The central chokepoint you must expose yourself to cross.
fn lm_cooling_vat(c: Vec3, pal: Palette, top: f32, _seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let steel = rgb(190, 198, 205);
    let h = (top - c.y).clamp(3.0, 6.0);
    let mut vat = Mesh3::new(steel);
    let mut glow = Mesh3::new(rgb(60, 180, 255)).emit(0.9);
    let mut walk = Mesh3::new(scale_rgb(pal.floor, 0.7));
    let mut body = tube(Vec3::new(c.x, c.y, c.z), Vec3::new(c.x, c.y + h, c.z), 1.6, 18, steel);
    body.reflect = 0.85; // mirror finish
    vat.merge(&body);
    // glass porthole near the top + a blue wash on the catwalk
    glow.merge(&boxx(Vec3::new(c.x, c.y + h - 0.8, c.z + 1.58), Vec3::new(0.42, 0.42, 0.06), rgb(60, 180, 255), FACE_ALL));
    // grated catwalk ring around the tank
    let cr = 2.5;
    for k in 0..18 {
        let a = k as f32 / 18.0 * std::f32::consts::TAU;
        let (wx, wz) = (c.x + a.cos() * cr, c.z + a.sin() * cr);
        walk.merge(&boxx(Vec3::new(wx, c.y + h * 0.5, wz), Vec3::new(0.42, 0.06, 0.42), scale_rgb(pal.floor, 0.7), FACE_TOP));
    }
    let obs = vec![Obstacle { center: Vec3::new(c.x, c.y + h * 0.5, c.z), half: Vec3::new(1.6, h * 0.5, 1.6) }];
    (vec![vat, walk, glow], obs)
}

/// The Dangling Gondola (Rusting Midway POI): the rusted red lattice-steel base of a derelict Ferris wheel,
/// with one yellow fiberglass passenger capsule snapped free and swaying 15 feet up on a single frayed cable.
/// The only cover in the deadly centre lane.
fn lm_dangling_gondola(c: Vec3, _pal: Palette, top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let red = rgb(150, 52, 44);
    let yellow = rgb(214, 188, 70);
    let cabl = rgb(70, 66, 62);
    let h = (top - c.y).clamp(4.0, 8.0);
    let mut wheel = Mesh3::new(red);
    let mut gond = Mesh3::new(yellow);
    let mut cable = Mesh3::new(cabl);
    for s in [-1.0f32, 1.0] {
        wheel.merge(&tube(Vec3::new(c.x + s * 2.2, c.y, c.z), Vec3::new(c.x, c.y + h, c.z), 0.17, 6, red));
    }
    // a partial rim arc, lattice-style
    let rr = h * 0.72;
    let mut prev = Vec3::new(c.x + rr, c.y + h * 0.5, c.z);
    for k in 1..=7 {
        let a = k as f32 / 12.0 * std::f32::consts::TAU;
        let p = Vec3::new(c.x + a.cos() * rr, c.y + h * 0.5 + a.sin() * rr, c.z);
        wheel.merge(&tube(prev, p, 0.1, 5, dither(red, k as i32, 0.12)));
        prev = p;
    }
    // the dangling capsule on a frayed cable
    let gy = c.y + h * 0.42;
    cable.merge(&tube(Vec3::new(c.x, c.y + h * 0.7, c.z), Vec3::new(c.x, gy + 0.55, c.z), 0.03, 4, cabl));
    let cap = uv_sphere(Vec3::new(c.x, gy, c.z), 0.72, 12, 8, dither(yellow, seed as i32, 0.1));
    gond.merge(&cap);
    let obs = vec![Obstacle { center: Vec3::new(c.x, c.y + 1.0, c.z), half: Vec3::new(2.4, 1.0, 0.7) }];
    (vec![wheel, gond, cable], obs)
}

/// The Derailed Car (Ghost Terminus POI): a heavy maroon steel subway car, dusted grey, derailed and slewed
/// across the tunnel at a violent angle with grilled (barred) windows and a door forced crookedly open onto a
/// dark graffiti interior. A massive immovable cover object that splits the tunnel into edge chokes.
fn lm_derailed_car(c: Vec3, _pal: Palette, _top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let maroon = rgb(96, 40, 44);
    let dust = rgb(150, 146, 140);
    let bar = rgb(80, 70, 64);
    let mut car = Mesh3::new(maroon);
    let mut bars = Mesh3::new(bar);
    let mut body = boxx(Vec3::new(c.x, c.y + 1.3, c.z), Vec3::new(1.3, 1.3, 4.6), maroon, FACE_ALL);
    tint_height(&mut body, c.y + 2.0, c.y + 2.7, dust, maroon); // construction dust settled on top
    car.merge(&body);
    // dark graffiti interior visible through the forced-open door (a recessed darker panel)
    car.merge(&boxx(Vec3::new(c.x + 1.25, c.y + 1.2, c.z + 2.0), Vec3::new(0.1, 0.8, 0.7), rgb(30, 28, 32), FACE_ALL));
    // grilled windows: vertical bars along both sides
    for k in 0..6 {
        let z = c.z - 3.6 + k as f32 * 1.45;
        for sx in [-1.33f32, 1.33] {
            bars.merge(&tube(Vec3::new(c.x + sx, c.y + 1.25, z), Vec3::new(c.x + sx, c.y + 2.05, z), 0.04, 4, bar));
        }
    }
    let mut all = car;
    all.merge(&bars);
    rotate_y_about(&mut all, c, 0.52 + (seed & 3) as f32 * 0.03); // slewed off the rails
    let obs = vec![Obstacle { center: Vec3::new(c.x, c.y + 1.3, c.z), half: Vec3::new(1.6, 1.3, 4.7) }];
    (vec![all], obs)
}

/// A great concrete gravity **dam**: a row of stepped buttress segments curving gently upstream, with a
/// walkable crest (standable high ground spanning the map), a lowered central spillway, and impounded water
/// held behind it. Each segment is one box = one collision slab, so the crest you see is the crest you stand on.
fn lm_dam(c: Vec3, pal: Palette, top: f32, _seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut meshes = Vec::new();
    let mut obs = Vec::new();
    let concrete = scale_rgb(pal.wall, 0.96);
    let h = (top - c.y).clamp(4.0, 7.0);
    let span = 17.0;
    let segs = 6usize;
    let segw = span / segs as f32;
    let mid = segs / 2;
    for i in 0..segs {
        let t = i as f32 / (segs - 1) as f32 - 0.5; // -0.5..0.5
        let x = c.x + t * span;
        let z = c.z + t * t * 3.2 - 0.8; // gentle upstream arch
        let spill = i == mid - 1 || i == mid;
        let seg_h = if spill { h * 0.5 } else { h };
        let half = Vec3::new(segw * 0.5 * 0.97, seg_h * 0.5, 1.5);
        let center = Vec3::new(x, c.y + seg_h * 0.5, z);
        meshes.push(boxx(center, half, dither(concrete, i as i32 * 7, 0.09), FACE_ALL));
        obs.push(Obstacle { center, half });
        // downstream stepped buttress face for mass (mesh only — the slab above is the collision)
        let bz = z + 1.5 + 0.9;
        meshes.push(ramp(
            Vec3::new(x - half.x, c.y, bz + 0.9),
            Vec3::new(x + half.x, c.y, bz + 0.9),
            Vec3::new(x + half.x, c.y + seg_h * 0.7, z + 1.5),
            Vec3::new(x - half.x, c.y + seg_h * 0.7, z + 1.5),
            scale_rgb(concrete, 0.86),
        ));
    }
    // crest walkway — a thin standable deck along the full top, a sightline-dominating catwalk
    let crest = Vec3::new(c.x, c.y + h + 0.12, c.z - 0.4);
    let chalf = Vec3::new(span * 0.5, 0.12, 1.0);
    meshes.push(boxx(crest, chalf, scale_rgb(concrete, 1.05), FACE_ALL));
    obs.push(Obstacle { center: crest, half: chalf });
    // impounded reservoir behind the wall (upstream, -z), a still translucent sheet
    let water = scale_rgb(pal.accent, 0.85);
    let mut sheet = panel(Vec3::new(c.x, c.y + h * 0.62, c.z - 7.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, -1.0), span * 1.05, 12.0, water);
    set_alpha(&mut sheet, 120);
    sheet.reflect = 0.55;
    meshes.push(sheet);
    (meshes, obs)
}

/// A **railway** running across the arena: two steel rails on timber ties, a raised boarding platform you can
/// fight from, a signal mast, and a buffer stop. The platform is a standable slab; the rails are low enough to
/// vault. Long sightline down the line, cover across it.
fn lm_railway(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut meshes = Vec::new();
    let mut obs = Vec::new();
    let steel = rgb(120, 120, 132);
    let wood = rgb(86, 62, 40);
    let len = 18.0;
    let gauge = 1.5;
    // ties
    let ties = (len / 1.1) as i32;
    for i in 0..ties {
        let z = c.z - len * 0.5 + i as f32 * (len / ties as f32);
        meshes.push(boxx(Vec3::new(c.x, c.y + 0.08, z), Vec3::new(gauge * 0.75, 0.08, 0.22), dither(wood, i, 0.14), FACE_ALL));
    }
    // rails as thin tubes
    for s in [-1.0f32, 1.0] {
        meshes.push(tube(Vec3::new(c.x + s * gauge * 0.5, c.y + 0.18, c.z - len * 0.5), Vec3::new(c.x + s * gauge * 0.5, c.y + 0.18, c.z + len * 0.5), 0.07, 5, steel));
    }
    // boarding platform alongside (standable)
    let pcx = c.x + 2.6;
    let phalf = Vec3::new(1.4, 0.55, len * 0.32);
    let pcenter = Vec3::new(pcx, c.y + 0.55, c.z);
    meshes.push(boxx(pcenter, phalf, scale_rgb(pal.floor, 0.92), FACE_ALL));
    obs.push(Obstacle { center: pcenter, half: phalf });
    // signal mast with a colored lamp
    let mx = c.x - 2.2;
    meshes.push(tube(Vec3::new(mx, c.y, c.z + len * 0.3), Vec3::new(mx, c.y + 3.4, c.z + len * 0.3), 0.1, 6, steel));
    let lamp = if seed & 1 == 0 { rgb(230, 70, 60) } else { rgb(90, 220, 110) };
    let mut glow = uv_sphere(Vec3::new(mx, c.y + 3.3, c.z + len * 0.3 - 0.3), 0.28, 8, 6, lamp);
    glow.emissive = 0.8;
    meshes.push(glow);
    obs.push(Obstacle { center: Vec3::new(mx, c.y + 1.2, c.z + len * 0.3), half: Vec3::new(0.18, 1.2, 0.18) });
    // buffer stop at one end
    let bz = c.y;
    let bstop = Vec3::new(c.x, bz + 0.5, c.z + len * 0.5 + 0.3);
    let bhalf = Vec3::new(gauge * 0.8, 0.5, 0.3);
    meshes.push(boxx(bstop, bhalf, scale_rgb(steel, 0.7), FACE_ALL));
    obs.push(Obstacle { center: bstop, half: bhalf });
    (meshes, obs)
}

/// A working tiered **fountain**: a low octagonal basin pool, a central pedestal, an upper bowl, and a jet of
/// translucent water. The basin rim is standable low cover; the pedestal is a hard sightline break.
fn lm_fountain(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut meshes = Vec::new();
    let mut obs = Vec::new();
    let stone = scale_rgb(pal.trim, 1.0);
    let water = rgb(120, 190, 220);
    // outer basin rim (ring of short slabs → standable octagon)
    let n = 8;
    let rr = 2.6;
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let px = c.x + a.cos() * rr;
        let pz = c.z + a.sin() * rr;
        let half = Vec3::new(0.55, 0.45, 0.3);
        let mut seg = boxx(Vec3::new(px, c.y + 0.45, pz), half, dither(stone, i, 0.1), FACE_ALL);
        rotate_y_about(&mut seg, Vec3::new(px, c.y, pz), a + std::f32::consts::FRAC_PI_2);
        meshes.push(seg);
        obs.push(Obstacle { center: Vec3::new(px, c.y + 0.45, pz), half });
    }
    // pooled water inside the basin
    let mut pool = disc(Vec3::new(c.x, c.y + 0.4, c.z), rr - 0.45, 20, water);
    set_alpha(&mut pool, 150);
    pool.reflect = 0.5;
    meshes.push(pool);
    // central pedestal + upper bowl
    meshes.push(tube(Vec3::new(c.x, c.y + 0.4, c.z), Vec3::new(c.x, c.y + 1.7, c.z), 0.35, 8, stone));
    obs.push(Obstacle { center: Vec3::new(c.x, c.y + 1.0, c.z), half: Vec3::new(0.45, 1.0, 0.45) });
    meshes.push(disc(Vec3::new(c.x, c.y + 1.75, c.z), 0.9, 16, stone));
    let mut topwater = disc(Vec3::new(c.x, c.y + 1.8, c.z), 0.7, 16, water);
    set_alpha(&mut topwater, 150);
    meshes.push(topwater);
    // the jet — a slim translucent column
    let mut jet = tube(Vec3::new(c.x, c.y + 1.8, c.z), Vec3::new(c.x, c.y + 2.9 + (seed & 3) as f32 * 0.2, c.z), 0.12, 6, water);
    set_alpha(&mut jet, 130);
    jet.emissive = 0.15;
    meshes.push(jet);
    (meshes, obs)
}

/// A rock **cliff** with a standable mid-**ledge** and a talus ramp up to it — vertical play you mantle and
/// hold. The cliff body is a tall blocker; the ledge and talus steps are standable.
fn lm_cliff(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut meshes = Vec::new();
    let mut obs = Vec::new();
    let stone = pal.rock;
    // the main cliff mass — a tall rough rock wall
    let body_h = 6.5;
    let bw = 5.0;
    let bcenter = Vec3::new(c.x, c.y + body_h * 0.5, c.z - 1.5);
    meshes.push(rock(bcenter, bw, seed, scale_rgb(stone, 0.95)));
    meshes.push(rock(Vec3::new(c.x - 2.0, c.y + body_h * 0.4, c.z - 2.2), bw * 0.7, seed ^ 0x31, scale_rgb(stone, 0.88)));
    obs.push(Obstacle { center: bcenter, half: Vec3::new(bw * 0.8, body_h * 0.5, 2.2) });
    // a standable shelf partway up, reachable by mantle
    let ledge_y = c.y + 2.4;
    let lhalf = Vec3::new(2.2, 0.25, 1.3);
    let lcenter = Vec3::new(c.x + 0.6, ledge_y, c.z + 0.4);
    meshes.push(boxx(lcenter, lhalf, dither(scale_rgb(stone, 1.05), 5, 0.12), FACE_ALL));
    obs.push(Obstacle { center: lcenter, half: lhalf });
    // talus steps up to the ledge (each a standable block, rising)
    for k in 0..3 {
        let sy = c.y + 0.5 + k as f32 * 0.65;
        let sx = c.x + 2.4 - k as f32 * 0.5;
        let sh = Vec3::new(0.7, 0.32 + k as f32 * 0.32, 0.7);
        let sc = Vec3::new(sx, c.y + sh.y, c.z + 1.6 - k as f32 * 0.4);
        let _ = sy;
        meshes.push(boxx(sc, sh, dither(scale_rgb(stone, 0.9), k, 0.14), FACE_ALL));
        obs.push(Obstacle { center: sc, half: sh });
    }
    (meshes, obs)
}

/// A frozen pool of drifting **ice floes** and **snowdrifts** — flat floes you hop across as stepping cover and
/// rounded drifts you mound up onto. The floes and drifts are standable; the water between them is open.
fn lm_ice_floes(c: Vec3, _pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut meshes = Vec::new();
    let mut obs = Vec::new();
    let ice = rgb(208, 224, 236);
    let water = rgb(70, 110, 140);
    // the pool surface
    let mut pool = disc(Vec3::new(c.x, c.y + 0.05, c.z), 6.0, 24, water);
    set_alpha(&mut pool, 150);
    pool.reflect = 0.6;
    meshes.push(pool);
    // floes — flat low slabs scattered across the pool (standable stepping stones)
    let mut s = seed | 1;
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s >> 8) as f32 / (1u32 << 24) as f32
    };
    for _ in 0..6 {
        let a = rnd() * std::f32::consts::TAU;
        let rad = 1.2 + rnd() * 3.5;
        let px = c.x + a.cos() * rad;
        let pz = c.z + a.sin() * rad;
        let fr = 0.9 + rnd() * 0.8;
        let half = Vec3::new(fr, 0.16, fr * (0.8 + rnd() * 0.4));
        let center = Vec3::new(px, c.y + 0.16, pz);
        meshes.push(boxx(center, half, dither(ice, (px * 3.0) as i32, 0.08), FACE_ALL));
        obs.push(Obstacle { center, half });
    }
    // snowdrifts — rounded white mounds around the rim (standable)
    for k in 0..3 {
        let a = k as f32 / 3.0 * std::f32::consts::TAU + 0.6;
        let px = c.x + a.cos() * 6.4;
        let pz = c.z + a.sin() * 6.4;
        let r = 1.4 + rnd() * 0.8;
        let mut mound = uv_sphere(Vec3::new(px, c.y - r * 0.45, pz), r, 10, 6, scale_rgb(ice, 1.04));
        scale_xz(&mut mound, 1.5, 1.5);
        meshes.push(mound);
        obs.push(Obstacle { center: Vec3::new(px, c.y + r * 0.28, pz), half: Vec3::new(r * 1.2, r * 0.55, r * 1.2) });
    }
    (meshes, obs)
}

// ── Authored building & dressing kit (desert town) ───────────────────────────────────────────────────────
// A clay/adobe building with a flat standable roof, vaultable windows and an enterable ground floor, plus the
// market and earthquake dressing that fills the streets. Mesh and collision are one object throughout, so what
// you see is what you take cover behind, stand on, and vault through.

/// One axis-aligned wall run of a clay building, laid as a row of panels: solid piers, window bays (a low sill
/// you vault onto with an open gap above and a lintel), and one door bay (open to head height). Pushes aligned
/// meshes + collision. `axis_x` runs the wall along X at fixed Z (else along Z at fixed X).
fn clay_wall_run(
    axis_x: bool,
    fixed: f32,
    a: f32,
    b: f32,
    base_y: f32,
    height: f32,
    th: f32,
    door_panel: i32,
    col: Color32,
    seed: u32,
    out_m: &mut Vec<Mesh3>,
    out_o: &mut Vec<Obstacle>,
) {
    let span = (b - a).abs();
    let n = ((span / 1.8).round() as i32).max(1);
    let pw = span / n as f32;
    let sill = 1.05;
    let lintel = 2.25;
    // Adobe wall colour tones: a warm mud plaster, weathered darker and stained toward the base (rain splash,
    // ground damp) and bleached lighter at the top (sun). Window voids read as deep shadow; a timber lintel/
    // frame of dark wood surrounds each opening — the details that make adobe read as built, not toy-blocky.
    let wood = lerp_c(rgb(96, 66, 40), col, 0.15);
    let void_dark = dither(rgb(28, 24, 22), seed as i32, 0.15);
    let base_stain = lerp_c(col, rgb(70, 52, 40), 0.5); // damp/dirt at the foot
    let sun_bleach = lerp_c(col, rgb(240, 224, 196), 0.35); // sun-bleached crown
    for i in 0..n {
        let c0 = a + pw * i as f32;
        let cc = c0 + pw * 0.5;
        let hw = pw * 0.5 * 0.98;
        // pick panel kind: the door, else windows on a rhythm, else solid pier
        let is_door = i == door_panel;
        let is_window = !is_door && (i % 2 == 1);
        // Build one wall segment box with the adobe material and a bottom-dark / top-light weathering gradient.
        let mk = |lo: f32, hi: f32, c: Color32, key: i32, om: &mut Vec<Mesh3>, oo: &mut Vec<Obstacle>| {
            let (center, half) = if axis_x {
                (Vec3::new(cc, base_y + (lo + hi) * 0.5, fixed), Vec3::new(hw, (hi - lo) * 0.5, th))
            } else {
                (Vec3::new(fixed, base_y + (lo + hi) * 0.5, cc), Vec3::new(th, (hi - lo) * 0.5, hw))
            };
            let mut wm = boxx(center, half, dither(c, key, 0.06), FACE_ALL);
            wm.material = Some(crate::mesh::Material::StoneBlock); // adobe-block relief instead of flat plaster
            // weathering gradient over the whole wall height (uses absolute Y so it's continuous across pieces)
            crate::mesh::tint_height(&mut wm, base_y + 0.0, base_y + height, base_stain, sun_bleach);
            om.push(wm);
            oo.push(Obstacle { center, half });
        };
        // A recessed dark window void + timber lintel/sill frame, inset slightly into the wall so the opening
        // reads as depth rather than a painted-on square.
        let window_frame = |om: &mut Vec<Mesh3>| {
            let inset = th * 0.55;
            let (vc, vh) = if axis_x {
                (Vec3::new(cc, base_y + (sill + lintel) * 0.5, fixed), Vec3::new(hw * 0.72, (lintel - sill) * 0.5, inset))
            } else {
                (Vec3::new(fixed, base_y + (sill + lintel) * 0.5, cc), Vec3::new(inset, (lintel - sill) * 0.5, hw * 0.72))
            };
            om.push(boxx(vc, vh, void_dark, FACE_ALL)); // the dark recessed opening
            // timber lintel above and sill below (protruding a touch)
            let beam = |y: f32, om: &mut Vec<Mesh3>| {
                let (bc, bh) = if axis_x {
                    (Vec3::new(cc, base_y + y, fixed - 0.0), Vec3::new(hw * 0.86, 0.09, th * 1.25))
                } else {
                    (Vec3::new(fixed, base_y + y, cc), Vec3::new(th * 1.25, 0.09, hw * 0.86))
                };
                om.push(boxx(bc, bh, dither(wood, y as i32 * 7, 0.08), FACE_ALL));
            };
            beam(lintel + 0.05, om);
            beam(sill - 0.05, om);
        };
        if is_door {
            mk(lintel, height, col, i * 3, out_m, out_o); // lintel above the doorway only
            // a dark doorway void + a heavier timber lintel over the door
            let inset = th * 0.5;
            let (vc, vh) = if axis_x {
                (Vec3::new(cc, base_y + lintel * 0.5, fixed), Vec3::new(hw * 0.5, lintel * 0.5, inset))
            } else {
                (Vec3::new(fixed, base_y + lintel * 0.5, cc), Vec3::new(inset, lintel * 0.5, hw * 0.5))
            };
            out_m.push(boxx(vc, vh, void_dark, FACE_ALL));
            let (bc, bh) = if axis_x {
                (Vec3::new(cc, base_y + lintel + 0.08, fixed), Vec3::new(hw * 0.66, 0.12, th * 1.3))
            } else {
                (Vec3::new(fixed, base_y + lintel + 0.08, cc), Vec3::new(th * 1.3, 0.12, hw * 0.66))
            };
            out_m.push(boxx(bc, bh, dither(wood, i * 5, 0.08), FACE_ALL));
        } else if is_window {
            mk(0.0, sill, col, i * 3 + 1, out_m, out_o); // sill you vault onto
            mk(lintel, height, col, i * 3 + 2, out_m, out_o); // lintel + upper wall, gap between
            window_frame(out_m);
        } else {
            mk(0.0, height, col, i * 7, out_m, out_o); // solid pier
        }
    }
}

/// A clay/adobe building footprint centred at (cx,cz) with half-extents (hx,hz), `stories` tall. Four vaultable
/// wall runs around an enterable hollow interior, a flat standable roof with a low parapet (rooftop cover), and
/// a doorway on the chosen side. Returns aligned (meshes, obstacles); the interior floor is the level's own.
/// A clay/adobe building plus an optional external staircase up one side (0=N 1=S 2=W 3=E). The stair is a run of
/// solid steps (each rise ≤ 0.55, safely under the 0.65 step-up limit) hugging the outside of that wall,
/// topping out level with the roof slab; the parapet on that side is split, leaving a gap right where the
/// stair lands, so a player can walk street → stairs → rooftop → back down without a single jump. This is
/// what turns rooftops from "technically standable" into a real traversal lane the fight can flow through.
#[allow(clippy::too_many_arguments)]
fn clay_building_ext(cx: f32, cz: f32, hx: f32, hz: f32, stories: i32, door_side: u8, stair_side: Option<u8>, base_y: f32, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut m = Vec::new();
    let mut o = Vec::new();
    let story_h = 3.0;
    let height = stories as f32 * story_h;
    let th = 0.34;
    // Per-building clay hue variation: shift each building's wall toward a slightly different sun-baked mud
    // tone (some pinker, some more ochre, some greyer) so a street of adobe reads as many hand-built houses
    // rather than one repeated prefab — the biggest single fix for the "toy blocks" look.
    let hue_shift = (hash01(seed as i32, 0x7A1) - 0.5) * 2.0;
    let wall_base = if hue_shift > 0.33 {
        lerp_c(pal.wall, rgb(196, 150, 108), 0.35) // ochre
    } else if hue_shift < -0.33 {
        lerp_c(pal.wall, rgb(172, 138, 120), 0.32) // greyed / pinker
    } else {
        lerp_c(pal.wall, rgb(200, 168, 128), 0.22) // warm default
    };
    let wall = dither(wall_base, seed as i32, 0.04);
    let (x0, x1, z0, z1) = (cx - hx, cx + hx, cz - hz, cz + hz);
    // doors: choose which side gets the doorway panel; others all-windows/solid
    let dn = |side: u8| -> i32 {
        if door_side == side {
            1
        } else {
            -1
        }
    };
    clay_wall_run(true, z0, x0, x1, base_y, height, th, dn(0), wall, seed, &mut m, &mut o); // north
    clay_wall_run(true, z1, x0, x1, base_y, height, th, dn(1), wall, seed ^ 0x11, &mut m, &mut o); // south
    clay_wall_run(false, x0, z0, z1, base_y, height, th, dn(2), wall, seed ^ 0x22, &mut m, &mut o); // west
    clay_wall_run(false, x1, z0, z1, base_y, height, th, dn(3), wall, seed ^ 0x33, &mut m, &mut o); // east
    // flat roof slab (standable) + low parapet cover
    let roof_y = base_y + height;
    let rc = Vec3::new(cx, roof_y + 0.16, cz);
    let rh = Vec3::new(hx + th, 0.16, hz + th);
    let mut roof = boxx(rc, rh, scale_rgb(pal.wall, 0.86), FACE_ALL);
    roof.material = Some(crate::mesh::Material::Plaster); // roof reads as a floor surface, not a wall
    m.push(roof);
    o.push(Obstacle { center: rc, half: rh });
    let roof_top = roof_y + 0.32; // walkable roof surface height

    // ── Vigas: the round timber roof-beams of adobe architecture, protruding a foot past the wall just below
    // the roofline. A row of them down the two long faces instantly reads as "adobe house", not "grey box".
    {
        let viga = rgb(88, 60, 38);
        let vy = roof_y - 0.35;
        let nx = (hx / 0.9).round().max(2.0) as i32;
        for i in 0..nx {
            let t = (i as f32 + 0.5) / nx as f32;
            let px = cx - hx + t * hx * 2.0;
            for zface in [z0 - 0.0, z1 + 0.0] {
                let dir = if zface < cz { -1.0 } else { 1.0 };
                let a = Vec3::new(px, vy, zface);
                let b = a + Vec3::new(0.0, 0.0, dir * 0.55);
                m.push(tube(a, b, 0.09, 5, dither(viga, i * 13, 0.1)));
            }
        }
        let nz = (hz / 0.9).round().max(2.0) as i32;
        for i in 0..nz {
            let t = (i as f32 + 0.5) / nz as f32;
            let pz = cz - hz + t * hz * 2.0;
            for xface in [x0 - 0.0, x1 + 0.0] {
                let dir = if xface < cx { -1.0 } else { 1.0 };
                let a = Vec3::new(xface, vy, pz);
                let b = a + Vec3::new(dir * 0.55, 0.0, 0.0);
                m.push(tube(a, b, 0.09, 5, dither(viga, i * 17 + 3, 0.1)));
            }
        }
    }

    // ── external staircase (before the parapet, so the parapet can leave a gap at the stair head) ──────
    // The stair runs along the chosen wall's outside face toward the building's -corner, landing at the
    // +end. The gap centre is where the top step meets the roof.
    let mut stair_gap: Option<(u8, f32)> = None; // (side, gap centre along that wall)
    if let Some(side) = stair_side {
        let target = roof_top - base_y;
        let n = (target / 0.55).ceil().max(1.0) as i32;
        let rise = target / n as f32;
        let depth = 0.68f32;
        let width = 1.25f32;
        // wall geometry for this side: outward normal + run axis
        let along_x = side <= 1; // N/S walls run along X
        let side_len = if along_x { hx * 2.0 } else { hz * 2.0 };
        let run = (n as f32) * depth;
        if run < side_len {
            let (fixed, outward) = match side {
                0 => (z0, -1.0f32), // north face, stairs on -Z side
                1 => (z1, 1.0),
                2 => (x0, -1.0),
                _ => (x1, 1.0),
            };
            let lane = fixed + outward * (th + width * 0.5 + 0.05);
            // top step ends at the wall's +end minus half a step
            let head = if along_x { x1 - depth * 0.5 } else { z1 - depth * 0.5 };
            let stair_col = scale_rgb(pal.trim, 1.02);
            for i in 0..n {
                let top = base_y + rise * (i + 1) as f32;
                let cc = head - depth * (n - 1 - i) as f32; // step i sits (n-1-i) steps back from the head
                let (center, half) = if along_x {
                    (Vec3::new(cc, (base_y + top) * 0.5, lane), Vec3::new(depth * 0.5, (top - base_y) * 0.5, width * 0.5))
                } else {
                    (Vec3::new(lane, (base_y + top) * 0.5, cc), Vec3::new(width * 0.5, (top - base_y) * 0.5, depth * 0.5))
                };
                let mut step = boxx(center, half, dither(stair_col, seed as i32 + i, 0.07), FACE_ALL);
                step.material = Some(crate::mesh::Material::Plaster);
                m.push(step);
                o.push(Obstacle { center, half });
            }
            stair_gap = Some((side, head));
        }
    }

    // parapet, with a gap on the stair side where the stair lands
    let par = scale_rgb(pal.trim, 0.95);
    let parapet_seg = |px: f32, pz: f32, phx: f32, phz: f32, mm: &mut Vec<Mesh3>, oo: &mut Vec<Obstacle>| {
        if phx < 0.05 || phz < 0.05 {
            return; // degenerate sliver after the gap split
        }
        let pc = Vec3::new(px, roof_y + 0.6, pz);
        let ph = Vec3::new(phx, 0.4, phz);
        mm.push(boxx(pc, ph, par, FACE_ALL));
        oo.push(Obstacle { center: pc, half: ph });
    };
    const GAP: f32 = 1.6; // opening width at the stair head
    for (side, px, pz, phx, phz) in [
        (0u8, cx, z0, hx, 0.18f32),
        (1, cx, z1, hx, 0.18),
        (2, x0, cz, 0.18, hz),
        (3, x1, cz, 0.18, hz),
    ] {
        match stair_gap {
            Some((gs, gc)) if gs == side => {
                // split this run into two segments leaving GAP centred on the stair head
                if side <= 1 {
                    let (a0, a1) = (px - phx, px + phx);
                    let (l0, l1) = (a0, (gc - GAP * 0.5).max(a0));
                    let (r0, r1) = ((gc + GAP * 0.5).min(a1), a1);
                    parapet_seg((l0 + l1) * 0.5, pz, (l1 - l0) * 0.5, phz, &mut m, &mut o);
                    parapet_seg((r0 + r1) * 0.5, pz, (r1 - r0) * 0.5, phz, &mut m, &mut o);
                } else {
                    let (a0, a1) = (pz - phz, pz + phz);
                    let (l0, l1) = (a0, (gc - GAP * 0.5).max(a0));
                    let (r0, r1) = ((gc + GAP * 0.5).min(a1), a1);
                    parapet_seg(px, (l0 + l1) * 0.5, phx, (l1 - l0) * 0.5, &mut m, &mut o);
                    parapet_seg(px, (r0 + r1) * 0.5, phx, (r1 - r0) * 0.5, &mut m, &mut o);
                }
            }
            _ => parapet_seg(px, pz, phx, phz, &mut m, &mut o),
        }
    }
    // an interior story-2 floor slab for taller buildings (rooftop drop-through / upper room read)
    if stories >= 2 {
        let fy = base_y + story_h - 0.12;
        let fc = Vec3::new(cx, fy, cz);
        let fh = Vec3::new(hx - 0.4, 0.12, hz - 0.4);
        let mut slab = boxx(fc, fh, scale_rgb(pal.floor, 0.9), FACE_ALL);
        slab.material = Some(crate::mesh::Material::Plaster);
        m.push(slab);
        o.push(Obstacle { center: fc, half: fh });
    }
    // Walls and parapet stay smooth vertex-shaded colour — wrapping every wall in a repeating adobe
    // pattern reads busy up close between tight buildings; texture stays on the floor-like surfaces only.
    //
    // Art-director pass: weather the shell so no two buildings look freshly built. A height-based AO darkens
    // toward the ground (grime, splashback, shadow under the eaves never gets direct sun) and lifts toward
    // the roofline (sun-bleached adobe), then a couple of per-building streaks — seeded off this building's
    // own `seed` so it's stable and deterministic — blend in a dark water-stain running down from the roof,
    // the kind of runoff mark a real unguttered wall picks up.
    for mesh in &mut m {
        shade_ao(mesh, base_y, roof_y, 0.78, 1.1);
    }
    let mut srng = crate::level::Rng::new(seed as u64 ^ 0xA3B1);
    let n_streaks = 1 + (srng.f32() * 2.0) as i32;
    for _ in 0..n_streaks {
        let sx = cx + (srng.f32() - 0.5) * hx * 1.7;
        let sz = cz + (srng.f32() - 0.5) * hz * 1.7;
        let stain = rgb(58, 48, 38);
        for mesh in &mut m {
            streak_stain(mesh, Vec3::new(sx, roof_y, sz), 1.1 + srng.f32() * 0.8, stain);
        }
    }
    (m, o)
}

/// Blend `stain` onto vertices within `radius` (XZ) of a point below `top`, fading out with both distance
/// and height beneath the drip origin — a cheap decal for the kind of dark runoff streak an ungoverned roof
/// leaves down a wall over years of rain.
fn streak_stain(m: &mut Mesh3, top: Vec3, radius: f32, stain: Color32) {
    let cols: Vec<Color32> = m
        .verts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let dx = p.x - top.x;
            let dz = p.z - top.z;
            let horiz = (dx * dx + dz * dz).sqrt();
            let drop = (top.y - p.y).max(0.0);
            let fade = (1.0 - horiz / radius).clamp(0.0, 1.0) * (1.0 - (drop / 6.0).clamp(0.0, 1.0));
            let cur = m.vcol.as_ref().and_then(|v| v.get(i).copied()).unwrap_or(m.color);
            lerp_c(cur, stain, fade * 0.55)
        })
        .collect();
    m.vcol = Some(cols);
}

/// A market stall: four slim posts, a striped fabric awning, and a low table of goods. Posts + table block and
/// are standable cover; the awning is overhead. Earthquake variant tips the table.
fn market_stall(cx: f32, cz: f32, base_y: f32, cloth: Color32, seed: u32, toppled: bool) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut m = Vec::new();
    let mut o = Vec::new();
    let wood = rgb(120, 88, 56);
    let hw = 1.1;
    let post_h = 2.2;
    for (sx, sz) in [(-hw, -hw), (hw, -hw), (-hw, hw), (hw, hw)] {
        let mut post = tube(Vec3::new(cx + sx, base_y, cz + sz), Vec3::new(cx + sx, base_y + post_h, cz + sz), 0.08, 5, wood);
        post.material = Some(crate::mesh::Material::Planks);
        m.push(post);
    }
    // awning
    let mut aw = boxx(Vec3::new(cx, base_y + post_h + 0.05, cz), Vec3::new(hw + 0.2, 0.06, hw + 0.2), cloth, FACE_ALL);
    aw.material = Some(crate::mesh::Material::Cloth);
    m.push(aw);
    // table of goods
    if toppled {
        let mut t = boxx(Vec3::new(cx + 0.5, base_y + 0.5, cz), Vec3::new(hw, 0.1, 0.7), scale_rgb(wood, 0.8), FACE_ALL);
        rotate_y_about(&mut t, Vec3::new(cx, base_y, cz), 0.5);
        // tilt by shoving verts (cheap lean)
        for v in &mut t.verts {
            v.y += (v.x - cx) * 0.4;
        }
        t.material = Some(crate::mesh::Material::Planks);
        m.push(t);
        o.push(Obstacle { center: Vec3::new(cx + 0.4, base_y + 0.4, cz), half: Vec3::new(hw, 0.4, 0.7) });
    } else {
        let tc = Vec3::new(cx, base_y + 0.6, cz);
        let thf = Vec3::new(hw, 0.1, 0.7);
        let mut table = boxx(tc, thf, scale_rgb(wood, 0.9), FACE_ALL);
        table.material = Some(crate::mesh::Material::Planks);
        m.push(table);
        o.push(Obstacle { center: tc, half: Vec3::new(hw, 0.6, 0.7) });
        // a few crates of fruit on top
        for k in 0..3 {
            let ox = (k as f32 - 1.0) * 0.6;
            let fc = [rgb(210, 120, 60), rgb(190, 60, 60), rgb(220, 200, 90)][k % 3];
            m.push(boxx(Vec3::new(cx + ox, base_y + 0.85, cz), Vec3::new(0.22, 0.18, 0.22), fc, FACE_ALL));
        }
    }
    let _ = seed;
    (m, o)
}

/// A length of fabric strung between two points, sagging under gravity — a hanging cloth banner across a lane.
/// Pure dressing (no collision); it just breaks the skyline and reads as a souk.
fn hanging_fabric(a: Vec3, b: Vec3, sag: f32, col: Color32) -> Mesh3 {
    let mut m = Mesh3::new(col);
    let segs = 8;
    let widthv = Vec3::new(0.0, 0.0, 0.0); // banner hangs in the vertical plane between a,b
    let _ = widthv;
    let drop = 0.9;
    for i in 0..segs {
        let t0 = i as f32 / segs as f32;
        let t1 = (i + 1) as f32 / segs as f32;
        let p0 = a + (b - a) * t0;
        let p1 = a + (b - a) * t1;
        let s0 = (t0 * std::f32::consts::PI).sin() * sag;
        let s1 = (t1 * std::f32::consts::PI).sin() * sag;
        let top0 = Vec3::new(p0.x, p0.y - s0, p0.z);
        let top1 = Vec3::new(p1.x, p1.y - s1, p1.z);
        let bot0 = Vec3::new(top0.x, top0.y - drop, top0.z);
        let bot1 = Vec3::new(top1.x, top1.y - drop, top1.z);
        let nrm = Vec3::new(0.0, 0.0, 1.0);
        m.push_quad(top0, top1, bot1, bot0, nrm, dither(col, i, 0.1));
    }
    m.material = Some(crate::mesh::Material::Cloth);
    m
}

/// A parked moped — body, seat, two wheels, handlebars. Low standable/blocking cover, a bit of street life.
/// A parked/wrecked car: a low body, a cabin greenhouse, four wheels, glass, and (if `on_fire`) blackened,
/// buckled panels with the roof caved. Painted enamel (CarPaint material) so it catches the deck's fire
/// light. Returns geometry + a single body-sized collision box. The continuous flame plume itself is emitted
/// by the game from the map's `fire_sources`, not baked here (so it flickers live).
fn burning_car(cx: f32, cz: f32, base_y: f32, yaw: f32, col: Color32, on_fire: bool, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut m = Mesh3::new(col);
    let body_col = if on_fire { rgb(44, 38, 36) } else { col };
    // lower body
    let mut body = boxx(Vec3::new(cx, base_y + 0.5, cz), Vec3::new(2.0, 0.35, 0.9), body_col, FACE_ALL);
    body.material = Some(crate::mesh::Material::CarPaint);
    m.merge(&body);
    // cabin / greenhouse (shorter, set back) — caved if burnt
    let cab_h = if on_fire { 0.24 } else { 0.42 };
    let mut cab = boxx(Vec3::new(cx - 0.15, base_y + 0.5 + 0.35 + cab_h, cz), Vec3::new(1.1, cab_h, 0.78), scale_rgb(body_col, 0.86), FACE_ALL);
    cab.material = Some(crate::mesh::Material::CarPaint);
    m.merge(&cab);
    // windows (dark glass) — only if not burnt out
    if !on_fire {
        let mut glass = boxx(Vec3::new(cx - 0.15, base_y + 0.5 + 0.35 + cab_h, cz), Vec3::new(1.12, cab_h * 0.7, 0.8), rgb(40, 50, 58), FACE_ALL);
        set_alpha(&mut glass, 170);
        m.merge(&glass);
    }
    // wheels — tubes along Z for real thickness
    let tyre = rgb(30, 30, 34);
    let ww = 0.16;
    for &(ox, oz) in &[(-1.4f32, -0.85f32), (1.4, -0.85), (-1.4, 0.85), (1.4, 0.85)] {
        m.merge(&tube(
            Vec3::new(cx + ox, base_y + 0.3, cz + oz - ww),
            Vec3::new(cx + ox, base_y + 0.3, cz + oz + ww),
            0.32,
            8,
            tyre,
        ));
    }
    // burnt cars get a scorch/soot gradient and a few debris flecks
    if on_fire {
        crate::fsm::detail::weather(&mut m, base_y, base_y + 1.4);
        for k in 0..5 {
            let dx = (hash01(k, seed as i32) - 0.5) * 3.2;
            let dz = (hash01(k, seed as i32 ^ 0x7) - 0.5) * 1.6;
            m.merge(&boxx(Vec3::new(cx + dx, base_y + 0.05, cz + dz), Vec3::new(0.08, 0.05, 0.08), rgb(24, 22, 22), FACE_ALL));
        }
    }
    rotate_y_about(&mut m, Vec3::new(cx, base_y, cz), yaw);
    // Collision box reaches up to ~1.5 (roof height) so a car is real eye-height cover you fight around,
    // not a knee-high bump you shoot over — this is what makes the deck play as broken-up lanes.
    let o = vec![Obstacle { center: Vec3::new(cx, base_y + 0.75, cz), half: Vec3::new(2.0, 0.75, 0.95) }];
    (vec![m], o)
}

fn moped(cx: f32, cz: f32, base_y: f32, yaw: f32, col: Color32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut m = Mesh3::new(col);
    m.merge(&boxx(Vec3::new(cx, base_y + 0.55, cz), Vec3::new(0.8, 0.18, 0.28), col, FACE_ALL)); // body
    m.merge(&boxx(Vec3::new(cx - 0.2, base_y + 0.78, cz), Vec3::new(0.28, 0.12, 0.24), scale_rgb(col, 0.7), FACE_ALL)); // seat
    let tyre = rgb(40, 40, 44);
    // Wheels: short tubes along Z (the axle axis) so each is a real disc with thickness, not the zero-length
    // cylinder a stray `cz - 0.0` used to produce (both endpoints landed on the same point — half its
    // triangles were degenerate).
    let wheel_w = 0.14;
    m.merge(&tube(Vec3::new(cx - 0.7, base_y + 0.3, cz - wheel_w), Vec3::new(cx - 0.7, base_y + 0.3, cz + wheel_w), 0.3, 8, tyre));
    m.merge(&tube(Vec3::new(cx + 0.7, base_y + 0.3, cz - wheel_w), Vec3::new(cx + 0.7, base_y + 0.3, cz + wheel_w), 0.3, 8, tyre));
    m.merge(&tube(Vec3::new(cx + 0.7, base_y + 0.55, cz), Vec3::new(cx + 0.95, base_y + 0.95, cz), 0.05, 5, rgb(60, 60, 64))); // fork/bars
    rotate_y_about(&mut m, Vec3::new(cx, base_y, cz), yaw);
    let o = vec![Obstacle { center: Vec3::new(cx, base_y + 0.45, cz), half: Vec3::new(0.9, 0.45, 0.5) }];
    (vec![m], o)
}

// ── Street & rooftop detail kit ───────────────────────────────────────────────────────────────────────────
// The density pass: the small physical clutter that makes a block read as a lived-in place instead of a
// geometry test — AC units and antennas on roofs, cables and string lights sagging between buildings,
// sandbag mounts, fuel drums and pallets in the alleys, manholes and lane paint on the streets. Everything
// solid gets an aligned obstacle; pure dressing (cables, decals) deliberately doesn't.

/// Rooftop clutter inside the parapet: an AC unit (standable box + fan detail), a radio antenna, and a low
/// pipe run. Deterministic in `seed`; scaled to the roof so small roofs get less.
fn roof_clutter(cx: f32, cz: f32, hx: f32, hz: f32, roof_top: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut m = Vec::new();
    let mut o = Vec::new();
    let mut rng = Rng::new(seed as u64 ^ 0xB00F);
    // AC unit — offset toward one corner, chest-high standable cover on the roof
    let ax = cx + (rng.f32() - 0.5) * hx;
    let az = cz + (rng.f32() - 0.5) * hz;
    let ac = Vec3::new(ax, roof_top + 0.45, az);
    let ah = Vec3::new(0.65, 0.45, 0.5);
    let mut unit = boxx(ac, ah, rgb(148, 152, 156), FACE_ALL);
    unit.material = Some(crate::mesh::Material::Corrugated);
    m.push(unit);
    m.push(disc(Vec3::new(ax, roof_top + 0.91, az), 0.3, 10, rgb(70, 74, 78))); // fan intake
    o.push(Obstacle { center: ac, half: ah });
    // antenna mast with a cross-arm
    let mx = cx - (rng.f32() - 0.5) * hx;
    let mz = cz - (rng.f32() - 0.5) * hz;
    m.push(tube(Vec3::new(mx, roof_top, mz), Vec3::new(mx, roof_top + 2.6, mz), 0.045, 5, rgb(90, 90, 96)));
    m.push(tube(Vec3::new(mx - 0.4, roof_top + 2.1, mz), Vec3::new(mx + 0.4, roof_top + 2.1, mz), 0.03, 4, rgb(90, 90, 96)));
    // a short pipe run hugging the roof
    if rng.f32() < 0.7 {
        let py = roof_top + 0.12;
        m.push(tube(Vec3::new(cx - hx * 0.6, py, cz + hz * 0.5), Vec3::new(cx + hx * 0.4, py, cz + hz * 0.5), 0.09, 6, rgb(120, 96, 70)));
    }
    (m, o)
}

/// A sagging utility cable strung between two points — pure dressing, no collision. The sag is a simple
/// parabola; a couple of these criss-crossing a street instantly sells "old town wiring".
fn overhead_cable(a: Vec3, b: Vec3, sag: f32) -> Mesh3 {
    let mut m = Mesh3::new(rgb(38, 36, 34));
    let segs = 7;
    for i in 0..segs {
        let t0 = i as f32 / segs as f32;
        let t1 = (i + 1) as f32 / segs as f32;
        let sag_at = |t: f32| 4.0 * sag * t * (1.0 - t); // parabolic dip, max `sag` at midspan
        let p0 = a + (b - a) * t0 - Vec3::new(0.0, sag_at(t0), 0.0);
        let p1 = a + (b - a) * t1 - Vec3::new(0.0, sag_at(t1), 0.0);
        m.merge(&tube(p0, p1, 0.025, 4, rgb(38, 36, 34)));
    }
    m
}

/// Festival string lights: a sagging line with small glowing bulbs — the emissive detail that keeps a
/// courtyard warm at dusk. Dressing only, no collision.
fn string_lights(a: Vec3, b: Vec3, sag: f32, bulbs: usize) -> Vec<Mesh3> {
    let mut out = vec![overhead_cable(a, b, sag)];
    let mut glow = Mesh3::new(rgb(255, 214, 140));
    for i in 0..bulbs {
        let t = (i as f32 + 0.5) / bulbs as f32;
        let sag_at = 4.0 * sag * t * (1.0 - t);
        let p = a + (b - a) * t - Vec3::new(0.0, sag_at + 0.09, 0.0);
        glow.merge(&uv_sphere(p, 0.06, 5, 4, rgb(255, 214, 140)));
    }
    glow.emissive = 0.85;
    out.push(glow);
    out
}

/// A chest-high sandbag line — the classic mounted-cover piece: crouch behind it, stand to shoot over.
/// Bags are staggered rows of rounded-off boxes; one aligned obstacle covers the whole run.
fn sandbag_line(cx: f32, cz: f32, along_x: bool, len: f32, base_y: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let bag = rgb(150, 134, 100);
    let mut m = Mesh3::new(bag);
    let rows = 3;
    let per = (len / 0.62).max(1.0) as i32;
    for r in 0..rows {
        let y = base_y + 0.16 + r as f32 * 0.3;
        let off = if r % 2 == 1 { 0.31 } else { 0.0 };
        for i in 0..per - (r % 2) {
            let t = -len * 0.5 + 0.31 + i as f32 * 0.62 + off;
            let (px, pz) = if along_x { (cx + t, cz) } else { (cx, cz + t) };
            let (bx, bz) = if along_x { (0.3, 0.22) } else { (0.22, 0.3) };
            m.merge(&boxx(Vec3::new(px, y, pz), Vec3::new(bx, 0.15, bz), dither(bag, seed as i32 + r * 31 + i, 0.12), FACE_ALL));
        }
    }
    let half = if along_x { Vec3::new(len * 0.5, 0.5, 0.28) } else { Vec3::new(0.28, 0.5, len * 0.5) };
    let center = Vec3::new(cx, base_y + 0.5, cz);
    (vec![m], vec![Obstacle { center, half }])
}

/// A 55-gal drum as inert street furniture (the explosive ones are destructibles, spawned separately).
fn oil_drum(cx: f32, cz: f32, base_y: f32, col: Color32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let mut m = tube(Vec3::new(cx, base_y, cz), Vec3::new(cx, base_y + 0.95, cz), 0.34, 9, dither(col, seed as i32, 0.1));
    m.merge(&disc(Vec3::new(cx, base_y + 0.96, cz), 0.34, 9, scale_rgb(col, 0.72)));
    let o = Obstacle { center: Vec3::new(cx, base_y + 0.475, cz), half: Vec3::new(0.36, 0.475, 0.36) };
    (vec![m], vec![o])
}

/// A wooden shipping pallet — knee-high slat detail flat on the ground in alleys and loading spots.
fn pallet(cx: f32, cz: f32, base_y: f32, yaw: f32, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let wood = rgb(134, 104, 66);
    let mut m = Mesh3::new(wood);
    for i in 0..5 {
        let t = -0.5 + i as f32 * 0.25;
        m.merge(&boxx(Vec3::new(cx + t, base_y + 0.12, cz), Vec3::new(0.09, 0.03, 0.6), dither(wood, seed as i32 + i, 0.14), FACE_ALL));
    }
    m.merge(&boxx(Vec3::new(cx, base_y + 0.06, cz - 0.5), Vec3::new(0.6, 0.05, 0.06), scale_rgb(wood, 0.85), FACE_ALL));
    m.merge(&boxx(Vec3::new(cx, base_y + 0.06, cz + 0.5), Vec3::new(0.6, 0.05, 0.06), scale_rgb(wood, 0.85), FACE_ALL));
    m.material = Some(crate::mesh::Material::Planks);
    rotate_y_about(&mut m, Vec3::new(cx, base_y, cz), yaw);
    let o = Obstacle { center: Vec3::new(cx, base_y + 0.09, cz), half: Vec3::new(0.62, 0.09, 0.62) };
    (vec![m], vec![o])
}

/// Street surface details: a manhole cover and dashed lane paint down a straight. Decals only — flat discs
/// and panels a hair off the ground, no collision (nothing here is tall enough to matter).
fn road_details(cx: f32, cz: f32, along_x: bool, len: f32, base_y: f32, seed: u32) -> Vec<Mesh3> {
    let mut out = Vec::new();
    let mut rng = Rng::new(seed as u64 ^ 0x0AD5);
    // manhole
    let mt = (rng.f32() - 0.5) * len * 0.6;
    let (mx, mz) = if along_x { (cx + mt, cz + 0.6) } else { (cx + 0.6, cz + mt) };
    let mut man = disc(Vec3::new(mx, base_y + 0.025, mz), 0.42, 10, rgb(74, 70, 64));
    man.texturize(0.4, 0.15, seed);
    out.push(man);
    // dashed centre line
    let dashes = (len / 2.4) as i32;
    let paint = rgb(196, 186, 158);
    let mut line = Mesh3::new(paint);
    for i in 0..dashes {
        let t = -len * 0.5 + 1.2 + i as f32 * 2.4;
        let (px, pz) = if along_x { (cx + t, cz) } else { (cx, cz + t) };
        let (hx2, hz2) = if along_x { (0.55, 0.07) } else { (0.07, 0.55) };
        line.merge(&panel(Vec3::new(px, base_y + 0.02, pz), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0), hx2, hz2, dither(paint, i, 0.1)));
    }
    out.push(line);
    out
}

// ── Story vignettes ──────────────────────────────────────────────────────────────────────────────────────
// Small hand-authored tableaus the generator strews through the map. Each is arranged to imply an event —
// a firefight that ended here, someone who camped and moved on, a battlefield burial, a drop that landed
// wrong — so the world reads as built and lived-in rather than procedurally tiled. Cheap (≈15-30 tris) and
// returned like landmarks: (meshes, blocking obstacles).

/// A defender's last stand: a low sandbag horseshoe, a dropped helmet, a fallen rifle, and a litter of
/// spent brass — the spot where someone dug in and didn't leave.
fn vig_last_stand(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let bag_c = lerp_c(pal.cover, rgb(122, 112, 84), 0.45);
    let mut bags = Mesh3::new(bag_c);
    bags.merge(&crate::fsm::detail::sandbags(Vec3::new(c.x - 1.4, c.y, c.z + 1.1), Vec3::new(c.x + 1.4, c.y, c.z + 1.1), bag_c, seed));
    bags.merge(&crate::fsm::detail::sandbags(Vec3::new(c.x - 1.4, c.y, c.z + 1.1), Vec3::new(c.x - 1.4, c.y, c.z - 0.3), bag_c, seed ^ 0x7));
    let mut props = Mesh3::new(rgb(70, 76, 68));
    props.merge(&uv_sphere(Vec3::new(c.x + 0.5, c.y + 0.12, c.z + 0.2), 0.16, 10, 6, rgb(70, 76, 68))); // dropped helmet
    props.merge(&boxx(Vec3::new(c.x - 0.3, c.y + 0.05, c.z + 0.5), Vec3::new(0.45, 0.03, 0.06), rgb(40, 38, 36), FACE_ALL)); // fallen rifle
    let brass = crate::fsm::detail::debris(Vec3::new(c.x, c.y, c.z + 0.7), 1.3, 11, rgb(184, 150, 72), seed ^ 0x2C); // spent casings
    let obs = vec![
        Obstacle { center: Vec3::new(c.x, c.y + 0.4, c.z + 1.1), half: Vec3::new(1.4, 0.4, 0.28) },
        Obstacle { center: Vec3::new(c.x - 1.4, c.y + 0.4, c.z + 0.4), half: Vec3::new(0.28, 0.4, 0.8) },
    ];
    (vec![bags, props, brass], obs)
}

/// A cold camp: a dead fire ring of stones over grey ash with charred crossed logs, a bedroll, a tipped pot
/// and a couple of empty cans — someone sheltered here, then left or didn't make it.
fn vig_cold_camp(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let stone = scale_rgb(pal.rock, 0.9);
    let mut m = Mesh3::new(stone);
    for k in 0..7 {
        let a = k as f32 / 7.0 * std::f32::consts::TAU;
        m.merge(&rock(Vec3::new(c.x + a.cos() * 0.62, c.y + 0.08, c.z + a.sin() * 0.62), 0.14, seed + k, stone));
    }
    let mut ash = disc(Vec3::new(c.x, c.y + 0.03, c.z), 0.5, 12, rgb(64, 60, 56));
    ash.merge(&tube(Vec3::new(c.x - 0.4, c.y + 0.08, c.z - 0.3), Vec3::new(c.x + 0.4, c.y + 0.08, c.z + 0.3), 0.06, 5, rgb(40, 34, 30)));
    ash.merge(&tube(Vec3::new(c.x - 0.4, c.y + 0.08, c.z + 0.3), Vec3::new(c.x + 0.4, c.y + 0.08, c.z - 0.3), 0.06, 5, rgb(48, 40, 34)));
    let mut props = Mesh3::new(lerp_c(pal.accent, rgb(118, 92, 72), 0.5));
    props.merge(&boxx(Vec3::new(c.x + 1.4, c.y + 0.07, c.z), Vec3::new(0.34, 0.07, 0.7), lerp_c(pal.accent, rgb(118, 92, 72), 0.5), FACE_ALL)); // bedroll
    props.merge(&tube(Vec3::new(c.x - 1.2, c.y + 0.12, c.z + 0.2), Vec3::new(c.x - 0.78, c.y + 0.12, c.z + 0.2), 0.13, 8, rgb(40, 42, 46))); // tipped pot
    let cans = crate::fsm::detail::debris(Vec3::new(c.x + 0.8, c.y, c.z - 0.9), 0.8, 4, rgb(150, 140, 120), seed ^ 0x11);
    (vec![m, ash, props, cans], Vec::new())
}

/// A field grave: a small stone cairn with a rifle planted muzzle-down and a helmet resting on the stock —
/// a soldier buried where they fell.
fn vig_grave(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let stone = scale_rgb(pal.rock, 0.85);
    let mut m = Mesh3::new(stone);
    m.merge(&rock(Vec3::new(c.x, c.y + 0.2, c.z), 0.42, seed, stone));
    m.merge(&rock(Vec3::new(c.x, c.y + 0.52, c.z), 0.28, seed ^ 3, scale_rgb(stone, 1.1)));
    m.merge(&rock(Vec3::new(c.x + 0.5, c.y + 0.12, c.z + 0.3), 0.2, seed ^ 5, stone));
    let mut props = Mesh3::new(rgb(38, 36, 34));
    props.merge(&boxx(Vec3::new(c.x + 0.55, c.y + 0.46, c.z), Vec3::new(0.04, 0.46, 0.055), rgb(38, 36, 34), FACE_ALL)); // planted rifle
    props.merge(&uv_sphere(Vec3::new(c.x + 0.55, c.y + 0.94, c.z), 0.15, 10, 6, rgb(66, 72, 64))); // helmet on stock
    let obs = vec![Obstacle { center: Vec3::new(c.x, c.y + 0.35, c.z), half: Vec3::new(0.5, 0.35, 0.5) }];
    (vec![m, props], obs)
}

/// A supply drop gone wrong: a split-open crate spilling its contents, a draped parachute canopy, and a
/// spent signal flare still glowing faintly — help that arrived too late or in the wrong place.
fn vig_supply_drop(c: Vec3, pal: Palette, seed: u32) -> (Vec<Mesh3>, Vec<Obstacle>) {
    let wood = lerp_c(pal.wall, rgb(120, 96, 60), 0.5);
    let mut m = Mesh3::new(wood);
    m.merge(&boxx(Vec3::new(c.x - 0.4, c.y + 0.3, c.z), Vec3::new(0.42, 0.3, 0.5), wood, FACE_ALL)); // crate body
    m.merge(&boxx(Vec3::new(c.x + 0.6, c.y + 0.12, c.z + 0.35), Vec3::new(0.32, 0.04, 0.42), scale_rgb(wood, 0.9), FACE_ALL)); // sprung lid
    let cont = crate::fsm::detail::debris(Vec3::new(c.x + 0.2, c.y, c.z + 0.6), 1.0, 6, scale_rgb(wood, 1.1), seed);
    let mut chute = uv_sphere(Vec3::new(c.x - 1.7, c.y + 0.42, c.z - 0.8), 1.0, 12, 6, rgb(212, 212, 200));
    for v in chute.verts.iter_mut() {
        v.y = c.y + 0.42 + (v.y - (c.y + 0.42)) * 0.32; // squash into a draped sheet
    }
    set_alpha(&mut chute, 150);
    let mut glow = uv_sphere(Vec3::new(c.x + 1.0, c.y + 0.1, c.z - 0.5), 0.1, 6, 4, rgb(255, 120, 80));
    glow.emissive = 0.85; // spent flare
    let obs = vec![Obstacle { center: Vec3::new(c.x - 0.4, c.y + 0.3, c.z), half: Vec3::new(0.5, 0.3, 0.55) }];
    (vec![m, chute, cont, glow], obs)
}



/// Smooth value noise at a lattice point in [-0.5, 0.5].
fn lattice(xi: i32, zi: i32, seed: i32) -> f32 {
    crate::mesh::hash01(xi.wrapping_mul(374_761_393) ^ zi.wrapping_mul(668_265_263), seed) - 0.5
}
/// What the biome's low growth looks like.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UnderKind {
    Bush,
    Grass,
    Fern,
}

/// Per-biome tuning for the wilds scatter, so each open biome reads as a *different kind of place*: Meadow
/// is mostly open grass + flowers with rare trees; Redwood is sparse but colossal trunks; Toxic is choked.
#[derive(Clone, Copy)]
struct WildsProfile {
    tree_density: f32,  // multiplier on the mid-tree count
    mid_len: (f32, f32),// trunk length (min, random span)
    mid_rad: f32,       // trunk-radius multiplier
    hero_max: i32,      // cap on big "hero" trees
    hero_len: (f32, f32),
    hero_rad: f32,
    understory: f32,    // multiplier on bush/fern count
    under_kind: UnderKind,
    litter: f32,        // multiplier on ground-litter count
    boulders: f32,      // multiplier on boulder count
    flowers: bool,      // sprinkle bright flower dabs into the litter
    cap: f32,           // max tree height for this biome (lets Redwood tower)
}

fn wilds_profile(theme: MapTheme) -> WildsProfile {
    match theme {
        // Open sunlit field: very few small trees, lush flowering grass underfoot.
        MapTheme::Meadow => WildsProfile {
            tree_density: 0.22, mid_len: (2.0, 2.6), mid_rad: 0.9, hero_max: 4, hero_len: (3.5, 3.0), hero_rad: 1.0,
            understory: 0.7, under_kind: UnderKind::Grass, litter: 1.8, boulders: 0.5, flowers: true, cap: 6.0,
        },
        // Old-growth cathedral: sparse stands of immense, thick trunks; ferny floor; dim.
        MapTheme::Redwood => WildsProfile {
            tree_density: 0.55, mid_len: (4.5, 4.5), mid_rad: 1.7, hero_max: 16, hero_len: (9.0, 4.0), hero_rad: 2.3,
            understory: 0.8, under_kind: UnderKind::Fern, litter: 0.9, boulders: 0.5, flowers: false, cap: 13.0,
        },
        // Lush balanced grove (the baseline look).
        MapTheme::Canopy => WildsProfile {
            tree_density: 1.0, mid_len: (2.4, 4.4), mid_rad: 1.0, hero_max: 12, hero_len: (5.0, 4.0), hero_rad: 1.0,
            understory: 1.0, under_kind: UnderKind::Bush, litter: 1.0, boulders: 1.0, flowers: true, cap: 7.5,
        },
        // Drowned terraces: medium trees, lots of low understory, fewer big specimens.
        MapTheme::Mistfall => WildsProfile {
            tree_density: 0.8, mid_len: (2.2, 3.8), mid_rad: 1.0, hero_max: 8, hero_len: (4.5, 3.5), hero_rad: 1.0,
            understory: 1.2, under_kind: UnderKind::Bush, litter: 1.0, boulders: 0.8, flowers: false, cap: 7.0,
        },
        // Choked swamp: dense sickly growth, heavy understory.
        MapTheme::Toxic => WildsProfile {
            tree_density: 1.15, mid_len: (2.2, 4.0), mid_rad: 1.0, hero_max: 10, hero_len: (4.5, 3.5), hero_rad: 1.0,
            understory: 1.3, under_kind: UnderKind::Fern, litter: 1.1, boulders: 0.9, flowers: false, cap: 7.0,
        },
        // Alien jungle: glowing medium grove.
        MapTheme::Alien => WildsProfile {
            tree_density: 0.9, mid_len: (2.4, 4.2), mid_rad: 1.0, hero_max: 10, hero_len: (5.0, 4.0), hero_rad: 1.0,
            understory: 1.0, under_kind: UnderKind::Bush, litter: 1.0, boulders: 1.0, flowers: true, cap: 7.5,
        },
        // Sparse hardy growth for the non-open biomes that still route through the grove.
        _ => WildsProfile {
            tree_density: 0.6, mid_len: (1.6, 2.4), mid_rad: 1.0, hero_max: 6, hero_len: (3.0, 2.0), hero_rad: 1.0,
            understory: 0.7, under_kind: UnderKind::Bush, litter: 0.8, boulders: 1.0, flowers: false, cap: 4.0,
        },
    }
}

/// A small tuft of slim grass blades standing up from `base` (each blade a thin tapered triangle, fanned and
/// leaned outward). Cheap — a handful of triangles — but reads convincingly as field grass.
fn grass_tuft(base: Vec3, h: f32, seed: u32, col: Color32) -> Mesh3 {
    let mut m = Mesh3::new(col);
    let blades = 4 + (seed % 4) as i32;
    let s = seed as i32;
    for i in 0..blades {
        let a = (i as f32 / blades as f32) * std::f32::consts::TAU + crate::mesh::hash01(s, i) * 1.4;
        let (ca, sa) = (a.cos(), a.sin());
        let dir = Vec3::new(ca, 0.0, sa);
        let perp = Vec3::new(-sa, 0.0, ca);
        let w = 0.022 + crate::mesh::hash01(s, i + 99) * 0.02;
        let bh = h * (0.6 + crate::mesh::hash01(s, i + 7) * 0.7);
        let lean = bh * (0.18 + crate::mesh::hash01(s, i + 21) * 0.25);
        let bl = base - perp * w;
        let br = base + perp * w;
        let tip = base + Vec3::new(dir.x * lean, bh, dir.z * lean);
        // normal leans toward the blade face so it catches side light
        let n = (Vec3::new(0.0, 1.0, 0.0) + dir * 0.4).normalized();
        m.push_tri_sway(bl, br, tip, n, col, 0.0, 0.0, 1.0); // base rooted, tip bows in the wind
    }
    m
}

/// Distance from point `p` to segment `a`–`b` in the XZ plane (Vec2 stores x,z).
fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let (abx, aby) = (b.x - a.x, b.y - a.y);
    let (apx, apy) = (p.x - a.x, p.y - a.y);
    let len2 = abx * abx + aby * aby;
    let t = if len2 > 1e-6 { ((apx * abx + apy * aby) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (cx, cy) = (a.x + abx * t, a.y + aby * t);
    ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt()
}

/// Bilinear, smoothstep-interpolated value noise.
fn vnoise(gx: f32, gz: f32, seed: i32) -> f32 {
    let xi = gx.floor() as i32;
    let zi = gz.floor() as i32;
    let fx = gx - xi as f32;
    let fz = gz - zi as f32;
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sz = fz * fz * (3.0 - 2.0 * fz);
    let a = lattice(xi, zi, seed);
    let b = lattice(xi + 1, zi, seed);
    let c = lattice(xi, zi + 1, seed);
    let d = lattice(xi + 1, zi + 1, seed);
    let top = a * (1.0 - sx) + b * sx;
    let bot = c * (1.0 - sx) + d * sx;
    top * (1.0 - sz) + bot * sz
}

/// Multiply a colour's RGB by `f` (clamped) — quick way to darken/brighten a palette entry.
fn scale_rgb(c: Color32, f: f32) -> Color32 {
    let g = |v: u8| (v as f32 * f).clamp(0.0, 255.0) as u8;
    Color32::from_rgb(g(c.r()), g(c.g()), g(c.b()))
}

/// Build the visible mesh for one cover obstacle, choosing a biome-appropriate prop. The collision shape
/// stays the same AABB; only the look changes — crates indoors, faceted rock outdoors, glowing crystal
/// clusters on the alien world, lava-veined stone on the volcanic one.
fn build_prop(theme: MapTheme, pal: &Palette, o: &Obstacle, props: &mut Mesh3, glow: &mut Mesh3, stone: &mut Mesh3) {
    let key = (o.center.x * 13.0) as i32 + (o.center.z * 29.0) as i32;
    let seed = (key as u32).wrapping_mul(2_654_435_761);
    let floor_pt = o.center.with_y(o.center.y - o.half.y);
    let rc = o.center.with_y(o.center.y - o.half.y * 0.1);

    // Nature/organic biomes lean on boulders + foliage; built environments draw from an industrial kit.
    let organic = matches!(theme, MapTheme::Alien | MapTheme::Volcanic | MapTheme::Ice | MapTheme::Canopy | MapTheme::Mistfall | MapTheme::Toxic | MapTheme::Meadow | MapTheme::Redwood);

    if organic {
        match seed % 7 {
            0 | 1 => {
                let mut rk = rock(rc, o.half.x * 1.2, seed, pal.rock);
                let top_tint = match theme {
                    MapTheme::Ice => rgb(255, 255, 255),
                    MapTheme::Volcanic => rgb(70, 40, 30),
                    MapTheme::Mistfall => rgb(120, 170, 170),
                    MapTheme::Toxic => rgb(120, 150, 60),
                    _ => rgb(120, 150, 80),
                };
                tint_up(&mut rk, lerp_c(pal.rock, top_tint, 0.6), scale_rgb(pal.rock, 0.58));
                stone.merge(&rk); // homogeneous stone → its own textured mesh
                if theme == MapTheme::Volcanic {
                    glow.merge(&crystal(floor_pt, o.half.y * 1.3, o.half.x * 0.3, 5, seed ^ 7, rgb(255, 140, 50)));
                }
            }
            2 => prop_foliage(theme, pal, o, seed, props),
            3 => prop_cairn(pal, o, seed, stone),
            4 => prop_log(pal, o, seed, props),
            5 => prop_shrooms(theme, pal, o, seed, props, glow),
            _ => {
                // crystal cluster / ice spire / coral
                let col = match theme {
                    MapTheme::Ice => rgb(190, 224, 255),
                    MapTheme::Volcanic => rgb(255, 150, 60),
                    MapTheme::Mistfall => rgb(150, 210, 220),
                    MapTheme::Canopy => rgb(230, 200, 110),
                    MapTheme::Toxic => rgb(170, 230, 90),
                    _ => rgb(150, 220, 140),
                };
                let m = crystal(floor_pt, o.half.y * 1.6, o.half.x * 0.5, 5, seed, col);
                if matches!(theme, MapTheme::Volcanic | MapTheme::Toxic) {
                    glow.merge(&m);
                } else {
                    props.merge(&m);
                }
            }
        }
        return;
    }

    // Built environments (Industrial / Bunker / Warehouse / Urban / Military): a 10-way industrial prop kit.
    match seed % 10 {
        0 => prop_crate(pal, o, key, props),
        1 => prop_crate_stack(pal, o, seed, props),
        2 => prop_barrel(o, seed, props),
        3 => prop_pipes(pal, o, seed, props),
        4 => prop_pillar(pal, o, props),
        5 => prop_console(theme, pal, o, seed, props, glow),
        6 => prop_rubble(pal, o, seed, props),
        7 => prop_lockers(pal, o, seed, props),
        8 => prop_sandbags(pal, o, seed, props),
        _ => prop_generator(pal, o, seed, props, glow),
    }
}

/// A row of tall steel lockers — interior clutter that reads as a locker room / armory.
fn prop_lockers(pal: &Palette, o: &Obstacle, seed: u32, props: &mut Mesh3) {
    let body = scale_rgb(pal.cover, 0.85);
    let n = 3;
    let w = o.half.x * 2.0 / n as f32;
    for i in 0..n {
        let cx = o.center.x - o.half.x + w * (i as f32 + 0.5);
        let tint = dither(body, (seed as i32).wrapping_add(i * 7), 0.08);
        let mut lk = boxx(Vec3::new(cx, o.center.y, o.center.z), Vec3::new(w * 0.46, o.half.y, o.half.z * 0.8), tint, FACE_NO_BOTTOM);
        tint_up(&mut lk, lerp_c(tint, rgb(255, 255, 255), 0.1), scale_rgb(tint, 0.6));
        props.merge(&lk);
        // handle/seam strip
        props.merge(&boxx(Vec3::new(cx + w * 0.3, o.center.y, o.center.z - o.half.z * 0.8), Vec3::new(0.02, o.half.y * 0.5, 0.03), scale_rgb(pal.trim, 0.8), FACE_ALL));
    }
}

/// A low stacked sandbag wall — chest-high soft cover.
fn prop_sandbags(pal: &Palette, o: &Obstacle, seed: u32, props: &mut Mesh3) {
    let sand = lerp_c(pal.cover, rgb(150, 134, 96), 0.6);
    let rows = 3;
    let bag_h = o.half.y * 0.66 / rows as f32;
    let per = 4;
    for r in 0..rows {
        let y = o.center.y - o.half.y + bag_h * (2 * r + 1) as f32;
        let stagger = if r % 2 == 0 { 0.0 } else { o.half.x / per as f32 };
        for b in 0..per {
            let x = o.center.x - o.half.x + (o.half.x * 2.0 / per as f32) * (b as f32 + 0.5) + stagger;
            if x > o.center.x + o.half.x {
                continue;
            }
            let t = dither(sand, (seed as i32).wrapping_add(r * 13 + b), 0.07);
            // a rounded-ish bag: a flattened box
            props.merge(&boxx(Vec3::new(x, y, o.center.z), Vec3::new(o.half.x / per as f32 * 0.92, bag_h * 0.95, o.half.z * 0.7), t, FACE_NO_BOTTOM));
        }
    }
}

/// An industrial generator/machine: a ribbed body, an exhaust stack, and a small lit status panel.
fn prop_generator(pal: &Palette, o: &Obstacle, seed: u32, props: &mut Mesh3, glow: &mut Mesh3) {
    let body = scale_rgb(pal.cover, 0.7);
    let mut g = boxx(o.center, Vec3::new(o.half.x * 0.9, o.half.y, o.half.z * 0.9), body, FACE_NO_BOTTOM);
    tint_up(&mut g, lerp_c(body, rgb(255, 255, 255), 0.08), scale_rgb(body, 0.55));
    props.merge(&g);
    // cooling ribs
    let rib = scale_rgb(pal.trim, 0.7);
    for i in 0..4 {
        let z = o.center.z - o.half.z * 0.6 + o.half.z * 0.4 * i as f32;
        props.merge(&boxx(Vec3::new(o.center.x, o.center.y + o.half.y * 0.4, z), Vec3::new(o.half.x * 0.95, 0.04, 0.03), rib, FACE_ALL));
    }
    // exhaust stack
    let top = o.center.y + o.half.y;
    props.merge(&tube(Vec3::new(o.center.x + o.half.x * 0.5, top, o.center.z), Vec3::new(o.center.x + o.half.x * 0.5, top + o.half.y * 0.7, o.center.z), 0.08, 6, scale_rgb(pal.trim, 0.6)));
    // status panel (emissive)
    let on = seed % 3 != 0;
    let pc = if on { rgb(120, 235, 140) } else { rgb(235, 120, 90) };
    glow.merge(&boxx(Vec3::new(o.center.x, o.center.y + o.half.y * 0.2, o.center.z - o.half.z * 0.9), Vec3::new(o.half.x * 0.3, o.half.y * 0.18, 0.02), pc, FACE_ALL));
}

/// A fallen mossy log with a couple of leaf sprays — organic ground clutter.
fn prop_log(pal: &Palette, o: &Obstacle, seed: u32, props: &mut Mesh3) {
    let wood = lerp_c(pal.rock, rgb(96, 70, 44), 0.5);
    let y = o.center.y - o.half.y * 0.4;
    let ang = (seed % 8) as f32 * 0.4;
    let dx = ang.cos() * o.half.x;
    let dz = ang.sin() * o.half.z;
    let a = Vec3::new(o.center.x - dx, y, o.center.z - dz);
    let b = Vec3::new(o.center.x + dx, y, o.center.z + dz);
    let mut log = tube(a, b, o.half.y * 0.42, 7, wood);
    tint_up(&mut log, lerp_c(wood, rgb(110, 150, 70), 0.5), scale_rgb(wood, 0.6));
    props.merge(&log);
    // a small leaf spray near one end
    let leaf = lerp_c(pal.accent, rgb(80, 150, 70), 0.5);
    props.merge(&leaf_cluster(b + Vec3::new(0.0, o.half.y * 0.4, 0.0), o.half.x * 0.5, seed ^ 3, leaf));
}

/// A cluster of mushrooms / fungal caps; faintly glowing in alien and toxic biomes.
fn prop_shrooms(theme: MapTheme, pal: &Palette, o: &Obstacle, seed: u32, props: &mut Mesh3, glow: &mut Mesh3) {
    let glows = matches!(theme, MapTheme::Alien | MapTheme::Toxic | MapTheme::Mistfall);
    let cap_col = match theme {
        MapTheme::Toxic => rgb(180, 230, 90),
        MapTheme::Alien => rgb(150, 120, 230),
        MapTheme::Mistfall => rgb(150, 210, 220),
        MapTheme::Volcanic => rgb(230, 120, 70),
        _ => rgb(210, 170, 120),
    };
    let stalk_col = lerp_c(pal.rock, rgb(210, 205, 190), 0.5);
    let n = 3 + (seed % 3) as i32;
    for i in 0..n {
        let h = crate::mesh::hash01(i, seed as i32);
        let h2 = crate::mesh::hash01(i + 31, seed as i32);
        let ox = (h - 0.5) * o.half.x * 1.4;
        let oz = (h2 - 0.5) * o.half.z * 1.4;
        let sh = o.half.y * (0.5 + h * 0.7);
        let base = Vec3::new(o.center.x + ox, o.center.y - o.half.y, o.center.z + oz);
        let capc = base + Vec3::new(0.0, sh, 0.0);
        props.merge(&tube(base, capc, sh * 0.16, 5, stalk_col));
        let cap = uv_sphere(capc, sh * 0.34, 7, 4, cap_col);
        if glows {
            let mut c = cap;
            c.emissive = 0.5;
            glow.merge(&c);
        } else {
            props.merge(&cap);
        }
    }
}

/// A single shipping crate with a lit top.
fn prop_crate(pal: &Palette, o: &Obstacle, key: i32, props: &mut Mesh3) {
    let mut cr = boxx(o.center, o.half, dither(pal.cover, key, 0.10), FACE_NO_BOTTOM);
    tint_up(&mut cr, lerp_c(pal.cover, rgb(255, 255, 255), 0.20), scale_rgb(pal.cover, 0.6));
    props.merge(&cr);
}

/// Two or three crates stacked and jittered.
fn prop_crate_stack(pal: &Palette, o: &Obstacle, seed: u32, props: &mut Mesh3) {
    let n = 2 + (seed % 2) as i32;
    let bw = o.half.x * 0.8;
    let bh = o.half.y * 0.62;
    for i in 0..n {
        let jit = ((seed.wrapping_add(i as u32 * 97) % 100) as f32 / 100.0 - 0.5) * o.half.x * 0.5;
        let c = Vec3::new(o.center.x + jit, o.center.y - o.half.y + bh * (2 * i + 1) as f32, o.center.z + jit * 0.7);
        let col = scale_rgb(pal.cover, 0.85 + (i as f32) * 0.08);
        let mut cr = boxx(c, Vec3::new(bw, bh, bw), col, FACE_NO_BOTTOM);
        tint_up(&mut cr, lerp_c(col, rgb(255, 255, 255), 0.18), scale_rgb(col, 0.6));
        props.merge(&cr);
    }
}

/// A hazard barrel with rim bands.
fn prop_barrel(o: &Obstacle, seed: u32, props: &mut Mesh3) {
    let r = o.half.x.min(o.half.z) * 0.92;
    let bot = o.center.with_y(o.center.y - o.half.y);
    let top = o.center.with_y(o.center.y + o.half.y);
    let bcol = match seed % 3 {
        0 => rgb(150, 70, 50),
        1 => rgb(96, 104, 74),
        _ => rgb(70, 90, 120),
    };
    let mut body = tube(bot, top, r, 10, bcol);
    tint_height(&mut body, bot.y, top.y, scale_rgb(bcol, 0.5), bcol);
    props.merge(&body);
    let mut cap = disc(top, r, 10, lerp_c(bcol, rgb(255, 255, 255), 0.18));
    tint_up(&mut cap, lerp_c(bcol, rgb(255, 255, 255), 0.25), scale_rgb(bcol, 0.7));
    props.merge(&cap);
    for f in [0.34, 0.7] {
        let y = bot.y + (top.y - bot.y) * f;
        props.merge(&tube(bot.with_y(y - 0.03), bot.with_y(y + 0.03), r * 1.04, 10, scale_rgb(bcol, 0.7)));
    }
}

/// A run of parallel floor pipes / conduits.
fn prop_pipes(pal: &Palette, o: &Obstacle, seed: u32, props: &mut Mesh3) {
    let col = scale_rgb(lerp_c(pal.trim, rgb(140, 140, 150), 0.4), 0.9);
    let r = o.half.y * 0.28;
    let len = o.half.z * 1.7;
    let along_x = seed & 1 == 0;
    for i in -1..=1 {
        let off = i as f32 * r * 2.4;
        let y = o.center.y - o.half.y + r * 1.1;
        let (a, b) = if along_x {
            (Vec3::new(o.center.x - len, y, o.center.z + off), Vec3::new(o.center.x + len, y, o.center.z + off))
        } else {
            (Vec3::new(o.center.x + off, y, o.center.z - len), Vec3::new(o.center.x + off, y, o.center.z + len))
        };
        props.merge(&tube(a, b, r, 8, scale_rgb(col, 0.85 + 0.1 * (i + 1) as f32)));
    }
}

/// A square structural pillar with a base and capital.
fn prop_pillar(pal: &Palette, o: &Obstacle, props: &mut Mesh3) {
    let col = lerp_c(pal.wall, rgb(200, 200, 205), 0.15);
    let h = o.half.y * 2.2;
    let shaft = Vec3::new(o.half.x * 0.5, h * 0.5, o.half.z * 0.5);
    let c = o.center.with_y(o.center.y - o.half.y + h * 0.5);
    let mut s = boxx(c, shaft, col, FACE_ALL);
    tint_height(&mut s, c.y - shaft.y, c.y + shaft.y, scale_rgb(col, 0.55), col);
    props.merge(&s);
    let cap = Vec3::new(o.half.x * 0.72, h * 0.06, o.half.z * 0.72);
    props.merge(&boxx(o.center.with_y(o.center.y - o.half.y + 0.05), cap, scale_rgb(col, 0.7), FACE_ALL));
    props.merge(&boxx(o.center.with_y(o.center.y - o.half.y + h), cap, scale_rgb(col, 0.8), FACE_ALL));
}

/// A machine console with a glowing readout.
fn prop_console(theme: MapTheme, pal: &Palette, o: &Obstacle, seed: u32, props: &mut Mesh3, glow: &mut Mesh3) {
    let col = scale_rgb(pal.cover, 0.8);
    let body = Vec3::new(o.half.x, o.half.y * 0.9, o.half.z * 0.7);
    let mut b = boxx(o.center.with_y(o.center.y - o.half.y * 0.1), body, col, FACE_NO_BOTTOM);
    tint_up(&mut b, lerp_c(col, rgb(255, 255, 255), 0.16), scale_rgb(col, 0.55));
    props.merge(&b);
    // a slanted glowing screen
    let gc = match (theme, seed % 3) {
        (MapTheme::Volcanic, _) => rgb(255, 150, 70),
        (_, 0) => rgb(110, 220, 180),
        (_, 1) => rgb(120, 180, 255),
        _ => rgb(230, 200, 110),
    };
    let scr = Vec3::new(o.half.x * 0.7, o.half.y * 0.5, 0.03);
    let scr_c = Vec3::new(o.center.x, o.center.y + o.half.y * 0.5, o.center.z + o.half.z * 0.66);
    let mut s = boxx(scr_c, scr, gc, FACE_ALL);
    s.emissive = 0.8;
    glow.merge(&s);
}

/// A pile of broken rubble (small jittered blocks).
fn prop_rubble(pal: &Palette, o: &Obstacle, seed: u32, props: &mut Mesh3) {
    let col = scale_rgb(pal.wall, 0.85);
    for i in 0..5u32 {
        let s = seed.wrapping_add(i.wrapping_mul(2_246_822_519));
        let jx = ((s % 100) as f32 / 100.0 - 0.5) * o.half.x * 1.4;
        let jz = (((s >> 8) % 100) as f32 / 100.0 - 0.5) * o.half.z * 1.4;
        let sz = o.half.x * (0.18 + (s % 30) as f32 / 100.0);
        let c = Vec3::new(o.center.x + jx, o.center.y - o.half.y + sz, o.center.z + jz);
        let mut blk = boxx(c, Vec3::new(sz, sz, sz), dither(col, s as i32, 0.12), FACE_NO_BOTTOM);
        rotate_y_about(&mut blk, c, (s % 360) as f32 * 0.0174);
        props.merge(&blk);
    }
}

/// A cluster of stacked rough stones (Urban barrier / generic cairn).
fn prop_cairn(pal: &Palette, o: &Obstacle, seed: u32, props: &mut Mesh3) {
    let floor_pt = o.center.with_y(o.center.y - o.half.y);
    for st in cairn(floor_pt, 3, o.half.x * 0.8, seed, pal.cover) {
        let mut s = st;
        tint_up(&mut s, lerp_c(pal.cover, rgb(255, 255, 255), 0.2), scale_rgb(pal.cover, 0.6));
        props.merge(&s);
    }
}

/// A leafy bush/fern clump on a short stem for the lush biomes.
fn prop_foliage(theme: MapTheme, pal: &Palette, o: &Obstacle, seed: u32, props: &mut Mesh3) {
    let leaf = match theme {
        MapTheme::Mistfall => rgb(120, 170, 150),
        MapTheme::Ice => rgb(170, 200, 200),
        MapTheme::Canopy => rgb(108, 150, 70),
        _ => rgb(96, 140, 60),
    };
    let stem = o.center.with_y(o.center.y - o.half.y);
    props.merge(&tube(stem, stem.with_y(stem.y + o.half.y * 0.8), o.half.x * 0.12, 5, rgb(80, 64, 44)));
    let crown = stem.with_y(stem.y + o.half.y * 0.9);
    props.merge(&leaf_cluster(crown, o.half.x * 1.1, seed, leaf));
    let _ = pal;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moped_wheels_are_not_degenerate() {
        // The wheel tubes used to have identical start/end points (a copy-paste `cz - 0.0` == `cz`), making
        // a zero-length cylinder where half the triangles were zero-area. A mesh audit harness
        // (examples/mesh_audit.rs) caught this via every Desert Town moped.
        let (meshes, _obs) = moped(10.0, 10.0, 0.0, 0.0, rgb(70, 110, 120));
        for m in &meshes {
            for tri in &m.tris {
                let a = m.verts[tri[0] as usize];
                let b = m.verts[tri[1] as usize];
                let c = m.verts[tri[2] as usize];
                let area = (b - a).cross(c - a).len() * 0.5;
                assert!(area > 1e-6, "moped() produced a degenerate (zero-area) triangle");
            }
        }
    }

    #[test]
    fn clay_building_walls_show_real_grime_and_streak_weathering() {
        let palette = Palette {
            floor: rgb(168, 146, 112),
            wall: rgb(184, 144, 104),
            trim: rgb(150, 120, 88),
            ceil: rgb(72, 64, 56),
            cover: rgb(170, 150, 120),
            sky: rgb(214, 196, 158),
            fog: rgb(206, 188, 152),
            accent: rgb(150, 110, 70),
            rock: rgb(158, 138, 112),
        };
        let (meshes, _obs) = clay_building_ext(10.0, 10.0, 4.0, 4.0, 2, 0, None, 0.0, palette, 4242);
        let lum = |c: Color32| 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;

        // Base-vs-top AO: average luminance of the lowest-quarter vertices should be visibly darker than
        // the highest-quarter vertices, across the whole building (not just one panel).
        let mut low_lum = Vec::new();
        let mut high_lum = Vec::new();
        let mut any_streak_contrast = false;
        for m in &meshes {
            let Some(vc) = &m.vcol else { continue };
            if vc.len() != m.verts.len() || vc.is_empty() {
                continue;
            }
            let ys: Vec<f32> = m.verts.iter().map(|v| v.y).collect();
            let (ylo, yhi) = (ys.iter().cloned().fold(f32::MAX, f32::min), ys.iter().cloned().fold(f32::MIN, f32::max));
            if (yhi - ylo) < 0.01 {
                continue; // flat mesh (e.g. a thin sill), no vertical gradient possible
            }
            for (v, c) in m.verts.iter().zip(vc.iter()) {
                let t = (v.y - ylo) / (yhi - ylo);
                if t < 0.25 {
                    low_lum.push(lum(*c));
                } else if t > 0.75 {
                    high_lum.push(lum(*c));
                }
            }
            // within-mesh spread beyond a pure gradient signals the streak decal actually painted something
            let spread = vc.iter().map(|c| lum(*c)).fold((f32::MAX, f32::MIN), |(mn, mx), l| (mn.min(l), mx.max(l)));
            if spread.1 - spread.0 > 15.0 {
                any_streak_contrast = true;
            }
        }
        assert!(!low_lum.is_empty() && !high_lum.is_empty(), "should find vertices near both the base and the roofline");
        let low_mean = low_lum.iter().sum::<f32>() / low_lum.len() as f32;
        let high_mean = high_lum.iter().sum::<f32>() / high_lum.len() as f32;
        assert!(high_mean > low_mean, "sun-bleached top should read brighter than grimy base ({high_mean:.1} vs {low_mean:.1})");
        assert!(any_streak_contrast, "at least one wall mesh should show streak-decal contrast beyond the flat AO gradient");
    }

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            let v = r.f32();
            assert!((0.0..1.0).contains(&v));
            let i = r.i32_range(3, 9);
            assert!((3..9).contains(&i));
        }
    }

    #[test]
    fn generation_is_deterministic_for_a_seed() {
        let mk = || Level::generate(MapConfig { seed: 12345, ..Default::default() });
        let a = mk();
        let b = mk();
        assert_eq!(a.cells, b.cells, "same seed ⇒ identical map");
        assert_eq!(a.rooms.len(), b.rooms.len());
    }

    #[test]
    fn map_has_rooms_floor_and_walls() {
        let lvl = Level::generate(MapConfig { seed: 1, ..Default::default() });
        assert!(lvl.rooms.len() >= 4, "got {} rooms", lvl.rooms.len());
        let floors = lvl.cells.iter().filter(|c| **c == Cell::Floor).count();
        let walls = lvl.cells.iter().filter(|c| **c == Cell::Wall).count();
        assert!(floors > 100, "should carve plenty of floor, got {floors}");
        assert!(walls > 40, "rooms must be walled, got {walls}");
    }

    #[test]
    fn every_room_center_is_walkable_and_walls_block() {
        let lvl = Level::generate(MapConfig { seed: 99, ..Default::default() });
        for r in &lvl.rooms {
            let (cx, cz) = r.rect.center();
            assert!(lvl.cell(cx, cz).walkable(), "room {} centre must be floor", r.id);
        }
        assert!(Cell::Wall.blocks() && Cell::Solid.blocks());
        assert!(!Cell::Floor.blocks() && Cell::Floor.walkable());
    }

    #[test]
    fn meshes_get_built_with_geometry() {
        let lvl = Level::generate(MapConfig { seed: 5, ..Default::default() });
        let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
        assert!(tris > 500, "baked level should have substantial geometry, got {tris}");
    }

    #[test]
    fn rooms_are_reachable_from_spawn() {
        // flood-fill from room 0 centre over Floor; every room centre must be reached (corridors connect).
        let lvl = Level::generate(MapConfig { seed: 2024, ..Default::default() });
        let (sx, sz) = lvl.rooms[0].rect.center();
        let mut seen = vec![false; (lvl.w * lvl.h) as usize];
        let mut stack = vec![(sx, sz)];
        while let Some((x, z)) = stack.pop() {
            if !lvl.in_bounds(x, z) {
                continue;
            }
            let idx = (z * lvl.w + x) as usize;
            if seen[idx] || !lvl.cell(x, z).walkable() {
                continue;
            }
            seen[idx] = true;
            stack.extend_from_slice(&[(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)]);
        }
        for r in &lvl.rooms {
            let (cx, cz) = r.rect.center();
            assert!(seen[(cz * lvl.w + cx) as usize], "room {} unreachable from spawn", r.id);
        }
    }

    #[test]
    fn every_biome_layout_is_connected() {
        // Each biome uses a different floor-plan carver (rooms / streets / bays / clearings). For all of
        // them, every room centre must be reachable from the spawn over walkable tiles.
        let themes = [
            MapTheme::Industrial,
            MapTheme::Bunker,
            MapTheme::Warehouse,
            MapTheme::Urban,
            MapTheme::Military,
            MapTheme::Volcanic,
            MapTheme::Ice,
            MapTheme::Alien,
        ];
        for theme in themes {
            for seed in [1u64, 7, 42, 1000] {
                let lvl = Level::generate(MapConfig { seed, theme, ..Default::default() });
                assert!(lvl.rooms.len() >= 2, "{:?} seed {} produced too few rooms", theme, seed);
                let (sx, sz) = lvl.rooms[0].rect.center();
                let mut seen = vec![false; (lvl.w * lvl.h) as usize];
                let mut stack = vec![(sx, sz)];
                while let Some((x, z)) = stack.pop() {
                    if !lvl.in_bounds(x, z) {
                        continue;
                    }
                    let idx = (z * lvl.w + x) as usize;
                    if seen[idx] || !lvl.cell(x, z).walkable() {
                        continue;
                    }
                    seen[idx] = true;
                    stack.extend_from_slice(&[(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)]);
                }
                for r in &lvl.rooms {
                    let (cx, cz) = r.rect.center();
                    assert!(seen[(cz * lvl.w + cx) as usize], "{:?} seed {}: room {} unreachable", theme, seed, r.id);
                }
            }
        }
    }

    #[test]
    fn varied_configs_are_connected() {
        // The seed-varied configs change size/rooms/corridor width/pits; every room must still be reachable.
        for seed in [1u64, 2, 3, 17, 99, 256, 4096, 65537] {
            let lvl = Level::generate(MapConfig::varied(seed));
            assert!(lvl.rooms.len() >= 2, "varied seed {} produced too few rooms", seed);
            let (sx, sz) = lvl.rooms[0].rect.center();
            let mut seen = vec![false; (lvl.w * lvl.h) as usize];
            let mut stack = vec![(sx, sz)];
            while let Some((x, z)) = stack.pop() {
                if !lvl.in_bounds(x, z) {
                    continue;
                }
                let idx = (z * lvl.w + x) as usize;
                if seen[idx] || !lvl.cell(x, z).walkable() {
                    continue;
                }
                seen[idx] = true;
                stack.extend_from_slice(&[(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)]);
            }
            for r in &lvl.rooms {
                let (cx, cz) = r.rect.center();
                assert!(seen[(cz * lvl.w + cx) as usize], "varied seed {}: room {} unreachable", seed, r.id);
            }
        }
    }
#[test]
fn wilds_tri_budget() {
    for theme in [crate::level::MapTheme::Canopy, crate::level::MapTheme::Mistfall, crate::level::MapTheme::Toxic] {
        let lvl = crate::level::Level::generate(crate::level::MapConfig { seed: 7, theme, ..Default::default() });
        let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
        eprintln!("{:?}: {} meshes, {} tris", theme, lvl.meshes.len(), tris);
        assert!(tris < 200_000, "{:?} produced {} tris — too heavy", theme, tris);
    }
}

#[test]
fn wilds_have_a_trail() {
    use crate::level::{Level, MapConfig, MapTheme};
    let wild = Level::generate(MapConfig { seed: 5, theme: MapTheme::Canopy, ..Default::default() });
    assert!(wild.trail.len() >= 2, "open biome should carve a footpath");
    let p = wild.trail[wild.trail.len() / 2];
    assert!(wild.trail_dist(p.x, p.y) < 0.001);
    let built = Level::generate(MapConfig { seed: 5, theme: MapTheme::Industrial, ..Default::default() });
    assert!(built.trail.is_empty(), "built biome has no footpath");
    assert_eq!(built.trail_dist(10.0, 10.0), f32::MAX);
}

#[test]
fn water_biomes_have_a_stream() {
    use crate::level::{Level, MapConfig, MapTheme};
    let mist = Level::generate(MapConfig { seed: 9, theme: MapTheme::Mistfall, ..Default::default() });
    assert!(mist.stream.len() >= 2, "Mistfall should carve a stream");
    let c = mist.stream[mist.stream.len() / 2];
    let bed = mist.floor_h(c.x, c.y);
    let bank = mist.floor_h(c.x + 8.0, c.y + 8.0);
    assert!(bed < bank + 0.3, "stream bed should sit low");
    let dry = Level::generate(MapConfig { seed: 9, theme: MapTheme::Canopy, ..Default::default() });
    assert!(dry.stream.is_empty(), "dry biome has no stream");
}

#[test]
fn custom_map_builds_playable_level() {
    use crate::builder::{AssetKind, CustomMap, Placed};
    use crate::level::Level;
    let mut m = CustomMap::new("Arena", crate::level::MapTheme::Canopy, 120);
    let (sx, sz) = m.spawn;
    // two solid assets + one non-solid
    m.placed.push(Placed { kind: AssetKind::Tree, x: sx + 10.0, z: sz, rot: 0.0, scale: 1.0, param: 0.0 });
    m.placed.push(Placed { kind: AssetKind::Boulder, x: sx, z: sz + 10.0, rot: 0.0, scale: 1.0, param: 0.0 });
    m.placed.push(Placed { kind: AssetKind::Bush, x: sx - 8.0, z: sz, rot: 0.0, scale: 1.0, param: 0.0 });
    let lvl = Level::from_custom(&m);
    // spawn lands on walkable ground
    let sp = lvl.spawn(0);
    assert!(lvl.has_floor_at(crate::math::Vec2::new(sp.x, sp.z)), "spawn should be on floor");
    // exactly the two solid assets became obstacles
    assert_eq!(lvl.obstacles.len(), 2, "solid assets become obstacles");
    assert!(lvl.meshes.len() > 3, "base + asset meshes present");
}

#[test]
fn new_natural_biomes_generate() {
    use crate::level::{Level, MapConfig, MapTheme};
    for theme in [MapTheme::Meadow, MapTheme::Redwood] {
        assert!(theme.open_sky(), "{:?} should be open-sky", theme);
        let lvl = Level::generate(MapConfig { seed: 3, theme, ..Default::default() });
        let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
        assert!(tris > 20_000, "{:?} should grow a populated wood ({} tris)", theme, tris);
    }
}

#[test]
fn biome_profiles_tri_budget() {
    for theme in [crate::level::MapTheme::Meadow, crate::level::MapTheme::Redwood, crate::level::MapTheme::Alien] {
        let lvl = crate::level::Level::generate(crate::level::MapConfig { seed: 7, theme, ..Default::default() });
        let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
        let obs = lvl.obstacles.len();
        eprintln!("{:?}: {} tris, {} obstacles", theme, tris, obs);
        assert!(tris < 220_000, "{:?} too heavy: {}", theme, tris);
    }
}

#[test]
fn open_biomes_get_ponds_and_understory_variety() {
    use crate::level::{Level, MapConfig, MapTheme};
    // Meadow should carve at least one pond on most seeds; the basin sits below its rim.
    let mut found = false;
    for seed in 0..6u64 {
        let lvl = Level::generate(MapConfig { seed, theme: MapTheme::Meadow, ..Default::default() });
        if let Some(&(c, r)) = lvl.ponds.first() {
            found = true;
            let bed = lvl.floor_h(c.x, c.y);
            let rim = lvl.floor_h(c.x + r + 4.0, c.y);
            assert!(bed <= rim + 0.2, "pond bed should sit at/below the rim");
            assert!(lvl.pond_dist(c.x, c.y) < 0.0, "centre is inside the pond");
        }
    }
    assert!(found, "Meadow should carve ponds on at least one seed");
}

#[test]
fn organic_landmarks_and_seasons_vary_by_seed() {
    use crate::level::{Level, MapConfig, MapTheme};
    // Generate many seeds so every landmark pick (incl. stone circle, log, waterfall) and every season is
    // exercised without panic, and each still produces a populated, connected level.
    let mut seasons = std::collections::HashSet::new();
    for seed in 0..16u64 {
        let lvl = Level::generate(MapConfig { seed, theme: MapTheme::Canopy, ..Default::default() });
        seasons.insert(lvl.season());
        let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
        assert!(tris > 20_000, "seed {} too sparse", seed);
    }
    assert!(seasons.len() >= 3, "seeds should span multiple seasons");
}

#[test]
fn lane_map_smoke() {
    use crate::level::{Level, PRESETS};
    let p = PRESETS.iter().find(|p| p.lanes > 0).unwrap();
    let lvl = Level::generate(p.config(7));
    let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
    eprintln!("{}: size {}, {} lanes paths, {} tris, {} obstacles", p.name, p.size, lvl.lane_paths.len(), tris, lvl.obstacles.len());
    assert!(!lvl.lane_paths.is_empty(), "lanes should exist");
    // a point on a lane should be clear of the nearest obstacle-dense jungle
    assert!(lvl.lane_dist(lvl.lane_paths[0][5].x, lvl.lane_paths[0][5].y) < 0.01);
}

#[test]
fn large_maps_are_chunked() {
    use crate::level::{Level, PRESETS};
    let p = PRESETS.iter().find(|p| p.lanes > 0).unwrap();
    let lvl = Level::generate(p.config(3));
    // chunked → many meshes, each with cached bounds
    assert!(lvl.meshes.len() > 20, "large map should be split into many chunks ({} meshes)", lvl.meshes.len());
    let with_bounds = lvl.meshes.iter().filter(|m| m.bounds.is_some()).count();
    assert!(with_bounds > lvl.meshes.len() / 2, "chunks should carry bounds");
    let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
    eprintln!("chunked lane map: {} chunks, {} tris", lvl.meshes.len(), tris);
}

#[test]
fn station_map_generates() {
    use crate::level::{Level, MapConfig, MapTheme};
    let lvl = Level::generate(MapConfig { seed: 9, theme: MapTheme::Station, size: 200, ..Default::default() });
    let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
    eprintln!("Station: {} meshes, {} tris, {} obstacles, {} rooms", lvl.meshes.len(), tris, lvl.obstacles.len(), lvl.rooms.len());
    assert!(!lvl.rooms.is_empty(), "station needs a spawn deck");
    assert!(lvl.obstacles.len() > 8, "station should have modules/crates as cover");
    assert!(tris > 1000 && tris < 400_000, "tri budget sane: {tris}");
    // spawn should be on a walkable deck tile
    let s = lvl.spawn(0);
    assert!(s.x.is_finite());
}

#[test]
fn all_presets_generate() {
    use crate::level::{Level, PRESETS};
    for (i, p) in PRESETS.iter().enumerate() {
        let lvl = Level::generate(p.config(i as u64 + 1));
        let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
        assert!(!lvl.rooms.is_empty(), "{} has no rooms", p.name);
        assert!(tris > 200, "{} too sparse ({tris} tris)", p.name);
        assert!(tris < 600_000, "{} over budget ({tris} tris)", p.name);
        let s = lvl.spawn(0);
        assert!(s.x.is_finite() && s.z.is_finite(), "{} bad spawn", p.name);
    }
}

#[test]
fn large_maps_get_dynamic_lod_field() {
    use crate::level::{Level, PRESETS};
    let p = PRESETS.iter().find(|p| p.lanes > 0).unwrap();
    let mut lvl = Level::generate(p.config(4));
    let field = lvl.lod_field.as_mut().expect("large map builds a LOD field");
    // settled at spawn → a mix of tiers; moving the camera far demotes more chunks
    let near_counts = field.tier_counts();
    field.settle(crate::math::Vec3::new(0.0, 0.0, -5000.0));
    let far_counts = field.tier_counts();
    eprintln!("near tiers {:?} -> far tiers {:?}", near_counts, far_counts);
    assert!(far_counts.2 >= near_counts.2, "moving away should not reduce impostors");
    // render list is one mesh per chunk
    assert_eq!(field.render_meshes().len(), field.chunks.len());
}

#[test]
fn compose_a_map_from_areas() {
    use crate::level::{Level, MapConfig, MapTheme};
    use crate::area::{AreaComposer, AreaKind};
    let mut lvl = Level::generate(MapConfig { seed: 2, theme: MapTheme::Meadow, size: 160, ..Default::default() });
    let tris_before = lvl.meshes.iter().map(|m| m.tri_count()).sum::<usize>();
    let obs_before = lvl.obstacles.len();
    let mut comp = AreaComposer::new();
    comp.place(AreaKind::Grove, (40.0, 40.0, 100.0, 100.0), 1)
        .place(AreaKind::RuinCluster, (110.0, 40.0, 180.0, 110.0), 2)
        .scatter(AreaKind::RockField, (40.0, 110.0, 180.0, 200.0), 4, 9);
    lvl.stamp_areas(&comp);
    let tris_after = lvl.meshes.iter().map(|m| m.tri_count()).sum::<usize>();
    assert!(tris_after > tris_before, "stamping areas adds geometry");
    assert!(lvl.obstacles.len() > obs_before, "stamping adds collision");
}

#[test]
fn setpiece_landmarks_generate_and_emit() {
    use crate::level::{Level, MapConfig, MapTheme};
    let themes = [MapTheme::Industrial, MapTheme::Urban, MapTheme::Military, MapTheme::Volcanic, MapTheme::Bunker, MapTheme::Warehouse];
    let mut saw_emissive = false;
    for th in themes {
        for seed in 0..8u64 {
            let lvl = Level::generate(MapConfig { seed, theme: th, size: 150, ..Default::default() });
            let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
            assert!(tris > 500, "{:?} seed {} too sparse", th, seed);
            if lvl.meshes.iter().any(|m| m.emissive > 0.5) {
                saw_emissive = true; // ember / neon / plasma glow
            }
        }
    }
    assert!(saw_emissive, "setpieces should contribute emissive glow on some maps");
}

#[test]
fn carrier_map_generates() {
    use crate::level::{Level, MapConfig, MapTheme};
    let lvl = Level::generate(MapConfig { seed: 5, theme: MapTheme::Carrier, size: 220, ..Default::default() });
    let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
    eprintln!("Carrier: {} meshes, {} tris, {} obstacles, {} rooms", lvl.meshes.len(), tris, lvl.obstacles.len(), lvl.rooms.len());
    assert!(!lvl.rooms.is_empty());
    assert!(tris > 2000 && tris < 400_000);
    // ocean + deck markings present (reflective + emissive meshes)
    assert!(lvl.meshes.iter().any(|m| m.reflect > 0.3), "ocean sheet present");
    assert!(lvl.meshes.iter().any(|m| m.emissive > 0.1), "deck markings/antenna present");
    let s = lvl.spawn(0);
    assert!(s.x.is_finite());
}

#[test]
fn open_maps_have_relief_and_cliffs_not_cages() {
    use crate::level::{Level, MapConfig, MapTheme};
    for th in [MapTheme::Canopy, MapTheme::Meadow, MapTheme::Redwood, MapTheme::Alien] {
        let lvl = Level::generate(MapConfig { seed: 4, theme: th, size: 170, ..Default::default() });
        // vertical relief: spread between lowest and highest walkable tile should be substantial
        let mut lo = f32::MAX; let mut hi = f32::MIN;
        for z in 0..lvl.h { for x in 0..lvl.w {
            if lvl.cell(x, z).walkable() {
                let h = lvl.floor_h((x as f32 + 0.5) * lvl.ts(), (z as f32 + 0.5) * lvl.ts());
                lo = lo.min(h); hi = hi.max(h);
            }
        }}
        let relief = hi - lo;
        eprintln!("{:?}: relief {:.1}", th, relief);
        assert!(relief > 6.0, "{:?} too flat (relief {:.1})", th, relief);
    }
    // cliffs replace flat walls on open biomes: there should be a tall rock cliff mesh, no perimeter wall mesh
    let lvl = Level::generate(MapConfig { seed: 4, theme: MapTheme::Canopy, size: 170, ..Default::default() });
    let max_y = lvl.meshes.iter().flat_map(|m| m.verts.iter()).map(|v| v.y).fold(f32::MIN, f32::max);
    assert!(max_y > 10.0, "cliffs should rise tall ({max_y:.1})");
}

#[test]
fn stamp_blockout_dresses_onto_level() {
    use crate::level::{Level, MapConfig, MapTheme};
    use crate::blockout::Blockout;
    let mut lvl = Level::generate(MapConfig { seed: 3, theme: MapTheme::Urban, size: 150, ..Default::default() });
    let tris0 = lvl.meshes.iter().map(|m| m.tri_count()).sum::<usize>();
    let obs0 = lvl.obstacles.len();
    // place the essay's chokepoint at map centre
    let (cx, cz) = (lvl.w as f32 * 0.5 * lvl.ts(), lvl.h as f32 * 0.5 * lvl.ts());
    let bo = Blockout::crashed_chokepoint(cx, cz, 7);
    lvl.stamp_blockout(&bo);
    assert!(lvl.meshes.iter().map(|m| m.tri_count()).sum::<usize>() > tris0, "blockout adds geometry");
    assert!(lvl.obstacles.len() >= obs0 + 5, "blockout adds collision for each tagged volume");
}

#[test]
fn new_setpieces_generate_and_emit() {
    use crate::level::{Level, MapConfig, MapTheme};
    // Across seeds the facility + volcanic dispatch should sometimes roll the new emissive setpieces;
    // every generated map must be panic-free and carry some emissive geometry from its landmark.
    for th in [MapTheme::Industrial, MapTheme::Bunker, MapTheme::Warehouse, MapTheme::Volcanic] {
        let mut any_emissive = false;
        for seed in 0..12u64 {
            let lvl = Level::generate(MapConfig { seed, theme: th, size: 130, ..Default::default() });
            if lvl.meshes.iter().any(|m| m.emissive > 0.0 && m.tri_count() > 0) {
                any_emissive = true;
            }
        }
        assert!(any_emissive, "{:?} should produce emissive setpiece geometry", th);
    }
}

#[test]
fn built_biome_wall_tris_reasonable() {
    use crate::level::{Level, MapConfig, MapTheme};
    for th in [MapTheme::Industrial, MapTheme::Bunker, MapTheme::Urban, MapTheme::Military, MapTheme::Warehouse] {
        let lvl = Level::generate(MapConfig { seed: 5, theme: th, size: 160, ..Default::default() });
        let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
        eprintln!("{:?}: {} tris", th, tris);
        assert!(tris < 260_000, "{:?} too heavy after wall articulation: {}", th, tris);
    }
}

#[test]
fn rooms_take_varied_shapes() {
    use crate::level::{Level, MapConfig, MapTheme};
    // Across seeds, the room biomes should produce some non-rectangular rooms: a room whose bounding rect
    // contains solid (carved-away) cells. Spawn (room 0) stays rectangular.
    let mut shaped_rooms = 0;
    for seed in 0..10u64 {
        let lvl = Level::generate(MapConfig { seed, theme: MapTheme::Bunker, size: 150, ..Default::default() });
        for (i, room) in lvl.rooms.iter().enumerate() {
            if i == 0 { continue; }
            let r = room.rect;
            let mut solids = 0;
            for z in r.z0..r.z1 { for x in r.x0..r.x1 {
                if !lvl.cell(x, z).walkable() { solids += 1; }
            }}
            if solids > 2 { shaped_rooms += 1; }
        }
    }
    assert!(shaped_rooms > 0, "expected some non-rectangular rooms across seeds");
}

#[test]
fn archetype_presets_pin_their_setpiece() {
    use crate::level::{Level, PRESETS};
    let names = ["Frozen Bifrost", "Titan's Ribcage", "Dragon Canyon", "Sandstone Gauntlet", "Colosseum Trenches",
        "Sunken Villa", "Server Tomb", "Rusting Midway", "Ghost Terminus"];
    for name in names {
        let p = PRESETS.iter().find(|p| p.name == name).expect("preset exists");
        // forced landmark should be set
        let cfg = p.config(42);
        assert!(cfg.landmark.is_some(), "{name} should pin a landmark");
        // generates without panic and produces geometry
        let lvl = Level::generate(cfg);
        let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
        assert!(tris > 1000, "{name} generated too little geometry");
    }
    // Bifrost's glass + glow are emissive
    let lvl = Level::generate(PRESETS.iter().find(|p| p.name == "Frozen Bifrost").unwrap().config(7));
    assert!(lvl.meshes.iter().any(|m| m.emissive > 0.0 && m.tri_count() > 0), "bifrost should glow");
}

#[test]
fn arena_layout_is_connected_and_open() {
    use crate::level::{Cell, Level, PRESETS};
    let p = PRESETS.iter().find(|p| p.name == "Colosseum Trenches").unwrap();
    let cfg = p.config(5);
    assert!(cfg.arena, "colosseum should use the arena layout");
    let lvl = Level::generate(cfg);
    // flood-fill from spawn; every room center must be reachable across the open disc
    let start = {
        let s = lvl.spawn(0);
        ((s.x / lvl.ts()) as i32, (s.z / lvl.ts()) as i32)
    };
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![start];
    while let Some((x, z)) = stack.pop() {
        if !seen.insert((x, z)) { continue; }
        for (dx, dz) in [(1,0),(-1,0),(0,1),(0,-1)] {
            let (nx, nz) = (x+dx, z+dz);
            if lvl.in_bounds(nx, nz) && lvl.cell(nx, nz).walkable() {
                stack.push((nx, nz));
            }
        }
    }
    for r in &lvl.rooms {
        let (cx, cz) = r.rect.center();
        assert!(seen.contains(&(cx, cz)), "room center unreachable in arena");
    }
    // the disc should be large and open: a big connected floor area
    let floor_tiles = (0..lvl.h).flat_map(|z| (0..lvl.w).map(move |x| (x, z)))
        .filter(|&(x, z)| lvl.cell(x, z) == Cell::Floor).count();
    assert!(floor_tiles > 800, "arena disc should be a large open floor ({floor_tiles})");
}

#[test]
fn pits_never_punch_holes_through_the_boundary() {
    use crate::level::{Cell, Level, MapConfig, MapTheme};
    // non-rectangular rooms + pits across seeds: no Void may sit on the border ring (which would be a hole
    // out of the world), and the outer two-tile ring must stay non-walkable (sealed boundary).
    for th in [MapTheme::Bunker, MapTheme::Industrial, MapTheme::Military] {
        for seed in 0..14u64 {
            let lvl = Level::generate(MapConfig { seed, theme: th, size: 150, pit_chance: 0.5, ..Default::default() });
            for z in 0..lvl.h {
                for x in 0..lvl.w {
                    let border = x <= 1 || z <= 1 || x >= lvl.w - 2 || z >= lvl.h - 2;
                    if border {
                        assert!(!lvl.cell(x, z).walkable(), "border tile walkable at {x},{z} ({th:?} seed {seed})");
                        assert!(lvl.cell(x, z) != Cell::Void, "Void on border at {x},{z}");
                    }
                }
            }
        }
    }
}

#[test]
fn terrain_is_smooth_enough_to_walk() {
    use crate::level::{Level, MapConfig, MapTheme};
    // after the ADS-compliant smoothing pass, sharp upward micro-bumps (a tile poking >0.6 above ALL of its
    // walkable neighbours) should be rare — these are exactly what makes the camera stutter.
    for th in [MapTheme::Meadow, MapTheme::Canopy, MapTheme::Redwood, MapTheme::Alien] {
        let lvl = Level::generate(MapConfig { seed: 3, theme: th, size: 160, ..Default::default() });
        let (mut spikes, mut walk) = (0u32, 0u32);
        for z in 1..lvl.h - 1 {
            for x in 1..lvl.w - 1 {
                if !lvl.cell(x, z).walkable() { continue; }
                walk += 1;
                let h = lvl.floor_h((x as f32 + 0.5) * lvl.ts(), (z as f32 + 0.5) * lvl.ts());
                let mut is_spike = true;
                for (dx, dz) in [(1,0),(-1,0),(0,1),(0,-1)] {
                    if lvl.cell(x+dx, z+dz).walkable() {
                        let nh = lvl.floor_h((x+dx) as f32 * lvl.ts() + lvl.ts()*0.5, (z+dz) as f32 * lvl.ts() + lvl.ts()*0.5);
                        if h <= nh + 0.6 { is_spike = false; break; }
                    }
                }
                if is_spike { spikes += 1; }
            }
        }
        let frac = spikes as f32 / walk.max(1) as f32;
        eprintln!("{:?}: {:.3}% spikes", th, frac * 100.0);
        assert!(frac < 0.02, "{:?} too bumpy: {:.1}% spike tiles", th, frac * 100.0);
    }
}

#[test]
fn fsm_cover_adds_contextual_cover_to_open_maps() {
    use crate::level::{Level, MapConfig, MapTheme};
    // a fresh open map, then an extra FSM pass: it should grow some sightline-breaking cover (geometry + collision)
    let mut lvl = Level::generate(MapConfig { seed: 8, theme: MapTheme::Meadow, size: 170, ..Default::default() });
    let o0 = lvl.obstacles.len();
    let m0 = lvl.meshes.len();
    lvl.stamp_fsm_cover(123);
    assert!(lvl.obstacles.len() > o0, "FSM should add cover collision");
    assert!(lvl.meshes.len() > m0, "FSM should add cover geometry");
}

#[test]
fn boundary_dissolver_drops_a_hazard_below_every_map() {
    use crate::level::{Level, MapConfig, MapTheme};
    // Every theme now gets a large hazard sheet far below the floor at the map's outer edge — walls and
    // ceilings were removed everywhere in favour of a skybox + cliff/hazard boundary, so what used to be
    // "open-sky biomes only" now applies to formerly-enclosed themes too (Bunker included).
    for th in [MapTheme::Mistfall, MapTheme::Meadow, MapTheme::Toxic, MapTheme::Bunker] {
        let lvl = Level::generate(MapConfig { seed: 2, theme: th, size: 150, ..Default::default() });
        let low = lvl.meshes.iter().any(|m| {
            m.tri_count() > 0 && m.verts.iter().all(|v| v.y < lvl.floor_y - 5.0)
        });
        assert!(low, "{:?} should have a boundary hazard sheet below the floor", th);
    }
    // Toxic's hazard is acidic (emissive); Bunker falls to the generic fog-abyss default.
    let acid = Level::generate(MapConfig { seed: 2, theme: MapTheme::Toxic, size: 150, ..Default::default() });
    assert!(acid.meshes.iter().any(|m| m.emissive > 0.3 && m.verts.iter().all(|v| v.y < acid.floor_y - 5.0)),
        "toxic boundary should glow");
    // Station/Carrier build their own custom deck-edge look instead — they opt out of the generic hazard.
    let station = Level::generate(MapConfig { seed: 2, theme: MapTheme::Station, size: 150, ..Default::default() });
    assert!(!station.meshes.iter().any(|m| m.tri_count() > 0 && m.verts.iter().all(|v| v.y < station.floor_y - 5.0)),
        "Station has its own deck-edge treatment, not the generic hazard sheet");
}

#[test]
fn no_map_has_an_overhead_ceiling_anymore() {
    use crate::level::{Level, MapConfig, MapTheme};
    // build_meshes() emits meshes in [floor, ceil, walls, trim, deco, props, arch_glow, prop_glow] order and
    // the `ceil` slot is kept but never populated (the overhead lid was removed). Larger maps get chunked
    // afterward (which reorders/drops the empty slot), so to check the invariant at the source we use a map
    // below the chunking threshold, where the slot order is still intact.
    for th in [MapTheme::Bunker, MapTheme::Warehouse, MapTheme::Urban, MapTheme::Military, MapTheme::Industrial] {
        let lvl = Level::generate(MapConfig { seed: 3, theme: th, size: 80, ..Default::default() });
        assert!(lvl.meshes.len() > 1, "{:?} should still emit the standard mesh slots", th);
        assert_eq!(lvl.meshes[1].tri_count(), 0, "{:?} the ceiling slot should be empty — no overhead lid", th);
    }
}

#[test]
fn reference_town_buildings_have_window_openings_and_stay_connected() {
    use crate::level::{Level, PRESETS};
    // The blueprint stamps buildings as solid Wall massing with zero depth or transparency; the window-punch
    // pass should open some single-exposed-face cells into shallow street-facing nooks without ever carving
    // a tunnel through a building or breaking the street network's connectivity.
    let p = PRESETS.iter().find(|p| p.name == "Reference Town").expect("Reference Town preset");
    for seed in [1u64, 4, 9] {
        let l = Level::generate(p.config(seed));
        assert!(crate::mapeval::score(&l).connected, "seed {seed}: town must stay connected after window punches");
        let wall_cells = (0..l.h).flat_map(|z| (0..l.w).map(move |x| (x, z))).filter(|&(x, z)| l.cell(x, z) == crate::level::Cell::Wall).count();
        assert!(wall_cells > 200, "seed {seed}: buildings should still be substantially solid, not dissolved (only {wall_cells} wall cells left)");
    }
}

#[test]
fn boundary_margin_gets_floor_and_dense_dressing_within_budget() {
    use crate::level::{Level, MapConfig, MapTheme};
    // The ring beyond the cliffs used to be empty space (no floor, nothing beyond the rock face) — this
    // checks it now has ground and dense clutter, and specifically guards against the tree-depth blowup
    // that once made this pass emit a million triangles on a single map (depth=3 recursive branching is
    // exponential; the fix is depth=1, kept here as a budget so it can't silently regress).
    for (theme, budget) in [(MapTheme::Volcanic, 260_000usize), (MapTheme::Bunker, 260_000)] {
        let lvl = Level::generate(MapConfig { seed: 2, theme, size: 168, ..Default::default() });
        let tris: usize = lvl.meshes.iter().map(|m| m.tri_count()).sum();
        assert!(tris < budget, "{:?} boundary dressing pushed the map over budget ({tris} tris)", theme);
        // A size-168 map is chunked into a grid for culling, so mesh *count* is now higher by design (one
        // mesh per non-empty chunk per source layer). The real guard is the triangle budget above — the
        // tree-depth blowup would blow the tri count, not just the mesh count. We still cap mesh count
        // generously to catch a pathological chunk explosion.
        assert!(lvl.meshes.len() < 4000, "{:?} chunked into an unreasonable number of meshes ({})", theme, lvl.meshes.len());
    }
}

#[test]
fn every_theme_gets_a_boundary_cliff_but_keeps_interior_walls() {
    use crate::level::{Level, MapConfig, MapTheme};
    // Formerly-enclosed themes now terminate the map edge in rock cliffs (pal.rock-coloured geometry)
    // instead of a flat perimeter wall — but their interior room/corridor walls must still exist as
    // ordinary blocking obstacles, or the level loses its gameplay structure entirely.
    for th in [MapTheme::Bunker, MapTheme::Warehouse, MapTheme::Urban] {
        let lvl = Level::generate(MapConfig { seed: 5, theme: th, size: 140, ..Default::default() });
        assert!(lvl.obstacles.len() > 20, "{:?} should still have plenty of interior wall/prop obstacles", th);
        assert!(crate::mapeval::score(&lvl).connected, "{:?} should still be a connected, playable layout", th);
    }
    // Cliff geometry specifically — checked on themes/seeds confirmed to actually carve up to the map edge
    // (some layouts leave an unused buffer before the true boundary and legitimately produce no cliff faces
    // that generation, same as they'd have produced no boundary wall faces before this change either).
    for th in [MapTheme::Bunker, MapTheme::Urban] {
        let lvl = Level::generate(MapConfig { seed: 5, theme: th, size: 140, ..Default::default() });
        let rock = lvl.pal.rock;
        let has_cliff = lvl.meshes.iter().any(|m| m.color == rock && m.tri_count() > 0);
        assert!(has_cliff, "{:?} should have boundary cliff geometry in the rock tone", th);
    }
}


#[test]
fn socket_flattens_a_pad_under_the_centerpiece() {
    use crate::level::{Level, MapConfig, MapTheme};
    // the centre of the map (where the landmark sits) should be a flat graded pad, not noisy ground
    for th in [MapTheme::Meadow, MapTheme::Canopy, MapTheme::Redwood] {
        let lvl = Level::generate(MapConfig { seed: 6, theme: th, size: 160, ..Default::default() });
        // sample heights in a tight ring around the most-central room
        let (mcx, mcz) = (lvl.w / 2, lvl.h / 2);
        // find nearest walkable to map centre as a proxy for the pad
        let mut hs = Vec::new();
        for dz in -2..=2 {
            for dx in -2..=2 {
                let (x, z) = (mcx + dx, mcz + dz);
                if lvl.in_bounds(x, z) && lvl.cell(x, z).walkable() {
                    hs.push(lvl.floor_h((x as f32 + 0.5) * lvl.ts(), (z as f32 + 0.5) * lvl.ts()));
                }
            }
        }
        if hs.len() >= 4 {
            let lo = hs.iter().cloned().fold(f32::MAX, f32::min);
            let hi = hs.iter().cloned().fold(f32::MIN, f32::max);
            assert!(hi - lo < 1.2, "{:?} centerpiece pad not flat enough: {:.2}", th, hi - lo);
        }
    }
}

#[test]
fn bridge_layout_is_connected_over_the_void() {
    // The Chasm Span carves a narrow span across an all-Void abyss with flanking chambers. Every room centre
    // (both spawns + both chambers) must be reachable over walkable tiles, and the map must be mostly void.
    for seed in [1u64, 5, 42, 1000] {
        let lvl = Level::generate(MapConfig { seed, theme: MapTheme::Bunker, size: 120, bridge: true, landmark: Some(4), ..Default::default() });
        assert!(lvl.rooms.len() >= 4, "bridge seed {} produced too few rooms", seed);
        let (sx, sz) = lvl.rooms[0].rect.center();
        let mut seen = vec![false; (lvl.w * lvl.h) as usize];
        let mut stack = vec![(sx, sz)];
        while let Some((x, z)) = stack.pop() {
            if !lvl.in_bounds(x, z) {
                continue;
            }
            let idx = (z * lvl.w + x) as usize;
            if seen[idx] || !lvl.cell(x, z).walkable() {
                continue;
            }
            seen[idx] = true;
            stack.extend_from_slice(&[(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)]);
        }
        for r in &lvl.rooms {
            let (cx, cz) = r.rect.center();
            assert!(seen[(cz * lvl.w + cx) as usize], "bridge seed {}: room {} unreachable across the span", seed, r.id);
        }
        // most of the map should be abyss (Void), not floor — it's a bridge, not an arena
        let mut floor = 0;
        let mut void = 0;
        for z in 0..lvl.h {
            for x in 0..lvl.w {
                match lvl.cell(x, z) {
                    Cell::Floor => floor += 1,
                    Cell::Void => void += 1,
                    _ => {}
                }
            }
        }
        assert!(void > floor * 3, "bridge seed {}: expected a vast void ({} void vs {} floor)", seed, void, floor);
    }
}

#[test]
fn tolkien_presets_generate_connect_and_pin_setpieces() {
    let names = ["Culvert Breach", "Chasm Span", "Fallen Capital", "Slaughter Camp", "Blighted Watchtower"];
    for name in names {
        let preset = PRESETS.iter().find(|p| p.name == name).expect("preset present");
        for seed in [3u64, 17, 99] {
            let lvl = Level::generate(preset.config(seed));
            assert!(lvl.rooms.len() >= 2, "{}: too few rooms", name);
            assert!(lvl.meshes.iter().map(|m| m.tri_count()).sum::<usize>() > 500, "{}: too little geometry", name);
            // connectivity from spawn
            let (sx, sz) = lvl.rooms[0].rect.center();
            let mut seen = vec![false; (lvl.w * lvl.h) as usize];
            let mut stack = vec![(sx, sz)];
            while let Some((x, z)) = stack.pop() {
                if !lvl.in_bounds(x, z) {
                    continue;
                }
                let idx = (z * lvl.w + x) as usize;
                if seen[idx] || !lvl.cell(x, z).walkable() {
                    continue;
                }
                seen[idx] = true;
                stack.extend_from_slice(&[(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)]);
            }
            for r in &lvl.rooms {
                let (cx, cz) = r.rect.center();
                assert!(seen[(cz * lvl.w + cx) as usize], "{} seed {}: room {} unreachable", name, seed, r.id);
            }
        }
    }
}

#[test]
fn culvert_is_two_tier_and_crosses_only_at_the_breach() {
    let preset = PRESETS.iter().find(|p| p.name == "Culvert Breach").unwrap();
    for seed in [2u64, 13, 77] {
        let lvl = Level::generate(preset.config(seed));
        let ts = lvl.ts();
        let (cx, cz) = (lvl.w / 2, lvl.h / 2);
        // 1) connectivity: south spawn reaches north spawn over walkable cells (through the breach)
        let (sx, sz) = lvl.rooms[0].rect.center();
        let mut seen = vec![false; (lvl.w * lvl.h) as usize];
        let mut stack = vec![(sx, sz)];
        while let Some((x, z)) = stack.pop() {
            if !lvl.in_bounds(x, z) {
                continue;
            }
            let idx = (z * lvl.w + x) as usize;
            if seen[idx] || !lvl.cell(x, z).walkable() {
                continue;
            }
            seen[idx] = true;
            stack.extend_from_slice(&[(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)]);
        }
        for r in &lvl.rooms {
            let (rx, rz) = r.rect.center();
            assert!(seen[(rz * lvl.w + rx) as usize], "culvert seed {}: room {} unreachable across the breach", seed, r.id);
        }
        // 2) the wall blocks away from the breach (a band cell well off-centre is solid)
        assert!(!lvl.cell(cx + 20, cz).walkable(), "culvert seed {}: wall should block off-breach", seed);
        // 3) the breach itself is open
        assert!(lvl.cell(cx, cz).walkable(), "culvert seed {}: breach should be open", seed);
        // 4) two-tier: an elevated wall-walk platform exists near the wall, well above the court
        let walk = lvl.platforms.iter().any(|p| p.top > lvl.floor_y + 3.5 && (p.z0 + p.z1) * 0.5 < (cz as f32 + 1.0) * ts && (p.z0 + p.z1) * 0.5 > (cz as f32 - 4.0) * ts);
        assert!(walk, "culvert seed {}: no elevated wall-walk platform", seed);
        // 5) the southern trench is sunk below the northern court
        let south = lvl.floor_h((cx as f32 + 0.5) * ts, (lvl.h as f32 - 8.0) * ts);
        let north = lvl.floor_h((cx as f32 + 0.5) * ts, 8.0 * ts);
        assert!(south < north - 0.8, "culvert seed {}: trench ({:.2}) not sunk below court ({:.2})", seed, south, north);
    }
}

#[test]
fn open_biomes_grow_rideable_air_currents() {
    use crate::level::{Level, MapConfig, MapTheme};
    let lvl = Level::generate(MapConfig { seed: 4, theme: MapTheme::Meadow, size: 180, ..Default::default() });
    assert!(!lvl.air_currents.is_empty(), "meadow should have an air current");
    let ac = &lvl.air_currents[0];
    // a point on the flow line returns a flow direction; a point far off it does not
    let mid = ac.path[ac.path.len() / 2];
    assert!(ac.flow_at(mid).is_some(), "midline should be inside the current");
    let off = crate::math::Vec2::new(mid.x + 50.0, mid.y + 50.0);
    assert!(ac.flow_at(off).is_none(), "a far point should be outside the current");
    // enclosed biomes get none
    let bunker = Level::generate(MapConfig { seed: 4, theme: MapTheme::Bunker, size: 150, ..Default::default() });
    assert!(bunker.air_currents.is_empty(), "enclosed biomes have no air currents");
}

#[test]
fn hollow_forest_lights_a_glowing_gel_core() {
    use crate::level::{Level, PRESETS};
    let preset = PRESETS.iter().find(|p| p.name == "Hollow Forest").unwrap();
    for seed in [3u64, 31, 88] {
        let lvl = Level::generate(preset.config(seed));
        // the gel core is a strongly emissive centerpiece — there should be bright emissive geometry up high
        let lit = lvl.meshes.iter().any(|m| m.emissive > 0.7 && m.tri_count() > 0 && m.verts.iter().any(|v| v.y > lvl.floor_y + 2.0));
        assert!(lit, "hollow forest seed {} should glow with a gel core", seed);
        // and it should ride an air current too (Canopy is open-sky)
        assert!(!lvl.air_currents.is_empty(), "hollow forest should have an air current");
    }
}

#[test]
fn built_biomes_get_skylights_open_biomes_dont() {
    use crate::level::{Level, MapConfig, MapTheme};
    let bunker = Level::generate(MapConfig { seed: 5, theme: MapTheme::Bunker, size: 150, ..Default::default() });
    assert!(!bunker.skylights.is_empty(), "a built bunker should have roof openings");
    let meadow = Level::generate(MapConfig { seed: 5, theme: MapTheme::Meadow, size: 150, ..Default::default() });
    assert!(meadow.skylights.is_empty(), "open biomes have no roof to open");
}

#[test]
fn cryo_lab_has_its_overturned_tank() {
    use crate::level::{Level, PRESETS};
    let p = PRESETS.iter().find(|p| p.name == "Cryo Lab").unwrap();
    for seed in [2u64, 19, 50] {
        let lvl = Level::generate(p.config(seed));
        // the tank setpiece glows (oxidised cable tips + CRT screen) above the floor
        let lit = lvl.meshes.iter().any(|m| m.emissive > 0.4 && m.tri_count() > 8 && m.verts.iter().any(|v| v.y > lvl.floor_y + 1.0));
        assert!(lit, "cryo lab seed {} should show the glowing tank detail", seed);
    }
}

#[test]
fn detritus_is_cosmetic_and_keeps_maps_traversable() {
    use crate::level::{Level, MapConfig, MapTheme};
    // built maps get strewn with clutter, but it must not wall anything off or bury the spawn.
    for th in [MapTheme::Industrial, MapTheme::Urban, MapTheme::Ice] {
        let lvl = Level::generate(MapConfig { seed: 8, theme: th, size: 140, ..Default::default() });
        assert!(crate::mapeval::score(&lvl).connected, "{:?} stayed connected after scatter", th);
        // spawn tile is clean floor
        let s = lvl.spawn(0);
        assert!(lvl.has_floor_at(s.xz()), "spawn should be on clear floor");
    }
}

#[test]
fn story_vignettes_add_authored_cover_without_sealing_maps() {
    use crate::level::{Level, MapConfig, MapTheme};
    // a few seeds/biomes: vignettes should appear (extra obstacles vs a baseline) and never disconnect a map.
    let mut saw_vignette = false;
    for (th, seed) in [(MapTheme::Military, 4u64), (MapTheme::Meadow, 9), (MapTheme::Urban, 14), (MapTheme::Canopy, 21)] {
        let lvl = Level::generate(MapConfig { seed, theme: th, size: 150, ..Default::default() });
        assert!(crate::mapeval::score(&lvl).connected, "{:?}/{} disconnected by a vignette", th, seed);
        // vignettes drop cover obstacles; with rooms>=3 most maps get at least one
        if lvl.rooms.len() >= 3 && lvl.obstacles.len() > 4 {
            saw_vignette = true;
        }
    }
    assert!(saw_vignette, "expected vignettes to place cover somewhere");
}

#[test]
fn water_biomes_register_animated_water_bodies() {
    use crate::level::{Level, MapConfig, MapTheme};
    for th in [MapTheme::Mistfall, MapTheme::Volcanic, MapTheme::Toxic] {
        let lvl = Level::generate(MapConfig { seed: 6, theme: th, size: 150, ..Default::default() });
        assert!(!lvl.water.is_empty(), "{:?} should have a water/lava surface", th);
    }
    // Volcanic surface is flagged lava (low reflect, emissive); Mistfall is reflective water.
    let lava = Level::generate(MapConfig { seed: 6, theme: MapTheme::Volcanic, size: 150, ..Default::default() });
    assert!(lava.water.iter().any(|w| w.lava), "volcanic surface should be lava");
    let mist = Level::generate(MapConfig { seed: 6, theme: MapTheme::Mistfall, size: 150, ..Default::default() });
    assert!(mist.water.iter().all(|w| !w.lava && w.reflect > 0.4), "mistfall water should be reflective");
    // a dry built biome registers none
    let bunker = Level::generate(MapConfig { seed: 6, theme: MapTheme::Bunker, size: 150, ..Default::default() });
    assert!(bunker.water.is_empty(), "a dry bunker has no water");
}

#[test]
fn palette_variants_diversify_but_stay_legible() {
    use crate::level::{palette_variant, MapTheme};
    let base = MapTheme::Industrial.palette();
    let mut seen = std::collections::HashSet::new();
    for seed in 0..40u64 {
        let p = palette_variant(base, seed);
        seen.insert((p.floor.r(), p.floor.g(), p.floor.b()));
        // floor must stay readable against the wall (WCAG-ish: at least a faint step)
        let lum = |c: egui::Color32| 0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32;
        assert!((lum(p.floor) - lum(p.wall)).abs() > 1.0 || p.floor != p.wall, "floor/wall collapsed at seed {}", seed);
    }
    assert!(seen.len() >= 3, "expected several distinct floor tones, got {}", seen.len());
}

#[test]
fn texturize_adds_per_vertex_grain() {
    use crate::mesh::{boxx, rgb, FACE_TOP};
    // a flat slab with uniform colour → after texturize, vertex colours vary
    let mut m = boxx(crate::math::Vec3::new(20.0, 0.0, 20.0), crate::math::Vec3::new(8.0, 0.1, 8.0), rgb(120, 120, 120), FACE_TOP);
    let before = m.vcol.clone().unwrap();
    m.texturize(0.6, 0.15, 7);
    let after = m.vcol.clone().unwrap();
    let changed = before.iter().zip(after.iter()).filter(|(a, b)| a != b).count();
    assert!(changed > 0, "texturize should modulate vertex colours");
    // and the variation isn't uniform (different verts shift differently)
    let distinct: std::collections::HashSet<_> = after.iter().map(|c| (c.r(), c.g(), c.b())).collect();
    assert!(distinct.len() > 1, "grain should vary across the surface");
}

#[test]
fn graph_skinned_maps_inherit_the_flow_they_were_selected_for() {
    use crate::level::{Level, PRESETS};
    for name in ["Proving Ground", "Crossfire Mesa"] {
        let p = PRESETS.iter().find(|p| p.name == name).unwrap();
        for seed in [1u64, 7, 23] {
            let lvl = Level::generate(p.config(seed));
            let sc = crate::mapeval::score(&lvl);
            assert!(sc.connected, "{} seed {} must be connected (graph guarantees it)", name, seed);
            // graph maps are selected for balanced flow — they should clear a solid bar and beat bare corridors
            assert!(sc.total >= 50.0, "{} seed {} scored {:.1}", name, seed, sc.total);
            assert!(sc.flow > 0.35, "{} seed {} flow {:.2} should reflect the selected topology", name, seed, sc.flow);
            // graph elevation now drives the terrain — towers rise, trenches sink → real height spread
            assert!(sc.verticality > 0.15, "{} seed {} should have graph-driven verticality, got {:.2}", name, seed, sc.verticality);
            // rooms came from graph nodes → a handful of distinct spaces
            assert!(lvl.rooms.len() >= 6, "{} should have several graph rooms, got {}", name, lvl.rooms.len());
        }
    }
}

#[test]
fn blighted_watchtower_has_flanking_side_lanes() {
    use crate::level::{Level, PRESETS};
    let p = PRESETS.iter().find(|p| p.name == "Blighted Watchtower").unwrap();
    let with_lanes = Level::generate(p.config(4));
    assert!(crate::mapeval::score(&with_lanes).connected, "watchtower must stay connected with lanes");
    // the same arena without side lanes has strictly less floor (the lanes added walkable ground)
    let mut cfg = p.config(4);
    cfg.side_lanes = false;
    let plain = Level::generate(cfg);
    let count = |l: &Level| (0..l.h).flat_map(|z| (0..l.w).map(move |x| (x, z))).filter(|&(x, z)| l.cell(x, z) == crate::level::Cell::Floor).count();
    assert!(count(&with_lanes) > count(&plain), "side lanes should add walkable floor");
}

#[test]
fn salt_mine_has_its_ore_cart_setpiece() {
    use crate::level::{Level, PRESETS};
    let p = PRESETS.iter().find(|p| p.name == "Salt Mine").unwrap();
    for seed in [3u64, 14, 40] {
        let lvl = Level::generate(p.config(seed));
        assert!(crate::mapeval::score(&lvl).connected, "salt mine seed {} connected", seed);
        // the pink crystal pile is faintly emissive and sits up off the floor in the cart
        let has_crystal = lvl.meshes.iter().any(|m| m.emissive > 0.1 && m.emissive < 0.5 && m.tri_count() > 8
            && m.verts.iter().any(|v| v.y > lvl.floor_y + 0.7));
        assert!(has_crystal, "salt mine seed {} should show the ore-cart crystals", seed);
    }
}

#[test]
fn berthing_docks_build_a_flooded_split_level() {
    use crate::level::{Level, PRESETS};
    let p = PRESETS.iter().find(|p| p.name == "Berthing Docks").unwrap();
    for seed in [2u64, 11, 33] {
        let lvl = Level::generate(p.config(seed));
        assert!(crate::mapeval::score(&lvl).connected, "docks seed {} must stay connected", seed);
        // the basin floods (a water body), the container labyrinth adds cover, the walkways are raised platforms
        assert!(!lvl.water.is_empty(), "docks seed {} should flood the basin", seed);
        assert!(lvl.obstacles.len() >= 8, "docks seed {} should have a container labyrinth, got {}", seed, lvl.obstacles.len());
        let raised = lvl.platforms.iter().filter(|pl| pl.top > lvl.floor_y + 1.5).count();
        assert!(raised >= 2, "docks seed {} should have raised walkways, got {}", seed, raised);
    }
}

#[test]
fn agent_maps_skin_emergent_terrain_into_a_connected_level() {
    use crate::level::{Level, PRESETS};
    let p = PRESETS.iter().find(|p| p.name == "Contested Ground").unwrap();
    for seed in [5u64, 18, 41] {
        let lvl = Level::generate(p.config(seed));
        let sc = crate::mapeval::score(&lvl);
        assert!(sc.connected, "agent map seed {} must stay connected (open base guarantees it)", seed);
        // the emergent elevation field became the terrain
        assert!(!lvl.agent_height.is_empty(), "agent map seed {} should carry an emergent height field", seed);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for z in 0..lvl.h { for x in 0..lvl.w {
            if lvl.cell(x, z) == crate::level::Cell::Floor { let h = lvl.tile_h(x, z); lo = lo.min(h); hi = hi.max(h); }
        }}
        assert!(hi - lo > 1.2, "agent map seed {} should show the snipers' hills, spread {:.2}", seed, hi - lo);
        // campers piled cover
        assert!(lvl.obstacles.len() >= 8, "agent map seed {} should have cover pockets, got {}", seed, lvl.obstacles.len());
    }
}

#[test]
fn three_lane_maps_have_lanes_a_power_tower_and_an_objective() {
    use crate::level::{Level, PRESETS};
    for name in ["Triad Foundry", "Vanguard Reach", "Pinnacle"] {
        let p = PRESETS.iter().find(|p| p.name == name).unwrap();
        for seed in [1u64, 9, 22] {
            let lvl = Level::generate(p.config(seed));
            let sc = crate::mapeval::score(&lvl);
            assert!(sc.connected, "{} seed {} must be connected (lanes + connectors)", name, seed);
            // a contested central power tower: a raised platform near the map centre
            let (cx, cz) = (lvl.w as f32 * 0.5 * lvl.ts(), lvl.h as f32 * 0.5 * lvl.ts());
            let tower = lvl.platforms.iter().any(|pl| pl.top > lvl.floor_y + 1.8
                && pl.x0 <= cx && pl.x1 >= cx && pl.z0 <= cz && pl.z1 >= cz);
            assert!(tower, "{} seed {} should have a central power tower", name, seed);
            // the power pickup that makes the high ground worth fighting for
            assert!(!lvl.custom_pickups.is_empty(), "{} seed {} should place a power pickup", name, seed);
            // deliberate cover rhythm
            assert!(lvl.obstacles.len() >= 12, "{} seed {} should have a cover rhythm, got {}", name, seed, lvl.obstacles.len());
            // and the cover should give a healthy flow (mixed engagement ranges), not one long sightline
            assert!(sc.flow > 0.45, "{} seed {} flow {:.2} should reflect cover-broken sightlines", name, seed, sc.flow);
        }
    }
}

#[test]
fn graph_maps_get_coherent_functional_spaces() {
    use crate::level::{Level, PRESETS};
    use crate::player::audit_landings;
    for name in ["Crossfire Mesa", "Proving Ground"] {
        let p = PRESETS.iter().find(|p| p.name == name).unwrap();
        for seed in [2u64, 13, 27] {
            let lvl = Level::generate(p.config(seed));
            assert!(crate::mapeval::score(&lvl).connected, "{} seed {} connected", name, seed);
            // the plan was consumed into real structure
            assert!(lvl.space_plan.is_empty(), "{} space plan should be stamped, not left pending", name);
            assert!(lvl.obstacles.len() >= 10, "{} should have functional cover, got {}", name, lvl.obstacles.len());
            // and every prop is standable, not a sink-hole
            let a = audit_landings(&lvl, 800, 7);
            assert!(a.clean_rate() > 0.98, "{} seed {} functional props must be standable ({:.1}%)", name, seed, a.clean_rate() * 100.0);
        }
    }
}

#[test]
fn biomes_wear_distinct_materials_and_trees() {
    use crate::level::MapTheme;
    use crate::mesh::{Material, TreeKind};
    // different biomes choose meaningfully different surface materials
    let urban = MapTheme::Urban.materials(1);
    let ice = MapTheme::Ice.materials(1);
    let warehouse = MapTheme::Warehouse.materials(1);
    assert_ne!(urban.1, ice.1, "urban brick/concrete should differ from ice walls");
    assert!(matches!(MapTheme::Ice.materials(1).0, Material::Snow), "ice ground is snow");
    assert!(matches!(warehouse.1, Material::Corrugated), "warehouse walls are ribbed steel");
    // seed changes the choice for at least some biomes (cross-map variety)
    let a = MapTheme::Urban.materials(0);
    let b = MapTheme::Urban.materials(7);
    assert!(a != b || MapTheme::Industrial.materials(0) != MapTheme::Industrial.materials(7), "seed should vary materials");
    // tree silhouettes differ by biome
    assert_eq!(MapTheme::Redwood.tree_kind(), TreeKind::Conifer);
    assert_eq!(MapTheme::Toxic.tree_kind(), TreeKind::Snag);
    assert_eq!(MapTheme::Alien.tree_kind(), TreeKind::Frond);
    assert_ne!(MapTheme::Meadow.tree_kind(), MapTheme::Redwood.tree_kind());
}

#[test]
fn each_tree_style_builds_geometry() {
    use crate::math::Vec3;
    use crate::mesh::{rgb, tree_styled, TreeKind};
    for k in [TreeKind::Broadleaf, TreeKind::Conifer, TreeKind::Snag, TreeKind::Frond] {
        let parts = tree_styled(k, Vec3::new(0.0, 0.0, 0.0), 5.0, 0.3, 2, 11, rgb(90, 60, 40), rgb(60, 140, 70));
        let tris: usize = parts.iter().map(|m| m.tri_count()).sum();
        assert!(tris > 8, "{:?} should build real geometry, got {} tris", k, tris);
    }
}

#[test]
fn destructibles_place_block_and_open_routes() {
    use crate::destructible::DState;
    use crate::math::Vec2;
    use crate::player::{audit_landings, body_blocked};
    // find a preset that places destructibles
    let cfg = PRESETS
        .iter()
        .map(|p| p.config(7))
        .find(|c| !Level::generate(c.clone()).destructibles.is_empty())
        .expect("some preset should place destructibles");
    let mut l = Level::generate(cfg);
    assert!(!l.destructibles.is_empty());
    let d = l.destructibles[0];
    let probe = Vec2::new(d.center.x, d.center.z);
    // intact → blocks at its own height band, and is standable (clean landings)
    assert!(body_blocked(&l, probe, 0.34, d.center.y, 1.0), "intact destructible blocks");
    assert!(audit_landings(&l, 1200, 4).clean_rate() > 0.98, "intact destructibles are standable");
    // destroyed → stops blocking, opening the route
    l.destructibles[0].state = DState::Destroyed;
    assert!(!body_blocked(&l, probe, 0.34, d.center.y, 1.0), "destroyed destructible opens the route");
}

#[test]
fn new_structures_generate_connected_and_standable() {
    use crate::player::audit_landings;
    for name in ["Spillway Dam", "Rail Yard", "Plaza Fountain", "Frozen Lake", "Cliff Pass"] {
        let p = PRESETS.iter().find(|p| p.name == name).expect("showcase preset exists");
        for seed in [3u64, 19, 40] {
            let l = Level::generate(p.config(seed));
            assert!(crate::mapeval::score(&l).connected, "{} seed {} connected", name, seed);
            assert!(l.obstacles.len() > 20, "{} should have structure", name);
            // the new structures' crests, ledges, platforms, floes and drifts must be stand-on-able
            let a = audit_landings(&l, 1500, 11);
            assert!(a.clean_rate() > 0.98, "{} seed {} structures must be standable ({:.1}%)", name, seed, a.clean_rate() * 100.0);
        }
    }
}

#[test]
fn flood_zone_authored_map_generates_and_cycles() {
    use crate::player::audit_landings;
    let p = PRESETS.iter().find(|p| p.name == "Flood Zone").expect("Flood Zone preset");
    let mut l = Level::generate(p.config(7));
    // authored, not procedural
    assert_eq!(l.cfg.authored, Some(0));
    assert!(crate::mapeval::score(&l).connected, "flood zone is connected");
    assert_eq!(l.flood.len(), 2, "two spillway channels");
    assert_eq!(l.water.len(), 2, "each channel drives a water surface");
    assert!(audit_landings(&l, 2000, 9).clean_rate() > 0.98, "basin slabs/banks are standable");
    // the cycle: dry at the start, deep at the peak, dry again after draining
    let ch = &mut l.flood[0];
    ch.phase = 1.0;
    assert!(!ch.flooded(), "dry just after the cycle begins");
    ch.phase = ch.delay + ch.rise + 1.0;
    assert!(ch.flooded(), "flooded while held at crest");
    assert!(ch.surface() > ch.bed_y + 0.3);
    ch.phase = ch.delay + ch.rise + ch.hold + ch.fall + 1.0;
    assert!(!ch.flooded(), "drained back to dry");
}

#[test]
fn flood_zone_runs_in_a_game_and_floods_during_play() {
    use crate::game::Game;
    use crate::loadout::Loadout;
    let p = PRESETS.iter().position(|p| p.name == "Flood Zone").unwrap();
    let mut g = Game::new_preset(7, Some(p), Loadout::default());
    let bed = -1.7f32;
    let mut saw_flood = false;
    let input = crate::player::Input::default();
    for _ in 0..1800 {
        g.step(&input, 1.0 / 60.0);
        if g.flood_surface(0).map_or(false, |y| y > bed + 0.3) {
            saw_flood = true;
        }
    }
    assert!(saw_flood, "the channel should surge above its bed during normal play");
}

#[test]
fn reference_town_stamps_a_connected_street_grid() {
    use crate::player::audit_landings;
    let p = PRESETS.iter().find(|p| p.name == "Reference Town").expect("Reference Town preset");
    let l = Level::generate(p.config(7));
    assert_eq!(l.cfg.authored, Some(1));
    assert!(crate::mapeval::score(&l).connected, "the extracted street network is connected");
    // buildings became solid wall massing, streets/yards are walkable
    let walls = l.cells.iter().filter(|&&c| c == crate::level::Cell::Wall).count();
    let floors = l.cells.iter().filter(|&&c| c == crate::level::Cell::Floor).count();
    assert!(walls > 200, "building footprints became massing ({} wall cells)", walls);
    assert!(floors > walls, "the walkable street/yard network dominates");
    assert_eq!(l.rooms.len(), 2, "two spawns on the street network");
    assert!(audit_landings(&l, 2000, 9).clean_rate() > 0.98, "streets are clean to land/move on");
}

#[test]
fn desert_town_builds_with_standable_roofs_and_a_sandstorm() {
    use crate::player::audit_landings;
    let p = PRESETS.iter().find(|p| p.name == "Desert Town").expect("Desert Town preset");
    let l = Level::generate(p.config(7));
    assert_eq!(l.cfg.authored, Some(2));
    assert!(crate::mapeval::score(&l).connected, "streets/alleys/courtyard form a connected network");
    assert!(l.obstacles.len() > 400, "clay buildings produce real wall+roof massing ({} obs)", l.obstacles.len());
    assert!(l.destructibles.len() > 10, "earthquake rubble present");
    // roofs, sills and props are all standable, nothing to sink into
    assert!(audit_landings(&l, 2500, 9).clean_rate() > 0.98, "roofs and streets are clean");
    // the sandstorm cycles dry -> peak -> dry
    let mut st = l.sandstorm.expect("sandstorm present");
    st.phase = 1.0;
    assert_eq!(st.intensity(), 0.0, "clear at the start");
    st.phase = st.delay + st.ramp + 1.0;
    assert!(st.intensity() > 0.9, "full storm at the peak");
    st.phase = st.delay + st.ramp + st.hold + st.ease + 1.0;
    assert_eq!(st.intensity(), 0.0, "cleared after passing");
}

#[test]
fn sandstorm_rises_during_play() {
    use crate::game::Game;
    use crate::loadout::Loadout;
    let p = PRESETS.iter().position(|p| p.name == "Desert Town").unwrap();
    let mut g = Game::new_preset(7, Some(p), Loadout::default());
    let input = crate::player::Input::default();
    let mut peak = 0.0f32;
    let mut baseline_end = 0.0f32;
    let mut min_end = f32::MAX;
    for i in 0..2400 {
        g.step(&input, 1.0 / 60.0);
        if i == 1 {
            baseline_end = g.fog.end; // storm not yet risen
        }
        if g.storm > 0.5 {
            min_end = min_end.min(g.fog.end);
        }
        peak = peak.max(g.storm);
    }
    assert!(peak > 0.8, "the storm should reach high intensity during play (got {:.2})", peak);
    assert!(min_end < baseline_end * 0.75, "visibility should drop during the storm (base {:.0} -> {:.0})", baseline_end, min_end);
}

#[test]
fn authored_showcase_maps_score_well() {
    // Locks in the map-quality work: all three hand-authored showcase maps must grade at least C (>=60),
    // stay connected, and expose real vertical interest (rooftops/platforms) and freestanding cover so the
    // scorer reads them as the deliberately-designed arenas they are, not sparse or over-cluttered boxes.
    use crate::mapeval::score;
    for name in ["Flood Zone", "Reference Town", "Desert Town"] {
        let p = PRESETS.iter().find(|p| p.name == name).unwrap();
        let l = Level::generate(p.config(42));
        let sc = score(&l);
        assert!(sc.connected, "{name} must be fully connected");
        assert!(sc.total >= 60.0, "{name} should score at least C-grade (got {:.0})", sc.total);
        assert!(!l.platforms.is_empty(), "{name} should register standable platforms for verticality");
        assert!(!l.cover_obstacles.is_empty(), "{name} should register freestanding cover props");
        assert!(l.open_air, "{name} is an outdoor map and should be flagged open-air");
    }
    // The Pyre Garage is a different archetype: an enclosed, smoke-choked multi-story car park. It scores
    // lower on openness by design (no sky), so we hold it to a looser overall bar but still require it to be
    // connected, fully standable, and rich with vertical structure, cover and its signature fire sources.
    {
        let p = PRESETS.iter().find(|p| p.name == "Pyre Garage").unwrap();
        let l = Level::generate(p.config(42));
        let sc = score(&l);
        assert!(sc.connected, "Pyre Garage must be fully connected");
        assert!(sc.total >= 44.0, "Pyre Garage should still be a playable D-grade tight map (got {:.0})", sc.total);
        assert!(l.platforms.len() > 10, "Pyre Garage should have rich vertical structure (ramps, decks)");
        assert!(!l.cover_obstacles.is_empty(), "Pyre Garage should register cover");
        assert!(l.fire_sources.len() >= 5, "Pyre Garage should have its signature fire sources");
        assert!(!l.open_air, "Pyre Garage is an enclosed structure, not open-air");
    }
    // The Concrete Slab is a brutalist CQB maze: open-air external walkways over a rubble courtyard. It's a
    // proper C-grade playable arena with real verticality (stacked walkways + stairwells) and dense cover.
    {
        let p = PRESETS.iter().find(|p| p.name == "Concrete Slab").unwrap();
        let l = Level::generate(p.config(42));
        let sc = score(&l);
        assert!(sc.connected, "Concrete Slab must be fully connected");
        assert!(sc.total >= 60.0, "Concrete Slab should score at least C-grade (got {:.0})", sc.total);
        assert!(l.platforms.len() > 20, "Concrete Slab should have stacked-walkway verticality");
        assert!(l.cover_obstacles.len() >= 20, "Concrete Slab should register dense rubble/debris cover");
        assert!(l.open_air, "Concrete Slab's external walkways are under open sky");
    }
    // Redwood Canyon: a forested river gorge — a natural-terrain map with a wadeable river, log bridges,
    // giant redwoods, and boulder cover. Must be connected, standable, with real cover and a water body.
    {
        let p = PRESETS.iter().find(|p| p.name == "Redwood Canyon").unwrap();
        let l = Level::generate(p.config(42));
        let sc = score(&l);
        assert!(sc.connected, "Redwood Canyon must be fully connected");
        assert!(sc.total >= 50.0, "Redwood Canyon should be a playable map (got {:.0})", sc.total);
        assert!(!l.water.is_empty(), "Redwood Canyon should have its river");
        assert!(l.cover_obstacles.len() >= 20, "Redwood Canyon should have dense tree/boulder cover");
        assert!(l.platforms.len() > 10, "Redwood Canyon should have banks and log-bridge platforms");
        assert!(l.open_air, "Redwood Canyon is an open forest gorge");
    }
    // The Malecón Breakwater: a Havana seawall promenade — linear two-lane, ocean water, a seawall barrier,
    // the burned sedan cover, and pastel colonial buildings. Must be a solid playable arena with real cover.
    {
        let p = PRESETS.iter().find(|p| p.name == "Malecon Breakwater").unwrap();
        let l = Level::generate(p.config(42));
        let sc = score(&l);
        assert!(sc.connected, "Malecón must be fully connected");
        assert!(sc.total >= 60.0, "Malecón should be a strong playable map (got {:.0})", sc.total);
        assert!(!l.water.is_empty(), "Malecón should have its ocean");
        assert!(l.cover_obstacles.len() >= 20, "Malecón should have the seawall + car cover");
        assert!(!l.fire_sources.is_empty(), "Malecón's burned sedan should smoulder");
        // full wave populates (wave-spam guard)
        let e = crate::enemy::populate(&l, 13, 7);
        assert!(e.len() >= 10, "Malecón must populate a full wave, got {}", e.len());
    }
    // The Spire Foundry: a genuinely multi-LEVEL industrial hall — three stacked walkable floors (ground,
    // mezzanine, upper gantry) wrapped around an open atrium, joined by ramps and catwalks. This is the map
    // built to exercise real verticality, so it must expose platforms at several distinct heights.
    {
        let p = PRESETS.iter().find(|p| p.name == "Spire Foundry").unwrap();
        let l = Level::generate(p.config(42));
        let sc = score(&l);
        assert!(sc.connected, "Spire Foundry must be fully connected");
        assert!(l.platforms.len() > 15, "Spire Foundry should have many stacked decks/catwalks, got {}", l.platforms.len());
        // it must have platforms at genuinely different heights — at least three distinct levels
        let mut heights: Vec<f32> = l.platforms.iter().map(|p| (p.top * 4.0).round() / 4.0).collect();
        heights.sort_by(|a, b| a.partial_cmp(b).unwrap());
        heights.dedup();
        let distinct_high = heights.iter().filter(|&&h| h > 1.0).count();
        assert!(distinct_high >= 2, "Spire Foundry needs multiple raised levels (found high tiers: {distinct_high})");
        let max_h = l.platforms.iter().map(|p| p.top).fold(0.0f32, f32::max);
        assert!(max_h > 7.0, "Spire Foundry's upper gantry should be genuinely high up (got {:.1})", max_h);
        // full wave populates
        let e = crate::enemy::populate(&l, 13, 7);
        assert!(e.len() >= 10, "Spire Foundry must populate a full wave, got {}", e.len());
    }
}

}
