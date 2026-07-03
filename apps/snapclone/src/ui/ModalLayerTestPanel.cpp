#include "ui/ModalLayerTestPanel.hpp"
#include "ui/ModalSurface.hpp"
#include "ui/Theme.hpp"

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QPushButton>
#include <QLabel>
#include <QFrame>
#include <QTimer>
#include <QLoggingCategory>

namespace SnapClone::UI {

namespace {
Q_LOGGING_CATEGORY(lcModalTest, "snapclone.ui.modaltest")

// Per-layer visual identity for test widgets so we can SEE who's where.
struct LayerStyle {
    const char *name;
    const char *bg;
    const char *fg;
};
constexpr LayerStyle kStyles[] = {
    {"L1 Ambient",  "rgba(126, 87, 194, 200)",  "white"},  // purple, semi-trans
    {"L2 Hint",     "rgba(38, 166, 154, 220)",  "white"},  // teal
    {"L3 Dialog",   "rgba(244, 81, 30, 255)",   "white"},  // orange-red
    {"L4 Confirm",  "rgba(229, 57, 53, 255)",   "white"},  // red
    {"L5 Toast",    "rgba(67, 160, 71, 240)",   "white"},  // green
    {"L6 Critical", "rgba(0, 0, 0, 255)",       "white"},  // pure black
};
} // namespace

ModalLayerTestPanel::ModalLayerTestPanel(ModalSurface *surface, QWidget *parent)
    : QWidget(parent), m_surface(surface)
{
    setObjectName(QStringLiteral("modalLayerTestPanel"));
    // Sized as a sheet — fills most of the screen so test widgets have
    // room to render inside.
    setStyleSheet(QStringLiteral(
        "QWidget#modalLayerTestPanel {"
        "  background: %1; border-radius: 12px;"
        "  border: 2px solid %2;"
        "}")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::BORDER())));
    setMinimumSize(360, 540);
    resize(420, 640);

    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(16, 16, 16, 16);
    root->setSpacing(8);

    auto *title = new QLabel(tr("Modal Layer Test"), this);
    title->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 16px; font-weight: 700; "
        "background: transparent; border: none;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    root->addWidget(title);

    auto *blurb = new QLabel(tr(
        "Each button presents a colored test widget into a layer. "
        "Tap outside to dismiss (except L6 Critical, which must be "
        "dismissed explicitly via the Dismiss All button below)."), this);
    blurb->setWordWrap(true);
    blurb->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 11px; background: transparent; border: none;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    root->addWidget(blurb);

    // Six layer buttons, one row each.
    for (int i = 0; i < 6; ++i) {
        auto *btn = new QPushButton(
            tr("Present into %1").arg(QString::fromLatin1(kStyles[i].name)),
            this);
        btn->setStyleSheet(QStringLiteral(
            "QPushButton {"
            "  background: %1; color: %2;"
            "  border: 1px solid %3; border-radius: 6px;"
            "  padding: 10px; text-align: left; font-size: 12px;"
            "}"
            "QPushButton:hover { border-color: %4; }")
            .arg(QLatin1String(kStyles[i].bg))
            .arg(QLatin1String(kStyles[i].fg))
            .arg(QLatin1String(Theme::BORDER()))
            .arg(QLatin1String(Theme::ACCENT())));
        // V32.34+: the test harness used to pass blocksTouches=false
        // for Ambient/Hint/Toast (the "non-modal notification" layers),
        // mirroring how those layers SHOULD be used in production. But
        // with blocksTouches=false, the container container retains
        // WA_TransparentForMouseEvents and Qt skips the entire subtree
        // for mouse target picking — meaning the test widget itself
        // can't receive any mouse events, and tap-to-close doesn't work.
        // The test panel exists to verify that "tap-anywhere closes the
        // widget" works for L1..L5; the touch-passthrough behavior of
        // L1/L2/L5 is a separate property best demonstrated by the
        // "Stack all layers" button which presents into every layer.
        //
        // So: from here, the harness ALWAYS passes blocksTouches=true
        // (with the Critical exception, where dismissOnTap=false is the
        // whole point — explicit Close button only).
        const bool dismissOnTap  = (i != 5);
        const bool blocksTouches = (i != 5)  // Critical doesn't dismiss on tap, but DOES block
                                  ? true
                                  : true;    // Critical: still blocks (it's modal)
        // (Critical reaches both branches as true; spelled out so future
        // maintainers see the intent.)
        connect(btn, &QPushButton::clicked, this,
                [this, i, dismissOnTap, blocksTouches]() {
            presentTestWidget(i, QString::fromLatin1(kStyles[i].name),
                              dismissOnTap, blocksTouches);
        });
        root->addWidget(btn);
    }

    root->addStretch();

    // "Dismiss all" recovery button.
    auto *dismissAll = new QPushButton(tr("Dismiss All Layers"), this);
    dismissAll->setStyleSheet(QStringLiteral(
        "QPushButton {"
        "  background: %1; color: %2;"
        "  border: 1px solid %3; border-radius: 6px;"
        "  padding: 10px; font-size: 12px;"
        "}")
        .arg(QLatin1String(Theme::BG_CARD()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::WARNING())));
    connect(dismissAll, &QPushButton::clicked, this, [this]() {
        if (m_surface) m_surface->dismissAll();
    });
    root->addWidget(dismissAll);

    // "Stack all" button — presents one into each layer simultaneously
    // so we can SEE the z-order at a glance.
    auto *stackAll = new QPushButton(tr("Stack One Into Each Layer"), this);
    stackAll->setStyleSheet(QStringLiteral(
        "QPushButton {"
        "  background: %1; color: %2;"
        "  border: 1px solid %3; border-radius: 6px;"
        "  padding: 10px; font-size: 12px;"
        "}")
        .arg(QLatin1String(Theme::BG_CARD()))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(QLatin1String(Theme::ACCENT())));
    connect(stackAll, &QPushButton::clicked, this, [this]() {
        if (!m_surface) return;
        // Present from L1 to L6 in sequence — newer presentations end up on
        // top within their layer; cross-layer z-order is the structural
        // L1<L2<L3<L4<L5<L6 set by ModalSurface's ctor. Stagger by 80ms so
        // the user can see each pop in.
        //
        // V32.32 (revision): in the stack-all test, ALL six widgets are
        // dismissible-on-tap, including the Critical (L6). Why: the
        // Critical normally blocks dismiss-on-tap for everything below
        // (correct modal behavior), but that means a user clicking outside
        // L4 in the stack-all scenario gets their click eaten by L6 and
        // nothing happens — looks like a bug. With everything dismissible
        // here, tapping outside the topmost layer dismisses it and reveals
        // the next layer down, demonstrating the cascade. The standalone
        // "Present into L6 Critical" button (above) still creates a TRUE
        // non-dismissible critical for testing that specific behavior.
        for (int i = 0; i < 6; ++i) {
            QTimer::singleShot(i * 80, this, [this, i]() {
                if (!m_surface) return;
                // Toast (L5) still doesn't block touches — it's a notification.
                // Everything else does block, including Critical which is
                // now dismissible just for this stack-all test.
                const bool dismissOnTap = true;   // all dismissible in this test
                const bool blocks       = (i == 2 || i == 3 || i == 5);
                presentTestWidget(i, QString::fromLatin1(kStyles[i].name),
                                  dismissOnTap, blocks);
            });
        }
    });
    root->addWidget(stackAll);
}

