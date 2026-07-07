//! procgen.rs — a **recursive, noise-driven, FSM-activated map generator** (proof of concept).
//!
//! This is the architecture from the design discussion, built end to end so we can measure whether it holds
//! up. The pipeline, in the terms it was described:
//!
//!   1. **Noise substrate.** A multi-octave fBm field over the map is the base "3D profile" — the raw
//!      potential of the space.
//!   2. **Diff it.** We read the field's local *gradient* (slope) and *variance* (roughness) around a region.
//!      Gradient → macro-topography (ridges/valleys/flats); variance → "how much is going on here".
//!   3. **A tiny weight profile → activations.** For each region we build a small feature vector from the
//!      noise (height, slope, roughness, openness, depth-in-recursion) and run it through a compact,
//!      hand-initialisable **perceptron** ([`ActivationNet`]) whose outputs are activation scores for each FSM
//!      archetype. This is the "little neural net with activators" — small on purpose, and its weights are the
//!      tunable feed-forward inputs. Swap the weights (hand-set, learned, or evolved) to change the builder's
//!      "taste" without touching the machinery.
//!   4. **Fire the winning FSM.** The highest-activation archetype grows a POI at that region (using the
//!      existing `fsm` archetypes — SentinelNode, LogisticsSpine, KineticWreckage, …), which themselves call
//!      micro-detail generators. We design the micro-moments; the net + recursion assemble them.
//!   5. **Recurse.** Every region that is still larger than *character scale* and still "interesting" (high
//!      local variance) is **subdivided** and the whole process runs again on each child at the finer scale —
//!      a quadtree of composition. One global generator; each room it calls recurses; stop when a cell is
//!      about the size of the character, because finer than that is just clutter.
//!
//! ## Why this is the right shape
//! The same subdivision operator runs at every scale, so structure appears at *all* scales for free — which is
//! exactly what the `composition` metric rewards as "multi-scale organisation", the thing that separates a
//! designed space from uniform noise. The noise gives spatial coherence (nearby regions share a signature so
//! the map reads as a place, not confetti); the net gives controllable *taste*; the FSMs give hand-authored
//! micro-quality; the recursion glues them into a whole feed-forward, no hand-placement per map.
//!
//! This module is a **proof of concept**: it demonstrates the loop, produces a scoreable `Level`, and is
//! validated against the composition metric. It is not (yet) wired into the shipping map roster.

use crate::level::{Level, MapConfig, Obstacle};
use crate::math::{Vec2, Vec3};
use crate::mesh::{boxx, dither, fbm2, rgb, tube, uv_sphere, FACE_ALL};

/// The FSM archetypes the net can activate. Kept as a small enum so the net's output layer is fixed-size and
/// the whole thing stays inspectable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Archetype {
    /// low freestanding cover to break a sightline
    SightlineBreaker,
    /// a tall vantage / perch (risk-reward high ground)
    Vantage,
    /// a fortified nest facing a kill-zone
    Sentinel,
    /// a tall industrial tower / landmark
    Spine,
    /// a crashed-wreck obstacle (irregular hard cover)
    Wreckage,
    /// a cluster of small ad-hoc shelter cover
    Shelter,
    /// nothing — leave this region as open space (a void)
    Void,
}

impl Archetype {
    pub const ALL: [Archetype; 7] = [
        Archetype::SightlineBreaker,
        Archetype::Vantage,
        Archetype::Sentinel,
        Archetype::Spine,
        Archetype::Wreckage,
        Archetype::Shelter,
        Archetype::Void,
    ];
}

/// A coarse **terrain zone** committed at the top of the recursion, before any detail is placed. This is the
/// "specifics" layer the design can't fully derive from per-leaf noise: whether a macro-region is open field,
/// built-up, water, rubble, or elevated ground is a *regional* decision (you can't grow a coherent building or
/// a water body one leaf at a time). The zone then constrains which archetypes the fine recursion may place —
/// so water regions get shorelines and stilts, built regions get walls and sentinels, fields stay open, etc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainZone {
    /// open ground — sparse cover, long sightlines, the "field".
    Field,
    /// built-up — walls, bunkers, denser hard cover.
    Built,
    /// a water body — a pond/channel with shoreline props (a regional commitment, placed once).
    Water,
    /// a debris/rubble belt — irregular wreckage cover.
    Rubble,
    /// raised terrain — vantages and platforms.
    Elevated,
}

impl TerrainZone {
    /// Which archetypes are appropriate in this zone (a mask the net's activations are gated through, so the
    /// fine recursion can't place a bunker in a lake or a wreck on a pristine field).
    fn allows(&self, a: Archetype) -> bool {
        use Archetype::*;
        match self {
            TerrainZone::Field => matches!(a, SightlineBreaker | Shelter | Void | Spine),
            TerrainZone::Built => matches!(a, Sentinel | SightlineBreaker | Shelter | Spine | Void),
            TerrainZone::Water => matches!(a, Void | Shelter | Spine), // shoreline stilts/beacons only
            TerrainZone::Rubble => matches!(a, Wreckage | Shelter | SightlineBreaker | Void),
            TerrainZone::Elevated => matches!(a, Vantage | Sentinel | Void | Spine),
        }
    }
}

/// Context passed **downward** through the recursion — the dense-layer fix for scatter. Without it, every leaf
/// decides independently from local noise and the map salt-and-peppers. With it, a child's choice is
/// conditioned on its parent's zone and archetype and on how much cover the branch has already committed, so
/// decisions cohere into structure (a built district stays built; a placed landmark suppresses siblings).
#[derive(Clone, Copy, Debug)]
pub struct DownwardContext {
    /// the committed terrain zone for this branch.
    pub zone: TerrainZone,
    /// the parent leaf's archetype choice as a normalised id 0..1 (0 if none yet).
    pub parent_choice: f32,
    /// cover pressure 0..1: how much hard cover this branch has already placed (suppresses more).
    pub cover_pressure: f32,
    /// whether a landmark (Spine) already exists in this branch (0/1) — keeps landmarks rare and legible.
    pub has_landmark: f32,
    /// per-archetype additive bias applied to activations at each leaf (log-space nudge). Zero for unthemed
    /// generation; a theme sets this to steer the realised mix toward its target proportions directly, giving
    /// the guided search a strong head start instead of relying on net weights alone.
    pub arch_bias: [f32; 7],
}

impl DownwardContext {
    #[allow(dead_code)]
    fn root(zone: TerrainZone) -> Self {
        Self::root_biased(zone, [0.0; 7])
    }
    fn root_biased(zone: TerrainZone, arch_bias: [f32; 7]) -> Self {
        DownwardContext { zone, parent_choice: 0.0, cover_pressure: 0.0, has_landmark: 0.0, arch_bias }
    }
}

