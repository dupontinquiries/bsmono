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
}
