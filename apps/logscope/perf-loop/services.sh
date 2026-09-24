#!/usr/bin/env bash
# perf-loop/services.sh {stop|start} — runs ON the target. Edit the two lists to match what you actually run.
# Purpose: nothing else touches the GPU while a bench runs. Reversible: `start` puts it all back.
set -u
case "${1:-}" in
  stop)
    if [ "$(uname -s)" = Darwin ]; then
        brew services stop ollama 2>/dev/null || true
        # add your launchd labels here, e.g. com.you.llama-server
        for lbl in com.you.llama-server com.you.optiq; do launchctl bootout "gui/$(id -u)/$lbl" 2>/dev/null || true; done
    else
        systemctl --user stop ollama llama-server 2>/dev/null || true
    fi
    pkill -f 'llama-server|omlx|optiq serve|mlx_lm' 2>/dev/null || true
    sleep 2; pgrep -fl 'llama-server|ollama|omlx|optiq|mlx_lm' && echo "still running ^" || echo "quiet"
    ;;
  start)
    if [ "$(uname -s)" = Darwin ]; then
        brew services start ollama 2>/dev/null || true
        for lbl in com.you.llama-server com.you.optiq; do
            f="$HOME/Library/LaunchAgents/$lbl.plist"; [ -f "$f" ] && launchctl bootstrap "gui/$(id -u)" "$f" 2>/dev/null || true
        done
    else
        systemctl --user start ollama llama-server 2>/dev/null || true
    fi
    echo "services restarted"
    ;;
  *) echo "usage: services.sh {stop|start}"; exit 1 ;;
esac
