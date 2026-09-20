#!/usr/bin/env python3
"""kit_feature_check — an app using a kit PLACEHOLDER while the real thing sits
behind a feature it never enabled.

This is the shape of the snapclone QR bug, and it is worth a checker because
neither side of it looks wrong:

  * `uikit_egui::qr` draws a QR-shaped identicon. Its own header is explicit --
    "there is no Reed-Solomon encoder ... A real QR reader will not decode it".
  * `uikit_egui::qrbig` is Nayuki's qrcodegen, behind the `qrbig` feature.
  * voiceterm-egui enabled it. Snapclone did not, and shipped a code its own
    scanner could not read.

Nothing complained. Both compile. Both draw something QR-shaped. The DEFAULT is
the placeholder, so an app gets the fake by saying nothing -- which is how it
survived to a device.

## What this looks for

A kit module that says it is a placeholder (its docs name a real counterpart),
a feature that provides the real one, and an app that uses the placeholder's
API without enabling that feature.

The pairing is declared here rather than inferred. Inferring "X is a fake Y"
from source is guesswork; the cases are few, and a wrong guess in a checker is
worse than a missing one because someone will act on it.

## What it deliberately does NOT flag

An app that uses the placeholder and wants the placeholder. The identicon is
dependency-free and honest; an app that only needs a recognisable visual for an
id is right to use it. The signal is USING THE PLACEHOLDER WHILE ALSO DOING THE
THING THAT NEEDS THE REAL ONE -- for QR, shipping a scanner. That "corroborator"
is what separates a choice from a bug, and a pair without one is reported as
advisory rather than as a finding.
"""

import re
import sys
from pathlib import Path

# (kit placeholder module, real module, feature, corroborating import)
#
# `corroborator` is what turns "chose the simple one" into "needed the other
# one": an app that decodes QR codes is an app whose own codes should be
# decodable. Without it there is nothing to distinguish a deliberate choice.
PAIRS = [
    {
        "placeholder": "qr",
        "real": "qrbig",
        "feature": "qrbig",
        "corroborator": r"\brqrr\b|qr_decode",
        "why": "the kit's `qr` is a QR-shaped identicon with no Reed-Solomon; "
               "an app that also decodes QR is showing a code it cannot read",
    },
]


def app_crates(root: Path):
    for toml in sorted(root.glob("apps/*/egui-frontend/Cargo.toml")):
        yield toml.parent.parent.name, toml
    for toml in sorted(root.glob("apps/*/Cargo.toml")):
        if (toml.parent / "src").is_dir():
            yield toml.parent.name, toml


def uikit_features(toml_text: str):
    """Features enabled on the uikit-egui dependency, or None if not a dependent."""
    for line in toml_text.splitlines():
        s = line.strip()
        if not s.startswith("uikit-egui") or "=" not in s:
            continue
        m = re.search(r"features\s*=\s*\[([^\]]*)\]", s)
        if not m:
            return set()
        return {f.strip().strip('"\'') for f in m.group(1).split(",") if f.strip()}
    return None


def rust_sources(crate_dir: Path):
    src = crate_dir / "src"
    if not src.is_dir():
        return
    for p in src.rglob("*.rs"):
        try:
            yield p, p.read_text(errors="replace")
        except OSError:
            continue


def main(root_arg="."):
    root = Path(root_arg).resolve()
    findings, advisories = [], []

    for name, toml in app_crates(root):
        text = toml.read_text(errors="replace")
        feats = uikit_features(text)
        if feats is None:
            continue  # does not depend on the kit
        crate = toml.parent
        blob = "".join(body for _, body in rust_sources(crate))
        if not blob:
            continue

        for pair in PAIRS:
            if pair["feature"] in feats:
                continue
            uses_placeholder = re.search(
                r"uikit_egui::%s\b" % re.escape(pair["placeholder"]), blob
            )
            if not uses_placeholder:
                continue
            corroborated = re.search(pair["corroborator"], blob)
            entry = (name, pair)
            (findings if corroborated else advisories).append(entry)

    if findings:
        print("apps using a kit placeholder while doing the thing that needs the real one:\n")
        for name, pair in findings:
            print(f"    {name}")
            print(f"        uses  uikit_egui::{pair['placeholder']}")
            print(f"        wants uikit_egui::{pair['real']}  (feature \"{pair['feature']}\")")
            print(f"        why   {pair['why']}\n")

    if advisories:
        print("advisory — placeholder in use, but nothing suggests the real one is needed:\n")
        for name, pair in advisories:
            print(f"    {name}: uikit_egui::{pair['placeholder']} "
                  f"(feature \"{pair['feature']}\" would give {pair['real']})")
        print()

    if findings:
        print(f"{len(findings)} finding(s). Enable the feature, or use the real module "
              f"directly if the placeholder was deliberate.")
        return 1
    print(f"kit_feature_check: PASS — {len(advisories)} advisory placeholder use(s), "
          f"none corroborated")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "."))
