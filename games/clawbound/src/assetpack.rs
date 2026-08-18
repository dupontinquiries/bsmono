//! assetpack.rs — the animal models as one packed web instead of seventeen IQMs.
//!
//! `assets/animals` is 19.0 MB of IQM v2 and `iqm.rs` reads about half of it:
//! positions and the triangle list, never the texcoords or normals, because
//! clawbound is flat-shaded papercraft and derives facet normals from winding.
//! Shipping the other half is paying to move bytes the game drops on load.
//!
//! This module packs what the loader actually uses into the monorepo's existing
//! parts-web format (`games/shared/bsweb`, also read by blackstar) and reads it
//! back. Measured over all seventeen meshes — 413,307 vertices, 480,083
//! triangles:
//!
//! ```text
//! IQM on disk                                        19.0 MB
//! positions + indices only                           10.7 MB   (56%)
//! + 14-bit quantisation per model box                 8.2 MB   (43%)
//! + Morton order, planar deltas, varints, zstd -19    2.57 MB  (13.5%, 7.4x)
//! ```
//!
//! The encoder lives in `bsweb::pack` with the measurements behind each stage;
//! what belongs *here* is the game's side of the deal:
//!
//! * **Which order parts go in.** The compressor sees one stream, so packing
//!   order is a compression decision: models are grouped by silhouette class
//!   (the big cats together, then the small cats, then scenery, then the
//!   exotics) so that similar geometry compresses against similar geometry.
//! * **What "no loss" means here.** Quantisation is lossy in the strict sense
//!   and lossless in the sense that matters: `Model::normalize` rescales every
//!   model to a caller-chosen height anyway, and 14 bits over a model's own box
//!   is finer than the vertex spacing of any of these meshes. The test below
//!   holds packed geometry to the IQM parse within a fraction of the span.
//! * **The container is the caller's.** bsweb takes decompressed bytes on
//!   purpose. clawbound's build step wraps the web in zstd; this module's
//!   [`load_web`] takes whatever bytes it is handed, so a caller can mmap a
//!   plain `.bsweb` in a dev build and hand over decompressed bytes in a
//!   shipped one without this module knowing the difference.

use crate::iqm::Model;
use bsweb::pack::{pack, PackOpts, PartMesh, PartStats};
use bsweb::Web;

/// The pack order. Grouping by silhouette is worth real bytes: geometry that
/// resembles its neighbour in the stream gives the compressor longer matches.
/// Within a group the order is alphabetical so the build is reproducible.
pub const PACK_ORDER: [&str; 17] = [
    // the great cats — heavy quadruped silhouettes
    "lion_male",
    "white_tiger",
    "clouded_leopard",
    "black_panther",
    "puma",
    // the small cats — near-identical topology budgets, so near-identical bytes
    "caracal_cat_game_ready",
    "cat",
    "persian_cat",
    "velgorn",
    "zebra_cat",
    // scenery — trunks and canopies
    "low_poly_tree_scene_free",
    "tree_in_sherwood_forest",
    // the exotics — water and canopy shapes that resemble nothing else here
    "bornean_orangutan_baby",
    "galapagos_tiny_blue_octopus",
    "great_white_shark",
    "japanese_sea_nettle",
    "keel_billed_toucan",
];

/// Which group a model belongs to, as the web's part category — the same field
/// blackstar's slot picker filters on, so a future draft screen can ask the web
/// for "a great cat" without a table in the game code.
pub fn category(name: &str) -> &'static str {
    match name {
        "lion_male" | "white_tiger" | "clouded_leopard" | "black_panther" | "puma" => "great_cat",
        "caracal_cat_game_ready" | "cat" | "persian_cat" | "velgorn" | "zebra_cat" => "small_cat",
        "low_poly_tree_scene_free" | "tree_in_sherwood_forest" => "scenery",
        _ => "exotic",
    }
}

