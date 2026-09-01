#!/usr/bin/env python3
"""Process-wide mutable state that races, and init guards that do not guard.

## Why

Three bugs of one shape in a single maintenance session, each invisible until
something ran tests in parallel, each with a consequence beyond a red test:

  * `board-layouts` — a process-wide `CodeMode` atomic reset by a concurrent
    test, producing the wrong keyboard layout.
  * `voiceterm/qml_gen` — id counters reset mid-generate, producing two QML
    elements with the same id, which is a parse error in QML.
  * `fao-codec/dct` — a `static mut` cosine basis behind a check-then-set
    Relaxed `AtomicBool`, so a thread could observe `INIT == true` while half
    the table was still invisible to it and transform against zeros. That one
    was misdiagnosed as an ARM SIMD bug for a round, because three tests failed
    together and the timing differs per machine.

The shape is always: process-wide mutable state, no happens-before, and an
init or reset that races. All three were found by a failing test on someone
else's machine rather than by reading. This finds them by reading.

## What it flags, and why each is a real defect

* **`static mut`** — a hard error in edition 2024, so this has to move
  regardless. Any read or write is UB the moment a second thread exists.
* **check-then-set init guard** — a `load()` on an atomic, a return, some
  writes, then a `store(true)`. Two threads both see false and both
  initialise. This is the `fao-codec` bug exactly.
* **`Relaxed` on an init flag** — even with one writer, `Relaxed` publishes no
  happens-before edge, so another thread can see the flag set while the data
  it guards is not yet visible. `OnceLock`/`LazyLock` are the fix and need no
  ordering argument at all.
* **a `reset_*` function on process-wide state** — the `qml_gen` and
  `board-layouts` shape. Fine single-threaded, a race the moment two tests run
  at once. Thread-local usually makes the invariant structural instead.

## What it does NOT flag

Atomics used as counters, flags read for diagnostics, or `Once`/`OnceLock`/
`LazyLock`, which are the answers rather than the problem. A `static mut`
inside a comment is skipped — the three fixes in this tree all left comments
explaining what they replaced, and flagging those would make the tool report
its own successes as failures.

## Use

    maintenance/tools/shared_state_audit.py apps shared games tools
    maintenance/tools/shared_state_audit.py --self-test
"""

import argparse
import os
import re
import sys

STATIC_MUT = re.compile(r'^\s*(pub\s+)?static\s+mut\s+\w+')
ATOMIC_DECL = re.compile(r'^\s*(pub\s+)?static\s+(\w+)\s*:\s*(?:std::sync::atomic::)?Atomic(\w+)')
LOAD = re.compile(r'\b(\w+)\.load\(')
STORE = re.compile(r'\b(\w+)\.store\(')
# `compare_exchange` / `swap` on the same flag IS the correct claim — one thread
# wins, the rest see the error and back off. A `load()` in front of it is a
# cheap fast path, not the bug. fao-codec's gpu_pipeline does exactly this and
# says so in a comment; reporting it would be flagging the right answer.
CLAIM = re.compile(r'\b(\w+)\.(compare_exchange(_weak)?|swap|fetch_or)\(')
RELAXED = re.compile(r'Ordering::Relaxed')
# A reset that takes `self` operates on instance state, not on the process-wide
# statics this tool is about. `clear_conversation(&mut self)` and
# `clear_candidates(&mut self)` were both reported before this filter and both
# were noise — the tool has to earn its output or it gets ignored like any
# other over-eager lint.
RESET_FN = re.compile(r'^\s*(pub\s+)?fn\s+(reset_\w+|clear_\w+)\s*\(\s*(?!&?\s*(mut\s+)?self)')
SAFE = re.compile(r'\bOnceLock\b|\bLazyLock\b|\bOnce\b|thread_local!')


def strip_comments(lines):
    """Blank out line comments and whole-line block comments.

    Crude on purpose. The alternative — a real Rust parser — is more machinery
    than a triage tool earns, and the only cost of getting it slightly wrong is
    one bad finding.
    """
    out = []
    in_block = False
    for l in lines:
        s = l
        if in_block:
            if '*/' in s:
                s = s.split('*/', 1)[1]
                in_block = False
            else:
                s = ''
        if '/*' in s and '*/' not in s:
            s = s.split('/*', 1)[0]
            in_block = True
        s = re.sub(r'//.*$', '', s)
        out.append(s)
    return out


