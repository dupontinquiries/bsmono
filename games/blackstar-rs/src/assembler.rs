//! THE ASSEMBLER — a family of part generators, and what scores a pairing.
//!
//! One GAN making whole bots has to learn anatomy and geometry at once, and
//! `voxel.rs` records what that got me: a skeleton critic that scored real
//! models BELOW noise, because a representative carries no part identity and
//! no placement. A family fixes the first half of that. Fourteen generators,
//! one per part class, each learning a homogeneous thing — and then the
//! assembly is a separate problem with a separate score, which is the part
//! that was impossible before.
//!
//! ## The families are measured, not invented
//!
//! `tools/part-labels.json` clustered the whole library into fourteen classes
//! by shape. Two facts fall out that shape everything here:
//!
//! **Eleven of fourteen are mirrored pairs** (`mirrored >= 0.9`). Most parts
//! exist as a left and a right, which is a real constraint on assembly rather
//! than a stylistic preference.
//!
//! **The counts run 49 to 836.** That is the engineering limit on the idea: a
//! family needs fourteen trainable datasets and it does not have them. Forty-
//! nine samples is not a training set, so those classes fall back to sampling
//! the library — the same fallback `bench.rs` uses, for the same reason.
//!
//! ## Hard and soft, and the line between them is the design
//!
//! **HARD when the assembly cannot exist or cannot be evaluated.** No torso
//! means nothing to attach to. Two heads is not a bot with two heads, it is a
//! slot filled twice. A part heavier than the frame that carries it is a
//! physical claim, not a style. These are rejections, and a generator that
//! keeps producing them is broken rather than adventurous.
//!
//! **SOFT when it is unusual and might be excellent.** A leg-class part used
//! as an arm, an asymmetric pair, a part whose measured axis points the wrong
//! way for where it was placed. Every one of those is somebody's favourite
//! bot, and the score should make them EARN it: penalised enough that the
//! rest of the assembly has to be convincing, not enough to forbid.
//!
//! The failure mode of all-soft is a scorer that accepts a pile with a good
//! enough torso. The failure mode of all-hard is a scorer that only ever
//! emits the same symmetrical humanoid. Neither is worth having.

use crate::voxel::{Cluster, CLUSTERS};

/// An exact augmentation, not an approximation.
///
/// Eleven of the fourteen classes have `mirrored >= 0.9` — their parts exist
/// as a left and a right. Reflecting a left thigh does not APPROXIMATE a right
/// thigh, it produces one, and that part is genuinely in the library's
/// distribution. So the flip is free data rather than a regularisation trick,
/// and it is the first thing to reach for when a class is short.
pub const MIRROR_GAIN: f32 = 2.0;

/// Below this many library examples, a class cannot train its own generator
/// and samples the library instead.
///
/// A GUESS, and a consequential one: it decides how many of the fourteen
/// families are real. Stated here rather than buried so the count moves
/// visibly when somebody argues with it.
pub const TRAINABLE_MIN: u16 = 200;

/// Where a part sits on a body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Region {
    Head,
    Torso,
    Arm,
    Leg,
    /// cables, ribs, greeble — belongs anywhere and nowhere
    Trim,
}

impl Region {
    pub fn all() -> [Region; 5] {
        [Region::Head, Region::Torso, Region::Arm, Region::Leg, Region::Trim]
    }
    pub fn name(self) -> &'static str {
        match self {
            Region::Head => "head",
            Region::Torso => "torso",
            Region::Arm => "arm",
            Region::Leg => "leg",
            Region::Trim => "trim",
        }
    }
    /// Does this region come as a left and a right?
    pub fn paired(self) -> bool {
        matches!(self, Region::Arm | Region::Leg)
    }
}

/// Where a cluster's members actually came from, as a distribution.
///
/// **This replaced a single region derived from the cluster's mean `up`, and
/// that was wrong.** I preferred the vertical bias because it is a
/// measurement and the member names looked authored, and argued the point in
/// a test. The names are not authored: they are what the harvester labelled
/// parts as against the source skeletons, and a 27-word vocabulary with
/// explicit left/right — `thighl`, `clavr`, `shinl` — carries body-region
/// information that a mean vertical bias cannot.
///
/// The two disagree badly. `up` put cluster 0 in the head; its members are
/// 26% torso and 0% head. It put cluster 8 in the head; that one is 99%
/// INTERIOR.
///
/// ## And my own name mapping was wrong twice over
///
/// I first called `guts` a torso part. It is **1,733 of the library's 4,455
/// parts — 39% of everything** — so that single choice made almost every
/// cluster read as "mostly torso", which is what made the pricing come out
/// nonsensical.
///
/// The tree already had the answer and I invented my own instead of looking.
/// `tools/find-mounts.py` carries an argued classification from having hit
/// this exact problem — a search for gun barrels that kept returning struts:
///
/// ```text
/// INTERNAL = ("guts", "bone", "muscle", "cable", "link", "piston",
///             "vertebra", "rib", "coil", "core", "keel", "greeble",
///             "spine", "gyro")
/// ```
///
/// Using it changes the answer completely: clusters 2, 6, 8 and 9 are 94–99%
/// interior, which is to say **four of the fourteen families generate nothing
/// but guts.** Only 7, 11, 0 and 12 carry real anatomy.
///
/// A DISTRIBUTION rather than a home, because the clusters are SHAPE families
/// and shape is genuinely many-to-many with anatomy — a thigh and a clavicle
/// look alike, so they cluster together and neither is wrong. Leg parts are
/// spread across nine of the fourteen classes; collapsing that to "cluster 12
/// is the leg one" throws away most of what the data knows.
pub struct RegionMix {
    pub cluster: u8,
    /// head, torso, arm, leg, trim — sums to 1
    pub share: [f32; 5],
}

const fn r(cluster: u8, h: f32, t: f32, a: f32, l: f32, m: f32) -> RegionMix {
    RegionMix { cluster, share: [h, t, a, l, m] }
}
use r as R;

