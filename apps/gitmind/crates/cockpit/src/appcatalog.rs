//! Recognising real apps/repos (not arbitrary folders) and gathering the metadata
//! the catalog shows: name, crate count, on-disk size, README excerpt.
//!
//! Two ways in, sharing the same parsers:
//!   • local — `scan_dir` / `app_info_local` walk the filesystem (used when a device
//!     inventories its *own* apps; fully unit-tested here with temp dirs).
//!   • remote — `remote_listing_cmd` / `remote_details_cmd` produce shell snippets
//!     run over SSH on the server; `parse_app_listing` / `parse_app_details` turn
//!     their output back into the same structs (the parsers are unit-tested; the
//!     SSH execution itself is on-device).
//!
//! This is the "step 2/3" foundation from maintenance/DISTRIBUTION.md: the client
//! browses *recognised repositories* with real metadata, instead of raw folders.

use std::path::Path;

/// One entry in a directory: whether it looks like an app we can build/install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    pub name: String,
    pub is_cargo: bool, // has Cargo.toml (a Rust app/workspace)
    pub is_git: bool,   // has a .git (a recognised repository)
}

impl AppEntry {
    /// A folder counts as an app if it's a git repo and/or a cargo project.
    pub fn is_app(&self) -> bool {
        self.is_cargo || self.is_git
    }
}

/// Richer details shown once an app is selected.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppDetails {
    pub crate_count: u32,
    pub size_human: String,
    pub readme_excerpt: String,
}

// ───────────────────────── remote (over SSH) ─────────────────────────

/// Shell that prints `pwd`, then one `"<flags>\t<name>"` line per subdirectory,
/// where flags contain `c` if it has a Cargo.toml and `g` if it has a .git.
pub fn remote_listing_cmd(target: &str) -> String {
    format!(
        "cd {target} 2>/dev/null && pwd && for d in */; do n=${{d%/}}; [ -d \"$n\" ] || continue; \
         f=\"\"; [ -e \"$n/Cargo.toml\" ] && f=\"${{f}}c\"; [ -e \"$n/.git\" ] && f=\"${{f}}g\"; \
         printf '%s\\t%s\\n' \"$f\" \"$n\"; done"
    )
}

/// Parse `remote_listing_cmd` output (after the pwd line) into entries.
/// Pure — unit-tested.
pub fn parse_app_listing(body: &str) -> Vec<AppEntry> {
    body.lines()
        .filter_map(|l| {
            let (flags, name) = l.split_once('\t')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some(AppEntry {
                name: name.to_string(),
                is_cargo: flags.contains('c'),
                is_git: flags.contains('g'),
            })
        })
        .collect()
}

/// Shell that gathers an app's metadata in a fixed, parseable shape.
pub fn remote_details_cmd(app_path: &str) -> String {
    format!(
        "cd {app_path} 2>/dev/null && {{ echo \"CRATES:$(find . -maxdepth 4 -name Cargo.toml 2>/dev/null | wc -l)\"; \
         echo \"SIZE:$(du -sh . 2>/dev/null | cut -f1)\"; echo READMESTART; \
         head -n 6 README* readme* 2>/dev/null; echo READMEEND; }}"
    )
}

/// Parse `remote_details_cmd` output. Pure — unit-tested.
pub fn parse_app_details(raw: &str) -> AppDetails {
    let mut d = AppDetails::default();
    let mut in_readme = false;
    let mut readme = String::new();
    for line in raw.lines() {
        if let Some(n) = line.strip_prefix("CRATES:") {
            d.crate_count = n.trim().parse().unwrap_or(0);
        } else if let Some(s) = line.strip_prefix("SIZE:") {
            d.size_human = s.trim().to_string();
        } else if line == "READMESTART" {
            in_readme = true;
        } else if line == "READMEEND" {
            in_readme = false;
        } else if in_readme {
            readme.push_str(line);
            readme.push('\n');
        }
    }
    d.readme_excerpt = readme.trim().to_string();
    d
}

