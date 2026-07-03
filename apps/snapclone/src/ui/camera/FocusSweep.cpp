#include "ui/camera/FocusSweep.hpp"
#include "ui/camera/VcmController.hpp"

#include <QLoggingCategory>
#include <QThread>
#include <algorithm>

namespace SnapClone::UI::Camera {

Q_LOGGING_CATEGORY(lcFocus, "snapclone.ui.camera.focus")

// V32.32: Laplacian variance on the centre 1/3 ROI of the green channel.
// We convert to RGB888 if the input is something else — this is cheap
// (typically the input is already RGB from the camera pipeline) and lets
// us treat the buffer as plain bytes.
double laplacianSharpness(const QImage &input)
{
    if (input.isNull() || input.width() < 6 || input.height() < 6) return 0.0;

    QImage img = (input.format() == QImage::Format_RGB888)
                     ? input
                     : input.convertToFormat(QImage::Format_RGB888);

    const int w = img.width();
    const int h = img.height();
    // Centre 1/3 ROI — focus metric for typical handheld photography
    // should ignore borders, sky, etc. and concentrate on the subject
    // the user is aiming at.
    const int x0 = w / 3, x1 = 2 * w / 3;
    const int y0 = h / 3, y1 = 2 * h / 3;

    // QImage scanlines may be padded — use bytesPerLine for stride.
    const int stride = img.bytesPerLine();
    const uchar *bits = img.constBits();

    auto green = [&](int x, int y) -> double {
        // RGB888 is byte-packed RGB, green is the middle byte.
        return bits[y * stride + x * 3 + 1];
    };

    double sum = 0.0, sumSq = 0.0;
    quint64 n = 0;
    for (int y = y0 + 1; y < y1 - 1; ++y) {
        for (int x = x0 + 1; x < x1 - 1; ++x) {
            // 4-point Laplacian: 4·centre − neighbours.
            const double lap =
                  4.0 * green(x, y)
                - green(x - 1, y) - green(x + 1, y)
                - green(x, y - 1) - green(x, y + 1);
            sum   += lap;
            sumSq += lap * lap;
            ++n;
        }
    }
    if (n == 0) return 0.0;
    const double mean = sum / static_cast<double>(n);
    return (sumSq / static_cast<double>(n)) - (mean * mean);
}

namespace {

// V32.32: helper — discard a few frames after a lens move so we read
// post-settle sharpness, not the blur from a lens still in motion.
// Caller-controlled count because settle time varies with sweep step
// size (bigger moves = longer settle).
QImage acquireSettled(std::function<QImage()> &get, int settleMs, int discardCount = 1)
{
    QThread::msleep(settleMs);
    for (int i = 0; i < discardCount; ++i) {
        QImage stale = get();
        Q_UNUSED(stale);
    }
    return get();
}

} // namespace

CdafResult runCdafSweep(VcmController &vcm,
                         std::function<QImage()> getNextFrame,
                         int settleMs)
{
    CdafResult result{};
    if (!vcm.isOpen()) {
        qCWarning(lcFocus) << "runCdafSweep: VCM not open";
        return result;
    }

    struct Step { int pos; double sharp; };
    std::vector<Step> coarse;
    coarse.reserve(9);

    // ── Coarse sweep: 9 steps across full range ─────────────────────────
    for (int i = 0; i <= 8; ++i) {
        const int pos = vcm.minPos()
                      + i * (vcm.maxPos() - vcm.minPos()) / 8;
        if (!vcm.setPosition(pos)) {
            qCWarning(lcFocus) << "coarse: setPosition failed at" << pos;
            return result;
        }
        const QImage frame = acquireSettled(getNextFrame, settleMs, 2);
        if (frame.isNull()) {
            qCWarning(lcFocus) << "coarse: null frame at" << pos;
            continue;
        }
        const double s = laplacianSharpness(frame);
        coarse.push_back({pos, s});
    }

    if (coarse.empty()) {
        qCWarning(lcFocus) << "coarse sweep produced no samples";
        return result;
    }

    auto coarseBest = std::max_element(coarse.begin(), coarse.end(),
        [](const Step &a, const Step &b) { return a.sharp < b.sharp; });
    const int coarsePeak = coarseBest->pos;

    // ── Fine sweep: 11 steps in ±(range/8) around the coarse peak ──────
    const int range   = (vcm.maxPos() - vcm.minPos()) / 8;
    const int fineMin = std::max(vcm.minPos(), coarsePeak - range);
    const int fineMax = std::min(vcm.maxPos(), coarsePeak + range);

    std::vector<Step> fine;
    fine.reserve(11);
    for (int i = 0; i <= 10; ++i) {
        const int pos = fineMin + i * (fineMax - fineMin) / 10;
        if (!vcm.setPosition(pos)) continue;
        // Shorter settle in fine pass — smaller moves settle faster.
        const QImage frame = acquireSettled(getNextFrame, settleMs / 2, 1);
        if (frame.isNull()) continue;
        const double s = laplacianSharpness(frame);
        fine.push_back({pos, s});
    }

    if (fine.empty()) {
        // Couldn't refine — settle on the coarse peak.
        vcm.setPosition(coarsePeak);
        result.finalPos      = coarsePeak;
        result.finalSharpness = coarseBest->sharp;
        result.succeeded     = true;
        return result;
    }

    auto fineBest = std::max_element(fine.begin(), fine.end(),
        [](const Step &a, const Step &b) { return a.sharp < b.sharp; });
    int finalPos = fineBest->pos;
    double finalSharp = fineBest->sharp;

    // ── Parabolic sub-step refinement ──────────────────────────────────
    // Fit a parabola through the best fine point and its two neighbours.
    // The vertex of that parabola is typically a better estimate than
    // the discrete sample maximum.
    if (fineBest != fine.begin() && fineBest + 1 != fine.end()) {
        const auto prev = fineBest - 1;
        const auto next = fineBest + 1;
        const double y0 = prev->sharp;
        const double y1 = fineBest->sharp;
        const double y2 = next->sharp;
        const double step = static_cast<double>(fineBest->pos - prev->pos);
        // Vertex offset: 0.5 * (y0 - y2) / (y0 - 2y1 + y2). Tiny epsilon
        // in the denominator guards against division by zero when the
        // three points are colinear (flat sharpness curve).
        const double denom  = (y0 - 2.0 * y1 + y2);
        const double offset = (std::abs(denom) < 1e-9)
                                  ? 0.0
                                  : 0.5 * (y0 - y2) / denom;
        const int refined = static_cast<int>(
            fineBest->pos + offset * step);
        finalPos = std::clamp(refined, vcm.minPos(), vcm.maxPos());
        vcm.setPosition(finalPos);
        // V32.38k: we just moved the lens to the refined sub-step position, so
        // measure the sharpness HERE rather than reporting the pre-refinement
        // discrete fineBest sample. finalSharpness feeds m_focusBaseline (the
        // scene-change re-focus threshold); a stale/too-low baseline makes the
        // detector trigger-happy. One extra settled frame is cheap and we're
        // already parked at finalPos.
        const QImage refinedFrame = acquireSettled(getNextFrame, settleMs / 2, 1);
        if (!refinedFrame.isNull()) {
            const double rs = laplacianSharpness(refinedFrame);
            // Only adopt it if it's a sane measurement; otherwise keep the
            // discrete best (never report a worse-than-sampled baseline).
            if (rs > 0.0) finalSharp = std::max(finalSharp, rs);
        }
    }

    result.finalPos      = finalPos;
    result.finalSharpness = finalSharp;
    result.succeeded     = true;
    return result;
}

CdafResult runFastFocus(VcmController &vcm,
                         std::function<QImage()> getNextFrame,
                         int settleMs,
                         std::function<bool()> isCancelled)
{
    CdafResult result{};
    if (!vcm.isOpen()) return result;

    // V32.35+: helper — true if the caller asked us to bail (double-tap flip).
    auto cancelled = [&isCancelled]() {
        return isCancelled && isCancelled();
    };
    // Bail before doing anything if a flip already arrived in the gap between
    // the GUI thread arming the sweep and this worker starting.
    if (cancelled()) {
        qCInfo(lcFocus) << "fast focus cancelled before first move";
        return result;
    }

    struct Step { int pos; double sharp; };
    std::vector<Step> samples;
    samples.reserve(5);

    // 5 steps total — chosen empirically as the minimum that reliably
    // identifies the peak hemisphere of the focus range. Subjects in
    // typical phone-snap range fall within 1-2 steps of one of these.
    for (int i = 0; i <= 4; ++i) {
        // V32.35+: check cancellation BEFORE each lens move, so a flip stops
        // the motor where it is rather than completing the sweep.
        if (cancelled()) {
            qCInfo(lcFocus) << "fast focus cancelled mid-sweep at step" << i;
            return result; // succeeded stays false
        }
        const int pos = vcm.minPos()
                      + i * (vcm.maxPos() - vcm.minPos()) / 4;
        if (!vcm.setPosition(pos)) continue;
        const QImage frame = acquireSettled(getNextFrame, settleMs, 1);
        if (frame.isNull()) continue;
        samples.push_back({pos, laplacianSharpness(frame)});
    }

    if (samples.empty()) {
        qCWarning(lcFocus) << "fast focus produced no samples";
        return result;
    }

    auto best = std::max_element(samples.begin(), samples.end(),
        [](const Step &a, const Step &b) { return a.sharp < b.sharp; });

    // One last cancellation check before the final park move.
    if (cancelled()) {
        qCInfo(lcFocus) << "fast focus cancelled before final park";
        return result;
    }
    vcm.setPosition(best->pos);
    result.finalPos      = best->pos;
    result.finalSharpness = best->sharp;
    result.succeeded     = true;
    return result;
}

} // namespace SnapClone::UI::Camera
