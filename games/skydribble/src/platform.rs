//! Platform detection — is this a phone? — plus what THIS game does about it.
//!
//! The detection half now lives in `games/shared/handheld-detect`: it was
//! byte-identical here and in the other game, and "which machine is this"
//! has one right answer both want. See that crate's doc for where the seam
//! is and why it stops here.
//!
//! What stays below is [`ApplyProfile::apply`]. Render scale, bloom, vsync,
//! camera distance and gyro gating are this game's opinion about how it
//! should look on a small GPU, not shared knowledge — and blackstar-rs is
//! deliberately diverging into its own game, so the tuning is exactly the
//! part that should stay free to differ. The shared crate has no `Config`
//! type and no `apply`.
//!
//! The target that prompted the module:
//!
//! ```text
//! Linux fp5-black 7.1.2 #1-postmarketos-qcom-sc7280 SMP PREEMPT aarch64
//! ```

// Re-exported so every existing `platform::Profile` / `platform::classify`
// / `platform::Detection` call site keeps working unchanged — the
// extraction moves where the code lives, not how this game spells it.
pub use handheld_detect::{classify, Detection, Profile};

/// The environment variable that forces a profile for THIS game.
///
/// Both games used to read `SKYDRIBBLE_MOBILE`, because the file was copied
/// and the name came with it — so overriding the other game's detection
/// meant setting a variable named after this one, unguessable without
/// reading the source. The shared crate takes the name from the caller so
/// each game owns its own.
pub const MOBILE_ENV: &str = "SKYDRIBBLE_MOBILE";

/// Detect, honouring [`MOBILE_ENV`].
pub fn detect() -> Detection {
    handheld_detect::detect(MOBILE_ENV)
}

/// What this game does about being on a handheld.
///
/// A trait rather than an inherent impl because `Profile` is a foreign type
/// now — the orphan rule forbids `impl Profile { .. }` here, and a free
/// `apply(profile, cfg)` function would have changed every call site. This
/// keeps `Profile::Handheld.apply(&mut cfg)` reading exactly as it did.
pub trait ApplyProfile {
    fn apply(self, cfg: &mut crate::config::Config);
}

impl ApplyProfile for Profile {
    /// Adapt the config for the detected machine. Only ever called when no
    /// save file existed OR the user asked to re-apply, so a player who
    /// tunes their phone's settings keeps them.
    fn apply(self, cfg: &mut crate::config::Config) {
        // GYRO FLYING on a handheld: motion controls are the natural way to
        // fly a car you cannot get a second thumb onto, so they arrive on,
        // gated to the air where they help and cannot upset your driving.
        cfg.gyro.on = true;
        cfg.gyro.gate = 2;
        cfg.gyro.air_mode = 0;
        if self != Profile::Handheld {
            return;
        }
        // ---- low graphics ----
        cfg.video.quality = 0.30; // AO off, short shadows, cheap reflections
        cfg.video.rays = false; // no mirror marching on a phone GPU
        cfg.video.bloom = cfg.video.bloom.min(0.35);
        cfg.video.screen_door = 0.0;
        cfg.video.render_scale = 0.5; // HALF resolution scene, native UI
        // Fifo: the Wayland compositor paces us instead of fighting us. The
        // monorepo's own Fairphone notes call this the stutter fix.
        cfg.video.vsync = true;
        // ---- touch ----
        cfg.touch.enabled = true;
        // a phone screen is small: bring the camera in a little
        cfg.cam.dist = cfg.cam.dist.min(6.2);
    }
}
