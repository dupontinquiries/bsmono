#pragma once
// ─── UI/MediaSendSummary.hpp ──────────────────────────────────────────────────
//
// A quiet "send receipt" for group media — three plain facts, one line each,
// that make the snapclone-mediashare guarantees visible before you send:
//
//   ✓  privacy     — metadata stripped (sc_image_strip_metadata)
//   ↑  size        — only what isn't already shared goes out (content-addressed
//                    dedup: chunk store `missing`)
//   🔒 encryption  — sealed to everyone in the thread (multiway keyshare)
//
// Pure view. The controller fills it from the values it already computes on the
// send path:
//   strip report (format + segments removed)  → setStripResult
//   manifest vs. peer chunk store             → setDedup(uploadBytes, totalBytes)
//   group size                                → setRecipients
//
// Nothing animates — this is a moment of reassurance, not motion.
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QString>
#include <QRectF>

namespace SnapClone::UI {

class MediaSendSummary : public QWidget {
    Q_OBJECT
public:
    explicit MediaSendSummary(QWidget *parent = nullptr);
    ~MediaSendSummary() override;

    QSize sizeHint() const override;

public Q_SLOTS:
    /// `format`: 0 unknown/passthrough, 1 JPEG, 2 PNG (matches the strip FFI).
    void setStripResult(int format, int segmentsRemoved);
    /// `uploadBytes` = bytes not already shared; `totalBytes` = full media size.
    void setDedup(qint64 uploadBytes, qint64 totalBytes);
    /// How many people the content key is wrapped for.
    void setRecipients(int count);
    void reset();

protected:
    void paintEvent(QPaintEvent *) override;

private:
    enum class Facet { Privacy, Size, Crypto };

    int rowHeight() const;
    void paintFacet(QPainter &p, Facet facet, const QRectF &row);
    void paintGlyph(QPainter &p, Facet facet, const QRectF &box, const QColor &c);
    QString primaryText(Facet facet) const;
    QString detailText(Facet facet) const;
    QColor facetColor(Facet facet) const;
    static QString humanBytes(qint64 n);

    int m_format = 0;
    int m_segmentsRemoved = 0;
    qint64 m_uploadBytes = -1;
    qint64 m_totalBytes = -1;
    int m_recipients = 0;
};

} // namespace SnapClone::UI
