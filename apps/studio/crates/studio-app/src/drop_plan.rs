//! drop_plan — what a dropped file should become.
//!
//! The drop handler in `lib.rs` has one branch that matters: a directory is a
//! kit, and **everything else becomes a rack channel**. So dropping a
//! four-minute video on the timeline made a sampler hit out of it, and
//! dropping a melody loop where you wanted it on a track put it on a pad
//! instead. `import_clip` and `import_video_clip` both exist and place media
//! on the timeline; nothing routed to them.
//!
//! This is the routing decision, separated out so it can be tested without a
//! window, a filesystem, or a drop.
//!
//! ## Where the pointer is, not what the file is
//!
//! The temptation is to decide by file type — short samples to the rack, long
//! files to the timeline. That is a heuristic dressed as a rule, and it gets
//! the interesting cases backwards: a one-shot vocal ad-lib is short and
//! belongs on the timeline, a two-bar drum loop is long and belongs on a pad.
//!
//! The user already said where they wanted it, by dropping it there. So
//! [`DropZone`] decides, and duration is used only to *warn*, never to
//! override — see [`Plan::caution`].

use std::path::{Path, PathBuf};

/// Where the pointer was released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    /// The rack / pad grid: a sampler channel.
    Rack,
    /// The timeline, on a specific track at a specific tick.
    Timeline { track: usize, at_ticks: i64 },
    /// Anywhere else — the window at large. Keeps today's behaviour, which is
    /// the rack, because a drop with no target is a drop into the instrument.
    Unspecified,
}

/// What kind of media this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    Audio,
    Video,
    /// A folder.
    Kit,
    /// Recognised as neither, and NOT guessed at. A `.txt` dropped on a DAW
    /// is a mistake, and importing it as silent audio hides the mistake
    /// instead of reporting it.
    Unsupported,
}

/// What to do with the drop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Every audio file in the folder becomes a pad. `import_kit`.
    ImportKit(PathBuf),
    /// One sampler channel. `import_clip` then a rack slot.
    RackChannel(PathBuf),
    /// A clip on the timeline. `import_clip` / `import_video_clip`.
    TimelineClip { path: PathBuf, video: bool, track: usize, at_ticks: i64 },
    /// Nothing, with a reason to show.
    Refuse { path: PathBuf, why: String },
}

/// A plan, plus anything worth saying about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub intent: Intent,
    /// A non-blocking note. The drop still happens.
    pub caution: Option<String>,
}

/// Long enough that turning it into a single pad hit is probably not what was
/// meant. Only ever produces a caution.
pub const LONG_FOR_A_PAD_SECS: f32 = 30.0;

/// Classify by extension.
///
/// Extension rather than content sniffing because this runs on the UI thread
/// during a drop, and a drop that stalls while it reads headers off a network
/// volume is worse than one that occasionally guesses wrong — the decode
/// immediately afterwards will catch a real mismatch anyway.
pub fn media_of(path: &Path, is_dir: bool, is_video: impl Fn(&Path) -> bool) -> Media {
    if is_dir {
        return Media::Kit;
    }
    if is_video(path) {
        return Media::Video;
    }
    const AUDIO: [&str; 9] = ["wav", "flac", "ogg", "opus", "mp3", "aiff", "aif", "m4a", "caf"];
    match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase) {
        Some(e) if AUDIO.contains(&e.as_str()) => Media::Audio,
        _ => Media::Unsupported,
    }
}

