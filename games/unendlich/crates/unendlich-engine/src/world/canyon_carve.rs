//! Canyon depth profile: deepest at the channel, ramps to 0 by `width` (worldgen).

pub fn depth(dist_to_channel: f32, width: f32, max_depth: f32) -> f32 {
    let w = width.max(1e-6);
    let md = max_depth.max(0.0);
    let d = dist_to_channel.abs();
    if d >= w {
        0.0
    } else {
        let t = 1.0 - d / w;
        md * t * t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn deepest_at_channel() { assert!((depth(0.0, 10.0, 50.0) - 50.0).abs() < 1e-4); }
    #[test] fn zero_beyond_width() { assert_eq!(depth(11.0, 10.0, 50.0), 0.0); }
    #[test] fn wider_scales() { assert!(depth(5.0, 20.0, 50.0) > depth(5.0, 10.0, 50.0)); }
    #[test] fn monotone_toward_channel() {
        let (mut prev, mut d) = (f32::MAX, 0.0f32);
        while d < 10.0 { let v = depth(d, 10.0, 50.0); assert!(v <= prev + 1e-6); prev = v; d += 0.5; }
    }
    #[test] fn bounded() {
        let mut d = 0.0f32;
        while d < 20.0 { let v = depth(d, 10.0, 50.0); assert!((0.0..=50.0).contains(&v)); d += 0.3; }
    }
    #[test] fn deterministic() { assert_eq!(depth(3.0, 10.0, 50.0).to_bits(), depth(3.0, 10.0, 50.0).to_bits()); }
}
