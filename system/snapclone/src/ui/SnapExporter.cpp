#include "ui/SnapExporter.hpp"

#include <QStandardPaths>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QProcess>
#include <QDate>
#include <QObject>   // QObject::tr in this non-QObject helper
#include <QLoggingCategory>

Q_LOGGING_CATEGORY(lcExport, "snapclone.export")

namespace SnapClone::UI {

QString SnapExporter::mediaDir()
{
    return QStandardPaths::writableLocation(QStandardPaths::AppDataLocation)
         + QStringLiteral("/media");
}

QString SnapExporter::suggestedFileName()
{
    return QStringLiteral("snapclone-export-%1.zip")
        .arg(QDate::currentDate().toString(QStringLiteral("yyyy-MM")));
}

bool SnapExporter::hasTool(const QString &tool)
{
    return !QStandardPaths::findExecutable(tool).isEmpty();
}

SnapExporter::Result SnapExporter::createArchive(const QString &requestedPath)
{
    Result r;
    const QString dir = mediaDir();
    QDir media(dir);
    if (!media.exists()) {
        // No media folder yet — treat as an empty (successful) export so the
        // caller can say "nothing to export" rather than show an error.
        r.ok = true;
        r.fileCount = 0;
        return r;
    }

    // Collect the media files (flat folder of per-snap blobs). We archive the
    // folder's CONTENTS, not the folder itself, so the zip opens to the files
    // directly. Count them for the result + the empty-folder short-circuit.
    const QStringList entries =
        media.entryList(QDir::Files | QDir::NoDotAndDotDot);
    r.fileCount = entries.size();
    if (entries.isEmpty()) {
        r.ok = true;   // nothing to do, not an error
        return r;
    }

    // Decide tool + normalize the output extension to match what we'll run.
    // Prefer zip (native-open everywhere); fall back to tar.gz.
    QFileInfo reqInfo(requestedPath);
    const QString baseNoExt =
        reqInfo.absolutePath() + QLatin1Char('/') + reqInfo.completeBaseName();

    QString program;
    QStringList args;
    if (hasTool(QStringLiteral("zip"))) {
        r.archivePath = baseNoExt + QStringLiteral(".zip");
        // `zip -r -j <out> .` from inside the media dir: -j junks paths so the
        // archive is a flat set of files (no AppData/media/ prefix). Use -q
        // for quiet; overwrite any existing file at that path first so a
        // re-export is deterministic.
        QFile::remove(r.archivePath);
        program = QStringLiteral("zip");
        args = { QStringLiteral("-q"), QStringLiteral("-j"),
                 QStringLiteral("-r"), r.archivePath, QStringLiteral(".") };
    } else if (hasTool(QStringLiteral("tar"))) {
        r.archivePath = baseNoExt + QStringLiteral(".tar.gz");
        QFile::remove(r.archivePath);
        program = QStringLiteral("tar");
        // -C <dir> . archives the dir contents with paths relative to it (a
        // leading "./" which every extractor strips). czf = create gzip file.
        args = { QStringLiteral("czf"), r.archivePath,
                 QStringLiteral("-C"), dir, QStringLiteral(".") };
    } else {
        r.ok = false;
        r.error = QObject::tr("Neither 'zip' nor 'tar' is available to create "
                              "the archive.");
        qCWarning(lcExport) << "No archiver found (zip/tar)";
        return r;
    }

    QProcess proc;
    // For zip we run inside the media dir so `.` means its contents.
    if (program == QStringLiteral("zip"))
        proc.setWorkingDirectory(dir);
    qCInfo(lcExport) << "Archiving" << r.fileCount << "media file(s) via"
                     << program << "->" << r.archivePath;
    proc.start(program, args);
    if (!proc.waitForStarted(3000)) {
        r.ok = false;
        r.error = QObject::tr("Could not start %1.").arg(program);
        return r;
    }
    // A month of snaps is small; 60s is a generous ceiling.
    if (!proc.waitForFinished(60000)) {
        proc.kill();
        proc.waitForFinished(2000);
        r.ok = false;
        r.error = QObject::tr("Archiving timed out.");
        return r;
    }
    if (proc.exitStatus() != QProcess::NormalExit || proc.exitCode() != 0) {
        r.ok = false;
        r.error = QObject::tr("%1 failed (code %2).")
                      .arg(program).arg(proc.exitCode());
        qCWarning(lcExport) << program << "failed:"
                            << proc.readAllStandardError();
        return r;
    }
    if (!QFile::exists(r.archivePath)) {
        r.ok = false;
        r.error = QObject::tr("Archive was not created.");
        return r;
    }
    r.ok = true;
    qCInfo(lcExport) << "Export complete:" << r.archivePath;
    return r;
}

} // namespace SnapClone::UI
