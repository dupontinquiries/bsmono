#!/usr/bin/env python3
"""
qml_lint.py — pre-ship lint for web2qml QML files.

Two checks:

  1) Brace balance.
     Naive {/}/(/)/[/] counting, but regex-aware:
     strips string literals AND regex literals before counting.
     The pre-v0.1.152 brace counter (in this project's working
     memory) didn't handle regex and reported a phantom diff=1
     on main.qml for many ships. This version distinguishes
     between `/` as division and `/` as regex-start by looking
     at the previous non-whitespace character.

  2) Unguarded backend method calls in QML bindings.
     The segfault in user's v0.1.149 testing came from a
     binding evaluating `backend.term_in_alt_screen()` when
     backend was null. The fix everywhere else in the codebase
     is a `backend && ` or `backend ?` short-circuit before the
     method call.
     This check flags `backend.<name>(` calls whose containing
     line OR previous 3 lines don't contain a backend null-guard.
     Conservative: some false positives expected when the guard
     is structurally further away (e.g. at the top of a long
     function). Maintainers should add an inline guard or a
     "// LINT-OK: <reason>" comment to suppress.

Run from the repo root:
    python3 scripts/qml_lint.py
Exit code: 0 if no findings, 1 otherwise.
"""

import os
import re
import sys
from pathlib import Path


# ──────────────────────────────────────────────────────────────────
# Check 1: brace balance with regex-aware string/comment/regex strip
# ──────────────────────────────────────────────────────────────────


def strip_strings_comments_regex(src: str) -> str:
    """Return src with strings, comments, and regex literals replaced
    by spaces (preserving line structure for nicer error reporting).

    The regex detection is heuristic: a `/` is the start of a regex
    literal if the previous non-whitespace character is one that can
    legitimately precede an expression (operators, opening punctuation,
    statement separators). Otherwise it's division.
    """
    out = []
    i = 0
    in_lc = False           # // line comment
    in_bc = False           # /* block comment */
    in_s = None             # current string delimiter (" or ')
    in_regex = False
    prev_nonspace = ''

    while i < len(src):
        c = src[i]
        n = src[i + 1] if i + 1 < len(src) else ''

        if in_lc:
            if c == '\n':
                in_lc = False
                out.append(c)
            else:
                out.append(' ')
            i += 1
            continue

        if in_bc:
            if c == '*' and n == '/':
                in_bc = False
                out.append('  ')
                i += 2
                continue
            out.append('\n' if c == '\n' else ' ')
            i += 1
            continue

        if in_s:
            if c == '\\':
                out.append('  ')
                i += 2
                continue
            if c == in_s:
                in_s = None
                out.append(' ')
                i += 1
                continue
            out.append('\n' if c == '\n' else ' ')
            i += 1
            continue

        if in_regex:
            if c == '\\':
                out.append('  ')
                i += 2
                continue
            if c == '/':
                in_regex = False
                out.append(' ')
                i += 1
                continue
            out.append('\n' if c == '\n' else ' ')
            i += 1
            continue

        # Detect comment starts
        if c == '/' and n == '/':
            in_lc = True
            i += 2
            out.append('  ')
            continue
        if c == '/' and n == '*':
            in_bc = True
            i += 2
            out.append('  ')
            continue

        # Detect string starts
        if c == '"' or c == "'":
            in_s = c
            out.append(' ')
            i += 1
            continue

        # Detect regex literal — `/` preceded by a non-identifier non-
        # numeric context. Operators, open-brackets, commas, semicolons,
        # etc. trigger regex mode; alphanumerics and `)` `]` are
        # division.
        if c == '/':
            if prev_nonspace == '' or prev_nonspace in '({[,;=!&|?:+-*~^<>%':
                in_regex = True
                out.append(' ')
                i += 1
                continue
            # else falls through to be appended as a division operator

        if not c.isspace():
            prev_nonspace = c
        out.append(c)
        i += 1

    return ''.join(out)


def check_brace_balance(path: Path) -> list[str]:
    """Return list of finding strings for this file."""
    try:
        src = path.read_text()
    except (OSError, UnicodeDecodeError) as e:
        return [f"{path}: could not read ({e})"]

    stripped = strip_strings_comments_regex(src)
    findings = []
    for pair in [('{', '}'), ('(', ')'), ('[', ']')]:
        o = stripped.count(pair[0])
        cl = stripped.count(pair[1])
        if o != cl:
            findings.append(
                f"{path}: unbalanced {pair[0]}{pair[1]} — "
                f"{o} open / {cl} close (diff {o - cl})"
            )
    return findings


