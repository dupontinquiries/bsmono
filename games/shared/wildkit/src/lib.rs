//! wildkit — what an animal senses, and what the world does to its senses.
//!
//! Extracted verbatim from `unendlich-engine`. See `Cargo.toml` for why and
//! for what was deliberately left behind.
//!
//! ## What is in here, grouped by the question it answers
//!
//! **"Is anything there?"** — [`sim::sight_detect`], [`sim::hearing_radius`],
//! [`sim::creature_hearing`], [`sim::perception`]. Sight and hearing are
//! separate channels with separate ranges, which is the whole reason a
//! stalking animal is interesting: you can be heard before you are seen, or
//! seen before you are heard, and which happens depends on the ground and the
//! weather.
//!
//! **"Who was here, and how long ago?"** — [`sim::scent_strength`],
//! [`sim::scent_decay`], [`sim::scent_gradient`], [`sim::scent_track`]. Scent
//! is the channel with a *past tense*, and it is what makes a border a place
//! rather than a line.
//!
//! **"What is degrading those?"** — [`sim::wind_field`], [`sim::fog_model`],
//! [`sim::light_physics`], [`sim::audio_occlusion`]. Wind moves scent and
//! masks sound; fog and light set how far sight reaches. These are the knobs
//! that make the same terrain play differently at dawn and in rain.
//!
//! **"Where can I go, and what did it cost last time?"** —
//! [`sim::path_cost`], [`sim::path_memory`], [`sim::step_height`],
//! [`sim::neighbor_grid`].
//!
//! **"What is the ground doing?"** — [`sim::river_flow`],
//! [`sim::terrain_variation`].
//!
//! **"Do I chase or run?"** — [`sim::predator_prey`], [`sim::prey_scan`].
//!
//! ## Why this is a shared crate and not a copy
//!
//! clawbound-battleground wants cats that move like the ones in the Warriors
//! books — a patrol scented before it is seen, a stalk spoiled by a turn of
//! the wind, prey that bolts because a paw disturbed the ground. Every one of
//! those is a sensory question, and unendlich had already answered all of
//! them, headless and tested, for its own fauna.
//!
//! The alternative was writing a second set. That is what
//! `games/shared/creature-rig` was extracted to avoid, for the same reason,
//! and the reasoning has not changed: a fork of 3,400 actively-developed
//! lines is a liability the day after it is taken.
//!
//! ## What is NOT here
//!
//! No renderer, no ECS, no game. And specifically no **stamina**, no
//! **biome climate** and no **perception field** — those three reach back
//! into the engine (a service registry, a world module, a GPU benchmark) and
//! want a seam cut before they can travel. They are the obvious next
//! extraction and are named in `Cargo.toml` so nobody has to rediscover why
//! they were skipped.

pub mod sim;

pub use sim::{
    hearing_radius, path_cost, path_memory, perception, predator_prey, scent_decay,
    scent_gradient, scent_strength, scent_track, sight_detect, wind_field,
};
