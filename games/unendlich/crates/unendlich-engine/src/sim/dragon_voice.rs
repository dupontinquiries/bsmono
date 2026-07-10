//! `sim::dragon_voice` — a dragon's voice as a deterministic acoustic expression
//! of its genome.
//!
//! From `03_Voice_and_Audio/dragon_voice.docx`. A dragon does not *choose* its
//! voice: the same genetic variables that set physical size, elemental attunement,
//! and disposition also set the fundamental frequency, harmonic-series shape,
//! formant structure, noise layer, and envelope. Two dragons of a lineage sound
//! recognisably related; opposite attunements sound like different instruments in
//! different registers. The voice is not static — **age deepens the fundamental**,
//! and years of increasingly complex bending **sharpen the harmonic precision**.
//!
//! Pure and deterministic: same genome → same voice, every time.

/// Elemental attunement, which sets the harmonic character / register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VoiceElement {
    Fire,
    Storm,
    Air,
    Ice,
    Water,
    Earth,
}

impl VoiceElement {
    /// Baseline harmonic brightness (0 = dark/low-passed, 1 = bright/harmonic-rich).
    pub const fn brightness_base(self) -> f32 {
        match self {
            VoiceElement::Storm => 0.90,
            VoiceElement::Fire => 0.85,
            VoiceElement::Air => 0.72,
            VoiceElement::Ice => 0.66,
            VoiceElement::Water => 0.48,
            VoiceElement::Earth => 0.28,
        }
    }
    /// Register multiplier on the fundamental — airy elements sit higher, heavy
    /// elements lower, so opposite attunements read as different instruments.
    pub const fn register_shift(self) -> f32 {
        match self {
            VoiceElement::Air => 1.18,
            VoiceElement::Storm => 1.10,
            VoiceElement::Fire => 1.04,
            VoiceElement::Ice => 0.98,
            VoiceElement::Water => 0.90,
            VoiceElement::Earth => 0.80,
        }
    }
    /// Map a genome attunement index (0..6) to an element (bridge for the live
    /// dragon genome, which stores attunement numerically).
    pub const fn from_index(i: u8) -> VoiceElement {
        match i % 6 {
            0 => VoiceElement::Fire,
            1 => VoiceElement::Storm,
            2 => VoiceElement::Air,
            3 => VoiceElement::Ice,
            4 => VoiceElement::Water,
            _ => VoiceElement::Earth,
        }
    }
}

/// The computed acoustic profile of a dragon's voice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragonVoice {
    /// Vocal fundamental (Hz). Larger and older dragons are lower.
    pub fundamental_hz: f32,
    /// Peak vocal-tract formant (Hz). `f_peak *= (50/mass)^0.15` — larger = lower.
    pub formant_peak_hz: f32,
    /// Harmonic brightness 0..1 (element base, sharpened by bending experience).
    pub brightness: f32,
    /// Harmonic precision 0..1 — how cleanly the partials line up. Rises with years
    /// of complex bending.
    pub harmonic_precision: f32,
    /// Sub/infrasonic ground-resonance frequency (Hz) the voice excites — lower for
    /// more massive dragons.
    pub ground_resonance_hz: f32,
}

/// Reference mass (kg) the formant formula is normalised around.
const REF_MASS_KG: f32 = 50.0;

#[inline]
fn powf_pos(base: f32, exp: f32) -> f32 {
    if base <= 0.0 {
        0.0
    } else {
        base.powf(exp)
    }
}

