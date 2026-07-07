use early_fps::mesh::build_material_atlas;
use std::io::Write;
fn main() {
    let (rgba, w, h) = build_material_atlas(512, 5);
    // write PPM (P6) — easy to convert
    let mut f = std::fs::File::create("/tmp/atlas.ppm").unwrap();
    write!(f, "P6\n{} {}\n255\n", w, h).unwrap();
    let mut rgb = Vec::with_capacity(w * h * 3);
    for px in rgba.chunks(4) {
        rgb.push(px[0]);
        rgb.push(px[1]);
        rgb.push(px[2]);
    }
    f.write_all(&rgb).unwrap();
    println!("wrote /tmp/atlas.ppm {}x{}", w, h);
}
