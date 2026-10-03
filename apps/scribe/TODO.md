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
