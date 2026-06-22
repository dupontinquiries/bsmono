# TODO: Voice in the web tab

The voice button works on the web tab but the experience is much shallower
than on the terminal tab. Here's what's there, what's missing, and the
priority order to fix it.

## What works today on web tab

- `navigate to <site>` → loads URL
- `search <query>` → search engine query
- `back` / `forward` / `reload`
- `scroll up` / `scroll down` / `go to top` / `go to bottom`
- `find <query>` → opens find-in-page
- `click <link text>` → finds and follows link
- `scroll to <heading>` → jumps to matching section heading

These all work because voice_intent::parse() recognizes them and
executeIntent() in QML dispatches to the right action. The candidates list
shows them, and the browser tab is the default context.

## What's missing or shallow

### High priority (user-visible)

1. **No "what can voice do" hint on the web tab.** The mic FAB is just
   floating there. First-time users don't know they can say "scroll down"
   or "click the third link". A one-shot toast on first browser-tab voice
   button hover, or an info panel in settings, would help.

2. **No voice readback / TTS.** If user says "read me this article" we have
   no TTS. Phrases like "summarize this page" need LLM + readback. Web tab
   could be a much richer experience if we narrate.

3. **Click-by-position commands.** "click the third button", "the second
   image", "the link in the middle of the page" — we have ClickLink for
   text-match but not positional. Useful for icon-heavy pages.

4. **Form-fill by voice.** "type 'hello' in the search box", "submit the
   form". Currently you'd have to tap the field and use voice-to-text.
   Field detection from QML is limited; would need DOM-aware navigation.

### Medium priority

5. **"Open this in a new tab".** We don't have multi-tab browsing yet, so
   this is gated on tab management work.

6. **Voice bookmarks.** "bookmark this page", "open my bookmarks", "go to
   my python bookmark". Easy once we have bookmark storage.

7. **Reading-mode toggle by voice.** "reader mode" / "make this readable" —
   strip ads/sidebars, show prose only. We have ad-filter; just need a
   reader-mode preset that's more aggressive.

8. **Site-specific commands learned from history.** "search wikipedia for
   X", "ddg X", "github search X" — should detect domain history and
   surface as candidates. Per-site URL templates.

### Low priority / speculative

9. **Voice-driven highlighting.** "highlight the part about transformers"
   → search the rendered text and visually mark matches.

10. **Page summarization with LLM.** "summarize this page" → ollama call
    with the visible text. Already have ollama wired for system info; just
    need to thread visible-page text through.

11. **Voice translate.** "translate this to French" → run page text through
    a translation model (which would need to be added).

12. **Voice navigation history scrubbing.** "go to the page about
    PostgreSQL I was reading earlier" → history search + voice match.

## Architectural note

Browser-tab voice today is simpler than terminal-tab voice because there's
no entity extraction. On terminal we extract devices/files/commits from
output and resolve voice references against them. On browser, we'd want to
extract things like "the third link", "the heading about X", "the image of
Y" — and resolve voice references against them. That's a real piece of
work that would unlock a lot of items above (1, 3, 4, 9, 10).

Worth doing when the entity extractor framework is mature on the terminal
side. For now, terminal-tab voice is far ahead of browser-tab voice and
that's fine — the gap is acknowledged and the structure is here to fill it.

---

## v0.1.596 — item 3 (click-by-position) PARSE side done

The intent parsing for positional clicks now exists + is verified:
- New `Intent::ClickPositional { ordinal: i32, kind: String }` (ordinal
  1-based, -1 = last; kind = link/button/image/field/item).
- Pure `parse_positional_click` in voice_intent.rs: "click the third
  link", "tap the last button", "the second blue button" (adjective
  between ordinal and noun handled). Verified standalone (13 cases incl.
  adversarial). CRITICAL no-regression: a non-ordinal click ("click the
  login button") returns None and falls through to text-match ClickLink
  unchanged — tested.
- describe/to-tuple match arms + the browser-context scoring list in
  main.rs updated for the new variant.

STILL PENDING (execution wiring, needs the running build):
- QML executeIntent: add a `case "click_positional"` next to the
  existing `case "click_link"` (qml/main.qml ~16671). It receives kind
  in param + ordinal in the third tuple field.
- Backend: resolve (ordinal, kind) against the page. `page_links()`
  (main.rs ~6937) already returns the link list, so "the Nth link" is:
  parse page_links, pick index (ordinal-1, or last for -1), follow it.
  For kind=button/image/field, need the equivalent element lists from
  the page model (may need a new `page_elements(kind)` backend method).
- Confidence: if ordinal exceeds the element count, toast "only N links
  on this page" rather than silently doing nothing.

So the recognizer is done; the DOM-resolution + QML dispatch are the
build-gated remainder.

### v0.1.597 probe note (no code change)
Adversarially tested parse_positional_click for hijacking: confirmed
"select a file", "select all", "press enter", "press escape", "press
control c", "select the text", "select first responder" all return None
and fall through to their real handlers — the two-gate design (needs BOTH
a valid ordinal as first word AND a valid element-kind as last word)
prevents collisions. Genuine positional clicks ("select the third
result", "press the second button") parse correctly. No bug found; the
parser is sound. (Recorded so this isn't re-probed.)
