//! Every live-tunable number in one place: camera feel, arena physics
//! (gravity / bounce / friction / size), ball mutators, car setup, flip
//! rules and video. Edited from the in-game SETTINGS menus, persisted as
//! `cfg key value` lines in the save file, all multipliers of the Rocket
//! League baselines in physics.rs so 100% always means "stock RL feel".

pub struct CamCfg {
    pub fov_deg: f32,   // 55..110, RL default ~76
    pub dist: f32,      // chase distance
    pub height: f32,    // camera height above the car
    pub angle_deg: f32, // boom pitch in degrees, RL-style (negative = down)
    pub stiffness: f32, // smoothing speed multiplier
    pub shake: f32,     // impact shake multiplier
    pub grav_follow: f32, // 0 = classic Y-up, 1 = horizon follows local gravity
}

pub struct ArenaCfg {
    pub gravity: f32,  // 0..2.5 x
    pub bounce: f32,   // surface restitution x (stacks with ball bounce)
    pub friction: f32, // surface friction x (ball roll + tire grip baseline)
    pub scale: f32,    // playfield spread; applied when the map (re)generates
}

pub struct BallCfg {
    pub size: f32,     // radius x
    pub bounce: f32,   // restitution x
    pub friction: f32, // rolling friction x
    pub weight: f32,   // mass x (touch impulse feel)
}

/// One-touch graphics tiers. AUTO follows the detected hardware; the rest
/// are explicit so a phone can be forced to LOW from the menu.
pub const GFX_TIERS: [&str; 4] = ["AUTO", "LOW", "MEDIUM", "HIGH"];

pub fn apply_gfx_tier(cfg: &mut Config, tier: u8) {
    match tier {
        1 => {
            cfg.video.rays = false;
            cfg.video.quality = 0.30;
            cfg.video.render_scale = 0.5;
            cfg.video.bloom = cfg.video.bloom.min(0.35);
            cfg.video.screen_door = 0.0;
            cfg.video.vsync = true;
        }
        2 => {
            cfg.video.rays = true;
            cfg.video.quality = 0.55;
            cfg.video.render_scale = 0.75;
            cfg.video.bloom = 0.55;
        }
        3 => {
            cfg.video.rays = true;
            cfg.video.quality = 1.0;
            cfg.video.render_scale = 1.0;
            cfg.video.bloom = 0.7;
        }
        _ => {}
    }
    cfg.video.tier = tier;
}

#[derive(Clone, Copy)]
pub struct GyroCfg {
    pub on: bool,
    pub sens: f32,
    pub deadzone: f32,
    pub smooth: f32,
    pub invert_x: bool,
    pub invert_y: bool,
    pub air_mode: u8, // 0 = roll+pitch (flick aim), 1 = yaw+pitch
    pub gate: u8,     // 0 = always, 1 = while boosting, 2 = airborne only
}

pub struct CarCfg {
    pub steer_rate: f32,   // 0.5..1.5 x ground steering yaw rate
    pub input_ease: f32,   // 0.5..1.5 x temporal buildup on steer/roll
    pub size: f32,        // whole-car scale
    pub grip: f32,        // tire grip x
    pub top_speed: f32,   // speed cap x (supersonic follows)
    pub accel: f32,       // drive acceleration x
    pub boost: f32,       // boost thrust x
    pub boost_regen: f32, // passive refill, units per second (0 = RL rules)
    pub boost_max: f32,   // tank size x (100 = stock)
    pub unlimited_boost: bool,
    pub jump: f32,        // jump strength x
    pub analog: bool,     // wooting analog keys feed the movement axes
    pub analog_soft: f32, // wind-in depth: 0 = digital, 0.45 = default feather
}

pub struct FlipCfg {
    pub spin: f32, // 0.5..1.5 x dodge rotation speed
    pub strength: f32,  // dodge impulse + spin x
    pub resets: bool,   // underside contact restores your flip
    pub strict: bool,   // resets need clean wheel-side alignment
    pub unlimited: bool,// mutator: the flip never runs out
    pub cancel: bool,   // holding opposite pitch cancels the flip rotation
    pub land_assist: f32, // wheels-down magnetism while falling near ground
    pub air_roll_default: bool, // steer ROLLS in the air by default; holding
                                // SLIDE swaps to the other one (RL's classic
                                // powerslide-doubles-as-air-roll scheme)
}

