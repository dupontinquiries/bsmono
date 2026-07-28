#pragma once
// ─── debug/TraceControl.hpp ──────────────────────────────────────────────────
// The Qt glue around the Qt-free policy in TraceChannels.hpp: persistence
// (QSettings), application (QLoggingCategory filter rules), auto-expiry
// (QTimer), and change notification (signal) for the Settings UI.
//
// Lifecycle, in main():
//     TraceControl::instance().initFromLaunch(qApp->arguments());
// which loads the saved channel set, lets env/CLI override it, prints
// --trace-help if asked, applies the rules, and arms the expiry timer.
// ─────────────────────────────────────────────────────────────────────────────

#include "debug/TraceChannels.hpp"

#include <QObject>
#include <QStringList>
#include <QTimer>

namespace SnapClone::Debug {

class TraceControl : public QObject
{
    Q_OBJECT
public:
    static TraceControl &instance();

    /// Read persisted channels, apply env + CLI overrides, print help if asked,
    /// apply the filter rules, arm auto-expiry. Returns the resulting mask.
    TraceMask initFromLaunch(const QStringList &args);

    [[nodiscard]] TraceMask mask() const { return liveMask(); }
    [[nodiscard]] bool isOn(TraceChannel c) const { return liveMask().has(c); }

    /// Flip one channel at runtime (Settings → Debug). Persists, re-applies the
    /// rules, restarts the expiry window, and emits changed().
    void setChannel(TraceChannel c, bool on, const QString &source = QStringLiteral("settings"));

    /// Replace the whole set at runtime (e.g. an "enable everything" button).
    void setMask(TraceMask m, const QString &source = QStringLiteral("settings"));

    /// Minutes before tracing auto-disables; 0 = never. Persisted.
    [[nodiscard]] int autoOffMinutes() const;
    void setAutoOffMinutes(int minutes);

    /// One-line status for the Settings page and bug reports, e.g.
    ///   "tracing: input,net — auto-off in 24 min"
    [[nodiscard]] QString statusLine() const;

    /// The same text as `--trace-help`, for display inside Settings.
    [[nodiscard]] static QString helpTextQ();

signals:
    void changed();

private:
    TraceControl();
    Q_DISABLE_COPY_MOVE(TraceControl)

    void applyRules() const;
    void persist() const;
    void armExpiry();

    QTimer m_expiry;
    qint64 m_deadlineMs = 0;
    int m_minutes = kDefaultTraceMinutes;
};

} // namespace SnapClone::Debug
