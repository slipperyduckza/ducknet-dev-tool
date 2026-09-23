cmake_minimum_required(VERSION 3.24)

project(ducknet-dev-tool VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)
set(CMAKE_AUTORCC ON)

# Rust / Corrosion - will be found via cxx-qt-cmake integration
# find_package(Corrosion REQUIRED)

# ECM / KDE
find_package(ECM 6.0 REQUIRED NO_MODULE)
list(APPEND CMAKE_MODULE_PATH ${ECM_MODULE_PATH})

include(KDEInstallDirs)
include(KDECMakeSettings)
include(ECMUninstallTarget)

# Qt6 - required components for cxx-qt + QML + Kirigami
find_package(Qt6 REQUIRED COMPONENTS Core Gui Qml Quick QuickControls2 Svg)

# Optional KDE / Kirigami integration - warn but don't fail hard for first attempt
include(ECMFindQmlModule OPTIONAL)
if(COMMAND ecm_find_qmlmodule)
    ecm_find_qmlmodule(org.kde.kirigami QUIET)
    if(NOT org.kde.kirigami_FOUND)
        message(WARNING "org.kde.kirigami QML module not found - QML will still compile but Kirigami pages may not render without it")
    endif()
else()
    message(WARNING "ECMFindQmlModule not available - skipping kirigami check")
endif()

find_package(KF6 QUIET COMPONENTS QQC2DesktopStyle)
if(NOT KF6_FOUND)
    message(WARNING "KF6 QQC2DesktopStyle not found - styling will fallback to default")
endif()

# cxx-qt cmake integration - fetch if not installed system-wide
find_package(CxxQt QUIET)
if(NOT CxxQt_FOUND)
    include(FetchContent)
    FetchContent_Declare(
        CxxQt
        GIT_REPOSITORY https://github.com/KDAB/cxx-qt-cmake.git
        GIT_TAG main
    )
    FetchContent_MakeAvailable(CxxQt)
endif()

# Import Rust crate via CxxQt (wraps Corrosion) - requires cxx-qt-cmake
set(CORROSION_INSTALL_EXECUTABLE ON CACHE BOOL "Install Rust executables" FORCE)
if(COMMAND cxx_qt_import_crate)
    cxx_qt_import_crate(MANIFEST_PATH Cargo.toml CRATES ducknet-dev-tool QT_MODULES Qt::Core Qt::Gui Qt::Qml Qt::Quick Qt::QuickControls2 Qt::Svg)
    if(COMMAND cxx_qt_import_qml_module)
        cxx_qt_import_qml_module(ducknet-dev-tool_qml_module
            URI "org.kde.ducknetdevtool"
            SOURCE_CRATE ducknet-dev-tool
        )
    endif()
else()
    message(WARNING "cxx_qt_import_crate not available - falling back to corrosion_import_crate (QML module will not be built via CMake)")
    corrosion_import_crate(MANIFEST_PATH Cargo.toml)
endif()

# Executable - cxx-qt provides the Rust static lib; we link via CMake
# The Rust crate already defines the binary via [[bin]] or src/main.rs,
# but for CMake+Corrosion we create an alias target.
# If src/main.rs defines `fn main`, corrosion_import_crate creates `ducknet-dev-tool` binary.
# We just ensure QML module is linked.

# Installation - binary install
# Corrosion creates CMake targets for Cargo binaries.
# For the shared library (cdylib) we still use plain install(TARGETS).
if(COMMAND corrosion_install)
    if(TARGET ducknet-dev-tool)
        corrosion_install(TARGETS ducknet-dev-tool)
    endif()
    if(TARGET live_test)
        corrosion_install(TARGETS live_test)
    endif()
