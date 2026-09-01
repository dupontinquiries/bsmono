//! HARDPOINTS — where the guns, the thrusters and the cockpit ARE, read
//! off the geometry the game actually loaded.
//!
//! The vehicles are models with their fittings baked in: a barrel over
//! the deck, nozzles at the tail, a canopy up front, wheels underneath.
//! `tools/find-mounts.py` read some of that offline into
//! `tools/mounts.json`, which the game never loaded, and which was as far
//! as the offline approach got: a JSON in the vehicle pose's RAW space
//! that the loader's rescale and re-orientation would have had to be
//! reproduced against, for 28 of the chassis. So the shell invented a
//! "hood gun" at `half_len * 0.9` and a boost flame at the tail centre,
//! and both were wrong on every model that had a real answer.
//!
//! This runs IN THE LOADER, on the same oriented per-prim points
//! `turret::find` already walks (+Y up, nose at -Z, final metres,
//! centred), so the answers are in the space the renderer draws in and
//! need no second transform. It is also the reason `turret.rs` is not
//! simply extended: that module is a tank's fittings — a turret ring and
//! the barrel that sits on it. This is every hull's.
//!
//! What it finds, and how:
//!
//! * **Guns.** A thin prim, long along Z, clear of the ground, its narrow
//!   end forward. The muzzle is the narrow end's tip. A tank's turret
//!   barrel is one of these too, and agrees with `turret::find` when both
//!   see it.
//! * **Thrusters.** A round prim at the rear face, its open (wider) end
//!   aft, not low enough to be a wheel. The flame goes at the open end.
//! * **Cockpit.** A compact prim riding high and forward on the hull.
//!   Not used for firing; used for the forward hint and for cameras.
//! * **Popouts.** When nothing on the hull is gun-shaped — a racer — the
//!   weapon comes out of the upper hull, forward of centre, as a mirrored
//!   pair, at the actual top surface there. Every vehicle therefore has a
//!   muzzle that is on its own body.
//!
//! And a **forward hint**: thrusters open aft and guns point forward, so
//! a hull whose nozzles open at -Z is loaded backwards. Reported, not
//! applied — `model.rs`'s orientation search has its own evidence and its
//! own history of flip-flopping; this is one more witness for it to call.
//!
//! Every threshold is a ratio against the hull's own extents, so a
//! freighter and a racer are judged by their own proportions. The
//! synthetic tests below are the geometry each rule was written against;
//! the imported roster is the geometry it has to survive, and
//! `--hardpoints` prints what it found there.

use glam::Vec3;

/// A prim must be at least this many times longer than its mean
/// thickness to read as a barrel or a nozzle.
pub const BARREL_ASPECT: f32 = 2.6;
/// ...and its thickness no more than this fraction of the hull's half
/// width: a car body is elongated too, and is not a gun.
pub const THIN: f32 = 0.22;
/// A gun's axis within this of Z (cosine). 25 degrees.
pub const ALONG_Z: f32 = 0.906;
/// A gun sits above this fraction of the hull's height (0 = bottom).
pub const GUN_ABOVE: f32 = 0.30;
/// A thruster's centre is behind this fraction of the half length.
pub const THRUSTER_AFT: f32 = 0.45;
/// A thruster's cross-section is round to within this ratio.
pub const ROUND: f32 = 1.45;
/// A thruster is above this fraction of the height — below it is a wheel.
pub const THRUSTER_ABOVE: f32 = 0.22;
/// A cockpit rides above this fraction of the height and forward of the
/// hull's middle, and is no wider than this fraction of the half width.
pub const COCKPIT_ABOVE: f32 = 0.55;
pub const COCKPIT_WIDE: f32 = 0.6;
/// Popouts sit this far forward and this far outboard, as fractions.
pub const POPOUT_FWD: f32 = 0.35;
pub const POPOUT_OUT: f32 = 0.45;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Gun,
    Thruster,
    Cockpit,
    /// synthetic: the upper hull, because nothing gun-shaped was found
    Popout,
}

