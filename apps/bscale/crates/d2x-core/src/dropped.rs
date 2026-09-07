//! dropped — what a file dragged onto the window means, and what to ask
//! before acting on it.
//!
//! A drop is the one input gesture with no undo. It replaces the input path
//! and the previous one is gone from the UI — there is no text left to
//! retype from and no history to walk back. So the drop does not take
//! effect on release; it produces a [`Pending`] the app confirms.
//!
//! That is also why the classification is worth getting right rather than
//! "if it exists, use it". A drop of the wrong thing that is ACCEPTED and
//! then fails four minutes into a job is much worse than one refused at the
//! moment of dropping, when the person still has the file under the
//! pointer and knows what they meant.
//!
//! Pure: paths and a directory predicate in, a decision out. The caller
//! supplies whether a path is a directory (a test supplies a closure), so
//! this needs no filesystem.

use std::path::{Path, PathBuf};

/// Containers ffmpeg reads that this pipeline is actually tested against.
/// Deliberately a list rather than "anything ffmpeg might open": a drop
/// that is accepted and then dies in the decoder is the failure mode this
/// module exists to avoid.
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "mpg", "mpeg", "wmv", "flv", "ts", "m2ts",
];

/// What was dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dropped {
    /// A video container this pipeline reads.
    Video(PathBuf),
    /// A directory — the PNG-sequence input.
    Directory(PathBuf),
    /// A single image. Not an input on its own: the pipeline wants the
    /// FOLDER, and saying so is more use than refusing silently.
    LooseImage(PathBuf),
    /// Something else entirely.
    Unsupported { path: PathBuf, ext: String },
    /// More than one thing at once. The input is a single path, so a
    /// multi-drop has no meaning — better said than half-honoured by
    /// taking the first.
    TooMany(usize),
    /// Nothing usable in the drop at all.
    Empty,
}

/// Extensions that are images but not inputs.
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff"];

fn ext_of(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Classify a drop. `is_dir` answers whether a path is a directory.
pub fn classify(paths: &[PathBuf], is_dir: impl Fn(&Path) -> bool) -> Dropped {
    match paths.len() {
        0 => return Dropped::Empty,
        1 => {}
        n => return Dropped::TooMany(n),
    }
    let p = &paths[0];
    if is_dir(p) {
        return Dropped::Directory(p.clone());
    }
    let ext = ext_of(p);
    if VIDEO_EXTS.contains(&ext.as_str()) {
        Dropped::Video(p.clone())
    } else if IMAGE_EXTS.contains(&ext.as_str()) {
        Dropped::LooseImage(p.clone())
    } else {
        Dropped::Unsupported { path: p.clone(), ext }
    }
}

/// A drop waiting on the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub what: Dropped,
    /// The input path this would overwrite, so the prompt can say what is
    /// being lost rather than only what is arriving.
    pub replacing: String,
}

impl Pending {
    /// Can this be accepted at all? An unsupported drop still produces a
    /// prompt — it explains — but with nothing to confirm.
    pub fn is_acceptable(&self) -> bool {
        matches!(self.what, Dropped::Video(_) | Dropped::Directory(_))
    }

    /// The path the input becomes if accepted.
    pub fn accepted_path(&self) -> Option<String> {
        match &self.what {
            Dropped::Video(p) | Dropped::Directory(p) => Some(p.display().to_string()),
            _ => None,
        }
    }

    /// What the dialog says.
    pub fn message(&self) -> String {
        match &self.what {
            Dropped::Video(p) => format!("Use {} as the input?", name(p)),
            Dropped::Directory(p) => {
                format!("Use the folder {} as a PNG sequence?", name(p))
            }
            Dropped::LooseImage(p) => format!(
                "{} is a single image. Drop the FOLDER that contains the frames instead.",
                name(p)
            ),
            Dropped::Unsupported { path, ext } if ext.is_empty() => {
                format!("{} has no extension — this reads video files or a folder of PNGs.", name(path))
            }
            Dropped::Unsupported { path, ext } => format!(
                "{} is a .{ext} — this reads video files or a folder of PNGs.",
                name(path)
            ),
            Dropped::TooMany(n) => {
                format!("{n} items dropped. The input is one path — drop a single file or folder.")
            }
            Dropped::Empty => "Nothing usable in that drop.".to_string(),
        }
    }

