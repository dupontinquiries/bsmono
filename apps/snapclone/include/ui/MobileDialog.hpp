#pragma once
// ─── MobileDialog.hpp ─────────────────────────────────────────────────────────
//
// Adaptive dialog base class. On desktop it behaves like a normal QDialog.
// On Plasma Mobile / GNOME Mobile / any fullscreen environment it insets
// itself as a bottom sheet inside the main window — no separate OS window.
//
// Why: On mobile Linux, separate OS windows (Qt::Dialog) frequently appear
// behind the main window, lose focus, or fail to get keyboard input because
// the compositor treats them as floating panels rather than child surfaces.
// Bottom-sheet inlining avoids all of these issues.
//
// Usage: replace QDialog with MobileDialog. The API is identical for:
//   - open() / close() / accept() / reject()
//   - accepted / rejected signals
//   - result()
//
// Migration: existing QDialog subclasses just change the base class.
// exec() is intentionally NOT overridden — callers should use open() instead.

#include <QWidget>
#include <QDialog>
#include <QPropertyAnimation>
#include <QPointer>
#include <QGraphicsOpacityEffect>

QT_BEGIN_NAMESPACE
class QVBoxLayout;
class QTimer;
class QHideEvent;
QT_END_NAMESPACE

namespace SnapClone::UI {

/// Returns true if we should show dialogs as inlined bottom sheets.
/// True on: Plasma Mobile, Phosh, GNOME Mobile, or any fullscreen window.
bool isMobileFormFactor();

class MobileDialog : public QDialog
{
    Q_OBJECT
public:
    explicit MobileDialog(QWidget *parent = nullptr);

    /// Show the dialog. On mobile: animates up as a bottom sheet.
    /// On desktop: calls QDialog::open() (non-blocking, preferred over exec).
    void open() override;

    /// Accept the dialog (also handles sheet close animation on mobile).
    /// MUST be overridden symmetrically with reject() — without this, callers
    /// that call accept() leak the scrim widget (it stays visible while the
    /// dialog itself hides, dimming the screen with no UI on top).
    void accept() override;

    /// Reject the dialog (also handles sheet close animation on mobile).
    void reject() override;

    ~MobileDialog() override;

protected:
    void resizeEvent(QResizeEvent *e) override;
    void hideEvent(QHideEvent *e) override;
    bool eventFilter(QObject *o, QEvent *e) override;
    /// Resets the safety auto-dismiss timer on user interaction so
    /// long-form editing (e.g. SnapComposerDialog) isn't cut short.
    bool event(QEvent *e) override;

    /// Tune the idle auto-dismiss for this dialog's context. The base no-tap
    /// window is 15s; some contexts legitimately keep the user idle-handed
    /// longer and add a bonus:
    ///   Photo     — inspecting a shot before sending      (+30s)
    ///   PhoneCall — on a voice call (you listen/talk, not tapping) (+120s)
    ///   PhoneCallLong — a call with the "enable long calls" advanced setting on;
    ///                   effectively never auto-dismisses (exit via End/Close).
    /// Default is Normal (no bonus). Safe to change live (e.g. when a call
    /// starts/ends); re-arms the running timer with the new window.
    enum class DismissContext { Normal, Photo, PhoneCall, PhoneCallLong };
    void setDismissContext(DismissContext ctx);

    /// Reset the idle window to its full length right now. Use for activity the
    /// event system can't see as a tap — e.g. an inbound voice frame on a call,
    /// so the call isn't auto-dismissed while the peer is talking and you're not
    /// touching the screen. No-op if the timer isn't currently armed.
    void pokeAutoDismiss();

private:
    void setupAsBottomSheet();
    void animateSheetIn();
    void animateSheetOut(std::function<void()> then = nullptr);
    void repositionSheet();

    /// Effective idle window in ms: base 15s + the current context bonus.
    int  effectiveDismissMs() const;
    /// True if a point (dialog-local coords) lands in the edge "escape" band —
    /// where a user trying to bail tends to tap (close X, back edge, swipe-down).
    bool inEscapeZone(const QPointF &p) const;

    bool            m_isSheet   = false;
    // QPointer because m_scrim is parented to the root window (MainWindow),
    // not to this dialog. If the root is destroyed before we are (e.g. app
    // shutdown sequence where MainWindow goes first), the scrim is auto-
    // deleted via parent ownership, leaving us with a dangling pointer.
    // QPointer ensures our destructor's null-check actually catches it.
    QPointer<QWidget>            m_scrim;
    // QPointer is critical: QPropertyAnimation::start(DeleteWhenStopped) makes
    // the animation self-delete when it finishes. The raw pointer would not
    // be nulled, leaving a dangling reference. Next call to animateSheetIn /
    // animateSheetOut would deref it via `if (m_anim) m_anim->stop();` and
    // crash. QPointer auto-nulls on destruction so the if-guard works.
    QPointer<QPropertyAnimation> m_anim;

    /// Safety auto-dismiss timer. If the scrim stays visible without
    /// any user input for kStuckDialogTimeoutMs, force-close the dialog.
    /// Defends against any future code path that hides the dialog without
    /// hiding the scrim — last-line-of-defense so the user is never stuck
    /// on a darkened screen they can't dismiss.
    QTimer        *m_stuckTimer = nullptr;
    /// Idle-window bonus (ms) added to the 15s base, set by setDismissContext.
    int            m_dismissBonusMs = 0;
    /// Timestamp (ms since epoch) of the last edge-zone tap, for WantOut
    /// rapid-tap detection. 0 = none yet.
    qint64         m_lastEdgeTapMs = 0;
};

} // namespace SnapClone::UI
