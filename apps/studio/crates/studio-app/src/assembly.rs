//! assembly — the app's side of `studio-assemble`: a first cut from the
//! transcripts, laid on a track of its own, with the review list as markers.
//!
//! The crate decides; this module only gathers its inputs from the project
//! and turns its answer into ops:
//!
//! * **Takes**: every file source with a transcript (generators -- patterns --
//!   say nothing), named by file stem.
//! * **Angles**: a project group whose video clips overlap in time is a
//!   camera group -- the same fact the multiview relies on -- so its sources
//!   are one performance and a tie between them is not a choice of take.
//! * **Outline**: `outline.txt`, else `script.txt`, beside the project file.
//!   None: the takes in name order, the last saying of a repeated line kept.
//! * **Noise**: `studio_video::spoken_slate::hallucination`.
//!
//! The ops put the cut on a NEW track at the top (never on top of anyone's
//! clips), starting at `at`, one undo group; every review is a marker named
//! `review N: ...` at the line it is about.

use std::collections::BTreeMap;
use std::path::Path;

use studio_assemble::{Assembly, Options, Take};
use studio_core::{Op, Project, SourceId, Ticks, Track, TrackId, TrackKind};
use studio_voice::Segment;

/// Sources that are angles of one performance: source -> group id.
pub fn angle_groups(project: &Project) -> BTreeMap<SourceId, u64> {
    let mut out = BTreeMap::new();
    for (gid, g) in &project.groups {
        let clips: Vec<&studio_core::Clip> = g.clips.iter().filter_map(|id| project.clips.get(id))
            .filter(|c| project.sources.get(&c.source).is_some_and(|s| s.has_video)).collect();
        if clips.len() < 2 { continue; }
        let overlap_all = clips.iter().all(|a| clips.iter().any(|b| a.id != b.id && a.pos < b.end() && b.pos < a.end()));
        if overlap_all {
            for c in clips { out.insert(c.source, gid.0); }
        }
    }
    out
}

/// The takes: every file source that has a transcript.
pub fn takes(project: &Project, words: &BTreeMap<SourceId, Vec<Segment>>) -> Vec<Take> {
    let angles = angle_groups(project);
    words.iter().filter_map(|(id, segs)| {
        let src = project.sources.get(id)?;
        if src.is_generator() || segs.is_empty() { return None; }
        let name = Path::new(&src.path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| src.path.clone());
        Some(Take { source: *id, name, segments: segs.clone(), len: src.len, angle_group: angles.get(id).copied() })
    }).collect()
}

/// The outline beside the project, if one is there: `(file name, lines)`.
pub fn outline_beside(project_file: &Path) -> Option<(String, Vec<String>)> {
    let dir = project_file.parent()?;
    ["outline.txt", "script.txt"].iter().find_map(|n| {
        let text = std::fs::read_to_string(dir.join(n)).ok()?;
        let lines = studio_assemble::parse_outline(&text);
        (!lines.is_empty()).then(|| (n.to_string(), lines))
    })
}

