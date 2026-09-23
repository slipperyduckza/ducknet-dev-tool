// SPDX-License-Identifier: MIT
//! System Info backend: host details for the System Info page.
//!
//! Live values refresh via `refreshLive()`; one-shot values load once via `refreshAll()`.

use core::pin::Pin;
use cxx_qt_lib::{QString, QStringList};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use sysinfo::{Disks, System};

use crate::common::{family, family_str, find_binary, os_release_field, to_qstringlist};

/// 1. Hostname: proc file first (no subprocess), then fallbacks.
fn read_hostname() -> String {
    if let Ok(s) = fs::read_to_string("/proc/sys/kernel/hostname") {
        let t = s.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    if let Ok(out) = Command::new("hostname").output() {
        if out.status.success() {
            let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !t.is_empty() {
                return t;
            }
        }
    }
    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string())
}

/// 2. Platform: PRETTY_NAME, fallback NAME+VERSION_ID.
fn read_platform() -> String {
    let pretty = os_release_field("PRETTY_NAME");
    if !pretty.is_empty() {
        return pretty;
    }
    let name = os_release_field("NAME");
    let ver = os_release_field("VERSION_ID");
    let combined = format!("{} {}", name, ver).trim().to_string();
    if combined.is_empty() {
        "unknown".to_string()
    } else {
        combined
    }
}

fn read_distro_id() -> String {
    let id = crate::common::distro_id();
    if !id.is_empty() {
        id
    } else {
        "unknown".to_string()
    }
}

fn read_distro_family() -> String {
    family_str(&family()).to_string()
}

/// 3. Locale: LC_ALL, then LANG, then first LANGUAGE entry.
fn read_locale() -> String {
    for key in ["LC_ALL", "LANG"] {
        let v = std::env::var(key).unwrap_or_default();
        let v = v.trim().to_string();
        if !v.is_empty() {
            return v;
        }
    }
    let lang = std::env::var("LANGUAGE").unwrap_or_default();
    let first = lang.split(':').next().unwrap_or("").trim().to_string();
    if !first.is_empty() {
        return first;
    }
    "C".to_string()
}

/// 4a. Timezone: /etc/localtime symlink target under zoneinfo, else /etc/timezone.
fn read_timezone() -> String {
    if let Ok(target) = fs::read_link("/etc/localtime") {
        let s = target.to_string_lossy().to_string();
        if let Some(pos) = s.find("zoneinfo/") {
            let tz = s[pos + "zoneinfo/".len()..].to_string();
            if !tz.is_empty() {
                return tz;
            }
        }
        // Non-symlink layout: last two path components often are Area/City.
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() >= 2 {
            let guess = format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1]);
            if !guess.contains('.') {
                return guess;
            }
        }
    }
    if let Ok(s) = fs::read_to_string("/etc/timezone") {
        let t = s.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    "local".to_string()
}

/// 4b. Current local date + time.
fn read_date_time() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S %Z").to_string()
}

/// 5. Primary IP: first IPv4 on a physical interface in kernel order.
/// Skips VPN/tunnel/container interfaces; default route follows VPN when active.
fn read_primary_ip() -> String {
    if let Ok(out) = Command::new("ip").args(["-o", "-4", "addr", "show"]).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).to_string();
            if let Some(ip) = parse_physical_ipv4(&s) {
                return ip;
            }
        }
    }
    // Fallbacks for exotic setups (no physical IPv4, e.g. IPv6-only LAN).
    if let Ok(out) = Command::new("ip")
        .args(["-4", "route", "get", "1.1.1.1"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).to_string();
            // "... src 192.168.1.20 uid ..."
            let tokens: Vec<&str> = s.split_whitespace().collect();
            for (i, t) in tokens.iter().enumerate() {
                if *t == "src" && i + 1 < tokens.len() {
                    return tokens[i + 1].to_string();
                }
            }
        }
    }
    if let Ok(out) = Command::new("hostname").args(["-I"]).output() {
        if out.status.success() {
            let first = String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            if !first.is_empty() {
                return first;
            }
        }
    }
    String::new()
}

