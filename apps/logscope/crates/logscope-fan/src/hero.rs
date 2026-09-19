//! hero — finite-state workers: a worker whose brain is a machine, not a model.
//!
//! ## Why
//!
//! A model worker pays a full generate → parse → execute → observe cycle for every step, and on
//! the hardware this project targets a cycle is seconds to minutes. A large share of those steps
//! carry no judgement at all: the compiler said `--> src/ui.rs:88:9`, so the next turn is "read
//! src/ui.rs around line 88". A test named `tests::parse::empty_input` failed, so the next turn is
//! "find `fn empty_input`". The model is being used as a very slow `if`.
//!
//! A [`Hero`] does that step with no model in the loop. It is a worker in every sense the roster
//! means — it has a [`WorkerSpec`], a tool whitelist, a lane — and it runs through the same tool
//! funnel, so everything that confines a bee confines a hero. What differs is the brain.
//!
//! ## What "more advanced than bot-arena's" means, precisely
//!
//! `apps/bot-arena/src/fsm.rs` heroes are GENERATIVE: a state emits a character or a word, edges
//! are a weighted RANDOM walk, memory is one register, and nothing is ever read. That is the right
//! genome for evolving command strings. A worker needs the opposite on each axis:
//!
//! | axis        | bot-arena `Fsm`            | `hero::Machine`                                   |
//! |-------------|----------------------------|---------------------------------------------------|
//! | a state     | emits text                 | performs an [`Action`] — a TOOL CALL, a report line |
//! | an edge     | weight, sampled            | a [`Guard`] on what came back; first match wins   |
//! | input       | none                       | every observation, plus the one that woke it      |
//! | memory      | one register               | named slots, filled by [`Pattern`] captures       |
//! | loops       | until `max_steps`          | bounded per state ([`Guard::VisitsOver`])         |
//! | composition | `concat`                   | [`Action::Sub`] — a hero calls a hero, depth-bound |
//! | outcome     | a string                   | [`End`]: done / failed / YIELD-with-a-reason      |
//! | determinism | seeded RNG                 | none needed: same observations, same run          |
//!
//! [`Machine::from_weighted_graph`] imports an arena machine as the degenerate case (no guards,
//! no memory), so heroes evolved there can be enlisted here; native heroes are strictly richer.
//!
//! ## Context-gated activation
//!
//! A hero with a [`Gate`] is a REFLEX. After a model worker's turn the runtime shows the turn's
//! observation to the [`Stable`]; the first hero whose gate matches — and whose tools the current
//! worker's whitelist permits — runs, and its report is appended to what the model sees next. The
//! model's following turn starts with the evidence instead of asking for it. A gate that does not
//! match costs a string scan. Nothing here can make a run slower by more than that.
//!
//! ## The seam
//!
//! [`ToolPort`] is the only way a machine touches the world. Here it is a trait with a scripted
//! implementation for tests; in `logscope-egui` it is `Workspace::apply`. Engine, then transport.
//!
//! Pure: no I/O, no clock, no RNG, no model.

use std::collections::BTreeMap;

use crate::queen::{SpecError, WorkerSpec};

// ───────────────────────────── the world, as a machine sees it ─────────────────────────────

/// What a tool call returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Obs { pub ok: bool, pub text: String }

/// The one way a machine acts. `args` are already expanded — no templates reach the port.
pub trait ToolPort {
    fn call(&mut self, tool: &str, args: &[(String, String)]) -> Obs;
}

// ───────────────────────────────────── patterns ─────────────────────────────────────

/// What a hole may swallow. A hole takes the characters its class allows and STOPS WHERE THE NEXT
/// LITERAL BEGINS — so `TODO({who:word})` captures `ada` from `TODO(ada)`, not `ada)`. (The first
/// version was purely greedy: `word` swallowed the `)` and the pattern could never match. The seed
/// heroes never noticed, because their classes happen to exclude their delimiters; the first hero
/// enlisted from text did.) A hole at the end of a pattern takes everything its class allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Non-whitespace.
    Word,
    /// ASCII digits.
    Int,
    /// `[A-Za-z0-9_./+-]` — a path without spaces. Stops at `:`, which is what makes
    /// `{file:path}:{line:int}` work on `src/ui.rs:88:9`.
    Path,
    /// A [`Class::Path`] that also contains a `/` and ends in `.ext` — i.e. looks like a FILE in a
    /// tree, not a word that happens to be path-legal. For pulling paths out of task prose.
    File,
    /// To the end of the line.
    Rest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Seg { Lit(String), Hole(String, Class) }

/// `--> {file:path}:{line:int}:` — literals and named, classed holes. Deliberately not a regex:
/// this crate has no regex dependency, heroes are meant to be read by the operator, and the
/// observations being matched are compiler and test-runner lines, not free text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern { segs: Vec<Seg>, src: String }

impl Pattern {
    pub fn parse(src: &str) -> Result<Pattern, HeroError> {
        let bad = |why: &str| HeroError::BadPattern { pattern: src.to_string(), why: why.to_string() };
        let mut segs = Vec::new();
        let mut lit = String::new();
        let mut it = src.chars().peekable();
        while let Some(c) = it.next() {
            if c != '{' { lit.push(c); continue; }
            if it.peek() == Some(&'{') { it.next(); lit.push('{'); continue; } // `{{` is a literal brace
            if !lit.is_empty() { segs.push(Seg::Lit(std::mem::take(&mut lit))); }
            let mut body = String::new();
            loop {
                match it.next() { Some('}') => break, Some(ch) => body.push(ch), None => return Err(bad("unclosed `{`")) }
            }
            let (name, class) = match body.split_once(':') {
                None => (body.as_str(), Class::Word),
                Some((n, "word")) => (n, Class::Word), Some((n, "int")) => (n, Class::Int),
                Some((n, "path")) => (n, Class::Path), Some((n, "file")) => (n, Class::File),
                Some((n, "rest")) => (n, Class::Rest),
                Some((_, other)) => return Err(bad(&format!("unknown class `{other}` (word, int, path, file, rest)"))),
            };
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(bad("a hole needs a name: letters, digits, `_`"));
            }
            if matches!(segs.last(), Some(Seg::Hole(..))) { return Err(bad("two holes with no literal between them cannot be told apart")); }
            segs.push(Seg::Hole(name.to_string(), class));
        }
        if !lit.is_empty() { segs.push(Seg::Lit(lit)); }
        if !segs.iter().any(|s| matches!(s, Seg::Hole(..))) { return Err(bad("no hole — use Guard::Contains for a plain literal")); }
        Ok(Pattern { segs, src: src.to_string() })
    }

    pub fn source(&self) -> &str { &self.src }

    /// First match at or after byte `from`. Returns the captures and the byte just past the match,
    /// so a caller can iterate ("the next error site") by feeding the end back in.
    pub fn find(&self, text: &str, from: usize) -> Option<(Vec<(String, String)>, usize)> {
        let lit_first = matches!(self.segs.first(), Some(Seg::Lit(_)));
        let mut start = from.min(text.len());
        while start <= text.len() {
            if !text.is_char_boundary(start) { start += 1; continue; }
            if lit_first {
                // jump straight to the next occurrence of the leading literal
                let Seg::Lit(l) = &self.segs[0] else { unreachable!() };
                match text[start..].find(l.as_str()) { Some(k) => start += k, None => return None }
            } else if start > 0 {
                // a hole-first pattern may only begin where a token could: line start or after space
                let prev = text[..start].chars().next_back().unwrap();
                if !prev.is_whitespace() { start += 1; continue; }
            }
            if let Some(hit) = self.match_at(text, start) { return Some(hit); }
            start += 1;
        }
        None
    }

    fn match_at(&self, text: &str, start: usize) -> Option<(Vec<(String, String)>, usize)> {
        let mut at = start;
        let mut caps = Vec::new();
        for (si, seg) in self.segs.iter().enumerate() {
            match seg {
                Seg::Lit(l) => { if !text[at..].starts_with(l.as_str()) { return None; } at += l.len(); }
                Seg::Hole(name, class) => {
                    let rest = &text[at..];
                    let allowed = |c: char| match class {
                        Class::Word => !c.is_whitespace(),
                        Class::Int => c.is_ascii_digit(),
                        Class::Path | Class::File => c.is_ascii_alphanumeric() || "_./+-".contains(c),
                        Class::Rest => c != '\n' && c != '\r',
                    };
                    // stop early where the following literal begins (a hole is never empty, so the
                    // literal may not claim the hole's first character)
                    let stop_at = match self.segs.get(si + 1) { Some(Seg::Lit(l)) => Some(l.as_str()), _ => None };
                    let mut len = 0usize;
                    for c in rest.chars() {
                        if !allowed(c) { break; }
                        if len > 0 && stop_at.is_some_and(|l| rest[len..].starts_with(l)) { break; }
                        len += c.len_utf8();
                    }
                    if len == 0 { return None; }
                    let got = &rest[..len];
                    if *class == Class::File {
                        let ext_ok = got.rsplit_once('.').is_some_and(|(stem, e)|
                            !stem.is_empty() && (1..=5).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()));
                        if !got.contains('/') || !ext_ok || got.contains("..") { return None; }
                    }
                    caps.push((name.clone(), got.to_string()));
                    at += len;
                }
            }
        }
        Some((caps, at))
    }
}

