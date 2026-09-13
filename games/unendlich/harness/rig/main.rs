//! harness/rig — the animation, physics, brain and combat cores run outside
//! the engine, on the REAL files, and dumped as JSON for the three.js
//! viewer (`viewer.html`).
//!
//! Build with plain rustc (seconds, no cargo, no engine compile):
//!
//!     harness/rig/run.sh            # builds, runs, writes rig_viewer.html
//!
//! Every module below is `#[path]`-included from crates/unendlich-engine/src.
//! Nothing here is a copy of engine logic except three tiny functions that
//! live in crate-coupled files (each is marked `mirrors:` with the engine
//! function it duplicates, so a change there is a change here):
//!
//!   * `advance_phase`     mirrors pose::advance_gait_phase   (pose.rs needs crate::creature)
//!   * `sample_body_local` mirrors ragdoll_bridge::sample_body_local (needs crate::creature)
//!   * `part_color`        mirrors world::entity::part_color     (entity.rs is the vertex pass)
//!
//! What the JSON holds (see `viewer.html` for how it's read):
//!   items      — every ItemClass: hand boxes + icon quads
//!   creatures  — the rig fixtures (spawn.rs's fauna and dragon anatomies)
//!   sequences  — posed frames (8 corners per box, body-local) for walk, turn,
//!                hurt, land, strike, settle, fly, death (ragdoll)
//!   melee      — two fighters exchanging blows; strike01 drives the rig
//!   brain      — behavior_arbiter choices over scripted drives
#![allow(dead_code)]

#[path = "../../crates/unendlich-engine/src/item_model.rs"]
mod item_model;
#[path = "../../crates/unendlich-engine/src/anim_rig.rs"]
mod anim_rig;
mod sim;
mod util;

use anim_rig::{MotionInput, PosedBox, RagdollSample, RigPart, Role};
use sim::liveliness::MotionState;
use std::f32::consts::TAU;
use std::fmt::Write;

// ── tiny mirrors of crate-coupled engine functions ────────────────────

/// mirrors: pose::advance_gait_phase (CADENCE = 2.2 rad per block).
const CADENCE: f32 = 2.2;
fn advance_phase(phase: f32, speed: f32, dt: f32) -> f32 {
    (phase + CADENCE * speed * dt).rem_euclid(TAU)
}

/// mirrors: sim::ragdoll_bridge::sample_body_local.
fn sample_body_local(nodes: &[sim::ragdoll::Node], body_pos: [f32; 3], yaw: f32) -> ([f32; 3], Vec<[f32; 3]>) {
    let (s, c) = yaw.sin_cos();
    let local = |p: [f32; 3]| {
        let (dx, dy, dz) = (p[0] - body_pos[0], p[1] - body_pos[1], p[2] - body_pos[2]);
        [dx * c + dz * s, dy, -dx * s + dz * c]
    };
    let torso = nodes.first().map(|n| local(n.pos)).unwrap_or([0.0; 3]);
    (torso, nodes.iter().skip(1).map(|n| local(n.pos)).collect())
}

/// mirrors: world::entity::part_color + CREATURE_BODY_COLOR, by rig role.
fn part_color(role: Option<Role>) -> [f32; 3] {
    match role {
        None => [0.50, 0.45, 0.40],
        Some(Role::Head) => [0.80, 0.75, 0.62],
        Some(Role::Wing) => [0.62, 0.66, 0.72],
        Some(Role::Leg) => [0.30, 0.26, 0.22],
        Some(Role::Arm) => [0.42, 0.32, 0.26],
        Some(Role::Fin) => [0.40, 0.52, 0.62],
        Some(Role::Tail) => [0.46, 0.41, 0.36],
        Some(Role::Neck) => [0.50, 0.45, 0.40],
        Some(Role::Other) => [0.5, 0.5, 0.5],
    }
}

// ── fixtures (the anatomies spawn.rs builds, by the numbers) ──────────

struct Fixture {
    name: &'static str,
    torso: [f32; 3],
    parts: Vec<RigPart>,
    mass: f32,
}

