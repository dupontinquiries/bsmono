//! mediagen.rs — build the exact `sd-cli` (stable-diffusion.cpp) command line for a generation.
//!
//! This encodes the invocation we validated against the official `docs/z_image.md`, retuned for this target
//! (Apple-silicon unified memory ⇒ no `--offload-to-cpu`; the model + vae + llm are the three resolved
//! companion files). It's a PURE argv builder — no spawning here — so the exact flags are unit-tested and the
//! actual `Command::spawn` (off the sandbox subprocess infra) is a thin, separate layer. Keeping the argv
//! pure means the command the app runs is the command the tests pin.

use std::path::PathBuf;

/// A resolved image generation for `sd-cli`. Values default (via [`ImageGen::z_image_turbo`]) to the
/// Z-Image-Turbo recipe: 8 steps, cfg 1.0, euler, flash-attention on.
#[derive(Clone, Debug)]
pub struct ImageGen {
    pub sd_cli: PathBuf,          // the built binary, e.g. build/bin/sd-cli
    pub diffusion_model: PathBuf, // ~/models/image/z-image-turbo-Q8_0.gguf
    pub vae: PathBuf,             // ~/models/vae/ae.safetensors
    pub llm: PathBuf,             // ~/models/text_encoders/Qwen3-4B-Instruct-2507-*.gguf
    pub prompt: String,
    pub steps: u32,
    pub cfg: f32,
    pub width: u32,
    pub height: u32,
    pub sampler: String,
    pub flash_attn: bool,
    pub verbose: bool,
    pub out: PathBuf,
    /// Pass-through for anything not modelled here — e.g. a seed (`-s 42`) for continuity-locking a shot's
    /// keyframes, or `--vae-tiling`. Appended verbatim, so it's the escape hatch for flags we don't pin.
    pub extra: Vec<String>,
}

impl ImageGen {
    /// The Z-Image-Turbo recipe with only the paths + prompt + output filled in. `models` is the `~/models`
    /// root; this assumes the ComfyUI-style layout (`image/`, `vae/`, `text_encoders/`) the resolver uses.
    pub fn z_image_turbo(sd_cli: PathBuf, models: &std::path::Path, diffusion_file: &str, llm_file: &str, prompt: impl Into<String>, out: PathBuf) -> Self {
        ImageGen {
            sd_cli,
            diffusion_model: models.join("image").join(diffusion_file),
            vae: models.join("vae").join("ae.safetensors"),
            llm: models.join("text_encoders").join(llm_file),
            prompt: prompt.into(),
            steps: 8,
            cfg: 1.0,
            width: 1024,
            height: 1024,
            sampler: "euler".to_string(),
            flash_attn: true,
            verbose: true,
            out,
            extra: Vec::new(),
        }
    }

    /// Fix the seed (adds `-s <seed>` via `extra`) — the continuity lever: the same seed across a shot's
    /// keyframes keeps a character/scene consistent frame to frame.
    pub fn with_seed(mut self, seed: i64) -> Self {
        self.extra.push("-s".to_string());
        self.extra.push(seed.to_string());
        self
    }

    pub fn from_image(mut self, init: &std::path::Path, strength: f32) -> Self {
        self.extra.push("-i".to_string());
        self.extra.push(init.to_string_lossy().into_owned());
        self.extra.push("--strength".to_string());
        self.extra.push(fmt_num(strength.clamp(0.0, 1.0)));
        self
    }

    /// The full command line, program first. Mirrors the validated z_image.md example; a short float like
    /// `1` is emitted for cfg 1.0 so it matches the docs exactly.
    pub fn argv(&self) -> Vec<String> {
        let p = |pb: &std::path::Path| pb.to_string_lossy().into_owned();
        let mut v = vec![
            p(&self.sd_cli),
            "--diffusion-model".into(), p(&self.diffusion_model),
            "--vae".into(), p(&self.vae),
            "--llm".into(), p(&self.llm),
            "-p".into(), self.prompt.clone(),
            "--cfg-scale".into(), fmt_num(self.cfg),
            "--sampling-method".into(), self.sampler.clone(),
            "--steps".into(), self.steps.to_string(),
            "-W".into(), self.width.to_string(),
            "-H".into(), self.height.to_string(),
        ];
        if self.flash_attn {
            v.push("--diffusion-fa".into());
        }
        if self.verbose {
            v.push("-v".into());
        }
        v.push("-o".into());
        v.push(p(&self.out));
        v.extend(self.extra.iter().cloned());
        v
    }
}

/// Format a float without a trailing `.0` (so `1.0` → `"1"`, `2.5` → `"2.5"`) to match the CLI examples.
fn fmt_num(x: f32) -> String {
    if x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        let s = format!("{x}");
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn z_image_turbo_argv_matches_the_validated_command() {
        let g = ImageGen::z_image_turbo(
            PathBuf::from("build/bin/sd-cli"),
            Path::new("/home/u/models"),
            "z-image-turbo-Q8_0.gguf",
            "Qwen3-4B-Instruct-2507-UD-Q6_K_XL.gguf",
            "a spiral galaxy over mountains",
            PathBuf::from("out.png"),
        );
        assert_eq!(
            g.argv(),
            vec![
                "build/bin/sd-cli",
                "--diffusion-model", "/home/u/models/image/z-image-turbo-Q8_0.gguf",
                "--vae", "/home/u/models/vae/ae.safetensors",
                "--llm", "/home/u/models/text_encoders/Qwen3-4B-Instruct-2507-UD-Q6_K_XL.gguf",
                "-p", "a spiral galaxy over mountains",
                "--cfg-scale", "1",
                "--sampling-method", "euler",
                "--steps", "8",
                "-W", "1024",
                "-H", "1024",
                "--diffusion-fa",
                "-v",
                "-o", "out.png",
            ]
        );
    }

    #[test]
    fn no_offload_flag_for_unified_memory() {
        let g = ImageGen::z_image_turbo(PathBuf::from("sd-cli"), Path::new("/m"), "d.gguf", "l.gguf", "x", PathBuf::from("o.png"));
        assert!(!g.argv().iter().any(|a| a == "--offload-to-cpu"), "unified memory: never offload");
    }

    #[test]
    fn seed_is_appended_for_continuity() {
        let g = ImageGen::z_image_turbo(PathBuf::from("sd-cli"), Path::new("/m"), "d.gguf", "l.gguf", "x", PathBuf::from("o.png")).with_seed(42);
        let a = g.argv();
        let i = a.iter().position(|s| s == "-s").expect("seed flag present");
        assert_eq!(a[i + 1], "42");
        // extras come after the output, not before it
        assert!(a.iter().position(|s| s == "-o").unwrap() < i);
    }

    #[test]
    fn flags_can_be_toggled() {
        let mut g = ImageGen::z_image_turbo(PathBuf::from("sd-cli"), Path::new("/m"), "d.gguf", "l.gguf", "x", PathBuf::from("o.png"));
        g.flash_attn = false;
        g.verbose = false;
        g.cfg = 2.5;
        let a = g.argv();
        assert!(!a.iter().any(|s| s == "--diffusion-fa"));
        assert!(!a.iter().any(|s| s == "-v"));
        let i = a.iter().position(|s| s == "--cfg-scale").unwrap();
        assert_eq!(a[i + 1], "2.5"); // fractional cfg keeps its decimal
    }
}
