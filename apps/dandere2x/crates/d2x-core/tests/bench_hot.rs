//! Not a test: `cargo test --release -p d2x-core --test bench_hot -- --ignored --nocapture`
//! prints per-call timings of the CPU hot paths that surround the GPU.
use d2x_core::frame::{block_sse, Frame, Pad};
use d2x_core::net::{Network, TileIo, TileRect};
use std::time::Instant;

fn frame(w: u32, h: u32) -> Frame {
    let mut f = Frame::new(w, h);
    for (i, b) in f.data.iter_mut().enumerate() {
        *b = ((i * 31 + (i >> 7) * 17) % 256) as u8;
    }
    f
}

fn time<R>(label: &str, iters: u32, mut f: impl FnMut() -> R) {
    let t = Instant::now();
    for _ in 0..iters {
        std::hint::black_box(f());
    }
    let per = t.elapsed().as_secs_f64() * 1e3 / iters as f64;
    println!("{label:<44} {per:9.3} ms");
}

#[test]
#[ignore]
fn hot_paths() {
    let hd = frame(1920, 1080);
    let other = { let mut o = hd.clone(); o.add_uniform_noise(3, 1); o };
    time("padded_to replicate 1080p +16", 5, || hd.padded(16, Pad::Replicate));
    time("psnr 1080p", 5, || hd.psnr(&other));
    time("add_uniform_noise 1080p", 5, || { let mut f = hd.clone(); f.add_uniform_noise(8, 3); f });
    time("block_sse x2304 (1080p grid, bs 30)", 5, || {
        let mut s = 0u64;
        for by in 0..36 { for bx in 0..64 { s += block_sse(&hd, bx * 30, by * 30, &other, bx * 30, by * 30, 30); } }
        s
    });
    time("hash_cell x2304 (cell 32)", 5, || {
        let p = hd.padded(1, Pad::Black);
        let mut h = 0u64;
        for by in 0..36 { for bx in 0..64 { h ^= d2x_core::blockcache::hash_cell(&p, bx * 30, by * 30, 32); } }
        h
    });
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/realesr-animevideov3.safetensors");
    let srvgg = Network::load(&p).unwrap();
    let padded = srvgg.pad_frame(&frame(480, 480));
    let io: TileIo = srvgg.tile_io(TileRect { x: 0, y: 0, w: 400, h: 400 });
    time("extract_input 400-tile, u=1", 20, || srvgg.extract_input(&padded, io));
    let rgba = vec![0u8; ((io.region_w * 4) * (io.region_h * 4) * 4) as usize];
    let mut out = Frame::new(1920, 1920);
    time("write_output_rgba8 400-tile x4", 20, || srvgg.write_output_rgba8(&mut out, io, &rgba));
    if let Ok(w) = std::env::var("D2X_RRDB_X2") {
        let rrdb = Network::load(std::path::Path::new(&w)).unwrap();
        let padded = rrdb.pad_frame(&frame(480, 480));
        let io = rrdb.tile_io(TileRect { x: 0, y: 0, w: 400, h: 400 });
        time("extract_input 400-tile, u=2 (rrdb x2)", 20, || rrdb.extract_input(&padded, io));
    }
}
