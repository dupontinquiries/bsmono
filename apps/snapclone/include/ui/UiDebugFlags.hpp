#pragma once

// ─── ui/UiDebugFlags.hpp ───────────────────────────────────────────────────────
//
// Centralized switches for noisy, diagnostic-only behavior — settings the user
// can toggle in Settings → Debug to surface information that's helpful when
// reporting bugs but pure noise day-to-day.
//
// Why we need this:
//   Some categories of diagnostic information are valuable but VERY chatty
//   — for instance the virtual-keyboard watcher logs every rect change,
//   most of which are duplicate-empty-rect compositor signals; the modal
//   stack reports every tap with click coords and geometry. We don't want
//   to leave this stuff in default-on logs (they'd dominate the log file),
//   but we DO want users to be able to flip it on without rebuilding when
//   they're trying to diagnose a touch / layout issue.
//
// Design:
//   • Each flag is a settings key (under "debug/<name>"). Stored in
//     QSettings; persists across launches.
//   • A static apply() function called from main() reads ALL flags
//     and adjusts the corresponding QLoggingCategory filter rules.
//   • Settings UI emits a flagsChanged() signal; the watcher re-applies
//     when it fires.
//
// Adding a new flag:
//   1. Add a static accessor (e.g. `mobileDisplayDebug()`) that reads
//      from QSettings with a sensible default (typically false).
//   2. In apply(), add a `setFilterRules` call that switches the
//      relevant logging category on/off based on the flag.
//   3. Add a checkbox row in SettingsPage's "Debug" section that
//      writes the setting and calls apply().
//
// Why not a plain QSettings read at every log site:
//   QLoggingCategory caches its enabled-ness — checking the setting on
//   every qCInfo call would defeat the macro's compile-time filter. The
//   right level is a filter-rule registration at the application level,
//   which Qt handles per-category once.

#include <QObject>

namespace SnapClone::UI {

class UiDebugFlags : public QObject
{
    Q_OBJECT
public:
    /// Process-wide accessor.
    static UiDebugFlags &instance();

    /// Apply all current flag values to QLoggingCategory filter rules.
    /// Call once at application start AFTER QApplication is constructed
    /// (filter rules are global to QGuiApplication). Also called
    /// automatically from setMobileDisplayDebug.
    static void apply();

    /// True when verbose mobile-display debug logging is on. Covers:
    ///   • snapclone.ui.vkeyboard  — virtual keyboard rect changes
    ///   • snapclone.ui.modal      — modal stack tap dispatch
    ///   • snapclone.ui.modaltest  — modal test panel events
    ///   • snapclone.dialog        — MobileDialog lifecycle
    [[nodiscard]] static bool mobileDisplayDebug();
    static void setMobileDisplayDebug(bool on);

signals:
    /// Emitted whenever any flag changes. Settings UI can subscribe
    /// to re-render its checkboxes if the flag was changed elsewhere.
    void flagsChanged();

private:
    UiDebugFlags() = default;
};

} // namespace SnapClone::UI