# ──────────────────────────────────────────────────────────────────
# Check 2: unguarded backend method calls
# ──────────────────────────────────────────────────────────────────


# Matches: backend.identifier(  — method call, NOT property read.
# Captures the method name for reporting.
BACKEND_CALL_RE = re.compile(r'\bbackend\.([a-zA-Z_]\w*)\s*\(')

# Guards we recognize as "this scope has null-checked backend":
#   if (!backend) ...        — early return / guard clause
#   if (backend) ...         — same
#   backend && ...           — short-circuit conjunction
#   backend !== null         — explicit null check (with && or in conditional)
#   backend ?                — ternary guard
#   typeof backend           — defensive existence check
#   // LINT-OK                — explicit suppression
GUARD_PATTERNS = [
    re.compile(r'!\s*backend\b'),               # !backend
    re.compile(r'\bbackend\s*&&'),              # backend &&
    re.compile(r'&&\s*backend\b'),              # && backend
    re.compile(r'\bbackend\s*!==?\s*null\b'),   # backend !== null / != null
    re.compile(r'\bbackend\s*===?\s*null\b'),   # backend === null
    re.compile(r'null\s*!==?\s*backend\b'),     # null !== backend
    re.compile(r'\bbackend\s*\?'),              # backend ? ...
    re.compile(r'\btypeof\s+backend\b'),        # typeof backend
    re.compile(r'if\s*\(\s*backend\s*[\)\&]'),  # if (backend) or if (backend &&
    re.compile(r'if\s*\(\s*!\s*backend\b'),     # if (!backend ...)
    re.compile(r'//\s*LINT-OK'),                # explicit suppression
]


# Calls that are safe regardless because they're attribute reads, not
# methods. Exclusion list for property names that are commonly bound.
# We catch by `.NAME(` syntax above; property reads like `backend.theme_idx`
# never match the (-suffix.
# This is just here for future docs.
KNOWN_PROPERTY_READS = ['theme_idx', 'last_query', 'recursion_depth']


def line_guards_backend(line: str) -> bool:
    """Return True if this line contains a recognized backend guard."""
    return any(p.search(line) for p in GUARD_PATTERNS)


