//! custom — the garage.
//!
//! Rocket League's exotic and black-market items are mostly TEXTURES that
//! move. This engine can do better than a scrolling texture, because it
//! already ray-marches reflections: a skin here is a small shader program
//! that can read the view angle, the surface normal, the world position and
//! the clock, so a finish can respond to how you are looking at it rather
//! than just animate on a loop.
//!
//! Four independent slots, each purely cosmetic and each saved:
//!
//!   SKIN       how the body's surface behaves
//!   WHEELS     rim geometry
//!   TRAIL      what comes out of the back under boost
//!   EXPLOSION  what happens when you score
//!
//! Skins are numbered, and the number is handed to the shader per object, so
//! adding one is a match arm there plus a name here.

/// A finish. The comments describe what the shader does with each, since the
/// name alone will not tell you why one is interesting.
pub const SKINS: &[(&str, &str)] = &[
    ("FACTORY", "the body's own paint, untouched"),
    // reads the view angle: the hue rotates as the panel turns away from
    // you, so a curved flank shows a whole spectrum at once
    ("IRIDESCENT", "hue rotates with the viewing angle"),
    // bands of light travel nose to tail along the body's own axis
    ("ENERGY FLOW", "light runs the length of the car"),
    // the emissive rises and falls on a slow cycle, and the seams glow
    // brightest at the peak: a machine that is idling, not decorated
    ("BREATHING", "the panels breathe, seams glowing on the rise"),
    // albedo down, reflectivity up: the finish becomes the reflection, so
    // this one is entirely made of whatever the marcher sees
    ("LIQUID CHROME", "no colour of its own — only what it reflects"),
    // fine lines sweeping over the surface, brightest where they cross a
    // steep normal, so the car's shape is drawn out by the sweep
    ("SCANLINE", "a sweep that traces the bodywork"),
    // cell noise in world space, drifting: an oil-slick that moves with the
    // car rather than sliding over it
    ("PLASMA", "a drifting oil-slick that belongs to the body"),
    // a lattice of glowing traces with pulses running along them
    ("CIRCUIT", "traces with pulses running through them"),
    // heat: dark when cool, glowing along the leading edges the faster you
    // go — this one is driven by speed, not time
    ("FORGE", "leading edges glow with speed"),
    // interference bands that shift with both angle and time — the closest
    // thing here to a black-market finish
    ("MOIRE", "interference that shifts as you move"),
    // ---- the exotics: these read the GAME, not just the clock ----
    ("HUNTER", "glows on the side facing the ball"),
    ("AFTERBURNER", "lights from the tail forward as boost comes on"),
    ("GHOST", "translucent when you coast, solid when you work"),
    ("THERMAL", "false colour by the air the bodywork is pushing"),
    ("AURORA", "curtains that hang in the air as you climb"),
    ("CRYSTAL", "a lens rather than a mirror"),
    ("MOMENTUM", "brightens as your combo builds, and cools when you drop it"),
    ("IMPACT", "rings spread from wherever you last hit something"),
    ("CONSTELLATION", "stars fixed in the world; the car moves through them"),
    ("MIRAGE", "the paint refuses to hold still"),
];

/// PAINT. Every finish takes a colour, so a skin is a behaviour and the
/// paint is whose car it is. Stored as linear RGB, deliberately saturated —
/// the skins tend to wash a colour out, so a timid palette disappears.
pub const PAINTS: &[(&str, [f32; 3])] = &[
    ("FACTORY", [1.00, 1.00, 1.00]),
    ("CRIMSON", [0.72, 0.06, 0.10]),
    ("EMBER", [0.95, 0.35, 0.05]),
    ("SOLAR", [1.00, 0.78, 0.10]),
    ("LIME", [0.45, 0.90, 0.15]),
    ("JADE", [0.05, 0.55, 0.35]),
    ("CYAN", [0.10, 0.75, 0.95]),
    ("COBALT", [0.10, 0.25, 0.85]),
    ("VIOLET", [0.45, 0.15, 0.85]),
    ("MAGENTA", [0.95, 0.12, 0.60]),
    ("BONE", [0.92, 0.90, 0.84]),
    ("OBSIDIAN", [0.06, 0.06, 0.08]),
];

