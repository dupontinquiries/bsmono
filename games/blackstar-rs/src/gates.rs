//! GATES — buy-a-door map expansion for Escalation, ported from the C
//! engine's `exengine/gates.h`/`gates.c`.
//!
//! Black-Ops-Zombies-style, but Cybertron: the run starts in a small
//! arena, and each gate — once you can afford it — opens permanently
//! and pushes the playable perimeter outward for the rest of the run,
//! so the encounter has more room and the spawns range further.
//!
//! Pure logic, same boundary as the C original states in its own
//! header: this module owns the gate chain and the arithmetic; the
//! game supplies the valence wallet (`escalation::Waves::valence`)
//! and applies the returned spawn extent to its own arena bounds.
//! Nothing here places a wall, draws a gate, or knows what an arena
//! IS — same honest split as `escalation.rs` and `enemy.rs`.
//!
//! Numbers are the C engine's own: three gates (300/750/1500 valence,
//! opening to 100/150/220 m half-extent) over a 60 m starting arena.

pub const GATE_COUNT: usize = 3;

#[derive(Clone, Debug)]
pub struct Gate {
    /// valence required to open
    pub cost: i64,
    /// spawn half-extent once open (replaces the current, if bigger)
    pub extent: f32,
    pub opened: bool,
    pub name: &'static str,
}

/// The escalating causeway/wing/outer-rim gate chain, same three
/// stops and numbers as the C original.
pub fn init() -> [Gate; GATE_COUNT] {
    [
        Gate { cost: 300, extent: 100.0, opened: false, name: "the outer causeway" },
        Gate { cost: 750, extent: 150.0, opened: false, name: "the reclaimed foundry wing" },
        Gate { cost: 1500, extent: 220.0, opened: false, name: "the outer rim causeway" },
    ]
}

/// Spend `gate.cost` from `*valence` if affordable and the gate isn't
/// already open. Returns whether it opened; `valence` is deducted
/// only on success, unchanged otherwise (including the
/// already-open case — no double-spend).
pub fn try_open(gate: &mut Gate, valence: &mut i64) -> bool {
    if gate.opened {
        return false;
    }
    if *valence < gate.cost {
        return false;
    }
    *valence -= gate.cost;
    gate.opened = true;
    true
}

/// Index of the cheapest still-locked gate, or `None` if all are
/// open (for UI: "press N to open the next gate"). Gates are checked
/// in chain order, which is cost order for this chain, same as the
/// C original's plain linear scan.
pub fn next_locked(gates: &[Gate; GATE_COUNT]) -> Option<usize> {
    gates.iter().position(|g| !g.opened)
}

/// The current spawn half-extent: the largest OPENED gate's extent,
/// or 60 (the starting arena) if none are open yet.
pub fn extent(gates: &[Gate; GATE_COUNT]) -> f32 {
    let mut ext = 60.0; // starting arena half-extent
    for g in gates {
        if g.opened && g.extent > ext {
            ext = g.extent;
        }
    }
    ext
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported directly from games/blackstar/tests/test_gates.c, case for
    // case.
    #[test]
    fn the_gate_chain_matches_the_c_reference() {
        let mut gates = init();
        assert_eq!(extent(&gates), 60.0, "nothing open yet should be the base arena");
        assert_eq!(next_locked(&gates), Some(0));

        let mut wallet: i64 = 100;
        assert!(!try_open(&mut gates[0], &mut wallet), "300-cost gate opened for 100");
        assert_eq!(wallet, 100);
        assert!(!gates[0].opened);

        wallet = 300;
        assert!(try_open(&mut gates[0], &mut wallet), "exact cost should open the gate");
        assert_eq!(wallet, 0);
        assert!(gates[0].opened);
        assert_eq!(extent(&gates), gates[0].extent);
        assert_eq!(next_locked(&gates), Some(1));

        assert!(!try_open(&mut gates[0], &mut wallet), "an already-open gate double-spent");
        assert_eq!(wallet, 0);

        wallet = 750;
        assert!(try_open(&mut gates[1], &mut wallet));
        assert_eq!(wallet, 0);
        assert_eq!(extent(&gates), gates[1].extent, "the bigger opened gate should win");

        wallet = 1500;
        assert!(try_open(&mut gates[2], &mut wallet));
        assert_eq!(extent(&gates), gates[2].extent);
        assert_eq!(next_locked(&gates), None, "all three should be open");
    }

    #[test]
    fn opening_out_of_order_still_reports_the_largest_extent() {
        // the C reference only exercises opening in cost order; this
        // checks the "bigger wins" rule holds the other way too, since
        // extent() takes the max over ALL opened gates, not just the
        // most-recently-opened one
        let mut gates = init();
        let mut wallet: i64 = 1500;
        assert!(try_open(&mut gates[2], &mut wallet)); // open the biggest first
        assert_eq!(extent(&gates), gates[2].extent);
        wallet = 300;
        assert!(try_open(&mut gates[0], &mut wallet)); // then the smallest
        assert_eq!(
            extent(&gates),
            gates[2].extent,
            "opening a smaller gate after a bigger one should not shrink the extent"
        );
    }

    #[test]
    fn insufficient_valence_never_partially_spends() {
        let mut gates = init();
        let mut wallet: i64 = 299; // one short of gate 0's 300 cost
        assert!(!try_open(&mut gates[0], &mut wallet));
        assert_eq!(wallet, 299, "a failed open must not touch the wallet at all");
    }
}