pub struct TouchCfg {
    pub enabled: bool,
    pub scale: f32, // on-screen control size multiplier
    pub tap_to_move: bool,
}

pub struct VideoCfg {
    pub rays: bool,     // ray-traced shadows / AO / reflections
    pub quality: f32,   // ray step budget x
    pub filmic: bool,   // ACES filmic tonemap
    pub exposure: f32,  // 0.6..1.6
    pub vignette: f32,  // 0..1
    pub grain: f32,     // 0..1 film grain
    pub letterbox: bool,// cinema bars
    pub bloom: f32,     // bloom strength, 0..2
    pub vsync: bool,
    pub screen_door: f32,
    /// 0 AUTO, 1 LOW, 2 MEDIUM, 3 HIGH
    pub tier: u8,
    pub render_scale: f32, // scene resolution factor (0.5 = half res) // subtle checker mask, 0..0.35
}

pub struct Config {
    pub gyro: GyroCfg,
    pub cam: CamCfg,
    pub arena: ArenaCfg,
    pub ball: BallCfg,
    pub car: CarCfg,
    pub flip: FlipCfg,
    pub video: VideoCfg,
    pub preset: usize, // last mutator preset picked (display only)
    pub touch: TouchCfg,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            gyro: GyroCfg { on: false, sens: 1.0, deadzone: 0.06, smooth: 0.05, invert_x: false, invert_y: false, air_mode: 0, gate: 0 },
            cam: CamCfg { fov_deg: 86.0, dist: 4.9, height: 5.0, angle_deg: -30.0, stiffness: 1.0, shake: 0.6, grav_follow: 1.0 },
            arena: ArenaCfg { gravity: 1.0, bounce: 1.0, friction: 1.0, scale: 1.0 },
            ball: BallCfg { size: 1.0, bounce: 1.0, friction: 1.0, weight: 1.0 },
            car: CarCfg { steer_rate: 0.9, input_ease: 1.1, size: 1.0, grip: 1.0, top_speed: 1.0, accel: 1.0, boost: 1.0, boost_regen: 0.0, boost_max: 1.0, unlimited_boost: false, jump: 1.0, analog: true, analog_soft: 0.45 },
            flip: FlipCfg { spin: 1.0, strength: 1.0, resets: true, strict: true, unlimited: false, cancel: true, land_assist: 0.0, air_roll_default: false },
            video: VideoCfg { render_scale: 1.0, rays: true, quality: 1.0, filmic: true, exposure: 1.0, vignette: 0.32, grain: 0.12, letterbox: false, bloom: 0.7, vsync: true, screen_door: 0.05, tier: 0 },
            touch: TouchCfg { enabled: false, scale: 1.0, tap_to_move: true },
            preset: 0,
        }
    }
}

pub const PRESETS: [&str; 6] = ["CUSTOM", "STOCK RL", "MOON BALL", "PINBALL", "BEACH DAY", "HEAVYWEIGHT"];

impl Config {
    /// Mutator presets touch only arena + ball numbers; car setup is yours.
    pub fn apply_preset(&mut self, idx: usize) {
        self.preset = idx % PRESETS.len();
        let (g, ab, bs, bb, bf, bw) = match self.preset {
            1 => (1.0, 1.0, 1.0, 1.0, 1.0, 1.0),       // stock
            2 => (0.35, 1.1, 1.0, 1.15, 0.8, 0.8),     // moon
            3 => (1.0, 1.3, 0.55, 1.6, 0.4, 0.7),      // pinball
            4 => (0.8, 1.0, 2.2, 1.2, 1.0, 0.35),      // beach day
            5 => (1.0, 0.9, 1.25, 0.6, 1.4, 2.5),      // heavyweight
            _ => return,                                // CUSTOM: leave as-is
        };
        self.arena.gravity = g;
        self.arena.bounce = ab;
        self.ball.size = bs;
        self.ball.bounce = bb;
        self.ball.friction = bf;
        self.ball.weight = bw;
    }

