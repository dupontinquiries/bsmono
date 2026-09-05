#!/usr/bin/env python3
"""Every eframe dependency must make its backend choice explicit and survivable.

## Why

eframe 0.34's default feature set enables the `wgpu` backend and NOT `glow`,
so `Renderer::default()` silently becomes Wgpu. egui-wgpu's default device
descriptor then requests `wgpu::Limits::default()` — max_color_attachments: 8
— without clamping to the adapter it is literally holding in the closure. On
snapshot-cam this was invisible in every sandbox build and killed the app at
startup on a phone whose GPU allows 5:

    Error: Wgpu(RequestDeviceError(... LimitsExceeded(FailedLimit {
        name: "max_color_attachments", requested: 8, allowed: 5 })))

logscope-egui's manifest records the identical crash on Asahi
(max_color_attachments=5). The fleet convention is glow; two manifests had
drifted onto defaults and nothing said so until a device did.

## The rule

For every `eframe` dependency in a committed Cargo.toml:

1. `default-features = false` — the backend must be a choice someone made,
   never eframe's default.
2. `"glow"` in the feature list, OR `"wgpu"` present AND the crate's sources
   show the choice being handled: an explicit `Renderer::Wgpu` selection or a
   `wgpu_options`/`device_descriptor` configuration. A wgpu backend nobody
   configured is the snapshot-cam crash waiting for a smaller GPU.
   (This check is evidence of intent, not proof of clamped limits — it
   cannot read a closure. The clamp itself is pinned by the crate's own
   tests; see apps/snapclone/camera-bridge's `device_limits`.)

Exempt: `system/snapclone/` — the stale duplicate of `apps/snapclone` (C9-7a,
an open deletion decision; edits go to the live tree only, per C10-0b's note).
REMOVE THIS EXEMPTION when C9-7a lands.

## Dual-backend runtime fallback

`default-features = false` is required UNLESS the source names both
renderers explicitly — `Renderer::Wgpu` and `Renderer::Glow` both present —
which is the shape of "try Wgpu, fall back to Glow on error". That crate
needs eframe's *other* defaults (accesskit, default_fonts, wayland, x11,
web_screen_reader, and winit/default) and cannot cleanly restate them from a
dependent's feature list — winit/default in particular cannot be re-enabled
once turned off from here. Found via `unendlich-dash`, which the first
version of this checker flagged even though its source runs Wgpu and falls
back to Glow on the very next line (C14_TODO §6). This does not relax the
backend-feature requirement below: `"glow"` still has to be declared.

The first version of this checker regex-matched single lines and flagged
logscope-egui, whose multi-line feature list contained `"glow"` two lines
below the match. tomllib parses the manifest the way cargo does.
"""
import pathlib
import re
import sys
import tomllib

ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
EXEMPT_PREFIXES = ("system/snapclone/",)  # C9-7a: stale duplicate, pending deletion

WGPU_INTENT = re.compile(r"Renderer::Wgpu|wgpu_options|device_descriptor")
# Runtime dual-backend: BOTH renderers named explicitly, which is the shape of
# "try Wgpu, fall back to Glow" — not proof of a working fallback (a text scan
# can't see the match-arm structure), but the same evidentiary bar WGPU_INTENT
# already accepts for the single-backend case.
DUAL_BACKEND_WGPU = re.compile(r"Renderer::Wgpu")
DUAL_BACKEND_GLOW = re.compile(r"Renderer::Glow")


def eframe_deps(doc):
    """Yield (section_name, dep) for every eframe dependency in a parsed manifest."""
    for section in ("dependencies", "dev-dependencies", "build-dependencies"):
        dep = doc.get(section, {}).get("eframe")
        if dep is not None:
            yield section, dep
    for target_cfg, tbl in doc.get("target", {}).items():
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            dep = tbl.get(section, {}).get("eframe")
            if dep is not None:
                yield f"target.{target_cfg}.{section}", dep


def crate_sources(toml: pathlib.Path) -> str:
    src = toml.parent / "src"
    if not src.is_dir():
        return ""
    return "\n".join(
        p.read_text(errors="replace") for p in sorted(src.rglob("*.rs"))
    )