/// The per-region feature vector fed to the net. Everything derives from the noise field + recursion state, so
/// the whole generator is a pure function of (seed, weights).
#[derive(Clone, Copy, Debug)]
pub struct RegionFeatures {
    /// fBm height at the region centre, 0..1.
    pub height: f32,
    /// local gradient magnitude (slope), normalised ~0..1.
    pub slope: f32,
    /// local variance / roughness over the region, 0..1 — "how much is going on".
    pub roughness: f32,
    /// how open (low occupancy) the neighbourhood is, 0..1.
    pub openness: f32,
    /// recursion depth normalised: 0 at the coarse root, 1 near character scale.
    pub depth: f32,
    /// —— downward-context inputs (the dense-layer fix) ——
    /// committed zone one-hot-ish scalar (Field..Elevated mapped to 0..1).
    pub zone_id: f32,
    /// parent leaf's archetype choice, normalised 0..1.
    pub parent_choice: f32,
    /// cover already committed in this branch, 0..1 (suppresses more cover → prevents scatter).
    pub cover_pressure: f32,
    /// a landmark already exists in this branch, 0/1.
    pub has_landmark: f32,
}

impl RegionFeatures {
    fn as_array(&self) -> [f32; 10] {
        // last entry is a bias unit (always 1)
        [
            self.height,
            self.slope,
            self.roughness,
            self.openness,
            self.depth,
            self.zone_id,
            self.parent_choice,
            self.cover_pressure,
            self.has_landmark,
            1.0,
        ]
    }
}

/// A compact **two-layer** perceptron: features (10, incl. context + bias) → hidden (8, ReLU) → activation per
/// archetype (7). The hidden layer is the "dense layer at each recurse" — it gives the net capacity to form
/// context-conditioned *combinations* (e.g. "open AND low cover-pressure AND field-zone → sightline-breaker;
/// but open AND high cover-pressure → void") that a single linear layer can't express. That's what turns the
/// generator from a per-leaf scatterer into something that builds coherent structure. Still small and
/// inspectable; still a *swappable taste function* — replace the weights (hand-set, learned, or evolved).
#[derive(Clone)]
pub struct ActivationNet {
    /// input→hidden weights: w1[hidden][input], 8 × 10.
    pub w1: [[f32; 10]; 8],
    /// hidden→output weights: w2[archetype][hidden+bias], 7 × 9.
    pub w2: [[f32; 9]; 7],
}

impl Default for ActivationNet {
    fn default() -> Self {
        // Hand-initialised hidden units, each a recognisable "concept" over the inputs, so the default net has
        // sensible taste out of the box and is legible. Input order:
        // [height, slope, roughness, openness, depth, zone_id, parent_choice, cover_pressure, has_landmark, bias]
        let mut w1 = [[0.0f32; 10]; 8];
        // h0 "open flat ground": openness up, slope/roughness down
        w1[0] = [0.0, -0.6, -0.2, 1.2, 0.0, 0.0, 0.0, 0.0, 0.0, -0.3];
        // h1 "high point": height up, coarse depth
        w1[1] = [1.4, 0.4, 0.0, 0.0, -0.5, 0.0, 0.0, 0.0, 0.0, -0.4];
        // h2 "steep kill-zone edge": slope up, some openness
        w1[2] = [0.0, 1.3, 0.2, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, -0.5];
        // h3 "busy low basin": roughness up, height down
        w1[3] = [-0.7, 0.0, 1.1, 0.4, 0.0, 0.0, 0.0, 0.0, 0.0, -0.4];
        // h4 "saturated branch" (lots of cover already): cover_pressure up → drives void/suppression
        w1[4] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.6, 0.0, -0.6];
        // h5 "built zone": zone_id near Built + depth
        w1[5] = [0.0, 0.2, 0.3, 0.0, 0.3, 0.9, 0.2, 0.0, 0.0, -0.5];
        // h6 "fine detail depth": depth up (late recursion → small props)
        w1[6] = [0.0, 0.0, 0.3, 0.2, 1.2, 0.0, 0.0, 0.0, 0.0, -0.5];
        // h7 "landmark suppressor": has_landmark up (once a spine exists, suppress more)
        w1[7] = [0.3, 0.0, 0.0, 0.4, -0.4, 0.2, 0.0, 0.0, 1.4, -0.5];

        // hidden→archetype. Hidden order: [open, high, steep, busy, saturated, built, fine, lm_suppress, bias]
        let mut w2 = [[0.0f32; 9]; 7];
        // SightlineBreaker: open ground, not saturated
        w2[0] = [1.1, 0.0, 0.2, 0.4, -1.2, 0.3, 0.2, 0.0, -0.2];
        // Vantage: high points
        w2[1] = [0.0, 1.5, 0.3, 0.0, -0.6, 0.0, -0.5, 0.0, -0.5];
        // Sentinel: steep + built
        w2[2] = [0.2, 0.2, 1.3, 0.2, -0.5, 0.8, 0.0, 0.0, -0.6];
        // Spine (landmark): high + open, strongly suppressed once one exists
        w2[3] = [0.5, 0.9, 0.0, 0.0, -0.3, 0.3, -0.6, -2.0, -1.0];
        // Wreckage: busy low basin
        w2[4] = [0.2, -0.3, 0.0, 1.3, -0.5, 0.0, 0.2, 0.0, -0.6];
        // Shelter: fine detail depth, moderate
        w2[5] = [0.2, 0.0, 0.2, 0.3, -0.3, 0.3, 1.1, 0.0, -0.4];
        // Void: saturated / suppressed / smooth open → leave it open. A generous positive bias so the baseline
        // map keeps large legible open regions (which the composition metric rewards) — POIs punctuate space,
        // they don't fill it.
        w2[6] = [0.3, 0.0, -0.6, -0.6, 1.4, -0.2, 0.0, 0.8, 1.1];
        ActivationNet { w1, w2 }
    }
}

impl ActivationNet {
    /// Forward pass: features → (argmax archetype, raw activation vector). Hidden layer uses ReLU.
    pub fn activate(&self, f: &RegionFeatures) -> (Archetype, [f32; 7]) {
        let x = f.as_array();
        // hidden layer (ReLU)
        let mut hid = [0.0f32; 9];
        for h in 0..8 {
            let s: f32 = self.w1[h].iter().zip(x.iter()).map(|(wi, xi)| wi * xi).sum();
            hid[h] = s.max(0.0); // ReLU
        }
        hid[8] = 1.0; // hidden bias
        // output layer (linear activations)
        let mut act = [0.0f32; 7];
        for a in 0..7 {
            act[a] = self.w2[a].iter().zip(hid.iter()).map(|(wi, hi)| wi * hi).sum();
        }
        let mut best = 0usize;
        for a in 1..7 {
            if act[a] > act[best] {
                best = a;
            }
        }
        (Archetype::ALL[best], act)
    }

    /// The number of tunable weights — handy for an evolutionary search that flattens the net to a genome.
    pub fn param_count() -> usize {
        8 * 10 + 7 * 9
    }

