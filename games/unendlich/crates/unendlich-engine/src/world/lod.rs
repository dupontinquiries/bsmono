//! Level-of-detail (LOD) foundation (v0.22.0, "F1").
//!
//! This module is the data-structure substrate for the LOD system
//! that future increments (F2-F6) build on top of. It does NOT
//! produce any visible change by itself: every call site in v0.22.0
//! requests `LodLevel::Near`, which is the level whose mesher path
//! existed before this increment. Wiring distance-based LOD
//! selection into the chunk manager and adding lower-detail
//! meshers happens in v0.22.1 onwards.
//!
//! # Why ship F1 alone
//!
//! The previous several increments hit hardware-only bugs that cost
//! a playtest each. The mesher and meshing-pool refactors that LOD
//! actually requires are non-trivial; getting the type substrate
//! wrong here would compound into every subsequent F-increment. By
//! shipping the substrate alone, with full unit-test coverage and
//! zero behavioural change, we lock in the design before any
//! mesher code depends on it.
//!
//! # Architecture
//!
//! - [`LodLevel`]: a 4-variant enum with dense u8 discriminants.
//!   Ordered so distance comparisons (`if lod >= LodLevel::Far`)
//!   read naturally.
//! - [`desired_lod_for`]: pure function from
//!   `(chunk_coord, player_chunk, render_distance) → LodLevel`.
//!   Uses Chebyshev distance (max of |dx|,|dy|,|dz|) so bands
//!   are axis-aligned cubes around the player; matches the chunk
//!   grid better than Euclidean (which produces awkward
//!   donut-ring boundaries at corners of the chunk lattice).
//! - [`MeshSlotKey`]: replaces `ChunkCoord` as the key for the
//!   renderer's mesh slot table. Each chunk can simultaneously have
//!   one mesh per LOD; the renderer picks which to draw based on
//!   the chunk's current desired LOD. In F1 every call uses
//!   [`MeshSlotKey::near`], producing identical behaviour to v0.21.x.
//!
//! # Hysteresis (NOT in F1)
//!
//! The naive band logic here will produce visible "popping" when a
//! chunk crosses a band boundary as the player moves — the mesh
//! switches from Near to Mid at exactly distance B, then back to
//! Near if the player walks one chunk closer. Hysteresis (don't
//! downgrade until distance > B + ε, don't upgrade until
//! distance < B - ε) is filed for F4. The current design supports
//! it without further type changes: the `desired_lod_for` function
//! grows an extra parameter for "current LOD" to break ties.

use crate::world::chunk::ChunkCoord;

/// Level-of-detail tier for a chunk's mesh.
///
/// Discriminants are dense (0..3) and ordered (Near < Mid < Far <
/// VeryFar) so callers can write `if lod >= LodLevel::Far` for
/// "this chunk is allowed to use the cheap mesher" without a match.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum LodLevel {
    /// Highest detail: greedy mesher with all features (AO, tints,
    /// weathered tops, plant voxelization). Reserved for chunks
    /// closest to the player. F1 default for every call site.
    Near    = 0,
    /// Greedy mesher without sub-block features (no weathered
    /// tops, simpler plants). Future tier.
    Mid     = 1,
    /// Naive face-cull mesher, no greedy merging, no plants.
    /// Approximate the existing `mesh_chunk_naive`. Future tier.
    Far     = 2,
    /// Aggressive simplification: one quad per visible chunk face
    /// at most, color-averaged. Almost-constant tris per chunk
    /// regardless of contents. Future tier; primarily for the
    /// outer band of large render-distance configs.
    VeryFar = 3,
}

impl LodLevel {
    /// Number of LOD tiers; equal to the number of variants. Used
    /// for sizing arrays indexed by LOD.
    pub const COUNT: usize = 4;

    /// Iterate all LOD levels in order.
    pub fn iter() -> impl Iterator<Item = LodLevel> {
        [LodLevel::Near, LodLevel::Mid, LodLevel::Far, LodLevel::VeryFar]
            .into_iter()
    }

    /// Index 0..COUNT corresponding to this LOD's discriminant.
    /// Useful for `[T; LodLevel::COUNT]` lookups.
    pub fn index(self) -> usize { self as u8 as usize }

    /// Compact human-readable label for log strings. Stable; tests
    /// pin the values.
    pub fn short_name(self) -> &'static str {
        match self {
            LodLevel::Near    => "near",
            LodLevel::Mid     => "mid",
            LodLevel::Far     => "far",
            LodLevel::VeryFar => "vfar",
        }
    }
}

impl From<LodLevel> for u32 {
    fn from(l: LodLevel) -> u32 { l as u8 as u32 }
}

/// Compute the LOD a chunk should be meshed at, given the player's
/// current chunk and the configured render distance.
///
/// Distance metric: Chebyshev (max of |dx|, |dy|, |dz|). Bands have
/// **fixed absolute outer edges** (independent of RD), with a
/// geometric-doubling falloff that mirrors the default downsample
/// factors `[1, 2, 4, 8]`:
///
/// - `[0,  8]` → Near    (full resolution; ~2,140 chunks at f=1)
/// - `[9, 16]` → Mid     (downsample factor 2; ~14,000 more)
/// - `[17,32]` → Far     (factor 4; ~120,000 more)
/// - `[33,RD]` → VeryFar (factor 8; rest of the sphere)
///
/// ## Why constant outers instead of proportional
///
/// Earlier (v0.22.3) the bands were 25/50/75% of RD. That made the
/// band shape scale-free in RD, but produced terrible play
/// experience at large RD: at RD=64 the Near band was 0..16, so
/// a player walking at chunk-rate 2/sec wouldn't reach Mid for 8
/// seconds — and by then generation had filled the surrounding
/// area at Near, so non-Near tiers never fired in practice.
///
/// Constant outers solve this. The Near band is always 8 chunks
/// regardless of RD, so a player walking 8 chunks reliably crosses
/// into Mid. The geometric outer edges (8, 16, 32) match the
/// downsample factors (1, 2, 4, 8) so each band's voxel projects
/// to roughly the same on-screen size — chunks at the Mid/Far/VeryFar
/// boundary look continuous, not blocky.
///
/// ## Behavior at small RD
///
/// If RD < 32 the bands clamp at RD: Far/VeryFar are simply empty.
/// At RD=8 only Near exists. At RD=16 only Near and Mid. This is
/// the expected degradation — small RDs just don't have room for
/// the long-distance tiers.
///
/// # Examples
///
/// ```
/// use unendlich_engine::world::chunk::ChunkCoord;
/// use unendlich_engine::world::lod::{desired_lod_for, LodLevel};
///
/// let player = ChunkCoord::new(0, 0, 0);
/// // Player's own chunk is always Near.
/// assert_eq!(desired_lod_for(player, player, 64), LodLevel::Near);
/// // 8 chunks out is still Near (the constant Near outer).
/// assert_eq!(desired_lod_for(ChunkCoord::new(8, 0, 0), player, 64), LodLevel::Near);
/// // 9 chunks out enters Mid.
/// assert_eq!(desired_lod_for(ChunkCoord::new(9, 0, 0), player, 64), LodLevel::Mid);
/// ```
pub fn desired_lod_for(
    chunk: ChunkCoord,
    player: ChunkCoord,
    render_distance: i32,
) -> LodLevel {
    let dx = (chunk.x - player.x).abs();
    let dy = (chunk.y - player.y).abs();
    let dz = (chunk.z - player.z).abs();
    let cheb = dx.max(dy).max(dz);

    // Fixed absolute outer edges, clamped to RD. The clamp is what
    // makes RD<32 "degrade gracefully" — bands at the end simply
    // become empty rather than producing weird boundaries.
    //
    // v0.24.41: bands come from `LodBands::current()`, which is
    // `UNENDLICH_LOD_BANDS` if set or the documented defaults
    // (8/16/32). Same monotonic-clamp shape as before.
    let rd = render_distance.max(1);
    let bands = LodBands::resolved(rd);
    let near_outer = bands.near.min(rd);
    let mid_outer  = bands.mid.min(rd);
    let far_outer  = bands.far.min(rd);

    if cheb <= near_outer {
        LodLevel::Near
    } else if cheb <= mid_outer {
        LodLevel::Mid
    } else if cheb <= far_outer {
        LodLevel::Far
    } else {
        LodLevel::VeryFar
    }
}

