#include "ui/UiDebugFlags.hpp"
#include "debug/TraceControl.hpp"

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
    // Delegates to the trace-channel system (debug/TraceChannels.hpp), which is
    // now the single source of truth for which diagnostic categories are live.
    //
    // Two reasons this stopped owning its own rules:
    //   1. QLoggingCategory::setFilterRules REPLACES the entire rule set, so this
    //      function's partial string silently reset every category it didn't know
    //      about back to Qt's defaults — including any channel enabled from the
    //      CLI a moment earlier.
    //   2. Filter rules alone still pay to FORMAT each dropped message; the
    //      SC_TRACE macros skip the arguments entirely.
    //
    // The legacy "mobile display debug" switch maps onto the Layout channel,
    // which owns exactly the categories it used to list (vkeyboard, modal,
    // modaltest, dialog), so existing settings keep working.
    using namespace SnapClone::Debug;
    TraceControl::instance().setChannel(TraceChannel::Layout, mobileDisplayDebug(),
                                        QStringLiteral("UiDebugFlags (legacy)"));
}

} // namespace SnapClone::UI
