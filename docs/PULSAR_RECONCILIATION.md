# Pulsar inspiration packages vs. where aperture / voiceterm / uikit-egui are now

21 Atom/Pulsar packages, reconciled against the monorepo as of c19unified.
Everything below was checked against source, not recalled.

The headline: **the inline-rendering packages are the ones that matter, and
the code shows exactly why "getting the size to feel right" was hard.** It
was not a tuning problem. It was two architectural choices, both visible in
`inline-markdown-images/index.js`.

---

## 1. The sizing problem, solved by looking at the source

### Finding A — the size was one global CSS string

```js
config: {
    defaultHeight: {
        type: 'string',
        default: '150px',
        description: 'CSS-style option for adjusting inline image size'
    },
```

One number, in device pixels, for every image in every document. Nothing
about the image's own aspect, nothing about the text around it, no per-image
override. There is no setting of `150px` that is right for both a wide
sequence diagram and a tall flowchart, so it always felt wrong somewhere.

### Finding B — there was an attempt at per-image sizing, and it was abandoned

```js
const scaledHeight = hit.matchText.match(markdownImageLinkCaptureGroupRegExp)
```

`scaledHeight` is assigned and **never read again** — the only occurrence in
the file is that line. Someone started extracting a size from the markdown
link itself and stopped. That is the missing feature, half-built, still in
the tree.

### Finding C — block decorations force variable row height

```js
return editor.decorateMarker(marker, { type: 'block', item: imageContainer,
                                       position: 'before' })
```

An Atom **block decoration** inserts a full-width element *above* the line.
Consequences, all of them structural:

* the image can never be "to the right of" anything — block decorations span
  the full width by construction;
* every rendered image changes its line's height, so scroll position, jump-
  to-line and scrollbar length all become estimates;
* the estimates change as images load asynchronously (`image.onload` flips
  `display` to `block`), so the document *reflows while you read it*.

That last one is the real culprit behind "never felt right". It is not the
dimensions that were wrong, it is that the page moved underneath you.

**`impakt-inline-charts` tried to escape it** and the evidence is a local
shadow:

```js
const strBlock = 'inline';          // shadows the module-level 'block'
return editor.decorateMarker(marker, { type: strBlock, item: imageContainer,
                                    // position: strBefore   ← commented out
```

Someone changed the decoration type to `inline` and commented out the
position. That is the same instinct as "render it to the right of the
comment" — reached by editing a constant, because Atom's decoration model
had no better handle to pull.

### How the present design differs

The aperture proposal sizes from the *comment block's row span*: height =
(lines in the block) × `line_h`, width = the dead margin right of the code,
aspect from the SVG `viewBox`. Three consequences follow directly from the
three findings above:

| Pulsar package | aperture design |
|---|---|
| one global `150px` | size derived per-diagram from its own comment block |
| `scaledHeight` never wired | the size control *is* the source — write more comment lines |
| block decoration, full width, above | painted into the right margin, row span unchanged |
| reflows on `image.onload` | deterministic SVG, cached on `(source_hash, font_px)` |

And the reason aperture can do this and Atom could not: `aperture-egui`'s
renderer is a uniform grid — `line * line_h` appears in the caret, the scroll
target, both follow modes and `PageGrid`. Keeping row height constant is not
a nicety there, it is what makes the margin approach the *only* one that does
not require rewriting the camera.

---

## 2. What the monorepo already has

