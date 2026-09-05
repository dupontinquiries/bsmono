//! Type-checks the Pending::Model arm lifted verbatim from the bin.
#![allow(dead_code)]

// The bin has the real `log` crate; the harness substitutes a shim so
// the arm can be lifted verbatim.
mod log {
    macro_rules! info { ($($t:tt)*) => { eprintln!($($t)*) } }
    pub(crate) use info;
}
use crate::paintbrush::Paintbrush;
use crate::stubs::ChunkManager;
use voxel_tools::focus::Gaze;
pub struct Eye { pub x: f32, pub y: f32, pub z: f32 }
pub fn model_arm(paintbrush: &mut Paintbrush, chunk_manager: &ChunkManager,
                 gaze: &Gaze, eye: Eye, prose: String) {
                            // `/build debug` always uses the fixture. Anything
                            // else uses whatever /provider selected, which
                            // defaults to $UNENDLICH_BUILD_HTTP or the fixture.
                            // The choice is ANNOUNCED: a silent fallback to the
                            // fixture once looked exactly like a working model.
                            use crate::paintbrush::ProviderChoice;
                            let debug = prose.trim_start().starts_with("debug");
                            let choice = if debug {
                                ProviderChoice::Fixture
                            } else {
                                paintbrush.provider.clone()
                            };
                            log::info!("[paintbrush] provider choice: {}", choice.describe());

                            let (sys, user) =
                                paintbrush.prompt_for(&chunk_manager, &gaze, None, &prose);

                            // Build the completer for this turn. HTTP is
                            // in-process; a script is spawned per request.
                            let mut completer: Option<Box<dyn voxel_tools::llm::Completer>> =
                                match &choice {
                                    ProviderChoice::Http(cfg) => Some(Box::new(
                                        voxel_tools::llm::HttpProvider::new(cfg.clone()))),
                                    ProviderChoice::Fixture | ProviderChoice::Script(_) => {
                                        let script = match &choice {
                                            ProviderChoice::Script(a) if !a.is_empty() =>
                                                Ok(a.clone()),
                                            ProviderChoice::Script(_) =>
                                                crate::paintbrush::find_provider(
                                                    "llama-cli.sh", true),
                                            _ => crate::paintbrush::find_provider(
                                                    "fixture.sh", !debug),
                                        };
                                        match script {
                                            Ok(argv) => Some(Box::new(
                                                voxel_tools::llm::CommandProvider::new(argv))),
                                            Err(e) => { paintbrush.say(e); None }
                                        }
                                    }
                                };

                            if let Some(c) = completer.as_mut() {
                                // Blocking on the frame thread either way; an
                                // in-process request to a warm server is fast,
                                // a cold script reload is not.
                                paintbrush.say(format!(
                                    "asking {} ... (the window freezes until it answers)",
                                    c.name()));
                                match c.complete(&sys, &user) {
                                    Ok(raw) => paintbrush.finish_turn(
                                        &chunk_manager, &gaze, &raw,
                                        Some((eye.x as i32, eye.y as i32, eye.z as i32))),
                                    Err(e) => paintbrush.say(format!("provider failed: {e}")),
                                }
                            }
                        
}
