use cxx_qt_build::{CxxQtBuilder, QmlModule, PluginType};
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=src/");
    println!("cargo:rerun-if-changed=CMakeLists.txt");
    CxxQtBuilder::new_qml_module(
        QmlModule::new("org.kde.ducknetdevtool")
            .qml_file("src/qml/Main.qml")
            .qml_file("src/qml/DevCertsPage.qml")
            .qml_file("src/qml/CertHelpPage.qml")
            .qml_file("src/qml/UnsupportedPage.qml")
            .qml_file("src/qml/SystemInfoPage.qml")
            .qml_file("src/qml/NginxManagerPage.qml")
            .qml_file("src/qml/EditSitePage.qml")
            .qml_file("src/qml/EditNginxBasePage.qml")
            .qml_file("src/qml/EditPhpfpmBasePage.qml")
            .qml_file("src/qml/SetupToolingPage.qml")
            .qml_file("src/qml/EditorFindBar.qml")
            .qml_file("src/qml/DatabaseManagerPage.qml")
            .qml_file("src/qml/CreateDatabasePage.qml")
            .plugin_type(PluginType::Dynamic),
    )
    .files(["src/devcerts.rs", "src/sysinfo.rs", "src/nginxman.rs", "src/tooling.rs", "src/appinfo.rs", "src/dbman.rs"])
    .build();

    // Build number stamp (DUCKNET_BUILD_NUMBER): git commit count + short
    // hash, so every release build carries a fresh, auto-updating number.
    // Falls back to a UTC timestamp outside git. Re-runs with commits and
    // any src change, and release builds always start from a clean tree.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
    let build_number = git_build_number()
        .unwrap_or_else(|| utc_fallback_build_number());
    println!("cargo:rustc-env=DUCKNET_BUILD_NUMBER={}", build_number);

    // Window-icon bridge (plain cxx; cxx-qt-lib 0.10 has no QIcon).
    // Qt headers come from qmake6 (build host already requires Qt dev packages).
    if let Ok(out) = std::process::Command::new("qmake6")
        .args(["-query", "QT_INSTALL_HEADERS"])
        .output()
    {
        if out.status.success() {
            let headers = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !headers.is_empty() {
                // Note: cxx_build adds the cxx crate's own include dir (rust/cxx.h) itself.
                // Debian/Fedora keep module headers in QtCore/QtGui subdirs; src/ holds appicon.h.
                let mut b = cxx_build::bridge("src/appicon.rs");
                b.file("src/appicon.cpp").std("c++17");
                b.include(&headers);
                b.include(format!("{}/QtCore", headers));
                b.include(format!("{}/QtGui", headers));
                b.include("src");
                b.compile("ducknet-appicon");
                // cargo only forwards `static=` link-lib to the lib target, NOT to same-package
                // bins — pass the archive explicitly so the binary links too.
                if let Ok(out_dir) = std::env::var("OUT_DIR") {
                    println!("cargo:rustc-link-arg={}/libducknet-appicon.a", out_dir);
                }
            }
        }
    }

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = manifest.join("target");
    if let Ok(entries) = fs::read_dir(&target) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                fix_qmldir_files(&path);
            }
        }
    }
    fix_qmldir_files(&manifest.join("target/cxxqt"));
}

fn git_build_number() -> Option<String> {
    let count = std::process::Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;
    if count.is_empty() || count == "0" {
        return None;
    }
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())?;
    Some(format!("{}-{}", count, hash))
}

