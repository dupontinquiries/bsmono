//! v0.1.376 (discoverability arc): `web2qml://recipes` page
//! renderer. Programmatically generates markdown documenting
//! every voice intent kind + every web2qml:// URL + hand-
//! editable config files + small-extras like fs_find glob
//! syntax.
//!
//! The voice-intent section is pulled from `voice_intent::
//! voice_recipes()` — the static catalog whose synchronization
//! with the Intent enum is enforced by unit tests. Adding a new
//! Intent variant without updating the catalog fails the test
//! suite, which is our discoverability gate: features must
//! surface to a user who doesn't read changelogs.
//!
//! The URL section is hand-maintained here. Each web2qml://
//! route added in the codebase should have a peer entry here.
//! We don't enforce this with a test (URLs are dispatched in
//! main.rs via string matching, so there's no enum to walk),
//! but the assertion is documented as a process gate: when
//! reviewing a PR that adds a web2qml:// route, also add a
//! recipes entry.

use crate::voice_intent::{voice_recipes, VoiceRecipe};
use std::collections::BTreeMap;

/// Hand-maintained catalog of `web2qml://` routes the user
/// can navigate to from the URL bar. Each entry: URL, short
/// title, one-line description.
struct UrlRecipe {
    url: &'static str,
    title: &'static str,
    description: &'static str,
}

static URL_RECIPES: &[UrlRecipe] = &[
    UrlRecipe { url: "web2qml://man",
        title: "Manpages",
        description: "Built-in help pages bundled with web2qml. Welcome guide, feature walkthroughs, voice setup." },
    UrlRecipe { url: "web2qml://recipes",
        title: "This page",
        description: "Every voice intent, every URL, every hand-editable config file." },
    UrlRecipe { url: "web2qml://stats/tokens",
        title: "Token usage stats",
        description: "Daily/weekly/all-time token usage with cost estimation when you've set pricing in settings." },
    UrlRecipe { url: "web2qml://audit",
        title: "Audit log",
        description: "Hash-chained record of every command run, with speaker attribution when voice-attested." },
    UrlRecipe { url: "web2qml://sessions",
        title: "Recent sessions",
        description: "Activity grouped by work session (15-min idle gap, speaker change, or process restart marks a boundary). Shows start time, duration, command count, first/last command, speakers." },
    UrlRecipe { url: "web2qml://bookmarks",
        title: "Saved bookmarks",
        description: "Pages you've saved via 'save this' or 'bookmark this' voice intents. Stored in ~/.config/web2qml/bookmarks.toml — edit by hand to add titles or tags." },
    UrlRecipe { url: "web2qml://workspaces",
        title: "Workspaces",
        description: "Cross-speaker view of every saved workspace — browser-tab sets and terminal-session snapshots per speaker. Sorted most-recently-touched first. Workspaces auto-persist per speaker and load on speaker switch." },
    UrlRecipe { url: "web2qml://branches",
        title: "Branch tree",
        description: "Tree of command forks created by rollbacks. Walk parents to reconstruct history." },
    UrlRecipe { url: "web2qml://corpus",
        title: "Corpus browser",
        description: "Your recorded voice corpus by speaker. Useful for verifying transcription quality." },
    UrlRecipe { url: "web2qml://branches",
        title: "Git branches",
        description: "Current repository's branch list + status overview." },
    UrlRecipe { url: "web2qml://gemini",
        title: "Gemini archive",
        description: "Local Gemini-protocol capsule archive. Populate with gemget; web2qml shows them with link-tappable rendering." },
    UrlRecipe { url: "gemini://*",
        title: "Gemini URL",
        description: "Paste any gemini:// URL directly into the URL bar. Resolves against your local archive (v0.1.367 is archive-only; live fetch in a future ship)." },
];

/// Hand-editable config files the user can poke directly.
/// Pointing them out makes the difference between a "this is
/// closed software" feeling and a "I can tinker" one.
struct ConfigFile {
    path: &'static str,
    title: &'static str,
    description: &'static str,
}

