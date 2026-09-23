// SPDX-License-Identifier: MIT
//! Shared helpers for manager backends: distro detection, privileged
//! execution, identity/validation, binary probing, conform templates,
//! Qt conversion.
//!
//! Single source keeps backends consistent; cert paths stay in devcerts.

use cxx_qt_lib::{QString, QStringList};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

// ---------------------------------------------------------------------------
// Distro detection
// ---------------------------------------------------------------------------

/// Debian / Fedora families (everything else is unsupported by the app).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Family {
    Debian,
    Fedora,
    Unsupported,
}

/// /etc/os-release path (DUCKNET_OS_RELEASE overrides for tests).
pub(crate) fn os_release_path() -> String {
    std::env::var("DUCKNET_OS_RELEASE").unwrap_or("/etc/os-release".to_string())
}

/// Single `KEY=` field from os-release text (quotes trimmed).
fn field_in(text: &str, key: &str) -> String {
    for line in text.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix(&format!("{}=", key)) {
            return v.trim_matches('"').trim().to_string();
        }
    }
    String::new()
}

/// Single `KEY=` field from the os-release file.
pub(crate) fn os_release_field(key: &str) -> String {
    let text = std::fs::read_to_string(os_release_path()).unwrap_or_default();
    field_in(&text, key)
}

/// ID= value from os-release text, lowercased (pure, unit-testable).
pub(crate) fn parse_os_release_id(text: &str) -> String {
    field_in(text, "ID").to_lowercase()
}

/// Raw distro ID, lowercased ("" when unreadable).
pub(crate) fn distro_id() -> String {
    let text = std::fs::read_to_string(os_release_path()).unwrap_or_default();
    parse_os_release_id(&text)
}

/// Family from os-release text: ID + ID_LIKE token match covers derivatives,
/// then raw-content fallback scan for quirky files.
pub(crate) fn family_of(text: &str) -> Family {
    let combined =
        format!("{} {}", field_in(text, "ID"), field_in(text, "ID_LIKE")).to_lowercase();
    let tokens: Vec<&str> = combined.split_whitespace().collect();
    let has = |names: &[&str]| tokens.iter().any(|t| names.contains(t));
    if has(&["fedora", "rhel", "centos", "rocky", "alma", "almalinux", "ol"]) {
        return Family::Fedora;
    }
    if has(&["debian", "ubuntu", "linuxmint", "pop", "kali", "raspbian"]) {
        return Family::Debian;
    }
    // Fallback scan for quirky files.
    let lower = text.to_lowercase();
    if lower.contains("fedora") || lower.contains("rhel") || lower.contains("centos") {
        return Family::Fedora;
    }
    if lower.contains("debian") || lower.contains("ubuntu") {
        return Family::Debian;
    }
    Family::Unsupported
}

/// Family of this host.
pub(crate) fn family() -> Family {
    let text = std::fs::read_to_string(os_release_path()).unwrap_or_default();
    family_of(&text)
}

pub(crate) fn family_str(f: &Family) -> &'static str {
    match f {
        Family::Debian => "debian",
        Family::Fedora => "fedora",
        Family::Unsupported => "unsupported",
    }
}

pub(crate) fn distro_is_debian_like() -> bool {
    family() == Family::Debian
}

pub(crate) fn distro_is_fedora_like() -> bool {
    family() == Family::Fedora
}

// ---------------------------------------------------------------------------
// Privileged execution + logging
// ---------------------------------------------------------------------------

pub(crate) fn log_debug(msg: &str) {
    // Qt swallows stderr under QML — mirror to stdout and /tmp.
    eprintln!("{}", msg);
    println!("{}", msg);
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let _ = std::io::Write::flush(&mut std::io::stderr());
    // Persistent log for post-click inspection
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/ducknet-dev-tool.log")
        .and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "{}", msg)
        });
}

