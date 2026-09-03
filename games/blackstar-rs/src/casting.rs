//! WHO A GENERATED RAID IS ALLOWED TO PUT IN IT.
//!
//! The generative half already exists. `raidgen` turns a boss's fiction into
//! three playable phases from a seed, with every rule tested; `raidforge` asks
//! a llama3-class model for a raid and parses it leniently, and the Wardeck's
//! forge rows already emit those orders. So infinite raids are not the missing
//! piece — they ship.
//!
//! What a generated raid cannot currently say is WHO IS THERE. `EncSpec`
//! carries a position, a kind, a mechanic and a unit count. That is enough to
//! build a fight and not enough to build a moment: the same gap `content.rs`
//! had before `Presence`, where a mission could name its opposition but not
//! the people standing next to you.
//!
//! ## Why this needs a rule rather than just a field
//!
//! Casting is where infinite content damages a finite story. Three ways, all
//! of which a generator will find on its own:
//!
//! * **Spoiling.** A forged raid that casts the Morpher before Film 4 has
//!   introduced him hands the player a character the campaign was about to
//!   reveal. Generated content must not run ahead of authored content.
//! * **Contradiction.** Cleave plants himself under the closing plates in
//!   `beneath_the_pacific` and is never found. A raid that casts him after
//!   that is not a cameo, it is the campaign being wrong.
//! * **Cheapening.** A character who turns up in every third procedural raid
//!   stops being a character. Presence has to stay rare enough to mean
//!   something, which is a budget, not a filter.
//!
//! So `castable` answers "may this character appear, for this player, right
//! now" from the campaign log — the same mechanism as `raidcon::unlocked` and
//! `patrol::bridge_powered`, because a third way of asking "how far in are
//! you" would be a third thing to keep in sync.

use crate::campaign::CampaignLog;
use crate::content::Presence;

/// A character a generated raid may draw on, and the campaign facts that
/// govern them.
pub struct Castable {
    /// `codex.rs` id, same key `content::Mission::cast` uses.
    pub id: &'static str,
    /// The mission that introduces them. Before it is cleared they cannot
    /// appear, because a procedural raid must not pre-empt a written reveal.
    pub introduced_by: &'static str,
    /// The mission after which they cannot appear again, if any. This is how
    /// the campaign stays true while content keeps generating.
    pub written_out_by: Option<&'static str>,
    /// The strongest presence a GENERATED raid may give them. Authored
    /// missions are unconstrained; a forged one should not be handing out
    /// playable Keelwrights.
    pub ceiling: Presence,
}

/// The roster a forge may draw from.
///
/// Deliberately short. Every entry costs the character something — see the
/// cheapening note in the module doc — so this is a list of people whose
/// presence in a procedural raid would still mean something, not everyone the
/// codex knows about.
pub const CASTABLE: &[Castable] = &[
    Castable {
        id: "the_morpher",
        introduced_by: "the_far_gate",
        written_out_by: None,
        ceiling: Presence::Interactable,
    },
    Castable {
        id: "billy_the_fire_clone",
        introduced_by: "the_tide_forges",
        written_out_by: None,
        ceiling: Presence::Ally,
    },
    Castable {
        id: "halyard",
        introduced_by: "the_drowned_approach",
        written_out_by: None,
        ceiling: Presence::Ally,
    },
    Castable {
        id: "keelwright",
        introduced_by: "the_pluto_line",
        written_out_by: None,
        ceiling: Presence::Seen,
    },
    // Cleave plants himself under the closing plates and is never found.
    // He is castable only in the window where that has not happened yet.
    Castable {
        id: "cleave",
        introduced_by: "the_ashen_truce",
        written_out_by: Some("beneath_the_pacific"),
        ceiling: Presence::Seen,
    },
];

pub fn entry(id: &str) -> Option<&'static Castable> {
    CASTABLE.iter().find(|c| c.id == id)
}

/// How a cast member is BILLED to one particular player.
///
/// The insight this exists for: a player who has not met someone should not be
/// lied to about who is standing next to them. Substituting an anonymous
/// stand-in — "a Fire Clone" — is a lie that a friend on voice chat will
/// immediately contradict, and the player then learns the game misled them.
/// `???` does not lie. It says there is something here you do not know yet,
/// which is true, and it hands the spoiler decision to the table instead of
/// taking it: ask your friends or do not, and either is a choice you made.
///
/// It also turns the gate from a WALL into a FUNNEL. A casual player who is
/// six missions behind can raid with their group, meet an unnamed someone who
/// fights beside them, and go looking for which mission that was. Locking them
/// out teaches nothing; redacting teaches them there is something to find.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Billing {
    /// What this player sees on the label and in dialogue.
    pub label: &'static str,
    /// Do they know who it is? False means the name is redacted for them and
    /// nobody else — the same raid, the same character, a different label.
    pub known: bool,
    pub presence: Presence,
}

/// The label shown to a player who has not met this character.
pub const REDACTED: &'static str = "???";

