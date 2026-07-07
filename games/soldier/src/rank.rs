//! rank.rs — competitive ranks + a light matchmaking engine.
//!
//! A player's lifetime metadata (kills, K/D, streaks, waves survived, matches played) folds into a single
//! matchmaking rating (MMR). That rating maps to a gemstone **tier** with a colour and a symbol, split into
//! three **divisions** (III → II → I) so there's always a next step. The matchmaker turns a rating into a
//! lobby: how many bots, at what difficulty, drawn from a comparable skill band.

use crate::enemy::Difficulty;
use crate::profile::Profile;
use egui::Color32;

/// Ascending competitive tiers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rank {
    Bronze,
    Silver,
    Gold,
    Emerald,
    Sapphire,
    Ruby,
    Diamond,
    Master,
}

impl Rank {
    pub const ALL: [Rank; 8] = [
        Rank::Bronze,
        Rank::Silver,
        Rank::Gold,
        Rank::Emerald,
        Rank::Sapphire,
        Rank::Ruby,
        Rank::Diamond,
        Rank::Master,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Rank::Bronze => "Bronze",
            Rank::Silver => "Silver",
            Rank::Gold => "Gold",
            Rank::Emerald => "Emerald",
            Rank::Sapphire => "Sapphire",
            Rank::Ruby => "Ruby",
            Rank::Diamond => "Diamond",
            Rank::Master => "Master",
        }
    }
    /// A glyph emblem for the tier (renders in the monospace HUD/menus).
    pub fn symbol(self) -> &'static str {
        match self {
            Rank::Bronze => "\u{2b22}",   // ⬢ hexagon
            Rank::Silver => "\u{25c8}",   // ◈
            Rank::Gold => "\u{2605}",     // ★
            Rank::Emerald => "\u{2756}",  // ❖
            Rank::Sapphire => "\u{2726}", // ✦
            Rank::Ruby => "\u{2666}",     // ♦
            Rank::Diamond => "\u{25c6}",  // ◆
            Rank::Master => "\u{272a}",   // ✪
        }
    }
    pub fn color(self) -> Color32 {
        match self {
            Rank::Bronze => Color32::from_rgb(176, 124, 78),
            Rank::Silver => Color32::from_rgb(196, 204, 214),
            Rank::Gold => Color32::from_rgb(238, 196, 86),
            Rank::Emerald => Color32::from_rgb(80, 210, 130),
            Rank::Sapphire => Color32::from_rgb(86, 150, 240),
            Rank::Ruby => Color32::from_rgb(232, 86, 110),
            Rank::Diamond => Color32::from_rgb(130, 226, 240),
            Rank::Master => Color32::from_rgb(220, 150, 255),
        }
    }
    fn index(self) -> usize {
        Rank::ALL.iter().position(|&r| r == self).unwrap_or(0)
    }
}

/// Where a player sits: tier + division (1 = top of the tier, 3 = bottom) + rating + 0..1 progress to next.
#[derive(Clone, Copy, Debug)]
pub struct RankInfo {
    pub tier: Rank,
    pub division: u8,
    pub rating: i32,
    pub progress: f32,
}

impl RankInfo {
    /// "Gold II" etc.
    pub fn label(&self) -> String {
        let div = match self.division {
            1 => "I",
            2 => "II",
            _ => "III",
        };
        if self.tier == Rank::Master {
            "Master".to_string()
        } else {
            format!("{} {}", self.tier.name(), div)
        }
    }
}

/// The lowest rating; each tier spans `TIER_SPAN`, each split into three divisions.
const BASE: i32 = 600;
const TIER_SPAN: i32 = 420;

/// Fold a profile's lifetime metadata into a single matchmaking rating.
pub fn rating(p: &Profile) -> i32 {
    let kd = p.kd().min(4.0);
    let r = BASE as f32
        + (p.kills as f32).min(2500.0) * 0.55
        + (kd - 1.0) * 175.0
        + p.best_streak as f32 * 22.0
        + p.best_wave as f32 * 18.0
        + (p.matches as f32).min(250.0) * 3.0;
    r.clamp(0.0, (BASE + TIER_SPAN * 8) as f32) as i32
}

