// src/connections.rs
//
// v0.1.196 (ship 2.5.0 of the connections-overview arc) — node graph
// foundation.
//
// User asked for an Obsidian-inspired connections view: nodes for
// voice data, local files, web links, with previews and tags and
// fuzzy filtering and voice navigation. That's a multi-ship arc.
// This module is the spine: a unified Node abstraction over the
// existing data systems (page archive, voice corpus, chat history,
// workflows, speakers), so later UI ships have a stable shape to
// render against.
//
// What this ship DOES:
//   - Defines `Node`, `NodeKind`, `Edge`, `Tag` types
//   - Implements `enumerate_nodes()` — single function that scans
//     all existing data sources and yields nodes
//   - Implements basic edge derivation (page → speaker if a voice
//     recording came from that page; chat → page if a conversation
//     referenced a URL)
//   - Pure Rust + filesystem reads, no QML, no rendering
//
// What this ship does NOT:
//   - No UI surface (filed: 2.5.1)
//   - No fuzzy filter (filed: 2.5.2 — Claude has phonetic.rs for the
//     fuzzy substrate; once we know what we're filtering over,
//     wire it in)
//   - No tag persistence (filed: 2.5.3 — write tags to a sidecar
//     file once the UI lets users assign them)
//   - No voice command integration (filed: 2.5.4)
//   - No graph layout (filed: 2.5.5 — depends on UI surface)
//   - No spatial navigation (filed: 2.5.6)
//   - No edge weights or graph algorithms (filed: 2.5.7)
//
// Design notes:
//
//   - `Node` is a value type, not a reference into the source data.
//     Pulling a snapshot is the simplest way to keep the graph
//     decoupled from the underlying systems (which evolve at
//     different rates).
//   - Edges are derived, not stored. We re-derive on each scan.
//     Cheap enough at expected scale (hundreds of nodes, not
//     millions). When the scale grows, this is the right place
//     to introduce a cache.
//   - Tags are stored as a Vec<String> per Node. Tag-source is the
//     node's own metadata (the page's domain becomes an auto-tag,
//     for example). User-assigned tags will land in a sidecar
//     when ship 2.5.3 introduces them.

use std::path::PathBuf;

/// v0.1.432: cosine-similarity threshold for adding a
/// SimilarContent edge between two (cross-domain) pages. 0.30 is
/// deliberately conservative — the page "preview" is only ~120
/// chars, so even genuinely-related pages won't score very high;
/// too low a threshold would link everything. Tunable after a
/// hardware look at real edge density.
const SIMILAR_CONTENT_THRESHOLD: f32 = 0.30;

/// What kind of thing this node represents. Drives icon, preview
/// shape, and which edges can attach to it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    /// A page the user has visited and archived. Source:
    /// `archive::ArchivedPage`. Preview: title, domain, snippet.
    Page,
    /// A captured voice recording from the corpus pipeline.
    /// Source: `~/.config/web2qml/corpus/<speaker>/<ts>.{wav,txt}`.
    /// Preview: reference text, duration, speaker.
    VoiceRecording,
    /// A chat conversation with the LLM. Source: `chat::Conversation`.
    /// Preview: title or first user message, model name.
    ChatThread,
    /// A recorded workflow. Source: `~/.config/web2qml/workflows/*.toml`.
    /// Preview: name, command count, first command.
    Workflow,
    /// A known speaker (voice fingerprint identity).
    /// Source: `speakers::list_with_ids()`. Preview: name, color,
    /// cadence stats.
    Speaker,
    /// A local file the user has referenced (e.g. via terminal).
    /// Source: stub for now; will scan recent terminal output for
    /// file paths in ship 2.5.0+ once we wire term_blocks in.
    LocalFile,
}

impl NodeKind {
    /// Single-letter prefix for the node ID. Keeps IDs short and
    /// debuggable (e.g. "p_abc123" is a Page node).
    pub fn id_prefix(&self) -> &'static str {
        match self {
            NodeKind::Page => "p",
            NodeKind::VoiceRecording => "v",
            NodeKind::ChatThread => "c",
            NodeKind::Workflow => "w",
            NodeKind::Speaker => "s",
            NodeKind::LocalFile => "f",
        }
    }

    /// Human-readable label for the kind. UI uses this in the
    /// filter dropdown and the node preview.
    pub fn label(&self) -> &'static str {
        match self {
            NodeKind::Page => "page",
            NodeKind::VoiceRecording => "voice",
            NodeKind::ChatThread => "chat",
            NodeKind::Workflow => "workflow",
            NodeKind::Speaker => "speaker",
            NodeKind::LocalFile => "file",
        }
    }
}

/// A single node in the connections graph. Holds enough to render
/// a preview without going back to the source data.
#[derive(Clone, Debug)]
pub struct Node {
    /// Stable ID: `<kind-prefix>_<hash>`. Stable across re-scans
    /// for the same underlying entity.
    pub id: String,
    pub kind: NodeKind,
    /// Primary display name. Page title, voice paragraph snippet,
    /// chat first-message, workflow name, speaker name, file name.
    pub title: String,
    /// Secondary string for context. URL, speaker name (for voice),
    /// model name (for chat), command count (for workflow), file
    /// path (for file). Empty string if not applicable.
    pub subtitle: String,
    /// Up to ~120 chars of content preview. For pages: text_snippet
    /// from the archive. For voice: the reference paragraph text.
    /// For chat: first user message or assistant reply preview.
    pub preview: String,
    /// Optional path to a thumbnail image. None for nodes without
    /// thumbnails. (Future: web page screenshots, voice waveforms.)
    pub thumbnail: Option<PathBuf>,
    /// Auto-derived tags from the node's own metadata. Examples:
    /// domain for pages, "@<speaker>" for voice recordings,
    /// model name for chats. User-assigned tags will merge in
    /// from a sidecar in ship 2.5.3.
    pub tags: Vec<String>,
    /// When the underlying entity was created/captured. Unix
    /// seconds. Used for "recently visited" sorting and date
    /// filtering.
    pub created_at: u64,
}

/// An undirected connection between two nodes. Derived from
/// metadata overlap, not stored.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Edge {
    pub from: String,
    pub to: String,
    /// Why these two are connected. Drives edge styling and the
    /// "what links this to that?" hover text.
    pub reason: EdgeReason,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EdgeReason {
    /// Voice recording captured while reading this page.
    /// (Currently the corpus pipeline doesn't tag recordings with
    /// the source URL; ship 2.5.1 will add that metadata.)
    VoiceFromPage,
    /// Voice recording attributed to this speaker.
    VoiceBySpeaker,
    /// Chat conversation referenced this URL in user input or
    /// assistant output.
    ChatReferencesPage,
    /// Pages share the same domain — weak but useful.
    SameDomain,
    /// Nodes share at least one tag.
    SharedTag(String),
    /// v0.1.432: two pages whose content vectors are cosine-
    /// similar above a threshold — "about the same thing", even
    /// across different domains. Derived via the hashing
    /// vectorizer (nlp::SparseVec), not metadata. The u32 carries
    /// the similarity ×1000 (milli-units) so the enum keeps its
    /// Eq/Hash derive (f32 isn't Eq/Hash); convert back via
    /// `similarity_score()`.
    SimilarContent(u32),
}