| Pulsar package | present-day counterpart | state |
|---|---|---|
| `atom-ide-definitions`, `atom-ide-hyperclick` | `aperture-lsp` (client.rs, rpc.rs) + `aperture-core::route` / `workspace_symbols` | **present**, two tiers — deterministic scoring plus LSP |
| `autocomplete-clang`, `autocomplete-glsl` | `aperture-complete` (Debounce, Engine, newest-request-wins) | **present**, language-agnostic rather than per-language |
| `teletype-diy` | `aperture-collab` (370 lines) | **present**, early |
| `markdown-preview-plus`, `-enhanced` | voiceterm `note_render.rs` + the Typora blend in `docs/design/note_editor.md` | **present**, and further — live-preview blend, not a side pane |
| `markdown-mindmap` (markmap + d3) | `nifty-core::mermaid` + `graph_layout` + voiceterm `layered_layout.rs` (Sugiyama) | **present**, better — real crossing reduction, zero deps |
| `svg-preview` | `nifty-core::svg` — `render()`, `render_source()`, 330 lines | **present** as a *producer*; no viewer |
| `markdown-image-paste` | — | **absent** |
| `Chromo` / `Chromo-Color-Previews` | — | **absent** in aperture; `snapclone-theme` has 29 palettes but no inline swatch |
| `impakt-inline-images` / `-charts` | the design above | **designed, not built** |
| `buffer-list`, `maximize-panes`, `pane-status-bar` | `aperture/TODO_panes_and_collab.md`; voiceterm has panes via zellij | **partial** — aperture is explicitly single-pane in v0 |
| `impakt-quick-menu` | `uikit-egui::quick_launcher`, `command_bar`, `palette_flow` | **present**, three variants |
| `impakt-ui` ("ultra-minimalist theme") | `snapclone-theme` 29 palettes + `uikit-egui::skin` carved material + `theme_bridge` | **present**, far beyond it |
| `ask-stack` (Stack Overflow in-editor) | aperture's F2 loop pane — every AI request, latency and outcome as a row | **present**, different shape: the model is the source, and the loop is inspectable |

---

## 3. The three real gaps

**Colour previews.** `Chromo` renders a swatch beside any hex / rgb / hsl /
named colour / gradient. Nothing in aperture does this, and it is the single
cheapest win in the set: the lexer already emits spans, a hex literal is a
regex, and the swatch is a filled rect in the margin — the *same* margin the
diagram design uses, at one row tall instead of N. It is the diagram feature
with the size problem deleted, so it is also the right thing to build first
to prove the margin mechanism.

**Paste image into markdown.** `markdown-image-paste` binds Ctrl+V to "write
the clipboard image next to the document and insert the link". voiceterm's
`.assets/` convention already defines where the file goes and
`manpages::resolve_one_img_src` already resolves it. The only missing piece
is the clipboard read.

**An SVG viewer.** `nifty-core` produces SVG and nothing in the fleet renders
it. That is the actual blocker for inline diagrams — not parsing, not layout,
not sizing. A minimal SVG-subset painter over `egui::Painter` covering what
`svg.rs` itself emits (rects, rounded rects, lines, polylines, text) is a
bounded job, because the producer and consumer are the same codebase and you
control the subset.

---

## 4. Things the old packages did that the new design deliberately drops

* **Rendering remote images.** `allowNetworkLinks` fetches from the web
  inside the editor. Off by default there; should not exist here — a diagram
  in a code comment that requires the network is a diagram that is blank on a
  plane, and it is an exfiltration surface in a file you did not write.
* **Async load driving layout.** Everything upstream reflows on
  `image.onload`. The cached deterministic SVG render removes the async step
  entirely: the size is known before anything is painted.
* **Per-language autocomplete packages.** `autocomplete-clang` and
  `autocomplete-glsl` are separate packages with separate config.
  `aperture-complete` is one engine with a provider trait, which is why
  adding a language is a spec rather than a package.

---

## 5. Suggested order

1. **Colour swatches in the margin** — proves the margin mechanism with no
   sizing question at all.
2. **SVG painter over the `nifty-core` subset** — the actual blocker.
3. **Inline diagrams from comments**, using the derived-size rule.
4. **Image paste**, which is small and independent.

`teletype-diy` is worth reading before `aperture-collab` goes further, but as
a source of failure modes rather than architecture — its CRDT assumptions do
not match an "edit is a value" model where every mutation already carries its
own inverse.
