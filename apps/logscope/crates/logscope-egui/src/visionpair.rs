//! visionpair — matching a vision projector to the model it belongs to.
//!
//! `model_kind` already recognises an `mmproj` and refuses to load it standalone: *"it pairs
//! with a vision-language model as its --mmproj companion"*. Nothing does the pairing, so the
//! guidance ends at "load the VLM and point --mmproj at this" and the user is left to work out
//! which VLM, from a directory of similarly-named files.
//!
//! This is the pairing, and the argv that follows from it — the image-IN half of the media
//! pipeline, mirroring `mediagen`'s design for image-out: a PURE builder, so the flags are
//! unit-tested and the command the app runs is the command the tests pin.
//!
//! ## Why the pairing needs care
//!
//! A projector is trained WITH its model. `mmproj-Qwen2.5-VL-7B` fed to Gemma3 does not error
//! — the tensor shapes may even line up — it produces confident nonsense about the image,
//! which is the worst failure available here because nothing in the output says the pairing
//! was wrong. So a mismatch must be refused rather than guessed, and a guess that "looks
//! close" is exactly what must not happen.
//!
//! Matching is on the model STEM shared by the two filenames, with the quantisation and the
//! `mmproj` marker stripped, because that is what the publishers actually vary:
//!
//! ```text
//!   Qwen2.5-VL-7B-Instruct-Q4_K_M.gguf   ← the model
//!   mmproj-Qwen2.5-VL-7B-Instruct-f16.gguf ← its projector
//! ```

use std::path::{Path, PathBuf};

/// Does this filename look like a vision projector?
///
/// Name-based on purpose: this runs over a DIRECTORY LISTING, before anything is opened.
/// Opening every gguf in a models folder to ask `model_kind` costs a header read per file and
/// several seconds on a phone, for a question the naming convention answers.
///
/// `model_kind::classify` remains the authority once a file is actually loaded — this is a
/// filter for building a candidate list, not a second opinion about what a file is.
pub fn looks_like_projector(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".gguf") && (n.starts_with("mmproj") || n.contains("mmproj") || n.contains("-vision-"))
}

/// The stem two filenames must share to be a pair.
///
/// Strips the extension, the `mmproj` prefix/marker, and a trailing quantisation or precision
/// tag — the parts that differ BETWEEN a model and its own projector, which are published at
/// different precisions (the model quantised, the projector usually f16).
pub fn pair_stem(name: &str) -> String {
    let mut s = name.to_ascii_lowercase();
    if let Some(i) = s.rfind(".gguf") {
        s.truncate(i);
    }
    for marker in ["mmproj-", "mmproj_", "mmproj."] {
        if let Some(rest) = s.strip_prefix(marker) {
            s = rest.to_string();
        }
    }
    s = s.replace("-mmproj", "").replace("_mmproj", "").replace("mmproj", "");
    // Trailing precision/quantisation tags, longest first so `q4_k_m` is not left as `_k_m`.
    const TAGS: &[&str] = &[
        "q2_k", "q3_k_s", "q3_k_m", "q3_k_l", "q4_k_s", "q4_k_m", "q5_k_s", "q5_k_m", "q6_k",
        "q4_0", "q4_1", "q5_0", "q5_1", "q8_0", "iq4_xs", "iq4_nl", "bf16", "f16", "f32",
    ];
    loop {
        let before = s.len();
        let t = s.trim_end_matches(['-', '_', '.']).to_string();
        s = t;
        for tag in TAGS {
            if let Some(rest) = s.strip_suffix(tag) {
                if rest.ends_with(['-', '_', '.']) || rest.is_empty() {
                    s = rest.to_string();
                    break;
                }
            }
        }
        if s.len() == before {
            break;
        }
    }
    s.trim_matches(['-', '_', '.']).to_string()
}

/// A model and the projector that belongs to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub model: PathBuf,
    pub mmproj: PathBuf,
}

/// Find the projector for `model` among `files`.
///
/// `None` when nothing matches, and `None` when SEVERAL do. Two projectors with the same stem
/// means the directory holds something this cannot disambiguate — and picking one would be a
/// coin toss whose wrong outcome is confident nonsense about the image rather than an error.
pub fn projector_for(model: &Path, files: &[PathBuf]) -> Option<PathBuf> {
    let want = pair_stem(&model.file_name()?.to_string_lossy());
    if want.is_empty() {
        return None;
    }
    let mut hits: Vec<&PathBuf> = files
        .iter()
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            looks_like_projector(&n) && pair_stem(&n) == want
        })
        .collect();
    hits.dedup();
    match hits.len() {
        1 => Some(hits[0].clone()),
        _ => None,
    }
}

/// Every model in `files` that has a projector beside it.
pub fn pairs(files: &[PathBuf]) -> Vec<Pair> {
    files
        .iter()
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            n.to_ascii_lowercase().ends_with(".gguf") && !looks_like_projector(&n)
        })
        .filter_map(|m| projector_for(m, files).map(|mmproj| Pair { model: m.clone(), mmproj }))
        .collect()
}

/// A vision run: a paired model, an image, and a prompt about it.
#[derive(Debug, Clone)]
pub struct VisionRun {
    pub bin: PathBuf,
    pub pair: Pair,
    pub image: PathBuf,
    pub prompt: String,
    /// Context length. A single image is worth hundreds of tokens, so the default here is
    /// larger than a text-only run would need — a 2048 context with an image in it leaves
    /// almost nothing for the answer.
    pub n_ctx: u32,
    pub n_predict: u32,
    pub temperature: f32,
}

