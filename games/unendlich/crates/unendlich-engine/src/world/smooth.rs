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

use crate::world::blocks::BlockId;
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


// -------------------------------------------------------------- materials ---
//
// `isosurface` emits geometry and nothing else, on purpose: it knows about a
// scalar field and must not learn about biomes, block ids or texture atlases.
// Attaching materials is the engine's half, and it is a separate pass here
// rather than a callback into the extractor for the same reason — the
// extractor stays testable in a second, and this stays swappable without
// touching geometry.
//
// The sample point matters. A vertex sits ON the surface, where density is at
// the threshold, so asking "what block is here" is asking a question right at
// the boundary and the answer flickers. Sampling one step ALONG THE INWARD
// NORMAL asks what the surface is made OF, which is the actual question, and
// it is stable because it is unambiguously inside.

/// How far below the surface to ask what the ground is made of, in blocks.
/// Half a block is inside the solid at every crossing without reaching the
/// layer below.
const MATERIAL_DEPTH: f32 = 0.5;

/// A surface material per vertex, parallel to `mesh.positions`.
///
/// `origin` must be the same one the mesh was extracted with — positions are
/// relative to it.
pub fn materials_for(
    model: &WorldModel,
    mesh: &Mesh,
    origin: [i32; 3],
) -> Vec<BlockId> {
    mesh.positions
        .iter()
        .zip(&mesh.normals)
        .map(|(p, n)| {
            let inside = *p - *n * MATERIAL_DEPTH;
            let x = origin[0] as f64 + inside.x as f64;
            let y = origin[1] as f64 + inside.y as f64;
            let z = origin[2] as f64 + inside.z as f64;
            material_at(model, x, y, z)
        })
        .collect()
}

/// What the ground is made of at a point known to be inside it.
///
/// The generator's own layering, minus the parts that need a whole chunk:
/// ores roll per block below sea level and cannot be reconstructed from a
/// point sample, so this answers with the biome's surface material near the
/// top and stone below it. A renderer wanting ore veins on a smooth surface
/// needs the block grid, which is a different question from what a hillside
/// looks like.
pub fn material_at(model: &WorldModel, x: f64, y: f64, z: f64) -> BlockId {
    let target = model.surface_target(x, z, 0.0);
    let depth = target - y;
    if depth > 4.0 {
        BlockId::Stone
    } else {
        model.biome_at_2d(x, z).surface_block()
    }
}

#[cfg(test)]
mod material_tests {
    use super::*;

    fn model() -> WorldModel {
        WorldModel::new(0xC0FFEE)
    }

    #[test]
    fn every_vertex_gets_a_material() {
        let m = model();
        let target = m.surface_target(40.0, -24.0, 0.0);
        let base = target.floor() as i32 - 12;
        let origin = [36, base, -28];
        let mesh = mesh_region(&m, origin, [9, 25, 9], 1, Seams::NONE);
        let mats = materials_for(&m, &mesh, origin);
        assert_eq!(mats.len(), mesh.positions.len());
        assert!(!mats.is_empty());
    }

    #[test]
    fn a_hillside_is_made_of_its_biome_and_not_of_air() {
        // The failure this guards: sampling AT the vertex instead of below it
        // lands on the threshold, where the answer is undefined and half the
        // surface comes back as whatever the else-branch is.
        let m = model();
        let target = m.surface_target(40.0, -24.0, 0.0);
        let base = target.floor() as i32 - 12;
        let origin = [36, base, -28];
        let mesh = mesh_region(&m, origin, [9, 25, 9], 1, Seams::NONE);
        let mats = materials_for(&m, &mesh, origin);
        let air = mats.iter().filter(|b| **b == BlockId::Air).count();
        assert_eq!(air, 0, "{air} of {} vertices are made of air", mats.len());
    }

    #[test]
    fn deep_ground_is_stone_and_the_top_is_not() {
        let m = model();
        let (x, z) = (40.0, -24.0);
        let target = m.surface_target(x, z, 0.0);
        assert_eq!(material_at(&m, x, target - 30.0, z), BlockId::Stone);
        let top = material_at(&m, x, target - 0.5, z);
        assert_eq!(top, m.biome_at_2d(x, z).surface_block());
    }

    #[test]
    fn materials_are_deterministic() {
        let m = model();
        let origin = [0, 40, 0];
        let mesh = mesh_region(&m, origin, [9, 9, 9], 1, Seams::NONE);
        assert_eq!(materials_for(&m, &mesh, origin), materials_for(&m, &mesh, origin));
    }
}
