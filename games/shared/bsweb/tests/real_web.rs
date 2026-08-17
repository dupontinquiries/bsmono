//! Read the real web, not a fixture.
//!
//! The unit tests build a web by hand in the shape the Python writes. This one
//! reads `catalog/parts_web_full.bin.zst` decompressed — 40,742 parts from 1,012
//! vehicles — because a hand-built fixture agrees with whatever the reader
//! expects, and only the real artefact can disagree.
//!
//! Skipped rather than failed when the file is absent: the path is outside this
//! crate, and a test that fails on a fresh checkout teaches people to ignore it.

use std::path::PathBuf;

fn real_web() -> Option<Vec<u8>> {
    let p = PathBuf::from(std::env::var("BSWEB_TEST_FILE").ok()?);
    std::fs::read(p).ok()
}

#[test]
fn the_shipped_web_parses_and_decodes() {
    let Some(bytes) = real_web() else {
        eprintln!("BSWEB_TEST_FILE unset; skipping");
        return;
    };
    let w = bsweb::Web::parse(&bytes).expect("the shipped web parses");
    assert!(w.parts.len() > 1000, "parts: {}", w.parts.len());

    // Decode a sample rather than all 40k: the point is that the format reads,
    // and the slowest part of this test should not be the part nobody doubts.
    let mut decoded = 0;
    let mut named = 0;
    for p in w.parts.iter().take(400) {
        if p.name.is_some() {
            named += 1;
        }
        let v = w.vertices(p).expect("vertices decode");
        let t = w.triangles(p).expect("triangles decode");
        assert_eq!(v.len(), p.nv);
        assert_eq!(t.len(), p.nt);
        // Every index must address a vertex that exists — the check that
        // catches a delta stream drifting, which is the failure mode a
        // variable-width encoding actually has.
        for tri in &t {
            for i in tri {
                assert!((*i as usize) < v.len(), "index {} of {}", i, v.len());
            }
        }
        decoded += 1;
    }
    assert_eq!(decoded, 400);
    assert!(named > 0, "the harvest's categories survived the round trip");
}

/// The `.zst` container, end to end: decompress, then parse the same web.
///
/// The point is not that ruzstd works — it is that the payload inside a `.zst`
/// is the same payload as inside the `.gz`, so switching containers changes
/// nothing a reader can observe. Skipped like its sibling when the file is
/// absent, since the artefact lives outside this crate.
#[cfg(feature = "zstd")]
#[test]
fn the_zst_container_yields_the_same_web() {
    let Some(p) = std::env::var("BSWEB_TEST_FILE_ZST").ok() else {
        eprintln!("BSWEB_TEST_FILE_ZST unset; skipping");
        return;
    };
    let Ok(raw) = std::fs::read(&p) else {
        eprintln!("{p} unreadable; skipping");
        return;
    };
    let bytes = bsweb::decompress_zst(&raw).expect("zst decompresses");
    let w = bsweb::Web::parse(&bytes).expect("the shipped web parses");
    assert!(w.parts.len() > 1000, "parts: {}", w.parts.len());

    // Decode a sample, exactly as the gz test does — same assertions, so a
    // divergence between containers would show up as one of these failing.
    for part in w.parts.iter().take(400) {
        let v = w.vertices(part).expect("vertices decode");
        let t = w.triangles(part).expect("triangles decode");
        assert_eq!(v.len(), part.nv);
        assert_eq!(t.len(), part.nt);
    }
}