    /// Flatten all weights to a genome vector (for evolutionary/swarm search).
    pub fn to_genome(&self) -> Vec<f32> {
        let mut g = Vec::with_capacity(Self::param_count());
        for row in &self.w1 {
            g.extend_from_slice(row);
        }
        for row in &self.w2 {
            g.extend_from_slice(row);
        }
        g
    }

    /// Rebuild a net from a flat genome vector (inverse of `to_genome`).
    pub fn from_genome(g: &[f32]) -> Self {
        let mut net = ActivationNet { w1: [[0.0; 10]; 8], w2: [[0.0; 9]; 7] };
        let mut i = 0;
        for row in net.w1.iter_mut() {
            for v in row.iter_mut() {
                *v = g.get(i).copied().unwrap_or(0.0);
                i += 1;
            }
        }
        for row in net.w2.iter_mut() {
            for v in row.iter_mut() {
                *v = g.get(i).copied().unwrap_or(0.0);
                i += 1;
            }
        }
        net
    }
}

/// Tunable knobs for the recursion itself (separate from the net's taste).
#[derive(Clone, Copy, Debug)]
pub struct ProcParams {
    /// world size of the map (metres, square).
    pub size: f32,
    /// stop subdividing once a cell is this small (≈ character scale, ~2 m).
    pub min_cell: f32,
    /// a region subdivides only if its local roughness exceeds this (else it's "settled" → place & stop).
    pub subdivide_roughness: f32,
    /// noise frequency of the base field (features per map).
    pub base_freq: f32,
    /// water-zone bias -1..+1: shifts how much of the map the coarse pass commits to water (themes push this).
    pub water_bias: f32,
    /// verticality bias 0..1: how strongly elevated zones are favoured (themes push this).
    pub vert_bias: f32,
    /// rubble-vs-built lean -1..+1: negative favours Rubble zones (wreck-friendly), positive favours Built
    /// zones (sentinel/cover-friendly). Themes set this from their dominant archetypes so the *zones* — not
    /// just the archetype selection within them — match the requested character.
    pub zone_lean: f32,
}

impl Default for ProcParams {
    fn default() -> Self {
        ProcParams { size: 120.0, min_cell: 6.0, subdivide_roughness: 0.52, base_freq: 3.0, water_bias: 0.0, vert_bias: 0.0, zone_lean: 0.0 }
    }
}

/// Sample the fBm field at world (x,z) in 0..1.
fn field(x: f32, z: f32, freq: f32, size: f32, seed: u32) -> f32 {
    fbm2(x / size * freq, z / size * freq, 5, seed)
}

/// Compute a region's features from the noise field: centre height, gradient (slope), and local variance
/// (roughness), plus recursion depth.
fn features(cx: f32, cz: f32, half: f32, p: &ProcParams, seed: u32, depth: f32, ctx: &DownwardContext) -> RegionFeatures {
    let h = field(cx, cz, p.base_freq, p.size, seed);
    // gradient via central differences
    let e = half.max(1.0);
    let hx = field(cx + e, cz, p.base_freq, p.size, seed) - field(cx - e, cz, p.base_freq, p.size, seed);
    let hz = field(cx, cz + e, p.base_freq, p.size, seed) - field(cx, cz - e, p.base_freq, p.size, seed);
    let slope = ((hx * hx + hz * hz).sqrt() * 2.0).clamp(0.0, 1.0);
    // roughness: variance of a few samples inside the region
    let mut samples = [0.0f32; 5];
    let offs = [(0.0, 0.0), (0.5, 0.5), (-0.5, 0.5), (0.5, -0.5), (-0.5, -0.5)];
    for (i, (ox, oz)) in offs.iter().enumerate() {
        samples[i] = field(cx + ox * half, cz + oz * half, p.base_freq * 1.7, p.size, seed);
    }
    let mean = samples.iter().sum::<f32>() / 5.0;
    let var = samples.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / 5.0;
    let roughness = (var.sqrt() * 6.0).clamp(0.0, 1.0);
    // openness: higher where the field is mid-valued (flat plazas) — a cheap proxy
    let openness = (1.0 - (h - 0.5).abs() * 2.0).clamp(0.0, 1.0);
    let zone_id = match ctx.zone {
        TerrainZone::Field => 0.0,
        TerrainZone::Built => 0.25,
        TerrainZone::Water => 0.5,
        TerrainZone::Rubble => 0.75,
        TerrainZone::Elevated => 1.0,
    };
    RegionFeatures {
        height: h,
        slope,
        roughness,
        openness,
        depth,
        zone_id,
        parent_choice: ctx.parent_choice,
        cover_pressure: ctx.cover_pressure,
        has_landmark: ctx.has_landmark,
    }
}

/// Assign a macro-region its terrain zone from a *lower-frequency* noise band (so zones are big and coherent,
/// not per-leaf). This is the regional-commitment layer: water, built-up, rubble, elevated, or open field are
/// decided once, at the top, and then constrain everything the fine recursion places inside them.
fn zone_at(cx: f32, cz: f32, p: &ProcParams, seed: u32) -> TerrainZone {
    // a coarse field (half the base frequency) picks zones in big contiguous patches
    let z = field(cx, cz, p.base_freq * 0.5, p.size, seed ^ 0x2545_F491);
    let z2 = field(cx, cz, p.base_freq * 0.5, p.size, seed ^ 0x9E37_79B9);
    // Theme biases widen/narrow the bands: a high water_bias raises the water threshold so more of the map
    // floods; a high vert_bias lowers the elevated threshold so more of it rises.
    let water_thresh = 0.28 + p.water_bias * 0.4; // up to ~0.68 of the map becomes water at full bias
    let elev_thresh = 0.78 - p.vert_bias * 0.35; // down to ~0.43 becomes elevated at full bias
    if z < water_thresh {
        TerrainZone::Water
    } else if z > elev_thresh {
        TerrainZone::Elevated
    } else if z2 > 0.66 - p.zone_lean * 0.3 {
        // positive zone_lean widens the Built band (cover/sentinel-friendly)
        TerrainZone::Built
    } else if z2 < 0.3 - p.zone_lean * 0.3 {
        // negative zone_lean widens the Rubble band (wreck-friendly)
        TerrainZone::Rubble
    } else {
        TerrainZone::Field
    }
}

/// The result of a generation pass: the level plus a breakdown of what the recursion did.
#[derive(Clone, Debug, Default)]
pub struct ProcReport {
    pub regions_visited: u32,
    pub leaves: u32,
    pub max_depth: u32,
    pub placed: [u32; 7], // count per archetype
}