/// spawn.rs `CreatureKind::Fauna` anatomy (eyes, jaw, four feet, tail).
fn fauna() -> Fixture {
    let p = |role, offset, half| RigPart { role, offset, half };
    Fixture {
        name: "fauna",
        torso: [0.55, 0.32, 0.30],
        parts: vec![
            p(Role::Head, [0.5, 0.35, 0.18], [0.08, 0.08, 0.08]),
            p(Role::Head, [0.5, 0.35, -0.18], [0.08, 0.08, 0.08]),
            p(Role::Head, [0.6, 0.1, 0.0], [0.16, 0.12, 0.16]),
            p(Role::Leg, [0.45, -0.5, 0.25], [0.12, 0.3, 0.12]),
            p(Role::Leg, [0.45, -0.5, -0.25], [0.12, 0.3, 0.12]),
            p(Role::Leg, [-0.45, -0.5, 0.25], [0.12, 0.3, 0.12]),
            p(Role::Leg, [-0.45, -0.5, -0.25], [0.12, 0.3, 0.12]),
            p(Role::Tail, [-0.7, 0.1, 0.0], [0.4, 0.12, 0.12]),
        ],
        mass: 2.0,
    }
}

/// spawn.rs `dragon_anatomy` at genome midpoints (every gene 0.5).
fn dragon() -> Fixture {
    let p = |role, offset, half| RigPart { role, offset, half };
    let head_x = 3.25;
    Fixture {
        name: "dragon",
        torso: [2.6, 1.1, 1.3],
        parts: vec![
            p(Role::Head, [head_x, 0.0, 0.0], [0.85, 0.65, 0.75]),
            p(Role::Head, [head_x - 0.2, 0.65, 0.62], [0.2, 0.2, 0.2]),
            p(Role::Head, [head_x - 0.2, 0.65, -0.62], [0.2, 0.2, 0.2]),
            p(Role::Wing, [0.0, 0.8, 2.6], [2.6, 0.2, 1.5]),
            p(Role::Wing, [0.0, 0.8, -2.6], [2.6, 0.2, 1.5]),
            p(Role::Leg, [1.0, -1.35, 1.05], [0.5, 0.675, 0.5]),
            p(Role::Leg, [1.0, -1.35, -1.05], [0.5, 0.675, 0.5]),
            p(Role::Tail, [-3.4, 0.0, 0.0], [2.1, 0.5, 0.5]),
        ],
        mass: 12.0,
    }
}

// ── JSON writer (no serde: the harness has no dependencies) ───────────

struct J(String);
impl J {
    fn f(&mut self, v: f32) {
        let v = if v.is_finite() { v } else { 0.0 };
        let _ = write!(self.0, "{:.3}", v);
    }
    fn arr3(&mut self, v: [f32; 3]) {
        self.0.push('[');
        self.f(v[0]);
        self.0.push(',');
        self.f(v[1]);
        self.0.push(',');
        self.f(v[2]);
        self.0.push(']');
    }
    fn key(&mut self, k: &str) {
        let _ = write!(self.0, "\"{}\":", k);
    }
    fn str(&mut self, s: &str) {
        let _ = write!(self.0, "\"{}\"", s.replace('"', "'"));
    }
    fn boxes(&mut self, boxes: &[PosedBox]) {
        self.0.push('[');
        for (i, b) in boxes.iter().enumerate() {
            if i > 0 {
                self.0.push(',');
            }
            self.0.push('{');
            self.key("src");
            let _ = write!(self.0, "{},", b.source.map(|s| s as i64).unwrap_or(-1));
            self.key("role");
            self.str(&format!("{:?}", b.role));
            self.0.push(',');
            self.key("color");
            self.arr3(part_color(b.role));
            self.0.push(',');
            self.key("c");
            self.0.push('[');
            for (k, c) in anim_rig::corners(b).iter().enumerate() {
                if k > 0 {
                    self.0.push(',');
                }
                self.f(c[0]);
                self.0.push(',');
                self.f(c[1]);
                self.0.push(',');
                self.f(c[2]);
            }
            self.0.push(']');
            self.0.push('}');
        }
        self.0.push(']');
    }
}

