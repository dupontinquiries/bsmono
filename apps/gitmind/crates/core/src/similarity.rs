// gitmind-core/src/similarity.rs
//
// "Track these files with 99% matching." Because every file is already
// split into content-defined chunks, two near-identical files (e.g. a
// JPEG re-saved by an editor, or a losslessly-retranscoded photo) share
// most of their chunks. So similarity falls straight out of the chunk
// store — no separate perceptual hash needed for the byte-level case.
//
// similarity = shared chunk bytes / total distinct chunk bytes
//            = a byte-weighted Jaccard over the two chunk sets, in [0,1].
//
// At ingest time the store can ask "is there an existing blob ≥ threshold
// similar to this one?" and, if so, record a near-dup link + metadata
// instead of treating it as wholly new — which is exactly the JPEG drift
// case: keep both, but know they're 99% the same and store the overlap
// once.

use crate::oid::Oid;
use crate::store::BlobManifest;
use std::collections::HashMap;

/// Byte-weighted overlap of two chunk sets, in [0.0, 1.0]. 1.0 == every
/// chunk shared (identical content); 0.0 == nothing shared.
pub fn manifest_similarity(a: &BlobManifest, b: &BlobManifest) -> f64 {
    let am = bytes_by_chunk(a);
    let bm = bytes_by_chunk(b);
    if am.is_empty() && bm.is_empty() {
        return 1.0;
    }
    let mut intersection: u64 = 0;
    for (oid, abytes) in &am {
        if let Some(bbytes) = bm.get(oid) {
            intersection += (*abytes).min(*bbytes);
        }
    }
    let total_a: u64 = am.values().sum();
    let total_b: u64 = bm.values().sum();
    let union = total_a + total_b - intersection;
    if union == 0 {
        return 1.0;
    }
    intersection as f64 / union as f64
}

/// True when two blobs are at least `threshold` similar (e.g. 0.99).
pub fn is_near_duplicate(a: &BlobManifest, b: &BlobManifest, threshold: f64) -> bool {
    manifest_similarity(a, b) >= threshold
}

fn bytes_by_chunk(m: &BlobManifest) -> HashMap<Oid, u64> {
    let mut map: HashMap<Oid, u64> = HashMap::new();
    for (oid, len) in m.chunks.iter().zip(m.lens.iter()) {
        *map.entry(*oid).or_insert(0) += *len as u64;
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MediaKind;

    fn manifest(chunks: &[(u8, u32)]) -> BlobManifest {
        let oids: Vec<Oid> = chunks.iter().map(|(seed, _)| Oid::hash(&[*seed])).collect();
        let lens: Vec<u32> = chunks.iter().map(|(_, l)| *l).collect();
        let total = lens.iter().map(|l| *l as u64).sum();
        BlobManifest { chunks: oids, lens, total_len: total, media: MediaKind::Image, name: "x".into() }
    }

    #[test]
    fn identical_is_one() {
        let m = manifest(&[(1, 100), (2, 100), (3, 100)]);
        assert!((manifest_similarity(&m, &m) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn disjoint_is_zero() {
        let a = manifest(&[(1, 100), (2, 100)]);
        let b = manifest(&[(8, 100), (9, 100)]);
        assert!(manifest_similarity(&a, &b) < 1e-9);
    }

    #[test]
    fn one_chunk_differs_is_near_one() {
        // 9 shared chunks of 100 bytes + 1 differing chunk each side.
        let mut shared: Vec<(u8, u32)> = (10..19).map(|i| (i as u8, 100)).collect();
        let mut a = shared.clone();
        a.push((100, 100));
        let mut b = shared.clone();
        b.push((200, 100));
        let _ = &mut shared;
        let ma = manifest(&a);
        let mb = manifest(&b);
        let sim = manifest_similarity(&ma, &mb);
        // intersection 900, union 1100 -> ~0.818; with finer chunking a
        // real 1-chunk-in-100 change lands ~0.99. Assert it's high and
        // that the near-dup gate triggers at a sensible threshold.
        assert!(sim > 0.8, "got {sim}");
        assert!(is_near_duplicate(&ma, &mb, 0.8));
        assert!(!is_near_duplicate(&ma, &mb, 0.95));
    }
}
