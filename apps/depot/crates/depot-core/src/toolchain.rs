//! Rust toolchains. A device that can reach the crate mirror still cannot
//! build anything without `rustc`, `cargo` and `rust-std` for its triple, and
//! `rustup` cannot install them offline — it insists on fetching a channel
//! manifest from static.rust-lang.org. The standalone component tarballs
//! (`rustc-1.96.0-x86_64-unknown-linux-gnu.tar.xz` and friends) each carry
//! their own `install.sh`, which is what rustup runs under the hood; run
//! them directly and there is no rustup in the loop at all.
//!
//! `pack-rust.sh` (maintenance/) already downloads and verifies those
//! tarballs into a `rust-install/<triple>/` directory with an `INSTALL.sh`.
//! depot ingests that directory (or single tarballs) and serves:
//!
//! ```text
//! toolchains/index.json                          every (version, triple, component)
//! toolchains/<version>/<triple>/<component>-<version>-<triple>.tar.xz
//! toolchains/install.sh                          fetch + install for the host triple
//! ```
//!
//! The alternative — serving the rustup dist layout so `RUSTUP_DIST_SERVER`
//! can point here — needs the channel manifest too; see MANUAL.md "rustup
//! against the depot" for the steps once that is mirrored.

use crate::catalog::ToolchainEntry;
use crate::{invalid, Result, Store};
use std::path::Path;

/// `rustc-1.96.0-x86_64-unknown-linux-gnu.tar.xz` →
/// ("rustc", "1.96.0", "x86_64-unknown-linux-gnu"). Components may contain
/// dashes (`rust-std`, `rust-analysis`); `rust-src` has no triple → "*".
pub fn parse_component_filename(name: &str) -> Option<(String, String, String)> {
    let stem = name.strip_suffix(".tar.xz").or_else(|| name.strip_suffix(".tar.gz"))?;
    let parts: Vec<&str> = stem.split('-').collect();
    // The version is the first dash-separated part that starts with a digit
    // (1.96.0) or is a channel word (nightly/beta) followed by a date.
    let vi = parts.iter().position(|p| p.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) || *p == "nightly" || *p == "beta")?;
    if vi == 0 {
        return None;
    }
    let component = parts[..vi].join("-");
    let (version, triple) = if parts[vi] == "nightly" || parts[vi] == "beta" {
        // nightly-2026-09-01-<triple>: three date parts follow
        if parts.len() < vi + 4 {
            return None;
        }
        (parts[vi..vi + 4].join("-"), parts[vi + 4..].join("-"))
    } else {
        (parts[vi].to_string(), parts[vi + 1..].join("-"))
    };
    let triple = if triple.is_empty() { "*".to_string() } else { triple };
    Some((component, version, triple))
}

/// Ingest one component tarball.
pub fn ingest_file(store: &Store, path: &Path) -> Result<(ToolchainEntry, bool)> {
    let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| invalid("bad filename"))?;
    let (component, version, triple) = parse_component_filename(name).ok_or_else(|| invalid(format!("{name}: not <component>-<version>-<triple>.tar.xz")))?;
    // Cheap sanity: a component tarball is xz or gz; refuse an HTML error page
    // saved with the right name, which is how a bad mirror fetch shows up.
    let mut magic = [0u8; 6];
    {
        use std::io::Read;
        let mut f = std::fs::File::open(path)?;
        let n = f.read(&mut magic)?;
        let is_xz = n >= 6 && magic == [0xfd, b'7', b'z', b'X', b'Z', 0x00];
        let is_gz = n >= 2 && magic[..2] == [0x1f, 0x8b];
        if !(is_xz || is_gz) {
            return Err(invalid(format!("{name}: not an xz or gzip stream (magic {:02x?})", &magic[..n.min(6)])));
        }
    }
    let s = store.put_file(path, None)?;
    Ok((ToolchainEntry { version, triple, component, filename: name.to_string(), hash: s.hash, size: s.size, added: crate::now() }, s.fresh))
}

