# DuckNet Dev Tool

A native desktop app for **KDE Plasma** on **Debian** and **Fedora** that turns a fresh install into a ready-to-use local web-development machine — HTTPS certs, NGINX + PHP, databases, and dev tooling, all from one friendly GUI.

Built with **Rust + Qt6/QML + Kirigami**. Current version **0.1.0**, licensed **MIT**.

## What it does

**Setup Tooling** — Get a fresh machine dev-ready
- Install prerequisites, NGINX + PHP 8.4, and a full C/C++/Python/Rust build toolchain
- Set up passwordless sudo / pkexec for smooth Plasma workflows
- Create standard dev folders (`Coding`, `MyApps`, `WebRoots`) with Dolphin Places entries
- Optionally install VSCodium and Opencode

**DEV HTTPS Certs** — Real local HTTPS in three steps
- Create your own local Certificate Authority
- Trust it system-wide in Chrome and Firefox
- Issue per-domain certificates (with SAN and wildcard support) for domains like `myapp.test`

**Nginx Manager** — Run and manage local sites
- Start, stop, and restart NGINX and PHP-FPM with live status
- Create localhost-only development sites from your certificates in one click
- Edit NGINX and PHP-FPM configs with backup/restore and built-in safety checks
- Live config linting while you type, plus find-in-editor (Ctrl+F)
- Built-in viewers for NGINX, PHP-FPM, and per-site error/access logs

**Database Manager** — Local databases without the hassle
- Install PostgreSQL and MariaDB with one click and clear status indicators
- Create databases and owners through a simple form (including utf8mb4 support for MariaDB)
- Back up and restore databases, change owner passwords, and delete databases with a type-to-confirm safety gate

**System Info** — Live overview of your host (platform, CPU, memory, disks)

**Dev Tool Help** — Built-in docs with commands, NGINX recipes, and troubleshooting tips

## Who it's for

Developers who build and test PHP / web projects locally on Debian- or Fedora-based KDE Plasma systems and want repeatable, GUI-driven setup instead of memorizing server admin steps.

## Requirements

- Debian-family (Debian, Ubuntu, Mint, Pop!_OS, Kali, Raspbian) or Fedora-family (Fedora, RHEL, CentOS, Alma, Rocky, Oracle Linux)
- KDE Plasma desktop, Qt6, Rust toolchain
- See `BUILDSPEC.md` §9 for the full dependency list

## Quick start

```bash
cargo run
```

Or build a release binary:

```bash
cargo build --release
./target/release/ducknet-dev-tool
```

A portable AppImage can be built with `./build-appimage.sh`.

## Typical workflow

1. **Setup Tooling** → Activate Prerequisites → enable passwordless sudo/pkexec → Install NGINX + PHP and Dev Env
2. **DEV HTTPS Certs** → Setup CA → Install Root CA → Generate a certificate for `myapp.test`
3. **Nginx Manager** → Quick-create the site → Start NGINX + PHP-FPM
4. **Database Manager** → Install PostgreSQL / MariaDB → Create your dev database
5. Open `https://myapp.test` in your browser and start coding

## Documentation

- Full technical specification: [`BUILDSPEC.md`](BUILDSPEC.md)
- In-app help: open **Dev Tool Help** in the left navigation

## License

MIT — see [`LICENSE`](LICENSE).
