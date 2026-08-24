// NOTE: this file is include!d at the root of each harness, so its header
// must be `//` and not `//!` — an inner doc comment is only legal at the top
// of a file, and after inlining it is not at the top of anything.
// The minimum of the crate that `model.rs` needs when it is pulled in
// verbatim by a harness. `model.rs` names three siblings — `meshgen`,
// `render::Gpu`, and `physics::Hover` — and only `meshgen` carries real
// logic the loader depends on. The other two are satisfied here so that the
// loader compiles and runs without a device, without a physics step, and
// without touching the game's 180 KB `main.rs`.
//
// Two more real siblings since that comment was written: `model.rs` now
// reads packed poses through `crate::botpack::pose_buffer` (the v3 pack
// migration) and `xform.rs` now carries `crate::compact::Params`/
// `displace` (the compaction feature). Neither needs stubbing — both are
// self-contained (`botpack.rs` uses only `std::path`; `compact.rs` only
// `glam::Vec3`, already a dependency) — so they're pulled in for real,
// the same way `meshgen` is, rather than faked. Discovered because
// `examples/stripcheck.rs` (added by the fleet-additions patch) failed
// to compile against this shim at all until these two were added: the
// shim's own doc comment had gone stale the same way the botpack test
// fixtures had, and nobody had run a harness since either landed.
// And four more since THAT comment was written, all arriving with the
// animation-layers work: `model.rs` now calls `crate::turret::find`,
// `crate::massfilter`, `crate::cockpit` and `crate::skeleton`. Same
// treatment and for the same reason — each is self-contained (`glam::Vec3`
// and nothing else), so they are pulled in for real rather than stubbed.
// Discovered the same way as last time: every harness failed to compile
// against this shim, `cannot find 'turret' in 'crate'`, because the
// harnesses are not part of the fast edit loop that produced them.
#[path = "../src/meshgen.rs"]
pub mod meshgen;
#[path = "../src/botpack.rs"]
pub mod botpack;
#[path = "../src/compact.rs"]
pub mod compact;
#[path = "../src/turret.rs"]
pub mod turret;
#[path = "../src/massfilter.rs"]
pub mod massfilter;
#[path = "../src/cockpit.rs"]
pub mod cockpit;
#[path = "../src/skeleton.rs"]
pub mod skeleton;
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
