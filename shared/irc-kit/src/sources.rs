//! The source-consent gate: may a channel's log be handed to a model?
//!
//! Modelled on `dialer-core`'s `command_guard.rs`, for the same reason that
//! module exists — the safe answer is the default, the unsafe one costs a
//! deliberate act, and a corrupt policy file fails closed.
//!
//! ## What this is actually protecting
//!
//! An IRC log is other people's writing. Even a log you captured yourself by
//! sitting in the channel contains hundreds of messages from people who did
//! not agree to be training input, summarisation fodder, or context for a
//! generated answer. Some communities have said plainly that they do not want
//! their words used this way. "It was publicly readable" is not the same
//! claim as "they agreed to this", and this module refuses to conflate them.
//!
//! Three rules, each enforced structurally rather than by documentation:
//!
//! 1. **Local participation logs only.** A [`Source`] is a filesystem path.
//!    [`Source::parse`] rejects anything carrying a URL scheme, so "point the
//!    importer at the web archive" is not expressible in the type, let alone
//!    a config file. Fetching someone's log site is a different program, and
//!    this is not it.
//! 2. **Per-line provenance.** Only [`log::Origin::Participated`] lines pass
//!    [`admit`]. A file that mixes hand-imported text with captured text
//!    yields only the captured part; the imported lines are dropped, not
//!    laundered.
//! 3. **Per-channel, typed confirmation.** A grant requires the operator to
//!    have typed the channel name back. [`CONSENT_PROMPT`] is a const here,
//!    not UI copy, so a surface cannot soften the wording into a shrug.
//!
//! None of this makes using a log *right* — it makes the decision explicit,
//! attributable, and narrow. Whether a given channel is okay with it is a
//! question for the channel, and the honest answer is often to ask.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::log::{self, Line, Origin};
use net_kit::json;

/// The exact wording a surface must show before recording a grant. A const so
/// every surface asks the same question; paraphrasing it in a UI is a bug.
pub const CONSENT_PROMPT: &str = "\
This channel's log is other people's writing. Adding it as a source sends \
their messages to a model. Some communities have asked that their discussions \
not be used this way, and public readability is not their agreement.\n\
Only lines this client captured while joined to the channel will be used; \
anything imported by hand is skipped.\n\
Type the channel name exactly to confirm, or anything else to cancel.";

/// A permitted source. Deliberately a path and nothing else — see rule 1.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Source {
    pub path: PathBuf,
}

impl Source {
    /// Parse a source string. Rejects URLs, and anything that is not a
    /// `.jsonl` file, before the caller ever gets a value it could read.
    pub fn parse(s: &str) -> Result<Source, String> {
        let t = s.trim();
        if t.is_empty() {
            return Err("empty source".into());
        }
        // Any scheme at all. Not a denylist of http/https: a source is a local
        // path, so *every* scheme is wrong, including ones invented later.
        if let Some(i) = t.find("://") {
            return Err(format!(
                "'{}' is a URL. Sources are local participation logs, never fetched archives — \
                 join the channel and log it yourself.",
                &t[..i + 3]
            ));
        }
        if t.starts_with("http:") || t.starts_with("https:") {
            return Err("sources are local participation logs, not URLs".into());
        }
        let p = PathBuf::from(t);
        if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            return Err("a source must be an irc-kit .jsonl log".into());
        }
        Ok(Source { path: p })
    }
}

/// One recorded permission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub network: String,
    /// Stored casemapped, so `#Asahi-Dev` and `#asahi-dev` are one grant.
    pub channel: String,
    /// Unix seconds the grant was recorded, so it can be shown and revoked
    /// knowingly rather than being an invisible sticky bit.
    pub ts: u64,
}

fn key(network: &str, channel: &str) -> String {
    format!("{}/{}", crate::proto::lower(network), crate::proto::lower(channel))
}

/// The whole policy. Empty means nothing is permitted, which is also what a
/// missing or damaged file produces.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    grants: BTreeMap<String, Grant>,
    /// True when a policy file existed but could not be read. Surfaced so the
    /// UI can say "policy unreadable, sources off" rather than the operator
    /// wondering why their grants vanished.
    pub load_error: Option<String>,
}

