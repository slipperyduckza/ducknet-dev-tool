//! Nginx Manager backend: sites, base configs, logs, service control.
//! Status probes run unprivileged and never prompt; mutations go
//! through pkexec / sudo -n via `privileged_output`.

use cxx_qt_lib::{QString, QStringList};
use std::pin::Pin;
use std::process::Command;

use crate::common::{dev_username, log_debug, primary_group, privileged_output, read_log_capped, to_qstringlist, valid_site_domain, DEBIAN_DEFAULT_SITE_CONTENT, FEDORA_NGINX_CONF_TEMPLATE, NEEDS_ROOT};
use crate::devcerts::{certs_dir, list_generated_certs};

/// Parse `systemctl status nginx.service` output.
/// Returns (running, worker_count): running when "active (running)" appears,
/// workers = lines mentioning "worker process".
fn parse_nginx_status(output: &str) -> (bool, i32) {
    let running = output.contains("active (running)");
    let workers = output
        .lines()
        .filter(|l| l.contains("worker process"))
        .count() as i32;
    (running, workers)
}

/// Parse `systemctl status php*-fpm.service` output.
/// Returns (running, worker_count): running when "active (running)" appears,
/// workers = pool process lines (`"php-fpm: pool <name>"`).
fn parse_phpfpm_status(output: &str) -> (bool, i32) {
    let running = output.contains("active (running)");
    let workers = output
        .lines()
        .filter(|l| l.contains("php-fpm: pool"))
        .count() as i32;
    (running, workers)
}

/// First `user <name>;` directive in nginx.conf (ignores comments).
fn parse_nginx_user(conf: &str) -> Option<String> {
    for line in conf.lines() {
        let t = line.trim_start();
        if t.starts_with("user ") || t.starts_with("user\t") {
            let rest = t["user".len()..].trim_start();
            let name: String = rest.chars().take_while(|c| *c != ';' && !c.is_whitespace()).collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

fn read_nginx_conf() -> Option<String> {
    std::fs::read_to_string("/etc/nginx/nginx.conf").ok()
}

/// PHP-FPM pool file + test binary + service, Debian versioned path first
/// then Fedora's (same paths the Setup Tooling install uses).
/// Returns (pool file, fpm test binary, fpm service).
fn php_pool_path() -> Option<(&'static str, &'static str, &'static str)> {
    const DEB: &str = "/etc/php/8.4/fpm/pool.d/www.conf";
    const FED: &str = "/etc/php-fpm.d/www.conf";
    if std::path::Path::new(DEB).exists() {
        Some((DEB, "php-fpm8.4", "php8.4-fpm.service"))
    } else if std::path::Path::new(FED).exists() {
        Some((FED, "php-fpm", "php-fpm.service"))
    } else {
        None
    }
}

/// Base config locations for the "Base Configuration" section.
/// NGINX is identical on Debian + Fedora; the PHP-FPM pool file is resolved
/// via `php_pool_path()` (Debian versioned path first, then Fedora).
const NGINX_BASE_CONF: &str = "/etc/nginx/nginx.conf";
const NGINX_BACKUP_PREFIX: &str = "nginx";
const PHPFPM_BACKUP_PREFIX: &str = "www";

/// Global NGINX error log (root-owned 640; direct read fails for dev user).
/// Viewed via [`crate::common::read_log_capped`].
const NGINX_ERROR_LOG: &str = "/var/log/nginx/error.log";

/// `error_log = <path>` in php-fpm.conf (strips comments and quotes).
/// Pure for unit tests.
fn parse_fpm_error_log(conf: &str) -> Option<String> {
    for line in conf.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with(';') || t.starts_with('#') || t.starts_with('[') {
            continue;
        }
        let bare = t.split([';', '#']).next().unwrap_or("").trim();
        let Some(rest) = bare.strip_prefix("error_log") else {
            continue;
        };
        let rest = rest.trim_start_matches([' ', '\t', '=']).trim();
        if rest.is_empty() {
            continue;
        }
        let path = rest.trim_matches('"').trim_matches('\'').trim().to_string();
        if !path.is_empty() {
            return Some(path);
        }
    }
    None
}

/// Global php-fpm.conf candidates (Debian versioned path first, then Fedora).
fn phpfpm_main_conf() -> Option<&'static str> {
    const DEB: &str = "/etc/php/8.4/fpm/php-fpm.conf";
    const FED: &str = "/etc/php-fpm.conf";
    if std::path::Path::new(DEB).exists() {
        Some(DEB)
    } else if std::path::Path::new(FED).exists() {
        Some(FED)
    } else {
        None
    }
}

/// Resolved PHP-FPM error log: the `error_log` directive from the active
/// php-fpm.conf when readable, else the first existing fallback, else the
/// conventional `/var/log/php-fpm/error.log` (caller reports it missing).
fn phpfpm_error_log_path() -> String {
    const FALLBACKS: &[&str] = &[
        "/var/log/php-fpm/error.log",
        "/var/log/php8.4-fpm.log",
        "/var/log/php-fpm.log",
    ];
    if let Some(conf) = phpfpm_main_conf() {
        let content = std::fs::read_to_string(conf).ok().and_then(|c| {
            // Privileged fallback for hardened perms.
            Some(c)
        }).or_else(|| {
            crate::common::privileged_output(&["cat", conf])
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        });
        if let Some(text) = content {
            if let Some(path) = parse_fpm_error_log(&text) {
                return path;
            }
        }
    }
    for cand in FALLBACKS {
        if std::path::Path::new(cand).exists() {
            return cand.to_string();
        }
    }
    FALLBACKS[0].to_string()
}

/// ~/.local/backups/<kind> — same destinations as backupnginx.sh (nginx)
/// and backupphpfpm-{debian,fedora}.sh (php-fpm).
fn backup_root_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    std::path::Path::new(&home).join(".local").join("backups")
}

fn base_backup_dir(kind: &str) -> std::path::PathBuf {
    backup_root_dir().join(kind)
}

/// Per-site backup dir: ~/.local/backups/sites/<domain> — one subfolder
/// per site so each domain's history stays separate.
fn site_backup_dir(domain: &str) -> std::path::PathBuf {
    base_backup_dir("sites").join(domain)
}

/// Backup filenames made by the scripts: `<prefix>-DD-MM-YYYY-HH-MM-SS.conf.bak`.
fn valid_backup_filename(name: &str, prefix: &str) -> bool {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
    {
        return false;
    }
    name.starts_with(&format!("{}-", prefix)) && name.ends_with(".conf.bak")
}

/// Backup files in `dir` with `prefix`, most recent first (mtime desc,
/// filename desc as tiebreak — the DD-MM-YYYY stamp does not sort lexically).
fn list_base_backups(dir: &std::path::Path, prefix: &str) -> Vec<String> {
    let mut entries: Vec<(std::time::SystemTime, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Some(name) = e.file_name().to_str().map(|s| s.to_string()) {
                if valid_backup_filename(&name, prefix) {
                    let mtime = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    entries.push((mtime, name));
                }
            }
        }
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    entries.into_iter().map(|(_, n)| n).collect()
}

/// Delete one backup file from its backup dir. The validated filename
/// cannot contain `/`, `\` or `..`, so the join can never escape `dir`.
/// Backups live in $HOME — no privilege needed.
fn delete_backup_file(
    dir: &std::path::Path,
    prefix: &str,
    filename: &str,
) -> Result<(), String> {
    if !valid_backup_filename(filename, prefix) {
        return Err("Invalid backup selection.".to_string());
    }
    let path = dir.join(filename);
    std::fs::remove_file(&path).map_err(|e| format!("Delete failed: {}", e))
}

/// Copy root-owned `src` to a timestamped backup in `dir`.
/// Mirrors backupnginx.sh / backupphpfpm-*.sh naming.
/// Tries direct copy first (644), falls back to privileged `cp -a`.
fn do_base_backup(
    src: &str,
    dir: &std::path::Path,
    prefix: &str,
    priv_step: &dyn Fn(&[&str]) -> Result<String, String>,
) -> Result<String, String> {
    if !std::path::Path::new(src).exists() {
        return Err(format!("source not found: {}", src));
    }
    if let Err(e) = std::fs::create_dir_all(dir) {
        return Err(format!("cannot create {}: {}", dir.display(), e));
    }
    let stamp = chrono::Local::now().format("%d-%m-%Y-%H-%M-%S").to_string();
    let filename = format!("{}-{}.conf.bak", prefix, stamp);
    let dest = dir.join(&filename);
    let dest_s = dest.to_string_lossy().to_string();
    if std::fs::copy(src, &dest).is_ok() {
        return Ok(filename);
    }
    priv_step(&["cp", "-a", src, &dest_s]).map(|_| filename).map_err(|e| {
        let _ = std::fs::remove_file(&dest);
        format!("backup copy failed: {} {NEEDS_ROOT}", e)
    })
}

/// Parse `user = x` / `group = y` from a pool www.conf.
/// Skips comments, blank lines and section headers; tolerates spacing.
fn parse_pool_user_group(text: &str) -> (Option<String>, Option<String>) {
    let mut user = None;
    let mut group = None;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with(';') || t.starts_with('#') || t.starts_with('[') {
            continue;
        }
        if let Some(v) = t.strip_prefix("user") {
            // Require a separator next (else `username =` would false-match).
            if !v.starts_with([' ', '\t', '=']) {
                continue;
            }
            let v = v.trim_start_matches([' ', '\t', '=']).trim();
            let v = v.trim_end_matches([' ', '\t']);
            if !v.is_empty() && user.is_none() {
                user = Some(v.to_string());
            }
        } else if let Some(v) = t.strip_prefix("group") {
            if !v.starts_with([' ', '\t', '=']) {
                continue;
            }
            let v = v.trim_start_matches([' ', '\t', '=']).trim();
            let v = v.trim_end_matches([' ', '\t']);
            if !v.is_empty() && group.is_none() {
                group = Some(v.to_string());
            }
        }
    }
    (user, group)
}

/// Debian-style default site block (shared template in common).
fn default_site_content() -> &'static str {
    DEBIAN_DEFAULT_SITE_CONTENT
}

/// Debian-style nginx.conf keeping Fedora specifics (module path, logging),
/// no inline server block, includes for conf.d + sites-enabled. The dev
/// user is baked in via the shared template.
fn render_nginx_conf(user: &str) -> String {
    FEDORA_NGINX_CONF_TEMPLATE.replace("@@USER@@", user)
}

/// Probe nginx.service state. Returns (running, workers, detail_message).
fn probe_nginx() -> (bool, i32, String) {
    match Command::new("systemctl")
        .args(["status", "nginx.service"])
        .output()
    {
        Err(e) => (false, 0, format!("Failed to run systemctl status: {}", e)),
        Ok(out) => {
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            if text.contains("could not be found")
                || text.contains("not loaded")
                || text.contains("No such file")
            {
                return (false, 0, "nginx.service not found — is nginx installed?".to_string());
            }
            let (running, workers) = parse_nginx_status(&text);
            let detail = if running {
                format!("active (running), {} worker(s)", workers)
            } else if text.contains("inactive (dead)") {
                "inactive (dead)".to_string()
            } else if text.contains("failed") {
                "service failed — check journalctl -u nginx.service".to_string()
            } else {
                "not running".to_string()
            };
            (running, workers, detail)
        }
    }
}

/// PHP-FPM systemd unit for this distro, via the pool-file probe
/// (php8.4-fpm.service on Debian, php-fpm.service on Fedora).
fn phpfpm_service_unit() -> Option<&'static str> {
    php_pool_path().map(|(_, _, service)| service)
}

