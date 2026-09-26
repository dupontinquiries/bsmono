//! story_check — the timeline held against its documents and its footage.
//!
//! Gathers `studio_assemble::constraints` from beside the project --
//! `outline.txt` (hard, per scene block), `constraints.txt` (hard, explicit),
//! every `*.lrc` (hard, lyric lines pinned to song time, ±0.5 s) -- and from
//! the loaded transcripts (soft, the cheap ruling-out), then checks every
//! clip on the timeline against them. Each clip is known by its source's
//! file stem, the names of the groups (scenes, comps) it belongs to, and the
//! transcript lines inside its trim.
//!
//! The answer lands as markers, replacing the last check's: `constraint:
//! violated ...` at the clip that breaks it, `constraint: conflict ...` at 0
//! when documents disagree. Unresolved constraints are counted, not marked --
//! a line not yet placed is not a fault.

use std::collections::BTreeMap;
use std::path::Path;

use studio_assemble::constraints::{self, Constraint, Kind, Placed, Status};
use studio_core::{Op, Project, SourceId, Ticks};
use studio_voice::Segment;

pub const PREFIX: &str = "constraint: ";

/// Every constraint from the documents beside `project_file`, plus what the
/// transcripts infer; and every document line that could not be read.
pub fn gather(project: &Project, words: &BTreeMap<SourceId, Vec<Segment>>, project_file: &Path)
    -> (Vec<Constraint>, Vec<String>)
{
    let (mut cs, mut unread) = (Vec::new(), Vec::new());
    if let Some(dir) = project_file.parent() {
        if let Ok(t) = std::fs::read_to_string(dir.join("outline.txt")) { cs.extend(constraints::from_outline(&t, "outline.txt", false)); }
        if let Ok(t) = std::fs::read_to_string(dir.join("constraints.txt")) {
            let (c, bad) = constraints::parse(&t, "constraints.txt");
            cs.extend(c);
            unread.extend(bad.into_iter().map(|(n, l)| format!("constraints.txt:{n}: {l}")));
        }
        let mut lrcs: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lrc"))).collect();
        lrcs.sort();
        for p in lrcs {
            if let Ok(t) = std::fs::read_to_string(&p) {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                cs.extend(constraints::from_lrc(&t, &name, Ticks::from_seconds(0.5)));
            }
        }
    }
    let noise = |t: &str| studio_video::spoken_slate::hallucination(t).is_some();
    cs.extend(constraints::infer(&crate::assembly::takes(project, words), &noise));
    (cs, unread)
}

/// Every clip as the checker sees it.
pub fn placed(project: &Project, words: &BTreeMap<SourceId, Vec<Segment>>) -> Vec<(studio_core::ClipId, Placed)> {
    let mut v: Vec<_> = project.clips.values().map(|c| {
        let mut names: Vec<String> = project.sources.get(&c.source).map(|s| Path::new(&s.path).file_stem()
            .map(|x| x.to_string_lossy().into_owned()).unwrap_or_default()).into_iter().collect();
        names.extend(project.groups.values().filter(|g| g.clips.contains(&c.id)).map(|g| g.name.clone()));
        let end_src = c.src_in.saturating_add(c.len);
        let lines = words.get(&c.source).map(|segs| segs.iter()
            .filter(|s| s.start >= c.src_in && s.end <= end_src).map(|s| s.text.clone()).collect()).unwrap_or_default();
        (c.id, Placed { names, lines, at: c.pos, end: c.end() })
    }).collect();
    v.sort_by_key(|(_, p)| p.at);
    v
}

