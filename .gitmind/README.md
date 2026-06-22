# .gitmind (permanent, reproducible)

git and gitmind run side-by-side in this repo. The heavy object store
(`store.redb`) is **gitignored and regenerated**, not committed — committing it
would ~double `.git` (redb is incompressible to git) and self-reference on the
next import. To materialise/refresh it:

    maintenance/gitmind-sync.sh        # builds gin if needed, runs `gin import`

See `maintenance/gitmind-eval.md` for the reliability trials and the
compression findings (delta compression — not zstd level — is the real lever).

## Receiving a new bundle (git + gitmind workflow)

Bundles are **git** bundles. To take a new one safely while keeping gitmind in
sync:

```sh
# 1. checkpoint your current state first (shadow snapshot, doesn't touch git)
gin snapshot "before <new bundle>"

# 2. pull the new history from the bundle
git fetch /path/to/new.bundle 'main:refs/heads/incoming'
git merge --ff-only incoming      # if your main can fast-forward
# ...or, if you have local commits: git rebase incoming   (replay yours on top)

# 3. resync the gitmind store to the new git graph
maintenance/gitmind-sync.sh

# 4. (optional) re-verify gitmind fidelity
gin export-native /tmp/check && diff <(git rev-parse main^{tree}) \
  <(git -C /tmp/check rev-parse HEAD^{tree})
```

The snapshot in step 1 means a bad merge is always recoverable via gitmind.