endif()
if(TARGET ducknet_dev_tool)
    # shared lib for QML dynamic plugin (if built as cdylib)
    install(TARGETS ducknet_dev_tool DESTINATION ${CMAKE_INSTALL_LIBDIR} OPTIONAL)
    install(TARGETS ducknet_dev_tool DESTINATION ${KDE_INSTALL_QMLDIR}/org/kde/ducknetdevtool OPTIONAL)
endif()
if(TARGET ducknet-dev-tool_qml_module)
    install(TARGETS ducknet-dev-tool_qml_module DESTINATION ${KDE_INSTALL_QMLDIR}/org/kde/ducknetdevtool OPTIONAL)
endif()

# Fallback: install cargo-built binary directly if corrosion_install didn't already
# place it at /usr/bin. Handles DESTDIR for AppImage (DESTDIR=AppDir).
install(CODE "
    set(_dest \"\$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/bin/ducknet-dev-tool\")
    if(NOT EXISTS \"\${_dest}\")
        set(_installed FALSE)
        foreach(_triple IN ITEMS x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu)
            set(_src \"${CMAKE_BINARY_DIR}/cargo/build/\${_triple}/release/ducknet-dev-tool\")
            if(EXISTS \"\${_src}\")
                file(INSTALL \"\${_src}\" DESTINATION \"\$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/bin\"
                     FILE_PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
                message(STATUS \"Fallback install: copied ducknet-dev-tool from \${_src} to \$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/bin\")
                set(_installed TRUE)
                break()
            endif()
        endforeach()
        if(NOT _installed)
            set(_alt \"${CMAKE_BINARY_DIR}/bin/ducknet-dev-tool\")
            if(EXISTS \"\${_alt}\")
                file(INSTALL \"\${_alt}\" DESTINATION \"\$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/bin\"
                     FILE_PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
                message(STATUS \"Fallback install: copied ducknet-dev-tool from \${_alt}\")
            endif()
        endif()
    endif()
")

install(FILES org.kde.ducknetdevtool.desktop
    DESTINATION ${KDE_INSTALL_APPDIR}
)

# JSON is optional - only install if it exists as plugin metadata
if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/org.kde.ducknetdevtool.json")
    install(FILES org.kde.ducknetdevtool.json
        DESTINATION ${KDE_INSTALL_APPDIR}
    )
endif()

# App icon: scalable SVG + fixed-size PNGs (X11/Wayland taskbars often need PNG).
install(FILES org.kde.ducknetdevtool.svg
    DESTINATION ${KDE_INSTALL_ICONDIR}/hicolor/scalable/apps/
)
if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/icons/hicolor/256x256/apps/org.kde.ducknetdevtool.png")
    install(DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/icons/hicolor/"
        DESTINATION "${KDE_INSTALL_ICONDIR}/hicolor"
        FILES_MATCHING PATTERN "*.png")
endif()

install(FILES org.kde.ducknetdevtool.metainfo.xml
    DESTINATION ${KDE_INSTALL_METAINFODIR}
)

# QML files: install to QML import path. DevCertsManager is provided
# by Rust (cxx-qt) - no QML mock. Includes UnsupportedPage for non-Debian/Fedora.
if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/qml/Main.qml")
    install(DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/src/qml/"
        DESTINATION "${KDE_INSTALL_QMLDIR}/org/kde/ducknetdevtool/src/qml"
        FILES_MATCHING PATTERN "*.qml")
    # Banner art for WelcomePage.qml (qml-only glob above skips images).
    install(DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/src/qml/"
        DESTINATION "${KDE_INSTALL_QMLDIR}/org/kde/ducknetdevtool/src/qml"
        FILES_MATCHING PATTERN "*.png")
endif()
install(CODE "
    set(_qml_dest \"\$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/${KDE_INSTALL_QMLDIR}/org/kde/ducknetdevtool\")
    file(MAKE_DIRECTORY \"\${_qml_dest}\")
    # Create symlink so Qt can find the plugin by its metadata name.
    # The Rust cdylib is libducknet_dev_tool.so, QML module plugin may be libducknet-dev-tool_qml_module.so.
    # Ensure liborg_kde_ducknetdevtool.so exists for qmldir \"optional plugin org_kde_ducknetdevtool\".
    if(EXISTS \"\${_qml_dest}/libducknet_dev_tool.so\" AND NOT EXISTS \"\${_qml_dest}/liborg_kde_ducknetdevtool.so\")
        file(CREATE_LINK \"\${_qml_dest}/libducknet_dev_tool.so\" \"\${_qml_dest}/liborg_kde_ducknetdevtool.so\" SYMBOLIC)
    endif()
    file(GLOB _cand \"\${_qml_dest}/libducknet*.so\")
    foreach(_so IN LISTS _cand)
        get_filename_component(_name \"\${_so}\" NAME)
        if(NOT _name STREQUAL \"liborg_kde_ducknetdevtool.so\")
            if(NOT EXISTS \"\${_qml_dest}/liborg_kde_ducknetdevtool.so\")
                file(CREATE_LINK \"\${_so}\" \"\${_qml_dest}/liborg_kde_ducknetdevtool.so\" SYMBOLIC)
            endif()
        endif()
    endforeach()
    # Generate qmldir for installed AppImage. Rust plugin provides DevCertsManager.
    # Distro detection via /etc/os-release (Debian/Fedora) happens at runtime in src/common.rs.
    file(WRITE \"\${_qml_dest}/qmldir\" \"module org.kde.ducknetdevtool\noptional plugin org_kde_ducknetdevtool\nclassname org_kde_ducknetdevtool_plugin\ntypeinfo plugin.qmltypes\nprefer :/qt/qml/org/kde/ducknetdevtool/\nMain 1.0 src/qml/Main.qml\nDevCertsPage 1.0 src/qml/DevCertsPage.qml\nCertHelpPage 1.0 src/qml/CertHelpPage.qml\nUnsupportedPage 1.0 src/qml/UnsupportedPage.qml\nSystemInfoPage 1.0 src/qml/SystemInfoPage.qml\nNginxManagerPage 1.0 src/qml/NginxManagerPage.qml\nEditSitePage 1.0 src/qml/EditSitePage.qml\nEditNginxBasePage 1.0 src/qml/EditNginxBasePage.qml\nEditPhpfpmBasePage 1.0 src/qml/EditPhpfpmBasePage.qml\nSetupToolingPage 1.0 src/qml/SetupToolingPage.qml\nEditorFindBar 1.0 src/qml/EditorFindBar.qml\nDatabaseManagerPage 1.0 src/qml/DatabaseManagerPage.qml\nCreateDatabasePage 1.0 src/qml/CreateDatabasePage.qml\n\")
    message(STATUS \"Generated qmldir at \${_qml_dest}/qmldir\")
    # Also try to install plugin.qmltypes if present
    # NOTE: file(INSTALL) already prepends $ENV{DESTDIR}, so do NOT include it in DESTINATION
    # (including it causes double-DESTDIR: AppDir/home/.../AppDir/...).
    file(GLOB _qmltypes_candidates \"${CMAKE_BINARY_DIR}/cargo/build/*/release/build/*/out/qt-build-utils/qml_modules/org/kde/ducknetdevtool/plugin.qmltypes\" \"${CMAKE_BINARY_DIR}/cxxqt/qml_modules/org/kde/ducknetdevtool/plugin.qmltypes\")
    list(LENGTH _qmltypes_candidates _qcnt)
    if(_qcnt GREATER 0)
        list(GET _qmltypes_candidates 0 _src_qtypes)
        file(INSTALL \"\${_src_qtypes}\" DESTINATION \"${CMAKE_INSTALL_PREFIX}/${KDE_INSTALL_QMLDIR}/org/kde/ducknetdevtool\")
    endif()
")

feature_summary(WHAT ALL INCLUDE_QUIET_PACKAGES)
