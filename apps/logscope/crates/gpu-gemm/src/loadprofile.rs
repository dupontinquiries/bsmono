//! loadprofile.rs — what must be measured before the load path can be hand-tuned.
//!
//! A load reports one number: "loaded in 83.3s". That number cannot be optimised, because it is four
//! stages with four different bottlenecks and no indication which dominates.
//!
//! ```text
//!   file  ──read──▶  page cache  ──convert──▶  host buffer  ──stage──▶  staging  ──upload──▶  device
//! ```
//!
//! ## The fact that matters most on this hardware
//!
//! **On unified memory the staging copy may be pure waste.** An M2's "device memory" is the same
//! silicon as its host memory. If the driver exposes host-visible device allocations, a mapped write
//! puts bytes where the GPU will read them and the staging hop disappears — halving the bytes moved
//! for the whole load.
//!
//! If it does not expose them, the copy is mandatory and no amount of tuning removes it. **That one
//! boolean decides whether a whole stage is optimisable or fixed**, and nothing currently measures
//! it.

/// A stage of the load path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    /// File to page cache. Bounded by storage, or free on a second load.
    Read,
    /// GGUF blocks to f32 or to the resident quant. Pure CPU, parallelisable.
    Convert,
    /// Host buffer to a staging buffer the driver can DMA from.
    Stage,
    /// Staging to device.
    Upload,
}

/// What must be known about the machine before a stage can be tuned, and what knowing it unlocks.
pub struct Requirement {
    pub stage: Stage,
    /// The hardware fact.
    pub fact: &'static str,
    /// How to obtain it.
    pub how: &'static str,
    /// What changes if it comes back one way rather than the other. **A fact that changes no
    /// decision is not worth measuring**, and every entry here has to name its consequence.
    pub unlocks: &'static str,
}

/// The facts needed, in the order a tuner should collect them.
///
/// Ordered by how much each one CONSTRAINS the others: whether the staging copy can be skipped
/// decides the shape of the whole path, so it comes first even though it is the least familiar.
pub fn requirements() -> Vec<Requirement> {
    vec![
        Requirement {
            stage: Stage::Stage,
            fact: "does the adapter expose HOST-VISIBLE DEVICE memory (BAR / unified)?",
            how: "wgpu::Adapter memory heaps, or attempt a mapped-at-creation buffer with \
COPY_DST | STORAGE and see whether it is accepted",
            unlocks: "if yes, the staging hop disappears — a mapped write puts bytes where the GPU \
reads them, halving the bytes moved for the entire load. If no, the copy is mandatory and this \
stage is fixed rather than slow. THIS DECIDES THE SHAPE OF THE PATH and is why it is first",
        },
        Requirement {
            stage: Stage::Upload,
            fact: "sustained host-to-device bandwidth, and whether it varies with transfer size",
            how: "upload 16/64/256/1024 MiB, timed, after a warm-up transfer",
            unlocks: "the slab size. A driver that peaks at 64 MiB and degrades above it wants many \
small uploads; one that climbs to 1 GiB wants few large ones, and the arena's current 1024 MiB \
choice is a guess either way",
        },
        Requirement {
            stage: Stage::Convert,
            fact: "cores that scale on this workload, and whether they are heterogeneous",
            how: "convert one tensor on 1, 2, 4, 8, all threads and plot the curve",
            unlocks: "the worker count. On Apple silicon the efficiency cores are half the speed of \
the performance ones, so a naive num_cpus() split leaves fast cores idle waiting for slow ones — \
the curve shows where it stops helping",
        },
        Requirement {
            stage: Stage::Read,
            fact: "is the file in page cache, and does mmap beat read() here",
            how: "load twice and compare; the second load is the cached number",
            unlocks: "whether read time is worth optimising at all. A 30 GiB file on a machine with \
128 GB is cached after the first load, so the FIRST load is storage-bound and every subsequent one \
is not — and tuning against the wrong one of those two optimises a case that happens once",
        },
        Requirement {
            stage: Stage::Convert,
            fact: "does conversion overlap upload today, or are they serialised?",
            how: "timestamp the first and last of each per tensor and check for overlap",
            unlocks: "the largest structural win available if they are serialised. Converting tensor \
N+1 while N uploads makes the slower stage the only cost, and the two are near enough in magnitude \
that overlapping them approaches halving the load",
        },
    ]
}