    pub fn save_lines(&self, out: &mut String) {
        use std::fmt::Write as _;
        let mut w = |k: &str, v: f32| {
            let _ = writeln!(out, "cfg {k} {v:.3}");
        };
        w("cam_fov", self.cam.fov_deg);
        w("cam_dist", self.cam.dist);
        w("cam_height", self.cam.height);
        w("cam_angle", self.cam.angle_deg);
        w("cam_shake", self.cam.shake);
        w("cam_grav", self.cam.grav_follow);
        w("cam_stiff", self.cam.stiffness);
        w("gravity", self.arena.gravity);
        w("arena_bounce", self.arena.bounce);
        w("arena_friction", self.arena.friction);
        w("arena_scale", self.arena.scale);
        w("ball_size", self.ball.size);
        w("ball_bounce", self.ball.bounce);
        w("ball_friction", self.ball.friction);
        w("ball_weight", self.ball.weight);
        w("car_size", self.car.size);
        w("car_grip", self.car.grip);
        w("car_top", self.car.top_speed);
        w("car_accel", self.car.accel);
        w("car_boost", self.car.boost);
        w("car_boost_regen", self.car.boost_regen);
        w("car_boost_max", self.car.boost_max);
        w("car_boost_unl", self.car.unlimited_boost as u8 as f32);
        w("car_jump", self.car.jump);
        w("car_analog", self.car.analog as u8 as f32);
        w("car_analog_soft", self.car.analog_soft);
        w("flip_strength", self.flip.strength);
        w("flip_resets", self.flip.resets as u8 as f32);
        w("flip_strict", self.flip.strict as u8 as f32);
        w("flip_unlimited", self.flip.unlimited as u8 as f32);
        w("flip_cancel", self.flip.cancel as u8 as f32);
        w("land_assist", self.flip.land_assist);
        w("flip_spin", self.flip.spin);
        w("gyro_on", if self.gyro.on { 1.0 } else { 0.0 });
        w("gyro_sens", self.gyro.sens);
        w("gyro_dz", self.gyro.deadzone);
        w("gyro_smooth", self.gyro.smooth);
        w("gyro_ix", if self.gyro.invert_x { 1.0 } else { 0.0 });
        w("gyro_iy", if self.gyro.invert_y { 1.0 } else { 0.0 });
        w("gyro_air", self.gyro.air_mode as f32);
        w("gyro_gate", self.gyro.gate as f32);
        w("steer_rate", self.car.steer_rate);
        w("input_ease", self.car.input_ease);
        w("air_roll_default", self.flip.air_roll_default as u8 as f32);
        w("rays", self.video.rays as u8 as f32);
        w("gfx_tier", self.video.tier as f32);
        w("ray_quality", self.video.quality);
        w("filmic", self.video.filmic as u8 as f32);
        w("exposure", self.video.exposure);
        w("vignette", self.video.vignette);
        w("grain", self.video.grain);
        w("letterbox", self.video.letterbox as u8 as f32);
        w("bloom", self.video.bloom);
        w("vsync", self.video.vsync as u8 as f32);
        w("screen_door", self.video.screen_door);
        w("render_scale", self.video.render_scale);
        w("touch_enabled", if self.touch.enabled { 1.0 } else { 0.0 });
        w("touch_scale", self.touch.scale);
        w("preset", self.preset as f32);
    }

