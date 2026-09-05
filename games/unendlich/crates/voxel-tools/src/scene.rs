//! Scene: showing the model what it is standing in.
//!
//! The model cannot see. Everything it knows about the site arrives as
//! text, and until now that text was a density digest ("42% solid") and
//! a column probe — enough to avoid building inside a hill, nowhere
//! near enough to build something that FITS one.
//!
//! The obvious move is a scene-setting model pass: ask it to describe
//! the surroundings, then plan against its own description. That is a
//! third of the latency spent producing a worse answer than the one
//! already available, because perception here is not inference. The
//! world is right there and exactly knowable. So this pass is COMPUTED,
//! and the model's calls are spent on the two jobs that actually need
//! judgement: what to build, and how to say it.
//!
//! Three things go into the context:
//!
//! * A **plan view** — the ground heights around the anchor, drawn as a
//!   height map with a character per band. This is the one that carries
//!   the most: slope, cliffs, water, and where there is room.
//! * An **elevation** — a north-south section through the anchor, so a
//!   drop or a rise reads as a shape rather than as numbers.
//! * **Words** — "a steep west-facing slope, water 9 blocks north, an
//!   existing structure adjoining to the east". A model acts on a
//!   phrase more reliably than on a grid, and the grid is there for the
//!   cases the phrase misses.
//!
//! The character bands are relative to the anchor, not absolute: what
//! matters is what is higher and lower than where you are standing.

use crate::{VoxelTools, VoxelWorld};

/// A read of the surroundings, ready to be pasted into a prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// Height map around the anchor, one char per column.
    pub plan: String,
    /// North-south section through the anchor.
    pub section: String,
    /// Plain-language notes, most important first.
    pub notes: Vec<String>,
    /// Anchor-relative height range covered by the plan view.
    pub relief: (i32, i32),
}

impl Scene {
    /// The block handed to the model. Compact on purpose: it competes
    /// with the region digest and the system prompt for a small
    /// model's attention.
    pub fn prompt_block(&self) -> String {
        let mut s = String::from("TERRAIN (plan view, @ = you, higher is later in .-=+#^):\n");
        s.push_str(&self.plan);
        s.push_str("\n\nsection north-south through you:\n");
        s.push_str(&self.section);
        if !self.notes.is_empty() {
            s.push_str("\n\n");
            s.push_str(&self.notes.iter()
                .map(|n| format!("- {n}")).collect::<Vec<_>>().join("\n"));
        }
        s
    }
}

/// Height bands relative to the anchor. Six characters is as many as a
/// reader can hold in their head at once; more precision would not be
/// used and costs tokens.
fn band(dy: i32) -> char {
    match dy {
        d if d <= -8 => '.',
        d if d <= -3 => '-',
        d if d < 2 => '=',
        d if d < 5 => '+',
        d if d < 10 => '#',
        _ => '^',
    }
}

