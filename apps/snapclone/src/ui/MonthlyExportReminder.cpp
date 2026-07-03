#include "ui/MonthlyExportReminder.hpp"

#include <QSettings>
#include <QDate>

namespace SnapClone::UI {

namespace {
constexpr const char *kKeyLastShown   = "export/lastReminderShown";
constexpr const char *kKeyAutoSync    = "export/autoSyncPath";
} // namespace

QString MonthlyExportReminder::currentMonthKey()
{
    return QDate::currentDate().toString(QStringLiteral("yyyy-MM"));
}

bool MonthlyExportReminder::shouldShowThisMonth()
{
    QSettings s;
    // Auto-sync configured → no reminder. The user has already opted into
    // continuous export, which is the more powerful version of the prompt.
    if (!s.value(QLatin1String(kKeyAutoSync)).toString().isEmpty()) {
        return false;
    }
    // Already shown / snoozed this month → no.
    const QString lastShown = s.value(QLatin1String(kKeyLastShown)).toString();
    if (lastShown == currentMonthKey()) {
        return false;
    }
    // Otherwise — show once per month, whenever the user next opens the app
    // in a new month. We don't restrict to "exactly on the 1st" because
    // many users won't open the app on day 1 of every month, and we'd
    // rather catch them on day 3 than skip the entire cycle.
    return true;
}

void MonthlyExportReminder::markShownThisMonth()
{
    QSettings s;
    s.setValue(QLatin1String(kKeyLastShown), currentMonthKey());
}

void MonthlyExportReminder::snoozeForThisMonth()
{
    // Same as markShownThisMonth — "Maybe later" is functionally "I'm not
    // doing this right now, don't ask again until next month."
    markShownThisMonth();
}

QString MonthlyExportReminder::autoSyncPath()
{
    QSettings s;
    return s.value(QLatin1String(kKeyAutoSync)).toString();
}

void MonthlyExportReminder::setAutoSyncPath(const QString &pathOrUrl)
{
    QSettings s;
    if (pathOrUrl.isEmpty()) {
        s.remove(QLatin1String(kKeyAutoSync));
    } else {
        s.setValue(QLatin1String(kKeyAutoSync), pathOrUrl);
    }
}

} // namespace SnapClone::UI
