#pragma once
// ─── core/MediaMetadataStripper.hpp ───────────────────────────────────────────
//
// V32.39+: thin wrapper over the snapclone-mediashare metadata-strip FFI. Removes
// EXIF/GPS, XMP/IPTC, comments, timestamps and embedded thumbnails from a JPEG or
// PNG before it leaves the device — structurally, no pixel decode. Unknown
// formats pass through untouched. The strip is the privacy gate to run on any
// image just before it's encrypted + sent.
//
// When the mediashare crate isn't compiled in (no SNAPCLONE_RUST_MEDIASHARE),
// this degrades to an identity passthrough (stripped=false) so callers need no
// feature #ifdefs.
// ─────────────────────────────────────────────────────────────────────────────

#include <QByteArray>

namespace SnapClone::Core {

struct StripOutcome {
    QByteArray data;            // stripped bytes, or the input unchanged on passthrough
    int        format   = 0;    // SC_IMAGE_FORMAT_* — 0 unknown, 1 JPEG, 2 PNG
    int        removed  = 0;    // metadata segments removed
    bool       stripped = false;// true iff a real strip ran (mediashare built + JPEG/PNG)
};

namespace MediaStrip {

/// Strip metadata from an image. Output is always ≤ input. Never throws; on any
/// error or when mediashare isn't built, returns the input unchanged.
[[nodiscard]] StripOutcome stripImage(const QByteArray &in, bool keepColorProfile = true);

} // namespace MediaStrip
} // namespace SnapClone::Core
