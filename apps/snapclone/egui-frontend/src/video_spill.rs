//! video_spill — the frames themselves, on disk, so an encoder has something to read.
//!
//! `video_record::Recorder` admits frames and hands each to a `sink`; the camera page passed a
//! sink that did nothing. So the recorder counted, `av1_encode` built a command, and there was
//! no input for ffmpeg to read — the gap `TODO_end_to_end.md` names as the blocker for video
//! snaps, with everything downstream of it already built.
//!
//! This is that sink.
//!
//! ## Raw, and why that is the right shape here
//!
//! Frames are written exactly as the recorder admitted them: RGBA8, tightly packed, no header
//! and no per-frame marker. That is precisely what `av1_encode`'s `Source::RawStdin` describes
//! to ffmpeg (`-f rawvideo -pix_fmt <fmt> -s WxH -r fps -i -`), so the file can be poured
//! straight into its stdin with no conversion step to get wrong.
//!
//! It is also large: 720x1280 RGBA is 3.7MB a frame, ~110MB a second at 30fps. That is fine
//! for a SNAP, which is seconds long and encoded immediately, and it is why the byte cap
//! below exists rather than being left to the disk to enforce.
//!
//! ## Failures are recorded, not thrown
//!
//! A sink cannot return an error — the recorder's signature does not have one, deliberately,
//! because a frame arriving from a camera thread is not a place to unwind. So a write failure
//! is remembered and reported at the end. A recording that silently lost its middle would
//! encode into a clip that jumps, and nothing would say why.

use std::io::Write;

/// Frames on their way to an encoder.
pub struct Spill {
    file: Option<std::fs::File>,
    path: std::path::PathBuf,
    bytes: u64,
    frames: u64,
    /// The first write that failed, kept verbatim.
    error: Option<String>,
    max_bytes: u64,
}

/// How much raw video to keep before refusing more.
///
/// 600MB is about five seconds at 720p30 RGBA — comfortably past any snap, and far short of
/// filling a phone. The cap is here rather than left to the disk because running out of space
/// mid-recording takes the rest of the system with it, and a snap is not worth that.
pub const MAX_SPILL_BYTES: u64 = 600 * 1024 * 1024;

impl Spill {
    /// Open a spill file. Errors are held rather than returned so the caller can start
    /// recording and learn about a failure at the end — the alternative is refusing to record
    /// because a directory was missing, which loses the moment the user was trying to catch.
    pub fn create(path: impl Into<std::path::PathBuf>) -> Spill {
        let path = path.into();
        let mut error = None;
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                error = Some(format!("{}: {e}", dir.display()));
            }
        }
        let file = match std::fs::File::create(&path) {
            Ok(f) => Some(f),
            Err(e) => {
                error.get_or_insert(format!("{}: {e}", path.display()));
                None
            }
        };
        Spill { file, path, bytes: 0, frames: 0, error, max_bytes: MAX_SPILL_BYTES }
    }

    /// A spill that writes nowhere, for callers that want the counting without the bytes.
    pub fn discarding() -> Spill {
        Spill { file: None, path: Default::default(), bytes: 0, frames: 0, error: None, max_bytes: 0 }
    }

    pub fn with_cap(mut self, bytes: u64) -> Spill {
        self.max_bytes = bytes;
        self
    }

    /// Write one admitted frame. Never panics and never unwinds — see the module docs.
    pub fn write(&mut self, rgba: &[u8]) {
        if self.error.is_some() {
            return;
        }
        if self.bytes + rgba.len() as u64 > self.max_bytes {
            // A cap is a refusal, not a silent truncation: a clip that stops early and says so
            // is fixable, and one that stops early quietly is a bug report about a "corrupt"
            // video.
            self.error = Some(format!(
                "stopped at {:.0}MB after {} frames — the spill cap",
                self.bytes as f64 / 1e6,
                self.frames
            ));
            return;
        }
        let Some(f) = self.file.as_mut() else { return };
        match f.write_all(rgba) {
            Ok(()) => {
                self.bytes += rgba.len() as u64;
                self.frames += 1;
            }
            Err(e) => self.error = Some(format!("write failed after {} frames: {e}", self.frames)),
        }
    }

    /// Flush and hand back the path, or say what went wrong.
    ///
    /// A spill with no frames is an ERROR rather than an empty file: ffmpeg fed zero bytes of
    /// rawvideo produces a confusing failure of its own, and "nothing was recorded" is the
    /// truth the user needs.
    pub fn finish(mut self) -> Result<Ready, String> {
        if let Some(f) = self.file.as_mut() {
            if let Err(e) = f.flush() {
                self.error.get_or_insert(format!("flush failed: {e}"));
            }
        }
        drop(self.file.take());
        if let Some(e) = self.error {
            return Err(e);
        }
        if self.frames == 0 {
            return Err("no frames were written".into());
        }
        Ok(Ready { path: self.path, frames: self.frames, bytes: self.bytes })
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// A finished spill: raw frames an encoder can read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    pub path: std::path::PathBuf,
    pub frames: u64,
    pub bytes: u64,
}

