#include "appicon.h"
#include "rust/cxx.h"

#include <QCoreApplication>
#include <QGuiApplication>
#include <QIcon>
#include <QString>

namespace ducknet {

void set_window_icon_from_path(rust::Str path)
{
    QIcon icon;
    if (!path.empty()) {
        icon = QIcon(QString::fromUtf8(path.data(), static_cast<int>(path.size())));
    }
    if (icon.isNull()) {
        icon = QIcon::fromTheme(QStringLiteral("org.kde.ducknetdevtool"));
    }
    if (auto *app = qobject_cast<QGuiApplication *>(QCoreApplication::instance())) {
        app->setWindowIcon(icon);
    }
}

} // namespace ducknet
