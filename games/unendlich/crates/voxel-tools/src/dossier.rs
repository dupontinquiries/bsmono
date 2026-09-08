//! Plan once into a document, then execute each part with a clean slate.
//!
//! `context.rs` keeps the conversation and folds its oldest exchanges
//! into a summary when the budget runs out. That is the right thing
//! for a chat and the wrong thing for a build. It means:
//!
//!   * context GROWS with the build, so the twentieth pass is planned
//!     with less room than the first
//!   * the model re-reads its own earlier turns, including the wrong
//!     ones, and a mistake made in turn three is still in the prompt
//!     at turn twenty
//!   * nothing survives the session -- the plan lives only as the
//!     transcript of how it was reached
//!   * a section cannot be re-run without re-running everything before
//!     it
//!
//! A DOSSIER inverts that. The model plans ONCE into a document:
//! what is being built, what parts it has, what each part needs. Then
//! each part is executed in its own turn, whose prompt contains ONLY
//! the goal, that one part, and what the neighbouring parts left
//! behind. No history. The prompt for part twenty is the same size as
//! the prompt for part one.
//!
//! WHAT THAT BUYS
//!
//!   FIXED CONTEXT   every execution turn is small and the same size,
//!                   which is the difference between a 4k model being
//!                   usable and not
//!   RE-RUNNABLE     a part that came out badly is re-run alone, with
//!                   the same prompt it had the first time plus what
//!                   went wrong
//!   INSPECTABLE     the plan is a document the player can read and
//!                   edit before a single block is placed
//!   RESUMABLE       it survives the session, because it is not the
//!                   transcript
//!
//! The cost is that a part cannot see the whole. That is mostly a
//! feature -- it is what keeps the prompt small -- and where it is
//! not, `depends` names the parts whose results are worth carrying.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Not attempted.
    Waiting,
    /// Attempted and something was built.
    Done,
    /// Attempted and it produced nothing.
    Failed,
    /// Deliberately skipped: the plan changed, or it was not needed.
    Skipped,
}

impl State {
    pub fn mark(self) -> char {
        match self { State::Waiting => '.', State::Done => 'x',
                     State::Failed => '!', State::Skipped => '-' }
    }
}

/// One part of the build, executable on its own.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub name: String,
    /// What this part is, in the planner's words. This is the whole
    /// brief for the turn that builds it.
    pub what: String,
    /// Roughly how big, so a turn knows its scale without the whole.
    pub size: Option<i32>,
    /// Names of parts whose results this one needs. Only these are
    /// carried into its prompt -- the point of the dossier is that
    /// most parts need none.
    pub depends: Vec<String>,
    pub state: State,
    /// What happened when it ran: cells built, or why not.
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dossier {
    /// What the player asked for, unchanged. Every turn sees this.
    pub goal: String,
    /// One line: what the whole thing IS. Every turn sees this too,
    /// because a part built without knowing the whole is a part of
    /// nothing.
    pub summary: String,
    pub parts: Vec<Part>,
    pub notes: Vec<String>,
}

impl Dossier {
    pub fn next(&self) -> Option<usize> {
        self.parts.iter().position(|p| p.state == State::Waiting)
    }

    /// Parts done, failed, and left.
    pub fn tally(&self) -> (usize, usize, usize) {
        let d = self.parts.iter().filter(|p| p.state == State::Done).count();
        let f = self.parts.iter().filter(|p| p.state == State::Failed).count();
        (d, f, self.parts.len() - d - f)
    }

    pub fn status(&self) -> String {
        let (d, f, left) = self.tally();
        format!("{}: {d} done, {f} failed, {left} to go  [{}]", self.summary,
                self.parts.iter().map(|p| p.state.mark()).collect::<String>())
    }

