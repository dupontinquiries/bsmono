//! `unendlich-launcher` — CLI front end for `crate::launcher`.
//!
//! c.11 implements the `run` subcommand. c.12 will add `compare`,
//! `run-suite`, `gc`, `list`, `install`.
//!
//! ## Usage
//!
//! ```text
//! unendlich-launcher run <SPEC> [--version <REF>] [--repo <PATH>]
//!
//!   Run a .unendtest spec against a git ref of the engine.
//!
//!   --version <REF>   Git ref to run against (default: @HEAD).
//!                     Examples: @HEAD, @v0.24.2-c.7, @abc1234,
//!                     @my-feature-branch. Leading @ is optional.
//!   --repo    <PATH>  Path to the source repository (default: cwd).
//! ```

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_help();
        std::process::exit(if args.is_empty() { 1 } else { 0 });
    }

    match args[0].as_str() {
        "run" => cmd_run(&args[1..]),
        "compare" => cmd_compare(&args[1..]),
        "run-suite" => cmd_run_suite(&args[1..]),
        "diag" => cmd_diag(&args[1..]),
        "--version" | "-V" => {
            println!("unendlich-launcher {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        cmd => {
            eprintln!("Unknown command: {}", cmd);
            print_help();
            std::process::exit(1);
        }
    }
}

fn print_help() {
    println!("unendlich-launcher {} — version-aware test runner", env!("CARGO_PKG_VERSION"));
    println!();
    println!("USAGE:");
    println!("    unendlich-launcher run <SPEC> [--version <REF>] [--repo <PATH>]");
    println!("    unendlich-launcher compare <SPEC> --versions <REF,REF,...> [--repo <PATH>]");
    println!("    unendlich-launcher compare --results <FILE> <FILE> [<FILE>...]");
    println!("    unendlich-launcher run-suite [--specs-dir <DIR>] [--version <REF>] [--repo <PATH>]");
    println!("    unendlich-launcher diag [<UUID>] [--log-lines <N>]");
    println!();
    println!("SUBCOMMANDS:");
    println!("    run        Run a single spec against a single version.");
    println!("    compare    Run a spec against multiple versions (or load existing");
    println!("               result files) and print a side-by-side diff table.");
    println!("    run-suite  Run every .unendtest in tests/specs/ against a version.");
    println!("    diag       Summarise the latest run (or a specific UUID) — workload");
    println!("               context, assertion verdicts, recent engine log, key timings.");
    println!("               One-paste diagnostic for sharing with collaborators.");
    println!();
    println!("OPTIONS:");
    println!("    --version <REF>    Git ref to run against (default: @HEAD)");
    println!("                       Examples: @HEAD, @v0.24.2-c.7, @abc1234");
    println!("    --versions <LIST>  Comma-separated git refs (for compare).");
    println!("                       Example: @v0.24.2-c.7,@v0.24.2-c.10,@HEAD");
    println!("    --results <LIST>   Paths to existing .unendresult.json files");
    println!("                       (for compare, skips running anything).");
    println!("    --repo <PATH>      Source repository path (default: cwd)");
    println!("    --specs-dir <DIR>  Directory of .unendtest files (default: tests/specs)");
    println!();
    println!("ENVIRONMENT:");
    println!("    UNENDLICH_LAUNCHER_HOME    Override ~/.unendlich location");
    println!();
    println!("EXIT CODES:");
    println!("    0    Run succeeded, all assertions passed");
    println!("    1    Run succeeded, one or more assertions failed");
    println!("    2    Launcher couldn't set up or build");
    println!("    3    Binary aborted before writing a result file");
}

/// Parse args for the `run` subcommand. Returns (spec_path, version,
/// repo_dir). Exits the process on parse error.
struct RunArgs {
    spec_path: PathBuf,
    version: String,
    repo_dir: PathBuf,
}

