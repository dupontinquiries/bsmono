//! Reverb wet/dry mix from enclosure (0 open → 1 sealed) and room size. More
//! enclosed + larger ⇒ wetter. In [0,1]. Pure, deterministic.

pub fn wetness(enclosure: f32, room_size: f32) -> f32 {
    let e = enclosure.clamp(0.0, 1.0);
    let rs = room_size.clamp(0.0, 1.0);
    (e * (0.3 + 0.7 * rs)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn open_is_dry() { assert_eq!(wetness(0.0, 0.8), 0.0); }
    #[test] fn sealed_large_wet() { assert!((wetness(1.0, 1.0) - 1.0).abs() < 1e-6); }
    #[test] fn larger_wetter() { assert!(wetness(1.0, 1.0) > wetness(1.0, 0.2)); }
    #[test] fn bounded() {
        for i in 0..11 { for j in 0..11 { let v = wetness(i as f32 / 10.0, j as f32 / 10.0); assert!((0.0..=1.0).contains(&v)); } }
    }
    #[test] fn monotone_in_enclosure() {
        let (mut prev, mut e) = (f32::MIN, 0.0f32);
        while e <= 1.0 { let w = wetness(e, 0.5); assert!(w >= prev); prev = w; e += 0.1; }
    }
    #[test] fn deterministic() { assert_eq!(wetness(0.7, 0.5).to_bits(), wetness(0.7, 0.5).to_bits()); }
}
