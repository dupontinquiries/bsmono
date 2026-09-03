//! SMOOTH TERRAIN — the surface the generator already knows about.
//!
//! `docs/TERRAIN_RESOLUTION.md` planned a new per-block `density` store to
//! feed a surface-nets extractor, and reading the generator to build it turned
//! that plan up as unnecessary. **The continuous field is already here.**
//! `WorldModel::density(x, y, z, t) -> f64` is the source of truth for
//! solidity, and `cpu_generator` builds the block grid by THRESHOLDING it:
//!
//! ```text
//!     solid  <=>  density > DENSITY_THRESHOLD && !is_cave
//! ```
//!
//! `DENSITY_THRESHOLD` is 0.0. So the block array is not the terrain — it is a
//! one-bit quantisation of the terrain, and the precision a smooth mesher
//! needs was computed and then discarded. This module stops discarding it.
//!
//! Two consequences worth stating plainly, because both cut work that was
//! budgeted:
//!
//! * **Generated terrain needs no new storage at all.** The field is a
//!   function; sample it. Storage is only wanted for EDITED terrain — the
//!   same situation the block grid is already in, and the same place its
//!   answer belongs.
//! * **The field was already tuned for this.** `density_from_surface`'s own
//!   comment says the detail amplitude is held to ~3x margin below the
//!   vertical bias gradient so that "density is monotonic through the surface
//!   → one clean boundary, no striping". Monotone through the crossing is
//!   exactly the property linear interpolation between two samples needs to
//!   land in the right place. The generator was written to be isosurfaced;
//!   nobody had done it.
//!
//! ## Caves
//!
//! `is_cave` is deliberately independent of density — it carves through
//! already-solid rock — so it cannot be interpolated, only applied. A cave
//! sample is forced hard negative, which puts the surface within one cell of
//! the cave wall rather than smoothly through it. Cave mouths will therefore
//! read blockier than hillsides until caves grow a continuous field of their
//! own. Noted rather than solved.

use isosurface::{extract, Field, Mesh, Mesher, Seams};

use crate::world::world_model::{anchors, WorldModel};

/// Value handed to the extractor for a carved cell. Large enough that no
/// interpolation from a neighbouring solid sample lands short of the wall.
const CAVE_AIR: f32 = -4.0;

/// `WorldModel`'s density, as something the extractor can read.
///
/// Borrows rather than owns: meshing runs on worker threads against a shared
/// model, and this is a view, not a copy.
pub struct ModelField<'a> {
    pub model: &'a WorldModel,
    /// Apply cave carving. Off is faster and is what a coarse LOD tier wants,
    /// since it cannot resolve a cave anyway.
    pub caves: bool,
    /// Time, for the model's animated terms. Meshing is a snapshot.
    pub t: f64,
}

impl<'a> ModelField<'a> {
    pub fn new(model: &'a WorldModel) -> ModelField<'a> {
        ModelField { model, caves: true, t: 0.0 }
    }
    pub fn without_caves(mut self) -> Self {
        self.caves = false;
        self
    }
}

impl Field for ModelField<'_> {
    fn sample(&self, x: i32, y: i32, z: i32) -> f32 {
        let (xf, yf, zf) = (x as f64, y as f64, z as f64);
        if self.caves && self.model.is_cave(xf, yf, zf) {
            return CAVE_AIR;
        }
        self.model.density(xf, yf, zf, self.t) as f32
    }
}

