//! beatgrid — ruler ticks that follow the tempo, not the clock.
//!
//! The C12-14f rendering task: with a per-song [`TempoMap`], the ruler's
//! fixed seconds-spacing stops being the musical truth — bar 9 is not "at
//! 16 seconds" once the bridge slows down. This module computes WHERE the
//! beats and bars actually fall, as data, so the ruler draws detents that
//! stretch and compress with the map. Non-uniform spacing is not an effect
//! layered on top; it falls straight out of [`TempoMap::beat_to_tick`],
//! which converts each beat within its own segment's tempo.
//!
//! Pure and separately tested, for the same reason `snap` and
//! `responsive_nav::choose` are: the density ladder (beats → bars → every
//! Nth bar) is exactly the kind of logic that looks right at one zoom
//! level and collides labels at another, and a paint function cannot be
//! asserted against.
//!
//! Per-frame cost: one `beat_to_tick` per visible tick, each O(segments).
//! `TempoMap`'s docs name a cursor variant as the optimization if this
//! ever matters; with the tick count capped below and segments in the
//! single digits for real songs, it does not yet.

use studio_core::{TempoMap, Ticks};

/// How strongly a tick draws. Bars carry the label; beats are the texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight { Bar, Beat }

#[derive(Debug, Clone, PartialEq)]
pub struct GridTick {
    pub at: Ticks,
    pub weight: Weight,
    /// 1-based bar number, present exactly on `Bar` ticks — the ruler's
    /// label, numbered the way every DAW and every chart numbers bars.
    pub bar: Option<u32>,
}

/// Hard cap on emitted ticks. The density ladder keeps real cases far
/// below this; the cap exists so a hostile zoom/range combination is a
/// short vector, never a hang.
const MAX_TICKS: usize = 512;

