//! Application identity for the Welcome page (name, version, build).
//! Static strings baked at compile time; no runtime probing.

use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, app_name, cxx_name = "appName")]
        #[qproperty(QString, app_version, cxx_name = "appVersion")]
        #[qproperty(QString, app_build, cxx_name = "appBuild")]
        type AppInfo = super::AppInfoRust;
    }
}

pub struct AppInfoRust {
    app_name: QString,
    app_version: QString,
    app_build: QString,
}

impl Default for AppInfoRust {
    fn default() -> Self {
        Self {
            app_name: QString::from("DuckNet Dev Tool"),
            app_version: QString::from(env!("CARGO_PKG_VERSION")),
            app_build: QString::from(env!("DUCKNET_BUILD_NUMBER")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_stamped() {
        let info = AppInfoRust::default();
        assert_eq!(info.app_name.to_string(), "DuckNet Dev Tool");
        assert_eq!(info.app_version.to_string(), env!("CARGO_PKG_VERSION"));
        assert!(!info.app_build.to_string().is_empty());
    }
}