def check_unguarded_backend(path: Path) -> list[str]:
    """Flag backend.<method>() calls in BINDING contexts (property RHS
    expressions) without a null-guard on the same line.

    Why bindings only: bindings re-evaluate whenever dependencies
    change, which includes the dependency being briefly null during
    startup, hot-reload, or shutdown. The v0.1.149 segfault came
    from exactly this — a `visible: ... && !backend.term_in_alt_screen()`
    binding fired with backend == null.

    Handler bodies (`onClicked: { ... }`, `function foo() { ... }`)
    only run on user actions and usually have explicit early-return
    guards at the top. Flagging them too produces noise.

    The heuristic: a line is in BINDING context if it matches
    `<propertyName>:<rhs>` AND the property name does NOT start with
    `on` (which would be a handler). Lines inside function bodies or
    multi-line handlers will start mid-expression and miss this
    pattern — they're handled by walking back to find the binding
    they're part of, capped at 5 lines.
    """
    try:
        src = path.read_text()
    except (OSError, UnicodeDecodeError) as e:
        return [f"{path}: could not read ({e})"]

    stripped = strip_strings_comments_regex(src)
    lines = stripped.split('\n')
    orig_lines = src.split('\n')  # preserved for LINT-OK suppression lookup
    findings = []

    # Pattern for "this line starts a binding RHS"
    # e.g. "visible: !backend.x()" or "    width: 38"
    BINDING_START_RE = re.compile(r'^\s*([A-Za-z_]\w*(?:\.\w+)?)\s*:')
    HANDLER_NAME_RE = re.compile(r'^on[A-Z]')

    # Keywords that look like property bindings but are actually
    # language constructs. `case foo:` in a switch, `default:` in a
    # switch, etc. We DO want to flag the case-body if it contains
    # an unguarded backend call, but the case body lives in a
    # handler/function scope, not a binding scope — so skipping
    # these effectively defers detection to the enclosing scope
    # detection (which finds the handler above).
    KEYWORD_LABELS = {'case', 'default'}

    # Looking back to find the binding context for a multi-line RHS.
    def find_binding_context(idx: int) -> tuple[bool, str | None]:
        """Walk back up to 5 lines to find a `<prop>:` start.
        Return (is_binding, prop_name).  is_binding=False if the
        nearest start is a handler (onFoo:), a switch label (case/
        default:), or we couldn't find one.
        """
        for back in range(idx, max(-1, idx - 6), -1):
            m = BINDING_START_RE.match(lines[back])
            if m:
                prop = m.group(1)
                # Switch labels: `case X:` and `default:` aren't bindings.
                # The body that follows runs inside an enclosing function,
                # which the search will not re-enter (we stop at first
                # match). Skip these so we don't false-flag case bodies.
                if prop in KEYWORD_LABELS:
                    return (False, prop)
                # `on<Capital>` is a signal/event handler — not a binding.
                if HANDLER_NAME_RE.match(prop):
                    return (False, prop)
                return (True, prop)
        return (False, None)

    for idx, line in enumerate(lines):
        for m in BACKEND_CALL_RE.finditer(line):
            method = m.group(1)

            # LINT-OK suppression: check the ORIGINAL source (comments
            # preserved) within the 6-line window. Without this, the
            # stripped source has comments blanked out and the
            # suppression marker is invisible to subsequent checks.
            sup_start = max(0, idx - 6)
            sup_window = '\n'.join(orig_lines[sup_start:idx + 1])
            if re.search(r'//\s*LINT-OK', sup_window):
                continue

            # Same-line guard?
            if line_guards_backend(line):
                continue

            # Determine binding-vs-handler context.
            is_binding, prop = find_binding_context(idx)
            if not is_binding:
                continue  # in a handler body or function — skip

            # Check a small window around the line for a guard. The
            # window is JOINED before regex-matching so multi-line
            # guards like
            #   text: backend
            #       ? backend.foo()
            #       : ""
            # are recognized (the `backend ?` pattern spans lines 1
            # and 2). 6-line window catches longer ternaries with
            # nested conditions like termScreen.colorMode ? a : b.
            window_start = max(0, idx - 6)
            window_text = '\n'.join(lines[window_start:idx + 1])
            if any(p.search(window_text) for p in GUARD_PATTERNS):
                continue

            findings.append(
                f"{path}:{idx + 1}: unguarded backend.{method}() in "
                f"binding `{prop}:` — add a `backend &&` short-circuit "
                f"or `// LINT-OK: <reason>` if backend is guaranteed."
            )

    return findings


# ──────────────────────────────────────────────────────────────────
# Driver
# ──────────────────────────────────────────────────────────────────


def check_empty_components(path: Path) -> list[str]:
    """Flag empty `Component { id: ... }` declarations.

    Qt's QML loader fails at runtime with "Cannot create empty
    component specification" — the error doesn't surface during
    `qml -compile` style checks because the failure is at
    instantiation, not parse. So we lint statically: a Component
    block with only an id/comment line and no child item type
    is almost certainly a leftover scaffold from a refactor.

    A valid Component has at least one capitalized-name child
    QML type declaration (e.g. `Rectangle { ... }`,
    `ColumnLayout { ... }`) between the `Component {` and the
    matching `}`.
    """
    src = path.read_text()
    stripped = strip_strings_comments_regex(src)
    findings = []
    # Find each "Component {" opener and scan forward, brace-
    # balanced, for the matching close. Then check if the body
    # contains a child type declaration.
    i = 0
    while i < len(stripped):
        m = re.search(r'\bComponent\s*\{', stripped[i:])
        if not m:
            break
        open_pos = i + m.end() - 1  # position of '{'
        # Walk forward to matching '}'.
        depth = 1
        j = open_pos + 1
        while j < len(stripped) and depth > 0:
            ch = stripped[j]
            if ch == '{':
                depth += 1
            elif ch == '}':
                depth -= 1
            j += 1
        if depth != 0:
            # Unbalanced — brace check will catch it. Skip.
            break
        body = stripped[open_pos + 1:j - 1]
        # Look for a capitalized-name child type declaration:
        # an identifier starting with [A-Z] followed by `{`.
        has_child = re.search(r'\b[A-Z][A-Za-z0-9_]*\s*\{', body) is not None
        if not has_child:
            # Determine line number of the `Component` keyword.
            line_no = stripped[:i + m.start()].count('\n') + 1
            findings.append(
                f"{path}:{line_no}: empty Component {{...}} — Qt will fail to load")
        i = j
    return findings


