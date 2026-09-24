//! dispatch — run a plan against the matrix. Waves fan out across every box that has the task's role,
//! bounded by each box's concurrency; premise checks run first; only the forwarded summary reaches
//! dependents; everything is written to the ledger as it happens. No egui, no async: threads + channels.
use std::collections::HashMap;
use std::process::Command;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::endpoint::{self, Event, Request};
use crate::ledger::{self, Entry};
use crate::matrix::{BoxSpec, Matrix, Tier};
use crate::plan::{forwarded, Function, Plan, Task};

#[derive(Debug, Clone)]
pub struct Outcome { pub task: String, pub r#box: String, pub full: String, pub summary: String, pub keep: bool, pub reason: String, pub ms: f64, pub out_tokens: u32 }

/// Progress callbacks so a UI can draw the DAG as it runs.
pub trait Observer {
    fn wave(&mut self, _n: usize, _ids: &[String]) {}
    fn started(&mut self, _task: &str, _on: &str) {}
    fn delta(&mut self, _task: &str, _text: &str) {}
    fn finished(&mut self, _o: &Outcome) {}
}

pub struct Quiet;
impl Observer for Quiet {}

fn premise_ok(task: &Task) -> Result<(), String> {
    for c in &task.premise {
        let out = Command::new("sh").arg("-c").arg(&c.command).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr)).unwrap_or_default();
        let ok = out.contains(c.expect.as_str());
        let _ = ledger::append(Entry::Premise { task: task.id.clone(), command: c.command.clone(), expect: c.expect.clone(), ok, saw: out.lines().take(2).collect::<Vec<_>>().join(" | ") });
        if !ok { return Err(format!("premise failed: `{}` did not contain `{}`", c.command, c.expect)); }
    }
    Ok(())
}

fn verify(task: &Task, output: &str) -> (bool, String) {
    let Some(v) = &task.verify else { return (true, "no verifier".into()) };
    match Command::new("sh").arg("-c").arg(v).env("OUT", output).output() {
        Ok(o) if o.status.success() => (true, "verifier passed".into()),
        Ok(o) => (false, format!("verifier exit {}: {}", o.status, String::from_utf8_lossy(&o.stderr).lines().take(2).collect::<Vec<_>>().join(" | "))),
        Err(e) => (false, format!("verifier could not run: {e}")),
    }
}

fn substitute(prompt: &str, summaries: &HashMap<String, String>) -> String {
    let mut s = prompt.to_string();
    for (id, sum) in summaries { s = s.replace(&format!("{{{{dep:{id}}}}}"), sum); }
    // whole artifacts by kind: {{guide:ID}}, {{design:ID}}, {{plan:ID}}, {{review:ID}}
    for kind in ["guide", "design", "plan", "review"] {
        let tag = format!("{{{{{kind}:");
        while let Some(a) = s.find(&tag) {
            let Some(b) = s[a..].find("}}") else { break };
            let id = s[a + tag.len()..a + b].to_string();
            let body = ledger::get_artifact(&id, kind).unwrap_or_else(|| format!("(no {kind} artifact for {id})"));
            s.replace_range(a..a + b + 2, &body);
        }
    }
    s
}

fn artifact_kind(f: Function) -> &'static str {
    match f { Function::Plan => "plan", Function::Design => "design", Function::Guide => "guide", Function::Review => "review", _ => "result" }
}

/// The router: for each task, the boxes that satisfy its route (role, tier window, locality), cheapest
/// tier first; ties broken round-robin so a wave spreads across equal boxes. Returns (task index, box).
fn assign<'a>(tasks: &[&Task], m: &'a Matrix) -> Vec<(usize, &'a BoxSpec)> {
    let mut out = Vec::new();
    let mut cursor: HashMap<(crate::matrix::Role, Tier), usize> = HashMap::new();
    for (i, t) in tasks.iter().enumerate() {
        let cands = m.candidates(t.role, t.route.min_tier, t.route.max_tier, t.route.allow_external);
        let Some(cheapest) = cands.first().map(|b| b.tier) else { continue };
        let same: Vec<&BoxSpec> = cands.into_iter().filter(|b| b.tier == cheapest).collect();
        let c = cursor.entry((t.role, cheapest)).or_insert(0);
        out.push((i, same[*c % same.len()])); *c += 1;
    }
    out
}

