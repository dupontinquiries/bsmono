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
    if let Some(p) = env_override {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    for dir in dirs {
        for cand in MODEL_CANDIDATES {
            let p = dir.join(cand);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
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
}
