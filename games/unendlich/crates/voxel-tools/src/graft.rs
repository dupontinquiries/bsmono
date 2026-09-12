//! One thing in the manner of another.
//!
//! The crate builds one thing from one source. A request names a
//! subject, the subject picks a route, the route builds it. There is
//! no way at all to say:
//!
//!   a lighthouse made of bones
//!   a tower that branches like a tree
//!   a cathedral built the way a wasp builds
//!   a bridge in the manner of a spine
//!
//! Each names TWO things and takes something different from each: the
//! STRUCTURE from the first -- what parts it has and how they stack --
//! and the MANNER from the second -- what the parts are made of, how
//! they meet, how they repeat, whether they taper or branch or coil.
//!
//! This is the same arithmetic as `transform`. A category divides; a
//! graft multiplies. Any subject in the manner of any other, from one
//! operator and no new table of nouns.
//!
//! WHAT A MANNER IS
//!
//! Not a material. A manner is the small set of habits that make a
//! thing recognisable at the level of construction rather than shape:
//!
//!   how parts JOIN      welded smooth, socketed, stacked loose
//!   how they REPEAT     even rhythm, thinning outward, branching
//!   how they TAPER      constant, narrowing, swelling then narrowing
//!   what they are OF    the palette, which is the least of it
//!
//! A lighthouse in the manner of bone is not a white lighthouse. It is
//! a lighthouse whose parts socket into each other, thin toward their
//! ends, and are hollow. The material is the last and smallest part of
//! the answer, and treating it as the whole answer is why "made of
//! bones" would otherwise produce a white tower.

use std::collections::BTreeSet;

/// How parts of a thing meet each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Join { Welded, Socketed, Stacked, Woven, Grown }

/// How parts repeat along a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rhythm { Even, Thinning, Branching, Coiling, Clustered }

/// How a part changes along its length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taper { Constant, Narrowing, Swelling, Segmented }

/// A way of making things, borrowed from one subject and applied to
/// another.
#[derive(Debug, Clone, PartialEq)]
pub struct Manner {
    pub name: String,
    /// Other words for this way of making. A manner people reach for
    /// by a different word is a manner they cannot use: "in the manner
    /// of a spine" is the bone manner, and naming only `bone` misses
    /// it.
    pub also: Vec<String>,
    pub join: Join,
    pub rhythm: Rhythm,
    pub taper: Taper,
    /// Hollow things are lighter and read as grown or made; solid
    /// things read as carved or piled.
    pub hollow: bool,
    pub materials: Vec<String>,
    /// One line: what this manner IS, for a prompt and for the player.
    pub reads_as: String,
}

impl Manner {
    /// The sketch-language habits this manner implies, as lines a
    /// build can be told to follow. This is the whole point: a manner
    /// has to reach the geometry or it is decoration.
    pub fn habits(&self) -> Vec<String> {
        let mut v = Vec::new();
        v.push(match self.join {
            Join::Welded => "parts flow into each other with no seam".into(),
            Join::Socketed => "parts meet at a narrowed neck, one entering the other".into(),
            Join::Stacked => "parts sit on each other without blending".into(),
            Join::Woven => "parts cross and interlace rather than meeting end to end".into(),
            Join::Grown => "parts emerge from a parent, thickest where they leave it".into(),
        });
        v.push(match self.rhythm {
            Rhythm::Even => "repeats at a steady spacing".into(),
            Rhythm::Thinning => "repeats more sparsely and smaller toward the ends".into(),
            Rhythm::Branching => "each element splits into two smaller ones".into(),
            Rhythm::Coiling => "elements wind around the main run".into(),
            Rhythm::Clustered => "elements gather in knots with gaps between".into(),
        });
        v.push(match self.taper {
            Taper::Constant => "parts keep their thickness".into(),
            Taper::Narrowing => "parts thin toward their far end".into(),
            Taper::Swelling => "parts thicken in the middle and thin at both ends".into(),
            Taper::Segmented => "parts are made of repeated joints, not one length".into(),
        });
        if self.hollow { v.push("parts are hollow, walls rather than masses".into()); }
        if !self.materials.is_empty() {
            v.push(format!("built of {}", self.materials.join(" and ")));
        }
        v
    }