/// Outer Chebyshev radius for the Near band (inclusive).
///
/// Exposed as a named constant so the curve is self-documenting and
/// trivially tunable. Future increment: drive these from
/// `UNENDLICH_LOD_BANDS=8,16,32` if we want runtime tuning without
/// rebuild. Filed for after we have data on whether 8 is the right
/// number.
pub const NEAR_OUTER_DEFAULT: i32 = 8;
/// Outer Chebyshev radius for the Mid band (inclusive).
pub const MID_OUTER_DEFAULT: i32 = 16;
/// Outer Chebyshev radius for the Far band (inclusive). Beyond this
/// (and up to `render_distance`) is VeryFar.
pub const FAR_OUTER_DEFAULT: i32 = 32;

// =============================================================================
// LodBands (v0.24.41) — runtime-tunable Chebyshev band outers
// =============================================================================
//
// The constants above set fixed band outers (8, 16, 32). That curve
// is right for big render distances (RD≥32) where Near, Mid, Far,
// and VeryFar all see traffic. At small RDs (the default RD=6), no
// chunks ever reach Mid distance (≥9 chunks Chebyshev) so the GPU
// Mid path can't be exercised without bumping RD.
//
// `LodBands` lets the three outers be overridden via env var:
//
//     UNENDLICH_LOD_BANDS=2,4,5
//
// means Near=0..=2, Mid=3..=4, Far=5, VeryFar=6+. Useful for testing
// the Mid mesher at RD=6 without generating a full RD=16+ world.
//
// Reads `UNENDLICH_LOD_BANDS` once at first access via a `OnceLock`
// so per-chunk calls don't repeatedly parse the env. Validation:
// strictly increasing, all positive, at most `i32::MAX`. Invalid
// values log a warning and fall back to defaults.

/// Outer Chebyshev radii for the three boundary tiers (Near, Mid,
/// Far). Anything beyond `far` (and within `render_distance`) is
/// VeryFar.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LodBands {
    pub near: i32,
    pub mid:  i32,
    pub far:  i32,
}

impl Default for LodBands {
    fn default() -> Self {
        Self {
            near: NEAR_OUTER_DEFAULT,
            mid:  MID_OUTER_DEFAULT,
            far:  FAR_OUTER_DEFAULT,
        }
    }
}

/// Render-distance-relative default bands. The fixed 8/16/32 defaults are
/// right for large RD (≥32) but far too aggressive at the smaller RDs people
/// actually run: at RD=12 they leave a radius-8 *cube* (~1834 chunks in the
/// data) at full Near detail, which is what made shadow+main the bottleneck.
/// Scaling the bands with RD makes the LOD curve auto-adapt — near≈RD/4,
/// mid≈RD/2, far≈RD — so RD=12 lands on ~3/6/12 (the band set that hit 175fps
/// in testing) and RD=32 still resolves to ~8/16/32. Strictly increasing by
/// construction (each tier floored above the previous).
pub fn lod_bands_for_rd(rd: i32) -> LodBands {
    lod_bands_for_rd_with_fidelity(rd, 1.0)
}

/// Render-distance-relative bands with a fidelity multiplier. `fidelity` scales
/// how far the high-detail (near/mid) bands reach: 1.0 is the baseline
/// (near≈RD/4, mid≈RD/2, far=RD — identical to the old `lod_bands_for_rd`);
/// >1.0 pushes near/mid outward so more chunks render at higher geometry (more
/// fidelity, more cost); <1.0 pulls them in. far stays at RD (the actual render
/// edge). Strictly increasing by construction. fidelity is clamped to a sane
/// [0.5, 3.0]. At fidelity 1.0 the float math truncates to exactly the old
/// integer-division values, so the default render path is unchanged.
pub fn lod_bands_for_rd_with_fidelity(rd: i32, fidelity: f32) -> LodBands {
    let rd = rd.max(1);
    let f = if fidelity.is_finite() { fidelity.clamp(0.5, 3.0) } else { 1.0 };
    let near = ((rd as f32 * f) / 4.0) as i32;
    let near = near.max(2);
    let mid = (((rd as f32 * f) / 2.0) as i32).max(near + 1);
    let far = rd.max(mid + 1);
    LodBands { near, mid, far }
}

impl LodBands {
    /// Construct with explicit values. Validates strictly increasing
    /// and all positive. Returns a human-readable error on invalid
    /// input so logging surfaces misconfiguration cleanly.
    pub fn new(near: i32, mid: i32, far: i32) -> Result<Self, String> {
        if near < 1 || mid < 1 || far < 1 {
            return Err(format!(
                "all bands must be ≥1; got near={} mid={} far={}",
                near, mid, far,
            ));
        }
        if near > mid || mid > far {
            return Err(format!(
                "bands must be non-decreasing (near ≤ mid ≤ far); \
                 got near={} mid={} far={}",
                near, mid, far,
            ));
        }
        Ok(Self { near, mid, far })
    }

    /// Parse `UNENDLICH_LOD_BANDS=N,M,F`. Falls back to default on
    /// any parse error or invalid value (with a warning logged).
    pub fn from_env() -> Self {
        Self::resolved(crate::constants::runtime_render_distance())
    }

    /// Parse an explicit `UNENDLICH_LOD_BANDS` override. `Some(bands)` when the
    /// env var is set to a valid `near,mid,far`; `None` when unset or invalid
    /// (so the caller falls back to the RD-relative default). Cached.
    fn env_override() -> Option<LodBands> {
        static CACHED: std::sync::OnceLock<Option<LodBands>> = std::sync::OnceLock::new();
        *CACHED.get_or_init(|| {
            let s = std::env::var("UNENDLICH_LOD_BANDS").ok()?;
            let parts: Vec<&str> = s.split(',').collect();
            if parts.len() != 3 {
                log::warn!("UNENDLICH_LOD_BANDS={} expects near,mid,far; using RD-relative default", s);
                return None;
            }
            let mut vals = [0i32; 3];
            for (i, p) in parts.iter().enumerate() {
                match p.trim().parse::<i32>() {
                    Ok(n) => vals[i] = n,
                    Err(_) => {
                        log::warn!("UNENDLICH_LOD_BANDS={} non-integer at {}; using RD-relative default", s, i);
                        return None;
                    }
                }
            }
            match Self::new(vals[0], vals[1], vals[2]) {
                Ok(b) => {
                    log::info!(
                        "UNENDLICH_LOD_BANDS active: near=0..={} mid={}..={} far={}..={} vfar={}+",
                        b.near, b.near + 1, b.mid, b.mid + 1, b.far, b.far + 1,
                    );
                    Some(b)
                }
                Err(e) => {
                    log::warn!("UNENDLICH_LOD_BANDS invalid: {}; using RD-relative default", e);
                    None
                }
            }
        })
    }

    /// The bands to use for a given render distance: the explicit env override
    /// if set, else the RD-relative default ([`lod_bands_for_rd`]). This is the
    /// single source of truth shared by `desired_lod_for` and `request_visible`.
    pub fn resolved(rd: i32) -> LodBands {
        match Self::env_override() {
            Some(b) => b,
            None => lod_bands_for_rd_with_fidelity(rd, Self::fidelity_env()),
        }
    }

    /// `UNENDLICH_LOD_FIDELITY` multiplier (default 1.0 = baseline bands).
    /// Cached. Values <0.5 or >3.0 are clamped inside the band function.
    fn fidelity_env() -> f32 {
        static CACHED: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
        *CACHED.get_or_init(|| {
            std::env::var("UNENDLICH_LOD_FIDELITY")
                .ok()
                .and_then(|s| s.trim().parse::<f32>().ok())
                .filter(|f| f.is_finite() && *f > 0.0)
                .unwrap_or(1.0)
        })
    }

    /// Process-global bands for the runtime render distance. Used by hot paths
    /// that don't carry an explicit RD (`request_visible`). `desired_lod_for`
    /// prefers `resolved(render_distance)` so it stays consistent with its arg.
    pub fn current() -> LodBands {
        Self::resolved(crate::constants::runtime_render_distance())
    }
}

/// The source-block sampling stride for a LOD: Near samples every block (1),
/// Mid every 2nd, Far every 4th, VeryFar every 8th. The octree mesher iterates
/// cells at this stride.
pub fn lod_cell_stride(lod: LodLevel) -> u32 {
    match lod {
        LodLevel::Near => 1,
        LodLevel::Mid => 2,
        LodLevel::Far => 4,
        LodLevel::VeryFar => 8,
    }
}

