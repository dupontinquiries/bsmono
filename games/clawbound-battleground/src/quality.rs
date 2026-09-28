//! quality — what to stop drawing when the frame budget is 11ms.
//!
//! 90fps leaves 11.1ms a frame. The costs worth cutting were measured from the
//! shaders rather than guessed at, and they are lopsided:
//!
//! * **`post.wgsl` is the frame.** The bloom ring is 12 iterations of 2 taps —
//!   24 texture samples on EVERY pixel — and the god rays add 22 more. At
//!   1080p that is roughly 50M samples for bloom alone before anything else
//!   happens. Nothing else in the pipeline is in that league.
//! * **The shadow map is 2048².** Four million depth texels, re-rendered every
//!   frame, for a map that covers the whole play area on a phone screen.
//! * The remaining passes (main geometry, UI) scale with what is on screen and
//!   are not where a mobile GPU loses a frame.
//!
//! ## What a tier does NOT change
//!
//! Anything a player could mistake for an advantage. Draw distance stays,
//! because seeing a god sooner is an advantage; shadow RESOLUTION drops but
//! shadows do not disappear, because a shadow is how you read a leap's
//! landing. The rule is that Low should look worse and play identically.

/// How much to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    /// Everything. The desktop default and what the look was designed at.
    #[default]
    High,
    /// The middle setting: cheaper shadows, bloom kept.
    Medium,
    /// For phones. No bloom, no god rays, quarter-area shadow map.
    Low,
    /// High, plus the ultra preview: characters as smooth, furred surfaces
    /// with physically based shading, coloured lights from abilities, and
    /// ray-traced soft shadows between the cats (smoothvox, smooth.wgsl).
    Ultra,
}

/// The player's chosen quality (an index into `Quality::ALL`; High).
static CHOSEN: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

impl Quality {
    pub const ALL: [Quality; 4] = [Quality::Ultra, Quality::High, Quality::Medium, Quality::Low];

