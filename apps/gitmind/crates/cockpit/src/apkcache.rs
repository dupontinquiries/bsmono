//! apkcache — one download per package for the whole fleet.
//!
//! Rerolling four phones currently pulls the same packages from
//! postmarketOS's mirrors four times, and a wipe-and-reflash cycle does it
//! again. That is bandwidth somebody else pays for, and it is also the slowest
//! part of a reroll: `apk update` plus a few hundred packages over a WAN link,
//! per device, every time.
//!
//! A cache on the LAN fixes both. The phones point at it, it holds what it has
//! already fetched, and only a genuine miss reaches the internet.
//!
//! ## The rule that makes it correct
//!
//! Two kinds of file live in an apk repository and they have opposite caching
//! rules:
//!
//! * **`*.apk`** — named `<pkg>-<version>-r<rel>.apk`, and that name is a
//!   version. The bytes behind it never change; a rebuild gets a new `-r`.
//!   Safe to cache forever, and the reason a fleet cache is worth having at
//!   all.
//! * **`APKINDEX.tar.gz`** — rewritten every time the repository changes.
//!   Cache it and the phone is told a package does not exist when it does, or
//!   is offered a version that has been pulled. The failure looks like a
//!   broken mirror rather than a stale cache, which is what makes it worth
//!   being careful about.
//!
//! So: apks are immutable, indexes are revalidated, and nothing else is
//! guessed at.
//!
//! ## What this module is and is not
//!
//! Pure policy: what to cache, for how long, where it lands on disk, and how
//! to point a device at it. The HTTP server and the fetching live in the app —
//! same split as the rest of this crate, and the same reason: the decisions
//! are the part worth testing.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long an index may be served without asking upstream.
///
/// Short. The index is small — tens of kilobytes against megabytes of
/// packages — so a conservative TTL costs almost nothing and a generous one
/// costs correctness. Five minutes covers the burst of a fleet reroll, where
/// four phones ask within a minute of each other, without holding a stale view
/// long enough to matter.
pub const INDEX_TTL: Duration = Duration::from_secs(300);

/// What kind of thing was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A package. Immutable: the version is in the filename.
    Package,
    /// A repository index. Mutable, revalidated.
    Index,
    /// Something else in the tree — a signature, a description. Passed
    /// through without caching rather than guessed at, because a wrong
    /// guess about a file we do not understand is worse than a slow fetch.
    Other,
}

impl Kind {
    pub fn of(path: &str) -> Kind {
        let name = path.rsplit('/').next().unwrap_or(path);
        if name.ends_with(".apk") {
            Kind::Package
        } else if name.starts_with("APKINDEX") {
            Kind::Index
        } else {
            Kind::Other
        }
    }

    /// May a hit be served without asking upstream?
    pub fn cacheable(self) -> bool {
        matches!(self, Kind::Package | Kind::Index)
    }

    /// How long a stored copy stays authoritative. `None` means forever.
    pub fn ttl(self) -> Option<Duration> {
        match self {
            Kind::Package => None,
            Kind::Index => Some(INDEX_TTL),
            Kind::Other => Some(Duration::ZERO),
        }
    }
}

/// What the cache should do with a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Serve the stored copy.
    Serve(PathBuf),
    /// Fetch upstream, store it, then serve.
    FetchAndStore(PathBuf),
    /// Fetch upstream and serve without storing.
    Passthrough,
}

/// Decide, given whether a copy exists and how old it is.
pub fn decide(path: &str, on_disk: Option<Duration>, store: &Path) -> Decision {
    let kind = Kind::of(path);
    if !kind.cacheable() {
        return Decision::Passthrough;
    }
    let target = cache_path(store, path);
    match (on_disk, kind.ttl()) {
        (Some(_), None) => Decision::Serve(target),
        (Some(age), Some(ttl)) if age < ttl => Decision::Serve(target),
        _ => Decision::FetchAndStore(target),
    }
}

/// Where a request lands under the cache root.
///
/// Mirrors the upstream path so the store is browsable and so an operator can
/// point a plain HTTP server at it if this process is not running. Leading
/// slashes are dropped and `..` is refused — a path from the network must not
/// be able to write outside the store, and this is the one place that could
/// happen.
pub fn cache_path(store: &Path, request: &str) -> PathBuf {
    let mut p = store.to_path_buf();
    for seg in request.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            continue;
        }
        p.push(seg);
    }
    p
}

// ── /etc/apk/repositories ────────────────────────────────────────────────────

/// One line of an apk repositories file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoLine {
    /// A repository URL, with its `@tag` if it had one.
    Repo { tag: Option<String>, url: String },
    /// A comment or a disabled repository. Preserved verbatim.
    Comment(String),
    Blank,
}

