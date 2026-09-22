//! fragile_demo — a card of footage, organised, as a Studio project.
//!
//!     cargo run --release -p studio-app --example fragile_demo -- \
//!         --clips DIR --transcripts DIR --sound DIR --out demo.studio
//!
//! Demonstrates the whole ingest path on "A Fragile Thing", in the order a
//! person would actually do it:
//!
//! 1. **The cliplist.** Every video file on the card becomes a source.
//! 2. **What was said.** Each clip's whisper transcript is read for the
//!    SPOKEN slate (`studio_video::spoken_slate`): scene, shot, take. A
//!    transcript that is whisper talking to itself is flagged, not read.
//! 3. **Which sound is whose.** Every clip is matched against every recorder
//!    file by listening (`studio_video::dualsync`), and only confident pairs
//!    are used. Unpaired takes are not guessed at — they are listed.
//! 4. **Order.** Clips are grouped into beats (the film's own structure),
//!    and within a beat sorted by scene/shot/take, which is the order an
//!    editor reads a bin in.
//! 5. **The project.** One video track per beat with its takes laid end to
//!    end, the paired sound under each take at its measured offset, a
//!    take-bucket composition per setup with the last take active, and
//!    markers where each beat starts and wherever something needs a human.
//!
//! The report it prints alongside is the point as much as the project: it
//! says what was read, what was paired, and what a person still has to look
//! at.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use studio_core::{
    Clip, ClipId, GroupId, Op, Project, Source, SourceId, SourceKind, Ticks, Track, TrackId, TrackKind,
};
use studio_video::dualsync::{self, Onsets, Verdict};
use studio_video::spoken_slate;

/// Which camera files belong to which beat of the film, by clip number.
/// Checked against picture and transcript by a person — the one part of
/// this that is not derived, and the reason it is a table here rather than
/// a guess in code.
const BEATS: &[(&str, &str, &str)] = &[
    ("P2", "the play on stage", "C0015-C0020"),
    ("N3", "extra girlfriend scene", "C0021-C0023"),
    ("P2b", "the play, single on Max", "C0024-C0024"),
    ("N8", "death call", "C0008-C0009"),
    ("N8b", "death call, mother's side", "C0025-C0027"),
    ("N3b", "cafe romance", "C0028-C0029"),
    ("N7", "rooftop party", "C0030-C0036"),
    ("N2", "cafe exterior", "C0037-C0037"),
    ("S2", "theatre jog", "C0038-C0041"),
    ("S2b", "teens outside the theatre", "C0042-C0048"),
    ("S1", "nun scene", "C0049-C0069"),
    ("S2c", "casting poster inserts", "C0070-C0074"),
    ("S4", "church steps", "C0075-C0075"),
    ("S5", "audition", "C0010-C0014"),
    ("S6", "apartment, Jude visits", "C0000-C0007"),
];

/// The film's order, as the canon document has it — not the card's order.
const ORDER: &[&str] = &["P2", "S1", "S2", "S2b", "S2c", "S4", "S5", "N2", "N3", "N3b",
                         "S6", "N7", "N8", "N8b", "P2b"];

fn clip_number(name: &str) -> Option<String> {
    let i = name.rfind('C')?;
    let n: String = name[i..].chars().take(5).collect();
    (n.len() == 5 && n[1..].chars().all(|c| c.is_ascii_digit())).then_some(n)
}

fn beat_of(name: &str) -> Option<(&'static str, &'static str)> {
    let n = clip_number(name)?;
    BEATS.iter().find(|(_, _, range)| {
        let (a, b) = range.split_once('-').unwrap();
        n.as_str() >= a && n.as_str() <= b
    }).map(|(id, what, _)| (*id, *what))
}

struct Args {
    clips: PathBuf,
    transcripts: PathBuf,
    sound: PathBuf,
    out: PathBuf,
}

