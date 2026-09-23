# DuckNet Dev Tool

A native **all-Rust + Qt6/QML** desktop application for **KDE Plasma** on **Debian** and **Fedora** that automates local-development environment setup tasks.

Current version: **0.1.0** · License: **MIT** · QML module: `org.kde.ducknetdevtool`

Shipped functions (left navigation in `Main.qml`):

| Nav entry | Page | Backend | What it does |
|---|---|---|---|
| **Welcome** | `Main.qml` (inline) | `AppInfo` (`appinfo.rs`) | App name / version / build stamp |
| **Setup Tooling** | `SetupToolingPage.qml` | `ToolingManager` (`tooling.rs`) | Prerequisites (+ dev folders), passwordless sudo/pkexec, NGINX+PHP install, dev toolchain install, dev places, optional tools |
| **DEV HTTPS Certs** | `DevCertsPage.qml` | `DevCertsManager` (`devcerts.rs`) | Local CA + per-domain HTTPS certs trusted by Chrome/Firefox (workflow based on <https://deliciousbrains.com/ssl-certificate-authority-for-local-https-development/>) |
| **Nginx Manager** | `NginxManagerPage.qml` | `NginxManager` (`nginxman.rs`, lint via `nginxlint.rs`) | NGINX/PHP-FPM status & control, base-config and per-site management, log viewers |
| **Database Manager** | `DatabaseManagerPage.qml` | `DatabaseManager` (`dbman.rs`) | One-click PostgreSQL / MariaDB installs with LED status + live log dialog |
| **System Info** | `SystemInfoPage.qml` | `SystemInfoManager` (`sysinfo.rs`) | Live host environment details |
| **Dev Tool Help** | `CertHelpPage.qml` | — | Docs: commands, nginx recipes, troubleshooting |

Non-Debian/Fedora systems show `UnsupportedPage.qml` (detected from `/etc/os-release`).

The app is designed as a **multi-function shell**: new tools are added as one Rust backend + one QML page + build registrations. Modularity rule (see §16): one function ⇒ one `src/*.rs` + one `src/qml/*.qml` + entries in `build.rs` / `CMakeLists.txt` / `build-appimage.sh`. Shared QML lives in components like `EditorFindBar.qml`; shared Rust lives in `src/common.rs`.

---

## Table of contents

1. [Features](#1-features)
2. [Tech stack](#2-tech-stack)
3. [Repository layout](#3-repository-layout)
4. [Architecture](#4-architecture)
5. [Setup Tooling in detail](#5-setup-tooling-in-detail)
6. [DEV HTTPS Certs in detail](#6-dev-https-certs-in-detail)
7. [Nginx Manager in detail](#7-nginx-manager-in-detail)
8. [Data locations & system changes](#8-data-locations--system-changes)
9. [Requirements](#9-requirements)
10. [Build & run](#10-build--run)
11. [Install / packaging](#11-install--packaging)
12. [Usage walkthrough](#12-usage-walkthrough)
13. [Security model](#13-security-model)
14. [Troubleshooting & logs](#14-troubleshooting--logs)
15. [Testing](#15-testing)
16. [Adding a new function](#16-adding-a-new-function)
17. [Known limitations](#17-known-limitations)
18. [License](#18-license)

---

## 1. Features

### Setup Tooling (`SetupToolingPage.qml` + `tooling.rs`)

LED-gated setup flow for turning a fresh Debian 13 / Fedora install into a dev machine. See §5.

- **Prerequisite tooling** (`Activate Prerequisites`): `pkexec` + `libnss3-tools` on Debian; `nss-tools` + `SELINUX=disabled` on Fedora — installed in a visible terminal window. Also ensures the dev folders below (best-effort, same helper as the button).
- **Sudo-group enrollment** (Debian fresh installs): stages `/tmp/ducknet-fixsudo.sh`, prompts for the **root** password via `su`, adds the dev user to `sudo`, reboots. Skipped when already a member (probed + script-level guard).
- **Passwordless sudo (console):** staged `/etc/sudoers.d/<user>` (`0440`, `root:root`, `visudo -c` gate).
- **Passwordless pkexec (Plasma):** polkit rule (`49-sudo-nopassword.rules` on Debian / `49-wheel-nopassword.rules` on Fedora) + `polkit` restart. Auto-installs the `pkexec` package on Debian if missing.
- **Install / Uninstall NGINX + PHP** (8.4): background operation with live log dialog; Fedora pulls PHP 8.4 via the Remi repo + `php:remi-8.4` module stream, and is conformed to the Debian `sites-available`/`sites-enabled` layout as part of the install (mandatory — the old Conform button is now an LED-only check on the Nginx page); services are rewritten to run as the dev user, with a user-owned PHP session dir (`~/WebRoots/.php-session`, `700`) and 512M memory limits (pool + `php.ini`).
- **Install Dev Env:** C/C++/Python/build toolchain + `rustup` (installed as the dev user, never root).
- **Developer Places/Folders & Optional tools:** `Add Dev Folders` creates `~/Coding`, `~/MyApps`, `~/WebRoots` plus Dolphin Places bookmarks (LED dims when all three hrefs exist; folders also ride along with Activate Prerequisites, the button stays for hand-built systems). `Install VSCodium` adds the upstream repo and installs `codium` per distro; `Install Opencode` runs the universal installer as the dev user. Both use the same background-op progress dialog as the NGINX+PHP install.

### DEV HTTPS Certs (fully implemented)

A 3-step guided workflow exposed in `src/qml/DevCertsPage.qml`, backed by `src/devcerts.rs`:

| Step | UI state (`caSetupStatus`) | What happens |
|------|----------------------------|--------------|
| **1 — Setup CA** | `NotSetup` | Generates AES-256-encrypted `~/certs/myCA.key` + self-signed `~/certs/myCA.pem` via OpenSSL. Stores the passphrase in `~/certs/.ca_passphrase` (mode `600`) and an obfuscated copy in `~/.local/state/devbox.arc`. |
| **2 — Install Root CA** | `Setup` | Copies `myCA.pem` to the distro trust anchor and runs the distro trust updater via `pkexec` (fallback: passwordless `sudo -n`). Status becomes `Installed`. |
| **3 — Generate / manage** | `Installed` | Issues per-domain certs (`~/certs/<domain>.key/.csr/.crt/.ext`) with SAN + optional wildcard, appends `127.0.0.1 <domain>` to `/etc/hosts`, lists/deletes certs, installs/removes the CA in Chrome (`~/.pki/nssdb`) and Firefox (profile NSSDB), and can **Remove Root CA & Regenerate** (full cleanup back to step 1). |

Additional supporting UI:

- **System Info** (`SystemInfoPage.qml` + `sysinfo.rs`): live host details (hostname, platform, memory, CPU, disks) with timed refresh.
- **Certificate Help** (`CertHelpPage.qml`): what the tool does, exact OpenSSL commands, nginx `.local`/`.test` recipe, tips & troubleshooting.
- **UnsupportedPage** (`UnsupportedPage.qml`): shown automatically on non-Debian/Fedora systems.
- **Welcome** (`Main.qml` inline page + `AppInfo`): name, `Cargo.toml` version, `DUCKNET_BUILD_NUMBER` stamp.

### Nginx Manager (`NginxManagerPage.qml` + `nginxman.rs`)

Full lifecycle management for local NGINX development servers. See §7.

- **Status & control:** live NGINX + PHP-FPM status/workers (5 s poller), Start/Stop/Restart for both services.
- **Conform + identity:** Fedora installs are conformed to Debian `sites-available`/`sites-enabled` by Setup Tooling (backup + `.orig`, dirs, `default` site, rewritten `nginx.conf`, symlink, `nginx -t`); the CONFORMED LED verifies it took effect (the `conformNginx` backend stays as a manual repair path). Run NGINX / PHP-FPM as the dev user (no `~/WebRoots` permission tweaks).
- **Base configs:** timestamped backups under `~/.local/backups`, revert-guarded editors (`nginx -t` / `php-fpm -t` gate, auto-revert on failure).
- **Sites:** quick-create from generated certs (**localhost-only template**: `listen 127.0.0.1:80` / `127.0.0.1:443 ssl`), revert-guarded editor with live lint (`nginxlint.rs`), per-site backup/restore, delete.
- **Log viewers:** popout dialogs (500-line cap) for the global NGINX error log, the PHP-FPM error log (path resolved from `php-fpm.conf`), and per-site error/access logs parsed from each site file's own directives.
- **Config editors** (`EditSitePage` / `EditNginxBasePage` / `EditPhpfpmBasePage`) share the `EditorFindBar.qml` incremental find bar (Ctrl+F, wraps, Enter = next, Esc = close).

### Database Manager (`DatabaseManagerPage.qml` + `dbman.rs`)

- **Install your Database:** `Install PostgreSQL` / `Install MariaDB` buttons on one row, each with its own green/red LED. LEDs use unprivileged `systemctl` checks (same read-only pattern as the Nginx page): MariaDB via `systemctl status mariadb.service`; PostgreSQL via `systemctl list-units … 'postgresql*'` because Debian runs versioned template instances (`postgresql@17-main.service` — the number varies per host) under an umbrella unit that only ever reaches `active (exited)`, so only a `running` sub-state lights the LED and the status names the instance. A unit that exists counts as installed, the status label carries the running detail (`active (running)` / `installed — inactive (dead)` / `not installed`).
- Scripts (distro stock versions, no third-party repos): Debian `postgresql postgresql-client postgresql-contrib` / `mariadb-server mariadb-client` (`apt-get update` first); Fedora `postgresql-server postgresql-contrib` (+ best-effort `postgresql-setup --initdb`) / `mariadb mariadb-server`; PostgreSQL installs switch `pg_hba.conf` to `scram-sha-256` (Debian path hardcodes the stock PG17 cluster; Fedora keeps a `local all postgres peer` line so the tool's own `runuser` calls keep working) with a `reload` so the rules bite; every install ends with `systemctl enable --now <unit>`.
- **Enable MariaDB (adopt a pre-install):** Fedora KDE ships `mariadb-server` out of the box (via `akonadi-server-mysql`, unit left `disabled`), so the installed LED can be green before Database Manager ever installs anything — none of the Setup Tooling buttons pull it in. When MariaDB is installed but not enabled on Fedora-family, an accent-highlighted `Enable MariaDB` button appears in the install row next to the (disabled) Install button and runs privileged `systemctl enable --now mariadb.service` (adopt + start in one click, same convention as the install path). The gate (`fedora && installed && !enabled`) is computed in Rust (`mariadb_enable_visible`, exposed as `mariadbEnableVisible`) from an unprivileged `systemctl is-enabled` probe, so the rule lives in exactly one place; the db status debug line logs `family=… enabled=… enable_visible=…` for diagnosis.
- Same background-op console method as the Setup Tooling package ops (shared `privileged_spawn` / `op_log_tail` in `common.rs`): progress dialog streams `/tmp/ducknet-db-op.log`, no cancel, LEDs re-probe on completion.
- **Database Services:** per-engine status bars (running LEDs + detail) with Start/Stop/Restart + Refresh, same layout and sequenced-restart behavior as the Nginx Manager page. Install LEDs mean *installed*; services LEDs mean *running*.
- **Create Database** (`CreateDatabasePage.qml`): full-page form (heading + three fields: Database Name / Database User / Password) with a PostgreSQL-left / MariaDB-right toggle switch — the selected side's label highlights; with a single engine installed the switch locks to it. `[Create]` runs that engine's procedure (PostgreSQL: `CREATE USER` + `CREATE DATABASE … OWNER … ENCODING 'UTF8' TEMPLATE template0` as the `postgres` superuser via `runuser`; MariaDB: `CREATE DATABASE … CHARACTER SET utf8 …` + `CREATE USER …@localhost` + `GRANT … ON db.*` + flush, as root via unix_socket auth — localhost-only users). A MariaDB-only tickbox switches the charset pair to `utf8mb4`/`utf8mb4_unicode_ci` for 4-byte characters/emoji. Strict identifier validation + SQL single-quote escaping; returns to Database Manager on success.
- **Current Databases:** per-engine lists governed by the installed LEDs (PostgreSQL excludes template + `postgres` maintenance DBs; MariaDB filters `information_schema`/`mysql`/`performance_schema`/`sys`), painted instantly from `~/.cache/ducknet-dev-tool/db-lists` on page open (silent — no prompt) with a manual Refresh plus auto-refresh after create/install/delete (best-effort — install-prompt auth is usually still cached). Each row carries **Backup DB** (superuser dump to `~/BACKUPDB/<engine>/<db>/<db>-<date>.sql`, no passwords stored — peer/unix-socket auth plus a non-secret owner/charset sidecar), **Restore DB** (newest-first dialog with per-file Restore/Delete; recreates the database when missing), **Change Owner Password** (two matching entries with reveal toggles; applies to the catalog owner / grant-holding user), and **Delete** (red two-stage confirm + type-the-name gate; wrong name cancels with "Database delete not accepted.").

### Distro awareness

`family()` in `src/common.rs` parses `/etc/os-release` (`ID` + `ID_LIKE`, case-insensitive, overrideable via `DUCKNET_OS_RELEASE` for tests):

| Family | Matches | Trust anchor | Update command |
|--------|---------|--------------|----------------|
| Debian | `debian`, `ubuntu`, `linuxmint`, `pop`, `kali`, `raspbian` | `/usr/local/share/ca-certificates/myCA.crt` | `update-ca-certificates` (`/usr/sbin/update-ca-certificates`) |
| Fedora | `fedora`, `rhel`, `centos`, `almalinux`, `rocky`, `ol` | `/etc/pki/ca-trust/source/anchors/myCA.crt` | `update-ca-trust` (`/usr/bin/update-ca-trust`) |
| Unsupported | anything else | — | — (install/remove blocked, `UnsupportedPage` shown) |

The cert startup status is derived from anchor existence: anchor present → `Installed`, CA files present but anchor missing → `Setup`, otherwise `NotSetup` (or `Unsupported`).

---

## 2. Tech stack

- **Language:** Rust 2021 edition (rustc ≥ 1.88 required by `cxx` 1.0.199 / `cxx-qt` 0.10).
- **Qt bindings:** [`cxx-qt` 0.10](https://github.com/KDAB/cxx-qt) + `cxx-qt-lib` (`qt_full`) + `cxx` 1.0. QML types registered via `#[qobject] #[qml_element]`: `DevCertsManager`, `ToolingManager`, `NginxManager`, `DatabaseManager`, `SystemInfoManager`, `AppInfo`.
- **UI:** Qt6 QML + **Kirigami** (`org.kde.kirigami`) `ApplicationWindow` / `Page`. Style `org.kde.desktop` on dev hosts; bundled default style inside the AppImage (see §4).
- **Parsing/data crates:** `nginx-lint-parser` (rowan CST, parser only — keeps wasmtime out of the AppImage) for live site-config lint; `sysinfo` for host details; `chrono` for backup stamps.
- **Build glue:** `build.rs` (`CxxQtBuilder::new_qml_module` for `org.kde.ducknetdevtool`, Dynamic plugin, 13 QML files) + plain-`cxx` bridge for the window icon (`src/appicon.{rs,cpp,h}` — `cxx-qt-lib` 0.10 has no `QIcon`) + `CMakeLists.txt` (ECM, `KDEInstallDirs`, `CxxQt` CMake integration with `FetchContent` fallback).
- **Crypto / system tools (host-provided, not bundled):** `openssl`, `pkexec` (polkit) / `sudo -n`, `certutil`/`pk12util` (`libnss3-tools` on Debian, `nss-tools` on Fedora), `update-ca-certificates` / `update-ca-trust`, `nginx`, PHP-FPM.
- **KDE integration files:** `org.kde.ducknetdevtool.desktop`, `org.kde.ducknetdevtool.json` (KPlugin metadata), `org.kde.ducknetdevtool.metainfo.xml` (AppStream), `org.kde.ducknetdevtool.svg` + `icons/hicolor/` PNGs.

> Note: `cxx-kde-frameworks` is intentionally **not** a dependency right now — upstream `main` still requires `cxx-qt` 0.8 and conflicts with this project's `cxx-qt` 0.10 (`links = "cxx-qt"` conflict). `src/main.rs` therefore uses a pure-Qt fallback. Re-enable when upstream supports 0.10 (see commented block in `Cargo.toml`).

---

## 3. Repository layout

```
.
├── Cargo.toml                  # crate ducknet-dev-tool 0.1.0, cxx-qt 0.10 deps
├── build.rs                    # QML module build (13 files), appicon bridge, qmldir fixups
├── CMakeLists.txt              # ECM/KDE install, CxxQt crate import, QML install + qmldir generation
├── src/
│   ├── main.rs                 # QGuiApplication, style, icon, QML import paths, Main.qml loading
│   ├── lib.rs                  # pub mods: common, appinfo, dbman, devcerts, nginxman, nginxlint, sysinfo, tooling
│   ├── common.rs               # shared helpers: distro detect, privileged exec, identity/validation,
│   │                           #   binary probing, background-op spawn/log-tail, log-viewer helpers,
│   │                           #   home-dir/Qt conversion/message-suffix helpers, conform templates
│   ├── tooling.rs              # ToolingManager backend (~1900 lines): sudo/pkexec, package ops, dev places, probes
│   ├── devcerts.rs             # DevCertsManager backend (~1700 lines, openssl/pkexec logic)
│   ├── nginxman.rs             # NginxManager backend (~2500 lines): services, conform, base/site configs, log viewers
│   ├── dbman.rs                # DatabaseManager backend (~1900 lines): probes, installs, pg_hba fix, create/delete,
│   │                           #   backups, passwords, db-list cache + bg op
│   ├── nginxlint.rs            # live site-config lint (nginx-lint-parser + hand-written checks)
│   ├── sysinfo.rs              # SystemInfoManager backend (~1000 lines, live host details)
│   ├── appinfo.rs              # AppInfo backend (compile-time name/version/build)
│   ├── appicon.rs / .cpp / .h  # qApp->setWindowIcon() bridge (duck logo)
│   ├── bin/live_test.rs        # manual end-to-end OpenSSL workflow test in ~/certs
│   └── qml/
│       ├── Main.qml            # Kirigami shell: left nav (Welcome/Tooling/Certs/Nginx/SysInfo/Help) + detail loader
│       ├── SetupToolingPage.qml# prerequisites / passwordless / package ops + progress + sudo/SELinux dialogs
│       ├── DevCertsPage.qml    # 3-step cert UI (setup / install / generate + Firefox + regenerate)
│       ├── NginxManagerPage.qml# status, conform, base configs, sites, log-viewer dialogs
│       ├── DatabaseManagerPage.qml # postgres/mariadb install buttons + Enable-adoption button + LEDs + op log dialog
│       ├── CreateDatabasePage.qml  # new-database form (engine toggle + name/user/password)
│       ├── EditSitePage.qml    # full-page site editor + live lint
│       ├── EditNginxBasePage.qml # full-page nginx.conf editor (no site-lint; nginx -t gate)
│       ├── EditPhpfpmBasePage.qml# full-page www.conf editor (php-fpm -t gate)
│       ├── EditorFindBar.qml   # shared incremental find bar (Ctrl+F) for the three editors
│       ├── SystemInfoPage.qml  # live host details view
│       ├── CertHelpPage.qml    # docs page: commands, nginx .local recipe
│       └── UnsupportedPage.qml # non-Debian/Fedora notice
├── tests/cert_integration.rs   # openssl AES-256 + SAN workflow test in temp dir
├── dev1.conf / dev2.conf / example_nextcloud.conf  # sample real-world site configs (reference)
├── org.kde.ducknetdevtool.desktop | .json | .metainfo.xml | .svg
├── icons/hicolor/...           # fixed-size PNGs for X11/Wayland taskbars
├── check-qml.sh                # qmllint over src/qml/*.qml (warnings advisory, errors fail; --format check)
├── build-appimage.sh           # linuxdeploy + plugin-qt AppImage build (Qt 6.8 bundled)
├── cleanup_imagebuild.sh       # AppDir/build artifact cleanup helper
└── build-appimage.log          # log from last AppImage build
```

(`repo/`, `third-party/`, `.flatpak-builder/` are build/packaging work areas, not sources.)

---

## 4. Architecture

### 4.1 Rust ↔ QML bridges

Each backend is a `#[qobject] #[qml_element]` exposed via the Dynamic plugin `liborg_kde_ducknetdevtool.so` (built from the `cdylib` `libducknet_dev_tool.so`):

| Backend (`src/`) | QML type | Owns |
|---|---|---|
| `tooling.rs` | `ToolingManager` | sudo/pkexec state, prerequisite + package operations (§5) |
| `dbman.rs` | `DatabaseManager` | postgres/mariadb probes + installs + enable-adoption + background op (§1, Database Manager) |
| `devcerts.rs` | `DevCertsManager` | CA + domain certs + browser trust (§6) |
| `nginxman.rs` | `NginxManager` | services, conform, base/site configs, log viewers (§7) |
| `sysinfo.rs` | `SystemInfoManager` | live host details (hostname, platform, CPU/mem/disk) |
| `appinfo.rs` | `AppInfo` | compile-time name / `CARGO_PKG_VERSION` / `DUCKNET_BUILD_NUMBER` |

`Default` on each manager probes the host so pages paint correct state on open. Status probes are **unprivileged and never prompt** (page open must stay silent); mutations go through `privileged_output` (`pkexec`, falling back to passwordless `sudo -n`), which logs exit code + stdout/stderr to `/tmp/ducknet-dev-tool.log`.

### 4.2 Shared helpers (`src/common.rs`)

Single implementation so backends cannot drift apart: `family()`/`distro_is_*` (+ `DUCKNET_OS_RELEASE` override), `privileged_output`, `dev_username`/`valid_username`/`valid_password`/`primary_group`/`service_group`, `home_dir_for`/`home_dir`, `valid_site_domain`/`valid_cert_domain`, `path_lookup`/`find_binary`/`binary_present` (PATH → well-known paths → `whereis` db), `NEEDS_ROOT` message suffix, `to_qstringlist`, the Fedora conform templates, and the log-viewer trio `LOG_TAIL_LINES` (500) / `tail_lines` / `read_log_capped` (direct read first, privileged `tail -n` fallback).

### 4.3 Application shell (`src/main.rs`, `src/qml/Main.qml`)

- Sets `org.kde.desktop` Quick style on dev hosts; inside an AppImage uses the bundled default style (host modules such as Fedora Qt 6.11 `/usr/lib64/qt6/qml/org/kde/desktop` are version-incompatible with the bundled Qt 6.8 and are deliberately **not** added to the import path — `in_appimage()` guard).
- Sets the duck logo as window/taskbar icon via `appicon::set_window_icon()` (searches AppImage mount → `/usr/share/icons` → cargo tree → theme fallback).
- Resolves `Main.qml` by probing installed locations (`../share/ducknet-dev-tool/qml`, `../lib*/qt6/qml/...`, `/usr/...`), then the cargo tree, then `qrc:` fallback.
- Left nav `functionModel` holds `welcome` / `tooling` / `devcerts` / `nginx` / `dbman` / `sysinfo` / `help` (+ `CertHelpPage` content); unsupported distros swap detail pages for `UnsupportedPage`.

### 4.4 Build glue (`build.rs`, `CMakeLists.txt`)

- `build.rs` registers the **13 QML files** in the `org.kde.ducknetdevtool` module (Dynamic plugin), compiles the `appicon` cxx bridge against `qmake6`-reported Qt headers, strips `prefer` lines from generated `qmldir` files (so file-based loading wins), and mirrors QML + plugin `.so` for `cargo run` (the mirror loop matches each registered component name — new components must be added to both the `.qml_file(...)` list and that filter).
- `CMakeLists.txt` finds ECM/Qt6 (`Core Gui Qml Quick QuickControls2 Svg`), warns (does not fail) if Kirigami/`KF6QQC2DesktopStyle` are absent, imports the crate via `cxx_qt_import_crate` (+ QML module) with a `corrosion_import_crate` fallback, installs binary, QML, desktop file, icons (SVG + PNGs), AppStream metadata, and generates the installed `qmldir` (explicit per-file list + `liborg_kde_ducknetdevtool.so` symlink handling).

---

## 5. Setup Tooling in detail

`ToolingManager` (`tooling.rs`) with the `PREREQUISITE TOOLING` / `PASSWORDLESS SUDO` / `PASSWORDLESS PKEXEC` LEDs on `SetupToolingPage.qml`. Page open only re-probes (unprivileged); every mutation prompts via terminal/`pkexec` as appropriate.

### 5.1 Prerequisite tooling (`Activate Prerequisites`)

| Distro | Packages (`prereqPackages` label) | Extra step |
|---|---|---|
| Debian-family | `pkexec libnss3-tools` (`certutil` for Firefox trust) | sudo-group gate first (fresh installs omit it) |
| Fedora-family | `nss-tools` | `SELINUX=disabled` in `/etc/selinux/config` + live `setenforce 0` |

- The green LED requires the **full gate**: `certutil` present everywhere, **plus `pkexec` installed on Debian** (checked via `dpkg -s pkexec`), **plus `SELINUX=disabled` on Fedora**. A present `certutil` alone must never dim the button while `pkexec` is still missing.
- Clicking opens the install in a terminal (`konsole` → `x-terminal-emulator` → `gnome-terminal` → `xfce4-terminal` → `xterm`) as `sudo sh -c '…'` (interactive password there — passwordless sudo is a *later* tool, so `-n` is never used). Fire-and-forget; the idle poller re-probes so the LED flips without a manual refresh. The dev folders (`~/Coding`, `~/MyApps`, `~/WebRoots` + Places bookmarks) ride along via the same helper as the `Add Dev Folders` button — best-effort, so a non-Plasma session never blocks the package install.
- **Reboot guards (idempotent):** the Fedora reboot variant re-checks `SELINUX=disabled` in the shell *and* downgrades to no-reboot in the backend when the config is already compliant (covers a manual disable between probe and click) — never reboot an already-compliant machine.

### 5.2 Sudo-group enrollment (Debian)

Fresh Debian installs leave the user outside `sudo`. `needsSudoGroup` diverts `Activate Prerequisites` to a confirm dialog; `addUserToSudoGroup()` stages `/tmp/ducknet-fixsudo.sh` (mode `700`), which **no-ops when `id -nG $USER` already contains `sudo`** (covers a manual `adduser`), otherwise prompts for the **root** password via `su`, runs `adduser <user> sudo` and reboots immediately (group membership needs a fresh login; reboot is the reliable path). The Rust side refuses early with `"<user> is already in the sudo group."`.

### 5.3 Passwordless sudo / pkexec

- **Sudo:** stages `/etc/sudoers.d/<user>` (`<user> ALL=(ALL:ALL) NOPASSWD:ALL`, `0440`, `root:root`), validated with `visudo -c` (removed on failure); disable removes the file and kills the cached timestamp (`sudo -n -K`) so it bites immediately.
- **Pkexec:** installs the distro rule (`/etc/polkit-1/rules.d/49-sudo-nopassword.rules` for group `sudo` on Debian, `49-wheel-nopassword.rules` for `wheel` on Fedora — local + active + in-group ⇒ `YES`), restarts `polkit`. On Debian the standalone `pkexec` package is installed first when missing. The LED also honors `pkcheck` (covers unreadable `rules.d` and equivalent third-party rules).

### 5.4 Install / Uninstall NGINX + PHP

Background `PkgOp` (apt/dnf run for minutes — never blocks the UI; the QML Timer polls `pollPackageOp` for state + last ~120 log lines from `/tmp/ducknet-pkg-op.log`; no cancel — killing apt/dnf mid-transaction risks lock mess).

- **Debian:** `apt-get update && apt-get install -y` the exact 19-package set (`nginx-full`, `php8.4*` incl. fpm/mbstring/intl/imagick/redis/soap/memcached/apcu/pgsql/pear). Uninstall removes exactly that set (`remove`, never `purge`).
- **Fedora:** Remi repo RPM → verify (`rpm -q`, enabled-repo + file greps) → `makecache` → `module reset/enable php:remi-8.4` → `dnf install` the set → enable+start `nginx` + `php-fpm`. Every verification grep stays in the `&&` chain so a broken repo fails honestly. Imagick must be requested as `php-pecl-imagick-im7`: that is the only build the `php:remi-8.4` stream ships — the generic `php-pecl-imagick` name resolves to Fedora's own 8.5-linked build (higher release wins the provides race) whose `php(api)` can never resolve once the 8.4 stream filters out every 8.5 `php-common`, and the `php84-php-*` SCL-style name exists in no Fedora/Remi repo at all (aborts the whole transaction with "Unable to find a match").
- **Both:** a post-install service-user rewrite (PHP-FPM pool `user`/`group` + socket ownership, nginx `user` directive → dev user/group), a user-owned PHP session dir (`~/WebRoots/.php-session`, created as root then `chown`ed back + `chmod 700` — the old ban was on `chown -R` of *system* paths, not user dirs), pool `session.save_path` + 512M `memory_limit` (replace-or-append, since stock pool files ship them commented/absent) plus 512M in the CLI/`php.ini`, then `daemon-reload` + restart. No systemd overrides.
- **Fedora conform (mandatory, was the Conform button):** runs inside the install right after `dnf install`, before the service-user rewrite so the backup captures pristine stock: timestamped `.bak-*` + stable `.orig` of `nginx.conf`, `sites-available`/`sites-enabled` dirs, `default` site (kept if present), rewritten `nginx.conf` from the shared `common.rs` template with the dev user baked in (byte-identical to the Nginx Manager repair path — single source, no drift), default symlink (dangling-safe), `nginx -t`. Idempotent; Debian already ships the layout and is never touched.

### 5.5 Install Dev Env

Privileged toolchain install, then `rustup` **as the dev user** (`sudo -u`, so it lands in `~/.cargo`, never `/root/.cargo`) in one child/one log. Debian: `build-essential`, python3/dev/pip/venv, clang/lld/lldb/clangd/format/tidy, cmake/ninja, gdb/valgrind, git, manpages, gcc/g++. Fedora: `@c-development` + `@development-tools` groups plus the matching packages.

### 5.6 Developer Places/Folders & Optional tools

- **Add Dev Folders:** creates `~/Coding`, `~/MyApps`, `~/WebRoots` and inserts all three Places bookmarks into `~/.local/share/user-places.xbel` immediately after the Desktop entry (dev username interpolated; `folder-script` / `folder-extension` / `folder-html` icons, IDs `1711111111–13`), then opens the file browser. The probe is href-based (both — all three — hrefs must exist), so the LED/button follows automatically, including after `Activate Prerequisites` runs. Already-present counts as success.
- **Install VSCodium:** adds the upstream `paulcarroty/vscodium-deb-rpm-repo` (Debian: dearmored keyring + deb822 `.sources`; Fedora: `.repo` overwrite, never append) then installs `codium`. Scripts run as root already, so no `sudo` prefix.
- **Install Opencode:** the universal `curl -fsSL https://opencode.ai/install | bash` pipe, run **as the dev user** (`sudo -u`) — a user-space install into `~/.opencode` that root would strand in `/root/.opencode` (same trap as rustup).

---

## 6. DEV HTTPS Certs in detail

Reference workflow: `CertHelpPage.qml` documents every command so the GUI never hides what it runs.

### Step 1 — Setup CA (`setupCA`)

1. `mkdir -p ~/certs`; write passphrase to `~/certs/.ca_passphrase` (mode `600`); store XOR+base64 copy in `~/.local/state/devbox.arc` (mode `600`, key = `/etc/machine-id`) for later browser installs.
2. `openssl genrsa -aes256 -out ~/certs/myCA.key -passout file:~/.ca_passphrase <keySize>` (default 2048; UI allows 1024–4096). Key chmod `600`.
3. `openssl req -x509 -new -nodes -key myCA.key -sha256 -days <N> -out ~/certs/myCA.pem -passin file:... -subj "/C=../ST=../L=../O=../OU=../CN=<cn>/emailAddress=.."` (empty subject fields omitted; defaults: `US / California / San Francisco / DuckNet / Dev / DuckNet Dev CA / dev@ducknet.test`, 1825 days).
4. Status → `Setup`. Overwrite is allowed: if the system anchor is missing the app returns to step 1 and Setup replaces `myCA.*` in place.

### Step 2 — Install Root CA (`installRootCert`)

Blocked on unsupported distros. Otherwise:

1. `pkexec /usr/bin/cp ~/certs/myCA.pem <anchor>` (bare-`cp` retry if polkit rejects the absolute path; then `sudo -n` fallback). Verifies the anchor exists afterwards.
2. `pkexec /usr/sbin/update-ca-certificates` (Debian) or `pkexec /usr/bin/update-ca-trust` (Fedora), with bare-name retry.
3. Status → `Installed`. Failure messages include actionable hints (container `no_new_privs`, cancelled polkit dialog, passwordless-sudo manual command).

### Step 3 — Domain certificates (`generateCert` / `deleteCert`)

`generateCert(domain, sans, wildcard, validityDays)` (default 825 days — Apple limit):

1. Validates domain (non-empty, no spaces/`/`).
2. `openssl genrsa -out ~/certs/<domain>.key 2048` → `openssl req -new -key ... -out <domain>.csr -subj "/CN=<domain>"`.
3. Builds `<domain>.ext` (`authorityKeyIdentifier`, `basicConstraints=CA:FALSE`, `keyUsage`, `subjectAltName=@alt_names` with `DNS.n`/`IP.n` auto-detection; primary domain always first; wildcard adds `*.<domain>`).
4. `openssl x509 -req -in <domain>.csr -CA myCA.pem -CAkey myCA.key -CAcreateserial -out <domain>.crt -days N -sha256 -extfile <domain>.ext -passin file:...`.
5. Appends `127.0.0.1 <domain>` to `/etc/hosts` via privileged `sh -c '... $1 ...' sh <domain>` (token-exact check first, so no duplicates or substring matches).
6. Refreshes `generatedCerts`.

`deleteCert(domain)`: path-traversal-guarded (rejects `/`, `\`, `..`; requires the domain in the generated list or an existing `.crt`), removes `.key/.csr/.crt/.ext`, removes `/etc/hosts` lines for the domain (both `127.0.0.1` and legacy `12.0.0.1` forms), refreshes the list.

### Browser trust (Chrome / Firefox)

- **Chrome** needs no extra step for system trust (it reads the system store), but `installToChrome`/`deleteFromChrome` manage `~/.pki/nssdb` via `certutil -A -d sql:... -n "My Local Development CA" -t "C,," -i myCA.pem` (with delete-and-re-add on `already exists`; verified with `certutil -L`).
- **Firefox** uses its own NSSDB: `installToFirefox`/`deleteFromFirefox` locate the profile via `profiles.ini` `Default=1` (searching `~/.mozilla/firefox`, `~/.config/mozilla/firefox` — Fedora default — and the Flatpak path, falling back to `*.default-release`/`*.default` scan), then run the same `certutil -A/-D` flow. The CA passphrase is recovered from `~/.local/state/devbox.arc` (re-run Setup CA if missing).
- Note: `bundle.p12`/`pk12util` is deliberately **skipped** for the CA — importing a `.p12` lands in *Your certificates*, not *Authorities*; only `certutil -A -t "C,,"` achieves CA trust.

### Regenerate (`removeRootCert`)

Confirm-dialog-gated. Removes the system anchor + trust update, **all** `~/certs` domain certs + their `/etc/hosts` entries, the CA key/cert/serial/passphrase/`bundle.p12`, the encrypted `devbox.arc` copy, and the Firefox CA — then returns to `NotSetup` with a summary message (e.g. `CA removed with 2 domain certificate(s) ...`).

---

## 7. Nginx Manager in detail

`NginxManager` (`nginxman.rs`, ~2600 lines) + live lint (`nginxlint.rs`). Status probes are unprivileged; mutations go through `privileged_output`.

### 7.1 Status & control

- Status bars for `nginx.service` and the distro PHP-FPM unit (`php8.4-fpm.service` on Debian, `php-fpm.service` on Fedora — resolved from the pool-file probe). Start/Stop/Restart buttons per service; a 5 s QML Timer calls `refreshLive()` (running + workers only, no message churn — workers fluctuate across reloads). Restarts are sequenced through a short timer so the "requested" toast paints before the blocking call.

### 7.2 Conform + service identity

- **Conform NGINX:** applied automatically by Setup Tooling on Fedora installs (see §5.4) — timestamped backup + stable `.orig`, dirs, `default` site, rewritten `nginx.conf` with the dev user baked in, symlink, `nginx -t` + reload. Idempotent — existing default site/symlink/`.orig` are never overwritten. The CONFORMED LED (`sites-available` exists) verifies it took effect; the `conformNginx` backend stays as a manual repair path (no button).
- **Run NGINX / PHP-FPM as DevUser:** rewrites the `user` directive / pool `user`+`group` to the invoking developer (same paths Setup Tooling uses), gated by `nginx -t` / `<fpm> -t` with backup restore, then reload/restart. `~/WebRoots` then needs no permission tweaks.

### 7.3 Base configs (backup → edit → revert guard)

`nginx.conf` and the distro `www.conf`, with LEDs, backup lists (most recent first), `Browse Backups` (`xdg-open ~/.local/backups`), per-file Restore/Delete:

1. Load stages a pristine `/tmp` copy; Save commits via privileged `cp`, then gates on `nginx -t` / `<fpm> -t`.
2. On test failure the pristine copy is restored and the user is told the edit was reverted — a typo can never crash the service.
3. Backups live in `~/.local/backups/nginx|php-fpm` as `<prefix>-DD-MM-YYYY-HH-MM-SS.conf.bak` (mirrors the `backupnginx.sh`/`backupphpfpm-*` naming).

### 7.4 Sites

Domains come from generated certs (`list_generated_certs` — same source as page 3). Per site: **Quick Create** (webroot `~/WebRoots/<domain>` + `index.html` unprivileged; `sites-available/<domain>.conf` + symlink privileged; `nginx -t` gate with rollback; **no auto-reload** — the user applies it with Restart), **Edit Site Config** (full-page editor + live lint, debounced 400 ms; save path identical to base configs), **Delete Site** (symlink + file, then webroot, then test + reload), per-site **Backup/Restore** under `~/.local/backups/sites/<domain>`.

**Localhost-only template:** quick-created sites listen on `127.0.0.1:80` / `127.0.0.1:443 ssl` (never `0.0.0.0`) with per-domain `access_log`/`error_log` pairs (plain + `.ssl`), modern TLS only. (The repo-root `dev1.conf`/`dev2.conf`/`example_nextcloud.conf` are real-world reference samples, not templates.)

**Live lint** (`nginxlint.rs`, `lintSiteDraft` — pure, safe per keystroke): `nginx-lint-parser` syntax layer (rowan CST, error offsets → line:col) + hand-written site checks (missing/duplicate `listen`, missing `server_name`, `location` outside `server`, `root` inside `location`). Output is `L<line>:<col> [error|warn] msg` lines (capped at 30); `nginx -t` remains the gate at save.

### 7.5 Log viewers

Popout dialogs with persistent scrollbars, readonly monospace views and Refresh (all capped at the last **500 lines** via `common.rs`: direct read first, privileged `tail -n` fallback — root-owned logs prompt via pkexec/sudo):

| Button | Source |
|---|---|
| `View Error Log` (next to `Edit NGINX`) | `/var/log/nginx/error.log` |
| `View Error Log` (next to `Edit PHP-FPM`) | `error_log` directive from the active `php-fpm.conf` (`/etc/php/8.4/fpm/php-fpm.conf` → `/etc/php-fpm.conf`), else `/var/log/php-fpm/error.log` → `/var/log/php8.4-fpm.log` |
| `Error Log` / `Access Log` (per site, right of `Restore`) | every absolute `error_log` / `access_log` path parsed from that site's own config (plain + ssl joined under `==> path <==` headers; unreadable files become inline notices) |

Per-site buttons are **dimmed unless the site file directly reads *and* carries that directive** (`siteHasSiteLog` — unprivileged probe, never prompts), so customised configs without the line simply disable their button.

### 7.6 Config editors & find

`EditSitePage` / `EditNginxBasePage` / `EditPhpfpmBasePage` are full-page dark editors (fixed colors — the AppImage skips the `org.kde.desktop` style). All three embed the shared **`EditorFindBar.qml`**: `editor:` property + `open()`/`close()`/`refresh()`/`step()`; Ctrl+F (or the toolbar `Find` button) opens it, pre-fills the current selection, incremental case-insensitive search, cursor jumps to each match (wraps, `N of M` counter), Enter = next, ↑/↓ buttons, Esc = close. Registered like any page component: `build.rs` `.qml_file(...)` + mirror filter, `CMakeLists.txt` qmldir list, `build-appimage.sh` verify loop (`check-qml.sh` and the CMake `src/qml/` install glob pick it up automatically).

---

## 8. Data locations & system changes

| Path | Purpose |
|---|---|
| `~/certs/myCA.key` (`600`) | AES-256 CA private key |
| `~/certs/myCA.pem` | Self-signed CA certificate |
| `~/certs/.ca_passphrase` (`600`) | CA passphrase (used via `-passin file:` / `-passout file:`) |
| `~/certs/<domain>.key/.csr/.crt/.ext` | Per-domain material |
| `~/certs/myCA.srl` | OpenSSL CA serial (from `-CAcreateserial`) |
| `~/.local/state/devbox.arc` (`600`) | XOR+base64 passphrase copy (key = `/etc/machine-id`) |
| `/usr/local/share/ca-certificates/myCA.crt` (Debian) | System trust anchor |
| `/etc/pki/ca-trust/source/anchors/myCA.crt` (Fedora) | System trust anchor |
| `/etc/hosts` | `127.0.0.1 <domain>` entries (added/removed by the app) |
| `~/.pki/nssdb` | Chrome/Chromium NSSDB |
| `~/.mozilla/firefox`, `~/.config/mozilla/firefox` (+ Flatpak path) | Firefox profiles / NSSDBs |
| `/etc/nginx/nginx.conf`, `/etc/nginx/sites-available|sites-enabled/` | NGINX config (conformed layout; backups kept as `.bak-*`/`.orig`) |
| `/etc/php/8.4/fpm/pool.d/www.conf` (Debian) / `/etc/php-fpm.d/www.conf` (Fedora) | PHP-FPM pool (user/group rewritten to dev user) |
| `~/.local/backups/nginx\|php-fpm\|sites/<domain>` | Timestamped `.conf.bak` backups (user-owned, most recent first) |
| `~/WebRoots/<domain>` | Per-site webroots (`755`, `index.html` `644`) |
| `~/WebRoots/.php-session` (`700`, dev-owned) | PHP session dir (pool `session.save_path` points here) |
| `~/Coding`, `~/MyApps`, `~/WebRoots` | Standard dev folders (Setup Tooling prerequisites/button) |
| `~/.local/share/user-places.xbel` | Dolphin Places bookmarks for the dev folders (after Desktop) |
| `~/BACKUPDB/<engine>/<db>/` | Per-database dumps (`<db>-<date>.sql`) + non-secret `meta` sidecar (owner/charset) |
| `~/.cache/ducknet-dev-tool/db-lists` | Last-known database lists (silent page-open paint; re-validated on load) |
| `/etc/postgresql/17/main/pg_hba.conf` (Debian) / `/var/lib/pgsql/data/pg_hba.conf` (Fedora) | `scram-sha-256` auth (Debian path hardcodes the stock PG17 cluster; Fedora keeps `local all postgres peer`) |
| `/etc/php/8.4/cli/php.ini` (Debian) / `/etc/php.ini` (Fedora) | `memory_limit = 512M` (install step) |
| `/var/log/nginx/error.log`, per-site `*.access/error.log` | Viewed read-only (tail-capped) by the log viewers |
| `/var/log/php-fpm/error.log` (or `error_log` from `php-fpm.conf`) | Viewed read-only by the PHP-FPM viewer |
| `/etc/sudoers.d/<user>` (`0440 root:root`) | Passwordless sudo entry (visudo-validated) |
| `/etc/polkit-1/rules.d/49-*-nopassword.rules` | Passwordless pkexec rule (removed on disable) |
| `/etc/selinux/config` | `SELINUX=disabled` on Fedora (prerequisite) |
| `/tmp/ducknet-*.sh`, `/tmp/ducknet-site-*`, `/tmp/ducknet-base-*`, `/tmp/ducknet-pkg-op.log` | Staged scripts / editor revert copies / package log (transient) |
| `/tmp/ducknet-dev-tool.log` | Persistent debug log (every backend action is logged here + stderr/stdout) |

---

## 9. Requirements

- **OS:** Debian-family (Debian, Ubuntu, Mint, Pop!_OS, Kali, Raspbian) or Fedora-family (Fedora, RHEL, CentOS, Alma, Rocky, Oracle). Others run but show `UnsupportedPage`.
- **Rust:** ≥ 1.88 (via rustup; Debian 13 ships 1.85 — `build-appimage.sh` auto-detects and either switches to rustup or pins `cxx` to 1.0.130).
- **Qt6/KDE dev packages:**
  - Debian: `qt6-base-dev qt6-tools-dev libqt6svg6-dev qml6-module-qtquick qml6-module-org-kde-kirigami extra-cmake-modules libkf6qqc2desktopstyle-dev`
  - Fedora: `qt6-qtbase-devel qt6-qtdeclarative-devel qt6-qtsvg-devel kf6-kirigami extra-cmake-modules kf6-qqc2-desktop-style`
- **Runtime tools (host):** `openssl`, `pkexec` (polkit) and/or passwordless `sudo`, `certutil` (`apt install libnss3-tools` / `dnf install nss-tools` — or one click in Setup Tooling), `update-ca-certificates` / `update-ca-trust`, `nginx` + PHP-FPM (one click in Setup Tooling), C++ compiler (`build-essential` / `gcc-c++`), `cmake`, `ninja`, `pkg-config`.
- **Fedora note:** SELinux must be `disabled` (Setup Tooling does this + reboots); the cert/nginx tooling does not work under enforcing.
- Optional for packaging: `linuxdeploy` + `linuxdeploy-plugin-qt` + `appimagetool` (auto-downloaded by `build-appimage.sh` into `.appimage-tools/`), `dpkg-deb` for `--deb`.

---

## 10. Build & run

```bash
# Development run (needs system Kirigami for org.kde.desktop style)
cargo run

# Release binary
cargo build --release
./target/release/ducknet-dev-tool

# Via CMake (also installs QML module / desktop / icons)
cmake -B /tmp/duck-build -S . -DCMAKE_BUILD_TYPE=Release
cmake --build /tmp/duck-build -j"$(nproc)"
DESTDIR=/tmp/duck-install cmake --install /tmp/duck-build

# Quality gates (also enforced fail-fast by build-appimage.sh)
cargo test --lib        # Rust backend unit tests (fast, no prompts)
./check-qml.sh          # qmllint over src/qml/*.qml (warnings advisory, errors fail)
./check-qml.sh --format # qmlformat cleanliness check
```

`cargo run` on a dev host resolves QML through `CARGO_MANIFEST_DIR`, `target/debug/build/*/out/qt-build-utils/qml_modules`, and system paths (`/usr/lib*/qt6/qml`, `/usr/lib/qt6/qml`, ...). No environment variables are required.

---

## 11. Install / packaging

> Note: there is no `build-installer.sh` — tarball/`.deb` packaging is done via the CMake install tree (`DESTDIR=… cmake --install …`, optionally wrapped with `dpkg-deb`).

### `build-appimage.sh` — portable AppImage

```bash
./build-appimage.sh
# → DuckNet-Dev-Tool-<version>-<arch>.AppImage  (full log: build-appimage.log)
```

Fail-fast gates first: `check-qml.sh` (errors only) then `cargo test --lib`. Configures with `CMAKE_INSTALL_PREFIX=/usr`, `CMAKE_INSTALL_LIBDIR=lib`, installs to `AppDir`, then runs `linuxdeploy --plugin qt` (bundles Qt 6.8 + Kirigami + QQC2DesktopStyle; `QML_SOURCES_PATHS=src/qml` picks up every QML file by directory scan), manually ensures Wayland platform plugins + `libQt6WaylandClient`/`libwayland-*` (needed on Fedora Wayland sessions), deploys `liborg_kde_ducknetdevtool.so` next to each `qmldir`, strips `prefer :/qt/qml/...` lines (the binary has no qrc resources), and mirrors the module under `usr/qml` (matches `qt.conf Imports=qml`).

UI-content gate (fail the build, not a warning): the bundled `DatabaseManagerPage.qml` is grepped for the current backend bindings (`mariadbEnableVisible`, `enableMariadb`). The backend can log healthy values while a stale page bundled here silently renders no button with zero errors — that combination must never ship.

QML inclusion audit (dir scans are automatic; explicit lists must name new files):

| Inclusion path | Mode |
|---|---|
| `CMakeLists.txt` `install(DIRECTORY src/qml/ … *.qml)` | glob — automatic |
| `QML_SOURCES_PATHS=src/qml` (plugin-qt scan) | dir scan — automatic |
| `check-qml.sh` (`ls src/qml/*.qml`) | glob — automatic |
| `build.rs` `.qml_file(...)` (cxx-qt module) | explicit — add the file |
| `build.rs` `fix_qmldir_files` mirror filter | explicit — add the `Name ` prefix |
| `CMakeLists.txt` generated `qmldir` content | explicit — add `Name 1.0 src/qml/Name.qml` |
| `build-appimage.sh` QML verify loop | explicit — add the filename (warns if missing); backend↔page binding markers (fails the build if missing, e.g. `mariadbEnableVisible`) |

Without the `build.rs` entries a new component silently fails to resolve under `cargo run` and is absent from the deployed `qmldir` (see `EditorFindBar.qml`, which is registered in all four).

AppImage runtime notes:

- Host trust stores are directly visible (no sandbox): distro detection and `pkexec cp` + trust update operate on the **host**.
- Host Qt QML paths are skipped and `org.kde.desktop` style is not forced (avoids Fedora Qt 6.11 vs bundled Qt 6.8 `uses incompatible Qt library` failures).
- Still requires host `openssl`, `pkexec`/polkit, `certutil`, `nginx`/PHP-FPM, and `update-ca-*`.

### Installed files (CMake / .deb)

Binary → `bin/ducknet-dev-tool`; QML → `<qml>/org/kde/ducknetdevtool/src/qml/*.qml` (+ `qmldir`); desktop → `share/applications/org.kde.ducknetdevtool.desktop`; icon → `share/icons/hicolor/scalable/apps/*.svg` (+ `.../256x256/...png`, etc.); metadata → `share/metainfo/org.kde.ducknetdevtool.metainfo.xml`.

---

## 12. Usage walkthrough

### A. Fresh machine → dev-ready (Setup Tooling)

1. Open **Setup Tooling**. On fresh Debian you are not in `sudo` yet: click **Activate Prerequisites** → confirm the SUDO dialog → enter the **root** password in the terminal → machine reboots.
2. Click **Activate Prerequisites** again (installs `pkexec` + `libnss3-tools`; on Fedora: `nss-tools` + disables SELinux and reboots). The LED turns green.
3. **Enable Passwordless Sudo** (console) and **Enable Passwordless Pkexec** (Plasma) — later steps stop prompting.
4. **Install NGINX PHP** (progress dialog streams the log; takes minutes — Fedora conforms + applies session/memory fixes inside this step) → **Install Dev Env** (toolchain + rustup) → optional **Add Dev Folders** / **Install VSCodium** / **Install Opencode**.

### B. Local HTTPS (DEV HTTPS Certs)

1. **1 — Setup CA:** fill Passphrase + Common Name (or *Fill demo*), adjust subject/validity/key-size, click **Setup CA**. Verify: `ls -l ~/certs/myCA.key ~/certs/myCA.pem`.
2. **2 — Install Root CA:** click **Install Root CA** → approve the polkit dialog. Verify: `ls <anchor>` (see §1 table) and `openssl x509 -noout -subject -in ~/certs/myCA.pem`.
3. **3 — Generate:** enter `myapp.test`, optionally add SANs / check Wildcard, click **Generate Certificate**. Verify: `openssl x509 -noout -text -in ~/certs/myapp.test.crt | grep -A2 "Subject Alternative Name"` and `grep myapp.test /etc/hosts`.
4. **Serve it (Nginx Manager — or manual config):**
   ```nginx
   server {
     listen 127.0.0.1:443 ssl;
     server_name myapp.test www.myapp.test;
     ssl_certificate /home/$USER/certs/myapp.test.crt;
     ssl_certificate_key /home/$USER/certs/myapp.test.key;
     root /home/$USER/WebRoots/myapp.test; index index.html;
   }
   ```
   Then `sudo nginx -t && sudo systemctl reload nginx` and `curl -vk https://myapp.test/` (expect `issuer: CN = DuckNet Dev CA`). (Quick-created sites already look like this, localhost-only.)
5. **Firefox:** click **Install to Firefox** (launch Firefox once first so a profile exists). **Chrome:** system trust is enough; the Chrome buttons manage the `~/.pki/nssdb` entry explicitly.
6. **Delete** individual domains via the trash icon; **Remove Root CA & Regenerate** (with confirmation) wipes everything tied to the old CA and returns to step 1.

### C. Day-to-day (Nginx Manager)

- Start/Stop/Restart NGINX + PHP-FPM from the status bars; CONFORMED LED confirms the Fedora layout (applied by Setup Tooling); **Run as DevUser** so webroots need no permission tweaks.
- Databases: **Database Manager** → **Install PostgreSQL** / **Install MariaDB** (LEDs confirm, services auto-enabled + started).
- Create dev databases: **Create Database** → toggle the engine → fill name/user/password (tick utf8mb4 for MariaDB if needed) → **Create** (lists refresh in Current Databases).
- Per database: **Backup DB** dumps to `~/BACKUPDB`, **Restore DB** replays a dump (recreates when missing), **Change Owner Password** (double entry), **Delete** (red confirm + type-the-name gate).
- **Backup** before editing base configs or sites; edits are revert-guarded (`nginx -t`).
- **View Error Log** buttons (global + per-site) tail the relevant logs without leaving the app; **Ctrl+F** in any editor jumps between matches.

---

## 13. Security model

- Private key (`myCA.key`) and passphrase files are created mode `600`; the passphrase is passed to OpenSSL only via `-passin file:` / `-passout file:` (never on the command line).
- Privileged operations (anchor `cp`, trust updates, `/etc/hosts` edits, `systemctl`, `apt/dnf`, config writes, log `tail`) go through `pkexec` with `sudo -n` (passwordless-only, never interactive) as fallback — except the prerequisite/sudo-enrollment terminals, which intentionally use interactive `sudo`/`su` so a password *can* be entered. Cancellations produce explicit error messages with manual commands.
- Sudoers handling is staged + `visudo -c`-gated; polkit rules are group-scoped (`sudo`/`wheel`) and removed on disable. Usernames interpolated into privileged paths are `valid_username`-checked.
- `/etc/hosts` edits use token-exact matching and `sh -c '... "$1" ...' sh <domain>` (no shell injection; domains with spaces/`/` are rejected). Site domains are `valid_site_domain`-checked at every call site; backup filenames are prefix/extension-validated so joins can never escape their directory.
- `deleteCert`/`removeRootCert` are traversal-guarded (`..`, `/`, `\` rejected; deletion limited to listed/generated certs).
- No systemd overrides and no `chown -R` of system paths anywhere (removed after they caused knock-on permission problems); services run as the dev user via config directives instead. The one exception is user-owned territory: `~/WebRoots/.php-session` is created as root during install and `chown`ed back to the dev user (`700`) so PHP-FPM can write sessions.
- Database dumps are captured and written by the app itself (dev-owned files, never a root redirect); pg restores stage through `/tmp` (`644`, cleaned up) because the `postgres` OS user cannot read into a `0700` home. Filenames/owners/charsets are validated before interpolation; the `meta` sidecar is non-secret.
- Log viewers are read-only (`tail -n`, 500-line cap); site-log paths must be absolute and `..`-free, `off`/`syslog:` targets are never followed.
- Quick-created sites bind `127.0.0.1` only — dev servers never listen on all interfaces by default.
- `~/.local/state/devbox.arc` is obfuscation (XOR with `/etc/machine-id` + base64, `600`), not strong encryption — it protects against casual reads, not root. Treat `~/certs/.ca_passphrase` as the real secret.
- Debug logging is verbose by design (`/tmp/ducknet-dev-tool.log` + stderr/stdout); the passphrase itself is never logged.

---

## 14. Troubleshooting & logs

- **Log first:** `tail -f /tmp/ducknet-dev-tool.log` (also stderr when run from a terminal). Every `pkexec`/`sudo` attempt logs its exit code + stdout/stderr. In-app, the global NGINX / PHP-FPM / per-site **View Error Log** buttons tail the service logs directly.
- **`install failed (cp exit …)`:** usually a dismissed polkit dialog, container `no_new_privs` (pkexec blocked — run the printed manual `sudo cp … && sudo update-ca-…` on the host), or missing passwordless sudo.
- **`Unsupported distro`:** check `cat /etc/os-release`; `ID_LIKE` derivatives are covered, anything else shows `UnsupportedPage`. Test overrides: `DUCKNET_OS_RELEASE=/tmp/fake-os-release`.
- **Firefox `No Firefox profile found`:** launch Firefox once to create `*.default-release`, then retry.
- **`certutil` failures:** install NSS tools (`libnss3-tools` / `nss-tools`) or click **Activate Prerequisites**.
- **Prerequisites button dimmed but `pkexec` missing:** fixed — the LED now requires `pkexec` on Debian (see §5.1); re-pull if you saw this on an older build.
- **Fedora still enforcing after the tooling ran:** the SELinux change needs the reboot the dialog performs; verify with `getenforce` / `grep ^SELINUX= /etc/selinux/config`.
- **Just joined `sudo`, still can't install:** group membership needs a fresh login — the enrollment script reboots for exactly this reason.
- **Site log buttons dimmed:** the site file has no `error_log`/`access_log` line (custom config) or isn't readable — add the directive or check perms.
- **AppImage `module "org.kde.desktop" is not installed` / `uses incompatible Qt library`:** fixed by design (host paths + forced style disabled in AppImage); if you still see it, ensure you run the latest `build-appimage.sh` output.
- **`EditorFindBar is not a type` / new page blank in AppImage:** the component missed an explicit registration (§11 table) — `cargo run` works (dir scan) but the deployed `qmldir` doesn't list it.
- **Browser still warns:** Chrome — restart it after step 2; Firefox — use **Install to Firefox** (own NSSDB); after CA regeneration, delete/re-generate old domain certs (they were signed by the old CA).

---

## 15. Testing

```bash
cargo test --lib        # ~103 backend unit tests (fast, never prompt, host-safe)
cargo test              # + tests/cert_integration.rs: AES-256 CA + SAN (DNS+wildcard+IP) in a temp dir
cargo run --bin live_test   # src/bin/live_test.rs: full workflow in real ~/certs (wipes it first!)
./check-qml.sh [--format]   # qmllint over all QML (errors fail, warnings advisory)
```

- **Fixture convention:** pure-function tests use `const TEST_USER: &str = "testdev"` — never the machine's real username, so any developer's checkout passes. Host-dependent probe tests (`*_on_this_host`) assert live state and are inherently host-specific. The `site_conf_render_shape` test derives expected paths from `$HOME` exactly like the renderer (verified under a foreign `HOME`).
- **Shell safety:** generated install scripts are syntax-checked with `sh -n` (parsed, never executed) in-process.
- **Release gates:** `build-appimage.sh` runs `check-qml.sh` + `cargo test --lib` fail-fast before the 30-minute bundling burn.

---

## 16. Adding a new function

Modularity rule: one function ⇒ one `src/<tool>.rs` + one `src/qml/<Tool>Page.qml` + registrations. Shared code goes to `src/common.rs` (Rust) or a registered component like `EditorFindBar.qml` (QML) — not copy-paste.

1. **QML page:** add `src/qml/MyToolPage.qml` (`Kirigami.Page`). Register in **four** places: `build.rs` `.qml_file(...)` **and** the `fix_qmldir_files` mirror filter (`Name ` prefix), `CMakeLists.txt` installed-`qmldir` content list (`Name 1.0 src/qml/Name.qml`), and the `build-appimage.sh` verify loop. (`check-qml.sh` and the CMake `src/qml/` install glob pick up new files automatically.)
2. **Nav entry:** append `ListElement { name: "My Tool"; desc: "..."; iconName: "..."; page: "mytool" }` to `functionModel` in `Main.qml` and route it in the delegate + add a `Component { id: myToolPage ... }`.
3. **Backend (if needed):** add `src/mytool.rs`, declare `pub mod mytool` in `lib.rs`, expose a `#[qobject] #[qml_element]` type with `#[qproperty]`/`#[qinvokable]`s, and list it in `build.rs` (`.files([...])`). Probes must be unprivileged and prompt-free; mutations via `common::privileged_output`; unit tests with the `TEST_USER` fixture convention (§15).
4. **Docs:** extend the help page, `org.kde.ducknetdevtool.metainfo.xml` (`<ul>` features), and this spec's Features + Usage + Data-locations sections.

Ideas already compatible with the current privilege/file patterns: `/etc/hosts` manager, local DNS (`dnsmasq`/`systemd-resolved`) helper, dev-service launcher. (The nginx vhost generator idea is now **done** — see §7.4.)

---

## 17. Known limitations

- Debian + Fedora families only; Arch/openSUSE/etc. show `UnsupportedPage` (trust paths/commands differ).
- Passwordless `pkexec` approval or `sudo -n` required for install/hosts steps (except the prerequisite/sudo-enrollment terminals, which prompt interactively by design); containers with `no_new_privs` must run the printed manual commands on the host.
- The encrypted passphrase store is machine-bound obfuscation (see §13), not a keyring integration.
- `sans_list_to_vec()` in `devcerts.rs` parses `QStringList` via its `Debug` representation — works (primary domain + wildcard always applied) but should be replaced with direct `cxx-qt-lib` iteration once the API is pinned.
- AppImage bundles Qt 6.8 while Fedora ships Qt 6.11 — host QML modules/styles are intentionally excluded (see §4.3); visual style inside the AppImage falls back to bundled QtQuick Controls.
- Log viewers show the last 500 lines per file (by design — full multi-MB logs never cross the QML bridge); the editor find bar is case-insensitive plain-text (no regex / match-case toggle yet).
- Site rows carry seven buttons per domain — tight on narrow windows.
- The Debian `pg_hba.conf` path hardcodes the stock PostgreSQL 17 cluster (`/etc/postgresql/17/main/`); a distro upgrade moving the default cluster needs the constant updated.
- The AppImage UI-content gate greps the bundled `DatabaseManagerPage.qml` for `mariadbEnableVisible`/`enableMariadb` only — newer bindings (`deleteDatabase`, `backupDatabase`, …) should join that marker list.

---

## 18. License

MIT — see `LICENSE`, `Cargo.toml` (`license`), `org.kde.ducknetdevtool.metainfo.xml` (`project_license`), and the `SPDX-License-Identifier` headers in the source files and build scripts.
