#include "ui/ExportReminderCard.hpp"
#include "ui/Theme.hpp"
#include "ui/ThumbnailService.hpp"

#include <QHBoxLayout>
#include <QVBoxLayout>
#include <QLabel>
#include <QPushButton>
#include <QPixmap>

namespace SnapClone::UI {

ExportReminderCard::ExportReminderCard(const QStringList &recentMediaPaths,
                                       QWidget *parent)
    : QWidget(parent)
{
    setAttribute(Qt::WA_StyledBackground, true);
    setObjectName(QStringLiteral("ExportReminderCard"));
    // Card surface: a raised rounded panel that stands off the inbox list.
    setStyleSheet(QStringLiteral(
        "#ExportReminderCard { background: %1; border: 1px solid %2;"
        "  border-radius: %3px; }")
        .arg(QLatin1String(Theme::BG_CARD()))
        .arg(QLatin1String(Theme::BORDER()))
        .arg(Theme::RADIUS_MD));

    auto *outer = new QVBoxLayout(this);
    outer->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                              Theme::SPACE_MD, Theme::SPACE_MD);
    outer->setSpacing(Theme::SPACE_SM);

    // ── Prompt heading ───────────────────────────────────────────────────
    auto *title = new QLabel(tr("Save your snaps from this month?"), this);
    title->setStyleSheet(QStringLiteral(
        "color:%1; font-size:%2px; font-weight:700;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(Theme::FONT_LG));
    title->setWordWrap(true);
    outer->addWidget(title);

    auto *sub = new QLabel(
        tr("Snaps disappear after viewing. Export a copy to keep the ones "
           "you liked."), this);
    sub->setStyleSheet(QStringLiteral("color:%1; font-size:%2px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(Theme::FONT_SM));
    sub->setWordWrap(true);
    outer->addWidget(sub);

    // ── Thumbnail strip (optional) ───────────────────────────────────────
    if (!recentMediaPaths.isEmpty()) {
        m_thumbRow = new QHBoxLayout();
        m_thumbRow->setContentsMargins(0, Theme::SPACE_XS, 0, Theme::SPACE_XS);
        m_thumbRow->setSpacing(Theme::SPACE_XS);
        m_thumbRow->addStretch(0);   // leading flex so thumbs cluster left
        // (we add the trailing stretch after the thumbs in loadThumbnails)
        outer->addLayout(m_thumbRow);
        loadThumbnails(recentMediaPaths);
    }

    // ── Action row ───────────────────────────────────────────────────────
    auto *actions = new QHBoxLayout();
    actions->setContentsMargins(0, 0, 0, 0);
    actions->setSpacing(Theme::SPACE_SM);

    auto *later = new QPushButton(tr("Maybe later"), this);
    later->setCursor(Qt::PointingHandCursor);
    later->setMinimumHeight(Theme::TOUCH_MIN);
    later->setStyleSheet(QStringLiteral(
        "QPushButton { background: transparent; color:%1; border:none;"
        "  font-size:%2px; padding: 0 12px; }"
        "QPushButton:hover { color:%3; }")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(Theme::FONT_SM)
        .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    connect(later, &QPushButton::clicked, this, &ExportReminderCard::dismissed);

    auto *exportBtn = new QPushButton(tr("Export as zip"), this);
    exportBtn->setCursor(Qt::PointingHandCursor);
    exportBtn->setMinimumHeight(Theme::TOUCH_MIN);
    exportBtn->setStyleSheet(QStringLiteral(
        "QPushButton { background:%1; color:#fff; border:none;"
        "  border-radius:%2px; font-size:%3px; font-weight:600;"
        "  padding: 0 16px; }"
        "QPushButton:hover { background:%4; }")
        .arg(QLatin1String(Theme::ACCENT()))
        .arg(Theme::RADIUS_SM)
        .arg(Theme::FONT_SM)
        .arg(QLatin1String(Theme::ACCENT_DIM())));
    connect(exportBtn, &QPushButton::clicked,
            this, &ExportReminderCard::exportRequested);

    actions->addStretch(1);          // right-align the buttons
    actions->addWidget(later);
    actions->addWidget(exportBtn);
    outer->addLayout(actions);
}

void ExportReminderCard::loadThumbnails(const QStringList &paths)
{
    const int n = qMin(paths.size(), kMaxThumbs);
    for (int i = 0; i < n; ++i) {
        auto *thumb = new QLabel(this);
        thumb->setFixedSize(kThumbSize, kThumbSize);
        thumb->setStyleSheet(QStringLiteral(
            "background:%1; border-radius:%2px;")
            .arg(QLatin1String(Theme::BG_RAISED()))
            .arg(Theme::RADIUS_SM));
        // Insert before the trailing flex; the leading stretch is index 0, so
        // we append the thumb just before adding the final stretch below.
        m_thumbRow->addWidget(thumb);
        // Async load with `this` as the lifetime guard — if the card is
        // destroyed before the worker finishes, the callback is dropped.
        const QString path = paths[i];
        ThumbnailService::instance().request(
            path, kThumbSize, this,
            [thumb](const QPixmap &pm) {
                if (!pm.isNull()) thumb->setPixmap(pm);
            });
    }
    m_thumbRow->addStretch(1);   // trailing flex so the strip is left-aligned
}

} // namespace SnapClone::UI
