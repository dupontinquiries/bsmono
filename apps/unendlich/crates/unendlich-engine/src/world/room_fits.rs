//! Whether a (room_w × room_h) room fits in a (region_w × region_h) region with a
//! `margin` border on every side. Pure, deterministic.

pub fn fits(room_w: f32, room_h: f32, region_w: f32, region_h: f32, margin: f32) -> bool {
    let m = margin.max(0.0);
    room_w.max(0.0) + 2.0 * m <= region_w.max(0.0) && room_h.max(0.0) + 2.0 * m <= region_h.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn fits_small() { assert!(fits(5.0, 5.0, 10.0, 10.0, 0.0)); }
    #[test] fn too_wide() { assert!(!fits(12.0, 5.0, 10.0, 10.0, 0.0)); }
    #[test] fn too_tall() { assert!(!fits(5.0, 12.0, 10.0, 10.0, 0.0)); }
    #[test] fn exact_fit() { assert!(fits(10.0, 10.0, 10.0, 10.0, 0.0)); }
    #[test] fn margin_blocks() { assert!(!fits(10.0, 10.0, 10.0, 10.0, 1.0)); }
    #[test] fn deterministic() { assert_eq!(fits(5.0, 5.0, 10.0, 10.0, 0.0), fits(5.0, 5.0, 10.0, 10.0, 0.0)); }
}
