//! The minimum of the crate that `model.rs` needs when it is pulled in
//! verbatim by a harness. `model.rs` names three siblings — `meshgen`,
//! `render::Gpu`, and `physics::Hover` — and only `meshgen` carries real
//! logic the loader depends on. The other two are satisfied here so that the
//! loader compiles and runs without a device, without a physics step, and
//! without touching the game's 180 KB `main.rs`.
#[path = "../src/meshgen.rs"]
pub mod meshgen;
#[path = "../src/model.rs"]
pub mod model;

pub mod render {
    pub struct Gpu;
    impl Gpu {
        pub fn add_texture(&mut self, _w: u32, _h: u32, _px: &image::RgbaImage) -> usize { 0 }
        pub fn add_mesh(&mut self, _m: &crate::meshgen::MeshData) -> usize { 0 }
    }
}

pub mod physics {
    /// Field-for-field as declared in `src/physics.rs`; `model.rs` only ever
    /// stores `None` here, but the type has to exist for `Model` to compile.
    #[derive(Clone, Copy, Debug)]
    pub struct Hover {
        pub ride: f32,
        pub stiff: f32,
        pub hold: f32,
        pub field_r: f32,
        pub field_k: f32,
    }
    impl Default for Hover {
        fn default() -> Hover {
            Hover { ride: 0.55, stiff: 42.0, hold: 0.18, field_r: 3.2, field_k: 26.0 }
        }
    }
}
