//! The parts of building that get cut from the video.
//!
//! A timelapse is EDITED. It shows forward progress at speed, and what
//! it leaves out is systematic: the reference-gathering, the undoing,
//! the standing still and looking, the flattening before a single
//! block goes down, the trying of five materials on a test patch, and
//! the lighting. None of it is interesting to watch and all of it is
//! the difference between a build that works and one that photographs.
//!
//! Checking this crate against that list found one glaring hole and
//! two real ones.
//!
//! LIGHTING. Six hundred mentions of light in the source and NOTHING
//! EVER PLACED ONE. Every interior ever built here -- the fourteen-room
//! castle, the windmill, the town -- is pitch black. In a game that is
//! not a cosmetic failure: it is a building nobody can use, and mobs
//! spawn in it. It is the single most-cut stage in any build video
//! because watching someone place torches is boring, and it is
//! mandatory.
//!
//! SITE PREPARATION. A builder flattens, clears and levels before
//! starting. `terraform` exists but nothing in the build flow calls
//! it, so a castle dropped on a slope has one corner buried and the
//! opposite corner in the air.
//!
//! THE PALETTE TRIAL. Nobody picks a material from a table. They put
//! three candidates side by side on a test patch, look, and choose.
//! `palette.rs` ranks analytically and returns one answer, which is a
//! decision made without ever looking at it.

use crate::weather::Change;
use std::collections::BTreeSet;

/// How dark a place is, and where to fix it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DarkReport {
    /// Standable cells further than `reach` from any light.
    pub dark: usize,
    pub total: usize,
    /// Where to put lights, chosen to cover the most darkness first.
    pub places: Vec<(i32, i32, i32)>,
}

impl DarkReport {
    pub fn summary(&self) -> String {
        if self.total == 0 { return "nothing enclosed to light".into(); }
        format!("{} of {} interior cell(s) dark ({:.0}%); {} light(s) would cover it",
                self.dark, self.total, self.dark as f32 / self.total as f32 * 100.0,
                self.places.len())
    }
}

/// Which cells are INSIDE: enclosed, with a floor, and out of the sky.
///
/// A light in the open is decoration; a light indoors is the
/// difference between a room and a hole. So the search is for cells
/// with something solid above them, which is what "indoors" means to
/// anyone standing in one.
pub fn interior(solid: &BTreeSet<(i32, i32, i32)>, headroom: i32) -> BTreeSet<(i32, i32, i32)> {
    if solid.is_empty() { return BTreeSet::new(); }
    let hi_y = solid.iter().map(|c| c.1).max().unwrap();
    let mut out = BTreeSet::new();
    // columns the structure covers
    let cols: BTreeSet<(i32, i32)> = solid.iter().map(|c| (c.0, c.2)).collect();
    for (x, z) in cols {
        let mut y = solid.iter().filter(|c| c.0 == x && c.2 == z).map(|c| c.1).min().unwrap();
        while y <= hi_y {
            // a floor here?
            if solid.contains(&(x, y, z)) {
                // is there air above it, with a roof over that?
                let space = (1..=headroom).all(|d| !solid.contains(&(x, y + d, z)));
                let roofed = ((headroom + 1)..=(hi_y - y).max(1))
                    .any(|d| solid.contains(&(x, y + d, z)));
                if space && roofed { out.insert((x, y + 1, z)); }
            }
            y += 1;
        }
    }
    out
}

/// Where the lights go.
///
/// Greedy: put one where it covers the most unlit interior, repeat.
/// Not optimal and does not need to be -- a room lit by four torches
/// in slightly wrong places is a lit room, and a room lit by none is
/// not a room.
pub fn plan_lights(solid: &BTreeSet<(i32, i32, i32)>, reach: i32, headroom: i32) -> DarkReport {
    let inside = interior(solid, headroom);
    let mut r = DarkReport { total: inside.len(), dark: inside.len(), ..Default::default() };
    if inside.is_empty() { return r; }
    let mut unlit: BTreeSet<(i32, i32, i32)> = inside.clone();
    let reach2 = reach * reach;
    // cap the number: a build that would need three hundred lights is
    // a build whose interior is a maze, and saying so beats placing
    // three hundred lights
    while !unlit.is_empty() && r.places.len() < 200 {
        // candidates are the unlit cells themselves: a torch stands
        // where a person could
        let best = unlit.iter().copied()
            .max_by_key(|c| unlit.iter().filter(|u| {
                let (dx, dy, dz) = (u.0 - c.0, u.1 - c.1, u.2 - c.2);
                dx * dx + dy * dy + dz * dz <= reach2
            }).count());
        let Some(at) = best else { break };
        unlit.retain(|u| {
            let (dx, dy, dz) = (u.0 - at.0, u.1 - at.1, u.2 - at.2);
            dx * dx + dy * dy + dz * dz > reach2
        });
        r.places.push(at);
    }
    r.dark = unlit.len();
    r
}

