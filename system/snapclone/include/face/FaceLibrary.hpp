#pragma once

// ─── face/FaceLibrary.hpp ────────────────────────────────────────────────────
//
// RAII C++ wrapper around the Rust snapclone-face library FFI. Owns a
// FaceLibraryHandle for its lifetime; methods are thin wrappers around the
// `sc_face_library_*` C functions with idiomatic C++ types.
//
// Build gating: this header is always included, but ALL methods are no-ops
// when the SNAPCLONE_RUST_FACE compile-time flag is off. That lets callers
// write code that uses the library unconditionally without worrying about
// whether the Rust crate was built in — the cost of "do nothing" is one
// function call and one branch. Switching the build flag on flips the
// methods to call into the Rust side.
//
// Phase 1 of the face-bandwidth project. See docs/FACE_BANDWIDTH.md.
//
// Threading: the underlying Rust library is Send+Sync; multiple C++ threads
// can call methods on the same FaceLibrary without external synchronization.
// Internally Rust uses a Mutex; contention should be negligible for typical
// add-once-per-snap usage patterns.
//
// Ownership: non-copyable, non-movable (the handle is tied to the FFI
// allocation; copying would double-free, moving would leave the source
// holding a stale pointer). If sharing across owners is needed, wrap in
// std::shared_ptr at the call site.

#include <QString>
#include <QByteArray>
#include <cstddef>
#include <cstdint>
#include <array>
#include <optional>
#include <vector>  // V32.32: Detector::detect returns std::vector<Detection>

#ifdef SNAPCLONE_RUST_FACE
extern "C" {
#include "face/snapclone_face_ffi.h"
}
#endif

namespace SnapClone::Face {

/// Label kind passed to add() / labelCount(). Mirrors FaceLabelKind in the
/// C header so callers don't include the C header directly.
enum class LabelKind : int {
    SelfFace = 0,
    Contact  = 1,
};

/// Match the FFI error enum. Most callers only check for Ok vs not-Ok; the
/// specific variant is useful for logging.
enum class Status : int {
    Ok               = 0,
    InvalidInput     = 1,
    ConsentDisabled  = 2,
    BufferTooSmall   = 3,
    InvalidLabel     = 4,
    NotCompiled      = 100,  ///< C++ side built without SNAPCLONE_RUST_FACE
    Internal         = 99,
};

/// RAII wrapper around the Rust face library handle.
///
/// Construct → use → destruct. No explicit close needed; destructor calls
/// `sc_face_library_close` automatically.
class Library
{
public:
    /// Construct a new in-memory library. budgetBytes=0 means "use crate
    /// default" (50 MB). When SNAPCLONE_RUST_FACE is off, construction
    /// succeeds but all methods return NotCompiled.
    explicit Library(std::size_t budgetBytes = 0);
    ~Library();

    Library(const Library&)            = delete;
    Library& operator=(const Library&) = delete;
    Library(Library&&)                 = delete;
    Library& operator=(Library&&)      = delete;

    /// True if the FFI is compiled in AND the handle was created. When
    /// false, all other methods return NotCompiled and are no-ops.
    [[nodiscard]] bool isAvailable() const noexcept;

    /// Enable / disable the library. Disabling refuses future writes;
    /// existing data stays until wipe() is called.
    Status setEnabled(bool enabled);

    /// True if the library is currently enabled. False also when the FFI
    /// isn't compiled in — sane "feature unavailable" semantics for the
    /// caller.
    [[nodiscard]] bool isEnabled() const noexcept;

    /// Add a face crop. See FFI docs for `sc_face_library_add` for full
    /// semantics. The QByteArray contents are copied across the boundary;
    /// it can be safely discarded after the call returns.
    ///
    /// For SelfFace, contactId is ignored. For Contact, contactId must be
    /// non-empty (the cryptographic contact identifier).
    Status add(LabelKind kind,
               const QString &contactId,
               const std::array<std::uint8_t, 32> &contentHash,
               const QByteArray &bytes,
               std::uint32_t width,
               std::uint32_t height,
               std::int64_t  capturedAtMs,
               float quality);

    /// Wipe everything; returns the count of removed crops via the
    /// optional out-param. THE consent-withdrawal operation; always
    /// succeeds when the FFI is compiled in.
    Status wipe(std::size_t *removedOut = nullptr);

    /// Total bytes currently stored. 0 when FFI not compiled in or
    /// library is empty.
    [[nodiscard]] std::size_t totalBytes() const noexcept;

    /// Number of crops stored under a specific label. Returns std::nullopt
    /// on FFI error; the count for an empty label is 0 (not nullopt).
    [[nodiscard]] std::optional<std::size_t>
        labelCount(LabelKind kind, const QString &contactId) const;

private:
#ifdef SNAPCLONE_RUST_FACE
    FaceLibraryHandle *m_handle = nullptr;
#endif
};

/// Detected face: bounding box in source-image pixel space + confidence.
/// Mirrors the FFI's per-detection record; consumers get a Vec<Detection>
/// back from Detector::detect().
struct Detection {
    float xMin;
    float yMin;
    float xMax;
    float yMax;
    float confidence;

    /// Width of the bounding box.
    [[nodiscard]] float width()  const noexcept { return xMax - xMin; }
    /// Height of the bounding box.
    [[nodiscard]] float height() const noexcept { return yMax - yMin; }
    /// Area in source-image pixels.
    [[nodiscard]] float area()   const noexcept { return width() * height(); }
};

/// RAII wrapper around the Rust face detector.
///
/// Construct from a YOLO ONNX model path; detect runs inference and
/// returns bounding boxes. Heavy construction (model load is 50-200ms)
/// so create once at app startup and share across callers.
///
/// Thread-safety: the underlying Rust detector is Send + Sync. Multiple
/// C++ threads can call detect() concurrently. Methods take a const
/// QByteArray view of the RGB bytes so callers don't have to think
/// about buffer ownership.
///
/// Build gating identical to Face::Library — when SNAPCLONE_RUST_FACE
/// is off, construction "succeeds" but isAvailable() returns false and
/// detect() returns empty.
class Detector
{
public:
    /// Construct from a YOLO ONNX model file path. Use isAvailable()
    /// to check whether the model loaded successfully.
    ///
    /// confThreshold < 0 means "use default" (0.45). Same for iouThreshold.
    explicit Detector(const QString &modelPath,
                      float confThreshold = -1.0f,
                      float iouThreshold  = -1.0f);
    ~Detector();

    Detector(const Detector&)            = delete;
    Detector& operator=(const Detector&) = delete;
    Detector(Detector&&)                 = delete;
    Detector& operator=(Detector&&)      = delete;

    /// True if the FFI is compiled in AND the model loaded successfully.
    /// When false, detect() returns an empty vector.
    [[nodiscard]] bool isAvailable() const noexcept;

    /// Run face detection on an RGB888 image.
    ///
    /// `rgb` must contain `width * height * 3` bytes. The QByteArray's
    /// data is read but not modified or retained — safe to discard
    /// after the call returns.
    ///
    /// `maxResults` caps the output size (default 8 — typical
    /// "head and shoulders" photos have 1-4 faces; even group photos
    /// rarely exceed 8 in the foreground). Detections beyond max are
    /// dropped (lowest confidence first).
    [[nodiscard]] std::vector<Detection> detect(
        const QByteArray &rgb,
        std::uint32_t     width,
        std::uint32_t     height,
        std::size_t       maxResults = 8) const;

private:
#ifdef SNAPCLONE_RUST_FACE
    FaceDetectorHandle *m_handle = nullptr;
#endif
};

} // namespace SnapClone::Face
