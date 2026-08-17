//! agent-score — score a model's PROTOCOL behaviour on the 15-case Queen suite.
//!
//! Two modes, because the scoring and the model-running are separate problems:
//!
//!   --prompts            print the cases, one per line, tab-separated with the case name.
//!                        Pipe them to whatever runs your model.
//!   --score <dir>        read <dir>/<case-name>.txt as that case's raw model output and score it.
//!
//! Keeping the runner out of this binary is deliberate: it means the scorer works for a local GGUF,
//! a remote endpoint, or output pasted from a chat window, and the score is comparable across all
//! three. What gets measured is whether the model ends its turn with exactly one action, whether it
//! asks instead of assuming, and whether every entity it names was actually supplied.
use logscope_fan::agentscore::{score, suite, Report, Violation};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("--help");
    match mode {
        "--prompts" => {
            for (c, p) in suite() { println!("{}\t{}", c.name, p); }
        }
        "--score" => {
            let dir = args.get(2).map(String::as_str).unwrap_or(".");
            let mut scores = Vec::new();
            let mut missing = 0;
            for (c, _) in suite() {
                let path = format!("{dir}/{}.txt", c.name.replace(' ', "_"));
                match std::fs::read_to_string(&path) {
                    Ok(out) => scores.push(score(&c, &out)),
                    Err(_) => { missing += 1; eprintln!("missing: {path}"); }
                }
            }
            if missing > 0 {
                eprintln!("{missing} case(s) had no output file — scoring only what is present, \
because a partial run reported as a full one is worse than no number.");
            }
            for s in &scores {
                let mark = if s.protocol_clean() && s.choice_correct { "ok  " } else { "FAIL" };
                println!("{mark} {:<32} actions {:?} narration {:.0}%", s.case, s.actions,
                         100.0 * s.narration_share());
                for v in &s.violations {
                    let who = if v.enforceable_by_runtime() { "runtime can prevent" } else { "needs a better model" };
                    println!("       - {v:?}  [{who}]");
                }
            }
            let r = Report { scores };
            println!("\n{}", r.summary());
            let inv = r.invented();
            if !inv.is_empty() {
                println!("invented entities (never supplied, never requested):");
                for e in inv.iter().take(10) { println!("  {e}"); }
            }
        }
        _ => {
            eprintln!("usage:\n  agent-score --prompts          # emit the 15 cases\n  \
agent-score --score <dir>      # score <dir>/<case_name>.txt\n\n\
Violations marked 'runtime can prevent' are protocol failures a bigger model will NOT fix — they \
come from asking a model to simulate a state machine. Those are the ones to enforce in the loop.");
        }
    }
}
