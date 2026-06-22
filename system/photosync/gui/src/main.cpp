// PhotoSync Camera GUI - Qt/QML Interface with libcamera
// Snapchat-like camera interface for Linux phones and laptops

#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QtConcurrent>
#include "CameraManager.h"
#include "PhotoProcessor.h"

int main(int argc, char *argv[])
{
    QGuiApplication app(argc, argv);
    
    // Set app properties
    app.setOrganizationName("PhotoSync");
    app.setOrganizationDomain("photosync.org");
    app.setApplicationName("PhotoSync Camera");
    
    // Register QML types
    qmlRegisterType<CameraManager>("PhotoSync", 1, 0, "CameraManager");
    qmlRegisterType<PhotoProcessor>("PhotoSync", 1, 0, "PhotoProcessor");
    
    // Create QML engine
    QQmlApplicationEngine engine;
    
    // Load main QML file
    const QUrl url(QStringLiteral("qrc:/qml/main.qml"));
    
    QObject::connect(
        &engine, &QQmlApplicationEngine::objectCreated,
        &app, [url](QObject *obj, const QUrl &objUrl) {
            if (!obj && url == objUrl)
                QCoreApplication::exit(-1);
        },
        Qt::QueuedConnection
    );
    
    engine.load(url);
    
    return app.exec();
}
