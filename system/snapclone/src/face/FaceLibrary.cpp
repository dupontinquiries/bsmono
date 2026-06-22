#include "face/FaceLibrary.hpp"

// All Rust-touching code lives behind SNAPCLONE_RUST_FACE. When the flag
// is off, every method is a no-op returning Status::NotCompiled (for
// write ops) or sane defaults (for reads). This keeps callers from having
// to scatter #ifdefs through their own code.

#include <QLoggingCategory>

namespace SnapClone::Face {

namespace {
Q_LOGGING_CATEGORY(lcFaceLib, "snapclone.face.library")
} // anon

#ifdef SNAPCLONE_RUST_FACE

// Helper: translate the C FFI enum to our C++ Status.
static Status fromFfi(FaceFfiError e)
{
    switch (e) {
        case FaceFfiError_Ok:              return Status::Ok;
        case FaceFfiError_InvalidInput:    return Status::InvalidInput;
        case FaceFfiError_ConsentDisabled: return Status::ConsentDisabled;
        case FaceFfiError_BufferTooSmall:  return Status::BufferTooSmall;
        case FaceFfiError_InvalidLabel:    return Status::InvalidLabel;
        default:                           return Status::Internal;
    }
}

static FaceLabelKind toFfiKind(LabelKind k)
{
    switch (k) {
        case LabelKind::SelfFace: return FaceLabelKind_SelfFace;
        case LabelKind::Contact:  return FaceLabelKind_Contact;
    }
    return FaceLabelKind_SelfFace; // unreachable on a valid enum value
}

Library::Library(std::size_t budgetBytes)
{
    m_handle = sc_face_library_open(budgetBytes);
    if (!m_handle) {
        qCWarning(lcFaceLib) << "sc_face_library_open returned null —"
                                " allocation failure?";
    }
}

Library::~Library()
{
    if (m_handle) {
        sc_face_library_close(m_handle);
        m_handle = nullptr;
    }
}

bool Library::isAvailable() const noexcept { return m_handle != nullptr; }

Status Library::setEnabled(bool enabled)
{
    if (!m_handle) return Status::NotCompiled;
    return fromFfi(sc_face_library_set_enabled(m_handle, enabled));
}

bool Library::isEnabled() const noexcept
{
    if (!m_handle) return false;
    return sc_face_library_is_enabled(m_handle);
}

Status Library::add(LabelKind kind,
                    const QString &contactId,
                    const std::array<std::uint8_t, 32> &contentHash,
                    const QByteArray &bytes,
                    std::uint32_t width,
                    std::uint32_t height,
                    std::int64_t capturedAtMs,
                    float quality)
{
    if (!m_handle) return Status::NotCompiled;

    // Convert QString → UTF-8 \0-terminated buffer. toUtf8() returns a
    // QByteArray that already has the terminator; we hold it on the
    // stack so the pointer is valid for the duration of the FFI call.
    const QByteArray idUtf8 = (kind == LabelKind::Contact)
                                  ? contactId.toUtf8()
                                  : QByteArray();
    const char *idPtr = (kind == LabelKind::Contact)
                            ? idUtf8.constData()
                            : nullptr;

    const std::uint8_t *bytesPtr =
        bytes.isEmpty() ? nullptr
                        : reinterpret_cast<const std::uint8_t *>(bytes.constData());
    const std::size_t bytesLen = static_cast<std::size_t>(bytes.size());

    const auto err = sc_face_library_add(
        m_handle,
        toFfiKind(kind),
        idPtr,
        contentHash.data(),
        bytesPtr,
        bytesLen,
        width, height,
        capturedAtMs,
        quality);
    return fromFfi(err);
}

Status Library::wipe(std::size_t *removedOut)
{
    if (!m_handle) {
        if (removedOut) *removedOut = 0;
        return Status::NotCompiled;
    }
    std::size_t n = 0;
    const auto err = sc_face_library_wipe(m_handle, &n);
    if (removedOut) *removedOut = n;
    return fromFfi(err);
}

std::size_t Library::totalBytes() const noexcept
{
    if (!m_handle) return 0;
    return sc_face_library_total_bytes(m_handle);
}

std::optional<std::size_t>
Library::labelCount(LabelKind kind, const QString &contactId) const
{
    if (!m_handle) return std::nullopt;
    const QByteArray idUtf8 = (kind == LabelKind::Contact)
                                  ? contactId.toUtf8()
                                  : QByteArray();
    const char *idPtr = (kind == LabelKind::Contact)
                            ? idUtf8.constData()
                            : nullptr;
    std::size_t count = 0;
    const auto err = sc_face_library_label_count(
        m_handle, toFfiKind(kind), idPtr, &count);
    if (err != FaceFfiError_Ok) return std::nullopt;
    return count;
}

#else // SNAPCLONE_RUST_FACE not defined

// All methods are NotCompiled / safe-default fallbacks. We still compile
// the class into the binary so callers don't need to wrap usages in
// #ifdef — they get back NotCompiled and can decide what to do.

Library::Library(std::size_t /*budgetBytes*/) {}
Library::~Library() = default;

bool   Library::isAvailable() const noexcept { return false; }
Status Library::setEnabled(bool /*e*/)       { return Status::NotCompiled; }
bool   Library::isEnabled() const noexcept   { return false; }

Status Library::add(LabelKind, const QString &,
                    const std::array<std::uint8_t, 32> &,
                    const QByteArray &, std::uint32_t, std::uint32_t,
                    std::int64_t, float)
{
    return Status::NotCompiled;
}

Status Library::wipe(std::size_t *removedOut)
{
    if (removedOut) *removedOut = 0;
    return Status::NotCompiled;
}

std::size_t Library::totalBytes() const noexcept { return 0; }

std::optional<std::size_t>
Library::labelCount(LabelKind, const QString &) const { return std::nullopt; }

#endif // SNAPCLONE_RUST_FACE

// ─── Detector ─────────────────────────────────────────────────────────────

#ifdef SNAPCLONE_RUST_FACE

Detector::Detector(const QString &modelPath,
                   float confThreshold,
                   float iouThreshold)
{
    const QByteArray pathUtf8 = modelPath.toUtf8();
    FaceFfiError err = FaceFfiError_Ok;
    m_handle = sc_face_detector_open(
        pathUtf8.constData(),
        confThreshold,
        iouThreshold,
        &err);
    if (!m_handle) {
        qCWarning(lcFaceLib) << "Failed to load face detection model from"
                             << modelPath << "— FFI err code:" << int(err);
    } else {
        qCInfo(lcFaceLib) << "Loaded face detection model from" << modelPath;
    }
}

Detector::~Detector()
{
    if (m_handle) {
        sc_face_detector_close(m_handle);
        m_handle = nullptr;
    }
}

bool Detector::isAvailable() const noexcept { return m_handle != nullptr; }

std::vector<Detection> Detector::detect(
    const QByteArray &rgb,
    std::uint32_t width,
    std::uint32_t height,
    std::size_t maxResults) const
{
    std::vector<Detection> out;
    if (!m_handle || maxResults == 0 || rgb.isEmpty()) return out;

    // Pre-allocate the parallel arrays the FFI expects.
    std::vector<float> xMin(maxResults), yMin(maxResults);
    std::vector<float> xMax(maxResults), yMax(maxResults);
    std::vector<float> conf(maxResults);
    std::size_t count = 0;

    const auto bytesPtr = reinterpret_cast<const std::uint8_t*>(rgb.constData());
    const auto err = sc_face_detector_detect(
        m_handle,
        bytesPtr,
        width, height,
        maxResults,
        xMin.data(), yMin.data(),
        xMax.data(), yMax.data(),
        conf.data(),
        &count);

    if (err != FaceFfiError_Ok) {
        qCDebug(lcFaceLib) << "detect() returned err code" << int(err);
        return out;
    }
    out.reserve(count);
    for (std::size_t i = 0; i < count; ++i) {
        out.push_back(Detection{
            xMin[i], yMin[i], xMax[i], yMax[i], conf[i],
        });
    }
    return out;
}

#else // SNAPCLONE_RUST_FACE not defined

Detector::Detector(const QString &, float, float) {}
Detector::~Detector() = default;
bool Detector::isAvailable() const noexcept { return false; }
std::vector<Detection> Detector::detect(
    const QByteArray &, std::uint32_t, std::uint32_t, std::size_t) const
{
    return {};
}

#endif // SNAPCLONE_RUST_FACE

} // namespace SnapClone::Face
