//! gpu-kit — take the repeated GPU-setup boilerplate out of the apps.
//!
//! Seven apps use wgpu (av1tuner, av1tuner-m1, beatspace, fao-codec, logscope, snapclone,
//! unendlich) and each re-derives the same setup *decisions*: which adapter (power
//! preference / backend), which surface format (sRGB or not), which present mode
//! (vsync vs low-latency), how many MSAA samples. They also span three wgpu versions
//! (0.19 / 27 / 29), so a kit pinned to one version can't serve them all.
//!
//! **v0.1 is therefore the version-agnostic policy core**: gpu-kit's own small enums for
//! those decisions plus pure selection helpers that operate on the *app's* capability
//! lists (whatever wgpu version produced them). The actual `instance → adapter → device →
//! queue` construction lives behind the `wgpu29` feature so it can be pinned and verified
//! on-device without forcing a version on everyone.

/// Adapter power preference — maps 1:1 onto every wgpu version's enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    HighPerformance,
    LowPower,
}

/// What the app wants from the present mode, expressed by intent rather than by a
/// version-specific enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Present {
    /// Prefer no tearing (Fifo/vsync); the safe default.
    Vsync,
    /// Prefer lowest latency (Mailbox, else Immediate, else Fifo).
    LowLatency,
    /// Prefer highest throughput (Immediate, else Mailbox, else Fifo).
    Fast,
}

/// The setup policy an app hands to gpu-kit instead of re-deriving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub power: Power,
    pub prefer_srgb: bool,
    pub present: Present,
    /// MSAA sample count request (clamped to a supported count by [`pick_sample_count`]).
    pub msaa: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Policy { power: Power::HighPerformance, prefer_srgb: true, present: Present::Vsync, msaa: 1 }
    }
}

/// A version-neutral name for present modes, so selection logic is testable without wgpu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentMode {
    Fifo,
    FifoRelaxed,
    Mailbox,
    Immediate,
}

impl Policy {
    /// Choose a present mode from those the surface actually supports, honouring intent and
    /// always falling back to `Fifo` (guaranteed present in every backend).
    pub fn pick_present_mode(&self, supported: &[PresentMode]) -> PresentMode {
        let order: &[PresentMode] = match self.present {
            Present::Vsync => &[PresentMode::Fifo, PresentMode::FifoRelaxed, PresentMode::Mailbox, PresentMode::Immediate],
            Present::LowLatency => &[PresentMode::Mailbox, PresentMode::Immediate, PresentMode::FifoRelaxed, PresentMode::Fifo],
            Present::Fast => &[PresentMode::Immediate, PresentMode::Mailbox, PresentMode::FifoRelaxed, PresentMode::Fifo],
        };
        order
            .iter()
            .copied()
            .find(|m| supported.contains(m))
            .unwrap_or(PresentMode::Fifo)
    }
}

/// Clamp a requested MSAA count to the largest supported value that is ≤ the request
/// (1 is always valid). `supported` is the app's flag set (e.g. from wgpu's sample-count
/// flags), any order.
pub fn pick_sample_count(requested: u32, supported: &[u32]) -> u32 {
    let mut best = 1;
    for &s in supported {
        if s <= requested && s > best {
            best = s;
        }
    }
    best.max(1)
}

/// Pick a surface format from `available` by a tiny scoring rule: sRGB-ness matching the
/// policy is the primary key, then earlier-in-list (surfaces list preferred formats first).
/// `is_srgb` classifies a format without gpu-kit depending on wgpu's enum. Returns the
/// index into `available`, or 0 if empty-safe (`available` must be non-empty; `None` if it
/// is empty).
pub fn pick_format_index<F>(available: &[F], prefer_srgb: bool, is_srgb: impl Fn(&F) -> bool) -> Option<usize> {
    if available.is_empty() {
        return None;
    }
    let mut best_idx = 0usize;
    let mut best_score = i32::MIN;
    for (i, f) in available.iter().enumerate() {
        let srgb = is_srgb(f);
        // matching sRGB preference is worth more than position; earlier position breaks ties
        let score = (if srgb == prefer_srgb { 1000 } else { 0 }) - i as i32;
        if score > best_score {
            best_score = score;
            best_idx = i;
        }
    }
    Some(best_idx)
}

// ── the actual wgpu context builder, pinned + opt-in (verify on-device) ──
#[cfg(feature = "wgpu29")]
pub mod ctx {
    use super::{Policy, Power};

    /// The four things every wgpu app carries around.
    pub struct Gpu {
        pub instance: wgpu::Instance,
        pub adapter: wgpu::Adapter,
        pub device: wgpu::Device,
        pub queue: wgpu::Queue,
    }

    impl Gpu {
        /// Headless context (compute / offscreen): the instance→adapter→device→queue dance
        /// each app copies, driven by [`Policy::power`].
        pub async fn headless(policy: &Policy) -> Result<Gpu, String> {
            let instance = wgpu::Instance::default();
            let power = match policy.power {
                Power::HighPerformance => wgpu::PowerPreference::HighPerformance,
                Power::LowPower => wgpu::PowerPreference::LowPower,
            };
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: power,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                })
                .await
                .ok_or_else(|| "no suitable GPU adapter".to_string())?;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default(), None)
                .await
                .map_err(|e| format!("request_device: {e}"))?;
            Ok(Gpu { instance, adapter, device, queue })
        }

        /// Create a shader module from WGSL source (the other line every app copies).
        pub fn shader(&self, label: &str, wgsl: &str) -> wgpu::ShaderModule {
            self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(wgsl.into()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_mode_honours_intent_and_falls_back_to_fifo() {
        let low = Policy { present: Present::LowLatency, ..Policy::default() };
        assert_eq!(low.pick_present_mode(&[PresentMode::Fifo, PresentMode::Mailbox]), PresentMode::Mailbox);
        // only Fifo available → always safe
        assert_eq!(low.pick_present_mode(&[PresentMode::Fifo]), PresentMode::Fifo);
        let vsync = Policy::default();
        assert_eq!(vsync.pick_present_mode(&[PresentMode::Immediate, PresentMode::Fifo]), PresentMode::Fifo);
    }

    #[test]
    fn msaa_clamps_to_supported() {
        assert_eq!(pick_sample_count(4, &[1, 4]), 4);
        assert_eq!(pick_sample_count(8, &[1, 4]), 4); // no 8 → best ≤ 8
        assert_eq!(pick_sample_count(2, &[1, 4]), 1); // no 2 → down to 1
        assert_eq!(pick_sample_count(1, &[]), 1);
    }

    #[test]
    fn format_pick_prefers_matching_srgb_then_position() {
        // pretend formats: (name, is_srgb)
        let fmts = [("bgra8", false), ("bgra8_srgb", true), ("rgba8_srgb", true)];
        let i = pick_format_index(&fmts, true, |f| f.1).unwrap();
        assert_eq!(fmts[i].0, "bgra8_srgb", "first sRGB wins when sRGB preferred");
        let j = pick_format_index(&fmts, false, |f| f.1).unwrap();
        assert_eq!(fmts[j].0, "bgra8", "non-sRGB preferred picks the linear one");
        assert_eq!(pick_format_index::<(&str, bool)>(&[], true, |f| f.1), None);
    }
}