/// The proposal and the ops that lay it down (a new top track first), or
/// why there is nothing to assemble.
pub fn plan(project: &Project, words: &BTreeMap<SourceId, Vec<Segment>>, outline: &[String], at: Ticks)
    -> Result<(Assembly, Vec<Op>), String>
{
    let takes = takes(project, words);
    if takes.is_empty() {
        return Err("assemble: no transcripts loaded -- load them first (palette: transcripts)".into());
    }
    let noise = |t: &str| studio_video::spoken_slate::hallucination(t).is_some();
    let asm = studio_assemble::assemble(&takes, outline, &Options::default(), &noise, at);
    if asm.placements.is_empty() {
        return Err(format!("assemble: nothing matched ({})", asm.summary()));
    }
    let video = asm.placements.iter().any(|p| project.sources.get(&p.source).is_some_and(|s| s.has_video));
    let track = TrackId(project.tracks.iter().map(|t| t.id.0).max().unwrap_or(0) + 1);
    let next_clip = project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1;
    let names: BTreeMap<SourceId, String> = takes.iter().map(|t| (t.source, t.name.clone())).collect();
    let mut ops = vec![Op::AddTrack { track: Track { id: track, kind: if video { TrackKind::Video } else { TrackKind::Audio },
        name: "assembly".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }];
    ops.extend(asm.ops(track, next_clip, &names));
    Ok((asm, ops))
}

/// Draft a story outline from the footage (`studio_assemble::story`) and
/// write it beside the project: `outline.txt` when there is none, else
/// `outline.draft.txt` -- an outline the editor has shaped is never
/// overwritten. Returns the file written and the draft. Scene labels from
/// slates are not read yet: scenes go in shot order, and the draft says so.
pub fn draft_story(project: &Project, words: &BTreeMap<SourceId, Vec<Segment>>, project_file: &Path)
    -> Result<(std::path::PathBuf, studio_assemble::story::Story), String>
{
    let takes = takes(project, words);
    if takes.is_empty() {
        return Err("story: no transcripts loaded -- load them first (palette: transcripts)".into());
    }
    let noise = |t: &str| studio_video::spoken_slate::hallucination(t).is_some();
    let story = studio_assemble::story::draft(&takes, &BTreeMap::new(), &noise);
    let dir = project_file.parent().ok_or("story: the project has no folder")?;
    let file = if dir.join("outline.txt").exists() { dir.join("outline.draft.txt") } else { dir.join("outline.txt") };
    std::fs::write(&file, story.outline_text()).map_err(|e| format!("story: could not write {}: {e}", file.display()))?;
    Ok((file, story))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cartoon-fragile project, opened as the app opens it, with the
    /// transcripts found beside its media as the app finds them.
    fn fragile() -> (Project, BTreeMap<SourceId, Vec<Segment>>, std::path::PathBuf) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cartoon-fragile");
        let file = dir.join("fragile-cartoon.studio");
        let project = studio_collab::file::SaveFile::decode(&std::fs::read(&file).expect("fixture project"))
            .expect("the fixture project opens").to_project();
        let loaded = crate::transcripts::beside_media(&project, &dir);
        (project, loaded.words, file)
    }

    #[test]
    fn the_fixtures_camera_group_is_one_performance() {
        let (p, words, _) = fragile();
        let a = angle_groups(&p);
        let video: Vec<SourceId> = p.sources.values().filter(|s| s.has_video).map(|s| s.id).collect();
        assert_eq!(video.len(), 3);
        let g: Vec<_> = video.iter().map(|s| a.get(s)).collect();
        assert!(g.iter().all(|x| x.is_some() && *x == g[0]), "all three angles in one group: {g:?}");
        assert_eq!(takes(&p, &words).len(), 3, "the three cameras have transcripts; the recorder has none");
    }

    #[test]
    fn an_outline_beside_the_project_assembles_on_a_new_track_with_one_undo() {
        let (p, words, file) = fragile();
        let tmp = std::env::temp_dir().join(format!("studio-asm-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("script.txt"), "JUDE: You like acting, come on.\nMAX: Partying is great.\nNOBODY: A line nobody says.\n").unwrap();
        let (name, outline) = outline_beside(&tmp.join("x.studio")).expect("script.txt found");
        assert_eq!(name, "script.txt");
        let (asm, ops) = plan(&p, &words, &outline, Ticks::from_seconds(20.0)).expect("a plan");
        assert_eq!(asm.placements.len(), 2, "{}", asm.summary());
        assert!(asm.summary().contains("1 outline line not found"));
        // applied the way the shell applies it: one group, and it lands
        let mut q = p.clone();
        q.apply(&Op::Group { ops: ops.clone() }).expect("the ops apply to the project they were planned on");
        let track = q.tracks.iter().find(|t| t.name == "assembly").expect("a new track");
        assert_eq!(q.tracks[0].id, track.id, "at the top");
        assert_eq!(q.track_clips(track.id).len(), 2);
        assert_eq!(q.track_clips(track.id)[0].pos, Ticks::from_seconds(20.0));
        assert!(q.markers.iter().any(|m| m.name.starts_with("review 1: missing")), "{:?}", q.markers);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_drafted_story_is_written_beside_the_project_never_over_the_editors_outline_and_assembles() {
        let (p, words, _) = fragile();
        let tmp = std::env::temp_dir().join(format!("studio-story-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("x.studio");
        let (written, story) = draft_story(&p, &words, &file).expect("a draft");
        assert_eq!(written, tmp.join("outline.txt"));
        assert_eq!(story.scenes.len(), 1, "three angles of one scene");
        assert_eq!(story.line_count(), 4);
        // the editor's outline is there now: a second draft goes beside it
        let (again, _) = draft_story(&p, &words, &file).unwrap();
        assert_eq!(again, tmp.join("outline.draft.txt"));
        // and the draft is an outline the assembler takes: all four lines, no reviews
        let (_, lines) = outline_beside(&file).unwrap();
        let (asm, _) = plan(&p, &words, &lines, Ticks::ZERO).unwrap();
        assert_eq!(asm.placements.len(), 4);
        assert!(asm.reviews.is_empty(), "{:?}", asm.reviews);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn with_no_transcripts_it_says_so_instead_of_laying_an_empty_track() {
        let (p, _, _) = fragile();
        let err = plan(&p, &BTreeMap::new(), &[], Ticks::ZERO).unwrap_err();
        assert!(err.contains("no transcripts"), "{err}");
    }
}
