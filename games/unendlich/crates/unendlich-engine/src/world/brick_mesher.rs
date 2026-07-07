//! Per-brick greedy mesher — the piece that turns the `mesh_brick` dirty-tracking
//! into a cheap, bounded re-mesh. When a brick is dirty, mesh just that 8³ region
//! instead of re-baking the whole chunk, so the near-LOD re-mesh that spikes to
//! 100-200ms+ becomes a sub-millisecond brick mesh. It's also a correct, tested
//! CPU reference the GPU mesher can be checked against (OCTREE_MESH_VERIFY).
//!
//! Opaque face culling + greedy 2D merge, matching the engine's face order
//! (+Y, -Y, +X, -X, +Z, -Z) and air convention (block id 0 = Air = empty). The
//! caller supplies solidity for any local coord in `[-1, BRICK]` so boundary
//! faces cull correctly against neighbouring bricks/chunks.
//!
//! Pure, deterministic.

use crate::world::mesh_brick::BRICK_SIZE;

const B: i32 = BRICK_SIZE; // 8

/// A greedy-merged quad. `pos` is the brick-local voxel the face sits on; `du`/`dv`
/// are its extents along the face's two in-plane axes (see [`face_axes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quad {
    pub dir: u8, // 0..6, engine face order
    pub block: u16,
    pub pos: [i32; 3],
    pub du: i32,
    pub dv: i32,
}

/// (normal axis 0=x/1=y/2=z, sign, u-axis, v-axis) for each face dir.
fn face_axes(dir: u8) -> (usize, i32, usize, usize) {
    match dir {
        0 => (1, 1, 0, 2),  // +Y top
        1 => (1, -1, 0, 2), // -Y bottom
        2 => (0, 1, 2, 1),  // +X east
        3 => (0, -1, 2, 1), // -X west
        4 => (2, 1, 0, 1),  // +Z south
        _ => (2, -1, 0, 1), // -Z north
    }
}

fn unit(axis: usize) -> [i32; 3] {
    let mut u = [0, 0, 0];
    u[axis] = 1;
    u
}

/// Mesh one brick. `block_at(local)` returns the block id at a brick-local coord;
/// it must answer for any component in `-1..=BRICK` (the one-voxel border lets
/// boundary faces cull against neighbours). Air (0) is empty.
pub fn mesh_brick(block_at: impl Fn([i32; 3]) -> u16) -> Vec<Quad> {
    let mut quads = Vec::new();
    for dir in 0u8..6 {
        let (na, sign, ua, va) = face_axes(dir);
        for s in 0..B {
            // Build the exposed-face mask for this slice: mask[u][v] = Some(id).
            let mut mask = [[None::<u16>; B as usize]; B as usize];
            for ui in 0..B {
                for vi in 0..B {
                    let mut p = [0i32; 3];
                    p[na] = s;
                    p[ua] = ui;
                    p[va] = vi;
                    let here = block_at(p);
                    if here == 0 {
                        continue; // empty cell, no face
                    }
                    let mut n = p;
                    n[na] += sign;
                    if block_at(n) == 0 {
                        mask[ui as usize][vi as usize] = Some(here); // exposed to air
                    }
                }
            }
            greedy_merge(&mask, |u0, v0, du, dv, id| {
                let mut pos = [0i32; 3];
                pos[na] = s;
                pos[ua] = u0;
                pos[va] = v0;
                quads.push(Quad { dir, block: id, pos, du, dv });
            });
        }
    }
    quads
}

/// Greedy-merge an 8×8 mask of `Option<id>` into maximal same-id rectangles,
/// calling `emit(u0, v0, du, dv, id)` for each.
fn greedy_merge(mask: &[[Option<u16>; B as usize]; B as usize], mut emit: impl FnMut(i32, i32, i32, i32, u16)) {
    let n = B as usize;
    let mut done = [[false; B as usize]; B as usize];
    for v0 in 0..n {
        for u0 in 0..n {
            if done[u0][v0] {
                continue;
            }
            let Some(id) = mask[u0][v0] else {
                done[u0][v0] = true;
                continue;
            };
            // Extend width along u.
            let mut du = 1;
            while u0 + du < n && !done[u0 + du][v0] && mask[u0 + du][v0] == Some(id) {
                du += 1;
            }
            // Extend height along v while the whole strip matches.
            let mut dv = 1;
            'grow: while v0 + dv < n {
                for k in 0..du {
                    if done[u0 + k][v0 + dv] || mask[u0 + k][v0 + dv] != Some(id) {
                        break 'grow;
                    }
                }
                dv += 1;
            }
            for du_i in 0..du {
                for dv_i in 0..dv {
                    done[u0 + du_i][v0 + dv_i] = true;
                }
            }
            emit(u0 as i32, v0 as i32, du as i32, dv as i32, id);
        }
    }
}

