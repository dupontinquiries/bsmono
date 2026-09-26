//! project_check FILE.studio... -- print studio_core::journal::check findings
//! for each saved project, one line per finding, and a count per file.
//!
//! Loads the project the way render_project does (SaveFile::decode ->
//! to_project), so a finding here is a finding against what renders.

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    assert!(!files.is_empty(), "usage: project_check FILE.studio...");
    for f in &files {
        let save = studio_collab::file::SaveFile::decode(&std::fs::read(f).unwrap()).expect("project");
        let project = save.to_project();
        let found = studio_core::journal::check(&project);
        println!("{f}: {} finding(s)", found.len());
        for x in &found { println!("  [{}] {}", x.key, x.text); }
    }
}