/// Interfaces excluded from primary NIC: tunnels/VPNs, PPP, bridges, veth.
/// Matches case-insensitively; strips `@suffix` first.
fn is_virtual_iface(ifname: &str) -> bool {
    let base = ifname.split('@').next().unwrap_or(ifname).to_lowercase();
    if base == "lo" {
        return true;
    }
    const EXACT: &[&str] = &["br0", "docker0", "lxcbr0", "virbr0"];
    if EXACT.contains(&base.as_str()) {
        return true;
    }
    const PREFIX: &[&str] = &[
        "tun", "tap", "utun", "wg", "ppp", "slip", // tunnels
        "tailscale", "mullvad", "proton", "nordlynx", "ivpn", "warp", "cloudflare",
        "zerotier", "zt", "express", "surfshark", // VPN products (CloudflareWARP, nordlynx, ...)
        "docker", "br-", "veth", "virbr", "lxcbr", "vboxnet", "vmnet", "anbox",
        "cali", "flannel", "cni", "kube-", // bridges / container networking
    ];
    PREFIX.iter().any(|p| base.starts_with(p))
}

/// First IPv4 on non-virtual, non-loopback interface from `ip -o -4 addr show`.
/// Deprioritizes link-local (169.254/16): routable wins, link-local still beats VPN.
fn parse_physical_ipv4(output: &str) -> Option<String> {
    let mut link_local_fallback: Option<String> = None;
    for line in output.lines() {
        let mut fields = line.split_whitespace();
        let _idx = fields.next()?;
        let ifname = fields.next()?;
        let family = fields.next()?;
        let cidr = fields.next()?;
        if family != "inet" {
            continue;
        }
        let ip = cidr.split('/').next().unwrap_or("");
        if ip.is_empty() || ip.starts_with("127.") {
            continue;
        }
        if is_virtual_iface(ifname) {
            continue;
        }
        if ip.starts_with("169.254.") {
            if link_local_fallback.is_none() {
                link_local_fallback = Some(ip.to_string());
            }
            continue;
        }
        return Some(ip.to_string());
    }
    link_local_fallback
}

/// Absolute path of nginx binary via shared layered probe.
fn nginx_binary() -> Option<std::path::PathBuf> {
    find_binary(
        "nginx",
        &[
            "/usr/sbin/nginx",       // Debian / Ubuntu
            "/usr/local/sbin/nginx", // source builds
            "/usr/bin/nginx",        // some Fedoras / containers
            "/opt/nginx/sbin/nginx",
        ],
    )
}

/// Extract `nginx/x.y.z` from `nginx -v` output.
fn parse_nginx_version_line(line: &str) -> String {
    let line = line.trim();
    if line.is_empty() {
        return "not installed".to_string();
    }
    if let Some(pos) = line.find("nginx/") {
        return line[pos..].split_whitespace().next().unwrap_or(line).to_string();
    }
    line.to_string()
}

