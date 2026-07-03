// ContactCard.hpp — V32.34+
//
// A single contact's card with an EXPANDING action submenu that collapses on
// swipe-out. Built as a self-contained widget so the expand/collapse animation
// and the swipe-to-close gesture live here rather than bloating ContactsPage.
//
// Structure:
//   ┌─ card (this) ───────────────────────────────┐
//   │  [ headerRow: avatar | name/id | badge | … ] │  ← always visible, tap toggles
//   │  [ submenu: action buttons … ]               │  ← height-animated, hidden when collapsed
//   └──────────────────────────────────────────────┘
//
// Interaction:
//   • Tap the header  → expand (reveal submenu) / collapse (if already open).
//   • Swipe-out (horizontal drag past a threshold) on an expanded card → collapse.
//   • Only the owner (ContactsPage) decides single-open-at-a-time by calling
//     collapse() on the previously-open card.
//
// The header content is supplied by the caller (ContactsPage::makeCard builds
// the avatar/name/badge row exactly as before); ContactCard just owns the
// expand/collapse chrome + gesture. Action buttons are added via addAction().

#pragma once

#include <QWidget>
#include <QString>
#include <QList>
#include <functional>

class QVBoxLayout;
class QHBoxLayout;
class QPushButton;
class QPropertyAnimation;
class QMouseEvent;

namespace SnapClone::UI {

class ContactCard : public QWidget {
    Q_OBJECT
    // Animated property: the submenu's visible height in px (0 = collapsed).
    Q_PROPERTY(int submenuHeight READ submenuHeight WRITE setSubmenuHeight)

public:
    explicit ContactCard(const QString &contactId, QWidget *parent = nullptr);

    /// The header is the always-visible row (avatar / name / badge / inline
    /// buttons). The caller builds it; ContactCard parents it into the header
    /// area. Call once, right after construction.
    void setHeader(QWidget *header);

    /// Add an action to the expanding submenu. `label` is the button text;
    /// `onClick` runs when tapped (the card auto-collapses afterward). Optional
    /// `iconName` maps to an SvgIcon asset; `danger` styles it as destructive.
    void addAction(const QString &label, std::function<void()> onClick,
                   const QString &iconName = QString(), bool danger = false);

    [[nodiscard]] QString contactId() const { return m_contactId; }
    [[nodiscard]] bool     isExpanded() const { return m_expanded; }

    int  submenuHeight() const { return m_submenuHeight; }
    void setSubmenuHeight(int h);

public slots:
    void expand();
    void collapse();
    void toggle();

signals:
    /// Emitted when this card expands, so the owner can collapse any other.
    void expanded(SnapClone::UI::ContactCard *self);

protected:
    bool eventFilter(QObject *obj, QEvent *ev) override;
    /// Paints a small expand/collapse chevron at the card's right edge as an
    /// affordance that the card is tappable-to-expand (▾ when collapsed, ▴ when
    /// expanded). Self-contained — doesn't disturb the caller-built header.
    void paintEvent(QPaintEvent *e) override;

private:
    void startAnim(int from, int to);
    [[nodiscard]] int submenuFullHeight() const;

    QString             m_contactId;
    QWidget            *m_header   = nullptr;
    QWidget            *m_submenu  = nullptr;   // container holding the action buttons
    QVBoxLayout        *m_outer    = nullptr;   // header + submenu stacked
    QVBoxLayout        *m_submenuLayout = nullptr;
    QPropertyAnimation *m_anim     = nullptr;
    int                 m_submenuHeight = 0;    // current animated height
    bool                m_expanded = false;

    // Swipe-out-to-close gesture state (tracked on the header press).
    bool   m_tracking   = false;
    QPointF m_pressPos;
    bool   m_swipeArmed = false;   // crossed the "this is a swipe not a tap" threshold
};

} // namespace SnapClone::UI
