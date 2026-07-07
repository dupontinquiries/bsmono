//! Player state and input-driven movement.

use crate::constants::{HORIZONTAL_ACCELERATION, JUMP_IMPULSE};
use crate::input::InputCache;
use crate::keybindings::{GameAction, KeyBindings};
use nalgebra_glm::{vec3, Vec3};
use std::time::Instant;

#[derive(Clone)]
pub struct Rotation {
    pub yaw: f32,
    pub pitch: f32,
}

impl Rotation {
    pub fn new() -> Self { Self { yaw: 0.0, pitch: 0.0 } }
    pub fn forward(&self) -> Vec3 {
        let (yaw_sin, yaw_cos) = self.yaw.sin_cos();
        let (pitch_sin, pitch_cos) = self.pitch.sin_cos();
        vec3(yaw_cos * pitch_cos, pitch_sin, yaw_sin * pitch_cos)
    }
}

pub struct PlayerState {
    pub rotation: Rotation,
    pub is_flying: bool,
    pub is_on_ground: bool,
    pub is_sneaking: bool,
    pub is_sprinting: bool,
    pub jump_last_executed: Instant,
}

impl PlayerState {
    pub fn new() -> Self {
        Self {
            rotation: Rotation::new(),
            is_flying: false,
            is_on_ground: false,
            is_sneaking: false,
            is_sprinting: false,
            jump_last_executed: Instant::now(),
        }
    }
}

pub struct PlayerPhysicsState {
    pub position: Vec3,
    pub velocity: Vec3,
    pub acceleration: Vec3,
}

impl PlayerPhysicsState {
    pub fn new_at_position(p: Vec3) -> Self {
        Self {
            position: p,
            velocity: Vec3::zeros(),
            acceleration: Vec3::zeros(),
        }
    }

    /// Apply input to the player's acceleration & velocity. Read keybindings via the
    /// supplied `kb` so controls stay configurable.
    pub fn apply_input(
        &mut self,
        state: &mut PlayerState,
        input: &InputCache,
        kb: &KeyBindings,
    ) {
        // Toggle flight (just pressed this frame).
        if let Some(toggle) = kb.get_key(GameAction::ToggleFlight) {
            if input.is_key_just_pressed(toggle) {
                state.is_flying = !state.is_flying;
            }
        }

        let rotation = &state.rotation;
        let forward = rotation.forward();
        let vec_bf = forward.cross(&Vec3::y_axis()).cross(&Vec3::y_axis());
        let vec_rl = forward.cross(&Vec3::y_axis());

        if state.is_flying {
            if kb.is_action_pressed(GameAction::FlyUp, input) {
                self.acceleration.y = 100.0;
            }
            if kb.is_action_pressed(GameAction::FlyDown, input) {
                self.acceleration.y = -100.0;
            }
        }

        if kb.is_action_pressed(GameAction::Jump, input) && state.is_on_ground {
            let now = Instant::now();
            if now.duration_since(state.jump_last_executed).as_secs_f32() >= 0.475 {
                self.velocity.y = JUMP_IMPULSE;
                state.jump_last_executed = now;
            }
        }

        let mut horiz = Vec3::zeros();
        if kb.is_action_pressed(GameAction::MoveForward, input) { horiz -= vec_bf; }
        if kb.is_action_pressed(GameAction::MoveBackward, input) { horiz += vec_bf; }
        if kb.is_action_pressed(GameAction::MoveLeft, input) { horiz -= vec_rl; }
        if kb.is_action_pressed(GameAction::MoveRight, input) { horiz += vec_rl; }

        if horiz.norm_squared() > 0.0 {
            let mut accel = horiz.normalize() * HORIZONTAL_ACCELERATION;
            if state.is_sprinting { accel *= 1.3; }
            else if state.is_sneaking { accel *= 0.3; }
            self.acceleration.x = accel.x;
            self.acceleration.z = accel.z;
        }
    }
}

/// Update sneak/sprint toggles. Call this once per frame before physics.
pub fn update_movement_modifiers(state: &mut PlayerState, input: &InputCache, kb: &KeyBindings) {
    if kb.is_action_pressed(GameAction::Sneak, input) && state.is_on_ground {
        state.is_sneaking = true;
        state.is_sprinting = false;
    } else {
        state.is_sneaking = false;
    }

    if kb.is_action_pressed(GameAction::Sprint, input)
        && kb.is_action_pressed(GameAction::MoveForward, input)
        && !state.is_sneaking
    {
        state.is_sprinting = true;
    } else if !kb.is_action_pressed(GameAction::MoveForward, input) {
        state.is_sprinting = false;
    }
}
