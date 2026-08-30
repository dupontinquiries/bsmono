//! triplanar.rs — putting a texture on a shape that has no UVs.
//!
//! An SDF has no parameterisation. There is no unwrap to inherit, and any unwrap computed for one
//! version of a part is invalidated the moment an op changes it — which, in a modeller where the
//! whole point is that history replays, is constantly. Triplanar projection sidesteps the problem:
//! sample the texture three times, projected down each axis, and blend by the surface normal. It
//! costs three lookups instead of one and needs no layout at all.
//!
//! The seam behaviour is the reason to prefer it here specifically. A triplanar blend is smooth
//! wherever the normal turns smoothly, so a filleted corner — which is most of what CADscope's
//! Blend operator produces — crosses between projections without a visible line.

use cad_kernel::Sdf;
use glam::{Vec2, Vec3};

use crate::material::Material;
use crate::noise::N;

/// Anything that can be asked for a colour at a texture coordinate.
pub trait Sampler {
    /// `uv` in texture space; values outside [0,1) wrap.
    fn sample(&self, uv: Vec2) -> [u8; 3];
}

// NOTE: there is deliberately no `impl Sampler for Material`. Material stores a scalar field and
// builds its albedo on demand, so sampling one per texel would regenerate the whole image for every
// lookup — an easy accident that turns a bake into minutes. `Baked::of(&material)` flattens once,
// and the type system then makes the fast path the only path.

/// A pre-flattened image, so a bake does not regenerate the material per texel.
pub struct Baked {
    pub rgb: Vec<u8>,
    pub size: usize,
}

impl Baked {
    pub fn of(m: &Material) -> Baked {
        Baked { rgb: m.albedo(), size: N }
    }
}

impl Sampler for Baked {
    fn sample(&self, uv: Vec2) -> [u8; 3] {
        let x = ((uv.x.rem_euclid(1.0)) * self.size as f32) as usize % self.size;
        let y = ((uv.y.rem_euclid(1.0)) * self.size as f32) as usize % self.size;
        let i = (y * self.size + x) * 3;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }
}

/// Blend weights for the three projections, from a surface normal. `sharpness` above 1 narrows the
/// transition; 4 is the usual starting point and what the viewport should default to.
pub fn blend_weights(n: Vec3, sharpness: f32) -> Vec3 {
    let mut w = n.abs().powf(sharpness.max(1.0));
    let sum = w.x + w.y + w.z;
    if sum > 1e-6 {
        w /= sum;
    } else {
        w = Vec3::new(0.0, 1.0, 0.0);
    }
    w
}

/// Sample a texture at a world point on a surface with normal `n`. `scale` is texture repeats per
/// model unit — at 1/50 a 256px texture covers 50mm.
pub fn sample_triplanar<S: Sampler>(tex: &S, p: Vec3, n: Vec3, scale: f32) -> [u8; 3] {
    let w = blend_weights(n, 4.0);
    let s = p * scale;
    let x = tex.sample(Vec2::new(s.z, s.y));
    let y = tex.sample(Vec2::new(s.x, s.z));
    let z = tex.sample(Vec2::new(s.x, s.y));
    let mix = |i: usize| {
        (x[i] as f32 * w.x + y[i] as f32 * w.y + z[i] as f32 * w.z).round().clamp(0.0, 255.0) as u8
    };
    [mix(0), mix(1), mix(2)]
}

/// Sample at a point on an SDF's surface, taking the normal from the field itself. This is the
/// call the viewport makes: no UVs, no unwrap, no stored parameterisation — the shape answers for
/// its own orientation.
pub fn sample_on<S: Sampler>(tex: &S, sdf: &Sdf, p: Vec3, scale: f32) -> [u8; 3] {
    sample_triplanar(tex, p, sdf.normal(p), scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{Material, Pattern};

    #[test]
    fn weights_sum_to_one_and_follow_the_normal() {
        for n in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(1.0, 1.0, 0.0).normalize()] {
            let w = blend_weights(n, 4.0);
            assert!((w.x + w.y + w.z - 1.0).abs() < 1e-4, "{w:?}");
        }
        // A face pointing up is textured almost entirely by the Y projection.
        let w = blend_weights(Vec3::Y, 4.0);
        assert!(w.y > 0.99, "{w:?}");
        // A degenerate normal must not produce NaNs.
        let w = blend_weights(Vec3::ZERO, 4.0);
        assert!(w.is_finite() && (w.x + w.y + w.z - 1.0).abs() < 1e-4);
    }

    #[test]
    fn sampling_is_continuous_across_a_turning_normal() {
        // The property that justifies triplanar over a planar projection: as the normal sweeps
        // from +Y to +X, the colour must not jump.
        let tex = Baked::of(&Material::new(Pattern::Rock, 4));
        let p = Vec3::new(3.0, 2.0, 1.0);
        let mut prev = sample_triplanar(&tex, p, Vec3::Y, 0.02);
        let mut worst = 0i32;
        for i in 1..=64 {
            let t = i as f32 / 64.0 * std::f32::consts::FRAC_PI_2;
            let n = Vec3::new(t.sin(), t.cos(), 0.0);
            let c = sample_triplanar(&tex, p, n, 0.02);
            let step: i32 = (0..3).map(|k| (c[k] as i32 - prev[k] as i32).abs()).max().unwrap();
            worst = worst.max(step);
            prev = c;
        }
        assert!(worst < 40, "colour jumped by {worst} as the normal turned");
    }

    #[test]
    fn a_shape_textures_itself_without_uvs() {
        let sdf = cad_kernel::sdf::sphere(10.0);
        let tex = Baked::of(&Material::new(Pattern::Plating, 2));
        // Points scattered over the sphere, each sampled through the field's own normal.
        //
        // Note the axis points are avoided deliberately: at exactly +X the projection weights are
        // (1,0,0) and the sampled coordinate is (z,y) = (0,0), so +X and -X land on the same texel
        // of a texture that has no idea which side of the sphere it is on. Six axis points on a
        // symmetric pattern therefore prove nothing, which is what the first version of this test
        // did. Off-axis points exercise the actual blend.
        let mut seen = std::collections::HashSet::new();
        for i in 0..12 {
            let a = i as f32 / 12.0 * std::f32::consts::TAU;
            let d = Vec3::new(a.cos(), (a * 1.7).sin() * 0.6, a.sin()).normalize();
            seen.insert(sample_on(&tex, &sdf, d * 10.0, 0.05));
        }
        assert!(
            seen.len() > 4,
            "only {} distinct colours over twelve points: the mapping is not varying with position",
            seen.len()
        );
    }

    #[test]
    fn scale_controls_repeats() {
        let tex = Baked::of(&Material::new(Pattern::Hexmesh, 1));
        // At a coarse scale two nearby points land in the same texel; at a fine scale they do not.
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(0.4, 0.0, 0.0);
        let coarse = (sample_triplanar(&tex, a, Vec3::Z, 0.001), sample_triplanar(&tex, b, Vec3::Z, 0.001));
        assert_eq!(coarse.0, coarse.1, "at 0.001 repeats/unit these should share a texel");
        let fine = (sample_triplanar(&tex, a, Vec3::Z, 4.0), sample_triplanar(&tex, b, Vec3::Z, 4.0));
        assert_ne!(fine.0, fine.1, "at 4 repeats/unit they should not");
    }
}
