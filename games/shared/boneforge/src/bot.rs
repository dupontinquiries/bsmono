//! A vehicle in, a rigged bot out.
//!
//! The skeleton is a template scaled to the vehicle, the armour is generated
//! from how much panel each bone was allocated, and the plate section is
//! borrowed from the panels themselves. Nothing here is authored per vehicle,
//! which is the point: a library of cars should give a library of bots.

use crate::anatomy::{self, Body, End, Kind, Part, Side};
use crate::island::Island;
use crate::rig::{along, Bone, Pose};

/// A humanoid, proportioned relative to the machine's standing height.
///
/// Lengths are fractions of that height so one template fits a hatchback and
/// a truck. A 458-long car should stand about its own length tall: at 64% the
/// panels had to be shrunk to a third of their size and the machine looked
/// like it was wearing a scale model of the car it came from.
pub const STAND_FRACTION: f32 = 0.86;

pub fn skeleton(height: f32) -> Vec<Bone> {
    let h = height;
    let b = |name, parent, len, rest: [f32; 3], radius| Bone {
        name, parent, len: len * h, rest, radius: radius * h,
    };
    vec![
        b("root",      None,     0.000, [0.0, 0.0, 0.0],  0.000),
        b("pelvis",    Some(0),  0.066, [0.0, 1.0, 0.0],  0.054),
        b("spine",     Some(1),  0.164, [0.0, 1.0, 0.0],  0.050),
        b("chest",     Some(2),  0.157, [0.0, 1.0, 0.0],  0.061),
        b("neck",      Some(3),  0.064, [0.0, 1.0, 0.0],  0.021),
        b("head",      Some(4),  0.071, [0.0, 1.0, 0.0],  0.039),
        b("hip_l",     Some(1),  0.076, [0.0, 0.0,-1.0],  0.036),
        b("thigh_l",   Some(6),  0.242, [0.0,-1.0, 0.0],  0.046),
        b("shin_l",    Some(7),  0.228, [0.0,-1.0, 0.0],  0.039),
        b("foot_l",    Some(8),  0.107, [-1.0,0.0, 0.0],  0.032),
        b("hip_r",     Some(1),  0.076, [0.0, 0.0, 1.0],  0.036),
        b("thigh_r",   Some(10), 0.242, [0.0,-1.0, 0.0],  0.046),
        b("shin_r",    Some(11), 0.228, [0.0,-1.0, 0.0],  0.039),
        b("foot_r",    Some(12), 0.107, [-1.0,0.0, 0.0],  0.032),
        b("clav_l",    Some(3),  0.087, [0.0, 0.0,-1.0],  0.029),
        b("uparm_l",   Some(14), 0.193, [0.0,-1.0, 0.0],  0.036),
        b("forearm_l", Some(15), 0.171, [0.0,-1.0, 0.0],  0.030),
        b("hand_l",    Some(16), 0.064, [0.0,-1.0, 0.0],  0.025),
        b("clav_r",    Some(3),  0.087, [0.0, 0.0, 1.0],  0.029),
        b("uparm_r",   Some(18), 0.193, [0.0,-1.0, 0.0],  0.036),
        b("forearm_r", Some(19), 0.171, [0.0,-1.0, 0.0],  0.030),
        b("hand_r",    Some(20), 0.064, [0.0,-1.0, 0.0],  0.025),
    ]
}

pub fn bone_index(bones: &[Bone], name: &str) -> Option<usize> {
    bones.iter().position(|b| b.name == name)
}

