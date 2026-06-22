//! Procedural creature *blueprints* for design iteration (pure).
//!
//! A blueprint is just a bag of coloured boxes — the cheap, randomisable visual
//! sketch the candidate contact-sheet renders. It is deliberately NOT a
//! `Creature`/`Anatomy` (no physics, no capabilities): it's the form we iterate
//! on to calibrate the *axes of variation* before authoring a chosen design into
//! the real anatomy. `generate_candidates` is deterministic given its seed, so a
//! sheet reproduces exactly and "candidate #7 on seed 42" is a stable address.

/// What part of the creature a box represents — lets a viewer/animator move limbs
/// independently (wing-flap, leg-swing) and is the bridge toward authoring a chosen
/// sketch into real anatomy.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BoxRole {
    #[default]
    Body = 0,
    Neck = 1,
    Head = 2,
    Snout = 3,
    Eye = 4,
    Jaw = 5,
    Horn = 6,
    Leg = 7,
    Wing = 8,
    Tail = 9,
    Ridge = 10,
    /// An additional torso segment (multi-part body). Box 0 stays the canonical
    /// Body; these extend it fore and aft into a flexing spine.
    Segment = 11,
    /// A thin grass-like hair strand. Many per creature; the tallest sets how low
    /// a ceiling the creature can stand under before it must crawl.
    Hair = 12,
}

/// One coloured box, positioned by its centre offset from the creature origin.
#[derive(Clone, Copy, Debug)]
pub struct BlueprintBox {
    pub offset: [f32; 3],
    pub half: [f32; 3],
    pub color: [f32; 3],
    pub role: BoxRole,
}

/// A creature sketch. Box 0 is the torso by convention; the rest are appendages.
/// Dragon skin pattern family painted over the scales (T08-R27). Heritable like
/// the other dragon traits: derived deterministically from the egg seed (see
/// [`dragon_skin_genes`]), so it is reproduced exactly on re-hatch / save-load.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PatternKind {
    /// Uniform skin, no overlay.
    #[default]
    Plain,
    /// Transverse bands (alternating tone along the body).
    Banded,
    /// Irregular blotches.
    Mottled,
    /// Net-like ridged reticulation.
    Reticulated,
}

impl PatternKind {
    pub fn label(self) -> &'static str {
        match self {
            PatternKind::Plain => "plain",
            PatternKind::Banded => "banded",
            PatternKind::Mottled => "mottled",
            PatternKind::Reticulated => "reticulated",
        }
    }

    /// Map a unit value `[0,1)` to a kind (even quartiles).
    pub fn from_unit(u: f32) -> PatternKind {
        let u = if u.is_nan() { 0.0 } else { u.clamp(0.0, 0.9999) };
        match (u * 4.0) as u32 {
            0 => PatternKind::Plain,
            1 => PatternKind::Banded,
            2 => PatternKind::Mottled,
            _ => PatternKind::Reticulated,
        }
    }

    pub const ALL: [PatternKind; 4] =
        [PatternKind::Plain, PatternKind::Banded, PatternKind::Mottled, PatternKind::Reticulated];
}

// r193 (guide 4.1): splitmix64 lives in util::hash_util; wrappers keep
// the local names + behavior, byte-identical outputs.
fn skin_unit(seed: u64) -> f32 { crate::util::hash_util::mix64_unit(seed) }

/// Seed-derived dragon skin genes (T08-R27): `(scale_bumpiness, pattern,
/// pattern_color)`. `scale_bumpiness` in `[0,1]` is how raised the scales read
/// (drives the skin normal/displacement perturbation); `pattern` is the
/// [`PatternKind`]; `pattern_color` in `[0,1]³` is the accent tone the pattern
/// paints over the base skin colour.
///
/// Drawn from salted seeds so they're deterministic per dragon and round-trip
/// through the seed-based save/load **without perturbing the geometry RNG
/// stream** — the same approach the elemental-affinity gene uses.
pub fn dragon_skin_genes(seed: u64) -> (f32, PatternKind, [f32; 3]) {
    let bump = skin_unit(seed ^ 0x5CA1_E0DE_BBBB_0001);
    let pattern = PatternKind::from_unit(skin_unit(seed ^ 0x5CA1_E0DE_BBBB_0002));
    let accent = [
        skin_unit(seed ^ 0x5CA1_E0DE_BBBB_0003),
        skin_unit(seed ^ 0x5CA1_E0DE_BBBB_0004),
        skin_unit(seed ^ 0x5CA1_E0DE_BBBB_0005),
    ];
    (bump, pattern, accent)
}

#[derive(Clone, Debug, Default)]
pub struct CreatureBlueprint {
    pub boxes: Vec<BlueprintBox>,
    /// Human-readable archetype/build label (e.g. "dragon · eastern"), for the
    /// viewer readout. Empty if unspecified.
    pub arch: &'static str,
    /// Life stage ("juvenile" / "adult" / "ancient"), which scales overall size.
    /// Empty for creatures with no age model.
    pub age: &'static str,
    /// Joint rotational looseness in [0,1] — how freely limbs/neck/tail swing in
    /// the viewer. 0 ≈ locked/rigid, 1 ≈ loose/floppy. A per-creature animation
    /// trait, independent of body geometry.
    pub looseness: f32,
    /// Elemental breath affinity label ("fire", "ice", "fire+ice", "none", …) for
    /// the dragons that carry one. Empty for creatures with no elemental trait.
    pub element: &'static str,
    /// T08-R27 scale-bumpiness gene `[0,1]`: how raised the scales read, driving
    /// a normal/displacement perturbation on the rendered skin. 0 = smooth.
    pub scale_bumpiness: f32,
    /// T08-R27 skin pattern family (plain / banded / mottled / reticulated).
    pub pattern: PatternKind,
    /// T08-R27 pattern accent colour `[0,1]³` — the second tone painted over the
    /// base skin colour (the pattern's colour param).
    pub pattern_color: [f32; 3],
}

impl CreatureBlueprint {
    fn push(&mut self, offset: [f32; 3], half: [f32; 3], color: [f32; 3], role: BoxRole) {
        self.boxes.push(BlueprintBox { offset, half, color, role });
    }

    /// Full standing height: the vertical extent from the lowest box (feet) to the
    /// highest (hair tips). This is the height a low ceiling has to clear for the
    /// creature to stand — grass-like hair raises it, so a shaggier creature must
    /// drop to a crawl under a ceiling a bald one could walk under. Feed this to
    /// [`crate::sim::cognition::navigate::posture_for_clearance`].
    pub fn standing_height(&self) -> f32 {
        // r193 (guide 4.2): shared axis fold; empty-safe by contract.
        let (lo, hi) = crate::util::bbox::axis_bounds(
            self.boxes.iter().map(|b| (b.offset[1], b.half[1])),
        );
        hi - lo
    }

    /// The pivot the lower jaw swings about: the rear edge of the rear-most
    /// Jaw-role box, at its centre height. An animator rotates the Jaw-role boxes
    /// about this point (about the lateral z-axis) by an angle proportional to
    /// the gape (see `crate::sim::cognition::behavior::gape_for_drive`) so the
    /// mandible drops open and reveals the teeth. `None` if the creature has no
    /// jaw.
    pub fn jaw_hinge(&self) -> Option<[f32; 3]> {
        let rear = self
            .boxes
            .iter()
            .filter(|b| matches!(b.role, BoxRole::Jaw))
            .min_by(|a, b| {
                (a.offset[0] - a.half[0])
                    .partial_cmp(&(b.offset[0] - b.half[0]))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })?;
        Some([rear.offset[0] - rear.half[0], rear.offset[1], 0.0])
    }

    /// The eye point in body-local space: the average of the Eye boxes, falling
    /// back to the head, then the torso. This is the spot a perception sim places
    /// in the world — so the head/neck genetics that move the head (a rearing or
    /// S-curved neck lifts it) directly set the creature's vantage and sightline.
    pub fn eye_point(&self) -> [f32; 3] {
        let mut sum = [0.0f32; 3];
        let mut n = 0.0f32;
        for b in self.boxes.iter().filter(|b| b.role == BoxRole::Eye) {
            for k in 0..3 {
                sum[k] += b.offset[k];
            }
            n += 1.0;
        }
        if n > 0.0 {
            return [sum[0] / n, sum[1] / n, sum[2] / n];
        }
        let role = |r: BoxRole| self.boxes.iter().find(|b| b.role == r).map(|b| b.offset);
        role(BoxRole::Head)
            .or_else(|| self.boxes.first().map(|b| b.offset))
            .unwrap_or([0.0; 3])
    }

    /// Eye height above the lowest point of the body (the ground the feet rest
    /// on). A high-vantage gene (tall rearing neck) yields a larger value.
    pub fn eye_height(&self) -> f32 {
        let ground = self
            .boxes
            .iter()
            .map(|b| b.offset[1] - b.half[1])
            .fold(f32::INFINITY, f32::min);
        self.eye_point()[1] - ground
    }

    /// Unit gaze direction: from the eyes toward the snout (the head faces +x), so
    /// a tilted head — set by neck curvature — tilts the sightline. Falls back to
    /// straight ahead (+x) if there is no snout.
    pub fn gaze_forward(&self) -> [f32; 3] {
        let eye = self.eye_point();
        let snout = self
            .boxes
            .iter()
            .find(|b| b.role == BoxRole::Snout)
            .map(|b| b.offset);
        let d = match snout {
            Some(s) => [s[0] - eye[0], s[1] - eye[1], s[2] - eye[2]],
            None => [1.0, 0.0, 0.0],
        };
        // r193 (guide 4.3): shared norm3 (same +X degenerate fallback).
        crate::util::vecmath::norm3(d)
    }

