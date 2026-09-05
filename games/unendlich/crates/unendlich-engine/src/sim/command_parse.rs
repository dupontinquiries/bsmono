//! command_parse — turn a chat line (`/find water`, `/settime night`) into a
//! typed [`Command`]. Pure tokenize + match: split on whitespace, lower-case the
//! verb and its keyword arguments (ids and item names keep their case), match to
//! a variant, and fall back to [`Command::Unknown`] for anything that doesn't fit
//! — never a panic. A parsed command knows whether dispatching it should open a
//! GUI ([`Command::opens_gui`]) so the bin can route `/recipes` to the recipe
//! book instead of running it headless.
//!
//! It is deliberately self-contained (rustc-testable in isolation, R1-pure): the
//! parser owns its own small command vocabulary rather than reaching into a
//! shared registry. Round-tripping is the contract that keeps it honest — every
//! known command renders back to a canonical string that re-parses to the same
//! value (see [`Command::to_command_string`] and `prop_round_trip`).

/// What `/find` is looking for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FindKind {
    Structure,
    Mob,
    Biome,
    Water,
}

/// What `/land` was asked to do.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LandOp {
    /// Bare `/land`: what is claimed where I stand.
    Here,
    Info(String),
    Add { id: String, player: String },
    /// `/land claim <name>` — claim the last build.
    Claim(String),
}

/// `/anim` reveal profile. Mirrors `voxel_tools::anim::Profile`; kept as
/// its own enum so this parser stays dependency-free.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnimProfile {
    GroundUp,
    Physics,
    Random,
    Instant,
}

impl AnimProfile {
    fn keyword(self) -> &'static str {
        match self {
            AnimProfile::GroundUp => "ground-up",
            AnimProfile::Physics => "physics",
            AnimProfile::Random => "random",
            AnimProfile::Instant => "instant",
        }
    }
}

/// `/focus` tier adjustment.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FocusAdjust {
    Bigger,
    Smaller,
}

/// Whitelist mutation verb.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WhitelistOp {
    Add,
    Remove,
}

/// Player interaction mode for `/gamemode`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameMode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl GameMode {
    /// r711: bridge to the atomic trait system (`sim::gamemode_traits`). The doc's
    /// four base modes are Survival/Adventure/Creative/CrashDummy; the engine's
    /// fourth mode is Spectator, which the doc frames as pure overlays — so we
    /// compose it from a Creative base plus noclip/vision/invisibility and no
    /// build. Systems can then gate behaviour on `game_mode.traits().can_build()`,
    /// `.takes_damage()`, `.mobs_target()`, `.no_collision()`, etc. instead of
    /// matching the mode name.
    pub fn traits(self) -> crate::sim::gamemode_traits::TraitSet {
        use crate::sim::gamemode_traits::{Mode, Trait};
        match self {
            GameMode::Survival => Mode::Survival.traits(),
            GameMode::Creative => Mode::Creative.traits(),
            GameMode::Adventure => Mode::Adventure.traits(),
            // Ghost observer: fly + noclip + immune + ignored + full vision +
            // hidden, with no build and outside the survival system.
            GameMode::Spectator => Mode::Creative
                .traits()
                .without(Trait::Build)
                .with(Trait::PassThrough)
                .with(Trait::Vision)
                .with(Trait::InvisToPlayers),
        }
    }
}