// ───────────────────────── store catalog (one round-trip) ─────────────────────────

/// A storefront entry: an app's name, on-disk size, and a one-line blurb (the
/// Cargo.toml `description`, falling back to the README's first prose line).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CatalogEntry {
    pub name: String,
    pub size: String,
    pub description: String,
}

/// One shell command that lists every recognised app under `root` with its size and
/// blurb — so the store renders from a single ssh call instead of one per app.
/// `root` should be shell-safe (caller quotes); empty/`~` resolve to `$HOME`.
pub fn remote_catalog_cmd(root: &str) -> String {
    format!(
        "r={root}; case \"$r\" in ''|'~') r=\"$HOME\";; '~/'*) r=\"$HOME/${{r#~/}}\";; esac; \
         cd \"$r\" 2>/dev/null && pwd && for d in */; do n=${{d%/}}; [ -d \"$n\" ] || continue; \
         [ -f \"$n/Cargo.toml\" ] || [ -d \"$n/.git\" ] || continue; \
         sz=$(du -sh \"$n\" 2>/dev/null | cut -f1); \
         ds=$(grep -m1 '^description' \"$n/Cargo.toml\" 2>/dev/null | sed -e 's/^description[^=]*= *//' -e 's/^\\\"//' -e 's/\\\"$//'); \
         [ -n \"$ds\" ] || ds=$(grep -m1 '[A-Za-z]' \"$n/README\"* \"$n/readme\"* 2>/dev/null | sed -e 's/^#* *//'); \
         printf '%s\\t%s\\t%s\\n' \"$n\" \"$sz\" \"$ds\"; done"
    )
}

/// Parse `remote_catalog_cmd` output (after the pwd line) into store entries.
/// Pure — unit-tested.
pub fn parse_catalog(body: &str) -> Vec<CatalogEntry> {
    body.lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, '\t');
            let name = parts.next()?.trim();
            if name.is_empty() {
                return None;
            }
            let size = parts.next().unwrap_or("").trim().to_string();
            let description = parts.next().unwrap_or("").trim().to_string();
            Some(CatalogEntry { name: name.to_string(), size, description })
        })
        .collect()
}

// ───────────────────────── local (own filesystem) ─────────────────────────