    /// The resting sightline: a forward gaze tilted down just enough that the
    /// ground — a body-length ahead — sits at the *bottom edge* of a nominal
    /// field of view, so a creature standing at rest can just barely see the
    /// ground in front of it. Multiparametric: it folds in eye height (set by the
    /// head, neck curvature and rearing/low posture) and the body's forward
    /// extent. A low-slung creature already has the ground in view and looks
    /// straight ahead; a tall rearing one must tip its gaze down to keep it.
    pub fn resting_gaze(&self) -> [f32; 3] {
        let h = self.eye_height().max(0.01);
        // r193 (guide 4.2): shared axis fold.
        let (min_x, max_x) = crate::util::bbox::axis_bounds(
            self.boxes.iter().map(|b| (b.offset[0], b.half[0])),
        );
        let reach = (max_x - min_x).max(h); // a body-length ahead; never straight down
        let ground_angle = h.atan2(reach); // downward angle from the eye to that spot
        let half_fov = 0.52_f32; // ~30° nominal half vertical FOV
        let pitch = (ground_angle - half_fov).max(0.0);
        [pitch.cos(), -pitch.sin(), 0.0]
    }
}

/// Tiny deterministic xorshift PRNG (no external crate, pure).
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 11) as f32 / (1u64 << 53) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
    fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next_u64() as usize) % xs.len()]
    }
}

/// Coarse body plan — the strongest axis of variation. Public so a UI (egui) and
/// the egg/inventory system can name a chosen body plan; see [`EggSpec`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arche {
    Biped,
    Quadruped,
    Critter,
    Serpent,
    Heavy,
    /// Small bird — round body, beak, two legs, little folded wings (owl/chicken).
    Bird,
    /// Canid — low body, muzzle, pointed ears, four legs, bushy tail (fox/wolf).
    Canid,
    /// Lauscher (Listener) — a sentinel design plan built around hearing: a
    /// compact alert body, small eyes, and a pair of oversized dish ears. Like
    /// Dragon/Dinosaur it's request-only, so the general fauna sheet's specimen
    /// addresses stay stable.
    Lauscher,
    /// Long four-legged winged serpent — the dragon plan. Only produced on
    /// request (never in the general 5-archetype sheet, so existing specimen
    /// addresses stay stable).
    Dragon,
    /// Theropod (biped, big head, tiny arms, heavy tail) or sauropod (long neck,
    /// four pillar legs, long tail). Request-only, like Dragon.
    Dinosaur,
}

const BODY_PALETTE: [[f32; 3]; 6] = [
    [0.45, 0.40, 0.36],
    [0.36, 0.42, 0.34],
    [0.30, 0.34, 0.42],
    [0.50, 0.34, 0.30],
    [0.40, 0.40, 0.46],
    [0.52, 0.47, 0.34],
];

fn tint(c: [f32; 3], k: f32) -> [f32; 3] {
    [(c[0] * k).min(1.0), (c[1] * k).min(1.0), (c[2] * k).min(1.0)]
}

/// How strongly the heritable genome coat colour pulls the curated palette base.
/// The palette keeps creatures looking natural; the genome adds individual,
/// *heritable* variation (breeding shifts offspring toward their parents). Tune
/// this one knob up for more genetic colour spread, down for a tighter palette.
const GENE_COLOR_WEIGHT: f32 = 0.35;

/// Blend a palette base toward the genome's expressed coat colour. Pure; the
/// genome is derived from the seed without touching the geometry RNG, so this
/// only shifts colour, never shape.
fn genetic_body_color(base: [f32; 3], genome: &crate::genetics::Genome, seed: u64, env: u8) -> [f32; 3] {
    let g = genome.express(seed, env).color;
    let w = GENE_COLOR_WEIGHT;
    [
        (base[0] * (1.0 - w) + g[0] * w).clamp(0.0, 1.0),
        (base[1] * (1.0 - w) + g[1] * w).clamp(0.0, 1.0),
        (base[2] * (1.0 - w) + g[2] * w).clamp(0.0, 1.0),
    ]
}

fn one(seed: u64) -> CreatureBlueprint {
    one_with(seed, None)
}