    /// THE PROMPT FOR ONE PART, and the reason this module exists.
    ///
    /// It contains the goal, the one-line summary, this part, and the
    /// outcomes of the parts it declared it depends on. It does NOT
    /// contain the conversation, the other parts' briefs, or anything
    /// from earlier turns. Its size does not grow as the build
    /// proceeds, which is what makes a small model usable past the
    /// third pass.
    pub fn part_prompt(&self, i: usize, vocabulary: &str) -> Option<String> {
        let p = self.parts.get(i)?;
        let mut s = format!("\
BUILDING: {}
WHICH IS: {}

YOUR PART, and only this one: {}
{}
", self.goal, self.summary, p.name, p.what);
        if let Some(sz) = p.size { s.push_str(&format!("About {sz} blocks.\n")); }
        // only what this part said it needed
        let done: BTreeMap<&str, &str> = self.parts.iter()
            .filter(|q| q.state == State::Done)
            .map(|q| (q.name.as_str(), q.outcome.as_str())).collect();
        let carried: Vec<String> = p.depends.iter()
            .filter_map(|d| done.get(d.as_str()).map(|o| format!("  {d}: {o}")))
            .collect();
        if !carried.is_empty() {
            s.push_str(&format!("\nALREADY BUILT, which yours must meet:\n{}\n", carried.join("\n")));
        }
        let missing: Vec<&String> = p.depends.iter()
            .filter(|d| !done.contains_key(d.as_str())).collect();
        if !missing.is_empty() {
            // said, not hidden: a part built before what it leans on is
            // a part that will not line up
            s.push_str(&format!("\nNOT BUILT YET: {} -- do not assume where it is.\n",
                                missing.iter().map(|d| d.as_str()).collect::<Vec<_>>().join(", ")));
        }
        s.push_str(&format!("\n{vocabulary}\n\nWrite the passes for YOUR PART only.\n"));
        Some(s)
    }

    /// Record what a part's turn produced.
    pub fn finish(&mut self, i: usize, built: usize, outcome: &str) {
        if let Some(p) = self.parts.get_mut(i) {
            p.state = if built > 0 { State::Done } else { State::Failed };
            p.outcome = if built > 0 { format!("{built} cells; {outcome}") } else { outcome.into() };
        }
    }

    /// Re-run a part, with what went wrong last time added to its
    /// brief. The prompt is otherwise IDENTICAL -- which it can be,
    /// because it never depended on the conversation.
    pub fn retry_prompt(&self, i: usize, vocabulary: &str, why: &str) -> Option<String> {
        let base = self.part_prompt(i, vocabulary)?;
        Some(format!("{base}\nYOUR LAST ATTEMPT AT THIS PART FAILED: {why}\nTry a different \
                      approach, not the same one again.\n"))
    }
}

/// Where a part sits in the whole, decided before anything is built.
///
/// A dossier says what the parts ARE and what each needs; it does not
/// say where they go. Without that a blockout cannot be drawn, and
/// without a blockout the proportions are discovered after a hundred
/// thousand blocks are placed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    /// Offset from the anchor.
    pub at: (i32, i32, i32),
    /// Bounding extent.
    pub size: (i32, i32, i32),
}