// ───────────────────────────────────── the machine ─────────────────────────────────────

pub type StateId = usize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Call a tool. Argument values are templates (see [`expand`]).
    Call { tool: String, args: Vec<(String, String)> },
    /// Append a line to the report the hero hands back. A template.
    Emit(String),
    /// Set a slot from a template.
    Set { slot: String, value: String },
    /// Run another hero's machine to completion, sharing slots. Depth-bounded by [`Limits`].
    Sub(String),
    /// Do nothing; the state exists for its edges.
    Nop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Guard {
    Always,
    /// The last call succeeded / failed. Before any call: the waking observation's status.
    Ok, Failed,
    /// The CURRENT observation (last call's output) contains this expanded template.
    Contains(String),
    /// Match the current observation from its cursor; captures fill slots; the cursor advances, so
    /// the same guard on a later visit finds the NEXT occurrence.
    Matches(Pattern),
    /// The same, against the observation that WOKE the hero — which stays available, with its own
    /// cursor, after calls have replaced the current one. This is how a hero walks a list of error
    /// sites while reading a file for each.
    WakeMatches(Pattern),
    SlotSet(String),
    SlotEq(String, String),
    /// This state has been entered more than `n` times. The loop bound.
    VisitsOver(u32),
    Not(Box<Guard>), All(Vec<Guard>), Any(Vec<Guard>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome { Succeeded, Failed }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    To(StateId),
    Done(Outcome),
    /// Hand back to whoever called, saying why. Not a failure: "this needs judgement" is the
    /// correct output of a machine that has reached the edge of what it is for.
    Yield(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge { pub when: Guard, pub next: Next }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State { pub name: String, pub action: Action, pub edges: Vec<Edge> }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Machine { pub states: Vec<State>, pub start: StateId }

pub const MAX_STATES: usize = 64;

/// Hard bounds on one run. A hero that hits one stops and says which.
#[derive(Clone, Copy, Debug)]
pub struct Limits { pub max_steps: u32, pub max_calls: u32, pub max_report_bytes: usize, pub max_depth: u8, pub max_obs_bytes: usize }

impl Default for Limits {
    fn default() -> Self { Limits { max_steps: 64, max_calls: 8, max_report_bytes: 6000, max_depth: 3, max_obs_bytes: 64 * 1024 } }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    Done(Outcome),
    Yield(String),
    /// No edge of `state` matched. A machine bug, reported as one rather than looped on.
    Stuck { state: String },
    StepLimit, CallLimit,
}

/// One step, for the operator. `did` is the action with its templates expanded — what actually ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step { pub state: String, pub did: String, pub ok: Option<bool>, pub took: String }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trace {
    pub steps: Vec<Step>,
    pub calls: u32,
    pub report: String,
    pub end: End,
    pub slots: BTreeMap<String, String>,
}

impl Trace {
    pub fn succeeded(&self) -> bool { self.end == End::Done(Outcome::Succeeded) }
    /// `locate: read_range src/ui.rs 80 96 [ok] → report` — one line per step.
    pub fn render(&self) -> String {
        let mut s = String::new();
        for st in &self.steps {
            let ok = match st.ok { Some(true) => " [ok]", Some(false) => " [failed]", None => "" };
            s.push_str(&format!("{}: {}{ok} → {}\n", st.state, st.did, st.took));
        }
        s.push_str(&format!("end: {:?} after {} call(s)", self.end, self.calls));
        s
    }
}

/// Everything a run carries. Separate from [`Machine`] so a machine is immutable data.
struct Run<'a> {
    task: &'a str,
    wake: &'a str, wake_cursor: usize,
    cur: String, cur_ok: bool, cur_cursor: usize,
    slots: BTreeMap<String, String>,
    calls: u32, steps: u32,
    report: String,
    trail: Vec<Step>,
    limits: Limits,
}

/// Expand `${slot}`, `${slot+N}`, `${slot-N}` (integers; floors at 1, because every integer a hero
/// handles is a line number), `${slot:last}` (after the final `::` — a test path's function name),
/// `${obs}` and `${task}`. An unset slot is an ERROR, not an empty string: a `read_range` with an
/// empty path is a tool call nobody meant.
fn expand(tpl: &str, run: &Run) -> Result<String, String> {
    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let Some(j) = after.find('}') else { return Err(format!("unclosed `${{` in template {tpl:?}")) };
        let expr = &after[..j];
        rest = &after[j + 1..];
        if expr == "obs" { out.push_str(&run.cur); continue; }
        if expr == "task" { out.push_str(run.task); continue; }
        let (name, op) = match expr.find(['+', '-', ':']) { Some(k) => (&expr[..k], &expr[k..]), None => (expr, "") };
        let Some(val) = run.slots.get(name) else {
            return Err(format!("the template needs ${{{name}}}, which no guard has captured"));
        };
        if op.is_empty() { out.push_str(val); }
        else if op == ":last" { out.push_str(val.rsplit("::").next().unwrap_or(val)); }
        else {
            let n: i64 = val.parse().map_err(|_| format!("${{{expr}}}: {val:?} is not an integer"))?;
            let d: i64 = op[1..].parse().map_err(|_| format!("${{{expr}}}: bad offset"))?;
            let r = if op.starts_with('+') { n.saturating_add(d) } else { n.saturating_sub(d) };
            out.push_str(&r.max(1).to_string());
        }
    }
    out.push_str(rest);
    Ok(out)
}

fn eval(g: &Guard, run: &mut Run, visits: u32) -> Result<bool, String> {
    Ok(match g {
        Guard::Always => true,
        Guard::Ok => run.cur_ok,
        Guard::Failed => !run.cur_ok,
        Guard::Contains(t) => { let needle = expand(t, run)?; run.cur.contains(&needle) }
        Guard::Matches(p) => match p.find(&run.cur, run.cur_cursor) {
            Some((caps, end)) => { run.cur_cursor = end; run.slots.extend(caps); true }
            None => false,
        },
        Guard::WakeMatches(p) => match p.find(run.wake, run.wake_cursor) {
            Some((caps, end)) => { run.wake_cursor = end; run.slots.extend(caps); true }
            None => false,
        },
        Guard::SlotSet(s) => run.slots.contains_key(s),
        Guard::SlotEq(s, v) => { let want = expand(v, run)?; run.slots.get(s).is_some_and(|x| *x == want) }
        Guard::VisitsOver(n) => visits > *n,
        Guard::Not(g) => !eval(g, run, visits)?,
        // All/Any short-circuit LEFT TO RIGHT, and captures made by a guard that matched persist
        // even if a later one fails — put the capturing guard last when that matters.
        Guard::All(gs) => { for g in gs { if !eval(g, run, visits)? { return Ok(false); } } true }
        Guard::Any(gs) => { for g in gs { if eval(g, run, visits)? { return Ok(true); } } false }
    })
}

fn clip(s: &str, max: usize) -> &str {
    if s.len() <= max { return s; }
    let mut e = max; while !s.is_char_boundary(e) { e -= 1; } &s[..e]
}

impl Machine {
    /// Run to an [`End`]. `wake` is the observation that activated the hero (the task text, for a
    /// task-start hero); `wake_ok` its status. `subs` resolves [`Action::Sub`].
    pub fn run(&self, task: &str, wake: &str, wake_ok: bool, seed_slots: BTreeMap<String, String>,
               port: &mut dyn ToolPort, subs: &dyn Fn(&str) -> Option<Machine>, limits: Limits) -> Trace {
        self.run_from(task, wake, 0, wake_ok, seed_slots, port, subs, limits)
    }

    /// [`Machine::run`] with the waking observation already consumed up to `wake_from`. A gate that
    /// MATCHED to wake the hero has read part of the observation; the machine must continue from
    /// there, or its first "next site" is the site the gate already captured.
    #[allow(clippy::too_many_arguments)]
    pub fn run_from(&self, task: &str, wake: &str, wake_from: usize, wake_ok: bool, seed_slots: BTreeMap<String, String>,
               port: &mut dyn ToolPort, subs: &dyn Fn(&str) -> Option<Machine>, limits: Limits) -> Trace {
        let mut run = Run { task, wake: clip(wake, limits.max_obs_bytes), wake_cursor: wake_from,
            cur: clip(wake, limits.max_obs_bytes).to_string(), cur_ok: wake_ok, cur_cursor: 0,
            slots: seed_slots, calls: 0, steps: 0, report: String::new(), trail: Vec::new(), limits };
        let end = self.run_in(&mut run, port, subs, 0);
        Trace { steps: run.trail, calls: run.calls, report: run.report, end, slots: run.slots }
    }

