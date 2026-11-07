#include "shim.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QLibrary>
#include <QtCore/QLocale>
#include <QtCore/QTranslator>
#include <QtDBus/QDBusConnection>
#include <QtDBus/QDBusConnectionInterface>
#include <QtDBus/QDBusInterface>
#include <QtGui/QGuiApplication>
#include <QtGui/QIcon>
#include <QtGui/QOffscreenSurface>
#include <QtGui/QOpenGLContext>
#include <QtGui/QSurfaceFormat>
#include <QtQml/QQmlApplicationEngine>

namespace truesense {

namespace {
QQmlApplicationEngine *g_engine = nullptr;
QTranslator *g_english = nullptr;
QTranslator *g_language = nullptr;

bool load_into(QTranslator *&slot, const QString &code) {
    if (slot != nullptr) {
        QCoreApplication::removeTranslator(slot);
        delete slot;
        slot = nullptr;
    }
    auto *t = new QTranslator(QCoreApplication::instance());
    // The catalogues are compiled with `lrelease -idbased`, so qsTrId("id") finds them.
    if (!t->load(QStringLiteral(":/i18n/%1.qm").arg(code))) {
        delete t;
        return false;
    }
    QCoreApplication::installTranslator(t);
    slot = t;
    return true;
}
}  // namespace

void set_identity(const QString &app_id, const QString &display_name, const QString &icon_path) {
    QCoreApplication::setApplicationName(app_id);
    QGuiApplication::setApplicationDisplayName(display_name);
    QGuiApplication::setDesktopFileName(app_id);
    if (!icon_path.isEmpty()) {
        QGuiApplication::setWindowIcon(QIcon(icon_path));
    }
}

bool set_language(const QString &code) {
    // English first and always: later-installed translators are asked first, so a language only
    // has to translate what it translates.
    if (g_english == nullptr) {
        load_into(g_english, QStringLiteral("en"));
    }
    bool found = true;
    if (code != QStringLiteral("en")) {
        found = load_into(g_language, code);
    } else if (g_language != nullptr) {
        QCoreApplication::removeTranslator(g_language);
        delete g_language;
        g_language = nullptr;
    }
    if (g_engine != nullptr) {
        g_engine->retranslate();
    }
    return found;
}

QString system_locale_name() { return QLocale::system().name(); }

void set_quit_on_last_window_closed(bool quit) { QGuiApplication::setQuitOnLastWindowClosed(quit); }

bool tray_host_registered() {
    QDBusConnection bus = QDBusConnection::sessionBus();
    if (!bus.isConnected()) return false;
    QDBusConnectionInterface *bus_interface = bus.interface();
    const QString service = QStringLiteral("org.kde.StatusNotifierWatcher");
    if (bus_interface == nullptr || !bus_interface->isServiceRegistered(service)) return false;
    QDBusInterface watcher(service, QStringLiteral("/StatusNotifierWatcher"), service, bus);
    if (!watcher.isValid()) return false;
    watcher.setTimeout(500);
    const QVariant hosted = watcher.property("IsStatusNotifierHostRegistered");
    return hosted.isValid() && hosted.toBool();
}

void allow_resource_reads() { qputenv("QML_XHR_ALLOW_FILE_READ", QByteArray("1")); }

namespace {
// Can this process get a GL context at all (the driver is installed, the session allows it)? Qt Quick
// does not fall back by itself: with RHI the window creation just fails, so the answer is needed
// before the first window exists.
bool can_create_opengl_context() {
    QOpenGLContext context;
    if (!context.create()) return false;
    QOffscreenSurface surface;
    surface.setFormat(context.format());
    surface.create();
    return surface.isValid() && context.makeCurrent(&surface);
}

// Only whether the Vulkan loader is there: creating an instance needs QVulkanInstance, which this
// build of the shim does not see. Vulkan is an option for people who ask for it, not the default.
bool can_create_vulkan_instance() {
    QLibrary loader(QStringLiteral("vulkan"), 1);
    return loader.load();
}
}  // namespace

QString choose_renderer(const QString &requested) {
    // Qt's own switches always win: whoever sets them knows what they want.
    const QByteArray qt_backend = qgetenv("QT_QUICK_BACKEND");
    if (!qt_backend.isEmpty()) return QStringLiteral("qt:") + QString::fromLatin1(qt_backend);
    if (qEnvironmentVariableIsSet("QMLSCENE_DEVICE")) return QStringLiteral("qt:QMLSCENE_DEVICE");
    const QByteArray rhi = qgetenv("QSG_RHI_BACKEND");
    QString mode = requested.toLower();
    if (mode.isEmpty() || mode == QStringLiteral("auto")) {
        mode = rhi.isEmpty() ? QStringLiteral("auto") : QString::fromLatin1(rhi).toLower();
    }
    if (mode == QStringLiteral("auto")) mode = QStringLiteral("software");
    if (mode == QStringLiteral("opengl") || mode == QStringLiteral("gl")) {
        if (can_create_opengl_context()) {
            qputenv("QSG_RHI_BACKEND", QByteArray("opengl"));
            return QStringLiteral("opengl");
        }
        mode = QStringLiteral("software-fallback");
    } else if (mode == QStringLiteral("vulkan")) {
        if (can_create_vulkan_instance()) {
            qputenv("QSG_RHI_BACKEND", QByteArray("vulkan"));
            return QStringLiteral("vulkan");
        }
        mode = QStringLiteral("software-fallback");
    }
    qunsetenv("QSG_RHI_BACKEND");
    qputenv("QT_QUICK_BACKEND", QByteArray("software"));
    return mode == QStringLiteral("software-fallback") ? mode : QStringLiteral("software");
}

void set_engine(QQmlApplicationEngine &engine) { g_engine = &engine; }

}  // namespace truesense
