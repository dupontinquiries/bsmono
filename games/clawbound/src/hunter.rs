//! Hunters — five archetypes with distinct senses, plus a compact micro-FSM
//! (adapted from early-fps `enemy.rs`: sense → transition → act, extended from
//! Idle/Chase/Search to Patrol/Alert/Track/Chase/Search for our stalk-and-scent
//! design). A hunter's "vision" is a CASCADING cone — wide awareness up close,
//! narrowing to a point far away — plus an omni radius (overhead sight for the
//! hawk, ground-vibration sense for the snake), and a downwind scent sense.

#[derive(Clone, Copy, PartialEq)]
pub enum HState { Patrol, Alert, Track, Chase, Search }

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind { Fox, Dog, Badger, Hawk, Snake, Shadow }

pub struct Params {
    pub name: &'static str,
    pub range: f32,      // forward reach of the cascading cone
    pub near_half: f32,  // half-width of the cone at the hunter (the "wide near")
    pub omni: f32,       // close/overhead/vibration radius (0 = none)
    pub smell: f32,      // downwind scent reach
    pub speed: f32,      // chase speed (units/s)
    pub aerial: bool,
    pub vibration: bool, // senses a MOVING cat within omni, through cover
}

impl Kind {
    pub fn params(self) -> Params {
        match self {
            Kind::Fox    => Params { name: "Fox",    range: 30.0, near_half: 8.0,  omni: 3.0,  smell: 40.0, speed: 17.0, aerial: false, vibration: false },
            Kind::Dog    => Params { name: "Dog",    range: 36.0, near_half: 8.5,  omni: 3.0,  smell: 52.0, speed: 20.0, aerial: false, vibration: false },
            Kind::Badger => Params { name: "Badger", range: 15.0, near_half: 12.0, omni: 4.0,  smell: 80.0, speed: 14.0, aerial: false, vibration: false },
            Kind::Hawk   => Params { name: "Hawk",   range: 58.0, near_half: 18.0, omni: 16.0, smell: 12.0, speed: 19.0, aerial: true,  vibration: false },
            Kind::Snake  => Params { name: "Snake",  range: 10.0, near_half: 5.0,  omni: 9.0,  smell: 34.0, speed: 11.0, aerial: false, vibration: true },
            // the dead dark-warrior: moderate senses, but it blinks across the map
            // and drags the afterlife over the world to strip away every hiding place.
            Kind::Shadow => Params { name: "Shadow", range: 26.0, near_half: 11.0, omni: 6.0,  smell: 30.0, speed: 15.5, aerial: false, vibration: false },
        }
    }
    pub fn is_shadow(self) -> bool { matches!(self, Kind::Shadow) }
}

/// Cascading-cone containment in the hunter's gaze frame (heading: +z at 0).
pub fn in_cone(p: &Params, hx: f32, hz: f32, heading: f32, cx: f32, cz: f32) -> bool {
    let (fx, fz) = (heading.sin(), heading.cos());   // forward
    let (rx, rz) = (heading.cos(), -heading.sin());  // right
    let dx = cx - hx; let dz = cz - hz;
    let fwd = dx * fx + dz * fz;      // distance along gaze
    let lat = dx * rx + dz * rz;      // lateral offset
    let dist = (dx * dx + dz * dz).sqrt();
    if p.omni > 0.0 && dist < p.omni { return true; }               // overhead / vibration / close
    fwd > 0.0 && fwd < p.range && lat.abs() < p.near_half * (1.0 - fwd / p.range)
}

/// Detected this tick? (sight/omni; scent handled separately for tracking.)
pub fn detects(k: Kind, hx: f32, hz: f32, heading: f32, cx: f32, cz: f32, cat_moving: bool, cat_hidden: bool) -> bool {
    let p = k.params();
    if p.vibration {
        let d = ((cx - hx).powi(2) + (cz - hz).powi(2)).sqrt();
        if d < p.omni && cat_moving { return true; }   // feels footfalls through cover
    }
    if cat_hidden && !p.aerial { return false; }        // crouched in a bush
    in_cone(&p, hx, hz, heading, cx, cz)
}

/// Micro-FSM transition (adapted from early-fps enemy sense→transition).
/// Seeing the quarry pins the hunter to at least Alert (and heads toward it);
/// it only relaxes toward Search/Patrol once it genuinely loses sight.
pub fn step(state: HState, suspicion: f32, has_scent: bool, seen: bool) -> HState {
    if seen {
        return if suspicion > 0.9 { HState::Chase } else { HState::Alert };
    }
    match state {
        HState::Chase => if suspicion < 0.3 { HState::Search } else { HState::Chase },
        HState::Alert => if suspicion < 0.15 { if has_scent { HState::Track } else { HState::Patrol } } else { HState::Alert },
        HState::Search => if has_scent { HState::Track } else if suspicion < 0.05 { HState::Patrol } else { HState::Search },
        HState::Track => if has_scent { HState::Track } else { HState::Search },
        HState::Patrol => if has_scent { HState::Track } else { HState::Patrol },
    }
}

pub fn state_color(s: HState) -> u32 {
    match s {
        HState::Chase => 0xff5040,
        HState::Alert | HState::Search => 0xffb04a,
        HState::Track => 0xb07adf,
        HState::Patrol => 0xffdf7a,
    }
}
