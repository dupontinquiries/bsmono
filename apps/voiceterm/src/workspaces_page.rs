//! v0.1.389: render `web2qml://workspaces` — a cross-speaker
//! view of every saved workspace, with browser-tab counts,
//! terminal-session counts, and updated-time. The workspaces
//! data layer already exists in `workspaces.rs` (load/save
//! per speaker); this module is an exposure layer for it.
//!
//! Why a new module instead of extending workspaces.rs: the
//! rendering logic touches speakers.rs (display names) and
//! the markdown helpers shared by sessions/bookmarks/recipes,
//! while workspaces.rs is the pure data-model layer.
//! Separating keeps each focused.

use crate::{speakers, workspaces};

/// Build the workspaces-page markdown. Same render pipeline
/// as sessions, recipes, bookmarks — flows through
/// `manpages::to_qml` in the URL handler.
pub fn render_workspaces_page() -> String {
    render_workspaces_page_with_live(None)
}

/// v0.1.392: per-tab snapshot of the currently-running
/// browser session. Built by Backend from `_tabs`,
/// `_tab_opened_ms`, and the active speaker; passed in so
/// the workspaces page can show live state at the top
/// before the persisted list.
#[derive(Debug, Clone, Default)]
pub struct LiveSession {
    /// Active speaker display name (empty when no speaker
    /// has been identified yet).
    pub speaker_display: String,
    /// Per-tab (url, opened_ms) pairs. Empty when only one
    /// tab is open (single-tab UI hides the strip; we treat
    /// single-tab sessions as "no live workspace state to
    /// show" for parity with how the strip behaves).
    pub tabs: Vec<(String, u64)>,
    /// Active tab index. Clamped to `tabs.len()`.
    pub active_tab: usize,
}

/// v0.1.392: render with optional live-session injection.
/// When `live` is Some and contains ≥2 tabs, a "Current
/// session" section appears at the top of the page with
/// per-tab ages from the live state. When None or empty,
/// the page renders just the persisted-on-disk list (the
/// original behavior).
pub fn render_workspaces_page_with_live(live: Option<&LiveSession>) -> String {
    let speakers_with_ws = workspaces::list_speakers();

    let mut rows: Vec<(String, String, workspaces::Workspace)> =
        Vec::new();
    for sid in &speakers_with_ws {
        let display = speakers::name_for_id(sid)
            .unwrap_or_else(|| sid.clone());
        for ws_name in workspaces::list(sid) {
            if let Some(ws) = workspaces::load(sid, &ws_name) {
                rows.push((sid.clone(), display.clone(), ws));
            }
        }
    }

    // Sort by updated_ms descending so the most-recently-
    // touched workspace appears at the top regardless of
    // which speaker.
    rows.sort_by(|a, b| b.2.updated_ms.cmp(&a.2.updated_ms));

    render_rows_with_live(&rows, live)
}

fn render_rows(rows: &[(String, String, workspaces::Workspace)]) -> String {
    render_rows_with_live(rows, None)
}

