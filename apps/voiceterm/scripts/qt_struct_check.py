#!/usr/bin/env python3
"""
qt_struct_check.py — pre-ship semantic check for the Backend struct.

Two checks:

  1) qt_method/property/signal field declared but missing from the
     `Self { ... }` initializer block.

     Catches the v0.1.184 class of bug. Rust will error with E0063
     ("missing field in initializer of"), but only at compile time.
     This check catches it before invoking cargo so the iteration
     loop stays fast on slow machines.

  2) qt_property NOTIFY clause referencing an undeclared signal.

     Added v0.1.201. Catches `prop: qt_property!(T; NOTIFY foo_changed)`
     where `foo_changed: qt_signal!()` was forgotten. Rust will
     error at compile time but the message is confusing; this gives
     a direct pointer to the missing signal.

We restrict the check to the Backend struct because that's where
the qt_* macros are used in this codebase. Generalizing to other
structs would require parsing more carefully.

The check is deliberately conservative:
  - It only flags fields declared with qt_method!, qt_property!,
    or qt_signal! in the struct definition.
  - It only checks against the Self { ... } block(s) inside the
    same file.
  - It tolerates the qt_property's NOTIFY-emitter pair (a
    qt_property line and a qt_signal line on the same line via
    `prop: qt_property!(T; NOTIFY changed), changed: qt_signal!()`)
    by parsing each comma-separated field.

Run:
    python3 scripts/qt_struct_check.py src/main.rs

Exit code: 0 if all checks pass; 1 otherwise.
"""

import re
import sys
from pathlib import Path


# Match a field declaration in the struct. We accept:
#   name: qt_method!(...)
#   name: qt_property!(...)
#   name: qt_signal!()
# Names are valid Rust identifiers. The macro args are matched
# permissively (we don't parse them, just consume up to the
# closing paren). We don't try to handle nested parens deeply —
# the macros in this codebase don't nest.
QT_FIELD_RE = re.compile(
    r'\b(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*qt_(?:method|property|signal)!'
)

# v0.1.201: detect qt_property NOTIFY targets and the signals
# declared in the struct, so we can cross-check that every NOTIFY
# points to a real signal.
#
# A property's NOTIFY clause looks like:
#   name: qt_property!(QString; NOTIFY signal_name)
#
# After NOTIFY, the next identifier is the signal name. We capture
# both the property's own name (for error messages) and the
# referenced signal name.
QT_PROPERTY_NOTIFY_RE = re.compile(
    r'\b(?P<prop>[a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*qt_property!\s*\('
    r'[^)]*?NOTIFY\s+(?P<signal>[a-zA-Z_][a-zA-Z0-9_]*)'
)

# qt_signal! declarations. The macro has empty args () or args
# describing the signal's payload (uncommon in this codebase but
# possible).
QT_SIGNAL_DECL_RE = re.compile(
    r'\b(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*qt_signal!'
)

# Note: regex-only field detection is fragile because expression
# values can contain identifiers in path positions (e.g.
# `Default::default()` — `Default` looks like a field name to a
# naive matcher). We use a small structural splitter below
# instead: split the Self body on TOP-LEVEL commas (not inside
# nested parens/braces/brackets/strings), then take the first
# identifier of each item.

IDENT_RE = re.compile(r'[a-zA-Z_][a-zA-Z0-9_]*')


