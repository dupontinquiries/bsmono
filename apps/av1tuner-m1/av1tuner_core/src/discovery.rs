//! # discovery — find video files in folders and archives
//!
//! Utilities for pulling video files into a batch from messy sources: 
//! a folder (optionally recursive), or a zip archive (with a warning 
//! that zipping already-compressed video is pointless).
//!
//! These are pure helpers — no job/encode logic. The UI calls these to 
//! populate a batch list, then the batch logic (elsewhere) takes over.

use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("path is not a directory: {0}")]
    NotADirectory(String),
    #[error("zip: {0}")]
    Zip(String),
}

/// Video container extensions we recognize. Lowercase, no dot.
/// Deliberately conservative — only formats ffmpeg reliably decodes 
/// and that are plausibly video (not audio-only or images).
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "webm", "m4v", "mpg", "mpeg",
    "ts", "mts", "m2ts", "wmv", "flv", "ogv", "3gp", "vob",
    "y4m", "ivf",
];

/// True if a path has a recognized video extension.
pub fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let lower = e.to_ascii_lowercase();
            VIDEO_EXTENSIONS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

/// A discovered video, with enough metadata for the UI to show it 
/// before committing to encode.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredVideo {
    pub path: PathBuf,
    /// Size in bytes (for display + sorting).
    pub bytes: u64,
    /// Path relative to the scanned root, for display + output mirroring.
    pub relative: PathBuf,
}

/// Find video files in a folder.
///
/// `recursive` controls whether subfolders are walked. Results are 
/// sorted by relative path so the UI shows them in a stable, 
/// folder-grouped order. Symlinks are not followed (avoids cycles and 
/// surprises). Hidden files/folders (leading `.`) are skipped.
pub fn scan_folder(root: &Path, recursive: bool) -> Result<Vec<DiscoveredVideo>, DiscoveryError> {
    if !root.is_dir() {
        return Err(DiscoveryError::NotADirectory(root.display().to_string()));
    }
    let mut out = Vec::new();
    scan_inner(root, root, recursive, &mut out)?;
    out.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(out)
}

fn scan_inner(
    root: &Path,
    dir: &Path,
    recursive: bool,
    out: &mut Vec<DiscoveredVideo>,
) -> Result<(), DiscoveryError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // Skip hidden files/folders.
        if path.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('.'))
            .unwrap_or(false)
        {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;  // don't follow symlinks
        }
        if file_type.is_dir() {
            if recursive {
                scan_inner(root, &path, recursive, out)?;
            }
        } else if file_type.is_file() && is_video_file(&path) {
            let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let relative = path.strip_prefix(root)
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|_| path.clone());
            out.push(DiscoveredVideo { path, bytes, relative });
        }
    }
    Ok(())
}

/// Summary of a zip's video contents, produced before extraction so 
/// the UI can warn the user.
#[derive(Clone, Debug)]
pub struct ZipInspection {
    /// Video entries found inside the zip (names only, not extracted).
    pub video_names: Vec<String>,
    /// Total compressed size of the zip's video entries.
    pub compressed_bytes: u64,
    /// Total uncompressed size of the zip's video entries.
    pub uncompressed_bytes: u64,
}

impl ZipInspection {
    /// The compression ratio the zip achieved on its video content. 
    /// For already-compressed video this is near 1.0 (zip can't shrink 
    /// it), which is the basis for the "not recommended" warning.
    pub fn video_compression_ratio(&self) -> f64 {
        if self.uncompressed_bytes == 0 {
            return 1.0;
        }
        self.compressed_bytes as f64 / self.uncompressed_bytes as f64
    }
    
    /// Whether zipping these videos was pointless (ratio near 1.0 means 
    /// the zip barely shrank them — typical for already-compressed 
    /// video). Threshold 0.95: if the zip saved less than 5%, warn.
    pub fn zip_was_pointless(&self) -> bool {
        !self.video_names.is_empty() && self.video_compression_ratio() > 0.95
    }
}

