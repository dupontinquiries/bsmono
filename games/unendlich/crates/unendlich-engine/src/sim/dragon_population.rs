//! `sim::dragon_population` — build and breed *populations* of dragons so the
//! world isn't one canonical body per element. Each founder rolls the full
//! body-design space (`dragon_archetype::Genome::from_seed`), and later
//! generations are bred from the survivors (`dragon_breeding::cross`), so traits
//! recombine and drift over time. Pure, deterministic.

use crate::sim::dragon_archetype::Genome;
use crate::sim::dragon_breeding::{breed_clutch, cross};
use crate::sim::mate_select::compat;

fn mix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `n` founder genomes, each an independent full-range roll — distinct
/// individuals, not copies of an archetype.
pub fn founders(base_seed: u64, n: usize) -> Vec<Genome> {
    let mut s = base_seed ^ 0xF0_0D_FACE_u64;
    (0..n).map(|_| Genome::from_seed(mix(&mut s))).collect()
}

/// Breed the next generation: pair random parents from `parents` and cross them.
/// With fewer than two parents it falls back to fresh founders. Offspring vary
/// around their parents, so the population diversifies generation over generation.
pub fn next_generation(parents: &[Genome], n: usize, seed: u64) -> Vec<Genome> {
    if parents.len() < 2 {
        return founders(seed, n);
    }
    let mut s = seed ^ 0xB4ED_u64;
    (0..n)
        .map(|_| {
            let a = &parents[(mix(&mut s) as usize) % parents.len()];
            let b = &parents[(mix(&mut s) as usize) % parents.len()];
            cross(a, b, mix(&mut s))
        })
        .collect()
}

/// Breed a generation as discrete mated *pairs*, each laying a clutch (vs the
/// flat random-pairing of [`next_generation`]). Parents are paired up in order
/// (`2p`, `2p+1`); each pair lays a [`breed_clutch`], so the generation size is
/// emergent — the sum of clutch sizes — and leaner dragons (bigger clutches)
/// contribute more offspring. Falls back to founders with fewer than two parents.
/// Deterministic in `(parents, seed)`. Wires the per-pair clutch model into the
/// population layer.
pub fn next_generation_clutched(parents: &[Genome], seed: u64) -> Vec<Genome> {
    if parents.len() < 2 {
        return founders(seed, parents.len().max(1));
    }
    let mut s = seed ^ 0xC1A7_C0DE_u64;
    let mut kids = Vec::new();
    for p in 0..(parents.len() / 2) {
        let mother = &parents[2 * p];
        let father = &parents[2 * p + 1];
        kids.extend(breed_clutch(mother, father, mix(&mut s)));
    }
    kids
}

/// Pair parents greedily by mate compatibility: each still-unpaired dragon is
/// matched with its most-compatible remaining partner (`mate_select::compat` over
/// morphological traits; ties broken by lower index). Returns `(mother, father)`
/// index pairs; an odd one out is left unpaired. Pure and deterministic — no seed.
pub fn pair_by_compat(parents: &[Genome]) -> Vec<(usize, usize)> {
    let mut paired = vec![false; parents.len()];
    let mut pairs = Vec::new();
    for i in 0..parents.len() {
        if paired[i] {
            continue;
        }
        let ti = parents[i].traits();
        let mut best: Option<(usize, f32)> = None;
        for j in (i + 1)..parents.len() {
            if paired[j] {
                continue;
            }
            let score = compat(&ti, &parents[j].traits());
            // strict `>` keeps the lowest-index partner on ties → deterministic.
            if best.map_or(true, |(_, bs)| score > bs) {
                best = Some((j, score));
            }
        }
        if let Some((j, _)) = best {
            paired[i] = true;
            paired[j] = true;
            pairs.push((i, j));
        }
    }
    pairs
}

