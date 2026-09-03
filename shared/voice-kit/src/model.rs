//! model.rs — whisper (ggml) model resolution and variant metadata.
//!
//! Both apps locate a ggml model on disk and reason about its size/quantization;
//! rusty-board's `voice/model_path.rs` is the basis. Resolution is generalized so
//! the app supplies its own search directories (XDG/install paths differ per app);
//! the preference order (quantized `.gguf` > `.bin` > full-precision; base before
//! tiny) and the filename → variant parsing are the shared parts. std only.

use std::path::{Path, PathBuf};

/// Candidate model filenames in preference order: quantized `.gguf` first (2–4×
/// smaller than fp16 with negligible WER loss — important on phones), then `.bin`,
/// then full-precision; `base` before `tiny`.
pub const MODEL_CANDIDATES: &[&str] = &[
    "ggml-base.en-q5_1.gguf",
    "ggml-base.en-q5_0.gguf",
    "ggml-tiny.en-q5_1.gguf",
    "ggml-tiny.en-q5_0.gguf",
    "ggml-base.en-q5_1.bin",
    "ggml-tiny.en-q5_1.bin",
    "ggml-base.en.gguf",
    "ggml-base.gguf",
    "ggml-tiny.en.gguf",
    "ggml-base.en.bin",
    "ggml-base.bin",
    "ggml-tiny.en.bin",
];

/// Resolve a whisper model: if `env_override` is set and names an existing file,
/// use it; otherwise return the first existing `dir/candidate`, scanning `dirs` in
/// the given order and [`MODEL_CANDIDATES`] in preference order. `None` if nothing
/// is found.
pub fn find_model_in(dirs: &[PathBuf], env_override: Option<&str>) -> Option<PathBuf> {
    find_model_in_preferring(dirs, env_override, prefer_from_env())
}

/// Which size to reach for first when more than one model is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Prefer {
    /// [`MODEL_CANDIDATES`] as written: `base` before `tiny` (accuracy).
    #[default]
    Accuracy,
    /// `tiny` before `base` — roughly 3× faster inference, for a phone that reports its
    /// partials falling behind, or for the keyboard's streaming loop where the next partial
    /// corrects the last one anyway.
    Speed,
}

/// `WHISPER_PREFER=speed|tiny` → [`Prefer::Speed`]; anything else (or unset) → accuracy.
/// An env rather than a per-app config because the trade-off is the machine's, not the
/// app's: every voice surface on a slow phone wants the same answer.
pub fn prefer_from_env() -> Prefer {
    match std::env::var("WHISPER_PREFER").ok().as_deref().map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("speed") | Some("tiny") | Some("fast") => Prefer::Speed,
        _ => Prefer::Accuracy,
    }
}

