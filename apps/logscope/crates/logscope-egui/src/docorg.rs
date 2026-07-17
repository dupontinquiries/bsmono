//! docorg.rs — a document-organization convention so agents can fan out on docs and MERGE while staying
//! organized.
//!
//! There is endless prior art on organizing docs (Diátaxis's guide/reference/how-to/explanation split,
//! docs-as-code front-matter, manifest-driven indexes). This distills the durable parts into a tiny,
//! machine-checkable convention the agent loop can teach and enforce:
//!
//!   * Every doc opens with a FRONT-MATTER block (`--- … ---`): title, section, order, status, summary.
//!   * Docs live under `docs/<section>/…`; sections are a small fixed set so branches don't invent
//!     divergent taxonomies.
//!   * An INDEX (`docs/README.md`) is GENERATED from the front-matter, never hand-maintained — so it can't
//!     drift, and merging is just re-scanning the union of docs.
//!
//! Because the index is derived, MERGING parallel branches is clean: each branch writes distinct docs (its
//! slice of the tree), and merge = union + collision-check + regenerate the index. Front-matter makes every
//! doc self-describing, so ordering/grouping survives the merge without a hand-kept manifest.
//!
//! Pure + unit-tested; no model, no filesystem.

use std::collections::BTreeMap;

/// The canonical section taxonomy (Diátaxis-derived). A small fixed set keeps parallel branches from
/// inventing divergent trees. `Other` catches anything off-convention so it's still placed, not dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section { Overview, Guide, HowTo, Reference, Design, Other }

impl Section {
    pub fn slug(self) -> &'static str {
        match self { Section::Overview => "overview", Section::Guide => "guide", Section::HowTo => "how-to",
                     Section::Reference => "reference", Section::Design => "design", Section::Other => "other" }
    }
    pub fn title(self) -> &'static str {
        match self { Section::Overview => "Overview", Section::Guide => "Guides", Section::HowTo => "How-To",
                     Section::Reference => "Reference", Section::Design => "Design", Section::Other => "Other" }
    }
    /// The display order of sections in the index.
    fn rank(self) -> u8 { match self { Section::Overview => 0, Section::Guide => 1, Section::HowTo => 2, Section::Reference => 3, Section::Design => 4, Section::Other => 5 } }
    pub fn parse(s: &str) -> Section {
        match s.trim().to_ascii_lowercase().as_str() {
            "overview" | "intro" | "introduction" => Section::Overview,
            "guide" | "guides" | "tutorial" | "tutorials" => Section::Guide,
            "how-to" | "howto" | "how_to" | "recipe" | "recipes" => Section::HowTo,
            "reference" | "ref" | "api" => Section::Reference,
            "design" | "architecture" | "adr" | "rfc" => Section::Design,
            _ => Section::Other,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocStatus { Draft, Review, Final }
impl DocStatus {
    pub fn label(self) -> &'static str { match self { DocStatus::Draft => "draft", DocStatus::Review => "review", DocStatus::Final => "final" } }
    fn parse(s: &str) -> DocStatus {
        match s.trim().to_ascii_lowercase().as_str() { "final" | "done" => DocStatus::Final, "review" => DocStatus::Review, _ => DocStatus::Draft }
    }
}

/// Parsed front-matter + provenance for one doc.
#[derive(Clone, Debug, PartialEq)]
pub struct DocMeta {
    pub title: String,
    pub section: Section,
    pub order: i32,
    pub status: DocStatus,
    pub summary: String,
    pub path: String,   // relative path, e.g. "docs/guide/getting-started.md"
    pub branch: String, // which agent branch produced it (empty for a single run)
}

/// Render the front-matter block an agent prepends to a doc. Kept minimal + example-shaped so a small model
/// reproduces it reliably.
pub fn front_matter_block(title: &str, section: Section, order: i32, status: DocStatus, summary: &str) -> String {
    format!(
"---\ntitle: {title}\nsection: {}\norder: {order}\nstatus: {}\nsummary: {summary}\n---\n",
        section.slug(), status.label())
}

/// Parse a doc's leading front-matter block into `DocMeta`. Returns `None` if the doc doesn't open with a
/// `---` fence (so the caller can flag it as off-convention). Unknown keys are ignored; missing keys default.
pub fn parse_front_matter(text: &str, path: &str, branch: &str) -> Option<DocMeta> {
    let t = text.trim_start();
    let body = t.strip_prefix("---")?;
    // find the closing fence
    let end = body.find("\n---")?;
    let block = &body[..end];
    let mut title = String::new();
    let mut section = Section::Other;
    let mut order = 0i32;
    let mut status = DocStatus::Draft;
    let mut summary = String::new();
    for line in block.lines() {
        let line = line.trim();
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "title" => title = v.to_string(),
            "section" => section = Section::parse(v),
            "order" => order = v.parse().unwrap_or(0),
            "status" => status = DocStatus::parse(v),
            "summary" => summary = v.to_string(),
            _ => {}
        }
    }
    if title.is_empty() {
        // fall back to the first markdown H1, else the filename stem
        title = text.lines().find_map(|l| l.strip_prefix("# ").map(|s| s.trim().to_string()))
            .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).trim_end_matches(".md").replace(['-', '_'], " "));
    }
    Some(DocMeta { title, section, order, status, summary, path: path.to_string(), branch: branch.to_string() })
}