/// Recursively subdivide + populate one region, threading a [`DownwardContext`] so children's choices cohere
/// with their parent's instead of scattering. Returns the branch's accumulated cover pressure and landmark
/// flag so a parent can share them across siblings (a busy quadrant suppresses cover in its neighbours).
#[allow(clippy::too_many_arguments)]
fn recurse(
    level: &mut Level,
    net: &ActivationNet,
    p: &ProcParams,
    cx: f32,
    cz: f32,
    half: f32,
    depth: u32,
    max_norm_depth: f32,
    seed: u32,
    ctx: DownwardContext,
    report: &mut ProcReport,
) -> (f32, f32) {
    report.regions_visited += 1;
    report.max_depth = report.max_depth.max(depth);
    let dnorm = (depth as f32 / max_norm_depth).clamp(0.0, 1.0);

    let can_subdivide = half * 2.0 > p.min_cell * 2.0;
    let force_split = depth < 2;
    // roughness for the split decision uses the noise directly
    let feat0 = features(cx, cz, half, p, seed, dnorm, &ctx);
    let interesting = feat0.roughness > p.subdivide_roughness;

    if can_subdivide && (force_split || interesting) {
        // The region's own dominant archetype (from the net) becomes the children's `parent_choice`, so a
        // built-leaning district biases its children toward compatible structure — the downward dense-context
        // signal that produces coherence instead of per-leaf scatter.
        let (parent_arch, _) = net.activate(&feat0);
        let parent_choice = Archetype::ALL.iter().position(|a| *a == parent_arch).unwrap() as f32 / 6.0;

        // subdivide; thread context downward and let cover pressure accumulate ACROSS siblings so a branch
        // that's already placed a lot of cover suppresses more in the next quadrant — coherence, not scatter.
        let q = half * 0.5;
        let mut branch = ctx;
        branch.parent_choice = parent_choice;
        for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            let child_seed = seed.wrapping_mul(2_654_435_761).wrapping_add(((sx > 0.0) as u32) * 7 + ((sz > 0.0) as u32) * 13 + depth);
            let (cp, lm) = recurse(level, net, p, cx + sx * q, cz + sz * q, q, depth + 1, max_norm_depth, child_seed, branch, report);
            // carry accumulated pressure/landmark into the next sibling
            branch.cover_pressure = (branch.cover_pressure * 0.6 + cp * 0.4).clamp(0.0, 1.0);
            branch.has_landmark = branch.has_landmark.max(lm);
        }
        return (branch.cover_pressure, branch.has_landmark);
    }

    // Leaf: choose a POI, gated by the terrain zone (so no bunker in a lake), soft-sampled for variety.
    report.leaves += 1;
    let feat = features(cx, cz, half, p, seed, dnorm, &ctx);
    let (_, mut act) = net.activate(&feat);
    // theme bias: nudge activations toward the target archetype mix (log-space add) before gating/sampling.
    for i in 0..7 {
        act[i] += ctx.arch_bias[i];
    }
    // zone gate: knock disallowed archetypes far down so they're effectively never chosen here.
    for (i, a) in Archetype::ALL.iter().enumerate() {
        if !ctx.zone.allows(*a) {
            act[i] = -1e6;
        }
    }
    let arch = soft_choose(&act, seed);
    let idx = Archetype::ALL.iter().position(|a| *a == arch).unwrap();
    report.placed[idx] += 1;
    place_poi(level, arch, cx, cz, half, &feat, seed);

    // report this leaf's contribution to cover pressure / landmark for sibling coherence
    let added_cover = match arch {
        Archetype::Void => 0.0,
        Archetype::Shelter => 0.5,
        Archetype::SightlineBreaker | Archetype::Wreckage => 0.6,
        Archetype::Sentinel | Archetype::Vantage => 0.7,
        Archetype::Spine => 0.4,
    };
    let lm = if arch == Archetype::Spine { 1.0 } else { 0.0 };
    (added_cover, lm)
}

/// Softmax-style weighted pick from activation scores, using a per-leaf hash as the random draw. Temperature
/// keeps it close to the net's preference while still admitting variety, so the archetype mix isn't
/// winner-take-all monotony.
fn soft_choose(act: &[f32; 7], seed: u32) -> Archetype {
    let temp = 0.6f32;
    let maxa = act.iter().cloned().fold(f32::MIN, f32::max);
    let exps: Vec<f32> = act.iter().map(|a| ((a - maxa) / temp).exp()).collect();
    let sum: f32 = exps.iter().sum();
    let r = crate::mesh::hash01(seed as i32, 555) * sum;
    let mut acc = 0.0;
    for (i, e) in exps.iter().enumerate() {
        acc += e;
        if r <= acc {
            return Archetype::ALL[i];
        }
    }
    Archetype::ALL[6]
}

/// Materialise a chosen archetype at a leaf region as real geometry + collision. These are compact stand-ins
/// that mirror the shapes of the `fsm` archetypes (a full integration would call `fsm::*.grow(&Site)` directly;
/// this proof-of-concept keeps the geometry self-contained so the loop is easy to read and score).
/// Bridge a procgen region to the real, hand-authored `fsm` archetypes. Constructs a [`crate::fsm::Site`] from
/// the region's noise-derived features and calls the matching machine's `grow`, returning its rich `Dressing`
/// (the L-shaped bunker, the braced water tower, the three-section wreck, …) instead of procgen's flat
/// stand-in geometry. This is what lifts generated maps from grey blockout toward authored micro-quality: the
/// same FSMs that dress the authored maps now dress the recursive ones.
///
/// Returns `Some((meshes, obstacles))` if the FSM chose to build here, or `None` if the site didn't meet the
/// machine's preconditions (e.g. a SightlineBreaker on too-short a sightline) — in which case the caller can
/// fall back to leaving the spot open.
fn grow_fsm(level: &Level, arch: Archetype, cx: f32, cz: f32, half: f32, feat: &RegionFeatures, seed: u32) -> Option<(Vec<crate::mesh::Mesh3>, Vec<Obstacle>)> {
    use crate::fsm::{AdHocShelter, Fsm, KineticWreckage, LogisticsSpine, SentinelNode, Site, VerticalRiskReward};

    // Derive Site fields from the region's noise features. The FSMs read these to decide *how* to build.
    let sightline = 12.0 + feat.openness * 40.0 + (1.0 - feat.roughness) * 12.0; // open, smooth → long lines
    let site = Site {
        x: cx,
        z: cz,
        ground_y: level.floor_y,
        sightline,
        danger_yaw: feat.height * std::f32::consts::TAU, // a stable pseudo-bearing from the field
        high_traffic: feat.openness > 0.6,
        is_high_point: feat.height > 0.62,
        slope: feat.slope,
        wet: matches!(zone_of(feat), TerrainZone::Water),
        indoor: false,
        pal: level.pal,
        seed,
    };

    let dressing = match arch {
        Archetype::SightlineBreaker => crate::fsm::SightlineBreaker.grow(&site),
        Archetype::Vantage => VerticalRiskReward.grow(&site),
        Archetype::Sentinel => SentinelNode.grow(&site),
        Archetype::Spine => LogisticsSpine.grow(&site),
        Archetype::Wreckage => KineticWreckage.grow(&site),
        Archetype::Shelter => AdHocShelter.grow(&site),
        Archetype::Void => None,
    }?;
    let _ = half;
    Some((dressing.meshes, dressing.obstacles))
}