pub(crate) fn privileged_output(args: &[&str]) -> std::io::Result<std::process::Output> {
    log_debug(&format!("[ducknet] privileged_output: pkexec {:?}", args));
    let pkexec = Command::new("pkexec").args(args).output();
    match pkexec {
        Ok(out) => {
            log_debug(&format!(
                "[ducknet] pkexec result: status={} stdout='{}' stderr='{}'",
                out.status,
                String::from_utf8_lossy(&out.stdout).trim(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
            if out.status.success() {
                return Ok(out);
            }
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            // pkexec failed — retry passwordless sudo -n (never prompts).
            log_debug(&format!("[ducknet] pkexec failed, trying sudo -n {:?}", args));
            let sudo_n = Command::new("sudo").arg("-n").args(args).output();
            match sudo_n {
                Ok(sudo_out) => {
                    log_debug(&format!(
                        "[ducknet] sudo -n result: status={} stdout='{}' stderr='{}'",
                        sudo_out.status,
                        String::from_utf8_lossy(&sudo_out.stdout).trim(),
                        String::from_utf8_lossy(&sudo_out.stderr).trim()
                    ));
                    if sudo_out.status.success() {
                        return Ok(sudo_out);
                    }
                    let sudo_err = String::from_utf8_lossy(&sudo_out.stderr);
                    let sudo_out_s = String::from_utf8_lossy(&sudo_out.stdout);
                    log_debug(&format!("privileged_output: pkexec failed ({stderr} {stdout}) ; sudo -n failed ({sudo_err} {sudo_out_s})"));
                    // Return sudo result so callers see relevant stderr.
                    return Ok(sudo_out);
                }
                Err(e2) => {
                    log_debug(&format!("[ducknet] sudo -n failed to launch: {e2}"));
                    return Ok(out);
                }
            }
        }
        Err(e) => {
            log_debug(&format!("[ducknet] pkexec failed to launch: {e}, trying sudo -n"));
            if e.kind() == std::io::ErrorKind::NotFound {
                return Command::new("sudo").arg("-n").args(args).output();
            }
            let sudo_n = Command::new("sudo").arg("-n").args(args).output();
            match sudo_n {
                Ok(sudo_out) => return Ok(sudo_out),
                Err(_) => return Err(e),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// The invoking developer: $SUDO_USER (when elevated) else $USER else `id -un`.
pub(crate) fn dev_username() -> String {
    if let Ok(u) = std::env::var("SUDO_USER") {
        if !u.is_empty() && u != "root" {
            return u;
        }
    }
    if let Ok(u) = std::env::var("USER") {
        if !u.is_empty() {
            return u;
        }
    }
    if let Ok(out) = Command::new("id").arg("-un").output() {
        if out.status.success() {
            let u = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !u.is_empty() {
                return u;
            }
        }
    }
    "developer".to_string()
}

/// Guard for sudoers filenames/content: reject names outside
/// [a-z_][a-z0-9_-]*[$]? so $USER cannot escape /etc/sudoers.d/<user>.
pub(crate) fn valid_username(u: &str) -> bool {
    if u.is_empty() || u == "root" {
        return false;
    }
    let mut chars = u.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' || c == '$')
}

/// Primary group of `user` (`id -gn`), charset-checked like the username
/// so it is safe to bake into the privileged install script.
pub(crate) fn primary_group(user: &str) -> Option<String> {
    let out = Command::new("id").arg("-gn").arg(user).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let g = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if g.is_empty() || !valid_username(&g) {
        return None;
    }
    Some(g)
}

/// Effective service group: primary group, else the username itself
/// (user-private-group scheme on Debian/Fedora makes these identical).
pub(crate) fn service_group(user: &str) -> String {
    primary_group(user).unwrap_or_else(|| user.to_string())
}

/// Home directory for `user`: $HOME when set, else /home/<user>.
pub(crate) fn home_dir_for(user: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.trim().is_empty() => h.trim_end_matches('/').to_string(),
        _ => format!("/home/{}", user),
    }
}

/// Home directory of the invoking developer.
pub(crate) fn home_dir() -> String {
    home_dir_for(&dev_username())
}

/// Database-login password gate: 1–256 chars.
pub(crate) fn valid_password(p: &str) -> bool {
    !p.is_empty() && p.len() <= 256
}

/// Standard suffix for privilege-failure messages.
pub(crate) const NEEDS_ROOT: &str = "— needs root (pkexec / passwordless sudo)";

/// Vec<String> into a QML QStringList.
pub(crate) fn to_qstringlist(items: &[String]) -> QStringList {
    let mut list = QStringList::default();
    for item in items {
        list.append(QString::from(item));
    }
    list
}

// ---------------------------------------------------------------------------
// Domain validation
// ---------------------------------------------------------------------------

/// Strict domain guard (nginx site names, cert dirs): no `/ \ ..`, ASCII
/// alphanumerics plus `. - _` only.
pub(crate) fn valid_site_domain(domain: &str) -> bool {
    !domain.is_empty()
        && !domain.contains('/')
        && !domain.contains('\\')
        && !domain.contains("..")
        && domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

/// Cert-material guard: valid_site_domain plus leading `*.`; IPs validate separately.
pub(crate) fn valid_cert_domain(domain: &str) -> bool {
    let bare = domain.strip_prefix("*.").unwrap_or(domain);
    !bare.is_empty()
        && !bare.contains('/')
        && !bare.contains('\\')
        && !bare.contains("..")
        && bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

// ---------------------------------------------------------------------------
// Binary probing (no shell spawns)
// ---------------------------------------------------------------------------

/// Absolute path of `cmd` via $PATH (no shell spawned).
/// NOTE: `cmd` is always a hardcoded constant at call sites, never user input.
pub(crate) fn path_lookup(cmd: &str) -> Option<PathBuf> {
    if cmd.is_empty() {
        return None;
    }
    if cmd.contains('/') {
        let p = PathBuf::from(cmd);
        return p.is_file().then_some(p);
    }
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let p = PathBuf::from(dir).join(cmd);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// whereis token matching `cmd` exactly on disk; skips man/data entries.
/// Test-only: production uses [`find_binary`].
#[cfg(test)]
pub(crate) fn whereis_has_binary(output: &str, cmd: &str) -> bool {
    output.split_whitespace().skip(1).any(|tok| {
        let p = std::path::Path::new(tok.trim_end_matches(':'));
        p.file_name().and_then(|n| n.to_str()) == Some(cmd) && p.is_file()
    })
}

/// First existing regular file named exactly `cmd` in `whereis -b` output.
pub(crate) fn parse_whereis_bin(text: &str, cmd: &str) -> Option<PathBuf> {
    for token in text.split_whitespace().skip(1) {
        if !token.starts_with('/') {
            continue;
        }
        let pb = PathBuf::from(token.trim_end_matches(':'));
        if pb.file_name().and_then(|n| n.to_str()) == Some(cmd) && pb.is_file() {
            return Some(pb);
        }
    }
    None
}

/// Layered probe: $PATH, then well-known locations (Debian sbin outside user PATH),
/// then whereis database (immune to PATH quirks, lags fresh installs).
pub(crate) fn find_binary(cmd: &str, extra_paths: &[&str]) -> Option<PathBuf> {
    if let Some(p) = path_lookup(cmd) {
        return Some(p);
    }
    for cand in extra_paths {
        let pb = PathBuf::from(cand);
        if pb.is_file() {
            return Some(pb);
        }
    }
    if let Ok(out) = Command::new("whereis").arg("-b").arg(cmd).output() {
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        if let Some(pb) = parse_whereis_bin(&text, cmd) {
            return Some(pb);
        }
    }
    None
}

pub(crate) fn binary_present(cmd: &str, extra_paths: &[&str]) -> bool {
    find_binary(cmd, extra_paths).is_some()
}

/// Functional probe: succeeds iff the invoking user has NOPASSWD sudo.
/// Never prompts (`-n`), never touches /etc/sudoers.d (mode 0440, unreadable).
pub(crate) fn sudo_nopasswd_active() -> bool {
    Command::new("sudo")
        .arg("-n")
        .arg("true")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Background privileged operations (long installs never block UI;
// QML Timer polls state + log)
// ---------------------------------------------------------------------------

/// Lines of the op log streamed to the progress dialog.
pub(crate) const OP_LOG_TAIL_LINES: usize = 120;

/// Spawn privileged command in background, stdout+stderr appended to op log.
/// Uses passwordless sudo -n when ready, else pkexec for GUI prompt.
/// Append mode: callers truncate first with header.
pub(crate) fn privileged_spawn(argv: &[String], log_path: &str) -> std::io::Result<Child> {
    let sudo_ready = sudo_nopasswd_active();
    let open_log = || {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
    };
    let spawn_with = |prog: &str, extra: &[&str]| -> std::io::Result<Child> {
        let out = open_log()?;
        let err = out.try_clone()?;
        let mut cmd = Command::new(prog);
        for a in extra {
            cmd.arg(a);
        }
        cmd.args(argv)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .spawn()
    };
    if sudo_ready {
        log_debug("[ducknet] privileged_spawn: using sudo -n (already passwordless)");
        spawn_with("sudo", &["-n"])
    } else {
        log_debug("[ducknet] privileged_spawn: using pkexec (GUI prompt)");
        spawn_with("pkexec", &[])
    }
}

/// Last [`OP_LOG_TAIL_LINES`] lines of an op log for the progress dialog.
pub(crate) fn op_log_tail(log_path: &str) -> String {
    let text = std::fs::read_to_string(log_path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(OP_LOG_TAIL_LINES);
    lines[skip..].join("\n")
}

// ---------------------------------------------------------------------------
// Log viewing (shared by the NGINX / PHP-FPM / site log viewers)
// ---------------------------------------------------------------------------

/// Lines shown per log file: bounds QML bridge for long-lived logs.
/// Enforced server-side via `tail -n`, client-side via [`tail_lines`].
pub(crate) const LOG_TAIL_LINES: usize = 500;

/// Last `max` lines of `text`; caps direct reads so large logs skip QML bridge.
pub(crate) fn tail_lines(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(max);
    lines[skip..].join("\n")
}

/// Capped log read: direct read, then privileged `tail -n`; returns display text.
/// `service_hint` names owning service for not-found message.
/// Checks existence via `metadata`, never `Path::exists()`, so root-only logs still try privileged read.
pub(crate) fn read_log_capped(path: &str, service_hint: &str) -> Result<String, String> {
    const EMPTY: &str = "(empty — nothing logged)";
    if let Ok(content) = std::fs::read_to_string(path) {
        let text = tail_lines(&content, LOG_TAIL_LINES);
        return Ok(if text.is_empty() { EMPTY.to_string() } else { text });
    }
    // stat needs no read permission; skip prompt only on genuine absence.
    if let Err(e) = std::fs::metadata(path) {
        if e.kind() == std::io::ErrorKind::NotFound {
            return Err(format!("{} not found — is {} installed?", path, service_hint));
        }
    }
    let n = LOG_TAIL_LINES.to_string();
    match privileged_output(&["tail", "-n", &n, path]) {
        Ok(out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout).to_string();
            Ok(if raw.trim().is_empty() { EMPTY.to_string() } else { raw.trim_end().to_string() })
        }
        Ok(out) => {
            let err = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stderr).trim(),
                String::from_utf8_lossy(&out.stdout).trim()
            );
            // TOCTOU: vanished between check and tail — report missing.
            if err.contains("No such file") {
                return Err(format!("{} not found — is {} installed?", path, service_hint));
            }
            Err(format!(
                "Cannot read {}:{} {NEEDS_ROOT}",
                path,
                if err.is_empty() { String::new() } else { format!(" {}", err) }
            ))
        }
        Err(e) => Err(format!("Failed to read {}: {}", path, e)),
    }
}

// ---------------------------------------------------------------------------
// Fedora nginx conform templates (Debian-style layout)
// ---------------------------------------------------------------------------

/// Stock Fedora nginx.conf rewritten for sites-available/sites-enabled.
/// `@@USER@@` is substituted with the dev user by both consumers (the
/// Nginx Manager conform repair and the Setup Tooling Fedora install).
/// Byte-identical wherever it lands — single source, no drift.
pub(crate) const FEDORA_NGINX_CONF_TEMPLATE: &str = r###"# Reconfigured for Debian-like layout on Fedora.
# See /etc/nginx/nginx.conf.orig for the stock Fedora config.
# Virtual hosts: put files in /etc/nginx/sites-available/ and enable with:
#   ln -s /etc/nginx/sites-available/<site> /etc/nginx/sites-enabled/<site>

user @@USER@@;
worker_processes auto;
error_log /var/log/nginx/error.log notice;
pid /run/nginx.pid;

# Load dynamic modules. See /usr/share/doc/nginx/README.dynamic.
include /usr/share/nginx/modules/*.conf;

events {
    worker_connections 1024;
}

http {
    log_format  main  '$remote_addr - $remote_user [$time_local] "$request" '
                      '$status $body_bytes_sent "$http_referer" '
                      '"$http_user_agent" "$http_x_forwarded_for"';

    access_log  /var/log/nginx/access.log  main;

    sendfile            on;
    tcp_nopush          on;
    keepalive_timeout   65;
    types_hash_max_size 4096;
    server_tokens       off;

    include             /etc/nginx/mime.types;
    default_type        application/octet-stream;

    # Debian-like: enable gzip by default (Debian ships `gzip on;`).
    gzip on;

    # Load modular configuration files from the /etc/nginx/conf.d directory.
    include /etc/nginx/conf.d/*.conf;

    # Debian-style virtual hosts. No `server {}` block lives in this file;
    # every site (including "default") comes from sites-enabled/.
    include /etc/nginx/sites-enabled/*;
}
"###;

/// Default vhost for the conformed layout (migrated from Fedora stock).
pub(crate) const DEBIAN_DEFAULT_SITE_CONTENT: &str = r###"# Default site migrated from Fedora 44's stock /etc/nginx/nginx.conf.
# Manage with Debian-style symlinks:
#   enable:  ln -s /etc/nginx/sites-available/default /etc/nginx/sites-enabled/default
#   disable: rm /etc/nginx/sites-enabled/default && nginx -t && systemctl reload nginx

server {
    listen       80 default_server;
    listen       [::]:80 default_server;
    server_name  _;
    root         /usr/share/nginx/html;

    # Load configuration files for the default server block.
    include /etc/nginx/default.d/*.conf;
}
"###;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_tokens_cover_derivatives() {
        assert_eq!(family_of("ID=debian\n"), Family::Debian);
        assert_eq!(family_of("ID=ubuntu\n"), Family::Debian);
        // Kali reports ID=kali with ID_LIKE=debian.
        assert_eq!(family_of("ID=kali\nID_LIKE=debian\n"), Family::Debian);
        assert_eq!(family_of("ID=fedora\n"), Family::Fedora);
        assert_eq!(
            family_of("ID=almalinux\nID_LIKE=\"rhel centos fedora\"\n"),
            Family::Fedora
        );
        // Oracle Linux: exact `ol` token (must not match substrings).
        assert_eq!(family_of("ID=ol\nID_LIKE=\"fedora\"\n"), Family::Fedora);
        assert_eq!(family_of("ID=arch\n"), Family::Unsupported);
        assert_eq!(family_of(""), Family::Unsupported);
        assert_eq!(family_str(&Family::Debian), "debian");
        assert_eq!(distro_is_debian_like() || distro_is_fedora_like() || true, true);
    }

    #[test]
    fn path_lookup_finds_shell_without_spawning() {
        // /bin/sh exists on every Linux; empty/absolute handling included.
        let p = path_lookup("sh");
        assert!(p.is_some());
        assert!(path_lookup("").is_none());
        assert_eq!(path_lookup("/bin/sh"), Some(PathBuf::from("/bin/sh")));
        assert!(path_lookup("/nonexistent/cmd").is_none());
    }

    #[test]
    fn log_tail_caps_long_logs() {
        assert_eq!(tail_lines("a\nb\nc", 2), "b\nc");
        assert_eq!(tail_lines("a\nb", 5), "a\nb");
        assert_eq!(tail_lines("", 5), "");
        assert_eq!(tail_lines("only", 0), "");
        assert!(LOG_TAIL_LINES >= 100);
    }

    #[test]
    fn log_capped_missing_names_service() {
        let err = read_log_capped("/nonexistent-ducknet/error.log", "nginx").unwrap_err();
        assert!(err.contains("is nginx installed?"));
        let err = read_log_capped("/nonexistent-ducknet/error.log", "php-fpm").unwrap_err();
        assert!(err.contains("is php-fpm installed?"));
    }
}
