//! scene-kit — logscope's physical UI as a shared kit.
//!
//! Lifted from `apps/logscope/crates/logscope-egui` (spatial, mesh3d, gpuscene,
//! glow_scene, texgen, textsurface — see each module's original header) when
//! aperture became the second consumer. logscope still carries its own copy;
//! it migrates here at the next scene co-change (TODO.md §scene-kit): the
//! only source-visible differences are `Mesh3.uvs` (new field, `None` keeps
//! old behavior), `perf` replacing `theme::{power_mode,detail}`, and the
//! 14-float vertex.
//!
//! What extraction ADDED (the two gaps `physical-ui-findings.md` names):
//! 1. **Per-vertex UVs** — vertices are now 14 floats; [`gpuscene::MAT_INSCRIPTION`]
//!    faces sample `u_page` by UV, so glyphs are authored in page space and
//!    stay put on the surface. This is the enabling change for words-on-faces.
//! 2. **`u_touch` is live** — `touch_response` was wired end to end except
//!    the one assignment; the fragment shader now drives emissive from it
//!    (all surfaces warm faintly; inscriptions warm more), and callers set
//!    `Uniforms.touch` with an age advanced by frame delta.
//! Plus [`inscribe`]: a CPU page rasterizer (font8x8, span-colored) producing
//! the RGBA that `u_page` uploads.

pub mod spatial;
pub mod mesh3d;
pub mod texgen;
pub mod gpuscene;
pub mod textsurface;
pub mod inscribe;
pub mod perf;
#[cfg(feature = "glow")]
pub mod scene_types;
/// The GPU scene renderer (glow). On e-ink builds (cfg `eink`) the same API
/// is provided without a GPU; see glow_scene_eink.rs.
#[cfg(all(feature = "glow", not(eink)))]
pub mod glow_scene;
#[cfg(all(feature = "glow", eink))]
#[path = "glow_scene_eink.rs"]
pub mod glow_scene;
