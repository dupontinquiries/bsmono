#include "ui/QrCodeRenderer.hpp"
#include "ui/Theme.hpp"

#include <QPainter>
#include <QApplication>
#include <QImage>
#include <QtMath>
#include <QDialog>
#include <QVBoxLayout>
#include <QLabel>
#include <QPushButton>
#include <QPropertyAnimation>
#include <QGraphicsOpacityEffect>
#include <QAbstractAnimation>
#include <QTimer>
#include <QGuiApplication>
#include <QScreen>
#include <QMouseEvent>

namespace SnapClone::UI {

// ─── makeIdQr ────────────────────────────────────────────────────────────────

QPixmap makeIdQr(const QString &id, int px)
{
    // Render at the screen's device pixel ratio so the QR is crisp on
    // hi-DPI displays (FP5 has ~2x DPR, most phones are 2-3x). Qt scales
    // the pixmap by 1/DPR on display, so a 2x render at DPR=2 produces
    // pixel-perfect modules. Without this, Qt upscales a low-res pixmap
    // and blurs the modules — visible as the soft edges on the FP5 QR.
    const qreal dpr  = qApp ? qApp->devicePixelRatio() : 1.0;
    const int cellSize = qMax(2, static_cast<int>(px * dpr) / 22);
    const int size     = cellSize * 21;

    const QColor bgColor(QLatin1String(Theme::BG_CARD()));
    QColor fgColor(QLatin1String(Theme::TEXT_PRIMARY()));
    if (bgColor.lightness() > 200)
        fgColor = QColor(QLatin1String(Theme::ACCENT_DIM()));

    // Contrast guard — WCAG 4.5:1 minimum
    auto luminance = [](const QColor &c) -> double {
        auto chan = [](double v) {
            v /= 255.0;
            return v <= 0.03928 ? v / 12.92 : qPow((v + 0.055) / 1.055, 2.4);
        };
        return 0.2126 * chan(c.red()) + 0.7152 * chan(c.green()) + 0.0722 * chan(c.blue());
    };
    const double L1 = luminance(fgColor);
    const double L2 = luminance(bgColor);
    const double contrast = (qMax(L1, L2) + 0.05) / (qMin(L1, L2) + 0.05);
    if (contrast < 4.5)
        fgColor = bgColor.lightness() > 128 ? QColor("#000000") : QColor("#FFFFFF");

    QPixmap pix(size, size);
    pix.fill(bgColor);
    QPainter p(&pix);
    p.setRenderHint(QPainter::Antialiasing, false);

    auto finder = [&](int ox, int oy) {
        p.fillRect(ox * cellSize, oy * cellSize, 7 * cellSize, 7 * cellSize, fgColor);
        p.fillRect((ox+1)*cellSize, (oy+1)*cellSize, 5*cellSize, 5*cellSize, bgColor);
        p.fillRect((ox+2)*cellSize, (oy+2)*cellSize, 3*cellSize, 3*cellSize, fgColor);
    };
    finder(0, 0);
    finder(14, 0);
    finder(0, 14);

    for (int i = 8; i < 13; ++i) {
        if ((i % 2) == 0) {
            p.fillRect(i * cellSize, 6 * cellSize, cellSize, cellSize, fgColor);
            p.fillRect(6 * cellSize, i * cellSize, cellSize, cellSize, fgColor);
        }
    }

    const QByteArray idBytes = id.toLatin1();
    int byteIdx = 0;
    for (int row = 0; row < 21; ++row) {
        for (int col = 8; col < 21; ++col) {
            if (row < 9 && col < 9) continue;
            if (row < 9 && col > 12) continue;
            if (row > 12 && col < 9) continue;
            const quint8 bit = byteIdx < static_cast<int>(idBytes.size())
                ? static_cast<quint8>((idBytes[byteIdx / 8] >> (byteIdx % 8)) & 1)
                : static_cast<quint8>((row ^ col) & 1);
            if (bit) p.fillRect(col*cellSize, row*cellSize, cellSize, cellSize, fgColor);
            ++byteIdx;
        }
    }
    p.end();

    // Smoothing for large displays (helps phone cameras detect modules)
    if (px > 200) {
        QImage img = pix.toImage();
        const int smoothSize = (size * 4) / 5;
        QImage small = img.scaled(smoothSize, smoothSize,
                                   Qt::IgnoreAspectRatio, Qt::SmoothTransformation);
        QImage smoothed = small.scaled(size, size,
                                        Qt::IgnoreAspectRatio, Qt::SmoothTransformation);
        pix = QPixmap::fromImage(smoothed);
    }

    QPixmap rounded(size, size);
    rounded.fill(Qt::transparent);
    QPainter rp(&rounded);
    rp.setRenderHint(QPainter::Antialiasing);
    rp.setBrush(bgColor);
    rp.setPen(Qt::NoPen);
    rp.drawRoundedRect(0, 0, size, size, 8, 8);
    rp.setCompositionMode(QPainter::CompositionMode_SourceIn);
    rp.drawPixmap(0, 0, pix);
    // Tag the pixmap with its device pixel ratio so Qt renders it 1:1
    // on the physical screen — no upscaling blur.
    rounded.setDevicePixelRatio(dpr);
    return rounded;
}

// ─── ExpandedQrOverlay ───────────────────────────────────────────────────────
// Internal class so we can override mouseRelease for tap-anywhere-to-dismiss

namespace {
class ExpandedQrOverlay : public QDialog
{
public:
    ExpandedQrOverlay(QWidget *topWindow, const QString &id)
        : QDialog(topWindow,
                  Qt::Dialog | Qt::FramelessWindowHint | Qt::WindowStaysOnTopHint)
    {
        setAttribute(Qt::WA_DeleteOnClose);
        setAttribute(Qt::WA_TranslucentBackground);
        setModal(true);

        const QColor scrim(QLatin1String(Theme::BG_DEEP()));
        setStyleSheet(QStringLiteral(
            "QDialog { background-color: rgba(%1,%2,%3,0.95); }")
            .arg(scrim.red()).arg(scrim.green()).arg(scrim.blue()));

        // Fill the actual window
        setGeometry(topWindow->geometry());

        auto *layout = new QVBoxLayout(this);
        const int margin = (topWindow->width() < 420) ? 16 : 32;
        layout->setContentsMargins(margin, margin, margin, margin);
        layout->setSpacing(16);
        layout->addStretch(1);

        // QR — fill 80% of usable width, capped at 70% of usable height
        const int qrSize = qMin(
            topWindow->width()  - margin * 2,
            (topWindow->height() - margin * 2) * 7 / 10);

        m_qrLabel = new QLabel(this);
        m_qrLabel->setPixmap(makeIdQr(id, qrSize));
        m_qrLabel->setAlignment(Qt::AlignCenter);
        m_qrLabel->setFixedSize(qrSize, qrSize);
        m_qrLabel->setStyleSheet(QStringLiteral(
            "background: %1; border-radius: 12px; padding: 8px;")
            .arg(QLatin1String(Theme::BG_CARD())));
        layout->addWidget(m_qrLabel, 0, Qt::AlignCenter);

        auto *idLbl = new QLabel(id, this);
        idLbl->setAlignment(Qt::AlignCenter);
        idLbl->setStyleSheet(QStringLiteral(
            "font-family: 'JetBrains Mono', monospace; font-size: 24px;"
            " font-weight: bold; color: %1; background: transparent;"
            " letter-spacing: 1px;")
            .arg(QLatin1String(Theme::TEXT_PRIMARY())));
        layout->addWidget(idLbl);

        auto *hint = new QLabel(tr("Tap outside QR or press Close"), this);
        hint->setAlignment(Qt::AlignCenter);
        hint->setStyleSheet(QStringLiteral(
            "color: %1; font-size: 13px; background: transparent;")
            .arg(QLatin1String(Theme::TEXT_DISABLED())));
        layout->addWidget(hint);

        layout->addStretch(1);

        auto *closeBtn = new QPushButton(tr("✕ Close"), this);
        closeBtn->setMinimumHeight(48);
        closeBtn->setStyleSheet(QStringLiteral(
            "QPushButton { background: %1; color: %2;"
            " border: 1px solid %3; border-radius: 12px;"
            " padding: 12px 24px; font-size: 15px; font-weight: 500; }"
            "QPushButton:hover   { background: %4; }"
            "QPushButton:pressed { background: %5; }")
            .arg(QLatin1String(Theme::BG_CARD()))
            .arg(QLatin1String(Theme::TEXT_PRIMARY()))
            .arg(QLatin1String(Theme::BORDER()))
            .arg(QLatin1String(Theme::BG_HOVER()))
            .arg(QLatin1String(Theme::BG_RAISED())));
        connect(closeBtn, &QPushButton::clicked, this, &QDialog::close);
        layout->addWidget(closeBtn);
    }