fn motion_json(j: &mut J, m: &MotionInput, extra: &[(&str, f32)]) {
    j.0.push('{');
    for (i, (k, v)) in [
        ("phase", m.gait_phase),
        ("speed", m.speed),
        ("leg_health", m.leg_health),
        ("turn", m.turn),
        ("impact", m.impact),
        ("settle", m.settle),
        ("strike", m.strike),
        ("progress", m.progress),
        ("fly_power", if m.flying { m.fly_power } else { 0.0 }),
    ]
    .iter()
    .chain(extra.iter())
    .enumerate()
    {
        if i > 0 {
            j.0.push(',');
        }
        j.key(k);
        j.f(*v);
    }
    j.0.push(',');
    j.key("state");
    j.str(&format!("{:?}", m.state));
    j.0.push('}');
}

// ── sequences ─────────────────────────────────────────────────────────

const DT: f32 = 1.0 / 30.0;

struct Frame {
    t: f32,
    m: MotionInput,
    boxes: Vec<PosedBox>,
    /// body position offset for the viewer (a walking creature travels)
    body: [f32; 3],
    note: String,
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// A scripted sequence: `script(t, &mut m)` fills the input for time t and
/// returns a note; the rig is posed each frame.
fn sequence(fx: &Fixture, secs: f32, mut script: impl FnMut(f32, &mut MotionInput) -> String) -> Vec<Frame> {
    let mut frames = Vec::new();
    let mut m = MotionInput::default();
    let mut phase = 0.0;
    let mut x = 0.0;
    let mut t = 0.0;
    while t <= secs {
        let note = script(t, &mut m);
        phase = advance_phase(phase, m.speed * (1.0 - m.settle), DT);
        m.gait_phase = phase;
        m.time = t;
        x += m.speed * (1.0 - m.settle) * DT;
        let mut boxes = Vec::new();
        anim_rig::pose(&fx.parts, fx.torso, &m, None, &mut boxes);
        frames.push(Frame { t, m, boxes, body: [x, 0.0, 0.0], note });
        t += DT;
    }
    frames
}

/// The death sequence: run, then a killing blow; the ragdoll takes over
/// while `Dying` progress ramps, then the heap settles.
fn death(fx: &Fixture) -> Vec<Frame> {
    let mut frames = Vec::new();
    let mut m = MotionInput { speed: 4.0, ..Default::default() };
    let mut phase = 0.0;
    let mut x = 0.0;
    let mut t = 0.0;
    // the ground is where the standing creature's lowest rest corner sits
    let ground_y = fx.parts.iter().map(|p| p.offset[1] - p.half[1]).fold(-fx.torso[1], f32::min);
    let mut rag: Option<(Vec<sim::ragdoll::Node>, Vec<sim::ragdoll::Stick>)> = None;
    let mut body_pos = [0.0f32; 3];
    let died_at: f32 = 0.6;
    while t <= 3.5 {
        let note;
        if t < died_at {
            m.speed = 4.0;
            m.state = MotionState::Walking;
            phase = advance_phase(phase, m.speed, DT);
            m.gait_phase = phase;
            x += m.speed * DT;
            body_pos = [x, 0.0, 0.0];
            note = "running".to_string();
        } else {
            if rag.is_none() {
                // mirrors: ragdoll_bridge::from_anatomy (body velocity + the blow's impulse)
                let parts: Vec<([f32; 3], [f32; 3])> = fx.parts.iter().map(|p| (p.offset, p.half)).collect();
                let r = (fx.torso[0] + fx.torso[1] + fx.torso[2]) / 3.0;
                let launch = [4.0 + 1.5, 2.5, 0.8];
                rag = Some(sim::ragdoll::from_limbs(body_pos, 0.0, r, &parts, launch, DT));
            }
            let (nodes, sticks) = rag.as_mut().unwrap();
            sim::ragdoll::step(nodes, sticks, ground_y, DT, 4);
            m.speed = 0.0;
            m.state = MotionState::Dying;
            m.progress = ((t - died_at) / 0.7).min(1.0);
            note = format!("dying: limpness {:.2}", sim::liveliness::limpness(m.state, m.progress));
        }
        m.time = t;
        let mut boxes = Vec::new();
        match rag.as_ref() {
            Some((nodes, _)) => {
                // the body-local sample is relative to the ORIGINAL body position
                // (the ragdoll carries the corpse away from it; that motion is in
                // the sample, so the viewer keeps the body anchor where it died)
                let (torso, parts) = sample_body_local(nodes, body_pos, 0.0);
                let rd = RagdollSample { body: torso, parts: &parts };
                anim_rig::pose(&fx.parts, fx.torso, &m, Some(&rd), &mut boxes);
            }
            None => anim_rig::pose(&fx.parts, fx.torso, &m, None, &mut boxes),
        }
        frames.push(Frame { t, m, boxes, body: body_pos, note });
        t += DT;
    }
    frames
}

fn write_sequence(j: &mut J, name: &str, fx: &Fixture, frames: &[Frame]) {
    j.0.push('{');
    j.key("name");
    j.str(name);
    j.0.push(',');
    j.key("creature");
    j.str(fx.name);
    j.0.push(',');
    j.key("dt");
    j.f(DT);
    j.0.push(',');
    j.key("frames");
    j.0.push('[');
    for (i, f) in frames.iter().enumerate() {
        if i > 0 {
            j.0.push(',');
        }
        j.0.push('{');
        j.key("t");
        j.f(f.t);
        j.0.push(',');
        j.key("body");
        j.arr3(f.body);
        j.0.push(',');
        j.key("note");
        j.str(&f.note);
        j.0.push(',');
        j.key("input");
        motion_json(j, &f.m, &[]);
        j.0.push(',');
        j.key("boxes");
        j.boxes(&f.boxes);
        j.0.push('}');
    }
    j.0.push_str("]}");
}

// ── melee ─────────────────────────────────────────────────────────────

fn melee_json(j: &mut J) {
    use sim::melee::{self, Bout, Fighter, Move, Phase};
    let fx = fauna();
    // A: a bigger, slower brawler. B: lighter, nimbler.
    let mut a = Fighter { hp: 40.0, max_hp: 40.0, mass: 3.0, power: 7.0, accuracy: 1.0, evasion: 0.4, armor: 2.0, reach: 1.3, ..Default::default() };
    let mut b = Fighter { hp: 30.0, max_hp: 30.0, mass: 1.6, power: 5.0, accuracy: 1.1, evasion: 0.9, armor: 0.0, reach: 1.1, ..Default::default() };
    let mut ba = Bout::default();
    let mut bb = Bout::default();
    let (mut xa, mut xb) = (-3.0f32, 3.0f32);
    let mut phase_a = 0.0;
    let mut phase_b = 0.0;
    let (mut ma, mut mb) = (MotionInput::default(), MotionInput::default());
    let mut seed: u32 = 0xC0FFEE;
    let mut roll = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((seed >> 8) as f32) / ((1u32 << 24) as f32)
    };
    j.key("melee");
    j.0.push_str("{\"dt\":");
    j.f(DT);
    j.0.push_str(",\"fighters\":{\"a\":{\"max_hp\":");
    j.f(a.max_hp);
    j.0.push_str(",\"mass\":");
    j.f(a.mass);
    j.0.push_str("},\"b\":{\"max_hp\":");
    j.f(b.max_hp);
    j.0.push_str(",\"mass\":");
    j.f(b.mass);
    j.0.push_str("}},\"frames\":[");
    let speed = 2.5;
    let mut t = 0.0;
    let mut first = true;
    let mut dead: Option<&str> = None;
    let mut death_t = 0.0;
    while t <= 18.0 && (dead.is_none() || t < death_t + 1.5) {
        let dist = (xb - xa).abs();
        let mut events: Vec<String> = Vec::new();
        let (mut va, mut vb) = (0.0f32, 0.0f32);
        if dead.is_none() {
            let ra = roll();
            let rb = roll();
            let threat_a = (b.hp / b.max_hp).clamp(0.0, 1.0) * 0.8;
            let threat_b = (a.hp / a.max_hp).clamp(0.0, 1.0) * 0.9;
            let (mv_a, ex_a) = melee::tick(&mut ba, &a, &b, 0.45, dist, threat_a, DT, ra);
            let (mv_b, ex_b) = melee::tick(&mut bb, &b, &a, 0.45, dist, threat_b, DT, rb);
            va = match mv_a { Move::Close => speed, Move::Back => -speed * 0.6, Move::Hold => 0.0 };
            vb = match mv_b { Move::Close => speed * 1.2, Move::Back => -speed * 0.7, Move::Hold => 0.0 };
            if let Some(e) = ex_a {
                if e.hit {
                    b.hp = e.defender_hp;
                    xb += e.knockback * 0.12;
                    let fl = bb.flinch(e.damage, b.max_hp);
                    events.push(format!("A hits B for {:.1}{}{}", e.damage, if fl { " (B flinches)" } else { "" }, if e.lethal { " — LETHAL" } else { "" }));
                    if e.lethal { dead = Some("b"); death_t = t; }
                } else {
                    events.push("A misses".into());
                }
            }
            if let Some(e) = ex_b {
                if e.hit {
                    a.hp = e.defender_hp;
                    xa -= e.knockback * 0.12;
                    let fl = ba.flinch(e.damage, a.max_hp);
                    events.push(format!("B hits A for {:.1}{}{}", e.damage, if fl { " (A flinches)" } else { "" }, if e.lethal { " — LETHAL" } else { "" }));
                    if e.lethal { dead = Some("a"); death_t = t; }
                } else {
                    events.push("B misses".into());
                }
            }
            // never overlap: stop closing at contact
            let min_d = 0.9;
            if dist - (va + vb) * DT < min_d {
                va = va.max(0.0).min((dist - min_d).max(0.0) / DT * 0.5);
                vb = vb.max(0.0).min((dist - min_d).max(0.0) / DT * 0.5);
            }
            xa += va * DT;
            xb -= vb * DT;
        }
        phase_a = advance_phase(phase_a, va.abs(), DT);
        phase_b = advance_phase(phase_b, vb.abs(), DT);
        ma = MotionInput {
            gait_phase: phase_a, speed: va.abs(), time: t,
            strike: ba.strike01(a.mass),
            state: if dead == Some("a") { MotionState::Dying } else if va.abs() > 0.1 { MotionState::Walking } else { MotionState::Idle },
            progress: if dead == Some("a") { ((t - death_t) / 0.6).min(1.0) } else { 0.0 },
            leg_health: (a.hp / a.max_hp).clamp(0.3, 1.0),
            ..ma
        };
        mb = MotionInput {
            gait_phase: phase_b, speed: vb.abs(), time: t,
            strike: bb.strike01(b.mass),
            state: if dead == Some("b") { MotionState::Dying } else if vb.abs() > 0.1 { MotionState::Walking } else { MotionState::Idle },
            progress: if dead == Some("b") { ((t - death_t) / 0.6).min(1.0) } else { 0.0 },
            leg_health: (b.hp / b.max_hp).clamp(0.3, 1.0),
            ..mb
        };
        let (mut boxes_a, mut boxes_b) = (Vec::new(), Vec::new());
        // a dead fighter collapses toward a flat heap (no solver here: the
        // death sequence shows the real ragdoll; this keeps the bout readable)
        let heap: Vec<[f32; 3]> = fx.parts.iter().map(|p| [p.offset[0] * 1.2, -0.75, p.offset[2] * 1.3]).collect();
        let rd = RagdollSample { body: [0.3, -0.7, 0.0], parts: &heap };
        anim_rig::pose(&fx.parts, fx.torso, &ma, if dead == Some("a") { Some(&rd) } else { None }, &mut boxes_a);
        anim_rig::pose(&fx.parts, fx.torso, &mb, if dead == Some("b") { Some(&rd) } else { None }, &mut boxes_b);
        if !first {
            j.0.push(',');
        }
        first = false;
        j.0.push('{');
        j.key("t");
        j.f(t);
        j.0.push(',');
        j.key("dist");
        j.f(dist);
        j.0.push(',');
        j.key("events");
        j.0.push('[');
        for (i, e) in events.iter().enumerate() {
            if i > 0 { j.0.push(','); }
            j.str(e);
        }
        j.0.push_str("],");
        for (name, f, bout, x, yaw, m, boxes) in [
            ("a", &a, &ba, xa, 0.0f32, &ma, &boxes_a),
            ("b", &b, &bb, xb, std::f32::consts::PI, &mb, &boxes_b),
        ] {
            j.key(name);
            j.0.push('{');
            j.key("hp"); j.f(f.hp); j.0.push(',');
            j.key("x"); j.f(x); j.0.push(',');
            j.key("yaw"); j.f(yaw); j.0.push(',');
            j.key("phase"); j.str(&format!("{:?}", bout.phase)); j.0.push(',');
            j.key("strike01"); j.f(bout.strike01(f.mass)); j.0.push(',');
            j.key("cooldown"); j.f(bout.cooldown); j.0.push(',');
            let _ = bout.phase == Phase::Approach;
            j.key("input"); motion_json(j, m, &[]); j.0.push(',');
            j.key("boxes"); j.boxes(boxes);
            j.0.push('}');
            if name == "a" { j.0.push(','); }
        }
        j.0.push('}');
        t += DT;
    }
    j.0.push_str("]}");
}

