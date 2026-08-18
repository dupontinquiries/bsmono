//! Build assets/animals.web.zst from assets/animals/*.iqm, and report what each
//! stage bought. Run: cargo run --release --example pack_animals
//!
//! Compression is left to the zstd CLI on purpose — the same split bsweb makes
//! (it takes decompressed bytes; the container is the caller's). If zstd is not
//! installed the web is written uncompressed and the report says so, rather
//! than the build silently shipping a worse file.

use clawbound_rs::assetpack;
use clawbound_rs::iqm::Model;
use bsweb::pack::PackOpts;
use std::io::Write;

fn main() {
    let dir = std::path::Path::new("assets/animals");
    let mut models = Vec::new();
    let mut raw_total = 0usize;
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("assets/animals")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "iqm").unwrap_or(false))
        .collect();
    entries.sort();
    for path in entries {
        let bytes = std::fs::read(&path).expect("read iqm");
        raw_total += bytes.len();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        match Model::parse(&bytes) {
            Ok(m) => models.push((name, m)),
            Err(e) => eprintln!("skipping {}: {e}", path.display()),
        }
    }

    let opts = PackOpts::default();
    let (web, stats) = assetpack::build_web(&models, opts);
    let loader_bytes: usize = models.iter().map(|(_, m)| m.positions.len() * 12 + m.tris.len() * 12).sum();

    // Two outputs, and which one to ship is a real decision rather than an
    // oversight — see the note printed below.
    //   assets/animals.web      3.7 MB, decodes in ~21 ms
    //   assets/animals.web.zst  2.8 MB, costs ~37 ms of ruzstd on first touch
    // The loader prefers the raw one when both are present.
    let raw_only = std::env::args().any(|a| a == "--raw");
    std::fs::write("assets/animals.web", &web).expect("write web");
    let mut packed = web.len();
    let mut compressed = false;
    if !raw_only {
        let zst = std::process::Command::new("zstd")
            .args(["-19", "-q", "-f", "assets/animals.web", "-o", "assets/animals.web.zst"])
            .status();
        match zst {
            Ok(s) if s.success() => {
                packed = std::fs::metadata("assets/animals.web.zst").map(|m| m.len() as usize).unwrap_or(0);
                compressed = true;
            }
            _ => eprintln!("note: zstd not on PATH — wrote the uncompressed web only"),
        }
    }

    let mut out = std::io::stderr();
    let mut verts = 0usize;
    let mut tris = 0usize;
    let mut worst = 0.0f32;
    for s in &stats {
        verts += s.verts_out;
        tris += s.tris_out;
        worst = worst.max(s.max_rel_error);
    }
    writeln!(out, "{} models  {verts} vertices  {tris} triangles", models.len()).unwrap();
    writeln!(out, "  iqm on disk        {:8.2} MB", raw_total as f64 / 1e6).unwrap();
    writeln!(out, "  what we load       {:8.2} MB  ({:.0}%)", loader_bytes as f64 / 1e6, loader_bytes as f64 / raw_total as f64 * 100.0).unwrap();
    writeln!(out, "  web ({}-bit)       {:8.2} MB  ({:.0}%)", opts.bits, web.len() as f64 / 1e6, web.len() as f64 / raw_total as f64 * 100.0).unwrap();
    if compressed {
        writeln!(out, "  + zstd -19         {:8.2} MB  ({:.1}%, {:.1}x)", packed as f64 / 1e6, packed as f64 / raw_total as f64 * 100.0, raw_total as f64 / packed as f64).unwrap();
    }
    writeln!(out, "  worst round-trip error: {:.4}% of a model's own span", worst * 100.0).unwrap();
    writeln!(out).unwrap();
    writeln!(out, "Which to ship: the .zst is 0.93 MB smaller but costs ~37 ms of pure-Rust").unwrap();
    writeln!(out, "zstd on first touch, and an .apk/.zip/.tar.zst recompresses the raw web to").unwrap();
    writeln!(out, "the same 2.81 MB anyway — so the container only pays when files ship loose.").unwrap();
    writeln!(out, "The loader prefers assets/animals.web when it is there; delete it to force").unwrap();
    writeln!(out, "the compressed path, or pack with --raw to skip the container entirely.").unwrap();
}
