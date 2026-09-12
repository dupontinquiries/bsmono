//! ragdoll_service — the service-registry registration for `sim::ragdoll`,
//! split out so the solver file stays free of `linkme`/`crate::` and can be
//! compiled alone by the rig harness. Behaviour unchanged.

use super::ragdoll::{step, Node, Stick};
// ── service_registry wiring (S2) ───────────────────────────────────
// ragdoll exposes a free `step(nodes, sticks, ...)`; the service owns a
// minimal two-node, one-stick chain over flat ground and relaxes it each
// tick. Proves the solver wiring is panic-free.
use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use linkme::distributed_slice;

struct RagdollService {
    nodes: Vec<Node>,
    sticks: Vec<Stick>,
}
impl Service for RagdollService {
    fn name(&self) -> &'static str {
        "ragdoll"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        step(&mut self.nodes, &self.sticks, 0.0, ctx.dt, 4);
    }
}
#[distributed_slice(SERVICES)]
static RAGDOLL_SERVICE: ServiceEntry = ServiceEntry {
    name: "ragdoll",
    make: || {
        let nodes = vec![Node::new([0.0, 2.0, 0.0], 0.25), Node::new([0.0, 1.0, 0.0], 0.25)];
        let sticks = vec![Stick::between(&nodes, 0, 1)];
        Box::new(RagdollService { nodes, sticks })
    },
};