/// The lights, as changes ready to apply.
pub fn light_changes(r: &DarkReport, block: &str) -> Vec<Change> {
    let (id, _) = crate::palette::resolve_or_suggest(block);
    r.places.iter().map(|at| Change { at: *at, to: id }).collect()
}

// ── site preparation ─────────────────────────────────────────────────

/// What the ground does under a footprint, and what to do about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    pub lowest: i32,
    pub highest: i32,
    /// The level to build from: the ground's own median, so half is cut
    /// and half is filled rather than the whole thing perching or
    /// burying.
    pub level: i32,
    pub cut: usize,
    pub fill: usize,
    pub notes: Vec<String>,
}

impl Site {
    pub fn summary(&self) -> String {
        format!("ground {} to {} ({} of fall); level at {}, {} to cut and {} to fill",
                self.lowest, self.highest, self.highest - self.lowest, self.level,
                self.cut, self.fill)
    }
    /// Is it flat enough to build on untouched?
    pub fn flat_enough(&self) -> bool { self.highest - self.lowest <= 2 }
}

/// Survey the ground under a footprint. The stage before the first
/// block, which no build here has ever done: `terraform` exists and
/// nothing in the flow calls it, so a castle on a slope has one corner
/// buried and the opposite corner in the air.
pub fn survey<W: crate::VoxelWorld>(w: &W, min: (i32, i32), max: (i32, i32), from_y: i32) -> Site {
    let mut heights: Vec<i32> = Vec::new();
    let step = (((max.0 - min.0).max(max.1 - min.1)) / 24).max(1);
    for x in (min.0..=max.0).step_by(step as usize) {
        for z in (min.1..=max.1).step_by(step as usize) {
            if let Some(y) = (0..=from_y + 160).rev().find(|&y| w.get(x, y, z) != 0) {
                heights.push(y);
            }
        }
    }
    if heights.is_empty() {
        return Site { lowest: from_y, highest: from_y, level: from_y, cut: 0, fill: 0,
                      notes: vec!["no ground under this footprint at all".into()] };
    }
    heights.sort_unstable();
    let level = heights[heights.len() / 2];
    let (lowest, highest) = (heights[0], heights[heights.len() - 1]);
    let cut = heights.iter().filter(|h| **h > level).count();
    let fill = heights.iter().filter(|h| **h < level).count();
    let mut notes = Vec::new();
    if highest - lowest > 8 {
        notes.push(format!("a {}-block fall across the footprint: level it, terrace it, or \
                            put it somewhere else", highest - lowest));
    }
    Site { lowest, highest, level, cut, fill, notes }
}

/// The plan that levels a footprint. Cuts what stands above the level
/// and fills what falls below, so the building sits ON the ground
/// rather than in it or over it.
pub fn level_plan(s: &Site, min: (i32, i32), max: (i32, i32), fill_with: &str) -> String {
    if s.flat_enough() { return String::new(); }
    format!("# level the site: {}\n\
             pass cut the site cut\n  \
               part above box w={} h={} d={} material=stone\n  \
               rel above at ground dx={} dy={} dz={}\n\
             end\n\
             pass fill the site add\n  \
               part below box w={} h={} d={} material={fill_with}\n  \
               rel below at ground dx={} dy={} dz={}\n\
             end\n",
            s.summary(),
            max.0 - min.0 + 1, (s.highest - s.level).max(1) + 1, max.1 - min.1 + 1,
            (min.0 + max.0) / 2, s.level - 64 + (s.highest - s.level).max(1) / 2 + 1, (min.1 + max.1) / 2,
            max.0 - min.0 + 1, (s.level - s.lowest).max(1), max.1 - min.1 + 1,
            (min.0 + max.0) / 2, s.lowest - 64 + (s.level - s.lowest).max(1) / 2, (min.1 + max.1) / 2)
}

// ── the palette trial ────────────────────────────────────────────────