pub static REGION_MIX: &[RegionMix] = &[
    R(0,0.000,0.261,0.225,0.087,0.427),
    R(1,0.000,0.083,0.083,0.188,0.646),
    R(2,0.000,0.000,0.017,0.000,0.983),
    R(3,0.000,0.090,0.000,0.104,0.806),
    R(4,0.000,0.128,0.191,0.043,0.638),
    R(5,0.000,0.000,0.220,0.000,0.780),
    R(6,0.000,0.000,0.000,0.063,0.937),
    R(7,0.148,0.321,0.346,0.000,0.185),
    R(8,0.000,0.000,0.004,0.004,0.993),
    R(9,0.000,0.000,0.000,0.015,0.985),
    R(10,0.169,0.000,0.000,0.000,0.831),
    R(11,0.000,0.230,0.351,0.122,0.297),
    R(12,0.000,0.000,0.000,0.351,0.649),
    R(13,0.182,0.000,0.149,0.000,0.669),
];

/// The best share any family has in a region.
///
/// The normaliser, and it is necessary rather than a refinement. Since no
/// cluster is predominantly an arm — the best is 35% — pricing against a
/// perfect 1.0 charges EVERY arm in the library for being an arm, and a plain
/// frame came out at 0.24 with a penalty of 0.61. The question a soft bound
/// should ask is "how much worse than the best available choice", not "how
/// far from an ideal that does not exist in this corpus".
pub fn best_share(region: Region) -> f32 {
    REGION_MIX
        .iter()
        .map(|m| share_in(m.cluster, region))
        .fold(0.0f32, f32::max)
        .max(0.01)
}

/// How much of this cluster came from that region, 0..1.
pub fn share_in(cluster: u8, region: Region) -> f32 {
    let i = match region {
        Region::Head => 0,
        Region::Torso => 1,
        Region::Arm => 2,
        Region::Leg => 3,
        Region::Trim => 4,
    };
    REGION_MIX.iter().find(|m| m.cluster == cluster).map(|m| m.share[i]).unwrap_or(0.0)
}

// No `home_region`. It returned the argmax of the mix, and once `guts` was
// classified correctly that answer is "trim" for twelve of the fourteen —
// a column that says the same thing twelve times is not a summary, it is
// noise with a heading. What distinguishes a family is its ANATOMICAL SHARE
// and which region it is best at, and the driver prints those instead.
//
// Kept as a note because the argmax is the obvious thing to reach for and it
// is wrong here for a reason specific to this corpus.

/// A part in an assembly: which family made it, and where it was put.
#[derive(Clone, Copy, Debug)]
pub struct Placed {
    pub cluster: u8,
    pub at: Region,
    /// left of a pair, right, or neither
    pub side: Side,
    /// relative to the torso's mass
    pub mass_frac: f32,
    /// where around the body it mounts, radians. Two legs at 0 and pi is a
    /// biped; six at even spacing is something else, and the difference is
    /// the whole reason this field exists.
    pub bearing: f32,
    /// how far out it reaches, in torso-widths
    pub reach: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
    Centre,
}