/// Probe the PHP-FPM service state. Returns (running, workers, detail).
fn probe_phpfpm_service() -> (bool, i32, String) {
    let Some(unit) = phpfpm_service_unit() else {
        return (false, 0, "php-fpm service not found — is php-fpm installed?".to_string());
    };
    match Command::new("systemctl").args(["status", unit]).output() {
        Err(e) => (false, 0, format!("Failed to run systemctl status: {}", e)),
        Ok(out) => {
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            if text.contains("could not be found")
                || text.contains("not loaded")
                || text.contains("No such file")
            {
                return (false, 0, format!("{} not found — is php-fpm installed?", unit));
            }
            let (running, workers) = parse_phpfpm_status(&text);
            let detail = if running {
                format!("active (running), {} worker(s)", workers)
            } else if text.contains("inactive (dead)") {
                "inactive (dead)".to_string()
            } else if text.contains("failed") {
                format!("service failed — check journalctl -u {}", unit)
            } else {
                "not running".to_string()
            };
            (running, workers, detail)
        }
    }
}

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(bool, nginx_running, cxx_name = "nginxRunning")]
        #[qproperty(QString, nginx_status_text, cxx_name = "nginxStatusText")]
        #[qproperty(i32, worker_count, cxx_name = "workerCount")]
        #[qproperty(bool, phpfpm_running, cxx_name = "phpfpmRunning")]
        #[qproperty(QString, phpfpm_status_text, cxx_name = "phpfpmStatusText")]
        #[qproperty(i32, phpfpm_worker_count, cxx_name = "phpfpmWorkerCount")]
        #[qproperty(QString, status_message, cxx_name = "statusMessage")]
        #[qproperty(bool, sites_conformed, cxx_name = "sitesConformed")]
        #[qproperty(QString, nginx_user, cxx_name = "nginxUser")]
        #[qproperty(bool, devuser_active, cxx_name = "devuserActive")]
        #[qproperty(QString, phpfpm_user, cxx_name = "phpfpmUser")]
        #[qproperty(bool, phpfpm_devuser_active, cxx_name = "phpfpmDevuserActive")]
        #[qproperty(QStringList, site_domains, cxx_name = "siteDomains")]
        #[qproperty(QStringList, configured_sites, cxx_name = "configuredSites")]
        #[qproperty(QString, site_config_text, cxx_name = "siteConfigText")]
        #[qproperty(bool, nginx_base_backed_up, cxx_name = "nginxBaseBackedUp")]
        #[qproperty(QString, nginx_base_backup_status, cxx_name = "nginxBaseBackupStatus")]
        #[qproperty(bool, phpfpm_base_backed_up, cxx_name = "phpfpmBaseBackedUp")]
        #[qproperty(QString, phpfpm_base_backup_status, cxx_name = "phpfpmBaseBackupStatus")]
        #[qproperty(QStringList, nginx_base_backups, cxx_name = "nginxBaseBackups")]
        #[qproperty(QStringList, phpfpm_base_backups, cxx_name = "phpfpmBaseBackups")]
        #[qproperty(QString, nginx_base_config_text, cxx_name = "nginxBaseConfigText")]
        #[qproperty(QString, phpfpm_base_config_text, cxx_name = "phpfpmBaseConfigText")]
        #[qproperty(QString, nginx_base_path, cxx_name = "nginxBasePath")]
        #[qproperty(QString, phpfpm_base_path, cxx_name = "phpfpmBasePath")]
        #[qproperty(QStringList, site_backups, cxx_name = "siteBackups")]
        #[qproperty(QString, site_backup_domain, cxx_name = "siteBackupDomain")]
        #[qproperty(bool, backup_folder_ready, cxx_name = "backupFolderReady")]
        #[qproperty(QString, backup_folder_status, cxx_name = "backupFolderStatus")]
        #[qproperty(QString, nginx_error_log, cxx_name = "nginxErrorLog")]
        #[qproperty(QString, phpfpm_error_log, cxx_name = "phpfpmErrorLog")]
        #[qproperty(QString, phpfpm_error_log_path, cxx_name = "phpfpmErrorLogPath")]
        #[qproperty(QString, site_log_text, cxx_name = "siteLogText")]
        #[qproperty(QString, site_log_title, cxx_name = "siteLogTitle")]
        type NginxManager = super::NginxManagerRust;

        /// Re-probe nginx.service (running, workers, status text).
        #[qinvokable]
        #[cxx_name = "refreshStatus"]
        fn refresh_status(self: Pin<&mut Self>) -> bool;

        /// Lightweight re-probe (running + workers only, no message churn).
        /// Safe for a 5s QML Timer — workers fluctuate across reloads.
        #[qinvokable]
        #[cxx_name = "refreshLive"]
        fn refresh_live(self: Pin<&mut Self>) -> bool;

        /// `systemctl start nginx.service` (privileged). No-op when running.
        #[qinvokable]
        #[cxx_name = "startNginx"]
        fn start_nginx(self: Pin<&mut Self>) -> bool;

        /// `systemctl stop nginx.service` (privileged). No-op when stopped.
        #[qinvokable]
        #[cxx_name = "stopNginx"]
        fn stop_nginx(self: Pin<&mut Self>) -> bool;

        /// `systemctl restart nginx.service` (privileged). No-op when stopped.
        #[qinvokable]
        #[cxx_name = "restartNginx"]
        fn restart_nginx(self: Pin<&mut Self>) -> bool;

        /// `systemctl start <php-fpm unit>` (privileged). No-op when running.
        #[qinvokable]
        #[cxx_name = "startPhpFpm"]
        fn start_phpfpm(self: Pin<&mut Self>) -> bool;

        /// `systemctl stop <php-fpm unit>` (privileged). No-op when stopped.
        #[qinvokable]
        #[cxx_name = "stopPhpFpm"]
        fn stop_phpfpm(self: Pin<&mut Self>) -> bool;

        /// `systemctl restart <php-fpm unit>` (privileged). No-op when stopped.
        #[qinvokable]
        #[cxx_name = "restartPhpFpm"]
        fn restart_phpfpm(self: Pin<&mut Self>) -> bool;

        /// Conform Fedora nginx to Debian sites-available/sites-enabled layout.
        /// Backs up, writes dirs/default/conf, symlinks, tests + reloads.
        #[qinvokable]
        #[cxx_name = "conformNginx"]
        fn conform_nginx(self: Pin<&mut Self>) -> bool;

        /// Rewrite the nginx.conf `user` directive to the invoking developer
        /// (so ~/WebRoots need no permission tweaks). Test + reload after.
        #[qinvokable]
        #[cxx_name = "runAsDevUser"]
        fn run_as_devuser(self: Pin<&mut Self>) -> bool;

        /// Rewrite the PHP-FPM pool `user`/`group` to the invoking developer.
        /// Gated by `php-fpm -t` with backup restore, then restarts FPM.
        #[qinvokable]
        #[cxx_name = "runPhpFpmAsDevUser"]
        fn run_phpfpm_as_devuser(self: Pin<&mut Self>) -> bool;

        /// Create site conf + symlink + webroot, gated by `nginx -t` (no reload).
        /// Fails when the site exists or the domain has no generated cert.
        #[qinvokable]
        #[cxx_name = "quickCreate"]
        fn quick_create(self: Pin<&mut Self>, domain: &QString) -> bool;

        /// Load a site file into siteConfigText AND stage a pristine /tmp
        /// backup for the save-time revert guard. False when unreadable.
        #[qinvokable]
        #[cxx_name = "loadSiteConfig"]
        fn load_site_config(self: Pin<&mut Self>, domain: &QString) -> bool;

        /// Write edited text to the site file, gated by `nginx -t`: on
        /// failure the pristine /tmp backup is restored and the user is told
        /// the file was reverted, so a typo can never crash nginx.
        #[qinvokable]
        #[cxx_name = "saveSiteConfig"]
        fn save_site_config(self: Pin<&mut Self>, domain: &QString, text: &QString) -> bool;

        /// Drop /tmp staging files (editor Cancel). Best effort.
        #[qinvokable]
        #[cxx_name = "cancelSiteEdit"]
        fn cancel_site_edit(self: Pin<&mut Self>, domain: &QString) -> bool;

        /// Delete a site: symlink + sites-available file (privileged) and the
        /// ~/WebRoots/<domain> folder (own files), then test + reload.
        #[qinvokable]
        #[cxx_name = "deleteSite"]
        fn delete_site(self: Pin<&mut Self>, domain: &QString) -> bool;

        /// Live lint of a draft: "" when clean, else `L<line>:<col> [error|warn] msg` lines.
        /// Pure; safe per keystroke (debounced in QML).
        /// `nginx -t` remains the gate at save time.
        #[qinvokable]
        #[cxx_name = "lintSiteDraft"]
        fn lint_site_draft(self: &Self, text: &QString) -> QString;

        /// Re-probe base-config backup LEDs + backup lists + resolved paths.
        #[qinvokable]
        #[cxx_name = "refreshBaseStatus"]
        fn refresh_base_status(self: Pin<&mut Self>) -> bool;

        /// Timestamped backup of /etc/nginx/nginx.conf into
        /// ~/.local/backups/nginx (mirrors backupnginx.sh).
        #[qinvokable]
        #[cxx_name = "backupNginxBase"]
        fn backup_nginx_base(self: Pin<&mut Self>) -> bool;

        /// Timestamped backup of the distro www.conf into
        /// ~/.local/backups/php-fpm (mirrors backupphpfpm-*.sh).
        #[qinvokable]
        #[cxx_name = "backupPhpfpmBase"]
        fn backup_phpfpm_base(self: Pin<&mut Self>) -> bool;

        /// Refresh the nginx backup filename list (most recent first).
        #[qinvokable]
        #[cxx_name = "refreshNginxBackups"]
        fn refresh_nginx_backups(self: Pin<&mut Self>) -> bool;

        /// Refresh the php-fpm backup filename list (most recent first).
        #[qinvokable]
        #[cxx_name = "refreshPhpfpmBackups"]
        fn refresh_phpfpm_backups(self: Pin<&mut Self>) -> bool;

        /// Restore a chosen nginx backup over /etc/nginx/nginx.conf,
        /// gated by `nginx -t` + reload.
        #[qinvokable]
        #[cxx_name = "restoreNginxBackup"]
        fn restore_nginx_backup(self: Pin<&mut Self>, filename: &QString) -> bool;

        /// Restore a chosen php-fpm backup over the distro www.conf,
        /// gated by `<fpm> -t` + service restart.
        #[qinvokable]
        #[cxx_name = "restorePhpfpmBackup"]
        fn restore_phpfpm_backup(self: Pin<&mut Self>, filename: &QString) -> bool;

        /// Load /etc/nginx/nginx.conf into nginxBaseConfigText + stage a
        /// pristine /tmp backup for the save-time revert guard.
        #[qinvokable]
        #[cxx_name = "loadNginxBaseConfig"]
        fn load_nginx_base_config(self: Pin<&mut Self>) -> bool;

        /// Save edited nginx.conf text, gated by `nginx -t` with revert.
        #[qinvokable]
        #[cxx_name = "saveNginxBaseConfig"]
        fn save_nginx_base_config(self: Pin<&mut Self>, text: &QString) -> bool;

        /// Drop /tmp nginx-base staging files (editor Cancel).
        #[qinvokable]
        #[cxx_name = "cancelNginxBaseEdit"]
        fn cancel_nginx_base_edit(self: Pin<&mut Self>) -> bool;

        /// Load the distro www.conf into phpfpmBaseConfigText + stage /tmp backup.
        #[qinvokable]
        #[cxx_name = "loadPhpfpmBaseConfig"]
        fn load_phpfpm_base_config(self: Pin<&mut Self>) -> bool;

        /// Save edited www.conf text, gated by `<fpm> -t` with revert + restart.
        /// No nginx-lint: www.conf is INI-style, not nginx syntax.
        #[qinvokable]
        #[cxx_name = "savePhpfpmBaseConfig"]
        fn save_phpfpm_base_config(self: Pin<&mut Self>, text: &QString) -> bool;

        /// Drop /tmp php-fpm-base staging files (editor Cancel).
        #[qinvokable]
        #[cxx_name = "cancelPhpfpmBaseEdit"]
        fn cancel_phpfpm_base_edit(self: Pin<&mut Self>) -> bool;

        /// Timestamped backup of a site file into
        /// ~/.local/backups/sites/<domain> (same naming as the base backups).
        #[qinvokable]
        #[cxx_name = "backupSiteConfig"]
        fn backup_site_config(self: Pin<&mut Self>, domain: &QString) -> bool;

        /// Refresh the backup filename list for a site (most recent first).
        #[qinvokable]
        #[cxx_name = "refreshSiteBackups"]
        fn refresh_site_backups(self: Pin<&mut Self>, domain: &QString) -> bool;

        /// Restore a site backup over sites-available/<domain>.conf.
        /// Gated by `nginx -t` + reload, reverts on failure.
        #[qinvokable]
        #[cxx_name = "restoreSiteBackup"]
        fn restore_site_backup(self: Pin<&mut Self>, domain: &QString, filename: &QString) -> bool;

        /// Delete one NGINX base backup file.
        #[qinvokable]
        #[cxx_name = "deleteNginxBackup"]
        fn delete_nginx_backup(self: Pin<&mut Self>, filename: &QString) -> bool;

        /// Delete one PHP-FPM base backup file.
        #[qinvokable]
        #[cxx_name = "deletePhpfpmBackup"]
        fn delete_phpfpm_backup(self: Pin<&mut Self>, filename: &QString) -> bool;

        /// Delete one site backup file from ~/.local/backups/sites/<domain>.
        #[qinvokable]
        #[cxx_name = "deleteSiteBackup"]
        fn delete_site_backup(self: Pin<&mut Self>, domain: &QString, filename: &QString) -> bool;

        /// Open ~/.local/backups in the desktop file manager via xdg-open.
        /// False when the folder does not exist or the launcher fails.
        #[qinvokable]
        #[cxx_name = "openBackupFolder"]
        fn open_backup_folder(self: Pin<&mut Self>) -> bool;

        /// Load the last 500 lines of /var/log/nginx/error.log for the viewer.
        /// False when missing or unreadable even as root.
        #[qinvokable]
        #[cxx_name = "loadNginxErrorLog"]
        fn load_nginx_error_log(self: Pin<&mut Self>) -> bool;

        /// Load the last 500 lines of the PHP-FPM error log for the viewer.
        /// Path comes from the active php-fpm.conf `error_log` directive.
        #[qinvokable]
        #[cxx_name = "loadPhpfpmErrorLog"]
        fn load_phpfpm_error_log(self: Pin<&mut Self>) -> bool;

        /// Per-row `enabled` probe for the site log buttons.
        /// True when the site file carries an `error_log` / `access_log` path.
        /// Unprivileged only; never prompts.
        #[qinvokable]
        #[cxx_name = "siteHasSiteLog"]
        fn site_has_site_log(self: &Self, domain: &QString, kind: &QString) -> bool;

        /// Load a site's logs for the viewer, joining files under `==> path <==` headers.
        /// Resolves every `error_log` / `access_log` path, tails each (capped).
        /// False when the directive is absent or nothing reads even as root.
        #[qinvokable]
        #[cxx_name = "loadSiteLog"]
        fn load_site_log(self: Pin<&mut Self>, domain: &QString, kind: &QString) -> bool;
    }
}

