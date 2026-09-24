//! story — a draft outline from the footage itself, for when there is no
//! script to hand (or the script is intent, not inventory).
//!
//! The order of a story is two questions, answered separately because they
//! are known with very different confidence:
//!
//! * **Within a scene**, the order of the lines is well known: every take is
//!   a run-through of the scene, so the MOST COMPLETE take is its script.
//!   Lines that only other takes have (an ad-lib, a line the reference take
//!   dropped) are slotted in after the line they followed in their own take.
//! * **Between scenes**, the footage says little. Slate boards say something
//!   when they were read confidently -- and never trust a number otherwise
//!   (the film's rule 1) -- so the caller may pass scene labels with a
//!   sort key; if EVERY scene has a sure one they order the scenes. Otherwise
//!   scenes go in shot order and the draft says so on its review list.
//!
//! Scenes are found by what is SAID, not by numbers: two takes whose lines
//! mostly match are takes of the same scene (union-find over "shares at least
//! half of the shorter take's lines"). Angles of one performance
//! ([`Take::angle_group`]) are one scene by definition.
//!
//! The result is TEXT ([`Story::outline_text`]): a `#` header per scene
//! naming its takes and why it sits where it does, then its lines. The
//! editor reorders scenes by moving blocks and deletes lines by deleting
//! them, in any text editor, and [`crate::parse_outline`] reads it back
//! (headers are comments). That file is the steering wheel.

use std::collections::BTreeMap;

use studio_core::SourceId;

use crate::{clean, similarity, Take};

/// A scene label the caller trusts to some degree (from a slate board).
#[derive(Debug, Clone, PartialEq)]
pub struct SceneLabel {
    pub label: String,
    /// Orders scenes when every scene has one and all are `sure`.
    pub sort: Option<u32>,
    pub sure: bool,
}

/// One scene of the draft.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub label: String,
    /// Take names, shot order.
    pub takes: Vec<String>,
    /// The take whose line order is the scene's.
    pub reference: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Story {
    pub scenes: Vec<Scene>,
    /// How the scenes were ordered, in words.
    pub order: String,
    /// What to check: scene order, lines only one take has, unsure labels.
    pub reviews: Vec<String>,
}

impl Story {
    pub fn line_count(&self) -> usize { self.scenes.iter().map(|s| s.lines.len()).sum() }

    /// The outline file: move blocks to reorder scenes, delete lines to cut.
    pub fn outline_text(&self) -> String {
        let mut t = String::from("# A DRAFT outline, drafted from the footage by studio-assemble.\n");
        t.push_str(&format!("# Scene order: {}\n", self.order));
        t.push_str("# Move a block to move a scene; delete a line to cut it; then run \"assemble\".\n");
        for r in &self.reviews { t.push_str(&format!("# check: {r}\n")); }
        for s in &self.scenes {
            t.push_str(&format!("\n# ── {} ── takes: {} · line order from {}\n", s.label, s.takes.join(", "), s.reference));
            for l in &s.lines { t.push_str(l); t.push('\n'); }
        }
        t
    }
}

/// Same line? The threshold the assembler uses for "said again".
const SAME: f32 = 0.75;