/// Where a part goes, decided by what it is and where it sat on the vehicle.
///
/// The mapping is the one the geometry supports rather than a taste: the rear
/// axle is already where the heels need to be, the front axle lands at the
/// shoulders, and the widest panel on the vehicle belongs on the widest flat
/// place on a body.
pub fn assign(p: &Part) -> &'static str {
    let side = |l: &'static str, r: &'static str| if p.side == Side::L { l } else { r };
    match (p.kind, p.end) {
        (Kind::Wheel, End::Front) => side("clav_l", "clav_r"),
        (Kind::Wheel, _)          => side("shin_l", "shin_r"),
        (Kind::Glass, _)          => "head",
        (Kind::Beam, End::Rear)   => side("thigh_l", "thigh_r"),
        (Kind::Beam, _)           => "spine",
        (Kind::Panel, End::Front) => "chest",
        (Kind::Panel, End::Mid)   => if p.side == Side::C { "spine" }
                                     else { side("clav_l", "clav_r") },
        (Kind::Panel, End::Rear)  => "pelvis",
        (Kind::Greeble, End::Front) => "head",
        (Kind::Greeble, End::Rear)  => side("shin_l", "shin_r"),
        (Kind::Greeble, _)          => side("forearm_l", "forearm_r"),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Plate {
    pub bone: usize,
    /// In the bone's own frame.
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    /// width around, length along, thickness.
    pub size: [f32; 3],
    /// Which interleave shell, for the depth bias that stops coplanar plates
    /// fighting. Geometry offsets alone cannot: two plates meeting at a
    /// shallow angle are coplanar over part of their area however far apart
    /// their centres are.
    pub shell: u8,
    pub crown: f32,
    pub taper: f32,
    /// 0..1, when this plate arrives.
    pub phase: f32,
}

/// Rows around, rows along, and how much of the bone to cover.
fn plan(name: &str) -> (usize, usize, f32) {
    match name {
        "chest"  => (6, 9, 0.88),
        "spine"  => (6, 7, 0.92),
        "pelvis" => (6, 5, 0.88),
        "head"   => (5, 4, 0.72),
        n if n.starts_with("clav")    => (4, 4, 0.82),
        n if n.starts_with("uparm")   => (5, 6, 0.84),
        n if n.starts_with("forearm") => (5, 6, 0.82),
        n if n.starts_with("thigh")   => (5, 8, 0.86),
        n if n.starts_with("shin")    => (5, 7, 0.84),
        n if n.starts_with("foot")    => (6, 5, 0.76),
        _ => (0, 0, 0.0),
    }
}

/// Generate scale armour for one bone from the panel area allocated to it.
///
/// Plates are wide and short -- their long axis runs AROUND the limb. Built
/// the other way they read as vertical slats, which is what a scale is not.
pub fn plates(bi: usize, b: &Bone, area: f32, crown: f32, taper: f32) -> Vec<Plate> {
    let (rows, per, cover) = plan(b.name);
    if rows == 0 { return vec![]; }
    let band = std::f32::consts::TAU * b.radius * b.len * cover;
    // A bone with no panel allocated is still clad -- in plain plate rather
    // than in confetti.
    let area = area.max(band * 0.95);
    let frac = (area / band.max(1.0)).sqrt().clamp(0.85, 1.45);

    // two axes perpendicular to the bone, for the ring the plates sit on
    let n = {
        let l = (b.rest[0].powi(2) + b.rest[1].powi(2) + b.rest[2].powi(2)).sqrt().max(1e-6);
        [b.rest[0] / l, b.rest[1] / l, b.rest[2] / l]
    };
    let k = (0..3).min_by(|&a, &c| n[a].abs().partial_cmp(&n[c].abs()).unwrap()).unwrap();
    let mut t = [0.0f32; 3];
    t[k] = 1.0;
    let d = t[0] * n[0] + t[1] * n[1] + t[2] * n[2];
    let mut rx = [t[0] - n[0] * d, t[1] - n[1] * d, t[2] - n[2] * d];
    let rl = (rx[0].powi(2) + rx[1].powi(2) + rx[2].powi(2)).sqrt().max(1e-6);
    for v in rx.iter_mut() { *v /= rl; }
    let rz = [n[1] * rx[2] - n[2] * rx[1],
              n[2] * rx[0] - n[0] * rx[2],
              n[0] * rx[1] - n[1] * rx[0]];

    let mut out = Vec::with_capacity(rows * per);
    for j in 0..per {
        let v = (j as f32 + 0.5) / per as f32;
        let u = 0.5 - cover * 0.5 + cover * v;
        let ax = along(b, u);
        // wider at the ends, because a housing bulges at every joint and a
        // pauldron is bigger than the plate halfway down the arm
        let edge = 1.0 - v.min(1.0 - v) * 2.0;
        let bulge = 1.0 + 0.40 * edge * edge;
        let stagger = if j % 2 == 1 { 0.5 } else { 0.0 };
        for i in 0..rows {
            let a = std::f32::consts::TAU * ((i as f32 + stagger) / rows as f32);
            let thick = b.radius * 0.11 * frac.min(1.6) * bulge;
            // offsets CYCLE rather than accumulate: separated by a plate
            // thickness, total standoff about one and a half
            let phase_a = (i % 3) as f32;
            let step = thick * (0.62 * (j % 2) as f32 + 0.44 * phase_a);
            let r = b.radius + thick * 0.55 + step;
            let (c, s) = (a.cos() * r, a.sin() * r);
            out.push(Plate {
                bone: bi,
                pos: [ax[0] + rx[0] * c + rz[0] * s,
                      ax[1] + rx[1] * c + rz[1] * s,
                      ax[2] + rx[2] * c + rz[2] * s],
                normal: [rx[0] * a.cos() + rz[0] * a.sin(),
                         rx[1] * a.cos() + rz[1] * a.sin(),
                         rx[2] * a.cos() + rz[2] * a.sin()],
                size: [std::f32::consts::TAU * b.radius / rows as f32 * frac * 1.05 * bulge,
                       b.len * cover / per as f32 * 1.55,
                       thick],
                shell: ((i % 3) + 3 * (j % 2)) as u8,
                crown, taper,
                phase: 0.06 + 0.38 * v + 0.08 * (i as f32 / rows as f32)
                       + 0.022 * phase_a,
            });
        }
    }
    out
}

/// A road wheel is about 0.65 m across whatever the file says.
///
/// This is the only physical constant in the pipeline, and it earns its place
/// twice. It tells you the file's UNITS -- one vehicle in the library is
/// modelled in metres while the rest are in centimetres, and nothing in the
/// geometry says which -- and it tells the engine how big to draw the bot,
/// because `load_raw` normalises every model's longest extent to 1.32 and
/// `chassis.json`'s scale is the only thing that can undo that. Left at 1.0,
/// a bot from a truck and one from a hatchback arrive the same size.
pub const WHEEL_METRES: f32 = 0.65;

/// What the engine normalises a model's longest extent to.
pub const ENGINE_UNIT: f32 = 1.32;

pub struct Bot {
    pub bones: Vec<Bone>,
    pub plates: Vec<Plate>,
    pub height: f32,
    /// Per island: which bone carries it, and what it is.
    pub carried: Vec<(usize, Part)>,
    pub body: Body,
    /// File units per metre, from the wheels. `None` when there were none to
    /// measure -- a vehicle with no recognisable wheels gives no scale
    /// reference, and guessing one from its bounding box would be inventing
    /// a fact.
    pub units_per_m: Option<f32>,
}

impl Bot {
    /// The bot's real height in metres, if the wheels said what a unit is.
    pub fn height_m(&self) -> Option<f32> {
        self.units_per_m.map(|u| self.height / u)
    }
    /// What `chassis.json` should carry, so bots keep their relative sizes
    /// after the engine has normalised them all to the same extent.
    pub fn chassis_scale(&self) -> f32 {
        match self.height_m() {
            Some(m) if m.is_finite() && m > 0.1 => (m / ENGINE_UNIT).clamp(0.4, 6.0),
            _ => 1.0,
        }
    }
}

/// The whole pipeline: a vehicle mesh becomes a rigged, clad bot.
pub fn build(islands: &[Island]) -> Bot {
    let body = anatomy::frame(islands);
    let vsize = body.size();
    let height = vsize[body.long_axis] * STAND_FRACTION;
    let bones = skeleton(height);

    let mut carried = Vec::with_capacity(islands.len());
    let mut area = vec![0.0f32; bones.len()];
    let mut crown = vec![(0.0f32, 0.0f32); bones.len()];
    for is in islands {
        let p = anatomy::classify(is, &body);
        let bi = bone_index(&bones, assign(&p)).unwrap_or(3);
        carried.push((bi, p));
        let s = is.sorted_size();
        let w = s[0] * s[1];
        area[bi] += w;
        crown[bi].0 += p.crown * w;
        crown[bi].1 += w;
    }

    // Mirrored bones share a section. A vehicle is not symmetric in what it
    // donates -- one sill, one scoop -- and a machine whose left leg is domed
    // and whose right is flat reads as a mistake, not as character.
    let mut sect: Vec<(f32, f32)> = crown.iter().map(|&(c, w)| {
        if w > 0.0 { (c / w, 0.55) } else { (0.20, 0.55) }
    }).collect();
    for i in 0..bones.len() {
        if let Some(stem) = bones[i].name.strip_suffix("_l") {
            if let Some(j) = bones.iter().position(|b| b.name == format!("{stem}_r")) {
                let m = ((sect[i].0 + sect[j].0) * 0.5, (sect[i].1 + sect[j].1) * 0.5);
                sect[i] = m;
                sect[j] = m;
                let a = (area[i] + area[j]) * 0.5;
                area[i] = a;
                area[j] = a;
            }
        }
    }

    // units, from the wheels
    let parts: Vec<Part> = islands.iter().map(|i| anatomy::classify(i, &body)).collect();
    let cs = anatomy::corners(islands, &parts, &body);
    let units_per_m = if cs.is_empty() { None } else {
        let mut d: Vec<f32> = cs.iter()
            .map(|c| c.iter().map(|&i| islands[i].sorted_size()[0])
                      .fold(0.0f32, f32::max))
            .collect();
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = d[d.len() / 2];
        if med > 1e-6 { Some(med / WHEEL_METRES) } else { None }
    };

    let mut plates = Vec::new();
    for (i, b) in bones.iter().enumerate() {
        if b.len <= 0.0 { continue; }
        plates.extend(plates_for(i, b, area[i], sect[i]));
    }
    Bot { bones, plates, height, carried, body, units_per_m }
}

fn plates_for(i: usize, b: &Bone, area: f32, sect: (f32, f32)) -> Vec<Plate> {
    plates(i, b, area, sect.0.clamp(0.06, 0.55), sect.1.clamp(0.25, 0.95))
}

/// A standing pose, so a freshly built bot has something to be looked at in.
pub fn stand(bones: &[Bone], height: f32) -> Pose {
    let mut p = Pose::rest(bones.len());
    p.root_pos = [0.0, height * 0.46, 0.0];
    let set = |p: &mut Pose, n: &str, a: [f32; 3]| {
        if let Some(i) = bone_index(bones, n) { p.angles[i] = a; }
    };
    set(&mut p, "hip_l", [0.0, 0.0, -8.0]);
    set(&mut p, "hip_r", [0.0, 0.0, 8.0]);
    set(&mut p, "thigh_l", [-4.0, 0.0, 3.0]);
    set(&mut p, "thigh_r", [-4.0, 0.0, -3.0]);
    set(&mut p, "shin_l", [0.0, 0.0, -6.0]);
    set(&mut p, "shin_r", [0.0, 0.0, -6.0]);
    set(&mut p, "clav_l", [0.0, 0.0, 10.0]);
    set(&mut p, "clav_r", [0.0, 0.0, -10.0]);
    set(&mut p, "uparm_l", [0.0, 0.0, -14.0]);
    set(&mut p, "uparm_r", [0.0, 0.0, 14.0]);
    set(&mut p, "forearm_l", [0.0, 0.0, -18.0]);
    set(&mut p, "forearm_r", [0.0, 0.0, 18.0]);
    p
}

/// The get-up: a machine folded inside a vehicle stands the way a body does.
///
/// Prone, spread, push-up, kneel, rise. Authored as ANGLES so each pose is a
/// sentence about a body rather than a list of endpoints, and because bone
/// lengths are fixed there is no other way to say it.
///
/// The tilt runs monotonically from face-down to upright. Every pose must tip
/// the SAME way: mixing signs once rotated the body 176 degrees through
/// vertical between two frames -- a front somersault performed from inside a
/// car.
pub fn getup(bones: &[Bone], height: f32) -> Vec<Pose> {
    let set = |p: &mut Pose, n: &str, a: [f32; 3]| {
        if let Some(i) = bone_index(bones, n) { p.angles[i] = a; }
    };
    let h = height;

    let mut prone = Pose::rest(bones.len());
    // flat: the folded volume under a vehicle's floor is a few percent of its
    // length tall, so this pose is effectively two-dimensional
    prone.root_pos = [0.0, h * 0.055, 0.0];
    prone.root_rot = [0.0, 0.0, -90.0];
    for (n, a) in [
        ("pelvis", [0.0, 0.0, 0.0]), ("spine", [0.0, 0.0, -4.0]),
        ("chest", [0.0, 0.0, 2.0]), ("neck", [0.0, 0.0, 10.0]),
        ("head", [0.0, 0.0, 12.0]),
        ("clav_l", [16.0, 0.0, 0.0]), ("clav_r", [-16.0, 0.0, 0.0]),
        ("uparm_l", [0.0, -62.0, 0.0]), ("uparm_r", [0.0, 62.0, 0.0]),
        ("forearm_l", [0.0, -108.0, 0.0]), ("forearm_r", [0.0, 108.0, 0.0]),
        ("thigh_l", [0.0, -14.0, 0.0]), ("thigh_r", [0.0, 14.0, 0.0]),
        ("shin_l", [0.0, -30.0, 0.0]), ("shin_r", [0.0, 30.0, 0.0]),
        ("foot_l", [0.0, -20.0, 0.0]), ("foot_r", [0.0, 20.0, 0.0]),
    ] { set(&mut prone, n, a); }

    let mut spread = prone.clone();
    spread.root_pos = [h * -0.02, h * 0.09, 0.0];
    spread.root_rot = [0.0, 0.0, -86.0];
    for (n, a) in [
        ("clav_l", [34.0, 0.0, 0.0]), ("clav_r", [-34.0, 0.0, 0.0]),
        ("uparm_l", [-26.0, 0.0, 78.0]), ("uparm_r", [26.0, 0.0, 78.0]),
        ("forearm_l", [0.0, 0.0, 22.0]), ("forearm_r", [0.0, 0.0, 22.0]),
        ("thigh_l", [-34.0, 0.0, 74.0]), ("thigh_r", [34.0, 0.0, 74.0]),
        ("shin_l", [0.0, 0.0, 14.0]), ("shin_r", [0.0, 0.0, 14.0]),
    ] { set(&mut spread, n, a); }

    // arms straighten and take the weight; the chest comes off the floor
    // because they pushed it, not because it was raised
    let mut pushup = spread.clone();
    pushup.root_pos = [h * -0.09, h * 0.19, 0.0];
    pushup.root_rot = [0.0, 0.0, -64.0];
    for (n, a) in [
        ("uparm_l", [-12.0, 0.0, 76.0]), ("uparm_r", [12.0, 0.0, 76.0]),
        ("forearm_l", [0.0, 0.0, 2.0]), ("forearm_r", [0.0, 0.0, 2.0]),
        // ASYMMETRIC from here: the left tucks under as the load-bearing leg
        // and the right swings clear, already travelling. Committing at the
        // kneel instead means dragging a leg out from under a loaded body.
        ("thigh_l", [-8.0, 0.0, 84.0]), ("thigh_r", [26.0, 0.0, 48.0]),
        ("shin_l", [0.0, 0.0, 14.0]), ("shin_r", [0.0, 0.0, -22.0]),
    ] { set(&mut pushup, n, a); }

    let mut kneel = pushup.clone();
    kneel.root_pos = [h * -0.05, h * 0.31, 0.0];
    kneel.root_rot = [0.0, 0.0, -24.0];
    for (n, a) in [
        ("spine", [-14.0, 0.0, 0.0]), ("chest", [-10.0, 0.0, 0.0]),
        ("thigh_l", [-8.0, 0.0, 74.0]), ("thigh_r", [8.0, 0.0, 14.0]),
        ("shin_l", [0.0, 0.0, -64.0]), ("shin_r", [0.0, 0.0, 30.0]),
        ("uparm_l", [-30.0, 0.0, 26.0]), ("uparm_r", [-44.0, 0.0, 20.0]),
    ] { set(&mut kneel, n, a); }

    let mut rise = stand(bones, h);
    rise.root_rot = [0.0, 0.0, 0.0];

    // the back straightens LAST: standing is the end of the motion
    vec![prone, spread, pushup, kneel, rise]
}

/// Resample the key poses into `n` frames for a fold strip.
///
/// Five to seven. Every pose is a real model and gets its own SDF bake, so
/// they cost startup time -- short and readable beats smooth.
pub fn strip_poses(bones: &[Bone], height: f32, n: usize) -> Vec<Pose> {
    let keys = getup(bones, height);
    if n <= keys.len() { return keys.into_iter().take(n.max(2)).collect(); }
    (0..n).map(|k| {
        let u = k as f32 / (n - 1) as f32 * (keys.len() - 1) as f32;
        let i = (u as usize).min(keys.len() - 2);
        let f = u - i as f32;
        keys[i].lerp(&keys[i + 1], f * f * (3.0 - 2.0 * f))
    }).collect()
}

/// What the robot form is FOR.
///
/// The vehicle already owns speed and distance, so a robot form that is built
/// to run is duplicating the other form's job -- with the same fixed mass,
/// because it is the same car either way. Every kilogram in long legs is a
/// kilogram not in the arms, the shoulders, or the weapon. The robot form
/// should specialise in what a vehicle cannot do: grip, strike, brace, carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The original template: legs 47% of height. A sprinter's proportions.
    Generic,
    /// Close fighting. Arms longer than legs, low hips, broad and thick-set.
    /// Short legs are not slow for footwork -- a boxer's steps are short --
    /// they only lose STRIDE, and covering distance is the vehicle's job.
    Brawler,
    /// A heavy weapon. Shoulders built to take recoil, a braced wide stance,
    /// long feet for the fore-aft moment a gun actually puts into a body.
    Gunner,
}