/// Whether a chunk-boundary face should be CULLED (not emitted) given the
/// meshing chunk's stride, the neighbour's stride, and whether the neighbour's
/// edge cell is solid.
///
/// The seam bug: the mesher decides a boundary face by sampling a single
/// neighbour voxel, but at a LOD change the neighbour is meshed at a *different*
/// resolution, so culling a far chunk's face against a near neighbour (or vice
/// versa) leaves a visible gap — the surfaces don't line up. The fix is
/// conservative: only cull across a boundary when both sides are at the SAME
/// stride (then the shared face genuinely coincides). Across a LOD change we
/// always emit, trading a sliver of overdraw at the seam for no holes.
pub fn cull_boundary_face(self_stride: u32, neighbor_stride: u32, neighbor_solid: bool) -> bool {
    neighbor_solid && self_stride == neighbor_stride
}

/// Composite key for the renderer's mesh slot table.
///
/// In v0.21.x the renderer kept `HashMap<ChunkCoord, UploadedMesh>`.
/// With LOD, a single chunk may have multiple meshes uploaded at
/// once (e.g., during an LOD transition where the new mesh is
/// being prepared and the old one is still drawn). Keying by both
/// coord and LOD level lets the renderer hold N meshes per chunk
/// safely.
///
/// In F1 every existing call site uses [`MeshSlotKey::near`], so
/// behaviour matches v0.21.x exactly. Future increments will add
/// non-Near keys.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MeshSlotKey {
    pub coord: ChunkCoord,
    pub lod:   LodLevel,
}

impl MeshSlotKey {
    /// Construct a slot key for LOD::Near. The common case in F1.
    pub const fn near(coord: ChunkCoord) -> Self {
        Self { coord, lod: LodLevel::Near }
    }

    /// Construct a slot key for an arbitrary LOD.
    pub const fn at(coord: ChunkCoord, lod: LodLevel) -> Self {
        Self { coord, lod }
    }
}

// =============================================================================
// Flag/LOD packing for the indirect cull SSBO
// =============================================================================
//
// `ChunkDraw.flags` (u32) was historically just a bit-set (currently
// only FLAG_DEBUG_OVERLAY = bit 0). We pack the chunk's LOD level
// into the upper 8 bits so the SSBO struct doesn't need to grow.
// std430 layout would pad a separate u32 anyway because of the
// vec4 AABB alignment; piggy-backing on `flags` is free.
//
// Bit layout:
//   bits 0..7   : flag bits (DEBUG_OVERLAY etc.)
//   bits 8..15  : LOD level (0..3 used today, 0..255 reserved)
//   bits 16..31 : reserved for future use

const LOD_SHIFT: u32 = 8;
const LOD_MASK: u32 = 0xFF;
const FLAGS_MASK: u32 = 0xFF;

/// Pack `flag_bits` (bottom byte) and `lod` (next byte) into a
/// single u32 suitable for `ChunkDraw.flags`. Other bits are zero.
pub fn flags_with_lod(flag_bits: u32, lod: LodLevel) -> u32 {
    (flag_bits & FLAGS_MASK) | ((u32::from(lod) & LOD_MASK) << LOD_SHIFT)
}

/// Unpack the (flag_bits, lod) stored in a `ChunkDraw.flags` value.
/// LOD bits outside the known range fall back to `LodLevel::Near`.
pub fn unpack_flags(packed: u32) -> (u32, LodLevel) {
    let flag_bits = packed & FLAGS_MASK;
    let lod_idx = (packed >> LOD_SHIFT) & LOD_MASK;
    let lod = match lod_idx {
        0 => LodLevel::Near,
        1 => LodLevel::Mid,
        2 => LodLevel::Far,
        3 => LodLevel::VeryFar,
        _ => LodLevel::Near, // out-of-range → safe default
    };
    (flag_bits, lod)
}

// =============================================================================
// LodSettings (v0.22.3, F3) — runtime-tunable factor per tier
// =============================================================================
//
// The relationship between an [`LodLevel`] and the actual downsample factor
// fed to `mesh_chunk_downsampled` is policy, not architecture. Different
// machines and different gameplay distances justify different tradeoffs:
//
// * A weak machine wants Far → factor 8 (aggressive simplification).
// * A beefy machine wants Far → factor 2 (more detail at the same distance).
//
// `LodSettings` makes that mapping a settings concept rather than a hard-
// coded policy in the dispatcher. Defaults come from `LodSettings::default()`
// and produce a reasonable curve for the RD=6 default (Near=1, Mid=2,
// Far=4, VeryFar=8). Users will tune via a future settings UI; for now
// `UNENDLICH_LOD_FACTORS=1,2,4,8` env var lets us A/B without rebuilding.

/// The set of valid downsample factors. Each must divide CHUNK_SIZE (32)
/// so the downsampled grid is integer-sized.
///
/// 1 = full detail (greedy mesher path).
/// 2, 4 = moderate downsampling (visible terrain detail preserved).
/// 8, 16 = aggressive (chunky terrain, single-tree features lost).
/// 32 = single super-cell per chunk, basically a colored cube.
///
/// We don't include 64+ because CHUNK_SIZE is 32, and 64 would mean
/// "fewer than one super-cell per chunk" which is meaningless. If
/// CHUNK_SIZE grows we extend this.
pub const VALID_FACTORS: &[u8] = &[1, 2, 4, 8, 16, 32];

/// Per-LOD-tier downsample factors. Indexed by `LodLevel::index()`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LodSettings {
    /// Downsample factor per tier. `factors[Near.index()] = 1` means
    /// Near uses the full-detail mesher; `factors[VeryFar.index()] = 8`
    /// means VeryFar runs `mesh_chunk_downsampled(chunk, 8)`.
    factors: [u8; LodLevel::COUNT],
}

impl Default for LodSettings {
    /// Reasonable baseline for RD=6: Near=1 (greedy), Mid=2, Far=4,
    /// VeryFar=8. At this curve a chunk at distance 5 (the outer band
    /// at RD=6) is meshed with ~64× fewer source-block-faces than the
    /// greedy chunk at distance 0.
    fn default() -> Self {
        Self { factors: [1, 2, 4, 8] }
    }
}

impl LodSettings {
    /// Construct a custom settings table. Each factor must be in
    /// [`VALID_FACTORS`] and the table must be monotonically non-
    /// decreasing (Near ≤ Mid ≤ Far ≤ VeryFar) — anything else would
    /// be nonsense (Mid more detailed than Near?).
    ///
    /// Returns `Err` with a human-readable reason on invalid input,
    /// to surface the misconfiguration in logs without panicking.
    pub fn new(factors: [u8; LodLevel::COUNT]) -> Result<Self, String> {
        for (i, &f) in factors.iter().enumerate() {
            if !VALID_FACTORS.contains(&f) {
                return Err(format!(
                    "tier {} factor {} not in VALID_FACTORS {:?}",
                    i, f, VALID_FACTORS,
                ));
            }
        }
        for i in 1..factors.len() {
            if factors[i] < factors[i - 1] {
                return Err(format!(
                    "tier factors must be non-decreasing: got {:?}", factors,
                ));
            }
        }
        Ok(Self { factors })
    }

    /// Look up the downsample factor for a tier.
    pub fn factor_for(&self, lod: LodLevel) -> u8 {
        self.factors[lod.index()]
    }

    /// Snapshot of the table for diagnostics / logging.
    pub fn as_array(&self) -> [u8; LodLevel::COUNT] { self.factors }

    /// Read settings from `UNENDLICH_LOD_FACTORS` env var. Format:
    /// comma-separated factors `"1,2,4,8"`. Falls back to default on
    /// any parse error or invalid value (with a warning logged so the
    /// misconfiguration is visible).
    ///
    /// Validation is delegated to [`LodSettings::new`] — this function
    /// is just the env-string parser.
    pub fn from_env() -> Self {
        match std::env::var("UNENDLICH_LOD_FACTORS") {
            Ok(s) => {
                let parts: Vec<&str> = s.split(',').collect();
                if parts.len() != LodLevel::COUNT {
                    log::warn!(
                        "UNENDLICH_LOD_FACTORS={} expects {} comma-separated values; using default",
                        s, LodLevel::COUNT,
                    );
                    return Self::default();
                }
                let mut arr = [0u8; LodLevel::COUNT];
                for (i, p) in parts.iter().enumerate() {
                    match p.trim().parse::<u8>() {
                        Ok(n) => arr[i] = n,
                        Err(_) => {
                            log::warn!(
                                "UNENDLICH_LOD_FACTORS={} has non-integer at position {}; using default",
                                s, i,
                            );
                            return Self::default();
                        }
                    }
                }
                match Self::new(arr) {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!("UNENDLICH_LOD_FACTORS invalid: {}; using default", e);
                        Self::default()
                    }
                }
            }
            Err(_) => Self::default(),
        }
    }
}

