# Terminal layout bugs (deferred)

## Cursor not aligned

User report (multiple sessions): the cursor overlay rectangle drawn by
QML doesn't sit exactly on the character that the shell believes it's
at. Visible as a 1-2px offset, more pronounced on some characters.

Suspected causes:
- `fallbackX = 2 + backend.term_cursor_col * termScreen.charW` uses the
  M-glyph advance width as `charW`. Variable-advance rendering by Qt's
  TextEdit (kerning, hinting, fractional widths) means actual glyph X
  positions diverge from this assumption row-to-row.
- The 2px offset hardcoded matches `anchors.margins: 2` on TextEdit
  but might not match Qt's internal RichText padding which can vary
  by Wayland vs X11.

Workaround in v0.1.66: cursorMode setting (0=disabled, 1=static,
2=animated). User can pick disabled if the misalignment bothers them.

Real fix: query TextEdit.cursorRectangle() for the actual screen-space
position of the logical cursor (the underlying char). Already attempted
in earlier versions via `termText.positionToRectangle(pos)` but that
needs the char index, not row+col. Mapping row+col → char-index given
multi-byte and ANSI-escape-stripped content needs a parallel position
table maintained in pty_term.

Estimated size: ~150 lines pty_term changes + 30 lines QML. One session.

## Terminal overflows past bottom

User report: rendered output extends below the termScreen Rectangle's
bottom edge.

Suspected causes:
- The resize math (`r = Math.floor((height - 4 - charH) / charH)`) tells
  the PTY a row count R. But TextEdit.RichText may render the text
  larger than `R * charH` because of:
  - implicit `<pre>` padding in some Qt builds
  - fractional line spacing not matching `charMetrics.lineSpacing`
  - trailing blank lines from PTY output not stripped by qml_gen
- `clip: true` is set on termScreen so children should be clipped.
  If the user sees overflow regardless, it might be the cursor overlay
  (positioned by JS math, not Item-tree-clipped by termScreen).

Real fix: instrument the actual rendered TextEdit.contentHeight vs
termScreen.height ratio in the log. If contentHeight > height, we know
to subtract more rows. If contentHeight matches but cursor is below,
the cursor positioning is the culprit (see above).

Estimated size: ~50 lines instrumentation, then targeted fix. One session.

## Helix scroll fixed in v0.1.66

In alt-screen mode, mouse wheel and touch swipe now send PgUp/PgDn
key bytes (`\x1B[5~` / `\x1B[6~`) instead of trying to scroll our
local scrollback buffer (which is empty in alt-screen anyway).

Works for: helix, less, man, htop with paging, vim with `:set mouse=`.

Doesn't work for apps that explicitly use SGR mouse mode 1006 (e.g.
mosh, some tmux configs). For those, need to send actual SGR mouse
wheel events. Documented as future work.
