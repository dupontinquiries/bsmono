//! Particle color ramp: lerp an RGB triple from `start` to `end` over life
//! (e.g. ember white → orange → dark). Pure, deterministic.

pub fn color(age: f32, lifetime: f32, start: [f32; 3], end: [f32; 3]) -> [f32; 3] {
    let lt = lifetime.max(1e-6);
    let t = (age.max(0.0) / lt).clamp(0.0, 1.0);
    [
        start[0] * (1.0 - t) + end[0] * t,
        start[1] * (1.0 - t) + end[1] * t,
        start[2] * (1.0 - t) + end[2] * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn birth_is_start() { assert_eq!(color(0.0, 10.0, [1.0, 1.0, 1.0], [1.0, 0.5, 0.0]), [1.0, 1.0, 1.0]); }
    #[test] fn death_is_end() { assert_eq!(color(10.0, 10.0, [1.0, 1.0, 1.0], [1.0, 0.5, 0.0]), [1.0, 0.5, 0.0]); }
    #[test] fn mid_between() {
        let c = color(5.0, 10.0, [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]);
        assert!(c[0] > 0.0 && c[0] < 1.0);
    }
    #[test] fn clamped_past_life() { assert_eq!(color(20.0, 10.0, [1.0, 1.0, 1.0], [0.2, 0.2, 0.2]), [0.2, 0.2, 0.2]); }
    #[test] fn channelwise() {
        let c = color(5.0, 10.0, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!((c[0] - 0.5).abs() < 1e-6 && (c[1] - 0.5).abs() < 1e-6);
    }
    #[test] fn deterministic() { assert_eq!(color(5.0, 10.0, [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]), color(5.0, 10.0, [1.0, 1.0, 1.0], [0.0, 0.0, 0.0])); }
}