/// 6. Nginx version: `<binary> -v` writes to stderr ("nginx version: nginx/1.26.2").
fn read_nginx_version() -> String {
    let bin = match nginx_binary() {
        Some(b) => b,
        None => return "not installed".to_string(),
    };
    match Command::new(&bin).arg("-v").output() {
        Err(_) => "not installed".to_string(),
        Ok(out) => {
            let combined = format!(
                "{} {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            parse_nginx_version_line(combined.lines().next().unwrap_or(""))
        }
    }
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0 / 1024.0
}

fn sample_memory() -> (f64, f64, f64) {
    let mut sys = System::new();
    sys.refresh_memory();
    let total = sys.total_memory(); // bytes in sysinfo 0.30+
    let used = sys.used_memory();
    let pct = if total > 0 {
        used as f64 * 100.0 / total as f64
    } else {
        0.0
    };
    (gib(total), gib(used), pct)
}

/// Live CPU % from /proc/stat deltas; previous counters live in process-wide slot.
/// First call primes slot and returns 0.0; later ticks yield real values.
static CPU_PREV: std::sync::Mutex<Option<(u64, u64)>> = std::sync::Mutex::new(None);

fn read_cpu_counters() -> Option<(u64, u64)> {
    let content = fs::read_to_string("/proc/stat").ok()?;
    let line = content.lines().next()?;
    let mut nums = line
        .split_whitespace()
        .skip(1)
        .filter_map(|t| t.parse::<u64>().ok());
    let user = nums.next()?;
    let nice = nums.next()?;
    let system = nums.next()?;
    let idle = nums.next()?;
    let iowait = nums.next().unwrap_or(0);
    let irq = nums.next().unwrap_or(0);
    let softirq = nums.next().unwrap_or(0);
    let steal = nums.next().unwrap_or(0);
    let total = user + nice + system + idle + iowait + irq + softirq + steal;
    Some((total, idle + iowait))
}

fn live_cpu_percent() -> f64 {
    let cur = read_cpu_counters();
    let mut prev = CPU_PREV.lock().unwrap();
    match (cur, *prev) {
        (Some((total, idle)), Some((ptotal, pidle))) => {
            *prev = Some((total, idle));
            let dtotal = total.saturating_sub(ptotal);
            let didle = idle.saturating_sub(pidle);
            if dtotal == 0 {
                0.0
            } else {
                ((dtotal - didle) as f64 * 100.0 / dtotal as f64).clamp(0.0, 100.0)
            }
        }
        (Some(cur), None) => {
            *prev = Some(cur);
            0.0
        }
        _ => 0.0,
    }
}

fn read_cpu_cores() -> i32 {
    if let Some(n) = System::physical_core_count() {
        return n as i32;
    }
    std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(1)
}

/// 9. Storage for `/` (one-shot): total + used GiB and percent.
fn sample_storage() -> (String, f64, f64, f64) {
    let disks = Disks::new_with_refreshed_list();
    // Prefer the `/` mount; fall back to the largest filesystem.
    let mut pick: Option<(u64, u64)> = None; // (total, available)
    let mut largest: Option<(u64, u64)> = None;
    for d in disks.list() {
        let total = d.total_space();
        let avail = d.available_space();
        if total == 0 {
            continue;
        }
        if largest.map(|(t, _)| total > t).unwrap_or(true) {
            largest = Some((total, avail));
        }
        if d.mount_point().to_string_lossy() == "/" {
            pick = Some((total, avail));
        }
    }
    let (total, avail) = pick.or(largest).unwrap_or((0, 0));
    if total == 0 {
        return ("/".to_string(), 0.0, 0.0, 0.0);
    }
    let used = total.saturating_sub(avail);
    let pct = used as f64 * 100.0 / total as f64;
    ("/".to_string(), gib(total), gib(used), pct)
}

fn mem_text(used_gb: f64, total_gb: f64, pct: f64) -> String {
    format!("{:.1} / {:.1} GiB ({:.0}%)", used_gb, total_gb, pct)
}

fn storage_text(path: &str, used_gb: f64, total_gb: f64, pct: f64) -> String {
    format!("{} — {:.1} / {:.1} GiB ({:.0}%)", path, used_gb, total_gb, pct)
}

/// Path to plasma-localerc (DUCKNET_PLASMA_LOCALERC overrides for tests).
fn plasma_localerc_path() -> PathBuf {
    let o = std::env::var("DUCKNET_PLASMA_LOCALERC").unwrap_or_default();
    if !o.is_empty() {
        return PathBuf::from(o);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".config/plasma-localerc")
}

/// Available locales from `localectl list-locales`, falling back to
/// `locale -a` when localectl is missing/empty (containers, tests).
fn list_available_locales() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(res) = Command::new("localectl").arg("list-locales").output() {
        if res.status.success() {
            for line in String::from_utf8_lossy(&res.stdout).lines() {
                let t = line.trim().to_string();
                if !t.is_empty() {
                    out.push(t);
                }
            }
        }
    }
    if out.is_empty() {
        if let Ok(res) = Command::new("locale").arg("-a").output() {
            if res.status.success() {
                for line in String::from_utf8_lossy(&res.stdout).lines() {
                    let t = line.trim().to_string();
                    if !t.is_empty() {
                        out.push(t);
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Current Plasma Formats locale: `[Formats] LANG=` from plasma-localerc.
fn read_plasma_locale() -> String {
    let path = plasma_localerc_path();
    let content = fs::read_to_string(&path).unwrap_or_default();
    let mut section = String::new();
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t.to_string();
            continue;
        }
        if section == "[Formats]" {
            if let Some(v) = t.strip_prefix("LANG=") {
                let v = v.trim().to_string();
                if !v.is_empty() {
                    return v;
                }
            }
        }
    }
    String::new()
}

fn is_valid_locale_name(s: &str) -> bool {
    if s.is_empty() || s.len() > 64 {
        return false;
    }
    // Allows en_ZA.UTF-8 shape only; blocks paths, spaces, shell/INI metachars.
    for c in s.chars() {
        if !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '+' | '@')) {
            return false;
        }
    }
    true
}

/// Write `LANG=` under `[Formats]` and `LANGUAGE=` under `[Translations]`,
/// preserving all other keys/comments. Creates the file if missing.
fn write_plasma_locale(locale: &str) -> Result<(), String> {
    let locale = locale.trim().to_string();
    if !is_valid_locale_name(&locale) {
        return Err(format!("Invalid locale '{}'", locale));
    }
    let path = plasma_localerc_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    }
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let has_file = path.exists();
    let mut lines: Vec<String> = if has_file {
        existing.lines().map(|l| l.to_string()).collect()
    } else {
        Vec::new()
    };
    let mut has_formats = lines.iter().any(|l| l.trim() == "[Formats]");
    let mut has_translations = lines.iter().any(|l| l.trim() == "[Translations]");
    if !has_formats {
        if !lines.is_empty() && !lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            lines.push(String::new());
        }
        lines.push("[Formats]".to_string());
        has_formats = true;
    }
    if !has_translations {
        if !lines.is_empty() && !lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            lines.push(String::new());
        }
        lines.push("[Translations]".to_string());
        has_translations = true;
    }
    let _ = (has_formats, has_translations);
    let mut section = String::new();
    let mut formats_done = false;
    let mut translations_done = false;
    for line in lines.iter_mut() {
        let t = line.trim().to_string();
        if t.starts_with('[') && t.ends_with(']') {
            section = t;
            continue;
        }
        if section == "[Formats]" && (t.starts_with("LANG=") || t.starts_with("LANG =")) {
            *line = format!("LANG={}", locale);
            formats_done = true;
        } else if section == "[Translations]"
            && (t.starts_with("LANGUAGE=") || t.starts_with("LANGUAGE ="))
        {
            *line = format!("LANGUAGE={}", locale);
            translations_done = true;
        }
    }
    if !formats_done {
        if let Some(i) = lines.iter().position(|l| l.trim() == "[Formats]") {
            lines.insert(i + 1, format!("LANG={}", locale));
        }
    }
    if !translations_done {
        if let Some(i) = lines.iter().position(|l| l.trim() == "[Translations]") {
            lines.insert(i + 1, format!("LANGUAGE={}", locale));
        }
    }
    let mut content = lines.join("\n");
    content.push('\n');
    fs::write(&path, &content)
        .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    Ok(())
}