/// Bone lengths and radii as fractions of height, per role.
/// (pelvis, spine, chest, neck, head, hip, thigh, shin, foot,
///  clav, uparm, forearm, hand)
fn props(role: Role) -> ([f32; 13], [f32; 13]) {
    match role {
        Role::Generic => (
            [0.066, 0.164, 0.157, 0.064, 0.071, 0.076, 0.242, 0.228, 0.107,
             0.087, 0.193, 0.171, 0.064],
            [0.054, 0.050, 0.061, 0.021, 0.039, 0.036, 0.046, 0.039, 0.032,
             0.029, 0.036, 0.030, 0.025]),
        Role::Brawler => (
            // legs 0.29 of height; arms 0.54 -- longer than the legs, which
            // is the gorilla's answer to the same problem
            [0.090, 0.215, 0.235, 0.045, 0.085, 0.110, 0.150, 0.140, 0.150,
             0.120, 0.240, 0.215, 0.085],
            [0.075, 0.068, 0.090, 0.034, 0.045, 0.050, 0.070, 0.062, 0.042,
             0.042, 0.058, 0.055, 0.045]),
        Role::Gunner => (
            [0.085, 0.205, 0.230, 0.050, 0.075, 0.125, 0.165, 0.155, 0.180,
             0.135, 0.200, 0.180, 0.070],
            [0.072, 0.066, 0.095, 0.036, 0.042, 0.052, 0.072, 0.066, 0.050,
             0.050, 0.060, 0.050, 0.040]),
    }
}

