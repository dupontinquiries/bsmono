#include "ui/OutboxWidget.hpp"
#include "ui/Theme.hpp"

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QLabel>
#include <QScrollArea>
#include <QPushButton>
#include <QFrame>
#include <QDateTime>
#include <QFont>
#include <QLoggingCategory>

Q_LOGGING_CATEGORY(lcOutbox, "snapclone.outbox")

namespace SnapClone::UI {

// ─── Helpers ─────────────────────────────────────────────────────────────────

static QString relativeTime(const QDateTime &dt)
{
    const qint64 secs = dt.secsTo(QDateTime::currentDateTimeUtc());
    if (secs < 60)  return QObject::tr("just now");
    if (secs < 3600) return QObject::tr("%1m ago").arg(secs / 60);
    if (secs < 86400) return QObject::tr("%1h ago").arg(secs / 3600);
    return QObject::tr("%1d ago").arg(secs / 86400);
}

static QString phaseIcon(const QString &phase)
{
    if (phase == QLatin1String("connecting"))  return QStringLiteral("⟳");
    if (phase == QLatin1String("punching"))    return QStringLiteral("⚡");
    if (phase == QLatin1String("mailbox"))     return QStringLiteral("📬");
    if (phase == QLatin1String("relookup"))    return QStringLiteral("🔍");
    if (phase == QLatin1String("relay"))       return QStringLiteral("🔀");
    if (phase == QLatin1String("udp"))         return QStringLiteral("📡");
    if (phase == QLatin1String("failed"))      return QStringLiteral("✗");
    if (phase == QLatin1String("offline"))     return QStringLiteral("⏸");
    return QStringLiteral("…");
}

// ─── OutboxWidget ─────────────────────────────────────────────────────────────

OutboxWidget::OutboxWidget(QWidget *parent)
    : QWidget(parent)
{
    buildUi();
}

void OutboxWidget::buildUi()
{
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                              Theme::SPACE_MD, Theme::SPACE_MD);
    root->setSpacing(Theme::SPACE_SM);

    // ── Header ────────────────────────────────────────────────────────────
    auto *header = new QHBoxLayout;

    auto *title = new QLabel(tr("Outbox"), this);
    title->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px; font-weight: 700;")
        .arg(Theme::TEXT_PRIMARY()).arg(Theme::FONT_LG));
    header->addWidget(title);
    header->addStretch();

    // Summary pill: "2 / 5 delivered"
    m_summaryPill = new QLabel(this);
    m_summaryPill->setStyleSheet(QStringLiteral(
        "background: %1; color: %2; border-radius: %3px;"
        " padding: 3px 10px; font-size: %4px; font-weight: 600;")
        .arg(Theme::BG_RAISED()).arg(Theme::TEXT_SECONDARY())
        .arg(Theme::RADIUS_FULL).arg(Theme::FONT_SM));
    m_summaryPill->hide();
    header->addWidget(m_summaryPill);

    root->addLayout(header);

    // ── Scroll area ───────────────────────────────────────────────────────
    auto *scroll = new QScrollArea(this);
    scroll->setWidgetResizable(true);
    scroll->setFrameShape(QFrame::NoFrame);
    scroll->setStyleSheet(QStringLiteral(
        "QScrollArea { background: transparent; border: none; }"
        "QScrollBar:vertical { width: 4px; background: transparent; }"
        "QScrollBar::handle:vertical { background: %1; border-radius: 2px; }"
        "QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { height: 0; }")
        .arg(Theme::BG_HOVER()));

    m_listContainer = new QWidget(scroll);
    m_listContainer->setStyleSheet(QStringLiteral(
        "background: transparent;"));
    m_listLayout = new QVBoxLayout(m_listContainer);
    m_listLayout->setContentsMargins(0, 0, 0, 0);
    m_listLayout->setSpacing(Theme::SPACE_XS);

    m_emptyLabel = new QLabel(tr("No sent snaps yet.\nSend one from the Camera tab!"), m_listContainer);
    m_emptyLabel->setAlignment(Qt::AlignCenter);
    m_emptyLabel->setWordWrap(true);
    m_emptyLabel->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px; padding: %3px;")
        .arg(Theme::TEXT_DISABLED()).arg(Theme::FONT_BASE).arg(Theme::SPACE_XL));
    m_listLayout->addWidget(m_emptyLabel);
    m_listLayout->addStretch();

    scroll->setWidget(m_listContainer);
    root->addWidget(scroll, 1);
}