/// Parse a repositories file.
pub fn parse_repositories(text: &str) -> Vec<RepoLine> {
    text.lines()
        .map(|raw| {
            let l = raw.trim();
            if l.is_empty() {
                return RepoLine::Blank;
            }
            if l.starts_with('#') {
                return RepoLine::Comment(raw.to_string());
            }
            match l.strip_prefix('@') {
                Some(rest) => match rest.split_once(char::is_whitespace) {
                    Some((tag, url)) => RepoLine::Repo {
                        tag: Some(tag.to_string()),
                        url: url.trim().to_string(),
                    },
                    None => RepoLine::Comment(raw.to_string()),
                },
                None => RepoLine::Repo { tag: None, url: l.to_string() },
            }
        })
        .collect()
}

/// Rewrite one URL to go through the cache.
///
/// `http://host/path` becomes `<base>/host/path`, so the cache's store mirrors
/// upstream layout and two different mirrors of the same repo do not collide.
/// A URL that is already local — a file path, or the cache itself — is left
/// alone: rewriting a device's on-disk repo into an HTTP fetch would break
/// installs that currently work offline.
pub fn rewrite_url(base: &str, url: &str) -> String {
    let base = base.trim_end_matches('/');
    let rest = match url.split_once("://") {
        Some((scheme, rest)) if scheme == "http" || scheme == "https" => rest,
        // Local paths and unknown schemes pass through untouched.
        _ => return url.to_string(),
    };
    if url.starts_with(base) {
        return url.to_string();
    }
    format!("{base}/{rest}")
}

/// Rewrite a whole repositories file to use the cache.
///
/// The original lines are kept, commented, immediately above their
/// replacements. Two reasons: an operator can see what changed and undo it
/// with a text editor on a phone with no tooling, and a wiped device that gets
/// this file has a record of where the packages actually come from.
pub fn rewrite_repositories(text: &str, base: &str) -> String {
    let mut out = String::new();
    for line in parse_repositories(text) {
        match line {
            RepoLine::Blank => out.push('\n'),
            RepoLine::Comment(c) => {
                out.push_str(&c);
                out.push('\n');
            }
            RepoLine::Repo { tag, url } => {
                let new = rewrite_url(base, &url);
                if new == url {
                    // Unchanged: emit as-is, no misleading "was" comment.
                    match &tag {
                        Some(t) => out.push_str(&format!("@{t} {url}\n")),
                        None => out.push_str(&format!("{url}\n")),
                    }
                    continue;
                }
                // The tag goes in the breadcrumb too. Without it a restore
                // emits a bare URL and `apk add pkg@testing` stops resolving —
                // silently, because the repo is still there and only the tag
                // is gone. A test caught this.
                match &tag {
                    Some(t) => out.push_str(&format!("# was: @{t} {url}\n")),
                    None => out.push_str(&format!("# was: {url}\n")),
                }
                match &tag {
                    Some(t) => out.push_str(&format!("@{t} {new}\n")),
                    None => out.push_str(&format!("{new}\n")),
                }
            }
        }
    }
    out
}