/// Bill a cast member for one viewer.
///
/// PER-VIEWER, not per-lobby. Nobody is excluded from a raid because of where
/// they are in the story — the seventh friend joins whatever is running — and
/// nobody is spoiled either, because identity resolves against THEIR campaign
/// log rather than the host's. Same encounter, same fight, same loot; one
/// player sees "billy" and another sees "???".
///
/// This works only while a forged cast is flavour: every ceiling in `CASTABLE`
/// is Ally, Interactable or Seen, never Playable, so no build, damage number
/// or objective depends on who is standing there. The day a character becomes
/// mechanically necessary — a door only the Morpher opens — redaction stops
/// being safe and that raid has to gate the lobby instead. Keeping forged
/// casts flavour-only is what keeps this available.
pub fn billing(id: &str, log: &CampaignLog) -> Option<Billing> {
    let c = entry(id)?;
    // written out is still written out: redaction cannot resurrect anyone,
    // because a player who HAS seen the plates close would be shown a
    // contradiction rather than a mystery
    if let Some(out) = c.written_out_by {
        if log.is_cleared(out) {
            return None;
        }
    }
    let known = log.is_cleared(c.introduced_by);
    Some(Billing {
        label: if known { c.id } else { REDACTED },
        known,
        presence: c.ceiling,
    })
}

/// May a generated raid cast this character for this player, and how strongly?
///
/// `None` means no. A `Some(p)` is the CEILING, not an instruction — the
/// generator may use a weaker presence and usually should.
pub fn castable(id: &str, log: &CampaignLog) -> Option<Presence> {
    let c = entry(id)?;
    if !log.is_cleared(c.introduced_by) {
        return None; // not met yet: a forged raid must not pre-empt the reveal
    }
    if let Some(out) = c.written_out_by {
        if log.is_cleared(out) {
            return None; // the campaign has closed their story
        }
    }
    Some(c.ceiling)
}

/// Everyone a forge may draw on right now.
pub fn available(log: &CampaignLog) -> Vec<&'static str> {
    CASTABLE.iter().filter(|c| castable(c.id, log).is_some()).map(|c| c.id).collect()
}

/// How many named characters one generated raid may carry.
///
/// A budget rather than a filter: presence is only worth something while it is
/// scarce, and a generator asked for "someone interesting" will otherwise cast
/// everyone available every time.
pub const CAST_BUDGET: usize = 1;

