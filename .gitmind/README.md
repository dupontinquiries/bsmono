# .gitmind (permanent, reproducible)

git and gitmind run side-by-side in this repo. The heavy object store
(`store.redb`) is **gitignored and regenerated**, not committed — committing it
would ~double `.git` (redb is incompressible to git) and self-reference on the
next import. To materialise/refresh it:

    maintenance/gitmind-sync.sh        # builds gin if needed, runs `gin import`

See `maintenance/gitmind-eval.md` for the reliability trials and the
compression findings (delta compression — not zstd level — is the real lever).
