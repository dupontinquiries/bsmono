#!/bin/sh
# What an Apple Silicon (Asahi) machine has for video, in one paste, before
# testing vidkit-player on it. Read-only; runs nothing as root.
#
#   tools/asahi-preflight.sh [path/to/vidkit-player]
#
# Every line says what it means for the player. Paste the whole output back.
PLAYER=${1:-$(dirname "$0")/../target/release/vidkit-player}
say() { printf '%-34s %s\n' "$1" "$2"; }

say "kernel" "$(uname -r)"
say "page size" "$(getconf PAGESIZE) bytes (Asahi: 16384)"
chip=$(cat /proc/device-tree/compatible 2>/dev/null | tr -d '\0' | grep -o 'apple,t[0-9]*' | head -1)
say "SoC" "${chip:-unknown}  (t6020/t6021/t8112 = M2 family: AVD has NO AV1; M3+ does)"
say "AVD V4L2 device" "$(for d in /sys/class/video4linux/video*; do cat "$d/name" 2>/dev/null; done | tr '\n' ' ')"
say "LIBVA_DRIVER_NAME" "${LIBVA_DRIVER_NAME:-unset -- the AVD VA layer needs it set for the session}"

echo "--- vainfo --display drm (the VA layer; what the player reads)"
if command -v vainfo >/dev/null; then
  vainfo --display drm 2>&1 | grep -E 'Driver version|VAEntrypoint' | sed 's/^/  /'
  vainfo --display drm 2>/dev/null | grep -q VAEntrypointVideoProc \
    && echo "  VideoProc present: scale_vaapi works" \
    || echo "  no VideoProc: the player downloads and scales in software (expected on AVD)"
else
  echo "  vainfo missing (libva-utils): the player cannot see the AVD VA layer"
fi

echo "--- ffmpeg"
command -v ffmpeg >/dev/null || { echo "  ffmpeg missing"; exit 1; }
ffmpeg -hide_banner -version | head -1 | sed 's/^/  /'
dec=$(ffmpeg -hide_banner -decoders 2>/dev/null)
for n in vp9 av1 libdav1d h264 libopenh264 hevc; do
  if echo "$dec" | grep -qE "^ [A-Z.]{6} $n "; then say "  decoder $n" "yes"; else say "  decoder $n" "NO"; fi
done
echo "  (no native h264/hevc = Fedora ffmpeg-free: the player will not pick those streams for hardware;"
echo "   'dnf swap ffmpeg-free ffmpeg --allowerasing' from RPM Fusion adds them)"
ffmpeg -hide_banner -hwaccels 2>/dev/null | grep -q vaapi && say "  hwaccel vaapi" "yes" || say "  hwaccel vaapi" "NO: no hardware decode at all"

echo "--- yt-dlp"
command -v yt-dlp >/dev/null && say "  version" "$(yt-dlp --version)  (YouTube breaks old ones: keep it current)" || say "  yt-dlp" "missing: no YouTube"

echo "--- GL"
command -v glxinfo >/dev/null && glxinfo -B 2>/dev/null | grep -E 'renderer|core profile version' | sed 's/^/  /' || echo "  glxinfo missing (mesa-demos)"

echo "--- whisper models (~/models/whisper)"
ls "$HOME/models/whisper" 2>/dev/null | sed 's/^/  /' || echo "  none: put ggml-tiny.en-q5_1.bin there for transcripts"

echo "--- vidkit-player --gpu-selftest"
if [ -x "$PLAYER" ]; then timeout 60 "$PLAYER" --gpu-selftest 2>/dev/null | sed 's/^/  /'; else echo "  build first: cargo build --release (in apps/vidkit-player)"; fi
