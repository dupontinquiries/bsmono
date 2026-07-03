//! Cross-validate the codec against the Python PoC on real luma frames.
//! Reads raw 8-bit luma `.bin` files (`width*height` bytes each) and reports the
//! VQ PSNR + the shared-table case (table from frame 0 applied to frame 1).
//!
//! Usage:
//!   cargo run --release --example face_demo -- <W> <H> a.bin b.bin [more.bin ...]

use snapclone_vqcodec::*;
use std::fs;

fn load(path: &str, w: usize, h: usize) -> GrayImage {
    let data = fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    assert_eq!(data.len(), w * h, "{path}: expected {} bytes, got {}", w * h, data.len());
    GrayImage::new(w, h, data)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: face_demo <W> <H> a.bin b.bin [...]");
        std::process::exit(2);
    }
    let w: usize = args[1].parse().expect("W");
    let h: usize = args[2].parse().expect("H");
    let imgs: Vec<GrayImage> = args[3..].iter().map(|p| load(p, w, h)).collect();

    let (block, k) = (4usize, 256usize);
    let cb = train_codebook(&imgs, block, k, 25, 0);
    let raw = w * h;
    println!(
        "joint table: {} entries, {}-bit index, {} B table",
        cb.centroids.len(),
        cb.index_bits(),
        cb.to_bytes().len()
    );
    for (i, im) in imgs.iter().enumerate() {
        let enc = encode(im, &cb);
        let dec = decode(&enc, &cb);
        let bytes = enc.index_payload_bytes(cb.index_bits());
        println!(
            "  img{i}: PSNR {:.2} dB | {} B ({:.1}x vs {} B raw)",
            psnr(im, &dec),
            bytes,
            raw as f64 / bytes as f64,
            raw
        );
    }
    if imgs.len() >= 2 {
        let cba = train_codebook(&imgs[0..1], block, k, 25, 0);
        let dec = decode(&encode(&imgs[1], &cba), &cba);
        println!("  cross (table from img0 -> img1): PSNR {:.2} dB", psnr(&imgs[1], &dec));
    }
}