/// [`find_model_in`] with the size preference explicit. An explicit `env_override` file still
/// wins outright — a person naming a model gets that model.
pub fn find_model_in_preferring(dirs: &[PathBuf], env_override: Option<&str>, prefer: Prefer) -> Option<PathBuf> {
    if let Some(p) = env_override {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let order = candidates_for(prefer);
    for dir in dirs {
        for cand in &order {
            let p = dir.join(cand);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// The candidate list in the order `prefer` wants: for [`Prefer::Speed`], every `tiny`
/// candidate (in its own quantization-first order) ahead of everything else.
pub fn candidates_for(prefer: Prefer) -> Vec<&'static str> {
    match prefer {
        Prefer::Accuracy => MODEL_CANDIDATES.to_vec(),
        Prefer::Speed => {
            let (tiny, rest): (Vec<_>, Vec<_>) = MODEL_CANDIDATES.iter().copied().partition(|c| c.contains("tiny"));
            tiny.into_iter().chain(rest).collect()
        }
    }
}

/// Find a whisper model in the **canonical model store** (via models-kit), honoring a
/// `GITMIND_MODELS`-style env override. This routes voice model discovery through the shared
/// `~/models` layout that every app uses, instead of each caller guessing directories.
pub fn find_whisper_model(env_override: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = env_override {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    find_model_in(&models_kit::search_dirs("whisper"), None)
}

/// Whisper model size class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelSize {
    Tiny,
    Base,
    Small,
    Medium,
    Large,
}

/// Metadata parsed from a ggml model filename.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    pub size: ModelSize,
    /// `.en` English-only model (vs. multilingual).
    pub english_only: bool,
    /// Quantization tag like `q5_1` / `q8_0`, or `None` for full precision.
    pub quantization: Option<String>,
}

impl Variant {
    /// Parse a ggml filename, e.g. `ggml-base.en-q5_1.gguf` →
    /// `{ Base, english_only: true, quantization: Some("q5_1") }`. `None` if no
    /// size token (`tiny`/`base`/`small`/`medium`/`large`) is present.
    pub fn from_filename(name: &str) -> Option<Variant> {
        let lower = name.to_lowercase();
        let size = if lower.contains("large") {
            ModelSize::Large
        } else if lower.contains("medium") {
            ModelSize::Medium
        } else if lower.contains("small") {
            ModelSize::Small
        } else if lower.contains("base") {
            ModelSize::Base
        } else if lower.contains("tiny") {
            ModelSize::Tiny
        } else {
            return None;
        };
        let english_only = lower.contains(".en");
        // quantization tag follows "-q": "...-q5_1.gguf" → "q5_1"
        let quantization = lower
            .find("-q")
            .and_then(|i| lower[i + 1..].split('.').next())
            .map(|s| s.to_string());
        Some(Variant { size, english_only, quantization })
    }
}

/// Convenience: parse a path's filename via [`Variant::from_filename`].
pub fn variant_of(path: &Path) -> Option<Variant> {
    path.file_name().and_then(|n| n.to_str()).and_then(Variant::from_filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("voicekit-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn find_whisper_model_uses_override_then_models_kit() {
        // a valid override file wins
        let dir = scratch("wm");
        let m = dir.join("model.bin");
        std::fs::write(&m, b"x").unwrap();
        assert_eq!(find_whisper_model(Some(m.to_str().unwrap())), Some(m.clone()));
        // a bogus override falls through to models-kit's canonical whisper dirs without panic
        assert!(find_whisper_model(Some("/no/such/model.bin")).is_none());
        // models-kit always offers at least the canonical whisper dir to search
        assert!(!models_kit::search_dirs("whisper").is_empty());
    }

    #[test]
    fn prefers_gguf_and_quantized_over_alternatives() {
        let d = scratch("pref");
        // a less-preferred and the most-preferred candidate both present
        std::fs::write(d.join("ggml-base.en.bin"), b"x").unwrap();
        std::fs::write(d.join("ggml-base.en-q5_1.gguf"), b"x").unwrap();
        let found = find_model_in(&[d.clone()], None).unwrap();
        assert!(found.ends_with("ggml-base.en-q5_1.gguf"), "got {found:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn env_override_wins_when_the_file_exists() {
        let d = scratch("env");
        let over = d.join("my-custom-model.gguf");
        std::fs::write(&over, b"x").unwrap();
        std::fs::write(d.join("ggml-base.en-q5_1.gguf"), b"x").unwrap();
        assert_eq!(find_model_in(&[d.clone()], over.to_str()).unwrap(), over);
        // a non-existent override falls through to the scan
        let found = find_model_in(&[d.clone()], Some("/nope/missing.gguf")).unwrap();
        assert!(found.ends_with("ggml-base.en-q5_1.gguf"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn none_when_no_model_present() {
        let d = scratch("none");
        assert!(find_model_in(&[d.clone()], None).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn parses_variant_metadata() {
        let v = Variant::from_filename("ggml-base.en-q5_1.gguf").unwrap();
        assert_eq!(v.size, ModelSize::Base);
        assert!(v.english_only);
        assert_eq!(v.quantization.as_deref(), Some("q5_1"));

        let t = Variant::from_filename("ggml-tiny.bin").unwrap();
        assert_eq!(t.size, ModelSize::Tiny);
        assert!(!t.english_only && t.quantization.is_none());

        assert_eq!(Variant::from_filename("ggml-large-v3.bin").unwrap().size, ModelSize::Large);
        assert!(Variant::from_filename("not-a-model.txt").is_none());
    }

    #[test]
    fn speed_preference_puts_every_tiny_first_and_keeps_quantization_order() {
        let fast = candidates_for(Prefer::Speed);
        let n_tiny = fast.iter().take_while(|c| c.contains("tiny")).count();
        assert!(n_tiny >= 4, "{fast:?}");
        assert!(fast[n_tiny..].iter().all(|c| !c.contains("tiny")), "no tiny after the first non-tiny");
        assert!(fast[0].contains("q5_1"), "quantized first within the tinies: {}", fast[0]);
        assert_eq!(candidates_for(Prefer::Accuracy), MODEL_CANDIDATES.to_vec());
        // and the explicit file still wins under either preference
        let dir = std::env::temp_dir().join(format!("vk-prefer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["ggml-base.en-q5_1.gguf", "ggml-tiny.en-q5_1.gguf", "custom.bin"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let dirs = vec![dir.clone()];
        assert!(find_model_in_preferring(&dirs, None, Prefer::Accuracy).unwrap().ends_with("ggml-base.en-q5_1.gguf"));
        assert!(find_model_in_preferring(&dirs, None, Prefer::Speed).unwrap().ends_with("ggml-tiny.en-q5_1.gguf"));
        let custom = dir.join("custom.bin");
        assert_eq!(find_model_in_preferring(&dirs, custom.to_str(), Prefer::Speed).unwrap(), custom);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
