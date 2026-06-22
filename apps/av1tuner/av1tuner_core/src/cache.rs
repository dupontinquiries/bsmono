//! # cache — content-signature caching to skip redundant re-encodes
//!
//! Batch re-runs often re-encode clips that haven't changed. This module
//! computes a cheap-but-reliable signature of a source file + the
//! settings that affect its output, plus a persistent manifest so the
//! batch runner can check "already encoded this exact thing → reuse the
//! output" before spending minutes re-encoding.

use std::path::{Path, PathBuf};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use serde::{Serialize, Deserialize};

/// How thoroughly to fingerprint source content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureMode {
    /// size + mtime + sampled regions. Fast. Default.
    Cheap,
    /// Whole-file streamed hash. Slow but certain.
    Full,
}

impl Default for SignatureMode {
    fn default() -> Self { SignatureMode::Cheap }
}

/// A content+settings signature. Two runs with equal signatures produce
/// byte-identical output, so the cached output can be reused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub content_hash: u64,
    pub size: u64,
    pub settings_hash: u64,
    pub mode: SignatureMode,
}

impl Signature {
    pub fn matches(&self, other: &Signature) -> bool {
        self.mode == other.mode
            && self.size == other.size
            && self.content_hash == other.content_hash
            && self.settings_hash == other.settings_hash
    }

    /// A stable string key for manifest lookup (content+settings+mode).
    pub fn key(&self) -> String {
        let mode = match self.mode {
            SignatureMode::Cheap => "c",
            SignatureMode::Full => "f",
        };
        format!("{}-{:016x}-{:016x}-{}", mode, self.content_hash, self.settings_hash, self.size)
    }
}

/// Compute a content signature for a source file + a settings hash.
pub fn compute_signature(
    source: &Path,
    settings_hash: u64,
    mode: SignatureMode,
) -> std::io::Result<Signature> {
    let meta = std::fs::metadata(source)?;
    let size = meta.len();
    let content_hash = match mode {
        SignatureMode::Cheap => cheap_content_hash(source, size, &meta)?,
        SignatureMode::Full => full_content_hash(source)?,
    };
    Ok(Signature { content_hash, size, settings_hash, mode })
}

fn cheap_content_hash(source: &Path, size: u64, meta: &std::fs::Metadata) -> std::io::Result<u64> {
    let mut h = DefaultHasher::new();
    size.hash(&mut h);
    if let Ok(modified) = meta.modified() {
        if let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) {
            dur.as_secs().hash(&mut h);
        }
    }
    let sample_len: u64 = 64 * 1024;
    let mut f = std::fs::File::open(source)?;
    let mut buf = vec![0u8; sample_len as usize];
    let offsets: [u64; 3] = [
        0,
        size.saturating_sub(sample_len) / 2,
        size.saturating_sub(sample_len),
    ];
    let mut last_off = u64::MAX;
    for off in offsets {
        if off == last_off { continue; }
        last_off = off;
        f.seek(SeekFrom::Start(off))?;
        let n = read_up_to(&mut f, &mut buf)?;
        buf[..n].hash(&mut h);
    }
    Ok(h.finish())
}

fn full_content_hash(source: &Path) -> std::io::Result<u64> {
    let mut h = DefaultHasher::new();
    let mut f = std::fs::File::open(source)?;
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        buf[..n].hash(&mut h);
    }
    Ok(h.finish())
}

fn read_up_to(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        let n = f.read(&mut buf[total..])?;
        if n == 0 { break; }
        total += n;
    }
    Ok(total)
}

/// Helper for callers to hash their settings into a u64.
pub fn hash_settings_fields(fields: &[&str]) -> u64 {
    let mut h = DefaultHasher::new();
    for f in fields { f.hash(&mut h); }
    h.finish()
}

/// One cached encode result: where the output is + its hash for
/// integrity verification on reuse.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CacheEntry {
    pub signature: Signature,
    /// Absolute path to the cached output file.
    pub output_path: PathBuf,
    /// Size of the output file (cheap integrity check).
    pub output_size: u64,
    /// Cheap content hash of the OUTPUT (so we detect a
    /// corrupted/truncated/replaced output and re-encode).
    pub output_hash: u64,
    /// Unix seconds when this entry was recorded.
    pub recorded_at: u64,
}

/// Persistent cache manifest: signature key → entry. Stored as JSON in
/// the config dir (global, so cache hits work across output folders and
/// sessions).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CacheManifest {
    #[serde(default)]
    pub entries: HashMap<String, CacheEntry>,
}

impl CacheManifest {
    /// Path to the global manifest file.
    pub fn default_path() -> Option<PathBuf> {
        config_dir().map(|d| d.join("encode_cache.json"))
    }