/// Build one blueprint. If `forced` is set, use that archetype; otherwise pick
/// from the general five (Dragon is never picked here, only forced).
fn one_with(seed: u64, forced: Option<Arche>) -> CreatureBlueprint {
    let mut r = Rng::new(seed);
    let mut bp = CreatureBlueprint::default();
    let arche = match forced {
        Some(a) => a,
        None => *r.pick(&[Arche::Biped, Arche::Quadruped, Arche::Critter, Arche::Serpent, Arche::Heavy, Arche::Bird, Arche::Canid]),
    };
    let is_dragon = matches!(arche, Arche::Dragon);
    let is_dino = matches!(arche, Arche::Dinosaur);
    let is_bird = matches!(arche, Arche::Bird);
    let is_canid = matches!(arche, Arche::Canid);
    let is_lauscher = matches!(arche, Arche::Lauscher);
    let palette = *r.pick(&BODY_PALETTE);
    // Heritable coat colour: blend the curated palette base toward this seed's
    // genome. The r.pick above is kept so the geometry RNG stream is unchanged —
    // only colour shifts, and it's now inheritable through breeding.
    let body = genetic_body_color(palette, &crate::genetics::Genome::from_seed(seed), seed, 0);
    let accent = tint(body, r.range(1.1, 1.4));
    let dark = tint(body, 0.5);
    // A dragon is sometimes the long, lean, wingless Eastern build (the
    // serpentine look) instead of the broad winged Western one — and it carries
    // a posture: rearing (tall, near-vertical neck) or low (head out front).
    let serpentine = is_dragon && r.chance(0.45);
    let rear = is_dragon && r.chance(0.5);
    // A dinosaur is a long-necked sauropod or a big-headed biped theropod.
    let sauropod = is_dino && r.chance(0.5);
    // Lauscher mouth genome: a hinge plan (0 = underbite, 1 = wide gape, 2 =
    // split mandible) and a resting gape amount. Rolled here so the hinge can
    // name the archetype below; only consumed for a Lauscher, so other body
    // plans keep their RNG streams (and specimen seeds) unchanged.
    let lauscher_hinge: u8 = if is_lauscher { *r.pick(&[0u8, 1, 2]) } else { 0 };
    let lauscher_gape: f32 = if is_lauscher { r.range(0.15, 1.0) } else { 0.0 };
    bp.arch = match arche {
        Arche::Biped => "biped",
        Arche::Quadruped => "quadruped",
        Arche::Critter => "critter",
        Arche::Serpent => "serpent",
        Arche::Heavy => "heavy",
        Arche::Bird => "bird",
        Arche::Canid => "canid",
        Arche::Lauscher => match lauscher_hinge {
            0 => "lauscher · underbite",
            1 => "lauscher · wide-maw",
            _ => "lauscher · split-jaw",
        },
        Arche::Dragon => match (serpentine, rear) {
            (false, true) => "dragon · western · rearing",
            (false, false) => "dragon · western · low",
            (true, true) => "dragon · eastern · rearing",
            (true, false) => "dragon · eastern · low",
        },
        Arche::Dinosaur => if sauropod { "dinosaur · sauropod" } else { "dinosaur · theropod" },
    };

    // Torso dimensions by archetype.
    let (bx, by, bz) = match arche {
        Arche::Biped => (r.range(0.7, 1.1), r.range(0.8, 1.2), r.range(0.6, 0.9)),
        Arche::Quadruped => (r.range(1.4, 2.2), r.range(0.5, 0.8), r.range(0.6, 1.0)),
        Arche::Critter => (r.range(0.4, 0.7), r.range(0.35, 0.55), r.range(0.4, 0.7)),
        Arche::Serpent => (r.range(1.8, 3.0), r.range(0.35, 0.6), r.range(0.4, 0.7)),
        Arche::Heavy => (r.range(1.6, 2.4), r.range(0.9, 1.4), r.range(1.0, 1.5)),
        // Small round bird body; low lean canid body.
        Arche::Bird => (r.range(0.4, 0.6), r.range(0.5, 0.72), r.range(0.42, 0.58)),
        Arche::Canid => (r.range(1.0, 1.5), r.range(0.4, 0.58), r.range(0.45, 0.62)),
        // Compact, upright-ish alert body — the ears are the feature, not the bulk.
        // Around human scale — thin and tall: a narrow, slightly built upright
        // frame, the height coming from proportion rather than bulk.
        Arche::Lauscher => (r.range(0.24, 0.36), r.range(0.6, 0.85), r.range(0.3, 0.42)),
        // Leaner and not too long — girth held down so the body reads as a
        // lithe lizard, with length coming from neck + tail, not a fat trunk.
        Arche::Dragon => (r.range(1.1, 1.55), r.range(0.46, 0.64), r.range(0.55, 0.8)),
        // Theropod: medium horizontal body. Sauropod: big and bulky.
        Arche::Dinosaur => if sauropod {
            (r.range(1.8, 2.6), r.range(0.9, 1.3), r.range(1.0, 1.4))
        } else {
            (r.range(1.3, 1.9), r.range(0.6, 0.85), r.range(0.65, 0.9))
        },
    };
    // Eastern build: stretch the torso longer and leaner.
    let (bx, by, bz) = if serpentine { (bx * 1.35, by * 0.82, bz * 0.85) } else { (bx, by, bz) };
    bp.push([0.0, 0.0, 0.0], [bx, by, bz], body, BoxRole::Body);

    // Multi-part lizard torso: dragons get a raised chest in front and a hip
    // block behind, so the body reads as a flexing segmented spine rather than
    // one slab. Kept compact so the trunk doesn't dominate the silhouette.
    let (mut body_front, mut body_back) = (bx, -bx);
    if is_dragon {
        let chest = [bx * 0.34, by * 1.0, bz * 1.0];
        bp.push([bx + chest[0], by * 0.14, 0.0], chest, tint(body, 1.0), BoxRole::Segment);
        body_front = bx + chest[0] * 2.0;
        let hip = [bx * 0.4, by * 0.9, bz * 0.92];
        bp.push([-bx - hip[0], -by * 0.05, 0.0], hip, tint(body, 0.95), BoxRole::Segment);
        body_back = -bx - hip[0] * 2.0;
    }

    // Neck: a chain rising forward toward the head. Dragons get a longer neck
    // (so the body no longer dominates), and posture sets its angle — a rearing
    // dragon's neck climbs steeply to hold the head high, a low one reaches out
    // ahead like a lizard.
    let neck_segs = match arche {
        Arche::Biped => 2,
        Arche::Quadruped => if r.chance(0.5) { 2 } else { 1 },
        Arche::Critter => 1,
        Arche::Serpent => 3,
        Arche::Heavy => 1,
        Arche::Bird => 1,
        Arche::Canid => 1,
        Arche::Lauscher => 1,
        Arche::Dragon => if rear { 5 + (r.chance(0.5) as usize) } else { 4 + (r.chance(0.5) as usize) },
        // Sauropod: a very long neck. Theropod: short, head carried forward.
        Arche::Dinosaur => if sauropod { 6 + 2 * (r.chance(0.5) as usize) } else { 2 },
    };
    let neck_segs = if serpentine { neck_segs + 2 } else { neck_segs };
    let neck_r = by * r.range(0.24, 0.36);
    // Dragons get a chunkier neck (the step math still uses neck_r, so length is
    // unchanged — only the segment girth grows).
    let neck_box = if is_dragon { by * r.range(0.44, 0.6) } else { neck_r };
    let (step_x, step_y) = if is_dragon {
        if rear {
            (neck_r * r.range(0.7, 1.1), neck_r * r.range(1.7, 2.4))
        } else {
            (neck_r * r.range(1.7, 2.3), neck_r * r.range(0.4, 0.8))
        }
    } else if is_dino {
        if sauropod {
            (neck_r * r.range(0.9, 1.4), neck_r * r.range(1.2, 1.9)) // long and rising
        } else {
            (neck_r * r.range(1.8, 2.4), neck_r * r.range(-0.1, 0.3)) // forward, near-level
        }
    } else if is_lauscher {
        // Upright: the neck climbs nearly straight up so the head sits ON TOP
        // (humanoid), not carried out front (animal).
        (neck_r * 0.12, neck_r * r.range(1.1, 1.6))
    } else {
        (neck_r * r.range(1.2, 1.8), neck_r * r.range(0.5, 1.3))
    };
    let mut nx = if is_dragon { body_front } else if is_lauscher { 0.0 } else { bx * 0.7 };
    let mut ny = if is_dragon && rear { by * r.range(0.7, 1.0) } else if is_lauscher { by * r.range(0.85, 1.0) } else { by * r.range(0.45, 0.85) };
    // Neck curvature: dragons sometimes bend the neck — a single forward arc or a
    // sinuous double (S) curve — instead of a straight ramp. Non-dragons stay
    // straight (gated, so the fauna sheet's RNG is undisturbed).
    let (neck_wave, neck_amp) = if is_dragon {
        (*r.pick(&[0.0f32, 0.0, 1.0, 2.0]), neck_box * r.range(0.8, 2.0))
    } else {
        (0.0, 0.0)
    };
    let mut tip = [nx, ny];
    for i in 0..neck_segs {
        nx += step_x;
        ny += step_y;
        let t = (i as f32 + 1.0) / neck_segs as f32;
        let wy = neck_amp * (t * std::f32::consts::PI * neck_wave).sin();
        tip = [nx, ny + wy];
        bp.push([tip[0], tip[1], 0.0], [neck_box, neck_box, neck_box], tint(body, 0.95), BoxRole::Neck);
    }

    // Head at the neck tip, with a smaller snout forward.
    let hh = by * if is_dino {
        if sauropod { r.range(0.3, 0.45) } else { r.range(0.8, 1.1) }
    } else {
        r.range(0.5, 0.8)
    };
    let hx = tip[0] + if is_lauscher { hh * 0.15 } else { hh * 0.8 };
    let hy = tip[1] + step_y * 0.3;
    bp.push([hx, hy, 0.0], [hh, hh, hh * r.range(0.85, 1.05)], tint(body, 1.05), BoxRole::Head);
    if is_bird {
        // A short pointed beak, warm-tinted.
        bp.push([hx + hh * 1.05, hy - hh * 0.05, 0.0], [hh * 0.45, hh * 0.28, hh * 0.34], [0.86, 0.62, 0.22], BoxRole::Snout);
    } else if is_canid {
        // A longer muzzle.
        bp.push([hx + hh * 1.05, hy - hh * 0.3, 0.0], [hh * 0.8, hh * 0.42, hh * 0.5], tint(body, 1.0), BoxRole::Snout);
    } else {
        bp.push([hx + hh * 0.9, hy - hh * 0.2, 0.0], [hh * 0.55, hh * 0.5, hh * 0.7], tint(body, 1.0), BoxRole::Snout);
    }

    // Eyes. The Lauscher leans on its ears, so its eyes are smaller.
    let eye = hh * if is_lauscher { 0.17 } else { 0.26 };
    let ez = hh * 0.6;
    bp.push([hx + hh * 0.4, hy + hh * 0.35, ez], [eye, eye, eye], [0.06, 0.06, 0.08], BoxRole::Eye);
    bp.push([hx + hh * 0.4, hy + hh * 0.35, -ez], [eye, eye, eye], [0.06, 0.06, 0.08], BoxRole::Eye);

    // Jaw under the snout (often; always on a dragon's maw or a theropod's).
    if is_lauscher {
        // A hinged lower jaw tucked under the head, with teeth standing UP from
        // it INTO the mouth cavity — so they sit behind the face line (no buck
        // teeth) and are revealed when the jaw swings open. `gape` sets the
        // resting openness.
        let jw = hh * 0.7;
        let jh = hh * 0.26;
        let jaw_x = hx + hh * 0.5;                          // under the head, behind the snout front
        let jaw_y = hy - hh * (0.55 + lauscher_gape * 0.45); // gape drops it open
        let bone = [0.86, 0.83, 0.72];
        match lauscher_hinge {
            2 => {
                // Split mandible — two side jaws instead of one.
                for &s in &[1.0f32, -1.0] {
                    bp.push([jaw_x, jaw_y, s * hh * 0.42], [hh * 0.5, jh, hh * 0.28], [0.78, 0.73, 0.6], BoxRole::Jaw);
                }
            }
            _ => {
                bp.push([jaw_x, jaw_y, 0.0], [hh * 0.5, jh, jw], [0.78, 0.73, 0.6], BoxRole::Jaw);
            }
        }
        // Diverse teeth: a row standing up from the jaw top, inside the maw,
        // alternating fangs and nubs. Tooth x stays at the jaw (behind the snout
        // front at hx + hh*0.9), so nothing protrudes from the closed face.
        let teeth = 4 + (r.range(0.0, 6.99)) as usize; // 4..10
        let top = jaw_y + jh;
        for t in 0..teeth {
            let f = if teeth > 1 { t as f32 / (teeth as f32 - 1.0) } else { 0.5 };
            let tz = (f - 0.5) * 2.0 * jw * 0.85;
            let big = t % 2 == 0; // alternating sizes read as diverse dentition
            let tlen = hh * if big { 0.2 } else { 0.12 };
            let tw = hh * if big { 0.07 } else { 0.05 };
            bp.push([jaw_x - hh * 0.05, top + tlen, tz], [tw, tlen, tw], bone, BoxRole::Jaw);
        }
        // Upper fangs descending from the palate. These ride the head (Snout
        // role) — NOT the hinged lower jaw — so when the mandible swings open the
        // gape reveals a SECOND row of teeth above the first. Tucked behind the
        // snout front (no overbite past the face). A hunting gape (see
        // behavior::gape_for_drive) exposes them.
        let upper = 3 + (r.range(0.0, 4.99)) as usize; // 3..7
        let palate = hy - hh * 0.45;
        for t in 0..upper {
            let f = if upper > 1 { t as f32 / (upper as f32 - 1.0) } else { 0.5 };
            let tz = (f - 0.5) * 2.0 * jw * 0.8;
            let big = t % 2 == 1;
            let tlen = hh * if big { 0.17 } else { 0.1 };
            let tw = hh * if big { 0.06 } else { 0.045 };
            bp.push([jaw_x - hh * 0.05, palate - tlen, tz], [tw, tlen, tw], bone, BoxRole::Snout);
        }
    } else if r.chance(0.7) || is_dragon || (is_dino && !sauropod) {
        bp.push([hx + hh * 0.8, hy - hh * 0.55, 0.0], [hh * 0.55, hh * 0.3, hh * 0.65], [0.80, 0.75, 0.62], BoxRole::Jaw);
    }

    // Ears / horns / crest. Canids get tall pointed ears; owls sometimes get
    // tufts; otherwise the usual swept horn pair (always on a dragon).
    if is_lauscher {
        // Oversized dish ears — tall (y), broad across (z), thin front-to-back
        // (x): the signature of a Listener. They read clearly from any angle.
        let ear_h = hh * r.range(1.1, 1.6);
        let ear_w = hh * r.range(0.45, 0.7);
        let ear_t = hh * 0.12;
        for &hz in &[ez * 1.1, -ez * 1.1] {
            bp.push([hx - hh * 0.1, hy + hh * 1.05, hz], [ear_t, ear_h, ear_w], tint(body, 0.92), BoxRole::Horn);
        }
    } else if is_canid {
        let ear = hh * 0.2;
        for &hz in &[ez, -ez] {
            bp.push([hx - hh * 0.1, hy + hh * 0.95, hz], [ear, hh * 0.55, ear], tint(body, 0.9), BoxRole::Horn);
        }
    } else if is_bird && r.chance(0.5) {
        let tuft = hh * 0.16;
        for &hz in &[ez * 0.7, -ez * 0.7] {
            bp.push([hx - hh * 0.05, hy + hh * 0.95, hz], [tuft, hh * 0.45, tuft], tint(body, 0.85), BoxRole::Horn);
        }
    } else if r.chance(0.45) || is_dragon {
        let horn = hh * 0.22;
        for &hz in &[ez, -ez] {
            bp.push([hx - hh * 0.2, hy + hh * 0.9, hz], [horn, hh * 0.7, horn], accent, BoxRole::Horn);
        }
    }

    if is_lauscher {
        // Curly body hair, mostly down the BACK and flanks (not the head). Each
        // strand is a short chain of small segments tracing an arc, so it reads
        // as a curl rather than a straight blade; curl direction/tightness are
        // per-strand. The whole pelt's tallest point still feeds standing_height
        // (so a shaggy back can brush a low ceiling and tip it into a crawl).
        let strands = 10 + (r.range(0.0, 12.99)) as usize; // 10..22
        let hair_col = tint(body, 0.6);
        let tw = bx * 0.10;
        for _ in 0..strands {
            // base on the back surface (negative x), scattered along spine + flank
            let mut px = -bx * r.range(0.6, 1.0);
            let mut py = by * r.range(-0.85, 0.95);
            let mut pz = bz * r.range(-0.95, 0.95);
            let segs = 3 + (r.range(0.0, 1.99)) as usize; // 3..4 segments per curl
            let curl = r.range(0.6, 1.3) * if r.chance(0.5) { 1.0 } else { -1.0 };
            let rad = bx * r.range(0.3, 0.55);
            let phase = r.range(0.0, std::f32::consts::TAU);
            for s in 0..segs {
                let a = phase + curl * s as f32;
                px -= bx * 0.12; // grow outward, away from the back
                py += rad * 0.4 * a.sin();
                pz += rad * 0.25 * a.cos();
                bp.push([px, py, pz], [tw, tw, tw], hair_col, BoxRole::Hair);
            }
        }
    }

    // Legs: 2 for a biped, 0 for a serpent, 4 otherwise (dragons included).
    let leg_count = match arche {
        Arche::Biped => 2,
        Arche::Serpent => 0,
        Arche::Bird => 2,
        Arche::Canid => 4,
        Arche::Dinosaur => if sauropod { 4 } else { 2 },
        Arche::Lauscher => 2,
        _ => 4,
    };
    if leg_count > 0 && is_dragon {
        // Sprawled lizard stance: each of the four limbs is a thigh jutting out
        // to the side, then a shin dropping down at the outer end — a wide, low,
        // crocodilian posture. The lower leg also arcs along the body (a genetic
        // amount): the front pair reaches forward, the hind pair pushes back, for
        // a more dynamic, less table-legged stance.
        let reach = bz * r.range(0.55, 0.85);
        let thigh_h = by * 0.16;
        let shin_len = by * r.range(0.5, 0.85);
        let hip_y = -by * 0.3;
        let arc = bx * r.range(0.0, 0.5);
        for &(cx, dir) in &[(body_front * 0.5, 1.0f32), (body_back * 0.55, -1.0)] {
            for &s in &[1.0f32, -1.0] {
                // Thigh: from the body side outward.
                bp.push([cx, hip_y, s * (bz + reach * 0.5)], [bx * 0.12, thigh_h, reach * 0.5], dark, BoxRole::Leg);
                // Shin + foot: down at the outer end, arced fore/aft.
                bp.push([cx + dir * arc, hip_y - shin_len, s * (bz + reach)], [bx * 0.13, shin_len, bz * 0.18], dark, BoxRole::Leg);
            }
        }
    } else if is_dino {
        if sauropod {
            // Four thick pillar legs.
            let leg_len = by * r.range(0.7, 1.0);
            let leg_half = [bx * 0.22, leg_len, bz * 0.26];
            let lr = bx * 0.6;
            let lz = bz * 0.7;
            for &lx in &[lr, -lr] {
                bp.push([lx, -(by + leg_len), lz], leg_half, dark, BoxRole::Leg);
                bp.push([lx, -(by + leg_len), -lz], leg_half, dark, BoxRole::Leg);
            }
        } else {
            // Theropod: two big hind legs plus two tiny forelimbs.
            let leg_len = by * r.range(0.8, 1.2);
            let leg_half = [bx * 0.18, leg_len, bz * 0.28];
            let lz = bz * 0.7;
            let hind_x = -bx * 0.2;
            bp.push([hind_x, -(by + leg_len), lz], leg_half, dark, BoxRole::Leg);
            bp.push([hind_x, -(by + leg_len), -lz], leg_half, dark, BoxRole::Leg);
            let arm = [bx * 0.08, by * 0.28, bz * 0.1];
            let ax = bx * 0.7;
            bp.push([ax, -by * 0.1, bz * 0.5], arm, dark, BoxRole::Leg);
            bp.push([ax, -by * 0.1, -bz * 0.5], arm, dark, BoxRole::Leg);
        }
    } else if leg_count > 0 {
        let leg_len = by * r.range(0.4, 0.7);
        let lt = if matches!(arche, Arche::Heavy) { 1.5 } else { 1.0 };
        let leg_half = [bx * r.range(0.16, 0.24) * lt, leg_len, bz * r.range(0.20, 0.30) * lt];
        let lr = bx * r.range(0.45, 0.7);
        let lz = bz * r.range(0.6, 0.95);
        let foot_y = -(by + leg_len);
        let two = [0.0f32];
        let four = [lr, -lr];
        let xs: &[f32] = if leg_count == 2 { &two } else { &four };
        for &lx in xs {
            bp.push([lx, foot_y, lz], leg_half, dark, BoxRole::Leg);
            bp.push([lx, foot_y, -lz], leg_half, dark, BoxRole::Leg);
        }
    }

    if is_lauscher {
        // Arms hang from the shoulders at the SIDES (humanoid), each ARTICULATED:
        // an upper section dropping down, a forearm bent slightly forward at the
        // elbow, and a small clawed hand. Extra pairs stack down the flanks for
        // the unsettling many-armed look. Static (Segment role) so they read as
        // jointed without per-box limb swing. 2..3 pairs => 4..6 arms.
        let pairs = 2 + (r.range(0.0, 1.99)) as usize;
        let aw = bx * 0.16;
        for p in 0..pairs {
            let sh_y = by * (0.7 - p as f32 * 0.5); // shoulder height, descending the flank
            let up_len = by * r.range(0.30, 0.45);
            let fore_len = by * r.range(0.26, 0.40);
            for &s in &[1.0f32, -1.0] {
                let sz = s * (bz + aw); // out at the side, just past the torso
                // upper arm — drops straight down from the shoulder
                bp.push([0.0, sh_y - up_len, sz], [aw, up_len, aw], dark, BoxRole::Segment);
                // forearm — continues down, bent slightly forward at the elbow
                let ey = sh_y - up_len * 2.0;
                bp.push([aw * 1.3, ey - fore_len, sz], [aw * 0.85, fore_len, aw * 0.85], dark, BoxRole::Segment);
                // clawed hand at the wrist — a small palm plus three downturned claws
                let hgx = aw * 1.3;
                let hgy = ey - fore_len * 2.0;
                bp.push([hgx, hgy, sz], [aw, aw * 0.8, aw * 1.2], tint(body, 0.8), BoxRole::Segment);
                for c in 0..3 {
                    let cz = sz + (c as f32 - 1.0) * aw * 0.7;
                    bp.push([hgx + aw * 0.2, hgy - aw * 0.9, cz], [aw * 0.28, aw * 0.45, aw * 0.18], [0.90, 0.88, 0.80], BoxRole::Segment);
                }
            }
        }
    }

    // Wings: span scales with body size (no more giant wings on tiny critters).
    let wing_p = match arche {
        Arche::Biped => 0.7,
        Arche::Quadruped => 0.25,
        Arche::Critter => 0.05,
        Arche::Serpent => 0.15,
        Arche::Heavy => 0.1,
        Arche::Dragon => 1.0,
        Arche::Dinosaur => 0.0,
        Arche::Bird => 1.0,
        Arche::Canid => 0.0,
        Arche::Lauscher => 0.0,
    };
    if r.chance(wing_p) && !serpentine {
        if is_bird {
            // Small folded wings down the sides.
            let whalf = [bx * 0.7, by * 0.5, bz * 0.32];
            let wy = by * 0.1;
            bp.push([0.0, wy, bz * 0.9 + whalf[2]], whalf, tint(body, 0.9), BoxRole::Wing);
            bp.push([0.0, wy, -(bz * 0.9 + whalf[2])], whalf, tint(body, 0.9), BoxRole::Wing);
        } else if is_dragon {
            // A swept, arced membrane: panels step outward (z), sweep back (x),
            // arc up across the span, and taper toward the tip — approximating a
            // wing's curve with a fan of boxes instead of one flat slab.
            let wy = by * r.range(0.35, 0.7);
            // Span varies widely — small-winged dragons through broad, sail-winged
            // ones at the top end.
            let reach = (bx + bz) * r.range(0.85, 2.1);
            let chord0 = bx * r.range(0.65, 1.45);
            let arc = by * r.range(0.55, 1.4);
            let segs = 4usize;
            const PI: f32 = std::f32::consts::PI;
            for &s in &[1.0f32, -1.0] {
                for i in 0..segs {
                    let t0 = i as f32 / segs as f32;
                    let t1 = (i + 1) as f32 / segs as f32;
                    let tm = (t0 + t1) * 0.5;
                    let zc = bz + reach * tm;
                    let zhalf = (reach * (t1 - t0)) * 0.5;
                    let xc = -reach * 0.35 * tm; // trailing sweep
                    let yc = wy + arc * (tm * PI).sin(); // arc up across the span
                    let chord = chord0 * (1.0 - 0.6 * tm); // taper to the tip
                    bp.push([xc, yc, s * zc], [chord, by * 0.08, zhalf], [0.62, 0.66, 0.72], BoxRole::Wing);
                }
            }
        } else {
            let wing_len = bx * r.range(0.4, 0.7);
            let span = (bx + bz) * r.range(0.7, 1.2);
            let whalf = [wing_len, by * 0.12, span * 0.5];
            let wy = by * r.range(0.3, 0.7);
            bp.push([0.0, wy, bz + whalf[2]], whalf, [0.62, 0.66, 0.72], BoxRole::Wing);
            bp.push([0.0, wy, -(bz + whalf[2])], whalf, [0.62, 0.66, 0.72], BoxRole::Wing);
        }
    }

    // Tail: long and many-segmented on a serpent or dragon, else 1-2 short ones.
    let (tail_segs, tail_p) = match arche {
        Arche::Serpent => (4, 1.0),
        // Dragon tails vary a lot in length: a stub of 4 up to a long 10-segment
        // whip (before the serpentine bonus).
        Arche::Dragon => (4 + (r.range(0.0, 6.99)) as usize, 1.0),
        Arche::Dinosaur => if sauropod { (7, 1.0) } else { (6, 1.0) },
        Arche::Canid => (3, 1.0),
        Arche::Bird => (2, 1.0),
        Arche::Lauscher => (0, 0.0), // humanoid — no tail
        _ => (if r.chance(0.5) { 2 } else { 1 }, 0.7),
    };
    let tail_segs = if serpentine { tail_segs + 4 } else { tail_segs };
    if r.chance(tail_p) {
        let mut tx = if is_dragon { body_back } else { -bx };
        let mut th = by * r.range(0.5, 0.8);
        // A genetic taper rate: low = a long even whip, high = a quick stubby cone.
        let taper = if is_dragon { r.range(0.72, 0.9) } else { 0.78 };
        for _ in 0..tail_segs {
            bp.push([tx - th, 0.0, 0.0], [th, th, th], accent, BoxRole::Tail);
            tx -= th * 2.0;
            th *= taper;
        }
    }

    // Back ridge (sometimes; always a spined back on a dragon). For dragons the
    // pattern is genetic: 1–3 rows, a unified crest of tall scutes or low stubs,
    // and side rows either mirrored symmetrically or staggered in an alternating
    // zigzag. Non-dragons keep the simple single centre row.
    if r.chance(0.4) || is_dragon {
        let n = if serpentine { 9 } else if is_dragon { 6 } else { 3 };
        if is_dragon {
            let rows = *r.pick(&[1usize, 1, 2, 2, 3]);
            let unified = r.chance(0.5);
            let symmetrical = r.chance(0.6);
            let rh = by * if unified { 0.5 } else { 0.22 };
            let rw = if unified { bx * 0.16 } else { bx * 0.09 }; // overlap when unified
            let zo = bz * 0.5;
            let row_z: &[f32] = match rows {
                1 => &[0.0],
                2 => &[1.0, -1.0],
                _ => &[0.0, 1.0, -1.0],
            };
            for (ri, &zs) in row_z.iter().enumerate() {
                // Alternating: stagger the side rows half a step along the spine.
                let stagger = if !symmetrical && ri > 0 { 0.5 } else { 0.0 };
                for i in 0..n {
                    let t = (i as f32 + stagger) / (n as f32 - 1.0);
                    let rx = (t - 0.5) * bx * 1.2;
                    bp.push([rx, by + by * 0.25, zs * zo], [rw, rh, bz * 0.14], accent, BoxRole::Ridge);
                }
            }
        } else {
            for i in 0..n {
                let t = i as f32 / (n as f32 - 1.0);
                let rx = (t - 0.5) * bx * 1.2;
                bp.push([rx, by + by * 0.25, 0.0], [bx * 0.12, by * 0.3, bz * 0.15], accent, BoxRole::Ridge);
            }
        }
    }

    // Age → size. A dragon's life stage scales its whole body uniformly. Most
    // are juveniles or adults — already bigger than a ~7 m tree, up to a few
    // trees. Ancients are rare and can be giants: the top of the range reaches
    // ~10 trees long. (Heritable genetics can later bias the stage/scale.)
    if is_dragon {
        let roll = r.range(0.0, 1.0);
        let (age, scale) = if roll < 0.50 {
            ("juvenile", r.range(0.85, 1.4))
        } else if roll < 0.85 {
            ("adult", r.range(1.4, 3.2))
        } else {
            ("ancient", r.range(3.2, 9.5))
        };
        bp.age = age;
        for b in &mut bp.boxes {
            for k in 0..3 {
                b.offset[k] *= scale;
                b.half[k] *= scale;
            }
        }
    }

    // Joint looseness — rolled last, so it perturbs no geometry. Skewed toward
    // the middle, with the loose and locked extremes rarer.
    bp.looseness = {
        let a = r.range(0.0, 1.0);
        let b = r.range(0.0, 1.0);
        (a + b) * 0.5
    };

    // Elemental breath affinity (dragons only). Drawn from a salted seed so it's
    // deterministic per dragon without perturbing the geometry RNG stream — most
    // carry a single element, a multi-element arsenal is the rare prize.
    if is_dragon {
        bp.element = crate::sim::elements::Affinity::from_seed(seed ^ 0x5F3D_C0DE_E1E3).label();
    }

    // Skin appearance genes (T08-R27): scale bumpiness + pattern + accent colour,
    // dragons only. Seed-derived like the elemental affinity above, so they're
    // deterministic per dragon and round-trip through save/load without
    // perturbing the geometry RNG stream.
    if is_dragon {
        let (bump, pattern, accent) = dragon_skin_genes(seed);
        bp.scale_bumpiness = bump;
        bp.pattern = pattern;
        bp.pattern_color = accent;
    }

    bp
}

