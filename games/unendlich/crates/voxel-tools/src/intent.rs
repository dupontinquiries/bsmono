//! Intent: when may the world itself be changed?
//!
//! `/world hotter` exists, and a model that knows it exists will reach
//! for it. Asked for "a garden" on a glacier it will helpfully warm the
//! glacier, and the player who wanted a hardy alpine garden now has a
//! melted one and no idea why.
//!
//! THE DISCIPLINE
//!
//! The land is not the paintbrush's to reshape. A build adapts to where
//! it is — that is the entire point of the ambient and scene layers,
//! and a model that can rewrite the climate will stop adapting and
//! start flattening. Changing the world is reserved for the player
//! saying so, explicitly, about the environment rather than about a
//! building.
//!
//! TWO KEYS
//!
//! A prompt rule alone will not hold this. Prompts are advisory and a
//! small model under pressure drops the advisory parts first. So the
//! model may PROPOSE a world change, and the player's own words must
//! AUTHORIZE it: `authorize` reads the prose, and a world op with no
//! corroborating imperative is refused with the reason.
//!
//! The asymmetry is deliberate. A refused world change costs one
//! rephrase. An unrequested one changes a biome the player chose to
//! build in, and `/undo` restores blocks — it does not restore a
//! climate field the engine has since spread and settled.

/// What the player explicitly asked to be done to the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldIntent {
    Hotter,
    Colder,
    Wetter,
    Drier,
    Flood,
    Freeze,
}

impl WorldIntent {
    pub fn verb(self) -> &'static str {
        match self {
            WorldIntent::Hotter => "hotter",
            WorldIntent::Colder => "colder",
            WorldIntent::Wetter => "wetter",
            WorldIntent::Drier => "drier",
            WorldIntent::Flood => "flood",
            WorldIntent::Freeze => "freeze",
        }
    }
    pub fn parse(v: &str) -> Option<Self> {
        Some(match v.trim().to_lowercase().as_str() {
            "hotter" | "warmer" | "heat" => WorldIntent::Hotter,
            "colder" | "cooler" | "cool" => WorldIntent::Colder,
            "wetter" | "humid" | "rain" => WorldIntent::Wetter,
            "drier" | "dryer" | "dry" => WorldIntent::Drier,
            "flood" | "fill" => WorldIntent::Flood,
            "freeze" | "frozen" => WorldIntent::Freeze,
            _ => return None,
        })
    }
}

/// Why a world change was refused.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub message: String,
}

