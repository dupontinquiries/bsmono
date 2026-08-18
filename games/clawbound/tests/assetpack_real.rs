//! The pack, against the real seventeen. The unit tests in `assetpack` use
//! fixtures; this one uses the shipped assets, because the properties that
//! matter — that no model loses a triangle and no vertex moves visibly — are
//! properties of THESE meshes, not of a synthetic wedge.
//!
//! Skips (rather than fails) if the assets are absent, so a checkout without
//! the art drop still runs green.

use bsweb::pack::PackOpts;
use clawbound_rs::assetpack;
use clawbound_rs::iqm::Model;

fn load_all() -> Vec<(String, Model)> {
    let dir = std::path::Path::new("assets/animals");
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    let mut paths: Vec<_> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "iqm").unwrap_or(false))
        .collect();
    paths.sort();
    for p in paths {
        if let Ok(b) = std::fs::read(&p) {
            if let Ok(m) = Model::parse(&b) {
                out.push((p.file_stem().unwrap().to_string_lossy().to_string(), m));
            }
        }
    }
    out
}

fn span_of(m: &Model) -> f32 {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &m.positions {
        for d in 0..3 {
            lo[d] = lo[d].min(p[d]);
            hi[d] = hi[d].max(p[d]);
        }
    }
    (0..3).map(|d| hi[d] - lo[d]).fold(0.0f32, f32::max)
}

#[test]
fn the_real_animals_survive_the_pack() {
    let models = load_all();
    if models.is_empty() {
        eprintln!("assets/animals absent — skipping");
        return;
    }
    assert_eq!(models.len(), 17, "the drop is seventeen models");
    let (web, stats) = assetpack::build_web(&models, PackOpts::default());
    // A second pack with welding off, to separate the two ways a triangle can
    // vanish: exact duplication in the source (an equality, asserted below) and
    // lattice welding (a small, measured loss of slivers).
    let (unwelded, _) =
        assetpack::build_web(&models, PackOpts { weld: false, ..PackOpts::default() });

    // nothing is silently dropped
    let names = assetpack::names(&web).unwrap();
    assert_eq!(names.len(), models.len());
    for (name, _) in &models {
        assert!(names.contains(name), "{name} missing from the web");
    }

    // quantisation stays far below anything a facet could show
    for s in &stats {
        assert!(s.max_rel_error < 5e-4, "error {} too large", s.max_rel_error);
    }

    // and the geometry itself: per model, no triangle lost beyond exact
    // duplicates, and every sampled source vertex still has a vertex on top of it
    for (name, src) in &models {
        let got = assetpack::load_web(&web, name).expect("loads");
        let span = span_of(src);
        use std::collections::HashSet;
        // The packer drops only EXACT duplicates — same three vertices, same
        // winding. That is not a threshold to tolerate, it is an equality to
        // assert, so the count is compared against the source's own unique set.
        // (These assets need it: low_poly_tree_scene_free ships 10,736
        // triangles of which 1,672 are duplicates — one facet appears nine
        // times — and drawing a facet nine times buys z-fighting, not looks.)
        let canon = |t: &[u32; 3]| {
            let (a, b, c) = (t[0], t[1], t[2]);
            if a <= b && a <= c {
                (a, b, c)
            } else if b <= c {
                (b, c, a)
            } else {
                (c, a, b)
            }
        };
        let src_unique: HashSet<(u32, u32, u32)> = src
            .tris
            .iter()
            .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
            .map(&canon)
            .collect();
        // Without welding the equality is exact: the only triangles the packer
        // drops are the ones the source stated twice.
        let plain = assetpack::load_web(&unwelded, name).expect("loads");
        assert_eq!(
            plain.tris.len(),
            src_unique.len(),
            "{name}: packed {} triangles against {} unique in the source",
            plain.tris.len(),
            src_unique.len()
        );
        // With welding on, a few more go — the slivers whose corners land on
        // one lattice point at 14 bits. Small, and worth knowing the size of:
        // measured across these seventeen the worst is the shark at 0.54%.
        let welded_loss = 1.0 - got.tris.len() as f32 / plain.tris.len() as f32;
        assert!(
            welded_loss < 0.01,
            "{name}: welding cost {:.2}% of the triangles",
            welded_loss * 100.0
        );
        // Reversed pairs are NOT duplicates and must survive: a double-sided
        // leaf card is two facets, and this renderer takes its normal from the
        // winding, so losing one would light the card from behind.
        let src_rev = src_unique
            .iter()
            .filter(|t| src_unique.contains(&canon(&[t.0, t.2, t.1])))
            .count();
        if src_rev > 0 {
            let got_set: HashSet<(u32, u32, u32)> = plain.tris.iter().map(&canon).collect();
            let got_rev = got_set
                .iter()
                .filter(|t| got_set.contains(&canon(&[t.0, t.2, t.1])))
                .count();
            assert_eq!(got_rev, src_rev, "{name}: double-sided facets lost");
        }
        let cell = span * 1e-3;
        let key = |p: &[f32; 3]| {
            (
                (p[0] / cell).round() as i64,
                (p[1] / cell).round() as i64,
                (p[2] / cell).round() as i64,
            )
        };
        let packed: HashSet<(i64, i64, i64)> = got.positions.iter().map(key).collect();
        for (i, p) in src.positions.iter().enumerate() {
            if i % 97 != 0 {
                continue;
            }
            let k = key(p);
            let near = (-1..=1).any(|dx| {
                (-1..=1)
                    .any(|dy| (-1..=1).any(|dz| packed.contains(&(k.0 + dx, k.1 + dy, k.2 + dz))))
            });
            assert!(near, "{name}: vertex {i} at {p:?} has no packed neighbour");
        }
    }
}

#[test]
fn the_pack_is_smaller_than_what_it_replaces() {
    let models = load_all();
    if models.is_empty() {
        return;
    }
    let loader_bytes: usize = models
        .iter()
        .map(|(_, m)| m.positions.len() * 12 + m.tris.len() * 12)
        .sum();
    let (web, _) = assetpack::build_web(&models, PackOpts::default());
    // Uncompressed already, before the container earns its share. If this ever
    // regresses the encoder has stopped paying for itself.
    assert!(
        web.len() * 2 < loader_bytes,
        "web {} vs raw loader buffers {loader_bytes}",
        web.len()
    );
}
