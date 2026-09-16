//! stems_dme — the three stems a film is actually delivered in.
//!
//! `export`'s `stems: bool` gives one file per track, which is the music
//! convention and is right for music. Film delivery is a different shape: a
//! mix goes out as **Dialogue, Music and Effects**, and the reason is
//! specific. A distributor dubbing the picture into another language replaces
//! the D stem and keeps M and E, so the score and the footsteps and the door
//! slams survive translation. That combination — music plus effects without
//! dialogue — is the **M&E**, and it is the single most requested deliverable
//! in post.
//!
//! A per-track stem export cannot answer that request. Forty tracks is not
//! three, and the distributor does not want to be told which of them is a
//! boom mic.
//!
//! ## An unassigned track is refused, never guessed
//!
//! The obvious convenience is to default unassigned tracks to Effects. That is
//! the one thing this module must not do.
//!
//! A dialogue track misfiled into Effects cannot be dubbed out. It ships, it
//! passes QC because the mix sounds right in the original language, and the
//! error surfaces when a foreign distributor hears an English line under their
//! own dub — at which point the fix is a recall. Every other failure here is
//! caught by listening to the file; that one is not, because the file sounds
//! correct.
//!
//! So [`plan`] refuses and names the tracks. A person spending thirty seconds
//! assigning them is the cheap outcome.

use std::collections::BTreeMap;
use studio_core::{Project, TrackId, TrackKind};

/// Which deliverable a track belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Production sound, ADR, voiceover. Replaced on a dub.
    Dialogue,
    /// Score and source music.
    Music,
    /// Foley, hard effects, backgrounds, ambience.
    Effects,
}

impl Role {
    /// In delivery order, which is the order a spec sheet lists them.
    pub const ALL: [Role; 3] = [Role::Dialogue, Role::Music, Role::Effects];

    /// The short tag used in delivered filenames.
    pub fn tag(self) -> &'static str {
        match self {
            Role::Dialogue => "DX",
            Role::Music => "MX",
            Role::Effects => "FX",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Role::Dialogue => "Dialogue",
            Role::Music => "Music",
            Role::Effects => "Effects",
        }
    }

    /// Does this stem survive into the M&E?
    pub fn in_me(self) -> bool {
        !matches!(self, Role::Dialogue)
    }
}

/// Track-to-role assignments.
///
/// Sparse, and a map rather than a field on `Track`, for the same reason
/// `Rack::swing_overrides` is: a music project assigns nothing and stores
/// nothing, and adding a field would mean editing every `Track` literal in the
/// tree for a feature most projects never use.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Roles {
    by_track: BTreeMap<TrackId, Role>,
}

impl Roles {
    pub fn new() -> Roles {
        Roles::default()
    }

    pub fn set(&mut self, track: TrackId, role: Role) {
        self.by_track.insert(track, role);
    }

    pub fn clear(&mut self, track: TrackId) {
        self.by_track.remove(&track);
    }

    pub fn get(&self, track: TrackId) -> Option<Role> {
        self.by_track.get(&track).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.by_track.is_empty()
    }

    /// Audio tracks with no role. The list a refusal names.
    pub fn unassigned(&self, project: &Project) -> Vec<TrackId> {
        project
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio && !self.by_track.contains_key(&t.id))
            .map(|t| t.id)
            .collect()
    }

    /// Tracks in a role, in project order.
    pub fn tracks_in(&self, project: &Project, role: Role) -> Vec<TrackId> {
        project
            .tracks
            .iter()
            .filter(|t| self.by_track.get(&t.id) == Some(&role))
            .map(|t| t.id)
            .collect()
    }
}

/// One stem to render: a name and the tracks soloed into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stem {
    pub name: String,
    pub solo: Vec<TrackId>,
}

/// What went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Audio tracks carry no role. Named, because guessing is the one
    /// unrecoverable mistake here — see the module header.
    Unassigned(Vec<TrackId>),
    /// Nothing to deliver.
    NoAudioTracks,
    /// A role has no tracks at all.
    ///
    /// A warning rather than a hard stop in most houses, but refused here for
    /// **Dialogue specifically**: a film with no dialogue stem is either a
    /// silent film or a misassignment, and the second is far more likely.
    EmptyDialogue,
}

impl Refusal {
    pub fn explain(&self, project: &Project) -> String {
        match self {
            Refusal::NoAudioTracks => "there are no audio tracks to deliver".into(),
            Refusal::EmptyDialogue => "no track is assigned to Dialogue — if this really has \
                no dialogue, assign an empty track to it so the delivery is explicit"
                .into(),
            Refusal::Unassigned(ids) => {
                let names: Vec<&str> = ids
                    .iter()
                    .filter_map(|id| project.tracks.iter().find(|t| t.id == *id))
                    .map(|t| t.name.as_str())
                    .collect();
                format!(
                    "assign a stem role to {} before delivering: {}. \
                     Guessing is not safe — a dialogue track filed under Effects \
                     cannot be dubbed out, and the error is only found abroad.",
                    names.len(),
                    names.join(", ")
                )
            }
        }
    }
}

