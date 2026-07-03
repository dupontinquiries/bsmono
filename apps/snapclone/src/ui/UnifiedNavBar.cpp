#include "ui/UnifiedNavBar.hpp"
#include "ui/SwipeNavigator.hpp"
#include "ui/SubTabbed.hpp"
#include "ui/Theme.hpp"
#include "ui/SvgIcon.hpp"   // V32.34+: responsive icon fallback for narrow bars

#include <QHBoxLayout>
#include <QLayout>
#include <QPushButton>
#include <QResizeEvent>
#include <QShowEvent>
#include <QFontMetrics>
#include <QPainter>
#include <QPropertyAnimation>
#include <QLinearGradient>
#include <QColor>
#include <QSet>
#include <QVariantAnimation>

namespace SnapClone::UI {

UnifiedNavBar::UnifiedNavBar(SwipeNavigator *nav, QWidget *parent)
    : QWidget(parent), m_nav(nav)
{
    setAttribute(Qt::WA_StyledBackground, true);
    setObjectName(QStringLiteral("UnifiedNavBar"));
    setStyleSheet(QStringLiteral(
        "#UnifiedNavBar { background: %1; border-top: 1px solid %2; }")
        .arg(QLatin1String(Theme::BG_RAISED()))
        .arg(QLatin1String(Theme::BORDER())));

    // Bar height = one touch target + asymmetric vertical breathing room: a
    // little above, more below so the arrows clear the phone's bottom screen
    // edge in fullscreen (V32.49+). Math is exact: TOUCH_MIN content +
    // SPACE_XS top + SPACE_MD bottom = barH.
    const int barH = Theme::TOUCH_MIN + Theme::SPACE_XS + Theme::SPACE_MD;
    setFixedHeight(barH);
    setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Fixed);

    buildChrome();
    rebuildForCurrentPage();

    if (m_nav) {
        connect(m_nav, &SwipeNavigator::pageChanged,
                this, [this](int) { rebuildForCurrentPage(); });
        connect(m_nav, &SwipeNavigator::subTabChanged,
                this, [this](int) { refreshActiveSegment(); });
        connect(m_nav, &SwipeNavigator::subTabDragProgress,
                this, &UnifiedNavBar::onDragProgress);
    }
}

void UnifiedNavBar::buildChrome()
{
    auto *row = new QHBoxLayout(this);
    row->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_XS,
                            Theme::SPACE_MD, Theme::SPACE_MD);
    row->setSpacing(Theme::SPACE_XS);

    const QString navBtnStyle = QStringLiteral(
        "QPushButton { background: transparent; color:%1; border:none;"
        "  font-size:%2px; padding: 0; }"
        "QPushButton:hover:enabled { color:%3; }"
        "QPushButton:disabled { color:%4; }")
        .arg(QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(Theme::FONT_XL)
        .arg(QLatin1String(Theme::ACCENT()))
        .arg(QLatin1String(Theme::TEXT_DISABLED()));

    m_prevPageBtn = new QPushButton(QStringLiteral("‹"), this);
    m_prevPageBtn->setCursor(Qt::PointingHandCursor);
    m_prevPageBtn->setFixedSize(Theme::TOUCH_MIN + 8, Theme::TOUCH_MIN);
    m_prevPageBtn->setStyleSheet(navBtnStyle);
    connect(m_prevPageBtn, &QPushButton::clicked, this, [this]() {
        if (m_nav) m_nav->goToPage(m_nav->currentIndex() - 1,
                                   /*animate=*/true, /*resetSubTab=*/true);
    });
    row->addWidget(m_prevPageBtn, 0);

    // Page-name chip (left of the sub-tabs). Tapping it could open a page
    // picker later; for now it's a non-interactive label-styled button so the
    // user always sees which top-level page they're on.
    m_pageNameBtn = new QPushButton(this);
    m_pageNameBtn->setEnabled(false);
    m_pageNameBtn->setFixedHeight(Theme::TOUCH_MIN);
    m_pageNameBtn->setStyleSheet(QStringLiteral(
        "QPushButton:disabled { background: transparent; color:%1; border:none;"
        "  font-size:%2px; font-weight:700; padding: 0 6px; }")
        .arg(QLatin1String(Theme::TEXT_PRIMARY()))
        .arg(Theme::FONT_BASE));
    row->addWidget(m_pageNameBtn, 0);

    // The morphing sub-tab segment row.
    m_segRow = new QHBoxLayout();
    m_segRow->setContentsMargins(0, 0, 0, 0);
    m_segRow->setSpacing(Theme::SPACE_XS);
    row->addLayout(m_segRow, 1);   // takes the flexible middle space

    m_nextPageBtn = new QPushButton(QStringLiteral("›"), this);
    m_nextPageBtn->setCursor(Qt::PointingHandCursor);
    m_nextPageBtn->setFixedSize(Theme::TOUCH_MIN + 8, Theme::TOUCH_MIN);
    m_nextPageBtn->setStyleSheet(navBtnStyle);
    connect(m_nextPageBtn, &QPushButton::clicked, this, [this]() {
        if (m_nav) m_nav->goToPage(m_nav->currentIndex() + 1,
                                   /*animate=*/true, /*resetSubTab=*/true);
    });
    row->addWidget(m_nextPageBtn, 0);
}