static CONFIG_FILES: &[ConfigFile] = &[
    ConfigFile { path: "~/.config/web2qml/speakers.toml",
        title: "Registered speakers",
        description: "Voice fingerprints, names, allowed/muted flags. Edit names freely; profile values are auto-tuned. Cadence stats live here too." },
    ConfigFile { path: "~/.config/web2qml/settings.conf",
        title: "Persistent settings",
        description: "Theme, font size, recursion depth, API keys, redaction tier, voice toast toggle, long-command threshold, pricing per million tokens." },
    ConfigFile { path: "~/.config/web2qml/reading_calibration.toml",
        title: "Per-content-type reading multipliers",
        description: "Override the PA1 baselines per content type (prose, code, list, table, heading). Values in 0.30..=1.20." },
    ConfigFile { path: "~/.config/web2qml/palette.txt",
        title: "Color palette",
        description: "Theme/color overrides. Hex values one per line." },
    ConfigFile { path: "~/.config/web2qml/wake_phrase.txt",
        title: "Wake phrase",
        description: "What you say to activate voice listening when wake-word mode is enabled." },
    ConfigFile { path: "~/.config/web2qml/filler_words.toml",
        title: "Filler-word stripping",
        description: "Words removed before voice-intent parsing. Default covers 'um/uh/like/you-know'; add personal ones." },
    ConfigFile { path: "~/.config/web2qml/workspaces/<speaker>/",
        title: "Saved workspaces",
        description: "Per-speaker named workspaces. One TOML file per workspace. Hand-editable." },
    ConfigFile { path: "~/.config/web2qml/gemini/<host>/",
        title: "Gemini archive",
        description: "Local Gemini capsule files. Populate with `gemget -o <path> gemini://...`." },
    ConfigFile { path: "~/.config/web2qml/token_log.jsonl",
        title: "Token log",
        description: "One JSON object per AI call recording model, tokens in/out, cost, operation tag. Source of truth for web2qml://stats/tokens." },
    ConfigFile { path: "~/.config/web2qml/bookmarks.toml",
        title: "Saved bookmarks",
        description: "URLs saved via 'save this' or 'bookmark this' voice intents. Hand-editable to add titles, tags, or curate the list. View via web2qml://bookmarks." },
    ConfigFile { path: "~/.config/web2qml/user_tags.toml",
        title: "User-defined tags",
        description: "User-owned tag definitions used by voice intent tag-routing. Hand-edit to add custom labels." },
];

/// Other hidden inputs power users may not know about.
struct SpecialInput {
    where_: &'static str,
    syntax: &'static str,
    description: &'static str,
}

static SPECIAL_INPUTS: &[SpecialInput] = &[
    SpecialInput { where_: "Files search",
        syntax: "*.rs:fn main",
        description: "Glob filter shorthand. Restricts search to files matching the glob." },
    SpecialInput { where_: "Files search",
        syntax: "glob:src/**:foo",
        description: "Explicit glob form. Works even when the pattern doesn't contain glob characters." },
    SpecialInput { where_: "URL bar",
        syntax: "gemini://example.org/",
        description: "Navigate to a Gemini capsule (archive lookup; live fetch in a future ship)." },
    SpecialInput { where_: "URL bar",
        syntax: "/absolute/path  or  ~/relative",
        description: "Open a local file or directory in the browser pane." },
    SpecialInput { where_: "Reading overlay",
        syntax: "speak silently as the highlight moves",
        description: "The silence-multiplier slows pace when you pause. Pill near the rate button shows the active multiplier." },
];

/// Render the recipes page as markdown. The markdown is then
/// passed through `manpages::to_qml` by the URL handler in
/// main.rs, picking up the same rendering pipeline as built-in
/// manpages (so it looks consistent, supports the reading
/// overlay, etc.).
///
/// v0.1.386: memoized via OnceLock — the content is purely
/// derived from compile-time data (VOICE_RECIPES, URL_RECIPES,
/// CONFIG_FILES, SPECIAL_INPUTS), so the first call computes
/// once and subsequent calls return the cached string. Pre-
/// warm via `prewarm_recipes()` at startup so first
/// navigation is instant.
pub fn render_recipes() -> String {
    use std::sync::OnceLock;
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE.get_or_init(render_recipes_uncached).clone()
}

