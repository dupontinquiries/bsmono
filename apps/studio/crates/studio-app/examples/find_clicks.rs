//! Clicks (not drum attacks) in a raw render: 48 kHz stereo f32le, as
//! `render_project` writes. `find_clicks render.f32 [offset_s]`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let b = std::fs::read(&a[1]).expect("render");
    let off: f64 = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(0.0);
    let (l, r): (Vec<f32>, Vec<f32>) = b.chunks_exact(8).map(|c| (f32::from_le_bytes(c[0..4].try_into().unwrap()), f32::from_le_bytes(c[4..8].try_into().unwrap()))).unzip();
    let mut all: Vec<f64> = studio_app::reference_check::clicks(&l, 48_000, 8.0);
    all.extend(studio_app::reference_check::clicks(&r, 48_000, 8.0));
    all.sort_by(f64::total_cmp);
    all.dedup_by(|x, y| (*x - *y).abs() < 0.005);
    println!("{} clicks in {:.1} s{}", all.len(), l.len() as f64 / 48_000.0,
        if all.is_empty() { String::new() } else { format!(": {}", all.iter().take(20).map(|t| format!("{:.3}", t + off)).collect::<Vec<_>>().join(" ")) });
}
