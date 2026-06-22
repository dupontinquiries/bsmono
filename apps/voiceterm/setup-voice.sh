#!/bin/bash
# setup-voice.sh — Download Whisper model(s) and verify voice setup.
#
# Usage:
#   ./setup-voice.sh                  # default: all-quantized (tiny-q + base-q)
#   ./setup-voice.sh tiny             # tiny.en fp16, ~75MB
#   ./setup-voice.sh base             # base.en fp16, ~142MB
#   ./setup-voice.sh tiny-q           # tiny.en quantized Q5_1, ~31MB
#   ./setup-voice.sh base-q           # base.en quantized Q5_1, ~57MB
#   ./setup-voice.sh all              # tiny + base + their quants (~305MB)
#   ./setup-voice.sh all-fp16         # tiny + base fp16 only (~217MB)
#   ./setup-voice.sh small | small-q | medium
#   ./setup-voice.sh turbo            # large-v3-turbo Q5_0 (~547MB)
#   ./setup-voice.sh turbo-fp16       # large-v3-turbo fp16 (~1.62GB)
#
# DEFAULT (no args): downloads tiny-q + base-q only. ~88MB combined.
# These run in parallel via the dual-model dispatch in voice.rs and are
# faster than fp16 with negligible WER loss. This is the recommended
# starting setup for desktop and mobile alike.
#
# The Rust side runs tiny AND base in parallel when both are present,
# using whichever finishes first when base takes too long.
#
# About turbo: ggml-large-v3-turbo is Whisper's fastest large model
# (~6x faster than large-v3 by reducing decoder layers from 32 to 4).
# However on a typical desktop CPU it's still SLOWER than base.en in
# absolute terms because parameter count dominates over decoder depth
# at small scales. Useful only if you want best-quality transcription
# AND have GPU acceleration. The current voice.rs dual-model code does
# NOT auto-use turbo even if downloaded — adding turbo as a third tier
# is a TODO. For now turbo just sits on disk; you'd run whisper-cli
# manually against it for testing.

set -e

MODEL_DIR="$HOME/.config/web2qml/models"
mkdir -p "$MODEL_DIR"

# All URLs come from the upstream whisper.cpp Hugging Face repo:
#   https://huggingface.co/ggerganov/whisper.cpp/tree/main
URL_BASE="https://huggingface.co/ggerganov/whisper.cpp/resolve/main"

download_one() {
    local file="$1"
    local size_label="$2"
    local target="$MODEL_DIR/$file"
    local url="$URL_BASE/$file"
    if [ -f "$target" ]; then
        local sz; sz=$(du -h "$target" | cut -f1)
        echo "  already present: $file ($sz)"
        return 0
    fi
    echo "  downloading $file ($size_label)..."
    if command -v curl >/dev/null; then
        curl -L --fail "$url" -o "$target.partial" && mv "$target.partial" "$target"
    elif command -v wget >/dev/null; then
        wget "$url" -O "$target.partial" && mv "$target.partial" "$target"
    else
        echo "ERROR: need curl or wget"; exit 1
    fi
    echo "  done: $target"
}

# Default to "all-q" — quantized tiny + base, ~88MB combined.
# These cover the dual-model pipeline with minimal disk footprint.
VARIANT="${1:-all-q}"
echo "=== Web2QML Voice Setup ==="
echo "variant: $VARIANT"
echo "model dir: $MODEL_DIR"
echo ""