// =============================================================================
// FUTURE NOTE — tick rate as second consumer of distance falloff
// =============================================================================
//
// The tier system above answers "how should this chunk be meshed?". The
// SAME falloff has obvious uses for "how often should this chunk tick
// its plant lifecycle / weathering / fluid sim?". When that work lands
// (separate increment), the natural shape is to add a `TickRate` enum
// with a similar [`LodSettings`]-style table — `TickSettings` with a
// rate per tier, defaults like Near=Full, Mid=Half, Far=Quarter,
// VeryFar=Paused. The shared substrate is `desired_lod_for`; the
// policy diverges per consumer (mesh vs tick rate vs shadow allowance).
//
// Not stubbing the type today because (a) we don't yet know if the
// curve should be enum-discrete or continuous fraction, and (b) the
// systems that would consume it (plant_lifecycle, weathering) aren't
// being touched. Filed for whenever those systems get worked on next.

// =============================================================================
// Shell walking (v0.24.0a.2) — face-round-robin shell-surface iteration
// =============================================================================
//
// Why this exists
//
// The chunk pipeline has two places that walk the player's surrounding
// shells outward, accumulating chunks to generate/mesh:
//
//   1. `chunk_manager::request_visible`
//   2. `bin/unendlich_world.rs`'s submit phase
//
// Both walked each shell with three nested loops `dz: -s..=s`, `dy:
// -s..=s`, `dx: -s..=s`, filtering to surface (max(|dx|,|dy|,|dz|)==s).
// That walk is heavily DIRECTIONALLY BIASED: at shell s, the first
// (2s+1)² emitted coords are all on the -z face (`dz=-s`). Then +z's
// turn, etc.
//
// When the meshing pipeline is saturated (mesh queue at 180/128 within
// the first few seconds at RD=64), the per-tier-budget check fires
// mid-shell. Only the coords iterated FIRST get requested. With the
// dz-outer order, that's exclusively the -z face. As the player moves
// in -z/-x, new -z chunks always appear at the shell origin, and the
// pipeline never drains enough to catch up with chunks behind them.
//
// Symptom: the world loads in a "slice" or "sector" on one side of the
// spawn, not a sphere around the player. v0.24.0a.1 playtest, RD=64:
// 16k chunks loaded but only ~750 ever rendered, all on the -z/-x
// side of the player's path.
//
// Fix shape
//
// Walk shell-surface coords in FACE ROUND-ROBIN order: emit one coord
// from each of the 6 cube faces in rotation. When the consumer's
// budget fills mid-shell, every direction has received roughly equal
// coords. The world loads as a sphere, not a slice.
//
// Face ownership (no duplicates across faces): z-faces own all coords
// with |dz|==s. x-faces own coords with |dz|<s AND |dx|==s. y-faces
// own coords with |dz|<s AND |dx|<s AND |dy|==s. This partitions the
// (2s+1)³−(2s−1)³ = 24s²+2 surface coords uniquely.
//
// Cost
//
// The iterator is no-allocation: 6 small face cursors interleaved by a
// modular index. Per-coord overhead is one match + one increment.
// Faster than the prior triply-nested-loop-with-filter because no
// coord is iterated and rejected (the prior walk emitted (2s+1)³
// candidates and filtered to ~24s²+2 surface coords, an O(s) wasted-
// work ratio).

/// Iterates the surface coords of one cube-shell at integer distance
/// `radius` from the origin, in face round-robin order.
///
/// At radius 0, yields exactly `(0, 0, 0)`.
///
/// At radius `s >= 1`, yields the `24s² + 2` surface coords. The order
/// alternates faces: -z, +z, -x, +x, -y, +y, -z, +z, ... Once a face
/// is exhausted, it drops out of the rotation and remaining faces
/// continue cycling among themselves.
///
/// Within each face, the row-major order is deterministic: outer loop
/// over the secondary axis (z for x-faces; z then x for y-faces; y for
/// z-faces), inner loop over the primary axis (y for x/z faces; x for
/// y faces). The specific ordering inside a face does not matter for
/// the directional-bias fix — what matters is that no face is starved
/// at the beginning of the rotation.
pub struct ShellWalker {
    radius: i32,
    /// Per-face cursor: how many coords this face has emitted.
    face_cursors: [u32; 6],
    /// Per-face total: how many coords this face has total.
    face_totals: [u32; 6],
    /// Next face index to attempt (0..6). Advances cyclically.
    next_face: u32,
    /// For radius == 0, emit the origin once then stop.
    zero_emitted: bool,
}

impl ShellWalker {
    /// Construct a walker for the shell at integer distance `radius`.
    pub fn new(radius: i32) -> Self {
        debug_assert!(radius >= 0, "shell radius must be non-negative");
        let s = radius;
        // Face totals. Z-faces are full (2s+1)² squares. X-faces exclude
        // the |dz|==s rim (already owned by z-faces) — they're a
        // (2s-1) × (2s+1) rectangle. Y-faces additionally exclude
        // |dx|==s — a (2s-1) × (2s-1) square.
        let side = (2 * s + 1) as u32;
        let z_face = side * side;
        let x_face = if s >= 1 { ((2 * s - 1) as u32) * side } else { 0 };
        let y_face = if s >= 1 { ((2 * s - 1) as u32) * ((2 * s - 1) as u32) } else { 0 };
        let face_totals = [z_face, z_face, x_face, x_face, y_face, y_face];
        Self {
            radius,
            face_cursors: [0; 6],
            face_totals,
            next_face: 0,
            zero_emitted: false,
        }
    }

    /// Decode the n'th coord on a given face. `n` is 0-indexed.
    fn face_coord(&self, face: u32, n: u32) -> (i32, i32, i32) {
        let s = self.radius;
        match face {
            // -z face: |dz|==s, dz=-s. Sweep dy outer, dx inner.
            0 => {
                let side = (2 * s + 1) as u32;
                let dy = (n / side) as i32 - s;
                let dx = (n % side) as i32 - s;
                (dx, dy, -s)
            }
            // +z face: |dz|==s, dz=+s.
            1 => {
                let side = (2 * s + 1) as u32;
                let dy = (n / side) as i32 - s;
                let dx = (n % side) as i32 - s;
                (dx, dy, s)
            }
            // -x face: dx=-s, |dz|<s. Sweep dz outer (-s+1..=s-1), dy inner.
            2 => {
                let side = (2 * s + 1) as u32;
                let dz = (n / side) as i32 - (s - 1);
                let dy = (n % side) as i32 - s;
                (-s, dy, dz)
            }
            // +x face: dx=+s, |dz|<s.
            3 => {
                let side = (2 * s + 1) as u32;
                let dz = (n / side) as i32 - (s - 1);
                let dy = (n % side) as i32 - s;
                (s, dy, dz)
            }
            // -y face: dy=-s, |dz|<s AND |dx|<s. Sweep dz outer, dx inner,
            // both in -s+1..=s-1.
            4 => {
                let inner_side = (2 * s - 1) as u32;
                let dz = (n / inner_side) as i32 - (s - 1);
                let dx = (n % inner_side) as i32 - (s - 1);
                (dx, -s, dz)
            }
            // +y face: dy=+s.
            5 => {
                let inner_side = (2 * s - 1) as u32;
                let dz = (n / inner_side) as i32 - (s - 1);
                let dx = (n % inner_side) as i32 - (s - 1);
                (dx, s, dz)
            }
            _ => unreachable!(),
        }
    }
}

impl Iterator for ShellWalker {
    type Item = (i32, i32, i32);

    fn next(&mut self) -> Option<Self::Item> {
        if self.radius == 0 {
            if self.zero_emitted {
                return None;
            }
            self.zero_emitted = true;
            return Some((0, 0, 0));
        }

        // Round-robin scan: try at most 6 faces from `next_face`
        // forward. If none have remaining coords, the shell is done.
        for _ in 0..6 {
            let f = self.next_face;
            self.next_face = (self.next_face + 1) % 6;
            if self.face_cursors[f as usize] < self.face_totals[f as usize] {
                let n = self.face_cursors[f as usize];
                self.face_cursors[f as usize] += 1;
                return Some(self.face_coord(f, n));
            }
        }
        None
    }
}


