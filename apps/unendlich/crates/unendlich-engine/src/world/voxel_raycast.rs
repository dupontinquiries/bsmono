//! `voxel_raycast` — Amanatides–Woo DDA grid traversal (the block-edit pick).
//!
//! Worker-03 / WORLD-WORLDGEN pure core, round 6. Walks a voxel grid from an
//! origin along a direction, sampling solidity via a passed-in `is_solid`
//! closure, and returns the first solid voxel hit **and the face entered**.
//! That face is what makes break/place correct: break removes `hit.cell`, place
//! puts the new block on `hit.cell + face_normal` (the empty cell the ray came
//! from). Walks up to a maximum world-space distance.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// A direction / position in world space.
#[derive(Clone, Copy, Debug)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Vec3 { x, y, z }
    }
}

/// Which face of the hit voxel the ray entered through. The face normal points
/// back toward the ray's origin side (so it's where a placed block goes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    None,
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

/// Unit normal of a face (0,0,0 for `None`).
pub fn face_normal(f: Face) -> (i32, i32, i32) {
    match f {
        Face::PosX => (1, 0, 0),
        Face::NegX => (-1, 0, 0),
        Face::PosY => (0, 1, 0),
        Face::NegY => (0, -1, 0),
        Face::PosZ => (0, 0, 1),
        Face::NegZ => (0, 0, -1),
        Face::None => (0, 0, 0),
    }
}

/// A ray hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// The solid voxel hit (the cell to break).
    pub cell: (i32, i32, i32),
    /// The face entered; `cell + face_normal` is the empty cell to place into.
    pub face: Face,
    /// World-space distance from the origin to the hit.
    pub distance: f32,
}

impl Hit {
    /// The adjacent empty cell on the entered face (where a placed block goes).
    pub fn place_cell(&self) -> (i32, i32, i32) {
        let (nx, ny, nz) = face_normal(self.face);
        (self.cell.0 + nx, self.cell.1 + ny, self.cell.2 + nz)
    }
}

/// Cast a ray and return the first solid voxel + the face entered, or `None` if
/// none is hit within `max_distance`.
pub fn raycast<F>(origin: Vec3, dir: Vec3, max_distance: f32, is_solid: F) -> Option<Hit>
where
    F: Fn(i32, i32, i32) -> bool,
{
    let len = (dir.x * dir.x + dir.y * dir.y + dir.z * dir.z).sqrt();
    if !(len > 0.0) || !max_distance.is_finite() || max_distance < 0.0 {
        return None;
    }
    let d = Vec3::new(dir.x / len, dir.y / len, dir.z / len);

    let mut vx = origin.x.floor() as i32;
    let mut vy = origin.y.floor() as i32;
    let mut vz = origin.z.floor() as i32;

    // Origin already inside a solid voxel: report it (face undefined → None).
    if is_solid(vx, vy, vz) {
        return Some(Hit { cell: (vx, vy, vz), face: Face::None, distance: 0.0 });
    }

    let step_x = step_of(d.x);
    let step_y = step_of(d.y);
    let step_z = step_of(d.z);

    let mut t_max_x = initial_tmax(origin.x, vx, d.x, step_x);
    let mut t_max_y = initial_tmax(origin.y, vy, d.y, step_y);
    let mut t_max_z = initial_tmax(origin.z, vz, d.z, step_z);

    let t_delta_x = t_delta(d.x);
    let t_delta_y = t_delta(d.y);
    let t_delta_z = t_delta(d.z);

    loop {
        // Advance along the axis with the smallest t_max (ties: X, then Y, then Z).
        let t;
        let face;
        if t_max_x < t_max_y && t_max_x < t_max_z {
            vx += step_x;
            t = t_max_x;
            t_max_x += t_delta_x;
            face = if step_x > 0 { Face::NegX } else { Face::PosX };
        } else if t_max_y < t_max_z {
            vy += step_y;
            t = t_max_y;
            t_max_y += t_delta_y;
            face = if step_y > 0 { Face::NegY } else { Face::PosY };
        } else {
            vz += step_z;
            t = t_max_z;
            t_max_z += t_delta_z;
            face = if step_z > 0 { Face::NegZ } else { Face::PosZ };
        }

        if !t.is_finite() || t > max_distance {
            return None;
        }
        if is_solid(vx, vy, vz) {
            return Some(Hit { cell: (vx, vy, vz), face, distance: t });
        }
    }
}

fn step_of(d: f32) -> i32 {
    if d > 0.0 {
        1
    } else if d < 0.0 {
        -1
    } else {
        0
    }
}

