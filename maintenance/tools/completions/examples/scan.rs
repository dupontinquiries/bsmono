use mono_completions::*;
use std::{fs, process::Command};
fn walk(dir: &str, out: &mut Vec<String>, d: usize) {
    if d > 6 { return; }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path(); let s = p.to_string_lossy().to_string();
        if s.contains("/target/") || s.contains("/.git") { continue; }
        if p.is_dir() { walk(&s, out, d+1); } else if s.ends_with(".sh") { out.push(s); }
    }
}
fn refs(root: &str, name: &str, ext: &str) -> usize {
    Command::new("grep").args(["-rl", "--include", ext, name, root]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count()).unwrap_or(0)
}
fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let mut paths = Vec::new(); walk(&root, &mut paths, 0); paths.sort();
    let mut tools = Vec::new(); let mut advice = Vec::new();
    for p in &paths {
        let Ok(src) = fs::read_to_string(p) else { continue };
        let mut t = from_shell(p, &src);
        let b = t.name.clone();
        let docs = refs(&root, &b, "*.md");
        let scripts = refs(&root, &b, "*.sh").saturating_sub(1);
        t.role = role_of(docs, scripts);
        if let Some(a) = port_first(&t, src.lines().count()) { advice.push((b.clone(), a)); }
        tools.push(t);
    }
    let keep: Vec<Tool> = tools.iter().filter(|t| t.role.worth_completing()).cloned().collect();
    println!("{} scripts scanned, {} worth completing", tools.len(), keep.len());
    for r in [Role::EntryPoint, Role::Both, Role::Step, Role::Orphan] {
        println!("  {:>4}  {}", tools.iter().filter(|t| t.role == r).count(), r.name());
    }
    println!("\nport before blessing ({}):", advice.len());
    for (n, a) in advice.iter().take(4) { println!("  {n}: {}", &a[..a.len().min(88)]); }
    println!("\nindex: {} bytes", zsh_index(&keep, "mono").len());
}
