//! Manhattan (L-shaped) corridor length between two room centers — matches grid
//! dungeons whose corridors run axis-aligned. Pure, deterministic.

pub fn length(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    (bx - ax).abs() + (by - ay).abs()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn same_zero() { assert_eq!(length(0.0, 0.0, 0.0, 0.0), 0.0); }
    #[test] fn horizontal() { assert_eq!(length(0.0, 0.0, 5.0, 0.0), 5.0); }
    #[test] fn vertical() { assert_eq!(length(0.0, 0.0, 0.0, 3.0), 3.0); }
    #[test] fn l_shape() { assert_eq!(length(0.0, 0.0, 3.0, 4.0), 7.0); }
    #[test] fn symmetric() { assert_eq!(length(1.0, 2.0, 5.0, 6.0), length(5.0, 6.0, 1.0, 2.0)); }
    #[test] fn deterministic() { assert_eq!(length(1.0, 2.0, 5.0, 6.0).to_bits(), length(1.0, 2.0, 5.0, 6.0).to_bits()); }
}
