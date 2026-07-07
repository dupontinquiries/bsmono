//! media.rs — the thin executable layer under the pure media builders (`vision_cli`, `tts_cli`, `mediagen`).
//!
//! The builders pin the argv; this resolves which model files to use and actually spawns the process. The
//! RESOLVERS are pure (they take a directory listing, not the filesystem) so the "which gguf pairs with
//! which mmproj" logic is unit-tested; the spawn/capture is a thin std::process layer kept deliberately
//! small. Nothing here touches the !Send LLM backend — these are external processes, spawnable off any
//! thread.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// From a listing of a models `text/` (or flat) dir, pick a vision-language model and its `mmproj` vision
/// projector. mmproj files carry "mmproj" in the name; the model is any other `.gguf`. When several models
/// exist, prefer the one whose base name best overlaps the mmproj's (Qwen2-VL ↔ mmproj-Qwen2-VL).
pub fn resolve_vlm(files: &[String]) -> Option<(String, String)> {
    let mmproj = files.iter().find(|f| f.to_ascii_lowercase().contains("mmproj") && f.ends_with(".gguf"))?;
    let stem_words = |s: &str| s.to_ascii_lowercase().replace("mmproj", "")
        .split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| w.len() >= 2).map(|w| w.to_string()).collect::<Vec<_>>();
    let mm_words = stem_words(mmproj);
    let mut best: Option<(&String, usize)> = None;
    for f in files {
        if std::ptr::eq(f, mmproj) || !f.ends_with(".gguf") || f.to_ascii_lowercase().contains("mmproj") { continue; }
        let overlap = { let fw = stem_words(f); fw.iter().filter(|w| mm_words.contains(w)).count() };
        if best.map(|(_, o)| overlap > o).unwrap_or(true) { best = Some((f, overlap)); }
    }
    best.map(|(model, _)| (model.clone(), mmproj.clone()))
}

/// From a listing of a `voices/` dir, pick a Piper voice (`.onnx`, ignoring the `.onnx.json` config).
pub fn resolve_voice(files: &[String]) -> Option<String> {
    files.iter().find(|f| f.ends_with(".onnx")).cloned()
}

/// From a listing of an `image/` dir, pick the diffusion checkpoint (a `.gguf`/`.safetensors`, preferring a
/// name that looks like a diffusion model — z-image/flux/sd — over any stray file).
pub fn resolve_diffusion(files: &[String]) -> Option<String> {
    let is_model = |f: &str| f.ends_with(".gguf") || f.ends_with(".safetensors");
    let looks_diffusion = |f: &str| { let l = f.to_ascii_lowercase(); ["z-image", "zimage", "flux", "sd", "sdxl", "diffusion", "turbo"].iter().any(|k| l.contains(k)) };
    files.iter().find(|f| is_model(f) && looks_diffusion(f)).cloned()
        .or_else(|| files.iter().find(|f| is_model(f)).cloned())
}

/// From a `text_encoders/` listing, pick the LLM text encoder (`.gguf`) the diffusion model conditions on.
pub fn resolve_text_encoder(files: &[String]) -> Option<String> {
    files.iter().find(|f| f.ends_with(".gguf")).cloned()
}