fn parse_run_args(args: &[String]) -> RunArgs {
    let mut spec_path: Option<PathBuf> = None;
    let mut version = "@HEAD".to_string();
    let mut repo_dir: Option<PathBuf> = None;

    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--version" => {
                if i + 1 >= args.len() {
                    eprintln!("--version requires an argument");
                    std::process::exit(2);
                }
                version = args[i + 1].clone();
                i += 2;
            }
            "--repo" => {
                if i + 1 >= args.len() {
                    eprintln!("--repo requires an argument");
                    std::process::exit(2);
                }
                repo_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            other if other.starts_with("--") => {
                eprintln!("Unknown flag: {}", other);
                std::process::exit(2);
            }
            _ => {
                if spec_path.is_some() {
                    eprintln!("Unexpected positional argument: {}", args[i]);
                    std::process::exit(2);
                }
                spec_path = Some(PathBuf::from(&args[i]));
                i += 1;
            }
        }
    }

    let spec_path = spec_path.unwrap_or_else(|| {
        eprintln!("run: missing SPEC argument");
        std::process::exit(2);
    });
    let repo_dir = repo_dir.unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|e| {
            eprintln!("Could not get current directory: {}", e);
            std::process::exit(2);
        })
    });

    RunArgs { spec_path, version, repo_dir }
}

fn cmd_run(args: &[String]) {
    let a = parse_run_args(args);

    // Absolute path for the spec so the spawned binary (which runs
    // from a possibly-different cwd) can find it.
    let spec_path = match std::fs::canonicalize(&a.spec_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Spec file not found: {} ({})", a.spec_path.display(), e);
            std::process::exit(2);
        }
    };
    let repo_dir = match std::fs::canonicalize(&a.repo_dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Repo dir not found: {} ({})", a.repo_dir.display(), e);
            std::process::exit(2);
        }
    };

    println!("unendlich-launcher: resolving version {}...", a.version);
    let result = unendlich_launcher::run(&repo_dir, &a.version, &spec_path);

    match result {
        Ok((commit, outcome)) => {
            println!();
            println!("┌─ Run complete ─────────────────────────────────");
            println!("│ commit:    {}", commit);
            println!("│ result:    {}", outcome.result_path.display());
            if let Some(t) = &outcome.triage {
                // c.12-e: surface triage outcome. Healthy runs delete the
                // events file to save disk; incidents keep it for forensics.
                if t.events_kept {
                    println!("│ events:    {}", outcome.events_path.display());
                    if t.had_incident {
                        println!("│ triage:    INCIDENT — worst p50 was {:.1}ms > {:.0}ms threshold",
                            t.worst_p50_ms, t.threshold_ms);
                        println!("│            ({} steady windows examined)", t.steady_windows);
                    } else {
                        println!("│ triage:    kept (run too short to classify)");
                    }
                } else {
                    println!("│ events:    (deleted — run was healthy, worst p50={:.1}ms)",
                        t.worst_p50_ms);
                    println!("│ triage:    healthy ({} steady windows under {:.0}ms threshold)",
                        t.steady_windows, t.threshold_ms);
                }
            } else {
                println!("│ events:    {}", outcome.events_path.display());
                println!("│ triage:    (events file missing — no triage performed)");
            }
            println!("│ engine log: {}", outcome.engine_log_path.display());
            println!("│ exit:      {:?}", outcome.exit_status);
            println!("│ assertions: {}",
                if outcome.assertions_passed { "PASSED" } else { "FAILED" });
            println!("├─ Convenience symlinks (always point at most recent run) ─");
            println!("│   ~/.unendlich/latest-result.unendresult.json");
            println!("│   ~/.unendlich/latest-engine.log");
            println!("│   ~/.unendlich/latest-events.jsonl (if kept)");
            println!("│   Use `unendlich-launcher diag` to summarise the latest run.");
            println!("└────────────────────────────────────────────────");
            std::process::exit(if outcome.assertions_passed { 0 } else { 1 });
        }
        Err(e) => {
            eprintln!();
            eprintln!("unendlich-launcher: error");
            eprintln!("{}", e);
            // Use 3 for "binary aborted" (BinaryFailed status 2) and
            // 2 for setup / build errors.
            let code = match e {
                unendlich_launcher::LauncherError::BinaryFailed { .. } => 3,
                _ => 2,
            };
            std::process::exit(code);
        }
    }
}

// ============================================================
// compare
// ============================================================