/// A doc the agent produced but that lacks front-matter — still placed (inferred), and reported so the loop
/// can nudge the model to add a header.
fn infer_meta(text: &str, path: &str, branch: &str) -> DocMeta {
    // infer section from the path (docs/<section>/…), title from H1/filename
    let section = path.split('/').nth(1).map(Section::parse).unwrap_or(Section::Other);
    let title = text.lines().find_map(|l| l.strip_prefix("# ").map(|s| s.trim().to_string()))
        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).trim_end_matches(".md").replace(['-', '_'], " "));
    DocMeta { title, section, order: 999, status: DocStatus::Draft, summary: String::new(), path: path.to_string(), branch: branch.to_string() }
}

/// The organized result of scanning a doc set.
#[derive(Clone, Debug, Default)]
pub struct Index {
    pub docs: Vec<DocMeta>,          // all docs, in index order (section rank, then order, then title)
    pub warnings: Vec<String>,       // off-convention docs, duplicate order, collisions
}

/// Scan a set of `(path, text)` docs and build an organized index. Docs missing front-matter are inferred +
/// warned. Ordering: section rank → `order` → title. `branch` tags provenance (use "" for a single run).
pub fn build_index_for(docs: &[(String, String)], branch: &str) -> Index {
    let mut metas: Vec<DocMeta> = Vec::new();
    let mut warnings = Vec::new();
    for (path, text) in docs {
        match parse_front_matter(text, path, branch) {
            Some(m) => metas.push(m),
            None => {
                warnings.push(format!("{path}: no front-matter block — add `--- … ---` with title/section/order/status/summary"));
                metas.push(infer_meta(text, path, branch));
            }
        }
    }
    finalize_index(metas, warnings)
}

fn finalize_index(mut metas: Vec<DocMeta>, mut warnings: Vec<String>) -> Index {
    // duplicate (section, order) within a branch is a soft warning (ambiguous sort)
    let mut seen_order: BTreeMap<(u8, i32), String> = BTreeMap::new();
    for m in &metas {
        if let Some(prev) = seen_order.insert((m.section.rank(), m.order), m.path.clone()) {
            warnings.push(format!("duplicate order {} in section '{}': {} and {}", m.order, m.section.slug(), prev, m.path));
        }
    }
    metas.sort_by(|a, b| a.section.rank().cmp(&b.section.rank())
        .then(a.order.cmp(&b.order))
        .then(a.title.cmp(&b.title)));
    Index { docs: metas, warnings }
}

/// Render the generated index (`docs/README.md`): a grouped, ordered table of contents with summaries. This
/// is DERIVED — regenerate it after any change instead of hand-editing, so it never drifts.
pub fn render_index_markdown(index: &Index, project_title: &str) -> String {
    let mut out = format!("# {project_title}\n\n_Generated index — do not edit by hand; regenerate from the docs' front-matter._\n");
    let mut cur: Option<Section> = None;
    for m in &index.docs {
        if cur != Some(m.section) {
            out.push_str(&format!("\n## {}\n\n", m.section.title()));
            cur = Some(m.section);
        }
        let status = if m.status == DocStatus::Final { String::new() } else { format!(" _({})_", m.status.label()) };
        let summary = if m.summary.is_empty() { String::new() } else { format!(" — {}", m.summary) };
        out.push_str(&format!("- [{}]({}){}{}\n", m.title, m.path, status, summary));
    }
    out
}

/// One branch's produced docs, for merging.
#[derive(Clone, Debug)]
pub struct BranchDocs { pub branch: String, pub docs: Vec<(String, String)> } // (path, text)

