# scribe -- what is built and what is next

Built (7b5e74933): the quick pipeline (scenes and lines, `NAME: words`), real Fountain, truly centred HTML export in six
styles with font/size/paper/scene-number options, the `scribe` CLI, and the `scribe-egui` editor (type | page, autosave).

## Next (the owner's design, in the order it was asked)

1. **A Markdown project folder, readable with no tools on any system.** One file per scene (`scenes/01-alley.md`: the quick
   format IS Markdown: `# heading`, lines, `NAME: words`), `flow.md` as an ordered list of links with branches as nested
   lists, `project.md` for the title page and notes, `characters/*.md` (actor sheets and rosters as tables), `shots/*.md`
   (camera setup per scene: lens, framing, movement, body settings; link the snapshot-cam lens profiles), `plan/*.md`.
   Everything a person can open in any text viewer.
2. **File buckets:** read PARTIAL files into scenes: a bucket names a source file and a line range or heading, and a scene pulls
   from it (so the existing `femme-fatale.fountain` and `.md` feed scenes without being copied). `flow` composes scenes
   into a script; a workspace layout (buckets | flow | page) for the editor.
3. **Git history for several writers:** branch timelines and a graph view. The same machinery voiceterm-egui and aperture need,
   so build it once as a shared kit (commit DAG -> lanes -> a layout the UI draws), reuse `aperture-collab`'s RGA CRDT for live
   shared editing, and `aperture-core`'s buffer/undo for the text. Also written out as `history.md` (an ASCII graph) so it reads
   anywhere.
4. **A Prezi-style zoomable canvas** for the timeline/graph (and for the flow board): pan and zoom between scenes, branches as
   paths, a scene opens by zooming into it. `ink-kit`'s camera and the aperture Smooth camera are the starting points.

## Extras (later, only if wanted)

- **Host a LAN session** to show scripts off or share them: a read-only live view first (a small HTTP page of the exported HTML
  that refreshes), then shared editing over `aperture-collab`; `netcode`/`peermux` are the transport candidates in the repo.
- **The in-process wgpu engine for quick ideas:** `aperture-engine` links logscope's wgpu inference directly (no daemon):
  offer "a line that follows this one", loglines, alternate scene openings, as ghost text, off by default.
- **aperture-style AI completion** in the writing pane (the loop pane that shows every request, as aperture does).
- A face/dictation path: `voice-kit` + whisper on device for dictating lines on the phone (ties to the hatch voice-message idea).
- PDF export directly (the repo has no PDF writer crate; today the HTML prints to PDF from any browser).

## Shared kits (what voiceterm already has, and what is missing)

Read from voiceterm (2026-10-03): its HTML pipeline is HTML -> QML (Qt); its egui `browser.rs` only DETECTS actionable widgets (forms,
chat endpoints), it does not render pages. What IS reusable, all pure and Qt-free:

1. **`note_render.rs`: the WYSIWYG blend.** "The caret's line shows as raw source, every other line is rendered", per block, pure.
   That is exactly the single-surface editor scribe wants (the focused block is editable source, the rest is the formatted page),
   and its `[[wikilinks]]` (zbrackets) are how a scene can link a character sheet or a bucket.
2. **`link_graph.rs` + `niftyplot_view.rs` (+ `connections_graph.rs`, `graph_view.rs`):** a link graph with backlinks, ring layout
   around a focus, one-ring-at-a-time expansion, directional navigation, breadcrumbs, and the "layers rotate" orientation maths.
   voiceterm's egui frontend mounts them with `#[path]`; aperture needs the same for a repository graph and scribe for scene
   flow and branch history. Make them ONE shared crate (`shared/graph-kit`), keep voiceterm's Qt and egui mounts working, and
   put the Prezi-style camera (pan/zoom between nodes) on top, with `ink-kit`'s camera as the starting point.
3. **The missing piece, `shared/doc-kit`:** a small block/inline document model with two ways OUT: `to_html` (scribe's `html.rs` is
   the seed: escape, emphasis, centred/left blocks, print CSS) and an egui painter (scribe's `editor.rs` preview is the seed), and
   ways IN from Markdown and a Fountain/quick dialect (scribe's `script.rs`) and, later, an HTML subset. The browser question
   (an angelfish-style wrapper vs growing voiceterm-egui's web support) needs the HTML -> block model -> egui direction, which
   does not exist yet; scribe's exporter is the reverse of the same model, so building doc-kit once serves the writer, the
   browser, hatch notices and voice-message summaries.

Order: graph-kit first (it unblocks the git graph for scribe and aperture and is mostly moving files), then the WYSIWYG block
editor on note_render's pattern, then doc-kit.
