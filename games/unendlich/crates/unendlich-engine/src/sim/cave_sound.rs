//! Cave acoustics (`cave_biomes.docx`, each biome's Acoustic Signature):
//! how long a cave rings, and the limestone cave's "single iconic sound", the
//! water drip: "each drop produces a clean high-frequency impulse that the
//! cave reverb extends into a long crystalline ring." Drips are also the
//! first thing a CO₂ basin takes away (its "dead silence"), so they are what
//! makes that silence noticeable.
//!
//! Pure: synthesis and an echo plan for `sim::sound_render`, which the game
//! renders on the effects thread. What is not here: the docx's footstep
//! echoes and flood rumble, and any per-block ray tracing of the cave (the
//! ring comes from the zone's RT60, not from the chamber's real shape).
use crate::sim::land_echo::{EchoPlan, Path, Tail, SPEED_OF_SOUND};
use crate::sim::sound_stereo;
use crate::world::underground::Zone;

/// The docx RT60 range for a zone, in seconds (`None` for soil).
pub fn rt60_range(zone: Zone) -> Option<(f32, f32)> {
    match zone {
        Zone::Soil => None,
        Zone::Limestone => Some((2.0, 8.0)),
        Zone::Crystal => Some((4.0, 10.0)),
        Zone::Abyssal => Some((8.0, 20.0)),
        Zone::Geothermal => Some((3.0, 15.0)),
    }
}

/// RT60 at a place, `t01` picking where in the zone's range (the game passes
/// a slow map noise so cave systems differ).
pub fn rt60_s(zone: Zone, t01: f32) -> Option<f32> {
    let (lo, hi) = rt60_range(zone)?;
    Some(lo + (hi - lo) * t01.clamp(0.0, 1.0))
}

/// Mean drips per minute a listener hears in a zone: limestone caves are
/// wet (seepage, pools), crystal chambers and the abyss drier, the
/// geothermal band too hot for standing drips.
pub fn drips_per_minute(zone: Zone) -> f32 {
    match zone {
        Zone::Limestone => 10.0,
        Zone::Abyssal => 4.0,
        Zone::Crystal => 2.0,
        Zone::Soil | Zone::Geothermal => 0.0,
    }
}

fn hash(x: u32) -> u32 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ 0x85EB_CA6B;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

fn unit(x: u32) -> f32 {
    (hash(x) >> 8) as f32 / (1u32 << 24) as f32
}

/// One drop landing in water, mono at `rate` Hz: a short sine "plink" whose
/// pitch rises (a bubble closing), with a click at the impact, decaying in
/// tens of milliseconds. `seed` varies the pitch (about 1.1-2.6 kHz) and
/// length so no two drips sound alike. Peak is about 0.8.
pub fn drip(rate: u32, seed: u32) -> Vec<f32> {
    let rate_f = rate.max(8000) as f32;
    let f0 = 1100.0 + 1500.0 * unit(seed);
    let rise = 1.4 + 0.4 * unit(seed ^ 0xA5A5);
    let tau = 0.018 + 0.02 * unit(seed ^ 0x5A5A);
    let glide = 0.035;
    let len = (rate_f * (tau * 7.0)) as usize;
    let mut out = Vec::with_capacity(len);
    let mut phase = 0.0f32;
    for i in 0..len {
        let t = i as f32 / rate_f;
        let f = f0 * (1.0 + (rise - 1.0) * (t / glide).min(1.0));
        phase += std::f32::consts::TAU * f / rate_f;
        let env = (-t / tau).exp() * (1.0 - (-t / 0.0015).exp());
        let click = if i < (rate_f * 0.0008) as usize { 0.4 * (1.0 - i as f32 / (rate_f * 0.0008)) } else { 0.0 };
        out.push(0.8 * env * phase.sin() + click);
    }
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.0 {
        for s in &mut out {
            *s *= 0.8 / peak;
        }
    }
    out
}