/// The result of merging several branches' docs into one organized tree.
#[derive(Clone, Debug, Default)]
pub struct MergeResult {
    pub index: Index,                 // unified, organized index across all branches
    pub collisions: Vec<Collision>,   // GENUINE conflicts: same path, substantially-different content (need reconcile)
    pub auto_resolved: Vec<String>,   // paths where near-duplicate versions were auto-merged (kept the richer)
    pub needs_merge: Vec<(String, Vec<(String, String)>)>, // (path, versions) for the conflicts, for the model reconcile
    pub merged: Vec<(String, String)>,// (path, text) to write — the union with near-dups resolved
}
#[derive(Clone, Debug, PartialEq)]
pub struct Collision { pub path: String, pub branches: Vec<String> }

/// Strip a doc's front-matter block, returning just the body (for content comparison).
fn body_of(text: &str) -> &str {
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            return rest[end + 4..].trim_start_matches('\n');
        }
    }
    text
}

/// Word-set Jaccard similarity of two doc bodies (0..1). Cheap near-duplicate signal.
fn body_similarity(a: &str, b: &str) -> f64 {
    use std::collections::HashSet;
    let wa: HashSet<&str> = body_of(a).split_whitespace().collect();
    let wb: HashSet<&str> = body_of(b).split_whitespace().collect();
    if wa.is_empty() && wb.is_empty() { return 1.0; }
    let inter = wa.intersection(&wb).count();
    let union = wa.union(&wb).count();
    if union == 0 { 0.0 } else { inter as f64 / union as f64 }
}

/// "Richness" score for picking the better of two near-identical versions: more front-matter + longer body.
fn richness(text: &str, path: &str) -> usize {
    let fm = parse_front_matter(text, path, "");
    let fm_score = fm.as_ref().map(|m| {
        (!m.summary.is_empty()) as usize + (m.order != 0 && m.order != 999) as usize
        + (m.section != Section::Other) as usize
    }).unwrap_or(0);
    fm_score * 100 + body_of(text).split_whitespace().count()
}

/// How a collision (same path from multiple branches) resolves.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// Near-identical versions — keep the richer one automatically, no merge needed.
    KeepRicher { branch: String, text: String },
    /// Genuinely different content — needs a merge (depth model or human). Carries every version.
    NeedsMerge { versions: Vec<(String, String)> }, // (branch, text)
}

/// Classify a collision from its `(branch, text)` versions. If all versions are near-identical (high body
/// similarity), keep the richest and move on; otherwise it's a genuine conflict that needs reconciliation.
pub fn resolve_collision(path: &str, versions: &[(String, String)]) -> Resolution {
    if versions.len() < 2 {
        let (b, t) = versions.first().cloned().unwrap_or_default();
        return Resolution::KeepRicher { branch: b, text: t };
    }
    // near-duplicate iff every pair is above the similarity threshold
    let mut near_dup = true;
    'outer: for i in 0..versions.len() {
        for j in (i + 1)..versions.len() {
            if body_similarity(&versions[i].1, &versions[j].1) < 0.8 { near_dup = false; break 'outer; }
        }
    }
    if near_dup {
        let (branch, text) = versions.iter().max_by_key(|(_, t)| richness(t, path)).cloned().unwrap();
        Resolution::KeepRicher { branch, text }
    } else {
        Resolution::NeedsMerge { versions: versions.to_vec() }
    }
}

/// The prompt for the depth model to reconcile genuinely-different versions of one doc into a single coherent
/// document. Kept format-pinned (keep the front-matter, one merged body) so the output drops straight back in.
pub fn reconcile_prompt(path: &str, versions: &[(String, String)]) -> String {
    let mut p = format!(
"Several parallel agents each wrote a different version of the document `{path}`. Merge them into ONE \
coherent document that keeps the best of each, with no duplicated sections. Preserve the front-matter block \
(--- … ---) at the top with a single title/section/order/status/summary. Output ONLY the merged document.\n");
    for (i, (branch, text)) in versions.iter().enumerate() {
        p.push_str(&format!("\n===== version {} (branch: {branch}) =====\n{text}\n", i + 1));
    }
    p
}