/// The ticks to draw for the visible span `[from, to]`.
///
/// Density adapts by the TIGHTEST spacing in the window (the fastest
/// tempo — spacing is smallest there), stepping beats → bars → every
/// 2nd/4th/8th… bar until nothing lands closer than `min_px`. Adapting by
/// the average instead would let the fast section's ticks smear together,
/// which is precisely the section where the grid matters.
pub fn beat_grid(tempo: &TempoMap, from: Ticks, to: Ticks,
                 px_per_sec: f32, beats_per_bar: u32, min_px: f32) -> Vec<GridTick> {
    let mut out = Vec::new();
    if to <= from || px_per_sec <= 0.0 { return out; }
    let beats_per_bar = beats_per_bar.max(1) as i64;

    // Fastest tempo with any presence in the window: the segment `from`
    // falls in, plus every change point inside the span.
    let mut max_bpm = tempo.bpm_at(from);
    for (t, bpm) in tempo.points() {
        if *t > from && *t <= to { max_bpm = max_bpm.max(*bpm); }
    }
    let min_beat_px = px_per_sec as f64 * 60.0 / max_bpm as f64;

    // The ladder: beats if they fit; else bars, strided by powers of two
    // until the tightest bar-to-bar gap clears the floor.
    let (step_beats, beats_visible) = if min_beat_px >= min_px as f64 {
        (1i64, true)
    } else {
        let mut stride = 1i64;
        while (min_beat_px * (beats_per_bar * stride) as f64) < min_px as f64
            && stride < 1 << 20 {
            stride *= 2;
        }
        (beats_per_bar * stride, false)
    };

    let b0 = tempo.tick_to_beat(from).floor() as i64;
    let b1 = tempo.tick_to_beat(to).ceil() as i64;
    // First emitted beat: aligned DOWN to the step so bars stay bars — a
    // stride of 8 must emit bars 1, 3, 5…, not whatever beat the left
    // edge happened to scroll to.
    let mut b = b0.div_euclid(step_beats) * step_beats;
    while b <= b1 && out.len() < MAX_TICKS {
        if b >= 0 {
            let at = tempo.beat_to_tick(b as f64);
            if at >= from && at <= to {
                let on_bar = b % beats_per_bar == 0;
                out.push(GridTick {
                    at,
                    weight: if on_bar { Weight::Bar } else { Weight::Beat },
                    bar: if on_bar { Some((b / beats_per_bar) as u32 + 1) } else { None },
                });
            }
        }
        b += if beats_visible { 1 } else { step_beats };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = studio_core::TICKS_PER_SECOND;
    fn sec(s: f64) -> Ticks { Ticks((s * S as f64).round() as i64) }

    /// Constant 120 BPM, 4/4: beats every half second, bars every two —
    /// the uniform case must reproduce exactly what a fixed grid drew,
    /// with 1-based bar numbers.
    #[test]
    fn constant_tempo_is_the_uniform_grid() {
        let t = TempoMap::constant(120.0);
        let g = beat_grid(&t, Ticks::ZERO, sec(4.0), 100.0, 4, 12.0);
        let at: Vec<i64> = g.iter().map(|x| x.at.0).collect();
        let want: Vec<i64> = (0..=8).map(|i| S / 2 * i).collect();
        assert_eq!(at, want, "nine ticks, half a second apart");
        assert_eq!(g[0].weight, Weight::Bar);
        assert_eq!(g[0].bar, Some(1), "bars are 1-based, like every chart");
        assert_eq!(g[4].bar, Some(2), "beat 4 opens bar 2 at 2.0s");
        assert!(matches!(g[1].weight, Weight::Beat));
        assert_eq!(g[1].bar, None);
    }

    /// The whole point of the task: after a tempo change the spacing
    /// changes, computed through the map, not scaled globally. 120→150 at
    /// 4.0s (beat 8): the next beat is 0.4s later, not 0.5.
    #[test]
    fn spacing_follows_the_detents_across_a_change() {
        let t = TempoMap::new(vec![(Ticks::ZERO, 120.0), (sec(4.0), 150.0)]);
        let g = beat_grid(&t, Ticks::ZERO, sec(6.0), 200.0, 4, 12.0);
        let beat = |n: usize| g.iter().find(|x| x.at == t.beat_to_tick(n as f64))
            .unwrap_or_else(|| panic!("beat {n} missing"));
        assert_eq!(beat(7).at, sec(3.5), "before the change: 120 spacing");
        assert_eq!(beat(8).at, sec(4.0), "a detent lands ON the change");
        assert_eq!(beat(9).at, sec(4.4), "after: 150 BPM spacing = 0.4s");
        // Monotonic and duplicate-free, or the ruler double-draws.
        for w in g.windows(2) { assert!(w[1].at > w[0].at); }
    }

    /// Zoomed far out, beats would smear — the ladder steps to bars, then
    /// strided bars, and nothing lands closer than the floor. Density
    /// keys off the FASTEST section: the slow half's comfortable spacing
    /// must not license collisions in the fast half.
    #[test]
    fn density_backs_off_by_the_fastest_section_and_respects_the_floor() {
        let t = TempoMap::new(vec![(Ticks::ZERO, 60.0), (sec(30.0), 240.0)]);
        let px_per_sec = 4.0; // beats at 240 are 1px apart: far under floor
        let g = beat_grid(&t, Ticks::ZERO, sec(60.0), px_per_sec, 4, 12.0);
        assert!(!g.is_empty());
        assert!(g.iter().all(|x| x.weight == Weight::Bar),
                "too tight for beats: bars only");
        for w in g.windows(2) {
            let px = (w[1].at.0 - w[0].at.0) as f64 / S as f64 * px_per_sec as f64;
            assert!(px >= 11.9, "ticks {:?}->{:?} are {px:.1}px apart", w[0].bar, w[1].bar);
        }
        // Strided bars stay bar-NUMBER aligned (1, 1+n, 1+2n…), so the
        // labels count musically instead of drifting with the scroll.
        let bars: Vec<u32> = g.iter().filter_map(|x| x.bar).collect();
        let stride = bars[1] - bars[0];
        assert!(bars.windows(2).all(|w| w[1] - w[0] == stride));
        assert_eq!((bars[0] - 1) % stride, 0, "stride anchored at bar 1");
    }

    /// A hostile range cannot explode: capped, and a backwards or empty
    /// span is simply empty.
    #[test]
    fn hostile_inputs_stay_small_and_sane() {
        let t = TempoMap::constant(120.0);
        let g = beat_grid(&t, Ticks::ZERO, sec(100_000.0), 10_000.0, 4, 0.0);
        assert!(g.len() <= MAX_TICKS);
        assert!(beat_grid(&t, sec(5.0), sec(1.0), 100.0, 4, 12.0).is_empty());
        assert!(beat_grid(&t, sec(1.0), sec(5.0), 0.0, 4, 12.0).is_empty());
    }
}
