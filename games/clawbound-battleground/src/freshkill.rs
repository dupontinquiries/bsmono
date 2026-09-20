//! freshkill — prey caught, carried, piled, and eaten before it turns.
//!
//! > *"We'd better hunt on our way back," he told them. "That way we can
//! > pretend we went out to restock the fresh-kill pile."*
//!
//! The brief calls it a meta: *"hunt early before being chased and stock up
//! fresh kill for later"*. That only works if the pile is an **economy** —
//! something you invest in when it is safe and draw down when it is not.
//! `wildkit`'s `forage_*` modules are the other thing: a creature feeding
//! itself, moment to moment. Nothing there carries prey home.
//!
//! ## Four states, and the interesting one is carrying
//!
//! Caught → carried → piled → eaten. Carrying is where the tension lives,
//! because **a cat's jaws are full**. You cannot carry two mice and a squirrel
//! home; you cannot carry anything and fight. Hollyleaf catches her mouse on
//! the way back for a reason, and the reason is that she was not carrying
//! anything on the way out.
//!
//! ## Prey turns
//!
//! A pile you can hoard forever is not a decision, it is a counter. Prey
//! spoils, so stockpiling has a ceiling and *when* you hunt matters as much as
//! whether. The spoilage curve is deliberately not the scent exponential:
//! scent fades smoothly toward nothing, and prey is good, then getting old,
//! then not food. Stages, not a slope.
//!
//! ## What this does NOT do
//!
//! It does not heal anyone. Eating returns a nourishment value and the caller
//! decides what that is worth, because healing rates belong to the combat
//! model and a food module that knew them would have an opinion about damage.

/// What was caught. Size is the whole difference: a mouse is a snack, a
/// squirrel feeds a cat, a rabbit feeds several.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prey {
    Mouse,
    Vole,
    Squirrel,
    Bird,
    Rabbit,
}

impl Prey {
    /// Nourishment when perfectly fresh.
    pub fn nourishment(self) -> f32 {
        match self {
            Prey::Mouse => 0.25,
            Prey::Vole => 0.3,
            Prey::Bird => 0.5,
            Prey::Squirrel => 0.6,
            Prey::Rabbit => 1.0,
        }
    }

    /// How much of a cat's carry capacity it takes.
    ///
    /// A rabbit is the whole mouth. This is what stops a successful hunt from
    /// being a shopping trip.
    pub fn bulk(self) -> f32 {
        match self {
            // 0.33, not 0.34: three must actually fit. At 0.34 the third is
            // refused at 1.02 against a capacity of 1.0, so "a cat can carry
            // three mice" would have been a comment contradicted by the code.
            Prey::Mouse | Prey::Vole => 0.33,
            Prey::Bird => 0.5,
            Prey::Squirrel => 0.6,
            Prey::Rabbit => 1.0,
        }
    }

    /// Seconds before it is no longer worth eating. Smaller prey turns faster.
    pub fn keeps_for_s(self) -> f32 {
        match self {
            Prey::Mouse | Prey::Vole => 1.0 * DAY,
            Prey::Bird => 1.5 * DAY,
            Prey::Squirrel => 2.0 * DAY,
            Prey::Rabbit => 2.0 * DAY,
        }
    }
}

const DAY: f32 = 24.0 * 3600.0;

/// How good a piece of prey still is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Killed recently. Full value.
    Fresh,
    /// Past its best. Edible, worth less.
    Stale,
    /// Not food. Hollyleaf's mouse *"looked ready to die of old age"* — this
    /// is the state after that.
    Spoiled,
}

/// Fraction of `keeps_for_s` after which prey is Stale rather than Fresh.
pub const STALE_AT: f32 = 0.5;

/// What a Stale piece is worth, as a fraction of fresh.
///
/// Not a smooth ramp: prey is good, then it is getting old, then it is not
/// food. A continuous decay would make the pile's value a slowly leaking
/// number nobody could plan against, and planning is the point.
pub const STALE_VALUE: f32 = 0.5;

/// One piece of prey.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Kill {
    pub prey: Prey,
    /// When it was caught, seconds.
    pub caught_at_s: f32,
}

impl Kill {
    pub fn new(prey: Prey, now_s: f32) -> Kill {
        Kill { prey, caught_at_s: now_s }
    }

    pub fn age_s(&self, now_s: f32) -> f32 {
        (now_s - self.caught_at_s).max(0.0)
    }

    pub fn freshness(&self, now_s: f32) -> Freshness {
        let keeps = self.prey.keeps_for_s();
        let age = self.age_s(now_s);
        if age >= keeps {
            Freshness::Spoiled
        } else if age >= keeps * STALE_AT {
            Freshness::Stale
        } else {
            Freshness::Fresh
        }
    }