impl EdgeReason {
    /// v0.1.325 (cards arc 2.5.7): weight contribution this
    /// reason adds when computing relatedness or visual edge
    /// thickness. Higher = stronger signal. Calibrated so that
    /// a user-curated SharedTag matches the strength of a
    /// VoiceBySpeaker — both are explicit "this is connected"
    /// signals, while SameDomain (weakest) is incidental.
    pub fn weight(&self) -> u32 {
        match self {
            EdgeReason::VoiceFromPage => 4,
            EdgeReason::VoiceBySpeaker => 3,
            EdgeReason::SharedTag(_) => 3,
            EdgeReason::ChatReferencesPage => 3,
            EdgeReason::SameDomain => 1,
            // v0.1.432: content similarity is a meaningful signal
            // (cross-domain "about the same thing") — between
            // SameDomain and the explicit signals.
            EdgeReason::SimilarContent(_) => 2,
        }
    }

    /// v0.1.432: similarity score in 0.0..=1.0 for SimilarContent
    /// (0 for other reasons). Inverse of the ×1000 quantization.
    pub fn similarity_score(&self) -> f32 {
        match self {
            EdgeReason::SimilarContent(milli) => *milli as f32 / 1000.0,
            _ => 0.0,
        }
    }
}

/// Snapshot of the connection graph at one moment in time.
#[derive(Clone, Debug, Default)]
pub struct GraphSnapshot {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl GraphSnapshot {
    pub fn n_nodes(&self) -> usize { self.nodes.len() }
    pub fn n_edges(&self) -> usize { self.edges.len() }

    /// Lookup a node by ID. Linear scan; fine at expected scale.
    pub fn find_node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// All nodes whose tag list contains `tag` (case-insensitive
    /// match).
    pub fn nodes_with_tag(&self, tag: &str) -> Vec<&Node> {
        let needle = tag.to_lowercase();
        self.nodes.iter().filter(|n| {
            n.tags.iter().any(|t| t.to_lowercase() == needle)
        }).collect()
    }

    /// All edges touching the given node (either direction —
    /// edges are undirected logically even if stored as from/to).
    pub fn edges_for_node(&self, id: &str) -> Vec<&Edge> {
        self.edges.iter().filter(|e| e.from == id || e.to == id).collect()
    }
}

// ── v0.1.779: per-user "hidden from tree" set ──
// "Delete from tree (not disk)" hides a node from the connections graph WITHOUT
// touching the underlying file/page/archive entry. Persisted as newline-
// delimited node ids in ~/.config/web2qml/hidden_nodes.txt. Because the source
// is untouched, a re-scan still finds the entity — we just filter it back out.
fn hidden_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home).join(".config/web2qml/hidden_nodes.txt")
}

fn hidden_set() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static SET: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    SET.get_or_init(|| {
        let mut s = std::collections::HashSet::new();
        if let Ok(text) = std::fs::read_to_string(hidden_path()) {
            for line in text.lines() {
                let id = line.trim();
                if !id.is_empty() { s.insert(id.to_string()); }
            }
        }
        std::sync::Mutex::new(s)
    })
}

/// Hide a node id from the graph (persisted). Does NOT delete any file.
pub fn hide_node(id: &str) {
    let id = id.trim();
    if id.is_empty() { return; }
    if let Ok(mut set) = hidden_set().lock() {
        if set.insert(id.to_string()) {
            let path = hidden_path();
            if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(f, "{}", id);
            }
        }
    }
}

/// Remove hidden nodes (and any edges touching them) from a snapshot.
/// Call AFTER all nodes are attached (archive/chat), before rendering.
pub fn filter_hidden(snap: &mut GraphSnapshot) {
    if let Ok(set) = hidden_set().lock() {
        if set.is_empty() { return; }
        snap.nodes.retain(|n| !set.contains(&n.id));
        snap.edges.retain(|e| !set.contains(&e.from) && !set.contains(&e.to));
    }
}

/// Scan all known data sources and produce a fresh graph snapshot.
/// This is the single entry point UI code should call to get a
/// current view.
///
/// Cost: one filesystem walk per source (corpus directory,
/// workflows directory) plus in-memory traversals of archive,
/// chat, speakers. At expected scale (hundreds of nodes), well
/// under 100ms.
///
/// Sources currently scanned:
///   - Page archive (`Archive::list_all()` / `search` substrate)
///   - Voice corpus (`~/.config/web2qml/corpus/`)
///   - Chat conversations (via `ChatClient::list_conversations`)
///   - Workflows (`workflow::list_workflows()`)
///   - Speakers (`speakers::list_with_ids()`)
///
/// Sources NOT yet scanned (filed):
///   - LocalFile nodes from terminal history (2.5.0+ once the
///     term_blocks integration lands here)
///
/// Note: this function is intentionally permissive. If one source
/// fails to scan (corrupt cache, missing directory), it logs and
/// continues with the others. The graph degrades gracefully.
pub fn enumerate_nodes() -> GraphSnapshot {
    let mut snap = GraphSnapshot::default();

    if let Err(e) = scan_voice_corpus(&mut snap) {
        crate::clog!("connections", "voice corpus scan failed: {}", e);
    }
    if let Err(e) = scan_workflows(&mut snap) {
        crate::clog!("connections", "workflow scan failed: {}", e);
    }
    if let Err(e) = scan_speakers(&mut snap) {
        crate::clog!("connections", "speakers scan failed: {}", e);
    }

    // Archive scan + chat scan need handles to existing
    // structures that aren't global. Those callers will inject
    // nodes via `attach_archive` / `attach_chat` before rendering.
    // This keeps the module from depending on the runtime
    // graph of Backend fields.

    derive_edges(&mut snap);
    snap
}

