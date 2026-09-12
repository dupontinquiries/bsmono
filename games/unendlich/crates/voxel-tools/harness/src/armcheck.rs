#![allow(dead_code)]
use crate::paintbrush::Paintbrush;
use crate::stubs::ChunkManager;
use voxel_tools::focus::Gaze;
pub struct Eye { pub x: f32, pub y: f32, pub z: f32 }
pub fn model_arm(paintbrush: &mut Paintbrush, chunk_manager: &ChunkManager,
                 gaze: &Gaze, eye: Eye, prose: String) {
                            // Off the frame thread. `start_turn` returns at
                            // once; `poll_turn` below applies the reply when
                            // the worker has one, so a 22-second generation no
                            // longer freezes the window.
                            paintbrush.start_turn(
                                &chunk_manager, &gaze, None, &prose,
                                Some((eye.x as i32, eye.y as i32, eye.z as i32)));
                        
}
