//! SCREENPLAY — dialogue out of a marked-up script.
//!
//! The last manual step in the pipeline. Ids are derived
//! ([`ids`](crate::ids)), sessions import ([`session`](crate::session)), and
//! nothing turned a written scene into the [`Bank`](crate::Bank) that produces
//! the script the actor reads. This does.
//!
//! ## The format, and the one line that looks like a cue and is not
//!
//! ```text
//! **AIDEN**
//! *(quiet)*
//! Did he do all of this alone?
//! ```
//!
//! A cue is a bold line **on its own**, in capitals, with nothing after the
//! closing marker. That last clause is the whole trick: the same documents
//! open with
//!
//! ```text
//! **Setting**: Weeks after the shield crossing.
//! **Characters**: Halyard, Aiden, the Morpher, Meridian
//! ```
//!
//! which is bold at the start of a line and is metadata. A parser matching
//! "starts with `**`" imports the entire front matter as dialogue and gives
//! `Setting` and `Premise` speaking parts — and it does so silently, because
//! the result is a plausible-looking bank.
//! `front_matter_does_not_get_a_speaking_part` is that case.
//!
//! An italic line in parentheses is DIRECTION, not speech: it belongs on the
//! script as guidance and must never be recorded as a line. Italic lines
//! *without* parentheses are action, and are dropped entirely.
//!
//! ## Line breaks are typography, not structure
//!
//! Screenplay prose is hard-wrapped at column 78. A speech is therefore
//! several source lines and one spoken line, so consecutive text is joined
//! rather than treated as separate units. Splitting on the newline would give
//! an actor two half-sentences to read with no clue they belong together, and
//! the join is invisible in the output — which is why
//! `a_wrapped_speech_is_one_line_not_three` exists.

use crate::ids::derive_unique;
use crate::{Bank, Kind, Unit};

/// One speech lifted from a script.
#[derive(Clone, PartialEq, Debug)]
pub struct Line {
    pub speaker: String,
    /// The spoken text, unwrapped onto one line.
    pub text: String,
    /// A parenthetical, without its brackets. Empty when there was none.
    pub direction: String,
}

fn cue(line: &str) -> Option<String> {
    let t = line.trim();
    let inner = t.strip_prefix("**")?.strip_suffix("**")?;
    // Metadata is `**Setting**: ...` — bold, then a colon. A cue has nothing
    // after the closing marker at all, which `strip_suffix` already enforced.
    if inner.is_empty() || inner.contains("**") {
        return None;
    }
    // Cues are in capitals. `Setting` and `Premise` are not, which is a second
    // independent reason they cannot be mistaken for one.
    let has_letter = inner.chars().any(|c| c.is_alphabetic());
    if !has_letter || inner.chars().any(|c| c.is_lowercase()) {
        return None;
    }
    Some(inner.trim().to_string())
}

/// Split a trailing `(V.O.)`, `(O.S.)` or `(CONT'D)` off a cue.
///
/// These are the SAME ACTOR with a different microphone or a continuation, and
/// leaving them in the speaker name splits the roster — an actor then gets two
/// scripts, each missing half their lines. Running this against the real
/// screenplays returned seven speakers for five actors, which is how it was
/// found; the literal fixture in the tests below would never have shown it.
///
/// The suffix is KEPT as direction rather than discarded, because a voice-over
/// genuinely is performed differently.
fn split_cue(cue: &str) -> (String, String) {
    let t = cue.trim();
    if t.ends_with(')') {
        if let Some(open) = t.rfind('(') {
            let suffix = t[open + 1..t.len() - 1].trim().to_string();
            let name = t[..open].trim().to_string();
            if !name.is_empty() {
                return (name, suffix);
            }
        }
    }
    (t.to_string(), String::new())
}

fn parenthetical(line: &str) -> Option<String> {
    let t = line.trim();
    let inner = t.strip_prefix('*')?.strip_suffix('*')?.trim();
    let inner = inner.strip_prefix('(')?.strip_suffix(')')?;
    Some(inner.trim().to_string())
}

fn is_italic_action(line: &str) -> bool {
    let t = line.trim();
    t.len() > 2 && t.starts_with('*') && t.ends_with('*') && !t.starts_with("**")
}