    /// A manner as instructions to append to a build prompt.
    pub fn prompt(&self) -> String {
        format!("\nIN THE MANNER OF {}, which reads as {}:\n{}\n\
                 Take the WAY it is made, not its shape. The thing being built keeps its own \
                 form; only how the parts join, repeat, taper and are made changes.\n",
                self.name.to_uppercase(), self.reads_as,
                self.habits().iter().map(|h| format!("  {h}")).collect::<Vec<_>>().join("\n"))
    }
}

/// The manners this crate knows how to describe.
///
/// Few and deliberately not a taxonomy of things -- these are ways of
/// MAKING, and a way of making applies to anything. Bone is a manner
/// whether the subject is a lighthouse or a bridge.
pub fn known() -> Vec<Manner> {
    let m = |name: &str, also: &[&str], join, rhythm, taper, hollow, mats: &[&str], reads: &str| Manner {
        name: name.into(), also: also.iter().map(|s| s.to_string()).collect(),
        join, rhythm, taper, hollow,
        materials: mats.iter().map(|s| s.to_string()).collect(),
        reads_as: reads.into(),
    };
    vec![
        m("bone", &["spine", "skeleton", "rib", "skull", "vertebra", "antler", "ivory"], Join::Socketed, Rhythm::Thinning, Taper::Narrowing, true,
          &["bone_block", "quartz"], "sockets, hollow shafts, everything thinning to a point"),
        m("tree", &["branch", "bough", "trunk", "wood", "forest", "vine"], Join::Grown, Rhythm::Branching, Taper::Narrowing, false,
          &["oak_log", "leaves"], "one trunk splitting again and again, each branch thinner"),
        m("crystal", &["shard", "gem", "quartz", "amethyst", "prism", "geode"], Join::Welded, Rhythm::Clustered, Taper::Narrowing, false,
          &["quartz", "glass"], "clusters of straight shafts at hard angles, all pointing out"),
        m("masonry", &["brick", "block", "course", "stonework", "wall"], Join::Stacked, Rhythm::Even, Taper::Constant, false,
          &["stone", "cobblestone"], "courses of even blocks, nothing tapering, everything level"),
        m("timber", &["frame", "beam", "plank", "lattice", "truss"], Join::Woven, Rhythm::Even, Taper::Constant, true,
          &["oak_planks", "dark_oak_planks"], "a frame of crossing members with the space between filled"),
        m("shell", &["nautilus", "conch", "spiral", "snail", "carapace"], Join::Welded, Rhythm::Coiling, Taper::Swelling, true,
          &["sandstone", "quartz"], "one continuous surface winding around itself, thin and hollow"),
        m("insect", &["chitin", "beetle", "ant", "wasp", "hive", "segment"], Join::Segmented(), Rhythm::Even, Taper::Segmented, true,
          &["stone", "obsidian"], "repeated identical segments, hard outside and hollow within"),
        m("root", &["rootwork", "tendril", "rhizome", "burrow"], Join::Grown, Rhythm::Branching, Taper::Narrowing, false,
          &["dark_oak_planks", "dirt"], "spreading downward and outward, splitting and rejoining"),
    ]
}

// `Segmented` is a taper, not a join; the insect manner sockets.
#[allow(non_snake_case)]
impl Join { fn Segmented() -> Join { Join::Socketed } }

