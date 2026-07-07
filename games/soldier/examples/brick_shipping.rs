//! brick_shipping — render the SHIPPING brick straight from material_pixel (the real atlas generator) to
//! confirm the wired-in weathering. Tiled 2x2. Run: cargo run --example brick_shipping
use early_fps::mesh::{material_pixel, Material};
fn main() {
    let n = 512usize;
    let mut buf = vec![0u8; n * n * 3];
    for py in 0..n {
        for px in 0..n {
            let u = (px as f32 / n as f32 * 2.0).fract();
            let v = (py as f32 / n as f32 * 2.0).fract();
            let c = material_pixel(Material::Brick, u, v);
            let i = (py * n + px) * 3;
            buf[i] = c.r(); buf[i + 1] = c.g(); buf[i + 2] = c.b();
        }
    }
    image::save_buffer("brick_shipping.png", &buf, n as u32, n as u32, image::ColorType::Rgb8).unwrap();
    println!("wrote brick_shipping.png");
}
