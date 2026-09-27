//! voicescript -- every line the game can voice, as data: who says it, the
//! words, how to deliver it, and the file it must be recorded to.
//!
//! One table, three uses: `examples/script_cards` renders it as SVG cue
//! cards to read at the mic and a recording sheet (TSV); the tests hold it
//! to exactly the keys the game asks for (`cues`, `announcer`), so nothing
//! recorded is orphaned and no cue is left unscripted.
//!
//! Files: `voice/<ancestor>/<key>.opus` for the ancestors (`kill.1`,
//! `kill.2` are two takes of the kill line, played at random),
//! `announcer/<key>.opus` for the announcer.

/// One line to record.
#[derive(Debug, Clone, Copy)]
pub struct Line {
    /// The file stem: "cast1", "kill.2", "first_blood".
    pub key: &'static str,
    pub text: &'static str,
    /// How to say it.
    pub direction: &'static str,
    /// 1 (a murmur) .. 5 (flat-out shout).
    pub energy: u8,
    /// When the game plays it.
    pub trigger: &'static str,
}

/// One voice: an ancestor or the announcer.
#[derive(Debug, Clone, Copy)]
pub struct Part {
    /// The key segment: the ancestor's name lower-cased ("goldenmane"), or
    /// "announcer".
    pub who: &'static str,
    /// Who they are, for the performer.
    pub voice: &'static str,
    pub lines: &'static [Line],
}

const fn l(key: &'static str, text: &'static str, direction: &'static str, energy: u8, trigger: &'static str) -> Line {
    Line { key, text, direction, energy, trigger }
}

const SELECT: &str = "locked in on the select screen";
const KILL: &str = "takes down an enemy ancestor";
const DEATH: &str = "falls";