pub fn skeleton_for(height: f32, role: Role) -> Vec<Bone> {
    let (l, r) = props(role);
    let mut b = skeleton(height);
    let map = |n: &str| -> usize {
        match n {
            "pelvis" => 0, "spine" => 1, "chest" => 2, "neck" => 3, "head" => 4,
            x if x.starts_with("hip") => 5, x if x.starts_with("thigh") => 6,
            x if x.starts_with("shin") => 7, x if x.starts_with("foot") => 8,
            x if x.starts_with("clav") => 9, x if x.starts_with("uparm") => 10,
            x if x.starts_with("forearm") => 11, x if x.starts_with("hand") => 12,
            _ => usize::MAX,
        }
    };
    for bone in b.iter_mut() {
        let k = map(bone.name);
        if k != usize::MAX {
            bone.len = l[k] * height;
            bone.radius = r[k] * height;
        }
    }
    b
}

/// The numbers the argument rests on, for a bot standing square.
#[derive(Debug, Clone, Copy)]
pub struct Build {
    /// Leg length over height.
    pub legs: f32,
    /// Arm length over leg length. Above 1 is a brawler's reach.
    pub arm_to_leg: f32,
    /// Centre of mass height over total height. Lower is harder to move.
    pub com: f32,
    /// Horizontal push at shoulder height, as a fraction of the bot's own
    /// weight, before it tips over the front of its feet. This is recoil
    /// capacity, and it is also what a punch that lands has to be absorbed by.
    pub brace: f32,
}