def parse_init_field_names(body: str) -> set:
    """Walk `body` (the contents of a Self { ... } block), split
    on top-level commas, and return the set of field names being
    initialized. Handles both `name: <expr>,` and shorthand
    `name,` forms."""
    names = set()
    depth_paren = 0
    depth_brace = 0
    depth_bracket = 0
    in_str = False
    in_line_comment = False
    in_block_comment = False
    i = 0
    n = len(body)
    item_start = 0

    def commit(start: int, end: int):
        chunk = body[start:end]
        # Strip leading whitespace + comments. The chunk often
        # begins with a `//` comment block before the actual
        # field-name (e.g.
        #   anthropic_api_key: QString::from(...),
        # preceded by `// v0.1.161: ...` comment lines). Without
        # this, the IDENT_RE would match the first word inside
        # the comment (`v0`, `Compile`, etc.) instead of the
        # field name.
        j = 0
        m = len(chunk)
        while j < m:
            # Skip whitespace.
            while j < m and chunk[j] in ' \t\n\r':
                j += 1
            if j >= m:
                break
            # Skip line comment.
            if chunk[j] == '/' and j+1 < m and chunk[j+1] == '/':
                while j < m and chunk[j] != '\n':
                    j += 1
                continue
            # Skip block comment.
            if chunk[j] == '/' and j+1 < m and chunk[j+1] == '*':
                j += 2
                while j < m-1 and not (chunk[j] == '*' and chunk[j+1] == '/'):
                    j += 1
                j += 2
                continue
            break
        # Now j points at the first non-comment, non-whitespace
        # character — should be the field name.
        ident_match = IDENT_RE.match(chunk, j)
        if ident_match:
            names.add(ident_match.group(0))

    while i < n:
        c = body[i]
        nx = body[i+1] if i+1 < n else ''

        if in_line_comment:
            if c == '\n':
                in_line_comment = False
            i += 1
            continue
        if in_block_comment:
            if c == '*' and nx == '/':
                in_block_comment = False
                i += 2
                continue
            i += 1
            continue
        if in_str:
            if c == '\\':
                i += 2
                continue
            if c == '"':
                in_str = False
            i += 1
            continue

        if c == '/' and nx == '/':
            in_line_comment = True
            i += 2
            continue
        if c == '/' and nx == '*':
            in_block_comment = True
            i += 2
            continue
        if c == '"':
            in_str = True
            i += 1
            continue

        if c == '(':
            depth_paren += 1
        elif c == ')':
            depth_paren -= 1
        elif c == '{':
            depth_brace += 1
        elif c == '}':
            depth_brace -= 1
        elif c == '[':
            depth_bracket += 1
        elif c == ']':
            depth_bracket -= 1
        elif c == ',' and depth_paren == 0 and depth_brace == 0 and depth_bracket == 0:
            commit(item_start, i)
            item_start = i + 1

        i += 1

    # Final item (after last comma).
    commit(item_start, n)
    return names


def find_struct_body(src: str, struct_name: str = 'Backend'):
    """Return the substring of `src` between the `{` and matching
    `}` of `pub struct Backend { ... }` (or `struct Backend`).
    Returns None if not found."""
    # Match `[pub ]struct Backend [<...>] {`
    pat = re.compile(
        r'(?:pub\s+)?struct\s+' + re.escape(struct_name) + r'\b[^{]*\{'
    )
    m = pat.search(src)
    if not m:
        return None
    start = m.end()  # position right after `{`
    depth = 1
    i = start
    in_str = False
    in_line_comment = False
    in_block_comment = False
    n = len(src)
    while i < n and depth > 0:
        c = src[i]
        nx = src[i+1] if i+1 < n else ''
        if in_line_comment:
            if c == '\n':
                in_line_comment = False
            i += 1
            continue
        if in_block_comment:
            if c == '*' and nx == '/':
                in_block_comment = False
                i += 2
                continue
            i += 1
            continue
        if in_str:
            if c == '\\':
                i += 2
                continue
            if c == '"':
                in_str = False
            i += 1
            continue
        if c == '/' and nx == '/':
            in_line_comment = True
            i += 2
            continue
        if c == '/' and nx == '*':
            in_block_comment = True
            i += 2
            continue
        if c == '"':
            in_str = True
            i += 1
            continue
        if c == '{':
            depth += 1
        elif c == '}':
            depth -= 1
            if depth == 0:
                return src[start:i]
        i += 1
    return None