fn scan_voice_corpus(snap: &mut GraphSnapshot) -> std::io::Result<()> {
    let home = std::env::var("HOME").map_err(|_| std::io::Error::new(
        std::io::ErrorKind::Other, "HOME unset",
    ))?;
    let dir = PathBuf::from(home).join(".config/web2qml/corpus");
    if !dir.exists() {
        return Ok(()); // no recordings yet; not an error
    }
    // Two levels deep: <corpus>/<speaker_id>/<unix_ms>.{wav,txt}
    for speaker_entry in std::fs::read_dir(&dir)? {
        let speaker_entry = match speaker_entry { Ok(e) => e, Err(_) => continue };
        let speaker_path = speaker_entry.path();
        if !speaker_path.is_dir() { continue; }
        let speaker_id = speaker_path.file_name()
            .and_then(|n| n.to_str()).unwrap_or("unknown").to_string();

        for rec_entry in std::fs::read_dir(&speaker_path)? {
            let rec_entry = match rec_entry { Ok(e) => e, Err(_) => continue };
            let rec_path = rec_entry.path();
            if rec_path.extension().and_then(|s| s.to_str()) != Some("wav") {
                continue;
            }
            // Look up the matching .txt for the reference text.
            let txt_path = rec_path.with_extension("txt");
            let txt_content = std::fs::read_to_string(&txt_path)
                .unwrap_or_default();
            // v0.1.325 (cards arc 2.5.1): if the txt has the
            // URL/TITLE/--- header, split it out. Pre-2.5.1
            // files have no header and parse as all-body.
            let parsed = parse_voice_txt(&txt_content);
            let preview = parsed.body.chars().take(120).collect::<String>();

            let stem = rec_path.file_stem()
                .and_then(|s| s.to_str()).unwrap_or("").to_string();
            // The stem IS a unix-ms timestamp. Convert to seconds.
            let ts = stem.parse::<u64>().unwrap_or(0) / 1000;

            let id = format!("{}_{}_{}", NodeKind::VoiceRecording.id_prefix(),
                speaker_id, stem);
            let title = if !parsed.title.is_empty() {
                // Page title is the most useful display label
                // when available.
                if parsed.title.chars().count() > 32 {
                    let s: String = parsed.title.chars().take(30).collect();
                    format!("{}…", s)
                } else {
                    parsed.title.clone()
                }
            } else if preview.chars().count() > 32 {
                let s: String = preview.chars().take(30).collect();
                format!("{}…", s)
            } else if preview.is_empty() {
                "(no text)".to_string()
            } else {
                preview.clone()
            };
            // v0.1.325: subtitle now includes the source URL
            // when known (more informative than just speaker).
            let subtitle = if !parsed.url.is_empty() {
                parsed.url.clone()
            } else {
                format!("@{}", speaker_id)
            };
            // Tags: speaker + "voice" always. If we have a
            // URL, add a `from_url:<url>` tag — `derive_edges`
            // turns this into a VoiceFromPage edge.
            let mut tags = vec![format!("@{}", speaker_id), "voice".to_string()];
            if !parsed.url.is_empty() {
                tags.push(format!("from_url:{}", parsed.url));
            }
            snap.nodes.push(Node {
                id,
                kind: NodeKind::VoiceRecording,
                title,
                subtitle,
                preview,
                thumbnail: None,
                tags,
                created_at: ts,
            });
        }
    }
    Ok(())
}

/// v0.1.325 (cards arc 2.5.1): parsed pieces of a corpus .txt
/// sidecar. Pre-2.5.1 files have empty url/title — only body.
#[derive(Default, Debug)]
struct VoiceTxt {
    url: String,
    title: String,
    /// v0.1.510: active voice-grammar label when recorded (e.g.
    /// "editor:helix", "repl:gdb", "shell", "web"). Empty if the
    /// recording predates grammar stamping or had no header.
    grammar: String,
    body: String,
}

/// Parse the optional header from a corpus .txt sidecar.
/// Format (when present):
///
///     URL: <url>
///     TITLE: <title>
///     ---
///     <body>
///
/// If the first line isn't `URL: `, the whole content is body.
fn parse_voice_txt(content: &str) -> VoiceTxt {
    let mut out = VoiceTxt::default();
    let mut lines = content.lines();
    let first = match lines.next() {
        Some(l) => l,
        None => return out,
    };
    // A header exists only if the FIRST line is a known header key.
    // (URL was the original first-line sentinel; GRAMMAR may now lead
    // too, since recordings without a page source still carry a
    // grammar label.) If the first line isn't a known key, the whole
    // content is body — preserving the pre-header fallback.
    fn header_kv(line: &str) -> Option<(&'static str, &str)> {
        for key in ["URL: ", "TITLE: ", "GRAMMAR: "] {
            if let Some(v) = line.strip_prefix(key) {
                return Some((key, v.trim()));
            }
        }
        None
    }
    let first_kv = match header_kv(first) {
        Some(kv) => kv,
        None => {
            // No header — whole content is body (pre-2.5.1 files,
            // and any file whose first line isn't a header key).
            out.body = content.to_string();
            return out;
        }
    };
    // v0.1.510: key-ORDER-TOLERANT header scan. Previously the parser
    // assumed fixed positions (line1=URL, line2=TITLE, line3=---), so
    // inserting a new field would have mis-aligned old files. Now we
    // collect known KEY: lines in ANY order until the `---` marker,
    // then everything after is body. Adding a future field is just one
    // more entry in header_kv. Old URL/TITLE/--- files still parse;
    // so do new files carrying GRAMMAR.
    let apply = |out: &mut VoiceTxt, (key, val): (&str, &str)| {
        match key {
            "URL: " => out.url = val.to_string(),
            "TITLE: " => out.title = val.to_string(),
            "GRAMMAR: " => out.grammar = val.to_string(),
            _ => {}
        }
    };
    apply(&mut out, first_kv);
    let mut body_started = false;
    let mut body = String::new();
    for line in lines {
        if !body_started {
            if line.trim() == "---" {
                body_started = true;
                continue;
            }
            if let Some(kv) = header_kv(line) {
                apply(&mut out, kv);
                continue;
            }
            // A non-key, non-marker line inside the header region means
            // a malformed/legacy header without a `---`: treat from
            // here on as body (defensive, matches the old behaviour of
            // folding an unexpected line into the body).
            body_started = true;
            body.push_str(line);
            body.push('\n');
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    while body.ends_with('\n') { body.pop(); }
    out.body = body;
    out
}

fn scan_workflows(snap: &mut GraphSnapshot) -> std::io::Result<()> {
    let home = std::env::var("HOME").map_err(|_| std::io::Error::new(
        std::io::ErrorKind::Other, "HOME unset",
    ))?;
    let dir = PathBuf::from(home).join(".config/web2qml/workflows");
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&dir)? {
        let entry = match entry { Ok(e) => e, Err(_) => continue };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let name = path.file_stem().and_then(|s| s.to_str())
            .unwrap_or("").to_string();
        if name.is_empty() { continue; }

        // Read the TOML to extract command count + first command.
        // The format is documented in workflow.rs: simple keys.
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let (n_cmds, first_cmd, created_at) = parse_workflow_summary(&content);
        let id = format!("{}_{}", NodeKind::Workflow.id_prefix(), name);
        snap.nodes.push(Node {
            id,
            kind: NodeKind::Workflow,
            title: name.clone(),
            subtitle: format!("{} command{}", n_cmds, if n_cmds == 1 { "" } else { "s" }),
            preview: first_cmd,
            thumbnail: None,
            tags: vec!["workflow".to_string()],
            created_at,
        });
    }
    Ok(())
}

/// Pull the bits we need from a workflow TOML without taking a
/// real toml-parse dep. The format is line-oriented in practice;
/// this is robust enough for our own files.
fn parse_workflow_summary(content: &str) -> (usize, String, u64) {
    let mut created_at = 0_u64;
    let mut first_cmd = String::new();
    let mut n_cmds = 0_usize;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("created_at") {
            // `created_at = 1747929600`
            let rest = rest.trim_start_matches([' ', '=']);
            if let Ok(n) = rest.trim().parse::<u64>() {
                created_at = n;
            }
        } else if line.starts_with("commands") && line.contains('[') {
            // `commands = ["a", "b", "c"]` — count the quoted strings.
            // Robust enough for our own TOML output.
            let cmds: Vec<&str> = line.split('"').collect();
            // Quoted runs are at odd indices when split by ".
            for (i, s) in cmds.iter().enumerate() {
                if i % 2 == 1 && !s.is_empty() {
                    if first_cmd.is_empty() {
                        first_cmd = s.to_string();
                    }
                    n_cmds += 1;
                }
            }
        }
    }
    (n_cmds, first_cmd, created_at)
}

