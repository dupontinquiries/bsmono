// ─── debug/TraceControl.cpp ──────────────────────────────────────────────────
#include "debug/TraceControl.hpp"

#include <QCoreApplication>
#include <QDateTime>
#include <QLoggingCategory>
#include <QSettings>
#include <QTextStream>

#include <cstdio>
#include <cstdlib>

namespace SnapClone::Debug {
namespace {
constexpr auto kKeyChannels = "debug/trace_channels";
constexpr auto kKeyMinutes  = "debug/trace_auto_off_minutes";
Q_LOGGING_CATEGORY(lcTrace, "snapclone.debug.trace")
} // namespace

TraceControl &TraceControl::instance()
{
    static TraceControl c;
    return c;
}

TraceControl::TraceControl()
{
    m_expiry.setSingleShot(true);
    connect(&m_expiry, &QTimer::timeout, this, [this]() {
        if (!liveMask().any()) return;
        // Auto-off exists so a field build never quietly fills the disk: someone
        // enables a channel to catch a bug, forgets, and walks away.
        qCInfo(lcTrace, "diagnostic tracing auto-disabled after %d min (was: %s)",
               m_minutes, qUtf8Printable(QString::fromStdString(liveMask().toSpec())));
        setMask(TraceMask::none(), QStringLiteral("auto-off"));
    });
}

int TraceControl::autoOffMinutes() const { return m_minutes; }

void TraceControl::setAutoOffMinutes(int minutes)
{
    m_minutes = minutes < 0 ? 0 : minutes;
    persist();
    armExpiry();
    emit changed();
}

void TraceControl::applyRules() const
{
    QLoggingCategory::setFilterRules(QString::fromStdString(liveMask().filterRules()));
}

void TraceControl::persist() const
{
    QSettings s;
    s.setValue(QLatin1String(kKeyChannels), QString::fromStdString(liveMask().toSpec()));
    s.setValue(QLatin1String(kKeyMinutes), m_minutes);
}

void TraceControl::armExpiry()
{
    m_expiry.stop();
    m_deadlineMs = 0;
    if (liveMask().any() && m_minutes > 0) {
        m_deadlineMs = QDateTime::currentMSecsSinceEpoch() + qint64(m_minutes) * 60'000;
        m_expiry.start(m_minutes * 60'000);
    }
}

void TraceControl::setChannel(TraceChannel c, bool on, const QString &source)
{
    TraceMask m = liveMask();
    if (m.has(c) == on) return;
    m.set(c, on);
    setMask(m, source);
}

void TraceControl::setMask(TraceMask m, const QString &source)
{
    const auto before = liveMask();
    if (before.bits() == m.bits()) return;
    setLiveMask(m);
    applyRules();
    persist();
    armExpiry();
    // Always logged at info on a category that tracing itself never gates, so a
    // bug report shows what was on and who turned it on.
    qCInfo(lcTrace, "diagnostic tracing: %s -> %s (via %s)",
           before.toSpec().c_str(), m.toSpec().c_str(), qUtf8Printable(source));
    emit changed();
}

TraceMask TraceControl::initFromLaunch(const QStringList &args)
{
    QSettings s;
    TraceMask persisted;
    std::string err;
    const auto savedSpec = s.value(QLatin1String(kKeyChannels), QStringLiteral("none")).toString();
    if (!TraceMask::parse(savedSpec.toStdString(), TraceMask::none(), &persisted, &err)) {
        persisted = TraceMask::none(); // corrupt/stale settings never break startup
    }
    m_minutes = s.value(QLatin1String(kKeyMinutes), kDefaultTraceMinutes).toInt();

    std::vector<std::string> argv;
    argv.reserve(args.size());
    for (const auto &a : args) argv.push_back(a.toStdString());

    const auto lt = parseLaunch(std::getenv("SNAPCLONE_TRACE"),
                                std::getenv("SNAPCLONE_TRACE_MINUTES"),
                                argv, persisted);

    if (lt.showHelp) {
        // stdout, not the log: someone ran --trace-help at a terminal and wants
        // to read it right now.
        QTextStream(stdout) << helpTextQ() << Qt::endl;
    }
    if (!lt.error.empty()) {
        // A misspelled channel must be visible immediately — silently tracing
        // nothing would send the debugger chasing a phantom.
        QTextStream(stderr) << QStringLiteral("SnapClone: ")
                            << QString::fromStdString(lt.error) << Qt::endl;
    }
    if (lt.minutes > 0 || lt.explicitOff) m_minutes = lt.minutes;

    setLiveMask(lt.mask);
    applyRules();
    armExpiry();
    if (lt.mask.any()) {
        qCInfo(lcTrace, "diagnostic tracing active: %s (%s)",
               lt.mask.toSpec().c_str(), qUtf8Printable(statusLine()));
    }
    return lt.mask;
}

QString TraceControl::statusLine() const
{
    const auto m = liveMask();
    if (!m.any()) return QObject::tr("tracing: off");
    QString s = QObject::tr("tracing: %1").arg(QString::fromStdString(m.toSpec()));
    if (m_deadlineMs > 0) {
        const qint64 left = (m_deadlineMs - QDateTime::currentMSecsSinceEpoch()) / 60'000;
        s += QObject::tr(" — auto-off in %1 min").arg(qMax<qint64>(left, 0));
    } else {
        s += QObject::tr(" — until turned off");
    }
    return s;
}

QString TraceControl::helpTextQ()
{
    return QString::fromStdString(helpText());
}

} // namespace SnapClone::Debug
