//! journal — the undo history as something a person can read.
//!
//! [`History`](crate::History) is a stack of inverse ops: correct, and blind.
//! Pressing undo seven times to get back to "before I moved those clips"
//! means counting edits in your head. The journal keeps, for every change:
//!
//! * a **summary written from what the ops did** ("moved 3 clips on Vox by
//!   +0.10 s; faded 2"), computed against the project as it was BEFORE them,
//!   so it can name tracks and measure a move;
//! * **what it broke and what it fixed**: after each change the project
//!   [`check`]s run and the entry records which findings appeared and which
//!   went away ("broke: a clip on Vox runs past its source");
//! * **restore points**: a small change that leaves the project with no
//!   findings marks a KNOWN-GOOD state; a LARGE change (an import, a batch
//!   edit) gets a snapshot of the project taken before it, so undoing it is
//!   one restore rather than thousands of inverse ops;
//! * [`palette`](Journal::palette) lines for a command palette's Undo list.
//!
//! Headless and deterministic (the caller passes the clock), so the GUI is a
//! shell over it and every behaviour here is tested without one.

use std::collections::{BTreeMap, BTreeSet};

use crate::{EditError, Op, Project, Ticks};

/// When a change counts as LARGE (snapshot before it, never an automatic
/// restore point), in ops.
pub const LARGE_CHANGE: usize = 64;

/// One thing wrong with a project. `key` is stable across checks (so an
/// entry can say it appeared or went away); `text` is for a person.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding { pub key: String, pub text: String }

