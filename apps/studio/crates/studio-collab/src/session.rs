//! session — who is in the room, and what happens when they leave.
//!
//! `Doc` converges and `carrier` moves bytes, but neither has any notion of
//! membership. That gap shows up as three practical problems: a session nobody
//! can join deliberately, presence for people who left hours ago, and no answer
//! to "is anyone still working on this?" when deciding whether a project can be
//! archived.
//!
//! ## Teams come from snapclone contacts
//!
//! A [`Team`] is a name plus a list of snapclone sc-IDs — the contacts already
//! on the device. Nothing new to configure, no separate account: if you can
//! send someone a snap, you can invite them to an edit. The session key is
//! derived from the team id the same way snapclone derives a key from an sc-ID,
//! so every member computes the same key independently and nobody has to be
//! told where the session lives.
//!
//! ## The distinction that matters on leave
//!
//! Leaving removes your **presence**, not your **edits**. A collaborator who
//! goes home has still done the work; their ops are part of the project's
//! history and stay in the log. Deleting a departed peer's contributions would
//! be catastrophic and is exactly what a naive "clean up when they disconnect"
//! does. So [`Session::leave`] drops presence, releases their in-flight partial
//! transfers, and touches nothing else — and there is a test that the project is
//! byte-identical afterwards.
//!
//! ## Crashes do not send goodbye
//!
//! The other half of membership: a peer whose laptop shuts mid-edit never sends
//! a leave. Liveness is therefore a timeout on the last thing heard from them,
//! which is why [`Presence`] already carries a sequence number and why
//! [`Session::sweep`] takes a clock. A session that only removes people who left
//! politely accumulates ghosts.

use crate::carrier::session_key;
use crate::{Doc, Presence, SiteId};
use std::collections::BTreeMap;

/// A snapclone contact identifier, e.g. `sc-32a51661`.
pub type ScId = String;

/// A group of contacts a project can be shared with.
#[derive(Debug, Clone, PartialEq)]
pub struct Team {
    /// Stable id — this is what the session key is derived from, so it must not
    /// change when the team is renamed or its membership edited.
    pub id: String,
    pub name: String,
    pub members: Vec<ScId>,
}

impl Team {
    /// Build a team from snapclone contacts. The id is derived from the sorted
    /// member list so that two people creating "the same team" independently
    /// land on the same session, which is usually what they meant.
    pub fn from_contacts(name: &str, contacts: &[ScId]) -> Team {
        let mut members: Vec<ScId> = contacts.to_vec();
        members.sort();
        members.dedup();
        let joined = members.join(",");
        Team { id: format!("{:016x}", crate::assets::hash(joined.as_bytes())), name: name.into(), members }
    }

    /// Where this team's sessions live. Deterministic, so no directory service
    /// is required to find a session — everyone derives the same key.
    pub fn session_key(&self, project: &str) -> [u8; 20] {
        session_key(&format!("{}/{}", self.id, project))
    }

    pub fn contains(&self, sc: &str) -> bool { self.members.iter().any(|m| m == sc) }
}

/// One participant, as the roster sees them.
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub site: SiteId,
    pub sc_id: ScId,
    pub name: String,
    /// Clock reading when we last heard anything at all from them.
    pub last_seen_ms: i64,
}

/// How long a peer can go silent before being swept. Generous, because the cost
/// of dropping someone who was merely thinking is that they reappear a moment
/// later looking like they rejoined.
pub const LIVENESS_MS: i64 = 45_000;

/// Membership over a `Doc`.
pub struct Session {
    pub team: Team,
    pub project_name: String,
    pub doc: Doc,
    members: BTreeMap<SiteId, Member>,
    /// Sites that have left or been swept, kept so a late packet from them is
    /// recognised as stale rather than treated as a rejoin.
    departed: BTreeMap<SiteId, i64>,
}

/// What changed, for the UI to announce.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Joined(SiteId),
    Left(SiteId),
    /// Swept for silence rather than having said goodbye.
    TimedOut(SiteId),
    Rejoined(SiteId),
}

impl Session {
    pub fn open(team: Team, project: &str, doc: Doc) -> Session {
        Session {
            team, project_name: project.into(), doc,
            members: BTreeMap::new(), departed: BTreeMap::new(),
        }
    }

    pub fn key(&self) -> [u8; 20] { self.team.session_key(&self.project_name) }

