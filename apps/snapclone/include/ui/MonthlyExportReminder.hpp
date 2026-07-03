#pragma once

// ─── ui/MonthlyExportReminder.hpp ───────────────────────────────────────────
//
// "First of the month" reminder pattern: on the first time the app is opened
// in a new month, show a friendly card that prompts the user to export their
// snaps as a zip — surfacing little thumbnails of recent fun snaps as a
// "remember when?" prompt before they tap Export or Maybe Later.
//
// Why this exists:
//   Snaps are ephemeral by design (this app's defining characteristic), but
//   users do build up a folder over a month or two of usage. Without ever
//   surfacing this, the snaps stay invisible — and the user has no easy
//   path to keep what they liked. This prompt acknowledges the trade-off:
//   ephemeral by default, but not LOSSY by default.
//
// Why it's monthly, not daily:
//   Daily nag = annoyance, user disables it. Monthly = noticed-but-not-
//   irritating cadence; aligns with how people think about their photos
//   ("photos from last month").
//
// Power-user escape hatch:
//   Settings → Auto-sync to photos: when set (path or URL), snaps are
//   exported continuously and the reminder card never shows. This serves
//   the self-hosting / Syncthing crowd who don't want UI prompts — they
//   know what they want and they want it automated.
//
// State storage:
//   • `export/lastReminderShown` — YYYY-MM of last show (QSettings)
//   • `export/autoSyncPath`      — path or URL for continuous export
//   • `export/snoozeUntil`       — date string; if user tapped "Maybe
//     Later" we honour that decision for the rest of the month rather
//     than re-prompting on every launch.
//
// Public API:
//   • shouldShowThisMonth()   — pure function: true on the 1st of a new
//     month if not snoozed and no auto-sync configured.
//   • markShownThisMonth()    — call when reminder is rendered.
//   • snoozeForThisMonth()    — "Maybe later" handler.
//   • autoSyncPath() / setAutoSyncPath() — the export-elsewhere setting.

#include <QString>
#include <QDate>

namespace SnapClone::UI {

class MonthlyExportReminder
{
public:
    /// Returns true when:
    ///   • Today is the 1st of the month (or later but we haven't yet shown
    ///     this month and the user hasn't snoozed), AND
    ///   • No auto-sync path is configured (auto-sync replaces the reminder).
    [[nodiscard]] static bool shouldShowThisMonth();

    /// Record that we've shown the reminder for the current month. Subsequent
    /// calls to shouldShowThisMonth in this month return false.
    static void markShownThisMonth();

    /// User tapped "Maybe later" — same effect as markShownThisMonth (snoozes
    /// for the rest of the month).
    static void snoozeForThisMonth();

    /// Auto-sync destination — a local folder path OR a URL (rsync://, etc).
    /// Empty = no auto-sync.
    [[nodiscard]] static QString autoSyncPath();
    static void setAutoSyncPath(const QString &pathOrUrl);

private:
    static QString currentMonthKey();  ///< "2026-05"
};

} // namespace SnapClone::UI