/// What is wrong with `p` right now -- what the model's own checks cannot
/// refuse (it already refuses overlapping clips, clips past their source,
/// and the like; those are checked here too, as a guard). The one that
/// matters in practice is STEMS OUT OF SYNC: clips on different tracks from
/// sources of the SAME length (the signature of stems of one song) that line
/// up to within 50 ms but not exactly -- almost always one stem nudged by
/// accident, which no ear catches until the groove smears.
pub fn check(p: &Project) -> Vec<Finding> {
    let mut out = Vec::new();
    let tname = |id: crate::TrackId| p.tracks.iter().find(|t| t.id == id).map(|t| t.name.clone()).unwrap_or_else(|| format!("track {}", id.0));
    let mut by_track: BTreeMap<crate::TrackId, Vec<&crate::Clip>> = BTreeMap::new();
    for c in p.clips.values() {
        by_track.entry(c.track).or_default().push(c);
        match p.sources.get(&c.source) {
            None => out.push(Finding { key: format!("missing-source:{}", c.id.0), text: format!("a clip on {} plays a source that is not in the project", tname(c.track)) }),
            Some(s) if !s.is_generator() && s.len > Ticks::ZERO && c.src_in.saturating_add(c.len) > s.len.saturating_add(Ticks::from_seconds(0.001)) =>
                out.push(Finding { key: format!("past-source:{}", c.id.0),
                    text: format!("a clip on {} at {:.2} s runs {:.2} s past the end of its source", tname(c.track), c.pos.to_seconds(),
                        (c.src_in.saturating_add(c.len).to_seconds() - s.len.to_seconds())) }),
            _ => {}
        }
        if !p.tracks.iter().any(|t| t.id == c.track) {
            out.push(Finding { key: format!("missing-track:{}", c.id.0), text: format!("clip {} is on a track that does not exist", c.id.0) });
        }
        if let Some((fi, fo)) = p.fades.get(&c.id) {
            if fi.saturating_add(*fo) > c.len {
                out.push(Finding { key: format!("fades-overlap:{}", c.id.0), text: format!("a clip on {} at {:.2} s has fades longer than itself", tname(c.track), c.pos.to_seconds()) });
            }
        }
    }
    for (t, clips) in &mut by_track {
        clips.sort_by_key(|c| (c.pos, c.id.0));
        for w in clips.windows(2) {
            if w[1].pos < w[0].end() {
                out.push(Finding { key: format!("overlap:{}:{}", w[0].id.0, w[1].id.0),
                    text: format!("two clips overlap on {} at {:.2} s", tname(*t), w[1].pos.to_seconds()) });
            }
            // a butt join that jumps in the audio with no fade on either
            // side clicks; a split (same source, contiguous) does not
            let (a, b) = (w[0], w[1]);
            let gen = |c: &crate::Clip| p.sources.get(&c.source).map_or(true, |s| s.is_generator());
            let touching = b.pos >= a.end() && b.pos.to_seconds() - a.end().to_seconds() < 0.001;
            let split = a.source == b.source && (a.src_in.saturating_add(a.len).to_seconds() - b.src_in.to_seconds()).abs() < 0.0005;
            let fo = p.fades.get(&a.id).map_or(Ticks::ZERO, |f| f.1);
            let fi = p.fades.get(&b.id).map_or(Ticks::ZERO, |f| f.0);
            if touching && !split && !gen(a) && !gen(b) && fo == Ticks::ZERO && fi == Ticks::ZERO {
                out.push(Finding { key: format!("hard-cut:{}:{}", a.id.0, b.id.0),
                    text: format!("an unfaded cut on {} at {:.2} s may click", tname(*t), b.pos.to_seconds()) });
            }
        }
    }
    // stems out of sync
    let clips: Vec<&crate::Clip> = p.clips.values().collect();
    for (i, a) in clips.iter().enumerate() {
        for b in &clips[i + 1..] {
            if a.track == b.track || a.source == b.source { continue; }
            let (sa, sb) = match (p.sources.get(&a.source), p.sources.get(&b.source)) { (Some(x), Some(y)) => (x, y), _ => continue };
            if (sa.len.to_seconds() - sb.len.to_seconds()).abs() > 0.01 || sa.len <= Ticks::ZERO { continue; }
            if a.end() <= b.pos || b.end() <= a.pos { continue; } // not sounding together
            let d = (a.pos.to_seconds() - a.src_in.to_seconds()) - (b.pos.to_seconds() - b.src_in.to_seconds());
            if d.abs() > 0.0005 && d.abs() < 0.05 {
                let (x, y) = if a.id.0 < b.id.0 { (a, b) } else { (b, a) };
                out.push(Finding { key: format!("stem-drift:{}:{}", x.id.0, y.id.0),
                    text: format!("{} and {} are stems out of sync by {:+.1} ms at {:.2} s", tname(a.track), tname(b.track), d * 1000.0, a.pos.max(b.pos).to_seconds()) });
            }
        }
    }
    for t in &p.tracks {
        if let Some(b) = t.route { if !p.busses.contains_key(&b) {
            out.push(Finding { key: format!("route:{}", t.id.0), text: format!("{} is routed to a bus that does not exist", t.name) });
        } }
    }
    for b in p.busses.values() {
        let (mut at, mut seen) = (b.parent, BTreeSet::from([b.id]));
        while let Some(x) = at {
            if !seen.insert(x) { out.push(Finding { key: format!("bus-loop:{}", b.id.0), text: format!("bus {} feeds itself through its parents", b.name) }); break; }
            at = p.busses.get(&x).and_then(|y| y.parent);
        }
    }
    out.sort();
    out
}

