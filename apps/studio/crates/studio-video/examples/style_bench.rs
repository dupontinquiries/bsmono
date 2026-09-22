//! How fast is applying a baked style LUT, per frame, at real sizes?
use studio_video::style::{apply_lut, stats, Transfer, LUT_N};
fn main() {
    let mk = |w: usize, h: usize, s: u8| (0..w * h * 3).map(|i| ((i * 7 + s as usize) % 256) as u8).collect::<Vec<u8>>();
    let t = Transfer::fit(&stats(&mk(320, 180, 1)), &stats(&mk(320, 180, 90)), 1.0);
    let t0 = std::time::Instant::now(); let lut = t.lut(LUT_N);
    println!("bake {LUT_N}^3 LUT: {:.1} ms", t0.elapsed().as_secs_f64() * 1e3);
    for (w, h) in [(640usize, 360usize), (1280, 720), (1920, 1080), (3840, 2160)] {
        let mut f = mk(w, h, 3);
        let reps = if w > 2000 { 3 } else { 10 };
        let t = std::time::Instant::now();
        for _ in 0..reps { apply_lut(&mut f, &lut, LUT_N); }
        let ms = t.elapsed().as_secs_f64() * 1e3 / reps as f64;
        println!("{w}x{h}: {ms:6.1} ms per frame on one core -> {:5.1} fps", 1000.0 / ms);
    }
}
