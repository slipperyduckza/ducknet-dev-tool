import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.ducknetdevtool

// Create Database page: reached from Database Manager's [Create Database]
// button. The toggle selects the engine (locked to the installed one when
// only a single engine is present); [Create] runs that engine's procedure.
Kirigami.Page {
    id: createDatabasePage
    title: "Create a New Database"
    property var leftPage: null

    function notifyLeft(msg, duration, type) {
        if (leftPage && leftPage.showLeftNotification)
            leftPage.showLeftNotification(msg, duration, type)
    }
    function goBack() {
        if (leftPage && leftPage.closeCreateDatabase)
            leftPage.closeCreateDatabase()
    }

    // "postgres" (switch left) or "mariadb" (switch right).
    function selectedKind() {
        return dbSwitch.checked ? "mariadb" : "postgres"
    }

    DatabaseManager {
        id: dbman
    }

    Component.onCompleted: {
        dbman.refreshStatus()
        // Only one engine installed: pre-select it (the switch is locked
        // below, so the toggle cannot be changed).
        if (dbman.postgresInstalled && !dbman.mariadbInstalled)
            dbSwitch.checked = false
        else if (!dbman.postgresInstalled && dbman.mariadbInstalled)
            dbSwitch.checked = true
        else
            dbSwitch.checked = false
    }

    Controls.ScrollView {
        id: createDbScroll
        anchors.fill: parent
        contentWidth: availableWidth
        Controls.ScrollBar.vertical.policy: Controls.ScrollBar.AlwaysOn
        Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff

        ColumnLayout {
            width: createDbScroll.availableWidth - 24
            anchors.margins: Kirigami.Units.largeSpacing
            spacing: Kirigami.Units.smallSpacing

            Kirigami.Heading { text: "Create a New Database"; level: 2; wrapMode: Text.WordWrap; Layout.fillWidth: true }
            Controls.Label {
                text: "This tool allows you to generate a new Database in your Develpment environment for your WebApps."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // Engine toggle: PostgreSQL left, MariaDB right. The label on
            // the selected side is highlighted; with a single engine
            // installed the switch locks to that side.
            RowLayout {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignHCenter
                spacing: Kirigami.Units.smallSpacing
                Controls.Label {
                    text: "PostgreSQL"
                    font.bold: !dbSwitch.checked
                    font.pointSize: 10
                    color: !dbSwitch.checked ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.disabledTextColor
                }
                Controls.Switch {
                    id: dbSwitch
                    checked: false
                    enabled: dbman.postgresInstalled && dbman.mariadbInstalled
                    Accessible.name: "Database type: left PostgreSQL, right MariaDB"
                }
                Controls.Label {
                    text: "MariaDB"
                    font.bold: dbSwitch.checked
                    font.pointSize: 10
                    color: dbSwitch.checked ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.disabledTextColor
                }
            }
            Controls.Label {
                text: !dbman.postgresInstalled && !dbman.mariadbInstalled
                    ? "Neither database is installed — go back and install one first."
                    : (!dbman.postgresInstalled || !dbman.mariadbInstalled
                        ? "Only one database type is installed — the toggle is locked to it."
                        : "Toggle to select the database type.")
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                opacity: 0.6
                font.pointSize: 8
            }

            Kirigami.Separator { Layout.fillWidth: true }

            Controls.Label { text: "Database Name"; font.bold: true }
            Controls.TextField {
                id: dbNameField
                Layout.fillWidth: true
                placeholderText: "e.g. webapp"
            }
            Controls.Label { text: "Database User"; font.bold: true }
            Controls.TextField {
                id: dbUserField
                Layout.fillWidth: true
                placeholderText: "e.g. webuser"
            }
            Controls.Label { text: "Password"; font.bold: true }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.TextField {
                    id: dbPasswordField
                    Layout.fillWidth: true
                    echoMode: revealPassword.checked ? TextInput.Normal : TextInput.Password
                    placeholderText: "Owner password"
                }
                Controls.ToolButton {
                    id: revealPassword
                    checkable: true
                    icon.name: checked ? "view-hidden" : "view-visible"
                    Accessible.name: checked ? "Hide password" : "Reveal password to check it"
                }
            }
            Controls.Label {
                text: "Names: letter/underscore first, then letters, digits, _ or $ (1–63 chars). The user becomes the database owner (PostgreSQL) or gets full rights on it via localhost (MariaDB)."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.6
                font.pointSize: 8
            }
            // MariaDB-only: utf8mb4 tickbox (PostgreSQL databases are
            // always created UTF-8, so it stays hidden there).
            Controls.CheckBox {
                id: utf8mb4Check
                visible: dbSwitch.checked
                text: "Create with utf8mb4 encoding (4-byte characters/emoji)"
            }
            Controls.Label {
                visible: !dbSwitch.checked
                text: "PostgreSQL databases are always created with UTF-8 encoding."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.6
                font.pointSize: 8
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Item { Layout.fillWidth: true }
                Controls.Button {
                    text: "Create"
                    icon.name: "document-new"
                    enabled: dbman.postgresInstalled || dbman.mariadbInstalled
                    onClicked: {
                        var kind = createDatabasePage.selectedKind()
                        var ok = dbman.createDatabase(kind, dbNameField.text.trim(), dbUserField.text.trim(), dbPasswordField.text, utf8mb4Check.checked)
                        if (ok) {
                            createDatabasePage.notifyLeft(dbman.statusMessage, 4000, Kirigami.MessageType.Positive)
                            goBack()
                        } else {
                            createDatabasePage.notifyLeft(dbman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                }
                Controls.Button {
                    text: "Cancel"
                    icon.name: "dialog-cancel"
                    onClicked: goBack()
                }
            }
        }
    }
}
