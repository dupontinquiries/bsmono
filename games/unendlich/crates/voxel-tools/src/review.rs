//! The review loop: the model looks at what it made, and says what to
//! change -- in a language of ten tokens.
//!
//! WHY THE MODEL WAS OUT OF THE LOOP
//!
//! A model writes a plan once. Everything after -- recognizer,
//! finishing, tuner, greebles -- is model-free, because a model
//! cannot see blocks and asking it to reason about coordinates was
//! how we got towers for statues. So it never learned whether the
//! plan worked.
//!
//! WHAT THE MODEL CAN SEE
//!
//! It can read an ELEVATION. A front and a side view of the dry run,
//! downsampled to twenty-odd columns of ASCII, is a picture a language
//! model judges well: "that is a box", "the roof is missing", "the
//! statue has no arms". Beside it: what it asked for (the spec), what
//! the build measured, the ops as a numbered list, and the critique's
//! findings. No coordinates anywhere.
//!
//! WHAT THE MODEL MAY SAY
//!
//!   keep 1 2 4          the ops to keep, by number
//!   drop 3              the ops to drop
//!   add <one IR line>   up to two additions (validated like any line)
//!   spec ...            a nudge to the targets the tuner chases
//!   pick 2              when shown variants: which one
//!   done                it is good; stop
//!
//! Every line is one of these or is ignored. A round is cheap in both
//! directions, so the loop can run two or three times: plan, review,
//! revise, review, done. The tiny tier reviews too -- "pick 2" and
//! "done" are within a 1B.
//!
//! CONVERGENCE
//!
//! The loop stops on `done`, on a round that changes nothing, on the
//! measured distance to the spec no longer improving, or at the round
//! limit. Each round's verdict goes into the training record beside
//! the plan it judged.

use crate::plan::{parse_plan_at, Op, Severity};
use std::collections::BTreeSet;

/// Two elevations of a cell set, downsampled to at most `cols` wide
/// and `rows` tall. '#' solid, '.' empty; the front looks north (z
/// toward the viewer), the side looks west.
pub fn elevations(cells: &BTreeSet<(i32, i32, i32)>, cols: usize, rows: usize) -> String {
    if cells.is_empty() { return "(nothing placed)".into(); }
    let lo = cells.iter().fold((i32::MAX, i32::MAX, i32::MAX), |a, p| (a.0.min(p.0), a.1.min(p.1), a.2.min(p.2)));
    let hi = cells.iter().fold((i32::MIN, i32::MIN, i32::MIN), |a, p| (a.0.max(p.0), a.1.max(p.1), a.2.max(p.2)));
    let (w, h, d) = ((hi.0 - lo.0 + 1) as usize, (hi.1 - lo.1 + 1) as usize, (hi.2 - lo.2 + 1) as usize);
    let sx = (w + cols - 1) / cols.max(1); let sy = (h + rows - 1) / rows.max(1); let sz = (d + cols - 1) / cols.max(1);
    let (sx, sy, sz) = (sx.max(1), sy.max(1), sz.max(1));
    // front: x across, y down; a cell shows if any solid at that (x,y) for any z
    let mut front = vec![vec![false; (w + sx - 1) / sx]; (h + sy - 1) / sy];
    let mut side = vec![vec![false; (d + sz - 1) / sz]; (h + sy - 1) / sy];
    for p in cells {
        let (x, y, z) = ((p.0 - lo.0) as usize / sx, (p.1 - lo.1) as usize / sy, (p.2 - lo.2) as usize / sz);
        front[y][x] = true; side[y][z] = true;
    }
    let mut s = format!("FRONT ({}x{} blocks, 1 char = {}x{}):\n", w, h, sx, sy);
    for row in front.iter().rev() { s.push_str("  "); s.push_str(&row.iter().map(|b| if *b { '#' } else { '.' }).collect::<String>()); s.push('\n'); }
    s.push_str(&format!("SIDE ({}x{} blocks):\n", d, h));
    for row in side.iter().rev() { s.push_str("  "); s.push_str(&row.iter().map(|b| if *b { '#' } else { '.' }).collect::<String>()); s.push('\n'); }
    s
}

/// One variant the model may pick between: a name, its measurement,
/// and a small elevation.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub label: String,
    pub props: String,
    pub elevation: String,
}