    /// The line naming what would be lost. `None` when the input was empty,
    /// because "replacing nothing" is noise.
    pub fn replacing_note(&self) -> Option<String> {
        let cur = self.replacing.trim();
        (self.is_acceptable() && !cur.is_empty())
            .then(|| format!("Replaces {cur}"))
    }
}

fn name(p: &Path) -> String {
    p.file_name().and_then(|n| n.to_str()).unwrap_or("that").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pb(s: &str) -> PathBuf { PathBuf::from(s) }
    fn no_dirs(_: &Path) -> bool { false }

    #[test]
    fn a_video_and_a_folder_are_inputs_and_a_loose_png_is_not() {
        assert_eq!(classify(&[pb("/a/clip.mkv")], no_dirs), Dropped::Video(pb("/a/clip.mkv")));
        assert_eq!(classify(&[pb("/a/CLIP.MP4")], no_dirs), Dropped::Video(pb("/a/CLIP.MP4")),
                   "extensions are matched case-insensitively");
        assert_eq!(classify(&[pb("/a/frames")], |_| true), Dropped::Directory(pb("/a/frames")));
        // A single PNG is the near-miss worth naming: the pipeline wants the
        // folder, and refusing without saying so sends people in circles.
        assert_eq!(classify(&[pb("/a/f001.png")], no_dirs), Dropped::LooseImage(pb("/a/f001.png")));
    }

    #[test]
    fn a_multi_drop_is_refused_rather_than_half_honoured() {
        // Taking the first of five is the tempting behaviour and it is wrong:
        // the person cannot tell which one was used.
        let many = vec![pb("/a/one.mkv"), pb("/a/two.mkv"), pb("/a/three.mkv")];
        assert_eq!(classify(&many, no_dirs), Dropped::TooMany(3));
        assert_eq!(classify(&[], no_dirs), Dropped::Empty);
    }

    #[test]
    fn only_a_video_or_a_folder_can_be_accepted() {
        let ok = Pending { what: classify(&[pb("/a/clip.mkv")], no_dirs), replacing: "old.mp4".into() };
        assert!(ok.is_acceptable());
        assert_eq!(ok.accepted_path().as_deref(), Some("/a/clip.mkv"));
        for bad in [
            classify(&[pb("/a/f.png")], no_dirs),
            classify(&[pb("/a/notes.txt")], no_dirs),
            classify(&[pb("/a/x"), pb("/a/y")], no_dirs),
            classify(&[], no_dirs),
        ] {
            let p = Pending { what: bad, replacing: "old.mp4".into() };
            assert!(!p.is_acceptable(), "{:?}", p.what);
            assert!(p.accepted_path().is_none());
            assert!(!p.message().is_empty(), "a refusal must still explain");
        }
    }

    #[test]
    fn the_prompt_says_what_is_being_lost_and_only_when_there_is_something() {
        let with = Pending { what: classify(&[pb("/a/new.mkv")], no_dirs), replacing: "/old/clip.mp4".into() };
        assert_eq!(with.replacing_note().as_deref(), Some("Replaces /old/clip.mp4"));
        // An empty input is not "replacing nothing" — it is just a new input.
        let empty = Pending { what: classify(&[pb("/a/new.mkv")], no_dirs), replacing: "  ".into() };
        assert!(empty.replacing_note().is_none());
        // A refusal never claims to be replacing anything.
        let refused = Pending { what: classify(&[pb("/a/f.png")], no_dirs), replacing: "/old/clip.mp4".into() };
        assert!(refused.replacing_note().is_none());
    }

    #[test]
    fn every_message_names_the_file_and_an_extensionless_drop_does_not_say_dot_nothing() {
        let img = Pending { what: classify(&[pb("/a/f001.png")], no_dirs), replacing: String::new() };
        assert!(img.message().contains("f001.png") && img.message().contains("FOLDER"), "{}", img.message());
        let none = Pending { what: classify(&[pb("/a/README")], no_dirs), replacing: String::new() };
        assert!(none.message().contains("no extension"), "{}", none.message());
        assert!(!none.message().contains(".  "), "{}", none.message());
        let dir = Pending { what: classify(&[pb("/a/frames")], |_| true), replacing: String::new() };
        assert!(dir.message().contains("frames") && dir.message().contains("PNG"), "{}", dir.message());
    }
}
