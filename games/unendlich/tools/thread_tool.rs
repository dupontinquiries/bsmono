// thread_tool.rs — extractor / classifier / merger for the unendlich worker pipeline.
//
// STANDALONE: build with bare rustc (no cargo/deps):  rustc -O tools/thread_tool.rs -o /tmp/tt
// Run from the repo root. Shells out to `zip`/`unzip`/`patch`/`tar`.
//
// SUBCOMMANDS
//   extract --manifest <f> [--out threadwork] [--base .]
//       Dump one bundle per stanza into a LABELED working dir (<out>/), and write <out>/INDEX.txt.
//         mode: zip   -> <out>/<ID>.instr.zip  (base minus target/ + WORKER_ID + WORKER_BRIEF.md)
//                        for a worker WITH a workspace (Claude session)
//         mode: embed -> <out>/<ID>.brief.md   (preamble + brief + inlined file contents)
//                        for a worker WITHOUT a workspace (local ollama) — paste this in
//
//   scan --dir <d> [--write]
//       Classify everything in <d> as INSTRUCTION (a dispatch bundle) vs RESULT (a worker
//       return: patch / SOLUTION zip / raw .rs) vs IGNORE. Prints a table; --write saves
//       <d>/INDEX.txt. Lets you dump dispatches AND returns into one folder and sort them.
//
//   merge --returns <d> [--base .] [--out unendlich-super-merged.zip]
//       Classify <d>, apply only RESULTs (patches via `patch -p1 --forward`, dry-run-gated;
//       zips/raw .rs copied over base), AUTO-SKIP INSTRUCTION bundles, and roll ONE super zip
//       (+ MERGE_LOG.txt inside). Mechanical only — coordinator still `cargo check` after.
//
// MANIFEST: stanzas split by a line of `---`; `#` lines ignored; keys id/mode/brief/embed.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const PREAMBLE: &str = "\
# You are a WORKER thread, not the coordinator.
# - Ignore COORDINATOR_BOOTSTRAP.md; do ONLY the task below.
# - Raw changed file(s) at their repo paths OR a unified patch are both fine.
# - Don't block on HARNESS GREEN; don't touch coordinator shaders
#   (src/shaders/froxel_fill.comp, src/shaders/chunk.frag).
# - Don't reorder CameraUbo. Don't cache once-per-frame runtime-tunable env flags.
# - If you have a workspace: self-validate `cargo check -p unendlich-engine --features headless`.
# - RETURN: name your result after your WORKER_ID (e.g. <ID>-SOLUTION.zip or CHANGES-<ID>.patch).
";

const EMBED_PREAMBLE: &str = "\
# WORKER BRIEF (no-workspace / embedded-context edition)
# You do NOT have a filesystem. The relevant source files are inlined verbatim below.
# Do the task, then RETURN one of:
#   (a) a unified diff (`diff -u`-style) per changed file with correct a/<path> b/<path> headers, OR
#   (b) the FULL edited contents of each changed file, each in a fenced block whose first line is
#       `// FILE: <repo-relative-path>` so the coordinator can drop it back at its path.
# Constraints: don't touch coordinator shaders; don't reorder CameraUbo; behavior-preserving
# unless the task says otherwise. Keep the change minimal and self-contained.
";

fn sh(cmd: &str) -> Result<(), String> {
    let st = Command::new("bash").arg("-c").arg(cmd).status().map_err(|e| e.to_string())?;
    if st.success() { Ok(()) } else { Err(format!("command failed ({}): {}", st, cmd)) }
}
fn shout(cmd: &str) -> String {
    Command::new("bash").arg("-c").arg(cmd).output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}
fn arg(args: &[String], key: &str) -> Option<String> {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1).cloned())
}
fn has_flag(args: &[String], key: &str) -> bool { args.iter().any(|a| a == key) }
fn abs(p: &str) -> String {
    let pb = PathBuf::from(p);
    if pb.is_absolute() { p.to_string() }
    else { std::env::current_dir().unwrap().join(pb).to_string_lossy().to_string() }
}