/// Does the player's prose explicitly authorize changing the world?
///
/// The test is deliberately narrow: an IMPERATIVE aimed at the
/// ENVIRONMENT. Two halves, both required.
///
/// * A command verb — make/turn/set/render, or the bare adjective as an
///   order ("hotter"), or a direct verb like "flood".
/// * A target that is the place, not a thing being built. "make it
///   warmer" passes; "make a warm-looking hut" does not, because the
///   adjective is describing a building.
///
/// Everything ambiguous fails. A player who meant it will say it again
/// more plainly; a player who did not will never know how close they
/// came to losing their biome.
pub fn authorize(prose: &str) -> Option<WorldIntent> {
    let p = prose.to_lowercase();
    let p = p.trim();

    // A request that names something to BUILD is a build request, even
    // if it also mentions weather. "a hut for the cold" is a hut.
    const BUILD_NOUNS: &[&str] = &[
        "house", "hut", "tower", "wall", "roof", "bridge", "path", "castle",
        "village", "farm", "garden", "shed", "cabin", "temple", "statue",
        "fence", "dock", "mine", "tunnel", "stairs", "floor", "room",
    ];
    let names_a_build = BUILD_NOUNS.iter().any(|n| contains_word(&p, n));

    // The environment, named as the thing being acted on.
    // "it" is deliberately NOT here. It is the pronoun in the canonical
    // authorized phrase ("make it warmer"), so counting it as naming
    // the place makes every build sentence containing "it" look like an
    // environmental order -- "make it warmer inside the house" passed
    // the gate on the first version.
    const PLACE_WORDS: &[&str] = &[
        "here", "this area", "the area", "this place", "the place",
        "the land", "this land", "the ground", "the climate", "the weather",
        "this biome", "the biome", "the valley", "this valley", "the basin",
        "everything", "the region", "this region", "the world",
    ];
    let names_the_place = PLACE_WORDS.iter().any(|w| p.contains(w));

    // Explicit imperatives, each paired with what it would do.
    const IMPERATIVES: &[(&str, WorldIntent)] = &[
        ("make it hotter", WorldIntent::Hotter),
        ("make it warmer", WorldIntent::Hotter),
        ("make it warm", WorldIntent::Hotter),
        ("warm it up", WorldIntent::Hotter),
        ("heat it up", WorldIntent::Hotter),
        ("turn up the heat", WorldIntent::Hotter),
        ("make it colder", WorldIntent::Colder),
        ("make it cooler", WorldIntent::Colder),
        ("make it cold", WorldIntent::Colder),
        ("cool it down", WorldIntent::Colder),
        ("make it wetter", WorldIntent::Wetter),
        ("make it rain", WorldIntent::Wetter),
        ("make it humid", WorldIntent::Wetter),
        ("make it drier", WorldIntent::Drier),
        ("make it dryer", WorldIntent::Drier),
        ("dry it out", WorldIntent::Drier),
        ("make it snow", WorldIntent::Freeze),
        ("make it snowy", WorldIntent::Freeze),
        ("freeze it", WorldIntent::Freeze),
        ("freeze this", WorldIntent::Freeze),
        ("freeze the", WorldIntent::Freeze),
        ("flood it", WorldIntent::Flood),
        ("flood this", WorldIntent::Flood),
        ("flood the", WorldIntent::Flood),
        ("fill it with water", WorldIntent::Flood),
        ("drown", WorldIntent::Flood),
    ];
    for (phrase, intent) in IMPERATIVES {
        if p.contains(phrase) {
            // Even an explicit phrase loses to a named build: "flood the
            // moat around my castle" is a castle with a moat, and the
            // model should dig one rather than raise the water table.
            if names_a_build && !names_the_place { return None; }
            return Some(*intent);
        }
    }

    // A bare adjective as an order: "hotter", "colder here".
    const BARE: &[(&str, WorldIntent)] = &[
        ("hotter", WorldIntent::Hotter), ("warmer", WorldIntent::Hotter),
        ("colder", WorldIntent::Colder), ("cooler", WorldIntent::Colder),
        ("wetter", WorldIntent::Wetter), ("drier", WorldIntent::Drier),
    ];
    for (word, intent) in BARE {
        // Short and imperative: "colder here", not a sentence that
        // happens to contain the word.
        if p.split_whitespace().count() <= 3 && contains_word(&p, word) && !names_a_build {
            return Some(*intent);
        }
    }
    let _ = names_the_place;
    None
}

/// Check a world op the MODEL produced against what the player said.
///
/// This is the second key. The model may have good reasons; it does not
/// get to act on them alone.
pub fn corroborate(prose: &str, proposed: WorldIntent) -> Result<WorldIntent, Refusal> {
    match authorize(prose) {
        Some(a) if a == proposed => Ok(a),
        Some(a) => Err(Refusal { message: format!(
            "the plan asked to make the world {}, but you asked to make it {} -- \
             ignoring the world change; say it plainly if you meant it",
            proposed.verb(), a.verb()) }),
        None => Err(Refusal { message: format!(
            "the plan wanted to change the world ({}), but you asked for a build, \
             not a change to the land. Builds adapt to where they are. \
             Say \"make it {}\" if you really want the climate changed.",
            proposed.verb(), proposed.verb()) }),
    }
}

fn contains_word(hay: &str, needle: &str) -> bool {
    hay.split(|c: char| !c.is_alphanumeric()).any(|w| w == needle)
}

/// The prompt section teaching the discipline. Advisory, and backed by
/// `corroborate` because advisory is not enough.
pub const WORLD_DISCIPLINE: &str = "\
CHANGING THE WORLD ITSELF

You may emit `world <hotter|colder|wetter|drier|flood|freeze>` to change
the climate or water, but almost never should.

A build ADAPTS to where it is. On a glacier, build for a glacier: thick
walls, a steep roof, spruce and stone. Do not warm the glacier. The
player chose to stand there.

Emit a world line ONLY when the player explicitly told you to change the
environment -- \"make it warmer here\", \"flood this valley\", \"freeze
the lake\". A request for a building is never such an instruction, even
when the player complains about the conditions: \"it is freezing, build
me a shelter\" asks for a shelter.