fn system_actions_disabled() -> bool {
    matches!(
        std::env::var("DUCKNET_NO_SYSTEM_ACTIONS").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

fn do_reboot() -> Result<(), String> {
    if system_actions_disabled() {
        return Ok(());
    }
    // `systemctl reboot` talks to logind — no sudo needed in a Plasma session.
    match Command::new("systemctl").arg("reboot").status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("systemctl reboot exited with {}", s)),
        Err(e) => Err(format!("Failed to run systemctl reboot: {}", e)),
    }
}

fn do_logout() -> Result<(), String> {
    if system_actions_disabled() {
        return Ok(());
    }
    let mut errors: Vec<String> = Vec::new();
    // KDE logout via ksmserver; `logout 0 0 0` means no-confirm, logout, default mode.
    for bin in ["qdbus6", "qdbus"] {
        match Command::new(bin)
            .args([
                "org.kde.ksmserver",
                "/KSMServer",
                "logout",
                "0",
                "0",
                "0",
            ])
            .status()
        {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => errors.push(format!("{} exited with {}", bin, s)),
            Err(e) => errors.push(format!("{} not available: {}", bin, e)),
        }
    }
    // 2. End the current logind session (works on Wayland/X11).
    if let Ok(sid) = std::env::var("XDG_SESSION_ID") {
        if !sid.is_empty() {
            match Command::new("loginctl")
                .args(["terminate-session", &sid])
                .status()
            {
                Ok(s) if s.success() => return Ok(()),
                Ok(s) => errors.push(format!("loginctl terminate-session exited with {}", s)),
                Err(e) => errors.push(format!("loginctl failed: {}", e)),
            }
        }
    }
    // 3. Last resort: terminate all sessions of this user.
    let user = std::env::var("USER").unwrap_or_default();
    if !user.is_empty() {
        match Command::new("loginctl")
            .args(["terminate-user", &user])
            .status()
        {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => errors.push(format!("loginctl terminate-user exited with {}", s)),
            Err(e) => errors.push(format!("loginctl failed: {}", e)),
        }
    } else {
        errors.push("USER is unset, cannot terminate-user".to_string());
    }
    Err(errors.join("; "))
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
        #[qproperty(QString, hostname, cxx_name = "hostname")]
        #[qproperty(QString, platform, cxx_name = "platform")]
        #[qproperty(QString, distro_family, cxx_name = "distroFamily")]
        #[qproperty(QString, distro_id, cxx_name = "distroId")]
        #[qproperty(QString, locale, cxx_name = "locale")]
        #[qproperty(QString, plasma_locale, cxx_name = "plasmaLocale")]
        #[qproperty(QStringList, available_locales, cxx_name = "availableLocales")]
        #[qproperty(QString, locale_status, cxx_name = "localeStatus")]
        #[qproperty(QString, timezone, cxx_name = "timezone")]
        #[qproperty(QString, date_time, cxx_name = "dateTime")]
        #[qproperty(QString, primary_ip, cxx_name = "primaryIp")]
        #[qproperty(QString, nginx_version, cxx_name = "nginxVersion")]
        #[qproperty(QString, mem_text, cxx_name = "memText")]
        #[qproperty(f64, mem_usage_percent, cxx_name = "memUsagePercent")]
        #[qproperty(f64, mem_used_gb, cxx_name = "memUsedGb")]
        #[qproperty(f64, mem_total_gb, cxx_name = "memTotalGb")]
        #[qproperty(i32, cpu_cores, cxx_name = "cpuCores")]
        #[qproperty(f64, cpu_usage_percent, cxx_name = "cpuUsagePercent")]
        #[qproperty(QString, storage_path, cxx_name = "storagePath")]
        #[qproperty(QString, storage_text, cxx_name = "storageText")]
        #[qproperty(f64, storage_usage_percent, cxx_name = "storageUsagePercent")]
        #[qproperty(f64, storage_used_gb, cxx_name = "storageUsedGb")]
        #[qproperty(f64, storage_total_gb, cxx_name = "storageTotalGb")]
        type SystemInfoManager = super::SystemInfoManagerRust;

        /// Refresh live values only (memory, CPU, clock). Called by QML Timer.
        #[qinvokable]
        #[cxx_name = "refreshLive"]
        fn refresh_live(self: Pin<&mut Self>) -> bool;

        /// Refresh everything (also one-shot: storage, nginx, host identity).
        #[qinvokable]
        #[cxx_name = "refreshAll"]
        fn refresh_all(self: Pin<&mut Self>) -> bool;

        /// Reload `availableLocales` from `localectl list-locales` (+ plasma locale).
        #[qinvokable]
        #[cxx_name = "refreshLocales"]
        fn refresh_locales(self: Pin<&mut Self>) -> bool;

        /// Write the chosen locale to `~/.config/plasma-localerc`
        /// ([Formats] LANG + [Translations] LANGUAGE).
        #[qinvokable]
        #[cxx_name = "applyPlasmaLocale"]
        fn apply_plasma_locale(self: Pin<&mut Self>, locale: &QString) -> bool;

        /// Reboot the system (`systemctl reboot`).
        #[qinvokable]
        #[cxx_name = "rebootSystem"]
        fn reboot_system(self: Pin<&mut Self>) -> bool;

        /// Log out of the Plasma session (ksmserver, else loginctl).
        #[qinvokable]
        #[cxx_name = "logoutSession"]
        fn logout_session(self: Pin<&mut Self>) -> bool;
    }
}