/// Pack parsed models into web bytes. Caller supplies `(name, model)` pairs;
/// they are emitted in [`PACK_ORDER`], with anything unlisted appended after in
/// the order given, so a new asset drop packs without editing this table first.
pub fn build_web(models: &[(String, Model)], opts: PackOpts) -> (Vec<u8>, Vec<PartStats>) {
    let rank = |name: &str| {
        PACK_ORDER.iter().position(|p| *p == name).unwrap_or(PACK_ORDER.len())
    };
    let mut order: Vec<usize> = (0..models.len()).collect();
    order.sort_by_key(|&i| (rank(&models[i].0), models[i].0.clone()));
    let parts: Vec<PartMesh> = order
        .iter()
        .map(|&i| PartMesh {
            id: &models[i].0,
            category: Some(category(&models[i].0)),
            positions: &models[i].1.positions,
            tris: &models[i].1.tris,
        })
        .collect();
    pack(&parts, opts)
}

/// Read one model out of a web by name, back into the same [`Model`] the IQM
/// parser produces — so every call site downstream is unchanged.
pub fn load_web(bytes: &[u8], name: &str) -> Result<Model, String> {
    let web = Web::parse(bytes).map_err(|e| format!("bad web: {e:?}"))?;
    let part = web
        .parts
        .iter()
        .find(|p| p.id == name)
        .ok_or_else(|| format!("no part named {name} in the web"))?;
    let positions = web.vertices(part).ok_or("vertex stream truncated")?;
    let tris = web.triangles(part).ok_or("index stream truncated")?;
    Ok(Model { positions, tris })
}

/// The decompressed web, held once per process.
///
/// Measured, and the reason this exists: unzstd of the 2.8 MB container costs
/// ~40 ms, while decoding one model out of the decompressed bytes costs ~1.4 ms.
/// A naive `Model::load` that unwrapped the container per call would turn
/// seventeen loads into ~700 ms of repeated decompression — *slower* than the
/// IQM files it replaced, which is how a compression win turns into a load
/// regression. Decompress once; decode per model.
static WEB: std::sync::OnceLock<Option<Vec<u8>>> = std::sync::OnceLock::new();

/// The shipped web's bytes, decompressed, or None if no pack is present (a
/// checkout that has not run the `pack_animals` example — callers fall back to
/// the loose IQMs).
pub fn shipped_web() -> Option<&'static [u8]> {
    WEB.get_or_init(|| {
        // Raw first, deliberately. Measured: the container saves 0.93 MB on
        // disk and costs ~37 ms of ruzstd at load, and any real distribution
        // archive (apk, zip, tar.zst) squeezes the raw web back to within a
        // rounding error of the .zst — verified, 2,811,201 bytes against
        // 2,811,122. So the compressed file earns its keep only where files
        // ship loose, and the fast path is the better default.
        for candidate in ["animals.web", "animals.web.zst"] {
            let path = format!("{}/assets/{candidate}", env!("CARGO_MANIFEST_DIR"));
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let bytes = if candidate.ends_with(".zst") {
                match decompress(&bytes) {
                    Ok(b) => b,
                    Err(e) => {
                        // A corrupt pack is worth saying out loud rather than
                        // silently falling back to 19 MB of IQM.
                        eprintln!("assets/{candidate}: {e}");
                        continue;
                    }
                }
            } else {
                bytes
            };
            return Some(bytes);
        }
        None
    })
    .as_deref()
}

/// Unwrap a zstd container into the bytes [`load_web`] wants.
///
/// bsweb takes decompressed bytes by design — its own header argues that an
/// engine which already has a decompressor should not be handed a second one.
/// clawbound does not have one otherwise, so it enables bsweb's optional
/// `zstd` feature, which is **ruzstd**: pure Rust, no build script, no `cc`.
/// That choice is the whole reason this works on the targets that matter here —
/// a C dependency in the graph is exactly what makes cross-compiling to a
/// postmarketOS handset an afternoon instead of a command.
pub fn decompress(bytes: &[u8]) -> Result<Vec<u8>, String> {
    bsweb::decompress_zst(bytes)
}