If you emit one without being asked, it will be refused and the rest of
your plan will still run.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_environmental_imperatives_are_authorized() {
        for (prose, want) in [
            ("make it warmer here", WorldIntent::Hotter),
            ("make it hotter", WorldIntent::Hotter),
            ("warm it up a bit", WorldIntent::Hotter),
            ("make it colder", WorldIntent::Colder),
            ("cool it down please", WorldIntent::Colder),
            ("flood this valley", WorldIntent::Flood),
            ("flood the basin", WorldIntent::Flood),
            ("freeze the lake", WorldIntent::Freeze),
            ("make it snow", WorldIntent::Freeze),
            ("make it rain", WorldIntent::Wetter),
            ("dry it out", WorldIntent::Drier),
            ("colder here", WorldIntent::Colder),
            ("hotter", WorldIntent::Hotter),
        ] {
            assert_eq!(authorize(prose), Some(want), "{prose:?} should authorize");
        }
    }

    #[test]
    fn build_requests_never_authorize_a_world_change() {
        // The failure this module exists to prevent: asked for a garden
        // on a glacier, warm the glacier.
        for prose in [
            "a garden",
            "build a garden here",
            "a warm-looking cabin",
            "a hut for the cold",
            "it is freezing, build me a shelter",
            "a house with a fireplace to keep warm",
            "a village by the water",
            "somewhere to shelter from the rain",
            "a cold storage room",
            "a snowy-looking tower",
            "make a warm cosy cabin",
        ] {
            assert_eq!(authorize(prose), None, "{prose:?} must NOT authorize a world change");
        }
    }

    #[test]
    fn an_explicit_phrase_still_loses_to_a_named_build() {
        // "flood the moat around my castle" is a castle with a moat: dig
        // one, do not raise the water table.
        assert_eq!(authorize("flood the moat around my castle"), None);
        assert_eq!(authorize("make it warmer inside the house"), None);
        // but naming the PLACE as well brings it back
        assert_eq!(authorize("flood the valley, then a castle on the island"),
                   Some(WorldIntent::Flood));
    }

    #[test]
    fn ambiguity_fails_closed() {
        // A player who meant it will say it again more plainly; a player
        // who did not will never know how close they came.
        for prose in [
            "it's too cold here",
            "this place could be nicer",
            "I wish it were warmer",
            "the weather is miserable",
            "too much water around here",
        ] {
            assert_eq!(authorize(prose), None, "{prose:?} is a complaint, not an order");
        }
    }

    #[test]
    fn corroboration_refuses_an_uninvited_world_change_and_says_why() {
        let e = corroborate("a garden", WorldIntent::Hotter).unwrap_err();
        assert!(e.message.contains("adapt"), "{}", e.message);
        assert!(e.message.contains("make it hotter"),
                "and tells the player how to ask for it: {}", e.message);
    }

    #[test]
    fn corroboration_refuses_a_mismatch() {
        // The player asked for cold; the model proposed heat.
        let e = corroborate("make it colder", WorldIntent::Hotter).unwrap_err();
        assert!(e.message.contains("hotter") && e.message.contains("colder"), "{}", e.message);
    }

    #[test]
    fn corroboration_allows_what_was_asked_for() {
        assert_eq!(corroborate("flood this valley", WorldIntent::Flood),
                   Ok(WorldIntent::Flood));
    }

    #[test]
    fn the_discipline_text_is_prompt_safe() {
        assert!(WORLD_DISCIPLINE.is_ascii(), "tokenizer portability");
        assert!(WORLD_DISCIPLINE.contains("ADAPTS"), "the rule is stated positively first");
        assert!(WORLD_DISCIPLINE.contains("refused"),
                "and the model is told the gate exists, so a refusal is not a surprise");
    }

    #[test]
    fn verbs_round_trip() {
        for i in [WorldIntent::Hotter, WorldIntent::Colder, WorldIntent::Wetter,
                  WorldIntent::Drier, WorldIntent::Flood, WorldIntent::Freeze] {
            assert_eq!(WorldIntent::parse(i.verb()), Some(i));
        }
        assert_eq!(WorldIntent::parse("sideways"), None);
    }
}
