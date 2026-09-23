//! Window-icon bridge: cxx-qt-lib exposes no QIcon, so call qApp->setWindowIcon() via plain cxx.
//! Without it compositors fall back to a generic X11 icon.

#[cxx::bridge(namespace = "ducknet")]
mod ffi {
    unsafe extern "C++" {
        include!("appicon.h");
        /// Set window icon from file; empty/missing falls back to theme icon.
        fn set_window_icon_from_path(path: &str);
    }
}

fn candidate_icons() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin) = exe.parent() {
            // AppImage layout (usr/bin -> usr/share/...)
            out.push(bin.join("../share/icons/hicolor/256x256/apps/org.kde.ducknetdevtool.png"));
            out.push(bin.join("../share/icons/hicolor/128x128/apps/org.kde.ducknetdevtool.png"));
            out.push(bin.join("../share/icons/hicolor/scalable/apps/org.kde.ducknetdevtool.svg"));
            out.push(bin.join("../org.kde.ducknetdevtool.svg"));
        }
    }
    out.push(std::path::PathBuf::from(
        "/usr/share/icons/hicolor/256x256/apps/org.kde.ducknetdevtool.png",
    ));
    out.push(std::path::PathBuf::from(
        "/usr/share/icons/hicolor/scalable/apps/org.kde.ducknetdevtool.svg",
    ));
    // cargo dev fallback
    out.push(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("icons/hicolor/256x256/apps/org.kde.ducknetdevtool.png"),
    );
    out.push(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("org.kde.ducknetdevtool.svg"));
    out
}

/// Set window icon to duck logo; fall back to theme icon.
pub fn set_window_icon() {
    for p in candidate_icons() {
        if p.is_file() {
            let s = p.to_string_lossy().to_string();
            eprintln!("Setting window icon from {}", p.display());
            ffi::set_window_icon_from_path(&s);
            return;
        }
    }
    eprintln!("No icon file found, falling back to theme icon");
    ffi::set_window_icon_from_path("");
}