/// Ingest every `*.tar.xz`/`*.tar.gz` under `dir` (recursively one level —
/// the pack-rust layout is `rust-install/<triple>/*.tar.xz`).
pub fn ingest_dir(store: &Store, dir: &Path) -> Result<(Vec<ToolchainEntry>, Vec<String>)> {
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir)?.flatten() {
        let p = e.path();
        if p.is_dir() {
            for f in std::fs::read_dir(&p)?.flatten() {
                files.push(f.path());
            }
        } else {
            files.push(p);
        }
    }
    files.sort();
    for p in files {
        let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !(n.ends_with(".tar.xz") || n.ends_with(".tar.gz")) {
            continue;
        }
        match ingest_file(store, &p) {
            Ok((e, _)) => entries.push(e),
            Err(err) => errors.push(format!("{}: {err}", p.display())),
        }
    }
    Ok((entries, errors))
}

/// Relative path under `pub/toolchains/`.
pub fn served_path(e: &ToolchainEntry) -> String {
    format!("{}/{}/{}", e.version, e.triple, e.filename)
}

/// `index.json`: what a device needs to pick its files without a directory
/// listing.
pub fn render_index(entries: &[ToolchainEntry]) -> String {
    let mut sorted: Vec<&ToolchainEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| (&a.version, &a.triple, &a.component).cmp(&(&b.version, &b.triple, &b.component)));
    let items: Vec<serde_json::Value> = sorted
        .iter()
        .map(|e| serde_json::json!({"version": e.version, "triple": e.triple, "component": e.component, "path": served_path(e), "sha256": e.hash, "size": e.size}))
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({"toolchains": items})).unwrap() + "\n"
}

/// The installer a device runs: detects its triple, fetches the components
/// for the requested version from the depot, verifies sha256 against
/// index.json, and runs each component's own install.sh. POSIX sh, no
/// python, no jq — the boxes this targets may have neither.
pub const INSTALL_SH: &str = r#"#!/bin/sh
# depot toolchain installer. Usage:
#   sh install.sh [VERSION] [PREFIX]
# Env: DEPOT (base URL, default derived from how this script was fetched),
#      COMPONENTS (default "rustc cargo rust-std").
# Installs into PREFIX (default /usr/local; use $HOME/.local without root).
set -eu
DEPOT="${DEPOT:-__DEPOT_URL__}"
VERSION="${1:-}"
PREFIX="${2:-${PREFIX:-/usr/local}}"
COMPONENTS="${COMPONENTS:-rustc cargo rust-std}"

# host triple (mapping as in rustup-init.sh; musl detected via ldd)
os="$(uname -s)"; cpu="$(uname -m)"; clib=gnu
case "$os" in
  Linux) if ldd --version 2>&1 | grep -qi musl; then clib=musl; fi; os="unknown-linux-$clib" ;;
  Darwin) os="apple-darwin" ;;
  *) echo "unsupported OS: $os" >&2; exit 1 ;;
esac
case "$cpu" in
  x86_64|amd64) cpu=x86_64 ;;
  aarch64|arm64) cpu=aarch64 ;;
  riscv64) cpu=riscv64gc ;;
  *) echo "unsupported CPU: $cpu" >&2; exit 1 ;;
esac
triple="$cpu-$os"

fetch() { # url dest
  if command -v curl >/dev/null 2>&1; then curl -fsSL -o "$2" "$1";
  elif command -v wget >/dev/null 2>&1; then wget -q -O "$2" "$1";
  else echo "need curl or wget" >&2; exit 1; fi
}
sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
fetch "$DEPOT/toolchains/index.json" "$tmp/index.json"
if [ -z "$VERSION" ]; then
  VERSION="$(grep '"version"' "$tmp/index.json" | sed 's/.*: "\(.*\)".*/\1/' | sort -V | tail -1)"
  [ -n "$VERSION" ] || { echo "depot has no toolchains" >&2; exit 1; }
fi
echo "depot   : $DEPOT"
echo "version : $VERSION"
echo "triple  : $triple"
echo "prefix  : $PREFIX"