    fn run_in(&self, run: &mut Run, port: &mut dyn ToolPort, subs: &dyn Fn(&str) -> Option<Machine>, depth: u8) -> End {
        let mut visits = vec![0u32; self.states.len()];
        let mut at = self.start;
        loop {
            let Some(state) = self.states.get(at) else { return End::Stuck { state: format!("#{at} (out of range)") } };
            if run.steps >= run.limits.max_steps { return End::StepLimit; }
            run.steps += 1;
            visits[at] += 1;
            let mut ok = None;
            let did = match &state.action {
                Action::Nop => "—".to_string(),
                Action::Emit(t) => match expand(t, run) {
                    Ok(line) => {
                        let room = run.limits.max_report_bytes.saturating_sub(run.report.len());
                        if !run.report.is_empty() && room > 0 { run.report.push('\n'); }
                        let room = run.limits.max_report_bytes.saturating_sub(run.report.len());
                        run.report.push_str(clip(&line, room));
                        format!("emit {} bytes", line.len().min(room))
                    }
                    Err(why) => return End::Yield(why),
                },
                Action::Set { slot, value } => match expand(value, run) {
                    Ok(v) => { run.slots.insert(slot.clone(), v.clone()); format!("set {slot} = {v:?}") }
                    Err(why) => return End::Yield(why),
                },
                Action::Call { tool, args } => {
                    if run.calls >= run.limits.max_calls { return End::CallLimit; }
                    let mut real = Vec::with_capacity(args.len());
                    for (k, v) in args { match expand(v, run) { Ok(x) => real.push((k.clone(), x)), Err(why) => return End::Yield(why) } }
                    run.calls += 1;
                    let obs = port.call(tool, &real);
                    ok = Some(obs.ok);
                    run.cur_ok = obs.ok;
                    run.cur = clip(&obs.text, run.limits.max_obs_bytes).to_string();
                    run.cur_cursor = 0;
                    format!("{tool} {}", real.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "))
                }
                Action::Sub(name) => {
                    if depth >= run.limits.max_depth { return End::Yield(format!("sub-hero `{name}` would exceed the depth bound ({})", run.limits.max_depth)); }
                    let Some(m) = subs(name) else { return End::Yield(format!("no hero named `{name}` to call")) };
                    let end = m.run_in(run, port, subs, depth + 1);
                    match end {
                        End::Done(o) => { run.cur_ok = o == Outcome::Succeeded; ok = Some(run.cur_ok); format!("sub {name} → {o:?}") }
                        other => return other, // a yield or a limit inside a sub is the whole run's
                    }
                }
            };
            let mut taken = None;
            for (i, e) in state.edges.iter().enumerate() {
                match eval(&e.when, run, visits[at]) {
                    Ok(true) => { taken = Some(i); break; }
                    Ok(false) => {}
                    Err(why) => { run.trail.push(Step { state: state.name.clone(), did, ok, took: format!("guard error: {why}") }); return End::Yield(why); }
                }
            }
            let Some(i) = taken else {
                run.trail.push(Step { state: state.name.clone(), did, ok, took: "no edge matched".into() });
                return End::Stuck { state: state.name.clone() };
            };
            let took = match &state.edges[i].next {
                Next::To(t) => self.states.get(*t).map(|s| s.name.clone()).unwrap_or_else(|| format!("#{t}")),
                Next::Done(o) => format!("done ({o:?})"),
                Next::Yield(_) => "yield".into(),
            };
            run.trail.push(Step { state: state.name.clone(), did, ok, took });
            match &state.edges[i].next {
                Next::To(t) => at = *t,
                Next::Done(o) => return End::Done(*o),
                Next::Yield(t) => return End::Yield(expand(t, run).unwrap_or_else(|_| t.clone())),
            }
        }
    }

    /// Import a bot-arena style machine: per-state weighted out-edges and one action per state.
    /// The walk becomes DETERMINISTIC — each state goes to its heaviest target (the arena's own
    /// `run_greedy` reading) — and each state is entered at most once, which is what terminates a
    /// graph that was only ever bounded by `max_steps`. A state with no out-edges is `Done`.
    pub fn from_weighted_graph(start: usize, edges: &[Vec<(usize, f32)>], actions: &[Action]) -> Machine {
        let n = edges.len();
        let states = (0..n).map(|i| {
            let heaviest = edges[i].iter().filter(|(t, w)| *t < n && w.is_finite())
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|(t, _)| *t);
            let mut es = vec![Edge { when: Guard::VisitsOver(1), next: Next::Done(Outcome::Succeeded) }];
            es.push(match heaviest {
                Some(t) => Edge { when: Guard::Always, next: Next::To(t) },
                None => Edge { when: Guard::Always, next: Next::Done(Outcome::Succeeded) },
            });
            State { name: format!("s{i}"), action: actions.get(i % actions.len().max(1)).cloned().unwrap_or(Action::Nop), edges: es }
        }).collect();
        Machine { states, start: start.min(n.saturating_sub(1)) }
    }
}

// ───────────────────────────────────── heroes ─────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// Before the first model turn; the waking observation is the TASK TEXT.
    TaskStart,
    /// After a model worker's turn; the waking observation is what that turn's tools returned.
    AfterObservation,
}

/// When a hero may take a turn uninvited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gate { pub on: Trigger, pub when: Guard }

#[derive(Clone, Debug)]
pub struct Hero {
    /// A hero is a WORKER: name, purpose, lane, whitelist, success — validated like any other.
    pub spec: WorkerSpec,
    /// `None` = never activates by itself; it is staffed or called as a sub-hero.
    pub gate: Option<Gate>,
    pub machine: Machine,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HeroError {
    Spec(SpecError),
    BadPattern { pattern: String, why: String },
    /// A state calls a tool the hero's own spec does not whitelist.
    ToolNotWhitelisted { state: String, tool: String },
    BadEdge { state: String, target: StateId },
    Unreachable { state: String },
    /// No path from the start reaches a `Done`.
    NeverFinishes,
    /// A state's last edge is not `Always`, so an observation nobody anticipated strands it.
    NoFallthrough { state: String },
    Empty, TooManyStates(usize),
    UnknownSub { state: String, hero: String },
    Duplicate(String),
}

impl std::fmt::Display for HeroError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeroError::Spec(e) => write!(f, "{}", e.correction()),
            HeroError::BadPattern { pattern, why } => write!(f, "pattern {pattern:?}: {why}"),
            HeroError::ToolNotWhitelisted { state, tool } => write!(f, "state `{state}` calls `{tool}`, which this hero's own Tools list does not include — add it there or remove the call"),
            HeroError::BadEdge { state, target } => write!(f, "state `{state}` has an edge to state #{target}, which does not exist"),
            HeroError::Unreachable { state } => write!(f, "state `{state}` cannot be reached from the start — dead states are where stale logic hides; remove it or wire it in"),
            HeroError::NeverFinishes => write!(f, "no path from the start reaches a Done — this hero can only ever yield, stall, or hit a limit"),
            HeroError::NoFallthrough { state } => write!(f, "state `{state}`'s last edge is conditional — end every state with an Always edge (to a state, a Done, or a Yield that says why) so an unanticipated observation cannot strand it"),
            HeroError::Empty => write!(f, "the machine has no states"),
            HeroError::TooManyStates(n) => write!(f, "{n} states; the bound is {MAX_STATES} — past that it is a program, not a specialist"),
            HeroError::UnknownSub { state, hero } => write!(f, "state `{state}` calls sub-hero `{hero}`, which is not in the stable"),
            HeroError::Duplicate(n) => write!(f, "a hero named `{n}` is already in the stable"),
        }
    }
}

impl Hero {
    /// Everything that can be checked without running it.
    pub fn validate(&self, known_tools: &[&str]) -> Result<(), HeroError> {
        self.spec.validate(known_tools).map_err(HeroError::Spec)?;
        let m = &self.machine;
        if m.states.is_empty() { return Err(HeroError::Empty); }
        if m.states.len() > MAX_STATES { return Err(HeroError::TooManyStates(m.states.len())); }
        if m.start >= m.states.len() { return Err(HeroError::BadEdge { state: "(start)".into(), target: m.start }); }
        for s in &m.states {
            if let Action::Call { tool, .. } = &s.action {
                if !self.spec.tools.iter().any(|t| t == tool) { return Err(HeroError::ToolNotWhitelisted { state: s.name.clone(), tool: tool.clone() }); }
            }
            for e in &s.edges {
                if let Next::To(t) = e.next { if t >= m.states.len() { return Err(HeroError::BadEdge { state: s.name.clone(), target: t }); } }
            }
            if !matches!(s.edges.last(), Some(Edge { when: Guard::Always, .. })) { return Err(HeroError::NoFallthrough { state: s.name.clone() }); }
        }
        let mut seen = vec![false; m.states.len()];
        let mut stack = vec![m.start];
        let mut finishes = false;
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) { continue; }
            for e in &m.states[i].edges {
                match e.next { Next::To(t) => stack.push(t), Next::Done(_) => finishes = true, Next::Yield(_) => {} }
            }
        }
        if let Some(i) = seen.iter().position(|s| !s) { return Err(HeroError::Unreachable { state: m.states[i].name.clone() }); }
        if !finishes { return Err(HeroError::NeverFinishes); }
        Ok(())
    }

    /// The tools this hero's machine actually calls.
    pub fn calls(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.machine.states.iter().filter_map(|s| match &s.action { Action::Call { tool, .. } => Some(tool.as_str()), _ => None }).collect();
        v.sort(); v.dedup(); v
    }
}