def check_chord_registry_sync(path: Path) -> list[str]:
    """v0.1.354: chord registry sync check.

    The chordOverlay (v0.1.353) reads its content from a hand-
    curated `root.chordRegistry` array. Adding a new Shortcut
    element without also updating the registry leaves the help
    screen stale. This check finds the mismatch.

    Algorithm:
      1. Parse every `Shortcut { sequence: "..." ... }` declaration
         and collect the sequence strings.
      2. Parse the chordRegistry array and collect every `keys:
         "..."` string. Split on " / " so multi-key entries (e.g.
         "F1 / Ctrl+/ / Ctrl+?") expand to their individual keys.
      3. For each Shortcut sequence not in the registry, warn.
      4. For each registry key not in any Shortcut, warn —
         unless it's a known-WM-only entry (e.g. "(WM)") or a
         compound passthrough description.

    The check is intentionally tolerant: some chords are handled
    in non-Shortcut paths (Keys.onPressed on Items, helixKeys
    block, terminal-tab passthrough). The chord overlay is
    documentation, so the registry can be a superset of the
    Shortcut elements. We only loudly flag the more dangerous
    case: a Shortcut exists that the registry doesn't document.

    Override pattern: a Shortcut intentionally undocumented can
    carry a `// chord-registry-skip` comment on the same line
    or the immediately preceding line.
    """
    # Only runs on the main.qml file. Sub-component QML files
    # don't have a chord registry concept.
    if path.name != 'main.qml':
        return []
    src = path.read_text()
    stripped = strip_strings_comments_regex(src)
    findings = []

    # 1) Collect Shortcut sequences. We use the ORIGINAL src for
    # the actual string values (the strip step removed string
    # contents). Pattern: `Shortcut { sequence: "X+Y"` —
    # tolerant of whitespace and the newline-style we use.
    seq_pattern = re.compile(r'Shortcut\s*\{\s*sequence:\s*"([^"]+)"')
    raw_shortcuts = []
    for m in seq_pattern.finditer(src):
        seq = m.group(1)
        # Check for skip marker on the same or preceding line.
        start_line = src[:m.start()].count('\n') + 1
        # Look at this line and previous line in the source.
        src_lines = src.split('\n')
        skip = False
        for li in (start_line - 1, start_line - 2):
            if 0 <= li < len(src_lines) and 'chord-registry-skip' in src_lines[li]:
                skip = True
                break
        if not skip:
            raw_shortcuts.append((seq, start_line))

    # 2) Collect chordRegistry keys. Find the array literal and
    # extract every `keys: "..."` inside.
    reg_match = re.search(r'property\s+var\s+chordRegistry\s*:\s*\[', src)
    if not reg_match:
        # No registry — overlay machinery is absent. Skip silently;
        # other checks will catch real problems.
        return findings
    # Walk forward, bracket-balanced, to find the matching ].
    reg_start = reg_match.end() - 1  # position of '['
    depth = 1
    j = reg_start + 1
    while j < len(stripped) and depth > 0:
        ch = stripped[j]
        if ch == '[':
            depth += 1
        elif ch == ']':
            depth -= 1
        j += 1
    if depth != 0:
        return findings  # brace check will catch
    reg_body = src[reg_start:j]
    keys_pattern = re.compile(r'keys:\s*"([^"]+)"')
    registry_keys = set()
    for m in keys_pattern.finditer(reg_body):
        # Split multi-chord entries on " / " so each key is
        # checkable individually.
        for k in m.group(1).split(' / '):
            registry_keys.add(k.strip())

    # 3) Each Shortcut sequence should appear (literally) in the
    # registry's split keys. Sequences with Qt-style spelling
    # (e.g. "Ctrl+Shift+W") need to match exactly — the registry
    # is what shows to users, so this enforces consistent spelling.
    for seq, line_no in raw_shortcuts:
        if seq not in registry_keys:
            findings.append(
                f"{path}:{line_no}: Shortcut sequence \"{seq}\" "
                f"missing from chordRegistry (add an entry, or "
                f"mark with `// chord-registry-skip` on the same "
                f"or preceding line to suppress)")

    return findings