/// One fitting: where it is and which way it points, in model space.
#[derive(Clone, Copy, Debug)]
pub struct Mount {
    pub kind: Kind,
    pub pos: Vec3,
    pub aim: Vec3,
    /// which prim it was read from; `None` for a popout
    pub prim: Option<usize>,
    /// 0..1: how cleanly the rule matched
    pub confidence: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Hardpoints {
    pub guns: Vec<Mount>,
    pub thrusters: Vec<Mount>,
    pub cockpit: Option<Mount>,
    /// the pair every hull gets when it has no guns
    pub popouts: Vec<Mount>,
    /// +1: the fittings agree the nose is at -Z; -1: they say it is at
    /// +Z; 0: no witness. Thrusters weigh twice what guns do.
    pub forward_hint: f32,
}

impl Hardpoints {
    /// Where the `i`th shot comes from and where it goes: real guns,
    /// alternating; else the popouts, alternating. Never `None` for a
    /// hull with any points at all.
    pub fn muzzle(&self, i: usize) -> Option<Mount> {
        if !self.guns.is_empty() {
            return Some(self.guns[i % self.guns.len()]);
        }
        if !self.popouts.is_empty() {
            return Some(self.popouts[i % self.popouts.len()]);
        }
        None
    }

    pub fn has_real_guns(&self) -> bool {
        !self.guns.is_empty()
    }

    pub fn report(&self) -> String {
        let mut s = String::new();
        for g in &self.guns {
            s.push_str(&format!("  gun      at ({:.2},{:.2},{:.2}) aim ({:.1},{:.1},{:.1}) prim {:?} conf {:.2}\n", g.pos.x, g.pos.y, g.pos.z, g.aim.x, g.aim.y, g.aim.z, g.prim, g.confidence));
        }
        for t in &self.thrusters {
            s.push_str(&format!("  thruster at ({:.2},{:.2},{:.2}) aim ({:.1},{:.1},{:.1}) prim {:?} conf {:.2}\n", t.pos.x, t.pos.y, t.pos.z, t.aim.x, t.aim.y, t.aim.z, t.prim, t.confidence));
        }
        if let Some(c) = &self.cockpit {
            s.push_str(&format!("  cockpit  at ({:.2},{:.2},{:.2}) prim {:?} conf {:.2}\n", c.pos.x, c.pos.y, c.pos.z, c.prim, c.confidence));
        }
        for p in &self.popouts {
            s.push_str(&format!("  popout   at ({:.2},{:.2},{:.2}) (synthetic)\n", p.pos.x, p.pos.y, p.pos.z));
        }
        s.push_str(&format!("  forward hint {:+.1}\n", self.forward_hint));
        s
    }
}

struct Ext {
    lo: Vec3,
    hi: Vec3,
    c: Vec3,
    half: Vec3,
    n: usize,
}

fn ext(pts: &[Vec3]) -> Option<Ext> {
    if pts.is_empty() {
        return None;
    }
    let (mut lo, mut hi, mut sum) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN), Vec3::ZERO);
    for p in pts {
        lo = lo.min(*p);
        hi = hi.max(*p);
        sum += *p;
    }
    Some(Ext { lo, hi, c: sum / pts.len() as f32, half: (hi - lo) * 0.5, n: pts.len() })
}

/// Which end of a prim along `axis` is narrower: the mean cross-section
/// radius of the front third against the back third. Returns
/// (front_radius, back_radius) where "front" is the -axis end.
fn end_radii(pts: &[Vec3], c: Vec3, axis: Vec3, half_along: f32) -> (f32, f32) {
    let (mut fs, mut fn_, mut bs, mut bn) = (0.0f32, 0usize, 0.0f32, 0usize);
    for p in pts {
        let d = *p - c;
        let a = d.dot(axis);
        let r = (d - axis * a).length();
        if a < -half_along * 0.33 {
            fs += r;
            fn_ += 1;
        } else if a > half_along * 0.33 {
            bs += r;
            bn += 1;
        }
    }
    (if fn_ > 0 { fs / fn_ as f32 } else { 0.0 }, if bn > 0 { bs / bn as f32 } else { 0.0 })
}

/// Read a hull. `prims` are per-prim points in the oriented frame.
pub fn find(prims: &[&[Vec3]]) -> Hardpoints {
    find_traced(prims, false)
}