    pub fn load_line(&mut self, key: &str, val: &str) {
        let Ok(v) = val.trim().parse::<f32>() else { return };
        let b = v > 0.5;
        match key {
            "cam_fov" => self.cam.fov_deg = v.clamp(50.0, 215.0),
            "cam_dist" => self.cam.dist = v.clamp(2.5, 9.0),
            "cam_height" => self.cam.height = v.clamp(0.4, 9.2),
            "cam_angle" => self.cam.angle_deg = v.clamp(-25.0, 25.0),
            // "cam_angle" => self.cam.angle_deg = v.clamp(-15.0, 15.0),
            "cam_shake" => self.cam.shake = v.clamp(0.0, 1.5),
            "cam_grav" => self.cam.grav_follow = v.clamp(0.0, 1.0),
            "cam_stiff" => self.cam.stiffness = v.clamp(0.15, 3.2),
            "gravity" => self.arena.gravity = v.clamp(0.0, 2.6),
            "arena_bounce" => self.arena.bounce = v.clamp(0.2, 2.0),
            "arena_friction" => self.arena.friction = v.clamp(0.1, 2.5),
            "arena_scale" => self.arena.scale = v.clamp(0.55, 1.6),
            "ball_size" => self.ball.size = v.clamp(0.35, 3.0),
            "ball_bounce" => self.ball.bounce = v.clamp(0.2, 2.0),
            "ball_friction" => self.ball.friction = v.clamp(0.0, 2.5),
            "ball_weight" => self.ball.weight = v.clamp(0.25, 9.0),
            "car_size" => self.car.size = v.clamp(0.55, 2.0),
            "car_grip" => self.car.grip = v.clamp(0.3, 2.5),
            "car_top" => self.car.top_speed = v.clamp(0.5, 1.9),
            "car_accel" => self.car.accel = v.clamp(0.4, 2.6),
            "car_boost" => self.car.boost = v.clamp(0.4, 20.0),
            "car_boost_regen" => self.car.boost_regen = v.clamp(0.0, 50.0),
            "car_boost_max" => self.car.boost_max = v.clamp(0.3, 2.5),
            "car_boost_unl" => self.car.unlimited_boost = b,
            "car_jump" => self.car.jump = v.clamp(0.5, 2.2),
            "car_analog" => self.car.analog = b,
            "car_analog_soft" => self.car.analog_soft = v.clamp(0.0, 0.8),
            "flip_strength" => self.flip.strength = v.clamp(0.4, 2.2),
            "flip_resets" => self.flip.resets = b,
            "flip_strict" => self.flip.strict = b,
            "flip_unlimited" => self.flip.unlimited = b,
            "flip_cancel" => self.flip.cancel = b,
            "land_assist" => self.flip.land_assist = v.clamp(0.0, 1.5),
            "flip_spin" => self.flip.spin = v.clamp(0.5, 1.5),
            "gyro_on" => self.gyro.on = v > 0.5,
            "gyro_sens" => self.gyro.sens = v.clamp(0.1, 4.0),
            "gyro_dz" => self.gyro.deadzone = v.clamp(0.0, 0.4),
            "gyro_smooth" => self.gyro.smooth = v.clamp(0.008, 0.3),
            "gyro_ix" => self.gyro.invert_x = v > 0.5,
            "gyro_iy" => self.gyro.invert_y = v > 0.5,
            "gyro_air" => self.gyro.air_mode = v as u8 & 1,
            "gyro_gate" => self.gyro.gate = (v as u8).min(2),
            "render_scale" => self.video.render_scale = v.clamp(0.25, 1.0),
            "touch_enabled" => self.touch.enabled = v > 0.5,
            "touch_scale" => self.touch.scale = v.clamp(0.6, 1.8),
            "steer_rate" => self.car.steer_rate = v.clamp(0.5, 1.5),
            "input_ease" => self.car.input_ease = v.clamp(0.5, 1.5),
            "air_roll_default" => self.flip.air_roll_default = b,
            "rays" => self.video.rays = b,
            "gfx_tier" => self.video.tier = (v as u8).min(3),
            "ray_quality" => self.video.quality = v.clamp(0.4, 1.8),
            "filmic" => self.video.filmic = b,
            "exposure" => self.video.exposure = v.clamp(0.5, 1.8),
            "vignette" => self.video.vignette = v.clamp(0.0, 1.0),
            "grain" => self.video.grain = v.clamp(0.0, 1.0),
            "letterbox" => self.video.letterbox = b,
            "bloom" => self.video.bloom = v.clamp(0.0, 2.0),
            "vsync" => self.video.vsync = b,
            "screen_door" => self.video.screen_door = v.clamp(0.0, 0.35),
            "preset" => self.preset = (v as usize) % PRESETS.len(),
            _ => {}
        }
    }
}