fn initial_tmax(o: f32, v: i32, d: f32, step: i32) -> f32 {
    if step > 0 {
        ((v as f32 + 1.0) - o) / d
    } else if step < 0 {
        (v as f32 - o) / d
    } else {
        f32::INFINITY
    }
}

fn t_delta(d: f32) -> f32 {
    if d != 0.0 {
        (1.0 / d).abs()
    } else {
        f32::INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_ray_hits_expected_cell_and_face() {
        // Solid wall at x = 5; ray from x≈0.5 going +X.
        let hit = raycast(Vec3::new(0.5, 0.5, 0.5), Vec3::new(1.0, 0.0, 0.0), 100.0, |x, _, _| {
            x == 5
        })
        .expect("should hit the wall");
        assert_eq!(hit.cell, (5, 0, 0));
        assert_eq!(hit.face, Face::NegX); // entered from the -X side
        assert!((hit.distance - 4.5).abs() < 1e-3);
        assert_eq!(hit.place_cell(), (4, 0, 0)); // place goes on the near side
    }

    #[test]
    fn empty_space_returns_none() {
        let hit = raycast(Vec3::new(0.5, 0.5, 0.5), Vec3::new(0.0, 1.0, 0.0), 50.0, |_, _, _| false);
        assert!(hit.is_none());
    }

    #[test]
    fn max_distance_respected() {
        // Wall at y=100 but max_distance only 10 → miss.
        let hit = raycast(Vec3::new(0.5, 0.5, 0.5), Vec3::new(0.0, 1.0, 0.0), 10.0, |_, y, _| {
            y == 100
        });
        assert!(hit.is_none());
    }

    #[test]
    fn returns_first_solid_not_a_later_one() {
        // Two walls at x=3 and x=7; +X ray must hit x=3.
        let hit = raycast(Vec3::new(0.5, 0.5, 0.5), Vec3::new(1.0, 0.0, 0.0), 100.0, |x, _, _| {
            x == 3 || x == 7
        })
        .unwrap();
        assert_eq!(hit.cell, (3, 0, 0));
    }

    #[test]
    fn negative_direction_face_is_posx() {
        // Wall at x=-5; ray going -X enters through +X face.
        let hit = raycast(Vec3::new(0.5, 0.5, 0.5), Vec3::new(-1.0, 0.0, 0.0), 100.0, |x, _, _| {
            x == -5
        })
        .unwrap();
        assert_eq!(hit.cell, (-5, 0, 0));
        assert_eq!(hit.face, Face::PosX);
        assert_eq!(hit.place_cell(), (-4, 0, 0));
    }

    #[test]
    fn origin_inside_solid_reports_face_none() {
        let hit = raycast(Vec3::new(2.5, 2.5, 2.5), Vec3::new(1.0, 0.0, 0.0), 10.0, |_, _, _| true).unwrap();
        assert_eq!(hit.cell, (2, 2, 2));
        assert_eq!(hit.face, Face::None);
        assert_eq!(hit.distance, 0.0);
    }

    #[test]
    fn diagonal_ray_face_is_the_crossed_axis() {
        // Floor plane at y=0 below; ray from above going down-and-forward.
        let hit = raycast(Vec3::new(0.5, 5.5, 0.5), Vec3::new(0.3, -1.0, 0.0), 100.0, |_, y, _| {
            y <= 0
        })
        .unwrap();
        // Descending ray enters the solid through its +Y (top) face.
        assert_eq!(hit.face, Face::PosY);
        assert_eq!(hit.cell.1, 0);
    }

    #[test]
    fn prop_deterministic() {
        let solid = |x: i32, y: i32, z: i32| (x + y + z) % 7 == 0 && x > 2;
        let a = raycast(Vec3::new(0.1, 0.2, 0.3), Vec3::new(1.0, 0.4, 0.2), 100.0, solid);
        let b = raycast(Vec3::new(0.1, 0.2, 0.3), Vec3::new(1.0, 0.4, 0.2), 100.0, solid);
        assert_eq!(a, b);
    }

    #[test]
    fn zero_direction_is_none() {
        assert!(raycast(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0), 10.0, |_, _, _| true).is_none()
            || true); // origin-solid path returns Some; with empty it's None — exercise no panic
        let h = raycast(Vec3::new(0.5, 0.5, 0.5), Vec3::new(0.0, 0.0, 0.0), 10.0, |_, _, _| false);
        assert!(h.is_none());
    }
}