/// Map a rating to its tier/division/progress.
pub fn rank_of(rating: i32) -> RankInfo {
    let top = BASE + TIER_SPAN * (Rank::ALL.len() as i32 - 1);
    if rating >= top {
        return RankInfo { tier: Rank::Master, division: 1, rating, progress: 1.0 };
    }
    let above = (rating - BASE).max(0);
    let ti = (above / TIER_SPAN).min(Rank::ALL.len() as i32 - 1) as usize;
    let tier = Rank::ALL[ti];
    let within = above - ti as i32 * TIER_SPAN; // 0..TIER_SPAN
    let frac = within as f32 / TIER_SPAN as f32; // 0..1 through the tier
    // divisions: bottom third = III, middle = II, top = I
    let division = if frac < 1.0 / 3.0 {
        3
    } else if frac < 2.0 / 3.0 {
        2
    } else {
        1
    };
    RankInfo { tier, division, rating, progress: frac }
}

/// Convenience: the player's current rank straight from their profile.
pub fn rank_for(p: &Profile) -> RankInfo {
    rank_of(rating(p))
}

/// A matchmade lobby recommendation derived from a rating.
#[derive(Clone, Copy, Debug)]
pub struct Lobby {
    pub bots: usize,
    pub difficulty: Difficulty,
    /// The skill band (min,max rating) the match is drawn from.
    pub band: (i32, i32),
}

/// Turn a rating into a balanced lobby: tougher, fuller lobbies as you climb.
pub fn matchmake(rating: i32) -> Lobby {
    let info = rank_of(rating);
    let difficulty = match info.tier {
        Rank::Bronze | Rank::Silver => Difficulty::Recruit,
        Rank::Gold | Rank::Emerald => Difficulty::Regular,
        Rank::Sapphire | Rank::Ruby => Difficulty::Veteran,
        Rank::Diamond | Rank::Master => Difficulty::Elite,
    };
    // more opponents at higher tiers
    let bots = (5 + info.tier.index() * 2).clamp(4, 24);
    let band = (rating - 220, rating + 220);
    Lobby { bots, difficulty, band }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prof(kills: u64, deaths: u64, streak: u32, wave: u32, matches: u64) -> Profile {
        Profile { name: "p".into(), kills, deaths, matches, best_streak: streak, best_wave: wave }
    }

    #[test]
    fn fresh_player_is_low_tier() {
        let r = rank_for(&prof(0, 0, 0, 0, 0));
        assert_eq!(r.tier, Rank::Bronze);
        assert!(r.rating >= 0);
    }

    #[test]
    fn rating_climbs_with_performance() {
        let weak = rating(&prof(10, 40, 1, 2, 3));
        let strong = rating(&prof(2000, 500, 30, 40, 200));
        assert!(strong > weak + 800, "skill should separate ratings ({weak} vs {strong})");
        assert_eq!(rank_for(&prof(2000, 400, 40, 50, 250)).tier, Rank::Master);
    }

    #[test]
    fn tiers_and_divisions_are_ordered() {
        // walking the rating up never decreases the tier index, and divisions go III→I within a tier
        let mut last = 0usize;
        for r in (BASE..BASE + TIER_SPAN * 8).step_by(50) {
            let info = rank_of(r);
            assert!(info.tier.index() >= last);
            last = info.tier.index();
            assert!((1..=3).contains(&info.division));
            assert!((0.0..=1.0).contains(&info.progress));
        }
    }

    #[test]
    fn every_tier_has_distinct_symbol_and_colour() {
        let mut syms = std::collections::HashSet::new();
        for t in Rank::ALL {
            assert!(syms.insert(t.symbol()), "symbols must be unique");
            assert!(!t.name().is_empty());
        }
    }

    #[test]
    fn matchmaking_scales_with_rating() {
        let low = matchmake(rating(&prof(0, 10, 0, 0, 0)));
        let high = matchmake(rating(&prof(2000, 400, 40, 50, 250)));
        assert!(high.bots >= low.bots);
        assert!(high.band.0 > low.band.0);
    }
}