/// A parsed chat command. `Unknown` is the catch-all for unrecognised verbs,
/// bad arity, or unparseable arguments — parsing is total and never panics.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Command {
    /// `/find <structure|mob|biome|water>` — locate the nearest target of a kind.
    Find(FindKind),
    /// `/locate` — report the player's own coordinates.
    Locate,
    /// `/settime <ticks|day|noon|sunset|night|midnight|sunrise>` — set world time.
    SetTime(i64),
    /// `/whitelist <add|remove> <id>` — edit the access whitelist.
    Whitelist { op: WhitelistOp, id: String },
    /// `/give <item> [count]` — grant an item stack (count defaults to 1).
    Give { item: String, count: u32 },
    /// `/build <prose>` — hand a natural-language build request to the
    /// LLM paintbrush. The text is kept verbatim (not lower-cased) because
    /// it is prose, not a keyword. Opens the plan-preview GUI.
    Build(String),
    /// `/plan` — show the pending build plan (arithmetic echo + dry run)
    /// without committing it.
    ShowPlan,
    /// `/commit` — apply the pending build plan.
    CommitPlan,
    /// `/undo` — reverse the last committed build as a unit.
    UndoBuild,
    /// `/focus [bigger|smaller]` — report or adjust the placement focus tier.
    Focus(Option<FocusAdjust>),
    /// `/build enableVoice` / `/build disableVoice` — voice live mode.
    BuildVoice(bool),
    /// `/anim <ground-up|physics|random|instant>` — how builds reveal.
    Anim(AnimProfile),
    /// `/model` — list local models; `/model <name>` — load one. The name
    /// keeps its case (paths are case-sensitive) and may be partial.
    Model(Option<String>),
    /// `/keys` — print the keybindings into the chat log.
    Keys,
    /// `/cancel` — drop a model request in flight.
    Cancel,
    /// `/repair` — ask the model to fix only the lines it got wrong.
    Repair,
    /// `/deferred [drop]` — blocks waiting for their chunk to load.
    Deferred { drop: bool },
    /// `/land` — list claims here; `/land info <id>`; `/land add <id> <player>`.
    /// Player names and workset ids keep their case.
    Land(LandOp),
    /// `/shot <frame|orbit|reveal|fly> [id]` — camera presentation of a
    /// build: the hero framing, an orbit, a pull-back reveal, or a
    /// fly-through along its paths.
    Shot { kind: String, id: Option<String> },
    /// `/world <verb> [args]` — drive the engine's own systems:
    /// hotter, colder, wetter, drier, flood, freeze.
    World { verb: String, arg: Option<String> },
    /// `/provider` — show the current one; `/provider <spec>` — switch.
    /// Specs: `fixture`, `script`, `ollama[:host[:model]]`,
    /// `openai[:host]`. Case preserved: a model name is data.
    Provider(Option<String>),
    /// `/gamemode <survival|creative|adventure|spectator>`.
    Gamemode(GameMode),
    /// `/recipes` — open the recipe book GUI.
    Recipes,
    /// `/help` — open the help GUI.
    Help,
    /// Anything that didn't parse to a known command.
    Unknown,
}

/// Map a `/settime` argument to a tick value: a literal integer, or one of the
/// named times of day. Returns [`Command::Unknown`] for an unrecognised keyword.
fn parse_set_time(arg: &str) -> Command {
    if let Ok(n) = arg.parse::<i64>() {
        return Command::SetTime(n);
    }
    let ticks = match arg.to_ascii_lowercase().as_str() {
        "sunrise" | "dawn" => 0,
        "day" | "morning" => 1000,
        "noon" => 6000,
        "sunset" | "dusk" => 12000,
        "night" => 13000,
        "midnight" => 18000,
        _ => return Command::Unknown,
    };
    Command::SetTime(ticks)
}

fn parse_find_kind(arg: &str) -> Option<FindKind> {
    match arg.to_ascii_lowercase().as_str() {
        "structure" => Some(FindKind::Structure),
        "mob" => Some(FindKind::Mob),
        "biome" => Some(FindKind::Biome),
        "water" => Some(FindKind::Water),
        _ => None,
    }
}

fn parse_whitelist_op(arg: &str) -> Option<WhitelistOp> {
    match arg.to_ascii_lowercase().as_str() {
        "add" => Some(WhitelistOp::Add),
        "remove" | "rm" => Some(WhitelistOp::Remove),
        _ => None,
    }
}

fn parse_game_mode(arg: &str) -> Option<GameMode> {
    match arg.to_ascii_lowercase().as_str() {
        "survival" | "s" => Some(GameMode::Survival),
        "creative" | "c" => Some(GameMode::Creative),
        "adventure" | "a" => Some(GameMode::Adventure),
        "spectator" | "sp" => Some(GameMode::Spectator),
        _ => None,
    }
}

