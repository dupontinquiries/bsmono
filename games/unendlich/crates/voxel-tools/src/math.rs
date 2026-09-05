//! Spatial arithmetic: do it for the model, then check what it did
//! anyway.
//!
//! Models are unreliable at exactly the arithmetic voxel building
//! needs — inclusive ranges, radius versus diameter, centre versus
//! corner, volume growth. The response here is two layers, in this
//! order of preference:
//!
//! **1. Remove the arithmetic.** A dimensional syntax lets the model
//! state intent and the tool compute coordinates:
//!
//! ```text
//!     box at 10,64,10 size 20x8x20      -> min/max computed, no off-by-one
//!     tower at 27,-3 diameter=8 h=12    -> radius computed, no doubling error
//!     grid 5x3 spacing=12 at 0,0        -> positions computed
//! ```
//!
//! An off-by-one is impossible if the model never writes the second
//! corner. This is the layer that actually fixes the problem.
//!
//! **2. Check what it wrote anyway**, for the cases where raw
//! coordinates arrive — from native tool calls, from a user, from an
//! older plan. `check_math` recomputes every span, volume and centre
//! and reports mismatches as messages a model can act on, phrased as
//! corrections rather than complaints.
//!
//! **The echo.** `explain_plan` restates a plan with the arithmetic
//! spelled out — spans, volumes, block counts, the resolved Y. Sent
//! back before commit, it does two useful things: the user can catch
//! "that's ten times bigger than I meant" before it happens, and the
//! model sees its own numbers worked out, which is where it most
//! often notices its own error.

use crate::plan::{BuildPlan, Op};

#[derive(Debug, Clone, PartialEq)]
pub enum Note {
    /// Certainly wrong; refuse to commit.
    Error,
    /// Probably not what was meant; show the user.
    Suspect,
    /// Worked arithmetic, for the echo.
    Info,
}

#[derive(Debug, Clone)]
pub struct MathNote {
    pub level: Note,
    pub op: usize,
    pub message: String,
    /// A corrected line the model can use verbatim, when one exists.
    pub suggestion: Option<String>,
}

/// Inclusive span between two coordinates. The single most common
/// off-by-one: a box from 10 to 30 is 21 blocks wide, not 20.
pub fn span(a: i32, b: i32) -> i32 {
    (b - a).abs() + 1
}

/// Corner-and-size to inclusive min/max, so the model never computes
/// a second corner. `size` is in blocks; a size of 1 is a single
/// block at the origin.
pub fn box_from_size(at: (i32, i32, i32), size: (i32, i32, i32))
    -> ((i32, i32, i32), (i32, i32, i32)) {
    let d = |s: i32| if s >= 0 { s.max(1) - 1 } else { s.min(-1) + 1 };
    (at, (at.0 + d(size.0), at.1 + d(size.1), at.2 + d(size.2)))
}

/// Radius that produces a given outside diameter. A diameter of 8
/// needs radius 3 (7 blocks) or 4 (9 blocks) — exact even diameters
/// are not representable on a block grid, and silently rounding is
/// how "make it 8 wide" becomes 9. Returns the radius and the
/// diameter actually achieved.
pub fn radius_for_diameter(d: i32) -> (i32, i32) {
    // Clamp to 1: (d-1)/2 gives 0 for d<=2, and a radius of 0 draws
    // nothing at all. A caller asking for a 1-wide tower wants the
    // smallest tower, not an empty one.
    let r = ((d - 1) / 2).max(1);
    (r, 2 * r + 1)
}

/// Evenly spaced positions along a line, inclusive of both ends.
/// Spacing arithmetic ("five towers twelve apart") is a frequent
/// source of drift when done token by token.
pub fn spaced(from: i32, to: i32, count: usize) -> Vec<i32> {
    if count == 0 { return Vec::new(); }
    if count == 1 { return vec![from]; }
    let step = (to - from) as f64 / (count - 1) as f64;
    (0..count).map(|i| from + (step * i as f64).round() as i32).collect()
}

fn cyl_volume(r: i32, h: i32, hollow: bool) -> i64 {
    let area = if hollow {
        // shell only: outer disc minus inner disc
        let outer = std::f64::consts::PI * (r as f64 + 0.5).powi(2);
        let inner = std::f64::consts::PI * ((r - 1).max(0) as f64 + 0.5).powi(2);
        (outer - inner).max(1.0)
    } else {
        std::f64::consts::PI * (r as f64 + 0.5).powi(2)
    };
    (area * h as f64) as i64
}