impl Policy {
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("irc-kit").join("sources.jsonl")
    }

    /// Load. A missing file is an empty policy (not an error); an unreadable
    /// or malformed one is an empty policy *with* `load_error` set. Either
    /// way nothing is permitted — the failure direction is closed.
    pub fn load(path: &Path) -> Policy {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Policy::default(),
            Err(e) => {
                return Policy {
                    grants: BTreeMap::new(),
                    load_error: Some(format!("{}: {e}", path.display())),
                }
            }
        };
        let mut p = Policy::default();
        for (i, l) in text.lines().enumerate() {
            if l.trim().is_empty() {
                continue;
            }
            match parse_grant(l) {
                Some(g) => {
                    p.grants.insert(key(&g.network, &g.channel), g);
                }
                None => {
                    // One bad line poisons the file. A policy that silently
                    // drops the line it could not read is a policy that might
                    // be missing a revocation.
                    return Policy {
                        grants: BTreeMap::new(),
                        load_error: Some(format!("{}: line {} is malformed", path.display(), i + 1)),
                    };
                }
            }
        }
        p
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
        let mut out = String::new();
        for g in self.grants.values() {
            out.push_str(&format!(
                "{{\"network\":\"{}\",\"channel\":\"{}\",\"ts\":{}}}\n",
                json::escape(&g.network),
                json::escape(&g.channel),
                g.ts
            ));
        }
        fs::write(path, out).map_err(|e| format!("write {}: {e}", path.display()))
    }

    /// Is this channel permitted as a source?
    pub fn allows(&self, network: &str, channel: &str) -> bool {
        self.load_error.is_none() && self.grants.contains_key(&key(network, channel))
    }

    pub fn grants(&self) -> impl Iterator<Item = &Grant> {
        self.grants.values()
    }

    /// Record a grant, but only if `typed` matches the channel name. This is
    /// the hardcoded ask: a caller cannot grant on the operator's behalf by
    /// passing a bool, because there is no bool to pass.
    ///
    /// Matching is casemapped (`#Asahi-Dev` confirms `#asahi-dev`) but not
    /// fuzzy — a typo cancels, which is the intended failure direction.
    pub fn grant(
        &mut self,
        network: &str,
        channel: &str,
        typed: &str,
        now: u64,
    ) -> Result<&Grant, String> {
        if crate::proto::lower(typed.trim()) != crate::proto::lower(channel.trim()) {
            return Err(format!(
                "not confirmed: expected the channel name '{channel}', got '{}'",
                typed.trim()
            ));
        }
        if let Some(e) = &self.load_error {
            return Err(format!("policy is unreadable ({e}); fix or remove it before granting"));
        }
        let k = key(network, channel);
        self.grants.insert(
            k.clone(),
            Grant {
                network: crate::proto::lower(network),
                channel: crate::proto::lower(channel),
                ts: now,
            },
        );
        Ok(self.grants.get(&k).expect("just inserted"))
    }

    /// Withdraw a grant. Returns whether there was one.
    pub fn revoke(&mut self, network: &str, channel: &str) -> bool {
        self.grants.remove(&key(network, channel)).is_some()
    }
}

fn parse_grant(line: &str) -> Option<Grant> {
    let t = line.trim();
    if !t.starts_with('{') || !t.ends_with('}') {
        return None;
    }
    let network = json::find_string_field(t, "network")?;
    let channel = json::find_string_field(t, "channel")?;
    let ts = t
        .find("\"ts\":")
        .map(|i| {
            let rest = t[i + 5..].trim_start();
            let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
            rest[..end].parse().unwrap_or(0)
        })
        .unwrap_or(0);
    Some(Grant { network, channel, ts })
}

/// The result of filtering a log through the policy.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Admitted {
    pub lines: Vec<Line>,
    /// Dropped because the channel has no grant.
    pub denied_channels: Vec<String>,
    /// Dropped because the line was not captured by this client.
    pub dropped_imported: usize,
}

/// Filter lines to those a model may see. Both rules apply per line, so a
/// mixed-provenance file cannot smuggle imported text through on the strength
/// of the channel's grant.
pub fn admit(policy: &Policy, lines: &[Line]) -> Admitted {
    let mut out = Admitted::default();
    for l in lines {
        if l.origin != Origin::Participated {
            out.dropped_imported += 1;
            continue;
        }
        if !policy.allows(&l.network, &l.channel) {
            let k = key(&l.network, &l.channel);
            if !out.denied_channels.contains(&k) {
                out.denied_channels.push(k);
            }
            continue;
        }
        out.lines.push(l.clone());
    }
    out
}

