// test_SubPageDetent.cpp — standalone (no Qt) tests for the sub-page detent
// curve. Build: g++ -std=c++20 -I include tests/test_SubPageDetent.cpp -o /tmp/t && /tmp/t
//
// Verifies the three asks: (1) buffer/detent at the start, (2) increasing
// stickiness toward commit with a free middle (low–high–low sensitivity), and
// (3) the grow-text scale tied to progress — plus capped travel (less bounce).

#include "ui/SubPageDetent.hpp"

#include <cmath>
#include <cstdio>
#include <initializer_list>

using SnapClone::UI::SubPageDetent;
using SnapClone::UI::SubPageDetentConfig;

static int g_pass = 0, g_fail = 0;

static void check(bool ok, const char *name) {
    if (ok) { ++g_pass; std::printf("  ok   %s\n", name); }
    else    { ++g_fail; std::printf("  FAIL %s\n", name); }
}
static bool approx(double a, double b, double eps) { return std::fabs(a - b) <= eps; }

// raw overscroll corresponding to normalized travel t for a given config.
static double rawAt(const SubPageDetentConfig &c, double t) {
    return c.bufferPx + t * (c.commitPx - c.bufferPx);
}

int main() {
    std::printf("SubPageDetent tests\n");

    SubPageDetentConfig cfg; // defaults: buffer 26, commit 240, max 64, stick 1.6
    SubPageDetent d(cfg);

    // 1 — buffer/detent: nothing moves within the buffer.
    check(d.offset(0.0) == 0.0, "offset is zero at rest");
    check(d.offset(cfg.bufferPx) == 0.0, "offset is zero through the buffer");
    check(d.offset(cfg.bufferPx * 0.5) == 0.0, "offset is zero mid-buffer");
    check(d.progress(cfg.bufferPx) == 0.0, "progress is zero through the buffer");
    check(d.offset(cfg.bufferPx + 1.0) > 0.0, "offset becomes positive past the buffer");

    // 2 — monotonic, capped travel (less bounce).
    {
        bool mono = true;
        double prev = -1.0;
        for (double r = 0.0; r <= cfg.commitPx * 1.5; r += 2.0) {
            const double o = d.offset(r);
            if (o + 1e-9 < prev) mono = false;
            prev = o;
        }
        check(mono, "offset is monotonic non-decreasing");
    }
    check(d.offset(cfg.commitPx) <= cfg.maxOffsetPx + 1e-6, "offset never exceeds maxOffsetPx");
    check(approx(d.offset(cfg.commitPx), cfg.maxOffsetPx, 0.01), "offset reaches maxOffsetPx at commit");
    check(approx(d.offset(cfg.commitPx * 3.0), cfg.maxOffsetPx, 1e-6),
          "offset clamps to maxOffsetPx far past commit (no runaway bounce)");

    // 3 — progress endpoints + commit threshold.
    check(approx(d.progress(rawAt(cfg, 0.5)), 0.5, 1e-6), "progress is linear past the buffer");
    check(approx(d.progress(cfg.commitPx), 1.0, 1e-6), "progress is 1 at commit");
    check(approx(d.progress(cfg.commitPx * 2.0), 1.0, 1e-6), "progress clamps to 1 beyond commit");
    check(!d.shouldCommit(cfg.commitPx - 1.0), "no commit just below threshold");
    check(d.shouldCommit(cfg.commitPx), "commit exactly at threshold");
    check(d.shouldCommit(cfg.commitPx + 50.0), "commit beyond threshold");

    // 4 — THE feel: sensitivity is low–high–low (detent at start, free middle,
    // sticky near commit). Sample early / mid / late.
    {
        const double sEarly = d.sensitivity(rawAt(cfg, 0.08));
        const double sMid   = d.sensitivity(rawAt(cfg, 0.50));
        const double sLate  = d.sensitivity(rawAt(cfg, 0.95));
        check(sMid > sLate,  "stickier near commit: mid sensitivity > late sensitivity");
        check(sMid > sEarly, "detent at start: mid sensitivity > early sensitivity");
        check(sLate < sMid * 0.5, "late sensitivity is markedly lower (deliberate commit)");
    }

    // 5 — stickiness exponent adds buffer / damps early travel.
    {
        SubPageDetentConfig loose = cfg; loose.stickiness = 1.0;
        SubPageDetentConfig stick = cfg; stick.stickiness = 2.2;
        const double r = rawAt(cfg, 0.40);
        const double oLoose = SubPageDetent(loose).offset(r);
        const double oStick = SubPageDetent(stick).offset(r);
        check(oStick < oLoose, "higher stickiness ⇒ less early travel (more buffer)");
    }

    // 6 — grow-text scale tied to progress.
    check(approx(d.textScale(cfg.bufferPx), cfg.minTextScale, 1e-6), "text at min scale through buffer");
    check(approx(d.textScale(cfg.commitPx), cfg.maxTextScale, 1e-6), "text at max scale at commit");
    {
        const double sLow  = d.textScale(rawAt(cfg, 0.25));
        const double sHigh = d.textScale(rawAt(cfg, 0.75));
        check(sHigh > sLow, "text scale grows with progress");
        check(d.scaleFromProgress(0.5) > 1.0 == (cfg.minTextScale + cfg.maxTextScale > 2.0),
              "resting-relative scale midpoint is consistent");
    }

    // 7 — degenerate config (commit <= buffer) must not divide by zero.
    {
        SubPageDetentConfig bad; bad.bufferPx = 100.0; bad.commitPx = 50.0;
        SubPageDetent db(bad);
        check(db.progress(80.0) == 0.0, "degenerate config yields zero progress, no crash");
        check(db.offset(80.0) == 0.0, "degenerate config yields zero offset, no crash");
    }

    // 8 — asymptotic crawl (the visible overscroll): keeps following the finger,
    // never reaches the ceiling, and its inverse round-trips so a grab mid-spring
    // resumes exactly.
    {
        const double C = 600.0, S = 700.0;   // ceiling px, softness px
        check(SubPageDetent::crawlOffset(0.0, C, S) == 0.0, "crawl: zero at rest");
        // strictly below the ceiling for any finite pull, even a huge one
        check(SubPageDetent::crawlOffset(50.0, C, S) < C, "crawl: below ceiling at small pull");
        check(SubPageDetent::crawlOffset(100000.0, C, S) < C, "crawl: below ceiling at huge pull");
        check(SubPageDetent::crawlOffset(100000.0, C, S) > 0.95 * C, "crawl: approaches ceiling asymptotically");
        // monotonic increasing
        check(SubPageDetent::crawlOffset(300.0, C, S) > SubPageDetent::crawlOffset(150.0, C, S),
              "crawl: monotonic in pull");
        // initial slope ≈ C/S (responsiveness): tiny pull moves ~ (C/S)*pull
        {
            const double tiny = 1.0;
            const double off  = SubPageDetent::crawlOffset(tiny, C, S);
            check(approx(off, tiny * (C / S), 0.01), "crawl: initial slope ≈ ceiling/softness");
        }
        // round-trip: inverse(offset) == original pull
        for (double pull : {10.0, 120.0, 480.0, 1500.0}) {
            const double off = SubPageDetent::crawlOffset(pull, C, S);
            const double inv = SubPageDetent::crawlInverse(off, C, S);
            check(approx(inv, pull, 0.01), "crawl: inverse round-trips");
        }
        // guard at/over the ceiling doesn't divide by zero
        check(SubPageDetent::crawlInverse(C, C, S) > 0.0, "crawl: inverse at ceiling is finite");
        check(SubPageDetent::crawlOffset(100.0, 0.0, S) == 0.0, "crawl: zero ceiling → zero, no crash");
    }

    std::printf("\n%d passed, %d failed\n", g_pass, g_fail);
    return g_fail == 0 ? 0 : 1;
}