// ---- classification: instruction bundle vs worker result -------------------
// Returns (ROLE, why). ROLE is INSTRUCTION | RESULT | IGNORE.
fn classify(path: &str) -> (&'static str, String) {
    let lower = path.to_lowercase();
    if lower.ends_with(".patch") || lower.ends_with(".diff") {
        return ("RESULT", "unified patch".into());
    }
    if lower.ends_with(".rs") {
        return ("RESULT", "raw changed source".into());
    }
    if lower.ends_with(".zip") {
        let names = shout(&format!("unzip -Z1 '{}' 2>/dev/null", path));
        let line_is = |want: &str| names.lines().any(|l| {
            let b = l.rsplit('/').next().unwrap_or(l);
            b == want
        });
        let any_contains = |sub: &str| names.to_lowercase().contains(sub);
        let has_brief = line_is("WORKER_BRIEF.md") || line_is("WORKER_ID");
        let has_solution = any_contains("solution");
        let has_src = names.lines().any(|l| (l.contains("/src/") || l.contains("crates/")) && l.ends_with(".rs"));
        let count = names.lines().count();
        if has_brief && !has_solution {
            return ("INSTRUCTION", format!("dispatch bundle (WORKER_BRIEF + {} files)", count));
        }
        if has_solution { return ("RESULT", "SOLUTION zip".into()); }
        if has_src { return ("RESULT", "zip of changed sources".into()); }
        return ("IGNORE", "zip without sources or brief".into());
    }
    if lower.ends_with(".md") {
        let body = fs::read_to_string(path).unwrap_or_default();
        if body.contains("You are a WORKER") || body.contains("WORKER BRIEF") {
            return ("INSTRUCTION", "brief markdown".into());
        }
        return ("IGNORE", "markdown (not a brief)".into());
    }
    ("IGNORE", "unrecognized".into())
}

fn list_dir_sorted(dir: &str) -> Result<Vec<PathBuf>, String> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir).map_err(|e| format!("read {dir}: {e}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.file_name().map(|n| n != "INDEX.txt").unwrap_or(true))
        .collect();
    v.sort();
    Ok(v)
}

fn copy_base(base: &str, work: &str) -> Result<(), String> {
    let _ = fs::remove_dir_all(work);
    fs::create_dir_all(work).map_err(|e| e.to_string())?;
    sh(&format!(
        "cd '{base}' && tar --exclude=./target --exclude=./.git --exclude='*.zip' -cf - . | tar -xf - -C '{work}'"
    ))
}

/// Copy every *.rs under an extracted return tree onto the work tree, anchoring
/// each path at its `crates/` (or `src/`) segment so it lands correctly whether
/// or not the zip wrapped its contents in a top-level dir.
fn copy_sources(tmp: &str, work: &str) -> Result<(), String> {
    sh(&format!(
        "cd '{tmp}' && find . \\( -path '*/crates/*' -o -path '*crates/*' -o -path '*/src/*' \\) -name '*.rs' -print0 \
         | while IFS= read -r -d '' f; do \
             case \"$f\" in \
               *crates/*) rel=\"crates/${{f##*crates/}}\";; \
               *src/*)    rel=\"src/${{f##*src/}}\";; \
               *)         rel=\"${{f#./}}\";; \
             esac; \
             dst='{work}'/\"$rel\"; mkdir -p \"$(dirname \"$dst\")\"; cp \"$f\" \"$dst\"; \
           done"
    ))
}

type Stanza = BTreeMap<String, String>;
fn parse_manifest(text: &str) -> Vec<Stanza> {
    let mut out = Vec::new();
    for block in text.split("\n---") {
        let mut m = Stanza::new();
        for line in block.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') { continue; }
            if let Some((k, v)) = l.split_once(':') { m.insert(k.trim().to_string(), v.trim().to_string()); }
        }
        if m.contains_key("id") { out.push(m); }
    }
    out
}

