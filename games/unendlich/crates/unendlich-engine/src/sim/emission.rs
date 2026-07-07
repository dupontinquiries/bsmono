//! `sim::emission` — the shared elemental **discharge** substrate.
//!
//! Every emission in the game (dragon breath, an enrobed blade, a staff cast, a
//! lightning bolt) reduces to the same act: *deposit energy into cells along a
//! shape*. This module is that act, factored out of the emission bench:
//!
//! ```text
//!   deposit_at(cell, dir, energy, element)
//!       -> heat (thermal field)  +  push (impulse / "vector" field)  +  payload
//! ```
//!
//! Three layers, lowest shared:
//!   * **Discharge** — [`deposit_at`] + [`response`]: how a unit of `energy` of an
//!     [`Element`] splits into heat / impulse / element payload. Pure data.
//!   * **Shape emitters** — [`breath_cone`], [`burst_ring`], [`aura`],
//!     [`sky_call`], [`trail`], [`arc`]: walk a geometry, call `deposit_at`.
//!   * **[`Signature`]** — the per-emission knobs the emitters read (element,
//!     energy, range, spread, seed). The *identity* layer; sourced elsewhere from
//!     element × genome × brain.
//!
//! Everything here is deterministic fixed-point: no `sqrt`, no trig. Cones are
//! tested with squared dot/cross comparisons so they need no perpendicular basis
//! and no normalization. Output is collected into an [`EmissionBuffer`]; a later
//! world/render adapter drains it into `thermal::HeatField`, the impulse field,
//! and world payloads. **Render is a read-only consumer; nothing here is GPU.**

use crate::sim::elemental::Element;
use crate::sim::fixed::{Fixed, Vec3};

/// Integer voxel coordinate, matching `sim::lightning`'s path cells.
pub type Cell = [i32; 3];

/// Hard cap on the per-emission scan box half-extent (keeps a huge `range` from
/// blowing up the cell loop). Production emitters with large range should march
/// the axis instead; this is the correctness-first ceiling.
const MAX_BOX_HALF: i32 = 48;

// ---------------------------------------------------------------------------
// fixed-point geometry helpers (no sqrt / trig)
// ---------------------------------------------------------------------------

#[inline]
fn dot(a: Vec3, b: Vec3) -> Fixed {
    a.x * b.x + a.y * b.y + a.z * b.z
}

#[inline]
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

#[inline]
fn half() -> Fixed {
    Fixed::from_frac(1, 2)
}

/// World-space center of a voxel.
#[inline]
fn cell_center(c: Cell) -> Vec3 {
    Vec3::new(
        Fixed::from_int(c[0] as i64) + half(),
        Fixed::from_int(c[1] as i64) + half(),
        Fixed::from_int(c[2] as i64) + half(),
    )
}

/// Voxel containing a world-space point (floor toward −∞, matching `floor_int`).
#[inline]
fn vec_to_cell(v: Vec3) -> Cell {
    [
        v.x.floor_int() as i32,
        v.y.floor_int() as i32,
        v.z.floor_int() as i32,
    ]
}

#[inline]
fn box_half_for(range: Fixed) -> i32 {
    let r = range.floor_int() as i32 + 1;
    r.clamp(0, MAX_BOX_HALF)
}

// ---------------------------------------------------------------------------
// the deposit event + sink
// ---------------------------------------------------------------------------

/// One unit of discharge landing on one cell. Consumers split this across the
/// thermal field (`heat`), the impulse/vector field (`push`), and a per-element
/// world payload (`element` + `payload`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Deposit {
    /// Voxel touched.
    pub cell: Cell,
    /// Impulse vector to add to the force field at `cell`. Direction need not be
    /// unit length; magnitude is indicative and the consumer may renormalize.
    pub push: Vec3,
    /// Signed heat units to inject into the thermal field (negative = cooling,
    /// e.g. water).
    pub heat: i32,
    /// Which element's payload this is.
    pub element: Element,
    /// Payload magnitude — meaning is per-element (wetness, charge, growth,
    /// ignition, obscuration, …). Always ≥ 0 for `energy ≥ 0`.
    pub payload: Fixed,
}

/// Anything that can receive [`Deposit`]s. The pure emitters are generic over
/// this so they can be unit-tested against an in-memory buffer and, later, run
/// against the real world/thermal/impulse adapters.
pub trait DischargeSink {
    fn deposit(&mut self, d: Deposit);
}