# v0.1.415: Qt6-only QML APIs that break QtQuick 2.15 (Qt 5.15)
# at COMPONENT LOAD — a fatal "X is not available in QtQuick
# 2.15" that takes the whole window down. cargo check doesn't
# see QML; the brace/struct checks don't know Qt version
# semantics; so this class of bug shipped clean and only failed
# on hardware (v0.1.412's DragHandler.activeTranslation). This
# denylist catches the known offenders statically.
#
# Each entry: (regex, human explanation incl. the 2.15
# replacement). Keep the regex anchored on a word boundary so we
# don't false-positive on substrings. These are matched against
# the comment/string-stripped source so references inside
# comments don't trip the check.
QT6_ONLY_APIS = [
    (r'\bactiveTranslation\b',
     "DragHandler.activeTranslation is Qt6-only; use `translation` "
     "(+ onTranslationChanged) on QtQuick 2.15"),
    (r'\bonActiveTranslationChanged\b',
     "onActiveTranslationChanged is Qt6-only; use onTranslationChanged "
     "on QtQuick 2.15"),
    (r'\bactiveScale\b',
     "PinchHandler.activeScale is Qt6-only; use `scale` on QtQuick 2.15"),
    (r'\bpersistentTranslation\b',
     "persistentTranslation is Qt6-only on QtQuick 2.15"),
    (r'\bpersistentScale\b',
     "persistentScale is Qt6-only on QtQuick 2.15"),
    (r'\bCapsuleGeometry\b',
     "CapsuleGeometry is Qt 6.x-only"),
]


def check_popup_anchors(path: Path) -> list[str]:
    """Flag `anchors.*` bindings on a Popup/Dialog/Menu ROOT.

    Popup (and its Controls subtypes Dialog/Menu) is NOT an Item — it
    has no `anchors` property. `anchors.centerIn: parent` on a Popup
    root is a hard QML LOAD error that takes the whole component down
    ("Cannot assign to non-existent property anchors"), so the
    component silently never instantiates. cargo check can't see it and
    the old lint passed it clean — this is the check that would have
    caught v0.1.541's SetupWizard (zero [wizard-qml] logs because the
    Popup root used anchors.centerIn). Popups center via x/y or
    anchors.centerIn: Overlay.overlay, never bare `anchors` on the root.
    """
    src = path.read_text()
    stripped = strip_strings_comments_regex(src)
    lines = stripped.split('\n')
    # Find the root component type: first line whose trimmed text is an
    # identifier ending in `{` (after imports/pragmas).
    root_type = None
    root_line_idx = None
    for i, ln in enumerate(lines):
        t = ln.strip()
        if not t or t.startswith('import') or t.startswith('pragma'):
            continue
        m = re.match(r'^([A-Z][A-Za-z0-9_.]*)\s*\{', t)
        if m:
            root_type = m.group(1)
            root_line_idx = i
            break
        # Any other non-trivial line before the root → stop looking.
        if not re.match(r'^//', t):
            break
    POPUP_TYPES = {"Popup", "Dialog", "Menu"}
    if root_type not in POPUP_TYPES:
        return []
    findings = []
    # Scan top-level (depth 1) lines for an `anchors.` binding. Track
    # brace depth from just after the root's opening brace.
    depth = 0
    started = False
    for i in range(root_line_idx, len(lines)):
        ln = lines[i]
        # Update depth using brace counts on this line.
        opens = ln.count('{')
        closes = ln.count('}')
        # A top-level binding sits at depth 1 (inside the root only).
        if started and depth == 1:
            if re.match(r'\s*anchors\.', ln):
                # Allow the Overlay form which IS valid.
                if 'Overlay.overlay' not in ln:
                    findings.append(
                        f"{path}:{i+1}: `anchors` on a {root_type} root — "
                        f"{root_type} is not an Item; this is a LOAD error "
                        f"that prevents instantiation. Center via x/y or "
                        f"anchors.centerIn: Overlay.overlay.")
        depth += opens - closes
        if opens > 0:
            started = True
    return findings


