//! The model seam.
//!
//! `voxel-tools` has no HTTP client and should not grow one — the game
//! already has an async runtime and its own opinions. What it needs is
//! a trait narrow enough that any client satisfies it, plus the parts
//! that are the same for everyone: request shape, response cleaning,
//! and a fake for tests.
//!
//! The cleaning matters more than it sounds. Models wrap plans in code
//! fences, prefix them with "Sure! Here's a plan:", and add a closing
//! remark. Every one of those lines becomes an "unknown op" error and
//! a rejected plan. Stripping them is nine lines here versus a prompt
//! that has to fight the model's instincts.

/// Anything that can turn (system, user) into a plan.
///
/// Same shape as `aperture_complete::Provider`: `&mut self`, a name,
/// and a `Result<String, String>`. Deliberately not async and
/// deliberately not HTTP -- see `CommandProvider` below for why.
pub trait Completer {
    fn name(&self) -> &str { "completer" }
    /// Blocking. Call it off the frame thread.
    fn complete(&mut self, system: &str, user: &str) -> Result<String, String>;
}

/// Any executable is a model.
///
/// Lifted from `aperture_complete::providers::CommandProvider`, which
/// exists for the same reason it is the right choice here: the game
/// should not grow an HTTP client, a TLS stack, or an opinion about
/// which server you run. A provider is any program that reads a prompt
/// on stdin and writes a plan to stdout. A shell script around
/// `llama-cli`, a curl to a private relay, an ollama wrapper, a
/// fixture for tests -- all the same interface.
///
/// Wire format, matching aperture's `prefix NUL suffix`:
///
/// ```text
/// <system prompt>\0<user turn>
/// ```
///
/// with `VOXEL_PASS` in the environment when the caller knows it.
/// Read to EOF, reply on stdout, exit 0. Non-zero exit means the
/// stderr text becomes the error the player sees.
pub struct CommandProvider {
    pub argv: Vec<String>,
    /// Extra environment for the child, e.g. the model path.
    pub env: Vec<(String, String)>,
}

impl CommandProvider {
    pub fn new(argv: Vec<String>) -> Self {
        Self { argv, env: Vec::new() }
    }
    /// The conventional shape: a script in the game's data dir, given
    /// the chosen model path so it can pick the weights.
    pub fn with_model(argv: Vec<String>, model_path: &std::path::Path) -> Self {
        Self { argv, env: vec![("VOXEL_MODEL".into(), model_path.display().to_string())] }
    }
}

impl Completer for CommandProvider {
    fn name(&self) -> &str { self.argv.first().map(String::as_str).unwrap_or("command") }

    fn complete(&mut self, system: &str, user: &str) -> Result<String, String> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let (prog, args) = self.argv.split_first().ok_or("empty command")?;
        let mut cmd = Command::new(prog);
        cmd.args(args);
        for (k, v) in &self.env { cmd.env(k, v); }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn {prog}: {e}"))?;
        {
            let stdin = child.stdin.as_mut().ok_or("no stdin")?;
            // A provider that exits without draining stdin (crash, usage
            // error) closes the pipe under us; that is its report to
            // make. Swallow EPIPE so the child's exit status and stderr
            // surface below instead of a useless "Broken pipe" --
            // aperture learned this one the hard way.
            match stdin.write_all(format!("{system}\0{user}").as_bytes()) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                Err(e) => return Err(format!("write stdin: {e}")),
            }
        } // drop -> EOF
        let out = child.wait_with_output().map_err(|e| format!("wait: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!("{prog} exited {}: {}", out.status,
                        String::from_utf8_lossy(&out.stderr).trim()))
        }
    }
}

