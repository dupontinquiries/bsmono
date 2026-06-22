#pragma once
//
// QtWebSocketCompat.hpp — paper over the QWebSocket error-signal rename.
//
// Qt 6.5 renamed QWebSocket's error signal from `error(QAbstractSocket::
// SocketError)` to `errorOccurred(QAbstractSocket::SocketError)`, matching the
// equivalent rename that happened to QAbstractSocket back in Qt 5.15. Both
// signals carry the identical argument, so a connect() against either works as
// long as we name the one that exists for the Qt being compiled against.
//
// The project targets a newer Qt (where errorOccurred is correct), but to keep
// it buildable on Qt 6.4 and earlier too, route the four connect() sites
// through SC_QWEBSOCKET_ERROR_SIGNAL instead of hardcoding the name.
//
#include <QtGlobal>

#if QT_VERSION >= QT_VERSION_CHECK(6, 5, 0)
#  define SC_QWEBSOCKET_ERROR_SIGNAL (&QWebSocket::errorOccurred)
#else
// Pre-6.5 `error` is an overloaded name, so taking its address is ambiguous;
// pin the signal overload explicitly.
#  define SC_QWEBSOCKET_ERROR_SIGNAL \
       (QOverload<QAbstractSocket::SocketError>::of(&QWebSocket::error))
#endif
