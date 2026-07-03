#include "ui/ChevronConfirm.hpp"
#include "ui/UxGuide.hpp"
#include "ui/Theme.hpp"
#include <QPainter>
#include <QPainterPath>
#include <QPushButton>
#include <QLabel>
#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QPropertyAnimation>
#include <QGraphicsOpacityEffect>
#include <QTimer>
#include <QMouseEvent>
#include <QAbstractAnimation>
#include <QLoggingCategory>
#include "ui/TouchFeedback.hpp"

Q_LOGGING_CATEGORY(lcChevron, "snapclone.ux.confirm")

namespace SnapClone::UI {

static constexpr int kAutoDismissMs = 4000;
static constexpr int kHoldDurationMs = 1200;
static constexpr int kChevronH = 10;   // height of the pointing arrow
static constexpr int kRadius   = 14;   // panel corner radius

// ── Static factory ────────────────────────────────────────────────────────────

void ChevronConfirm::present(QWidget              *anchor,
                          const QString        &label,
                          const QString        &featureKey,
                          std::function<void()> onConfirm)
{
    const auto &fs = UxGuide::instance().familiarityFor(featureKey);
    const int score = fs.decayed();

    // Expert users: just run the action, no confirmation
    if (score >= 80) {
        qCDebug(lcChevron) << featureKey << ": expert, skipping confirm";
        UxGuide::instance().recordUse(featureKey);
        onConfirm();
        return;
    }

    // Find a suitable parent (top-level window)
    QWidget *root = anchor ? anchor->window() : nullptr;
    if (!root) { onConfirm(); return; }

    const Mode mode = (score < 40) ? Mode::Tap : Mode::Hold;
    auto *panel = new ChevronConfirm(root, mode, label, std::move(onConfirm));

    // Position below the anchor button (or above if near screen bottom)
    const QPoint anchorGlobal = anchor->mapToGlobal(QPoint(0, anchor->height()));
    const QPoint inRoot = root->mapFromGlobal(anchorGlobal);
    const int panelW = qMin(root->width() - 32, 320);
    const int panelX = qBound(16, inRoot.x() - panelW / 2 + anchor->width() / 2,
                               root->width() - panelW - 16);

    // Decide above/below based on space
    const bool fitsBelow = (inRoot.y() + 120 < root->height() - 20);
    panel->m_pointUp = fitsBelow;

    const int panelY = fitsBelow
        ? inRoot.y() + 4
        : inRoot.y() - anchor->height() - 120 - 4;

    panel->setGeometry(panelX, panelY, panelW, 110 + kChevronH);
    // WA_StyledBackground ensures the panel is composited correctly
    // WA_NoSystemBackground prevents the window background from showing through
    panel->setAttribute(Qt::WA_StyledBackground, true);
    panel->animateIn();
    qCDebug(lcChevron) << featureKey << ": mode=" << static_cast<int>(mode)
                       << "score=" << score;
}

// ── Constructor ───────────────────────────────────────────────────────────────

ChevronConfirm::ChevronConfirm(QWidget *parent, Mode mode,
                               const QString &label,
                               std::function<void()> onConfirm)
    : QWidget(parent, Qt::Widget)
    , m_mode(mode)
    , m_onConfirm(std::move(onConfirm))
{
    setAttribute(Qt::WA_TranslucentBackground);
    setAttribute(Qt::WA_NoSystemBackground);
    setAutoFillBackground(false);

    if (mode == Mode::Tap)  buildTapUi(label);
    if (mode == Mode::Hold) buildHoldUi(label);

    startAutoDismiss();
    // show() before raise() — raise() only works on visible widgets
    QWidget::show();
    // Raise above ALL siblings including scroll areas, search bars, and overlays.
    // Must be called after show() so the widget is in the sibling list.
    raise();
}

// ── Tap UI ────────────────────────────────────────────────────────────────────

void ChevronConfirm::buildTapUi(const QString &label)
{
    const int top = m_pointUp ? kChevronH : 0;
    auto *inner = new QWidget(this);
    inner->setGeometry(0, top, width(), height() - kChevronH);
    inner->setStyleSheet(QStringLiteral(
        "background: %1; border-radius: %2px;")
        .arg(QLatin1String(Theme::BG_CARD()))
        .arg(kRadius));

    auto *lay = new QVBoxLayout(inner);
    lay->setContentsMargins(16, 14, 16, 14);
    lay->setSpacing(10);

    auto *lbl = new QLabel(label, inner);
    lbl->setWordWrap(true);
    lbl->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 14px; background: transparent;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    lay->addWidget(lbl);

    auto *btnRow = new QHBoxLayout;
    btnRow->setSpacing(8);

    auto *cancelBtn = new QPushButton(tr("Cancel"), inner);
    cancelBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background: %1; color: %2; border-radius: 8px;"
        "  font-size: 14px; padding: 8px 16px; border: 1px solid %3; }"
        "QPushButton:pressed { background: %4; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(QLatin1String(Theme::BG_DEEP())));
    connect(cancelBtn, &QPushButton::clicked, this, &ChevronConfirm::dismiss);

    auto *confirmBtn = new QPushButton(tr("Confirm"), inner);
    confirmBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background: %1; color: white; border-radius: 8px;"
        "  font-size: 14px; font-weight: 600; padding: 8px 16px; }"
        "QPushButton:pressed { background: %2; }")
        .arg(QLatin1String(Theme::ACCENT()))
        .arg(QLatin1String(Theme::ACCENT_DIM())));
    TouchFeedback::install(confirmBtn, TouchFeedback::Effect::Pulse);
    connect(confirmBtn, &QPushButton::clicked, this, [this]() {
        UxGuide::instance().recordUse(QString{}); // generic
        if (m_onConfirm) m_onConfirm();
        dismiss();
    });

    btnRow->addWidget(cancelBtn);
    btnRow->addWidget(confirmBtn);
    lay->addLayout(btnRow);
}

