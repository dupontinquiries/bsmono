//! Templates: turning a build into vocabulary.
//!
//! The dictionary is fixed at compile time. `house`, `tower`,
//! `colonnade` — whatever a player invents beyond that is unnameable,
//! and building the same guard post five times along a wall means
//! describing it five times and getting five different guard posts.
//!
//! A template is a captured region, stored relative to its own origin,
//! that can be stamped again: rotated, mirrored, re-materialized into a
//! different palette, and jittered so copies are not clones. It makes
//! the system learn vocabulary FROM THE PLAYER, which is a larger
//! creative multiplier than any number of built-in forms — and unlike
//! built-ins it costs nothing to add one.
//!
//! WHY IT IS NOT JUST A COPY BUFFER
//!
//! Three things a clipboard does not do:
//!
//! * **Rotation.** Four guard posts around a courtyard face four ways.
//!   Without rotation the player builds four templates, and they drift.
//! * **Re-materialization.** The same gatehouse in stone on the ridge
//!   and in sandstone at the oasis. Cells are stored by PALETTE ROLE
//!   where they match one, so a stamp adopts the local palette.
//! * **Variation.** Copies that are identical read as copies. A seeded
//!   jitter drops or swaps a small fraction, so five posts are five
//!   posts rather than one post five times.

use crate::weather::Change;
use crate::VoxelWorld;
use std::collections::BTreeMap;

/// A cell of a template. Stored by ROLE when the source block matched
/// the palette it was captured in, so the same template can come back
/// in sandstone at the oasis and stone on the ridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// A literal block. Used when the source did not match a role.
    Block(u16),
    /// primary / secondary / accent / foliage.
    Role(u8),
}

/// A captured build, stored relative to its own minimum corner.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub name: String,
    /// Size in blocks.
    pub size: (i32, i32, i32),
    /// Offset from the origin to cell.
    pub cells: BTreeMap<(i32, i32, i32), Cell>,
    /// The palette it was captured in, for the record.
    pub palette: String,
}

impl Template {
    pub fn block_count(&self) -> usize { self.cells.len() }

    pub fn summary(&self) -> String {
        let roles = self.cells.values().filter(|c| matches!(c, Cell::Role(_))).count();
        format!("{} -- {}x{}x{}, {} blocks ({} palette-aware, captured in {})",
                self.name, self.size.0, self.size.1, self.size.2,
                self.cells.len(), roles, self.palette)
    }
}

/// Capture a region as a template.
///
/// Air is not stored: a template is a thing, not a box containing one,
/// so stamping it never erases what is already around it. That also
/// means a template cannot carve — which is the right trade, since a
/// stamp that removes blocks is a stamp that can destroy a neighbour's
/// build by accident.
pub fn capture<W: VoxelWorld>(w: &W, name: &str, min: (i32, i32, i32), max: (i32, i32, i32),
                              palette: &str) -> Template {
    let (lo, hi) = (
        (min.0.min(max.0), min.1.min(max.1), min.2.min(max.2)),
        (min.0.max(max.0), min.1.max(max.1), min.2.max(max.2)));
    let roles = crate::palettes();
    let pal = roles.get(palette);
    let mut cells = BTreeMap::new();
    for x in lo.0..=hi.0 {
        for y in lo.1..=hi.1 {
            for z in lo.2..=hi.2 {
                let b = w.get(x, y, z);
                if b == 0 { continue; }
                let cell = match pal {
                    Some(p) if b == p.primary => Cell::Role(0),
                    Some(p) if b == p.secondary => Cell::Role(1),
                    Some(p) if b == p.accent => Cell::Role(2),
                    Some(p) if b == p.foliage => Cell::Role(3),
                    _ => Cell::Block(b),
                };
                cells.insert((x - lo.0, y - lo.1, z - lo.2), cell);
            }
        }
    }
    Template {
        name: name.to_string(),
        size: (hi.0 - lo.0 + 1, hi.1 - lo.1 + 1, hi.2 - lo.2 + 1),
        cells,
        palette: palette.to_string(),
    }
}

/// How a stamp is oriented and varied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stamp {
    /// Quarter turns about Y, 0..3.
    pub turns: u8,
    /// Mirror across the template's own x axis after rotating.
    pub mirror: bool,
    /// Fraction of cells dropped, so copies are not clones. 0.0 = exact.
    pub vary: f32,
    pub seed: u64,
}

impl Default for Stamp {
    fn default() -> Self { Self { turns: 0, mirror: false, vary: 0.0, seed: 1 } }
}

