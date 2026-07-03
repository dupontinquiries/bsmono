#include "core/MediaMetadataStripper.hpp"

#ifdef SNAPCLONE_RUST_MEDIASHARE
#include "mediashare/snapclone_mediashare_ffi.h"
#include <cstdint>
#endif

namespace SnapClone::Core::MediaStrip {

StripOutcome stripImage(const QByteArray &in, bool keepColorProfile)
{
    StripOutcome r;
    r.data = in;                 // default: passthrough
    if (in.isEmpty()) return r;

#ifdef SNAPCLONE_RUST_MEDIASHARE
    // Stripping only removes bytes, so the output never exceeds the input — one
    // allocation suffices. The -2 (buffer too small) branch is honored anyway in
    // case the contract ever changes.
    QByteArray out(in.size(), Qt::Uninitialized);
    size_t outLen = 0, removed = 0;
    int fmt = 0;

    auto run = [&](void) {
        return sc_image_strip_metadata(
            reinterpret_cast<const uint8_t *>(in.constData()),
            static_cast<size_t>(in.size()),
            keepColorProfile ? 1 : 0,
            reinterpret_cast<uint8_t *>(out.data()),
            static_cast<size_t>(out.size()),
            &outLen, &removed, &fmt);
    };

    int rc = run();
    if (rc == -2 && outLen > 0) {           // grow to required size, retry once
        out.resize(static_cast<int>(outLen));
        rc = run();
    }

    if (rc == 0) {                          // JPEG/PNG cleaned
        out.resize(static_cast<int>(outLen));
        r.data     = out;
        r.format   = fmt;
        r.removed  = static_cast<int>(removed);
        r.stripped = true;
    } else if (rc == 1) {                   // unknown format — input passed through
        r.format   = fmt;                   // r.data stays = in
        r.stripped = false;
    }
    // rc < 0 (e.g. -1 null) → leave passthrough (stripped=false)
#else
    Q_UNUSED(keepColorProfile);
#endif
    return r;
}

} // namespace SnapClone::Core::MediaStrip
