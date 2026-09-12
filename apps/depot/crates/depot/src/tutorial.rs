//! Tutorials are markdown files under `tutorials/`, embedded like the manual.
//! The convention is small on purpose: the first `# ` line is the title,
//! everything before the first `## ` is the introduction, and every `## `
//! section is one step. The UI shows a step at a time with progress; the
//! CLI prints the whole file. One source, two renderings.

use serde::Serialize;

pub struct Source {
    pub slug: &'static str,
    pub text: &'static str,
}

pub const SOURCES: &[Source] = &[
    Source { slug: "linux-basics", text: include_str!("../../../tutorials/01-linux-basics.md") },
    Source { slug: "server-setup", text: include_str!("../../../tutorials/02-server-setup.md") },
    Source { slug: "fill-the-depot", text: include_str!("../../../tutorials/03-fill-the-depot.md") },
    Source { slug: "sync", text: include_str!("../../../tutorials/04-sync.md") },
    Source { slug: "prepare-clients", text: include_str!("../../../tutorials/05-prepare-clients.md") },
    Source { slug: "usb-link", text: include_str!("../../../tutorials/06-usb-link.md") },
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Step {
    pub title: String,
    pub body: String,
    /// `%check: <key>` on the step's first line: a live check the depot
    /// can run against itself (see `checks.rs`) — the wizard's
    /// `can_advance`, applied to a tutorial.
    pub check: Option<String>,
    /// The first prose paragraph — what "read aloud" speaks.
    pub say: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Tutorial {
    pub slug: String,
    pub title: String,
    pub intro: String,
    pub steps: Vec<Step>,
}

pub fn parse(slug: &str, text: &str) -> Tutorial {
    let mut title = String::new();
    let mut intro = String::new();
    let mut steps: Vec<Step> = Vec::new();
    let mut in_code = false;
    for line in text.lines() {
        if line.starts_with("```") {
            in_code = !in_code;
        }
        if !in_code {
            if let Some(t) = line.strip_prefix("# ") {
                if title.is_empty() {
                    title = t.trim().to_string();
                    continue;
                }
            }
            if let Some(t) = line.strip_prefix("## ") {
                steps.push(Step { title: t.trim().to_string(), body: String::new(), check: None, say: String::new() });
                continue;
            }
            if let Some(k) = line.strip_prefix("%check:") {
                if let Some(s) = steps.last_mut() {
                    if s.body.trim().is_empty() {
                        s.check = Some(k.trim().to_string());
                        continue;
                    }
                }
            }
        }
        match steps.last_mut() {
            Some(s) => {
                s.body.push_str(line);
                s.body.push('\n');
            }
            None => {
                intro.push_str(line);
                intro.push('\n');
            }
        }
    }
    for s in &mut steps {
        s.body = s.body.trim().to_string();
        s.say = first_prose(&s.body);
    }
    Tutorial { slug: slug.to_string(), title, intro: intro.trim().to_string(), steps }
}

/// The first paragraph of prose: fenced code and tables skipped, lines of
/// the paragraph joined with spaces. What "read aloud" speaks.
fn first_prose(body: &str) -> String {
    let mut in_code = false;
    let mut para: Vec<&str> = Vec::new();
    for line in body.lines() {
        if line.starts_with("```") {
            in_code = !in_code;
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if in_code {
            continue;
        }
        let t = line.trim();
        if t.is_empty() {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if t.starts_with('|') || t.starts_with('%') {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        para.push(t);
    }
    para.join(" ")
}

pub fn all() -> Vec<Tutorial> {
    SOURCES.iter().map(|s| parse(s.slug, s.text)).collect()
}

pub fn find(slug: &str) -> Option<&'static Source> {
    SOURCES.iter().find(|s| s.slug == slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tutorial_has_a_title_an_intro_and_real_steps() {
        let all = all();
        assert_eq!(all.len(), SOURCES.len());
        let mut slugs: Vec<&str> = all.iter().map(|t| t.slug.as_str()).collect();
        slugs.sort();
        slugs.dedup();
        assert_eq!(slugs.len(), all.len(), "slugs unique");
        for t in &all {
            assert!(!t.title.is_empty(), "{}: title", t.slug);
            assert!(!t.intro.is_empty(), "{}: intro", t.slug);
            assert!(t.steps.len() >= 5, "{}: only {} steps", t.slug, t.steps.len());
            for s in &t.steps {
                assert!(!s.title.is_empty() && s.body.len() > 80, "{}: step {:?} too thin", t.slug, s.title);
                assert!(!s.say.is_empty() && !s.say.contains("```"), "{}: step {:?} has no prose to say", t.slug, s.title);
                if let Some(k) = &s.check {
                    assert!(crate::checks::KEYS.contains(&k.as_str()), "{}: step {:?} unknown check {k:?}", t.slug, s.title);
                }
                assert!(!s.body.contains("%check:"), "{}: step {:?} check line leaked into body", t.slug, s.title);
                assert!(s.body.contains("```") || s.body.contains('|'), "{}: step {:?} has no command or table", t.slug, s.title);
                assert_eq!(s.body.matches("```").count() % 2, 0, "{}: step {:?} unbalanced code fence", t.slug, s.title);
            }
        }
        assert!(find("sync").is_some() && find("nope").is_none());
    }

    #[test]
    fn a_hash_inside_a_code_block_is_not_a_heading() {
        let t = parse("x", "# T\nintro\n\n## One\n%check: writes_enabled\n```sh\n# not a title\n## not a step\n```\ntext\n\n## Two\nbody here\n```sh\nls\n```\n");
        assert_eq!(t.title, "T");
        assert_eq!(t.steps.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), vec!["One", "Two"]);
        assert!(t.steps[0].body.contains("## not a step"));
        assert_eq!(t.steps[0].check.as_deref(), Some("writes_enabled"));
        assert!(!t.steps[0].body.contains("%check"));
        assert_eq!(t.steps[0].say, "text");
        assert_eq!(t.steps[1].say, "body here");
        assert_eq!(t.steps[1].check, None);
    }
}
