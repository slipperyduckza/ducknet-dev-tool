import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Controls // plain import required for the ToolTip attached object below — not a duplicate
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.Page {
    id: devCertsPage
    title: "DEV HTTPS Certs"
    property var devCertsManager: null
    property var leftPage: null
    readonly property string caStatus: devCertsManager ? devCertsManager.caSetupStatus : "NotSetup"

    function notifyLeft(msg, duration, type) {
        if (leftPage && leftPage.showLeftNotification) leftPage.showLeftNotification(msg, duration, type)
        else console.log("[devcerts] " + msg)
    }

    // Real DevCertsManager provided by Rust (cxx-qt) via org.kde.ducknetdevtool
    // No QML mock — all OpenSSL / pkexec logic lives in src/devcerts.rs

    Controls.ScrollView {
        id: certScroll
        anchors.fill: parent
        clip: true
        Controls.ScrollBar.vertical.policy: Controls.ScrollBar.AlwaysOn
        Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff
        // Ensure contentWidth tracks available width for FormLayout wrapping
        contentWidth: availableWidth

        Kirigami.FormLayout {
            // -24 keeps content clear of the overlaid AlwaysOn scrollbar.
            width: certScroll.availableWidth - 24
            // Use padding via anchors.margins on inner layout; ScrollView already clips
            // Keep original margins as internal padding
            anchors.margins: Kirigami.Units.largeSpacing

        Controls.Label {
            Kirigami.FormData.isSection: true
            text: "Setup Your Local Certificate Authority (CA)"
            font.bold: true
            font.pointSize: 12
        }
        Controls.Label {
            text: "This utility provides an easy visual GUI to set up a local (only) Certificate Authority.\nThis then lets you generate any local domain certificates that are trusted by your browser eg. mysite.test\nWith each domain generated the tool will associate the domainname to 127.0.0.1 (localhost) in /etc/hosts\nThe result is valid working SSL (HTTPS) certificates for websites/services local to this machine (only)\nThis ensures that your development of applications can include full SSL based security without the need of 3rd Party SSL services"
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
            opacity: 0.8
        }
        Controls.Label {
            Kirigami.FormData.isSection: true
            text: ({
                "NotSetup": "Certificate Authority: Not Setup",
                "Setup": "Certificate Authority: Ready — install to trust store",
                "Installed": "Certificate Authority: Installed",
                "Unsupported": "Certificate Authority: Unsupported OS"
            }[devCertsPage.caStatus] || "CA: " + devCertsPage.caStatus)
            font.bold: true
            color: devCertsPage.caStatus === "Installed" ? Kirigami.Theme.positiveTextColor :
                   devCertsPage.caStatus === "Setup" ? Kirigami.Theme.neutralTextColor :
                   devCertsPage.caStatus === "Unsupported" ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.negativeTextColor
        }
        Controls.Label {
            Kirigami.FormData.isSection: true
            text: devCertsManager ? devCertsManager.statusMessage : "No manager"
            wrapMode: Text.WordWrap
            opacity: 0.8
        }

        // ── NotSetup ──
        ColumnLayout {
            id: caNotSetup
            visible: devCertsPage.caStatus === "NotSetup"
            Kirigami.FormData.isSection: true
            spacing: Kirigami.Units.smallSpacing
            Kirigami.Heading { text: "1 — Setup Certificate Authority"; level: 3 }
            Controls.Label { text: "Creates AES-256 encrypted ~/certs/myCA.key (600) + ~/certs/myCA.pem via OpenSSL. Uses -passin file: to avoid CLI exposure."; wrapMode: Text.WordWrap; Layout.fillWidth: true; opacity: 0.6; font.pointSize: 8 }

            Kirigami.FormLayout {
                Controls.TextField { id: passField; Kirigami.FormData.label: "Passphrase*:"; echoMode: TextInput.Password; placeholderText: "Strong passphrase"; }
                Controls.TextField { id: cField; Kirigami.FormData.label: "Country (C):"; text: "US"; placeholderText: "US" }
                Controls.TextField { id: stField; Kirigami.FormData.label: "State (ST):"; text: "California" }
                Controls.TextField { id: lField; Kirigami.FormData.label: "Locality (L):"; text: "San Francisco" }
                Controls.TextField { id: oField; Kirigami.FormData.label: "Organization (O):"; text: "DuckNet" }
                Controls.TextField { id: ouField; Kirigami.FormData.label: "Org Unit (OU):"; text: "Dev" }
                Controls.TextField { id: cnField; Kirigami.FormData.label: "Common Name (CN)*:"; text: "DuckNet Dev CA"; placeholderText: "DuckNet Dev CA" }
                Controls.TextField { id: emailField; Kirigami.FormData.label: "Email:"; text: "dev@ducknet.test"; placeholderText: "dev@ducknet.test" }
                Controls.SpinBox { id: caDays; Kirigami.FormData.label: "Validity (days):"; from: 1; to: 9999; value: 1825; editable: true }
                Controls.SpinBox { id: keySize; Kirigami.FormData.label: "Key size:"; from: 1024; to: 4096; stepSize: 1024; value: 2048; editable: false; textFromValue: v => v.toString() }
            }
            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                Controls.Button {
                    text: "Setup CA"
                    icon.name: "security-high"
                    enabled: cnField.text.length > 0 && passField.text.length > 0
                    onClicked: {
                        let ok = devCertsPage.devCertsManager.setupCA(passField.text, cField.text, stField.text, lField.text, oField.text, ouField.text, cnField.text, emailField.text, caDays.value, keySize.value)
                        if (ok) notifyLeft("CA setup — now install", 3000)
                        else notifyLeft("Setup failed: " + devCertsPage.devCertsManager.statusMessage, 4000)
                    }
                }
                Controls.Button {
                    text: "Fill demo"
                    icon.name: "edit-clear"
                    onClicked: { passField.text = "demo123"; cnField.text = "DuckNet Dev CA" }
                }
            }
        }

        // ── Setup ──
        ColumnLayout {
            visible: devCertsPage.caStatus === "Setup"
            Kirigami.FormData.isSection: true
            spacing: Kirigami.Units.smallSpacing
            Kirigami.Heading { text: "2 — Install Root Certificate"; level: 3 }
            Controls.Label { text: "CA: " + (devCertsManager ? devCertsManager.commonName : ""); font.bold: true; visible: devCertsManager && devCertsManager.commonName }
            Controls.Label {
                text: devCertsManager ? ("Detected: " + devCertsManager.distroId + " (" + devCertsManager.distroFamily + ") — Runs: sudo cp ~/certs/myCA.pem " + devCertsManager.installedCertPath + " && sudo " + devCertsManager.updateCommand + " (passwordless sudo assumed).") : "Runs: sudo cp ~/certs/myCA.pem …"
                wrapMode: Text.WordWrap; Layout.fillWidth: true; opacity: 0.6; font.pointSize: 8
            }
            Controls.Button {
                text: "Install Root CA"
                icon.name: "dialog-ok"
                Layout.alignment: Qt.AlignHCenter
                onClicked: {
                    console.log("qml: installRootCert clicked, calling backend... distro:", devCertsPage.devCertsManager.distroFamily, "path:", devCertsPage.devCertsManager.installedCertPath)
                    let ok = devCertsPage.devCertsManager.installRootCert()
                    console.log("qml: installRootCert returned", ok, "status:", devCertsPage.devCertsManager.statusMessage, "caStatus:", devCertsPage.devCertsManager.caSetupStatus)
                    notifyLeft(ok ? "Installed — check " + devCertsPage.devCertsManager.installedCertPath : "Install failed: " + devCertsPage.devCertsManager.statusMessage, ok ? 4000 : 8000)
                }
            }
        }

        // ── Installed ──
        ColumnLayout {
            visible: devCertsPage.caStatus === "Installed"
            Kirigami.FormData.isSection: true
            spacing: Kirigami.Units.smallSpacing
            Kirigami.Heading { text: "3 — Generate Domain Certificate"; level: 3 }
            Rectangle {
                Layout.fillWidth: true
                color: Kirigami.Theme.highlightColor
                radius: Kirigami.Units.smallSpacing
                implicitHeight: caNameLabel.implicitHeight + Kirigami.Units.smallSpacing * 2
                visible: devCertsManager && devCertsManager.commonName
                Controls.Label {
                    id: caNameLabel
                    anchors.centerIn: parent
                    text: "CA: " + (devCertsManager ? devCertsManager.commonName : "")
                    font.bold: true
                    color: Kirigami.Theme.highlightedTextColor
                }
            }
            Controls.Label { text: "Creates ~/certs/<domain>.key/.csr/.crt + .ext (SAN, wildcard) signed by myCA via OpenSSL x509 -req -CAcreateserial -extfile -passin file:"; wrapMode: Text.WordWrap; Layout.fillWidth: true; opacity: 0.6; font.pointSize: 8 }

            Kirigami.FormLayout {
                Controls.TextField { id: domainField; Kirigami.FormData.label: "Domain*:"; placeholderText: "myapp.test"; }
                ColumnLayout {
                    Kirigami.FormData.label: "SANs:"; Kirigami.FormData.buddyFor: sansView; Layout.fillWidth: true; spacing: Kirigami.Units.smallSpacing
                    ListView {
                        id: sansView; Layout.fillWidth: true; Layout.preferredHeight: 90; clip: true
                        model: ListModel { id: sansModel }
                        delegate: RowLayout {
                            width: sansView.width
                            Controls.TextField { text: model.san; Layout.fillWidth: true; placeholderText: "api.myapp.test or 192.168.1.10"; onTextChanged: sansModel.setProperty(index, "san", text) }
                            Controls.ToolButton { icon.name: "list-remove"; onClicked: sansModel.remove(index) }
                        }
                        Controls.Label { anchors.centerIn: parent; text: "No extra SANs — primary domain auto-added"; visible: sansModel.count === 0; opacity: 0.5; font.pointSize: 8 }
                    }
                    RowLayout {
                        Controls.TextField { id: newSan; placeholderText: "Add SAN (DNS/IP)"; Layout.fillWidth: true; onAccepted: addBtn.clicked() }
                        Controls.Button { id: addBtn; text: "Add"; icon.name: "list-add"; onClicked: if (newSan.text) { sansModel.append({san: newSan.text}); newSan.clear() } }
                    }
                }
                Controls.CheckBox { id: wild; Kirigami.FormData.label: "Wildcard:"; text: "Add *.<domain> + base" }
                Controls.SpinBox { id: certDays; Kirigami.FormData.label: "Validity:"; from: 1; to: 9999; value: 825; editable: true }
            }
            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                Controls.Button {
                    text: "Generate Certificate"
                    icon.name: "document-new"
                    enabled: domainField.text.length > 0
                    onClicked: {
                        let sans = []
                        for (let i=0;i<sansModel.count;i++) sans.push(sansModel.get(i).san)
                        let ok = devCertsPage.devCertsManager.generateCert(domainField.text, sans, wild.checked, certDays.value)
                        notifyLeft(ok ? "Generated " + domainField.text : "Failed: " + devCertsPage.devCertsManager.statusMessage, 3000)
                    }
                }
                Controls.Button {
                    text: "Reset View"
                    icon.name: "edit-undo"
                    onClicked: { devCertsPage.devCertsManager.caSetupStatus = "NotSetup"; devCertsPage.devCertsManager.statusMessage = "Reset to NotSetup" }
                }
            }

            Kirigami.Heading { text: "Generated Certificates"; level: 4; visible: generatedList.count > 0 }
            ListView {
                id: generatedList; Layout.fillWidth: true; Layout.preferredHeight: 140; clip: true
                model: devCertsPage.devCertsManager ? devCertsPage.devCertsManager.generatedCerts : []
                delegate: Controls.ItemDelegate {
                    width: generatedList.width
                    contentItem: RowLayout {
                        Kirigami.Icon { source: "security-high"; Layout.preferredWidth: 22; Layout.preferredHeight: 22 }
                        ColumnLayout {
                            Layout.fillWidth: true
                            Controls.Label { text: modelData; font.bold: true; elide: Text.ElideRight }
                            Controls.Label { text: "~/certs/" + modelData + ".crt"; opacity: 0.6; font.pointSize: 8; elide: Text.ElideRight }
                        }
                        Controls.ToolButton {
                            icon.name: "edit-delete"
                            text: "Delete"
                            display: Controls.AbstractButton.IconOnly
                            ToolTip.text: "Delete " + modelData + " (removes ~/certs/" + modelData + ".key/.csr/.crt/.ext)"
                            ToolTip.visible: hovered
                            onClicked: {
                                console.log("qml: deleteCert clicked", modelData)
                                let ok = devCertsPage.devCertsManager.deleteCert(modelData)
                                console.log("qml: deleteCert returned", ok, "status:", devCertsPage.devCertsManager.statusMessage)
                                devCertsPage.notifyLeft(ok ? "Deleted " + modelData : "Delete failed: " + devCertsPage.devCertsManager.statusMessage, ok ? 3000 : 4000)
                            }
                        }
                    }
                }
            }

            Kirigami.Separator { Layout.fillWidth: true; Layout.topMargin: Kirigami.Units.largeSpacing }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Kirigami.Heading { text: "Firefox"; level: 4 }
                Controls.Label {
                    text: "Chrome auto-trusts the system CA from /usr/local/share/ca-certificates (Debian) or /etc/pki/ca-trust/source/anchors (Fedora) — no action needed. Firefox uses its own NSSDB: ~/.mozilla/firefox (Debian) or ~/.config/mozilla/firefox (Fedora), *.default-release + *.default (CA at ~/certs/myCA.pem, password from ~/.local/state/devbox.arc via /etc/machine-id)."
                    wrapMode: Text.WordWrap; Layout.fillWidth: true; opacity: 0.6; font.pointSize: 8
                }
                Controls.Label {
                    text: "Please NOTE: You must launch Firefox and complete the initial setup steps in order for a profile to be created, which is where the CA will be installed."
                    wrapMode: Text.WordWrap; Layout.fillWidth: true; opacity: 0.9; font.bold: true
                }
                RowLayout {
                    Layout.alignment: Qt.AlignHCenter
                    spacing: Kirigami.Units.smallSpacing
                    // LED status: green = CA detected in Firefox NSSDB, red = not detected
                    Rectangle {
                        id: firefoxLed
                        width: 12
                        height: 12
                        radius: 6
                        color: !devCertsPage.devCertsManager ? Kirigami.Theme.disabledTextColor
                            : devCertsPage.devCertsManager.firefoxCaInstalled ? Kirigami.Theme.positiveTextColor
                            : Kirigami.Theme.negativeTextColor
                        border.width: 1
                        border.color: Kirigami.Theme.disabledTextColor
                    }
                    Controls.Label {
                        text: !devCertsPage.devCertsManager ? "Status: Unknown"
                            : devCertsPage.devCertsManager.firefoxCaInstalled ? "Status: CA installed in Firefox"
                            : "Status: CA not installed in Firefox"
                        font.pointSize: 8
                        opacity: 0.8
                    }
                    Controls.ToolButton {
                        icon.name: "view-refresh"
                        ToolTip.text: "Re-check Firefox CA status"
                        ToolTip.visible: hovered
                        onClicked: {
                            if (devCertsPage.devCertsManager && devCertsPage.devCertsManager.refreshFirefoxStatus !== undefined) {
                                let installed = devCertsPage.devCertsManager.refreshFirefoxStatus()
                                console.log("qml: refreshFirefoxStatus returned", installed)
                            }
                        }
                    }
                    Component.onCompleted: {
                        if (devCertsPage.devCertsManager && devCertsPage.devCertsManager.refreshFirefoxStatus !== undefined)
                            devCertsPage.devCertsManager.refreshFirefoxStatus()
                    }
                }
                RowLayout {
                    Layout.alignment: Qt.AlignHCenter
                    spacing: Kirigami.Units.largeSpacing
                    Controls.Button {
                        text: "Install to Firefox"
                        icon.name: "firefox"
                        onClicked: {
                            console.log("qml: installToFirefox clicked")
                            let ok = devCertsPage.devCertsManager.installToFirefox()
                            console.log("qml: installToFirefox returned", ok, "status:", devCertsPage.devCertsManager.statusMessage)
                            notifyLeft(ok ? "Firefox: " + devCertsPage.devCertsManager.statusMessage : "Firefox failed: " + devCertsPage.devCertsManager.statusMessage, ok ? 4000 : 8000)
                        }
                    }
                    Controls.Button {
                        text: "Delete from Firefox"
                        icon.name: "edit-delete"
                        onClicked: {
                            console.log("qml: deleteFromFirefox clicked")
                            let ok = devCertsPage.devCertsManager.deleteFromFirefox()
                            console.log("qml: deleteFromFirefox returned", ok, "status:", devCertsPage.devCertsManager.statusMessage)
                            notifyLeft(ok ? "Firefox: " + devCertsPage.devCertsManager.statusMessage : "Firefox delete failed: " + devCertsPage.devCertsManager.statusMessage, ok ? 4000 : 8000)
                        }
                    }
                }
            }

            Kirigami.Separator { Layout.fillWidth: true; Layout.topMargin: Kirigami.Units.smallSpacing }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Kirigami.Heading { text: "Regenerate CA"; level: 4 }
                Controls.Label {
                    text: devCertsManager ? ("Removes " + devCertsManager.installedCertPath + " (runs sudo " + devCertsManager.updateCommand + " via pkexec), all ~/certs domain certificates + /etc/hosts entries, the CA from Firefox, and the stored passphrase — then returns to 1 — Setup.") : "Removes installed CA and returns to Setup."
                    wrapMode: Text.WordWrap; Layout.fillWidth: true; opacity: 0.6; font.pointSize: 8
                }
                RowLayout {
                    Layout.alignment: Qt.AlignHCenter
                    Controls.Button {
                        text: "Remove Root CA & Regenerate"
                        icon.name: "edit-delete"
                        // Kirigami.Theme.negativeTextColor for emphasis if available
                        onClicked: regenerateConfirm.open()
                    }
                }
            }
        }

        Kirigami.Separator { Kirigami.FormData.isSection: true }
        Controls.Label {
            Kirigami.FormData.isSection: true
            text: devCertsPage.devCertsManager ? devCertsPage.devCertsManager.statusMessage : "Ready"
            wrapMode: Text.WordWrap
            color: Kirigami.Theme.disabledTextColor
            font.pointSize: 8
        }
        }
    }

    Connections {
        target: devCertsPage.devCertsManager
        function onStatusChanged(m) { /* property binding handles */ }
        function onProgressUpdated(m) { notifyLeft(m, 2000) }
    }

    Controls.Dialog {
        id: regenerateConfirm
        title: "Regenerate CA?"
        modal: true
        anchors.centerIn: parent
        width: Math.min(parent.width * 0.8, 480)
        standardButtons: Controls.Dialog.Yes | Controls.Dialog.Cancel
        Controls.Label {
            text: "This will remove your CA and all associated certificates, you will need to recreate your certificates from scratch, would you like to proceed?"
            wrapMode: Text.WordWrap
            width: parent ? parent.width : 400
        }
        onAccepted: {
            console.log("qml: removeRootCert confirmed, distro:", devCertsPage.devCertsManager.distroFamily, "path:", devCertsPage.devCertsManager.installedCertPath)
            let ok = devCertsPage.devCertsManager.removeRootCert()
            console.log("qml: removeRootCert returned", ok, "status:", devCertsPage.devCertsManager.statusMessage, "caStatus:", devCertsPage.devCertsManager.caSetupStatus)
            notifyLeft(ok ? "Removed — back to Setup" : "Remove failed: " + devCertsPage.devCertsManager.statusMessage, ok ? 4000 : 8000)
        }
    }
}
