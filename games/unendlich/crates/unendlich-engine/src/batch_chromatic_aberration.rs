//! Perf: batched chromatic-aberration offset, bit-identical to the scalar ref.
//! Mirrors chromatic_aberration: radial offset (uv-0.5)*strength, clamped ±0.5.

pub fn ca_one(uv: [f32; 2], strength: f32) -> [f32; 2] {
    let dx = (uv[0] - 0.5) * strength;
    let dy = (uv[1] - 0.5) * strength;
    [dx.clamp(-0.5, 0.5), dy.clamp(-0.5, 0.5)]
}

pub fn ca_batch(uvs: &[[f32; 2]], strength: f32) -> Vec<[f32; 2]> {
    uvs.iter().map(|&uv| ca_one(uv, strength)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn bit_parity() {
        let uvs = [[0.0, 0.0], [1.0, 1.0], [0.5, 0.25], [0.9, 0.1]];
        let b = ca_batch(&uvs, 0.3);
        for (i, &uv) in uvs.iter().enumerate() {
            let s = ca_one(uv, 0.3);
            assert_eq!(b[i][0].to_bits(), s[0].to_bits());
            assert_eq!(b[i][1].to_bits(), s[1].to_bits());
        }
    }
    #[test] fn empty_is_empty() { assert!(ca_batch(&[], 0.3).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(ca_batch(&[[0.8, 0.2]], 0.4)[0], ca_one([0.8, 0.2], 0.4)); }
    #[test] fn center_zero() { assert_eq!(ca_one([0.5, 0.5], 0.5), [0.0, 0.0]); }
    #[test] fn bounded() {
        let o = ca_one([10.0, -10.0], 5.0);
        assert!(o[0].abs() <= 0.5 && o[1].abs() <= 0.5);
    }
    #[test] fn deterministic() { assert_eq!(ca_batch(&[[0.7, 0.3]], 0.2), ca_batch(&[[0.7, 0.3]], 0.2)); }
}
