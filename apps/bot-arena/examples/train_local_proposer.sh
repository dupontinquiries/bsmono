#!/bin/sh
# train_local_proposer.sh — distill harvested wins into a local tinygpt proposer.
#
# Closes the loop: experience.jsonl (winning patches the arena harvested while a strong
# model proposed) -> a small char-level transformer you can run as the fast tier-0
# proposer, with the strong model kept as the escalation tier.
#
#   examples/train_local_proposer.sh <experience.jsonl> <out_dir> [steps]
#
# Produces <out_dir>/tok.bin and <out_dir>/model.bin and prints the exact
# --proposer-cmd to paste into bot-arena.
#
# The hyperparameters below are the recipe that gave clean byte-exact diff
# reproduction in testing: a (near) char-level tokenizer — BPE merges cause token
# doubling on exact sequences — and a context window large enough to hold a whole
# fix-block. Bump <steps> for a larger harvest.
set -e

EXP="$1"; OUT="$2"; STEPS="${3:-1000}"
if [ -z "$EXP" ] || [ -z "$OUT" ]; then
  echo "usage: $0 <experience.jsonl> <out_dir> [steps]" >&2
  exit 1
fi
[ -f "$EXP" ] || { echo "no experience log at $EXP" >&2; exit 1; }

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
PIPE="$ROOT/pipeline"
BIN="${XDG_DATA_HOME:-$HOME/.local/share}/codebot-arena/bin"

mkdir -p "$OUT"

echo ">> building pipeline tools (bpe_train, tiny_gpt) ..."
make -C "$PIPE" >/dev/null 2>&1 || make -C "$PIPE" "$BIN/bpe_train" "$BIN/tiny_gpt" >/dev/null
[ -x "$BIN/tiny_gpt" ] || { echo "tiny_gpt not built (check $PIPE)" >&2; exit 1; }

echo ">> building training corpus from $EXP ..."
python3 "$SCRIPT_DIR/experience_to_corpus.py" "$EXP" "$OUT/corpus" 60

echo ">> training tokenizer (near char-level) ..."
CORPUS_EXT=.md "$BIN/bpe_train" "$OUT/corpus" "$OUT/tok.bin" 2 >/dev/null

echo ">> training tinygpt proposer ($STEPS steps) ..."
CORPUS_EXT=.md "$BIN/tiny_gpt" train "$OUT/corpus" "$OUT/tok.bin" "$OUT/model.bin" \
  "$STEPS" 48 2 4 160 4 1.5e-3 2>&1 | tail -1

cat <<EOF

done. local tinygpt proposer ready.

Use it as the fast tier-0 proposer (keep your strong model as the escalation tier):

  bot-arena --headless -w ./repo --backend local \\
    --proposer-cmd "TINYGPT_BIN=$BIN/tiny_gpt TINYGPT_TOK=$OUT/tok.bin TINYGPT_MODEL=$OUT/model.bin python3 $SCRIPT_DIR/proposer_tinygpt.py" \\
    --escalate-cmd "LLAMA_MODEL=/path/to/model.gguf python3 $SCRIPT_DIR/proposer_llamacpp.py"

tinygpt recalls fixes for failures whose test names it has seen; novel failures fail
the git-apply gate and escalate. Re-run this script as experience.jsonl grows to keep
sharpening the cheap tier on your own recurring work.
EOF