// ── brain ─────────────────────────────────────────────────────────────

fn brain_json(j: &mut J) {
    use sim::behavior_arbiter::{select, Behavior, Drives};
    j.key("brain");
    j.0.push_str("{\"hysteresis\":0.15,\"dt\":0.1,\"frames\":[");
    let mut prev: Option<Behavior> = None;
    let mut first = true;
    let mut t = 0.0f32;
    let mut flips = 0;
    // A day in a grazer's life, compressed: hunger rises and is fed at t≈8,
    // fatigue climbs all day and a rest at t≈14 clears it, a predator shows
    // up at t≈20 (threat spikes) while health drops with each bite.
    while t <= 30.0 {
        let hunger = if t < 8.0 { 0.15 + t / 8.0 * 0.7 } else { (0.85 - (t - 8.0) * 0.4).max(0.1) + (t - 8.0).max(0.0) * 0.02 };
        let fatigue = if t < 14.0 { t / 14.0 * 0.9 } else if t < 18.0 { 0.9 - (t - 14.0) / 4.0 * 0.85 } else { 0.05 + (t - 18.0) * 0.03 };
        let threat = if t < 20.0 { 0.0 } else { (1.0 - ((t - 20.0) - 4.0).abs() / 4.0).clamp(0.0, 1.0) };
        let health = if t < 20.0 { 1.0 } else { (1.0 - (t - 20.0) * 0.09).max(0.15) };
        let aggression = 0.55;
        let d = Drives { threat, hunger: hunger.clamp(0.0, 1.0), fatigue: fatigue.clamp(0.0, 1.0), aggression, health_frac: health };
        let c = select(&d, prev, 0.15);
        let raw = select(&d, None, 0.0);
        if prev.is_some() && prev != Some(c.behavior) { flips += 1; }
        if !first { j.0.push(','); }
        first = false;
        j.0.push('{');
        j.key("t"); j.f(t); j.0.push(',');
        j.key("drives"); j.0.push('{');
        j.key("threat"); j.f(d.threat); j.0.push(',');
        j.key("hunger"); j.f(d.hunger); j.0.push(',');
        j.key("fatigue"); j.f(d.fatigue); j.0.push(',');
        j.key("aggression"); j.f(d.aggression); j.0.push(',');
        j.key("health"); j.f(d.health_frac); j.0.push_str("},");
        j.key("choice"); j.str(&format!("{:?}", c.behavior)); j.0.push(',');
        j.key("score"); j.f(c.score); j.0.push(',');
        j.key("raw"); j.str(&format!("{:?}", raw.behavior)); j.0.push(',');
        j.key("held_by_hysteresis"); j.0.push_str(if raw.behavior != c.behavior { "true" } else { "false" });
        j.0.push('}');
        prev = Some(c.behavior);
        t += 0.1;
    }
    let _ = write!(j.0, "],\"flips\":{}}}", flips);
}

