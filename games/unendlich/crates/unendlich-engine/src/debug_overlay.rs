//! In-world debug visualization — markers, lines, AABBs drawn in
//! world space to show real-time engine adaptations.
//!
//! ## Why
//!
//! The user asked: "add a test that does world + physics engine UI
//! components/markers placed in world to show off realtime
//! adaptations." This module is the foundation for that.
//!
//! Currently, a lot of the engine's dynamic behavior is invisible:
//! plant lifecycle stages produce subtle color shifts; wind direction
//! affects player drift but isn't shown; the physics body's velocity
//! is invisible; tick culling decides which chunks are simulating but
//! gives no indicator. Debug markers expose all of this.
//!
//! ## Design
//!
//! `DebugScene` is a per-frame container. Each frame the bin clears
//! it and re-populates: walk the world for current state, queue
//! markers/arrows/AABBs that visualize that state. The renderer reads
//! the scene at draw time and emits geometry.
//!
//! Primitives:
//!   - `Marker`: a small colored cube at a world position. Use for
//!     "this plant is in stage X", "this chunk is being ticked",
//!     "this entity is here".
//!   - `Arrow`: a directional indicator. Use for velocities, wind,
//!     gradients. Drawn as a thin elongated cube along the direction
//!     vector.
//!   - `Aabb`: a wireframe box. Use for entity collision shapes,
//!     selection highlights, trigger volumes.
//!
//! Today there's no transparent / line-renderer pipeline, so all
//! primitives are rendered as small solid cubes. Lines become tiny
//! oblong cubes, AABBs become 12 tiny edge cubes. Slightly hacky but
//! works inside the existing chunk-mesh pipeline. A real
//! line-renderer pipeline can come later.
//!
//! ## Determinism note for rollback
//!
//! Debug markers are PURELY VISUAL. They don't affect simulation.
//! Don't snapshot them, don't rollback them, don't sync them across
//! the network. They're recomputed every frame from current sim state.

/// A colored marker at a world position. Drawn as a small cube
/// (typically 0.2-block side length).
#[derive(Debug, Clone, Copy)]
pub struct Marker {
    pub world_pos: [f32; 3],
    pub color: [f32; 3],
    pub size: f32,
}

/// A directional arrow in world space. Drawn as a thin oblong cube
/// from `origin` extending along `dir` for `length` blocks.
#[derive(Debug, Clone, Copy)]
pub struct Arrow {
    pub origin: [f32; 3],
    /// Direction vector (need not be normalized — internal logic does so).
    pub dir: [f32; 3],
    pub length: f32,
    pub color: [f32; 3],
    /// Thickness in blocks. Typical: 0.05 - 0.1.
    pub thickness: f32,
}

/// Axis-aligned bounding box outline.
#[derive(Debug, Clone, Copy)]
pub struct AabbOutline {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub color: [f32; 3],
    /// Edge thickness in blocks.
    pub thickness: f32,
}

/// Per-frame bag of debug visuals. The bin clears this each frame and
/// re-populates. The renderer reads from it at draw time.
#[derive(Debug, Default)]
pub struct DebugScene {
    pub markers: Vec<Marker>,
    pub arrows: Vec<Arrow>,
    pub aabbs: Vec<AabbOutline>,
    /// Master toggle — when false, nothing renders. Bound to a
    /// keyboard toggle (default F4).
    pub enabled: bool,
}

impl DebugScene {
    pub fn new() -> Self { Self::default() }

    /// Clear all primitives. Call at start of each frame before
    /// re-populating.
    pub fn clear(&mut self) {
        self.markers.clear();
        self.arrows.clear();
        self.aabbs.clear();
    }

    pub fn add_marker(&mut self, world_pos: [f32; 3], color: [f32; 3], size: f32) {
        self.markers.push(Marker { world_pos, color, size });
    }

    pub fn add_arrow(&mut self, origin: [f32; 3], dir: [f32; 3], length: f32,
        color: [f32; 3], thickness: f32)
    {
        self.arrows.push(Arrow { origin, dir, length, color, thickness });
    }

    pub fn add_aabb(&mut self, min: [f32; 3], max: [f32; 3], color: [f32; 3], thickness: f32) {
        self.aabbs.push(AabbOutline { min, max, color, thickness });
    }

