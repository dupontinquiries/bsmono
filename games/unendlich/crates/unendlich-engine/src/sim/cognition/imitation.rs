//! Observational behavior copying — culture by eyesight.
//!
//! A creature that can see a neighbor may copy what it's doing. The clearest and
//! most useful case is alarm contagion: see a neighbor freeze and bolt, and you
//! bolt too — which is how a flock scatters as one from a single bird's tell,
//! even though only that one bird actually saw the threat. The decision is
//! deterministic, no dice: whether you catch a neighbor's alarm depends on
//! whether it's visible (the caller filters that), on *your* shyness (the caution
//! gene — a jumpy animal spooks at anything), and on how credible the neighbor is
//! as a model (a confident, bold neighbor is worth heeding; a perpetual
//! panicker less so).
//!
//! This is the social layer over [`Disposition`] (the genome) and `Sentinel`
//! posture (the reveal): one creature's perception becomes the whole group's.

use crate::sim::fixed::Fixed;
use crate::sim::personality::Disposition;
use crate::sim::sentinel::Posture;

/// Combined shyness + credibility needed to catch a neighbor's alarm.
fn adopt_threshold() -> Fixed {
    Fixed::from_frac(60, 100)
}

fn posture_rank(p: Posture) -> u8 {
    match p {
        Posture::Calm => 0,
        Posture::Wary => 1,
        Posture::Alarmed => 2,
    }
}

/// A visible neighbor, as seen by the observer.
#[derive(Debug, Clone, Copy)]
pub struct Neighbor {
    pub posture: Posture,
    /// How compelling a model this neighbor is (`[0,1]`) — see [`credibility`].
    pub rank: Fixed,
}

/// How worth-copying a creature is: confident, bold animals (high aggression, low
/// caution) are heeded; timid ones are ignored. `(aggression + (1 − caution))/2`.
pub fn credibility(d: &Disposition) -> Fixed {
    (d.aggression + (Fixed::ONE - d.caution)) * Fixed::from_frac(1, 2)
}

/// Whether an observer with this `caution` would catch alarm from a neighbor of
/// the given `rank`: shyness and the model's credibility add — either a jumpy
/// animal or a believable neighbor will do it.
pub fn would_adopt(caution: Fixed, rank: Fixed) -> bool {
    caution + rank >= adopt_threshold()
}

/// The observer's posture after watching its visible neighbors. Contagion only
/// *escalates* (a calmer neighbor never soothes you out of your own alarm): if
/// the most-alarmed visible neighbor is more alarmed than you and you'd adopt
/// from it (shyness + its credibility clear the bar), you take on its posture.
pub fn adopt_posture(self_posture: Posture, self_disp: &Disposition, visible: &[Neighbor]) -> Posture {
    // The most alarmed neighbor; ties broken toward the more credible one.
    let leader = visible.iter().copied().reduce(|a, b| {
        match posture_rank(b.posture).cmp(&posture_rank(a.posture)) {
            std::cmp::Ordering::Greater => b,
            std::cmp::Ordering::Less => a,
            std::cmp::Ordering::Equal => if b.rank > a.rank { b } else { a },
        }
    });
    if let Some(n) = leader {
        if posture_rank(n.posture) > posture_rank(self_posture) && would_adopt(self_disp.caution, n.rank) {
            return n.posture;
        }
    }
    self_posture
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disp(caution: i64) -> Disposition {
        Disposition {
            aggression: Fixed::from_frac(30, 100),
            territoriality: Fixed::from_frac(20, 100),
            caution: Fixed::from_frac(caution, 100),
            curiosity: Fixed::from_frac(20, 100),
            greed: Fixed::from_frac(20, 100),
        }
    }
    fn nb(posture: Posture, rank: i64) -> Neighbor {
        Neighbor { posture, rank: Fixed::from_frac(rank, 100) }
    }

    #[test]
    fn a_shy_creature_catches_a_neighbors_alarm() {
        let shy = disp(80);
        // Even a low-credibility panicker spooks a jumpy animal.
        let out = adopt_posture(Posture::Calm, &shy, &[nb(Posture::Alarmed, 10)]);
        assert_eq!(out, Posture::Alarmed);
    }

    #[test]
    fn a_bold_creature_ignores_a_lone_panicker_but_heeds_a_credible_one() {
        let bold = disp(10);
        // A twitchy low-rank neighbor doesn't move a bold animal.
        assert_eq!(adopt_posture(Posture::Calm, &bold, &[nb(Posture::Alarmed, 20)]), Posture::Calm);
        // But a confident, credible neighbor's alarm is believed.
        assert_eq!(adopt_posture(Posture::Calm, &bold, &[nb(Posture::Alarmed, 90)]), Posture::Alarmed);
    }

    #[test]
    fn contagion_only_escalates() {
        let shy = disp(90);
        // An alarmed observer is not calmed by a calm neighbor.
        assert_eq!(adopt_posture(Posture::Alarmed, &shy, &[nb(Posture::Calm, 90)]), Posture::Alarmed);
    }

    #[test]
    fn no_visible_neighbors_changes_nothing() {
        let shy = disp(90);
        assert_eq!(adopt_posture(Posture::Wary, &shy, &[]), Posture::Wary);
    }

    #[test]
    fn the_most_alarmed_credible_neighbor_leads() {
        let mid = disp(40);
        // Picks the Alarmed one over the Wary one as the model.
        let out = adopt_posture(Posture::Calm, &mid, &[nb(Posture::Wary, 90), nb(Posture::Alarmed, 30)]);
        assert_eq!(out, Posture::Alarmed);
    }

    #[test]
    fn credibility_rewards_bold_confidence() {
        let bold = Disposition {
            aggression: Fixed::from_frac(90, 100),
            territoriality: Fixed::from_frac(50, 100),
            caution: Fixed::from_frac(10, 100),
            curiosity: Fixed::from_frac(50, 100),
            greed: Fixed::from_frac(50, 100),
        };
        let timid = disp(90);
        assert!(credibility(&bold) > credibility(&timid), "a bold animal is a more compelling model");
    }
}
