//! bbox — axis bounds folds (guide §4.2). std-only, pure, empty-safe.
/// Bounding interval on one axis across (center, half) pairs.
/// Returns (0.0, 0.0) on empty input (lo>hi sentinel collapsed).
pub fn axis_bounds(boxes: impl Iterator<Item = (f32, f32)>) -> (f32, f32) {
    let (lo, hi) = boxes.fold((f32::MAX, f32::MIN), |(lo, hi), (c, h)| {
        (lo.min(c - h), hi.max(c + h))
    });
    if hi >= lo { (lo, hi) } else { (0.0, 0.0) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_basic_and_empty() {
        let (lo, hi) = axis_bounds([(0.0, 1.0), (3.0, 0.5)].into_iter());
        assert_eq!((lo, hi), (-1.0, 3.5));
        assert_eq!(axis_bounds(std::iter::empty()), (0.0, 0.0));
    }
}
