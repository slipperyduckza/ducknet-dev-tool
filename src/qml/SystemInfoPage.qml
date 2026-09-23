import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.ducknetdevtool

Kirigami.Page {
    id: sysPage
    title: "System Info"

    SystemInfoManager {
        id: sysinfo
    }

    Timer {
        id: liveTimer
        interval: 2000
        running: true
        repeat: true
        triggeredOnStart: false
        onTriggered: sysinfo.refreshLive()
    }

    Component.onCompleted: sysinfo.refreshAll()

    Controls.ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth

        ColumnLayout {
            width: parent.width
            anchors.margins: Kirigami.Units.largeSpacing
            spacing: Kirigami.Units.smallSpacing

            Kirigami.Heading { text: "System Info"; level: 2 }

            // ---- Host identity (one-shot) ----
            Kirigami.Heading { text: "Host"; level: 3 }
            Kirigami.FormLayout {
                Layout.fillWidth: true
                Controls.Label { Kirigami.FormData.label: "Hostname:"; text: sysinfo.hostname; textFormat: Text.PlainText }
                Controls.Label { Kirigami.FormData.label: "Platform:"; text: sysinfo.platform; wrapMode: Text.WordWrap; Layout.fillWidth: true }
                Controls.Label { Kirigami.FormData.label: "Distro:"; text: sysinfo.distroFamily + " (" + sysinfo.distroId + ")" }
                Controls.Label { Kirigami.FormData.label: "Locale:"; text: sysinfo.locale }
                Controls.Label { Kirigami.FormData.label: "Plasma locale:"; text: sysinfo.plasmaLocale !== "" ? sysinfo.plasmaLocale : "—" }
                Controls.Label { Kirigami.FormData.label: "Timezone:"; text: sysinfo.timezone }
                Controls.Label { Kirigami.FormData.label: "Date / Time:"; text: sysinfo.dateTime }
                Controls.Label { Kirigami.FormData.label: "Primary IP:"; text: sysinfo.primaryIp !== "" ? sysinfo.primaryIp : "—" }
                Controls.Label { Kirigami.FormData.label: "Nginx:"; text: sysinfo.nginxVersion }
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.topMargin: Kirigami.Units.smallSpacing
                Controls.Button {
                    text: "Fix Plasma Locale"
                    icon.name: "set-language"
                    onClicked: {
                        sysinfo.refreshLocales()
                        localePicker.allLocales = Array.from(sysinfo.availableLocales)
                        localePicker.selectedLocale = sysinfo.plasmaLocale !== "" ? sysinfo.plasmaLocale : sysinfo.locale
                        localePicker.searchText = ""
                        localePicker.refreshFilter()
                        localePickerDialog.open()
                    }
                }
                Controls.Label {
                    text: sysinfo.localeStatus
                    visible: sysinfo.localeStatus !== ""
                    wrapMode: Text.WordWrap
                    Layout.fillWidth: true
                    opacity: 0.8
                    font.pointSize: 8
                }
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Live resources ----
            Kirigami.Heading { text: "Resources (live)"; level: 3 }

            Controls.Label { text: "Physical Memory — " + sysinfo.memText }
            Controls.ProgressBar {
                Layout.fillWidth: true
                from: 0; to: 100
                value: sysinfo.memUsagePercent
            }

            Controls.Label { text: "CPU — " + sysinfo.cpuCores + " cores, " + sysinfo.cpuUsagePercent.toFixed(1) + "%" }
            Controls.ProgressBar {
                Layout.fillWidth: true
                from: 0; to: 100
                value: sysinfo.cpuUsagePercent
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Storage (one-shot) ----
            Kirigami.Heading { text: "Storage"; level: 3 }
            Controls.Label { text: sysinfo.storageText; wrapMode: Text.WordWrap; Layout.fillWidth: true }
            Controls.ProgressBar {
                Layout.fillWidth: true
                from: 0; to: 100
                value: sysinfo.storageUsagePercent
            }

            RowLayout {
                Layout.topMargin: Kirigami.Units.smallSpacing
                Item { Layout.fillWidth: true }
                Controls.Button {
                    text: "Refresh all"
                    icon.name: "view-refresh"
                    onClicked: sysinfo.refreshAll()
                }
            }
        }
    }

    // Non-visual state for the locale picker (kept outside the Dialog so
    // filtering survives open/close and stays testable from QML).
    QtObject {
        id: localePicker
        property var allLocales: []
        property var filteredLocales: []
        property string selectedLocale: ""
        property string searchText: ""
        function refreshFilter() {
            let q = searchText.trim().toLowerCase()
            if (q === "") {
                filteredLocales = allLocales
            } else {
                filteredLocales = allLocales.filter(loc => String(loc).toLowerCase().includes(q))
            }
            // Keep selection visible when possible; else clear stale selection.
            if (selectedLocale !== "" && !allLocales.includes(selectedLocale)) {
                selectedLocale = ""
            }
        }
    }

    // ---- Step 1: searchable locale selection (from `localectl list-locales`) ----
    Controls.Dialog {
        id: localePickerDialog
        title: "Fix Plasma Locale"
        modal: true
        anchors.centerIn: parent
        width: Math.min(parent.width * 0.9, 520)
        height: Math.min(parent.height * 0.85, 560)
        standardButtons: Controls.Dialog.Ok | Controls.Dialog.Cancel
        // OK only meaningful with a selection.
        onOpened: localeSearch.forceActiveFocus()
        onAccepted: {
            if (localePicker.selectedLocale !== "") {
                localeConfirm.selectedLocale = localePicker.selectedLocale
                localeConfirmDialog.open()
            }
        }

        contentItem: ColumnLayout {
            spacing: Kirigami.Units.smallSpacing
            Controls.Label {
                text: "Select locale (from localectl list-locales). Written to ~/.config/plasma-localerc as [Formats] LANG + [Translations] LANGUAGE."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
                font.pointSize: 8
            }
            Controls.TextField {
                id: localeSearch
                placeholderText: "Search locales (e.g. en_ZA)…"
                Layout.fillWidth: true
                text: localePicker.searchText
                onTextChanged: {
                    localePicker.searchText = text
                    localePicker.refreshFilter()
                }
            }
            Controls.Label {
                text: localePicker.filteredLocales.length + " of " + localePicker.allLocales.length + " locales"
                    + (localePicker.selectedLocale !== "" ? " — selected: " + localePicker.selectedLocale : "")
                opacity: 0.6
                font.pointSize: 8
                Layout.fillWidth: true
                elide: Text.ElideRight
            }
            ListView {
                id: localeList
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.preferredHeight: 300
                clip: true
                model: localePicker.filteredLocales
                delegate: Controls.ItemDelegate {
                    width: localeList.width
                    highlighted: modelData === localePicker.selectedLocale
                    text: modelData
                    onClicked: localePicker.selectedLocale = modelData
                }
                Controls.Label {
                    anchors.centerIn: parent
                    text: "No matching locales"
                    visible: localePicker.filteredLocales.length === 0
                    opacity: 0.5
                }
            }
        }
    }

    // ---- Step 2: logout / reboot confirmation ----
    QtObject {
        id: localeConfirm
        property string selectedLocale: ""
    }
    Controls.Dialog {
        id: localeConfirmDialog
        title: "Apply locale?"
        modal: true
        anchors.centerIn: parent
        width: Math.min(parent.width * 0.85, 460)
        standardButtons: Controls.Dialog.NoButton
        contentItem: ColumnLayout {
            spacing: Kirigami.Units.smallSpacing
            Controls.Label {
                text: "Do you want to logout or reboot to enable your corrected locale?"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }
            Controls.Label {
                text: "Selected: " + localeConfirm.selectedLocale
                font.bold: true
                Layout.fillWidth: true
                elide: Text.ElideRight
            }
            Controls.Label {
                text: sysinfo.localeStatus
                visible: sysinfo.localeStatus !== ""
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.8
                font.pointSize: 8
            }
        }
        footer: RowLayout {
            spacing: Kirigami.Units.smallSpacing
            Layout.margins: Kirigami.Units.smallSpacing
            Item { Layout.fillWidth: true }
            Controls.Button {
                text: "Reboot Now"
                icon.name: "system-reboot"
                onClicked: {
                    let ok = sysinfo.applyPlasmaLocale(localeConfirm.selectedLocale)
                    localeConfirmDialog.close()
                    if (ok) sysinfo.rebootSystem()
                }
            }
            Controls.Button {
                text: "Logout Now"
                icon.name: "system-log-out"
                onClicked: {
                    let ok = sysinfo.applyPlasmaLocale(localeConfirm.selectedLocale)
                    localeConfirmDialog.close()
                    if (ok) sysinfo.logoutSession()
                }
            }
            Controls.Button {
                text: "Cancel"
                icon.name: "dialog-cancel"
                onClicked: localeConfirmDialog.close()
            }
        }
    }
}
