#pragma once
// ─── SvgIcon.hpp ─────────────────────────────────────────────────────────────
//
// Theme-aware SVG icon loader. All icons use `stroke="currentColor"` so the
// stroke color is recolored by Qt's CSS `color` property at paint time.
//
// Usage:
//   QPushButton *btn = new QPushButton;
//   btn->setIcon(SvgIcon::get("camera"));        // loads assets/icons/camera.svg
//   btn->setIconSize(QSize(22, 22));
//
// The icon is recolored to the current theme's accent or text color
// depending on context. Pass a QColor to tint:
//   btn->setIcon(SvgIcon::tinted("export", QColor(Theme::TEXT_SECONDARY())));
//
// For tab bar icons, use SvgIcon::tabIcon() which returns two versions
// (active=accent, inactive=text-disabled) pre-baked into a QIcon with states.

#include <QIcon>
#include <QColor>
#include <QString>
#include <QSize>

namespace SnapClone::UI {

class SvgIcon
{
public:
    /// Load an icon from assets/icons/{name}.svg, tinted to the given color.
    static QIcon tinted(const QString &name, const QColor &color,
                        const QSize &size = QSize(22, 22));

    /// Load with theme accent color.
    static QIcon accent(const QString &name, const QSize &size = QSize(22, 22));

    /// Load with theme text-secondary color.
    static QIcon secondary(const QString &name, const QSize &size = QSize(22, 22));

    /// Two-state icon for tab bars: active=accent, inactive=text-disabled.
    static QIcon tabIcon(const QString &name, const QSize &size = QSize(24, 24));

    /// Path to the SVG file.
    static QString svgPath(const QString &name);

private:
    SvgIcon() = delete;
    /// Render SVG bytes with the given stroke color substituted for currentColor.
    static QPixmap render(const QByteArray &svgData, const QColor &color,
                          const QSize &size);
};

} // namespace SnapClone::UI
