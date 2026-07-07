//! vision_cli.rs — build the exact llama.cpp multimodal (`llama-mtmd-cli`) command line for IMAGE-TO-TEXT.
//!
//! The counterpart to `mediagen` (text→image via sd-cli): this describes an image through a vision-language
//! model — the model gguf plus its `--mmproj` vision projector (the CLIP-ViT tower that turns pixels into
//! the embedding slots `multimodal::plan_image` lays out). Like mediagen it's a PURE argv builder — no
//! spawning here — so the flags the app runs are the flags the tests pin; the actual `Command::spawn` and
//! stdout capture are a thin separate layer (the sandbox subprocess infra), and `extract_description`
//! recovers the model's answer from that stdout.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// A single image-to-text invocation. Defaults are a conservative describe-this-image caption at low
/// temperature (deterministic-ish) with the whole model on GPU.
#[derive(Clone, Debug)]
pub struct VisionCli {
    pub bin: PathBuf,      // the built binary, e.g. build/bin/llama-mtmd-cli
    pub model: PathBuf,    // the VLM gguf, e.g. ~/models/text/Qwen2-VL-7B-Instruct-Q4_K_M.gguf
    pub mmproj: PathBuf,   // the vision projector gguf, e.g. ~/models/mmproj/mmproj-Qwen2-VL-7B-f16.gguf
    pub image: PathBuf,    // the input image (png/jpg/webp)
    pub prompt: String,
    pub n_predict: u32,    // max tokens of description
    pub temp: f32,
    pub ngl: u32,          // GPU layers (99 = all, on unified-memory Apple silicon)
    pub extra: Vec<String>, // escape hatch for un-modelled flags (e.g. `--top-p`, a second `--image`)
}

impl VisionCli {
    /// Describe `image` with a VLM. `models` is the `~/models` root; assumes `text/` for the model and
    /// `mmproj/` for the projector (the resolver's layout, paralleling mediagen's `image/`+`vae/`).
    pub fn describe(bin: PathBuf, models: &Path, model_file: &str, mmproj_file: &str, image: PathBuf, prompt: impl Into<String>) -> Self {
        let prompt = prompt.into();
        VisionCli {
            bin,
            model: models.join("text").join(model_file),
            mmproj: models.join("mmproj").join(mmproj_file),
            image,
            prompt: if prompt.trim().is_empty() { "Describe this image in detail.".into() } else { prompt },
            n_predict: 256,
            temp: 0.1,
            ngl: 99,
            extra: Vec::new(),
        }
    }

    pub fn with_prompt(mut self, p: impl Into<String>) -> Self { self.prompt = p.into(); self }
    pub fn with_tokens(mut self, n: u32) -> Self { self.n_predict = n; self }
    pub fn with_temp(mut self, t: f32) -> Self { self.temp = t; self }
    /// Attach an additional image (multi-image compare/describe): appends another `--image`.
    pub fn and_image(mut self, img: &Path) -> Self {
        self.extra.push("--image".into());
        self.extra.push(img.to_string_lossy().into_owned());
        self
    }

    /// The full command line, program first. Mirrors the `llama-mtmd-cli` invocation:
    ///   llama-mtmd-cli -m MODEL --mmproj MMPROJ --image IMG -p PROMPT -n N --temp T -ngl NGL
    pub fn argv(&self) -> Vec<String> {
        let p = |pb: &Path| pb.to_string_lossy().into_owned();
        let mut v = vec![
            p(&self.bin),
            "-m".into(), p(&self.model),
            "--mmproj".into(), p(&self.mmproj),
            "--image".into(), p(&self.image),
            "-p".into(), self.prompt.clone(),
            "-n".into(), self.n_predict.to_string(),
            "--temp".into(), fmt_num(self.temp),
            "-ngl".into(), self.ngl.to_string(),
        ];
        v.extend(self.extra.iter().cloned());
        v
    }
}