/// One tier up from `from` within the task's route, for escalation. None when there is nowhere to go.
fn escalation_box<'a>(t: &Task, from: &BoxSpec, m: &'a Matrix) -> Option<&'a BoxSpec> {
    m.candidates(t.role, t.route.min_tier, t.route.max_tier, t.route.allow_external)
        .into_iter().find(|b| b.tier > from.tier)
}

pub fn run(plan: &Plan, m: &Matrix, obs: &mut dyn Observer) -> Result<Vec<Outcome>, String> {
    let waves = plan.waves()?;
    let by_id: HashMap<&str, &Task> = plan.tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    let summaries: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut outcomes = Vec::new();
    for (n, ids) in waves.iter().enumerate() {
        obs.wave(n, ids);
        let tasks: Vec<&Task> = ids.iter().map(|i| by_id[i.as_str()]).collect();
        let assigned = assign(&tasks, m);
        for (i, t) in tasks.iter().enumerate() {
            if !assigned.iter().any(|(k, _)| *k == i) {
                let o = Outcome { task: t.id.clone(), r#box: "-".into(), full: String::new(), summary: String::new(), keep: false, reason: format!("no healthy box with role {:?}", t.role), ms: 0.0, out_tokens: 0 };
                let _ = ledger::append(Entry::Verdict { task: o.task.clone(), keep: false, reason: o.reason.clone() });
                obs.finished(&o); outcomes.push(o);
            }
        }
        // premise checks are cheap and local: run them before spending any box's time
        let mut runnable = Vec::new();
        for (i, b) in assigned {
            let t = tasks[i];
            match premise_ok(t) {
                Ok(()) => runnable.push((t, b)),
                Err(reason) => {
                    let o = Outcome { task: t.id.clone(), r#box: b.name.clone(), full: String::new(), summary: String::new(), keep: false, reason, ms: 0.0, out_tokens: 0 };
                    let _ = ledger::append(Entry::Verdict { task: o.task.clone(), keep: false, reason: o.reason.clone() });
                    obs.finished(&o); outcomes.push(o);
                }
            }
        }
        // fan out: one thread per task, gated per box by a counting semaphore made of a Mutex<usize>
        let (tx, rx) = channel::<(String, Result<(Outcome, ()), String>, Option<String>)>();
        let gates: HashMap<String, Arc<(Mutex<usize>, std::sync::Condvar)>> = m.boxes.iter().map(|b| (b.name.clone(), Arc::new((Mutex::new(b.concurrency.max(1)), std::sync::Condvar::new())))).collect();
        let mut handles = Vec::new();
        let (dtx, drx) = channel::<(String, String)>();   // deltas for the observer
        for (t, b) in runnable {
            let t = t.clone(); let b = b.clone(); let tx = tx.clone(); let dtx = dtx.clone();
            let gate = gates[&b.name].clone();
            let prompt = substitute(&t.prompt, &summaries.lock().unwrap());
            handles.push(std::thread::spawn(move || {
                { let (lock, cv) = &*gate; let mut n = lock.lock().unwrap(); while *n == 0 { n = cv.wait(n).unwrap(); } *n -= 1; }
                let req = Request { prompt, system: t.system.clone(), max_tokens: t.max_tokens, temperature: t.temperature, resume: None, grammar: None };
                let t0 = Instant::now();
                let mut full = String::new();
                let id = t.id.clone();
                let mut on = |e: Event| { if let Event::Delta(s) = e { full.push_str(&s); let _ = dtx.send((id.clone(), s)); } true };
                let r = endpoint::generate(&b.endpoint, &req, &mut on);
                { let (lock, cv) = &*gate; *lock.lock().unwrap() += 1; cv.notify_one(); }
                let res = match r {
                    Ok(mt) => {
                        let (keep, reason) = verify(&t, &full);
                        let summary = forwarded(&full, &t.forward);
                        Ok((Outcome { task: t.id.clone(), r#box: b.name.clone(), summary, keep, reason, ms: t0.elapsed().as_secs_f64() * 1000.0, out_tokens: mt.out_tokens, full }, ()))
                    }
                    Err(e) => Err(e),
                };
                let _ = tx.send((t.id.clone(), res, Some(b.name.clone())));
            }));
        }
        drop(tx); drop(dtx);
        // observer pump: deltas while the wave runs
        loop {
            while let Ok((id, s)) = drx.try_recv() { obs.delta(&id, &s); }
            match rx.try_recv() {
                Ok((id, res, bname)) => {
                    let o = match res {
                        Ok((o, ())) => o,
                        Err(e) => Outcome { task: id.clone(), r#box: bname.unwrap_or("-".into()), full: String::new(), summary: String::new(), keep: false, reason: e, ms: 0.0, out_tokens: 0 },
                    };
                    record(&o, by_id[o.task.as_str()], &summaries);
                    obs.finished(&o); outcomes.push(o);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => { std::thread::sleep(std::time::Duration::from_millis(20)); }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        for h in handles { let _ = h.join(); }
        while let Ok((id, s)) = drx.try_recv() { obs.delta(&id, &s); }

        // escalation: a DISCARD on a local tier re-runs once on the next tier the route allows
        let retry: Vec<(String, String)> = outcomes.iter().filter(|o| !o.keep && ids.contains(&o.task)).map(|o| (o.task.clone(), o.r#box.clone())).collect();
        for (tid, from_name) in retry {
            let t = by_id[tid.as_str()];
            if !t.route.escalate_on_fail { continue; }
            let Some(from) = m.get(&from_name) else { continue };
            let Some(to) = escalation_box(t, from, m) else { continue };
            let reason = outcomes.iter().rev().find(|o| o.task == tid).map(|o| o.reason.clone()).unwrap_or_default();
            let _ = ledger::append(Entry::Escalation { task: tid.clone(), from_box: from.name.clone(), to_box: to.name.clone(), reason: reason.clone() });
            obs.started(&tid, &to.name);
            let prompt = substitute(&t.prompt, &summaries.lock().unwrap());
            let req = Request { prompt, system: t.system.clone(), max_tokens: t.max_tokens, temperature: t.temperature, resume: None, grammar: None };
            let t0 = Instant::now();
            let mut full = String::new();
            let mut on = |e: Event| { if let Event::Delta(s) = e { full.push_str(&s); obs.delta(&tid, &s); } true };
            let o = match endpoint::generate(&to.endpoint, &req, &mut on) {
                Ok(mt) => { let (keep, reason) = verify(t, &full); let summary = forwarded(&full, &t.forward);
                    Outcome { task: tid.clone(), r#box: to.name.clone(), summary, keep, reason, ms: t0.elapsed().as_secs_f64() * 1000.0, out_tokens: mt.out_tokens, full } }
                Err(e) => Outcome { task: tid.clone(), r#box: to.name.clone(), full: String::new(), summary: String::new(), keep: false, reason: e, ms: 0.0, out_tokens: 0 },
            };
            record(&o, t, &summaries);
            obs.finished(&o); outcomes.push(o);
        }
    }
    Ok(outcomes)
}

/// Ledger writes for a finished task; artifacts (plan/design/guide/review) are stored whole.
fn record(o: &Outcome, t: &Task, summaries: &Arc<Mutex<HashMap<String, String>>>) {
    let _ = ledger::append(Entry::Result { task: o.task.clone(), r#box: o.r#box.clone(), out_tokens: o.out_tokens, total_ms: o.ms, forwarded_chars: o.summary.chars().count(), full_chars: o.full.chars().count() });
    let _ = ledger::append(Entry::Verdict { task: o.task.clone(), keep: o.keep, reason: o.reason.clone() });
    if o.keep {
        if t.produces_artifact() || t.function == Function::Review {
            if let Ok(p) = ledger::put_artifact(&o.task, artifact_kind(t.function), &o.full) {
                let _ = ledger::append(Entry::Artifact { task: o.task.clone(), artifact: artifact_kind(t.function).into(), r#box: o.r#box.clone(), chars: o.full.chars().count(), path: p.display().to_string() });
            }
        }
        summaries.lock().unwrap().insert(o.task.clone(), o.summary.clone());
    }
}