pub struct NginxManagerRust {
    nginx_running: bool,
    nginx_status_text: QString,
    worker_count: i32,
    phpfpm_running: bool,
    phpfpm_status_text: QString,
    phpfpm_worker_count: i32,
    status_message: QString,
    sites_conformed: bool,
    nginx_user: QString,
    devuser_active: bool,
    phpfpm_user: QString,
    phpfpm_devuser_active: bool,
    site_domains: QStringList,
    configured_sites: QStringList,
    site_config_text: QString,
    nginx_base_backed_up: bool,
    nginx_base_backup_status: QString,
    phpfpm_base_backed_up: bool,
    phpfpm_base_backup_status: QString,
    nginx_base_backups: QStringList,
    phpfpm_base_backups: QStringList,
    nginx_base_config_text: QString,
    phpfpm_base_config_text: QString,
    nginx_base_path: QString,
    phpfpm_base_path: QString,
    site_backups: QStringList,
    site_backup_domain: QString,
    backup_folder_ready: bool,
    backup_folder_status: QString,
    nginx_error_log: QString,
    phpfpm_error_log: QString,
    phpfpm_error_log_path: QString,
    site_log_text: QString,
    site_log_title: QString,
}

/// /etc/nginx/sites-available exists = Debian-style layout in place.
fn sites_conformed() -> bool {
    std::path::Path::new("/etc/nginx/sites-available").is_dir()
}

/// Domains with a generated cert (same source as page 3's list).
fn site_domain_list() -> Vec<String> {
    list_generated_certs()
}

/// Domains with an existing sites-available/<domain>.conf.
fn configured_site_list() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/etc/nginx/sites-available") {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some("conf") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    if stem != "default" {
                        out.push(stem.to_string());
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// Render a simple HTTP+HTTPS vhost.
fn render_site_conf(domain: &str, user: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| format!("/home/{}", user));
    let webroot = format!("{}/WebRoots/{}", home, domain);
    let cert = format!("{}/certs/{}.crt", home, domain);
    let key = format!("{}/certs/{}.key", home, domain);
    format!(
        r#"# {domain} - managed by DuckNet Dev Tool
# User: {user}

server {{
    listen 127.0.0.1:80;

    server_name {domain} www.{domain};

    root {webroot};
    index index.html;

    access_log /var/log/nginx/{domain}.access.log;
    error_log /var/log/nginx/{domain}.error.log;

    return 301 https://$host$request_uri;

    location / {{
        try_files $uri $uri/ =404;
    }}
}}

server {{
    listen 127.0.0.1:443 ssl;

    server_name {domain} www.{domain};

    root {webroot};
    index index.html;

    ssl_certificate {cert};
    ssl_certificate_key {key};

    # Modern TLS only
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_prefer_server_ciphers on;

    access_log /var/log/nginx/{domain}.ssl.access.log;
    error_log /var/log/nginx/{domain}.ssl.error.log;

    location / {{
        try_files $uri $uri/ =404;
    }}
}}
"#
    )
}

fn site_file(domain: &str) -> std::path::PathBuf {
    std::path::Path::new("/etc/nginx/sites-available").join(format!("{}.conf", domain))
}

/// Absolute log paths for one `error_log` / `access_log` kind in a site config.
/// Skips comments, `off`, non-absolute targets and duplicates.
/// Pure for unit tests.
fn parse_site_log_paths(conf: &str, kind: &str) -> Vec<String> {
    if kind != "error_log" && kind != "access_log" {
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in conf.lines() {
        // Log paths never contain `#`; requires the directive at statement start.
        let code = line.split('#').next().unwrap_or("").trim_start();
        let Some(rest) = code.strip_prefix(kind) else {
            continue;
        };
        // Require a whitespace separator so `error_log_format` never matches.
        if !rest.starts_with([' ', '\t']) {
            continue;
        }
        let rest = rest.trim_start().trim_end().trim_end_matches(';').trim();
        let raw = rest.split_whitespace().next().unwrap_or("").trim();
        let path = raw.trim_matches('"').trim_matches('\'');
        if path.is_empty() || path == "off" || !path.starts_with('/') || path.contains("..") {
            continue;
        }
        if !out.contains(&path.to_string()) {
            out.push(path.to_string());
        }
    }
    out
}

/// Direct (unprivileged, never prompting) read of a site file. Used by the
/// per-row `enabled` probe — hardened perms simply dim the buttons.
fn read_site_conf_direct(domain: &str) -> Option<String> {
    if !valid_site_domain(domain) {
        return None;
    }
    std::fs::read_to_string(site_file(domain)).ok()
}

/// Site log paths for the `enabled` probe (direct read only).
/// Empty when unreadable, unknown kind, or directive absent.
fn site_log_paths_direct(domain: &str, kind: &str) -> Vec<String> {
    match read_site_conf_direct(domain) {
        Some(conf) => parse_site_log_paths(&conf, kind),
        None => Vec::new(),
    }
}

/// Site config text for the viewer loader: direct read first, privileged
/// `cat` fallback for hardened permissions (explicit click — prompting ok).
fn read_site_conf_for_view(domain: &str) -> Option<String> {
    if let Some(conf) = read_site_conf_direct(domain) {
        return Some(conf);
    }
    let path = site_file(domain).to_string_lossy().to_string();
    crate::common::privileged_output(&["cat", &path])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}

fn site_link(domain: &str) -> std::path::PathBuf {
    std::path::Path::new("/etc/nginx/sites-enabled").join(format!("{}.conf", domain))
}

fn staging_path(domain: &str, kind: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("ducknet-site-{}.{}.conf", domain, kind))
}

/// /tmp staging files for the base-config editors (revert guard + save handoff).
fn base_staging_path(which: &str, kind: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("ducknet-base-{}-{}.conf", which, kind))
}

/// ~/.local/backups exists = backup folder ready for browsing.
fn probe_backup_folder() -> bool {
    backup_root_dir().is_dir()
}

/// (nginx_backed_up, phpfpm_backed_up): any *.bak present in each backup dir.
fn probe_base_backups() -> (bool, bool) {
    let nginx_any = !list_base_backups(&base_backup_dir("nginx"), NGINX_BACKUP_PREFIX).is_empty();
    let fpm_any = !list_base_backups(&base_backup_dir("php-fpm"), PHPFPM_BACKUP_PREFIX).is_empty();
    (nginx_any, fpm_any)
}

/// Current PHP-FPM pool user/group + whether both already match the dev
/// user/group. Unknown (no pool file) reads ("", false).
fn probe_phpfpm() -> (String, bool) {
    let dev = dev_username();
    let devgroup = primary_group(&dev).unwrap_or_else(|| dev.clone());
    let Some((pool, _, _)) = php_pool_path() else {
        return (String::new(), false);
    };
    let content = match std::fs::read_to_string(pool) {
        Ok(c) => c,
        Err(_) => return (String::new(), false),
    };
    let (user, group) = parse_pool_user_group(&content);
    let u = user.unwrap_or_default();
    let active = u == dev && group.as_deref() == Some(devgroup.as_str());
    (u, active)
}

impl Default for NginxManagerRust {
    fn default() -> Self {
        let (running, workers, detail) = probe_nginx();
        let (fpm_running, fpm_workers, _) = probe_phpfpm_service();
        let conformed = sites_conformed();
        let user = read_nginx_conf()
            .and_then(|c| parse_nginx_user(&c))
            .unwrap_or_default();
        let dev = dev_username();
        let (fpm_user, fpm_active) = probe_phpfpm();
        let (nginx_backed, fpm_backed) = probe_base_backups();
        let nginx_list = list_base_backups(&base_backup_dir("nginx"), NGINX_BACKUP_PREFIX);
        let fpm_list = list_base_backups(&base_backup_dir("php-fpm"), PHPFPM_BACKUP_PREFIX);
        let pool_path = php_pool_path().map(|(p, _, _)| p.to_string()).unwrap_or_default();
        let folder_ready = probe_backup_folder();
        Self {
            nginx_running: running,
            nginx_status_text: QString::from(if running { "Nginx is up." } else { "Nginx offline" }),
            worker_count: workers,
            phpfpm_running: fpm_running,
            phpfpm_status_text: QString::from(
                if fpm_running { "PHP-FPM is up." } else { "PHP-FPM offline" },
            ),
            phpfpm_worker_count: fpm_workers,
            status_message: QString::from(&detail),
            sites_conformed: conformed,
            nginx_user: QString::from(&user),
            devuser_active: !user.is_empty() && user == dev,
            phpfpm_user: QString::from(&fpm_user),
            phpfpm_devuser_active: fpm_active,
            site_domains: to_qstringlist(&site_domain_list()),
            configured_sites: to_qstringlist(&configured_site_list()),
            site_config_text: QString::default(),
            nginx_base_backed_up: nginx_backed,
            nginx_base_backup_status: QString::from(if nginx_backed { "Backed up" } else { "NO BACKUP!" }),
            phpfpm_base_backed_up: fpm_backed,
            phpfpm_base_backup_status: QString::from(if fpm_backed { "Backed up" } else { "NO BACKUP!" }),
            nginx_base_backups: to_qstringlist(&nginx_list),
            phpfpm_base_backups: to_qstringlist(&fpm_list),
            nginx_base_config_text: QString::default(),
            phpfpm_base_config_text: QString::default(),
            nginx_base_path: QString::from(NGINX_BASE_CONF),
            phpfpm_base_path: QString::from(&pool_path),
            site_backups: QStringList::default(),
            site_backup_domain: QString::default(),
            backup_folder_ready: folder_ready,
            backup_folder_status: QString::from(if folder_ready { "Backups Ready" } else { "No Backups" }),
            nginx_error_log: QString::default(),
            phpfpm_error_log: QString::default(),
            phpfpm_error_log_path: QString::from(&phpfpm_error_log_path()),
            site_log_text: QString::default(),
            site_log_title: QString::default(),
        }
    }
}

