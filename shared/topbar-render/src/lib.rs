//! topbar-render — the rendering seam `topbar-core::runtime` deliberately
//! left undefined. Not wired into `Runtime` yet (no `BarPainter` trait
//! there) — this crate stands alone until this actually gets exercised and
//! any resulting design pressure on that seam is known.

pub mod surface;

pub use surface::{RenderError, VulkanSurface};