/// THE BLOCKOUT: every part as a crude mass, all of it, cheaply.
///
/// This is the practised method and the stage this crate never had.
/// A builder blocks out the whole thing at low resolution first,
/// stands back, and only commits to detail once the silhouette reads.
/// Everything here went straight to full resolution, so a wrong
/// proportion cost a full build to discover -- and the review, which
/// judges what was made, could only ever find it afterwards.
///
/// Cheap on purpose: one box per part. It is not meant to look like
/// anything, it is meant to be JUDGED and thrown away.
pub fn blockout(d: &Dossier, anchor: (i32, i32, i32)) -> (Vec<Slot>, String) {
    let n = d.parts.len().max(1);
    // A part's size is a floor AREA in the plan's terms; its mass is
    // roughly cubic unless it depends on something, in which case it
    // stacks on it -- which is what `after` usually means physically.
    let mut slots: Vec<Slot> = Vec::new();
    let mut top: BTreeMap<String, (i32, i32, i32)> = BTreeMap::new();
    for (i, p) in d.parts.iter().enumerate() {
        let a = p.size.unwrap_or(400).clamp(8, 4_000_000) as f32;
        let side = a.sqrt().max(2.0);
        let (w, h, dp) = (side.round() as i32, (side * 0.8).round().max(2.0) as i32,
                          side.round() as i32);
        // stacked on the first thing it names, or set beside its
        // neighbours when it names nothing
        let at = match p.depends.first().and_then(|dep| top.get(dep.as_str())) {
            Some((bx, by, bz)) => (*bx, *by, *bz),
            None => {
                let ring = i as f32 / n as f32 * std::f32::consts::TAU;
                let r = (side * 1.2) as i32;
                (anchor.0 + (r as f32 * ring.cos()) as i32, anchor.1,
                 anchor.2 + (r as f32 * ring.sin()) as i32)
            }
        };
        top.insert(p.name.clone(), (at.0, at.1 + h, at.2));
        slots.push(Slot { at, size: (w, h, dp) });
    }
    // one program that draws them all: cheap, and it goes through the
    // same pipeline so the same measures apply
    let mut src = format!("# blockout of {}\npass blockout add\n", d.summary);
    for (i, (p, s)) in d.parts.iter().zip(slots.iter()).enumerate() {
        // HOLLOW. A blockout is a massing study: it exists to be seen
        // in silhouette and thrown away, and a solid one costs 37,000
        // cells where a shell costs 6,000 and looks identical from
        // outside. Filling it is paying for volume nobody will ever
        // look at.
        src.push_str(&format!("  part b{i} box w={} h={} d={} material=stone hollow\n",
                              s.size.0, s.size.1, s.size.2));
        src.push_str(&format!("  rel b{i} at ground dx={} dy={} dz={}\n",
                              s.at.0 - anchor.0, s.at.1 - anchor.1 + s.size.1 / 2, s.at.2 - anchor.2));
        let _ = p;
    }
    src.push_str("end\n");
    (slots, src)
}

/// What a blockout is FOR: a judgement, before anything is detailed.
///
/// The questions are the ones a builder asks standing back from a
/// blockout, and they are answerable from the crude version -- which
/// is the whole point of making one.
pub fn judge_prompt(d: &Dossier, elevation: &str, measures: &str) -> String {
    format!("This is a BLOCKOUT of: {}
Every part is one crude box. Nothing is detailed yet, and none of it is meant to look
finished -- that is what a blockout is for.

{elevation}
{measures}

Parts, in order:
{}

Stand back and judge the WHOLE, not the parts. Answer only these:

  reads yes|no          does the silhouette read as the thing asked for?
  wrong NAME            a part that is the wrong SIZE relative to the rest
  size NAME N           what its size should be instead
  move NAME above|beside|inside OTHER
  add NAME <what it is>       a part the whole plainly needs
  drop NAME
  go                    the massing is right; start detailing

Fixing a proportion here costs one line. Fixing it after everything is detailed costs the
whole build.", d.goal,
        d.parts.iter().enumerate()
            .map(|(i, p)| format!("  {}. {} ({} blocks)", i + 1, p.name, p.size.unwrap_or(400)))
            .collect::<Vec<_>>().join("\n"))
}

/// A verdict on a blockout.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Verdict {
    pub reads: Option<bool>,
    pub go: bool,
    /// (part, new size)
    pub resize: Vec<(String, i32)>,
    /// (part, relation, other)
    pub move_to: Vec<(String, String, String)>,
    pub add: Vec<(String, String)>,
    pub drop: Vec<String>,
    pub ignored: usize,
}