/// Parse a single chat line into a [`Command`]. Total and deterministic:
/// leading/trailing whitespace is trimmed, the verb and keyword arguments are
/// matched case-insensitively, and anything unrecognised yields
/// [`Command::Unknown`] rather than panicking.
pub fn parse(line: &str) -> Command {
    // `/build` is the one verb whose argument is prose: keep it verbatim,
    // including case and internal spacing, before the tokenizer runs.
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("/build") {
        if rest.is_empty() {
            return Command::Unknown;
        }
        if rest.starts_with(char::is_whitespace) {
            let text = rest.trim();
            // the two control words are not prose
            match text.to_ascii_lowercase().as_str() {
                "" => return Command::Unknown,
                "enablevoice" => return Command::BuildVoice(true),
                "disablevoice" => return Command::BuildVoice(false),
                _ => return Command::Build(text.to_string()),
            }
        }
    }
    let mut tokens = trimmed.split_whitespace();
    let verb = match tokens.next() {
        Some(v) => v.to_ascii_lowercase(),
        None => return Command::Unknown,
    };
    let args: Vec<&str> = tokens.collect();

    match verb.as_str() {
        "/find" => match args.as_slice() {
            [kind] => match parse_find_kind(kind) {
                Some(k) => Command::Find(k),
                None => Command::Unknown,
            },
            _ => Command::Unknown,
        },
        "/plan" => if args.is_empty() { Command::ShowPlan } else { Command::Unknown },
        "/commit" => if args.is_empty() { Command::CommitPlan } else { Command::Unknown },
        "/undo" => if args.is_empty() { Command::UndoBuild } else { Command::Unknown },
        "/focus" => match args.as_slice() {
            [] => Command::Focus(None),
            [a] => match a.to_ascii_lowercase().as_str() {
                "bigger" | "expand" | "up" => Command::Focus(Some(FocusAdjust::Bigger)),
                "smaller" | "shrink" | "down" => Command::Focus(Some(FocusAdjust::Smaller)),
                _ => Command::Unknown,
            },
            _ => Command::Unknown,
        },
        "/keys" => if args.is_empty() { Command::Keys } else { Command::Unknown },
        "/cancel" => if args.is_empty() { Command::Cancel } else { Command::Unknown },
        "/repair" => if args.is_empty() { Command::Repair } else { Command::Unknown },
        "/deferred" => match args.as_slice() {
            [] => Command::Deferred { drop: false },
            [d] if d.eq_ignore_ascii_case("drop") => Command::Deferred { drop: true },
            _ => Command::Unknown,
        },
        "/land" => match args.as_slice() {
            [] => Command::Land(LandOp::Here),
            [sub, rest @ ..] => match (sub.to_ascii_lowercase().as_str(), rest) {
                ("info", [id]) => Command::Land(LandOp::Info((*id).to_string())),
                ("add", [id, player]) => Command::Land(LandOp::Add {
                    id: (*id).to_string(), player: (*player).to_string() }),
                // A name may have spaces: `/land claim the big hall`.
                ("claim", n) if !n.is_empty() => Command::Land(LandOp::Claim(n.join(" "))),
                _ => Command::Unknown,
            },
        },
        "/shot" => match args.as_slice() {
            [k] => Command::Shot { kind: k.to_ascii_lowercase(), id: None },
            [k, id] => Command::Shot { kind: k.to_ascii_lowercase(), id: Some((*id).to_string()) },
            _ => Command::Unknown,
        },
        "/world" => match args.as_slice() {
            [v] => Command::World { verb: v.to_ascii_lowercase(), arg: None },
            [v, a] => Command::World { verb: v.to_ascii_lowercase(),
                                       arg: Some((*a).to_string()) },
            _ => Command::Unknown,
        },
        "/provider" => match args.as_slice() {
            [] => Command::Provider(None),
            [spec] => Command::Provider(Some((*spec).to_string())),
            _ => Command::Unknown,
        },
        "/model" => match args.as_slice() {
            [] => Command::Model(None),
            [name] => Command::Model(Some((*name).to_string())),
            _ => Command::Unknown,
        },
        "/anim" => match args.as_slice() {
            [p] => match p.to_ascii_lowercase().as_str() {
                "ground-up" | "groundup" | "ground" => Command::Anim(AnimProfile::GroundUp),
                "physics" | "fall" => Command::Anim(AnimProfile::Physics),
                "random" | "shuffle" => Command::Anim(AnimProfile::Random),
                "instant" | "now" => Command::Anim(AnimProfile::Instant),
                _ => Command::Unknown,
            },
            _ => Command::Unknown,
        },
        "/locate" => {
            if args.is_empty() {
                Command::Locate
            } else {
                Command::Unknown
            }
        }
        "/settime" => match args.as_slice() {
            [t] => parse_set_time(t),
            _ => Command::Unknown,
        },
        "/whitelist" => match args.as_slice() {
            [op, id] => match parse_whitelist_op(op) {
                Some(op) => Command::Whitelist {
                    op,
                    id: (*id).to_string(),
                },
                None => Command::Unknown,
            },
            _ => Command::Unknown,
        },
        "/give" => match args.as_slice() {
            [item] => Command::Give {
                item: (*item).to_string(),
                count: 1,
            },
            [item, count] => match count.parse::<u32>() {
                Ok(c) => Command::Give {
                    item: (*item).to_string(),
                    count: c,
                },
                Err(_) => Command::Unknown,
            },
            _ => Command::Unknown,
        },
        "/gamemode" => match args.as_slice() {
            [mode] => match parse_game_mode(mode) {
                Some(m) => Command::Gamemode(m),
                None => Command::Unknown,
            },
            _ => Command::Unknown,
        },
        "/recipes" => {
            if args.is_empty() {
                Command::Recipes
            } else {
                Command::Unknown
            }
        }
        "/help" => {
            if args.is_empty() {
                Command::Help
            } else {
                Command::Unknown
            }
        }
        _ => Command::Unknown,
    }
}