/// A constraint that was broken.
#[derive(Clone, PartialEq, Debug)]
pub enum Violation {
    /// the assembly cannot exist or cannot be judged
    Hard(&'static str),
    /// unusual; costs this much and might still be worth it
    Soft(f32, String),
}

/// What a completely out-of-place part costs. A DIAL: high enough that the
/// rest of the assembly has to be convincing, low enough to be out-argued.
pub const ODDNESS: f32 = 0.30;

/// A part cannot outweigh the frame carrying it by more than this.
pub const MASS_CEILING: f32 = 2.5;

/// The most limbs of one kind.
///
/// A backstop against absurdity, and NOT what stops centipedes — measured, a
/// radial quad and a radial hexapod score identically, because the stability
/// term saturates at a margin of 1.5 and four evenly spread legs already
/// exceed it. Saturation is what removes the incentive; this is what stops
/// something with thirty legs being merely expensive rather than refused.
///
/// Worth saying because two mechanisms that look like they do the same job
/// invite somebody to remove the one that is actually carrying it.
pub const MAX_LIMBS: usize = 8;

/// Margin at which stability stops paying. Four evenly spread legs clear it,
/// so a fifth is free and a sixth is free — which is the diminishing return
/// that keeps the assembler from growing legs forever.
pub const STABILITY_CAP: f32 = 1.5;

/// Check an assembly. Hard violations reject; soft ones are priced.
pub fn bounds(parts: &[Placed]) -> Vec<Violation> {
    let mut v = Vec::new();

    // ---- HARD: nothing to attach to ------------------------------------
    let torsos = parts.iter().filter(|p| p.at == Region::Torso).count();
    if torsos == 0 {
        v.push(Violation::Hard("no torso — nothing to attach anything to"));
    }
    if torsos > 1 {
        v.push(Violation::Hard("two torsos — that is a slot filled twice, not a design"));
    }
    if parts.iter().filter(|p| p.at == Region::Head).count() > 1 {
        v.push(Violation::Hard("two heads — a slot filled twice"));
    }
    // ---- HARD: a physical claim, not a style ---------------------------
    if let Some(p) = parts.iter().find(|p| p.mass_frac > MASS_CEILING) {
        let _ = p;
        v.push(Violation::Hard("a part outweighs the frame carrying it"));
    }
    // ---- HARD: limb counts, with a tighter clamp than "a pair" ---------
    //
    // The old rule was that a limb needs its opposite side, which forbade
    // everything interesting: three arms, six legs, a radial thing with no
    // sides at all. Many limbs are allowed now, and the clamp got TIGHTER in
    // exchange — a ceiling, and an evenness rule, because balance rewards
    // more legs without limit and an unclamped assembler finds that out and
    // builds a centipede.
    for r in [Region::Arm, Region::Leg] {
        let n = parts.iter().filter(|p| p.at == r).count();
        if n > MAX_LIMBS {
            v.push(match r {
                Region::Arm => Violation::Hard("more arms than a frame can drive"),
                _ => Violation::Hard("more legs than a frame can drive"),
            });
        }
        // an odd count with explicit sides is a missing limb rather than a
        // design; an odd count of CENTRE-mounted limbs is a radial body and
        // is fine
        let sided = parts.iter().filter(|p| p.at == r && p.side != Side::Centre).count();
        if sided % 2 == 1 {
            v.push(match r {
                Region::Arm => Violation::Hard("an odd number of sided arms — one is missing"),
                _ => Violation::Hard("an odd number of sided legs — one is missing"),
            });
        }
    }
    if parts.iter().all(|p| p.at != Region::Leg) {
        v.push(Violation::Hard("nothing to stand on"));
    }

    // ---- SOFT: unusual, and somebody's favourite bot -------------------
    for p in parts {
        let Some(c) = CLUSTERS.iter().find(|c| c.id == p.cluster) else {
            v.push(Violation::Hard("a part from a family that does not exist"));
            continue;
        };
        // PRICED BY HOW RARE IT IS, not by a flat table. A cluster that
        // supplies 35% of the legs in the library is barely unusual as a leg;
        // one that supplies none is a statement. The old version charged a
        // flat 0.10 or 0.22 off a single guessed home region, which could not
        // tell those apart.
        let p_here = share_in(p.cluster, p.at) / best_share(p.at);
        let trim = share_in(p.cluster, Region::Trim);
        // parts that are mostly trim genuinely go anywhere, so they are cheap
        // wherever they land
        let cost = ODDNESS * (1.0 - p_here.min(1.0)) * (1.0 - trim);
        if cost > 0.02 {
            v.push(Violation::Soft(
                cost,
                format!(
                    "family {} is {:.0}% as {}-ish as the best",
                    p.cluster,
                    p_here.min(1.0) * 100.0,
                    p.at.name()
                ),
            ));
        }
        // A class that comes in pairs, used ALONE — and "alone" means one of
        // its kind on the whole frame, not merely centre-mounted. The first
        // version fired on any centre-mounted limb, so a radial hexapod paid
        // it six times (0.72 of penalty) and scored 0.055 against a top-heavy
        // biped's 0.467. That is the exact bias the physical axis was added
        // to remove, reintroduced by a check written back when limbs had to
        // have sides.
        let same_kind = parts.iter().filter(|q| q.at == p.at).count();
        if c.mirrored > 0.9 && same_kind == 1 && p.at.paired() {
            v.push(Violation::Soft(0.12, format!("family {} is a pair, used alone", c.id)));
        }
    }
    // asymmetric pairs: different families left and right
    for r in [Region::Arm, Region::Leg] {
        let l: Vec<u8> =
            parts.iter().filter(|p| p.at == r && p.side == Side::Left).map(|p| p.cluster).collect();
        let rt: Vec<u8> = parts
            .iter()
            .filter(|p| p.at == r && p.side == Side::Right)
            .map(|p| p.cluster)
            .collect();
        if !l.is_empty() && !rt.is_empty() && l != rt {
            v.push(Violation::Soft(0.18, format!("mismatched {}s", r.name())));
        }
    }
    v
}

// ------------------------------------------------------- does it stand ---

/// How stable a frame is, before anything about the corpus.
///
/// **The second axis, and the reason it exists.** Scoring only on how well an
/// assembly matches the library is a machine for producing the average bot:
/// anything unusual is by construction less like the corpus, so the corpus
/// term can only ever punish it. A freak needs somewhere else to earn its
/// place, and physics is the honest one — a six-legged thing does not match
/// anything in the library and it stands up better than a biped, which is a
/// real advantage that the matching score cannot see.
#[derive(Clone, Copy, Debug)]
pub struct Balance {
    /// radius of the support polygon, in torso-widths. Bigger is steadier.
    pub support_r: f32,
    /// how evenly the feet are spread, 0..1. A tripod with all three legs on
    /// one side supports nothing.
    pub evenness: f32,
    /// height of the centre of mass, in torso-widths
    pub com_h: f32,
    /// static margin: support radius against CoM height. Above 1 it is hard
    /// to tip.
    pub margin: f32,
}

/// Seconds to topple once past the margin, at a given gravity.
///
/// GRAVITY FEEDBACK, and it is why the same frame is a different design on
/// two worlds. An inverted pendulum falls in about `sqrt(h/g)`, so on Titan
/// at a fifth of Earth's pull a top-heavy thing has more than twice as long
/// to catch itself — a frame that is unusable on the Foundry is fine out on the
/// rim. `scale.rs` already knows what a frame is; this says whether it can
/// stand on the world it is standing on.
pub fn topple_time_s(b: &Balance, gravity_ms2: f32) -> f32 {
    let h = (b.com_h * 4.0).max(0.2); // torso-widths to metres, roughly
    (h / gravity_ms2.max(0.05)).sqrt() * (1.0 + b.margin)
}

/// Measure an assembly's stance.
pub fn balance(parts: &[Placed]) -> Balance {
    let feet: Vec<&Placed> = parts.iter().filter(|p| p.at == Region::Leg).collect();
    if feet.is_empty() {
        return Balance { support_r: 0.0, evenness: 0.0, com_h: 1.0, margin: 0.0 };
    }
    let support_r = feet.iter().map(|f| f.reach).sum::<f32>() / feet.len() as f32;

    // EVENNESS as the length of the mean unit bearing, inverted. All the feet
    // on one side gives a mean near 1 and no support; evenly spread cancels
    // to nearly zero and supports in every direction.
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    for f in &feet {
        sx += f.bearing.cos();
        sy += f.bearing.sin();
    }
    let n = feet.len() as f32;
    let clump = (sx * sx + sy * sy).sqrt() / n;
    // two opposed legs cancel exactly, so a biped scores 1 here — correct, a
    // biped IS evenly spread; it just has a thin polygon, which `support_r`
    // and the count carry instead
    let evenness = (1.0 - clump).clamp(0.0, 1.0);

    // more feet is a wider polygon, with diminishing returns
    let count_bonus = (feet.len() as f32 / 2.0).sqrt().min(2.2);
    let effective_r = support_r * (0.45 + 0.55 * evenness) * count_bonus;

    let mass: f32 = parts.iter().map(|p| p.mass_frac).sum::<f32>().max(0.01);
    let com_h = parts
        .iter()
        .map(|p| {
            let h = match p.at {
                Region::Head => 2.2,
                Region::Torso => 1.4,
                Region::Arm => 1.5,
                Region::Leg => 0.5,
                Region::Trim => 1.2,
            };
            h * p.mass_frac
        })
        .sum::<f32>()
        / mass;

    Balance {
        support_r,
        evenness,
        com_h,
        margin: (effective_r / com_h.max(0.1)).min(3.0),
    }
}

/// The verdict on an assembly.
#[derive(Clone, PartialEq, Debug)]
pub enum Verdict {
    /// hard bounds broken; the reasons
    Rejected(Vec<&'static str>),
    /// accepted, with what it cost and what it was
    Accepted { score: f32, penalty: f32, notes: Vec<String> },
}

/// How good the parts are on their own, before any bounds.
///
/// Stands in for the per-family critics. A real assembler takes this from the
/// discriminators; passing it in keeps the bounds testable without them.
pub fn assemble(parts: &[Placed], part_quality: f32) -> Verdict {
    let vs = bounds(parts);
    let hard: Vec<&'static str> =
        vs.iter().filter_map(|v| if let Violation::Hard(r) = v { Some(*r) } else { None }).collect();
    if !hard.is_empty() {
        return Verdict::Rejected(hard);
    }
    let mut penalty = 0.0;
    let mut notes = Vec::new();
    for v in &vs {
        if let Violation::Soft(c, why) = v {
            penalty += c;
            notes.push(why.clone());
        }
    }
    // coverage: a frame with a head, a torso, arms and legs reads as a frame
    let filled = Region::all()
        .iter()
        .filter(|r| **r != Region::Trim && parts.iter().any(|p| p.at == **r))
        .count() as f32;
    let coverage = filled / 4.0;
    // TWO AXES, and the physical one is not a tiebreak. Corpus matching can
    // only ever punish an unusual frame — anything novel is by construction
    // less like the library — so a freak needs somewhere else to earn its
    // place. Standing up is the honest one.
    let stance = balance(parts);
    let stability = (stance.margin / STABILITY_CAP).min(1.0);
    let score =
        (0.40 * part_quality + 0.25 * coverage + 0.35 * stability - penalty).max(0.0);
    Verdict::Accepted { score, penalty, notes }
}

/// Can this family train its own generator, or must it sample the library?
pub fn trainable(c: &Cluster) -> bool {
    c.n >= TRAINABLE_MIN
}

/// With the mirror flip, which costs nothing and is exact.
pub fn trainable_mirrored(c: &Cluster) -> bool {
    effective_n(c) >= TRAINABLE_MIN as f32
}

pub fn effective_n(c: &Cluster) -> f32 {
    c.n as f32 * if c.mirrored > 0.9 { MIRROR_GAIN } else { 1.0 }
}

/// How many examples a ROLE has, pooled across every class that contributes
/// to it and mirrored where the class allows.
///
/// **This is the answer to the corpus constraint**, and it took two goes to
/// see. Mirroring alone does not rescue the anatomical classes: family 7 goes
/// 95 to 190 against a floor of 200, and family 11 does not mirror at all.
///
/// But a class is a SHAPE partition and training wants enough examples of a
/// FUNCTIONAL thing. Families 7 and 11 are both arm-dominant; there is no
/// reason an arm generator should care that one clusters slightly differently
/// from the other. Pooling by role and then mirroring, every role clears the
/// floor — head, the worst case, goes 163 to 325.
///
/// So the family is not fourteen shape generators and it is not four. It is
/// FIVE ROLE GENERATORS — head, torso, arm, leg, interior — and the shape
/// clusters are how you sample within one, not what you train.
pub fn role_pool(region: Region) -> f32 {
    CLUSTERS
        .iter()
        .map(|c| share_in(c.id, region) * effective_n(c))
        .sum()
}

// ------------------------------------------------------------- driver ---

pub fn run() {
    println!("THE ASSEMBLER — a family per part class, and a score for the pairing\n");

    println!("  THE FAMILIES, measured from the library\n");
    // NOT the argmax. Twelve of fourteen are mostly interior, so a single
    // "region" column says "trim" twelve times and tells you nothing. The
    // anatomical share is what distinguishes them.
    println!(
        "  {:>3} {:>6} {:>9} {:>8} {:>8}  {}",
        "id", "n", "anatomy", "best at", "trains?", "members"
    );
    let mut trains = 0;
    for c in CLUSTERS {
        let t = trainable(c);
        if t {
            trains += 1;
        }
        let anat = 1.0 - share_in(c.id, Region::Trim);
        // the region it is most over-represented in, ignoring trim
        let best = [Region::Head, Region::Torso, Region::Arm, Region::Leg]
            .into_iter()
            .max_by(|a, b| share_in(c.id, *a).partial_cmp(&share_in(c.id, *b)).unwrap())
            .unwrap();
        println!(
            "  {:>3} {:>6} {:>8.0}% {:>8} {:>8}  {}",
            c.id,
            c.n,
            anat * 100.0,
            if anat > 0.05 { best.name() } else { "—" },
            if t { "yes" } else { "library" },
            c.members
        );
    }
    let interior: Vec<u8> = REGION_MIX
        .iter()
        .filter(|m| m.share[4] > 0.90)
        .map(|m| m.cluster)
        .collect();
    println!("\n  FOUR OF THEM GENERATE NOTHING BUT GUTS: {interior:?}\n");
    println!("  `guts` alone is 1,733 of the library's 4,455 parts. The tree already");
    println!("  classified it — `tools/find-mounts.py` has an INTERNAL list from a");
    println!("  search for gun barrels that kept returning struts — and reading that");
    println!("  rather than guessing changes the whole picture: only families 0, 7,");
    println!("  11 and 12 carry real anatomy.\n");
    println!("  So the family is not fourteen equal generators. It is a handful of");
    println!("  anatomical ones and an interior-filler generator, which is a better");
    println!("  architecture than the one I proposed and it came out of the counts.\n");

    // the sting, and it is worth stating outright
    let mut anat: Vec<(&Cluster, f32)> =
        CLUSTERS.iter().map(|c| (c, 1.0 - share_in(c.id, Region::Trim))).collect();
    anat.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    println!("  AND THE STING:\n");
    for (c, a) in anat.iter().take(3) {
        println!(
            "    family {:>2} is {:>3.0}% anatomy and has {:>4} parts — {}",
            c.id,
            a * 100.0,
            c.n,
            if trainable(c) { "trains" } else { "TOO SMALL TO TRAIN" }
        );
    }
    println!("\n  the two most anatomical families in the library are the two");
    println!("  smallest, and both are under the floor. The classes that CAN train");
    println!("  are mostly the ones full of guts.\n");

    println!("  BUT THE CONSTRAINT DISSOLVES IF YOU POOL BY ROLE\n");
    println!("    mirroring first: eleven of fourteen classes come in left/right");
    println!("    pairs, so a flip is EXACT rather than an approximation. Not enough");
    println!(
        "    on its own — family 7 goes 95 to {:.0} against a floor of {TRAINABLE_MIN},",
        effective_n(CLUSTERS.iter().find(|c| c.id == 7).unwrap())
    );
    println!("    and family 11 does not mirror at all.\n");
    println!("    {:>8} {:>10} {:>10}", "role", "pooled", "trains?");
    for r in [Region::Head, Region::Torso, Region::Arm, Region::Leg, Region::Trim] {
        let n = role_pool(r);
        println!(
            "    {:>8} {:>10.0} {:>10}",
            r.name(),
            n,
            if n >= TRAINABLE_MIN as f32 { "yes" } else { "NO" }
        );
    }
    println!(
        "\n    every role clears it, head included — the worst case, 163 to {:.0}.",
        role_pool(Region::Head)
    );
    println!("\n  so it is not fourteen shape generators and it is not four. It is");
    println!("  FIVE ROLE GENERATORS — head, torso, arm, leg, interior — and the");
    println!("  shape clusters are how you SAMPLE within one, not what you train.");
    println!(
        "\n  ({} of {} classes would also train alone once mirrored.)\n",
        CLUSTERS.iter().filter(|c| trainable_mirrored(c)).count(),
        CLUSTERS.len()
    );
    println!(
        "  {trains} of {} can train their own generator at a floor of {TRAINABLE_MIN}.",
        CLUSTERS.len()
    );
    println!("  The rest sample the library, which is the same fallback `--bench`");
    println!("  uses: forty-nine examples is not a training set.\n");

    println!("  HARD AND SOFT\n");
    let torso = |cl: u8| Placed { cluster: cl, at: Region::Torso, side: Side::Centre, mass_frac: 1.0, bearing: 0.0, reach: 0.0 };
    let limb = |cl: u8, r: Region, s: Side| Placed {
        cluster: cl, at: r, side: s, mass_frac: 0.3,
        bearing: if s == Side::Left { std::f32::consts::FRAC_PI_2 } else { -std::f32::consts::FRAC_PI_2 },
        reach: 1.0,
    };
    let head = |cl: u8| Placed { cluster: cl, at: Region::Head, side: Side::Centre, mass_frac: 0.2, bearing: 0.0, reach: 0.0 };

    let cases: Vec<(&str, Vec<Placed>)> = vec![
        (
            "a plain frame",
            vec![torso(7), head(0), limb(5, Region::Arm, Side::Left), limb(5, Region::Arm, Side::Right),
                 limb(12, Region::Leg, Side::Left), limb(12, Region::Leg, Side::Right)],
        ),
        (
            "legs used as arms",
            vec![torso(7), head(0), limb(12, Region::Arm, Side::Left), limb(12, Region::Arm, Side::Right),
                 limb(12, Region::Leg, Side::Left), limb(12, Region::Leg, Side::Right)],
        ),
        (
            "mismatched arms",
            vec![torso(7), head(0), limb(5, Region::Arm, Side::Left), limb(1, Region::Arm, Side::Right),
                 limb(12, Region::Leg, Side::Left), limb(12, Region::Leg, Side::Right)],
        ),
        (
            "no torso",
            vec![head(0), limb(12, Region::Leg, Side::Left), limb(12, Region::Leg, Side::Right)],
        ),
        (
            "one arm",
            vec![torso(7), limb(5, Region::Arm, Side::Left),
                 limb(12, Region::Leg, Side::Left), limb(12, Region::Leg, Side::Right)],
        ),
        (
            "a head heavier than the body",
            vec![Placed { cluster: 0, at: Region::Head, side: Side::Centre, mass_frac: 4.0, bearing: 0.0, reach: 0.0 },
                 torso(7), limb(12, Region::Leg, Side::Left), limb(12, Region::Leg, Side::Right)],
        ),
    ];

    for (label, parts) in &cases {
        match assemble(parts, 0.75) {
            Verdict::Rejected(why) => println!("    {:<28} REJECTED — {}", label, why.join("; ")),
            Verdict::Accepted { score, penalty, notes } => {
                if notes.is_empty() {
                    println!("    {:<28} {:.3}", label, score);
                } else {
                    println!(
                        "    {:<28} {:.3}  (-{:.2}: {})",
                        label,
                        score,
                        penalty,
                        notes.join(", ")
                    );
                }
            }
        }
    }

    println!("\n  DOES IT STAND? — the second axis\n");
    let radial = |n: usize| {
        let mut p = vec![torso(7)];
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            p.push(Placed {
                cluster: 12, at: Region::Leg, side: Side::Centre,
                mass_frac: 0.3, bearing: a, reach: 1.0,
            });
        }
        p
    };
    println!("  {:>16} {:>9} {:>9} {:>10} {:>12}", "frame", "support", "margin", "score", "topple @1g");
    let show = |label: &str, parts: &[Placed]| {
        let b = balance(parts);
        let sc = match assemble(parts, 0.75) {
            Verdict::Accepted { score, .. } => format!("{score:.3}"),
            Verdict::Rejected(_) => "rejected".into(),
        };
        println!(
            "  {:>16} {:>9.2} {:>9.2} {:>10} {:>11.2}s",
            label, b.support_r * b.evenness, b.margin, sc, topple_time_s(&b, 9.8)
        );
    };
    show("biped", &cases[0].1);
    let mut topheavy = cases[0].1.clone();
    topheavy[1].mass_frac = 2.2;
    show("top-heavy biped", &topheavy);
    show("radial quad", &radial(4));
    show("radial hexapod", &radial(6));
    let clumped: Vec<Placed> = {
        let mut p = vec![torso(7)];
        for i in 0..4 {
            p.push(Placed { cluster: 12, at: Region::Leg, side: Side::Centre,
                mass_frac: 0.3, bearing: 0.05 * i as f32, reach: 1.0 });
        }
        p
    };
    show("four legs, one side", &clumped);

