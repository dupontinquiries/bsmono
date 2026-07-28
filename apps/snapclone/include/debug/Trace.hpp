#pragma once
// ─── debug/Trace.hpp ─────────────────────────────────────────────────────────
// The logging macros every per-event diagnostic should use.
//
//   SC_TRACE(Input, lcSwipeNav, "[WHEEL] v=%.0f h=%.0f", v, h);
//
// What this buys over a bare qCInfo:
//
//   * OFF BY DEFAULT. A production build prints nothing from these lines, so
//     the log holds the events that matter instead of a wheel-notch firehose.
//   * NEAR-ZERO COST WHEN OFF. The arguments are inside the `if`, so nothing is
//     formatted, no QString is built, no varargs are marshalled — one relaxed
//     atomic load and a well-predicted branch. (Plain qCInfo evaluates and
//     formats its arguments BEFORE Qt decides to drop the message; that is the
//     waste this replaces.)
//   * STILL THERE WHEN NEEDED. The statement survives in the shipped binary, so
//     somebody with an installed build and a bug can switch the channel on from
//     Settings, an env var, or a CLI flag — no rebuild, no debug package.
//
// SC_TRACE_HOT is the same thing for paths where even the branch is unwelcome
// (per-frame paint, per-sample audio). It compiles to nothing unless the build
// sets -DSNAPCLONE_HOT_TRACE=ON, so those sites cost literally zero in a
// production build while remaining available in a diagnostic build.
//
// Choosing between them: if it can fire more than a few hundred times a second
// on a busy machine, it is HOT. Wheel/touch/gesture events are not — they are
// bounded by human input rate — so they use SC_TRACE and stay field-switchable.
//
// Levels: SC_TRACE maps to qCInfo, SC_TRACE_DEBUG to qCDebug. Warnings and
// errors keep using qCWarning/qCCritical directly and are NEVER gated: a real
// problem must always reach the log.
// ─────────────────────────────────────────────────────────────────────────────

#include "debug/TraceChannels.hpp"

// SC_TRACE_TESTING lets tests/test_TraceChannels.cpp exercise these macros with
// a stub logger and prove the property the whole design rests on: when a channel
// is off, the arguments are never evaluated. Nothing else should define it.
#ifndef SC_TRACE_TESTING
#  include <QLoggingCategory>
#endif

#define SC_TRACE(chan, cat, ...)                                                        \
    do {                                                                                \
        if (::SnapClone::Debug::traceEnabled(::SnapClone::Debug::TraceChannel::chan)) {  \
            qCInfo(cat, __VA_ARGS__);                                                   \
        }                                                                               \
    } while (false)

#define SC_TRACE_DEBUG(chan, cat, ...)                                                  \
    do {                                                                                \
        if (::SnapClone::Debug::traceEnabled(::SnapClone::Debug::TraceChannel::chan)) {  \
            qCDebug(cat, __VA_ARGS__);                                                  \
        }                                                                               \
    } while (false)

/// Stream form, for sites written as `qCInfo(cat) << "x" << y`.
///
/// Uses a for-loop rather than an if/else so it is immune to the dangling-else
/// trap (`if (c) SC_TRACE_S(...) << "x"; else ...` stays correct) — the same
/// shape Qt's own category macros use. The stream operators are only reached
/// when the channel is live, so the QDebug object and every << conversion are
/// skipped entirely when it is off.
#define SC_TRACE_S(chan, cat)                                                           \
    for (bool sc_trace_on_ =                                                            \
             ::SnapClone::Debug::traceEnabled(::SnapClone::Debug::TraceChannel::chan);   \
         sc_trace_on_; sc_trace_on_ = false)                                            \
        qCInfo(cat)

#ifdef SNAPCLONE_HOT_TRACE
#  define SC_TRACE_HOT(chan, cat, ...) SC_TRACE(chan, cat, __VA_ARGS__)
#else
#  define SC_TRACE_HOT(chan, cat, ...) do { } while (false)
#endif
