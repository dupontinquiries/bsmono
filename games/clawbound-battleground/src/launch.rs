// Launch options — how the game centre (games/bsgames) and gitmind start us.
//
// bsgames spawns `cargo run --release --manifest-path games/<dir>/Cargo.toml`
// from the monorepo root, optionally with `--quick-match`, and for a
// friend-session handoff sets three env vars (see bsgames/src/centre.rs):
//   BSGAMES_HANDOFF_ROLE  "host" | "join"
//   BSGAMES_HANDOFF_ME    "<id>|<display name>"
//   BSGAMES_HANDOFF_PEER  friend id (join only)
// Launched by hand with none of these, the game behaves exactly as before.
// Parsing is pure so it is tested; `app.rs` acts on the result.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandoffRole {
    Host,
    Join,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct LaunchOptions {
    /// `--quick-match`: skip the menu and character select, pick a random
    /// ancestor, straight into a 5v5.
    pub quick_match: bool,
    /// `--screenshot`: the windowless render harness.
    pub screenshot: bool,
    /// `--lan`: open straight onto the LOCAL NETWORK screen. What bsgames
    /// should launch with for a LAN session, and what the Xvfb check uses so
    /// it does not have to guess where a button is.
    pub lan: bool,
    /// `--rig-sheet`: windowless close-ups of posed ancestors, through the
    /// real draw path. Implies `screenshot`.
    pub rig_sheet: bool,
    /// `--ancestor <name|id>`: pick this one (implies quick match).
    pub ancestor: Option<String>,
    pub handoff: Option<HandoffRole>,
    /// our display name from bsgames, if handed off
    pub player_name: Option<String>,
    pub player_id: Option<String>,
    pub peer_id: Option<String>,
    /// `--map <jungle|pass|starwell|atlantis>` (or `CLAWBOUND_MAP`): which map to play. Chosen
    /// at launch because the terrain mesh is built with the renderer.
    pub map: crate::map::MapId,
    /// A custom game's rules (`rules`), from `--rules "level=6 cd=0.5"`
    /// (or `CLAWBOUND_RULES`), `--gods <lobby|team|open>` (or
    /// `CLAWBOUND_GODS`) and `--mirror` (or `CLAWBOUND_MIRROR=1`; implies
    /// `--gods open`). Default: a normal match.
    pub rules: crate::rules::MatchRules,
    /// Why a `--rules` text was not read, if it was not.
    pub rules_error: Option<String>,
}

