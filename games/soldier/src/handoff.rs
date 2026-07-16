//! handoff — reads the `BSGAMES_HANDOFF_*` env vars bsgames' "join" button sets (see
//! `bsgames::centre::handoff_env`) so Soldier can skip its own main menu and dial straight into a
//! friend's match instead of the player re-navigating Multiplayer → Join by hand. Entirely
//! additive: with no handoff env set (launched directly, standalone — the common case, and the
//! only case anyone testing/playing Soldier on its own ever sees), `from_env()` returns `None` and
//! nothing about normal menu-driven startup changes.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Handoff {
    Host { me_id: String, me_name: String },
    Join { me_id: String, me_name: String, peer_id: String },
}

/// Parse the handoff env vars, if present. `None` on anything missing/malformed — a bad or
/// partial handoff should fall back to Soldier's normal menu, never crash startup.
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