/// Parsed args for `compare`. Either `--versions` (run spec against
/// each) or `--results` (load existing result files) must be set,
/// not both. When --versions is used, --spec is required.
struct CompareArgs {
    spec_path: Option<PathBuf>,
    versions: Vec<String>,
    result_files: Vec<PathBuf>,
    repo_dir: PathBuf,
}

fn parse_compare_args(args: &[String]) -> CompareArgs {
    let mut spec_path: Option<PathBuf> = None;
    let mut versions: Vec<String> = Vec::new();
    let mut result_files: Vec<PathBuf> = Vec::new();
    let mut repo_dir: Option<PathBuf> = None;

    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--versions" => {
                if i + 1 >= args.len() {
                    eprintln!("--versions requires a comma-separated argument");
                    std::process::exit(2);
                }
                versions = args[i + 1].split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                i += 2;
            }
            "--results" => {
                // Consumes all subsequent positional args until a flag
                // or end. This is the only place we do variadic args.
                i += 1;
                while i < args.len() && !args[i].starts_with("--") {
                    result_files.push(PathBuf::from(&args[i]));
                    i += 1;
                }
            }
            "--repo" => {
                if i + 1 >= args.len() {
                    eprintln!("--repo requires an argument");
                    std::process::exit(2);
                }
                repo_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            other if other.starts_with("--") => {
                eprintln!("Unknown flag: {}", other);
                std::process::exit(2);
            }
            _ => {
                if spec_path.is_some() {
                    eprintln!("Unexpected positional argument: {}", args[i]);
                    std::process::exit(2);
                }
                spec_path = Some(PathBuf::from(&args[i]));
                i += 1;
            }
        }
    }

    // Validation.
    let have_versions = !versions.is_empty();
    let have_results = !result_files.is_empty();
    if have_versions && have_results {
        eprintln!("compare: pass either --versions OR --results, not both");
        std::process::exit(2);
    }
    if !have_versions && !have_results {
        eprintln!("compare: pass --versions <REF,REF,...> or --results <FILE> <FILE>...");
        std::process::exit(2);
    }
    if have_versions && spec_path.is_none() {
        eprintln!("compare: --versions requires a SPEC argument");
        std::process::exit(2);
    }
    if have_versions && versions.len() < 2 {
        eprintln!("compare: --versions needs at least 2 refs (got {})", versions.len());
        std::process::exit(2);
    }
    if have_results && result_files.len() < 2 {
        eprintln!("compare: --results needs at least 2 files (got {})", result_files.len());
        std::process::exit(2);
    }

    let repo_dir = repo_dir.unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|e| {
            eprintln!("Could not get current directory: {}", e);
            std::process::exit(2);
        })
    });

    CompareArgs { spec_path, versions, result_files, repo_dir }
}

fn cmd_compare(args: &[String]) {
    let a = parse_compare_args(args);

    let columns: Vec<unendlich_launcher::CompareColumn> = if !a.versions.is_empty() {
        // Run path: launch the spec against each version.
        let spec_path = a.spec_path.as_ref().unwrap();
        let spec_path = match std::fs::canonicalize(spec_path) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Spec file not found: {} ({})", spec_path.display(), e);
                std::process::exit(2);
            }
        };
        let repo_dir = match std::fs::canonicalize(&a.repo_dir) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Repo dir not found: {} ({})", a.repo_dir.display(), e);
                std::process::exit(2);
            }
        };

        let mut cols: Vec<unendlich_launcher::CompareColumn> = Vec::new();
        for ver in &a.versions {
            println!("unendlich-launcher: running {} ...", ver);
            match unendlich_launcher::run(&repo_dir, ver, &spec_path) {
                Ok((_commit, outcome)) => {
                    match unendlich_launcher::load_result(&outcome.result_path) {
                        Ok(r) => cols.push(unendlich_launcher::CompareColumn {
                            label: ver.clone(),
                            result: Some(r),
                        }),
                        Err(e) => {
                            eprintln!("  (could not load result for {}: {})", ver, e);
                            cols.push(unendlich_launcher::CompareColumn {
                                label: ver.clone(),
                                result: None,
                            });
                        }
                    }
                }
                Err(e) => {
                    eprintln!("  (failed to run {}: {})", ver, e);
                    cols.push(unendlich_launcher::CompareColumn {
                        label: ver.clone(),
                        result: None,
                    });
                }
            }
        }
        cols
    } else {
        // Direct-load path: read each result file.
        a.result_files.iter().map(|p| {
            let label = p.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string());
            let result = unendlich_launcher::load_result(p).ok();
            unendlich_launcher::CompareColumn { label, result }
        }).collect()
    };

    let report = unendlich_launcher::build_compare_report(columns, None);
    println!();
    print!("{}", unendlich_launcher::render_compare_report(&report));

    // Exit code: 0 if everyone passed, 1 if any present result failed
    // assertions, 2 if any column couldn't be loaded.
    let any_failed = report.assertion_pass.iter().any(|p| p == &Some(false));
    let any_missing = report.assertion_pass.iter().any(|p| p.is_none());
    let code = if any_missing { 2 } else if any_failed { 1 } else { 0 };
    std::process::exit(code);
}