fn scan_speakers(snap: &mut GraphSnapshot) -> std::io::Result<()> {
    // speakers::list_with_ids() returns Vec<(id, name, color)>.
    // We call it directly; it reads the in-memory speaker list
    // which was loaded at startup. If speakers haven't loaded
    // yet, returns empty — not an error.
    let list = crate::speakers::list_with_ids();
    for (id, name, color) in list {
        snap.nodes.push(Node {
            id: format!("{}_{}", NodeKind::Speaker.id_prefix(), id),
            kind: NodeKind::Speaker,
            title: name.clone(),
            subtitle: format!("color {}", color),
            preview: String::new(),
            thumbnail: None,
            tags: vec!["speaker".to_string(), format!("@{}", name)],
            created_at: 0, // we don't track speaker creation time
        });
    }
    Ok(())
}

/// Walk the archive and add Page nodes. Caller passes the archive
/// since it's a Backend field, not a global. Designed to be
/// chained after `enumerate_nodes()`:
///
/// ```ignore
/// let mut snap = enumerate_nodes();
/// attach_archive(&mut snap, &backend.archive);
/// attach_chat(&mut snap, &backend.chat);
/// ```
pub fn attach_archive(snap: &mut GraphSnapshot, archive: &crate::archive::Archive) {
    for page in &archive.entries {
        let id = format!("{}_{}", NodeKind::Page.id_prefix(),
            short_hash(&page.url));
        snap.nodes.push(Node {
            id,
            kind: NodeKind::Page,
            title: if page.title.is_empty() { page.url.clone() } else { page.title.clone() },
            subtitle: page.domain.clone(),
            preview: page.text_snippet.chars().take(120).collect(),
            thumbnail: None,
            tags: vec![page.domain.clone(), "page".to_string()],
            created_at: parse_iso8601_to_unix(&page.archived_at),
        });
    }
    // Re-derive edges after the archive is attached. Cheap enough.
    derive_edges(snap);
}

/// Attach chat conversation nodes. Same chaining pattern as
/// `attach_archive`.
pub fn attach_chat(snap: &mut GraphSnapshot, chat: &crate::chat::ChatClient) {
    for conv in &chat.conversations {
        let id = format!("{}_{}", NodeKind::ChatThread.id_prefix(),
            short_hash(&conv.id));
        let title = if conv.title.is_empty() {
            // First user message as title.
            conv.messages.iter()
                .find(|m| m.role == "user")
                .map(|m| m.content.chars().take(40).collect::<String>())
                .unwrap_or_else(|| "(empty)".to_string())
        } else {
            conv.title.clone()
        };
        let preview = conv.messages.first()
            .map(|m| m.content.chars().take(120).collect::<String>())
            .unwrap_or_default();
        let n_msg = conv.messages.len();
        snap.nodes.push(Node {
            id,
            kind: NodeKind::ChatThread,
            title,
            subtitle: format!("{} message{}", n_msg, if n_msg == 1 { "" } else { "s" }),
            preview,
            thumbnail: None,
            tags: vec!["chat".to_string(), conv.provider.clone()],
            created_at: parse_iso8601_to_unix(&conv.created_at),
        });
    }
    derive_edges(snap);
}

/// v0.1.324 (cards arc 2.5.3): merge user-assigned tags into
/// every node's tag list. Auto-derived tags stay; user tags
/// append (de-duped against auto). Re-runs `derive_edges` so
/// the new tags participate in SharedTag edges.
///
/// Call this AFTER all node sources are aggregated
/// (`enumerate_nodes` + any `attach_*`) — typically the last
/// step before serialization.
pub fn merge_user_tags(snap: &mut GraphSnapshot) {
    for node in snap.nodes.iter_mut() {
        let user_tags = crate::user_tags::get(&node.id);
        for ut in user_tags {
            if !node.tags.iter().any(|t| t == &ut) {
                node.tags.push(ut);
            }
        }
    }
    derive_edges(snap);
    // v0.1.781: drop nodes the user removed from the tree (not disk), plus any
    // edges touching them, AFTER all nodes/edges are in place. Every snapshot
    // path runs merge_user_tags as its last enrichment step, so this is the
    // single chokepoint that hides a node from EVERY view — canvas, fuzzy
    // search, tag filter, mermaid export, session walk, and related/similar —
    // not just the main cards_layout.
    filter_hidden(snap);
}

/// v0.1.325 (cards arc 2.5.7): return the top-N nodes most
/// strongly connected to `id`, sorted by total edge weight
/// descending. Used by the preview overlay's "Related to"
/// list.
///
/// Edges are undirected — if `id` appears as either endpoint,
/// the OTHER endpoint counts toward the score. Multiple edges
/// between the same pair sum (common when two pages share
/// both a domain and a tag).
///
/// Returns Vec of (other_id, weight_sum). Empty if id isn't
/// in the snapshot or has no edges.
pub fn related_to(snap: &GraphSnapshot, id: &str, n: usize) -> Vec<(String, u32)> {
    if n == 0 || id.is_empty() { return Vec::new(); }
    let mut scores: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    for edge in &snap.edges {
        let other = if edge.from == id {
            &edge.to
        } else if edge.to == id {
            &edge.from
        } else {
            continue;
        };
        *scores.entry(other.clone()).or_insert(0) += edge.reason.weight();
    }
    let mut sorted: Vec<(String, u32)> = scores.into_iter().collect();
    // Sort by score desc, ties broken by id (stable across runs).
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    sorted.truncate(n);
    sorted
}

/// v0.1.325: total edge weight between two nodes (undirected,
/// summed across all parallel edges). Used for visual line
/// thickness in the cards canvas.
pub fn pair_weight(snap: &GraphSnapshot, a: &str, b: &str) -> u32 {
    snap.edges.iter()
        .filter(|e| (e.from == a && e.to == b) || (e.from == b && e.to == a))
        .map(|e| e.reason.weight())
        .sum()
}

