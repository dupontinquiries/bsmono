//! av1_encode — the step between "we recorded frames" and a playable file.
//!
//! `video_record::Recorder` counts frames and can already produce a
//! `snapclone_av1policy::Plan`, and the camera page says so out loud:
//! *"would record 2.4s — 72 frames at 30 fps, no encoder linked yet"*. The
//! policy decides HOW to encode; nothing turned that decision into a
//! command. This is that, and only that.
//!
//! Pure on purpose: it builds an argv, it does not spawn one. A command
//! that is a `Vec<String>` can be asserted on in a test and printed in the
//! journal before it runs, which for a step that consumes a recording the
//! user cannot get back is worth more than the convenience of spawning
//! here.
//!
//! # Why SVT-AV1 through ffmpeg rather than a Rust encoder
//!
//! `av1policy`'s own header says its `Plan` is written to be translated
//! into "SVT-AV1 parameters", and its fields (`hierarchical_levels`,
//! `temporal_filtering`) are SVT's vocabulary, not a generic encoder's.
//! Going through `ffmpeg -c:v libsvtav1` keeps that mapping one-to-one.
//! rav1e would need every field reinterpreted, and the mapping is exactly
//! where the policy's meaning would quietly get lost.

use snapclone_av1policy::{Plan, RateControl};

/// Where the frames come from. Raw frames on stdin is what `Recorder`
/// holds; a file is what a re-encode of an existing clip uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Raw frames piped to the encoder's stdin.
    RawStdin { width: u32, height: u32, fps: u32, pix_fmt: String },
    File(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    /// A recording with no frames, or a zero dimension. Encoding it would
    /// produce a file that plays as nothing.
    Empty(String),
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodeError::Empty(s) => write!(f, "nothing to encode: {s}"),
        }
    }
}

/// The full command, argv-style, `ffmpeg` first.
///
/// `-y` is deliberately ABSENT: overwriting is the caller's decision, and a
/// silent clobber of a previous snap is not a default worth having.
pub fn command(src: &Source, plan: &Plan, out_path: &str) -> Result<Vec<String>, EncodeError> {
    let mut a: Vec<String> = vec!["ffmpeg".into()];

    match src {
        Source::RawStdin { width, height, fps, pix_fmt } => {
            if *width == 0 || *height == 0 {
                return Err(EncodeError::Empty(format!("frame size {width}x{height}")));
            }
            if *fps == 0 {
                return Err(EncodeError::Empty("zero fps".into()));
            }
            a.extend(["-f", "rawvideo", "-pix_fmt"].iter().map(|s| s.to_string()));
            a.push(pix_fmt.clone());
            a.push("-s".into());
            a.push(format!("{width}x{height}"));
            a.push("-r".into());
            a.push(fps.to_string());
            a.push("-i".into());
            a.push("-".into()); // stdin
        }
        Source::File(p) => {
            if p.trim().is_empty() {
                return Err(EncodeError::Empty("no input path".into()));
            }
            a.push("-i".into());
            a.push(p.clone());
        }
    }

    a.push("-c:v".into());
    a.push("libsvtav1".into());

    // Rate control, straight from the policy.
    match plan.rate_control {
        RateControl::ConstantQuality { crf: q } => {
            a.push("-crf".into());
            a.push(q.to_string());
        }
        RateControl::TargetBitrate { kbps } => {
            a.push("-b:v".into());
            a.push(format!("{kbps}k"));
        }
    }

    a.push("-g".into());
    a.push(plan.keyint.to_string());

    // SVT's own knobs go through -svtav1-params, which is a single
    // colon-separated string; splitting them into separate flags is what
    // ffmpeg does NOT accept.
    let mut svt = vec![
        format!("hierarchical-levels={}", plan.hierarchical_levels),
        format!("enable-tf={}", u8::from(plan.temporal_filtering)),
    ];
    if plan.seekable {
        // A seekable clip needs closed GOPs, or a seek lands before a frame
        // that references something the decoder has not seen.
        svt.push("irefresh-type=2".into());
    }
    a.push("-svtav1-params".into());
    a.push(svt.join(":"));

    a.push(out_path.to_string());
    Ok(a)
}

