//! ancestor.rs — the **legendary ancestors**: six First Warriors of the great clans
//! of legend, playable champions for the Battleground mode ([`crate::battleground`]).
//!
//! Straight out of the books' deep lore: before the six living clans there were the
//! **great clans of legend** — the lion-clan, the tiger-clan, the leopard-clan — plus
//! the far wanderers the elders only whisper about (the shadow-panther of the
//! moonless woods, the wind-eared caracal of the dry plains, the grey ghost of the
//! high crags). Each ancestor is a philosophy of survival, not just a species: a
//! passive instinct, two learned techniques, and an ultimate that is the legend
//! itself. (This is also why the asset folder has a lion, a white tiger, and a
//! clouded leopard next to the house cats.)
//!
//! Pure data + arithmetic, no world/render dependency, so kits balance-test headless
//! and the same table drives the sim, the HUD, and any future draft screen.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ancestor { Lion, Tiger, Leopard, Panther, Caracal, Puma }

pub const ALL: [Ancestor; 6] =
    [Ancestor::Lion, Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther, Ancestor::Caracal, Ancestor::Puma];

/// What an ability *does* when the sim fires it. One mechanic per kind keeps the
/// sim generic over kits: a kit is numbers, the sim is verbs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verb {
    Strike,   // a burst of damage on the engaged target (power × attack)
    Aoe,      // damage every enemy within `range`
    Dash,     // close `range` toward the target instantly (and strike at `power`)
    Shield,   // absorb `power × attack` damage for `dur` (self + close allies)
    Stealth,  // untargetable / unseen for `dur`
    Heal,     // restore `power × attack` hp to self + close allies
    Fear,     // enemies within `range` turn and flee for `dur`
    Slow,     // enemies within `range` move at half pace for `dur`
}

#[derive(Clone, Copy)]
pub struct AbilitySpec {
    pub name: &'static str,
    pub desc: &'static str,
    pub verb: Verb,
    pub power: f32,   // in multiples of the ancestor's basic attack (0 = pure utility)
    pub range: f32,
    pub dur: f32,     // seconds the effect lasts (0 = instant)
    pub cd: f32,      // cooldown, seconds
}

#[derive(Clone, Copy)]
pub struct Kit {
    pub name: &'static str,          // the ancestor's warrior name
    pub legend: &'static str,        // the clan of legend they founded
    pub philosophy: &'static str,    // the teaching that survives them
    pub passive_name: &'static str,
    pub passive: &'static str,
    pub hp: f32,
    pub attack: f32,                 // damage per second at the claw
    pub speed: f32,
    pub abilities: [AbilitySpec; 2],
    pub ult: AbilitySpec,
}