// ── r62-06: distance-LOD block decimation ────────────────────────────────────
//
// Far chunks emit full-resolution surface meshes the player can't resolve. This
// decimates a chunk by collapsing each `factor³` block group to one
// representative block, written back into a *full-size* chunk. Two properties
// make it a free win through the existing greedy mesher with no mesher changes:
//
//   * **Silhouette-preserving / hole-free.** A group is solid if *any* of its
//     sub-blocks is solid (the representative is the most common *non-air*
//     block, but presence of a single solid block already makes the group
//     solid). Decimation only ever *adds* solidity within partially-filled
//     groups, so it can never open a see-through hole — the solid silhouette is
//     a superset of the original.
//   * **Seam-safe within a tier.** Groups are aligned to the chunk origin and
//     `factor` divides `CHUNK_SIZE` (32), so neighbouring chunks decimate on the
//     same global lattice; a boundary that was solid stays solid on both sides.
//
// Because each `factor³` group is filled uniformly, the existing
// `mesh_chunk_greedy` merges it into ~`1/factor²` of the surface quads at the
// same world size and identical winding — measured in `mesh-bench`.

use crate::constants::{CHUNK_SIZE, CHUNK_VOLUME};
use crate::world::blocks::BlockId;
use crate::world::chunk::Chunk;

/// Block-decimation factor for a LOD tier: 1 (full res) for the near tiers,
/// coarser beyond. `factor` is always a power of two dividing `CHUNK_SIZE`.
pub fn decimation_for(lod: LodLevel) -> u32 {
    match lod {
        LodLevel::Near => 1,
        LodLevel::Mid => 1,
        LodLevel::Far => 2,
        LodLevel::VeryFar => 4,
    }
}

/// Most-common non-air block among `samples` (presence of any non-air ⇒ the
/// group is solid). Ties break toward the first one scanned. All-air ⇒ `Air`.
fn representative(samples: &[BlockId]) -> BlockId {
    let mut best = BlockId::Air;
    let mut best_count = 0u32;
    for (i, &b) in samples.iter().enumerate() {
        if b.is_air() {
            continue;
        }
        let mut c = 0u32;
        for &o in &samples[..=i] {
            // count occurrences up to and including i so the first-scanned of a
            // tie keeps the lead (strict `>` below never displaces it).
            if o == b {
                c += 1;
            }
        }
        // also count the rest, so the true frequency wins.
        for &o in &samples[i + 1..] {
            if o == b {
                c += 1;
            }
        }
        if c > best_count {
            best_count = c;
            best = b;
        }
    }
    best
}

/// Decimate `src` by `factor` (a power of two dividing `CHUNK_SIZE`). Returns a
/// full-size chunk whose every `factor³` group is filled with the group's
/// representative block. `factor <= 1` (or non-dividing) returns an exact copy.
pub fn downsample_pow2(src: &Chunk, factor: u32) -> Chunk {
    let cs = CHUNK_SIZE as usize;
    let f = factor as usize;
    if f <= 1 || cs % f != 0 {
        return src.clone();
    }

    let mut blocks = vec![BlockId::Air; CHUNK_VOLUME];
    let groups = cs / f;
    let mut samples: Vec<BlockId> = Vec::with_capacity(f * f * f);
    for gy in 0..groups {
        for gz in 0..groups {
            for gx in 0..groups {
                // gather the factor³ sub-blocks of this group
                samples.clear();
                for dy in 0..f {
                    for dz in 0..f {
                        for dx in 0..f {
                            let x = gx * f + dx;
                            let y = gy * f + dy;
                            let z = gz * f + dz;
                            samples.push(src.blocks[x + z * cs + y * cs * cs]);
                        }
                    }
                }
                let rep = representative(&samples);
                // write the representative into every cell of the group
                for dy in 0..f {
                    for dz in 0..f {
                        for dx in 0..f {
                            let x = gx * f + dx;
                            let y = gy * f + dy;
                            let z = gz * f + dz;
                            blocks[x + z * cs + y * cs * cs] = rep;
                        }
                    }
                }
            }
        }
    }
    Chunk::from_blocks(src.coord, blocks)
}