impl Verdict {
    pub fn is_empty(&self) -> bool {
        !self.go && self.resize.is_empty() && self.move_to.is_empty()
            && self.add.is_empty() && self.drop.is_empty()
    }
    pub fn summary(&self) -> String {
        if self.go { return "go: the massing is right".into(); }
        let mut v = Vec::new();
        if !self.resize.is_empty() { v.push(format!("{} resize(s)", self.resize.len())); }
        if !self.move_to.is_empty() { v.push(format!("{} move(s)", self.move_to.len())); }
        if !self.add.is_empty() { v.push(format!("{} addition(s)", self.add.len())); }
        if !self.drop.is_empty() { v.push(format!("{} removal(s)", self.drop.len())); }
        if v.is_empty() { "nothing said".into() } else { v.join(", ") }
    }
}

pub fn parse_verdict(reply: &str) -> Verdict {
    let mut v = Verdict::default();
    for line in reply.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') { continue; }
        let w: Vec<&str> = t.split_whitespace().collect();
        match w.first().map(|x| x.to_lowercase()).as_deref() {
            Some("go") | Some("done") | Some("ok") => v.go = true,
            Some("reads") => v.reads = w.get(1).map(|x| {
                let l = x.to_lowercase();
                l.starts_with('y') || l == "true"
            }),
            Some("size") => match (w.get(1), w.last().and_then(|x| x.parse::<i32>().ok())) {
                (Some(_), Some(sz)) if w.len() >= 3 =>
                    v.resize.push((w[1..w.len() - 1].join(" ").to_lowercase(), sz.clamp(4, 4_000_000))),
                _ => v.ignored += 1,
            },
            Some("move") => {
                // move NAME above OTHER
                let rel = w.iter().position(|x| ["above", "beside", "inside", "under", "on"]
                    .contains(&x.to_lowercase().as_str()));
                match rel {
                    Some(i) if i > 1 && i + 1 < w.len() =>
                        v.move_to.push((w[1..i].join(" ").to_lowercase(),
                                        w[i].to_lowercase(), w[i + 1..].join(" ").to_lowercase())),
                    _ => v.ignored += 1,
                }
            }
            Some("add") if w.len() >= 3 =>
                v.add.push((w[1].to_lowercase(), w[2..].join(" "))),
            Some("drop") | Some("remove") if w.len() >= 2 =>
                v.drop.push(w[1..].join(" ").to_lowercase()),
            Some("wrong") => { /* named without a fix; the resize line carries it */ }
            _ => v.ignored += 1,
        }
    }
    v
}

impl Dossier {
    /// Apply a blockout verdict. Proportions are fixed HERE, where a
    /// change costs one line, rather than after everything is built.
    pub fn revise(&mut self, v: &Verdict) -> Vec<String> {
        let mut log = Vec::new();
        for (name, sz) in &v.resize {
            match self.parts.iter_mut().find(|p| &p.name == name) {
                Some(p) => { log.push(format!("{name}: {} -> {sz}", p.size.unwrap_or(400)));
                             p.size = Some(*sz); }
                None => log.push(format!("no part called '{name}' to resize")),
            }
        }
        for (name, rel, other) in &v.move_to {
            let known = self.parts.iter().any(|p| &p.name == other);
            match self.parts.iter_mut().find(|p| &p.name == name) {
                Some(p) if known => { p.depends = vec![other.clone()];
                                      log.push(format!("{name} now sits {rel} {other}")); }
                Some(_) => log.push(format!("cannot move '{name}': no part called '{other}'")),
                None => log.push(format!("no part called '{name}' to move")),
            }
        }
        for (name, what) in &v.add {
            if self.parts.iter().any(|p| &p.name == name) { continue; }
            self.parts.push(Part { name: name.clone(), what: what.clone(), size: None,
                                   depends: Vec::new(), state: State::Waiting,
                                   outcome: String::new() });
            log.push(format!("added {name}"));
        }
        for name in &v.drop {
            let before = self.parts.len();
            self.parts.retain(|p| &p.name != name);
            // and nothing may depend on what is gone
            for p in self.parts.iter_mut() { p.depends.retain(|x| x != name); }
            if self.parts.len() < before { log.push(format!("dropped {name}")); }
        }
        log
    }
}