case "$VARIANT" in
    tiny)
        download_one "ggml-tiny.en.bin" "~75MB fp16"
        ;;
    base)
        download_one "ggml-base.en.bin" "~142MB fp16"
        ;;
    tiny-q|tiny-quant)
        download_one "ggml-tiny.en-q5_1.bin" "~31MB Q5_1"
        ;;
    base-q|base-quant)
        download_one "ggml-base.en-q5_1.bin" "~57MB Q5_1"
        ;;
    all-q|all-quant|quant)
        # Quantized only — recommended default. Total ~88MB.
        download_one "ggml-tiny.en-q5_1.bin" "~31MB Q5_1"
        download_one "ggml-base.en-q5_1.bin" "~57MB Q5_1"
        ;;
    all-fp16)
        # fp16 only — what older versions installed by default. Total ~217MB.
        download_one "ggml-tiny.en.bin" "~75MB fp16"
        download_one "ggml-base.en.bin" "~142MB fp16"
        ;;
    all)
        # Get tiny + base + their quants. Total ~305MB. Lets the user
        # A/B test fp16 vs quantized by swapping which file is named
        # ggml-tiny.en.bin (the loader checks q5_1 first, then fp16).
        download_one "ggml-tiny.en.bin"      "~75MB fp16"
        download_one "ggml-base.en.bin"      "~142MB fp16"
        download_one "ggml-tiny.en-q5_1.bin" "~31MB Q5_1"
        download_one "ggml-base.en-q5_1.bin" "~57MB Q5_1"
        ;;
    small)    download_one "ggml-small.en.bin"      "~466MB fp16" ;;
    small-q)  download_one "ggml-small.en-q5_1.bin"  "~182MB Q5_1" ;;
    medium)   download_one "ggml-medium.en.bin"     "~1.5GB fp16" ;;
    turbo|turbo-q)
        # Quantized Q5_0 of large-v3-turbo. Smallest practical turbo.
        download_one "ggml-large-v3-turbo-q5_0.bin" "~547MB Q5_0"
        ;;
    turbo-fp16|turbo-full)
        download_one "ggml-large-v3-turbo.bin" "~1.62GB fp16"
        ;;
    all-three|three-tier|desktop-q)
        # All three tiers, quantized — recommended for fast desktops with
        # at least 1GB RAM headroom for the third whisper context. The
        # voice.rs dispatcher will run all three in parallel and pick
        # the highest-priority result that arrives within the 1s grace
        # window. On systems where turbo can't keep up, it just gets
        # skipped at decision time (wasting CPU but preserving fallback).
        download_one "ggml-tiny.en-q5_1.bin"        "~31MB Q5_1"
        download_one "ggml-base.en-q5_1.bin"        "~57MB Q5_1"
        download_one "ggml-large-v3-turbo-q5_0.bin" "~547MB Q5_0"
        ;;
    *)
        echo "Unknown variant: $VARIANT"
        echo "Valid: tiny | base | tiny-q | base-q"
        echo "       all-q (default) | all-fp16 | all | all-three"
        echo "       small | small-q | medium"
        echo "       turbo | turbo-fp16"
        exit 1
        ;;
esac

echo ""
echo "=== Build options ==="
echo ""
echo "Option A: Native voice (recommended, faster)"
echo "  Requires: cmake, C++ compiler, ALSA/PulseAudio dev headers"
echo "  Ubuntu/Debian: sudo apt install cmake libasound2-dev"
echo "  Arch:          sudo pacman -S cmake alsa-lib"
echo "  Fedora:        sudo dnf install cmake alsa-lib-devel"
echo "  Alpine:        sudo apk add cmake alsa-lib-dev"
echo "  Then build:    cargo build --release --features voice"
echo ""
echo "Option B: External tool fallback (no extra build deps)"
echo "  Install whisper.cpp's 'whisper-cli' or 'whisper-cpp' in PATH"
echo "  Or:            cargo install whisper_cli"
echo "  Then build:    cargo build --release"
echo ""
echo "Models present in $MODEL_DIR:"
ls -lh "$MODEL_DIR"/ggml-*.bin 2>/dev/null | awk '{print "  ", $5, $9}' || echo "  (none)"
echo ""
echo "Runtime model selection:"
echo "  - With both tiny + base present, web2qml runs them in parallel."
echo "  - Tiny is used when base hasn't returned within +1s."
echo "  - Override with WEB2QML_FORCE_MODEL=tiny.en|base.en"