/// Simple collecting sink: the sim produces one of these per tick of emission;
/// a world/render adapter drains `events` into the real fields.
#[derive(Clone, Default, Debug)]
pub struct EmissionBuffer {
    pub events: Vec<Deposit>,
}

impl EmissionBuffer {
    pub fn new() -> Self {
        EmissionBuffer { events: Vec::new() }
    }
    pub fn clear(&mut self) {
        self.events.clear();
    }
    pub fn len(&self) -> usize {
        self.events.len()
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
    /// Total signed heat across all deposits (handy for tests / debug).
    pub fn total_heat(&self) -> i64 {
        self.events.iter().map(|d| d.heat as i64).sum()
    }
    /// Does any deposit land on `cell`?
    pub fn touches(&self, cell: Cell) -> bool {
        self.events.iter().any(|d| d.cell == cell)
    }
}

impl DischargeSink for EmissionBuffer {
    fn deposit(&mut self, d: Deposit) {
        self.events.push(d);
    }
}

// ---------------------------------------------------------------------------
// Discharge core: element identity (energy -> heat / push / payload)
// ---------------------------------------------------------------------------

/// How a unit of `energy` of one element splits into the three channels. These
/// are placeholder ratios (the design doc treats numbers as tunable); the
/// *signs and relative emphasis* are the identity (fire heats, water cools,
/// earth/air push, wood/water carry strong payloads).
#[derive(Clone, Copy, Debug)]
struct ElementResponse {
    /// Heat per unit energy (signed; negative = cooling).
    heat: Fixed,
    /// Impulse magnitude per unit energy (applied along the deposit direction).
    push: Fixed,
    /// Element payload per unit energy.
    payload: Fixed,
}

fn response(e: Element) -> ElementResponse {
    let f = Fixed::from_frac;
    match e {
        // strong heat, modest shove, ignition payload
        Element::Fire => ElementResponse { heat: Fixed::ONE, push: f(1, 4), payload: f(1, 2) },
        // heats less than fire, real shove, strong charge payload
        Element::Lightning => ElementResponse { heat: f(2, 5), push: f(3, 10), payload: Fixed::ONE },
        // no heat, biggest shove (mass), debris payload
        Element::Earth => ElementResponse { heat: Fixed::ZERO, push: Fixed::ONE, payload: f(3, 5) },
        // no heat, light shove, obscuration payload
        Element::Dust => ElementResponse { heat: Fixed::ZERO, push: f(3, 10), payload: Fixed::ONE },
        // no heat, strong shove (wind), negligible payload
        Element::Air => ElementResponse { heat: Fixed::ZERO, push: Fixed::ONE, payload: f(1, 10) },
        // cools, moderate shove, strong wetness payload
        Element::Water => ElementResponse { heat: f(-7, 10), push: f(3, 5), payload: Fixed::ONE },
        // no heat, gentle shove, strong growth payload
        Element::Wood => ElementResponse { heat: Fixed::ZERO, push: f(1, 10), payload: Fixed::ONE },
    }
}

/// The Discharge core: deposit `energy` of `element` onto one `cell`, pushing
/// along `dir`. This is the single shared act every shape emitter funnels into.
pub fn deposit_at<S: DischargeSink>(
    sink: &mut S,
    cell: Cell,
    dir: Vec3,
    energy: Fixed,
    element: Element,
) {
    let r = response(element);
    let heat = (energy * r.heat).floor_int() as i32;
    let push = dir.scale(energy * r.push);
    let payload = energy * r.payload;
    sink.deposit(Deposit { cell, push, heat, element, payload });
}

// ---------------------------------------------------------------------------
// Signature: the knobs emitters read
// ---------------------------------------------------------------------------

/// Per-emission tunables. The identity layer — populated elsewhere from
/// element × genome × brain. Emitters read the fields they need.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Signature {
    pub element: Element,
    /// Energy deposited per touched cell.
    pub energy: Fixed,
    /// Reach in blocks: cone length / ring outer radius / ball radius.
    pub range: Fixed,
    /// Cone lateral slope (radius gained per block of axial distance); reused as
    /// the vertical half-thickness for a flattened [`burst_ring`].
    pub spread: Fixed,
    /// Per-individual variation hook (reserved; emitters may sample it later).
    pub jitter_seed: u64,
}

impl Signature {
    pub fn new(element: Element, energy: Fixed, range: Fixed, spread: Fixed, jitter_seed: u64) -> Self {
        Signature { element, energy, range, spread, jitter_seed }
    }