    void start()
    {
        // Two-phase animation: scrim fades in, then QR scales in
        auto *fx = new QGraphicsOpacityEffect(this);
        setGraphicsEffect(fx);
        fx->setOpacity(0.0);
        show();

        // Minimum-open time guard: until this timer fires, releases are
        // ignored. This catches the case where the user's opening tap's
        // release event lands here (m_sawOwnPress catches the common
        // case but Qt's mouse-from-touch synthesis on some platforms
        // can produce an unmatched release in our window).
        m_openGuardActive = true;
        QTimer::singleShot(300, this, [this]() {
            m_openGuardActive = false;
        });

        auto *fadeIn = new QPropertyAnimation(fx, "opacity", this);
        fadeIn->setDuration(220);
        fadeIn->setStartValue(0.0);
        fadeIn->setEndValue(1.0);
        fadeIn->setEasingCurve(QEasingCurve::OutCubic);
        fadeIn->start(QAbstractAnimation::DeleteWhenStopped);

        // V32.30: removed the nested QR-label opacity animation.
        // Previously we ran TWO QGraphicsOpacityEffect-driven fades —
        // one on the dialog (above) and a delayed second one on the
        // QR QLabel itself for a staggered feel. Qt's effect-composition
        // for nested QGraphicsOpacityEffects produces brief flicker
        // frames where the QR appears to vanish — both effects compete
        // for the same paint pipeline. User-reported "QR flickering and
        // disappearing" traced to exactly this. The single outer fade
        // is enough; the QR comes up cleanly with the rest of the
        // dialog content.
    }

protected:
    void mousePressEvent(QMouseEvent *e) override
    {
        // Mark that we OWN this press, so the matching release is one we
        // should honor. Without this, the user's opening tap on the small
        // QR (which spawned this dialog) generates a release event that
        // lands here — and we'd close immediately, making the dialog
        // appear to flash open and shut.
        m_sawOwnPress = true;
        QDialog::mousePressEvent(e);
    }

