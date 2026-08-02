//! Progress + settings persistence: a tiny human-readable `skydribble.save`
//! file of `key value` lines next to the binary's working directory. Parsed
//! leniently — a missing or mangled file just means defaults. Custom key
//! bindings ride along as `bind <action> <spec,spec,...>` lines.

use std::fmt::Write as _;
use std::path::PathBuf;

use crate::config::Config;
use crate::input::Bindings;

#[derive(Clone)]
pub struct Save {
    pub total_rings: u64,
    pub total_car_rings: u64,
    pub total_goals: u64,
    pub best_combo: u32,
    pub best_air: f32,      // longest air-dribble in seconds
    pub top_speed: f32,
    pub volume: f32,
    pub music: bool,
    pub minimap: bool,
    pub ball_cam: bool,
    pub fullscreen: bool,
    pub car: usize,
    pub map: usize,
    pub seed: u64,
    pub name: String,       // multiplayer display name / profile id
    /// which hardware profile these settings were auto-tuned for, so a save
    /// carried onto a phone (or off one) re-tunes exactly once
    pub tuned_profile: u8,
    /// best time per trial, 0 = unfinished
    pub trials: String,
}

impl Default for Save {
    fn default() -> Save {
        Save {
            total_rings: 0,
            total_car_rings: 0,
            total_goals: 0,
            best_combo: 0,
            best_air: 0.0,
            top_speed: 0.0,
            volume: 0.8,
            music: true,
            minimap: true,
            ball_cam: true,
            fullscreen: false,
            car: 0,
            map: 0,
            seed: 20260726,
            name: String::new(),
            tuned_profile: 255,
            trials: String::new(),
        }
    }
}

pub fn path() -> PathBuf {
    PathBuf::from("skydribble.save")
}

/// Does a save already exist? (first run gets platform defaults applied)
pub fn exists() -> bool {
    path().exists()
}

pub fn load() -> (Save, Bindings, Config) {
    let mut s = Save::default();
    let mut b = Bindings::classic();
    let mut cfg = Config::default();
    let Ok(text) = std::fs::read_to_string(path()) else { return (s, b, cfg) };
    let mut saw_bind = false;
    for line in text.lines() {
        let Some((k, v)) = line.split_once(' ') else { continue };
        let v = v.trim();
        match k {
            "tuned_profile" => s.tuned_profile = v.parse().unwrap_or(s.tuned_profile),
            "trials" => s.trials = v.to_string(),
            "total_rings" => s.total_rings = v.parse().unwrap_or(s.total_rings),
            "total_car_rings" => s.total_car_rings = v.parse().unwrap_or(s.total_car_rings),
            "total_goals" => s.total_goals = v.parse().unwrap_or(s.total_goals),
            "best_combo" => s.best_combo = v.parse().unwrap_or(s.best_combo),
            "best_air" => s.best_air = v.parse().unwrap_or(s.best_air),
            "top_speed" => s.top_speed = v.parse().unwrap_or(s.top_speed),
            "volume" => s.volume = v.parse::<f32>().unwrap_or(s.volume).clamp(0.0, 1.0),
            "music" => s.music = v == "1",
            "minimap" => s.minimap = v == "1",
            "ball_cam" => s.ball_cam = v == "1",
            "fullscreen" => s.fullscreen = v == "1",
            "car" => s.car = v.parse::<usize>().unwrap_or(s.car).min(8),
            "map" => s.map = v.parse::<usize>().unwrap_or(s.map) % crate::world::STYLE_COUNT,
            "seed" => s.seed = v.parse().unwrap_or(s.seed),
            "name" => s.name = v.to_string(),
            "cfg" => {
                if let Some((ck, cv)) = v.split_once(' ') {
                    cfg.load_line(ck, cv);
                }
            }
            "bind" => {
                if !saw_bind {
                    // first bind line: start from an empty table so cleared
                    // actions stay cleared instead of inheriting defaults
                    for (_, binds) in b.map.iter_mut() {
                        binds.clear();
                    }
                    saw_bind = true;
                }
                if let Some((action, specs)) = v.split_once(' ') {
                    b.load_line(action, specs.trim());
                } else {
                    b.load_line(v, "");
                }
            }
            _ => {}
        }
    }
    (s, b, cfg)
}

pub fn store(s: &Save, b: &Bindings, cfg: &Config) {
    let mut out = String::with_capacity(1024);
    let _ = writeln!(out, "tuned_profile {}", s.tuned_profile);
    let _ = writeln!(out, "trials {}", s.trials);
    let _ = writeln!(out, "total_rings {}", s.total_rings);
    let _ = writeln!(out, "total_car_rings {}", s.total_car_rings);
    let _ = writeln!(out, "total_goals {}", s.total_goals);
    let _ = writeln!(out, "best_combo {}", s.best_combo);
    let _ = writeln!(out, "best_air {:.2}", s.best_air);
    let _ = writeln!(out, "top_speed {:.2}", s.top_speed);
    let _ = writeln!(out, "volume {:.2}", s.volume);
    let _ = writeln!(out, "music {}", s.music as u8);
    let _ = writeln!(out, "minimap {}", s.minimap as u8);
    let _ = writeln!(out, "ball_cam {}", s.ball_cam as u8);
    let _ = writeln!(out, "fullscreen {}", s.fullscreen as u8);
    let _ = writeln!(out, "car {}", s.car);
    let _ = writeln!(out, "map {}", s.map);
    let _ = writeln!(out, "seed {}", s.seed);
    if !s.name.is_empty() {
        let _ = writeln!(out, "name {}", s.name);
    }
    b.save_lines(&mut out);
    cfg.save_lines(&mut out);
    if let Err(e) = std::fs::write(path(), out) {
        eprintln!("save: couldn't write {:?}: {e}", path());
    }
}
