//! Clamped step from `from` toward `to`, moving at most `max_step`. Arrives
//! exactly if within range. Pure, deterministic.

pub fn step_toward(from: [f32; 2], to: [f32; 2], max_step: f32) -> [f32; 2] {
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    let dist = (dx * dx + dy * dy).sqrt();
    let ms = max_step.max(0.0);
    if dist <= ms || dist < 1e-6 {
        to
    } else {
        [from[0] + dx / dist * ms, from[1] + dy / dist * ms]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn within_range_arrives() { assert_eq!(step_toward([0.0, 0.0], [1.0, 0.0], 5.0), [1.0, 0.0]); }
    #[test] fn far_moves_max() { assert_eq!(step_toward([0.0, 0.0], [10.0, 0.0], 3.0), [3.0, 0.0]); }
    #[test] fn zero_step_stays() { assert_eq!(step_toward([0.0, 0.0], [10.0, 0.0], 0.0), [0.0, 0.0]); }
    #[test] fn at_target_stays() { assert_eq!(step_toward([5.0, 5.0], [5.0, 5.0], 3.0), [5.0, 5.0]); }
    #[test] fn distance_moved_is_max() {
        let p = step_toward([0.0, 0.0], [10.0, 10.0], 3.0);
        let d = (p[0] * p[0] + p[1] * p[1]).sqrt();
        assert!((d - 3.0).abs() < 1e-4);
    }
    #[test] fn deterministic() { assert_eq!(step_toward([0.0, 0.0], [10.0, 0.0], 3.0), step_toward([0.0, 0.0], [10.0, 0.0], 3.0)); }
}
