//! Critique: check a build after the fact for the mistakes models make
//! that no arithmetic check can see.
//!
//! `check_math` catches numbers that are wrong. This catches shapes
//! that are wrong in the world: a wall hanging in the air because the
//! ground fell away under one corner, a tower placed through the
//! player, a hollow room with no way in. Each check is cheap, runs on
//! the dry-run result before commit, and reports a suggestion rather
//! than a verdict -- the player decides.

use crate::VoxelWorld;

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub kind: &'static str,
    pub message: String,
    pub at: (i32, i32, i32),
}

/// Connected groups of placed blocks with no support from the
/// pre-existing world.
///
/// The naive rule -- "a block is supported if anything solid is below
/// or beside it" -- is wrong, and the first version of this function
/// had it: a 3x3 slab in mid-air supported itself, because every block
/// had placed neighbours. Support has to CHAIN to something that was
/// already there. So: flood-fill the placed set into 6-connected
/// components, and a component is grounded only if some member touches
/// a solid cell that is not part of the build (below or beside).
///
/// Reports one finding per floating component with its size and
/// lowest block, which is what a player needs to find it.
pub fn floating<W: VoxelWorld>(world: &W, placed: &[(i32, i32, i32)]) -> Vec<Finding> {
    use std::collections::HashSet;
    let set: HashSet<(i32, i32, i32)> = placed.iter().copied().collect();
    let mut seen: HashSet<(i32, i32, i32)> = HashSet::new();
    let mut out = Vec::new();
    let nb = |(x, y, z): (i32, i32, i32)| [(x + 1, y, z), (x - 1, y, z), (x, y + 1, z),
                                            (x, y - 1, z), (x, y, z + 1), (x, y, z - 1)];
    for &start in placed {
        if seen.contains(&start) { continue; }
        let mut stack = vec![start];
        let mut comp: Vec<(i32, i32, i32)> = Vec::new();
        let mut grounded = false;
        seen.insert(start);
        while let Some(p) = stack.pop() {
            comp.push(p);
            for n in nb(p) {
                if set.contains(&n) {
                    if seen.insert(n) { stack.push(n); }
                } else if n.1 <= p.1 && world.get(n.0, n.1, n.2) != 0 {
                    // touches pre-existing solid below or beside (not
                    // above: hanging from a ceiling is not support)
                    grounded = true;
                }
            }
        }
        if !grounded {
            let low = comp.iter().min_by_key(|p| p.1).copied().unwrap_or(start);
            out.push(Finding { kind: "floating",
                message: format!("{} blocks hang in the air with nothing under them; lowest at ({},{},{})",
                                 comp.len(), low.0, low.1, low.2),
                at: low });
        }
    }
    out
}

/// Would the build entomb the player? `player_feet` is the block the
/// player stands in; a build that writes a solid block into that cell
/// or the one above it needs to be caught before commit, not after.
pub fn entombs_player(placed: &[(i32, i32, i32)], player_feet: (i32, i32, i32)) -> Option<Finding> {
    let head = (player_feet.0, player_feet.1 + 1, player_feet.2);
    placed.iter().find(|&&p| p == player_feet || p == head).map(|&p| Finding {
        kind: "entombs_player",
        message: "the build places a block where the player is standing; step back or move it".into(),
        at: p,
    })
}

/// For a hollow structure, is there any opening at ground level? A
/// room with no door is the second most common model output after a
/// floating one. Checks the ring of cells one outside the footprint at
/// surface height for at least one air gap in the wall.
pub fn has_ground_entrance<W: VoxelWorld>(world: &W, min: (i32, i32, i32), max: (i32, i32, i32),
                                          ground_y: i32) -> bool {
    let ys = [ground_y + 1, ground_y + 2];
    for y in ys {
        for x in min.0..=max.0 {
            for &z in &[min.2, max.2] {
                if world.get(x, y, z) == 0 { return true; }
            }
        }
        for z in min.2..=max.2 {
            for &x in &[min.0, max.0] {
                if world.get(x, y, z) == 0 { return true; }
            }
        }
    }
    false
}

