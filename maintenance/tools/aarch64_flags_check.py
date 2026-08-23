#!/usr/bin/env python3
"""aarch64_flags_check.py — no committed build flag may promise a CPU feature
the fleet's weakest aarch64 target does not have.

WHY

`gemm-f16` (via candle) emits `fmla v0.8h` and will not assemble for
`aarch64-unknown-linux-gnu`, whose baseline enables only `neon`. The obvious fix
is a repo `.cargo/config.toml`:

    [target.aarch64-unknown-linux-gnu]
    rustflags = ["-C", "target-feature=+fp16"]

That is correct on the Asahi M2 machines and WRONG for this tree, which also
targets the PinePhone — Cortex-A53, ARMv8.0-A, which has FP16 conversion but
not FP16 arithmetic. A binary built with `+fp16` runs there until it reaches one
of those instructions and then takes SIGILL.

So the trade a committed flag makes is: turn a loud build failure on one machine
into a runtime crash on another. This refuses that trade, because the flag is
easy to add in a hurry when a build is red and the consequence appears on
different hardware weeks later.

Per-machine is the right home — `~/.cargo/config.toml`, or RUSTFLAGS in the
invocation. See maintenance/ASAHI_VULKAN_LESSONS.md §6.
"""
import pathlib, re, sys

# Features and target-cpus that assume more than ARMv8.0-A baseline.
# `fp16` is rustc's name for LLVM's `fullfp16` — the error text says the
# latter, which is why copying from the error gives an unknown-feature warning.
UNSAFE_FEATURES = ["fp16", "fhm", "dotprod", "i8mm", "bf16", "sve", "sve2", "lse128"]
UNSAFE_CPUS = ["apple-", "native", "cortex-a7", "cortex-a71", "neoverse"]

AARCH64_SECTION = re.compile(r'\[target\.[^\]]*aarch64[^\]]*\]')


def offending(text: str):
    """Flags under an aarch64 target section that over-promise the CPU."""
    out = []
    # Only sections that name aarch64; an x86 or host-wide section is not our
    # business, and `[build] rustflags` applies to the host build too.
    for m in AARCH64_SECTION.finditer(text):
        start = m.end()
        nxt = text.find("\n[", start)
        body = text[start: nxt if nxt != -1 else len(text)]
        for feat in UNSAFE_FEATURES:
            if re.search(r'target-feature\s*=?\s*["\s,]*\+' + re.escape(feat) + r'\b', body) \
               or f"+{feat}" in body:
                out.append(f"target-feature=+{feat}")
        for cpu in UNSAFE_CPUS:
            if re.search(r'target-cpu\s*=?\s*["\s,]*' + re.escape(cpu), body):
                out.append(f"target-cpu={cpu}…")
    return out


def main() -> int:
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "/home/claude/repo")
    problems = []
    scanned = 0
    for p in sorted(root.rglob("config.toml")):
        if ".cargo" not in p.parts or "target" in p.parts or "vendor" in p.parts:
            continue
        scanned += 1
        try:
            hits = offending(p.read_text(errors="replace"))
        except OSError:
            continue
        for h in hits:
            problems.append(f"{p.relative_to(root)}: {h}")

    print(f"scanned {scanned} committed .cargo/config.toml file(s)")
    for msg in problems:
        print(f"  {msg}")
    if problems:
        print("\nThese promise a CPU feature the PinePhone (Cortex-A53, ARMv8.0-A) does not")
        print("have. A binary built this way runs there until it hits the instruction, then")
        print("SIGILLs. Set it per machine instead — see ASAHI_VULKAN_LESSONS.md §6.")
        return 1
    if scanned:
        print("no committed aarch64 flag over-promises the baseline")
    return 0


if __name__ == "__main__":
    sys.exit(main())