def find_self_blocks(src: str):
    """Find all `Self { ... }` struct-literal blocks in the source.
    Returns a list of (start_offset, body_string) tuples.

    Distinguishes a struct literal from a function return type by
    checking the preceding non-whitespace character: if it's `>`
    (from `-> Self {`), this is a function-signature return type
    and the following `{` opens the function body, not a struct
    literal. We skip those.
    """
    pat = re.compile(r'\bSelf\s*\{')
    results = []
    for m in pat.finditer(src):
        # Look backwards for the preceding non-whitespace char.
        # If it's `>`, this is `-> Self {` (return type), not a
        # struct literal — skip.
        j = m.start() - 1
        while j >= 0 and src[j] in ' \t\n\r':
            j -= 1
        if j >= 0 and src[j] == '>':
            continue
        start = m.end()
        depth = 1
        i = start
        n = len(src)
        in_str = False
        in_line_comment = False
        in_block_comment = False
        while i < n and depth > 0:
            c = src[i]
            nx = src[i+1] if i+1 < n else ''
            if in_line_comment:
                if c == '\n':
                    in_line_comment = False
                i += 1
                continue
            if in_block_comment:
                if c == '*' and nx == '/':
                    in_block_comment = False
                    i += 2
                    continue
                i += 1
                continue
            if in_str:
                if c == '\\':
                    i += 2
                    continue
                if c == '"':
                    in_str = False
                i += 1
                continue
            if c == '/' and nx == '/':
                in_line_comment = True
                i += 2
                continue
            if c == '/' and nx == '*':
                in_block_comment = True
                i += 2
                continue
            if c == '"':
                in_str = True
                i += 1
                continue
            if c == '{':
                depth += 1
            elif c == '}':
                depth -= 1
                if depth == 0:
                    results.append((m.start(), src[start:i]))
                    break
            i += 1
    return results


def check(path: str) -> bool:
    src = Path(path).read_text()

    struct_body = find_struct_body(src, 'Backend')
    if struct_body is None:
        # No Backend struct in this file. Not an error — this
        # script only validates files that contain it.
        print(f"{path}: no Backend struct found; skipping")
        return True

    declared = set(m.group('name') for m in QT_FIELD_RE.finditer(struct_body))
    if not declared:
        print(f"{path}: no qt_* fields declared; nothing to check")
        return True

    self_blocks = find_self_blocks(src)
    if not self_blocks:
        print(f"{path}: declared {len(declared)} qt_* fields but found no Self {{ ... }} block")
        return False

    # The Backend's initializer is the largest Self block (others
    # might exist for nested types or test fixtures). Use the
    # largest one as the canonical initializer.
    largest = max(self_blocks, key=lambda x: len(x[1]))
    init_body = largest[1]

    initialized = parse_init_field_names(init_body)

    missing = sorted(declared - initialized)
    if missing:
        print(f"{path}: {len(missing)} qt_* field(s) declared but not in Self initializer:")
        for name in missing:
            print(f"  - {name}")
        print(f"  (the Self block we checked has {len(initialized)} field initializers)")
        return False

    # v0.1.201: NOTIFY → signal cross-check.
    #
    # Every qt_property with a `NOTIFY foo` clause must have a
    # corresponding `foo: qt_signal!()` declaration in the same
    # struct. A missing signal produces a confusing Rust error
    # at compile time; this catches it earlier with a clearer
    # message.
    declared_signals = set(
        m.group('name') for m in QT_SIGNAL_DECL_RE.finditer(struct_body)
    )
    notify_targets = [
        (m.group('prop'), m.group('signal'))
        for m in QT_PROPERTY_NOTIFY_RE.finditer(struct_body)
    ]
    bad_notify = [
        (prop, sig) for prop, sig in notify_targets
        if sig not in declared_signals
    ]
    if bad_notify:
        print(f"{path}: {len(bad_notify)} qt_property NOTIFY clause(s) reference undeclared signals:")
        for prop, sig in bad_notify:
            print(f"  - property `{prop}` declares NOTIFY `{sig}`, but `{sig}: qt_signal!()` is not in the struct")
        return False

    n_notify = len(notify_targets)
    n_signals = len(declared_signals)
    print(f"{path}: clean ({len(declared)} qt_* fields, all initialized; {n_notify} NOTIFY clauses → {n_signals} signals)")
    return True


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    ok = True
    for p in sys.argv[1:]:
        if not check(p):
            ok = False
    sys.exit(0 if ok else 1)


if __name__ == '__main__':
    main()