// ── items ─────────────────────────────────────────────────────────────

fn items_json(j: &mut J) {
    use item_model::{icon_quads, icon_tilt, model_for, ItemClass, Palette};
    j.key("items");
    j.0.push('[');
    for (i, class) in ItemClass::ALL.iter().enumerate() {
        let pal = match class {
            ItemClass::Ingot => Palette::mono([0.87, 0.72, 0.25]),
            ItemClass::Egg => Palette::mono([0.34, 0.49, 0.27]),
            ItemClass::Block => Palette::mono([0.55, 0.55, 0.58]),
            ItemClass::Food => Palette::mono([0.75, 0.30, 0.28]),
            ItemClass::Generic => Palette::mono([0.55, 0.45, 0.35]),
            ItemClass::Shield => Palette { primary: [0.55, 0.35, 0.2], secondary: [0.72, 0.66, 0.5], grip: item_model::LEATHER },
            ItemClass::Pickaxe | ItemClass::Axe | ItemClass::Hammer => Palette::tool([0.76, 0.76, 0.78]),
            _ => Palette { primary: [0.78, 0.80, 0.86], secondary: [0.72, 0.66, 0.50], grip: item_model::LEATHER },
        };
        let m = model_for(*class, &pal);
        let tilt = icon_tilt(*class);
        if i > 0 { j.0.push(','); }
        j.0.push('{');
        j.key("class"); j.str(&format!("{class:?}")); j.0.push(',');
        j.key("pose_class"); let _ = write!(j.0, "{},", class.pose_class());
        j.key("length"); j.f(m.length()); j.0.push(',');
        j.key("tilt"); j.f(tilt); j.0.push(',');
        j.key("boxes"); j.0.push('[');
        for (k, b) in m.boxes.iter().enumerate() {
            if k > 0 { j.0.push(','); }
            j.0.push('{');
            j.key("size"); j.arr3(b.size); j.0.push(',');
            j.key("center"); j.arr3(b.center); j.0.push(',');
            j.key("color"); j.arr3(b.color);
            j.0.push('}');
        }
        j.0.push_str("],");
        j.key("icon"); j.0.push('[');
        for (k, q) in icon_quads(&m, tilt, 0.08).iter().enumerate() {
            if k > 0 { j.0.push(','); }
            j.0.push_str("{\"pts\":[");
            for (n, p) in q.pts.iter().enumerate() {
                if n > 0 { j.0.push(','); }
                j.0.push('['); j.f(p[0]); j.0.push(','); j.f(p[1]); j.0.push(']');
            }
            j.0.push_str("],\"color\":"); j.arr3(q.color); j.0.push('}');
        }
        j.0.push(']');
        j.0.push('}');
    }
    j.0.push(']');
}

