//! rules -- a custom game's match rules: who may play whom (`picks`), and
//! the numbers a normal match fixes -- starting level and gold, minions per
//! wave, cat health, cooldowns, XP, respawn.
//!
//! [`MatchRules`] is one plain value. The sim reads it (`Game::rules`, set by
//! `Game::apply_rules`) at the few places each number lives; everything
//! else about it -- the custom-game screen's sliders, the `--rules` launch
//! flag, the one-line summary -- is driven by [`KNOBS`], so a new knob is
//! one row there plus the line in the sim that reads it.
//!
//! [`MatchRules::encode`] / [`MatchRules::parse`] are its text form
//! (`level=6 gold=5000 cd=0.5`): the launch flag uses it now, and it is
//! what a match-setup message from a LAN host would carry -- the LAN lobby
//! does not start a synced match yet (`lanlobby`), so today the rules apply
//! to matches against bots.
//!
//! Headless: the screen is `ui::draw_custom`, the wiring `app`.

use crate::picks::{Fill, GodLimit};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatchRules {
    pub gods: GodLimit,
    /// Under no limit: every bot plays the player's ancestor.
    pub mirror: bool,
    /// Every cat starts at this level (its ability ranks with it).
    pub start_level: u32,
    pub start_gold: f32,
    /// Minions in each lane's wave, times this (0: no waves at all).
    pub minion_mul: f32,
    /// Every cat's max health, times this.
    pub hp_mul: f32,
    /// Every ability's cooldown, times this (0.5: back twice as fast).
    pub cd_mul: f32,
    pub xp_mul: f32,
    pub respawn_mul: f32,
}

impl Default for MatchRules {
    fn default() -> Self {
        MatchRules {
            gods: GodLimit::Lobby,
            mirror: false,
            start_level: 1,
            start_gold: crate::consts::START_GOLD,
            minion_mul: 1.0,
            hp_mul: 1.0,
            cd_mul: 1.0,
            xp_mul: 1.0,
            respawn_mul: 1.0,
        }
    }
}

/// How a knob's value reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Unit {
    Level,
    Gold,
    Times,
}

/// One numeric rule: its key in the text form, its label, its range and
/// step, and where it lives in [`MatchRules`].
pub struct Knob {
    pub key: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub unit: Unit,
    pub get: fn(&MatchRules) -> f32,
    pub set: fn(&mut MatchRules, f32),
}

impl Knob {
    /// `v` in range and on a step.
    pub fn fit(&self, v: f32) -> f32 {
        let v = v.clamp(self.min, self.max);
        (self.min + ((v - self.min) / self.step).round() * self.step).clamp(self.min, self.max)
    }

    /// The value as the screen shows it.
    pub fn show(&self, v: f32) -> String {
        match self.unit {
            Unit::Level => format!("{}", v as u32),
            Unit::Gold => format!("{}", v as u32),
            Unit::Times => format!("x{}", trim(v)),
        }
    }
}