impl VisionRun {
    pub fn new(bin: PathBuf, pair: Pair, image: PathBuf, prompt: &str) -> Self {
        VisionRun {
            bin,
            pair,
            image,
            prompt: prompt.to_string(),
            n_ctx: 4096,
            n_predict: 512,
            temperature: 0.2,
        }
    }

    /// The exact command line, pure — no spawning here, exactly as `mediagen` does it for
    /// image-out, so the flags are what the tests pin.
    pub fn argv(&self) -> Vec<String> {
        vec![
            self.bin.display().to_string(),
            "-m".into(),
            self.pair.model.display().to_string(),
            "--mmproj".into(),
            self.pair.mmproj.display().to_string(),
            "--image".into(),
            self.image.display().to_string(),
            "-c".into(),
            self.n_ctx.to_string(),
            "-n".into(),
            self.n_predict.to_string(),
            "--temp".into(),
            format!("{:.2}", self.temperature),
            // Last, and after `--`-free flags, because the prompt is arbitrary user text: a
            // flag added after it would be swallowed as part of it.
            "-p".into(),
            self.prompt.clone(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from("/models").join(s)
    }

    /// The published shape: the model quantised, the projector at f16, same stem.
    #[test]
    fn a_model_finds_the_projector_published_with_it() {
        let files = vec![
            p("Qwen2.5-VL-7B-Instruct-Q4_K_M.gguf"),
            p("mmproj-Qwen2.5-VL-7B-Instruct-f16.gguf"),
            p("Llama-3.1-8B-Q4_K_M.gguf"),
        ];
        let got = projector_for(&files[0], &files);
        assert_eq!(got, Some(files[1].clone()));

        // A text-only model has none, and must not borrow someone else's.
        assert_eq!(projector_for(&files[2], &files), None);
    }

    /// A projector is trained WITH its model. Feeding Qwen's to Gemma does not error — it
    /// produces confident nonsense about the image, which is the worst failure here because
    /// nothing in the output says the pairing was wrong.
    #[test]
    fn a_projector_is_never_matched_to_a_different_model() {
        let files = vec![
            p("gemma-3-12b-it-Q4_K_M.gguf"),
            p("mmproj-Qwen2.5-VL-7B-Instruct-f16.gguf"),
        ];
        assert_eq!(projector_for(&files[0], &files), None, "a mismatched projector was offered");
    }

    /// Two candidates is not a reason to pick one. The wrong outcome of that coin toss is
    /// confident nonsense rather than an error.
    #[test]
    fn an_ambiguous_directory_pairs_nothing() {
        let files = vec![
            p("Qwen2.5-VL-7B-Instruct-Q4_K_M.gguf"),
            p("mmproj-Qwen2.5-VL-7B-Instruct-f16.gguf"),
            p("mmproj-Qwen2.5-VL-7B-Instruct-Q8_0.gguf"),
        ];
        assert_eq!(projector_for(&files[0], &files), None, "it guessed between two projectors");
    }

    /// The quantisation tag is what differs between a model and its own projector, so it has
    /// to come off — and `q4_k_m` must come off whole rather than leaving `_k_m`.
    #[test]
    fn quantisation_and_the_mmproj_marker_are_stripped_whole() {
        assert_eq!(pair_stem("Qwen2.5-VL-7B-Q4_K_M.gguf"), "qwen2.5-vl-7b");
        assert_eq!(pair_stem("mmproj-Qwen2.5-VL-7B-f16.gguf"), "qwen2.5-vl-7b");
        assert_eq!(pair_stem("Qwen2.5-VL-7B-mmproj-f16.gguf"), "qwen2.5-vl-7b");
        assert_eq!(pair_stem("model-q8_0.gguf"), "model");
        assert_eq!(pair_stem("model-bf16.gguf"), "model");
        // A name that is only a tag does not become empty-and-matching-everything.
        assert_eq!(projector_for(&p("f16.gguf"), &[p("mmproj-f16.gguf")]), None);
    }

    /// `mediagen` builds image-out's argv purely so the tests pin the command; this is the
    /// same contract for image-in.
    #[test]
    fn the_command_line_is_exact_and_the_prompt_comes_last() {
        let pair = Pair { model: p("m.gguf"), mmproj: p("mmproj-m.gguf") };
        let run = VisionRun::new(PathBuf::from("/usr/bin/llama-mtmd-cli"), pair, p("shot.png"), "what is this?");
        let a = run.argv();
        assert_eq!(a[0], "/usr/bin/llama-mtmd-cli");
        for flag in ["-m", "--mmproj", "--image", "-c", "-n", "--temp", "-p"] {
            assert!(a.iter().any(|x| x == flag), "missing {flag} in {a:?}");
        }
        // The prompt is arbitrary user text: anything added after it would be swallowed into
        // it, so it goes last and its flag immediately precedes it.
        assert_eq!(a[a.len() - 2], "-p");
        assert_eq!(a[a.len() - 1], "what is this?");

        // A single image is worth hundreds of tokens; 2048 would leave almost nothing for the
        // answer.
        assert!(run.n_ctx >= 4096, "{}", run.n_ctx);
    }

    #[test]
    fn a_directory_lists_only_the_models_that_can_actually_see() {
        let files = vec![
            p("Qwen2.5-VL-7B-Q4_K_M.gguf"),
            p("mmproj-Qwen2.5-VL-7B-f16.gguf"),
            p("Llama-3.1-8B-Q4_K_M.gguf"),
            p("notes.txt"),
        ];
        let ps = pairs(&files);
        assert_eq!(ps.len(), 1, "{ps:?}");
        assert_eq!(ps[0].model, files[0]);
        // The projector itself is not offered as a model to run.
        assert!(!ps.iter().any(|x| looks_like_projector(&x.model.file_name().unwrap().to_string_lossy())));
    }
}
