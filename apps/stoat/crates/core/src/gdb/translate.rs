use crate::backend::{Channel, Event, Reply, Request};
use crate::model::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use stoat_mi::{AsyncKind, Command, Record, StreamKind, Value};

/// What a token was for, so the reply can be decoded into the right shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    Plain,
    Threads,
    Frames { thread: u32 },
    Locals { thread: u32, frame: u32 },
    Evaluate { expr: String },
    Breakpoint,
}

pub type PendingMap = HashMap<u64, Pending>;

/// Request -> MI commands. Most map 1:1; `Load` is several commands, and only
/// the last one carries the caller's reply.
pub fn encode(req: &Request) -> Vec<(Command, Pending)> {
    use Pending::*;
    let one = |c: Command, p: Pending| vec![(c, p)];
    match req {
        Request::Load { program, args, env, cwd } => {
            let mut v = Vec::new();
            // `set environment` takes the rest of the line verbatim, so the value is
            // not quoted; a newline would end the command, so such values are dropped.
            for (k, val) in env.iter().filter(|(k, v)| !k.contains(['=', '\n']) && !v.contains('\n')) {
                v.push((Command::new("gdb-set").opt("environment").opt(&format!("{k}={val}")), Plain));
            }
            if let Some(cwd) = cwd {
                v.push((Command::new("environment-cd").arg(cwd.display()), Plain));
            }
            let mut a = Command::new("exec-arguments");
            for arg in args {
                a = a.arg(arg);
            }
            v.push((a, Plain));
            v.push((Command::new("file-exec-and-symbols").arg(program.display()), Plain));
            v
        }
        Request::LoadCore { program, core } => vec![
            (Command::new("file-exec-and-symbols").arg(program.display()), Plain),
            (Command::new("target-select").opt("core").arg(core.display()), Plain),
        ],
        Request::Attach { pid } => one(Command::new("target-attach").arg(pid), Plain),
        Request::Run => one(Command::new("exec-run"), Plain),
        Request::Continue => one(Command::new("exec-continue"), Plain),
        Request::Interrupt => one(Command::new("exec-interrupt"), Plain),
        Request::Step { kind, thread } => {
            let op = match kind {
                crate::backend::StepKind::Over => "exec-next",
                crate::backend::StepKind::Into => "exec-step",
                crate::backend::StepKind::Out => "exec-finish",
            };
            let mut c = Command::new(op);
            if let Some(t) = thread {
                c = c.opt("--thread").arg(t);
            }
            one(c, Plain)
        }
        Request::Threads => one(Command::new("thread-info"), Threads),
        Request::Frames { thread } => {
            // Bounded: a runaway recursion has millions of frames, and a phone
            // should not try to list them. Deeper frames are reachable via Raw.
            one(Command::new("stack-list-frames").opt("--thread").arg(thread).arg(0).arg(255), Frames { thread: *thread })
        }
        Request::Locals { thread, frame } => one(
            Command::new("stack-list-variables")
                .opt("--thread")
                .arg(thread)
                .opt("--frame")
                .arg(frame)
                .opt("--simple-values"),
            Locals { thread: *thread, frame: *frame },
        ),
        Request::Evaluate { thread, frame, expr } => one(
            Command::new("data-evaluate-expression").opt("--thread").arg(thread).opt("--frame").arg(frame).quoted(expr),
            Evaluate { expr: expr.clone() },
        ),
        // -f: accept locations in libraries not loaded yet instead of erroring.
        Request::InsertBreakpoint { location } => one(Command::new("break-insert").opt("-f").quoted(location), Breakpoint),
        Request::DeleteBreakpoint { id } => one(Command::new("break-delete").arg(id), Plain),
        Request::Raw(line) => one(Command::new("interpreter-exec").opt("console").quoted(line), Plain),
        Request::TargetInput(_) => Vec::new(),
        Request::Shutdown => one(Command::new("gdb-exit"), Plain),
    }
}

pub fn frame_from(v: &Value) -> Frame {
    let func = v.get_str("func").filter(|f| *f != "??").map(str::to_string);
    Frame {
        level: v.get_u64("level").unwrap_or(0) as u32,
        addr: v.get_u64("addr").unwrap_or(0),
        func,
        file: v.get_str("fullname").or_else(|| v.get_str("file")).map(PathBuf::from),
        line: v.get_u64("line").map(|l| l as u32),
        from: v.get_str("from").map(str::to_string),
    }
}

