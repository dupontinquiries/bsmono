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
pub enum Ancestor {
    Lion, Tiger, Leopard, Panther, Caracal, Puma,
    // the deep roster: the whole Felidae line, and further back than that —
    Serval,     // Faryowl, the ranged caterwauler of the singing grasslands
    Lynx,       // Snowhush, the deep-snow controller — roots, chills, the long winter
    Jaguar,     // Riptail, the drowned-forest puller — drags whole fights together
    CaveLion,   // Oldroar, the Ice Age hulk — the more it bleeds, the harder it smashes
    Smilodon,   // Twofang, the saber-toothed executioner of the elder dawn
    SunCat,     // Suneye, the desert sun-priest — the old sun-god fantasy, wild-born
    Suanni,     // Stillsmoke, the ninth son of the storm-dragon — the seated colossus
}

pub const ALL: [Ancestor; 13] = [
    Ancestor::Lion, Ancestor::Tiger, Ancestor::Leopard, Ancestor::Panther, Ancestor::Caracal,
    Ancestor::Puma, Ancestor::Serval, Ancestor::CaveLion, Ancestor::Smilodon, Ancestor::SunCat,
    Ancestor::Lynx, Ancestor::Jaguar, Ancestor::Suanni,
];

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
    Fear,     // enemies within `range` turn and flee for `dur` (damages too if `power` > 0)
    Slow,     // enemies within `range` move at half pace for `dur`
    Knockback,// smash enemies within `range` away from the caster (and hurt them)
    Root,     // hold enemies within `range` fast where they stand for `dur` — they
              // can still claw, but they cannot move (hard crowd control)
    Pull,     // drag enemies within `range` in to the caster (and hurt them) — the
              // setup move: gather a scattered fight into one spot
    Execute,  // a killing strike whose bite grows with the target's missing strength
    Zone,     // scorch the ground for `dur`: enemies inside burn and the veil is seared away
    Rain,     // call sweet rain over the ground for `dur`: allies inside are mended —
              // and so is *wood*: Heal Trees and the Great Tree drink it too, the
              // roster's one way to knit a gnawed sanctuary back together
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
    pub attack: f32,                 // damage per second at the claw (or at the yowl)
    pub speed: f32,
    pub reach: f32,                  // how far the basic attack carries — melee ≈ 2.8; the
                                     // yowlers and sun-priests strike from many lengths away.
                                     // Reach applies to warriors and prey only: no cat ever
                                     // yowled down a tree — structures must be gnawed up close.
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
            hp: 190.0, attack: 11.0, speed: 6.0, reach: 2.8,
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
            hp: 170.0, attack: 13.0, speed: 6.2, reach: 2.8,
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
            hp: 160.0, attack: 11.5, speed: 6.8, reach: 2.8,
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
            hp: 150.0, attack: 12.5, speed: 6.6, reach: 2.8,
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
            hp: 145.0, attack: 10.5, speed: 7.4, reach: 2.8,
            abilities: [
                AbilitySpec { name: "Sky Snap", desc: "a vertical snatch that plucks the quick out of the air",
                    verb: Verb::Strike, power: 2.0, range: 4.5, dur: 0.0, cd: 6.5 },
                AbilitySpec { name: "Sand Dash", desc: "a dust-raising burst clean through the line",
                    verb: Verb::Dash, power: 1.1, range: 13.0, dur: 0.0, cd: 5.5 },
            ],
            ult: AbilitySpec { name: "Hundred Leaps", desc: "a blur of bounds — everything nearby is cuffed and left staggering",
                verb: Verb::Slow, power: 1.8, range: 9.0, dur: 3.0, cd: 56.0 },
        },
        // ── Faryowl — the far-yowler of the singing grasslands ─────────────────
        // A serval: mostly ears, all voice. The first *ranged* ancestor — its yowl
        // carries across the clearing and strikes like a cuffing paw.
        Ancestor::Serval => Kit {
            name: "Faryowl", legend: "the far-yowlers of the singing grasslands",
            philosophy: "Why cross the clearing when your voice already has?",
            passive_name: "The Far Song",
            passive: "a yowl bites deepest from a distance — far strikes land harder",
            hp: 130.0, attack: 11.0, speed: 6.4, reach: 11.0,
            abilities: [
                AbilitySpec { name: "Piercing Yowl", desc: "one note, aimed like a claw",
                    verb: Verb::Strike, power: 1.9, range: 13.0, dur: 0.0, cd: 6.5 },
                AbilitySpec { name: "Wall of Sound", desc: "a caterwaul that flings the close-pressed back",
                    verb: Verb::Knockback, power: 1.0, range: 7.0, dur: 0.0, cd: 11.0 },
            ],
            ult: AbilitySpec { name: "The Great Caterwaul", desc: "the song the elders warn of — everything in earshot breaks and runs bleeding",
                verb: Verb::Fear, power: 1.6, range: 13.0, dur: 1.8, cd: 58.0 },
        },
        // ── Oldroar — the cave-lion of the frozen long-ago ─────────────────────
        // Panthera spelaea, half again the size of any lion living. The hulk of the
        // roster: wound it and you have only made it angrier.
        Ancestor::CaveLion => Kit {
            name: "Oldroar", legend: "the cave-lions of the frozen long-ago",
            philosophy: "The old world did not fall. It is only sleeping in me.",
            passive_name: "Deep Wounds, Deep Fury",
            passive: "every wound feeds the fury — the closer to falling, the harder the blows",
            hp: 240.0, attack: 10.5, speed: 5.4, reach: 2.8,
            abilities: [
                AbilitySpec { name: "Boulder Paw", desc: "a paw like falling stone — the struck are hurled away",
                    verb: Verb::Knockback, power: 1.5, range: 4.5, dur: 0.0, cd: 9.0 },
                AbilitySpec { name: "Shatter Ground", desc: "stamp the earth until it cracks and grips",
                    verb: Verb::Slow, power: 1.0, range: 7.0, dur: 3.0, cd: 12.0 },
            ],
            ult: AbilitySpec { name: "Earthshaker", desc: "the roar and the blow together, as the old world made them",
                verb: Verb::Aoe, power: 3.4, range: 10.0, dur: 0.0, cd: 62.0 },
        },
        // ── Twofang — the saber-tooth of the elder dawn ────────────────────────
        // Smilodon: the fangs that ended arguments a million years before the clans.
        Ancestor::Smilodon => Kit {
            name: "Twofang", legend: "the two-fangs of the elder dawn",
            philosophy: "Claws begin the hunt. The fangs only ever end it.",
            passive_name: "The Saber Finishes",
            passive: "a wounded foe is a finished foe — strikes bite far deeper below half strength",
            hp: 175.0, attack: 12.5, speed: 6.0, reach: 3.2,
            abilities: [
                AbilitySpec { name: "Saber Lunge", desc: "the whole weight behind both fangs",
                    verb: Verb::Dash, power: 1.7, range: 9.0, dur: 0.0, cd: 8.5 },
                AbilitySpec { name: "Hamstring", desc: "one rake low, and the running is over",
                    verb: Verb::Slow, power: 0.8, range: 4.0, dur: 3.5, cd: 9.0 },
            ],
            ult: AbilitySpec { name: "The Killing Bite", desc: "the bite the elder dawn is remembered by",
                verb: Verb::Execute, power: 2.2, range: 3.5, dur: 0.0, cd: 55.0 },
        },
        // ── Suneye — the sun-priest of the endless dunes ───────────────────────
        // The desert wildcat whose line the elders say once *spoke with the sun*.
        // This is the old sun-god fantasy kept wild: no temples, no two-legs —
        // just a clan that watched the sun so long the sun began watching back.
        Ancestor::SunCat => Kit {
            name: "Suneye", legend: "the sun-priests of the endless dunes",
            philosophy: "The night hides many things. The sun keeps none of them.",
            passive_name: "The Sun's Favor",
            passive: "under the open sun every strike burns brighter — the day belongs to Suneye",
            hp: 125.0, attack: 10.8, speed: 6.2, reach: 12.0,
            abilities: [
                AbilitySpec { name: "Sunspear", desc: "a lance of noon light, thrown from the eye",
                    verb: Verb::Strike, power: 1.8, range: 14.0, dur: 0.0, cd: 6.0 },
                AbilitySpec { name: "Noon Blaze", desc: "scorch the ground — nothing hides on burning sand",
                    verb: Verb::Zone, power: 0.9, range: 6.0, dur: 4.0, cd: 14.0 },
            ],
            ult: AbilitySpec { name: "The Eye of the Sun", desc: "the sun looks down in anger — a great burning gaze no veil survives",
                verb: Verb::Zone, power: 1.6, range: 9.0, dur: 6.0, cd: 60.0 },
        },
        // ── Snowhush — the deep-snow lynx of the white silence ─────────────────
        // The dedicated *controller*: everything in this kit holds, chills, or slows.
        // Short cooldowns, modest claws — Snowhush wins by deciding where everyone
        // else is allowed to stand.
        Ancestor::Lynx => Kit {
            name: "Snowhush", legend: "the deep-snow lynxes of the white silence",
            philosophy: "You need not be the strongest in the fight. Only the one who chooses where it happens.",
            passive_name: "Hush of Snows",
            passive: "every blow carries the cold — struck enemies are briefly chilled and slowed",
            hp: 155.0, attack: 10.5, speed: 6.6, reach: 2.8,
            abilities: [
                AbilitySpec { name: "Snowbind", desc: "the drifts close over their paws — held fast where they stand",
                    verb: Verb::Root, power: 0.5, range: 7.0, dur: 1.6, cd: 9.0 },
                AbilitySpec { name: "White Silence", desc: "a settling cold that slows every enemy near",
                    verb: Verb::Slow, power: 0.6, range: 8.0, dur: 3.0, cd: 8.0 },
            ],
            ult: AbilitySpec { name: "The Long Winter", desc: "the season itself takes the field — everything near is frozen where it stands",
                verb: Verb::Root, power: 1.4, range: 11.0, dur: 2.2, cd: 55.0 },
        },
        // ── Riptail — the drowned-forest jaguar ────────────────────────────────
        // The other half of the control pair: Snowhush decides where the fight
        // stays, Riptail decides where it *gathers* — dragging scattered enemies
        // into one spot for a packmate's Earthshaker or Eclipse Hunt to land on.
        Ancestor::Jaguar => Kit {
            name: "Riptail", legend: "the drowned-forest jaguars of the flooded dark",
            philosophy: "The river does not chase either. It gathers.",
            passive_name: "The River Remembers",
            passive: "the held are already caught — strikes bite deeper into the slowed, rooted, or fleeing",
            hp: 185.0, attack: 11.0, speed: 6.1, reach: 2.8,
            abilities: [
                AbilitySpec { name: "Undertow", desc: "the current takes their legs — every enemy near is dragged in close",
                    verb: Verb::Pull, power: 0.7, range: 9.0, dur: 0.0, cd: 10.0 },
                AbilitySpec { name: "Crushing Jaws", desc: "the bite that cracks turtle shells",
                    verb: Verb::Strike, power: 2.1, range: 3.0, dur: 0.0, cd: 7.5 },
            ],
            ult: AbilitySpec { name: "The River Takes", desc: "the flood answers — the whole fight is dragged to one drowning point, battered together",
                verb: Verb::Pull, power: 2.0, range: 13.0, dur: 0.0, cd: 58.0 },
        },
        // ── Stillsmoke — the ninth son of the storm-dragon ─────────────────────
        // Out of the far-east thunder mountains, where the elders say the great
        // storm-dragon of the river sky had nine sons — and the ninth came out
        // cat-shaped: saber-fanged, scaled to the tail-tip, antlered like his
        // father, and wanting nothing his brothers wanted. No flight, no hoard.
        // He climbed onto a stone the size of a hill, folded his paws, and *sat*
        // — so still, the story goes, that the mountains learned stillness from
        // him, and the smoke of the peaks curled round him out of respect, and
        // the storm itself would part in a ring rather than disturb him. He is
        // the wild retelling of a lore the far-east cats kept about themselves:
        // rain when the land is wounded, thunder when it is wronged.
        // (The lion-dogs that flank the old drawings? Elders' embroidery. There
        // were never any two-legs to carve him. There was only the sitting.)
        Ancestor::Suanni => Kit {
            name: "Stillsmoke", legend: "the ninth son of the storm-dragon, of the far-east thunder mountains",
            philosophy: "Sit still long enough, and even the storm will come to ask your leave.",
            passive_name: "The Seated Mountain",
            passive: "stillness gathers the storm — the longer unmoving, the harder every strike, released on the move",
            hp: 145.0, attack: 10.8, speed: 5.8, reach: 8.0,
            abilities: [
                AbilitySpec { name: "Cloud Breath", desc: "a slow exhalation of mountain smoke — all it touches wades as through deep water",
                    verb: Verb::Slow, power: 0.7, range: 8.0, dur: 2.5, cd: 8.0 },
                AbilitySpec { name: "Sweetrain", desc: "the dragon's rain — packmates mend beneath it, and so does wounded wood",
                    verb: Verb::Rain, power: 0.8, range: 7.0, dur: 4.0, cd: 15.0 },
            ],
            ult: AbilitySpec { name: "The Ninth Thunder", desc: "he stands, once — and the thunder his father lent him empties the field",
                verb: Verb::Fear, power: 1.5, range: 12.0, dur: 1.6, cd: 60.0 },
        },
        // ── Stonewhisper — the grey ghost of the high crags ────────────────────
        Ancestor::Puma => Kit {
            name: "Stonewhisper", legend: "the grey ghosts of the high crags",
            philosophy: "The mountain does not hurry, and the mountain is never late.",
            passive_name: "Sure-Footed",
            passive: "slopes and scree cost nothing; blows barely shift those paws",
            hp: 210.0, attack: 10.0, speed: 5.6, reach: 2.8,
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
    // reach beyond the claw is safety, and safety is power — it costs budget
    k.hp / 30.0 + k.attack + k.speed * 1.5 + (k.reach - 2.8) * 0.5
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
        // and the archetypes are real: the hulk out-lives, the skirmisher outruns,
        // the yowlers pay for their reach in thinner pelts
        assert!(ALL.iter().all(|&a| kit(Ancestor::CaveLion).hp >= kit(a).hp));
        assert!(ALL.iter().all(|&a| kit(Ancestor::Caracal).speed >= kit(a).speed));
        assert!(ALL.iter().all(|&a| kit(Ancestor::Tiger).attack >= kit(a).attack));
        let ranged: Vec<Ancestor> = ALL.iter().copied().filter(|&a| kit(a).reach > 4.0).collect();
        assert_eq!(ranged.len(), 3, "three ranged kits: the yowler, the sun-priest, the ninth son");
        for a in ranged { assert!(kit(a).hp < 150.0, "{} must pay for reach in hp", kit(a).name); }
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
        // the smashers knock back; only the elder dawn executes; only the sun scorches ground
        assert!(verbs(Ancestor::Serval).contains(&Verb::Knockback));
        assert!(verbs(Ancestor::CaveLion).contains(&Verb::Knockback));
        assert_eq!(ALL.iter().filter(|&&a| verbs(a).contains(&Verb::Execute)).count(), 1);
        assert!(ALL.iter().all(|&a| !verbs(a).contains(&Verb::Zone) || a == Ancestor::SunCat));
        // the control pair own their crowd-control classes: only the lynx roots,
        // only the jaguar pulls — and both kits are control through and through
        assert!(ALL.iter().all(|&a| !verbs(a).contains(&Verb::Root) || a == Ancestor::Lynx));
        assert!(ALL.iter().all(|&a| !verbs(a).contains(&Verb::Pull) || a == Ancestor::Jaguar));
        // only the ninth son calls the rain — the one mender of wounded wood
        assert!(ALL.iter().all(|&a| !verbs(a).contains(&Verb::Rain) || a == Ancestor::Suanni));
        let control = |v: Verb| matches!(v, Verb::Root | Verb::Pull | Verb::Slow | Verb::Fear | Verb::Knockback);
        assert!(verbs(Ancestor::Lynx).iter().filter(|&&v| control(v)).count() >= 2);
        assert!(verbs(Ancestor::Jaguar).iter().filter(|&&v| control(v)).count() >= 2);
    }

    #[test]
    fn a_controllers_cooldowns_come_back_fast() {
        // the specialists' techniques cycle faster than anyone's ultimate and
        // faster than the bruisers' techniques — control has to be *available*
        for a in [Ancestor::Lynx, Ancestor::Jaguar] {
            let k = kit(a);
            assert!(k.abilities[0].cd <= 10.0 && k.abilities[1].cd <= 10.0,
                "{}: a controller's tools must cycle quickly", k.name);
        }
    }
}