    pub fn roster(&self) -> Vec<&Member> {
        let mut v: Vec<&Member> = self.members.values().collect();
        v.sort_by_key(|m| m.site.0);
        v
    }

    pub fn is_present(&self, site: SiteId) -> bool { self.members.contains_key(&site) }

    /// Someone arrives. Membership is checked against the team, because a
    /// session key is not a secret — it is derived from a team id, so anyone who
    /// can guess the id can find the session, and the roster is what actually
    /// decides who counts as a participant.
    pub fn join(&mut self, site: SiteId, sc_id: &str, name: &str, now_ms: i64) -> Option<Event> {
        if !self.team.contains(sc_id) { return None; }
        let rejoin = self.departed.remove(&site).is_some();
        let already = self.members.contains_key(&site);
        self.members.insert(site, Member {
            site, sc_id: sc_id.into(), name: name.into(), last_seen_ms: now_ms,
        });
        if already { None } else if rejoin { Some(Event::Rejoined(site)) } else { Some(Event::Joined(site)) }
    }

    /// Someone leaves deliberately.
    ///
    /// Their PRESENCE goes; their EDITS stay. A collaborator who goes home has
    /// still done the work, and their ops are part of the project's history.
    pub fn leave(&mut self, site: SiteId, now_ms: i64) -> Option<Event> {
        let m = self.members.remove(&site)?;
        self.departed.insert(site, now_ms);
        self.doc.peers.remove(&site);
        let _ = m;
        Some(Event::Left(site))
    }

    /// Note that we heard from someone.
    pub fn touch(&mut self, site: SiteId, now_ms: i64) {
        if let Some(m) = self.members.get_mut(&site) { m.last_seen_ms = now_ms; }
    }

    /// Merge a presence packet AND count it as a sign of life — the two always
    /// happen together, and separating them is how a peer who is actively
    /// scrubbing gets swept for silence.
    pub fn presence(&mut self, p: Presence, now_ms: i64) {
        let site = p.site;
        self.doc.presence(p);
        self.touch(site, now_ms);
    }

    /// Remove anyone who has gone quiet. A peer whose laptop shut mid-edit never
    /// sends a leave, so a session that only removes polite departures fills up
    /// with ghosts.
    pub fn sweep(&mut self, now_ms: i64) -> Vec<Event> {
        let gone: Vec<SiteId> = self.members.values()
            .filter(|m| now_ms - m.last_seen_ms > LIVENESS_MS)
            .map(|m| m.site)
            .collect();
        gone.into_iter().map(|s| {
            self.members.remove(&s);
            self.departed.insert(s, now_ms);
            self.doc.peers.remove(&s);
            Event::TimedOut(s)
        }).collect()
    }

    /// Everything a peer starting from NOTHING needs, in one packet.
    ///
    /// Deliberately a full snapshot rather than the op log. The log only
    /// describes edits made *this session*; a project opened from a file has a
    /// base state that predates it, so replaying the log alone hands a newcomer
    /// a clip op with no track to put it on — which is exactly what the test
    /// caught, and the same class of bug as `Doc::rebuild` starting from an
    /// empty project.
    ///
    /// Reuses `file::snapshot`, so the ops that rebuild a project for a
    /// collaborator are literally the ones that rebuild it from disk. A peer who
    /// already shares the base should ask for `Doc::catchup_for` instead, which
    /// is far smaller.
    pub fn broadcast(&self) -> crate::wire::Packet {
        let base = crate::file::snapshot(&self.doc.project);
        let mut ops: Vec<crate::StampedOp> = base.into_iter().enumerate()
            .map(|(i, op)| crate::StampedOp {
                stamp: crate::Stamp { lamport: i as u64 + 1, site: SiteId(0) }, op,
            })
            .collect();
        // then the session's own history, stamped after the snapshot so a
        // replay applies them in the right order
        let base_len = ops.len() as u64;
        for e in self.doc.history() {
            ops.push(crate::StampedOp {
                stamp: crate::Stamp { lamport: base_len + e.stamp.lamport, site: e.stamp.site },
                op: e.op.clone(),
            });
        }
        crate::wire::Packet::Catchup { ops }
    }

    /// Is anyone still here? The question that decides whether a project can be
    /// archived — and the reason membership exists at all.
    pub fn is_idle(&self) -> bool { self.members.is_empty() }

