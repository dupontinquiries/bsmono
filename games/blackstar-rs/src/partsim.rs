//! PART SIM — run `partdetect::classify` and `loadbearing::check`
//! against real chassis geometry.
//!
//! Read-only, unlike `orientsim.rs`'s `fix`/`sweep` — there is nowhere
//! to WRITE a part guess yet (no file format for it, no consumer), so
//! this only ever prints. That is deliberate: `partdetect.rs`'s own doc
//! is explicit that `classify`'s thresholds are reasoned about, not
//! fit to labelled data, and the only way to move that from "reasoned
//! about" to "checked" is to look at what it says about real chassis
//! and correct by eye — which is what these commands are for, not for
//! committing a guess anywhere yet.
//!
//!     skydribble --parts check <bot> <pose>
//!     skydribble --parts stability <bot> <pose>
//!
//! Whole-body up-axis comes from `orient::guess` on the fly, same
//! convention `orientsim.rs`'s own `guess_for` already uses rather than
//! trusting a saved `orient.json` that may or may not have been fixed.

use std::path::PathBuf;

use crate::loadbearing;
use crate::orient;
use crate::partdetect::{self, Kind};

fn bots_dir() -> PathBuf {
    let local = PathBuf::from("assets/bots");
    if local.exists() {
        local
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/bots")
    }
}

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--parts").unwrap();
    let cmd = argv.get(i + 1).map(|s| s.as_str());
    match cmd {
        Some("check") => {
            let (Some(bot), Some(pose)) = (argv.get(i + 2), argv.get(i + 3)) else {
                println!("usage: --parts check <bot> <pose>");
                return;
            };
            check(bot, pose);
        }
        Some("stability") => {
            let (Some(bot), Some(pose)) = (argv.get(i + 2), argv.get(i + 3)) else {
                println!("usage: --parts stability <bot> <pose>");
                return;
            };
            stability(bot, pose);
        }
        _ => {
            println!("usage:");
            println!("  skydribble --parts check <bot> <pose>");
            println!("  skydribble --parts stability <bot> <pose>");
        }
    }
}

fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::Leg => "LEG",
        Kind::Wheel => "WHEEL",
        Kind::Thruster => "THRUSTER",
        Kind::Wing => "WING",
        Kind::Body => "BODY",
    }
}

/// Everything both commands need from a pose: the raw prims, the
/// whole-body up-axis guess, and the whole-body bounds/centre/width
/// `partdetect`'s per-prim functions and `loadbearing::check` both
/// take as input rather than each re-deriving.
struct Loaded {
    raws: Vec<crate::model::RawPrim>,
    up: glam::Vec3,
    up_confident: bool,
    lo: f32,
    hi: f32,
    center: glam::Vec3,
    half_width: f32,
}

fn load(bot: &str, pose: &str) -> Option<Loaded> {
    let dir = bots_dir().join(bot).join(pose);
    let raws = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::model::load_raw_unoriented(&dir)
    }))
    .ok()?;
    if raws.is_empty() {
        return None;
    }
    let whole = orient::all_verts(&raws);
    let guess = orient::guess(&whole);
    let up = guess.up;

    // Whole-body bounds along up, and the horizontal centre/half-width
    // partdetect's ground_fraction/lateral_offset and loadbearing::check
    // all need -- computed once here rather than inside each of them,
    // the same "measure the whole body once, not per prim" discipline
    // xform::bounds_at uses.
    let (pa, pb) = orient::perp_pair(up);
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    let (mut a_lo, mut a_hi) = (f32::MAX, f32::MIN);
    let (mut b_lo, mut b_hi) = (f32::MAX, f32::MIN);
    for v in &whole {
        let p = glam::Vec3::from_array(v.pos);
        let h = p.dot(up);
        lo = lo.min(h);
        hi = hi.max(h);
        let x = p.dot(pa);
        a_lo = a_lo.min(x);
        a_hi = a_hi.max(x);
        let y = p.dot(pb);
        b_lo = b_lo.min(y);
        b_hi = b_hi.max(y);
    }
    let center_h = (lo + hi) * 0.5;
    // The full 3D centre, not just its up-axis component: an earlier
    // version of this assumed the horizontal centre was the origin
    // ("by construction of pa/pb being perpendicular" -- true of the
    // AXES, not of where the model's own geometry happens to sit
    // relative to them), which is only true if the raw mesh is already
    // centred on its own local origin. It is not, in general -- caught
    // by every prim in a real chassis reading lateral > 1.0 with none
    // near the spine, which is not a plausible reading for an actual
    // body.
    let center = up * center_h + pa * ((a_lo + a_hi) * 0.5) + pb * ((b_lo + b_hi) * 0.5);
    let half_width = ((a_hi - a_lo).max(b_hi - b_lo) * 0.5).max(1e-6);

    Some(Loaded { raws, up, up_confident: guess.up_confident, lo, hi, center, half_width })
}

fn check(bot: &str, pose: &str) {
    let Some(l) = load(bot, pose) else {
        println!("could not load {bot}/{pose}");
        return;
    };
    println!("{bot}/{pose}: {} prims, whole-body up guess {} (confident={})",
        l.raws.len(), orient::axis_name(l.up), l.up_confident);
    println!("  {:<4} {:<9} {:>10} {:>8} {:>8} {:>7}  verts", "idx", "kind", "elongation", "taper", "ground", "lateral");
    for (idx, r) in l.raws.iter().enumerate() {
        if r.verts.is_empty() {
            continue;
        }
        let elong = partdetect::elongation(&r.verts);
        let tap = partdetect::taper(&r.verts, l.up);
        let ground = partdetect::ground_fraction(&r.verts, l.up, l.lo, l.hi);
        let lateral = partdetect::lateral_offset(&r.verts, l.up, l.center, l.half_width);
        let kind = partdetect::classify(&r.verts, l.up, l.lo, l.hi, l.center, l.half_width);
        println!("  {:<4} {:<9} {:>10.2} {:>8.2} {:>8.2} {:>7.2}  {}",
            idx, kind_label(kind), elong, tap, ground, lateral, r.verts.len());
    }
}

fn stability(bot: &str, pose: &str) {
    let Some(l) = load(bot, pose) else {
        println!("could not load {bot}/{pose}");
        return;
    };
    // classify() needs Vec3 verts, not Vertex -- convert once per prim
    // rather than inside the hot loop loadbearing::check runs.
    let converted: Vec<(Vec<glam::Vec3>, Kind)> = l
        .raws
        .iter()
        .filter(|r| !r.verts.is_empty())
        .map(|r| {
            let vs: Vec<glam::Vec3> = r.verts.iter().map(|v| glam::Vec3::from_array(v.pos)).collect();
            let kind = partdetect::classify(&r.verts, l.up, l.lo, l.hi, l.center, l.half_width);
            (vs, kind)
        })
        .collect();
    let inputs: Vec<loadbearing::PrimInput> =
        converted.iter().map(|(vs, k)| loadbearing::PrimInput { verts: vs, kind: *k }).collect();
    let st = loadbearing::check(&inputs, l.up);
    println!("{bot}/{pose}: up guess {} (confident={})", orient::axis_name(l.up), l.up_confident);
    if st.contact_points < 3 {
        println!("  {} leg/wheel contact point(s) -- not enough for a support polygon", st.contact_points);
        return;
    }
    println!("  {} contact points, stability margin {:.4} ({})",
        st.contact_points, st.margin,
        if st.margin > 0.0 { "stands" } else { "would tip -- centre of mass falls outside its own feet" });
}
