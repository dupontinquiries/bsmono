//! Four passes: what is it, what is it made of, where does it go, do it.
//!
//! THE PROBLEM WITH ONE PASS
//!
//! A model asked "build a water sphere" against a prompt listing
//! twelve form names replied `form house`. Not because it is small,
//! but because the prompt is a MENU and a water sphere is not on it.
//! Every build looked prebuilt because every build WAS prebuilt: the
//! model's only expressive act was choosing which function to call.
//!
//! THE SPLIT
//!
//! Ask a builder how they would make something strange and they do not
//! answer with coordinates. They answer with what the thing IS -- "a
//! ball of water needs a skin to hold it, so a hollow shell, and
//! something to look at inside" -- and then with parts and how the
//! parts relate. Coordinates come last and are mostly arithmetic.
//!
//! So the passes split on where a model is good:
//!
//!   1. INTERPRET  what is this thing? its parts, their materials,
//!                 what makes it recognisable, what it is NOT.
//!                 Prose. The model is very good at this.
//!   2. COMPOSE    parts as shapes with relations. Still no
//!                 coordinates: "the core is inside the shell".
//!   3. RESOLVE    relations to positions. CODE. Deterministic and
//!                 checkable, and where every past failure lived.
//!   4. EMIT       ops, then the existing pipeline: dry run,
//!                 critique, reachability, hive.
//!
//! Passes 1 and 2 are model turns. Pass 3 is `sketch::resolve`. Pass 4
//! is `sketch::to_ir` plus rasterisation for the shapes with no op.
//!
//! WHEN TO USE IT
//!
//! Not always. "A cottage" is a cottage, and the dictionary makes a
//! better one than four passes of a 1B will. The pipeline is for what
//! the dictionary CANNOT say -- and the honest test for that is
//! whether the recognizer found anything. If it did, the fast path is
//! right; if it did not, this is the difference between a water sphere
//! and a house.

/// What pass 1 produces: the model's reading of the request.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reading {
    /// One line: what the thing fundamentally is.
    pub essence: String,
    /// The parts it needs, in the model's words.
    pub parts: Vec<String>,
    /// Materials named.
    pub materials: Vec<String>,
    /// Roughly how big, in blocks.
    pub scale: Option<i32>,
    /// What would make it read WRONG. The most useful line, and the
    /// one a menu-driven prompt can never produce.
    pub pitfalls: Vec<String>,
}

impl Reading {
    /// Everything pass 2 needs to know, compactly.
    pub fn brief(&self) -> String {
        let mut s = format!("IT IS: {}\n", self.essence);
        if !self.parts.is_empty() { s.push_str(&format!("PARTS: {}\n", self.parts.join("; "))); }
        if !self.materials.is_empty() { s.push_str(&format!("MATERIALS: {}\n", self.materials.join(", "))); }
        if let Some(n) = self.scale { s.push_str(&format!("ABOUT {n} BLOCKS ACROSS\n")); }
        if !self.pitfalls.is_empty() { s.push_str(&format!("AVOID: {}\n", self.pitfalls.join("; "))); }
        s
    }
}

/// Parse pass 1's reply. Tolerant: a model writing prose under headings
/// is the normal case, and anything unlabelled becomes the essence.
pub fn parse_reading(reply: &str) -> Reading {
    let mut r = Reading::default();
    let mut loose: Vec<&str> = Vec::new();
    for line in reply.lines() {
        let t = line.trim().trim_start_matches(['-', '*', '•']).trim();
        if t.is_empty() { continue; }
        let (head, rest) = match t.split_once(':') { Some((a, b)) => (a.to_lowercase(), b.trim()), None => (String::new(), t) };
        let items = |s: &str| -> Vec<String> {
            s.split(|c| c == ';' || c == ',').map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty() && x.len() < 80).collect()
        };
        match head.as_str() {
            "it is" | "essence" | "what" | "is" => r.essence = rest.to_string(),
            "parts" | "part" | "pieces" | "components" => r.parts.extend(items(rest)),
            "materials" | "material" | "blocks" => r.materials.extend(items(rest)),
            "scale" | "size" | "about" =>
                r.scale = rest.split_whitespace().find_map(|w| w.trim_matches(|c: char| !c.is_ascii_digit()).parse().ok()),
            "avoid" | "pitfalls" | "not" | "wrong" => r.pitfalls.extend(items(rest)),
            _ => loose.push(t),
        }
    }
    if r.essence.is_empty() { r.essence = loose.first().unwrap_or(&"").to_string(); }
    r.parts.truncate(8); r.materials.truncate(6); r.pitfalls.truncate(4);
    r
}

