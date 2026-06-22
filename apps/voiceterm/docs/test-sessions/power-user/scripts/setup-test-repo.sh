#!/usr/bin/env bash
# setup-test-repo.sh
#
# Create a predictable git repo at /tmp/w2q-test-repo for the
# power-user test session. Idempotent-ish: if the directory
# exists, we abort with a hint rather than silently corrupting
# state.
#
# State produced:
#   - main branch with 5 commits
#   - feature/foo branch (no extra commits, just exists)
#   - feature/bar branch with 2 extra commits beyond main
#   - One uncommitted modified file (README.md)
#   - One staged but uncommitted file (NEWFILE.txt)
#
# This produces a "main ●" badge in web2qml (dirty), with
# branches available for switch-branch voice tests.

set -euo pipefail

REPO=/tmp/w2q-test-repo

if [[ -e "$REPO" ]]; then
    echo "abort: $REPO already exists. rm -rf it first if you mean to recreate." >&2
    exit 1
fi

mkdir -p "$REPO"
cd "$REPO"

git init -q -b main
git config user.email "test@web2qml.local"
git config user.name "Test Runner"

cat > README.md <<'EOF'
# Test repo

This is a throwaway repo for the web2qml power-user test
session. Has predictable branches and dirty state so the
test session steps produce expected outcomes.

See docs/test-sessions/power-user/README.md in the main
web2qml checkout for the test plan.
EOF

git add README.md
git commit -q -m "initial commit"

# Build up history on main
for i in 1 2 3 4; do
    echo "line $i" >> CHANGELOG.txt
    git add CHANGELOG.txt
    git commit -q -m "feature $i: add changelog line"
done

# feature/foo: branched from main, no extra work
git branch feature/foo

# feature/bar: branched, with 2 extra commits
git checkout -q -b feature/bar
echo "bar work 1" > BAR.txt
git add BAR.txt
git commit -q -m "bar: add BAR.txt"
echo "bar work 2" >> BAR.txt
git add BAR.txt
git commit -q -m "bar: extend BAR.txt"

# Back to main, dirty it up
git checkout -q main
echo "dirty change" >> README.md   # modified, not staged

cat > NEWFILE.txt <<'EOF'
A new file that's staged but not committed.
The badge should show ● because of the unstaged README.md change.
EOF
git add NEWFILE.txt   # staged

echo "ready: $REPO"
echo ""
echo "Final state:"
echo "  Branches: $(git branch --format='%(refname:short)' | tr '\n' ' ')"
echo "  Current:  $(git rev-parse --abbrev-ref HEAD)"
echo "  Status:"
git status -s | sed 's/^/    /'
echo ""
echo "Open web2qml, switch to terminal tab, then:"
echo "  cd $REPO"
echo ""
echo "Badge should appear shortly. Proceed to §4 of the test session."
