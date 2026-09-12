# Reference artifacts for depot-core's tests

Nothing here was made by depot. Each file is what a REAL tool produced, so the
tests assert "depot derives the same thing the reference implementation does".

| file | origin | what it verifies |
|---|---|---|
| `cfg-if-1.0.0.crate`, `unicode-ident-1.0.12.crate` | `static.crates.io/crates/<n>/<n>-<v>.crate` | `.crate` → index line |
| `ref-cfg-if-1.0.0.json`, `ref-unicode-ident-1.0.12.json` | the matching line from `index.crates.io/<prefix>/<name>` (2026-09-12) | field-for-field equality (minus `pubtime`, which crates.io adds and cargo ignores) |
| `hello-1.0-1.i386.rpm`, `hello-2.0-1.x86_64.rpm`, `foo-1.0-1.noarch.rpm` | `rpm-software-management/rpm` `tests/data/RPMS/` (upstream test fixtures, tiny) | RPM lead/signature/header reading |
| `ref-primary.xml`, `ref-filelists.xml`, `ref-other.xml` | `createrepo_c 1.x --simple-md-filenames` (Ubuntu 24.04 package) over the three RPMs above, gunzipped | depot's native repodata vs createrepo_c's, package by package |

The `<time file=...>` attribute in `ref-primary.xml` is the mtime of the RPM on
the machine that ran createrepo_c and is excluded from comparison; everything
else must match.

`edge-1.2.3-4.fc42.noarch.rpm` was built here from `edge.spec` with
`rpmbuild 4.18` specifically to pin down createrepo_c's filtering rules on
awkward inputs: an epoch, versioned provides/requires/conflicts/obsoletes, a
rich (boolean) dependency, `Requires(post)` vs `Requires(postun)` on the same
name, a file requirement the package itself satisfies, a `%config`, a `%dir`,
a symlink, a `%ghost`, all four weak-dep kinds, and a three-entry changelog
with characters that need XML escaping. `ref-edge-*.xml` are createrepo_c's
output over it.
