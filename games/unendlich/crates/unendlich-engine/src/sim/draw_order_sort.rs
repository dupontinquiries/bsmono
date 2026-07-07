//! `sim::draw_order_sort` — draw-call ordering for the draw-bound path (R35).
//!
//! On M2 the playable frame is draw-bound, so ordering draws to (a) group by
//! GPU pipeline then LOD — minimizing pipeline/state rebinds — and (b) render
//! front-to-back within a group (ascending squared camera distance → early-z,
//! less overdraw) is a direct win. No draw-order core existed (verified gap).
//!
//! [`order`] writes a permutation of indices into a caller-owned `out` buffer.
//! Intended to be run over only the *visible* set each frame (pairs with the
//! R33 `visible_delta` + `chunk_frustum_cull` work), so the sort cost stays on
//! the chunks actually drawn.
//!
//! Implementation note: the sort key is `(pipeline, lod, dist2, index)` with the
//! original index as the final tiebreak. Since indices are unique the ordering
//! is total, so `sort_unstable_by` (which allocates no scratch buffer — "no
//! allocation beyond `out`") is fully deterministic and equal to a stable sort:
//! items with identical `(pipeline, lod, dist2)` keep their input order.
//!
//! Pure: logic in, data out — no `crate::render`, no GPU, no I/O, no clock, no
//! RNG, no `.unwrap()`/`.expect()` in non-test code.

/// One drawable, reduced to its sort keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawItem {
    /// GPU pipeline id — primary group key (fewest rebinds).
    pub pipeline: u16,
    /// LOD level — secondary group key.
    pub lod: u8,
    /// Squared camera distance — front-to-back within a group (early-z).
    pub dist2: u32,
    /// Caller's payload handle (carried through, not used as a sort key).
    pub slot: u32,
}

/// Write into `out` (cleared first) a permutation of indices `0..items.len()`
/// that groups by `pipeline` then `lod`, and orders ascending `dist2` within a
/// group. Stable for fully-equal keys. No allocation beyond `out`.
pub fn order(items: &[DrawItem], out: &mut Vec<u32>) {
    out.clear();
    out.reserve(items.len());
    let mut i = 0u32;
    while (i as usize) < items.len() {
        out.push(i);
        i += 1;
    }
    out.sort_unstable_by(|&a, &b| {
        let ia = &items[a as usize];
        let ib = &items[b as usize];
        // index `a`/`b` as the final tiebreak ⇒ total order ⇒ stable result.
        (ia.pipeline, ia.lod, ia.dist2, a).cmp(&(ib.pipeline, ib.lod, ib.dist2, b))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(pipeline: u16, lod: u8, dist2: u32, slot: u32) -> DrawItem {
        DrawItem { pipeline, lod, dist2, slot }
    }
    fn lcg(s: &mut u32) -> u32 {
        *s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *s
    }
    fn is_permutation(out: &[u32], n: usize) -> bool {
        if out.len() != n {
            return false;
        }
        let mut seen = vec![false; n];
        for &i in out {
            let i = i as usize;
            if i >= n || seen[i] {
                return false;
            }
            seen[i] = true;
        }
        seen.iter().all(|&b| b)
    }
    // out is correctly ordered iff the keys it indexes are non-decreasing.
    fn keys_sorted(items: &[DrawItem], out: &[u32]) -> bool {
        out.windows(2).all(|w| {
            let a = &items[w[0] as usize];
            let b = &items[w[1] as usize];
            (a.pipeline, a.lod, a.dist2, w[0]) <= (b.pipeline, b.lod, b.dist2, w[1])
        })
    }

    #[test]
    fn out_is_cleared_first() {
        let items = [item(1, 0, 5, 100), item(0, 0, 5, 101)];
        let mut out = vec![42u32, 7, 7, 7, 99]; // junk, wrong length
        order(&items, &mut out);
        assert_eq!(out.len(), 2);
        assert!(is_permutation(&out, 2));
    }

    #[test]
    fn empty_is_empty() {
        let mut out = vec![1u32, 2, 3];
        order(&[], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn len_and_permutation() {
        let items = [
            item(2, 1, 30, 0),
            item(0, 3, 10, 1),
            item(2, 0, 20, 2),
            item(0, 3, 5, 3),
            item(1, 0, 99, 4),
        ];
        let mut out = Vec::new();
        order(&items, &mut out);
        assert!(is_permutation(&out, items.len()));
    }

    #[test]
    fn grouped_by_pipeline_then_lod_then_dist2() {
        let items = [
            item(2, 1, 30, 0),
            item(0, 3, 10, 1),
            item(2, 0, 20, 2),
            item(0, 3, 5, 3),
            item(1, 0, 99, 4),
            item(0, 1, 50, 5),
        ];
        let mut out = Vec::new();
        order(&items, &mut out);
        assert!(keys_sorted(&items, &out), "out must index non-decreasing (pipeline,lod,dist2)");
        // spot-check: pipeline 0 group first, and within p0 lod1 before lod3
        let first = &items[out[0] as usize];
        assert_eq!(first.pipeline, 0);
    }

    #[test]
    fn ascending_dist2_within_group() {
        // same pipeline+lod, scrambled dist2 ⇒ output ascending by dist2
        let items = [
            item(5, 2, 300, 0),
            item(5, 2, 100, 1),
            item(5, 2, 200, 2),
        ];
        let mut out = Vec::new();
        order(&items, &mut out);
        assert_eq!(out, vec![1, 2, 0], "front-to-back: 100,200,300");
    }

    #[test]
    fn stable_on_full_ties() {
        // identical keys ⇒ input order preserved (ascending index)
        let items = [item(3, 1, 7, 10), item(3, 1, 7, 11), item(3, 1, 7, 12)];
        let mut out = Vec::new();
        order(&items, &mut out);
        assert_eq!(out, vec![0, 1, 2]);
    }

    #[test]
    fn deterministic() {
        let items = [item(1, 0, 9, 0), item(1, 0, 3, 1), item(0, 2, 4, 2)];
        let (mut a, mut b) = (Vec::new(), Vec::new());
        order(&items, &mut a);
        order(&items, &mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn property_random_permutation_and_sorted() {
        let mut s = 0x350B_0A35u32; // documented seed (R20): "OPT sort", round 35
        for _ in 0..300 {
            let n = (lcg(&mut s) % 64) as usize;
            let mut items = Vec::with_capacity(n);
            for _ in 0..n {
                items.push(item(
                    (lcg(&mut s) % 4) as u16,   // few pipelines ⇒ real groups
                    (lcg(&mut s) % 3) as u8,    // few lods
                    lcg(&mut s) % 1000,         // dist2
                    lcg(&mut s),                // slot
                ));
            }
            let mut out = Vec::new();
            order(&items, &mut out);
            assert!(is_permutation(&out, n), "permutation (n={n})");
            assert!(keys_sorted(&items, &out), "sorted by key (n={n})");
        }
    }
}