void OutboxWidget::addSnap(const Core::SnapId  &id,
                            const Core::UserId  &recipient,
                            const QString       &recipientName,
                            const QDateTime     &sentAt)
{
    // Deduplicate
    for (const SnapRow &r : m_rows)
        if (r.id.value == id.value) return;

    // Remove "no snaps" placeholder on first entry
    if (m_rows.isEmpty()) {
        m_emptyLabel->hide();
        // Remove the stretch
        QLayoutItem *stretch = m_listLayout->takeAt(m_listLayout->count() - 1);
        delete stretch;
    }

    // ── Card widget ───────────────────────────────────────────────────────
    auto *card = new QFrame(m_listContainer);
    card->setFrameShape(QFrame::NoFrame);
    card->setStyleSheet(QStringLiteral(
        "QFrame { background: %1; border-radius: %2px; }"
        "QFrame:hover { background: %3; }")
        .arg(Theme::BG_CARD()).arg(Theme::RADIUS_MD).arg(Theme::BG_HOVER()));

    auto *cardLayout = new QVBoxLayout(card);
    cardLayout->setContentsMargins(Theme::SPACE_SM, Theme::SPACE_SM,
                                    Theme::SPACE_SM, Theme::SPACE_SM);
    cardLayout->setSpacing(4);

    // Row 1: recipient + time
    auto *row1 = new QHBoxLayout;
    const QString displayName = recipientName.isEmpty()
        ? recipient.value : recipientName;
    auto *nameLbl = new QLabel(QStringLiteral("📷 → %1").arg(displayName), card);
    nameLbl->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px; font-weight: 600;")
        .arg(Theme::TEXT_PRIMARY()).arg(Theme::FONT_BASE));
    row1->addWidget(nameLbl);
    row1->addStretch();

    auto *timeLbl = new QLabel(relativeTime(sentAt), card);
    timeLbl->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px;")
        .arg(Theme::TEXT_SECONDARY()).arg(Theme::FONT_SM));
    row1->addWidget(timeLbl);
    cardLayout->addLayout(row1);

    // Row 2: network status label
    auto *statusLbl = new QLabel(tr("⟳ Connecting…"), card);
    statusLbl->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px;")
        .arg(Theme::TEXT_SECONDARY()).arg(Theme::FONT_SM));
    cardLayout->addWidget(statusLbl);

    // Insert at top (newest first)
    m_listLayout->insertWidget(0, card);

    SnapRow row;
    row.id            = id;
    row.recipient     = recipient;
    row.recipientName = recipientName;
    row.sentAt        = sentAt;
    row.card          = card;
    row.statusLbl     = statusLbl;
    row.timeLbl       = timeLbl;
    m_rows.prepend(row);

    rebuildSummary();
}

void OutboxWidget::updateStatus(const Core::SnapId &id,
                                 const QString &phase,
                                 const QString &detail)
{
    for (SnapRow &r : m_rows) {
        if (r.id.value != id.value) continue;
        if (r.delivered) return; // don't overwrite delivered state
        r.phase  = phase;
        r.detail = detail;

        const QString icon  = phaseIcon(phase);
        const QString label = detail.isEmpty() ? phase : detail;
        r.statusLbl->setText(QStringLiteral("%1 %2").arg(icon, label));

        const QString colour = (phase == QLatin1String("failed"))
            ? Theme::DANGER() : Theme::TEXT_SECONDARY();
        r.statusLbl->setStyleSheet(QStringLiteral(
            "color: %1; font-size: %2px;").arg(colour).arg(Theme::FONT_SM));
        return;
    }
}

void OutboxWidget::markDelivered(const Core::SnapId &id)
{
    for (SnapRow &r : m_rows) {
        if (r.id.value != id.value) continue;
        if (r.delivered) return;
        r.delivered = true;
        r.statusLbl->setText(tr("✓ Delivered"));
        r.statusLbl->setStyleSheet(QStringLiteral(
            "color: %1; font-size: %2px; font-weight: 600;")
            .arg(Theme::SUCCESS()).arg(Theme::FONT_SM));
        // Dim the card to visually de-emphasise delivered snaps
        r.card->setStyleSheet(QStringLiteral(
            "QFrame { background: %1; border-radius: %2px; opacity: 0.6; }"
            "QFrame:hover { background: %3; }")
            .arg(Theme::BG_DEEP()).arg(Theme::RADIUS_MD).arg(Theme::BG_CARD()));
        rebuildSummary();
        return;
    }
}

void OutboxWidget::pruneOldDelivered()
{
    const QDateTime cutoff = QDateTime::currentDateTimeUtc().addSecs(-24 * 3600);
    for (int i = static_cast<int>(m_rows.size()) - 1; i >= 0; --i) {
        const SnapRow &r = m_rows[i];
        if (r.delivered && r.sentAt < cutoff) {
            m_listLayout->removeWidget(r.card);
            r.card->deleteLater();
            m_rows.removeAt(i);
        }
    }
    if (m_rows.isEmpty()) {
        m_emptyLabel->show();
        m_listLayout->addStretch();
    }
    rebuildSummary();
}

void OutboxWidget::rebuildSummary()
{
    if (m_rows.isEmpty()) {
        m_summaryPill->hide();
        return;
    }
    int total     = static_cast<int>(m_rows.size());
    int delivered = 0;
    for (const SnapRow &r : m_rows)
        if (r.delivered) ++delivered;

    m_summaryPill->setText(tr("%1 / %2 delivered").arg(delivered).arg(total));
    // Colour: all delivered = green, partially = amber, none = muted
    const char *colour = (delivered == total) ? Theme::SUCCESS()
                       : (delivered > 0)      ? Theme::WARNING()
                                              : Theme::TEXT_SECONDARY();
    m_summaryPill->setStyleSheet(QStringLiteral(
        "background: %1; color: %2; border-radius: %3px;"
        " padding: 3px 10px; font-size: %4px; font-weight: 600;")
        .arg(Theme::BG_RAISED()).arg(colour)
        .arg(Theme::RADIUS_FULL).arg(Theme::FONT_SM));
    m_summaryPill->show();
}

} // namespace SnapClone::UI