/// The pass-1 prompt: how would a builder reason about this?
///
/// Deliberately asks for PROSE, and deliberately asks what would make
/// the thing read wrong. A model that has just written "a sphere of
/// water needs a shell to contain it, and a solid ball of water would
/// just look like a blue blob" will not then compose a house.
pub fn interpret_prompt(request: &str, site: &str) -> String {
    format!("\
A player in a voxel world asked for this:

  {request}

You are a builder who has made thousands of things in voxel worlds. Before building
anything, think about WHAT THIS IS. Not how to build it -- what it is.

Answer in these lines and nothing else:

IT IS: <one sentence: what this thing fundamentally is>
PARTS: <the pieces it needs, separated by semicolons>
MATERIALS: <what each part is made of>
SCALE: <roughly how many blocks across>
AVOID: <what would make it read WRONG or look like something else>

{site}

Some things are not buildings. A request may be a shape, a phenomenon, a feeling, a
piece of landscape, or something with no name. Say what it is, not what it is nearest to.

EXAMPLE
  player: a water sphere
IT IS: a ball of water held in the air, which needs something to hold it
PARTS: an outer shell of glass or ice; the water filling it; a support or nothing if it floats
MATERIALS: glass, water
SCALE: 16
AVOID: a solid ball of water reads as a blue blob; it needs a visible skin and something inside

EXAMPLE
  player: the sound of a bell, as a building
IT IS: not a bell, but the shape a bell's sound would make: rings spreading outward
PARTS: a tall central shaft; concentric rings around it at rising heights; the rings thinning outward
MATERIALS: stone, quartz
SCALE: 40
AVOID: building an actual bell; the rings must not touch, or it reads as a solid cone
")
}

/// The pass-2 prompt: the reading, into parts and relations.
pub fn compose_prompt(request: &str, reading: &Reading) -> String {
    format!("\
The player asked for: {request}

You already decided what it is:

{}
Now write it as PARTS and RELATIONS. Never write a coordinate -- positions are worked out
for you. Sizes are fine; positions are not.

  part NAME SHAPE [r=N] [w=N] [h=N] [d=N] [thickness=N] [turns=N] material=BLOCK [hollow]
  rel NAME RELATION OTHER [by=N] [dir=north|south|east|west] [count=N] [radius=N] [gap=N]

SHAPES: sphere, box, cylinder, torus (a ring), cone, helix (a spiral), slab, link (a beam
between two parts)

RELATIONS: inside, around, on, above, below, beside, level, between A B, ring_of (copies on
a circle), stack (copies stacked up)

`ground` is always available as the thing to relate to.

EXAMPLE
part shell sphere r=8 material=glass hollow
part water sphere r=7 material=water
part core sphere r=2 material=ice
rel shell above ground by=10
rel water inside shell
rel core inside water

EXAMPLE
part shaft cylinder r=2 h=40 material=stone
part ring torus r=10 thickness=1 material=quartz
rel shaft on ground
rel ring stack shaft count=5 gap=6

Write the parts and relations only.
", reading.brief())
}

/// Which path a request should take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// The dictionary knows this: one turn, a form line, done.
    Known,
    /// It does not: four passes.
    Compose,
}

/// Choose. The honest signal is whether the recognizer found anything:
/// it is a table of things the dictionary can actually make, so a miss
/// means the fast path would be guessing.
pub fn choose_path(prose: &str) -> Path {
    match crate::recipe::recognise(prose) {
        Some(_) => Path::Known,
        None => Path::Compose,
    }
}

/// Passes 3 and 4: a composed sketch to IR and cells.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Built {
    pub ir: String,
    /// Cells for the shapes with no op, as (x, y, z, block-name).
    pub cells: Vec<(i32, i32, i32, String)>,
    pub notes: Vec<String>,
    pub errors: Vec<String>,
    pub parts: usize,
}

