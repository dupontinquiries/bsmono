#pragma once

// ─── ui/SnapExporter.hpp ──────────────────────────────────────────────────────
//
// Collects the user's snap media files into a single archive they can save or
// share. Backs the Monthly Export Reminder's "Export as zip" action (and any
// future manual "export all" entry point).
//
// Archive format: prefers a real .zip via the `zip` CLI when present (what
// users expect and what every OS opens natively); falls back to .tar.gz via
// `tar` (effectively always available on Linux / postmarketOS) when `zip`
// isn't installed. The chosen output path's extension is corrected to match
// whichever tool ran, so the file is never mislabeled.
//
// Why shell out instead of a zip library: SnapClone deliberately carries no
// archive dependency, and Qt's only built-in zip writer is a private,
// version-fragile header. `zip`/`tar` are stable, ubiquitous CLIs and keep the
// binary dependency-free. The media files are already-encrypted-at-rest blobs
// or decrypted local copies depending on policy — SnapExporter does not
// re-encrypt; it archives exactly what's in the media folder.
//
// Threading: createArchive() runs the CLI synchronously with a bounded wait.
// Media folders are small (a month of snaps), so this is fast; callers may
// still want to show a brief "Exporting…" state. Returns a Result so the UI
// can report success (with the count) or a specific failure.
// ─────────────────────────────────────────────────────────────────────────────

#include <QString>

namespace SnapClone::UI {

class SnapExporter
{
public:
    struct Result {
        bool    ok          = false;
        int     fileCount   = 0;      ///< number of media files archived
        QString archivePath;          ///< final path (extension may differ from requested)
        QString error;                ///< human-readable failure reason when !ok
    };

    /// Archive every file in the snap media folder into `requestedPath`.
    /// The extension of the produced file is normalized to .zip or .tar.gz
    /// depending on which CLI was used (reflected in Result.archivePath).
    /// An empty media folder yields ok=true, fileCount=0 (caller can decide
    /// whether to surface "nothing to export").
    [[nodiscard]] static Result createArchive(const QString &requestedPath);

    /// The snap media folder (AppDataLocation/media). Exposed so the reminder
    /// card can count / thumbnail recent files without duplicating the path.
    [[nodiscard]] static QString mediaDir();

    /// Suggested default file name for the save dialog, e.g.
    /// "snapclone-export-2026-05.zip".
    [[nodiscard]] static QString suggestedFileName();

private:
    /// True if an executable named `tool` is on PATH.
    [[nodiscard]] static bool hasTool(const QString &tool);
};

} // namespace SnapClone::UI
