#include "ui/ContactPickerDialog.hpp"
#include "ui/Theme.hpp"

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QAbstractItemView>
#include <QListWidget>
#include <QListWidgetItem>
#include <QPushButton>
#include <QLabel>
#include <QDialogButtonBox>

namespace SnapClone::UI {

ContactPickerDialog::ContactPickerDialog(const QList<Core::Contact> &contacts,
                                          QWidget *parent)
    : MobileDialog(parent)
{
    setModal(true);
    setWindowTitle(tr("Send to…"));
    buildUi(contacts);
    resize(340, 480);
}

void ContactPickerDialog::buildUi(const QList<Core::Contact> &contacts)
{
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);

    setStyleSheet(QStringLiteral(
        "QDialog { background: %1; border-radius: %2px; }")
        .arg(Theme::BG_DEEP()).arg(Theme::RADIUS_LG));

    // ── Header ────────────────────────────────────────────────────────────
    auto *header = new QWidget(this);
    header->setStyleSheet(QStringLiteral(
        "background: %1; border-top-left-radius: %2px;"
        " border-top-right-radius: %2px;")
        .arg(Theme::BG_RAISED()).arg(Theme::RADIUS_LG));
    auto *headerRow = new QHBoxLayout(header);
    headerRow->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_SM,
                                   Theme::SPACE_MD, Theme::SPACE_SM);

    m_titleLabel = new QLabel(tr("Send snap to…"), header);
    m_titleLabel->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px; font-weight: 700;")
        .arg(Theme::TEXT_PRIMARY()).arg(Theme::FONT_LG));
    headerRow->addWidget(m_titleLabel);
    headerRow->addStretch();

    auto *cancelBtn = new QPushButton(tr("Cancel"), header);
    cancelBtn->setFlat(true);
    cancelBtn->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px; padding: 4px 8px;")
        .arg(Theme::TEXT_SECONDARY()).arg(Theme::FONT_BASE));
    connect(cancelBtn, &QPushButton::clicked, this, &QDialog::reject);
    headerRow->addWidget(cancelBtn);
    root->addWidget(header);

    // ── Contact list ──────────────────────────────────────────────────────
    m_list = new QListWidget(this);
    m_list->setStyleSheet(QStringLiteral(
        "QListWidget { background: %1; border: none; outline: none; }"
        "QListWidget::item { padding: 0; border: none; }"
        "QListWidget::item:selected { background: transparent; }"
        "QScrollBar:vertical { width: 4px; background: transparent; }"
        "QScrollBar::handle:vertical { background: %2; border-radius: 2px; }"
        "QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { height: 0; }")
        .arg(Theme::BG_DEEP()).arg(Theme::BG_HOVER()));
    m_list->setSpacing(2);

    // Sort: online first, then alphabetically
    QList<Core::Contact> sorted = contacts;
    std::stable_sort(sorted.begin(), sorted.end(), [](const Core::Contact &a, const Core::Contact &b) {
        if (a.online != b.online) return a.online > b.online;
        const QString na = a.nickname.isEmpty() ? a.displayName : a.nickname;
        const QString nb = b.nickname.isEmpty() ? b.displayName : b.nickname;
        return na.compare(nb, Qt::CaseInsensitive) < 0;
    });

    for (const Core::Contact &c : sorted) {
        auto *item = new QListWidgetItem(m_list);
        item->setData(Qt::UserRole, c.id.value);
        item->setSizeHint(QSize(0, 64));

        auto *card = new QWidget(m_list);
        card->setAttribute(Qt::WA_TranslucentBackground);
        auto *row = new QHBoxLayout(card);
        row->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_SM,
                                 Theme::SPACE_MD, Theme::SPACE_SM);
        row->setSpacing(Theme::SPACE_SM);

        // Avatar circle
        auto *avatar = new QLabel(card);
        const QString initial = (c.nickname.isEmpty() ? c.displayName : c.nickname)
            .left(1).toUpper();
        avatar->setText(initial.isEmpty() ? QStringLiteral("?") : initial);
        avatar->setFixedSize(40, 40);
        avatar->setAlignment(Qt::AlignCenter);
        avatar->setStyleSheet(QStringLiteral(
            "background: %1; border-radius: 20px;"
            " color: %2; font-size: 16px; font-weight: 700;")
            .arg(c.online ? Theme::ACCENT() : Theme::BG_RAISED())
            .arg(c.online ? Theme::TEXT_INVERSE() : Theme::TEXT_PRIMARY()));
        row->addWidget(avatar);

        // Name + status
        auto *nameCol = new QVBoxLayout;
        nameCol->setSpacing(2);
        const QString displayName = c.nickname.isEmpty() ? c.displayName : c.nickname;
        auto *nameLbl = new QLabel(displayName, card);
        nameLbl->setStyleSheet(QStringLiteral(
            "color: %1; font-size: %2px; font-weight: 600;")
            .arg(c.online ? Theme::TEXT_PRIMARY() : Theme::TEXT_SECONDARY())
            .arg(Theme::FONT_BASE));
        nameCol->addWidget(nameLbl);

        auto *statusLbl = new QLabel(
            c.online ? tr("● Online") : tr("○ Offline"), card);
        statusLbl->setStyleSheet(QStringLiteral(
            "color: %1; font-size: %2px;")
            .arg(c.online ? Theme::SUCCESS() : Theme::TEXT_DISABLED())
            .arg(Theme::FONT_SM));
        nameCol->addWidget(statusLbl);
        row->addLayout(nameCol, 1);

        // V32.16: removed the looping QPropertyAnimation+QGraphicsOpacityEffect
        // online-pulse decoration. Each animated label was a continuous source
        // of paint events; with N contacts in the picker that's N concurrent
        // animations, all targeting QListWidget item widgets. On Plasma Mobile
        // (Fairphone) this combined with the bottom-sheet open animation and
        // crashed the app a few seconds after the picker appeared (logs ended
        // with "DHT announce succeeded" then SnapClone restart line).
        //
        // The functional signal (online vs offline) is conveyed by:
        //   - filled vs hollow status dot ("● Online" / "○ Offline")
        //   - avatar background colour (accent vs muted)
        // The pulse was purely decorative — no information is lost by removing
        // it. If we want online status motion later, use a single shared
        // QTimer driving repaints of a custom-painted widget, not per-item
        // QGraphicsOpacityEffects.

        // Chevron
        auto *chevron = new QLabel(QStringLiteral("›"), card);
        chevron->setStyleSheet(QStringLiteral(
            "color: %1; font-size: 20px;").arg(Theme::TEXT_DISABLED()));
        row->addWidget(chevron);

        m_list->setItemWidget(item, card);
    }

    if (sorted.isEmpty()) {
        auto *empty = new QListWidgetItem(tr("No contacts yet — connect to someone first"), m_list);
        empty->setFlags(Qt::NoItemFlags);
        empty->setForeground(QColor(Theme::TEXT_DISABLED()));
        empty->setTextAlignment(Qt::AlignCenter);
    }

    connect(m_list, &QListWidget::itemDoubleClicked,
            this, &ContactPickerDialog::onItemDoubleClicked);
    connect(m_list, &QListWidget::itemSelectionChanged, this, [this]() {
        m_sendBtn->setEnabled(!m_list->selectedItems().isEmpty());
    });

    root->addWidget(m_list, 1);

    // ── Send button ───────────────────────────────────────────────────────
    auto *footer = new QWidget(this);
    footer->setStyleSheet(QStringLiteral(
        "background: %1; border-bottom-left-radius: %2px;"
        " border-bottom-right-radius: %2px;")
        .arg(Theme::BG_RAISED()).arg(Theme::RADIUS_LG));
    auto *footerRow = new QHBoxLayout(footer);
    footerRow->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_SM,
                                   Theme::SPACE_MD, Theme::SPACE_SM);

    m_sendBtn = new QPushButton(tr("Send Snap"), footer);
    m_sendBtn->setEnabled(false);
    m_sendBtn->setFixedHeight(44);
    m_sendBtn->setStyleSheet(QStringLiteral(
        "QPushButton {"
        "  background: %1; color: %2; border-radius: %3px;"
        "  font-size: %4px; font-weight: 700; border: none; }"
        "QPushButton:disabled { background: %5; color: %6; }"
        "QPushButton:pressed  { background: %7; }")
        .arg(Theme::ACCENT()).arg(Theme::TEXT_INVERSE())
        .arg(Theme::RADIUS_MD).arg(Theme::FONT_BASE)
        .arg(Theme::BG_CARD()).arg(Theme::TEXT_DISABLED())
        .arg(Theme::ACCENT_DIM()));
    connect(m_sendBtn, &QPushButton::clicked, this, [this]() {
        const auto items = m_list->selectedItems();
        if (items.isEmpty()) return;
        if (m_multi) {
            QList<Core::UserId> ids;
            ids.reserve(items.size());
            for (auto *it : items)
                ids.append(Core::UserId{ it->data(Qt::UserRole).toString() });
            emit contactsConfirmed(ids);
        } else {
            emit contactSelected(Core::UserId{ items.first()->data(Qt::UserRole).toString() });
        }
        accept();
    });
    footerRow->addWidget(m_sendBtn);
    root->addWidget(footer);
}

