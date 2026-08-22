//! `:irc` — IRC channel logs as fan-mode sources.
//!
//! The pure half of the `:irc` command family: target parsing, the two-step
//! consent handshake, and the text the operator sees. No filesystem, no
//! `irc_kit::sources::Policy` mutation — the host applies the decision this
//! module returns. Same split as the rest of the crate: everything with a
//! decision in it is a pure function, testable without a UI.
//!
//! ## Why two steps, and why the parser cannot do it in one
//!
//! `:workspace <path>` opens a workspace in one command because the only
//! thing at stake is your own files. A channel log is other people's writing,
//! and several communities — including the one whose logs prompted this —
//! have said plainly that they do not want their discussions used as model
//! input. So adding one is deliberately two commands: `:irc <net>/<chan>`
//! prints the ask, and `:ircy <channel-name>` confirms by making you type the
//! channel name back.
//!
//! The confirmation phrase is the channel name rather than `y` so that muscle
//! memory cannot carry you through it. A `y` prompt is answered before it is
//! read; typing `#asahi-dev` is not.
//!
//! Nothing here can reach the network. [`irc_kit::sources::Source::parse`]
//! rejects every URL scheme, and only lines this machine captured while
//! joined to a channel are ever admitted — see that module.

use irc_kit::sources::{Policy, CONSENT_PROMPT};

/// A `network/channel` pair as typed on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub network: String,
    pub channel: String,
}

/// Parse `oftc/#asahi-dev`.
///
/// The network is mandatory. A bare `#asahi-dev` is rejected rather than
/// guessed at, because the same channel name on two networks is two different
/// rooms of two different sets of people, and a grant that silently covered
/// the wrong one would be exactly the mistake this module exists to prevent.
pub fn parse_target(arg: &str) -> Result<Target, String> {
    let t = arg.trim();
    if t.is_empty() {
        return Err("expected <network>/<channel>, e.g. oftc/#asahi-dev".into());
    }
    let Some(slash) = t.find('/') else {
        return Err(format!(
            "'{t}' has no network. Write <network>/<channel>, e.g. oftc/{}",
            if t.starts_with('#') { t.to_string() } else { format!("#{t}") }
        ));
    };
    let (network, channel) = (t[..slash].trim(), t[slash + 1..].trim());
    if network.is_empty() {
        return Err("empty network name".into());
    }
    if channel.is_empty() {
        return Err("empty channel name".into());
    }
    if !irc_kit::proto::is_channel(channel) {
        return Err(format!(
            "'{channel}' is not a channel (expected a leading # & + or !). \
             Private messages are never logged as sources."
        ));
    }
    Ok(Target { network: network.to_string(), channel: channel.to_string() })
}

/// The ask raised by `:irc <net>/<chan>`, waiting for `:ircy`.
///
/// Held by the host between the two commands. Exactly one can be outstanding:
/// a second `:irc` replaces it, so a stale ask can never be confirmed by a
/// `:ircy` the operator meant for a different channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub target: Target,
    /// How many log lines the grant would expose, so the number is on screen
    /// before the decision rather than after.
    pub line_count: usize,
    /// Distinct nicks in that log — the count of people whose writing this is.
    pub nick_count: usize,
}

impl Pending {
    /// The exact text to print. Wraps [`CONSENT_PROMPT`] with the specifics;
    /// the prompt itself is a const in irc-kit so no surface can soften it.
    pub fn ask_text(&self) -> String {
        format!(
            "{}/{} — {} lines from {} people.\n\n{}\n\n  :ircy {}    to confirm\n  :irc          to cancel (or just run something else)",
            self.target.network,
            self.target.channel,
            self.line_count,
            self.nick_count,
            CONSENT_PROMPT,
            self.target.channel,
        )
    }
}

/// What the host should do after `:ircy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirmation {
    /// Record the grant and clear the pending ask.
    Granted(Target),
    /// The typed phrase did not match. The ask is cleared — a mistyped
    /// confirmation cancels rather than leaving a loaded gun for the next
    /// `:ircy`.
    Mismatch { expected: String, got: String },
    /// `:ircy` with nothing pending.
    NothingPending,
}

/// Resolve a `:ircy <typed>` against the outstanding ask.
pub fn confirm(pending: Option<&Pending>, typed: &str) -> Confirmation {
    let Some(p) = pending else { return Confirmation::NothingPending };
    if irc_kit::proto::lower(typed.trim()) == irc_kit::proto::lower(&p.target.channel) {
        Confirmation::Granted(p.target.clone())
    } else {
        Confirmation::Mismatch {
            expected: p.target.channel.clone(),
            got: typed.trim().to_string(),
        }
    }
}

/// One row of `:irc` status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRow {
    pub network: String,
    pub channel: String,
    pub granted: bool,
    pub lines: usize,
}