def resolve_workspace(toml: pathlib.Path, dep):
    """Resolve `eframe.workspace = true` against [workspace.dependencies].

    Cargo semantics: the member inherits the workspace definition; a member
    `features` list is ADDITIVE. The first version of this checker judged the
    member stub and flagged both av1tuner app crates whose workspace root
    already pins default-features = false + glow.
    """
    if not (isinstance(dep, dict) and dep.get("workspace")):
        return dep
    for ancestor in toml.parents:
        ws_toml = ancestor / "Cargo.toml"
        if ws_toml == toml or not ws_toml.is_file():
            continue
        try:
            ws = tomllib.loads(ws_toml.read_text(errors="replace"))
        except tomllib.TOMLDecodeError:
            continue
        if "workspace" not in ws:
            continue
        base = ws["workspace"].get("dependencies", {}).get("eframe")
        if base is None:
            return dep  # inherits from a workspace that does not define it: cargo would refuse
        if isinstance(base, str):
            base = {"version": base}
        merged = dict(base)
        merged["features"] = list(base.get("features", [])) + list(dep.get("features", []))
        return merged
    return dep


def violations(toml: pathlib.Path, section: str, dep):
    dep = resolve_workspace(toml, dep)
    if isinstance(dep, str):
        yield f"[{section}] bare version string: eframe's defaults choose wgpu, not glow"
        return
    if not isinstance(dep, dict):
        yield f"[{section}] unrecognised dependency form: {dep!r}"
        return
    features = dep.get("features", [])
    src = crate_sources(toml)
    # DUAL-BACKEND RUNTIME FALLBACK: both renderers named explicitly in source
    # (try Wgpu, fall back to Glow on error) is a legitimate reason to leave
    # default-features on. eframe's `default` isn't just "pick wgpu" — it also
    # carries accesskit, default_fonts, wayland, x11, web_screen_reader, and
    # winit/default, and that last one cannot be re-enabled from a dependent's
    # feature list once turned off. Restating the rest by hand to turn off
    # only the backend choice silently drops winit's own defaults with no way
    # to restore them from here (C14_TODO §6, unendlich-dash). This does NOT
    # exempt the backend-feature checks below — "glow" still has to be
    # declared, same as the single-backend case.
    dual_backend = bool(DUAL_BACKEND_WGPU.search(src) and DUAL_BACKEND_GLOW.search(src))
    if dep.get("default-features", True) and not dual_backend:
        yield f"[{section}] default-features is not false (eframe defaults enable wgpu, not glow)"
    if "glow" in features:
        return
    if "wgpu" in features:
        if not WGPU_INTENT.search(src):
            yield (
                f"[{section}] wgpu without glow, and no Renderer::Wgpu / "
                f"wgpu_options / device_descriptor anywhere in src/ — the "
                f"backend is wgpu by accident, with unclamped default limits"
            )
        return
    yield f"[{section}] no backend feature at all (neither \"glow\" nor \"wgpu\")"


manifests = [
    p for p in ROOT.rglob("Cargo.toml")
    if "target" not in p.parts and "vendor" not in p.parts
]
found, bad, exempted = 0, [], 0
for toml in sorted(manifests):
    rel = toml.relative_to(ROOT).as_posix()
    try:
        doc = tomllib.loads(toml.read_text(errors="replace"))
    except tomllib.TOMLDecodeError as e:
        bad.append((rel, f"unparseable manifest: {e}"))
        continue
    deps = list(eframe_deps(doc))
    if not deps:
        continue
    if any(rel.startswith(p) for p in EXEMPT_PREFIXES):
        exempted += 1
        continue
    found += 1
    for section, dep in deps:
        for why in violations(toml, section, dep):
            bad.append((rel, why))

print(
    f"scanned {len(manifests)} manifests: {found} with an eframe dependency, "
    f"{exempted} exempt (system/snapclone, C9-7a)"
)
# The found-something floor: a repo restructure that hid every manifest from
# the glob must not read as a pass. 13+ egui apps exist; require at least 5.
if found < 5:
    print(
        f"FAIL: only {found} eframe dependencies found — the scan is not "
        f"seeing the tree it was written for; fix the glob before trusting it"
    )
    sys.exit(1)
if bad:
    for rel, why in bad:
        print(f"FAIL {rel}  {why}")
    print(
        "\neframe backends are chosen, not inherited: default-features = "
        'false, plus "glow" — or "wgpu" with the choice visible in src/ '
        "(Renderer::Wgpu / wgpu_options) and its limits clamped (see "
        "apps/snapclone/camera-bridge). Full rationale in this file's "
        "docstring."
    )
    sys.exit(1)
print("all eframe dependencies choose their backend explicitly")
