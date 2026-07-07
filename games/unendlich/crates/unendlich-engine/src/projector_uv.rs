//! Project a world position into a box projector's UV in [0,1]², centered on
//! `origin` with half-extent `half_size`. Outside the box clamps to the edge. Pure.

pub fn project(world_pos: [f32; 2], origin: [f32; 2], half_size: f32) -> [f32; 2] {
    let hs = half_size.max(1e-6);
    let u = 0.5 + (world_pos[0] - origin[0]) / (2.0 * hs);
    let v = 0.5 + (world_pos[1] - origin[1]) / (2.0 * hs);
    [u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn center_is_middle() { assert_eq!(project([5.0, 5.0], [5.0, 5.0], 10.0), [0.5, 0.5]); }
    #[test] fn plus_x_higher_u() { assert!(project([12.0, 5.0], [5.0, 5.0], 10.0)[0] > 0.5); }
    #[test] fn outside_clamped() { assert_eq!(project([1000.0, 5.0], [5.0, 5.0], 10.0)[0], 1.0); }
    #[test] fn bounded() {
        let p = project([999.0, -999.0], [0.0, 0.0], 5.0);
        assert!((0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]));
    }
    #[test] fn plus_y_higher_v() { assert!(project([5.0, 12.0], [5.0, 5.0], 10.0)[1] > 0.5); }
    #[test] fn deterministic() { assert_eq!(project([12.0, 5.0], [5.0, 5.0], 10.0), project([12.0, 5.0], [5.0, 5.0], 10.0)); }
}