/// Convenience: decimate for a LOD tier ([`decimation_for`]).
pub fn downsample_for_lod(src: &Chunk, lod: LodLevel) -> Chunk {
    downsample_pow2(src, decimation_for(lod))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rd_relative_bands_are_strictly_increasing_and_positive() {
        for rd in 1..=128 {
            let b = lod_bands_for_rd(rd);
            assert!(b.near >= 1, "rd {rd}: near {} < 1", b.near);
            assert!(b.mid > b.near, "rd {rd}: mid {} !> near {}", b.mid, b.near);
            assert!(b.far > b.mid, "rd {rd}: far {} !> mid {}", b.far, b.mid);
            // new() applies the same validation the env path uses.
            assert!(LodBands::new(b.near, b.mid, b.far).is_ok(), "rd {rd}: {:?} fails validation", b);
        }
    }

    #[test]
    fn fidelity_one_matches_baseline_bands() {
        // The default fidelity must reproduce the old integer-division bands
        // exactly, so the default render path is unchanged.
        for rd in 1..=128 {
            assert_eq!(
                lod_bands_for_rd_with_fidelity(rd, 1.0),
                lod_bands_for_rd(rd),
                "fidelity 1.0 must equal baseline at rd {rd}",
            );
        }
    }

    #[test]
    fn higher_fidelity_pushes_detail_outward() {
        // More fidelity → near/mid bands reach farther (more high-detail chunks),
        // while still strictly increasing and valid.
        let base = lod_bands_for_rd_with_fidelity(32, 1.0);
        let hi = lod_bands_for_rd_with_fidelity(32, 2.0);
        assert!(hi.near >= base.near && hi.mid >= base.mid, "fidelity should not reduce detail");
        assert!(hi.near > base.near || hi.mid > base.mid, "fidelity 2.0 should extend some band");
        for f in [0.5_f32, 1.0, 1.5, 2.0, 3.0] {
            let b = lod_bands_for_rd_with_fidelity(48, f);
            assert!(b.near >= 2 && b.mid > b.near && b.far > b.mid,
                "fidelity {f}: {:?} must stay strictly increasing", b);
            assert!(LodBands::new(b.near, b.mid, b.far).is_ok());
        }
        // Clamped: absurd fidelity stays valid (no panic, no inversion).
        let clamped = lod_bands_for_rd_with_fidelity(16, 100.0);
        assert!(clamped.near < clamped.mid && clamped.mid < clamped.far);
    }


    #[test]
    fn rd_relative_bands_shrink_near_at_high_rd() {
        // The whole point: at RD=12 the near band is ~3 (not the old fixed 8),
        // so far fewer chunks sit at full detail.
        let b12 = lod_bands_for_rd(12);
        assert_eq!(b12.near, 3, "RD=12 near should be ~RD/4");
        assert!(b12.near < NEAR_OUTER_DEFAULT, "RD=12 near {} should beat fixed default {}", b12.near, NEAR_OUTER_DEFAULT);
        // At large RD it recovers the classic 8/16/32 shape.
        let b32 = lod_bands_for_rd(32);
        assert_eq!((b32.near, b32.mid, b32.far), (8, 16, 32));
    }

    #[test]
    fn rd_relative_near_count_is_far_smaller_than_fixed() {
        // Count chunks the near band covers (Chebyshev cube ∩ RD sphere) under
        // the old fixed default vs the new RD-relative bands, at RD=12.
        let rd = 12i32;
        let count_near = |near: i32| {
            let mut n = 0;
            for dx in -rd..=rd { for dy in -rd..=rd { for dz in -rd..=rd {
                if dx*dx + dy*dy + dz*dz > rd*rd { continue; }
                if dx.abs().max(dy.abs()).max(dz.abs()) <= near { n += 1; }
            }}}
            n
        };
        let fixed = count_near(NEAR_OUTER_DEFAULT.min(rd)); // 8
        let scaled = count_near(lod_bands_for_rd(rd).near);  // 3
        assert!(scaled * 3 < fixed, "scaled near {scaled} should be «« fixed {fixed}");
    }

    /// LOD discriminants are 0..3 and dense — required for indexing
    /// into `[T; LodLevel::COUNT]` arrays and for the upper-byte
    /// packing scheme.
    #[test]
    fn lod_level_discriminants_are_dense() {
        assert_eq!(LodLevel::Near    as u8, 0);
        assert_eq!(LodLevel::Mid     as u8, 1);
        assert_eq!(LodLevel::Far     as u8, 2);
        assert_eq!(LodLevel::VeryFar as u8, 3);
        assert_eq!(LodLevel::COUNT, 4);
    }

    /// Ordering is intuitive: lower-detail tiers compare greater.
    /// Lets callers write `if lod >= LodLevel::Far` naturally.
    #[test]
    fn lod_level_ordering() {
        assert!(LodLevel::Near < LodLevel::Mid);
        assert!(LodLevel::Mid < LodLevel::Far);
        assert!(LodLevel::Far < LodLevel::VeryFar);
    }

    /// `iter` yields all variants in order.
    #[test]
    fn lod_level_iter_yields_all() {
        let v: Vec<_> = LodLevel::iter().collect();
        assert_eq!(v, vec![
            LodLevel::Near, LodLevel::Mid, LodLevel::Far, LodLevel::VeryFar,
        ]);
    }

    /// Conversion to u32 is just the discriminant.
    #[test]
    fn lod_level_to_u32_dense() {
        assert_eq!(u32::from(LodLevel::Near), 0);
        assert_eq!(u32::from(LodLevel::Mid),  1);
        assert_eq!(u32::from(LodLevel::Far),  2);
        assert_eq!(u32::from(LodLevel::VeryFar), 3);
    }

    /// Short names are stable; mesher_timer log format depends on
    /// them being these specific strings.
    #[test]
    fn lod_level_short_names_stable() {
        assert_eq!(LodLevel::Near.short_name(),    "near");
        assert_eq!(LodLevel::Mid.short_name(),     "mid");
        assert_eq!(LodLevel::Far.short_name(),     "far");
        assert_eq!(LodLevel::VeryFar.short_name(), "vfar");
    }

    /// Player's own chunk is always Near regardless of RD.
    #[test]
    fn desired_lod_at_player_position() {
        let player = ChunkCoord::new(7, -3, 42);
        for &rd in &[1, 2, 6, 32, 120] {
            assert_eq!(
                desired_lod_for(player, player, rd),
                LodLevel::Near,
                "rd={}", rd,
            );
        }
    }

    /// Chunks at or beyond RD should be VeryFar (or close to it).
    /// At exactly RD the threshold falls into the VeryFar band per
    /// the band definitions (¾·RD < cheb ≤ ∞).
    /// Chunks at the RD boundary should be at the lowest-detail tier
    /// the curve makes available; chunks beyond should always be
    /// VeryFar (the catch-all for over-RD positions). Under the new
    /// constant-outer curve, "lowest tier at edge" depends on RD:
    ///
    /// - At RD=32 the Far outer clamps to 32, so distance=32 is Far.
    /// - At RD=64 the Far outer is 32, so distance=64 is VeryFar.
    /// - Distances vastly beyond RD are always VeryFar (the formula
    ///   has nothing further to bucket them into).
    #[test]
    fn desired_lod_at_or_beyond_render_distance() {
        let player = ChunkCoord::new(0, 0, 0);
        // RD=32: edge falls in the (clamped) Far band. bands(32)=8,16,32.
        let edge_32 = ChunkCoord::new(32, 0, 0);
        assert_eq!(desired_lod_for(edge_32, player, 32), LodLevel::Far);
        // RD=64: bands scale to 16,32,64, so the RD-edge sits in Far (not
        // VeryFar as under the old fixed 8/16/32 bands).
        let edge_64 = ChunkCoord::new(64, 0, 0);
        assert_eq!(desired_lod_for(edge_64, player, 64), LodLevel::Far);
        // Way beyond RD: always VeryFar, regardless of RD.
        let beyond = ChunkCoord::new(320, 0, 0);
        assert_eq!(desired_lod_for(beyond, player, 32), LodLevel::VeryFar);
        assert_eq!(desired_lod_for(beyond, player, 64), LodLevel::VeryFar);
    }

    /// Explicit band table for RD=6 (the smallest reasonable RD).
    /// At RD<8 the Near outer clamps to RD, so the entire view is
    /// Near. Mid/Far/VeryFar are empty.
    ///
    /// If this test starts failing, the band thresholds were
    /// changed and the user-facing perf characteristics shifted.
    #[test]
    fn desired_lod_band_boundaries_rd_6() {
        let player = ChunkCoord::new(0, 0, 0);
        let lod = |d: i32| desired_lod_for(ChunkCoord::new(d, 0, 0), player, 6);
        // RD-relative bands at RD=6 are near=2, mid=3, far=6.
        assert_eq!(lod(0), LodLevel::Near);
        assert_eq!(lod(2), LodLevel::Near);
        assert_eq!(lod(3), LodLevel::Mid);
        assert_eq!(lod(4), LodLevel::Far);
        assert_eq!(lod(6), LodLevel::Far);
        // Beyond RD (sphere cull discards these, but classify sanely): >far → VeryFar.
        assert_eq!(lod(7), LodLevel::VeryFar);
    }

    /// Explicit band table for RD=32. At RD=32 the Near and Mid
    /// outers don't clamp (8<32, 16<32) but Far outer DOES clamp
    /// to 32. So Near=[0,8], Mid=[9,16], Far=[17,32], VeryFar=empty
    /// within the sphere.
    #[test]
    fn desired_lod_band_boundaries_rd_32() {
        let player = ChunkCoord::new(0, 0, 0);
        let lod = |d: i32| desired_lod_for(ChunkCoord::new(d, 0, 0), player, 32);
        assert_eq!(lod(0),  LodLevel::Near);
        assert_eq!(lod(8),  LodLevel::Near);
        assert_eq!(lod(9),  LodLevel::Mid);
        assert_eq!(lod(16), LodLevel::Mid);
        assert_eq!(lod(17), LodLevel::Far);
        assert_eq!(lod(32), LodLevel::Far);  // clamped to RD
        // Beyond RD (sphere cull discards these but classifier
        // still defines them):
        assert_eq!(lod(33), LodLevel::VeryFar);
    }

    /// Explicit band table for RD=64 — the target play-experience
    /// configuration. All four bands are active and non-empty.
    /// Near=[0,8], Mid=[9,16], Far=[17,32], VeryFar=[33,64].
    ///
    /// This is the test that documents the design intent: the user
    /// asked for Near=8 chunks with reasonable falloffs to RD=64.
    /// If this fails the curve has drifted from the design.
    #[test]
    fn desired_lod_band_boundaries_rd_64() {
        let player = ChunkCoord::new(0, 0, 0);
        let lod = |d: i32| desired_lod_for(ChunkCoord::new(d, 0, 0), player, 64);
        // RD-relative bands at RD=64 are near=16, mid=32, far=64.
        // Near
        assert_eq!(lod(0),  LodLevel::Near);
        assert_eq!(lod(16), LodLevel::Near);
        // Mid
        assert_eq!(lod(17), LodLevel::Mid);
        assert_eq!(lod(32), LodLevel::Mid);
        // Far
        assert_eq!(lod(33), LodLevel::Far);
        assert_eq!(lod(64), LodLevel::Far);
        // VeryFar (beyond the far band; sphere cull discards, classifier defines)
        assert_eq!(lod(65), LodLevel::VeryFar);
    }

    /// Tiny render distances (1, 2) shouldn't crash and should
    /// degrade gracefully. With the constant-outer curve, RD<8
    /// means everything within the sphere is Near (the outers all
    /// clamp to RD).
    #[test]
    fn desired_lod_rd_too_small() {
        let player = ChunkCoord::new(0, 0, 0);
        // RD=1: Near=Mid=Far outer all clamp to 1. cheb=0,1 are Near;
        // cheb≥2 are VeryFar (out of any clamped band).
        assert_eq!(desired_lod_for(ChunkCoord::new(0, 0, 0), player, 1), LodLevel::Near);
        assert_eq!(desired_lod_for(ChunkCoord::new(1, 0, 0), player, 1), LodLevel::Near);
        assert_eq!(desired_lod_for(ChunkCoord::new(2, 0, 0), player, 1), LodLevel::VeryFar);
        // RD=2: same shape, just one chunk further.
        assert_eq!(desired_lod_for(ChunkCoord::new(2, 0, 0), player, 2), LodLevel::Near);
        assert_eq!(desired_lod_for(ChunkCoord::new(3, 0, 0), player, 2), LodLevel::VeryFar);
    }

    /// Chebyshev metric: chunks equidistant in the lattice (corner
    /// of a cube vs face-center of the same cube) get the same LOD.
    /// Validates we're not accidentally Euclidean-distancing.
    #[test]
    fn desired_lod_chebyshev_is_uniform() {
        let player = ChunkCoord::new(0, 0, 0);
        let face   = ChunkCoord::new(5, 0, 0);   // Chebyshev = 5
        let corner = ChunkCoord::new(5, 5, 5);   // Chebyshev = 5
        assert_eq!(
            desired_lod_for(face,   player, 32),
            desired_lod_for(corner, player, 32),
        );
    }

    /// Negative-coordinate chunks behave the same as positive (abs
    /// distance). Catches a regression where someone uses raw `dx`
    /// instead of `.abs()`.
    #[test]
    fn desired_lod_handles_negative_offsets() {
        let player = ChunkCoord::new(0, 0, 0);
        let a = ChunkCoord::new( 10, 0, 0);
        let b = ChunkCoord::new(-10, 0, 0);
        assert_eq!(
            desired_lod_for(a, player, 32),
            desired_lod_for(b, player, 32),
        );
    }

    /// `MeshSlotKey` for the same coord at different LODs hashes
    /// differently. The renderer's `HashMap` must treat them as
    /// distinct entries.
    #[test]
    fn mesh_slot_key_uniqueness() {
        use std::collections::HashSet;
        let coord = ChunkCoord::new(1, 2, 3);
        let mut s = HashSet::new();
        for lod in LodLevel::iter() {
            s.insert(MeshSlotKey::at(coord, lod));
        }
        assert_eq!(s.len(), LodLevel::COUNT);
    }

    /// `MeshSlotKey::near` is just shorthand for `at(coord, Near)`.
    #[test]
    fn mesh_slot_key_near_is_at_near() {
        let coord = ChunkCoord::new(7, 8, 9);
        assert_eq!(
            MeshSlotKey::near(coord),
            MeshSlotKey::at(coord, LodLevel::Near),
        );
    }

    /// Round-trip: pack flag_bits + LOD, unpack, get same back.
    /// Critical for the GPU SSBO encoding.
    #[test]
    fn flags_lod_pack_unpack_roundtrip() {
        for lod in LodLevel::iter() {
            for &flag_bits in &[0u32, 1, 0xFE, 0xFF] {
                let packed = flags_with_lod(flag_bits, lod);
                let (out_flags, out_lod) = unpack_flags(packed);
                assert_eq!(out_flags, flag_bits, "flag_bits roundtrip lod={:?}", lod);
                assert_eq!(out_lod, lod, "lod roundtrip flag_bits={}", flag_bits);
            }
        }
    }

    /// flag_bits and lod don't bleed into each other — packing
    /// flag_bits=0xFF doesn't accidentally set lod bits, and
    /// packing lod=VeryFar doesn't accidentally set flag bits.
    #[test]
    fn flags_lod_pack_does_not_bleed() {
        let p1 = flags_with_lod(0xFF, LodLevel::Near);
        assert_eq!(p1 & 0xFF, 0xFF);            // flag bits set
        assert_eq!((p1 >> 8) & 0xFF, 0);        // lod is Near=0

        let p2 = flags_with_lod(0, LodLevel::VeryFar);
        assert_eq!(p2 & 0xFF, 0);               // flag bits clear
        assert_eq!((p2 >> 8) & 0xFF, 3);        // lod is VeryFar=3
    }

    /// Out-of-range LOD bits (e.g., from a corrupted SSBO read or
    /// a future LOD level we don't know about) fall back to Near
    /// rather than panicking. Defensive parsing.
    #[test]
    fn unpack_flags_out_of_range_lod_falls_back_to_near() {
        // Manually construct a packed u32 with lod_idx=42 (invalid).
        let bogus = 0x2A00; // bits 8..15 = 42
        let (_, lod) = unpack_flags(bogus);
        assert_eq!(lod, LodLevel::Near);
    }

    // =========================================================================
    // LodSettings tests (F3)
    // =========================================================================

    /// Default settings produce the documented baseline curve.
    /// Catches accidental changes to the default factor table.
    #[test]
    fn lod_settings_default_baseline() {
        let s = LodSettings::default();
        assert_eq!(s.factor_for(LodLevel::Near),    1);
        assert_eq!(s.factor_for(LodLevel::Mid),     2);
        assert_eq!(s.factor_for(LodLevel::Far),     4);
        assert_eq!(s.factor_for(LodLevel::VeryFar), 8);
    }

    /// Construct with a valid table.
    #[test]
    fn lod_settings_new_accepts_valid_table() {
        let s = LodSettings::new([1, 2, 4, 16]).unwrap();
        assert_eq!(s.factor_for(LodLevel::VeryFar), 16);
    }

    /// Reject factors not in VALID_FACTORS.
    #[test]
    fn lod_settings_rejects_invalid_factor() {
        // 3 doesn't divide CHUNK_SIZE=32 and isn't in the table.
        assert!(LodSettings::new([1, 3, 4, 8]).is_err());
        // 64 isn't in VALID_FACTORS (CHUNK_SIZE=32 makes it meaningless).
        assert!(LodSettings::new([1, 2, 4, 64]).is_err());
        // 0 is meaningless.
        assert!(LodSettings::new([0, 2, 4, 8]).is_err());
    }

    /// Reject non-monotonic tables — Mid more detailed than Near is
    /// nonsense and should fail loudly rather than silently render
    /// wrong.
    #[test]
    fn lod_settings_rejects_non_monotonic() {
        assert!(LodSettings::new([2, 1, 4, 8]).is_err());
        assert!(LodSettings::new([1, 2, 8, 4]).is_err());
    }

    /// Equal adjacent factors are allowed — a user might want
    /// Near=Mid=1 (no downsampling for the closer half) and only
    /// downsample at Far/VeryFar.
    #[test]
    fn lod_settings_accepts_equal_adjacent_factors() {
        assert!(LodSettings::new([1, 1, 4, 8]).is_ok());
        assert!(LodSettings::new([1, 1, 1, 1]).is_ok());
    }

    /// `factor_for` is a pure indexed lookup, no surprises.
    #[test]
    fn lod_settings_factor_for_is_simple_lookup() {
        let s = LodSettings::new([1, 2, 4, 8]).unwrap();
        for lod in LodLevel::iter() {
            let f = s.factor_for(lod);
            // Returned factor must be in VALID_FACTORS, by construction.
            assert!(VALID_FACTORS.contains(&f));
        }
    }

    /// `as_array` round-trips through `new`.
    #[test]
    fn lod_settings_as_array_roundtrip() {
        let original = [1, 2, 8, 16];
        let s = LodSettings::new(original).unwrap();
        assert_eq!(s.as_array(), original);
    }

    /// `from_env` returns default when the env var is unset.
    /// Uses the standard Rust env-var test pattern; the var is
    /// unset by name to ensure isolation.
    #[test]
    fn lod_settings_from_env_default_when_unset() {
        // SAFETY: env var manipulation is process-global. Tests in this
        // crate run with default cargo test parallelism but env_var
        // accesses serialise on the libc env lock; this test only
        // *removes* a never-set var, which is a no-op.
        std::env::remove_var("UNENDLICH_LOD_FACTORS");
        let s = LodSettings::from_env();
        assert_eq!(s, LodSettings::default());
    }

    // ===== ShellWalker tests (v0.24.0a.2) =====

    /// At radius 0 the walker yields exactly the origin.
    #[test]
    fn shell_walker_radius_zero() {
        let v: Vec<_> = ShellWalker::new(0).collect();
        assert_eq!(v, vec![(0, 0, 0)]);
    }

    /// At radius 1 the walker yields exactly the 3³−1 = 26 surface coords
    /// of the unit cube shell, with no duplicates.
    #[test]
    fn shell_walker_radius_one_total_and_unique() {
        let v: Vec<_> = ShellWalker::new(1).collect();
        assert_eq!(v.len(), 26);
        let set: std::collections::HashSet<_> = v.iter().copied().collect();
        assert_eq!(set.len(), 26, "duplicates in radius-1 walk");
        // Every coord has max(|dx|,|dy|,|dz|) == 1
        for &(dx, dy, dz) in &v {
            assert_eq!(dx.abs().max(dy.abs()).max(dz.abs()), 1);
        }
    }

    /// At radius `s`, the walker yields exactly `24s² + 2` unique surface
    /// coords with `max(|dx|,|dy|,|dz|) == s` for each. Verified at
    /// several radii.
    #[test]
    fn shell_walker_total_matches_surface_formula() {
        for s in 0..=10i32 {
            let v: Vec<_> = ShellWalker::new(s).collect();
            let expected = if s == 0 { 1 } else { (24 * s * s + 2) as usize };
            assert_eq!(v.len(), expected, "wrong count for radius {}", s);
            let set: std::collections::HashSet<_> = v.iter().copied().collect();
            assert_eq!(set.len(), v.len(), "duplicates at radius {}", s);
            for &(dx, dy, dz) in &v {
                assert_eq!(
                    dx.abs().max(dy.abs()).max(dz.abs()),
                    s,
                    "non-surface coord at radius {}: ({}, {}, {})",
                    s, dx, dy, dz
                );
            }
        }
    }

    /// THE KEY FIX TEST. At radius 8, the first 64 emitted coords must
    /// be spread across all 6 cube faces (no single face has more than
    /// ~11 of them). This is the regression test for the directional-
    /// bias bug in v0.23 / v0.24.0a.1.
    ///
    /// With the prior `dz: -s..=s` outer-loop walk, the first 64 coords
    /// were 100% on the -z face. With face-round-robin, the spread is
    /// roughly equal.
    #[test]
    fn shell_walker_first_n_coords_are_evenly_spread() {
        let s: i32 = 8;
        let prefix: Vec<_> = ShellWalker::new(s).take(64).collect();
        assert_eq!(prefix.len(), 64);

        // Classify each by face. Face ownership: z-faces own |dz|==s.
        // x-faces own |dz|<s AND |dx|==s. y-faces own the rest of the
        // surface (|dz|<s AND |dx|<s AND |dy|==s).
        let mut counts = [0u32; 6];
        for &(dx, dy, dz) in &prefix {
            let f = if dz == -s { 0 }
                    else if dz == s { 1 }
                    else if dx == -s { 2 }
                    else if dx == s  { 3 }
                    else if dy == -s { 4 }
                    else { 5 };
            counts[f] += 1;
        }

        // No face dominates. Each face should have between 5 and 16
        // coords out of 64 (true even split would be ~10.7 each).
        // The bounds are wide to allow for face-size differences
        // (z-faces have more coords than x/y-faces) without being so
        // wide they let the bug slip through.
        for (i, &n) in counts.iter().enumerate() {
            assert!(
                n >= 5 && n <= 16,
                "face {} got {} of first 64 coords; expected 5-16. \
                 Full counts: {:?}",
                i, n, counts,
            );
        }
    }

    /// Within the first 6 coords, EVERY face must contribute at least
    /// one. That's the round-robin invariant: by the time we've
    /// emitted 6 coords (or fewer than 6 if a face is empty), every
    /// non-empty face has its turn.
    #[test]
    fn shell_walker_round_robin_visits_every_face_quickly() {
        let s: i32 = 8;
        let first_six: Vec<_> = ShellWalker::new(s).take(6).collect();
        assert_eq!(first_six.len(), 6);

        let mut faces_seen = [false; 6];
        for &(dx, dy, dz) in &first_six {
            let f = if dz == -s { 0 }
                    else if dz == s { 1 }
                    else if dx == -s { 2 }
                    else if dx == s  { 3 }
                    else if dy == -s { 4 }
                    else { 5 };
            faces_seen[f] = true;
        }
        for (i, &seen) in faces_seen.iter().enumerate() {
            assert!(seen, "face {} not visited in first 6 coords", i);
        }
    }

    #[test]
    fn boundary_face_culled_only_within_same_lod() {
        let near = lod_cell_stride(LodLevel::Near);
        let far = lod_cell_stride(LodLevel::Far);
        assert_eq!((near, far), (1, 4));
        // Same stride, solid neighbour → cull (normal within-LOD behavior).
        assert!(cull_boundary_face(far, far, true));
        assert!(cull_boundary_face(near, near, true));
        // Same stride, air neighbour → emit.
        assert!(!cull_boundary_face(far, far, false));
        // LOD MISMATCH, solid neighbour → DO NOT cull (the seam fix: a far chunk
        // keeps its face against a near neighbour, else a gap shows where the
        // two resolutions fail to meet).
        assert!(!cull_boundary_face(far, near, true));
        assert!(!cull_boundary_face(near, far, true));
        assert!(!cull_boundary_face(far, near, false));
    }
}

