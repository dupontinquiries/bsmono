//! Minimal math — Vec3 + column-major Mat4 (wgpu/WGSL convention, NDC z in [0,1]).
//!
//! Since the math-kit extraction, these are `shared/math-kit`'s types: the kit
//! adopted this file's `Vec3` core (`add`/`sub`/`scale`/`dot`/`cross`/`len`/
//! `norm`, incl. the 1e-6 norm guard), `Mat4` layout + `mul`, `perspective`,
//! and `look_at` **verbatim**, so this re-export is behavior-identical — the
//! crate's own test suite is the parity proof. The kit adds operator overloads
//! (`+`/`-`/`* f32`), `lerp`/`dist`/`len_sq`, and more constructors; use them
//! freely in new code.

pub use math_kit::mat::{look_at, perspective, Mat4};
pub use math_kit::vec::{v3, Vec3};