// ============================================================
// run-suite
// ============================================================

struct RunSuiteArgs {
    specs_dir: PathBuf,
    version: String,
    repo_dir: PathBuf,
}

fn parse_run_suite_args(args: &[String]) -> RunSuiteArgs {
    let mut specs_dir: Option<PathBuf> = None;
    let mut version = "@HEAD".to_string();
    let mut repo_dir: Option<PathBuf> = None;

    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--specs-dir" => {
                if i + 1 >= args.len() {
                    eprintln!("--specs-dir requires an argument");
                    std::process::exit(2);
                }
                specs_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--version" => {
                if i + 1 >= args.len() {
                    eprintln!("--version requires an argument");
                    std::process::exit(2);
                }
                version = args[i + 1].clone();
                i += 2;
            }
            "--repo" => {
                if i + 1 >= args.len() {
                    eprintln!("--repo requires an argument");
                    std::process::exit(2);
                }
                repo_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            other => {
                eprintln!("run-suite: unexpected argument: {}", other);
                std::process::exit(2);
            }
        }
    }

    let specs_dir = specs_dir.unwrap_or_else(|| PathBuf::from("tests/specs"));
    let repo_dir = repo_dir.unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|e| {
            eprintln!("Could not get current directory: {}", e);
            std::process::exit(2);
        })
    });

    RunSuiteArgs { specs_dir, version, repo_dir }
}

fn cmd_run_suite(args: &[String]) {
    let a = parse_run_suite_args(args);

    let specs_dir = match std::fs::canonicalize(&a.specs_dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Specs dir not found: {} ({})", a.specs_dir.display(), e);
            std::process::exit(2);
        }
    };
    let repo_dir = match std::fs::canonicalize(&a.repo_dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Repo dir not found: {} ({})", a.repo_dir.display(), e);
            std::process::exit(2);
        }
    };

    println!("unendlich-launcher run-suite");
    println!("  specs dir: {}", specs_dir.display());
    println!("  version:   {}", a.version);
    println!();

    match unendlich_launcher::run_suite(&repo_dir, &a.version, &specs_dir) {
        Ok((commit, outcomes)) => {
            println!("Commit: {}", commit);
            println!();
            let mut pass = 0;
            let mut fail = 0;
            let mut err = 0;
            for o in &outcomes {
                match o.assertions_passed {
                    Some(true) => { println!("  PASS  {}", o.spec_name); pass += 1; }
                    Some(false) => { println!("  FAIL  {}", o.spec_name); fail += 1; }
                    None => {
                        println!("  ERR   {} ({})", o.spec_name,
                            o.error.as_deref().unwrap_or("?"));
                        err += 1;
                    }
                }
            }
            println!();
            println!("Total: {} pass, {} fail, {} error", pass, fail, err);
            std::process::exit(if fail > 0 || err > 0 { 1 } else { 0 });
        }
        Err(e) => {
            eprintln!("run-suite: setup failed: {}", e);
            std::process::exit(2);
        }
    }
}

// ============================================================
// diag: post-run summary, one-paste for collaborators
// ============================================================

