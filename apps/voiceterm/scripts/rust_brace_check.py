#!/usr/bin/env python3
"""Rust-aware brace/paren depth tracker.

Handles:
- // line comments
- /* block comments */ (non-nested per Rust spec — actually Rust DOES
  allow nested block comments, but our codebase doesn't use them)
- "string literals" with \" escapes
- 'a'/'\n'/'\x7f' char literals
- 'lifetime references (not char literals)
"""
import sys, re

def check(path):
    with open(path) as f:
        src = f.read()
    i = 0
    n = len(src)
    depth = 0
    paren = 0
    bracket = 0
    line = 1
    col = 1
    line_at_paren_zero = None
    while i < n:
        c = src[i]
        nx = src[i+1] if i+1 < n else ''
        # newline
        if c == '\n':
            line += 1; col = 1; i += 1; continue
        # line comment
        if c == '/' and nx == '/':
            while i < n and src[i] != '\n': i += 1
            continue
        # block comment
        if c == '/' and nx == '*':
            i += 2
            while i < n - 1 and not (src[i] == '*' and src[i+1] == '/'):
                if src[i] == '\n': line += 1; col = 1
                i += 1
            i += 2
            continue
        # string literal
        if c == '"':
            i += 1
            while i < n:
                if src[i] == '\\':
                    if src[i+1] == '\n': line += 1; col = 1
                    i += 2
                    continue
                if src[i] == '"':
                    i += 1
                    break
                if src[i] == '\n': line += 1; col = 1
                i += 1
            continue
        # raw string r"..." or r#"..."#
        if c == 'r' and (nx == '"' or nx == '#'):
            j = i + 1
            hashes = 0
            while j < n and src[j] == '#':
                hashes += 1; j += 1
            if j < n and src[j] == '"':
                j += 1
                close = '"' + '#' * hashes
                while j < n:
                    if src[j:j+len(close)] == close:
                        j += len(close)
                        break
                    if src[j] == '\n': line += 1; col = 1
                    j += 1
                i = j
                continue
        # char literal vs lifetime
        if c == "'":
            # peek to disambiguate
            # 'x' or '\x' or '\u{...}'
            # vs 'a or 'static (lifetime)
            if i+1 < n:
                if src[i+1] == '\\':
                    # definitely char literal
                    j = i + 2
                    if j < n and src[j] == 'x':
                        j += 3  # \xNN
                    elif j < n and src[j] == 'u':
                        while j < n and src[j] != '}': j += 1
                        j += 1
                    else:
                        j += 1
                    if j < n and src[j] == "'":
                        i = j + 1
                        continue
                elif i+2 < n and src[i+2] == "'":
                    # 'x' single-char literal
                    i += 3
                    continue
                # otherwise it's a lifetime
                i += 1
                continue
        if c == '{':
            depth += 1
        elif c == '}':
            depth -= 1
            if depth < 0:
                print(f"{path}:{line}:{col}: brace went negative")
                return False
        elif c == '(':
            paren += 1
            if paren == 1: line_at_paren_zero = line
        elif c == ')':
            paren -= 1
            if paren < 0:
                print(f"{path}:{line}:{col}: paren went negative")
                return False
        elif c == '[':
            bracket += 1
        elif c == ']':
            bracket -= 1
            if bracket < 0:
                print(f"{path}:{line}:{col}: bracket went negative")
                return False
        i += 1; col += 1
    if depth != 0:
        print(f"{path}: ENDED with brace depth {depth}")
        return False
    if paren != 0:
        print(f"{path}: ENDED with paren depth {paren} (last open near line {line_at_paren_zero})")
        return False
    if bracket != 0:
        print(f"{path}: ENDED with bracket depth {bracket}")
        return False
    print(f"{path}: clean")
    return True

ok = True
for p in sys.argv[1:]:
    if not check(p):
        ok = False
sys.exit(0 if ok else 1)