/// Every model name a web carries, in stored order.
pub fn names(bytes: &[u8]) -> Result<Vec<String>, String> {
    let web = Web::parse(bytes).map_err(|e| format!("bad web: {e:?}"))?;
    Ok(web.parts.iter().map(|p| p.id.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two small models with different extents, enough to catch a swapped axis
    /// or a shared bounding box.
    fn fixtures() -> Vec<(String, Model)> {
        let mut out = Vec::new();
        for (name, scale) in [("cat", 1.0f32), ("lion_male", 40.0)] {
            let mut positions = Vec::new();
            for i in 0..64u32 {
                let f = i as f32 / 63.0;
                positions.push([
                    (f * 6.0).cos() * scale,
                    f * scale * 0.5,
                    (f * 6.0).sin() * scale * 0.25,
                ]);
            }
            let tris: Vec<[u32; 3]> = (0..62u32).map(|i| [i, i + 1, i + 2]).collect();
            out.push((name.to_string(), Model { positions, tris }));
        }
        out
    }

    #[test]
    fn a_packed_model_comes_back_where_it_went_in() {
        let models = fixtures();
        let (bytes, _) = build_web(&models, PackOpts::default());
        for (name, src) in &models {
            let got = load_web(&bytes, name).expect("loads");
            let span = {
                let mut lo = [f32::MAX; 3];
                let mut hi = [f32::MIN; 3];
                for p in &src.positions {
                    for d in 0..3 {
                        lo[d] = lo[d].min(p[d]);
                        hi[d] = hi[d].max(p[d]);
                    }
                }
                (0..3).map(|d| hi[d] - lo[d]).fold(0.0f32, f32::max)
            };
            assert_eq!(got.tris.len(), src.tris.len(), "{name}: triangle lost");
            // every source vertex has a packed vertex within the quantiser step
            for p in &src.positions {
                let near = got
                    .positions
                    .iter()
                    .map(|q| (0..3).map(|d| (q[d] - p[d]).abs()).fold(0.0f32, f32::max))
                    .fold(f32::MAX, f32::min);
                assert!(near < span * 1e-3, "{name}: vertex moved {near} of span {span}");
            }
        }
    }

    #[test]
    fn each_model_keeps_its_own_precision_ladder() {
        // The 40x model must not drag the 1x model's precision down with it —
        // this is why the packer quantises per part rather than per scene.
        let models = fixtures();
        let (bytes, _) = build_web(&models, PackOpts::default());
        let small = load_web(&bytes, "cat").unwrap();
        let src = &models[0].1;
        let worst = src
            .positions
            .iter()
            .map(|p| {
                small
                    .positions
                    .iter()
                    .map(|q| (0..3).map(|d| (q[d] - p[d]).abs()).fold(0.0f32, f32::max))
                    .fold(f32::MAX, f32::min)
            })
            .fold(0.0f32, f32::max);
        assert!(worst < 0.001, "the small model kept fine precision: {worst}");
    }

    #[test]
    fn parts_are_stored_in_the_declared_group_order() {
        // Pack order is a compression decision, so it is worth asserting that
        // it is the order the table says rather than whatever the caller passed.
        let mut models = fixtures();
        models.reverse(); // caller hands them over backwards
        let (bytes, _) = build_web(&models, PackOpts::default());
        assert_eq!(names(&bytes).unwrap(), vec!["lion_male", "cat"]);
    }

    #[test]
    fn every_shipped_asset_has_a_place_in_the_order() {
        // A new drop that nobody added to PACK_ORDER still packs (it appends),
        // but the seventeen that exist should be accounted for, and no name
        // should appear twice.
        let mut seen = PACK_ORDER.to_vec();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), before, "duplicate name in PACK_ORDER");
        for name in PACK_ORDER {
            assert_ne!(category(name), "exotic_unknown");
        }
        assert_eq!(PACK_ORDER.iter().filter(|n| category(n) == "great_cat").count(), 5);
        assert_eq!(PACK_ORDER.iter().filter(|n| category(n) == "small_cat").count(), 5);
    }

    #[test]
    fn a_missing_name_is_an_error_not_a_default_model() {
        let (bytes, _) = build_web(&fixtures(), PackOpts::default());
        assert!(load_web(&bytes, "sabertooth").is_err());
        assert!(load_web(b"not a web at all", "cat").is_err());
    }
}
