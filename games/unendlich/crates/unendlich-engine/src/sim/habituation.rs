//! Diminished response to a repeated stimulus: base × exp(−rate·exposures). Pure.

pub fn response(base_response: f32, exposures: u32, rate: f32) -> f32 {
    let b = base_response.max(0.0);
    let n = exposures as f32;
    let r = rate.max(0.0);
    b * (-(r * n)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_exposures_base() { assert_eq!(response(1.0, 0, 0.2), 1.0); }
    #[test] fn more_exposures_less() { assert!(response(1.0, 10, 0.2) < response(1.0, 2, 0.2)); }
    #[test] fn higher_rate_faster() { assert!(response(1.0, 5, 0.5) < response(1.0, 5, 0.1)); }
    #[test] fn bounded_by_base() { assert!(response(1.0, 3, 0.2) <= 1.0); }
    #[test] fn monotone_decreasing() {
        let mut prev = f32::MAX;
        for n in 0..30u32 { let v = response(1.0, n, 0.2); assert!(v <= prev); prev = v; }
    }
    #[test] fn deterministic() { assert_eq!(response(1.0, 5, 0.2).to_bits(), response(1.0, 5, 0.2).to_bits()); }
}
