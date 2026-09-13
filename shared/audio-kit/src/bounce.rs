//! bounce — what a library-bound render is encoded as.
//!
//! Lifted from `apps/studio/crates/studio-app/src/morph_pad.rs`, where it
//! was the morph pad's bounce format, because beatspace's arrangement
//! projects, saved beats and exports wanted the same choice and the same
//! reasoning. It is a PER-DEVICE choice — a workspace sidecar's, never a
//! project's — because storage constraints are per device: a Fairphone
//! and a studio desktop editing one project want different answers.
//!
//! The rungs ascend in storage. High-quality Opus is the default because
//! on most systems and most ears it IS the sound; FLAC is the lossless
//! rung for the rest; WAV exists at the top for whoever insists — never as
//! anyone's default. A build without an Opus encoder falls to the lossless
//! rung, not to silence: bigger, never a hole.
//!
//! Pure data. The encoders live beside it (`flac`) and in `opus-kit`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BounceFormat {
    Opus32,
    Opus64,
    Opus128,
    Opus192,
    Opus256,
    Flac,
    Wav,
}

impl Default for BounceFormat {
    fn default() -> Self {
        BounceFormat::Opus192
    }
}

impl BounceFormat {
    pub const ALL: [BounceFormat; 7] = [
        BounceFormat::Opus32, BounceFormat::Opus64, BounceFormat::Opus128,
        BounceFormat::Opus192, BounceFormat::Opus256, BounceFormat::Flac, BounceFormat::Wav,
    ];

    pub fn label(self) -> &'static str {
        match self {
            // 32k/64k are voice rungs, not music ones — named as such so a
            // slider doesn't imply they're a smaller version of the same
            // thing 128k is.
            BounceFormat::Opus32 => "opus 32k (voice)",
            BounceFormat::Opus64 => "opus 64k (voice)",
            BounceFormat::Opus128 => "opus 128k",
            BounceFormat::Opus192 => "opus 192k",
            BounceFormat::Opus256 => "opus 256k",
            BounceFormat::Flac => "flac (lossless)",
            BounceFormat::Wav => "wav (uncompressed)",
        }
    }

    /// Sidecar round-trip token — stable, label-independent.
    pub fn token(self) -> &'static str {
        match self {
            BounceFormat::Opus32 => "opus-32",
            BounceFormat::Opus64 => "opus-64",
            BounceFormat::Opus128 => "opus-128",
            BounceFormat::Opus192 => "opus-192",
            BounceFormat::Opus256 => "opus-256",
            BounceFormat::Flac => "flac",
            BounceFormat::Wav => "wav",
        }
    }

    /// An unknown token is the default, not an error: the sidecar is
    /// tolerant by design.
    pub fn from_token(t: &str) -> BounceFormat {
        let t = t.trim();
        Self::ALL.iter().copied().find(|f| f.token() == t).unwrap_or_default()
    }

    pub fn ext(self) -> &'static str {
        match self {
            BounceFormat::Flac => "flac",
            BounceFormat::Wav => "wav",
            _ => "opus",
        }
    }

    pub fn is_opus(self) -> bool {
        self.bitrate_bps().is_some()
    }

    /// The Opus target bitrate, for the Opus rungs.
    pub fn bitrate_bps(self) -> Option<i32> {
        match self {
            BounceFormat::Opus32 => Some(32_000),
            BounceFormat::Opus64 => Some(64_000),
            BounceFormat::Opus128 => Some(128_000),
            BounceFormat::Opus192 => Some(192_000),
            BounceFormat::Opus256 => Some(256_000),
            BounceFormat::Flac | BounceFormat::Wav => None,
        }
    }

    /// What this rung becomes on a build that cannot encode Opus: the
    /// lossless rung. Bigger, never a hole.
    pub fn without_opus(self) -> BounceFormat {
        if self.is_opus() { BounceFormat::Flac } else { self }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|f| *f == self).unwrap_or(3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_round_trip_and_unknown_is_the_default() {
        for f in BounceFormat::ALL {
            assert_eq!(BounceFormat::from_token(f.token()), f);
            assert_eq!(BounceFormat::from_token(&format!("  {}  ", f.token())), f);
        }
        assert_eq!(BounceFormat::from_token("mp3"), BounceFormat::Opus192);
        assert_eq!(BounceFormat::default(), BounceFormat::Opus192, "never wav");
    }

    #[test]
    fn rungs_ascend_in_storage_and_the_opus_ones_fall_to_flac() {
        assert_eq!(BounceFormat::ALL[0], BounceFormat::Opus32);
        assert_eq!(BounceFormat::ALL[6], BounceFormat::Wav);
        assert_eq!(BounceFormat::Opus192.bitrate_bps(), Some(192_000));
        assert_eq!(BounceFormat::Flac.bitrate_bps(), None);
        assert_eq!(BounceFormat::Opus32.without_opus(), BounceFormat::Flac);
        assert_eq!(BounceFormat::Wav.without_opus(), BounceFormat::Wav);
        assert_eq!(BounceFormat::Opus256.ext(), "opus");
        assert_eq!(BounceFormat::Opus192.index(), 3);
    }
}