/// `find`, narrating every barrel-shaped prim's verdict to stdout when
/// `trace` — the driver's per-hull mode.
pub fn find_traced(prims: &[&[Vec3]], trace: bool) -> Hardpoints {
    let mut out = Hardpoints::default();
    let all: Vec<Vec3> = prims.iter().flat_map(|p| p.iter().copied()).collect();
    let Some(hull) = ext(&all) else { return out };
    if hull.half.x < 1e-4 || hull.half.y < 1e-4 || hull.half.z < 1e-4 {
        return out;
    }
    let height = hull.hi.y - hull.lo.y;
    let mut hint = 0.0f32;
    let mut front_flares: Vec<Mount> = Vec::new();

    for (i, p) in prims.iter().enumerate() {
        if p.len() < 8 {
            continue;
        }
        let Some(e) = ext(p) else { continue };
        let thick_x = e.half.x / hull.half.x;
        let thick_y = e.half.y / hull.half.y;
        let above = (e.c.y - hull.lo.y) / height;
        // does its underside sit on the ground line: a wheel, a skid, a
        // tread — whatever else it looks like, it is not a nozzle
        let grounded = (e.lo.y - hull.lo.y) / height < 0.10;
        let aft = (e.c.z - hull.c.z) / hull.half.z;
        let mean_thick = (e.half.x + e.half.y) * 0.5;
        let aspect_z = if mean_thick > 1e-5 { e.half.z / mean_thick } else { 0.0 };

        // ---- a barrel: long along Z, thin, off the ground ----
        if aspect_z >= BARREL_ASPECT && thick_x <= THIN && thick_y <= THIN && above >= GUN_ABOVE {
            let (front_r, back_r) = end_radii(p, e.c, Vec3::Z, e.half.z);
            // the muzzle is the narrow end; a barrel with equal ends
            // points forward by convention (nose at -Z)
            let forward = back_r >= front_r * 0.95;
            let aim = if forward { Vec3::NEG_Z } else { Vec3::Z };
            let tip = if forward { Vec3::new(e.c.x, e.c.y, e.lo.z) } else { Vec3::new(e.c.x, e.c.y, e.hi.z) };
            let conf = ((aspect_z / BARREL_ASPECT).min(2.0) / 2.0 * 0.6 + 0.4 * (1.0 - thick_x.max(thick_y) / THIN)).clamp(0.2, 1.0);
            // a nozzle also reads as a thin tube along Z; the rear face
            // and the flare tell them apart
            let flared_aft = aft > THRUSTER_AFT && back_r > front_r * 1.08;
            if flared_aft {
                if trace {
                    println!("  barrel-shaped prim {i}: flared aft -> thruster");
                }
                out.thrusters.push(Mount { kind: Kind::Thruster, pos: Vec3::new(e.c.x, e.c.y, e.hi.z), aim: Vec3::Z, prim: Some(i), confidence: conf });
                hint += 2.0;
                continue;
            }
            // A PIPE IS NOT A GUN. Exhausts, sills and frame rails are thin
            // tubes along Z too; on the roster they outnumbered barrels
            // five to one. A gun's tip is in the FRONT half of the hull,
            // or the gun rides on the deck (a turret's barrel can sit over
            // the middle). A tube whose tip is aft and low is a pipe —
            // and a pipe at the rear that flares is a nozzle, caught above.
            let deck = above >= 0.6;
            let tip_forward = tip.z < hull.c.z - hull.half.z * 0.1;
            if trace {
                println!("  barrel-shaped prim {i}: aspect {aspect_z:.1} above {above:.2} aft {aft:.2} tip z {:.3} forward {forward} deck {deck} tip_forward {tip_forward}", tip.z);
            }
            if !(deck || tip_forward) {
                if trace {
                    println!("    -> dropped: tip aft and not on the deck");
                }
                continue;
            }
            // AND IT IS RAISED. A roof rail, a hood scoop, a driveshaft
            // tunnel are thin tubes along Z that pass both tests above;
            // what they are not is clear of the surface under them. A
            // barrel sits off the deck by its own thickness or more, and
            // a gun pod beside a hull has nothing under it at all. Read
            // the other prims' points in the column beneath this one.
            // The column is under the MUZZLE half: at the breech a barrel
            // meets its mantlet with no gap at all, and reading there
            // called every tank's gun a roof rail.
            let r = mean_thick * 2.5 + 0.02;
            let (z0, z1) = if forward { (tip.z, tip.z + e.half.z) } else { (tip.z - e.half.z, tip.z) };
            let mut under_top = f32::MIN;
            for (j, q) in prims.iter().enumerate() {
                if j == i {
                    continue;
                }
                for v in q.iter() {
                    if v.y < e.lo.y && (v.x - e.c.x).abs() <= r && v.z >= z0 && v.z <= z1 {
                        // a concentric sleeve or a bore evacuator is the same
                        // tube in another prim, not a surface under it: skip
                        // anything hugging the barrel's own axis
                        let (dx, dy) = (v.x - e.c.x, v.y - e.c.y);
                        if (dx * dx + dy * dy).sqrt() < mean_thick * 1.6 {
                            continue;
                        }
                        under_top = under_top.max(v.y);
                    }
                }
            }
            let protrudes = tip.z <= hull.lo.z + hull.half.z * 0.06 || tip.z >= hull.hi.z - hull.half.z * 0.06;
            let raised = under_top == f32::MIN || (e.lo.y - under_top) >= mean_thick * 0.3;
            if trace {
                println!("    under_top {:.3} lo.y {:.3} gap {:.3} need {:.3} raised {raised} protrudes {protrudes}", under_top, e.lo.y, e.lo.y - under_top, mean_thick * 0.3);
            }
            if !(raised || protrudes) {
                if trace {
                    println!("    -> dropped: not raised off the surface under it");
                }
                continue;
            }
            hint += if forward { 1.0 } else { -1.0 };
            out.guns.push(Mount { kind: Kind::Gun, pos: tip, aim, prim: Some(i), confidence: conf });
            continue;
        }

        // ---- a nozzle: round, at the rear face, not a wheel ----
        let round = {
            let (a, b) = (e.half.x.max(1e-5), e.half.y.max(1e-5));
            (a / b).max(b / a) <= ROUND
        };
        // a nacelle can be most of a wingless fuselage's height, so the
        // thickness gate is against the hull's HEIGHT as well as its
        // width: whichever the hull is narrow in
        let nozzle_thick = e.half.x <= hull.half.y * 0.75 && thick_x <= 0.5;
        let nozzle_shape = round && !grounded && nozzle_thick && above >= THRUSTER_ABOVE && e.half.z <= hull.half.z * 0.5;
        if nozzle_shape && aft > THRUSTER_AFT {
            let (front_r, back_r) = end_radii(p, e.c, Vec3::Z, e.half.z.max(1e-4));
            let opens_aft = back_r >= front_r * 0.98;
            let conf = (0.5 + 0.5 * (aft - THRUSTER_AFT) / (1.0 - THRUSTER_AFT)).clamp(0.3, 1.0);
            if opens_aft {
                out.thrusters.push(Mount { kind: Kind::Thruster, pos: Vec3::new(e.c.x, e.c.y, e.hi.z), aim: Vec3::Z, prim: Some(i), confidence: conf });
                hint += 2.0;
            }
            continue;
        }
        if nozzle_shape && aft < -THRUSTER_AFT {
            // a round thing at the NOSE that flares forward is either an
            // exhaust opening the wrong way — the hull is loaded backwards
            // — or a headlight. Held until the whole hull has been read:
            // it only counts when nothing else says the front is the front.
            let (front_r, back_r) = end_radii(p, e.c, Vec3::Z, e.half.z.max(1e-4));
            if front_r > back_r * 1.05 {
                let conf = (0.5 + 0.5 * (-aft - THRUSTER_AFT) / (1.0 - THRUSTER_AFT)).clamp(0.3, 1.0) * 0.6;
                front_flares.push(Mount { kind: Kind::Thruster, pos: Vec3::new(e.c.x, e.c.y, e.lo.z), aim: Vec3::NEG_Z, prim: Some(i), confidence: conf });
                continue;
            }
        }

        // ---- a canopy: compact, high, forward ----
        let compact = e.half.z <= hull.half.z * 0.5 && thick_x <= COCKPIT_WIDE;
        // and on the centreline — a headlight is compact, high and forward too
        let centred = (e.c.x - hull.c.x).abs() <= hull.half.x * 0.35;
        if compact && centred && above >= COCKPIT_ABOVE && aft < 0.0 && e.n >= 24 {
            let conf = (above - COCKPIT_ABOVE) / (1.0 - COCKPIT_ABOVE) * 0.5 + (-aft).min(1.0) * 0.5;
            let m = Mount { kind: Kind::Cockpit, pos: e.c, aim: Vec3::NEG_Z, prim: Some(i), confidence: conf.clamp(0.1, 1.0) };
            if out.cockpit.map(|c| conf > c.confidence).unwrap_or(true) {
                out.cockpit = Some(m);
            }
            hint += 0.5;
        }
    }

    // ---- the front flares: backwards, or headlights? ----
    // With a gun pointing forward or a nozzle opening aft on record, a
    // flared round thing at the nose is a light and is dropped. With
    // neither, it is the only witness there is, and it says backwards.
    let front_is_front = out.guns.iter().any(|g| g.aim.z < 0.0) || out.thrusters.iter().any(|t| t.aim.z > 0.0);
    if !front_is_front {
        for m in front_flares {
            out.thrusters.push(m);
            hint -= 2.0;
        }
    }

    // ---- popouts: the upper hull, forward, mirrored ----
    let zf = hull.c.z - hull.half.z * POPOUT_FWD;
    // outboard on the BODY, not the wingspan: a fuselage is about as
    // wide as it is tall, so the height bounds the column
    let out_x = (hull.half.x * POPOUT_OUT).min(hull.half.y * 1.2);
    for side in [-1.0f32, 1.0] {
        let x0 = hull.c.x + side * out_x;
        // the actual top surface in a column there
        let col_w = hull.half.x * 0.12;
        let col_l = hull.half.z * 0.12;
        let top = all
            .iter()
            .filter(|p| (p.x - x0).abs() <= col_w && (p.z - zf).abs() <= col_l)
            .map(|p| p.y)
            .fold(f32::MIN, f32::max);
        let y = if top > f32::MIN { top } else { hull.hi.y };
        out.popouts.push(Mount { kind: Kind::Popout, pos: Vec3::new(x0, y, zf), aim: Vec3::NEG_Z, prim: None, confidence: 0.3 });
    }

    // guns: nearest the centreline first, so a single shot comes off the
    // main gun and pairs alternate
    out.guns.sort_by(|a, b| a.pos.x.abs().partial_cmp(&b.pos.x.abs()).unwrap_or(std::cmp::Ordering::Equal));
    out.forward_hint = hint.clamp(-1.0, 1.0);
    out
}