fn extract(args: &[String]) -> Result<(), String> {
    let manifest = arg(args, "--manifest").ok_or("extract needs --manifest <file>")?;
    let out = arg(args, "--out").unwrap_or_else(|| "threadwork".to_string());
    fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let out = abs(&out);
    let base = abs(&arg(args, "--base").unwrap_or_else(|| ".".to_string()));
    let text = fs::read_to_string(&manifest).map_err(|e| format!("read {manifest}: {e}"))?;
    let stanzas = parse_manifest(&text);
    if stanzas.is_empty() { return Err("no stanzas with an `id:` found in manifest".into()); }

    let mut index = String::from("# INDEX — labeled dispatch working dir\n# ROLE\tMODE\tID\tFILE\n");
    for s in &stanzas {
        let id = s.get("id").unwrap();
        let mode = s.get("mode").map(String::as_str).unwrap_or("zip");
        let brief_path = s.get("brief").ok_or_else(|| format!("{id}: missing `brief:`"))?;
        let brief = fs::read_to_string(brief_path).map_err(|e| format!("{id}: read {brief_path}: {e}"))?;
        match mode {
            "zip" => {
                fs::write(format!("{base}/WORKER_ID"), id).map_err(|e| e.to_string())?;
                fs::write(format!("{base}/WORKER_BRIEF.md"),
                    format!("{PREAMBLE}\n## TASK {id}\n\n{brief}\n")).map_err(|e| e.to_string())?;
                let fname = format!("{id}.instr.zip");
                let zip = format!("{out}/{fname}");
                let _ = fs::remove_file(&zip);
                sh(&format!("cd '{base}' && zip -q -r '{zip}' . -x './target/*' -x './.git/*' -x './*.zip'"))?;
                let _ = fs::remove_file(format!("{base}/WORKER_ID"));
                let _ = fs::remove_file(format!("{base}/WORKER_BRIEF.md"));
                index.push_str(&format!("INSTRUCTION\tzip\t{id}\t{fname}\n"));
                println!("  [instr.zip] {fname}");
            }
            "embed" => {
                let mut doc = String::new();
                doc.push_str(EMBED_PREAMBLE);
                doc.push_str(&format!("\n## TASK {id}\n\n{brief}\n\n---\n## EMBEDDED SOURCE FILES\n"));
                for f in s.get("embed").map(String::as_str).unwrap_or("").split(',').map(str::trim).filter(|f| !f.is_empty()) {
                    let p = format!("{base}/{f}");
                    let bodyf = fs::read_to_string(&p).map_err(|e| format!("{id}: embed {p}: {e}"))?;
                    let lang = if f.ends_with(".rs") {"rust"} else if f.ends_with(".comp")||f.ends_with(".vert")||f.ends_with(".frag") {"glsl"} else {""};
                    doc.push_str(&format!("\n### FILE: {f}\n```{lang}\n{bodyf}\n```\n"));
                }
                let fname = format!("{id}.brief.md");
                fs::write(format!("{out}/{fname}"), doc).map_err(|e| e.to_string())?;
                index.push_str(&format!("INSTRUCTION\tembed\t{id}\t{fname}\n"));
                println!("  [brief.md]  {fname}  (paste into the no-workspace session)");
            }
            other => return Err(format!("{id}: unknown mode `{other}` (use zip|embed)")),
        }
    }
    fs::write(format!("{out}/INDEX.txt"), &index).map_err(|e| e.to_string())?;
    println!("extracted {} bundle(s) into {out}/  (+ INDEX.txt)", stanzas.len());
    Ok(())
}

fn scan(args: &[String]) -> Result<(), String> {
    let dir = arg(args, "--dir").ok_or("scan needs --dir <dir>")?;
    let write = has_flag(args, "--write");
    let mut index = String::from("# INDEX — classified\n# ROLE\tWHY\tFILE\n");
    let (mut ni, mut nr, mut ng) = (0, 0, 0);
    for p in list_dir_sorted(&dir)? {
        let ps = p.to_string_lossy().to_string();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let (role, why) = classify(&ps);
        match role { "INSTRUCTION" => ni += 1, "RESULT" => nr += 1, _ => ng += 1 }
        println!("  {:<11} {:<32} {}", role, name, why);
        index.push_str(&format!("{role}\t{why}\t{name}\n"));
    }
    println!("-- {ni} instruction, {nr} result, {ng} ignored --");
    if write { fs::write(format!("{}/INDEX.txt", dir), &index).map_err(|e| e.to_string())?; println!("wrote {dir}/INDEX.txt"); }
    Ok(())
}

