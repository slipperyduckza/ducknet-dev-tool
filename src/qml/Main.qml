import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.ducknetdevtool

Kirigami.ApplicationWindow {
    id: root
    title: "DuckNet Dev Tool"
    width: 1280
    height: 720
    visible: true
    // Fallback size if the user un-maximizes; the real request is the
    // showMaximized() call in Component.onCompleted below.
    minimumWidth: 720
    minimumHeight: 480

    // Explicit runtime request — a bare `visibility:` binding can be
    // overridden by the compositor's restored geometry at map time.
    Component.onCompleted: showMaximized()

    DevCertsManager {
        id: realCertsManager
        Component.onCompleted: console.log("[real] caSetupStatus:", caSetupStatus, "commonName:", commonName, "generated:", JSON.stringify(generatedCerts), "status:", statusMessage, "distroFamily:", distroFamily, "distroId:", distroId, "installedCertPath:", installedCertPath, "updateCommand:", updateCommand)
    }

    pageStack.initialPage: Kirigami.Page {
        id: mainSplitPage
        title: "DuckNet Dev Tool"
        padding: 0
        actions: [ Kirigami.Action { text: "About"; icon.name: "help-about"; onTriggered: root.pageStack.push(aboutPage) } ]
        // Site-editor handoff: the Nginx page stages the file, then opens
        // the full-page editor with the loaded text.
        property string editDomain: ""
        property string editDraft: ""
        function openSiteEditor(domain, text) {
            editDomain = domain
            editDraft = text
            detailLoader.sourceComponent = editSitePageComponent
        }
        function closeSiteEditor() {
            editDomain = ""
            editDraft = ""
            navView.currentIndex = 3
            detailLoader.sourceComponent = nginxPage
        }
        // Base-config editor handoff (nginx.conf with lint, www.conf without):
        // the Nginx page stages the file via loadNginxBaseConfig /
        // loadPhpfpmBaseConfig, then opens the matching full-page editor.
        property string baseDraft: ""
        function openNginxBaseEditor(text) {
            baseDraft = text
            detailLoader.sourceComponent = editNginxBasePageComponent
        }
        function openPhpfpmBaseEditor(text) {
            baseDraft = text
            detailLoader.sourceComponent = editPhpfpmBasePageComponent
        }
        function closeBaseEditor() {
            baseDraft = ""
            navView.currentIndex = 3
            detailLoader.sourceComponent = nginxPage
        }
        // Create-database handoff: Database Manager opens the full-page
        // form; Create/Cancel returns to it (nav index 4 = Database Manager).
        function openCreateDatabase() {
            detailLoader.sourceComponent = createDbPageComponent
        }
        function closeCreateDatabase() {
            navView.currentIndex = 4
            detailLoader.sourceComponent = dbmanPageComponent
        }

        RowLayout {
            anchors.fill: parent
            spacing: 0

            Controls.ScrollView {
                Layout.preferredWidth: 280
                Layout.fillHeight: true
                Layout.minimumWidth: 220
                background: Rectangle { color: Kirigami.Theme.backgroundColor; opacity: 0.3 }
                ColumnLayout {
                    width: parent.width
                    spacing: 0
                    Kirigami.Heading { text: "DEV TOOLS:"; level: 3; Layout.margins: Kirigami.Units.largeSpacing; Layout.bottomMargin: Kirigami.Units.smallSpacing }
                    Controls.Label { text: "Making Development Setup Easier"; opacity: 0.5; font.pointSize: 8; Layout.leftMargin: Kirigami.Units.largeSpacing; Layout.rightMargin: Kirigami.Units.largeSpacing }
                    Kirigami.Separator { Layout.fillWidth: true; Layout.topMargin: Kirigami.Units.smallSpacing }
                    ListView {
                        id: navView
                        Layout.fillWidth: true
                        Layout.preferredHeight: contentHeight
                        interactive: false
                        currentIndex: 0
                        model: ListModel {
                            id: functionModel
                            ListElement { name: "Welcome"; desc: "Start here"; iconName: "go-home"; page: "welcome" }
                            ListElement { name: "Setup Tooling"; desc: "Packages & config"; iconName: "system-software-install"; page: "tooling" }
                            ListElement { name: "DEV HTTPS Certs"; desc: "Local CA + certs"; iconName: "security-high"; page: "devcerts" }
                            ListElement { name: "Nginx Manager"; desc: "Status + start/stop"; iconName: "network-server"; page: "nginx" }
                            ListElement { name: "Database Manager"; desc: "PostgreSQL & MariaDB"; iconName: "server-database"; page: "dbman" }
                            ListElement { name: "System Info"; desc: "Versions & platform"; iconName: "computer"; page: "sysinfo" }
                             ListElement { name: "Dev Tool Help"; desc: "General Help Information"; iconName: "help-about"; page: "help" }
                        }
                        delegate: Controls.ItemDelegate {
                            width: navView.width
                            highlighted: navView.currentIndex === index
                            onClicked: {
                                navView.currentIndex = index
                                if (model.page === "welcome") detailLoader.sourceComponent = welcomePageComponent
                                else if (model.page === "devcerts") detailLoader.sourceComponent = devCertsPageComponent
                                else if (model.page === "sysinfo") detailLoader.sourceComponent = sysInfoPage
                                else if (model.page === "nginx") detailLoader.sourceComponent = nginxPage
                                else if (model.page === "dbman") detailLoader.sourceComponent = dbmanPageComponent
                                else if (model.page === "tooling") detailLoader.sourceComponent = toolingPageComponent
                                else if (model.page === "help") detailLoader.sourceComponent = certHelpComponent
                            }
                            contentItem: RowLayout {
                                spacing: Kirigami.Units.smallSpacing
                                Kirigami.Icon { source: model.iconName; Layout.preferredWidth: 22; Layout.preferredHeight: 22 }
                                ColumnLayout {
                                    spacing: 2
                                    Controls.Label { text: model.name; font.bold: navView.currentIndex===index; elide: Text.ElideRight; Layout.fillWidth: true }
                                    Controls.Label { text: model.desc; opacity: 0.6; font.pointSize: 8; elide: Text.ElideRight; Layout.fillWidth: true }
                                }
                            }
                        }
                    }
                    Item { Layout.fillHeight: true }
                    Kirigami.InlineMessage {
                        id: leftNotification
                        Layout.fillWidth: true
                        Layout.margins: Kirigami.Units.smallSpacing
                        Layout.leftMargin: Kirigami.Units.largeSpacing
                        Layout.rightMargin: Kirigami.Units.largeSpacing
                        visible: false
                        showCloseButton: true
                        type: Kirigami.MessageType.Information
                        Timer {
                            id: leftNotificationTimer
                            interval: 4000
                            onTriggered: leftNotification.visible = false
                        }
                    }
                    Kirigami.Separator { Layout.fillWidth: true }
                    Controls.Label { text: "Wayland & XWayland auto — org.kde.desktop style"; wrapMode: Text.WordWrap; opacity: 0.4; font.pointSize: 7; Layout.margins: Kirigami.Units.smallSpacing; Layout.fillWidth: true }
                }
            }
            Kirigami.Separator { Layout.fillHeight: true; Layout.preferredWidth: 1 }
            Loader {
                id: detailLoader
                Layout.fillWidth: true
                Layout.fillHeight: true
                sourceComponent: welcomePageComponent
                Behavior on opacity { NumberAnimation { duration: 150 } }
            }
        }
        function showLeftNotification(msg, duration, type) {
            leftNotification.text = msg
            leftNotification.type = type !== undefined ? type : Kirigami.MessageType.Information
            leftNotification.visible = true
            leftNotificationTimer.interval = duration || 4000
            leftNotificationTimer.restart()
        }
    }

    Component {
        id: welcomePageComponent
        Kirigami.Page {
            title: "Welcome"
            AppInfo {
                id: appInfo
            }
            Controls.ScrollView {
                anchors.fill: parent
                contentWidth: availableWidth
                Controls.ScrollBar.vertical.policy: Controls.ScrollBar.AlwaysOn
                Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff
                ColumnLayout {
                    width: parent.width
                    anchors.margins: Kirigami.Units.largeSpacing
                    spacing: Kirigami.Units.largeSpacing
                    // Corners are baked into the PNG alpha (no shader /
                    // effects-module dependency on the startup page).
                    // Resolves next to Main.qml in dev (src/qml) and
                    // installed (…/src/qml) trees alike.
                    Image {
                        source: "images/workingduck_wide_bw_400.png"
                        Layout.alignment: Qt.AlignHCenter
                        Layout.preferredWidth: 400
                        Layout.preferredHeight: 200
                        fillMode: Image.PreserveAspectCrop
                    }
                    Kirigami.Heading { text: "Welcome to DuckNet Dev Tool"; level: 2; Layout.alignment: Qt.AlignHCenter }
                    Controls.Label { text: "Pick a function on the left to begin."; opacity: 0.6; Layout.alignment: Qt.AlignHCenter }
                    Controls.Label {
                        text: "The goal of this tool is to create a consistent standardized environment for Fedora 44 and Debian 13 KDE Plasma desktop environments.\n\nIn the current iteration of the tool, the focus is on web based applications.\nWe have chosen Nginx as the web server for its speed and flexibility.\nOur tool allows you to set up Valid SSL Certificates by generating a Trust Authority Certificate locally.\nThis allows you to generate local valid certificates that are trusted by the browser in your development environment.\n\nUse this tool with caution, it is intended for professionals with experience to assist with fresh installations. This tool does make changes to your system, examine the functionality carefully and decide if this tool is right for you. We cannot be held liable for changes made to your system, this is your choice."
                        wrapMode: Text.WordWrap
                        horizontalAlignment: Text.AlignHCenter
                        Layout.fillWidth: true
                        Layout.leftMargin: Kirigami.Units.largeSpacing * 2
                        Layout.rightMargin: Kirigami.Units.largeSpacing * 2
                        opacity: 0.7
                    }
                    Controls.Label {
                        text: appInfo.appName + " v" + appInfo.appVersion + " (build " + appInfo.appBuild + ")"
                        horizontalAlignment: Text.AlignHCenter
                        Layout.fillWidth: true
                        opacity: 0.7
                        font.pointSize: 9
                        font.bold: true
                    }
                    Controls.Label {
                        text: 'Released under the <a href="https://opensource.org/licenses/MIT">MIT License</a>.'
                        textFormat: Text.RichText
                        onLinkActivated: function(link) { Qt.openUrlExternally(link) }
                        horizontalAlignment: Text.AlignHCenter
                        Layout.fillWidth: true
                        opacity: 0.6
                        font.pointSize: 8
                    }
                }
            }
        }
    }
    Component {
        id: devCertsPageComponent
        Loader {
            sourceComponent: realCertsManager.caSetupStatus === "Unsupported" ? unsupportedPageComponent : devCertsRealComponent
        }
    }
    Component {
        id: devCertsRealComponent
        DevCertsPage { devCertsManager: realCertsManager; leftPage: mainSplitPage }
    }
    Component {
        id: unsupportedPageComponent
        UnsupportedPage { devCertsManager: realCertsManager }
    }
    Component {
        id: sysInfoPage
        SystemInfoPage {}
    }
    Component {
        id: nginxPage
        NginxManagerPage { leftPage: mainSplitPage }
    }
    Component {
        id: dbmanPageComponent
        DatabaseManagerPage { leftPage: mainSplitPage }
    }
    Component {
        id: createDbPageComponent
        CreateDatabasePage { leftPage: mainSplitPage }
    }
    Component {
        id: editSitePageComponent
        EditSitePage {
            leftPage: mainSplitPage
            domain: mainSplitPage.editDomain
            initialText: mainSplitPage.editDraft
        }
    }
    Component {
        id: editNginxBasePageComponent
        EditNginxBasePage {
            leftPage: mainSplitPage
            initialText: mainSplitPage.baseDraft
        }
    }
    Component {
        id: editPhpfpmBasePageComponent
        EditPhpfpmBasePage {
            leftPage: mainSplitPage
            initialText: mainSplitPage.baseDraft
        }
    }
    Component {
        id: toolingPageComponent
        SetupToolingPage { leftPage: mainSplitPage }
    }
    Component {
        id: certHelpComponent
        CertHelpPage {}
    }
    Component {
        id: aboutPage
        Kirigami.Page {
            title: "About"
            ColumnLayout {
                anchors.centerIn: parent
                Kirigami.Icon { source: "org.kde.ducknetdevtool"; Layout.preferredWidth: 64; Layout.preferredHeight: 64; Layout.alignment: Qt.AlignHCenter }
                Kirigami.Heading { text: "DuckNet Dev Tool 0.1.0"; level: 2; Layout.alignment: Qt.AlignHCenter }
                Controls.Label { text: "MIT • KDE Plasma 6.3 • cxx-qt 0.10"; Layout.alignment: Qt.AlignHCenter; opacity: 0.7 }
            }
        }
    }
}
