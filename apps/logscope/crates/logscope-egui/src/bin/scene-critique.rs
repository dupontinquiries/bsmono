//! scene-critique — the missing glue between the two existing pure builders (`mediagen` = text→image via
//! sd-cli/z-image-turbo, `vision_cli` = image→text via llama-mtmd-cli + a VLM's mmproj) and the headless
//! scene screenshots produced by `scene-view` (see scene_preview.rs). Both builders + their resolvers
//! already existed in `media.rs`, fully unit-tested, but nothing in the tree called them — this binary is
//! that caller, so the render → screenshot → describe → (optionally) generate-a-target loop is one command.
//!
//! WORKFLOW (run scene-view + Xvfb/import yourself to get a screenshot, then):
//!
//!   LOGSCOPE_MODELS=~/models \
//!   LOGSCOPE_MTMD_BIN=~/llama.cpp/build/bin/llama-mtmd-cli \
//!     scene-critique --describe /tmp/alley_shot.png
//!
//!   LOGSCOPE_MODELS=~/models \
//!   LOGSCOPE_SD_BIN=~/stable-diffusion.cpp/build/bin/sd-cli \
//!     scene-critique --generate "Montreal green alley, golden hour, ivy walls, wet cobblestone" \
//!                     --out /tmp/alley_target.png
//!
//!   # Side-by-side critique: describes both the current render and a reference/target image, then prints
//!   # a diff-oriented prompt ready to paste into the next scene-code editing pass.
//!   scene-critique --describe /tmp/alley_shot.png --ref /tmp/alley_target.png
//!
//! Model resolution (see media::resolve_vlm / resolve_diffusion / resolve_text_encoder):
//!   models/text/*.gguf + models/mmproj/*mmproj*.gguf  → the VLM pair (e.g. a Qwen2-VL or llava gguf)
//!   models/image/*.gguf                               → the diffusion checkpoint (e.g. z-image-turbo-Q8_0)
//!   models/text_encoders/*.gguf                        → the text encoder the diffusion model conditions on
//!
//! Nothing here downloads models — point LOGSCOPE_MODELS at wherever they already live locally (e.g. the
//! Odiora/Qwen3-4b-Z-Image-Turbo-AbliteratedV1-Q8_0-GGUF checkpoint under models/image/, plus any
//! llava-style vision gguf + its mmproj under models/text/ + models/mmproj/).

use logscope_egui::media;
use std::path::PathBuf;

fn env_path(key: &str, default: &str) -> PathBuf {
    std::env::var(key).map(PathBuf::from).unwrap_or_else(|_| shellexpand_home(default))
}

fn shellexpand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") { return PathBuf::from(home).join(rest); }
    }
    PathBuf::from(p)
}

fn usage() -> ! {
    eprintln!("usage:");
    eprintln!("  scene-critique --describe <shot.png> [--ref <target.png>] [--prompt \"...\"]");
    eprintln!("  scene-critique --generate \"<prompt>\" --out <target.png>");
    eprintln!();
    eprintln!("env: LOGSCOPE_MODELS (default ~/models), LOGSCOPE_MTMD_BIN (default llama-mtmd-cli),");
    eprintln!("     LOGSCOPE_SD_BIN (default sd-cli)");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() { usage(); }

    let models = env_path("LOGSCOPE_MODELS", "~/models");
    let mtmd_bin = env_path("LOGSCOPE_MTMD_BIN", "llama-mtmd-cli");
    let sd_bin = env_path("LOGSCOPE_SD_BIN", "sd-cli");

    let mut describe_path: Option<PathBuf> = None;
    let mut ref_path: Option<PathBuf> = None;
    let mut prompt = String::new();
    let mut gen_prompt: Option<String> = None;
    let mut out_path: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--describe" => { i += 1; describe_path = args.get(i).map(PathBuf::from); }
            "--ref" => { i += 1; ref_path = args.get(i).map(PathBuf::from); }
            "--prompt" => { i += 1; prompt = args.get(i).cloned().unwrap_or_default(); }
            "--generate" => { i += 1; gen_prompt = args.get(i).cloned(); }
            "--out" => { i += 1; out_path = args.get(i).map(PathBuf::from); }
            _ => usage(),
        }
        i += 1;
    }

    if let Some(gp) = gen_prompt {
        let out = out_path.unwrap_or_else(|| PathBuf::from("/tmp/scene-critique-gen.png"));
        match media::generate_image(sd_bin, &models, &gp, out.clone()) {
            Ok(p) => println!("wrote {}", p.display()),
            Err(e) => { eprintln!("generate failed: {e}"); std::process::exit(1); }
        }
        return;
    }

    let Some(shot) = describe_path else { usage() };
    let shot_desc = match media::describe_image(mtmd_bin.clone(), &models, shot.clone(), &prompt) {
        Ok(d) => d,
        Err(e) => { eprintln!("describe failed for {}: {e}", shot.display()); std::process::exit(1); }
    };
    println!("=== render ({}) ===\n{}\n", shot.display(), shot_desc);

    if let Some(rp) = ref_path {
        let ref_desc = match media::describe_image(mtmd_bin, &models, rp.clone(), &prompt) {
            Ok(d) => d,
            Err(e) => { eprintln!("describe failed for {}: {e}", rp.display()); std::process::exit(1); }
        };
        println!("=== reference ({}) ===\n{}\n", rp.display(), ref_desc);
        println!("=== critique prompt (paste into the next editing pass) ===");
        println!(
            "The current scene render is described as:\n\"{shot_desc}\"\n\nThe reference/target image is described as:\n\"{ref_desc}\"\n\nList the concrete visual elements present in the reference but missing (or wrong) in the render, ordered by how much each would change the read of the scene."
        );
    }
}
