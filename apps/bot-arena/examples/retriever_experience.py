#!/usr/bin/env python3
"""RAG retriever — a research bot for the context chain.

It reads the proposer context on stdin and writes back an *enriched* context with a
`knowledge` field: relevant facts pulled from grounded sources so a small model can
reason over supplied knowledge instead of leaning on thin baked-in memory.

    bot-arena --headless -w ./repo --backend local \\
      --context-cmd "python3 examples/retriever_experience.py" \\
      --proposer-cmd "LLAMA_MODEL=qwen2.5-coder-1.5b.gguf python3 examples/proposer_llamacpp.py"

Sources (all local, all grounded — no hallucinated "knowledge"):
  1. experience.jsonl — past patches that scored well, ranked by overlap with the
     current failing tests / touched files. "We fixed something like this before."
  2. the repo itself — definitions of symbols referenced near the failing code, so the
     model sees the signatures it must respect rather than guessing them.

Protocol: context JSON in → context JSON out (with `knowledge` added). On any problem
it prints the input unchanged, so it can never break the run. Retrieval precision
matters: injecting irrelevant facts pollutes a small model's context, so this stays
conservative (a few high-overlap hits, capped).
"""
import os, sys, json, re

EXP_PATHS = [
    os.environ.get("EXPERIENCE_LOG", ""),
    os.path.expanduser("~/.local/share/codebot-arena/experience.jsonl"),
    os.path.join(os.environ.get("XDG_DATA_HOME", ""), "codebot-arena/experience.jsonl")
        if os.environ.get("XDG_DATA_HOME") else "",
]
MAX_PRIOR = 3          # at most N prior-fix snippets
MAX_KNOWLEDGE = 6      # total knowledge items

def tokens(s):
    return set(re.findall(r"[A-Za-z_][A-Za-z0-9_]{2,}", s or ""))

def load_experience():
    for p in EXP_PATHS:
        if p and os.path.exists(p):
            recs = []
            for line in open(p):
                line = line.strip()
                if not line:
                    continue
                try:
                    recs.append(json.loads(line))
                except Exception:
                    pass
            return recs
    return []

def retrieve_prior_fixes(ctx):
    """Rank past winning patches by token overlap with the current failing tests +
    file contents; return the most relevant diffs."""
    recs = load_experience()
    if not recs:
        return []
    cur = set()
    for f in ctx.get("failing", []):
        cur |= tokens(f)
    for fl in ctx.get("files", []):
        cur |= tokens(fl.get("content", ""))
    scored = []
    for r in recs:
        sig = set()
        for f in r.get("failing_before", []):
            sig |= tokens(f)
        sig |= tokens(r.get("diff", ""))
        overlap = len(cur & sig)
        if overlap > 0:
            scored.append((overlap, r))
    scored.sort(key=lambda x: -x[0])
    out = []
    seen = set()
    for _, r in scored[:MAX_PRIOR * 3]:
        d = r.get("diff", "").strip()
        if d and d not in seen:
            seen.add(d)
            out.append(f"A previously-successful fix for a similar failure:\n{d}")
        if len(out) >= MAX_PRIOR:
            break
    return out

def retrieve_symbol_defs(ctx):
    """For symbols referenced in the failing tests, surface their definition lines from
    the repo so the model respects the real signatures."""
    files = {f.get("path", ""): f.get("content", "") for f in ctx.get("files", [])}
    # crude: collect identifiers appearing in #[cfg(test)] / test regions, then find defs
    wanted = set()
    for content in files.values():
        for m in re.finditer(r"\b([a-z_][a-z0-9_]{2,})\s*\(", content):
            wanted.add(m.group(1))
    defs = []
    defpat = re.compile(r"^\s*(pub\s+)?(fn|struct|enum|trait|def|class|function)\s+([A-Za-z_][A-Za-z0-9_]*)")
    for path, content in files.items():
        for line in content.splitlines():
            m = defpat.match(line)
            if m and m.group(3) in wanted:
                defs.append(f"{path}: {line.strip()}")
            if len(defs) >= MAX_KNOWLEDGE:
                break
    return defs[:MAX_KNOWLEDGE]

def main():
    try:
        ctx = json.load(sys.stdin)
    except Exception:
        return  # can't parse → emit nothing → engine keeps original context
    knowledge = list(ctx.get("knowledge", []))
    knowledge += retrieve_prior_fixes(ctx)
    defs = retrieve_symbol_defs(ctx)
    if defs:
        knowledge.append("Relevant definitions in this repo:\n" + "\n".join(defs))
    ctx["knowledge"] = knowledge[:MAX_KNOWLEDGE]
    sys.stdout.write(json.dumps(ctx))

if __name__ == "__main__":
    main()
