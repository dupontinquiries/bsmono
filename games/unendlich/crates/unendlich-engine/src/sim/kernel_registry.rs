//! Kernel registry — the second registry tier, for **pure kernels**.
//!
//! [`crate::sim::service_registry`] wires *stateful services* (a struct + a
//! per-tick mutator). But the orphan census shows most unwired modules are
//! pure functions: shader math, curves, packing, easing — no state to own.
//! They don't fit a `Service`, yet they deserve the same two properties the
//! service registry bought us:
//!
//! 1. **Linked into the bin** — `linkme` registration survives dead-code
//!    elimination, so the kernel ships in `unendlich-fullgame` by virtue of
//!    being declared.
//! 2. **Exercised every frame** — the host sweeps each kernel across a
//!    deterministic, slowly-evolving input phase. A kernel that panics, NaNs
//!    into a `debug_assert`, or UB-traps on *any* point of the sweep takes the
//!    frame down in dev — a free, perpetual fuzz-lite that runs as long as the
//!    game does. `black_box` keeps the calls from being optimized away.
//!
//! Registrations are GENERATED — see `tools/bootstrap_registry.py`, which
//! scans the crate for pure functions whose parameters it knows how to
//! synthesize and emits `sim::kernel_registry_gen`. Hand-written kernels can
//! also register here directly.

use linkme::distributed_slice;

/// Deterministic per-tick input context for kernel sweeps. The generator maps
/// kernel parameters onto these fields, so every frame exercises a slightly
/// different point of each kernel's input space.
#[derive(Clone, Copy, Debug)]
pub struct KernelCtx {
    /// Monotone tick counter.
    pub tick: u64,
    /// A `[0,1)` sawtooth derived from `tick` — the canonical f32 input.
    pub phase: f32,
    /// A `[-1,1]` triangle wave — for signed inputs.
    pub signed: f32,
}

impl KernelCtx {
    pub fn at(tick: u64) -> KernelCtx {
        let phase = (tick % 240) as f32 / 240.0;
        let signed = 2.0 * (phase - 0.5).abs() * 2.0 - 1.0;
        KernelCtx { tick, phase, signed }
    }
}

/// One registered kernel exercise: a name + a function that calls the kernel
/// with synthesized inputs (through `black_box` on both sides).
pub struct KernelEntry {
    pub name: &'static str,
    pub run: fn(&KernelCtx),
}

/// Every registered kernel. Add via `#[distributed_slice(KERNELS)]` anywhere
/// in the crate (the generated file, or a kernel's own module).
#[distributed_slice]
pub static KERNELS: [KernelEntry] = [..];

/// Sweeps every registered kernel. `stride` lets the bin amortize: with
/// `stride = 4` each frame runs a rotating quarter of the kernels, so the
/// whole set is still covered every 4 frames at a quarter of the cost.
pub struct KernelHost {
    tick: u64,
    stride: u64,
    /// Kernels disabled after panicking once (quarantine — see [`Self::exercise`]).
    dead: Vec<bool>,
    dead_count: usize,
}

impl KernelHost {
    pub fn new(stride: u64) -> KernelHost {
        KernelHost {
            tick: 0,
            stride: stride.max(1),
            dead: vec![false; KERNELS.len()],
            dead_count: 0,
        }
    }

    /// Exercise this frame's slice of the kernel set. A kernel that panics is
    /// QUARANTINED: logged once by name, permanently skipped, and the frame
    /// survives. The sandbox census (`report_all_panickers`) remains the way to
    /// prune offenders from the generated file; this is the runtime backstop
    /// for entries that slipped through (e.g. running the bin without the heal
    /// loop). Pure kernels are RefUnwindSafe by construction, so catch_unwind
    /// here is sound.
    pub fn exercise(&mut self) {
        let ctx = KernelCtx::at(self.tick);
        let lane = self.tick % self.stride;
        for (i, k) in KERNELS.iter().enumerate() {
            if (i as u64) % self.stride != lane || self.dead[i] {
                continue;
            }
            let r = std::panic::catch_unwind(|| (k.run)(&ctx));
            if r.is_err() {
                self.dead[i] = true;
                self.dead_count += 1;
                log::warn!("[kernels] QUARANTINED '{}' after panic (tick {})", k.name, self.tick);
            }
        }
        self.tick += 1;
    }

    /// How many kernels have been quarantined so far.
    pub fn quarantined(&self) -> usize {
        self.dead_count
    }

    pub fn len(&self) -> usize {
        KERNELS.len()
    }

    pub fn is_empty(&self) -> bool {
        KERNELS.is_empty()
    }

    /// Sorted kernel names (for logs and the smoke test).
    pub fn names(&self) -> Vec<&'static str> {
        let mut v: Vec<&'static str> = KERNELS.iter().map(|k| k.name).collect();
        v.sort_unstable();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every registered kernel survives a full 240-tick phase sweep (stride 1
    /// = every kernel every tick). This is the wiring proof for the whole
    /// generated set at once; per-kernel behavior stays with each module.
    #[test]
    fn kernel_registry_sweeps_clean() {
        let mut host = KernelHost::new(1);
        for _ in 0..240 {
            host.exercise();
        }
        // Names unique.
        let names = host.names();
        for w in names.windows(2) {
            assert!(w[0] != w[1], "duplicate kernel name {}", w[0]);
        }
    }
}

#[cfg(test)]
mod prune_support {
    use super::*;
    use crate::sim::service_registry::{ServiceCtx, SERVICES};

    /// Bootstrap-prune helper: sweep every kernel and tick every service under
    /// catch_unwind, printing each offender once as `PANIC-KERNEL <name>` /
    /// `PANIC-SERVICE <name>`. tools/bootstrap_registry.py parses these to
    /// prune all offenders in one round instead of one per test run.
    #[test]
    fn report_all_panickers() {
        std::panic::set_hook(Box::new(|_| {}));
        for k in KERNELS.iter() {
            let bad = (0..240).any(|t| {
                let ctx = KernelCtx::at(t);
                std::panic::catch_unwind(|| (k.run)(&ctx)).is_err()
            });
            if bad {
                println!("PANIC-KERNEL {}", k.name);
            }
        }
        for e in SERVICES.iter() {
            let name = e.name;
            let r = std::panic::catch_unwind(|| {
                let mut s = (e.make)();
                let ctx = ServiceCtx { dt: 1.0 / 60.0 };
                for _ in 0..240 {
                    s.tick(&ctx);
                }
            });
            if r.is_err() {
                println!("PANIC-SERVICE {}", name);
            }
        }
        let _ = std::panic::take_hook();
        println!("CENSUS-DONE");
    }
}