// ───────────────────────────────────── the text form ─────────────────────────────────────

/// What the Queen (or the operator) is told when asked to design a hero. The format below is what
/// [`Hero::parse`] reads; the header is a `WorkerSpec`, so the same `validate` rules apply.
pub const HERO_FORMAT: &str = r#"A HERO is a worker that is a fixed procedure, not a model. Write one as:

Name: error-locator
Purpose: show the code at the compiler error the last turn produced
Tools:
- read_range
Success:
- the output contains the file and line of the first error
Gate: after-observation
  contains "error"
  matches "--> {file:path}:{line:int}:"
State read: call read_range path=${file} start=${line-8} end=${line+8}
  ok -> report
  else -> yield could not read ${file}
State report: emit the code at ${file}:${line}:\n${obs}
  not visits>1 and wake-matches "--> {file:path}:{line:int}:" -> read
  else -> done

Rules. Gate is `task-start` or `after-observation`, then one guard per indented line (all must hold);
omit Gate for a hero that is only ever called. The FIRST State is the start. An action is one of
`call TOOL key=value …` (quote a value with spaces), `emit TEXT`, `set SLOT = TEXT`, `sub HERO`, `nop`.
Each indented line under a State is `GUARD -> TARGET`; the first that holds is taken, and the LAST must
be `else`. GUARD is terms joined by ` and `: ok, failed, else, contains "T", matches "P",
wake-matches "P", set SLOT, SLOT == "T", visits>N, and `not TERM`. TARGET is a state name, `done`,
`fail`, or `yield REASON`. In a pattern P, `{name:class}` captures into ${name}; classes are word, int,
path, file, rest. In text T, ${name}, ${name+N}, ${name-N}, ${name:last}, ${obs}, ${task} expand.
Only tools listed under Tools may be called, and a hero that writes needs an Owns lane like any worker."#;

#[derive(Clone, Debug, PartialEq)]
pub struct ParseError { pub line: usize, pub why: String }
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "line {}: {}", self.line, self.why) }
}

/// Split on whitespace, keeping "double quoted" runs (with `\"` and `\\`) as one token, quotes removed.
fn tokens(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false; let mut any = false;
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match c {
            '"' => { in_q = !in_q; any = true; }
            '\\' if in_q => match it.next() { Some('n') => cur.push('\n'), Some(o) => cur.push(o), None => return Err("dangling `\\`".into()) },
            c if c.is_whitespace() && !in_q => { if any || !cur.is_empty() { out.push(std::mem::take(&mut cur)); any = false; } }
            c => cur.push(c),
        }
    }
    if in_q { return Err("unclosed `\"`".into()); }
    if any || !cur.is_empty() { out.push(cur); }
    Ok(out)
}

fn unescape(s: &str) -> String { s.replace("\\n", "\n").replace("\\t", "\t") }

fn parse_term(toks: &[String]) -> Result<Guard, String> {
    let pat = |s: &String| Pattern::parse(s).map_err(|e| e.to_string());
    match toks {
        [n, rest @ ..] if n == "not" && !rest.is_empty() => Ok(Guard::Not(Box::new(parse_term(rest)?))),
        [t] if t == "ok" => Ok(Guard::Ok),
        [t] if t == "failed" => Ok(Guard::Failed),
        [t] if t == "else" || t == "always" => Ok(Guard::Always),
        [k, v] if k == "contains" => Ok(Guard::Contains(v.clone())),
        [k, v] if k == "matches" => Ok(Guard::Matches(pat(v)?)),
        [k, v] if k == "wake-matches" => Ok(Guard::WakeMatches(pat(v)?)),
        [k, v] if k == "set" => Ok(Guard::SlotSet(v.clone())),
        [slot, eq, v] if eq == "==" => Ok(Guard::SlotEq(slot.clone(), v.clone())),
        [t] if t.starts_with("visits>") => t["visits>".len()..].parse().map(Guard::VisitsOver).map_err(|_| format!("`{t}`: visits> needs a number")),
        other => Err(format!("`{}` is not a guard — see the guard list (ok, failed, else, contains, matches, wake-matches, set, ==, visits>N, not)", other.join(" "))),
    }
}

fn parse_guard(s: &str) -> Result<Guard, String> {
    let toks = tokens(s)?;
    let mut terms = Vec::new();
    for part in toks.split(|t| t == "and") {
        if part.is_empty() { return Err("an `and` with nothing beside it".into()); }
        terms.push(parse_term(part)?);
    }
    Ok(if terms.len() == 1 { terms.pop().unwrap() } else { Guard::All(terms) })
}

impl Hero {
    /// Read [`HERO_FORMAT`]. Forgiving about layout, strict about content — and it does NOT
    /// validate: call [`Hero::validate`] (or [`Stable::add`], which does) with the runtime's tools.
    pub fn parse(text: &str) -> Result<Hero, ParseError> {
        let lines: Vec<&str> = text.lines().collect();
        let body_at = lines.iter().position(|l| { let t = l.trim_start(); t.starts_with("Gate:") || t.starts_with("State ") })
            .ok_or(ParseError { line: lines.len().max(1), why: "no `State NAME: ACTION` line — a hero needs at least one state".into() })?;
        let spec = WorkerSpec::parse(&lines[..body_at].join("\n")).map_err(|e| ParseError { line: 1, why: e.correction() })?;

        struct RawState { name: String, action: Action, edges: Vec<(Guard, String, usize)> }
        let mut gate_on: Option<Trigger> = None;
        let mut gate_terms: Vec<Guard> = Vec::new();
        let mut states: Vec<RawState> = Vec::new();
        let mut in_gate = false;
        for (i, raw) in lines.iter().enumerate().skip(body_at) {
            let ln = i + 1;
            let err = |why: String| ParseError { line: ln, why };
            let t = raw.trim();
            if t.is_empty() || t.starts_with('#') { continue; }
            if let Some(rest) = t.strip_prefix("Gate:") {
                gate_on = Some(match rest.trim() { "task-start" => Trigger::TaskStart, "after-observation" => Trigger::AfterObservation,
                    o => return Err(err(format!("`Gate: {o}` — use `task-start` or `after-observation`"))) });
                in_gate = true; continue;
            }
            if let Some(rest) = t.strip_prefix("State ") {
                in_gate = false;
                let (name, act) = rest.split_once(':').ok_or_else(|| err("`State NAME: ACTION` — the colon is missing".into()))?;
                let name = name.trim().to_string();
                if name.is_empty() || ["done", "fail", "yield"].contains(&name.as_str()) { return Err(err(format!("`{name}` cannot be a state name (done, fail and yield are targets)"))); }
                if states.iter().any(|s| s.name == name) { return Err(err(format!("state `{name}` is defined twice"))); }
                let act = act.trim();
                let (verb, arg) = act.split_once(char::is_whitespace).map(|(v, a)| (v, a.trim())).unwrap_or((act, ""));
                let action = match verb {
                    "nop" => Action::Nop,
                    "emit" => Action::Emit(unescape(arg)),
                    "sub" if !arg.is_empty() => Action::Sub(arg.to_string()),
                    "set" => { let (slot, v) = arg.split_once('=').ok_or_else(|| err("`set SLOT = TEXT`".into()))?;
                               Action::Set { slot: slot.trim().to_string(), value: unescape(v.trim()) } }
                    "call" => { let toks = tokens(arg).map_err(&err)?;
                        let (tool, kvs) = toks.split_first().ok_or_else(|| err("`call TOOL key=value …` — no tool named".into()))?;
                        let mut args = Vec::new();
                        for kv in kvs { let (k, v) = kv.split_once('=').ok_or_else(|| err(format!("`{kv}` is not key=value")))?; args.push((k.to_string(), v.to_string())); }
                        Action::Call { tool: tool.clone(), args } }
                    o => return Err(err(format!("`{o}` is not an action — use call, emit, set, sub or nop"))),
                };
                states.push(RawState { name, action, edges: Vec::new() });
                continue;
            }
            if in_gate { gate_terms.push(parse_guard(t).map_err(err)?); continue; }
            let Some(st) = states.last_mut() else { return Err(err("an edge line before any State".into())) };
            let (g, target) = t.rsplit_once("->").ok_or_else(|| err("an edge is `GUARD -> TARGET`".into()))?;
            st.edges.push((parse_guard(g.trim()).map_err(err)?, target.trim().to_string(), ln));
        }
        let index: BTreeMap<String, usize> = states.iter().enumerate().map(|(i, s)| (s.name.clone(), i)).collect();
        let mut out = Vec::with_capacity(states.len());
        for s in states {
            let mut edges = Vec::new();
            for (when, target, ln) in s.edges {
                let next = if target == "done" { Next::Done(Outcome::Succeeded) }
                    else if target == "fail" { Next::Done(Outcome::Failed) }
                    else if let Some(why) = target.strip_prefix("yield") { Next::Yield(unescape(why.trim())) }
                    else { Next::To(*index.get(&target).ok_or(ParseError { line: ln, why: format!("`-> {target}`: no state has that name (states: {})", index.keys().cloned().collect::<Vec<_>>().join(", ")) })?) };
                edges.push(Edge { when, next });
            }
            out.push(State { name: s.name, action: s.action, edges });
        }
        let gate = gate_on.map(|on| Gate { on, when: match gate_terms.len() { 0 => Guard::Always, 1 => gate_terms.pop().unwrap(), _ => Guard::All(gate_terms) } });
        Ok(Hero { spec, gate, machine: Machine { states: out, start: 0 } })
    }
}