fn args() -> Args {
    let mut a = Args { clips: "".into(), transcripts: "".into(), sound: "".into(), out: "fragile.studio".into() };
    let mut it = std::env::args().skip(1);
    while let Some(f) = it.next() {
        let v: PathBuf = it.next().unwrap_or_default().into();
        match f.as_str() {
            "--clips" => a.clips = v,
            "--transcripts" => a.transcripts = v,
            "--sound" => a.sound = v,
            "--out" => a.out = v,
            _ => {}
        }
    }
    if a.clips.as_os_str().is_empty() || a.sound.as_os_str().is_empty() {
        eprintln!("usage: fragile_demo --clips DIR --transcripts DIR --sound DIR [--out FILE]");
        std::process::exit(2);
    }
    a
}

fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok())
        .map(|e| e.path()).filter(|p| p.extension().map(|x| x == ext).unwrap_or(false)).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn stem(p: &Path) -> String { p.file_stem().unwrap_or_default().to_string_lossy().into_owned() }

/// One camera take, as much as is known about it.
struct Take {
    path: PathBuf,
    name: String,
    beat: &'static str,
    what: &'static str,
    board: Option<studio_video::slate_ocr::Board>,
    hallucinated: Option<spoken_slate::Hallucination>,
    len: Ticks,
    sound: Option<(PathBuf, f64)>,  // recorder file and offset (rec = cam + offset)
}

