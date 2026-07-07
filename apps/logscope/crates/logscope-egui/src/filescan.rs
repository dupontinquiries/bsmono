//! filescan.rs — bounded, filtered project scanning for the mount.
//!
//! `read_tree` used to slurp EVERY utf-8 file with no bounds: a large repo would blow memory, and the swarm
//! would drown in vendored/generated noise (node_modules, target, lockfiles, minified bundles). This decides
//! what's worth mounting — source-like text under size caps, skipping vcs/build/vendor directories and
//! lock/binary files, with per-file and total-bytes ceilings. Pure predicates, unit-tested; the IO walk in
//! `devloop::read_tree` consumes them.

/// Ceilings for a single mount scan. Defaults are generous for a source tree but bound the pathological
/// cases (a checked-in dataset, a giant generated file, a monorepo).
#[derive(Clone, Copy, Debug)]
pub struct ScanLimits {
    pub max_file_bytes: usize,  // skip any single file larger than this
    pub max_total_bytes: usize, // stop once the cumulative mounted text passes this
    pub max_files: usize,       // stop once this many files are mounted
}
impl Default for ScanLimits {
    fn default() -> Self { Self { max_file_bytes: 512 * 1024, max_total_bytes: 16 * 1024 * 1024, max_files: 2000 } }
}

/// Why a path was left out — surfaced so a mount can report "skipped 12 files (3 too large, 9 vendored)".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip { VendorDir, Binary, TooLarge, LockFile }
impl Skip {
    pub fn label(self) -> &'static str {
        match self { Skip::VendorDir => "vendored/generated", Skip::Binary => "binary", Skip::TooLarge => "too large", Skip::LockFile => "lockfile" }
    }
}

/// Directories never worth descending into for a source mount: version control, build output, dependency
/// caches, and framework scratch. Matches on the bare component name (case-sensitive — these are canonical).
pub fn skip_dir(name: &str) -> bool {
    matches!(name,
        ".git" | ".hg" | ".svn" |
        "target" | "build" | "dist" | "out" | "bin" | "obj" |
        "node_modules" | "vendor" | ".venv" | "venv" | "__pycache__" | ".mypy_cache" | ".pytest_cache" |
        ".next" | ".nuxt" | ".svelte-kit" | ".cargo" | ".gradle" | ".idea" | ".vscode"
    )
}

/// Extensions that are binary or otherwise not source text worth mounting.
fn is_binary_ext(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" |
        "pdf" | "zip" | "gz" | "tar" | "xz" | "zst" | "7z" | "bundle" |
        "wasm" | "so" | "dylib" | "dll" | "a" | "o" | "rlib" | "exe" | "bin" |
        "mp3" | "mp4" | "wav" | "mov" | "ogg" | "flac" |
        "ttf" | "otf" | "woff" | "woff2" |
        "gguf" | "safetensors" | "onnx" | "npz" | "pt" | "ckpt" |
        "lock" | "min" // *.lock, *.min.js catch-alls (lock also handled by name below)
    )
}

/// Lockfiles: huge, generated, and useless for the swarm to read or edit.
fn is_lock(name: &str) -> bool {
    matches!(name, "Cargo.lock" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml" | "poetry.lock" | "Gemfile.lock" | "composer.lock" | "go.sum")
}

/// Should this file be mounted? `Ok(())` to include, `Err(reason)` to skip. Size is the on-disk byte length.
pub fn classify_file(name: &str, size: usize, lim: &ScanLimits) -> Result<(), Skip> {
    if is_lock(name) { return Err(Skip::LockFile); }
    if is_binary_ext(name) { return Err(Skip::Binary); }
    if size > lim.max_file_bytes { return Err(Skip::TooLarge); }
    Ok(())
}

/// Running budget for a scan: tracks totals against the ceilings so the walk can stop early.
#[derive(Clone, Copy, Debug)]
pub struct ScanBudget { pub files: usize, pub bytes: usize }
impl ScanBudget {
    pub fn new() -> Self { Self { files: 0, bytes: 0 } }
    /// True once the scan should stop taking files (limits reached).
    pub fn full(&self, lim: &ScanLimits) -> bool { self.files >= lim.max_files || self.bytes >= lim.max_total_bytes }
    /// Record an included file.
    pub fn add(&mut self, bytes: usize) { self.files += 1; self.bytes += bytes; }
}
impl Default for ScanBudget { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_vendor_and_build_dirs() {
        for d in ["target", "node_modules", ".git", ".venv", "dist", "__pycache__", ".next"] {
            assert!(skip_dir(d), "{d} should be skipped");
        }
        assert!(!skip_dir("src"));
        assert!(!skip_dir("crates"));
        assert!(!skip_dir("tests"));
    }

    #[test]
    fn classify_keeps_source_skips_the_rest() {
        let lim = ScanLimits::default();
        assert_eq!(classify_file("lib.rs", 1000, &lim), Ok(()));
        assert_eq!(classify_file("main.wgsl", 5000, &lim), Ok(()));
        assert_eq!(classify_file("README.md", 2000, &lim), Ok(()));
        assert_eq!(classify_file("Cargo.lock", 40_000, &lim), Err(Skip::LockFile));
        assert_eq!(classify_file("logo.png", 10_000, &lim), Err(Skip::Binary));
        assert_eq!(classify_file("model.gguf", 10_000, &lim), Err(Skip::Binary));
        assert_eq!(classify_file("huge.rs", 2_000_000, &lim), Err(Skip::TooLarge));
    }

    #[test]
    fn budget_stops_at_ceilings() {
        let lim = ScanLimits { max_file_bytes: 1024, max_total_bytes: 3000, max_files: 5 };
        let mut b = ScanBudget::new();
        assert!(!b.full(&lim));
        b.add(1000); b.add(1000); b.add(1000); // 3000 bytes
        assert!(b.full(&lim), "hit the total-bytes ceiling");
        let mut b2 = ScanBudget::new();
        for _ in 0..5 { b2.add(1); }
        assert!(b2.full(&lim), "hit the file-count ceiling");
    }

    #[test]
    fn skip_labels_are_stable() {
        assert_eq!(Skip::TooLarge.label(), "too large");
        assert_eq!(Skip::VendorDir.label(), "vendored/generated");
    }
}
