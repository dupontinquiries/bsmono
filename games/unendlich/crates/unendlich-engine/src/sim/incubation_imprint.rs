//! `sim::incubation_imprint` — dragon egg incubation imprint (spec §14.5.2, R36).
//!
//! Over incubation an egg accumulates the field history of its surroundings.
//! That history nudges the hatchling's attunement and biases which
//! sub-discipline surfaces — incubate beside a heat vent for a fire-attuned
//! hatchling, in deep water for coral. This is the dragon analogue of R34
//! `germination`'s bloom imprint (the dragon core did not exist — verified gap);
//! its output feeds `dragon_attunement` at hatch.
//!
//! It only *surfaces latent bias* — it cannot create an allele the genome lacks
//! (the caller gates the result against the genome).
//!
//! ## Fields and mapping
//! `accum[7]` accumulates exposure to seven ambient fields `h,r,w,p,l,s,b`.
//! Six of them each feed one sub-discipline; `s` is the attunement catalyst
//! (it strengthens the nudge but surfaces no specific discipline):
//!
//! | sub_bias | discipline | fed by      |
//! |----------|------------|-------------|
//! | 0        | fire       | `r` (idx 1) |
//! | 1        | storm      | `l` (idx 4) |
//! | 2        | coral      | `w` (idx 2) |
//! | 3        | stone      | `h` (idx 0) |
//! | 4        | verdant    | `b` (idx 6) |
//! | 5        | void       | `p` (idx 3) |
//!
//! `readout` centers the six discipline exposures about their mean, so **balanced
//! exposure reads neutral** (all biases ≈ 0); a sustained dominant field surfaces
//! its discipline (positive) and suppresses the rest. `attunement_delta` rises
//! with how *concentrated* and *strong* the imprint is (plus the `s` catalyst),
//! so sustained high-`r` gives a fire bias and a positive nudge, while balanced
//! exposure nudges ≈ 0.
//!
//! Pure, deterministic, saturating, panic-free — no `crate::render`, no GPU, no
//! I/O, no clock, no RNG, no `.unwrap()`/`.expect()` in non-test code.

/// Accumulated incubation exposure to the seven ambient fields `h,r,w,p,l,s,b`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Imprint {
    pub accum: [f32; 7],
}

/// How the accumulated history nudges the hatchling at readout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImprintResult {
    /// Attunement nudge `[0, 1]` — stronger for concentrated, sustained imprints.
    pub attunement_delta: f32,
    /// Per-discipline bias `[-1, 1]`, centered so balanced exposure ≈ 0.
    pub sub_bias: [f32; 6],
}

/// Saturation ceiling for any one accumulated field.
const ACCUM_CAP: f32 = 1000.0;
/// Scales centered exposure into the `[-1, 1]` bias range.
const BIAS_SCALE: f32 = 1.0 / ACCUM_CAP;
/// Total exposure (six disciplines + catalyst) at which the nudge saturates.
const DELTA_SCALE: f32 = 2.0 * ACCUM_CAP;
/// Maximum attunement nudge.
const ATTUNE_GAIN: f32 = 1.0;

fn sanitize(x: f32) -> f32 {
    // exposure / dt are non-negative; NaN or negatives contribute nothing.
    if x.is_nan() || x < 0.0 {
        0.0
    } else {
        x
    }
}

/// Add one incubation step: each field's exposure (`fields[i]`, clamped `≥ 0`)
/// times `dt` (clamped `≥ 0`) is added into `accum`, saturating at `ACCUM_CAP`.
pub fn accumulate(im: Imprint, fields: &[f32; 7], dt: f32) -> Imprint {
    let dt = sanitize(dt);
    let mut accum = im.accum;
    let mut i = 0;
    while i < 7 {
        let v = accum[i] + sanitize(fields[i]) * dt;
        accum[i] = if v > ACCUM_CAP {
            ACCUM_CAP
        } else if v < 0.0 {
            0.0
        } else {
            v
        };
        i += 1;
    }
    Imprint { accum }
}

