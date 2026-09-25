//! Persistent preferences for transcription: which whisper model, which
//! language handling. `$XDG_CONFIG_HOME/vidkit/prefs`, key=value lines.
//!
//! The default is the QUICK model over the whole video: tiny.en is cheap
//! enough to transcribe everything (a search like "goes down" then finds
//! every fight), and a bigger model is brought in when a search finds
//! nothing -- the palette offers it -- rather than paid for up front.

use std::path::{Path, PathBuf};

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelTier {
    /// tiny: fast enough for the whole video.
    Quick,
    /// base: better recall on names and fast speech.
    Better,
    /// small: slow on a laptop CPU; for when base misses.
    Best,
}

impl ModelTier {
    pub fn name(self) -> &'static str {
        match self {
            ModelTier::Quick => "tiny",
            ModelTier::Better => "base",
            ModelTier::Best => "small",
        }
    }
    pub fn bigger(self) -> Option<ModelTier> {
        match self {
            ModelTier::Quick => Some(ModelTier::Better),
            ModelTier::Better => Some(ModelTier::Best),
            ModelTier::Best => None,
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "tiny" | "quick" => ModelTier::Quick,
            "base" | "better" => ModelTier::Better,
            "small" | "best" => ModelTier::Best,
            _ => return None,
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LanguageMode {
    /// English-only models, told "en": no detection pass at all.
    English,
    /// A multilingual model detects on the first chunk and keeps that.
    DetectOnce,
    /// Detect on every chunk: videos that switch language. Costs an extra
    /// encoder pass per chunk.
    EveryChunk,
}

impl LanguageMode {
    pub fn name(self) -> &'static str {
        match self {
            LanguageMode::English => "english",
            LanguageMode::DetectOnce => "detect-once",
            LanguageMode::EveryChunk => "every-chunk",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "english" | "en" => LanguageMode::English,
            "detect-once" | "detect" => LanguageMode::DetectOnce,
            "every-chunk" | "multilingual" => LanguageMode::EveryChunk,
            _ => return None,
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub model: ModelTier,
    pub language: LanguageMode,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { model: ModelTier::Quick, language: LanguageMode::English }
    }
}

impl Prefs {
    pub fn to_text(&self) -> String {
        format!("transcribe_model={}\ntranscribe_language={}\n", self.model.name(), self.language.name())
    }

    /// Unknown keys and bad values are ignored (defaults kept): a prefs file
    /// from a newer version must not stop an older one starting.
    pub fn from_text(text: &str) -> Self {
        let mut p = Prefs::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k.trim() {
                "transcribe_model" => p.model = ModelTier::parse(v.trim()).unwrap_or(p.model),
                "transcribe_language" => p.language = LanguageMode::parse(v.trim()).unwrap_or(p.language),
                _ => {}
            }
        }
        p
    }

    pub fn path() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|d| d.join("vidkit/prefs"))
    }

    pub fn load() -> Self {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Self::from_text(&t)).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let tmp = p.with_extension("tmp");
            if std::fs::write(&tmp, self.to_text()).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }
}

/// Model file names for a tier, best first: quantized before plain (about
/// twice as fast on a CPU for the same weights), English-only first when the
/// language is English. Pure.
pub fn candidates(tier: ModelTier, language: LanguageMode) -> Vec<String> {
    let t = tier.name();
    let en = [format!("ggml-{t}.en-q5_1.bin"), format!("ggml-{t}.en-q8_0.bin"), format!("ggml-{t}.en.bin")];
    let multi = [format!("ggml-{t}-q5_1.bin"), format!("ggml-{t}-q8_0.bin"), format!("ggml-{t}.bin")];
    match language {
        LanguageMode::English => en.into_iter().chain(multi).collect(),
        _ => multi.into_iter().collect(),
    }
}

/// The model to use: the tier asked for if installed, else the nearest tier
/// that is (smaller first -- a missing small falls back to base, not to
/// nothing). Returns the path and the tier actually found.
pub fn find_model(dirs: &[PathBuf], tier: ModelTier, language: LanguageMode) -> Option<(PathBuf, ModelTier)> {
    let order: Vec<ModelTier> = match tier {
        ModelTier::Quick => vec![ModelTier::Quick, ModelTier::Better, ModelTier::Best],
        ModelTier::Better => vec![ModelTier::Better, ModelTier::Quick, ModelTier::Best],
        ModelTier::Best => vec![ModelTier::Best, ModelTier::Better, ModelTier::Quick],
    };
    for t in order {
        for name in candidates(t, language) {
            for d in dirs {
                let p = d.join(&name);
                if p.is_file() {
                    return Some((p, t));
                }
            }
        }
    }
    None
}

pub fn model_dirs() -> Vec<PathBuf> {
    let mut d = models_kit::search_dirs("whisper");
    if let Some(env) = std::env::var_os("VIDKIT_WHISPER_MODEL").map(PathBuf::from) {
        if let Some(parent) = env.parent().map(Path::to_path_buf) {
            d.insert(0, parent);
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_round_trip_and_survive_junk() {
        let p = Prefs { model: ModelTier::Best, language: LanguageMode::EveryChunk };
        assert_eq!(Prefs::from_text(&p.to_text()), p);
        let junk = Prefs::from_text("transcribe_model=gigantic\nfuture_key=1\ntranscribe_language=detect\n");
        assert_eq!(junk, Prefs { model: ModelTier::Quick, language: LanguageMode::DetectOnce });
        assert_eq!(Prefs::default().model, ModelTier::Quick, "tiny over the whole video by default");
    }

    #[test]
    fn quantized_english_models_come_first() {
        let c = candidates(ModelTier::Quick, LanguageMode::English);
        assert_eq!(c[0], "ggml-tiny.en-q5_1.bin");
        assert!(c.contains(&"ggml-tiny.bin".to_string()), "a multilingual model still works for English");
        assert!(candidates(ModelTier::Better, LanguageMode::EveryChunk).iter().all(|n| !n.contains(".en")), "multilingual only");
    }

    #[test]
    fn a_missing_tier_falls_back_to_the_nearest_installed() {
        let d = std::env::temp_dir().join(format!("vk-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("ggml-base.en.bin"), b"x").unwrap();
        let dirs = vec![d.clone()];
        let (p, t) = find_model(&dirs, ModelTier::Quick, LanguageMode::English).unwrap();
        assert_eq!((p.file_name().unwrap().to_str().unwrap(), t), ("ggml-base.en.bin", ModelTier::Better));
        std::fs::write(d.join("ggml-tiny.en-q5_1.bin"), b"x").unwrap();
        assert_eq!(find_model(&dirs, ModelTier::Quick, LanguageMode::English).unwrap().1, ModelTier::Quick);
        assert_eq!(find_model(&dirs, ModelTier::Best, LanguageMode::English).unwrap().1, ModelTier::Better, "no small: base, not tiny");
        assert!(find_model(&dirs, ModelTier::Quick, LanguageMode::EveryChunk).is_none(), "no multilingual file installed");
        let _ = std::fs::remove_dir_all(&d);
    }
}