fn trim(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Every numeric rule, in the order the screen lists them.
pub const KNOBS: &[Knob] = &[
    Knob { key: "level", label: "Starting level", min: 1.0, max: crate::ranks::MAX as f32, step: 1.0, unit: Unit::Level, get: |r| r.start_level as f32, set: |r, v| r.start_level = v as u32 },
    Knob { key: "gold", label: "Starting gold", min: 0.0, max: 20_000.0, step: 100.0, unit: Unit::Gold, get: |r| r.start_gold, set: |r, v| r.start_gold = v },
    Knob { key: "minions", label: "Minions per wave", min: 0.0, max: 4.0, step: 0.25, unit: Unit::Times, get: |r| r.minion_mul, set: |r, v| r.minion_mul = v },
    Knob { key: "hp", label: "Cat health", min: 0.25, max: 5.0, step: 0.25, unit: Unit::Times, get: |r| r.hp_mul, set: |r, v| r.hp_mul = v },
    Knob { key: "cd", label: "Cooldowns", min: 0.1, max: 3.0, step: 0.05, unit: Unit::Times, get: |r| r.cd_mul, set: |r, v| r.cd_mul = v },
    Knob { key: "xp", label: "Experience", min: 0.0, max: 5.0, step: 0.25, unit: Unit::Times, get: |r| r.xp_mul, set: |r, v| r.xp_mul = v },
    // (not 0: a cat with no respawn wait is never counted dead, and would
    // stand in the lane at 0 health -- systems' respawn needs a wait > 0)
    Knob { key: "respawn", label: "Respawn time", min: 0.25, max: 3.0, step: 0.25, unit: Unit::Times, get: |r| r.respawn_mul, set: |r, v| r.respawn_mul = v },
];

impl MatchRules {
    pub fn fill(&self) -> Fill {
        if self.gods == GodLimit::Open && self.mirror {
            Fill::Mirror
        } else {
            Fill::Random
        }
    }

    pub fn is_default(&self) -> bool {
        *self == MatchRules::default()
    }

    /// A wave of `melee` + `casters` as these rules field it.
    pub fn wave(&self, (melee, casters): (usize, usize)) -> (usize, usize) {
        let m = |n: usize| (n as f32 * self.minion_mul).round() as usize;
        (m(melee), m(casters))
    }

    /// The text form: only what differs from a normal match, space
    /// separated `key=value` (empty for a normal match).
    pub fn encode(&self) -> String {
        let d = MatchRules::default();
        let mut out = Vec::new();
        if self.gods != d.gods {
            out.push(format!("gods={}", self.gods.key()));
        }
        if self.mirror {
            out.push("mirror=1".to_string());
        }
        for k in KNOBS {
            let v = (k.get)(self);
            if (v - (k.get)(&d)).abs() > 1e-4 {
                out.push(format!("{}={}", k.key, trim(v)));
            }
        }
        out.join(" ")
    }

    /// Read the text form onto `self` (keys not named keep their value);
    /// values are fitted to their knob's range and step. An unknown key or
    /// an unreadable value is an error, naming it.
    pub fn parse_onto(&mut self, text: &str) -> Result<(), String> {
        for part in text.split(|c: char| c.is_whitespace() || c == ',').filter(|p| !p.is_empty()) {
            let (key, val) = part.split_once('=').ok_or_else(|| format!("'{part}' is not key=value"))?;
            match key {
                "gods" => self.gods = GodLimit::from_key(val).ok_or_else(|| format!("gods: '{val}' is not lobby, team or open"))?,
                "mirror" => self.mirror = matches!(val, "1" | "true" | "on" | "yes"),
                _ => {
                    let k = KNOBS.iter().find(|k| k.key == key).ok_or_else(|| format!("no rule called '{key}'"))?;
                    let v: f32 = val.parse().map_err(|_| format!("{key}: '{val}' is not a number"))?;
                    (k.set)(self, k.fit(v));
                }
            }
        }
        if self.mirror {
            self.gods = GodLimit::Open;
        }
        Ok(())
    }

    pub fn parse(text: &str) -> Result<MatchRules, String> {
        let mut r = MatchRules::default();
        r.parse_onto(text)?;
        Ok(r)
    }

    /// One line for the select screen: what is custom about this match.
    pub fn summary(&self) -> String {
        let d = MatchRules::default();
        let mut out = Vec::new();
        if self.gods != d.gods || self.mirror {
            out.push(if self.mirror { "every cat yours".to_string() } else { self.gods.name().to_lowercase() });
        }
        for k in KNOBS {
            let v = (k.get)(self);
            if (v - (k.get)(&d)).abs() > 1e-4 {
                out.push(format!("{} {}", k.label.to_lowercase(), k.show(v)));
            }
        }
        if out.is_empty() {
            "standard rules".into()
        } else {
            out.join(" · ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_form_round_trips_and_a_normal_match_is_empty() {
        assert_eq!(MatchRules::default().encode(), "");
        assert!(MatchRules::parse("").unwrap().is_default());
        let r = MatchRules::parse("gods=team level=6 gold=5000 minions=2 hp=1.5 cd=0.5 xp=2 respawn=0.5").unwrap();
        assert_eq!((r.gods, r.start_level, r.start_gold, r.minion_mul, r.hp_mul, r.cd_mul, r.xp_mul, r.respawn_mul), (GodLimit::Team, 6, 5000.0, 2.0, 1.5, 0.5, 2.0, 0.5));
        assert_eq!(MatchRules::parse(&r.encode()).unwrap(), r);
        // every knob round-trips on its own, off its default
        for k in KNOBS {
            let mut r = MatchRules::default();
            let v = if (k.get)(&r) == k.max { k.min } else { k.max };
            (k.set)(&mut r, v);
            assert_eq!(MatchRules::parse(&r.encode()).unwrap(), r, "{}", k.key);
            assert!(r.encode().starts_with(k.key), "{}: {}", k.key, r.encode());
        }
    }

    #[test]
    fn values_are_fitted_and_bad_ones_named() {
        let r = MatchRules::parse("level=99 gold=-5 cd=0.52").unwrap();
        assert_eq!(r.start_level, crate::ranks::MAX, "past the top level");
        assert_eq!(r.start_gold, 0.0);
        assert!((r.cd_mul - 0.5).abs() < 1e-5, "onto the step: {}", r.cd_mul);
        assert_eq!(MatchRules::parse("respawn=0").unwrap().respawn_mul, 0.25, "a respawn wait is never zero");
        assert!(MatchRules::parse("speed=2").unwrap_err().contains("speed"));
        assert!(MatchRules::parse("hp=lots").unwrap_err().contains("hp"));
        assert!(MatchRules::parse("gods=some").is_err());
        let m = MatchRules::parse("mirror=1").unwrap();
        assert_eq!((m.gods, m.fill()), (GodLimit::Open, Fill::Mirror), "the mirror is a no-limit fill");
    }

    /// Every knob's default is inside its own range and on its step (or
    /// the screen could not show a normal match).
    #[test]
    fn every_default_is_a_setting() {
        let d = MatchRules::default();
        for k in KNOBS {
            let v = (k.get)(&d);
            assert!((k.fit(v) - v).abs() < 1e-4, "{}: default {v} is not a setting", k.key);
        }
        assert_eq!(d.summary(), "standard rules");
        assert_eq!(MatchRules::parse("level=6 cd=0.5").unwrap().summary(), "starting level 6 · cooldowns x0.5");
    }

    use crate::sim::{Game, TargetRef};

    fn game(rules: &str) -> Game {
        let mut g = Game::new_teams(&[0, 1], &[2, 3]);
        g.apply_rules(MatchRules::parse(rules).unwrap());
        g
    }

    /// A normal match's rules change nothing: applying them to a new game
    /// leaves every cat exactly as it was built.
    #[test]
    fn normal_rules_leave_a_new_game_as_it_was() {
        let a = Game::new_teams(&[0, 1], &[2, 3]);
        let b = game("");
        for (x, y) in a.gods.iter().zip(&b.gods) {
            assert_eq!((x.level, x.xp, x.gold, x.hp, x.max_hp, x.mana, x.max_mana), (y.level, y.xp, y.gold, y.hp, y.max_hp, y.mana, y.max_mana));
        }
    }

    /// Each rule reaches the sim: start level (the ultimate unlocked with
    /// it), gold, health, cooldowns, XP and respawn.
    #[test]
    fn each_rule_changes_the_match() {
        let (n, c) = (game(""), game("level=6 gold=5000 hp=2 cd=0.5 xp=2 respawn=0.5"));
        assert_eq!(c.gods.iter().map(|g| g.level).collect::<Vec<_>>(), vec![6; 4]);
        assert!(c.ability(0, 3).is_some() && n.ability(0, 3).is_none(), "level 6 unlocks the ultimate");
        assert_eq!(c.gods[0].gold, 5000.0, "the player's purse");
        assert!(c.gods.iter().all(|g| g.hp == g.max_hp));
        // a bot makes its opening buy from the new purse: a dearer one
        let spent = |g: &Game, i: usize| g.gods[i].items.iter().map(|&id| crate::items::all()[id as usize].cost).sum::<f32>();
        assert!((c.gods[1].gold + spent(&c, 1) - 5000.0).abs() < 1e-3, "the bot's purse: {} + {}", c.gods[1].gold, spent(&c, 1));
        assert!(spent(&c, 1) > spent(&n, 1), "the opening buy did not use the bigger purse");
        // health: against a normal cat at the same level (the player's,
        // which has bought nothing)
        let mut lv6 = game("level=6");
        assert!((c.gods[0].max_hp / lv6.gods[0].max_hp - 2.0).abs() < 1e-4, "{} vs {}", c.gods[0].max_hp, lv6.gods[0].max_hp);
        // cooldowns: the same cast, half the wait
        let mut c = c;
        let aim = c.gods[0].pos + glam::Vec3::Z * 4.0;
        c.cast(0, 0, aim);
        lv6.cast(0, 0, aim);
        assert!(lv6.gods[0].cds[0] > 0.0, "the cast did not happen");
        assert!((c.gods[0].cds[0] / lv6.gods[0].cds[0] - 0.5).abs() < 1e-4, "{} vs {}", c.gods[0].cds[0], lv6.gods[0].cds[0]);
        // XP: the same grant, twice as much
        let mut n = n;
        n.grant_xp(1, 10.0);
        c.grant_xp(1, 10.0);
        assert!((c.gods[1].xp / n.gods[1].xp - 2.0).abs() < 1e-4);
        // respawn: the same death, half the wait
        for g in [&mut n, &mut c] {
            g.set_level(3, 6);
            g.deal_damage(TargetRef::God(3), 1e6, false, Some(0), 0.0, 0.0);
        }
        assert!(n.gods[3].respawn_t > 0.0, "the cat did not die");
        assert!((c.gods[3].respawn_t / n.gods[3].respawn_t - 0.5).abs() < 1e-4, "{} vs {}", c.gods[3].respawn_t, n.gods[3].respawn_t);
    }

    /// The first wave, on the jungle's three lanes: normal, doubled, none.
    #[test]
    fn the_wave_rule_fields_the_minions() {
        let first = |rules: &str| {
            let mut g = game(rules);
            while g.minions.is_empty() && g.t < crate::consts::WAVE_FIRST + 1.0 {
                g.tick(0.1);
                g.events.clear();
            }
            g.minions.iter().filter(|m| m.clay.is_none()).count()
        };
        let normal = first("");
        assert!(normal > 0);
        assert_eq!(first("minions=2"), 2 * normal);
        assert_eq!(first("minions=0"), 0);
    }

    #[test]
    fn waves_scale_and_zero_is_none() {
        let r = MatchRules::parse("minions=2").unwrap();
        assert_eq!(r.wave((3, 1)), (6, 2));
        assert_eq!(MatchRules::parse("minions=0").unwrap().wave((3, 2)), (0, 0));
        assert_eq!(MatchRules::default().wave((24, 8)), (24, 8));
    }
}