pub fn kit(a: Ancestor) -> Kit {
    match a {
        // ── Goldenmane — the lion-clan's founder: the pride fights as one ──────
        Ancestor::Lion => Kit {
            name: "Goldenmane", legend: "the lion-clan of legend",
            philosophy: "A warrior alone is a whisker; a pride is a storm.",
            passive_name: "Pride's Warmth",
            passive: "allies fighting beside Goldenmane strike harder",
            hp: 190.0, attack: 11.0, speed: 6.0,
            abilities: [
                AbilitySpec { name: "Sunstrike Pounce", desc: "leap the gap and rake on landing",
                    verb: Verb::Dash, power: 1.6, range: 9.0, dur: 0.0, cd: 8.0 },
                AbilitySpec { name: "Mane of Dawn", desc: "a golden guard-hair shield over the pride",
                    verb: Verb::Shield, power: 2.2, range: 8.0, dur: 4.0, cd: 14.0 },
            ],
            ult: AbilitySpec { name: "The Great Roar", desc: "a roar out of the elder days — every enemy in earshot turns tail",
                verb: Verb::Fear, power: 0.0, range: 14.0, dur: 2.2, cd: 55.0 },
        },
        // ── Duskstripe — the tiger-clan's founder: the night hunts alone ───────
        Ancestor::Tiger => Kit {
            name: "Duskstripe", legend: "the tiger-clan of legend",
            philosophy: "Strike once, from where they never looked.",
            passive_name: "Night's Hunger",
            passive: "unseen or from behind, the first blow bites far deeper",
            hp: 170.0, attack: 13.0, speed: 6.2,
            abilities: [
                AbilitySpec { name: "Shadow Maul", desc: "a heavy double-paw blow",
                    verb: Verb::Strike, power: 2.4, range: 3.0, dur: 0.0, cd: 7.0 },
                AbilitySpec { name: "Stripes in the Reeds", desc: "melt into the undergrowth",
                    verb: Verb::Stealth, power: 0.0, range: 0.0, dur: 3.5, cd: 16.0 },
            ],
            ult: AbilitySpec { name: "Long Shadows", desc: "dusk falls early — nearby enemies grope half-blind and slow",
                verb: Verb::Slow, power: 1.2, range: 13.0, dur: 4.0, cd: 60.0 },
        },
        // ── Rainspots — the leopard-clan's founder: the river feeds the quick ──
        Ancestor::Leopard => Kit {
            name: "Rainspots", legend: "the leopard-clan of legend",
            philosophy: "The river never argues; it goes around, and wins.",
            passive_name: "Riverborn",
            passive: "water is home ground — swim fast, and wounds close in the shallows",
            hp: 160.0, attack: 11.5, speed: 6.8,
            abilities: [
                AbilitySpec { name: "Spotted Blur", desc: "flicker through the enemy, claws trailing",
                    verb: Verb::Dash, power: 1.3, range: 11.0, dur: 0.0, cd: 6.0 },
                AbilitySpec { name: "Fisher's Mercy", desc: "share the day's catch — the taste knits flesh",
                    verb: Verb::Heal, power: 2.6, range: 7.0, dur: 0.0, cd: 15.0 },
            ],
            ult: AbilitySpec { name: "Floodrush", desc: "the river answers — a drowning surge batters everything around",
                verb: Verb::Aoe, power: 3.2, range: 10.0, dur: 0.0, cd: 58.0 },
        },
        // ── Nightpelt — the shadow ancestor of the moonless woods ──────────────
        Ancestor::Panther => Kit {
            name: "Nightpelt", legend: "the shadow-walkers of the moonless woods",
            philosophy: "Never chase. Become where the prey chooses to run.",
            passive_name: "Silent Footfall",
            passive: "moving through shadow leaves no scent and quickens the step",
            hp: 150.0, attack: 12.5, speed: 6.6,
            abilities: [
                AbilitySpec { name: "Pounce of Midnight", desc: "drop onto prey and pin it",
                    verb: Verb::Dash, power: 1.8, range: 10.0, dur: 0.0, cd: 9.0 },
                AbilitySpec { name: "Veilwalker", desc: "the dark closes over you like water",
                    verb: Verb::Stealth, power: 0.0, range: 0.0, dur: 4.5, cd: 15.0 },
            ],
            ult: AbilitySpec { name: "Eclipse Hunt", desc: "the moon goes out — everything near is struck from the dark",
                verb: Verb::Aoe, power: 2.8, range: 11.0, dur: 0.0, cd: 62.0 },
        },
        // ── Windclaw — the sky-hunter of the dry plains ────────────────────────
        Ancestor::Caracal => Kit {
            name: "Windclaw", legend: "the wind-eared hunters of the dry plains",
            philosophy: "The sky itself can be hunted.",
            passive_name: "Skyward Hunger",
            passive: "every kill quickens the paws — speed stacks with the tally",
            hp: 145.0, attack: 10.5, speed: 7.4,
            abilities: [
                AbilitySpec { name: "Sky Snap", desc: "a vertical snatch that plucks the quick out of the air",
                    verb: Verb::Strike, power: 2.0, range: 4.5, dur: 0.0, cd: 6.5 },
                AbilitySpec { name: "Sand Dash", desc: "a dust-raising burst clean through the line",
                    verb: Verb::Dash, power: 1.1, range: 13.0, dur: 0.0, cd: 5.5 },
            ],
            ult: AbilitySpec { name: "Hundred Leaps", desc: "a blur of bounds — everything nearby is cuffed and left staggering",
                verb: Verb::Slow, power: 1.8, range: 9.0, dur: 3.0, cd: 56.0 },
        },
        // ── Stonewhisper — the grey ghost of the high crags ────────────────────
        Ancestor::Puma => Kit {
            name: "Stonewhisper", legend: "the grey ghosts of the high crags",
            philosophy: "The mountain does not hurry, and the mountain is never late.",
            passive_name: "Sure-Footed",
            passive: "slopes and scree cost nothing; blows barely shift those paws",
            hp: 210.0, attack: 10.0, speed: 5.6,
            abilities: [
                AbilitySpec { name: "Crag Leap", desc: "a boulder-to-boulder leap ending in a landslide of a landing",
                    verb: Verb::Dash, power: 1.5, range: 8.0, dur: 0.0, cd: 9.0 },
                AbilitySpec { name: "Stone Patience", desc: "settle like bedrock — blows glance away",
                    verb: Verb::Shield, power: 2.8, range: 5.0, dur: 5.0, cd: 16.0 },
            ],
            ult: AbilitySpec { name: "The Crags Answer", desc: "the scree comes down — everything near is buried and battered",
                verb: Verb::Aoe, power: 3.0, range: 9.0, dur: 0.0, cd: 60.0 },
        },
    }
}