/// Where a recording spills to.
///
/// The CACHE dir, not data: this is bytes on their way to an encoder, worthless once the clip
/// exists, and exactly what a cache cleaner should be free to delete.
pub fn spill_path(id: u64) -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("snapclone").join("spill").join(format!("{id}.rgba"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("sc-spill-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    /// The blocker itself: frames now exist on disk, byte-for-byte as the recorder admitted
    /// them, because that is exactly what `Source::RawStdin` describes to ffmpeg.
    #[test]
    fn admitted_frames_land_on_disk_unaltered() {
        let dir = scratch("write");
        let path = dir.join("clip.rgba");
        let mut s = Spill::create(&path);
        let a = vec![1u8; 16];
        let b = vec![2u8; 16];
        s.write(&a);
        s.write(&b);
        let ready = s.finish().expect("finished");

        assert_eq!(ready.frames, 2);
        assert_eq!(ready.bytes, 32);
        let on_disk = std::fs::read(&ready.path).unwrap();
        // No header, no per-frame marker: rawvideo is the frames and nothing else, and a
        // single stray byte would shift every pixel after it.
        assert_eq!(on_disk, [a, b].concat());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A recording that silently lost its middle encodes into a clip that jumps, and nothing
    /// says why. A sink cannot return an error, so the failure is remembered.
    #[test]
    fn a_write_failure_is_reported_at_the_end_not_swallowed() {
        // An unwritable path: the directory is a FILE.
        let dir = scratch("fail");
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("notadir");
        std::fs::write(&blocker, b"x").unwrap();

        let mut s = Spill::create(blocker.join("clip.rgba"));
        s.write(&[0u8; 8]);
        let e = s.finish().unwrap_err();
        assert!(!e.is_empty(), "a failed spill reported success");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cap is a refusal, not a silent truncation: a clip that stops early and SAYS SO is
    /// fixable, and one that stops early quietly becomes a bug report about a corrupt video.
    #[test]
    fn the_cap_stops_and_explains_rather_than_truncating() {
        let dir = scratch("cap");
        let mut s = Spill::create(dir.join("clip.rgba")).with_cap(20);
        s.write(&[0u8; 16]);
        assert_eq!(s.frames(), 1);
        s.write(&[0u8; 16]); // over the cap
        assert_eq!(s.frames(), 1, "it wrote past the cap");
        let e = s.finish().unwrap_err();
        assert!(e.contains("cap"), "{e}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ffmpeg fed zero bytes of rawvideo fails in its own confusing way. "Nothing was
    /// recorded" is the truth the user needs.
    #[test]
    fn an_empty_recording_is_an_error_not_an_empty_file() {
        let dir = scratch("empty");
        let s = Spill::create(dir.join("clip.rgba"));
        let e = s.finish().unwrap_err();
        assert!(e.contains("no frames"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bytes on their way to an encoder, worthless once the clip exists — exactly what a
    /// cache cleaner should be free to delete.
    #[test]
    fn spills_live_in_the_cache_dir() {
        let p = spill_path(7);
        let s = p.to_string_lossy();
        assert!(s.contains("/snapclone/spill/"), "{s}");
        assert!(!s.contains("/.local/share/"), "a spill is not user data: {s}");
        assert!(s.ends_with("7.rgba"));
    }
}
