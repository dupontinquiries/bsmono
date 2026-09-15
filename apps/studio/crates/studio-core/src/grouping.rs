//! grouping — making the "clips can be in unlimited overlapping
//! compositions" model VISIBLE before deciding anything about limiting or
//! tiering it. `moving_set` (studio-app) already computes "what moves with
//! this clip" as a union over every group it's in; this module is the
//! read-only, whole-project version of that same question, phrased as a
//! report a person can read rather than a `Vec<ClipId>` a drag handler
//! consumes.
//!
//! The core idea: two compositions that share a clip are coupled — moving
//! a clip in one can move clips in the other, invisibly, unless something
//! says so out loud. [`Project::group_tangles`] finds every such coupled
//! cluster (connected components over "shares a clip with"); a project
//! where every composition is its own tangle of one has no hidden
//! coupling at all — that's the number to watch as a project grows.

use crate::{ClipId, GroupId, Project};
use std::collections::BTreeMap;

/// One connected cluster of compositions, joined by shared clip membership.
/// A tangle of one group is the uncoupled, unsurprising case; a tangle of
/// two or more is exactly the "drag this, watch something else move too"
/// situation.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupTangle {
    pub groups: Vec<GroupId>,
    /// Union of every member clip across the whole tangle, dangling ids
    /// (a deleted clip still listed in some group — see `groups`' own doc
    /// on `Project`) excluded, since a dangling id can't actually move.
    pub clips: Vec<ClipId>,
    /// The clips that ARE the coupling: present in 2+ groups of this
    /// tangle. Empty for a tangle of one — there's nothing to be coupled
    /// to.
    pub shared_clips: Vec<ClipId>,
}

impl Project {
    /// Every group clustered by shared-clip connectivity. Groups with zero
    /// live members are skipped entirely (nothing to move, nothing to
    /// couple). Order is deterministic (sorted by each tangle's lowest
    /// `GroupId`) so a report/test doesn't depend on `BTreeMap` iteration
    /// happening to agree with itself between runs.
    pub fn group_tangles(&self) -> Vec<GroupTangle> {
        // Union-find over GroupId, joined whenever two groups share a
        // live clip.
        let mut parent: BTreeMap<GroupId, GroupId> = self.groups.keys().map(|g| (*g, *g)).collect();
        fn find(parent: &mut BTreeMap<GroupId, GroupId>, x: GroupId) -> GroupId {
            let p = parent[&x];
            if p == x { return x; }
            let root = find(parent, p);
            parent.insert(x, root);
            root
        }
        fn union(parent: &mut BTreeMap<GroupId, GroupId>, a: GroupId, b: GroupId) {
            let (ra, rb) = (find(parent, a), find(parent, b));
            if ra != rb { parent.insert(ra, rb); }
        }

        // Clip -> every live group it's in; any clip with 2+ entries joins
        // those groups' components.
        let mut clip_groups: BTreeMap<ClipId, Vec<GroupId>> = BTreeMap::new();
        for (gid, g) in &self.groups {
            for c in &g.clips {
                if self.clips.contains_key(c) {
                    clip_groups.entry(*c).or_default().push(*gid);
                }
            }
        }
        for gids in clip_groups.values() {
            for w in gids.windows(2) {
                union(&mut parent, w[0], w[1]);
            }
            // windows(2) only links consecutive pairs; union-find's path
            // compression still merges the whole set correctly as long as
            // every element gets unioned with SOME other element in the
            // same set, which consecutive pairs does (a chain, not a stars
            // topology, but union-find doesn't care which spanning
            // structure joins them).
        }

        let mut clusters: BTreeMap<GroupId, Vec<GroupId>> = BTreeMap::new();
        for gid in self.groups.keys() {
            let root = find(&mut parent, *gid);
            clusters.entry(root).or_default().push(*gid);
        }

        let mut tangles: Vec<GroupTangle> = clusters.into_values().map(|mut groups| {
            groups.sort();
            let mut clip_count: BTreeMap<ClipId, usize> = BTreeMap::new();
            for gid in &groups {
                for c in &self.groups[gid].clips {
                    if self.clips.contains_key(c) {
                        *clip_count.entry(*c).or_default() += 1;
                    }
                }
            }
            let clips: Vec<ClipId> = clip_count.keys().copied().collect();
            let shared_clips: Vec<ClipId> = clip_count.iter().filter(|(_, &n)| n >= 2).map(|(c, _)| *c).collect();
            GroupTangle { groups, clips, shared_clips }
        }).collect();
        tangles.sort_by_key(|t| t.groups.first().copied());
        tangles
    }

