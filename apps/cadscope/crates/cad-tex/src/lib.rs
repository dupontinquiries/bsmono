//! cad-tex — the texturing system (plan §8).
//!
//! Two halves, joined by the engine's file convention:
//!
//!   * **materials** — procedural, seamless, seeded. Ported from
//!     `games/blackstar/tools/gen_texture.py`, which already generates the textures the games use,
//!     so a CADscope part can be surfaced with something that belongs in the same world rather
//!     than an unrelated look.
//!   * **mapping** — how a texture lands on an f-rep solid. There is no UV layout to inherit,
//!     because an SDF has no parameterisation at all, so the default is triplanar: project the
//!     texture down each axis and blend by the surface normal. It costs three lookups and needs no
//!     unwrap, which for a shape that changes whenever an op changes is the right trade.
//!
//! Output follows `<name>.png`, `norm_<name>.png`, `spec_<name>.png` — not a new convention, the
//! one already in `games/blackstar/engine_src/data/textures` and already understood by `cad-ref`'s
//! material join. A CADscope material is therefore usable by the games without a converter.

pub mod material;
pub mod noise;
pub mod png;
pub mod triplanar;

pub use material::{Material, Pattern, Rgb};
pub use noise::{tileable_fbm, Field, N};
pub use triplanar::{sample_triplanar, Sampler};

/// Milestone marker kept from the scaffold, now describing what exists rather than what is planned.
pub const MILESTONE: &str = "M4";

#[cfg(test)]
mod tests {
    #[test]
    fn the_crate_is_no_longer_a_stub() {
        assert_eq!(super::MILESTONE, "M4");
        // A material renders to a full-size image, which a stub could not do.
        let m = super::Material::new(super::Pattern::Plating, 1);
        assert_eq!(m.albedo().len(), super::N * super::N * 3);
    }
}

/// Write a material's three maps in the engine's convention: `<name>.png`, `norm_<name>.png`,
/// `spec_<name>.png`. Not a new layout — the one already in
/// `games/blackstar/engine_src/data/textures`, and the one `cad_ref::Index::textures_of_model`
/// joins on, so a CADscope material is usable by the games and findable by the corpus without a
/// converter or an import step.
pub fn export(m: &Material, dir: &std::path::Path, name: &str) -> std::io::Result<Vec<std::path::PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let maps: [(String, Vec<u8>); 3] = [
        (format!("{name}.png"), m.albedo()),
        (format!("norm_{name}.png"), m.normal(1.0)),
        (format!("spec_{name}.png"), m.specular()),
    ];
    let mut written = Vec::new();
    for (file, rgb) in maps {
        let bytes = png::encode_rgb(&rgb, N as u32, N as u32)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad image size"))?;
        let path = dir.join(file);
        std::fs::write(&path, bytes)?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod export_tests {
    use super::*;

    #[test]
    fn exports_the_three_map_convention() {
        let dir = std::env::temp_dir().join(format!("cadtex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let m = Material::new(Pattern::Hexmesh, 3);
        let files = export(&m, &dir, "testplate").expect("writes");
        assert_eq!(files.len(), 3);
        for (expect, path) in ["testplate.png", "norm_testplate.png", "spec_testplate.png"]
            .iter()
            .zip(files.iter())
        {
            assert!(path.ends_with(expect), "{path:?}");
            assert!(path.exists());
        }
        // The corpus must be able to read back what we wrote, including grouping the three maps
        // under one name — this is the join `texture from <model>` depends on.
        let head = std::fs::read(&files[1]).unwrap();
        assert_eq!(&head[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(u32::from_be_bytes([head[16], head[17], head[18], head[19]]), N as u32);
        std::fs::remove_dir_all(&dir).ok();
    }
}
