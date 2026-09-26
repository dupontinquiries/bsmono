//! Cost of `av1_grain::prepare` (grain LUT + scaling curves), which the GPU
//! grain stage runs on the CPU once per frame.
fn main() {
    use vidkit_core::av1_grain::{prepare, GrainParams};
    let mut p = GrainParams {
        y_points: vec![[0, 20], [255, 20]],
        cb_points: vec![[0, 16], [255, 16]],
        cr_points: vec![[0, 16], [255, 16]],
        scaling_shift: 10,
        ar_coeff_lag: 3,
        ar_coeff_shift: 7,
        overlap: true,
        ..Default::default()
    };
    for (i, c) in p.ar_coeffs_y.iter_mut().enumerate() { *c = (i as i8 % 7) - 3; }
    let n = 2000;
    let t = std::time::Instant::now();
    let mut sink = 0i64;
    for s in 0..n {
        p.seed = s as u16;
        let g = prepare(&p, true, true);
        sink += g.lut[0][10][10] as i64;
    }
    println!("prepare: {:.1} us/frame ({sink})", t.elapsed().as_secs_f64() * 1e6 / n as f64);
}