void ModalLayerTestPanel::presentTestWidget(int layerIndex, const QString &label,
                                              bool dismissOnTap,
                                              bool blocksTouches)
{
    if (!m_surface) return;
    if (layerIndex < 0 || layerIndex >= 6) return;

    auto *w = new QFrame();
    w->setObjectName(QStringLiteral("modalTestWidget"));
    w->setStyleSheet(QStringLiteral(
        "QFrame#modalTestWidget {"
        "  background: %1; color: %2;"
        "  border-radius: 14px;"
        "  border: 2px solid white;"
        "}"
        "QFrame#modalTestWidget QLabel {"
        "  color: %2; background: transparent; border: none;"
        "  font-size: 14px; font-weight: 600;"
        "}")
        .arg(QLatin1String(kStyles[layerIndex].bg))
        .arg(QLatin1String(kStyles[layerIndex].fg)));

    auto *lay = new QVBoxLayout(w);
    lay->setContentsMargins(20, 16, 20, 16);
    auto *titleLabel = new QLabel(label, w);
    titleLabel->setAlignment(Qt::AlignCenter);
    lay->addWidget(titleLabel);
    auto *detail = new QLabel(
        QStringLiteral("dismissOnTap=%1\nblocksTouches=%2")
            .arg(dismissOnTap ? QStringLiteral("yes") : QStringLiteral("no"))
            .arg(blocksTouches ? QStringLiteral("yes") : QStringLiteral("no")),
        w);
    detail->setAlignment(Qt::AlignCenter);
    lay->addWidget(detail);

    // Per-layer placement for visual differentiation.
    const QSize hostSize = m_surface->size();
    switch (layerIndex) {
    case 0:   // Ambient — full-screen translucent overlay
        w->setGeometry(0, 0, hostSize.width(), hostSize.height());
        break;
    case 1:   // Hint — small pill, top-right
        w->resize(180, 60);
        w->move(hostSize.width() - 200, 80);
        break;
    case 2: { // Dialog — centered card, ~80% width, ~60% height
        const int dw = qMin(420, static_cast<int>(hostSize.width() * 0.8));
        const int dh = qMin(520, static_cast<int>(hostSize.height() * 0.6));
        w->resize(dw, dh);
        w->move((hostSize.width() - dw) / 2, (hostSize.height() - dh) / 2);
        break;
    }
    case 3: { // Confirm — smaller centered card
        const int dw = qMin(360, static_cast<int>(hostSize.width() * 0.7));
        const int dh = 220;
        w->resize(dw, dh);
        w->move((hostSize.width() - dw) / 2, (hostSize.height() - dh) / 2);
        break;
    }
    case 4:   // Toast — bottom-center pill
        w->resize(280, 60);
        w->move((hostSize.width() - 280) / 2, hostSize.height() - 100);
        // V32.34+: removed the 3-second auto-dismiss. In production
        // toast widgets DO auto-dismiss after a few seconds — that's
        // by design. But in the test harness it competes with the
        // manual tap-to-close test: the user couldn't tell whether
        // their tap closed it or the timer did. Keep the test focused
        // on tap dismissal; production toasts will install the timer
        // themselves at the call site.
        break;
    case 5: { // Critical — large dialog with no escape
        const int dw = qMin(360, static_cast<int>(hostSize.width() * 0.75));
        const int dh = 260;
        w->resize(dw, dh);
        w->move((hostSize.width() - dw) / 2, (hostSize.height() - dh) / 2);
        // Add an explicit close button since tap-outside is disabled.
        auto *closeBtn = new QPushButton(tr("Close (only way)"), w);
        closeBtn->setStyleSheet(QStringLiteral(
            "QPushButton { background: white; color: black;"
            " border-radius: 6px; padding: 6px 12px; font-weight: 600; }"));
        QObject::connect(closeBtn, &QPushButton::clicked, w, &QWidget::close);
        lay->addWidget(closeBtn);
        break;
    }
    }

    const auto layerEnum = static_cast<ModalSurface::Layer>(layerIndex + 1);
    qCInfo(lcModalTest) << "presenting test widget into"
                        << ModalSurface::layerName(layerEnum)
                        << "geom=" << w->geometry();
    // V32.34+: also close on tap-INSIDE-widget for the test panel. The
    // ModalSurface tap-outside-dismiss machinery only fires on taps in
    // the scrim/empty area; if the user taps the colored card itself
    // (which is a natural thing to do — it's a big visible affordance),
    // nothing happens, looking like the modal is stuck. For the test
    // harness we want every tap to be reactive so the user can verify
    // dismiss behavior without having to find the narrow scrim region.
    //
    // For layer 5 (Critical) we still skip — its whole point is "the
    // only way out is the explicit Close button" and breaking that
    // would defeat the test.
    if (layerIndex != 5) {
        // V32.34+: install event filter but do NOT set
        // WA_AcceptTouchEvents. Setting that attribute makes Qt deliver
        // raw TouchBegin/Update/End and STOP synthesizing mouse events
        // from those touches. Our event filter listens to
        // MouseButtonRelease (Qt's default synthesis from touch), so
        // setting WA_AcceptTouchEvents would prevent ANY tap from
        // closing the widget. The filter sees both MouseButtonRelease
        // and TouchEnd to handle either delivery path.
        w->installEventFilter(this);
        m_clickToClose.insert(w);
    }
    m_surface->present(w, layerEnum, dismissOnTap, blocksTouches);
}