/// Generate `n` deterministic candidate blueprints from a base seed.
pub fn generate_candidates(seed: u64, n: usize) -> Vec<CreatureBlueprint> {
    (0..n)
        .map(|i| one(seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)))
        .collect()
}

/// Generate `n` deterministic *dragon* blueprints — a focused sheet of the
/// dragon body plan (long winged four-legged serpents) for design iteration.
pub fn generate_dragons(seed: u64, n: usize) -> Vec<CreatureBlueprint> {
    (0..n)
        .map(|i| one_with(seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15), Some(Arche::Dragon)))
        .collect()
}

/// Generate `n` deterministic *dinosaur* blueprints — theropods and sauropods.
pub fn generate_dinosaurs(seed: u64, n: usize) -> Vec<CreatureBlueprint> {
    (0..n)
        .map(|i| one_with(seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15), Some(Arche::Dinosaur)))
        .collect()
}

/// Generate `n` deterministic *Lauscher* (Listener) blueprints — a focused sheet
/// of the big-eared sentinel plan for design iteration.
pub fn generate_lauschers(seed: u64, n: usize) -> Vec<CreatureBlueprint> {
    (0..n)
        .map(|i| one_with(seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15), Some(Arche::Lauscher)))
        .collect()
}

impl Arche {
    /// Every body plan a player can deliberately choose (e.g. from an egg in the
    /// inventory UI). Order is stable for menu display.
    pub const CHOOSABLE: &'static [Arche] = &[
        Arche::Biped,
        Arche::Quadruped,
        Arche::Critter,
        Arche::Serpent,
        Arche::Heavy,
        Arche::Bird,
        Arche::Canid,
        Arche::Lauscher,
        Arche::Dragon,
        Arche::Dinosaur,
    ];

    /// Stable identifier for UI display and serialization (what an egg stores).
    /// Distinct from `CreatureBlueprint::arch`, which is a descriptive label.
    pub fn label(self) -> &'static str {
        match self {
            Arche::Biped => "biped",
            Arche::Quadruped => "quadruped",
            Arche::Critter => "critter",
            Arche::Serpent => "serpent",
            Arche::Heavy => "heavy",
            Arche::Bird => "bird",
            Arche::Canid => "canid",
            Arche::Lauscher => "lauscher",
            Arche::Dragon => "dragon",
            Arche::Dinosaur => "dinosaur",
        }
    }

    /// Parse a label back to an archetype (for deserializing an egg's choice).
    pub fn from_label(s: &str) -> Option<Arche> {
        Arche::CHOOSABLE.iter().copied().find(|a| a.label() == s)
    }
}