/// Inspect a zip's video contents without extracting. Used to show the 
/// user what's inside + warn about pointless video-in-zip.
pub fn inspect_zip(zip_path: &Path) -> Result<ZipInspection, DiscoveryError> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| DiscoveryError::Zip(e.to_string()))?;
    
    let mut video_names = Vec::new();
    let mut compressed_bytes = 0u64;
    let mut uncompressed_bytes = 0u64;
    
    for i in 0..archive.len() {
        let entry = archive.by_index(i)
            .map_err(|e| DiscoveryError::Zip(e.to_string()))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if is_video_file(Path::new(&name)) {
            video_names.push(name);
            compressed_bytes += entry.compressed_size();
            uncompressed_bytes += entry.size();
        }
    }
    
    Ok(ZipInspection { video_names, compressed_bytes, uncompressed_bytes })
}

/// Extract video files from a zip into a destination folder. Returns 
/// the paths of the extracted videos. Only video files are extracted; 
/// other contents are ignored. Flattens the zip's internal structure 
/// into `dest` using just the file names (collisions get a numeric 
/// suffix).
pub fn extract_videos_from_zip(
    zip_path: &Path,
    dest: &Path,
) -> Result<Vec<PathBuf>, DiscoveryError> {
    std::fs::create_dir_all(dest)?;
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| DiscoveryError::Zip(e.to_string()))?;
    
    let mut extracted = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)
            .map_err(|e| DiscoveryError::Zip(e.to_string()))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if !is_video_file(Path::new(&name)) {
            continue;
        }
        // Use just the file name, flattening the zip's directory layout.
        let base = Path::new(&name)
            .file_name()
            .map(|s| s.to_os_string())
            .unwrap_or_else(|| std::ffi::OsString::from(format!("video_{}", i)));
        let mut target = dest.join(&base);
        // Avoid collisions: foo.mp4 → foo_1.mp4, foo_2.mp4, ...
        if target.exists() {
            target = unique_path(dest, &base);
        }
        let mut out = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut out)?;
        extracted.push(target);
    }
    Ok(extracted)
}

/// Produce a non-colliding path in `dir` based on `base` by inserting 
/// a numeric suffix before the extension.
fn unique_path(dir: &Path, base: &std::ffi::OsStr) -> PathBuf {
    let base_path = Path::new(base);
    let stem = base_path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("video");
    let ext = base_path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    for n in 1..10_000 {
        let candidate = if ext.is_empty() {
            dir.join(format!("{}_{}", stem, n))
        } else {
            dir.join(format!("{}_{}.{}", stem, n, ext))
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    // Pathological fallback.
    dir.join(format!("{}_{}", stem, std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn recognizes_video_extensions() {
        assert!(is_video_file(Path::new("clip.mp4")));
        assert!(is_video_file(Path::new("clip.MKV")));  // case-insensitive
        assert!(is_video_file(Path::new("/a/b/c.mov")));
        assert!(!is_video_file(Path::new("notes.txt")));
        assert!(!is_video_file(Path::new("song.mp3")));
        assert!(!is_video_file(Path::new("noext")));
    }
    
    #[test]
    fn zip_pointless_detection() {
        // Already-compressed video: zip barely shrinks it.
        let inspection = ZipInspection {
            video_names: vec!["a.mp4".into()],
            compressed_bytes: 990,
            uncompressed_bytes: 1000,
        };
        assert!(inspection.zip_was_pointless());
        assert!((inspection.video_compression_ratio() - 0.99).abs() < 0.001);
        
        // Empty zip: not pointless (nothing to warn about).
        let empty = ZipInspection {
            video_names: vec![],
            compressed_bytes: 0,
            uncompressed_bytes: 0,
        };
        assert!(!empty.zip_was_pointless());
    }
    
    #[test]
    fn scan_nonexistent_errors() {
        let result = scan_folder(Path::new("/nonexistent/xyz/123"), false);
        assert!(result.is_err());
    }
}