def check_qt6_only_apis(path: Path) -> list[str]:
    """Flag Qt6-only QML APIs that fail to load on QtQuick 2.15.

    These are the highest-severity QML bugs: they don't parse-
    error, they LOAD-error, taking the whole component (window)
    down at runtime with "X is not available in QtQuick 2.15".
    cargo check can't see them. This is the check that would have
    caught v0.1.412's DragHandler.activeTranslation before it
    shipped.
    """
    src = path.read_text()
    # Only enforce when the file actually targets QtQuick 2.x.
    # (If a file ever imports QtQuick 6, these would be valid.)
    if not re.search(r'import\s+QtQuick\s+2\.', src):
        return []
    stripped = strip_strings_comments_regex(src)
    findings = []
    for pat, explain in QT6_ONLY_APIS:
        for m in re.finditer(pat, stripped):
            # Compute line number from match offset.
            line_no = stripped.count('\n', 0, m.start()) + 1
            findings.append(f"{path}:{line_no}: Qt6-only API — {explain}")
    return findings


# v0.1.424: QML signal-handler names must capitalize the first
# letter AFTER the leading `on`. For a property `fooBar`, the
# changed signal is `fooBarChanged` and the handler is
# `onFooBarChanged`. For an underscore-prefixed property like
# `_tiltAngle`, the handler is `on_TiltAngleChanged` (the char
# after the underscore is uppercased: t -> T). Getting this wrong
# (e.g. `on_tiltAngleChanged`) is NOT a parse error — QML logs
# "not a properly capitalized signal handler name" at LOAD and
# the handler silently never fires. cargo check can't see it, so
# it lurks until someone reads the runtime log. This caught the
# v0.1.329 graph-tilt edge-repaint handler.
#
# Heuristic: find `function on<X>(` declarations (inside
# Connections blocks, where this matters). Flag any where the
# character right after the `on` prefix — skipping a single
# leading underscore — is a lowercase letter.
_HANDLER_RE = re.compile(r'\bfunction\s+(on[A-Za-z_]\w*)\s*\(')


def check_signal_handler_caps(path: Path) -> list[str]:
    """Flag `function onXxx(` handlers whose first post-`on`
    letter is lowercase — QML rejects them at load (handler never
    fires), no parse error."""
    src = path.read_text()
    stripped = strip_strings_comments_regex(src)
    findings = []
    for m in _HANDLER_RE.finditer(stripped):
        name = m.group(1)           # e.g. on_tiltAngleChanged / onClicked
        rest = name[2:]             # after "on"
        if not rest:
            continue
        # Skip a single leading underscore (underscore-prefixed
        # property). The NEXT char is the one QML uppercases.
        idx = 1 if rest.startswith('_') else 0
        if idx >= len(rest):
            continue
        first = rest[idx]
        # Lowercase ascii letter in the capitalize slot = wrong.
        # (Built-in handlers like onClicked/onPressed are fine —
        # their first letter is already uppercase.)
        if first.islower() and first.isascii():
            line_no = stripped.count('\n', 0, m.start()) + 1
            correct = name[:2] + rest[:idx] + first.upper() + rest[idx + 1:]
            findings.append(
                f"{path}:{line_no}: signal handler '{name}' is mis-capitalized "
                f"(QML rejects it at load, handler never fires) — use '{correct}'")
    return findings


def check_int_pixelsize(path: Path) -> list[str]:
    """Flag fractional `font.pixelSize: N.M` (and `pointSize`).

    pixelSize/pointSize are `int` in QML — a fractional literal is an
    "Invalid property assignment: int expected" LOAD error that fails the
    whole component (ReplVoiceGlow.qml had `pixelSize: 10.5`, which kept
    the app from starting). cargo check + the other lint rules don't see
    it. (This rule was first added then LOST to a workspace reset, while
    the bug it catches silently came back — restored v0.1.575.)
    """
    src = path.read_text()
    stripped = strip_strings_comments_regex(src)
    findings = []
    for m in re.finditer(r'\bfont\.(pixelSize|pointSize)\s*:\s*([0-9]+\.[0-9]+)', stripped):
        line_no = stripped.count('\n', 0, m.start()) + 1
        findings.append(
            f"{path}:{line_no}: font.{m.group(1)} = {m.group(2)} is fractional but "
            f"the property is int — QML fails the component load "
            f"('int expected'). Use an integer.")
    return findings