/// What a set of ops did, in words, measured against `before`.
pub fn summarize(before: &Project, ops: &[Op]) -> String {
    let mut flat = Vec::new();
    fn walk<'a>(ops: &'a [Op], out: &mut Vec<&'a Op>) { for o in ops { if let Op::Group { ops } = o { walk(ops, out) } else { out.push(o) } } }
    walk(ops, &mut flat);
    let tname = |id: crate::TrackId| before.tracks.iter().find(|t| t.id == id).map(|t| t.name.clone()).unwrap_or_else(|| format!("track {}", id.0));
    let clip_track = |id: crate::ClipId| before.clips.get(&id).map(|c| c.track);
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut tracks: BTreeSet<String> = BTreeSet::new();
    let mut moves: Vec<f64> = Vec::new();
    let mut other = 0usize;
    for o in &flat {
        let (verb, track) = match o {
            Op::AddClip { clip } => ("added", Some(clip.track)),
            Op::RemoveClip { id } => ("removed", clip_track(*id)),
            Op::MoveClip { id, pos, track } => {
                if let Some(c) = before.clips.get(id) { if c.track == *track { moves.push(pos.to_seconds() - c.pos.to_seconds()); } }
                ("moved", Some(*track))
            }
            Op::TrimClip { id, .. } => ("trimmed", clip_track(*id)),
            Op::SetGain { id, .. } => ("gain on", clip_track(*id)),
            Op::SetEffects { id, .. } => ("effects on", clip_track(*id)),
            Op::SetClipFade { id, .. } => ("faded", clip_track(*id)),
            Op::SetTrackGain { id, .. } | Op::SetTrackFlags { id, .. } => ("track settings", Some(*id)),
            Op::SetRoute { track, .. } => ("routing", Some(*track)),
            Op::AddTrack { .. } => ("tracks added", None),
            Op::RemoveTrack { .. } => ("tracks removed", None),
            Op::AddBus { .. } | Op::RemoveBus { .. } | Op::SetBusGain { .. } | Op::SetBusChain { .. } => ("bus changes", None),
            Op::AddSource { .. } | Op::RemoveSource { .. } => ("sources", None),
            Op::SetAutoPoint { .. } | Op::RemoveAutoPoint { .. } => ("automation points", None),
            _ => { other += 1; continue }
        };
        *counts.entry(verb).or_default() += 1;
        if let Some(t) = track { tracks.insert(tname(t)); }
    }
    let clip_verb = |v: &str| matches!(v, "added" | "removed" | "moved" | "trimmed");
    let mut parts: Vec<String> = counts.iter().map(|(v, n)| {
        if clip_verb(v) { format!("{v} {n} clip{}", if *n == 1 { "" } else { "s" }) }
        else if *v == "faded" || v.ends_with(" on") { format!("{v} {n} clip{}", if *n == 1 { "" } else { "s" }) }
        else { format!("{n} {v}") }
    }).collect();
    // a uniform move says by how much
    if !moves.is_empty() && moves.iter().all(|d| (d - moves[0]).abs() < 1e-6) {
        if let Some(p) = parts.iter_mut().find(|p| p.starts_with("moved")) { *p = format!("{p} by {:+.2} s", moves[0]); }
    }
    if other > 0 { parts.push(format!("{other} other")); }
    let mut s = if parts.is_empty() { "no change".to_string() } else { parts.join(", ") };
    if !tracks.is_empty() && tracks.len() <= 3 {
        s = format!("{s} on {}", tracks.into_iter().collect::<Vec<_>>().join(", "));
    } else if tracks.len() > 3 {
        s = format!("{s} on {} tracks", tracks.len());
    }
    s
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// What the caller called it ("nudge", "paste", "import").
    pub label: String,
    /// What it did, from the ops (see [`summarize`]).
    pub summary: String,
    /// Ops, flattened (its size).
    pub size: usize,
    /// When, in the caller's clock (ms).
    pub at_ms: u64,
    /// Findings that appeared with this change, and that went away.
    pub broke: Vec<Finding>,
    pub fixed: Vec<Finding>,
    /// A small change that left the project with NO findings: known good.
    pub restore_point: bool,
    forward: Vec<Op>,
    inverse: Vec<Op>,
}

/// The journal. `entries[..applied]` are in the project; the rest can be redone.
#[derive(Debug, Default)]
pub struct Journal {
    entries: Vec<Entry>,
    applied: usize,
    /// Project states BEFORE entry i, for large entries: undoing one restores.
    snapshots: BTreeMap<usize, Project>,
}

fn flat_len(ops: &[Op]) -> usize { ops.iter().map(|o| if let Op::Group { ops } = o { flat_len(ops) } else { 1 }).sum() }

impl Journal {
    pub fn new() -> Journal { Journal::default() }
    pub fn entries(&self) -> &[Entry] { &self.entries }
    /// How many entries are in the project (the rest are redoable).
    pub fn applied(&self) -> usize { self.applied }

