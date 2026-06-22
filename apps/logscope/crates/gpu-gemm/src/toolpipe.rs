//! toolpipe.rs — calling and piping OS binaries at scale, for FSM-driven tool use.
//!
//! `std::process` only (no deps), and it mirrors the inference fleet's Arc + threaded
//! fan-out so a bot's tool calls and the model's decode share one concurrency model.
//!
//! The pieces, bottom-up:
//!   * `Stage` / `Pipeline`  — a binary + args, chained with OS pipes (`a | b | c`).
//!   * `Policy`              — allowlist + output cap + timeout (the safety gate).
//!   * `run_pipeline`        — spawn the chain, wire stdout→stdin, capture/cap output, kill on timeout.
//!   * `ToolRunner`          — N worker threads draining a job queue (the "at scale" layer).
//!   * `Bot` / `drive`       — the FSM contract: observe → act (Call/Emit/Done/Fail) → repeat.
//!   * `parse_calls`         — a *reference* parser turning model text into pipelines.
//!
//! Nothing here uses a shell: args are passed as a vector and exec'd directly, so there is
//! no `sh -c` and no shell-injection surface. Execution is gated by an explicit allowlist.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// One binary invocation in a pipeline.
#[derive(Clone)]
pub struct Stage { pub bin: String, pub args: Vec<String> }
impl Stage {
    pub fn new(bin: &str, args: &[&str]) -> Self {
        Self { bin: bin.to_string(), args: args.iter().map(|s| s.to_string()).collect() }
    }
}

/// A chain of stages connected by OS pipes, with optional stdin fed to the first stage.
#[derive(Clone)]
pub struct Pipeline { pub stages: Vec<Stage>, pub stdin: Option<String> }
impl Pipeline {
    pub fn single(bin: &str, args: &[&str]) -> Self { Self { stages: vec![Stage::new(bin, args)], stdin: None } }
    pub fn pipe(mut self, bin: &str, args: &[&str]) -> Self { self.stages.push(Stage::new(bin, args)); self }
    pub fn with_stdin(mut self, s: &str) -> Self { self.stdin = Some(s.to_string()); self }
    pub fn binaries(&self) -> impl Iterator<Item = &str> { self.stages.iter().map(|s| s.bin.as_str()) }
    /// Human-readable `bin args | bin args` for logging.
    pub fn display(&self) -> String {
        self.stages.iter()
            .map(|s| format!("{} {}", s.bin, s.args.join(" ")).trim().to_string())
            .collect::<Vec<_>>().join(" | ")
    }
}

/// Safety gate: only allow-listed binaries run; output is capped; the pipeline is killed
/// if it exceeds `timeout`.
#[derive(Clone)]
pub struct Policy { pub allow: Arc<HashSet<String>>, pub max_output: usize, pub timeout: Duration }
impl Policy {
    pub fn new(allow: &[&str], max_output: usize, timeout: Duration) -> Self {
        Self { allow: Arc::new(allow.iter().map(|s| s.to_string()).collect()), max_output, timeout }
    }
}

#[derive(Debug)]
pub enum ToolError { NotAllowed(String), Spawn(String), Io(String), Timeout }
impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ToolError::NotAllowed(b) => write!(f, "binary not allowed: {b}"),
            ToolError::Spawn(e) => write!(f, "spawn failed: {e}"),
            ToolError::Io(e) => write!(f, "io error: {e}"),
            ToolError::Timeout => write!(f, "timed out"),
        }
    }
}

/// The result of running a pipeline. `code` is the *last* stage's exit code.
pub struct PipeOutcome { pub stdout: String, pub stderr: String, pub code: Option<i32>, pub truncated: bool, pub dur: Duration }
impl PipeOutcome {
    pub fn ok(&self) -> bool { self.code == Some(0) }
}

/// Spawn `p`'s stages, wiring each stdout into the next stdin, feeding `p.stdin` to the
/// first, and capturing the last stage's stdout (capped) and stderr. Kills the chain and
/// returns `Timeout` if it runs past `policy.timeout`.
pub fn run_pipeline(p: &Pipeline, policy: &Policy) -> Result<PipeOutcome, ToolError> {
    if p.stages.is_empty() { return Err(ToolError::Spawn("empty pipeline".into())); }
    for b in p.binaries() {
        if !policy.allow.contains(b) { return Err(ToolError::NotAllowed(b.to_string())); }
    }
    let start = Instant::now();
    let mut children = Vec::new();
    let mut prev_stdout: Option<std::process::ChildStdout> = None;

    for (i, s) in p.stages.iter().enumerate() {
        let mut cmd = Command::new(&s.bin);
        cmd.args(&s.args);
        match (i, &p.stdin) {
            (0, Some(_)) => { cmd.stdin(Stdio::piped()); }
            (0, None) => { cmd.stdin(Stdio::null()); }
            _ => { cmd.stdin(Stdio::from(prev_stdout.take().expect("prev stdout"))); }
        }
        cmd.stdout(Stdio::piped());
        // Only the last stage's stderr is captured; intermediates discard theirs.
        cmd.stderr(if i + 1 == p.stages.len() { Stdio::piped() } else { Stdio::null() });
        let mut child = cmd.spawn().map_err(|e| ToolError::Spawn(format!("{}: {e}", s.bin)))?;
        if i == 0 {
            if let Some(data) = p.stdin.clone() {
                if let Some(mut sin) = child.stdin.take() {
                    // write on a thread so large input can't deadlock against a full pipe
                    std::thread::spawn(move || { let _ = sin.write_all(data.as_bytes()); });
                }
            }
        }
        prev_stdout = child.stdout.take();
        children.push(child);
    }

    let mut last_stderr = children.last_mut().and_then(|c| c.stderr.take());
    let mut last_stdout = prev_stdout.take().expect("last stdout");
    let cap = policy.max_output;

    // Read (capped) on a thread so we can enforce a timeout via recv_timeout.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut truncated = false;
        loop {
            match last_stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if buf.len() < cap {
                        let room = cap - buf.len();
                        buf.extend_from_slice(&chunk[..n.min(room)]);
                        if n > room { truncated = true; }
                    } else { truncated = true; }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send((buf, truncated));
    });

    let (stdout_bytes, truncated) = match rx.recv_timeout(policy.timeout) {
        Ok(v) => v,
        Err(_) => { for c in &mut children { let _ = c.kill(); } return Err(ToolError::Timeout); }
    };

    let mut stderr = String::new();
    if let Some(mut se) = last_stderr.take() { let _ = se.read_to_string(&mut stderr); }

    let n = children.len();
    let mut code = None;
    for (i, mut c) in children.into_iter().enumerate() {
        let st = c.wait().map_err(|e| ToolError::Io(e.to_string()))?;
        if i + 1 == n { code = st.code(); }
    }

    Ok(PipeOutcome {
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr, code, truncated, dur: start.elapsed(),
    })
}

