use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QQuickStyle, QString, QUrl};
use std::path::PathBuf;

mod appicon;

/// True in AppImage; host QML paths must be skipped.
/// Bundled Qt 6.8 vs host Qt (e.g. Fedora 6.11) modules are incompatible.
fn in_appimage() -> bool {
    if std::env::var("APPIMAGE").is_ok() || std::env::var("APPDIR").is_ok() {
        return true;
    }
    if let Ok(exe) = std::env::current_exe() {
        if exe.to_string_lossy().starts_with("/tmp/.mount_") {
            return true;
        }
    }
    false
}

fn add_qml_import_paths(engine: &mut cxx::UniquePtr<QQmlApplicationEngine>) {
    if let Some(mut eng) = engine.as_mut() {
        eng.as_mut().add_import_path(&QString::from("qrc:/qt/qml"));
        eng.as_mut().add_plugin_path(&QString::from("qrc:/qt/qml"));
    }

    // Host paths are dev-only; AppImage must not see host Qt.
    let appimage = in_appimage();
    if appimage {
        eprintln!("Running as AppImage, skipping host system QML paths");
    }

    if !appimage {
    for p in [
        "/app/lib/qml",
        "/app/lib/x86_64-linux-gnu/qt6/qml",
        "/app/lib/qt6/qml",
        "/usr/lib/qml",
        "/usr/lib/x86_64-linux-gnu/qt6/qml",
        // Fedora / generic lib64 locations
        "/usr/lib64/qt6/qml",
        "/usr/lib/qt6/qml",
        "/usr/lib64/qml",
    ] {
        if std::path::Path::new(p).exists() {
            if let Some(mut eng) = engine.as_mut() {
                eng.as_mut().add_import_path(&QString::from(p));
                eng.as_mut().add_plugin_path(&QString::from(p));
            }
        }
    }
    } // end host-only guard (AppImage must not see host Qt)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin_dir) = exe.parent() {
            // Probe Debian multiarch, Fedora lib64, and qt.conf layouts.
            for rel in ["../lib/x86_64-linux-gnu/qt6/qml", "../lib/qt6/qml", "../lib64/qt6/qml", "../lib/qml", "../lib64/qml", "../qml", "../../lib/x86_64-linux-gnu/qt6/qml"] {
                let qml = bin_dir.join(rel);
                if qml.exists() {
                    if let Some(mut eng) = engine.as_mut() {
                        eng.as_mut().add_import_path(&QString::from(qml.to_string_lossy().to_string()));
                        eng.as_mut().add_plugin_path(&QString::from(qml.to_string_lossy().to_string()));
                    }
                }
            }
            let app_qml = bin_dir.join("../qml");
            if app_qml.exists() {
                if let Some(mut eng) = engine.as_mut() {
                    eng.as_mut().add_import_path(&QString::from(app_qml.to_string_lossy().to_string()));
                }
            }
        }
    }
    // /app paths are dev-only; skip in AppImage.
    if !appimage {
    for p in [
        "/app/share/ducknet-dev-tool/qml",
        "/app/share/qml/org/kde/ducknetdevtool",
        "/app/lib/qml/org/kde/ducknetdevtool",
    ] {
        if std::path::Path::new(p).exists() {
            if let Some(mut eng) = engine.as_mut() {
                eng.as_mut().add_import_path(&QString::from(p));
            }
        }
    }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Cargo dev-tree paths do not exist on user machines; skip in AppImage.
    if appimage {
        return;
    }
    let build_dir = manifest_dir.join("target/debug/build");
    if let Ok(entries) = std::fs::read_dir(&build_dir) {
        for entry in entries.flatten() {
            let qml_modules = entry.path().join("out/qt-build-utils/qml_modules");
            let qmldir = qml_modules.join("org/kde/ducknetdevtool/qmldir");
            if qmldir.exists() {
                // Dynamic import needs the plugin .so present.
                let plugin_dir = qml_modules.join("org/kde/ducknetdevtool");
                let plugin_so = plugin_dir.join("liborg_kde_ducknetdevtool.so");
                let debug_lib = manifest_dir.join("target/debug/libducknet_dev_tool.so");
                if !plugin_so.exists() && debug_lib.exists() {
                    let _ = std::fs::copy(&debug_lib, &plugin_so);
                    eprintln!("Copied plugin {} -> {}", debug_lib.display(), plugin_so.display());
                }
                if let Some(mut eng) = engine.as_mut() {
                    let path_str = qml_modules.to_string_lossy().to_string();
                    eprintln!("Adding QML import/plugin path: {}", path_str);
                    eng.as_mut().add_import_path(&QString::from(&path_str));
                    eng.as_mut().add_plugin_path(&QString::from(&path_str));
                    eng.as_mut().add_plugin_path(&QString::from(plugin_dir.to_string_lossy().to_string()));
                }
                break;
            }
        }
    }

    let target_debug = manifest_dir.join("target/debug");
    if target_debug.exists() {
        if let Some(mut eng) = engine.as_mut() {
            eng.as_mut().add_plugin_path(&QString::from(target_debug.to_string_lossy().to_string()));
        }
    }

    for cmake_build in ["/tmp/duck-build3", "/tmp/duck-build"] {
        let p = PathBuf::from(cmake_build).join("qml_modules");
        if p.exists() {
            if let Some(mut eng) = engine.as_mut() {
                eng.as_mut().add_import_path(&QString::from(p.to_string_lossy().to_string()));
                eng.as_mut().add_plugin_path(&QString::from(p.to_string_lossy().to_string()));
            }
        }
    }

    if let Some(mut eng) = engine.as_mut() {
        let src_qml = manifest_dir.join("src/qml");
        if src_qml.exists() {
            eng.as_mut().add_import_path(&QString::from(manifest_dir.to_string_lossy().to_string()));
        }
    }
}