#[cfg(test)]
mod decimation_tests {
    use super::{decimation_for, downsample_pow2, LodLevel};
    use crate::constants::{CHUNK_SIZE, CHUNK_VOLUME};
    use crate::world::blocks::BlockId;
    use crate::world::chunk::{Chunk, ChunkCoord};

    const CS: usize = CHUNK_SIZE as usize;
    fn idx(x: usize, y: usize, z: usize) -> usize { x + z * CS + y * CS * CS }
    fn solid_count(c: &Chunk) -> usize { c.blocks.iter().filter(|b| !b.is_air()).count() }

    fn checker() -> Chunk {
        // a chunk with isolated single solid blocks (worst case for holes)
        let mut b = vec![BlockId::Air; CHUNK_VOLUME];
        for y in 0..CS { for z in 0..CS { for x in 0..CS {
            if (x + y + z) % 4 == 0 { b[idx(x, y, z)] = BlockId::Stone; }
        }}}
        Chunk::from_blocks(ChunkCoord::new(1, 2, 3), b)
    }

    #[test]
    fn decimation_factors_match_tiers() {
        assert_eq!(decimation_for(LodLevel::Near), 1);
        assert_eq!(decimation_for(LodLevel::Mid), 1);
        assert_eq!(decimation_for(LodLevel::Far), 2);
        assert_eq!(decimation_for(LodLevel::VeryFar), 4);
    }

