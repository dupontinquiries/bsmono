//! model — what a speaker can actually do, which is what every other stage is sized from.
//!
//! A phone speaker is a small sealed-ish driver. Below its low-frequency cutoff the radiated
//! power falls roughly with the fourth power of frequency while the voice coil still moves (and
//! heats) as much as ever, so energy below the cutoff buys NO loudness and costs excursion,
//! heat and battery. That one fact drives the chain:
//!
//! * a protective high-pass at the cutoff (the "power efficiency" half of this crate), and
//! * virtual bass that puts its harmonics ABOVE the cutoff (the "psychoacoustics" half): the ear
//!   infers a missing fundamental from its harmonic series, so a note at 80 Hz is heard from
//!   energy at 400/480/560 Hz that the speaker CAN radiate.
//!
//! ## The case changes the speaker
//!
//! A bare phone speaker starts near 400 Hz. The owner's Fairphone 5 in a Bark Sweden wooden case,
//! running the previous bankstown chain (floor 45 / ceil 180 Hz, 2nd and 3rd harmonics weighted
//! 0.3 / 0.6, a 40 Hz final high-pass, NO high-pass on the main speaker), was judged better than
//! an iPhone by ear by several listeners. That is anecdote, not measurement — but it is the one
//! working reference there is, and it says a case can load the speaker well enough that bass
//! below the bare-phone cutoff is worth sending. So the DEFAULT presets reproduce that chain's
//! protection (a subsonic guard on the main speaker, 400 Hz on the earpiece, as the deployed
//! `hpTop` had), and the bare-phone cutoffs are kept as explicit alternatives. A cutoff is only
//! raised from a sweep measured WITH THE CASE ON.
//!
//! ## Provenance
//!
//! Every preset here is [`Provenance::Assumed`]: the defaults are parity with the old chain, the
//! bare-phone ones are typical-of-class. A measured preset replaces them and flips the tag, so a
//! log line or a UI can say which one it is running on.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Typical-of-class guess. Replace with a measurement.
    Assumed,
    /// Taken from a sweep of this very speaker.
    Measured,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerModel {
    pub name: &'static str,
    /// Lower edge of what the speaker radiates (its -6 dB point), Hz. Nothing below this is worth
    /// sending; virtual-bass harmonics must land above it.
    pub f_low_hz: f32,
    /// Top of the region virtual-bass harmonics are allowed to occupy, Hz. Above it the ear needs
    /// no help and added harmonic energy only reads as harshness.
    pub f_harm_top_hz: f32,
    /// Ceiling of the protective limiter, dBFS. Leaves excursion and thermal margin.
    pub ceiling_dbfs: f32,
    pub provenance: Provenance,
}

impl SpeakerModel {
    /// Fairphone 5 bottom (main) speaker, as the previous chain treated it: a subsonic guard at
    /// 40 Hz (bankstown's own `final_hp`) and nothing else. ASSUMED parity, not a measurement.
    pub fn fp5_bottom() -> SpeakerModel {
        SpeakerModel { name: "fp5-bottom", f_low_hz: 40.0, f_harm_top_hz: 1400.0, ceiling_dbfs: -3.0, provenance: Provenance::Assumed }
    }

    /// Fairphone 5 earpiece used as the upper speaker: 400 Hz, the deployed `hpTop` default — a
    /// receiver cannot take bass. ASSUMED parity, with a lower ceiling since it is built for far
    /// less power.
    pub fn fp5_earpiece() -> SpeakerModel {
        SpeakerModel { name: "fp5-earpiece", f_low_hz: 400.0, f_harm_top_hz: 1800.0, ceiling_dbfs: -6.0, provenance: Provenance::Assumed }
    }

    /// A typical BARE phone main speaker (no case): usable response from ~400 Hz. For comparison
    /// and for cases that do not help; the most power-saving of the assumed models.
    pub fn bare_phone_main() -> SpeakerModel {
        SpeakerModel { name: "bare-main", f_low_hz: 400.0, f_harm_top_hz: 1400.0, ceiling_dbfs: -3.0, provenance: Provenance::Assumed }
    }

    /// A typical bare receiver: ~650 Hz.
    pub fn bare_phone_receiver() -> SpeakerModel {
        SpeakerModel { name: "bare-receiver", f_low_hz: 650.0, f_harm_top_hz: 1800.0, ceiling_dbfs: -6.0, provenance: Provenance::Assumed }
    }
}