/// Recompute the arithmetic in a plan and report discrepancies.
/// `budget` is the block count above which a build is called out as
/// probably unintended.
pub fn check_math(plan: &BuildPlan, budget: i64) -> Vec<MathNote> {
    let mut out = Vec::new();
    let mut total: i64 = 0;

    for (i, op) in plan.ops.iter().enumerate() {
        let n = i + 1;
        let mut push = |level: Note, message: String, suggestion: Option<String>| {
            out.push(MathNote { level, op: n, message, suggestion });
        };
        match op {
            Op::Fill { min, max, hollow, .. } => {
                let (w, h, d) = (span(min.0, max.0), span(min.1, max.1), span(min.2, max.2));
                let vol = if *hollow {
                    (w as i64 * h as i64 * d as i64)
                        - ((w - 2).max(0) as i64 * (h - 2).max(0) as i64 * (d - 2).max(0) as i64)
                } else { w as i64 * h as i64 * d as i64 };
                total += vol;
                push(Note::Info,
                     format!("fill spans {w}x{h}x{d} = {vol} blocks (inclusive of both corners)"),
                     None);
                if min.0 > max.0 || min.1 > max.1 || min.2 > max.2 {
                    push(Note::Suspect,
                         "min is greater than max on some axis; the box will be normalised, \
                          which may not be where you meant".into(),
                         Some(format!("fill {},{},{} {},{},{}",
                              min.0.min(max.0), min.1.min(max.1), min.2.min(max.2),
                              min.0.max(max.0), min.1.max(max.1), min.2.max(max.2))));
                }
            }
            Op::Cylinder { radius, height, hollow, at, .. } => {
                let vol = cyl_volume(*radius, *height, *hollow);
                total += vol;
                let dia = 2 * radius + 1;
                push(Note::Info,
                     format!("tower at {},{}: radius {radius} = {dia} blocks across, \
                              height {height}, about {vol} blocks", at.0, at.1),
                     None);
                // Radius/diameter confusion: a "10 wide" tower written
                // as r=10 is 21 across. Large radii on short towers are
                // the signature.
                if *radius >= 8 && *height <= *radius {
                    push(Note::Suspect,
                         format!("radius {radius} gives {dia} blocks across but the tower is only \
                                  {height} tall — if you meant {radius} blocks WIDE, use r={}",
                                 radius_for_diameter(*radius).0),
                         Some(format!("cylinder {},{} r={} h={}", at.0, at.1,
                                      radius_for_diameter(*radius).0, height)));
                }
            }
            Op::Sphere { radius, hollow, at, .. } => {
                let vol = if *hollow {
                    (4.0 * std::f64::consts::PI * (*radius as f64 + 0.5).powi(2)) as i64
                } else {
                    (4.0 / 3.0 * std::f64::consts::PI * (*radius as f64 + 0.5).powi(3)) as i64
                };
                total += vol;
                push(Note::Info,
                     format!("dome at {},{}: radius {radius} = {} across, about {vol} blocks",
                             at.0, at.1, 2 * radius + 1),
                     None);
            }
            Op::Path { from, to, width, .. } => {
                let dx = (to.0 - from.0).abs();
                let dz = (to.1 - from.1).abs();
                let len = dx.max(dz) + 1;
                let vol = len as i64 * (*width as i64);
                total += vol;
                let diag = ((dx * dx + dz * dz) as f64).sqrt();
                push(Note::Info,
                     format!("path {len} steps ({dx} east, {dz} south, {diag:.1} direct), \
                              width {width}, about {vol} blocks"),
                     None);
            }
            Op::Enclose { min, max, height, .. } => {
                let (w, d) = (span(min.0, max.0), span(min.1, max.1));
                // columns on the boundary of a w x d rectangle:
                // 2w + 2d - 4 (the four corners would be double counted)
                let perim = if w <= 1 || d <= 1 {
                    (w as i64) * (d as i64)
                } else {
                    (2 * w + 2 * d - 4) as i64
                };
                let vol = perim * (*height as i64);
                total += vol;
                push(Note::Info,
                     format!("wall around {w}x{d}: perimeter {perim} columns x {height} high \
                              = {vol} blocks (interior {}x{} stays open)",
                             (w - 2).max(0), (d - 2).max(0)),
                     None);
            }
            Op::Terraform { min, max, delta, smooth, .. } => {
                let cols = span(min.0, max.0) as i64 * span(min.1, max.1) as i64;
                let vol = if *smooth { cols } else { cols * delta.unsigned_abs() as i64 };
                total += vol;
                push(Note::Info,
                     format!("terraform {} columns, {}",
                             cols,
                             if *smooth { "smoothing".into() } else { format!("{delta:+} blocks each") }),
                     None);
                if cols > 65_536 {
                    push(Note::Error,
                         format!("{cols} columns exceeds the 65536 limit; split the footprint"),
                         None);
                }
            }
            Op::Scatter { clusters, per, .. } => {
                let vol = (*clusters as i64) * (*per as i64);
                total += vol;
                push(Note::Info,
                     format!("{clusters} clusters x {per} = up to {vol} decorations \
                              (fewer if the ground is occupied)"),
                     None);
            }
            Op::Tree { height, .. } => {
                total += *height as i64 + 60;
                push(Note::Info, format!("tree height {height}, roughly {} blocks with canopy",
                                          height + 60), None);
            }
            Op::Weather { min, max, preset, .. } => {
                let vol = span(min.0, max.0) as i64 * span(min.1, max.1) as i64
                        * span(min.2, max.2) as i64;
                push(Note::Info,
                     format!("weather '{preset}' over {vol} cells \
                              (only exposed blocks change; the core is untouched)"),
                     None);
            }
            Op::Views { views, at, depth, .. } => {
                let names: Vec<&str> = views.iter().map(|(n, _)| n.as_str()).collect();
                let biggest = views.iter()
                    .map(|(_, a)| crate::views::Mask::parse(a).count()).max().unwrap_or(0);
                // Upper bound only: the hull is the INTERSECTION, so the
                // real count is at most one silhouette swept the depth.
                total += (biggest * *depth as usize) as i64;
                push(Note::Info,
                     format!("views [{}] at {:?}, depth {depth}: at most {} blocks \
                              (the intersection is usually far smaller)",
                             names.join(", "), at, biggest * *depth as usize),
                     None);
                if views.len() < 2 {
                    push(Note::Suspect,
                         "one view only: this is a plain extrusion, and any shape that \
                          varies along the view axis needs a second drawing".into(),
                         None);
                }
            }
            Op::Form { name, min, max, style, nums, .. } => {
                // The arithmetic echo cannot know a recipe's volume
                // without building it, and building it here would double
                // the work. Report the footprint and the parameters --
                // which is what the player needs to sanity-check scale --
                // and let the dry run give the exact count.
                let (w, d) = (span(min.0, max.0), span(min.1, max.1));
                let params = if nums.is_empty() { String::new() } else {
                    format!("  {}", nums.iter().map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>().join(" "))
                };
                let sty = if style.is_empty() { String::new() } else { format!(" {style}") };
                total += w as i64 * d as i64 * 2;
                push(Note::Info,
                     format!("form '{name}'{sty} over {w}x{d}{params} \
                              (exact count from the dry run)"),
                     None);
            }
            Op::Roof { min, max, style, overhang, .. } => {
                let (w, d) = (span(min.0, max.0) + 2 * overhang, span(min.1, max.1) + 2 * overhang);
                // A pitched roof is roughly the footprint plus its slopes;
                // close enough to warn about scale, which is the job.
                let vol = match style.as_str() {
                    "flat" => w as i64 * d as i64,
                    _ => (w as i64 * d as i64 * 3) / 2,
                };
                total += vol;
                push(Note::Info,
                     format!("{style} roof over {w}x{d} (overhang {overhang}), about {vol} blocks"),
                     None);
            }
            Op::Mirror { min, max, axis, about } => {
                let vol = span(min.0, max.0) as i64 * span(min.1, max.1) as i64
                        * span(min.2, max.2) as i64;
                push(Note::Info,
                     format!("mirror across {axis}={about}, scanning {vol} cells"), None);
            }
            Op::Bridge { from, to, width, supports, .. } => {
                let len = (to.0 - from.0).abs().max((to.1 - from.1).abs()) + 1;
                let vol = len as i64 * *width as i64;
                total += vol;
                push(Note::Info,
                     format!("bridge {len} long x {width} wide, about {vol} blocks{}",
                             if *supports { " plus piers" } else { " (no supports)" }),
                     None);
            }
            Op::Stairs { from, to, width, .. } => {
                let len = (to.0 - from.0).abs().max((to.1 - from.1).abs()) + 1;
                total += len as i64 * *width as i64;
                push(Note::Info,
                     format!("stairs {len} steps, rising {len} blocks, {width} wide"), None);
            }
            Op::Floor { min, max, .. } => {
                let vol = span(min.0, max.0) as i64 * span(min.1, max.1) as i64;
                total += vol;
                push(Note::Info, format!("floor {}x{} = {vol} blocks",
                                          span(min.0, max.0), span(min.1, max.1)), None);
            }
            Op::Restyle { min, max, .. } => {
                let vol = span(min.0, max.0) as i64 * span(min.1, max.1) as i64
                        * span(min.2, max.2) as i64;
                push(Note::Info, format!("restyle scans {vol} cells"), None);
            }
        }
    }

    out.push(MathNote {
        level: if total > budget { Note::Suspect } else { Note::Info },
        op: 0,
        message: format!("plan total: about {total} blocks{}", if total > budget {
            format!(" — over the {budget} budget; check the scale is what you meant")
        } else { String::new() }),
        suggestion: None,
    });
    out
}