// ----------------------------------------------------------------- driver ---

/// `--hardpoints [name]`: read every chassis's vehicle pose off disk the
/// way the loader does (no GPU), run `find`, and print what each hull
/// carries. With a name, print that hull's mounts in full. This is the
/// loop the rules get iterated against: the synthetic tests are what the
/// rules were written to, the roster is what they have to survive.
pub fn run(argv: &[String]) {
    let want = argv.iter().position(|a| a == "--hardpoints").and_then(|i| argv.get(i + 1)).filter(|a| !a.starts_with("--"));
    let root = std::path::Path::new("assets/bots");
    let Ok(rd) = std::fs::read_dir(root) else {
        println!("hardpoints: no assets/bots here");
        return;
    };
    let mut names: Vec<String> = rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
    names.sort();
    let (mut n, mut with_guns, mut with_thr, mut with_cockpit, mut backwards, mut popout_only) = (0, 0, 0, 0, 0, 0);
    let mut backwards_names: Vec<String> = Vec::new();
    for name in names {
        if let Some(w) = &want {
            if *w != &name {
                continue;
            }
        }
        let dir = root.join(&name).join("f00");
        if !dir.join("scene.gltf").is_file() {
            continue;
        }
        let (raws, _half) = crate::model::load_raw(&dir, 1.0);
        let pts: Vec<Vec<Vec3>> = raws.iter().map(|r| r.verts.iter().map(|v| Vec3::from_array(v.pos)).collect()).collect();
        let refs: Vec<&[Vec3]> = pts.iter().map(|v| v.as_slice()).collect();
        let h = find_traced(&refs, want.is_some());
        n += 1;
        if !h.guns.is_empty() { with_guns += 1; } else { popout_only += 1; }
        if !h.thrusters.is_empty() { with_thr += 1; }
        if h.cockpit.is_some() { with_cockpit += 1; }
        if h.forward_hint < 0.0 { backwards += 1; backwards_names.push(name.clone()); }
        if want.is_some() {
            println!("{name}: {} prims", refs.len());
            print!("{}", h.report());
            // every prim's box, so a missed barrel can be found by eye
            let hull = ext(&pts.iter().flatten().copied().collect::<Vec<_>>()).unwrap();
            println!("  hull c ({:.2},{:.2},{:.2}) half ({:.2},{:.2},{:.2})", hull.c.x, hull.c.y, hull.c.z, hull.half.x, hull.half.y, hull.half.z);
            for (i, p) in refs.iter().enumerate() {
                if let Some(e) = ext(p) {
                    let mt = (e.half.x + e.half.y) * 0.5;
                    println!("  prim {:>2}: n={:>5} c ({:+.2},{:+.2},{:+.2}) half ({:.3},{:.3},{:.3}) aspect_z {:.1}", i, e.n, e.c.x, e.c.y, e.c.z, e.half.x, e.half.y, e.half.z, if mt > 1e-5 { e.half.z / mt } else { 0.0 });
                }
            }
        } else {
            println!(
                "{:<56} guns {:>2}  thrusters {:>2}  cockpit {}  hint {:+.1}{}",
                name, h.guns.len(), h.thrusters.len(), if h.cockpit.is_some() { "y" } else { "-" }, h.forward_hint,
                if h.forward_hint < 0.0 { "  BACKWARDS?" } else { "" }
            );
        }
    }
    if want.is_none() {
        println!();
        println!("{n} hulls: {with_guns} with guns ({popout_only} popout-only), {with_thr} with thrusters, {with_cockpit} with a cockpit, {backwards} the fittings call backwards");
        if !backwards_names.is_empty() {
            println!("backwards: {}", backwards_names.join(", "));
        }
    }
}

