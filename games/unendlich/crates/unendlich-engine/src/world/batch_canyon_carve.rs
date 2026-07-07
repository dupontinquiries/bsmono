//! Perf: batched canyon depth profile, bit-identical to the scalar reference.
//! Mirrors canyon_carve: md·(1 − d/width)² inside width, else 0.

pub fn depth_one(dist_to_channel: f32, width: f32, max_depth: f32) -> f32 {
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

pub fn depth_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(d, w, m)| depth_one(d, w, m)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 10.0f32, 50.0f32), (5.0, 10.0, 50.0), (20.0, 10.0, 50.0), (3.0, 8.0, 30.0)];
        let b = depth_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), depth_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(depth_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(depth_batch(&[(2.0, 10.0, 50.0)])[0].to_bits(), depth_one(2.0, 10.0, 50.0).to_bits()); }
    #[test] fn channel_is_max() { assert!((depth_one(0.0, 10.0, 50.0) - 50.0).abs() < 1e-4); }
    #[test] fn beyond_width_zero() { assert_eq!(depth_one(11.0, 10.0, 50.0), 0.0); }
    #[test] fn deterministic() { assert_eq!(depth_batch(&[(3.0, 10.0, 50.0)]), depth_batch(&[(3.0, 10.0, 50.0)])); }
}
