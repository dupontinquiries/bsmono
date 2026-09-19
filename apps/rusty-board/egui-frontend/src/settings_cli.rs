//! settings_cli — the settings model, driven by name instead of by thumb.
//!
//! One command grammar behind two front ends: `rusty-board-settings get/set`
//! on a terminal, and the full-page configurator. Both go through here, so a
//! key that is settable in one is settable in the other and neither can drift
//! into having its own idea of what the options are.
//!
//! ## Why this is not just `toggle`
//!
//! `settings_model::toggle` cycles a setting to its next value, which is the
//! right verb for a key you press: one control, no list to render, no way to
//! pick wrong. It is the wrong verb for a name-driven interface, where
//! "dvorak" is a thing you can say directly and cycling until you arrive is
//! absurd. [`Cmd::Set`] names the value; `toggle` stays as it is.
//!
//! ## Refuse, and say what was allowed
//!
//! A CLI that rejects a value without listing the alternatives makes the user
//! go and read source. Every refusal here carries the option list, because the
//! moment someone is wrong about a value is exactly when they need it.
//!
//! ## No I/O
//!
//! Parsing and validation only; applying a [`Change`] is the caller's, which
//! is what lets the configurator preview a change and the CLI write one.

use crate::settings_model::{group, Change, Group, Setting, SETTINGS};

