//! rig_dump — the rig on the REAL engine: every design-layer archetype spawned
//! by `spawn::spawn_designed`, posed by `world::entity::creature_to_vertices_rigged`,
//! killed by `sim::ragdoll_bridge::from_anatomy`, and the engine-coupled checks
//! the test binary cannot run in the sandbox (the test crate's codegen dies;
//! a plain `--lib` build and an example link fine).
//!
//!     cargo run --example rig_dump -p unendlich-engine --profile sandboxtest > rig_real.json
//!
//! Exit status is the verdict: the checks are `assert!`s. The JSON has the
//! `harness/rig/viewer.html` shape (`creatures` + `sequences`) and run.sh
//! merges it into the viewer next to the rustc-only harness's output, so the
//! Creature tab shows "quadruped (engine)" beside the hand-written fixtures —
//! no mirrored helpers on this path at all.

use unendlich_engine::anim_rig::{MotionInput, RagdollSample};
use unendlich_engine::creature::Creature;
use unendlich_engine::creature_design::Arche;
use unendlich_engine::sim::liveliness::MotionState;
use unendlich_engine::world::entity::{creature_to_vertices, creature_to_vertices_rigged, EntityVertex};
use std::fmt::Write;

/// Sim step for the checks; the dump samples every other frame (the file
/// is embedded in an HTML page, 40 MB at 30 fps for nine creatures).
const DT: f32 = 1.0 / 30.0;
const DUMP_EVERY: usize = 3;

fn f(out: &mut String, v: f32) {
    let v = if v.is_finite() { v } else { 0.0 };
    let _ = write!(out, "{:.2}", v);
}

/// The mesher emits eight vertices per box in push_box_oriented order; group them.
fn boxes_json(out: &mut String, verts: &[EntityVertex]) {
    out.push('[');
    for (i, chunk) in verts.chunks(8).enumerate() {
        if chunk.len() < 8 {
            break;
        }
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"src\":");
        let _ = write!(out, "{}", i as i64 - 1);
        out.push_str(",\"role\":\"engine\",\"color\":[");
        f(out, chunk[0].color[0]);
        out.push(',');
        f(out, chunk[0].color[1]);
        out.push(',');
        f(out, chunk[0].color[2]);
        out.push_str("],\"c\":[");
        for (k, v) in chunk.iter().enumerate() {
            if k > 0 {
                out.push(',');
            }
            f(out, v.position[0]);
            out.push(',');
            f(out, v.position[1]);
            out.push(',');
            f(out, v.position[2]);
        }
        out.push_str("]}");
    }
    out.push(']');
}

fn lowest_y(v: &[EntityVertex]) -> f32 {
    v.iter().map(|q| q.position[1]).fold(f32::MAX, f32::min)
}

struct Seq {
    name: String,
    creature: String,
    frames: Vec<(f32, MotionInput, String, Vec<EntityVertex>)>,
}

fn sequence(c: &Creature, name: &str, label: &str, secs: f32, mut script: impl FnMut(f32, &mut MotionInput) -> String) -> Seq {
    let mut frames = Vec::new();
    let mut m = MotionInput::default();
    let mut phase = 0.0f32;
    let mut t = 0.0f32;
    while t <= secs {
        let note = script(t, &mut m);
        phase = (phase + 2.2 * m.speed * (1.0 - m.settle) * DT).rem_euclid(std::f32::consts::TAU);
        m.gait_phase = phase;
        m.time = t;
        let (verts, _) = creature_to_vertices_rigged(c, &m, None, 0.0);
        frames.push((t, m, note, verts));
        t += DT;
    }
    Seq { name: name.to_string(), creature: label.to_string(), frames }
}

fn death(c: &Creature, name: &str, label: &str) -> Seq {
    use unendlich_engine::sim::ragdoll_bridge::{from_anatomy, sample_body_local};
    let anat = c.anatomy.as_ref().expect("design creature has an anatomy");
    let mut frames = Vec::new();
    let mut m = MotionInput { speed: 3.0, state: MotionState::Walking, ..Default::default() };
    let mut phase = 0.0f32;
    let mut t = 0.0f32;
    // lowest_point is the (negative) offset of the sole below the body centre
    let ground = c.body.position[1] + c.lowest_point();
    let mut rag = None;
    while t <= 3.5 {
        let note;
        if t < 0.6 {
            phase = (phase + 2.2 * 3.0 * DT).rem_euclid(std::f32::consts::TAU);
            m.gait_phase = phase;
            note = "walking".to_string();
        } else {
            if rag.is_none() {
                rag = Some(from_anatomy(c, anat, [3.0, 2.5, 0.5], DT));
            }
            let (nodes, sticks) = rag.as_mut().unwrap();
            unendlich_engine::sim::ragdoll::step(nodes, sticks, ground, DT, 4);
            m.speed = 0.0;
            m.state = MotionState::Dying;
            m.progress = ((t - 0.6) / 0.7).min(1.0);
            note = format!("dying {:.2}", m.progress);
        }
        m.time = t;
        let verts = match rag.as_ref() {
            Some((nodes, _)) => {
                let (body, parts) = sample_body_local(nodes, c.body.position, c.yaw);
                let rd = RagdollSample { body, parts: &parts };
                creature_to_vertices_rigged(c, &m, Some(&rd), 0.0).0
            }
            None => creature_to_vertices_rigged(c, &m, None, 0.0).0,
        };
        frames.push((t, m, note, verts));
        t += DT;
    }
    Seq { name: name.to_string(), creature: label.to_string(), frames }
}

