#!/usr/bin/env python3
"""Reference proposer: bridge bot-arena's patch seam to a local llama.cpp model.

The engine speaks one protocol:  JSON context on stdin  →  unified diff on stdout.
This script implements that against llama.cpp, so you can run:

    bot-arena --headless -w ./repo --backend local \\
      --proposer-cmd "LLAMA_MODEL=qwen2.5-coder-1.5b.gguf python3 examples/proposer_llamacpp.py"

and, for the cascade, point --escalate-cmd at a bigger model (or an API wrapper that
emits the same thing). Same protocol at every tier.

Config via environment:
    LLAMA_BIN    llama.cpp CLI binary           (default: llama-cli)
    LLAMA_MODEL  path to a .gguf model          (required)
    LLAMA_NPRED  max tokens to generate         (default: 512)
    LLAMA_EXTRA  extra args passed to llama-cli (default: "-ngl 99 -c 8192 --temp 0.2")

It is deliberately defensive: any failure (no binary, bad model, no parseable diff)
prints NOTHING, which the engine reads as "no change" — so a flaky cheap model just
falls through to the next cascade tier instead of crashing the run.

To "laser" the cheap tier later: train a LoRA on experience.jsonl (the wins this very
system harvests), merge it, and point LLAMA_MODEL at the result. No engine change, no
protocol change — you swap one file path.
"""
import os, sys, json, subprocess, re

def build_prompt(ctx):
    files = ctx.get("files", [])
    failing = ctx.get("failing", [])
    plan = ctx.get("plan", "")  # optional: injected by a higher tier's planner
    test_cmd = ctx.get("test_cmd", "")

    parts = []
    parts.append(
        "You are a precise code-repair tool. You are given a project's source files "
        "and its currently failing tests. Output ONLY a unified diff (git-apply "
        "compatible) that makes the failing tests pass.\n"
        "Rules: do NOT edit test files. Do NOT explain. No markdown, no prose. "
        "Begin directly with '--- a/<path>'. Keep the diff minimal.\n"
    )
    if plan:
        parts.append(f"\nHigh-level plan from the lead model (follow it):\n{plan}\n")
    knowledge = ctx.get("knowledge", [])
    if knowledge:
        parts.append("\nReference knowledge (retrieved; reason over it, don't just copy):\n")
        for k in knowledge:
            parts.append(f"---\n{k}\n")
    if test_cmd:
        parts.append(f"\nTest command: {test_cmd}")
    if failing:
        parts.append("\nFailing tests:\n" + "\n".join(f"  - {f}" for f in failing))
    parts.append("\n\nSource files:")
    budget = 12000  # keep the prompt small; cheap models have small contexts
    for f in files:
        path, content = f.get("path", "?"), f.get("content", "")
        if path.endswith((".lock",)) or "/target/" in path:
            continue
        if budget <= 0:
            break
        snippet = content[:budget]
        budget -= len(snippet)
        parts.append(f"\n=== {path} ===\n{snippet}")
    parts.append("\n\nUnified diff:\n")
    return "".join(parts)

def extract_diff(text):
    """Pull a unified diff out of model output: strip fences/prose, keep from the
    first '--- ' or 'diff --git' line onward."""
    if not text:
        return ""
    # fenced ```diff ... ``` or ``` ... ```
    m = re.search(r"```(?:diff|patch)?\s*\n(.*?)```", text, re.DOTALL)
    if m:
        text = m.group(1)
    # take from the first diff marker to the end
    lines = text.splitlines()
    start = next((i for i, l in enumerate(lines)
                  if l.startswith("--- ") or l.startswith("diff --git")), None)
    if start is None:
        return ""
    diff = "\n".join(lines[start:]).rstrip()
    return diff + "\n" if diff else ""

def main():
    try:
        ctx = json.load(sys.stdin)
    except Exception:
        return  # unparseable context → emit nothing
    model = os.environ.get("LLAMA_MODEL")
    if not model:
        sys.stderr.write("proposer_llamacpp: set LLAMA_MODEL=path/to/model.gguf\n")
        return
    binary = os.environ.get("LLAMA_BIN", "llama-cli")
    npred = os.environ.get("LLAMA_NPRED", "512")
    extra = os.environ.get("LLAMA_EXTRA", "-ngl 99 -c 8192 --temp 0.2").split()
    prompt = build_prompt(ctx)

    cmd = [binary, "-m", model, "-n", npred, "-p", prompt, "--no-display-prompt", *extra]
    try:
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    except FileNotFoundError:
        sys.stderr.write(f"proposer_llamacpp: '{binary}' not found on PATH\n")
        return
    except subprocess.TimeoutExpired:
        return
    diff = extract_diff(out.stdout)
    if diff:
        sys.stdout.write(diff)

if __name__ == "__main__":
    main()