/// Sample the world around an anchor and describe it.
///
/// `radius` is in blocks; the plan view is downsampled to at most
/// `cols` characters so a 48-block landscape read still fits a prompt.
pub fn read<W: VoxelWorld>(tools: &VoxelTools<W>, anchor: (i32, i32, i32),
                           radius: i32, from_y: i32, cols: usize) -> Scene {
    let radius = radius.max(2);
    let cols = cols.clamp(5, 41);
    // The sampled range is 2r+1 wide, not 2r, and integer division
    // rounds DOWN — both of which add columns. Round up on the true
    // width or a 120-block read overflows the budget by a few chars,
    // which is how this first shipped.
    let width = (radius * 2 + 1) as usize;
    let step = width.div_ceil(cols).max(1) as i32;
    // Sample OUTWARD FROM ZERO, so the anchor column is always taken.
    // Striding from -radius upward skips it whenever step does not
    // divide radius, and the '@' -- the one mark that tells the model
    // where it is standing -- silently vanished from every downsampled
    // view.
    let half = radius / step;
    let offsets: Vec<i32> = (-half..=half).map(|k| k * step).collect();

    let mut rows: Vec<String> = Vec::new();
    let (mut lowest, mut highest) = (i32::MAX, i32::MIN);
    let mut water_dirs: Vec<(&str, i32)> = Vec::new();
    let mut heights: Vec<(i32, i32, i32)> = Vec::new();   // dx, dz, dy

    for &z in &offsets {
        let mut row = String::new();
        for &x in &offsets {
            let (wx, wz) = (anchor.0 + x, anchor.2 + z);
            match tools.find_surface(wx, wz, from_y) {
                Some(sy) => {
                    let dy = sy - anchor.1;
                    lowest = lowest.min(dy);
                    highest = highest.max(dy);
                    heights.push((x, z, dy));
                    // Water reads as its own character: it is the thing
                    // most likely to make a plan unbuildable.
                    if tools.world.get(wx, sy, wz) == 9 {
                        row.push('~');
                        if x == 0 || z == 0 { water_dirs.push((compass(x, z), x.abs().max(z.abs()))); }
                    } else if x == 0 && z == 0 {
                        row.push('@');
                    } else {
                        row.push(band(dy));
                    }
                }
                None => row.push(' '),
            }
        }
        rows.push(row);
    }

    // North-south section through the anchor, drawn as a profile.
    let section_heights: Vec<i32> = offsets.iter()
        .map(|&z| tools.find_surface(anchor.0, anchor.2 + z, from_y)
             .map(|s| s - anchor.1).unwrap_or(0))
        .collect();
    let section = draw_section(&section_heights);

    let mut notes = Vec::new();

    // Slope: compare the mean height on each side. A number is useless
    // here; "a steep slope falling to the west" is actionable.
    if !heights.is_empty() {
        let mean = |f: &dyn Fn(&(i32, i32, i32)) -> bool| {
            let v: Vec<i32> = heights.iter().filter(|h| f(h)).map(|h| h.2).collect();
            if v.is_empty() { 0 } else { v.iter().sum::<i32>() / v.len() as i32 }
        };
        let (w, e) = (mean(&|h| h.0 < 0), mean(&|h| h.0 > 0));
        let (n, s) = (mean(&|h| h.1 < 0), mean(&|h| h.1 > 0));
        let (dx, dz) = (e - w, s - n);
        let grade = dx.abs().max(dz.abs());
        if grade >= 3 {
            let dir = if dx.abs() >= dz.abs() {
                if dx > 0 { "east" } else { "west" }
            } else if dz > 0 { "south" } else { "north" };
            let steep = if grade >= 10 { "steeply" } else { "gently" };
            notes.push(format!("the ground rises {steep} to the {dir} ({grade} blocks across the view)"));
        } else {
            notes.push("the ground is level here".into());
        }
    }

    if lowest != i32::MAX && highest - lowest >= 12 {
        notes.push(format!(
            "big relief: {} blocks between the lowest and highest ground in view -- \
             a large flat build will need terracing or terraform first",
            highest - lowest));
    }

    if let Some((dir, dist)) = water_dirs.iter().min_by_key(|(_, d)| *d) {
        notes.push(format!("water {dist} blocks to the {dir}"));
    }

    // Room: how much of the view is within a couple of blocks of the
    // anchor's own level. This is the number that decides whether a
    // footprint fits without earthworks.
    if !heights.is_empty() {
        let flat = heights.iter().filter(|h| (h.2).abs() <= 2).count();
        let pct = flat * 100 / heights.len();
        notes.push(format!("{pct}% of the view is within 2 blocks of your level"));
        if pct < 25 {
            notes.push("very little level ground: prefer a small footprint, or terraform".into());
        }
    }

    Scene {
        plan: rows.join("\n"),
        section,
        notes,
        relief: (if lowest == i32::MAX { 0 } else { lowest },
                 if highest == i32::MIN { 0 } else { highest }),
    }
}

/// A profile drawn as columns, anchor level marked.
fn draw_section(heights: &[i32]) -> String {
    if heights.is_empty() { return String::new(); }
    let hi = heights.iter().copied().max().unwrap_or(0).max(1);
    let lo = heights.iter().copied().min().unwrap_or(0).min(-1);
    // Cap the drawn height: a cliff should not produce forty rows.
    let (hi, lo) = (hi.min(8), lo.max(-8));
    let mut out = Vec::new();
    let mut y = hi;
    while y >= lo {
        let row: String = heights.iter()
            .map(|h| {
                let h = (*h).clamp(lo, hi);
                if h == y { '#' } else if h > y { '|' } else if y == 0 { '-' } else { ' ' }
            })
            .collect();
        out.push(row);
        y -= 1;
    }
    out.join("\n")
}