/// Rotate an offset by quarter turns about Y, within a footprint of
/// `size`. Kept separate and tested on its own because an off-by-one
/// here shears a build by a block per copy and is invisible until four
/// of them are side by side.
pub fn rotate(off: (i32, i32, i32), size: (i32, i32, i32), turns: u8)
    -> ((i32, i32, i32), (i32, i32, i32)) {
    let (x, y, z) = off;
    let (sx, _sy, sz) = size;
    match turns % 4 {
        0 => ((x, y, z), size),
        // 90 degrees: x <- z, z <- (sx-1) - x, and the footprint swaps.
        1 => ((sz - 1 - z, y, x), (sz, size.1, sx)),
        2 => ((sx - 1 - x, y, sz - 1 - z), size),
        _ => ((z, y, sx - 1 - x), (sz, size.1, sx)),
    }
}

fn hash(x: i32, y: i32, z: i32, seed: u64) -> u64 {
    let mut h = seed
        ^ (x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (z as i64 as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 31;
    h = h.wrapping_mul(0x7FB5_D329_728E_A185);
    h ^ (h >> 27)
}

/// Place a template at a world position.
///
/// `palette` re-materializes role cells; pass the template's own to get
/// it back unchanged. Literal cells are never remapped, so a stained
/// window stays a stained window in any palette.
pub fn place(t: &Template, at: (i32, i32, i32), s: &Stamp, palette: &str) -> Vec<Change> {
    let pals = crate::palettes();
    let pal = pals.get(palette).or_else(|| pals.get(t.palette.as_str()));
    let mut out = Vec::new();
    for (&off, &cell) in &t.cells {
        let (r, rsize) = rotate(off, t.size, s.turns);
        let (rx, ry, rz) = if s.mirror { (rsize.0 - 1 - r.0, r.1, r.2) } else { r };
        // Variation drops cells rather than adding them: a dropped block
        // reads as wear, an added one reads as a mistake.
        if s.vary > 0.0 {
            let roll = (hash(off.0, off.1, off.2, s.seed) % 10_000) as f32 / 10_000.0;
            if roll < s.vary { continue; }
        }
        let block = match cell {
            Cell::Block(b) => b,
            Cell::Role(r) => match pal {
                Some(p) => match r { 0 => p.primary, 1 => p.secondary,
                                     2 => p.accent, _ => p.foliage },
                None => 1,
            },
        };
        out.push(Change { at: (at.0 + rx, at.1 + ry, at.2 + rz), to: block });
    }
    out
}

/// Named templates, saved with the world beside the worksets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Library {
    items: BTreeMap<String, Template>,
}

impl Library {
    pub fn new() -> Self { Self::default() }
    pub fn len(&self) -> usize { self.items.len() }
    pub fn is_empty(&self) -> bool { self.items.is_empty() }
    pub fn get(&self, name: &str) -> Option<&Template> { self.items.get(&norm(name)) }
    pub fn names(&self) -> Vec<&str> { self.items.values().map(|t| t.name.as_str()).collect() }

    pub fn add(&mut self, t: Template) -> Result<(), String> {
        if t.cells.is_empty() {
            return Err(format!("'{}' captured no blocks -- nothing there to remember", t.name));
        }
        // A template that shadows a built-in form would make `form
        // house` ambiguous, and the player would never learn which one
        // they got.
        if crate::dict::lookup(&t.name).is_some() {
            return Err(format!("'{}' is already a built-in form; pick another name", t.name));
        }
        self.items.insert(norm(&t.name), t);
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> bool { self.items.remove(&norm(name)).is_some() }

    /// The line listing what the player has taught it, for the prompt.
    pub fn vocabulary(&self) -> String {
        if self.items.is_empty() { return String::new(); }
        format!("LEARNED FORMS (use `place NAME @x,z`): {}",
                self.items.values()
                    .map(|t| format!("{} ({}x{})", t.name, t.size.0, t.size.2))
                    .collect::<Vec<_>>().join(", "))
    }

    pub fn save(&self) -> String {
        let mut s = String::from("# unendlich templates v1\n");
        for t in self.items.values() {
            s.push_str(&format!("tpl {}\n", t.name.replace('\n', " ")));
            s.push_str(&format!("size {} {} {}\n", t.size.0, t.size.1, t.size.2));
            s.push_str(&format!("palette {}\n", t.palette));
            for (&(x, y, z), &c) in &t.cells {
                match c {
                    Cell::Block(b) => s.push_str(&format!("b {x} {y} {z} {b}\n")),
                    Cell::Role(r) => s.push_str(&format!("r {x} {y} {z} {r}\n")),
                }
            }
        }
        s
    }

    pub fn load(text: &str) -> Self {
        let mut lib = Library::new();
        let mut cur: Option<Template> = None;
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() { continue; }
            let (k, rest) = line.split_once(' ').unwrap_or((line, ""));
            let n: Vec<i32> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
            match k {
                "tpl" => {
                    if let Some(t) = cur.take() { lib.items.insert(norm(&t.name), t); }
                    cur = Some(Template { name: rest.to_string(), size: (0, 0, 0),
                                          cells: BTreeMap::new(), palette: "stone".into() });
                }
                "size" => if let (Some(t), true) = (cur.as_mut(), n.len() == 3) {
                    t.size = (n[0], n[1], n[2]);
                },
                "palette" => if let Some(t) = cur.as_mut() { t.palette = rest.to_string(); },
                "b" => if let (Some(t), true) = (cur.as_mut(), n.len() == 4) {
                    t.cells.insert((n[0], n[1], n[2]), Cell::Block(n[3] as u16));
                },
                "r" => if let (Some(t), true) = (cur.as_mut(), n.len() == 4) {
                    t.cells.insert((n[0], n[1], n[2]), Cell::Role(n[3] as u8));
                },
                _ => {}
            }
        }
        if let Some(t) = cur.take() { lib.items.insert(norm(&t.name), t); }
        lib
    }
}

fn norm(s: &str) -> String { s.trim().to_lowercase() }

#[cfg(test)]
mod tests {
    use super::*;

    struct W(BTreeMap<(i32, i32, i32), u16>);
    impl VoxelWorld for W {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.0.get(&(x, y, z)) { return b; }
            if y <= 64 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
            self.0.insert((x, y, z), b); true
        }
    }

    /// An L, so rotation is detectable: a symmetric shape would pass a
    /// broken rotation.
    fn ell() -> W {
        let mut w = W(BTreeMap::new());
        for x in 0..4 { w.set(x, 65, 0, 1); }
        for z in 1..3 { w.set(0, 65, z, 1); }
        w
    }

    #[test]
    fn capture_stores_a_thing_not_a_box() {
        // Air is not stored, so a stamp never erases what is around it.
        let w = ell();
        let t = capture(&w, "post", (0, 65, 0), (3, 65, 2), "stone");
        assert_eq!(t.block_count(), 6, "only the solid cells");
        assert_eq!(t.size, (4, 1, 3));
        assert!(!t.cells.contains_key(&(3, 0, 2)), "the empty corner is not stored");
    }

    #[test]
    fn four_quarter_turns_return_the_original() {
        // An off-by-one here shears a build by a block per copy and is
        // invisible until four of them stand side by side.
        let size = (4, 1, 3);
        for off in [(0, 0, 0), (3, 0, 0), (0, 0, 2), (2, 0, 1)] {
            let mut cur = off;
            let mut sz = size;
            for _ in 0..4 {
                let (n, ns) = rotate(cur, sz, 1);
                cur = n; sz = ns;
            }
            assert_eq!(cur, off, "four turns must be identity for {off:?}");
            assert_eq!(sz, size, "and the footprint must come back too");
        }
    }

    #[test]
    fn rotation_stays_inside_the_footprint() {
        let size = (5, 1, 3);
        for turns in 0..4u8 {
            for x in 0..5 { for z in 0..3 {
                let (r, rs) = rotate((x, 0, z), size, turns);
                assert!(r.0 >= 0 && r.0 < rs.0 && r.2 >= 0 && r.2 < rs.2,
                        "turns={turns} ({x},{z}) -> {r:?} escaped {rs:?}");
            }}
        }
    }

    #[test]
    fn a_rotated_stamp_is_actually_rotated() {
        let w = ell();
        let t = capture(&w, "post", (0, 65, 0), (3, 65, 2), "stone");
        let flat = |s: &Stamp| {
            let mut v: Vec<(i32, i32, i32)> =
                place(&t, (0, 0, 0), s, "stone").iter().map(|c| c.at).collect();
            v.sort();
            v
        };
        let a = flat(&Stamp::default());
        let b = flat(&Stamp { turns: 1, ..Default::default() });
        assert_ne!(a, b, "a quarter turn changes the shape's orientation");
        assert_eq!(a.len(), b.len(), "but not how much of it there is");
        // the long arm ran along x; after a turn it runs along z
        let a_xspan = a.iter().map(|p| p.0).max().unwrap() - a.iter().map(|p| p.0).min().unwrap();
        let b_zspan = b.iter().map(|p| p.2).max().unwrap() - b.iter().map(|p| p.2).min().unwrap();
        assert_eq!(a_xspan, b_zspan, "the long arm swapped axes");
    }

    #[test]
    fn a_template_re_materializes_into_a_local_palette() {
        // The same gatehouse in stone on the ridge and sandstone at the
        // oasis, without capturing it twice.
        let pals = crate::palettes();
        let stone = pals.get("stone").unwrap();
        let mut w = W(BTreeMap::new());
        for x in 0..3 { w.set(x, 65, 0, stone.primary); }
        w.set(1, 66, 0, stone.accent);
        let t = capture(&w, "gate", (0, 65, 0), (2, 66, 0), "stone");
        assert!(t.cells.values().all(|c| matches!(c, Cell::Role(_))),
                "everything matched a role, so everything is portable");
        let desert = pals.get("desert").unwrap();
        let out = place(&t, (0, 0, 0), &Stamp::default(), "desert");
        assert!(out.iter().any(|c| c.to == desert.primary), "primary followed the palette");
        assert!(out.iter().any(|c| c.to == desert.accent), "and so did the accent");
        assert!(!out.iter().any(|c| c.to == stone.primary), "nothing kept the old material");
    }

    #[test]
    fn a_literal_block_survives_re_materialization() {
        // A stained window is a stained window in any palette.
        let mut w = W(BTreeMap::new());
        w.set(0, 65, 0, 61);        // glass: not in the stone palette
        let t = capture(&w, "win", (0, 65, 0), (0, 65, 0), "stone");
        assert_eq!(t.cells.values().next(), Some(&Cell::Block(61)));
        let out = place(&t, (0, 0, 0), &Stamp::default(), "desert");
        assert_eq!(out[0].to, 61, "a literal is never remapped");
    }

    #[test]
    fn variation_makes_copies_that_are_not_clones() {
        let w = ell();
        let t = capture(&w, "post", (0, 65, 0), (3, 65, 2), "stone");
        let exact = place(&t, (0, 0, 0), &Stamp::default(), "stone");
        let varied = place(&t, (0, 0, 0), &Stamp { vary: 0.3, seed: 5, ..Default::default() },
                           "stone");
        assert!(varied.len() < exact.len(), "variation drops cells");
        assert!(!varied.is_empty(), "but not all of them");
        // and only ever drops: an added block reads as a mistake
        let exact_set: std::collections::BTreeSet<_> = exact.iter().map(|c| c.at).collect();
        assert!(varied.iter().all(|c| exact_set.contains(&c.at)),
                "variation never invents a block that was not in the template");
    }

    #[test]
    fn variation_is_deterministic_per_seed() {
        let w = ell();
        let t = capture(&w, "post", (0, 65, 0), (3, 65, 2), "stone");
        let s = Stamp { vary: 0.4, seed: 9, ..Default::default() };
        assert_eq!(place(&t, (0, 0, 0), &s, "stone"), place(&t, (0, 0, 0), &s, "stone"));
        let other = Stamp { seed: 10, ..s };
        assert_ne!(place(&t, (0, 0, 0), &s, "stone"), place(&t, (0, 0, 0), &other, "stone"));
    }

    #[test]
    fn a_name_that_shadows_a_builtin_is_refused() {
        // `form house` must not be ambiguous; the player would never
        // learn which one they got.
        let w = ell();
        let mut lib = Library::new();
        let t = capture(&w, "house", (0, 65, 0), (3, 65, 2), "stone");
        assert!(lib.add(t).is_err());
        let ok = capture(&w, "guardpost", (0, 65, 0), (3, 65, 2), "stone");
        assert!(lib.add(ok).is_ok());
        assert!(lib.get("GuardPost").is_some(), "lookup is case-insensitive");
    }

    #[test]
    fn capturing_nothing_is_an_error_not_an_empty_template() {
        let w = W(BTreeMap::new());
        let mut lib = Library::new();
        let t = capture(&w, "air", (0, 100, 0), (4, 104, 4), "stone");
        assert_eq!(t.block_count(), 0);
        let e = lib.add(t).unwrap_err();
        assert!(e.contains("nothing there"), "{e}");
    }

    #[test]
    fn a_library_round_trips_through_a_save() {
        let w = ell();
        let mut lib = Library::new();
        lib.add(capture(&w, "guardpost", (0, 65, 0), (3, 65, 2), "stone")).unwrap();
        let mut mixed = W(BTreeMap::new());
        mixed.set(0, 65, 0, 61);
        mixed.set(1, 65, 0, crate::palettes().get("stone").unwrap().primary);
        lib.add(capture(&mixed, "window", (0, 65, 0), (1, 65, 0), "stone")).unwrap();
        assert_eq!(Library::load(&lib.save()), lib);
    }

    #[test]
    fn the_learned_vocabulary_reaches_the_prompt() {
        let w = ell();
        let mut lib = Library::new();
        assert!(lib.vocabulary().is_empty(), "nothing learned yet, nothing said");
        lib.add(capture(&w, "guardpost", (0, 65, 0), (3, 65, 2), "stone")).unwrap();
        let v = lib.vocabulary();
        assert!(v.contains("guardpost") && v.contains("place NAME"),
                "the model is told both the name and how to use it: {v}");
    }
}