    /// Nourishment right now. Zero once spoiled.
    pub fn value(&self, now_s: f32) -> f32 {
        match self.freshness(now_s) {
            Freshness::Fresh => self.prey.nourishment(),
            Freshness::Stale => self.prey.nourishment() * STALE_VALUE,
            Freshness::Spoiled => 0.0,
        }
    }
}

/// What a cat can carry at once. One rabbit, or three mice.
pub const CARRY_CAPACITY: f32 = 1.0;

/// What a cat is carrying in its jaws.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Jaws {
    held: Vec<Kill>,
}

impl Jaws {
    pub fn new() -> Jaws {
        Jaws::default()
    }
    pub fn held(&self) -> &[Kill] {
        &self.held
    }
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
    pub fn load(&self) -> f32 {
        self.held.iter().map(|k| k.prey.bulk()).sum()
    }

    /// Can this be picked up without dropping something?
    pub fn has_room_for(&self, prey: Prey) -> bool {
        self.load() + prey.bulk() <= CARRY_CAPACITY + 1e-4
    }

    /// Pick up a kill. Refused rather than silently dropping what is already
    /// held — losing a squirrel to pick up a mouse should be the player's
    /// decision, not a side effect.
    pub fn take(&mut self, kill: Kill) -> bool {
        if !self.has_room_for(kill.prey) {
            return false;
        }
        self.held.push(kill);
        true
    }

    /// Full jaws cannot fight or call.
    ///
    /// The tension the carrying state exists for. A patrol carrying prey home
    /// is a patrol that cannot challenge an intruder without dropping dinner.
    pub fn encumbered(&self) -> bool {
        !self.held.is_empty()
    }

    /// Drop everything — what happens when a fight starts.
    pub fn drop_all(&mut self) -> Vec<Kill> {
        std::mem::take(&mut self.held)
    }
}

/// The clan's fresh-kill pile.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pile {
    kills: Vec<Kill>,
}

impl Pile {
    pub fn new() -> Pile {
        Pile::default()
    }
    pub fn len(&self) -> usize {
        self.kills.len()
    }
    pub fn is_empty(&self) -> bool {
        self.kills.is_empty()
    }
    pub fn all(&self) -> &[Kill] {
        &self.kills
    }

    /// Add everything a cat carried home.
    pub fn deposit(&mut self, kills: Vec<Kill>) {
        self.kills.extend(kills);
    }

    /// Total nourishment available right now. What a leader looks at before
    /// deciding whether to send out another patrol.
    pub fn value(&self, now_s: f32) -> f32 {
        self.kills.iter().map(|k| k.value(now_s)).sum()
    }

    /// Take the piece that should be eaten next: the OLDEST still-edible one.
    ///
    /// Oldest first, not best first. Eating the freshest leaves the stale
    /// pieces to spoil, which throws away food a clan already paid a hunt for
    /// — and "eat it before it turns" is the actual rule a pile teaches.
    pub fn take_next(&mut self, now_s: f32) -> Option<Kill> {
        let idx = self
            .kills
            .iter()
            .enumerate()
            .filter(|(_, k)| k.freshness(now_s) != Freshness::Spoiled)
            .max_by(|a, b| a.1.age_s(now_s).total_cmp(&b.1.age_s(now_s)))
            .map(|(i, _)| i)?;
        Some(self.kills.remove(idx))
    }

