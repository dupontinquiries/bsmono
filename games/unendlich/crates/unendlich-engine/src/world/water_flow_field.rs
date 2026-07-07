//! `water_flow_field` — per-column horizontal flow vectors derived from the water
//! settle. Phase 1 of "flowing water as a shader": the dynamic water sim produces this
//! field each tick; a later phase uploads it so the surface shader can steer its waves
//! along the current (blended with wind) instead of only downwind.
//!
//! The field is a `side × side` grid of 2-D flow vectors covering the same world region
//! the sim simulates, anchored at `(origin_x, origin_z)` (world coords of column 0,0).
//! A vector points the way water is MOVING; its length is how much moved (a speed proxy).
//!
//! The producer side is pure and tested here; the only sim coupling is `accumulate`,
//! which folds in one settle step's directional out-flux array
//! `send[i] = [+x, -x, +z, -z]` (the exact deltas the Jacobi settle already computes).

/// A `side × side` grid of horizontal flow vectors over a world region.
#[derive(Clone, Debug)]
pub struct WaterFlowField {
    pub side: usize,
    pub origin_x: i32,
    pub origin_z: i32,
    /// Flow X / Z per column, row-major `idx = x + z*side`.
    pub vx: Vec<f32>,
    pub vz: Vec<f32>,
}

impl WaterFlowField {
    pub fn new(side: usize, origin_x: i32, origin_z: i32) -> Self {
        Self {
            side,
            origin_x,
            origin_z,
            vx: vec![0.0; side * side],
            vz: vec![0.0; side * side],
        }
    }

    /// Re-anchor to a new world region and zero the vectors (call at the start of a tick).
    pub fn reset(&mut self, origin_x: i32, origin_z: i32) {
        self.origin_x = origin_x;
        self.origin_z = origin_z;
        for v in &mut self.vx {
            *v = 0.0;
        }
        for v in &mut self.vz {
            *v = 0.0;
        }
    }

    #[inline]
    pub fn idx(&self, x: usize, z: usize) -> usize {
        x + z * self.side
    }

    #[inline]
    pub fn at(&self, x: usize, z: usize) -> (f32, f32) {
        let i = self.idx(x, z);
        (self.vx[i], self.vz[i])
    }

    /// Fold one settle step's directional out-flux into the flow vectors.
    /// `send[i] = [+x, -x, +z, -z]`; net horizontal flow is `(+x - -x, +z - -z)`.
    /// Tolerant of a shorter/longer `send` than the grid (uses the overlap).
    pub fn accumulate(&mut self, send: &[[f32; 4]]) {
        let n = self.vx.len().min(send.len());
        for i in 0..n {
            self.vx[i] += send[i][0] - send[i][1];
            self.vz[i] += send[i][2] - send[i][3];
        }
    }

    /// Convert a column's accumulated flow to a normalized direction + a speed scalar in
    /// `[0, 1]` (saturating at `speed_full`). Calm columns return a zero direction so the
    /// consumer can fall back to wind. `speed_full` is the per-tick flow magnitude treated
    /// as "fully flowing".
    pub fn dir_speed(&self, x: usize, z: usize, speed_full: f32) -> (f32, f32, f32) {
        let (fx, fz) = self.at(x, z);
        let mag = (fx * fx + fz * fz).sqrt();
        if mag <= 1e-6 || speed_full <= 0.0 {
            return (0.0, 0.0, 0.0);
        }
        let speed = (mag / speed_full).clamp(0.0, 1.0);
        (fx / mag, fz / mag, speed)
    }

    /// Dominant flow over the whole field (mean vector): `(dir_x, dir_z, mean_speed)`.
    /// Handy for a quick debug readout of "which way is the water going near me".
    pub fn dominant(&self, speed_full: f32) -> (f32, f32, f32) {
        let mut sx = 0.0f32;
        let mut sz = 0.0f32;
        for i in 0..self.vx.len() {
            sx += self.vx[i];
            sz += self.vz[i];
        }
        let mag = (sx * sx + sz * sz).sqrt();
        if mag <= 1e-6 || speed_full <= 0.0 {
            return (0.0, 0.0, 0.0);
        }
        let speed = (mag / (self.vx.len() as f32 * speed_full)).clamp(0.0, 1.0);
        (sx / mag, sz / mag, speed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send_grid(n: usize) -> Vec<[f32; 4]> {
        vec![[0.0; 4]; n]
    }

    #[test]
    fn new_is_zero_and_sized() {
        let f = WaterFlowField::new(4, 100, -50);
        assert_eq!(f.side, 4);
        assert_eq!(f.vx.len(), 16);
        assert_eq!(f.origin_x, 100);
        assert_eq!(f.origin_z, -50);
        assert!(f.vx.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn accumulate_plus_x_gives_positive_vx() {
        let mut f = WaterFlowField::new(2, 0, 0);
        let mut send = send_grid(4);
        send[0] = [0.7, 0.0, 0.0, 0.0]; // column 0 pushes +x
        f.accumulate(&send);
        assert!((f.vx[0] - 0.7).abs() < 1e-6);
        assert!(f.vz[0].abs() < 1e-6);
    }

    #[test]
    fn opposing_transfers_cancel() {
        let mut f = WaterFlowField::new(2, 0, 0);
        let mut send = send_grid(4);
        send[1] = [0.5, 0.5, 0.0, 0.0]; // equal +x and -x
        f.accumulate(&send);
        assert!(f.vx[1].abs() < 1e-6);
    }

    #[test]
    fn accumulate_is_additive_over_steps() {
        let mut f = WaterFlowField::new(1, 0, 0);
        let mut a = send_grid(1);
        a[0] = [0.0, 0.0, 0.3, 0.0];
        f.accumulate(&a);
        f.accumulate(&a);
        assert!((f.vz[0] - 0.6).abs() < 1e-6); // two +z steps
    }

    #[test]
    fn reset_rezeros_and_reanchors() {
        let mut f = WaterFlowField::new(2, 0, 0);
        let mut send = send_grid(4);
        send[0] = [1.0, 0.0, 0.0, 0.0];
        f.accumulate(&send);
        f.reset(10, 20);
        assert_eq!(f.origin_x, 10);
        assert_eq!(f.origin_z, 20);
        assert!(f.vx.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn dir_speed_normalizes_and_saturates() {
        let mut f = WaterFlowField::new(1, 0, 0);
        // strong +x flow, beyond speed_full -> saturates to 1.0, dir = (1,0)
        f.vx[0] = 4.0;
        let (dx, dz, s) = f.dir_speed(0, 0, 2.0);
        assert!((dx - 1.0).abs() < 1e-6);
        assert!(dz.abs() < 1e-6);
        assert!((s - 1.0).abs() < 1e-6);
    }

    #[test]
    fn dir_speed_calm_is_zero() {
        let f = WaterFlowField::new(1, 0, 0);
        let (dx, dz, s) = f.dir_speed(0, 0, 2.0);
        assert_eq!((dx, dz, s), (0.0, 0.0, 0.0));
    }

    #[test]
    fn dominant_points_along_net_flow() {
        let mut f = WaterFlowField::new(2, 0, 0);
        for i in 0..4 {
            f.vx[i] = 1.0; // everything flows +x
        }
        let (dx, dz, s) = f.dominant(1.0);
        assert!((dx - 1.0).abs() < 1e-6);
        assert!(dz.abs() < 1e-6);
        assert!(s > 0.0);
    }
}
