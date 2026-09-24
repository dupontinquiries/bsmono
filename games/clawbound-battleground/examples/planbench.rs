use clawbound::going::Carriage;
use glam::Vec3;
fn main() {
    let w = clawbound::scene::world_blockers();
    // The botwalk corner, plus 200 random jungle points with random wants.
    let mut pts = vec![(Vec3::new(-30.7, 0.0, 27.6), Vec3::new(0.3, 0.0, 0.95))];
    let mut s = 12345u64;
    let mut r = || { s ^= s << 13; s ^= s >> 7; s ^= s << 17; (s % 10000) as f32 / 10000.0 };
    for _ in 0..200 {
        let a = r() * 6.283; let rad = 20.0 + r() * 130.0; let b = r() * 6.283;
        pts.push((Vec3::new(a.cos() * rad, 0.0, a.sin() * rad), Vec3::new(b.cos(), 0.0, b.sin())));
    }
    let mut times: Vec<f64> = Vec::new();
    for (p, d) in &pts {
        let t = std::time::Instant::now();
        let path = clawbound::steer::plan(&w, *p, *d, 0.55, Carriage::default());
        times.push(t.elapsed().as_secs_f64() * 1e6);
        std::hint::black_box(path);
    }
    let corner = times[0];
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("plan(): corner {:.0} us; over 200 jungle points median {:.0} us, p95 {:.0} us, max {:.0} us", corner, times[100], times[190], times[199]);
}