/// The echo plan for a drip at `source` heard at `listener` facing
/// `forward`, in a cave that rings for `rt60_s`: the direct plink, two early
/// reflections off the chamber walls (later and more spread in a longer
/// ringing cave), and a long tail at the cave's RT60.
pub fn drip_plan(source: [f32; 3], listener: [f32; 3], forward: [f32; 3], rt60_s: f32) -> EchoPlan {
    let d = ((source[0] - listener[0]).powi(2) + (source[1] - listener[1]).powi(2) + (source[2] - listener[2]).powi(2)).sqrt();
    let g = (4.0 / d.max(4.0)).min(1.0);
    let sg = sound_stereo::stereo(g, listener, forward, source);
    let direct = Path { delay_s: d / SPEED_OF_SOUND, left: sg.left, right: sg.right, lowpass_hz: 14_000.0 };
    let rt = rt60_s.max(0.1);
    // Early reflections: a bigger, longer-ringing chamber answers later.
    let e1 = direct.delay_s + 0.03 + 0.012 * rt;
    let e2 = direct.delay_s + 0.05 + 0.025 * rt;
    let reflections = vec![
        Path { delay_s: e1, left: 0.30 * g, right: 0.12 * g, lowpass_hz: 7_000.0 },
        Path { delay_s: e2, left: 0.10 * g, right: 0.26 * g, lowpass_hz: 5_000.0 },
    ];
    EchoPlan {
        direct,
        occluded: false,
        ridge_m: 0.0,
        reflections,
        tail: Tail { level: 0.35 * g.max(0.3), decay_s: rt, enclosure: 1.0 },
        distance: d,
    }
}

/// How the game renders a drip: quiet (a drip is small), with room for the
/// cave's whole ring (up to the abyss's 20 s) and little damping in the tail,
/// so the RT60 heard is the RT60 asked for. With `sound_render`'s defaults
/// (tail cut at 6 s, damping 0.3) a 9.7 s abyssal plan measured about 3 s in
/// a soak's dump.
pub fn render_opts() -> crate::sim::sound_render::RenderOpts {
    crate::sim::sound_render::RenderOpts { gain: 0.35, max_tail_s: 20.0, tail_damp: 0.05, ..Default::default() }
}

/// When the next drip falls: exponential gaps (a Poisson process) at the
/// zone's rate, deterministic from a seed and a counter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DripClock {
    next: f32,
    n: u32,
    seed: u32,
}

impl DripClock {
    pub fn new(seed: u32) -> DripClock {
        DripClock { next: f32::NAN, n: 0, seed }
    }

    /// Is a drip due at `now` (seconds) at `per_minute`? Returns the drip's
    /// own seed (for its sound and position) when one falls. A rate of 0
    /// never drips and forgets the schedule, so entering a wet cave starts
    /// fresh rather than firing a backlog.
    pub fn due(&mut self, now: f32, per_minute: f32) -> Option<u32> {
        if per_minute <= 0.0 {
            self.next = f32::NAN;
            return None;
        }
        let mean = 60.0 / per_minute;
        if self.next.is_nan() {
            self.next = now + self.gap(mean);
            return None;
        }
        if now < self.next {
            return None;
        }
        self.n = self.n.wrapping_add(1);
        let s = hash(self.seed ^ self.n.wrapping_mul(0x27D4_EB2F));
        self.next = now + self.gap(mean);
        Some(s)
    }

    fn gap(&self, mean: f32) -> f32 {
        let u = unit(self.seed.wrapping_add(self.n.wrapping_mul(0x1656_67B1))).max(1e-4);
        (-u.ln() * mean).clamp(0.3, mean * 5.0)
    }
}