/// Pull every speech out of a screenplay.
pub fn lines(markdown: &str) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let mut cur: Option<Line> = None;

    for raw in markdown.lines() {
        let t = raw.trim();

        if let Some(who) = cue(raw) {
            if let Some(l) = cur.take() {
                if !l.text.is_empty() {
                    out.push(l);
                }
            }
            let (speaker, suffix) = split_cue(&who);
            cur = Some(Line { speaker, text: String::new(), direction: suffix });
            continue;
        }

        let Some(l) = cur.as_mut() else { continue };

        if t.is_empty() || t.starts_with("---") || t.starts_with('#') {
            if !l.text.is_empty() {
                out.push(cur.take().unwrap());
            } else {
                // A cue whose speech has not started yet survives a blank
                // line; a cue with nothing after it at all is dropped when the
                // next cue arrives.
                if t.starts_with("---") || t.starts_with('#') {
                    cur = None;
                }
            }
            continue;
        }

        if let Some(d) = parenthetical(raw) {
            if l.text.is_empty() {
                if l.direction.is_empty() {
                    l.direction = d;
                } else {
                    // a V.O. cue AND a parenthetical: both are real direction
                    l.direction = format!("{}, {d}", l.direction);
                }
            }
            continue;
        }
        if is_italic_action(raw) {
            continue;
        }

        // Hard-wrapped prose: join rather than split.
        if !l.text.is_empty() {
            l.text.push(' ');
        }
        l.text.push_str(t);
    }
    if let Some(l) = cur {
        if !l.text.is_empty() {
            out.push(l);
        }
    }
    out
}