// ── Hold UI ───────────────────────────────────────────────────────────────────

void ChevronConfirm::buildHoldUi(const QString &label)
{
    const int top = m_pointUp ? kChevronH : 0;
    auto *inner = new QWidget(this);
    inner->setGeometry(0, top, width(), height() - kChevronH);
    inner->setStyleSheet(QStringLiteral(
        "background: %1; border-radius: %2px;")
        .arg(QLatin1String(Theme::BG_CARD()))
        .arg(kRadius));

    auto *lay = new QVBoxLayout(inner);
    lay->setContentsMargins(16, 14, 16, 14);
    lay->setSpacing(10);

    auto *lbl = new QLabel(
        label + QStringLiteral("\n") + tr("Hold to confirm"), inner);
    lbl->setWordWrap(true);
    lbl->setAlignment(Qt::AlignCenter);
    lbl->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 14px; background: transparent;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    lay->addWidget(lbl);

    // The hold button — pressing and holding fills a progress arc
    auto *holdBtn = new QPushButton(tr("Hold to confirm"), inner);
    holdBtn->setFixedHeight(44);
    holdBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background: %1; color: %2; border-radius: 10px;"
        "  font-size: 13px; font-weight: 500; }"
        "QPushButton:pressed { background: %3; }")
        .arg(QLatin1String(Theme::ACCENT()))
        .arg(QStringLiteral("white"))
        .arg(QLatin1String(Theme::ACCENT_DIM())));

    // Hold timer — 1200ms hold to confirm
    holdBtn->installEventFilter(this);

    // Progress bar grows while pressed
    auto *progressBar = new QWidget(inner);
    progressBar->setFixedHeight(3);
    progressBar->setStyleSheet(QStringLiteral(
        "background: %1; border-radius: 1px;")
        .arg(QLatin1String(Theme::ACCENT())));
    progressBar->setFixedWidth(0);

    m_holdTimer = new QTimer(this);
    m_holdTimer->setInterval(16); // ~60fps
    connect(m_holdTimer, &QTimer::timeout, this,
            [this, holdBtn, progressBar]() {
        m_holdProgress += qRound(16.0 * 100.0 / kHoldDurationMs);
        m_holdProgress = qMin(m_holdProgress, 100);
        const int barW = width() - 32;
        progressBar->setFixedWidth(qRound(barW * m_holdProgress / 100.0));

        if (m_holdProgress >= 100) {
            m_holdTimer->stop();
            UxGuide::instance().recordUse(QString{});
            if (m_onConfirm) m_onConfirm();
            dismiss();
        }
    });

    TouchFeedback::install(holdBtn, TouchFeedback::Effect::Ripple);
    connect(holdBtn, &QPushButton::pressed, this, [this]() {
        m_holdActive = true;
        m_holdProgress = 0;
        m_holdTimer->start();
        // Pause the auto-dismiss timer while holding
        if (m_dismissTimer) m_dismissTimer->stop();
    });
    connect(holdBtn, &QPushButton::released, this, [this, progressBar]() {
        if (!m_holdActive) return;
        m_holdActive = false;
        m_holdTimer->stop();
        m_holdProgress = 0;
        progressBar->setFixedWidth(0);
        // Restart auto-dismiss
        startAutoDismiss();
    });

    lay->addWidget(holdBtn);
    lay->addWidget(progressBar);
}