/// Default tail length of engine log to include in `diag`.
const DIAG_DEFAULT_LOG_LINES: usize = 60;

fn cmd_diag(args: &[String]) {
    let mut uuid_arg: Option<String> = None;
    let mut log_lines: usize = DIAG_DEFAULT_LOG_LINES;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--log-lines" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("diag: --log-lines requires a number");
                    std::process::exit(2);
                }
                log_lines = match args[i].parse::<usize>() {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!("diag: --log-lines: not a number: {}", args[i]);
                        std::process::exit(2);
                    }
                };
            }
            other if !other.starts_with("--") && uuid_arg.is_none() => {
                uuid_arg = Some(other.to_string());
            }
            other => {
                eprintln!("diag: unrecognized argument: {}", other);
                eprintln!("Usage: unendlich-launcher diag [<UUID>] [--log-lines <N>]");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    // Resolve the run directory. Three modes:
    //   1. uuid given: ~/.unendlich/runs/<uuid>/
    //   2. no uuid: follow ~/.unendlich/latest-result.unendresult.json
    //      back to its dir.
    let home = match std::env::var("HOME") {
        Ok(h) => std::path::PathBuf::from(h),
        Err(_) => {
            eprintln!("diag: HOME not set");
            std::process::exit(2);
        }
    };
    let unendlich_root = home.join(".unendlich");

    let result_path: std::path::PathBuf = if let Some(uuid) = &uuid_arg {
        // Strip an "r-" prefix if the user included it.
        let bare = uuid.strip_prefix("r-").unwrap_or(uuid);
        unendlich_root.join("runs").join(format!("r-{}", bare))
            .join("result.unendresult.json")
    } else {
        let latest = unendlich_root.join("latest-result.unendresult.json");
        if !latest.exists() {
            eprintln!("diag: no `latest-result.unendresult.json` symlink at {}", latest.display());
            eprintln!("diag: run something first, or pass an explicit UUID.");
            std::process::exit(2);
        }
        // Follow the symlink so subsequent prints show the real path.
        match std::fs::canonicalize(&latest) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("diag: cannot resolve latest symlink {}: {}",
                    latest.display(), e);
                std::process::exit(2);
            }
        }
    };

    if !result_path.exists() {
        eprintln!("diag: no result file at {}", result_path.display());
        std::process::exit(2);
    }

    let run_dir = result_path.parent().expect("result file has parent dir");

    // Read result file.
    let result_text = match std::fs::read_to_string(&result_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("diag: cannot read {}: {}", result_path.display(), e);
            std::process::exit(2);
        }
    };
    let result: serde_json::Value = match serde_json::from_str(&result_text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("diag: malformed result file: {}", e);
            std::process::exit(2);
        }
    };

    println!("┌─ diag: {} ─", run_dir.file_name()
        .and_then(|s| s.to_str()).unwrap_or("?"));
    println!("│ result:    {}", result_path.display());

    // Header info: spec, binary version, duration, exit.
    let spec_name = result.get("spec_name").and_then(|v| v.as_str()).unwrap_or("?");
    let binary_version = result.get("binary_version").and_then(|v| v.as_str()).unwrap_or("?");
    let duration_secs = result.get("duration_secs").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let shutdown_reason = result.get("shutdown_reason").and_then(|v| v.as_str()).unwrap_or("?");
    let passed = result.get("passed").and_then(|v| v.as_bool()).unwrap_or(false);
    println!("│ spec:      {}", spec_name);
    println!("│ version:   {}", binary_version);
    println!("│ duration:  {:.2}s  ({})", duration_secs, shutdown_reason);
    println!("│ verdict:   {}", if passed { "PASSED" } else { "FAILED" });
    println!("└─");
    println!();

    // Assertion verdicts.
    if let Some(asserts) = result.get("assertions").and_then(|v| v.as_array()) {
        println!("Assertions:");
        for a in asserts {
            let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let passed = a.get("passed").and_then(|v| v.as_bool()).unwrap_or(false);
            let reason = a.get("reason").and_then(|v| v.as_str()).unwrap_or("");
            let marker = if passed { "PASS" } else { "FAIL" };
            if reason.is_empty() {
                println!("  {}  {}", marker, name);
            } else {
                println!("  {}  {} — {}", marker, name, reason);
            }
        }
        println!();
    }

    // Workload context.
    if let Some(stats) = result.get("stats").and_then(|v| v.as_object()) {
        if let Some(workload) = stats.get("workload").and_then(|v| v.as_object()) {
            println!("Workload:");
            for (label, field) in &[("loaded", "loaded"), ("rendering", "rendering")] {
                if let Some(d) = workload.get(*field).and_then(|v| v.as_object()) {
                    let n = d.get("n").and_then(|v| v.as_u64()).unwrap_or(0);
                    if n == 0 { continue; }
                    let mn = d.get("min").and_then(|v| v.as_u64()).unwrap_or(0);
                    let p50 = d.get("p50").and_then(|v| v.as_u64()).unwrap_or(0);
                    let mx = d.get("max").and_then(|v| v.as_u64()).unwrap_or(0);
                    println!("  {:10} min={} p50={} max={}  (n={})", label, mn, p50, mx, n);
                }
            }
            if let Some(snap) = workload.get("snapshot").and_then(|v| v.as_object()) {
                if let Some(s) = snap.get("start").and_then(|v| v.as_object()) {
                    let lc = s.get("loaded_chunks").and_then(|v| v.as_u64()).unwrap_or(0);
                    let nb = s.get("non_air_blocks").and_then(|v| v.as_u64()).unwrap_or(0);
                    println!("  start      loaded={} blocks={}", lc, nb);
                }
                if let Some(e) = snap.get("end").and_then(|v| v.as_object()) {
                    let lc = e.get("loaded_chunks").and_then(|v| v.as_u64()).unwrap_or(0);
                    let nb = e.get("non_air_blocks").and_then(|v| v.as_u64()).unwrap_or(0);
                    println!("  end        loaded={} blocks={}", lc, nb);
                }
                if let Some(d) = snap.get("delta").and_then(|v| v.as_object()) {
                    let dc = d.get("loaded_chunks").and_then(|v| v.as_i64()).unwrap_or(0);
                    let db = d.get("non_air_blocks").and_then(|v| v.as_i64()).unwrap_or(0);
                    let dcs = if dc >= 0 { "+" } else { "" };
                    let dbs = if db >= 0 { "+" } else { "" };
                    println!("  delta      loaded={}{} blocks={}{}", dcs, dc, dbs, db);
                }
            }
            println!();
        }

        // Top zones by mean.
        let mut zones: Vec<(String, String, f64, f64, f64)> = Vec::new();
        for cat in &["gpu", "cpu"] {
            if let Some(cm) = stats.get(*cat).and_then(|v| v.as_object()) {
                for (zname, z) in cm.iter() {
                    let n = z.get("n").and_then(|v| v.as_u64()).unwrap_or(0);
                    if n == 0 { continue; }
                    let mean = z.get("mean_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let p50 = z.get("p50_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let p99 = z.get("p99_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    zones.push((cat.to_string(), zname.clone(), mean, p50, p99));
                }
            }
        }
        zones.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        if !zones.is_empty() {
            println!("Top zones (by mean ms):");
            println!("  {:8} {:14} {:>10} {:>10} {:>10}",
                "cat", "zone", "mean_ms", "p50_ms", "p99_ms");
            for (cat, name, mean, p50, p99) in zones.iter().take(6) {
                println!("  {:8} {:14} {:>10.3} {:>10.3} {:>10.3}",
                    cat, name, mean, p50, p99);
            }
            println!();
        }
    }

    // Tail of engine.log.
    let log_path = run_dir.join("engine.log");
    if log_path.exists() {
        if let Ok(text) = std::fs::read_to_string(&log_path) {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(log_lines);
            let tail = &lines[start..];
            println!("Engine log (last {} of {} lines):", tail.len(), lines.len());
            for line in tail {
                println!("  {}", line);
            }
        }
    } else {
        println!("Engine log: (no engine.log in run dir — pre-c.13-instrumentation-followup run?)");
    }
}
