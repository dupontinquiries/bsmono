//! River width from upstream flow accumulation (drainage area): ∝ √(accumulated
//! flow) per hydraulic geometry, capped at max_width. Pure, deterministic.

pub fn width(flow_accumulation: f32, scale: f32, max_width: f32) -> f32 {
    let fa = flow_accumulation.max(0.0);
    let sc = scale.max(0.0);
    let mw = max_width.max(0.0);
    (sc * fa.sqrt()).min(mw)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_flow_zero() { assert_eq!(width(0.0, 0.5, 50.0), 0.0); }
    #[test] fn more_flow_wider() { assert!(width(100.0, 0.5, 50.0) > width(25.0, 0.5, 50.0)); }
    #[test] fn diminishing() {
        let early = width(25.0, 1.0, 1000.0) - width(0.0, 1.0, 1000.0);
        let late = width(100.0, 1.0, 1000.0) - width(75.0, 1.0, 1000.0);
        assert!(early > late);
    }
    #[test] fn capped() { assert_eq!(width(1.0e6, 0.5, 50.0), 50.0); }
    #[test] fn scale_adjusts() { assert!(width(100.0, 1.0, 1000.0) > width(100.0, 0.5, 1000.0)); }
    #[test] fn deterministic() { assert_eq!(width(100.0, 0.5, 50.0).to_bits(), width(100.0, 0.5, 50.0).to_bits()); }
}