/// The command as a copyable line, for the journal. Arguments containing a
/// space are quoted so the line can actually be pasted.
pub fn display(argv: &[String]) -> String {
    argv.iter()
        .map(|s| if s.contains(' ') { format!("'{s}'") } else { s.clone() })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapclone_av1policy::{plan_for, Tier};

    fn find(a: &[String], flag: &str) -> Option<String> {
        a.iter().position(|x| x == flag).and_then(|i| a.get(i + 1)).cloned()
    }

    #[test]
    fn raw_frames_are_described_to_the_encoder_before_the_input() {
        let src = Source::RawStdin { width: 720, height: 1280, fps: 30, pix_fmt: "yuv420p".into() };
        let plan = plan_for(Tier::Standard, false, 30);
        let a = command(&src, &plan, "/tmp/out.mkv").unwrap();
        assert_eq!(a[0], "ffmpeg");
        assert_eq!(find(&a, "-s").as_deref(), Some("720x1280"));
        assert_eq!(find(&a, "-r").as_deref(), Some("30"));
        assert_eq!(find(&a, "-i").as_deref(), Some("-"), "frames arrive on stdin");
        assert_eq!(find(&a, "-c:v").as_deref(), Some("libsvtav1"));
        assert_eq!(a.last().unwrap(), "/tmp/out.mkv");
        // Every raw-video flag must precede -i, or ffmpeg applies them to the output.
        let i_at = a.iter().position(|x| x == "-i").unwrap();
        for flag in ["-f", "-pix_fmt", "-s", "-r"] {
            assert!(a.iter().position(|x| x == flag).unwrap() < i_at, "{flag} after -i");
        }
        assert!(!a.iter().any(|x| x == "-y"), "never clobbers without being told to");
    }

    #[test]
    fn the_policys_rate_control_reaches_the_command_either_way() {
        let src = Source::File("in.mkv".into());
        let mut plan = plan_for(Tier::Standard, false, 30);
        plan.rate_control = RateControl::ConstantQuality { crf: 34 };
        let a = command(&src, &plan, "o.mkv").unwrap();
        assert_eq!(find(&a, "-crf").as_deref(), Some("34"));
        assert!(!a.iter().any(|x| x == "-b:v"));
        plan.rate_control = RateControl::TargetBitrate { kbps: 1800 };
        let a = command(&src, &plan, "o.mkv").unwrap();
        assert_eq!(find(&a, "-b:v").as_deref(), Some("1800k"));
        assert!(!a.iter().any(|x| x == "-crf"));
    }

    #[test]
    fn svt_specific_knobs_go_in_one_colon_separated_parameter() {
        let src = Source::File("in.mkv".into());
        let mut plan = plan_for(Tier::Standard, true, 30);
        plan.hierarchical_levels = 4;
        plan.temporal_filtering = false;
        plan.seekable = true;
        let a = command(&src, &plan, "o.mkv").unwrap();
        let p = find(&a, "-svtav1-params").expect("params present");
        assert!(p.contains("hierarchical-levels=4"), "{p}");
        assert!(p.contains("enable-tf=0"), "{p}");
        assert!(p.contains("irefresh-type=2"), "seekable means closed GOPs: {p}");
        // One argument, not several: ffmpeg rejects them split apart. The
        // params VALUE legitimately starts with the first knob, so what is
        // asserted is that exactly one argument mentions it, and that it is
        // the one following -svtav1-params.
        assert_eq!(a.iter().filter(|x| x.contains("hierarchical-levels")).count(), 1);
        assert_eq!(find(&a, "-svtav1-params").as_deref(), Some(p.as_str()));
        assert_eq!(find(&a, "-g"), Some(plan.keyint.to_string()));

        let mut unseekable = plan.clone();
        unseekable.seekable = false;
        let b = command(&src, &unseekable, "o.mkv").unwrap();
        assert!(!find(&b, "-svtav1-params").unwrap().contains("irefresh-type"));
    }

    #[test]
    fn a_recording_with_nothing_in_it_is_refused_rather_than_encoded() {
        let plan = plan_for(Tier::Standard, false, 30);
        let zero = Source::RawStdin { width: 0, height: 720, fps: 30, pix_fmt: "yuv420p".into() };
        assert!(matches!(command(&zero, &plan, "o.mkv"), Err(EncodeError::Empty(_))));
        let nofps = Source::RawStdin { width: 720, height: 720, fps: 0, pix_fmt: "yuv420p".into() };
        assert!(matches!(command(&nofps, &plan, "o.mkv"), Err(EncodeError::Empty(_))));
        assert!(matches!(command(&Source::File("  ".into()), &plan, "o.mkv"), Err(EncodeError::Empty(_))));
    }

    #[test]
    fn the_command_prints_as_something_you_could_paste() {
        let src = Source::File("my clip.mkv".into());
        let plan = plan_for(Tier::Standard, false, 30);
        let line = display(&command(&src, &plan, "out.mkv").unwrap());
        assert!(line.starts_with("ffmpeg "));
        assert!(line.contains("'my clip.mkv'"), "a path with a space is quoted: {line}");
    }
}