/// Read the accumulated history out as an attunement nudge + per-discipline bias.
pub fn readout(im: &Imprint) -> ImprintResult {
    let a = &im.accum;
    // Six discipline exposures: fire, storm, coral, stone, verdant, void.
    let raw = [a[1], a[4], a[2], a[0], a[6], a[3]];

    let total6: f32 = raw.iter().sum();
    let mean = total6 / 6.0;

    let mut sub_bias = [0.0f32; 6];
    let mut k = 0;
    while k < 6 {
        sub_bias[k] = ((raw[k] - mean) * BIAS_SCALE).clamp(-1.0, 1.0);
        k += 1;
    }

    // attunement nudge: concentration (how much one discipline dominates) × the
    // overall imprint strength (incl. the `s` catalyst at idx 5).
    let max_raw = raw.iter().copied().fold(0.0f32, f32::max);
    let conc = if total6 > 0.0 {
        (max_raw / total6 - 1.0 / 6.0).max(0.0)
    } else {
        0.0
    };
    let strength = ((total6 + a[5]) / DELTA_SCALE).clamp(0.0, 1.0);
    let attunement_delta = (ATTUNE_GAIN * conc * strength).clamp(0.0, ATTUNE_GAIN);

    ImprintResult { attunement_delta, sub_bias }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRE: usize = 0;
    const CORAL: usize = 2;
    const VOID: usize = 5;

    // Accumulate `steps` of a single-field exposure.
    fn incubate_field(field_idx: usize, amount: f32, steps: usize) -> Imprint {
        let mut f = [0.0f32; 7];
        f[field_idx] = amount;
        let mut im = Imprint::default();
        for _ in 0..steps {
            im = accumulate(im, &f, 1.0);
        }
        im
    }

    fn argmax(bias: &[f32; 6]) -> usize {
        let mut best = 0;
        for k in 1..6 {
            if bias[k] > bias[best] {
                best = k;
            }
        }
        best
    }

    #[test]
    fn sustained_high_r_gives_fire_bias_and_positive_delta() {
        // r is field idx 1.
        let im = incubate_field(1, 5.0, 200);
        let r = readout(&im);
        assert_eq!(argmax(&r.sub_bias), FIRE, "fire bias should dominate");
        assert!(r.sub_bias[FIRE] > 0.0, "fire bias positive");
        assert!(r.attunement_delta > 0.0, "attunement nudged up");
    }

    #[test]
    fn deep_water_gives_coral_foggy_gives_void() {
        // w is idx 2 ⇒ coral; p is idx 3 ⇒ void.
        let water = readout(&incubate_field(2, 5.0, 200));
        assert_eq!(argmax(&water.sub_bias), CORAL);
        assert!(water.sub_bias[CORAL] > 0.0);

        let fog = readout(&incubate_field(3, 5.0, 200));
        assert_eq!(argmax(&fog.sub_bias), VOID);
        assert!(fog.sub_bias[VOID] > 0.0);
    }

    #[test]
    fn balanced_exposure_is_neutral() {
        let im = accumulate(Imprint::default(), &[3.0; 7], 100.0); // all fields equal
        let r = readout(&im);
        for (k, &b) in r.sub_bias.iter().enumerate() {
            assert!(b.abs() < 1e-4, "bias[{k}]={b} should be ~0 when balanced");
        }
        assert!(r.attunement_delta.abs() < 1e-4, "balanced ⇒ ~0 nudge");
    }

    #[test]
    fn accum_saturates_at_cap() {
        let im = incubate_field(1, 1e9, 50); // enormous exposure
        for (i, &v) in im.accum.iter().enumerate() {
            assert!(v <= ACCUM_CAP, "accum[{i}]={v} exceeds cap");
            assert!(v.is_finite());
        }
        assert!((im.accum[1] - ACCUM_CAP).abs() < 1e-3, "r saturated to cap");
    }

    #[test]
    fn readout_monotone_in_accum() {
        // More r accumulated ⇒ fire bias and attunement_delta non-decreasing.
        let mut prev_fire = f32::MIN;
        let mut prev_delta = f32::MIN;
        for steps in [0usize, 1, 5, 20, 80, 200] {
            let r = readout(&incubate_field(1, 4.0, steps));
            assert!(r.sub_bias[FIRE] >= prev_fire - 1e-6, "fire bias monotone");
            assert!(r.attunement_delta >= prev_delta - 1e-6, "delta monotone");
            prev_fire = r.sub_bias[FIRE];
            prev_delta = r.attunement_delta;
        }
    }

    #[test]
    fn nan_and_out_of_range_fields_clamp() {
        let weird = [f32::NAN, -5.0, 1e30, f32::INFINITY, -0.0, 2.0, f32::NAN];
        let im = accumulate(Imprint::default(), &weird, 1.0);
        for (i, &v) in im.accum.iter().enumerate() {
            assert!(v.is_finite() && (0.0..=ACCUM_CAP).contains(&v), "accum[{i}]={v}");
        }
        // negative dt contributes nothing
        let im2 = accumulate(Imprint::default(), &[5.0; 7], -3.0);
        assert_eq!(im2.accum, [0.0; 7]);
        let r = readout(&im);
        assert!(r.attunement_delta.is_finite());
        assert!(r.sub_bias.iter().all(|b| b.is_finite() && (-1.0..=1.0).contains(b)));
    }

    #[test]
    fn deterministic() {
        let f = [1.0, 2.0, 0.5, 3.0, 0.0, 1.5, 0.2];
        let a = readout(&accumulate(Imprint::default(), &f, 2.0));
        let b = readout(&accumulate(Imprint::default(), &f, 2.0));
        assert_eq!(a, b);
    }

    #[test]
    fn biases_centered_sum_near_zero() {
        // centering ⇒ the six biases always sum to ~0 (before clamping bites)
        let im = incubate_field(1, 5.0, 30);
        let r = readout(&im);
        let s: f32 = r.sub_bias.iter().sum();
        assert!(s.abs() < 1e-3, "centered biases sum to ~0, got {s}");
    }
}
