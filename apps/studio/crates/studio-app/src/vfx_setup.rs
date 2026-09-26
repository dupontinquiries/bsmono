//! vfx_setup — the touch-up setup pass over the cut: every video clip on an
//! unmuted track surveyed (`studio_video::vfx_survey`), the shot list written
//! beside the project, and a marker on the timeline where each problem is.
//!
//! The shot list goes to `vfx.md`, or `vfx.draft.md` when a `vfx.md` is
//! already there -- statuses a person has set are never overwritten. Markers
//! are `vfx: ...`, at the problem's TIMELINE time (a shake at 7.5 s in the
//! source of a clip trimmed from 2 s and placed at 10 s is marked at 15.5 s),
//! and replace the last survey's.
//!
//! Synchronous: at survey size a shot measures in about a second here (the
//! fixture's three 14 s shots: ~2 s). A whole film wants the touch-up job's
//! worker; that is the next step, not this one.

use std::path::{Path, PathBuf};

use studio_core::{Op, Project, Ticks, TrackKind};
use studio_video::vfx_survey::{self, Finding, Problem};

pub const PREFIX: &str = "vfx: ";

pub struct Survey {
    /// `(shot name, source in, length, findings)`, timeline order.
    pub shots: Vec<(String, f64, f64, Vec<Finding>)>,
    /// Clips that could not be measured, with why.
    pub failed: Vec<String>,
    pub ops: Vec<Op>,
}

impl Survey {
    pub fn problems(&self) -> usize { self.shots.iter().map(|s| s.3.len()).sum() }
}

pub fn survey(project: &Project, base: &Path) -> Survey {
    let o = vfx_survey::Options::default();
    let mut clips: Vec<&studio_core::Clip> = project.clips.values().filter(|c| {
        project.tracks.iter().any(|t| t.id == c.track && t.kind == TrackKind::Video && !t.muted)
            && project.sources.get(&c.source).is_some_and(|s| s.has_video)
    }).collect();
    clips.sort_by_key(|c| c.pos);
    let mut s = Survey { shots: Vec::new(), failed: Vec::new(),
        ops: project.markers.iter().filter(|m| m.name.starts_with(PREFIX)).map(|m| Op::RemoveMarker { name: m.name.clone() }).collect() };
    let mut n = 0;
    for c in clips {
        let src = &project.sources[&c.source];
        let name = Path::new(&src.path).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| src.path.clone());
        let (from, len) = (c.src_in.to_seconds(), c.len.to_seconds());
        match vfx_survey::measure(&base.join(&src.path), from, len, src.len.to_seconds()) {
            Err(e) => s.failed.push(format!("{name}: {e}")),
            Ok(st) => {
                let fs = vfx_survey::triage(&st, &o);
                for f in &fs {
                    let src_at = match f.problem { Problem::Shake { from: a, .. } => a, _ => from };
                    let at = c.pos.saturating_add(Ticks::from_seconds((src_at - from).max(0.0)));
                    n += 1;
                    let what = match f.problem { Problem::Shake { .. } => "shake", Problem::Flicker { .. } => "flicker",
                                                 Problem::ShortHandles { .. } => "short handles" };
                    s.ops.push(Op::SetMarker { at, name: format!("{PREFIX}{n} {name}: {what} -> {}", f.technique) });
                }
                s.shots.push((name, from, len, fs));
            }
        }
    }
    s
}

/// Write the shot list beside the project; returns the file.
pub fn write_list(survey: &Survey, project_file: &Path) -> Result<PathBuf, String> {
    let dir = project_file.parent().ok_or("vfx: the project has no folder")?;
    let file = if dir.join("vfx.md").exists() { dir.join("vfx.draft.md") } else { dir.join("vfx.md") };
    std::fs::write(&file, vfx_survey::shot_list(&survey.shots)).map_err(|e| format!("vfx: could not write {}: {e}", file.display()))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixture_cut_is_surveyed_and_the_wides_shake_is_marked_at_timeline_time() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() { eprintln!("skip: no ffmpeg"); return; }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cartoon-fragile");
        let p = studio_collab::file::SaveFile::decode(&std::fs::read(dir.join("fragile-cartoon.studio")).unwrap())
            .expect("the fixture project opens").to_project();
        let s = survey(&p, &dir);
        assert!(s.failed.is_empty(), "{:?}", s.failed);
        assert_eq!(s.shots.len(), 3, "three angles; the recorder has no picture");
        let wide = p.clips.values().find(|c| p.sources[&c.source].path.contains("A001")).unwrap();
        let marks: Vec<(Ticks, &str)> = s.ops.iter().filter_map(|o| match o { Op::SetMarker { at, name } => Some((*at, name.as_str())), _ => None })
            .filter(|(_, n)| n.contains("shake")).collect();
        assert_eq!(marks.len(), 1, "{marks:?}");
        assert!(marks[0].1.contains("A001_wide"), "{marks:?}");
        let expect = wide.pos.to_seconds() + 7.5 - wide.src_in.to_seconds();
        assert!((marks[0].0.to_seconds() - expect).abs() < 0.4, "marked at {:.2}, the shake is at {expect:.2} on the timeline", marks[0].0.to_seconds());
        // written beside the project, never over a person's list
        let tmp = std::env::temp_dir().join(format!("studio-vfx-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert_eq!(write_list(&s, &tmp.join("x.studio")).unwrap(), tmp.join("vfx.md"));
        assert_eq!(write_list(&s, &tmp.join("x.studio")).unwrap(), tmp.join("vfx.draft.md"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