fn compass(x: i32, z: i32) -> &'static str {
    if x.abs() >= z.abs() {
        if x > 0 { "east" } else { "west" }
    } else if z > 0 { "south" } else { "north" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Terrain { f: fn(i32, i32) -> i32, cells: BTreeMap<(i32, i32, i32), u16> }
    impl VoxelWorld for Terrain {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
            if y <= (self.f)(x, z) { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
            self.cells.insert((x, y, z), b); true
        }
    }
    fn t(f: fn(i32, i32) -> i32) -> Terrain { Terrain { f, cells: BTreeMap::new() } }

    #[test]
    fn flat_ground_says_so_and_draws_flat() {
        let mut w = t(|_, _| 64);
        let tools = VoxelTools::new(&mut w);
        let s = read(&tools, (0, 65, 0), 12, 200, 13);
        assert!(s.notes.iter().any(|n| n.contains("level here")), "{:?}", s.notes);
        assert!(s.plan.contains('@'), "the anchor is marked");
        // every column the same band
        let chars: std::collections::BTreeSet<char> =
            s.plan.chars().filter(|c| !c.is_whitespace() && *c != '@').collect();
        assert_eq!(chars.len(), 1, "flat ground is one band: {chars:?}");
        assert!(s.notes.iter().any(|n| n.contains("100%")), "{:?}", s.notes);
    }

    #[test]
    fn a_slope_is_named_with_its_direction() {
        // rising to the east
        let mut w = t(|x, _| 64 + x / 2);
        let tools = VoxelTools::new(&mut w);
        let s = read(&tools, (0, 65, 0), 16, 250, 17);
        let note = s.notes.iter().find(|n| n.contains("rises")).expect("a slope note");
        assert!(note.contains("east"), "{note}");
    }

    #[test]
    fn big_relief_warns_before_a_flat_build_is_attempted() {
        let mut w = t(|x, _| 64 + x * 2);
        let tools = VoxelTools::new(&mut w);
        let s = read(&tools, (0, 65, 0), 16, 300, 17);
        assert!(s.notes.iter().any(|n| n.contains("big relief")), "{:?}", s.notes);
        assert!(s.notes.iter().any(|n| n.contains("terrac") || n.contains("terraform")),
                "and says what to do about it: {:?}", s.notes);
    }

    #[test]
    fn water_is_called_out_with_a_direction() {
        let mut w = t(|_, _| 64);
        for x in -20..=20 { for z in 6..12 { w.set(x, 64, z, 9); } }
        let tools = VoxelTools::new(&mut w);
        let s = read(&tools, (0, 65, 0), 14, 200, 15);
        assert!(s.plan.contains('~'), "water is drawn:\n{}", s.plan);
        let note = s.notes.iter().find(|n| n.contains("water")).expect("a water note");
        assert!(note.contains("south"), "{note}");
    }

    #[test]
    fn the_bands_are_relative_to_where_you_stand() {
        // Identical terrain shape, different absolute altitude: the
        // picture must be the same, because what matters is what is
        // higher and lower than YOU.
        let mut a = t(|x, _| 64 + x / 3);
        let mut b = t(|x, _| 180 + x / 3);
        let sa = read(&VoxelTools::new(&mut a), (0, 65, 0), 12, 250, 13);
        let sb = read(&VoxelTools::new(&mut b), (0, 181, 0), 12, 300, 13);
        assert_eq!(sa.plan, sb.plan, "the same shape reads the same at any altitude");
    }

    #[test]
    fn a_cliff_does_not_produce_forty_rows_of_section() {
        let mut w = t(|_, z| if z > 0 { 100 } else { 64 });
        let tools = VoxelTools::new(&mut w);
        let s = read(&tools, (0, 65, 0), 12, 300, 13);
        assert!(s.section.lines().count() <= 18,
                "the section is capped, not one row per block: {}", s.section.lines().count());
        assert!(s.section.contains('|'), "the cliff face is drawn");
    }

    #[test]
    fn the_view_is_downsampled_to_fit_a_prompt() {
        let mut w = t(|_, _| 64);
        let tools = VoxelTools::new(&mut w);
        let s = read(&tools, (0, 65, 0), 60, 200, 21);
        for line in s.plan.lines() {
            assert!(line.chars().count() <= 22,
                    "a 120-block read must still fit: {} chars", line.chars().count());
        }
        assert!(s.prompt_block().len() < 2000, "and the whole block stays small");
    }

    #[test]
    fn the_prompt_block_carries_all_three_parts() {
        let mut w = t(|x, _| 64 + x / 2);
        let tools = VoxelTools::new(&mut w);
        let b = read(&VoxelTools::new(&mut t(|x, _| 64 + x / 2)), (0, 65, 0), 12, 250, 13)
            .prompt_block();
        let _ = tools;
        assert!(b.contains("TERRAIN"));
        assert!(b.contains("section"));
        assert!(b.contains("- "), "notes are bulleted: {b}");
    }

    #[test]
    fn the_anchor_is_always_marked_however_the_view_is_downsampled() {
        // Striding from -radius upward skips x=0 whenever the step does
        // not divide the radius, and the one mark telling the model
        // where it stands vanished from every downsampled view.
        let mut w = t(|x, _| 64 + x / 2);
        for radius in [7, 12, 20, 33, 47] {
            for cols in [7, 11, 15, 21] {
                let tools = VoxelTools::new(&mut w);
                let s = read(&tools, (0, 65, 0), radius, 300, cols);
                assert!(s.plan.contains('@'),
                        "r={radius} cols={cols} lost the anchor:\n{}", s.plan);
                // and it is in the middle row and middle column
                let lines: Vec<&str> = s.plan.lines().collect();
                let mid = lines.len() / 2;
                assert!(lines[mid].contains('@'),
                        "r={radius} cols={cols}: anchor not centred:\n{}", s.plan);
            }
        }
    }

    #[test]
    fn the_view_is_square() {
        let mut w = t(|_, _| 64);
        let tools = VoxelTools::new(&mut w);
        let s = read(&tools, (0, 65, 0), 20, 300, 15);
        let lines: Vec<&str> = s.plan.lines().collect();
        let w0 = lines[0].chars().count();
        assert!(lines.iter().all(|l| l.chars().count() == w0), "ragged view");
        assert_eq!(lines.len(), w0, "rows and columns must match, or north reads as east");
    }
}