fn main() {
    // ── engine-coupled checks ─────────────────────────────────────────
    // item models: the whole catalog and the survival ids
    {
        use unendlich_engine::item::{full_catalog, item_model_for_id, ItemKind};
        use unendlich_engine::item_model::ItemClass;
        for it in full_catalog() {
            let m = it.model();
            assert!(!m.boxes.is_empty() && m.class != ItemClass::Generic, "{it:?}");
            assert_eq!(m.class, it.model_class());
            let [r, g, b] = it.display_color();
            let p = it.model_palette().primary;
            assert!((p[0] - r as f32 / 255.0).abs() < 1e-6 && (p[1] - g as f32 / 255.0).abs() < 1e-6 && (p[2] - b as f32 / 255.0).abs() < 1e-6);
        }
        for id in 2000..=2002 {
            assert_eq!(item_model_for_id(id).class, ItemClass::Food);
        }
        assert_eq!(item_model_for_id(ItemKind::UNMAPPED_ID).class, ItemClass::Generic);
        eprintln!("ok  item models: {} catalog items + survival ids", full_catalog().len());
    }
    // every ecology kind spawns, rigs, and (with anatomy) ragdolls
    let mut creatures: Vec<(String, Creature)> = Vec::new();
    for arche in unendlich_engine::spawn::FAUNA_ARCHES.iter().copied().chain([Arche::Lauscher, Arche::Dinosaur]) {
        let mut c = unendlich_engine::spawn::spawn_designed(arche, [0.0, 0.0, 0.0]);
        c.body.position = [0.0; 3];
        assert!(c.design.is_some(), "{arche:?} kept no design");
        assert!(c.anatomy.as_ref().map_or(0, |a| a.parts.len()) > 0, "{arche:?} has no parts");
        let label = c.design.as_ref().map(|d| d.arch.to_string()).unwrap_or_default();
        creatures.push((format!("{} (engine)", label), c));
    }
    for kind in unendlich_engine::item::CreatureKind::ALL {
        let mut c = unendlich_engine::spawn::spawn(kind, [0.0, 0.0, 0.0]);
        c.body.position = [0.0; 3];
        let rest = creature_to_vertices_rigged(&c, &MotionInput::default(), None, 0.0).0;
        assert!(!rest.is_empty(), "{kind:?} rigs to nothing");
        assert!(rest.iter().all(|v| v.position.iter().all(|x| x.is_finite())), "{kind:?} non-finite");
        if c.design.is_none() && c.forge.is_none() {
            // box anatomy: torso + non-chain parts identical to the plain path
            let plain = creature_to_vertices(&c).0;
            let n = plain.len().min(rest.len()).min(8 * 3);
            for i in 0..n {
                for k in 0..3 {
                    assert!((plain[i].position[k] - rest[i].position[k]).abs() < 1e-3, "{kind:?} vertex {i} axis {k} differs at rest");
                }
            }
        }
        if let Some(anat) = c.anatomy.as_ref() {
            let (nodes, sticks) = unendlich_engine::sim::ragdoll_bridge::from_anatomy(&c, anat, [0.0; 3], DT);
            assert_eq!(nodes.len(), anat.parts.len() + 1, "{kind:?}");
            assert_eq!(sticks.len(), anat.parts.len());
            let (torso, parts) = unendlich_engine::sim::ragdoll_bridge::sample_body_local(&nodes, c.body.position, c.yaw);
            assert!(torso.iter().all(|v| v.abs() < 1e-4));
            assert_eq!(parts.len(), anat.parts.len());
        }
        eprintln!("ok  {:?}: design {} forge {} parts {} verts {}", kind, c.design.is_some(), c.forge.is_some(), c.anatomy.as_ref().map_or(0, |a| a.parts.len()), rest.len());
    }
    // walking lifts a foot on a design quadruped and the death heap settles below standing
    {
        let (_, c) = &creatures[0];
        let rest = creature_to_vertices_rigged(c, &MotionInput::default(), None, 0.0).0;
        let mut lifted = false;
        for k in 0..30 {
            let m = MotionInput { gait_phase: k as f32 / 30.0 * std::f32::consts::TAU, speed: 5.0, ..Default::default() };
            let walk = creature_to_vertices_rigged(c, &m, None, 0.0).0;
            assert!(lowest_y(&walk) >= lowest_y(&rest) - 0.05, "a foot sinks below the rest sole while walking");
            // any single box above its rest lowest point by a stride's lift
            for (i, (a, b)) in rest.chunks(8).zip(walk.chunks(8)).enumerate() {
                if i == 0 { continue; }
                if lowest_y(b) > lowest_y(a) + 0.05 { lifted = true; }
            }
        }
        assert!(lifted, "no box ever lifts while walking");
        let d = death(c, "death", "x");
        let last = &d.frames[d.frames.len() - 1].3;
        let first = &d.frames[0].3;
        let cy = |v: &[EntityVertex]| v.iter().map(|q| q.position[1]).sum::<f32>() / v.len() as f32;
        assert!(cy(last) < cy(first) - 0.1, "the corpse does not settle lower than the standing body: {} vs {}", cy(last), cy(first));
        eprintln!("ok  walk lifts, death settles ({:.2} -> {:.2})", cy(first), cy(last));
    }
    // melee + abilities on real forged weapons
    {
        use unendlich_engine::sim::legendary_weapon::{forge, WeaponClass};
        use unendlich_engine::sim::weapon_ability::on_hit;
        for e in 0..6u8 {
            let w = forge(11, WeaponClass::Blade, e, 2);
            let o = on_hit(&w, [0.0; 3], [1.0, 0.0, 0.0], &[[2.0, 0.0, 0.0]], 0.0);
            assert!(o.target_status.is_some() || o.knockback_mult > 1.0, "element {e}");
        }
        eprintln!("ok  abilities on forged weapons");
    }

    // ── dump ──────────────────────────────────────────────────────────
    let mut out = String::with_capacity(8 << 20);
    out.push_str("{\"generated_by\":\"examples/rig_dump.rs — the real engine: spawn_designed -> creature_to_vertices_rigged\",\"creatures\":{");
    for (i, (label, c)) in creatures.iter().enumerate() {
        if i > 0 { out.push(','); }
        let _ = write!(out, "\"{}\":{{\"torso\":[", label);
        f(&mut out, c.body.half_extents[0]); out.push(','); f(&mut out, c.body.half_extents[1]); out.push(','); f(&mut out, c.body.half_extents[2]);
        out.push_str("],\"mass\":"); f(&mut out, c.mass);
        out.push_str(",\"parts\":[");
        if let Some(a) = c.anatomy.as_ref() {
            for (k, p) in a.parts.iter().enumerate() {
                if k > 0 { out.push(','); }
                let _ = write!(out, "{{\"role\":\"{:?}\",\"offset\":[", p.part);
                f(&mut out, p.offset[0]); out.push(','); f(&mut out, p.offset[1]); out.push(','); f(&mut out, p.offset[2]);
                out.push_str("],\"half\":[");
                f(&mut out, p.half_extents[0]); out.push(','); f(&mut out, p.half_extents[1]); out.push(','); f(&mut out, p.half_extents[2]);
                out.push_str("]}");
            }
        }
        out.push_str("]}");
    }
    out.push_str("},\"sequences\":[");
    let mut first = true;
    for (label, c) in &creatures {
        let seqs = vec![
            sequence(c, "walk+turn", label, 4.0, |t, m| { m.speed = 5.0 * (t / 1.5).min(1.0); m.turn = if t > 2.0 { (std::f32::consts::TAU * (t - 2.0) / 2.0).sin() } else { 0.0 }; m.state = MotionState::Walking; format!("speed {:.1} turn {:+.2}", m.speed, m.turn) }),
            sequence(c, "strike+settle", label, 4.0, |t, m| {
                m.strike = if t > 0.3 && t < 1.0 { (t - 0.3) / 0.7 } else { 0.0 };
                m.settle = if t > 1.6 { ((t - 1.6) / 1.5).min(1.0) } else { 0.0 };
                m.state = if m.settle > 0.5 { MotionState::Resting } else { MotionState::Idle };
                format!("strike {:.2} settle {:.2}", m.strike, m.settle)
            }),
            death(c, "death", label),
        ];
        for s in seqs {
            if !first { out.push(','); }
            first = false;
            let _ = write!(out, "{{\"name\":\"{}\",\"creature\":\"{}\",\"dt\":{:.4},\"frames\":[", s.name, s.creature, DT * DUMP_EVERY as f32);
            for (i, (t, m, note, verts)) in s.frames.iter().step_by(DUMP_EVERY).enumerate() {
                if i > 0 { out.push(','); }
                out.push_str("{\"t\":"); f(&mut out, *t);
                out.push_str(",\"body\":[0,0,0],\"note\":\""); out.push_str(&note.replace('"', "'"));
                out.push_str("\",\"input\":{\"phase\":"); f(&mut out, m.gait_phase);
                out.push_str(",\"speed\":"); f(&mut out, m.speed);
                out.push_str(",\"turn\":"); f(&mut out, m.turn);
                out.push_str(",\"settle\":"); f(&mut out, m.settle);
                out.push_str(",\"strike\":"); f(&mut out, m.strike);
                out.push_str(",\"progress\":"); f(&mut out, m.progress);
                let _ = write!(out, ",\"state\":\"{:?}\"}},\"boxes\":", m.state);
                boxes_json(&mut out, verts);
                out.push('}');
            }
            out.push_str("]}");
        }
    }
    out.push_str("]}");
    println!("{}", out);
    eprintln!("ok  dumped {} creatures", creatures.len());
}
