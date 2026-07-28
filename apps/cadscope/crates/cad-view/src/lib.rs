//! cad-view — the viewport: skydribble's renderer transplanted into an `egui_wgpu` paint callback
//! (the "next rung" logscope's gpuscene.rs names). Pure pieces (`meshgen`, `camera`) carry the
//! tests; `renderer` owns the GPU objects and the CallbackTrait. The WGSL is validated offline by
//! naga in dev-tests, per the skydribble convention.

pub mod camera;
pub mod meshgen;
pub mod renderer;

/// The one lit pass. Included at compile time so the naga test below pins exactly what ships.
pub const SHADER: &str = include_str!("shader.wgsl");

#[cfg(test)]
mod tests {
    #[test]
    fn wgsl_parses_and_validates() {
        let module = naga::front::wgsl::parse_str(super::SHADER).expect("WGSL parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        );
        validator.validate(&module).expect("WGSL validate");
    }
}