void UnifiedNavBar::clearSegments()
{
    // Drain EVERY item from the segment row — buttons AND the addStretch()
    // spacer items. The earlier version only removed the tracked buttons, so
    // the stretch spacers accumulated one (or, on non-SubTabbed pages, two) per
    // rebuild and progressively squeezed the segments. takeAt(0) in a loop
    // removes spacers (which we delete) and widgets (which we deleteLater via
    // their button pointer). m_segs/m_segLabels are the source of truth for the
    // buttons; here we just make sure the layout itself is empty.
    while (QLayoutItem *it = m_segRow->takeAt(0)) {
        if (QWidget *w = it->widget()) w->deleteLater();
        delete it;   // frees the layout item (and any spacer it owned)
    }
    m_segs.clear();
    m_segLabels.clear();
}

QString UnifiedNavBar::segmentStyle(bool active, bool compactPadding) const
{
    // Single source of truth for segment styling at any collapse level. The
    // only thing that varies with collapse is horizontal padding (compact) —
    // colours/weight track the active state. Kept as one template so the
    // active-highlight refresh and the collapse restyle can't drift apart.
    const char *pad = compactPadding ? "0 4px" : "0 10px";
    return QStringLiteral(
        "QPushButton { background:%1; color:%2; border:none;"
        "  border-radius:%3px; font-size:%4px; font-weight:%5; padding:%6; }"
        "QPushButton:hover { color:%7; }")
        .arg(active ? QLatin1String(Theme::ACCENT_SOFT())
                    : QLatin1String("transparent"))
        .arg(active ? QLatin1String(Theme::ACCENT())
                    : QLatin1String(Theme::TEXT_SECONDARY()))
        .arg(Theme::RADIUS_SM)
        .arg(Theme::FONT_SM)
        .arg(active ? 700 : 500)
        .arg(QLatin1String(pad))
        .arg(QLatin1String(Theme::TEXT_PRIMARY()));
}

