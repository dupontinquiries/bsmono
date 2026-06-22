#include "ui/MobileDialog.hpp"
#include <QTimer>
#include <QHideEvent>
#include <QApplication>
#include "ui/Theme.hpp"
#include <QApplication>
#include <QScreen>
#include <QVBoxLayout>
#include <QMouseEvent>
#include <QTouchEvent>
#include <QDateTime>
#include <QResizeEvent>
#include <QLoggingCategory>
#include <functional>

Q_LOGGING_CATEGORY(lcDialog, "snapclone.dialog")

namespace SnapClone::UI {

namespace {
// Idle auto-dismiss tuning. Base is the no-tap window every popup gets; the
// context bonuses extend it where idle is legitimate; WantOut shrinks it when
// the user is clearly trying to bail.
constexpr int kAutoDismissBaseMs  = 15000;  // base "stuck popup" escape window
constexpr int kDismissPhotoBonus  = 30000;  // DismissContext::Photo  (+30s)
constexpr int kDismissPhoneBonus  = 120000; // DismissContext::PhoneCall (+120s)
constexpr int kDismissLongCallMs  = 24 * 60 * 60 * 1000; // PhoneCallLong: effectively never
constexpr int kWantOutPenaltyMs   = 5000;   // each rapid edge tap shaves this off
constexpr int kWantOutWindowMs    = 900;    // two edge taps within this = "rapid"
constexpr int kEdgeMarginPx       = 64;     // width of the edge "escape" band
} // namespace


// ── Form-factor detection ─────────────────────────────────────────────────────

bool isMobileFormFactor()
{
    // 1. Explicit environment variables set by Plasma Mobile / Phosh
    const QString plasma = qEnvironmentVariable("PLASMA_PLATFORM");
    if (plasma.contains(QLatin1String("phone"), Qt::CaseInsensitive)) return true;

    const QString gdkBk = qEnvironmentVariable("GDK_BACKEND");
    const QString xdgSt = qEnvironmentVariable("XDG_SESSION_DESKTOP");
    if (xdgSt.contains(QLatin1String("phosh"), Qt::CaseInsensitive)) return true;
    if (xdgSt.contains(QLatin1String("mobile"), Qt::CaseInsensitive)) return true;

    // 2. GNOME Mobile (libhandy / libadwaita breakpoint: width < 480px)
    const QString gtkTheme = qEnvironmentVariable("GTK_THEME");
    if (gtkTheme.contains(QLatin1String("mobile"), Qt::CaseInsensitive)) return true;

    // 3. Primary screen physical size — phones are narrower than ~600 px
    //    at the display's native DPI. Use logical pixels / DPR.
    const QScreen *screen = QApplication::primaryScreen();
    if (screen) {
        const qreal dpr = screen->devicePixelRatio();
        const QSize sz  = screen->availableSize();
        const int logW  = static_cast<int>(sz.width()  / dpr);
        if (logW < 500) return true;  // portrait phone
    }

    // 4. Main window is fullscreen (e.g. user hit fullscreen on desktop)
    for (const QWidget *w : QApplication::topLevelWidgets()) {
        if (w->isFullScreen()) return true;
    }

    return false;
}

// ── MobileDialog ──────────────────────────────────────────────────────────────

MobileDialog::MobileDialog(QWidget *parent)
    : QDialog(parent)
{
    m_isSheet = isMobileFormFactor();

    if (m_isSheet) {
        // Reparent to the top-level window so we're an overlay, not a
        // separate OS window. This prevents the "dialog behind main window"
        // problem on Plasma Mobile and Phosh.
        QWidget *root = parent ? parent->window() : nullptr;
        if (root) {
            setParent(root);
            setWindowFlags(Qt::Widget); // embedded widget, not OS window
        }
        setupAsBottomSheet();
        qCDebug(lcDialog) << "MobileDialog: inline bottom sheet mode";
    } else {
        // Desktop: use standard modeless dialog flags
        setWindowFlags(Qt::Dialog | Qt::WindowCloseButtonHint);
        // CRITICAL: set a sensible default size — without this Qt uses sizeHint()
        // which is too narrow for a camera viewfinder dialog. 480×640 is a good
        // mobile-form-factor reference; users can resize from there.
        setMinimumSize(420, 600);
        resize(480, 720);
        qCDebug(lcDialog) << "MobileDialog: desktop window mode";
    }
}

void MobileDialog::setupAsBottomSheet()
{
    // The scrim covers the parent window and dims it
    QWidget *root = parentWidget() ? parentWidget()->window() : nullptr;
    if (!root) return;

    m_scrim = new QWidget(root);
    m_scrim->setObjectName(QStringLiteral("dialogScrim"));
    m_scrim->setGeometry(root->rect());
    m_scrim->setStyleSheet(QStringLiteral(
        "background: rgba(0,0,0,0.55);"));
    // Accept touch directly so tap-outside-to-dismiss works on mobile.
    // Without this, the touch would route up to SwipeNavigator (the ancestor
    // with WA_AcceptTouchEvents) which would consume it, blocking Qt's
    // mouse-event synthesis and leaving the scrim non-interactive on touch.
    m_scrim->setAttribute(Qt::WA_AcceptTouchEvents);
    m_scrim->installEventFilter(this); // tap outside = reject
    m_scrim->hide();

    // Position sheet at bottom, full width, up to 85% height
    repositionSheet();
    setStyleSheet(QStringLiteral(
        "MobileDialog, QDialog { background: %1; border-radius: 20px 20px 0 0; }")
        .arg(QLatin1String(Theme::BG_RAISED())));

    // Drag handle pill at the top
    auto *handle = new QWidget(this);
    handle->setFixedSize(40, 4);
    handle->setStyleSheet(QStringLiteral(
        "background: %1; border-radius: 2px;")
        .arg(QLatin1String(Theme::TEXT_DISABLED())));
    handle->move((width() - 40) / 2, 10);
    handle->show();
}

void MobileDialog::repositionSheet()
{
    QWidget *root = parentWidget() ? parentWidget()->window() : nullptr;
    if (!root) return;
    const int rw = root->width();
    const int rh = root->height();
    const int sh = qMin(static_cast<int>(rh * 0.88), rh - 40);
    setGeometry(0, rh, rw, sh); // starts off-screen below (before animation)
    hide();
}

void MobileDialog::animateSheetIn()
{
    QWidget *root = parentWidget() ? parentWidget()->window() : nullptr;
    if (!root) return;

    const int rh  = root->height();
    const int sh  = height();
    const int yOn = rh - sh;

    m_scrim->setGeometry(root->rect());
    m_scrim->show();
    m_scrim->raise();

    // ── Safety: auto-dismiss after 15s if user can't close it ───────────
    // Last-line-of-defense if any code path leaves the scrim visible without
    // a dialog on top. The user said "popup screens can never be closed
    // on the phone" — this guarantees they can ALWAYS get out within 15s
    // by waiting, even if every other dismissal path is broken.
    if (m_stuckTimer) m_stuckTimer->stop();
    else { m_stuckTimer = new QTimer(this); m_stuckTimer->setSingleShot(true); }
    disconnect(m_stuckTimer, nullptr, this, nullptr);
    connect(m_stuckTimer, &QTimer::timeout, this, [this]() {
        // Only act if the dialog is still visible — accept() / reject()
        // already stop the timer, so this fires only if something went wrong.
        if (isVisible() || (m_scrim && m_scrim->isVisible())) {
            qWarning("MobileDialog: stuck-dialog safety timer fired — auto-rejecting");
            reject();
        }
    });
    m_stuckTimer->start(effectiveDismissMs());  // base + context bonus (see setDismissContext)
    raise();
    show();

    // Slide up from bottom
    m_anim = new QPropertyAnimation(this, "pos", this);
    m_anim->setDuration(Theme::ANIM_NORMAL);
    m_anim->setStartValue(QPoint(0, rh));
    m_anim->setEndValue(QPoint(0, yOn));
    m_anim->setEasingCurve(QEasingCurve::OutCubic);
    m_anim->start(QAbstractAnimation::DeleteWhenStopped);

    // V32.32: post-animation diagnostic. Dump the geometry, parent, and
    // visibility chain so we can see why "visible=1" in logs doesn't
    // match what the user sees on screen. Common causes:
    //   • dialog landed off-screen (yOn miscalculated)
    //   • scrim isn't where it should be (wrong parent)
    //   • another widget is raised above the dialog after open
    QTimer::singleShot(Theme::ANIM_NORMAL + 50, this, [this, root]() {
        qInfo("MobileDialog::animateSheetIn settled: "
              "dialog geom=%dx%d@(%d,%d) visible=%d "
              "scrim geom=%dx%d@(%d,%d) visible=%d "
              "root=%dx%d parent=%s",
              width(), height(), x(), y(), isVisible() ? 1 : 0,
              m_scrim ? m_scrim->width()  : -1,
              m_scrim ? m_scrim->height() : -1,
              m_scrim ? m_scrim->x()      : -1,
              m_scrim ? m_scrim->y()      : -1,
              m_scrim && m_scrim->isVisible() ? 1 : 0,
              root->width(), root->height(),
              parentWidget() ? parentWidget()->metaObject()->className() : "null");
    });
}

void MobileDialog::animateSheetOut(std::function<void()> then)
{
    QWidget *root = parentWidget() ? parentWidget()->window() : nullptr;
    const int rh  = root ? root->height() : height() * 2;

    if (m_anim) { m_anim->stop(); m_anim = nullptr; }

    m_anim = new QPropertyAnimation(this, "pos", this);
    m_anim->setDuration(Theme::ANIM_FAST);
    m_anim->setStartValue(pos());
    m_anim->setEndValue(QPoint(0, rh));
    m_anim->setEasingCurve(QEasingCurve::InCubic);

    connect(m_anim, &QPropertyAnimation::finished, this, [this, then]() {
        if (m_scrim) m_scrim->hide();
        hide();
        if (then) then();
    });
    m_anim->start(QAbstractAnimation::DeleteWhenStopped);
}

void MobileDialog::open()
{
    if (m_isSheet) {
        repositionSheet();
        animateSheetIn();
        // Don't call QDialog::open() — that would create an event loop
    } else {
        QDialog::open();
    }
}

void MobileDialog::reject()
{
    if (m_isSheet) {
        animateSheetOut([this]() {
            QDialog::reject(); // emit rejected() after animation
        });
    } else {
        QDialog::reject();
    }
}

void MobileDialog::accept()
{
    // Symmetric with reject(). Without this, callers that emit a result via
    // accept() (e.g. SnapComposerDialog's Send button) would call the base
    // QDialog::accept() which hides the dialog but leaves the scrim visible
    // — the user would see a dimmed screen with no UI on top and no way to
    // dismiss it.
    if (m_isSheet) {
        animateSheetOut([this]() {
            QDialog::accept(); // emit accepted() after animation
        });
    } else {
        QDialog::accept();
    }
}

MobileDialog::~MobileDialog()
{
    // The scrim is parented to the root window, NOT to us. Without explicit
    // cleanup the scrim is leaked AND remains visible after we're destroyed
    // — which is what causes the stuck-dim-screen bug when callers use
    // WA_DeleteOnClose without waiting for animation completion.
    if (m_scrim) {
        m_scrim->hide();
        m_scrim->deleteLater();
        m_scrim = nullptr;
    }
}

void MobileDialog::hideEvent(QHideEvent *e)
{
    QDialog::hideEvent(e);
    // Whenever the dialog hides for ANY reason (accept, reject, deleteLater,
    // direct hide(), parent destruction), hide the scrim too. Belt-and-suspenders
    // for code paths that bypass our accept/reject overrides.
    if (m_scrim) m_scrim->hide();
    if (m_stuckTimer) m_stuckTimer->stop();
}

void MobileDialog::resizeEvent(QResizeEvent *e)
{
    QDialog::resizeEvent(e);
    if (m_isSheet && m_scrim) {
        QWidget *root = parentWidget() ? parentWidget()->window() : nullptr;
        if (root) m_scrim->setGeometry(root->rect());
    }
}

bool MobileDialog::eventFilter(QObject *o, QEvent *e)
{
    // Tap on the scrim = dismiss. We listen for both mouse release (desktop
    // and Qt's mouse-from-touch synthesis) AND TouchEnd directly, because
    // synthesis is unreliable when ancestors with WA_AcceptTouchEvents
    // (like SwipeNavigator) consume the touch first. The scrim now has
    // WA_AcceptTouchEvents itself, so it gets the touch directly.
    if (o == m_scrim) {
        if (e->type() == QEvent::MouseButtonRelease
         || e->type() == QEvent::TouchEnd) {
            reject();
        }
    }
    return QDialog::eventFilter(o, e);
}

// Reset the safety timer on any user interaction with the dialog itself.
// Otherwise the dialog would auto-dismiss mid-edit on long compositions.
// We use the dialog's own event() override (not eventFilter) so we don't
// have to install filters on every child widget.
//
// Two behaviors:
//   • Engaging with the dialog (a press on its body, a key, a wheel) RESETS the
//     window to full — the user is clearly still using it.
//   • Rapid taps in the edge "escape" band are read as WantOut: the user is
//     trying to bail, so each one SHAVES kWantOutPenaltyMs off the remaining
//     time (and once it hits zero, gets them out immediately) instead of
//     extending. A single edge tap is ambiguous, so it neither extends nor
//     shrinks.
bool MobileDialog::event(QEvent *e)
{
    if (m_stuckTimer && m_stuckTimer->isActive()) {
        switch (e->type()) {
        case QEvent::MouseButtonPress:
        case QEvent::TouchBegin: {
            QPointF pos;
            bool havePos = false;
            if (e->type() == QEvent::MouseButtonPress) {
                pos = static_cast<QMouseEvent *>(e)->position();
                havePos = true;
            } else {
                const auto pts = static_cast<QTouchEvent *>(e)->points();
                if (!pts.isEmpty()) { pos = pts.first().position(); havePos = true; }
            }
            if (havePos && inEscapeZone(pos)) {
                const qint64 now   = QDateTime::currentMSecsSinceEpoch();
                const bool   rapid = m_lastEdgeTapMs != 0
                                  && (now - m_lastEdgeTapMs) < kWantOutWindowMs;
                m_lastEdgeTapMs = now;
                if (rapid) {
                    const int next = m_stuckTimer->remainingTime() - kWantOutPenaltyMs;
                    if (next <= 0) reject();
                    else           m_stuckTimer->start(next);
                }
                // single (non-rapid) edge tap: leave the timer untouched
            } else {
                m_stuckTimer->start(effectiveDismissMs()); // engaging → full reset
                m_lastEdgeTapMs = 0;
            }
            break;
        }
        case QEvent::KeyPress:
        case QEvent::Wheel:
            m_stuckTimer->start(effectiveDismissMs());
            m_lastEdgeTapMs = 0;
            break;
        default:
            break;
        }
    }
    return QDialog::event(e);
}

int MobileDialog::effectiveDismissMs() const
{
    return kAutoDismissBaseMs + m_dismissBonusMs;
}

bool MobileDialog::inEscapeZone(const QPointF &p) const
{
    return p.x() < kEdgeMarginPx || p.x() > width()  - kEdgeMarginPx
        || p.y() < kEdgeMarginPx || p.y() > height() - kEdgeMarginPx;
}

void MobileDialog::setDismissContext(DismissContext ctx)
{
    switch (ctx) {
    case DismissContext::Photo:         m_dismissBonusMs = kDismissPhotoBonus; break;
    case DismissContext::PhoneCall:     m_dismissBonusMs = kDismissPhoneBonus; break;
    case DismissContext::PhoneCallLong: m_dismissBonusMs = kDismissLongCallMs; break;
    case DismissContext::Normal:
    default:                            m_dismissBonusMs = 0;                  break;
    }
    // If already showing, re-arm with the new window so a live context change
    // (e.g. a call starting or ending) takes effect immediately.
    if (m_stuckTimer && m_stuckTimer->isActive())
        m_stuckTimer->start(effectiveDismissMs());
}

void MobileDialog::pokeAutoDismiss()
{
    if (m_stuckTimer && m_stuckTimer->isActive())
        m_stuckTimer->start(effectiveDismissMs());
}

} // namespace SnapClone::UI
