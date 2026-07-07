#!/usr/bin/env python3
"""check_delimiters.py — comment/string/lifetime-aware delimiter balancer.

A blind Claude can't compile; this catches the unbalanced-brace class of error a
compiler would. Handles Rust (// , /* */, "..", r"..", r#".."#, char vs lifetime)
and GLSL (// , /* */, ".."). Returns nonzero if any file is unbalanced.

Usage: python3 handoff/check_delimiters.py <file.rs> [<file.comp> ...]
"""
import sys

def check(path):
    s = open(path, encoding="utf-8", errors="replace").read()
    i, n, line = 0, len(s), 1
    st = []
    pairs = {')': '(', ']': '[', '}': '{'}
    rust = path.endswith((".rs",))
    while i < n:
        c = s[i]
        if c == '\n':
            line += 1; i += 1; continue
        if c == '/' and i + 1 < n and s[i + 1] == '/':
            j = s.find('\n', i); i = n if j < 0 else j; continue
        if c == '/' and i + 1 < n and s[i + 1] == '*':
            j = s.find('*/', i + 2)
            line += s.count('\n', i, (j if j >= 0 else n))
            i = n if j < 0 else j + 2; continue
        if rust and c == 'r' and i + 1 < n and s[i + 1] in '"#':
            k = i + 1; h = 0
            while k < n and s[k] == '#':
                h += 1; k += 1
            if k < n and s[k] == '"':
                term = '"' + '#' * h
                j = s.find(term, k + 1)
                line += s.count('\n', i, (j if j >= 0 else n))
                i = n if j < 0 else j + len(term); continue
        if c == '"':
            j = i + 1
            while j < n:
                if s[j] == '\\':
                    j += 2; continue
                if s[j] == '"':
                    break
                if s[j] == '\n':
                    line += 1
                j += 1
            i = j + 1; continue
        if rust and c == "'":
            # char literal ('x' / '\x') vs lifetime ('a). char closes within 3.
            if (i + 2 < n and s[i + 1] == '\\') or (i + 2 < n and s[i + 2] == "'"):
                j = i + 1
                j += 2 if s[j] == '\\' else 1
                if j < n and s[j] == "'":
                    i = j + 1; continue
            i += 1; continue
        if c in '([{':
            st.append((c, line)); i += 1; continue
        if c in ')]}':
            if not st or st[-1][0] != pairs[c]:
                return f"MISMATCH '{c}' at line {line} (open stack top: {st[-1] if st else None})"
            st.pop(); i += 1; continue
        i += 1
    return "OK" if not st else f"UNCLOSED (last opens: {st[-3:]})"

def main(argv):
    if len(argv) < 2:
        print("usage: check_delimiters.py <files...>"); return 2
    bad = 0
    for p in argv[1:]:
        try:
            r = check(p)
        except FileNotFoundError:
            r = "FILE NOT FOUND"
        print(f"{r:40} {p}")
        if r != "OK":
            bad += 1
    return 1 if bad else 0

if __name__ == "__main__":
    sys.exit(main(sys.argv))
