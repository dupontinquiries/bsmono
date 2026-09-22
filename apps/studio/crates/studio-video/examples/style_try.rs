//! Transfer the look of one frame onto another and write before/after/reference.
//!
//!     cargo run -p studio-video --example style_try -- VIDEO TARGET_S REFERENCE_S OUT.ppm
use studio_core::Ticks;
use studio_video::style::{apply_lut, stats, Transfer, LUT_N};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (w, h) = (480u32, 270u32);
    let grab = |s: &str| studio_video::frame_rgb(std::path::Path::new(&a[0]), Ticks::from_seconds(s.parse().unwrap()), w, h).unwrap();
    let (target, reference) = (grab(&a[1]), grab(&a[2]));
    let (st, sr) = (stats(&target), stats(&reference));
    let t0 = std::time::Instant::now();
    let t = Transfer::fit(&st, &sr, 1.0);
    let lut = t.lut(LUT_N);
    let fit_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let mut out = target.clone();
    let t1 = std::time::Instant::now();
    apply_lut(&mut out, &lut, LUT_N);
    let apply_ms = t1.elapsed().as_secs_f64() * 1000.0;
    let so = stats(&out);
    let show = |n: &str, s: &studio_video::style::Stats| println!(
        "{n:10} median L {:5.1} | shadows a {:+5.1} b {:+5.1} | highlights a {:+5.1} b {:+5.1} | sat (mid) {:4.1}",
        s.q[4], s.band[0][0], s.band[0][1], s.band[2][0], s.band[2][1], s.band[1][2]);
    show("target", &st); show("result", &so); show("reference", &sr);
    println!("fit + bake {LUT_N}^3 LUT: {fit_ms:.0} ms (once) | apply to a {w}x{h} frame: {apply_ms:.1} ms (per frame)");
    // triptych: target | result | reference
    let mut ppm = format!("P6\n{} {}\n255\n", w * 3, h).into_bytes();
    for y in 0..h as usize { for img in [&target, &out, &reference] {
        ppm.extend_from_slice(&img[y * w as usize * 3..(y + 1) * w as usize * 3]);
    }}
    std::fs::write(&a[3], ppm).unwrap();
}