/// Summarise findings for the chat log.
pub fn summarise(findings: &[Finding]) -> String {
    if findings.is_empty() { return "no problems found".into(); }
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for f in findings { *counts.entry(f.kind).or_default() += 1; }
    let parts: Vec<String> = counts.iter().map(|(k, n)| format!("{n} {k}")).collect();
    let first = &findings[0];
    format!("{} -- e.g. {}", parts.join(", "), first.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Flat { cells: BTreeMap<(i32, i32, i32), u16> }
    impl Flat { fn new() -> Self { Self { cells: BTreeMap::new() } } }
    impl VoxelWorld for Flat {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
            if y <= 64 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.cells.insert((x, y, z), b); true }
    }

    #[test]
    fn a_wall_on_the_ground_is_not_floating() {
        let w = Flat::new();
        let placed: Vec<_> = (65..70).map(|y| (0, y, 0)).collect();
        assert!(floating(&w, &placed).is_empty());
    }

    #[test]
    fn a_block_in_midair_is_floating() {
        let w = Flat::new();
        let f = floating(&w, &[(0, 80, 0)]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, "floating");
    }

    #[test]
    fn side_support_counts_when_it_chains_to_ground() {
        // a cantilever attached to a grounded column
        let w = Flat::new();
        let placed = vec![(0, 65, 0), (0, 66, 0), (1, 66, 0)];
        assert!(floating(&w, &placed).is_empty(), "the overhang chains to the ground");
    }

    #[test]
    fn a_slab_in_midair_does_not_support_itself() {
        // the bug the first version had: every block had placed
        // neighbours, so none was flagged
        let w = Flat::new();
        let placed: Vec<_> = (-1..=1).flat_map(|x| (-1..=1).map(move |z| (x, 85, z))).collect();
        let f = floating(&w, &placed);
        assert_eq!(f.len(), 1, "one floating component, not zero and not nine");
        assert!(f[0].message.contains("9 blocks"));
    }

    #[test]
    fn hanging_from_a_ceiling_is_not_support() {
        let mut w = Flat::new();
        w.set(0, 90, 0, 1);                       // a pre-existing ceiling block
        let f = floating(&w, &[(0, 89, 0)]);      // placed directly under it
        assert_eq!(f.len(), 1, "touching solid ABOVE does not ground a block");
    }

    #[test]
    fn entombment_is_caught_at_feet_or_head() {
        let feet = (5, 65, 5);
        assert!(entombs_player(&[(5, 66, 5)], feet).is_some(), "head");
        assert!(entombs_player(&[(5, 65, 5)], feet).is_some(), "feet");
        assert!(entombs_player(&[(6, 65, 5)], feet).is_none(), "beside is fine");
    }

    #[test]
    fn a_sealed_room_has_no_entrance_and_a_doorway_fixes_it() {
        let mut w = Flat::new();
        // 5x5 hollow box of walls, y 65..69
        for y in 65..=69 { for x in 0..=4 { for z in 0..=4 {
            if x == 0 || x == 4 || z == 0 || z == 4 { w.set(x, y, z, 58); }
        }}}
        assert!(!has_ground_entrance(&w, (0, 65, 0), (4, 69, 4), 64), "sealed");
        w.set(2, 65, 0, 0); w.set(2, 66, 0, 0);          // carve a door
        assert!(has_ground_entrance(&w, (0, 65, 0), (4, 69, 4), 64), "door found");
    }

    #[test]
    fn summary_is_one_line_with_a_concrete_example() {
        let f = vec![
            Finding { kind: "floating", message: "block at (1,2,3) has nothing below".into(), at: (1, 2, 3) },
            Finding { kind: "floating", message: "block at (4,5,6) has nothing below".into(), at: (4, 5, 6) },
        ];
        let s = summarise(&f);
        assert!(s.starts_with("2 floating"));
        assert!(s.contains("(1,2,3)"));
        assert!(!s.contains('\n'));
    }
}
