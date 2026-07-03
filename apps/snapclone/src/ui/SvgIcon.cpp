#include "ui/SvgIcon.hpp"
#include "ui/Theme.hpp"
#include <QFile>
#include <QPainter>
#include <QSvgRenderer>
#include <QPixmap>
#include <QStandardPaths>
#include <QCoreApplication>
#include <QLoggingCategory>

Q_LOGGING_CATEGORY(lcIcon, "snapclone.icon")

namespace SnapClone::UI {

QString SvgIcon::svgPath(const QString &name)
{
    // Search order:
    //   1. Flatpak resource path    /app/share/snapclone/icons/
    //   2. Installed data path      /usr/share/snapclone/icons/
    //   3. Beside the binary        ../assets/icons/  (dev builds)
    //   4. App source tree          assets/icons/     (running from build dir)
    const QString fname = name + QStringLiteral(".svg");
    // Qt resource path — always works when icons.qrc is compiled in
    const QString qrcPath = QStringLiteral(":/icons/") + fname;
    if (QFile::exists(qrcPath)) return qrcPath;

    // Filesystem fallbacks for dev builds without qrc
    const QStringList candidates = {
        QStringLiteral("/app/share/snapclone/icons/") + fname,
        QStringLiteral("/usr/share/snapclone/icons/") + fname,
        QCoreApplication::applicationDirPath() + QStringLiteral("/../assets/icons/") + fname,
        QCoreApplication::applicationDirPath() + QStringLiteral("/assets/icons/") + fname,
        QStringLiteral("assets/icons/") + fname,
    };
    for (const QString &p : candidates) {
        if (QFile::exists(p)) return p;
    }
    qCWarning(lcIcon) << "SVG not found:" << name << "(checked :/icons/ + filesystem)";
    return {};
}

QPixmap SvgIcon::render(const QByteArray &svgData, const QColor &color,
                        const QSize &size)
{
    // Replace currentColor with the actual hex color.
    // This works because all our SVGs use stroke="currentColor".
    QByteArray recolored = svgData;
    recolored.replace("currentColor", color.name(QColor::HexRgb).toLatin1());

    QSvgRenderer renderer(recolored);
    QPixmap px(size * 2); // 2x for HiDPI
    px.fill(Qt::transparent);
    {
        QPainter p(&px);
        p.setRenderHint(QPainter::Antialiasing);
        renderer.render(&p, QRectF(QPointF(0, 0), QSizeF(size * 2)));
    }
    px.setDevicePixelRatio(2.0);
    return px;
}

QIcon SvgIcon::tinted(const QString &name, const QColor &color, const QSize &size)
{
    const QString path = svgPath(name);
    if (path.isEmpty()) return {};
    QFile f(path);
    if (!f.open(QIODevice::ReadOnly)) return {};
    return QIcon(render(f.readAll(), color, size));
}

QIcon SvgIcon::accent(const QString &name, const QSize &size)
{
    return tinted(name, QColor(QLatin1String(Theme::ACCENT())), size);
}

QIcon SvgIcon::secondary(const QString &name, const QSize &size)
{
    return tinted(name, QColor(QLatin1String(Theme::TEXT_SECONDARY())), size);
}

QIcon SvgIcon::tabIcon(const QString &name, const QSize &size)
{
    const QString path = svgPath(name);
    if (path.isEmpty()) return {};
    QFile f(path);
    if (!f.open(QIODevice::ReadOnly)) return {};
    const QByteArray svg = f.readAll();

    QIcon icon;
    // Active = accent color
    icon.addPixmap(render(svg, QColor(QLatin1String(Theme::ACCENT())), size),
                   QIcon::Normal, QIcon::On);
    // Inactive = dimmed text
    icon.addPixmap(render(svg, QColor(QLatin1String(Theme::TEXT_DISABLED())), size),
                   QIcon::Normal, QIcon::Off);
    return icon;
}

} // namespace SnapClone::UI