    /// A neutral starting point: 1 energy, 8-block reach, 0.5 slope cone.
    pub fn balanced(element: Element) -> Self {
        Signature {
            element,
            energy: Fixed::ONE,
            range: Fixed::from_int(8),
            spread: Fixed::from_frac(1, 2),
            jitter_seed: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// shape emitters
// ---------------------------------------------------------------------------

/// A dragon's breath / flamethrower: a cone from `origin` along `aim`.
///
/// `aim` should be ~unit length (its magnitude scales the impulse). A cell is
/// hit when it is in front of `origin`, within `range`, and inside the widening
/// cone — all tested with squared dot/cross comparisons, no sqrt:
///   * in front:      `dot(r,aim) > 0`
///   * within range:  `d² ≤ range²·|aim|²`            where `d = dot(r,aim)`
///   * inside cone:    `|cross(r,aim)|² ≤ spread²·d²`  (the `|aim|²` cancels)
pub fn breath_cone<S: DischargeSink>(sink: &mut S, origin: Vec3, aim: Vec3, sig: &Signature) {
    let dirlen2 = dot(aim, aim);
    if dirlen2 <= Fixed::ZERO {
        return;
    }
    let oc = vec_to_cell(origin);
    let rb = box_half_for(sig.range);
    let range2 = sig.range * sig.range;
    let slope2 = sig.spread * sig.spread;
    for dx in -rb..=rb {
        for dy in -rb..=rb {
            for dz in -rb..=rb {
                let cell = [oc[0] + dx, oc[1] + dy, oc[2] + dz];
                let r = cell_center(cell) - origin;
                let d = dot(r, aim);
                if d <= Fixed::ZERO {
                    continue; // behind the muzzle
                }
                let dd = d * d;
                if dd > range2 * dirlen2 {
                    continue; // beyond reach
                }
                let c = cross(r, aim);
                if dot(c, c) > slope2 * dd {
                    continue; // outside the cone
                }
                deposit_at(sink, cell, aim, sig.energy, sig.element);
            }
        }
    }
}

/// A spherical shell from `inner` to `sig.range` around `origin`, pushing
/// outward — the impact ring / shockwave / enrobe burst. Set `flatten` for a
/// ground slam: only cells within `sig.spread` of the origin plane are hit.
pub fn burst_ring<S: DischargeSink>(
    sink: &mut S,
    origin: Vec3,
    sig: &Signature,
    inner: Fixed,
    flatten: bool,
) {
    let oc = vec_to_cell(origin);
    let rb = box_half_for(sig.range);
    let outer2 = sig.range * sig.range;
    let inner2 = inner * inner;
    for dx in -rb..=rb {
        for dy in -rb..=rb {
            for dz in -rb..=rb {
                let cell = [oc[0] + dx, oc[1] + dy, oc[2] + dz];
                let r = cell_center(cell) - origin;
                let d2 = dot(r, r);
                if d2 < inner2 || d2 > outer2 {
                    continue;
                }
                if flatten && r.y.abs() > sig.spread {
                    continue;
                }
                deposit_at(sink, cell, r, sig.energy, sig.element); // push radially out
            }
        }
    }
}

/// A sustained filled ball around the caster (a shield/aura tick) — a
/// [`burst_ring`] with no inner hole and no flattening. Callers typically set a
/// lower `sig.energy` since it fires every tick.
pub fn aura<S: DischargeSink>(sink: &mut S, origin: Vec3, sig: &Signature) {
    burst_ring(sink, origin, sig, Fixed::ZERO, false);
}

/// A bolt called from the sky onto `ground`: a vertical column `height` blocks
/// tall pushing downward, plus a flattened impact ring at the base.
pub fn sky_call<S: DischargeSink>(sink: &mut S, ground: Vec3, sig: &Signature, height: Fixed) {
    let g = vec_to_cell(ground);
    let h = (height.floor_int() as i32).max(0);
    let down = Vec3::new(Fixed::ZERO, Fixed::from_int(-1), Fixed::ZERO);
    for y in 0..=h {
        deposit_at(sink, [g[0], g[1] + y, g[2]], down, sig.energy, sig.element);
    }
    burst_ring(sink, ground, sig, Fixed::ZERO, true);
}

/// Deposit along a movement path (a charge's fire trail, a drifting cloud):
/// one deposit per point, pushed along `travel`.
pub fn trail<S: DischargeSink>(sink: &mut S, points: &[Vec3], travel: Vec3, sig: &Signature) {
    for p in points {
        deposit_at(sink, vec_to_cell(*p), travel, sig.energy, sig.element);
    }
}

/// Deposit along precomputed conduction paths. `paths` come from
/// `sim::lightning::arc_tree` (run with the real conductivity closure) so this
/// stays decoupled from the Dijkstra. Push runs along each segment.
pub fn arc<S: DischargeSink>(sink: &mut S, paths: &[Vec<Cell>], sig: &Signature) {
    for path in paths {
        for (i, cell) in path.iter().enumerate() {
            let dir = if i + 1 < path.len() {
                let n = path[i + 1];
                Vec3::new(
                    Fixed::from_int((n[0] - cell[0]) as i64),
                    Fixed::from_int((n[1] - cell[1]) as i64),
                    Fixed::from_int((n[2] - cell[2]) as i64),
                )
            } else {
                Vec3::ZERO
            };
            deposit_at(sink, *cell, dir, sig.energy, sig.element);
        }
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_x() -> Vec3 {
        Vec3::new(Fixed::ONE, Fixed::ZERO, Fixed::ZERO)
    }
    // origin sitting at the center of cell [0,0,0]
    fn origin0() -> Vec3 {
        Vec3::new(half(), half(), half())
    }

    #[test]
    fn response_signs_are_the_identity() {
        assert!(response(Element::Fire).heat > Fixed::ZERO, "fire heats");
        assert!(response(Element::Water).heat < Fixed::ZERO, "water cools");
        assert_eq!(response(Element::Earth).heat, Fixed::ZERO, "earth is cold");
        assert!(response(Element::Wood).payload > Fixed::ZERO, "wood carries growth");
        assert!(response(Element::Earth).push >= response(Element::Wood).push, "earth shoves harder than wood");
    }

    #[test]
    fn deposit_at_splits_energy() {
        let mut b = EmissionBuffer::new();
        deposit_at(&mut b, [2, 0, 0], unit_x(), Fixed::ONE, Element::Fire);
        assert_eq!(b.len(), 1);
        let d = b.events[0];
        assert_eq!(d.cell, [2, 0, 0]);
        assert_eq!(d.heat, 1); // 1 energy * fire heat(1.0) -> 1
        assert_eq!(d.payload, Fixed::from_frac(1, 2)); // fire payload 0.5
        assert_eq!(d.push.x, Fixed::from_frac(1, 4)); // fire push 0.25 along +x
        assert_eq!(d.push.y, Fixed::ZERO);
    }

    #[test]
    fn water_deposit_cools() {
        let mut b = EmissionBuffer::new();
        deposit_at(&mut b, [0, 0, 0], unit_x(), Fixed::ONE, Element::Water);
        assert!(b.events[0].heat < 0, "water deposit removes heat");
        assert!(b.events[0].payload > Fixed::ZERO, "but carries wetness");
    }

    #[test]
    fn cone_hits_forward_not_behind() {
        let mut b = EmissionBuffer::new();
        let sig = Signature::new(Element::Fire, Fixed::ONE, Fixed::from_int(6), Fixed::from_frac(1, 2), 0);
        breath_cone(&mut b, origin0(), unit_x(), &sig);
        assert!(!b.is_empty(), "cone should hit something");
        for d in &b.events {
            assert!(d.cell[0] >= 0, "no deposit behind the muzzle: {:?}", d.cell);
        }
        assert!(b.touches([1, 0, 0]), "near-axis cell hit");
    }

    #[test]
    fn cone_respects_range() {
        let mut b = EmissionBuffer::new();
        let sig = Signature::new(Element::Fire, Fixed::ONE, Fixed::from_int(6), Fixed::from_frac(1, 2), 0);
        breath_cone(&mut b, origin0(), unit_x(), &sig);
        for d in &b.events {
            assert!(d.cell[0] <= 6, "axial reach within range: {:?}", d.cell);
        }
        assert!(!b.touches([9, 0, 0]), "nothing past range");
    }

    #[test]
    fn cone_respects_spread() {
        let mut b = EmissionBuffer::new();
        // slope 0.5 -> at axial 4 the cone radius is 2 blocks
        let sig = Signature::new(Element::Fire, Fixed::ONE, Fixed::from_int(8), Fixed::from_frac(1, 2), 0);
        breath_cone(&mut b, origin0(), unit_x(), &sig);
        assert!(b.touches([4, 0, 0]), "on-axis hit");
        assert!(b.touches([4, 1, 0]), "perp 1 < radius 2: hit");
        assert!(!b.touches([4, 4, 0]), "perp 4 > radius 2: miss");
    }

    #[test]
    fn cone_is_deterministic() {
        let sig = Signature::new(Element::Lightning, Fixed::ONE, Fixed::from_int(7), Fixed::from_frac(2, 3), 42);
        let aim = Vec3::new(Fixed::from_frac(2, 3), Fixed::from_frac(1, 3), Fixed::ONE);
        let mut a = EmissionBuffer::new();
        let mut c = EmissionBuffer::new();
        breath_cone(&mut a, origin0(), aim, &sig);
        breath_cone(&mut c, origin0(), aim, &sig);
        assert_eq!(a.events, c.events, "same input -> bit-identical output");
    }

    #[test]
    fn burst_ring_is_a_shell() {
        let mut b = EmissionBuffer::new();
        let sig = Signature::new(Element::Earth, Fixed::ONE, Fixed::from_int(4), Fixed::ZERO, 0);
        burst_ring(&mut b, origin0(), &sig, Fixed::from_int(2), false);
        assert!(b.touches([3, 0, 0]), "in the shell");
        assert!(!b.touches([1, 0, 0]), "inside the inner hole");
        assert!(!b.touches([5, 0, 0]), "outside the outer radius");
        // outward push at [3,0,0]
        let d = b.events.iter().find(|d| d.cell == [3, 0, 0]).unwrap();
        assert!(d.push.x > Fixed::ZERO, "pushed radially outward");
    }

    #[test]
    fn burst_ring_flatten_keeps_the_plane() {
        let mut b = EmissionBuffer::new();
        let sig = Signature::new(Element::Earth, Fixed::ONE, Fixed::from_int(4), Fixed::from_frac(1, 2), 0);
        burst_ring(&mut b, origin0(), &sig, Fixed::ONE, true);
        assert!(b.touches([3, 0, 0]), "in-plane shell cell kept");
        assert!(!b.touches([0, 3, 0]), "out-of-plane shell cell dropped");
    }

    #[test]
    fn aura_fills_the_center() {
        let mut b = EmissionBuffer::new();
        let sig = Signature::new(Element::Lightning, Fixed::ONE, Fixed::from_int(3), Fixed::ZERO, 0);
        aura(&mut b, origin0(), &sig);
        assert!(b.touches([0, 0, 0]), "aura fills the caster's cell");
    }

    #[test]
    fn sky_call_has_column_and_impact() {
        let mut b = EmissionBuffer::new();
        let sig = Signature::new(Element::Lightning, Fixed::ONE, Fixed::from_int(3), Fixed::ZERO, 0);
        sky_call(&mut b, origin0(), &sig, Fixed::from_int(5));
        assert!(b.touches([0, 3, 0]), "column cell above the strike");
        assert!(b.touches([0, 0, 0]), "ground impact at the base");
        // column pushes downward
        let d = b.events.iter().find(|d| d.cell == [0, 4, 0]).unwrap();
        assert!(d.push.y < Fixed::ZERO, "sky bolt drives down");
    }

    #[test]
    fn arc_deposits_along_paths() {
        let mut b = EmissionBuffer::new();
        let sig = Signature::balanced(Element::Lightning);
        let paths = vec![vec![[0, 0, 0], [1, 0, 0], [2, 0, 0]]];
        arc(&mut b, &paths, &sig);
        assert_eq!(b.len(), 3);
        assert!(b.touches([0, 0, 0]) && b.touches([1, 0, 0]) && b.touches([2, 0, 0]));
        // first segment pushes along +x
        let d = b.events.iter().find(|d| d.cell == [0, 0, 0]).unwrap();
        assert!(d.push.x > Fixed::ZERO, "arc pushes along the segment");
    }

    #[test]
    fn trail_deposits_one_per_point() {
        let mut b = EmissionBuffer::new();
        let sig = Signature::balanced(Element::Fire);
        let pts = [
            Vec3::new(Fixed::from_int(0), Fixed::ZERO, Fixed::ZERO),
            Vec3::new(Fixed::from_int(3), Fixed::ZERO, Fixed::ZERO),
        ];
        trail(&mut b, &pts, unit_x(), &sig);
        assert_eq!(b.len(), 2);
        assert!(b.touches([0, 0, 0]) && b.touches([3, 0, 0]));
    }
}