impl qobject::NginxManager {
    fn apply_probe(mut self: Pin<&mut Self>) -> bool {
        let (running, workers, detail) = probe_nginx();
        self.as_mut().set_nginx_running(running);
        self.as_mut().set_nginx_status_text(QString::from(
            if running { "Nginx is up." } else { "Nginx offline" },
        ));
        self.as_mut().set_worker_count(workers);
        self.as_mut().set_status_message(QString::from(&detail));
        let (fpm_running, fpm_workers, _) = probe_phpfpm_service();
        self.as_mut().set_phpfpm_running(fpm_running);
        self.as_mut().set_phpfpm_status_text(QString::from(
            if fpm_running { "PHP-FPM is up." } else { "PHP-FPM offline" },
        ));
        self.as_mut().set_phpfpm_worker_count(fpm_workers);
        let conformed = sites_conformed();
        self.as_mut().set_sites_conformed(conformed);
        let user = read_nginx_conf()
            .and_then(|c| parse_nginx_user(&c))
            .unwrap_or_default();
        let dev = dev_username();
        self.as_mut().set_nginx_user(QString::from(&user));
        self.as_mut()
            .set_devuser_active(!user.is_empty() && user == dev);
        let (fpm_user, fpm_active) = probe_phpfpm();
        self.as_mut().set_phpfpm_user(QString::from(&fpm_user));
        self.as_mut().set_phpfpm_devuser_active(fpm_active);
        self.as_mut()
            .set_site_domains(to_qstringlist(&site_domain_list()));
        self.as_mut()
            .set_configured_sites(to_qstringlist(&configured_site_list()));
        self.as_mut().apply_base_probe();
        log_debug(&format!(
            "[ducknet] nginx status: running={} workers={} conformed={} user='{}' ({})",
            running, workers, conformed, user, detail
        ));
        running
    }

    /// Refresh base-config LEDs, backup lists and resolved paths.
    /// Separate from the 5s live probe (backup dirs change rarely).
    fn apply_base_probe(mut self: Pin<&mut Self>) {
        let (nginx_backed, fpm_backed) = probe_base_backups();
        self.as_mut().set_nginx_base_backed_up(nginx_backed);
        self.as_mut().set_nginx_base_backup_status(QString::from(if nginx_backed {
            "Backed up"
        } else {
            "NO BACKUP!"
        }));
        self.as_mut().set_phpfpm_base_backed_up(fpm_backed);
        self.as_mut().set_phpfpm_base_backup_status(QString::from(if fpm_backed {
            "Backed up"
        } else {
            "NO BACKUP!"
        }));
        let nginx_list = list_base_backups(&base_backup_dir("nginx"), NGINX_BACKUP_PREFIX);
        let fpm_list = list_base_backups(&base_backup_dir("php-fpm"), PHPFPM_BACKUP_PREFIX);
        self.as_mut().set_nginx_base_backups(to_qstringlist(&nginx_list));
        self.as_mut().set_phpfpm_base_backups(to_qstringlist(&fpm_list));
        self.as_mut().set_nginx_base_path(QString::from(NGINX_BASE_CONF));
        let pool = php_pool_path().map(|(p, _, _)| p.to_string()).unwrap_or_default();
        self.as_mut().set_phpfpm_base_path(QString::from(&pool));
        let folder_ready = probe_backup_folder();
        self.as_mut().set_backup_folder_ready(folder_ready);
        self.as_mut().set_backup_folder_status(QString::from(if folder_ready {
            "Backups Ready"
        } else {
            "No Backups"
        }));
    }

    fn refresh_status(mut self: Pin<&mut Self>) -> bool {
        self.as_mut().apply_probe();
        true
    }

    fn refresh_base_status(mut self: Pin<&mut Self>) -> bool {
        self.as_mut().apply_base_probe();
        true
    }

    fn refresh_live(mut self: Pin<&mut Self>) -> bool {
        let (running, workers, _) = probe_nginx();
        self.as_mut().set_nginx_running(running);
        self.as_mut().set_nginx_status_text(QString::from(
            if running { "Nginx is up." } else { "Nginx offline" },
        ));
        self.as_mut().set_worker_count(workers);
        let (fpm_running, fpm_workers, _) = probe_phpfpm_service();
        self.as_mut().set_phpfpm_running(fpm_running);
        self.as_mut().set_phpfpm_status_text(QString::from(
            if fpm_running { "PHP-FPM is up." } else { "PHP-FPM offline" },
        ));
        self.as_mut().set_phpfpm_worker_count(fpm_workers);
        true
    }