/// Decide what a drop becomes.
///
/// `secs` is the media's duration when already known (from a library entry,
/// say); `None` before the decode, which is the normal case for a fresh drop.
pub fn plan(path: &Path, media: Media, zone: DropZone, secs: Option<f32>) -> Plan {
    let p = path.to_path_buf();
    match media {
        Media::Unsupported => Plan {
            intent: Intent::Refuse {
                path: p,
                why: format!(
                    "{} is not audio or video this build can read",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("that file")
                ),
            },
            caution: None,
        },
        Media::Kit => match zone {
            // A folder is a kit wherever it lands. There is no sensible
            // "folder on a timeline track" — and silently importing only the
            // first file would be worse than ignoring the zone.
            _ => Plan { intent: Intent::ImportKit(p), caution: None },
        },
        Media::Video => match zone {
            DropZone::Timeline { track, at_ticks } => Plan {
                intent: Intent::TimelineClip { path: p, video: true, track, at_ticks },
                caution: None,
            },
            // A VIDEO ON A PAD IS ITS AUDIO. I had this redirecting to the
            // timeline, reasoning that a sampler channel has nowhere to put
            // the picture and the drop would silently discard it.
            //
            // The live drop handler already disagreed, and its reason is
            // better: "to the rack, a video is its audio (refusing a video's
            // audio to a sampler would be the tool overruling the musician —
            // library.rs's own words)". Someone dropping an mp4 on a pad wants
            // the sound in it. Sending them to the timeline instead is the
            // tool deciding it knows what they meant.
            //
            // The picture is not silently discarded, it is not wanted; and
            // Unspecified follows the same rule for the same reason, since a
            // drop with no target is a drop into the instrument.
            DropZone::Rack | DropZone::Unspecified => Plan {
                intent: Intent::RackChannel(p),
                caution: Some("Using the video's audio. Drop it on a track for the picture.".into()),
            },
        },
        Media::Audio => match zone {
            DropZone::Timeline { track, at_ticks } => Plan {
                intent: Intent::TimelineClip { path: p, video: false, track, at_ticks },
                caution: None,
            },
            DropZone::Rack | DropZone::Unspecified => {
                let caution = secs
                    .filter(|s| *s > LONG_FOR_A_PAD_SECS)
                    .map(|s| format!("{s:.0}s on a pad — drop it on a track for a clip."));
                Plan { intent: Intent::RackChannel(p), caution }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vid(p: &Path) -> bool {
        matches!(p.extension().and_then(|e| e.to_str()), Some("mp4") | Some("mov"))
    }
    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// THE BUG. Everything that was not a folder became a rack channel, so a
    /// melody loop dropped on a track landed on a pad.
    #[test]
    fn audio_dropped_on_a_track_becomes_a_timeline_clip() {
        let got = plan(&p("/a/loop.wav"), Media::Audio,
                       DropZone::Timeline { track: 2, at_ticks: 960 }, None);
        assert_eq!(got.intent, Intent::TimelineClip {
            path: p("/a/loop.wav"), video: false, track: 2, at_ticks: 960 });
        assert!(got.caution.is_none());
    }

    /// ...and a drop on the rack still makes a pad, which was right all along.
    #[test]
    fn audio_dropped_on_the_rack_still_becomes_a_channel() {
        let got = plan(&p("/a/kick.wav"), Media::Audio, DropZone::Rack, Some(0.4));
        assert_eq!(got.intent, Intent::RackChannel(p("/a/kick.wav")));
        assert!(got.caution.is_none());
    }

    /// A drop with no target keeps today's behaviour rather than inventing a
    /// new one.
    #[test]
    fn an_untargeted_drop_behaves_as_it_always_did() {
        let got = plan(&p("/a/kick.wav"), Media::Audio, DropZone::Unspecified, None);
        assert_eq!(got.intent, Intent::RackChannel(p("/a/kick.wav")));
    }

    /// THE ZONE DECIDES, NOT THE DURATION. A long file on a pad is a caution,
    /// never an override — a two-bar loop is long and belongs on a pad.
    #[test]
    fn a_long_file_on_a_pad_warns_but_is_still_a_pad() {
        let got = plan(&p("/a/song.wav"), Media::Audio, DropZone::Rack, Some(180.0));
        assert_eq!(got.intent, Intent::RackChannel(p("/a/song.wav")),
            "the user said pad, so it is a pad");
        assert!(got.caution.as_deref().unwrap().contains("track"), "{:?}", got.caution);
    }

    /// A VIDEO ON A PAD IS ITS AUDIO, and the caution says where the picture
    /// went rather than moving the drop.
    ///
    /// This test asserted the opposite -- that the drop was redirected to the
    /// timeline -- until the live drop handler was read: "to the rack, a video
    /// is its audio (refusing a video's audio to a sampler would be the tool
    /// overruling the musician)". Someone dropping an mp4 on a pad wants the
    /// sound in it.
    #[test]
    fn video_on_the_rack_becomes_a_channel_from_its_audio() {
        let got = plan(&p("/a/take.mp4"), Media::Video, DropZone::Rack, None);
        match got.intent {
            Intent::RackChannel(_) => {}
            other => panic!("a video on a pad is its audio: {other:?}"),
        }
        assert!(got.caution.as_deref().unwrap().contains("picture"),
                "say where the picture went: {:?}", got.caution);
    }

    /// Unspecified follows Rack for the same reason -- a drop with no target
    /// is a drop into the instrument, which the zone's own doc already says.
    #[test]
    fn video_with_no_target_follows_the_rack_rule() {
        let got = plan(&p("/a/take.mp4"), Media::Video, DropZone::Unspecified, None);
        assert!(matches!(got.intent, Intent::RackChannel(_)), "{:?}", got.intent);
    }

    #[test]
    fn video_on_a_track_lands_where_it_was_dropped() {
        let got = plan(&p("/a/take.mov"), Media::Video,
                       DropZone::Timeline { track: 1, at_ticks: 4800 }, None);
        assert_eq!(got.intent, Intent::TimelineClip {
            path: p("/a/take.mov"), video: true, track: 1, at_ticks: 4800 });
        assert!(got.caution.is_none(), "dropped where it belongs, so nothing to say");
    }

    /// A folder is a kit wherever it lands — importing only its first file
    /// would be worse than ignoring the zone.
    #[test]
    fn a_folder_is_a_kit_in_every_zone() {
        for z in [DropZone::Rack, DropZone::Unspecified,
                  DropZone::Timeline { track: 0, at_ticks: 0 }] {
            assert_eq!(plan(&p("/kit"), Media::Kit, z, None).intent,
                       Intent::ImportKit(p("/kit")));
        }
    }

    /// An unreadable file is REFUSED with a reason. Importing it as silent
    /// audio would hide the mistake instead of reporting it.
    #[test]
    fn an_unsupported_file_is_refused_by_name() {
        let got = plan(&p("/a/notes.txt"), Media::Unsupported, DropZone::Rack, None);
        match got.intent {
            Intent::Refuse { why, .. } => assert!(why.contains("notes.txt"), "{why}"),
            other => panic!("must refuse: {other:?}"),
        }
    }

    #[test]
    fn extensions_classify_case_insensitively() {
        assert_eq!(media_of(&p("/a/x.WAV"), false, vid), Media::Audio);
        assert_eq!(media_of(&p("/a/x.flac"), false, vid), Media::Audio);
        assert_eq!(media_of(&p("/a/x.mp4"), false, vid), Media::Video);
        assert_eq!(media_of(&p("/a/x.txt"), false, vid), Media::Unsupported);
        assert_eq!(media_of(&p("/a/x"), false, vid), Media::Unsupported,
            "no extension is unsupported, not audio");
        assert_eq!(media_of(&p("/dir"), true, vid), Media::Kit,
            "a directory wins over any extension it happens to have");
    }

    /// Duration is unknown at drop time, which must not itself produce a
    /// warning — an unknown length is not a long one.
    #[test]
    fn an_unknown_duration_produces_no_caution() {
        assert!(plan(&p("/a/x.wav"), Media::Audio, DropZone::Rack, None).caution.is_none());
    }
}
