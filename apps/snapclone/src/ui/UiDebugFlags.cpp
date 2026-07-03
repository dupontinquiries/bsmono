#include "ui/UiDebugFlags.hpp"

#include <QSettings>
#include <QLoggingCategory>

namespace SnapClone::UI {

namespace {
constexpr const char *kKeyMobileDisplay = "debug/mobileDisplay";

// Categories that compose the "mobile display debug" group. Adding a
// new category here is enough to bring it under the same toggle.
const char *kMobileDisplayCategories[] = {
    "snapclone.ui.vkeyboard",
    "snapclone.ui.modal",
    "snapclone.ui.modaltest",
    "snapclone.dialog",
};
} // namespace

UiDebugFlags &UiDebugFlags::instance()
{
    static UiDebugFlags s;
    return s;
}

bool UiDebugFlags::mobileDisplayDebug()
{
    QSettings s;
    return s.value(QLatin1String(kKeyMobileDisplay), false).toBool();
}

void UiDebugFlags::setMobileDisplayDebug(bool on)
{
    QSettings s;
    s.setValue(QLatin1String(kKeyMobileDisplay), on);
    apply();
    emit instance().flagsChanged();
}

void UiDebugFlags::apply()
{
    // Build a filter-rules string. QLoggingCategory takes a multi-line
    // format like:
    //   snapclone.ui.vkeyboard.info=true
    //   snapclone.ui.vkeyboard.debug=true
    //
    // When the flag is OFF, we set both info and debug to FALSE — that
    // silences info() AND debug() level output. warning() and critical()
    // stay on regardless (real problems still surface). When ON, we
    // set them back to TRUE so the diagnostic chatter resumes.
    //
    // Why explicit booleans (not just commenting): Qt's default is
    // "info+ enabled, debug disabled". Once we've EVER applied a rule
    // for a category we have to explicitly state both info and debug,
    // otherwise the previous run's setting leaks.
    const bool mobOn = mobileDisplayDebug();
    QString rules;
    for (const char *cat : kMobileDisplayCategories) {
        const QString c = QString::fromLatin1(cat);
        // info default: ON when flag ON, OFF when flag OFF
        rules += c + QStringLiteral(".info=")
              + (mobOn ? QStringLiteral("true") : QStringLiteral("false"))
              + QStringLiteral("\n");
        // debug default: same.
        rules += c + QStringLiteral(".debug=")
              + (mobOn ? QStringLiteral("true") : QStringLiteral("false"))
              + QStringLiteral("\n");
    }
    QLoggingCategory::setFilterRules(rules);
}

} // namespace SnapClone::UI
