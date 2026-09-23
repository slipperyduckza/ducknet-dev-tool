import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.Page {
    id: helpRoot
    title: "Dev Tool Help"
    padding: 0
    Controls.ScrollView {
        id: helpScroll
        anchors.fill: parent
        clip: true
        Controls.ScrollBar.vertical.policy: Controls.ScrollBar.AlwaysOn
        Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff
        // Fixed scrollbar policy keeps availableWidth stable, so wrapped
        // text heights are computed once — no overlap or cut-off below.
        contentWidth: availableWidth

        ColumnLayout {
            // -24 keeps content clear of the overlaid AlwaysOn scrollbar.
            width: helpScroll.availableWidth - 24
            anchors.margins: Kirigami.Units.largeSpacing
            spacing: Kirigami.Units.largeSpacing

            Kirigami.Heading { text: "Certificates & nginx — What this tool does"; level: 2 }

            Controls.Label {
                wrapMode: Text.WordWrap; Layout.fillWidth: true
                text: "Creates a **local Certificate Authority (CA)** in <b>~/certs/</b> so your dev domains (e.g. <code>myapp.test</code>, <code>*.myapp.local</code>) serve real HTTPS without browser warnings — same workflow as <a href=\"https://deliciousbrains.com/ssl-certificate-authority-for-local-https-development/\">deliciousbrains.com</a>."
            }

            Kirigami.Card {
                Layout.fillWidth: true
                header: Kirigami.Heading { text: "1 — Setup CA"; level: 3 }
                contentItem: Column {
                    // Plain Column (not ColumnLayout): child y-positions are
                    // derived directly from laid-out heights, so wrapped-text
                    // heights can never desync from positions (the AppImage /
                    // Fusion nav-path spill). parent is the card's inner
                    // padding item, so this tracks the real content width.
                    width: parent.width
                    spacing: 5
                    Controls.Label { wrapMode: Text.WordWrap; width: parent.width; text: "• <b>Files:</b> <code>~/certs/myCA.key</code> (AES-256, 600), <code>~/certs/myCA.pem</code>, <code>~/certs/.ca_passphrase</code> (600, used via <code>-passin file:</code> so passphrase never hits argv)<br>• <b>Commands:</b> <code>openssl genrsa -aes256 -passout file:… 2048</code> → <code>openssl req -x509 -sha256 -days N -passin file: -subj \"/C=…/CN=…\"</code><br>• <b>Overwrite:</b> allowed — if system CA missing, app shows <b>1 — Setup</b> on start and Setup overwrites <code>myCA.*</code> in place." }
                    Controls.Label { wrapMode: Text.WordWrap; width: parent.width; opacity: 0.6; font.pointSize: 8; text: "Tip: Common Name (CN) is what you’ll see as issuer — e.g. “DuckNet Dev CA”. Validity 1825d ≈5y, key 2048 is the modern default (4096 slower). Overwrite is safe because you control the CA." }
                    Kirigami.Separator { width: parent.width }
                }
            }

            Kirigami.Card {
                Layout.fillWidth: true
                header: Kirigami.Heading { text: "2 — Install Root CA"; level: 3 }
                contentItem: Column {
                    // Plain Column (not ColumnLayout): child y-positions are
                    // derived directly from laid-out heights, so wrapped-text
                    // heights can never desync from positions (the AppImage /
                    // Fusion nav-path spill). parent is the card's inner
                    // padding item, so this tracks the real content width.
                    width: parent.width
                    spacing: 5
                    Controls.Label { wrapMode: Text.WordWrap; width: parent.width; text: "• <b>What:</b> copies <code>myCA.pem</code> into the system trust store — Debian: <code>/usr/local/share/ca-certificates/myCA.crt</code> (<code>.crt</code> required by <code>update-ca-certificates</code>); Fedora: <code>/etc/pki/ca-trust/source/anchors/myCA.crt</code> — then runs the matching trust update (<code>update-ca-certificates</code> / <code>update-ca-trust</code>) via pkexec, or passwordless sudo if you enabled it on the Setup Tooling page.<br>• <b>Effect:</b> system store + Chrome trust any cert you sign — no more <code>NET::ERR_CERT_AUTHORITY_INVALID</code>. Firefox is handled separately (see below).<br>• <b>Check:</b> <code>ls /usr/local/share/ca-certificates/myCA.crt</code> (Debian) or <code>ls /etc/pki/ca-trust/source/anchors/myCA.crt</code> (Fedora), plus <code>openssl x509 -noout -subject -in ~/certs/myCA.pem</code>" }
                    Controls.Label { wrapMode: Text.WordWrap; width: parent.width; opacity: 0.6; font.pointSize: 8; text: "App start checks the distro-appropriate path; if missing it shows 1 — Setup again. Re-install after re-creating CA. Firefox profiles are installed automatically (all <code>*.default*</code> profiles via <code>certutil</code>) — no manual import needed." }
                    Kirigami.Separator { width: parent.width }
                }
            }

            Kirigami.Card {
                Layout.fillWidth: true
                header: Kirigami.Heading { text: "3 — Generate Domain Cert"; level: 3 }
                contentItem: Column {
                    // Plain Column (not ColumnLayout): child y-positions are
                    // derived directly from laid-out heights, so wrapped-text
                    // heights can never desync from positions (the AppImage /
                    // Fusion nav-path spill). parent is the card's inner
                    // padding item, so this tracks the real content width.
                    width: parent.width
                    spacing: 5
                    Controls.Label { wrapMode: Text.WordWrap; width: parent.width; text: "• <b>Files per domain:</b> <code>~/certs/&lt;domain&gt;.key/.csr/.crt/.ext</code> + <code>myCA.srl</code><br>• <b>Commands:</b> <code>genrsa 2048 → req -new -subj \"/CN=&lt;domain&gt;\" → x509 -req -CA myCA.pem -CAkey myCA.key -CAcreateserial -extfile &lt;domain&gt;.ext -passin file:</code><br>• <b>SAN:</b> auto-adds primary domain + each entry in SAN list + optional wildcard (<code>*.test</code> and <code>IP.1 = 192.168.1.10</code> detected).<br>• <b>Validity:</b> 825d default (Apple limit). Wildcard checked → generates <code>DNS.2 = *.&lt;domain&gt;</code>." }
                    Controls.Label { wrapMode: Text.WordWrap; width: parent.width; opacity: 0.6; font.pointSize: 8; text: "Example SAN .ext checked via <code>openssl x509 -noout -text -in ~/certs/myapp.test.crt | grep -A2 \"Subject Alternative Name\"</code>" }
                    Kirigami.Separator { width: parent.width }
                }
            }

            Kirigami.Card {
                Layout.fillWidth: true
                header: Kirigami.Heading { text: "Using for nginx — .local domain"; level: 3 }
                contentItem: Column {
                    // Plain Column (not ColumnLayout): child y-positions are
                    // derived directly from laid-out heights, so wrapped-text
                    // heights can never desync from positions (the AppImage /
                    // Fusion nav-path spill). parent is the card's inner
                    // padding item, so this tracks the real content width.
                    width: parent.width
                    spacing: 5
                    Controls.Label {
                        wrapMode: Text.WordWrap; width: parent.width
                        text: "The <b>Nginx Manager</b> page automates this whole flow — prefer it over hand-editing files. It conforms nginx to a <code>sites-available</code>/<code>sites-enabled</code> layout, runs nginx (and PHP-FPM) as your dev user so no <code>www-data</code> permission tweaks are needed, and every save is gated by <code>nginx -t</code> with automatic revert + reload.<br>"
                            + "1) <b>/etc/hosts:</b> the app already maps each generated domain to <code>127.0.0.1</code> when the cert is created (and removes it on delete). <code>.local</code> is mDNS — Avahi will also claim it; <code>.test</code> per RFC 2606 is collision-free, but <code>.local</code> works for nginx with the hosts entry.<br>"
                            + "2) <b>Generate:</b> in app, Domain <code>myapp.local</code>, add SAN <code>api.myapp.local</code>, check Wildcard → creates <code>~/certs/myapp.local.{key,crt}</code> with <code>DNS: myapp.local, api.myapp.local, *.myapp.local</code>.<br>"
                            + "3) <b>Quick Create</b> (Nginx Manager): builds <code>sites-available/myapp.local.conf</code> (HTTP→HTTPS redirect + 443 block pointing at <code>~/certs/myapp.local.{crt,key}</code>), enables it, creates <code>~/WebRoots/myapp.local/index.html</code>, then tests + reloads. <b>Edit Site Config</b> opens the full-page editor with live lint instead of relying on save-time <code>nginx -t</code>.<br>"
                            + "4) <b>Permissions:</b> none needed — nginx runs as your dev user (<b>Run NGINX as DevUser</b>), so <code>~/WebRoots</code> and <code>~/certs</code> are readable as-is (<code>chmod 644</code> certs, <code>600</code> keys).<br>"
                            + "5) <b>Trust:</b> ensure the CA is installed (step 2 above) — then <code>curl https://myapp.local --cacert ~/certs/myCA.pem</code> and Chrome/Firefox show 🔒 without import.<br>"
                            + "6) <b>Test:</b> the app runs <code>nginx -t</code> on every save/reload; manually: <code>sudo nginx -t && sudo systemctl reload nginx; curl -vk https://myapp.local/</code> — look for <code>issuer: CN = DuckNet Dev CA</code>."
                    }
                    Kirigami.Separator { width: parent.width }
                }
            }

            Kirigami.Card {
                Layout.fillWidth: true
                header: Kirigami.Heading { text: "Tips & troubleshooting"; level: 3 }
                contentItem: Column {
                    // Plain Column (not ColumnLayout): child y-positions are
                    // derived directly from laid-out heights, so wrapped-text
                    // heights can never desync from positions (the AppImage /
                    // Fusion nav-path spill). parent is the card's inner
                    // padding item, so this tracks the real content width.
                    width: parent.width
                    spacing: 5
                    Controls.Label { wrapMode: Text.WordWrap; width: parent.width; opacity: 0.8; text: "• <b>.local vs .test:</b> <code>.local</code> is mDNS (Avahi/Bonjour) — may conflict on LAN; <code>.test</code>/<code>.example</code> never leaves the machine. Prefer <code>.test</code> for pure dev, <code>.local</code> only if you need mDNS discovery.<br>• <b>Browser still warns?</b> The app installs the CA into every Firefox profile it finds, plus the system store for Chrome/Edge. If a warning persists: restart the browser, or re-run step 2 (a re-created CA must be re-installed everywhere).<br>• <b>Chrome/Edge:</b> uses the system store — re-run step 2 and restart the browser.<br>• <b>Regenerate after CA overwrite:</b> old domain certs are still signed by the old CA — delete <code>~/certs/*.crt</code> or re-generate.<br>• <b>Files:</b> <code>ls -l ~/certs/</code> — <code>.ca_passphrase</code> 600, <code>myCA.key</code> 600, <code>*.crt</code> 644, plus <code>bundle.p12</code> (CA key+cert bundle) and an encrypted passphrase backup at <code>~/.local/state/devbox.arc</code>." }
                    Kirigami.Separator { width: parent.width }
                }
            }

            Kirigami.Card {
                Layout.fillWidth: true
                header: Kirigami.Heading { text: "Reference"; level: 3 }
                contentItem: Column {
                    // Plain Column (not ColumnLayout): child y-positions are
                    // derived directly from laid-out heights, so wrapped-text
                    // heights can never desync from positions (the AppImage /
                    // Fusion nav-path spill). parent is the card's inner
                    // padding item, so this tracks the real content width.
                    width: parent.width
                    spacing: 5
                    Controls.Label {
                        wrapMode: Text.WordWrap; width: parent.width; opacity: 0.5; font.pointSize: 8
                        textFormat: Text.RichText
                        onLinkActivated: function(link) { Qt.openUrlExternally(link) }
                        text: "Ref: deliciousbrains.com/ssl-certificate-authority-for-local-https-development — local CA pattern. OpenSSL 3.5.7 on this host uses <code>-aes256</code> (modern) instead of deprecated <code>-des3</code>.<br>Ref: <a href=\"https://github.com/walf443/nginx-lint\">github.com/walf443/nginx-lint</a> — A linter for nginx configuration files. Implemented the lint function for our Site Configuration file Editor."
                    }
                    Kirigami.Separator { width: parent.width }
                }
            }
        }
    }
}
