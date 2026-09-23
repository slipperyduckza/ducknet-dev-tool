import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.ducknetdevtool

Kirigami.Page {
    id: nginxPage
    title: "Nginx Manager"
    property var leftPage: null
    // Site log viewer memory for Refresh (domain + "error_log"/"access_log").
    property string siteLogDomain: ""
    property string siteLogKind: "error_log"

    function notifyLeft(msg, duration, type) {
        if (leftPage && leftPage.showLeftNotification)
            leftPage.showLeftNotification(msg, duration, type)
    }

    NginxManager {
        id: nginxman
    }

    Component.onCompleted: {
        nginxman.refreshStatus()
        nginxman.refreshBaseStatus()
    }

    // Workers fluctuate across reloads — keep the status bar live.
    // refreshLive touches only running/workers, never status messages.
    Timer {
        id: statusTimer
        interval: 5000
        running: true
        repeat: true
        triggeredOnStart: false
        onTriggered: nginxman.refreshLive()
    }

    // Restart blocks on pkexec/systemctl, so sequence the two popups:
    // "requested" paints first, the restart runs, then the result replaces it.
    Timer {
        id: restartTimer
        interval: 400
        repeat: false
        onTriggered: {
            let ok = nginxman.restartNginx()
            nginxman.refreshStatus()
            if (ok)
                nginxPage.notifyLeft("NGINX restarted", 4000)
            else
                nginxPage.notifyLeft("Restart failed: " + nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
        }
    }
    Timer {
        id: phpfpmRestartTimer
        interval: 400
        repeat: false
        onTriggered: {
            let ok = nginxman.restartPhpFpm()
            nginxman.refreshStatus()
            if (ok)
                nginxPage.notifyLeft("PHP-FPM restarted", 4000)
            else
                nginxPage.notifyLeft("Restart failed: " + nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
        }
    }

    Controls.ScrollView {
        id: nginxScroll
        anchors.fill: parent
        contentWidth: availableWidth
        // Persistent bar — overlayPolicy hides it until scrolled, which reads
        // as "no scrolling" on a page whose domain list keeps growing.
        Controls.ScrollBar.vertical.policy: Controls.ScrollBar.AlwaysOn
        Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff

        ColumnLayout {
            // -24 keeps content clear of the overlaid AlwaysOn scrollbar.
            width: nginxScroll.availableWidth - 24
            anchors.margins: Kirigami.Units.largeSpacing
            spacing: Kirigami.Units.smallSpacing

            Kirigami.Heading { text: "Developers Handy NGINX Manager"; level: 2 }
            Controls.Label {
                text: "Simplifying the complexity of setting up web servers using NGINX for your web applications"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Status bar ----
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.largeSpacing
                Controls.Label { text: "NGINX STATUS:"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: nginxman.nginxRunning ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Label {
                    text: nginxman.nginxStatusText
                    font.bold: true
                    color: nginxman.nginxRunning ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Item { Layout.fillWidth: true }
                Controls.Label { text: "NGINX WORKERS:"; font.bold: true }
                Controls.Label { text: nginxman.workerCount; font.bold: true }
            }
            Controls.Label {
                text: nginxman.statusMessage
                visible: nginxman.statusMessage !== ""
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
                font.pointSize: 8
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Actions ----
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Button {
                    text: "Start NGINX"
                    icon.name: "media-playback-start"
                    enabled: !nginxman.nginxRunning
                    onClicked: {
                        nginxman.startNginx()
                        nginxman.refreshStatus()
                    }
                }
                Controls.Button {
                    text: "Stop NGINX"
                    icon.name: "media-playback-stop"
                    enabled: nginxman.nginxRunning
                    onClicked: {
                        nginxman.stopNginx()
                        nginxman.refreshStatus()
                    }
                }
                Controls.Button {
                    text: "Restart NGINX"
                    icon.name: "view-refresh"
                    enabled: nginxman.nginxRunning
                    onClicked: {
                        nginxPage.notifyLeft("NGINX restart requested", 2500)
                        restartTimer.restart()
                    }
                }
                Item { Layout.fillWidth: true }
                Controls.Button {
                    text: "Refresh"
                    icon.name: "view-refresh"
                    onClicked: nginxman.refreshStatus()
                }
            }
            Controls.Label {
                text: "Start / Stop / Restart (NGINX and PHP-FPM) run via pkexec (or passwordless sudo)."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.5
                font.pointSize: 8
            }

            // ---- PHP-FPM status bar (php8.4-fpm on Debian, php-fpm on Fedora) ----
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.largeSpacing
                Controls.Label { text: "PHP-FPM STATUS:"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: nginxman.phpfpmRunning ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Label {
                    text: nginxman.phpfpmStatusText
                    font.bold: true
                    color: nginxman.phpfpmRunning ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Item { Layout.fillWidth: true }
                Controls.Label { text: "PHP-FPM WORKERS:"; font.bold: true }
                Controls.Label { text: nginxman.phpfpmWorkerCount; font.bold: true }
            }

            // ---- PHP-FPM actions ----
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Button {
                    text: "Start PHP-FPM"
                    icon.name: "media-playback-start"
                    enabled: !nginxman.phpfpmRunning
                    onClicked: {
                        nginxman.startPhpFpm()
                        nginxman.refreshStatus()
                    }
                }
                Controls.Button {
                    text: "Stop PHP-FPM"
                    icon.name: "media-playback-stop"
                    enabled: nginxman.phpfpmRunning
                    onClicked: {
                        nginxman.stopPhpFpm()
                        nginxman.refreshStatus()
                    }
                }
                Controls.Button {
                    text: "Restart PHP-FPM"
                    icon.name: "view-refresh"
                    enabled: nginxman.phpfpmRunning
                    onClicked: {
                        nginxPage.notifyLeft("PHP-FPM restart requested", 2500)
                        phpfpmRestartTimer.restart()
                    }
                }
                Item { Layout.fillWidth: true }
                Controls.Button {
                    text: "Refresh"
                    icon.name: "view-refresh"
                    onClicked: nginxman.refreshStatus()
                }
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Conform Nginx Configuration ----
            Kirigami.Heading { text: "Conform Nginx Configuration"; level: 3 }
            Controls.Label {
                text: "This app works with both Debian 13 and Fedora 44, the configuration philosophies between these two is different. It may be opinionated, but we will conform Fedora's nginx configuration to use the [sites-available]~[sites-enabled] method used by Debian. This simplifies the tool design so that we have a unified site configuration path in the development environment. Conforming is applied automatically by the Setup Tooling [Install NGINX PHP] step on Fedora — the status below confirms it took effect."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Label { text: "CONFORMED:"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: nginxman.sitesConformed ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Label { text: "NGINX USER:"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: nginxman.devuserActive ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Label {
                    text: nginxman.nginxUser !== "" ? nginxman.nginxUser : "unknown"
                    font.bold: true
                    color: nginxman.devuserActive ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Button {
                    text: "Run NGINX as DevUser"
                    icon.name: "system-users"
                    enabled: !nginxman.devuserActive
                    onClicked: {
                        nginxman.runAsDevUser()
                        nginxman.refreshStatus()
                    }
                }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Label { text: "PHP-FPM USER:"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: nginxman.phpfpmDevuserActive ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Label {
                    text: nginxman.phpfpmUser !== "" ? nginxman.phpfpmUser : "unknown"
                    font.bold: true
                    color: nginxman.phpfpmDevuserActive ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Button {
                    text: "Run PHP-FPM as DevUser"
                    icon.name: "system-users"
                    enabled: !nginxman.phpfpmDevuserActive
                    onClicked: {
                        var ok = nginxman.runPhpFpmAsDevUser()
                        nginxman.refreshStatus()
                        if (!ok)
                            nginxPage.notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                    }
                }
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Base Configuration files for NGINX and PHP-FPM ----
            Kirigami.Heading { text: "Base Configuration files for NGINX and PHP-FPM"; level: 3 }
            Controls.Label {
                text: "WARNING: USE WITH CAUTION! This section provides editors for the NGINX and PHP-FPM base config files for tuning purposes"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
            }
            // Backup folder status + browser (xdg-open ~/.local/backups)
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Label { text: "Backup folder:"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: nginxman.backupFolderReady ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Label {
                    text: nginxman.backupFolderStatus
                    font.bold: true
                    color: nginxman.backupFolderReady ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Item { Layout.fillWidth: true }
                Controls.Button {
                    text: "Browse Backups"
                    icon.name: "folder-open"
                    enabled: nginxman.backupFolderReady
                    onClicked: {
                        var ok = nginxman.openBackupFolder()
                        if (!ok)
                            nginxPage.notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                    }
                }
            }
            // NGINX base tool line
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Label { text: "NGINX BASE CONFIG(nginx.conf):"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: nginxman.nginxBaseBackedUp ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Label {
                    text: nginxman.nginxBaseBackupStatus
                    font.bold: true
                    color: nginxman.nginxBaseBackedUp ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Item { Layout.fillWidth: true }
                Controls.Button {
                    text: "Backup NGINX"
                    icon.name: "document-save"
                    onClicked: {
                        var ok = nginxman.backupNginxBase()
                        nginxman.refreshStatus()
                        nginxman.refreshBaseStatus()
                        if (!ok)
                            nginxPage.notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                    }
                }
                Controls.Button {
                    text: "Edit NGINX"
                    icon.name: "document-edit"
                    onClicked: {
                        if (nginxman.loadNginxBaseConfig()) {
                            leftPage.openNginxBaseEditor(nginxman.nginxBaseConfigText)
                        } else {
                            notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                }
                Controls.Button {
                    text: "View Error Log"
                    icon.name: "document-preview"
                    onClicked: {
                        if (nginxman.loadNginxErrorLog()) {
                            errorLogDialog.open()
                        } else {
                            notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                }
            }
            // NGINX error-log viewer: last 500 lines of
            // /var/log/nginx/error.log (privileged tail — pkexec/sudo
            // prompt on first open, Refresh re-reads).
            Controls.Dialog {
                id: errorLogDialog
                title: "NGINX Error Log (/var/log/nginx/error.log)"
                modal: true
                anchors.centerIn: parent
                width: Math.min(parent.width - 80, 720)
                height: Math.min(parent.height - 80, 520)
                ColumnLayout {
                    width: parent.width
                    height: parent.height
                    spacing: Kirigami.Units.smallSpacing
                    Controls.Label {
                        text: "Last 500 lines — use Refresh after reproducing an error."
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                        opacity: 0.7
                        font.pointSize: 8
                    }
                    Controls.ScrollView {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        Controls.ScrollBar.vertical.policy: Controls.ScrollBar.AlwaysOn
                        Controls.TextArea {
                            text: nginxman.nginxErrorLog
                            readOnly: true
                            font.family: "monospace"
                            font.pointSize: 8
                            wrapMode: TextEdit.NoWrap
                            selectByMouse: true
                        }
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Kirigami.Units.smallSpacing
                        Controls.Button {
                            text: "Refresh"
                            icon.name: "view-refresh"
                            onClicked: {
                                if (!nginxman.loadNginxErrorLog())
                                    notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                            }
                        }
                        Item { Layout.fillWidth: true }
                        Controls.Button {
                            text: "Close"
                            icon.name: "dialog-cancel"
                            onClicked: errorLogDialog.close()
                        }
                    }
                }
            }
            Controls.Label {
                text: "To delete a backup, simply click the Delete icon next to the backup file in the Restore Backup List."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.6
                font.pointSize: 8
            }
            Controls.Label {
                text: "Restore backup:"
                opacity: 0.7
            }
            ListView {
                id: nginxBackupView
                Layout.fillWidth: true
                Layout.preferredHeight: Math.max(44, Math.min(148, contentHeight))
                clip: true
                // Persistent bar (same as the page): a capped list with many
                // backups must always show it can scroll.
                Controls.ScrollBar.vertical: Controls.ScrollBar { policy: Controls.ScrollBar.AlwaysOn }
                flickableDirection: Flickable.VerticalFlick
                // Most recent backup first (backend sorts by mtime desc).
                model: nginxman.nginxBaseBackups
                delegate: RowLayout {
                    // Clear of the overlaid AlwaysOn scrollbar.
                    width: nginxBackupView.width - 20
                    spacing: Kirigami.Units.smallSpacing
                    Controls.Label {
                        text: modelData
                        Layout.fillWidth: true
                        elide: Text.ElideRight
                        font.family: "monospace"
                        font.pointSize: 8
                    }
                    Controls.Button {
                        text: "Restore"
                        icon.name: "document-revert"
                        onClicked: {
                            var ok = nginxman.restoreNginxBackup(modelData)
                            nginxman.refreshStatus()
                            nginxman.refreshBaseStatus()
                            if (ok)
                                nginxPage.notifyLeft("NGINX base config restored from " + modelData, 4000)
                            else
                                nginxPage.notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                    Controls.ToolButton {
                        icon.name: "edit-delete"
                        Accessible.name: "Delete " + modelData
                        onClicked: {
                            var ok = nginxman.deleteNginxBackup(modelData)
                            nginxman.refreshBaseStatus()
                            if (!ok)
                                nginxPage.notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                }
            }
            Controls.Label {
                text: "No NGINX backups yet — use Backup NGINX first."
                visible: nginxBackupView.count === 0
                opacity: 0.5
                font.pointSize: 8
            }
            // PHP-FPM base tool line
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Label { text: "PHP-FPM BASE CONFIG(www.conf):"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: nginxman.phpfpmBaseBackedUp ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Label {
                    text: nginxman.phpfpmBaseBackupStatus
                    font.bold: true
                    color: nginxman.phpfpmBaseBackedUp ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Item { Layout.fillWidth: true }
                Controls.Button {
                    text: "Backup PHP-FPM"
                    icon.name: "document-save"
                    onClicked: {
                        var ok = nginxman.backupPhpfpmBase()
                        nginxman.refreshStatus()
                        nginxman.refreshBaseStatus()
                        if (!ok)
                            nginxPage.notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                    }
                }
                Controls.Button {
                    text: "Edit PHP-FPM"
                    icon.name: "document-edit"
                    onClicked: {
                        if (nginxman.loadPhpfpmBaseConfig()) {
                            leftPage.openPhpfpmBaseEditor(nginxman.phpfpmBaseConfigText)
                        } else {
                            notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                }
                Controls.Button {
                    text: "View Error Log"
                    icon.name: "document-preview"
                    onClicked: {
                        if (nginxman.loadPhpfpmErrorLog()) {
                            phpfpmErrorLogDialog.open()
                        } else {
                            notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                }
            }
            // PHP-FPM error-log viewer: last 500 lines of the resolved
            // php-fpm.conf `error_log` path (privileged tail — pkexec/sudo
            // prompt on first open, Refresh re-reads).
            Controls.Dialog {
                id: phpfpmErrorLogDialog
                title: "PHP-FPM Error Log (" + nginxman.phpfpmErrorLogPath + ")"
                modal: true
                anchors.centerIn: parent
                width: Math.min(parent.width - 80, 720)
                height: Math.min(parent.height - 80, 520)
                ColumnLayout {
                    width: parent.width
                    height: parent.height
                    spacing: Kirigami.Units.smallSpacing
                    Controls.Label {
                        text: "Last 500 lines — use Refresh after reproducing an error."
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                        opacity: 0.7
                        font.pointSize: 8
                    }
                    Controls.ScrollView {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        Controls.ScrollBar.vertical.policy: Controls.ScrollBar.AlwaysOn
                        Controls.TextArea {
                            text: nginxman.phpfpmErrorLog
                            readOnly: true
                            font.family: "monospace"
                            font.pointSize: 8
                            wrapMode: TextEdit.NoWrap
                            selectByMouse: true
                        }
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Kirigami.Units.smallSpacing
                        Controls.Button {
                            text: "Refresh"
                            icon.name: "view-refresh"
                            onClicked: {
                                if (!nginxman.loadPhpfpmErrorLog())
                                    notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                            }
                        }
                        Item { Layout.fillWidth: true }
                        Controls.Button {
                            text: "Close"
                            icon.name: "dialog-cancel"
                            onClicked: phpfpmErrorLogDialog.close()
                        }
                    }
                }
            }
            Controls.Label {
                text: "Restore backup:"
                opacity: 0.7
            }
            ListView {
                id: phpfpmBackupView
                Layout.fillWidth: true
                Layout.preferredHeight: Math.max(44, Math.min(148, contentHeight))
                clip: true
                Controls.ScrollBar.vertical: Controls.ScrollBar { policy: Controls.ScrollBar.AlwaysOn }
                flickableDirection: Flickable.VerticalFlick
                model: nginxman.phpfpmBaseBackups
                delegate: RowLayout {
                    // Clear of the overlaid AlwaysOn scrollbar.
                    width: phpfpmBackupView.width - 20
                    spacing: Kirigami.Units.smallSpacing
                    Controls.Label {
                        text: modelData
                        Layout.fillWidth: true
                        elide: Text.ElideRight
                        font.family: "monospace"
                        font.pointSize: 8
                    }
                    Controls.Button {
                        text: "Restore"
                        icon.name: "document-revert"
                        onClicked: {
                            var ok = nginxman.restorePhpfpmBackup(modelData)
                            nginxman.refreshStatus()
                            nginxman.refreshBaseStatus()
                            if (ok)
                                nginxPage.notifyLeft("PHP-FPM base config restored from " + modelData, 4000)
                            else
                                nginxPage.notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                    Controls.ToolButton {
                        icon.name: "edit-delete"
                        Accessible.name: "Delete " + modelData
                        onClicked: {
                            var ok = nginxman.deletePhpfpmBackup(modelData)
                            nginxman.refreshBaseStatus()
                            if (!ok)
                                nginxPage.notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                }
            }
            Controls.Label {
                text: "No PHP-FPM backups yet — use Backup PHP-FPM first."
                visible: phpfpmBackupView.count === 0
                opacity: 0.5
                font.pointSize: 8
            }
            Controls.Label {
                text: "Backups: ~/.local/backups/nginx and ~/.local/backups/php-fpm (most recent first). Restore overwrites /etc/nginx/nginx.conf or the distro www.conf, then tests + reloads."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.5
                font.pointSize: 8
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Nginx Site Configs ----
            Kirigami.Heading { text: "Nginx Site Configs"; level: 3 }
            Controls.Label {
                text: "You can create simple nginx web servers that correlate to your [DEV HTTPS Certs] you created here. Note Adding a site does not automatically restart Nginx, please use provided buttons above."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
            }
            ListView {
                id: siteView
                Layout.fillWidth: true
                Layout.preferredHeight: Math.max(60, contentHeight)
                clip: true
                interactive: false
                model: nginxman.siteDomains
                delegate: RowLayout {
                    width: siteView.width
                    spacing: Kirigami.Units.smallSpacing
                    Controls.Label { text: modelData; font.bold: true; Layout.fillWidth: true; elide: Text.ElideRight }
                    Controls.Button {
                        text: "Quick Create"
                        icon.name: "document-new"
                        enabled: Array.from(nginxman.configuredSites).indexOf(modelData) === -1
                        onClicked: {
                            nginxman.quickCreate(modelData)
                            nginxman.refreshStatus()
                        }
                    }
                    Controls.Button {
                        text: "Edit Site Config"
                        icon.name: "document-edit"
                        enabled: Array.from(nginxman.configuredSites).indexOf(modelData) !== -1
                        onClicked: {
                            if (nginxman.loadSiteConfig(modelData)) {
                                leftPage.openSiteEditor(modelData, nginxman.siteConfigText)
                            } else {
                                notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                            }
                        }
                    }
                    Controls.Button {
                        text: "Delete Site"
                        icon.name: "edit-delete"
                        enabled: Array.from(nginxman.configuredSites).indexOf(modelData) !== -1
                        onClicked: {
                            nginxman.deleteSite(modelData)
                            nginxman.refreshStatus()
                        }
                    }
                    Controls.Button {
                        text: "Backup"
                        icon.name: "document-save"
                        enabled: Array.from(nginxman.configuredSites).indexOf(modelData) !== -1
                        onClicked: {
                            var ok = nginxman.backupSiteConfig(modelData)
                            if (ok)
                                nginxPage.notifyLeft("Site " + modelData + " backed up", 4000)
                            else
                                nginxPage.notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                    Controls.Button {
                        text: "Restore"
                        icon.name: "document-revert"
                        enabled: Array.from(nginxman.configuredSites).indexOf(modelData) !== -1
                        onClicked: {
                            nginxman.refreshSiteBackups(modelData)
                            siteRestoreDialog.open()
                        }
                    }
                    Controls.Button {
                        text: "Error Log"
                        icon.name: "document-preview"
                        // Dimmed when the site file is missing/unreadable or
                        // carries no error_log line (customised configs).
                        enabled: Array.from(nginxman.configuredSites).indexOf(modelData) !== -1 && nginxman.siteHasSiteLog(modelData, "error_log")
                        onClicked: {
                            siteLogDomain = modelData
                            siteLogKind = "error_log"
                            if (nginxman.loadSiteLog(modelData, "error_log")) {
                                siteLogDialog.open()
                            } else {
                                notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                            }
                        }
                    }
                    Controls.Button {
                        text: "Access Log"
                        icon.name: "view-list-text"
                        // Dimmed when the site file is missing/unreadable or
                        // carries no access_log line (customised configs).
                        enabled: Array.from(nginxman.configuredSites).indexOf(modelData) !== -1 && nginxman.siteHasSiteLog(modelData, "access_log")
                        onClicked: {
                            siteLogDomain = modelData
                            siteLogKind = "access_log"
                            if (nginxman.loadSiteLog(modelData, "access_log")) {
                                siteLogDialog.open()
                            } else {
                                notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                            }
                        }
                    }
                }
            }
            // Per-site restore dialog: backup list (most recent first)
            // from ~/.local/backups/sites/<domain>, with per-file Restore
            // buttons and Delete icons.
            Controls.Dialog {
                id: siteRestoreDialog
                title: "Restore " + nginxman.siteBackupDomain + ".conf"
                modal: true
                anchors.centerIn: parent
                width: Math.min(parent.width - 80, 560)
                ColumnLayout {
                    width: parent.width
                    spacing: Kirigami.Units.smallSpacing
                    Controls.Label {
                        text: "Backups for " + nginxman.siteBackupDomain + " (most recent first). Restoring overwrites the site file, then tests + reloads."
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                        opacity: 0.7
                    }
                    Controls.Label {
                        text: "To delete a backup, simply click the Delete icon next to the backup file in the Restore Backup List."
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                        opacity: 0.6
                        font.pointSize: 8
                    }
                    ListView {
                        id: siteBackupView
                        Layout.fillWidth: true
                        Layout.preferredHeight: Math.max(44, Math.min(180, contentHeight))
                        clip: true
                        // Persistent bar: the dialog is fixed-size, so a long
                        // backup history must always show it can scroll.
                        Controls.ScrollBar.vertical: Controls.ScrollBar { policy: Controls.ScrollBar.AlwaysOn }
                        flickableDirection: Flickable.VerticalFlick
                        model: nginxman.siteBackups
                        delegate: RowLayout {
                            // Clear of the overlaid AlwaysOn scrollbar.
                            width: siteBackupView.width - 20
                            spacing: Kirigami.Units.smallSpacing
                            Controls.Label {
                                text: modelData
                                Layout.fillWidth: true
                                elide: Text.ElideRight
                                font.family: "monospace"
                                font.pointSize: 8
                            }
                            Controls.Button {
                                text: "Restore"
                                icon.name: "document-revert"
                                onClicked: {
                                    var ok = nginxman.restoreSiteBackup(nginxman.siteBackupDomain, modelData)
                                    nginxman.refreshStatus()
                                    if (ok) {
                                        nginxPage.notifyLeft("Site " + nginxman.siteBackupDomain + " restored from " + modelData, 4000)
                                        siteRestoreDialog.close()
                                    } else {
                                        nginxPage.notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                                    }
                                }
                            }
                            Controls.ToolButton {
                                icon.name: "edit-delete"
                                Accessible.name: "Delete " + modelData
                                onClicked: {
                                    var ok = nginxman.deleteSiteBackup(nginxman.siteBackupDomain, modelData)
                                    if (!ok)
                                        nginxPage.notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                                }
                            }
                        }
                    }
                    Controls.Label {
                        text: "No backups yet — use Backup first."
                        visible: siteBackupView.count === 0
                        opacity: 0.5
                        font.pointSize: 8
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        Item { Layout.fillWidth: true }
                        Controls.Button {
                            text: "Close"
                            icon.name: "dialog-cancel"
                            onClicked: siteRestoreDialog.close()
                        }
                    }
                }
            }
            // Per-site log viewer: paths come from the site file's own
            // error_log / access_log lines (plain + ssl joined under
            // `==> path <==` headers). Refresh re-reads the stored site.
            Controls.Dialog {
                id: siteLogDialog
                title: nginxman.siteLogTitle !== "" ? nginxman.siteLogTitle : "Site Log"
                modal: true
                anchors.centerIn: parent
                width: Math.min(parent.width - 80, 720)
                height: Math.min(parent.height - 80, 520)
                ColumnLayout {
                    width: parent.width
                    height: parent.height
                    spacing: Kirigami.Units.smallSpacing
                    Controls.Label {
                        text: "Last 500 lines per file — use Refresh after reproducing an error."
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                        opacity: 0.7
                        font.pointSize: 8
                    }
                    Controls.ScrollView {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        Controls.ScrollBar.vertical.policy: Controls.ScrollBar.AlwaysOn
                        Controls.TextArea {
                            text: nginxman.siteLogText
                            readOnly: true
                            font.family: "monospace"
                            font.pointSize: 8
                            wrapMode: TextEdit.NoWrap
                            selectByMouse: true
                        }
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Kirigami.Units.smallSpacing
                        Controls.Button {
                            text: "Refresh"
                            icon.name: "view-refresh"
                            onClicked: {
                                if (!nginxman.loadSiteLog(siteLogDomain, siteLogKind))
                                    notifyLeft(nginxman.statusMessage, 8000, Kirigami.MessageType.Error)
                            }
                        }
                        Item { Layout.fillWidth: true }
                        Controls.Button {
                            text: "Close"
                            icon.name: "dialog-cancel"
                            onClicked: siteLogDialog.close()
                        }
                    }
                }
            }
            Controls.Label {
                text: "No domain certificates yet — generate one on page 3 first."
                visible: nginxman.siteDomains.length === 0
                opacity: 0.5
                font.pointSize: 8
            }
            Controls.Label {
                text: "Site backups live in ~/.local/backups/sites/<domain> (most recent first). Restore overwrites the site file, then tests + reloads."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.5
                font.pointSize: 8
            }

        }
    }
}
