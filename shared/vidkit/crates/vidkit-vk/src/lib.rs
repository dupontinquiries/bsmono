//! Vulkan presentation for vidkit.
//!
//! `harness` is a headless compute runner used to *execute* and verify
//! `shaders/convert.wgsl`; it runs on lavapipe, so the colour maths is checked
//! in CI without a GPU. `import` is the DMA-BUF import path, which still needs
//! real hardware: lavapipe does not expose `VK_EXT_image_drm_format_modifier`.

pub mod harness;
pub mod import;
