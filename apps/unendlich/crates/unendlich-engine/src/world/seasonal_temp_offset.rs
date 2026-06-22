//! Temperature offset from season phase: warmer in summer (+), colder in winter
//! (−), scaled by the region's seasonal amplitude. Pure, deterministic.

pub fn temp_offset(season_phase: f32, amplitude: f32) -> f32 {
    season_phase.clamp(-1.0, 1.0) * amplitude.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn summer_warmer() { assert!(temp_offset(1.0, 15.0) > 0.0); }
    #[test] fn winter_colder() { assert!(temp_offset(-1.0, 15.0) < 0.0); }
    #[test] fn equinox_zero() { assert_eq!(temp_offset(0.0, 15.0), 0.0); }
    #[test] fn amplitude_scales() { assert!(temp_offset(1.0, 20.0) > temp_offset(1.0, 10.0)); }
    #[test] fn bounded() {
        let mut sp = -1.0f32;
        while sp <= 1.0 { assert!(temp_offset(sp, 15.0).abs() <= 15.0 + 1e-6); sp += 0.1; }
    }
    #[test] fn deterministic() { assert_eq!(temp_offset(0.5, 15.0).to_bits(), temp_offset(0.5, 15.0).to_bits()); }
}