fn render_rows_with_live(
    rows: &[(String, String, workspaces::Workspace)],
    live: Option<&LiveSession>,
) -> String {
    let mut s = String::with_capacity(2 * 1024);
    s.push_str("# Workspaces\n\n");

    // v0.1.392: live-session section at top when present.
    // Only render when there are ≥2 tabs (matches the
    // strip's single-tab-hide behavior — a 1-tab "session"
    // isn't workspace-worthy).
    if let Some(live) = live {
        if live.tabs.len() >= 2 {
            s.push_str("## Current session");
            if !live.speaker_display.is_empty() {
                s.push_str(&format!(" — {}", live.speaker_display));
            }
            s.push_str("\n\n");
            s.push_str(&format!("**{} browser tab(s) open right now** \
                — not yet saved. Use \"save workspace as <name>\" \
                to snapshot.\n\n", live.tabs.len()));
            let now = current_ms_for_render();
            for (idx, (url, opened_ms)) in live.tabs.iter().enumerate() {
                let active_marker = if idx == live.active_tab {
                    " ← active"
                } else {
                    ""
                };
                let short = short_url(url);
                let age = format_relative_ago(*opened_ms, now);
                s.push_str(&format!("- `{}`{}  *(opened {})*\n",
                    short, active_marker, age));
            }
            s.push_str("\n---\n\n");
            s.push_str("## Saved workspaces\n\n");
        }
    }

    s.push_str("Saved browser-tab sets and terminal-session \
        snapshots, grouped by speaker. Workspaces persist \
        per-speaker; switching speakers loads that person's \
        active workspace automatically.\n\n");

    if rows.is_empty() {
        s.push_str("*No workspaces saved yet. Open a few tabs \
            and they'll be persisted to your speaker's \
            default workspace on the next save.*\n\n");
        s.push_str("To create a named workspace: voice \
            *\"save workspace as <name>\"* (S follow-up — \
            command not yet wired).\n");
        return s;
    }

    s.push_str(&format!("**{} workspace(s) across {} speaker(s).**\n\n",
        rows.len(),
        count_unique_speakers(rows),
    ));

    for (i, (_sid, display, ws)) in rows.iter().enumerate() {
        if i > 0 {
            s.push_str("\n---\n\n");
        }
        s.push_str(&format!("## {} — {}\n\n", display, ws.name));
        s.push_str(&format!("*Last touched {}*\n\n",
            format_ago_ms(ws.updated_ms)));

        let tab_count = ws.browser_tabs.len();
        let term_count = ws.terminal_sessions.len();
        s.push_str(&format!("- {} browser tab{}\n",
            tab_count, if tab_count == 1 { "" } else { "s" }));
        s.push_str(&format!("- {} terminal session{}\n",
            term_count, if term_count == 1 { "" } else { "s" }));

        if !ws.browser_tabs.is_empty() {
            s.push_str("\n**Tabs:**\n");
            for (idx, url) in ws.browser_tabs.iter().enumerate() {
                let active_marker = if idx == ws.active_browser_tab {
                    " ← active"
                } else {
                    ""
                };
                let short = short_url(url);
                s.push_str(&format!("- `{}`{}\n", short, active_marker));
            }
        }

        if !ws.terminal_sessions.is_empty() {
            s.push_str("\n**Terminal sessions:**\n");
            for (idx, term) in ws.terminal_sessions.iter().enumerate() {
                let active_marker = if idx == ws.active_terminal {
                    " ← active"
                } else {
                    ""
                };
                let cwd = if term.cwd.is_empty() {
                    "(no cwd)".to_string()
                } else {
                    term.cwd.clone()
                };
                let recent = term.recent_commands.last()
                    .map(|c| truncate(c, 60))
                    .unwrap_or_else(|| "(no commands)".to_string());
                s.push_str(&format!(
                    "- `{}`{}\n  Last command: `{}`\n",
                    cwd, active_marker, recent));
            }
        }
        s.push_str("\n");
    }

    s
}

fn count_unique_speakers(
    rows: &[(String, String, workspaces::Workspace)]
) -> usize {
    let mut seen: std::collections::HashSet<&str> =
        std::collections::HashSet::new();
    for (sid, _, _) in rows {
        seen.insert(sid.as_str());
    }
    seen.len()
}

/// Shorten a URL for the workspace listing. Strips scheme,
/// caps at 80 chars with unicode-safe ellipsis truncation.
fn short_url(url: &str) -> String {
    let stripped = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    truncate(stripped, 80)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) { cut -= 1; }
    format!("{}…", &s[..cut])
}

/// Format a unix-ms timestamp as a human-readable "X ago"
/// string. Approximations:
///   < 60s     → "just now"
///   < 60m     → "Nm ago"
///   < 24h     → "Nh ago"
///   < 7d      → "Nd ago"
///   < 30d     → "Nw ago"
///   < 12mo    → "Nmo ago"
///   otherwise → "Ny ago"
fn format_ago_ms(ts_ms: u64) -> String {
    let now_ms = current_ms_for_render();
    format_relative_ago(ts_ms, now_ms)
}

/// v0.1.392: same formatter as `format_ago_ms` but takes an
/// explicit `now_ms`. Lets a single render pass use a
/// consistent "now" across many rows — without this, two
/// rows rendered milliseconds apart could disagree on
/// whether "59s ago" rounded to "just now" or "1m ago".
fn format_relative_ago(ts_ms: u64, now_ms: u64) -> String {
    if ts_ms == 0 || ts_ms > now_ms {
        return "(unknown time)".to_string();
    }
    let delta_s = (now_ms - ts_ms) / 1000;
    if delta_s < 60 {
        return "just now".to_string();
    }
    let m = delta_s / 60;
    if m < 60 { return format!("{}m ago", m); }
    let h = m / 60;
    if h < 24 { return format!("{}h ago", h); }
    let d = h / 24;
    if d < 7 { return format!("{}d ago", d); }
    let w = d / 7;
    if w < 5 { return format!("{}w ago", w); }
    let mo = d / 30;
    if mo < 12 { return format!("{}mo ago", mo); }
    let y = d / 365;
    format!("{}y ago", y)
}