/// Three candidates side by side, to be looked at rather than ranked.
///
/// Nobody picks a material from a table. They put three on a test
/// patch, look, and choose -- and `palette.rs` returning one analytic
/// answer is a decision made without ever seeing it.
pub fn trial_patch(candidates: &[&str], at: (i32, i32, i32), size: i32) -> (String, Vec<String>) {
    let n = candidates.len().clamp(1, 5) as i32;
    let s = size.clamp(3, 24);
    let mut src = String::from("# palette trial\npass trial add\n");
    let mut labels = Vec::new();
    for (i, c) in candidates.iter().take(5).enumerate() {
        let (id, why) = crate::palette::resolve_or_suggest(c);
        let name = crate::palette::by_id(id).map(|b| b.name).unwrap_or("stone");
        labels.push(match why {
            Some(w) => format!("{c} -> {name} ({w})"),
            None => format!("{name}"),
        });
        let dx = at.0 + (i as i32 - n / 2) * (s + 2);
        src.push_str(&format!("  part p{i} box w={s} h={} d={s} material={name}\n", s * 2));
        src.push_str(&format!("  rel p{i} at ground dx={dx} dy={} dz={}\n", at.1 - 64 + s, at.2));
    }
    src.push_str("end\n");
    (src, labels)
}

/// What to say about a trial, so the choice is made by looking.
pub fn trial_prompt(what: &str, labels: &[String]) -> String {
    format!("\
Three test blocks of material, side by side, for: {what}

  {}

Look at them together rather than one at a time -- a material is only light or dark, warm
or cold, busy or plain NEXT TO ANOTHER ONE. Then answer:

  use NAME            for the main mass
  trim NAME           for edges and detail, if a second reads well against the first
  none                if none of them suits, and say what would

A palette chosen from a description is a guess. This is the same choice made by looking.",
        labels.iter().enumerate().map(|(i, l)| format!("{}. {l}", i + 1))
            .collect::<Vec<_>>().join("\n  "))
}

/// Read a trial's verdict.
pub fn parse_trial(reply: &str) -> (Option<String>, Option<String>, Vec<String>) {
    let (mut main, mut trim, mut notes) = (None, None, Vec::new());
    for line in reply.lines() {
        let t = line.trim();
        let w: Vec<&str> = t.split_whitespace().collect();
        match w.first().map(|x| x.to_lowercase()).as_deref() {
            Some("use") | Some("main") if w.len() > 1 => main = Some(w[1].to_lowercase()),
            Some("trim") | Some("accent") if w.len() > 1 => trim = Some(w[1].to_lowercase()),
            Some("none") => notes.push(w[1..].join(" ")),
            _ => {}
        }
    }
    (main, trim, notes)
}