impl LaunchOptions {
    pub fn parse<I: IntoIterator<Item = String>>(args: I, env: &dyn Fn(&str) -> Option<String>) -> LaunchOptions {
        let mut o = LaunchOptions::default();
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--quick-match" => o.quick_match = true,
                "--screenshot" => o.screenshot = true,
                "--lan" => o.lan = true,
                "--rig-sheet" => {
                    o.rig_sheet = true;
                    o.screenshot = true;
                }
                "--ancestor" => {
                    o.ancestor = it.next();
                    o.quick_match = true;
                }
                "--gods" => {
                    if let Some(g) = it.next().as_deref().and_then(crate::picks::GodLimit::from_key) {
                        o.rules.gods = g;
                    }
                }
                "--mirror" => o.rules.mirror = true,
                "--rules" => {
                    let text = it.next().unwrap_or_default();
                    if let Err(e) = o.rules.parse_onto(&text) {
                        o.rules_error = Some(e);
                    }
                }
                "--map" => {
                    if let Some(m) = it.next().as_deref().and_then(crate::map::MapId::from_key) {
                        o.map = m;
                    }
                }
                _ => {}
            }
        }
        match env("BSGAMES_HANDOFF_ROLE").as_deref() {
            Some("host") => o.handoff = Some(HandoffRole::Host),
            Some("join") => o.handoff = Some(HandoffRole::Join),
            _ => {}
        }
        if let Some(me) = env("BSGAMES_HANDOFF_ME") {
            let (id, name) = me.split_once('|').unwrap_or((me.as_str(), ""));
            o.player_id = Some(id.to_string());
            if !name.is_empty() {
                o.player_name = Some(name.to_string());
            }
        }
        o.peer_id = env("BSGAMES_HANDOFF_PEER");
        if let Some(m) = env("CLAWBOUND_MAP").as_deref().and_then(crate::map::MapId::from_key) {
            o.map = m;
        }
        if let Some(g) = env("CLAWBOUND_GODS").as_deref().and_then(crate::picks::GodLimit::from_key) {
            o.rules.gods = g;
        }
        o.rules.mirror |= env("CLAWBOUND_MIRROR").as_deref() == Some("1");
        if let Some(text) = env("CLAWBOUND_RULES") {
            if let Err(e) = o.rules.parse_onto(&text) {
                o.rules_error = Some(e);
            }
        }
        if o.rules.mirror {
            o.rules.gods = crate::picks::GodLimit::Open;
        }
        // a handoff means "we were sent here to play": don't sit at the menu
        if o.handoff.is_some() {
            o.quick_match = true;
        }
        o
    }

    pub fn from_process() -> LaunchOptions {
        let args: Vec<String> = std::env::args().skip(1).collect();
        LaunchOptions::parse(args, &|k| std::env::var(k).ok())
    }

    /// Roster id for `--ancestor`, by id number or (case-insensitive) name.
    pub fn ancestor_id(&self) -> Option<u32> {
        let a = self.ancestor.as_ref()?;
        if let Ok(n) = a.parse::<u32>() {
            return (n < crate::gods::ROSTER_LEN as u32).then_some(n);
        }
        crate::gods::ROSTER.iter().find(|d| d.name.eq_ignore_ascii_case(a)).map(|d| d.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn the_match_rules_come_from_the_flags_or_the_env() {
        use crate::picks::GodLimit;
        let o = LaunchOptions::parse(vec!["--gods".to_string(), "team".to_string()], &env_of(&[]));
        assert_eq!((o.rules.gods, o.rules.mirror), (GodLimit::Team, false));
        let o = LaunchOptions::parse(vec!["--mirror".to_string()], &env_of(&[]));
        assert_eq!((o.rules.gods, o.rules.mirror), (GodLimit::Open, true), "the mirror is a no-limit fill");
        let o = LaunchOptions::parse(Vec::<String>::new(), &env_of(&[("CLAWBOUND_GODS", "open")]));
        assert_eq!(o.rules.gods, GodLimit::Open);
        let o = LaunchOptions::parse(vec!["--rules".to_string(), "level=6 cd=0.5".to_string()], &env_of(&[]));
        assert_eq!((o.rules.start_level, o.rules.cd_mul, o.rules_error.is_none()), (6, 0.5, true));
        let o = LaunchOptions::parse(Vec::<String>::new(), &env_of(&[("CLAWBOUND_RULES", "gold=9000 wings=2")]));
        assert_eq!(o.rules.start_gold, 9000.0, "the readable part still applies");
        assert!(o.rules_error.as_deref().unwrap_or("").contains("wings"));
        assert!(LaunchOptions::parse(Vec::<String>::new(), &env_of(&[])).rules.is_default());
    }

    #[test]
    fn plain_launch_is_the_menu() {
        let o = LaunchOptions::parse(Vec::<String>::new(), &env_of(&[]));
        assert_eq!(o, LaunchOptions::default());
    }

    #[test]
    fn bsgames_quick_match_flag_skips_the_menu() {
        let o = LaunchOptions::parse(vec!["--quick-match".to_string()], &env_of(&[]));
        assert!(o.quick_match && !o.screenshot && o.handoff.is_none());
    }

    #[test]
    fn bsgames_handoff_env_is_read_and_implies_quick_match() {
        let env = env_of(&[("BSGAMES_HANDOFF_ROLE", "join"), ("BSGAMES_HANDOFF_ME", "kesslerd@3|Kessler"), ("BSGAMES_HANDOFF_PEER", "friend@9")]);
        let o = LaunchOptions::parse(Vec::<String>::new(), &env);
        assert_eq!(o.handoff, Some(HandoffRole::Join));
        assert_eq!(o.player_name.as_deref(), Some("Kessler"));
        assert_eq!(o.player_id.as_deref(), Some("kesslerd@3"));
        assert_eq!(o.peer_id.as_deref(), Some("friend@9"));
        assert!(o.quick_match);
    }

    #[test]
    fn ancestor_can_be_named_or_numbered() {
        let o = LaunchOptions::parse(vec!["--ancestor".to_string(), "snowhush".to_string()], &env_of(&[]));
        assert_eq!(o.ancestor_id().map(|i| crate::gods::god(i).name), Some("Snowhush"));
        let o = LaunchOptions::parse(vec!["--ancestor".to_string(), "3".to_string()], &env_of(&[]));
        assert_eq!(o.ancestor_id(), Some(3));
        let o = LaunchOptions::parse(vec!["--ancestor".to_string(), "nobody".to_string()], &env_of(&[]));
        assert_eq!(o.ancestor_id(), None);
    }
}