fn merge(args: &[String]) -> Result<(), String> {
    let returns = arg(args, "--returns").ok_or("merge needs --returns <dir>")?;
    let base = abs(&arg(args, "--base").unwrap_or_else(|| ".".to_string()));
    let out = abs(&arg(args, "--out").unwrap_or_else(|| "unendlich-super-merged.zip".to_string()));
    let work = abs(".tt_merge_work");
    copy_base(&base, &work)?;
    let mut log = String::from("# MERGE_LOG (mechanical apply — coordinator still compile-checks)\n");

    for p in list_dir_sorted(&returns)? {
        let ps = p.to_string_lossy().to_string();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let (role, why) = classify(&ps);
        if role == "INSTRUCTION" { log.push_str(&format!("SKIP-INSTR {name}  ({why})\n")); continue; }
        if role == "IGNORE"      { log.push_str(&format!("IGNORED    {name}  ({why})\n")); continue; }
        // RESULT:
        if ps.to_lowercase().ends_with(".patch") || ps.to_lowercase().ends_with(".diff") {
            let absf = fs::canonicalize(&p).map_err(|e| e.to_string())?;
            let dry = sh(&format!("cd '{work}' && patch -p1 --forward --dry-run < '{}'", absf.display()));
            if dry.is_ok() {
                sh(&format!("cd '{work}' && patch -p1 --forward < '{}'", absf.display()))?;
                log.push_str(&format!("APPLIED    {name}  (patch)\n"));
            } else {
                log.push_str(&format!("SKIP-FAIL  {name}  (patch dry-run failed — resolve by hand)\n"));
            }
        } else if ps.to_lowercase().ends_with(".zip") {
            let tmp = format!("{work}/.unz");
            let _ = fs::remove_dir_all(&tmp); let _ = fs::create_dir_all(&tmp);
            sh(&format!("unzip -oq '{}' -d '{tmp}'", ps)).ok();
            // Prefer an embedded patch if present; else copy changed .rs over the base.
            let patch_in = shout(&format!("find '{tmp}' -name '*.patch' | head -1")).trim().to_string();
            if !patch_in.is_empty() {
                let dry = sh(&format!("cd '{work}' && patch -p1 --forward --dry-run < '{patch_in}'"));
                if dry.is_ok() { sh(&format!("cd '{work}' && patch -p1 --forward < '{patch_in}'"))?;
                    log.push_str(&format!("APPLIED    {name}  (patch inside zip)\n")); }
                else { copy_sources(&tmp, &work)?;
                    log.push_str(&format!("APPLIED    {name}  (zip patch failed; raw .rs copied)\n")); }
            } else {
                copy_sources(&tmp, &work)?;
                log.push_str(&format!("APPLIED    {name}  (raw .rs copied over base)\n"));
            }
            let _ = fs::remove_dir_all(&tmp);
        } else if ps.to_lowercase().ends_with(".rs") {
            log.push_str(&format!("MANUAL     {name}  (loose .rs — unknown target path; place by hand)\n"));
        }
    }
    fs::write(format!("{work}/MERGE_LOG.txt"), &log).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&out);
    sh(&format!("cd '{work}' && zip -q -r '{out}' . -x './target/*' -x './.git/*'"))?;
    print!("{log}");
    println!("rolled {out}  (work tree left in {work}/ for inspection)");
    Ok(())
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let res = match argv.get(1).map(String::as_str) {
        Some("extract") => extract(&argv[2..]),
        Some("scan")    => scan(&argv[2..]),
        Some("merge")   => merge(&argv[2..]),
        _ => { eprintln!("usage:\n  thread_tool extract --manifest <f> [--out threadwork] [--base .]\n  thread_tool scan --dir <d> [--write]\n  thread_tool merge --returns <d> [--base .] [--out unendlich-super-merged.zip]"); std::process::exit(2); }
    };
    if let Err(e) = res { eprintln!("error: {e}"); std::process::exit(1); }
}