def check_signal_property_collision(path: Path) -> list[str]:
    """Flag an explicit `signal Xchanged()` that collides with a property's
    auto-generated change signal (or `signal X()` named like a property).

    QML auto-creates `<prop>Changed` for every `property`. Declaring a signal
    with that exact name is "Duplicate signal name: invalid override of property
    change signal" — a FATAL component LOAD error (the whole UI fails to start).
    cargo check can't see it, and the other rules didn't either; shipped once in
    v0.1.715 (peerPanel's `property var peers` + `signal peersChanged()`),
    caught only at runtime. Restored as a gate here (v0.1.716).
    """
    src = path.read_text()
    stripped = strip_strings_comments_regex(src)
    props = set(re.findall(r'\bproperty\s+(?:var|int|bool|real|double|string|'
                           r'color|url|font|point|rect|size|date|list\s*<[^>]*>|'
                           r'[A-Za-z_]\w*)\s+(\w+)', stripped))
    findings = []
    for m in re.finditer(r'\bsignal\s+(\w+)\s*\(', stripped):
        sig = m.group(1)
        line_no = stripped.count('\n', 0, m.start()) + 1
        # signal "Pchanged" collides with property P's auto change-signal
        if sig.endswith("Changed") and sig[:-7] in props:
            findings.append(
                f"{path}:{line_no}: signal '{sig}()' collides with the "
                f"auto-generated change signal of property '{sig[:-7]}' — "
                f"FATAL QML load error ('Duplicate signal name'). Remove the "
                f"explicit signal; reassigning the property already notifies.")
        # signal named exactly like a property is also an override
        elif sig in props:
            findings.append(
                f"{path}:{line_no}: signal '{sig}()' has the same name as "
                f"property '{sig}' — invalid override, FATAL QML load error.")
    return findings


def check_animation_target(path: Path) -> list[str]:
    """Flag `XxxAnimation on <prop>` where the NEAREST ENCLOSING object
    does not declare <prop>.

    The `Animation on <prop>` attached form binds to a property of the
    nearest enclosing object. v0.1.564's ReplVoiceGlow declared _halo on
    the ROOT but put `SequentialAnimation on _halo` inside a CHILD
    (card) — QML looked for _halo on the card, didn't find it, and
    failed the WHOLE component load ("Cannot assign to non-existent
    property _halo"), so the app wouldn't start. Note: the property DID
    exist in the file (on the root), so a naive "declared anywhere"
    check misses it — we must check the *enclosing* object specifically.

    Approach: walk objects by brace depth. For each `... on <prop>`,
    find the innermost object block that encloses it and check whether
    THAT block (its own direct body, not nested children, not the
    parent) declares <prop>. Built-in animatable props are skipped.
    """
    src = path.read_text()
    stripped = strip_strings_comments_regex(src)
    builtin = {
        'opacity', 'x', 'y', 'z', 'width', 'height', 'scale', 'rotation',
        'color', 'radius', 'anchors', 'spacing', 'border', 'visible',
        'implicitWidth', 'implicitHeight', 'contentY', 'contentX',
    }
    # Build a depth map: for each char offset, which brace depth we're at.
    # Then for an animation match, the enclosing object is the block that
    # opened at depth-1 (the Animation itself opens a block at the match).
    # We find the enclosing object's body span and scan it (excluding
    # deeper-nested blocks) for `property ... <prop>`.
    findings = []
    for m in re.finditer(r'\b([A-Z][A-Za-z0-9_]*Animation)\s+on\s+([A-Za-z_][A-Za-z0-9_.]*)', stripped):
        prop = m.group(2).split('.')[0]
        if prop in builtin:
            continue
        # Find the enclosing object: walk backward from the match,
        # tracking brace depth, to the nearest unmatched '{' — that's the
        # open brace of the object the animation sits in. Then find its
        # matching close.
        pos = m.start()
        depth = 0
        open_idx = None
        i = pos - 1
        while i >= 0:
            c = stripped[i]
            if c == '}':
                depth += 1
            elif c == '{':
                if depth == 0:
                    open_idx = i
                    break
                depth -= 1
            i -= 1
        if open_idx is None:
            continue
        # matching close brace of the enclosing object
        depth = 1
        j = open_idx + 1
        close_idx = len(stripped)
        while j < len(stripped):
            c = stripped[j]
            if c == '{':
                depth += 1
            elif c == '}':
                depth -= 1
                if depth == 0:
                    close_idx = j
                    break
            j += 1
        body = stripped[open_idx:close_idx]
        # Does THIS object's body declare the property? (Nested children's
        # declarations also appear in `body`, but a false-negative there is
        # safe — we only emit when NO declaration is in scope at all, which
        # is the load-failing case.)
        decl_re = re.compile(
            r'\bproperty\s+(?:var|real|int|bool|string|color|double|alias)\s+'
            + re.escape(prop) + r'\b')
        if decl_re.search(body):
            continue
        line_no = stripped.count('\n', 0, m.start()) + 1
        findings.append(
            f"{path}:{line_no}: '{m.group(1)} on {m.group(2)}' — enclosing object "
            f"does not declare '{prop}' (it may be declared on a PARENT). The `on` "
            f"form binds to the nearest object; a mismatch fails the WHOLE component "
            f"load. Move the property here, or use a standalone Animation with "
            f"explicit target:/property:.")
    return findings


