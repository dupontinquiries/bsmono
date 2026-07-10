//! `sim::stance_culture` — the social lifecycle of player-authored fighting stances.
//!
//! From `02_Bending_and_Disciplines/bending_fighting_culture.docx`. The bending
//! *grammar* is physical (environment, gesture, resonance — fixed by the world).
//! The fighting *culture* is social: a **stance** is a player-named sequence of
//! moves from the existing vocabulary, invocable in one breath. Players name what
//! they do repeatedly, pass those names to each other, and some names outlast the
//! players who coined them. Culture is not designed top-down — it emerges from
//! practice.
//!
//! This module is the lifecycle: a stance is coined **Nascent**; when another
//! player learns it, it **Spreads**; once enough players practise it, the name
//! escapes its origin onto the Global Dojo board (**Canon**, adoptable by anyone);
//! sustained heavy use makes it **Renowned**; and a stance that falls out of use —
//! or a Nascent stance forgotten before it was ever shared — becomes **Dead**. The
//! forgotten-Nascent case is the doc's only irreversible loss.

/// Where a named stance sits in the culture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StanceLife {
    /// Coined but never shared — exists only in the coiner's memory.
    Nascent,
    /// Learned by at least one other player; propagating.
    Spreading,
    /// Crossed the adoption threshold onto the Global Dojo board — adoptable by
    /// any player, whether or not they ever met the author.
    Canon,
    /// Canon *and* heavily practised across the world.
    Renowned,
    /// Fell out of use, or a Nascent stance the coiner forgot before sharing.
    Dead,
}

/// A player-authored named stance and its cultural standing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamedStance {
    /// Player who coined it.
    pub coiner_id: u32,
    /// Total invocations across all practitioners — the renown proxy.
    pub uses: u32,
    /// Distinct players who have adopted it (beyond the coiner).
    pub adopters: u32,
    /// Whether the coiner still remembers it (a forgotten unshared stance is lost).
    pub coiner_remembers: bool,
}

impl NamedStance {
    /// Coin a fresh stance — Nascent, remembered only by its author.
    pub fn coin(coiner_id: u32) -> NamedStance {
        NamedStance { coiner_id, uses: 0, adopters: 0, coiner_remembers: true }
    }

    /// Record one invocation (by anyone). Renown accrues with use.
    pub fn record_use(&mut self) {
        self.uses = self.uses.saturating_add(1);
    }

    /// Another player learns the stance — it spreads.
    pub fn adopt(&mut self) {
        self.adopters = self.adopters.saturating_add(1);
    }

    /// The coiner forgets it. Irreversible loss *only* if it was never shared.
    pub fn forget_by_coiner(&mut self) {
        self.coiner_remembers = false;
    }

    /// Current life stage, given the world's thresholds: `canon_adopters` distinct
    /// adopters to reach the Global Dojo board, and `renowned_uses` total uses to
    /// be Renowned. Note `renowned_uses` is a *relative* bar — the doc lets the
    /// world's use-threshold rise as the corpus grows, so a once-Renowned stance
    /// can slip back to Canon.
    pub fn status(&self, canon_adopters: u32, renowned_uses: u32) -> StanceLife {
        // Forgotten before it ever spread → gone for good.
        if self.adopters == 0 && !self.coiner_remembers {
            return StanceLife::Dead;
        }
        if self.adopters >= canon_adopters {
            if self.uses >= renowned_uses {
                StanceLife::Renowned
            } else {
                StanceLife::Canon
            }
        } else if self.adopters >= 1 {
            StanceLife::Spreading
        } else {
            StanceLife::Nascent
        }
    }

    /// Has this stance escaped its author (reached Canon or beyond)? Once true, it
    /// survives the coiner forgetting it — the name outlasts the player.
    pub fn is_shared_culture(&self, canon_adopters: u32) -> bool {
        self.adopters >= canon_adopters
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANON: u32 = 5; // adopters to reach the Global Dojo board
    const RENOWN: u32 = 1000; // uses to be Renowned

    #[test]
    fn a_coined_stance_is_nascent() {
        let s = NamedStance::coin(42);
        assert_eq!(s.status(CANON, RENOWN), StanceLife::Nascent);
    }

    #[test]
    fn forgotten_nascent_is_the_only_irreversible_loss() {
        let mut s = NamedStance::coin(42);
        s.forget_by_coiner(); // never shared, then forgotten
        assert_eq!(s.status(CANON, RENOWN), StanceLife::Dead);
    }

    #[test]
    fn one_adopter_starts_it_spreading() {
        let mut s = NamedStance::coin(42);
        s.adopt();
        assert_eq!(s.status(CANON, RENOWN), StanceLife::Spreading);
    }

    #[test]
    fn crossing_adoption_threshold_makes_it_canon() {
        let mut s = NamedStance::coin(42);
        for _ in 0..CANON {
            s.adopt();
        }
        assert_eq!(s.status(CANON, RENOWN), StanceLife::Canon);
        // Canon survives the coiner forgetting — the name has outlasted the author.
        assert!(s.is_shared_culture(CANON));
        s.forget_by_coiner();
        assert_eq!(s.status(CANON, RENOWN), StanceLife::Canon);
    }

    #[test]
    fn heavy_use_makes_canon_renowned_and_can_decay_back() {
        let mut s = NamedStance::coin(42);
        for _ in 0..CANON {
            s.adopt();
        }
        s.uses = 1200;
        assert_eq!(s.status(CANON, RENOWN), StanceLife::Renowned);
        // The world's use-threshold rises (corpus grew) → slips back to Canon.
        assert_eq!(s.status(CANON, 5000), StanceLife::Canon);
    }

    #[test]
    fn lifecycle_ordering() {
        assert!(StanceLife::Nascent < StanceLife::Spreading);
        assert!(StanceLife::Spreading < StanceLife::Canon);
        assert!(StanceLife::Canon < StanceLife::Renowned);
    }
}
