//! Ask narrow questions, several times, instead of one wide one.
//!
//! "Build a statue of Zeus" gets ONE interpret turn: what is it, its
//! parts, its materials, what to avoid. A model answers that in five
//! lines and every one is a summary, so the statue comes out bland --
//! not because the model does not know what Zeus looks like, but
//! because nothing ever asked.
//!
//! The fix is not a longer prompt. It is MORE TURNS, each narrow:
//!
//!   1. SUBJECT     who or what is this, specifically? Not "a god" --
//!                  bearded, mature, bare-chested, a himation over one
//!                  shoulder, thunderbolt in the right hand, eagle at
//!                  the foot. The identifying details.
//!   2. ANATOMY     the parts, with PROPORTIONS. A head is one eighth
//!                  of a standing figure and the shoulders are two
//!                  heads across; a model knows this and was never
//!                  asked, so every figure came out at whatever
//!                  proportion the code guessed.
//!   3. POSE        what the body is DOING, joint by joint, and what
//!                  the silhouette reads as from a distance. The
//!                  silhouette is the thing a voxel build has and a
//!                  drawing does not need to think about.
//!   4. VOXELISE    at this block size, what survives and what does
//!                  not? Fingers do not. A beard does if it is a mass
//!                  and not strands. This is the question a voxel
//!                  artist asks that nobody else does, and asking it
//!                  explicitly is the difference between a shape that
//!                  reads and a shape that is technically correct and
//!                  unreadable.
//!
//! WHY FOUR AND NOT ONE
//!
//! Each is a different KIND of knowledge and a model retrieves each
//! better alone. Asked all four at once it gives one sentence on each,
//! and one sentence on anatomy is "it has arms and legs".
//!
//! The ladder is only worth it above a size where detail can land: a
//! six-block statue has no room for an eagle. `depth_for` decides.

use std::collections::BTreeMap;

/// How a body is arranged. The proportions say how big the parts are;
/// this says where they go, and it is the question that was never
/// asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BodyPlan {
    /// Upright, two legs under a torso, arms at the shoulders, head on
    /// top. A person, a god, a statue of either.
    #[default]
    Biped,
    /// Four legs splayed from a low horizontal body, head at the front.
    /// A frog, a horse, a lion, a dog.
    Quadruped,
    /// A long body with no legs: a snake, a worm, a dragon in flight.
    Serpentine,
    /// A body with wings as the dominant feature.
    Winged,
    /// No body plan at all -- a mass with features on it. A face in a
    /// cliff, an object, a machine.
    Mass,
}

impl BodyPlan {
    pub fn parse(s: &str) -> Option<BodyPlan> {
        let l = s.to_lowercase();
        let has = |w: &str| l.split(|c: char| !c.is_alphanumeric()).any(|x| x == w);
        // ORDER MATTERS: a winged quadruped is winged, and a serpentine
        // dragon with wings is winged before it is serpentine, because
        // the dominant feature is what a silhouette shows.
        if has("winged") || has("wings") || has("bird") || has("wing") { return Some(BodyPlan::Winged); }
        if has("serpentine") || has("snake") || has("serpent") || has("worm") || has("eel") {
            return Some(BodyPlan::Serpentine);
        }
        if has("quadruped") || has("four") || has("fourlegged") || has("squat")
            || has("crouching") || has("beast") || has("animal") {
            return Some(BodyPlan::Quadruped);
        }
        if has("biped") || has("upright") || has("standing") || has("human")
            || has("humanoid") || has("person") { return Some(BodyPlan::Biped); }
        if has("mass") || has("object") || has("none") { return Some(BodyPlan::Mass); }
        None
    }
    pub fn name(self) -> &'static str {
        match self { BodyPlan::Biped => "biped", BodyPlan::Quadruped => "quadruped",
                     BodyPlan::Serpentine => "serpentine", BodyPlan::Winged => "winged",
                     BodyPlan::Mass => "mass" }
    }
}

/// The body plan a subject implies, from its own words. A fallback for
/// when the model does not answer the PLAN line, and the reason a frog
/// is not a man even if the anatomy turn says nothing useful.
pub fn plan_from_subject(subject: &str) -> BodyPlan {
    let l = subject.to_lowercase();
    let has = |w: &str| l.split(|c: char| !c.is_alphanumeric()).any(|x| x == w || x == &format!("{w}s"));
    for w in ["frog", "toad", "horse", "lion", "dog", "wolf", "cat", "bear", "bull", "ox",
              "deer", "boar", "tiger", "elephant", "cow", "ram", "goat", "sphinx", "beast"] {
        if has(w) { return BodyPlan::Quadruped; }
    }
    for w in ["snake", "serpent", "worm", "eel", "hydra", "leviathan", "naga"] {
        if has(w) { return BodyPlan::Serpentine; }
    }
    for w in ["eagle", "bird", "griffin", "phoenix", "angel", "harpy", "raven", "owl", "hawk"] {
        if has(w) { return BodyPlan::Winged; }
    }
    // a dragon is winged unless it is said not to be
    if has("dragon") || has("wyvern") {
        return if l.contains("wingless") { BodyPlan::Serpentine } else { BodyPlan::Winged };
    }
    for w in ["orb", "sphere", "cube", "monolith", "obelisk", "machine", "engine", "skull", "head"] {
        if has(w) { return BodyPlan::Mass; }
    }
    BodyPlan::Biped
}

/// One rung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rung { Subject, Anatomy, Pose, Voxelise }

impl Rung {
    pub fn name(self) -> &'static str {
        match self { Rung::Subject => "subject", Rung::Anatomy => "anatomy",
                     Rung::Pose => "pose", Rung::Voxelise => "voxelise" }
    }
    pub fn all() -> [Rung; 4] { [Rung::Subject, Rung::Anatomy, Rung::Pose, Rung::Voxelise] }
}

/// What the ladder has learned so far. Every rung's answer is kept as
/// lines under headings, because the next rung needs it and a summary
/// of a summary is nothing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Study {
    pub subject: String,
    /// HOW THE BODY IS ARRANGED. `to_sketch` assumed a standing human
    /// -- one torso, one head above it, an arm pair at the shoulders,
    /// a leg pair below -- and built that for everything. Asked for a
    /// giant frog it produced a man. A frog is squat, four-legged and
    /// splayed, and no amount of correct PROPORTIONS fixes a body plan
    /// that is wrong.
    pub plan: BodyPlan,
    pub facts: BTreeMap<String, String>,
    /// Parts with proportions, as (name, fraction of total height).
    pub parts: Vec<(String, f32)>,
    pub pose: Vec<String>,
    pub silhouette: String,
    /// What survives voxelisation and what to drop.
    pub keep: Vec<String>,
    pub drop: Vec<String>,
    pub notes: Vec<String>,
}

