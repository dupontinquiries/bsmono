//! coreclass.rs — which cores are efficiency cores, and what belongs on them.
//!
//! On an M2 Ultra under Asahi the CPU is asymmetric: 16 performance cores and 8 efficiency cores. The
//! Thinker wants every P-core it can get; the Liaison wants to exist without taking any of them. Pinning
//! the small model to the E-cores gives the large one an uncontended machine, which is worth more than
//! the Liaison's own throughput.
//!
//! ## `cpu_capacity`, not a model table
//!
//! Linux publishes `/sys/devices/system/cpu/cpuN/cpu_capacity` — a normalised figure the asymmetric
//! scheduler itself uses, with the largest core at 1024. E-cores report lower. Reading that works on any
//! asymmetric machine and needs no table of SoC names to fall out of date, which is the same reason the
//! VRAM ceiling is probed rather than constant.
//!
//! A uniform machine reports the same value everywhere, and this correctly finds no E-cores rather than
//! inventing a split — a container or an x86 box must not get half its cores declared slow.
//!
//! ## uclamp vs affinity
//!
//! `sched_setaffinity` is a HARD pin: the thread cannot run elsewhere even when the E-cores are busy and
//! the P-cores are idle. `uclamp_max` is a HINT: it caps the thread's contribution to the frequency and
//! placement decision, so the scheduler prefers small cores but can still move it when that would
//! otherwise stall.
//!
//! For a Liaison the hint is the better default. Its job is to answer while the Thinker works, and a
//! hard pin turns "the E-cores are busy" into "the Liaison stops" — which is precisely when someone is
//! waiting on it. The hard pin is available for measurement, where determinism matters more.

/// One CPU and its capacity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cpu { pub id: usize, pub capacity: u32 }

/// Read the capacity of every online CPU.
pub fn read_capacities() -> Vec<Cpu> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/sys/devices/system/cpu") else { return out };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("cpu") else { continue };
        let Ok(id) = rest.parse::<usize>() else { continue };
        let p = e.path().join("cpu_capacity");
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(c) = s.trim().parse::<u32>() { out.push(Cpu { id, capacity: c }); }
        }
    }
    out.sort_by_key(|c| c.id);
    out
}

/// Split CPUs into efficiency and performance sets.
///
/// Anything below the maximum capacity is an E-core. Returns None when the machine is uniform — there
/// is no split to make, and manufacturing one would pin the Liaison to cores no slower than the rest
/// while halving what the Thinker can use.
pub fn classify(cpus: &[Cpu]) -> Option<(Vec<usize>, Vec<usize>)> {
    if cpus.len() < 2 { return None; }
    let max = cpus.iter().map(|c| c.capacity).max()?;
    let min = cpus.iter().map(|c| c.capacity).min()?;
    if min == max { return None; }
    let e: Vec<usize> = cpus.iter().filter(|c| c.capacity < max).map(|c| c.id).collect();
    let p: Vec<usize> = cpus.iter().filter(|c| c.capacity == max).map(|c| c.id).collect();
    Some((e, p))
}

/// What the E-cores are worth relative to the P-cores, as a throughput fraction.
///
/// Used to decide whether a model is small enough to be worth putting there — an answer that arrives at
/// a third of the speed is still an answer, but only if it arrives.
pub fn efficiency_ratio(cpus: &[Cpu]) -> Option<f32> {
    let (e, p) = classify(cpus)?;
    let cap = |ids: &[usize]| ids.iter()
        .filter_map(|i| cpus.iter().find(|c| c.id == *i)).map(|c| c.capacity as f32).sum::<f32>();
    let (ec, pc) = (cap(&e), cap(&p));
    if pc <= 0.0 { return None; }
    Some(ec / pc)
}

/// How to place a model.
#[derive(Clone, PartialEq, Debug)]
pub enum Placement {
    /// Bias toward the efficiency cores, but let the scheduler move it if they are busy.
    ///
    /// The default for a Liaison: its job is to answer WHILE the Thinker works, and a hard pin turns
    /// "the E-cores are busy" into "the Liaison stops" — exactly when someone is waiting on it.
    PreferEfficiency { cores: Vec<usize>, hint_only: bool },
    /// Use everything. The Thinker, and anything the E-cores cannot carry.
    AllCores,
}

