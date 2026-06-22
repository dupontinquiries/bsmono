//! whisper_models: the curated catalog of whisper.cpp speech models +
//! URL construction, so voice models can be downloaded IN-APP instead
//! of only via setup-voice.sh. Mirrors hf_hub's curated-GGUF approach;
//! the actual download reuses model_download::download_to_file (URL-
//! agnostic). PURE + dependency-free: catalog data + URL/recommend
//! logic; no IO here.
//!
//! Filenames match what voice.rs's loader expects per tier (quantized
//! q5_1 preferred). The default install is tiny-q + base-q (the dual-
//! model pipeline), ~88MB combined.

/// whisper.cpp model files live under this HF repo.
pub const WHISPER_URL_BASE: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// A curated whisper model the user can download in-app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WhisperModel {
    /// Filename on disk (matches voice.rs loader candidates).
    pub filename: &'static str,
    /// Short label for the UI.
    pub label: &'static str,
    /// Approximate on-disk size, MB (for the UI + budget hints).
    pub size_mb: u32,
    /// Part of the recommended default install set?
    pub in_default_set: bool,
    /// One-line description.
    pub blurb: &'static str,
}

/// The curated list, ordered smallest→largest. The loader prefers the
/// q5_1 (quantized) variants, so those are what we offer.
pub const CURATED: &[WhisperModel] = &[
    WhisperModel {
        filename: "ggml-tiny.en-q5_1.bin",
        label: "Tiny (English, quantized)",
        size_mb: 31,
        in_default_set: true,
        blurb: "Fastest; great for short commands. Part of the default pair.",
    },
    WhisperModel {
        filename: "ggml-base.en-q5_1.bin",
        label: "Base (English, quantized)",
        size_mb: 57,
        in_default_set: true,
        blurb: "More accurate; runs in parallel with Tiny. Part of the default pair.",
    },
    WhisperModel {
        filename: "ggml-small.en-q5_1.bin",
        label: "Small (English, quantized)",
        size_mb: 182,
        in_default_set: false,
        blurb: "Higher accuracy, noticeably slower on CPU.",
    },
    WhisperModel {
        filename: "ggml-large-v3-turbo-q5_0.bin",
        label: "Large v3 Turbo (quantized)",
        size_mb: 547,
        in_default_set: false,
        blurb: "Best quality; best with GPU acceleration.",
    },
];

/// Full download URL for a whisper model file.
pub fn url_for(filename: &str) -> String {
    format!("{}/{}", WHISPER_URL_BASE, filename)
}

/// Look up a curated model by filename.
pub fn find(filename: &str) -> Option<&'static WhisperModel> {
    CURATED.iter().find(|m| m.filename == filename)
}

/// The recommended default install set (tiny-q + base-q) — the dual-
/// model pipeline voice.rs runs in parallel.
pub fn default_set() -> Vec<&'static WhisperModel> {
    CURATED.iter().filter(|m| m.in_default_set).collect()
}

/// Total MB of the default set (for the UI's "~88MB" hint).
pub fn default_set_size_mb() -> u32 {
    default_set().iter().map(|m| m.size_mb).sum()
}

/// JSON for the QML voice-setup UI: an array of curated models.
pub fn catalog_json() -> String {
    let mut out = String::from("[");
    for (i, m) in CURATED.iter().enumerate() {
        if i > 0 { out.push(','); }
        out.push_str(&format!(
            "{{\"filename\":\"{}\",\"label\":\"{}\",\"size_mb\":{},\
             \"default\":{},\"blurb\":\"{}\"}}",
            m.filename, m.label, m.size_mb, m.in_default_set, m.blurb
        ));
    }
    out.push(']');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_construction_matches_whisper_cpp_repo() {
        assert_eq!(
            url_for("ggml-tiny.en-q5_1.bin"),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en-q5_1.bin"
        );
    }

    #[test]
    fn default_set_is_tiny_and_base_quantized() {
        let d = default_set();
        assert_eq!(d.len(), 2, "default = the dual-model pair");
        let names: Vec<&str> = d.iter().map(|m| m.filename).collect();
        assert!(names.contains(&"ggml-tiny.en-q5_1.bin"));
        assert!(names.contains(&"ggml-base.en-q5_1.bin"));
    }

    #[test]
    fn default_set_size_is_about_88mb() {
        // matches setup-voice.sh's "~88MB combined" claim
        let mb = default_set_size_mb();
        assert!((85..=95).contains(&mb), "default set ~88MB, got {}", mb);
    }

    #[test]
    fn filenames_match_loader_candidates() {
        // voice.rs loader prefers these exact quantized names.
        assert!(find("ggml-tiny.en-q5_1.bin").is_some());
        assert!(find("ggml-base.en-q5_1.bin").is_some());
        assert!(find("ggml-large-v3-turbo-q5_0.bin").is_some());
    }

    #[test]
    fn find_unknown_is_none() {
        assert!(find("ggml-nonexistent.bin").is_none());
    }

    #[test]
    fn catalog_json_well_formed_and_complete() {
        let j = catalog_json();
        assert!(j.starts_with('[') && j.ends_with(']'));
        // one object per curated model
        assert_eq!(j.matches("\"filename\"").count(), CURATED.len());
        // a known field/value present
        assert!(j.contains("ggml-tiny.en-q5_1.bin"));
        assert!(j.contains("\"default\":true"));
    }

    #[test]
    fn curated_ordered_smallest_first() {
        for w in CURATED.windows(2) {
            assert!(w[0].size_mb <= w[1].size_mb, "catalog ordered by size");
        }
    }
}