fn stop_from(v: &Value, at: Instant) -> Stop {
    let id = || v.get_u64("bkptno").unwrap_or(0) as u32;
    let reason = match v.get_str("reason") {
        Some("breakpoint-hit") => StopReason::Breakpoint { id: id() },
        Some("watchpoint-trigger" | "read-watchpoint-trigger" | "access-watchpoint-trigger") => {
            let n = v.get("wpt").or_else(|| v.get("hw-rwpt")).or_else(|| v.get("hw-awpt"));
            StopReason::Watchpoint { id: n.and_then(|w| w.get_u64("number")).unwrap_or(0) as u32 }
        }
        Some("signal-received") => StopReason::Signal {
            name: v.get_str("signal-name").unwrap_or("?").to_string(),
            meaning: v.get_str("signal-meaning").unwrap_or("").to_string(),
        },
        Some("end-stepping-range") => StopReason::StepDone,
        Some("function-finished") => StopReason::FunctionFinished,
        Some("exited-normally") => StopReason::Exited { code: 0 },
        // MI prints the exit code in octal: 101 (a Rust panic) arrives as "0145".
        Some("exited") => StopReason::Exited {
            code: v.get_str("exit-code").and_then(|c| i32::from_str_radix(c, 8).ok()).unwrap_or(-1),
        },
        Some("exited-signalled") => StopReason::ExitedSignalled { name: v.get_str("signal-name").unwrap_or("?").to_string() },
        Some(other) => StopReason::Other(other.to_string()),
        None => StopReason::Other("stopped".to_string()),
    };
    Stop { reason, thread: v.get_u64("thread-id").map(|t| t as u32), frame: v.get("frame").map(frame_from), at }
}

fn decode(p: Pending, r: &Value) -> Reply {
    match p {
        Pending::Plain => Reply::Ok,
        Pending::Threads => Reply::Threads {
            current: r.get_u64("current-thread-id").map(|t| t as u32),
            threads: r
                .get("threads")
                .map(|ts| {
                    ts.items()
                        .map(|t| Thread {
                            id: t.get_u64("id").unwrap_or(0) as u32,
                            name: t.get_str("name").map(str::to_string),
                            target_id: t.get_str("target-id").unwrap_or("").to_string(),
                            stopped: t.get_str("state") == Some("stopped"),
                            top: t.get("frame").map(frame_from),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        Pending::Frames { thread } => {
            Reply::Frames { thread, frames: r.get("stack").map(|s| s.items().map(frame_from).collect()).unwrap_or_default() }
        }
        Pending::Locals { thread, frame } => Reply::Locals {
            thread,
            frame,
            vars: r
                .get("variables")
                .map(|vs| {
                    vs.items()
                        .map(|v| Variable {
                            name: v.get_str("name").unwrap_or("?").to_string(),
                            ty: v.get_str("type").map(str::to_string),
                            value: v.get_str("value").map(str::to_string),
                            is_arg: v.get_str("arg") == Some("1"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        Pending::Evaluate { expr } => Reply::Value { expr, value: r.get_str("value").unwrap_or("").to_string() },
        Pending::Breakpoint => {
            let b = r.get("bkpt");
            let addr = b.and_then(|b| b.get_str("addr"));
            let pending = addr == Some("<PENDING>") || b.is_some_and(|b| b.get("pending").is_some() && addr.is_none());
            let locations = match b.and_then(|b| b.get("locations")) {
                Some(l) => l.items().count() as u32,
                None => u32::from(!pending),
            };
            Reply::Breakpoint { id: b.and_then(|b| b.get_u64("number")).unwrap_or(0) as u32, pending, locations }
        }
    }
}

pub fn translate(rec: Record, at: Instant, pending: &mut PendingMap) -> Vec<Event> {
    match rec {
        Record::Prompt => Vec::new(),
        Record::Unparsed(text) => vec![Event::Output { channel: Channel::Target, text: text + "\n" }],
        Record::Stream { kind, text } => {
            let channel = match kind {
                StreamKind::Console => Channel::Console,
                StreamKind::Target => Channel::Target,
                StreamKind::Log => Channel::Log,
            };
            vec![Event::Output { channel, text }]
        }
        Record::Async { kind: AsyncKind::Exec, class, results, .. } => match class.as_str() {
            "running" => vec![Event::Running],
            "stopped" => vec![Event::Stopped(stop_from(&results, at))],
            _ => Vec::new(),
        },
        Record::Async { kind: AsyncKind::Notify, class, results, .. } if class == "thread-group-started" => {
            vec![Event::Started { pid: results.get_u64("pid").map(|p| p as u32) }]
        }
        Record::Async { .. } => Vec::new(),
        Record::Result { token, class, results } => {
            if class == "exit" {
                return vec![Event::BackendGone { detail: "gdb exited".into() }];
            }
            let Some(token) = token else { return Vec::new() };
            let Some(p) = pending.remove(&token) else { return Vec::new() };
            let result = match class.as_str() {
                "error" => Err(results.get_str("msg").unwrap_or("unknown error").to_string()),
                // ^running and ^connected are successes whose payload arrives as async records.
                _ => Ok(decode(p, &results)),
            };
            vec![Event::Reply { id: token, result }]
        }
    }
}