/// One parsed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    /// Every key, or one group's.
    List(Option<Group>),
    /// One key's current value and its options.
    Get(&'static str),
    /// Set a key to a named value.
    Set(&'static str, String),
    /// Put a key back to its first declared option.
    Reset(&'static str),
}

/// Why a command could not be understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CmdError {
    /// No such key. Carries near-misses, because a typo is the likely cause.
    UnknownKey { given: String, did_you_mean: Vec<&'static str> },
    /// The key exists; the value is not one of its options.
    BadValue { key: &'static str, given: String, allowed: Vec<&'static str> },
    /// `set` with nothing to set it to.
    MissingValue { key: &'static str, allowed: Vec<&'static str> },
    /// No such group.
    UnknownGroup { given: String, allowed: Vec<&'static str> },
    /// Not a verb this understands.
    UnknownCommand(String),
}

impl CmdError {
    /// A line a person can act on. Every variant names the way forward, not
    /// just the problem.
    pub fn explain(&self) -> String {
        match self {
            CmdError::UnknownKey { given, did_you_mean } if !did_you_mean.is_empty() => {
                format!("no setting '{given}' — did you mean {}?", did_you_mean.join(", "))
            }
            CmdError::UnknownKey { given, .. } => {
                format!("no setting '{given}' — try `list` for all of them")
            }
            CmdError::BadValue { key, given, allowed } => {
                format!("'{given}' is not a value for {key} — allowed: {}", allowed.join(", "))
            }
            CmdError::MissingValue { key, allowed } => {
                format!("set {key} to what? allowed: {}", allowed.join(", "))
            }
            CmdError::UnknownGroup { given, allowed } => {
                format!("no group '{given}' — allowed: {}", allowed.join(", "))
            }
            CmdError::UnknownCommand(c) => {
                format!("unknown command '{c}' — try list, get, set, reset")
            }
        }
    }
}

fn find(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.key == key)
}

/// Keys sharing a prefix or a suffix with `given`.
///
/// Deliberately crude: keys here are `group.name`, so the overwhelmingly
/// common typo is the right leaf under the wrong group or a misspelled group.
/// Matching on either half catches both, and a Levenshtein pass would be more
/// code for the same answers on a list of seven.
fn near(given: &str) -> Vec<&'static str> {
    let g = given.to_lowercase();
    let leaf = g.rsplit('.').next().unwrap_or(&g);
    SETTINGS
        .iter()
        .filter(|s| {
            let k = s.key;
            k.rsplit('.').next() == Some(leaf)
                || g.split('.').next().is_some_and(|p| !p.is_empty() && k.starts_with(p))
        })
        .map(|s| s.key)
        .collect()
}

fn group_by_name(name: &str) -> Option<Group> {
    Group::ALL.iter().copied().find(|g| g.title().eq_ignore_ascii_case(name))
}

/// Parse one command line, already split into words.
pub fn parse(args: &[&str]) -> Result<Cmd, CmdError> {
    let Some((verb, rest)) = args.split_first() else {
        return Ok(Cmd::List(None));
    };
    match verb.to_lowercase().as_str() {
        "list" | "ls" => match rest.first() {
            None => Ok(Cmd::List(None)),
            Some(g) => group_by_name(g).map(|g| Cmd::List(Some(g))).ok_or_else(|| {
                CmdError::UnknownGroup {
                    given: (*g).to_string(),
                    allowed: Group::ALL.iter().map(|g| g.title()).collect(),
                }
            }),
        },
        "get" | "set" | "reset" => {
            let Some(key) = rest.first() else {
                return Err(CmdError::UnknownKey { given: String::new(), did_you_mean: vec![] });
            };
            let s = find(key).ok_or_else(|| CmdError::UnknownKey {
                given: (*key).to_string(),
                did_you_mean: near(key),
            })?;
            match verb.to_lowercase().as_str() {
                "get" => Ok(Cmd::Get(s.key)),
                "reset" => Ok(Cmd::Reset(s.key)),
                _ => {
                    let Some(v) = rest.get(1) else {
                        return Err(CmdError::MissingValue {
                            key: s.key,
                            allowed: s.values.to_vec(),
                        });
                    };
                    // Accept the DISPLAY label as well as the stored value:
                    // `set layout.letters Dvorak` is what someone reading the
                    // GUI will type, and refusing it to insist on "dvorak"
                    // would be pedantry about an internal spelling.
                    let matched = s
                        .values
                        .iter()
                        .position(|x| x.eq_ignore_ascii_case(v))
                        .or_else(|| s.options.iter().position(|x| x.eq_ignore_ascii_case(v)));
                    match matched {
                        Some(i) => Ok(Cmd::Set(s.key, s.values[i].to_string())),
                        None => Err(CmdError::BadValue {
                            key: s.key,
                            given: (*v).to_string(),
                            allowed: s.values.to_vec(),
                        }),
                    }
                }
            }
        }
        other => Err(CmdError::UnknownCommand(other.to_string())),
    }
}

/// The change a `Set` or `Reset` asks for. `List` and `Get` produce none.
///
/// Reset goes to the FIRST declared option, which is the model's own default:
/// `Setting::next` cycles from there, so first-declared is where a fresh
/// install sits.
pub fn change_for(cmd: &Cmd) -> Option<Change> {
    match cmd {
        Cmd::Set(key, value) => Some(Change {
            // `*key` — matching on `&Cmd` gives `&&'static str`, and the
            // reborrow is what makes it the 'static the field wants.
            key: *key,
            value: value.clone(),
            broadcast: true,
            persist: true,
        }),
        Cmd::Reset(key) => find(key).map(|s| Change {
            key: s.key,
            value: s.values[0].to_string(),
            broadcast: true,
            persist: true,
        }),
        _ => None,
    }
}

/// Render a `List`, one line per setting.
pub fn render_list(g: Option<Group>, current: impl Fn(&str) -> String) -> Vec<String> {
    let rows: Vec<&'static Setting> = match g {
        Some(g) => group(g),
        None => SETTINGS.iter().collect(),
    };
    rows.iter()
        .map(|s| {
            let now = current(s.key);
            format!("{:<28} {:<12} [{}]", s.key, now, s.values.join(", "))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_can_be_named_rather_than_cycled_to() {
        let c = parse(&["set", "layout.letters", "dvorak"]).expect("a set");
        assert_eq!(c, Cmd::Set("layout.letters", "dvorak".into()));
        let ch = change_for(&c).expect("a change");
        assert_eq!(ch.value, "dvorak");
        assert!(ch.persist && ch.broadcast);
    }

    /// The DISPLAY label works too: it is what someone reading the GUI will
    /// type, and insisting on the internal spelling would be pedantry.
    #[test]
    fn the_label_is_accepted_as_well_as_the_stored_value() {
        assert_eq!(
            parse(&["set", "layout.letters", "Dvorak"]).unwrap(),
            Cmd::Set("layout.letters", "dvorak".into()),
            "the label should resolve to the stored value"
        );
    }

    /// A refusal that does not list the alternatives sends the user to read
    /// source. Every one of them carries the way forward.
    #[test]
    fn every_refusal_names_what_was_allowed() {
        let e = parse(&["set", "layout.letters", "colemak"]).unwrap_err();
        let msg = e.explain();
        assert!(msg.contains("qwerty") && msg.contains("dvorak"), "{msg}");

        let e = parse(&["set", "layout.letters"]).unwrap_err();
        assert!(e.explain().contains("qwerty"), "{}", e.explain());

        let e = parse(&["list", "Nonsense"]).unwrap_err();
        assert!(e.explain().contains("Layout"), "{}", e.explain());

        let e = parse(&["frobnicate"]).unwrap_err();
        assert!(e.explain().contains("list"), "{}", e.explain());
    }

    /// The likely cause of an unknown key is a typo, so near-misses come back
    /// with it.
    #[test]
    fn an_unknown_key_suggests_the_near_ones() {
        let e = parse(&["get", "colors.letters"]).unwrap_err();
        match &e {
            CmdError::UnknownKey { did_you_mean, .. } => {
                assert!(did_you_mean.contains(&"layout.letters"), "{did_you_mean:?}");
            }
            other => panic!("{other:?}"),
        }
        assert!(e.explain().contains("layout.letters"));
    }

    /// Reset goes to the first declared option, which is where a fresh install
    /// sits because `next` cycles from there.
    #[test]
    fn reset_returns_a_setting_to_its_first_option() {
        let ch = change_for(&parse(&["reset", "layout.letters"]).unwrap()).unwrap();
        assert_eq!(ch.value, "qwerty");
    }

    /// Reading commands produce no change -- a `get` that wrote the config
    /// would be a surprising thing for a read to do.
    #[test]
    fn reading_commands_change_nothing() {
        assert!(change_for(&parse(&["get", "layout.letters"]).unwrap()).is_none());
        assert!(change_for(&parse(&["list"]).unwrap()).is_none());
    }

    /// No arguments lists everything, because a bare invocation should show
    /// you what there is rather than an error.
    #[test]
    fn a_bare_invocation_lists_everything() {
        assert_eq!(parse(&[]).unwrap(), Cmd::List(None));
        let lines = render_list(None, |_| "qwerty".into());
        assert_eq!(lines.len(), SETTINGS.len());
        assert!(lines.iter().any(|l| l.contains("layout.letters")));
    }

    /// A group narrows it, and the group name is matched case-insensitively
    /// because "layout" is what a person types.
    #[test]
    fn a_group_can_be_listed_by_name_in_any_case() {
        assert_eq!(parse(&["list", "layout"]).unwrap(), Cmd::List(Some(Group::Layout)));
        assert_eq!(parse(&["list", "LAYOUT"]).unwrap(), Cmd::List(Some(Group::Layout)));
        let lines = render_list(Some(Group::Layout), |_| "x".into());
        assert!(lines.iter().all(|l| l.contains("layout.")));
    }

    /// Every key the GUI shows is reachable by name -- that is the point of
    /// one model behind both front ends.
    #[test]
    fn every_setting_the_gui_has_is_settable_by_name() {
        for s in SETTINGS {
            let cmd = parse(&["set", s.key, s.values[0]]);
            assert!(cmd.is_ok(), "{} is not settable: {:?}", s.key, cmd.err());
        }
    }
}
