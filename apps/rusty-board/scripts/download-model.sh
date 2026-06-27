#!/usr/bin/env bash
# ============================================================
# RustyBoard - download-model.sh
# Fetches a Whisper model from HuggingFace into the user's
# data directory.
# ============================================================
set -euo pipefail

MODEL="${1:-base.en}"
DEST_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/rusty-board/models"
BASE_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main"

# Validate name; we only accept the official whisper.cpp model names.
case "$MODEL" in
    tiny|tiny.en|base|base.en|small|small.en|medium|medium.en|large-v3|large)
        ;;
    *)
        echo "Unknown model '$MODEL'." >&2
        echo "Known: tiny, tiny.en, base, base.en, small, small.en, medium, medium.en, large-v3" >&2
        exit 2
        ;;
esac

FILE="ggml-${MODEL}.bin"
URL="$BASE_URL/$FILE"

mkdir -p "$DEST_DIR"

echo "==> Downloading $MODEL"
echo "    from: $URL"
echo "    to:   $DEST_DIR/$FILE"

if command -v curl >/dev/null 2>&1; then
    curl -L --fail --progress-bar -o "$DEST_DIR/$FILE" "$URL"
elif command -v wget >/dev/null 2>&1; then
    wget --show-progress -O "$DEST_DIR/$FILE" "$URL"
else
    echo "ERROR: need curl or wget." >&2
    exit 1
fi

# Quick sanity check — ggml model files start with the magic 'ggml' or 'lmgg'.
HEAD=$(head -c 4 "$DEST_DIR/$FILE" || true)
case "$HEAD" in
    ggml|lmgg) echo "==> Model OK." ;;
    *) echo "WARNING: $DEST_DIR/$FILE does not look like a ggml model. Re-download?" >&2 ;;
esac

echo
echo "Now set your config:"
echo "    rusty-board-config show > /tmp/rb.toml"
echo "    sed -i 's|model_path = .*|model_path = \"$DEST_DIR/$FILE\"|' /tmp/rb.toml"
echo "    cp /tmp/rb.toml \"\$(rusty-board-config path)\""
