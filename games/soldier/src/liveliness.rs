//! liveliness.rs — a **cinematic-liveliness** scoring harness. Where `mapeval`/`sightline` score static
//! geometry and `scenic` scores the still image, this scores *motion over time* as the player would feel it:
//! how alive a map is, and — crucially — whether that life is paced to keep a player on edge rather than
//! either dead-still or a constant churn the eye tunes out.
//!
//! The thesis (from the design brief): the ideal is a tense, mostly-still baseline punctuated by periodic
//! twitch-inducing shifts — a gust of sand, a tremor raining dust, a curtain stirring — so that *motion pricks
//! peripheral vision* at intervals and keeps the player scanning. Too little motion feels like a dead
//! backdrop; too much (everything always moving) becomes visual noise that hides real threats and exhausts
//! attention. So we reward a **low, uneasy baseline with distinct spikes**, and penalise both flatlines and
//! constant saturation.
//!
//! ## How it measures
//! It runs a real `Game` for a window of frames from a fixed vantage and, each frame, counts the moving
//! elements *within the camera frustum*:
//!   - live **particles** (sand drift, dust, embers, smoke, splashes) — the dominant twitch source,
//!   - **tumble** debris chunks (falling masonry),
//!   - **sway** vertices (foliage, hung cloth/wires bowing in the wind) weighted by how much they're moving,
//!   - **water/lava** surfaces (always-animated) — a low, constant ambient motion,
//!   - **fires** and **dynamic lights** — flickering point sources.
//! These fold into a per-frame "activity" scalar. From the resulting time series we compute the liveliness
//! score from four terms: baseline calm, spike presence, spike rhythm, and an anti-saturation penalty.

use crate::camera::FpsCam;
use crate::game::Game;
use crate::level::PRESETS;
use crate::loadout::Loadout;
use crate::math::Vec3;

/// The outcome of a liveliness analysis.
#[derive(Clone, Debug)]
pub struct LivelinessReport {
    /// Mean per-frame activity (raw moving-element count in frustum, normalised).
    pub mean_activity: f32,
    /// The calm baseline: the low-percentile activity level (how still the map is *between* events).
    pub baseline: f32,
    /// Peak activity (the strongest twitch spike).
    pub peak: f32,
    /// How spiky the series is: the ratio of peak-to-baseline variation (higher = more distinct events
    /// against a calm ground).
    pub spikiness: f32,
    /// Number of distinct activity spikes detected over the window (events that prick the eye).
    pub events: usize,
    /// The headline 0..1 cinematic-liveliness score: rewards a calm-but-not-dead baseline, the presence of
    /// distinct spikes, a healthy event rhythm, and penalises constant saturation.
    pub score: f32,
}

/// Sample per-frame activity within the camera frustum of a running game.
fn frustum_activity(game: &Game, cam: &FpsCam) -> f32 {
    // Frustum test: an element counts if it's in front of the camera and within a generous FOV cone and range.
    let fwd = cam.forward();
    let in_view = |p: Vec3| -> bool {
        let rel = p - cam.pos;
        let d = rel.dot(fwd);
        if d <= 0.3 || d > 60.0 {
            return false;
        }
        // cone half-angle ~ 45° horizontal-ish; cheap dot-based cone
        let cos = d / rel.len().max(1e-3);
        cos > 0.62
    };

    let mut act = 0.0f32;
    // particles — the dominant twitch source
    for p in game.particles_iter() {
        if in_view(p) {
            act += 1.0;
        }
    }
    // tumble debris — heavy, attention-grabbing
    act += game.tumble_positions().iter().filter(|&&p| in_view(p)).count() as f32 * 2.0;
    // fires — flickering, always drawing the eye
    for f in game.fire_positions() {
        if in_view(f) {
            act += 1.5;
        }
    }
    // dynamic lights (flares, blasts) already correlate with the above; count lightly
    act += game.dynamic_lights().iter().filter(|l| in_view(l.pos)).count() as f32 * 0.5;
    // water/lava surfaces: a low, constant ambient motion when any is in view
    if game.level.water.iter().any(|w| in_view(Vec3::new(w.cx, w.y, w.cz))) {
        act += 1.0;
    }
    // sway foliage/dangling: sampled cheaply as a small constant when swaying geometry is near & in view
    // (the wind is always bowing them a little); we approximate presence rather than per-vertex motion.
    act += game.sway_sample(cam.pos, &in_view) * 0.5;

    // normalise into a friendlier ~0..1+ range
    act / 12.0
}

/// Analyse a running game over `frames` from a fixed vantage, returning the liveliness profile.
pub fn analyze_game(game: &mut Game, cam: &FpsCam, frames: usize) -> LivelinessReport {
    let dt = 1.0 / 60.0;
    let mut series: Vec<f32> = Vec::with_capacity(frames);
    let input = crate::player::Input::default();
    for _ in 0..frames {
        game.step(&input, dt);
        series.push(frustum_activity(game, cam));
    }
    score_series(&series)
}

