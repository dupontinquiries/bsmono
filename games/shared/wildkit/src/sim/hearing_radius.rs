//! Auditory detection in [0,1]: a sound's loudness above the ambient floor,
//! attenuated by distance. Louder + closer + quieter ambient ⇒ stronger. Pure.

pub fn detect(sound_level: f32, distance: f32, ambient: f32, hearing_range: f32) -> f32 {
    let hr = hearing_range.max(1e-6);
    let d = distance.max(0.0);
    if d >= hr {
        return 0.0;
    }
    let signal = (sound_level.max(0.0) - ambient.max(0.0)).max(0.0);
    (signal * (1.0 - d / hr)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn loud_close_strong() { assert!(detect(1.0, 1.0, 0.0, 50.0) > 0.9); }
    #[test] fn below_ambient_zero() { assert_eq!(detect(0.2, 5.0, 0.5, 50.0), 0.0); }
    #[test] fn too_far_zero() { assert_eq!(detect(1.0, 60.0, 0.0, 50.0), 0.0); }
    #[test] fn closer_stronger() { assert!(detect(1.0, 5.0, 0.0, 50.0) > detect(1.0, 40.0, 0.0, 50.0)); }
    #[test] fn louder_stronger() { assert!(detect(1.0, 5.0, 0.0, 50.0) > detect(0.6, 5.0, 0.0, 50.0)); }
    #[test] fn deterministic() { assert_eq!(detect(1.0, 5.0, 0.0, 50.0).to_bits(), detect(1.0, 5.0, 0.0, 50.0).to_bits()); }
}