pub struct SystemInfoManagerRust {
    hostname: QString,
    platform: QString,
    distro_family: QString,
    distro_id: QString,
    locale: QString,
    plasma_locale: QString,
    available_locales: QStringList,
    locale_status: QString,
    timezone: QString,
    date_time: QString,
    primary_ip: QString,
    nginx_version: QString,
    mem_text: QString,
    mem_usage_percent: f64,
    mem_used_gb: f64,
    mem_total_gb: f64,
    cpu_cores: i32,
    cpu_usage_percent: f64,
    storage_path: QString,
    storage_text: QString,
    storage_usage_percent: f64,
    storage_used_gb: f64,
    storage_total_gb: f64,
}

impl qobject::SystemInfoManager {
    fn refresh_live(mut self: Pin<&mut Self>) -> bool {
        self.as_mut().set_date_time(QString::from(&read_date_time()));
        let (total, used, pct) = sample_memory();
        self.as_mut().set_mem_total_gb(total);
        self.as_mut().set_mem_used_gb(used);
        self.as_mut().set_mem_usage_percent(pct);
        self.as_mut()
            .set_mem_text(QString::from(&mem_text(used, total, pct)));
        self.as_mut().set_cpu_usage_percent(live_cpu_percent());
        true
    }

    fn refresh_all(mut self: Pin<&mut Self>) -> bool {
        self.as_mut().set_hostname(QString::from(&read_hostname()));
        self.as_mut().set_platform(QString::from(&read_platform()));
        self.as_mut()
            .set_distro_family(QString::from(&read_distro_family()));
        self.as_mut()
            .set_distro_id(QString::from(&read_distro_id()));
        self.as_mut().set_locale(QString::from(&read_locale()));
        self.as_mut()
            .set_plasma_locale(QString::from(&read_plasma_locale()));
        self.as_mut()
            .set_available_locales(to_qstringlist(&list_available_locales()));
        self.as_mut().set_timezone(QString::from(&read_timezone()));
        self.as_mut()
            .set_primary_ip(QString::from(&read_primary_ip()));
        self.as_mut()
            .set_nginx_version(QString::from(&read_nginx_version()));
        self.as_mut().set_cpu_cores(read_cpu_cores());
        self.as_mut().set_date_time(QString::from(&read_date_time()));
        let (total, used, pct) = sample_memory();
        self.as_mut().set_mem_total_gb(total);
        self.as_mut().set_mem_used_gb(used);
        self.as_mut().set_mem_usage_percent(pct);
        self.as_mut()
            .set_mem_text(QString::from(&mem_text(used, total, pct)));
        let (path, stotal, sused, spct) = sample_storage();
        self.as_mut().set_storage_path(QString::from(&path));
        self.as_mut().set_storage_total_gb(stotal);
        self.as_mut().set_storage_used_gb(sused);
        self.as_mut().set_storage_usage_percent(spct);
        self.as_mut()
            .set_storage_text(QString::from(&storage_text(&path, sused, stotal, spct)));
        // Prime the /proc/stat sampler so the first paint shows a real value.
        live_cpu_percent();
        std::thread::sleep(Duration::from_millis(250));
        self.as_mut().set_cpu_usage_percent(live_cpu_percent());
        true
    }

