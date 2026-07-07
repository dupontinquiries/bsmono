//! Impact response — a generic mass/energy model for collisions and footfalls.
//!
//! Any event with a mass, a speed, a footprint and a location (a dragon's foot
//! striking ground, a body slam, a rockfall) carries kinetic energy. From that
//! one event this core estimates three things the rest of the engine wants:
//!   * **camera shake** — energy delivered, attenuated by distance and spread
//!     over the footprint (a broad, soft footfall shakes less than a sharp one);
//!   * **loudness** — a perceptual (log) function of energy;
//!   * **erosion** — energy *density* at the contact patch (energy per footprint
//!     area), i.e. how much the ground is gouged.
//!
//! Pure and deterministic, no panic. Distances/areas are guarded against zero.

/// One impact event.
#[derive(Clone, Copy, Debug)]
pub struct Impact {
    /// Mass behind the impact (kg-ish, engine units).
    pub mass: f32,
    /// Closing speed at contact (units/s).
    pub speed: f32,
    /// World position of the contact.
    pub pos: [f32; 3],
    /// Contact patch radius (a foot pad, a body). Larger spreads the energy.
    pub footprint: f32,
}

/// What an impact produces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Response {
    /// Camera shake amplitude `>= 0` (feed a trauma/shake system).
    pub shake: f32,
    /// Loudness `0..~1+` (perceptual).
    pub loudness: f32,
    /// Erosion / gouge estimate `>= 0` (energy density at the patch).
    pub erosion: f32,
}

/// Kinetic energy `0.5·m·v²` (clamped non-negative).
pub fn energy(mass: f32, speed: f32) -> f32 {
    0.5 * mass.max(0.0) * speed * speed
}

/// Footprint area `π·r²`, floored to avoid divide-by-zero.
fn area(footprint: f32) -> f32 {
    (core::f32::consts::PI * footprint * footprint).max(1e-3)
}

/// Evaluate an impact relative to the camera.
pub fn respond(im: &Impact, camera_pos: [f32; 3]) -> Response {
    let e = energy(im.mass, im.speed);

    let dx = im.pos[0] - camera_pos[0];
    let dy = im.pos[1] - camera_pos[1];
    let dz = im.pos[2] - camera_pos[2];
    let dist2 = dx * dx + dy * dy + dz * dz;

    // A sharp (small) footprint concentrates the jolt; a broad one softens it.
    let concentration = 1.0 / (1.0 + im.footprint.max(0.0));
    // Inverse-square-ish falloff with distance.
    let shake = (e * concentration) / (1.0 + dist2);

    // Perceptual loudness: log of energy, distance-attenuated.
    let loudness = (1.0 + e).ln() / (1.0 + 0.05 * dist2).sqrt();

    // Erosion: energy per unit contact area at the site (distance-independent).
    let erosion = e / area(im.footprint);

    Response { shake, loudness, erosion }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imp(mass: f32, speed: f32, footprint: f32, pos: [f32; 3]) -> Impact {
        Impact { mass, speed, pos, footprint }
    }

    #[test]
    fn energy_formula() {
        assert!((energy(2.0, 3.0) - 9.0).abs() < 1e-4); // 0.5*2*9
        assert_eq!(energy(-5.0, 4.0), 0.0); // negative mass clamped
    }

    #[test]
    fn closer_camera_shakes_more() {
        let i = imp(40.0, 5.0, 0.5, [0.0, 0.0, 0.0]);
        let near = respond(&i, [1.0, 0.0, 0.0]);
        let far = respond(&i, [50.0, 0.0, 0.0]);
        assert!(near.shake > far.shake);
    }

    #[test]
    fn broader_footprint_softens_shake_and_erosion() {
        let sharp = respond(&imp(40.0, 5.0, 0.2, [0.0; 3]), [2.0, 0.0, 0.0]);
        let broad = respond(&imp(40.0, 5.0, 3.0, [0.0; 3]), [2.0, 0.0, 0.0]);
        assert!(sharp.shake > broad.shake, "sharp footfall jolts more");
        assert!(sharp.erosion > broad.erosion, "concentrated energy gouges more");
    }

    #[test]
    fn heavier_faster_does_more_of_everything() {
        let light = respond(&imp(10.0, 2.0, 0.5, [0.0; 3]), [3.0, 0.0, 0.0]);
        let heavy = respond(&imp(80.0, 8.0, 0.5, [0.0; 3]), [3.0, 0.0, 0.0]);
        assert!(heavy.shake > light.shake);
        assert!(heavy.loudness > light.loudness);
        assert!(heavy.erosion > light.erosion);
    }

    #[test]
    fn zero_energy_is_silent_and_still() {
        let r = respond(&imp(0.0, 0.0, 0.5, [0.0; 3]), [1.0, 0.0, 0.0]);
        assert_eq!(r, Response { shake: 0.0, loudness: 0.0, erosion: 0.0 });
    }

    #[test]
    fn deterministic_and_no_panic_on_zero_footprint() {
        let r = respond(&imp(40.0, 5.0, 0.0, [0.0; 3]), [0.0, 0.0, 0.0]);
        assert!(r.shake.is_finite() && r.erosion.is_finite());
    }
}