/// Why the two Scout quants need separate measurement rather than one profile.
///
/// **Q4_K_M and IQ1_S stress different stages**, so a tuning that helps one may not touch the other:
/// Q4_K_M converts cheaply and moves more bytes; IQ1_S (once it loads natively) moves 2.88x fewer
/// bytes and does no conversion at all.
pub fn per_quant_note(quant: &str) -> &'static str {
    match quant {
        "Q4_K_M" =>
            "Q4_K_M converts from q4_k blocks to the resident form, so CONVERT is real work and \
UPLOAD moves 40 GiB. Expect convert and upload to be comparable, which is exactly the case where \
overlapping them wins most",
        "IQ1_S" =>
            "IQ1_S loaded natively does NO conversion — the file's bytes are the resident form — so \
the profile collapses to read and upload, and 2.88x fewer bytes move. A convert-side optimisation \
that helps Q4_K_M does nothing here, which is why one profile cannot serve both",
        _ => "unknown quant; measure it rather than assuming it resembles either",
    }
}

/// What a single number cannot tell you.
pub fn why_one_number_is_not_enough(total_s: f32) -> String {
    format!("'loaded in {total_s:.1}s' is four stages with four bottlenecks. It is consistent with \
being storage-bound, CPU-bound on conversion, bandwidth-bound on upload, or serialised between two \
stages that could overlap — and the fix for each of those is different and mostly exclusive of the \
others")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_HOST_VISIBLE_question_comes_first_because_it_constrains_the_rest() {
        // Whether the staging copy can be skipped decides the shape of the whole path, so it leads
        // even though it is the least familiar of the five.
        let r = requirements();
        assert_eq!(r[0].stage, Stage::Stage);
        assert!(r[0].unlocks.contains("DECIDES THE SHAPE OF THE PATH"));
    }

    #[test]
    fn EVERY_requirement_names_what_changes_if_the_answer_differs() {
        // A fact that changes no decision is not worth a measurement, and the tool must not collect
        // numbers for their own sake.
        for r in requirements() {
            assert!(r.unlocks.len() > 60, "{} has no stated consequence", r.fact);
            assert!(r.how.len() > 20, "{} has no method", r.fact);
        }
    }

    #[test]
    fn the_two_QUANTS_are_said_to_stress_different_stages() {
        // A convert-side win on Q4_K_M does nothing for a natively-loaded IQ1_S, so one profile
        // cannot serve both.
        assert!(per_quant_note("Q4_K_M").contains("CONVERT is real work"));
        assert!(per_quant_note("IQ1_S").contains("does NO conversion"));
        assert!(per_quant_note("IQ1_S").contains("one profile cannot serve both"));
    }

    #[test]
    fn an_UNKNOWN_quant_is_told_to_measure_rather_than_assume() {
        assert!(per_quant_note("IQ2_XXS").contains("rather than assuming"));
    }

    #[test]
    fn OVERLAP_is_listed_as_a_requirement_and_not_only_a_speed() {
        // Whether two stages are serialised is a structural fact, and it is the largest single win
        // available if the answer is yes.
        assert!(requirements().iter().any(|r| r.fact.contains("overlap")));
    }

    #[test]
    fn the_READ_requirement_distinguishes_FIRST_load_from_SUBSEQUENT() {
        // A 30 GiB file on a 128 GB machine is cached after the first load, so tuning against the
        // wrong one optimises a case that happens once.
        let read = requirements().into_iter().find(|r| r.stage == Stage::Read).unwrap();
        assert!(read.unlocks.contains("happens once"));
    }

    #[test]
    fn one_number_is_explained_as_consistent_with_FOUR_different_diagnoses() {
        let s = why_one_number_is_not_enough(83.3);
        for w in ["storage-bound", "CPU-bound", "bandwidth-bound", "serialised"] {
            assert!(s.contains(w), "missing {w}");
        }
    }
}

/// Is per-TENSOR hand-tuning worth it?
///
/// **Mostly no, and the reason is worth stating precisely.** A model has hundreds of tensors and the
/// load path treats them identically: read, convert, stage, upload. Tuning tensor 47 differently from
/// tensor 48 optimises variance that does not exist, and the bookkeeping costs more than it saves.
///
/// **But two properties DO change the path**, and they partition the tensors into a handful of groups
/// rather than hundreds of cases:
///
/// * **Quant type.** A UD checkpoint carries seven, and they convert at different costs — IQ3_S is
///   codebook lookups, Q4_K is shifts and a scale. A convert-side optimisation lands on some and not
///   others.
/// * **Destination.** Resident, pinned expert, or streamed-on-demand. A streamed expert is uploaded
///   again every time it is needed, so its cost is multiplied by how often the router picks it,
///   while a resident tensor is uploaded once.
///
/// So: **cluster, do not enumerate.** Four hundred tensors become five or six groups, and a group is
/// large enough that measuring it is meaningful and small enough that tuning it is one decision.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Destination {
    /// Uploaded once, stays. Cost is paid at load.
    Resident,
    /// Uploaded once and kept for the session, but competing for the pin budget.
    PinnedExpert,
    /// **Uploaded every time the router picks it.** Its load cost is multiplied by selection
    /// frequency, which is the one place a per-group optimisation compounds.
    Streamed,
}