    /// Its name in the config file.
    pub fn key(self) -> &'static str {
        match self {
            Quality::Ultra => "ultra",
            Quality::High => "high",
            Quality::Medium => "medium",
            Quality::Low => "low",
        }
    }

    pub fn from_key(k: &str) -> Option<Quality> {
        Quality::ALL.into_iter().find(|q| q.key() == k)
    }

    /// The quality in effect: `--ultra` or CLAWBOUND_QUALITY if given, else
    /// the player's setting (`set_chosen`).
    pub fn effective() -> Quality {
        Quality::requested().unwrap_or_else(|| Quality::ALL[CHOSEN.load(std::sync::atomic::Ordering::Relaxed) as usize % Quality::ALL.len()])
    }

    /// The player's setting, from the config (the app sets it).
    pub fn set_chosen(q: Quality) {
        let i = Quality::ALL.iter().position(|x| *x == q).unwrap_or(1) as u8;
        CHOSEN.store(i, std::sync::atomic::Ordering::Relaxed);
    }

    /// The ultra preview's smooth, furred characters.
    pub fn smooth_characters(self) -> bool {
        self == Quality::Ultra
    }

    /// `--ultra` on the command line or `CLAWBOUND_QUALITY=ultra`
    /// (`high`, `medium`, `low` also read); None if neither says.
    pub fn requested() -> Option<Quality> {
        if std::env::args().any(|a| a == "--ultra") {
            return Some(Quality::Ultra);
        }
        match std::env::var("CLAWBOUND_QUALITY").ok()?.to_ascii_lowercase().as_str() {
            "ultra" => Some(Quality::Ultra),
            "high" => Some(Quality::High),
            "medium" => Some(Quality::Medium),
            "low" => Some(Quality::Low),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::High => "High",
            Quality::Ultra => "Ultra (preview)",
            Quality::Medium => "Medium",
            Quality::Low => "Low (mobile)",
        }
    }

    /// Shadow map edge, in texels.
    ///
    /// Halving the edge quarters the area AND the depth pass's fill: 2048² is
    /// 4.2M texels a frame, 1024² is 1.0M. The map still covers the same world
    /// area, so shadows stay where they were and get softer edges — which on a
    /// phone screen is most of the way to invisible.
    pub fn shadow_size(self) -> u32 {
        match self {
            // Ultra: four times the texels -- crisp shadows from every leaf
            // and paw at the fitted box's full reach
            Quality::Ultra => 4096,
            Quality::High => 2048,
            Quality::Medium => 1024,
            Quality::Low => 1024,
        }
    }

    /// Multiplier on the artist's bloom strength.
    ///
    /// ZERO ON LOW, and zero is load-bearing: `post.wgsl` skips the entire
    /// 24-tap ring when the strength is below 0.001. Scaling to 0.1 would look
    /// nearly the same and still pay for every sample, which is the trap in
    /// making this a slider instead of a switch.
    pub fn bloom_scale(self) -> f32 {
        match self {
            Quality::High | Quality::Ultra => 1.0,
            Quality::Medium => 1.0,
            Quality::Low => 0.0,
        }
    }

    /// Multiplier on god-ray strength. Also zero-skipped in the shader, which
    /// that pass already did on its own.
    pub fn ray_scale(self) -> f32 {
        match self {
            Quality::High | Quality::Ultra => 1.0,
            Quality::Medium => 0.5,
            Quality::Low => 0.0,
        }
    }

    /// Does the water surface get its own reflective treatment?
    ///
    /// Low draws water flat. It is the one visual cut a player might notice as
    /// a CHANGE rather than as "lower settings", so it is worth naming: the
    /// water is still there, still the right colour, still drowns you.
    /// The ground's look (render.rs FrameUniform `look`): how far its fine
    /// detail carries, and its dry sheen. Ultra carries detail to 220 u
    /// (High 120, where it always stopped); Low has no sheen.
    pub fn ground_look(self) -> [f32; 4] {
        match self {
            Quality::Ultra => [220.0, 1.2, 0.0, 0.0],
            Quality::High => [120.0, 1.0, 0.0, 0.0],
            Quality::Medium => [90.0, 0.8, 0.0, 0.0],
            Quality::Low => [60.0, 0.0, 0.0, 0.0],
        }
    }

    pub fn fancy_water(self) -> bool {
        !matches!(self, Quality::Low)
    }

    /// Particles a single effect may spawn, as a fraction of the authored
    /// count.
    ///
    /// Not zero on Low. Particles are how a hit reads as a hit, and a game
    /// where blows land invisibly is a different game rather than an uglier
    /// one.
    pub fn particle_scale(self) -> f32 {
        match self {
            Quality::High | Quality::Ultra => 1.0,
            Quality::Medium => 0.7,
            Quality::Low => 0.4,
        }
    }

    /// A rough per-frame saving over High, as a FRACTION of the post pass.
    ///
    /// Only the two big costs are counted, because they are the two that were
    /// measured. This is for ordering the tiers and for a settings screen to
    /// say something honest — it is not a frame-rate promise, which would need
    /// the device.
    pub fn est_post_saving(self) -> f32 {
        let bloom = if self.bloom_scale() <= 0.001 { 24.0 } else { 0.0 };
        let rays = (1.0 - self.ray_scale()) * 22.0;
        (bloom + rays) / 46.0
    }
}