/// Trim a proposed cast to what is allowed, in the order given.
pub fn admit(proposed: &[&'static str], log: &CampaignLog) -> Vec<(&'static str, Presence)> {
    let mut out = Vec::new();
    for id in proposed {
        if out.len() >= CAST_BUDGET {
            break;
        }
        if out.iter().any(|(x, _): &(&str, Presence)| x == id) {
            continue; // a model that names someone twice gets them once
        }
        if let Some(p) = castable(id, log) {
            out.push((*id, p));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(ids: &[&str]) -> CampaignLog {
        CampaignLog::from_line(&ids.join(","))
    }

    #[test]
    fn a_forged_raid_cannot_pre_empt_a_reveal() {
        // The failure that matters most: infinite content running ahead of
        // authored content and handing the player a character the campaign
        // was about to introduce.
        let fresh = log(&[]);
        assert_eq!(castable("the_morpher", &fresh), None, "the Morpher before Film 4");
        assert_eq!(castable("billy_the_fire_clone", &fresh), None, "Billy before Film 5");
        assert!(available(&fresh).is_empty(), "a fresh save can already cast someone");

        let after_four = log(&["the_far_gate"]);
        assert!(castable("the_morpher", &after_four).is_some(), "met and still uncastable");
    }

    #[test]
    fn the_campaign_can_write_someone_out() {
        // Cleave is never found after the plates close. A raid casting him
        // afterwards is not a cameo, it is the campaign being wrong.
        let during = log(&["the_ashen_truce"]);
        assert!(castable("cleave", &during).is_some(), "the truce window is closed");
        let after = log(&["the_ashen_truce", "beneath_the_pacific"]);
        assert_eq!(castable("cleave", &after), None, "cast after he is written out");
        // and the others are unaffected by his exit
        assert!(castable("keelwright", &log(&["the_pluto_line", "beneath_the_pacific"])).is_some());
    }

    #[test]
    fn presence_has_a_ceiling_and_a_budget() {
        // Two different limits, and both are needed. A ceiling stops a forge
        // handing out playable Keelwrights; a budget stops it casting everyone
        // available every single time, which is what makes presence stop
        // meaning anything.
        let late = log(&["the_pluto_line", "the_drowned_approach", "the_far_gate", "the_tide_forges"]);
        assert!(available(&late).len() >= 3, "not enough cast to test a budget");

        assert_eq!(castable("keelwright", &late), Some(Presence::Seen), "Keelwright is not playable in a forge");
        assert!(
            !matches!(castable("keelwright", &late), Some(Presence::Playable)),
            "a generated raid can cast a playable lead"
        );

        let asked = ["keelwright", "halyard", "billy_the_fire_clone", "the_morpher"];
        let got = admit(&asked, &late);
        assert_eq!(got.len(), CAST_BUDGET, "the budget was not applied: {got:?}");
        assert_eq!(got[0].0, "keelwright", "admit did not respect the order asked for");
    }

    #[test]
    fn admit_survives_a_model_that_says_nonsense() {
        // raidforge parses model output leniently on purpose — "unknown arenas
        // drop the phase, unknown mechs drop the line". Casting has to be as
        // forgiving: an invented name is dropped, a repeat is collapsed, and a
        // request nobody has met yields nothing rather than an error.
        let late = log(&["the_pluto_line"]);
        let junk = ["a_bot_that_does_not_exist", "keelwright", "keelwright"];
        let got = admit(&junk, &late);
        assert_eq!(got.len(), 1, "nonsense was not filtered: {got:?}");
        assert_eq!(got[0].0, "keelwright");
        assert!(admit(&["the_morpher"], &late).is_empty(), "unmet character admitted");
        assert!(admit(&[], &late).is_empty());
    }

    #[test]
    fn an_unmet_character_is_redacted_not_replaced() {
        // The distinction that matters. A stand-in ("a Fire Clone") is a lie a
        // friend on voice chat contradicts in one sentence; `???` is true and
        // hands the spoiler decision to the table.
        let fresh = log(&[]);
        let b = billing("billy_the_fire_clone", &fresh).expect("present, just unknown");
        assert_eq!(b.label, REDACTED, "an unmet character was named");
        assert!(!b.known);
        assert_eq!(b.presence, Presence::Ally, "redaction changed what they DO");

        let met = log(&["the_tide_forges"]);
        let b2 = billing("billy_the_fire_clone", &met).expect("met");
        assert_eq!(b2.label, "billy_the_fire_clone");
        assert!(b2.known);
        assert_eq!(b2.presence, b.presence, "meeting them changed the encounter");
    }

    #[test]
    fn the_same_raid_bills_two_players_differently() {
        // Per-viewer, not per-lobby: nobody is excluded and nobody is spoiled.
        // The seventh friend joins whatever is running.
        let behind = log(&[]);
        let ahead = log(&["the_tide_forges"]);
        let a = billing("billy_the_fire_clone", &behind).expect("a");
        let b = billing("billy_the_fire_clone", &ahead).expect("b");
        assert_ne!(a.label, b.label, "both players see the same label");
        assert_eq!(a.presence, b.presence, "the encounter differs between players");
    }

    #[test]
    fn redaction_cannot_resurrect_someone_the_campaign_closed() {
        // The one case where hiding the name is wrong. A player who has seen
        // the plates close would be shown a contradiction rather than a
        // mystery, so written-out still means absent — for everyone.
        let after = log(&["the_ashen_truce", "beneath_the_pacific"]);
        assert_eq!(billing("cleave", &after), None, "the dead were redacted back in");
        // and before that, an unmet player sees the mystery rather than nothing
        let early = log(&[]);
        let m = billing("cleave", &early).expect("present in the window");
        assert_eq!(m.label, REDACTED);
    }

    #[test]
    fn billing_and_castable_agree_about_who_is_absent() {
        // Two functions answering adjacent questions. `castable` is the strict
        // gate (may a forge PLAN this), `billing` is the per-viewer view (what
        // does this player SEE). They must never disagree about someone being
        // written out, or a raid plans a character the viewer cannot be shown.
        for c in CASTABLE {
            for l in [
                log(&[]),
                log(&["the_pluto_line"]),
                log(&["the_ashen_truce"]),
                log(&["the_ashen_truce", "beneath_the_pacific"]),
                log(&["the_pluto_line", "the_far_gate", "the_tide_forges"]),
            ] {
                let gone_strict = castable(c.id, &l).is_none() && {
                    // distinguish "not yet met" from "written out"
                    c.written_out_by.map(|o| l.is_cleared(o)).unwrap_or(false)
                };
                let gone_view = billing(c.id, &l).is_none();
                assert_eq!(
                    gone_strict, gone_view,
                    "{} disagrees between castable and billing",
                    c.id
                );
            }
        }
    }

    #[test]
    fn every_castable_names_missions_that_exist() {
        // The ids are campaign facts, so a typo would silently make someone
        // permanently uncastable — the worst kind of wrong, because nothing
        // errors and a character simply never appears.
        for c in CASTABLE {
            assert!(
                crate::content::MISSIONS.iter().any(|m| m.id == c.introduced_by),
                "{} is introduced by '{}', which is not a mission",
                c.id,
                c.introduced_by
            );
            if let Some(out) = c.written_out_by {
                assert!(
                    crate::content::MISSIONS.iter().any(|m| m.id == out),
                    "{} is written out by '{out}', which is not a mission",
                    c.id
                );
            }
            // and the character is someone the game can talk about
            assert!(
                include_str!("codex.rs").contains(&format!("id: \"{}\"", c.id)),
                "{} has no codex entry",
                c.id
            );
        }
        assert!(CASTABLE.len() >= 3, "the castable roster shrank to {}", CASTABLE.len());
    }
}