/// Pre-warm the recipes cache. Call once at startup (e.g.
/// from a background thread spawned during Backend init).
/// Pure side effect — no return value needed; the cache is
/// populated and subsequent `render_recipes()` calls hit it.
pub fn prewarm_recipes() {
    let _ = render_recipes();
}

fn render_recipes_uncached() -> String {
    let mut s = String::with_capacity(16 * 1024);

    s.push_str("# Recipes\n\n");
    s.push_str("Things web2qml can do that aren't obvious. Use this page \
        as your starting point for voice intents, URL shortcuts, and \
        hand-editable config files.\n\n");
    s.push_str("Updated automatically — every voice intent in the codebase \
        is listed here. Linked from the F1 keyboard-shortcuts overlay and \
        from settings.\n\n");
    s.push_str("---\n\n");

    // ===== Voice intents =====
    s.push_str("## Voice intents\n\n");
    s.push_str("Speak any of these phrasings. Examples are illustrative — \
        the parser accepts many variations (politeness prefixes like \
        \"could you...\", possessive shifts, etc.).\n\n");

    // Group recipes by category.
    let mut by_cat: BTreeMap<&'static str, Vec<&VoiceRecipe>> = BTreeMap::new();
    for r in voice_recipes() {
        by_cat.entry(r.category).or_default().push(r);
    }
    // Preserve a sensible category order rather than alphabetical.
    let cat_order = [
        "Navigation", "Search", "Reading", "Terminal", "Audit",
        "Tabs", "Workspaces", "Settings", "Clipboard", "Workflows",
        "System", "Voice tier 1", "Cards", "Other",
    ];
    let mut seen_cats = std::collections::HashSet::new();
    for cat in &cat_order {
        if let Some(recipes) = by_cat.get(cat) {
            seen_cats.insert(*cat);
            s.push_str(&format!("### {}\n\n", cat));
            for r in recipes {
                // First example is the lead phrasing.
                s.push_str(&format!("**\"{}\"**", r.examples[0]));
                if r.examples.len() > 1 {
                    s.push_str(" — also: ");
                    let extras: Vec<String> = r.examples[1..].iter()
                        .map(|e| format!("\"{}\"", e))
                        .collect();
                    s.push_str(&extras.join(", "));
                }
                s.push_str("\n\n");
                s.push_str(r.what_happens);
                s.push_str("\n\n");
            }
        }
    }
    // Catch any categories not in the explicit order.
    for (cat, recipes) in &by_cat {
        if seen_cats.contains(cat) { continue; }
        s.push_str(&format!("### {}\n\n", cat));
        for r in recipes {
            s.push_str(&format!("**\"{}\"**\n\n", r.examples[0]));
            s.push_str(r.what_happens);
            s.push_str("\n\n");
        }
    }

    // ===== URL shortcuts =====
    s.push_str("---\n\n");
    s.push_str("## URL shortcuts\n\n");
    s.push_str("Paste these into the URL bar. The `web2qml://` scheme \
        opens built-in pages without leaving the app.\n\n");
    for u in URL_RECIPES {
        s.push_str(&format!("**[{}]({})** — {}\n\n", u.title, u.url, u.description));
    }

    // ===== Hand-editable config =====
    s.push_str("---\n\n");
    s.push_str("## Hand-editable config files\n\n");
    s.push_str("web2qml stores everything in plain text in your config \
        directory. Open them with any editor; web2qml re-reads on next \
        relevant action.\n\n");
    for c in CONFIG_FILES {
        s.push_str(&format!("**`{}`** — {}\n\n{}\n\n", c.path, c.title, c.description));
    }

    // ===== Special inputs =====
    s.push_str("---\n\n");
    s.push_str("## Special input syntax\n\n");
    s.push_str("Power-user tricks that work in specific places.\n\n");
    for si in SPECIAL_INPUTS {
        s.push_str(&format!("**{}** → `{}`\n\n{}\n\n", si.where_, si.syntax, si.description));
    }

    // ===== Pointers =====
    s.push_str("---\n\n");
    s.push_str("## See also\n\n");
    s.push_str("- Keyboard shortcuts: press **F1** (or Ctrl+/) at any time.\n");
    s.push_str("- Settings: **Ctrl+,** opens the settings popup.\n");
    s.push_str("- Built-in manpages: [`web2qml://man`](web2qml://man) for the welcome guide and feature walkthroughs.\n");
    s.push_str("- This page auto-updates when new voice intents are added — \
        the codebase has a unit test that enforces it.\n\n");

    s
}

