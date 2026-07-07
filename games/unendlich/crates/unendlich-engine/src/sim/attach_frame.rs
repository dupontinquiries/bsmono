//! attach_frame — ride-along transforms. A thing attached to a moving entity (a
//! deck on a boat, a mini-base strapped to a dragon's back) is authored in the
//! entity's LOCAL frame and must stay fixed there as the entity translates and
//! rotates through the world. This is the pure rigid-transform core: a `Frame`
//! (position + orthonormal rotation), `to_world`/`to_local` point maps, and
//! `compose` for nested attachments. One core powers both boats and dragon-backs.
//! Pure, std-only, deterministic.

/// A rigid frame: world position plus a 3×3 orthonormal rotation (rows).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub pos: [f32; 3],
    pub rot: [[f32; 3]; 3],
}

fn mat_vec(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}
fn transpose(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}
fn mat_mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut o = [[0.0; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            o[r][c] = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
        }
    }
    o
}
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

impl Frame {
    pub fn identity() -> Frame {
        Frame { pos: [0.0; 3], rot: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] }
    }

    /// A frame from a world position and an orientation given as yaw (about +Y)
    /// then pitch (about the body's +X). Orthonormal by construction.
    pub fn from_yaw_pitch(pos: [f32; 3], yaw: f32, pitch: f32) -> Frame {
        let (cy, sy) = (yaw.cos(), yaw.sin());
        let (cp, sp) = (pitch.cos(), pitch.sin());
        // R = Ry(yaw) * Rx(pitch)
        let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
        let rx = [[1.0, 0.0, 0.0], [0.0, cp, -sp], [0.0, sp, cp]];
        Frame { pos, rot: mat_mul(ry, rx) }
    }

    /// Map a point given in this frame's LOCAL space into world space.
    pub fn to_world(&self, local: [f32; 3]) -> [f32; 3] {
        add(self.pos, mat_vec(self.rot, local))
    }

    /// Map a WORLD point into this frame's local space (inverse of `to_world`).
    pub fn to_local(&self, world: [f32; 3]) -> [f32; 3] {
        mat_vec(transpose(self.rot), sub(world, self.pos))
    }

    /// Compose: `self` is the parent, `child` is expressed in the parent's local
    /// frame; the result is the child's frame in world space. Associative.
    pub fn compose(&self, child: &Frame) -> Frame {
        Frame { pos: self.to_world(child.pos), rot: mat_mul(self.rot, child.rot) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }
    fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
        let d = sub(a, b);
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    }

    #[test]
    fn identity_parent_world_equals_local() {
        let f = Frame::identity();
        let pt = [1.0, 2.0, 3.0];
        assert_eq!(f.to_world(pt), pt);
        assert_eq!(f.to_local(pt), pt);
    }

    #[test]
    fn round_trip_is_identity() {
        let f = Frame::from_yaw_pitch([5.0, -2.0, 7.0], 1.1, -0.6);
        let local = [0.3, 1.5, -0.8];
        let back = f.to_local(f.to_world(local));
        assert!(dist(back, local) < 1e-5, "round-trip {back:?} vs {local:?}");
    }

    #[test]
    fn a_point_rides_along_as_the_parent_moves() {
        // a deck point fixed at local [0,1,0]; move+rotate the parent → it tracks.
        let p0 = Frame::from_yaw_pitch([0.0, 0.0, 0.0], 0.0, 0.0);
        let p1 = Frame::from_yaw_pitch([10.0, 0.0, 0.0], std::f32::consts::FRAC_PI_2, 0.0);
        let local = [0.0, 1.0, 0.0];
        // local stays local; only the world position changes with the parent
        assert!(dist(p0.to_local(p0.to_world(local)), local) < 1e-5);
        assert!(dist(p1.to_local(p1.to_world(local)), local) < 1e-5);
        assert!(dist(p0.to_world(local), p1.to_world(local)) > 1.0, "world pos moved with the parent");
    }

    #[test]
    fn composition_is_associative() {
        let a = Frame::from_yaw_pitch([1.0, 0.0, 0.0], 0.4, 0.1);
        let b = Frame::from_yaw_pitch([0.0, 2.0, 0.0], -0.7, 0.3);
        let c = Frame::from_yaw_pitch([0.0, 0.0, 3.0], 1.2, -0.2);
        let left = a.compose(&b).compose(&c);
        let right = a.compose(&b.compose(&c));
        let pt = [0.5, -0.5, 0.5];
        assert!(dist(left.to_world(pt), right.to_world(pt)) < 1e-4, "(a∘b)∘c == a∘(b∘c)");
    }

    #[test]
    fn prop_round_trip_and_finite() {
        let mut s = 0xA77A_C4EDu32;
        for _ in 0..4000 {
            let f = Frame::from_yaw_pitch(
                [lcg(&mut s) * 100.0 - 50.0, lcg(&mut s) * 100.0 - 50.0, lcg(&mut s) * 100.0 - 50.0],
                lcg(&mut s) * 12.0 - 6.0,
                lcg(&mut s) * 12.0 - 6.0,
            );
            let local = [lcg(&mut s) * 10.0 - 5.0, lcg(&mut s) * 10.0 - 5.0, lcg(&mut s) * 10.0 - 5.0];
            let w = f.to_world(local);
            let back = f.to_local(w);
            assert!(w[0].is_finite() && back[0].is_finite(), "finite");
            assert!(dist(back, local) < 1e-3, "round-trip stable: {}", dist(back, local));
        }
    }
}