def check_id_property_collision(path: Path) -> list[str]:
    """v0.1.736: an element `id:` that matches a `property` name elsewhere in
    the document is a latent footgun — QML ids are document-global, so the id
    shadows the property in unqualified scopes. Assigning to such a property
    throws 'left-hand side ... is not an lvalue' at runtime (this is what broke
    card-tap preview: id _previewText shadowed cardsCanvas's _previewText
    property, so _openPreview's `_previewText = ...` failed). Flag any name used
    as BOTH an id and a property."""
    import re as _re
    text = path.read_text(encoding='utf-8', errors='replace')
    ids = set(_re.findall(r'\bid:\s*([A-Za-z_]\w*)', text))
    props = set(_re.findall(
        r'\bproperty\s+(?:var|int|bool|real|double|string|color|url|font|'
        r'date|point|rect|size|alias)\s+([A-Za-z_]\w*)', text))
    findings = []
    for name in sorted(ids & props):
        findings.append(f"{path.name}: name '{name}' is used as BOTH an element "
                        f"id and a property name — ids are document-global and "
                        f"shadow the property (assigning to it throws 'not an "
                        f"lvalue'). Rename one.")
    return findings


def check_dup_root_functions(path: Path) -> list[str]:
    """v0.1.805: a function name declared twice at the same object scope fails
    the QML compile with 'Duplicate method name' — and qmllint does NOT catch it
    (this is what broke v804: a second `shellQuote` was added at root scope).
    Heuristic: flag duplicate `function NAME` at the file's root indent (4 spaces),
    where the root object's methods live. Per-file, so same names in different
    files don't collide."""
    import re as _re
    try:
        text = path.read_text(encoding='utf-8', errors='replace')
    except Exception as e:
        return [f"{path}: could not read ({e})"]
    seen: dict[str, int] = {}
    findings = []
    for m in _re.finditer(r'(?m)^    function\s+([A-Za-z_]\w*)\s*\(', text):
        name = m.group(1)
        line_no = text.count('\n', 0, m.start()) + 1
        if name in seen:
            findings.append(f"{path.name}:{line_no}: duplicate root function "
                            f"'{name}' (first at line {seen[name]}) — QML compile "
                            f"fails with 'Duplicate method name'. Rename or remove one.")
        else:
            seen[name] = line_no
    return findings


def main():
    script_dir = Path(__file__).resolve().parent
    repo_root = script_dir.parent
    qml_dir = repo_root / 'qml'

    if not qml_dir.is_dir():
        print(f"qml_lint: no qml/ directory at {qml_dir}", file=sys.stderr)
        return 2

    qml_files = sorted(qml_dir.rglob('*.qml'))
    if not qml_files:
        print(f"qml_lint: no .qml files found under {qml_dir}", file=sys.stderr)
        return 2

    all_findings = []
    for f in qml_files:
        all_findings.extend(check_brace_balance(f))
        all_findings.extend(check_unguarded_backend(f))
        all_findings.extend(check_empty_components(f))
        all_findings.extend(check_chord_registry_sync(f))
        all_findings.extend(check_qt6_only_apis(f))
        all_findings.extend(check_popup_anchors(f))
        all_findings.extend(check_signal_handler_caps(f))
        all_findings.extend(check_animation_target(f))
        all_findings.extend(check_int_pixelsize(f))
        all_findings.extend(check_signal_property_collision(f))
        all_findings.extend(check_id_property_collision(f))
        all_findings.extend(check_dup_root_functions(f))

    if not all_findings:
        print(f"qml_lint: {len(qml_files)} file(s) clean")
        return 0

    print(f"qml_lint: {len(all_findings)} finding(s) across "
          f"{len(qml_files)} file(s):")
    for f in all_findings:
        print(f"  {f}")
    return 1


if __name__ == '__main__':
    sys.exit(main())