/// Expand quads back into the set of unit faces they cover — `(voxel, dir)` — for
/// verification.
pub fn unit_faces(quads: &[Quad]) -> std::collections::HashSet<([i32; 3], u8)> {
    let mut set = std::collections::HashSet::new();
    for q in quads {
        let (_, _, ua, va) = face_axes(q.dir);
        let (uu, uv) = (unit(ua), unit(va));
        for i in 0..q.du {
            for j in 0..q.dv {
                let p = [
                    q.pos[0] + uu[0] * i + uv[0] * j,
                    q.pos[1] + uu[1] * i + uv[1] * j,
                    q.pos[2] + uu[2] * i + uv[2] * j,
                ];
                set.insert((p, q.dir));
            }
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Naive (unmerged) exposed faces, the ground truth greedy must match.
    fn naive_faces(block_at: impl Fn([i32; 3]) -> u16) -> HashSet<([i32; 3], u8)> {
        let mut set = HashSet::new();
        for x in 0..B {
            for y in 0..B {
                for z in 0..B {
                    let p = [x, y, z];
                    if block_at(p) == 0 {
                        continue;
                    }
                    for dir in 0u8..6 {
                        let (na, sign, _, _) = face_axes(dir);
                        let mut n = p;
                        n[na] += sign;
                        if block_at(n) == 0 {
                            set.insert((p, dir));
                        }
                    }
                }
            }
        }
        set
    }

    #[test]
    fn lone_voxel_in_air_has_six_faces() {
        let q = mesh_brick(|p| if p == [3, 3, 3] { 1 } else { 0 });
        assert_eq!(q.len(), 6, "all six faces exposed");
        assert!(q.iter().all(|q| q.du == 1 && q.dv == 1));
    }

    #[test]
    fn brick_buried_in_solid_emits_nothing() {
        // Everything solid, including the border → every face culled.
        let q = mesh_brick(|_| 1);
        assert!(q.is_empty(), "fully buried brick has no visible faces");
    }

    #[test]
    fn open_top_merges_to_one_quad() {
        // Brick solid inside; neighbours solid except the +Y border (air above).
        let q = mesh_brick(|p| {
            let inside = (0..B).contains(&p[0]) && (0..B).contains(&p[1]) && (0..B).contains(&p[2]);
            if inside {
                1
            } else if p[1] == B {
                0 // air directly above the brick
            } else {
                1 // solid on every other side
            }
        });
        assert_eq!(q.len(), 1, "the whole top merges to a single quad");
        assert_eq!(q[0].dir, 0);
        assert_eq!((q[0].du, q[0].dv), (B, B), "8x8 merged");
    }

    #[test]
    fn greedy_covers_exactly_the_same_faces_as_naive() {
        // A non-trivial shape (a diagonal ramp), border all air.
        let shape = |p: [i32; 3]| -> u16 {
            if (0..B).contains(&p[0]) && (0..B).contains(&p[1]) && (0..B).contains(&p[2]) && p[1] <= p[0] {
                1
            } else {
                0
            }
        };
        let greedy = unit_faces(&mesh_brick(shape));
        let naive = naive_faces(shape);
        assert_eq!(greedy, naive, "greedy merge must cover exactly the exposed faces — no more, no less");
    }

    #[test]
    fn different_block_ids_do_not_merge_across_their_boundary() {
        // A flat solid layer at y=0, half id 1 / half id 2, open above.
        let shape = |p: [i32; 3]| -> u16 {
            if p[1] != 0 || !(0..B).contains(&p[0]) || !(0..B).contains(&p[2]) {
                return 0;
            }
            if p[0] < 4 {
                1
            } else {
                2
            }
        };
        let top: Vec<&Quad> = mesh_brick(shape).into_iter().filter(|q| q.dir == 0).collect::<Vec<_>>().leak().iter().collect();
        // Two top quads (one per id), not one merged across the colour change.
        assert_eq!(top.len(), 2, "ids 1 and 2 stay separate quads");
        assert!(top.iter().any(|q| q.block == 1) && top.iter().any(|q| q.block == 2));
    }

    #[test]
    fn merging_actually_reduces_quad_count() {
        // A solid floor (y=0 layer), open above → naive 64 top faces, greedy 1.
        let shape = |p: [i32; 3]| -> u16 {
            if p[1] == 0 && (0..B).contains(&p[0]) && (0..B).contains(&p[2]) { 1 } else { 0 }
        };
        let top = mesh_brick(shape).into_iter().filter(|q| q.dir == 0).count();
        assert_eq!(top, 1, "64 unit faces merge to a single quad");
    }
}