/// Find a manner named in a request. This reads the phrases people
/// actually use for it, and nothing else in the module reads words.
pub fn read(request: &str) -> Option<(Manner, String)> {
    let l = request.to_lowercase();
    // "X in the manner of Y", "X made of Y", "X like a Y", "X built
    // the way a Y is"
    const MARKS: &[&str] = &["in the manner of", "in the style of", "made of", "made from",
                             "built like", "shaped like", "that is also", "like a", "like an",
                             "as if it were", "grown like"];
    let at = MARKS.iter().filter_map(|m| l.find(m).map(|i| (i, m.len()))).min_by_key(|(i, _)| *i)?;
    let (subject, rest) = (l[..at.0].trim().to_string(), l[at.0 + at.1..].trim());
    if subject.is_empty() { return None; }
    let manner = known().into_iter().find(|m| {
        rest.split(|c: char| !c.is_alphanumeric()).any(|w| {
            w == m.name || w == format!("{}s", m.name)
                || m.also.iter().any(|a| w == a || w == &format!("{a}s"))
                // the material names a manner too: "made of bones" is
                // the bone manner, not merely the bone palette
                || m.materials.iter().any(|mat| mat.split('_').next() == Some(w))
        })
    })?;
    Some((manner, subject))
}

/// Two things grafted: what to build, and how.
#[derive(Debug, Clone, PartialEq)]
pub struct Graft {
    /// The thing itself, which keeps its own structure.
    pub subject: String,
    pub manner: Manner,
}

impl Graft {
    pub fn summary(&self) -> String {
        format!("{} in the manner of {} -- {}", self.subject, self.manner.name,
                self.manner.reads_as)
    }
}