impl Study {
    /// What the next rung needs to know, compactly.
    pub fn brief(&self, upto: Rung) -> String {
        let mut s = String::new();
        if !self.subject.is_empty() { s.push_str(&format!("SUBJECT: {}\n", self.subject)); }
        for (k, v) in &self.facts { s.push_str(&format!("{k}: {v}\n")); }
        if upto >= Rung::Pose && !self.parts.is_empty() {
            s.push_str(&format!("PARTS: {}\n", self.parts.iter()
                .map(|(n, f)| format!("{n} {:.0}%", f * 100.0)).collect::<Vec<_>>().join("; ")));
        }
        if upto >= Rung::Voxelise {
            if !self.pose.is_empty() { s.push_str(&format!("POSE: {}\n", self.pose.join("; "))); }
            if !self.silhouette.is_empty() { s.push_str(&format!("SILHOUETTE: {}\n", self.silhouette)); }
        }
        s
    }

    pub fn summary(&self) -> String {
        format!("{} -- {} part(s), {} pose note(s), {} kept, {} dropped",
                if self.subject.is_empty() { "(no subject)" } else { &self.subject },
                self.parts.len(), self.pose.len(), self.keep.len(), self.drop.len())
    }
}

/// How many rungs are worth climbing at this size.
///
/// A six-block statue has no room for an eagle at its foot, and asking
/// four questions to build one is three turns of latency for nothing.
/// The thresholds are where a feature can physically land: below ~10
/// blocks a figure is a silhouette, by 20 it has limbs worth posing,
/// by 40 it has surface.
pub fn depth_for(height_blocks: i32) -> usize {
    match height_blocks {
        h if h < 8 => 0,
        h if h < 16 => 1,
        h if h < 30 => 3,
        _ => 4,
    }
}

/// Split a list. SEMICOLONS WIN when they are present: a clause may
/// contain a comma -- "left arm forward, low" is one pose note, not
/// two -- and splitting on both turned five notes into seven. Commas
/// are the fallback for a model that used them as the separator.
fn parse_pairs(rest: &str) -> Vec<String> {
    let sep: &[char] = if rest.contains(';') { &[';'] } else { &[','] };
    rest.split(sep).map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() < 90).collect()
}

/// Read a rung's reply into the study. Tolerant: a model writing
/// prose under headings is the normal case.
pub fn absorb(study: &mut Study, rung: Rung, reply: &str) {
    for line in reply.lines() {
        let t = line.trim().trim_start_matches(['-', '*', '•']).trim();
        if t.is_empty() { continue; }
        let Some((head, rest)) = t.split_once(':') else {
            if rung == Rung::Subject && study.subject.is_empty() { study.subject = t.to_string(); }
            continue;
        };
        let (head, rest) = (head.trim().to_uppercase(), rest.trim());
        match head.as_str() {
            "SUBJECT" | "WHO" | "IT IS" => study.subject = rest.to_string(),
            "PARTS" | "ANATOMY" => {
                for p in parse_pairs(rest) {
                    // "head 12%" or "head 0.12" or just "head"
                    let (name, frac) = match p.rsplit_once(' ') {
                        Some((n, v)) => {
                            let v = v.trim_end_matches('%');
                            match v.parse::<f32>() {
                                Ok(x) if v.len() < 6 => (n.trim().to_string(),
                                                         if x > 1.0 { x / 100.0 } else { x }),
                                _ => (p.clone(), 0.0),
                            }
                        }
                        None => (p.clone(), 0.0),
                    };
                    if !name.is_empty() { study.parts.push((name, frac)); }
                }
            }
            "PLAN" | "BODY" | "BODY PLAN" | "ARRANGEMENT" =>
                if let Some(p) = BodyPlan::parse(rest) { study.plan = p; },
            "POSE" | "ACTION" | "STANCE" => study.pose.extend(parse_pairs(rest)),
            "SILHOUETTE" | "OUTLINE" => study.silhouette = rest.to_string(),
            "KEEP" | "SURVIVES" | "READS" => study.keep.extend(parse_pairs(rest)),
            "DROP" | "LOSE" | "OMIT" | "CANNOT" => study.drop.extend(parse_pairs(rest)),
            other => { study.facts.insert(other.to_string(), rest.to_string()); }
        }
    }
    // proportions that were given as bare names get an even share, so
    // a model that answered without numbers still produces a figure
    let unpriced = study.parts.iter().filter(|(_, f)| *f <= 0.0).count();
    if unpriced > 0 {
        let spoken: f32 = study.parts.iter().map(|(_, f)| *f).sum();
        let each = ((1.0 - spoken).max(0.1)) / unpriced as f32;
        for (_, f) in study.parts.iter_mut().filter(|(_, f)| *f <= 0.0) { *f = each; }
        study.notes.push(format!("{unpriced} part(s) had no proportion; shared the remainder"));
    }
    // If the model did not say, the SUBJECT says: a frog is a frog
    // whatever the anatomy turn answered, and defaulting to a man is
    // how "a giant frog statue" became a giant man.
    if rung == Rung::Anatomy && study.plan == BodyPlan::Biped {
        study.plan = plan_from_subject(&study.subject);
    }
    study.parts.truncate(14);
    study.pose.truncate(8);
    study.keep.truncate(8);
    study.drop.truncate(8);
}

