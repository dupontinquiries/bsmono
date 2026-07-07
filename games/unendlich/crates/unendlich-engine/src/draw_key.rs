//! draw_key.rs — pure front-to-back draw sort key.
//!
//! Packs a quantized depth bucket into the high bits and a pipeline key into the
//! low bits of a `u64`, so sorting the keys ascending orders draws near→far
//! (depth-major, for early-Z) while grouping equal-depth draws by pipeline state
//! (state-change minimization within a depth band). Pure, deterministic,
//! panic-free (R1/R11/R21).

/// Bits reserved for the pipeline key in the low end of the sort key.
const PIPELINE_BITS: u32 = 40;
/// Bits for the depth bucket in the high end.
const DEPTH_BITS: u32 = 24;

const PIPELINE_MASK: u64 = (1u64 << PIPELINE_BITS) - 1;
const DEPTH_MAX: u64 = (1u64 << DEPTH_BITS) - 1;

/// Build a front-to-back sort key. `depth` is quantized into `DEPTH_BITS` over
/// `[near, far]` (near → 0, so smaller keys sort first), and `pipeline` (e.g. a
/// `pipeline_key`) occupies the low `PIPELINE_BITS`. Sort the resulting keys
/// ascending for a near→far, state-grouped draw order.
pub fn draw_key(depth: f32, pipeline: u64, near: f32, far: f32) -> u64 {
    let near = near.max(0.0);
    let far = far.max(near + 1e-4);
    let t = ((depth - near) / (far - near)).max(0.0).min(1.0);
    let bucket = (t * DEPTH_MAX as f32) as u64; // 0 = nearest
    let bucket = if bucket > DEPTH_MAX { DEPTH_MAX } else { bucket };
    (bucket << PIPELINE_BITS) | (pipeline & PIPELINE_MASK)
}

/// Extract the depth bucket back out of a key (for debugging / tests).
pub fn depth_bucket(key: u64) -> u64 {
    key >> PIPELINE_BITS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearer_sorts_first() {
        let near = draw_key(1.0, 123, 0.0, 100.0);
        let far = draw_key(90.0, 123, 0.0, 100.0);
        assert!(near < far, "nearer should sort before farther");
    }

    #[test]
    fn same_depth_groups_by_pipeline() {
        let a = draw_key(50.0, 10, 0.0, 100.0);
        let b = draw_key(50.0, 20, 0.0, 100.0);
        // same depth bucket -> identical high bits
        assert_eq!(depth_bucket(a), depth_bucket(b), "same depth should share bucket");
        // ordered by pipeline within the bucket
        assert!(a < b, "within a depth band, lower pipeline sorts first");
    }

    #[test]
    fn monotonic_in_depth() {
        let mut prev = 0u64;
        let mut d = 0.0f32;
        while d <= 100.0 {
            let k = draw_key(d, 5, 0.0, 100.0);
            assert!(k >= prev, "key not monotone in depth at {}", d);
            prev = k;
            d += 0.25;
        }
    }

    #[test]
    fn clamps_outside_range() {
        let below = draw_key(-10.0, 0, 0.0, 100.0);
        let at0 = draw_key(0.0, 0, 0.0, 100.0);
        assert_eq!(below, at0, "depths below near clamp to nearest bucket");
        let beyond = draw_key(1000.0, 0, 0.0, 100.0);
        assert_eq!(depth_bucket(beyond), DEPTH_MAX, "beyond far clamps to last bucket");
    }

    #[test]
    fn pipeline_fits_low_bits() {
        // a pipeline value wider than PIPELINE_BITS is masked, never bleeding into depth
        let k = draw_key(50.0, u64::MAX, 0.0, 100.0);
        let expected_bucket = depth_bucket(draw_key(50.0, 0, 0.0, 100.0));
        assert_eq!(depth_bucket(k), expected_bucket, "pipeline bled into depth bits");
    }

    #[test]
    fn deterministic() {
        assert_eq!(draw_key(33.0, 7, 0.0, 100.0), draw_key(33.0, 7, 0.0, 100.0));
    }
}