/// Decide where a model goes.
///
/// `est_tok_s_all_cores` is what it would achieve using the whole machine. The rule: put it on the
/// E-cores only if the result would still be useful there — a Liaison at 3 tok/s is fine, at 0.4 is
/// not, and the difference is what makes this a decision rather than a preference.
pub fn place(cpus: &[Cpu], est_tok_s_all_cores: f32, min_useful_tok_s: f32) -> Placement {
    let Some((e, _p)) = classify(cpus) else { return Placement::AllCores };
    let Some(ratio) = efficiency_ratio(cpus) else { return Placement::AllCores };
    if est_tok_s_all_cores * ratio < min_useful_tok_s { return Placement::AllCores; }
    Placement::PreferEfficiency { cores: e, hint_only: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m2_ultra() -> Vec<Cpu> {
        // 16 P at 1024, 8 E at 380 — the shape Asahi reports for this SoC class.
        (0..24).map(|id| Cpu { id, capacity: if id < 16 { 1024 } else { 380 } }).collect()
    }

    #[test]
    fn a_uniform_machine_has_no_split_to_make() {
        // Manufacturing one would pin the Liaison to cores no slower than the rest while halving what
        // the Thinker can use — strictly worse than doing nothing, on a container or any x86 box.
        let uniform: Vec<Cpu> = (0..8).map(|id| Cpu { id, capacity: 1024 }).collect();
        assert!(classify(&uniform).is_none());
        assert!(efficiency_ratio(&uniform).is_none());
        assert_eq!(place(&uniform, 50.0, 1.0), Placement::AllCores);
    }

    #[test]
    fn the_asymmetric_split_finds_the_efficiency_cores() {
        let (e, p) = classify(&m2_ultra()).expect("asymmetric");
        assert_eq!(p.len(), 16);
        assert_eq!(e, (16..24).collect::<Vec<_>>());
        let r = efficiency_ratio(&m2_ultra()).unwrap();
        assert!((r - (8.0 * 380.0) / (16.0 * 1024.0)).abs() < 1e-4, "got {r}");
        assert!(r < 0.25, "the E-cores are worth well under a quarter of the P-cores: {r}");
    }

    #[test]
    fn a_model_too_slow_on_E_cores_gets_the_whole_machine() {
        // The rule that makes this a decision rather than a preference: an answer at a third of the
        // speed is still an answer, but only if it arrives. A liaison at 0.4 tok/s is not a liaison.
        let cpus = m2_ultra();
        // 40 tok/s all-cores * ~0.19 = 7.4 on the E-cores: fine for a liaison
        assert!(matches!(place(&cpus, 40.0, 2.0), Placement::PreferEfficiency { .. }));
        // 5 tok/s all-cores would be ~0.9 there: not useful, so it keeps the machine
        assert_eq!(place(&cpus, 5.0, 2.0), Placement::AllCores);
    }

    #[test]
    fn the_default_is_a_HINT_not_a_hard_pin() {
        // A hard pin turns "the E-cores are busy" into "the Liaison stops" — precisely when someone is
        // waiting on it. uclamp-style biasing degrades instead of stalling.
        match place(&m2_ultra(), 40.0, 2.0) {
            Placement::PreferEfficiency { hint_only, cores } => {
                assert!(hint_only, "a liaison must still run when the E-cores are contended");
                assert_eq!(cores.len(), 8);
            }
            other => panic!("expected efficiency placement, got {other:?}"),
        }
    }

    #[test]
    fn reading_capacities_on_a_machine_without_them_returns_empty_not_a_guess() {
        // Absent sysfs means "unknown", and classify() then declines to split. Inventing a topology
        // from a core count would be a table that falls out of date, which is what cpu_capacity exists
        // to avoid.
        let none: Vec<Cpu> = Vec::new();
        assert!(classify(&none).is_none());
        assert_eq!(place(&none, 100.0, 1.0), Placement::AllCores);
    }

    #[test]
    fn a_single_core_machine_is_not_split() {
        let one = vec![Cpu { id: 0, capacity: 1024 }];
        assert!(classify(&one).is_none());
    }
}
