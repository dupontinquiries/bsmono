#include <QApplication>
#include "debug/TraceControl.hpp"
#include <QSettings>
#include "ui/Theme.hpp"
#include "ui/UiDebugFlags.hpp"
#include <QDir>
#include <QStandardPaths>
#include <QByteArray>
#include <QCommandLineParser>
#include <QCommandLineOption>
#include <QFile>
#include <QTextStream>
#include <QDateTime>
#include <QMutex>
#include <QDebug>
#include <cstdio>

#include "ui/MainWindow.hpp"
#include "ui/UxGuide.hpp"   // V32.34+: flush pending saves on exit
#include "core/AppController.hpp"
#include "core/SingleInstanceGuard.hpp"  // V32.34+: one process per profile
#include "debug/DebugFlags.hpp"
#include "network/NetworkStats.hpp"

// ─── File logging ─────────────────────────────────────────────────────────────
// Activated by --log-file <path> or -l <path>.
// Thread-safe: mutex guards the file write.
// Format: ISO timestamp  CATEGORY  LEVEL  message
// The file is line-buffered so `tail -f snapclone.log` works in real time.
// ─────────────────────────────────────────────────────────────────────────────

namespace {

QFile       g_logFile;
QTextStream g_logStream;
QMutex      g_logMutex;
bool        g_fileLogging = false;

void messageHandler(QtMsgType type,
                    const QMessageLogContext &ctx,
                    const QString &msg)
{
    // Format: 2026-04-21T18:32:01.123  snapclone.nat  [W]  Punch timeout
    const char *level = "D";
    switch (type) {
    case QtDebugMsg:    level = "D"; break;
    case QtInfoMsg:     level = "I"; break;
    case QtWarningMsg:  level = "W"; break;
    case QtCriticalMsg: level = "C"; break;
    case QtFatalMsg:    level = "F"; break;
    }
    const QString ts  = QDateTime::currentDateTime()
                            .toString(QStringLiteral("yyyy-MM-ddTHH:mm:ss.zzz"));
    const QString cat = ctx.category ? QString::fromLatin1(ctx.category)
                                     : QStringLiteral("default");
    const QString line = QStringLiteral("%1  %2  [%3]  %4\n")
                             .arg(ts, cat, QString::fromLatin1(level), msg);

    // Always write to stderr (preserves existing terminal behaviour)
    fputs(qPrintable(line), stderr);

    // Also write to log file if enabled
    if (g_fileLogging) {
        QMutexLocker lock(&g_logMutex);
        g_logStream << line;
        g_logStream.flush(); // line-buffered — readable via tail -f
    }
}

void configureQtAttributes()
{
    QApplication::setAttribute(Qt::AA_SynthesizeTouchForUnhandledMouseEvents, false);

    // V32.32: high-frequency event compression. On Linux mobile (phosh,
    // Plasma Mobile on the FP5) touch screens commonly poll at 120Hz+
    // while the display refreshes at 60Hz. Without compression we get
    // 2 TouchUpdate events per displayable frame, each of which can
    // trigger a paint via scroll-bar setValue. Qt then produces 2x the
    // frames the compositor can present; the kernel scheduler sees a
    // backed-up paint queue and ramps cpufreq to catch up, which heats
    // up the device and adds latency to everything else.
    //
    // AA_CompressHighFrequencyEvents tells Qt to coalesce consecutive
    // events of the same type (touch updates, wheel events) before
    // delivery, so the application sees one event per displayable
    // frame instead of one per touchscreen sample. Effect on the user:
    // none (motion is still smooth, since we use accumulated dx/dy).
    // Effect on power: significant — we stop competing with the
    // compositor for CPU.
    QApplication::setAttribute(Qt::AA_CompressHighFrequencyEvents, true);
    // V32.32: tablet events also compressed. Less critical (we don't
    // really support stylus input) but cheap belt-and-braces.
    QApplication::setAttribute(Qt::AA_CompressTabletEvents, true);
}

void ensureDataDirectories()
{
    const QStringList paths = {
        QStandardPaths::writableLocation(QStandardPaths::AppDataLocation),
        QStandardPaths::writableLocation(QStandardPaths::CacheLocation),
        QStandardPaths::writableLocation(QStandardPaths::AppDataLocation) + QStringLiteral("/media"),
        QStandardPaths::writableLocation(QStandardPaths::AppDataLocation) + QStringLiteral("/db"),
    };
    for (const QString &path : paths)
        QDir().mkpath(path);
}

} // namespace

