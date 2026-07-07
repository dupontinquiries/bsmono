//! `sim::wing_buffet` — the downwash a dragon's wing-beat throws at nearby
//! things.
//!
//! A hard flap from a big wing shoves the air (and whatever is in it) down and
//! outward. [`wing_buffet_force`](crate::sim::wing_buffet_force) gives the
//! knockback *magnitude* as a linear falloff to `max_range`; this adds the two
//! things it leaves out — how much power a given wing/flap produces, and which
//! way the gust points — to turn it into a force vector per target. It composes
//! that core (and reads naturally from `dragon_flight`'s wing area + flap
//! effort); it doesn't reimplement the falloff. Pure and deterministic.

use crate::sim::wing_buffet_force;

/// Peak buffet power from wing area and flap effort `flap01` (`0` = gliding ⇒ no
/// buffet). Bigger wings and harder flaps push harder; `gain` scales to taste.
pub fn buffet_power(wing_area: f32, flap01: f32, gain: f32) -> f32 {
    wing_area.max(0.0) * flap01.clamp(0.0, 1.0) * gain.max(0.0)
}

/// Knockback force on a target from a dragon flapping at `origin` with peak
/// `power` over `max_range`. Magnitude falls off linearly with distance
/// ([`wing_buffet_force::force`]); direction is radially away from the dragon in
/// the horizontal plane with a downward bias `down_bias` (downwash pushes out
/// and down). The returned vector's length equals the falloff magnitude. Zero
/// beyond range or at zero power; finite everywhere (a target directly above or
/// below falls back to a fixed horizontal axis).
pub fn buffet_force(
    origin: [f32; 3],
    target: [f32; 3],
    power: f32,
    max_range: f32,
    down_bias: f32,
) -> [f32; 3] {
    let dx = target[0] - origin[0];
    let dy = target[1] - origin[1];
    let dz = target[2] - origin[2];
    let dist = (dx * dx + dy * dy + dz * dz).max(0.0).sqrt();
    let mag = wing_buffet_force::force(dist, max_range, power);
    if mag <= 0.0 {
        return [0.0, 0.0, 0.0];
    }
    let horiz = (dx * dx + dz * dz).max(0.0).sqrt();
    let (ux, uz) = if horiz > 1e-4 { (dx / horiz, dz / horiz) } else { (1.0, 0.0) };
    let down = down_bias.max(0.0);
    let norm = (1.0 + down * down).sqrt(); // (ux,uz) is unit; add the down leg
    [mag * ux / norm, -mag * down / norm, mag * uz / norm]
}

/// Buffet forces on a set of targets (same index order) — convenience over
/// [`buffet_force`] for an AoE wing-beat.
pub fn buffet_targets(
    origin: [f32; 3],
    targets: &[[f32; 3]],
    power: f32,
    max_range: f32,
    down_bias: f32,
) -> Vec<[f32; 3]> {
    targets
        .iter()
        .map(|&t| buffet_force(origin, t, power, max_range, down_bias))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn len(v: [f32; 3]) -> f32 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }

    #[test]
    fn no_flap_no_power() {
        assert_eq!(buffet_power(2.0, 0.0, 10.0), 0.0, "gliding ⇒ no buffet");
        assert!(buffet_power(2.0, 1.0, 10.0) > buffet_power(1.0, 1.0, 10.0), "bigger wing pushes harder");
        assert!(buffet_power(2.0, 1.0, 10.0) > buffet_power(2.0, 0.4, 10.0), "harder flap pushes harder");
    }

    #[test]
    fn pushes_away_and_down() {
        // target to the +x of the dragon, slightly above ground
        let f = buffet_force([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], 50.0, 8.0, 0.5);
        assert!(f[0] > 0.0, "shoved away from the dragon (+x)");
        assert!(f[1] < 0.0, "shoved downward (downwash)");
        assert!(f[2].abs() < 1e-5);
    }

    #[test]
    fn magnitude_matches_falloff_and_fades_with_distance() {
        let near = buffet_force([0.0; 3], [2.0, 0.0, 0.0], 50.0, 8.0, 0.5);
        let far = buffet_force([0.0; 3], [6.0, 0.0, 0.0], 50.0, 8.0, 0.5);
        assert!(len(near) > len(far), "farther ⇒ weaker");
        // length equals the scalar falloff
        let expect = wing_buffet_force::force(2.0, 8.0, 50.0);
        assert!((len(near) - expect).abs() < 1e-3, "vector length == falloff magnitude");
    }

    #[test]
    fn zero_beyond_range() {
        let f = buffet_force([0.0; 3], [20.0, 0.0, 0.0], 50.0, 8.0, 0.5);
        assert_eq!(f, [0.0, 0.0, 0.0], "outside max_range ⇒ no buffet");
    }

    #[test]
    fn directly_above_is_finite_and_downward() {
        let f = buffet_force([0.0, 0.0, 0.0], [0.0, 3.0, 0.0], 50.0, 8.0, 0.5);
        assert!(f.iter().all(|c| c.is_finite()), "no NaN when horizontal direction is degenerate");
        assert!(f[1] < 0.0, "still pushed down");
    }

    #[test]
    fn batch_matches_singles_and_is_deterministic() {
        let origin = [1.0, 2.0, 3.0];
        let targets = [[4.0, 2.0, 3.0], [1.0, 2.0, 7.0], [30.0, 0.0, 0.0]];
        let batch = buffet_targets(origin, &targets, 40.0, 8.0, 0.6);
        for (i, &t) in targets.iter().enumerate() {
            assert_eq!(batch[i], buffet_force(origin, t, 40.0, 8.0, 0.6));
        }
        assert_eq!(batch, buffet_targets(origin, &targets, 40.0, 8.0, 0.6));
    }
}
