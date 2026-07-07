#!/usr/bin/env bash
# git_sync.sh — pull before applying, push commits + tags after. Run this
# manually if you prefer keeping git out of apply_and_test.sh (GIT_SYNC=0).
set -euo pipefail
REPO="${REPO:-$HOME/Code/unendlich-development/unendlich-game}"
BRANCH="${BRANCH:-rc29-adaptive-uploads}"
cd "$REPO"

case "${1:-pull}" in
  pull)  git fetch origin && git pull --rebase origin "$BRANCH" ;;
  push)  git push origin "$BRANCH" && git push origin --tags ;;
  *)     echo "usage: $0 [pull|push]"; exit 1 ;;
esac