void UnifiedNavBar::rebuildForCurrentPage()
{
    if (!m_nav) return;
    clearSegments();

    // Page label + prev/next enabled state.
    const int idx = m_nav->currentIndex();
    const QString pageName = m_nav->pageName(idx).toUpper();
    m_pageNameBtn->setText(pageName);
    if (m_prevPageBtn) {
        m_prevPageBtn->setEnabled(idx > 0);
        m_prevPageBtn->setToolTip(idx > 0
            ? tr("Previous: %1").arg(m_nav->pageName(idx - 1)) : QString());
    }
    if (m_nextPageBtn) {
        m_nextPageBtn->setEnabled(idx < m_nav->pageCount() - 1);
        m_nextPageBtn->setToolTip(idx < m_nav->pageCount() - 1
            ? tr("Next: %1").arg(m_nav->pageName(idx + 1)) : QString());
    }

    // Sub-tab segments for the current page (empty for non-SubTabbed pages —
    // Camera/Wayfinder — which then show just the page name + arrows).
    SubTabbed *st = m_nav->currentSubTabbed();
    const int count = st ? st->subTabCount() : 0;
    const int active = st ? st->currentSubTab() : 0;

    if (count == 0) {
        // Non-SubTabbed page: hide the fixed left chip and drop a CENTERED page
        // name into the stretchy segment row, so the bar shows "‹   CAMERA   ›"
        // instead of a left-aligned name with a hollow gap. Built as a labelled
        // segment (reusing m_segs so clearSegments tears it down) but
        // non-interactive and centered.
        m_pageNameBtn->setVisible(false);
        m_segRow->addStretch(1);
        auto *nameLbl = new QPushButton(pageName, this);
        nameLbl->setEnabled(false);
        nameLbl->setFixedHeight(Theme::TOUCH_MIN);
        nameLbl->setStyleSheet(QStringLiteral(
            "QPushButton:disabled { background: transparent; color:%1; border:none;"
            "  font-size:%2px; font-weight:700; letter-spacing:1px; }")
            .arg(QLatin1String(Theme::TEXT_PRIMARY()))
            .arg(Theme::FONT_BASE));
        m_segRow->addWidget(nameLbl, 0);
        m_segs.append(nameLbl);          // so clearSegments() reclaims it
        m_segLabels.append(pageName);
        m_segRow->addStretch(1);
        update();
        return;
    }

    m_pageNameBtn->setVisible(true);
    for (int i = 0; i < count; ++i) {
        const bool on = (i == active);
        const QString label = st->subTabLabel(i);
        auto *seg = new QPushButton(label, this);
        seg->setCursor(Qt::PointingHandCursor);
        seg->setFixedHeight(Theme::TOUCH_MIN);
        seg->setStyleSheet(segmentStyle(on, /*compactPadding=*/false));
        const int target = i;
        connect(seg, &QPushButton::clicked, this, [this, target]() {
            // Tap = direct jump to that sub-tab.
            if (auto *s = m_nav->currentSubTabbed()) {
                s->setCurrentSubTab(target);
                refreshActiveSegment();
            }
        });
        m_segs.append(seg);
        m_segLabels.append(label);
        m_segRow->addWidget(seg, 0);
    }
    m_segRow->addStretch(1);   // left-align the segments

    applyResponsiveCollapse();   // pick the right label/icon form for this width
    update();                    // refresh page-position dots for the new page
}

void UnifiedNavBar::refreshActiveSegment()
{
    if (!m_nav || !m_nav->currentSubTabbed()) return;
    // V32.34+ morph: animate the underline to the new active segment instead of
    // snapping. Capture the target index BEFORE applyResponsiveCollapse re-lays
    // out (geometries are valid after layout, so we compute spans post-collapse
    // inside startUnderlineMorph).
    const int active = m_nav->currentSubTabbed()->currentSubTab();
    // Re-running the collapse keeps the active-keeps-label rule correct (the
    // newly-active segment must regain its full label even if it was collapsed
    // while inactive) and restyles every segment for the new active index — it
    // reads the active index from the SubTabbed model itself.
    applyResponsiveCollapse();
    startUnderlineMorph(active);
    update();
}

