#!/usr/bin/env python3
"""Loops the compiler probably could not vectorize, and why.

## Why

The CPU paths in gpu-gemm are real work — `cpu-dequant type=12 0.72 GB in 0.6s
(16 threads)` in a recent run log, and every recurrent layer of a Gated DeltaNet
model does a download/compute/upload per token. Whether those loops vectorize is
worth a factor of 4-8 on ARM NEON, and nothing currently says which ones do.

The alternative to this tool is building a benchmark harness per loop, or reading
`-C llvm-args=-pass-remarks-analysis=loop-vectorize` output across the whole
crate. Both are more work than the question deserves for a FIRST pass. This is a
triage list: it finds the shapes that are known to defeat the vectorizer and
points at them, so a human decides which are worth measuring.

## What it does NOT do

It does not prove anything. LLVM vectorizes plenty of loops this flags, and
misses plenty it does not. A finding here means "worth looking at", never "this
is slow". Anything acted on should be measured before and after — the whole point
of the list is to make that measurement cheap to aim.

## The patterns, and why each blocks vectorization

Each is something LLVM's loop vectorizer genuinely cannot do, not a style
preference:

* **`push` in a hot loop** — the trip count is not known up front and the buffer
  may reallocate mid-loop, so there is nothing to widen. `Vec::with_capacity`
  plus indexed writes, or `extend` over an iterator, gives it a shape it can use.
* **`.iter().enumerate()` with an index used for a second slice** — the compiler
  must prove the two slices do not alias. Zipping them instead states it.
* **Bounds-checked indexing of two different slices in one body** — same aliasing
  problem, plus a branch per access. Slicing to a common length first, or
  `chunks_exact`, removes both.
* **A `break`/`continue`/`?` inside the body** — an early exit means the trip
  count is not fixed. Sometimes essential; sometimes it can be hoisted.
* **`f32`/`f64` accumulation into one variable** — floating-point addition is not
  associative, so reordering changes the result and the compiler will not do it
  without `-ffast-math` (which Rust does not offer, deliberately). Manual
  multiple accumulators, or `chunks_exact(N)` with N partial sums, is the fix
  and it is an explicit numerical choice, not a free one.
* **A function call that is not obviously inlinable** — an opaque call in the
  body is a hard barrier.

## Use

    maintenance/tools/vectorize_audit.py apps/logscope/crates/gpu-gemm/src
    maintenance/tools/vectorize_audit.py --min-score 3 <paths...>
    maintenance/tools/vectorize_audit.py --json <paths...>

Exit status is 0 always: this is advisory and must never fail a build.
"""

import argparse
import json
import os
import re
import sys

# Loop headers we care about. `loop {` is excluded — an unbounded loop is not a
# vectorization candidate in the first place and flagging it is pure noise.
LOOP_RE = re.compile(r'^\s*(for\s+.+?\s+in\s+.+?\{|while\s+.+?\{)')

# A body this short is not where time goes, and a body this long is a function
# that happens to contain a loop rather than a hot kernel.
MIN_BODY = 2
MAX_BODY = 120


def find_loops(lines):
    """Yield (start_index, end_index_exclusive, header) for each loop body.

    Brace depth rather than indentation, for the same reason egui_chrome_audit.py
    switched to it: rustfmt's wrapping makes indentation an unreliable guide to
    nesting, and a heuristic that is wrong on formatted code is worse than none.
    """
    out = []
    for i, line in enumerate(lines):
        if not LOOP_RE.match(line):
            continue
        # Walk forward to the matching close brace, ignoring braces inside
        # strings and line comments (crude, but these are Rust sources with
        # normal formatting, and a mis-scan costs one bad finding, not a crash).
        depth = 0
        started = False
        for j in range(i, min(i + MAX_BODY + 40, len(lines))):
            code = strip_noncode(lines[j])
            for ch in code:
                if ch == '{':
                    depth += 1
                    started = True
                elif ch == '}':
                    depth -= 1
            if started and depth <= 0:
                out.append((i, j + 1, lines[i].strip()))
                break
    return out


def strip_noncode(line):
    """Remove line comments and string literals so brace counting is not fooled."""
    out = []
    in_str = False
    esc = False
    k = 0
    while k < len(line):
        c = line[k]
        if in_str:
            if esc:
                esc = False
            elif c == '\\':
                esc = True
            elif c == '"':
                in_str = False
        else:
            if c == '"':
                in_str = True
            elif c == '/' and k + 1 < len(line) and line[k + 1] == '/':
                break
            else:
                out.append(c)
        k += 1
    return ''.join(out)


# (id, regex, points, explanation). Points are a rough ordering, not a
# measurement — a finding's value is the explanation, not the number.
CHECKS = [
    ("push_in_loop", re.compile(r'\.push\('), 2,
     "grows a Vec inside the loop: trip count unknown and the buffer can "
     "realloc mid-loop. with_capacity + indexed writes, or extend()."),
    ("enumerate_crossindex", re.compile(r'\.iter\(\)\.enumerate\(\)'), 1,
     "enumerate() whose index reads a second slice forces the compiler to "
     "prove non-aliasing. zip() the slices instead and it is stated."),
    ("float_accum", re.compile(r'\b(sum|acc|total|dot)\s*\+=' ), 2,
     "single float accumulator: FP add is not associative so LLVM will not "
     "reorder it. Multiple partial sums (chunks_exact(N)) unlocks it — an "
     "explicit numerical choice, since the result can differ in the last bits."),
    ("early_exit", re.compile(r'^\s*(break|continue)\b|\?;'), 1,
     "early exit inside the body: the trip count is not fixed."),
    ("indexed_two_slices", re.compile(r'\w+\[\w+\].*\w+\[\w+\]'), 2,
     "two bounds-checked indexings in one statement: a branch per access plus "
     "an aliasing question. Slice to a common length, or chunks_exact."),
    ("nested_index", re.compile(r'\[\s*\w+\s*\*\s*\w+\s*\+'), 1,
     "manual row-major addressing: correct, but chunks_exact(row_len) gives "
     "the vectorizer the stride for free."),
    ("opaque_call", re.compile(r'\bself\.\w+\('), 1,
     "method call in the body — a barrier unless it inlines. #[inline] on the "
     "callee, or hoist it."),
]