/// Inventory `root`'s immediate subdirectories as app entries. Local — unit-tested.
pub fn scan_dir(root: &Path) -> Vec<AppEntry> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(root) {
        Ok(rd) => rd,
        Err(_) => return out,
    };
    for ent in rd.flatten() {
        if !ent.path().is_dir() {
            continue;
        }
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        out.push(AppEntry {
            name: name.clone(),
            is_cargo: ent.path().join("Cargo.toml").exists(),
            is_git: ent.path().join(".git").exists(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Gather one app's details from the local filesystem. Local — unit-tested.
pub fn app_info_local(app: &Path) -> AppDetails {
    let mut crate_count = 0u32;
    let mut total: u64 = 0;
    fn walk(dir: &Path, depth: u32, crates: &mut u32, total: &mut u64) {
        if depth > 4 {
            return;
        }
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return,
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                walk(&p, depth + 1, crates, total);
            } else {
                if p.file_name().map(|n| n == "Cargo.toml").unwrap_or(false) {
                    *crates += 1;
                }
                if let Ok(m) = ent.metadata() {
                    *total += m.len();
                }
            }
        }
    }
    walk(app, 0, &mut crate_count, &mut total);
    let readme = ["README.md", "README", "readme.md", "README.txt"]
        .iter()
        .find_map(|n| std::fs::read_to_string(app.join(n)).ok())
        .map(|s| s.lines().take(6).collect::<Vec<_>>().join("\n").trim().to_string())
        .unwrap_or_default();
    AppDetails {
        crate_count,
        size_human: human_size(total),
        readme_excerpt: readme,
    }
}

/// Bytes → a short human string (matches `du -h` style closely enough). Pure.
pub fn human_size(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{}{}", bytes, U[0])
    } else {
        format!("{:.1}{}", v, U[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remote_listing_flags() {
        let body = "c\tav1tuner\ng\tnotes\ncg\tgitmind\n\tplain_folder\n";
        let e = parse_app_listing(body);
        assert_eq!(e.len(), 4);
        assert!(e[0].is_cargo && !e[0].is_git && e[0].is_app()); // av1tuner
        assert!(e[1].is_git && !e[1].is_cargo && e[1].is_app()); // notes
        assert!(e[2].is_cargo && e[2].is_git); // gitmind
        assert!(!e[3].is_app(), "plain folder is not an app"); // plain_folder
    }

    #[test]
    fn parses_remote_details_block() {
        let raw = "CRATES:7\nSIZE:42M\nREADMESTART\n# av1tuner\nTunes AV1 encodes.\nREADMEEND\n";
        let d = parse_app_details(raw);
        assert_eq!(d.crate_count, 7);
        assert_eq!(d.size_human, "42M");
        assert!(d.readme_excerpt.contains("Tunes AV1 encodes."));
        assert!(d.readme_excerpt.starts_with("# av1tuner"));
    }

    #[test]
    fn listing_and_details_cmds_are_well_formed() {
        let l = remote_listing_cmd("$HOME");
        assert!(l.contains("cd $HOME") && l.contains("Cargo.toml") && l.contains("/.git"));
        let d = remote_details_cmd("'/home/kesslerd/Code/av1tuner'");
        assert!(d.contains("CRATES:") && d.contains("du -sh") && d.contains("READMESTART"));
    }

    #[test]
    fn local_scan_recognises_repos_and_gathers_details() {
        let tmp = std::env::temp_dir().join(format!("gm-cat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // an app: cargo + git + readme + a workspace member
        let app = tmp.join("myapp");
        std::fs::create_dir_all(app.join("crates/inner")).unwrap();
        std::fs::create_dir_all(app.join(".git")).unwrap();
        std::fs::write(app.join("Cargo.toml"), b"[workspace]\n").unwrap();
        std::fs::write(app.join("crates/inner/Cargo.toml"), b"[package]\nname=\"inner\"\n").unwrap();
        std::fs::write(app.join("README.md"), b"# myapp\nline two\n").unwrap();
        // a plain folder: neither cargo nor git
        std::fs::create_dir_all(tmp.join("docs")).unwrap();

        let entries = scan_dir(&tmp);
        assert_eq!(entries.len(), 2);
        let myapp = entries.iter().find(|e| e.name == "myapp").unwrap();
        assert!(myapp.is_cargo && myapp.is_git && myapp.is_app());
        let docs = entries.iter().find(|e| e.name == "docs").unwrap();
        assert!(!docs.is_app());

        let det = app_info_local(&app);
        assert_eq!(det.crate_count, 2, "root + workspace member");
        assert!(det.readme_excerpt.starts_with("# myapp"));
        assert!(det.size_human.ends_with('B') || det.size_human.ends_with('K'));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn human_size_scales() {
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(2048), "2.0K");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0M");
    }

    #[test]
    fn catalog_cmd_filters_apps_and_grabs_blurb() {
        let c = remote_catalog_cmd("'/home/kesslerd/Code'");
        assert!(c.contains("Cargo.toml") && c.contains(".git"), "recognises apps");
        assert!(c.contains("du -sh") && c.contains("description"), "size + blurb");
        assert!(c.contains("README"), "README fallback");
    }

    #[test]
    fn parses_catalog_lines() {
        let out = "voiceterm\t2.3M\tVoice-controlled terminal\n\
                   av1tuner\t1.1M\t\n\
                   \t\t\n\
                   photosync\t8.0M\tSync photos over LAN";
        let c = parse_catalog(out);
        assert_eq!(c.len(), 3, "blank-name line dropped");
        assert_eq!(c[0], CatalogEntry { name: "voiceterm".into(), size: "2.3M".into(), description: "Voice-controlled terminal".into() });
        assert_eq!(c[1].description, "", "missing blurb tolerated");
        assert_eq!(c[2].name, "photosync");
    }
}