pub fn build(sketch_src: &str, anchor: (i32, i32, i32)) -> Built {
    let (sk, errs) = crate::sketch::parse(sketch_src);
    let (placed, notes) = crate::sketch::resolve(&sk, anchor);
    let mut b = Built { ir: crate::sketch::to_ir(&placed), notes, errors: errs, parts: placed.len(), ..Default::default() };
    for p in crate::sketch::needs_cells(&placed) {
        for (x, y, z, _) in crate::sketch::cells(p, 1) {
            b.cells.push((x, y, z, p.material.clone()));
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_parses_from_the_headings_a_model_writes() {
        let r = parse_reading("IT IS: a ball of water held in the air\n\
                               PARTS: an outer shell of glass; the water inside; a core of ice\n\
                               MATERIALS: glass, water, ice\n\
                               SCALE: about 16 blocks\n\
                               AVOID: a solid blue blob; anything that looks like a house\n");
        assert_eq!(r.essence, "a ball of water held in the air");
        assert_eq!(r.parts.len(), 3);
        assert_eq!(r.materials, vec!["glass", "water", "ice"]);
        assert_eq!(r.scale, Some(16));
        assert!(r.pitfalls.iter().any(|p| p.contains("house")));
        let brief = r.brief();
        assert!(brief.contains("IT IS:") && brief.contains("AVOID:"));
        // a model that writes loose prose still gets an essence
        let loose = parse_reading("It's basically a floating orb of water.\nMATERIALS: water\n");
        assert!(loose.essence.contains("floating orb"), "{:?}", loose.essence);
        assert_eq!(loose.materials, vec!["water"]);
    }

    #[test]
    fn the_path_is_chosen_by_whether_the_dictionary_actually_knows_it() {
        assert_eq!(choose_path("a stone cottage"), Path::Known);
        assert_eq!(choose_path("carve a great schism"), Path::Known, "the carve table knows this");
        // the failing case, and the kind of thing the pipeline is for
        assert_eq!(choose_path("a water sphere"), Path::Compose);
        assert_eq!(choose_path("the sound of a bell, as a building"), Path::Compose);
        assert_eq!(choose_path("a spiral staircase to the sky"), Path::Compose);
    }

    #[test]
    fn the_interpret_prompt_asks_what_a_thing_is_not_which_form_to_pick() {
        let p = interpret_prompt("a water sphere", "SITE: grass, sea level");
        assert!(p.contains("WHAT THIS IS"), "{p}");
        assert!(p.contains("AVOID:"), "the pitfall line is the point");
        assert!(!p.contains("form house"), "no menu of forms anywhere in it");
        assert!(!p.to_lowercase().contains("choose"), "it does not ask the model to choose");
        assert!(p.contains("Some things are not buildings"));
        assert!(p.len() < 2200, "{} chars", p.len());
    }

    #[test]
    fn the_compose_prompt_forbids_coordinates_and_carries_the_reading() {
        let r = parse_reading("IT IS: a ball of water\nPARTS: a shell; the water\nAVOID: a blue blob\n");
        let p = compose_prompt("a water sphere", &r);
        assert!(p.contains("Never write a coordinate"), "{p}");
        assert!(p.contains("a ball of water") && p.contains("a blue blob"), "the reading carries forward");
        assert!(p.contains("torus") && p.contains("helix"), "the shapes are listed");
        // and its own examples are valid sketches
        for ex in p.split("EXAMPLE").skip(1) {
            let body: String = ex.lines().skip(1).map(|l| l.trim())
                .take_while(|l| l.starts_with("part") || l.starts_with("rel"))
                .map(|l| format!("{l}\n")).collect();
            let (sk, errs) = crate::sketch::parse(&body);
            assert!(errs.is_empty(), "{body}\n{errs:?}");
            assert!(sk.parts.len() >= 2, "{body}");
            let (placed, _) = crate::sketch::resolve(&sk, (0, 64, 0));
            assert_eq!(placed.len(), sk.parts.len());
        }
    }

    #[test]
    fn the_whole_pipeline_turns_a_water_sphere_into_a_water_sphere() {
        // pass 1, as a model would answer it
        let reading = parse_reading("IT IS: a ball of water held aloft, needing a skin\n\
                                     PARTS: a glass shell; water within; an ice core\n\
                                     MATERIALS: glass, water, ice\n\
                                     SCALE: 16\n\
                                     AVOID: a solid blue blob\n");
        // pass 2, as a model would answer it
        let sketch = "part shell sphere r=8 material=glass hollow\n\
                      part water sphere r=7 material=water\n\
                      part core sphere r=2 material=ice\n\
                      rel shell above ground by=10\n\
                      rel water inside shell\n\
                      rel core inside water\n";
        // passes 3 and 4
        let b = build(sketch, (0, 64, 0));
        assert!(b.errors.is_empty(), "{:?}", b.errors);
        assert_eq!(b.parts, 3);
        assert!(b.ir.contains("block=glass") && b.ir.contains("block=water") && b.ir.contains("block=ice"),
                "all three materials appear: {}", b.ir);
        assert!(!b.ir.contains("house"), "and no house anywhere");
        // it floats: the shell's centre is well above the ground
        let shell_line = b.ir.lines().find(|l| l.contains("glass")).unwrap();
        assert!(shell_line.starts_with("sphere 0,0 r=8"), "{shell_line}");
        for l in b.ir.lines() {
            assert!(crate::plan::line_is_valid_ir(l), "{l:?}");
        }
        let _ = reading;
    }

    #[test]
    fn a_strange_request_composes_rather_than_reaching_for_the_nearest_form() {
        // the bell-sound sketch from the prompt's own example
        let b = build("part shaft cylinder r=2 h=40 material=stone\n\
                       part ring torus r=10 thickness=1 material=quartz\n\
                       rel shaft on ground\n\
                       rel ring stack shaft count=5 gap=6\n", (0, 64, 0));
        assert!(b.errors.is_empty(), "{:?}", b.errors);
        // the rings have no IR op, so they arrive as cells
        assert!(!b.cells.is_empty(), "the rings rasterise");
        assert!(b.cells.iter().all(|(_, _, _, m)| m == "quartz"), "and they are quartz");
        // five of them, at rising heights
        let ys: std::collections::BTreeSet<i32> = b.cells.iter().map(|(_, y, _, _)| *y).collect();
        assert!(ys.len() > 5, "spread over height: {} distinct", ys.len());
        assert!(b.ir.contains("cylinder"), "the shaft has an op: {}", b.ir);
    }
}