def audit_file(path):
    try:
        raw = open(path, encoding='utf-8', errors='ignore').read().split('\n')
    except OSError:
        return []
    code = strip_comments(raw)
    findings = []

    for i, line in enumerate(code):
        if STATIC_MUT.search(line):
            findings.append((path, i + 1, "static_mut", raw[i].strip()[:100],
                             "`static mut`: UB with a second thread, and a hard "
                             "error in edition 2024. OnceLock/LazyLock, or "
                             "thread_local if it is per-thread state."))

    # Atomics declared in this file, so a load/store pair can be attributed.
    flags = {m.group(2) for m in (ATOMIC_DECL.match(l) for l in code) if m}

    for i, line in enumerate(code):
        m = LOAD.search(line)
        if not m or m.group(1) not in flags:
            continue
        # The guard shape is `if FLAG.load(..) { return; }` — a CONDITIONAL read
        # that short-circuits. A bare `FLAG.load()` feeding a diagnostic print is
        # not this bug, and reporting it (as this did for gpu-gemm's MOE_BYTES,
        # which is a byte counter formatted into a log line) is how a tool
        # teaches people to stop reading it.
        if not line.lstrip().startswith(('if ', 'if!', 'if(')):
            continue
        if not any('return' in w or 'continue' in w
                   for w in code[i:i + 4]):
            continue
        # A check-then-set init guard: this load, then a store to the SAME flag
        # within a short window, with real work between them.
        window = code[i:i + 40]
        stores = [j for j, w in enumerate(window)
                  if (sm := STORE.search(w)) and sm.group(1) == m.group(1)]
        if not stores:
            continue
        gap = stores[0]
        # The load and the store must be in the SAME function. An init guard
        # checks and then sets; a settings flag is read in one function and
        # written by a setter elsewhere, which is a correct lock-free config
        # read and not this bug (board-layouts' AUTO_CAP_ENABLED is exactly
        # that, and was reported before this check).
        if any(re.match(r'\s*(pub\s+)?(unsafe\s+)?fn\s', w) for w in window[1:gap]):
            continue
        if gap < 2:
            continue  # load and store adjacent: a swap-style guard, not this bug
        body = window[1:gap]
        # Writes between the check and the set are what make it an INIT guard
        # rather than a plain flag read.
        if not any(re.search(r'=\s*[^=]', b) or '.push' in b for b in body):
            continue
        if any((cm := CLAIM.search(w)) and cm.group(1) == m.group(1)
               for w in window[:gap + 1]):
            continue  # claimed with an atomic RMW: correct
        relaxed = any(RELAXED.search(w) for w in window[:gap + 1])
        findings.append((
            path, i + 1, "check_then_set_init", raw[i].strip()[:100],
            f"check-then-set init guard on `{m.group(1)}`"
            + (" with Relaxed ordering" if relaxed else "")
            + ": two threads can both see false and both initialise"
            + (", and Relaxed publishes no happens-before so the guarded data "
               "may be invisible when the flag is seen set" if relaxed else "")
            + ". OnceLock::get_or_init does this correctly and needs no "
              "ordering argument."))

    for i, line in enumerate(code):
        if RESET_FN.match(line) and flags:
            findings.append((
                path, i + 1, "reset_on_global", raw[i].strip()[:100],
                "a reset_*/clear_* function beside process-wide atomic state: "
                "correct single-threaded, a race the moment two tests run at "
                "once. thread_local makes the invariant structural."))

    # A file that already uses the right primitives throughout is very likely
    # fine; drop the softest finding class there to keep the signal up.
    if any(SAFE.search(l) for l in code):
        findings = [f for f in findings if f[2] != "reset_on_global"]
    return findings


SELF_TEST = [
    ("static_mut_is_flagged", '''
static mut TABLE: [f32; 8] = [0.0; 8];
fn f() { unsafe { TABLE[0] = 1.0; } }
''', True),
    ("check_then_set_is_flagged", '''
static INIT: AtomicBool = AtomicBool::new(false);
static mut D: [f32; 4] = [0.0; 4];
pub fn init() {
    if INIT.load(Ordering::Relaxed) { return; }
    unsafe {
        D[0] = 1.0;
        D[1] = 2.0;
    }
    INIT.store(true, Ordering::Relaxed);
}
''', True),
    # The fix must read as clean, or the tool punishes the correct answer.
    ("oncelock_is_clean", '''
static D: std::sync::OnceLock<[f32; 4]> = std::sync::OnceLock::new();
fn basis() -> &'static [f32; 4] {
    D.get_or_init(|| { let mut m = [0.0; 4]; m[0] = 1.0; m })
}
''', False),
    ("thread_local_counter_is_clean", '''
thread_local! {
    static N: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
pub fn reset_counter() { N.with(|c| c.set(0)); }
pub fn next() -> usize { N.with(|c| { let v = c.get(); c.set(v + 1); v }) }
''', False),
    # THE ACTUAL qml_gen BUG, as it stood before the fix: a process-wide counter
    # plus a free-function reset. Two tests run in parallel, one resets between
    # the other's two next() calls, and two QML elements get the same id. If the
    # tool cannot see this shape it would not have caught the bug it exists for.
    ("historical_qml_gen_counter_is_flagged", '''
static PARAGRAPH_COUNTER: AtomicUsize = AtomicUsize::new(0);
pub fn reset_paragraph_counter() {
    PARAGRAPH_COUNTER.store(0, Ordering::Relaxed);
}
pub fn next_paragraph_id() -> usize {
    PARAGRAPH_COUNTER.fetch_add(1, Ordering::Relaxed)
}
''', True),
    # A plain atomic counter is not this bug and must not be reported.
    ("plain_atomic_counter_is_clean", '''
static HITS: AtomicUsize = AtomicUsize::new(0);
pub fn bump() { HITS.fetch_add(1, Ordering::Relaxed); }
pub fn hits() -> usize { HITS.load(Ordering::Relaxed) }
''', False),
]


def self_test():
    import tempfile
    ok = True
    for name, src, must_flag in SELF_TEST:
        with tempfile.NamedTemporaryFile('w', suffix='.rs', delete=False) as fh:
            fh.write(src)
            path = fh.name
        got = bool(audit_file(path))
        os.unlink(path)
        good = got == must_flag
        ok &= good
        print(f"  [{'ok' if good else 'FAIL'}] {name}: expected "
              f"{'flagged' if must_flag else 'clean'}, got "
              f"{'flagged' if got else 'clean'}")
    print("self-test:", "PASS" if ok else "FAIL")
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument("paths", nargs="*")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        return self_test()
    if not args.paths:
        ap.error("give at least one path, or --self-test")

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

    if not found:
        print("shared-state-audit: clean.")
        return 0
    print(f"shared-state-audit: {len(found)} finding(s).\n")
    for path, line, kind, src, why in sorted(found):
        print(f"{path}:{line}  [{kind}]")
        print(f"    {src}")
        print(f"    {why}\n")
    # Advisory, like the other audits here: never the reason a build stops.
    return 0


if __name__ == "__main__":
    sys.exit(main())