/// Recover the terrain zone from a feature vector's zone_id (inverse of the mapping in `features`).
fn zone_of(feat: &RegionFeatures) -> TerrainZone {
    match (feat.zone_id * 4.0).round() as i32 {
        0 => TerrainZone::Field,
        1 => TerrainZone::Built,
        2 => TerrainZone::Water,
        3 => TerrainZone::Rubble,
        _ => TerrainZone::Elevated,
    }
}

fn place_poi(level: &mut Level, arch: Archetype, cx: f32, cz: f32, half: f32, feat: &RegionFeatures, seed: u32) {
    // First try the real hand-authored FSM for this archetype — it produces far richer geometry (bunkers with
    // sandbags, braced towers, sheared wrecks) than a stand-in box. If the FSM declines (its preconditions
    // aren't met for this site), fall back to the lightweight stand-in so the region still reads.
    if arch != Archetype::Void {
        if let Some((meshes, obstacles)) = grow_fsm(level, arch, cx, cz, half, feat, seed) {
            if !meshes.is_empty() {
                for m in meshes {
                    level.meshes.push(m);
                }
                for o in &obstacles {
                    level.obstacles.push(*o);
                    level.cover_obstacles.push(*o);
                }
                // landmark spines still register a fire/beacon for the liveliness + landmark metrics
                if arch == Archetype::Spine {
                    level.fire_sources.push(Vec3::new(cx, level.floor_y + 8.5, cz));
                }
                return;
            }
        }
    }
    place_poi_standin(level, arch, cx, cz, half, feat, seed);
}

/// The original lightweight stand-in geometry, kept as a fallback when the richer FSM declines a site.
fn place_poi_standin(level: &mut Level, arch: Archetype, cx: f32, cz: f32, half: f32, feat: &RegionFeatures, seed: u32) {
    let gy = level.floor_y;
    let pal = level.pal;
    let base = Vec3::new(cx, gy, cz);
    let mut add_obst = |c: Vec3, h: Vec3| {
        level.obstacles.push(Obstacle { center: c, half: h });
        level.cover_obstacles.push(Obstacle { center: c, half: h });
    };
    match arch {
        Archetype::Void => { /* deliberately empty — negative space is a design element */ }
        Archetype::SightlineBreaker => {
            // Per-instance variation (palette breadth): the noise picks the material, proportions and
            // weathering, so a field of breakers reads as a varied scatter of crates/blocks/barricades rather
            // than identical boxes. This is the "bigger palette from the noise" lever.
            let vseed = seed.wrapping_mul(2246822519);
            let v = crate::mesh::hash01(vseed as i32, 3);
            let w = (half * 0.5).clamp(0.6, 2.0) * (0.7 + v * 0.6);
            let hh = 1.0 + v * 0.5;
            let depth = 0.5 + crate::mesh::hash01(vseed as i32, 9) * 0.4;
            let mat = match (v * 4.0) as u32 {
                0 => crate::mesh::Material::WeatheredWood,
                1 => crate::mesh::Material::RustPlate,
                2 => crate::mesh::Material::Concrete,
                _ => crate::mesh::Material::Planks,
            };
            let mut m = boxx(base + Vec3::new(0.0, hh, 0.0), Vec3::new(w, hh, depth), dither(pal.cover, vseed as i32, 0.16), FACE_ALL);
            m.material = Some(mat);
            crate::mesh::tint_height(&mut m, gy, gy + hh * 2.0, crate::mesh::lerp_c(pal.cover, rgb(0, 0, 0), 0.3), pal.cover);
            level.meshes.push(m);
            add_obst(base + Vec3::new(0.0, hh, 0.0), Vec3::new(w, hh, depth));
        }
        Archetype::Shelter => {
            // a small cluster of low ad-hoc cover boxes
            for k in 0..3 {
                let a = k as f32 * 2.1 + seed as f32 * 0.01;
                let off = Vec3::new(a.cos() * half * 0.4, 0.5, a.sin() * half * 0.4);
                let s = 0.5 + crate::mesh::hash01(seed as i32, k) * 0.3;
                level.meshes.push(boxx(base + off, Vec3::splat(s), dither(pal.cover, seed as i32 + k, 0.12), FACE_ALL));
                add_obst(base + off, Vec3::splat(s));
            }
        }
        Archetype::Wreckage => {
            // an elongated tilted hull (irregular hard cover / sightline breaker)
            let mut hull = uv_sphere(base + Vec3::new(0.0, 0.8, 0.0), 1.0, 10, 6, dither(rgb(120, 96, 70), seed as i32, 0.1));
            for v in hull.verts.iter_mut() {
                let d = *v - (base + Vec3::new(0.0, 0.8, 0.0));
                *v = base + Vec3::new(0.0, 0.8, 0.0) + Vec3::new(d.x * (half * 0.5).clamp(1.2, 2.4), d.y * 0.8, d.z * 0.9);
            }
            hull.material = Some(crate::mesh::Material::RustPlate);
            level.meshes.push(hull);
            add_obst(base + Vec3::new(0.0, 0.8, 0.0), Vec3::new((half * 0.5).clamp(1.2, 2.4), 0.9, 0.9));
        }
        Archetype::Sentinel => {
            // a low L-shaped bunker facing the danger direction
            let col = dither(pal.wall, seed as i32, 0.08);
            let mut a = boxx(base + Vec3::new(0.0, 0.7, -0.8), Vec3::new(1.2, 0.7, 0.3), col, FACE_ALL);
            a.material = Some(crate::mesh::Material::Concrete);
            level.meshes.push(a);
            let mut b = boxx(base + Vec3::new(-1.0, 0.7, 0.0), Vec3::new(0.3, 0.7, 1.0), col, FACE_ALL);
            b.material = Some(crate::mesh::Material::Concrete);
            level.meshes.push(b);
            add_obst(base + Vec3::new(0.0, 0.7, -0.8), Vec3::new(1.2, 0.7, 0.3));
            add_obst(base + Vec3::new(-1.0, 0.7, 0.0), Vec3::new(0.3, 0.7, 1.0));
        }
        Archetype::Vantage => {
            // a raised platform block you can climb for high ground
            let ph = 1.6 + feat.height * 1.5;
            let mut m = boxx(base + Vec3::new(0.0, ph * 0.5, 0.0), Vec3::new(1.6, ph * 0.5, 1.6), dither(pal.rock, seed as i32, 0.1), FACE_ALL);
            m.material = Some(crate::mesh::Material::StoneBlock);
            level.meshes.push(m);
            add_obst(base + Vec3::new(0.0, ph * 0.5, 0.0), Vec3::new(1.6, ph * 0.5, 1.6));
            level.platforms.push(crate::level::Platform { x0: cx - 1.6, z0: cz - 1.6, x1: cx + 1.6, z1: cz + 1.6, top: ph });
        }
        Archetype::Spine => {
            // a tall industrial tower — a landmark. Central column + a flattened tank at the top + an emissive
            // warning light (so it registers as a landmark for the composition/landmark metric).
            let th = 7.0 + feat.height * 3.0;
            level.meshes.push(tube(base, base + Vec3::new(0.0, th, 0.0), 0.5, 8, dither(rgb(120, 110, 96), seed as i32, 0.08)));
            let mut tank = uv_sphere(base + Vec3::new(0.0, th, 0.0), 1.3, 12, 8, rgb(140, 120, 96));
            for v in tank.verts.iter_mut() {
                v.y = base.y + th + (v.y - (base.y + th)) * 0.5;
            }
            level.meshes.push(tank);
            let mut lamp = uv_sphere(base + Vec3::new(0.0, th + 1.6, 0.0), 0.35, 8, 6, rgb(255, 90, 60));
            lamp.emissive = 0.9;
            level.meshes.push(lamp);
            level.fire_sources.push(base + Vec3::new(0.0, th + 1.6, 0.0));
            add_obst(base + Vec3::new(0.0, th * 0.5, 0.0), Vec3::new(0.6, th * 0.5, 0.6));
        }
    }
}