/// The prompt that produces a dossier. One planning turn, and the
/// only one that sees the whole.
pub fn plan_prompt(request: &str, site: &str) -> String {
    format!("\
A player asked for: {request}

{site}

Break this into PARTS that can each be built on their own. You are writing a plan, not
building anything: no shapes, no coordinates.

  is <one line: what the whole thing is>
  part NAME [size=N] [after=OTHER,OTHER]
    <what this part is, in one or two lines -- enough that someone who
     cannot see the rest of the plan could build it>
  end

Between three and eight parts. Each will be built in its OWN turn, by someone who sees
only the line above, that part, and the results of the parts it lists in `after`. So:

  * name in `after` only what your part must physically MEET -- a roof needs its walls,
    a bridge needs both banks. Most parts need nothing.
  * write each part so it stands alone. \"the same as the other tower\" is useless to
    someone who cannot see the other tower.
  * order them so a part comes after what it sits on.

EXAMPLE
  player: a windmill on the hill
is a tower carrying sails, with the machinery below and a store at the base
part base size=400
  A round stone base, wide and low, with a door facing the path. It carries everything
  above it, so it is the widest thing here.
end
part tower size=900 after=base
  A tapering stone tower rising from the base, narrowing as it goes, with small windows
  spiralling up one side.
end
part cap size=200 after=tower
  A domed timber cap on top of the tower, wider than the tower's crown so rain runs
  clear of the walls.
end
part sails size=300 after=cap
  Four long timber sails, thin and latticed, radiating from the front of the cap.
end

Write the plan only.")
}

/// Read a plan into a dossier.
pub fn parse(src: &str) -> (Dossier, Vec<String>) {
    let mut d = Dossier::default();
    let mut errs = Vec::new();
    let mut cur: Option<Part> = None;
    let mut body = String::new();
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() && cur.is_none() { continue; }
        if t.starts_with('#') { continue; }
        let w: Vec<&str> = t.split_whitespace().collect();
        let head = w.first().map(|v| v.to_lowercase()).unwrap_or_default();
        if head == "end" && cur.is_some() {
            let mut p = cur.take().unwrap();
            p.what = body.trim().to_string();
            if p.what.is_empty() { errs.push(format!("part '{}' says nothing about itself", p.name)); }
            body.clear();
            d.parts.push(p);
            continue;
        }
        if head == "part" {
            if let Some(mut p) = cur.take() {
                // a part that never closed: keep it rather than losing it
                p.what = body.trim().to_string();
                body.clear();
                errs.push(format!("part '{}' was not closed with `end`", p.name));
                d.parts.push(p);
            }
            let kv_at = w.iter().position(|s| s.contains('=')).unwrap_or(w.len());
            if kv_at <= 1 { errs.push(format!("part needs a name: {t:?}")); continue; }
            let kv = |k: &str| w[kv_at..].iter().find_map(|s| s.strip_prefix(&format!("{k}=")));
            cur = Some(Part {
                name: w[1..kv_at].join(" ").to_lowercase(),
                what: String::new(),
                size: kv("size").and_then(|v| v.parse().ok()),
                depends: kv("after").or_else(|| kv("needs")).map(|v| v.split(',')
                    .map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()).collect())
                    .unwrap_or_default(),
                state: State::Waiting,
                outcome: String::new(),
            });
            continue;
        }
        if head == "is" || head == "summary" {
            d.summary = w[1..].join(" ");
            continue;
        }
        if cur.is_some() { body.push_str(t); body.push('\n'); } else { d.notes.push(t.to_string()); }
    }
    if let Some(mut p) = cur {
        p.what = body.trim().to_string();
        errs.push(format!("part '{}' was not closed with `end`", p.name));
        d.parts.push(p);
    }
    // a dependency on a part that does not exist would silently never
    // resolve, so the part would build against nothing forever
    let names: Vec<&str> = d.parts.iter().map(|p| p.name.as_str()).collect();
    for p in &d.parts {
        for dep in &p.depends {
            if !names.contains(&dep.as_str()) {
                errs.push(format!("'{}' waits on '{dep}', which is not a part", p.name));
            }
        }
    }
    if d.summary.is_empty() && !d.parts.is_empty() {
        d.summary = d.parts.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(" + ");
        errs.push("no `is` line; used the part names".into());
    }
    (d, errs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windmill() -> Dossier {
        parse("is a tower carrying sails, with the machinery below\n\
               part base size=400\n  A round stone base, wide and low.\nend\n\
               part tower size=900 after=base\n  A tapering stone tower rising from it.\nend\n\
               part cap size=200 after=tower\n  A domed timber cap, wider than the crown.\nend\n\
               part sails size=300 after=cap\n  Four long latticed sails.\nend\n").0
    }

    #[test]
    fn a_parts_prompt_does_not_grow_with_the_build() {
        // context GREW with the build, so the twentieth pass was
        // planned with less room than the first, and the model re-read
        // its own earlier turns including the wrong ones
        let mut d = windmill();
        let first = d.part_prompt(0, "VOCAB").expect("a prompt").len();
        // build everything up to the last part
        for i in 0..3 { d.finish(i, 1000 + i, "a stone mass"); }
        let last = d.part_prompt(3, "VOCAB").expect("a prompt").len();
        assert!((last as i32 - first as i32).abs() < 400,
                "the last prompt is about the size of the first: {first} vs {last}");
        // and it carries ONLY what that part said it needs
        let p = d.part_prompt(3, "VOCAB").unwrap();
        assert!(p.contains("cap:"), "its dependency is there: {p}");
        assert!(!p.contains("A round stone base"), "the base's brief is not: {p}");
        assert!(!p.contains("tower:"), "nor a part it did not name");
    }

    #[test]
    fn every_part_sees_the_goal_and_what_the_whole_thing_is() {
        // a part built without knowing the whole is a part of nothing
        let d = windmill();
        for i in 0..d.parts.len() {
            let p = d.part_prompt(i, "V").unwrap();
            assert!(p.contains("BUILDING:"), "part {i}");
            assert!(p.contains("tower carrying sails"), "part {i} knows what it is part of");
            assert!(p.contains("YOUR PART, and only this one"), "part {i}");
        }
    }

    #[test]
    fn a_part_whose_dependency_is_not_built_yet_is_told_so() {
        // a part built before what it leans on will not line up, and
        // guessing where the other thing is, is worse than knowing it
        // is absent
        let d = windmill();
        let p = d.part_prompt(3, "V").unwrap();
        assert!(p.contains("NOT BUILT YET: cap"), "{p}");
        assert!(p.contains("do not assume where it is"));
        // once it is built, it is carried instead
        let mut d2 = d.clone();
        d2.finish(2, 220, "a dome 14 across at y=61");
        let q = d2.part_prompt(3, "V").unwrap();
        assert!(q.contains("ALREADY BUILT") && q.contains("dome 14 across"), "{q}");
        assert!(!q.contains("NOT BUILT YET"));
    }

    #[test]
    fn a_failed_part_is_re_run_alone_with_what_went_wrong() {
        // a section could not be re-run without re-running everything
        // before it, because the prompt was the conversation
        let mut d = windmill();
        d.finish(0, 0, "the model wrote prose");
        assert_eq!(d.parts[0].state, State::Failed);
        assert_eq!(d.next(), Some(1), "the run carries on past a failure");
        let r = d.retry_prompt(0, "V", "the model wrote prose").unwrap();
        assert!(r.contains("YOUR LAST ATTEMPT AT THIS PART FAILED"), "{r}");
        assert!(r.contains("not the same one again"));
        // and it is otherwise the SAME prompt, which it can be because
        // it never depended on the conversation
        let base = d.part_prompt(0, "V").unwrap();
        assert!(r.starts_with(&base), "the retry is the original plus the reason");
    }

    #[test]
    fn the_status_line_shows_the_whole_run_at_a_glance() {
        let mut d = windmill();
        assert_eq!(d.tally(), (0, 0, 4));
        d.finish(0, 500, "done");
        d.finish(1, 0, "nothing");
        d.parts[2].state = State::Skipped;
        assert_eq!(d.tally(), (1, 1, 2));
        let s = d.status();
        assert!(s.contains("1 done, 1 failed"), "{s}");
        assert!(s.contains("[x!-.]"), "one mark per part, in order: {s}");
    }

    #[test]
    fn a_plan_that_names_a_part_that_does_not_exist_says_so() {
        // it would silently never resolve, so the part would build
        // against nothing forever
        let (_, errs) = parse("is a thing\npart a after=nosuch\n  A part.\nend\n");
        assert!(errs.iter().any(|e| e.contains("waits on 'nosuch'")), "{errs:?}");
        // an unclosed part is kept, not lost
        let (d, e2) = parse("is a thing\npart a\n  A part.\npart b\n  Another.\nend\n");
        assert_eq!(d.parts.len(), 2, "both survive");
        assert!(e2.iter().any(|x| x.contains("not closed")), "{e2:?}");
        assert_eq!(d.parts[0].what, "A part.");
        // and a part with no description is a part nobody can build
        let (_, e3) = parse("part a\nend\n");
        assert!(e3.iter().any(|x| x.contains("says nothing about itself")), "{e3:?}");
        // no `is` line still yields a usable summary
        let (d4, e4) = parse("part a\n  x\nend\npart b\n  y\nend\n");
        assert_eq!(d4.summary, "a + b");
        assert!(e4.iter().any(|x| x.contains("no `is` line")));
    }

    #[test]
    fn the_plan_prompt_asks_for_parts_that_stand_alone() {
        let p = plan_prompt("a windmill on the hill", "SITE: a grassy hill");
        assert!(p.contains("built in its OWN turn"), "{p}");
        assert!(p.contains("stands alone"), "the constraint is stated");
        assert!(p.contains("useless to"), "and why it matters");
        assert!(p.contains("no shapes, no coordinates"), "planning is not building");
        assert!(p.contains("SITE: a grassy hill"));
        // and its own example parses into a runnable dossier
        let body: String = p.split("EXAMPLE").nth(1).unwrap().lines().skip(2)
            .take_while(|l| !l.trim().is_empty() || true)
            .take_while(|l| !l.starts_with("Write")).map(|l| format!("{l}\n")).collect();
        let (d, errs) = parse(&body);
        assert!(errs.is_empty(), "{body}\n{errs:?}");
        assert_eq!(d.parts.len(), 4);
        assert!(d.summary.contains("carrying sails"));
        assert_eq!(d.parts[1].depends, vec!["base"]);
        // every part is described well enough to build alone
        for part in &d.parts {
            assert!(part.what.len() > 30, "'{}' is too thin to build from: {:?}",
                    part.name, part.what);
        }
    }

    #[test]
    fn a_blockout_is_the_whole_thing_cheaply_before_anything_is_detailed() {
        // Everything went straight to full resolution, so a wrong
        // proportion cost a full build to discover -- and the review,
        // which judges what was MADE, could only find it afterwards.
        let d = windmill();
        let (slots, src) = blockout(&d, (0, 64, 0));
        assert_eq!(slots.len(), 4);
        let (prog, errs) = crate::program::parse(&src);
        assert!(errs.is_empty(), "{src}\n{errs:?}");
        let r = crate::program::run(&prog, (0, 64, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        // CHEAP: a blockout of a 1800-block plan is a few thousand
        // cells, not the hundred thousand the real build would be
        assert!(r.changes.len() < 30_000, "a blockout is cheap: {} cells", r.changes.len());
        assert!(r.changes.len() > 500, "but it is something to look at");
        // parts stack on what they depend on, which is what `after`
        // usually means physically
        let base = slots[0];
        let tower = slots[1];
        assert_eq!((tower.at.0, tower.at.2), (base.at.0, base.at.2), "the tower is on the base");
        assert!(tower.at.1 > base.at.1, "and above it");
        // and the sails end up highest, being last in the chain
        assert!(slots[3].at.1 > slots[1].at.1);
    }

    #[test]
    fn a_proportion_is_fixed_in_the_plan_where_it_costs_one_line() {
        let mut d = windmill();
        let before = crate::program::run(&crate::program::parse(&blockout(&d, (0, 64, 0)).1).0,
                                         (0, 64, 0)).changes.len();
        let v = parse_verdict("reads no\n                               wrong sails\n                               size sails 1400\n                               size base 200\n                               move cap above tower\n                               add gallery a railed walkway around the tower's waist\n                               drop store\n");
        assert_eq!(v.reads, Some(false));
        assert!(!v.go);
        assert_eq!(v.resize.len(), 2);
        assert_eq!(v.move_to.len(), 1);
        assert_eq!(v.add.len(), 1);
        let log = d.revise(&v);
        assert_eq!(d.parts.iter().find(|p| p.name == "sails").unwrap().size, Some(1400));
        assert_eq!(d.parts.iter().find(|p| p.name == "base").unwrap().size, Some(200));
        assert!(d.parts.iter().any(|p| p.name == "gallery"), "{log:?}");
        // dropping something nobody has is said, not silently ignored
        assert!(log.iter().any(|l| l.contains("no part called")) || !log.is_empty());
        // and the blockout really changed
        let after = crate::program::run(&crate::program::parse(&blockout(&d, (0, 64, 0)).1).0,
                                        (0, 64, 0)).changes.len();
        assert_ne!(before, after, "the massing moved");
    }

    #[test]
    fn dropping_a_part_takes_it_out_of_everything_that_leaned_on_it() {
        // a dependency on a part that is gone would never resolve, and
        // the part would wait for it forever
        let mut d = windmill();
        d.revise(&parse_verdict("drop tower\n"));
        assert!(!d.parts.iter().any(|p| p.name == "tower"));
        assert!(d.parts.iter().all(|p| !p.depends.contains(&"tower".to_string())),
                "{:?}", d.parts.iter().map(|p| &p.depends).collect::<Vec<_>>());
    }

    #[test]
    fn go_means_go_and_a_silent_reply_does_not() {
        assert!(parse_verdict("go\n").go);
        assert!(parse_verdict("reads yes\ngo\n").go);
        let quiet = parse_verdict("Looks pretty good to me!\n");
        assert!(!quiet.go, "prose is not consent to build");
        assert!(quiet.is_empty());
        assert!(quiet.ignored > 0, "and it says it understood nothing");
        assert_eq!(parse_verdict("go").summary(), "go: the massing is right");
    }

    #[test]
    fn the_judge_prompt_asks_what_a_blockout_can_actually_answer() {
        let d = windmill();
        let p = judge_prompt(&d, "FRONT:\n ####\n", "1800 cells, 40w x 30h");
        assert!(p.contains("Stand back and judge the WHOLE"), "{p}");
        assert!(p.contains("Nothing is detailed yet"), "it says what it is looking at");
        assert!(p.contains("costs one line"), "and why now: {p}");
        assert!(p.contains("1. base") && p.contains("4. sails"), "the parts, in order");
        assert!(p.contains("reads yes|no"));
        // it does not ask for anything a blockout cannot show
        assert!(!p.to_lowercase().contains("material"), "no material questions: {p}");
        // it says there is no detail, and asks for none
        assert!(p.contains("Nothing is detailed yet") && p.contains("start detailing"),
                "detail is mentioned only as what comes AFTER: {p}");
    }
}