/// A group of tensors that share a load path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LoadGroup {
    pub ggml_type: u32,
    pub destination: Destination,
    pub tensors: usize,
    pub bytes: u64,
}

impl LoadGroup {
    /// How many times these bytes cross the bus in a session, roughly.
    ///
    /// **This is why grouping beats enumerating.** A streamed group's bytes move once per selection,
    /// so a 2 GiB group picked every third token moves more total than a 20 GiB resident group that
    /// moves once — and no per-tensor view surfaces that.
    pub fn bus_bytes(&self, tokens: u64, selection_rate: f64) -> u64 {
        match self.destination {
            Destination::Resident | Destination::PinnedExpert => self.bytes,
            Destination::Streamed =>
                self.bytes + (self.bytes as f64 * selection_rate * tokens as f64) as u64,
        }
    }

    pub fn worth_tuning(&self, total_bytes: u64) -> bool {
        // Below a twentieth of the load, a group cannot repay the work of treating it specially.
        total_bytes > 0 && (self.bytes as f64 / total_bytes as f64) > 0.05
    }
}

/// Cluster tensors into load groups.
pub fn group(tensors: impl IntoIterator<Item = (u32, Destination, u64)>) -> Vec<LoadGroup> {
    let mut m: std::collections::BTreeMap<(u32, Destination), (usize, u64)> = Default::default();
    for (t, d, b) in tensors {
        let e = m.entry((t, d)).or_insert((0, 0));
        e.0 += 1;
        e.1 += b;
    }
    let mut v: Vec<_> = m.into_iter()
        .map(|((ggml_type, destination), (tensors, bytes))|
            LoadGroup { ggml_type, destination, tensors, bytes })
        .collect();
    v.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    v
}

#[cfg(test)]
mod group_tests {
    use super::*;

    #[test]
    fn HUNDREDS_of_tensors_become_a_handful_of_groups() {
        // Cluster, do not enumerate. A group is large enough to measure and small enough to tune as
        // one decision.
        let t: Vec<_> = (0..400u64)
            .map(|i| (if i % 3 == 0 { 12u32 } else { 21 },
                      if i % 5 == 0 { Destination::Streamed } else { Destination::Resident },
                      1_000_000))
            .collect();
        assert!(group(t).len() <= 6);
    }

    #[test]
    fn a_STREAMED_group_can_outweigh_a_much_larger_RESIDENT_one() {
        // The thing no per-tensor view surfaces: streamed bytes move once per selection.
        let streamed = LoadGroup { ggml_type: 12, destination: Destination::Streamed,
                                   tensors: 10, bytes: 2 << 30 };
        let resident = LoadGroup { ggml_type: 12, destination: Destination::Resident,
                                   tensors: 100, bytes: 20 << 30 };
        assert!(streamed.bus_bytes(1000, 0.33) > resident.bus_bytes(1000, 0.33));
    }

    #[test]
    fn a_RESIDENT_group_costs_its_bytes_ONCE_however_many_tokens_run() {
        let g = LoadGroup { ggml_type: 12, destination: Destination::Resident,
                            tensors: 1, bytes: 1 << 30 };
        assert_eq!(g.bus_bytes(1, 1.0), g.bus_bytes(100_000, 1.0));
    }

    #[test]
    fn a_SMALL_group_is_not_worth_tuning_separately() {
        // Below a twentieth of the load it cannot repay the bookkeeping, which is the whole argument
        // against per-tensor tuning applied one level up.
        let small = LoadGroup { ggml_type: 13, destination: Destination::Resident,
                                tensors: 27, bytes: 100 };
        assert!(!small.worth_tuning(10_000));
    }

    #[test]
    fn groups_are_ordered_by_BYTES_so_the_first_one_is_the_one_to_tune() {
        let g = group(vec![(13, Destination::Resident, 10), (21, Destination::Resident, 5000)]);
        assert_eq!(g[0].ggml_type, 21);
    }
}