pub fn measure(role: Role) -> Build {
    let h = 100.0;
    let b = skeleton_for(h, role);
    let len = |n: &str| b[bone_index(&b, n).unwrap()].len;
    let legs = len("thigh_l") + len("shin_l");
    let arms = len("uparm_l") + len("forearm_l") + len("hand_l");
    // mass ~ length x radius^2, placed at each bone's midpoint when standing
    let (mut m, mut my) = (0.0f32, 0.0f32);
    let foot_y = 0.0;
    let hip_y = legs;
    let mut y = hip_y;
    let seg = |n: &str| { let x = &b[bone_index(&b, n).unwrap()]; x.len * x.radius * x.radius };
    // legs, both sides
    for (n, y0) in [("shin_l", foot_y), ("thigh_l", len("shin_l"))] {
        let w = seg(n) * 2.0;
        m += w; my += w * (y0 + len(n) * 0.5);
    }
    for n in ["pelvis", "spine", "chest", "neck", "head"] {
        let w = seg(n);
        m += w; my += w * (y + len(n) * 0.5);
        y += len(n);
    }
    let shoulder_y = hip_y + len("pelvis") + len("spine") + len("chest");
    // arms hang from the shoulder, both sides
    let mut ay = shoulder_y;
    for n in ["uparm_l", "forearm_l", "hand_l"] {
        let w = seg(n) * 2.0;
        m += w; my += w * (ay - len(n) * 0.5);
        ay -= len(n);
    }
    let com = my / m;
    // tipping: push F at shoulder height, pivot at the toe, restoring moment
    // is weight times the distance from the CoM to the toe -- half a foot
    let brace = (len("foot_l") * 0.5) / shoulder_y;
    Build { legs: legs / h, arm_to_leg: arms / legs, com: com / h, brace }
}