bool ModalLayerTestPanel::eventFilter(QObject *obj, QEvent *ev)
{
    // V32.34+: tap-anywhere-to-close for test widgets.
    //
    // Accept both Press and Release events. Some platforms (notably
    // Phosh on some pmOS edge versions) deliver MouseButtonPress from
    // a touch but the matching Release goes elsewhere — closing on
    // Press is fine for the test harness since there's no useful
    // gesture inside the colored card. We also accept TouchEnd for
    // platforms that bypass mouse synthesis.
    //
    // The event is logged at info level when triggering close so the
    // user can confirm in journalctl that the filter is firing. If
    // they tap and DON'T see the log line, the event isn't reaching
    // our filter at all — then it's a z-order / hit-test problem
    // (the click is landing on a different widget than they think).
    const auto t = ev->type();
    const bool isTap = (t == QEvent::MouseButtonRelease
                     || t == QEvent::MouseButtonPress
                     || t == QEvent::TouchEnd);
    if (isTap) {
        if (auto *w = qobject_cast<QWidget *>(obj)) {
            if (m_clickToClose.contains(w)) {
                qCInfo(lcModalTest) << "tap-to-close fired on"
                                    << w->metaObject()->className()
                                    << "event=" << static_cast<int>(t);
                m_clickToClose.remove(w);
                w->close();
                return true;
            }
        }
    }
    return QWidget::eventFilter(obj, ev);
}

} // namespace SnapClone::UI