/// Recover the model's description from `llama-mtmd-cli` stdout, stripping the tool's log noise. The CLI
/// prints load/timing lines (many prefixed with the encoded image, `llama_`, `clip_`, `main:`, `ggml_`,
/// `encoding image`, `decode:` …) around the actual generated text. We keep the lines that look like the
/// answer: non-empty, not a known log prefix, and past the "encoding image" marker if present.
pub fn extract_description(stdout: &str) -> String {
    let mut started = !stdout.contains("encoding image"); // if no marker, take everything non-log
    let mut out: Vec<&str> = Vec::new();
    for line in stdout.lines() {
        let t = line.trim();
        if !started {
            if t.contains("encoding image") || t.contains("image encoded") { started = true; }
            continue;
        }
        if t.is_empty() || is_log_line(t) { continue; }
        out.push(t);
    }
    out.join("\n").trim().to_string()
}

/// Heuristic: is this a tool/log line rather than model output?
fn is_log_line(t: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "llama_", "clip_", "main:", "ggml_", "build:", "system_info", "sampler", "generate:",
        "encode_image", "encoding image", "image encoded", "decode:", "print_info", "load:",
        "llm_load", "mtmd_", "warning:", "error:", "loading model", "n_predict", "eval time",
        "prompt eval", "total time", "srv ", "> ",
    ];
    PREFIXES.iter().any(|p| t.starts_with(p)) || (t.starts_with('[') && t.ends_with(']'))
}

fn fmt_num(x: f32) -> String {
    if x.fract() == 0.0 { format!("{}", x as i64) } else { format!("{x}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli() -> VisionCli {
        VisionCli::describe(
            PathBuf::from("build/bin/llama-mtmd-cli"),
            Path::new("/models"),
            "Qwen2-VL-7B-Instruct-Q4_K_M.gguf",
            "mmproj-Qwen2-VL-7B-f16.gguf",
            PathBuf::from("/tmp/shot.png"),
            "",
        )
    }

    #[test]
    fn argv_pins_the_mtmd_invocation() {
        let v = cli().argv();
        assert_eq!(v[0], "build/bin/llama-mtmd-cli");
        // model + projector + image are the three resolved companions
        let joined = v.join(" ");
        assert!(joined.contains("-m /models/text/Qwen2-VL-7B-Instruct-Q4_K_M.gguf"));
        assert!(joined.contains("--mmproj /models/mmproj/mmproj-Qwen2-VL-7B-f16.gguf"));
        assert!(joined.contains("--image /tmp/shot.png"));
        // default prompt filled when blank
        let pi = v.iter().position(|s| s == "-p").unwrap();
        assert_eq!(v[pi + 1], "Describe this image in detail.");
        // temp 0.1 formatted, defaults present
        assert!(joined.contains("--temp 0.1") && joined.contains("-n 256") && joined.contains("-ngl 99"));
    }

    #[test]
    fn builders_and_multi_image() {
        let v = cli().with_prompt("What breed is this dog?").with_tokens(64).with_temp(0.0)
            .and_image(Path::new("/tmp/b.png")).argv();
        let j = v.join(" ");
        assert!(j.contains("-p What breed is this dog?"));
        assert!(j.contains("-n 64"));
        assert!(j.contains("--temp 0")); // 0.0 → "0"
        // two images: the recipe one + the appended one
        assert_eq!(v.iter().filter(|s| *s == "--image").count(), 2);
        assert!(j.contains("--image /tmp/b.png"));
    }

    #[test]
    fn extract_strips_log_noise_after_marker() {
        let stdout = "\
main: loading model
clip_model_load: loaded meta data
encoding image 1/1
image encoded in 812 ms
A golden retriever sits on a wooden porch,
its head tilted toward the camera.
llama_print_timings: total time = 1893 ms
";
        let got = extract_description(stdout);
        assert_eq!(got, "A golden retriever sits on a wooden porch,\nits head tilted toward the camera.");
    }

    #[test]
    fn extract_without_marker_keeps_answer_lines() {
        let stdout = "llama_model_load: ok\nA red bicycle leans against a brick wall.\nllama_print_timings: 5ms";
        assert_eq!(extract_description(stdout), "A red bicycle leans against a brick wall.");
    }

    #[test]
    fn extract_empty_on_pure_noise() {
        assert_eq!(extract_description("main: start\nllama_load: ok\nclip_: done"), "");
    }
}
