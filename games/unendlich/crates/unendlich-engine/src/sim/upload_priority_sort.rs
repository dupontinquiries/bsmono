//! `sim::upload_priority_sort` — order pending chunk uploads so a tight
//! per-frame budget is spent on what the player actually sees.
//!
//! `tick_apply` (main-thread chunk-upload application) can spike (M2 data: 7.8ms
//! when it applies a full budget of 12 chunks in one frame), so when the budget
//! is tight the chunks it *does* apply should be the visible, nearby ones — pop-in
//! then lands where the player is looking, not behind the camera.
//! `UNENDLICH_UPLOAD_PRIORITY=1` gates this; [`order`] is the pure ordering it
//! was missing. It composes with R33 `visible_delta` (what entered view) and
//! R34 `draw_order_sort`.
//!
//! ## Priority (lower rank applied first)
//! 1. **in-frustum** uploads (what the player sees) — always first.
//! 2. **aged off-screen** uploads (`age_frames > age_boost_frames`) — promoted
//!    ahead of fresh off-screen work so nothing starves while the camera lingers.
//! 3. **fresh off-screen** uploads.
//!
//! Within a class, **nearer first** (smaller `dist2`). Full ties keep input
//! order (stable). `lod` is carried on the item but is not a sort key.
//!
//! [`order`] writes a *permutation* of `0..pending.len()` into `out` (cleared
//! first) and allocates nothing beyond `out` itself: it sorts the index list in
//! place with an unstable sort whose comparator falls back to the original index,
//! which both makes the order total/deterministic and reproduces stable
//! (input) order on full ties — without the scratch buffer a stable sort needs.
//!
//! Pure, deterministic. No `crate::render`, no `ash`/`vk`, no IO, no clock, no
//! RNG, no `.unwrap()`/`.expect()` in non-test code. Compiles clean under
//! `cargo check --release`; no helper is gated behind `cfg(test)`/
//! `cfg(debug_assertions)`.

/// One chunk waiting to be uploaded/applied this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingUpload {
    pub slot: u32,
    pub in_frustum: bool,
    pub dist2: u32,
    pub lod: u8,
    pub age_frames: u32,
}

/// Priority class: 0 = in-frustum, 1 = aged off-screen (anti-starvation),
/// 2 = fresh off-screen. Defined unconditionally (release-safe).
#[inline]
pub fn priority_class(p: &PendingUpload, age_boost_frames: u32) -> u8 {
    if p.in_frustum {
        0
    } else if p.age_frames > age_boost_frames {
        1
    } else {
        2
    }
}