void UnifiedNavBar::startUnderlineMorph(int toIndex)
{
    if (toIndex < 0 || toIndex >= m_segs.size()) { m_morphActive = -1; return; }
    // If a drag preview is active, the underline is being driven by the drag —
    // don't fight it with a morph; the drag's commit will land us here cleanly.
    if (m_dragDir != 0) { m_morphActive = toIndex; m_morph = 1.0; return; }

    const QRect target = m_segs[toIndex]->geometry();
    const qreal toL = target.left(), toR = target.right();

    // "From" = where the underline visually sits right now. If we're already
    // settled on a segment, that's its span; if a morph is mid-flight, use the
    // current interpolated span so a rapid re-tap doesn't jump.
    qreal fromL, fromR;
    if (m_morphActive >= 0 && m_morph < 1.0) {
        fromL = m_morphFromL + (m_morphToL - m_morphFromL) * m_morph;
        fromR = m_morphFromR + (m_morphToR - m_morphFromR) * m_morph;
    } else if (m_morphActive >= 0 && m_morphActive < m_segs.size()) {
        const QRect cur = m_segs[m_morphActive]->geometry();
        fromL = cur.left(); fromR = cur.right();
    } else {
        // No prior position (first show) → no morph, settle instantly.
        m_morphActive = toIndex; m_morph = 1.0;
        m_morphFromL = m_morphToL = toL; m_morphFromR = m_morphToR = toR;
        return;
    }

    m_morphFromL = fromL; m_morphFromR = fromR;
    m_morphToL   = toL;   m_morphToR   = toR;
    m_morphActive = toIndex;
    m_morph = 0.0;

    auto *anim = static_cast<QVariantAnimation *>(m_morphAnim);
    if (!anim) {
        anim = new QVariantAnimation(this);
        anim->setDuration(200);
        anim->setEasingCurve(QEasingCurve::OutCubic);
        connect(anim, &QVariantAnimation::valueChanged, this,
                [this](const QVariant &v) {
                    m_morph = v.toReal();
                    update();   // paintEvent reads interpolated span; no stylesheet
                });
        m_morphAnim = anim;
    }
    anim->stop();
    anim->setStartValue(0.0);
    anim->setEndValue(1.0);
    anim->start();
}

void UnifiedNavBar::resizeEvent(QResizeEvent *e)
{
    QWidget::resizeEvent(e);
    applyResponsiveCollapse();
}

void UnifiedNavBar::showEvent(QShowEvent *e)
{
    QWidget::showEvent(e);
    // Re-sync from the live model in case the current page's sub-tab changed
    // via a path we didn't directly observe while hidden (the navigator's
    // subTabChanged + our own tap handler cover the common cases; this is the
    // catch-all). refreshActiveSegment re-reads currentSubTab() and restyles.
    refreshActiveSegment();
}

