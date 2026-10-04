//! How good is a model, honestly: hold whole DOTS out. Frames of one dot are near-duplicates, so a random split of frames
//! leaks the answer; the held-out set here is dots the model never saw a frame of.

use crate::dataset::{Dataset, Sample};
use crate::model::{Config, Model};
use crate::rng::Rng;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Report {
    /// Root-mean-square error, in screen fractions (distance on the unit square).
    pub rmse: f32,
    pub median_err: f32,
    /// Fraction of held-out frames predicted within `radius` of the dot.
    pub hit_rate: f32,
    pub radius: f32,
    pub n: usize,
}

/// Leave-dots-out cross-validation with `folds` folds, deterministic in `seed`. None when there are fewer dots than folds or
/// a fold cannot be trained.
pub fn cross_validate(ds: &Dataset, cfg: &Config, folds: usize, radius: f32) -> Option<Report> {
    let mut keys: Vec<(u32, u32)> = ds.samples.iter().map(|s| (s.session, s.dot)).collect();
    keys.sort();
    keys.dedup();
    if folds < 2 || keys.len() < folds { return None; }
    let mut rng = Rng::new(cfg.seed ^ 0x5eed);
    rng.shuffle(&mut keys);
    let mut errs: Vec<f32> = Vec::new();
    for fold in 0..folds {
        let held: std::collections::HashSet<(u32, u32)> = keys.iter().enumerate().filter(|(i, _)| i % folds == fold).map(|(_, k)| *k).collect();
        let train: Vec<Sample> = ds.samples.iter().filter(|s| !held.contains(&(s.session, s.dot))).cloned().collect();
        let model = Model::fit(&train, cfg)?;
        for s in ds.samples.iter().filter(|s| held.contains(&(s.session, s.dot))) {
            let p = model.predict(&s.f)?;
            errs.push(((p[0] - s.target[0]).powi(2) + (p[1] - s.target[1]).powi(2)).sqrt());
        }
    }
    let n = errs.len();
    let rmse = (errs.iter().map(|e| e * e).sum::<f32>() / n as f32).sqrt();
    let hits = errs.iter().filter(|&&e| e <= radius).count() as f32 / n as f32;
    errs.sort_by(|a, b| a.total_cmp(b));
    Some(Report { rmse, median_err: errs[n / 2], hit_rate: hits, radius, n })
}

/// The candidates the app chooses between, simplest first.
pub fn candidates(base: &Config) -> Vec<Config> {
    use crate::model::Kind;
    vec![
        Config { kind: Kind::Mean, ..base.clone() },
        Config { kind: Kind::Ridge, ..base.clone() },
        Config { kind: Kind::Mlp, epochs: 200, lr: 0.003, hidden: 32, ..base.clone() },
    ]
}

/// Cross-validate every candidate and return the winner by held-out RMSE, with all the reports. A more complex model must
/// win by at least 3% to displace a simpler one: a network that is only as good as a line is the line, trained slower.
pub fn pick_best(ds: &Dataset, base: &Config, folds: usize, radius: f32) -> Option<(Config, Vec<(Config, Report)>)> {
    let mut all: Vec<(Config, Report)> = Vec::new();
    for c in candidates(base) { all.push((c.clone(), cross_validate(ds, &c, folds, radius)?)); }
    let mut best = 0;
    for i in 1..all.len() { if all[i].1.rmse < all[best].1.rmse * 0.97 { best = i; } }
    Some((all[best].0.clone(), all))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kind;

    /// 20 dots, 6 near-identical frames each: a learnable linear signal, plus frame-to-frame noise.
    fn dots(signal: bool) -> Dataset {
        let mut rng = Rng::new(9);
        let mix: Vec<[f32; 2]> = (0..40).map(|_| [rng.normal() as f32, rng.normal() as f32]).collect();
        let mut d = Dataset::new(40);
        for dot in 0..20u32 {
            let (x, y) = (rng.unit() as f32, rng.unit() as f32);
            for _ in 0..6 {
                let f: Vec<f32> = mix.iter().map(|m| if signal { m[0] * (x - 0.5) * 3.0 + m[1] * (y - 0.5) * 3.0 } else { 0.0 } + rng.normal() as f32 * 0.4).collect();
                d.push(Sample { f, target: [x, y], dot, session: 1 }).unwrap();
            }
        }
        d
    }

    #[test]
    fn held_out_dots_show_learning_when_there_is_a_signal_and_none_when_there_is_not() {
        let cfg = |k| Config { kind: k, epochs: 40, ..Config::default() };
        let real = cross_validate(&dots(true), &cfg(Kind::Ridge), 5, 0.15).unwrap();
        let none = cross_validate(&dots(false), &cfg(Kind::Ridge), 5, 0.15).unwrap();
        let mean = cross_validate(&dots(true), &cfg(Kind::Mean), 5, 0.15).unwrap();
        assert_eq!(real.n, 120);
        assert!(real.rmse < mean.rmse * 0.6, "with a signal ridge beats the mean on UNSEEN dots: {} vs {}", real.rmse, mean.rmse);
        assert!(none.rmse > real.rmse * 1.5, "with no signal it cannot: {} vs {}", none.rmse, real.rmse);
        assert!(real.hit_rate > mean.hit_rate && (0.0..=1.0).contains(&real.hit_rate));
    }

    #[test]
    fn it_is_deterministic_and_refuses_what_it_cannot_do() {
        let d = dots(true);
        let c = Config { kind: Kind::Ridge, ..Config::default() };
        assert_eq!(cross_validate(&d, &c, 5, 0.1), cross_validate(&d, &c, 5, 0.1));
        assert!(cross_validate(&d, &c, 1, 0.1).is_none(), "one fold is not a validation");
        assert!(cross_validate(&d, &c, 21, 0.1).is_none(), "more folds than dots");
        assert!(cross_validate(&Dataset::new(4), &c, 5, 0.1).is_none());
    }

    #[test]
    fn the_choice_is_made_on_unseen_dots_and_prefers_the_simpler_model_when_it_is_as_good() {
        use crate::model::Kind;
        let (best, all) = pick_best(&dots(true), &Config::default(), 5, 0.15).unwrap();
        assert_eq!(all.len(), 3);
        assert_ne!(best.kind, Kind::Mean, "there is a signal: something beats the centre");
        let ridge = all.iter().find(|(c, _)| c.kind == Kind::Ridge).unwrap().1.rmse;
        let mlp = all.iter().find(|(c, _)| c.kind == Kind::Mlp).unwrap().1.rmse;
        if mlp >= ridge * 0.97 { assert_eq!(best.kind, Kind::Ridge, "a network no better than a line is not chosen: mlp {mlp} ridge {ridge}"); }
        // no signal at all: nothing may be called better than the plain centre by more than the margin
        let (none_best, _) = pick_best(&dots(false), &Config::default(), 5, 0.15).unwrap();
        assert_eq!(none_best.kind, Kind::Mean, "pure noise: the honest answer is the centre");
    }
}
