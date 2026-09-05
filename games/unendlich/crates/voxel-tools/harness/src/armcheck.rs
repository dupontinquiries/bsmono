#![allow(dead_code)]
mod log {
    macro_rules! info { ($($t:tt)*) => { eprintln!($($t)*) } }
    macro_rules! warn_ { ($($t:tt)*) => { eprintln!($($t)*) } }
    pub(crate) use info;
    pub(crate) use warn_ as warn;
}
use crate::paintbrush::Paintbrush;
use crate::stubs::ChunkManager;
use voxel_tools::focus::Gaze;
pub struct Eye { pub x: f32, pub y: f32, pub z: f32 }
pub fn model_arm(paintbrush: &mut Paintbrush, chunk_manager: &ChunkManager,
                 gaze: &Gaze, eye: Eye, prose: String) {
                            // Provider selection lives in voxel_chat, so the
                            // frame loop does not grow a second copy of it.
                            let debug = prose.trim_start().starts_with("debug");
                            let (sys, user) =
                                paintbrush.prompt_for(&chunk_manager, &gaze, None, &prose);
                            let completer = paintbrush.completer_for(debug);
                            if completer.is_none() {
                                // Every None path logs its own reason now, but
                                // say it once more here so the chat shows a
                                // turn ended rather than appearing to hang.
                                paintbrush.say("no provider available -- /provider to check");
                            }
                            if let Some(mut c) = completer {
                                // Blocking on the frame thread. In-instance on
                                // the GPU this is fast; a cold script reload is
                                // not, and either way say so first.
                                log::info!("[paintbrush] asking {} ({} + {} chars)",
                                           c.name(), sys.len(), user.len());
                                paintbrush.say(format!("asking {} ...", c.name()));
                                let t0 = std::time::Instant::now();
                                match c.complete(&sys, &user) {
                                    Ok(raw) => {
                                        log::info!("[paintbrush] reply in {:.1}s, {} chars",
                                                   t0.elapsed().as_secs_f32(), raw.len());
                                        paintbrush.finish_turn(
                                            &chunk_manager, &gaze, &raw,
                                            Some((eye.x as i32, eye.y as i32, eye.z as i32)));
                                    }
                                    Err(e) => {
                                        log::warn!("[paintbrush] provider failed: {e}");
                                        paintbrush.say(format!("provider failed: {e}"));
                                    }
                                }
                            }
                        
}
