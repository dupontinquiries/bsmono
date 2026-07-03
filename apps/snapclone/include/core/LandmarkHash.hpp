#pragma once
// ─── Core/LandmarkHash.hpp ───────────────────────────────────────────────────
//
// V32.31: privacy-preserving image fingerprints for landmark matching.
//
// The original photo a user took to mark a landmark NEVER leaves the
// publisher's device. What's stored (and eventually shared on the DHT)
// is a pair of transforms:
//
//   • blurHash(image, 8) → 8-byte aHash of a heavily-blurred 64x64
//     downscale. Heavy gaussian (σ≈8) destroys text, faces, license
//     plates, micro-textures. What survives: building silhouettes,
//     skyline contour, dominant color blocks. Matches "is this the
//     same place from roughly the same angle?"
//
//   • edgeHash(image, 4) → 4-byte binary fingerprint of a Sobel edge
//     map, thresholded and reduced to an 8x4 cell grid. Discards
//     color and brightness entirely; keeps line geometry. Matches
//     the "same alleyway different day / different season" case
//     where blur-based hashes might miss due to lighting changes.
//
// Both are computed at landmark-creation time and stored as BLOBs on
// the landmark row. Matching is Hamming distance:
//   distance <= 10 bits (out of 64) is a confident blur match;
//   distance <= 6 bits  (out of 32) is a confident edge match.
//
// Why two hashes? Different failure modes:
//   • Blur hash is robust to lighting / minor pose changes but fails
//     if the scene's *content* changes (parked cars, signage, foliage)
//   • Edge hash is robust to content changes but fails on heavy
//     lighting / weather changes (night vs day swap)
// Together they give good coverage. A landmark resolution engine
// (Phase 2) takes the best of either signal.
//
// IMPORTANT — privacy guarantee:
//   The hashes are LOSSY one-way transforms. From an 8-byte blur hash
//   or 4-byte edge hash, you cannot reconstruct anything recognizable
//   about the original photo. This is by design — when (Phase 2) we
//   put hashes on the DHT, an attacker who scrapes the DHT cannot
//   recover faces or text from what they collected.
// ─────────────────────────────────────────────────────────────────────────────

#include <QByteArray>

QT_BEGIN_NAMESPACE
class QImage;
QT_END_NAMESPACE

namespace SnapClone::Core::LandmarkHash {

/// Compute the blur-based 64-bit aHash. Returns 8 bytes (big-endian
/// MSB-first order). Empty QByteArray on null input.
[[nodiscard]] QByteArray blurHash(const QImage &img);

/// Compute the edge-based 32-bit fingerprint. Returns 4 bytes.
/// Empty QByteArray on null input.
[[nodiscard]] QByteArray edgeHash(const QImage &img);

/// Hamming distance between two byte arrays (counted across all bits).
/// Returns -1 if sizes differ.
[[nodiscard]] int hammingDistance(const QByteArray &a, const QByteArray &b);

} // namespace SnapClone::Core::LandmarkHash
