//! Verifies the engine's own decode_opus_file on a real Ogg-Opus file (no device).
//! Run: cargo run --release --example opus_decode_check --features audio -- <path>
fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/mnt/user-data/uploads/koYopper.opus".into());
    match unendlich_engine::audio::decode_opus_file(std::path::Path::new(&path)) {
        Ok(p) => {
            let frames = p.samples.len() / p.channels.max(1) as usize;
            let dur = frames as f64 / p.rate as f64;
            let peak = p.samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
            println!("decode_opus_file OK: ch={} rate={} samples={} dur={:.1}s peak={}", p.channels, p.rate, p.samples.len(), dur, peak);
        }
        Err(e) => { eprintln!("decode failed: {e}"); std::process::exit(1); }
    }
}