// ── at scale: a worker pool draining a job queue ─────────────────────────────

pub type JobResult = (u64, Result<PipeOutcome, ToolError>);

/// N worker threads sharing one `Policy`, each pulling pipelines off a queue and pushing
/// results back — the same shape as the model fleet, so a scheduler can interleave tool
/// calls and decode against one budget.
pub struct ToolRunner { tx: mpsc::Sender<(u64, Pipeline)>, rx: mpsc::Receiver<JobResult>, _workers: Vec<JoinHandle<()>> }
impl ToolRunner {
    pub fn new(workers: usize, policy: Policy) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<(u64, Pipeline)>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let (res_tx, res_rx) = mpsc::channel::<JobResult>();
        let mut hs = Vec::new();
        for _ in 0..workers.max(1) {
            let (jr, rt, pol) = (job_rx.clone(), res_tx.clone(), policy.clone());
            hs.push(std::thread::spawn(move || loop {
                let job = { jr.lock().unwrap().recv() };
                match job {
                    Ok((id, p)) => { let r = run_pipeline(&p, &pol); if rt.send((id, r)).is_err() { break; } }
                    Err(_) => break,
                }
            }));
        }
        Self { tx: job_tx, rx: res_rx, _workers: hs }
    }
    pub fn submit(&self, id: u64, p: Pipeline) { let _ = self.tx.send((id, p)); }
    pub fn recv(&self) -> Option<JobResult> { self.rx.recv().ok() }
    pub fn try_results(&self) -> Vec<JobResult> { self.rx.try_iter().collect() }
}

// ── FSM contract: what an external bot implements ────────────────────────────

/// What the FSM sees before each decision: the previous action's outcome (if it was a
/// tool call) and the step index.
pub struct Observation<'a> { pub last: Option<&'a PipeOutcome>, pub step: usize }

/// What the FSM decides to do next.
pub enum Action { Call(Pipeline), Emit(String), Done, Fail(String) }

/// Implement this on your state machine; `drive` runs the loop and executes `Call`s.
pub trait Bot { fn step(&mut self, obs: &Observation) -> Action; }

pub enum Status { Done, Failed(String), StepLimit }
pub struct DriveLog { pub emitted: Vec<String>, pub calls: Vec<String>, pub status: Status }

/// Run a bot's FSM under a policy: observe → step → (execute tool call | emit | stop),
/// bounded by `max_steps`. Tool calls run through `run_pipeline`, and their outcome
/// becomes the next observation — this is the pipe back into the model's context.
pub fn drive<B: Bot>(bot: &mut B, policy: &Policy, max_steps: usize) -> DriveLog {
    let mut emitted = Vec::new();
    let mut calls = Vec::new();
    let mut last: Option<PipeOutcome> = None;
    for step in 0..max_steps {
        let obs = Observation { last: last.as_ref(), step };
        match bot.step(&obs) {
            Action::Call(p) => {
                calls.push(p.display());
                match run_pipeline(&p, policy) {
                    Ok(o) => last = Some(o),
                    Err(e) => return DriveLog { emitted, calls, status: Status::Failed(e.to_string()) },
                }
            }
            Action::Emit(s) => { emitted.push(s); last = None; }
            Action::Done => return DriveLog { emitted, calls, status: Status::Done },
            Action::Fail(s) => return DriveLog { emitted, calls, status: Status::Failed(s) },
        }
    }
    DriveLog { emitted, calls, status: Status::StepLimit }
}

/// Reference parser: turn model output into pipelines. The model emits lines like
/// `!run journalctl -u nginx | grep ERROR`; each becomes a `Pipeline`. Parsing does NOT
/// execute — the `Policy` still gates every binary at run time. Whitespace-split only
/// (no quoting); a real bot can swap in its own parser and keep the rest of the pipeline.
pub fn parse_calls(text: &str) -> Vec<Pipeline> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("!run ") else { continue };
        let mut pl: Option<Pipeline> = None;
        for seg in rest.split('|') {
            let toks: Vec<&str> = seg.split_whitespace().collect();
            if toks.is_empty() { continue; }
            let (bin, args) = (toks[0], &toks[1..]);
            pl = Some(match pl { None => Pipeline::single(bin, args), Some(p) => p.pipe(bin, args) });
        }
        if let Some(p) = pl { out.push(p); }
    }
    out
}
