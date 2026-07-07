//! Project a world XZ position onto minimap UV in [0,1]², centered on `center_xz`
//! with `world_radius` mapped to the half-extent. Off-map clamps to the edge.

pub fn project(world_xz: [f32; 2], center_xz: [f32; 2], world_radius: f32) -> [f32; 2] {
    let r = world_radius.max(1e-6);
    let u = 0.5 + (world_xz[0] - center_xz[0]) / (2.0 * r);
    let v = 0.5 + (world_xz[1] - center_xz[1]) / (2.0 * r);
    [u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn center_is_middle() { assert_eq!(project([10.0, 10.0], [10.0, 10.0], 100.0), [0.5, 0.5]); }
    #[test] fn east_higher_u() { assert!(project([60.0, 10.0], [10.0, 10.0], 100.0)[0] > 0.5); }
    #[test] fn offmap_clamped() { assert_eq!(project([10000.0, 10.0], [10.0, 10.0], 100.0)[0], 1.0); }
    #[test] fn bounded() {
        let p = project([12345.0, -9999.0], [0.0, 0.0], 50.0);
        assert!((0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]));
    }
    #[test] fn north_changes_v() { assert!(project([10.0, 60.0], [10.0, 10.0], 100.0)[1] > 0.5); }
    #[test] fn deterministic() { assert_eq!(project([60.0, 10.0], [10.0, 10.0], 100.0), project([60.0, 10.0], [10.0, 10.0], 100.0)); }
}