// ── Paint (chevron arrow) ─────────────────────────────────────────────────────

void ChevronConfirm::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing);

    const QColor bg = QColor(QLatin1String(Theme::BG_CARD()));
    p.setBrush(bg);
    p.setPen(Qt::NoPen);

    // Draw the chevron triangle
    const int cx = width() / 2;
    QPainterPath path;
    if (m_pointUp) {
        // Triangle pointing up at the button above
        path.moveTo(cx - 10, kChevronH);
        path.lineTo(cx,      0);
        path.lineTo(cx + 10, kChevronH);
        path.closeSubpath();
    } else {
        // Triangle pointing down at the button below
        const int y = height() - kChevronH;
        path.moveTo(cx - 10, y);
        path.lineTo(cx,      height());
        path.lineTo(cx + 10, y);
        path.closeSubpath();
    }
    p.drawPath(path);
}

// ── Lifecycle ─────────────────────────────────────────────────────────────────

void ChevronConfirm::animateIn()
{
    // show() must precede raise() — Qt ignores raise() on hidden widgets.
    // Then raise() twice: once to sort within siblings, once more after
    // the event loop processes the show (some platforms defer Z-order).
    QWidget::show();
    raise();
    QTimer::singleShot(0, this, [this]() { raise(); }); // deferred re-raise

    auto *fx = new QGraphicsOpacityEffect(this);
    setGraphicsEffect(fx);
    auto *anim = new QPropertyAnimation(fx, "opacity", this);
    anim->setDuration(180);
    anim->setStartValue(0.0);
    anim->setEndValue(1.0);
    anim->setEasingCurve(QEasingCurve::OutCubic);
    anim->start(QAbstractAnimation::DeleteWhenStopped);
}

void ChevronConfirm::startAutoDismiss()
{
    if (!m_dismissTimer) {
        m_dismissTimer = new QTimer(this);
        m_dismissTimer->setSingleShot(true);
        connect(m_dismissTimer, &QTimer::timeout, this, &ChevronConfirm::dismiss);
    }
    m_dismissTimer->start(kAutoDismissMs);
}

void ChevronConfirm::dismiss()
{
    if (m_holdTimer) m_holdTimer->stop();
    if (m_dismissTimer) m_dismissTimer->stop();

    // Fade out then delete
    auto *fx = qobject_cast<QGraphicsOpacityEffect *>(graphicsEffect());
    if (!fx) {
        // Create one for the fade
        fx = new QGraphicsOpacityEffect(this);
        setGraphicsEffect(fx);
        fx->setOpacity(1.0);
    }
    auto *anim = new QPropertyAnimation(fx, "opacity", this);
    anim->setDuration(160);
    anim->setStartValue(fx->opacity());
    anim->setEndValue(0.0);
    anim->setEasingCurve(QEasingCurve::InCubic);
    connect(anim, &QPropertyAnimation::finished, this, &QWidget::deleteLater);
    anim->start(QAbstractAnimation::DeleteWhenStopped);
}

} // namespace SnapClone::UI