/// Strip everything that is not a plan line from a model reply.
///
/// Handles, in order: code fences (``` and ```text), a leading
/// conversational sentence, trailing commentary, and blank padding.
/// Lines that look like IR are kept verbatim -- including comments,
/// which the parser uses for the plan name.
pub fn clean_reply(raw: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut in_fence = false;
    let mut saw_fence = false;
    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            in_fence = !in_fence;
            saw_fence = true;
            continue;
        }
        lines.push(line);
    }
    // If the reply had a fence, keep only what was inside it. Rebuild
    // rather than track state, so an unterminated fence still works.
    if saw_fence {
        let mut inside: Vec<&str> = Vec::new();
        let mut on = false;
        for line in raw.lines() {
            if line.trim().starts_with("```") { on = !on; continue; }
            if on { inside.push(line); }
        }
        if !inside.is_empty() { lines = inside; }
    }
    // Drop lines that cannot be IR: no known verb and not a comment.
    let keep: Vec<&str> = lines.into_iter()
        .filter(|l| {
            let t = l.trim();
            if t.is_empty() { return false; }
            if t.starts_with('#') { return true; }
            let verb = t.split_whitespace().next().unwrap_or("");
            KNOWN_VERBS.contains(&verb)
        })
        .collect();
    let mut out = keep.join("\n");
    if !out.is_empty() { out.push('\n'); }
    out
}

/// Verbs the plan parser accepts. Kept here so `clean_reply` and the
/// parser cannot drift; there is a test asserting each one parses.
pub const KNOWN_VERBS: &[&str] = &[
    "terraform", "path", "enclose", "cylinder", "tower", "sphere", "dome",
    "tree", "scatter", "restyle", "fill", "box",
];

/// A scripted completer for tests and for driving the game headlessly.
/// Returns replies in order; repeats the last one once exhausted, so a
/// loop that asks twice does not panic.
pub struct FakeCompleter {
    replies: Vec<String>,
    calls: std::cell::RefCell<usize>,
    /// Every (system, user) pair seen, for assertions.
    pub seen: std::cell::RefCell<Vec<(String, String)>>,
}

impl FakeCompleter {
    pub fn new(replies: Vec<String>) -> Self {
        Self { replies, calls: std::cell::RefCell::new(0), seen: Default::default() }
    }
    pub fn calls(&self) -> usize { *self.calls.borrow() }
}

impl Completer for FakeCompleter {
    fn name(&self) -> &str { "fake" }
    fn complete(&mut self, system: &str, user: &str) -> Result<String, String> {
        self.seen.borrow_mut().push((system.to_string(), user.to_string()));
        let mut n = self.calls.borrow_mut();
        let i = (*n).min(self.replies.len().saturating_sub(1));
        *n += 1;
        self.replies.get(i).cloned().ok_or_else(|| "no scripted reply".to_string())
    }
}