/// What an egg will hatch into. This is the UI-agnostic entry point the egui
/// inventory drives: `chosen` for "I picked this body plan," `random` for a roll.
/// Hatching is deterministic in the seed, so the same egg always yields the same
/// creature — eggs are reproducible.
#[derive(Clone, Copy, Debug)]
pub struct EggSpec {
    /// The chosen body plan, or `None` for a random roll over the general fauna.
    pub choice: Option<Arche>,
    /// Chosen elemental affinity, or `None` to leave it to the seed (dragons roll
    /// a wild affinity; other body plans default to none unless chosen here).
    pub element: Option<crate::sim::elements::Affinity>,
    /// Seed that fixes every other gene (size, limbs, looseness, element, ...).
    pub seed: u64,
}

impl EggSpec {
    /// A random roll: the body plan is picked from the general fauna by `seed`.
    pub fn random(seed: u64) -> EggSpec {
        EggSpec { choice: None, element: None, seed }
    }

    /// A chosen body plan; all other genes still come from `seed`.
    pub fn chosen(arche: Arche, seed: u64) -> EggSpec {
        EggSpec { choice: Some(arche), element: None, seed }
    }

    /// Choose the body plan (builder form; `None` re-randomizes it).
    pub fn with_choice(mut self, choice: Option<Arche>) -> EggSpec {
        self.choice = choice;
        self
    }

    /// Choose the elemental affinity (builder form; `None` leaves it to the seed).
    pub fn with_element(mut self, element: Option<crate::sim::elements::Affinity>) -> EggSpec {
        self.element = element;
        self
    }

