#!/usr/bin/env python3
"""voiceprint_parity_check.py — voiceterm and voiceprint-kit must compute the
same fingerprint, because nothing else guarantees it.

WHY THIS EXISTS

`shared/voiceprint-kit` arrived (the c9-006g merge, C12) as a straight copy of
`apps/voiceterm/src/acoustic_index.rs::fingerprint` and
`apps/voiceterm/src/phonetic.rs::compute_mfcc`, `pub(crate)` widened to `pub`
so studio could reach it. Studio compares a bounce against the SAME
fingerprint voiceterm computed for a bundled voice line — that identity is
the entire point of studio depending on this crate at all. Two independent
copies of the algorithm is how that silently stops being true: someone
tunes voiceterm's MFCC windowing for a bug report, does not know
voiceprint-kit exists, and every future fingerprint comparison across the two
apps keeps running and keeps returning numbers — just wrong ones. The
failure mode is "the match rate got worse," not a compile error or a panic,
which is why this needs a checker rather than a code review to catch: a
diligent reviewer of the voiceterm change has no reason to even look at
shared/.

The real fix (recorded as C12-2a in maintenance/C12_TODO.md) is voiceterm
depending on the kit instead of carrying its own copy, which touches
voiceterm's AI01->AI02 bundle format and is not a small change. This is the
cheap guard until that happens: fail loudly if the two copies drift, or if
either function goes missing (renamed, moved, deleted) — a checker that
finds nothing to compare must not silently pass, same principle CLAUDE.md
§5b states for a test.

WHAT COUNTS AS "THE SAME"

Not byte-identical: voiceterm's `fingerprint` calls `compute_mfcc` and
`N_MFCC` through `crate::phonetic::`-qualified paths because it lives in a
different module; voiceprint-kit's calls them unqualified because both live
in one file. That is an artifact of WHERE the code was pasted, not a
behavioural difference, so the qualified-path prefix is stripped before
comparing. Comments and blank lines are also stripped — a doc comment
rephrase is not drift. Everything else — every operator, every constant,
every line of actual logic — must match exactly.

USAGE

    python3 maintenance/tools/voiceprint_parity_check.py [repo_root]

Exit 0 and a one-line PASS when both function pairs match. Exit 1 with a
diff and a named cause otherwise: MISSING (a function could not be found —
treated as a hard failure, not a skip) or DRIFTED (found both, bodies
differ after normalisation).
"""
import difflib
import pathlib
import re
import sys

ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "/home/claude/repo").resolve()

# (label, file_a, sig_a, file_b, sig_b)
PAIRS = [
    (
        "fingerprint",
        "apps/voiceterm/src/acoustic_index.rs",
        "fn fingerprint(audio_16k_mono: &[f32]) -> Vec<f32> {",
        "shared/voiceprint-kit/src/lib.rs",
        "fn fingerprint(audio_16k_mono: &[f32]) -> Vec<f32> {",
    ),
    (
        "compute_mfcc",
        "apps/voiceterm/src/phonetic.rs",
        "fn compute_mfcc(audio: &[f32]) -> Vec<f32> {",
        "shared/voiceprint-kit/src/lib.rs",
        "fn compute_mfcc(audio: &[f32]) -> Vec<f32> {",
    ),
]

# The one structural difference the merge itself introduces: voiceterm's
# fingerprint reaches into its own phonetic module by qualified path;
# voiceprint-kit's does not need to, since both functions live in the same
# file there. Stripping this is the difference between a real parity
# checker and one that permanently reports drift on a file layout artifact.
QUALIFIED_PATH = re.compile(r"\bcrate::phonetic::")


def extract_body(path: pathlib.Path, sig: str):
    """Return the brace-balanced body text starting at `sig`, or None if
    `sig` does not appear in the file at all — a rename or deletion, which
    this checker must treat as a failure, not as "nothing to check"."""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as e:
        return None, f"cannot read {path}: {e}"
    i = text.find(sig)
    if i < 0:
        return None, f"signature not found: {sig!r}"
    brace = text.find("{", i)
    if brace < 0:
        return None, f"no opening brace after signature: {sig!r}"
    depth = 0
    j = brace
    while j < len(text):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[brace:j + 1], None
        j += 1
    return None, f"unbalanced braces after signature: {sig!r}"


def normalize(body: str) -> list:
    body = QUALIFIED_PATH.sub("", body)
    lines = []
    for line in body.split("\n"):
        line = line.strip()
        if not line or line.startswith("//"):
            continue
        lines.append(line)
    return lines


def main():
    failures = []
    for label, fa, sa, fb, sb in PAIRS:
        pa, pb = ROOT / fa, ROOT / fb
        body_a, err_a = extract_body(pa, sa)
        body_b, err_b = extract_body(pb, sb)
        if err_a or err_b:
            failures.append((label, "MISSING", [
                f"  {fa}: {err_a or 'ok'}",
                f"  {fb}: {err_b or 'ok'}",
            ]))
            continue
        norm_a, norm_b = normalize(body_a), normalize(body_b)
        # A normalisation bug that empties both sides must not read as a
        # pass — same shape as lock_check's containment fix and the
        # blackstar clearance-scan fix earlier this session: a check that
        # can match nothing must not silently agree.
        if not norm_a or not norm_b:
            failures.append((label, "EMPTY-AFTER-NORMALIZE", [
                f"  {fa} produced {len(norm_a)} lines, {fb} produced {len(norm_b)} lines",
            ]))
            continue
        if norm_a != norm_b:
            diff = list(difflib.unified_diff(
                norm_a, norm_b, fromfile=fa, tofile=fb, lineterm=""))
            failures.append((label, "DRIFTED", diff))

    if not failures:
        print(f"voiceprint_parity_check: PASS — {len(PAIRS)} function pair(s) "
              f"match (fingerprint, compute_mfcc)")
        return 0

    print(f"voiceprint_parity_check: FAIL — {len(failures)} of {len(PAIRS)} "
          f"pair(s) failed\n")
    for label, cause, detail in failures:
        print(f"[{cause}] {label}")
        for line in detail:
            print(line)
        print()
    print("voiceterm and shared/voiceprint-kit compute the fingerprint\n"
          "independently (see this file's module docstring). One of them\n"
          "changed and the other did not, or a function moved/renamed.\n"
          "Cross-app fingerprint comparison is silently wrong until these\n"
          "agree again.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