/// What a reflex did, for the HUD and the next prompt.
#[derive(Clone, Debug)]
pub struct Fired { pub hero: String, pub trace: Trace }

/// The heroes that exist. The reflex dispatcher.
#[derive(Clone, Debug, Default)]
pub struct Stable {
    heroes: Vec<Hero>,
    /// Per hero: the captures it last fired on. A persisting error would otherwise wake the same
    /// hero on the same site every turn, re-reading what the model already has.
    last: BTreeMap<String, BTreeMap<String, String>>,
    /// Every firing this stable has made, in order: (hero, how it ended, calls made). The visible
    /// half of the loop — "did a reflex run, and what happened" is a question with an answer.
    history: Vec<(String, End, u32)>,
}

impl Stable {
    pub fn new() -> Self { Self::default() }
    pub fn len(&self) -> usize { self.heroes.len() }
    pub fn is_empty(&self) -> bool { self.heroes.is_empty() }
    pub fn names(&self) -> Vec<&str> { self.heroes.iter().map(|h| h.spec.name.as_str()).collect() }
    pub fn history(&self) -> &[(String, End, u32)] { &self.history }
    pub fn get(&self, name: &str) -> Option<&Hero> { self.heroes.iter().find(|h| h.spec.name == name) }
    /// Every hero's spec — so a [`crate::roster::Roster`] can staff a hero by lane like any worker.
    pub fn specs(&self) -> Vec<WorkerSpec> { self.heroes.iter().map(|h| h.spec.clone()).collect() }

    /// Validate and add. Sub-hero references must already be in the stable: adding in dependency
    /// order is what makes a reference cycle unrepresentable.
    pub fn add(&mut self, hero: Hero, known_tools: &[&str]) -> Result<(), HeroError> {
        hero.validate(known_tools)?;
        if self.get(&hero.spec.name).is_some() { return Err(HeroError::Duplicate(hero.spec.name.clone())); }
        for s in &hero.machine.states {
            if let Action::Sub(n) = &s.action {
                let Some(sub) = self.get(n) else { return Err(HeroError::UnknownSub { state: s.name.clone(), hero: n.clone() }) };
                // a sub runs under the CALLER's whitelist: it may not smuggle a tool in
                for t in sub.calls() { if !hero.spec.tools.iter().any(|x| x == t) {
                    return Err(HeroError::ToolNotWhitelisted { state: format!("{} (via sub-hero `{n}`)", s.name), tool: t.to_string() }); } }
            }
        }
        self.heroes.push(hero);
        Ok(())
    }

    /// Show an observation to the stable. The first hero whose trigger matches, whose every tool
    /// the caller's whitelist permits (`allowed` empty = unconfined), and whose gate passes, runs.
    /// Returns `None` — having cost a string scan — when nothing applies.
    pub fn fire(&mut self, on: Trigger, task: &str, obs: &str, obs_ok: bool, allowed: &[String],
                port: &mut dyn ToolPort, limits: Limits) -> Option<Fired> {
        let subs_src = self.heroes.clone();
        let subs = move |n: &str| subs_src.iter().find(|h| h.spec.name == n).map(|h| h.machine.clone());
        for h in &self.heroes {
            let Some(gate) = &h.gate else { continue };
            if gate.on != on { continue; }
            if !allowed.is_empty() && !h.calls().iter().all(|t| allowed.iter().any(|a| a == t)) { continue; }
            // evaluate the gate on a scratch run so its captures seed the real one
            let mut probe = Run { task, wake: clip(obs, limits.max_obs_bytes), wake_cursor: 0, cur: clip(obs, limits.max_obs_bytes).to_string(),
                cur_ok: obs_ok, cur_cursor: 0, slots: BTreeMap::new(), calls: 0, steps: 0, report: String::new(), trail: Vec::new(), limits };
            if !matches!(eval(&gate.when, &mut probe, 0), Ok(true)) { continue; }
            if self.last.get(&h.spec.name) == Some(&probe.slots) { continue; } // same site as last time
            let seed = probe.slots.clone();
            let name = h.spec.name.clone();
            // The gate read the waking observation through `cur` (they are the same text at this
            // point), so whichever cursor moved is how far it got.
            let consumed = probe.cur_cursor.max(probe.wake_cursor);
            let trace = h.machine.run_from(task, obs, consumed, obs_ok, seed.clone(), port, &subs, limits);
            self.last.insert(name.clone(), seed);
            self.history.push((name.clone(), trace.end.clone(), trace.calls));
            return Some(Fired { hero: name, trace });
        }
        None
    }
}

// ───────────────────────────────────── the seed stable ─────────────────────────────────────