    fn refresh_locales(mut self: Pin<&mut Self>) -> bool {
        let locales = list_available_locales();
        self.as_mut()
            .set_available_locales(to_qstringlist(&locales));
        self.as_mut()
            .set_plasma_locale(QString::from(&read_plasma_locale()));
        if locales.is_empty() {
            self.as_mut().set_locale_status(QString::from(
                "No locales found (localectl list-locales / locale -a returned nothing)",
            ));
            return false;
        }
        self.as_mut().set_locale_status(QString::from(""));
        true
    }

    fn apply_plasma_locale(mut self: Pin<&mut Self>, locale: &QString) -> bool {
        let selected = locale.to_string();
        match write_plasma_locale(&selected) {
            Ok(_) => {
                self.as_mut()
                    .set_plasma_locale(QString::from(&selected));
                self.as_mut().set_locale_status(QString::from(&format!(
                    "Plasma locale set to {} — logout or reboot to apply",
                    selected
                )));
                true
            }
            Err(e) => {
                self.as_mut().set_locale_status(QString::from(&e));
                false
            }
        }
    }

    fn reboot_system(mut self: Pin<&mut Self>) -> bool {
        match do_reboot() {
            Ok(_) => {
                self.as_mut()
                    .set_locale_status(QString::from("Rebooting…"));
                true
            }
            Err(e) => {
                self.as_mut().set_locale_status(QString::from(&e));
                false
            }
        }
    }

    fn logout_session(mut self: Pin<&mut Self>) -> bool {
        match do_logout() {
            Ok(_) => {
                self.as_mut()
                    .set_locale_status(QString::from("Logging out…"));
                true
            }
            Err(e) => {
                self.as_mut().set_locale_status(QString::from(&e));
                false
            }
        }
    }
}