// ------------------------------------------------------------------ tests ---

#[cfg(test)]
mod tests {
    use super::*;

    /// A box of points, half extents `h` around `c`, sampled on its faces.
    fn boxp(c: Vec3, h: Vec3) -> Vec<Vec3> {
        let mut v = Vec::new();
        let n = 5;
        for i in 0..=n {
            for j in 0..=n {
                let (a, b) = (i as f32 / n as f32 * 2.0 - 1.0, j as f32 / n as f32 * 2.0 - 1.0);
                v.push(c + Vec3::new(a * h.x, b * h.y, -h.z));
                v.push(c + Vec3::new(a * h.x, b * h.y, h.z));
                v.push(c + Vec3::new(a * h.x, -h.y, b * h.z));
                v.push(c + Vec3::new(a * h.x, h.y, b * h.z));
                v.push(c + Vec3::new(-h.x, a * h.y, b * h.z));
                v.push(c + Vec3::new(h.x, a * h.y, b * h.z));
            }
        }
        v
    }

    /// A tube along Z from z0 to z1 at (x, y) with radius r0 at z0 and
    /// r1 at z1 — a tapered barrel or a flared nozzle.
    fn tube(x: f32, y: f32, z0: f32, z1: f32, r0: f32, r1: f32) -> Vec<Vec3> {
        let mut v = Vec::new();
        for k in 0..=12 {
            let t = k as f32 / 12.0;
            let z = z0 + (z1 - z0) * t;
            let r = r0 + (r1 - r0) * t;
            for a in 0..8 {
                let th = a as f32 / 8.0 * std::f32::consts::TAU;
                v.push(Vec3::new(x + r * th.cos(), y + r * th.sin(), z));
            }
        }
        v
    }

