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

## Receiving a zip drop (not a bundle)

The recipe above assumes the incoming history is a **descendant** — it either
fast-forwards or you rebase onto it. Project drops that arrive as zip archives
usually are not: they come from a line that branched at some earlier import
point and moved on independently, so they carry neither your later commits nor
any record of what they chose not to have.

Unpacking one over its directory therefore **deletes** whatever the monorepo
added since that branch point. Merge it as a sibling instead:

```sh
gin snapshot "before <drop>"                # checkpoint first, as always
git checkout -b drop/<project> <import-point>
rm -rf <path> && cp -a <unzipped>/. <path>/ # then commit
git checkout main && git merge --no-ff drop/<project>
```

Finding `<import-point>` is the real work: measure divergence against each
candidate commit rather than assuming the latest one. A file the drop lacks is
then correctly read as "they never had it" rather than "they deleted it".

If the archive genuinely lacks something the monorepo added, restore it on the
drop branch before committing — **and restore what wires it in**. The loss
lands in two places: the file, and its `mod` declaration / `[[bin]]` entry /
CMake source line. Restoring only the file leaves a tree that looks complete
and is not. After merging, diff those declaration sets against the pre-merge
state — the cheapest way to catch the whole class:

```sh
git diff <pre-merge>..HEAD -- '*lib.rs' '*mod.rs' '*Cargo.toml' '*CMakeLists.txt'
```

This has bitten twice now: `pub mod pngutil;` at r746 (fixed), and
`sim::netview` at r742 (still unwired).

See `maintenance/MERGE_C4_245.md` for a worked example across three drops.