/// Build a bank from a script, with derived ids.
///
/// Everything lifted is a [`Kind::Phrase`]: a written speech plays whole and is
/// never spliced. Blips are authored separately, because a script does not
/// contain them.
pub fn into_bank(markdown: &str, bank: &mut Bank) -> usize {
    let mut ids: Vec<String> = bank.units().iter().map(|u| u.id.clone()).collect();
    let mut n = 0;
    for l in lines(markdown) {
        let id = derive_unique(&l.speaker, &l.text, ids.iter().map(|s| s.as_str()));
        if id.is_empty() {
            continue;
        }
        ids.push(id.clone());
        let mut u = Unit::new(&id, &l.speaker, &l.text, Kind::Phrase);
        if !l.direction.is_empty() {
            u = u.directed(&l.direction);
        }
        bank.add(u);
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped exactly like the real documents, front matter and all.
    const SCENE: &str = "\
# FILM 5 — SOLAR

**Status**: Reconciled replacement for the abandoned material.

**Premise**: The Coalition's fleet is operating past the shield boundary.

### SCENE 1.1 — THE EDGE

**Setting**: Weeks after the shield crossing.

**Characters**: Halyard, Aiden, the Morpher, Meridian

---

*Wreckage. Hijack wreckage, specifically -- dozens of hulls, cooling.*

**MERIDIAN**
Sensor ghosts on three headings. Nothing that holds a shape long enough to
call a contact.

**HALYARD**
Hijacks.

**AIDEN**
*(quiet)*
Did he do all of this alone?

*A long beat. He isn't wrong.*

---
";

    #[test]
    fn front_matter_does_not_get_a_speaking_part() {
        // THE TRAP. `**Setting**: ...` is bold at the start of a line, so a
        // parser matching "starts with **" imports the front matter as
        // dialogue and gives Setting and Premise speaking parts — silently,
        // because the result looks like a plausible bank.
        let ls = lines(SCENE);
        let speakers: Vec<&str> = ls.iter().map(|l| l.speaker.as_str()).collect();
        for bad in ["Status", "Premise", "Setting", "Characters"] {
            assert!(!speakers.contains(&bad), "{bad} got a speaking part: {speakers:?}");
        }
        assert_eq!(speakers, vec!["MERIDIAN", "HALYARD", "AIDEN"]);
    }

    #[test]
    fn a_wrapped_speech_is_one_line_not_three() {
        // Screenplay prose is hard-wrapped at 78 columns, so a speech is
        // several source lines and ONE spoken line. Splitting would hand the
        // actor two half-sentences with no clue they belong together.
        let ls = lines(SCENE);
        assert_eq!(
            ls[0].text,
            "Sensor ghosts on three headings. Nothing that holds a shape long enough to call a contact."
        );
        assert!(!ls[0].text.contains('\n'));
    }

    #[test]
    fn a_parenthetical_becomes_direction_and_never_a_recorded_line() {
        let ls = lines(SCENE);
        let aiden = ls.iter().find(|l| l.speaker == "AIDEN").unwrap();
        assert_eq!(aiden.direction, "quiet");
        assert_eq!(aiden.text, "Did he do all of this alone?");
        assert!(!aiden.text.contains("quiet"), "direction leaked into the spoken text");
    }

    #[test]
    fn action_lines_are_dropped_entirely() {
        // Italic without parentheses is scene action. It is neither speech nor
        // direction for an actor reading a line.
        let joined: String = lines(SCENE).iter().map(|l| l.text.clone()).collect();
        assert!(!joined.contains("Wreckage"), "an action line was recorded");
        assert!(!joined.contains("long beat"), "an action line was recorded");
    }

    #[test]
    fn a_scene_break_ends_a_speech_rather_than_swallowing_the_next_one() {
        let ls = lines(SCENE);
        assert_eq!(ls.len(), 3);
        assert_eq!(ls[2].text, "Did he do all of this alone?");
    }

    #[test]
    fn a_bank_built_from_a_script_is_recordable() {
        // The end of the chain: script in, ids derived, faults clean apart
        // from having no takes yet.
        let mut b = Bank::default();
        assert_eq!(into_bank(SCENE, &mut b), 3);
        assert_eq!(b.speakers(), vec!["AIDEN", "HALYARD", "MERIDIAN"]);
        assert!(b.units().iter().all(|u| u.kind == Kind::Phrase));
        assert!(b.units().iter().any(|u| u.id == "meridian_sensor_ghosts_on_three"));
        // every fault is a missing take, not a broken id
        assert!(
            b.faults().iter().all(|f| matches!(f, crate::Fault::Short { .. })),
            "{:?}",
            b.faults()
        );
        // and the direction survived onto the page
        assert!(b.script_for("AIDEN").contains("quiet"));
    }

    #[test]
    fn two_speeches_that_open_alike_get_distinct_ids() {
        let script = "\
**BILLY**
I know.

**BILLY**
I know.
";
        let mut b = Bank::default();
        assert_eq!(into_bank(script, &mut b), 2);
        let ids: Vec<&str> = b.units().iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, vec!["billy_i_know", "billy_i_know_2"]);
        assert!(b.faults().iter().all(|f| matches!(f, crate::Fault::Short { .. })));
    }


    #[test]
    fn a_voice_over_cue_is_the_same_actor_as_the_on_camera_one() {
        // FOUND BY RUNNING THIS AGAINST THE REAL SCRIPTS, not by the fixture:
        // they returned seven speakers for five actors, because
        // `**THE MORPHER (V.O.)**` and `**THE MORPHER**` are one person with a
        // different microphone. Left split, that actor gets two scripts and
        // half their lines are missing from each.
        let script = "**THE MORPHER (V.O.)**\nOrbits turn without concern.\n\n**THE MORPHER**\nI do not yet know what.\n";
        let ls = lines(script);
        assert_eq!(ls[0].speaker, "THE MORPHER", "the V.O. suffix split the roster");
        assert_eq!(ls[1].speaker, "THE MORPHER");
        assert_eq!(ls[0].direction, "V.O.", "the suffix was discarded instead of kept");
        assert!(ls[1].direction.is_empty());

        let mut b = Bank::default();
        into_bank(script, &mut b);
        assert_eq!(b.speakers(), vec!["THE MORPHER"], "one actor, one script");
    }

    #[test]
    fn an_empty_or_dialogue_free_document_yields_nothing() {
        assert!(lines("").is_empty());
        assert!(lines("# Title\n\nJust prose, no cues.\n").is_empty());
        // a cue with no speech under it is not a line
        assert!(lines("**BILLY**\n\n---\n").is_empty());
    }
}