    /// Apply `ops` as ONE change named `label`, all or nothing, and record it.
    pub fn commit(&mut self, project: &mut Project, label: &str, ops: &[Op], now_ms: u64) -> Result<&Entry, EditError> {
        let before_findings = check(project);
        let summary = summarize(project, ops);
        let size = flat_len(ops);
        let snapshot = (size > LARGE_CHANGE).then(|| project.clone());
        let mut inverse = Vec::with_capacity(ops.len());
        for op in ops {
            match project.apply(op) {
                Ok(inv) => inverse.push(inv),
                Err(e) => {
                    for inv in inverse.iter().rev() { let _ = project.apply(inv); }
                    return Err(e);
                }
            }
        }
        inverse.reverse();
        let after = check(project);
        let (b, a): (BTreeSet<&Finding>, BTreeSet<&Finding>) = (before_findings.iter().collect(), after.iter().collect());
        let broke = a.difference(&b).map(|f| (*f).clone()).collect();
        let fixed = b.difference(&a).map(|f| (*f).clone()).collect();
        // a new change drops the redo tail, and any snapshot taken for it
        self.entries.truncate(self.applied);
        self.snapshots.retain(|k, _| *k < self.applied);
        if let Some(s) = snapshot { self.snapshots.insert(self.applied, s); }
        self.entries.push(Entry { label: label.into(), summary, size, at_ms: now_ms, broke, fixed,
            restore_point: size <= LARGE_CHANGE && after.is_empty(), forward: ops.to_vec(), inverse });
        self.applied += 1;
        Ok(&self.entries[self.applied - 1])
    }

    /// Undo the newest applied entry.
    pub fn undo(&mut self, project: &mut Project) -> Option<&Entry> {
        let i = self.applied.checked_sub(1)?;
        match self.snapshots.get(&i) {
            Some(s) => *project = s.clone(),
            None => for inv in &self.entries[i].inverse { project.apply(inv).ok()?; },
        }
        self.applied = i;
        Some(&self.entries[i])
    }

    /// Redo the next entry.
    pub fn redo(&mut self, project: &mut Project) -> Option<&Entry> {
        let i = self.applied;
        let e = self.entries.get_mut(i)?;
        let mut inverse = Vec::new();
        for op in &e.forward { inverse.push(project.apply(op).ok()?); }
        inverse.reverse();
        e.inverse = inverse;
        self.applied = i + 1;
        Some(&self.entries[i])
    }

    /// Go to the state right AFTER entry `index` (undoing or redoing as many
    /// as that takes). Returns how many steps it took.
    pub fn go_to(&mut self, project: &mut Project, index: usize) -> Option<usize> {
        let target = index + 1;
        if target > self.entries.len() { return None; }
        let mut steps = 0;
        while self.applied > target { self.undo(project)?; steps += 1; }
        while self.applied < target { self.redo(project)?; steps += 1; }
        Some(steps)
    }

    /// The last known-good state at or before the current one.
    pub fn last_restore_point(&self) -> Option<usize> {
        (0..self.applied).rev().find(|i| self.entries[*i].restore_point)
    }