    pub fn primitive_count(&self) -> usize {
        self.markers.len() + self.arrows.len() + self.aabbs.len()
    }
}

// ============================================================================
// Mesh builder
// ============================================================================

/// Build a `ChunkMesh` representing the debug scene. The output mesh
/// can be rendered through the existing chunk-render pipeline as a
/// synthetic chunk — no separate pipeline required.
///
/// All primitives become small solid cubes. Lines/arrows are oblong
/// cubes oriented along their direction. AABB outlines are 12 thin
/// edge cubes (the box's edges).
///
/// Returns an empty mesh if the scene is disabled or empty.
#[cfg(feature = "render-core")]
pub fn build_debug_mesh(scene: &DebugScene) -> crate::render::mesher::ChunkMesh {
    let mut mesh = crate::render::mesher::ChunkMesh::empty();
    if !scene.enabled { return mesh; }

    for m in &scene.markers {
        let half = m.size * 0.5;
        let lo = [m.world_pos[0] - half, m.world_pos[1] - half, m.world_pos[2] - half];
        let hi = [m.world_pos[0] + half, m.world_pos[1] + half, m.world_pos[2] + half];
        emit_cube(&mut mesh, lo, hi, m.color);
    }

    for a in &scene.arrows {
        // Normalize direction.
        let dlen = (a.dir[0]*a.dir[0] + a.dir[1]*a.dir[1] + a.dir[2]*a.dir[2]).sqrt();
        if dlen < 1e-6 { continue; }
        let n = [a.dir[0]/dlen, a.dir[1]/dlen, a.dir[2]/dlen];
        // Build an axis-aligned bounding box from origin to origin+n*length,
        // then thicken perpendicular to the dominant axis. Cheap proxy
        // for an oriented box; gives a recognizable arrow shape for
        // arrows mostly along an axis (which is most use cases).
        let tip = [
            a.origin[0] + n[0] * a.length,
            a.origin[1] + n[1] * a.length,
            a.origin[2] + n[2] * a.length,
        ];
        let half = a.thickness * 0.5;
        let lo = [
            a.origin[0].min(tip[0]) - half,
            a.origin[1].min(tip[1]) - half,
            a.origin[2].min(tip[2]) - half,
        ];
        let hi = [
            a.origin[0].max(tip[0]) + half,
            a.origin[1].max(tip[1]) + half,
            a.origin[2].max(tip[2]) + half,
        ];
        emit_cube(&mut mesh, lo, hi, a.color);
        // Add a slightly bigger cube at the tip to suggest an arrowhead.
        let tip_half = a.thickness * 1.5;
        emit_cube(
            &mut mesh,
            [tip[0] - tip_half, tip[1] - tip_half, tip[2] - tip_half],
            [tip[0] + tip_half, tip[1] + tip_half, tip[2] + tip_half],
            a.color,
        );
    }

    for b in &scene.aabbs {
        emit_aabb_outline(&mut mesh, b.min, b.max, b.color, b.thickness);
    }

    mesh
}

/// Emit a solid cube as 6 quads (12 triangles, 24 vertices) in the
/// chunk-mesh format. Front-face winding follows the chunk pipeline's
/// convention (CW due to the projection Y-flip).
#[cfg(feature = "render-core")]
fn emit_cube(
    mesh: &mut crate::render::mesher::ChunkMesh,
    lo: [f32; 3],
    hi: [f32; 3],
    color: [f32; 3],
) {
    use crate::render::mesher::Vertex;

    let v = |p: [f32; 3]| Vertex::new(p, color);
    let push_quad = |mesh: &mut crate::render::mesher::ChunkMesh,
                     a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]|
    {
        let base = mesh.vertices.len() as u32;
        mesh.vertices.push(v(a));
        mesh.vertices.push(v(b));
        mesh.vertices.push(v(c));
        mesh.vertices.push(v(d));
        // Two CW triangles. Same winding as the main chunk mesher.
        mesh.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    };

    let (x0, y0, z0) = (lo[0], lo[1], lo[2]);
    let (x1, y1, z1) = (hi[0], hi[1], hi[2]);

    // +Y top
    push_quad(mesh, [x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]);
    // -Y bottom
    push_quad(mesh, [x0, y0, z1], [x1, y0, z1], [x1, y0, z0], [x0, y0, z0]);
    // +X east
    push_quad(mesh, [x1, y0, z0], [x1, y0, z1], [x1, y1, z1], [x1, y1, z0]);
    // -X west
    push_quad(mesh, [x0, y0, z1], [x0, y0, z0], [x0, y1, z0], [x0, y1, z1]);
    // +Z south
    push_quad(mesh, [x1, y0, z1], [x0, y0, z1], [x0, y1, z1], [x1, y1, z1]);
    // -Z north
    push_quad(mesh, [x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0]);
}