    /// Load the manifest, or an empty one if none exists / unreadable.
    pub fn load() -> Self {
        match Self::default_path().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(text) => serde_json::from_str(&text).unwrap_or_default(),
            None => Self::default(),
        }
    }

    /// Persist the manifest to disk (best-effort; creates config dir).
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::default_path() else {
            return Ok(());  // no config dir → silently skip persistence
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Look up a cache hit for a signature. Returns the entry ONLY if the
    /// output file still exists and its hash + size still match what was
    /// recorded — so a corrupted/deleted/modified output correctly
    /// misses and triggers a re-encode rather than handing back a broken
    /// file.
    pub fn lookup(&self, sig: &Signature) -> Option<&CacheEntry> {
        let entry = self.entries.get(&sig.key())?;
        if !entry.signature.matches(sig) {
            return None;
        }
        // Verify the output is still intact.
        let meta = std::fs::metadata(&entry.output_path).ok()?;
        if meta.len() != entry.output_size {
            return None;
        }
        let current_hash = cheap_content_hash(&entry.output_path, meta.len(), &meta).ok()?;
        if current_hash != entry.output_hash {
            return None;
        }
        Some(entry)
    }

    /// Record a successful encode. Hashes the output for later integrity
    /// checks. Does not save to disk — caller batches saves.
    pub fn record(&mut self, sig: Signature, output_path: PathBuf) -> std::io::Result<()> {
        let meta = std::fs::metadata(&output_path)?;
        let output_size = meta.len();
        let output_hash = cheap_content_hash(&output_path, output_size, &meta)?;
        let recorded_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0);
        let key = sig.key();
        self.entries.insert(key, CacheEntry {
            signature: sig, output_path, output_size, output_hash, recorded_at,
        });
        Ok(())
    }

    /// Remove entries whose output file no longer exists (housekeeping).
    pub fn prune_missing(&mut self) {
        self.entries.retain(|_, e| e.output_path.exists());
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
}

/// Config dir for the manifest. Mirrors the convention used elsewhere.
fn config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    { std::env::var("APPDATA").ok().map(|a| PathBuf::from(a).join("av1tuner")) }
    #[cfg(target_os = "macos")]
    { std::env::var("HOME").ok().map(|h|
        PathBuf::from(h).join("Library").join("Application Support").join("av1tuner")) }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() { return Some(PathBuf::from(xdg).join("av1tuner")); }
        }
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config").join("av1tuner"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(content: &[u8]) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "cache_test_{}_{}", std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(content).unwrap();
        p
    }

    #[test]
    fn identical_content_same_signature() {
        let a = temp_file(b"hello world this is some video-ish content");
        let s1 = compute_signature(&a, 42, SignatureMode::Cheap).unwrap();
        let s2 = compute_signature(&a, 42, SignatureMode::Cheap).unwrap();
        assert!(s1.matches(&s2));
        let _ = std::fs::remove_file(&a);
    }

    #[test]
    fn different_settings_differ() {
        let a = temp_file(b"same content");
        let s1 = compute_signature(&a, 100, SignatureMode::Cheap).unwrap();
        let s2 = compute_signature(&a, 200, SignatureMode::Cheap).unwrap();
        assert!(!s1.matches(&s2));
        let _ = std::fs::remove_file(&a);
    }

    #[test]
    fn cheap_and_full_dont_cross_match() {
        let a = temp_file(b"some content for mode comparison test here");
        let cheap = compute_signature(&a, 1, SignatureMode::Cheap).unwrap();
        let full = compute_signature(&a, 1, SignatureMode::Full).unwrap();
        assert!(!cheap.matches(&full));
        let _ = std::fs::remove_file(&a);
    }

    #[test]
    fn settings_hash_helper() {
        let h1 = hash_settings_fields(&["av1", "crf28", "preset6"]);
        let h2 = hash_settings_fields(&["av1", "crf28", "preset6"]);
        let h3 = hash_settings_fields(&["av1", "crf30", "preset6"]);
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn manifest_record_and_lookup() {
        let src = temp_file(b"source video content for manifest test, long enough");
        let out = temp_file(b"encoded output bytes here, also reasonably long");
        let sig = compute_signature(&src, 7, SignatureMode::Cheap).unwrap();

        let mut m = CacheManifest::default();
        m.record(sig.clone(), out.clone()).unwrap();

        // Hit: output intact.
        assert!(m.lookup(&sig).is_some());

        // Miss after output changes (simulate corruption).
        {
            let mut f = std::fs::OpenOptions::new().write(true).open(&out).unwrap();
            f.write_all(b"corrupted!").unwrap();
        }
        // Size or hash now differs → miss.
        assert!(m.lookup(&sig).is_none());

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn manifest_miss_when_output_deleted() {
        let src = temp_file(b"another source for deletion test, padded out a bit");
        let out = temp_file(b"output that will be deleted before lookup occurs");
        let sig = compute_signature(&src, 3, SignatureMode::Cheap).unwrap();
        let mut m = CacheManifest::default();
        m.record(sig.clone(), out.clone()).unwrap();
        std::fs::remove_file(&out).unwrap();
        assert!(m.lookup(&sig).is_none());
        let _ = std::fs::remove_file(&src);
    }

    #[test]
    fn signature_key_stable() {
        let a = temp_file(b"key stability check content here padded out");
        let s1 = compute_signature(&a, 5, SignatureMode::Cheap).unwrap();
        let s2 = compute_signature(&a, 5, SignatureMode::Cheap).unwrap();
        assert_eq!(s1.key(), s2.key());
        let _ = std::fs::remove_file(&a);
    }
}
