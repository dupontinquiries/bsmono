//! pulse_svg — the "held nearby" animation, as actual SVG markup
//! regenerated each frame from elapsed time, not procedural egui shapes.
//!
//! Pure string generation: given elapsed milliseconds and a color, returns
//! an SVG document with expanding, fading concentric rings — a classic
//! "detecting" pulse. Rasterized via `resvg` by the caller, same pipeline
//! `hatch-egui::icons_img` already uses for its own icons, kept separate
//! from rendering so the actual ring math (radius growth, opacity falloff,
//! ring count/stagger) is checkable as plain strings/numbers without a
//! GPU or a window.

/// One ring's state at a point in its own lifecycle (0.0 = just born at
/// the center, 1.0 = fully expanded and faded to nothing).
struct RingState {
    radius: f32,
    opacity: f32,
}

fn ring_at(progress: f32, max_radius: f32) -> RingState {
    let p = progress.clamp(0.0, 1.0);
    RingState { radius: p * max_radius, opacity: (1.0 - p).max(0.0) }
}

/// Builds the pulse SVG for a given elapsed time. `period_ms` is how long
/// one ring takes to fully expand and fade; `ring_count` rings are staggered
/// evenly across that period so a new one starts as an earlier one is
/// still fading — the actual "pulsing" look, not one ring repeating with a
/// visible gap.
pub fn pulse_svg(elapsed_ms: u32, period_ms: u32, ring_count: u32, viewbox: f32, color_hex: &str) -> String {
    let max_radius = viewbox * 0.45;
    let center = viewbox / 2.0;
    let mut circles = String::new();
    for i in 0..ring_count.max(1) {
        let stagger = (period_ms * i) / ring_count.max(1);
        let local_elapsed = (elapsed_ms + stagger) % period_ms.max(1);
        let progress = local_elapsed as f32 / period_ms.max(1) as f32;
        let ring = ring_at(progress, max_radius);
        circles.push_str(&format!(
            r#"<circle cx="{center}" cy="{center}" r="{:.1}" fill="none" stroke="{color_hex}" stroke-width="3" opacity="{:.3}"/>"#,
            ring.radius, ring.opacity
        ));
    }
    // A small solid dot at the center, always fully opaque — an anchor
    // point so the animation still reads as "something is here" during
    // the instant between one ring fully fading and the next becoming
    // visible, not just empty space with rings passing through it.
    format!(
        r#"<svg viewBox="0 0 {viewbox} {viewbox}" xmlns="http://www.w3.org/2000/svg">{circles}<circle cx="{center}" cy="{center}" r="{:.1}" fill="{color_hex}"/></svg>"#,
        viewbox * 0.04
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_freshly_born_ring_starts_at_zero_radius_fully_opaque() {
        let r = ring_at(0.0, 100.0);
        assert_eq!(r.radius, 0.0);
        assert_eq!(r.opacity, 1.0);
    }

    #[test]
    fn a_fully_expanded_ring_reaches_max_radius_fully_transparent() {
        let r = ring_at(1.0, 100.0);
        assert_eq!(r.radius, 100.0);
        assert_eq!(r.opacity, 0.0);
    }

    #[test]
    fn progress_past_one_clamps_rather_than_overshooting() {
        let r = ring_at(1.5, 100.0);
        assert_eq!(r.radius, 100.0);
        assert_eq!(r.opacity, 0.0);
    }

    #[test]
    fn the_svg_is_well_formed_and_contains_the_requested_ring_count() {
        let svg = pulse_svg(0, 1000, 3, 200.0, "#FF0000");
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
        assert_eq!(svg.matches("<circle").count(), 4, "3 rings + 1 center dot");
    }

    #[test]
    fn rings_are_staggered_not_all_born_at_the_same_moment() {
        // At elapsed=0, with 2 rings staggered across a 1000ms period,
        // ring 0 is freshly born (radius ~0) and ring 1 is mid-cycle
        // (radius > 0) -- they must NOT show identical radii.
        let svg = pulse_svg(0, 1000, 2, 200.0, "#00FF00");
        let radii: Vec<f32> = svg
            .split("r=\"")
            .skip(1)
            .filter_map(|s| s.split('"').next())
            .filter_map(|s| s.parse::<f32>().ok())
            .collect();
        assert!(radii.len() >= 2);
        assert_ne!(radii[0], radii[1], "staggered rings must be at different points in their cycle");
    }

    #[test]
    fn zero_ring_count_still_produces_valid_svg_with_just_the_center_dot() {
        let svg = pulse_svg(500, 1000, 0, 200.0, "#0000FF");
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert_eq!(svg.matches("<circle").count(), 2, "ring_count.max(1) still draws one ring, plus the center dot");
    }
}