/// Draft the story from `takes`. `labels` is optional per source.
pub fn draft(takes: &[Take], labels: &BTreeMap<SourceId, SceneLabel>, noise: &dyn Fn(&str) -> bool) -> Story {
    let lines: Vec<Vec<String>> = takes.iter().map(|t| clean(t, noise).into_iter()
        .filter(|l| l.dropped.is_none()).map(|l| l.text.trim().to_string()).collect()).collect();
    // ── scenes: union-find over shared lines ────────────────────────────
    let mut parent: Vec<usize> = (0..takes.len()).collect();
    fn root(p: &mut Vec<usize>, i: usize) -> usize { let mut i = i; while p[i] != i { p[i] = p[p[i]]; i = p[i]; } i }
    let shared = |a: &[String], b: &[String]| a.iter().filter(|x| b.iter().any(|y| similarity(x, y) >= SAME)).count();
    for i in 0..takes.len() {
        for j in i + 1..takes.len() {
            let same_angles = takes[i].angle_group.is_some() && takes[i].angle_group == takes[j].angle_group;
            let n = lines[i].len().min(lines[j].len());
            if same_angles || (n > 0 && shared(&lines[i], &lines[j]) * 2 >= n) {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..takes.len() { let r = root(&mut parent, i); groups.entry(r).or_default().push(i); }
    let mut reviews = Vec::new();
    let mut scenes: Vec<(Option<u32>, bool, String, Scene)> = Vec::new(); // (sort, sure, first take name, scene)
    for (_, mut members) in groups {
        members.sort_by(|a, b| takes[*a].name.cmp(&takes[*b].name));
        // the reference: most lines; ties to the LATER take (the one they kept going for)
        let reference = *members.iter().max_by(|a, b| lines[**a].len().cmp(&lines[**b].len())
            .then(takes[**a].name.cmp(&takes[**b].name))).expect("a group has members");
        let mut order: Vec<String> = lines[reference].clone();
        for &m in members.iter().filter(|m| **m != reference) {
            let mut after: Option<usize> = None; // index in `order` of the last matched line
            for l in &lines[m] {
                match order.iter().position(|o| similarity(o, l) >= SAME) {
                    Some(k) => after = Some(k),
                    None => {
                        let at = after.map(|k| k + 1).unwrap_or(0);
                        order.insert(at, l.clone());
                        reviews.push(format!("\"{}\" is only in {} -- an ad-lib or a dropped line?", crate::short(l, 40), takes[m].name));
                        after = Some(at);
                    }
                }
            }
        }
        let lab: Vec<&SceneLabel> = members.iter().filter_map(|m| labels.get(&takes[*m].source)).collect();
        let label = lab.first().map(|l| l.label.clone())
            .unwrap_or_else(|| format!("scene of {}", takes[members[0]].name));
        let sort = lab.first().and_then(|l| l.sort);
        let sure = !lab.is_empty() && lab.iter().all(|l| l.sure && l.sort == sort);
        if !lab.is_empty() && !sure {
            reviews.push(format!("{label}: its takes' slates disagree or were not read surely"));
        }
        scenes.push((sort, sure, takes[members[0]].name.clone(), Scene {
            label, takes: members.iter().map(|m| takes[*m].name.clone()).collect(),
            reference: takes[reference].name.clone(), lines: order }));
    }
    let by_slate = scenes.len() > 1 && scenes.iter().all(|(sort, sure, _, _)| sort.is_some() && *sure);
    let order = if by_slate {
        scenes.sort_by_key(|(sort, _, _, _)| *sort);
        "by slate scene number (every board read surely)".to_string()
    } else {
        scenes.sort_by(|a, b| a.2.cmp(&b.2));
        if scenes.len() > 1 { reviews.insert(0, "scene order is SHOT order, not story order -- move the blocks".into()); }
        "shot order".to_string()
    };
    Story { scenes: scenes.into_iter().map(|s| s.3).collect(), order, reviews }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::Ticks;
    use studio_voice::Segment;

    fn take(id: u64, name: &str, lines: &[&str]) -> Take {
        Take { source: SourceId(id), name: name.into(), len: Ticks::from_seconds(60.0), angle_group: None,
               segments: lines.iter().enumerate().map(|(i, t)| Segment { start: Ticks::from_seconds(i as f64 * 3.0 + 1.0),
                   end: Ticks::from_seconds(i as f64 * 3.0 + 2.5), text: t.to_string(), speaker: None, confidence: 0.9 }).collect() }
    }
    fn none(_: &str) -> bool { false }

    /// Two scenes, shot out of order, with retakes; one retake drops a line
    /// and another adds an ad-lib.
    fn shoot() -> Vec<Take> {
        vec![
            take(1, "C0010", &["Where were you last night?", "Out.", "Out where?"]),               // scene B
            take(2, "C0011", &["Where were you last night?", "Out where?"]),                        // B, drops "Out."
            take(3, "C0020", &["The show opens Friday.", "Are you ready?", "Wait.", "No."]),       // A, its own ad-lib
            take(4, "C0021", &["The show opens Friday.", "Honestly?", "Are you ready?", "No."]),     // A, ad-lib
        ]
    }

    #[test]
    fn retakes_are_one_scene_found_by_what_is_said_and_the_fullest_take_orders_it() {
        let st = draft(&shoot(), &BTreeMap::new(), &none);
        assert_eq!(st.scenes.len(), 2, "{st:?}");
        let b = &st.scenes[0];
        assert_eq!(b.takes, vec!["C0010", "C0011"]);
        assert_eq!(b.reference, "C0010");
        assert_eq!(b.lines, vec!["Where were you last night?", "Out.", "Out where?"], "a dropped line is not lost");
        let a = &st.scenes[1];
        assert_eq!(a.reference, "C0021", "as full as C0020, and later: the one they kept going for");
        // C0020's ad-lib goes where C0020 said it: after "Are you ready?", before "No."
        assert_eq!(a.lines, vec!["The show opens Friday.", "Honestly?", "Are you ready?", "Wait.", "No."]);
        assert!(st.reviews.iter().any(|r| r.contains("\"Wait.\" is only in C0020")), "{:?}", st.reviews);
    }

    #[test]
    fn without_sure_slates_the_scenes_go_in_shot_order_and_the_draft_says_so() {
        let st = draft(&shoot(), &BTreeMap::new(), &none);
        assert_eq!(st.order, "shot order");
        assert!(st.reviews[0].contains("SHOT order"), "{:?}", st.reviews);
        let text = st.outline_text();
        // it reads back as the outline the assembler takes: headers are comments
        let back = crate::parse_outline(&text);
        assert_eq!(back.len(), st.line_count());
        assert_eq!(back[0], "Where were you last night?");
        assert!(text.contains("# ── scene of C0010 ── takes: C0010, C0011"), "{text}");
    }

    #[test]
    fn sure_slates_order_the_scenes_and_unsure_ones_do_not() {
        let lab = |l: &str, n: u32, sure: bool| SceneLabel { label: l.into(), sort: Some(n), sure };
        let mut labels: BTreeMap<SourceId, SceneLabel> = [
            (SourceId(1), lab("Escena 13", 13, true)), (SourceId(2), lab("Escena 13", 13, true)),
            (SourceId(3), lab("Escena 2", 2, true)), (SourceId(4), lab("Escena 2", 2, true)),
        ].into();
        let st = draft(&shoot(), &labels, &none);
        assert_eq!(st.scenes.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), vec!["Escena 2", "Escena 13"]);
        assert!(st.order.starts_with("by slate"));
        // one board misheard ("Scenna 3"): back to shot order, and flagged
        labels.insert(SourceId(2), lab("Escena 3", 3, false));
        let st = draft(&shoot(), &labels, &none);
        assert_eq!(st.order, "shot order");
        assert!(st.reviews.iter().any(|r| r.contains("slates disagree")), "{:?}", st.reviews);
    }

    #[test]
    fn angles_are_one_scene_even_when_their_transcripts_differ() {
        let mut t = vec![take(1, "A001", &["Line one.", "Line two."]), take(2, "A002", &["Something whisper misheard."])];
        t[0].angle_group = Some(1);
        t[1].angle_group = Some(1);
        let st = draft(&t, &BTreeMap::new(), &none);
        assert_eq!(st.scenes.len(), 1);
    }
}