/// Merge complementary docs from parallel agent branches. Each branch owns distinct docs (its slice of the
/// tree); the union is the merged doc set. A path written by more than one branch is a COLLISION (reported,
/// not silently overwritten). The index is regenerated across the whole union so grouping/order survive the
/// merge. This is the "fan out and merge while staying organized" core: branches don't coordinate live, the
/// convention + derived index make their outputs compose.
pub fn merge_branches(branches: &[BranchDocs], project_title: &str) -> MergeResult {
    // group texts by path, tracking which branches wrote each
    let mut by_path: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new(); // path → [(branch, text)]
    for b in branches {
        for (path, text) in &b.docs {
            by_path.entry(path.clone()).or_default().push((b.branch.clone(), text.clone()));
        }
    }
    let mut collisions = Vec::new();
    let mut auto_resolved = Vec::new();
    let mut needs_merge = Vec::new();
    let mut merged: Vec<(String, String)> = Vec::new();
    let mut metas: Vec<DocMeta> = Vec::new();
    let mut warnings = Vec::new();
    for (path, writers) in by_path {
        let (branch, text) = if writers.len() > 1 {
            // a collision — try to resolve near-duplicates automatically; only report genuine conflicts
            match resolve_collision(&path, &writers) {
                Resolution::KeepRicher { branch, text } => {
                    auto_resolved.push(path.clone());
                    (branch, text)
                }
                Resolution::NeedsMerge { versions } => {
                    collisions.push(Collision { path: path.clone(), branches: versions.iter().map(|(b, _)| b.clone()).collect() });
                    needs_merge.push((path.clone(), versions.clone()));
                    // keep the richest deterministically so the tree is still usable pending reconcile
                    versions.into_iter().max_by_key(|(_, t)| richness(t, &path)).unwrap()
                }
            }
        } else {
            writers.into_iter().next().unwrap()
        };
        match parse_front_matter(&text, &path, &branch) {
            Some(m) => metas.push(m),
            None => { warnings.push(format!("{path}: no front-matter (from branch {branch})")); metas.push(infer_meta(&text, &path, &branch)); }
        }
        merged.push((path, text));
    }
    let index = finalize_index(metas, warnings);
    let _ = project_title;
    MergeResult { index, collisions, auto_resolved, needs_merge, merged }
}

/// The system-prompt fragment that teaches the doc-organization convention. Appended to an agent's prompt in
/// DOC MODE so fanned-out branches produce consistent, mergeable docs.
pub const DOC_ORG_GUIDE: &str = "\
DOCUMENT ORGANIZATION (follow exactly so parallel work merges cleanly):
- Put every document under `docs/<section>/<kebab-case-name>.md`, where <section> is ONE of:
  overview, guide, how-to, reference, design. Do not invent other top-level sections.
- Begin EVERY document with a front-matter block:
  ---
  title: <human title>
  section: <one of the sections above>
  order: <integer sort order within the section>
  status: draft|review|final
  summary: <one sentence for the index>
  ---
- Do NOT hand-write an index/table-of-contents; `docs/README.md` is generated from the front-matter.
- Keep each document focused on ONE topic; split rather than pile unrelated material into one file.
- When working as one of several parallel branches, write only YOUR assigned documents — do not touch
  siblings' files; the merge combines everyone's docs by section + order.";