    /// Run a privileged systemctl action against `service`, then re-probe.
    fn run_service_action(
        mut self: Pin<&mut Self>,
        service: &str,
        action: &str,
        done_word: &str,
        label: &str,
    ) -> bool {
        let res = privileged_output(&["systemctl", action, service]);
        // Re-probe first so LEDs/workers are current, then report the result.
        self.as_mut().apply_probe();
        match res {
            Ok(out) if out.status.success() => {
                let msg = format!("{} {}.", label, done_word);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] systemctl {} {}: ok", action, service));
                true
            }
            Ok(out) => {
                let err = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stderr).trim(),
                    String::from_utf8_lossy(&out.stdout).trim()
                );
                let msg = format!(
                    "systemctl {} failed{} {NEEDS_ROOT}",
                    action,
                    if err.is_empty() { String::new() } else { format!(": {}", err) }
                );
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                false
            }
            Err(e) => {
                let msg = format!("Failed to run systemctl {}: {}", action, e);
                self.as_mut().set_status_message(QString::from(&msg));
                false
            }
        }
    }

    /// Nginx wrapper for run_service_action.
    fn run_action(mut self: Pin<&mut Self>, action: &str, done_word: &str) -> bool {
        self.as_mut()
            .run_service_action("nginx.service", action, done_word, "nginx")
    }

    fn start_nginx(mut self: Pin<&mut Self>) -> bool {
        if self.nginx_running {
            self.as_mut()
                .set_status_message(QString::from("Nginx already running."));
            return true;
        }
        self.as_mut().run_action("start", "started")
    }

    fn stop_nginx(mut self: Pin<&mut Self>) -> bool {
        if !self.nginx_running {
            self.as_mut()
                .set_status_message(QString::from("Nginx already stopped."));
            return true;
        }
        self.as_mut().run_action("stop", "stopped")
    }

    fn restart_nginx(mut self: Pin<&mut Self>) -> bool {
        if !self.nginx_running {
            self.as_mut()
                .set_status_message(QString::from("Nginx is stopped — start it first."));
            return false;
        }
        self.as_mut().run_action("restart", "restarted")
    }

    /// Resolve this distro's PHP-FPM unit or refuse with a message when no
    /// pool file exists (php-fpm not installed).
    fn phpfpm_unit_or_msg(mut self: Pin<&mut Self>) -> Option<String> {
        match phpfpm_service_unit() {
            Some(u) => Some(u.to_string()),
            None => {
                let msg = "php-fpm service not found — is php-fpm installed?".to_string();
                self.as_mut().set_status_message(QString::from(&msg));
                None
            }
        }
    }

    fn start_phpfpm(mut self: Pin<&mut Self>) -> bool {
        if self.phpfpm_running {
            self.as_mut()
                .set_status_message(QString::from("PHP-FPM already running."));
            return true;
        }
        let Some(unit) = self.as_mut().phpfpm_unit_or_msg() else {
            return false;
        };
        self.as_mut()
            .run_service_action(&unit, "start", "started", "PHP-FPM")
    }

    fn stop_phpfpm(mut self: Pin<&mut Self>) -> bool {
        if !self.phpfpm_running {
            self.as_mut()
                .set_status_message(QString::from("PHP-FPM already stopped."));
            return true;
        }
        let Some(unit) = self.as_mut().phpfpm_unit_or_msg() else {
            return false;
        };
        self.as_mut()
            .run_service_action(&unit, "stop", "stopped", "PHP-FPM")
    }

    fn restart_phpfpm(mut self: Pin<&mut Self>) -> bool {
        if !self.phpfpm_running {
            self.as_mut()
                .set_status_message(QString::from("PHP-FPM is stopped — start it first."));
            return false;
        }
        let Some(unit) = self.as_mut().phpfpm_unit_or_msg() else {
            return false;
        };
        self.as_mut()
            .run_service_action(&unit, "restart", "restarted", "PHP-FPM")
    }

    /// Helper: run a privileged command, log it, return (success, trimmed output).
    fn priv_step(argv: &[&str]) -> Result<String, String> {
        match privileged_output(argv) {
            Ok(out) => {
                let combined = format!(
                    "{} {}",
                    String::from_utf8_lossy(&out.stdout).trim(),
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                if out.status.success() {
                    Ok(combined.trim().to_string())
                } else {
                    Err(combined.trim().to_string())
                }
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Conform to Debian sites-available/sites-enabled layout, minus the
    /// `user` change (see run_as_devuser). Idempotent: existing default site,
    /// symlink and .orig backup are never overwritten.
    fn conform_nginx(mut self: Pin<&mut Self>) -> bool {
        const CONF: &str = "/etc/nginx/nginx.conf";
        const AVAIL: &str = "/etc/nginx/sites-available";
        const ENABLED: &str = "/etc/nginx/sites-enabled";
        if !std::path::Path::new(CONF).exists() {
            let msg = "Cannot conform: /etc/nginx/nginx.conf not found — is nginx installed?".to_string();
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let keep_user = read_nginx_conf()
            .and_then(|c| parse_nginx_user(&c))
            .unwrap_or_else(|| "nginx".to_string());
        // 1. Timestamped backup + stable .orig (first run only).
        let stamp = chrono::Local::now().format("%F-%H%M%S").to_string();
        if let Err(e) = Self::priv_step(&["cp", "-a", CONF, &format!("{}.bak-{}", CONF, stamp)]) {
            let msg = format!("Backup failed: {} {NEEDS_ROOT}", e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        if !std::path::Path::new(&format!("{}.orig", CONF)).exists() {
            if let Err(e) = Self::priv_step(&["cp", "-a", CONF, &format!("{}.orig", CONF)]) {
                log_debug(&format!("[ducknet] .orig backup failed (non-fatal): {}", e));
            }
        }
        // 2. Debian-style directories.
        if let Err(e) = Self::priv_step(&["mkdir", "-p", AVAIL, ENABLED]) {
            let msg = format!("mkdir failed: {}", e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let _ = Self::priv_step(&["chmod", "755", AVAIL, ENABLED]);
        // 3. Default site (only if missing — never overwrite customisations).
        if !std::path::Path::new(&format!("{}/default", AVAIL)).exists() {
            let tmp = std::env::temp_dir().join("ducknet-default-site");
            if std::fs::write(&tmp, default_site_content()).is_err() {
                self.as_mut().set_status_message(QString::from("Failed to stage default site file"));
                return false;
            }
            if let Err(e) = Self::priv_step(&["cp", &tmp.to_string_lossy(), &format!("{}/default", AVAIL)]) {
                let msg = format!(" default site install failed: {}", e);
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
            let _ = Self::priv_step(&["chmod", "644", &format!("{}/default", AVAIL)]);
            let _ = std::fs::remove_file(&tmp);
        }
        // 4. Rewrite nginx.conf.
        {
            let tmp = std::env::temp_dir().join("ducknet-nginx.conf");
            if std::fs::write(&tmp, render_nginx_conf(&keep_user)).is_err() {
                self.as_mut().set_status_message(QString::from("Failed to stage nginx.conf"));
                return false;
            }
            if let Err(e) = Self::priv_step(&["cp", &tmp.to_string_lossy(), CONF]) {
                let msg = format!("nginx.conf rewrite failed: {}", e);
                self.as_mut().set_status_message(QString::from(&msg));
                return false;
            }
            let _ = Self::priv_step(&["chmod", "644", CONF]);
            let _ = std::fs::remove_file(&tmp);
        }
        // 5. Enable default site via relative symlink (Debian style), if missing.
        let link = format!("{}/default", ENABLED);
        if std::fs::symlink_metadata(&link).is_err() {
            if let Err(e) = Self::priv_step(&["ln", "-s", "../sites-available/default", &link]) {
                log_debug(&format!("[ducknet] default site symlink failed (non-fatal): {}", e));
            }
        }
        // 6. Test + reload (restart if stopped).
        if let Err(e) = Self::priv_step(&["nginx", "-t"]) {
            let msg = format!("nginx -t FAILED — config not reloaded, restore from {}.bak-{} : {}", CONF, stamp, e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        if Self::priv_step(&["systemctl", "reload", "nginx.service"]).is_err() {
            // Not running (or no systemd): start it.
            if let Err(e) = Self::priv_step(&["systemctl", "restart", "nginx.service"]) {
                let msg = format!("Conformed but nginx reload/restart failed: {}", e);
                self.as_mut().set_status_message(QString::from(&msg));
                self.as_mut().apply_probe();
                return false;
            }
        }
        self.as_mut().apply_probe();
        let msg = "Nginx conformed to sites-available/sites-enabled layout (backup kept).".to_string();
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }

    /// Set nginx.conf `user` to the invoking developer + test + reload.
    fn run_as_devuser(mut self: Pin<&mut Self>) -> bool {
        const CONF: &str = "/etc/nginx/nginx.conf";
        let dev = dev_username();
        let current = read_nginx_conf().and_then(|c| parse_nginx_user(&c));
        if current.as_deref() == Some(dev.as_str()) {
            self.as_mut().set_status_message(QString::from(&format!("Nginx already runs as {}.", dev)));
            return true;
        }
        if read_nginx_conf().is_none() {
            self.as_mut().set_status_message(QString::from("Cannot edit user: /etc/nginx/nginx.conf not found — is nginx installed?"));
            return false;
        }
        // Edits only the `user ...;` line.
        let pattern = format!("s/^user[^;]*;/user {};/", dev);
        if let Err(e) = Self::priv_step(&["sed", "-i", &pattern, CONF]) {
            let msg = format!("user change failed: {} — needs root", e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        if let Err(e) = Self::priv_step(&["nginx", "-t"]) {
            let msg = format!("nginx -t FAILED after user change: {}", e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        if Self::priv_step(&["systemctl", "reload", "nginx.service"]).is_err() {
            if let Err(e) = Self::priv_step(&["systemctl", "restart", "nginx.service"]) {
                let msg = format!("User set to {} but reload/restart failed: {}", dev, e);
                self.as_mut().set_status_message(QString::from(&msg));
                self.as_mut().apply_probe();
                return false;
            }
        }
        self.as_mut().apply_probe();
        let msg = format!("Nginx now runs as {} (reload done).", dev);
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }

    /// Set the PHP-FPM pool `user`/`group` to the invoking developer + group,
    /// gated by `<fpm> -t` with backup restore, then restart the FPM service.
    fn run_phpfpm_as_devuser(mut self: Pin<&mut Self>) -> bool {
        let Some((pool, test_bin, service)) = php_pool_path() else {
            self.as_mut().set_status_message(QString::from(
                "PHP-FPM pool file not found — install NGINX + PHP first (Setup Tooling).",
            ));
            return false;
        };
        let dev = dev_username();
        let devgroup = primary_group(&dev).unwrap_or_else(|| dev.clone());
        let current = std::fs::read_to_string(pool)
            .ok()
            .map(|c| parse_pool_user_group(&c));
        if let Some((ref u, ref g)) = current {
            if u.as_deref() == Some(dev.as_str()) && g.as_deref() == Some(devgroup.as_str()) {
                self.as_mut().set_status_message(QString::from(&format!(
                    "PHP-FPM already runs as {}:{}.",
                    dev, devgroup
                )));
                return true;
            }
        }
        // Backup (best effort) so a failed `-t` restores the working pool.
        let backup = std::env::temp_dir().join("ducknet-pool-www.conf.bak");
        let have_backup = match std::fs::read_to_string(pool) {
            Ok(c) => std::fs::write(&backup, c).is_ok(),
            Err(_) => Self::priv_step(&["cp", "-a", pool, &backup.to_string_lossy()]).is_ok(),
        };
        let sed_user = format!("s/^user = .*/user = {}/", dev);
        let sed_group = format!("s/^group = .*/group = {}/", devgroup);
        if let Err(e) = Self::priv_step(&["sed", "-i", &sed_user, pool])
            .and_then(|_| Self::priv_step(&["sed", "-i", &sed_group, pool]))
        {
            let msg = format!("pool user change failed: {} — needs root", e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        if let Err(e) = Self::priv_step(&[test_bin, "-t"]) {
            if have_backup {
                let _ = Self::priv_step(&["cp", &backup.to_string_lossy(), pool]);
            }
            let msg = format!("{} -t FAILED after user change — reverted: {}", test_bin, e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let _ = std::fs::remove_file(&backup);
        if let Err(e) = Self::priv_step(&["systemctl", "restart", service]) {
            let msg = format!("Pool set to {}:{} but {} restart failed: {}", dev, devgroup, service, e);
            self.as_mut().set_status_message(QString::from(&msg));
            self.as_mut().apply_probe();
            return false;
        }
        self.as_mut().apply_probe();
        let msg = format!("PHP-FPM now runs as {}:{} (restart done).", dev, devgroup);
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }

    /// Test with `nginx -t`, then reload (restart if stopped).
    fn test_and_reload() -> Result<(), String> {
        if let Err(e) = Self::priv_step(&["nginx", "-t"]) {
            return Err(format!("nginx -t FAILED: {}", e));
        }
        if Self::priv_step(&["systemctl", "reload", "nginx.service"]).is_err() {
            Self::priv_step(&["systemctl", "restart", "nginx.service"])
                .map_err(|e| format!("reload/restart failed: {}", e))?;
        }
        Ok(())
    }

    /// Quick-create a site: webroot + conf + symlink, gated by `nginx -t` with rollback.
    /// No auto-reload; the user makes the site live with Start/Restart.
    /// Requires a generated cert; refuses when the site file exists.
    fn quick_create(mut self: Pin<&mut Self>, domain: &QString) -> bool {
        let d = domain.to_string();
        if !valid_site_domain(&d) {
            self.as_mut().set_status_message(QString::from("Invalid domain name."));
            return false;
        }
        if !site_domain_list().contains(&d) {
            let msg = format!("No generated certificate for {} — create it on page 3 first.", d);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        if site_file(&d).exists() {
            self.as_mut().set_status_message(QString::from(&format!("Site {} already exists.", d)));
            return false;
        }
        let certs = certs_dir();
        if !certs.join(format!("{}.crt", d)).exists() || !certs.join(format!("{}.key", d)).exists() {
            let msg = format!("Certificate or key missing for {} — generate it on page 3 first.", d);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let user = dev_username();
        let home = std::env::var("HOME").unwrap_or_else(|_| format!("/home/{}", user));
        // 1. Webroot + index.html.
        let webroot = std::path::Path::new(&home).join("WebRoots").join(&d);
        if let Err(e) = std::fs::create_dir_all(&webroot) {
            let msg = format!("Webroot creation failed: {}", e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let index = webroot.join("index.html");
        if !index.exists() {
            let content = "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"UTF-8\">\n  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n  <title>Site LIVE</title>\n</head>\n<body>\n  <h1>Yes the basic setup of this Site is LIVE</h1>\n</body>\n</html>\n";
            if std::fs::write(&index, content).is_err() {
                self.as_mut().set_status_message(QString::from("Failed to write index.html"));
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&webroot, std::fs::Permissions::from_mode(0o755));
                let _ = std::fs::set_permissions(&index, std::fs::Permissions::from_mode(0o644));
                let _ = std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o711));
            }
        }
        // 2. Site conf (staged in /tmp, installed privileged).
        let tmp = std::env::temp_dir().join(format!("ducknet-site-{}.new.conf", d));
        if std::fs::write(&tmp, render_site_conf(&d, &user)).is_err() {
            self.as_mut().set_status_message(QString::from("Failed to stage site file"));
            return false;
        }
        let dest = site_file(&d).to_string_lossy().to_string();
        if let Err(e) = Self::priv_step(&["cp", &tmp.to_string_lossy(), &dest]) {
            let msg = format!("Site file install failed: {} — needs root", e);
            self.as_mut().set_status_message(QString::from(&msg));
            let _ = std::fs::remove_file(&tmp);
            return false;
        }
        let _ = Self::priv_step(&["chmod", "644", &dest]);
        let _ = std::fs::remove_file(&tmp);
        // 3. Symlink into sites-enabled (absolute target).
        let link = site_link(&d).to_string_lossy().to_string();
        if std::fs::symlink_metadata(&link).is_err() {
            if let Err(e) = Self::priv_step(&["ln", "-s", &dest, &link]) {
                log_debug(&format!("[ducknet] site symlink failed (non-fatal): {}", e));
            }
        }
        // 4. Gate on nginx -t with rollback: never leave a broken site behind.
        if let Err(e) = Self::priv_step(&["nginx", "-t"]) {
            let _ = Self::priv_step(&["rm", "-f", &dest, &link]);
            let msg = format!("nginx -t FAILED — site not created: {}", e);
            self.as_mut().set_status_message(QString::from(&msg));
            self.as_mut().apply_probe();
            return false;
        }
        self.as_mut().apply_probe();
        let msg = format!("Site {} created — press Restart NGINX above to make it live.", d);
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }

    /// Load site text and stage the revert backup.
    fn load_site_config(mut self: Pin<&mut Self>, domain: &QString) -> bool {
        let d = domain.to_string();
        if !valid_site_domain(&d) {
            self.as_mut().set_status_message(QString::from("Invalid domain name."));
            return false;
        }
        let path = site_file(&d);
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                // Site files are root-owned 644 (world-readable); staging copy
                // is owned by us in /tmp.
                if std::fs::write(staging_path(&d, "orig"), &content).is_err() {
                    self.as_mut().set_status_message(QString::from("Failed to stage backup copy"));
                    return false;
                }
                self.as_mut().set_site_config_text(QString::from(&content));
                true
            }
            Err(e) => {
                let msg = format!("Cannot read {}: {}", path.display(), e);
                self.as_mut().set_status_message(QString::from(&msg));
                false
            }
        }
    }

    /// Save site text, gated by `nginx -t` with revert on failure.
    fn save_site_config(mut self: Pin<&mut Self>, domain: &QString, text: &QString) -> bool {
        let d = domain.to_string();
        if !valid_site_domain(&d) {
            self.as_mut().set_status_message(QString::from("Invalid domain name."));
            return false;
        }
        let path = site_file(&d);
        if !path.exists() {
            self.as_mut().set_status_message(QString::from("Site file no longer exists."));
            return false;
        }
        // Refresh the pristine backup so revert restores pre-save state.
        if let Ok(current) = std::fs::read_to_string(&path) {
            let _ = std::fs::write(staging_path(&d, "orig"), &current);
        }
        let new_tmp = staging_path(&d, "new");
        if std::fs::write(&new_tmp, text.to_string()).is_err() {
            self.as_mut().set_status_message(QString::from("Failed to stage edited file"));
            return false;
        }
        let dest = path.to_string_lossy().to_string();
        if Self::priv_step(&["cp", &new_tmp.to_string_lossy(), &dest]).is_err() {
            self.as_mut().set_status_message(QString::from(format!("Save failed {NEEDS_ROOT}")));
            return false;
        }
        if Self::priv_step(&["nginx", "-t"]).is_err() {
            let orig = staging_path(&d, "orig").to_string_lossy().to_string();
            let _ = Self::priv_step(&["cp", &orig, &dest]);
            let msg = "Your site file is not valid, reverting change.".to_string();
            self.as_mut().set_status_message(QString::from(&msg));
            log_debug(&format!("[ducknet] site save for {} failed nginx -t — reverted", d));
            return false;
        }
        if let Err(e) = Self::test_and_reload() {
            let msg = format!("Saved but reload failed: {}", e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let _ = std::fs::remove_file(&new_tmp);
        let _ = std::fs::remove_file(staging_path(&d, "orig"));
        let msg = format!("Site {} saved and reloaded.", d);
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }

    /// Discard /tmp staging files when the editor is cancelled.
    fn cancel_site_edit(mut self: Pin<&mut Self>, domain: &QString) -> bool {
        let d = domain.to_string();
        let _ = std::fs::remove_file(staging_path(&d, "orig"));
        let _ = std::fs::remove_file(staging_path(&d, "new"));
        self.as_mut().set_site_config_text(QString::default());
        true
    }

    fn lint_site_draft(&self, text: &QString) -> QString {
        QString::from(&crate::nginxlint::lint_site_draft(&text.to_string()))
    }

    /// Delete a site: enabled symlink + sites-available file first (so nginx
    /// never points at a removed root), then the ~/WebRoots folder, then
    /// test + reload to apply.
    fn delete_site(mut self: Pin<&mut Self>, domain: &QString) -> bool {
        let d = domain.to_string();
        if !valid_site_domain(&d) {
            self.as_mut().set_status_message(QString::from("Invalid domain name."));
            return false;
        }
        let file = site_file(&d).to_string_lossy().to_string();
        let link = site_link(&d).to_string_lossy().to_string();
        if !site_file(&d).exists() && std::fs::symlink_metadata(&link).is_err() {
            self.as_mut().set_status_message(QString::from(&format!("Site {} does not exist.", d)));
            return false;
        }
        // 1. Remove symlink + site file (privileged).
        if let Err(e) = Self::priv_step(&["rm", "-f", &link, &file]) {
            let msg = format!("Site removal failed: {} — needs root", e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        // 2. Remove the webroot.
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            let webroot = std::path::Path::new(&home).join("WebRoots").join(&d);
            if webroot.exists() {
                if let Err(e) = std::fs::remove_dir_all(&webroot) {
                    log_debug(&format!("[ducknet] webroot removal failed (non-fatal): {}", e));
                }
            }
        }
        // 3. Validate the remaining config + reload to apply.
        if let Err(e) = Self::test_and_reload() {
            let msg = format!("Site files removed but reload failed: {}", e);
            self.as_mut().set_status_message(QString::from(&msg));
            self.as_mut().apply_probe();
            return false;
        }
        self.as_mut().apply_probe();
        let msg = format!("Site {} deleted (config + webroot) and nginx reloaded.", d);
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }

    fn backup_nginx_base(mut self: Pin<&mut Self>) -> bool {
        match do_base_backup(
            NGINX_BASE_CONF,
            &base_backup_dir("nginx"),
            NGINX_BACKUP_PREFIX,
            &Self::priv_step,
        ) {
            Ok(name) => {
                self.as_mut().apply_base_probe();
                let msg = format!("NGINX base config backed up as {}.", name);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                true
            }
            Err(e) => {
                let msg = format!("NGINX backup failed: {}", e);
                self.as_mut().set_status_message(QString::from(&msg));
                false
            }
        }
    }

    fn backup_phpfpm_base(mut self: Pin<&mut Self>) -> bool {
        let Some((pool, _, _)) = php_pool_path() else {
            self.as_mut().set_status_message(QString::from(
                "PHP-FPM pool file not found — install NGINX + PHP first (Setup Tooling).",
            ));
            return false;
        };
        match do_base_backup(
            pool,
            &base_backup_dir("php-fpm"),
            PHPFPM_BACKUP_PREFIX,
            &Self::priv_step,
        ) {
            Ok(name) => {
                self.as_mut().apply_base_probe();
                let msg = format!("PHP-FPM base config backed up as {}.", name);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                true
            }
            Err(e) => {
                let msg = format!("PHP-FPM backup failed: {}", e);
                self.as_mut().set_status_message(QString::from(&msg));
                false
            }
        }
    }

    fn refresh_nginx_backups(mut self: Pin<&mut Self>) -> bool {
        let list = list_base_backups(&base_backup_dir("nginx"), NGINX_BACKUP_PREFIX);
        let backed = !list.is_empty();
        self.as_mut().set_nginx_base_backups(to_qstringlist(&list));
        self.as_mut().set_nginx_base_backed_up(backed);
        self.as_mut().set_nginx_base_backup_status(QString::from(if backed {
            "Backed up"
        } else {
            "NO BACKUP!"
        }));
        true
    }

    fn refresh_phpfpm_backups(mut self: Pin<&mut Self>) -> bool {
        let list = list_base_backups(&base_backup_dir("php-fpm"), PHPFPM_BACKUP_PREFIX);
        let backed = !list.is_empty();
        self.as_mut().set_phpfpm_base_backups(to_qstringlist(&list));
        self.as_mut().set_phpfpm_base_backed_up(backed);
        self.as_mut().set_phpfpm_base_backup_status(QString::from(if backed {
            "Backed up"
        } else {
            "NO BACKUP!"
        }));
        true
    }

    /// Shared restore: copy ~/.local/backups/<kind>/<file> over `dest`
    /// (privileged), stripping the timestamp/`.bak` by writing to the live path.
    fn restore_base_file(
        mut self: Pin<&mut Self>,
        kind: &str,
        prefix: &str,
        dest: &str,
        filename: &str,
        label: &str,
    ) -> Result<(), String> {
        if !valid_backup_filename(filename, prefix) {
            return Err("Invalid backup selection.".to_string());
        }
        let src = base_backup_dir(kind).join(filename);
        if !src.exists() {
            self.as_mut().apply_base_probe();
            return Err(format!("Backup {} no longer exists.", filename));
        }
        let src_s = src.to_string_lossy().to_string();
        Self::priv_step(&["cp", "-a", &src_s, dest])
            .map(|_| ())
            .map_err(|e| format!("Restore copy failed: {} — needs root", e))?;
        log_debug(&format!("[ducknet] {} backup {} restored to {}", label, filename, dest));
        Ok(())
    }

    fn restore_nginx_backup(mut self: Pin<&mut Self>, filename: &QString) -> bool {
        let name = filename.to_string();
        if !std::path::Path::new(NGINX_BASE_CONF).exists() {
            self.as_mut().set_status_message(QString::from(
                "Cannot restore: /etc/nginx/nginx.conf not found — is nginx installed?",
            ));
            return false;
        }
        if let Err(e) =
            self.as_mut().restore_base_file("nginx", NGINX_BACKUP_PREFIX, NGINX_BASE_CONF, &name, "NGINX")
        {
            self.as_mut().set_status_message(QString::from(&e));
            return false;
        }
        if let Err(e) = Self::priv_step(&["nginx", "-t"]) {
            let msg = format!("Restored {} but nginx -t FAILED: {}", name, e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        if let Err(e) = Self::test_and_reload() {
            let msg = format!("Restored {} but reload failed: {}", name, e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        self.as_mut().apply_probe();
        let msg = format!("NGINX base config restored from {} and reloaded.", name);
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }

    fn restore_phpfpm_backup(mut self: Pin<&mut Self>, filename: &QString) -> bool {
        let name = filename.to_string();
        let Some((pool, test_bin, service)) = php_pool_path() else {
            self.as_mut().set_status_message(QString::from(
                "PHP-FPM pool file not found — install NGINX + PHP first (Setup Tooling).",
            ));
            return false;
        };
        if let Err(e) =
            self.as_mut().restore_base_file("php-fpm", PHPFPM_BACKUP_PREFIX, pool, &name, "PHP-FPM")
        {
            self.as_mut().set_status_message(QString::from(&e));
            return false;
        }
        if let Err(e) = Self::priv_step(&[test_bin, "-t"]) {
            let msg = format!("Restored {} but {} -t FAILED: {}", name, test_bin, e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        if let Err(e) = Self::priv_step(&["systemctl", "restart", service]) {
            let msg = format!("Restored {} but {} restart failed: {}", name, service, e);
            self.as_mut().set_status_message(QString::from(&msg));
            self.as_mut().apply_probe();
            return false;
        }
        self.as_mut().apply_probe();
        let msg = format!("PHP-FPM base config restored from {} and restarted.", name);
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }

    /// Load a base file for editing: sets *ConfigText + stages /tmp pristine.
    fn load_base_file(
        mut self: Pin<&mut Self>,
        which: &str,
        dest: &str,
        label: &str,
    ) -> Option<String> {
        match std::fs::read_to_string(dest) {
            Ok(content) => {
                if std::fs::write(base_staging_path(which, "orig"), &content).is_err() {
                    self.as_mut().set_status_message(QString::from("Failed to stage backup copy"));
                    return None;
                }
                log_debug(&format!("[ducknet] {} base config loaded from {}", label, dest));
                Some(content)
            }
            Err(e) => {
                // Direct read normally works (644); privileged fallback for hardened perms.
                if let Ok(out) = privileged_output(&["cat", dest]) {
                    if out.status.success() {
                        let content = String::from_utf8_lossy(&out.stdout).to_string();
                        if std::fs::write(base_staging_path(which, "orig"), &content).is_ok() {
                            return Some(content);
                        }
                    }
                }
                let msg = format!("Cannot read {}: {}", dest, e);
                self.as_mut().set_status_message(QString::from(&msg));
                None
            }
        }
    }

    fn load_nginx_base_config(mut self: Pin<&mut Self>) -> bool {
        if !std::path::Path::new(NGINX_BASE_CONF).exists() {
            self.as_mut().set_status_message(QString::from(
                "Cannot edit: /etc/nginx/nginx.conf not found — is nginx installed?",
            ));
            return false;
        }
        match self.as_mut().load_base_file("nginx", NGINX_BASE_CONF, "NGINX") {
            Some(content) => {
                self.as_mut().set_nginx_base_config_text(QString::from(&content));
                true
            }
            None => false,
        }
    }

    /// Save edited base text: commit, gate on the service test, revert from
    /// the pristine /tmp backup on failure so a typo can never crash the service.
    fn save_base_text(which: &str, dest: &str, text: &str) -> Result<(), String> {
        if let Ok(current) = std::fs::read_to_string(dest) {
            let _ = std::fs::write(base_staging_path(which, "orig"), &current);
        }
        let new_tmp = base_staging_path(which, "new");
        std::fs::write(&new_tmp, text)
            .map_err(|_| "Failed to stage edited file".to_string())?;
        Self::priv_step(&["cp", &new_tmp.to_string_lossy(), dest])
            .map_err(|_| format!("Save failed {NEEDS_ROOT}"))?;
        Ok(())
    }

    fn save_nginx_base_config(mut self: Pin<&mut Self>, text: &QString) -> bool {
        if !std::path::Path::new(NGINX_BASE_CONF).exists() {
            self.as_mut().set_status_message(QString::from(
                "Cannot save: /etc/nginx/nginx.conf not found — is nginx installed?",
            ));
            return false;
        }
        if Self::save_base_text("nginx", NGINX_BASE_CONF, &text.to_string()).is_err() {
            self.as_mut().set_status_message(QString::from(
                format!("Save failed {NEEDS_ROOT}"),
            ));
            return false;
        }
        if Self::priv_step(&["nginx", "-t"]).is_err() {
            let orig = base_staging_path("nginx", "orig").to_string_lossy().to_string();
            let _ = Self::priv_step(&["cp", &orig, NGINX_BASE_CONF]);
            let msg = "Your nginx.conf is not valid, reverting change.".to_string();
            self.as_mut().set_status_message(QString::from(&msg));
            log_debug("[ducknet] nginx base save failed nginx -t — reverted");
            return false;
        }
        if let Err(e) = Self::test_and_reload() {
            let msg = format!("Saved but reload failed: {}", e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let _ = std::fs::remove_file(base_staging_path("nginx", "new"));
        let _ = std::fs::remove_file(base_staging_path("nginx", "orig"));
        let msg = "NGINX base config saved and reloaded.".to_string();
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }

    fn cancel_nginx_base_edit(mut self: Pin<&mut Self>) -> bool {
        let _ = std::fs::remove_file(base_staging_path("nginx", "orig"));
        let _ = std::fs::remove_file(base_staging_path("nginx", "new"));
        self.as_mut().set_nginx_base_config_text(QString::default());
        true
    }

    fn load_phpfpm_base_config(mut self: Pin<&mut Self>) -> bool {
        let Some((pool, _, _)) = php_pool_path() else {
            self.as_mut().set_status_message(QString::from(
                "PHP-FPM pool file not found — install NGINX + PHP first (Setup Tooling).",
            ));
            return false;
        };
        match self.as_mut().load_base_file("phpfpm", pool, "PHP-FPM") {
            Some(content) => {
                self.as_mut().set_phpfpm_base_config_text(QString::from(&content));
                true
            }
            None => false,
        }
    }

    fn save_phpfpm_base_config(mut self: Pin<&mut Self>, text: &QString) -> bool {
        let Some((pool, test_bin, service)) = php_pool_path() else {
            self.as_mut().set_status_message(QString::from(
                "PHP-FPM pool file not found — install NGINX + PHP first (Setup Tooling).",
            ));
            return false;
        };
        if Self::save_base_text("phpfpm", pool, &text.to_string()).is_err() {
            self.as_mut().set_status_message(QString::from(
                format!("Save failed {NEEDS_ROOT}"),
            ));
            return false;
        }
        if let Err(e) = Self::priv_step(&[test_bin, "-t"]) {
            let orig = base_staging_path("phpfpm", "orig").to_string_lossy().to_string();
            let _ = Self::priv_step(&["cp", &orig, pool]);
            let msg = format!("Your www.conf is not valid ({} -t failed), reverting change: {}", test_bin, e);
            self.as_mut().set_status_message(QString::from(&msg));
            log_debug("[ducknet] php-fpm base save failed -t — reverted");
            return false;
        }
        if let Err(e) = Self::priv_step(&["systemctl", "restart", service]) {
            let msg = format!("Saved but {} restart failed: {}", service, e);
            self.as_mut().set_status_message(QString::from(&msg));
            self.as_mut().apply_probe();
            return false;
        }
        let _ = std::fs::remove_file(base_staging_path("phpfpm", "new"));
        let _ = std::fs::remove_file(base_staging_path("phpfpm", "orig"));
        self.as_mut().apply_probe();
        let msg = "PHP-FPM base config saved and restarted.".to_string();
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }

    fn delete_nginx_backup(mut self: Pin<&mut Self>, filename: &QString) -> bool {
        let name = filename.to_string();
        match delete_backup_file(&base_backup_dir("nginx"), NGINX_BACKUP_PREFIX, &name) {
            Ok(()) => {
                self.as_mut().refresh_nginx_backups();
                let msg = format!("NGINX backup {} deleted.", name);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                true
            }
            Err(e) => {
                self.as_mut().set_status_message(QString::from(&e));
                false
            }
        }
    }

    fn delete_phpfpm_backup(mut self: Pin<&mut Self>, filename: &QString) -> bool {
        let name = filename.to_string();
        match delete_backup_file(&base_backup_dir("php-fpm"), PHPFPM_BACKUP_PREFIX, &name) {
            Ok(()) => {
                self.as_mut().refresh_phpfpm_backups();
                let msg = format!("PHP-FPM backup {} deleted.", name);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                true
            }
            Err(e) => {
                self.as_mut().set_status_message(QString::from(&e));
                false
            }
        }
    }

    fn delete_site_backup(mut self: Pin<&mut Self>, domain: &QString, filename: &QString) -> bool {
        let d = domain.to_string();
        let name = filename.to_string();
        if !valid_site_domain(&d) {
            self.as_mut().set_status_message(QString::from("Invalid domain name."));
            return false;
        }
        match delete_backup_file(&site_backup_dir(&d), &d, &name) {
            Ok(()) => {
                self.as_mut().refresh_site_backups(domain);
                let msg = format!("Site {} backup {} deleted.", d, name);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                true
            }
            Err(e) => {
                self.as_mut().set_status_message(QString::from(&e));
                false
            }
        }
    }

    fn cancel_phpfpm_base_edit(mut self: Pin<&mut Self>) -> bool {
        let _ = std::fs::remove_file(base_staging_path("phpfpm", "orig"));
        let _ = std::fs::remove_file(base_staging_path("phpfpm", "new"));
        self.as_mut().set_phpfpm_base_config_text(QString::default());
        true
    }

    fn open_backup_folder(mut self: Pin<&mut Self>) -> bool {
        let dir = backup_root_dir();
        if !dir.is_dir() {
            self.as_mut().set_status_message(QString::from("No Backups"));
            return false;
        }
        match Command::new("xdg-open").arg(&dir).spawn() {
            Ok(_) => {
                log_debug(&format!("[ducknet] opened backup folder {}", dir.display()));
                true
            }
            Err(e) => {
                let msg = format!("Failed to open backup folder: {}", e);
                self.as_mut().set_status_message(QString::from(&msg));
                false
            }
        }
    }

    fn load_nginx_error_log(mut self: Pin<&mut Self>) -> bool {
        match read_log_capped(NGINX_ERROR_LOG, "nginx") {
            Ok(text) => {
                self.as_mut().set_nginx_error_log(QString::from(&text));
                log_debug("[ducknet] nginx error log loaded");
                true
            }
            Err(msg) => {
                self.as_mut().set_nginx_error_log(QString::default());
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                false
            }
        }
    }

    fn load_phpfpm_error_log(mut self: Pin<&mut Self>) -> bool {
        let path = phpfpm_error_log_path();
        self.as_mut().set_phpfpm_error_log_path(QString::from(&path));
        match read_log_capped(&path, "php-fpm") {
            Ok(text) => {
                self.as_mut().set_phpfpm_error_log(QString::from(&text));
                log_debug(&format!("[ducknet] php-fpm error log loaded from {}", path));
                true
            }
            Err(msg) => {
                self.as_mut().set_phpfpm_error_log(QString::default());
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                false
            }
        }
    }

    fn site_has_site_log(&self, domain: &QString, kind: &QString) -> bool {
        !site_log_paths_direct(&domain.to_string(), &kind.to_string()).is_empty()
    }

    fn load_site_log(mut self: Pin<&mut Self>, domain: &QString, kind: &QString) -> bool {
        let d = domain.to_string();
        let k = kind.to_string();
        if !valid_site_domain(&d) {
            self.as_mut().set_status_message(QString::from("Invalid domain name."));
            return false;
        }
        if k != "error_log" && k != "access_log" {
            self.as_mut().set_status_message(QString::from("Unknown log kind."));
            return false;
        }
        let Some(conf) = read_site_conf_for_view(&d) else {
            let msg = format!("Cannot read site config for {} {NEEDS_ROOT}", d);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        };
        let paths = parse_site_log_paths(&conf, &k);
        if paths.is_empty() {
            let msg = format!("Site {} has no {} directive — add one to the site config first.", d, k);
            self.as_mut().set_status_message(QString::from(&msg));
            self.as_mut().set_site_log_text(QString::default());
            self.as_mut().set_site_log_title(QString::from(&format!("{} — {} (none)", d, k)));
            return false;
        }
        // Caps both logs for the dialog; unreadable files become inline notices.
        let mut sections = Vec::new();
        let mut loaded = 0;
        for p in &paths {
            match read_log_capped(p, "nginx") {
                Ok(text) => {
                    loaded += 1;
                    if paths.len() == 1 {
                        sections.push(text);
                    } else {
                        sections.push(format!("==> {} <==\n{}", p, text));
                    }
                }
                Err(e) => {
                    sections.push(format!("==> {} <==\n({})", p, e));
                }
            }
        }
        if loaded == 0 {
            let msg = format!("No {} log for {} could be read {NEEDS_ROOT}", k, d);
            self.as_mut().set_status_message(QString::from(&msg));
            log_debug(&format!("[ducknet] {}", msg));
            return false;
        }
        let title = if paths.len() == 1 {
            format!("{} — {}", d, paths[0])
        } else {
            format!("{} — {} ({} files)", d, k, paths.len())
        };
        self.as_mut().set_site_log_title(QString::from(&title));
        self.as_mut().set_site_log_text(QString::from(&sections.join("\n\n")));
        log_debug(&format!("[ducknet] site {} {} log loaded ({} of {} files)", d, k, loaded, paths.len()));
        true
    }

    fn backup_site_config(mut self: Pin<&mut Self>, domain: &QString) -> bool {
        let d = domain.to_string();
        if !valid_site_domain(&d) {
            self.as_mut().set_status_message(QString::from("Invalid domain name."));
            return false;
        }
        let src = site_file(&d).to_string_lossy().to_string();
        if !site_file(&d).exists() {
            self.as_mut().set_status_message(QString::from(&format!("Site {} does not exist.", d)));
            return false;
        }
        match do_base_backup(&src, &site_backup_dir(&d), &d, &Self::priv_step) {
            Ok(name) => {
                // Keep the restore dialog in sync when it targets this site.
                self.as_mut().refresh_site_backups(domain);
                let msg = format!("Site {} backed up as {}.", d, name);
                self.as_mut().set_status_message(QString::from(&msg));
                log_debug(&format!("[ducknet] {}", msg));
                true
            }
            Err(e) => {
                let msg = format!("Site backup failed: {}", e);
                self.as_mut().set_status_message(QString::from(&msg));
                false
            }
        }
    }

    fn refresh_site_backups(mut self: Pin<&mut Self>, domain: &QString) -> bool {
        let d = domain.to_string();
        if !valid_site_domain(&d) {
            return false;
        }
        let list = list_base_backups(&site_backup_dir(&d), &d);
        self.as_mut().set_site_backups(to_qstringlist(&list));
        self.as_mut().set_site_backup_domain(QString::from(&d));
        true
    }

    fn restore_site_backup(
        mut self: Pin<&mut Self>,
        domain: &QString,
        filename: &QString,
    ) -> bool {
        let d = domain.to_string();
        let name = filename.to_string();
        if !valid_site_domain(&d) {
            self.as_mut().set_status_message(QString::from("Invalid domain name."));
            return false;
        }
        // Validated domain doubles as prefix; cannot escape its backup folder.
        if !valid_backup_filename(&name, &d) {
            self.as_mut().set_status_message(QString::from("Invalid backup selection."));
            return false;
        }
        let src = site_backup_dir(&d).join(&name);
        if !src.exists() {
            self.as_mut().refresh_site_backups(domain);
            self.as_mut().set_status_message(QString::from(&format!(
                "Backup {} no longer exists.",
                name
            )));
            return false;
        }
        let dest = site_file(&d).to_string_lossy().to_string();
        // Stage the current file so a failed `nginx -t` can revert.
        let had_current = std::fs::read_to_string(site_file(&d))
            .map(|c| std::fs::write(staging_path(&d, "orig"), c).is_ok())
            .unwrap_or(false);
        let src_s = src.to_string_lossy().to_string();
        if Self::priv_step(&["cp", "-a", &src_s, &dest]).is_err() {
            self.as_mut().set_status_message(QString::from(
                format!("Restore copy failed {NEEDS_ROOT}"),
            ));
            return false;
        }
        // Re-ensure the symlink; a restore can resurrect a removed site.
        let link = site_link(&d).to_string_lossy().to_string();
        if std::fs::symlink_metadata(&link).is_err() {
            let _ = Self::priv_step(&["ln", "-s", &dest, &link]);
        }
        if Self::priv_step(&["nginx", "-t"]).is_err() {
            if had_current {
                let orig = staging_path(&d, "orig").to_string_lossy().to_string();
                let _ = Self::priv_step(&["cp", &orig, &dest]);
            }
            let msg = format!("Restored {} but nginx -t FAILED — reverted.", name);
            self.as_mut().set_status_message(QString::from(&msg));
            log_debug(&format!("[ducknet] site restore for {} failed nginx -t — reverted", d));
            return false;
        }
        if let Err(e) = Self::test_and_reload() {
            let msg = format!("Restored {} but reload failed: {}", name, e);
            self.as_mut().set_status_message(QString::from(&msg));
            return false;
        }
        let _ = std::fs::remove_file(staging_path(&d, "orig"));
        self.as_mut().apply_probe();
        let msg = format!("Site {} restored from {} and reloaded.", d, name);
        self.as_mut().set_status_message(QString::from(&msg));
        log_debug(&format!("[ducknet] {}", msg));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Neutral fixture user; never the real dev user.
    const TEST_USER: &str = "testdev";

    const ACTIVE_SAMPLE: &str = "● nginx.service - A high performance web server\n   \
        Loaded: loaded (/lib/systemd/system/nginx.service; enabled)\n   \
        Active: active (running) since Tue 2026-09-08 14:00:00 SAST; 1 day ago\n \
        Main PID: 1234 (nginx)\n      Tasks: 5 (limit: 100)\n     \
        CGroup: /system.slice/nginx.service\n             \
        ├─1234 \"nginx: master process /usr/sbin/nginx -g daemon on; master_process on;\"\n             \
        ├─1235 \"nginx: worker process\"\n             \
        ├─1236 \"nginx: worker process\"\n             \
        ├─1237 \"nginx: worker process\"\n             \
        └─1238 \"nginx: worker process\"\n";

    const INACTIVE_SAMPLE: &str = "○ nginx.service - A high performance web server\n   \
        Loaded: loaded (/lib/systemd/system/nginx.service; disabled)\n   \
        Active: inactive (dead)\n";

    #[test]
    fn status_active_counts_workers() {
        let (running, workers) = parse_nginx_status(ACTIVE_SAMPLE);
        assert!(running);
        assert_eq!(workers, 4);
    }

    #[test]
    fn status_inactive_no_workers() {
        let (running, workers) = parse_nginx_status(INACTIVE_SAMPLE);
        assert!(!running);
        assert_eq!(workers, 0);
    }

    #[test]
    fn status_empty_means_offline() {
        let (running, workers) = parse_nginx_status("");
        assert!(!running);
        assert_eq!(workers, 0);
    }

    #[test]
    fn phpfpm_status_counts_pool_workers() {
        let sample = "● php8.4-fpm.service - The PHP 8.4 FastCGI Process Manager\n   \
            Loaded: loaded (/usr/lib/systemd/system/php8.4-fpm.service; enabled)\n   \
            Active: active (running) since Sat 2026-09-12 15:33:27 SAST; 58min ago\n     \
            Main PID: 876 (php-fpm8.4)\n     \
            CGroup: /system.slice/php8.4-fpm.service\n             \
            ├─876 \"php-fpm: master process (/etc/php/8.4/fpm/php-fpm.conf)\"\n             \
            ├─988 \"php-fpm: pool www\"\n             \
            ├─989 \"php-fpm: pool www\"\n             \
            └─990 \"php-fpm: pool www\"\n";
        let (running, workers) = parse_phpfpm_status(sample);
        assert!(running);
        // Master process line must not count — pools only.
        assert_eq!(workers, 3);
    }

    #[test]
    fn phpfpm_status_inactive_and_empty() {
        let (running, workers) = parse_phpfpm_status("○ php-fpm.service\n   Active: inactive (dead)\n");
        assert!(!running);
        assert_eq!(workers, 0);
        let (running, workers) = parse_phpfpm_status("");
        assert!(!running);
        assert_eq!(workers, 0);
    }

    #[test]
    fn phpfpm_probe_runs_unprivileged_on_this_host() {
        // Liveness shape only; never prompts.
        let (running, workers, detail) = probe_phpfpm_service();
        assert!(running);
        assert!(workers > 0);
        assert!(detail.contains("active (running)"));
        assert_eq!(phpfpm_service_unit(), Some("php8.4-fpm.service"));
    }

    #[test]
    fn nginx_user_parsing() {
        assert_eq!(
            parse_nginx_user("user nginx;\nworker_processes auto;\n").as_deref(),
            Some("nginx")
        );
        assert_eq!(
            parse_nginx_user(&format!("events {{}}\n  user {};\n", TEST_USER)).as_deref(),
            Some(TEST_USER)
        );
        // Commented-out directive must not match.
        assert_eq!(parse_nginx_user("#user nginx;\nworker_processes auto;\n"), None);
        assert_eq!(parse_nginx_user("worker_processes auto;\n"), None);
    }

    #[test]
    fn nginx_conf_render_keeps_user_and_layout() {
        let conf = render_nginx_conf(TEST_USER);
        assert!(conf.contains(&format!("user {};", TEST_USER)), "user line missing");
        assert!(!conf.contains("user nginx;"), "default user leaked in");
        assert!(!conf.contains("@@USER@@"), "template placeholder leaked");
        assert!(conf.contains("include /etc/nginx/sites-enabled/*;"));
        assert!(conf.contains("include /etc/nginx/conf.d/*.conf;"));
        // No inline server block (sites carry the vhosts).
        assert!(!conf.lines().any(|l| l.trim_start().starts_with("server {")));
    }

    #[test]
    fn pool_user_group_parsing() {
        let debian = "[www]\nuser = www-data\ngroup = www-data\nlisten = /run/php/php8.4-fpm.sock\n";
        assert_eq!(
            parse_pool_user_group(debian),
            (Some("www-data".to_string()), Some("www-data".to_string()))
        );
        let fedora = "; RPM pool\n[www]\nuser = apache\ngroup = apache\n";
        assert_eq!(
            parse_pool_user_group(fedora),
            (Some("apache".to_string()), Some("apache".to_string()))
        );
        // Comments, spacing variants and section headers are skipped.
        let messy = format!("[www]\n;user = nobody\nuser= {}\ngroup={}\n# group = root\n", TEST_USER, TEST_USER);
        assert_eq!(
            parse_pool_user_group(&messy),
            (Some(TEST_USER.to_string()), Some(TEST_USER.to_string()))
        );
        // Look-alike keys must not false-match.
        let trap = "[www]\nusername = sneaky\nlisten.owner = www-data\n";
        assert_eq!(parse_pool_user_group(trap), (None, None));
        // Empty file.
        assert_eq!(parse_pool_user_group(""), (None, None));
    }

    #[test]
    fn dev_username_never_empty() {
        assert!(!dev_username().is_empty());
    }

    #[test]
    fn site_domain_validation() {
        assert!(valid_site_domain("test.test"));
        assert!(valid_site_domain("my-app_1.test"));
        assert!(!valid_site_domain(""));
        assert!(!valid_site_domain("../evil"));
        assert!(!valid_site_domain("a/b"));
        assert!(!valid_site_domain("a;b"));
    }

    #[test]
    fn site_conf_render_shape() {
        let conf = render_site_conf("test.test", TEST_USER);
        // Paths follow $HOME, else /home/<user>.
        let home = std::env::var("HOME").unwrap_or_else(|_| format!("/home/{}", TEST_USER));
        assert!(conf.contains("server_name test.test www.test.test;"));
        assert!(conf.contains(&format!("root {}/WebRoots/test.test;", home)));
        assert!(conf.contains(&format!("ssl_certificate {}/certs/test.test.crt;", home)));
        assert!(conf.contains(&format!("ssl_certificate_key {}/certs/test.test.key;", home)));
        assert!(conf.contains("listen 127.0.0.1:80;"));
        assert!(conf.contains("listen 127.0.0.1:443 ssl;"));
        assert!(conf.contains("try_files $uri $uri/ =404;"));
    }

    #[test]
    fn base_backup_filename_validation() {
        assert!(valid_backup_filename("nginx-13-09-2026-10-00-00.conf.bak", "nginx"));
        assert!(valid_backup_filename("www-13-09-2026-10-00-00.conf.bak", "www"));
        assert!(!valid_backup_filename("www-13-09-2026-10-00-00.conf.bak", "nginx"));
        assert!(!valid_backup_filename("../evil.conf.bak", "nginx"));
        assert!(!valid_backup_filename("nginx-evil/bad.conf.bak", "nginx"));
        assert!(!valid_backup_filename("nginx.conf", "nginx"));
        assert!(!valid_backup_filename("", "nginx"));
    }

    #[test]
    fn base_backup_list_orders_most_recent_first() {
        let dir = std::env::temp_dir().join(format!("ducknet-test-backups-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("nginx-01-01-2025-00-00-00.conf.bak");
        let new = dir.join("nginx-13-09-2026-10-00-00.conf.bak");
        std::fs::write(&old, "old").unwrap();
        // Ensure distinct mtimes (filesystem granularity).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&new, "new").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignore me").unwrap();
        let list = list_base_backups(&dir, "nginx");
        assert_eq!(
            list,
            vec![
                "nginx-13-09-2026-10-00-00.conf.bak".to_string(),
                "nginx-01-01-2025-00-00-00.conf.bak".to_string()
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn base_backup_roundtrip_mirrors_shell_scripts() {
        let root = std::env::temp_dir().join(format!("ducknet-test-bak-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("nginx.conf");
        let dest_dir = root.join("backups").join("nginx");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, "user nginx;\n").unwrap();
        let ok = |_: &[&str]| -> Result<String, String> { Err("no priv in test".to_string()) };
        let name = do_base_backup(&src.to_string_lossy(), &dest_dir, "nginx", &ok).unwrap();
        assert!(valid_backup_filename(&name, "nginx"));
        assert!(dest_dir.join(&name).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn site_backup_dir_is_per_domain() {
        let dir = site_backup_dir("test.test");
        assert!(dir.ends_with("sites/test.test"));
        // Join stays a single component; callers validate the domain.
        assert_eq!(dir.file_name().and_then(|s| s.to_str()), Some("test.test"));
    }

    #[test]
    fn site_backup_filename_uses_dotted_prefix() {
        assert!(valid_backup_filename("test.test-13-09-2026-10-00-00.conf.bak", "test.test"));
        assert!(!valid_backup_filename("test.test-13-09-2026-10-00-00.conf.bak", "other.test"));
        // Cross-site escape rejected: traversal never passes validation.
        assert!(!valid_backup_filename("../nginx-13-09-2026-10-00-00.conf.bak", "test.test"));
    }

    #[test]
    fn site_backup_roundtrip_in_domain_folder() {
        let root = std::env::temp_dir().join(format!("ducknet-test-sitebak-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("test.test.conf");
        let dest_dir = root.join("backups").join("sites").join("test.test");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, "server {}\n").unwrap();
        let ok = |_: &[&str]| -> Result<String, String> { Err("no priv in test".to_string()) };
        let name = do_base_backup(&src.to_string_lossy(), &dest_dir, "test.test", &ok).unwrap();
        assert!(valid_backup_filename(&name, "test.test"));
        assert_eq!(list_base_backups(&dest_dir, "test.test"), vec![name.clone()]);
        // Other domains see nothing (isolation between site folders).
        assert!(list_base_backups(&dest_dir, "other.test").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn backup_delete_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ducknet-test-del-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = "nginx-13-09-2026-10-00-00.conf.bak";
        std::fs::write(dir.join(name), "data").unwrap();
        assert!(delete_backup_file(&dir, "nginx", name).is_ok());
        assert!(!dir.join(name).exists());
        // Second delete fails (already gone).
        assert!(delete_backup_file(&dir, "nginx", name).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_delete_rejects_bad_names() {
        let dir = std::env::temp_dir();
        assert!(delete_backup_file(&dir, "nginx", "../evil.conf.bak").is_err());
        assert!(delete_backup_file(&dir, "nginx", "nginx.conf").is_err());
        assert!(delete_backup_file(&dir, "nginx", "www-13-09-2026-10-00-00.conf.bak").is_err());
        // Dotted site prefix cannot delete another site's file.
        assert!(delete_backup_file(&dir, "test.test", "other.test-13-09-2026-10-00-00.conf.bak").is_err());
    }

    #[test]
    fn base_backup_missing_source_errors() {
        let ok = |_: &[&str]| -> Result<String, String> { Ok(String::new()) };
        let err = do_base_backup(
            "/nonexistent-ducknet/nginx.conf",
            &std::env::temp_dir(),
            "nginx",
            &ok,
        )
        .unwrap_err();
        assert!(err.contains("source not found"));
    }

    #[test]
    fn nginx_error_log_path_shape() {
        // Viewer source path only; tail-cap covered in common.rs.
        assert_eq!(NGINX_ERROR_LOG, "/var/log/nginx/error.log");
    }

    #[test]
    fn site_log_paths_parse_per_kind() {
        let conf = render_site_conf("test.test", TEST_USER);
        // Template carries plain + ssl logs per kind, deduped in order.
        assert_eq!(
            parse_site_log_paths(&conf, "error_log"),
            vec![
                "/var/log/nginx/test.test.error.log".to_string(),
                "/var/log/nginx/test.test.ssl.error.log".to_string()
            ]
        );
        assert_eq!(
            parse_site_log_paths(&conf, "access_log"),
            vec![
                "/var/log/nginx/test.test.access.log".to_string(),
                "/var/log/nginx/test.test.ssl.access.log".to_string()
            ]
        );
        // Custom single-directive file.
        let single = "server {\n    access_log /var/log/nginx/custom.access.log main;\n}\n";
        assert_eq!(
            parse_site_log_paths(single, "access_log"),
            vec!["/var/log/nginx/custom.access.log".to_string()]
        );
        assert!(parse_site_log_paths(single, "error_log").is_empty());
        // Absent directives (customised file) -> empty, unknown kind -> empty.
        assert!(parse_site_log_paths("server {\n listen 80;\n}\n", "error_log").is_empty());
        assert!(parse_site_log_paths(&conf, "ssl_certificate").is_empty());
        // `off`, syslog targets, comments and look-alike names never surface.
        let tricky = "server {\n    access_log off;\n    error_log syslog:server=unix:/dev/log;\n    # error_log /var/log/nginx/commented.log;\n    error_log_format detailed;\n}\n";
        assert!(parse_site_log_paths(tricky, "access_log").is_empty());
        assert!(parse_site_log_paths(tricky, "error_log").is_empty());
    }

    #[test]
    fn fpm_error_log_directive_parsing() {        assert_eq!(
            parse_fpm_error_log("[global]\nerror_log = /var/log/php-fpm/error.log\n"),
            Some("/var/log/php-fpm/error.log".to_string())
        );
        // Comments, spacing and inline comments tolerated.
        assert_eq!(
            parse_fpm_error_log("; comment\n  error_log=/var/log/php8.4-fpm.log ; inline\n"),
            Some("/var/log/php8.4-fpm.log".to_string())
        );
        assert_eq!(parse_fpm_error_log("[global]\n"), None);
        assert_eq!(parse_fpm_error_log(""), None);
        // Resolved path is never empty (directive or conventional fallback).
        assert!(!phpfpm_error_log_path().is_empty());
    }
}