n=0
for c in $COMPONENTS; do
  file="$c-$VERSION-$triple.tar.xz"
  # pull the sha256 for this exact path out of index.json (no jq: one path per block)
  want="$(awk -v p="$VERSION/$triple/$file" '
    $0 ~ "\"path\": \"" p "\"" {found=1} found && /"sha256"/ {gsub(/[",]/,"",$2); print $2; exit}' "$tmp/index.json")"
  if [ -z "$want" ]; then echo "  $file: not in depot (skipping)" >&2; continue; fi
  echo "-> $file"
  fetch "$DEPOT/toolchains/$VERSION/$triple/$file" "$tmp/$file"
  got="$(sha256 "$tmp/$file")"
  [ "$got" = "$want" ] || { echo "  sha256 MISMATCH: $got != $want" >&2; exit 1; }
  mkdir -p "$tmp/x"; tar -xf "$tmp/$file" -C "$tmp/x"
  ( cd "$tmp/x/$c-$VERSION-$triple" && sh ./install.sh --prefix="$PREFIX" --disable-ldconfig >/dev/null )
  rm -rf "$tmp/x"
  n=$((n+1))
done
[ "$n" -gt 0 ] || { echo "nothing installed" >&2; exit 1; }
echo
"$PREFIX/bin/rustc" --version
"$PREFIX/bin/cargo" --version
echo "OK. Make sure $PREFIX/bin is on PATH."
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_parsing_covers_dashed_components_channels_and_no_triple() {
        let p = |s: &str| parse_component_filename(s).map(|(c, v, t)| format!("{c}|{v}|{t}"));
        assert_eq!(p("rustc-1.96.0-x86_64-unknown-linux-gnu.tar.xz").unwrap(), "rustc|1.96.0|x86_64-unknown-linux-gnu");
        assert_eq!(p("rust-std-1.96.0-aarch64-unknown-linux-musl.tar.xz").unwrap(), "rust-std|1.96.0|aarch64-unknown-linux-musl");
        assert_eq!(p("rust-src-1.96.0.tar.xz").unwrap(), "rust-src|1.96.0|*");
        assert_eq!(p("cargo-nightly-2026-09-01-x86_64-unknown-linux-gnu.tar.xz").unwrap(), "cargo|nightly-2026-09-01|x86_64-unknown-linux-gnu");
        assert_eq!(p("clippy-1.96.0-riscv64gc-unknown-linux-gnu.tar.gz").unwrap(), "clippy|1.96.0|riscv64gc-unknown-linux-gnu");
        assert_eq!(p("README.md"), None);
        assert_eq!(p("1.96.0-x86_64.tar.xz"), None);
    }

    #[test]
    fn ingest_refuses_non_archives_and_indexes_the_rest() {
        let d = crate::tmpdir("tc");
        let st = Store::open(&d);
        st.init().unwrap();
        let src = d.join("rust-install/x86_64-unknown-linux-gnu");
        std::fs::create_dir_all(&src).unwrap();
        let mut xz = vec![0xfd, b'7', b'z', b'X', b'Z', 0x00];
        xz.extend_from_slice(b"payload");
        std::fs::write(src.join("rustc-1.96.0-x86_64-unknown-linux-gnu.tar.xz"), &xz).unwrap();
        std::fs::write(src.join("cargo-1.96.0-x86_64-unknown-linux-gnu.tar.xz"), b"<html>404</html>").unwrap();
        std::fs::write(d.join("rust-install/INSTALL.sh"), b"#!/bin/sh").unwrap();
        let (entries, errors) = ingest_dir(&st, &d.join("rust-install")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("cargo-1.96.0") && errors[0].contains("magic"));
        assert_eq!(served_path(&entries[0]), "1.96.0/x86_64-unknown-linux-gnu/rustc-1.96.0-x86_64-unknown-linux-gnu.tar.xz");
        let idx = render_index(&entries);
        assert!(idx.contains("\"component\": \"rustc\""));
        assert!(INSTALL_SH.contains("__DEPOT_URL__"));
        std::fs::remove_dir_all(d).unwrap();
    }
}