/// Apply a manner to cells that already exist: the part of a graft
/// that does not need a model.
///
/// Only the habits that can be applied AFTER the fact are here --
/// hollowing, and the palette. Join, rhythm and taper have to reach
/// the build while it is being planned, which is what `Manner::prompt`
/// is for. Pretending otherwise would make this a recolouring and call
/// it a graft.
pub fn apply_surface(m: &Manner, cells: &BTreeSet<(i32, i32, i32)>, seed: u64)
    -> Vec<crate::weather::Change> {
    let mut out = Vec::new();
    if cells.is_empty() || m.materials.is_empty() { return out; }
    let ids: Vec<u16> = m.materials.iter()
        .map(|s| crate::palette::resolve_or_suggest(s).0).collect();
    let solid = |c: (i32, i32, i32)| cells.contains(&c);
    let exposed = |c: (i32, i32, i32)| [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)]
        .iter().any(|d| !solid((c.0 + d.0, c.1 + d.1, c.2 + d.2)));
    for c in cells {
        let e = exposed(*c);
        if m.hollow && !e {
            // hollow: only the shell survives
            out.push(crate::weather::Change { at: *c, to: 0 });
            continue;
        }
        // the primary almost everywhere, the second as relief
        let h = {
            let mut v = (c.0 as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ (c.1 as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
                ^ (c.2 as i64 as u64) ^ seed;
            v ^= v >> 31; v.wrapping_mul(0x7FB5_D329_728E_A185) >> 33
        };
        let pick = if ids.len() > 1 && h % 100 < 18 { ids[1] } else { ids[0] };
        out.push(crate::weather::Change { at: *c, to: pick });
    }
    out
}

/// The manners, for a prompt.
pub fn vocabulary() -> String {
    known().iter().map(|m| format!("  {:<9} {}", m.name, m.reads_as))
        .collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_naming_two_things_takes_something_from_each() {
        // the crate built one thing from one source and had no way to
        // say "X in the manner of Y" at all
        for (said, subject, manner) in [
            ("a lighthouse made of bones", "a lighthouse", "bone"),
            ("a tower that branches like a tree", "a tower that branches", "tree"),
            ("a bridge in the manner of a spine", "a bridge", "bone"),
            ("a cathedral made of crystal", "a cathedral", "crystal"),
            ("a hut built like timber framing", "a hut", "timber"),
        ] {
            let (m, s) = read(said).unwrap_or_else(|| panic!("{said}"));
            assert_eq!(m.name, manner, "{said}");
            assert!(s.starts_with(subject.split_whitespace().next().unwrap()), "{said} -> {s}");
        }
        // a request naming one thing is not a graft
        assert!(read("a stone lighthouse").is_none());
        assert!(read("a castle with a chapel").is_none());
    }

    #[test]
    fn a_manner_is_how_it_is_made_not_what_it_is_made_of() {
        // "a lighthouse made of bones" is not a WHITE LIGHTHOUSE. It is
        // one whose parts socket, thin toward their ends, and are
        // hollow. Material is the last and smallest part of the answer.
        let bone = known().into_iter().find(|m| m.name == "bone").unwrap();
        let h = bone.habits();
        assert!(h.iter().any(|x| x.contains("narrowed neck")), "how they JOIN: {h:?}");
        assert!(h.iter().any(|x| x.contains("sparsely and smaller toward the ends")),
                "how they REPEAT: {h:?}");
        assert!(h.iter().any(|x| x.contains("thin toward their far end")), "how they TAPER");
        assert!(h.iter().any(|x| x.contains("hollow")));
        // the material is present but it is ONE of five habits
        let mats = h.iter().filter(|x| x.contains("built of")).count();
        assert_eq!(mats, 1, "material is one line of five: {h:?}");
        assert!(h.len() >= 4);
        // and the prompt says to keep the subject's own form
        let p = bone.prompt();
        assert!(p.contains("keeps its own form"), "{p}");
        assert!(p.contains("Take the WAY it is made, not its shape"));
    }

    #[test]
    fn no_two_manners_make_the_same_thing() {
        let all = known();
        assert!(all.len() >= 7);
        let mut seen = BTreeSet::new();
        for m in &all {
            // a manner is its habits, and two manners with identical
            // habits are one manner named twice
            let key = m.habits().join("|");
            assert!(seen.insert(key), "{} is the same as another manner", m.name);
            assert!(!m.reads_as.is_empty(), "{} says nothing about itself", m.name);
            assert!(m.prompt().contains(&m.name.to_uppercase()));
        }
        // and the vocabulary describes each rather than listing names
        let v = vocabulary();
        assert_eq!(v.lines().count(), all.len());
        assert!(v.contains("splitting again and again"));
    }

    #[test]
    fn the_surface_habits_reach_the_cells_and_the_rest_reaches_the_prompt() {
        // pretending a manner can be applied entirely after the fact
        // would make this a recolouring and call it a graft
        let mut cells = BTreeSet::new();
        for x in 0..8 { for y in 64..72 { for z in 0..8 { cells.insert((x, y, z)); } } }
        let bone = known().into_iter().find(|m| m.name == "bone").unwrap();
        let ch = apply_surface(&bone, &cells, 3);
        // hollow: the inside is emptied
        let emptied = ch.iter().filter(|c| c.to == 0).count();
        assert!(emptied > 0, "a hollow manner empties the inside");
        assert!(emptied < cells.len() / 2, "but the shell survives: {emptied} of {}", cells.len());
        // two materials, the second as relief rather than half
        let ids: BTreeSet<u16> = ch.iter().filter(|c| c.to != 0).map(|c| c.to).collect();
        assert_eq!(ids.len(), 2, "primary and relief");
        let counts: Vec<usize> = ids.iter()
            .map(|i| ch.iter().filter(|c| c.to == *i).count()).collect();
        let (mx, mn) = (counts.iter().max().unwrap(), counts.iter().min().unwrap());
        assert!(mx > &(mn * 3), "the second is relief, not half: {counts:?}");
        // a solid manner empties nothing
        let masonry = known().into_iter().find(|m| m.name == "masonry").unwrap();
        assert_eq!(apply_surface(&masonry, &cells, 3).iter().filter(|c| c.to == 0).count(), 0);
        // and it is deterministic
        assert_eq!(apply_surface(&bone, &cells, 3), apply_surface(&bone, &cells, 3));
    }

    #[test]
    fn a_graft_says_what_it_is_in_one_line() {
        let (m, s) = read("a lighthouse made of bones").unwrap();
        let g = Graft { subject: s, manner: m };
        let sum = g.summary();
        assert!(sum.contains("lighthouse") && sum.contains("manner of bone"), "{sum}");
        assert!(sum.contains("sockets, hollow shafts"), "and what that will look like: {sum}");
    }
}