/// The three stems, plus the M&E if asked for.
///
/// `me` adds a fourth job soloing Music and Effects together. It is redundant
/// in principle — a distributor can sum MX and FX — and requested anyway,
/// because delivery specs ask for the file and not for the instructions.
pub fn plan(project: &Project, roles: &Roles, me: bool) -> Result<Vec<Stem>, Refusal> {
    let audio: Vec<TrackId> = project
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .map(|t| t.id)
        .collect();
    if audio.is_empty() {
        return Err(Refusal::NoAudioTracks);
    }
    let missing = roles.unassigned(project);
    if !missing.is_empty() {
        return Err(Refusal::Unassigned(missing));
    }
    if roles.tracks_in(project, Role::Dialogue).is_empty() {
        return Err(Refusal::EmptyDialogue);
    }

    let mut out: Vec<Stem> = Role::ALL
        .iter()
        .map(|&r| Stem { name: r.tag().to_string(), solo: roles.tracks_in(project, r) })
        .collect();

    if me {
        // Built from `in_me` rather than by naming Music and Effects, so a
        // fourth role added later is classified once rather than in two
        // places that can disagree.
        let solo: Vec<TrackId> = project
            .tracks
            .iter()
            .filter(|t| roles.get(t.id).map(Role::in_me).unwrap_or(false))
            .map(|t| t.id)
            .collect();
        out.push(Stem { name: "M&E".into(), solo });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Op, Track};

    fn project(names: &[&str]) -> Project {
        let mut p = Project::default();
        for (i, n) in names.iter().enumerate() {
            let track = Track {
                id: TrackId(i as u64 + 1),
                kind: TrackKind::Audio,
                name: (*n).into(),
                muted: false,
                locked: false,
                gain: 1.0,
                route: None,
            };
            p.apply(&Op::AddTrack { track, at: i }).unwrap();
        }
        p
    }

    fn assigned(p: &Project) -> Roles {
        let mut r = Roles::new();
        r.set(TrackId(1), Role::Dialogue);
        r.set(TrackId(2), Role::Music);
        r.set(TrackId(3), Role::Effects);
        let _ = p;
        r
    }

    /// THE DELIVERABLE. Three stems, in spec-sheet order, each soloing its own
    /// tracks.
    #[test]
    fn three_stems_come_out_in_delivery_order() {
        let p = project(&["boom", "score", "foley"]);
        let s = plan(&p, &assigned(&p), false).unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(s.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
                   vec!["DX", "MX", "FX"]);
        assert_eq!(s[0].solo, vec![TrackId(1)]);
    }

    /// THE M&E, which is the whole reason for the split: music plus effects,
    /// dialogue gone, so the picture can be dubbed.
    #[test]
    fn the_me_carries_music_and_effects_but_never_dialogue() {
        let p = project(&["boom", "score", "foley"]);
        let s = plan(&p, &assigned(&p), true).unwrap();
        let me = s.iter().find(|x| x.name == "M&E").expect("an M&E");
        assert!(me.solo.contains(&TrackId(2)) && me.solo.contains(&TrackId(3)));
        assert!(!me.solo.contains(&TrackId(1)), "dialogue must not survive into the M&E");
    }

    /// THE ONE UNRECOVERABLE MISTAKE. A dialogue track defaulted into Effects
    /// ships, passes QC in the original language, and is found abroad.
    #[test]
    fn an_unassigned_track_is_refused_by_name_rather_than_guessed() {
        let p = project(&["boom", "score", "foley", "crowd loop"]);
        let mut r = assigned(&p);
        r.clear(TrackId(4));
        // (it was never set, but be explicit about the premise)
        match plan(&p, &r, false) {
            Err(Refusal::Unassigned(ids)) => {
                assert_eq!(ids, vec![TrackId(4)]);
                let msg = Refusal::Unassigned(ids).explain(&p);
                assert!(msg.contains("crowd loop"), "names the track: {msg}");
                assert!(msg.contains("dubbed out"), "says why guessing is unsafe: {msg}");
            }
            other => panic!("must refuse: {other:?}"),
        }
    }

    /// A film with no dialogue stem is far more likely a misassignment than a
    /// silent film, so it is refused with a way to say you meant it.
    #[test]
    fn a_missing_dialogue_stem_is_refused_with_an_escape_hatch() {
        let p = project(&["score", "foley"]);
        let mut r = Roles::new();
        r.set(TrackId(1), Role::Music);
        r.set(TrackId(2), Role::Effects);
        let err = plan(&p, &r, false).unwrap_err();
        assert_eq!(err, Refusal::EmptyDialogue);
        assert!(err.explain(&p).contains("assign an empty track"), "{}", err.explain(&p));
    }

    /// Empty Music or Effects is fine — a documentary with no score is
    /// ordinary, and refusing it would be the tool having an opinion about the
    /// film.
    #[test]
    fn an_empty_music_stem_is_allowed() {
        let p = project(&["boom", "foley"]);
        let mut r = Roles::new();
        r.set(TrackId(1), Role::Dialogue);
        r.set(TrackId(2), Role::Effects);
        let s = plan(&p, &r, true).unwrap();
        assert_eq!(s.len(), 4);
        assert!(s.iter().find(|x| x.name == "MX").unwrap().solo.is_empty());
        assert_eq!(s.iter().find(|x| x.name == "M&E").unwrap().solo, vec![TrackId(2)]);
    }

    #[test]
    fn a_project_with_no_audio_has_nothing_to_deliver() {
        let p = Project::default();
        assert_eq!(plan(&p, &Roles::new(), false), Err(Refusal::NoAudioTracks));
    }

    /// Roles are sparse: a music project assigns nothing and stores nothing.
    #[test]
    fn roles_are_empty_until_something_is_assigned() {
        let p = project(&["a", "b"]);
        let r = Roles::new();
        assert!(r.is_empty());
        assert_eq!(r.unassigned(&p).len(), 2);
        assert_eq!(r.get(TrackId(1)), None);
    }

    /// `in_me` is the single classifier, so a fourth role later is decided
    /// once rather than in two places that can disagree.
    #[test]
    fn the_me_membership_rule_lives_in_one_place() {
        assert!(!Role::Dialogue.in_me());
        assert!(Role::Music.in_me() && Role::Effects.in_me());
    }
}