pub const PARTS: &[Part] = &[
    Part {
        who: "goldenmane",
        voice: "Warm, commanding baritone. A leader rallying a pride -- never cruel, always certain.",
        lines: &[
            l("select", "The pride rides with me!", "rallying call, chest out", 4, SELECT),
            l("cast1", "Sunstrike!", "a pounce -- sharp, leaping upward", 4, "winds up Sunstrike Pounce"),
            l("cast2", "Behind my mane!", "protective, planting your feet", 3, "winds up Mane of Dawn"),
            l("cast3", "As one, pride!", "a battle cry to the team", 4, "winds up Pride's Warmth"),
            l("cast4", "Hear me ROAR!", "build to a full roar on the last word", 5, "winds up The Great Roar"),
            l("kill.1", "A whisker, alone.", "quiet satisfaction, almost kind", 2, KILL),
            l("kill.2", "The pride feeds.", "low, pleased growl", 3, KILL),
            l("death.1", "The pride... endures...", "wounded, proud to the last", 2, DEATH),
            l("death.2", "Not... alone...", "fading breath", 1, DEATH),
        ],
    },
    Part {
        who: "duskstripe",
        voice: "Low, unhurried, close to the mic. Menace by understatement -- the tiger never raises its voice.",
        lines: &[
            l("select", "You never looked.", "a whisper right in the ear", 2, SELECT),
            l("cast1", "Maul.", "one heavy word, a blow", 3, "winds up Shadow Maul"),
            l("cast2", "Into the reeds.", "breathy, vanishing", 1, "winds up Stripes in the Reeds"),
            l("cast3", "Hungry.", "a purr with teeth", 2, "winds up Night's Hunger"),
            l("cast4", "Let dusk fall.", "slow, final, sinking pitch", 3, "winds up Long Shadows"),
            l("kill.1", "Once was enough.", "matter-of-fact", 2, KILL),
            l("kill.2", "From behind.", "a smile you can hear", 2, KILL),
            l("death.1", "Seen... at last...", "surprised, then resigned", 2, DEATH),
            l("death.2", "Too... bright...", "fading hiss", 1, DEATH),
        ],
    },
    Part {
        who: "rainspots",
        voice: "Easy, fluid, amused. A leopard who goes around every obstacle and always arrives.",
        lines: &[
            l("select", "Around, and through.", "light and knowing", 3, SELECT),
            l("cast1", "Flow!", "quick burst, forward motion", 4, "winds up Spotted Blur"),
            l("cast2", "Mend, like water.", "gentle, soothing", 2, "winds up Fisher's Mercy"),
            l("cast3", "Riverborn!", "proud, rising", 4, "winds up Riverborn"),
            l("cast4", "The flood comes!", "swelling, unstoppable", 5, "winds up Floodrush"),
            l("kill.1", "Washed away.", "breezy", 2, KILL),
            l("kill.2", "The river wins.", "calm certainty", 3, KILL),
            l("death.1", "Back... to the river...", "peaceful, drifting off", 2, DEATH),
            l("death.2", "Still... flowing...", "a last smile", 1, DEATH),
        ],
    },
    Part {
        who: "nightpelt",
        voice: "Hushed and patient, almost tender. The moonless woods speaking. Never shout -- except the eclipse.",
        lines: &[
            l("select", "Where you run, I am.", "soft, inevitable", 2, SELECT),
            l("cast1", "Midnight.", "a single struck bell of a word", 3, "winds up Pounce of Midnight"),
            l("cast2", "Veil.", "barely voiced", 1, "winds up Veilwalker"),
            l("cast3", "Hush...", "a breath out", 1, "winds up Silent Footfall"),
            l("cast4", "ECLIPSE!", "the one time the woods scream", 5, "winds up Eclipse Hunt"),
            l("kill.1", "You ran to me.", "gentle, almost sorry", 2, KILL),
            l("kill.2", "Sleep.", "a lullaby of one word", 1, KILL),
            l("death.1", "The moon... rises...", "wonder, fading", 2, DEATH),
            l("death.2", "Found...", "a sigh", 1, DEATH),
        ],
    },
    Part {
        who: "windclaw",
        voice: "Bright, fast, a little wild. A plains hunter who thinks the sky is fair game.",
        lines: &[
            l("select", "The sky is prey!", "exuberant, head thrown back", 4, SELECT),
            l("cast1", "Snap!", "a clipped bark", 4, "winds up Sky Snap"),
            l("cast2", "Sand!", "spat out mid-sprint", 3, "winds up Sand Dash"),
            l("cast3", "Up, up!", "eager, climbing", 3, "winds up Skyward Hunger"),
            l("cast4", "A hundred leaps!", "joyful frenzy", 5, "winds up Hundred Leaps"),
            l("kill.1", "Caught on the wind!", "gleeful", 4, KILL),
            l("kill.2", "Plucked!", "a laugh in it", 3, KILL),
            l("death.1", "The ground... again...", "disgusted", 2, DEATH),
            l("death.2", "Grounded...", "deflated", 1, DEATH),
        ],
    },
    Part {
        who: "faryowl",
        voice: "Projected, singing, theatrical -- a voice built to cross grasslands. Let the vowels ring.",
        lines: &[
            l("select", "Hear me across the clearing!", "project to the back row", 4, SELECT),
            l("cast1", "YOWL!", "a piercing sung note", 5, "winds up Piercing Yowl"),
            l("cast2", "Wall of sound!", "big and round", 4, "winds up Wall of Sound"),
            l("cast3", "Sing, far!", "lilting, a phrase of song", 3, "winds up The Far Song"),
            l("cast4", "The Great Caterwaul!", "an operatic swell", 5, "winds up The Great Caterwaul"),
            l("kill.1", "Heard that?", "cheeky", 3, KILL),
            l("kill.2", "Silenced.", "a sung falling third", 3, KILL),
            l("death.1", "My... song...", "a note that breaks", 2, DEATH),
            l("death.2", "...quiet...", "almost nothing", 1, DEATH),
        ],
    },
    Part {
        who: "oldroar",
        voice: "Vast, slow, gravelly -- an ice-age cave lion waking. Leave space between words.",
        lines: &[
            l("select", "The old world... wakes.", "deep, slow, a long pause", 3, SELECT),
            l("cast1", "Boulder!", "a grunt of effort", 4, "winds up Boulder Paw"),
            l("cast2", "Shatter!", "cracking, harsh", 4, "winds up Shatter Ground"),
            l("cast3", "Wounds... fury!", "pain turning to rage", 4, "winds up Deep Wounds, Deep Fury"),
            l("cast4", "EARTH -- SHAKE!", "two huge beats", 5, "winds up Earthshaker"),
            l("kill.1", "Back to the ice.", "rumbling", 3, KILL),
            l("kill.2", "Old. And stronger.", "slow pride", 3, KILL),
            l("death.1", "Sleep... again...", "a glacier settling", 2, DEATH),
            l("death.2", "The ice... takes me...", "fading rumble", 1, DEATH),
        ],
    },
    Part {
        who: "twofang",
        voice: "Clipped, deadly, a butcher's economy of words. The sabertooth never says more than it must.",
        lines: &[
            l("select", "Claws begin. Fangs end.", "two cold statements", 3, SELECT),
            l("cast1", "Lunge!", "explosive", 4, "winds up Saber Lunge"),
            l("cast2", "Hamstring.", "precise, surgical", 3, "winds up Hamstring"),
            l("cast3", "Finish it.", "impatient", 3, "winds up The Saber Finishes"),
            l("cast4", "The killing bite!", "a snarl on 'bite'", 5, "winds up The Killing Bite"),
            l("kill.1", "Ended.", "flat", 2, KILL),
            l("kill.2", "Two fangs. One end.", "grim relish", 3, KILL),
            l("death.1", "Dull... fangs...", "bitter", 2, DEATH),
            l("death.2", "Not... the end...", "defiant, fading", 1, DEATH),
        ],
    },
    Part {
        who: "suneye",
        voice: "Radiant, ceremonial, a priestly cadence. The sun hides nothing, and neither does this voice.",
        lines: &[
            l("select", "The sun keeps no secrets.", "a proclamation", 3, SELECT),
            l("cast1", "Sunspear!", "a thrown command", 4, "winds up Sunspear"),
            l("cast2", "Noon!", "blazing, held", 4, "winds up Noon Blaze"),
            l("cast3", "By the sun's favor.", "a blessing", 2, "winds up The Sun's Favor"),
            l("cast4", "The Eye opens!", "awe and fury", 5, "winds up The Eye of the Sun"),
            l("kill.1", "Burned clean.", "serene", 2, KILL),
            l("kill.2", "Nothing hides.", "judgement", 3, KILL),
            l("death.1", "Sunset...", "a benediction", 2, DEATH),
            l("death.2", "So... dark...", "fear, at the very end", 1, DEATH),
        ],
    },
    Part {
        who: "snowhush",
        voice: "Quiet authority, cold and clear. A lynx who decides where the fight happens.",
        lines: &[
            l("select", "Here. The fight happens here.", "calm, absolute", 3, SELECT),
            l("cast1", "Snowbind.", "crisp", 3, "winds up Snowbind"),
            l("cast2", "Hush.", "a command, not a request", 2, "winds up White Silence"),
            l("cast3", "Snow, cover us.", "protective", 2, "winds up Hush of Snows"),
            l("cast4", "The long winter comes.", "slow, spreading", 4, "winds up The Long Winter"),
            l("kill.1", "Silence.", "satisfied", 2, KILL),
            l("kill.2", "Snowed under.", "dry humour", 2, KILL),
            l("death.1", "Cold... at last...", "relief", 2, DEATH),
            l("death.2", "The snow... keeps me...", "drifting", 1, DEATH),
        ],
    },
    Part {
        who: "riptail",
        voice: "Heavy, wet, patient -- a jaguar of the drowned forest. Words pulled down like a current.",
        lines: &[
            l("select", "The river gathers.", "low and certain", 3, SELECT),
            l("cast1", "Undertow!", "a heave", 4, "winds up Undertow"),
            l("cast2", "Jaws!", "a snap", 4, "winds up Crushing Jaws"),
            l("cast3", "Remember.", "ominous", 3, "winds up The River Remembers"),
            l("cast4", "The river TAKES!", "a roar on 'takes'", 5, "winds up The River Takes"),
            l("kill.1", "Drowned.", "cold", 2, KILL),
            l("kill.2", "Gathered.", "patient pleasure", 2, KILL),
            l("death.1", "The current... turns...", "surprised", 2, DEATH),
            l("death.2", "Pulled... under...", "sinking", 1, DEATH),
        ],
    },
    Part {
        who: "stillsmoke",
        voice: "Serene, unhurried, faintly amused -- the storm-dragon's ninth son. Calm until the thunder.",
        lines: &[
            l("select", "Sit. The storm will come.", "tranquil", 2, SELECT),
            l("cast1", "Breathe.", "a slow exhale", 1, "winds up Cloud Breath"),
            l("cast2", "Sweet rain.", "kind", 2, "winds up Sweetrain"),
            l("cast3", "The mountain sits.", "immovable", 3, "winds up The Seated Mountain"),
            l("cast4", "NINTH THUNDER!", "all the calm breaks at once", 5, "winds up The Ninth Thunder"),
            l("kill.1", "The storm asked leave.", "gently wry", 2, KILL),
            l("kill.2", "Still.", "a single settled word", 1, KILL),
            l("death.1", "Smoke... clears...", "unbothered", 1, DEATH),
            l("death.2", "The thunder... waits...", "a promise", 2, DEATH),
        ],
    },
    Part {
        who: "stonewhisper",
        voice: "A grey ghost of the crags: dry, sparse, never hurried. Every word placed like a foothold.",
        lines: &[
            l("select", "Never late.", "understated", 2, SELECT),
            l("cast1", "Crag!", "a leap", 4, "winds up Crag Leap"),
            l("cast2", "Stone.", "unmoved", 2, "winds up Stone Patience"),
            l("cast3", "Steady.", "reassuring", 2, "winds up Sure-Footed"),
            l("cast4", "The crags answer!", "an echoing call", 5, "winds up The Crags Answer"),
            l("kill.1", "On time.", "dry", 2, KILL),
            l("kill.2", "The mountain waited.", "patient", 2, KILL),
            l("death.1", "Crumble...", "a rockfall of a word", 2, DEATH),
            l("death.2", "Stone... sleeps...", "settling", 1, DEATH),
        ],
    },
    Part {
        who: "cometail",
        voice: "Bright, quick tenor with a spark of laughter under it: a young daredevil who has already jumped. Fast in, fast out -- then one huge call for the falling star.",
        lines: &[
            l("select", "Look up. I'm already falling.", "grinning, a little breathless", 3, SELECT),
            l("cast1", "Starshot!", "a flick of the paw, sharp", 4, "winds up Starshot"),
            l("cast2", "Pounce!", "a leap -- up on the vowel", 4, "winds up Meteor Pounce"),
            l("cast3", "Flare!", "a bright burst, eyes shut", 4, "winds up Flare"),
            l("cast4", "Watch the SKY!", "from far above, rising to a shout", 5, "winds up Falling Star"),
            l("kill.1", "Never saw me coming. Nobody does.", "light, teasing", 3, KILL),
            l("kill.2", "Burned out.", "quiet, almost gentle", 2, KILL),
            l("death.1", "Just... a shooting star...", "fading, wistful", 2, DEATH),
            l("death.2", "Back... to the sky...", "a last breath, looking up", 1, DEATH),
        ],
    },
    Part {
        who: "sproutkit",
        voice: "A kitten's voice: bright, high, fearless and giggly -- a baby god who thinks the whole war is a game. Laughter bubbles up between the words. Never menacing, even on a kill.",
        lines: &[
            l("select", "Let's play!", "bouncing, delighted", 4, SELECT),
            l("cast1", "Everybody, go!", "calling the litter out, a bounce on 'go'", 4, "winds up Litter"),
            l("cast2", "Hee hee hee!", "a giggle fit that won't stop", 3, "winds up Giggle Fit"),
            l("cast3", "Grow, grow!", "soft, sing-song", 3, "winds up First Spring"),
            l("cast4", "DANCE PARTY!", "a squeal of pure joy", 5, "winds up Dance Party"),
            l("kill.1", "Tag, you're out!", "teasing", 3, KILL),
            l("kill.2", "Nap time!", "a yawn in it", 2, KILL),
            l("death.1", "Not... fair...", "a pout", 2, DEATH),
            l("death.2", "I'll grow... back...", "sleepy, sure of it", 1, DEATH),
        ],
    },
    Part {
        who: "hollowstep",
        voice: "Low, slow and close to the microphone: a stalker who never raises his voice because he never has to. Patient, almost gentle, with long silences left in. Breathe. Only the ultimate is loud.",
        lines: &[
            l("select", "I'm already there.", "quiet, certain, right at the mic", 2, SELECT),
            l("cast1", "Behind you.", "a whisper at the ear", 2, "winds up Step Between"),
            l("cast2", "Go on. Wait for them.", "patient, to someone beside you", 2, "winds up Send the Other"),
            l("cast3", "Too slow.", "flat, a little disappointed", 3, "winds up The Long Reach"),
            l("cast4", "Which one... is ME?", "from a whisper to a roar on 'me'", 5, "winds up Every Shadow"),
            l("kill.1", "Found you.", "soft, almost tender", 2, KILL),
            l("kill.2", "You ran. They always run.", "weary", 2, KILL),
            l("death.1", "I'll... be... waiting...", "fading, not beaten", 1, DEATH),
            l("death.2", "Check... the shadows...", "a last whisper, a threat", 1, DEATH),
        ],
    },
    Part {
        who: "brasshowl",
        voice: "A small cat with an enormous voice: warm, loud, a caravan hand calling down the line at night. Big and round even when he is being kind. The ultimate is the biggest bellow you have.",
        lines: &[
            l("select", "Keep to the light!", "calling back down the road", 4, SELECT),
            l("cast1", "Catch!", "a throw, cheerful", 3, "winds up Lamp Oil"),
            l("cast2", "BACK!", "one bark, sharp", 4, "winds up Bark"),
            l("cast3", "Sit by the fire.", "warm, looking after someone", 2, "winds up Watchfire"),
            l("cast4", "SHADOWS!", "the biggest bellow you have", 5, "winds up SHADOWS!"),
            l("kill.1", "Out you go.", "brisk", 3, KILL),
            l("kill.2", "The road is clear.", "satisfied, to the caravan", 3, KILL),
            l("death.1", "The lamp... is out...", "surprised, quiet", 2, DEATH),
            l("death.2", "Keep... it... lit...", "a last instruction", 1, DEATH),
        ],
    },
    Part {
        who: "oremantle",
        voice: "A very old, very big grandmother: low, slow and unbothered, every word set down like a stone. She is never in a hurry and never unkind. Chest voice, room left between words. Only the ultimate rises -- from a rumble to a roar.",
        lines: &[
            l("select", "The mountain is here.", "slow, settling in", 2, SELECT),
            l("cast1", "Stand behind me.", "firm, protective", 3, "shifts to Granite"),
            l("cast2", "Cold, now.", "quiet, breath misting", 2, "shifts to Rime"),
            l("cast3", "Come here, child.", "patient, drawing someone in", 2, "shifts to Lodestone"),
            l("cast4", "The mountain has a HEART!", "from a rumble to a roar on 'heart'", 5, "shifts to The Mountain's Heart"),
            l("kill.1", "Rest now.", "grandmotherly, almost kind", 2, KILL),
            l("kill.2", "Stone outlasts.", "flat, certain", 2, KILL),
            l("death.1", "Even mountains... fall...", "surprised, slow", 1, DEATH),
            l("death.2", "Back... to the rock...", "content, going home", 1, DEATH),
        ],
    },
    Part {
        who: "narrator",
        voice: "The storyteller of the clans, over the first match's prologue: unhurried, weighty, a little in awe. Let each line breathe; the music leaves room between them. Record each as its own file.",
        lines: &[
            l("prologue1", "Before any word was written, the world belonged to the clans.", "hushed, as if starting an old story", 2, "the prologue opens, line 1 of 8 (first match of a session)"),
            l("prologue2", "No hand raised a wall. No fire burned but the lightning's.", "slow, each sentence its own image", 2, "the prologue, line 2 of 8"),
            l("prologue3", "From the frozen long-ago to the endless dunes, the great cats kept it.", "growing, sweeping", 3, "the prologue, line 3 of 8"),
            l("prologue4", "Lions of the golden plains. Tigers of the dusk. Sabre-fangs of the elder dawn.", "a roll-call, each name a beat", 3, "the prologue, line 4 of 8"),
            l("prologue5", "Their bones are ice and sand now. Their stories never were.", "quiet, then warm on the last phrase", 2, "the prologue, line 5 of 8"),
            l("prologue6", "When a thing cannot be divided, and must not be warred over, the clans hold the Gathering.", "measured, the law of the clans", 3, "the prologue, line 6 of 8"),
            l("prologue7", "They split by lot -- Frostfang and Emberclaw -- and call their ancestors home.", "rising, naming the two halves with weight", 4, "the prologue, line 7 of 8"),
            l("prologue8", "Beasts of power. Beasts of glory. Let the ages meet.", "the peak: full voice on the last four words", 5, "line 8 of 8: the prologue ends, the world comes into view"),
        ],
    },
    Part {
        who: "announcer",
        voice: "The voice of the battleground: big, warm, theatrical, like a sports caller who loves these cats. Leave half a second of silence before and after every line.",
        lines: &[
            l("match_start", "The clans are loosed!", "the opening bell", 5, "the intro ends and play begins"),
            l("first_blood", "First blood!", "electric", 5, "the first ancestor falls"),
            l("kill", "An ancestor has fallen!", "dramatic", 3, "any other kill (when nothing bigger applies)"),
            l("double_kill", "Double kill!", "rising", 4, "two kills in quick succession"),
            l("triple_kill", "Triple kill!", "higher still", 5, "three in quick succession"),
            l("rampage", "RAMPAGE!", "lose control a little", 5, "four or more in quick succession"),
            l("spree", "On the prowl!", "impressed", 4, "three kills without dying"),
            l("unstoppable", "Unstoppable!", "awed", 5, "six kills without dying"),
            l("shutdown", "Shut down!", "a hard stop", 4, "a cat on a spree is taken down"),
            l("ace", "Ace! The whole clan has fallen!", "the crowd goes wild", 5, "every cat of one team is dead at once"),
            l("tower_down_frostfang", "A Frostfang tower falls!", "news of the front", 4, "a Frostfang (blue) tower is destroyed"),
            l("tower_down_emberclaw", "An Emberclaw tower falls!", "news of the front", 4, "an Emberclaw (red) tower is destroyed"),
            l("titan_frostfang", "The Frostfang Elder stands unshielded!", "urgent", 5, "the blue Elder can be struck"),
            l("titan_emberclaw", "The Emberclaw Elder stands unshielded!", "urgent", 5, "the red Elder can be struck"),
            l("boss_spawn", "Something stirs in the deep...", "hushed suspense", 3, "a great beast appears"),
            l("boss_down", "The great beast is slain!", "triumphant", 4, "a great beast is killed"),
            l("disaster", "The land turns against you -- take cover!", "alarm", 5, "a disaster is about to strike"),
            l("victory", "Victory!", "pure joy", 5, "your team wins"),
            l("defeat", "Defeat.", "sombre, respectful", 2, "your team loses"),
            l("frostfang_win", "Frostfang are victorious!", "the big finish", 5, "Frostfang win (spectating)"),
            l("emberclaw_win", "Emberclaw are victorious!", "the big finish", 5, "Emberclaw win (spectating)"),
        ],
    },
];

