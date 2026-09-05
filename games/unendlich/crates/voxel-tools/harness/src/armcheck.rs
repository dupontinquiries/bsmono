//! Type-checks the Pending::Model arm lifted verbatim from
//! unendlich_fullgame.rs, so a scope or borrow error shows up here
//! instead of in the operator's build.
#![allow(dead_code)]
use crate::paintbrush as _;
use crate::paintbrush::Paintbrush;
use crate::stubs::ChunkManager;
use voxel_tools::focus::Gaze;

pub struct Eye { pub x: f32, pub y: f32, pub z: f32 }

pub fn model_arm(paintbrush: &mut Paintbrush, chunk_manager: &ChunkManager,
                 gaze: &Gaze, eye: Eye, prose: String) {
                            // `/build debug ...` always uses the fixture;
                            // anything else honours $UNENDLICH_BUILD_PROVIDER
                            // and falls back to the fixture so a first run
                            // never silently does nothing.
                            let debug = prose.trim_start().starts_with("debug");
                            match crate::paintbrush::find_provider(
                                "fixture.sh", !debug)
                            {
                                // A missing provider is reported and that is
                                // the end of the turn -- no sentinel flag, no
                                // empty argv threaded past the failure.
                                Err(e) => paintbrush.say(e),
                                Ok(argv) => {
                                    let (sys, user) = paintbrush
                                        .prompt_for(&chunk_manager, &gaze, None, &prose);
                                    // Blocking. A local 7B takes a second or
                                    // two and this stalls the frame; the
                                    // worker-thread version is the obvious
                                    // next step, but a stall you can see beats
                                    // a loop you cannot test.
                                    let mut prov =
                                        voxel_tools::llm::CommandProvider::new(argv);
                                    match voxel_tools::llm::Completer::complete(
                                        &mut prov, &sys, &user)
                                    {
                                        Ok(raw) => paintbrush.finish_turn(
                                            &chunk_manager, &gaze, &raw,
                                            Some((eye.x as i32, eye.y as i32, eye.z as i32))),
                                        Err(e) => paintbrush
                                            .say(format!("provider failed: {e}")),
                                    }
                                }
                            }
                        
}