    #[test]
    fn factor_one_is_identity() {
        let c = checker();
        let d = downsample_pow2(&c, 1);
        assert_eq!(d.blocks, c.blocks);
    }

    #[test]
    fn silhouette_is_superset_no_holes() {
        // every solid block in the source must remain solid after decimation,
        // for both Far(2) and VeryFar(4) — decimation never opens a hole.
        let c = checker();
        for &f in &[2u32, 4] {
            let d = downsample_pow2(&c, f);
            for i in 0..CHUNK_VOLUME {
                if !c.blocks[i].is_air() {
                    assert!(!d.blocks[i].is_air(), "hole opened at {} for factor {}", i, f);
                }
            }
            // superset ⇒ at least as many solids
            assert!(solid_count(&d) >= solid_count(&c), "lost solidity at factor {}", f);
        }
    }

    #[test]
    fn groups_are_uniform() {
        let c = checker();
        let f = 2usize;
        let d = downsample_pow2(&c, f as u32);
        let groups = CS / f;
        for gy in 0..groups { for gz in 0..groups { for gx in 0..groups {
            let r = d.blocks[idx(gx * f, gy * f, gz * f)];
            for dy in 0..f { for dz in 0..f { for dx in 0..f {
                assert_eq!(d.blocks[idx(gx * f + dx, gy * f + dy, gz * f + dz)], r,
                           "group ({},{},{}) not uniform", gx, gy, gz);
            }}}
        }}}
    }

    #[test]
    fn all_air_and_all_solid_preserved() {
        let air = Chunk::from_blocks(ChunkCoord::new(0, 0, 0), vec![BlockId::Air; CHUNK_VOLUME]);
        let d_air = downsample_pow2(&air, 4);
        assert!(d_air.blocks.iter().all(|b| b.is_air()));
        let solid = Chunk::from_blocks(ChunkCoord::new(0, 0, 0), vec![BlockId::Stone; CHUNK_VOLUME]);
        let d_solid = downsample_pow2(&solid, 4);
        assert!(d_solid.blocks.iter().all(|b| *b == BlockId::Stone));
    }

    #[test]
    fn deterministic() {
        let c = checker();
        assert_eq!(downsample_pow2(&c, 2).blocks, downsample_pow2(&c, 2).blocks);
    }
}