/// Score a raw activity time-series into a LivelinessReport.
fn score_series(series: &[f32]) -> LivelinessReport {
    if series.is_empty() {
        return LivelinessReport { mean_activity: 0.0, baseline: 0.0, peak: 0.0, spikiness: 0.0, events: 0, score: 0.0 };
    }
    let n = series.len();
    let mean = series.iter().sum::<f32>() / n as f32;
    let mut sorted = series.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let baseline = sorted[(n as f32 * 0.25) as usize]; // 25th-percentile "between events" level
    let peak = *sorted.last().unwrap();
    let spikiness = if baseline > 1e-3 { (peak - baseline) / baseline } else { peak * 10.0 };

    // Count distinct events: contiguous runs where activity rises well above baseline, then falls back.
    let thresh = baseline + (peak - baseline) * 0.45;
    let mut events = 0usize;
    let mut above = false;
    for &v in series {
        if v >= thresh && !above {
            events += 1;
            above = true;
        } else if v < thresh * 0.8 {
            above = false;
        }
    }

    // ── the four scoring terms ──────────────────────────────────────────────────────────────────────────
    // 1) Baseline calm: reward a low-but-nonzero baseline. Dead-still (0) scores low; a gentle uneasy hum is
    //    ideal; a high baseline (constant churn) scores low. Peak reward around baseline ≈ 0.10.
    let calm = {
        let b = baseline;
        (1.0 - ((b - 0.10).abs() / 0.28)).clamp(0.0, 1.0)
    };
    // 2) Spike presence: there SHOULD be moments where motion jumps out. Reward real peaks above the calm.
    let spike_term = (spikiness / 3.0).clamp(0.0, 1.0);
    // 3) Rhythm: a healthy cadence of events over the window — enough to keep the player scanning, not so many
    //    it's constant. Over ~10s (600 frames) ideal is ~a handful of distinct events.
    let per_10s = events as f32 / (n as f32 / 600.0).max(0.25);
    let rhythm = (1.0 - (per_10s - 3.5).abs() / 3.5).clamp(0.0, 1.0);
    // 4) Anti-saturation: penalise a map whose mean activity is high (the eye tunes out constant motion).
    let anti_sat = (1.0 - (mean - 0.18).max(0.0) / 0.5).clamp(0.0, 1.0);

    let score = (calm * 0.28 + spike_term * 0.30 + rhythm * 0.24 + anti_sat * 0.18).clamp(0.0, 1.0);

    LivelinessReport { mean_activity: mean, baseline, peak, spikiness, events, score }
}

/// Convenience: analyse a preset map by name over a default window from its centre, spinning slowly so the
/// vantage sees the whole map's activity.
pub fn analyze_preset(name: &str, seed: u64, frames: usize) -> Option<LivelinessReport> {
    let i = PRESETS.iter().position(|p| p.name == name)?;
    let mut game = Game::new_preset(seed, Some(i), Loadout::default());
    let lvl = &game.level;
    let c = Vec3::new(lvl.w as f32 * lvl.ts() * 0.5, lvl.floor_y + 2.0, lvl.h as f32 * lvl.ts() * 0.5);
    let cam = FpsCam { pos: c, yaw: 0.7, pitch: -0.08 };
    Some(analyze_game(&mut game, &cam, frames))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_series_rewards_spikes_over_flatline() {
        // A flat, dead series should score low; a calm baseline with periodic spikes should score higher.
        let flat: Vec<f32> = vec![0.0; 600];
        let dead = score_series(&flat);
        let mut spiky: Vec<f32> = vec![0.10; 600];
        for k in 0..600 {
            if k % 150 < 20 {
                spiky[k] = 0.8; // periodic bursts
            }
        }
        let alive = score_series(&spiky);
        assert!(alive.score > dead.score, "a spiky, uneasy series should beat a dead flatline ({:.2} vs {:.2})", alive.score, dead.score);
        assert!(alive.events >= 3, "should detect the periodic spikes, got {}", alive.events);
    }

    #[test]
    fn constant_churn_scores_below_paced_events() {
        // A map that's always saturated with motion should score below one with paced spikes (the eye tunes
        // out constant churn).
        let churn: Vec<f32> = vec![0.9; 600];
        let paced: Vec<f32> = (0..600).map(|k| if k % 150 < 20 { 0.8 } else { 0.10 }).collect();
        let cs = score_series(&churn);
        let ps = score_series(&paced);
        assert!(ps.score > cs.score, "paced events should beat constant churn ({:.2} vs {:.2})", ps.score, cs.score);
    }

    #[test]
    fn every_preset_analyses_without_panic() {
        for name in ["Desert Town", "Malecon Breakwater", "Redwood Canyon"] {
            if let Some(r) = analyze_preset(name, 7, 120) {
                assert!(r.score.is_finite() && r.score >= 0.0 && r.score <= 1.0);
            }
        }
    }
}
