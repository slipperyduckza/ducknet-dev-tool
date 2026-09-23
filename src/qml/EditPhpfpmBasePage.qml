import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.ducknetdevtool

// Full-page editor for the PHP-FPM pool www.conf (Debian:
// /etc/php/8.4/fpm/pool.d/www.conf, Fedora: /etc/php-fpm.d/www.conf).
// Same own-page method as EditSitePage, but WITHOUT the nginx linter:
// www.conf is INI-style, not nginx syntax. `<fpm> -t` remains the gate.
Kirigami.Page {
    id: editPhpfpmBasePage
    title: "Edit www.conf"
    property var leftPage: null
    property string initialText: ""

    function notifyLeft(msg, duration, type) {
        if (leftPage && leftPage.showLeftNotification)
            leftPage.showLeftNotification(msg, duration, type)
    }
    function goBack() {
        if (leftPage && leftPage.closeBaseEditor)
            leftPage.closeBaseEditor()
    }

    NginxManager {
        id: editPhpfpmman
    }

    Shortcut {
        sequence: "Ctrl+F"
        context: Qt.WindowShortcut
        onActivated: findBar.open()
    }

    Component.onCompleted: {
        baseEditor.text = initialText
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: Kirigami.Units.largeSpacing
        spacing: Kirigami.Units.smallSpacing

        RowLayout {
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing
            Controls.Button {
                text: "Back"
                icon.name: "go-previous"
                onClicked: {
                    editPhpfpmman.cancelPhpfpmBaseEdit()
                    goBack()
                }
            }
            Item { Layout.fillWidth: true }
            Controls.Button {
                text: "Save"
                icon.name: "document-save"
                onClicked: {
                    var ok = editPhpfpmman.savePhpfpmBaseConfig(baseEditor.text)
                    notifyLeft(editPhpfpmman.statusMessage, ok ? 4000 : 8000,
                        ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                    if (ok) {
                        goBack()
                    }
                }
            }
            Controls.Button {
                text: "Cancel"
                icon.name: "dialog-cancel"
                onClicked: {
                    editPhpfpmman.cancelPhpfpmBaseEdit()
                    goBack()
                }
            }
            Controls.Button {
                text: "Find"
                icon.name: "edit-find"
                onClicked: findBar.open()
            }
        }

        Controls.Label {
            text: "Editing www.conf — tested with php-fpm -t before applying (no nginx lint for this INI-style file)."
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
            opacity: 0.6
            font.pointSize: 8
        }

        // Find bar (Ctrl+F) — shared component, see EditorFindBar.qml.
        EditorFindBar {
            id: findBar
            Layout.fillWidth: true
            editor: baseEditor
        }

        Controls.ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            Controls.TextArea {
                id: baseEditor
                font.family: "monospace"
                wrapMode: TextEdit.NoWrap
                selectByMouse: true
                persistentSelection: true
                color: "#e8e8e8"
                selectionColor: "#3a6ea5"
                selectedTextColor: "#ffffff"
                background: Rectangle {
                    color: "#1e1e1e"
                    border.color: "#4a4a4a"
                    border.width: 1
                    radius: 4
                }
                onTextChanged: {
                    if (findBar.visible)
                        findBar.refresh()
                }
            }
        }

        Controls.Label {
            text: "Save tests the pool file first — an invalid file is reverted automatically and PHP-FPM is restarted on success."
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
            opacity: 0.6
            font.pointSize: 8
        }
    }
}