    #[test]
    fn a_tank_has_a_gun_over_the_deck_and_the_muzzle_is_the_narrow_end() {
        // hull 2 wide, 1 tall, 4 long; barrel forward from the turret
        let hull = boxp(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 2.0));
        let turret = boxp(Vec3::new(0.0, 1.2, 0.2), Vec3::new(0.6, 0.25, 0.6));
        let barrel = tube(0.0, 1.25, -3.2, -0.4, 0.06, 0.09); // narrow at the nose
        let prims: Vec<&[Vec3]> = vec![&hull, &turret, &barrel];
        let h = find(&prims);
        assert_eq!(h.guns.len(), 1, "guns: {:?}", h.guns);
        let g = h.guns[0];
        assert_eq!(g.prim, Some(2));
        assert!(g.aim.z < 0.0, "muzzle aims aft");
        assert!((g.pos.z + 3.2).abs() < 0.05, "muzzle not at the tip: {:?}", g.pos);
        assert!(h.thrusters.is_empty());
        assert!(h.forward_hint > 0.0);
        assert!(h.muzzle(0).unwrap().kind == Kind::Gun);
    }

    #[test]
    fn a_jet_has_two_nozzles_at_the_tail_and_a_canopy_up_front() {
        let fuselage = boxp(Vec3::new(0.0, 0.6, 0.0), Vec3::new(0.5, 0.4, 3.0));
        let wing = boxp(Vec3::new(0.0, 0.5, 0.5), Vec3::new(3.0, 0.05, 0.8));
        let noz_l = tube(-0.35, 0.6, 2.4, 3.3, 0.18, 0.24); // flares aft
        let noz_r = tube(0.35, 0.6, 2.4, 3.3, 0.18, 0.24);
        let canopy = boxp(Vec3::new(0.0, 1.15, -1.6), Vec3::new(0.25, 0.18, 0.5));
        let prims: Vec<&[Vec3]> = vec![&fuselage, &wing, &noz_l, &noz_r, &canopy];
        let h = find(&prims);
        assert_eq!(h.thrusters.len(), 2, "thrusters: {:?}", h.thrusters);
        for t in &h.thrusters {
            assert!(t.aim.z > 0.0, "a nozzle opens aft");
            assert!(t.pos.z > 3.0, "flame not at the exit: {:?}", t.pos);
        }
        assert!(h.cockpit.is_some(), "no canopy found");
        assert!(h.cockpit.unwrap().pos.z < 0.0);
        assert!(h.guns.is_empty(), "a wing is not a gun: {:?}", h.guns);
        assert!(h.forward_hint > 0.5);
        // no guns: the popouts stand in, on the hull's own top surface
        let m = h.muzzle(0).unwrap();
        assert_eq!(m.kind, Kind::Popout);
        assert!((m.pos.y - 1.0).abs() < 0.05, "popout not on the top surface: {:?}", m.pos);
        assert_ne!(h.muzzle(0).unwrap().pos.x, h.muzzle(1).unwrap().pos.x, "popouts do not alternate sides");
    }

    #[test]
    fn a_hull_loaded_backwards_is_called_backwards_by_its_nozzles() {
        let fuselage = boxp(Vec3::new(0.0, 0.6, 0.0), Vec3::new(0.5, 0.4, 3.0));
        // nozzles at the -Z end, flaring toward -Z: this model faces +Z
        let noz = tube(0.0, 0.6, -3.3, -2.4, 0.24, 0.18);
        let prims: Vec<&[Vec3]> = vec![&fuselage, &noz];
        let h = find(&prims);
        assert!(h.forward_hint < 0.0, "hint {}", h.forward_hint);
    }

    #[test]
    fn a_racer_with_nothing_gun_shaped_gets_popouts_and_no_guns() {
        let body = boxp(Vec3::new(0.0, 0.4, 0.0), Vec3::new(0.9, 0.4, 2.2));
        let wheels: Vec<Vec<Vec3>> = [(-0.9, -1.4), (0.9, -1.4), (-0.9, 1.4), (0.9, 1.4)]
            .iter()
            .map(|(x, z)| tube(*x, 0.3, z - 0.3, z + 0.3, 0.3, 0.3))
            .collect();
        let mut prims: Vec<&[Vec3]> = vec![&body];
        for w in &wheels {
            prims.push(w);
        }
        let h = find(&prims);
        assert!(h.guns.is_empty(), "{:?}", h.guns);
        assert!(h.thrusters.is_empty(), "a wheel read as a thruster: {:?}", h.thrusters);
        assert_eq!(h.popouts.len(), 2);
        assert!(h.popouts.iter().all(|p| p.pos.z < 0.0 && p.pos.y > 0.7));
    }

    #[test]
    fn an_empty_hull_has_nothing_and_does_not_panic() {
        let h = find(&[]);
        assert!(h.muzzle(0).is_none());
        assert_eq!(h.forward_hint, 0.0);
    }
}
