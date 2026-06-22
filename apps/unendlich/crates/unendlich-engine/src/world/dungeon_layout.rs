//! `dungeon_layout` — rooms + connecting corridors.
//!
//! Worker-03 / WORLD-PERF pure core, round 10. Places up to `n` non-overlapping
//! rooms inside a bounds rectangle (deterministic rejection sampling from a
//! seed) and connects their centers with a corridor tree (Prim MST), so every
//! room is reachable with no redundant tunnels. Pairs with `structure_grammar`
//! for the decorative pass.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const MIN_SIZE: i32 = 3;
const MAX_SIZE: i32 = 10;
const ATTEMPTS_PER_ROOM: u64 = 16;

/// An axis-aligned room: top-left `(x, z)`, size `w × h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Room {
    pub x: i32,
    pub z: i32,
    pub w: i32,
    pub h: i32,
}

/// Lay out up to `n` rooms within a `bw × bh` bounds and a corridor tree over
/// them. Rooms never overlap (kept ≥ 1 cell apart); the tree has
/// `rooms.len()-1` ordered, unique edges. Degenerate inputs yield empty output.
pub fn layout(seed: u64, n: u32, bw: i32, bh: i32) -> (Vec<Room>, Vec<(usize, usize)>) {
    let mut rooms: Vec<Room> = Vec::new();
    if n == 0 || bw <= 0 || bh <= 0 {
        return (rooms, Vec::new());
    }

    for i in 0..n as u64 {
        for attempt in 0..ATTEMPTS_PER_ROOM {
            let base = hash(seed ^ i.wrapping_mul(0x9E37_79B9) ^ attempt.wrapping_mul(0x85EB_CA77));
            let rw = MIN_SIZE + (span_pick(base, MAX_SIZE - MIN_SIZE + 1));
            let rh = MIN_SIZE + (span_pick(base >> 16, MAX_SIZE - MIN_SIZE + 1));
            if rw > bw || rh > bh {
                continue;
            }
            let rx = span_pick(base >> 32, bw - rw + 1);
            let rz = span_pick(base >> 48, bh - rh + 1);
            let cand = Room { x: rx, z: rz, w: rw, h: rh };
            if rooms.iter().all(|r| !overlaps_with_gap(r, &cand)) {
                rooms.push(cand);
                break;
            }
        }
    }

    let centers: Vec<[i32; 2]> = rooms
        .iter()
        .map(|r| [r.x + r.w / 2, r.z + r.h / 2])
        .collect();
    let edges = mst(&centers);
    (rooms, edges)
}

/// AABB overlap test with a 1-cell separation margin.
fn overlaps_with_gap(a: &Room, b: &Room) -> bool {
    a.x - 1 < b.x + b.w && b.x - 1 < a.x + a.w && a.z - 1 < b.z + b.h && b.z - 1 < a.z + a.h
}

/// Prim MST over 2D points → ordered, unique index pairs.
fn mst(pts: &[[i32; 2]]) -> Vec<(usize, usize)> {
    let n = pts.len();
    let mut edges = Vec::new();
    if n < 2 {
        return edges;
    }
    let mut in_tree = vec![false; n];
    in_tree[0] = true;
    let mut count = 1;
    while count < n {
        let mut best: Option<(i64, usize, usize)> = None;
        for a in 0..n {
            if !in_tree[a] {
                continue;
            }
            for b in 0..n {
                if in_tree[b] {
                    continue;
                }
                let d = dist2(pts[a], pts[b]);
                let take = match best {
                    None => true,
                    Some((bd, ba, bb)) => d < bd || (d == bd && (b < bb || (b == bb && a < ba))),
                };
                if take {
                    best = Some((d, a, b));
                }
            }
        }
        match best {
            Some((_, a, b)) => {
                edges.push(if a < b { (a, b) } else { (b, a) });
                in_tree[b] = true;
                count += 1;
            }
            None => break,
        }
    }
    edges
}

fn dist2(a: [i32; 2], b: [i32; 2]) -> i64 {
    let dx = (a[0] - b[0]) as i64;
    let dz = (a[1] - b[1]) as i64;
    dx * dx + dz * dz
}

fn span_pick(h: u64, span: i32) -> i32 {
    if span <= 1 {
        0
    } else {
        (h % span as u64) as i32
    }
}

fn hash(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aabb_overlap(a: &Room, b: &Room) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.z < b.z + b.h && b.z < a.z + a.h
    }

    fn connected(n: usize, edges: &[(usize, usize)]) -> bool {
        if n == 0 {
            return true;
        }
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut Vec<usize>, mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        for &(a, b) in edges {
            let ra = find(&mut parent, a);
            let rb = find(&mut parent, b);
            parent[ra] = rb;
        }
        let r0 = find(&mut parent, 0);
        (0..n).all(|i| find(&mut parent, i) == r0)
    }

    #[test]
    fn rooms_do_not_overlap() {
        let (rooms, _) = layout(0xDEAD, 20, 80, 80);
        for i in 0..rooms.len() {
            for j in (i + 1)..rooms.len() {
                assert!(!aabb_overlap(&rooms[i], &rooms[j]), "rooms overlap");
            }
        }
    }

    #[test]
    fn at_most_n_rooms_all_in_bounds() {
        let (bw, bh) = (60, 50);
        let (rooms, _) = layout(7, 12, bw, bh);
        assert!(rooms.len() <= 12);
        for r in &rooms {
            assert!(r.x >= 0 && r.z >= 0);
            assert!(r.x + r.w <= bw && r.z + r.h <= bh, "room out of bounds");
        }
    }

    #[test]
    fn corridors_connect_every_room() {
        let (rooms, edges) = layout(0xBEEF, 15, 90, 90);
        assert!(connected(rooms.len(), &edges));
        if rooms.len() >= 2 {
            assert_eq!(edges.len(), rooms.len() - 1);
        }
    }

    #[test]
    fn no_self_corridor() {
        let (_, edges) = layout(123, 18, 80, 80);
        for &(a, b) in &edges {
            assert_ne!(a, b);
            assert!(a < b);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(layout(99, 14, 70, 70), layout(99, 14, 70, 70));
    }

    #[test]
    fn degenerate_inputs_handled() {
        assert_eq!(layout(1, 0, 50, 50), (vec![], vec![]));
        let (rooms, edges) = layout(1, 1, 50, 50);
        assert!(rooms.len() <= 1);
        assert!(edges.is_empty());
        assert_eq!(layout(1, 5, 0, 50), (vec![], vec![])); // zero bounds
    }
}
