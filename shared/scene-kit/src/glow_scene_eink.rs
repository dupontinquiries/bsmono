//! `glow_scene` for e-ink builds (cfg `eink`, set by build.rs when gitmind
//! builds for a tablet with EINK_SHIM=1).
//!
//! Same public API as the GPU renderer, so every app using scene-kit
//! compiles unchanged. There is no GPU on the tablet: `callback` returns a
//! paint callback that eink-kit's CPU rasterizer skips, so the 3D panel
//! draws empty and the 2D UI around it keeps working. A flat-shaded CPU
//! fallback (projecting `gpuscene::pack_sorted` triangles into an egui mesh)
//! would slot in here without touching any app.
pub use crate::scene_types::{PageTex, Uniforms};

/// What the GPU version's callback carries; here it is inert.
pub struct NoGpu;

/// Same signature as the GPU renderer's `callback`.
pub fn callback(rect: egui::Rect, u: Uniforms, verts: Vec<f32>) -> egui::PaintCallback {
    let _ = (u, verts);
    egui::PaintCallback { rect, callback: std::sync::Arc::new(NoGpu) }
}
