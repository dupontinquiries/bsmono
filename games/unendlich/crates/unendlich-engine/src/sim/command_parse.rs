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
    let mut tokens = line.trim().split_whitespace();
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
        matches!(self, Command::Recipes | Command::Help)
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
}
