//! `cave_network` — connect cave seeds into a tunnel network.
//!
//! Worker-03 / WORLD-PERF pure core, round 9. Given scattered cave seed points
//! (e.g. `giant_cave` centers), build a minimum spanning tree of nearest links
//! so every cave is reachable through tunnels without a tangle of redundant
//! connections. Prim's algorithm over squared Euclidean distance, with
//! lowest-index tiebreaks for determinism.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Connect `seeds` into a spanning tree, returning `n-1` edges as ordered,
/// unique index pairs `(a, b)` with `a < b`. Fewer than two seeds → no edges.
pub fn connect(seeds: &[[i32; 3]]) -> Vec<(usize, usize)> {
    let n = seeds.len();
    let mut edges: Vec<(usize, usize)> = Vec::new();
    if n < 2 {
        return edges;
    }

    let mut in_tree = vec![false; n];
    in_tree[0] = true;
    let mut count = 1;

    while count < n {
        // Cheapest edge from a tree node to a non-tree node.
        let mut best: Option<(i128, usize, usize)> = None; // (dist2, tree_a, new_b)
        for a in 0..n {
            if !in_tree[a] {
                continue;
            }
            for b in 0..n {
                if in_tree[b] {
                    continue;
                }
                let d = dist2(seeds[a], seeds[b]);
                let take = match best {
                    None => true,
                    Some((bd, ba, bb)) => {
                        d < bd || (d == bd && (b < bb || (b == bb && a < ba)))
                    }
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

fn dist2(a: [i32; 3], b: [i32; 3]) -> i128 {
    let dx = (a[0] as i128) - (b[0] as i128);
    let dy = (a[1] as i128) - (b[1] as i128);
    let dz = (a[2] as i128) - (b[2] as i128);
    dx * dx + dy * dy + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;

    // Union-find connectivity check.
    fn connected(n: usize, edges: &[(usize, usize)]) -> bool {
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

    fn sample(n: usize) -> Vec<[i32; 3]> {
        // deterministic scattered points
        (0..n)
            .map(|i| {
                let i = i as i32;
                [(i * 37) % 100, (i * 53) % 80, (i * 17) % 60]
            })
            .collect()
    }

    #[test]
    fn n_seeds_give_n_minus_one_edges() {
        for n in 2..12 {
            assert_eq!(connect(&sample(n)).len(), n - 1);
        }
    }

    #[test]
    fn single_or_empty_has_no_edges() {
        assert!(connect(&[]).is_empty());
        assert!(connect(&[[1, 2, 3]]).is_empty());
    }

    #[test]
    fn no_self_edges() {
        for &(a, b) in &connect(&sample(10)) {
            assert_ne!(a, b);
        }
    }

    #[test]
    fn graph_is_connected() {
        let s = sample(9);
        assert!(connected(s.len(), &connect(&s)));
    }

    #[test]
    fn pairs_unique_and_ordered() {
        let edges = connect(&sample(11));
        for &(a, b) in &edges {
            assert!(a < b, "edge not ordered");
        }
        let mut sorted = edges.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), edges.len(), "duplicate edge present");
    }

    #[test]
    fn deterministic() {
        let s = sample(8);
        assert_eq!(connect(&s), connect(&s));
    }

    #[test]
    fn picks_nearest_link_for_a_simple_line() {
        // Colinear points: MST is the chain of adjacent links.
        let s = [[0, 0, 0], [10, 0, 0], [20, 0, 0], [30, 0, 0]];
        let mut e = connect(&s);
        e.sort();
        assert_eq!(e, vec![(0, 1), (1, 2), (2, 3)]);
    }
}
