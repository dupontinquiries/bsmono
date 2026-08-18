//! Verify the shipped pack against the source IQMs, and time the load.
//! Run: cargo run --release --example pack_verify

use clawbound_rs::assetpack;
use clawbound_rs::iqm::Model;
use std::time::Instant;

fn main() {
    let zst = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/animals.web.zst"))
        .expect("assets/animals.web.zst — run the pack_animals example first");
    let t0 = Instant::now();
    let web = assetpack::decompress(&zst).expect("decompress");
    let t_dec = t0.elapsed();
    let names = assetpack::names(&web).unwrap();

    let t1 = Instant::now();
    let mut verts = 0usize;
    let mut tris = 0usize;
    for n in &names {
        let m = assetpack::load_web(&web, n).expect("part");
        verts += m.positions.len();
        tris += m.tris.len();
    }
    let t_all = t1.elapsed();

    eprintln!(
        "unzstd {:.1} MB -> {:.1} MB in {:.1} ms; decoded {} models, {verts} verts, {tris} tris in {:.1} ms",
        zst.len() as f64 / 1e6,
        web.len() as f64 / 1e6,
        t_dec.as_secs_f64() * 1e3,
        names.len(),
        t_all.as_secs_f64() * 1e3
    );

    // against the source, model by model: extents must survive the round trip
    let mut worst = 0.0f32;
    for n in &names {
        let iqm_path = format!("{}/assets/animals/{n}.iqm", env!("CARGO_MANIFEST_DIR"));
        let Ok(bytes) = std::fs::read(&iqm_path) else { continue };
        let src = Model::parse(&bytes).expect("parse");
        let got = assetpack::load_web(&web, n).unwrap();
        let ext = |m: &Model| {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &m.positions {
                for d in 0..3 {
                    lo[d] = lo[d].min(p[d]);
                    hi[d] = hi[d].max(p[d]);
                }
            }
            (lo, hi)
        };
        let (alo, ahi) = ext(&src);
        let (blo, bhi) = ext(&got);
        let span = (0..3).map(|d| ahi[d] - alo[d]).fold(0.0f32, f32::max);
        for d in 0..3 {
            worst = worst.max((alo[d] - blo[d]).abs() / span);
            worst = worst.max((ahi[d] - bhi[d]).abs() / span);
        }
    }
    eprintln!("worst bounding-box drift vs the IQM source: {:.5}% of span", worst * 100.0);

    // and the loader path the game actually calls: first call pays the
    // decompression, the rest are decode-only
    let t2 = Instant::now();
    let m = Model::load("white_tiger").expect("Model::load");
    let first = t2.elapsed();
    let t3 = Instant::now();
    for n in &names {
        let _ = Model::load(n).expect("Model::load");
    }
    let rest = t3.elapsed();
    eprintln!(
        "Model::load(\"white_tiger\") -> {} verts, {} tris; first call {:.1} ms (decompress), \
         then all {} models in {:.1} ms ({:.1} ms each)",
        m.positions.len(),
        m.tris.len(),
        first.as_secs_f64() * 1e3,
        names.len(),
        rest.as_secs_f64() * 1e3,
        rest.as_secs_f64() * 1e3 / names.len() as f64
    );
    // what it replaces, for the honest comparison
    let t4 = Instant::now();
    let mut n_iqm = 0;
    for n in &names {
        let p = format!("{}/assets/animals/{n}.iqm", env!("CARGO_MANIFEST_DIR"));
        if let Ok(b) = std::fs::read(&p) {
            let _ = Model::parse(&b).unwrap();
            n_iqm += 1;
        }
    }
    eprintln!(
        "for comparison: parsing {n_iqm} loose IQMs from disk takes {:.1} ms",
        t4.elapsed().as_secs_f64() * 1e3
    );
}