/// Order pending uploads into `out` (a permutation of `0..pending.len()`).
/// See module docs for the priority. `out` is cleared first; no other allocation.
pub fn order(pending: &[PendingUpload], age_boost_frames: u32, out: &mut Vec<u32>) {
    out.clear();
    let mut i = 0u32;
    while (i as usize) < pending.len() {
        out.push(i);
        i += 1;
    }
    // Total order: class, then nearer (dist2), then original index (stable on
    // full ties + fully deterministic). Unstable sort ⇒ no scratch allocation.
    out.sort_unstable_by(|&a, &b| {
        let pa = &pending[a as usize];
        let pb = &pending[b as usize];
        priority_class(pa, age_boost_frames)
            .cmp(&priority_class(pb, age_boost_frames))
            .then(pa.dist2.cmp(&pb.dist2))
            .then(a.cmp(&b))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    fn up(slot: u32, in_frustum: bool, dist2: u32, age_frames: u32) -> PendingUpload {
        PendingUpload { slot, in_frustum, dist2, lod: 0, age_frames }
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
        seen.iter().all(|&s| s)
    }

    #[test]
    fn output_is_a_permutation() {
        let mut seed = 0x0BCD_0000u32; // documented seed
        for _ in 0..3000 {
            let n = (lcg(&mut seed) % 40) as usize;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(up(
                    lcg(&mut seed),
                    lcg(&mut seed) & 1 == 0,
                    lcg(&mut seed) % 10_000,
                    lcg(&mut seed) % 600,
                ));
            }
            let mut out = vec![999u32; 7]; // prefilled to prove it's cleared
            order(&v, 300, &mut out);
            assert!(is_permutation(&out, n), "not a permutation for n={}", n);
        }
    }

    #[test]
    fn in_frustum_before_off_screen() {
        let v = vec![
            up(0, false, 1, 0),
            up(1, true, 9000, 0),
            up(2, false, 0, 0),
            up(3, true, 5000, 0),
        ];
        let mut out = Vec::new();
        order(&v, 300, &mut out);
        // every in-frustum index precedes every off-screen index
        let mut seen_offscreen = false;
        for &i in &out {
            if v[i as usize].in_frustum {
                assert!(!seen_offscreen, "in-frustum {} came after an off-screen item", i);
            } else {
                seen_offscreen = true;
            }
        }
        // and the two in-frustum come first
        assert!(v[out[0] as usize].in_frustum && v[out[1] as usize].in_frustum);
    }

    #[test]
    fn nearer_before_farther_within_class() {
        // in-frustum
        let v = vec![up(0, true, 500, 0), up(1, true, 10, 0), up(2, true, 100, 0)];
        let mut out = Vec::new();
        order(&v, 300, &mut out);
        assert_eq!(out, vec![1, 2, 0]); // dist2 10 < 100 < 500
        // fresh off-screen
        let v2 = vec![up(0, false, 800, 0), up(1, false, 8, 0)];
        order(&v2, 300, &mut out);
        assert_eq!(out, vec![1, 0]);
    }

    #[test]
    fn aged_offscreen_promoted_ahead_of_fresh_offscreen() {
        // aged item is FAR; fresh ones are NEAR — aged must still come first
        // (after any in-frustum), because it's been starving.
        let v = vec![
            up(0, false, 9000, 500), // aged (age 500 > boost 300), far
            up(1, false, 1, 0),      // fresh, very near
            up(2, false, 2, 10),     // fresh, near
            up(3, true, 9999, 0),    // in-frustum, far
        ];
        let mut out = Vec::new();
        order(&v, 300, &mut out);
        // in-frustum first, then aged, then fresh (near→far)
        assert_eq!(out, vec![3, 0, 1, 2]);
    }

    #[test]
    fn stable_on_full_ties() {
        // identical sort keys (off-screen, same dist2, same age-class), differing
        // only in slot/lod — must keep input order.
        let v = vec![
            PendingUpload { slot: 10, in_frustum: false, dist2: 5, lod: 0, age_frames: 0 },
            PendingUpload { slot: 11, in_frustum: false, dist2: 5, lod: 3, age_frames: 0 },
            PendingUpload { slot: 12, in_frustum: false, dist2: 5, lod: 7, age_frames: 0 },
        ];
        let mut out = Vec::new();
        order(&v, 300, &mut out);
        assert_eq!(out, vec![0, 1, 2], "full ties must preserve input order");
    }

    #[test]
    fn empty_yields_empty() {
        let mut out = vec![1, 2, 3];
        order(&[], 300, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x0BCD_0001u32;
        for _ in 0..3000 {
            let n = (lcg(&mut seed) % 30) as usize;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(up(
                    lcg(&mut seed),
                    lcg(&mut seed) & 1 == 0,
                    lcg(&mut seed) % 5000,
                    lcg(&mut seed) % 600,
                ));
            }
            let mut a = Vec::new();
            let mut b = Vec::new();
            order(&v, 300, &mut a);
            order(&v, 300, &mut b);
            assert_eq!(a, b);
        }
    }

    #[test]
    fn budget_prefix_is_visible_and_near() {
        // The bonus: take the first N (the affordable budget). With a mix, the
        // prefix should be in-frustum, nearest-first.
        let mut v = Vec::new();
        for d in [50u32, 5, 900, 20].iter() {
            v.push(up(0, true, *d, 0));
        }
        for d in [1u32, 2].iter() {
            v.push(up(0, false, *d, 0)); // off-screen, even if near
        }
        let mut out = Vec::new();
        order(&v, 300, &mut out);
        // first 4 are the in-frustum ones, nearest-first
        let first4: Vec<u32> = out.iter().take(4).cloned().collect();
        assert_eq!(first4, vec![1, 3, 0, 2]); // dist 5,20,50,900
        for &i in &first4 {
            assert!(v[i as usize].in_frustum);
        }
    }
}