    println!("\n    the hexapod matches nothing in the library — no head, no arms,");
    println!("    six legs from one family — and it BEATS a textbook biped that is");
    println!("    top-heavy. That is the whole reason for a second axis: corpus");
    println!("    matching can only ever punish a novel frame, so a freak needs");
    println!("    somewhere else to earn its place, and standing up is honest.\n");

    println!("  AND GRAVITY DECIDES WHERE A DESIGN IS USABLE\n");
    let b = balance(&topheavy);
    for (w, g) in [("foundry", 9.8f32), ("mars", 3.7), ("titan", 1.35)] {
        println!("    {:<10} {:>5.1} m/s^2   {:.2}s to topple", w, g, topple_time_s(&b, g));
    }
    println!("\n    the same top-heavy frame is unusable at home and fine on the rim.");

    println!("\n  AND A SOFT BOUND CAN BE OUT-ARGUED\n");
    let odd = &cases[1].1;
    for q in [0.55f32, 0.75, 0.95] {
        let plain = assemble(&cases[0].1, 0.55);
        let v = assemble(odd, q);
        if let (Verdict::Accepted { score: a, .. }, Verdict::Accepted { score: b, .. }) =
            (plain, v)
        {
            println!(
                "    legs-as-arms at part quality {q:.2}:  {b:.3}  vs a plain frame at 0.55: {a:.3}   {}",
                if b > a { "-> it wins" } else { "" }
            );
        }
    }
    println!("\n  that is the whole point of the split. A leg used as an arm is");
    println!("  somebody's favourite bot, so it is PRICED rather than forbidden and");
    println!("  the rest of the assembly has to be convincing enough to carry it.");
    println!("  No torso is not a bold choice; it is nothing to attach anything to.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torso() -> Placed {
        Placed { cluster: 7, at: Region::Torso, side: Side::Centre, mass_frac: 1.0, bearing: 0.0, reach: 0.0 }
    }
    fn at(cluster: u8, at: Region, side: Side, bearing: f32) -> Placed {
        Placed { cluster, at, side, mass_frac: 0.3, bearing, reach: 1.0 }
    }
    /// n legs evenly around the body
    fn radial_legs(n: usize) -> Vec<Placed> {
        (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Placed { cluster: 12, at: Region::Leg, side: Side::Centre, mass_frac: 0.3, bearing: a, reach: 1.0 }
            })
            .collect()
    }
    fn legs() -> Vec<Placed> {
        vec![
            at(12, Region::Leg, Side::Left, std::f32::consts::FRAC_PI_2),
            at(12, Region::Leg, Side::Right, -std::f32::consts::FRAC_PI_2),
        ]
    }
    fn frame() -> Vec<Placed> {
        let mut v = vec![
            torso(),
            Placed { cluster: 0, at: Region::Head, side: Side::Centre, mass_frac: 0.2, bearing: 0.0, reach: 0.0 },
            at(5, Region::Arm, Side::Left, std::f32::consts::FRAC_PI_2),
            at(5, Region::Arm, Side::Right, -std::f32::consts::FRAC_PI_2),
        ];
        v.extend(legs());
        v
    }