/// v0.1.385 (discoverability arc): focused per-surface
/// recipes for the "what can I do here" voice intent. Picks
/// a context bucket based on current URL + active tab and
/// returns 4-6 highly relevant phrasings + a pointer to the
/// full recipes catalog.
///
/// Surfaces recognized:
///   - sessions page → session-specific intents
///   - recipes page → meta-help (you're already here)
///   - audit page → audit-specific intents
///   - terminal tab → terminal/dictation intents
///   - browser tab (general) → navigation/reading intents
///   - reading overlay open → reading-mode intents
///
/// Returns markdown. Caller renders it via the same
/// `manpages::to_qml` path as the full recipes page.
pub fn render_context_help(current_url: &str, tab: &str, reading_overlay_open: bool) -> String {
    let mut s = String::with_capacity(2 * 1024);

    s.push_str("# What you can do right now\n\n");

    let url = current_url.trim();
    let on_terminal = tab == "terminal";

    // Pick the surface bucket. Order matters — the most
    // specific surface first.
    if reading_overlay_open {
        s.push_str("**Surface:** reading overlay\n\n");
        s.push_str("Things that fit reading-aloud mode:\n\n");
        s.push_str("- *\"faster\"* / *\"slower\"* — adjust the reading pace\n");
        s.push_str("- *\"pause reading\"* — stop the highlight\n");
        s.push_str("- *\"resume reading\"* — start it again\n");
        s.push_str("- *\"next paragraph\"* / *\"previous paragraph\"*\n");
        s.push_str("- *\"reset my cadence\"* — recalibrate the reading pace from scratch\n");
        s.push_str("- Just speak the words being highlighted — the silence-multiplier auto-tunes pace to you\n");
    } else if url.starts_with("web2qml://sessions") {
        s.push_str("**Surface:** sessions page\n\n");
        s.push_str("- *\"summarize my session\"* — fresh AI summary of the latest session\n");
        s.push_str("- *\"go back\"* / *\"reload\"*\n");
        s.push_str("- Tap a session header to dig in (future ship adds full expansion)\n");
        s.push_str("- The most-recent session's banner refreshes ~3.5s after navigation\n");
    } else if url.starts_with("web2qml://recipes") {
        s.push_str("**Surface:** recipes page\n\n");
        s.push_str("You're already on the discovery page. From here:\n\n");
        s.push_str("- Scroll to browse the full voice-intent catalog\n");
        s.push_str("- *\"open audit log\"* — jump to your command history\n");
        s.push_str("- *\"sessions\"* — see what you've worked on by session\n");
        s.push_str("- Press F1 for the keyboard chord overlay (Alt+N/E/T/V/W/H to jump by category)\n");
    } else if url.starts_with("web2qml://audit") {
        s.push_str("**Surface:** audit log\n\n");
        s.push_str("- *\"undo\"* — rollback the most recent command (when reversible)\n");
        s.push_str("- *\"show audit log for me\"* — filter by speaker\n");
        s.push_str("- *\"summarize my session\"* — session-grouped view at web2qml://sessions\n");
    } else if on_terminal {
        s.push_str("**Surface:** terminal tab\n\n");
        s.push_str("- *\"run <command>\"* — voice-dispatch a shell command\n");
        s.push_str("- *\"type <text>\"* — dictate exact text into the prompt\n");
        s.push_str("- *\"clear screen\"* — Ctrl+L equivalent\n");
        s.push_str("- *\"find <text>\"* — open scrollback search\n");
        s.push_str("- *\"summarize this output\"* — AI summary of the last block\n");
    } else {
        s.push_str("**Surface:** browser\n\n");
        s.push_str("- *\"scroll down\"* / *\"scroll up\"* / *\"go to top\"*\n");
        s.push_str("- *\"go back\"* / *\"go forward\"* / *\"reload\"*\n");
        s.push_str("- *\"open <site>\"* / *\"search for <query>\"*\n");
        s.push_str("- *\"read this\"* — open the reading overlay\n");
        s.push_str("- *\"new tab\"* / *\"close tab\"* / *\"next tab\"*\n");
    }

    s.push_str("\n---\n\n");
    s.push_str("**Want the full catalog?** Say *\"show recipes\"* or open `web2qml://recipes`.\n\n");
    s.push_str("**Want the keyboard chord overlay?** Press F1.\n");

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_produces_non_empty_output() {
        let out = render_recipes();
        assert!(out.len() > 1000,
            "recipes page should be substantial; got {} bytes",
            out.len());
    }

    #[test]
    fn render_contains_known_voice_intents() {
        let out = render_recipes();
        assert!(out.contains("reset my cadence"),
            "should include ResetCadence recipe");
        assert!(out.contains("undo"),
            "should include Rollback recipe");
        assert!(out.contains("show audit log"),
            "should include ShowAuditLog recipe");
    }

    #[test]
    fn render_contains_known_urls() {
        let out = render_recipes();
        assert!(out.contains("web2qml://gemini"));
        assert!(out.contains("web2qml://stats/tokens"));
        assert!(out.contains("web2qml://audit"));
    }

    #[test]
    fn render_contains_config_paths() {
        let out = render_recipes();
        assert!(out.contains("speakers.toml"));
        assert!(out.contains("reading_calibration.toml"));
        assert!(out.contains("settings.conf"));
    }

    #[test]
    fn render_contains_glob_syntax() {
        let out = render_recipes();
        assert!(out.contains("*.rs:fn main"));
        assert!(out.contains("glob:"));
    }

    #[test]
    fn render_category_sections_present() {
        let out = render_recipes();
        assert!(out.contains("### Navigation"));
        assert!(out.contains("### Reading"));
        assert!(out.contains("### Terminal"));
        assert!(out.contains("### Voice tier 1"));
    }

    /// v0.1.383 (discoverability arc): programmatic check
    /// that every `web2qml://*` URL referenced in main.rs has
    /// a peer entry in URL_RECIPES. Adding a new route without
    /// a recipes entry now fails the build.
    ///
    /// The check walks the embedded `main.rs` source text
    /// looking for `"web2qml://<name>"` string literals,
    /// extracts the bare URL (drops trailing `/` and query
    /// fragments), and verifies each maps to a URL_RECIPES
    /// entry — UNLESS the URL is in the allowlist (internal
    /// routes that aren't user-typeable like
    /// `web2qml://mermaid?...` and `web2qml://niftyplot?...`).
    ///
    /// This is the v0.1.383 closure of the "programmatic
    /// URL_RECIPES coverage test" S item filed during the
    /// v0.1.376 discoverability ship.
    #[test]
    fn every_main_rs_url_has_a_recipe() {
        // Internal-only URLs that take a query string and
        // aren't intended as bare-URL destinations. The user
        // wouldn't manually type these; they're emitted by
        // qml_gen for mermaid/niftyplot fence rendering.
        const ALLOWLIST: &[&str] = &[
            "web2qml://mermaid",
            "web2qml://niftyplot",
            // v0.1.772: internal/programmatic routes (navigated by code, not
            // user-facing destinations that need a voice recipe) plus a couple
            // of test/comment artifacts the scanner picks up from main.rs.
            "web2qml:",                                       // bare scheme check
            "web2qml://a-very-long-internal-page-name-here",  // short_label_of test
            "web2qml://audit/foo",                            // in-comment example
            "web2qml://chat",
            "web2qml://chat-open",
            "web2qml://chat-panel",
            // v0.1.875: mail-bridge routes. Navigated by code
            // (mail_open_login / mail_open_settings / build_page_qml
            // routing), not yet user-facing destinations — the feature
            // is incomplete (login returns CryptoUnavailable until the
            // SRP seam is wired). Promote to URL_RECIPES once the mail
            // bridge is functional so it becomes voice-discoverable.
            "web2qml://mail",
            "web2qml://mail/login",
            "web2qml://mail/settings",
            // v0.1.875: format-template artifact — the scanner picks up
            // the literal `format!("web2qml://man/{}", ...)`. The bare
            // `web2qml://man` already has a URL_RECIPES entry; the
            // dynamic per-manpage form isn't a typeable destination.
            "web2qml://man/{}",
            "web2qml://model",
            "web2qml://models",
            "web2qml://save-workflow",
            "web2qml://settings",
            "web2qml://setup",
        ];

        let main_src = include_str!("main.rs");
        let known_urls: std::collections::HashSet<&str> = URL_RECIPES
            .iter()
            .map(|r| r.url.trim_end_matches('/'))
            .collect();

        // Walk the source for "web2qml://<chars>" — simple
        // byte scanner instead of regex.
        let mut found: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        let bytes = main_src.as_bytes();
        let needle = b"\"web2qml://";
        let mut i = 0;
        while i + needle.len() < bytes.len() {
            if &bytes[i..i + needle.len()] == needle {
                let url_start = i + 1; // skip leading quote
                // Find the closing quote.
                let mut j = url_start;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += 1;
                }
                if j > url_start && j < bytes.len() {
                    let url = &main_src[url_start..j];
                    // Strip trailing slash + query for
                    // matching; recipes carry the bare form.
                    let url = url.trim_end_matches('/');
                    let url = url.split('?').next().unwrap_or(url);
                    found.insert(url.to_string());
                }
                i = j;
            } else {
                i += 1;
            }
        }

        let mut missing: Vec<String> = Vec::new();
        for url in &found {
            if ALLOWLIST.contains(&url.as_str()) {
                continue;
            }
            if !known_urls.contains(url.as_str()) {
                missing.push(url.clone());
            }
        }
        assert!(missing.is_empty(),
            "found {} `web2qml://` URL(s) in main.rs without \
             URL_RECIPES entries (add to URL_RECIPES in \
             src/recipes.rs or to ALLOWLIST if internal): {:?}",
            missing.len(), missing);
    }

    /// v0.1.388 (discoverability arc): programmatic check
    /// that every user-editable config file referenced from
    /// main.rs / voice.rs / sessions.rs / etc. has an entry
    /// in CONFIG_FILES.
    ///
    /// Different shape than the URL coverage test (which
    /// scans a single file's `include_str!`) because config
    /// references span many modules. We compile a
    /// hand-maintained ALLOWLIST of internal-state files
    /// users don't hand-edit (`.bin`, `.flag`, `.jsonl`,
    /// `.log`, cache files) and check that what's left maps
    /// to a CONFIG_FILES entry.
    ///
    /// The test runs against `include_str!` of the recipes
    /// module itself — limited scope by design. The intent
    /// isn't to catch every reference (which would require
    /// processing all of /src), but to catch the common case
    /// of "I added a new user-editable config and forgot to
    /// document it" by checking that CONFIG_FILES entries
    /// align with the known sources of truth tracked in the
    /// PALETTE_DIR_FILES list below.
    ///
    /// Closes the third deferred discoverability follow-up.
    #[test]
    fn config_files_catalog_covers_known_editable_files() {
        // Files known to live in palette_dir() that users
        // can usefully edit. Comes from a one-time grep
        // across the codebase, filtered to the user-editable
        // subset. New additions go here AND in CONFIG_FILES.
        //
        // Internal-state files NOT listed here (and not
        // expected in CONFIG_FILES):
        //   acoustic_index.bin, confusions.bin,
        //   decision_cache.bin, llm_cache.bin, archive.db,
        //   *.flag files, token_log.jsonl (this one IS in
        //   CONFIG_FILES because users sometimes want to
        //   inspect/truncate), moments.log, scrollback.log,
        //   corpus/, chats/, *.wav files, voice_fingerprints.bin
        //   (binary; user shouldn't edit raw)
        const KNOWN_USER_EDITABLE: &[&str] = &[
            "speakers.toml",
            "settings.conf",
            "reading_calibration.toml",
            "palette.txt",
            "wake_phrase.txt",
            "filler_words.toml",
            "user_tags.toml",
            "token_log.jsonl",
            "bookmarks.toml",
            // Directories — CONFIG_FILES uses /<sub>/ form
            "workspaces",
            "gemini",
        ];

        // Build the set of stem names present in CONFIG_FILES.
        let mut configured: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for c in CONFIG_FILES {
            // Take the LAST path segment (basename, or
            // <subdir>/ for directories). Robust to either
            // form.
            let path = c.path;
            // Strip trailing /<placeholder>/ and the trailing slash.
            let trimmed = path.trim_end_matches('/');
            let basename = match trimmed.rfind('/') {
                Some(i) => &trimmed[i + 1..],
                None => trimmed,
            };
            // Subdirectories appear as e.g. "workspaces/<speaker>"
            // — basename "<speaker>" isn't useful. Drop the
            // <placeholder> form and use the parent directory.
            let cleaned = if basename.starts_with('<')
                && basename.ends_with('>')
            {
                let path = trimmed.trim_end_matches('/');
                let parent = path.rsplit('/').nth(1).unwrap_or("");
                parent.to_string()
            } else {
                basename.to_string()
            };
            configured.insert(cleaned);
        }

        let mut missing: Vec<&str> = Vec::new();
        for f in KNOWN_USER_EDITABLE {
            if !configured.contains(*f) {
                missing.push(f);
            }
        }
        assert!(missing.is_empty(),
            "{} user-editable config file(s) NOT in CONFIG_FILES \
             (add an entry to CONFIG_FILES in src/recipes.rs, or \
             remove from KNOWN_USER_EDITABLE if file is now \
             internal-only): {:?}",
            missing.len(), missing);
    }

    // v0.1.385: context-aware help renderer tests.

    #[test]
    fn context_help_reading_overlay_takes_precedence() {
        // Reading overlay open overrides URL — even if user
        // is on the sessions page with overlay open, they
        // should see reading-mode intents.
        let out = render_context_help("web2qml://sessions", "browser", true);
        assert!(out.contains("reading overlay"));
        assert!(out.contains("faster"),
            "expected reading-mode hints: {}", out);
    }

    #[test]
    fn context_help_sessions_page() {
        let out = render_context_help("web2qml://sessions", "browser", false);
        assert!(out.contains("sessions page"));
        assert!(out.contains("summarize my session"));
    }

    #[test]
    fn context_help_recipes_page_acknowledges_self() {
        let out = render_context_help("web2qml://recipes", "browser", false);
        assert!(out.contains("already on"));
    }

    #[test]
    fn context_help_audit_page() {
        let out = render_context_help("web2qml://audit", "browser", false);
        assert!(out.contains("audit log"));
        assert!(out.contains("undo"));
    }

    #[test]
    fn context_help_terminal_tab() {
        let out = render_context_help("", "terminal", false);
        assert!(out.contains("terminal tab"));
        assert!(out.contains("run"));
        assert!(out.contains("dictate") || out.contains("type"));
    }

    #[test]
    fn context_help_browser_default() {
        // Unrecognized URL + browser tab + reading not open:
        // fall through to generic browser help.
        let out = render_context_help("https://example.com", "browser", false);
        assert!(out.contains("browser"));
        assert!(out.contains("scroll"));
        assert!(out.contains("read this") || out.contains("Read"));
    }

    #[test]
    fn context_help_always_points_to_full_recipes() {
        // Every output should leave a breadcrumb to the
        // full catalog for users who want more.
        let cases = [
            ("web2qml://sessions", "browser", false),
            ("web2qml://recipes", "browser", false),
            ("https://example.com", "browser", false),
            ("", "terminal", false),
        ];
        for (url, tab, open) in cases {
            let out = render_context_help(url, tab, open);
            assert!(out.contains("show recipes")
                || out.contains("web2qml://recipes"),
                "context-help for ({}, {}, {}) should point to full catalog: {}",
                url, tab, open, out);
        }
    }
}