/// Emit the 12 edges of an AABB as thin oblong cubes.
#[cfg(feature = "render-core")]
fn emit_aabb_outline(
    mesh: &mut crate::render::mesher::ChunkMesh,
    min: [f32; 3],
    max: [f32; 3],
    color: [f32; 3],
    t: f32,
) {
    let h = t * 0.5;
    // 12 edges = 4 along each axis.
    // X-aligned edges (at the 4 (y, z) corners of the box):
    for &(y, z) in &[(min[1], min[2]), (max[1], min[2]), (min[1], max[2]), (max[1], max[2])] {
        emit_cube(
            mesh,
            [min[0] - h, y - h, z - h],
            [max[0] + h, y + h, z + h],
            color,
        );
    }
    // Y-aligned edges (at the 4 (x, z) corners):
    for &(x, z) in &[(min[0], min[2]), (max[0], min[2]), (min[0], max[2]), (max[0], max[2])] {
        emit_cube(
            mesh,
            [x - h, min[1] - h, z - h],
            [x + h, max[1] + h, z + h],
            color,
        );
    }
    // Z-aligned edges (at the 4 (x, y) corners):
    for &(x, y) in &[(min[0], min[1]), (max[0], min[1]), (min[0], max[1]), (max[0], max[1])] {
        emit_cube(
            mesh,
            [x - h, y - h, min[2] - h],
            [x + h, y + h, max[2] + h],
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_clears_all_primitives() {
        let mut s = DebugScene::new();
        s.add_marker([1.0, 2.0, 3.0], [1.0, 0.0, 0.0], 0.2);
        s.add_arrow([0.0; 3], [1.0, 0.0, 0.0], 1.0, [0.0, 1.0, 0.0], 0.05);
        s.add_aabb([0.0; 3], [1.0; 3], [0.0, 0.0, 1.0], 0.05);
        assert_eq!(s.primitive_count(), 3);
        s.clear();
        assert_eq!(s.primitive_count(), 0);
    }

    #[test]
    fn enabled_defaults_to_false() {
        let s = DebugScene::new();
        assert!(!s.enabled, "debug overlay should start hidden");
    }

    #[cfg(feature = "render-core")]
    #[test]
    fn build_debug_mesh_is_empty_when_disabled() {
        let mut s = DebugScene::new();
        s.add_marker([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 0.2);
        // enabled defaults to false
        let mesh = build_debug_mesh(&s);
        assert!(mesh.is_empty(), "disabled scene should produce no geometry");
    }

    #[cfg(feature = "render-core")]
    #[test]
    fn build_debug_mesh_emits_cube_per_marker() {
        let mut s = DebugScene::new();
        s.enabled = true;
        s.add_marker([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 0.2);
        let mesh = build_debug_mesh(&s);
        // Each cube = 6 faces × 4 verts = 24 verts, 36 indices.
        assert_eq!(mesh.vertex_count(), 24);
        assert_eq!(mesh.index_count(), 36);
    }

    #[cfg(feature = "render-core")]
    #[test]
    fn build_debug_mesh_emits_geometry_per_arrow() {
        let mut s = DebugScene::new();
        s.enabled = true;
        s.add_arrow([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 1.0, [0.0, 1.0, 0.0], 0.05);
        let mesh = build_debug_mesh(&s);
        // Arrow = shaft cube + tip cube = 48 verts.
        assert_eq!(mesh.vertex_count(), 48);
    }

    #[cfg(feature = "render-core")]
    #[test]
    fn build_debug_mesh_emits_12_edges_per_aabb() {
        let mut s = DebugScene::new();
        s.enabled = true;
        s.add_aabb([0.0; 3], [1.0; 3], [1.0, 1.0, 1.0], 0.05);
        let mesh = build_debug_mesh(&s);
        // 12 edges × 24 verts/cube = 288 verts.
        assert_eq!(mesh.vertex_count(), 12 * 24);
    }
}
