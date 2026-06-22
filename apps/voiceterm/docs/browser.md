# Browser

The web tab — how we fetch, parse, and render web pages as native QML.

## Architecture

```
backend.load_page(url, depth)
        ↓
fetch via ureq (with UA fallback if challenged)
        ↓
scraper::Html parse
        ↓
qml_gen::generate(doc, base_url) → big string of QML
        ↓
write to /tmp/web2qml_temp.qml
        ↓
QML Loader replaces the browse tab content
```

## Module

- **`src/qml_gen.rs`** — HTML → QML translator. ~1000 lines. Walks
  the parsed DOM and emits Text/Image/Rectangle/Column elements.

## What we render

- Headings (h1-h6) with size scaling
- Paragraphs with bold/italic/underline spans
- Links (`<a>`) — clickable, with hover state (v0.1.108)
- Images — fetched async, cached
- Code blocks (`<pre>`, `<code>`) — monospace with background
  rectangle
- Buttons — clickable; map to JS hrefs when detectable
- Forms — text fields, checkboxes, radio buttons with label
  association (v0.1.104)
- Lists (ul/ol/li)
- Tables (limited; flat row layout)
- Headers/navigation islands

## What we don't render

- JavaScript execution — we extract static content only.
  JS-only sites won't work.
- Video / audio embeds beyond a basic placeholder
- Complex CSS (we honor a small subset: color, font-size, font-weight,
  font-style, text-decoration, hidden display)
- Iframes (skipped)
- WebGL / Canvas

## Layout cache

`qml_gen` hashes content sections (NavBar, Generic body, etc) and
caches the generated QML by hash. Subsequent renders with the same
section content reuse the cache. Helps when navigating within a site
that has stable headers.

## Ad blocking

`src/ad_filter.rs` runs over the parsed DOM and removes nodes matching
filter selectors (common ad classes, iframe ad networks). Logged as
`AD FILTER: Blocked N elements`.

## Sections

The page is split into recognized "sections":
- **NavBar** — top navigation
- **Header** — page title area
- **Generic** — main content
- **Footer**

Each section renders to its own QML Column inside the scrollable
browse area.

## "Islands"

After basic parsing, an island detector finds interactive elements
that look like widgets: video players, image galleries, comment
threads. These get their own dedicated component instead of being
inlined into the body text.

## alx.sh / CSS-driven static sites (TODO)

Some single-file sites use CSS-only interactivity (radio button
inputs drive `:checked` state for visible tab content). We render
them flat — the user sees all the "tabs" stacked. Future work:
detect this pattern and emit a QML stack with binding to a state
variable.

See [todo.md](todo.md) for current status.

## Link hover (v0.1.108)

Each link has a local `property bool _hover: false` on its Text
element. MouseArea sets `parent._hover = containsMouse`. Color
binds: `_hover ? Qt.lighter(linkColor, 1.4) : linkColor`. Non-bold
links bold on hover.

Earlier attempts used MouseArea ids → duplicate id collisions when
the same page had multiple links in sibling Text elements. Local
property avoids that.

## Form rendering (v0.1.104)

For IRC-log-viewer-style sites with checkbox-driven filters:

- `<input type="checkbox">` finds its associated `<label for=ID>`
  by walking up 4 ancestor levels and using a CSS selector to
  match the label
- Rendered as a CheckBox with the label text
- Standalone `<label for=ID>` is suppressed when an associated
  input was already rendered (to avoid double-display)
- Text/search/email/url/tel inputs render as TextField with
  placeholderText

## URL fallback fetching

If a fetch returns 403/406/challenge response:
1. Retry with a compat User-Agent string (Chrome/Firefox UA)
2. Log `Challenge detected, retrying with compat UA...`

Some sites still reject; we surface the error to the user.

## Page archive

`src/archive.rs` — pages can be archived to local DB
(`~/.config/web2qml/archive.db`). Archive includes original HTML,
generated QML, screenshot path. Lets users re-read offline.

Triggered by voice "archive this page" or palette action.

## Site discover

`backend.site_discover(domain)` does a quick crawl of the domain's
main page to find link patterns and add them to the palette as
discoverable destinations.

## Browser drawer

The browser tab has a slide-in drawer (left edge) showing:
- Recent browser history
- Bookmarks / archive entries
- Action chips (reload, archive, discover site)
- Heading list of current page (jump to section)