/// v0.1.392: current unix-ms, shared with the format helpers.
fn current_ms_for_render() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::{Workspace, TerminalSession};

    fn make_ws(name: &str, tabs: &[&str], term_cwds: &[&str], updated_ms: u64) -> Workspace {
        Workspace {
            name: name.to_string(),
            browser_tabs: tabs.iter().map(|s| s.to_string()).collect(),
            active_browser_tab: 0,
            terminal_sessions: term_cwds.iter().map(|cwd| TerminalSession {
                cwd: cwd.to_string(),
                recent_commands: vec![],
            }).collect(),
            active_terminal: 0,
            created_ms: 0,
            updated_ms,
        }
    }

    #[test]
    fn render_empty_shows_message() {
        let out = render_rows(&[]);
        assert!(out.contains("No workspaces"));
    }

    #[test]
    fn render_single_workspace() {
        let ws = make_ws("default",
            &["https://a.com", "https://b.com"], &["/tmp"], 0);
        let out = render_rows(&[
            ("alice_id".into(), "Alice".into(), ws),
        ]);
        assert!(out.contains("Alice"));
        assert!(out.contains("default"));
        assert!(out.contains("2 browser tabs"));
        assert!(out.contains("1 terminal session"));
        assert!(out.contains("a.com"));
        assert!(out.contains("b.com"));
    }

    #[test]
    fn render_active_marker_on_active_tab() {
        let mut ws = make_ws("default",
            &["https://a.com", "https://b.com"], &[], 0);
        ws.active_browser_tab = 1;
        let out = render_rows(&[
            ("alice_id".into(), "Alice".into(), ws),
        ]);
        // The active tab line should contain the active
        // marker, the other shouldn't.
        let b_pos = out.find("b.com").expect("b.com should be in output");
        let after_b = &out[b_pos..];
        assert!(after_b.contains("← active"),
            "active marker should appear after b.com: {}", after_b);
    }

    #[test]
    fn render_pluralization_correct() {
        let ws = make_ws("default", &["https://a.com"], &[], 0);
        let out = render_rows(&[
            ("alice_id".into(), "Alice".into(), ws),
        ]);
        assert!(out.contains("1 browser tab\n"),
            "singular form for 1 tab: {}", out);
        assert!(out.contains("0 terminal sessions"));
    }

    #[test]
    fn render_orders_newest_first() {
        // render_rows assumes rows are pre-sorted by caller,
        // but verify the output preserves order (so the
        // public-API sort matters).
        let old = make_ws("old", &["https://old.com"], &[], 100);
        let new = make_ws("new", &["https://new.com"], &[], 1000);
        let out = render_rows(&[
            ("a".into(), "Alice".into(), new),
            ("a".into(), "Alice".into(), old),
        ]);
        let new_pos = out.find("new.com").unwrap();
        let old_pos = out.find("old.com").unwrap();
        assert!(new_pos < old_pos);
    }

    #[test]
    fn short_url_strips_scheme() {
        assert_eq!(short_url("https://example.com/path"),
            "example.com/path");
        assert_eq!(short_url("http://example.com"),
            "example.com");
        assert_eq!(short_url("gemini://example.org"),
            "gemini://example.org");
    }

    #[test]
    fn short_url_truncates_long() {
        let long = "example.com/".to_string() + &"x".repeat(100);
        let url = format!("https://{}", long);
        let result = short_url(&url);
        assert!(result.ends_with('…'));
        assert!(result.chars().count() <= 81);
    }

    #[test]
    fn truncate_unicode_safe() {
        let s = "日本".repeat(50);
        let t = truncate(&s, 50);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn format_ago_ranges() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        assert!(format_ago_ms(now - 10_000).contains("just now")
                || format_ago_ms(now - 10_000).contains("s ago")
                || format_ago_ms(now - 10_000).contains("m ago"),
            "10s old should be just-now-ish");
        assert!(format_ago_ms(now - 5 * 60 * 1000).ends_with("m ago"));
        assert!(format_ago_ms(now - 3 * 3600 * 1000).ends_with("h ago"));
        assert!(format_ago_ms(now - 2 * 86_400 * 1000).ends_with("d ago"));
        assert!(format_ago_ms(now - 10 * 86_400 * 1000).ends_with("w ago"));
        // 60 days = about 2mo
        assert!(format_ago_ms(now - 60 * 86_400 * 1000).ends_with("mo ago"));
    }

    #[test]
    fn format_ago_zero_and_future() {
        assert_eq!(format_ago_ms(0), "(unknown time)");
        // Future timestamps shouldn't crash; treat as unknown.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        assert_eq!(format_ago_ms(now + 60_000_000), "(unknown time)");
    }

    #[test]
    fn render_multiple_speakers_correctly_counted() {
        let ws1 = make_ws("default", &[], &[], 0);
        let ws2 = make_ws("project-a", &[], &[], 0);
        let ws3 = make_ws("default", &[], &[], 0);
        let out = render_rows(&[
            ("alice".into(), "Alice".into(), ws1),
            ("alice".into(), "Alice".into(), ws2),
            ("bob".into(), "Bob".into(), ws3),
        ]);
        assert!(out.contains("3 workspace(s) across 2 speaker(s)"),
            "should count distinct speakers: {}", out);
    }

    // v0.1.392: live-session injection tests.

    #[test]
    fn render_no_live_section_when_live_is_none() {
        let out = render_rows_with_live(&[], None);
        assert!(!out.contains("Current session"),
            "should not show live section: {}", out);
    }

    #[test]
    fn render_no_live_section_for_single_tab() {
        // Single-tab live state is filtered out (matches
        // strip hide-when-single behavior).
        let live = LiveSession {
            speaker_display: "Alice".into(),
            tabs: vec![("https://only.com".into(), 1700000000000)],
            active_tab: 0,
        };
        let out = render_rows_with_live(&[], Some(&live));
        assert!(!out.contains("Current session"),
            "single-tab live state should be hidden: {}", out);
    }

    #[test]
    fn render_live_section_for_two_or_more_tabs() {
        let live = LiveSession {
            speaker_display: "Alice".into(),
            tabs: vec![
                ("https://a.com".into(), 1700000000000),
                ("https://b.com".into(), 1700000000000),
            ],
            active_tab: 1,
        };
        let out = render_rows_with_live(&[], Some(&live));
        assert!(out.contains("Current session"));
        assert!(out.contains("Alice"));
        assert!(out.contains("a.com"));
        assert!(out.contains("b.com"));
        assert!(out.contains("2 browser tab(s) open"));
    }

    #[test]
    fn render_live_section_active_marker() {
        let live = LiveSession {
            speaker_display: "Alice".into(),
            tabs: vec![
                ("https://a.com".into(), 1700000000000),
                ("https://b.com".into(), 1700000000000),
            ],
            active_tab: 1,
        };
        let out = render_rows_with_live(&[], Some(&live));
        let b_pos = out.find("b.com").unwrap();
        let after_b = &out[b_pos..];
        assert!(after_b.contains("← active"),
            "active marker should appear after b.com: {}", after_b);
    }

    #[test]
    fn render_live_section_without_speaker_omits_dash() {
        // When no speaker identified yet, the heading
        // shouldn't show " — " with nothing after it.
        let live = LiveSession {
            speaker_display: "".into(),
            tabs: vec![
                ("https://a.com".into(), 1700000000000),
                ("https://b.com".into(), 1700000000000),
            ],
            active_tab: 0,
        };
        let out = render_rows_with_live(&[], Some(&live));
        assert!(out.contains("## Current session"));
        // Should NOT have a "— " followed by newline (empty
        // speaker after the dash).
        assert!(!out.contains("Current session — \n"),
            "shouldn't have stray dash with empty speaker: {}", out);
    }

    #[test]
    fn render_live_then_saved_workspaces() {
        // Live section + a persisted workspace should both
        // appear, live first.
        let live = LiveSession {
            speaker_display: "Alice".into(),
            tabs: vec![
                ("https://live-a.com".into(), 1700000000000),
                ("https://live-b.com".into(), 1700000000000),
            ],
            active_tab: 0,
        };
        let ws = make_ws("research",
            &["https://saved-x.com", "https://saved-y.com"], &[], 0);
        let rows = vec![
            ("alice".into(), "Alice".into(), ws),
        ];
        let out = render_rows_with_live(&rows, Some(&live));
        // Live first.
        let live_pos = out.find("Current session").unwrap();
        let saved_pos = out.find("Saved workspaces").unwrap();
        assert!(live_pos < saved_pos,
            "live section should precede saved: live={}, saved={}",
            live_pos, saved_pos);
        assert!(out.contains("live-a.com"));
        assert!(out.contains("saved-x.com"));
    }

    #[test]
    fn format_relative_ago_uses_provided_now() {
        // Same input timestamp, different "now" → different
        // labels. Confirms format_relative_ago uses the
        // passed-in now rather than re-reading the clock.
        let ts = 1_700_000_000_000u64;
        let now_close = ts + 30_000;
        let now_far = ts + 5 * 86_400_000;
        assert_eq!(format_relative_ago(ts, now_close), "just now");
        assert_eq!(format_relative_ago(ts, now_far), "5d ago");
    }
}