/// Mesh a region of the world as a smooth surface.
///
/// `origin` and `dims` are in blocks; `stride` and `seams` are the LOD
/// controls described in the `isosurface` crate. Output positions are in
/// full-detail block units relative to `origin`.
pub fn mesh_region(
    model: &WorldModel,
    origin: [i32; 3],
    dims: [i32; 3],
    stride: i32,
    seams: Seams,
) -> Mesh {
    let field = ModelField::new(model);
    extract(
        &field,
        Mesher::new(dims)
            .at(origin)
            .stride(stride)
            .seams(seams)
            // solid where density > 0, which is what cpu_generator thresholds on
            .iso(anchors::DENSITY_THRESHOLD as f32),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::world_model::{anchors, WorldModel};

    fn model() -> WorldModel {
        WorldModel::new(0xC0FFEE)
    }

    /// The column the tests probe. Picked once so a failure is reproducible.
    const PX: i32 = 40;
    const PZ: i32 = -24;

    #[test]
    fn the_smooth_surface_sits_where_the_generator_says_the_surface_is() {
        // The whole claim of this module: extracting the density field puts
        // the mesh where the generator's own continuous surface target is,
        // WITHOUT consulting the block grid at all.
        let m = model();
        let target = m.surface_target(PX as f64, PZ as f64, 0.0);
        let base = target.floor() as i32 - 12;
        let mesh = mesh_region(&m, [PX - 4, base, PZ - 4], [9, 25, 9], 1, Seams::NONE);
        assert!(!mesh.is_empty(), "no surface found near y={target:.1}");

        // vertices near the probe column, back in world y
        let near: Vec<f32> = mesh
            .positions
            .iter()
            .filter(|p| (p.x - 4.0).abs() < 1.5 && (p.z - 4.0).abs() < 1.5)
            .map(|p| p.y + base as f32)
            .collect();
        assert!(!near.is_empty(), "surface found, but not over the probe column");
        let mean = near.iter().sum::<f32>() / near.len() as f32;
        assert!(
            (mean - target as f32).abs() < 3.0,
            "smooth surface at {mean:.2}, generator's target {target:.2}"
        );
    }

    #[test]
    fn the_surface_is_not_quantised_to_whole_blocks() {
        // The REASON for this module. A block grid can only put the surface on
        // an integer; if every vertex here lands on one, the extractor is
        // reading something binary and the precision is still being thrown
        // away.
        let m = model();
        let target = m.surface_target(PX as f64, PZ as f64, 0.0);
        let base = target.floor() as i32 - 12;
        let mesh = mesh_region(&m, [PX - 8, base, PZ - 8], [17, 25, 17], 1, Seams::NONE);
        let fractional = mesh
            .positions
            .iter()
            .filter(|p| {
                let f = p.y - p.y.floor();
                f > 0.02 && f < 0.98
            })
            .count();
        assert!(
            fractional * 4 > mesh.positions.len(),
            "only {fractional} of {} vertices are off-lattice — the surface is quantised",
            mesh.positions.len()
        );
    }

    #[test]
    fn the_field_is_deterministic_for_a_seed() {
        // Meshing runs on worker threads and a chunk may be re-meshed when a
        // neighbour's LOD changes. Two extractions of the same region must
        // agree exactly or the seam work is built on sand.
        let m = model();
        let a = mesh_region(&m, [0, 40, 0], [9, 9, 9], 1, Seams::NONE);
        let b = mesh_region(&m, [0, 40, 0], [9, 9, 9], 1, Seams::NONE);
        assert_eq!(a.positions.len(), b.positions.len());
        for (p, q) in a.positions.iter().zip(&b.positions) {
            assert_eq!(p.to_array(), q.to_array());
        }
    }

    #[test]
    fn a_coarse_tier_still_finds_the_same_hillside() {
        // Stride is a detail reduction, not a different world — the check that
        // makes an LOD ladder trustworthy.
        let m = model();
        let target = m.surface_target(PX as f64, PZ as f64, 0.0);
        let base = target.floor() as i32 - 16;
        let fine = mesh_region(&m, [PX - 8, base, PZ - 8], [17, 33, 17], 1, Seams::NONE);
        let coarse = mesh_region(&m, [PX - 8, base, PZ - 8], [9, 17, 9], 2, Seams::NONE);
        assert!(!fine.is_empty() && !coarse.is_empty());
        let mean = |m: &Mesh| m.positions.iter().map(|p| p.y).sum::<f32>() / m.positions.len() as f32;
        assert!(
            (mean(&fine) - mean(&coarse)).abs() < 2.0,
            "fine {:.2} vs coarse {:.2}",
            mean(&fine),
            mean(&coarse)
        );
    }

    #[test]
    fn caves_can_be_switched_off_and_that_changes_something_underground() {
        // Not an assertion about cave PLACEMENT — that is the model's business.
        // Just that the flag is wired, so a coarse tier can skip the cost.
        let m = model();
        let deep = [0, -40, 0];
        let with = extract(
            &ModelField::new(&m),
            Mesher::new([17, 17, 17]).at(deep).iso(anchors::DENSITY_THRESHOLD as f32),
        );
        let without = extract(
            &ModelField::new(&m).without_caves(),
            Mesher::new([17, 17, 17]).at(deep).iso(anchors::DENSITY_THRESHOLD as f32),
        );
        // deep underground with caves off should be solid rock: no surface
        assert!(
            without.is_empty() || without.positions.len() < with.positions.len(),
            "carving made no difference underground: {} vs {}",
            with.positions.len(),
            without.positions.len()
        );
    }
}
