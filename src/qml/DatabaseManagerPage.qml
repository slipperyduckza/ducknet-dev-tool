import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.ducknetdevtool

Kirigami.Page {
    id: databaseManagerPage
    title: "Database Manager"
    property var leftPage: null

    function notifyLeft(msg, duration, type) {
        if (leftPage && leftPage.showLeftNotification)
            leftPage.showLeftNotification(msg, duration, type)
    }

    DatabaseManager {
        id: dbman
    }

    // The Enable button gate (installed but not enabled, Fedora-family
    // only) is computed in Rust as dbman.mariadbEnableVisible, so the
    // rule lives in exactly one place — see mariadb_enable_visible().

    Component.onCompleted: {
        // Paint last-known lists instantly (silent); live data follows
        // via Refresh Lists or any create/delete.
        dbman.loadCachedDbLists()
        dbman.refreshStatus()
    }

    // Restarts block on pkexec/systemctl, so sequence the two popups:
    // "requested" paints first, the restart runs, then the result replaces it.
    Timer {
        id: postgresRestartTimer
        interval: 400
        repeat: false
        onTriggered: {
            let ok = dbman.restartPostgres()
            dbman.refreshStatus()
            if (ok)
                databaseManagerPage.notifyLeft("PostgreSQL restarted", 4000)
            else
                databaseManagerPage.notifyLeft("Restart failed: " + dbman.statusMessage, 8000, Kirigami.MessageType.Error)
        }
    }
    Timer {
        id: mariadbRestartTimer
        interval: 400
        repeat: false
        onTriggered: {
            let ok = dbman.restartMariadb()
            dbman.refreshStatus()
            if (ok)
                databaseManagerPage.notifyLeft("MariaDB restarted", 4000)
            else
                databaseManagerPage.notifyLeft("Restart failed: " + dbman.statusMessage, 8000, Kirigami.MessageType.Error)
        }
    }

    Controls.ScrollView {
        id: dbScroll
        anchors.fill: parent
        contentWidth: availableWidth
        Controls.ScrollBar.vertical.policy: Controls.ScrollBar.AlwaysOn
        Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff

        ColumnLayout {
            // -24 keeps content clear of the overlaid AlwaysOn scrollbar.
            width: dbScroll.availableWidth - 24
            anchors.margins: Kirigami.Units.largeSpacing
            spacing: Kirigami.Units.smallSpacing

            Kirigami.Heading { text: "Developers Handy Database Manager"; level: 2; wrapMode: Text.WordWrap; Layout.fillWidth: true }
            Controls.Label {
                text: "This section provides the developer with the option to Install Postgresql or Mariadb for use with their WebApp development efforts"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Install your Database ----
            Kirigami.Heading { text: "Install your Database"; level: 3 }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.largeSpacing
                // PostgreSQL group: LED + install button.
                RowLayout {
                    spacing: Kirigami.Units.smallSpacing
                    Rectangle {
                        Layout.preferredWidth: 14
                        Layout.preferredHeight: 14
                        radius: 7
                        color: dbman.postgresInstalled ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                    }
                    Controls.Button {
                        text: "Install PostgreSQL"
                        icon.name: "system-software-install"
                        enabled: !dbman.postgresInstalled && !dbman.opActive
                        onClicked: {
                            if (dbman.startDbOp("postgres")) {
                                opDialog.open()
                            } else {
                                databaseManagerPage.notifyLeft(dbman.statusMessage, 8000, Kirigami.MessageType.Error)
                            }
                        }
                    }
                }
                // MariaDB group: LED + install button (+ Enable when
                // pre-installed-but-disabled on Fedora — same row as the
                // greyed-out Install button, so it can't be missed).
                RowLayout {
                    spacing: Kirigami.Units.smallSpacing
                    Rectangle {
                        Layout.preferredWidth: 14
                        Layout.preferredHeight: 14
                        radius: 7
                        color: dbman.mariadbInstalled ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                    }
                    Controls.Button {
                        text: "Install MariaDB"
                        icon.name: "system-software-install"
                        enabled: !dbman.mariadbInstalled && !dbman.opActive
                        onClicked: {
                            if (dbman.startDbOp("mariadb")) {
                                opDialog.open()
                            } else {
                                databaseManagerPage.notifyLeft(dbman.statusMessage, 8000, Kirigami.MessageType.Error)
                            }
                        }
                    }
                    Controls.Button {
                        text: "Enable MariaDB"
                        icon.name: "dialog-ok-apply"
                        // Accent-highlighted: this is the recommended action
                        // whenever it is visible (pre-installed Fedora case).
                        highlighted: true
                        visible: dbman.mariadbEnableVisible
                        onClicked: {
                            dbman.enableMariadb()
                            dbman.refreshStatus()
                        }
                    }
                }
            }
            Controls.Label {
                text: "MariaDB is already on this Fedora system (e.g. pre-installed via Akonadi) but is not set to start on boot — Enable adopts it into your dev environment (enables + starts) instead of reinstalling."
                visible: dbman.mariadbEnableVisible
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.6
                font.pointSize: 8
            }
            Controls.Label {
                text: "PostgreSQL: " + dbman.postgresStatusText
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
                font.pointSize: 8
                color: dbman.postgresInstalled ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
            }
            Controls.Label {
                text: "MariaDB: " + dbman.mariadbStatusText
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
                font.pointSize: 8
                color: dbman.mariadbInstalled ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
            }
            Controls.Label {
                text: "Installs run in the background with the log below (needs root via pkexec / passwordless sudo). Debian: postgresql / mariadb-server from distro repos. Fedora: postgresql-server (+ initdb) / mariadb-server from distro repos; services are enabled and started."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.6
                font.pointSize: 8
            }
            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Database Services (same layout as Nginx Manager) ----
            Kirigami.Heading { text: "Database Services"; level: 3 }
            // PostgreSQL status bar + actions.
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.largeSpacing
                Controls.Label { text: "POSTGRESQL STATUS:"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: dbman.postgresRunning ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Label {
                    text: dbman.postgresStatusText
                    font.bold: true
                    color: dbman.postgresRunning ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Button {
                    text: "Start PostgreSQL"
                    icon.name: "media-playback-start"
                    enabled: dbman.postgresInstalled && !dbman.postgresRunning
                    onClicked: {
                        dbman.startPostgres()
                        dbman.refreshStatus()
                    }
                }
                Controls.Button {
                    text: "Stop PostgreSQL"
                    icon.name: "media-playback-stop"
                    enabled: dbman.postgresRunning
                    onClicked: {
                        dbman.stopPostgres()
                        dbman.refreshStatus()
                    }
                }
                Controls.Button {
                    text: "Restart PostgreSQL"
                    icon.name: "view-refresh"
                    enabled: dbman.postgresRunning
                    onClicked: {
                        databaseManagerPage.notifyLeft("PostgreSQL restart requested", 2500)
                        postgresRestartTimer.restart()
                    }
                }
                Item { Layout.fillWidth: true }
                Controls.Button {
                    text: "Refresh"
                    icon.name: "view-refresh"
                    onClicked: dbman.refreshStatus()
                }
            }
            // MariaDB status bar + actions.
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.largeSpacing
                Controls.Label { text: "MARIADB STATUS:"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: dbman.mariadbRunning ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Label {
                    text: dbman.mariadbStatusText
                    font.bold: true
                    color: dbman.mariadbRunning ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Button {
                    text: "Start MariaDB"
                    icon.name: "media-playback-start"
                    enabled: dbman.mariadbInstalled && !dbman.mariadbRunning
                    onClicked: {
                        dbman.startMariadb()
                        dbman.refreshStatus()
                    }
                }
                Controls.Button {
                    text: "Stop MariaDB"
                    icon.name: "media-playback-stop"
                    enabled: dbman.mariadbRunning
                    onClicked: {
                        dbman.stopMariadb()
                        dbman.refreshStatus()
                    }
                }
                Controls.Button {
                    text: "Restart MariaDB"
                    icon.name: "view-refresh"
                    enabled: dbman.mariadbRunning
                    onClicked: {
                        databaseManagerPage.notifyLeft("MariaDB restart requested", 2500)
                        mariadbRestartTimer.restart()
                    }
                }
                Item { Layout.fillWidth: true }
                Controls.Button {
                    text: "Refresh"
                    icon.name: "view-refresh"
                    onClicked: dbman.refreshStatus()
                }
            }
            Controls.Label {
                text: "Start / Stop / Restart run via pkexec (or passwordless sudo)."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.5
                font.pointSize: 8
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Button {
                    text: "Create Database"
                    icon.name: "document-new"
                    enabled: (dbman.postgresInstalled || dbman.mariadbInstalled) && !dbman.opActive
                    onClicked: leftPage.openCreateDatabase()
                }
            }            Controls.Label {
                text: "Install PostgreSQL or MariaDB above to create databases."
                visible: !dbman.postgresInstalled && !dbman.mariadbInstalled
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.6
                font.pointSize: 8
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Current Databases ----
            Kirigami.Heading { text: "Current Databases"; level: 3 }
            Controls.Label {
                text: "Current Databases are listed below:"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Item { Layout.fillWidth: true }
                Controls.Button {
                    text: "Refresh Lists"
                    icon.name: "view-refresh"
                    onClicked: {
                        if (!dbman.refreshDbLists())
                            databaseManagerPage.notifyLeft(dbman.statusMessage, 8000, Kirigami.MessageType.Error)
                    }
                }
            }
            // PostgreSQL databases (only when installed).
            Controls.Label {
                text: "PostgreSQL databases:"
                visible: dbman.postgresInstalled
                font.bold: true
                opacity: 0.7
            }
            ListView {
                id: postgresDbView
                visible: dbman.postgresInstalled
                Layout.fillWidth: true
                Layout.preferredHeight: Math.max(44, Math.min(148, contentHeight))
                clip: true
                Controls.ScrollBar.vertical: Controls.ScrollBar { policy: Controls.ScrollBar.AlwaysOn }
                flickableDirection: Flickable.VerticalFlick
                model: dbman.postgresDatabases
                delegate: RowLayout {
                    width: postgresDbView.width - 20
                    spacing: Kirigami.Units.smallSpacing
                    Controls.Label {
                        text: modelData
                        elide: Text.ElideRight
                        font.family: "monospace"
                        font.pointSize: 8
                        Layout.fillWidth: true
                    }
                    Controls.Button {
                        text: "Backup DB"
                        icon.name: "document-save"
                        onClicked: {
                            var ok = dbman.backupDatabase("postgres", modelData)
                            databaseManagerPage.notifyLeft(dbman.statusMessage, ok ? 4000 : 8000,
                                ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                        }
                    }
                    Controls.Button {
                        text: "Restore DB"
                        icon.name: "document-revert"
                        onClicked: {
                            restoreDialog.dbKind = "postgres"
                            restoreDialog.dbName = modelData
                            dbman.loadDbBackups("postgres", modelData)
                            restoreDialog.open()
                        }
                    }
                    Controls.Button {
                        text: "Change Owner Password"
                        icon.name: "dialog-password"
                        onClicked: {
                            passwordDialog.dbKind = "postgres"
                            passwordDialog.dbName = modelData
                            passwordDialog.open()
                        }
                    }
                    Controls.Button {
                        text: "Delete"
                        icon.name: "edit-delete"
                        onClicked: {
                            deleteDialog.dbKind = "postgres"
                            deleteDialog.dbName = modelData
                            deleteDialog.stage = "confirm"
                            deleteDialog.open()
                        }
                    }
                }
            }
            Controls.Label {
                text: "No PostgreSQL databases yet — use Create Database."
                visible: dbman.postgresInstalled && postgresDbView.count === 0
                opacity: 0.5
                font.pointSize: 8
            }
            // MariaDB databases (only when installed).
            Controls.Label {
                text: "MariaDB databases:"
                visible: dbman.mariadbInstalled
                font.bold: true
                opacity: 0.7
            }
            ListView {
                id: mariadbDbView
                visible: dbman.mariadbInstalled
                Layout.fillWidth: true
                Layout.preferredHeight: Math.max(44, Math.min(148, contentHeight))
                clip: true
                Controls.ScrollBar.vertical: Controls.ScrollBar { policy: Controls.ScrollBar.AlwaysOn }
                flickableDirection: Flickable.VerticalFlick
                model: dbman.mariadbDatabases
                delegate: RowLayout {
                    width: mariadbDbView.width - 20
                    spacing: Kirigami.Units.smallSpacing
                    Controls.Label {
                        text: modelData
                        elide: Text.ElideRight
                        font.family: "monospace"
                        font.pointSize: 8
                        Layout.fillWidth: true
                    }
                    Controls.Button {
                        text: "Backup DB"
                        icon.name: "document-save"
                        onClicked: {
                            var ok = dbman.backupDatabase("mariadb", modelData)
                            databaseManagerPage.notifyLeft(dbman.statusMessage, ok ? 4000 : 8000,
                                ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                        }
                    }
                    Controls.Button {
                        text: "Restore DB"
                        icon.name: "document-revert"
                        onClicked: {
                            restoreDialog.dbKind = "mariadb"
                            restoreDialog.dbName = modelData
                            dbman.loadDbBackups("mariadb", modelData)
                            restoreDialog.open()
                        }
                    }
                    Controls.Button {
                        text: "Change Owner Password"
                        icon.name: "dialog-password"
                        onClicked: {
                            passwordDialog.dbKind = "mariadb"
                            passwordDialog.dbName = modelData
                            passwordDialog.open()
                        }
                    }
                    Controls.Button {
                        text: "Delete"
                        icon.name: "edit-delete"
                        onClicked: {
                            deleteDialog.dbKind = "mariadb"
                            deleteDialog.dbName = modelData
                            deleteDialog.stage = "confirm"
                            deleteDialog.open()
                        }
                    }
                }
            }
            Controls.Label {
                text: "No MariaDB databases yet — use Create Database."
                visible: dbman.mariadbInstalled && mariadbDbView.count === 0
                opacity: 0.5
                font.pointSize: 8
            }
            Controls.Label {
                text: "Install PostgreSQL or MariaDB above to list databases."
                visible: !dbman.postgresInstalled && !dbman.mariadbInstalled
                opacity: 0.5
                font.pointSize: 8
            }
        }
    }

    // Restore dialog: newest-first dump list for one database, each row
    // with Restore (replays into the database, recreating it when missing)
    // and Delete (removes the dump file). Backups live in
    // ~/BACKUPDB/<engine>/<db>/.
    Controls.Dialog {
        id: restoreDialog
        property string dbKind: ""
        property string dbName: ""
        title: "Restore " + dbName
        modal: true
        width: Math.min(560, databaseManagerPage.width - 40)
        height: Math.min(420, databaseManagerPage.height - 40)
        x: (databaseManagerPage.width - width) / 2
        y: (databaseManagerPage.height - height) / 2
        standardButtons: Controls.Dialog.NoButton
        contentItem: ColumnLayout {
            spacing: Kirigami.Units.smallSpacing
            Controls.Label {
                text: "Backups for \"" + restoreDialog.dbName + "\" (newest first):"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                font.bold: true
            }
            ListView {
                id: restoreDbView
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                Controls.ScrollBar.vertical: Controls.ScrollBar { policy: Controls.ScrollBar.AlwaysOn }
                model: dbman.dbBackupFiles
                delegate: RowLayout {
                    width: restoreDbView.width - 20
                    spacing: Kirigami.Units.smallSpacing
                    Controls.Label {
                        text: modelData
                        elide: Text.ElideRight
                        font.family: "monospace"
                        font.pointSize: 8
                        Layout.fillWidth: true
                    }
                    Controls.Button {
                        text: "Restore"
                        icon.name: "document-revert"
                        onClicked: {
                            var ok = dbman.restoreDbBackup(restoreDialog.dbKind, restoreDialog.dbName, modelData)
                            restoreDialog.close()
                            databaseManagerPage.notifyLeft(dbman.statusMessage, ok ? 4000 : 8000,
                                ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                        }
                    }
                    Controls.Button {
                        text: "Delete"
                        icon.name: "edit-delete"
                        onClicked: {
                            var ok = dbman.deleteDbBackup(restoreDialog.dbKind, restoreDialog.dbName, modelData)
                            if (!ok)
                                databaseManagerPage.notifyLeft(dbman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                }
            }
            Controls.Label {
                visible: restoreDbView.count === 0
                text: "No backups yet — use Backup DB first."
                opacity: 0.5
                font.pointSize: 8
            }
        }
        footer: RowLayout {
            spacing: Kirigami.Units.smallSpacing
            Item { Layout.fillWidth: true }
            Controls.Button {
                text: "Close"
                icon.name: "dialog-close"
                onClicked: restoreDialog.close()
            }
        }
    }

    // Password dialog: two entries must match (each with a reveal toggle
    // for checking). Applies to the catalog owner (PostgreSQL) or the
    // grant-holding user (MariaDB), resolved by the backend.
    Controls.Dialog {
        id: passwordDialog
        property string dbKind: ""
        property string dbName: ""
        title: "Change Owner Password"
        modal: true
        width: Math.min(480, databaseManagerPage.width - 40)
        height: Math.min(360, databaseManagerPage.height - 40)
        x: (databaseManagerPage.width - width) / 2
        y: (databaseManagerPage.height - height) / 2
        standardButtons: Controls.Dialog.NoButton
        onClosed: {
            passwordField.clear()
            passwordConfirmField.clear()
        }
        contentItem: ColumnLayout {
            spacing: Kirigami.Units.smallSpacing
            Controls.Label {
                text: "Set a new password for the owner of \"" + passwordDialog.dbName + "\". Enter it twice below."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                font.bold: true
            }
            Controls.Label { text: "New Password"; font.bold: true }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.TextField {
                    id: passwordField
                    Layout.fillWidth: true
                    echoMode: revealPassword.checked ? TextInput.Normal : TextInput.Password
                    placeholderText: "New password"
                }
                Controls.ToolButton {
                    id: revealPassword
                    checkable: true
                    icon.name: checked ? "view-hidden" : "view-visible"
                    Accessible.name: checked ? "Hide password" : "Reveal password to check it"
                }
            }
            Controls.Label { text: "Confirm Password"; font.bold: true }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.TextField {
                    id: passwordConfirmField
                    Layout.fillWidth: true
                    echoMode: revealPasswordConfirm.checked ? TextInput.Normal : TextInput.Password
                    placeholderText: "Repeat new password"
                }
                Controls.ToolButton {
                    id: revealPasswordConfirm
                    checkable: true
                    icon.name: checked ? "view-hidden" : "view-visible"
                    Accessible.name: checked ? "Hide password" : "Reveal password to check it"
                }
            }
        }
        footer: RowLayout {
            spacing: Kirigami.Units.smallSpacing
            Item { Layout.fillWidth: true }
            Controls.Button {
                text: "Change"
                icon.name: "dialog-password"
                onClicked: {
                    if (passwordField.text === "" || passwordField.text !== passwordConfirmField.text) {
                        databaseManagerPage.notifyLeft("Passwords do not match.", 8000, Kirigami.MessageType.Error)
                    } else {
                        var ok = dbman.changeDbPassword(passwordDialog.dbKind, passwordDialog.dbName, passwordField.text)
                        passwordDialog.close()
                        databaseManagerPage.notifyLeft(dbman.statusMessage, ok ? 4000 : 8000,
                            ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                    }
                }
            }
            Controls.Button {
                text: "Cancel"
                icon.name: "dialog-cancel"
                onClicked: passwordDialog.close()
            }
        }
    }

    // Delete confirmation: stage "confirm" warns, stage "verify" demands
    // the name typed back. A wrong name cancels with "Database delete not
    // accepted." on the left pane — the backend never runs unconfirmed.
    Controls.Dialog {
        id: deleteDialog
        property string dbKind: ""
        property string dbName: ""
        property string stage: "confirm"
        title: "Delete Database"
        modal: true
        width: Math.min(480, databaseManagerPage.width - 40)
        height: Math.min(320, databaseManagerPage.height - 40)
        x: (databaseManagerPage.width - width) / 2
        y: (databaseManagerPage.height - height) / 2
        standardButtons: Controls.Dialog.NoButton
        onClosed: stage = "confirm"
        contentItem: ColumnLayout {
            spacing: Kirigami.Units.smallSpacing
            Controls.Label {
                visible: deleteDialog.stage === "confirm"
                text: "WARNING: DELETING A DATABASE IS PERMANENT."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                font.bold: true
                color: Kirigami.Theme.negativeTextColor
            }
            Controls.Label {
                visible: deleteDialog.stage === "confirm"
                text: "Are you sure you want to Delete the database \"" + deleteDialog.dbName + "\"?"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }
            Controls.Label {
                visible: deleteDialog.stage === "verify"
                text: "Please Enter Database name to confirm you would like to Delete the database " + deleteDialog.dbName
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                font.bold: true
            }
            Controls.TextField {
                id: deleteConfirmField
                visible: deleteDialog.stage === "verify"
                Layout.fillWidth: true
                placeholderText: deleteDialog.dbName
            }
        }
        footer: RowLayout {
            spacing: Kirigami.Units.smallSpacing
            Item { Layout.fillWidth: true }
            Controls.Button {
                visible: deleteDialog.stage === "confirm"
                text: "Yes Delete"
                icon.name: "edit-delete"
                onClicked: {
                    deleteConfirmField.clear()
                    deleteDialog.stage = "verify"
                    deleteConfirmField.forceActiveFocus()
                }
            }
            Controls.Button {
                visible: deleteDialog.stage === "verify"
                text: "Delete"
                icon.name: "edit-delete"
                onClicked: {
                    if (deleteConfirmField.text.trim() !== deleteDialog.dbName) {
                        deleteDialog.close()
                        databaseManagerPage.notifyLeft("Database delete not accepted.", 8000, Kirigami.MessageType.Error)
                    } else {
                        var ok = dbman.deleteDatabase(deleteDialog.dbKind, deleteDialog.dbName)
                        deleteDialog.close()
                        databaseManagerPage.notifyLeft(dbman.statusMessage, ok ? 4000 : 8000,
                            ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                    }
                }
            }
            Controls.Button {
                text: "Cancel"
                icon.name: "dialog-cancel"
                onClicked: deleteDialog.close()
            }
        }
    }

    // Install progress dialog + poller. The op runs for minutes in the
    // background; the timer streams the log and auto-closes on completion.
    // No cancel: killing apt/dnf mid-transaction risks a dpkg/rpm lock mess.
    Controls.Dialog {
        id: opDialog
        title: dbman.opName
        modal: true
        width: Math.min(720, databaseManagerPage.width - 40)
        height: Math.min(480, databaseManagerPage.height - 40)
        x: (databaseManagerPage.width - width) / 2
        y: (databaseManagerPage.height - height) / 2
        closePolicy: dbman.opActive ? Controls.Popup.NoAutoClose : Controls.Popup.CloseOnEscape
        contentItem: ColumnLayout {
            spacing: Kirigami.Units.smallSpacing
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.BusyIndicator { running: dbman.opActive; Layout.preferredWidth: 22; Layout.preferredHeight: 22 }
                Controls.Label {
                    text: dbman.opActive ? "Working — this takes a few minutes, please wait…" : (dbman.opOk ? "Done." : "Finished with errors — see log above.")
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    opacity: 0.8
                }
            }
            Controls.ScrollView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                Controls.ScrollBar.vertical.policy: Controls.ScrollBar.AlwaysOn
                Controls.TextArea {
                    text: dbman.opLog
                    readOnly: true
                    font.family: "monospace"
                    font.pointSize: 8
                    wrapMode: TextEdit.NoWrap
                    selectByMouse: true
                    onTextChanged: cursorPosition = length
                }
            }
        }
        footer: Controls.DialogButtonBox {
            Controls.Button {
                text: "Close"
                enabled: !dbman.opActive
                Controls.DialogButtonBox.buttonRole: Controls.DialogButtonBox.RejectRole
            }
        }
    }
    Timer {
        id: opTimer
        interval: 1000
        running: opDialog.visible || dbman.opActive
        repeat: true
        triggeredOnStart: false
        property bool wasActive: false
        onTriggered: {
            dbman.pollDbOp()
            if (wasActive && !dbman.opActive) {
                opDialog.close()
                databaseManagerPage.notifyLeft(dbman.statusMessage, dbman.opOk ? 4000 : 8000,
                    dbman.opOk ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                dbman.refreshStatus()
            }
            wasActive = dbman.opActive
        }
    }
}