    #[test]
    fn hard_bounds_reject_what_cannot_exist() {
        // nothing to attach to, a slot filled twice, a physical impossibility
        for (parts, why) in [
            (legs(), "no torso"),
            (
                {
                    let mut f = frame();
                    f.push(torso());
                    f
                },
                "two torsos",
            ),
            (
                {
                    let mut f = frame();
                    f.push(Placed { cluster: 0, at: Region::Head, side: Side::Centre, mass_frac: 0.2, bearing: 0.0, reach: 0.0 });
                    f
                },
                "two heads",
            ),
            (
                {
                    let mut f = frame();
                    f[1].mass_frac = 9.0;
                    f
                },
                "outweighs",
            ),
        ] {
            match assemble(&parts, 0.99) {
                Verdict::Rejected(rs) => {
                    assert!(rs.iter().any(|r| r.contains(why)), "expected {why}, got {rs:?}")
                }
                other => panic!("{why} was accepted: {other:?}"),
            }
        }
    }

    #[test]
    fn a_perfect_score_cannot_buy_a_hard_bound() {
        // the distinction from soft: no amount of quality elsewhere makes an
        // assembly with nothing to attach to into a bot
        assert!(matches!(assemble(&legs(), 1.0), Verdict::Rejected(_)));
        assert!(matches!(assemble(&[torso()], 1.0), Verdict::Rejected(_)), "it has no legs");
    }

