//! Locate whisper / speech models on disk.
//!
//! Lives outside the `#[cfg(feature = "voice")]` gate so the resolver
//! logic — and its tests — compile and run in every build config.
//! The actual whisper-rs `Transcriber` consumer of these paths IS
//! feature-gated; this module just answers "where's the model file?"
//! without depending on whisper-rs.

use std::path::{Path, PathBuf};

/// Filename candidates per directory. Order matters: first existing
/// wins. Quantized .gguf first, then .bin, then full-precision variants.
///
/// `.gguf` is the modern unified format (used by llama.cpp and recent
/// whisper.cpp builds). `.bin` is the older GGML format. Whisper.cpp's
/// `WhisperContext::new` auto-detects from the file header, so the
/// resolver doesn't need to care — it just returns a path.
///
/// Quantized variants are 2-4x smaller than fp16 with negligible WER
/// loss, important on phone where the fp16 base model is half the
/// app's RAM budget.
const FILENAMES: &[&str] = &[
    // Quantized GGUF — modern preferred format
    "ggml-base.en-q5_1.gguf",
    "ggml-base.en-q5_0.gguf",
    "ggml-tiny.en-q5_1.gguf",
    "ggml-tiny.en-q5_0.gguf",
    // Quantized GGML
    "ggml-base.en-q5_1.bin",
    "ggml-tiny.en-q5_1.bin",
    // Full-precision GGUF
    "ggml-base.en.gguf",
    "ggml-base.gguf",
    "ggml-tiny.en.gguf",
    // Full-precision GGML
    "ggml-base.en.bin",
    "ggml-base.bin",
    "ggml-tiny.en.bin",
];

/// Look in standard places for a whisper model. Returns the first
/// existing path, or `None`.
///
/// Directory priority, highest first:
///   1. `$WHISPER_MODELS_DIR` env override
///   2. `~/models/` — unified user-models directory (shared with
///      llama.cpp, ollama, jan.ai, etc.)
///   3. `~/.local/share/rusty-board/models/` — XDG data dir
///   4. `~/.config/web2qml/models/` — sister app
///   5. `~/.local/share/whisper/models/` — whisper.cpp convention
///   6. `/usr/share/rusty-board/models/` — system-wide
///   7. `/usr/share/whisper/models/`
pub fn find_default_model() -> Option<PathBuf> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));

    let mut dirs: Vec<PathBuf> = Vec::new();

    // 1. Env override.
    if let Ok(env_dir) = std::env::var("WHISPER_MODELS_DIR") {
        if !env_dir.is_empty() {
            dirs.push(PathBuf::from(env_dir));
        }
    }

    // 2. ~/models — unified user-models directory.
    dirs.push(home.join("models"));

    // 3. rusty-board's own data dir.
    dirs.push(dirs_share().join("rusty-board/models"));

    // 4. voiceterm/web2qml's data dir.
    dirs.push(home.join(".config/web2qml/models"));

    // 5. Whisper.cpp's conventional location.
    dirs.push(dirs_share().join("whisper/models"));

    // 6. System-wide.
    dirs.push(PathBuf::from("/usr/share/rusty-board/models"));
    dirs.push(PathBuf::from("/usr/share/whisper/models"));

    for dir in &dirs {
        for filename in FILENAMES {
            let p = dir.join(filename);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

fn dirs_share() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Process-unique temp dir under temp_dir. Each call returns a
    /// FRESH directory — tests in the same process don't collide.
    fn tempdir() -> PathBuf {
        let pid = std::process::id();
        static COUNTER: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("rusty-board-test-models-{pid}-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir -p");
        }
        std::fs::File::create(path).expect("touch");
    }

    /// Run a closure with isolated HOME + cleared WHISPER_MODELS_DIR.
    /// Process-wide env mutation, so tests in this module serialize
    /// via a mutex.
    fn with_isolated_env<F: FnOnce(&Path)>(f: F) {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let tmp = tempdir();
        let prev_home = std::env::var("HOME").ok();
        let prev_env = std::env::var("WHISPER_MODELS_DIR").ok();
        // SAFETY: serialized by LOCK above.
        unsafe {
            std::env::set_var("HOME", &tmp);
            std::env::remove_var("WHISPER_MODELS_DIR");
        }
        f(&tmp);
        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prev_env {
                Some(v) => std::env::set_var("WHISPER_MODELS_DIR", v),
                None => std::env::remove_var("WHISPER_MODELS_DIR"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn finds_model_in_home_models_directory() {
        with_isolated_env(|home| {
            let model = home.join("models/ggml-base.en-q5_1.gguf");
            touch(&model);
            let found = find_default_model().expect("model should be found");
            assert_eq!(found, model);
        });
    }

    #[test]
    fn gguf_preferred_over_bin_in_same_dir() {
        with_isolated_env(|home| {
            let gguf = home.join("models/ggml-base.en-q5_1.gguf");
            let bin = home.join("models/ggml-base.en-q5_1.bin");
            touch(&bin);
            touch(&gguf);
            let found = find_default_model().expect("should find a model");
            assert_eq!(
                found, gguf,
                ".gguf is the modern format and must take priority"
            );
        });
    }

    #[test]
    fn quantized_preferred_over_fp16() {
        with_isolated_env(|home| {
            let q5 = home.join("models/ggml-base.en-q5_1.gguf");
            let fp = home.join("models/ggml-base.en.gguf");
            touch(&fp);
            touch(&q5);
            let found = find_default_model().expect("should find");
            assert_eq!(
                found, q5,
                "quantized must win — fp16 is too big for phone RAM"
            );
        });
    }

    #[test]
    fn env_override_wins_over_home_models() {
        with_isolated_env(|home| {
            let home_model = home.join("models/ggml-base.en-q5_1.gguf");
            touch(&home_model);
            let env_dir = home.join("custom-build/models");
            let env_model = env_dir.join("ggml-base.en-q5_1.gguf");
            touch(&env_model);
            unsafe {
                std::env::set_var("WHISPER_MODELS_DIR", &env_dir);
            }
            let found = find_default_model().expect("should find");
            assert_eq!(
                found, env_model,
                "WHISPER_MODELS_DIR must take priority over ~/models"
            );
            unsafe {
                std::env::remove_var("WHISPER_MODELS_DIR");
            }
        });
    }

    #[test]
    fn env_dir_empty_string_is_ignored() {
        // Empty WHISPER_MODELS_DIR must not turn into an empty path
        // (which would resolve to cwd and could match unrelated files).
        with_isolated_env(|home| {
            let home_model = home.join("models/ggml-base.en-q5_1.gguf");
            touch(&home_model);
            unsafe {
                std::env::set_var("WHISPER_MODELS_DIR", "");
            }
            let found = find_default_model().expect("should find via home");
            assert_eq!(found, home_model);
            unsafe {
                std::env::remove_var("WHISPER_MODELS_DIR");
            }
        });
    }

    /// .bin still resolves when no .gguf is present — backward compat
    /// for users who haven't migrated.
    #[test]
    fn bin_still_resolves_without_gguf() {
        with_isolated_env(|home| {
            let bin = home.join("models/ggml-base.en-q5_1.bin");
            touch(&bin);
            let found = find_default_model().expect("should find");
            assert_eq!(found, bin);
        });
    }
}