fn spec(name: &str, purpose: &str, tools: &[&str], success: &str) -> WorkerSpec {
    WorkerSpec { name: name.into(), purpose: purpose.into(), owns: vec![], context: vec![],
        tools: tools.iter().map(|s| s.to_string()).collect(), constraints: vec!["never writes".into()],
        success: vec![success.into()], persistence: crate::queen::Persistence::Persistent }
}
fn call(tool: &str, args: &[(&str, &str)]) -> Action {
    Action::Call { tool: tool.into(), args: args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
}
fn edge(when: Guard, next: Next) -> Edge { Edge { when, next } }

/// rustc's location line. `{file:path}` stops at the colon; `{line:int}` at the next.
pub const RUSTC_SITE: &str = "--> {file:path}:{line:int}:";
/// libtest's failure line.
pub const TEST_FAILED: &str = "test {test:word} ... FAILED";

/// Three read-only reflexes, each replacing a model turn that carries no judgement.
///
/// * `error-locator` — a compiler error names `file:line`; read that window (and the next site's,
///   at most two) before the model has to ask.
/// * `test-triage` — a test failed by name; find its `fn` and read it.
/// * `path-prefetch` — the TASK names a file; read it before turn one.
///
/// All three own nothing and whitelist only read tools, so under last pass's enforcement they
/// cannot write even if a machine were edited to try — and a confined worker whose whitelist
/// lacks `read_range` simply never gets `error-locator`.
pub fn seed_stable(known_tools: &[&str]) -> Stable {
    let site = || Pattern::parse(RUSTC_SITE).expect("const pattern");
    let mut st = Stable::new();

    let error_locator = Hero {
        spec: spec("error-locator", "show the code at the compiler error the last turn produced", &["read_range"],
                   "the output contains the file and line of the first error"),
        gate: Some(Gate { on: Trigger::AfterObservation, when: Guard::All(vec![Guard::Contains("error".into()), Guard::Matches(site())]) }),
        machine: Machine { start: 0, states: vec![
            State { name: "read".into(), action: call("read_range", &[("path", "${file}"), ("start", "${line-8}"), ("end", "${line+8}")]),
                edges: vec![edge(Guard::Ok, Next::To(1)), edge(Guard::Always, Next::Yield("could not read ${file} — the path in the error may be relative to a different crate root".into()))] },
            State { name: "report".into(), action: Action::Emit("[error-locator] the code at ${file}:${line} — already read for you, no need to ask:\n${obs}".into()),
                // one more site, at most: the second visit falls through to Done
                edges: vec![edge(Guard::All(vec![Guard::Not(Box::new(Guard::VisitsOver(1))), Guard::WakeMatches(site())]), Next::To(0)),
                            edge(Guard::Always, Next::Done(Outcome::Succeeded))] },
        ] },
    };
    let test_triage = Hero {
        spec: spec("test-triage", "find and show the test function that just failed", &["search", "read_range"],
                   "the output contains the failing test's source file"),
        gate: Some(Gate { on: Trigger::AfterObservation, when: Guard::Matches(Pattern::parse(TEST_FAILED).expect("const pattern")) }),
        machine: Machine { start: 0, states: vec![
            State { name: "find".into(), action: call("search", &[("pattern", "fn ${test:last}"), ("path", "")]),
                edges: vec![edge(Guard::All(vec![Guard::Ok, Guard::Matches(Pattern::parse("{file:path}:{line:int}:").expect("const pattern"))]), Next::To(1)),
                            edge(Guard::Always, Next::Yield("no `fn ${test:last}` found — the test may be generated by a macro".into()))] },
            State { name: "read".into(), action: call("read_range", &[("path", "${file}"), ("start", "${line-2}"), ("end", "${line+30}")]),
                edges: vec![edge(Guard::Ok, Next::To(2)), edge(Guard::Always, Next::Done(Outcome::Failed))] },
            State { name: "report".into(), action: Action::Emit("[test-triage] failing test ${test} is at ${file}:${line} — already read for you:\n${obs}".into()),
                edges: vec![edge(Guard::Always, Next::Done(Outcome::Succeeded))] },
        ] },
    };
    let path_prefetch = Hero {
        spec: spec("path-prefetch", "read the file a task names before the first turn", &["read_file"],
                   "the output contains the named file"),
        gate: Some(Gate { on: Trigger::TaskStart, when: Guard::Matches(Pattern::parse("{file:file}").expect("const pattern")) }),
        machine: Machine { start: 0, states: vec![
            State { name: "read".into(), action: call("read_file", &[("path", "${file}")]),
                // a task may name a file that does not exist yet ("create src/new.rs"): say nothing
                edges: vec![edge(Guard::Ok, Next::To(1)), edge(Guard::Always, Next::Done(Outcome::Failed))] },
            State { name: "report".into(), action: Action::Emit("[path-prefetch] the task names ${file} — here it is:\n${obs}".into()),
                edges: vec![edge(Guard::Always, Next::Done(Outcome::Succeeded))] },
        ] },
    };
    for h in [error_locator, test_triage, path_prefetch] { st.add(h, known_tools).expect("a seed hero must validate"); }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOOLS: &[&str] = &["write_file", "edit", "read_file", "read_range", "read_near", "list_dir", "search", "run",
                             "git_log", "doc", "propose", "scratch_code", "make_tool"];

    /// A port that answers from a table and records every call — the engine's whole view of the world.
    #[derive(Default)]
    struct Script { answers: Vec<(String, Obs)>, log: Vec<String> }
    impl Script {
        fn on(mut self, starts_with: &str, ok: bool, text: &str) -> Self { self.answers.push((starts_with.into(), Obs { ok, text: text.into() })); self }
    }
    impl ToolPort for Script {
        fn call(&mut self, tool: &str, args: &[(String, String)]) -> Obs {
            let line = format!("{tool} {}", args.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "));
            self.log.push(line.clone());
            self.answers.iter().find(|(p, _)| line.starts_with(p.as_str())).map(|(_, o)| o.clone())
                .unwrap_or(Obs { ok: false, text: format!("no such thing: {line}") })
        }
    }
    fn no_subs(_: &str) -> Option<Machine> { None }

    const RUSTC: &str = "error[E0425]: cannot find value `x` in this scope\n --> src/ui.rs:88:9\n  |\n88 |     x + 1\n\n\
error[E0308]: mismatched types\n --> src/model.rs:12:20\n\nerror: could not compile `app` due to 2 previous errors\n";

    #[test]
    fn patterns_capture_by_class_and_iterate_by_cursor() {
        let p = Pattern::parse(RUSTC_SITE).unwrap();
        let (caps, end) = p.find(RUSTC, 0).unwrap();
        assert_eq!(caps, [("file".into(), "src/ui.rs".into()), ("line".into(), "88".into())]);
        let (caps2, _) = p.find(RUSTC, end).unwrap();
        assert_eq!(caps2[0].1, "src/model.rs", "the cursor finds the NEXT site, not the first again");
        assert!(p.find(RUSTC, RUSTC.len()).is_none());
        // hole-first (grep output), and the File class refusing things that are merely path-legal
        let g = Pattern::parse("{file:path}:{line:int}:").unwrap();
        assert_eq!(g.find("src/a.rs:14:    fn empty_input() {", 0).unwrap().0[1].1, "14");
        let f = Pattern::parse("{file:file}").unwrap();
        assert_eq!(f.find("please tidy crates/app/src/ui.rs today", 0).unwrap().0[0].1, "crates/app/src/ui.rs");
        for not_a_file in ["make the renderer faster", "use v1.2 of the api", "see ../../etc/passwd.txt", "and/or maybe"] {
            assert!(f.find(not_a_file, 0).is_none(), "{not_a_file:?} is not a file in a tree");
        }
        // a hole stops where the next literal begins — the natural thing to write must work
        let todo = Pattern::parse("TODO({who:word})").unwrap();
        assert_eq!(todo.find("see TODO(ada) and TODO(grace-h), soon", 0).unwrap().0[0].1, "ada");
        let (_, e) = todo.find("see TODO(ada) and TODO(grace-h), soon", 0).unwrap();
        assert_eq!(todo.find("see TODO(ada) and TODO(grace-h), soon", e).unwrap().0[0].1, "grace-h");
        let arrow = Pattern::parse("{from:rest} -> {to:rest}").unwrap();
        assert_eq!(arrow.find("src/a.rs -> src/b.rs", 0).unwrap().0, [("from".into(), "src/a.rs".into()), ("to".into(), "src/b.rs".into())]);
        assert!(todo.find("TODO() nobody", 0).is_none(), "a hole is never empty");
        assert_eq!(Pattern::parse("v{maj:int}.{min:int}").unwrap().find("rustc v1.96", 0).unwrap().0[1].1, "96");
        // refusals say what to change
        for (bad, why) in [("{a}{b}", "two holes"), ("plain", "no hole"), ("{x:regex}", "unknown class"), ("{", "unclosed"), ("{:int}", "needs a name")] {
            let HeroError::BadPattern { why: w, .. } = Pattern::parse(bad).unwrap_err() else { panic!() };
            assert!(w.contains(why), "{bad:?}: {w}");
        }
        // `{{` is a literal open brace; a close brace outside a hole is always literal
        assert_eq!(Pattern::parse("{{x} {n:int}").unwrap().find("{x} 7", 0).unwrap().0[0].1, "7");
    }

    /// The reflex this module exists for, end to end through the dispatcher: the gate's capture
    /// seeds the machine, the window arithmetic is right, the second site is read, the third is not.
    #[test]
    fn error_locator_reads_the_error_sites_without_a_model() {
        let mut st = seed_stable(TOOLS);
        let mut port = Script::default().on("read_range path=src/ui.rs", true, "88 |     x + 1").on("read_range path=src/model.rs", true, "12 |     let n: u8 = \"no\";");
        let fired = st.fire(Trigger::AfterObservation, "fix the build", RUSTC, false, &[], &mut port, Limits::default()).expect("the gate matches rustc output");
        assert_eq!(fired.hero, "error-locator");
        assert_eq!(port.log, ["read_range path=src/ui.rs start=80 end=96", "read_range path=src/model.rs start=4 end=20"]);
        let t = &fired.trace;
        assert!(t.succeeded() && t.calls == 2, "{}", t.render());
        assert!(t.report.contains("the code at src/ui.rs:88") && t.report.contains("x + 1") && t.report.contains("src/model.rs:12"), "{}", t.report);
        assert!(t.render().contains("read: read_range path=src/ui.rs start=80 end=96 [ok] → report"), "{}", t.render());
        // the SAME observation again does not re-fire on the same site: the model already has it
        assert!(st.fire(Trigger::AfterObservation, "fix the build", RUSTC, false, &[], &mut port, Limits::default()).is_none());
        assert_eq!(port.log.len(), 2);
        // a line number near the top floors at 1 rather than going to 0 or negative
        let mut p2 = Script::default().on("read_range", true, "fn main() {}");
        let mut st2 = seed_stable(TOOLS);
        st2.fire(Trigger::AfterObservation, "t", "error: x\n --> src/main.rs:3:1\n", false, &[], &mut p2, Limits::default()).unwrap();
        assert_eq!(p2.log, ["read_range path=src/main.rs start=1 end=11"]);
    }

    /// The complement (CLAUDE.md §5c): what must NOT wake a hero, and what a hero must not do.
    #[test]
    fn gates_stay_shut_and_whitelists_hold() {
        let mut st = seed_stable(TOOLS);
        let mut port = Script::default();
        for quiet in ["Compiling app v0.1.0\n    Finished dev [unoptimized] target(s) in 1.2s",
                      "warning: unused variable\n --> src/ui.rs:3:9",           // a location, but no error
                      "error: linker `cc` not found",                             // an error, but no location
                      "test result: ok. 12 passed; 0 failed"] {
            assert!(st.fire(Trigger::AfterObservation, "t", quiet, true, &[], &mut port, Limits::default()).is_none(), "{quiet:?} woke a hero");
        }
        assert!(port.log.is_empty(), "a gate that stays shut costs no tool call");
        // a confined worker whose whitelist lacks read_range never gets error-locator …
        let reader_without_range = vec!["read_file".to_string(), "search".to_string()];
        assert!(st.fire(Trigger::AfterObservation, "t", RUSTC, false, &reader_without_range, &mut port, Limits::default()).is_none());
        assert!(st.history().is_empty() && port.log.is_empty(), "SKIPPED, not fired-and-refused: no history, no call");
        // … and one that has it, does
        let with_range = vec!["read_range".to_string()];
        assert!(st.fire(Trigger::AfterObservation, "t", RUSTC, false, &with_range, &mut port, Limits::default()).is_some());
        assert_eq!(st.history().len(), 1);
        assert_eq!(st.history()[0].0, "error-locator");
        // triggers are not interchangeable: a task naming a file does not fire after-observation heroes
        let mut st3 = seed_stable(TOOLS);
        let mut p3 = Script::default().on("read_file path=src/ui.rs", true, "fn draw() {}");
        assert!(st3.fire(Trigger::AfterObservation, "t", "tidy src/ui.rs", true, &[], &mut p3, Limits::default()).is_none());
        let f = st3.fire(Trigger::TaskStart, "tidy src/ui.rs", "tidy src/ui.rs", true, &[], &mut p3, Limits::default()).unwrap();
        assert_eq!((f.hero.as_str(), f.trace.succeeded()), ("path-prefetch", true));
        // a task naming a file that does not exist yet: the hero says NOTHING rather than reporting a failure
        let mut p4 = Script::default();
        let f = seed_stable(TOOLS).fire(Trigger::TaskStart, "create src/new.rs", "create src/new.rs", true, &[], &mut p4, Limits::default()).unwrap();
        assert!(f.trace.report.is_empty() && f.trace.end == End::Done(Outcome::Failed));
    }

    #[test]
    fn test_triage_chains_a_search_into_a_read_and_yields_when_it_cannot() {
        let obs = "running 3 tests\ntest tests::parse::empty_input ... FAILED\ntest tests::parse::ok ... ok\n";
        let mut port = Script::default().on("search pattern=fn empty_input", true, "src/parse.rs:140:    fn empty_input() {\n")
            .on("read_range path=src/parse.rs start=138 end=170", true, "fn empty_input() { assert_eq!(parse(\"\"), None); }");
        let f = seed_stable(TOOLS).fire(Trigger::AfterObservation, "t", obs, false, &[], &mut port, Limits::default()).unwrap();
        assert_eq!(f.hero, "test-triage");
        assert!(f.trace.succeeded() && f.trace.report.contains("tests::parse::empty_input is at src/parse.rs:140"), "{}", f.trace.render());
        // a macro-generated test: nothing to find. The hero YIELDS with a reason — not a failure, not a loop.
        let mut none = Script::default().on("search", true, "");
        let f = seed_stable(TOOLS).fire(Trigger::AfterObservation, "t", obs, false, &[], &mut none, Limits::default()).unwrap();
        assert_eq!(f.trace.end, End::Yield("no `fn empty_input` found — the test may be generated by a macro".into()));
        assert!(f.trace.report.is_empty());
    }

    fn hero(name: &str, tools: &[&str], states: Vec<State>) -> Hero {
        Hero { spec: spec(name, "p", tools, "the output contains the file"), gate: None, machine: Machine { states, start: 0 } }
    }

    /// Validation refuses, by name, each way a machine can be wrong before it runs.
    #[test]
    fn validate_names_what_is_wrong_with_a_machine() {
        let done = || vec![edge(Guard::Always, Next::Done(Outcome::Succeeded))];
        let ok = hero("fine", &["read_file"], vec![State { name: "a".into(), action: call("read_file", &[("path", "x")]), edges: done() }]);
        assert_eq!(ok.validate(TOOLS), Ok(()));
        let e = hero("sneaky", &["read_file"], vec![State { name: "a".into(), action: call("write_file", &[]), edges: done() }]).validate(TOOLS).unwrap_err();
        assert_eq!(e, HeroError::ToolNotWhitelisted { state: "a".into(), tool: "write_file".into() });
        // and listing write_file does not help a lane-less hero: the SPEC refuses it (last pass's rule)
        let e = hero("writer", &["write_file"], vec![State { name: "a".into(), action: call("write_file", &[]), edges: done() }]).validate(TOOLS).unwrap_err();
        assert!(matches!(e, HeroError::Spec(SpecError::WriteWithoutLane(_))), "{e:?}");
        let e = hero("dangling", &["read_file"], vec![State { name: "a".into(), action: Action::Nop, edges: vec![edge(Guard::Always, Next::To(7))] }]).validate(TOOLS).unwrap_err();
        assert_eq!(e, HeroError::BadEdge { state: "a".into(), target: 7 });
        let e = hero("dead", &["read_file"], vec![State { name: "a".into(), action: Action::Nop, edges: done() },
                                                  State { name: "orphan".into(), action: Action::Nop, edges: done() }]).validate(TOOLS).unwrap_err();
        assert_eq!(e, HeroError::Unreachable { state: "orphan".into() });
        let e = hero("spinner", &["read_file"], vec![State { name: "a".into(), action: Action::Nop, edges: vec![edge(Guard::Always, Next::To(0))] }]).validate(TOOLS).unwrap_err();
        assert_eq!(e, HeroError::NeverFinishes);
        let e = hero("strandable", &["read_file"], vec![State { name: "a".into(), action: Action::Nop, edges: vec![edge(Guard::Ok, Next::Done(Outcome::Succeeded))] }]).validate(TOOLS).unwrap_err();
        assert_eq!(e, HeroError::NoFallthrough { state: "a".into() });
        assert!(e.to_string().contains("Always edge"), "{e}");
        assert_eq!(hero("none", &["read_file"], vec![]).validate(TOOLS).unwrap_err(), HeroError::Empty);
    }

    /// Bounds are ends with names, not hangs. (These machines bypass `validate` on purpose: the
    /// engine must be safe even for a machine nobody checked.)
    #[test]
    fn every_limit_is_an_end_the_trace_can_name() {
        let spin = Machine { start: 0, states: vec![State { name: "a".into(), action: Action::Nop, edges: vec![edge(Guard::Always, Next::To(0))] }] };
        let mut port = Script::default();
        let t = spin.run("t", "", true, BTreeMap::new(), &mut port, &no_subs, Limits { max_steps: 10, ..Limits::default() });
        assert_eq!((t.end.clone(), t.steps.len()), (End::StepLimit, 10));
        let greedy = Machine { start: 0, states: vec![State { name: "a".into(), action: call("read_file", &[("path", "x")]), edges: vec![edge(Guard::Always, Next::To(0))] }] };
        let t = greedy.run("t", "", true, BTreeMap::new(), &mut port, &no_subs, Limits { max_calls: 3, ..Limits::default() });
        assert_eq!((t.end, t.calls, port.log.len()), (End::CallLimit, 3, 3));
        let stuck = Machine { start: 0, states: vec![State { name: "a".into(), action: Action::Nop, edges: vec![edge(Guard::Failed, Next::Done(Outcome::Failed))] }] };
        assert_eq!(stuck.run("t", "", true, BTreeMap::new(), &mut port, &no_subs, Limits::default()).end, End::Stuck { state: "a".into() });
        // an unset slot is a YIELD with the slot's name — never a tool call with an empty argument
        let blind = Machine { start: 0, states: vec![State { name: "a".into(), action: call("read_file", &[("path", "${nope}")]), edges: vec![edge(Guard::Always, Next::Done(Outcome::Succeeded))] }] };
        let before = port.log.len();
        let t = blind.run("t", "", true, BTreeMap::new(), &mut port, &no_subs, Limits::default());
        assert!(matches!(&t.end, End::Yield(w) if w.contains("${nope}")), "{:?}", t.end);
        assert_eq!(port.log.len(), before, "the call was never made");
        // the report is capped, on a char boundary
        let loud = Machine { start: 0, states: vec![State { name: "a".into(), action: Action::Emit("é".repeat(100)), edges: vec![edge(Guard::Always, Next::Done(Outcome::Succeeded))] }] };
        let t = loud.run("t", "", true, BTreeMap::new(), &mut port, &no_subs, Limits { max_report_bytes: 11, ..Limits::default() });
        assert_eq!(t.report, "ééééé", "10 bytes: the 11th would split a character");
    }

    /// Heroes compose: a sub-hero shares slots with its caller, runs under the caller's whitelist,
    /// and recursion is bounded by depth rather than trusted not to happen.
    #[test]
    fn sub_heroes_share_slots_and_cannot_smuggle_tools() {
        let done = |o| vec![edge(Guard::Always, Next::Done(o))];
        let mut st = Stable::new();
        st.add(hero("reader", &["read_file"], vec![
            State { name: "read".into(), action: call("read_file", &[("path", "${file}")]), edges: vec![edge(Guard::Ok, Next::To(1)), edge(Guard::Always, Next::Done(Outcome::Failed))] },
            State { name: "keep".into(), action: Action::Set { slot: "body".into(), value: "${obs}".into() }, edges: done(Outcome::Succeeded) },
        ]), TOOLS).unwrap();
        // the caller does NOT whitelist read_file, which its sub needs
        let e = st.add(hero("caller-without", &["search"], vec![State { name: "go".into(), action: Action::Sub("reader".into()), edges: done(Outcome::Succeeded) }]), TOOLS).unwrap_err();
        assert!(matches!(&e, HeroError::ToolNotWhitelisted { tool, .. } if tool == "read_file"), "{e:?}");
        assert!(matches!(st.add(hero("caller-unknown", &["read_file"], vec![State { name: "go".into(), action: Action::Sub("ghost".into()), edges: done(Outcome::Succeeded) }]), TOOLS).unwrap_err(), HeroError::UnknownSub { .. }));
        st.add(hero("caller", &["read_file"], vec![
            State { name: "pick".into(), action: Action::Set { slot: "file".into(), value: "Cargo.toml".into() }, edges: vec![edge(Guard::Always, Next::To(1))] },
            State { name: "go".into(), action: Action::Sub("reader".into()), edges: vec![edge(Guard::Ok, Next::To(2)), edge(Guard::Always, Next::Done(Outcome::Failed))] },
            State { name: "say".into(), action: Action::Emit("got: ${body}".into()), edges: done(Outcome::Succeeded) },
        ]), TOOLS).unwrap();
        assert_eq!(st.add(hero("caller", &["read_file"], vec![State { name: "a".into(), action: Action::Nop, edges: done(Outcome::Succeeded) }]), TOOLS).unwrap_err(), HeroError::Duplicate("caller".into()));
        let src = st.clone();
        let subs = move |n: &str| src.get(n).map(|h| h.machine.clone());
        let mut port = Script::default().on("read_file path=Cargo.toml", true, "[package]");
        let t = st.get("caller").unwrap().machine.run("t", "", true, BTreeMap::new(), &mut port, &subs, Limits::default());
        assert!(t.succeeded() && t.report == "got: [package]", "{}", t.render());
        // a machine that calls ITSELF (unrepresentable through `add`, so built by hand) hits the depth bound
        let selfish = Machine { start: 0, states: vec![State { name: "again".into(), action: Action::Sub("me".into()), edges: done(Outcome::Succeeded) }] };
        let me = selfish.clone();
        let t = selfish.run("t", "", true, BTreeMap::new(), &mut port, &move |_| Some(me.clone()), Limits::default());
        assert!(matches!(&t.end, End::Yield(w) if w.contains("depth bound")), "{:?}", t.end);
    }

    /// bot-arena's machines import as the degenerate case: heaviest edge, each state once.
    #[test]
    fn an_arena_graph_imports_as_a_deterministic_terminating_hero() {
        // 0 →(0.2) 1, 0 →(0.9) 2 ; 2 →(1.0) 0 (a cycle the arena only bounded by max_steps) ; 1 terminal
        let edges = vec![vec![(1, 0.2), (2, 0.9)], vec![], vec![(0, 1.0)]];
        let actions = vec![call("list_dir", &[("path", "")]), call("read_file", &[("path", "README.md")]), call("git_log", &[("n", "3"), ("path", "")])];
        let m = Machine::from_weighted_graph(0, &edges, &actions);
        let h = Hero { spec: spec("arena-import", "p", &["list_dir", "read_file", "git_log"], "the output contains the file"), gate: None, machine: m.clone() };
        // s1 is unreachable under the greedy reading (0 always prefers 2) — validate SAYS so rather than hiding it
        assert_eq!(h.validate(TOOLS), Err(HeroError::Unreachable { state: "s1".into() }));
        let mut port = Script::default().on("list_dir", true, "src").on("git_log", true, "abc fix");
        let t = m.run("t", "", true, BTreeMap::new(), &mut port, &no_subs, Limits::default());
        assert_eq!(port.log, ["list_dir path=", "git_log n=3 path=", "list_dir path="], "0 → 2 → 0, and the second entry to 0 is its last");
        assert!(t.succeeded(), "{}", t.render());
        // same inputs, same run — there is no RNG to seed
        let mut port2 = Script::default().on("list_dir", true, "src").on("git_log", true, "abc fix");
        assert_eq!(m.run("t", "", true, BTreeMap::new(), &mut port2, &no_subs, Limits::default()), t);
    }

    /// The format the Queen is shown must PARSE, must VALIDATE, and must be the same machine as
    /// the hand-built seed hero — otherwise the documentation teaches a hero that does not run.
    #[test]
    fn the_documented_format_is_the_seed_hero() {
        let example = HERO_FORMAT.split_once("Write one as:\n\n").unwrap().1.split_once("\n\nRules.").unwrap().0;
        let h = Hero::parse(example).expect("the example in HERO_FORMAT parses");
        assert_eq!(h.validate(TOOLS), Ok(()));
        let seed = seed_stable(TOOLS);
        let built = seed.get("error-locator").unwrap();
        assert_eq!(h.gate, built.gate, "the gate");
        assert_eq!(h.machine.states[0].action, built.machine.states[0].action, "the call and its window arithmetic");
        assert_eq!(h.machine.states[1].edges, built.machine.states[1].edges, "the bounded loop to the next site");
        // and it RUNS the same: both sites read, through a fresh stable holding only the parsed hero
        let mut st = Stable::new(); st.add(h, TOOLS).unwrap();
        let mut port = Script::default().on("read_range", true, "code");
        let f = st.fire(Trigger::AfterObservation, "t", RUSTC, false, &[], &mut port, Limits::default()).unwrap();
        assert_eq!(port.log, ["read_range path=src/ui.rs start=80 end=96", "read_range path=src/model.rs start=4 end=20"]);
        assert!(f.trace.succeeded());
    }

    /// Every refusal names a line and says what to write instead.
    #[test]
    fn a_malformed_hero_is_refused_by_line() {
        let head = "Name: hero-x\nPurpose: p\nTools:\n- read_file\nSuccess:\n- the output contains the file\n";
        for (body, line, want) in [
            ("State a: teleport x\n  else -> done", 7, "not an action"),
            ("State a: nop\n  maybe -> done", 8, "is not a guard"),
            ("State a: nop\n  else -> nowhere", 8, "no state has that name"),
            ("State a: nop\n  else done", 8, "GUARD -> TARGET"),
            ("Gate: sometimes\nState a: nop\n  else -> done", 7, "task-start"),
            ("State a: call read_file path\n  else -> done", 7, "not key=value"),
            ("State a: nop\n  matches \"{a}{b}\" -> done\n  else -> done", 8, "two holes"),
            ("State a: nop\n  else -> done\nState a: nop\n  else -> done", 9, "defined twice"),
            ("State done: nop\n  else -> done", 7, "cannot be a state name"),
            ("State a: nop\n  contains \"x -> done", 8, "unclosed"),
        ] {
            let e = Hero::parse(&format!("{head}{body}")).unwrap_err();
            assert!(e.line == line && e.why.contains(want), "{body:?}: got line {} {:?}", e.line, e.why);
        }
        assert!(Hero::parse(head).unwrap_err().why.contains("at least one state"));
        // The header IS a WorkerSpec, read by WorkerSpec::parse — which already refuses a nameless
        // one (I had assumed that was left to validate). The refusal is the spec's own correction.
        let e = Hero::parse("Purpose: p\nState a: nop\n  else -> done").unwrap_err();
        assert!(e.why.contains("name"), "{e}");
        // what parse does NOT check is the machine against the tools: that is validate's
        let sneaky = Hero::parse(&format!("{head}State a: call write_file path=x\n  else -> done")).expect("well-formed");
        assert_eq!(sneaky.validate(TOOLS), Err(HeroError::ToolNotWhitelisted { state: "a".into(), tool: "write_file".into() }));
        // quoting: a value with spaces, an escaped quote, `\n` in an emit, forward references
        let h = Hero::parse(&format!("{head}State a: call read_file path=\"my file.rs\"\n  ok and contains \"say \\\"hi\\\"\" -> b\n  else -> fail\nState b: emit one\\ntwo\n  else -> done")).unwrap();
        assert_eq!(h.machine.states[0].action, call("read_file", &[("path", "my file.rs")]));
        assert_eq!(h.machine.states[0].edges[0].when, Guard::All(vec![Guard::Ok, Guard::Contains("say \"hi\"".into())]));
        assert_eq!(h.machine.states[1].action, Action::Emit("one\ntwo".into()));
        assert_eq!(h.machine.states[0].edges[1].next, Next::Done(Outcome::Failed));
    }

    #[test]
    fn the_seed_heroes_are_workers_a_roster_can_hold() {
        let st = seed_stable(TOOLS);
        assert_eq!(st.names(), ["error-locator", "test-triage", "path-prefetch"]);
        let mut roster = crate::roster::seed_roster();
        for s in st.specs() {
            assert!(s.validate(TOOLS).is_ok() && s.owns.is_empty(), "{}", s.name);
            assert_eq!(crate::deploy::Confinement::of(&s).lane, crate::deploy::Lane::ReadOnly);
            roster.add(s);
        }
        assert!(roster.get("error-locator").is_some() && roster.len() == 6);
    }
}