void UnifiedNavBar::applyResponsiveCollapse()
{
    // Only the real sub-tab segments are collapsible. On non-SubTabbed pages
    // m_segs holds just the centered page-name label (currentSubTabbed() null),
    // which must NOT be treated as a collapsible segment — bail out.
    SubTabbed *st = m_nav ? m_nav->currentSubTabbed() : nullptr;
    if (!st || m_segs.isEmpty()) {
        return;
    }
    const int active = st->currentSubTab();

    // Budget: how much horizontal room do the segments actually have? Total
    // width minus the prev/next arrows and the page-name chip and margins.
    const int arrows   = (m_prevPageBtn ? m_prevPageBtn->sizeHint().width() : 0)
                       + (m_nextPageBtn ? m_nextPageBtn->sizeHint().width() : 0);
    const int chrome   = 2 * Theme::SPACE_SM + arrows;
    int avail          = width() - chrome;

    // First pass: measure the full-label width of every segment.
    const QFontMetrics fm(font());
    auto fullW = [&](const QString &lbl) {
        return fm.horizontalAdvance(lbl) + 2 * 10 /*full padding*/ + Theme::SPACE_XS;
    };
    int needFull = 0;
    for (const auto &lbl : m_segLabels) needFull += fullW(lbl);
    const int pageNameW = m_pageNameBtn ? m_pageNameBtn->sizeHint().width() : 0;

    // Collapse ladder, least → most aggressive:
    //   L0 WIDE   : page name shown + all full labels (fits as-is).
    //   L1 TIGHT  : hide the page-name chip, keep all full labels.
    //   L2 NARROW : active keeps full label; inactive shrink to compact padding.
    //   L3 ICONS  : active keeps full label; inactive → icon (if any) else a
    //               short 1–3 char form, compact padding.
    int level = 0;
    if (needFull + pageNameW > avail)              level = 1;  // drop page name
    if (level >= 1 && needFull > avail)            level = 2;  // compact padding
    // For L3 we need the active full width + the inactive compact/icon widths.
    if (level >= 2) {
        int needCompact = 0;
        for (int i = 0; i < m_segLabels.size(); ++i) {
            if (i == active) needCompact += fullW(m_segLabels[i]);
            else needCompact += fm.horizontalAdvance(m_segLabels[i]) + 2 * 4;
        }
        if (needCompact > avail)                   level = 3;  // icon/short form
    }

    // Page-name chip: visible only at L0.
    if (m_pageNameBtn) m_pageNameBtn->setVisible(level == 0);

    // For the L3 short form: are all labels' first letters distinct? If so a
    // single-initial form is unambiguous (and cleaner); otherwise we use a
    // 3-char form. Computed once per layout pass over this page's labels.
    bool initialsUnique = true;
    {
        QSet<QChar> seen;
        for (const auto &lbl : m_segLabels) {
            const QChar c = lbl.isEmpty() ? QChar() : lbl.at(0).toUpper();
            if (seen.contains(c)) { initialsUnique = false; break; }
            seen.insert(c);
        }
    }

    // Apply per-segment label/icon + padding for the chosen level.
    for (int i = 0; i < m_segs.size(); ++i) {
        auto *seg = m_segs[i];
        const bool on = (i == active);
        const bool compact = (level >= 2);
        if (level <= 2 || on) {
            // Full text label (active always keeps it; everyone keeps it at
            // L0–L2). Clear any icon left from a previous narrower layout.
            seg->setIcon(QIcon());
            seg->setText(m_segLabels[i]);
        } else {
            // L3 inactive: prefer an icon if the page supplies one for this
            // sub-tab; otherwise a compact short text form. Prefer a SINGLE
            // uppercase initial (clean, like a tight segmented control) — but
            // only if initials are unique across this page's labels, else fall
            // back to a 3-char form so two same-initial tabs ("Sent"/"Storage")
            // stay distinguishable. The full label is always in the tooltip.
            const QString iconName = st ? st->subTabIcon(i) : QString();
            if (!iconName.isEmpty()) {
                seg->setIcon(SvgIcon::secondary(iconName, QSize(20, 20)));
                seg->setText(QString());
            } else {
                seg->setIcon(QIcon());
                seg->setText(initialsUnique ? m_segLabels[i].left(1).toUpper()
                                            : m_segLabels[i].left(3).toUpper());
            }
        }
        seg->setStyleSheet(segmentStyle(on, compact));
        // When a segment is reduced to an icon or a 3-char abbreviation (L3
        // inactive), its full label is no longer visible — surface it as a
        // tooltip so the meaning isn't lost on narrow screens. At L0–L2 (or for
        // the active full-label segment) the text speaks for itself, so clear
        // the tooltip to avoid a redundant hover.
        const bool collapsedToShort = (level >= 3) && !on;
        seg->setToolTip(collapsedToShort ? m_segLabels[i] : QString());
    }
}

void UnifiedNavBar::onDragProgress(int dir, qreal progress)
{
    // Cheap state update + repaint. No setStyleSheet here — the preview is a
    // painted underline (paintEvent), so the ~60Hz drag stream never touches
    // the QStyleSheetStyle caches.
    const qreal p = qBound(0.0, progress, 1.0);
    if (m_dragDir == dir && qFuzzyCompare(m_dragProgress, p)) return;
    m_dragDir      = (p <= 0.0) ? 0 : dir;
    m_dragProgress = p;
    update();
}

void UnifiedNavBar::refreshBadges()
{
    // Counts are read live from SubTabbed::subTabBadge() during paintEvent, so
    // a repaint is all that's needed to reflect a changed count.
    update();
}