impl Ancestor {
    pub fn name(self) -> &'static str { kit(self).name }
}

/// A single balance number per kit so tuning drift is caught by a test, in the
/// spirit of clan.rs's trait-sum check: hp is worth its healing burden, attack its
/// threat, speed its map control. Roughly commensurate scales.
pub fn budget(a: Ancestor) -> f32 {
    let k = kit(a);
    k.hp / 30.0 + k.attack + k.speed * 1.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ancestor_has_a_full_kit() {
        for a in ALL {
            let k = kit(a);
            assert!(!k.name.is_empty() && !k.philosophy.is_empty());
            for ab in k.abilities.iter().chain(std::iter::once(&k.ult)) {
                assert!(ab.cd > 0.0, "{}: {} has no cooldown", k.name, ab.name);
                // an ability either deals/absorbs/heals (power) or has a lasting effect
                assert!(ab.power > 0.0 || ab.dur > 0.0, "{}: {} does nothing", k.name, ab.name);
            }
            // an ultimate should be a *legend*: much rarer than the techniques
            assert!(k.ult.cd >= 3.0 * k.abilities[0].cd.max(k.abilities[1].cd) / 2.0,
                "{}: ult cd {} too short", k.name, k.ult.cd);
        }
    }

    #[test]
    fn kits_are_balanced_tradeoffs() {
        // no ancestor is strong at everything — the budget stays inside a band
        for a in ALL {
            let b = budget(a);
            assert!(b > 25.0 && b < 30.0, "{} off-budget: {b}", kit(a).name);
        }
        // and the archetypes are real: the tank out-lives, the skirmisher outruns
        assert!(ALL.iter().all(|&a| kit(Ancestor::Puma).hp >= kit(a).hp));
        assert!(ALL.iter().all(|&a| kit(Ancestor::Caracal).speed >= kit(a).speed));
        assert!(ALL.iter().all(|&a| kit(Ancestor::Tiger).attack >= kit(a).attack));
    }

    #[test]
    fn roles_diverge_by_verbs() {
        let verbs = |a: Ancestor| {
            let k = kit(a);
            [k.abilities[0].verb, k.abilities[1].verb, k.ult.verb]
        };
        // the two stealth kits are the ambushers; the two shield kits hold the line
        assert!(verbs(Ancestor::Tiger).contains(&Verb::Stealth));
        assert!(verbs(Ancestor::Panther).contains(&Verb::Stealth));
        assert!(verbs(Ancestor::Lion).contains(&Verb::Shield));
        assert!(verbs(Ancestor::Puma).contains(&Verb::Shield));
        // only the river kit heals others
        assert_eq!(ALL.iter().filter(|&&a| verbs(a).contains(&Verb::Heal)).count(), 1);
    }
}