impl Default for SystemInfoManagerRust {
    fn default() -> Self {
        let (mtotal, mused, mpct) = sample_memory();
        let (spath, stotal, sused, spct) = sample_storage();
        // Prime the /proc/stat sampler so the first paint shows a real value.
        live_cpu_percent();
        std::thread::sleep(Duration::from_millis(250));
        let cpu = live_cpu_percent();
        Self {
            hostname: QString::from(&read_hostname()),
            platform: QString::from(&read_platform()),
            distro_family: QString::from(&read_distro_family()),
            distro_id: QString::from(&read_distro_id()),
            locale: QString::from(&read_locale()),
            plasma_locale: QString::from(&read_plasma_locale()),
            available_locales: to_qstringlist(&list_available_locales()),
            locale_status: QString::from(""),
            timezone: QString::from(&read_timezone()),
            date_time: QString::from(&read_date_time()),
            primary_ip: QString::from(&read_primary_ip()),
            nginx_version: QString::from(&read_nginx_version()),
            mem_text: QString::from(&mem_text(mused, mtotal, mpct)),
            mem_usage_percent: mpct,
            mem_used_gb: mused,
            mem_total_gb: mtotal,
            cpu_cores: read_cpu_cores(),
            cpu_usage_percent: cpu,
            storage_path: QString::from(&spath),
            storage_text: QString::from(&storage_text(&spath, sused, stotal, spct)),
            storage_usage_percent: spct,
            storage_used_gb: sused,
            storage_total_gb: stotal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gib_math() {
        assert!((gib(1024 * 1024 * 1024) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn nginx_version_line_parsing() {
        assert_eq!(parse_nginx_version_line("nginx version: nginx/1.26.2"), "nginx/1.26.2");
        assert_eq!(parse_nginx_version_line("  nginx version: nginx/1.24.0  "), "nginx/1.26.2".replace("26.2", "24.0"));
        assert_eq!(parse_nginx_version_line(""), "not installed");
        assert_eq!(parse_nginx_version_line("   "), "not installed");
    }

    #[test]
    fn whereis_parsing_ignores_non_binaries() {
        // `whereis -b` lists man/data dirs too — only the `nginx` file counts.
        // Binary may exist on dev hosts; assert shape, not fixed outcome.
        let out = "nginx: /usr/share/nginx /usr/share/man/man8/nginx.8.gz";
        assert!(crate::common::parse_whereis_bin(out, "nginx").is_none());
        let out = "nginx:";
        assert!(crate::common::parse_whereis_bin(out, "nginx").is_none());
    }

    #[test]
    fn nginx_detected_on_this_host() {
        // Host provides nginx; version must resolve.
        let bin = nginx_binary();
        assert!(bin.is_some(), "nginx binary should be found on this host");
        assert_ne!(read_nginx_version(), "not installed");
    }

    #[test]
    fn mem_text_format() {
        assert_eq!(mem_text(1.5, 3.0, 50.0), "1.5 / 3.0 GiB (50%)");
    }

    #[test]
    fn locale_never_empty() {
        assert!(!read_locale().is_empty());
    }

    #[test]
    fn hostname_never_empty() {
        assert!(!read_hostname().is_empty());
    }

    #[test]
    fn platform_parses_override() {
        std::env::set_var(
            "DUCKNET_OS_RELEASE",
            "/tmp/ducknet-test-os-release-sysinfo",
        );
        fs::write(
            "/tmp/ducknet-test-os-release-sysinfo",
            "NAME=\"Fedora Linux\"\nVERSION_ID=44\nPRETTY_NAME=\"Fedora Linux 44 (KDE)\"\nID=fedora\nID_LIKE=\"rhel centos fedora\"\n",
        )
        .unwrap();
        assert_eq!(read_platform(), "Fedora Linux 44 (KDE)");
        assert_eq!(read_distro_family(), "fedora");
        assert_eq!(read_distro_id(), "fedora");
        std::env::remove_var("DUCKNET_OS_RELEASE");
        let _ = fs::remove_file("/tmp/ducknet-test-os-release-sysinfo");
    }

    #[test]
    fn samples_return_sane_values() {
        let (total, used, pct) = sample_memory();
        assert!(total > 0.0 && used <= total && pct >= 0.0 && pct <= 100.0);
        assert!(read_cpu_cores() >= 1);
        let (_, st, su, sp) = sample_storage();
        assert!(st >= 0.0 && su <= st && sp >= 0.0 && sp <= 100.0);
    }

    #[test]
    fn locale_name_validation() {
        assert!(is_valid_locale_name("en_ZA.UTF-8"));
        assert!(is_valid_locale_name("C.UTF-8"));
        assert!(is_valid_locale_name("af_ZA.UTF-8"));
        assert!(!is_valid_locale_name(""));
        assert!(!is_valid_locale_name("en_ZA.UTF-8; rm -rf /"));
        assert!(!is_valid_locale_name("../../etc/passwd"));
        assert!(!is_valid_locale_name("en ZA"));
    }

    #[test]
    fn plasma_localerc_roundtrip() {
        let tmp = "/tmp/ducknet-test-plasma-localerc";
        std::env::set_var("DUCKNET_PLASMA_LOCALERC", tmp);
        let _ = fs::remove_file(tmp);
        // Fresh file: creates both sections.
        write_plasma_locale("en_ZA.UTF-8").unwrap();
        assert_eq!(read_plasma_locale(), "en_ZA.UTF-8");
        let content = fs::read_to_string(tmp).unwrap();
        assert!(content.contains("[Formats]"));
        assert!(content.contains("LANG=en_ZA.UTF-8"));
        assert!(content.contains("[Translations]"));
        assert!(content.contains("LANGUAGE=en_ZA.UTF-8"));
        // Existing file with other keys: preserves them, updates locale.
        fs::write(
            tmp,
            "[Formats]\nLANG=en_GB.UTF-8\nuseDetailed=true\n\n[Translations]\nLANGUAGE=en_GB\n",
        )
        .unwrap();
        write_plasma_locale("af_ZA.UTF-8").unwrap();
        assert_eq!(read_plasma_locale(), "af_ZA.UTF-8");
        let content = fs::read_to_string(tmp).unwrap();
        assert!(content.contains("useDetailed=true"));
        assert!(content.contains("LANG=af_ZA.UTF-8"));
        assert!(content.contains("LANGUAGE=af_ZA.UTF-8"));
        // Invalid locale rejected, file untouched.
        assert!(write_plasma_locale("bad;locale").is_err());
        assert_eq!(read_plasma_locale(), "af_ZA.UTF-8");
        std::env::remove_var("DUCKNET_PLASMA_LOCALERC");
        let _ = fs::remove_file(tmp);
    }

    #[test]
    fn available_locales_lists() {
        // Host provides locales; expect at least one entry.
        let locales = list_available_locales();
        assert!(
            !locales.is_empty(),
            "expected at least one locale from localectl/locale -a"
        );
    }

    #[test]
    fn physical_ip_skips_vpn_after_lo() {
        // Physical eth0 after lo, VPN below.
        let out = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever\n\
                   2: eth0    inet 192.168.0.205/24 brd 192.168.0.255 scope global noprefixroute eth0\\       valid_lft forever preferred_lft forever\n\
                   3: CloudflareWARP    inet 172.16.0.2/32 scope global CloudflareWARP\\       valid_lft forever preferred_lft forever\n";
        assert_eq!(parse_physical_ipv4(out).as_deref(), Some("192.168.0.205"));
    }

    #[test]
    fn physical_ip_skips_vpn_first_tunnels_bridges() {
        let out = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever\n\
                   2: tun0    inet 10.8.0.6/24 scope global tun0\\       valid_lft forever preferred_lft forever\n\
                   3: nordlynx    inet 10.5.0.2/32 scope global nordlynx\\       valid_lft forever preferred_lft forever\n\
                   4: docker0    inet 172.17.0.1/16 scope global docker0\\       valid_lft forever preferred_lft forever\n\
                   5: veth1a2b3c@if4    inet 169.254.10.1/16 scope global veth1a2b3c\\       valid_lft forever preferred_lft forever\n\
                   6: wlp2s0    inet 192.168.1.20/24 brd 192.168.1.255 scope global dynamic wlp2s0\\       valid_lft forever preferred_lft forever\n";
        assert_eq!(parse_physical_ipv4(out).as_deref(), Some("192.168.1.20"));
    }

    #[test]
    fn physical_ip_link_local_last_resort() {
        let out = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever\n\
                   2: tun0    inet 10.8.0.6/24 scope global tun0\\       valid_lft forever preferred_lft forever\n\
                   3: eno1    inet 169.254.5.6/16 scope global eno1\\       valid_lft forever preferred_lft forever\n";
        assert_eq!(parse_physical_ipv4(out).as_deref(), Some("169.254.5.6"));
    }

    #[test]
    fn physical_ip_none_when_only_virtual() {
        let out = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever\n\
                   2: wg0-mullvad    inet 10.64.0.2/32 scope global wg0-mullvad\\       valid_lft forever preferred_lft forever\n";
        assert_eq!(parse_physical_ipv4(out), None);
    }

    #[test]
    fn virtual_iface_classification() {
        for v in ["tun0", "tap0", "wg0", "wg0-mullvad", "ppp0", "CloudflareWARP",
                  "tailscale0", "nordlynx", "protonvpn", "docker0", "br-abc123",
                  "veth1a2b3c@if4", "virbr0", "lo"] {
            assert!(is_virtual_iface(v), "{v} should be virtual");
        }
        for p in ["eth0", "eno1", "enp0s31f6", "wlp2s0", "wlan0", "end0"] {
            assert!(!is_virtual_iface(p), "{p} should be physical");
        }
    }
}