    /// Resolve the egg into a concrete creature blueprint. Chosen traits override
    /// the seed-rolled ones; everything unchosen still comes from `seed`, so the
    /// egg stays reproducible.
    pub fn hatch(self) -> CreatureBlueprint {
        let mut bp = one_with(self.seed, self.choice);
        // Element is a post-hoc label (it doesn't perturb geometry), so a chosen
        // affinity simply overrides whatever the seed rolled — for any body plan,
        // not just dragons.
        if let Some(aff) = self.element {
            bp.element = aff.label();
        }
        bp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragons_have_the_full_four_legged_serpent_plan() {
        for seed in 0..120u64 {
            let bp = &generate_dragons(seed, 1)[0];
            let count = |role: BoxRole| bp.boxes.iter().filter(|b| b.role == role).count();
            // Box 0 is the canonical core torso — elongated, but deliberately
            // shorter than the old 2:1 (a leaner lizard, not a fat trunk).
            assert_eq!(bp.boxes[0].role, BoxRole::Body);
            let b = bp.boxes[0].half;
            assert!(b[0] > b[1] * 1.5, "seed {seed}: a dragon torso is elongated, got {b:?}");
            // A multi-part torso (chest + hip), four sprawled limbs (thigh + shin
            // each = 8 leg boxes), a maw, horns, a neck chain, a long tail, a
            // spined back.
            assert_eq!(count(BoxRole::Segment), 2, "seed {seed}: chest + hip segments");
            assert_eq!(count(BoxRole::Leg), 8, "seed {seed}: four sprawled limbs (2 boxes each)");
            assert_eq!(count(BoxRole::Jaw), 1, "seed {seed}: a maw");
            assert_eq!(count(BoxRole::Horn), 2, "seed {seed}: a pair of horns");
            assert!(count(BoxRole::Neck) >= 3, "seed {seed}: a long neck");
            assert!(count(BoxRole::Tail) >= 4, "seed {seed}: a tail (length now varies)");
            assert!(count(BoxRole::Ridge) >= 5, "seed {seed}: a spined back");
            // Wings come as a pair (Western) or not at all (Eastern/serpentine).
            assert!(matches!(count(BoxRole::Wing), 0 | 8), "seed {seed}: a swept wing (8 panels) or none");
            assert_eq!(count(BoxRole::Body), 1, "seed {seed}: exactly one Body box");
            // Every box finite and non-degenerate.
            for x in &bp.boxes {
                for k in 0..3 {
                    assert!(x.half[k] > 0.0 && x.half[k].is_finite() && x.offset[k].is_finite(), "seed {seed}: degenerate box");
                }
            }
        }
    }

    #[test]
    fn resting_gaze_keeps_the_ground_just_in_view() {
        // A low, long creature already has the ground in its forward cone, so it
        // looks essentially straight ahead.
        let mut low = CreatureBlueprint::default();
        low.push([0.0, 0.0, 0.0], [3.0, 0.5, 1.0], [0.5, 0.5, 0.5], BoxRole::Body);
        low.push([3.0, 0.4, 0.2], [0.1, 0.1, 0.1], [0.0, 0.0, 0.0], BoxRole::Eye);
        low.push([3.0, 0.4, -0.2], [0.1, 0.1, 0.1], [0.0, 0.0, 0.0], BoxRole::Eye);
        let gl = low.resting_gaze();
        assert!(gl[0] > 0.0 && gl[1] <= 0.0, "forward, not upward: {gl:?}");

        // A tall, rearing creature must tilt its gaze down further to keep the
        // ground at the bottom of its view.
        let mut tall = CreatureBlueprint::default();
        tall.push([0.0, 0.0, 0.0], [1.0, 0.5, 1.0], [0.5, 0.5, 0.5], BoxRole::Body);
        tall.push([0.5, 6.0, 0.2], [0.1, 0.1, 0.1], [0.0, 0.0, 0.0], BoxRole::Eye);
        tall.push([0.5, 6.0, -0.2], [0.1, 0.1, 0.1], [0.0, 0.0, 0.0], BoxRole::Eye);
        let gt = tall.resting_gaze();
        assert!(gt[1] < 0.0 && gt[0] > 0.0, "forward and downward: {gt:?}");
        assert!(gt[1] < gl[1] - 0.1, "high eyes tilt down more than low: {gl:?} vs {gt:?}");
    }

    #[test]
    fn eye_geometry_bridges_to_vision() {
        use crate::sim::fixed::{Fixed, Vec3};
        use crate::sim::physics::{BlockPhysics, SceneWorld};
        use crate::sim::vision::line_of_sight;

        // Hand-built sanity: the eye point averages the eyes, height is measured
        // above the lowest box (the feet), and gaze points toward the snout.
        let mut bp = CreatureBlueprint::default();
        bp.push([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.5, 0.5, 0.5], BoxRole::Body); // bottom y = -1
        bp.push([2.0, 3.0, 0.3], [0.1, 0.1, 0.1], [0.0, 0.0, 0.0], BoxRole::Eye);
        bp.push([2.0, 3.0, -0.3], [0.1, 0.1, 0.1], [0.0, 0.0, 0.0], BoxRole::Eye);
        bp.push([3.0, 2.6, 0.0], [0.3, 0.3, 0.3], [0.0, 0.0, 0.0], BoxRole::Snout);
        assert!((bp.eye_point()[1] - 3.0).abs() < 1e-5, "eye point is the eyes' average");
        assert!((bp.eye_height() - 4.0).abs() < 1e-5, "height above the feet");
        let g = bp.gaze_forward();
        assert!(g[0] > 0.0 && g[1] < 0.0, "gaze leans forward and slightly down toward the snout");

        // Across generated dragons, the eye's vantage spans a wide range — the
        // head/neck genetics and the age→size gene move it. Scanning enough seeds
        // guarantees an ancient giant, so the spread is large.
        let (mut hi, mut lo) = (0.0f32, f32::MAX);
        for seed in 0..500u64 {
            let h = generate_dragons(seed, 1)[0].eye_height();
            hi = hi.max(h);
            lo = lo.min(h);
        }
        assert!(hi > lo + 6.0, "eye vantage varies widely: {lo:.1}..{hi:.1}");

        // And that vantage decides sight: the high-eyed dragon clears a wall that
        // blocks the low one — the body plan now feeds the vision sim.
        let (eh, el) = (hi.ceil() as i64, lo.floor() as i64);
        let mut world = SceneWorld::new();
        for y in 0..(el + 2) {
            world.set(4, y as i32, 0, BlockPhysics::solid(Fixed::ZERO));
        }
        let target = Vec3::new(Fixed::from_int(8), Fixed::from_int(el), Fixed::ZERO);
        let tall = Vec3::new(Fixed::ZERO, Fixed::from_int(eh), Fixed::ZERO);
        let short = Vec3::new(Fixed::ZERO, Fixed::from_int(el), Fixed::ZERO);
        assert!(line_of_sight(tall, target, &world), "high vantage sees over the wall");
        assert!(!line_of_sight(short, target, &world), "low vantage is blocked");
    }

    #[test]
    fn dragons_vary_in_age_size_and_ridge_pattern() {
        use std::collections::HashSet;
        let mut ages: HashSet<&str> = HashSet::new();
        let mut ridge_counts: HashSet<usize> = HashSet::new();
        let (mut min_len, mut max_len) = (f32::MAX, 0.0f32);
        let (mut min_tail, mut max_tail) = (usize::MAX, 0usize);
        let (mut min_loose, mut max_loose) = (f32::MAX, 0.0f32);
        for seed in 0..300u64 {
            let bp = &generate_dragons(seed, 1)[0];
            ages.insert(bp.age);
            ridge_counts.insert(bp.boxes.iter().filter(|b| b.role == BoxRole::Ridge).count());
            let tail = bp.boxes.iter().filter(|b| b.role == BoxRole::Tail).count();
            min_tail = min_tail.min(tail);
            max_tail = max_tail.max(tail);
            min_loose = min_loose.min(bp.looseness);
            max_loose = max_loose.max(bp.looseness);
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for b in &bp.boxes {
                lo = lo.min(b.offset[0] - b.half[0]);
                hi = hi.max(b.offset[0] + b.half[0]);
            }
            min_len = min_len.min(hi - lo);
            max_len = max_len.max(hi - lo);
        }
        // All three life stages occur.
        assert!(
            ages.contains("juvenile") && ages.contains("adult") && ages.contains("ancient"),
            "all life stages appear, saw {ages:?}"
        );
        // Size spans from tree-scale (~7 m) to giant (ancients many times larger).
        assert!(min_len < 14.0, "smallest dragon is tree-scale, got {min_len}");
        assert!(max_len > 45.0, "largest dragon is a giant, got {max_len}");
        assert!(max_len > min_len * 4.0, "a wide size range, {min_len}..{max_len}");
        // Ridge pattern varies — single and multi-row both appear.
        assert!(ridge_counts.len() >= 2, "ridge row counts vary, saw {ridge_counts:?}");
        assert!(
            ridge_counts.iter().any(|&c| c > 9),
            "some dragons have multi-row ridges, saw {ridge_counts:?}"
        );
        // Tail length and joint looseness both span a real range.
        assert!(max_tail >= min_tail + 4, "tail length varies, {min_tail}..{max_tail}");
        assert!(max_loose - min_loose > 0.5, "looseness varies, {min_loose:.2}..{max_loose:.2}");
    }

    #[test]
    fn small_fauna_have_birds_and_canids() {
        let mut bird = 0;
        let mut canid = 0;
        for seed in 0..400u64 {
            let bp = &generate_candidates(seed, 1)[0];
            let count = |role: BoxRole| bp.boxes.iter().filter(|b| b.role == role).count();
            match bp.arch {
                "bird" => {
                    bird += 1;
                    assert_eq!(count(BoxRole::Leg), 2, "seed {seed}: a bird stands on two legs");
                    assert_eq!(count(BoxRole::Wing), 2, "seed {seed}: folded wings");
                    assert!(bp.boxes[0].half[0] < 0.7, "seed {seed}: bird body is small/compact");
                }
                "canid" => {
                    canid += 1;
                    assert_eq!(count(BoxRole::Horn), 2, "seed {seed}: a canid has two pointed ears");
                    assert_eq!(count(BoxRole::Leg), 4, "seed {seed}: four legs");
                    assert!(bp.boxes[0].half[0] > bp.boxes[0].half[1], "seed {seed}: canid body is long/low");
                }
                _ => {}
            }
        }
        assert!(bird > 0 && canid > 0, "both small fauna occur: bird {bird}, canid {canid}");
    }

    #[test]
    fn dinosaurs_come_as_theropods_and_sauropods() {
        let mut thero = 0;
        let mut sauro = 0;
        for seed in 0..160u64 {
            let bp = &generate_dinosaurs(seed, 1)[0];
            let count = |role: BoxRole| bp.boxes.iter().filter(|b| b.role == role).count();
            assert_eq!(bp.boxes[0].role, BoxRole::Body);
            assert_eq!(count(BoxRole::Wing), 0, "seed {seed}: dinosaurs have no wings");
            assert!(count(BoxRole::Tail) >= 5, "seed {seed}: a long tail");
            assert!(bp.arch.starts_with("dinosaur · "), "got {}", bp.arch);
            // No degenerate boxes.
            for x in &bp.boxes {
                for k in 0..3 {
                    assert!(x.half[k] > 0.0 && x.half[k].is_finite() && x.offset[k].is_finite(), "seed {seed}");
                }
            }
            if bp.arch.ends_with("sauropod") {
                sauro += 1;
                assert!(count(BoxRole::Neck) >= 6, "seed {seed}: sauropod long neck");
                assert_eq!(count(BoxRole::Leg), 4, "seed {seed}: sauropod four legs");
            } else {
                thero += 1;
                assert_eq!(count(BoxRole::Leg), 4, "seed {seed}: theropod 2 hind + 2 tiny arms");
                assert_eq!(count(BoxRole::Jaw), 1, "seed {seed}: theropod maw");
            }
        }
        assert!(thero > 0 && sauro > 0, "both builds occur: theropod {thero}, sauropod {sauro}");
    }

    #[test]
    fn dinosaurs_do_not_disturb_the_general_or_dragon_sheets() {
        let g = &generate_candidates(7, 1)[0];
        assert!(["biped", "quadruped", "critter", "serpent", "heavy", "bird", "canid"].contains(&g.arch));
        let d = &generate_dragons(7, 1)[0];
        assert!(d.arch.starts_with("dragon"));
    }

    #[test]
    fn blueprints_carry_a_readable_arch_label() {
        // Dragons are labelled by build; the general sheet by archetype.
        let mut west = 0;
        let mut east = 0;
        for seed in 0..80u64 {
            let d = &generate_dragons(seed, 1)[0];
            assert!(d.arch.starts_with("dragon · western") || d.arch.starts_with("dragon · eastern"), "got {}", d.arch);
            if d.arch.contains("western") { west += 1; } else { east += 1; }
        }
        assert!(west > 0 && east > 0);
        // General sheet uses the five archetype names, never a dragon label.
        for seed in 0..80u64 {
            let g = &generate_candidates(seed, 1)[0];
            assert!(["biped", "quadruped", "critter", "serpent", "heavy", "bird", "canid"].contains(&g.arch), "got {}", g.arch);
        }
    }

    #[test]
    fn both_dragon_builds_appear_and_serpentine_is_longer_and_wingless() {
        let mut winged = 0;
        let mut serpent = 0;
        for seed in 0..200u64 {
            let bp = &generate_dragons(seed, 1)[0];
            let wings = bp.boxes.iter().filter(|b| b.role == BoxRole::Wing).count();
            let tail = bp.boxes.iter().filter(|b| b.role == BoxRole::Tail).count();
            let ratio = bp.boxes[0].half[0] / bp.boxes[0].half[1];
            if wings > 0 {
                winged += 1;
            } else {
                serpent += 1;
                // The Eastern build: leaner torso and a longer tail.
                assert!(tail >= 8, "seed {seed}: serpentine dragon has a longer tail, got {tail}");
                assert!(ratio >= 3.0, "seed {seed}: serpentine torso is leaner, ratio {ratio:.1}");
            }
        }
        assert!(winged > 0 && serpent > 0, "both builds occur: winged {winged}, serpentine {serpent}");
    }

    #[test]
    fn dragon_sheet_does_not_disturb_the_general_sheet() {
        // The general 5-archetype sheet must be byte-for-byte what it was before
        // dragons existed, so previously-noted specimen addresses stay valid.
        let a = generate_candidates(7, 16);
        let b = generate_candidates(7, 16);
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.boxes.len(), y.boxes.len());
        }
        // And a dragon sheet is clearly different in shape from the general one.
        let g = &generate_candidates(7, 1)[0];
        let d = &generate_dragons(7, 1)[0];
        assert_ne!(g.boxes.len(), d.boxes.len(), "a dragon is a distinct, busier shape");
    }

    #[test]
    fn lauscher_is_a_creepy_big_jawed_biped() {
        use std::collections::HashSet;
        let mut hinges: HashSet<&str> = HashSet::new();
        for seed in [1u64, 7, 42, 100, 256, 999] {
            let bp = &generate_lauschers(seed, 1)[0];
            assert!(bp.arch.starts_with("lauscher"), "arch {}", bp.arch);
            hinges.insert(bp.arch);
            // Bipedal: exactly the two legs ride the Leg role now.
            let legs = bp.boxes.iter().filter(|b| matches!(b.role, BoxRole::Leg)).count();
            assert_eq!(legs, 2, "bipedal (got {legs} legs)");
            // Articulated arms ride the Segment role (>=4 arms x >=3 parts each).
            let arm_parts = bp.boxes.iter().filter(|b| matches!(b.role, BoxRole::Segment)).count();
            assert!(arm_parts >= 12, "articulated clawed arms (got {arm_parts} parts)");
            // A jaw + a row of teeth, all Jaw-role.
            let maw: Vec<_> = bp.boxes.iter().filter(|b| matches!(b.role, BoxRole::Jaw)).collect();
            assert!(maw.len() >= 5, "jaw + teeth (got {})", maw.len());
            // Teeth don't protrude past the face: every maw piece sits behind the
            // snout's front edge (no buck teeth).
            let snout_front = bp.boxes.iter().find(|b| matches!(b.role, BoxRole::Snout))
                .map(|b| b.offset[0] + b.half[0]).unwrap();
            for m in &maw {
                assert!(m.offset[0] <= snout_front + 1e-4, "maw piece protrudes past the face");
            }
            // A second, UPPER row of teeth (Snout role) hangs from the palate, so
            // a gape reveals more teeth. Snout boxes = the snout itself + fangs.
            let snouts = bp.boxes.iter().filter(|b| matches!(b.role, BoxRole::Snout)).count();
            assert!(snouts >= 4, "upper fang row present (snout-role boxes: {snouts})");
            // The jaw hinge sits behind the front of the maw (a real pivot to
            // swing the mandible about).
            let hinge = bp.jaw_hinge().expect("a maw has a hinge");
            let maw_front = maw.iter().map(|b| b.offset[0] + b.half[0]).fold(f32::MIN, f32::max);
            assert!(hinge[0] < maw_front, "hinge is at the rear of the jaw, ahead of the maw front");
            // Still a Listener — a pair of dish ears (Horn role).
            let ears = bp.boxes.iter().filter(|b| matches!(b.role, BoxRole::Horn)).count();
            assert!(ears >= 2, "a pair of ears (got {ears})");
            // Humanoid: no tail, and the head sits ON TOP (more up than forward).
            let tails = bp.boxes.iter().filter(|b| matches!(b.role, BoxRole::Tail)).count();
            assert_eq!(tails, 0, "humanoid — no tail");
            let head = bp.boxes.iter().find(|b| matches!(b.role, BoxRole::Head)).unwrap();
            assert!(head.offset[1] > head.offset[0].abs() * 2.0, "head on top, not carried forward");
            // Curly body hair, mostly down the back: many small segments sitting
            // behind the body centre (negative x), not a head tuft.
            let hair: Vec<_> = bp.boxes.iter().filter(|b| matches!(b.role, BoxRole::Hair)).collect();
            assert!(hair.len() >= 24, "a curly pelt of segments (got {})", hair.len());
            let on_back = hair.iter().filter(|b| b.offset[0] < 0.0).count();
            assert!(on_back * 2 >= hair.len(), "hair sits mostly on the back");
            // standing_height spans feet to the tallest point.
            let lo = bp.boxes.iter().map(|b| b.offset[1] - b.half[1]).fold(f32::MAX, f32::min);
            let hi = bp.boxes.iter().map(|b| b.offset[1] + b.half[1]).fold(f32::MIN, f32::max);
            assert!((bp.standing_height() - (hi - lo)).abs() < 1e-3, "standing height = feet to crown");
            // Thin and tall: taller than it is laterally wide.
            let (mut ymin, mut ymax, mut zmin, mut zmax) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
            for b in &bp.boxes {
                ymin = ymin.min(b.offset[1] - b.half[1]);
                ymax = ymax.max(b.offset[1] + b.half[1]);
                zmin = zmin.min(b.offset[2] - b.half[2]);
                zmax = zmax.max(b.offset[2] + b.half[2]);
            }
            assert!((ymax - ymin) > (zmax - zmin) * 1.2, "thin and tall");
        }
        assert!(hinges.len() >= 2, "the mouth-hinge axis produces variety");
    }

    #[test]
    fn egg_choose_or_random_is_deterministic() {
        // A chosen egg forces the body plan regardless of seed.
        assert!(EggSpec::chosen(Arche::Lauscher, 123).hatch().arch.starts_with("lauscher"));
        assert!(EggSpec::chosen(Arche::Dragon, 1).hatch().arch.starts_with("dragon"));
        // A random roll matches the general candidate path for the same seed.
        let egg = EggSpec::random(99).hatch();
        let cand = &generate_candidates(99, 1)[0];
        assert_eq!(egg.arch, cand.arch);
        assert_eq!(egg.boxes.len(), cand.boxes.len());
        // The same egg always hatches the same creature (eggs are reproducible).
        let a = EggSpec::chosen(Arche::Lauscher, 55).hatch();
        let b = EggSpec::chosen(Arche::Lauscher, 55).hatch();
        assert_eq!(a.boxes.len(), b.boxes.len());
        for (x, y) in a.boxes.iter().zip(b.boxes.iter()) {
            assert_eq!(x.offset, y.offset);
            assert_eq!(x.half, y.half);
        }
    }

    #[test]
    fn egg_element_trait_can_be_chosen_or_left_to_the_seed() {
        use crate::sim::elements::Affinity;
        // A chosen element overrides the seed roll — on any body plan, not just
        // dragons (a fire fauna is a perfectly good egg).
        let fire_fauna = EggSpec::chosen(Arche::Biped, 7).with_element(Some(Affinity::FIRE)).hatch();
        assert_eq!(fire_fauna.element, "fire");
        let ice_dragon = EggSpec::chosen(Arche::Dragon, 7).with_element(Some(Affinity::ICE)).hatch();
        assert_eq!(ice_dragon.element, "ice");
        // Choosing the element changes ONLY the label, not the geometry — the body
        // is still the seed's dragon.
        let wild_dragon = EggSpec::chosen(Arche::Dragon, 7).hatch();
        assert_eq!(ice_dragon.boxes.len(), wild_dragon.boxes.len());
        // Left unchosen, a non-dragon stays elementless and a dragon keeps its
        // wild (seed-rolled) affinity.
        assert_eq!(EggSpec::chosen(Arche::Biped, 7).hatch().element, "");
        assert!(!EggSpec::chosen(Arche::Dragon, 7).hatch().element.is_empty());
        // The picker's choices all round-trip through label/from_label.
        for a in Affinity::CHOOSABLE {
            assert_eq!(Affinity::from_label(a.label()), Some(a));
        }
    }

    #[test]
    fn body_colour_is_genome_driven_and_heritable() {
        // Torso (box 0) colour now comes from the seed's genome blended into the
        // palette, so different seeds give different coats...
        let a = EggSpec::chosen(Arche::Quadruped, 1).hatch();
        let b = EggSpec::chosen(Arche::Quadruped, 2).hatch();
        assert_ne!(a.boxes[0].color, b.boxes[0].color, "colour varies by genome");
        for ch in a.boxes[0].color {
            assert!((0.0..=1.0).contains(&ch), "colour channel in range: {ch}");
        }
        // ...but the same seed reproduces the same coat (deterministic), and the
        // geometry is identical (the colour change doesn't perturb the RNG).
        let a2 = EggSpec::chosen(Arche::Quadruped, 1).hatch();
        assert_eq!(a.boxes[0].color, a2.boxes[0].color);
        assert_eq!(a.boxes.len(), a2.boxes.len());
        for (x, y) in a.boxes.iter().zip(a2.boxes.iter()) {
            assert_eq!(x.offset, y.offset);
            assert_eq!(x.half, y.half);
        }
    }

    #[test]
    fn arche_labels_round_trip_and_include_lauscher() {
        assert!(Arche::CHOOSABLE.contains(&Arche::Lauscher));
        for a in Arche::CHOOSABLE {
            assert_eq!(Arche::from_label(a.label()), Some(*a));
        }
        assert_eq!(Arche::from_label("nonsense"), None);
    }

    #[test]
    fn dump_blueprints_json() {
        if std::env::var("DUMP_BLUEPRINTS").is_err() {
            return;
        }
        let n: usize = std::env::var("DUMP_N").ok().and_then(|v| v.parse().ok()).unwrap_or(16);
        let seed: u64 = std::env::var("DUMP_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(42);
        // DUMP_SEEDS = how many consecutive seeds to emit (each tagged), so the
        // phone viewer's reroll has a spread of shapes to browse.
        let seeds: u64 = std::env::var("DUMP_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        // DUMP_ARCHE=dragon | dino | all bakes a focused or combined sheet.
        let mode = std::env::var("DUMP_ARCHE").unwrap_or_default();
        let mut s = String::from("[");
        let mut first = true;
        for sd in seed..seed + seeds {
            let mut sheet = Vec::new();
            match mode.as_str() {
                "dragon" => sheet.extend(generate_dragons(sd, n)),
                "dino" => sheet.extend(generate_dinosaurs(sd, n)),
                "lauscher" => sheet.extend(generate_lauschers(sd, n)),
                "all" => {
                    sheet.extend(generate_candidates(sd, n));
                    sheet.extend(generate_lauschers(sd, n));
                    sheet.extend(generate_dragons(sd, n));
                    sheet.extend(generate_dinosaurs(sd, n));
                }
                _ => sheet.extend(generate_candidates(sd, n)),
            }
            for bp in sheet.iter() {
                if !first {
                    s.push(',');
                }
                first = false;
                s.push_str(&format!("{{\"seed\":{},\"arch\":\"{}\",\"age\":\"{}\",\"loose\":{:.3},\"element\":\"{}\",\"bump\":{:.3},\"pattern\":\"{}\",\"pcolor\":[{:.3},{:.3},{:.3}],\"boxes\":[", sd, bp.arch, bp.age, bp.looseness, bp.element, bp.scale_bumpiness, bp.pattern.label(), bp.pattern_color[0], bp.pattern_color[1], bp.pattern_color[2]));
                for (bi, b) in bp.boxes.iter().enumerate() {
                    if bi > 0 {
                        s.push(',');
                    }
                    s.push_str(&format!(
                        "{{\"o\":[{:.4},{:.4},{:.4}],\"h\":[{:.4},{:.4},{:.4}],\"c\":[{:.4},{:.4},{:.4}],\"r\":{}}}",
                        b.offset[0], b.offset[1], b.offset[2],
                        b.half[0], b.half[1], b.half[2],
                        b.color[0], b.color[1], b.color[2],
                        b.role as u8
                    ));
                }
                s.push_str("]}");
            }
        }
        s.push(']');
        std::fs::write("/tmp/blueprints.json", s).unwrap();
    }

    #[test]
    fn generates_the_requested_count_with_a_torso_each() {
        let c = generate_candidates(42, 12);
        assert_eq!(c.len(), 12);
        for bp in &c {
            assert!(!bp.boxes.is_empty(), "every candidate has at least a torso");
        }
    }

    #[test]
    fn box_zero_is_always_the_body_and_roles_are_tagged() {
        for seed in 0..200u64 {
            let bp = &generate_candidates(seed, 1)[0];
            assert_eq!(bp.boxes[0].role, BoxRole::Body, "box 0 is the torso");
            // No stray Body boxes beyond the torso — every appendage is tagged.
            let extra_body = bp.boxes[1..].iter().filter(|b| b.role == BoxRole::Body).count();
            assert_eq!(extra_body, 0, "seed {seed}: appendages must carry a non-Body role");
            // A head always exists (neck tip).
            assert!(bp.boxes.iter().any(|b| b.role == BoxRole::Head), "seed {seed}: has a head");
        }
    }

    #[test]
    fn is_deterministic_for_a_seed() {
        let a = generate_candidates(7, 16);
        let b = generate_candidates(7, 16);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.boxes.len(), y.boxes.len());
            for (bx, by) in x.boxes.iter().zip(&y.boxes) {
                assert_eq!(bx.offset, by.offset);
                assert_eq!(bx.half, by.half);
                assert_eq!(bx.color, by.color);
            }
        }
        // Different seeds should (almost surely) differ in shape.
        let d = generate_candidates(8, 16);
        assert!(a.iter().zip(&d).any(|(x, y)| x.boxes.len() != y.boxes.len()
            || x.boxes[0].half != y.boxes[0].half));
    }

    #[test]
    fn boxes_are_finite_and_non_degenerate() {
        for bp in generate_candidates(123, 20) {
            for b in &bp.boxes {
                for k in 0..3 {
                    assert!(b.half[k] > 0.0 && b.half[k].is_finite());
                    assert!(b.offset[k].is_finite());
                    assert!((0.0..=1.0).contains(&b.color[k]));
                }
            }
        }
    }

    #[test]
    fn dragon_skin_genes_are_in_range_and_deterministic() {
        for seed in 0..500u64 {
            let (bump, _kind, accent) = dragon_skin_genes(seed);
            assert!((0.0..=1.0).contains(&bump), "bumpiness in range: {bump}");
            assert!(accent.iter().all(|&c| (0.0..=1.0).contains(&c)), "accent in gamut");
            // Deterministic per seed → round-trips through seed-based save/load.
            assert_eq!(dragon_skin_genes(seed), dragon_skin_genes(seed));
        }
        // All four pattern kinds appear across seeds (the gene actually varies).
        let mut seen = [false; 4];
        for seed in 0..1000u64 {
            seen[dragon_skin_genes(seed).1 as usize] = true;
        }
        assert!(seen.iter().all(|&s| s), "every PatternKind should occur across seeds");
    }

    #[test]
    fn hatched_dragon_carries_skin_genes_and_rehatch_round_trips() {
        for seed in 0..200u64 {
            let d = EggSpec::chosen(Arche::Dragon, seed).hatch();
            assert!((0.0..=1.0).contains(&d.scale_bumpiness));
            assert!(d.pattern_color.iter().all(|&c| (0.0..=1.0).contains(&c)));
            // Re-hatching the same egg reproduces the genes exactly (round-trip).
            let again = EggSpec::chosen(Arche::Dragon, seed).hatch();
            assert_eq!(d.scale_bumpiness, again.scale_bumpiness);
            assert_eq!(d.pattern, again.pattern);
            assert_eq!(d.pattern_color, again.pattern_color);
        }
        // Non-dragons keep the inert defaults (the genes are a dragon trait).
        let fauna = EggSpec::chosen(Arche::Biped, 5).hatch();
        assert_eq!(fauna.scale_bumpiness, 0.0);
        assert_eq!(fauna.pattern, PatternKind::Plain);
    }
}