# Loops that are obviously not hot. Skipping these is what keeps the output
# short enough to read, which is the difference between a tool someone uses and
# a tool someone silences.
SKIP_BODY = re.compile(
    r'eprintln!|println!|write!|writeln!|format!|\.to_string\(\)|panic!|assert')


def score_loop(body):
    hits = []
    total = 0
    for name, rx, pts, why in CHECKS:
        n = sum(1 for l in body if rx.search(strip_noncode(l)))
        if n:
            hits.append({"check": name, "count": n, "why": why})
            total += pts
    return total, hits


def audit_file(path):
    try:
        lines = open(path, encoding="utf-8", errors="ignore").read().split('\n')
    except OSError:
        return []
    findings = []
    for (a, b, header) in find_loops(lines):
        body = lines[a + 1:b - 1]
        if not (MIN_BODY <= len(body) <= MAX_BODY):
            continue
        if any(SKIP_BODY.search(l) for l in body):
            continue
        score, hits = score_loop(body)
        if score:
            findings.append({
                "file": path, "line": a + 1, "lines": len(body),
                "score": score, "header": header[:100], "hits": hits,
            })
    return findings


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument("paths", nargs="+")
    ap.add_argument("--min-score", type=int, default=4,
                    help="report loops at or above this score (default 4). "
                         "Chosen from the distribution on gpu-gemm: scores 1-2 "
                         "are 294 loops (unreadable), 3 adds 78 more that are "
                         "mostly single weak signals, and 4+ is 29 crate-wide "
                         "with only 4 in the actual CPU kernels — a list short "
                         "enough to work through.")
    ap.add_argument("--self-test", action="store_true",
                    help="check the detector against known-good and known-bad "
                         "loops and exit")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--top", type=int, default=25)
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    found = []
    for p in args.paths:
        if os.path.isfile(p):
            found += audit_file(p)
        else:
            for root, dirs, files in os.walk(p):
                dirs[:] = [d for d in dirs if d not in ("target", ".git")]
                for f in files:
                    if f.endswith(".rs"):
                        found += audit_file(os.path.join(root, f))

    found = [f for f in found if f["score"] >= args.min_score]
    found.sort(key=lambda f: (-f["score"], f["file"], f["line"]))

    if args.json:
        print(json.dumps(found, indent=2))
        return 0

    if not found:
        print("vectorize-audit: nothing at or above the threshold.")
        return 0

    print(f"vectorize-audit: {len(found)} loop(s) worth a look "
          f"(score >= {args.min_score}). ADVISORY — measure before changing "
          f"anything; this proves nothing.\n")
    for f in found[:args.top]:
        print(f"{f['file']}:{f['line']}  score {f['score']}  ({f['lines']} lines)")
        print(f"    {f['header']}")
        for h in f["hits"]:
            print(f"      [{h['check']} x{h['count']}] {h['why']}")
        print()
    if len(found) > args.top:
        print(f"... and {len(found) - args.top} more (--top to see them).")
    return 0


SELF_TEST_CASES = [
    # (name, source, must_flag)
    #
    # These pin the detector against the two ways it can be useless: silence on
    # a loop that genuinely cannot vectorize, and noise on one that plainly can.
    ("single_float_accumulator", """
fn f(a: &[f32], b: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    for i in 0..a.len() {
        sum += a[i] * b[i];
        sum += 1.0;
    }
    sum
}
""", True),
    ("push_in_body", """
fn f(a: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..a.len() {
        let v = a[i] ^ a[i];
        out.push(v);
        out.push(v);
    }
    out
}
""", True),
    # The clean counterpart: zipped, no accumulator, no push, no indexing. If
    # this is ever flagged the tool has become noise and should be distrusted.
    ("zipped_elementwise_is_clean", """
fn f(a: &[f32], b: &[f32], out: &mut [f32]) {
    for ((o, x), y) in out.iter_mut().zip(a).zip(b) {
        *o = x * y;
        *o += 1.0;
    }
}
""", False),
    # A loop that only prints is skipped regardless of shape — this is what
    # keeps the output short enough that anyone reads it.
    ("logging_loop_is_skipped", """
fn f(a: &[f32]) {
    let mut sum = 0.0f32;
    for i in 0..a.len() {
        sum += a[i];
        println!("{} {}", i, a[i]);
    }
}
""", False),
]


def self_test():
    import tempfile
    ok = True
    for name, src, must_flag in SELF_TEST_CASES:
        with tempfile.NamedTemporaryFile("w", suffix=".rs", delete=False) as fh:
            fh.write(src)
            path = fh.name
        findings = [f for f in audit_file(path) if f["score"] >= 3]
        os.unlink(path)
        got = bool(findings)
        status = "ok" if got == must_flag else "FAIL"
        if got != must_flag:
            ok = False
        want = "flagged" if must_flag else "clean"
        print(f"  [{status}] {name}: expected {want}, "
              f"got {'flagged' if got else 'clean'}"
              + (f" (score {findings[0]['score']})" if findings else ""))
    print("self-test:", "PASS" if ok else "FAIL")
    # Still exit 0 even on failure: this tool is advisory and must never be the
    # reason a build or a hook stops. A human reading FAIL is the signal.
    return 0


if __name__ == "__main__":
    sys.exit(main())