/// Load and admit a source file in one step. Returns the admitted lines and
/// the count of damaged records, or an error for a source that never should
/// have been named.
pub fn load_source(policy: &Policy, src: &Source) -> Result<(Admitted, usize), String> {
    let (lines, damaged) = log::read(&src.path)?;
    Ok((admit(policy, &lines), damaged))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(channel: &str, origin: Origin, text: &str) -> Line {
        Line {
            ts: 1,
            server_time: None,
            network: "oftc".into(),
            channel: channel.into(),
            nick: "someone".into(),
            text: text.into(),
            notice: false,
            action: false,
            origin,
        }
    }

    #[test]
    fn a_fresh_policy_permits_nothing() {
        let p = Policy::default();
        assert!(!p.allows("oftc", "#asahi-dev"));
        let a = admit(&p, &[line("#asahi-dev", Origin::Participated, "hi")]);
        assert!(a.lines.is_empty());
        assert_eq!(a.denied_channels, vec!["oftc/#asahi-dev"]);
    }

    #[test]
    fn granting_requires_typing_the_channel_name() {
        let mut p = Policy::default();
        assert!(p.grant("oftc", "#asahi-dev", "yes", 100).is_err());
        assert!(p.grant("oftc", "#asahi-dev", "", 100).is_err());
        assert!(p.grant("oftc", "#asahi-dev", "#asahi", 100).is_err());
        assert!(!p.allows("oftc", "#asahi-dev"));

        assert!(p.grant("oftc", "#asahi-dev", "#asahi-dev", 100).is_ok());
        assert!(p.allows("oftc", "#asahi-dev"));
    }

    #[test]
    fn confirmation_is_casemapped_but_not_fuzzy() {
        let mut p = Policy::default();
        assert!(p.grant("oftc", "#asahi-dev", "  #Asahi-Dev  ", 1).is_ok());
        let mut q = Policy::default();
        assert!(q.grant("oftc", "#asahi-dev", "#asahidev", 1).is_err(), "a typo must cancel");
    }

    #[test]
    fn a_grant_covers_only_the_channel_it_names() {
        let mut p = Policy::default();
        p.grant("oftc", "#asahi-dev", "#asahi-dev", 1).unwrap();
        assert!(p.allows("oftc", "#asahi-dev"));
        assert!(!p.allows("oftc", "#asahi"), "sibling channel is not covered");
        assert!(!p.allows("libera", "#asahi-dev"), "same channel on another network is not covered");
    }

    #[test]
    fn imported_lines_never_pass_even_in_a_granted_channel() {
        let mut p = Policy::default();
        p.grant("oftc", "#asahi-dev", "#asahi-dev", 1).unwrap();
        let a = admit(
            &p,
            &[
                line("#asahi-dev", Origin::Participated, "mine"),
                line("#asahi-dev", Origin::Imported, "scraped from somewhere"),
            ],
        );
        assert_eq!(a.lines.len(), 1);
        assert_eq!(a.lines[0].text, "mine");
        assert_eq!(a.dropped_imported, 1);
    }

    #[test]
    fn urls_are_not_expressible_as_sources() {
        for bad in [
            "https://oftc.catirclogs.org/asahi-dev/2026-08-22",
            "http://example/log.jsonl",
            "gopher://old/log.jsonl",
            "ftp://host/x.jsonl",
        ] {
            let e = Source::parse(bad).unwrap_err();
            assert!(
                e.contains("local") || e.contains("URL"),
                "{bad} gave a confusing error: {e}"
            );
        }
    }

    #[test]
    fn only_jsonl_logs_are_sources() {
        assert!(Source::parse("/tmp/oftc__asahi-dev.jsonl").is_ok());
        assert!(Source::parse("/tmp/scrape.html").is_err());
        assert!(Source::parse("/tmp/notes.md").is_err());
        assert!(Source::parse("").is_err());
    }

    #[test]
    fn revoking_takes_effect() {
        let mut p = Policy::default();
        p.grant("oftc", "#asahi-dev", "#asahi-dev", 1).unwrap();
        assert!(p.revoke("oftc", "#asahi-dev"));
        assert!(!p.allows("oftc", "#asahi-dev"));
        assert!(!p.revoke("oftc", "#asahi-dev"), "second revoke reports nothing to do");
    }

    #[test]
    fn policy_round_trips_through_a_file() {
        let d = std::env::temp_dir().join(format!("irc-kit-pol-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let f = d.join("sources.jsonl");
        let mut p = Policy::default();
        p.grant("oftc", "#asahi-dev", "#asahi-dev", 4242).unwrap();
        p.save(&f).unwrap();

        let q = Policy::load(&f);
        assert!(q.load_error.is_none());
        assert!(q.allows("oftc", "#asahi-dev"));
        assert_eq!(q.grants().next().unwrap().ts, 4242);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_policy_file_is_empty_not_an_error() {
        let p = Policy::load(Path::new("/nonexistent/irc-kit/sources.jsonl"));
        assert!(p.load_error.is_none());
        assert!(!p.allows("oftc", "#asahi-dev"));
    }

    #[test]
    fn a_corrupt_policy_fails_closed_and_says_so() {
        let d = std::env::temp_dir().join(format!("irc-kit-pol-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let f = d.join("sources.jsonl");
        fs::write(
            &f,
            "{\"network\":\"oftc\",\"channel\":\"#asahi-dev\",\"ts\":1}\nnot json at all\n",
        )
        .unwrap();

        let p = Policy::load(&f);
        assert!(p.load_error.is_some(), "a damaged policy must be visible");
        assert!(!p.allows("oftc", "#asahi-dev"), "and must permit nothing");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn you_cannot_grant_on_top_of_a_broken_policy() {
        let mut p = Policy { grants: BTreeMap::new(), load_error: Some("boom".into()) };
        assert!(p.grant("oftc", "#asahi-dev", "#asahi-dev", 1).is_err());
    }

    #[test]
    fn the_prompt_names_the_actual_concern() {
        // If someone rewrites the prompt into "Allow AI features? [y/N]" this
        // test should fail, because that is not the same question.
        assert!(CONSENT_PROMPT.contains("other people's writing"));
        assert!(CONSENT_PROMPT.contains("not their agreement"));
    }
}