/// Fallback when git is unavailable (e.g. tarball builds): UTC timestamp.
fn utc_fallback_build_number() -> String {
    // date is universal on Debian/Fedora build hosts.
    std::process::Command::new("date")
        .args(["-u", "+%Y%m%d-%H%M"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn fix_qmldir_files(dir: &PathBuf) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            // Never follow symlinks: cxx-qt-build symlinks the crate root into
            // OUT_DIR/cxxbridge/crate/<name> -> repo root, and path.is_dir()
            // follows it -> infinite recursion (build "hangs" until EMFILE).
            // entry.file_type() uses symlink_metadata, so symlinks are skipped.
            let is_real_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let path = entry.path();
            if is_real_dir {
                fix_qmldir_files(&path);
            } else if path.file_name().map(|n| n == "qmldir").unwrap_or(false) {
                if let Ok(content) = fs::read_to_string(&path) {
                    let module_dir = path.parent().unwrap();
                    // Keep plugin line for dynamic loading, only strip 'prefer' which can force qrc-only lookup.
                    // No mock fallback — real backend via Rust.
                    let fixed: String = content
                        .lines()
                        .filter(|l| !l.trim_start().starts_with("prefer"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let fixed = format!("{}\n", fixed);
                    if fixed != content {
                        let _ = fs::write(&path, fixed);
                    }
                    for qml_line in content.lines().filter(|l| {
                        let trimmed = l.trim_start();
                        trimmed.starts_with("Main ") || trimmed.starts_with("DevCertsPage ") || trimmed.starts_with("CertHelpPage ") || trimmed.starts_with("UnsupportedPage ") || trimmed.starts_with("SystemInfoPage ") || trimmed.starts_with("NginxManagerPage ") || trimmed.starts_with("EditSitePage ") || trimmed.starts_with("EditNginxBasePage ") || trimmed.starts_with("EditPhpfpmBasePage ") ||                         trimmed.starts_with("SetupToolingPage ") || trimmed.starts_with("EditorFindBar ") || trimmed.starts_with("DatabaseManagerPage ") || trimmed.starts_with("CreateDatabasePage ")
                    }) {
                        let parts: Vec<&str> = qml_line.split_whitespace().collect();
                        if parts.len() >= 3 {
                            // qmldir line: "Main 1.0 src/qml/Main.qml" -> path is last token
                            let qml_rel_path = parts[2];
                            // qml_rel_path already is "src/qml/Main.qml", join directly with manifest dir
                            let src_qml = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(qml_rel_path);
                            let dest = module_dir.join(qml_rel_path);
                            if src_qml.exists() {
                                let should_copy = if !dest.exists() {
                                    true
                                } else {
                                    // overwrite if source is newer (handles QML edits during cargo run dev)
                                    let src_m = fs::metadata(&src_qml).and_then(|m| m.modified()).ok();
                                    let dst_m = fs::metadata(&dest).and_then(|m| m.modified()).ok();
                                    match (src_m, dst_m) {
                                        (Some(s), Some(d)) => s > d,
                                        _ => true,
                                    }
                                };
                                if should_copy {
                                    if let Some(parent) = dest.parent() {
                                        let _ = fs::create_dir_all(parent);
                                    }
                                    let _ = fs::copy(&src_qml, &dest);
                                }
                            }
                        }
                    }
                    // Ensure dynamic plugin .so is available in qml_modules for import org.kde.ducknetdevtool.
                    // CxxQt Dynamic plugin is built as cdylib libducknet_dev_tool.so, but qmldir expects
                    // liborg_kde_ducknetdevtool.so in the same dir. Copy/symlink it.
                    let debug_lib = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/libducknet_dev_tool.so");
                    let plugin_so = module_dir.join("liborg_kde_ducknetdevtool.so");
                    if debug_lib.exists() {
                        let should_copy_plugin = if !plugin_so.exists() {
                            true
                        } else {
                            let src_m = fs::metadata(&debug_lib).and_then(|m| m.modified()).ok();
                            let dst_m = fs::metadata(&plugin_so).and_then(|m| m.modified()).ok();
                            match (src_m, dst_m) {
                                (Some(s), Some(d)) => s > d,
                                _ => false,
                            }
                        };
                        if should_copy_plugin {
                            let _ = fs::copy(&debug_lib, &plugin_so);
                        }
                    }
                }
            }
        }
    }
}
