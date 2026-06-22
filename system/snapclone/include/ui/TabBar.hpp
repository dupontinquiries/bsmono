#pragma once
#include <QWidget>
#include <QList>
#include <QIcon>
#include <QPointer>

// ─── TabBar ───────────────────────────────────────────────────────────────────
// Fixed bottom tab bar with icon + label for each page.
// Mirrors the SwipeNavigator's current page and drives goToPage() on tap.
// Styled to match the app's dark warm-sand theme.

QT_BEGIN_NAMESPACE
class QPropertyAnimation;
QT_END_NAMESPACE

namespace SnapClone::UI {

class TabBar : public QWidget
{
    Q_OBJECT
    Q_PROPERTY(int indicatorX READ indicatorX WRITE setIndicatorX)

public:
    struct Tab {
        QString icon;   // emoji fallback
        QIcon   svgIcon; // preferred — theme-colored SVG or single unicode char used as icon
        QString label;
    };

    explicit TabBar(QWidget *parent = nullptr);

    void addTab(const QString &icon, const QString &label);
    void setCurrentIndex(int index);
    void setBadge(int tabIndex, int count);
    void setTabSvgIcon(int tabIndex, const QIcon &icon); // 0 = clear, >0 = show dot
    [[nodiscard]] int currentIndex() const { return m_current; }

    int  indicatorX() const { return m_indicatorX; }
    void setIndicatorX(int x) { m_indicatorX = x; if (x >= 0) update(); }

signals:
    void tabSelected(int index);

protected:
    void paintEvent(QPaintEvent *) override;
    void mousePressEvent(QMouseEvent *) override;
    bool event(QEvent *) override;
    void showEvent(QShowEvent *e) override;
    void resizeEvent(QResizeEvent *) override;

private:
    int tabWidth() const;
    int indicatorTargetX(int index) const;
    void animateIndicator(int toX);

    QList<Tab>           m_tabs;
    QHash<int,int>       m_badges; // tabIndex → unread count
    int                  m_current      = 1; // app starts on Inbox (kPageInbox)
    int                  m_indicatorX   = -1; // -1 = not yet positioned; set on first resize
    QPropertyAnimation  *m_anim         = nullptr;
    // Active tap-highlight widget. QPointer so it auto-nulls when the
    // highlight finishes its fade and deleteLaters itself. Setting a new
    // tap deletes the previous, preventing pile-up of QGraphicsOpacityEffects
    // during rapid swiping — which caused "A paint device can only be painted
    // by one painter at a time" warnings.
    QPointer<QWidget>    m_activeHighlight;
};

} // namespace SnapClone::UI