    /// Lines for a command palette's Undo list, newest first: what each did,
    /// how long ago, what it broke -- and which one is the last known-good.
    pub fn palette(&self, now_ms: u64) -> Vec<(usize, String)> {
        let good = self.last_restore_point();
        (0..self.applied).rev().map(|i| {
            let e = &self.entries[i];
            let ago = now_ms.saturating_sub(e.at_ms) / 1000;
            let when = if ago < 60 { format!("{ago} s ago") } else if ago < 3600 { format!("{} min ago", ago / 60) } else { format!("{} h ago", ago / 3600) };
            let mut s = format!("undo to before: {} -- {} ({when})", e.label, e.summary);
            if !e.broke.is_empty() { s.push_str(&format!(" | BROKE {}: {}", e.broke.len(), e.broke[0].text)); }
            if !e.fixed.is_empty() { s.push_str(&format!(" | fixed {}", e.fixed.len())); }
            if Some(i) == good { s.push_str(" | last known good"); }
            (i, s)
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, ClipId, Source, SourceId, Track, TrackId, TrackKind};

    fn project() -> Project {
        let mut p = Project::default();
        for (id, name) in [(1u64, "Vox"), (2, "Drums")] {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(id), kind: TrackKind::Audio, name: name.into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        }
        p.apply(&Op::AddSource { source: Source::file(SourceId(1), "a.wav", Ticks::from_seconds(10.0), 48_000, 2, false) }).unwrap();
        for k in 0..4u64 {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(k + 1), source: SourceId(1), track: TrackId(1), pos: Ticks::from_seconds(k as f64 * 2.0),
                src_in: Ticks::ZERO, len: Ticks::from_seconds(1.5), gain: 1.0, effects: vec![] } }).unwrap();
        }
        p
    }
    fn nudge(ids: &[u64], p: &Project, by: f64) -> Vec<Op> {
        ids.iter().map(|i| { let c = &p.clips[&ClipId(*i)]; Op::MoveClip { id: c.id, pos: Ticks::from_seconds(c.pos.to_seconds() + by), track: c.track } }).collect()
    }

    #[test]
    fn a_change_is_summarised_from_what_it_did() {
        let p = project();
        assert_eq!(summarize(&p, &nudge(&[1, 2, 3], &p, 0.1)), "moved 3 clips by +0.10 s on Vox");
        let mixed = vec![Op::TrimClip { id: ClipId(1), src_in: Ticks::ZERO, len: Ticks::from_seconds(1.0) },
                         Op::SetClipFade { id: ClipId(2), fade_in: Ticks::from_seconds(0.1), fade_out: Ticks::ZERO },
                         Op::SetClipFade { id: ClipId(3), fade_in: Ticks::from_seconds(0.1), fade_out: Ticks::ZERO }];
        assert_eq!(summarize(&p, &mixed), "faded 2 clips, trimmed 1 clip on Vox");
        assert_eq!(summarize(&p, &[Op::Group { ops: nudge(&[4], &p, -0.25) }]), "moved 1 clip by -0.25 s on Vox", "groups are looked inside");
    }

    /// Drums: stems of the same song (a source of the same 10 s), clip k+10
    /// exactly under Vox clip k.
    #[test]
    fn an_unfaded_jump_is_a_hard_cut_and_a_split_or_a_fade_is_not() {
        let mut p = project();
        let clip = |id: u64, pos: f64, src_in: f64| Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(1), track: TrackId(1),
            pos: Ticks::from_seconds(pos), src_in: Ticks::from_seconds(src_in), len: Ticks::from_seconds(1.0), gain: 1.0, effects: vec![] } };
        let cuts = |p: &Project| check(p).into_iter().filter(|f| f.key.starts_with("hard-cut")).map(|f| f.key).collect::<Vec<_>>();
        assert!(cuts(&p).is_empty(), "gapped clips are not cuts");
        // clip 4 is 6.0..7.5 playing 0..1.5: continuing at 1.5 is a split
        p.apply(&clip(20, 7.5, 1.5)).unwrap();
        assert!(cuts(&p).is_empty(), "{:?}", cuts(&p));
        // jumping back to 0 at 8.5 is a cut
        p.apply(&clip(21, 8.5, 0.0)).unwrap();
        assert_eq!(cuts(&p), vec!["hard-cut:20:21".to_string()]);
        assert!(check(&p).iter().any(|f| f.text == "an unfaded cut on Vox at 8.50 s may click"));
        // a fade on either side clears it
        p.apply(&Op::SetClipFade { id: ClipId(21), fade_in: Ticks::from_seconds(0.005), fade_out: Ticks::ZERO }).unwrap();
        assert!(cuts(&p).is_empty());
    }
    fn with_stems() -> Project {
        let mut p = project();
        p.apply(&Op::AddSource { source: Source::file(SourceId(2), "drums.wav", Ticks::from_seconds(10.0), 48_000, 2, false) }).unwrap();
        for k in 0..4u64 {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(k + 11), source: SourceId(2), track: TrackId(2), pos: Ticks::from_seconds(k as f64 * 2.0),
                src_in: Ticks::ZERO, len: Ticks::from_seconds(1.5), gain: 1.0, effects: vec![] } }).unwrap();
        }
        p
    }

    #[test]
    fn stems_nudged_out_of_sync_are_found_and_exact_ones_are_not() {
        let mut p = with_stems();
        assert!(check(&p).is_empty(), "aligned stems: nothing to say");
        let o = nudge(&[2], &p, 0.02);
        for op in &o { p.apply(op).unwrap(); }
        let f = check(&p);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].text, "Vox and Drums are stems out of sync by +20.0 ms at 2.02 s");
        // a big, deliberate offset (a whole beat) is not a drift
        let o = nudge(&[2], &p, 0.355);
        for op in &o { p.apply(op).unwrap(); }
        assert!(check(&p).is_empty(), "{:?}", check(&p));
    }

    #[test]
    fn an_entry_says_what_it_broke_and_what_fixed_it() {
        let mut p = with_stems();
        let mut j = Journal::new();
        { let o = nudge(&[3], &p, 0.012); j.commit(&mut p, "nudge", &o, 1_000).unwrap(); }
        assert_eq!(j.entries()[0].broke.len(), 1);
        assert!(j.entries()[0].broke[0].text.contains("out of sync by +12.0 ms"), "{:?}", j.entries()[0].broke);
        assert!(!j.entries()[0].restore_point, "a change that broke something is not known-good");
        { let o = nudge(&[3], &p, -0.012); j.commit(&mut p, "nudge back", &o, 2_000).unwrap(); }
        assert_eq!(j.entries()[1].fixed.len(), 1);
        assert!(j.entries()[1].broke.is_empty() && j.entries()[1].restore_point);
    }

    #[test]
    fn restore_points_mark_known_good_states_and_go_to_jumps_there() {
        let mut p = with_stems();
        let original = p.clone();
        let mut j = Journal::new();
        j.commit(&mut p, "gain", &[Op::SetGain { id: ClipId(1), gain: 0.8 }], 0).unwrap(); // 0: good
        let good = p.clone();
        { let o = nudge(&[2], &p, 0.01); j.commit(&mut p, "nudge", &o, 0).unwrap(); }    // 1: breaks
        j.commit(&mut p, "gain", &[Op::SetGain { id: ClipId(3), gain: 0.5 }], 0).unwrap(); // 2: still broken
        assert_eq!(j.last_restore_point(), Some(0));
        assert_eq!(j.go_to(&mut p, 0), Some(2), "two undos back to the known-good state");
        assert_eq!(p, good);
        assert_eq!(j.go_to(&mut p, 2), Some(2), "and forward again");
        while j.undo(&mut p).is_some() {}
        assert_eq!(p, original, "all the way back");
    }

    #[test]
    fn a_large_change_is_snapshotted_and_undone_in_one_restore() {
        let mut p = project();
        let before = p.clone();
        let mut j = Journal::new();
        let ops: Vec<Op> = (0..100u64).map(|k| Op::AddClip { clip: Clip { id: ClipId(100 + k), source: SourceId(1), track: TrackId(2),
            pos: Ticks::from_seconds(k as f64), src_in: Ticks::ZERO, len: Ticks::from_seconds(0.5), gain: 1.0, effects: vec![] } }).collect();
        let e = j.commit(&mut p, "import", &ops, 0).unwrap();
        assert_eq!(e.size, 100);
        assert!(!e.restore_point, "a large change is never an automatic restore point");
        assert!(j.snapshots.contains_key(&0));
        j.undo(&mut p).unwrap();
        assert_eq!(p, before);
        j.redo(&mut p).unwrap();
        assert_eq!(p.clips.len(), 104);
    }

    #[test]
    fn a_failed_change_leaves_no_trace_and_the_palette_reads_newest_first() {
        let mut p = with_stems();
        let before = p.clone();
        let mut j = Journal::new();
        let bad = vec![Op::SetGain { id: ClipId(1), gain: 0.5 }, Op::RemoveClip { id: ClipId(99) }];
        assert!(j.commit(&mut p, "bad", &bad, 0).is_err());
        assert_eq!(p, before);
        assert!(j.entries().is_empty());
        j.commit(&mut p, "gain", &[Op::SetGain { id: ClipId(1), gain: 0.8 }], 0).unwrap();
        { let o = nudge(&[4], &p, 0.03); j.commit(&mut p, "nudge", &o, 90_000).unwrap(); }
        let pal = j.palette(149_000);
        assert_eq!(pal.len(), 2);
        assert!(pal[0].1.starts_with("undo to before: nudge -- moved 1 clip by +0.03 s on Vox (59 s ago) | BROKE 1: Vox and Drums are stems out of sync by +30.0 ms"), "{}", pal[0].1);
        assert!(j.palette(150_000)[0].1.contains("(1 min ago)"));
        assert!(pal[1].1.ends_with("| last known good"), "{}", pal[1].1);
    }
}