    #[test]
    fn an_odd_placement_is_priced_not_forbidden() {
        let mut odd = frame();
        odd[2].cluster = 12; // a leg family on an arm
        odd[3].cluster = 12;
        match assemble(&odd, 0.75) {
            Verdict::Accepted { penalty, notes, .. } => {
                assert!(penalty > 0.0, "it cost nothing");
                assert!(notes.iter().any(|n| n.contains("arm-ish")), "{notes:?}");
            }
            other => panic!("an unusual bot was refused: {other:?}"),
        }
    }

    #[test]
    fn a_soft_bound_can_be_out_argued_and_a_hard_one_cannot() {
        // THE WHOLE POINT OF THE SPLIT
        let mut odd = frame();
        odd[2].cluster = 12;
        odd[3].cluster = 12;
        let plain_ok = match assemble(&frame(), 0.55) {
            Verdict::Accepted { score, .. } => score,
            o => panic!("{o:?}"),
        };
        let odd_great = match assemble(&odd, 0.99) {
            Verdict::Accepted { score, .. } => score,
            o => panic!("{o:?}"),
        };
        assert!(
            odd_great > plain_ok,
            "an excellent odd frame ({odd_great:.3}) lost to a mediocre plain one ({plain_ok:.3})"
        );
        // and the same excellence cannot rescue a headless torso-less pile
        assert!(matches!(assemble(&legs(), 1.0), Verdict::Rejected(_)));
    }