fn creatures_json(j: &mut J, fxs: &[&Fixture]) {
    j.key("creatures");
    j.0.push('{');
    for (i, fx) in fxs.iter().enumerate() {
        if i > 0 { j.0.push(','); }
        j.key(fx.name);
        j.0.push('{');
        j.key("torso"); j.arr3(fx.torso); j.0.push(',');
        j.key("mass"); j.f(fx.mass); j.0.push(',');
        j.key("parts"); j.0.push('[');
        for (k, p) in fx.parts.iter().enumerate() {
            if k > 0 { j.0.push(','); }
            j.0.push('{');
            j.key("role"); j.str(&format!("{:?}", p.role)); j.0.push(',');
            j.key("offset"); j.arr3(p.offset); j.0.push(',');
            j.key("half"); j.arr3(p.half);
            j.0.push('}');
        }
        j.0.push(']');
        j.0.push('}');
    }
    j.0.push('}');
}

fn main() {
    let fa = fauna();
    let dr = dragon();
    let mut j = J(String::with_capacity(4 << 20));
    j.0.push('{');
    j.key("generated_by");
    j.str("harness/rig/main.rs — real engine files by #[path]; see the header for the three mirrored helpers");
    j.0.push(',');
    items_json(&mut j);
    j.0.push(',');
    creatures_json(&mut j, &[&fa, &dr]);
    j.0.push(',');
    j.key("sequences");
    j.0.push('[');
    let seqs: Vec<(&str, &Fixture, Vec<Frame>)> = vec![
        ("walk", &fa, sequence(&fa, 5.0, |t, m| {
            m.speed = 6.0 * smoothstep(t / 3.5);
            m.state = if m.speed > 0.2 { MotionState::Walking } else { MotionState::Idle };
            format!("speed {:.1} blocks/s", m.speed)
        })),
        ("turn", &fa, sequence(&fa, 6.0, |t, m| {
            m.speed = 4.0;
            m.state = MotionState::Walking;
            m.turn = (TAU * t / 4.0).sin();
            m.gaze_yaw = m.turn * 0.5;
            format!("turn {:+.2}", m.turn)
        })),
        ("hurt", &fa, sequence(&fa, 4.0, |t, m| {
            m.speed = 4.0;
            m.leg_health = if t < 2.0 { 1.0 } else { 0.35 };
            m.state = MotionState::Walking;
            if t < 2.0 { "healthy".into() } else { "leg_health 0.35 — a limp".into() }
        })),
        ("land", &fa, sequence(&fa, 3.0, |t, m| {
            m.speed = 0.0;
            m.impact = if t < 0.6 { 0.0 } else { (-(t - 0.6) * 5.0).exp() };
            m.state = MotionState::Idle;
            if t < 0.6 { "airborne… (torso only)".into() } else { format!("landing squash {:.2}", m.impact) }
        })),
        ("strike", &fa, sequence(&fa, 3.0, |t, m| {
            let d = sim::melee::strike_duration(fa.mass);
            let s = if t < 0.4 { 0.0 } else if t < 0.4 + d { (t - 0.4) / d } else if t < 1.4 { 0.0 } else if t < 1.4 + d { (t - 1.4) / d } else { 0.0 };
            m.strike = s;
            m.state = MotionState::Idle;
            format!("strike {:.2} (curve {:+.2})", s, anim_rig::strike_curve(s))
        })),
        ("settle", &fa, sequence(&fa, 6.0, |t, m| {
            m.speed = 0.0;
            m.settle = if t < 1.0 { 0.0 } else if t < 2.5 { smoothstep((t - 1.0) / 1.5) } else if t < 4.5 { 1.0 } else { 1.0 - smoothstep((t - 4.5) / 1.5) };
            m.state = if m.settle > 0.5 { MotionState::Resting } else { MotionState::Idle };
            format!("settle {:.2}", m.settle)
        })),
        ("fly", &dr, sequence(&dr, 5.0, |t, m| {
            m.speed = 5.0;
            m.flying = t > 1.0;
            m.fly_power = 1.0;
            m.state = MotionState::Walking;
            if t > 1.0 { "airborne: wings beat at 3× the gait".into() } else { "grounded: wings still".into() }
        })),
        ("death", &fa, death(&fa)),
    ];
    for (i, (name, fx, frames)) in seqs.iter().enumerate() {
        if i > 0 { j.0.push(','); }
        write_sequence(&mut j, name, fx, frames);
    }
    j.0.push(']');
    j.0.push(',');
    melee_json(&mut j);
    j.0.push(',');
    brain_json(&mut j);
    j.0.push('}');
    println!("{}", j.0);
}