/// COLLECTIONS — a whole look in one pick.
///
/// Choosing five slots that agree with one another is most of the work of
/// looking good, and it is work most people would rather not do. Each of
/// these is a curated set where the parts share an idea: NOVA CORE is all
/// heat and pressure, WRAITH is all absence, PARADE is deliberately loud.
/// Picking one writes every slot; changing a slot afterwards just leaves
/// you on your own set, which is how it should be.
pub struct Collection {
    pub name: &'static str,
    pub blurb: &'static str,
    pub skin: usize,
    pub rig: usize,
    pub wheels: usize,
    pub trail: usize,
    pub explosion: usize,
    pub paint: usize,
}

pub const COLLECTIONS: &[Collection] = &[
    Collection { name: "FACTORY", blurb: "as it left the works", skin: 0, rig: 0, wheels: 0, trail: 0, explosion: 0, paint: 0 },
    Collection { name: "NOVA CORE", blurb: "heat and pressure: forge paint, unfolding plates, a supernova", skin: 8, rig: 3, wheels: 1, trail: 3, explosion: 1, paint: 2 },
    Collection { name: "WRAITH", blurb: "absence: ghost paint, a void trail, and everything falls inward", skin: 12, rig: 4, wheels: 6, trail: 4, explosion: 5, paint: 11 },
    Collection { name: "CIRCUITRY", blurb: "a machine that admits it: traces, a tremor, hexagons", skin: 7, rig: 2, wheels: 3, trail: 1, explosion: 2, paint: 6 },
    Collection { name: "PARADE", blurb: "loud on purpose: iridescent, prismatic, spectrum everywhere", skin: 1, rig: 1, wheels: 5, trail: 5, explosion: 4, paint: 9 },
    Collection { name: "COLD FRONT", blurb: "aurora over chrome, ion thrust, a clean shockwave", skin: 14, rig: 1, wheels: 2, trail: 1, explosion: 0, paint: 7 },
    Collection { name: "PREDATOR", blurb: "it watches the ball, and it breathes while it waits", skin: 10, rig: 1, wheels: 2, trail: 3, explosion: 3, paint: 1 },
    Collection { name: "MERIDIAN", blurb: "liquid chrome, hovering, nothing but reflections", skin: 4, rig: 4, wheels: 4, trail: 2, explosion: 0, paint: 10 },
    Collection { name: "SPECTRE", blurb: "crystal facets, a shiver on contact, prism shards", skin: 15, rig: 5, wheels: 6, trail: 5, explosion: 4, paint: 8 },
];

/// RIGS — how the car's parts MOVE. Skins change what a surface looks like;
/// a rig changes where it is. The body is split into prims already (a hull,
/// a canopy, fenders, a spoiler, four wheels), so each one can be displaced
/// independently by a function of its own position and the clock. That is
/// what makes a car feel mechanical rather than painted.
pub const RIGS: &[(&str, &str)] = &[
    ("RIGID", "the car as built"),
    ("BREATHING", "panels rise and settle, slightly out of step with each other"),
    ("FLUTTER", "a fast tremor, strongest at the extremities"),
    ("UNFOLD", "plates lift away from the body under boost, and close when you lift off"),
    ("HOVER", "the body floats free of its wheels and leans into the turn"),
    ("SHIVER", "parts jolt on impact and settle back"),
];

pub const WHEELS: &[(&str, u32, f32)] = &[
    // (name, spokes, dish) — dish is how deeply the rim is inset
    ("STOCK", 6, 0.26),
    ("TURBINE", 12, 0.18),
    ("BLADE", 5, 0.34),
    ("MESH", 18, 0.22),
    ("DISC", 0, 0.10),
    ("SPINDLE", 8, 0.40),
    ("HALO", 3, 0.30),
];

pub const TRAILS: &[(&str, &str)] = &[
    ("FLAME", "burning fuel: turbulent, orange, slow"),
    ("ION", "a collimated jet: fast, narrow, white to violet"),
    ("RIBBON", "a flat sheet that hangs in the air behind you"),
    ("EMBER", "sparks that fall and fade"),
    ("VOID", "dark particles that eat the light around them"),
    ("PRISM", "shards that split into their own spectrum"),
];