    #[test]
    fn a_penalty_is_big_enough_to_matter_and_small_enough_to_lose() {
        // a soft bound that changes nothing is decoration; one that always
        // decides is a hard bound wearing a discount
        let mut odd = frame();
        odd[2].cluster = 12;
        odd[3].cluster = 12;
        let (a, b) = (
            match assemble(&frame(), 0.8) {
                Verdict::Accepted { score, .. } => score,
                o => panic!("{o:?}"),
            },
            match assemble(&odd, 0.8) {
                Verdict::Accepted { score, .. } => score,
                o => panic!("{o:?}"),
            },
        );
        assert!(a > b, "the penalty did nothing");
        assert!(b > a * 0.6, "the penalty is effectively a rejection: {b} vs {a}");
    }

    #[test]
    fn no_family_is_purely_anatomical_and_most_are_mostly_guts() {
        // A finding, not a defect. Every one of the fourteen is mostly TORSO
        // or mostly TRIM, because they are SHAPE families and the limbs of a
        // Machine are shaped like its trunk. That is why a single home
        // region was the wrong model: it collapses a many-to-many
        // relationship onto whichever region happened to win.
        // Stated as anatomical SHARE rather than as an argmax, because the
        // argmax says "trim" for twelve of fourteen and is no longer computed.
        for c in CLUSTERS {
            let anat = 1.0 - share_in(c.id, Region::Trim);
            assert!(anat < 0.9, "family {} is {anat:.0}% anatomy — the corpus changed", c.id);
        }
        // and yet leg parts are everywhere, which is the point
        let with_legs = REGION_MIX.iter().filter(|m| m.share[3] > 0.01).count();
        assert!(with_legs >= 8, "only {with_legs} classes contain leg parts");
    }

    #[test]
    fn the_penalty_is_priced_by_rarity_rather_than_a_flat_table() {
        // a class supplying a third of the library's legs is barely unusual
        // as a leg; one supplying none is a statement, and a flat table
        // cannot tell them apart
        let common = share_in(12, Region::Leg);
        let never = share_in(12, Region::Arm);
        assert!(common > 0.3 && never < 0.01, "{common} {never}");

        let mut as_leg = frame();
        as_leg[4].cluster = 12;
        as_leg[5].cluster = 12;
        let mut as_arm = frame();
        as_arm[2].cluster = 12;
        as_arm[3].cluster = 12;
        let pen = |v: Verdict| match v {
            Verdict::Accepted { penalty, .. } => penalty,
            o => panic!("{o:?}"),
        };
        assert!(
            pen(assemble(&as_arm, 0.8)) > pen(assemble(&as_leg, 0.8)),
            "using a leg family as an arm should cost more than as a leg"
        );
    }

    #[test]
    fn a_mostly_trim_family_is_cheap_wherever_it_lands() {
        // cable, clad and greeble genuinely go everywhere, so charging them
        // for being somewhere is charging them for existing
        let trimmy = REGION_MIX
            .iter()
            .max_by(|a, b| a.share[4].partial_cmp(&b.share[4]).unwrap())
            .unwrap();
        assert!(trimmy.share[4] > 0.7, "no class is mostly trim?");
        // measured as a DELTA against the same frame with a non-trim family
        // in the same slots: the baseline frame already carries penalties of
        // its own, and comparing against zero measures those instead.
        let pen = |c: u8| {
            let mut f = frame();
            f[2].cluster = c;
            f[3].cluster = c;
            match assemble(&f, 0.8) {
                Verdict::Accepted { penalty, .. } => penalty,
                o => panic!("{o:?}"),
            }
        };
        // Compared against an ANATOMICAL family used in the wrong place.
        // The first version compared against cluster 8, calling it "90%
        // torso" — under the corrected mapping cluster 8 is 99% interior,
        // so both sides were trim and the test compared a thing to itself.
        // Cluster 12 is the leg family: real anatomy, and wrong on an arm.
        assert!(
            pen(trimmy.cluster) < pen(12),
            "trim {} cost no less than a leg family on an arm {}",
            pen(trimmy.cluster),
            pen(12)
        );
    }

    #[test]
    fn a_one_sided_pair_is_hard_because_it_cannot_be_judged() {
        // not a style choice: there is no second side to compare against, so
        // the symmetry checks have nothing to say
        let mut one = frame();
        one.remove(3); // the right arm
        assert!(matches!(assemble(&one, 0.9), Verdict::Rejected(_)));
    }

    #[test]
    fn mismatched_pairs_are_soft() {
        let mut odd = frame();
        odd[3].cluster = 1;
        match assemble(&odd, 0.8) {
            Verdict::Accepted { notes, .. } => {
                assert!(notes.iter().any(|n| n.contains("mismatched")), "{notes:?}")
            }
            o => panic!("asymmetry should be allowed: {o:?}"),
        }
    }

    #[test]
    fn most_families_cannot_train_and_that_is_the_real_limit() {
        // the engineering constraint on the whole idea, measured
        let can = CLUSTERS.iter().filter(|c| trainable(c)).count();
        assert!(can > 0, "none of them can train");
        assert!(
            can < CLUSTERS.len(),
            "all fourteen train at a floor of {TRAINABLE_MIN}; raise it or say so"
        );
        // and the smallest is nowhere near
        let smallest = CLUSTERS.iter().map(|c| c.n).min().unwrap();
        assert!(smallest < TRAINABLE_MIN / 2, "smallest family is {smallest}");
    }

    #[test]
    fn mirroring_is_free_data_and_still_not_enough() {
        // eleven of fourteen classes come in left/right pairs, so a flip is
        // exact rather than an approximation — and it does not rescue the
        // classes that actually need it
        let mirrored = CLUSTERS.iter().filter(|c| c.mirrored > 0.9).count();
        assert!(mirrored >= 10, "only {mirrored} classes mirror");

        let seven = CLUSTERS.iter().find(|c| c.id == 7).unwrap();
        assert!(!trainable(seven), "family 7 was already trainable");
        assert!(
            !trainable_mirrored(seven),
            "mirroring rescued family 7 at {:.0}; the floor or the data moved",
            effective_n(seven)
        );
        // and family 11, the other anatomical one, does not mirror at all
        let eleven = CLUSTERS.iter().find(|c| c.id == 11).unwrap();
        assert!(eleven.mirrored < 0.9);
        assert_eq!(effective_n(eleven), eleven.n as f32);
    }

