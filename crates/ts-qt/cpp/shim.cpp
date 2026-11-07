#include "shim.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QLocale>
#include <QtCore/QTranslator>
#include <QtDBus/QDBusConnection>
#include <QtDBus/QDBusConnectionInterface>
#include <QtDBus/QDBusInterface>
#include <QtGui/QGuiApplication>
#include <QtGui/QIcon>
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

bool set_language(const QString &code, bool rtl) {
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
    QGuiApplication::setLayoutDirection(rtl ? Qt::RightToLeft : Qt::LeftToRight);
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

void prefer_software_renderer() {
    if (qEnvironmentVariableIsSet("QT_QUICK_BACKEND") || qEnvironmentVariableIsSet("QMLSCENE_DEVICE")) return;
    if (qgetenv("TRUESENSE_QT_RENDER").toLower() == "opengl") return;
    qputenv("QT_QUICK_BACKEND", QByteArray("software"));
}

void set_engine(QQmlApplicationEngine &engine) { g_engine = &engine; }

}  // namespace truesense
