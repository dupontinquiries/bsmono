//! Center-to-center spacing of buildings on a grid: plot footprint plus road width.
//! Pure, deterministic.

pub fn spacing(plot_size: f32, road_width: f32) -> f32 {
    plot_size.max(0.0) + road_width.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn sums() { assert_eq!(spacing(10.0, 4.0), 14.0); }
    #[test] fn bigger_plot_more() { assert!(spacing(20.0, 4.0) > spacing(10.0, 4.0)); }
    #[test] fn wider_road_more() { assert!(spacing(10.0, 8.0) > spacing(10.0, 4.0)); }
    #[test] fn zero_road_is_plot() { assert_eq!(spacing(10.0, 0.0), 10.0); }
    #[test] fn nonneg() { assert!(spacing(10.0, 4.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(spacing(10.0, 4.0).to_bits(), spacing(10.0, 4.0).to_bits()); }
}
