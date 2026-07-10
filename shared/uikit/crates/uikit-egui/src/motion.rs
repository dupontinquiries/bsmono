//! motion — easing curves and eased animation helpers, so egui widgets get QML-quality motion by
//! default instead of egui's linear `animate_bool`.
//!
//! The QML views leaned heavily on tuned `Behavior`/`NumberAnimation` transitions — `OutCubic` fades,
//! `OutBack` spring pop-ins, 120–200ms `ColorAnimation` hover/countdown feedback. Ports that drop those
//! feel dead. This module brings the curves over as reusable primitives:
//!
//!   * [`ease`] — the easing functions (matching the `Easing.*` the QML used),
//!   * [`animate_eased`] — `animate_bool` with a curve,
//!   * [`animate_color`] — a smooth colour approach (`Behavior on color`),
//!   * [`interactive_fill`] — a button/cell fill that eases between rest/hover/press by default,
//!   * [`pop_scale`] — a spring pop-in scale for things that appear.

use egui::{Color32, Context, Id, Response};

/// Easing curves in `t ∈ [0,1]`. Names mirror QML `Easing.*`.
pub mod ease {
    /// No easing.
    pub fn linear(t: f32) -> f32 {
        t
    }
    /// Decelerating (fast → slow). QML `Easing.OutQuad`.
    pub fn out_quad(t: f32) -> f32 {
        1.0 - (1.0 - t) * (1.0 - t)
    }
    /// Accelerating (slow → fast). QML `Easing.InQuad`.
    pub fn in_quad(t: f32) -> f32 {
        t * t
    }
    /// Ease in then out. QML `Easing.InOutQuad`.
    pub fn in_out_quad(t: f32) -> f32 {
        if t < 0.5 {
            2.0 * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
        }
    }
    /// Strong deceleration — the workhorse for fades/expands. QML `Easing.OutCubic`.
    pub fn out_cubic(t: f32) -> f32 {
        1.0 - (1.0 - t).powi(3)
    }
    /// Accelerating cubic. QML `Easing.InCubic`.
    pub fn in_cubic(t: f32) -> f32 {
        t * t * t
    }
    /// Springy overshoot past 1 then settles — great for pop-ins. QML `Easing.OutBack`.
    pub fn out_back(t: f32) -> f32 {
        const C1: f32 = 1.70158;
        const C3: f32 = C1 + 1.0;
        1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2)
    }
    /// Gentle sine ease-out — soft, used for breathing/idle motion.
    pub fn out_sine(t: f32) -> f32 {
        (t * std::f32::consts::FRAC_PI_2).sin()
    }
}

/// `animate_bool` with an easing curve: drives a 0→1 value for `id` toward `target` over `duration`
/// seconds, then shapes it with `easing`. Returns the eased value in `[0,1]` (may briefly exceed 1 for
/// `out_back`).
pub fn animate_eased(ctx: &Context, id: Id, target: bool, duration: f32, easing: fn(f32) -> f32) -> f32 {
    let linear = ctx.animate_bool_with_time(id, target, duration);
    easing(linear)
}

/// A spring pop-in scale (`out_back`) for something appearing/disappearing — scale a rect by this
/// around its center. `0` when closed, overshoots ~1.05 then settles at `1.0`.
pub fn pop_scale(ctx: &Context, id: Id, open: bool, duration: f32) -> f32 {
    animate_eased(ctx, id, open, duration, ease::out_back)
}

/// Smoothly approach `target` colour over roughly `duration` seconds — the equivalent of QML's
/// `Behavior on color { ColorAnimation }`. Keeps the shown colour in egui memory keyed by `id`, so
/// hover/press/state changes glide instead of snapping.
pub fn animate_color(ctx: &Context, id: Id, target: Color32, duration: f32) -> Color32 {
    let tgt = [target.r() as f32, target.g() as f32, target.b() as f32, target.a() as f32];
    let cur: [f32; 4] = ctx.data_mut(|d| d.get_temp(id)).unwrap_or(tgt);
    let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
    // Exponential approach; the factor 4 makes it substantially settle within `duration`.
    let k = if duration <= 1e-4 { 1.0 } else { (1.0 - (-dt * 4.0 / duration).exp()).clamp(0.0, 1.0) };
    let next = [
        cur[0] + (tgt[0] - cur[0]) * k,
        cur[1] + (tgt[1] - cur[1]) * k,
        cur[2] + (tgt[2] - cur[2]) * k,
        cur[3] + (tgt[3] - cur[3]) * k,
    ];
    ctx.data_mut(|d| d.insert_temp(id, next));
    let dist: f32 = (0..4).map(|i| (next[i] - tgt[i]).abs()).sum();
    if dist > 0.6 {
        ctx.request_repaint(); // keep gliding until we arrive
    }
    Color32::from_rgba_unmultiplied(
        next[0].round() as u8,
        next[1].round() as u8,
        next[2].round() as u8,
        next[3].round() as u8,
    )
}

/// A fill colour that eases between rest/hover/press based on `response` — drop-in smoothness for any
/// custom-painted button or cell (matching the QML 120ms hover `ColorAnimation`). Use for painter-drawn
/// widgets where egui's own widget animation doesn't apply.
pub fn interactive_fill(ctx: &Context, response: &Response, rest: Color32, hover: Color32, press: Color32) -> Color32 {
    let target = if response.is_pointer_button_down_on() {
        press
    } else if response.hovered() {
        hover
    } else {
        rest
    };
    animate_color(ctx, response.id.with("motion_fill"), target, 0.12)
}

#[cfg(test)]
mod tests {
    use super::ease::*;

    fn endpoints(f: fn(f32) -> f32) {
        assert!((f(0.0)).abs() < 1e-4, "f(0)=0");
        assert!((f(1.0) - 1.0).abs() < 1e-4, "f(1)=1");
    }

    #[test]
    fn curves_hit_their_endpoints() {
        for f in [linear, out_quad, in_quad, in_out_quad, out_cubic, in_cubic, out_back, out_sine] {
            endpoints(f);
        }
    }

    #[test]
    fn out_curves_decelerate_and_in_curves_accelerate() {
        // out_* is above the diagonal (ahead early); in_* is below (behind early)
        assert!(out_quad(0.25) > 0.25);
        assert!(out_cubic(0.25) > 0.25);
        assert!(in_quad(0.25) < 0.25);
        assert!(in_cubic(0.25) < 0.25);
    }

    #[test]
    fn out_back_overshoots_past_one() {
        // the spring goes beyond 1.0 somewhere before settling
        let peak = (70..=95).map(|i| out_back(i as f32 / 100.0)).fold(0.0f32, f32::max);
        assert!(peak > 1.0, "out_back should overshoot, peak={peak}");
    }
}