/// The tier to start on for a display of this size.
///
/// A phone panel is the signal available before any benchmark: nothing here
/// knows the GPU, and guessing a tier from a device name is how a new phone
/// gets the wrong default forever. Overridable — this is a starting point, not
/// a verdict.
pub fn default_for_display(width: u32, height: u32) -> Quality {
    let (short, long) = (width.min(height) as f32, width.max(height) as f32);
    if short <= 0.0 {
        return Quality::High;
    }
    // ASPECT, not size. My first version used "short side <= 1200 and long
    // side >= 1600", which is ALSO true of a 1920x1080 laptop -- short 1080,
    // long 1920 -- so every 1080p desktop would have been given the mobile
    // tier. Measured instead: phones are 2.2:1 (1080x2400, and the Fairphone 5
    // at 1224x2700), every desktop panel from 1080p to 4K is 1.78, and tablets
    // are 1.60. Nothing sits between 1.78 and 2.2, so the gap is real rather
    // than a threshold picked to fit.
    //
    // A short side under 1300 as well, so an ultrawide monitor (2.33:1 at
    // 3440x1440) is not read as an enormous phone.
    let aspect = long / short;
    if aspect >= 2.0 && short <= 1300.0 {
        Quality::Low
    } else {
        Quality::High
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ZERO IS LOAD-BEARING. `post.wgsl` skips the 24-tap ring below 0.001; a
    /// small non-zero value looks nearly identical and pays for every sample.
    #[test]
    fn low_sets_bloom_to_exactly_zero_so_the_ring_is_skipped() {
        assert_eq!(Quality::Low.bloom_scale(), 0.0);
        assert!(Quality::Low.bloom_scale() < 0.001, "the shader's own threshold");
        assert!(Quality::High.bloom_scale() > 0.001, "and High still pays for it");
    }

    /// The two measured costs are the two that get cut, in that order.
    #[test]
    fn low_saves_most_of_the_post_pass() {
        assert!(Quality::Low.est_post_saving() > 0.9, "{}", Quality::Low.est_post_saving());
        assert!(Quality::High.est_post_saving() < 0.01);
        assert!(Quality::Medium.est_post_saving() > Quality::High.est_post_saving());
        assert!(Quality::Low.est_post_saving() > Quality::Medium.est_post_saving());
    }

    /// Halving the edge quarters the fill, which is the point.
    #[test]
    fn the_shadow_map_shrinks_by_area_not_edge() {
        let hi = Quality::High.shadow_size() as f32;
        let lo = Quality::Low.shadow_size() as f32;
        assert!((hi * hi) / (lo * lo) >= 3.9, "expected ~4x fewer texels");
    }

    /// SHADOWS DO NOT DISAPPEAR. A shadow is how a player reads where a leap
    /// lands; removing it would change the game, not the picture.
    #[test]
    fn every_tier_still_has_shadows() {
        for q in Quality::ALL {
            assert!(q.shadow_size() >= 1024, "{q:?} would lose shadows");
        }
    }

    /// Nor do particles. A hit that lands invisibly is a different game.
    #[test]
    fn low_keeps_enough_particles_to_read_a_hit() {
        assert!(Quality::Low.particle_scale() >= 0.3, "a hit must still show");
        assert!(Quality::Low.particle_scale() < Quality::High.particle_scale());
    }

    /// The tiers are ordered, with no setting where Medium beats High or Low
    /// beats Medium.
    #[test]
    fn the_tiers_are_monotonic() {
        let q = Quality::ALL;
        for w in q.windows(2) {
            let (a, b) = (w[0], w[1]);
            assert!(a.shadow_size() >= b.shadow_size(), "{a:?} vs {b:?}");
            assert!(a.bloom_scale() >= b.bloom_scale(), "{a:?} vs {b:?}");
            assert!(a.ray_scale() >= b.ray_scale(), "{a:?} vs {b:?}");
            assert!(a.particle_scale() >= b.particle_scale(), "{a:?} vs {b:?}");
        }
    }

    /// A phone gets Low, a laptop does not. The SHORT side plus a long long
    /// side is the tell -- a 1080p laptop is 1920x1080, short side 1080, and
    /// must not be mistaken for a phone.
    #[test]
    fn a_phone_panel_defaults_to_low_and_a_laptop_does_not() {
        assert_eq!(default_for_display(1080, 2400), Quality::Low, "Fairphone-ish, portrait");
        assert_eq!(default_for_display(2400, 1080), Quality::Low, "and landscape");
        assert_eq!(default_for_display(1920, 1080), Quality::High, "a 1080p laptop");
        assert_eq!(default_for_display(2560, 1440), Quality::High);
        assert_eq!(default_for_display(3840, 2160), Quality::High);
        // An ultrawide is 2.33:1 -- past the aspect gate -- and must not be
        // read as an enormous phone. Its short side is what saves it.
        assert_eq!(default_for_display(3440, 1440), Quality::High, "ultrawide");
        assert_eq!(default_for_display(1600, 2560), Quality::High, "a tablet at 1.6");
        // A degenerate size must not panic or divide by zero.
        assert_eq!(default_for_display(0, 0), Quality::High);
    }

    /// Ultra is High plus the preview: every setting High pays for, it pays
    /// for too; only it draws smooth characters.
    #[test]
    fn ultra_is_high_plus_the_preview() {
        let (u, h) = (Quality::Ultra, Quality::High);
        // (and more: four times High's shadow texels, detail carried
        // further -- `ground_look`)
        assert_eq!(u.shadow_size(), h.shadow_size() * 2);
        assert!(u.ground_look()[0] > h.ground_look()[0] && u.ground_look()[1] >= h.ground_look()[1]);
        assert_eq!(u.bloom_scale(), h.bloom_scale());
        assert_eq!(u.particle_scale(), h.particle_scale());
        assert!(u.smooth_characters());
        assert!(Quality::ALL.iter().filter(|q| q.smooth_characters()).count() == 1);
        assert_eq!(u.label(), "Ultra (preview)");
    }
}
