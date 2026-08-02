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
    pub wheels: usize,
    pub trail: usize,
    pub explosion: usize,
}

impl Default for Loadout {
    fn default() -> Loadout {
        Loadout { skin: 0, rig: 0, wheels: 0, trail: 0, explosion: 0 }
    }
}

impl Loadout {
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
        format!("{} {} {} {} {}", self.skin, self.wheels, self.trail, self.explosion, self.rig)
    }
    pub fn decode(s: &str) -> Loadout {
        let v: Vec<usize> = s.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        Loadout {
            skin: v.first().copied().unwrap_or(0).min(SKINS.len() - 1),
            wheels: v.get(1).copied().unwrap_or(0).min(WHEELS.len() - 1),
            trail: v.get(2).copied().unwrap_or(0).min(TRAILS.len() - 1),
            explosion: v.get(3).copied().unwrap_or(0).min(EXPLOSIONS.len() - 1),
            rig: v.get(4).copied().unwrap_or(0).min(RIGS.len() - 1),
        }
    }
}

/// The four rows the customizer screen shows.
pub fn slots() -> [(&'static str, usize); 5] {
    [
        ("SKIN", SKINS.len()),
        ("RIG", RIGS.len()),
        ("WHEELS", WHEELS.len()),
        ("BOOST TRAIL", TRAILS.len()),
        ("GOAL EXPLOSION", EXPLOSIONS.len()),
    ]
}