void UnifiedNavBar::setCameraGlassMode(bool on)
{
    if (m_cameraGlass == on) return;
    m_cameraGlass = on;

    // When glass is (about to be) visible, the stylesheet background must be
    // transparent so our painted gradient shows and the live viewfinder reads
    // through; when fully off, restore the normal opaque bar. We switch the
    // stylesheet immediately on turn-ON (so nothing opaque flashes), and on
    // turn-OFF only after the fade-out completes (handled below).
    if (on) {
        setStyleSheet(QStringLiteral(
            "#UnifiedNavBar { background: transparent; border-top: 1px solid %1; }")
            .arg(QLatin1String(Theme::BORDER())));
    }

    if (!m_glassAnim) {
        m_glassAnim = new QPropertyAnimation(this, "glassAmount", this);
        m_glassAnim->setDuration(Theme::ANIM_NORMAL);
        connect(m_glassAnim, &QPropertyAnimation::finished, this, [this]() {
            // After fading fully OUT, restore the opaque stylesheet bar.
            if (!m_cameraGlass && m_glassAmount <= 0.001) {
                setStyleSheet(QStringLiteral(
                    "#UnifiedNavBar { background: %1; border-top: 1px solid %2; }")
                    .arg(QLatin1String(Theme::BG_RAISED()))
                    .arg(QLatin1String(Theme::BORDER())));
            }
        });
    }
    m_glassAnim->stop();
    m_glassAnim->setStartValue(m_glassAmount);
    m_glassAnim->setEndValue(on ? 1.0 : 0.0);
    m_glassAnim->start();
}

void UnifiedNavBar::setGlassAmount(qreal a)
{
    m_glassAmount = qBound(0.0, a, 1.0);
    update();
}