/// Like [`next_generation_clutched`] but pairs mates by compatibility
/// ([`pair_by_compat`]) rather than in index order, so similar dragons are
/// likelier to breed. Each pair lays a [`breed_clutch`]; generation size is
/// emergent. Falls back to founders with fewer than two parents. Deterministic
/// in `(parents, seed)`.
pub fn next_generation_clutched_compat(parents: &[Genome], seed: u64) -> Vec<Genome> {
    if parents.len() < 2 {
        return founders(seed, parents.len().max(1));
    }
    let mut s = seed ^ 0xC0FF_EE11_u64;
    let mut kids = Vec::new();
    for (mi, fi) in pair_by_compat(parents) {
        kids.extend(breed_clutch(&parents[mi], &parents[fi], mix(&mut s)));
    }
    kids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::dragon_breeding::MAX_CLUTCH;

    fn body(g: &Genome) -> [f32; 6] {
        [g.bulk, g.neck_len, g.tail_len, g.leg_len, g.wing_span, g.horns]
    }

    #[test]
    fn founders_are_varied() {
        let pop = founders(42, 24);
        assert_eq!(pop.len(), 24);
        // no two founders share the same body vector
        for i in 0..pop.len() {
            for j in (i + 1)..pop.len() {
                assert_ne!(body(&pop[i]), body(&pop[j]), "founders must differ");
            }
        }
        // the body genes actually span a wide range (not clustered)
        let mut lo = 1.0f32;
        let mut hi = 0.0f32;
        for g in &pop {
            lo = lo.min(g.bulk);
            hi = hi.max(g.bulk);
        }
        assert!(hi - lo > 0.5, "bulk spans a broad range across the population");
    }

    #[test]
    fn next_generation_breeds_varied_offspring_in_range() {
        let parents = founders(7, 8);
        let kids = next_generation(&parents, 12, 99);
        assert_eq!(kids.len(), 12);
        for k in &kids {
            for v in [k.bulk, k.neck_len, k.tail_len, k.leg_len, k.wing_span, k.horns] {
                assert!((0.0..=1.0).contains(&v) && v.is_finite());
            }
            for e in k.element {
                assert!((0.0..=1.0).contains(&e));
            }
        }
        // offspring aren't all identical
        assert!(kids.windows(2).any(|w| body(&w[0]) != body(&w[1])));
    }

    #[test]
    fn fewer_than_two_parents_falls_back_to_founders() {
        let one = founders(1, 1);
        let kids = next_generation(&one, 5, 3);
        assert_eq!(kids.len(), 5);
    }

    #[test]
    fn deterministic() {
        assert_eq!(
            founders(5, 4).iter().map(|g| g.bulk).collect::<Vec<_>>(),
            founders(5, 4).iter().map(|g| g.bulk).collect::<Vec<_>>(),
        );
        let p = founders(2, 6);
        let a = next_generation(&p, 6, 11);
        let b = next_generation(&p, 6, 11);
        assert_eq!(a.iter().map(|g| g.bulk).collect::<Vec<_>>(), b.iter().map(|g| g.bulk).collect::<Vec<_>>());
    }

    #[test]
    fn clutched_generation_size_is_emergent_and_in_range() {
        let parents = founders(7, 6); // 3 mated pairs
        let pairs = parents.len() / 2;
        let kids = next_generation_clutched(&parents, 99);
        assert!(kids.len() >= pairs, "each pair lays at least one egg");
        assert!(kids.len() <= pairs * MAX_CLUTCH as usize, "bounded by max clutch per pair");
        for k in &kids {
            for v in body(k) {
                assert!((0.0..=1.0).contains(&v) && v.is_finite());
            }
            for e in k.element {
                assert!((0.0..=1.0).contains(&e));
            }
        }
        assert!(kids.windows(2).any(|w| body(&w[0]) != body(&w[1])), "offspring vary");
    }

    #[test]
    fn clutched_generation_is_deterministic() {
        let p = founders(2, 6);
        let a = next_generation_clutched(&p, 11);
        let b = next_generation_clutched(&p, 11);
        assert_eq!(a.len(), b.len());
        assert_eq!(
            a.iter().map(body).collect::<Vec<_>>(),
            b.iter().map(body).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn clutched_fewer_than_two_parents_falls_back_to_founders() {
        assert_eq!(next_generation_clutched(&[], 3).len(), 1);
        let one = founders(1, 1);
        assert_eq!(next_generation_clutched(&one, 3).len(), 1);
    }

    #[test]
    fn compat_pairing_matches_like_with_like_not_index_order() {
        // interleaved: indices 0&2 are identical, 1&3 are identical, a != b.
        let a1 = Genome::from_seed(1);
        let a2 = Genome::from_seed(1);
        let b1 = Genome::from_seed(987_654);
        let b2 = Genome::from_seed(987_654);
        let parents = vec![a1, b1, a2, b2];
        // in-order would mate (0,1),(2,3) — dissimilar; compat mates like-with-like.
        assert_eq!(
            pair_by_compat(&parents),
            vec![(0, 2), (1, 3)],
            "compat pairs the identical dragons, overriding index order"
        );
    }

    #[test]
    fn compat_pairing_drops_the_odd_one_out() {
        let p = founders(7, 3);
        assert_eq!(pair_by_compat(&p).len(), 1, "3 parents → 1 pair, one unpaired");
    }

    #[test]
    fn compat_generation_in_range_and_deterministic() {
        let parents = founders(7, 6);
        let a = next_generation_clutched_compat(&parents, 42);
        let b = next_generation_clutched_compat(&parents, 42);
        assert!(!a.is_empty());
        assert_eq!(a.len(), b.len());
        for k in &a {
            for v in body(k) {
                assert!((0.0..=1.0).contains(&v) && v.is_finite());
            }
        }
        assert_eq!(
            a.iter().map(body).collect::<Vec<_>>(),
            b.iter().map(body).collect::<Vec<_>>(),
        );
    }
}