/// Undo [`rewrite_repositories`], from the `# was:` breadcrumbs.
///
/// Worth having because the failure mode of a cache is that it goes away — the
/// laptop is closed, the network changed — and a phone whose repositories all
/// point at a machine that is not there cannot install anything. Restoring has
/// to work without the cache being reachable, which is why the originals are
/// in the file rather than in a database on the cache.
pub fn restore_repositories(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some(orig) = lines[i].trim().strip_prefix("# was: ") {
            out.push_str(orig);
            out.push('\n');
            // Skip the rewritten line that follows it.
            i += 2;
            continue;
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPOS: &str = "\
#/media/mmcblk0p1/apks
http://mirror.postmarketos.org/postmarketos/master
https://dl-cdn.alpinelinux.org/alpine/edge/main
https://dl-cdn.alpinelinux.org/alpine/edge/community
@testing https://dl-cdn.alpinelinux.org/alpine/edge/testing
";

    #[test]
    fn a_package_is_immutable_and_an_index_is_not() {
        // The whole correctness argument. The version is in the .apk filename,
        // so those bytes never change; APKINDEX is rewritten whenever the repo
        // does, and serving a stale one tells the phone a package does not
        // exist when it does.
        assert_eq!(Kind::of("alpine/edge/main/aarch64/busybox-1.36.1-r5.apk"), Kind::Package);
        assert_eq!(Kind::of("alpine/edge/main/aarch64/APKINDEX.tar.gz"), Kind::Index);
        assert_eq!(Kind::Package.ttl(), None, "forever");
        assert_eq!(Kind::Index.ttl(), Some(INDEX_TTL));
    }

    #[test]
    fn a_fresh_index_is_served_and_a_stale_one_is_refetched() {
        let store = Path::new("/srv/apkcache");
        let idx = "postmarketos/master/aarch64/APKINDEX.tar.gz";
        assert!(matches!(
            decide(idx, Some(Duration::from_secs(60)), store),
            Decision::Serve(_)
        ));
        assert!(matches!(
            decide(idx, Some(INDEX_TTL + Duration::from_secs(1)), store),
            Decision::FetchAndStore(_)
        ));
        assert!(matches!(decide(idx, None, store), Decision::FetchAndStore(_)));
    }

    #[test]
    fn an_old_package_is_still_served_because_age_cannot_make_it_wrong() {
        // A year-old .apk is the same bytes it was: the name carries the
        // version, and a rebuild gets a new -r. This is what makes the cache
        // worth running.
        let store = Path::new("/srv/apkcache");
        let pkg = "alpine/edge/main/aarch64/busybox-1.36.1-r5.apk";
        let ancient = Duration::from_secs(365 * 24 * 3600);
        assert!(matches!(decide(pkg, Some(ancient), store), Decision::Serve(_)));
    }

    #[test]
    fn an_unrecognised_file_is_passed_through_rather_than_guessed_at() {
        let store = Path::new("/srv/apkcache");
        assert_eq!(decide("some/dir/README", Some(Duration::ZERO), store), Decision::Passthrough);
        assert_eq!(Kind::of("x/APKINDEX.tar.gz.sig"), Kind::Index, "index sidecars are indexes");
    }

    #[test]
    fn a_request_cannot_write_outside_the_store() {
        // The path comes off the network. This is the one place a traversal
        // could land, so `..` is dropped rather than resolved.
        let store = Path::new("/srv/apkcache");
        let p = cache_path(store, "../../etc/passwd");
        assert_eq!(p, Path::new("/srv/apkcache/etc/passwd"));
        assert!(p.starts_with(store));
        let p = cache_path(store, "/alpine//edge/./main/x.apk");
        assert_eq!(p, Path::new("/srv/apkcache/alpine/edge/main/x.apk"));
    }

    #[test]
    fn the_upstream_host_is_kept_in_the_path_so_mirrors_do_not_collide() {
        let base = "http://cache.lan:8080";
        assert_eq!(
            rewrite_url(base, "https://dl-cdn.alpinelinux.org/alpine/edge/main"),
            "http://cache.lan:8080/dl-cdn.alpinelinux.org/alpine/edge/main"
        );
        // Two mirrors of the same repo path stay distinct.
        assert_ne!(
            rewrite_url(base, "https://a.example/alpine/edge/main"),
            rewrite_url(base, "https://b.example/alpine/edge/main")
        );
    }

    #[test]
    fn a_local_repository_is_left_alone() {
        // Turning an on-disk repo into an HTTP fetch would break installs that
        // currently work with no network at all — including the SD card a
        // wiped phone boots from.
        let base = "http://cache.lan:8080";
        assert_eq!(rewrite_url(base, "/media/mmcblk0p1/apks"), "/media/mmcblk0p1/apks");
        // And rewriting twice is a no-op.
        let once = rewrite_url(base, "https://dl-cdn.alpinelinux.org/alpine/edge/main");
        assert_eq!(rewrite_url(base, &once), once);
    }

    #[test]
    fn tags_and_comments_survive_a_rewrite() {
        let out = rewrite_repositories(REPOS, "http://cache.lan:8080");
        // A tagged repo keeps its tag, or `apk add pkg@testing` stops working.
        assert!(out.contains("@testing http://cache.lan:8080/dl-cdn.alpinelinux.org/alpine/edge/testing"));
        // A commented-out local repo stays commented and unrewritten.
        assert!(out.contains("#/media/mmcblk0p1/apks"));
        // Every active repo now points at the cache.
        for line in out.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            assert!(l.contains("cache.lan:8080"), "not rewritten: {l}");
        }
    }

    #[test]
    fn the_original_urls_ride_along_so_a_phone_can_be_restored_offline() {
        // The cache's failure mode is going away — laptop closed, different
        // network — and a phone pointing at a machine that is not there cannot
        // install anything. Restoring must not require the cache.
        let rewritten = rewrite_repositories(REPOS, "http://cache.lan:8080");
        assert!(rewritten.contains("# was: http://mirror.postmarketos.org/postmarketos/master"));
        let restored = restore_repositories(&rewritten);
        assert!(restored.contains("http://mirror.postmarketos.org/postmarketos/master"));
        assert!(!restored.contains("cache.lan"));
        assert!(restored.contains("@testing https://dl-cdn.alpinelinux.org/alpine/edge/testing"));
        assert!(restored.contains("#/media/mmcblk0p1/apks"), "comments survive the round trip");
    }
}
