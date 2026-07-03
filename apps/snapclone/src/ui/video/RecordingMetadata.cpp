#include "ui/video/RecordingMetadata.hpp"

#include <QMutexLocker>

namespace SnapClone::UI::Video {

RecordingMetadata &RecordingMetadata::instance()
{
    // Meyers singleton — initialized on first use, thread-safe in C++11+.
    static RecordingMetadata s_instance;
    return s_instance;
}

void RecordingMetadata::setHasAudio(const QString &path, bool hasAudio)
{
    if (path.isEmpty()) return;
    QMutexLocker lock(&m_mu);
    m_audioByPath.insert(path, hasAudio);
}

std::optional<bool> RecordingMetadata::hasAudio(const QString &path) const
{
    if (path.isEmpty()) return std::nullopt;
    QMutexLocker lock(&m_mu);
    auto it = m_audioByPath.constFind(path);
    if (it == m_audioByPath.constEnd()) return std::nullopt;
    return *it;
}

void RecordingMetadata::forget(const QString &path)
{
    if (path.isEmpty()) return;
    QMutexLocker lock(&m_mu);
    m_audioByPath.remove(path);
}

void RecordingMetadata::clear()
{
    QMutexLocker lock(&m_mu);
    m_audioByPath.clear();
}

} // namespace SnapClone::UI::Video