impl DragonVoice {
    /// Compute the voice from genome-derived quantities:
    /// - `mass_kg`: physical size (larger → lower fundamental & formants).
    /// - `element`: elemental attunement (harmonic character / register).
    /// - `age_years`: age (deepens the fundamental, asymptotically).
    /// - `bending_experience`: 0..1, years of increasingly complex bending
    ///   (sharpens harmonic precision and lifts brightness slightly).
    pub fn from_genome(
        mass_kg: f32,
        element: VoiceElement,
        age_years: f32,
        bending_experience: f32,
    ) -> DragonVoice {
        let mass = mass_kg.max(1.0);
        let exp = bending_experience.clamp(0.0, 1.0);
        let age = age_years.max(0.0);

        // Fundamental: a ~50 kg juvenile sits near 200 Hz; scales down with mass^0.33
        // (bigger = lower), shifted by elemental register.
        let size_pitch = 200.0 * powf_pos(REF_MASS_KG / mass, 0.33);
        // Age deepens asymptotically: -up to ~30% over a long life.
        let age_deepen = 1.0 - 0.30 * (age / (age + 40.0));
        let fundamental_hz = (size_pitch * element.register_shift() * age_deepen).max(12.0);

        // Formant peak — the doc's `f_peak *= (50/mass)^0.15` around a 1000 Hz base.
        let formant_peak_hz = 1000.0 * powf_pos(REF_MASS_KG / mass, 0.15);

        // Brightness: element base, lifted toward 1 by bending experience.
        let brightness = (element.brightness_base() + 0.15 * exp).clamp(0.0, 1.0);

        // Harmonic precision rises with experience (a practised bender's partials
        // line up cleanly); a floor of 0.2 for a naive dragon.
        let harmonic_precision = (0.2 + 0.8 * exp).clamp(0.0, 1.0);

        // Ground resonance: subsonic, lower for heavier dragons.
        let ground_resonance_hz = 30.0 * powf_pos(REF_MASS_KG / mass, 0.20);

        DragonVoice {
            fundamental_hz,
            formant_peak_hz,
            brightness,
            harmonic_precision,
            ground_resonance_hz,
        }
    }

    /// Whether two voices read as "the same lineage": close fundamentals and
    /// matching element register. A quick similarity gate for family resemblance.
    pub fn same_lineage_as(&self, other: &DragonVoice, hz_tol: f32) -> bool {
        (self.fundamental_hz - other.fundamental_hz).abs() <= hz_tol
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn larger_dragons_are_lower() {
        let small = DragonVoice::from_genome(50.0, VoiceElement::Fire, 0.0, 0.0);
        let big = DragonVoice::from_genome(2000.0, VoiceElement::Fire, 0.0, 0.0);
        assert!(big.fundamental_hz < small.fundamental_hz, "bigger = lower pitch");
        assert!(big.formant_peak_hz < small.formant_peak_hz, "bigger = lower formants");
        assert!(big.ground_resonance_hz < small.ground_resonance_hz, "bigger = lower subsonic");
    }

    #[test]
    fn formant_formula_matches_doc() {
        // f_peak = 1000 * (50/mass)^0.15
        let v = DragonVoice::from_genome(400.0, VoiceElement::Water, 0.0, 0.0);
        let expected = 1000.0 * (50.0f32 / 400.0).powf(0.15);
        assert!((v.formant_peak_hz - expected).abs() < 0.5);
    }

    #[test]
    fn age_deepens_fundamental() {
        let young = DragonVoice::from_genome(300.0, VoiceElement::Storm, 0.0, 0.5);
        let old = DragonVoice::from_genome(300.0, VoiceElement::Storm, 200.0, 0.5);
        assert!(old.fundamental_hz < young.fundamental_hz, "age lowers pitch");
    }

    #[test]
    fn experience_sharpens_precision_and_brightness() {
        let naive = DragonVoice::from_genome(300.0, VoiceElement::Ice, 10.0, 0.0);
        let master = DragonVoice::from_genome(300.0, VoiceElement::Ice, 10.0, 1.0);
        assert!(master.harmonic_precision > naive.harmonic_precision);
        assert!(master.brightness >= naive.brightness);
    }

    #[test]
    fn opposite_elements_sit_in_different_registers() {
        let airy = DragonVoice::from_genome(300.0, VoiceElement::Air, 10.0, 0.5);
        let heavy = DragonVoice::from_genome(300.0, VoiceElement::Earth, 10.0, 0.5);
        // Same mass/age, but Air sits clearly above Earth.
        assert!(airy.fundamental_hz > heavy.fundamental_hz * 1.2);
        assert!(airy.brightness > heavy.brightness);
    }

    #[test]
    fn deterministic_and_lineage_gate() {
        let a = DragonVoice::from_genome(300.0, VoiceElement::Fire, 12.0, 0.4);
        let b = DragonVoice::from_genome(300.0, VoiceElement::Fire, 12.0, 0.4);
        assert_eq!(a, b); // same genome → identical voice
        assert!(a.same_lineage_as(&b, 1.0));
        let distant = DragonVoice::from_genome(1500.0, VoiceElement::Fire, 12.0, 0.4);
        assert!(!a.same_lineage_as(&distant, 5.0));
    }

    #[test]
    fn element_index_bridge_wraps() {
        assert_eq!(VoiceElement::from_index(0), VoiceElement::Fire);
        assert_eq!(VoiceElement::from_index(6), VoiceElement::Fire);
        assert_eq!(VoiceElement::from_index(5), VoiceElement::Earth);
    }
}