fn main() {
    cxx_qt::init_crate!(cxx_qt_lib);

    let mut app = QGuiApplication::new();

    QGuiApplication::set_desktop_file_name(&QString::from("org.kde.ducknetdevtool"));
    // org.kde.desktop needs system modules; a missing/incompatible style is fatal at load.
    // The AppImage ships neither it nor host modules, so keep the bundled default style there.
    if !in_appimage() {
        QQuickStyle::set_style(&QString::from("org.kde.desktop"));
    } else {
        eprintln!("AppImage: using bundled default QtQuick style (org.kde.desktop not shipped)");
    }

    app.as_mut().unwrap().set_application_name(&QString::from("ducknet-dev-tool"));
    app.as_mut().unwrap().set_application_version(&QString::from("0.1.0"));

    appicon::set_window_icon();

    let mut engine = QQmlApplicationEngine::new();
    add_qml_import_paths(&mut engine);

    // Try candidate Main.qml locations.
    let mut loaded = false;
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin_dir) = exe.parent() {
            candidates.push(bin_dir.join("../share/ducknet-dev-tool/qml/Main.qml"));
            candidates.push(bin_dir.join("../lib/x86_64-linux-gnu/qt6/qml/org/kde/ducknetdevtool/src/qml/Main.qml"));
            candidates.push(bin_dir.join("../lib/qt6/qml/org/kde/ducknetdevtool/src/qml/Main.qml"));
            // Fedora lib64 + qt.conf usr/qml mirror.
            candidates.push(bin_dir.join("../lib64/qt6/qml/org/kde/ducknetdevtool/src/qml/Main.qml"));
            candidates.push(bin_dir.join("../qml/org/kde/ducknetdevtool/src/qml/Main.qml"));
            if let Ok(real) = std::fs::read_link(&exe) {
                if let Some(real_bin) = real.parent() {
                    candidates.push(real_bin.join("../share/ducknet-dev-tool/qml/Main.qml"));
                }
            }
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/qml/Main.qml"));
    candidates.push(PathBuf::from("/app/share/ducknet-dev-tool/qml/Main.qml"));
    candidates.push(PathBuf::from("/app/lib/qml/org/kde/ducknetdevtool/src/qml/Main.qml"));
    candidates.push(PathBuf::from("/app/lib/x86_64-linux-gnu/qt6/qml/org/kde/ducknetdevtool/src/qml/Main.qml"));
    candidates.push(PathBuf::from("/app/lib/qt6/qml/org/kde/ducknetdevtool/src/qml/Main.qml"));
    candidates.push(PathBuf::from("/usr/share/ducknet-dev-tool/qml/Main.qml"));
    candidates.push(PathBuf::from("/usr/lib/x86_64-linux-gnu/qt6/qml/org/kde/ducknetdevtool/src/qml/Main.qml"));
    candidates.push(PathBuf::from("/usr/lib64/qt6/qml/org/kde/ducknetdevtool/src/qml/Main.qml"));
    candidates.push(PathBuf::from("/usr/lib/qt6/qml/org/kde/ducknetdevtool/src/qml/Main.qml"));
    candidates.push(PathBuf::from("/usr/qml/org/kde/ducknetdevtool/src/qml/Main.qml"));
    for cand in &candidates {
        if cand.exists() {
            let local_url = QUrl::from_local_file(&QString::from(cand.to_string_lossy().to_string()));
            eprintln!("Loading QML via file:// {}", cand.display());
            engine.as_mut().unwrap().load(&local_url);
            loaded = true;
            break;
        }
    }
    if !loaded {
        // Fallback to qrc for installed builds.
        for qrc in [
            "qrc:/qt/qml/org/kde/ducknetdevtool/src/qml/Main.qml",
            "qrc:/qt/qml/org/kde/ducknetdevtool/Main.qml",
        ] {
            eprintln!("Trying QML via {}", qrc);
            let qrc_url = QUrl::from(&QString::from(qrc));
            engine.as_mut().unwrap().load(&qrc_url);
            break;
        }
    }

    let exit_code = app.as_mut().unwrap().exec();
    std::process::exit(exit_code);
}