/// Derive edges from node metadata. Re-run whenever the node set
/// changes.
fn derive_edges(snap: &mut GraphSnapshot) {
    snap.edges.clear();
    let nodes = snap.nodes.clone();

    // VoiceBySpeaker: voice nodes with `@<speaker>` tags link to
    // the corresponding Speaker node.
    for v in nodes.iter().filter(|n| n.kind == NodeKind::VoiceRecording) {
        for tag in &v.tags {
            if let Some(name) = tag.strip_prefix('@') {
                // Match by speaker id (the @ tag is the speaker_id
                // string, since that's what corpus_recorder uses).
                for s in nodes.iter().filter(|n| n.kind == NodeKind::Speaker) {
                    // Speaker tags include "@<name>"; we want both
                    // id-match and name-match to succeed.
                    let id_part = s.id.strip_prefix("s_").unwrap_or(&s.id);
                    if id_part == name || s.title == *name {
                        snap.edges.push(Edge {
                            from: v.id.clone(),
                            to: s.id.clone(),
                            reason: EdgeReason::VoiceBySpeaker,
                        });
                    }
                }
            }
        }
    }

    // SameDomain: pages with the same domain link to each other.
    // O(n²) over pages but n is small; refactor if pages > 500.
    let pages: Vec<&Node> = nodes.iter().filter(|n| n.kind == NodeKind::Page).collect();
    for i in 0..pages.len() {
        for j in (i+1)..pages.len() {
            if !pages[i].subtitle.is_empty() && pages[i].subtitle == pages[j].subtitle {
                snap.edges.push(Edge {
                    from: pages[i].id.clone(),
                    to: pages[j].id.clone(),
                    reason: EdgeReason::SameDomain,
                });
            }
        }
    }

    // SharedTag: any two nodes sharing a non-trivial tag.
    // "trivial" = the kind label itself (every page has "page",
    // not interesting as an edge).
    let trivial_tags: std::collections::HashSet<&str> =
        ["page", "voice", "chat", "workflow", "speaker", "file"].into_iter().collect();
    for i in 0..nodes.len() {
        for j in (i+1)..nodes.len() {
            for tag in &nodes[i].tags {
                if trivial_tags.contains(tag.as_str()) { continue; }
                if nodes[j].tags.contains(tag) {
                    snap.edges.push(Edge {
                        from: nodes[i].id.clone(),
                        to: nodes[j].id.clone(),
                        reason: EdgeReason::SharedTag(tag.clone()),
                    });
                    break; // one edge per pair is enough
                }
            }
        }
    }

    // v0.1.432: SimilarContent — pages whose content vectors are
    // cosine-similar above SIMILAR_CONTENT_THRESHOLD. Uses the
    // hashing vectorizer over title + preview. Only CROSS-domain
    // pairs (same-domain pages already get a SameDomain edge; the
    // new signal is "different sites, same topic"). Vectors are
    // precomputed once per page (O(n) build, O(n²) compare; n is
    // small — pages are capped).
    {
        let page_vecs: Vec<(usize, crate::nlp::SparseVec, f32)> = pages.iter().enumerate()
            .map(|(idx, p)| {
                let text = format!("{} {}", p.title, p.preview);
                let v = crate::nlp::SparseVec::from_text(&text);
                let norm = v.norm();
                (idx, v, norm)
            })
            .collect();
        for a in 0..page_vecs.len() {
            for b in (a + 1)..page_vecs.len() {
                let (pi, va, na) = &page_vecs[a];
                let (pj, vb, nb) = &page_vecs[b];
                // Skip same-domain (already linked by SameDomain).
                if !pages[*pi].subtitle.is_empty()
                    && pages[*pi].subtitle == pages[*pj].subtitle {
                    continue;
                }
                let sim = va.cosine_with_norms(*na, vb, *nb);
                if sim >= SIMILAR_CONTENT_THRESHOLD {
                    snap.edges.push(Edge {
                        from: pages[*pi].id.clone(),
                        to: pages[*pj].id.clone(),
                        reason: EdgeReason::SimilarContent((sim * 1000.0) as u32),
                    });
                }
            }
        }
    }
}

/// Short content-based ID. Not cryptographically meaningful; just
/// enough to keep node IDs stable across re-scans for the same
/// entity, and unique enough at expected scale.
fn short_hash(s: &str) -> String {
    // v0.1.454: delegate to the shared FNV-1a util (was an
    // inline copy of the same hash + low-32-bit hex format).
    crate::util::short_hash(s)
}

/// Best-effort parse of ISO 8601 to unix seconds. Returns 0 on
/// any error — date sorting will treat unknown dates as ancient,
/// which is the right default ("show recent stuff first").
fn parse_iso8601_to_unix(s: &str) -> u64 {
    // Accept formats like "2026-05-14T12:30:00Z" or
    // "2026-05-14T12:30:00+00:00". We parse only the date+time
    // and ignore timezone for sorting purposes (close enough).
    //
    // Inner helper returns Option so the `?` propagation works;
    // the outer wrapper converts None → 0 for the public API.
    fn inner(s: &str) -> Option<u64> {
        let s = s.trim();
        if s.len() < 19 { return None; }
        let date_part = &s[..10];     // YYYY-MM-DD
        let time_part = &s[11..19];   // HH:MM:SS

        let parse_n = |sub: &str| -> Option<u64> { sub.parse().ok() };
        let y = parse_n(&date_part[..4])?;
        let mo = parse_n(&date_part[5..7])?;
        let d = parse_n(&date_part[8..10])?;
        let h = parse_n(&time_part[..2])?;
        let mi = parse_n(&time_part[3..5])?;
        let se = parse_n(&time_part[6..8])?;

        let days = days_from_civil(y as i64, mo as u32, d as u32);
        if days < 0 { return Some(0); } // pre-epoch — sort as 0
        Some(days as u64 * 86400 + h * 3600 + mi * 60 + se)
    }
    inner(s).unwrap_or(0)
}

/// Howard Hinnant's days_from_civil. Returns days since 1970-01-01
/// (negative for earlier dates).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = m as u64;
    let d = d as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}

/// v0.1.323 (cards arc 2.5.2): score-and-rank fuzzy filter over
/// the snapshot. Returns indices into `snap.nodes` sorted by
/// match score descending. Empty query → all indices (in source
/// order). Zero-score nodes are filtered out for non-empty
/// queries.
///
/// Scoring (per node):
///   +100 if the WHOLE query (lowercased) is a substring of the
///        node title (case-insensitive). Strongest signal.
///   +50  if the WHOLE query is a substring of the combined
///        haystack (title + subtitle + preview + tags). Catches
///        matches where the query word lives in URL or preview.
///   +20 per query TOKEN that's a prefix of a word in the
///        haystack (case-insensitive token boundary by whitespace).
///   +5  per query TOKEN that's a contained substring of the
///        haystack but doesn't satisfy the prefix rule above.
///
/// Tokens are space-separated; punctuation other than space is
/// treated as part of the token (so "rust.lang" stays one token).
///
/// `cap` is the maximum number of indices to return; pass usize::MAX
/// for no cap.
pub fn fuzzy_filter(snap: &GraphSnapshot, query: &str, cap: usize) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        let limit = cap.min(snap.nodes.len());
        return (0..limit).collect();
    }
    let tokens: Vec<&str> = q.split_whitespace().filter(|t| !t.is_empty()).collect();

    // Precompute haystacks once per node.
    let mut scored: Vec<(usize, i32)> = Vec::with_capacity(snap.nodes.len());
    for (i, n) in snap.nodes.iter().enumerate() {
        let title_lc = n.title.to_lowercase();
        let mut hay = String::with_capacity(
            n.title.len() + n.subtitle.len() + n.preview.len() + 32
        );
        hay.push_str(&title_lc);
        hay.push(' ');
        hay.push_str(&n.subtitle.to_lowercase());
        hay.push(' ');
        hay.push_str(&n.preview.to_lowercase());
        for t in &n.tags {
            hay.push(' ');
            hay.push_str(&t.to_lowercase());
        }

        let mut score: i32 = 0;
        if title_lc.contains(&q) {
            score += 100;
        }
        if hay.contains(&q) {
            score += 50;
        }
        // Token-level matches.
        for tok in &tokens {
            // Skip very short tokens (would match too broadly).
            if tok.len() < 2 { continue; }
            // Prefix-of-word check: scan haystack by whitespace
            // boundaries.
            let mut prefix_hit = false;
            for word in hay.split_whitespace() {
                if word.starts_with(tok) {
                    prefix_hit = true;
                    break;
                }
            }
            if prefix_hit {
                score += 20;
            } else if hay.contains(tok) {
                score += 5;
            }
        }
        if score > 0 {
            scored.push((i, score));
        }
    }
    // Sort by score descending, ties broken by node index (stable
    // ordering — earlier-appearing nodes win ties).
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.truncate(cap);
    scored.into_iter().map(|(i, _)| i).collect()
}