/// The review prompt. Everything the model needs to judge, nothing it
/// would misuse.
pub fn prompt(prose: &str, ops: &[Op], spec: Option<&str>, measured: &str, elevation: &str,
              findings: &[String], variants: &[Variant], round: usize) -> String {
    let mut s = format!("REVIEW round {round}. The player asked for: {prose}\n\n");
    // Beyond a dozen ops the plan is GROUPED -- "8 x house (cottage), 2
    // x path" -- with detail only for the ops the findings mention. A
    // model judging a village needs to know there are eight cottages,
    // not the eighth cottage's line.
    let mentioned: Vec<usize> = findings.iter().flat_map(|f| f.split(|c: char| !c.is_ascii_digit())
        .filter_map(|t| t.parse::<usize>().ok())).filter(|n| *n >= 1 && *n <= ops.len()).collect();
    s.push_str("The plan, by number:\n");
    s.push_str(&crate::context::plan_summary(ops, &mentioned, 12));
    s.push('\n');
    if let Some(sp) = spec { s.push_str(&format!("\nTarget: {sp}\n")); }
    s.push_str(&format!("Measured: {measured}\n\n{elevation}\n"));
    if !findings.is_empty() {
        s.push_str("Problems found:\n");
        for f in findings { s.push_str(&format!("  - {f}\n")); }
    }
    if !variants.is_empty() {
        s.push_str("\nVARIANTS to choose from:\n");
        for (i, v) in variants.iter().enumerate() {
            s.push_str(&format!("  [{}] {}: {}\n{}\n", i + 1, v.label, v.props, v.elevation));
        }
    }
    s.push_str("\nDoes it match what the player asked for? Reply with ONLY lines of these forms:\n\
                keep 1 2 4        (op numbers to keep)\n\
                drop 3            (op numbers to drop)\n\
                add <one plan line>   (at most two)\n\
                spec height=A-B durability=low|mid|high sound=... (a target nudge)\n");
    if !variants.is_empty() { s.push_str("pick N            (which variant)\n"); }
    s.push_str("done              (it is right; stop)\n\nReply:\n");
    s
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Review {
    pub keep: Option<Vec<usize>>,
    pub drop: Vec<usize>,
    pub add: Vec<String>,
    pub spec: Option<String>,
    pub pick: Option<usize>,
    pub done: bool,
    /// Lines that were none of the forms, for the log.
    pub ignored: usize,
}

impl Review {
    pub fn is_empty(&self) -> bool {
        self.keep.is_none() && self.drop.is_empty() && self.add.is_empty() && self.spec.is_none()
            && self.pick.is_none() && !self.done
    }
    pub fn summary(&self) -> String {
        let mut v = Vec::new();
        if let Some(k) = &self.keep { v.push(format!("keep {:?}", k)); }
        if !self.drop.is_empty() { v.push(format!("drop {:?}", self.drop)); }
        if !self.add.is_empty() { v.push(format!("add {}", self.add.len())); }
        if let Some(s) = &self.spec { v.push(format!("spec {}", s.trim_start_matches("spec").trim())); }
        if let Some(p) = self.pick { v.push(format!("pick {p}")); }
        if self.done { v.push("done".into()); }
        if v.is_empty() { "no verdict".into() } else { v.join("; ") }
    }
}

/// Parse a reply. Tolerant: numbers may be comma-separated, a `#`
/// before them is fine, case does not matter, and anything that is
/// not one of the forms is counted and ignored.
pub fn parse(reply: &str, n_ops: usize) -> Review {
    let mut r = Review::default();
    let nums = |s: &str| -> Vec<usize> {
        s.split(|c: char| !c.is_ascii_digit()).filter(|t| !t.is_empty())
            .filter_map(|t| t.parse::<usize>().ok()).filter(|n| *n >= 1 && *n <= n_ops).collect()
    };
    for line in reply.lines() {
        let t = line.trim().trim_start_matches(['-', '*', '•']).trim();
        if t.is_empty() { continue; }
        let lower = t.to_lowercase();
        let (verb, rest) = lower.split_once(' ').unwrap_or((lower.as_str(), ""));
        match verb.trim_end_matches(':') {
            "keep" => { let k = nums(rest); if !k.is_empty() { r.keep = Some(k); } }
            "drop" | "remove" | "delete" => r.drop.extend(nums(rest)),
            "add" => {
                if r.add.len() < 2 {
                    let line = t[3..].trim().trim_start_matches(':').trim();
                    if crate::plan::line_is_valid_ir(line) { r.add.push(line.to_string()); } else { r.ignored += 1; }
                }
            }
            "spec" | "properties" => if crate::spec::Spec::parse(t).is_ok() { r.spec = Some(t.to_string()); } else { r.ignored += 1; },
            "pick" | "choose" | "variant" => r.pick = nums(rest).first().copied(),
            "done" | "ok" | "good" | "finished" => r.done = true,
            _ => r.ignored += 1,
        }
    }
    r.drop.sort_unstable(); r.drop.dedup();
    r
}

/// Apply a review to a plan's IR (the expanded text the plan was
/// parsed from), returning the revised IR. Ops are matched by their
/// line order among op-bearing lines; comments, defs and specs are
/// carried. `keep` wins over `drop` when both name an op.
pub fn apply(ir: &str, review: &Review) -> String {
    let mut out = String::new();
    let mut op_index = 0usize;
    for line in ir.lines() {
        let t = line.trim();
        let is_op = !t.is_empty() && !t.starts_with('#')
            && !["def", "end", "use", "spec", "properties"].contains(&t.split_whitespace().next().unwrap_or("").to_lowercase().as_str())
            && { let (p, issues) = parse_plan_at(&format!("{t}\n"), Some((0, 64, 0)));
                 !p.ops.is_empty() && !issues.iter().any(|i| i.severity == Severity::Error) };
        // A spec line is KEPT unless the review replaces it. It is not
        // an op and has no number, so `keep 1 2` must not silently drop
        // the targets the tuner chases -- the run's distance went to
        // None after the first review.
        if t.to_lowercase().starts_with("spec") { if review.spec.is_some() { continue; } out.push_str(line); out.push('\n'); continue; }
        if is_op {
            op_index += 1;
            let kept = review.keep.as_ref().map(|k| k.contains(&op_index)).unwrap_or(true);
            let dropped = review.drop.contains(&op_index) && review.keep.as_ref().map(|k| !k.contains(&op_index)).unwrap_or(true);
            if !kept || dropped { continue; }
        }
        out.push_str(line); out.push('\n');
    }
    if let Some(s) = &review.spec { out = format!("{s}\n{out}"); }
    for a in &review.add { out.push_str(a); out.push('\n'); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevations_downsample_and_show_shape() {
        let mut cells = BTreeSet::new();
        for x in 0..40 { for y in 0..6 { for z in 0..30 { if x == 0 || x == 39 || z == 0 || z == 29 || y == 0 { cells.insert((x, y, z)); } } } }
        for y in 6..20 { cells.insert((20, y, 15)); }   // a spire
        let e = elevations(&cells, 20, 12);
        assert!(e.contains("FRONT (40x20 blocks, 1 char = 2x2)"), "{e}");
        let front: Vec<&str> = e.lines().skip(1).take_while(|l| !l.starts_with("SIDE")).collect();
        assert!(front.len() <= 12);
        assert!(front[0].trim().contains('#') && front[0].trim().matches('#').count() <= 2, "the spire alone at the top: {:?}", front[0]);
        assert!(front.last().unwrap().trim().chars().all(|c| c == '#'), "the base is solid across");
    }

    #[test]
    fn a_verdict_parses_tolerantly() {
        let r = parse("Keep 1, 2 and 4.\n- drop #3\nadd sphere @3,0 r=1 block=air\nadd nonsense here\nspec height=10-14 durability=high\nsomething else\ndone", 5);
        assert_eq!(r.keep, Some(vec![1, 2, 4]));
        assert_eq!(r.drop, vec![3]);
        assert_eq!(r.add, vec!["sphere @3,0 r=1 block=air"]);
        assert!(r.spec.as_deref().unwrap().starts_with("spec height=10-14"));
        assert!(r.done);
        assert_eq!(r.ignored, 2, "the bad add and the stray line");
        assert_eq!(parse("keep 9 12", 5).keep, None, "numbers past the plan are not ops");
        assert_eq!(parse("pick 2\n", 3).pick, Some(2));
        assert!(parse("", 3).is_empty());
    }

    #[test]
    fn a_spec_survives_a_review_that_does_not_replace_it() {
        let ir = "spec height=5-7\nenclose @-4,-4 @4,4 h=5\ntree @9,9 oak h=7\n";
        let out = apply(ir, &parse("keep 1\n", 2));
        assert!(out.contains("spec height=5-7"), "the targets survive: {out}");
        assert!(!out.contains("tree"));
    }

    #[test]
    fn applying_a_review_edits_the_plan_by_op_number() {
        let ir = "# hall\nspec height=5-7\nenclose @-4,-4 @4,4 h=5\nroof @-4,-4 @4,4 gable\ntree @9,9 oak h=7\n";
        let r = parse("drop 3\nadd sphere @4,0 r=1 block=air\nspec height=8-10\n", 3);
        let out = apply(ir, &r);
        assert!(out.starts_with("spec height=8-10\n"), "{out}");
        assert!(out.contains("# hall"));
        assert!(out.contains("enclose") && out.contains("roof"));
        assert!(!out.contains("tree"), "dropped: {out}");
        assert!(out.trim_end().ends_with("sphere @4,0 r=1 block=air"));
        assert!(!out.contains("height=5-7"), "the old spec is replaced");
        // keep wins over drop
        let r2 = parse("keep 1 2 3\ndrop 3\n", 3);
        assert!(apply(ir, &r2).contains("tree"));
    }

    #[test]
    fn the_prompt_shows_ops_by_number_and_never_coordinates_from_the_build() {
        let (plan, _) = parse_plan_at("enclose @-4,-4 @4,4 h=5\nroof @-4,-4 @4,4 gable\n", Some((0, 65, 0)));
        let p = prompt("a hall", &plan.ops, Some("spec height=5-7"), "241 blocks, 6 tall", "FRONT:\n  ####\n",
                       &["3 floating blocks".into()], &[], 1);
        assert!(p.contains("1. ") && p.contains("2. "));
        assert!(p.contains("Target: spec height=5-7") && p.contains("Measured: 241 blocks"));
        assert!(p.contains("- 3 floating blocks"));
        assert!(p.contains("keep 1 2 4") && p.contains("done"));
        assert!(!p.contains("pick N"), "no variants, no pick line");
        let v = vec![Variant { label: "turret".into(), props: "300 blocks".into(), elevation: "  ##\n".into() }];
        let p2 = prompt("a hall", &plan.ops, None, "x", "y", &[], &v, 2);
        assert!(p2.contains("[1] turret") && p2.contains("pick N"));
    }
}