/// The echo: restate a plan with its arithmetic worked out. Send this
/// back before committing. The user catches scale errors, and the
/// model sees its own numbers computed — which is where it usually
/// notices it meant width and wrote radius.
pub fn explain_plan(plan: &BuildPlan, budget: i64) -> String {
    let notes = check_math(plan, budget);
    let mut s = format!("Plan '{}' — {} operations\n", plan.name, plan.ops.len());
    for n in notes.iter().filter(|n| n.op > 0) {
        let tag = match n.level {
            Note::Error => "ERROR  ",
            Note::Suspect => "CHECK  ",
            Note::Info => "       ",
        };
        s.push_str(&format!("{tag}{}. {}\n", n.op, n.message));
        if let Some(sug) = &n.suggestion {
            s.push_str(&format!("         suggested: {sug}\n"));
        }
    }
    if let Some(tot) = notes.iter().find(|n| n.op == 0) {
        s.push_str(&format!("\n{}\n", tot.message));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parse_plan;

    #[test]
    fn span_is_inclusive() {
        assert_eq!(span(10, 30), 21, "10..30 inclusive is 21 blocks, not 20");
        assert_eq!(span(0, 0), 1);
        assert_eq!(span(30, 10), 21, "order must not matter");
    }

    #[test]
    fn size_to_corners_removes_the_off_by_one() {
        let (min, max) = box_from_size((10, 64, 10), (20, 8, 20));
        assert_eq!(min, (10, 64, 10));
        assert_eq!(max, (29, 71, 29));
        assert_eq!(span(min.0, max.0), 20, "a size of 20 must produce 20 blocks");
        assert_eq!(span(min.1, max.1), 8);
    }

    #[test]
    fn size_of_one_is_a_single_block() {
        let (min, max) = box_from_size((5, 5, 5), (1, 1, 1));
        assert_eq!(min, max);
    }

    #[test]
    fn diameter_to_radius_reports_what_it_can_actually_do() {
        // even diameters are not representable on a block grid
        assert_eq!(radius_for_diameter(9), (4, 9));
        assert_eq!(radius_for_diameter(8), (3, 7), "8 is not achievable; 7 is the honest answer");
        // degenerate input must still return a usable radius rather
        // than 0, which would make an empty cylinder
        let (r, achieved) = radius_for_diameter(1);
        assert!(r >= 1 && achieved >= 3, "got r={r} achieved={achieved}");
    }

    #[test]
    fn spacing_hits_both_ends() {
        let p = spaced(0, 48, 5);
        assert_eq!(p.first(), Some(&0));
        assert_eq!(p.last(), Some(&48));
        assert_eq!(p.len(), 5);
        assert_eq!(p, vec![0, 12, 24, 36, 48]);
    }

    #[test]
    fn radius_diameter_confusion_is_flagged_with_a_fix() {
        let (plan, _) = parse_plan("cylinder 0,0 r=10 h=6 block=stone\n");
        let notes = check_math(&plan, 1_000_000);
        let sus = notes.iter().find(|n| n.level == Note::Suspect)
            .expect("a radius-10 tower 6 tall should be questioned");
        assert!(sus.message.contains("21 blocks across"));
        assert!(sus.suggestion.as_ref().unwrap().contains("r=4"),
                "should suggest the radius for a 10-wide tower: {sus:?}");
    }

    #[test]
    fn reversed_corners_are_flagged_with_a_normalised_line() {
        let plan = BuildPlan {
            name: "t".into(),
            ops: vec![Op::Fill { min: (30, 70, 30), max: (10, 64, 10), block: 1, hollow: false }],
            anchor: None,
        };
        let notes = check_math(&plan, 1_000_000);
        let sus = notes.iter().find(|n| n.level == Note::Suspect).expect("should flag");
        assert!(sus.suggestion.as_ref().unwrap().contains("10,64,10"));
    }

    #[test]
    fn totals_flag_an_unintended_scale() {
        let (plan, _) = parse_plan("cylinder 0,0 r=60 h=200 block=stone\n");
        let notes = check_math(&plan, 200_000);
        let total = notes.iter().find(|n| n.op == 0).unwrap();
        assert_eq!(total.level, Note::Suspect);
        assert!(total.message.contains("budget"));
    }

    #[test]
    fn the_echo_states_the_working() {
        let (plan, _) = parse_plan(
            "path 0,0 30,40 w=3 block=cobblestone\nenclose 0,0 10,10 h=3 block=oak_planks\n");
        let text = explain_plan(&plan, 1_000_000);
        assert!(text.contains("41 steps"), "path length should be computed: {text}");
        assert!(text.contains("50.0 direct"), "diagonal distance too: {text}");
        // enclose 0,0 10,10 spans 11x11 inclusive -> 2*11+2*11-4 = 40 columns
        assert!(text.contains("perimeter 40"), "wall perimeter: {text}");
        assert!(text.contains("interior 9x9"), "and what stays open: {text}");
    }
}