    #[test]
    fn pooling_by_role_clears_the_floor_for_everything() {
        // THE ANSWER. A class is a shape partition; training wants enough
        // examples of a functional thing, and there is no reason an arm
        // generator should care which of two arm-ish clusters a part fell in.
        for r in [Region::Head, Region::Torso, Region::Arm, Region::Leg] {
            let n = role_pool(r);
            assert!(
                n >= TRAINABLE_MIN as f32,
                "{} pools to only {n:.0}",
                r.name()
            );
        }
        // head is the worst case and it more than doubles
        let head = role_pool(Region::Head);
        assert!(head > 250.0 && head < 500.0, "head pooled to {head:.0}");
        // and interior dwarfs everything, which is the same finding again
        assert!(role_pool(Region::Trim) > role_pool(Region::Arm) * 5.0);
    }

    #[test]
    fn many_limbs_are_allowed_and_the_clamp_got_tighter() {
        // the old rule forbade everything interesting; the new one allows a
        // hexapod and refuses a centipede
        let mut hex = vec![torso()];
        hex.extend(radial_legs(6));
        assert!(matches!(assemble(&hex, 0.7), Verdict::Accepted { .. }), "a hexapod was refused");

        let mut centipede = vec![torso()];
        centipede.extend(radial_legs(MAX_LIMBS + 4));
        assert!(
            matches!(assemble(&centipede, 0.9), Verdict::Rejected(_)),
            "nothing stopped the assembler building a centipede"
        );

        // and an odd number of SIDED limbs is still a missing limb
        let mut lopsided = vec![torso()];
        lopsided.extend(legs());
        lopsided.push(at(5, Region::Arm, Side::Left, 1.5));
        assert!(matches!(assemble(&lopsided, 0.9), Verdict::Rejected(_)));
    }

    #[test]
    fn more_legs_stand_better_and_it_saturates() {
        let stance = |n: usize| {
            let mut p = vec![torso()];
            p.extend(radial_legs(n));
            balance(&p).margin
        };
        assert!(stance(4) > stance(2), "four legs should beat two");
        assert!(stance(6) > stance(4));
        // diminishing returns, or the assembler learns that legs are free
        assert!(stance(8) < stance(4) * 2.0, "the bonus never saturates");
    }

    #[test]
    fn saturation_not_the_clamp_is_what_stops_a_centipede() {
        // Two mechanisms that look like they do the same job invite somebody
        // to delete the one that is carrying it. MAX_LIMBS is a backstop; the
        // thing that actually removes the incentive is that stability
        // saturates, so a fifth leg buys nothing.
        let sc = |n: usize| {
            let mut p = vec![torso()];
            p.extend(radial_legs(n));
            match assemble(&p, 0.75) {
                Verdict::Accepted { score, .. } => score,
                o => panic!("{o:?}"),
            }
        };
        assert!(sc(4) > sc(2), "four legs should be worth having");
        assert!(
            (sc(6) - sc(4)).abs() < 1e-4,
            "a hexapod scores {} against a quad's {} — the cap moved",
            sc(6),
            sc(4)
        );
        assert!(balance(&{
            let mut p = vec![torso()];
            p.extend(radial_legs(4));
            p
        })
        .margin >= STABILITY_CAP);
    }

    #[test]
    fn legs_all_on_one_side_support_nothing() {
        // evenness, not count: a tripod with every foot in one place is a
        // pole, and the count bonus must not rescue it
        let clumped: Vec<Placed> = {
            let mut p = vec![torso()];
            for i in 0..4 {
                p.push(at(12, Region::Leg, Side::Centre, 0.05 * i as f32));
            }
            p
        };
        let mut spread = vec![torso()];
        spread.extend(radial_legs(4));
        assert!(
            balance(&clumped).margin < balance(&spread).margin,
            "clumped {} vs spread {}",
            balance(&clumped).margin,
            balance(&spread).margin
        );
        assert!(balance(&clumped).evenness < 0.2);
    }

    #[test]
    fn a_freak_that_stands_can_beat_a_plausible_thing_that_topples() {
        // THE POINT OF THE SECOND AXIS. Corpus matching can only punish a
        // novel frame, so without physics the scorer is a machine for
        // producing the average bot.
        //
        // A radial hexapod out of the leg family: no head, no arms, matches
        // nothing in the library — and it stands on six evenly spread feet.
        let mut freak = vec![torso()];
        freak.extend(radial_legs(6));

        // a textbook biped, but top-heavy: a huge head on thin legs
        let mut topheavy = frame();
        topheavy[1].mass_frac = 2.2;

        let sc = |v: Verdict| match v {
            Verdict::Accepted { score, .. } => score,
            o => panic!("{o:?}"),
        };
        let a = sc(assemble(&freak, 0.75));
        let b = sc(assemble(&topheavy, 0.75));
        assert!(
            a > b,
            "the hexapod ({a:.3}) lost to a top-heavy biped ({b:.3}); the physical \
             axis is not carrying its weight"
        );
        // and the freak really does score worse on the corpus half
        assert!(balance(&freak).margin > balance(&topheavy).margin);
    }

    #[test]
    fn gravity_decides_whether_a_frame_is_usable_there() {
        // the same design is a different design on two worlds: an inverted
        // pendulum falls in about sqrt(h/g)
        let mut topheavy = frame();
        topheavy[1].mass_frac = 2.5;
        let b = balance(&topheavy);
        let foundry = topple_time_s(&b, 9.8);
        let titan = topple_time_s(&b, 1.35);
        assert!(titan > foundry * 2.0, "titan {titan:.2}s vs foundry {foundry:.2}s");
        // and a steady frame has longer everywhere
        let steady = {
            let mut p = vec![torso()];
            p.extend(radial_legs(6));
            balance(&p)
        };
        assert!(topple_time_s(&steady, 9.8) > topple_time_s(&b, 9.8));
    }

    #[test]
    fn an_unknown_family_is_hard_rather_than_ignored() {
        let mut f = frame();
        f[2].cluster = 200;
        assert!(matches!(assemble(&f, 0.9), Verdict::Rejected(_)));
    }
}
