#!/usr/bin/env python3
"""proposer_tinygpt.py — the locally-trained tinygpt as a fast tier-0 proposer.

Same protocol as proposer_llamacpp.py (context JSON in → unified diff out), so the two
are interchangeable cascade tiers:

    bot-arena --headless -w ./repo --backend local \\
      --proposer-cmd "TINYGPT_MODEL=m.bin TINYGPT_TOK=tok.bin python3 examples/proposer_tinygpt.py" \\
      --escalate-cmd "LLAMA_MODEL=qwen.gguf python3 examples/proposer_llamacpp.py"

tinygpt is tiny and trained only on harvested fixes, so it shines on *recurring*
patterns and produces near-miss output on novel ones. That is fine by design: its raw
output goes through the engine's `git apply --check` gate, so a malformed patch is a
no-op that simply escalates to llama.cpp. tinygpt costs almost nothing to run, so trying
it first is free insurance.

Config via environment:
    TINYGPT_BIN    tiny_gpt binary               (default: tiny_gpt)
    TINYGPT_TOK    BPE tokenizer (bpe_train out) (required)
    TINYGPT_MODEL  trained checkpoint            (required)
    TINYGPT_NTOK   tokens to generate            (default: 220)
    TINYGPT_TEMP   sampling temperature          (default: 0.1)
"""
import os, sys, json, subprocess, re

def header(ctx):
    failing = ",".join(sorted(ctx.get("failing", [])))
    return f"<<<FIX failing={failing}>>>\n"

def extract_diff(text, prompt):
    # drop the primed prompt, then collect the diff up to any marker-like line.
    body = text
    i = body.find(prompt)
    if i != -1:
        body = body[i + len(prompt):]
    lines = body.splitlines()
    start = next((k for k, l in enumerate(lines)
                  if l.startswith("--- ") or l.startswith("diff --git")), None)
    if start is None:
        return ""
    out = []
    for l in lines[start:]:
        if l.lstrip().startswith("<<<") or l.lstrip().startswith(">>>"):
            break  # END marker (possibly malformed like <<<<END>>>>) — stop
        out.append(l)
    diff = "\n".join(out).rstrip()
    return diff + "\n" if diff else ""

def main():
    try:
        ctx = json.load(sys.stdin)
    except Exception:
        return
    tok, model = os.environ.get("TINYGPT_TOK"), os.environ.get("TINYGPT_MODEL")
    if not tok or not model:
        sys.stderr.write("proposer_tinygpt: set TINYGPT_TOK and TINYGPT_MODEL\n")
        return
    binary = os.environ.get("TINYGPT_BIN", "tiny_gpt")
    ntok = os.environ.get("TINYGPT_NTOK", "220")
    temp = os.environ.get("TINYGPT_TEMP", "0.1")
    prompt = header(ctx)
    try:
        out = subprocess.run([binary, "generate", tok, model, prompt, ntok, temp],
                             capture_output=True, text=True, timeout=60)
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return
    diff = extract_diff(out.stdout, prompt)
    if diff:
        sys.stdout.write(diff)

if __name__ == "__main__":
    main()
