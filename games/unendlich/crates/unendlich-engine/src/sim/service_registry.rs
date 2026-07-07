//! Service registry — the "wire many at once" mechanism for stateful services.
//!
//! Most orphaned sim modules are *pure kernels* that just need a caller; they do
//! not belong here. The minority that are **stateful services** (a struct with
//! `new` + a per-tick `update`/`tick`/`step`) register themselves into the
//! [`SERVICES`] distributed slice, and [`ServiceHost`] instantiates and ticks all
//! of them with no hand-edited composition. `linkme` makes registration survive
//! dead-code elimination, so a service that nothing else references still links in
//! by virtue of being declared — which is exactly the property the 402 orphans
//! lacked.
//!
//! Scaling property: adding a wired service is O(1) — write the service, write a
//! small adapter that maps [`ServiceCtx`] onto its `tick`, and drop a
//! `#[distributed_slice(SERVICES)]` entry (here, or co-located in the service's own
//! file — see `stamina` for the co-located form). No edit to `App` or to a central
//! list. The [`tests::registry_wires_and_ticks_all_services`] smoke test then
//! verifies the *wiring* of every registered service at once (registers, ticks,
//! does not panic). Per-service *behavior* is still proven by that service's own
//! harness.

use linkme::distributed_slice;

/// Per-tick context handed to every service. Intentionally minimal for now (just
/// `dt`); it grows as services need shared signals (world refs, input, a
/// blackboard). Services pull what they need and ignore the rest.
#[derive(Clone, Copy, Debug)]
pub struct ServiceCtx {
    /// Seconds since the last tick.
    pub dt: f32,
}

/// A stateful service the host owns and ticks each frame. One uniform shape over
/// the heterogeneous service structs (each gets a thin adapter that impls this).
pub trait Service: Send {
    /// Stable identifier (for logs, the smoke test, and debugging).
    fn name(&self) -> &'static str;
    /// Advance one tick.
    fn tick(&mut self, ctx: &ServiceCtx);
}

/// A registration: a name + a constructor for the service. Collected at link time.
pub struct ServiceEntry {
    pub name: &'static str,
    pub make: fn() -> Box<dyn Service>,
}

/// Every registered service. Entries are added via `#[distributed_slice(SERVICES)]`
/// from anywhere in the crate (this module, or a service's own file).
#[distributed_slice]
pub static SERVICES: [ServiceEntry] = [..];

/// Owns one instance of every registered service and ticks them together.
pub struct ServiceHost {
    services: Vec<Box<dyn Service>>,
}

impl ServiceHost {
    /// Instantiate every registered service.
    pub fn from_registry() -> ServiceHost {
        let mut services: Vec<Box<dyn Service>> = SERVICES.iter().map(|e| (e.make)()).collect();
        // deterministic order by name (link order is unspecified)
        services.sort_by_key(|s| s.name());
        ServiceHost { services }
    }

    /// Tick every service once.
    pub fn tick_all(&mut self, ctx: &ServiceCtx) {
        for s in &mut self.services {
            s.tick(ctx);
        }
    }

    pub fn len(&self) -> usize {
        self.services.len()
    }

    pub fn is_empty(&self) -> bool {
        self.services.is_empty()
    }

    /// Names of all live services (sorted).
    pub fn names(&self) -> Vec<&'static str> {
        self.services.iter().map(|s| s.name()).collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Central registrations (the simple form). Co-located registrations live in the
// service's own file — see `crate::sim::stamina` for that pattern.
// ─────────────────────────────────────────────────────────────────────────────

use crate::sim::affliction::Afflicted;
use crate::sim::camera_tremble::TrembleState;
use crate::sim::dragon_thirst::Hydration;

struct TrembleService(TrembleState);
impl Service for TrembleService {
    fn name(&self) -> &'static str {
        "camera_tremble"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        let _ = self.0.step(ctx.dt);
    }
}
#[distributed_slice(SERVICES)]
static TREMBLE: ServiceEntry = ServiceEntry {
    name: "camera_tremble",
    make: || Box::new(TrembleService(TrembleState::new())),
};

struct ThirstService(Hydration);
impl Service for ThirstService {
    fn name(&self) -> &'static str {
        "dragon_thirst"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        self.0.tick(ctx.dt, 0.0); // neutral exertion until wired to a dragon body
    }
}
#[distributed_slice(SERVICES)]
static THIRST: ServiceEntry = ServiceEntry {
    name: "dragon_thirst",
    make: || Box::new(ThirstService(Hydration::new(1.0))),
};

struct AfflictionService(Afflicted);
impl Service for AfflictionService {
    fn name(&self) -> &'static str {
        "affliction"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        let _ = self.0.tick();
    }
}
#[distributed_slice(SERVICES)]
static AFFLICTION: ServiceEntry = ServiceEntry {
    name: "affliction",
    make: || Box::new(AfflictionService(Afflicted::new())),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_wires_and_ticks_all_services() {
        let mut host = ServiceHost::from_registry();
        // Every registered service instantiated (including the co-located stamina).
        assert!(host.len() >= 4, "expected >=4 registered services, got {}: {:?}", host.len(), host.names());
        // Tick the whole registry for ~2s of frames — proves wiring is panic-free.
        let ctx = ServiceCtx { dt: 1.0 / 60.0 };
        for _ in 0..120 {
            host.tick_all(&ctx);
        }
        // names are unique + sorted
        let names = host.names();
        for w in names.windows(2) {
            assert!(w[0] <= w[1], "names should be sorted");
        }
    }
}
