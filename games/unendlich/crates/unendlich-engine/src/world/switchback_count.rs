//! Number of switchbacks to climb an elevation gain while keeping each segment
//! under max_grade: ceil(gain / (max_grade·segment_len)) − 1, floored at 0. Pure.

pub fn count(elevation_gain: f32, max_grade: f32, segment_len: f32) -> u32 {
    let gain = elevation_gain.max(0.0);
    let rise_per_segment = max_grade.max(1e-4) * segment_len.max(1e-4);
    let segments = (gain / rise_per_segment).ceil() as u32;
    segments.saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_gain_zero() { assert_eq!(count(0.0, 0.1, 50.0), 0); }
    #[test] fn one_segment_zero() { assert_eq!(count(5.0, 0.1, 50.0), 0); }
    #[test] fn big_gain_several() { assert!(count(50.0, 0.1, 50.0) > 0); }
    #[test] fn steeper_grade_fewer() { assert!(count(50.0, 0.2, 50.0) < count(50.0, 0.1, 50.0)); }
    #[test] fn longer_segments_fewer() { assert!(count(50.0, 0.1, 100.0) < count(50.0, 0.1, 50.0)); }
    #[test] fn deterministic() { assert_eq!(count(50.0, 0.1, 50.0), count(50.0, 0.1, 50.0)); }
}