/// Everything the video cuts, as a checklist a build can be held to.
pub fn omissions(has_lights: bool, site_prepared: bool, palette_tried: bool) -> Vec<&'static str> {
    let mut v = Vec::new();
    if !has_lights { v.push("nothing lights the inside: it is a black box, and mobs spawn in it"); }
    if !site_prepared { v.push("the ground was never levelled: it will bury one side and float the other"); }
    if !palette_tried { v.push("the materials were chosen from a description, not by looking"); }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hollow room: floor, walls, roof.
    fn room(w: i32, h: i32) -> BTreeSet<(i32, i32, i32)> {
        let mut s = BTreeSet::new();
        for x in 0..w { for z in 0..w { for y in 64..64 + h {
            if x == 0 || z == 0 || x == w - 1 || z == w - 1 || y == 64 || y == 64 + h - 1 {
                s.insert((x, y, z));
            }
        }}}
        s
    }

    #[test]
    fn an_interior_is_found_and_it_is_dark() {
        // Six hundred mentions of light in this crate and NOTHING EVER
        // PLACED ONE. Every interior ever built here is pitch black,
        // which in a game is not cosmetic: it is a building nobody can
        // use and mobs spawn in it.
        let r = plan_lights(&room(14, 8), 6, 2);
        assert!(r.total > 50, "it found the inside: {}", r.summary());
        assert!(!r.places.is_empty(), "and somewhere to put a light");
        assert_eq!(r.dark, 0, "which covers it: {}", r.summary());
        assert!(r.summary().contains("light(s) would cover it"));
        // the lights are INSIDE, not in the walls
        let solid = room(14, 8);
        for p in &r.places {
            assert!(!solid.contains(p), "{p:?} is inside a wall");
            assert!(p.0 > 0 && p.0 < 13 && p.2 > 0 && p.2 < 13, "{p:?} is outside the room");
        }
        // a solid lump has no interior to light, and says so
        let mut lump = BTreeSet::new();
        for x in 0..6 { for y in 64..70 { for z in 0..6 { lump.insert((x, y, z)); } } }
        assert_eq!(plan_lights(&lump, 6, 2).total, 0);
        assert!(plan_lights(&lump, 6, 2).summary().contains("nothing enclosed"));
    }

    #[test]
    fn fewer_lights_cover_a_bigger_room_when_they_reach_further() {
        let big = room(20, 6);
        let near = plan_lights(&big, 4, 2).places.len();
        let far = plan_lights(&big, 10, 2).places.len();
        assert!(far < near, "a longer reach needs fewer: {far} vs {near}");
        assert!(near > 1, "and a big room needs more than one");
        // and they become real blocks
        let ch = light_changes(&plan_lights(&big, 8, 2), "glowstone");
        assert!(!ch.is_empty());
        assert!(ch.iter().all(|c| c.to != 0), "a light is not air");
    }

    #[test]
    fn the_ground_is_surveyed_before_the_first_block() {
        // `terraform` exists and nothing in the flow called it, so a
        // castle on a slope had one corner buried and the opposite
        // corner in the air
        struct Slope;
        impl crate::VoxelWorld for Slope {
            fn get(&self, x: i32, y: i32, _z: i32) -> u16 {
                if y <= 64 + x / 3 { 1 } else { 0 }
            }
            fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { false }
        }
        let s = survey(&Slope, (0, 0), (30, 30), 64);
        assert!(s.highest - s.lowest >= 9, "it found the fall: {}", s.summary());
        assert!(!s.flat_enough());
        assert!(s.level > s.lowest && s.level < s.highest, "the level is the median, so half is \
                 cut and half filled rather than the whole thing perching: {}", s.summary());
        assert!(s.cut > 0 && s.fill > 0);
        assert!(s.notes.iter().any(|n| n.contains("fall across the footprint")), "{:?}", s.notes);
        // and the plan it makes runs
        let src = level_plan(&s, (0, 0), (30, 30), "dirt");
        assert!(src.contains("cut the site cut") && src.contains("fill the site add"), "{src}");
        let (prog, errs) = crate::program::parse(&src);
        assert!(errs.is_empty(), "{src}\n{errs:?}");
        assert_eq!(prog.passes.len(), 2);
        let r = crate::program::run(&prog, (0, 64, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(!r.changes.is_empty());
        // flat ground needs no plan at all
        struct Flat;
        impl crate::VoxelWorld for Flat {
            fn get(&self, _: i32, y: i32, _: i32) -> u16 { if y <= 64 { 1 } else { 0 } }
            fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { false }
        }
        let f = survey(&Flat, (0, 0), (20, 20), 64);
        assert!(f.flat_enough());
        assert!(level_plan(&f, (0, 0), (20, 20), "dirt").is_empty(), "no work where none is needed");
    }

    #[test]
    fn a_palette_is_chosen_by_looking_at_candidates_together() {
        // `palette.rs` returns one analytic answer, which is a decision
        // made without ever seeing it
        let (src, labels) = trial_patch(&["stone", "sandstone", "dark_oak_planks"], (0, 64, 0), 6);
        assert_eq!(labels.len(), 3);
        let (prog, errs) = crate::program::parse(&src);
        assert!(errs.is_empty(), "{src}\n{errs:?}");
        let r = crate::program::run(&prog, (0, 64, 0));
        // three separate patches, side by side
        let xs: BTreeSet<i32> = r.changes.iter().map(|c| c.at.0 / 8).collect();
        assert!(xs.len() >= 3, "side by side: {} groups", xs.len());
        // three different materials
        let mats: BTreeSet<u16> = r.changes.iter().map(|c| c.to).collect();
        assert_eq!(mats.len(), 3, "one per candidate");
        // a candidate this world lacks is substituted AND the label says so
        let (_, subbed) = trial_patch(&["deepslate"], (0, 64, 0), 5);
        assert!(subbed[0].contains("deepslate") && subbed[0].contains("used"),
                "the label names what was asked for AND what stood in: {:?}", subbed[0]);
        // the prompt says why they are side by side
        let p = trial_prompt("a keep", &labels);
        assert!(p.contains("NEXT TO ANOTHER ONE"), "{p}");
        assert!(p.contains("chosen from a description is a guess"));
        let (m, t, _) = parse_trial("use sandstone\ntrim quartz\n");
        assert_eq!(m.as_deref(), Some("sandstone"));
        assert_eq!(t.as_deref(), Some("quartz"));
    }

    #[test]
    fn the_checklist_names_what_the_video_leaves_out() {
        let all = omissions(false, false, false);
        assert_eq!(all.len(), 3);
        assert!(all[0].contains("black box") && all[0].contains("mobs"));
        assert!(all[1].contains("bury one side"));
        assert!(all[2].contains("not by looking"));
        assert!(omissions(true, true, true).is_empty(), "a build that did all three is clean");
    }
}