/// Where a drip falls, `seed` picking an offset from the listener: 4 to 24
/// blocks away horizontally. The caller finds the ceiling above that column.
pub fn drip_offset(seed: u32) -> [f32; 2] {
    let a = std::f32::consts::TAU * unit(seed ^ 0x1234);
    let r = 4.0 + 20.0 * unit(seed ^ 0x4321).sqrt();
    [a.cos() * r, a.sin() * r]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::sound_render::{render, RenderOpts};

    const RATE: u32 = 24_000;

    #[test]
    fn rt60_ranges_are_the_docx_ones() {
        assert_eq!(rt60_range(Zone::Limestone), Some((2.0, 8.0)));
        assert_eq!(rt60_range(Zone::Crystal), Some((4.0, 10.0)));
        assert_eq!(rt60_range(Zone::Abyssal), Some((8.0, 20.0)));
        assert_eq!(rt60_range(Zone::Geothermal), Some((3.0, 15.0)));
        assert_eq!(rt60_range(Zone::Soil), None);
        assert_eq!(rt60_s(Zone::Limestone, 0.5), Some(5.0));
        assert_eq!(rt60_s(Zone::Limestone, 7.0), Some(8.0), "clamped into the range");
    }

    /// Dominant frequency from zero crossings over a slice.
    fn zc_hz(s: &[f32]) -> f32 {
        let z = s.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        z as f32 * RATE as f32 / 2.0 / s.len() as f32
    }

    #[test]
    fn a_drip_is_a_short_high_plink_that_rises_in_pitch() {
        for seed in [1u32, 7, 99, 12345] {
            let d = drip(RATE, seed);
            let secs = d.len() as f32 / RATE as f32;
            assert!((0.08..0.35).contains(&secs), "length {secs}");
            let peak = d.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!((0.7..=0.81).contains(&peak), "peak {peak}");
            // Most of the energy in the first 60 ms.
            let e = |a: usize, b: usize| d[a..b.min(d.len())].iter().map(|s| s * s).sum::<f32>();
            let cut = (RATE as f32 * 0.06) as usize;
            assert!(e(0, cut) > 0.9 * e(0, d.len()), "seed {seed}: energy lingers");
            // High: a kilohertz or two, and higher at the end of the glide.
            let (a, b) = ((RATE as f32 * 0.002) as usize, (RATE as f32 * 0.012) as usize);
            let (c, dd) = ((RATE as f32 * 0.03) as usize, (RATE as f32 * 0.045) as usize);
            let (f_early, f_late) = (zc_hz(&d[a..b]), zc_hz(&d[c..dd]));
            assert!((900.0..3200.0).contains(&f_early), "seed {seed}: starts at {f_early} Hz");
            assert!(f_late > f_early * 1.2, "seed {seed}: {f_early} -> {f_late} Hz, no rise");
        }
        assert_ne!(drip(RATE, 1), drip(RATE, 2), "no two drips alike");
    }

    /// Energy of an interleaved stereo render between two times.
    fn energy(st: &[f32], t0: f32, t1: f32) -> f32 {
        let (a, b) = ((t0 * RATE as f32) as usize * 2, (t1 * RATE as f32) as usize * 2);
        st[a.min(st.len())..b.min(st.len())].iter().map(|s| s * s).sum()
    }

    #[test]
    fn a_longer_ringing_cave_keeps_the_drip_alive_longer() {
        let dry = drip(RATE, 5);
        let (src, ear, fwd) = ([10.0, 8.0, 0.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        let short = render(&dry, RATE, &drip_plan(src, ear, fwd, 2.0), RenderOpts::default());
        let long = render(&dry, RATE, &drip_plan(src, ear, fwd, 6.0), RenderOpts::default());
        let (s_late, l_late) = (energy(&short, 1.0, 2.0), energy(&long, 1.0, 2.0));
        assert!(l_late > 2.0 * s_late, "late energy {l_late} vs {s_late}");
        assert!(long.len() > short.len());
        // It still rings at all: a late tail is not silence.
        assert!(energy(&long, 1.0, 2.0) > 1e-3 * energy(&long, 0.0, 0.2));
    }

    /// RT60 measured from a render: the slope of the energy envelope (50 ms
    /// windows, dB) between 10 and 35 dB below the tail's level at 0.3 s,
    /// extrapolated to 60 dB.
    fn measured_rt60(st: &[f32]) -> f32 {
        let win = 0.05;
        let db = |t: f32| 10.0 * (energy(st, t, t + win).max(1e-20)).log10();
        let ref_db = db(0.3);
        let (mut t10, mut t35) = (None, None);
        let mut t = 0.3;
        while t < st.len() as f32 / 2.0 / RATE as f32 - win {
            let d = ref_db - db(t);
            if t10.is_none() && d >= 10.0 {
                t10 = Some(t);
            }
            if t35.is_none() && d >= 35.0 {
                t35 = Some(t);
                break;
            }
            t += win;
        }
        match (t10, t35) {
            (Some(a), Some(b)) => (b - a) * 60.0 / 25.0,
            _ => f32::INFINITY,
        }
    }

    #[test]
    fn the_rendered_ring_lasts_about_the_rt60_asked_for() {
        let dry = drip(RATE, 11);
        let (src, ear, fwd) = ([8.0, 6.0, 3.0], [0.0; 3], [1.0, 0.0, 0.0]);
        for target in [3.0f32, 8.0, 14.0] {
            let st = render(&dry, RATE, &drip_plan(src, ear, fwd, target), render_opts());
            let rt = measured_rt60(&st);
            assert!(rt > target * 0.6 && rt < target * 1.4, "asked {target} s, measured {rt:.2} s");
        }
    }

    #[test]
    fn the_drip_arrives_from_its_side() {
        let dry = drip(RATE, 3);
        // Listener faces +x; +z is to the right in sound_stereo's convention
        // or to the left; whichever it is, the two sides must differ and
        // mirror each other.
        let a = render(&dry, RATE, &drip_plan([0.0, 4.0, 12.0], [0.0; 3], [1.0, 0.0, 0.0], 0.1), RenderOpts::default());
        let b = render(&dry, RATE, &drip_plan([0.0, 4.0, -12.0], [0.0; 3], [1.0, 0.0, 0.0], 0.1), RenderOpts::default());
        let lr = |s: &[f32]| {
            let n = (RATE as f32 * 0.1) as usize * 2;
            let (mut l, mut r) = (0.0, 0.0);
            for f in s[..n.min(s.len())].chunks(2) {
                l += f[0] * f[0];
                r += f[1] * f[1];
            }
            (l, r)
        };
        let ((al, ar), (bl, br)) = (lr(&a), lr(&b));
        assert!((al > ar) != (bl > br), "both sides sound the same: {al}/{ar} vs {bl}/{br}");
    }

    #[test]
    fn drips_fall_at_about_the_zone_rate_and_not_at_all_where_it_is_dry() {
        let mut c = DripClock::new(42);
        let mut n = 0;
        let mut t = 0.0;
        while t < 600.0 {
            if c.due(t, drips_per_minute(Zone::Limestone)).is_some() {
                n += 1;
            }
            t += 0.05;
        }
        assert!((70..=130).contains(&n), "{n} drips in ten minutes at 10/min");
        // And a different rate gives a different count (the rate is used).
        let mut c = DripClock::new(42);
        let mut n4 = 0;
        let mut t = 0.0;
        while t < 600.0 {
            if c.due(t, drips_per_minute(Zone::Abyssal)).is_some() {
                n4 += 1;
            }
            t += 0.05;
        }
        assert!((22..=60).contains(&n4), "{n4} drips in ten minutes at 4/min");
        let mut c = DripClock::new(42);
        for k in 0..2000 {
            assert_eq!(c.due(k as f32 * 0.1, drips_per_minute(Zone::Geothermal)), None);
        }
        assert!(drips_per_minute(Zone::Limestone) > drips_per_minute(Zone::Abyssal));
    }

    #[test]
    fn drip_offsets_stay_in_the_ring() {
        for s in 0..500u32 {
            let [x, z] = drip_offset(s);
            let r = (x * x + z * z).sqrt();
            assert!((3.99..=24.01).contains(&r), "{r}");
        }
    }
}