    void mouseReleaseEvent(QMouseEvent *e) override
    {
        // Two guards against close-on-opening-tap-release:
        //   1. m_openGuardActive: 300 ms after open, releases are
        //      unconditionally ignored. Catches the case where Qt
        //      synthesizes mouse events for touch in a way that routes
        //      release to us without a press.
        //   2. m_sawOwnPress: only honor releases that pair with a press
        //      we received ourselves. Defends against stray mouse-up.
        // Either is sufficient; both together are robust.
        if (m_openGuardActive || !m_sawOwnPress) {
            QDialog::mouseReleaseEvent(e);
            return;
        }
        // Tap on the scrim/background (not on any child widget) = close.
        // Don't include m_qrLabel here even though the hint says "tap
        // anywhere": including child widgets makes the overlay too easy
        // to dismiss accidentally during the user's natural finger motion.
        if (childAt(e->position().toPoint()) == nullptr) {
            close();
            return;
        }
        QDialog::mouseReleaseEvent(e);
    }

    void keyPressEvent(QKeyEvent *e) override
    {
        if (e->key() == Qt::Key_Escape) { close(); return; }
        QDialog::keyPressEvent(e);
    }

private:
    QLabel *m_qrLabel = nullptr;
    bool    m_sawOwnPress = false;
    bool    m_openGuardActive = false;
};
} // anon namespace

void showExpandedQr(QWidget *parent, const QString &id)
{
    QWidget *topWindow = parent ? parent->window() : nullptr;
    if (!topWindow) topWindow = parent;
    if (!topWindow) return;

    auto *overlay = new ExpandedQrOverlay(topWindow, id);
    overlay->start();
}

} // namespace SnapClone::UI
