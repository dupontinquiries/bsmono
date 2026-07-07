//! `sim::batch_perception` — a batched perception update over many agents
//! sharing one stimulus source, with a scalar reference for parity.
//!
//! Each agent senses one [`Source`] and produces an alertness in `0..1` that
//! grows with the source's intensity and the agent's sensitivity and falls off
//! with squared distance. [`perceive_one`] is the readable reference;
//! [`perceive_batch`] runs the whole herd in one call. Both route through the
//! same `#[inline]` `kernel`, so the batch is **bit-for-bit identical** to the
//! scalar path — the microopt is purely in the call/iteration shape, never in
//! the arithmetic.
//!
//! Perf shape: agents are read as a contiguous slice and written into a
//! contiguous `Vec` in index order with no cross-agent dependencies, so the
//! loop is trivially vectorizable and cache-friendly (one streaming pass, one
//! shared `Source` kept in registers) — the win a per-agent virtual call or
//! scattered layout would forfeit.
//!
//! Pure and deterministic: no clock, no randomness, no cross-imports.

/// A sensing agent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Agent {
    /// World position.
    pub pos: [f32; 2],
    /// How keenly it senses, `0..1`-ish (clamped into the result).
    pub sensitivity: f32,
}

/// A shared stimulus all agents perceive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Source {
    /// World position of the stimulus.
    pub pos: [f32; 2],
    /// Strength of the stimulus.
    pub intensity: f32,
}

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// The shared arithmetic. Both the scalar and batch paths call exactly this,
/// guaranteeing identical floating-point results.
#[inline]
fn kernel(a: &Agent, s: &Source) -> f32 {
    let dx = a.pos[0] - s.pos[0];
    let dy = a.pos[1] - s.pos[1];
    let d2 = dx * dx + dy * dy;
    clamp01(s.intensity * a.sensitivity / (1.0 + d2))
}

/// Scalar reference: one agent's alertness toward the source.
pub fn perceive_one(a: &Agent, s: &Source) -> f32 {
    kernel(a, s)
}

/// Batched: alertness for every agent toward the shared source, in order.
pub fn perceive_batch(agents: &[Agent], s: &Source) -> Vec<f32> {
    crate::util::batch_kernel::map_collect(agents, |a| kernel(a, s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rf(seed: &mut u32) -> f32 {
        (lcg(seed) % 200_001) as f32 / 1000.0 - 100.0
    }
    fn rand_agent(seed: &mut u32) -> Agent {
        Agent {
            pos: [rf(seed), rf(seed)],
            sensitivity: (lcg(seed) % 100_001) as f32 / 100_000.0,
        }
    }
    fn rand_source(seed: &mut u32) -> Source {
        Source {
            pos: [rf(seed), rf(seed)],
            intensity: (lcg(seed) % 100_001) as f32 / 50_000.0,
        }
    }

    #[test]
    fn batch_matches_scalar_bit_for_bit() {
        let mut seed = 0xBA7C_u32;
        for _ in 0..2000 {
            let s = rand_source(&mut seed);
            let n = (lcg(&mut seed) % 12) as usize;
            let mut agents = Vec::new();
            for _ in 0..n {
                agents.push(rand_agent(&mut seed));
            }
            let batch = perceive_batch(&agents, &s);
            assert_eq!(batch.len(), agents.len());
            for i in 0..agents.len() {
                // exact bit equality (to_bits) — not approximate
                assert_eq!(batch[i].to_bits(), perceive_one(&agents[i], &s).to_bits());
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        let s = Source {
            pos: [0.0, 0.0],
            intensity: 1.0,
        };
        assert!(perceive_batch(&[], &s).is_empty());
    }

    #[test]
    fn order_independent_per_agent() {
        let mut seed = 0x0DE6_u32;
        let s = rand_source(&mut seed);
        let mut agents = Vec::new();
        for _ in 0..8 {
            agents.push(rand_agent(&mut seed));
        }
        let forward = perceive_batch(&agents, &s);
        let mut rev = agents.clone();
        rev.reverse();
        let backward = perceive_batch(&rev, &s);
        // reversing the inputs reverses the outputs, value-for-value
        let n = forward.len();
        for i in 0..n {
            assert_eq!(forward[i].to_bits(), backward[n - 1 - i].to_bits());
        }
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let s = rand_source(&mut seed);
            let a = rand_agent(&mut seed);
            let v = perceive_one(&a, &s);
            assert!(v >= 0.0 && v <= 1.0, "alertness {}", v);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..2000 {
            let s = rand_source(&mut seed);
            let n = (lcg(&mut seed) % 8) as usize;
            let mut agents = Vec::new();
            for _ in 0..n {
                agents.push(rand_agent(&mut seed));
            }
            assert_eq!(perceive_batch(&agents, &s), perceive_batch(&agents, &s));
        }
    }

    #[test]
    fn closer_or_stronger_means_more_alert() {
        let s = Source {
            pos: [0.0, 0.0],
            intensity: 1.0,
        };
        let near = Agent {
            pos: [1.0, 0.0],
            sensitivity: 1.0,
        };
        let far = Agent {
            pos: [10.0, 0.0],
            sensitivity: 1.0,
        };
        assert!(perceive_one(&near, &s) > perceive_one(&far, &s));
    }
}
