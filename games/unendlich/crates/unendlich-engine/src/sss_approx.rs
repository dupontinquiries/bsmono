//! sss_approx.rs — pure cheap subsurface-scatter term for translucent wings.
//!
//! Back-lit translucency: light behind a thin surface transmits through and
//! glows; thicker material blocks it. Pairs with `wing_opacity` (R14). Pure,
//! deterministic, panic-free (R1/R11/R21). Result in `[0,1]`.

/// Subsurface term for light alignment `n_dot_l` and `thickness`. Back-lighting
/// (`n·l < 0`) produces a glow that `exp(-thickness)` attenuates: thin ⇒
/// transmits, thick ⇒ blocked. In `[0,1]`.
pub fn sss(n_dot_l: f32, thickness: f32) -> f32 {
    let back = crate::util::mathx::saturate(-n_dot_l); // how back-lit, [0,1]
    let transmit = (-thickness.max(0.0)).exp(); // thin->1, thick->0
    crate::util::mathx::saturate(back * transmit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_lit_glows() {
        assert!(sss(-0.8, 0.2) > 0.0, "back-lit should glow");
    }

    #[test]
    fn front_lit_no_glow() {
        assert_eq!(sss(0.8, 0.2), 0.0, "front-lit should not transmit");
        assert_eq!(sss(0.0, 0.2), 0.0);
    }

    #[test]
    fn thick_blocks() {
        assert!(sss(-1.0, 10.0) < 1e-3, "thick should block");
    }

    #[test]
    fn thin_transmits() {
        assert!((sss(-1.0, 0.0) - 1.0).abs() < 1e-6, "fully back-lit + thin -> ~1");
    }

    #[test]
    fn monotone_in_thinness() {
        // for fixed back-lighting, more thickness => less transmission
        let mut prev = 2.0f32;
        let mut th = 0.0f32;
        while th <= 5.0 {
            let s = sss(-1.0, th);
            assert!(s <= prev + 1e-6, "sss grew with thickness at {}", th);
            assert!((0.0..=1.0).contains(&s));
            prev = s;
            th += 0.05;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(sss(-0.5, 0.7), sss(-0.5, 0.7));
    }
}