fn main() {
    let a = args();

    // ---- 1. the cliplist -------------------------------------------------
    let clips = files(&a.clips, "mkv");
    if clips.is_empty() { eprintln!("no .mkv in {:?}", a.clips); std::process::exit(1); }
    println!("cliplist: {} camera files in {}", clips.len(), a.clips.display());

    // ---- 2. what was said ------------------------------------------------
    let mut takes: Vec<Take> = Vec::new();
    for p in &clips {
        let name = stem(p);
        let text = std::fs::read_to_string(a.transcripts.join(format!("{name}.txt"))).unwrap_or_default();
        let (beat, what) = beat_of(&name).unwrap_or(("?", "not in the beat table"));
        let len = studio_video::probe(p).map(|x| x.duration).unwrap_or(Ticks::from_seconds(10.0));
        takes.push(Take {
            path: p.clone(), name, beat, what,
            board: spoken_slate::read(&text),
            hallucinated: spoken_slate::hallucination(&text),
            len, sound: None,
        });
    }
    let read = takes.iter().filter(|t| t.board.is_some()).count();
    println!("slates:   {read}/{} read from what the crew said", takes.len());
    let boards: Vec<Option<_>> = takes.iter().map(|t| t.board.clone()).collect();
    let suspects = spoken_slate::suspicious(&boards);
    for s in &suspects {
        println!("          ? {} says scene {}, its neighbours say {}", takes[s.index].name, s.heard, s.likely);
    }

    // ---- 3. which sound is whose ----------------------------------------
    let sound = files(&a.sound, "wav");
    println!("sound:    {} recorder files — matching by listening", sound.len());
    let rec: Vec<(PathBuf, Onsets)> = sound.iter().filter_map(|p| {
        studio_media::decode_file(p).ok().and_then(|x| Onsets::of(&x)).map(|o| (p.clone(), o))
    }).collect();
    let cam: Vec<Option<Onsets>> = takes.iter().map(|t| {
        studio_video::extract_audio(&t.path).ok().flatten().and_then(|x| Onsets::of(&x))
    }).collect();
    let pool: Vec<Onsets> = rec.iter().map(|r| r.1.clone()).collect();
    let live: Vec<usize> = cam.iter().enumerate().filter(|(_, o)| o.is_some()).map(|(i, _)| i).collect();
    let live_onsets: Vec<Onsets> = live.iter().map(|&i| cam[i].clone().unwrap()).collect();
    for (&i, m) in live.iter().zip(dualsync::match_pool(&live_onsets, &pool)) {
        if let Some(m) = m {
            if m.verdict == Verdict::Sure {
                takes[i].sound = Some((rec[m.sound].0.clone(), m.offset_seconds()));
            }
        }
    }
    let paired = takes.iter().filter(|t| t.sound.is_some()).count();
    println!("          {paired}/{} takes paired with confidence", takes.len());

    // ---- 4. order --------------------------------------------------------
    // Within a beat: by what the slate says (scene, then setup, then take),
    // unread slates last in card order — an editor reads a bin that way.
    takes.sort_by(|x, y| {
        let rank = |t: &Take| ORDER.iter().position(|b| *b == t.beat).unwrap_or(usize::MAX);
        let key = |t: &Take| t.board.as_ref().map(|b| (
            b.scene.clone().unwrap_or_default(), b.shot.clone().unwrap_or_default(), b.take.unwrap_or(0)));
        rank(x).cmp(&rank(y))
            .then(key(x).is_none().cmp(&key(y).is_none()))
            .then(key(x).cmp(&key(y)))
            .then(x.name.cmp(&y.name))
    });

    // ---- 5. the project --------------------------------------------------
    let mut p = Project { name: "A Fragile Thing — organised card".into(), ..Default::default() };
    let mut next_source = 1u64;
    let mut next_clip = 1u64;
    let mut next_track = 1u64;
    let mut next_group = 1u64;
    let mut at = Ticks::ZERO;
    let mut buckets: BTreeMap<String, Vec<ClipId>> = BTreeMap::new();
    let mut report = String::new();

    for beat in ORDER {
        let group: Vec<&Take> = takes.iter().filter(|t| t.beat == *beat).collect();
        if group.is_empty() { continue; }
        let what = group[0].what;
        let (vt, at_ix) = (TrackId(next_track), p.tracks.len());
        next_track += 1;
        p.apply(&Op::AddTrack { track: Track { id: vt, kind: TrackKind::Video,
            name: format!("{beat} — {what}"), muted: false, locked: false, gain: 1.0, route: None },
            at: at_ix }).unwrap();
        let st = TrackId(next_track);
        next_track += 1;
        p.apply(&Op::AddTrack { track: Track { id: st, kind: TrackKind::Audio,
            name: format!("{beat} — sound"), muted: false, locked: false, gain: 1.0, route: None },
            at: at_ix + 1 }).unwrap();
        p.apply(&Op::SetMarker { at, name: format!("{beat}: {what}") }).unwrap();
        report.push_str(&format!("\n## {beat} — {what}\n\n"));

        for t in group {
            let sid = SourceId(next_source);
            next_source += 1;
            p.apply(&Op::AddSource { source: Source { id: sid, path: t.path.display().to_string(),
                len: t.len, sample_rate: 48_000, channels: 2, has_video: true, kind: SourceKind::File } }).unwrap();
            let cid = ClipId(next_clip);
            next_clip += 1;
            p.apply(&Op::AddClip { clip: Clip { id: cid, source: sid, track: vt, pos: at,
                src_in: Ticks::ZERO, len: t.len, gain: 1.0, effects: vec![] } }).unwrap();
            // The take bucket: every take of one setup, so the editor picks
            // between them instead of hunting for them.
            if let Some(b) = &t.board {
                let setup = format!("{beat} {}{}", b.scene.clone().unwrap_or_default(),
                                    b.shot.clone().unwrap_or_default());
                buckets.entry(setup).or_default().push(cid);
            }
            // Its sound, under it, at the measured offset: the recorder was
            // rolling first, so the sound file starts BEFORE the picture.
            if let Some((sp, off)) = &t.sound {
                let len = studio_media::decode_file(sp).map(|x|
                    Ticks::from_seconds(x.frames() as f64 / x.sample_rate as f64)).unwrap_or(t.len);
                let asid = SourceId(next_source);
                next_source += 1;
                p.apply(&Op::AddSource { source: Source { id: asid, path: sp.display().to_string(),
                    len, sample_rate: 48_000, channels: 1, has_video: false, kind: SourceKind::File } }).unwrap();
                // picture t=0 is recorder t=(-offset)
                let into = Ticks::from_seconds((-off).max(0.0));
                let acid = ClipId(next_clip);
                next_clip += 1;
                let alen = (len.saturating_sub(into)).min(t.len);
                if alen > Ticks::ZERO {
                    p.apply(&Op::AddClip { clip: Clip { id: acid, source: asid, track: st, pos: at,
                        src_in: into, len: alen, gain: 1.0, effects: vec![] } }).unwrap();
                }
            }
            let slate = t.board.as_ref().map(|b| b.label()).unwrap_or_else(|| "slate unread".into());
            let sound = match &t.sound {
                Some((sp, off)) => format!("{} ({off:+.2}s)", stem(sp)),
                None => "not paired — needs an ear".into(),
            };
            report.push_str(&format!("- `{}` — slate {slate} — sound {sound}{}\n", t.name, slate_notes(t)));
            if t.hallucinated.is_some() || t.board.is_none() {
                p.apply(&Op::SetMarker { at, name: format!("check {}: {}", t.name,
                    if t.board.is_none() { "no slate read" } else { "transcript is whisper talking to itself" }) }).unwrap();
            }
            at = at.saturating_add(t.len);
        }
    }

    // Take buckets, last take active — the convention an editor expects.
    for (name, clips) in &buckets {
        if clips.len() < 2 { continue; }
        let g = GroupId(next_group);
        next_group += 1;
        p.apply(&Op::SetGroup { id: g, name: name.clone(), clips: clips.clone() }).unwrap();
        p.apply(&Op::SetGroupActive { id: g, active: clips.last().copied() }).unwrap();
    }
    for s in &suspects {
        // A slate that disagrees with its neighbours is a question for a
        // person, so it lands where they will see it: on the timeline.
        p.apply(&Op::SetMarker { at: Ticks::ZERO,
            name: format!("slate? {} heard {} — neighbours say {}", takes[s.index].name, s.heard, s.likely) }).unwrap();
    }

    let file = studio_collab::file::SaveFile::from_project(&p, Vec::new(), None);
    std::fs::write(&a.out, file.encode()).expect("write project");
    let md = a.out.with_extension("md");
    std::fs::write(&md, format!(
        "# A Fragile Thing — organised card\n\n\
         {} takes, {} with a slate read from what the crew said, {} paired with sound by listening.\n\
         {} take buckets; {} slate(s) disagree with their neighbours and want a person.\n\n\
         ## How to read this\n\n\
         Each beat below is one video track in the project, with its takes laid end to end in\n\
         slate order (scene, then setup, then take), and its paired sound on the audio track\n\
         under it at the measured offset. A negative offset means the recorder was rolling\n\
         before the camera, which is the normal case.\n\n\
         Takes with no pairing are not guessed at: they are mostly wides and inserts with no\n\
         dialogue, where there is nothing to match and nothing is the right answer. Every take\n\
         whose slate could not be read, or whose transcript turned out to be the recogniser\n\
         talking to itself, has a marker on the timeline at its position.\n\n\
         Takes of one setup are a composition — a take bucket — with the last take active, so\n\
         an editor picks between them instead of hunting for them.\n\n\
         _Generated by `studio-app --example fragile_demo`. The beat table is the one\n\
         human-checked input; everything else here is derived from the media._\n{report}",
        takes.len(), read, paired, buckets.values().filter(|v| v.len() > 1).count(), suspects.len()))
        .expect("write report");

    println!("\nproject:  {} ({} tracks, {} clips, {} markers)",
             a.out.display(), p.tracks.len(), p.clips.len(), p.markers.len());
    println!("report:   {}", md.display());
}

fn slate_notes(t: &Take) -> String {
    match t.hallucinated {
        Some(spoken_slate::Hallucination::Loop) => " — transcript loops (whisper, not speech)".into(),
        Some(spoken_slate::Hallucination::StockPhrase) => " — transcript is whisper's silence filler".into(),
        None => String::new(),
    }
}