    /// A person-readable version of [`Project::group_tangles`] -- the
    /// "cause and effect" text this can show in a debug palette entry
    /// before there's a richer visual for it. One line per tangle.
    pub fn grouping_report(&self) -> String {
        let tangles = self.group_tangles();
        if tangles.is_empty() {
            return "no compositions in this project".into();
        }
        let mut lines = Vec::new();
        for t in &tangles {
            let names: Vec<&str> = t.groups.iter().map(|g| self.groups[g].name.as_str()).collect();
            if t.groups.len() == 1 {
                lines.push(format!("'{}': {} clip(s), no overlap with any other composition", names[0], t.clips.len()));
            } else {
                let shared_desc = if t.shared_clips.is_empty() {
                    "linked but sharing no live clip right now".to_string() // possible after a delete; see the dangling-member contract
                } else {
                    format!("sharing clip(s) {:?}", t.shared_clips.iter().map(|c| c.0).collect::<Vec<_>>())
                };
                lines.push(format!(
                    "{}: {shared_desc} -- moving any member can move all {} clips across this tangle",
                    names.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(" + "),
                    t.clips.len()
                ));
            }
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, Op, Source, SourceId, Ticks, Track, TrackId, TrackKind};

    fn base() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio, name: "t".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "s.wav".into(),
            len: Ticks(1_000_000), sample_rate: 48_000, channels: 1, has_video: false, kind: crate::SourceKind::File } }).unwrap();
        for i in 0..6u64 {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(i + 1), source: SourceId(1), track: TrackId(1),
                pos: Ticks(i as i64 * 1000), src_in: Ticks::ZERO, len: Ticks(500), gain: 1.0, effects: vec![] } }).unwrap();
        }
        p
    }

    #[test]
    fn two_independent_groups_are_two_separate_tangles() {
        let mut p = base();
        p.apply(&Op::SetGroup { id: GroupId(1), name: "a".into(), clips: vec![ClipId(1), ClipId(2)] }).unwrap();
        p.apply(&Op::SetGroup { id: GroupId(2), name: "b".into(), clips: vec![ClipId(3), ClipId(4)] }).unwrap();
        let tangles = p.group_tangles();
        assert_eq!(tangles.len(), 2, "no shared clip -- must not be merged into one tangle");
        assert!(tangles.iter().all(|t| t.shared_clips.is_empty()));
    }

    #[test]
    fn two_groups_sharing_one_clip_form_a_single_tangle() {
        let mut p = base();
        p.apply(&Op::SetGroup { id: GroupId(1), name: "hook".into(), clips: vec![ClipId(1), ClipId(4)] }).unwrap();
        p.apply(&Op::SetGroup { id: GroupId(2), name: "bridge".into(), clips: vec![ClipId(4), ClipId(5)] }).unwrap();
        let tangles = p.group_tangles();
        assert_eq!(tangles.len(), 1, "clip 4 couples them -- one tangle, not two");
        let t = &tangles[0];
        assert_eq!(t.groups, vec![GroupId(1), GroupId(2)]);
        assert_eq!(t.shared_clips, vec![ClipId(4)]);
        let mut clips = t.clips.clone(); clips.sort();
        assert_eq!(clips, vec![ClipId(1), ClipId(4), ClipId(5)]);
    }

    /// The transitive case, genuinely worth checking: A-B share a clip,
    /// B-C share a DIFFERENT clip. A and C share nothing directly, but
    /// must still land in the SAME tangle, because moving something in A
    /// can reach C through B.
    #[test]
    fn transitively_linked_groups_form_one_tangle_even_with_no_direct_overlap() {
        let mut p = base();
        p.apply(&Op::SetGroup { id: GroupId(1), name: "a".into(), clips: vec![ClipId(1), ClipId(2)] }).unwrap();
        p.apply(&Op::SetGroup { id: GroupId(2), name: "b".into(), clips: vec![ClipId(2), ClipId(3)] }).unwrap();
        p.apply(&Op::SetGroup { id: GroupId(3), name: "c".into(), clips: vec![ClipId(3), ClipId(4)] }).unwrap();
        let tangles = p.group_tangles();
        assert_eq!(tangles.len(), 1, "a-b-c chain must merge into one tangle transitively");
        assert_eq!(tangles[0].groups, vec![GroupId(1), GroupId(2), GroupId(3)]);
        assert_eq!(tangles[0].shared_clips, vec![ClipId(2), ClipId(3)]);
    }

    #[test]
    fn a_dangling_deleted_clip_does_not_count_as_a_live_coupling() {
        let mut p = base();
        p.apply(&Op::SetGroup { id: GroupId(1), name: "hook".into(), clips: vec![ClipId(1), ClipId(4)] }).unwrap();
        p.apply(&Op::SetGroup { id: GroupId(2), name: "bridge".into(), clips: vec![ClipId(4), ClipId(5)] }).unwrap();
        p.apply(&Op::RemoveClip { id: ClipId(4) }).unwrap(); // the shared clip itself, deleted
        let tangles = p.group_tangles();
        assert_eq!(tangles.len(), 2, "the coupling clip is gone -- these must no longer be one tangle");
        for t in &tangles { assert!(!t.clips.contains(&ClipId(4)), "a dangling id must not appear as a live clip"); }
    }

    #[test]
    fn report_reads_sensibly_for_a_solo_group_and_a_tangle() {
        let mut p = base();
        p.apply(&Op::SetGroup { id: GroupId(1), name: "solo".into(), clips: vec![ClipId(1), ClipId(2)] }).unwrap();
        p.apply(&Op::SetGroup { id: GroupId(2), name: "hook".into(), clips: vec![ClipId(3), ClipId(4)] }).unwrap();
        p.apply(&Op::SetGroup { id: GroupId(3), name: "bridge".into(), clips: vec![ClipId(4), ClipId(5)] }).unwrap();
        let report = p.grouping_report();
        assert!(report.contains("'solo'") && report.contains("no overlap"));
        assert!(report.contains("'hook' + 'bridge'"));
        assert!(report.contains("3 clips"), "report was: {report}");
    }

    #[test]
    fn an_empty_project_reports_plainly_rather_than_an_empty_string() {
        assert_eq!(base().grouping_report(), "no compositions in this project");
    }
}