/// A completer that always fails, for exercising the error path. A
/// build loop that has never been tested against a dead server will
/// hang the first time llama-server is not running.
pub struct DeadCompleter(pub String);
impl Completer for DeadCompleter {
    fn name(&self) -> &str { "dead" }
    fn complete(&mut self, _s: &str, _u: &str) -> Result<String, String> { Err(self.0.clone()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parse_plan_at;

    #[test]
    fn fences_and_chatter_are_stripped() {
        let raw = "Sure! Here's a plan for a watchtower:\n\
                   ```text\n\
                   # watchtower\n\
                   terraform @-5,-5 @5,5 smooth block=stone\n\
                   cylinder @0,0 r=3 h=11 block=stone hollow\n\
                   ```\n\
                   Let me know if you'd like it taller!";
        let c = clean_reply(raw);
        assert!(c.starts_with("# watchtower"), "{c}");
        assert!(c.contains("cylinder @0,0"));
        assert!(!c.contains("Sure"));
        assert!(!c.contains("taller"));
        let (plan, issues) = parse_plan_at(&c, Some((0, 65, 0)));
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(plan.ops.len(), 2);
        assert_eq!(plan.name, "watchtower");
    }

    #[test]
    fn an_unfenced_reply_with_chatter_still_parses() {
        let raw = "Okay, placing a tower here.\ncylinder @0,0 r=2 h=8\nDone!";
        let c = clean_reply(raw);
        assert_eq!(c, "cylinder @0,0 r=2 h=8\n");
    }

    #[test]
    fn an_unterminated_fence_does_not_lose_the_plan() {
        let raw = "```\ntree @0,0 oak h=7\n";
        assert_eq!(clean_reply(raw), "tree @0,0 oak h=7\n");
    }

    #[test]
    fn a_reply_with_no_plan_cleans_to_nothing() {
        let c = clean_reply("I'm not sure what you mean by that.");
        assert!(c.is_empty(), "{c:?}");
        let (plan, _) = parse_plan_at(&c, Some((0, 65, 0)));
        assert!(plan.ops.is_empty(), "an empty plan is rejected downstream, not silently built");
    }

    #[test]
    fn every_known_verb_actually_parses() {
        // the guard against clean_reply and the parser drifting apart
        for verb in KNOWN_VERBS {
            let line = match *verb {
                "terraform" | "path" | "enclose" | "scatter" => format!("{verb} @0,0 @4,4\n"),
                "restyle" => "restyle @0,0,0 @4,4,4 stone oak\n".to_string(),
                "fill" | "box" => format!("{verb} @0,0,0 @2,2,2\n"),
                "tree" => "tree @0,0 oak\n".to_string(),
                _ => format!("{verb} @0,0 r=2 h=4\n"),
            };
            let (plan, issues) = parse_plan_at(&line, Some((0, 65, 0)));
            assert!(issues.is_empty() && plan.ops.len() == 1,
                    "verb '{verb}' in KNOWN_VERBS does not parse: {line:?} {issues:?}");
        }
    }

    #[test]
    fn the_fake_records_what_it_was_asked() {
        let mut f = FakeCompleter::new(vec!["tree @0,0 oak h=6\n".into()]);
        let r = f.complete("SYS", "USER").unwrap();
        assert_eq!(r, "tree @0,0 oak h=6\n");
        assert_eq!(f.calls(), 1);
        assert_eq!(f.seen.borrow()[0].0, "SYS");
        // exhausted: repeats rather than panicking
        assert!(f.complete("SYS", "AGAIN").is_ok());
    }


    #[test]
    fn a_shell_script_is_a_model() {
        // the whole point of the process style: no HTTP in this crate
        let mut p = CommandProvider::new(vec![
            "sh".into(), "-c".into(),
            // read stdin, ignore it, emit a plan
            "cat >/dev/null; printf 'cylinder @0,0 r=2 h=6\\n'".into()]);
        let out = p.complete("SYS", "USER").unwrap();
        assert_eq!(out, "cylinder @0,0 r=2 h=6\n");
        assert_eq!(p.name(), "sh");
    }

    #[test]
    fn the_prompt_arrives_as_system_nul_user() {
        let mut p = CommandProvider::new(vec![
            "sh".into(), "-c".into(),
            // split on NUL and echo the halves back as comments
            "tr '\\0' '\\n' | sed 's/^/# /'".into()]);
        let out = p.complete("SYSTEM_HALF", "USER_HALF").unwrap();
        assert!(out.contains("# SYSTEM_HALF"), "{out}");
        assert!(out.contains("# USER_HALF"), "{out}");
    }

    #[test]
    fn a_failing_provider_reports_its_stderr_not_a_pipe_error() {
        let mut p = CommandProvider::new(vec![
            "sh".into(), "-c".into(), "echo 'model file missing' >&2; exit 3".into()]);
        let e = p.complete("s", "u").unwrap_err();
        assert!(e.contains("model file missing"), "{e}");
        assert!(!e.contains("Broken pipe"), "EPIPE must not mask the real report: {e}");
    }

    #[test]
    fn a_provider_that_never_reads_stdin_still_reports_properly() {
        // exits immediately, closing the pipe mid-write
        let mut p = CommandProvider::new(vec![
            "sh".into(), "-c".into(), "exit 1".into()]);
        let e = p.complete(&"x".repeat(200_000), "u").unwrap_err();
        assert!(e.contains("exited"), "{e}");
    }

    #[test]
    fn a_missing_program_is_a_clear_message() {
        let mut p = CommandProvider::new(vec!["definitely-not-a-real-binary-xyz".into()]);
        let e = p.complete("s", "u").unwrap_err();
        assert!(e.starts_with("spawn definitely-not-a-real-binary-xyz"), "{e}");
    }

    #[test]
    fn model_path_reaches_the_child_environment() {
        let mut p = CommandProvider::with_model(
            vec!["sh".into(), "-c".into(), "cat >/dev/null; echo \"# $VOXEL_MODEL\"".into()],
            std::path::Path::new("/home/u/models/qwen.gguf"));
        let out = p.complete("s", "u").unwrap();
        assert!(out.contains("/home/u/models/qwen.gguf"), "{out}");
    }
}
