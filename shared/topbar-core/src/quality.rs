//! quality — device-capability tiering, and what a shell should turn down
//! when it lands on the weak end. Exists because the fleet spans real
//! hardware Vulkan (Fairphone 5's Adreno 690, via Turnip) down to no GPU
//! acceleration at all (PinePhone's Mali-400/450 has no Vulkan driver and
//! never will — see `topbar-render`'s own notes) with a software-rasterizer
//! middle ground (lavapipe/llvmpipe) in between that runs but is slow
//! enough to make full animation + translucency a bad default.
//!
//! This module doesn't detect anything itself — `topbar-render` does that
//! (it's the crate that actually has a `wgpu::AdapterInfo` to look at) and
//! maps its finding onto [`RenderTier`], defined here so `topbar-core`
//! doesn't need a wgpu dependency just to know there are three tiers.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderTier {
    /// A real GPU driver: Turnip/RADV/ANV-class hardware acceleration.
    Hardware,
    /// A software rasterizer standing in for a GPU (lavapipe under
    /// Vulkan, llvmpipe under GL) — runs correctly, but every pixel is a
    /// CPU cycle, so effects that are nearly free on real hardware
    /// (translucency, per-frame restack animation) are not free here.
    SoftwareEmulated,
    /// No GPU path at all — `systemcompkit::wayland_io::paint_argb8888`'s
    /// wl_shm path. The floor every device in the fleet can reach
    /// regardless of driver support, per its own doc.
    ShmOnly,
}

/// What a shell should turn on/off for a given tier. Deliberately just two
/// booleans, not a general settings struct — these are specifically the
/// two costs that scale with fill-rate/overdraw (translucency) and with
/// per-frame tile-count work (animation), which is exactly what a
/// software rasterizer or no-GPU path can't absorb for free. Anything
/// else (icon resolution quality, text hinting, etc.) isn't this module's
/// concern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderQuality {
    pub animate: bool,
    pub translucent: bool,
}

impl RenderTier {
    pub fn quality(&self) -> RenderQuality {
        match self {
            RenderTier::Hardware => RenderQuality { animate: true, translucent: true },
            // Software rasterizers handle solid fills fine; it's
            // full-surface alpha blending (translucency) and animating
            // many tiles every frame that cost real time here — cut both,
            // keep the bar itself instant and legible rather than a slow
            // approximation of the hardware-tier look.
            RenderTier::SoftwareEmulated => RenderQuality { animate: false, translucent: false },
            RenderTier::ShmOnly => RenderQuality { animate: false, translucent: false },
        }
    }

    /// The `hatch_core::anim::BoardConfig` matching this tier's quality —
    /// zeroed durations rather than a different pattern, so
    /// `Board::retarget`'s own logic (skip animating when nothing
    /// changed) still applies unmodified; a "reduced" tier is just a
    /// hardware-tier config with every duration set to zero, not a
    /// separate code path.
    pub fn board_config(&self) -> hatch_core::anim::BoardConfig {
        let base = hatch_core::anim::BoardConfig::default();
        if self.quality().animate {
            base
        } else {
            hatch_core::anim::BoardConfig { enter_ms: 0.0, stagger_ms: 0.0, restack_ms: 0.0, restack_stagger_ms: 0.0, exit_ms: 0.0, ..base }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_hardware_gets_animation_and_translucency() {
        assert_eq!(RenderTier::Hardware.quality(), RenderQuality { animate: true, translucent: true });
        assert_eq!(RenderTier::SoftwareEmulated.quality(), RenderQuality { animate: false, translucent: false });
        assert_eq!(RenderTier::ShmOnly.quality(), RenderQuality { animate: false, translucent: false });
    }

    #[test]
    fn reduced_board_config_zeroes_every_duration_not_just_some() {
        let cfg = RenderTier::ShmOnly.board_config();
        assert_eq!(cfg.enter_ms, 0.0);
        assert_eq!(cfg.stagger_ms, 0.0);
        assert_eq!(cfg.restack_ms, 0.0);
        assert_eq!(cfg.restack_stagger_ms, 0.0);
        assert_eq!(cfg.exit_ms, 0.0);
    }

    #[test]
    fn hardware_board_config_is_just_the_ordinary_default() {
        let cfg = RenderTier::Hardware.board_config();
        let default = hatch_core::anim::BoardConfig::default();
        assert_eq!(cfg.enter_ms, default.enter_ms);
        assert_eq!(cfg.stagger_ms, default.stagger_ms);
    }
}