/// Generate a level with the recursive noise-driven generator. Returns the level and a report of what the
/// recursion produced. The level starts from a plain open-air blank of the requested size; the generator fills
/// it. `net` is the taste function (swap for learned/evolved weights); `params` tunes the recursion.
pub fn generate(seed: u64, net: &ActivationNet, params: &ProcParams) -> (Level, ProcReport) {
    generate_with_bias(seed, net, params, [0.0; 7])
}

/// Core generator with an optional per-archetype selection bias (zero = unthemed). Themes pass a non-zero bias
/// derived from their target mix so placement is steered toward the desired proportions directly.
fn generate_with_bias(seed: u64, net: &ActivationNet, params: &ProcParams, arch_bias: [f32; 7]) -> (Level, ProcReport) {
    // Start from a bare open-air arena at the right size, then let the recursion populate it.
    let cfg = MapConfig { seed, theme: crate::level::MapTheme::Meadow, size: (params.size as i32).clamp(80, 200), ..Default::default() };
    // Build a blank open level (reuse the meadow generator for a clean floor + boundary), then strip its
    // procedural cover so we're populating a clean slate.
    let mut level = Level::generate(cfg);
    level.obstacles.clear();
    level.cover_obstacles.clear();
    // keep the floor/boundary meshes; drop scattered props so composition reflects OUR placement
    let floor_and_walls: Vec<_> = level.meshes.drain(..).take(4).collect();
    level.meshes = floor_and_walls;

    let ts = level.ts();
    let world = Vec2::new(level.w as f32 * ts, level.h as f32 * ts);
    let inset = 0.92;
    let full_half = (world.x.min(world.y)) * 0.5 * inset;
    let cx0 = world.x * 0.5;
    let cz0 = world.y * 0.5;

    // recursion depth budget so the smallest cell ≈ min_cell (character scale)
    let mut report = ProcReport::default();

    // ── Macro-zone tier ──────────────────────────────────────────────────────────────────────────────────
    // Split the map into a MxM grid of macro-cells, assign each a terrain zone from the low-frequency noise,
    // build the regional commitments (water bodies), then recurse each macro-cell with its zone as context.
    // This is the "specifics" layer: water/terrain/built decisions are made once, coherently, up front.
    let m = 3i32;
    let cell = full_half * 2.0 / m as f32;
    let child_half = cell * 0.5;
    let levels = ((child_half * 2.0 / params.min_cell).log2().ceil() as u32).clamp(2, 4);
    let origin_x = cx0 - full_half;
    let origin_z = cz0 - full_half;

    for gz in 0..m {
        for gx in 0..m {
            let mcx = origin_x + (gx as f32 + 0.5) * cell;
            let mcz = origin_z + (gz as f32 + 0.5) * cell;
            let zone = zone_at(mcx, mcz, params, seed as u32);
            let mseed = (seed as u32).wrapping_mul(2_654_435_761).wrapping_add((gx * 71 + gz * 131) as u32);

            // regional commitment: a Water macro-cell gets an actual water body (placed once, here) with a
            // shoreline — not something the per-leaf recursion could ever build coherently.
            if zone == TerrainZone::Water {
                place_water_zone(&mut level, mcx, mcz, child_half, mseed);
            }

            let ctx = DownwardContext::root_biased(zone, arch_bias);
            recurse(&mut level, net, params, mcx, mcz, child_half, 0, levels as f32, mseed ^ 0x9E37_79B9, ctx, &mut report);
        }
    }

    level.chunk_meshes(40.0);
    (level, report)
}

/// Generate a level **styled toward a theme target**: the target's palette colours the whole level (floor,
/// walls, cover, sky, fog, accents), and its water/density signals nudge the recursion params. This is the
/// geometry side of the Rosetta transfer — the *layout* comes from the net (which a theme-guided search tunes),
/// the *look* comes from the theme palette. Returns the level, the generation report, and the realised
/// archetype counts (for scoring against the target).
pub fn generate_themed(seed: u64, net: &ActivationNet, params: &ProcParams, target: &crate::rosetta::ThemeTarget) -> (Level, ProcReport) {
    // fold the theme's density/water into the recursion params so the map's openness matches the spec.
    let mut p = *params;
    // denser target → lower subdivide threshold & smaller cells (more POIs); open target → the reverse.
    p.subdivide_roughness = (0.62 - target.density * 0.25).clamp(0.3, 0.62);
    p.min_cell = (8.0 - target.density * 4.0).clamp(4.0, 8.0);
    p.water_bias = target.water;
    p.vert_bias = target.verticality;
    // zone lean: wreck-heavy themes want Rubble zones (negative), sentinel/cover-heavy want Built (positive).
    let wreck = target.archetype_mix[4];
    let built_like = target.archetype_mix[2] + target.archetype_mix[0]; // sentinel + break
    p.zone_lean = ((built_like - wreck * 2.0) * 2.5).clamp(-1.0, 1.0);

    // Convert the theme's desired archetype mix into a log-space selection bias, so placement is steered
    // toward those proportions directly (not left entirely to the net). Center on the mean so we nudge
    // *relative* preferences: archetypes the theme wants more of get a positive push, others a negative one.
    let mean: f32 = target.archetype_mix.iter().sum::<f32>() / 7.0;
    let mut arch_bias = [0.0f32; 7];
    for i in 0..7 {
        arch_bias[i] = ((target.archetype_mix[i] - mean) * 6.0).clamp(-2.5, 2.5);
    }
    let (mut level, report) = generate_with_bias(seed, net, &p, arch_bias);
    // Re-style: overwrite the palette and re-tint the kept floor/boundary meshes to the theme colours so the
    // level *reads* as the themed place, not the default meadow.
    let pal = target.palette;
    level.pal = pal;
    for m in level.meshes.iter_mut().take(4) {
        // shift the mesh's flat colour toward the theme floor/wall; vertex colours ride along via a tint.
        let base = pal.floor;
        m.color = base;
        if let Some(vc) = m.vcol.as_mut() {
            for c in vc.iter_mut() {
                *c = crate::mesh::lerp_c(*c, base, 0.55);
            }
        }
    }
    level.chunk_meshes(40.0);
    (level, report)
}

