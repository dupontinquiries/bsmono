//! Limestone dissolution → sinkholes/caves: saturating growth with limestone
//! fraction, water flow, and time (worldgen). Result in [0,1].

pub fn dissolve(limestone: f32, water_flow: f32, time: f32) -> f32 {
    let l = limestone.clamp(0.0, 1.0);
    let f = water_flow.max(0.0);
    let t = time.max(0.0);
    let rate = l * f * t * 0.05;
    1.0 - (-rate).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_limestone_no_dissolution() { assert_eq!(dissolve(0.0, 5.0, 10.0), 0.0); }
    #[test] fn more_flow_more() { assert!(dissolve(1.0, 8.0, 5.0) > dissolve(1.0, 2.0, 5.0)); }
    #[test] fn time_scales() { assert!(dissolve(1.0, 5.0, 20.0) > dissolve(1.0, 5.0, 5.0)); }
    #[test] fn bounded_unit() {
        for &(f, t) in &[(1.0f32, 1.0f32), (100.0, 100.0), (5.0, 50.0)] {
            let v = dissolve(1.0, f, t); assert!((0.0..=1.0).contains(&v));
        }
    }
    #[test] fn monotone_in_flow() {
        let (mut prev, mut f) = (f32::MIN, 0.0f32);
        while f < 20.0 { let v = dissolve(0.8, f, 5.0); assert!(v >= prev); prev = v; f += 0.5; }
    }
    #[test] fn deterministic() { assert_eq!(dissolve(0.5, 5.0, 5.0).to_bits(), dissolve(0.5, 5.0, 5.0).to_bits()); }
}
