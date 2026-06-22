#!/usr/bin/env python3
"""experience_to_corpus.py — turn harvested wins into a tinygpt training corpus.

The arena writes every winning (context → patch) to experience.jsonl. This converts
those into the FIX-block format the local tinygpt proposer is trained on and reads
back at inference, closing the distillation loop:

    arena runs (llama.cpp proposes) → experience.jsonl
        → experience_to_corpus.py → corpus/
        → bpe_train + tiny_gpt train → model
        → proposer_tinygpt.py (fast tier-0, with llama.cpp as escalation)

Block format (header is reconstructable from live context at inference):

    <<<FIX failing=test_a,test_b>>>
    <unified diff>
    <<<END>>>

Usage:
    python3 experience_to_corpus.py <experience.jsonl> <out_dir> [repeat]

`repeat` (default 1) duplicates the corpus to encourage memorization of a small,
narrow set of recurring fixes — appropriate when the distribution is your own repos.
"""
import os, sys, json

def block(rec):
    failing = ",".join(sorted(rec.get("failing_before", [])))
    diff = rec.get("diff", "").rstrip("\n")
    return f"<<<FIX failing={failing}>>>\n{diff}\n<<<END>>>\n"

def main():
    if len(sys.argv) < 3:
        sys.stderr.write("usage: experience_to_corpus.py <experience.jsonl> <out_dir> [repeat]\n")
        sys.exit(1)
    src, out_dir = sys.argv[1], sys.argv[2]
    repeat = int(sys.argv[3]) if len(sys.argv) > 3 else 1
    os.makedirs(out_dir, exist_ok=True)
    blocks = []
    for line in open(src):
        line = line.strip()
        if not line:
            continue
        try:
            rec = json.loads(line)
        except Exception:
            continue
        if rec.get("diff"):
            blocks.append(block(rec))
    if not blocks:
        sys.stderr.write("no usable records in experience log\n")
        sys.exit(1)
    text = "".join(blocks) * repeat
    # tiny_gpt/bpe_train read files matching CORPUS_EXT (default .md)
    with open(os.path.join(out_dir, "corpus.md"), "w") as f:
        f.write(text)
    sys.stderr.write(f"wrote {len(blocks)} fix-blocks ×{repeat} ({len(text)} chars) → {out_dir}/corpus.md\n")

if __name__ == "__main__":
    main()
