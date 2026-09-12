//! models-kit — one place for AI model weights: `~/models`.
//!
//! Every app in the portfolio downloads or bundles model weights somewhere different
//! (voiceterm's `model_download`, voice-kit's ggml resolver, beatspace's GAN checkpoints,
//! snapclone's YOLO weights). This crate makes `~/models` the single canonical home and
//! provides the migration-aware search path so nothing re-downloads what another app
//! already has.
//!
//! Layout convention under the root: `<kind>/<name>` — e.g. `~/models/whisper/ggml-base.bin`,
//! `~/models/yolo/yolov8n-face.onnx`, `~/models/beatspace/kbm3-mel.safetensors`.

use std::path::PathBuf;

/// The canonical models root: `$GITMIND_MODELS`, else `$XDG_DATA_HOME/../models` is *not*
/// used — we deliberately pick the simple, discoverable `~/models`.
pub fn models_root() -> PathBuf {
    if let Ok(p) = std::env::var("GITMIND_MODELS") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("models")
}

/// Resolve a model by `kind` and `name` under the root, e.g. `resolve("whisper",
/// "ggml-base.en.bin")` → `~/models/whisper/ggml-base.en.bin`.
pub fn resolve(kind: &str, name: &str) -> PathBuf {
    models_root().join(kind).join(name)
}

/// Ensure `~/models/<kind>/` exists, returning it. Best-effort; errors surface to the
/// caller so a read-only home fails loudly rather than silently writing elsewhere.
pub fn ensure_kind_dir(kind: &str) -> std::io::Result<PathBuf> {
    let d = models_root().join(kind);
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

/// Migration-aware search path: the canonical root first, then the legacy locations apps
/// used before unification, so an existing download is found instead of re-fetched. Pass
/// this to voice-kit's `find_model_in`.
pub fn search_dirs(kind: &str) -> Vec<PathBuf> {
    let mut dirs = vec![models_root().join(kind), models_root()];
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    // legacy spots (pre-unification) — read-only fallbacks for discovery, never write here
    for legacy in [
        format!("{home}/.cache/whisper"),
        format!("{home}/.cache/gitmind/models"),
        format!("{home}/.local/share/models"),
        format!("{home}/.{kind}"),
        // voiceterm downloaded whisper models here for a long time and still
        // does; a user who has one already should not be told to fetch it
        // again because the canonical store is empty.
        format!("{home}/.config/web2qml/models"),
    ] {
        dirs.push(PathBuf::from(legacy));
    }
    dirs
}

/// True if a model file exists anywhere on the search path for `kind`.
pub fn find(kind: &str, name: &str) -> Option<PathBuf> {
    for d in search_dirs(kind) {
        let p = d.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn with_env<T>(key: &str, val: Option<&str>, f: impl FnOnce() -> T) -> T {
        let prev = std::env::var(key).ok();
        match val {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        let out = f();
        match prev {
            Some(p) => std::env::set_var(key, p),
            None => std::env::remove_var(key),
        }
        out
    }

    #[test]
    fn env_override_wins_and_root_defaults_to_home_models() {
        with_env("GITMIND_MODELS", Some("/data/models"), || {
            assert_eq!(models_root(), Path::new("/data/models"));
            assert_eq!(resolve("whisper", "ggml-base.bin"), Path::new("/data/models/whisper/ggml-base.bin"));
        });
        with_env("GITMIND_MODELS", None, || {
            with_env("HOME", Some("/home/kesslerd"), || {
                assert_eq!(models_root(), Path::new("/home/kesslerd/models"));
            });
        });
    }

    #[test]
    fn search_puts_canonical_first_then_legacy() {
        with_env("GITMIND_MODELS", Some("/m"), || {
            let dirs = search_dirs("yolo");
            assert_eq!(dirs[0], Path::new("/m/yolo"));
            assert_eq!(dirs[1], Path::new("/m"));
            assert!(dirs.len() > 2, "legacy fallbacks included for migration");
        });
    }
}

#[cfg(test)]
mod convention_tests {
    use super::*;

    /// `~/models` is the canonical root, and the layout is `<kind>/<name>`.
    #[test]
    fn the_root_is_home_models() {
        let root = models_root();
        assert!(
            root.ends_with("models"),
            "the canonical root moved: {}",
            root.display()
        );
        assert_eq!(
            resolve("whisper", "ggml-base.en.bin"),
            root.join("whisper").join("ggml-base.en.bin")
        );
    }

    /// The override wins, so a machine with weights on another volume does not
    /// have to symlink a home directory.
    #[test]
    fn the_env_override_wins_and_blank_does_not_count() {
        // A blank override is someone exporting an empty variable, which must
        // not silently relocate every model lookup to the filesystem root.
        std::env::set_var("GITMIND_MODELS", "   ");
        assert!(models_root().ends_with("models"));
        std::env::set_var("GITMIND_MODELS", "/tmp/elsewhere");
        assert_eq!(models_root(), std::path::PathBuf::from("/tmp/elsewhere"));
        std::env::remove_var("GITMIND_MODELS");
    }

    /// The canonical directory must be searched BEFORE any legacy one, or a
    /// stale copy in an old location shadows the current model forever.
    #[test]
    fn the_canonical_dir_is_searched_first() {
        let dirs = search_dirs("whisper");
        let canonical = models_root().join("whisper");
        assert_eq!(dirs.first(), Some(&canonical), "{dirs:?}");
    }

    /// voiceterm's historical directory stays discoverable: a user who already
    /// has a model there should not be told to download it again.
    #[test]
    fn the_legacy_voiceterm_directory_is_still_searched() {
        let dirs = search_dirs("whisper");
        assert!(
            dirs.iter().any(|d| d.ends_with(".config/web2qml/models")),
            "voiceterm's existing installs became invisible: {dirs:?}"
        );
    }

    /// Every legacy entry is read-only by convention; only the canonical root
    /// is ever written to. Nothing enforces that mechanically, so this at
    /// least pins that the first entry — the one a writer would pick — is the
    /// canonical one.
    #[test]
    fn a_writer_taking_the_first_dir_writes_canonically() {
        for kind in ["whisper", "yolo", "beatspace"] {
            assert_eq!(search_dirs(kind).first(), Some(&models_root().join(kind)));
        }
    }
}