int main(int argc, char *argv[])
{
    configureQtAttributes();

    QApplication app(argc, argv);
    app.setApplicationName(QStringLiteral("SnapClone"));
    // V32.32: full reverse-DNS desktop file name. The portal subsystem
    // (used by Flatpak / Snap / xdg-desktop-portal-* on Fedora et al.)
    // looks up the .desktop file by exactly this string. Previously
    // "snapclone" was set, which didn't match any installed .desktop —
    // producing the warning:
    //   qt.qpa.services: "Could not register app ID:
    //                     App info not found for 'snapclone'"
    // The installed file is `com.snapclone.SnapClone.desktop`
    // (see src/CMakeLists.txt install rules and the AppStream metainfo
    // alongside it). Aligning Qt's app-id with the installed basename
    // satisfies the portal lookup. GeoClue2 also accepts the
    // reverse-DNS form via D-Bus.
    app.setDesktopFileName(QStringLiteral("com.snapclone.SnapClone"));
    app.setApplicationVersion(QStringLiteral("1.0.0"));
    app.setOrganizationName(QStringLiteral("SnapCloneProject"));
    app.setOrganizationDomain(QStringLiteral("snapclone.example.com"));

    // V32.34+: apply persistent debug flags. Must run AFTER QSettings'
    // organization/application names are set (so the settings file is
    // findable) and BEFORE any logging-category-using code runs. Reads
    // settings like "debug/mobileDisplay" and configures QLoggingCategory
    // filter rules accordingly.
    // Diagnostic tracing: load the saved channel set, let SNAPCLONE_TRACE and
    // --trace=... override it, print --trace-help if asked, and apply the
    // logging-category rules. Must run AFTER the QSettings org/app names are
    // set and BEFORE any category-using code logs.
    //
    // This replaces the old UiDebugFlags::apply() call. That function emitted
    // filter rules for only the categories it knew about, and
    // QLoggingCategory::setFilterRules REPLACES the whole rule set — so calling
    // it here would have silently reset every channel a CLI flag had just
    // enabled. UiDebugFlags now delegates to the same TraceControl, so the
    // legacy "mobile display debug" checkbox still works.
    SnapClone::Debug::TraceControl::instance().initFromLaunch(app.arguments());

    // ── CLI argument parsing ─────────────────────────────────────────────────
    QCommandLineParser parser;
    parser.setApplicationDescription(QStringLiteral("SnapClone — peer-to-peer photo messenger"));
    parser.addHelpOption();
    parser.addVersionOption();

    QCommandLineOption logFileOpt(
        { QStringLiteral("l"), QStringLiteral("log-file") },
        QStringLiteral("Write all log output to FILE (line-buffered; use 'tail -f' to follow). "
                       "Default: $XDG_DATA/SnapClone/snapclone.log when --log is passed without a path."),
        QStringLiteral("FILE"));
    parser.addOption(logFileOpt);

    QCommandLineOption logDefaultOpt(
        QStringLiteral("log"),
        QStringLiteral("Enable file logging to the default path "
                       "($XDG_DATA/SnapClone/snapclone.log). "
                       "Equivalent to --log-file with the default path."));
    parser.addOption(logDefaultOpt);

    QCommandLineOption verboseOpt(
        QStringLiteral("verbose"),  // no -v shortcut: Qt claims -v for --version
        QStringLiteral("Enable debug-level logging (sets QT_LOGGING_RULES=snapclone.*=true)."));
    parser.addOption(verboseOpt);

    // ── Debug toggles ───────────────────────────────────────────────────────
    // These can also be set via env vars (SNAPCLONE_NO_FSM_TUNING etc.) or
    // toggled live from the Settings page. Each toggle's activity is
    // captured in the end-of-session summary.
    QCommandLineOption noFsmTuningOpt(
        QStringLiteral("no-fsm-tuning"),
        QStringLiteral("Disable learned per-peer connectivity ranking. "
                       "Address ordering falls back to the static "
                       "same-LAN-first heuristic. Useful for A/B testing "
                       "or reproducing pre-V31 behaviour."));
    parser.addOption(noFsmTuningOpt);

    QCommandLineOption noMonBlockingOpt(
        QStringLiteral("no-monitoring-blocking"),
        QStringLiteral("Suppress FSM misalignment warnings. Validators "
                       "still run for stats but produce no log output. "
                       "Useful when debugging unrelated code paths that "
                       "would otherwise be drowned in expected warnings."));
    parser.addOption(noMonBlockingOpt);

    QCommandLineOption verboseFsmOpt(
        QStringLiteral("verbose-fsm"),
        QStringLiteral("Log every FSM transition, not just misaligned "
                       "ones. Useful for diagnosing why a particular "
                       "session got into an unexpected state."));
    parser.addOption(verboseFsmOpt);

    parser.process(app);

    // ── Logging setup ────────────────────────────────────────────────────────
    if (parser.isSet(verboseOpt)) {
        qputenv("QT_LOGGING_RULES", "snapclone.*=true");
    }

    QString logPath;
    if (parser.isSet(logFileOpt)) {
        logPath = parser.value(logFileOpt);
    } else if (parser.isSet(logDefaultOpt)) {
        logPath = QStandardPaths::writableLocation(QStandardPaths::AppDataLocation)
                + QStringLiteral("/snapclone.log");
    }

    if (!logPath.isEmpty()) {
        // Ensure parent directory exists
        QDir().mkpath(QFileInfo(logPath).absolutePath());
        g_logFile.setFileName(logPath);
        if (g_logFile.open(QIODevice::WriteOnly | QIODevice::Append | QIODevice::Text)) {
            g_logStream.setDevice(&g_logFile);
            g_fileLogging = true;
            // Write a session separator so multiple runs are distinguishable
            g_logStream << QStringLiteral("\n=== SnapClone started %1 ===\n")
                               .arg(QDateTime::currentDateTime()
                                        .toString(Qt::ISODateWithMs));
            g_logStream.flush();
            fprintf(stderr, "Logging to: %s\n", qPrintable(logPath));
        } else {
            fprintf(stderr, "Warning: could not open log file %s: %s\n",
                    qPrintable(logPath),
                    qPrintable(g_logFile.errorString()));
        }
    }

    // Install handler AFTER opening the file
    qInstallMessageHandler(messageHandler);

    // ── Debug flags (V31) ────────────────────────────────────────────────────
    // Sources, in order of priority (later overrides earlier):
    //   1. Settings file (persisted user preference)
    //   2. Environment variables (test-pipeline / CI setup)
    //   3. CLI args (this invocation only)
    // Anything that turned a flag ON is logged at WARNING level so the
    // logs make it obvious when monitoring is being bypassed.
    auto &debugFlags = SnapClone::Debug::DebugFlags::instance();
    debugFlags.loadFromSettings();
    debugFlags.loadFromEnvAndArgs(app.arguments());

    ensureDataDirectories();

    // ── Single-instance guard (V32.34+) ──────────────────────────────────────
    // Refuse to start a second process for the same profile. Running two
    // instances corrupts the shared SQLite db, makes our DHT identity
    // incoherent (two sets of ports/session-keys announced for one sc-id),
    // and multiplies DHT traffic + battery. If another instance is already
    // running we ping it to raise its window, then exit cleanly. MUST run
    // BEFORE controller.initialize() so a rejected second launch never
    // brings up any networking or opens the database.
    //
    // Profile is "default" for now; multi-account (switch-in-place) will
    // parameterize this. See the brief in docs/TODO.md.
    auto *instanceGuard =
        new SnapClone::Core::SingleInstanceGuard(QStringLiteral("default"), &app);
    if (!instanceGuard->tryBecomePrimary()) {
        fprintf(stderr, "SnapClone is already running for this profile — "
                        "raising the existing window and exiting.\n");
        return 0;
    }

    SnapClone::Core::AppController controller;
    controller.initialize();

    // Restore the user's theme preference before any widget is shown.
    // Without this, the app starts with the default palette regardless of what
    // the user picked in Settings — they'd see a flash of the default theme
    // on every launch and have to revisit Settings to "re-apply" it. The
    // setting is written to QSettings under "ui/palette" by SettingsPage on
    // every theme change.
    {
        QSettings s;
        const QString savedPalette = s.value(QStringLiteral("ui/palette"),
                                             QStringLiteral("dark")).toString();
        SnapClone::UI::Theme::setPalette(savedPalette);
        // V32.32: route through the dedup helper so its s_lastApplied
        // cache is primed with the startup sheet. Without this, the
        // first ambient-mode tick would always re-apply (comparing
        // against an empty string) even if the synthetic palette
        // happens to derive to the same sheet.
        SnapClone::UI::Theme::applyGlobalStyleSheetIfChanged();
    }

    SnapClone::UI::MainWindow window(&controller);
    window.resize(390, 844);
    window.show();

    // V32.34+: when a second launch is attempted, the guard's primary
    // instance emits raiseRequested — surface this window. On desktop
    // shells this brings the window forward; on Plasma Mobile the WM may
    // ignore programmatic raise, but the second process has already exited
    // either way, which is the part that matters for correctness/battery.
    QObject::connect(instanceGuard,
                     &SnapClone::Core::SingleInstanceGuard::raiseRequested,
                     &window, [&window]() {
        window.show();
        window.raise();
        window.activateWindow();
    });

    const int rc = QApplication::exec();

    // V32.34+ SHUTDOWN CRASH FIX (QStyleSheetStyleCaches corruption).
    // Backtrace showed a malloc_consolidate abort inside
    // QStyleSheetStyleCaches::objectDestroyed during the widget-tree
    // teardown at `return rc` below — a long-standing Qt hazard
    // (QTBUG-11658 / QTBUG-48213): with many per-widget setStyleSheet calls
    // plus a global app stylesheet, Qt can hold a per-object QRenderRule
    // cache whose destroyed()-signal connections outlive the
    // QStyleSheetStyle that owns them. Destroying a styled widget then
    // writes into a stale cache → heap corruption, detected at free.
    //
    // The established, Qt-version-independent workaround is to FLUSH the
    // stylesheet machinery in a controlled order while the QApplication and
    // its style are still fully alive — BEFORE the widget tree unwinds.
    // Clearing the global app stylesheet (and the main window's) makes Qt
    // re-resolve/evict the render-rule caches now, so by the time `window`
    // is destroyed there are no stale cache entries to corrupt. Cheap, and
    // purely a teardown-ordering measure — no behavior change in-session.
    qApp->setStyleSheet(QString());
    window.setStyleSheet(QString());
    QCoreApplication::processEvents();   // let the StyleChange propagate

    // V32.34+: flush any pending UxGuide debounced save. recordUse()
    // and friends arm a 2s debounce timer; if the user exits within
    // that window, the last batch of updates would be lost without
    // this. flush() is idempotent — no-op if nothing pending.
    SnapClone::UI::UxGuide::instance().flush();

    // ── End-of-session summary ───────────────────────────────────────────────
    // After the event loop exits but before objects tear down, dump:
    //   • Debug flag activity (which were ON, how long, how many toggles)
    //   • NetworkStats per-peer summary (success rate per address class)
    // Useful for test pipelines (CI can grep this section), for manual
    // post-mortems, and for users who hit a weird state and report it.
    qInfo().noquote() << "\n" << debugFlags.sessionSummary();
    qInfo().noquote() << "\n"
        << SnapClone::Network::Stats::NetworkStats::instance().sessionSummary();

    return rc;
}