/// The prompt for one rung.
pub fn prompt(rung: Rung, request: &str, height: i32, study: &Study) -> String {
    let known = study.brief(rung);
    match rung {
        Rung::Subject => format!("\
A player asked for: {request}

You are describing the SUBJECT so someone who has never seen it could recognise it. Be
specific. \"A god\" is useless; \"bearded, mature, bare-chested, a cloak over the left
shoulder\" is the answer.

SUBJECT: <one sentence naming exactly who or what>
BUILD: <body type, age, bearing>
DRESS: <what they wear, and how it hangs>
CARRIES: <what is in the hands, and which hand>
WITH: <anything at the feet or beside them>
KNOWN BY: <the ONE detail that identifies them at a glance>"),

        Rung::Anatomy => format!("\
{known}
This will be built {height} blocks tall.

Give the PARTS with their PROPORTIONS, as a percentage of the total height. A standing
human figure is about eight heads tall, shoulders two heads across; say what is true of
THIS subject, which may not be human and may not be standing.

SAY THE BODY PLAN FIRST. A frog is a QUADRUPED -- squat, four legs splayed from a low
wide body, head at the front, no neck. Building it as a standing human with correct
proportions still gives a man.

PLAN: <biped, quadruped, serpentine, winged or mass -- how the body is ARRANGED>
PARTS: head 12%; neck 3%; torso 30%; arm 34%; leg 47%; ...
WIDTH: <shoulders and hips, as a fraction of the height>
MASS: <where the weight sits: chest-heavy, long-limbed, squat>

Percentages need not sum to 100 -- limbs overlap the torso's span."),

        Rung::Pose => format!("\
{known}
What is the body DOING? Give it joint by joint, then say what the SILHOUETTE reads as
from fifty blocks away, in one line. The silhouette is what a voxel build has instead of
a face, and it is the only thing that carries at distance.

POSE: <shoulders>; <arms>; <hips>; <legs>; <head>
SILHOUETTE: <one line: the outline, and what makes it unmistakable>
WEIGHT: <which foot bears it>"),

        Rung::Voxelise => format!("\
{known}
Now think as a VOXEL BUILDER. At {height} blocks tall, one block is about {} of the
figure's height. Anything smaller than a block does not exist.

KEEP: <the features that survive at this size, and what to exaggerate so they read>
DROP: <the features that cannot survive, and what to replace them with>
MASS: <what to build as one solid form rather than as separate parts>

A beard survives if it is a mass; it does not if it is strands. Fingers never survive --
a hand is a block. Say what to do INSTEAD of each thing that is lost.", 
            if *&height > 0 { format!("1/{height}") } else { "?".into() }),
    }
}

/// The ladder as a plan the host can execute: which rungs, in order.
pub fn rungs_for(height: i32) -> Vec<Rung> {
    Rung::all().into_iter().take(depth_for(height)).collect()
}

/// The study as guidance for the sketch pass -- what to build, what to
/// make big enough to see, what not to attempt.
pub fn to_guidance(s: &Study, height: i32) -> String {
    let mut out = format!("BUILD {height} BLOCKS TALL: {}\n", s.subject);
    if !s.silhouette.is_empty() { out.push_str(&format!("It must read as: {}\n", s.silhouette)); }
    if !s.parts.is_empty() {
        out.push_str("Parts, in blocks:\n");
        for (n, f) in &s.parts {
            let b = (f * height as f32).round().max(1.0) as i32;
            out.push_str(&format!("  {n:<14} {b} block(s)\n"));
        }
    }
    if !s.pose.is_empty() { out.push_str(&format!("Pose: {}\n", s.pose.join("; "))); }
    if !s.keep.is_empty() { out.push_str(&format!("MAKE THESE READ: {}\n", s.keep.join("; "))); }
    if !s.drop.is_empty() { out.push_str(&format!("DO NOT ATTEMPT: {}\n", s.drop.join("; "))); }
    out
}

/// The study as GEOMETRY: parts and relations at the proportions the
/// model gave.
///
/// The ladder gathered "head 12%, torso 32%, leg 48%, bolt 18%" and
/// then handed off to `form statue`, which draws the same figure it
/// always draws. Every number was collected and discarded. This is
/// where the proportions become a body.
///
/// Parts are matched by NAME to a role, because a model writes "leg"
/// and "legs" and "left leg" and means the same thing. An unrecognised
/// part becomes a mass attached to the torso rather than being
/// dropped -- a thunderbolt is not a limb and still has to exist.
pub fn to_sketch(st: &Study, height: i32, material: &str) -> String {
    let h = height.max(4) as f32;
    let blocks = |f: f32| (f * h).round().max(1.0);
    let find = |keys: &[&str]| -> Option<f32> {
        st.parts.iter().find(|(n, _)| {
            let n = n.to_lowercase();
            keys.iter().any(|k| n.split_whitespace().any(|w| w == *k) || n == *k)
        }).map(|(_, f)| *f)
    };
    // The core proportions, with fallbacks that are a human figure --
    // a model that answered half the question still gets a body.
    let head = find(&["head", "skull"]).unwrap_or(0.12);
    let torso = find(&["torso", "body", "chest", "trunk"]).unwrap_or(0.32);
    let leg = find(&["leg", "legs", "thigh"]).unwrap_or(0.47);
    let arm = find(&["arm", "arms"]).unwrap_or(0.36);
    let neck = find(&["neck"]).unwrap_or(0.03);
    let width = st.facts.get("WIDTH").and_then(|v| v.split_whitespace()
        .find_map(|w| w.trim_end_matches('%').parse::<f32>().ok()))
        .map(|v| if v > 1.0 { v / 100.0 } else { v }).unwrap_or(0.24);
    let pose = st.pose.join(" ").to_lowercase();
    let raised = pose.contains("raised") || pose.contains("aloft") || pose.contains("up");
    let seated = pose.contains("seated") || pose.contains("sitting") || pose.contains("enthroned");
    let mut out = String::new();
    // torso: a tapering capsule, upright, the trunk of the thing
    let torso_r = (blocks(width) / 2.0).max(1.5);
    // A PERCENTAGE IS A SPAN, and a capsule spans `len + 2r`.
    //
    // "leg 47%" means the leg occupies nearly half the figure, caps
    // and all. Passing that percentage straight in as `len` made every
    // part longer than the model asked by its two radii -- a torso of
    // length 13 and radius 5 is 23 tall, not 13 -- so a 40-block
    // figure came out 50 and nothing noticed. The segment is the span
    // MINUS the caps.
    //
    // The parts also overlap rather than stacking: the hip joint is
    // inside the torso and the neck inside the shoulders, which is why
    // head, torso and leg sum to over 90% and are still a whole
    // figure. The torso sits down into the legs by that surplus. Both
    // corrections are needed and neither substitutes for the other.
    let leg_r = (torso_r * 0.45).max(1.2);
    let seg = |span: f32, r: f32| (span - 2.0 * r).max(1.0);
    let stack = head + torso + leg;
    let overlap = ((stack - 1.0).max(0.0) * h).round().max(0.0);
    // THE BODY PLAN DECIDES THE ARRANGEMENT. A quadruped's torso lies
    // HORIZONTAL with four legs under it and the head at the front; a
    // serpent has no legs at all. Emitting an upright torso with an
    // arm pair for everything is how a giant frog came out a giant
    // man, and no correction to the proportions could have fixed it.
    if st.plan != BodyPlan::Biped {
        return to_sketch_nonhuman(st, height, material, torso_r, blocks(head), blocks(torso),
                                  blocks(leg), blocks(arm));
    }
    out.push_str(&format!("part torso capsule r={:.1} r2={:.1} len={} dir=0 pitch=90 material={material}
",
                          torso_r, torso_r * 0.78, seg(blocks(torso), torso_r)));
    out.push_str(&format!("part head sphere r={:.1} material={material}
",
                          (blocks(head) / 2.0).max(1.0)));
    if neck > 0.01 {
        out.push_str(&format!("part neck capsule r={:.1} r2={:.1} len={} pitch=90 material={material}
",
                              (torso_r * 0.4).max(1.0), (torso_r * 0.4).max(1.0), blocks(neck)));
    }
    // limbs: a PAIR each, hanging from a joint. Without the pair both
    // legs land in the same place, which is what a figure did before
    // the relation existed.
    out.push_str(&format!("part arm capsule r={:.1} r2={:.1} len={} dir={} pitch={} material={material}
",
                          (torso_r * 0.35).max(1.0), (torso_r * 0.26).max(0.8), blocks(arm),
                          if raised { 200 } else { 190 }, if raised { 35 } else { -70 }));
    out.push_str(&format!("part leg capsule r={:.1} r2={:.1} len={} dir=190 pitch={} material={material}
",
                          leg_r, (torso_r * 0.34).max(1.0), seg(blocks(leg), leg_r),
                          if seated { -40 } else { -85 }));
    // anything else the model named, as a mass on the torso: a
    // thunderbolt, a beard, an eagle. Not a limb, and not droppable.
    let core = ["head", "skull", "torso", "body", "chest", "trunk", "leg", "legs", "thigh",
                "arm", "arms", "neck"];
    let mut extras = 0;
    for (n, f) in &st.parts {
        let low = n.to_lowercase();
        if core.iter().any(|c| low.split_whitespace().any(|w| w == *c) || low == *c) { continue; }
        if extras >= 4 { break; }
        let clean: String = low.chars().filter(|c| c.is_alphanumeric()).collect();
        if clean.is_empty() { continue; }
        // KEEP says what to exaggerate so it reads at this size
        let boost = if st.keep.iter().any(|k| k.to_lowercase().contains(&clean)) { 1.8 } else { 1.0 };
        out.push_str(&format!("part {clean} capsule r={:.1} r2={:.1} len={} dir=0 pitch=20 material={material}
",
                              (blocks(*f) * 0.16 * boost).max(1.0),
                              (blocks(*f) * 0.10 * boost).max(0.8), blocks(*f)));
        extras += 1;
    }
    // and the relations
    out.push_str("rel torso above ground by=");
    // the torso sits DOWN into the legs by the overlap the percentages
    // imply, because the hip joint is inside the torso and not below it
    out.push_str(&format!("{}
", (blocks(leg) - overlap).round().max(1.0) as i32));
    out.push_str("rel head on torso
");
    if neck > 0.01 { out.push_str("rel neck on torso
"); }
    out.push_str("rel arm joint torso at=0.88 out=1.3 pair
");
    out.push_str("rel leg joint torso at=0.05 out=0.55 pair
");
    out.push_str("rel leg reaches ground
");
    extras = 0;
    for (n, _) in &st.parts {
        let low = n.to_lowercase();
        if core.iter().any(|c| low.split_whitespace().any(|w| w == *c) || low == *c) { continue; }
        if extras >= 4 { break; }
        let clean: String = low.chars().filter(|c| c.is_alphanumeric()).collect();
        if clean.is_empty() { continue; }
        // held things go to the hand, worn things to the torso, things
        // at the feet to the ground
        let at_feet = st.facts.get("WITH").map(|v| v.to_lowercase().contains(&clean)).unwrap_or(false);
        if at_feet {
            out.push_str(&format!("rel {clean} beside torso dir=west by=2
"));
        } else {
            out.push_str(&format!("rel {clean} joint arm at=0.9 out=0.4
"));
        }
        extras += 1;
    }
    out
}
/// Bodies that are not standing people.
fn to_sketch_nonhuman(st: &Study, height: i32, material: &str, r: f32,
                      head: f32, torso: f32, leg: f32, arm: f32) -> String {
    let mut o = format!("# {} ({}) at {height} blocks\n", st.subject, st.plan.name());
    let seg = |span: f32, rr: f32| (span - 2.0 * rr).max(1.0);
    match st.plan {
        BodyPlan::Quadruped => {
            // A LOW HORIZONTAL BODY. The torso lies along z, the head
            // is at the FRONT rather than on top, and four legs splay
            // from it -- the two pairs at different heights on the
            // body, which is what makes a crouch read as a crouch.
            let squat = st.pose.join(" ").to_lowercase().contains("squat")
                || st.pose.join(" ").to_lowercase().contains("crouch")
                || st.subject.to_lowercase().contains("frog");
            let leg_r = (r * 0.4).max(1.0);
            let leg_len = if squat { leg * 0.5 } else { leg };
            o.push_str(&format!("part body capsule r={:.1} r2={:.1} len={:.0} dir=90 pitch=0 material={material}\n",
                                r, r * 0.85, seg(torso.max(head * 2.0), r)));
            o.push_str(&format!("part head sphere r={:.1} material={material}\n", (head / 2.0).max(1.0)));
            // splayed: a frog's legs go OUT before they go down, which
            // is the whole silhouette of a frog
            o.push_str(&format!("part foreleg capsule r={:.1} r2={:.1} len={:.0} dir=215 pitch=-{} material={material}\n",
                                leg_r, leg_r * 0.7, leg_len, if squat { 45 } else { 78 }));
            o.push_str(&format!("part hindleg capsule r={:.1} r2={:.1} len={:.0} dir=215 pitch=-{} material={material}\n",
                                leg_r * 1.25, leg_r * 0.8, leg_len * 1.15, if squat { 38 } else { 80 }));
            o.push_str(&format!("rel body above ground by={:.0}\n", leg_len * 0.75));
            o.push_str("rel head joint body at=0.5 out=1.15\n");
            o.push_str("rel foreleg joint body at=0.35 out=0.9 pair\n");
            o.push_str("rel hindleg joint body at=0.35 out=0.9 pair\n");
            o.push_str("rel foreleg reaches ground\nrel hindleg reaches ground\n");
            let _ = arm;
        }
        BodyPlan::Serpentine => {
            // one long arching body and a head at its end; no limbs
            o.push_str(&format!("part body spine r={:.1} r2={:.1} len={:.0} dir=90 arch=0.35 segments=10 material={material}\n",
                                r, r * 0.35, (torso + leg).max(height as f32 * 0.8)));
            o.push_str(&format!("part head sphere r={:.1} material={material}\n", (head / 2.0).max(1.0)));
            o.push_str(&format!("rel body above ground by={:.0}\n", r));
            o.push_str("rel head joint body at=0.95 out=0.9\n");
        }
        BodyPlan::Winged => {
            // an upright body whose WINGS are the silhouette: they are
            // the thing it is known by, so they are built large and
            // swept back rather than as an afterthought
            let wing = st.parts.iter().find(|(n, _)| n.to_lowercase().contains("wing"))
                .map(|(_, f)| *f * height as f32).unwrap_or(height as f32 * 0.55);
            o.push_str(&format!("part body capsule r={:.1} r2={:.1} len={:.0} pitch=90 material={material}\n",
                                r, r * 0.8, seg(torso, r)));
            o.push_str(&format!("part head sphere r={:.1} material={material}\n", (head / 2.0).max(1.0)));
            o.push_str(&format!("part wing capsule r={:.1} r2={:.1} len={wing:.0} dir=200 pitch=25 material={material}\n",
                                (r * 0.5).max(1.0), (r * 0.2).max(0.8)));
            o.push_str(&format!("part leg capsule r={:.1} r2={:.1} len={leg:.0} dir=190 pitch=-80 material={material}\n",
                                (r * 0.35).max(1.0), (r * 0.25).max(0.8)));
            o.push_str(&format!("rel body above ground by={leg:.0}\n"));
            o.push_str("rel head on body\n");
            o.push_str("rel wing joint body at=0.75 out=1.0 pair\n");
            o.push_str("rel leg joint body at=0.05 out=0.5 pair\n");
            o.push_str("rel leg reaches ground\n");
        }
        _ => {
            // a mass with its features on it, and nothing pretending
            // to be anatomy
            // A mass still needs to STAND on something: a lone sphere
            // floating at head height is not an object, it is a
            // balloon. A plinth is what a monolith or a carved head
            // sits on, and it is what makes it read as placed.
            o.push_str(&format!("part mass sphere r={:.1} material={material}\n",
                                (height as f32 * 0.35).max(2.0)));
            o.push_str(&format!("part plinth box w={:.0} h={:.0} d={:.0} material={material}\n",
                                (height as f32 * 0.55).max(3.0), (height as f32 * 0.18).max(2.0),
                                (height as f32 * 0.55).max(3.0)));
            o.push_str("rel plinth on ground\n");
            o.push_str("rel mass on plinth\n");
        }
    }
    // whatever else the study named, on the body
    let core = ["head", "torso", "body", "leg", "arm", "wing", "neck", "chest"];
    let mut extras = 0;
    for (n, f) in &st.parts {
        let low = n.to_lowercase();
        if core.iter().any(|c| low.contains(c)) { continue; }
        if extras >= 3 { break; }
        let clean: String = low.chars().filter(|c| c.is_alphanumeric()).collect();
        if clean.is_empty() { continue; }
        let boost = if st.keep.iter().any(|k| k.to_lowercase().contains(&clean)) { 1.8 } else { 1.0 };
        o.push_str(&format!("part {clean} sphere r={:.1} material={material}\n",
                            (f * height as f32 * 0.2 * boost).max(1.0)));
        o.push_str(&format!("rel {clean} joint {} at=0.6 out=1.0\n",
                            if st.plan == BodyPlan::Mass { "mass" } else { "body" }));
        extras += 1;
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zeus() -> Study {
        let mut s = Study::default();
        absorb(&mut s, Rung::Subject,
            "SUBJECT: Zeus, king of the gods, enthroned or striding\n\
             BUILD: mature, powerfully built, bare-chested\n\
             DRESS: a himation draped over the left shoulder and across the hips\n\
             CARRIES: a thunderbolt in the raised right hand\n\
             WITH: an eagle at the left foot\n\
             KNOWN BY: the beard and the raised bolt\n");
        absorb(&mut s, Rung::Anatomy,
            "PARTS: head 12%; beard 6%; torso 32%; arm 36%; leg 48%; bolt 18%; eagle 14%\n\
             WIDTH: shoulders 26% of height\n\
             MASS: chest-heavy, weight through the left leg\n");
        absorb(&mut s, Rung::Pose,
            "POSE: right arm raised and back; left arm forward, low; hips square; \
             left leg bearing, right trailing; head turned slightly right\n\
             SILHOUETTE: a broad triangle -- wide raised arm to planted feet, broken by the bolt\n\
             WEIGHT: the left foot\n");
        absorb(&mut s, Rung::Voxelise,
            "KEEP: the beard as one mass; the bolt, exaggerated to twice its scale; the raised arm\n\
             DROP: fingers -- the hand is one block; facial features -- the beard carries the face\n\
             MASS: torso and himation as one solid form\n");
        s
    }

    #[test]
    fn four_narrow_questions_gather_what_one_wide_one_does_not() {
        let s = zeus();
        assert!(s.subject.contains("Zeus"));
        // the identifying details a single interpret turn never asks for
        assert!(s.facts.get("CARRIES").is_some_and(|v| v.contains("thunderbolt")));
        assert!(s.facts.get("KNOWN BY").is_some_and(|v| v.contains("beard")));
        assert!(s.facts.get("WITH").is_some_and(|v| v.contains("eagle")));
        // proportions, as numbers
        assert_eq!(s.parts.len(), 7);
        let head = s.parts.iter().find(|(n, _)| n == "head").unwrap().1;
        assert!((head - 0.12).abs() < 0.001, "head is 12%: {head}");
        assert!(s.parts.iter().any(|(n, f)| n == "leg" && *f > 0.4));
        // the pose and the silhouette
        assert_eq!(s.pose.len(), 5);
        assert!(s.silhouette.contains("triangle"));
        // and the voxel judgement
        assert!(s.keep.iter().any(|k| k.contains("exaggerated")));
        assert!(s.drop.iter().any(|d| d.contains("fingers")));
    }

    #[test]
    fn proportions_become_blocks_at_the_size_asked_for() {
        let s = zeus();
        let g = to_guidance(&s, 50);
        assert!(g.contains("head") && g.contains("6 block(s)"), "12% of 50: {g}");
        assert!(g.contains("leg") && g.contains("24 block(s)"), "48% of 50: {g}");
        assert!(g.contains("MAKE THESE READ") && g.contains("DO NOT ATTEMPT"));
        assert!(g.contains("must read as"), "the silhouette carries forward");
        // and at a different size the numbers change
        let small = to_guidance(&s, 12);
        assert!(small.contains("head") && small.contains("1 block(s)"), "{small}");
        // nothing rounds to zero: a part that exists must be at least a block
        for line in small.lines().filter(|l| l.contains("block(s)")) {
            assert!(!line.contains(" 0 block"), "{line}");
        }
    }

    #[test]
    fn a_part_with_no_proportion_still_gets_one() {
        let mut s = Study::default();
        absorb(&mut s, Rung::Anatomy, "PARTS: head; torso; arm; leg\n");
        assert_eq!(s.parts.len(), 4);
        assert!(s.parts.iter().all(|(_, f)| *f > 0.0), "{:?}", s.parts);
        assert!(s.notes.iter().any(|n| n.contains("no proportion")), "{:?}", s.notes);
        // a model that half-answers gets the rest shared out
        let mut h = Study::default();
        absorb(&mut h, Rung::Anatomy, "PARTS: head 20%; torso; legs\n");
        let head = h.parts.iter().find(|(n, _)| n == "head").unwrap().1;
        assert!((head - 0.2).abs() < 0.001, "the stated one is kept: {head}");
        assert!(h.parts.iter().filter(|(n, _)| n != "head").all(|(_, f)| *f > 0.0));
    }

    #[test]
    fn the_ladder_is_only_climbed_when_there_is_room_for_the_detail() {
        // a six-block statue has no room for an eagle, and three extra
        // turns of latency buy nothing
        assert_eq!(depth_for(6), 0);
        assert_eq!(rungs_for(6).len(), 0);
        assert_eq!(depth_for(12), 1, "big enough for a subject, not for anatomy");
        assert_eq!(depth_for(24), 3);
        assert_eq!(depth_for(60), 4, "at this size every rung earns its turn");
        assert_eq!(rungs_for(60), Rung::all().to_vec());
        // and the order never changes: anatomy needs the subject
        assert_eq!(rungs_for(24), vec![Rung::Subject, Rung::Anatomy, Rung::Pose]);
    }

    #[test]
    fn each_prompt_carries_what_the_last_rung_learned() {
        let s = zeus();
        let anat = prompt(Rung::Anatomy, "a statue of Zeus", 50, &s);
        assert!(anat.contains("Zeus"), "the subject reached the anatomy question");
        assert!(anat.contains("50 blocks tall"));
        assert!(anat.contains("eight heads"), "it is told what proportion MEANS");
        let pose = prompt(Rung::Pose, "a statue of Zeus", 50, &s);
        assert!(pose.contains("head 12%"), "the anatomy reached the pose question: {pose}");
        assert!(pose.contains("fifty blocks away"), "the silhouette is asked for at distance");
        let vox = prompt(Rung::Voxelise, "a statue of Zeus", 50, &s);
        assert!(vox.contains("triangle"), "the silhouette reached the voxel question");
        assert!(vox.contains("1/50"), "and the block size is spelled out");
        assert!(vox.contains("Fingers never survive"));
        // the subject prompt asks for the identifying detail, which is
        // the thing a summary always loses
        let subj = prompt(Rung::Subject, "a statue of Zeus", 50, &Study::default());
        assert!(subj.contains("KNOWN BY"));
        assert!(subj.contains("\"A god\" is useless"), "it says why specificity matters");
    }

    #[test]
    fn a_reply_that_is_only_prose_still_yields_a_subject() {
        let mut s = Study::default();
        absorb(&mut s, Rung::Subject, "He's the king of the gods, bearded and holding a bolt.\n");
        assert!(s.subject.contains("king of the gods"), "{:?}", s.subject);
    }

    #[test]
    fn the_proportions_become_a_body_rather_than_being_discarded() {
        // the ladder gathered "head 12%, torso 32%, leg 48%" and then
        // handed off to `form statue`, which draws the same figure it
        // always draws. Every number was collected and thrown away.
        let st = zeus();
        let src = to_sketch(&st, 100, "stone");
        let (sk, errs) = crate::sketch::parse(&src);
        assert!(errs.is_empty(), "{src}\n{errs:?}");
        // the parts the model named are all there
        let names: Vec<&str> = sk.parts.iter().map(|p| p.name.as_str()).collect();
        for want in ["torso", "head", "arm", "leg"] {
            assert!(names.contains(&want), "{want} missing from {names:?}");
        }
        assert!(names.iter().any(|n| *n == "bolt" || *n == "beard" || *n == "eagle"),
                "the things that make it Zeus: {names:?}");
        // AT THE PROPORTIONS GIVEN: 12% of 100 is a head about 6 across
        let head = sk.parts.iter().find(|p| p.name == "head").unwrap();
        match head.shape { crate::sketch::Shape::Sphere { r } =>
            assert!((r - 6.0).abs() < 1.5, "head radius {r} for 12% of 100"), ref o => panic!("{o:?}") }
        // The percentage is the part's SPAN, caps included -- `len` is
        // the segment between the caps, and asserting on it was the
        // very confusion that made every figure a quarter too tall.
        let leg = sk.parts.iter().find(|p| p.name == "leg").unwrap();
        match leg.shape { crate::sketch::Shape::Capsule { len, r, .. } =>
            assert!((len + 2.0 * r - 48.0).abs() < 3.0,
                    "leg spans {} for 48% of 100 (len {len}, r {r})", len + 2.0 * r),
            ref o => panic!("{o:?}") }
        // and it resolves to a connected figure of about the right height
        let (placed, _) = crate::sketch::resolve(&sk, (0, 64, 0));
        let cells: Vec<(i32, i32, i32, u16)> = placed.iter()
            .flat_map(|p| crate::sketch::cells(p, 1)).collect();
        assert!(!cells.is_empty());
        let (lo, hi) = (cells.iter().map(|c| c.1).min().unwrap(), cells.iter().map(|c| c.1).max().unwrap());
        assert!((hi - lo) > 60, "a figure of some height: {}", hi - lo);
        // limbs came in PAIRS, so it is not one-legged
        let placed_leg = placed.iter().find(|p| p.name == "leg").unwrap();
        assert_eq!(placed_leg.extra.len(), 1, "one leg each side");
        assert_eq!(placed.iter().find(|p| p.name == "arm").unwrap().extra.len(), 1);
        let xs: std::collections::BTreeSet<i32> = cells.iter().map(|c| c.0).collect();
        assert!(xs.iter().any(|x| *x < -3) && xs.iter().any(|x| *x > 3), "it has two sides");
    }

    #[test]
    fn a_different_proportion_gives_a_different_body() {
        // the whole point: the numbers must change the shape
        let mut squat = Study::default();
        absorb(&mut squat, Rung::Anatomy, "PARTS: head 20%; torso 45%; leg 25%; arm 30%\nWIDTH: shoulders 45%\n");
        let mut lanky = Study::default();
        absorb(&mut lanky, Rung::Anatomy, "PARTS: head 8%; torso 25%; leg 62%; arm 45%\nWIDTH: shoulders 16%\n");
        let one = crate::sketch::parse(&to_sketch(&squat, 60, "stone")).0;
        let two = crate::sketch::parse(&to_sketch(&lanky, 60, "stone")).0;
        let leg = |s: &crate::sketch::Sketch| match s.parts.iter().find(|p| p.name == "leg").unwrap().shape {
            crate::sketch::Shape::Capsule { len, .. } => len, _ => 0.0 };
        let torso_r = |s: &crate::sketch::Sketch| match s.parts.iter().find(|p| p.name == "torso").unwrap().shape {
            crate::sketch::Shape::Capsule { r, .. } => r, _ => 0.0 };
        assert!(leg(&two) > leg(&one) * 2.0, "the lanky one has long legs: {} vs {}", leg(&two), leg(&one));
        assert!(torso_r(&one) > torso_r(&two) * 2.0, "and the squat one is broad");
    }

    #[test]
    fn the_pose_changes_the_arms_and_a_seat_changes_the_legs() {
        let mut raised = Study::default();
        absorb(&mut raised, Rung::Anatomy, "PARTS: torso 32%; arm 36%; leg 47%\n");
        absorb(&mut raised, Rung::Pose, "POSE: the right arm raised high\n");
        let mut sat = Study::default();
        absorb(&mut sat, Rung::Anatomy, "PARTS: torso 32%; arm 36%; leg 47%\n");
        absorb(&mut sat, Rung::Pose, "POSE: enthroned, seated on a great chair\n");
        let pitch = |src: &str, part: &str| {
            let s = crate::sketch::parse(src).0;
            match s.parts.iter().find(|p| p.name == part).unwrap().shape {
                crate::sketch::Shape::Capsule { pitch, .. } => pitch, _ => 0.0 }
        };
        assert!(pitch(&to_sketch(&raised, 50, "stone"), "arm") > 0.0, "a raised arm points UP");
        assert!(pitch(&to_sketch(&sat, 50, "stone"), "arm") < 0.0, "an ordinary one hangs");
        assert!(pitch(&to_sketch(&sat, 50, "stone"), "leg")
                > pitch(&to_sketch(&raised, 50, "stone"), "leg"), "a seated leg is not vertical");
    }

    #[test]
    fn a_study_that_answered_almost_nothing_still_makes_a_figure() {
        let mut thin = Study::default();
        absorb(&mut thin, Rung::Subject, "SUBJECT: a guardian\n");
        let src = to_sketch(&thin, 40, "stone");
        let (sk, errs) = crate::sketch::parse(&src);
        assert!(errs.is_empty(), "{src}\n{errs:?}");
        assert!(sk.parts.len() >= 4, "the fallbacks are a human figure: {}", sk.parts.len());
        let (placed, _) = crate::sketch::resolve(&sk, (0, 64, 0));
        assert!(placed.iter().flat_map(|p| crate::sketch::cells(p, 1)).count() > 100);
    }

    #[test]
    fn the_study_becomes_a_figure_built_to_what_it_learned() {
        // Four turns established the proportions, the pose, the mass
        // and what cannot exist -- and the build used the DICTIONARY
        // STATUE anyway, which knows none of it. The guidance reached
        // only the review, which can complain about a shape after it
        // is made and not before.
        let st = zeus();
        let src = to_sketch(&st, 160, "stone");
        let (sk, errs) = crate::sketch::parse(&src);
        assert!(errs.is_empty(), "{src}\n{errs:?}");
        let names: Vec<&str> = sk.parts.iter().map(|p| p.name.as_str()).collect();
        for want in ["torso", "leg", "head", "arm", "beard", "bolt", "eagle"] {
            assert!(names.contains(&want), "{want} is missing: {names:?}");
        }
        let (placed, _) = crate::sketch::resolve(&sk, (0, 64, 0));
        // THE PROPORTIONS ARE THE STUDIED ONES: head 12% of 160 is 19
        // blocks across, so a radius near 10
        let head = placed.iter().find(|p| p.name == "head").unwrap();
        match head.shape {
            crate::sketch::Shape::Sphere { r } => assert!((r - 9.6).abs() < 1.5, "head r={r}"),
            ref o => panic!("{o:?}"),
        }
        // the arms and legs are pairs, not one of each
        assert_eq!(placed.iter().find(|p| p.name == "arm").unwrap().extra.len(), 1);
        assert_eq!(placed.iter().find(|p| p.name == "leg").unwrap().extra.len(), 1);
        // THE BOLT IS EXAGGERATED, because the study said to: at voxel
        // scale a thing at true scale disappears
        let plain = { let mut c = st.clone(); c.keep.clear();
                      crate::sketch::parse(&to_sketch(&c, 160, "stone")).0 };
        let big_r = |sk: &crate::sketch::Sketch| sk.parts.iter().find(|p| p.name == "bolt")
            .map(|p| p.shape.extent()[1]).unwrap_or(0.0);
        assert!(big_r(&sk) > big_r(&plain), "{} vs {}", big_r(&sk), big_r(&plain));
        // and it builds something of about the height asked for
        let cells: usize = placed.iter().map(|p| crate::sketch::cells(p, 1).len()).sum();
        assert!(cells > 2000, "a colossus: {cells} cells");
    }

    #[test]
    fn a_part_this_vocabulary_has_no_word_for_is_still_built() {
        // dropping it would silently lose whatever made the subject
        // recognisable
        let mut s = Study::default();
        absorb(&mut s, Rung::Subject, "SUBJECT: a winged victory\n");
        absorb(&mut s, Rung::Anatomy, "PARTS: head 10%; torso 30%; leg 45%; arm 32%; wings 55%\n");
        let src = to_sketch(&s, 60, "quartz");
        assert!(src.contains("part wings"), "{src}");
        let (sk, errs) = crate::sketch::parse(&src);
        assert!(errs.is_empty(), "{errs:?}");
        assert!(sk.parts.iter().any(|p| p.name == "wings"));
        let (placed, _) = crate::sketch::resolve(&sk, (0, 64, 0));
        assert!(placed.iter().any(|p| p.name == "wings"));
    }

    #[test]
    fn the_pose_changes_the_figure_rather_than_being_noted_and_ignored() {
        let mut raised = zeus();
        let mut down = zeus();
        down.pose = vec!["both arms hanging at the sides".into()];
        let a = crate::sketch::parse(&to_sketch(&raised, 80, "stone")).0;
        let b = crate::sketch::parse(&to_sketch(&down, 80, "stone")).0;
        let arm_pitch = |sk: &crate::sketch::Sketch| match sk.parts.iter()
            .find(|p| p.name == "arm").unwrap().shape {
            crate::sketch::Shape::Capsule { pitch, .. } => pitch, ref o => panic!("{o:?}") };
        assert!(arm_pitch(&a) > 0.0, "raised arms point up: {}", arm_pitch(&a));
        assert!(arm_pitch(&b) < 0.0, "hanging arms point down: {}", arm_pitch(&b));
        // and a seated figure's legs are not a standing figure's
        raised.pose = vec!["enthroned, seated".into()];
        let c = crate::sketch::parse(&to_sketch(&raised, 80, "stone")).0;
        let leg_pitch = |sk: &crate::sketch::Sketch| match sk.parts.iter()
            .find(|p| p.name == "leg").unwrap().shape {
            crate::sketch::Shape::Capsule { pitch, .. } => pitch, ref o => panic!("{o:?}") };
        assert!(leg_pitch(&c) > leg_pitch(&a), "seated legs are less vertical");
    }

    #[test]
    fn the_height_asked_for_is_the_height_built() {
        // A model's percentages describe parts that OVERLAP: "leg 47%"
        // means the leg spans nearly half the figure, not that it owns
        // the bottom 47% alone. Stacking them raw made a 90-block Zeus
        // 121 blocks tall -- a third over, and nothing noticed.
        for want in [40, 90, 160] {
            let src = to_sketch(&zeus(), want, "stone");
            let (sk, _) = crate::sketch::parse(&src);
            let (placed, _) = crate::sketch::resolve(&sk, (0, 64, 0));
            let mut lo = i32::MAX; let mut hi = i32::MIN;
            for p in &placed {
                // The STATURE: head to heel. A raised arm and the bolt
                // in it legitimately reach above the head -- that is
                // what raised means -- so a figure's height is measured
                // by its body, as a person's is.
                if ["bolt", "eagle", "arm"].contains(&p.name.as_str()) { continue; }
                for c in crate::sketch::cells(p, 1) { lo = lo.min(c.1); hi = hi.max(c.1); }
            }
            let got = hi - lo + 1;
            // The height asked for is the height built, within a
            // block or two of rounding. It ran a QUARTER OVER until
            // two things were separated: a percentage is a part's
            // SPAN and a capsule spans `len + 2r`, so passing the
            // percentage in as `len` made every part longer than
            // asked by its two radii; and the parts OVERLAP rather
            // than stacking, because a hip joint is inside the torso.
            // Fixing either alone leaves the other.
            assert!((got - want).abs() * 100 / want <= 12,
                    "asked for {want}, built {got} ({}% out)", (got - want) * 100 / want);
        }
    }

    #[test]
    fn a_frog_is_not_a_man() {
        // "create a giant emerald frog statue" built a HUMAN, because
        // `to_sketch` emitted one upright torso, a head on top, an arm
        // pair and a leg pair for everything. Correct proportions on
        // the wrong body plan is still the wrong creature.
        let mut s = Study::default();
        absorb(&mut s, Rung::Subject, "SUBJECT: a giant frog, squat and wide\n");
        absorb(&mut s, Rung::Anatomy, "PARTS: head 22%; torso 40%; leg 45%\nWIDTH: 60%\n");
        assert_eq!(s.plan, BodyPlan::Quadruped, "the subject says so even if the model did not");
        let src = to_sketch(&s, 60, "emerald");
        let (sk, errs) = crate::sketch::parse(&src);
        assert!(errs.is_empty(), "{src}\n{errs:?}");
        let names: Vec<&str> = sk.parts.iter().map(|p| p.name.as_str()).collect();
        assert!(!names.contains(&"arm"), "a frog has no arms: {names:?}");
        assert!(names.contains(&"foreleg") && names.contains(&"hindleg"), "{names:?}");
        // the body is HORIZONTAL, not upright: that is the silhouette
        let body = sk.parts.iter().find(|p| p.name == "body").unwrap();
        match body.shape {
            crate::sketch::Shape::Capsule { pitch, .. } =>
                assert!(pitch.abs() < 30.0, "a frog's body lies down: pitch {pitch}"),
            ref o => panic!("{o:?}"),
        }
        // and it is wider than it is tall, which a standing man is not
        let (placed, _) = crate::sketch::resolve(&sk, (0, 64, 0));
        let cells: Vec<(i32, i32, i32, u16)> = placed.iter()
            .flat_map(|p| crate::sketch::cells(p, 1)).collect();
        let w = cells.iter().map(|c| c.0).max().unwrap() - cells.iter().map(|c| c.0).min().unwrap();
        let d = cells.iter().map(|c| c.2).max().unwrap() - cells.iter().map(|c| c.2).min().unwrap();
        let ht = cells.iter().map(|c| c.1).max().unwrap() - cells.iter().map(|c| c.1).min().unwrap();
        assert!(w.max(d) > ht, "squat: {}x{} across, {ht} tall", w, d);
    }

    #[test]
    fn the_subject_decides_the_plan_when_the_model_does_not_say() {
        for (subject, want) in [("a giant frog", BodyPlan::Quadruped),
                                ("a rearing horse", BodyPlan::Quadruped),
                                ("a coiled serpent", BodyPlan::Serpentine),
                                ("an eagle with spread wings", BodyPlan::Winged),
                                ("a dragon", BodyPlan::Winged),
                                ("a wingless dragon", BodyPlan::Serpentine),
                                ("Zeus, king of the gods", BodyPlan::Biped),
                                ("a monolith", BodyPlan::Mass)] {
            assert_eq!(plan_from_subject(subject), want, "{subject}");
        }
        // and an explicit PLAN line beats the guess
        let mut s = Study::default();
        absorb(&mut s, Rung::Subject, "SUBJECT: a giant frog\n");
        absorb(&mut s, Rung::Anatomy, "PLAN: winged\nPARTS: head 20%; torso 40%\n");
        assert_eq!(s.plan, BodyPlan::Winged, "the model overrides the guess");
    }

    #[test]
    fn every_plan_builds_something_and_none_of_them_is_the_same_shape() {
        let mut shapes = std::collections::BTreeSet::new();
        for plan in [BodyPlan::Biped, BodyPlan::Quadruped, BodyPlan::Serpentine,
                     BodyPlan::Winged, BodyPlan::Mass] {
            let mut s = Study::default();
            s.subject = "a thing".into();
            s.plan = plan;
            absorb(&mut s, Rung::Anatomy, "PARTS: head 15%; torso 35%; leg 40%; wing 50%\nWIDTH: 30%\n");
            s.plan = plan;   // absorb's fallback must not undo it
            let src = to_sketch(&s, 50, "stone");
            let (sk, errs) = crate::sketch::parse(&src);
            assert!(errs.is_empty(), "{plan:?}: {src}\n{errs:?}");
            // count through the PROGRAM, because `sketch::cells` only
            // draws the shapes with no IR op -- a sphere has one, so a
            // plan made of spheres rasterises to nothing here
            let prog_src = format!("pass a add\n{src}end\n");
            let (prog, perrs) = crate::program::parse(&prog_src);
            assert!(perrs.is_empty(), "{plan:?}: {perrs:?}");
            let run = crate::program::run(&prog, (0, 64, 0));
            assert!(run.changes.len() > 100, "{plan:?} built {} cells", run.changes.len());
            let names: Vec<String> = sk.parts.iter().map(|p| p.name.clone()).collect();
            assert!(shapes.insert(names.join(",")), "{plan:?} is the same shape as another plan");
        }
    }
}