pub const EXPLOSIONS: &[(&str, &str)] = &[
    ("SHOCKWAVE", "a single expanding ring"),
    ("SUPERNOVA", "a flash, then a slow shell of debris"),
    ("HEXBURST", "a lattice that shatters outward"),
    ("INKFALL", "dark plumes that rise and drift down"),
    ("PRISMATIC", "a spectrum thrown in every direction"),
    ("SINGULARITY", "everything rushes IN, then out"),
];

/// The player's four choices.
#[derive(Clone, Copy)]
pub struct Loadout {
    pub skin: usize,
    pub rig: usize,
    pub paint: usize,
    pub wheels: usize,
    pub trail: usize,
    pub explosion: usize,
}

impl Default for Loadout {
    fn default() -> Loadout {
        Loadout { skin: 0, rig: 0, paint: 0, wheels: 0, trail: 0, explosion: 0 }
    }
}

impl Loadout {
    pub fn paint_name(&self) -> &'static str {
        PAINTS[self.paint.min(PAINTS.len() - 1)].0
    }
    pub fn paint_rgb(&self) -> [f32; 3] {
        PAINTS[self.paint.min(PAINTS.len() - 1)].1
    }
    /// Apply a whole collection.
    pub fn set_collection(&mut self, i: usize) {
        let c = &COLLECTIONS[i.min(COLLECTIONS.len() - 1)];
        self.skin = c.skin.min(SKINS.len() - 1);
        self.rig = c.rig.min(RIGS.len() - 1);
        self.wheels = c.wheels.min(WHEELS.len() - 1);
        self.trail = c.trail.min(TRAILS.len() - 1);
        self.explosion = c.explosion.min(EXPLOSIONS.len() - 1);
        self.paint = c.paint.min(PAINTS.len() - 1);
    }
    /// Which collection this matches, if any — so the screen can say.
    pub fn matches_collection(&self) -> Option<usize> {
        COLLECTIONS.iter().position(|c| {
            c.skin == self.skin && c.rig == self.rig && c.wheels == self.wheels
                && c.trail == self.trail && c.explosion == self.explosion && c.paint == self.paint
        })
    }

    pub fn rig_name(&self) -> &'static str {
        RIGS[self.rig.min(RIGS.len() - 1)].0
    }
    pub fn rig_blurb(&self) -> &'static str {
        RIGS[self.rig.min(RIGS.len() - 1)].1
    }
    pub fn skin_name(&self) -> &'static str {
        SKINS[self.skin.min(SKINS.len() - 1)].0
    }
    pub fn skin_blurb(&self) -> &'static str {
        SKINS[self.skin.min(SKINS.len() - 1)].1
    }
    pub fn wheel_name(&self) -> &'static str {
        WHEELS[self.wheels.min(WHEELS.len() - 1)].0
    }
    pub fn trail_name(&self) -> &'static str {
        TRAILS[self.trail.min(TRAILS.len() - 1)].0
    }
    pub fn explosion_name(&self) -> &'static str {
        EXPLOSIONS[self.explosion.min(EXPLOSIONS.len() - 1)].0
    }
    pub fn encode(&self) -> String {
        format!("{} {} {} {} {} {}", self.skin, self.wheels, self.trail, self.explosion, self.rig, self.paint)
    }
    pub fn decode(s: &str) -> Loadout {
        let v: Vec<usize> = s.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        Loadout {
            skin: v.first().copied().unwrap_or(0).min(SKINS.len() - 1),
            wheels: v.get(1).copied().unwrap_or(0).min(WHEELS.len() - 1),
            trail: v.get(2).copied().unwrap_or(0).min(TRAILS.len() - 1),
            explosion: v.get(3).copied().unwrap_or(0).min(EXPLOSIONS.len() - 1),
            rig: v.get(4).copied().unwrap_or(0).min(RIGS.len() - 1),
            paint: v.get(5).copied().unwrap_or(0).min(PAINTS.len() - 1),
        }
    }
}

/// The four rows the customizer screen shows.
pub fn slots() -> [(&'static str, usize); 7] {
    [
        ("COLLECTION", COLLECTIONS.len()),
        ("SKIN", SKINS.len()),
        ("PAINT", PAINTS.len()),
        ("RIG", RIGS.len()),
        ("WHEELS", WHEELS.len()),
        ("BOOST TRAIL", TRAILS.len()),
        ("GOAL EXPLOSION", EXPLOSIONS.len()),
    ]
}