    /// Members who have not been heard from recently but are not yet swept, for
    /// the "connection may be poor" hint that stops people assuming a
    /// collaborator has silently vanished.
    pub fn stale(&self, now_ms: i64, threshold_ms: i64) -> Vec<SiteId> {
        self.members.values()
            .filter(|m| now_ms - m.last_seen_ms > threshold_ms)
            .map(|m| m.site)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Clip, ClipId, Op, Project, Source, SourceId, Ticks, Track, TrackId, TrackKind};

    fn team() -> Team {
        Team::from_contacts("the cut", &[
            "sc-32a51661".into(), "sc-9f0c22ab".into(), "sc-44de7710".into(),
        ])
    }

    fn seeded_session() -> Session {
        let mut p = Project { name: "cut".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video,
            name: "V1".into(), muted: false, locked: false }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "a.mov".into(),
            len: Ticks::from_seconds(600.0), sample_rate: 48_000, channels: 2, has_video: true } }).unwrap();
        Session::open(team(), "cut", Doc::new(SiteId(1), p))
    }

    #[test]
    fn a_team_is_derived_from_contacts_and_lands_everyone_on_one_key() {
        // Two people building "the same team" independently must find the same
        // session, which is what they meant — so the id comes from the sorted
        // member list rather than creation order.
        let a = Team::from_contacts("cut", &["sc-b".into(), "sc-a".into()]);
        let b = Team::from_contacts("different name", &["sc-a".into(), "sc-b".into()]);
        assert_eq!(a.id, b.id, "order and name must not change the id");
        assert_eq!(a.session_key("proj"), b.session_key("proj"));
        // a different membership is a different team
        let c = Team::from_contacts("cut", &["sc-a".into(), "sc-c".into()]);
        assert_ne!(a.id, c.id);
        // and different projects within a team are different sessions
        assert_ne!(a.session_key("proj1"), a.session_key("proj2"));
        // duplicates in the contact list do not create a new team
        let d = Team::from_contacts("cut", &["sc-a".into(), "sc-b".into(), "sc-a".into()]);
        assert_eq!(a.id, d.id);
    }

    #[test]
    fn only_contacts_on_the_team_can_join() {
        // The session key is not a secret — it is derived from a team id, so
        // anyone who can guess the id can find the session. The roster is what
        // actually decides who counts.
        let mut s = seeded_session();
        assert_eq!(s.join(SiteId(2), "sc-9f0c22ab", "ada", 1000), Some(Event::Joined(SiteId(2))));
        assert!(s.join(SiteId(3), "sc-not-a-member", "mallory", 1000).is_none());
        assert_eq!(s.roster().len(), 1);
        // joining twice is not an event, and does not duplicate the roster entry
        assert_eq!(s.join(SiteId(2), "sc-9f0c22ab", "ada", 1200), None);
        assert_eq!(s.roster().len(), 1);
    }

    #[test]
    fn leaving_removes_presence_but_never_the_work() {
        // THE distinction. A collaborator who goes home has still done the work,
        // and deleting a departed peer's contributions is what a naive "clean up
        // on disconnect" does.
        let mut s = seeded_session();
        s.join(SiteId(2), "sc-9f0c22ab", "ada", 1000);
        // ada makes an edit and reports where she is
        let edit = Op::AddClip { clip: Clip { id: ClipId(7), source: SourceId(1), track: TrackId(1),
            pos: Ticks::from_seconds(3.0), src_in: Ticks::ZERO, len: Ticks::from_seconds(2.0),
            gain: 1.0, effects: Vec::new() } };
        s.doc.local(edit).unwrap();
        s.presence(Presence { site: SiteId(2), name: "ada".into(),
            playhead: Ticks::from_seconds(3.5), selection: vec![ClipId(7)],
            active_track: Some(TrackId(1)), seq: 1 }, 1100);
        assert_eq!(s.doc.peers.len(), 1);

        let before = s.doc.project.clone();
        assert_eq!(s.leave(SiteId(2), 2000), Some(Event::Left(SiteId(2))));
        assert!(s.roster().is_empty());
        assert!(s.doc.peers.is_empty(), "presence is gone");
        assert_eq!(s.doc.project, before, "and the project is byte-identical");
        assert!(s.doc.project.clips.contains_key(&ClipId(7)), "her edit survives her leaving");
        // leaving twice is not an event
        assert!(s.leave(SiteId(2), 2100).is_none());
    }

    #[test]
    fn a_peer_that_crashes_is_swept_because_it_never_says_goodbye() {
        // The other half of membership: a session that only removes polite
        // departures fills up with ghosts.
        let mut s = seeded_session();
        s.join(SiteId(2), "sc-9f0c22ab", "ada", 0);
        s.join(SiteId(3), "sc-44de7710", "bo", 0);
        // bo keeps talking; ada's laptop shuts
        s.touch(SiteId(3), LIVENESS_MS);
        let events = s.sweep(LIVENESS_MS + 1);
        assert_eq!(events, vec![Event::TimedOut(SiteId(2))]);
        assert!(s.is_present(SiteId(3)), "the peer that was still talking stays");
        assert!(!s.is_present(SiteId(2)));
        // and a sweep with nobody stale is silent
        assert!(s.sweep(LIVENESS_MS + 2).is_empty());
    }

    #[test]
    fn presence_counts_as_a_sign_of_life() {
        // Separating "merge presence" from "note we heard from them" is how a
        // peer who is actively scrubbing gets swept for silence.
        let mut s = seeded_session();
        s.join(SiteId(2), "sc-9f0c22ab", "ada", 0);
        for t in [10_000i64, 20_000, 30_000, 40_000, 50_000, 60_000] {
            s.presence(Presence { site: SiteId(2), name: "ada".into(),
                playhead: Ticks::from_seconds(t as f64 / 1000.0), selection: vec![],
                active_track: None, seq: t as u64 }, t);
            assert!(s.sweep(t).is_empty(), "she is plainly still here at {t}ms");
        }
        assert!(s.is_present(SiteId(2)));
    }

    #[test]
    fn coming_back_is_a_rejoin_not_a_new_arrival() {
        // Worth distinguishing because the UI should say "ada is back" rather
        // than announcing a stranger.
        let mut s = seeded_session();
        s.join(SiteId(2), "sc-9f0c22ab", "ada", 0);
        s.leave(SiteId(2), 1000);
        assert_eq!(s.join(SiteId(2), "sc-9f0c22ab", "ada", 2000), Some(Event::Rejoined(SiteId(2))));
        assert_eq!(s.roster().len(), 1, "and not duplicated");
        // the same after a sweep
        s.sweep(2000 + LIVENESS_MS + 1);
        assert_eq!(s.join(SiteId(2), "sc-9f0c22ab", "ada", 99_999), Some(Event::Rejoined(SiteId(2))));
    }

    #[test]
    fn an_empty_session_is_idle_which_is_what_makes_cleanup_decidable() {
        // The question membership exists to answer: can this project be
        // archived, or is someone still working on it?
        let mut s = seeded_session();
        assert!(s.is_idle(), "nobody has joined yet");
        s.join(SiteId(2), "sc-9f0c22ab", "ada", 0);
        assert!(!s.is_idle());
        s.leave(SiteId(2), 100);
        assert!(s.is_idle(), "everyone has gone home");
        // but the work is still there to archive
        assert!(!s.doc.project.tracks.is_empty());
    }

    #[test]
    fn stale_members_are_flagged_before_they_are_swept() {
        // "connection may be poor" beats a collaborator silently vanishing.
        let mut s = seeded_session();
        s.join(SiteId(2), "sc-9f0c22ab", "ada", 0);
        assert!(s.stale(5_000, 10_000).is_empty());
        assert_eq!(s.stale(15_000, 10_000), vec![SiteId(2)]);
        assert!(s.is_present(SiteId(2)), "flagged, not removed");
    }

    #[test]
    fn broadcasting_hands_a_newcomer_the_whole_project() {
        let mut s = seeded_session();
        s.doc.local(Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1),
            track: TrackId(1), pos: Ticks::ZERO, src_in: Ticks::ZERO,
            len: Ticks::from_seconds(4.0), gain: 1.0, effects: Vec::new() } }).unwrap();
        let mut newcomer = Doc::new(SiteId(5), Project { name: "cut".into(), ..Default::default() });
        newcomer.receive(s.broadcast());
        assert_eq!(newcomer.project, s.doc.project, "the broadcast reconstructs the session");
    }
}