/// v0.1.677: convert literal escape sequences (`\n`, `\t`, `\r` — the
/// two-character backslash forms, NOT real control chars) in a card
/// title/preview to spaces, then collapse whitespace. Mindmap/script
/// content often carries literal `\n` inside node labels (e.g. a choice
/// node "He holds it himself\nUnicron is destroyed"); without this the
/// graph cards render the backslash-n verbatim (the image-1 bug). Real
/// control chars are handled separately by the emit (it replaces '\n').
pub fn unescape_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('n') | Some('t') | Some('r') => { chars.next(); out.push(' '); }
                Some('\\') => { chars.next(); out.push('\\'); }
                _ => out.push('\\'),
            }
        } else if c == '\n' || c == '\t' || c == '\r' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    // collapse whitespace runs
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_inline_handles_literal_backslash_n() {
        // the image-1 card bug: literal backslash-n shown verbatim
        assert_eq!(unescape_inline("He holds it himself\\nUnicron is destroyed"),
                   "He holds it himself Unicron is destroyed");
        // real control char too
        assert_eq!(unescape_inline("a\nb"), "a b");
        // \t and \r unescape to spaces (these are prose labels, not paths)
        assert_eq!(unescape_inline("a\\tb\\rc"), "a b c");
        // a real backslash before a non-escape char is preserved
        assert_eq!(unescape_inline("foo\\xbar"), "foo\\xbar");
        // escaped backslash collapses to one
        assert_eq!(unescape_inline("a\\\\b"), "a\\b");
        // whitespace runs collapse
        assert_eq!(unescape_inline("a\\n\\nb"), "a b");
    }

    fn empty_graph() {
        let snap = GraphSnapshot::default();
        assert_eq!(snap.n_nodes(), 0);
        assert_eq!(snap.n_edges(), 0);
    }

    #[test]
    fn nodekind_prefixes_are_unique() {
        let kinds = [
            NodeKind::Page,
            NodeKind::VoiceRecording,
            NodeKind::ChatThread,
            NodeKind::Workflow,
            NodeKind::Speaker,
            NodeKind::LocalFile,
        ];
        let prefixes: std::collections::HashSet<&str> = kinds.iter()
            .map(|k| k.id_prefix()).collect();
        assert_eq!(prefixes.len(), kinds.len(),
            "node kind id prefixes must be unique");
    }

    #[test]
    fn find_node_by_id() {
        let snap = GraphSnapshot {
            nodes: vec![Node {
                id: "p_abc".into(),
                kind: NodeKind::Page,
                title: "Example".into(),
                subtitle: "example.com".into(),
                preview: "".into(),
                thumbnail: None,
                tags: vec!["example.com".into(), "page".into()],
                created_at: 0,
            }],
            edges: vec![],
        };
        assert!(snap.find_node("p_abc").is_some());
        assert!(snap.find_node("missing").is_none());
    }

    #[test]
    fn nodes_with_tag_case_insensitive() {
        let snap = GraphSnapshot {
            nodes: vec![
                Node {
                    id: "p_1".into(), kind: NodeKind::Page,
                    title: "A".into(), subtitle: "".into(), preview: "".into(),
                    thumbnail: None, tags: vec!["Rust".into()], created_at: 0,
                },
                Node {
                    id: "p_2".into(), kind: NodeKind::Page,
                    title: "B".into(), subtitle: "".into(), preview: "".into(),
                    thumbnail: None, tags: vec!["rust".into()], created_at: 0,
                },
            ],
            edges: vec![],
        };
        assert_eq!(snap.nodes_with_tag("rust").len(), 2);
        assert_eq!(snap.nodes_with_tag("RUST").len(), 2);
    }

    #[test]
    fn edges_for_node_matches_both_directions() {
        let snap = GraphSnapshot {
            nodes: vec![],
            edges: vec![
                Edge { from: "a".into(), to: "b".into(), reason: EdgeReason::SameDomain },
                Edge { from: "c".into(), to: "a".into(), reason: EdgeReason::SameDomain },
                Edge { from: "b".into(), to: "c".into(), reason: EdgeReason::SameDomain },
            ],
        };
        let for_a = snap.edges_for_node("a");
        assert_eq!(for_a.len(), 2);
    }

    #[test]
    fn derive_edges_same_domain() {
        let mut snap = GraphSnapshot {
            nodes: vec![
                Node {
                    id: "p_1".into(), kind: NodeKind::Page,
                    title: "A".into(), subtitle: "example.com".into(),
                    preview: "".into(), thumbnail: None,
                    tags: vec!["example.com".into(), "page".into()],
                    created_at: 0,
                },
                Node {
                    id: "p_2".into(), kind: NodeKind::Page,
                    title: "B".into(), subtitle: "example.com".into(),
                    preview: "".into(), thumbnail: None,
                    tags: vec!["example.com".into(), "page".into()],
                    created_at: 0,
                },
                Node {
                    id: "p_3".into(), kind: NodeKind::Page,
                    title: "C".into(), subtitle: "other.com".into(),
                    preview: "".into(), thumbnail: None,
                    tags: vec!["other.com".into(), "page".into()],
                    created_at: 0,
                },
            ],
            edges: vec![],
        };
        derive_edges(&mut snap);
        let same_dom: Vec<_> = snap.edges.iter()
            .filter(|e| matches!(e.reason, EdgeReason::SameDomain)).collect();
        // Only p_1 ↔ p_2 share a domain.
        assert_eq!(same_dom.len(), 1);
    }

    // v0.1.432: cross-domain content similarity edges.
    #[test]
    fn derive_edges_similar_content_cross_domain() {
        let mut snap = GraphSnapshot {
            nodes: vec![
                Node {
                    id: "p_1".into(), kind: NodeKind::Page,
                    title: "Rust memory safety guide".into(),
                    subtitle: "siteA.com".into(),
                    preview: "Rust guarantees memory safety through ownership and the borrow checker without a garbage collector".into(),
                    thumbnail: None, tags: vec!["siteA.com".into(), "page".into()],
                    created_at: 0,
                },
                Node {
                    id: "p_2".into(), kind: NodeKind::Page,
                    title: "Memory safety in Rust".into(),
                    subtitle: "siteB.com".into(),
                    preview: "Memory safety in Rust comes from ownership rules and the borrow checker enforced at compile time".into(),
                    thumbnail: None, tags: vec!["siteB.com".into(), "page".into()],
                    created_at: 0,
                },
                Node {
                    id: "p_3".into(), kind: NodeKind::Page,
                    title: "Banana bread recipe".into(),
                    subtitle: "siteC.com".into(),
                    preview: "A delicious banana bread recipe with walnuts cinnamon and brown sugar baked for an hour".into(),
                    thumbnail: None, tags: vec!["siteC.com".into(), "page".into()],
                    created_at: 0,
                },
            ],
            edges: vec![],
        };
        derive_edges(&mut snap);
        let sim: Vec<_> = snap.edges.iter()
            .filter(|e| matches!(e.reason, EdgeReason::SimilarContent(_))).collect();
        // The two Rust pages (different domains) should link;
        // the banana page should not link to either.
        assert!(sim.iter().any(|e|
            (e.from == "p_1" && e.to == "p_2") || (e.from == "p_2" && e.to == "p_1")),
            "the two Rust pages should be SimilarContent: {:?}", sim);
        assert!(!sim.iter().any(|e| e.from == "p_3" || e.to == "p_3"),
            "banana page should not be similar to the Rust pages");
        // Score is recoverable.
        for e in &sim {
            assert!(e.reason.similarity_score() > 0.0 && e.reason.similarity_score() <= 1.0);
        }
    }

    #[test]
    fn derive_edges_shared_tag_ignores_trivial() {
        let mut snap = GraphSnapshot {
            nodes: vec![
                Node {
                    id: "a".into(), kind: NodeKind::Page,
                    title: "A".into(), subtitle: "".into(), preview: "".into(),
                    thumbnail: None,
                    tags: vec!["page".into(), "Rust".into()],
                    created_at: 0,
                },
                Node {
                    id: "b".into(), kind: NodeKind::Page,
                    title: "B".into(), subtitle: "".into(), preview: "".into(),
                    thumbnail: None,
                    tags: vec!["page".into(), "Rust".into()],
                    created_at: 0,
                },
            ],
            edges: vec![],
        };
        derive_edges(&mut snap);
        // Should produce a SharedTag("Rust") edge but NOT a
        // SharedTag("page") edge (page is a trivial kind label).
        let shared: Vec<_> = snap.edges.iter()
            .filter(|e| matches!(&e.reason, EdgeReason::SharedTag(_))).collect();
        assert_eq!(shared.len(), 1);
        if let EdgeReason::SharedTag(t) = &shared[0].reason {
            assert_eq!(t, "Rust");
        }
    }

    #[test]
    fn short_hash_stable() {
        // Same input → same output across calls.
        assert_eq!(short_hash("hello"), short_hash("hello"));
        // Different input → different output (no guarantee but
        // good signal at this scale).
        assert_ne!(short_hash("hello"), short_hash("world"));
    }

    #[test]
    fn iso8601_to_unix_round_numbers() {
        // Just verify a known anchor. Jan 1 1970 = 0.
        let n = parse_iso8601_to_unix("1970-01-01T00:00:00Z");
        assert_eq!(n, 0);
        // Jan 2 1970 = 86400.
        let n = parse_iso8601_to_unix("1970-01-02T00:00:00Z");
        assert_eq!(n, 86400);
    }

    #[test]
    fn iso8601_to_unix_invalid_returns_zero() {
        assert_eq!(parse_iso8601_to_unix("not a date"), 0);
        assert_eq!(parse_iso8601_to_unix(""), 0);
    }

    #[test]
    fn parse_workflow_summary_basic() {
        let toml = r#"
            name = "deploy"
            created_at = 1747929600
            commands = ["git add -A", "git commit", "git push"]
        "#;
        let (n, first, ts) = parse_workflow_summary(toml);
        assert_eq!(n, 3);
        assert_eq!(first, "git add -A");
        assert_eq!(ts, 1747929600);
    }

    // ─── fuzzy_filter tests (v0.1.323) ───

    fn mk_snap(nodes: Vec<(&str, &str, &str, &str, Vec<&str>)>) -> GraphSnapshot {
        // (id_suffix, title, subtitle, preview, tags) → GraphSnapshot.
        let mut snap = GraphSnapshot::default();
        for (i, (id_suffix, title, subtitle, preview, tags)) in nodes.into_iter().enumerate() {
            snap.nodes.push(Node {
                id: format!("test_{}_{}", id_suffix, i),
                kind: NodeKind::Page,
                title: title.to_string(),
                subtitle: subtitle.to_string(),
                preview: preview.to_string(),
                thumbnail: None,
                tags: tags.into_iter().map(String::from).collect(),
                created_at: 0,
            });
        }
        snap
    }

    #[test]
    fn fuzzy_empty_query_returns_all() {
        let snap = mk_snap(vec![
            ("a", "First", "", "", vec![]),
            ("b", "Second", "", "", vec![]),
        ]);
        let r = fuzzy_filter(&snap, "", 100);
        assert_eq!(r, vec![0, 1]);
    }

    #[test]
    fn fuzzy_empty_query_cap_works() {
        let snap = mk_snap(vec![
            ("a", "First", "", "", vec![]),
            ("b", "Second", "", "", vec![]),
            ("c", "Third", "", "", vec![]),
        ]);
        let r = fuzzy_filter(&snap, "", 2);
        assert_eq!(r, vec![0, 1]);
    }

    #[test]
    fn fuzzy_title_match_wins() {
        let snap = mk_snap(vec![
            ("a", "Random page", "", "rust mentioned", vec![]),
            ("b", "Rust programming guide", "", "", vec![]),
            ("c", "Cooking with potatoes", "", "", vec![]),
        ]);
        let r = fuzzy_filter(&snap, "rust", 100);
        // Title match (b) scores highest; preview-only (a) lower;
        // (c) excluded.
        assert_eq!(r[0], 1, "title match should be first, got {:?}", r);
        assert!(r.contains(&0));
        assert!(!r.contains(&2));
    }

    #[test]
    fn fuzzy_zero_score_excluded_for_nonempty_query() {
        let snap = mk_snap(vec![
            ("a", "Apples", "", "", vec![]),
            ("b", "Oranges", "", "", vec![]),
        ]);
        let r = fuzzy_filter(&snap, "bananas", 100);
        assert!(r.is_empty(), "no matches → empty result, got {:?}", r);
    }

    #[test]
    fn fuzzy_case_insensitive() {
        let snap = mk_snap(vec![
            ("a", "Rust Lang", "", "", vec![]),
        ]);
        assert_eq!(fuzzy_filter(&snap, "RUST", 10), vec![0]);
        assert_eq!(fuzzy_filter(&snap, "rust", 10), vec![0]);
        assert_eq!(fuzzy_filter(&snap, "rUsT", 10), vec![0]);
    }

    #[test]
    fn fuzzy_token_prefix_match() {
        let snap = mk_snap(vec![
            // Title doesn't contain whole query "rust prog" but
            // both tokens are prefixes of words in the haystack.
            ("a", "Rust Programming Cookbook", "", "", vec![]),
        ]);
        let r = fuzzy_filter(&snap, "rust prog", 10);
        assert_eq!(r, vec![0]);
    }

    #[test]
    fn fuzzy_tag_match() {
        let snap = mk_snap(vec![
            ("a", "Some page", "", "", vec!["rust", "tutorial"]),
            ("b", "Other page", "", "", vec!["cooking"]),
        ]);
        let r = fuzzy_filter(&snap, "tutorial", 10);
        assert_eq!(r[0], 0);
        assert!(!r.contains(&1));
    }

    #[test]
    fn fuzzy_subtitle_match() {
        let snap = mk_snap(vec![
            // URL in subtitle should be searchable.
            ("a", "Random title", "github.com/rust-lang/rust", "", vec![]),
        ]);
        let r = fuzzy_filter(&snap, "github", 10);
        assert_eq!(r, vec![0]);
    }

    #[test]
    fn fuzzy_single_letter_query_ignored() {
        // Tokens < 2 chars are skipped at the token level. The
        // whole-query substring rule still applies if "a" is a
        // substring, which it usually is — most titles contain
        // 'a'. So we test with a 1-char query that ISN'T a
        // substring of anything (to confirm token logic ignores).
        let snap = mk_snap(vec![
            ("a", "Bxyz", "", "", vec![]),
        ]);
        let r = fuzzy_filter(&snap, "q", 10);
        // Whole-query substring check fails ("q" not in "bxyz");
        // token rule short-circuits at len<2. So zero score.
        assert!(r.is_empty());
    }

    #[test]
    fn fuzzy_cap_limits_results() {
        let snap = mk_snap(vec![
            ("a", "Rust 1", "", "", vec![]),
            ("b", "Rust 2", "", "", vec![]),
            ("c", "Rust 3", "", "", vec![]),
            ("d", "Rust 4", "", "", vec![]),
        ]);
        let r = fuzzy_filter(&snap, "rust", 2);
        assert_eq!(r.len(), 2);
    }

    // ─── related_to + pair_weight tests (v0.1.325) ───

    #[test]
    fn edge_weight_calibration() {
        // Calibration test pins the weight values so changes
        // are intentional (they affect the "related to" rankings).
        assert_eq!(EdgeReason::VoiceFromPage.weight(), 4);
        assert_eq!(EdgeReason::VoiceBySpeaker.weight(), 3);
        assert_eq!(EdgeReason::SharedTag("any".into()).weight(), 3);
        assert_eq!(EdgeReason::ChatReferencesPage.weight(), 3);
        assert_eq!(EdgeReason::SameDomain.weight(), 1);
    }

    #[test]
    fn related_to_empty_returns_empty() {
        let snap = GraphSnapshot::default();
        assert!(related_to(&snap, "anything", 5).is_empty());
    }

    #[test]
    fn related_to_zero_cap_returns_empty() {
        let mut snap = mk_snap(vec![
            ("a", "A", "", "", vec![]),
            ("b", "B", "", "", vec![]),
        ]);
        snap.edges.push(Edge {
            from: "test_a_0".into(), to: "test_b_1".into(),
            reason: EdgeReason::SameDomain,
        });
        assert!(related_to(&snap, "test_a_0", 0).is_empty());
    }

    #[test]
    fn related_to_undirected() {
        // Edge from→to should produce a connection regardless
        // of which endpoint we query.
        let mut snap = mk_snap(vec![
            ("a", "A", "", "", vec![]),
            ("b", "B", "", "", vec![]),
        ]);
        snap.edges.push(Edge {
            from: "test_a_0".into(), to: "test_b_1".into(),
            reason: EdgeReason::VoiceBySpeaker,
        });
        let from_a = related_to(&snap, "test_a_0", 5);
        let from_b = related_to(&snap, "test_b_1", 5);
        assert_eq!(from_a, vec![("test_b_1".into(), 3)]);
        assert_eq!(from_b, vec![("test_a_0".into(), 3)]);
    }

    #[test]
    fn related_to_sorts_by_weight() {
        // Two neighbors with different total weights — the
        // stronger one ranks first.
        let mut snap = mk_snap(vec![
            ("a", "A", "", "", vec![]),
            ("b", "B", "", "", vec![]),
            ("c", "C", "", "", vec![]),
        ]);
        // a-b: weight 1 (SameDomain)
        snap.edges.push(Edge {
            from: "test_a_0".into(), to: "test_b_1".into(),
            reason: EdgeReason::SameDomain,
        });
        // a-c: weight 4 (VoiceFromPage)
        snap.edges.push(Edge {
            from: "test_a_0".into(), to: "test_c_2".into(),
            reason: EdgeReason::VoiceFromPage,
        });
        let r = related_to(&snap, "test_a_0", 5);
        assert_eq!(r.len(), 2);
        // Stronger connection first.
        assert_eq!(r[0].0, "test_c_2");
        assert_eq!(r[0].1, 4);
        assert_eq!(r[1].0, "test_b_1");
        assert_eq!(r[1].1, 1);
    }

    #[test]
    fn related_to_sums_parallel_edges() {
        // Two edges between same pair → weights sum.
        let mut snap = mk_snap(vec![
            ("a", "A", "", "", vec![]),
            ("b", "B", "", "", vec![]),
        ]);
        snap.edges.push(Edge {
            from: "test_a_0".into(), to: "test_b_1".into(),
            reason: EdgeReason::SameDomain, // 1
        });
        snap.edges.push(Edge {
            from: "test_a_0".into(), to: "test_b_1".into(),
            reason: EdgeReason::SharedTag("important".into()), // 3
        });
        let r = related_to(&snap, "test_a_0", 5);
        assert_eq!(r, vec![("test_b_1".into(), 4)]);
    }

    #[test]
    fn related_to_cap_respected() {
        let mut snap = mk_snap(vec![
            ("a", "A", "", "", vec![]),
            ("b", "B", "", "", vec![]),
            ("c", "C", "", "", vec![]),
            ("d", "D", "", "", vec![]),
        ]);
        snap.edges.push(Edge {
            from: "test_a_0".into(), to: "test_b_1".into(),
            reason: EdgeReason::SameDomain,
        });
        snap.edges.push(Edge {
            from: "test_a_0".into(), to: "test_c_2".into(),
            reason: EdgeReason::SameDomain,
        });
        snap.edges.push(Edge {
            from: "test_a_0".into(), to: "test_d_3".into(),
            reason: EdgeReason::SameDomain,
        });
        let r = related_to(&snap, "test_a_0", 2);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn related_to_unknown_id_returns_empty() {
        let snap = mk_snap(vec![
            ("a", "A", "", "", vec![]),
        ]);
        assert!(related_to(&snap, "ghost_id_not_present", 5).is_empty());
    }

    #[test]
    fn pair_weight_basic() {
        let mut snap = mk_snap(vec![
            ("a", "A", "", "", vec![]),
            ("b", "B", "", "", vec![]),
        ]);
        snap.edges.push(Edge {
            from: "test_a_0".into(), to: "test_b_1".into(),
            reason: EdgeReason::SameDomain,
        });
        snap.edges.push(Edge {
            from: "test_a_0".into(), to: "test_b_1".into(),
            reason: EdgeReason::VoiceFromPage,
        });
        // Symmetric.
        assert_eq!(pair_weight(&snap, "test_a_0", "test_b_1"), 5);
        assert_eq!(pair_weight(&snap, "test_b_1", "test_a_0"), 5);
    }

    #[test]
    fn pair_weight_zero_when_no_edge() {
        let snap = mk_snap(vec![
            ("a", "A", "", "", vec![]),
            ("b", "B", "", "", vec![]),
        ]);
        assert_eq!(pair_weight(&snap, "test_a_0", "test_b_1"), 0);
    }
}