    /// Remove what has turned. Returns how many were thrown out.
    ///
    /// Explicit, like `Markers::prune`: a query should not mutate, and a
    /// clan noticing it wasted three squirrels is information worth surfacing
    /// rather than tidying away.
    pub fn clear_spoiled(&mut self, now_s: f32) -> usize {
        let before = self.kills.len();
        self.kills.retain(|k| k.freshness(now_s) != Freshness::Spoiled);
        before - self.kills.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE CARRYING CONSTRAINT. A successful hunt is not a shopping trip.
    #[test]
    fn a_cat_cannot_carry_a_rabbit_and_anything_else() {
        let mut j = Jaws::new();
        assert!(j.take(Kill::new(Prey::Rabbit, 0.0)));
        assert!(!j.take(Kill::new(Prey::Mouse, 0.0)), "jaws are full");
        assert_eq!(j.held().len(), 1);
    }

    /// Three mice fit; a fourth does not.
    #[test]
    fn small_prey_stacks_up_to_the_same_mouthful() {
        let mut j = Jaws::new();
        for _ in 0..3 {
            assert!(j.take(Kill::new(Prey::Mouse, 0.0)));
        }
        assert!(!j.take(Kill::new(Prey::Mouse, 0.0)));
        assert!((j.load() - 0.99).abs() < 1e-3, "load {}", j.load());
    }

    /// Refused, not silently swapped: losing a squirrel to pick up a mouse
    /// must be a decision, not a side effect.
    #[test]
    fn a_refused_pickup_does_not_drop_what_is_held() {
        let mut j = Jaws::new();
        j.take(Kill::new(Prey::Rabbit, 0.0));
        j.take(Kill::new(Prey::Mouse, 0.0));
        assert_eq!(j.held()[0].prey, Prey::Rabbit, "the rabbit survived");
    }

    /// The tension carrying exists for: a patrol bringing prey home cannot
    /// challenge an intruder without dropping dinner.
    #[test]
    fn carrying_encumbers_and_dropping_frees() {
        let mut j = Jaws::new();
        assert!(!j.encumbered());
        j.take(Kill::new(Prey::Squirrel, 0.0));
        assert!(j.encumbered());
        let dropped = j.drop_all();
        assert_eq!(dropped.len(), 1);
        assert!(!j.encumbered() && j.is_empty());
    }

    /// Stages, not a slope. Prey is good, then old, then not food.
    #[test]
    fn prey_turns_in_stages() {
        let k = Kill::new(Prey::Mouse, 0.0);
        let keeps = Prey::Mouse.keeps_for_s();
        assert_eq!(k.freshness(0.0), Freshness::Fresh);
        assert_eq!(k.freshness(keeps * 0.4), Freshness::Fresh);
        assert_eq!(k.freshness(keeps * 0.6), Freshness::Stale);
        assert_eq!(k.freshness(keeps + 1.0), Freshness::Spoiled);
        assert_eq!(k.value(keeps + 1.0), 0.0, "not food");
        assert!(k.value(keeps * 0.6) < k.value(0.0));
    }

    /// Hollyleaf's mouse, which *"looked ready to die of old age"* -- a pile
    /// you can hoard forever is a counter, not a decision.
    #[test]
    fn a_hoarded_pile_loses_its_value() {
        let mut p = Pile::new();
        p.deposit((0..5).map(|_| Kill::new(Prey::Mouse, 0.0)).collect());
        assert!(p.value(0.0) > 1.0);
        let later = 3.0 * DAY;
        assert_eq!(p.value(later), 0.0, "all of it turned");
        assert_eq!(p.len(), 5, "but the query did not mutate");
        assert_eq!(p.clear_spoiled(later), 5, "and the clan learns it wasted five");
        assert!(p.is_empty());
    }

    /// OLDEST FIRST. Eating the freshest leaves the stale pieces to spoil,
    /// throwing away food the clan already paid a hunt for.
    #[test]
    fn the_pile_serves_the_oldest_edible_piece_first() {
        let mut p = Pile::new();
        let now = 0.6 * DAY;
        p.deposit(vec![
            Kill::new(Prey::Squirrel, now),          // freshest
            Kill::new(Prey::Squirrel, 0.0),          // oldest, still edible
            Kill::new(Prey::Squirrel, now * 0.5),
        ]);
        let first = p.take_next(now).unwrap();
        assert_eq!(first.caught_at_s, 0.0, "the one about to turn");
        assert_eq!(p.len(), 2);
    }

    /// A spoiled piece is never served, even if it is the oldest.
    #[test]
    fn spoiled_prey_is_skipped_rather_than_served() {
        let mut p = Pile::new();
        let now = 2.0 * DAY;
        p.deposit(vec![
            Kill::new(Prey::Mouse, 0.0),       // long spoiled
            Kill::new(Prey::Mouse, now - 100.0), // fresh
        ]);
        let got = p.take_next(now).expect("something edible");
        assert_eq!(got.caught_at_s, now - 100.0);
        assert_eq!(p.len(), 1, "the spoiled one is still there to be cleared");
    }

    /// An empty pile feeds nobody and does not panic.
    #[test]
    fn an_empty_pile_serves_nothing() {
        let mut p = Pile::new();
        assert!(p.take_next(0.0).is_none());
        assert_eq!(p.value(0.0), 0.0);
        assert_eq!(p.clear_spoiled(0.0), 0);
    }

    /// THE META. Hunting early and depositing beats carrying it around,
    /// because jaws hold one mouthful and a pile holds a clan's worth.
    #[test]
    fn the_pile_holds_far_more_than_a_cat_can_carry() {
        let mut j = Jaws::new();
        let mut p = Pile::new();
        let mut trips = 0;
        for _ in 0..4 {
            // Mice, not squirrels: one squirrel is 0.6 of a mouthful, so a
            // trip of squirrels is a trip of ONE and the pile never outgrows
            // the trip count. Four mice fit.
            while j.take(Kill::new(Prey::Mouse, 0.0)) {}
            p.deposit(j.drop_all());
            trips += 1;
        }
        assert_eq!(trips, 4);
        assert!(p.len() > 4, "four trips stocked {} pieces", p.len());
        assert!(p.value(0.0) > CARRY_CAPACITY, "more than one mouthful of food");
    }
}
