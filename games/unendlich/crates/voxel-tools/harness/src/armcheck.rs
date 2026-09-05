//! Type-checks the Pending::Model arm lifted verbatim from the bin.
#![allow(dead_code)]
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
                            if let Some(mut c) = paintbrush.completer_for(debug) {
                                // Blocking on the frame thread. In-instance on
                                // the GPU this is fast; a cold script reload is
                                // not, and either way say so first.
                                paintbrush.say(format!("asking {} ...", c.name()));
                                match c.complete(&sys, &user) {
                                    Ok(raw) => paintbrush.finish_turn(
                                        &chunk_manager, &gaze, &raw,
                                        Some((eye.x as i32, eye.y as i32, eye.z as i32))),
                                    Err(e) => paintbrush.say(format!("provider failed: {e}")),
                                }
                            }
                        
}
