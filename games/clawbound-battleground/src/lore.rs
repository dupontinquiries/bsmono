//! lore -- who the ancestors are, as data, so the lore can be checked
//! against everything else the game says about them.
//!
//! The world (dev/LORE.md tells it at length): there were never humans. The
//! great cats inherited the world and kept it through every age -- the
//! frozen long-ago, the elder dawn, the green ages after. Each ancestor here
//! founded or embodies a clan of its age and land. When the living clans
//! face a thing that cannot be divided and must not be warred over, they
//! hold the Gathering: all the clans split by lot into two halves --
//! FROSTFANG, the cold half of the year, and EMBERCLAW, the warm -- meet on
//! the old grounds, and call their ancestors home across the ages to stand
//! with them. That is why a cave lion fights beside a tiger, why sides are
//! drawn by lot (the game's random picks), and why the teams are named for
//! seasons, not species.
//!
//! Genders: only Stillsmoke's was stated ("the ninth son", "he", "his
//! father"). The rest were decided here, against the data: a mane is a male
//! lion's, so Goldenmane and Oldroar (the Hulk pelt has "a ragged mane") are
//! male; the others as their stories suggest. Tests hold every decision to
//! the rest of the game's text.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    Female,
    Male,
}

impl Gender {
    pub fn pronouns(self) -> &'static str {
        match self {
            Gender::Female => "she/her",
            Gender::Male => "he/him",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Ancestor {
    /// The ancestor's name, as in the roster.
    pub name: &'static str,
    pub species: &'static str,
    /// When they lived.
    pub age: &'static str,
    /// Where their clan kept the world.
    pub land: &'static str,
    pub gender: Gender,
    /// A few sentences: who they were.
    pub story: &'static str,
    /// The sound of their clan's culture: for scoring and sound design.
    pub instruments: &'static str,
}

pub const ANCESTORS: [Ancestor; 17] = [
    Ancestor {
        name: "Goldenmane",
        species: "lion",
        age: "the green ages",
        land: "the golden plains",
        gender: Gender::Male,
        story: "First of the lion-clan to lead a pride that was not his by birth. He taught that a cat alone is a whisker and a pride is a storm, and the plains have been loud with prides ever since.",
        instruments: "kora and balafon, talking drum, djembe; grass wind and distant thunder",
    },
    Ancestor {
        name: "Duskstripe",
        species: "tiger",
        age: "the green ages",
        land: "the reed-forests of the river deltas",
        gender: Gender::Female,
        story: "A tigress who raised four litters alone and taught every one of them to strike once, from where no one looked. The tiger-clan still hunts at dusk because she did.",
        instruments: "bansuri flute, tabla, low drones; frogs, rain on broad leaves",
    },
    Ancestor {
        name: "Rainspots",
        species: "leopard",
        age: "the green ages",
        land: "the river valleys",
        gender: Gender::Female,
        story: "A leopard who fished the great rivers and mended the hurt she found on their banks. She never fought the current; she went around, and the leopard-clan learned to win that way.",
        instruments: "mbira and kalimba, water drums, shakers; running water, birdsong",
    },
    Ancestor {
        name: "Nightpelt",
        species: "black leopard",
        age: "the green ages",
        land: "the moonless woods",
        gender: Gender::Female,
        story: "Mother of the shadow-walkers, black as the woods between moons. She never chased; she learned where prey would choose to run, and was already there.",
        instruments: "soft gongs, slit drums, bamboo flute; night insects, a distant owl",
    },
    Ancestor {
        name: "Windclaw",
        species: "caracal",
        age: "the green ages",
        land: "the dry plains",
        gender: Gender::Male,
        story: "A young caracal who leapt at birds in the air and caught them, and so decided the sky could be hunted. The wind-eared clan counts its years by his hundred leaps.",
        instruments: "ney flute, bendir frame drum, rattles; hot wind, cicadas",
    },
    Ancestor {
        name: "Faryowl",
        species: "serval",
        age: "the green ages",
        land: "the singing grasslands",
        gender: Gender::Female,
        story: "The first of the far-yowlers, whose call crossed the grasslands before she did. Her clan settles quarrels by song, and she has never lost one.",
        instruments: "layered voices, musical bow, udu; tall grass, weaver birds",
    },
    Ancestor {
        name: "Oldroar",
        species: "cave lion",
        age: "the frozen long-ago",
        land: "the ice-steppes of the old north",
        gender: Gender::Male,
        story: "Lord of the cave-lions when the ice lay over half the world. The ice took his clan, but not him: the old world, he says, is only sleeping in him.",
        instruments: "bone flute, lithophone (struck stones), deep skin drums, bullroarer, long horns; wind over ice",
    },
    Ancestor {
        name: "Twofang",
        species: "sabre-tooth (smilodon)",
        age: "the elder dawn",
        land: "the tar-pit plains of the far west",
        gender: Gender::Male,
        story: "A sabre-tooth of the elder dawn, when the prey was the size of hills. His claws began the hunt and his fangs ended it. The pass where the Gathering meets bears his fangs' name.",
        instruments: "clay ocarinas, log drums, rattles; bubbling tar, great hooves",
    },
    Ancestor {
        name: "Suneye",
        species: "sand cat",
        age: "the golden ages",
        land: "the endless dunes and the sun temples",
        gender: Gender::Female,
        story: "High priestess of the sun-cats, who read the dunes by noon light and kept no secrets from it. The disc above her brow is the sun's promise: nothing hides.",
        instruments: "sistrum, ney, darbuka, oud; desert wind, sand hiss",
    },
    Ancestor {
        name: "Snowhush",
        species: "lynx",
        age: "the long winters",
        land: "the white silence of the northern taiga",
        gender: Gender::Female,
        story: "A lynx of the deep snows who never fought where her enemy chose. She picked the ground, let the snow fall, and waited; her clan keeps the silence still.",
        instruments: "hardanger fiddle, nyckelharpa, kulning (herding calls), goat horn, frame drum; wind in spruce, creaking ice",
    },
    Ancestor {
        name: "Riptail",
        species: "jaguar",
        age: "the green ages",
        land: "the drowned forest",
        gender: Gender::Female,
        story: "A jaguar of the flooded dark, who hunted under the water as easily as over it. The river does not chase, she says; it gathers, and so does she.",
        instruments: "rainstick, panpipes, water percussion, slit drum; rain, rushing floodwater",
    },
    Ancestor {
        name: "Stillsmoke",
        species: "dragon-cat, son of the storm-dragon",
        age: "the age of storms",
        land: "the far-east thunder mountains",
        gender: Gender::Male,
        story: "Ninth son of the storm-dragon: antlered, scaled, sabre-fanged, and happiest sitting still in incense smoke. He sits so long that the storm comes to ask his leave -- and when he stands, his father's thunder stands with him.",
        instruments: "guqin, dizi flute, temple bells, great barrel drums; mountain rain, far thunder",
    },
    Ancestor {
        name: "Stonewhisper",
        species: "snow leopard",
        age: "the long winters",
        land: "the high crags",
        gender: Gender::Female,
        story: "The grey ghost of the crags, seldom seen and never late. She crossed the high passes in every weather; the mountain taught her not to hurry.",
        instruments: "long mountain horns, singing bowls, damaru drum, low chant; high wind, falling scree",
    },
    Ancestor {
        name: "Cometail",
        species: "caracal",
        age: "the elder dawn",
        land: "the glacier-sky above the cloud sea, where the Starwell is",
        gender: Gender::Male,
        story: "A caracal who leapt so high after the birds that he caught the tail of a falling comet, and came down with it onto the glacier. Where he landed, the Starwell opened, and he has been a little made of starlight ever since. He never once asked where he might land.",
        instruments: "glass harmonica, crystal bowls, frame drum, overtone singing; the hiss of a meteor, ice cracking in the cold",
    },
    Ancestor {
        name: "Sproutkit",
        species: "a kitten -- of no one clan, and so of every clan",
        age: "every spring",
        land: "wherever something is being born",
        gender: Gender::Male,
        story: "The youngest of the ancestors, and the only one who never grew up. Every litter born in every clan is a little of him, so there is always more of him than anyone counted. He turns every fight into a game, and every game into a dance -- and what dances cannot fight. Where he naps, the first green of the year comes up through the snow.",
        instruments: "toy piano, music box, ukulele, kazoo, handclaps, a disco beat; birdsong, a spring brook",
    },
    Ancestor {
        name: "Hollowstep",
        species: "wildcat",
        age: "the long nights",
        land: "the fog moors",
        gender: Gender::Male,
        story: "Nobody on the moors ever saw Hollowstep arrive. His tread was heard on one side of the heath and he was found on the other, and the clans learned to count their own shadows before they slept. He never ran after anything in his life; he never had to. They say another of him is still out on the moor tonight, walking, and that you will hear it long before you see it.",
        instruments: "bowed saw, low cello drones, a slow bass drum like a heartbeat, held breath; wind over heather, a curlew far off",
    },
    Ancestor {
        name: "Brasshowl",
        species: "sand cat",
        age: "the caravan years",
        land: "the salt roads",
        gender: Gender::Male,
        story: "The smallest of the ancestors, with the biggest voice. He walked ahead of the caravans on the salt roads at night with a brass lantern, and when something moved at the edge of its light he raised the lamp and bellowed, and whatever it was did not come in. It is said that Hollowstep has never once stood in Brasshowl's light, and that the two of them know exactly why.",
        instruments: "frame drum, oud, a brass bell, clapping hands, a caravan's camel bells; desert wind, a crackling lamp",
    },
];

pub fn of(name: &str) -> Option<&'static Ancestor> {
    ANCESTORS.iter().find(|a| a.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gods::{PeltKind, ROSTER};

    #[test]
    fn every_ancestor_has_lore_and_the_names_match() {
        for d in ROSTER.iter() {
            assert!(of(d.name).is_some(), "no lore for {}", d.name);
        }
        assert_eq!(ANCESTORS.len(), ROSTER.len());
    }

    /// A mane is a male lion's: every maned pelt (Goldenmane's Mane,
    /// Oldroar's Hulk -- "a ragged mane") is a male ancestor.
    #[test]
    fn manes_are_on_males() {
        for d in ROSTER.iter() {
            if matches!(d.pelt, PeltKind::Mane | PeltKind::Hulk) {
                assert_eq!(of(d.name).unwrap().gender, Gender::Male, "{} has a mane", d.name);
            }
        }
    }

    /// Every pronoun the game says about an ancestor -- in their title,
    /// motto and ability descriptions -- agrees with the decided gender.
    /// (Stillsmoke's "son", "he", "his father" are the only ones today.)
    #[test]
    fn the_games_own_words_agree() {
        let male = ["he", "him", "his", "himself", "son", "father's"];
        let female = ["she", "her", "hers", "herself", "daughter", "mother's"];
        for d in ROSTER.iter() {
            let g = of(d.name).unwrap().gender;
            let mut text = format!("{} {}", d.title, d.philosophy);
            for ab in d.abilities.iter() {
                text.push(' ');
                text.push_str(ab.desc);
            }
            let words: Vec<String> = text.split(|c: char| !c.is_alphabetic() && c != '\'').map(|w| w.to_ascii_lowercase()).collect();
            let wrong: &[&str] = if g == Gender::Male { &female } else { &male };
            for w in &words {
                assert!(!wrong.contains(&w.as_str()), "{}: the game says '{w}', the lore says {}", d.name, g.pronouns());
            }
        }
        assert_eq!(of("Stillsmoke").unwrap().gender, Gender::Male, "canon: the ninth son");
    }

    /// Each story speaks of its ancestor as the lore's gender does. (When
    /// Riptail was recast as she, nothing here would have noticed a story
    /// still saying "he says" -- the check above reads the game's text,
    /// not the stories.)
    #[test]
    fn the_stories_agree() {
        let male = ["he", "him", "his", "himself"];
        let female = ["she", "her", "hers", "herself"];
        let mut checked = 0;
        for a in ANCESTORS.iter() {
            let words: Vec<String> = a.story.split(|c: char| !c.is_alphabetic() && c != '\'').map(|w| w.to_ascii_lowercase()).collect();
            let (right, wrong): (&[&str], &[&str]) = if a.gender == Gender::Male { (&male, &female) } else { (&female, &male) };
            for w in &words {
                assert!(!wrong.contains(&w.as_str()), "{}: the story says '{w}', the lore says {}", a.name, a.gender.pronouns());
            }
            checked += words.iter().any(|w| right.contains(&w.as_str())) as usize;
        }
        assert!(checked >= 10, "only {checked} stories use a pronoun at all");
    }

    /// The voice notes the performer reads do not contradict the gender
    /// (no baritone for a female ancestor, no 'she' for a male).
    #[test]
    fn the_voice_notes_agree() {
        for a in ANCESTORS.iter() {
            let who = a.name.to_ascii_lowercase();
            let p = crate::voicescript::part(&who).unwrap();
            let v = p.voice.to_ascii_lowercase();
            if a.gender == Gender::Female {
                for w in ["baritone", "bass", " he ", " his "] {
                    assert!(!v.contains(w), "{}: voice note says '{w}'", a.name);
                }
            } else {
                for w in [" she ", " her ", "alto", "soprano"] {
                    assert!(!v.contains(w), "{}: voice note says '{w}'", a.name);
                }
            }
        }
    }

    /// A balanced cast.
    #[test]
    fn the_cast_is_balanced() {
        let f = ANCESTORS.iter().filter(|a| a.gender == Gender::Female).count();
        assert!((6..=8).contains(&f) && (f as i32 - (ANCESTORS.len() - f) as i32).abs() <= 1, "{f} of {} female", ANCESTORS.len());
    }
}