// ── tests ───────────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(section: &str, order: i32, title: &str, summary: &str) -> String {
        format!("---\ntitle: {title}\nsection: {section}\norder: {order}\nstatus: draft\nsummary: {summary}\n---\n# {title}\n\nbody\n")
    }

    #[test]
    fn front_matter_round_trips() {
        let block = front_matter_block("Getting Started", Section::Guide, 1, DocStatus::Draft, "Install and run.");
        let full = format!("{block}# Getting Started\n");
        let m = parse_front_matter(&full, "docs/guide/getting-started.md", "").unwrap();
        assert_eq!(m.title, "Getting Started");
        assert_eq!(m.section, Section::Guide);
        assert_eq!(m.order, 1);
        assert_eq!(m.summary, "Install and run.");
    }

    #[test]
    fn missing_front_matter_is_inferred_and_warned() {
        let docs = vec![("docs/reference/api.md".to_string(), "# API\n\nno header here\n".to_string())];
        let idx = build_index_for(&docs, "");
        assert_eq!(idx.docs.len(), 1);
        assert_eq!(idx.docs[0].section, Section::Reference, "section inferred from the path");
        assert_eq!(idx.docs[0].title, "API", "title inferred from the H1");
        assert!(idx.warnings.iter().any(|w| w.contains("no front-matter")));
    }

    #[test]
    fn index_groups_by_section_then_order() {
        let docs = vec![
            ("docs/reference/b.md".into(), doc("reference", 2, "B Ref", "second ref")),
            ("docs/overview/intro.md".into(), doc("overview", 1, "Intro", "the intro")),
            ("docs/reference/a.md".into(), doc("reference", 1, "A Ref", "first ref")),
            ("docs/guide/start.md".into(), doc("guide", 1, "Start", "get going")),
        ];
        let idx = build_index_for(&docs, "");
        let titles: Vec<&str> = idx.docs.iter().map(|d| d.title.as_str()).collect();
        // overview → guide → reference(order 1, then 2)
        assert_eq!(titles, vec!["Intro", "Start", "A Ref", "B Ref"]);
        let md = render_index_markdown(&idx, "My Project");
        assert!(md.contains("## Overview") && md.contains("## Guides") && md.contains("## Reference"));
        assert!(md.contains("[Intro](docs/overview/intro.md)"));
    }

    #[test]
    fn duplicate_order_is_warned() {
        let docs = vec![
            ("docs/guide/a.md".into(), doc("guide", 1, "A", "")),
            ("docs/guide/b.md".into(), doc("guide", 1, "B", "")),
        ];
        let idx = build_index_for(&docs, "");
        assert!(idx.warnings.iter().any(|w| w.contains("duplicate order 1")));
    }

    #[test]
    fn merge_unions_complementary_branches() {
        // two branches each wrote DIFFERENT docs — merge is the organized union
        let b1 = BranchDocs { branch: "guides".into(), docs: vec![
            ("docs/guide/install.md".into(), doc("guide", 1, "Install", "how to install")),
            ("docs/guide/config.md".into(), doc("guide", 2, "Config", "configuration")),
        ]};
        let b2 = BranchDocs { branch: "reference".into(), docs: vec![
            ("docs/reference/api.md".into(), doc("reference", 1, "API", "the api")),
        ]};
        let m = merge_branches(&[b1, b2], "Docs");
        assert_eq!(m.merged.len(), 3, "union of all three distinct docs");
        assert!(m.collisions.is_empty());
        let titles: Vec<&str> = m.index.docs.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(titles, vec!["Install", "Config", "API"]); // guide(1,2) then reference(1)
        // provenance survives
        assert_eq!(m.index.docs.iter().find(|d| d.title == "API").unwrap().branch, "reference");
    }

    #[test]
    fn near_duplicate_collision_auto_resolves_to_the_richer() {
        // both branches wrote nearly the same doc — auto-resolve, keeping the richer (has summary + order).
        let thin = "---\ntitle: Setup\nsection: guide\norder: 0\nstatus: draft\nsummary:\n---\n# Setup\ninstall then run the thing\n";
        let rich = "---\ntitle: Setup\nsection: guide\norder: 1\nstatus: draft\nsummary: how to set up\n---\n# Setup\ninstall then run the thing please\n";
        let b1 = BranchDocs { branch: "x".into(), docs: vec![("docs/guide/setup.md".into(), thin.into())] };
        let b2 = BranchDocs { branch: "y".into(), docs: vec![("docs/guide/setup.md".into(), rich.into())] };
        let m = merge_branches(&[b1, b2], "Docs");
        assert!(m.collisions.is_empty(), "near-duplicate is auto-resolved, not a reported conflict");
        assert_eq!(m.auto_resolved, vec!["docs/guide/setup.md"]);
        assert_eq!(m.merged.len(), 1);
        assert!(m.merged[0].1.contains("how to set up"), "kept the richer version");
    }

    #[test]
    fn genuine_conflict_is_reported_with_versions_for_reconcile() {
        // two substantially-different docs at the same path → a real conflict + versions carried for the model.
        let a = "---\ntitle: Setup\nsection: guide\norder: 1\nstatus: draft\nsummary: docker path\n---\n# Setup\nUse docker compose up to start the stack in containers with volumes and ports.\n";
        let b = "---\ntitle: Setup\nsection: guide\norder: 1\nstatus: draft\nsummary: bare metal\n---\n# Setup\nCompile from source with cargo build release then run the native binary directly.\n";
        let b1 = BranchDocs { branch: "docker".into(), docs: vec![("docs/guide/setup.md".into(), a.into())] };
        let b2 = BranchDocs { branch: "native".into(), docs: vec![("docs/guide/setup.md".into(), b.into())] };
        let m = merge_branches(&[b1, b2], "Docs");
        assert_eq!(m.collisions.len(), 1);
        assert_eq!(m.needs_merge.len(), 1, "versions carried for the reconcile step");
        assert_eq!(m.needs_merge[0].1.len(), 2);
        // reconcile prompt carries both versions + the path
        let p = reconcile_prompt(&m.needs_merge[0].0, &m.needs_merge[0].1);
        assert!(p.contains("docs/guide/setup.md") && p.contains("branch: docker") && p.contains("branch: native"));
        assert!(p.contains("Merge them into ONE"));
    }
}