void ContactPickerDialog::onItemDoubleClicked(QListWidgetItem *item)
{
    if (!item) return;
    if (m_multi) return;   // multi-select confirms via the footer button, not a double-tap
    emit contactSelected(Core::UserId{ item->data(Qt::UserRole).toString() });
    accept();
}

Core::UserId ContactPickerDialog::selectedContact() const
{
    const auto items = m_list->selectedItems();
    if (items.isEmpty()) return {};
    return Core::UserId{ items.first()->data(Qt::UserRole).toString() };
}

void ContactPickerDialog::setMultiSelect(bool on, const QString &confirmLabel)
{
    m_multi = on;
    if (m_list)
        m_list->setSelectionMode(on ? QAbstractItemView::MultiSelection
                                    : QAbstractItemView::SingleSelection);
    if (m_sendBtn)
        m_sendBtn->setText(!confirmLabel.isEmpty() ? confirmLabel
                                                   : (on ? tr("Next") : tr("Send Snap")));
}

void ContactPickerDialog::setHeaderTitle(const QString &title)
{
    if (m_titleLabel) m_titleLabel->setText(title);
}

QList<Core::UserId> ContactPickerDialog::selectedContacts() const
{
    QList<Core::UserId> ids;
    if (!m_list) return ids;
    const auto items = m_list->selectedItems();
    ids.reserve(items.size());
    for (auto *it : items)
        ids.append(Core::UserId{ it->data(Qt::UserRole).toString() });
    return ids;
}

} // namespace SnapClone::UI