pub fn part(who: &str) -> Option<&'static Part> {
    PARTS.iter().find(|p| p.who == who)
}

/// The file a line is recorded to, under the audio root.
pub fn file_for(who: &str, key: &str) -> String {
    if who == "announcer" || who == "narrator" {
        format!("{who}/{key}.opus")
    } else {
        format!("voice/{who}/{key}.opus")
    }
}

/// The bank key a file plays as (a take's `.N` is not part of it).
pub fn bank_key(who: &str, key: &str) -> String {
    let stem = key.split('.').next().unwrap_or(key);
    if who == "announcer" || who == "narrator" {
        format!("{who}/{stem}")
    } else {
        format!("voice/{who}/{stem}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gods::ROSTER;

    /// Every ancestor has a part, with exactly the lines the game voices:
    /// select, cast1-4, two kill takes, two death takes.
    #[test]
    fn every_ancestor_has_every_line_the_game_voices() {
        let game = crate::sim::Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        for (i, def) in ROSTER.iter().enumerate() {
            let who = def.name.to_ascii_lowercase().replace([' ', '\''], "");
            let p = part(&who).unwrap_or_else(|| panic!("no part for {}", def.name));
            for k in ["select", "cast1", "cast2", "cast3", "cast4", "kill.1", "kill.2", "death.1", "death.2"] {
                assert!(p.lines.iter().any(|l| l.key == k), "{}: no '{k}' line", def.name);
            }
            assert_eq!(p.lines.len(), 9, "{}: stray lines", def.name);
            let _ = i;
            // the keys match what the cues ask for
            let slot0 = game.gods.iter().position(|g| g.def_id == def.id);
            if let Some(g) = slot0 {
                assert_eq!(crate::cues::ancestor_key(&game, g), who);
            }
        }
        assert_eq!(PARTS.len(), ROSTER.len() + 2, "every ancestor, the narrator and the announcer");
    }

    #[test]
    fn lines_are_shoutable_and_well_formed() {
        for p in PARTS {
            for l in p.lines {
                assert!((1..=5).contains(&l.energy), "{}/{}", p.who, l.key);
                // shouted lines are short; the narrator tells a story
                let max = if p.who == "narrator" { 100 } else { 48 };
                assert!(!l.text.is_empty() && l.text.len() <= max, "{}/{}: too long: {}", p.who, l.key, l.text);
                assert!(!l.direction.is_empty() && !l.trigger.is_empty());
            }
            let mut keys: Vec<_> = p.lines.iter().map(|l| l.key).collect();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), p.lines.len(), "{}: duplicate keys", p.who);
        }
    }

    #[test]
    fn files_and_bank_keys() {
        assert_eq!(file_for("goldenmane", "kill.2"), "voice/goldenmane/kill.2.opus");
        assert_eq!(bank_key("goldenmane", "kill.2"), "voice/goldenmane/kill");
        assert_eq!(file_for("announcer", "first_blood"), "announcer/first_blood.opus");
        // the file's key, read back the way the soundbank reads it
        let root = std::path::Path::new("/r");
        let (k, take) = crate::soundbank::key_of(root, &root.join(file_for("goldenmane", "kill.2"))).unwrap();
        assert_eq!((k.as_str(), take), ("voice/goldenmane/kill", 2));
    }
}
