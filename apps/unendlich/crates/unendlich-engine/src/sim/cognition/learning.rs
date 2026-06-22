//! Individual learning — try, get rewarded, adapt.
//!
//! The third leg of the adaptive brain, after the heritable genome
//! ([`crate::sim::genetics`]) and cultural copying
//! ([`crate::sim::cognition::imitation`]). A creature keeps a running value
//! estimate for each action it can take; when it acts and sees an outcome it
//! nudges that action's estimate toward the reward. Choosing usually exploits the
//! best-known action, but *explores* a different one now and then — and how often
//! it explores is the **curiosity** gene, so a bold curious lineage discovers good
//! behavior faster and (via breeding) passes that tendency on, while a cautious
//! one sticks to what works.
//!
//! Pure and deterministic: exploration is seeded (by tick / position), so the
//! same situation always yields the same choice and it replays under rollback.

use crate::sim::fixed::Fixed;

fn splitmix(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A learned value estimate per discrete action.
#[derive(Debug, Clone)]
pub struct Policy {
    pub values: Vec<Fixed>,
}

impl Policy {
    /// `n` actions, all initially neutral (zero expected value).
    pub fn new(n: usize) -> Policy {
        Policy { values: vec![Fixed::ZERO; n] }
    }

    /// The current best-known action (argmax of value; ties → lowest index).
    pub fn best(&self) -> usize {
        let mut bi = 0;
        for i in 1..self.values.len() {
            if self.values[i] > self.values[bi] {
                bi = i;
            }
        }
        bi
    }

    /// Choose an action this tick: usually the best known, but with probability
    /// `curiosity` explore a deterministic pseudo-random action instead. `seed`
    /// should vary per decision (tick, position) so exploration isn't frozen.
    pub fn choose(&self, curiosity: Fixed, seed: u64) -> usize {
        if self.values.is_empty() {
            return 0;
        }
        let roll = splitmix(seed);
        // roll's low bits → a fraction in [0,1) compared against curiosity.
        let frac = Fixed::from_frac((roll % 1000) as i64, 1000);
        if frac < curiosity {
            (splitmix(roll) % self.values.len() as u64) as usize
        } else {
            self.best()
        }
    }

    /// Update after taking `action` and observing `reward`: move that action's
    /// estimate a `rate` fraction toward the reward (a running average / TD(0)
    /// step), so consistently-rewarded actions rise to the top.
    pub fn reinforce(&mut self, action: usize, reward: Fixed, rate: Fixed) {
        if let Some(v) = self.values.get_mut(action) {
            *v = *v + (reward - *v) * rate;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(num: i64) -> Fixed {
        Fixed::from_frac(num, 100)
    }

    #[test]
    fn reinforce_moves_an_estimate_toward_observed_reward() {
        let mut p = Policy::new(3);
        for _ in 0..50 {
            p.reinforce(1, Fixed::ONE, f(30));
        }
        // Action 1's estimate should approach 1.0; the others stay at 0.
        assert!(p.values[1] > f(95), "estimate converges to reward: {:?}", p.values[1].to_f32());
        assert_eq!(p.values[0], Fixed::ZERO);
        assert_eq!(p.best(), 1);
    }

    #[test]
    fn a_consistently_rewarded_action_gets_chosen() {
        let mut p = Policy::new(4);
        // Action 2 pays off; learn it, then exploit (no curiosity).
        for _ in 0..40 {
            p.reinforce(2, Fixed::ONE, f(25));
        }
        for seed in 0..50u64 {
            assert_eq!(p.choose(Fixed::ZERO, seed), 2, "with no curiosity it exploits the best");
        }
    }

    #[test]
    fn curiosity_drives_exploration() {
        let mut p = Policy::new(4);
        for _ in 0..40 {
            p.reinforce(0, Fixed::ONE, f(25)); // action 0 is "best"
        }
        // No curiosity → always the best.
        let exploit: std::collections::HashSet<_> = (0..200u64).map(|s| p.choose(Fixed::ZERO, s)).collect();
        assert_eq!(exploit.len(), 1, "zero curiosity never explores");
        // High curiosity → tries the other actions too.
        let explore: std::collections::HashSet<_> = (0..200u64).map(|s| p.choose(f(90), s)).collect();
        assert!(explore.len() > 1, "high curiosity explores alternatives");
    }

    #[test]
    fn choosing_is_deterministic() {
        let mut p = Policy::new(4);
        p.reinforce(1, f(50), f(50));
        for seed in 0..100u64 {
            assert_eq!(p.choose(f(40), seed), p.choose(f(40), seed), "same situation → same choice");
        }
    }

    #[test]
    fn it_relearns_when_the_world_changes() {
        let mut p = Policy::new(2);
        // Action 0 was good…
        for _ in 0..30 {
            p.reinforce(0, Fixed::ONE, f(30));
        }
        assert_eq!(p.best(), 0);
        // …then stops paying off while action 1 starts to.
        for _ in 0..30 {
            p.reinforce(0, Fixed::ZERO, f(30));
            p.reinforce(1, Fixed::ONE, f(30));
        }
        assert_eq!(p.best(), 1, "the policy adapts to the new payoff");
    }
}