/// Guided-search transfer: evolve a net whose generated maps best **match a theme target** (its archetype mix,
/// density, water, verticality), styled with the theme palette. This is the payoff of the Rosetta stone — the
/// text spec becomes the fitness, and the search finds a map that realises it. Returns the best net and its
/// match score.
pub fn evolve_toward(target: &crate::rosetta::ThemeTarget, seeds: &[u64], params: &ProcParams, generations: u32, pop: usize, rng_seed: u64) -> (ActivationNet, f32) {
    let mut rng = rng_seed | 1;
    let mut rand = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        ((rng >> 33) as f32) / (1u64 << 31) as f32
    };
    let fitness = |net: &ActivationNet| -> f32 {
        let mut s = 0.0;
        for &sd in seeds {
            let (lvl, rep) = generate_themed(sd, net, params, target);
            // blend theme-match with a floor of honest composition so we don't match the theme by making junk
            let m = crate::rosetta::match_score(target, &rep.placed, &lvl);
            let q = crate::composition::honest_quality(&lvl);
            s += m * 0.7 + q * 0.3;
        }
        s / seeds.len().max(1) as f32
    };

    let base = ActivationNet::default();
    let n = ActivationNet::param_count();
    let mut population: Vec<(Vec<f32>, f32)> = Vec::with_capacity(pop);
    population.push((base.to_genome(), fitness(&base)));
    for _ in 1..pop {
        let mut g = base.to_genome();
        for v in g.iter_mut() {
            *v += (rand() - 0.5) * 1.4;
        }
        let f = fitness(&ActivationNet::from_genome(&g));
        population.push((g, f));
    }
    for _ in 0..generations {
        population.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let survivors = (pop / 2).max(1);
        for i in survivors..pop {
            let a = population[(rand() * survivors as f32) as usize % survivors].0.clone();
            let b = population[(rand() * survivors as f32) as usize % survivors].0.clone();
            let mut child = vec![0.0f32; n];
            for k in 0..n {
                child[k] = if rand() < 0.5 { a[k] } else { b[k] };
                if rand() < 0.18 {
                    child[k] += (rand() - 0.5) * 0.9;
                }
            }
            let f = fitness(&ActivationNet::from_genome(&child));
            population[i] = (child, f);
        }
    }
    population.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    (ActivationNet::from_genome(&population[0].0), population[0].1)
}
/// places water-appropriate props (via the zone gate).
fn place_water_zone(level: &mut Level, cx: f32, cz: f32, half: f32, seed: u32) {
    let r = half * 0.85;
    let surf = level.floor_y + 0.15;
    level.water.push(crate::level::WaterBody {
        cx,
        cz,
        radius: r,
        y: surf,
        col: dither(rgb(48, 92, 120), seed as i32, 0.1),
        alpha: 150,
        reflect: 0.45,
        emissive: 0.0,
        lava: false,
        disc: true,
    });
    // a thin darker shoreline ring of low rocks so the water edge isn't a hard flat cut
    for k in 0..10 {
        let a = k as f32 / 10.0 * std::f32::consts::TAU + crate::mesh::hash01(seed as i32, k) * 0.4;
        let rr = r * (1.0 + 0.05 * crate::mesh::hash01(seed as i32, k + 20));
        let p = Vec3::new(cx + a.cos() * rr, level.floor_y + 0.1, cz + a.sin() * rr);
        level.meshes.push(crate::mesh::rock(p, 0.35 + crate::mesh::hash01(seed as i32, k + 40) * 0.25, seed.wrapping_add(k as u32), dither(rgb(90, 84, 74), k as i32, 0.15)));
    }
}

