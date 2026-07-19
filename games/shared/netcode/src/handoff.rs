//! handoff — the `BSGAMES_HANDOFF_*` env protocol every fleet game reads at startup so the
//! bsgames centre's "▶ join a friend" can skip that game's own menus and dial directly.
//!
//! Adopted VERBATIM (parser + tests) from the first consumer, `games/soldier/src/handoff.rs`,
//! now that every game reads the same three variables — one canonical parser instead of one copy
//! per game. The producer is `bsgames::centre::handoff_env`; the contract:
//!   * `BSGAMES_HANDOFF_ROLE` — `host` | `join`
//!   * `BSGAMES_HANDOFF_ME`   — `<id>|<display name>` (the identity bsgames already published,
//!     so the launched game presents as the SAME player, not a second minted identity)
//!   * `BSGAMES_HANDOFF_PEER` — the friend's profile id (join only)
//!
//! Entirely additive for every game: with no handoff env set (launched directly, standalone —
//! the common case), [`from_env`] returns `None` and normal menu-driven startup is untouched.
//! `None` on anything missing/malformed — a bad or partial handoff falls back to the menu,
//! never crashes startup.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Handoff {
    Host { me_id: String, me_name: String },
    Join { me_id: String, me_name: String, peer_id: String },
}

/// Parse the handoff env vars, if present.
pub fn from_env() -> Option<Handoff> {
    parse(
        std::env::var("BSGAMES_HANDOFF_ROLE").ok(),
        std::env::var("BSGAMES_HANDOFF_ME").ok(),
        std::env::var("BSGAMES_HANDOFF_PEER").ok(),
    )
}

fn parse(role: Option<String>, me: Option<String>, peer: Option<String>) -> Option<Handoff> {
    let (me_id, me_name) = me?.split_once('|').map(|(a, b)| (a.to_string(), b.to_string()))?;
    match role?.as_str() {
        "host" => Some(Handoff::Host { me_id, me_name }),
        "join" => Some(Handoff::Join { me_id, me_name, peer_id: peer? }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_join_with_all_three_vars() {
        let h = parse(Some("join".into()), Some("me@0|Kessler".into()), Some("bob@1".into()));
        assert_eq!(h, Some(Handoff::Join { me_id: "me@0".into(), me_name: "Kessler".into(), peer_id: "bob@1".into() }));
    }

    #[test]
    fn parses_host_without_a_peer() {
        let h = parse(Some("host".into()), Some("me@0|Kessler".into()), None);
        assert_eq!(h, Some(Handoff::Host { me_id: "me@0".into(), me_name: "Kessler".into() }));
    }

    #[test]
    fn join_without_a_peer_id_is_rejected_not_defaulted() {
        assert_eq!(parse(Some("join".into()), Some("me@0|Kessler".into()), None), None);
    }

    #[test]
    fn missing_or_malformed_me_field_is_rejected() {
        assert_eq!(parse(Some("join".into()), None, Some("bob@1".into())), None);
        assert_eq!(parse(Some("join".into()), Some("no-pipe-here".into()), Some("bob@1".into())), None);
    }

    #[test]
    fn unknown_role_and_nothing_set_both_yield_none() {
        assert_eq!(parse(Some("spectate".into()), Some("me@0|Kessler".into()), None), None);
        assert_eq!(parse(None, None, None), None);
    }
}