void UnifiedNavBar::paintEvent(QPaintEvent *e)
{
    QWidget::paintEvent(e);   // stylesheet background (border-top etc.)

    QPainter pnt(this);
    pnt.setRenderHint(QPainter::Antialiasing, true);

    // V32.38f camera-glass background. On the camera page (m_glassAmount→1) the
    // stylesheet bg is transparent; paint a dark frosted gradient here instead
    // so the bar matches the top/bottom viewfinder scrims and the live preview
    // reads through near the top edge. Fades in/out with m_glassAmount, so other
    // pages (amount 0) are untouched — this whole block no-ops there.
    if (m_glassAmount > 0.001) {
        const qreal a = m_glassAmount;
        QLinearGradient g(0, 0, 0, height());
        g.setColorAt(0.0, QColor(0, 0, 0, int(70  * a)));   // lighter at top edge
        g.setColorAt(0.45, QColor(0, 0, 0, int(140 * a)));
        g.setColorAt(1.0, QColor(8, 7, 6, int(225 * a)));   // dark, near-opaque base
        pnt.fillRect(rect(), g);
    }

    // ── Page-position dots ────────────────────────────────────────────────
    // Always drawn (every page, SubTabbed or not), centered along the TOP edge
    // of the bar — out of the way of the vertically-centered segment text and
    // the bottom underline. This is the global "where am I among the N pages"
    // indicator: the current page's dot is a filled accent pill, the others are
    // dim. It's what makes the bar feel coherent on Camera/Wayfinder (which
    // have no sub-tabs) instead of showing a hollow gap, and reinforces the
    // page position on the SubTabbed pages too.
    if (m_nav) {
        const int pages = m_nav->pageCount();
        const int cur   = m_nav->currentIndex();
        if (pages > 1) {
            const int dotD   = 5;                 // inactive dot diameter
            const int gap    = 6;
            const int curW   = 14;                // active dot is a stretched pill
            const int totalW = (pages - 1) * (dotD + gap) + curW;
            int dx = (width() - totalW) / 2;
            const int dy = 5;
            pnt.setPen(Qt::NoPen);
            for (int i = 0; i < pages; ++i) {
                const bool on = (i == cur);
                QColor dc(on ? QString::fromLatin1(Theme::ACCENT())
                             : QString::fromLatin1(Theme::TEXT_DISABLED()));
                if (on) {
                    pnt.setBrush(dc);
                    pnt.drawRoundedRect(QRectF(dx, dy, curW, dotD),
                                        dotD / 2.0, dotD / 2.0);
                    dx += curW + gap;
                } else {
                    pnt.setBrush(dc);
                    pnt.drawEllipse(QRectF(dx, dy, dotD, dotD));
                    dx += dotD + gap;
                }
            }
        }
    }

    // Everything below is segment-specific (the underline + count badges) and
    // applies ONLY to real SubTabbed pages. On non-SubTabbed pages m_segs holds
    // just the centered page-name label (st is null), so we stop here — no
    // underline/badges under the page name.
    SubTabbed *st = m_nav ? m_nav->currentSubTabbed() : nullptr;
    if (!st || m_segs.isEmpty()) return;
    const int active = st->currentSubTab();
    if (active < 0 || active >= m_segs.size()) return;

    // Underline geometry: a 3px bar hugging the bottom of the segment row,
    // RADIUS_SM-rounded. At rest it sits under the active segment. During an
    // in-flight drag it slides/stretches toward the neighbour the user is
    // heading to, so the bar "weighs into" the gesture before it commits.
    const QRect aRect = m_segs[active]->geometry();
    const int   y     = height() - 3;
    qreal x0 = aRect.left();
    qreal x1 = aRect.right();

    if (m_dragDir != 0 && m_dragProgress > 0.0) {
        const int neighbour = active + m_dragDir;
        if (neighbour >= 0 && neighbour < m_segs.size()) {
            const QRect nRect = m_segs[neighbour]->geometry();
            // Interpolate the underline's edges from the active segment toward
            // the neighbour by `progress`. The leading edge moves first so the
            // line appears to "reach" toward the target — a subtle pull.
            const qreal t = m_dragProgress;
            x0 = aRect.left()  + (nRect.left()  - aRect.left())  * t;
            x1 = aRect.right() + (nRect.right() - aRect.right()) * t;
        }
    } else if (m_morphActive == active && m_morph < 1.0) {
        // V32.34+ morph: a tap/detent sub-tab change is animating — interpolate
        // the underline span from where it was toward the new active segment so
        // it glides rather than snaps. (Drag preview above takes precedence; a
        // settled morph (m_morph==1) just uses the active rect.)
        x0 = m_morphFromL + (m_morphToL - m_morphFromL) * m_morph;
        x1 = m_morphFromR + (m_morphToR - m_morphFromR) * m_morph;
    }

    QColor c(Theme::ACCENT());
    // Fade the underline up slightly as the drag progresses so the preview is
    // perceptible but not loud at the start of a drag.
    c.setAlphaF(m_dragDir != 0 ? (0.55 + 0.35 * m_dragProgress) : 0.9);
    pnt.setPen(Qt::NoPen);
    pnt.setBrush(c);
    const QRectF line(x0 + 4, y - 1, (x1 - x0) - 8, 3);
    pnt.drawRoundedRect(line, 1.5, 1.5);

    // ── Sub-tab count badges ──────────────────────────────────────────────
    // The hidden top SegmentedTabStrip used to show these (unread convs on
    // Inbox "Unread", online peers on Contacts "Online"); after unification
    // they'd have vanished, so the unified bar renders them. Painted (not
    // overlaid widgets / stylesheet) to keep the ~60Hz drag repaints cheap and
    // off the QStyleSheetStyle caches. A small accent pill with the count,
    // top-right of the segment.
    if (st) {
        const QFont badgeFont = [&]{ QFont f = font(); f.setPointSize(8);
                                     f.setBold(true); return f; }();
        pnt.setFont(badgeFont);
        const QFontMetrics bfm(badgeFont);
        for (int i = 0; i < m_segs.size(); ++i) {
            const int n = st->subTabBadge(i);
            if (n <= 0) continue;
            const QString txt = (n > 99) ? QStringLiteral("99+")
                                         : QString::number(n);
            const QRect segR = m_segs[i]->geometry();
            const int   tw   = bfm.horizontalAdvance(txt);
            const int   ph   = bfm.height() + 2;             // pill height
            const int   pw   = qMax(ph, tw + 8);             // ≥ circle
            // Top-right corner of the segment, nudged inward so it doesn't
            // clip the bar edge.
            const int   px   = qMin(segR.right() - pw / 2, width() - pw - 2);
            const int   py   = segR.top() + 2;
            const QRectF pill(px, py, pw, ph);
            QColor pc(Theme::ACCENT());
            pnt.setPen(Qt::NoPen);
            pnt.setBrush(pc);
            pnt.drawRoundedRect(pill, ph / 2.0, ph / 2.0);
            pnt.setPen(QColor(QString::fromLatin1(Theme::TEXT_INVERSE())));
            pnt.drawText(pill, Qt::AlignCenter, txt);
        }
    }
}

} // namespace SnapClone::UI