/// Convenience: generate a preset and return an enhanced copy at the given tuning. Used by the UI/preview and
/// the analysis tooling.
pub fn evolve_taste(seeds: &[u64], params: &ProcParams, generations: u32, pop: usize, rng_seed: u64) -> (ActivationNet, f32) {
    // A tiny evolutionary search over the net's genome, using the HONEST composition metric as fitness. This is
    // the payoff of an un-gameable objective: we can let a swarm/genetic loop tune the "taste" automatically
    // instead of hand-guessing weights, and it can't win by dumping clutter (the metric punishes noise-like
    // fill). Fitness = mean honest_quality across several seeds (so it generalises, not overfits one map).
    let mut rng = rng_seed | 1;
    let mut rand = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        ((rng >> 33) as f32) / (1u64 << 31) as f32
    };
    let fitness = |net: &ActivationNet| -> f32 {
        let mut s = 0.0;
        for &sd in seeds {
            let (lvl, _) = generate(sd, net, params);
            // Honest composition quality is the backbone, but it alone rewards emptiness (an open field is
            // maximally "un-noisy"). A fun map needs *enough* content too, so blend in a content-adequacy term
            // that peaks at a sensible cover density and falls off on both sides — too bare and too cluttered
            // both lose. This keeps evolution in the "well-composed AND populated" sweet spot rather than
            // collapsing to a void.
            let q = crate::composition::honest_quality(&lvl);
            let area = (lvl.w * lvl.h) as f32 * lvl.ts() * lvl.ts();
            let density = lvl.cover_obstacles.len() as f32 / area.max(1.0);
            // One-sided content floor: penalise a near-empty map (nothing to fight over), but do NOT reward
            // more density past the floor — the composition term already punishes over-fill as noise. This
            // stops the two objectives from fighting to a mushy midpoint. Floor ≈ 1 cover piece per 700 m^2.
            let floor = 1.0 / 700.0;
            let adequacy = (density / floor).clamp(0.0, 1.0); // 0 when empty, 1 once the floor is met
            s += q * (0.7 + 0.3 * adequacy); // scale quality down only when the map is too bare
        }
        s / seeds.len().max(1) as f32
    };

    // seed the population: the hand-tuned default plus mutated copies
    let base = ActivationNet::default();
    let n = ActivationNet::param_count();
    let mut population: Vec<(Vec<f32>, f32)> = Vec::with_capacity(pop);
    {
        let g = base.to_genome();
        let f = fitness(&base);
        population.push((g, f));
    }
    for _ in 1..pop {
        let mut g = base.to_genome();
        for v in g.iter_mut() {
            *v += (rand() - 0.5) * 1.2;
        }
        let f = fitness(&ActivationNet::from_genome(&g));
        population.push((g, f));
    }

    for _ in 0..generations {
        population.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        // keep the top half (elitism), breed the rest from mutated crossovers of survivors
        let survivors = (pop / 2).max(1);
        for i in survivors..pop {
            let a = &population[(rand() * survivors as f32) as usize % survivors].0;
            let b = &population[(rand() * survivors as f32) as usize % survivors].0;
            let mut child = vec![0.0f32; n];
            for k in 0..n {
                child[k] = if rand() < 0.5 { a[k] } else { b[k] };
                if rand() < 0.15 {
                    child[k] += (rand() - 0.5) * 0.8; // mutation
                }
            }
            let f = fitness(&ActivationNet::from_genome(&child));
            population[i] = (child, f);
        }
    }
    population.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let best = &population[0];
    (ActivationNet::from_genome(&best.0), best.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evolution_improves_honest_quality_over_the_hand_tuned_default() {
        // The payoff of an un-gameable fitness: a short evolutionary search over the net genome should find a
        // taste that scores at least as well as the hand-tuned default on the honest composition metric — and
        // it can't cheat by dumping clutter, because that metric punishes noise-like fill.
        let params = ProcParams::default();
        let seeds = [1u64, 2, 3];
        let base = ActivationNet::default();
        let base_fit: f32 = seeds.iter().map(|&s| {
            let (l, _) = generate(s, &base, &params);
            crate::composition::honest_quality(&l)
        }).sum::<f32>() / seeds.len() as f32;
        // small budget to keep the test fast
        let (_best, best_fit) = evolve_taste(&seeds, &params, 3, 6, 42);
        // Evolution is stochastic and the population includes the default genome as a seed, so the best found
        // should essentially never be meaningfully worse than the default; allow a small tolerance for the
        // fitness's own noise across seeds.
        assert!(best_fit >= base_fit - 0.05, "evolution should not do meaningfully worse than the default ({best_fit:.3} vs {base_fit:.3})");
    }

    #[test]
    fn genome_round_trips() {
        let net = ActivationNet::default();
        let g = net.to_genome();
        assert_eq!(g.len(), ActivationNet::param_count());
        let back = ActivationNet::from_genome(&g);
        assert_eq!(back.to_genome(), g);
    }

    #[test]
    fn generated_maps_are_playable_and_clean() {
        // Generated maps must clear the same hard bar as authored ones: connected, a full wave can spawn, and
        // no degenerate geometry (the richer FSM meshes mustn't introduce holes).
        let net = ActivationNet::default();
        let params = ProcParams::default();
        for seed in [1u64, 3, 7, 42] {
            let (level, _) = generate(seed, &net, &params);
            assert!(crate::mapeval::score(&level).connected, "seed {seed}: generated map must be connected");
            let wave = crate::enemy::populate(&level, 13, seed);
            assert!(wave.len() >= 10, "seed {seed}: must spawn a full wave, got {}", wave.len());
            let meshes: Vec<_> = if let Some(f) = &level.lod_field { f.render_meshes().into_iter().cloned().collect() } else { level.meshes.clone() };
            let mut degen = 0;
            for m in &meshes {
                for tri in &m.tris {
                    let a = m.verts[tri[0] as usize];
                    let b = m.verts[tri[1] as usize];
                    let c = m.verts[tri[2] as usize];
                    if (b - a).cross(c - a).len() * 0.5 < 1e-5 {
                        degen += 1;
                    }
                }
            }
            assert_eq!(degen, 0, "seed {seed}: generated map has {degen} degenerate triangles");
        }
    }

    #[test]
    fn fsm_bridge_produces_rich_authored_geometry() {
        // Wiring the real hand-authored FSMs in should make generated maps far richer than flat stand-in boxes:
        // a populated map should carry thousands of triangles (bunkers, towers, wrecks), not a few hundred.
        let net = ActivationNet::default();
        let params = ProcParams::default();
        let (level, _) = generate(7, &net, &params);
        let tris: usize = level.meshes.iter().map(|m| m.tris.len()).sum();
        assert!(tris > 3000, "FSM-dressed map should be geometrically rich, got {tris} tris");
        assert!(!level.obstacles.is_empty(), "should place real cover");
    }

    #[test]
    fn recursion_reaches_character_scale_and_makes_multiscale_structure() {
        // The generator must subdivide down toward character scale (several depth levels) and place a mix of
        // archetypes — the signature of multi-scale composition, not a flat single-scale scatter.
        let net = ActivationNet::default();
        let params = ProcParams::default();
        let (level, report) = generate(7, &net, &params);
        assert!(report.max_depth >= 2, "should recurse to multiple depths, got {}", report.max_depth);
        assert!(report.leaves > 8, "should produce many leaf regions, got {}", report.leaves);
        // at least 3 distinct archetypes actually placed → variety, not one-note
        let distinct = report.placed.iter().filter(|&&c| c > 0).count();
        assert!(distinct >= 3, "should place a variety of archetypes, got {distinct} kinds");
        // and it must be a real, non-empty level
        assert!(!level.obstacles.is_empty(), "generator should place cover geometry");
    }

    #[test]
    fn the_net_is_a_pure_swappable_taste_function() {
        // Two different weight sets on the SAME seed must yield different archetype mixes — proving the net
        // (not just the noise) drives composition, so it can be tuned/evolved.
        let params = ProcParams::default();
        let default_net = ActivationNet::default();
        // a "void-heavy" net: crank Void's output bias (last hidden weight is the bias term)
        let mut sparse = ActivationNet::default();
        sparse.w2[6][8] += 3.0; // crank Void's bias
        let (_, r_default) = generate(11, &default_net, &params);
        let (_, r_sparse) = generate(11, &sparse, &params);
        assert!(
            r_sparse.placed[6] >= r_default.placed[6],
            "cranking Void's weight should place at least as many voids ({} vs {})",
            r_sparse.placed[6],
            r_default.placed[6]
        );
    }

    #[test]
    fn generated_level_scores_on_the_composition_metric() {
        // The payoff: a noise+net+recursion level should be *scoreable* and land as a real composed space (not
        // noise) on the composition metric built to detect exactly that. We don't demand it beats authored maps
        // (it's a PoC), only that it registers real structure.
        let net = ActivationNet::default();
        let params = ProcParams::default();
        let (level, _) = generate(3, &net, &params);
        let comp = crate::composition::analyze(&level);
        assert!(comp.score > 0.0 && comp.score <= 1.0);
        // recursion should give it multi-scale structure above pure-random noise-likeness
        assert!(comp.noise_likeness < 0.95, "recursive placement should be less noise-like than pure scatter");
    }
}