/// The report, and the ops that replace the last check's markers with this one's.
pub fn check(project: &Project, cs: &[Constraint], words: &BTreeMap<SourceId, Vec<Segment>>)
    -> (constraints::Report, Vec<Op>)
{
    let pl = placed(project, words);
    let only: Vec<Placed> = pl.iter().map(|(_, p)| p.clone()).collect();
    let report = constraints::check(cs, &only);
    let mut ops: Vec<Op> = project.markers.iter().filter(|m| m.name.starts_with(PREFIX))
        .map(|m| Op::RemoveMarker { name: m.name.clone() }).collect();
    let mut n = 0;
    for (c, st) in &report.statuses {
        let Status::Violated(by) = st else { continue };
        // mark it where it shows: the later anchor of a precedence, else the anchor
        let a = match &c.kind { Kind::Before(_, b) | Kind::Follows(_, b) => Some(b), Kind::At(a, _, _) => Some(a), Kind::Total(_) => None };
        let at = a.and_then(|a| only.iter().find(|p| anchor_hits(a, p)).map(|p| p.at))
            .unwrap_or_else(|| only.iter().map(|p| p.end).max().unwrap_or(Ticks::ZERO));
        n += 1;
        ops.push(Op::SetMarker { at, name: format!("{PREFIX}{n} violated by {by:.1} s: {} ({})", c.describe(), c.source) });
    }
    for (k, cyc) in report.conflicts.iter().enumerate() {
        let srcs: Vec<&str> = cyc.iter().map(|c| c.source.as_str()).collect();
        ops.push(Op::SetMarker { at: Ticks::ZERO, name: format!("{PREFIX}conflict {}: {}", k + 1, srcs.join(" vs "))});
    }
    (report, ops)
}

fn anchor_hits(a: &constraints::Anchor, p: &Placed) -> bool {
    use constraints::Anchor;
    match a {
        Anchor::Line(t) => p.lines.iter().any(|l| studio_assemble::similarity(l, t) >= 0.75),
        Anchor::Clip(c) | Anchor::Scene(c) => p.names.iter().any(|n| n.eq_ignore_ascii_case(c)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fragile() -> (Project, BTreeMap<SourceId, Vec<Segment>>) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cartoon-fragile");
        let p = studio_collab::file::SaveFile::decode(&std::fs::read(dir.join("fragile-cartoon.studio")).unwrap())
            .expect("the fixture project opens").to_project();
        let words = crate::transcripts::beside_media(&p, &dir).words;
        (p, words)
    }

    /// Assemble the fixture from a script, then write documents that
    /// disagree with the cut in known ways, and check.
    #[test]
    fn an_assembled_cut_is_checked_against_its_documents_and_the_markers_say_where() {
        let (p, words) = fragile();
        let tmp = std::env::temp_dir().join(format!("studio-check-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("x.studio");
        // the cut: script order, but "Partying" BEFORE "You like acting"
        let (asm, ops) = crate::assembly::plan(&p, &words, &[
            "Yeah, I found something I like more than that.".into(), "Partying is great.".into(),
            "You like acting, come on.".into(), "Hi guys, mind if we join the party?".into()], Ticks::ZERO).unwrap();
        let mut q = p.clone();
        q.apply(&Op::Group { ops }).unwrap();
        // the documents: the outline says acting then partying; a hard total
        std::fs::write(tmp.join("outline.txt"), "# ── N7\nYou like acting, come on.\nPartying is great.\n").unwrap();
        std::fs::write(tmp.join("constraints.txt"), format!("total <= {:.1}\nnonsense line\n", asm.end().to_seconds() + 20.0)).unwrap();
        let (cs, unread) = gather(&q, &words, &file);
        assert_eq!(unread, vec!["constraints.txt:2: nonsense line".to_string()]);
        assert!(cs.iter().any(|c| c.source == "outline.txt:2-3"));
        let (report, ops) = check(&q, &cs, &words);
        let violated: Vec<String> = report.violated().map(|(c, _)| c.source.clone()).collect();
        assert!(violated.contains(&"outline.txt:2-3".to_string()), "the swapped pair: {violated:?} / {}", report.summary());
        // the footage (3 angles, all in script order) rules the same swap out, softly
        assert!(report.violated().any(|(c, _)| c.source.starts_with("transcripts")), "{}", report.summary());
        let marks: Vec<&Op> = ops.iter().filter(|o| matches!(o, Op::SetMarker { .. })).collect();
        assert_eq!(marks.len(), report.violated().count());
        // re-checking replaces the markers instead of piling them up
        let mut r = q.clone();
        r.apply(&Op::Group { ops: ops.clone() }).unwrap();
        let before = r.markers.len();
        let (_, again) = check(&r, &cs, &words);
        r.apply(&Op::Group { ops: again }).unwrap();
        assert_eq!(r.markers.len(), before);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