/// List the file names (not paths) directly in `dir`. Convenience for the resolvers; returns empty on error.
pub fn list_dir(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Spawn `argv` (program first), optionally writing `stdin` to it, and capture stdout. Thin; errors carry
/// the tool's stderr tail so failures are legible. Not unit-tested (needs the binary) — the argv it runs is
/// pinned by the builder tests.
pub fn run_capture(argv: &[String], stdin: Option<&str>) -> Result<String, String> {
    use std::process::{Command, Stdio};
    use std::io::Write;
    let (prog, rest) = argv.split_first().ok_or("empty argv")?;
    let mut child = Command::new(prog).args(rest)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("spawn {prog}: {e}"))?;
    if let (Some(text), Some(si)) = (stdin, child.stdin.take()) {
        let mut si = si;
        let _ = si.write_all(text.as_bytes());
        // dropping `si` closes stdin so the tool sees EOF
    }
    let out = child.wait_with_output().map_err(|e| format!("wait {prog}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let tail: String = String::from_utf8_lossy(&out.stderr).lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        Err(format!("{prog} exited {}: {tail}", out.status.code().unwrap_or(-1)))
    }
}

/// Resolve the VLM under `models/text` (falling back to `models`), build the image→text command, run it,
/// and return the extracted description. `bin` is the `llama-mtmd-cli` binary.
pub fn describe_image(bin: PathBuf, models: &Path, image: PathBuf, prompt: &str) -> Result<String, String> {
    let text_dir = models.join("text");
    let scan_dir = if text_dir.is_dir() { text_dir } else { models.to_path_buf() };
    let files = list_dir(&scan_dir);
    let (model, mmproj) = resolve_vlm(&files).ok_or("no VLM + mmproj pair found under models/text")?;
    // resolve_vlm returns bare names under scan_dir; but VisionCli::describe joins models/text + models/mmproj.
    // Build the argv directly against the resolved files to honor whatever layout they were found in.
    let cli = crate::vision_cli::VisionCli {
        bin,
        model: scan_dir.join(&model),
        mmproj: scan_dir.join(&mmproj),
        image,
        prompt: if prompt.trim().is_empty() { "Describe this image in detail.".into() } else { prompt.to_string() },
        n_predict: 256, temp: 0.1, ngl: 99, extra: Vec::new(),
    };
    let raw = run_capture(&cli.argv(), None)?;
    Ok(crate::vision_cli::extract_description(&raw))
}

/// Resolve a Piper voice under `models/voices`, synthesize `text` to `out` (wav). `bin` is `piper`.
pub fn say(bin: PathBuf, models: &Path, text: &str, out: PathBuf) -> Result<PathBuf, String> {
    let files = list_dir(&models.join("voices"));
    let voice = resolve_voice(&files).ok_or("no Piper voice (.onnx) found under models/voices")?;
    let tts = crate::tts_cli::Tts::speak(bin, models, &voice, text, out.clone());
    run_capture(&tts.argv(), Some(&tts.stdin_text()))?;
    Ok(out)
}

/// Text→image: resolve the diffusion checkpoint (models/image) + its text encoder (models/text_encoders),
/// build the z-image sd-cli command, run it, and return the written PNG. `sd_cli` is the stable-diffusion.cpp
/// binary. This is the path a diffusion gguf (which the LLM loader correctly refuses) actually runs through.
pub fn generate_image(sd_cli: PathBuf, models: &Path, prompt: &str, out: PathBuf) -> Result<PathBuf, String> {
    let diffusion = resolve_diffusion(&list_dir(&models.join("image")))
        .ok_or("no diffusion model found under models/image")?;
    let llm = resolve_text_encoder(&list_dir(&models.join("text_encoders")))
        .ok_or("no text encoder (.gguf) found under models/text_encoders")?;
    let gen = crate::mediagen::ImageGen::z_image_turbo(sd_cli, models, &diffusion, &llm, prompt, out.clone());
    run_capture(&gen.argv(), None)?;
    Ok(out)
}

/// Image→image: seed the diffusion from `init` (edit / variation / restyle) instead of noise. `strength` in
/// [0,1] is how far to travel from the init. Same resolution as `generate_image`, plus `-i init --strength`.
pub fn edit_image(sd_cli: PathBuf, models: &Path, init: &Path, prompt: &str, strength: f32, out: PathBuf) -> Result<PathBuf, String> {
    let diffusion = resolve_diffusion(&list_dir(&models.join("image")))
        .ok_or("no diffusion model found under models/image")?;
    let llm = resolve_text_encoder(&list_dir(&models.join("text_encoders")))
        .ok_or("no text encoder (.gguf) found under models/text_encoders")?;
    let gen = crate::mediagen::ImageGen::z_image_turbo(sd_cli, models, &diffusion, &llm, prompt, out.clone())
        .from_image(init, strength);
    run_capture(&gen.argv(), None)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_vlm_pairs_model_with_its_mmproj() {
        let files = vec![
            "Qwen2-VL-7B-Instruct-Q4_K_M.gguf".to_string(),
            "mmproj-Qwen2-VL-7B-f16.gguf".to_string(),
            "some-text-only-model-Q4.gguf".to_string(),
        ];
        let (model, mmproj) = resolve_vlm(&files).unwrap();
        assert_eq!(mmproj, "mmproj-Qwen2-VL-7B-f16.gguf");
        assert_eq!(model, "Qwen2-VL-7B-Instruct-Q4_K_M.gguf", "picked the model whose name overlaps the projector");
    }

    #[test]
    fn resolve_vlm_single_model() {
        let files = vec!["llava-v1.6-7b.Q4.gguf".to_string(), "mmproj-llava-v1.6-7b.gguf".to_string()];
        assert_eq!(resolve_vlm(&files), Some(("llava-v1.6-7b.Q4.gguf".into(), "mmproj-llava-v1.6-7b.gguf".into())));
    }

    #[test]
    fn resolve_vlm_none_without_mmproj() {
        assert_eq!(resolve_vlm(&["plain-llm-Q4.gguf".to_string()]), None, "no mmproj ⇒ not a VLM setup");
        assert_eq!(resolve_vlm(&[]), None);
    }

    #[test]
    fn resolve_voice_picks_onnx_not_json() {
        let files = vec!["en_US-amy-medium.onnx.json".to_string(), "en_US-amy-medium.onnx".to_string()];
        assert_eq!(resolve_voice(&files), Some("en_US-amy-medium.onnx".to_string()));
        assert_eq!(resolve_voice(&["only-config.onnx.json".to_string()]), None, ".onnx.json config is not a voice");
    }

    #[test]
    fn resolve_diffusion_prefers_a_diffusion_named_model() {
        let files = vec!["z-image-turbo-Q8_0.gguf".to_string(), "notes.txt".to_string()];
        assert_eq!(resolve_diffusion(&files), Some("z-image-turbo-Q8_0.gguf".into()));
        // falls back to any model file when nothing is obviously diffusion-named
        assert_eq!(resolve_diffusion(&["model.safetensors".to_string()]), Some("model.safetensors".into()));
        assert_eq!(resolve_diffusion(&["readme.md".to_string()]), None);
    }

    #[test]
    fn resolve_text_encoder_picks_gguf() {
        assert_eq!(resolve_text_encoder(&["Qwen3-4B-Instruct.gguf".to_string(), "x.txt".to_string()]), Some("Qwen3-4B-Instruct.gguf".into()));
        assert_eq!(resolve_text_encoder(&["ae.safetensors".to_string()]), None);
    }
}