/// Render `:irc` status. `rows` is what the host found on disk; `policy`
/// decides which are live.
///
/// A policy that failed to load is reported first and loudly: the operator
/// needs to know their grants are not in effect, rather than wondering why
/// `:ircf` returns nothing.
pub fn status_text(policy: &Policy, rows: &[StatusRow]) -> String {
    let mut out = String::new();

    if let Some(e) = &policy.load_error {
        out.push_str(&format!(
            "!! source policy unreadable ({e})\n\
             !! no channel is permitted as a source until this is fixed or removed.\n\n"
        ));
    }

    if rows.is_empty() {
        out.push_str(
            "no channel logs found.\n\n\
             Logs are written by joining a channel with the irc backend.\n\
             There is no importer and no fetcher: a log site cannot be a source.\n",
        );
        return out;
    }

    let granted: Vec<&StatusRow> = rows.iter().filter(|r| r.granted).collect();
    let available: Vec<&StatusRow> = rows.iter().filter(|r| !r.granted).collect();

    out.push_str(&format!("sources ({} of {} logs granted)\n", granted.len(), rows.len()));
    for r in &granted {
        out.push_str(&format!("  ✓ {}/{}  {} lines\n", r.network, r.channel, r.lines));
    }
    if !available.is_empty() {
        out.push_str("\nlogged but NOT a source:\n");
        for r in &available {
            out.push_str(&format!("  · {}/{}  {} lines\n", r.network, r.channel, r.lines));
        }
        out.push_str("\n  :irc <network>/<channel>   to ask about adding one\n");
    }
    if !granted.is_empty() {
        out.push_str("  :ircx <network>/<channel>  to withdraw\n  :ircf <query>              to search granted logs\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Target {
        Target { network: "oftc".into(), channel: "#asahi-dev".into() }
    }

    fn pending() -> Pending {
        Pending { target: target(), line_count: 41_233, nick_count: 87 }
    }

    #[test]
    fn parses_a_network_qualified_channel() {
        assert_eq!(parse_target("oftc/#asahi-dev").unwrap(), target());
        assert_eq!(parse_target("  oftc / #asahi-dev  ").unwrap(), target());
    }

    #[test]
    fn a_bare_channel_is_rejected_with_the_fix_in_the_message() {
        let e = parse_target("#asahi-dev").unwrap_err();
        assert!(e.contains("no network"), "{e}");
        assert!(e.contains("oftc/#asahi-dev"), "the error should show the right spelling: {e}");
    }

    #[test]
    fn a_bare_name_without_a_hash_still_gets_a_usable_suggestion() {
        let e = parse_target("asahi-dev").unwrap_err();
        assert!(e.contains("oftc/#asahi-dev"), "{e}");
    }

    #[test]
    fn non_channels_are_rejected_so_dms_cannot_become_sources() {
        let e = parse_target("oftc/somenick").unwrap_err();
        assert!(e.contains("not a channel"), "{e}");
        assert!(parse_target("oftc/&local").is_ok(), "& is a channel prefix");
    }

    #[test]
    fn empty_halves_are_rejected() {
        assert!(parse_target("").is_err());
        assert!(parse_target("/#chan").is_err());
        assert!(parse_target("oftc/").is_err());
    }

    #[test]
    fn the_ask_states_the_scale_before_the_decision() {
        let text = pending().ask_text();
        assert!(text.contains("41233 lines from 87 people"), "{text}");
        assert!(text.contains("other people's writing"), "the const prompt must be present");
        assert!(text.contains(":ircy #asahi-dev"), "the exact confirming command is shown");
    }

    #[test]
    fn confirmation_requires_the_channel_name_not_a_yes() {
        let p = pending();
        assert_eq!(confirm(Some(&p), "#asahi-dev"), Confirmation::Granted(target()));
        assert!(matches!(confirm(Some(&p), "y"), Confirmation::Mismatch { .. }));
        assert!(matches!(confirm(Some(&p), "yes"), Confirmation::Mismatch { .. }));
        assert!(matches!(confirm(Some(&p), ""), Confirmation::Mismatch { .. }));
    }

    #[test]
    fn confirmation_is_casemapped_like_the_protocol() {
        let p = pending();
        assert_eq!(confirm(Some(&p), "  #Asahi-Dev "), Confirmation::Granted(target()));
    }

    #[test]
    fn a_near_miss_does_not_confirm() {
        let p = pending();
        assert!(matches!(confirm(Some(&p), "#asahi"), Confirmation::Mismatch { .. }));
        assert!(matches!(confirm(Some(&p), "#asahidev"), Confirmation::Mismatch { .. }));
    }

    #[test]
    fn confirming_with_nothing_pending_says_so() {
        assert_eq!(confirm(None, "#asahi-dev"), Confirmation::NothingPending);
    }

    #[test]
    fn status_with_no_logs_explains_there_is_no_importer() {
        let s = status_text(&Policy::default(), &[]);
        assert!(s.contains("no channel logs found"));
        assert!(s.contains("no importer"), "{s}");
    }

    #[test]
    fn status_separates_granted_from_merely_logged() {
        let rows = vec![
            StatusRow { network: "oftc".into(), channel: "#asahi-dev".into(), granted: true, lines: 10 },
            StatusRow { network: "oftc".into(), channel: "#asahi".into(), granted: false, lines: 5 },
        ];
        let s = status_text(&Policy::default(), &rows);
        assert!(s.contains("1 of 2 logs granted"));
        assert!(s.contains("✓ oftc/#asahi-dev"));
        assert!(s.contains("logged but NOT a source"));
        assert!(s.contains("· oftc/#asahi"));
    }

    #[test]
    fn a_broken_policy_is_reported_before_anything_else() {
        // Built field-wise rather than with struct-update syntax: `grants` is
        // private to irc-kit, which is correct — a downstream crate must not be
        // able to conjure grants, only observe them.
        let mut p = Policy::default();
        p.load_error = Some("sources.jsonl: line 3 is malformed".into());
        let rows = vec![StatusRow {
            network: "oftc".into(), channel: "#asahi-dev".into(), granted: false, lines: 10,
        }];
        let s = status_text(&p, &rows);
        assert!(s.starts_with("!! source policy unreadable"), "{s}");
        assert!(s.contains("no channel is permitted"));
    }
}