impl FindKind {
    fn keyword(self) -> &'static str {
        match self {
            FindKind::Structure => "structure",
            FindKind::Mob => "mob",
            FindKind::Biome => "biome",
            FindKind::Water => "water",
        }
    }
}

impl WhitelistOp {
    fn keyword(self) -> &'static str {
        match self {
            WhitelistOp::Add => "add",
            WhitelistOp::Remove => "remove",
        }
    }
}

impl GameMode {
    fn keyword(self) -> &'static str {
        match self {
            GameMode::Survival => "survival",
            GameMode::Creative => "creative",
            GameMode::Adventure => "adventure",
            GameMode::Spectator => "spectator",
        }
    }
}

impl Command {
    /// Whether dispatching this command should open an in-game GUI rather than
    /// run headless. The bin uses this to route `/recipes`/`/help` to a panel.
    pub fn opens_gui(&self) -> bool {
        matches!(self, Command::Recipes | Command::Help
                     | Command::Build(_) | Command::ShowPlan | Command::Model(_)
                     | Command::Land(_))
    }

    /// Render the command back to its canonical `/verb args` string. Returns
    /// `None` for [`Command::Unknown`]. The canonical form re-parses to an equal
    /// command (`SetTime` always renders numerically), which is the round-trip
    /// the property test pins down.
    pub fn to_command_string(&self) -> Option<String> {
        match self {
            Command::Find(k) => Some(format!("/find {}", k.keyword())),
            Command::Locate => Some("/locate".to_string()),
            Command::SetTime(t) => Some(format!("/settime {t}")),
            Command::Whitelist { op, id } => Some(format!("/whitelist {} {}", op.keyword(), id)),
            Command::Give { item, count } => Some(format!("/give {item} {count}")),
            Command::Build(text) => Some(format!("/build {text}")),
            Command::ShowPlan => Some("/plan".to_string()),
            Command::CommitPlan => Some("/commit".to_string()),
            Command::UndoBuild => Some("/undo".to_string()),
            Command::Focus(None) => Some("/focus".to_string()),
            Command::Focus(Some(FocusAdjust::Bigger)) => Some("/focus bigger".to_string()),
            Command::Focus(Some(FocusAdjust::Smaller)) => Some("/focus smaller".to_string()),
            Command::BuildVoice(true) => Some("/build enableVoice".to_string()),
            Command::BuildVoice(false) => Some("/build disableVoice".to_string()),
            Command::Anim(p) => Some(format!("/anim {}", p.keyword())),
            Command::Keys => Some("/keys".to_string()),
            Command::Cancel => Some("/cancel".to_string()),
            Command::Repair => Some("/repair".to_string()),
            Command::Deferred { drop: false } => Some("/deferred".to_string()),
            Command::Deferred { drop: true } => Some("/deferred drop".to_string()),
            Command::Land(LandOp::Here) => Some("/land".to_string()),
            Command::Land(LandOp::Info(id)) => Some(format!("/land info {id}")),
            Command::Land(LandOp::Add { id, player }) => Some(format!("/land add {id} {player}")),
            Command::Land(LandOp::Claim(n)) => Some(format!("/land claim {n}")),
            Command::Shot { kind, id: None } => Some(format!("/shot {kind}")),
            Command::Shot { kind, id: Some(i) } => Some(format!("/shot {kind} {i}")),
            Command::World { verb, arg: None } => Some(format!("/world {verb}")),
            Command::World { verb, arg: Some(a) } => Some(format!("/world {verb} {a}")),
            Command::Provider(None) => Some("/provider".to_string()),
            Command::Provider(Some(s)) => Some(format!("/provider {s}")),
            Command::Model(None) => Some("/model".to_string()),
            Command::Model(Some(n)) => Some(format!("/model {n}")),
            Command::Gamemode(m) => Some(format!("/gamemode {}", m.keyword())),
            Command::Recipes => Some("/recipes".to_string()),
            Command::Help => Some("/help".to_string()),
            Command::Unknown => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn parses_each_known_verb() {
        assert_eq!(parse("/find water"), Command::Find(FindKind::Water));
        assert_eq!(parse("/find Structure"), Command::Find(FindKind::Structure));
        assert_eq!(parse("/locate"), Command::Locate);
        assert_eq!(parse("/settime 6000"), Command::SetTime(6000));
        assert_eq!(parse("/settime night"), Command::SetTime(13000));
        assert_eq!(
            parse("/whitelist add Steve"),
            Command::Whitelist {
                op: WhitelistOp::Add,
                id: "Steve".to_string()
            }
        );
        assert_eq!(
            parse("/give torch 8"),
            Command::Give {
                item: "torch".to_string(),
                count: 8
            }
        );
        assert_eq!(
            parse("/give torch"),
            Command::Give {
                item: "torch".to_string(),
                count: 1
            }
        );
        assert_eq!(parse("/gamemode creative"), Command::Gamemode(GameMode::Creative));
        assert_eq!(parse("/recipes"), Command::Recipes);
        assert_eq!(parse("/help"), Command::Help);
    }

    #[test]
    fn case_and_whitespace_insensitive_verb() {
        assert_eq!(parse("   /FIND   water  "), Command::Find(FindKind::Water));
        // item names keep their case; only the verb/keywords fold.
        assert_eq!(
            parse("/give DiamondSword"),
            Command::Give {
                item: "DiamondSword".to_string(),
                count: 1
            }
        );
    }

    #[test]
    fn garbage_is_unknown_never_panics() {
        for s in &[
            "",
            "   ",
            "hello there",
            "/",
            "/find",
            "/find lava",
            "/locate now",
            "/settime soon",
            "/settime",
            "/whitelist add",
            "/whitelist toggle Steve",
            "/give torch notanumber",
            "/give",
            "/gamemode hardcore",
            "/recipes all",
            "/unknownverb a b c",
        ] {
            assert_eq!(parse(s), Command::Unknown, "expected Unknown for {s:?}");
        }
    }

    #[test]
    fn opens_gui_only_for_panel_commands() {
        assert!(Command::Recipes.opens_gui());
        assert!(Command::Help.opens_gui());
        assert!(!Command::Locate.opens_gui());
        assert!(!Command::Find(FindKind::Mob).opens_gui());
        assert!(!Command::Unknown.opens_gui());
    }

    #[test]
    fn unknown_has_no_canonical_string() {
        assert_eq!(Command::Unknown.to_command_string(), None);
    }

    // Deterministic generator of a valid command from a seed (no RNG crate).
    fn gen_command(seed: &mut u32) -> Command {
        let ids = ["a", "steve_01", "z9", "torch", "minecraft_stone", "Alex"];
        match lcg(seed) % 9 {
            0 => Command::Find(match lcg(seed) % 4 {
                0 => FindKind::Structure,
                1 => FindKind::Mob,
                2 => FindKind::Biome,
                _ => FindKind::Water,
            }),
            1 => Command::Locate,
            2 => Command::SetTime((lcg(seed) % 24000) as i64),
            3 => Command::Whitelist {
                op: if lcg(seed) % 2 == 0 {
                    WhitelistOp::Add
                } else {
                    WhitelistOp::Remove
                },
                id: ids[(lcg(seed) as usize) % ids.len()].to_string(),
            },
            4 => Command::Give {
                item: ids[(lcg(seed) as usize) % ids.len()].to_string(),
                count: 1 + lcg(seed) % 64,
            },
            5 => Command::Gamemode(match lcg(seed) % 4 {
                0 => GameMode::Survival,
                1 => GameMode::Creative,
                2 => GameMode::Adventure,
                _ => GameMode::Spectator,
            }),
            6 => Command::Recipes,
            7 => Command::Help,
            _ => Command::Find(FindKind::Biome),
        }
    }

    #[test]
    fn prop_round_trip() {
        let mut s = 0x1234_5678u32;
        for _ in 0..5000 {
            let cmd = gen_command(&mut s);
            let text = match cmd.to_command_string() {
                Some(t) => t,
                None => continue,
            };
            assert_eq!(parse(&text), cmd, "round-trip failed for {text:?}");
        }
    }

    #[test]
    fn prop_parse_is_total_and_deterministic() {
        let alphabet: &[u8] = b"abc /0123_xyz\tstructure addnight";
        let mut s = 0xC0FF_EE11u32;
        for _ in 0..5000 {
            let len = (lcg(&mut s) % 24) as usize;
            let bytes: Vec<u8> = (0..len)
                .map(|_| alphabet[(lcg(&mut s) as usize) % alphabet.len()])
                .collect();
            let line = String::from_utf8_lossy(&bytes).to_string();
            // total: must not panic, and must agree on repeated calls (determinism).
            let a = parse(&line);
            let b = parse(&line);
            assert_eq!(a, b);
        }
    }

    #[test]
    fn build_keeps_prose_verbatim() {
        // case and spacing inside the request must survive: it is
        // prose for a model, not a keyword for the tokenizer
        let c = parse("/build a Small stone tower  with a dome");
        assert_eq!(c, Command::Build("a Small stone tower  with a dome".to_string()));
        assert!(c.opens_gui(), "a build request opens the plan preview");
        assert_eq!(parse("/build"), Command::Unknown, "empty request is not a build");
        assert_eq!(parse("/buildx"), Command::Unknown, "prefix must be a whole verb");
    }

    #[test]
    fn plan_lifecycle_verbs_parse_and_round_trip() {
        for (line, want) in [
            ("/plan", Command::ShowPlan),
            ("/commit", Command::CommitPlan),
            ("/undo", Command::UndoBuild),
            ("/focus", Command::Focus(None)),
            ("/focus bigger", Command::Focus(Some(FocusAdjust::Bigger))),
            ("/focus SMALLER", Command::Focus(Some(FocusAdjust::Smaller))),
        ] {
            let c = parse(line);
            assert_eq!(c, want, "{line}");
            let s = c.to_command_string().expect("canonical form");
            assert_eq!(parse(&s), c, "round trip of {s}");
        }
        assert_eq!(parse("/focus sideways"), Command::Unknown);
        assert_eq!(parse("/commit now"), Command::Unknown, "extra args are not tolerated");
    }

    #[test]
    fn build_round_trips_through_its_canonical_string() {
        let c = Command::Build("three oak trees by the pond".into());
        let s = c.to_command_string().unwrap();
        assert_eq!(parse(&s), c);
    }

    #[test]
    fn voice_toggle_and_anim_parse_and_round_trip() {
        assert_eq!(parse("/build enableVoice"), Command::BuildVoice(true));
        assert_eq!(parse("/build ENABLEVOICE"), Command::BuildVoice(true));
        assert_eq!(parse("/build disableVoice"), Command::BuildVoice(false));
        // and a request that merely CONTAINS the word is still prose
        assert_eq!(parse("/build enableVoice on a hill"),
                   Command::Build("enableVoice on a hill".to_string()));
        for (line, want) in [
            ("/anim physics", Command::Anim(AnimProfile::Physics)),
            ("/anim GROUND-UP", Command::Anim(AnimProfile::GroundUp)),
            ("/anim random", Command::Anim(AnimProfile::Random)),
            ("/anim instant", Command::Anim(AnimProfile::Instant)),
        ] {
            let c = parse(line);
            assert_eq!(c, want, "{line}");
            assert_eq!(parse(&c.to_command_string().unwrap()), c, "round trip {line}");
        }
        assert_eq!(parse("/anim"), Command::Unknown);
        assert_eq!(parse("/anim sideways"), Command::Unknown);
        for c in [Command::BuildVoice(true), Command::BuildVoice(false)] {
            assert_eq!(parse(&c.to_command_string().unwrap()), c);
        }
    }

    #[test]
    fn model_verb_lists_or_loads_and_preserves_case() {
        assert_eq!(parse("/model"), Command::Model(None));
        // paths are case-sensitive: the name must not be lower-cased
        assert_eq!(parse("/model Qwen2.5-7B"), Command::Model(Some("Qwen2.5-7B".to_string())));
        assert_eq!(parse("/model a b"), Command::Unknown, "one name only");
        for c in [Command::Model(None), Command::Model(Some("qwen".into()))] {
            assert_eq!(parse(&c.to_command_string().unwrap()), c, "round trip");
        }
        assert!(Command::Model(None).opens_gui(), "the model list is a panel");
    }

    #[test]
    fn provider_verb_parses_and_round_trips() {
        assert_eq!(parse("/provider"), Command::Provider(None));
        // a model name is data: case must survive
        assert_eq!(parse("/provider ollama:127.0.0.1:11434:Qwen2.5-Coder"),
                   Command::Provider(Some("ollama:127.0.0.1:11434:Qwen2.5-Coder".to_string())));
        assert_eq!(parse("/provider a b"), Command::Unknown, "one spec only");
        for c in [Command::Provider(None), Command::Provider(Some("fixture".into()))] {
            assert_eq!(parse(&c.to_command_string().unwrap()), c);
        }
    }

    #[test]
    fn land_verbs_parse_and_keep_their_case() {
        // Workset ids and player names are identifiers, not keywords.
        assert_eq!(parse("/land"), Command::Land(LandOp::Here));
        assert_eq!(parse("/land info w3"), Command::Land(LandOp::Info("w3".into())));
        assert_eq!(parse("/land add w3 KesslerD"),
                   Command::Land(LandOp::Add { id: "w3".into(), player: "KesslerD".into() }));
        // a claim name may have spaces
        assert_eq!(parse("/land claim the Big Hall"),
                   Command::Land(LandOp::Claim("the Big Hall".into())));
        assert_eq!(parse("/land add w3"), Command::Unknown, "add needs both");
        assert_eq!(parse("/land wat"), Command::Unknown);
        for c in [Command::Land(LandOp::Here),
                  Command::Land(LandOp::Info("w1".into())),
                  Command::Land(LandOp::Add { id: "w1".into(), player: "Ann".into() }),
                  Command::Land(LandOp::Claim("big hall".into()))] {
            assert_eq!(parse(&c.to_command_string().unwrap()), c, "round trip");
        }
    }

    #[test]
    fn world_verbs_parse_and_round_trip() {
        assert_eq!(parse("/world hotter"),
                   Command::World { verb: "hotter".into(), arg: None });
        assert_eq!(parse("/world flood 68"),
                   Command::World { verb: "flood".into(), arg: Some("68".into()) });
        assert_eq!(parse("/world HOTTER"),
                   Command::World { verb: "hotter".into(), arg: None }, "verb lower-cased");
        assert_eq!(parse("/world"), Command::Unknown);
        for c in [Command::World { verb: "freeze".into(), arg: None },
                  Command::World { verb: "flood".into(), arg: Some("70".into()) }] {
            assert_eq!(parse(&c.to_command_string().unwrap()), c);
        }
    }

    #[test]
    fn shot_verbs_parse_and_round_trip() {
        assert_eq!(parse("/shot orbit"), Command::Shot { kind: "orbit".into(), id: None });
        assert_eq!(parse("/shot reveal w3"),
                   Command::Shot { kind: "reveal".into(), id: Some("w3".into()) });
        assert_eq!(parse("/shot"), Command::Unknown);
        for c in [Command::Shot { kind: "frame".into(), id: None },
                  Command::Shot { kind: "fly".into(), id: Some("w2".into()) }] {
            assert_eq!(parse(&c.to_command_string().unwrap()), c);
        }
    }

    #[test]
    fn repair_and_deferred_parse_and_round_trip() {
        assert_eq!(parse("/repair"), Command::Repair);
        assert_eq!(parse("/deferred"), Command::Deferred { drop: false });
        assert_eq!(parse("/deferred DROP"), Command::Deferred { drop: true });
        assert_eq!(parse("/deferred keep"), Command::Unknown);
        for c in [Command::Repair, Command::Deferred { drop: true }] {
            assert_eq!(parse(&c.to_command_string().unwrap()), c);
        }
    }
}
